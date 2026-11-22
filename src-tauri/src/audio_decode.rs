//! Symphonia 直接解碼（逐封包串流），不經 rodio::Decoder。
//!
//! rodio 0.19 的 `ReadSeekSource::byte_len()` 固定回傳 None，symphonia 的 MP4 demuxer
//! 需要檔案長度，於是 rodio 在初始化時 `unreachable!` panic，m4a/mp4 永遠開不起來。
//! 這裡直接把 `std::fs::File` 交給 MediaSourceStream（File 會回報長度），
//! 也讓長錄音不必整首解進記憶體。

use rodio::Source;
use std::fs::File;
use std::path::Path;
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder as SymDecoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

const MAX_DECODE_ERRORS: usize = 8;

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
}

impl SymStream {
    /// 開檔並（可選）精準跳到 start_ms。回傳時已解出第一個封包，channels / sample_rate 可信。
    pub fn open(path: &Path, start_ms: i64) -> Result<Self, String> {
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
        let mut errors = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => {
                    self.finished = true;
                    return false;
                }
            };
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
                    errors += 1;
                    if errors > MAX_DECODE_ERRORS {
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
