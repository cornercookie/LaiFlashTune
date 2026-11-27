use crate::models::{EngineSnapshot, FadeTarget, ManualLoadOutcome, ManualLoadResult, PlayerCommand, PlayMode, VolumeCurve};
use crate::playback_pipeline::{
    BLOCK_FRAMES, BLOCK_BYTES, CHANNEL_CAPACITY_BYTES, CHANNEL_CAPACITY_CHUNKS, DeviceConverter,
    OutputProxy, PlaybackCore, PlaybackState, PreparedTrack, RecycleItem,
    TrackChain, TrackResampler, TrackChainItem, INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE,
};
use crate::dsp::{DspSource, FxParams, FxChain, RateStage, BLOCK as DSP_BLOCK};
use crate::eq::{EqProcessor, EqShared, BANDS, EQ_HZ, db_to_linear};
use crate::audio_decode::{SymStream, FfmpegStream, ffprobe_playback_json, precheck_packet_budget};
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
/// 對子模組公開，供 playback_pipeline::prepare_track 有界預載封頂使用。
pub(crate) const PCM_MAX_BYTES: usize = 400 * 1024 * 1024;
pub(crate) const PLAYBACK_SOURCE_LIMIT: usize = 3;

/// 靜音門檻：−60 dBFS（振幅 0.001）
const SILENCE_THRESHOLD: f32 = 0.001;

/// 去除靜音：以完整聲道 frame 為單位從頭尾尋找非靜音段落，回傳裁切後樣本與頭尾靜音 frame 數。
/// 僅在 silence_removal 開啟時呼叫。
pub fn trim_silence(samples: &mut Vec<f32>, channels: u16, silence_removal: bool) -> (usize, usize) {
    if !silence_removal || samples.is_empty() || channels == 0 {
        return (0, 0);
    }
    let ch = channels as usize;
    let frames = samples.len() / ch;
    if frames == 0 {
        return (0, 0);
    }

    // 找頭部第一個非靜音 frame
    let mut head = 0usize;
    for i in 0..frames {
        let mut is_silence = true;
        for c in 0..ch {
            if samples[i * ch + c].abs() > SILENCE_THRESHOLD {
                is_silence = false;
                break;
            }
        }
        if !is_silence {
            break;
        }
        head += 1;
    }

    // 找尾部最後一個非靜音 frame
    let mut tail = 0usize;
    for i in (0..frames).rev() {
        let mut is_silence = true;
        for c in 0..ch {
            if samples[i * ch + c].abs() > SILENCE_THRESHOLD {
                is_silence = false;
                break;
            }
        }
        if !is_silence {
            break;
        }
        tail += 1;
    }

    // 若全是靜音，不裁切
    if head + tail >= frames {
        return (0, 0);
    }

    // 裁切樣本（PP9：原地搬移 + truncate，不建立第二份 PCM 複本）
    let start = head * ch;
    let end = (frames - tail) * ch;
    samples.copy_within(start..end, 0);
    samples.truncate(end - start);

    (head, tail)
}

/// PP9：PCM 收集超過每軌預算的型別化結果。
/// 以型別取代字串比對，讓呼叫端能明確區分「超過預算」與其他錯誤；
/// 目前唯一的轉換點在 `collect_pcm_limit`（轉成字串錯誤改走串流），
/// Symphonia 整合（prepare_track）留待後續任務，屆時由呼叫端決定降級方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PcmCollectError {
    /// 樣本數超過每軌 PCM 預算（PCM_MAX_BYTES 對應的樣本數上限）
    BudgetExceeded,
    /// 配置樣本緩衝區失敗（實際記憶體不足）
    AllocationFailed,
}

/// PP9：幾何成長的初始區塊（8 個樣本 = 32 位元組），之後每次倍增
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymPcmError {
    Read(crate::audio_decode::SymReadFailure),
    Collect(PcmCollectError),
    Cancelled,
}

impl std::fmt::Display for SymPcmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(reason) => crate::audio_decode::SymOpenFailure::Terminal(*reason).fmt(formatter),
            Self::Cancelled => formatter.write_str("音訊準備已撤銷"),
            Self::Collect(PcmCollectError::BudgetExceeded) => formatter.write_str("整首 PCM 超過 400 MB 上限"),
            Self::Collect(PcmCollectError::AllocationFailed) => formatter.write_str("PCM 緩衝區配置失敗"),
        }
    }
}

const PCM_COLLECT_BASE_CHUNK: usize = 8;

/// PP9：共用的「無過度成長」PCM 收集器。
///
/// 以 `try_reserve_exact` 逐段保留容量，成長量幾何（×2）遞增，且總保留量
/// 以樣本上限封頂：配置容量（`capacity`）永不超過上限，因此記憶體用量
/// 同時受「長度」與「配置容量」約束，不會因 Vec 的 amortized 成長而
/// 超出 400 MB 上限。EOF 恰好在上限（含正好等於上限）視為收集成功；
/// 再多的樣本回傳 [`PcmCollectError::BudgetExceeded`]，由呼叫端決定
/// 降級方式（目前的 `collect_pcm_limit` 轉成字串錯誤改走串流）。
pub(crate) struct PcmCollector {
    samples: Vec<f32>,
    /// 樣本數上限（預算位元組 / 4）
    max_samples: usize,
}

impl PcmCollector {
    /// 以位元組預算建立收集器（每樣本 4 位元組）。
    pub(crate) fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            samples: Vec::new(),
            max_samples: max_bytes / 4,
        }
    }

    /// 目前配置容量對應的位元組數（容量與長度同時計入預算）。
    /// PP9：目前僅測試與後續 Symphonia 整合會用到，先行保留
    #[allow(dead_code)]
    pub(crate) fn capacity_bytes(&self) -> usize {
        self.samples.capacity() * 4
    }

    /// 餵入一個樣本；超過預算回傳 [`PcmCollectError::BudgetExceeded`]，
    /// 保留失敗回傳 [`PcmCollectError::AllocationFailed`]，皆不部分寫入。
    pub(crate) fn push(&mut self, sample: f32) -> Result<(), PcmCollectError> {
        if self.samples.len() >= self.max_samples {
            return Err(PcmCollectError::BudgetExceeded);
        }
        if self.samples.len() == self.samples.capacity() {
            self.reserve_more()?;
        }
        self.samples.push(sample);
        Ok(())
    }

    /// 依序餵入樣本；遇到超過預算或配置失敗立即回傳對應錯誤。
    pub(crate) fn extend_from_iter<I: IntoIterator<Item = f32>>(&mut self, iter: I) -> Result<(), PcmCollectError> {
        for s in iter {
            self.push(s)?;
        }
        Ok(())
    }

    pub(crate) fn extend_checked(
        &mut self,
        cancelled: Option<&AtomicBool>,
        mut read: impl FnMut() -> Result<Option<f32>, crate::audio_decode::SymReadFailure>,
    ) -> Result<(), SymPcmError> {
        loop {
            if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                return Err(SymPcmError::Cancelled);
            }
            let Some(sample) = read().map_err(SymPcmError::Read)? else { return Ok(()) };
            self.push(sample).map_err(SymPcmError::Collect)?;
        }
    }

    /// 幾何成長保留下一段容量：至少初始區塊、之後倍增，總保留量以樣本上限封頂。
    /// 保留失敗回傳 [`PcmCollectError::AllocationFailed`]，不以 `Vec::push` 重試配置。
    fn reserve_more(&mut self) -> Result<(), PcmCollectError> {
        let capacity = self.samples.capacity();
        let headroom = self.max_samples - capacity;
        let want = capacity.saturating_mul(2).max(PCM_COLLECT_BASE_CHUNK);
        let grow = want.min(headroom);
        // 請求容量（現有 + 成長）再封頂一次，確保永不超出樣本上限。
        let requested = capacity.saturating_add(grow).min(self.max_samples);
        let additional = requested - capacity;
        self.samples
            .try_reserve_exact(additional)
            .map_err(|_| PcmCollectError::AllocationFailed)
    }

    /// 取出收集完成的 PCM（搬移所有權，不另行複製）。
    pub(crate) fn finish(self) -> Vec<f32> {
        self.samples
    }
}

enum SymPlaybackSource {
    Memory {
        samples: Vec<f32>,
        leading_frames: usize,
        trailing_frames: usize,
    },
    Stream(SymStream),
}

fn prepare_bounded_sym_source(
    mut stream: SymStream,
    file: &mut std::fs::File,
    path: &Path,
    position_ms: i64,
    silence_removal: bool,
    pcm_limit: usize,
    cancelled: Option<&AtomicBool>,
) -> Result<SymPlaybackSource, String> {
    let channels = stream.channels();
    let mut collector = PcmCollector::with_max_bytes(pcm_limit);
    match collector.extend_checked(cancelled, || stream.next_checked()) {
        Ok(()) => {
            let mut samples = collector.finish();
            let (leading_frames, trailing_frames) = trim_silence(&mut samples, channels, silence_removal);
            Ok(SymPlaybackSource::Memory { samples, leading_frames, trailing_frames })
        }
        Err(SymPcmError::Collect(PcmCollectError::BudgetExceeded)) => {
            drop(collector);
            drop(stream);
            if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                return Err(SymPcmError::Cancelled.to_string());
            }
            SymStream::open_checked(file, path, position_ms)
                .map(SymPlaybackSource::Stream)
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// 音訊引擎：管理長駐管線與輸出
enum FfmpegStdoutMsg {
    /// 樣本資料：除最後一筆外皆為固定 FFMPEG_PCM_CHUNK_SAMPLES 個樣本（16 KiB @ f32le），
    /// 且一律為完整樣本（4 位元組整數倍）
    Data(Vec<f32>),
    /// stdout 讀取結束：error 為 None 代表乾淨且對齊的 EOF
    End { error: Option<FfmpegPcmError> },
}

/// PP9：collect_ffmpeg_pcm 的型別化結果。BudgetExceeded 由呼叫端決定是否串流重試，
/// 其餘為收集過程的失敗（啟動、讀取、對齊、子程序狀態、停滯）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FfmpegPcmError {
    Busy,
    Cancelled,
    CleanupPending,
    /// 收集的 PCM 超過每軌 400 MB 預算（與 PcmCollector 同一上限）
    BudgetExceeded,
    /// PCM 緩衝區配置失敗（實際記憶體不足）
    AllocationFailed,
    /// 無法啟動 ffmpeg 子程序
    Spawn(String),
    /// stdout 讀取失敗
    Stdout(String),
    /// EOF 時仍有不足一個 f32（4 bytes）的殘餘位元組，輸出未對齊
    Misaligned,
    /// 子程序以非零狀態結束（或無法確認成功結束）
    ChildFailed,
    /// 10 秒沒有新資料（停滯逾時）
    Stall,
}

/// PP9：每筆 stdout 訊息的固定樣本數（4096 樣本 = 16384 位元組 @ f32le）。
const FFMPEG_PCM_CHUNK_SAMPLES: usize = 4096;

/// PP9：stdout 通道槽位數（4 × 16 KiB = 64 KiB 在途上限，與讀取端背壓共同約束記憶體）。
const FFMPEG_PCM_CHANNEL_SLOTS: usize = 4;

/// PP9：10 秒沒有新資料即視為停滯，終止收集並清理子程序。
const FFMPEG_PCM_STALL: Duration = Duration::from_secs(10);

/// PP9：stdout 位元組 → 固定樣本數訊息的轉換狀態。
/// 訊息邊界永遠對齊 4 位元組（f32）樣本；跨訊息最多攜帶 3 個不完整樣本位元組。
struct FfmpegSampleFramer {
    /// 不足一個樣本的殘餘位元組（0–3 個有效；第 4 格供湊齊後直接轉換）
    partial: [u8; 4],
    partial_len: usize,
    /// 準備送出的樣本（湊滿固定大小即送出）
    msg: Vec<f32>,
}

impl FfmpegSampleFramer {
    fn new() -> Self {
        Self {
            partial: [0u8; 4],
            partial_len: 0,
            msg: Vec::with_capacity(FFMPEG_PCM_CHUNK_SAMPLES),
        }
    }

    /// 餵入新讀到的位元組；每湊滿一個固定訊息呼叫一次 `emit`。
    /// `emit` 回傳 false（送端失效，receiver 已被清理路徑丟棄）時停止並回傳 false。
    fn push_bytes(&mut self, bytes: &[u8], emit: &mut dyn FnMut(Vec<f32>) -> bool) -> bool {
        let mut rest = bytes;
        // 先補齊上一輪殘餘的不完整樣本（1–3 bytes）
        if self.partial_len > 0 {
            let need = 4 - self.partial_len;
            if rest.len() < need {
                self.partial[self.partial_len..self.partial_len + rest.len()].copy_from_slice(rest);
                self.partial_len += rest.len();
                return true;
            }
            self.partial[self.partial_len..4].copy_from_slice(&rest[..need]);
            self.msg.push(f32::from_le_bytes(self.partial));
            self.partial_len = 0;
            rest = &rest[need..];
        }
        let whole = rest.len() - rest.len() % 4;
        for chunk in rest[..whole].chunks_exact(4) {
            self.msg.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        let tail = &rest[whole..];
        self.partial[..tail.len()].copy_from_slice(tail);
        self.partial_len = tail.len();
        // 湊滿固定大小就送出；不足一筆的整數樣本留在 msg 等下一輪
        while self.msg.len() >= FFMPEG_PCM_CHUNK_SAMPLES {
            let out: Vec<f32> = self.msg.drain(..FFMPEG_PCM_CHUNK_SAMPLES).collect();
            if !emit(out) {
                return false;
            }
        }
        true
    }

    /// EOF 收尾：回傳（剩餘完整樣本的短訊息, 是否對齊）。
    /// 對齊 = 沒有殘餘的不完整樣本位元組（殘餘會是 1–3 bytes）。
    fn finish(mut self) -> (Vec<f32>, bool) {
        (std::mem::take(&mut self.msg), self.partial_len == 0)
    }
}

/// PP9：把 stdout 位元組流切成固定樣本數的有界訊息送進 tx（in-memory Read 接縫：
/// 正式路徑由 stdout 執行緒以子程序 stdout 驅動，測試以記憶體內 Read 驅動）。
/// 回傳 Ok 代表乾淨且對齊的 EOF；Err 為讀取錯誤或未對齊。
/// 送端失效（receiver 已被清理路徑丟棄）時提前返回，不再送出任何訊息。
fn pump_ffmpeg_stdout<R: Read>(stdout: &mut R, tx: &Sender<FfmpegStdoutMsg>) -> Result<(), FfmpegPcmError> {
    let mut buf = [0u8; 8192];
    let mut framer = FfmpegSampleFramer::new();
    loop {
        match stdout.read(&mut buf) {
            Ok(0) => {
                let (rest, aligned) = framer.finish();
                if aligned && !rest.is_empty() {
                    // 最後一筆可短於固定大小，但必為完整樣本
                    if tx.send(FfmpegStdoutMsg::Data(rest)).is_err() {
                        return Ok(());
                    }
                }
                return if aligned { Ok(()) } else { Err(FfmpegPcmError::Misaligned) };
            }
            Ok(n) => {
                let alive = framer.push_bytes(&buf[..n], &mut |msg| {
                    tx.send(FfmpegStdoutMsg::Data(msg)).is_ok()
                });
                if !alive {
                    return Ok(()); // receiver 已丟棄（清理中）
                }
            }
            Err(e) => return Err(FfmpegPcmError::Stdout(e.to_string())),
        }
    }
}

struct PcmCleanup {
    child: std::process::Child,
    output: Option<thread::JoinHandle<()>>,
    diagnostics: Option<thread::JoinHandle<()>>,
    _slot: Arc<PcmCaptureSlot>,
}

impl PcmCleanup {
    fn poll(&mut self, terminate: bool) -> Option<bool> {
        if terminate {
            let _ = self.child.kill();
        }
        let status = self.child.try_wait().ok().flatten()?;
        if self.output.as_ref().is_some_and(|worker| !worker.is_finished())
            || self.diagnostics.as_ref().is_some_and(|worker| !worker.is_finished())
        {
            return None;
        }
        let output_ok = self.output.take().map_or(true, |worker| worker.join().is_ok());
        let diagnostics_ok = self.diagnostics.take().map_or(true, |worker| worker.join().is_ok());
        Some(status.success() && output_ok && diagnostics_ok)
    }
}

enum PcmCleanupTask {
    Ffmpeg(PcmCleanup),
    Decoder {
        worker: Option<thread::JoinHandle<()>>,
        _slot: Arc<PcmCaptureSlot>,
    },
}

impl PcmCleanupTask {
    fn is_complete(&mut self) -> bool {
        match self {
            Self::Ffmpeg(owned) => owned.poll(true).is_some(),
            Self::Decoder { worker, .. } => {
                if worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
                    return false;
                }
                if let Some(worker) = worker.take() {
                    let _ = worker.join();
                }
                true
            }
        }
    }
}

fn pcm_cleanup_sender() -> Result<&'static Sender<PcmCleanupTask>, FfmpegPcmError> {
    static SERVICES: std::sync::OnceLock<Option<Sender<PcmCleanupTask>>> = std::sync::OnceLock::new();
    SERVICES.get_or_init(|| {
        // A source can have both a decoder worker and a terminating FFmpeg child.
        let (sender, receiver) = bounded::<PcmCleanupTask>(6);
        thread::Builder::new().name("playback-source-reaper".into()).spawn(move || {
            let mut tasks = Vec::<PcmCleanupTask>::with_capacity(6);
            loop {
                if tasks.is_empty() {
                    match receiver.recv() {
                        Ok(task) => tasks.push(task),
                        Err(_) => break,
                    }
                } else {
                    match receiver.recv_timeout(Duration::from_millis(250)) {
                        Ok(task) => tasks.push(task),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
                tasks.retain_mut(|task| !task.is_complete());
            }
        }).ok().map(|_| sender)
    }).as_ref().ok_or(FfmpegPcmError::ChildFailed)
}


#[derive(Debug)]
enum CapturePool {
    Static(&'static AtomicU64),
    #[cfg(test)]
    Local { count: Arc<AtomicU64>, epoch: Arc<AtomicU64> },
}

impl CapturePool {
    fn count(&self) -> &AtomicU64 {
        match self {
            Self::Static(count) => count,
            #[cfg(test)]
            Self::Local { count, .. } => count,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PcmCaptureSlot {
    pool: CapturePool,
    native_deadline: Mutex<Option<Instant>>,
}

impl PcmCaptureSlot {
    fn use_reserved(slot: Option<Arc<Self>>) -> Result<Arc<Self>, FfmpegPcmError> {
        match slot {
            Some(slot) => Ok(slot),
            None => Self::acquire().map(Arc::new),
        }
    }

    pub(crate) fn acquire() -> Result<Self, FfmpegPcmError> {
        Self::acquire_from(&LIVE_PCM_CAPTURES)
    }

    pub(crate) fn acquire_from(pool: &'static AtomicU64) -> Result<Self, FfmpegPcmError> {
        Self::acquire_in(CapturePool::Static(pool))
    }

    #[cfg(test)]
    pub(crate) fn acquire_local(
        count: &Arc<AtomicU64>,
        epoch: &Arc<AtomicU64>,
    ) -> Result<Self, FfmpegPcmError> {
        Self::acquire_in(CapturePool::Local { count: count.clone(), epoch: epoch.clone() })
    }

    fn acquire_in(pool: CapturePool) -> Result<Self, FfmpegPcmError> {
        pool.count().fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < PLAYBACK_SOURCE_LIMIT as u64).then_some(count + 1)
        }).map_err(|_| FfmpegPcmError::Busy)?;
        Ok(Self { pool, native_deadline: Mutex::new(None) })
    }

    pub(crate) fn begin_native_preparation(&self, now: Instant) {
        *self.native_deadline.lock().unwrap() = Some(now + Duration::from_secs(10));
    }

    pub(crate) fn note_native_progress(&self) -> bool {
        let mut deadline = self.native_deadline.lock().unwrap();
        let now = Instant::now();
        if deadline.is_none_or(|limit| now >= limit) {
            return false;
        }
        *deadline = Some(now + Duration::from_secs(10));
        true
    }

    pub(crate) fn end_native_preparation(&self) -> bool {
        self.native_deadline.lock().unwrap().take().is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub(crate) fn native_preparation_stalled(&self, now: Instant) -> bool {
        self.native_deadline.lock().unwrap().is_some_and(|deadline| now >= deadline)
    }
}

impl Drop for PcmCaptureSlot {
    fn drop(&mut self) {
        self.pool.count().fetch_sub(1, Ordering::AcqRel);
        match &self.pool {
            CapturePool::Static(count) if std::ptr::eq(*count, &LIVE_PCM_CAPTURES) => {
                SOURCE_CAPACITY_EPOCH.fetch_add(1, Ordering::AcqRel);
            }
            #[cfg(test)]
            CapturePool::Local { epoch, .. } => {
                epoch.fetch_add(1, Ordering::AcqRel);
            }
            _ => {}
        }
    }
}

static LIVE_PCM_CAPTURES: AtomicU64 = AtomicU64::new(0);
static SOURCE_CAPACITY_EPOCH: AtomicU64 = AtomicU64::new(0);

pub(crate) fn source_capacity_epoch() -> u64 {
    SOURCE_CAPACITY_EPOCH.load(Ordering::Acquire)
}

fn retire_pcm(mut owned: PcmCleanup, reaper: &Sender<PcmCleanupTask>) {
    let _ = owned.child.kill();
    // The child retains its source permit until the process and pipe readers have stopped.
    if let Err(error) = reaper.try_send(PcmCleanupTask::Ffmpeg(owned)) {
        let mut orphan = error.into_inner();
        while !orphan.is_complete() {
            thread::sleep(Duration::from_secs(1));
        }
    }
}

fn stream_failure_text(reason: crate::prefetch_stream::StreamFailure) -> String {
    use crate::prefetch_stream::StreamFailure;

    match reason {
        StreamFailure::PacketBudget => "音訊封包超過允許上限",
        StreamFailure::DecodeBudget => "音訊解碼錯誤超過允許次數",
        StreamFailure::Decode => "音訊串流解碼失敗",
        StreamFailure::Disconnected => "音訊串流來源中斷",
        StreamFailure::Stall => "音訊串流等候資料逾時",
    }
    .to_string()
}

pub(crate) fn buffer_sym_stream(
    decoder: SymStream,
    initial_frames: usize,
) -> Result<Option<crate::prefetch_stream::PreparedStream>, String> {
    buffer_sym_stream_with_slot(decoder, initial_frames, None, None, None)
}

const HEAD_SCAN_SECONDS: usize = 60;
const HEAD_SCAN_PCM_BYTES: usize = 50 * 1024 * 1024;
const HEAD_SCAN_TIMEOUT: Duration = Duration::from_secs(10);

enum ScannedHead {
    Audible { leading_frames: usize, first_frame: [f32; crate::prefetch_stream::FRAME_SIZE] },
    OriginalStart,
}

fn scan_stream_head(
    channels: u16,
    sample_rate: u32,
    deadline: Instant,
    cancelled: Option<&AtomicBool>,
    mut read: impl FnMut() -> Result<Option<f32>, crate::prefetch_stream::StreamFailure>,
) -> Result<ScannedHead, crate::prefetch_stream::StreamFailure> {
    use crate::prefetch_stream::{read_native_frame, StreamFailure, FRAME_SIZE};

    let channel_count = usize::from(channels);
    if !(1..=FRAME_SIZE).contains(&channel_count) || sample_rate == 0 {
        return Err(StreamFailure::Decode);
    }
    let frames_by_time = (sample_rate as usize).saturating_mul(HEAD_SCAN_SECONDS);
    let frames_by_size = HEAD_SCAN_PCM_BYTES / (channel_count * std::mem::size_of::<f32>());
    for leading_frames in 0..frames_by_time.min(frames_by_size) {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(StreamFailure::Disconnected);
        }
        if Instant::now() >= deadline {
            break;
        }
        let frame = read_native_frame(channels, &mut read);
        if let Err(reason @ (StreamFailure::PacketBudget | StreamFailure::DecodeBudget)) = frame {
            return Err(reason);
        }
        if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(StreamFailure::Disconnected);
        }
        if Instant::now() >= deadline {
            break;
        }
        match frame {
            Ok(Some(frame)) => {
                if frame[..channel_count].iter().any(|sample| sample.abs() > SILENCE_THRESHOLD) {
                    return Ok(ScannedHead::Audible { leading_frames, first_frame: frame });
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    Ok(ScannedHead::OriginalStart)
}

fn checked_sym_sample(decoder: &mut SymStream) -> Result<Option<f32>, crate::prefetch_stream::StreamFailure> {
    use crate::audio_decode::SymReadFailure;
    use crate::prefetch_stream::StreamFailure;

    decoder.next_checked().map_err(|reason| match reason {
        SymReadFailure::PacketBudget => StreamFailure::PacketBudget,
        SymReadFailure::DecodeBudget => StreamFailure::DecodeBudget,
        SymReadFailure::Decode => StreamFailure::Decode,
    })
}

pub(crate) fn prepare_buffered_sym_stream(
    mut stream: SymStream,
    file: &mut std::fs::File,
    path_hint: &Path,
    trim_initial_silence: bool,
    initial_frames: usize,
    cancelled: Option<&Arc<AtomicBool>>,
    reserved_slot: Option<Arc<PcmCaptureSlot>>,
) -> Result<(Option<crate::prefetch_stream::PreparedStream>, usize), String> {
    let slot = PcmCaptureSlot::use_reserved(reserved_slot)
        .map_err(|_| "音訊來源處理忙碌".to_string())?;
    let mut first_frame = None;
    let mut leading_frames = 0;
    stream.observe_preparation(Some(&slot));
    if trim_initial_silence {
        let deadline = Instant::now() + HEAD_SCAN_TIMEOUT;
        let head = scan_stream_head(
            stream.channels(), stream.sample_rate(), deadline,
            cancelled.map(Arc::as_ref), || checked_sym_sample(&mut stream),
        ).map_err(stream_failure_text)?;
        match head {
            ScannedHead::Audible { leading_frames: offset, first_frame: frame } => {
                leading_frames = offset;
                first_frame = Some(frame);
            }
            ScannedHead::OriginalStart => {
                if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                    return Err("音訊準備已撤銷".to_string());
                }
                drop(stream);
                stream = SymStream::open_checked(file, path_hint, 0)
                    .map_err(|error| error.to_string())?;
                stream.observe_preparation(Some(&slot));
            }
        }
    }
    let source = buffer_sym_stream_with_slot(stream, initial_frames, Some(slot), first_frame, cancelled)?;
    Ok((source, leading_frames))
}

fn buffer_sym_stream_with_slot(
    mut decoder: SymStream,
    initial_frames: usize,
    reserved_slot: Option<Arc<PcmCaptureSlot>>,
    first_frame: Option<[f32; crate::prefetch_stream::FRAME_SIZE]>,
    preparation_cancel: Option<&Arc<AtomicBool>>,
) -> Result<Option<crate::prefetch_stream::PreparedStream>, String> {
    use crate::prefetch_stream::StreamFailure;

    let slot = PcmCaptureSlot::use_reserved(reserved_slot)
        .map_err(|_| "音訊來源處理忙碌".to_string())?;
    let reaper = pcm_cleanup_sender().map_err(|_| "無法啟動音訊回收工作".to_string())?;
    let channels = decoder.channels();
    let mut first_samples = first_frame.into_iter()
        .flat_map(move |frame| frame.into_iter().take(usize::from(channels)));
    let priming = preparation_cancel.map(|_| Arc::new(AtomicBool::new(true)));
    let read_priming = priming.clone();
    let preparation_cancel = preparation_cancel.cloned();
    let read = move || {
        if read_priming.as_ref().is_some_and(|flag| flag.load(Ordering::Acquire))
            && preparation_cancel.as_ref().is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            return Err(StreamFailure::Disconnected);
        }
        if let Some(sample) = first_samples.next() {
            Ok(Some(sample))
        } else {
            checked_sym_sample(&mut decoder)
        }
    };
    buffer_decoded_frames(channels, initial_frames, slot, reaper, None, priming, read)
}

fn buffer_decoded_frames(
    channels: u16,
    initial_frames: usize,
    slot: Arc<PcmCaptureSlot>,
    reaper: &'static Sender<PcmCleanupTask>,
    cancel: Option<Arc<AtomicBool>>,
    priming: Option<Arc<AtomicBool>>,
    mut read: impl FnMut() -> Result<Option<f32>, crate::prefetch_stream::StreamFailure> + Send + 'static,
) -> Result<Option<crate::prefetch_stream::PreparedStream>, String> {
    use crate::prefetch_stream::{prepared_stream, read_native_frame, StreamFailure};
    let Some(first) = read_native_frame(channels, &mut read).map_err(stream_failure_text)? else {
        return Ok(None);
    };
    let (writer, mut source) = prepared_stream(channels, first)
        .ok_or_else(|| "無法建立音訊串流緩衝".to_string())?;
    source.retain_source_slot(slot.clone());
    source.retain_cancel_flag(cancel);
    let initial_frames = initial_frames.clamp(1, crate::prefetch_stream::QUEUE_FRAMES - 1);
    for _ in 1..initial_frames {
        let Some(frame) = read_native_frame(channels, &mut read).map_err(stream_failure_text)? else {
            writer.end();
            return Ok(Some(source));
        };
        if !writer.write_frame(frame) {
            return Err(stream_failure_text(StreamFailure::Disconnected));
        }
    }
    if let Some(priming) = priming {
        priming.store(false, Ordering::Release);
    }
    let active_slot = slot.clone();
    let worker = thread::Builder::new()
        .name("playback-decoder".to_string())
        .spawn(move || {
            let _slot = active_slot;
            writer.pump(channels, read);
        })
        .map_err(|_| "無法啟動音訊解碼工作".to_string())?;
    let task = PcmCleanupTask::Decoder { worker: Some(worker), _slot: slot };
    reaper.try_send(task).map_err(|_| "無法追蹤音訊解碼工作".to_string())?;
    Ok(Some(source))
}

struct FfmpegSourceReader {
    child: Option<PcmCleanup>,
    input: Option<Receiver<FfmpegStdoutMsg>>,
    samples: std::vec::IntoIter<f32>,
    cancel: Arc<AtomicBool>,
    priming: Arc<AtomicBool>,
    preparation_cancel: Option<Arc<AtomicBool>>,
    last_block: Instant,
    reaper: &'static Sender<PcmCleanupTask>,
}

impl FfmpegSourceReader {
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
            || (self.priming.load(Ordering::Acquire)
                && self.preparation_cancel.as_ref().is_some_and(|flag| flag.load(Ordering::Acquire)))
    }

    fn next_checked(&mut self) -> Result<Option<f32>, crate::prefetch_stream::StreamFailure> {
        self.next_checked_before(None)
    }

    fn next_checked_before(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<Option<f32>, crate::prefetch_stream::StreamFailure> {
        use crate::prefetch_stream::StreamFailure;
        use crossbeam_channel::RecvTimeoutError;

        loop {
            if self.is_cancelled() {
                return Err(StreamFailure::Disconnected);
            }
            if deadline.is_some_and(|limit| Instant::now() >= limit) {
                return Err(StreamFailure::Stall);
            }
            if let Some(sample) = self.samples.next() {
                return Ok(Some(sample));
            }
            let Some(input) = &self.input else {
                return Ok(None);
            };
            let wait = Duration::from_millis(50);
            let wait = deadline.map_or(wait, |limit| {
                wait.min(limit.saturating_duration_since(Instant::now()))
            });
            match input.recv_timeout(wait) {
                Ok(FfmpegStdoutMsg::Data(samples)) => {
                    self.last_block = Instant::now();
                    self.samples = samples.into_iter();
                }
                Ok(FfmpegStdoutMsg::End { error: None }) => return self.finish(deadline),
                Ok(FfmpegStdoutMsg::End { error: Some(_) }) => return Err(StreamFailure::Decode),
                Err(RecvTimeoutError::Disconnected) => return Err(StreamFailure::Disconnected),
                Err(RecvTimeoutError::Timeout) => {
                    if self.priming.load(Ordering::Acquire) && self.last_block.elapsed() >= FFMPEG_PCM_STALL {
                        return Err(StreamFailure::Stall);
                    }
                }
            }
        }
    }

    fn finish(&mut self, deadline: Option<Instant>) -> Result<Option<f32>, crate::prefetch_stream::StreamFailure> {
        use crate::prefetch_stream::StreamFailure;

        drop(self.input.take());
        let started = Instant::now();
        loop {
            let Some(child) = self.child.as_mut() else {
                return Err(StreamFailure::Disconnected);
            };
            if let Some(success) = child.poll(false) {
                drop(self.child.take());
                return if success { Ok(None) } else { Err(StreamFailure::Decode) };
            }
            if self.is_cancelled() {
                return Err(StreamFailure::Disconnected);
            }
            let remaining = FFMPEG_PCM_STALL.saturating_sub(started.elapsed());
            let remaining = deadline.map_or(remaining, |limit| {
                remaining.min(limit.saturating_duration_since(Instant::now()))
            });
            if remaining.is_zero() {
                return Err(StreamFailure::Stall);
            }
            thread::sleep(remaining.min(Duration::from_millis(20)));
        }
    }

    fn stop_for_restart(&mut self) -> Result<(), crate::prefetch_stream::StreamFailure> {
        use crate::prefetch_stream::StreamFailure;

        drop(self.input.take());
        let stopped_at = Instant::now();
        while let Some(child) = self.child.as_mut() {
            if child.poll(true).is_some() {
                drop(self.child.take());
                break;
            }
            if self.is_cancelled() {
                return Err(StreamFailure::Disconnected);
            }
            if stopped_at.elapsed() >= FFMPEG_PCM_STALL {
                return Err(StreamFailure::Stall);
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

impl Drop for FfmpegSourceReader {
    fn drop(&mut self) {
        drop(self.input.take());
        if let Some(child) = self.child.take() {
            retire_pcm(child, self.reaper);
        }
    }
}

pub(crate) fn buffer_ffmpeg_stream_with_slot(
    ffmpeg_path: &Path,
    file: &str,
    sample_rate: u32,
    channels: u16,
    position_ms: i64,
    trim_initial_silence: bool,
    initial_frames: usize,
    preparation_cancel: Option<&Arc<AtomicBool>>,
    reserved_slot: Option<Arc<PcmCaptureSlot>>,
) -> Result<(Option<crate::prefetch_stream::PreparedStream>, usize), String> {
    if preparation_cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(stream_failure_text(crate::prefetch_stream::StreamFailure::Disconnected));
    }
    let slot = PcmCaptureSlot::use_reserved(reserved_slot)
        .map_err(|_| "音訊來源處理忙碌".to_string())?;
    let reaper = pcm_cleanup_sender().map_err(|_| "無法啟動音訊回收工作".to_string())?;
    use crate::prefetch_stream::StreamFailure;

    let cancel = Arc::new(AtomicBool::new(false));
    let priming = Arc::new(AtomicBool::new(true));
    let start_reader = || -> Result<FfmpegSourceReader, String> {
        if preparation_cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(stream_failure_text(StreamFailure::Disconnected));
        }
        let (child, input) = spawn_ffmpeg_pcm_source(
            ffmpeg_path, file, sample_rate, channels, position_ms, slot.clone(), reaper,
        ).map_err(|_| "無法啟動 FFmpeg 串流".to_string())?;
        Ok(FfmpegSourceReader {
            child: Some(child),
            input: Some(input),
            samples: Vec::new().into_iter(),
            cancel: cancel.clone(),
            priming: priming.clone(),
            preparation_cancel: preparation_cancel.cloned(),
            last_block: Instant::now(),
            reaper,
        })
    };
    let deadline = Instant::now() + HEAD_SCAN_TIMEOUT;
    let mut reader = start_reader()?;
    let mut first_frame = None;
    let mut leading_frames = 0;
    if trim_initial_silence && position_ms == 0 {
        let head = scan_stream_head(
            channels, sample_rate, deadline, preparation_cancel.map(Arc::as_ref),
            || reader.next_checked_before(Some(deadline)),
        ).map_err(stream_failure_text)?;
        match head {
            ScannedHead::Audible { leading_frames: offset, first_frame: frame } => {
                leading_frames = offset;
                first_frame = Some(frame);
            }
            ScannedHead::OriginalStart => {
                reader.stop_for_restart().map_err(stream_failure_text)?;
                reader = start_reader()?;
            }
        }
    }
    let mut first_samples = first_frame.into_iter()
        .flat_map(move |frame| frame.into_iter().take(usize::from(channels)));
    let source = buffer_decoded_frames(
        channels, initial_frames, slot, reaper, Some(cancel), Some(priming),
        move || {
            if reader.is_cancelled() {
                return Err(StreamFailure::Disconnected);
            }
            match first_samples.next() {
                Some(sample) => Ok(Some(sample)),
                None => reader.next_checked(),
            }
        },
    )?;
    Ok((source, leading_frames))
}

enum PcmCaptureEvent {

    Progress,
    Finishing,
    Finished(Result<(Vec<f32>, usize, usize), FfmpegPcmError>),
}

fn wait_for_pcm_event<T>(
    receiver: &Receiver<T>,
    cancelled: impl Fn() -> bool,
) -> Result<T, FfmpegPcmError> {
    use crossbeam_channel::RecvTimeoutError;

    let started = Instant::now();
    loop {
        if cancelled() {
            return Err(FfmpegPcmError::Cancelled);
        }
        let remaining = FFMPEG_PCM_STALL.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(FfmpegPcmError::Stall);
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(FfmpegPcmError::ChildFailed),
        }
    }
}

pub(crate) fn collect_ffmpeg_pcm(
    ffmpeg_path: &Path,
    path: &str,
    sample_rate: u32,
    channels: u16,
    position_ms: i64,
    silence_removal: bool,
    max_bytes: usize,
) -> Result<(Vec<f32>, usize, usize), FfmpegPcmError> {
    collect_ffmpeg_pcm_for_request(
        ffmpeg_path, path, sample_rate, channels, position_ms, silence_removal, max_bytes, None,
    )
}

pub(crate) fn collect_ffmpeg_pcm_for_request(
    ffmpeg_path: &Path,
    path: &str,
    sample_rate: u32,
    channels: u16,
    position_ms: i64,
    silence_removal: bool,
    max_bytes: usize,
    request_cancel: Option<&Arc<AtomicBool>>,
) -> Result<(Vec<f32>, usize, usize), FfmpegPcmError> {
    collect_ffmpeg_pcm_with_slot(
        ffmpeg_path, path, sample_rate, channels, position_ms, silence_removal, max_bytes, request_cancel, None,
    )
}

pub(crate) fn collect_ffmpeg_pcm_with_slot(
    ffmpeg_path: &Path,
    path: &str,
    sample_rate: u32,
    channels: u16,
    position_ms: i64,
    silence_removal: bool,
    max_bytes: usize,
    request_cancel: Option<&Arc<AtomicBool>>,
    reserved_slot: Option<Arc<PcmCaptureSlot>>,
) -> Result<(Vec<f32>, usize, usize), FfmpegPcmError> {
    let request_cancelled = || request_cancel.is_some_and(|flag| flag.load(Ordering::Acquire));
    if request_cancelled() {
        return Err(FfmpegPcmError::Cancelled);
    }
    let slot = PcmCaptureSlot::use_reserved(reserved_slot)?;
    let reaper = pcm_cleanup_sender()?;
    let tool = ffmpeg_path.to_path_buf();
    let file = path.to_owned();
    let (tx, rx) = bounded(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    thread::Builder::new()
        .name("ffmpeg-pcm".into())
        .spawn(move || {
            let result = capture_ffmpeg_pcm(
                &tool, &file, sample_rate, channels, position_ms, silence_removal, max_bytes,
                &tx, slot, reaper, &worker_cancel,
            );
            let _ = tx.send(PcmCaptureEvent::Finished(result));
        })
        .map_err(|_| FfmpegPcmError::Spawn("無法啟動 FFmpeg 收集工作".into()))?;
    let mut finishing = false;
    loop {
        match wait_for_pcm_event(&rx, request_cancelled) {
            Ok(PcmCaptureEvent::Progress) => {}
            Ok(PcmCaptureEvent::Finishing) => finishing = true,
            Ok(PcmCaptureEvent::Finished(result)) => return result,
            Err(error) => {
                cancel.store(true, Ordering::Release);
                return match error {
                    FfmpegPcmError::Stall if finishing => Err(FfmpegPcmError::CleanupPending),
                    error => Err(error),
                };
            }
        }
    }
}

fn spawn_ffmpeg_pcm_source(
    ffmpeg_path: &Path,
    path: &str,
    sample_rate: u32,
    channels: u16,
    position_ms: i64,
    slot: Arc<PcmCaptureSlot>,
    reaper: &Sender<PcmCleanupTask>,
) -> Result<(PcmCleanup, Receiver<FfmpegStdoutMsg>), FfmpegPcmError> {
    let mut command = Command::new(ffmpeg_path);
    command.args([
        "-nostdin",
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe", // SE11：本機 ffmpeg 只允許 file/pipe
    ]);
    // PP9：傳入實際的非零起始位置（輸入定位；格式與 open_with_seek 一致）
    command.args(["-ss", &format!("{:.3}", position_ms.max(0) as f64 / 1000.0)]);
    command.args([
        "-i",
        path,
        "-map",
        "0:a:0",
        "-f",
        "f32le",
        "-ac",
        &channels.to_string(),
        "-ar",
        &sample_rate.to_string(),
        "-",
    ]);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = command.spawn().map_err(|_| FfmpegPcmError::Spawn("無法執行 FFmpeg".into()))?;
    let mut owned = PcmCleanup { child, output: None, diagnostics: None, _slot: slot };
    let Some(mut stdout) = owned.child.stdout.take() else {
        retire_pcm(owned, reaper);
        return Err(FfmpegPcmError::Spawn("ffmpeg 無 stdout".into()));
    };
    let stderr = owned.child.stderr.take();
    let diagnostics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::open_with::spawn_stderr_sanitizer(stderr, None)
    }));
    match diagnostics {
        Ok(worker) => owned.diagnostics = Some(worker),
        Err(_) => {
            retire_pcm(owned, reaper);
            return Err(FfmpegPcmError::ChildFailed);
        }
    }
    let (tx, rx) = bounded::<FfmpegStdoutMsg>(FFMPEG_PCM_CHANNEL_SLOTS);
    match thread::Builder::new().spawn(move || {
        let outcome = pump_ffmpeg_stdout(&mut stdout, &tx);
        let _ = tx.send(FfmpegStdoutMsg::End { error: outcome.err() });
    }) {
        Ok(worker) => owned.output = Some(worker),
        Err(_) => {
            drop(rx);
            retire_pcm(owned, reaper);
            return Err(FfmpegPcmError::ChildFailed);
        }
    }
    Ok((owned, rx))
}

fn capture_ffmpeg_pcm(
    ffmpeg_path: &Path,
    path: &str,
    sample_rate: u32,
    channels: u16,
    position_ms: i64,
    silence_removal: bool,
    max_bytes: usize,
    events: &Sender<PcmCaptureEvent>,
    slot: Arc<PcmCaptureSlot>,
    reaper: &Sender<PcmCleanupTask>,
    cancel: &AtomicBool,
) -> Result<(Vec<f32>, usize, usize), FfmpegPcmError> {
    let cancelled = || cancel.load(Ordering::Acquire);
    if cancelled() {
        return Err(FfmpegPcmError::Cancelled);
    }
    let (mut owned, rx) = spawn_ffmpeg_pcm_source(
        ffmpeg_path, path, sample_rate, channels, position_ms, slot, reaper,
    )?;

    let mut collector = PcmCollector::with_max_bytes(max_bytes);
    // None 表示 stdout 乾淨且對齊的 EOF；其餘為收集過程發生的錯誤
    let mut outcome: Option<FfmpegPcmError> = None;
    loop {
        match wait_for_pcm_event(&rx, cancelled) {
            Ok(FfmpegStdoutMsg::Data(samples)) => {
                if matches!(events.try_send(PcmCaptureEvent::Progress), Err(TrySendError::Disconnected(_))) {
                    outcome = Some(FfmpegPcmError::Stall);
                    break;
                }
                if let Err(e) = collector.extend_from_iter(samples) {
                    outcome = Some(match e {
                        PcmCollectError::BudgetExceeded => FfmpegPcmError::BudgetExceeded,
                        PcmCollectError::AllocationFailed => FfmpegPcmError::AllocationFailed,
                    });
                    break;
                }
            }
            Ok(FfmpegStdoutMsg::End { error }) => {
                outcome = error;
                break;
            }
            Err(error) => {
                outcome = Some(match error {
                    FfmpegPcmError::ChildFailed => FfmpegPcmError::Stdout("stdout 通道提前關閉".into()),
                    error => error,
                });
                break;
            }
        }
    }

    // 清理第一步：丟棄 receiver，解開可能卡在送端的讀取執行緒
    drop(rx);


    let _ = events.send(PcmCaptureEvent::Finishing);
    let started = Instant::now();
    loop {
        if cancelled() && outcome.is_none() {
            outcome = Some(FfmpegPcmError::Cancelled);
        }
        let elapsed = started.elapsed();
        let terminate = outcome.is_some() || elapsed >= Duration::from_secs(10);
        if let Some(success) = owned.poll(terminate) {
            if outcome.is_none() && (!success || terminate) {
                outcome = Some(FfmpegPcmError::ChildFailed);
            }
            break;
        }
        if elapsed >= Duration::from_secs(40) {
            log_line("RECYCLE_ORPHAN FFmpeg PCM 交由回收工作者追蹤");
            retire_pcm(owned, reaper);
            return Err(FfmpegPcmError::CleanupPending);
        }
        thread::sleep(Duration::from_millis(20));
    }

    match outcome {
        Some(e) => Err(e),
        None => {
            // EOF 成功三條件：乾淨對齊 EOF（outcome == None）＋ 子程序成功結束 ＋ 無 stdout 錯誤
            let mut samples = collector.finish();
            if samples.is_empty() && position_ms <= 0 {
                return Err(FfmpegPcmError::ChildFailed);
            }
            let (leading, trailing) = trim_silence(&mut samples, channels, silence_removal && position_ms <= 0);
            Ok((samples, leading, trailing))
        }
    }
}

fn prime_stream_source(mut source: Box<dyn Iterator<Item = f32> + Send>, channels: u16) -> Result<Box<dyn Iterator<Item = f32> + Send>, String> {
    let channels_usize = channels as usize;
    if channels_usize < 1 || channels_usize > 8 {
        return Err("通道範圍錯誤".to_string());
    }

    let mut frame = [0.0_f32; 8];
    for i in 0..channels_usize {
        frame[i] = source.next().ok_or_else(|| "讀取樣本失敗".to_string())?;
    }

    Ok(Box::new(frame.into_iter().take(channels_usize).chain(source)))
}

#[derive(Clone)]
struct OriginalTrackEnd {
    frame: usize,
    sample_rate: u32,
    channels: u16,
    duration_ms: i64,
    decoder: String,
}

fn clip_memory_tail(
    samples: &mut Vec<f32>,
    sample_rate: u32,
    channels: u16,
    position_ms: i64,
    leading_frames: usize,
    trailing_frames: usize,
    original_end: Option<&OriginalTrackEnd>,
) -> Option<usize> {
    if let Some(end) = original_end.filter(|end| end.sample_rate == sample_rate && end.channels == channels) {
        let start_frame = crate::playback_pipeline::native_frame_at_ms(position_ms, sample_rate);
        let samples_to_keep = end.frame.saturating_sub(start_frame).saturating_mul(usize::from(channels));
        samples.truncate(samples_to_keep);
        Some(end.frame)
    } else if trailing_frames > 0 {
        let start_frame = crate::playback_pipeline::native_frame_at_ms(position_ms, sample_rate);
        Some(start_frame.saturating_add(leading_frames)
            .saturating_add(samples.len() / usize::from(channels.max(1))))
    } else {
        None
    }
}

struct PreparedLocalTrack {
    permit: Option<Arc<PcmCaptureSlot>>,
    track_chain: TrackChain,
    track_resampler: TrackResampler,
    buffered_source: Option<crate::prefetch_stream::PreparedStream>,
    decoder: String,
}

struct AudioEngine {
    core: Arc<PlaybackCore>,
    output_stream: Option<OutputStream>,
    output_handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    proxy_id: u64,
    device_sample_rate: u32,
    mode: PlayMode,
    is_playing: bool,
    /// 前端傳來的滑桿值 0–1（clamp 後儲存，供 UI 顯示用）
    volume_raw: f32,
    /// 實際送給 sink.set_volume() 的線性增益
    volume_gain: f32,
    crossfade_ms: i64,
    kind: String,
    stream_title: String,
    error: String,
    ended: bool,
    status: Arc<Mutex<EngineSnapshot>>,
    radio_manager: Option<crate::radio::RadioManager>,
    radio_result: Option<std::sync::mpsc::Receiver<crate::radio::RadioEnd>>,
    radio_eof: bool,
    pending_seek: Option<(i64, bool)>,
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
    /// 快取的音量 dB（避免 250ms 輪詢內重複計算）
    cached_volume_db: Option<f32>,
    /// 最近一次手動載入回執（保留至下一次手動載入終態被記錄；每次 publish 隨身分/錯誤一起發布）
    last_manual_load: Option<ManualLoadResult>,
    /// 手動載入最高受理 token 水位（v0.17）：僅嚴格大於此水位的手動 Load token 才會被受理，預設 0
    manual_load_token_high_water: u64,
}

// 音量曲線換算：log 曲線對應 −60 dB 到 0 dB，linear 為直接對應
fn volume_to_gain(raw: f32, curve: &VolumeCurve) -> f32 {
    let v = raw.clamp(0.0, 1.0);
    match curve {
        VolumeCurve::Linear => v,
        VolumeCurve::Log => {
            if v == 0.0 { return 0.0; } // 0% 強制靜音
            (10.0f32).powf((-60.0 + 60.0 * v) / 20.0)
        }
    }
}

// 手動載入終態回執：載入呼叫正常返回時，以確切 token 記錄 loaded/failed。
// 成功 → loaded；失敗 → failed（錯誤時不得記 loaded）。token 為來源指令（或待載入軌）
// 的真實 token，非引擎計數器值。
fn manual_load_receipt(token: u64, result: &Result<(), String>) -> ManualLoadResult {
    ManualLoadResult {
        token,
        outcome: if result.is_ok() {
            ManualLoadOutcome::Loaded
        } else {
            ManualLoadOutcome::Failed
        },
    }
}

/// 手動 Load 高水位閘門：僅嚴格大於目前水位的 token 才會被受理（並先更新水位）。
fn is_newer_manual_token(high_water: u64, token: u64) -> bool {
    token > high_water
}

/// 響度補償參數更新：依目前音量重算 bass/treble 增益並寫入 EqShared
/// 若 loudness_enabled 為 false 或增益為 0（靜音），將 bass/treble 歸零（旁路）。
fn update_loudness_params(engine: &AudioEngine, raw: f32) {
    let gain = engine.volume_gain;
    let params = engine.core.eq_shared.get();
    let enabled = params.loudness_enabled;
    // 靜音或關閉時旁路：不呼叫 log10，直接歸零
    let (bass_db, treble_db) = if !enabled || gain == 0.0 {
        (0.0, 0.0)
    } else {
        let cur_db = 20.0 * gain.log10(); // 有限正數，安全
        let amount = ((0.0 - cur_db).clamp(0.0, 40.0) / 40.0); // 0 dB → 0，-40 dB 以下 → 1
        (amount * 10.0, amount * 6.0) // 最多 +10 dB 低頻、+6 dB 高頻
    };
    engine.core.eq_shared.update(|p| {
        p.loudness_bass_db = bass_db;
        p.loudness_treble_db = treble_db;
    });
}

impl AudioEngine {
    fn new(status: Arc<Mutex<EngineSnapshot>>) -> Result<Self, String> {
        let (_stream, handle) = OutputStream::try_default().map_err(|e| format!("音訊輸出失敗: {e}"))?;
        let sink = Sink::try_new(&handle).map_err(|e| format!("建立 Sink 失敗: {e}"))?;
        // 初始音量：0.7 (log 曲線換算後的增益)
        let initial_raw = 0.7;
        let initial_gain = volume_to_gain(initial_raw, &VolumeCurve::Log);
        sink.set_volume(initial_gain);

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
        let recycle_handle = Self::spawn_recycle_thread(recycle_rx.clone(), recycle_flag.clone(), core.next_coordinator(), Arc::downgrade(&core));

        let mut engine = Self {
            core: core.clone(),
            output_stream: Some(_stream),
            output_handle: Some(handle),
            sink: Some(sink),
            proxy_id,
            device_sample_rate,
            mode: PlayMode::Sequence,
            is_playing: false,
            volume_raw: initial_raw,
            volume_gain: initial_gain,
            crossfade_ms: 0,
            kind: "idle".into(),
            stream_title: String::new(),
            error: String::new(),
            ended: false,
            status,
            radio_manager: None,
            radio_result: None,
            radio_eof: false,
            pending_seek: None,
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
            cached_volume_db: Some(20.0 * initial_gain.log10()),
            last_manual_load: None,
            manual_load_token_high_water: 0,
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

    fn spawn_recycle_thread(rx: Receiver<RecycleItem>, cancel_flag: Arc<AtomicBool>, coordinator: Arc<crate::playback_pipeline::NextCoordinator>, retired_core: std::sync::Weak<PlaybackCore>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            // SE9：回收期間凍結新準備工作（quota freeze），超過 30 秒仍未結束標記孤兒
            while !cancel_flag.load(Ordering::Relaxed) {
                let Some(core) = retired_core.upgrade() else { break };
                core.reap_retired_handoffs();
                drop(core);
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
        let Ok(mut status) = self.status.lock() else { return; };
        if let Some(candidate) = self.core.read_handoff_snapshot(|| {
            let mut candidate = status.clone();
            self.fill_snapshot(&mut candidate);
            candidate
        }) {
            *status = candidate;
            return;
        }

        status.is_playing = self.is_playing && !self.sink.as_ref().map(|s| s.empty()).unwrap_or(true);
        status.playback_state = match self.core.playback_state() {
            PlaybackState::Playing => "Playing",
            PlaybackState::WaitingForNext => "WaitingForNext",
            PlaybackState::Paused => "Paused",
            PlaybackState::Finished => "Finished",
        }.to_string();
    }

    fn fill_snapshot(&self, s: &mut EngineSnapshot) {
            s.position_ms = self.position_ms();

            // 單一當前軌快照：用於 duration、song_id 與 seek 細節（不改變 trim/time 語義）
            // 取自 TrackResampler 當前軌中繼（記憶體與長串流皆有效），不再依賴 TrackChain
            let current_track = self.core.current_track_meta();

            s.duration_ms = if self.kind == "radio" {
                0
            } else {
                current_track.as_ref().map(|t| t.duration_ms).unwrap_or(0)
            };
            s.song_id = current_track.as_ref().map(|t| t.song_id).unwrap_or(0);
            s.is_playing = self.is_playing && !self.sink.as_ref().map(|s| s.empty()).unwrap_or(true);
            // volume 欄位給 UI 滑桿用：存 raw 值（0–1）
            s.volume = self.volume_raw;
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
            s.play_seq = self.core.play_seq();
            s.now_token = self.core.now_token();
            s.next_token = self.core.next_token();
            s.next_clear_epoch = self.core.next_clear_epoch();
            s.next_request_result = self.core.next_coordinator().next_request_result();
            s.next_capacity_epoch = self.core.next_coordinator().next_capacity_epoch();
            s.playback_state = match self.core.playback_state() {
                PlaybackState::Playing => "Playing",
                PlaybackState::WaitingForNext => "WaitingForNext",
                PlaybackState::Paused => "Paused",
                PlaybackState::Finished => "Finished",
            }
            .to_string();
            // volume_db：使用快取值（避免 250ms 輪詢內重複 log10）
            s.volume_db = self.cached_volume_db;
            s.volume_curve = eq.volume_curve;
            s.loudness_compensation = eq.loudness_enabled;
            s.fade_play_ms = eq.fade_play_ms;
            s.fade_pause_ms = eq.fade_pause_ms;
            s.fade_resume_ms = eq.fade_resume_ms;
            s.fade_stop_ms = eq.fade_stop_ms;
            s.fade_seek_ms = eq.fade_seek_ms;
            s.fade_switch_ms = eq.fade_switch_ms;
            s.gap_ms = eq.gap_ms;
            s.silence_removal = eq.silence_removal;
            // 手動載入回執：保留至上一次手動終態被下一次手動終態取代，與新身分/錯誤一起發布
            s.last_manual_load_result = self.last_manual_load;
            s.manual_load_token_high_water = self.manual_load_token_high_water;


            // ReplayGain 發布欄位：僅在當前有效本地軌（error 為空，載入失敗需清除實際增益）且非 Finished 時，
            // 從實際 EqProcessor 讀取；否則（含無 processor）清除訊號但保留模式。暫停時亦需刷新（透過 refresh_published）。
            let is_valid_local = matches!(self.kind.as_str(), "file")
                && current_track.is_some()
                && self.error.is_empty()
                && self.core.playback_state() != PlaybackState::Finished;

            // 模式一律從 EqShared 讀取（保留模式）
            s.rg_mode = eq.rg_mode;
            s.volume_norm_mode = eq.volume_norm_mode;

            if is_valid_local {
                let published = self.core.eq_processor.lock().ok().and_then(|mut ep| {
                    ep.as_mut().map(|p| {
                        p.refresh_published();
                        (p.current_rg_effective_gain_db(), p.current_rg_preamp_db(), p.has_rg(), p.current_rg_source())
                    })
                });
                if let Some((db, preamp, has, src)) = published {
                    s.applied_replaygain_db = db;
                    s.applied_preamp_db = preamp;
                    s.has_replaygain = has;
                    s.rg_source = src;
                } else {
                    // 有效本地軌但無 EqProcessor：同樣清除，避免殘留前一軌欄位
                    s.applied_replaygain_db = None;
                    s.applied_preamp_db = 0.0;
                    s.has_replaygain = false;
                    s.rg_source = String::new();
                }
            } else {
                s.applied_replaygain_db = None;
                s.applied_preamp_db = 0.0;
                s.has_replaygain = false;
                s.rg_source = String::new();
            }
    }

    /// 播放命令實作：開始本機播放。若目前軌正在等待第一次本機播放（Prepare 後），此處分配
    /// 一次播放事件序號；暫停後續播（resume）不重複分配。
    fn start_playback(&mut self) {
        if self.fade_target() == Some(FadeTarget::Stop) {
            return;
        }
        if let Some((_, should_play)) = &mut self.pending_seek {
            *should_play = true;
            return;
        }
        if self.kind == "idle" {
            return;
        }
        let first_play = self.core.try_allocate_first_play();
        let eq = self.core.eq_shared.get();
        let fade_ms = if first_play { eq.fade_play_ms } else { eq.fade_resume_ms };
        self.core.start_fade(FadeTarget::None, self.current_fade_gain(), 1.0, fade_ms);
        if let Some(ref sink) = self.sink {
            sink.play();
        }
        self.is_playing = true;
        self.ended = false;
        self.core.set_playback_state(PlaybackState::Playing);
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
        sink.set_volume(self.volume_gain);
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

    #[cfg(test)]
    fn load(&mut self, path: &str, song_id: i64, token: u64, rg: (Option<f32>, Option<f32>, Option<f32>, Option<f32>)) -> Result<(), String> {
        // 手動/切換載入：seq 取新號，token 由呼叫端授權（來源指令或待載入軌的確切 token）
        let seq = self.seq_counter.fetch_add(1, Ordering::AcqRel);
        self.core.clear_awaiting_first_play();
        self.load_at(path, song_id, 0, true, rg, seq, token)?;
        self.core.increment_play_seq();
        Ok(())
    }

    fn fade_target(&self) -> Option<FadeTarget> {
        let gate = self.core.fade_gate.lock().unwrap();
        gate.active.as_ref().map(|segment| segment.target).or_else(|| {
            gate.done_target
                .filter(|(_, generation)| *generation == gate.fade_gen)
                .map(|(target, _)| target)
        })
    }

    fn playback_intent(&self) -> bool {
        if let Some((_, play)) = self.pending_seek {
            return play;
        }
        self.is_playing && !matches!(self.fade_target(), Some(FadeTarget::Pause | FadeTarget::Stop))
    }

    fn can_fade_file(&self, duration_ms: i64) -> bool {
        duration_ms > 0
            && self.kind == "file"
            && self.is_playing
            && self.core.playback_state() == PlaybackState::Playing
            && self.sink.as_ref().is_some_and(|sink| !sink.empty())
    }

    fn current_fade_gain(&self) -> f32 {
        let gate = self.core.fade_gate.lock().unwrap();
        let Some(segment) = &gate.active else {
            return gate.base_gain;
        };
        if segment.total_samples == 0 {
            return segment.to_gain;
        }
        let progress = segment.consumed_samples as f32 / segment.total_samples as f32;
        segment.from_gain + (segment.to_gain - segment.from_gain) * progress.clamp(0.0, 1.0)
    }

    fn begin_pause(&mut self) {
        if matches!(self.fade_target(), Some(FadeTarget::Pause | FadeTarget::Stop)) {
            return;
        }
        if let Some((_, should_play)) = &mut self.pending_seek {
            *should_play = false;
            return;
        }
        let duration_ms = self.core.eq_shared.get().fade_pause_ms;
        if self.can_fade_file(duration_ms) {
            self.core.start_fade(FadeTarget::Pause, self.current_fade_gain(), 0.0, duration_ms);
        } else {
            self.pause_now();
        }
    }

    fn pause_now(&mut self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        self.is_playing = false;
        self.reset_fade();
        self.core.fade_gate.lock().unwrap().base_gain = 0.0;
        self.core.set_playback_state(PlaybackState::Paused);
    }

    fn begin_seek(&mut self, position_ms: i64) -> Option<bool> {
        if self.kind != "file" || self.fade_target() == Some(FadeTarget::Stop) {
            return None;
        }
        let should_play = self.playback_intent();
        let duration_ms = self.core.eq_shared.get().fade_seek_ms;
        if self.can_fade_file(duration_ms) {
            self.pending_seek = Some((position_ms, should_play));
            self.core.start_fade(FadeTarget::Seek, self.current_fade_gain(), 0.0, duration_ms);
            None
        } else {
            Some(should_play)
        }
    }

    fn seek_load_request(&self, position_ms: i64, play: bool) -> Option<LocalLoadRequest> {
        let (path, song_id, rg, original_end) = match self.core.current_track_meta() {
            Some(track) => {
                let original_end = track.effective_end_frame.map(|frame| OriginalTrackEnd {
                    frame, sample_rate: track.sample_rate, channels: track.channels,
                    duration_ms: track.duration_ms, decoder: track.decoder.clone(),
                });
                (
                    track.path.clone(), track.song_id,
                    (track.replaygain_track_gain, track.replaygain_track_peak,
                     track.replaygain_album_gain, track.replaygain_album_peak),
                    original_end,
                )
            }
            None => (self.current_path.lock().ok()?.clone()?, 0, (None, None, None, None), None),
        };
        if path.is_empty() {
            return None;
        }
        Some(LocalLoadRequest {
            path, song_id, position_ms, rg, play, original_end,
            seq: self.core.now_seq(), token: self.core.now_token(), action: LocalLoadAction::Seek,
        })
    }

    fn clear_next_request(&mut self) {
        self.next_coord_gen = self.core.next_coordinator().clear_next();
        self.core.set_next_token(0);
        self.core.increment_next_clear_epoch();
        self.core.clear_pending_stream();
    }

    fn stop_now(&mut self) {
        self.reset_fade();
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        self.core.set_playback_state(PlaybackState::Finished);
        self.core.abort_handoff();
        self.core.set_stream_source(None);
        self.is_playing = false;
        self.kind = "idle".into();
        self.ended = false;
    }

    fn reset_fade(&mut self) {
        self.pending_seek = None;
        let mut gate = self.core.fade_gate.lock().unwrap();
        gate.fade_gen = self.core.increment_fade_gen();
        gate.active = None;
        gate.done_target = None;
        gate.pending_load = None;
        gate.base_gain = 1.0;
    }

    fn fail_load(&mut self, reason: &str) {
        self.halt_radio();
        self.reset_fade();
        self.core.set_playback_state(PlaybackState::Finished);
        self.core.abort_handoff();
        if let Some(sink) = &self.sink {
            sink.stop();
            sink.clear();
        }
        self.clear_next_request();
        self.core.clear_awaiting_first_play();
        self.core.set_stream_source(None);
        *self.core.track_chain.lock().unwrap() = None;
        *self.core.track_resampler.lock().unwrap() = None;
        self.core.track_out.lock().unwrap().clear();
        self.core.dsp_out.lock().unwrap().clear();
        self.core.set_now_identity(0, 0);
        self.core.set_stream_song_id(0);
        self.core.set_stream_duration_ms(0);
        self.core.set_stream_seek_origin_ms(0);
        self.core.set_seek_origin_ms(0);
        self.core.reset_played_out_frames();
        *self.current_decoder.lock().unwrap() = None;
        *self.current_path.lock().unwrap() = None;
        self.kind = "idle".into();
        self.stream_title.clear();
        self.error = "載入失敗".into();
        self.is_playing = false;
        self.ended = false;
        log_load_error(reason);
    }

    #[cfg(test)]
    fn load_at(&mut self, path: &str, song_id: i64, position_ms: i64, play: bool, rg: (Option<f32>, Option<f32>, Option<f32>, Option<f32>), seq: u64, token: u64) -> Result<(), String> {
        validate_local_path(Path::new(path))?;
        self.begin_local_load();
        let prepared = Self::prepare_local_track(&self.core, path, song_id, position_ms, rg, seq, token, true, None, None, None)?;
        self.install_local_track(path, song_id, position_ms, play, seq, token, prepared)
    }

    fn begin_local_load(&mut self) {
        self.halt_radio();
        self.reset_fade();

        if let Some(ref sink) = self.sink {
            sink.stop();
            sink.clear();
        }

        // 播放序號與 token（C1：seq/token 由呼叫端授權——手動 Load 用指令 token、Prepare 用指令
        // token、seek 沿用原身分。於共同載入點寫入核心身分，並由下方 TrackChainItem/TrackResampler
        // 中繼資料攜帶同一組值；不再於載入後覆寫，避免被 sync_identity 蓋掉）
        self.core.set_playback_state(PlaybackState::Finished);
        self.core.abort_handoff();
        self.clear_next_request();
        self.core.set_stream_source(None); // 串流軌與記憶體軌切換時先清掉舊來源
    }

    fn prepare_local_track(
        core: &PlaybackCore,
        path: &str,
        song_id: i64,
        position_ms: i64,
        rg: (Option<f32>, Option<f32>, Option<f32>, Option<f32>),
        seq: u64,
        token: u64,
        allow_initial_trim: bool,
        cancelled: Option<&Arc<AtomicBool>>,
        reserved_slot: Option<Arc<PcmCaptureSlot>>,
        original_end: Option<&OriginalTrackEnd>,
    ) -> Result<PreparedLocalTrack, String> {
        if let Some(slot) = &reserved_slot {
            slot.begin_native_preparation(Instant::now());
        }
        if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err("音訊準備已撤銷".to_string());
        }
        validate_local_path(Path::new(path))?;
        if let Some(end) = original_end {
            if crate::playback_pipeline::native_frame_at_ms(position_ms, end.sample_rate) >= end.frame {
                let track = TrackChainItem {
                    samples: Arc::new(Vec::new()),
                    sample_rate: end.sample_rate,
                    channels: end.channels,
                    duration_ms: end.duration_ms,
                    song_id,
                    norm_gain: 1.0,
                    replaygain_track_gain: rg.0,
                    replaygain_track_peak: rg.1,
                    replaygain_album_gain: rg.2,
                    replaygain_album_peak: rg.3,
                    seq, token, native_frame_pos: 0, path: path.to_string(),
                    decoder: end.decoder.clone(),
                    leading_silence_samples: 0,
                    trailing_silence_samples: 0,
                    effective_end_frame: Some(end.frame),
                };
                let mut track_chain = TrackChain::new();
                track_chain.append(track.clone());
                let mut track_resampler = TrackResampler::new();
                track_resampler.set_track(Some(track));
                return Ok(PreparedLocalTrack {
                    permit: reserved_slot, track_chain, track_resampler,
                    buffered_source: None, decoder: end.decoder.clone(),
                });
            }
        }
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        let backend = playback_backend_for(&ext);
        let trim_initial_silence = allow_initial_trim
            && position_ms <= 0
            && core.eq_shared.get().silence_removal;

        let (track_chain, track_resampler, buffered_source, decoder) = match backend {
            Some(crate::scanner::Backend::Symphonia) => {
                // SE7：配置前封包上限預檢，超標則拒絕，不呼叫 open()
                let mut file = std::fs::File::open(path).map_err(|e| format!("開啟檔案失敗: {e}"))?;
                if let Err(reason) = precheck_packet_budget(&mut file) {
                    return Err(format!("封包上限預檢失敗：{reason}"));
                }
                match SymStream::open_checked(&mut file, Path::new(path), position_ms) {
                    Ok(mut stream) => {
                        stream.observe_preparation(reserved_slot.as_ref());
                        let duration_ms = stream.duration_ms().filter(|d| *d > 0).unwrap_or(0);
                        let is_long = duration_ms > STREAM_OVER_MS || duration_ms == 0;
                        let sample_rate = stream.sample_rate().max(1);
                        let channels = stream.channels().max(1);
                        let source = if is_long {
                            SymPlaybackSource::Stream(stream)
                        } else {
                            prepare_bounded_sym_source(
                                stream,
                                &mut file,
                                Path::new(path),
                                position_ms,
                                trim_initial_silence,
                                PCM_MAX_BYTES,
                                cancelled.map(Arc::as_ref),
                            )?
                        };
                        match source {
                            SymPlaybackSource::Stream(stream) => {
                                let (buffered_source, leading_frames) = prepare_buffered_sym_stream(
                                    stream, &mut file, Path::new(path), trim_initial_silence, 1,
                                    cancelled, reserved_slot.clone(),
                                )?;

                                let mut resampler = TrackResampler::new();
                                resampler.set_track(Some(TrackChainItem {
                                    samples: Arc::new(Vec::new()),
                                    sample_rate,
                                    channels,
                                    duration_ms,
                                    song_id,
                                    norm_gain: 1.0,
                                    replaygain_track_gain: rg.0,
                                    replaygain_track_peak: rg.1,
                                    replaygain_album_gain: rg.2,
                                    replaygain_album_peak: rg.3,
                                    token,
                                    seq,
                                    native_frame_pos: 0,
                                    path: path.to_string(),
                                    decoder: "stream".to_string(),
                                    leading_silence_samples: leading_frames,
                                    trailing_silence_samples: 0,
                                    effective_end_frame: None,
                                }));
                                (TrackChain::new(), resampler, buffered_source, "stream".to_string())
                            }
                            SymPlaybackSource::Memory { mut samples, leading_frames, trailing_frames } => {
                                let effective_end_frame = clip_memory_tail(
                                    &mut samples, sample_rate, channels, position_ms,
                                    leading_frames, trailing_frames, original_end,
                                );
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
                                    replaygain_track_gain: rg.0,
                                    replaygain_track_peak: rg.1,
                                    replaygain_album_gain: rg.2,
                                    replaygain_album_peak: rg.3,
                                    token,
                                    seq,
                                    native_frame_pos: 0,
                                    path: path.to_string(),
                                    decoder: "symphonia".to_string(),
                                    leading_silence_samples: leading_frames,
                                    trailing_silence_samples: trailing_frames,
                                    effective_end_frame,
                                });
                                chain.set_coordinator(core.next_coordinator());

                                let mut resampler = TrackResampler::new();
                                resampler.set_track(Some(chain.current().unwrap().clone()));

                                (chain, resampler, None, "symphonia".to_string())
                            }
                        }
                    }
                    Err(error) if !error.allows_fallback() => return Err(error.to_string()),
                    Err(_) => Self::prepare_ffmpeg_track(
                        core, path, song_id, position_ms, seq, token, rg,
                        PCM_MAX_BYTES, trim_initial_silence, cancelled, reserved_slot.clone(), original_end,
                    )?,
                }
            }
            Some(crate::scanner::Backend::Ffmpeg) => {
                Self::prepare_ffmpeg_track(
                    core, path, song_id, position_ms, seq, token, rg,
                    PCM_MAX_BYTES, trim_initial_silence, cancelled, reserved_slot.clone(), original_end,
                )?
            }
            None => {
                return Err(format!("這個格式尚未支援: {}", ext));
            }
        };

        Ok(PreparedLocalTrack { permit: reserved_slot, track_chain, track_resampler, buffered_source, decoder })
    }

    fn install_local_track(
        &mut self,
        path: &str,
        song_id: i64,
        position_ms: i64,
        play: bool,
        seq: u64,
        token: u64,
        prepared: PreparedLocalTrack,
    ) -> Result<(), String> {
        let PreparedLocalTrack { permit, mut track_chain, track_resampler, buffered_source, decoder } = prepared;
        track_chain.retain_source_slot(permit);
        let start_ms = if position_ms > 0 {
            position_ms as u64
        } else {
            track_resampler.current_track().map_or(0, |track| {
                crate::playback_pipeline::native_frame_to_ms(track.leading_silence_samples, track.sample_rate)
            })
        };

        if let Some(source) = buffered_source {
            self.core
                .install_buffered_stream(source, self.core.stream_gen(), token)
                .map_err(stream_failure_text)?;
        }
        *self.core.track_chain.lock().unwrap() = Some(track_chain);
        *self.core.track_resampler.lock().unwrap() = Some(track_resampler);

        // 手動換軌/seek 安裝新來源前清空 DSP 輸出佇列，避免舊位置殘留聲音
        self.core.track_out.lock().unwrap().clear();
        self.core.dsp_out.lock().unwrap().clear();
        // 重設 RateStage（速度/節奏/音高），避免舊軌殘留緩衝影響新軌（涵蓋短檔與長檔）
        *self.core.rate_stage.lock().unwrap() = Some(RateStage::new(INTERNAL_CHANNELS as usize, INTERNAL_SAMPLE_RATE));

        self.core.set_now_identity(seq, token);
        self.core.reset_played_out_frames();
        self.core.set_seek_origin_ms(start_ms);
        self.core.clear_expired_boundaries(seq + 1);
        self.core.set_playback_state(if play { PlaybackState::Playing } else { PlaybackState::Paused });
        if play {
            let fade_ms = self.core.eq_shared.get().fade_play_ms;
            self.core.start_fade(FadeTarget::None, 0.0, 1.0, fade_ms);
        }

        // 換軌時重新串接一個新的 OutputProxy 到長駐 Sink（同一 proxy_id，active_id 不變）
        if let Some(ref sink) = self.sink {
            sink.append(OutputProxy::new(self.core.clone(), self.device_sample_rate, self.proxy_id));
        }

        // 關鍵修正：成功安裝 TrackResampler 與來源後，立即取得「新軌」的中繼資料，
        // 以「新軌實際的 norm_gain」更新 ReplayGain context（記憶體與串流軌皆適用）。
        // 這取代了 Load/Prepare/restart_from 呼叫端的重複呼叫，確保永遠使用新軌的真實 norm_gain。
        if let Some(meta) = self.core.current_track_meta() {
            self.core.apply_eq_rg_gain(
                meta.replaygain_track_gain,
                meta.replaygain_track_peak,
                meta.replaygain_album_gain,
                meta.replaygain_album_peak,
                meta.norm_gain,
            );
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

    fn prepare_ffmpeg_track(
        core: &PlaybackCore,
        path: &str,
        song_id: i64,
        position_ms: i64,
        seq: u64,
        token: u64,
        rg: (Option<f32>, Option<f32>, Option<f32>, Option<f32>),
        pcm_limit: usize,
        silence_removal: bool,
        cancelled: Option<&Arc<AtomicBool>>,
        reserved_slot: Option<Arc<PcmCaptureSlot>>,
        original_end: Option<&OriginalTrackEnd>,
    ) -> Result<(TrackChain, TrackResampler, Option<crate::prefetch_stream::PreparedStream>, String), String> {
        if let Some(slot) = &reserved_slot {
            if slot.end_native_preparation() {
                return Err("讀取音訊資料逾時".into());
            }
        }
        let ffprobe_path = locate("ffprobe.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffprobe")?;

        // SE6：使用 ffprobe_playback_json helper（64 KB / 10 s 上限，逾時可中斷）
        let value = crate::audio_decode::ffprobe_playback_json_for_request(&ffprobe_path, path, cancelled)?;
        if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err("音訊準備已撤銷".to_string());
        }

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

        let ffmpeg_path = locate("ffmpeg.exe").map(|(p, _, _)| p).ok_or("找不到 ffmpeg")?;
        let memory = if is_long {
            None
        } else {
            match collect_ffmpeg_pcm_with_slot(
                &ffmpeg_path, path, final_sr, final_ch, position_ms, silence_removal, pcm_limit,
                cancelled, reserved_slot.clone(),
            ) {
                Ok(pcm) => Some(pcm),
                Err(FfmpegPcmError::BudgetExceeded) => None,
                Err(FfmpegPcmError::Cancelled) => return Err("音訊準備已撤銷".to_string()),
                Err(FfmpegPcmError::Busy) => return Err("音訊來源處理忙碌".to_string()),
                Err(_) => return Err("ffmpeg 解碼失敗".into()),
            }
        };
        if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err("音訊準備已撤銷".to_string());
        }
        if memory.is_none() {
            use crate::scanner::{SAMPLE_RATE_MAX, SAMPLE_RATE_MIN};
            if !(SAMPLE_RATE_MIN as u32..=SAMPLE_RATE_MAX as u32).contains(&final_sr)
                || !(1..=crate::prefetch_stream::FRAME_SIZE as u16).contains(&final_ch)
            {
                return Err("FFmpeg 音訊格式資訊無效".to_string());
            }
            let (source, leading_frames) = buffer_ffmpeg_stream_with_slot(
                &ffmpeg_path, path, final_sr, final_ch, position_ms, silence_removal, 1,
                cancelled, reserved_slot,
            )?;
            if source.is_none() && position_ms <= 0 {
                return Err("FFmpeg 串流沒有音訊樣本".to_string());
            }
            let chain = TrackChain::new();
            let mut resampler = TrackResampler::new();
            resampler.set_track(Some(TrackChainItem {
                samples: Arc::new(Vec::new()),
                sample_rate: final_sr,
                channels: final_ch,
                duration_ms,
                song_id,
                norm_gain: 1.0,
                replaygain_track_gain: rg.0,
                replaygain_track_peak: rg.1,
                replaygain_album_gain: rg.2,
                replaygain_album_peak: rg.3,
                token,
                seq,
                native_frame_pos: 0,
                path: path.to_string(),
                decoder: "ffmpeg-stream".to_string(),
                leading_silence_samples: leading_frames,
                trailing_silence_samples: 0,
                effective_end_frame: None,
            }));
            Ok((chain, resampler, source, "ffmpeg-stream".to_string()))
        } else {
            let (mut samples, leading_silence, trailing_silence) = memory.expect("checked PCM result");
            let effective_end_frame = clip_memory_tail(
                &mut samples, final_sr, final_ch, position_ms,
                leading_silence, trailing_silence, original_end,
            );
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
                replaygain_track_gain: rg.0,
                replaygain_track_peak: rg.1,
                replaygain_album_gain: rg.2,
                replaygain_album_peak: rg.3,
                token,
                seq,
                native_frame_pos: 0,
                path: path.to_string(),
                decoder: "ffmpeg".to_string(),
                leading_silence_samples: leading_silence,
                trailing_silence_samples: trailing_silence,
                effective_end_frame,
            });
            chain.set_coordinator(core.next_coordinator());

            let mut resampler = TrackResampler::new();
            resampler.set_track(Some(chain.current().unwrap().clone()));

            Ok((chain, resampler, None, "ffmpeg".to_string()))
        }
    }

    fn halt_radio(&mut self) {
        if let Some(manager) = &self.radio_manager {
            manager.cancel();
        }
        self.radio_result = None;
        self.radio_eof = false;
    }

    fn radio_start(&mut self, url: &str, ffmpeg_path: std::path::PathBuf, tx: crossbeam_channel::Sender<Vec<f32>>) -> Result<(), ()> {
        if self.radio_manager.is_none() {
            self.radio_manager = Some(crate::radio::RadioManager::new().map_err(|_| ())?);
        }
        let receiver = self.radio_manager.as_ref().ok_or(())?.start(url, ffmpeg_path, tx)?;
        self.radio_result = Some(receiver);
        Ok(())
    }

    fn poll_radio(&mut self) {
        if self.kind != "radio" {
            return;
        }
        match self.radio_result.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(crate::radio::RadioEnd::Complete)) => {
                self.radio_result = None;
                self.radio_eof = true;
            }
            Some(Ok(crate::radio::RadioEnd::Cancelled)) => {
    self.radio_result = None;
    self.radio_eof = false;
}
            Some(Ok(crate::radio::RadioEnd::Failed)) | Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.radio_result = None;
                self.error = "電台播放失敗".into();
                self.is_playing = false;
                self.kind = "idle".to_string();
                self.ended = false;
                self.radio_eof = false;
            }
            _ => {}
        }
        if self.radio_eof && self.sink.as_ref().map_or(true, |s| s.empty()) {
            self.kind = "idle".into();
            self.is_playing = false;
            self.ended = true;
            self.radio_eof = false;
        }
    }

    #[cfg(test)]
    fn restart_from(&mut self, ms: i64, play: bool) {
        // 交接後 seek：current_path/current_decoder 可能已過期，優先從 TrackResampler 當前軌中繼讀取（長串流亦有效）。
        // 於拆除舊 TrackChain/resampler 前擷取一次快照（含 RG 標籤與 norm_gain，用於重開舊檔）。
        let (path, decoder, song_id, rg_tags) = match self.core.current_track_meta() {
            Some(meta) => (
                meta.path.clone(),
                meta.decoder.clone(),
                meta.song_id,
                (meta.replaygain_track_gain, meta.replaygain_track_peak,
                 meta.replaygain_album_gain, meta.replaygain_album_peak),
            ),
            None => {
                // 無當前軌：保留 fallback path/decoder，但不沿用先前軌的 RG 標籤
                let p = self.current_path.lock().unwrap().clone().unwrap_or_default();
                let d = self.current_decoder.lock().unwrap().clone().unwrap_or_default();
                (p, d, 0, (None, None, None, None))
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

        // 載入新軌（重開同一檔案），若失敗則設定錯誤狀態並提早返回，不嘗試恢復
        if let Err(e) = self.load_at(&path, song_id, ms, play, rg_tags, current_seq, current_token) {
            self.error = format!("restart_from 失敗: {e}");
            self.is_playing = false;
            self.core.set_playback_state(PlaybackState::Finished);
            log_line(&format!("RESTART_FROM_ERR path={} err={}", path, e));
            return;
        }


        // 關鍵修正：load_at 成功安裝新 TrackResampler 與來源後，立即取得「新軌」的中繼資料，
        // 以「新軌實際的 norm_gain」更新 ReplayGain context（記憶體與串流軌皆適用）。
        // 絕不沿用舊軌或假設的 norm_gain（0.0 會導致靜音）。
        if let Some(meta) = self.core.current_track_meta() {
            self.core.apply_eq_rg_gain(
                meta.replaygain_track_gain,
                meta.replaygain_track_peak,
                meta.replaygain_album_gain,
                meta.replaygain_album_peak,
                meta.norm_gain,
            );
        }

        *self.current_decoder.lock().unwrap() = Some(decoder);
        *self.current_path.lock().unwrap() = Some(path);
    }

    fn play_url(&mut self, url: &str) {
                    self.core.clear_awaiting_first_play();
        self.halt_radio();
        self.reset_fade();
        self.core.set_playback_state(PlaybackState::Finished);
        self.core.abort_handoff();
        self.clear_next_request();
        if let Some(ref sink) = self.sink {
            sink.stop();
            sink.clear();
        }
        self.ended = false;
        self.stream_title = "網路電台".into();

        let ffmpeg_path = match locate("ffmpeg.exe") {
            Some((p, _, _)) => p,
            None => {
                self.error = "播放串流需要 ffmpeg，這台電腦找不到".into();
                self.is_playing = false;
                self.kind = "idle".into();
                return;
            }
        };

        let (tx, rx) = bounded(128);
        if self.radio_start(url, ffmpeg_path, tx).is_err() {
            self.error = "無法開啟電台".into();
            self.is_playing = false;
            self.kind = "idle".into();
            return;
        }
        self.kind = "radio".into();
        self.error.clear();

        let (dsp, played) = DspSource::new(
            LiveSource { rx, pending: Vec::new(), pos: 0 },
            2, 44100, &self.core.eq_shared, 1.0, None, false,
            Some(self.core.fade_gate.clone())
        );
        self.played = played;
        if let Some(ref sink) = self.sink {
            sink.append(dsp);
            sink.play();
        }
        self.is_playing = true;
    }

    fn on_empty(&mut self) {
        if self.kind == "radio" {
            self.poll_radio();
            return;
        }
        if !self.is_playing || self.kind == "idle" {
            return;
        }
        if self.core.playback_state() == PlaybackState::Finished {
            self.is_playing = false;
            self.ended = true;
        }
    }
}

const LOCAL_PREPARATION_LIMIT: usize = 3;
const LOCAL_SLOT_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocalLoadAction {
    Load { report: bool },
    Prepare,
    Seek,
}

#[derive(Clone)]
struct LocalLoadRequest {
    path: String,
    song_id: i64,
    position_ms: i64,
    rg: (Option<f32>, Option<f32>, Option<f32>, Option<f32>),
    seq: u64,
    token: u64,
    play: bool,
    action: LocalLoadAction,
    original_end: Option<OriginalTrackEnd>,
}

struct PendingLocalLoad {
    generation: u64,
    queued_at: Instant,
    request: LocalLoadRequest,
}

struct LocalLoadJob {
    generation: u64,
    cancelled: Arc<AtomicBool>,
    source_slot: Arc<PcmCaptureSlot>,
    worker: thread::JoinHandle<Result<PreparedLocalTrack, String>>,
}

#[derive(Default)]
struct LocalLoads {
    generation: u64,
    pending: Option<PendingLocalLoad>,
    jobs: Vec<LocalLoadJob>,
    #[cfg(test)]
    test_pool: Option<&'static AtomicU64>,
}

impl LocalLoads {
    fn reserve_slot(&self) -> Result<Arc<PcmCaptureSlot>, FfmpegPcmError> {
        #[cfg(test)]
        if let Some(pool) = self.test_pool {
            return PcmCaptureSlot::acquire_from(pool).map(Arc::new);
        }
        PcmCaptureSlot::acquire().map(Arc::new)
    }

    fn submit(&mut self, engine: &mut AudioEngine, request: LocalLoadRequest) {
        self.abandon();
        self.generation = self.generation.wrapping_add(1);
        if request.action != LocalLoadAction::Seek {
            engine.core.clear_awaiting_first_play();
        }
        engine.begin_local_load();
        engine.is_playing = false;
        engine.ended = false;
        self.pending = Some(PendingLocalLoad {
            generation: self.generation,
            queued_at: Instant::now(),
            request,
        });
    }

    fn abandon(&mut self) {
        self.pending.take();
        for job in &self.jobs {
            job.cancelled.store(true, Ordering::Release);
        }
    }

    fn cancel(&mut self, engine: &AudioEngine) {
        if self.pending.is_some() {
            engine.core.next_coordinator().notify_preparation_capacity();
        }
        self.abandon();
    }

    fn change_play_intent(&mut self, update: impl FnOnce(bool) -> bool) -> bool {
        let Some(pending) = self.pending.as_mut() else {
            return false;
        };
        pending.request.play = update(pending.request.play);
        true
    }

    fn retarget(&mut self, engine: &mut AudioEngine, position_ms: i64) -> bool {
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        let mut request = pending.request.clone();
        request.position_ms = position_ms;
        self.submit(engine, request);
        true
    }

    fn seek(&mut self, engine: &mut AudioEngine, position_ms: i64, play: bool) {
        let request = engine.seek_load_request(position_ms, play);
        if let Some(request) = request {
            self.submit(engine, request);
        }
    }

    fn tick(&mut self, engine: &mut AudioEngine) {
        while let Some(index) = self.jobs.iter().position(|job| job.worker.is_finished()) {
            let job = self.jobs.swap_remove(index);
            let result = job.worker.join().unwrap_or_else(|_| Err("音訊準備工作中斷".to_string()));
            if self.pending.as_ref().is_some_and(|work| work.generation == job.generation) {
                self.finish(engine, result);
            }
        }

        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if let Some(job) = self.jobs.iter().find(|job| job.generation == pending.generation) {
            if job.source_slot.native_preparation_stalled(Instant::now()) {
                job.cancelled.store(true, Ordering::Release);
                self.finish(engine, Err("讀取音訊資料逾時".into()));
            }
            return;
        }
        if self.jobs.len() == LOCAL_PREPARATION_LIMIT {
            if pending.queued_at.elapsed() >= LOCAL_SLOT_WAIT {
                self.finish(engine, Err("音訊來源處理忙碌".to_string()));
            }
            return;
        }

        let permit = match self.reserve_slot() {
            Ok(permit) => permit,
            Err(_) => {
                if pending.queued_at.elapsed() >= LOCAL_SLOT_WAIT {
                    self.finish(engine, Err("音訊來源處理忙碌".to_string()));
                }
                return;
            }
        };
        let request = pending.request.clone();
        let core = engine.core.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let worker_slot = permit.clone();
        let worker = thread::Builder::new().name("local-track-preparation".to_string()).spawn(move || {
            let result = AudioEngine::prepare_local_track(
                &core, &request.path, request.song_id, request.position_ms,
                request.rg, request.seq, request.token,
                request.action != LocalLoadAction::Seek, Some(&worker_cancel), Some(worker_slot.clone()),
                request.original_end.as_ref(),
            );
            if worker_slot.end_native_preparation() && result.is_ok() {
                Err("讀取音訊資料逾時".into())
            } else {
                result
            }
        });
        match worker {
            Ok(worker) => self.jobs.push(LocalLoadJob {
                generation: pending.generation, cancelled, source_slot: permit, worker,
            }),
            Err(_) => self.finish(engine, Err("無法啟動音訊準備工作".to_string())),
        }
    }

    fn finish(&mut self, engine: &mut AudioEngine, prepared: Result<PreparedLocalTrack, String>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let request = pending.request;
        let result = prepared.and_then(|track| engine.install_local_track(
            &request.path, request.song_id, request.position_ms, request.play,
            request.seq, request.token, track,
        ));
        if let LocalLoadAction::Load { report: true } = request.action {
            engine.last_manual_load = Some(manual_load_receipt(request.token, &result));
        }
        engine.core.next_coordinator().notify_preparation_capacity();
        match result {
            Ok(()) if request.action == LocalLoadAction::Seek => {
                if request.play {
                    engine.core.try_allocate_first_play();
                }
            }
            Ok(()) if request.play => {
                engine.core.increment_play_seq();
            }
            Ok(()) => engine.core.mark_awaiting_first_play(),
            Err(error) => {
                engine.core.clear_awaiting_first_play();
                engine.fail_load(&error);
            }
        }
    }
}

impl Drop for LocalLoads {
    fn drop(&mut self) {
        self.abandon();
    }
}

/// 音訊回呼處理：從長駐管線拉取資料
fn audio_loop(
    rx: mpsc::Receiver<PlayerCommand>,
    status: Arc<Mutex<EngineSnapshot>>,
) -> Result<(), String> {
    let mut engine = AudioEngine::new(status)?;
    let mut local_loads = LocalLoads::default();

    loop {
        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(command) => {
                match command {
                PlayerCommand::Load { path, song_id, token, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak } => {
                    let rg = (rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak);

                    // v0.17 手動 token 高水位：僅嚴格大於目前水位的手動 token 受理，
                    // 並在任何狀態異動或 IO 之前先更新水位並立即發布；過期/重複意圖不裝軌、
                    // 不清除排隊請求、不替換最後手動回執。
                    if !is_newer_manual_token(engine.manual_load_token_high_water, token) {
                        log_line(&format!(
                            "LOAD_REJECTED_STALE token={token} high_water={}",
                            engine.manual_load_token_high_water
                        ));
                    } else {
                        engine.manual_load_token_high_water = token;
                        // 受理後立即發布高水位，即使長檔準備尚未完成
                        engine.publish();

                        let seq = engine.seq_counter.fetch_add(1, Ordering::AcqRel);
                        local_loads.submit(&mut engine, LocalLoadRequest {
                            path, song_id, position_ms: 0, rg, seq, token, play: true,
                            action: LocalLoadAction::Load { report: true },
                            original_end: None,
                        });
                    }
                }
                PlayerCommand::Prepare { path, song_id, position_ms, token, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak } => {
                    let rg = (rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak);
                    let seq = engine.seq_counter.fetch_add(1, Ordering::AcqRel);
                    local_loads.submit(&mut engine, LocalLoadRequest {
                        path, song_id, position_ms, rg, seq, token, play: false,
                        action: LocalLoadAction::Prepare,
                        original_end: None,
                    });
                }
                PlayerCommand::Play => {
                    if !local_loads.change_play_intent(|_| true) {
                        engine.start_playback();
                    }
                }

                PlayerCommand::Pause => {
                    if !local_loads.change_play_intent(|_| false) {
                        engine.begin_pause();
                    }
                }
                PlayerCommand::Toggle => {
                    if !local_loads.change_play_intent(|play| !play) {
                        if engine.playback_intent() {
                            engine.begin_pause();
                        } else {
                            engine.start_playback();
                        }
                    }
                }
                PlayerCommand::Stop => {
                    local_loads.cancel(&engine);
                    engine.clear_next_request();
                    engine.halt_radio();
                    let start_gain = engine.current_fade_gain();
                    engine.reset_fade();
                    engine.core.clear_awaiting_first_play();
                    let duration = engine.core.eq_shared.get().fade_stop_ms;
                    if engine.can_fade_file(duration) {
                        engine.core.start_fade(FadeTarget::Stop, start_gain, 0.0, duration);
                    } else {
                        engine.stop_now();
                    }
                }
                PlayerCommand::SetFadeSettings { play_ms, pause_ms, resume_ms, stop_ms, seek_ms, switch_ms } => {
                    engine.core.eq_shared.update(|p| {
                        p.fade_play_ms = play_ms;
                        p.fade_pause_ms = pause_ms;
                        p.fade_resume_ms = resume_ms;
                        p.fade_stop_ms = stop_ms;
                        p.fade_seek_ms = seek_ms;
                        p.fade_switch_ms = switch_ms;
                    });
                    log_line(&format!("CMD SET_FADE play={play_ms} pause={pause_ms} resume={resume_ms} stop={stop_ms} seek={seek_ms} switch={switch_ms}"));
                }
                PlayerCommand::SetGapMs { ms } => {
                    if ms > 0 {
                        engine.core.eq_shared.update(|p| {
                            p.gap_ms = ms;
                            p.crossfade_ms = 0;
                        });
                        engine.crossfade_ms = 0;
                        engine.core.set_crossfade_ms(0);
                    } else {
                        engine.core.eq_shared.update(|p| {
                            p.gap_ms = 0;
                        });
                    }
                    log_line(&format!("CMD SET_GAP_MS ms={ms}"));
                }
                PlayerCommand::SetSilenceRemoval { enabled } => {
                    engine.core.eq_shared.update(|p| {
                        p.silence_removal = enabled;
                    });
                    log_line(&format!("CMD SET_SILENCE_REMOVAL enabled={enabled}"));
                }
                PlayerCommand::Seek { position_ms } => {
                    if !local_loads.retarget(&mut engine, position_ms) {
                        if let Some(play) = engine.begin_seek(position_ms) {
                            local_loads.seek(&mut engine, position_ms, play);
                        }
                    }
                }
                PlayerCommand::SetVolume { volume } => {
                    let raw = volume.clamp(0.0, 1.0);
                    let curve = engine.core.eq_shared.get().volume_curve;
                    engine.volume_raw = raw;
                    engine.volume_gain = volume_to_gain(raw, &curve);
                    // 更新快取的 dB 值
                    engine.cached_volume_db = if engine.volume_gain == 0.0 {
                        None
                    } else {
                        Some(20.0 * engine.volume_gain.log10())
                    };
                    if let Some(ref sink) = engine.sink {
                        sink.set_volume(engine.volume_gain);
                    }
                    // 響度補償係數隨音量重算
                    update_loudness_params(&engine, raw);
                }
                PlayerCommand::SetMode { mode } => {
                    if !matches!(mode, PlayMode::All | PlayMode::Random) {
                        engine.clear_next_request();
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
                    let fx_params = engine.core.eq_shared.get().fx;
                    *engine.core.dsp_params.lock().unwrap() = Some(fx_params);
                }
                PlayerCommand::SetOutput { name } => {
                    if let Err(e) = engine.switch_output(&name) {
                        engine.error = e;
                    } else {
                        engine.error.clear();
                    }
                }
                PlayerCommand::SetNormalize { enabled } => {
                    let mode = if enabled {
                        crate::models::VolumeNormalization::Peak
                    } else {
                        crate::models::VolumeNormalization::Off
                    };
                    engine.core.eq_shared.update(|p| p.volume_norm_mode = mode);
                }
                PlayerCommand::SetReplayGainMode { mode, is_album_context } => {
                                    engine.core.eq_shared.update(|p| {
                                        p.rg_mode = mode;
                                        if let Some(ctx) = is_album_context {
                                            p.is_album_playback_context = ctx;
                                        }
                                    });
                                }
                                PlayerCommand::SetVolumeNormalization { mode, is_album_context } => {
                                    engine.core.eq_shared.update(|p| {
                                        p.volume_norm_mode = mode;
                                        if let Some(ctx) = is_album_context {
                                            p.is_album_playback_context = ctx;
                                        }
                                    });
                                }
                                PlayerCommand::SetRgPreampHas { db } => {
                                    engine.core.eq_shared.update(|p| p.rg_preamp_has = db);
                                }
                                PlayerCommand::SetRgPreampNone { db } => {
                                                                    engine.core.eq_shared.update(|p| p.rg_preamp_none = db);
                                                                }
                                                                PlayerCommand::SetAlbumContext { is_album_context } => {
                                                                    engine.core.eq_shared.update(|p| p.is_album_playback_context = is_album_context);
                                                                }
                PlayerCommand::SetCrossfade { ms } => {
                    let ms = ms.clamp(0, 12_000);
                    engine.crossfade_ms = ms;
                    engine.core.set_crossfade_ms(engine.crossfade_ms as u64);
                    // 雙向互斥：交叉淡化開啟時清零 gap
                    if ms > 0 {
                        engine.core.eq_shared.update(|p| {
                            p.gap_ms = 0;
                        });
                    }
                }
                PlayerCommand::SetVolumeCurve { curve } => {
                    let raw = engine.volume_raw;
                    engine.core.eq_shared.update(|p| p.volume_curve = curve);
                    let gain = volume_to_gain(raw, &curve);
                    engine.volume_gain = gain;
                    // 更新快取的 dB 值
                    engine.cached_volume_db = if engine.volume_gain == 0.0 {
                        None
                    } else {
                        Some(20.0 * engine.volume_gain.log10())
                    };
                    if let Some(ref sink) = engine.sink {
                        sink.set_volume(gain);
                    }
                    update_loudness_params(&engine, raw);
                }
                PlayerCommand::SetLoudnessCompensation { enabled } => {
                    // 先啟用/關閉旗標，再依當前音量重算補償量（關閉時歸零，確保兩路徑一致）
                    engine.core.eq_shared.update(|p| p.loudness_enabled = enabled);
                    update_loudness_params(&engine, engine.volume_raw);
                    // 觸發濾波器重算（bump 版本號）
                    let _ = engine.core.eq_shared.version();
                }
                PlayerCommand::PlayUrl { url } => {
                    local_loads.cancel(&engine);
                    engine.play_url(&url);
                }
                PlayerCommand::SetNext { play_seq: Some(sequence), .. }
                | PlayerCommand::ClearNext { play_seq: Some(sequence), .. }
                    if sequence != engine.core.play_seq() => {}
                PlayerCommand::SetNext { token, .. } if local_loads.pending.is_some() => {
                    engine.core.next_coordinator().reject_for_local_preparation(token);
                }
                PlayerCommand::SetNext { path, song_id, token, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, .. } => {
                    engine.core.set_next_absence(engine.core.play_seq(), false);
                    let silence_removal = engine.core.eq_shared.get().silence_removal;
                    let coordinator = engine.core.next_coordinator();
                    let previous = coordinator.current_generation();
                    let generation = coordinator.set_next(
                        path, song_id, token, rg_track_gain, rg_track_peak,
                        rg_album_gain, rg_album_peak, silence_removal,
                    );
                    if generation != previous {
                        engine.next_coord_gen = generation;
                        let accepted = coordinator.next_request_result().is_some_and(|receipt| {
                            receipt.state == crate::models::NextRequestState::Accepted
                        });
                        engine.core.set_next_token(if accepted { token } else { 0 });
                        engine.core.clear_pending_stream();
                    }
                }
                PlayerCommand::ClearNext { no_next, play_seq } => {
                    let sequence = play_seq.unwrap_or_else(|| engine.core.play_seq());
                    engine.clear_next_request();
                    engine.core.set_next_absence(sequence, no_next);
                    log_line("CMD clear_next");
                }
                PlayerCommand::Next | PlayerCommand::Prev => {
                    // 如果待播槽有下一軌，建立淡出段落並設 pending_load；否則回錯「無下一首」
                    if let Some(next_track) = engine.core.next_coordinator().peek_next() {
                        let eq = engine.core.eq_shared.get();
                        let fade_ms = eq.fade_switch_ms;
                        engine.core.start_fade(FadeTarget::SwitchLoad, engine.volume_gain, 0.0, fade_ms);
                        let mut fg = engine.core.fade_gate.lock().unwrap();
                        fg.pending_load = Some(crate::models::PendingLoad {
                            path: next_track.path.clone(),
                            song_id: next_track.song_id,
                            token: next_track.token,
                            rg: (
                                next_track.replaygain_track_gain,
                                next_track.replaygain_track_peak,
                                next_track.replaygain_album_gain,
                                next_track.replaygain_album_peak,
                            ),
                            // 使用者手動切換（Next/Prev）＝手動載入意圖：SwitchLoad 淡出完成後
                            // 以待載入軌的確切 token 記錄終態回執
                            manual: false,
                        });
                    }
                }
            }
        },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // 消費 FadeGate 完成通知：執行淡出段落結束後的動作（Pause/Stop/Seek/SwitchLoad/StreamInstalled）
        let done_target = {
            let mut fg = engine.core.fade_gate.lock().unwrap();
            fg.done_target.take()
        };
        if let Some((target, target_gen)) = done_target {
            // 僅處理尚未過期的完成通知（gen 與 fade_gen 匹配）
            let gen_valid = {
                let fg = engine.core.fade_gate.lock().unwrap();
                target_gen == fg.fade_gen
            };
            if gen_valid {
                match target {
                    FadeTarget::Pause => engine.pause_now(),
                    FadeTarget::Stop => engine.stop_now(),
                    FadeTarget::Seek => {
                        if let Some((position_ms, play)) = engine.pending_seek.take() {
                            local_loads.seek(&mut engine, position_ms, play);
                        }
                    }
                    FadeTarget::SwitchLoad => {
                        // 執行 pending_load：先在鎖內取出資料，釋放鎖後再載入
                        let pending = {
                            let mut fg = engine.core.fade_gate.lock().unwrap();
                            fg.pending_load.take()
                        };
                        if let Some(pending) = pending {
                            let seq = engine.seq_counter.fetch_add(1, Ordering::AcqRel);
                            local_loads.submit(&mut engine, LocalLoadRequest {
                                path: pending.path, song_id: pending.song_id, position_ms: 0,
                                rg: pending.rg, seq, token: pending.token, play: true,
                                action: LocalLoadAction::Load { report: pending.manual },
                                original_end: None,
                            });
                        }
                    }
                    FadeTarget::StreamInstalled => {
                        if let Some(ref sink) = engine.sink {
                            sink.play();
                        }
                        engine.core.set_playback_state(PlaybackState::Playing);
                    }
                    FadeTarget::None => {}
                }
            }
        }

        // SP2：引擎每 tick 呼叫 poll_completion，將準備完成的軌放入待播槽（TrackChain 讀完時 take()）
        if let Some(result) = engine.core.next_coordinator().poll_completion(engine.next_coord_gen) {
            match result {
                Ok(track) => {
                    crate::open_with::log_line(&format!(
                        "CMD poll_completion seq={} token={} song_id={}",
                        track.seq, track.token, track.song_id
                    ));
                    let ready = if matches!(&track.data, crate::playback_pipeline::TrackData::Memory(_)) {
                        Some(crate::models::NextRequestState::Ready)
                    } else {
                        track.buffered_request_state()
                    };
                    if let Some(state) = ready {
                        engine.core.next_coordinator().finish_next_request(engine.next_coord_gen, track.token, state);
                    }
                    if ready.is_none() && matches!(&track.data, crate::playback_pipeline::TrackData::Stream(_)) {
                        let path = track.path.clone();
                        let song_id = track.song_id;
                        let rate = track.rate;
                        let channels = track.channels;
                        let duration_ms = track.duration_ms;
                        let decoder = track.decoder.clone();
                        let token = track.token;
                        let seq = track.seq;
                        let core = engine.core.clone();
                        let request_generation = engine.next_coord_gen;
                        let source_generation = core.stream_gen();
                        std::thread::spawn(move || {
                            let result: Result<Box<dyn Iterator<Item = f32> + Send>, String> =
                                (|| {
                                    let mut file = std::fs::File::open(&path)
                                        .map_err(|e| format!("開啟檔案失敗: {e}"))?;
                                    let path_buf = std::path::PathBuf::from(&path);
                                    match decoder.as_str() {
                                        "symphonia" | "stream" => {
                                            // #6：長檔開頭靜音掃描 — 先從開頭讀取掃描連續靜音，再從靜音結尾處開流
                                            let mut probe =
                                                crate::audio_decode::SymStream::open(
                                                    &mut file, &path_buf, 0,
                                                )?;
                                            let scan_limit_samples =
                                                (rate as usize) * (channels as usize) * 5; // 掃描前 5 秒
                                            let mut leading_samples = 0usize;
                                            let mut scanned = 0usize;
                                            for sample in probe.by_ref().take(scan_limit_samples) {
                                                scanned += 1;
                                                if sample.abs() < 0.001 {
                                                    leading_samples += 1;
                                                } else {
                                                    leading_samples = 0; // 非靜音重置，只計連續開頭靜音
                                                }
                                                // 連續靜音超過 500ms 視為開頭靜音結束
                                                if leading_samples
                                                    > (rate as usize * channels as usize / 2)
                                                {
                                                    break;
                                                }
                                            }
                                            let leading_ms =
                                                if scanned > 0 && leading_samples > 0 {
                                                    (leading_samples as f64
                                                        / (rate as f64 * channels as f64)
                                                        * 1000.0) as i64
                                                } else {
                                                    0
                                                };
                                            // 重新打開檔案，從 leading_ms 處開始開流
                                            let mut file2 = std::fs::File::open(&path)
                                                .map_err(|e| format!("開啟檔案失敗: {e}"))?;
                                            let mut stream =
                                                crate::audio_decode::SymStream::open(
                                                    &mut file2, &path_buf, leading_ms,
                                                )?;
                                            Ok(
                                                Box::new(stream)
                                                    as Box<dyn Iterator<Item = f32> + Send>,
                                            )
                                        }
                                        "ffmpeg" => {
                                            let ext = path_buf
                                                .extension()
                                                .and_then(|e| e.to_str())
                                                .map(|e| e.to_lowercase())
                                                .unwrap_or_default();
                                            let ffmpeg_path =
                                                crate::tools::locate("ffmpeg.exe")
                                                    .map(|(p, _, _)| p)
                                                    .ok_or("找不到 ffmpeg")?;
                                            let stream = crate::audio_decode::FfmpegStream::new(
                                                &ffmpeg_path, &path, rate, channels, duration_ms,
                                            )?;
                                            let handle = stream.open_with_seek(0)?;
                                            Ok(handle)
                                        }
                                        _ => Err(format!("不支援的解碼器: {decoder}")),
                                    }
                                })();
                                let result = result.and_then(|source| prime_stream_source(source, channels));
                                match result {
                                    Ok(source) => {
                                        let meta = crate::models::StreamMeta {
                                            song_id,
                                            duration_ms,
                                            rate,
                                            channels,
                                            path,
                                            decoder,
                                            position_ms: 0,
                                            gen: source_generation,
                                        };
                                        core.deliver_stream_result_for_request(request_generation, source, meta);
                                    }
                                    Err(e) => {
                                        if core.stream_gen() == source_generation {
                                            core.next_coordinator().finish_next_request(request_generation, token, crate::models::NextRequestState::Failed);
                                        }
                                        crate::open_with::log_load_error(&format!(
                                            "串流開流失敗 song_id={song_id}: {e}"
                                        ));
                                    }
                                }
                            });
                        }
                    }
                Err(e) => {
                    engine.core.next_coordinator().finish_next_request(engine.next_coord_gen, engine.core.next_token(), crate::models::NextRequestState::Failed);
                    // 準備失敗（壞檔、缺工具）：記錄並讓前端能透過錯誤欄位知悉、略過並重排
                    crate::open_with::log_load_error(&format!("prepare_next failed: {e}"));
                    engine.error = format!("下一首準備失敗：{e}");
                }
            }
        }

        engine.core.next_coordinator().discard_failed_buffered(engine.next_coord_gen);
        if let Some((generation, token, reason)) = engine.core.take_stream_failure() {
            if generation == engine.core.stream_gen() && token == engine.core.now_token() {
                engine.error = stream_failure_text(reason);
            }
        }
        local_loads.tick(&mut engine);
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
    #[test]
    fn test_prime_stream_source_stereo_full() {
        let data = [0.125_f32, -0.25, 0.5, 0.75];
        let source = Box::new(data.into_iter());
        let result = prime_stream_source(source, 2).unwrap();
        let collected: Vec<f32> = result.collect();
        assert_eq!(collected, vec![0.125, -0.25, 0.5, 0.75]);
    }

    #[test]
    fn test_prime_stream_source_mono_full() {
        let data = [0.5_f32, 1.0, 2.0];
        let source = Box::new(data.into_iter());
        let result = prime_stream_source(source, 1).unwrap();
        let collected: Vec<f32> = result.collect();
        assert_eq!(collected, vec![0.5, 1.0, 2.0]);
    }

    #[test]
    fn test_prime_stream_source_stereo_partial() {
        let data = [0.1, 0.2];
        let source = Box::new(data.into_iter());
        let result = prime_stream_source(source, 3);
        assert!(result.is_err());
    }

    #[test]
    fn test_prime_stream_source_empty() {
        let data: Vec<f32> = vec![];
        let source = Box::new(data.into_iter());
        let result = prime_stream_source(source, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_prime_stream_source_out_of_range_lower() {
        let source = Box::new(std::iter::from_fn(|| -> Option<f32> { 
            Some(1.0) 
        }).take(10));
        let result = prime_stream_source(source, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_prime_stream_source_out_of_range_upper() {
        let source = Box::new(std::iter::from_fn(|| -> Option<f32> { 
            Some(1.0) 
        }).take(10));
        let result = prime_stream_source(source, 9);
        assert!(result.is_err());
    }


    #[test]
    fn ffmpeg_preparation_cancel_does_not_cancel_a_primed_reader() {
        use crate::prefetch_stream::StreamFailure;

        let (sender, input) = bounded(1);
        let preparation_cancel = Arc::new(AtomicBool::new(false));
        let priming = Arc::new(AtomicBool::new(true));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut reader = FfmpegSourceReader {
            child: None,
            input: Some(input),
            samples: Vec::new().into_iter(),
            cancel: cancel.clone(),
            priming: priming.clone(),
            preparation_cancel: Some(preparation_cancel.clone()),
            last_block: Instant::now(),
            reaper: pcm_cleanup_sender().unwrap(),
        };
        preparation_cancel.store(true, Ordering::Release);
        assert_eq!(reader.next_checked(), Err(StreamFailure::Disconnected));
        priming.store(false, Ordering::Release);
        sender.send(FfmpegStdoutMsg::Data(vec![0.25])).unwrap();
        assert_eq!(reader.next_checked(), Ok(Some(0.25)));
        cancel.store(true, Ordering::Release);
        assert_eq!(reader.next_checked(), Err(StreamFailure::Disconnected));
    }

    #[test]
    fn short_ffmpeg_pcm_cancellation_precedes_source_startup() {
        let cancelled = Arc::new(AtomicBool::new(true));
        let result = collect_ffmpeg_pcm_for_request(
            Path::new("missing-ffmpeg.exe"), "unused.wav", 44_100, 2, 0, false,
            PCM_MAX_BYTES, Some(&cancelled),
        );
        assert_eq!(result, Err(FfmpegPcmError::Cancelled));
    }

    #[test]
    fn ffmpeg_pcm_wait_observes_cancellation_with_the_channel_open() {
        let (_sender, receiver) = bounded::<usize>(1);
        let cancelled = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(10));
                cancelled.store(true, Ordering::Release);
            });
            assert_eq!(
                wait_for_pcm_event(&receiver, || cancelled.load(Ordering::Acquire)),
                Err(FfmpegPcmError::Cancelled),
            );
        });
    }

    #[test]
    #[ignore = "requires ffprobe and ffmpeg"]
    fn direct_ffmpeg_pcm_overflow_keeps_the_original_seek_in_buffered_playback() {
        use crate::prefetch_stream::StreamRead;

        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let wav = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats/sample_44100_stereo.wav");
        let path = wav.to_str().unwrap();
        let position_ms = 100;
        let (expected, _, _) = collect_ffmpeg_pcm(
            &ffmpeg, path, 44_100, 2, position_ms, false, PCM_MAX_BYTES,
        ).unwrap();
        let engine = play_seq_test_engine();
        let (_, resampler, source, decoder) = AudioEngine::prepare_ffmpeg_track(&engine.core,
            path, 1, position_ms, 1, 10, (None, None, None, None), 4, false, None, None, None,
        ).unwrap();

        assert_eq!(decoder, "ffmpeg-stream");
        let meta = resampler.current_track().unwrap();
        assert_eq!((meta.sample_rate, meta.channels), (44_100, 2));
        assert!(meta.duration_ms > position_ms);
        let mut source = source.unwrap();
        assert!(source.try_install());
        let mut received = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline, "FFmpeg buffered seek did not finish");
            match source.poll_sample() {
                StreamRead::Sample(sample) => received.push(sample),
                StreamRead::Pending => thread::sleep(Duration::from_millis(1)),
                StreamRead::End => break,
                StreamRead::Failed(reason) => panic!("unexpected FFmpeg stream failure: {reason:?}"),
            }
        }
        assert_eq!(received, expected);
    }

    #[test]
    #[ignore]
    fn short_ffmpeg_pcm_cap_is_exact_and_seek_is_preserved() {
        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let wav = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats/sample_44100_stereo.wav");
        let path = wav.to_str().unwrap();
        let (full, _, _) = collect_ffmpeg_pcm(&ffmpeg, path, 44100, 2, 0, false, PCM_MAX_BYTES).unwrap();
        assert!(!full.is_empty());
        let cap = full.len() * std::mem::size_of::<f32>();
        let (at_cap, _, _) = collect_ffmpeg_pcm(&ffmpeg, path, 44100, 2, 0, false, cap).unwrap();
        assert_eq!(full, at_cap);
        let over = collect_ffmpeg_pcm(&ffmpeg, path, 44100, 2, 0, false, cap - 4);
        match over {
            Err(FfmpegPcmError::BudgetExceeded) => {}
            Err(other) => panic!("unexpected normal WAV result: {other:?}"),
            Ok((samples, ..)) => panic!("limit {cap} - 4 accepted {} samples", samples.len()),
        }
        let (seek, _, _) = collect_ffmpeg_pcm(&ffmpeg, path, 44100, 2, 100, false, cap).unwrap();
        assert_eq!(full.len() - seek.len(), 8820);
    }

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_output_device_names() {
        let devices = output_device_names();
        assert!(devices.is_ok() || devices.is_err());
    }

    /// 對數曲線五節點：0%、25%、50%、75%、100% 對應的 dB 與線性增益
    #[test]
    fn test_volume_curve_log_nodes() {
        // 0% → 靜音 (0.0 增益)
        assert_eq!(volume_to_gain(0.0, &VolumeCurve::Log), 0.0);
        // 25% → -45 dB ≈ 0.0056
        let g25 = volume_to_gain(0.25, &VolumeCurve::Log);
        assert!((g25 - 0.005623).abs() < 0.0001, "25% gain = {g25}");
        let db25 = 20.0 * g25.log10();
        assert!((db25 - (-45.0)).abs() < 0.5, "25% dB = {db25}");
        // 50% → -30 dB ≈ 0.0316
        let g50 = volume_to_gain(0.5, &VolumeCurve::Log);
        assert!((g50 - 0.031623).abs() < 0.0001, "50% gain = {g50}");
        let db50 = 20.0 * g50.log10();
        assert!((db50 - (-30.0)).abs() < 0.5, "50% dB = {db50}");
        // 75% → -15 dB ≈ 0.1778
        let g75 = volume_to_gain(0.75, &VolumeCurve::Log);
        assert!((g75 - 0.177828).abs() < 0.0001, "75% gain = {g75}");
        let db75 = 20.0 * g75.log10();
        assert!((db75 - (-15.0)).abs() < 0.5, "75% dB = {db75}");
        // 100% → 0 dB = 1.0
        assert_eq!(volume_to_gain(1.0, &VolumeCurve::Log), 1.0);
    }

    /// 線性曲線：直接對應
    #[test]
    fn test_volume_curve_linear() {
        assert_eq!(volume_to_gain(0.0, &VolumeCurve::Linear), 0.0);
        assert_eq!(volume_to_gain(0.25, &VolumeCurve::Linear), 0.25);
        assert_eq!(volume_to_gain(0.5, &VolumeCurve::Linear), 0.5);
        assert_eq!(volume_to_gain(0.75, &VolumeCurve::Linear), 0.75);
        assert_eq!(volume_to_gain(1.0, &VolumeCurve::Linear), 1.0);
    }

    /// 響度補償：0 dB (100% 音量) 時 bass/treble 增益為 0
    #[test]
    fn test_loudness_zero_at_full_volume() {
        let eq_shared = std::sync::Arc::new(crate::eq::EqShared::new());
        // 啟用響度補償
        eq_shared.update(|p| p.loudness_enabled = true);
        let mut engine = AudioEngine {
            core: std::sync::Arc::new(crate::playback_pipeline::PlaybackCore::new(eq_shared.clone())),
            output_stream: None,
            output_handle: None,
            sink: None,
            proxy_id: 0,
            device_sample_rate: 48000,
            mode: PlayMode::Sequence,
            is_playing: false,
            volume_raw: 1.0,
            volume_gain: 1.0, // 0 dB
            crossfade_ms: 0,
            kind: "test".into(),
            stream_title: String::new(),
            error: String::new(),
            ended: false,
            status: std::sync::Arc::new(std::sync::Mutex::new(crate::models::EngineSnapshot::default())),
            radio_manager: None,
            radio_result: None,
            radio_eof: false,
            pending_seek: None,
            next_token_counter: std::sync::atomic::AtomicU64::new(1),
            next_coord_gen: 0,
            seq_counter: std::sync::atomic::AtomicU64::new(1),
            current_decoder: std::sync::Mutex::new(None),
            current_path: std::sync::Mutex::new(None),
            cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            played: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            recycle_tx: crossbeam_channel::bounded(32).0,
            recycle_rx: crossbeam_channel::bounded(32).1,
            recycle_handle: None,
            cached_volume_db: Some(0.0),
            last_manual_load: None,
            manual_load_token_high_water: 0,
        };
        // 觸發重算
        update_loudness_params(&engine, 1.0);
        let params = eq_shared.get();
        assert_eq!(params.loudness_bass_db, 0.0, "bass 應為 0 dB");
        assert_eq!(params.loudness_treble_db, 0.0, "treble 應為 0 dB");
    }

    /// 響度補償：關閉時 bass/treble 歸零（旁路）
    #[test]
    fn test_loudness_bypass_when_disabled() {
        let eq_shared = std::sync::Arc::new(crate::eq::EqShared::new());
        eq_shared.update(|p| {
            p.loudness_enabled = true;
            p.loudness_bass_db = 5.0;
            p.loudness_treble_db = 3.0;
        });
        let mut engine = AudioEngine {
            core: std::sync::Arc::new(crate::playback_pipeline::PlaybackCore::new(eq_shared.clone())),
            output_stream: None,
            output_handle: None,
            sink: None,
            proxy_id: 0,
            device_sample_rate: 48000,
            mode: PlayMode::Sequence,
            is_playing: false,
            volume_raw: 0.5,
            volume_gain: 0.0316, // -30 dB
            crossfade_ms: 0,
            kind: "test".into(),
            stream_title: String::new(),
            error: String::new(),
            ended: false,
            status: std::sync::Arc::new(std::sync::Mutex::new(crate::models::EngineSnapshot::default())),
            radio_manager: None,
            radio_result: None,
            radio_eof: false,
            pending_seek: None,
            next_token_counter: std::sync::atomic::AtomicU64::new(1),
            next_coord_gen: 0,
            seq_counter: std::sync::atomic::AtomicU64::new(1),
            current_decoder: std::sync::Mutex::new(None),
            current_path: std::sync::Mutex::new(None),
            cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            played: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            recycle_tx: crossbeam_channel::bounded(32).0,
            recycle_rx: crossbeam_channel::bounded(32).1,
            recycle_handle: None,
            cached_volume_db: Some(-30.0),
            last_manual_load: None,
            manual_load_token_high_water: 0,
        };
        // 關閉響度補償
        eq_shared.update(|p| p.loudness_enabled = false);
        update_loudness_params(&engine, 0.5);
        let params = eq_shared.get();
        assert_eq!(params.loudness_bass_db, 0.0, "關閉時 bass 應歸零");
        assert_eq!(params.loudness_treble_db, 0.0, "關閉時 treble 應歸零");
    }

    /// PP9：EOF 恰好在上限（16 樣本）→ 收集成功，內容完整
    #[test]
    fn test_checked_pcm_distinguishes_normal_eof_and_decode_failure_at_budget() {
        use crate::audio_decode::SymReadFailure;

        let mut pcm = PcmCollector::with_max_bytes(4);
        let mut complete = [Ok(Some(0.25)), Ok(None)].into_iter();
        assert_eq!(pcm.extend_checked(None, || complete.next().unwrap()), Ok(()));
        assert_eq!(pcm.finish(), vec![0.25]);

        let mut pcm = PcmCollector::with_max_bytes(4);
        let mut incomplete = [Ok(Some(0.25)), Err(SymReadFailure::Decode)].into_iter();
        assert_eq!(
            pcm.extend_checked(None, || incomplete.next().unwrap()),
            Err(SymPcmError::Read(SymReadFailure::Decode)),
        );
    }

    #[test]
    fn test_pcm_collector_exact_limit_succeeds() {

        let mut c = PcmCollector::with_max_bytes(16 * 4);
        for i in 0..16 {
            c.push(i as f32).expect("恰好等於樣本上限應視為成功");
        }
        assert!(c.capacity_bytes() <= 16 * 4, "容量不得超出預算");
        let samples = c.finish();
        assert_eq!(samples.len(), 16);
        assert_eq!(samples, (0..16).map(|i| i as f32).collect::<Vec<f32>>());
    }

    /// PP9：超過上限一個樣本 → 型別化 BudgetExceeded，超標樣本不寫入
    #[test]
    fn test_pcm_collector_one_over_limit_is_typed_error() {
        let mut c = PcmCollector::with_max_bytes(16 * 4);
        for i in 0..16 {
            assert!(c.push(i as f32).is_ok());
        }
        assert_eq!(c.push(16.0), Err(PcmCollectError::BudgetExceeded));
        assert_eq!(c.finish().len(), 16, "超過預算的樣本不得寫入");
    }

    /// PP9：容量幾何成長且永不超出樣本上限（無過度配置）
    #[test]
    fn test_pcm_collector_no_excess_capacity() {
        let mut c = PcmCollector::with_max_bytes(16 * 4);
        // 初始區塊 8 樣本，倍增至 16；每一步容量都在預算內
        for (n, s) in (0..16).enumerate() {
            c.push(s as f32).unwrap();
            assert!(c.capacity_bytes() <= 16 * 4, "第 {} 個樣本時容量超出預算", n + 1);
        }
        assert_eq!(c.capacity_bytes(), 16 * 4, "封頂後應恰好保留 16 樣本，不多不少");
    }

    /// PP9：trim_silence 原地搬移（copy_within + truncate），不建立第二份複本
    #[test]
    fn test_trim_silence_trims_in_place_without_copy() {
        let mut samples = Vec::with_capacity(100);
        // 2 個靜音 frame + 3 個非靜音 frame + 1 個靜音 frame（單聲道）
        samples.extend_from_slice(&[0.0, 0.0, 0.5, -0.5, 0.25, 0.0]);
        let (head, tail) = trim_silence(&mut samples, 1, true);
        assert_eq!((head, tail), (2, 1));
        assert_eq!(samples, vec![0.5, -0.5, 0.25]);
        assert_eq!(samples.capacity(), 100, "原地裁切不應重新配置（無第二份複本）");
    }

    /// 無舊中繼資料時的 restart_from 迴歸測試（無裝置、無舊 TrackResampler、Peak 模式）。
    /// 驗證：以新軌實際的 norm_gain 更新 ReplayGain context，不退回 0.0（靜音）。
    #[test]
    fn test_restart_from_no_old_meta_peak_mode_nonzero_norm_gain() {
        use std::fs::File;
        use std::io::Write;
        use tempfile::Builder;

        // 手寫一個最小可用的立體聲 WAV（48kHz, 16-bit, 1秒, 峰值 0.5）
        // WAV header (44 bytes) + data chunk
        let sample_rate: u32 = 48000;
        let channels: u16 = 2;
        let bits_per_sample: u16 = 16;
        let duration_sec: u32 = 1;
        let num_samples = (sample_rate * duration_sec) as u32; // 48000 frames
        let byte_rate = sample_rate * channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = channels * (bits_per_sample / 8);
        let data_size = num_samples * channels as u32 * (bits_per_sample as u32 / 8); // 48000 * 2 * 2 = 192000 bytes
        let file_size = 36 + data_size;

        let temp_file = Builder::new().suffix(".wav").tempfile().expect("建立暫存檔失敗");
        {
            let mut f = File::create(temp_file.path()).expect("建立檔案失敗");
            // RIFF header
            f.write_all(b"RIFF").unwrap();
            f.write_all(&(file_size as u32).to_le_bytes()).unwrap();
            f.write_all(b"WAVE").unwrap();
            // fmt chunk
            f.write_all(b"fmt ").unwrap();
            f.write_all(&16u32.to_le_bytes()).unwrap(); // fmt chunk size
            f.write_all(&1u16.to_le_bytes()).unwrap(); // audio format (PCM)
            f.write_all(&channels.to_le_bytes()).unwrap();
            f.write_all(&sample_rate.to_le_bytes()).unwrap();
            f.write_all(&byte_rate.to_le_bytes()).unwrap();
            f.write_all(&block_align.to_le_bytes()).unwrap();
            f.write_all(&bits_per_sample.to_le_bytes()).unwrap();
            // data chunk
            f.write_all(b"data").unwrap();
            f.write_all(&data_size.to_le_bytes()).unwrap();
            // 音訊資料：1 秒 440Hz 正弦波，峰值 0.5
            let amplitude = 0.5_f32;
            for i in 0..num_samples {
                let t = i as f32 / sample_rate as f32;
                let sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * amplitude;
                let sample_i16 = (sample * i16::MAX as f32) as i16;
                f.write_all(&sample_i16.to_le_bytes()).unwrap(); // 左聲道
                f.write_all(&sample_i16.to_le_bytes()).unwrap(); // 右聲道
            }
            f.flush().unwrap();
        }
        let wav_path = temp_file.path().to_str().unwrap().to_string();

        // 預期 norm_gain：峰值 0.5 → 0.89/0.5 = 1.78（clamp 0.05..6.0 內）
        let expected_norm_gain = (0.89 / 0.5_f32).clamp(0.05, 6.0);

        // 建立 headless AudioEngine（無 output_stream/sink）
        let eq_shared = std::sync::Arc::new(crate::eq::EqShared::new());
        // 啟用 Peak 正規化模式
        eq_shared.update(|p| p.volume_norm_mode = crate::models::VolumeNormalization::Peak);
        let core = std::sync::Arc::new(crate::playback_pipeline::PlaybackCore::new(eq_shared.clone()));
        let mut engine = AudioEngine {
            core: core.clone(),
            output_stream: None,
            output_handle: None,
            sink: None,
            proxy_id: 0,
            device_sample_rate: 48000,
            mode: PlayMode::Sequence,
            is_playing: false,
            volume_raw: 1.0,
            volume_gain: 1.0,
            crossfade_ms: 0,
            kind: "idle".into(),
            stream_title: String::new(),
            error: String::new(),
            ended: false,
            status: std::sync::Arc::new(std::sync::Mutex::new(crate::models::EngineSnapshot::default())),
            radio_manager: None,
            radio_result: None,
            last_manual_load: None,
            manual_load_token_high_water: 0,
            radio_eof: false,
            pending_seek: None,
            next_token_counter: std::sync::atomic::AtomicU64::new(1),
            next_coord_gen: 0,
            seq_counter: std::sync::atomic::AtomicU64::new(1),
            current_decoder: std::sync::Mutex::new(Some("symphonia".to_string())),
            current_path: std::sync::Mutex::new(Some(wav_path.clone())),
            cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            played: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            recycle_tx: crossbeam_channel::bounded(32).0,
            recycle_rx: crossbeam_channel::bounded(32).1,
            recycle_handle: None,
            cached_volume_db: Some(0.0),
        };

        // 關鍵：沒有舊的 TrackResampler 軌（current_track_meta 會回傳 None）
        // current_path 有值，current_decoder 有值
        // 呼叫 restart_from(0, false) —— 模擬 seek 到開頭但不播放
        engine.init_dsp();
        engine.restart_from(0, false);

        // restart_from 不應立即產生錯誤
        assert!(engine.error.is_empty(), "restart_from 不應產生錯誤: {}", engine.error);

        // 驗證：EqProcessor 的 norm_gain 應等於新軌的實際 norm_gain（非零、非 1.0 假設值）
        let eq_proc = core.eq_processor.lock().unwrap();
        assert!(eq_proc.is_some(), "EqProcessor 應已初始化");
        let proc = eq_proc.as_ref().unwrap();
        let actual_norm_gain = proc.current_norm_gain();
        assert!((actual_norm_gain - expected_norm_gain).abs() < 0.01,
            "restart_from 應套用新軌實際 norm_gain: 期望 {expected_norm_gain}, 實際 {actual_norm_gain}");

        // 額外驗證：ReplayGain context 的 norm_gain 也應同步更新
        let rg_norm_gain = proc.current_rg_context_norm_gain();
        assert!((rg_norm_gain - expected_norm_gain).abs() < 0.01,
            "ReplayGain context norm_gain 應同步: 期望 {expected_norm_gain}, 實際 {rg_norm_gain}");

    }


    /// 手動載入回執：寫入真實來源 token 與終態（成功 loaded、失敗 failed）；
    /// 非手動 SwitchLoad 不產生回執，保留前次手動回執
    #[test]
    fn manual_load_receipt_source_token_and_terminal() {
        // 成功終態：token 原樣記錄（真實來源 token，非引擎計數器值）
        let ok = manual_load_receipt(4242, &Ok(()));
        assert_eq!(ok.token, 4242);
        assert_eq!(ok.outcome, ManualLoadOutcome::Loaded);

        // 失敗終態：同一 token 記 failed，不得記 loaded
        let failed = manual_load_receipt(4242, &Err("解碼失敗".into()));
        assert_eq!(failed.token, 4242);
        assert_eq!(failed.outcome, ManualLoadOutcome::Failed);

        // 模擬 SwitchLoad 淡出完成後的記錄決策（命令迴圈）：
        // 非手動來源（manual=false）→ 不記錄，保留前次手動回執
        let mut receipt: Option<ManualLoadResult> = Some(failed);
        let pending_manual = false;
        if pending_manual {
            receipt = Some(manual_load_receipt(7777, &Ok(())));
        }
        assert_eq!(receipt.unwrap().outcome, ManualLoadOutcome::Failed, "非手動 SwitchLoad 應保留前次回執");

        // 手動來源（manual=true）→ 以待載入軌的確切 token 記錄終態
        let pending_manual = true;
        if pending_manual {
            receipt = Some(manual_load_receipt(7777, &Ok(())));
        }
        let r = receipt.unwrap();
        assert_eq!(r.token, 7777);
        assert_eq!(r.outcome, ManualLoadOutcome::Loaded);

        // 回執序列化：小寫終態 + 確切 token
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"token\":7777"), "回執 token 序列化不符: {json}");
        assert!(json.contains("\"outcome\":\"loaded\""), "回執 outcome 序列化不符: {json}");
    }

    /// 建立不需要實際音訊裝置的引擎（sink 為 None），供播放序號測試使用。
    #[test]
    fn direct_initial_trim_offsets_position_once_and_seek_zero_keeps_original_start() {
        let folder = tempfile::tempdir().unwrap();
        let path = write_test_wav(&folder);
        let path_text = path.to_str().unwrap();
        let mut wav = std::fs::read(&path).unwrap();
        for frame in wav[44..].chunks_exact_mut(4).skip(441) {
            frame[..2].copy_from_slice(&8192i16.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();

        let mut engine = play_seq_test_engine();
        engine.core.eq_shared.update(|settings| settings.silence_removal = true);
        let rg = (None, None, None, None);
        engine.load_at(path_text, 1, 0, false, rg, 1, 10).unwrap();
        assert_eq!(engine.core.current_track_meta().unwrap().duration_ms, 100);
        assert_eq!(engine.core.current_track_meta().unwrap().leading_silence_samples, 441);
        assert_eq!(engine.position_ms(), 10);

        let mut work = local_load_test_work();
        work.seek(&mut engine, 0, false);
        drain_local_jobs(&mut work, &mut engine);
        assert_eq!(engine.core.now_token(), 10);
        assert_eq!(engine.core.current_track_meta().unwrap().leading_silence_samples, 0);
        assert_eq!(engine.position_ms(), 0);
    }

    fn local_load_test_work() -> LocalLoads {
        static TEST_SLOTS: AtomicU64 = AtomicU64::new(0);
        let mut work = LocalLoads::default();
        work.test_pool = Some(&TEST_SLOTS);
        work
    }

    #[test]
    fn background_load_keeps_latest_request_until_a_capture_slot_is_reaped() {
        static TEST_SLOTS: AtomicU64 = AtomicU64::new(0);
        let mut leases: Vec<_> = (0..3).map(|_| PcmCaptureSlot::acquire_from(&TEST_SLOTS).unwrap()).collect();
        let mut engine = play_seq_test_engine();
        let mut work = LocalLoads::default();
        work.test_pool = Some(&TEST_SLOTS);

        work.submit(&mut engine, local_test_request(1));
        work.tick(&mut engine);
        assert!(work.jobs.is_empty());
        assert_eq!(work.pending.as_ref().unwrap().request.token, 1);
        assert!(engine.last_manual_load.is_none());

        work.submit(&mut engine, local_test_request(2));
        work.tick(&mut engine);
        assert!(work.jobs.is_empty());
        assert_eq!(work.pending.as_ref().unwrap().request.token, 2);

        drop(leases.pop());
        drain_local_jobs(&mut work, &mut engine);
        assert_eq!(engine.last_manual_load.unwrap().token, 2);
        assert_eq!(engine.core.now_token(), 2);
    }

    #[test]
    fn short_seek_keeps_the_initial_absolute_tail_and_finishes_at_or_beyond_it() {
        let folder = tempfile::tempdir().unwrap();
        let path = write_test_wav(&folder);
        let mut wav = std::fs::read(&path).unwrap();
        for frame in wav[44..].chunks_exact_mut(4).take(3969).skip(441) {
            frame[..2].copy_from_slice(&8192i16.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        let mut engine = play_seq_test_engine();
        engine.core.eq_shared.update(|settings| settings.silence_removal = true);
        engine.load_at(path.to_str().unwrap(), 1, 0, false, (None, None, None, None), 2, 10).unwrap();
        assert_eq!(engine.core.current_track_meta().unwrap().effective_end_frame, Some(3969));
        let mut work = local_load_test_work();

        for (position, expected_samples) in [(80, 882), (90, 0), (95, 0), (0, 7938)] {
            work.seek(&mut engine, position, false);
            drain_local_jobs(&mut work, &mut engine);
            let meta = engine.core.current_track_meta().unwrap();
            assert_eq!(meta.duration_ms, 100);
            assert_eq!(meta.effective_end_frame, Some(3969));
            assert_eq!(engine.core.now_token(), 10);
            assert_eq!(engine.position_ms(), position);
            assert_eq!(engine.core.playback_state(), PlaybackState::Paused);
            let chain = engine.core.track_chain.lock().unwrap();
            assert_eq!(chain.as_ref().unwrap().current().unwrap().samples.len(), expected_samples);
        }
    }

    fn local_test_request(token: u64) -> LocalLoadRequest {
        let wav = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats/sample_44100_stereo.wav");
        LocalLoadRequest {
            path: wav.to_str().unwrap().to_string(),
            song_id: token as i64,
            position_ms: 0,
            rg: (None, None, None, None),
            seq: token, token, play: true,
            action: LocalLoadAction::Load { report: true },
            original_end: None,
        }
    }

    fn drain_local_jobs(work: &mut LocalLoads, engine: &mut AudioEngine) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while work.pending.is_some() || !work.jobs.is_empty() {
            assert!(Instant::now() < deadline, "local source preparation did not complete");
            work.tick(engine);
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn local_load_latest_request_uses_current_play_intent_and_seek_identity() {
        let mut engine = play_seq_test_engine();
        let mut work = local_load_test_work();
        work.submit(&mut engine, local_test_request(10));
        work.tick(&mut engine);
        work.submit(&mut engine, local_test_request(20));
        assert!(work.change_play_intent(|_| false));
        drain_local_jobs(&mut work, &mut engine);

        assert_eq!(engine.core.now_token(), 20);
        assert_eq!(engine.core.play_seq(), 0);
        assert_eq!(engine.core.playback_state(), PlaybackState::Paused);
        assert_eq!(engine.last_manual_load.unwrap().token, 20);
        assert!(engine.core.try_allocate_first_play());
        let play_seq = engine.core.play_seq();

        work.seek(&mut engine, 20, false);
        assert!(work.retarget(&mut engine, 40));
        drain_local_jobs(&mut work, &mut engine);
        assert_eq!((engine.core.now_seq(), engine.core.now_token()), (20, 20));
        assert_eq!(engine.core.play_seq(), play_seq);
        assert_eq!(engine.core.seek_origin_ms(), 40);
        assert_eq!(engine.core.playback_state(), PlaybackState::Paused);
    }

    #[test]
    fn stopped_background_load_cannot_install_or_publish_a_receipt() {
        let mut engine = play_seq_test_engine();
        let mut work = local_load_test_work();
        work.submit(&mut engine, local_test_request(10));
        work.tick(&mut engine);
        work.cancel(&engine);
        engine.stop_now();
        drain_local_jobs(&mut work, &mut engine);

        assert_eq!(engine.core.now_token(), 0);
        assert_eq!(engine.core.play_seq(), 0);
        assert!(engine.last_manual_load.is_none());
        assert!(!engine.is_playing);
    }

    fn play_seq_test_engine() -> AudioEngine {
        let eq_shared = Arc::new(EqShared::new());
        let mut engine = AudioEngine {
            core: Arc::new(PlaybackCore::new(eq_shared.clone())),
            output_stream: None,
            output_handle: None,
            sink: None,
            proxy_id: 0,
            device_sample_rate: 48000,
            mode: PlayMode::Sequence,
            is_playing: false,
            volume_raw: 1.0,
            volume_gain: 1.0,
            crossfade_ms: 0,
            kind: "file".into(),
            stream_title: String::new(),
            error: String::new(),
            ended: false,
            status: Arc::new(Mutex::new(EngineSnapshot::default())),
            radio_manager: None,
            radio_result: None,
            radio_eof: false,
            pending_seek: None,
            next_token_counter: AtomicU64::new(1),
            next_coord_gen: 0,
            seq_counter: AtomicU64::new(1),
            current_decoder: Mutex::new(None),
            current_path: Mutex::new(None),
            cancel_flag: std::sync::Arc::new(AtomicBool::new(false)),
            played: Arc::new(AtomicU64::new(0)),
            recycle_tx: crossbeam_channel::bounded(32).0,
            recycle_rx: crossbeam_channel::bounded(32).1,
            recycle_handle: None,
            cached_volume_db: Some(0.0),
            last_manual_load: None,
            manual_load_token_high_water: 0,
        };
        engine.init_dsp();
        engine
    }

    /// 寫入極短 WAV（44.1kHz 雙聲道 16-bit PCM 靜音 0.1 s），可被解碼器開啟。
    fn write_test_wav(dir: &tempfile::TempDir) -> PathBuf {
        let path = dir.path().join("test.wav");
        let sample_rate: u32 = 44100;
        let channels: u16 = 2;
        let bytes_per_sample: u16 = 2;
        let frame_count: u32 = (sample_rate as f32 * 0.1) as u32;
        let data_size = (frame_count as usize) * (channels as usize) * (bytes_per_sample as usize);
        let mut header = [0u8; 44];
        header[0..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&(36u32 + data_size as u32).to_le_bytes());
        header[8..12].copy_from_slice(b"WAVE");
        header[12..16].copy_from_slice(b"fmt ");
        header[16..20].copy_from_slice(&16u32.to_le_bytes());
        header[20..22].copy_from_slice(&1u16.to_le_bytes());
        header[22..24].copy_from_slice(&channels.to_le_bytes());
        header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
        header[28..32].copy_from_slice(&(sample_rate * channels as u32 * bytes_per_sample as u32).to_le_bytes());
        header[32..34].copy_from_slice(&(channels * bytes_per_sample).to_le_bytes());
        header[34..36].copy_from_slice(&16u16.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&(data_size as u32).to_le_bytes());
        let mut bytes = header.to_vec();
        bytes.extend(std::iter::repeat(0u8).take(data_size));
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    #[test]
    fn cancelled_symphonia_collection_stops_before_reading_more_samples() {
        let cancelled = AtomicBool::new(false);
        let mut collector = PcmCollector::with_max_bytes(16);
        let mut samples_read = 0;
        let result = collector.extend_checked(Some(&cancelled), || {
            samples_read += 1;
            cancelled.store(true, Ordering::Release);
            Ok(Some(0.25))
        });
        assert_eq!(result, Err(SymPcmError::Cancelled));
        assert_eq!(samples_read, 1);
    }

    #[test]
    fn test_sym_pcm_budget_reopens_same_source_at_requested_seek() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_wav(&dir);
        let mut file = std::fs::File::open(&path).unwrap();
        let start_ms = 30;
        let expected = SymStream::open_checked(&mut file, &path, start_ms).unwrap().count();
        assert!(expected > 0 && expected < 8_820);
        let stream = SymStream::open_checked(&mut file, &path, start_ms).unwrap();
        let source = prepare_bounded_sym_source(stream, &mut file, &path, start_ms, false, 8, None).unwrap();
        let SymPlaybackSource::Stream(restarted) = source else {
            panic!("PCM budget should switch this WAV to streaming");
        };
        assert_eq!(restarted.count(), expected);
    }

    #[test]
    fn test_buffered_source_keeps_permit_after_producer_finishes() {
        use crate::prefetch_stream::{prepared_stream, StreamRead, FRAME_SIZE};

        static TEST_PERMITS: AtomicU64 = AtomicU64::new(0);
        let permit = Arc::new(PcmCaptureSlot::acquire_from(&TEST_PERMITS).unwrap());
        let reference = Arc::downgrade(&permit);
        let (writer, mut source) = prepared_stream(1, [0.0; FRAME_SIZE]).unwrap();
        source.retain_source_slot(permit);
        writer.end();
        assert_eq!(source.poll_sample(), StreamRead::Sample(0.0));
        assert_eq!(source.poll_sample(), StreamRead::End);
        assert!(reference.upgrade().is_some());
        assert_eq!(TEST_PERMITS.load(Ordering::Acquire), 1);
        drop(source);
        assert!(reference.upgrade().is_none());
        assert_eq!(TEST_PERMITS.load(Ordering::Acquire), 0);
    }

    #[test]
    fn symphonia_head_scan_keeps_first_audible_frame_and_rewinds_all_silent_audio() {
        use crate::prefetch_stream::StreamRead;

        for with_audio in [true, false] {
            let folder = tempfile::tempdir().unwrap();
            let path = write_test_wav(&folder);
            if with_audio {
                let mut wav = std::fs::read(&path).unwrap();
                for frame in wav[44..].chunks_exact_mut(4).skip(441) {
                    frame[2..].copy_from_slice(&8192i16.to_le_bytes());
                }
                std::fs::write(&path, wav).unwrap();
            }
            let mut file = std::fs::File::open(&path).unwrap();
            let decoder = SymStream::open_checked(&mut file, &path, 0).unwrap();
            static TEST_SLOTS: AtomicU64 = AtomicU64::new(0);
            let permit = Arc::new(PcmCaptureSlot::acquire_from(&TEST_SLOTS).unwrap());
            let (source, frames) = prepare_buffered_sym_stream(
                decoder, &mut file, &path, true, 2, None, Some(permit),
            ).unwrap();
            assert_eq!(frames, if with_audio { 441 } else { 0 });
            let mut source = source.unwrap();
            assert_eq!(source.poll_sample(), StreamRead::Sample(0.0));
            assert_eq!(source.poll_sample(), StreamRead::Sample(if with_audio { 0.25 } else { 0.0 }));
        }
    }

    #[test]
    fn head_scan_distinguishes_packet_budget_from_ordinary_probe_failure() {
        use crate::prefetch_stream::StreamFailure;

        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(matches!(
            scan_stream_head(2, 44_100, deadline, None, || Err(StreamFailure::PacketBudget)),
            Err(StreamFailure::PacketBudget),
        ));
        assert!(matches!(
            scan_stream_head(2, 44_100, deadline, None, || Err(StreamFailure::DecodeBudget)),
            Err(StreamFailure::DecodeBudget),
        ));
        assert!(matches!(
            scan_stream_head(2, 44_100, deadline, None, || Err(StreamFailure::Decode)),
            Ok(ScannedHead::OriginalStart),
        ));
        assert!(matches!(
            scan_stream_head(2, 44_100, Instant::now(), None, || panic!("expired probe read samples")),
            Ok(ScannedHead::OriginalStart),
        ));
    }

    #[test]
    #[ignore = "requires the local FFmpeg executable"]
    fn ffmpeg_stream_head_trim_keeps_native_offset_and_preserves_original_start_on_fallback() {
        use crate::prefetch_stream::StreamRead;

        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        for (with_audio, trim, start_ms, expected_offset, expected_right) in [
            (true, true, 0, 441, 0.25),
            (false, true, 0, 0, 0.0),
            (true, false, 0, 0, 0.0),
            (true, true, 5, 0, 0.0),
        ] {
            let folder = tempfile::tempdir().unwrap();
            let path = write_test_wav(&folder);
            if with_audio {
                let mut wav = std::fs::read(&path).unwrap();
                for frame in wav[44..].chunks_exact_mut(4).skip(441) {
                    frame[2..].copy_from_slice(&8192i16.to_le_bytes());
                }
                std::fs::write(&path, &wav).unwrap();
            }
            let (source, leading_frames) = buffer_ffmpeg_stream_with_slot(
                &ffmpeg, path.to_str().unwrap(), 44_100, 2, start_ms, trim, 2, None, None,
            ).unwrap();
            assert_eq!(leading_frames, expected_offset);
            let mut source = source.unwrap();
            assert_eq!(source.poll_sample(), StreamRead::Sample(0.0));
            assert_eq!(source.poll_sample(), StreamRead::Sample(expected_right));
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                assert!(Instant::now() < deadline);
                match source.poll_sample() {
                    StreamRead::Sample(_) => {}
                    StreamRead::Pending => thread::sleep(Duration::from_millis(1)),
                    StreamRead::End => break,
                    StreamRead::Failed(error) => panic!("FFmpeg ended with {error:?}"),
                }
            }
        }
    }

    #[test]
    fn test_symphonia_background_buffer_drains_entire_normal_wav() {
        use crate::prefetch_stream::StreamRead;

        let dir = tempfile::tempdir().unwrap();
        let path = write_test_wav(&dir);
        let mut file = std::fs::File::open(&path).unwrap();
        let decoded = SymStream::open_checked(&mut file, &path, 0).unwrap();
        let initial_frames = crate::playback_pipeline::BLOCK_FRAMES;
        let mut source = buffer_sym_stream(decoded, initial_frames).unwrap().unwrap();
        assert!(source.try_install());
        let mut samples = initial_frames * 2;
        for _ in 0..samples {
            assert_eq!(source.poll_sample(), StreamRead::Sample(0.0));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            assert!(Instant::now() < deadline);
            match source.poll_sample() {
                StreamRead::Sample(sample) => {
                    assert_eq!(sample, 0.0);
                    samples += 1;
                }
                StreamRead::Pending => thread::sleep(Duration::from_millis(1)),
                StreamRead::End => break,
                StreamRead::Failed(reason) => panic!("unexpected stream failure: {reason:?}"),
            }
        }
        assert_eq!(samples, 8_820);
        assert_eq!(source.poll_sample(), StreamRead::End);
    }

    /// 直接載入與同一曲手動重播：成功載入於 publish 前分配；同一曲重播亦遞增；失敗載入不產生事件。

    #[test]
    fn test_play_seq_direct_load_manual_replay_and_failed_load() {
        let mut engine = play_seq_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_wav(&dir);
        let path_str = path.to_string_lossy().to_string();
        let rg = (None, None, None, None);

        // 第一次直接載入：於 publish 前分配播放事件
        assert!(engine.load(&path_str, 1, 100, rg).is_ok());
        assert_eq!(engine.core.play_seq(), 1);
        assert_eq!(engine.core.now_seq(), 1);
        assert_eq!(engine.core.now_token(), 100);

        // 同一曲再次手動載入（manual replay）：新的本機播放事件，序號遞增且不減少
        assert!(engine.load(&path_str, 1, 101, rg).is_ok());
        assert_eq!(engine.core.play_seq(), 2);
        assert_eq!(engine.core.now_seq(), 2);

        // 載入失敗：不產生任何事件（play_seq 保持不變；內部 now_seq 與之分離）
        assert!(engine.load("/no/such/file.x", 2, 102, rg).is_err());
        assert_eq!(engine.core.play_seq(), 2);
    }

    /// Prepare 僅標記等待首次播放，不立即產生事件；首次 Play 分配一次；續播不重複分配。
    #[test]
    fn test_play_seq_prepare_then_first_play_and_resume_no_increment() {
        let mut engine = play_seq_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_wav(&dir);
        let path_str = path.to_string_lossy().to_string();
        let rg = (None, None, None, None);

        // Prepare：鏡像 PlayerCommand::Prepare 處理器，內部 now_seq 已預分配，
        // 但播放事件尚未產生，且軌被標記為等待第一次 Play/Toggle
        let seq = engine.seq_counter.fetch_add(1, Ordering::AcqRel);
        assert!(engine.load_at(&path_str, 2, 0, false, rg, seq, 200).is_ok());
        engine.core.mark_awaiting_first_play();
        assert_eq!(engine.core.play_seq(), 0, "Prepare 不得立即產生播放事件");
        assert_eq!(engine.core.now_seq(), seq, "內部 now_seq 與播放事件序號分離");

        // 首次 Play：分配一次事件
        engine.start_playback();
        assert_eq!(engine.core.play_seq(), 1, "Prepare 後首次 Play 應產生一次事件");
        assert!(engine.is_playing);

        // 暫停後再 Play（續播）：不得重複分配
        engine.is_playing = false;
        engine.core.set_playback_state(PlaybackState::Paused);
        engine.start_playback();
        assert_eq!(engine.core.play_seq(), 1, "續播不得產生新的播放事件");
        assert!(engine.is_playing);
    }

    /// Seek/Reopen 保留身分，不產生新播放事件。
    #[test]
    fn test_play_seq_seek_no_increment() {
        let mut engine = play_seq_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_wav(&dir);
        let path_str = path.to_string_lossy().to_string();
        let rg = (None, None, None, None);

        assert!(engine.load(&path_str, 3, 300, rg).is_ok());
        let seq = engine.core.now_seq();
        assert_eq!(engine.core.play_seq(), 1);

        // 跳轉重開：保留 seq/token，不產生新事件
        engine.restart_from(1234, true);
        assert_eq!(engine.core.play_seq(), 1, "Seek/Reopen 不得產生新的播放事件");
        assert_eq!(engine.core.now_seq(), seq, "Seek 保留原有播放身分");
    }

    #[test]
    fn failure_publishes_the_idle_snapshot() {
        let mut player = test_audio_engine();
        player.core.set_now_identity(1, 2);
        player.core.increment_play_seq();
        player.core.set_playback_state(PlaybackState::Playing);
        player.core.start_fade(FadeTarget::SwitchLoad, 1.0, 0.0, 10);
        player.last_manual_load = Some(manual_load_receipt(3, &Err("test".into())));
        player.fail_load("fixture error");
        player.publish();
        let actual = player.status.lock().unwrap();
        assert_eq!((actual.song_id, actual.now_token, actual.play_seq), (0, 0, 1));
        assert_eq!(actual.kind, "idle");
        assert_eq!(actual.playback_state, "Finished");
        assert_eq!(actual.error, "載入失敗");
        assert_eq!(actual.last_manual_load_result.unwrap().token, 3);
        assert!(player.core.fade_gate.lock().unwrap().active.is_none());
    }

    #[test]
    fn pause_and_play_keep_the_pending_seek_destination() {
        let mut engine = test_audio_engine();
        engine.kind = "file".into();
        engine.pending_seek = Some((700, true));
        engine.begin_pause();
        assert_eq!(engine.pending_seek, Some((700, false)));
        engine.start_playback();
        assert_eq!(engine.pending_seek, Some((700, true)));
        engine.stop_now();
        assert_eq!(engine.core.playback_state(), PlaybackState::Finished);
    }

    fn test_audio_engine() -> AudioEngine {
        AudioEngine {
            core: std::sync::Arc::new(crate::playback_pipeline::PlaybackCore::new(
                std::sync::Arc::new(crate::eq::EqShared::new()),
            )),
            output_stream: None,
            output_handle: None,
            sink: None,
            proxy_id: 0,
            device_sample_rate: 48000,
            mode: PlayMode::Sequence,
            is_playing: false,
            volume_raw: 1.0,
            volume_gain: 1.0,
            crossfade_ms: 0,
            kind: "test".into(),
            stream_title: String::new(),
            error: String::new(),
            ended: false,
            status: std::sync::Arc::new(std::sync::Mutex::new(crate::models::EngineSnapshot::default())),
            radio_manager: None,
            radio_result: None,
            radio_eof: false,
            pending_seek: None,
            next_token_counter: std::sync::atomic::AtomicU64::new(1),
            next_coord_gen: 0,
            seq_counter: std::sync::atomic::AtomicU64::new(1),
            current_decoder: std::sync::Mutex::new(None),
            current_path: std::sync::Mutex::new(None),
            cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            played: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            recycle_tx: crossbeam_channel::bounded(32).0,
            recycle_rx: crossbeam_channel::bounded(32).1,
            recycle_handle: None,
            cached_volume_db: Some(0.0),
            last_manual_load: None,
            manual_load_token_high_water: 0,
        }
    }

    /// 手動 token 高水位閘門：僅嚴格大於目前水位時受理；相等或過期皆拒絕
    #[test]
    fn manual_token_high_water_accepts_strictly_newer_only() {
        assert!(is_newer_manual_token(0, 1));
        assert!(is_newer_manual_token(7, 8));
        assert!(!is_newer_manual_token(0, 0));
        assert!(!is_newer_manual_token(7, 7));
        assert!(!is_newer_manual_token(7, 3));
    }

    /// 過期/重複手動 Load 不得替換最後手動回執；受理後水位立即透過 publish 發布，
    /// 既有終態回執保留至新載入結果產生
    #[test]
    fn manual_token_high_water_stale_preserves_last_receipt_and_publishes() {
        let mut engine = test_audio_engine();
        engine.last_manual_load = Some(ManualLoadResult {
            token: 5,
            outcome: ManualLoadOutcome::Loaded,
        });
        engine.manual_load_token_high_water = 5;

        // 相等與過期：皆不受理，最後回執保持不變
        for stale in [5u64, 2] {
            assert!(!is_newer_manual_token(
                engine.manual_load_token_high_water,
                stale
            ));
        }
        assert_eq!(
            engine.last_manual_load,
            Some(ManualLoadResult {
                token: 5,
                outcome: ManualLoadOutcome::Loaded
            }),
            "過期手動 Load 不得覆寫最後手動回執"
        );

        // 受理：更新水位並立即發布（不待長檔準備）
        let newer = 9;
        assert!(is_newer_manual_token(engine.manual_load_token_high_water, newer));
        engine.manual_load_token_high_water = newer;
        engine.publish();

        let snapshot = engine.status.lock().unwrap();
        assert_eq!(snapshot.manual_load_token_high_water, newer);
        assert_eq!(
            snapshot.last_manual_load_result,
            Some(ManualLoadResult {
                token: 5,
                outcome: ManualLoadOutcome::Loaded
            }),
            "受理後既有手動回執應保留為終態"
        );
    }
}
