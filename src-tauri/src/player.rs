use crate::models::{EngineSnapshot, PlayerCommand, PlayMode};
use crate::open_with::{log_line, log_load_error};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const EQ_HZ: [f32; 10] = [32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

#[derive(Clone)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn bypass() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }

    fn peaking(sample_rate: f32, freq: f32, gain_db: f32) -> Self {
        if gain_db.abs() < 0.05 || sample_rate <= 0.0 {
            return Self::bypass();
        }
        let q = 1.41;
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cosw = w0.cos();
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cosw;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cosw;
        let a2 = 1.0 - alpha / a;
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

struct PcmTrack {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
    duration_ms: i64,
    song_id: i64,
    norm_gain: f32,
}

impl PcmTrack {
    fn clone_track(&self) -> Self {
        Self {
            samples: self.samples.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            duration_ms: self.duration_ms,
            song_id: self.song_id,
            norm_gain: self.norm_gain,
        }
    }
}

struct EqSource {
    samples: Vec<f32>,
    pos: usize,
    channels: u16,
    sample_rate: u32,
    filters: Vec<Vec<Biquad>>,
    gain: f32,
}

impl Iterator for EqSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.samples.len() || self.channels == 0 {
            return None;
        }
        let ch = self.pos % self.channels as usize;
        let mut s = self.samples[self.pos] * self.gain;
        self.pos += 1;
        if let Some(chain) = self.filters.get_mut(ch) {
            for f in chain.iter_mut() {
                s = f.process(s);
            }
        }
        Some(s.clamp(-1.0, 1.0))
    }
}

impl Source for EqSource {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> {
        let frames = self.samples.len() / self.channels.max(1) as usize;
        Some(Duration::from_secs_f64(frames as f64 / self.sample_rate.max(1) as f64))
    }
}

struct AudioEngine {
    _stream: OutputStream,
    _handle: OutputStreamHandle,
    sink: Sink,
    current: Option<PcmTrack>,
    preloaded: Option<PcmTrack>,
    mode: PlayMode,
    is_playing: bool,
    volume: f32,
    seek_origin_ms: i64,
    eq: [f32; 10],
    preamp_db: f32,
    normalize: bool,
    crossfade_ms: i64,
    fading: bool,
    kind: String,
    stream_title: String,
    error: String,
    ended: bool,
    status: Arc<Mutex<EngineSnapshot>>,
    radio_stop: Option<Arc<AtomicBool>>,
}

fn decode_to_pcm(path: &Path, song_id: i64) -> Result<PcmTrack, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("開啟檔案失敗: {e}"))?;
    let decoder = Decoder::new(BufReader::new(file)).map_err(|e| format!("解碼失敗: {e}"))?;
    let sample_rate = decoder.sample_rate().max(1);
    let channels = decoder.channels().max(1);
    let samples_i16: Vec<i16> = decoder.collect();
    if samples_i16.is_empty() {
        return Err("未解碼到任何音訊樣本".into());
    }
    let samples: Vec<f32> = samples_i16.into_iter().map(|s| s as f32 / i16::MAX as f32).collect();
    let duration_ms = (samples.len() as f64 / (sample_rate as f64 * channels as f64) * 1000.0) as i64;
    let peak = samples.iter().map(|s| s.abs()).fold(0f32, f32::max);
    let norm_gain = if peak > 0.0001 { (0.89 / peak).clamp(0.05, 6.0) } else { 1.0 };
    Ok(PcmTrack { samples, sample_rate, channels, duration_ms, song_id, norm_gain })
}

fn sample_index(pcm: &PcmTrack, ms: i64) -> usize {
    let frames = (ms.max(0) as f64 / 1000.0) * pcm.sample_rate as f64;
    (frames as usize) * pcm.channels as usize
}

fn make_filters(sample_rate: u32, channels: u16, eq: &[f32; 10]) -> Vec<Vec<Biquad>> {
    (0..channels.max(1))
        .map(|_| EQ_HZ.iter().zip(eq.iter()).map(|(hz, g)| Biquad::peaking(sample_rate as f32, *hz, *g)).collect())
        .collect()
}

fn preamp_linear(db: f32) -> f32 {
    10f32.powf(db.clamp(-12.0, 12.0) / 20.0)
}

fn append_pcm(sink: &Sink, pcm: &PcmTrack, start_ms: i64, eq: &[f32; 10], normalize: bool, preamp_db: f32) {
    let start = sample_index(pcm, start_ms).min(pcm.samples.len());
    let slice = pcm.samples[start..].to_vec();
    if slice.is_empty() {
        return;
    }
    let gain = (if normalize { pcm.norm_gain } else { 1.0 }) * preamp_linear(preamp_db);
    sink.append(EqSource {
        samples: slice,
        pos: 0,
        channels: pcm.channels,
        sample_rate: pcm.sample_rate,
        filters: make_filters(pcm.sample_rate, pcm.channels, eq),
        gain,
    });
}

fn mix_crossfade(current: &PcmTrack, from_ms: i64, next: &PcmTrack, fade_ms: i64) -> PcmTrack {
    let ch = current.channels.max(1) as usize;
    let a0 = sample_index(current, from_ms).min(current.samples.len());
    let fade_frames = ((fade_ms.max(1) as f64 / 1000.0) * current.sample_rate as f64) as usize;
    let fade_samples = fade_frames * ch;
    let a = &current.samples[a0..];
    let n = fade_samples.min(a.len()).min(next.samples.len());
    let mut out = Vec::with_capacity(n + next.samples.len().saturating_sub(n));
    for i in 0..n {
        let t = if n <= 1 { 1.0 } else { i as f32 / (n - 1) as f32 };
        let b = if next.channels as usize == ch { next.samples[i] } else { next.samples[i % next.samples.len()] };
        out.push(a[i] * (1.0 - t) + b * t);
    }
    if next.samples.len() > n {
        out.extend_from_slice(&next.samples[n..]);
    }
    let duration_ms = (out.len() as f64 / (next.sample_rate.max(1) as f64 * ch as f64) * 1000.0) as i64;
    PcmTrack {
        samples: out,
        sample_rate: next.sample_rate,
        channels: ch as u16,
        duration_ms,
        song_id: next.song_id,
        norm_gain: next.norm_gain,
    }
}

struct LiveSource {
    rx: mpsc::Receiver<Vec<f32>>,
    pending: Vec<f32>,
    pos: usize,
}

impl Iterator for LiveSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.pending.len() {
            self.pending = self.rx.recv().ok()?;
            self.pos = 0;
            if self.pending.is_empty() {
                return None;
            }
        }
        let sample = self.pending[self.pos];
        self.pos += 1;
        Some(sample)
    }
}

impl Source for LiveSource {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { 2 }
    fn sample_rate(&self) -> u32 { 44100 }
    fn total_duration(&self) -> Option<Duration> { None }
}

impl AudioEngine {
    fn new(status: Arc<Mutex<EngineSnapshot>>) -> Result<Self, String> {
        let (_stream, handle) = OutputStream::try_default().map_err(|e| format!("音訊輸出失敗: {e}"))?;
        let sink = Sink::try_new(&handle).map_err(|e| format!("建立 Sink 失敗: {e}"))?;
        sink.set_volume(0.7);
        crate::open_with::log_line("AUDIO_ENGINE initialized");
        Ok(Self {
            _stream,
            _handle: handle,
            sink,
            current: None,
            preloaded: None,
            mode: PlayMode::Sequence,
            is_playing: false,
            volume: 0.7,
            seek_origin_ms: 0,
            eq: [0.0; 10],
            preamp_db: 0.0,
            normalize: false,
            crossfade_ms: 0,
            fading: false,
            kind: "idle".into(),
            stream_title: String::new(),
            error: String::new(),
            ended: false,
            status,
            radio_stop: None,
        })
    }

    fn position_ms(&self) -> i64 {
        let played = self.sink.get_pos().as_millis() as i64;
        if self.kind == "radio" { played } else { self.seek_origin_ms + played }
    }

    fn publish(&self) {
        if let Ok(mut s) = self.status.lock() {
            s.position_ms = self.position_ms();
            s.duration_ms = if self.kind == "radio" { 0 } else { self.current.as_ref().map(|p| p.duration_ms).unwrap_or(0) };
            s.is_playing = self.is_playing && !self.sink.empty();
            s.volume = self.volume;
            s.song_id = self.current.as_ref().map(|p| p.song_id).unwrap_or(0);
            s.ended = self.ended;
            s.kind = self.kind.clone();
            s.stream_title = self.stream_title.clone();
            s.error = self.error.clone();
            s.eq_bands = self.eq.to_vec();
            s.preamp_db = self.preamp_db;
            s.normalize = self.normalize;
            s.crossfade_ms = self.crossfade_ms;
            s.applied_gain = if self.normalize { self.current.as_ref().map(|p| p.norm_gain).unwrap_or(1.0) } else { 1.0 };
            s.band_hz = EQ_HZ.iter().map(|h| *h as i32).collect();
            s.mode = self.mode.clone();
        }
    }

    fn switch_output(&mut self, name: &str) -> Result<(), String> {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};
        let host = rodio::cpal::default_host();
        let devices = host.output_devices().map_err(|e| format!("列不出輸出裝置: {e}"))?;
        let mut found = None;
        for device in devices {
            if device.name().ok().as_deref() == Some(name) {
                found = Some(device);
                break;
            }
        }
        let device = found.ok_or_else(|| format!("找不到輸出裝置：{name}"))?;
        let (stream, handle) = OutputStream::try_from_device(&device).map_err(|e| format!("切換輸出裝置失敗: {e}"))?;
        let sink = Sink::try_new(&handle).map_err(|e| format!("建立 Sink 失敗: {e}"))?;
        sink.set_volume(self.volume);
        let play = self.is_playing;
        let pos = self.position_ms();
        let track = self.current.as_ref().map(|row| row.clone_track());
        let kind = self.kind.clone();
        let radio = self.stream_title.clone();
        self.sink.stop();
        self._stream = stream;
        self._handle = handle;
        self.sink = sink;
        if kind == "file" {
            if let Some(pcm) = track {
                self.seek_origin_ms = pos.max(0);
                append_pcm(&self.sink, &pcm, self.seek_origin_ms, &self.eq, self.normalize, self.preamp_db);
                if play {
                    self.sink.play();
                    self.is_playing = true;
                } else {
                    self.sink.pause();
                    self.is_playing = false;
                }
                self.current = Some(pcm);
            }
        } else if kind == "radio" && (radio.starts_with("http://") || radio.starts_with("https://")) {
            self.play_url(&radio);
        }
        Ok(())
    }

    fn restart_from(&mut self, ms: i64, play: bool) {
        let Some(pcm) = self.current.as_ref().map(|p| p.clone_track()) else { return };
        self.sink.stop();
        self.sink.clear();
        self.seek_origin_ms = ms.max(0);
        append_pcm(&self.sink, &pcm, self.seek_origin_ms, &self.eq, self.normalize, self.preamp_db);
        if play {
            self.sink.play();
            self.is_playing = true;
        } else {
            self.sink.pause();
            self.is_playing = false;
        }
    }

    fn halt_radio(&mut self) {
        if let Some(flag) = self.radio_stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    fn load(&mut self, path: &str, song_id: i64) -> Result<(), String> {
        self.load_at(path, song_id, 0, true)
    }

    /// play 為 false 時停在 position_ms，不呼叫 sink.play()。避免恢復進度時先出聲。
    fn load_at(&mut self, path: &str, song_id: i64, position_ms: i64, play: bool) -> Result<(), String> {
        self.halt_radio();
        let pcm = decode_to_pcm(Path::new(path), song_id)?;
        self.sink.stop();
        self.sink.clear();
        self.kind = "file".into();
        self.stream_title.clear();
        self.error.clear();
        self.ended = false;
        self.fading = false;
        let ms = if pcm.duration_ms <= 0 {
            0
        } else {
            position_ms.clamp(0, pcm.duration_ms.saturating_sub(1))
        };
        self.seek_origin_ms = ms;
        append_pcm(&self.sink, &pcm, ms, &self.eq, self.normalize, self.preamp_db);
        if play {
            self.sink.play();
            self.is_playing = true;
        } else {
            self.sink.pause();
            self.is_playing = false;
        }
        self.current = Some(pcm);
        Ok(())
    }

    fn preload(&mut self, path: &str, song_id: i64) -> Result<(), String> {
        self.preloaded = Some(decode_to_pcm(Path::new(path), song_id)?);
        self.fading = false;
        Ok(())
    }

    fn play_url(&mut self, url: &str) {
        self.halt_radio();
        self.sink.stop();
        self.sink.clear();
        self.current = None;
        self.preloaded = None;
        self.ended = false;
        self.seek_origin_ms = 0;
        self.stream_title = url.to_string();
        if Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false) == false {
            self.error = "播放串流需要 ffmpeg，這台電腦找不到".into();
            self.is_playing = false;
            self.kind = "idle".into();
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.radio_stop = Some(stop.clone());
        let (tx, rx) = mpsc::channel::<Vec<f32>>();
        let status = self.status.clone();
        let url = url.to_string();
        thread::spawn(move || {
            let mut child = match Command::new("ffmpeg")
                .args(["-nostdin", "-i", &url, "-f", "s16le", "-ac", "2", "-ar", "44100", "-"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(child) => child,
                Err(e) => {
                    if let Ok(mut s) = status.lock() {
                        s.error = format!("無法啟動 ffmpeg: {e}");
                        s.kind = "idle".into();
                        s.is_playing = false;
                    }
                    return;
                }
            };
            let mut stdout = match child.stdout.take() {
                Some(out) => out,
                None => return,
            };
            let mut buf = [0u8; 8192];
            let mut saw = false;
            loop {
                if stop.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    break;
                }
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        saw = true;
                        let mut samples = Vec::with_capacity(n / 2);
                        for chunk in buf[..n].chunks_exact(2) {
                            let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                            samples.push(s as f32 / i16::MAX as f32);
                        }
                        if tx.send(samples).is_err() {
                            let _ = child.kill();
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(Vec::new());
            if !saw {
                let err = child.stderr.take().and_then(|mut e| {
                    let mut text = String::new();
                    use std::io::Read as _;
                    e.read_to_string(&mut text).ok()?;
                    Some(text)
                }).unwrap_or_default();
                let tail: String = err.chars().rev().take(220).collect::<String>().chars().rev().collect();
                if let Ok(mut s) = status.lock() {
                    s.error = if tail.is_empty() { "串流沒有輸出音訊".into() } else { format!("串流失敗: {tail}") };
                    s.kind = "idle".into();
                    s.is_playing = false;
                }
            }
        });
        self.kind = "radio".into();
        self.error.clear();
        self.sink.append(LiveSource { rx, pending: Vec::new(), pos: 0 });
        self.sink.play();
        self.is_playing = true;
    }

    fn maybe_crossfade(&mut self) {
        if self.kind != "file" || self.fading || self.crossfade_ms < 200 || !self.is_playing {
            return;
        }
        if !matches!(self.mode, PlayMode::All | PlayMode::Random) {
            return;
        }
        let Some(current) = self.current.as_ref() else { return };
        if self.preloaded.is_none() {
            return;
        }
        let pos = self.position_ms();
        let remain = current.duration_ms - pos;
        if remain > self.crossfade_ms || remain < 150 {
            return;
        }
        let Some(next) = self.preloaded.take() else { return };
        let mixed = mix_crossfade(current, pos, &next, remain);
        self.sink.stop();
        self.sink.clear();
        self.seek_origin_ms = 0;
        append_pcm(&self.sink, &mixed, 0, &self.eq, self.normalize, self.preamp_db);
        self.sink.play();
        self.is_playing = true;
        self.current = Some(mixed);
        self.fading = true;
        self.ended = false;
    }

    fn on_empty(&mut self) {
        if !self.is_playing || self.kind == "idle" {
            return;
        }
        // 不在這裡開預載的下一首。順序模式要停、單曲要重播，交給前端依模式決定。
        self.is_playing = false;
        self.ended = true;
    }
}

fn audio_loop(rx: mpsc::Receiver<PlayerCommand>, status: Arc<Mutex<EngineSnapshot>>) -> Result<(), String> {
    let mut engine = AudioEngine::new(status)?;
    loop {
        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(command) => match command {
                PlayerCommand::Load { path, song_id } => {
                    if let Err(e) = engine.load(&path, song_id) {
                        engine.error = e.clone();
                        engine.is_playing = false;
                        log_load_error(&format!("Load {path} song_id={song_id}: {e}"));
                    } else {
                        log_line(&format!("LOAD_OK {path} song_id={song_id}"));
                    }
                }
                PlayerCommand::Prepare { path, song_id, position_ms } => {
                    if let Err(e) = engine.load_at(&path, song_id, position_ms, false) {
                        engine.error = e;
                        engine.is_playing = false;
                    }
                }
                PlayerCommand::Preload { path, song_id } => {
                    let _ = engine.preload(&path, song_id);
                }
                PlayerCommand::Play => {
                    log_line("CMD PLAY executing sink.play()");
                    engine.sink.play();
                    engine.is_playing = true;
                    engine.ended = false;
                }
                PlayerCommand::Pause => {
                    engine.sink.pause();
                    engine.is_playing = false;
                }
                PlayerCommand::Toggle => {
                    if engine.is_playing {
                        engine.sink.pause();
                        engine.is_playing = false;
                    } else {
                        engine.sink.play();
                        engine.is_playing = true;
                        engine.ended = false;
                    }
                }
                PlayerCommand::Stop => {
                    engine.halt_radio();
                    engine.sink.stop();
                    engine.is_playing = false;
                    engine.kind = "idle".into();
                    engine.ended = false;
                }
                PlayerCommand::Seek { position_ms } => {
                    if engine.kind == "file" {
                        let play = engine.is_playing;
                        engine.restart_from(position_ms, play);
                        engine.ended = false;
                    }
                }
                PlayerCommand::SetVolume { volume } => {
                    engine.volume = volume.clamp(0.0, 1.0);
                    engine.sink.set_volume(engine.volume);
                }
                PlayerCommand::SetMode { mode } => {
                    if !matches!(mode, PlayMode::All | PlayMode::Random) {
                        engine.preloaded = None;
                    }
                    engine.mode = mode;
                },
                PlayerCommand::SetEq { bands } => {
                    for (i, g) in bands.into_iter().take(10).enumerate() {
                        engine.eq[i] = g.clamp(-12.0, 12.0);
                    }
                    if engine.kind == "file" && engine.current.is_some() {
                        let pos = engine.position_ms();
                        let play = engine.is_playing;
                        engine.restart_from(pos, play);
                    }
                }
                PlayerCommand::SetPreamp { db } => {
                    engine.preamp_db = db.clamp(-12.0, 12.0);
                    if engine.kind == "file" && engine.current.is_some() {
                        let pos = engine.position_ms();
                        let play = engine.is_playing;
                        engine.restart_from(pos, play);
                    }
                }
                PlayerCommand::SetOutput { name } => {
                    if let Err(e) = engine.switch_output(&name) {
                        engine.error = e;
                    } else {
                        engine.error.clear();
                    }
                }
                PlayerCommand::SetNormalize { enabled } => {
                    engine.normalize = enabled;
                    if engine.kind == "file" && engine.current.is_some() {
                        let pos = engine.position_ms();
                        let play = engine.is_playing;
                        engine.restart_from(pos, play);
                    }
                }
                PlayerCommand::SetCrossfade { ms } => {
                    engine.crossfade_ms = ms.clamp(0, 12_000);
                }
                PlayerCommand::PlayUrl { url } => engine.play_url(&url),
                PlayerCommand::Next | PlayerCommand::Prev => {}
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        engine.maybe_crossfade();
        if engine.sink.empty() && engine.is_playing {
            engine.on_empty();
        }
        engine.publish();
    }
    Ok(())
}

pub fn output_device_names() -> Result<Vec<String>, String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let host = rodio::cpal::default_host();

    // 取得系統預設輸出裝置名稱
    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok())
        .filter(|n| !n.is_empty());

    let devices = host.output_devices().map_err(|e| format!("列不出輸出裝置: {e}"))?;

    // 螢幕/HDMI 音訊關鍵字（需過濾掉的裝置）
    const MONITOR_KEYWORDS: &[&str] = &[
        "NVIDIA High Definition Audio",
        "Intel(R) Display Audio",
        "Intel Display Audio",
        "AMD High Definition Audio Device",
        "Microsoft LifeCam",
    ];

    // 耳機/喇叭優先關鍵字
    const PRIORITY_KEYWORDS: &[&str] = &[
        "Headphones",
        "Headset",
        "Speakers",
        "Realtek",
        "Audio",
    ];

    let mut priority = Vec::new();
    let mut normal = Vec::new();

    for device in devices {
        if let Ok(name) = device.name() {
            if name.is_empty() || (priority.contains(&name) || normal.contains(&name)) {
                continue;
            }
            // 略過螢幕音訊裝置
            if MONITOR_KEYWORDS.iter().any(|kw| name.contains(kw)) {
                continue;
            }
            // 判斷優先順序
            let is_priority = PRIORITY_KEYWORDS.iter().any(|kw| name.contains(kw));
            if is_priority {
                priority.push(name);
            } else {
                normal.push(name);
            }
        }
    }

    // 組合：系統預設裝置優先（若不在列表中則插入最前），接著優先裝置，再一般裝置
    let mut result = Vec::new();
    if let Some(dn) = default_name {
        if !result.contains(&dn) {
            result.push(dn);
        }
    }
    for n in priority {
        if !result.contains(&n) {
            result.push(n);
        }
    }
    for n in normal {
        if !result.contains(&n) {
            result.push(n);
        }
    }

    if result.is_empty() {
        return Err("列不出輸出裝置".into());
    }
    Ok(result)
}

/// 啟動音訊執行緒。回傳指令通道與即時狀態（位置、等化器、錯誤）。
pub fn start_player_thread() -> (mpsc::Sender<PlayerCommand>, Arc<Mutex<EngineSnapshot>>) {
    let status = Arc::new(Mutex::new(EngineSnapshot::default()));
    let (tx, rx) = mpsc::channel();
    let shared = status.clone();
    thread::spawn(move || {
        if let Err(e) = audio_loop(rx, shared) {
            eprintln!("音訊執行緒結束: {e}");
        }
    });
    (tx, status)
}
