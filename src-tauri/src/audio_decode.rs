//! Symphonia 直接解碼（逐封包串流），不經 rodio::Decoder。
//!
//! rodio 0.19 的 `ReadSeekSource::byte_len()` 固定回傳 None，symphonia 的 MP4 demuxer
//! 需要檔案長度，於是 rodio 在初始化時 `unreachable!` panic，m4a/mp4 永遠開不起來。
//! 這裡直接把 `std::fs::File` 交給 MediaSourceStream（File 會回報長度），
//! 也讓長錄音不必整首解進記憶體。

use rodio::Source;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once};
use std::thread;
use std::time::{Duration, Instant};
use crate::playback_pipeline::CancelHandle;
use crate::tools::locate;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder as SymDecoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

const MAX_DECODE_ERRORS: usize = 10; // SE7：解碼錯誤跨封包累計，超過即終止
const MAX_PACKET_BYTES: usize = 256 * 1024; // SE7：Symphonia 單封包上限
const FFPROBE_MAX_BYTES: usize = 64 * 1024; // SE6：ffprobe stdout 上限
const FFPROBE_TIMEOUT: Duration = Duration::from_secs(10); // SE6：ffprobe 總逾時

/// SE7：配置前封包上限預檢。在 SymStream::open() 之前呼叫，讀取檔案標頭
/// 遍歷所有音軌與片段的宣告封包大小；若任何封包宣告大小超過 MAX_PACKET_BYTES，
/// 回傳 Err（呼叫者不得呼叫 open()）。無法可靠遍歷某種格式的封包索引時，
/// 回傳 Ok（允許 open），依賴 refill 第二層防護（packet.buf().len() > MAX_PACKET_BYTES）
/// 在執行時捕獲超標封包。
pub fn precheck_packet_budget(path: &Path) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return Err("無法開啟檔案".to_string());
    };

    // 讀取前 16 bytes 判斷容器格式
    let mut header = [0u8; 16];
    if file.read_exact(&mut header).is_err() {
        return Err("無法讀取檔案標頭".to_string());
    }

    // MP4：ftyp box，掃描 box 結構尋找 stsz（Sample Size box）
    if &header[4..8] == b"ftyp" {
        return scan_mp4_stsz(&mut file);
    }

    // 其他格式（MKV、MP3、FLAC 等）：無法在配置前可靠遍歷封包索引，
    // fail-open，依賴 refill 第二層防護（packet.buf().len() > MAX_PACKET_BYTES）
    // 在執行時捕獲超標封包。
    Ok(())
}

/// 掃描 MP4 box 結構尋找 stsz（Sample Size box），檢查所有樣本大小是否超過上限。
/// 支援巢狀 container box（moov/meta/ilst 等）遞迴走訪，涵蓋所有音軌與片段。
fn scan_mp4_stsz(file: &mut std::fs::File) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};

    const LIMIT: u64 = 10 * 1024 * 1024; // 僅掃描前 10 MB
    scan_mp4_boxes(file, 0, LIMIT)
}

/// 遞迴走訪 MP4 box，尋找 stbox 並檢查樣本大小。
fn scan_mp4_boxes(file: &mut std::fs::File, start: u64, limit: u64) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut pos = start;
    let end = start.saturating_add(limit);
    let mut header = [0u8; 8];

    while pos < end {
        if file.seek(SeekFrom::Start(pos)).is_err() {
            return Err("MP4 box 定位失敗".to_string());
        }
        if file.read_exact(&mut header).is_err() {
            break;
        }
        let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        if size == 0 {
            // size == 0 表示本 box 佔用至檔案/父 box 結尾（MP4 規格），無法推導下一個 box 位置，
            // 停止掃描但不視為錯誤（此處之後的 box 無法可靠定位）。
            break;
        }
        if size < 8 {
            return Err("MP4 box 大小異常".to_string());
        }
        let btype = &header[4..8];

        // 若是 stsz box，解析樣本大小
        if btype == b"stsz" {
            return check_stsz_samples(file, pos, size);
        }

        // 若是 container box，遞迴走訪子 box
        let is_container = matches!(
            btype,
            b"moov" | b"meta" | b"ilst" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts"
                | b"mvex"
        );
        if is_container && size > 8 {
            let child_start = pos + 8;
            let child_len = size.saturating_sub(8);
            scan_mp4_boxes(file, child_start, child_len)?;
        }

        let next = size.saturating_sub(8);
        pos = pos.saturating_add(8).saturating_add(next);
        if pos > end {
            break;
        }
    }
    Ok(())
}

/// 檢查 stsz box 內的樣本大小是否超過 MAX_PACKET_BYTES。
fn check_stsz_samples(file: &mut std::fs::File, box_start: u64, box_size: u64) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};

    // stsz box 結構：size(4) + type(4) + version(1) + flags(3) + sample_size(4) + sample_count(4) + [sample_sizes]
    if file.seek(SeekFrom::Start(box_start + 8)).is_err() {
        return Err("無法定位 stsz 內容".to_string());
    }
    let mut ver_flags = [0u8; 4];
    if file.read_exact(&mut ver_flags).is_err() {
        return Err("無法讀取 stsz 版本與旗標".to_string());
    }
    let mut sample_size_buf = [0u8; 4];
    if file.read_exact(&mut sample_size_buf).is_err() {
        return Err("無法讀取 stsz sample_size".to_string());
    }
    let default_sample_size = u32::from_be_bytes(sample_size_buf);
    let mut sample_count_buf = [0u8; 4];
    if file.read_exact(&mut sample_count_buf).is_err() {
        return Err("無法讀取 stsz sample_count".to_string());
    }
    let sample_count = u32::from_be_bytes(sample_count_buf);

    // 若 default_sample_size > 0，所有樣本大小相同
    if default_sample_size > 0 {
        if default_sample_size as usize > MAX_PACKET_BYTES {
            return Err(format!(
                "MP4 單封包宣告大小 {} bytes 超過上限 {} bytes",
                default_sample_size, MAX_PACKET_BYTES
            ));
        }
        return Ok(());
    }

    // default_sample_size == 0：每個樣本有獨立大小，逐一讀取
    // 以 remaining（box 內剩餘位元組）為上限，避免讀過 box 邊界；sample_count 上限也受 remaining 限制，防止惡意 MP4 造成過長迴圈（DoS）。
    let remaining = box_size.saturating_sub(16); // 扣除已讀欄位
    let max_samples = (remaining / 4) as u32; // 每個樣本大小 4 bytes
    let read_limit = sample_count.min(max_samples);
    let mut read_count = 0u32;
    let mut buf = [0u8; 4];
    while read_count < read_limit {
        if file.read_exact(&mut buf).is_err() {
            return Err("讀取 stsz 樣本大小失敗".to_string());
        }
        let s = u32::from_be_bytes(buf);
        if s as usize > MAX_PACKET_BYTES {
            return Err(format!(
                "MP4 樣本 {} 單封包大小 {} bytes 超過上限 {} bytes",
                read_count, s, MAX_PACKET_BYTES
            ));
        }
        read_count += 1;
    }
    // 若 sample_count > max_samples，表示 box 內聲稱的樣本數超過實際空間，視為異常
    if sample_count > max_samples {
        return Err(format!(
            "MP4 stsz 宣告樣本數 {} 超過 box 可容納數 {}，結構異常",
            sample_count, max_samples
        ));
    }

    Ok(())
}

/// SE6：所有 ffprobe 呼叫點共用全域並行配額（同時最多 4 個進程）。
static FFPROBE_SLOTS_INIT: Once = Once::new();
static mut FFPROBE_SLOTS: Option<(Mutex<usize>, Condvar)> = None;

/// 取得 ffprobe 配額槽位；無可用槽位時在 deadline 內等待，超時則拒絕整筆探測。
fn ffprobe_acquire_slot(deadline: Instant) -> Result<FfprobeSlot, String> {
    let sem = unsafe {
        FFPROBE_SLOTS_INIT.call_once(|| {
            FFPROBE_SLOTS = Some((Mutex::new(4usize), Condvar::new()));
        });
        FFPROBE_SLOTS.as_ref().expect("ffprobe quota initialised")
    };
    let (lock, condvar) = sem;
    let mut slots = lock.lock().map_err(|e| format!("ffprobe 配額鎖定失敗: {e}"))?;
    while *slots == 0 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("ffprobe 並行配額滿載（逾時）".into());
        }
        let (next, _result) = condvar
            .wait_timeout(slots, remaining)
            .map_err(|e| format!("ffprobe 配額等待失敗: {e}"))?;
        if *next > 0 {
            slots = next;
        } else {
            return Err("ffprobe 並行配額滿載（逾時）".into());
        }
    }
    *slots -= 1;
    Ok(FfprobeSlot)
}

/// ffprobe 配額槽位，Drop 時歸還槽位並喚醒等待者。
struct FfprobeSlot;
impl Drop for FfprobeSlot {
    fn drop(&mut self) {
        if let Some((lock, condvar)) = unsafe { FFPROBE_SLOTS.as_ref() } {
            let mut slots = lock.lock().unwrap();
            *slots += 1;
            condvar.notify_one();
        }
    }
}

/// 執行 ffprobe，並實作 64 KB / 10 秒上限（SE6，所有探測呼叫點皆應使用此）。
pub fn ffprobe_json(ffprobe_path: &Path, file_path: &str) -> Result<serde_json::Value, String> {
    use std::io::Read;
    let deadline = Instant::now() + FFPROBE_TIMEOUT;
    // SE6：全域四程序並行配額，超過則等待或拒絕
    let _slot = ffprobe_acquire_slot(deadline)?;
    let mut child = match Command::new(ffprobe_path)
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format", "-protocol_whitelist", "file,pipe", file_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("ffprobe 執行失敗: {e}")),
    };
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => return Err("ffprobe 無 stdout".into()),
    };
    let mut stderr = child.stderr.take();
    let mut out = Vec::new();

    // SE6：使用獨立線程讀取 stdout，主線程用 recv_timeout 等待，避免阻塞讀取導致 deadline 無法生效
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() { break; }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(Vec::new()); // 結束信號
        });

    // 並行讀取並清洗 stderr，避免 ffprobe 因 stderr 緩衝區滿而阻塞
    let stderr_thread = crate::open_with::spawn_stderr_sanitizer(stderr, None);

    'read: loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err("ffprobe 逾時（10 秒）".into());
        }
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(chunk) if chunk.is_empty() => break 'read,
            Ok(chunk) => {
                let remain = FFPROBE_MAX_BYTES.saturating_sub(out.len());
                if remain == 0 {
                    let _ = child.kill();
                    return Err("ffprobe 輸出超過 64 KB".into());
                }
                let take = chunk.len().min(remain);
                out.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    let _ = child.kill();
                    return Err("ffprobe 輸出超過 64 KB".into());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue 'read,
            Err(_) => break 'read,
        }
    }

    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    let _ = child.wait();
    let text = String::from_utf8_lossy(&out);
    serde_json::from_str(&text).map_err(|e| format!("ffprobe JSON 解析失敗: {e}"))
}

pub struct SymStream {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    buf: Option<SampleBuffer<f32>>,
    buf_frames: u64,
    pos: usize,
    len: usize,
    skip_samples: usize,
    channels: u16,
    sample_rate: u32,
    duration_ms: Option<i64>,
    finished: bool,
    /// SE7：解碼錯誤跨封包累計
    decode_errors: usize,
}

impl SymStream {
    /// 開檔並（可選）精準跳到 start_ms。回傳時已解出第一個封包，channels / sample_rate 可信。
    pub fn open(path: &Path, start_ms: i64) -> Result<Self, String> {
        // SE7：配置前封包上限預檢，作為 open() 內部的第二道防禦線；
        // 直接呼叫 open() 的所有路徑皆先經過此預檢，超標則拒絕不繼續開檔。
        precheck_packet_budget(path)?;
        let file = File::open(path).map_err(|e| format!("開啟檔案失敗: {e}"))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(&ext.to_ascii_lowercase());
        }
        let format_opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &format_opts, &MetadataOptions::default())
            .map_err(|e| format!("無法辨識格式: {e}"))?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| "檔案內沒有可解碼的音軌".to_string())?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(|e| format!("不支援的編碼: {e}"))?;
        let duration_ms = params.time_base.zip(params.n_frames).map(|(tb, n)| {
            let t = tb.calc_time(n);
            (t.seconds as f64 * 1000.0 + t.frac * 1000.0) as i64
        });
        let mut s = Self {
            format,
            decoder,
            track_id,
            time_base: params.time_base,
            buf: None,
            buf_frames: 0,
            pos: 0,
            len: 0,
            skip_samples: 0,
            channels: params.channels.map(|c| c.count() as u16).unwrap_or(0),
            sample_rate: params.sample_rate.unwrap_or(0),
            duration_ms,
            finished: false,
            decode_errors: 0,
        };
        if start_ms > 0 {
            s.seek_ms(start_ms)?;
        }
        if !s.refill() && (s.channels == 0 || s.sample_rate == 0) {
            return Err("未解碼到任何音訊樣本".into());
        }
        if s.channels == 0 || s.sample_rate == 0 {
            return Err("無法取得聲道或取樣率".into());
        }
        Ok(s)
    }

    pub fn duration_ms(&self) -> Option<i64> {
        self.duration_ms
    }

    fn seek_ms(&mut self, ms: i64) -> Result<(), String> {
        let time = Time::from(ms.max(0) as f64 / 1000.0);
        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track_id) })
            .map_err(|e| format!("跳轉失敗: {e}"))?;
        self.decoder.reset();
        self.pos = 0;
        self.len = 0;
        self.finished = false;
        // Accurate 模式落在 required_ts 之前的封包，多出來的樣本要丟掉。
        let delta = seeked.required_ts.saturating_sub(seeked.actual_ts);
        self.skip_samples = match (self.time_base, self.sample_rate, self.channels) {
            (Some(tb), rate, ch) if delta > 0 && rate > 0 && ch > 0 => {
                let t = tb.calc_time(delta);
                let secs = t.seconds as f64 + t.frac;
                (secs * rate as f64) as usize * ch as usize
            }
            _ => 0,
        };
        Ok(())
    }

    /// 解下一個封包到 buf。回傳 false 代表檔案結束或無法繼續。
    fn refill(&mut self) -> bool {
        if self.finished {
            return false;
        }
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => {
                    self.finished = true;
                    return false;
                }
            };
            if packet.buf().len() > MAX_PACKET_BYTES {
                self.finished = true;
                return false;
            }
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    if self.channels == 0 {
                        self.channels = spec.channels.count() as u16;
                    }
                    if self.sample_rate == 0 {
                        self.sample_rate = spec.rate;
                    }
                    let frames = decoded.capacity() as u64;
                    if self.buf.is_none() || frames > self.buf_frames {
                        self.buf = Some(SampleBuffer::<f32>::new(frames, spec));
                        self.buf_frames = frames;
                    }
                    let buf = self.buf.as_mut().unwrap();
                    buf.copy_interleaved_ref(decoded);
                    let len = buf.len();
                    if self.skip_samples >= len {
                        self.skip_samples -= len;
                        continue;
                    }
                    self.pos = self.skip_samples;
                    self.skip_samples = 0;
                    self.len = len;
                    if self.pos < self.len {
                        return true;
                    }
                }
                Err(SymError::DecodeError(_)) => {
                    self.decode_errors += 1;
                    if self.decode_errors >= MAX_DECODE_ERRORS {
                        self.finished = true;
                        return false;
                    }
                }
                Err(_) => {
                    self.finished = true;
                    return false;
                }
            }
        }
    }
}

impl Iterator for SymStream {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.len && !self.refill() {
            return None;
        }
        let s = self.buf.as_ref()?.samples()[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for SymStream {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> {
        self.duration_ms.map(|ms| Duration::from_millis(ms.max(0) as u64))
    }
}
/// Ffmpeg 串流解碼器（P0-2）
/// 
/// 使用 ffprobe 讀取中繼資料，ffmpeg 解碼輸出 f32le PCM。
/// 子程序管理：stdout 由專屬讀取執行緒送進有界通道，stderr 由另一條執行緒持續讀取。
/// 取消與回收：共享 CancelHandle，先設旗標並 kill，wait 與 join 交給回收執行緒。
#[derive(Debug)]
pub struct FfmpegStream {
    pub(crate) ffmpeg_path: std::path::PathBuf,
    pub(crate) file_path: String,
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) duration_ms: i64,
    pub(crate) cancel: Arc<CancelHandle>,
    pub(crate) rx: crossbeam_channel::Receiver<Vec<f32>>,
    /// SE4/SP4：逐個樣本輸出的內部緩衝
    buffer: Vec<f32>,
    pos: usize,
    pub(crate) _stdout_thread: Option<thread::JoinHandle<()>>,
    _stderr_thread: Option<thread::JoinHandle<()>>,
}

impl Clone for FfmpegStream {
    fn clone(&self) -> Self {
        Self {
            ffmpeg_path: self.ffmpeg_path.clone(),
            file_path: self.file_path.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            duration_ms: self.duration_ms,
            cancel: self.cancel.clone(),
            rx: self.rx.clone(),
            buffer: Vec::new(),
            pos: 0,
            _stdout_thread: None,
            _stderr_thread: None,
        }
    }
}

impl FfmpegStream {
    /// 建立新的 FfmpegStream
    pub fn new(
        ffmpeg_path: &Path,
        file_path: &str,
        sample_rate: u32,
        channels: u16,
        duration_ms: i64,
    ) -> Result<Self, String> {
        // 先用 ffprobe 讀取精確資訊（經過 locate，不繞過；SE6：64 KB / 10 秒上限）
        let ffprobe_path = locate("ffprobe.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffprobe")?;
        let value = crate::audio_decode::ffprobe_json(&ffprobe_path, file_path)?;

        let streams = value["streams"].as_array().ok_or("無音訊串流")?;
        let audio_stream = streams.iter().find(|s| s["codec_type"] == "audio").ok_or("無音訊串流")?;
        
        let codec = audio_stream["codec_name"].as_str().unwrap_or("");
        let sr = audio_stream["sample_rate"].as_str().unwrap_or("0").parse().unwrap_or(sample_rate);
        let ch = audio_stream["channels"].as_u64().unwrap_or(0) as u16;
        let dur = value["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok());
        let dur_ms = dur.map(|d| (d * 1000.0) as i64).unwrap_or(duration_ms);
        
        // DSD 特殊處理
        let (final_sr, final_ch) = if codec.starts_with("dsd") {
            let dsd_rate = sr;
            let pcm_rate = (dsd_rate / 32).min(176400);
            (pcm_rate, 2)
        } else {
            (sr.max(1), ch.max(1))
        };
        
        let cancel = Arc::new(CancelHandle::new(Duration::from_secs(10)));
        let (tx, rx) = crossbeam_channel::bounded(128); // 2 MiB / 16 KiB = 128 chunks
        
        let file_path_owned = file_path.to_string();
        let ffmpeg_path_owned = ffmpeg_path.to_path_buf();
        let cancel_stdout = cancel.clone();
        let cancel_stderr = cancel.clone();
        
        // stdout 讀取執行緒
        let stdout_thread = thread::spawn(move || {
            let mut child = match Command::new(&ffmpeg_path_owned)
                            .args([
                                "-nostdin",
                                "-v", "error",
                                "-protocol_whitelist", "file,pipe", // SE11：本機 ffmpeg 只允許 file/pipe
                                "-ss", "0",
                                "-i", &file_path_owned,
                                "-map", "0:a:0",
                                "-f", "f32le",
                                "-ac", &final_ch.to_string(),
                                "-ar", &final_sr.to_string(),
                                "-"
                            ])
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                        {
                            Ok(child) => child,
                            Err(e) => {
                                eprintln!("ffmpeg spawn failed: {e}");
                                return;
                            }
                        };
            
                        // 先取出 stdout 和 stderr，再移動 child
                        let mut stdout = match child.stdout.take() {
                            Some(out) => out,
                            None => return,
                        };
            
                        let mut stderr = child.stderr.take();
            
                        cancel_stdout.set_child(child);


                        // stderr 讀取執行緒：使用共用 helper（SE3-4）
                        let stderr_thread = crate::open_with::spawn_stderr_sanitizer(stderr, Some(cancel_stderr));

                        // 讀取 stdout 並送入通道
                        const CHUNK_SAMPLES: usize = 4096; // 16 KiB @ 48kHz stereo f32
                        let mut chunk = Vec::with_capacity(CHUNK_SAMPLES * 4);
                        let mut buf = [0u8; 8192];
                        
                        
                        loop {
                if cancel_stdout.is_cancelled() {
                    break;
                }
                
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        chunk.extend_from_slice(&buf[..n]);
                        while chunk.len() >= CHUNK_SAMPLES * 4 {
                            let chunk_bytes: Vec<u8> = chunk.drain(..CHUNK_SAMPLES * 4).collect();
                            let samples: Vec<f32> = chunk_bytes
                                .chunks_exact(4)
                                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                                .collect();

                            // 嘗試發送，帶背壓重試
                            loop {
                                if cancel_stdout.is_cancelled() {
                                    break;
                                }
                                match tx.try_send(samples.clone()) {
                                    Ok(()) => break,
                                    Err(crossbeam_channel::TrySendError::Full(_)) => {
                                        thread::sleep(Duration::from_millis(5));
                                        continue;
                                    }
                                    Err(crossbeam_channel::TrySendError::Disconnected(_)) => break,
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }

        }
        let _ = tx.send(Vec::new()); // 結束信號
        let _ = stderr_thread.join();
                    });
        Ok(Self {
            ffmpeg_path: ffmpeg_path.to_path_buf(),
            file_path: file_path.to_string(),
            sample_rate: final_sr,
            channels: final_ch,
            duration_ms: dur_ms,
            cancel,
            rx,
            buffer: Vec::new(),
            pos: 0,
            _stdout_thread: Some(stdout_thread),
            _stderr_thread: None,
        })
    }

    /// 帶 seek 位置開啟：重新建立帶 `-ss` 的 ffmpeg 子程序，回傳逐個樣本的迭代器（SP4）。
    pub fn open_with_seek(&self, start_ms: i64) -> Result<Box<dyn Iterator<Item = f32> + Send>, String> {
        let ffmpeg_path = self.ffmpeg_path.clone();
        let file_path = self.file_path.clone();
        let channels = self.channels as usize;
        let sample_rate = self.sample_rate.max(1);
        let cancel = self.cancel.clone();
        let (tx, rx) = crossbeam_channel::bounded(128);

        thread::spawn(move || {
            let Ok(mut child) = Command::new(&ffmpeg_path)
                .args([
                    "-nostdin",
                    "-v", "error",
                    "-protocol_whitelist", "file,pipe", // SE11：本機 ffmpeg 只允許 file/pipe
                    "-ss", &format!("{:.3}", start_ms as f64 / 1000.0),
                    "-i", &file_path,
                    "-map", "0:a:0",
                    "-f", "f32le",
                    "-ac", &channels.to_string(),
                    "-ar", &sample_rate.to_string(),
                    "-",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn() else {
                let _ = tx.send(Vec::new());
                return;
            };
            let mut stdout = match child.stdout.take() {
                Some(o) => o,
                None => { let _ = tx.send(Vec::new()); return; },
            };

            // stderr 讀取執行緒：使用共用 helper（SE3-4）
            let mut stderr = child.stderr.take();
            let cancel_stderr = cancel.clone();
            let _stderr_thread = crate::open_with::spawn_stderr_sanitizer(stderr, Some(cancel_stderr));

            cancel.set_child(child);
            let mut buf = [0u8; 8192];
            let mut chunk = Vec::with_capacity(4096 * 4);
            loop {
                if cancel.is_cancelled() { break; }
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        chunk.extend_from_slice(&buf[..n]);
                        while chunk.len() >= 4096 * 4 {
                            let chunk_bytes: Vec<u8> = chunk.drain(..4096 * 4).collect();
                            let samples: Vec<f32> = chunk_bytes
                                .chunks_exact(4)
                                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                                .collect();
                            if tx.send(samples).is_err() { return; }
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(Vec::new());
        });

        Ok(Box::new(SeekIter { rx, buffer: Vec::new(), pos: 0 }))
    }

    pub fn sample_rate(&self) -> u32 { self.sample_rate }
    pub fn channels(&self) -> u16 { self.channels }
    pub fn duration_ms(&self) -> Option<i64> {
        if self.duration_ms > 0 { Some(self.duration_ms) } else { None }
    }
    
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for FfmpegStream {
    fn drop(&mut self) {
        self.cancel.cancel();
        // 執行緒 join 交給回收執行緒
    }
}

impl Iterator for FfmpegStream {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        // SP4：收到資料回傳樣本（不再無條件回 None）
        loop {
            if self.pos < self.buffer.len() {
                let s = self.buffer[self.pos];
                self.pos += 1;
                return Some(s);
            }
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(chunk) => {
                    if chunk.is_empty() {
                        return None; // 結束信號
                    }
                    self.buffer = chunk;
                self.pos = 0;
                }
                Err(_) => return None,
            }
        }
    }
}

/// 用於 `open_with_seek` 重開 ffmpeg 後的逐個樣本迭代器（SP4）。
struct SeekIter {
    rx: crossbeam_channel::Receiver<Vec<f32>>,
    buffer: Vec<f32>,
    pos: usize,
}

impl Iterator for SeekIter {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        loop {
            if self.pos < self.buffer.len() {
                let s = self.buffer[self.pos];
                self.pos += 1;
                return Some(s);
            }
            // SE8：非阻塞消費，暫時無資料時回傳靜音而非 None，避免音訊回呼提前結束
            match self.rx.try_recv() {
                Ok(chunk) => {
                    if chunk.is_empty() {
                        return None;
                    }
                    self.buffer = chunk;
                    self.pos = 0;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    return Some(0.0);
                }
                Err(_) => return None,
            }
        }
    }
}

impl Source for FfmpegStream {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> {
        self.duration_ms().map(|ms| Duration::from_millis(ms.max(0) as u64))
    }

}
