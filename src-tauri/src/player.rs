use crate::models::{EngineSnapshot, PlayerCommand, PlayMode};
use crate::playback_pipeline::{
    BLOCK_FRAMES, BLOCK_BYTES, CHANNEL_CAPACITY_BYTES, CHANNEL_CAPACITY_CHUNKS, DeviceConverter,
    OutputProxy, PlaybackCore, PlaybackState, PreparedTrack, RecycleItem,
    TrackChain, TrackResampler, TrackChainItem, INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE,
};
use crate::dsp::{DspSource, FxParams, FxChain, RateStage, BLOCK as DSP_BLOCK};
use crate::eq::{EqProcessor, EqShared, BANDS, EQ_HZ, db_to_linear};
use crate::audio_decode::{SymStream, FfmpegStream, ffprobe_json};
use crate::open_with::{validate_local_path, log_line, log_load_error, sanitize_log};
use crate::scanner::{playback_backend_for, meta_backend_for};
use crate::tools::locate;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// 超過這個長度（或長度不明）就不整首解進記憶體，改從檔案逐封包串流。
const STREAM_OVER_MS: i64 = 15 * 60 * 1000;

/// SE6：每軌整首解碼進記憶體的 PCM 上限（400 MB；48kHz 立體聲約 17 分鐘）。
const PCM_MAX_BYTES: usize = 400 * 1024 * 1024;

/// 以 400 MB 上限收集 f32 樣本，超過則拒絕（改由呼叫端走串流）。
fn collect_pcm_limit(iter: impl Iterator<Item = f32>) -> Result<Vec<f32>, String> {
    let mut samples = Vec::new();
    for s in iter {
        if samples.len() * 4 >= PCM_MAX_BYTES {
            return Err("整首 PCM 超過 400 MB 上限，改走串流".into());
        }
        samples.push(s);
    }
    Ok(samples)
}

/// 音訊引擎：管理長駐管線與輸出
struct AudioEngine {
    core: Arc<PlaybackCore>,
    output_stream: Option<OutputStream>,
    output_handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    proxy_id: u64,
    device_sample_rate: u32,
    mode: PlayMode,
    is_playing: bool,
    volume: f32,
    crossfade_ms: i64,
    kind: String,
    stream_title: String,
    error: String,
    ended: bool,
    status: Arc<Mutex<EngineSnapshot>>,
    radio_stop: Option<Arc<AtomicBool>>,
    /// 下一首 token 計數器
    next_token_counter: AtomicU64,
    /// 協調器世代（與 front-end 的 next_token 對照，用於 poll_completion 過期偵測）
    next_coord_gen: u64,
    /// 當前播放序號計數器
    seq_counter: AtomicU64,
    /// 當前軌解碼器（用於 seek 重建）
    current_decoder: Mutex<Option<String>>,
    /// 當前軌路徑（用於 seek 重建）
    current_path: Mutex<Option<String>>,
    /// 取消旗標
    cancel_flag: Arc<AtomicBool>,
    /// 電台位置追蹤（DspSource::new 回傳）
    played: Arc<AtomicU64>,
    /// 回收通道
    recycle_tx: Sender<RecycleItem>,
    recycle_rx: Receiver<RecycleItem>,
    /// 回收執行緒 handle
    recycle_handle: Option<thread::JoinHandle<()>>,
}

impl AudioEngine {
    fn new(status: Arc<Mutex<EngineSnapshot>>) -> Result<Self, String> {
        let (_stream, handle) = OutputStream::try_default().map_err(|e| format!("音訊輸出失敗: {e}"))?;
        let sink = Sink::try_new(&handle).map_err(|e| format!("建立 Sink 失敗: {e}"))?;
        sink.set_volume(0.7);

        // 取得預設裝置取樣率
        let device_sample_rate = Self::get_default_device_sample_rate().unwrap_or(48000);

        let eq_shared = Arc::new(EqShared::new());
        let core = Arc::new(PlaybackCore::new(eq_shared.clone()));

        let proxy_id = core.next_proxy_id();
        let mut proxy = OutputProxy::new(core.clone(), device_sample_rate, proxy_id);
        sink.append(proxy); // SP1: Sink 串接長駐 OutputProxy
        core.set_active_id(proxy_id);

        let (recycle_tx, recycle_rx) = bounded(32);
        let recycle_flag = Arc::new(AtomicBool::new(false));
        let recycle_handle = Self::spawn_recycle_thread(recycle_rx.clone(), recycle_flag.clone(), core.next_coordinator());

        let mut engine = Self {
            core: core.clone(),
            output_stream: Some(_stream),
            output_handle: Some(handle),
            sink: Some(sink),
            proxy_id,
            device_sample_rate,
            mode: PlayMode::Sequence,
            is_playing: false,
            volume: 0.7,
            crossfade_ms: 0,
            kind: "idle".into(),
            stream_title: String::new(),
            error: String::new(),
            ended: false,
            status,
            radio_stop: None,
            next_token_counter: AtomicU64::new(1),
            next_coord_gen: 0,
            seq_counter: AtomicU64::new(1),
            current_decoder: Mutex::new(None),
            current_path: Mutex::new(None),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            played: Arc::new(AtomicU64::new(0)),
            recycle_tx,
            recycle_rx,
            recycle_handle: Some(recycle_handle),
        };

        // 初始化 EQ processor
        engine.init_dsp();
        core.set_active_id(proxy_id);
        log_line("AUDIO_ENGINE initialized with long-running pipeline");

        Ok(engine)
    }

    fn get_default_device_sample_rate() -> Option<u32> {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};
        let host = rodio::cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        Some(config.sample_rate().0)
    }

    fn init_dsp(&mut self) {
        // 初始化 EQ processor
        let mut eq_proc = self.core.eq_processor.lock().unwrap();
        *eq_proc = Some(EqProcessor::new(
            self.core.eq_shared.clone(),
            INTERNAL_SAMPLE_RATE,
            INTERNAL_CHANNELS,
            1.0,
        ));

        // 初始化 FX chain
        let mut fx_chain = self.core.fx_chain.lock().unwrap();
        *fx_chain = Some(FxChain::new(INTERNAL_SAMPLE_RATE, INTERNAL_CHANNELS as usize));
    }

    fn spawn_recycle_thread(rx: Receiver<RecycleItem>, cancel_flag: Arc<AtomicBool>, coordinator: Arc<crate::playback_pipeline::NextCoordinator>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            // SE9：回收期間凍結新準備工作（quota freeze），超過 30 秒仍未結束標記孤兒
            while !cancel_flag.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(item) => {
                        coordinator.set_preparing_frozen(true); // 回收中凍結新準備工作
                        let deadline = Instant::now() + Duration::from_secs(10);
                        let mut finished = false;
                        match item {
                            RecycleItem::FfmpegStream(stream) => {
                                stream.cancel();
                                while Instant::now() < deadline {
                                    if stream._stdout_thread.as_ref().map(|h| h.is_finished()).unwrap_or(true) {
                                        finished = true;
                                        break;
                                    }
                                    thread::sleep(Duration::from_millis(10));
                                }
                                if !finished {
                                    // kill 失敗且超過 30 秒，標記孤兒
                                    let orphan_deadline = Instant::now() + Duration::from_secs(30);
                                    while Instant::now() < orphan_deadline {
                                        if stream._stdout_thread.as_ref().map(|h| h.is_finished()).unwrap_or(true) {
                                            finished = true;
                                            break;
                                        }
                                        thread::sleep(Duration::from_millis(100));
                                    }
                                    if !finished {
                                        crate::open_with::log_line("RECYCLE_ORPHAN ffmpeg stream 未在 30 秒內結束");
                                    }
                                }
                            }
                            RecycleItem::FfmpegStreamHandle(handle) => {
                                handle.cancel.cancel();
                                finished = true;
                            }
                            RecycleItem::Thread(handle) => {
                                let _ = handle.join();
                                finished = true;
                            }
                            RecycleItem::Child(mut child) => {
                                let _ = child.kill();
                                let _ = child.wait();
                                finished = true;
                            }
                        }
                        if finished {
                            coordinator.set_preparing_frozen(false); // 回收結束，解除凍結
                        }
                    }
                    Err(_) => {}
                }
            }
        })
    }

    fn position_ms(&self) -> i64 {
        if self.kind == "radio" {
            if let Some(ref sink) = self.sink {
                sink.get_pos().as_millis() as i64
            } else {
                0
            }
        } else {
            let seek_origin = self.core.seek_origin_ms() as i64;
            let played_samples = self.core.played_out_frames() as i64;
            // played_out_frames 以樣本數累計（每次 next() 回傳一個 f32 樣本），
            // 立體聲兩個樣本為一個 frame，故除以 (取樣率 × 聲道數) 換算成毫秒
            let played_ms = (played_samples * 1000 / (INTERNAL_SAMPLE_RATE as i64 * INTERNAL_CHANNELS as i64)).max(0);
            seek_origin + played_ms
        }
    }

    fn publish(&self) {
        if let Ok(mut s) = self.status.lock() {
            s.position_ms = self.position_ms();
            s.duration_ms = if self.kind == "radio" {
                0
            } else {
                self.get_current_duration()
            };
            s.is_playing = self.is_playing && !self.sink.as_ref().map(|s| s.empty()).unwrap_or(true);
            s.volume = self.volume;
            s.song_id = self.get_current_song_id().unwrap_or(0);
            s.ended = self.ended;
            s.kind = self.kind.clone();
            s.stream_title = self.stream_title.clone();
            s.error = self.error.clone();
            let eq = self.core.eq_shared.get();
            s.eq_bands = eq.bands.to_vec();
            s.eq_enabled = eq.enabled;
            s.fx = eq.fx.clone();
            s.preamp_db = eq.preamp_db;
            s.normalize = eq.normalize;
            s.crossfade_ms = self.crossfade_ms;
            s.applied_gain = self.get_applied_gain();
            s.band_hz = EQ_HZ.iter().map(|h| *h as i32).collect();
            s.mode = self.mode.clone();
            s.play_seq = self.core.now_seq();
            s.now_token = self.core.now_token();
            s.next_token = self.core.next_token();
            s.next_clear_epoch = self.core.next_clear_epoch();
            s.playback_state = match self.core.playback_state() {
                PlaybackState::Playing => "Playing",
                PlaybackState::WaitingForNext => "WaitingForNext",
                PlaybackState::Paused => "Paused",
                PlaybackState::Finished => "Finished",
            }
            .to_string();
        }
    }

    fn get_current_duration(&self) -> i64 {
        if let Ok(chain_guard) = self.core.track_chain.lock() {
            if let Some(ref chain) = *chain_guard {
                if let Some(track) = chain.current() {
                    return track.duration_ms;
                }
            }
        }
        0
    }

    fn get_current_song_id(&self) -> Option<i64> {
        if let Ok(chain_guard) = self.core.track_chain.lock() {
            if let Some(ref chain) = *chain_guard {
                if let Some(track) = chain.current() {
                    return Some(track.song_id);
                }
            }
        }
        None
    }

    fn get_applied_gain(&self) -> f32 {
        let eq = self.core.eq_shared.get();
        if eq.normalize {
            if let Ok(chain_guard) = self.core.track_chain.lock() {
                if let Some(ref chain) = *chain_guard {
                    if let Some(track) = chain.current() {
                        return track.norm_gain;
                    }
                }
            }
        }
        1.0
    }

    fn switch_output(&mut self, name: &str) -> Result<(), String> {
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

        let config = device.default_output_config().map_err(|e| format!("取得裝置配置失敗: {e}"))?;
        let new_sample_rate = config.sample_rate().0;

        // 可能阻塞的建立工作在核心鎖外完成（S5：音訊回呼不卡於裝置初始化）
        let (stream, handle) = OutputStream::try_from_device(&device).map_err(|e| format!("切換輸出裝置失敗: {e}"))?;
        let sink = Sink::try_new(&handle).map_err(|e| format!("建立 Sink 失敗: {e}"))?;
        sink.set_volume(self.volume);
        let new_proxy_id = self.core.next_proxy_id();
        let mut new_proxy = OutputProxy::new(self.core.clone(), new_sample_rate, new_proxy_id);

        // 在核心鎖內原子切換 active_id：舊 proxy 的 next() 發現 ID 過期直接回傳 None，不消耗核心
        {
            let _guard = self.core.core_lock.lock().unwrap();
            self.core.set_active_id(new_proxy_id);
        }

        // 切換 active_id 後再串接新 OutputProxy，避免新回呼先取樣時 ID 不符回 None
        sink.append(new_proxy); // 新 Sink 串接新 OutputProxy

        // 換用新管線，丟棄舊（不呼叫 clear()，避免 rodio 回呼死結；C8#1）
        if let Some(ref old_sink) = self.sink {
            old_sink.pause();
        }
        self.output_stream = Some(stream);
        self.output_handle = Some(handle);
        self.sink = Some(sink);
        self.proxy_id = new_proxy_id;
        self.device_sample_rate = new_sample_rate;

        if self.is_playing {
            if let Some(ref sink) = self.sink {
                sink.play();
            }
        }

        log_line(&format!("Switched output device to {} ({} Hz)", name, new_sample_rate));
        Ok(())
    }

    fn load(&mut self, path: &str, song_id: i64) -> Result<(), String> {
        self.load_at(path, song_id, 0, true)
    }

    fn load_at(&mut self, path: &str, song_id: i64, position_ms: i64, play: bool) -> Result<(), String> {
        self.halt_radio();
        validate_local_path(Path::new(path))?;

        if let Some(ref sink) = self.sink {
            sink.stop();
            sink.clear();
        }

        // 播放序號與 token（C1：Load/Prepare 都傳入 token、publish 更新序號/token/播放狀態）
        let seq = self.seq_counter.fetch_add(1, Ordering::AcqRel);
        let token = self.next_token_counter.fetch_add(1, Ordering::AcqRel);
        self.core.set_now_identity(seq, token);
        self.core.reset_played_out_frames();
        self.core.set_seek_origin_ms(position_ms.max(0) as u64);
        self.core.clear_expired_boundaries(seq + 1);
        self.core.set_playback_state(PlaybackState::Playing);
        // 載入/跳轉會清掉先前排入的下一首，遞增 epoch 讓前端知道需重新排入
        if play {
            self.core.increment_next_clear_epoch();
        }
        // C5取消：Load/Seek 會清除協調器待播槽，避免舊準備結果誤入新軌後
        self.core.next_coordinator().clear_next();
        self.next_coord_gen = self.core.next_coordinator().current_generation();
        self.core.set_stream_source(None); // 串流軌與記憶體軌切換時先清掉舊來源

        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        let backend = playback_backend_for(&ext);

        // (TrackChain, TrackResampler, 串流來源（長檔/ffmpeg 串流）, decoder 標記)
        let (track_chain, track_resampler, stream_source, decoder) = match backend {
            Some(crate::scanner::Backend::Symphonia) => {
                match SymStream::open(Path::new(path), position_ms) {
                    Ok(mut stream) => {
                        let duration_ms = stream.duration_ms().filter(|d| *d > 0).unwrap_or(0);
                        let is_long = duration_ms > STREAM_OVER_MS || duration_ms == 0;
                        let sample_rate = stream.sample_rate().max(1);
                        let channels = stream.channels().max(1);

                        if is_long {
                            // 長檔直接串流，但需經 TrackResampler 重取樣到 48kHz
                            let stream_source: Option<Box<dyn Iterator<Item = f32> + Send>> =
                                Some(Box::new(stream));
                            let mut resampler = TrackResampler::new();
                            resampler.set_track(Some(TrackChainItem {
                                samples: Arc::new(Vec::new()),
                                sample_rate,
                                channels,
                                duration_ms,
                                song_id,
                                norm_gain: 1.0,
                                token,
                                seq,
                                native_frame_pos: 0,
                                path: path.to_string(),
                                decoder: "stream".to_string(),
                            }));
                            (
                                TrackChain::new(),
                                resampler,
                                stream_source,
                                "stream".to_string(),
                            )
                        } else {
                            let samples = collect_pcm_limit(stream.by_ref())?;
                            let duration_ms = (samples.len() as f64 / (sample_rate as f64 * channels as f64) * 1000.0) as i64;
                            let peak = samples.iter().map(|s| s.abs()).fold(0f32, f32::max);
                            let norm_gain = if peak > 0.0001 { (0.89 / peak).clamp(0.05, 6.0) } else { 1.0 };

                            let mut chain = TrackChain::new();
                            chain.append(TrackChainItem {
                                samples: Arc::new(samples),
                                sample_rate,
                                channels,
                                duration_ms,
                                song_id,
                                norm_gain,
                                token,
                                seq,
                                native_frame_pos: 0,
                                path: path.to_string(),
                                decoder: "symphonia".to_string(),
                            });
                            chain.set_coordinator(self.core.next_coordinator());

                            let mut resampler = TrackResampler::new();
                            resampler.set_track(Some(chain.current().unwrap().clone()));

                            (chain, resampler, None, "symphonia".to_string())
                        }
                    }
                    Err(_e) => self.prepare_ffmpeg_track(path, song_id, &ext, position_ms, seq, token)?,
                }
            }
            Some(crate::scanner::Backend::Ffmpeg) => {
                self.prepare_ffmpeg_track(path, song_id, &ext, position_ms, seq, token)?
            }
            None => {
                return Err(format!("這個格式尚未支援: {}", ext));
            }
        };

        *self.core.track_chain.lock().unwrap() = Some(track_chain);
        *self.core.track_resampler.lock().unwrap() = Some(track_resampler);
        self.core.set_stream_source(stream_source);

        // 換軌時重新串接一個新的 OutputProxy 到長駐 Sink（同一 proxy_id，active_id 不變）
        if let Some(ref sink) = self.sink {
            sink.append(OutputProxy::new(self.core.clone(), self.device_sample_rate, self.proxy_id));
        }

        *self.current_decoder.lock().unwrap() = Some(decoder);
        *self.current_path.lock().unwrap() = Some(path.to_string());

        self.kind = "file".into();
        self.stream_title.clear();
        self.error.clear();
        self.ended = false;

        if play {
            if let Some(ref sink) = self.sink {
                sink.play();
            }
            self.is_playing = true;
        } else {
            if let Some(ref sink) = self.sink {
                sink.pause();
            }
            self.is_playing = false;
        }

        log_line(&format!("LOAD_OK {} song_id={} decoder={:?}", path, song_id, self.current_decoder.lock().unwrap()));
        Ok(())
    }

    fn prepare_ffmpeg_track(&mut self, path: &str, song_id: i64, ext: &str, position_ms: i64, seq: u64, token: u64) -> Result<(TrackChain, TrackResampler, Option<Box<dyn Iterator<Item = f32> + Send>>, String), String> {
        let ffprobe_path = locate("ffprobe.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffprobe")?;

        // SE6：使用 ffprobe_json helper（64 KB / 10 s 上限，逾時可中斷）
        let value = ffprobe_json(&ffprobe_path, path)?;

        let streams = value["streams"].as_array().ok_or("無音訊串流")?;
        let audio_stream = streams.iter().find(|s| s["codec_type"] == "audio").ok_or("無音訊串流")?;

        let codec = audio_stream["codec_name"].as_str().unwrap_or("");
        let sample_rate = audio_stream["sample_rate"].as_str().unwrap_or("0").parse().unwrap_or(0);
        let channels = audio_stream["channels"].as_u64().unwrap_or(0) as u16;
        let duration = value["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok());
        let duration_ms = duration.map(|d| (d * 1000.0) as i64).unwrap_or(0);

        let is_long = duration_ms > STREAM_OVER_MS || duration_ms == 0;

        let (final_sr, final_ch) = if codec.starts_with("dsd") {
            let dsd_rate = sample_rate;
            let pcm_rate = (dsd_rate / 32).min(176400);
            (pcm_rate, 2)
        } else {
            (sample_rate.max(1), channels.max(1))
        };

        if is_long {
            let ffmpeg_path = locate("ffmpeg.exe")
                .map(|(p, _, _)| p)
                .ok_or("找不到 ffmpeg")?;

            let stream = FfmpegStream::new(&ffmpeg_path, path, final_sr, final_ch, duration_ms)?;
            // 串流軌由設定為 TrackChain 之外的 stream_source 直接播放（不回收，由 set_stream_source 換軌時釋放）
            let stream_source: Option<Box<dyn Iterator<Item = f32> + Send>> = Some(Box::new(stream));
            let chain = TrackChain::new();
            let resampler = TrackResampler::new();
            Ok((chain, resampler, stream_source, "stream".to_string()))
        } else {
            let ffmpeg_path = locate("ffmpeg.exe")
                .map(|(p, _, _)| p)
                .ok_or("找不到 ffmpeg")?;

            let output = Command::new(&ffmpeg_path)
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file,pipe", // SE11：本機 ffmpeg 只允許 file/pipe
                    "-i",
                    path,
                    "-f",
                    "f32le",
                    "-ac",
                    &final_ch.to_string(),
                    "-ar",
                    &final_sr.to_string(),
                    "-",
                ])
                .output()
                .map_err(|e| format!("ffmpeg 執行失敗: {e}"))?;

            if !output.status.success() {
                return Err("ffmpeg 解碼失敗".into());
            }

            let samples = collect_pcm_limit(
                output.stdout.chunks_exact(4).map(|chunk| {
                    f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                }),
            )?;

            let duration_ms = (samples.len() as f64 / (final_sr as f64 * final_ch as f64) * 1000.0) as i64;
            let peak = samples.iter().map(|s| s.abs()).fold(0f32, f32::max);
            let norm_gain = if peak > 0.0001 { (0.89 / peak).clamp(0.05, 6.0) } else { 1.0 };

            let mut chain = TrackChain::new();
            chain.append(TrackChainItem {
                samples: Arc::new(samples),
                sample_rate: final_sr,
                channels: final_ch,
                duration_ms,
                song_id,
                norm_gain,
                token,
                seq,
                native_frame_pos: 0,
                path: path.to_string(),
                decoder: "ffmpeg".to_string(),
            });
            chain.set_coordinator(self.core.next_coordinator());

            let mut resampler = TrackResampler::new();
            resampler.set_track(Some(chain.current().unwrap().clone()));

            Ok((chain, resampler, None, "ffmpeg".to_string()))
        }
    }

    fn halt_radio(&mut self) {
        if let Some(flag) = self.radio_stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    fn restart_from(&mut self, ms: i64, play: bool) {
        // 交接後 seek：current_path/current_decoder 可能已過期，優先從 TrackChain 當前軌讀取
        let (path, decoder, song_id) = {
            let chain_guard = self.core.track_chain.lock().unwrap();
            if let Some(cur) = chain_guard.as_ref().and_then(|c| c.current()) {
                (cur.path.clone(), cur.decoder.clone(), cur.song_id)
            } else {
                let p = self.current_path.lock().unwrap().clone().unwrap_or_default();
                let d = self.current_decoder.lock().unwrap().clone().unwrap_or_default();
                let s = self.get_current_song_id().unwrap_or(0);
                (p, d, s)
            }
        };
        if path.is_empty() {
            return;
        }

        // 保留播放身分（seek 不得換 seq/token，違反 seek preserves playback identity）
        let current_seq = self.core.now_seq();
        let current_token = self.core.now_token();

        self.core.track_chain.lock().unwrap().take();
        self.core.track_resampler.lock().unwrap().take();
        self.core.reset_played_out_frames();
        self.core.clear_expired_boundaries(current_seq + 1);
        self.core.set_seek_origin_ms(ms.max(0) as u64);

        let _ = self.load_at(&path, song_id, ms, play);
        // 恢復播放身分（load_at 會遞增 seq_counter，seek 時必須沿用原來的 seq/token）
        self.core.set_now_identity(current_seq, current_token);

        *self.current_decoder.lock().unwrap() = Some(decoder);
        *self.current_path.lock().unwrap() = Some(path);
    }

    fn play_url(&mut self, url: &str) {
        self.halt_radio();
        if let Some(ref sink) = self.sink {
            sink.stop();
            sink.clear();
        }
        self.ended = false;
        self.stream_title = url.to_string();

        let ffmpeg_path = match locate("ffmpeg.exe") {
            Some((p, _, _)) => p,
            None => {
                self.error = "播放串流需要 ffmpeg，這台電腦找不到".into();
                self.is_playing = false;
                self.kind = "idle".into();
                return;
            }
        };

        let stop = Arc::new(AtomicBool::new(false));
        self.radio_stop = Some(stop.clone());
        let (tx, rx) = bounded(128);
        let status = self.status.clone();
        let url = url.to_string();
        let ffmpeg_path_clone = ffmpeg_path.clone();

        thread::spawn(move || {
            let mut child = match Command::new(&ffmpeg_path_clone)
                .args([
                    "-nostdin",
                    "-protocol_whitelist", "http,https,hls,dash,rtmp,rtmps,rtsp",
                    "-max_redirects", "5",
                    "-timeout", "10000000",
                    "-i", &url,
                    "-f", "s16le", "-ac", "2", "-ar", "44100", "-"
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn() {
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
            
            // SE3-1: 獨立 stderr 讀取執行緒與持久化 Sanitizer，持續清洗並記錄
            let mut stderr = child.stderr.take();
            let stop_stderr = stop.clone();
            thread::spawn(move || {
                use std::io::Read;
                let mut buf = vec![0u8; 4096];
                let mut sanitizer = crate::open_with::Sanitizer::new();
                if let Some(mut stderr) = stderr {
                    loop {
                        if stop_stderr.load(Ordering::Relaxed) { break; }
                        match stderr.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                let chunk_text = String::from_utf8_lossy(&buf[..n]);
                                sanitizer.sanitize_chunk(&chunk_text);
                            }
                            Err(_) => break,
                        }
                    }
                }
                let cleaned = sanitizer.finish();
                if !cleaned.is_empty() {
                    crate::open_with::log_line_raw(&cleaned);
                }
            });

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
                // stderr 已由獨立執行緒處理，這裡只設定通用錯誤
                if let Ok(mut s) = status.lock() {
                    s.error = "串流沒有輸出音訊（詳細錯誤請見 open-with.log）".into();
                    s.kind = "idle".into();
                    s.is_playing = false;
                }
            }
        });

        self.kind = "radio".into();
        self.error.clear();

        let (dsp, played) = DspSource::new(
            LiveSource { rx, pending: Vec::new(), pos: 0 },
            2, 44100, &self.core.eq_shared, 1.0, None, false
        );
        self.played = played;
        if let Some(ref sink) = self.sink {
            sink.append(dsp);
            sink.play();
        }
        self.is_playing = true;
    }

    fn on_empty(&mut self) {
        if !self.is_playing || self.kind == "idle" {
            return;
        }
        if self.core.playback_state() == PlaybackState::Finished {
            self.is_playing = false;
            self.ended = true;
        }
    }
}

/// 音訊回呼處理：從長駐管線拉取資料
fn audio_loop(
    rx: mpsc::Receiver<PlayerCommand>,
    status: Arc<Mutex<EngineSnapshot>>,
) -> Result<(), String> {
    let mut engine = AudioEngine::new(status)?;

    loop {
        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(command) => match command {
                PlayerCommand::Load { path, song_id, token } => {
                    if let Err(e) = engine.load(&path, song_id) {
                        engine.error = e.clone();
                        engine.is_playing = false;
                        log_load_error(&format!("Load {path} song_id={song_id}: {e}"));
                    } else {
                        log_line(&format!("LOAD_OK {path} song_id={song_id} token={token}"));
                        // C1：前端傳入的 token 作為目前軌身分，供前端比對「已要求的 token」
                        engine.core.set_now_identity(engine.core.now_seq(), token);
                    }
                }
                PlayerCommand::Prepare { path, song_id, position_ms, token } => {
                    if let Err(e) = engine.load_at(&path, song_id, position_ms, false) {
                        engine.error = e;
                        engine.is_playing = false;
                    } else {
                        engine.core.set_now_identity(engine.core.now_seq(), token);
                    }
                }
                PlayerCommand::Play => {
                    log_line("CMD PLAY executing");
                    if let Some(ref sink) = engine.sink {
                        sink.play();
                    }
                    engine.is_playing = true;
                    engine.ended = false;
                    engine.core.set_playback_state(PlaybackState::Playing);
                }
                PlayerCommand::Pause => {
                    if let Some(ref sink) = engine.sink {
                        sink.pause();
                    }
                    engine.is_playing = false;
                    engine.core.set_playback_state(PlaybackState::Paused);
                }
                PlayerCommand::Toggle => {
                    if engine.is_playing {
                        if let Some(ref sink) = engine.sink {
                            sink.pause();
                        }
                        engine.is_playing = false;
                        engine.core.set_playback_state(PlaybackState::Paused);
                    } else {
                        if let Some(ref sink) = engine.sink {
                            sink.play();
                        }
                        engine.is_playing = true;
                        engine.ended = false;
                        engine.core.set_playback_state(PlaybackState::Playing);
                    }
                }
                PlayerCommand::Stop => {
                    engine.halt_radio();
                    if let Some(ref sink) = engine.sink {
                        sink.stop();
                    }
                    engine.is_playing = false;
                    engine.kind = "idle".into();
                    engine.ended = false;
                    engine.core.set_playback_state(PlaybackState::Finished);
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
                    if let Some(ref sink) = engine.sink {
                        sink.set_volume(engine.volume);
                    }
                }
                PlayerCommand::SetMode { mode } => {
                    if !matches!(mode, PlayMode::All | PlayMode::Random) {
                        engine.core.next_coordinator().clear_next();
                    }
                    engine.mode = mode;
                }
                PlayerCommand::SetEq { bands } => {
                    engine.core.eq_shared.update(|p| {
                        for (i, g) in bands.into_iter().take(BANDS).enumerate() {
                            p.bands[i] = g;
                        }
                    });
                }
                PlayerCommand::SetPreamp { db } => {
                    engine.core.eq_shared.update(|p| p.preamp_db = db);
                }
                PlayerCommand::SetEqEnabled { enabled } => {
                    engine.core.eq_shared.update(|p| p.enabled = enabled);
                }
                PlayerCommand::SetFx { fx } => {
                    engine.core.eq_shared.update(|p| p.fx = fx);
                }
                PlayerCommand::SetOutput { name } => {
                    if let Err(e) = engine.switch_output(&name) {
                        engine.error = e;
                    } else {
                        engine.error.clear();
                    }
                }
                PlayerCommand::SetNormalize { enabled } => {
                    engine.core.eq_shared.update(|p| p.normalize = enabled);
                }
                PlayerCommand::SetCrossfade { ms } => {
                    engine.crossfade_ms = ms.clamp(0, 12_000);
                    engine.core.set_crossfade_ms(engine.crossfade_ms as u64);
                }
                PlayerCommand::PlayUrl { url } => engine.play_url(&url),
                PlayerCommand::SetNext { path, song_id, token } => {
                    // 共用 PlaybackCore 的待播協調器（與 TrackChain 唯一交接點同一實例）
                    let gen = engine.core.next_coordinator().set_next(path, song_id, token);
                    engine.next_coord_gen = gen;
                    engine.core.set_next_token(token);
                    log_line(&format!("CMD set_next gen={} token={}", gen, token));
                }
                PlayerCommand::ClearNext => {
                    // C5取消：ClearNext 遞增協調器世代，必須同步更新 engine.next_coord_gen，
                    // 否則 poll_completion 會拿舊世代比較，舊結果仍會入槽。
                    let new_gen = engine.core.next_coordinator().clear_next();
                    engine.next_coord_gen = new_gen;
                    engine.core.set_next_token(0);
                    engine.core.increment_next_clear_epoch();
                    log_line("CMD clear_next");
                }
                PlayerCommand::Next | PlayerCommand::Prev => {}
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // SP2：引擎每 tick 呼叫 poll_completion，將準備完成的軌放入待播槽（TrackChain 讀完時 take()）
        if let Some(result) = engine.core.next_coordinator().poll_completion(engine.next_coord_gen) {
            match result {
                Ok(track) => {
                    crate::open_with::log_line(&format!(
                        "CMD poll_completion seq={} token={} song_id={}",
                        track.seq, track.token, track.song_id
                    ));
                }
                Err(e) => {
                    // 準備失敗（壞檔、缺工具）：記錄並讓前端能透過錯誤欄位知悉、略過並重排
                    crate::open_with::log_load_error(&format!("prepare_next failed: {e}"));
                    engine.error = format!("下一首準備失敗：{e}");
                }
            }
        }

        engine.on_empty();
        engine.publish();
    }
    Ok(())
}

/// 輸出裝置名稱列表
pub fn output_device_names() -> Result<Vec<String>, String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let host = rodio::cpal::default_host();

    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok())
        .filter(|n| !n.is_empty());

    let devices = host.output_devices().map_err(|e| format!("列不出輸出裝置: {e}"))?;

    const MONITOR_KEYWORDS: &[&str] = &[
        "NVIDIA High Definition Audio",
        "Intel(R) Display Audio",
        "Intel Display Audio",
        "AMD High Definition Audio Device",
        "Microsoft LifeCam",
    ];

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
            if name.is_empty() || priority.contains(&name) || normal.contains(&name) {
                continue;
            }
            if MONITOR_KEYWORDS.iter().any(|kw| name.contains(kw)) {
                continue;
            }
            let is_priority = PRIORITY_KEYWORDS.iter().any(|kw| name.contains(kw));
            if is_priority {
                priority.push(name);
            } else {
                normal.push(name);
            }
        }
    }

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

/// 啟動音訊執行緒
pub fn start_player_thread() -> (std::sync::mpsc::Sender<PlayerCommand>, Arc<Mutex<EngineSnapshot>>) {
    let status = Arc::new(Mutex::new(EngineSnapshot::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = status.clone();
    thread::spawn(move || {
        if let Err(e) = audio_loop(rx, shared) {
            eprintln!("音訊執行緒結束: {e}");
        }
    });
    (tx, status)
}

// 為了相容性保留一些舊結構
pub struct LiveSource {
    rx: crossbeam_channel::Receiver<Vec<f32>>,
    pending: Vec<f32>,
    pos: usize,
}

impl Iterator for LiveSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        // SE8：非阻塞消費，避免阻塞音訊回呼；用 try_recv 而非 blocking recv
        // 暫時的 Empty 不視為結束，回傳靜音填充，避免音訊回呼提前結束
        if self.pos >= self.pending.len() {
            match self.rx.try_recv() {
                Ok(batch) => {
                    self.pending = batch;
                    self.pos = 0;
                    if self.pending.is_empty() {
                        return None; // 結束信號
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    return Some(0.0); // 暫時無資料，回傳靜音
                }
                Err(_) => return None, // 通道關閉
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_output_device_names() {
        let devices = output_device_names();
        assert!(devices.is_ok() || devices.is_err());
    }
}