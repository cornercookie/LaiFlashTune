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
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use crate::playback_pipeline::CancelHandle;
use crate::tools::locate;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder as SymDecoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream, ReadBytes};
use symphonia::core::probe::Instantiate;
use symphonia::core::units::{Time, TimeBase};

const MAX_DECODE_ERRORS: usize = 10; // SE7：解碼錯誤跨封包累計，超過即終止
const MAX_PACKET_BYTES: usize = 256 * 1024; // SE7：Symphonia 單封包上限
const FFPROBE_MAX_BYTES: usize = 64 * 1024; // SE6：ffprobe stdout 上限
const FFPROBE_TIMEOUT: Duration = Duration::from_secs(10); // SE6：ffprobe 總逾時

/// C2a：解碼錯誤計數的處理決策。Recover＝吞掉此次可恢復錯誤繼續解碼；Stop＝超出預算，終止串流。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodeErrorDecision {
    Recover,
    Stop,
}

/// SE7/C2a：可恢復解碼錯誤的跨封包計數器（單元測試接縫）。
/// 成功解碼不重置計數；累計超過 MAX_DECODE_ERRORS 即回報終止。
#[derive(Debug, Default)]
struct DecodeErrorBudget {
    count: usize,
}

impl DecodeErrorBudget {
    /// 記錄一次可恢復的解碼錯誤，回傳此次應 Recover 還是 Stop。
    fn on_decode_error(&mut self) -> DecodeErrorDecision {
        self.count = self.count.saturating_add(1);
        if self.count > MAX_DECODE_ERRORS {
            DecodeErrorDecision::Stop
        } else {
            DecodeErrorDecision::Recover
        }
    }

    /// 記錄一次成功解碼：依 SE7 語意跨封包累計，成功不重置計數。
    fn on_decode_success(&mut self) {}
}

/// SE7：配置前封包上限預檢。在 SymStream::open() 之前呼叫，讀取檔案標頭
/// 遍歷所有音軌與片段的宣告封包大小；若任何封包宣告大小超過 MAX_PACKET_BYTES，
/// 回傳 Err（呼叫者不得呼叫 open()）。無法可靠遍歷某種格式的封包索引時，
/// 回傳 Ok（允許 open），依賴 refill 第二層防護（packet.buf().len() > MAX_PACKET_BYTES）
/// 在執行時捕獲超標封包。
pub fn precheck_packet_budget(file: &mut File) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};

    // 保存進入時的游標；無論既有判讀成功或錯誤，都嘗試還原進入時的游標，
    // 避免共用同一個 File handle 的呼叫端（player.rs、playback_pipeline.rs、
    // next_track.rs）發現游標被挪移。
    let entry_pos = file.stream_position().map_err(|e| format!("無法取得進入游標: {e}"))?;
    if file.seek(SeekFrom::Start(0)).is_err() {
        return Err("無法定位檔案開頭".to_string());
    }

    let result = (|| -> Result<(), String> {
        // 讀取前 16 bytes 判斷容器格式
        let mut header = [0u8; 16];
        if file.read_exact(&mut header).is_err() {
            return Err("無法讀取檔案標頭".to_string());
        }

        // MP4：ftyp box，掃描 box 結構尋找 stsz（Sample Size box）
        if &header[4..8] == b"ftyp" {
            scan_mp4_stsz(file)?;
        }

        // 其他格式（MKV、MP3、FLAC 等）：無法在配置前可靠遍歷封包索引，
        // fail-open，依賴 refill 第二層防護（packet.buf().len() > MAX_PACKET_BYTES）
        // 在執行時捕獲超標封包。
        Ok(())
    })();

    // 還原進入時的游標；定位或還原失敗回傳錯誤
    if file.seek(SeekFrom::Start(entry_pos)).is_err() {
        return Err("無法還原檔案游標".to_string());
    }
    result
}

/// 掃描 MP4 box 結構尋找 stsz（Sample Size box），檢查所有樣本大小是否超過上限。
/// 支援巢狀 container box（moov/meta/ilst 等）遞迴走訪，涵蓋所有音軌與片段。
fn scan_mp4_stsz(file: &mut File) -> Result<(), String> {
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

/// SE6：掃描與播放共用最多四個 ffprobe 進程。
static FFPROBE_SLOTS: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
const FFPROBE_CANCEL_POLL: Duration = Duration::from_millis(50);
const FFPROBE_CANCELLED: &str = "ffprobe 準備已撤銷";

fn ffprobe_cancelled(cancelled: Option<&AtomicBool>) -> bool {
    cancelled.is_some_and(|flag| flag.load(Ordering::Acquire))
}

fn ffprobe_wait_duration(remaining: Duration, cancelled: Option<&AtomicBool>) -> Duration {
    if cancelled.is_some() { remaining.min(FFPROBE_CANCEL_POLL) } else { remaining }
}

fn ffprobe_acquire_slot(deadline: Instant, cancelled: Option<&AtomicBool>) -> Result<FfprobeSlot<'static>, String> {
    let quota = FFPROBE_SLOTS.get_or_init(|| (Mutex::new(4), Condvar::new()));
    ffprobe_acquire_from(quota, deadline, cancelled)
}

fn ffprobe_acquire_from<'a>(
    quota: &'a (Mutex<usize>, Condvar),
    deadline: Instant,
    cancelled: Option<&AtomicBool>,
) -> Result<FfprobeSlot<'a>, String> {
    let (lock, available) = quota;
    let mut slots = lock.lock().map_err(|_| "ffprobe 配額鎖定失敗")?;
    loop {
        if ffprobe_cancelled(cancelled) {
            return Err(FFPROBE_CANCELLED.to_string());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("ffprobe 並行配額滿載（逾時）".to_string());
        }
        if *slots > 0 {
            *slots -= 1;
            return Ok(FfprobeSlot(quota));
        }
        slots = available
            .wait_timeout(slots, ffprobe_wait_duration(remaining, cancelled))
            .map_err(|_| "ffprobe 配額等待失敗")?
            .0;
    }
}

struct FfprobeSlot<'a>(&'a (Mutex<usize>, Condvar));

impl Drop for FfprobeSlot<'_> {
    fn drop(&mut self) {
        let (lock, available) = self.0;
        let mut slots = lock.lock().unwrap_or_else(|error| error.into_inner());
        *slots += 1;
        available.notify_all();
    }
}

/// 執行 ffprobe，並實作 64 KB / 10 秒上限（SE6，所有探測呼叫點皆應使用此）。
const FFPROBE_COMMON_ARGS: [&str; 4] = ["-v", "error", "-print_format", "json"];
/// C2e v0.13：ffprobe 共用協定白名單（本機只允許 file/pipe）。
const FFPROBE_PROTOCOL_ARGS: [&str; 2] = ["-protocol_whitelist", "file,pipe"];
/// C2e v0.13：掃描器用完整輸出選擇參數（維持原有行為，僅 ffprobe_json 使用）。
const FFPROBE_SCANNER_SELECTION_ARGS: [&str; 2] = ["-show_streams", "-show_format"];
/// C2e v0.13：播放用窄化輸出選擇參數（固定項目；不得含 -show_streams / -show_format，
/// 只取 codec_type/codec_name/sample_rate/channels 與 format.duration）。
const FFPROBE_PLAYBACK_SELECTION_ARGS: [&str; 2] =
    ["-show_entries", "stream=codec_type,codec_name,sample_rate,channels:format=duration"];

/// C2e v0.13：組出完整 ffprobe 參數：共用前綴 + 選擇參數 + 共用白名單 + 檔案路徑。
fn ffprobe_args(selection: &[&str], file_path: &str) -> Vec<String> {
    let mut args: Vec<String> = FFPROBE_COMMON_ARGS.iter().map(|a| a.to_string()).collect();
    args.extend(selection.iter().map(|a| a.to_string()));
    args.extend(FFPROBE_PROTOCOL_ARGS.iter().map(|a| a.to_string()));
    args.push(file_path.to_string());
    args
}

/// C2e：ffprobe 標準輸出訊息。讀取執行緒與主執行緒經有界通道交握，
/// 嚴格區分資料塊、真正的 EOF 與讀取失敗；斷線絕不被當作 EOF。
enum FfprobeStdoutMsg {
    /// 一筆讀取資料，長度不超過 8192 bytes
    Data(Vec<u8>),
    /// 真正的 EOF：read 回傳 Ok(0)
    Eof,
    /// 讀取失敗（非 Interrupted 可重試的錯誤）
    ReadError,
}



fn ffprobe_read_stdout(mut stdout: std::process::ChildStdout, tx: std::sync::mpsc::SyncSender<FfprobeStdoutMsg>) {
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => {
                    // 真正的零長度讀取 = EOF，僅發送 Eof，不發送空資料
                    let _ = tx.send(FfprobeStdoutMsg::Eof);
                    return;
                }
                Ok(n) => {
                    // 接收方已斷開時不視為 EOF，直接結束執行緒
                    if tx.send(FfprobeStdoutMsg::Data(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    let _ = tx.send(FfprobeStdoutMsg::ReadError);
                    return;
                }
            }
        }
}


fn ffprobe_wait<T>(
    rx: &std::sync::mpsc::Receiver<T>,
    deadline: Instant,
    cancelled: Option<&AtomicBool>,
    disconnected: &'static str,
) -> Result<T, &'static str> {
    use std::sync::mpsc::RecvTimeoutError;

    loop {
        if ffprobe_cancelled(cancelled) {
            return Err(FFPROBE_CANCELLED);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("ffprobe 逾時");
        }
        match rx.recv_timeout(ffprobe_wait_duration(remaining, cancelled)) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(disconnected),
        }
    }
}

fn ffprobe_collect(
    rx: &std::sync::mpsc::Receiver<FfprobeStdoutMsg>,
    deadline: Instant,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<u8>, &'static str> {
    let mut output = Vec::with_capacity(FFPROBE_MAX_BYTES);
    loop {
        match ffprobe_wait(rx, deadline, cancelled, "ffprobe 輸出提前結束")? {
            FfprobeStdoutMsg::Data(chunk) => {
                if chunk.len() > FFPROBE_MAX_BYTES - output.len() {
                    return Err("ffprobe 輸出超過 64 KiB");
                }
                output.extend_from_slice(&chunk);
            }
            FfprobeStdoutMsg::Eof => return Ok(output),
            FfprobeStdoutMsg::ReadError => return Err("ffprobe 讀取失敗"),
        }
    }
}

fn ffprobe_run(
    ffprobe_path: &Path,
    file_path: &str,
    selection: &[&str],
    cancelled: Option<&Arc<AtomicBool>>,
) -> Result<serde_json::Value, String> {
    let deadline = Instant::now() + FFPROBE_TIMEOUT;
    let cancel_flag = cancelled.map(Arc::as_ref);
    let slot = ffprobe_acquire_slot(deadline, cancel_flag)?;
    let path = ffprobe_path.to_path_buf();
    let args = ffprobe_args(selection, file_path);
    let worker_cancelled = cancelled.cloned();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);

    thread::Builder::new().name("ffprobe-supervisor".into()).spawn(move || {
        let result = ffprobe_capture(&path, &args, deadline, worker_cancelled.as_deref());
        drop(slot);
        let _ = tx.send(result);
    }).map_err(|_| "無法啟動 ffprobe 工作")?;

    ffprobe_wait(&rx, deadline, cancel_flag, "ffprobe 工作失敗")?
}

fn ffprobe_finish(
    child: &mut std::process::Child,
    deadline: Instant,
    captured: Result<Vec<u8>, &'static str>,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<u8>, &'static str> {
    let mut result = captured;
    while result.is_ok() && Instant::now() < deadline {
        if ffprobe_cancelled(cancelled) {
            result = Err(FFPROBE_CANCELLED);
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return result,
            Ok(Some(_)) => return Err("ffprobe 子行程非正常退出"),
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => result = Err("ffprobe 等待失敗"),
        }
    }
    let _ = child.kill();
    loop {
        match child.wait() {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(100));
                let _ = child.kill();
            }
        }
    }
    Err(result.err().unwrap_or("ffprobe 逾時"))
}

fn ffprobe_capture(
    path: &Path,
    args: &[String],
    deadline: Instant,
    cancelled: Option<&AtomicBool>,
) -> Result<serde_json::Value, String> {
    if ffprobe_cancelled(cancelled) {
        return Err(FFPROBE_CANCELLED.to_string());
    }
    if Instant::now() >= deadline {
        return Err("ffprobe 逾時".into());
    }
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "無法執行 ffprobe")?;
    let Some(stdout) = child.stdout.take() else {
        let _ = ffprobe_finish(&mut child, deadline, Err("ffprobe 無標準輸出"), cancelled);
        return Err("ffprobe 無標準輸出".into());
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(2);
    let stdout_thread = std::thread::Builder::new().spawn(move || ffprobe_read_stdout(stdout, tx));
    let stdout_thread = match stdout_thread {
        Ok(worker) => worker,
        Err(_) => {
            let _ = ffprobe_finish(&mut child, deadline, Err("無法啟動 ffprobe 讀取作業"), cancelled);
            return Err("無法啟動 ffprobe 讀取作業".into());
        }
    };
    let stderr = child.stderr.take();
    let stderr_thread = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::open_with::spawn_stderr_sanitizer(stderr, None)
    }));
    let captured = if stderr_thread.is_ok() {
        ffprobe_collect(&rx, deadline, cancelled)
    } else {
        Err("無法啟動 ffprobe 診斷讀取作業")
    };
    drop(rx);
    let captured = ffprobe_finish(&mut child, deadline, captured, cancelled);
    let stdout_joined = stdout_thread.join().is_ok();
    let stderr_joined = stderr_thread.map(|worker| worker.join().is_ok()).unwrap_or(false);
    let bytes = captured?;
    if !stdout_joined || !stderr_joined {
        return Err("ffprobe 讀取作業失敗".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "ffprobe JSON 解析失敗".into())
}

pub fn ffprobe_json(ffprobe_path: &Path, file_path: &str) -> Result<serde_json::Value, String> {
    ffprobe_run(ffprobe_path, file_path, &FFPROBE_SCANNER_SELECTION_ARGS, None)
}

/// C2e v0.13：播放路徑專用窄化 ffprobe：固定 -show_entries 只取播放所需欄位，
/// 不帶 -show_streams / -show_format；沿用與 ffprobe_json 相同的 helper 與上限
/// （SE6：64 KB / 10 秒、並行配額、file/pipe 白名單）。
pub fn ffprobe_playback_json(ffprobe_path: &Path, file_path: &str) -> Result<serde_json::Value, String> {
    ffprobe_playback_json_for_request(ffprobe_path, file_path, None)
}

pub(crate) fn ffprobe_playback_json_for_request(
    ffprobe_path: &Path,
    file_path: &str,
    cancelled: Option<&Arc<AtomicBool>>,
) -> Result<serde_json::Value, String> {
    ffprobe_run(ffprobe_path, file_path, &FFPROBE_PLAYBACK_SELECTION_ARGS, cancelled)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymReadFailure {
    PacketBudget,
    DecodeBudget,
    Decode,
}

#[derive(Debug)]
pub(crate) enum SymOpenFailure {
    Precheck(String),
    Terminal(SymReadFailure),
    Open(String),
}

impl SymOpenFailure {
    fn from_decoder(error: SymError, context: &str) -> Self {
        match error {
            SymError::LimitError(_) => Self::Terminal(SymReadFailure::PacketBudget),
            other => Self::Open(format!("{context}: {other}")),
        }
    }

    pub(crate) fn allows_fallback(&self) -> bool {
        matches!(self, Self::Open(_))
    }
}

impl std::fmt::Display for SymOpenFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Precheck(reason) | Self::Open(reason) => formatter.write_str(reason),
            Self::Terminal(SymReadFailure::PacketBudget) => formatter.write_str("音訊封包超過允許上限"),
            Self::Terminal(SymReadFailure::DecodeBudget) => formatter.write_str("音訊解碼錯誤次數超過上限"),
            Self::Terminal(SymReadFailure::Decode) => formatter.write_str("音訊解碼失敗"),
        }
    }
}

impl From<String> for SymOpenFailure {
    fn from(reason: String) -> Self {
        Self::Open(reason)
    }
}

impl From<&str> for SymOpenFailure {
    fn from(reason: &str) -> Self {
        Self::Open(reason.into())
    }
}

// The enabled Symphonia probe registers ID3v2 as its sole external metadata reader.
fn open_playback_format(
    mut source: MediaSourceStream,
    options: &FormatOptions,
) -> Result<Box<dyn FormatReader>, SymOpenFailure> {
    let probe = symphonia::default::get_probe();
    loop {
        match probe.next(&mut source)
            .map_err(|error| SymOpenFailure::from_decoder(error, "無法辨識格式"))?
        {
            Instantiate::Format(open) => return open(source, options)
                .map_err(|error| SymOpenFailure::from_decoder(error, "無法辨識格式")),
            Instantiate::Metadata(_) => skip_playback_id3v2(&mut source)?,
        }
    }
}

fn skip_playback_id3v2(source: &mut MediaSourceStream) -> Result<(), SymOpenFailure> {
    use std::io::{Seek, SeekFrom};

    let invalid = || SymOpenFailure::Precheck("ID3v2 標頭或範圍無效".into());
    let mut header = [0; 10];
    source.read_exact(&mut header).map_err(|_| invalid())?;
    if &header[..3] != b"ID3"
        || !(2..=4).contains(&header[3])
        || header[4] == 0xff
        || header[6..].iter().any(|byte| byte & 0x80 != 0)
    {
        return Err(invalid());
    }
    let body_len = header[6..].iter().fold(0u64, |len, byte| (len << 7) | u64::from(*byte));
    let footer_len = if header[3] == 4 && header[5] & 0x10 != 0 { 10 } else { 0 };
    let end = source.pos().checked_add(body_len + footer_len)
        .filter(|end| source.byte_len().is_some_and(|len| *end <= len))
        .ok_or_else(invalid)?;
    if footer_len > 0 {
        source.seek(SeekFrom::Start(end - footer_len)).map_err(|_| invalid())?;
        let mut footer = [0; 10];
        source.read_exact(&mut footer).map_err(|_| invalid())?;
        if &footer[..3] != b"3DI" || footer[3..] != header[3..] {
            return Err(invalid());
        }
    } else {
        source.seek(SeekFrom::Start(end)).map_err(|_| invalid())?;
    }
    Ok(())
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
    terminal_failure: Option<SymReadFailure>,
    /// SE7：解碼錯誤跨封包累計（C2a：決策邏輯移入 DecodeErrorBudget）
    decode_errors: DecodeErrorBudget,
    preparation_slot: Option<std::sync::Weak<crate::player::PcmCaptureSlot>>,
}

impl SymStream {
    /// 開檔並（可選）精準跳到 start_ms。回傳時已解出第一個封包，channels / sample_rate 可信。
    /// 從已驗證的 File handle 探測實際格式，避免重新開檔競態。
    pub fn open(file: &mut File, path: &Path, start_ms: i64) -> Result<Self, String> {
        Self::open_checked(file, path, start_ms).map_err(|error| error.to_string())
    }

    pub(crate) fn open_checked(file: &mut File, _path: &Path, start_ms: i64) -> Result<Self, SymOpenFailure> {
        use std::io::{Seek, SeekFrom};

        // SE7：配置前封包上限預檢，作為 open() 內部的第二道防禦線；
        // 直接呼叫 open() 的所有路徑皆先經過此預檢，超標則拒絕不繼續開檔。
        precheck_packet_budget(file).map_err(SymOpenFailure::Precheck)?;
        // 共用同一個 File handle：預檢已將游標還原，在此明確定位至 0 再複製，
        // 讓 MediaSourceStream 從檔案開頭開始讀取。
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("無法定位檔案開頭: {e}"))?;
        let mss = MediaSourceStream::new(Box::new(file.try_clone().map_err(|e| format!("複製 handle 失敗: {e}"))?), Default::default());
        let format_opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let format = open_playback_format(mss, &format_opts)?;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| "檔案內沒有可解碼的音軌".to_string())?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(|error| SymOpenFailure::from_decoder(error, "不支援的編碼"))?;
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
            terminal_failure: None,
            decode_errors: DecodeErrorBudget::default(),
            preparation_slot: None,
        };
        if start_ms > 0 {
            s.seek_ms(start_ms)?;
        }
        if !s.refill() {
            match s.terminal_failure {
                Some(failure @ (SymReadFailure::PacketBudget | SymReadFailure::DecodeBudget)) => {
                    return Err(SymOpenFailure::Terminal(failure));
                }
                Some(SymReadFailure::Decode) => return Err("初次解碼失敗".into()),
                None if start_ms <= 0 => return Err("未解碼到任何音訊樣本".into()),
                None => {}
            }
        }
        if s.channels == 0 || s.sample_rate == 0 {
            return Err("無法取得聲道或取樣率".into());
        }
        Ok(s)
    }

    pub fn duration_ms(&self) -> Option<i64> {
        self.duration_ms
    }

    fn seek_ms(&mut self, ms: i64) -> Result<(), SymOpenFailure> {
        if let Some(failure) = self.terminal_failure {
            return Err(SymOpenFailure::Terminal(failure));
        }
        let time = Time::from(ms.max(0) as f64 / 1000.0);
        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track_id) })
            .map_err(|error| SymOpenFailure::from_decoder(error, "跳轉失敗"))?;
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

    pub(crate) fn observe_preparation(&mut self, slot: Option<&Arc<crate::player::PcmCaptureSlot>>) {
        self.preparation_slot = slot.filter(|slot| slot.note_native_progress()).map(Arc::downgrade);
    }

    pub(crate) fn next_checked(&mut self) -> Result<Option<f32>, SymReadFailure> {
        if self.finished || (self.pos >= self.len && !self.refill()) {
            return self.terminal_failure.map_or(Ok(None), Err);
        }
        let sample = self.buf.as_ref().ok_or(SymReadFailure::Decode)?.samples()[self.pos];
        self.pos += 1;
        Ok(Some(sample))
    }

    fn mark_finished(&mut self, failure: Option<SymReadFailure>) -> bool {
        self.terminal_failure = failure;
        self.finished = true;
        false
    }

    /// 解下一個封包到 buf。回傳 false 代表檔案結束或無法繼續。
    fn refill(&mut self) -> bool {
        if self.finished {
            return false;
        }
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymError::LimitError(_)) => {
                    return self.mark_finished(Some(SymReadFailure::PacketBudget));
                }
                Err(SymError::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return self.mark_finished(None);
                }
                Err(SymError::ResetRequired) => return self.mark_finished(None),
                Err(_) => return self.mark_finished(Some(SymReadFailure::Decode)),
            };
            if packet.buf().len() > MAX_PACKET_BYTES {
                return self.mark_finished(Some(SymReadFailure::PacketBudget));
            }
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.decode_errors.on_decode_success();
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
                    if self.preparation_slot.is_some() && len > 0 {
                        let active = self.preparation_slot.as_ref().and_then(std::sync::Weak::upgrade)
                            .is_some_and(|slot| slot.note_native_progress());
                        if !active {
                            self.preparation_slot = None;
                        }
                    }
                    if self.pos < self.len {
                        return true;
                    }
                }
                Err(SymError::DecodeError(_)) => {
                    if self.decode_errors.on_decode_error() == DecodeErrorDecision::Stop {
                        return self.mark_finished(Some(SymReadFailure::DecodeBudget));
                    }
                }
                Err(SymError::LimitError(_)) => {
                    return self.mark_finished(Some(SymReadFailure::PacketBudget));
                }
                Err(_) => return self.mark_finished(Some(SymReadFailure::Decode)),
            }
        }
    }
}

impl Iterator for SymStream {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.next_checked().ok().flatten()
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
        Self::new_at(ffmpeg_path, file_path, sample_rate, channels, duration_ms, 0)
    }

        pub fn new_at(
        ffmpeg_path: &Path,
        file_path: &str,
        sample_rate: u32,
        channels: u16,
        duration_ms: i64,
        position_ms: i64,
    ) -> Result<Self, String> {
        // 先用 ffprobe 讀取精確資訊（經過 locate，不繞過；SE6：64 KB / 10 秒上限）
        let ffprobe_path = locate("ffprobe.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffprobe")?;
        let value = crate::audio_decode::ffprobe_playback_json(&ffprobe_path, file_path)?;

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
        let start = format!("{:.3}", position_ms.max(0) as f64 / 1000.0);
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
                                "-ss", &start,
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

#[cfg(test)]
mod tests {

#[test]
fn bounded_probe_requires_eof_and_enforces_limit() {
    let probe = |chunks: &[usize], tail: Option<FfprobeStdoutMsg>| {
        let (tx, rx) = std::sync::mpsc::sync_channel(chunks.len() + 1);
        for &length in chunks {
            tx.send(FfprobeStdoutMsg::Data(vec![b' '; length])).ok().unwrap();
        }
        if let Some(tail) = tail {
            tx.send(tail).ok().unwrap();
        }
        drop(tx);
        ffprobe_collect(&rx, Instant::now() + Duration::from_secs(1), None)
    };
    assert_eq!(probe(&[8192; 8], Some(FfprobeStdoutMsg::Eof)).unwrap().len(), 65536);
    assert!(probe(&[65536, 1], Some(FfprobeStdoutMsg::Eof)).is_err());
    assert!(probe(&[65536], None).is_err());
    assert!(probe(&[16], Some(FfprobeStdoutMsg::ReadError)).is_err());
}

#[test]
fn cancelling_a_waiting_probe_leaves_the_slot_for_a_live_waiter() {
    let quota = (Mutex::new(1), Condvar::new());
    let deadline = Instant::now() + Duration::from_secs(2);
    let held = ffprobe_acquire_from(&quota, deadline, None).unwrap();
    let cancel = AtomicBool::new(false);
    thread::scope(|scope| {
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(2);
        let cancelled_tx = started_tx.clone();
        let quota_ref = &quota;
        let cancel_ref = &cancel;
        let cancelled_waiter = scope.spawn(move || {
            cancelled_tx.send(()).unwrap();
            ffprobe_acquire_from(quota_ref, deadline, Some(cancel_ref))
        });
        let next_waiter = scope.spawn(move || {
            started_tx.send(()).unwrap();
            ffprobe_acquire_from(quota_ref, deadline, None)
        });
        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        cancel.store(true, Ordering::Release);
        drop(held);
        assert!(matches!(cancelled_waiter.join().unwrap(), Err(ref text) if text == FFPROBE_CANCELLED));
        let next = next_waiter.join().unwrap().unwrap();
        assert_eq!(*quota.0.lock().unwrap(), 0);
        drop(next);
        assert_eq!(*quota.0.lock().unwrap(), 1);
    });
}

#[test]
fn cancelled_probe_observer_keeps_the_background_quota_until_cleanup_finishes() {
    let quota = (Mutex::new(1), Condvar::new());
    let deadline = Instant::now() + Duration::from_secs(2);
    let held = ffprobe_acquire_from(&quota, deadline, None).unwrap();
    let (_waiting_tx, waiting_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let (cleanup_tx, cleanup_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let flag = AtomicBool::new(true);
    thread::scope(|scope| {
        let supervisor = scope.spawn(move || {
            let _ = cleanup_rx.recv_timeout(Duration::from_secs(2));
            drop(held);
        });
        assert_eq!(ffprobe_wait(&waiting_rx, deadline, Some(&flag), "disconnected"), Err(FFPROBE_CANCELLED));
        assert_eq!(*quota.0.lock().unwrap(), 0);
        cleanup_tx.send(()).unwrap();
        supervisor.join().unwrap();
        assert_eq!(*quota.0.lock().unwrap(), 1);
    });
}

#[test]
fn cancelled_playback_probe_does_not_start_a_subprocess() {
    let cancelled = Arc::new(AtomicBool::new(true));
    let result = ffprobe_playback_json_for_request(
        Path::new("missing-ffprobe.exe"), "unused.wav", Some(&cancelled),
    );
    assert_eq!(result.unwrap_err(), FFPROBE_CANCELLED);
}

#[test]
fn playback_probe_can_cancel_while_the_stdout_channel_is_open() {
    let (_tx, rx) = std::sync::mpsc::sync_channel::<FfprobeStdoutMsg>(1);
    let flag = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(Duration::from_millis(10));
            flag.store(true, Ordering::Release);
        });
        let result = ffprobe_collect(&rx, Instant::now() + Duration::from_secs(2), Some(&flag));
        assert_eq!(result.unwrap_err(), FFPROBE_CANCELLED);
    });
}

#[test]
#[ignore]
fn actual_probe_reports_normal_audio_fields() {
    let Some((exe, _, _)) = crate::tools::locate("ffprobe.exe") else {
        panic!("ffprobe is required for this test");
    };
    let wav = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats");
    let file = std::fs::read_dir(wav).unwrap()
        .filter_map(Result::ok).map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wav")).unwrap();
    let path = file.to_str().unwrap();
    let playback = ffprobe_playback_json(&exe, path).unwrap();
    let scan = ffprobe_json(&exe, path).unwrap();
    for name in ["codec_type", "codec_name", "sample_rate", "channels"] {
        assert!(!playback["streams"][0][name].is_null());
        assert_eq!(playback["streams"][0][name], scan["streams"][0][name]);
    }
    assert_eq!(playback["format"]["duration"], scan["format"]["duration"]);
}

    use super::*;
    use std::io::{Seek, SeekFrom};
    use std::path::PathBuf;

    #[test]
    #[ignore = "requires FFmpeg to generate ordinary Ogg and AIFF audio"]
    fn native_packet_limits_keep_wav_aiff_flac_and_ogg_playable() {
        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats");
        let wav = input.join("sample_44100_stereo.wav");
        let flac = input.join("sample_44100_stereo.flac");
        let folder = tempfile::tempdir().unwrap();
        let aiff = folder.path().join("normal.aiff");
        let ogg = folder.path().join("normal.ogg");
        let ogg_flac = folder.path().join("normal-flac.ogg");
        let labels = folder.path().join("normal.ffmetadata");
        std::fs::write(
            &labels,
            format!(";FFMETADATA1\nDESCRIPTION={}\n", "caption ".repeat(40_000)),
        ).unwrap();
        for (path, codec, use_labels) in [
            (&aiff, "pcm_s16be", false),
            (&ogg, "libvorbis", true),
            (&ogg_flac, "flac", true),
        ] {
            let mut cmd = Command::new(&ffmpeg);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            cmd.args(["-nostdin", "-v", "error", "-i"]).arg(&wav);
            if use_labels {
                cmd.arg("-i").arg(&labels).args(["-map_metadata", "1"]);
            }
            let status = cmd.args(["-map", "0:a:0", "-c:a", codec]).arg(path).status().unwrap();
            assert!(status.success(), "failed to encode an ordinary fixture with {codec}");
        }
        assert!(std::fs::metadata(&ogg).unwrap().len() > 256 * 1024);
        assert!(std::fs::metadata(&ogg_flac).unwrap().len() > 256 * 1024);

        for path in [wav, aiff, flac, ogg, ogg_flac] {
            let mut input = File::open(&path).unwrap();
            let mut stream = SymStream::open_checked(&mut input, &path, 0).unwrap();
            assert_eq!(stream.sample_rate(), 44_100);
            assert_eq!(stream.channels(), 2);
            let mut samples = 0;
            while stream.next_checked().unwrap().is_some() {
                samples += 1;
                assert!(samples < 44_100 * 2 * 10);
            }
            assert!(samples > 0, "ordinary audio sample did not decode: {path:?}");
        }
    }

    #[test]
    #[ignore = "需要本機 FFmpeg 產生兩個正常 AAC 音檔"]
    fn test_isomp4_local_patch_preserves_regular_and_fragmented_aac() {
        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let folder = tempfile::tempdir().unwrap();
        for (name, flags) in [
            ("regular.m4a", "+faststart"),
            ("fragmented.m4a", "+frag_keyframe+empty_moov+default_base_moof"),
        ] {
            let path = folder.path().join(name);
            let mut generate = Command::new(&ffmpeg);
            generate.args([
                "-nostdin", "-y", "-loglevel", "error", "-f", "lavfi", "-i",
                "sine=frequency=440:sample_rate=44100:duration=0.15",
                "-c:a", "aac", "-metadata", "title=Synthesized AAC", "-movflags", flags,
            ]).arg(&path).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                generate.creation_flags(0x0800_0000);
            }
            assert!(generate.status().unwrap().success(), "{name} generation failed");
            let tags = lofty::read_from_path(&path).unwrap();
            assert_eq!(
                tags.primary_tag().and_then(|tag| tag.get_string(&lofty::ItemKey::TrackTitle)),
                Some("Synthesized AAC"),
            );

            let mut file = File::open(&path).unwrap();
            let mut stream = SymStream::open_checked(&mut file, &path, 0).unwrap();
            let mut samples = 0;
            let mut audible = false;
            while let Some(sample) = stream.next_checked().unwrap() {
                assert!(sample.is_finite());
                audible |= sample.abs() > 0.01;
                samples += 1;
            }
            assert!(samples >= 1_024 && audible, "{name} did not decode the expected audio");
            assert_eq!(stream.next_checked(), Ok(None));
        }
    }

    #[test]
    #[ignore = "需要本機 FFmpeg 產生含有正常 ID3v2 標籤的 MP3"]
    fn playback_reads_tagged_mp3_without_decoding_id3_metadata() {
        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let folder = tempfile::tempdir().unwrap();
        for version in ["3", "4"] {
            let path = folder.path().join(format!("id3-v{version}.mp3"));
            let comment = format!("comment={}", "s".repeat(8_192));
            let mut generate = Command::new(&ffmpeg);
            generate.args([
                "-nostdin", "-y", "-loglevel", "error", "-f", "lavfi", "-i",
                "sine=frequency=440:sample_rate=44100:duration=0.25",
                "-c:a", "libmp3lame", "-id3v2_version", version, "-metadata", "title=Synthetic MP3",
                "-metadata", &comment,
            ]).arg(&path).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                generate.creation_flags(0x0800_0000);
            }
            assert!(generate.status().unwrap().success(), "ID3v2.{version} generation failed");
            let mut file = File::open(&path).unwrap();
            let mut marker = [0; 4];
            file.read_exact(&mut marker).unwrap();
            assert_eq!(&marker[..3], b"ID3");
            assert_eq!(marker[3], version.parse::<u8>().unwrap());
            let mut decoder = SymStream::open_checked(&mut file, &path, 0).unwrap();
            assert_eq!(decoder.sample_rate(), 44_100);
            assert_eq!(decoder.channels(), 1);
            let mut samples = 0;
            let mut heard = false;
            while let Some(sample) = decoder.next_checked().unwrap() {
                heard |= sample.abs() > 0.01;
                samples += 1;
            }
            assert!(heard && samples > 1_024);
        }
    }

    fn wav_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("formats")
            .join("sample_44100_stereo.wav")
    }

    fn flac_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("formats")
            .join("sample_44100_stereo.flac")
    }

    // C1 回歸測試：precheck_packet_budget 必須將 File 游標還原至進入時位置，
    // 無論預檢結果成功或錯誤；定位或還原失敗應回傳錯誤。
    #[test]
    fn precheck_packet_budget_restores_nonzero_cursor() {
        let mut file = std::fs::File::open(wav_fixture()).expect("開啟正常 WAV fixture 失敗");
        file.seek(SeekFrom::Start(123)).expect("定位非零游標失敗");
        let entry = file.stream_position().expect("讀取進入游標失敗");
        assert_eq!(entry, 123);

        let result = precheck_packet_budget(&mut file);

        let after = file.stream_position().expect("讀取還原後游標失敗");
        assert_eq!(after, 123, "預檢後游標應還原至進入時的非零位置");
        assert!(result.is_ok(), "正常 WAV 預檢應成功: {result:?}");
    }

    // C1：WAV 與 FLAC 皆可從非零 File 游標直接呼叫 SymStream::open，
    // 並正確辨認 44100 Hz / 2 聲道，且能逐個輸出非靜音樣本。
    #[test]
    fn symstream_open_from_nonzero_cursor_recognizes_44100_stereo() {
        for fixture in [wav_fixture(), flac_fixture()] {
            let mut file = std::fs::File::open(&fixture)
                .unwrap_or_else(|_| panic!("開啟 fixture 失敗: {:?}", fixture));
            file.seek(SeekFrom::Start(123)).expect("定位非零游標失敗");
            let entry = file.stream_position().expect("讀取進入游標失敗");
            assert_eq!(entry, 123, "SymStream::open 進入時游標應為非零位置: {:?}", fixture);

            let mut stream = SymStream::open(&mut file, fixture.as_path(), 0)
                .unwrap_or_else(|e| panic!("SymStream::open 失敗: {e} (fixture {:?})", fixture));

            assert_eq!(stream.sample_rate(), 44100, "取樣率應為 44100: {:?}", fixture);
            assert_eq!(stream.channels(), 2, "聲道數應為 2: {:?}", fixture);

            let mut sample_count = 0usize;
            let mut nonzero_count = 0usize;
            for sample in stream.by_ref().take(44100) {
                sample_count += 1;
                if sample != 0.0 {
                    nonzero_count += 1;
                }
            }
            assert!(sample_count > 0, "應能從 {:?} 輸出樣本", fixture);
            assert!(nonzero_count > 0, "輸出的樣本不應全為靜音: {:?}", fixture);
            while stream.next_checked().unwrap().is_some() {}
            assert_eq!(stream.next_checked(), Ok(None));
        }
    }

    /// C1：不足標頭長度的短 WAV 預檢回錯時，File 游標仍須復原至進入時位置。
    #[test]
    fn precheck_packet_budget_short_wav_restores_cursor_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.wav");
        // 標準 PCM WAV 標頭需 44 bytes；這裡僅寫 8 bytes，小於 precheck 需讀取的 16 bytes
        std::fs::write(&path, b"RIFF....").unwrap();

        let mut file = std::fs::File::open(&path).expect("開啟短 WAV 失敗");
        file.seek(SeekFrom::Start(4)).expect("定位失敗");
        let entry = file.stream_position().expect("讀取進入游標失敗");
        assert_eq!(entry, 4);

        let result = precheck_packet_budget(&mut file);

        assert!(result.is_err(), "短 WAV 預檢應失敗，實際：{result:?}");
        let after = file.stream_position().expect("讀取還原後游標失敗");
        assert_eq!(after, entry, "預檢失敗時游標應還原至進入時位置");
        assert!(matches!(SymStream::open_checked(&mut file, &path, 0), Err(SymOpenFailure::Precheck(_))));
        assert!(!SymOpenFailure::Precheck(String::new()).allows_fallback());
    }

    /// C2a 回歸：10 次可恢復解碼錯誤皆應 Recover，且其間穿插的成功解碼事件
    /// 不得重置累計計數；第 11 次錯誤應判定 Stop（終止），之後維持終止。
    #[test]
    fn decode_budget_stops_on_11th_error_despite_intervening_successes() {
        assert_eq!(MAX_DECODE_ERRORS, 10);
        let mut budget = DecodeErrorBudget::default();

        for i in 0..10 {
            assert_eq!(
                budget.on_decode_error(),
                DecodeErrorDecision::Recover,
                "第 {} 次錯誤應可恢復",
                i + 1
            );
            // 錯誤之間穿插成功解碼事件：成功不重置計數
            budget.on_decode_success();
            assert_eq!(budget.count, i + 1, "成功解碼後計數應維持累計，第 {} 次後", i + 1);
        }
        assert_eq!(budget.count, 10, "10 次錯誤後計數應恰為上限");

        // 第 11 次錯誤：超出 MAX_DECODE_ERRORS，應終止
        assert_eq!(
            budget.on_decode_error(),
            DecodeErrorDecision::Stop,
            "第 11 次錯誤應終止"
        );
        // 終止後不會再回到可恢復
        assert_eq!(budget.on_decode_error(), DecodeErrorDecision::Stop);
    }

    /// C2a：真實 SymStream::refill() 在受控解碼器錯誤注入下的跨封包錯誤累計行為。
    /// 建立 2 秒 44.1kHz 16-bit stereo PCM WAV，用 ScriptedDecoder 包裝真實解碼器，
    /// 僅在第 5、12 次 decode 呼叫時委派給內部解碼器，其餘回傳靜態 DecodeError，
    /// 驗證 refill 的 true/false/finished 與 decode_errors.count 精確對應。
    #[test]
    fn symstream_refill_decode_error_budget_integration() {
        use std::io::{Write, Seek, SeekFrom};
        use tempfile::NamedTempFile;
        use symphonia::core::codecs::{Decoder as SymDecoder, DecoderOptions, CODEC_TYPE_NULL};
        use symphonia::core::errors::Error as SymError;
        use symphonia::core::formats::{FormatOptions, FormatReader};
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;
        use symphonia::core::audio::{AudioBufferRef, Signal, SignalSpec};
        use symphonia::core::codecs::{CodecParameters, CodecDescriptor, FinalizeResult};

        // 1) 建立 2 秒 44.1kHz 16-bit stereo PCM WAV（常數非零樣本）
        let sample_rate = 44100u32;
        let channels = 2u16;
        let duration_secs = 2u32;
        let frames = (sample_rate * duration_secs) as usize;
        let bytes_per_sample = 2; // 16-bit
        let block_align = channels as usize * bytes_per_sample;
        let data_size = frames * block_align;
        let riff_size = 36 + data_size;

        let mut wav = Vec::with_capacity(44 + data_size);
        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(riff_size as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        // fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&(16u32).to_le_bytes()); // chunk size
        wav.extend_from_slice(&(1u16).to_le_bytes()); // audio format (PCM)
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate as u32 * block_align as u32).to_le_bytes()); // byte rate
        wav.extend_from_slice(&(block_align as u16).to_le_bytes());
        wav.extend_from_slice(&(16u16).to_le_bytes()); // bits per sample
        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data_size as u32).to_le_bytes());
        // PCM data: constant non-zero sample (e.g., 1000)
        let sample_val: i16 = 1000;
        let sample_bytes = sample_val.to_le_bytes();
        for _ in 0..frames {
            for _ in 0..channels {
                wav.extend_from_slice(&sample_bytes);
            }
        }

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&wav).unwrap();
        tmp.flush().unwrap();
        tmp.seek(SeekFrom::Start(0)).unwrap();

        // 2) 開啟真實 SymStream
        let mut file = std::fs::File::open(tmp.path()).unwrap();
        let mut stream = SymStream::open(&mut file, tmp.path(), 0).unwrap();

        // 3) 建立真實內部解碼器
        let inner = symphonia::default::get_codecs()
            .make(stream.decoder.codec_params(), &DecoderOptions::default())
            .unwrap();

        // 4) ScriptedDecoder 結構
        struct ScriptedDecoder {
            inner: Box<dyn SymDecoder>,
            calls: usize,
        }

        impl ScriptedDecoder {
            fn new(inner: Box<dyn SymDecoder>) -> Self {
                Self { inner, calls: 0 }
            }
        }

        // 實作 Decoder trait（本地實作，僅測試用）
        impl SymDecoder for ScriptedDecoder {
            fn try_new(_params: &CodecParameters, _opts: &DecoderOptions) -> Result<Self, SymError> {
                unreachable!("ScriptedDecoder::try_new 不應在測試中被呼叫")
            }

            fn supported_codecs() -> &'static [CodecDescriptor] {
                &[]
            }

            fn reset(&mut self) {
                self.inner.reset();
                self.calls = 0;
            }

            fn codec_params(&self) -> &CodecParameters {
                self.inner.codec_params()
            }

            fn decode(&mut self, packet: &symphonia::core::formats::Packet) -> Result<AudioBufferRef<'_>, SymError> {
                self.calls += 1;
                // 僅在第 5、12 次呼叫時委派給真實解碼器
                if self.calls == 5 || self.calls == 12 {
                    self.inner.decode(packet)
                } else {
                    Err(SymError::DecodeError("injected test error".into()))
                }
            }

            fn finalize(&mut self) -> FinalizeResult {
                self.inner.finalize()
            }

            fn last_decoded(&self) -> AudioBufferRef<'_> {
                self.inner.last_decoded()
            }
        }

        // 5) 替換 stream.decoder
        stream.decoder = Box::new(ScriptedDecoder::new(inner));

        // 6) 驗證 refill 行為
        // 第 1 次 refill：calls 1~4 失敗（count 1~4），call 5 成功 → 回傳 true，count == 4
        assert!(stream.refill(), "第 1 次 refill 應成功");
        assert_eq!(stream.decode_errors.count, 4, "第 1 次 refill 後 count 應為 4");

        // 第 2 次 refill：calls 6~11 失敗（count 5~10），call 12 成功 → 回傳 true，count == 10
        assert!(stream.refill(), "第 2 次 refill 應成功");
        assert_eq!(stream.decode_errors.count, 10, "第 2 次 refill 後 count 應為 10");

        // 第 3 次 refill：call 13 失敗（count 11 > MAX_DECODE_ERRORS）→ 回傳 false，finished，count == 11
        assert!(!stream.refill(), "第 3 次 refill 應失敗並終止");
        assert!(stream.finished, "第 3 次 refill 後應標記 finished");
        assert_eq!(stream.next_checked(), Err(SymReadFailure::DecodeBudget));
        assert_eq!(stream.next_checked(), Err(SymReadFailure::DecodeBudget));
        let seek = stream.seek_ms(0).unwrap_err();
        assert!(matches!(seek, SymOpenFailure::Terminal(SymReadFailure::DecodeBudget)));
        assert!(!seek.allows_fallback());
        assert_eq!(stream.next_checked(), Err(SymReadFailure::DecodeBudget));
        assert_eq!(stream.decode_errors.count, 11, "第 3 次 refill 後 count 應為 11");

        // 後續 refill：持續回傳 false，count 維持 11
        assert!(!stream.refill(), "第 4 次 refill 應失敗");
        assert_eq!(stream.decode_errors.count, 11, "第 4 次 refill 後 count 應維持 11");
        assert!(!stream.refill(), "第 5 次 refill 應失敗");
        assert_eq!(stream.decode_errors.count, 11, "第 5 次 refill 後 count 應維持 11");
    }
}
