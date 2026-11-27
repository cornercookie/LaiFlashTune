//! 長駐播放管線
//!
//! 架構：
//! TrackChain → TrackResampler(48kHz) → DSP(48kHz) → DeviceConverter(裝置取樣率) → Sink
//!
//! - TrackChain：串接多軌，內部各軌保留原生取樣率與聲道，依序無縫接上
//! - TrackResampler：把原生取樣率/聲道重取樣到 48kHz 雙聲道（固定內部取樣率）
//! - DSP：在 48kHz 雙聲道下跑到底（等化器、殘響、節奏、正規化、限幅）
//! - DeviceConverter：把 DSP 的 48kHz 雙聲道輸出，重取樣成裝置的取樣率與雙聲道
//! - Sink：rodio 的音訊輸出端，只看到單一長駐的 DspSource

use crate::models::FadeTarget;
use crate::dsp::{DspSource, FxChain, FxParams, RateStage};
use crate::eq::{EqProcessor, EqShared};
use crate::open_with::{log_line, open_validated_file, get_final_path_from_handle};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use rodio::Source;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use crate::audio_decode::precheck_packet_budget;
use crate::scanner::playback_backend_for;

/// DSP 內部固定取樣率
pub const INTERNAL_SAMPLE_RATE: u32 = 48000;
/// DSP 內部固定聲道數
pub const INTERNAL_CHANNELS: u16 = 2;
/// 區塊大小（frame 數）：立體聲 2048 幀 = 4096 個 f32 樣本 = 16 KiB
pub const BLOCK_FRAMES: usize = 2048;
/// 區塊位元組大小（16 KiB）
pub const BLOCK_BYTES: usize = BLOCK_FRAMES * INTERNAL_CHANNELS as usize * 4;
/// 通道容量（區塊數，2 MiB / 16 KiB = 128）
pub const CHANNEL_CAPACITY_CHUNKS: usize = 128;
/// 通道容量位元組（2 MiB）
pub const CHANNEL_CAPACITY_BYTES: usize = CHANNEL_CAPACITY_CHUNKS * BLOCK_BYTES;
/// 交界紀錄上限：最多保留最近 1000 筆，達上限先淘汰最舊再插入，避免無限增長
pub const BOUNDARY_RECORD_CAPACITY: usize = 1000;

/// 播放狀態機
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    /// 正常播放
    Playing,
    /// 目前軌結束、槽空，等待下一首（DSP 凍結，輸出靜音繞過 DSP）
    WaitingForNext,
    /// 暫停
    Paused,
    /// 播放結束（無下一首）
    Finished,
}

/// 唯一權威座標：DSP 輸出端的 48kHz frame（離開 DSP、進入 DeviceConverter 的那一刻）
/// 兩套邊界分離，職責分層：
/// - boundary_in_48k：TrackResampler 負責。原生邊界 × 48000 / 原生取樣率 + 重取樣器延遲 - 尾端補樣。單位：DSP 輸入端 48kHz frame。
/// - boundary_out_48k：DSP 負責。boundary_in_48k 經由 DSP 檢查點映射（含 WSOLA 伸縮、EQ 濾波器延遲、殘響、限幅）轉換而來。單位：DSP 輸出端 48kHz frame。

/// 交界紀錄
#[derive(Debug, Clone)]
pub struct BoundaryRecord {
    /// DSP 輸入端 48kHz frame 位置（用於進度計算）
    pub boundary_in_48k: u64,
    /// DSP 輸出端 48kHz frame 位置（用於交界發布、DeviceConverter 同步）
    pub boundary_out_48k: u64,
    /// 播放序號
    pub seq: u64,
    /// token
    pub token: u64,
    /// 歌曲 ID
    pub song_id: i64,
    /// 正規化增益
    pub norm_gain: f32,
    /// 時長
    pub duration_ms: i64,
    /// 原生取樣率
    pub native_sample_rate: u32,
    /// 原生聲道
    pub native_channels: u16,
}

/// 播放核心狀態（長駐，生命週期獨立於任何 Sink）
enum StreamInput {
    Legacy(Box<dyn Iterator<Item = f32> + Send>),
    Buffered {
        source: crate::prefetch_stream::PreparedStream,
        source_generation: u64,
        token: u64,
        failure_reported: bool,
    },
}

enum RetiredHandoff {
    Candidate {
        _prepared: PreparedHandoff,
        request_generation: u64,
        token: u64,
    },
    Active {
        _source: Option<StreamInput>,
        _chain: Option<TrackChain>,
        _resampler: Option<TrackResampler>,
        _rate_stage: Option<RateStage>,
    },
}

enum PlaybackSample {
    Audio(f32),
    Pending,
}

pub struct PlaybackCore {
    /// 當前 TrackChain
    pub(crate) track_chain: Arc<Mutex<Option<TrackChain>>>,
    /// 當前 TrackResampler
    pub(crate) track_resampler: Arc<Mutex<Option<TrackResampler>>>,
    /// DSP 狀態
    pub(crate) dsp_state: Arc<Mutex<Option<DspStateSnapshot>>>,
    /// 核心鎖：取樣與切換輸出裝置皆在這個臨界區內序列化
    pub(crate) core_lock: Mutex<()>,
    /// 活躍代理 ID（原子世代計數）
    active_id: AtomicU64,
    /// 下一個代理 ID
    next_id: AtomicU64,
    /// 播放狀態機
    playback_state: Mutex<PlaybackState>,
    /// 當前軌播放序號（預載/內部身分序號，於載入或準備時即行分配，不等同於實際播放事件）
    now_seq: AtomicU64,
    /// 當前軌 token
    now_token: AtomicU64,
    /// 實際播放事件序號：與預載/內部 now_seq 分離，從 0 開始，僅於真正的本機播放開始時遞增。
    /// 每次新的本機播放（直接載入成功、Prepare 後首次 Play/Toggle、自然軌交接、使用者切歌 SwitchLoad）
    /// 遞增一次；準備、暫停後續播、跳轉、預載、停止、電台及失敗載入皆不產生事件，永不減少。
    play_seq: AtomicU64,
    no_next_play_seq: AtomicU64,
    /// 目前已預載軌是否仍在等待第一次本機 Play/Toggle（Prepare 後置為 true，首次播放後歸 false）。
    awaiting_first_play: AtomicBool,
    /// 下一首 token（快照用）
    next_token: AtomicU64,
    /// 清除 epoch（前端用來檢測引擎自行清除下一首）
    next_clear_epoch: AtomicU64,
    /// 播放位置基準（seek 目標）
    seek_origin_ms: AtomicU64,
    /// 已播輸出 frame 累計（DSP 輸出端座標）
    played_out_frames: AtomicU64,
    /// 交界紀錄（VecDeque，達 BOUNDARY_RECORD_CAPACITY 時先淘汰最舊再插入）
    boundaries: Mutex<VecDeque<BoundaryRecord>>,
    /// 當前軌的 boundary_in_48k 起點
    current_boundary_in_start: AtomicU64,
    /// 當前軌的 boundary_out_48k 起點
    current_boundary_out_start: AtomicU64,
    /// 當前軌正規化增益（f32 bits）
    current_norm_gain_bits: AtomicU64,
    /// 共享 EQ 狀態
    pub(crate) eq_shared: Arc<EqShared>,
    /// 等化器處理器
    pub(crate) eq_processor: Mutex<Option<EqProcessor>>,
    /// 音效鏈
    pub(crate) fx_chain: Mutex<Option<FxChain>>,
    /// 速度/節奏/音高重取樣階段
    pub(crate) rate_stage: Mutex<Option<RateStage>>,
    /// TrackResampler 輸出暫存（48kHz 聲道樣本，尚未經 DSP）
    pub(crate) track_out: Mutex<VecDeque<f32>>,
    /// DSP 最終輸出佇列（立體聲交錯）
    pub(crate) dsp_out: Mutex<VecDeque<f32>>,
    /// DSP 參數快照（命令執行緒更新，音訊執行緒讀取）
    pub(crate) dsp_params: Mutex<Option<FxParams>>,
    /// 交叉淡化毫秒
    crossfade_ms: AtomicU64,
    /// 峰值正規化開關
    normalize: AtomicBool,
    /// 待播協調器（TrackChain 讀完時由此 take 下一軌）
    next_coordinator: Arc<NextCoordinator>,
    /// 串流軌直接來源（長檔/ffmpeg 串流，不經 TrackChain）
    stream_source: Mutex<Option<StreamInput>>,
    stream_pending_right: AtomicBool,
    stream_failure: Mutex<Option<(u64, u64, crate::prefetch_stream::StreamFailure)>>,
    /// 軌交接回呼：(seq, token, path, decoder)，供引擎更新 current_path/current_decoder
    pub(crate) on_track_change: Mutex<Option<Box<dyn Fn(u64, u64, &str, &str) + Send + Sync>>>,
    buffered_handoff_notification: Mutex<Option<TrackChainItem>>,
    retired_handoff_tx: Sender<RetiredHandoff>,
    retired_handoff_rx: Receiver<RetiredHandoff>,
    /// 響度補償擱架濾波器（本機歌曲路徑，EQ 後、FxChain 前）
    loudness_shelves: Mutex<LoudnessShelves>,
    /// FadeGate 淡入淡出狀態機（樣本-based，實際輸出時推進）
    pub(crate) fade_gate: Arc<Mutex<crate::models::FadeGate>>,
    /// 淡段世代計數（與 stream_gen 分離）
    fade_gen: AtomicU64,
    /// 待安裝的串流來源（已由命令執行緒背景開流完成，等候曲終/gap 耗盡安裝）
    pending_stream_source: Mutex<Option<Box<dyn Iterator<Item = f32> + Send>>>,
    /// 串流中繼資料（含 gen）
    pending_stream_meta: Mutex<Option<crate::models::StreamMeta>>,
    /// 目前有效串流安裝世代；開流交付前與 meta.gen 比對，安裝前再比對；不歸零，持續遞增
    stream_gen: AtomicU64,
    /// 串流軌歌曲身分（供 publish() 與 position_ms() 讀取，取代 TrackChain 僅讀）
    stream_song_id: AtomicI64,
    /// 串流軌總長度
    stream_duration_ms: AtomicI64,
    /// 串流軌進度基準（seek 目標）
    stream_seek_origin_ms: AtomicU64,
    /// 換軌發布版本，奇數表示新來源尚未提交。
    handoff_publication_version: AtomicU64,
    /// B1 修正：明確的活動收尾階段（獨立於 gap_remaining，消除 finite-gap 狀態歧義）：
    /// EOF 尾端排空轉場時建立，暫停/恢復不變更，新軌接手（sync_identity 發布新身分）
    /// 或 abort_handoff 明確邊界才結束。gap 只在第一次進入收尾階段的轉場初始化一次。
    handoff_ending_active: AtomicBool,
    /// B1 修正：播放狀態控制世代：set_playback_state（pause/resume/stop 等控制路徑）
    /// 每次遞增；guarded EOF 短轉場比對排水開始前捕捉的值，偵測並發 pause/stop
    /// （含 pause→resume 的 ABA）。音訊執行緒內部轉場直接寫 state mutex，不遞增此世代。
    state_gen: AtomicU64,
}

impl PlaybackCore {
    pub fn new(eq_shared: Arc<EqShared>) -> Self {
        let (retired_handoff_tx, retired_handoff_rx) = bounded(3);
        Self {
            track_chain: Arc::new(Mutex::new(None)),
            track_resampler: Arc::new(Mutex::new(None)),
            dsp_state: Arc::new(Mutex::new(None)),
            core_lock: Mutex::new(()),
            active_id: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
            playback_state: Mutex::new(PlaybackState::Finished),
            now_seq: AtomicU64::new(0),
            now_token: AtomicU64::new(0),
            play_seq: AtomicU64::new(0),
            no_next_play_seq: AtomicU64::new(0),
            awaiting_first_play: AtomicBool::new(false),
            next_token: AtomicU64::new(0),
            next_clear_epoch: AtomicU64::new(0),
            seek_origin_ms: AtomicU64::new(0),
            played_out_frames: AtomicU64::new(0),
            boundaries: Mutex::new(VecDeque::new()),
            current_boundary_in_start: AtomicU64::new(0),
            current_boundary_out_start: AtomicU64::new(0),
            current_norm_gain_bits: AtomicU64::new(1.0f32.to_bits() as u64),
            eq_shared,
            eq_processor: Mutex::new(None),
            fx_chain: Mutex::new(None),
            rate_stage: Mutex::new(None),
            track_out: Mutex::new(VecDeque::new()),
            dsp_out: Mutex::new(VecDeque::new()),
            dsp_params: Mutex::new(None),
            crossfade_ms: AtomicU64::new(0),
            normalize: AtomicBool::new(false),
            next_coordinator: Arc::new(NextCoordinator::new()),
            stream_source: Mutex::new(None),
            stream_pending_right: AtomicBool::new(false),
            stream_failure: Mutex::new(None),
            on_track_change: Mutex::new(None),
            buffered_handoff_notification: Mutex::new(None),
            retired_handoff_tx,
            retired_handoff_rx,
            loudness_shelves: Mutex::new(LoudnessShelves::new(INTERNAL_CHANNELS as usize)),
            fade_gate: Arc::new(Mutex::new(crate::models::FadeGate {
                active: None,
                base_gain: 1.0,
                fade_gen: 0,
                pending_load: None,
                pending_stream_source: None,
                pending_stream_meta: None,
                stream_gen: 0,
                stream_install_ready: None,
                done_target: None,
                gap_remaining: 0,
            })),
            fade_gen: AtomicU64::new(0),
            pending_stream_source: Mutex::new(None),
            pending_stream_meta: Mutex::new(None),
            stream_gen: AtomicU64::new(1),
            stream_song_id: AtomicI64::new(0),
            stream_duration_ms: AtomicI64::new(0),
            stream_seek_origin_ms: AtomicU64::new(0),
            handoff_publication_version: AtomicU64::new(0),
            handoff_ending_active: AtomicBool::new(false),
            state_gen: AtomicU64::new(0),
        }
    }

    /// 取得待播協調器
    pub fn next_coordinator(&self) -> Arc<NextCoordinator> {
        self.next_coordinator.clone()
    }

    /// 取得下一個代理 ID
    pub fn next_proxy_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::AcqRel)
    }

    /// 設定活躍代理 ID（原子更新）
    pub fn set_active_id(&self, id: u64) {
        self.active_id.store(id, Ordering::Release);
    }

    /// 取得活躍代理 ID
    pub fn active_id(&self) -> u64 {
        self.active_id.load(Ordering::Acquire)
    }

    /// 取得播放狀態
    pub fn playback_state(&self) -> PlaybackState {
        *self.playback_state.lock().unwrap()
    }

    /// 設定播放狀態
    pub fn set_playback_state(&self, state: PlaybackState) {
        let mut playback = self.playback_state.lock().unwrap();
        *playback = state;
        // B1 修正：控制路徑每次寫入狀態都遞增世代，供 guarded EOF 短轉場比對
        self.state_gen.fetch_add(1, Ordering::AcqRel);
    }

    /// 取得播放狀態控制世代（guarded EOF 短轉場比對用）
    pub fn state_gen(&self) -> u64 {
        self.state_gen.load(Ordering::Acquire)
    }

    /// 取得活動收尾階段旗標
    fn handoff_ending_active(&self) -> bool {
        self.handoff_ending_active.load(Ordering::Acquire)
    }

    fn set_handoff_ending_active(&self, active: bool) {
        self.handoff_ending_active.store(active, Ordering::Release);
    }

    /// B1 修正：明確的交接中止邊界（後續由 player 的 Load/Seek/Stop 接線呼叫）。
    /// 中止收尾及尚未提交的交接，讓下次曲終重新計算曲間停頓。暫停會保留交接狀態。
    /// 舊來源丟棄與佇列清空（pending_output/dsp_out/track_out）屬獨立後續工作，
    /// 這裡不清佇列、不更動 stream 世代與播放狀態。
    pub fn abort_handoff(&self) {
        let _playback = self.playback_state.lock().unwrap();
        self.state_gen.fetch_add(1, Ordering::AcqRel);
        self.set_handoff_ending_active(false);
        self.finish_handoff_publication();
        self.fade_gate.lock().unwrap().gap_remaining = 0;
    }

    /// 取得目前軌的 ReplayGain（若有）
        pub fn current_replaygain(&self) -> (Option<f32>, Option<f32>, Option<f32>, Option<f32>) {
            let chain = self.track_chain.lock().unwrap();
            if let Some(c) = chain.as_ref() {
                if c.current_index < c.tracks.len() {
                    let item = &c.tracks[c.current_index];
                    return (item.replaygain_track_gain, item.replaygain_track_peak, item.replaygain_album_gain, item.replaygain_album_peak);
                }
            }
            (None, None, None, None)
        }

    /// 取得下一首 token（快照用）
    pub fn next_token(&self) -> u64 {
        self.next_token.load(Ordering::Acquire)
    }

    /// 設定下一首 token
    pub fn set_next_token(&self, token: u64) {
        self.next_token.store(token, Ordering::Release);
    }

    /// 取得清除 epoch
    pub fn next_clear_epoch(&self) -> u64 {
        self.next_clear_epoch.load(Ordering::Acquire)
    }

    /// 遞增清除 epoch
    pub fn increment_next_clear_epoch(&self) -> u64 {
        self.next_clear_epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 取得當前軌 seq
    pub fn now_seq(&self) -> u64 {
        self.now_seq.load(Ordering::Acquire)
    }

    /// 取得當前軌 token
    pub fn now_token(&self) -> u64 {
        self.now_token.load(Ordering::Acquire)
    }

    /// 設定當前軌身分
    pub fn set_now_identity(&self, seq: u64, token: u64) {
        self.now_seq.store(seq, Ordering::Release);
        self.now_token.store(token, Ordering::Release);
    }

    /// 取得實際播放事件序號（從 0 開始；與內部 now_seq 分離）
    pub fn play_seq(&self) -> u64 {
        self.play_seq.load(Ordering::Acquire)
    }

    pub(crate) fn set_next_absence(&self, play_seq: u64, no_next: bool) {
        self.no_next_play_seq.store(if no_next { play_seq } else { 0 }, Ordering::Release);
    }

    fn next_is_intentionally_empty(&self) -> bool {
        let current = self.play_seq();
        current != 0 && self.no_next_play_seq.load(Ordering::Acquire) == current
    }

    /// 分配下一個播放事件序號（新本機播放開始時呼叫），回傳分配後的值
    pub fn increment_play_seq(&self) -> u64 {
        self.play_seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 標記目前預載軌正在等待第一次本機 Play/Toggle，不立即產生播放事件
    pub fn mark_awaiting_first_play(&self) {
        self.awaiting_first_play.store(true, Ordering::Release);
    }

    /// 嘗試分配首次播放事件：僅在等待中時遞增並歸零等待旗標，回傳是否已分配
    pub fn try_allocate_first_play(&self) -> bool {
        if self.awaiting_first_play.swap(false, Ordering::AcqRel) {
            self.increment_play_seq();
            true
        } else {
            false
        }
    }

    /// 直接載入時清除等待旗標（新軌由 Load 自身於 publish 前分配事件）
    pub fn clear_awaiting_first_play(&self) {
        self.awaiting_first_play.store(false, Ordering::Release);
    }

    /// 取得 seek 起點
    pub fn seek_origin_ms(&self) -> u64 {
        self.seek_origin_ms.load(Ordering::Acquire)
    }

    /// 設定 seek 起點
    pub fn set_seek_origin_ms(&self, ms: u64) {
        self.seek_origin_ms.store(ms, Ordering::Release);
    }

    /// 取得已播輸出 frame
    pub fn played_out_frames(&self) -> u64 {
        self.played_out_frames.load(Ordering::Acquire)
    }

    /// 增加已播輸出 frame
    pub fn add_played_out_frames(&self, frames: u64) {
        self.played_out_frames.fetch_add(frames, Ordering::AcqRel);
    }

    /// 重置已播輸出 frame
    pub fn reset_played_out_frames(&self) {
        self.played_out_frames.store(0, Ordering::Release);
    }

    /// 安裝 coordinator 取得的下一軌：記憶體軌建立 TrackChain，串流軌設 pending_stream_meta。
    /// 記憶體軌保留既有 TrackResampler，讓 sync_identity 在第一次取得新軌樣本時
    /// 依原本流程 flush/commit 舊交界並 set_track，避免在音訊回呼內重建重取樣器。
    pub fn install_next_track(&self, next_track: PreparedTrack) {
        match &next_track.data {
            TrackData::Memory(_) => {
                if let Some(item) = prepared_to_trackchain_item(&next_track) {
                    let mut chain = TrackChain::new();
                    chain.append(item);
                    chain.retain_source_slot(next_track.source_slot.clone());
                    chain.set_coordinator(self.next_coordinator.clone());
                    let old_source = self.stream_source.lock().unwrap().take();
                    let old_chain = self.track_chain.lock().unwrap().replace(chain);
                    self.stream_gen.fetch_add(1, Ordering::AcqRel);
                    let retired = RetiredHandoff::Active {
                        _source: old_source,
                        _chain: old_chain,
                        _resampler: None,
                        _rate_stage: None,
                    };
                    let _ = self.retired_handoff_tx.try_send(retired);
                }
            }
            TrackData::Stream(_) => {
                // 串流軌：設 pending_stream_meta，等候背景開流
                let meta = crate::models::StreamMeta {
                    song_id: next_track.song_id,
                    duration_ms: next_track.duration_ms,
                    rate: next_track.rate,
                    channels: next_track.channels,
                    path: next_track.path.clone(),
                    decoder: next_track.decoder.clone(),
                    position_ms: 0,
                    gen: self.stream_gen.load(Ordering::Acquire),
                };
                *self.pending_stream_meta.lock().unwrap() = Some(meta);
            }
        }
    }

    /// 建立淡入淡出段落：命令執行緒呼叫，返回新段落世代。
    /// 從_gain 線性插值到 _gain，total_samples = ms × 取樣率 × 聲道數 / 1000。
    /// ms == 0 表示立即生效（to_gain 直接鎖定）。
    pub fn start_fade(&self, target: FadeTarget, from_gain: f32, to_gain: f32, ms: i64) -> u64 {
        let total_samples = if ms <= 0 {
            0
        } else {
            (ms as u64) * INTERNAL_SAMPLE_RATE as u64 * INTERNAL_CHANNELS as u64 / 1000
        };
        let mut fg = self.fade_gate.lock().unwrap();
        let gen = self.fade_gen.fetch_add(1, Ordering::AcqRel) + 1;
        fg.active = Some(crate::models::FadeSegment {
            gen,
            target,
            from_gain,
            to_gain,
            total_samples,
            consumed_samples: 0,
        });
        fg.base_gain = from_gain;
        fg.fade_gen = gen;
        gen
    }

    /// 消費完成通知：執行對應動作並清除 done_target。由命令執行緒呼叫。
    pub fn consume_done_target(&self) -> Option<(FadeTarget, u64)> {
        let mut fg = self.fade_gate.lock().unwrap();
        fg.done_target.take()
    }

    /// 檢查 done_target 的世代是否仍有效（與 fade_gen 比對）。
    pub fn done_target_gen_valid(&self, gen: u64) -> bool {
        let fg = self.fade_gate.lock().unwrap();
        fg.done_target.as_ref().map(|(_, g)| *g == gen).unwrap_or(false)
    }

    /// 取得當前軌 boundary_in_48k 起點
    pub fn current_boundary_in_start(&self) -> u64 {
        self.current_boundary_in_start.load(Ordering::Acquire)
    }

    /// 設定當前軌 boundary_in_48k 起點
    pub fn set_current_boundary_in_start(&self, v: u64) {
        self.current_boundary_in_start.store(v, Ordering::Release);
    }

    /// 取得當前軌 boundary_out_48k 起點
    pub fn current_boundary_out_start(&self) -> u64 {
        self.current_boundary_out_start.load(Ordering::Acquire)
    }

    /// 設定當前軌 boundary_out_48k 起點
    pub fn set_current_boundary_out_start(&self, v: u64) {
        self.current_boundary_out_start.store(v, Ordering::Release);
    }

    /// 記錄交界：達 BOUNDARY_RECORD_CAPACITY 時先淘汰最舊（pop_front）再插入，
    /// 避免 Vec 無必要倍增，長度恆不超過上限。
    pub fn push_boundary(&self, record: BoundaryRecord) {
        let mut boundaries = self.boundaries.lock().unwrap();
        if boundaries.len() >= BOUNDARY_RECORD_CAPACITY {
            boundaries.pop_front();
        }
        boundaries.push_back(record);
    }

    /// 取得所有交界紀錄（舊→新順序，回傳型別與既有 callers 相同）
    pub fn boundaries(&self) -> Vec<BoundaryRecord> {
        self.boundaries.lock().unwrap().iter().cloned().collect()
    }

    /// 清除過期交界紀錄（只保留當前軌之後的）
    pub fn clear_expired_boundaries(&self, current_seq: u64) {
        self.boundaries.lock().unwrap().retain(|b| b.seq >= current_seq);
    }

    /// 取得 crossfade_ms
    pub fn crossfade_ms(&self) -> u64 {
        self.crossfade_ms.load(Ordering::Acquire)
    }

    /// 設定 crossfade_ms
    pub fn set_crossfade_ms(&self, ms: u64) {
        self.crossfade_ms.store(ms, Ordering::Release);
    }

    /// 取得 normalize
    pub fn normalize(&self) -> bool {
        self.normalize.load(Ordering::Acquire)
    }

    /// 設定 normalize
    pub fn set_normalize(&self, enabled: bool) {
        self.normalize.store(enabled, Ordering::Release);
    }

    /// 取得 FadeGate（用於淡入淡出處理）
    pub fn fade_gate(&self) -> Arc<Mutex<crate::models::FadeGate>> {
        self.fade_gate.clone()
    }

    /// 取得淡段世代
    pub fn fade_gen(&self) -> u64 {
        self.fade_gen.load(Ordering::Acquire)
    }

    /// 遞增淡段世代
    pub fn increment_fade_gen(&self) -> u64 {
        self.fade_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 取得待安裝的串流來源
    pub fn pending_stream_source(&self) -> &Mutex<Option<Box<dyn Iterator<Item = f32> + Send>>> {
        &self.pending_stream_source
    }

    /// 取得串流中繼資料
    pub fn pending_stream_meta(&self) -> &Mutex<Option<crate::models::StreamMeta>> {
        &self.pending_stream_meta
    }

    /// 取得串流世代
    pub fn stream_gen(&self) -> u64 {
        self.stream_gen.load(Ordering::Acquire)
    }

    /// 遞增串流世代
    pub fn increment_stream_gen(&self) -> u64 {
        self.stream_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 取得串流歌曲 ID
    pub fn stream_song_id(&self) -> i64 {
        self.stream_song_id.load(Ordering::Acquire)
    }

    /// 設定串流歌曲 ID
    pub fn set_stream_song_id(&self, id: i64) {
        self.stream_song_id.store(id, Ordering::Release);
    }

    /// 取得串流總長度
    pub fn stream_duration_ms(&self) -> i64 {
        self.stream_duration_ms.load(Ordering::Acquire)
    }

    /// 設定串流總長度
    pub fn set_stream_duration_ms(&self, ms: i64) {
        self.stream_duration_ms.store(ms, Ordering::Release);
    }

    /// 取得串流進度基準
    pub fn stream_seek_origin_ms(&self) -> u64 {
        self.stream_seek_origin_ms.load(Ordering::Acquire)
    }

    /// 設定串流進度基準
    pub fn set_stream_seek_origin_ms(&self, ms: u64) {
        self.stream_seek_origin_ms.store(ms, Ordering::Release);
    }

    /// 取得當前軌中繼資料快照（記憶體與長串流皆適用，song_id=0 亦有效）
    pub(crate) fn current_track_meta(&self) -> Option<TrackResamplerTrack> {
        self.track_resampler.lock().unwrap().as_ref().and_then(|r| r.current_track().cloned())
    }

    /// 以實際 EqProcessor（非副本）更新當前軌的 ReplayGain 與峰值增益。
    /// 呼吸點：直接載入/準備/重啟，以及自然交接。絕不使用副本。
    pub(crate) fn apply_eq_rg_gain(&self, track_gain: Option<f32>, track_peak: Option<f32>,
        album_gain: Option<f32>, album_peak: Option<f32>, norm_gain: f32) {
        // 模式與專輯上下文的唯一權威在 EqShared；此處取當前快照轉交 EqProcessor。
        let params = self.eq_shared.get();
        if let Ok(mut eq) = self.eq_processor.lock() {
            if let Some(p) = eq.as_mut() {
                p.set_rg_context(track_gain, track_peak, album_gain, album_peak,
                    params.rg_mode, params.volume_norm_mode, params.is_album_playback_context);
                // 一律更新 norm_gain（即使 RG/Off），之後切到 Peak 才會用到當前軌的值
                p.set_norm_gain(norm_gain);
            }
        }
    }

    /// 安裝串流來源（由命令執行緒 tick 呼叫）
    pub fn install_stream_source(&self) -> Option<Box<dyn Iterator<Item = f32> + Send>> {
        self.pending_stream_source.lock().unwrap().take()
    }

    pub fn clear_pending_stream(&self) {
        let _meta = self.pending_stream_meta.lock().unwrap().take();
        let _source = self.pending_stream_source.lock().unwrap().take();
        self.fade_gate.lock().unwrap().stream_install_ready = None;
    }

    pub fn deliver_stream_result_for_request(
        &self,
        generation: u64,
        source: Box<dyn Iterator<Item = f32> + Send>,
        meta: crate::models::StreamMeta,
    ) -> bool {
        let _lock = self.next_coordinator.submission.lock().unwrap();
        if self.next_coordinator.current_generation() != generation || meta.gen != self.stream_gen() {
            return false;
        }
        self.deliver_stream_result(source, meta);
        true
    }

    /// 交付開流完成結果：由背景執行緒呼叫，設 pending_stream_source / pending_stream_meta / stream_install_ready
    pub fn deliver_stream_result(&self, source: Box<dyn Iterator<Item = f32> + Send>, meta: crate::models::StreamMeta) {
        let mut meta_guard = self.pending_stream_meta.lock().unwrap();
        let mut source_guard = self.pending_stream_source.lock().unwrap();
        *meta_guard = Some(meta.clone());
        *source_guard = Some(source);
        drop(meta_guard);
        drop(source_guard);
        // 設 stream_install_ready，命令執行緒 tick 會消費
        let mut fg = self.fade_gate.lock().unwrap();
        fg.stream_install_ready = Some(meta.gen);
    }

    /// 取得並清除串流中繼資料
    pub fn take_stream_meta(&self) -> Option<crate::models::StreamMeta> {
        self.pending_stream_meta.lock().unwrap().take()
    }

    /// 取下一個 48kHz 雙聲道樣本（經 EQ/正規化，未經 DeviceConverter）。
    /// 播放狀態非 Playing 時回傳靜音，不啟動消費端停滯計時器。
    pub fn next(&self) -> Option<f32> {
        if self.stream_pending_right.load(Ordering::Relaxed)
            && self.stream_pending_right.swap(false, Ordering::AcqRel)
        {
            return Some(0.0);
        }
        loop {
            let mut state = self.playback_state.lock().unwrap();
            match *state {
                PlaybackState::Paused | PlaybackState::Finished => return Some(0.0),
                PlaybackState::Playing | PlaybackState::WaitingForNext => {}
            }
            let generation = self.state_gen.load(Ordering::Acquire);
            if *state == PlaybackState::WaitingForNext && self.next_is_intentionally_empty() {
                self.fade_gate.lock().unwrap().gap_remaining = 0;
                *state = PlaybackState::Finished;
                return Some(0.0);
            }
            {
                let mut gate = self.fade_gate.lock().unwrap();

                if gate.gap_remaining > 0 {
                    gate.gap_remaining -= 1;
                    let exhausted = gate.gap_remaining == 0;
                    drop(gate);
                    if exhausted {
                        *state = PlaybackState::WaitingForNext;
                        self.signal_stream_install_ready_if_available();
                    }
                    return Some(0.0);
                }
            }
            if *state == PlaybackState::WaitingForNext {
                if !self.try_install_ready_next() {
                    return Some(0.0);
                }
                *state = PlaybackState::Playing;
                drop(state);
                self.notify_buffered_handoff();
                continue;
            }
            drop(state);
            let sample = self.next_playing(generation);
            let state = self.playback_state.lock().unwrap();
            if self.state_gen.load(Ordering::Acquire) != generation {
                return Some(0.0);
            }
            let sample = match sample {
                PlaybackSample::Audio(sample) => sample,
                PlaybackSample::Pending => {
                    if *state == PlaybackState::Playing {
                        self.stream_pending_right.store(true, Ordering::Release);
                    }
                    return Some(0.0);
                }
            };
            if *state == PlaybackState::Playing {
                self.add_played_out_frames(1);
            }
            return self.apply_fade_gain_to_sample(Some(sample));
        }
    }

    /// gap 的 48kHz 雙聲道交錯樣本數：gap_ms × 48000 × 2 / 1000。
    fn gap_zero_samples(gap_ms: i64) -> u64 {
        if gap_ms <= 0 {
            0
        } else {
            (gap_ms as u64) * INTERNAL_SAMPLE_RATE as u64 * INTERNAL_CHANNELS as u64 / 1000
        }
    }

    /// 尾端排空後進入 WaitingForNext。gap 只在第一次進入收尾階段的轉場初始化一次，且
    /// 轉場輸出的第一個靜音樣本計入 gap。收尾階段已活動時（暫停/恢復後重新排水、或
    /// gap 已耗盡仍在等待下一軌）不得重啟 gap：直接非阻塞查看待播槽。gap 為 0 且
    /// 下一軌已就緒時，直接安裝記憶體軌並回報可繼續播放，避免多輸出一個樣本。
    fn begin_waiting_for_next_after_tail(&self, state: &mut PlaybackState) -> bool {
        if self.next_is_intentionally_empty() {
            *state = PlaybackState::Finished;
            return false;
        }
        // B1 修正：收尾階段已活動——gap 已於第一次轉場初始化（可能已耗盡或本就為 0），
        // 不得重新初始化；僅查看待播槽，gap 已為 0 時照樣可立即安裝下一軌。
        if self.handoff_ending_active() {
            if self.try_install_ready_next() {
                *state = PlaybackState::Playing;
                return true;
            }
            return false;
        }
        // B1 修正：建立明確的活動收尾階段（獨立於 gap；暫停/恢復不變更，
        // 直到新軌接手或 abort_handoff 才結束）。
        self.set_handoff_ending_active(true);
        let gap_ms = self.eq_shared.get().gap_ms;
        let total = Self::gap_zero_samples(gap_ms);
        if total > 0 {
            let mut fg = self.fade_gate.lock().unwrap();
            // 轉場呼叫本身即為第一個 gap 靜音，剩餘 total - 1 個由後續呼叫消耗。
            fg.gap_remaining = total - 1;
            return false;
        }
        if self.try_install_ready_next() {
            *state = PlaybackState::Playing;
            true
        } else {
            false
        }
    }

    /// 非阻塞查看待播槽：記憶體軌直接安裝並回報 true；串流軌維持原流程設定
    /// pending meta / stream_install_ready 並回報 false。
    fn install_prepared_handoff(
        &self,
        ready: PreparedHandoff,
        request_generation: u64,
    ) -> Result<(), crate::prefetch_stream::StreamFailure> {
        use crate::prefetch_stream::StreamFailure;

        let failure = ready.source.as_ref().and_then(|source| {
            (!source.try_install()).then(|| source.preinstall_failure().unwrap_or(StreamFailure::Disconnected))
        });
        if let Some(reason) = failure {
            let token = ready.identity.token;
            let _ = self.retired_handoff_tx.try_send(RetiredHandoff::Candidate {
                _prepared: ready, request_generation, token,
            });
            return Err(reason);
        }
        let PreparedHandoff { source, identity, chain, mut resampler, rate_stage } = ready;
        self.begin_handoff_publication();
        let source_generation = self.stream_gen.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        let installed = source.map(|source| StreamInput::Buffered {
            source, source_generation, token: identity.token, failure_reported: false,
        });
        let old_source = std::mem::replace(&mut *self.stream_source.lock().unwrap(), installed);
        let (old_resampler, boundary) = {
            let mut current = self.track_resampler.lock().unwrap();
            let boundary = current.as_ref().map_or(0, TrackResampler::boundary_in_48k);
            resampler.boundary_in_48k_accum = boundary;
            (current.replace(resampler), boundary)
        };
        let old_chain = self.track_chain.lock().unwrap().replace(chain);
        let old_rate = self.rate_stage.lock().unwrap().replace(rate_stage);
        self.publish_track_handoff(&identity, boundary);
        self.finish_handoff_publication();
        *self.buffered_handoff_notification.lock().unwrap() = Some(identity);
        let _ = self.retired_handoff_tx.try_send(RetiredHandoff::Active {
            _source: old_source, _chain: old_chain, _resampler: old_resampler, _rate_stage: old_rate,
        });
        Ok(())
    }

    pub(crate) fn reap_retired_handoffs(&self) {
        for retired in self.retired_handoff_rx.try_iter() {
            if let RetiredHandoff::Candidate { request_generation, token, .. } = &retired {
                self.next_coordinator.finish_next_request(
                    *request_generation, *token, crate::models::NextRequestState::Failed,
                );
            }
            drop(retired);
        }
    }

    fn notify_buffered_handoff(&self) {
        let identity = self.buffered_handoff_notification.lock().unwrap().take();
        if let Some(identity) = identity {
            if let Some(callback) = self.on_track_change.lock().unwrap().as_ref() {
                callback(identity.seq, identity.token, &identity.path, &identity.decoder);
            }
        }
    }

    fn try_install_ready_next(&self) -> bool {
        let Ok(submission) = self.next_coordinator.submission.try_lock() else { return false };
        let Ok(mut slot) = self.next_coordinator.slot.try_lock() else { return false };
        if self.retired_handoff_tx.is_full() {
            return false;
        }
        if let Some(staged) = slot.as_ref().and_then(PreparedTrack::staged_handoff) {
            let Some(prepared) = staged.try_lock().ok().and_then(|mut ready| ready.take()) else {
                return false;
            };
            slot.take();
            drop(slot);
            return self.install_prepared_handoff(
                prepared, self.next_coordinator.current_generation(),
            ).is_ok();
        }
        let next = slot.take();
        drop(slot);
        if let Some(track) = next {
            let is_memory = matches!(&track.data, TrackData::Memory(_));
            self.install_next_track(track);
            if is_memory {
                return true;
            }
        }
        drop(submission);
        self.signal_stream_install_ready_if_available();
        false
    }

    /// 串流來源已交付且世代相符時，設定 stream_install_ready 供命令執行緒消費。
    fn signal_stream_install_ready_if_available(&self) {
        let meta = self.pending_stream_meta.lock().unwrap().clone();
        let Some(meta) = meta else { return };
        if self.pending_stream_source.lock().unwrap().is_some()
            && meta.gen == self.stream_gen.load(Ordering::Acquire)
        {
            let mut fg = self.fade_gate.lock().unwrap();
            fg.stream_install_ready = Some(meta.gen);
        }
    }

    /// 從 FadeGate 取得當前 fade gain 並推進進度；段落結束時發布完成通知。
    /// 僅於實際逐樣本輸出出口呼叫一次（不在 fill() 中套用，避免重複衰減）。
    fn apply_fade_gain_to_sample(&self, s: Option<f32>) -> Option<f32> {
        match s {
            Some(s) => {
                if let Ok(mut fg) = self.fade_gate.lock() {
                    let base_gain = fg.base_gain;
                    if let Some(mut seg) = fg.active.take() {
                        seg.consumed_samples += 1;
                        let t = if seg.total_samples == 0 {
                            1.0
                        } else {
                            (seg.consumed_samples as f64 / seg.total_samples as f64).clamp(0.0, 1.0) as f32
                        };
                        let gain = seg.from_gain + (seg.to_gain - seg.from_gain) * t;
                        if t >= 1.0 {
                            // 段落結束：鎖定 base_gain，設 done_target，等候命令執行緒執行動作
                            let to_gain = seg.to_gain;
                            let target = seg.target;
                            let gen = seg.gen;
                            fg.base_gain = to_gain;
                            fg.active = None;
                            fg.done_target = Some((target, gen));
                        } else {
                            fg.active = Some(seg);
                        }
                        Some(s * gain)
                    } else {
                        Some(s * base_gain)
                    }
                } else {
                    Some(s)
                }
            }
            None => None,
        }
    }

    /// Playing 狀態下的取樣：串流軌直接來源 → 原生樣本 → 重取樣 → EQ/正規化。
    fn next_playing(&self, entry_gen: u64) -> PlaybackSample {
        // 串流軌直接來源（適用長檔與 ffmpeg 串流），需經 TrackResampler 重取樣到 48kHz
        loop {
            // 1. 先從 dsp_out 取樣本回傳（一個樣本即可回傳，不以兩幀為門檻）
            {
                let mut dsp_out = self.dsp_out.lock().unwrap();
                if let Some(s) = dsp_out.pop_front() {
                    return PlaybackSample::Audio(s);
                }
            }

            // 2. 先耗盡 TrackResampler 剩餘輸出至 track_out，不遺漏已產出樣本
            {
                let mut resampler = self.track_resampler.lock().unwrap();
                if let Some(r) = resampler.as_mut() {
                    while let Some(s) = r.pop_output() {
                        self.track_out.lock().unwrap().push_back(s);
                    }
                }
            }

            // 3. 若 track_out 夠兩個樣本，組成立體聲幀送入 apply_dsp_block
            {
                let enough = {
                    let track_out = self.track_out.lock().unwrap();
                    track_out.len() >= 2
                };
                if enough {
                    let (l, r) = {
                        let mut track_out = self.track_out.lock().unwrap();
                        (track_out.pop_front().unwrap(), track_out.pop_front().unwrap())
                    };
                    let mut tmp_out: Vec<f32> = Vec::new();
                    self.apply_dsp_block(&[l, r], &mut tmp_out);
                    self.dsp_out.lock().unwrap().extend(tmp_out);
                    continue;
                }
            }

            // 4. track_out 不足兩個樣本，拉上游（優先串流軌，然後 TrackChain 軌）
            let sample = match self.next_stream(entry_gen) {
                crate::prefetch_stream::StreamRead::Sample(sample) => Some(sample),
                crate::prefetch_stream::StreamRead::Pending => return PlaybackSample::Pending,
                crate::prefetch_stream::StreamRead::Failed(_) => None,
                crate::prefetch_stream::StreamRead::End => {
                    let mut chain = self.track_chain.lock().unwrap();
                    chain.as_mut().and_then(|c| c.next())
                }
            };
            // 若從 TrackChain 取得樣本，檢查並處理軌切換（sync_identity 會建立新 RateStage 並清空佇列）
            if sample.is_some() {
                self.sync_identity(entry_gen);
            }

            match sample {
                Some(s) => {
                    // 5. 鎖定 track_resmpler，必須存在；若為 None 不繞過重取樣，輸出靜音
                    let mut resampler = self.track_resampler.lock().unwrap();
                    let Some(r) = resampler.as_mut() else {
                        log_line("長檔串流：track_resampler 為 None，不繞過重取樣，輸出靜音");
                        return PlaybackSample::Audio(0.0);
                    };
                    // 6. 呼叫 process；Some(s) 追加至 track_out，None 表示累積中，繼續拉取上游
                    match r.process(s) {
                        Some(out_s) => {
                            self.track_out.lock().unwrap().push_back(out_s);
                        }
                        None => {
                            // 累積中，繼續拉取上游
                        }
                    }
                }
                None => {
                    // 上游結束：先 flush_tail 將 TrackResampler 尾端移至 pending_output（僅一次）
                    {
                        let mut resampler = self.track_resampler.lock().unwrap();
                        resampler.as_mut().map(|r| r.flush_tail());
                    }
                    // 將 pending_output 舊軌尾端樣本經 EQ 後餵入舊 RateStage（flush=false）
                    // 再以空輸入單次 flush=true 終結 WSOLA/Resampler 殘餘，輸出進 dsp_out
                    {
                        let mut old_rs = self.rate_stage.lock().unwrap();
                        if let Some(mut rs) = old_rs.take() {
                            let fx_params = self.dsp_params.lock().unwrap().clone().unwrap_or_default();
                            let tail_samples = {
                                let mut resampler = self.track_resampler.lock().unwrap();
                                let r = resampler.as_mut().unwrap();
                                std::mem::take(&mut r.pending_output)
                            };
                            if !tail_samples.is_empty() {
                                let mut eq_processed: Vec<f32> = Vec::with_capacity(tail_samples.len());
                                {
                                    let mut eq = self.eq_processor.lock().unwrap();
                                    if let Some(p) = eq.as_mut() {
                                        for chunk in tail_samples.chunks(2) {
                                            if chunk.len() < 2 { continue; }
                                            eq_processed.push(p.process(chunk[0]));
                                            eq_processed.push(p.process(chunk[1]));
                                        }
                                    } else {
                                        eq_processed.extend_from_slice(&tail_samples);
                                    }
                                }
                                let mut rs_out: Vec<f32> = Vec::new();
                                rs.run(&eq_processed, &fx_params, false, &mut rs_out);
                                rs.run(&[], &fx_params, true, &mut rs_out);
                                // 尾段只經後段 DSP（FxChain→正規化→限幅），不重複 EQ/RateStage
                                for chunk in rs_out.chunks(2) {
                                    if chunk.len() < 2 { continue; }
                                    let frame: [f32; 2] = [chunk[0], chunk[1]];
                                    let mut tmp_out: Vec<f32> = Vec::new();
                                    self.apply_dsp_block_post(&frame, &mut tmp_out);
                                    self.dsp_out.lock().unwrap().extend(tmp_out);
                                }
                            }
                        }
                    }
                    // 若兩個佇列皆空，表示舊軌尾端已完全排空，才進入 WaitingForNext。
                    let dsp_out_empty = self.dsp_out.lock().unwrap().is_empty();
                    let track_out_empty = self.track_out.lock().unwrap().is_empty();
                    if dsp_out_empty && track_out_empty {
                        // B1 修正：guarded 短轉場——只在「先前狀態仍是 Playing 且控制世代
                        // 未變」時寫入 WaitingForNext，不覆寫並發的 pause/stop（含
                        // pause→resume 的 ABA）。被攔下時不寫入狀態、不初始化 gap、
                        // 不建立收尾階段，輸出靜音，交由後續取樣重新判定。
                        let mut state = self.playback_state.lock().unwrap();
                        if *state != PlaybackState::Playing
                            || self.state_gen.load(Ordering::Acquire) != entry_gen
                        {
                            return PlaybackSample::Audio(0.0);
                        }
                        *state = PlaybackState::WaitingForNext;
                        if self.begin_waiting_for_next_after_tail(&mut state) {
                            drop(state);
                            self.notify_buffered_handoff();
                            // gap 為 0 且記憶體下一軌已就緒：直接繼續播放，不輸出多餘靜音。
                            continue;
                        }
                        return PlaybackSample::Audio(0.0);
                    }
                    continue;
                }
            }
        }
    }

    fn next_stream(&self, entry_gen: u64) -> crate::prefetch_stream::StreamRead {
        use crate::prefetch_stream::StreamRead;

        if entry_gen != self.state_gen() {
            return StreamRead::Pending;
        }
        let Ok(mut input) = self.stream_source.try_lock() else {
            return StreamRead::Pending;
        };
        match input.as_mut() {
            Some(StreamInput::Legacy(source)) => source.next().map_or(StreamRead::End, StreamRead::Sample),
            Some(StreamInput::Buffered { source, source_generation, token, failure_reported }) => {
                match source.poll_playing_sample(entry_gen, Instant::now) {
                    StreamRead::Failed(_) if *failure_reported => StreamRead::End,
                    StreamRead::Failed(reason) => {
                        let Ok(mut failure) = self.stream_failure.try_lock() else {
                            return StreamRead::Pending;
                        };
                        *failure = Some((*source_generation, *token, reason));
                        *failure_reported = true;
                        StreamRead::Failed(reason)
                    }
                    status => status,
                }
            }
            None => StreamRead::End,
        }
    }

    /// 設定既有直接串流來源。
    pub fn set_stream_source(&self, src: Option<Box<dyn Iterator<Item = f32> + Send>>) {
        *self.stream_source.lock().unwrap() = src.map(StreamInput::Legacy);
    }

    pub(crate) fn install_buffered_stream(
        &self,
        source: crate::prefetch_stream::PreparedStream,
        source_generation: u64,
        token: u64,
    ) -> Result<(), crate::prefetch_stream::StreamFailure> {
        if !source.try_install() {
            return Err(source.preinstall_failure().unwrap_or(crate::prefetch_stream::StreamFailure::Disconnected));
        }
        *self.stream_source.lock().unwrap() = Some(StreamInput::Buffered {
            source,
            source_generation,
            token,
            failure_reported: false,
        });
        Ok(())
    }

    pub(crate) fn take_stream_failure(&self) -> Option<(u64, u64, crate::prefetch_stream::StreamFailure)> {
        self.stream_failure.lock().unwrap().take()
    }

    /// 檢查 TrackChain 目前軌的身分，若與 now_seq/now_token 不同則發布新身分。
    /// 軌交接發生在 TrackChain 讀完目前軌並從待播槽 take 新軌時。
    ///
    /// 順序：flush_tail 舊軌尾端 → commit_boundary 寫入邊界 → set_track 切換重取樣器。
    /// 兩把鎖不同時持有，避免鎖序倒置。
    fn sync_identity(&self, entry_generation: u64) {
        let state = self.playback_state.lock().unwrap();
        if *state != PlaybackState::Playing || self.state_gen.load(Ordering::Acquire) != entry_generation {
            return;
        }
        let item = {
            let chain = self.track_chain.lock().unwrap();
            chain.as_ref().and_then(|c| c.current()).cloned()
        };

        let Some(item) = item else { return };

        let (seq, token) = (item.seq, item.token);
        if seq == self.now_seq() && token == self.now_token() {
            return;
        }

        self.begin_handoff_publication();
        let boundary_in_48k = {
            let mut resampler = self.track_resampler.lock().unwrap();
            let r = resampler.get_or_insert_with(|| TrackResampler::new());
            r.commit_boundary();
            r.set_track(Some(item.clone()));
            r.boundary_in_48k()
        };

        // Install new RateStage
        {
            let mut rs = self.rate_stage.lock().unwrap();
            *rs = Some(RateStage::new(INTERNAL_CHANNELS as usize, INTERNAL_SAMPLE_RATE));
        }

        self.publish_track_handoff(&item, boundary_in_48k);

        self.finish_handoff_publication();
        drop(state);

        if let Some(cb) = self.on_track_change.lock().unwrap().as_ref() {
            cb(seq, token, &item.path, &item.decoder);
        }
        log_line(&format!("TRACK_HANDOFF seq={} token={} song_id={}", seq, token, item.song_id));
    }

    fn publish_track_handoff(&self, item: &TrackChainItem, boundary_in_48k: u64) {
        let (seq, token) = (item.seq, item.token);
        // Apply new gains to existing EqProcessor, preserving state
        self.apply_eq_rg_gain(
            item.replaygain_track_gain,
            item.replaygain_track_peak,
            item.replaygain_album_gain,
            item.replaygain_album_peak,
            item.norm_gain,
        );

        // Reset frames and origins for the new source
        self.reset_played_out_frames();
        let start_ms = native_frame_to_ms(item.leading_silence_samples, item.sample_rate);
        self.set_seek_origin_ms(start_ms);
        self.set_stream_seek_origin_ms(start_ms);

        // Update identities and counters
        self.set_current_boundary_in_start(boundary_in_48k);
        self.set_now_identity(seq, token);
        self.set_handoff_ending_active(false);
        self.increment_play_seq();

        // Update atomic fields and IDs
        self.current_norm_gain_bits.store(item.norm_gain.to_bits() as u64, Ordering::Release);
        self.set_stream_song_id(item.song_id);
        self.set_stream_duration_ms(item.duration_ms);

        // Push boundary and callbacks using item fields directly
        self.push_boundary(BoundaryRecord {
            boundary_in_48k,
            boundary_out_48k: self.current_boundary_out_start(),
            seq,
            token,
            song_id: item.song_id,
            norm_gain: item.norm_gain,
            duration_ms: item.duration_ms,
            native_sample_rate: item.sample_rate,
            native_channels: item.channels,
        });
    }

    fn begin_handoff_publication(&self) {
        self.handoff_publication_version.fetch_or(1, Ordering::AcqRel);
    }

    fn finish_handoff_publication(&self) {
        let _ = self.handoff_publication_version.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |version| {
                if version & 1 == 0 {
                    None
                } else {
                    Some(version.wrapping_add(1))
                }
            },
        );
    }

    pub(crate) fn read_handoff_snapshot<T>(&self, read: impl FnOnce() -> T) -> Option<T> {
        let version = self.handoff_publication_version.load(Ordering::Acquire);
        if version & 1 != 0 {
            return None;
        }
        let snapshot = read();
        std::sync::atomic::fence(Ordering::Acquire);
        if self.handoff_publication_version.load(Ordering::Relaxed) == version {
            Some(snapshot)
        } else {
            None
        }
    }


    /// 套用 EQ、音效鏈與峰值正規化（48kHz 單一樣本）。
    fn apply_dsp(&self, sample: f32) -> f32 {
        // 先套用 EQ
        let mut eq = self.eq_processor.lock().unwrap();
        let mut s = match eq.as_mut() {
            Some(p) => p.process(sample),
            None => sample,
        };
        drop(eq);

        // 再套用音效鏈（殘響、節奏、音高、回音等）
        let mut fx = self.fx_chain.lock().unwrap();
        if let Some(mut p) = fx.as_mut() {
            // FxChain::frame 期望一幀（所有聲道），這裡以單聲道幀處理
            let mut frame = [s];
            p.frame(&mut frame);
            s = frame[0];
        }
        drop(fx);

        if self.normalize() {
            let norm = f32::from_bits(self.current_norm_gain_bits.load(Ordering::Acquire) as u32);
            s = s * norm.clamp(0.0, 8.0);
        }
        s.clamp(-1.0, 1.0)
    }

    /// 套用 DSP 至立體聲幀：EQ → 速度/節奏/音高 (RateStage) → 響度補償 → FxChain → 正規化 → 限幅。
    /// 輸入一個立體聲幀 `[L, R]`，輸出零到多個處理後樣本（交錯立體聲）追加至 `out`。
    fn apply_dsp_block(&self, frame: &[f32; 2], out: &mut Vec<f32>) {
        // 取得參數快照（命令執行緒更新，音訊執行緒讀取；快照後立即釋放鎖）
        let fx_params = {
            let p = self.dsp_params.lock().unwrap();
            p.clone().unwrap_or_default()
        };

        // EQ（每聲道獨立）
        let (eq_l, eq_r) = {
            let mut eq = self.eq_processor.lock().unwrap();
            match eq.as_mut() {
                Some(p) => (p.process(frame[0]), p.process(frame[1])),
                None => (frame[0], frame[1]),
            }
        };

        // 速度/節奏/音高 (RateStage)，正常處理時 flush=false
        let mut tmp_out: Vec<f32> = Vec::new();
        {
            let mut rate = self.rate_stage.lock().unwrap();
            if let Some(mut rs) = rate.as_mut() {
                rs.run(&[eq_l, eq_r], &fx_params, false, &mut tmp_out);
            } else {
                tmp_out.push(eq_l);
                tmp_out.push(eq_r);
            }
        }

        // 響度補償：在 RateStage 後、FxChain 前套用
        {
            let mut shelves = self.loudness_shelves.lock().unwrap();
            shelves.maybe_retune(&self.eq_shared, INTERNAL_SAMPLE_RATE);
        }

        // 對每個輸出立體聲幀套用響度補償 → FxChain → 正規化 → 限幅
        let mut frames_out: Vec<f32> = Vec::new();
        for chunk in tmp_out.chunks(2) {
            if chunk.len() < 2 {
                continue; // 不完整的幀，捨棄
            }
            let mut l = chunk[0];
            let mut r = chunk[1];

            // 響度補償擱架
            {
                let mut shelves = self.loudness_shelves.lock().unwrap();
                (l, r) = shelves.process(l, r);
            }

            // FxChain 音效（立體聲幀）
            {
                let mut fx = self.fx_chain.lock().unwrap();
                if let Some(mut p) = fx.as_mut() {
                    p.set(&fx_params);
                    let mut f = [l, r];
                    p.frame(&mut f);
                    l = f[0];
                    r = f[1];
                }
            }

            // 正規化
            if self.normalize() {
                let norm = f32::from_bits(self.current_norm_gain_bits.load(Ordering::Acquire) as u32);
                let g = norm.clamp(0.0, 8.0);
                l = l * g;
                r = r * g;
            }

            // 限幅
            l = l.clamp(-1.0, 1.0);
            r = r.clamp(-1.0, 1.0);

            frames_out.push(l);
            frames_out.push(r);
        }
        out.extend_from_slice(&frames_out);
    }

    /// 套用後段 DSP 至立體聲幀：響度補償 → FxChain → 正規化 → 限幅（不重複 EQ 與 RateStage）。
    /// 用於 tail 排空與 EOF 終結：輸入已經過 EQ 與 RateStage 的立體聲幀 `[L, R]`。
    fn apply_dsp_block_post(&self, frame: &[f32; 2], out: &mut Vec<f32>) {
        // 取得參數快照
        let fx_params = {
            let p = self.dsp_params.lock().unwrap();
            p.clone().unwrap_or_default()
        };

        let mut l = frame[0];
        let mut r = frame[1];

        // 響度補償：在 FxChain 前套用
        {
            let mut shelves = self.loudness_shelves.lock().unwrap();
            shelves.maybe_retune(&self.eq_shared, INTERNAL_SAMPLE_RATE);
            (l, r) = shelves.process(l, r);
        }

        // FxChain 音效（立體聲幀）
        {
            let mut fx = self.fx_chain.lock().unwrap();
            if let Some(mut p) = fx.as_mut() {
                p.set(&fx_params);
                let mut f = [l, r];
                p.frame(&mut f);
                l = f[0];
                r = f[1];
            }
        }

        // 正規化
        if self.normalize() {
            let norm = f32::from_bits(self.current_norm_gain_bits.load(Ordering::Acquire) as u32);
            let g = norm.clamp(0.0, 8.0);
            l = l * g;
            r = r * g;
        }

        // 限幅
        l = l.clamp(-1.0, 1.0);
        r = r.clamp(-1.0, 1.0);

        out.push(l);
        out.push(r);
    }
}

/// 響度補償擱架濾波器狀態（每聲道一組，用於 PlaybackCore 本機路徑）
#[derive(Clone)]
struct LoudnessShelves {
    bass: Vec<crate::dsp::Bq>,
    treble: Vec<crate::dsp::Bq>,
    seen: u64,
    enabled: bool,
}

impl LoudnessShelves {
    fn new(channels: usize) -> Self {
        Self {
            bass: vec![crate::dsp::Bq::default(); channels],
            treble: vec![crate::dsp::Bq::default(); channels],
            seen: 0,
            enabled: false,
        }
    }
    fn maybe_retune(&mut self, shared: &Arc<crate::eq::EqShared>, sample_rate: u32) {
        let v = shared.version();
        if v == self.seen {
            return;
        }
        self.seen = v;
        let params = shared.get();
        self.enabled = params.loudness_enabled;
        if !self.enabled {
            return;
        }
        let sr = sample_rate.max(1) as f32;
        for b in self.bass.iter_mut() {
            b.retune(crate::dsp::Bq::lowshelf(sr, params.loudness_bass_hz, params.loudness_bass_db));
        }
        for b in self.treble.iter_mut() {
            b.retune(crate::dsp::Bq::highshelf(sr, params.loudness_treble_hz, params.loudness_treble_db));
        }
    }
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if !self.enabled {
            return (l, r);
        }
        let l = self.bass[0].run(l);
        let l = self.treble[0].run(l);
        let r = self.bass[1].run(r);
        let r = self.treble[1].run(r);
        (l, r)
    }
}

/// DSP 狀態快照（用於換裝置時移交）
#[derive(Debug, Clone)]
pub struct DspStateSnapshot {
    /// EQ 濾波器狀態（每聲道一組 Biquad 陣列的 z1, z2）
    pub eq_states: Vec<Vec<(f32, f32)>>,
    /// 殘響緩衝
    pub reverb_state: Option<ReverbState>,
    /// 節奏緩衝
    pub wsola_state: Option<WsolaState>,
    /// 限幅/增益平滑狀態
    pub gain_now: f32,
    /// 音效參數
    pub fx_params: FxParams,
}

#[derive(Debug, Clone)]
pub struct ReverbState {
    pub combs: Vec<Vec<CombState>>,
    pub allpasses: Vec<Vec<AllpassState>>,
}

#[derive(Debug, Clone)]
pub struct CombState {
    pub buf: Vec<f32>,
    pub idx: usize,
    pub store: f32,
}

#[derive(Debug, Clone)]
pub struct AllpassState {
    pub buf: Vec<f32>,
    pub idx: usize,
}

#[derive(Debug, Clone)]
pub struct WsolaState {
    pub input: Vec<f32>,
    pub tail: Vec<f32>,
    pub have_tail: bool,
    pub skip_fract: f64,
    pub seq: usize,
    pub ovl: usize,
    pub seek: usize,
    pub ch: usize,
}

#[derive(Debug, Clone)]
pub struct FxChainState {
    pub bass_states: Vec<BqState>,
    pub voice_lp_state: BqState,
    pub enh_hp_states: Vec<BqState>,
    pub chorus_states: Vec<DelayLineState>,
    pub chorus_phase: f32,
    pub flanger_states: Vec<DelayLineState>,
    pub flanger_phase: f32,
    pub echo_states: Vec<DelayLineState>,
    pub reverb_state: Option<ReverbState>,
    pub sr_u: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct BqState {
    pub z1: f32,
    pub z2: f32,
}

#[derive(Debug, Clone)]
pub struct DelayLineState {
    pub buf: Vec<f32>,
    pub idx: usize,
}

/// TrackChain：串接多軌，內部各軌保留原生取樣率與聲道，依序無縫接上。
pub struct TrackChain {
    tracks: Vec<TrackChainItem>,
    _source_slot: Option<Arc<crate::player::PcmCaptureSlot>>,
    current_index: usize,
    finished: bool,
    /// 待播協調器：讀完目前軌且無預載下一軌時，由此 take() 下一軌（唯一交接點）
    coordinator: Option<Arc<NextCoordinator>>,
}

#[derive(Clone, Debug)]
pub(crate) struct TrackChainItem {
    /// 原生樣本
    pub(crate) samples: Arc<Vec<f32>>,
    /// 原生取樣率
    pub(crate) sample_rate: u32,
    /// 原生聲道
    pub(crate) channels: u16,
    /// 時長
    pub(crate) duration_ms: i64,
    /// 歌曲 ID
    pub(crate) song_id: i64,
    /// 正規化增益
    pub(crate) norm_gain: f32,
    /// ReplayGain 曲目增益
    pub(crate) replaygain_track_gain: Option<f32>,
    pub(crate) replaygain_track_peak: Option<f32>,
    pub(crate) replaygain_album_gain: Option<f32>,
    pub(crate) replaygain_album_peak: Option<f32>,
    /// token
    pub(crate) token: u64,
    /// 播放序號
    pub(crate) seq: u64,
    /// 原生 frame 位置（用於邊界計算）
    pub(crate) native_frame_pos: u64,
    /// 檔案路徑（用於 seek 重建，交接後更新）
    pub(crate) path: String,
    /// 解碼後端（用於 seek 重建，交接後更新）
    pub(crate) decoder: String,
    /// 開頭靜音樣本數（silence_removal 開啟時）
    pub(crate) leading_silence_samples: usize,
    /// 結尾靜音樣本數（silence_removal 開啟時）
    pub(crate) trailing_silence_samples: usize,
    pub(crate) effective_end_frame: Option<usize>,
}

pub(crate) fn native_frame_at_ms(milliseconds: i64, sample_rate: u32) -> usize {
    let frame = (milliseconds.max(0) as u64).saturating_mul(u64::from(sample_rate)) / 1000;
    usize::try_from(frame).unwrap_or(usize::MAX)
}

pub(crate) fn native_frame_to_ms(frame: usize, sample_rate: u32) -> u64 {
    (frame as u64).saturating_mul(1000) / u64::from(sample_rate.max(1))
}

impl TrackChain {
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            current_index: 0,
            finished: false,
            coordinator: None,
            _source_slot: None,
        }
    }

    pub(crate) fn retain_source_slot(&mut self, slot: Option<Arc<crate::player::PcmCaptureSlot>>) {
        self._source_slot = slot;
    }

    /// 掛上待播協調器（唯一交接點的來源）
    pub fn set_coordinator(&mut self, coordinator: Arc<NextCoordinator>) {
        self.coordinator = Some(coordinator);
    }

    /// 追加軌（用於初始載入）
    pub fn append(&mut self, item: TrackChainItem) {
        self.tracks.push(item);
        self.finished = false;
    }

    /// 從待播槽取出下一軌（唯一交接點）
    pub fn take_next(&mut self, next: Option<TrackChainItem>) -> Option<BoundaryRecord> {
        // 釋放已播放完畢的軌，只保留目前軌之後的資料（軌資料生命週期：最多三份）
        // 釋放所有已播放完畢的軌，只保留最多一軌（軌資料生命週期：最多三份）
        self.tracks.clear();
        self.current_index = 0;
        self.finished = false;

        if let Some(item) = next {
            self.tracks.push(item);
            self.current_index = 0;
            self.record_boundary()
        } else {
            // 沒有下一軌
            self.finished = true;
            None
        }
    }

    fn record_boundary(&mut self) -> Option<BoundaryRecord> {
        if self.current_index == 0 || self.current_index >= self.tracks.len() {
            return None;
        }
        let prev = &self.tracks[self.current_index - 1];
        let curr = &self.tracks[self.current_index];

        // 計算 boundary_in_48k：原生 frame 位置 × 48000 / 原生取樣率
        // 這裡簡化：累積前一軌的原生 frame 數轉 48kHz
        let native_frames = prev.samples.len() as u64 / prev.channels as u64;
        let boundary_in_48k = prev.native_frame_pos + (native_frames * 48000 / prev.sample_rate as u64);

        Some(BoundaryRecord {
            boundary_in_48k,
            boundary_out_48k: 0, // 由 DSP 填入
            seq: curr.seq,
            token: curr.token,
            song_id: curr.song_id,
            norm_gain: curr.norm_gain,
            duration_ms: curr.duration_ms,
            native_sample_rate: curr.sample_rate,
            native_channels: curr.channels,
        })
    }

    /// 取得當前軌
    pub fn current(&self) -> Option<&TrackChainItem> {
        if self.current_index < self.tracks.len() {
            Some(&self.tracks[self.current_index])
        } else {
            None
        }
    }

    /// 是否結束
    pub fn is_finished(&self) -> bool {
        self.finished && (self.tracks.is_empty() || self.current_index >= self.tracks.len() - 1)
    }
}

impl Iterator for TrackChain {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if let Some(track) = self.tracks.get_mut(self.current_index) {
                if track.native_frame_pos < track.samples.len() as u64 {
                    let sample = track.samples[track.native_frame_pos as usize];
                    track.native_frame_pos += 1;
                    return Some(sample);
                } else {
                    // 當前軌結束
                    if self.current_index + 1 < self.tracks.len() {
                        // 還有預載的軌，直接接上
                        self.current_index += 1;
                        // 釋放已播放完畢的軌（軌資料生命週期）
                        if self.current_index > 0 {
                            self.tracks.drain(..self.current_index);
                            self.current_index = 0;
                        }
                        continue;
                    } else {
                        // B1：TrackChain 只負責耗盡目前記憶體軌，不再直接 take coordinator。
                        // 下一軌交接由 PlaybackCore 在尾端排空後、gap 耗盡時統一拍取。
                        self.finished = true;
                        return None;
                    }
                }
            } else {
                return None;
            }
        }
    }
}

/// 將 PreparedTrack 轉成 TrackChainItem。僅記憶體軌可進入 TrackChain，
/// 串流軌由引擎直接以 DspSource 處理，不經 TrackChain。
fn prepared_to_trackchain_item(prepared: &PreparedTrack) -> Option<TrackChainItem> {
    let TrackData::Memory(samples) = &prepared.data else { return None };
    Some(trackchain_item(prepared, samples.clone()))
}

fn trackchain_item(prepared: &PreparedTrack, samples: Arc<Vec<f32>>) -> TrackChainItem {
    let effective_end_frame = (prepared.trailing_silence_samples > 0).then(|| {
        prepared.leading_silence_samples
            .saturating_add(samples.len() / usize::from(prepared.channels.max(1)))
    });
    TrackChainItem {
        samples,
        sample_rate: prepared.rate,
        channels: prepared.channels,
        duration_ms: prepared.duration_ms,
        song_id: prepared.song_id,
        norm_gain: prepared.norm_gain,
        replaygain_track_gain: prepared.replaygain_track_gain,
        replaygain_track_peak: prepared.replaygain_track_peak,
        replaygain_album_gain: prepared.replaygain_album_gain,
        replaygain_album_peak: prepared.replaygain_album_peak,
        token: prepared.token,
        seq: prepared.seq,
        native_frame_pos: 0,
        path: prepared.path.clone(),
        decoder: prepared.decoder.clone(),
        leading_silence_samples: prepared.leading_silence_samples,
        trailing_silence_samples: prepared.trailing_silence_samples,
        effective_end_frame,
    }
}

/// TrackResampler：把原生取樣率/聲道重取樣到 48kHz 雙聲道
pub struct TrackResampler {
    /// 當前軌的重取樣器
    resampler: Option<SincFixedIn<f32>>,
    /// 當前軌資訊
    current_track: Option<TrackResamplerTrack>,
    /// 輸入緩衝（交錯的原生樣本，累積到 BLOCK_FRAMES 個 native frame 再送 rubato）
    input_buffer: Vec<f32>,
    /// 緩衝內已滿的 native frame 數
    frames_in_buffer: usize,
    /// 輸出緩衝（48kHz 雙聲道）
    output_buffer: Vec<f32>,
    /// 輸出緩衝的讀取游標
    output_pos: usize,
    /// 軌切換時保留的尾端樣本（flush_tail 產出，不被 set_track 清除，確保無縫輸出）
    pending_output: Vec<f32>,
    /// 輸入 frame 計數（原生）
    input_frames: u64,
    /// 輸出 frame 計數（48kHz）
    output_frames: u64,
    /// 重取樣器延遲（輸出 frame）
    /// 邊界輸入累計
    boundary_in_48k_accum: u64,
    /// flush_tail 是否已經執行過（避免 EOF 後持續補零產出）
    tail_flushed: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct TrackResamplerTrack {
    pub(crate) leading_silence_samples: usize,
    pub(crate) effective_end_frame: Option<usize>,
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) duration_ms: i64,
    pub(crate) song_id: i64,
    pub(crate) norm_gain: f32,
    pub(crate) replaygain_track_gain: Option<f32>,
    pub(crate) replaygain_track_peak: Option<f32>,
    pub(crate) replaygain_album_gain: Option<f32>,
    pub(crate) replaygain_album_peak: Option<f32>,
    pub(crate) token: u64,
    pub(crate) seq: u64,
    pub(crate) path: String,
    pub(crate) decoder: String,
}

impl TrackResampler {
    pub fn new() -> Self {
        Self {
            resampler: None,
            current_track: None,
            input_buffer: Vec::new(),
            frames_in_buffer: 0,
            output_buffer: Vec::new(),
            output_pos: 0,
            pending_output: Vec::new(),
            input_frames: 0,
            output_frames: 0,
            boundary_in_48k_accum: 0,
            tail_flushed: false,
        }
    }

    /// 切換到新軌（由 TrackChain 呼叫）。呼叫者應在切換前先自行 `flush_tail()` 排出舊軌尾端。
    pub fn set_track(&mut self, track: Option<TrackChainItem>) {
        self.current_track = track.map(|t| TrackResamplerTrack {
            leading_silence_samples: t.leading_silence_samples,
            effective_end_frame: t.effective_end_frame,
            sample_rate: t.sample_rate,
            channels: t.channels,
            duration_ms: t.duration_ms,
            song_id: t.song_id,
            norm_gain: t.norm_gain,
            replaygain_track_gain: t.replaygain_track_gain,
            replaygain_track_peak: t.replaygain_track_peak,
            replaygain_album_gain: t.replaygain_album_gain,
            replaygain_album_peak: t.replaygain_album_peak,
            token: t.token,
            seq: t.seq,
            path: t.path,
            decoder: t.decoder,
        });

        self.resampler = None;
        self.input_buffer.clear();
        self.frames_in_buffer = 0;
        self.output_buffer.clear();
        self.output_pos = 0;
        self.input_frames = 0;
        self.output_frames = 0;
        self.tail_flushed = false;

        if let Some(ref ct) = self.current_track {
            use crate::scanner::{CHANNELS_MAX, CHANNELS_MIN, SAMPLE_RATE_MAX, SAMPLE_RATE_MIN};
            if !(SAMPLE_RATE_MIN as u32..=SAMPLE_RATE_MAX as u32).contains(&ct.sample_rate)
                || !(CHANNELS_MIN as u16..=CHANNELS_MAX as u16).contains(&ct.channels)
            {
                return;
            }
            // 建立新的重取樣器
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };

            let resampler = SincFixedIn::<f32>::new(
                INTERNAL_SAMPLE_RATE as f64 / ct.sample_rate as f64,
                2.0,
                params,
                BLOCK_FRAMES,
                INTERNAL_CHANNELS as usize,
            ).expect("Failed to create resampler");

            self.resampler = Some(resampler);
        }
    }

    /// 排出舊軌殘餘的輸入與重取樣器延遲，產出尾端資料到 output_buffer。
    /// 在換軌或鏈結束前呼叫，避免丟失最後一段不足 BLOCK_FRAMES 的資料。
    pub fn flush_tail(&mut self) {
        if self.tail_flushed {
            return;
        }
        let internal_ch = INTERNAL_CHANNELS as usize;
        let Some(resampler) = self.resampler.as_mut() else { return };
        let Some(ct) = self.current_track.as_ref() else { return };
        let ch = ct.channels as usize;
        let remaining = self.frames_in_buffer;

        // 即使 input_buffer 為空，也要排空 rubato 內部的延遲緩衝（尾端補樣）
        let block_samples: Vec<f32> = if remaining > 0 {
            self.input_buffer.drain(..).collect()
        } else {
            Vec::new()
        };
        let block: Vec<Vec<f32>> = (0..internal_ch)
            .map(|c| {
                (0..remaining)
                    .map(|i| {
                        let native_idx = i * ch + if c < ch { c } else { 0 };
                        block_samples[native_idx]
                    })
                    .collect()
            })
            .collect();
        let total_input = self.input_frames.saturating_add(remaining as u64);
        let expected_output = (u128::from(total_input) * u128::from(INTERNAL_SAMPLE_RATE)
            + u128::from(ct.sample_rate) / 2)
            / u128::from(ct.sample_rate);
        let expected_output = u64::try_from(expected_output).unwrap_or(u64::MAX);
        let mut input = (remaining > 0).then_some(block.as_slice());
        // A partial block includes the filtering delay and may need one more zero input block.
        for _ in 0..3 {
            if self.output_frames >= expected_output {
                break;
            }
            let output = match resampler.process_partial(input.take(), None) {
                Ok(output) => output,
                Err(error) => {
                    log_line(&format!("track resampler flush_tail error: {error}"));
                    break;
                }
            };
            let available = output.iter().map(Vec::len).min().unwrap_or(0);
            let needed = usize::try_from(expected_output - self.output_frames).unwrap_or(usize::MAX);
            let frames = available.min(needed);
            for frame in 0..frames {
                for channel in &output {
                    self.output_buffer.push(channel[frame]);
                }
            }
            self.output_frames += frames as u64;
        }
        self.frames_in_buffer = 0;
        self.input_frames = total_input;
        let unread = self.output_pos.min(self.output_buffer.len());
        self.pending_output.extend(self.output_buffer.drain(unread..));
        self.output_buffer.clear();
        self.output_pos = 0;
        self.tail_flushed = true;
    }

    /// 取出一個已緩衝的 48kHz 輸出樣本。
    pub fn pop_output(&mut self) -> Option<f32> {
        // 優先回傳軌切換時保留的尾端樣本
        if !self.pending_output.is_empty() {
            return Some(self.pending_output.remove(0));
        }
        if self.tail_flushed && self.output_pos >= self.output_buffer.len() {
            return None;
        }
        if self.output_pos >= self.output_buffer.len() {
            if !self.output_buffer.is_empty() {
                self.output_buffer.clear();
                self.output_pos = 0;
            }
            return None;
        }
        let s = self.output_buffer[self.output_pos];
        self.output_pos += 1;
        Some(s)
    }

    /// 處理輸入樣本，累積完整 BLOCK_FRAMES 區塊後才呼叫 rubato 重取樣，
    /// 並從 output_buffer 產出 48kHz 雙聲道樣本。
    pub fn process(&mut self, input: f32) -> Option<f32> {
        if self.resampler.is_none() {
            return None;
        }

        let ch = self.current_track.as_ref().unwrap().channels as usize;
        let internal_ch = INTERNAL_CHANNELS as usize;

        self.input_buffer.push(input);
        // 只有湊齊一個 native frame（ch 個樣本）才計入框計數
        if self.input_buffer.len() % ch != 0 {
            return self.pop_output();
        }
        self.frames_in_buffer += 1;

        // 累積到滿一個 rubato 區塊再處理；不足時回傳已緩衝的輸出（若有）
        if self.frames_in_buffer < BLOCK_FRAMES {
            return self.pop_output();
        }

        // 取出 BLOCK_FRAMES 個 native frame，轉成 rubato 非交錯格式（內建 internal_ch 聲道）
        let block_samples: Vec<f32> = self.input_buffer.drain(..).collect();
        let block: Vec<Vec<f32>> = (0..internal_ch)
            .map(|c| {
                (0..BLOCK_FRAMES).map(|i| {
                    let native_idx = i * ch + if c < ch { c } else { 0 };
                    block_samples[native_idx]
                }).collect()
            })
            .collect();
        if let Some(ref mut resampler) = self.resampler {
            match resampler.process(&block, None) {
                Ok(out) => {
                    let n = out.iter().map(Vec::len).min().unwrap_or(0);
                    for f in 0..n {
                        for c in 0..internal_ch {
                            self.output_buffer.push(out[c][f]);
                        }
                    }
                    self.output_frames += n as u64;
                }
                Err(e) => {
                    log_line(&format!("track resampler process error: {e}"));
                }
            }
        }
        self.frames_in_buffer = 0;
        self.input_frames += BLOCK_FRAMES as u64;

        self.pop_output()
    }

    /// 處理一批輸入（用於高效處理）
    pub fn process_batch(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if self.resampler.is_none() {
            return;
        }

        let ch = self.current_track.as_ref().unwrap().channels as usize;
        let frames = input.len() / ch;
        let internal_ch = INTERNAL_CHANNELS as usize;
        if let Some(ref mut resampler) = self.resampler {
            // 把交錯輸入轉成 rubato 的非交錯格式（一個 Vec per 聲道）。
            let input_chan: Vec<Vec<f32>> = (0..internal_ch)
                .map(|c| {
                    (0..frames)
                        .map(|i| {
                            let idx = i * ch + if c < ch { c } else { 0 };
                            input[idx]
                        })
                        .collect()
                })
                .collect();
            match resampler.process(&input_chan, None) {
                Ok(out) => {
                    let n = out.iter().map(Vec::len).min().unwrap_or(0);
                    for f in 0..n {
                        for c in 0..internal_ch {
                            output.push(out[c][f]);
                        }
                    }
                    self.output_frames += n as u64;
                }
                Err(_) => {
                    log_line("track resampler process_batch error");
                }
            }
        }
        self.input_frames += frames as u64;
    }

    /// 取得 boundary_in_48k（DSP 輸入端 48kHz frame 邊界）
    pub fn boundary_in_48k(&self) -> u64 {
        self.boundary_in_48k_accum + self.output_frames
    }

    /// 更新邊界累計（當軌切換時呼叫）
    pub fn commit_boundary(&mut self) {
        self.boundary_in_48k_accum += self.output_frames;
        self.output_frames = 0;
    }

    /// 取得當前軌資訊
    pub fn current_track(&self) -> Option<&TrackResamplerTrack> {
        self.current_track.as_ref()
    }

    /// pending_output 是否為空（供 sync_identity 判斷舊軌尾端是否已全部輸出）
    pub fn pending_output_is_empty(&self) -> bool {
        self.pending_output.is_empty()
    }

    /// 是否有當前軌
    pub fn has_track(&self) -> bool {
        self.current_track.is_some()
    }
}

/// DeviceConverter：把 DSP 的 48kHz 雙聲道輸出，重取樣成裝置的取樣率與雙聲道
pub struct DeviceConverter {
    resampler: Option<SincFixedIn<f32>>,
    device_sample_rate: u32,
    input_frames: u64,
    output_frames: u64,
}

impl DeviceConverter {
    pub fn new(device_sample_rate: u32) -> Self {
        let mut s = Self {
            resampler: None,
            device_sample_rate,
            input_frames: 0,
            output_frames: 0,
        };
        if device_sample_rate != INTERNAL_SAMPLE_RATE {
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            s.resampler = Some(
                SincFixedIn::<f32>::new(
                    device_sample_rate as f64 / INTERNAL_SAMPLE_RATE as f64,
                    2.0,
                    params,
                    BLOCK_FRAMES,
                    INTERNAL_CHANNELS as usize,
                ).expect("Failed to create device resampler"),
            );
        }
        s
    }

    /// 重建（換裝置時呼叫）
    pub fn rebuild(&mut self, device_sample_rate: u32) {
        if device_sample_rate != self.device_sample_rate || self.resampler.is_none() {
            self.device_sample_rate = device_sample_rate;
            if device_sample_rate != INTERNAL_SAMPLE_RATE {
                let params = SincInterpolationParameters {
                    sinc_len: 256,
                    f_cutoff: 0.95,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 256,
                    window: WindowFunction::BlackmanHarris2,
                };

                self.resampler = Some(
                    SincFixedIn::<f32>::new(
                        device_sample_rate as f64 / INTERNAL_SAMPLE_RATE as f64,
                        2.0,
                        params,
                        BLOCK_FRAMES,
                        INTERNAL_CHANNELS as usize,
                    ).expect("Failed to create device resampler")
                );
            } else {
                self.resampler = None;
            }
            self.input_frames = 0;
            self.output_frames = 0;
        }
    }

    /// 處理一批 48kHz 雙聲道輸入，產生裝置取樣率雙聲道輸出
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if self.device_sample_rate == INTERNAL_SAMPLE_RATE {
            // 不需要重取樣，直接複製
            output.extend_from_slice(input);
            self.input_frames += (input.len() / INTERNAL_CHANNELS as usize) as u64;
            self.output_frames = self.input_frames;
            return;
        }

        if let Some(ref mut resampler) = self.resampler {
            let frames = input.len() / INTERNAL_CHANNELS as usize;
            let input_chan: Vec<Vec<f32>> = (0..INTERNAL_CHANNELS as usize)
                .map(|c| (0..frames).map(|i| input[i * INTERNAL_CHANNELS as usize + c]).collect())
                .collect();
            match resampler.process(&input_chan, None) {
                Ok(out) => {
                    let n = out.first().map(|v| v.len()).unwrap_or(0);
                    for f in 0..n {
                        for c in 0..INTERNAL_CHANNELS as usize {
                            output.push(out[c][f]);
                        }
                    }
                    self.output_frames += n as u64;
                }
                Err(_) => {}
            }
            self.input_frames += frames as u64;
        }
    }

    /// 取得裝置取樣率
    pub fn device_sample_rate(&self) -> u32 {
        self.device_sample_rate
    }
}

/// 輸出代理：實作 Source，內部持有 PlaybackCore 與 DeviceConverter
pub struct OutputProxy {
    core: Arc<PlaybackCore>,
    device_converter: DeviceConverter,
    id: u64,
    /// 輸出緩衝（裝置格式樣本）
    out_buffer: Vec<f32>,
    out_pos: usize,
}

impl OutputProxy {
    pub fn new(core: Arc<PlaybackCore>, device_sample_rate: u32, id: u64) -> Self {
        Self {
            core,
            device_converter: DeviceConverter::new(device_sample_rate),
            id,
            out_buffer: Vec::new(),
            out_pos: 0,
        }
    }

    /// 取得代理 ID
    pub fn id(&self) -> u64 {
        self.id
    }

    /// 重建 DeviceConverter（換裝置時）
    pub fn rebuild_device(&mut self, device_sample_rate: u32) {
        self.device_converter.rebuild(device_sample_rate);
        self.out_buffer.clear();
        self.out_pos = 0;
    }
}

impl Iterator for OutputProxy {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let my_id = self.id;

        // 核心鎖內取樣並再次驗證 active_id：切換裝置與取樣序列化，
        // 舊 proxy 的 next() 在核心鎖內發現 ID 已過期直接回傳 None，不消耗核心。
        let _guard = self.core.core_lock.lock().unwrap();
        if self.core.active_id() != my_id {
            return None;
        }

        loop {
            if self.out_pos < self.out_buffer.len() {
                let s = self.out_buffer[self.out_pos];
                self.out_pos += 1;
                return Some(s);
            }

            // 從核心拉一批 48kHz 樣本，經 DeviceConverter 轉成裝置格式
            let capacity = BLOCK_FRAMES * INTERNAL_CHANNELS as usize;
            let mut batch: Vec<f32> = Vec::with_capacity(capacity);
            let mut filled = 0usize;
            for _ in 0..capacity {
                match self.core.next() {
                    Some(s) => {
                        batch.push(s);
                        filled += 1;
                    }
                    None => break,
                }
            }
            if filled == 0 {
                return None;
            }

            let mut out = Vec::new();
            self.device_converter.process(&batch, &mut out);
            self.out_buffer = out;
            self.out_pos = 0;
        }
    }
}

impl Source for OutputProxy {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        INTERNAL_CHANNELS
    }

    fn sample_rate(&self) -> u32 {
        self.device_converter.device_sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// 待播軌資料（準備階段產出）
#[derive(Debug, Clone)]
pub struct PreparedTrack {
    pub seq: u64,
    pub token: u64,
    pub song_id: i64,
    pub path: String,
    pub decoder: String, // "symphonia" | "ffmpeg" | "stream"
    pub rate: u32,
    pub channels: u16,
    pub duration_ms: i64,
    pub norm_gain: f32,
    pub replaygain_track_gain: Option<f32>,
    pub replaygain_track_peak: Option<f32>,
    pub replaygain_album_gain: Option<f32>,
    pub replaygain_album_peak: Option<f32>,
    pub data: TrackData,
    source_slot: Option<Arc<crate::player::PcmCaptureSlot>>,
    staged_memory: Option<Arc<Mutex<Option<PreparedHandoff>>>>,
    /// 開頭靜音樣本數（silence_removal 開啟時）
    pub leading_silence_samples: usize,
    /// 結尾靜音樣本數（silence_removal 開啟時）
    pub trailing_silence_samples: usize,
}

impl PreparedTrack {
    fn staged_handoff(&self) -> Option<&Mutex<Option<PreparedHandoff>>> {
        match &self.data {
            TrackData::Memory(_) => self.staged_memory.as_deref(),
            TrackData::Stream(wrapper) => match wrapper.as_ref() {
                StreamWrapper::Buffered(staged) => Some(staged),
                _ => None,
            },
        }
    }

    pub(crate) fn buffered_request_state(&self) -> Option<crate::models::NextRequestState> {
        use crate::models::NextRequestState;
        let TrackData::Stream(_) = &self.data else { return None };
        let staged = self.staged_handoff()?.lock().unwrap();
        let failed = staged.as_ref().and_then(|ready| ready.source.as_ref())
            .and_then(crate::prefetch_stream::PreparedStream::preinstall_failure).is_some();
        Some(if failed { NextRequestState::Failed } else { NextRequestState::Ready })
    }
}

#[derive(Debug, Clone)]
pub enum TrackData {
    Memory(Arc<Vec<f32>>),
    Stream(Arc<StreamWrapper>),
}

#[derive(Debug)]
pub enum StreamWrapper {
    Buffered(Mutex<Option<PreparedHandoff>>),
    SymStream { path: String, sample_rate: u32, channels: u16, duration_ms: i64 },
    FfmpegStream(Arc<FfmpegStreamHandle>),
    /// 延後開啟的 FFmpeg 串流（長檔），攜帶完整中繼與 handle
        FfmpegDeferred {
            path: String,
            sample_rate: u32,
            channels: u16,
            duration_ms: i64,
            seq: u64,
            token: u64,
            song_id: i64,
            norm_gain: f32,
            generation: u64,
            handle: std::sync::Mutex<Option<std::fs::File>>,
            /// 開頭靜音偏移（ms，silence_removal 開啟時）
            position_ms: i64,
        },
}

pub struct PreparedHandoff {
    source: Option<crate::prefetch_stream::PreparedStream>,
    identity: TrackChainItem,
    chain: TrackChain,
    resampler: TrackResampler,
    rate_stage: RateStage,
}

impl std::fmt::Debug for PreparedHandoff {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PreparedHandoff").finish_non_exhaustive()
    }
}

/// FfmpegStream 的 handle（用於 PreparedTrack 跨執行緒傳遞）
#[derive(Debug, Clone)]
pub struct FfmpegStreamHandle {
    pub ffmpeg_path: PathBuf,
    pub file_path: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: i64,
    pub cancel: Arc<CancelHandle>,
    pub rx: Receiver<Vec<f32>>,
}

impl FfmpegStreamHandle {
    pub fn open_with_seek(&self, start_ms: i64) -> Result<Box<dyn Iterator<Item = f32> + Send>, String> {
        // 重新建立帶 -ss 的子程序
        Err("seek not implemented".into())
    }
}

/// 共享取消控制
#[derive(Debug)]
pub struct CancelHandle {
    flag: Arc<AtomicBool>,
    child: Arc<Mutex<Option<std::process::Child>>>,
    stall_wait_limit: Duration,
}

impl CancelHandle {
    pub fn new(stall_wait_limit: Duration) -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
            stall_wait_limit,
        }
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        if let Ok(mut child_guard) = self.child.lock() {
            if let Some(mut child) = child_guard.take() {
                // SE9：kill 後等待進程結束，保留 handle 直到確認回收完成
                let kill_ok = child.kill().is_ok();
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut finished = false;
                while Instant::now() < deadline {
                    match child.try_wait() {
                        Ok(Some(_)) => { finished = true; break; }
                        Ok(None) => { std::thread::sleep(Duration::from_millis(50)); }
                        Err(_) => { finished = true; break; }
                    }
                }
                if !finished {
                    // kill 失敗或超過 10 秒，標記孤兒，繼續等待最多 30 秒
                    let orphan_deadline = Instant::now() + Duration::from_secs(30);
                    while Instant::now() < orphan_deadline {
                        match child.try_wait() {
                            Ok(Some(_)) => { finished = true; break; }
                            Ok(None) => { std::thread::sleep(Duration::from_millis(100)); }
                            Err(_) => { finished = true; break; }
                        }
                    }
                    if !finished {
                        crate::open_with::log_line(&format!(
                            "RECYCLE_ORPHAN ffmpeg child 未在 40 秒內結束 (kill_ok={kill_ok})"
                        ));
                    }
                }
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    pub fn set_child(&self, child: std::process::Child) {
        if let Ok(mut child_guard) = self.child.lock() {
            *child_guard = Some(child);
        }
    }

    pub fn take_child(&self) -> Option<std::process::Child> {
        if let Ok(mut child_guard) = self.child.lock() {
            child_guard.take()
        } else {
            None
        }
    }
}

impl crate::open_with::Cancelled for CancelHandle {
    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }
}

/// 下一首協調器
pub struct NextCoordinator {
    generation: AtomicU64,
    slot: Mutex<Option<PreparedTrack>>,
    /// 準備完成通知：攜帶 Result，讓準備失敗（壞檔、缺工具等）能通知前端略過並重排
    completion_tx: Sender<(Result<PreparedTrack, String>, u64)>,
    completion_rx: Receiver<(Result<PreparedTrack, String>, u64)>,
    pending_tx: Option<Sender<PrepareRequest>>,
    pending_rx: Receiver<PrepareRequest>,
    workers: Vec<thread::JoinHandle<()>>,
    cancel_flag: Arc<AtomicBool>,
    /// SE9：回收期間凍結新準備工作，避免回收中接受新請求導致 handle 競爭
    submission: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    watchdog_slot: Mutex<Option<std::sync::Weak<crate::player::PcmCaptureSlot>>>,
    preparing_frozen: AtomicBool,
    next_request_result: std::sync::Mutex<Option<crate::models::NextRequestResult>>,
    next_capacity_epoch: AtomicU64,
    #[cfg(test)]
    test_source_pool: Arc<AtomicU64>,
    #[cfg(test)]
    test_source_epoch: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
struct PrepareRequest {
    path: String,
    song_id: i64,
    generation: u64,
    seq: u64,
    token: u64,
    rg_track_gain: Option<f32>,
    rg_track_peak: Option<f32>,
    rg_album_gain: Option<f32>,
    rg_album_peak: Option<f32>,
    silence_removal: bool,
    cancelled: Arc<AtomicBool>,
    source_slot: Arc<crate::player::PcmCaptureSlot>,
}

impl NextCoordinator {
    pub fn new() -> Self {
        Self::with_preparer(Self::prepare_source)
    }

    fn with_preparer(
        prepare: impl Fn(&PrepareRequest) -> Result<PreparedTrack, String> + Send + Sync + 'static,
    ) -> Self {
        let (completion_tx, completion_rx) = bounded(1);
        let (pending_tx, pending_rx) = bounded(1);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let mut coordinator = Self {
            generation: AtomicU64::new(0),
            slot: Mutex::new(None),
            completion_tx,
            completion_rx,
            pending_tx: Some(pending_tx),
            pending_rx,
            workers: Vec::new(),
            cancel_flag: cancel_flag.clone(),
            submission: Arc::new(Mutex::new(None)),
            watchdog_slot: Mutex::new(None),
            preparing_frozen: AtomicBool::new(false),
            next_request_result: Mutex::new(None),
            next_capacity_epoch: AtomicU64::new(0),
            #[cfg(test)]
            test_source_pool: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            test_source_epoch: Arc::new(AtomicU64::new(0)),
        };

        let prepare = Arc::new(prepare);
        coordinator.workers = (0..crate::player::PLAYBACK_SOURCE_LIMIT)
            .map(|_| coordinator.spawn_worker(cancel_flag.clone(), prepare.clone()))
            .collect();
        coordinator
    }

    fn prepare_source(request: &PrepareRequest) -> Result<PreparedTrack, String> {
        prepare_track_for_request(
            &request.path, request.song_id, request.seq, request.token,
            request.rg_track_gain, request.rg_track_peak, request.rg_album_gain,
            request.rg_album_peak, request.silence_removal, crate::player::PCM_MAX_BYTES,
            Some(&request.cancelled), Some(request.source_slot.clone()),
        )
    }

    fn reserve_source_slot(&self) -> Result<Arc<crate::player::PcmCaptureSlot>, crate::player::FfmpegPcmError> {
        use crate::player::PcmCaptureSlot;

        #[cfg(test)]
        let slot = PcmCaptureSlot::acquire_local(&self.test_source_pool, &self.test_source_epoch);
        #[cfg(not(test))]
        let slot = PcmCaptureSlot::acquire();
        slot.map(Arc::new)
    }

    fn spawn_worker(
        &self,
        cancel_flag: Arc<AtomicBool>,
        prepare: Arc<impl Fn(&PrepareRequest) -> Result<PreparedTrack, String> + Send + Sync + 'static>,
    ) -> thread::JoinHandle<()> {
        let pending_rx = self.pending_rx.clone();
        let completion_tx = self.completion_tx.clone();
        let submission = self.submission.clone();

        thread::spawn(move || {
            loop {
                let request = match pending_rx.recv() {
                    Ok(req) => req,
                    Err(_) => break,
                };

                if cancel_flag.load(Ordering::Acquire) || request.cancelled.load(Ordering::Acquire) {
                    continue;
                }

                // 執行準備工作：成功或失敗都透過 completion channel 通知，
                // 讓前端能略過壞檔並重排，而不會靜默丟棄。
                let mut result = prepare(&request);
                if let Ok(track) = &mut result {
                    if let TrackData::Memory(samples) = &track.data {
                        track.source_slot = Some(request.source_slot.clone());
                        let handoff = prepare_handoff(track, samples.clone(), None);
                        track.staged_memory = Some(Arc::new(Mutex::new(Some(handoff))));
                    }
                }
                if request.source_slot.end_native_preparation() && result.is_ok() {
                    result = Err("讀取音訊資料逾時".into());
                }
                let generation = request.generation;
                let request_cancelled = request.cancelled.clone();
                drop(request);
                let mut completion = (result, generation);
                loop {
                    let sent = {
                        let _submission = submission.lock().unwrap();
                        if cancel_flag.load(Ordering::Acquire) || request_cancelled.load(Ordering::Acquire) {
                            break;
                        }
                        completion_tx.try_send(completion)
                    };
                    match sent {
                        Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                        Err(TrySendError::Full(waiting)) => {
                            completion = waiting;
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            }
        })
    }

    pub fn set_next(
        &self,
        path: String,
        song_id: i64,
        token: u64,
        rg_track_gain: Option<f32>,
        rg_track_peak: Option<f32>,
        rg_album_gain: Option<f32>,
        rg_album_peak: Option<f32>,
        silence_removal: bool,
    ) -> u64 {
        let mut submission = self.submission.lock().unwrap();
        let current = self.current_generation();
        if self.preparing_frozen.load(Ordering::Acquire) {
            self.record_next_request(token, crate::models::NextRequestState::Rejected);
            return current;
        }

        let generation = current + 1;
        let rejected_generation = if self.discard_previous_sources(&mut submission) {
            self.generation.store(generation, Ordering::Release);
            generation
        } else {
            current
        };
        let Ok(source_slot) = self.reserve_source_slot() else {
            self.record_next_request(token, crate::models::NextRequestState::Rejected);
            return rejected_generation;
        };

        let cancelled = Arc::new(AtomicBool::new(false));
        let watchdog = Arc::downgrade(&source_slot);
        let request = PrepareRequest {
            path,
            song_id,
            generation,
            seq: 0,
            token,
            rg_track_gain,
            rg_track_peak,
            rg_album_gain,
            rg_album_peak,
            silence_removal,
            cancelled: cancelled.clone(),
            source_slot,
        };
        let sender = self.pending_tx.as_ref().expect("next request sender");
        if sender.try_send(request).is_err() {
            self.record_next_request(token, crate::models::NextRequestState::Failed);
            return rejected_generation;
        }
        *submission = Some(cancelled);
        *self.watchdog_slot.lock().unwrap() = Some(watchdog);
        self.generation.store(generation, Ordering::Release);
        self.record_next_request(token, crate::models::NextRequestState::Accepted);
        generation
    }

    fn discard_previous_sources(&self, submission: &mut Option<Arc<AtomicBool>>) -> bool {
        self.watchdog_slot.lock().unwrap().take();
        let previous = submission.take();
        if let Some(cancelled) = &previous {
            cancelled.store(true, Ordering::Release);
        }

        let ready = self.slot.lock().unwrap().take();
        let request = self.pending_rx.try_recv().ok();
        let completion = self.completion_rx.try_recv().ok();
        previous.is_some() || ready.is_some() || request.is_some() || completion.is_some()
    }

    pub fn clear_next(&self) -> u64 {
        let mut submission = self.submission.lock().unwrap();
        self.discard_previous_sources(&mut submission);
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *self.next_request_result.lock().unwrap() = None;
        generation
    }

    pub fn poll_completion(&self, current_gen: u64) -> Option<Result<PreparedTrack, String>> {
        let submission = self.submission.lock().unwrap();
        if current_gen != self.current_generation() {
            return None;
        }
        match self.completion_rx.try_recv() {
            Ok((result, gen)) => {
                if gen == current_gen {
                    self.watchdog_slot.lock().unwrap().take();
                    if let Ok(track) = &result {
                        if let Ok(mut slot) = self.slot.lock() {
                            *slot = Some(track.clone());
                        }
                    }
                    Some(result)
                } else {
                    crate::open_with::log_line(&format!("NEXT_COORD discard stale preparation gen={} current={}", gen, current_gen));
                    None
                }
            }
            Err(TryRecvError::Empty) => {
                let mut watchdog = self.watchdog_slot.lock().unwrap();
                let stalled = watchdog.as_ref().and_then(std::sync::Weak::upgrade)
                    .is_some_and(|slot| slot.native_preparation_stalled(Instant::now()));
                if !stalled {
                    return None;
                }
                if let Some(cancelled) = submission.as_ref() {
                    cancelled.store(true, Ordering::Release);
                }
                watchdog.take();
                Some(Err("讀取音訊資料逾時".into()))
            }
            Err(TryRecvError::Disconnected) => None,
        }
    }

    pub fn take_next(&self) -> Option<PreparedTrack> {
        if let Ok(mut slot) = self.slot.lock() {
            slot.take()
        } else {
            None
        }
    }

    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn has_next(&self) -> bool {
        self.slot.lock().map(|s| s.is_some()).unwrap_or(false)
    }

    /// 偷看待播槽中的下一軌（不移除），供 Next/Prev 指令判斷是否可切歌。
    pub fn peek_next(&self) -> Option<PreparedTrack> {
        self.slot.lock().ok().and_then(|s| s.clone())
    }

    /// SE9：設定回收凍結狀態。凍結時 `set_next` 會拒絕新請求。
    fn record_next_request(&self, token: u64, state: crate::models::NextRequestState) {
        *self.next_request_result.lock().unwrap() = Some(crate::models::NextRequestResult { token, state });
    }

    pub(crate) fn discard_failed_buffered(&self, generation: u64) {
        use crate::models::NextRequestState;

        let discarded = {
            let _submission = self.submission.lock().unwrap();
            if generation != self.current_generation() {
                return;
            }
            let mut slot = self.slot.lock().unwrap();
            if slot.as_ref().and_then(PreparedTrack::buffered_request_state) != Some(NextRequestState::Failed) {
                return;
            }
            slot.take()
        };
        if let Some(track) = discarded {
            self.finish_next_request(generation, track.token, NextRequestState::Failed);
        }
    }

    pub fn next_request_result(&self) -> Option<crate::models::NextRequestResult> {
        self.next_request_result.lock().unwrap().clone()
    }

    pub fn next_capacity_epoch(&self) -> u64 {
        #[cfg(test)]
        let source_epoch = self.test_source_epoch.load(Ordering::Acquire);
        #[cfg(not(test))]
        let source_epoch = crate::player::source_capacity_epoch();
        self.next_capacity_epoch.load(Ordering::Acquire).wrapping_add(source_epoch)
    }

    pub(crate) fn notify_preparation_capacity(&self) {
        self.next_capacity_epoch.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn reject_for_local_preparation(&self, token: u64) {
        let _submission = self.submission.lock().unwrap();
        self.record_next_request(token, crate::models::NextRequestState::Rejected);
    }

    pub fn finish_next_request(&self, generation: u64, token: u64, state: crate::models::NextRequestState) {
        let _submission = self.submission.lock().unwrap();
        
        if self.current_generation() != generation {
            return;
        }
        
        let mut next_request_result = self.next_request_result.lock().unwrap();
        if let Some(receipt) = next_request_result.as_mut() {
            if receipt.token == token {
                match (receipt.state, state) {
                    (crate::models::NextRequestState::Accepted, crate::models::NextRequestState::Ready)
                    | (crate::models::NextRequestState::Accepted, crate::models::NextRequestState::Failed)
                    | (crate::models::NextRequestState::Ready, crate::models::NextRequestState::Failed) => {
                        receipt.state = state;
                    }
                    _ => {}
                }
            }
        }
        
        drop(next_request_result);
    }

    pub fn set_preparing_frozen(&self, frozen: bool) {
        let _submission = self.submission.lock().unwrap();
        let was_frozen = self.preparing_frozen.swap(frozen, Ordering::AcqRel);
        if was_frozen && !frozen {
            self.next_capacity_epoch.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn is_preparing_frozen(&self) -> bool {
        self.preparing_frozen.load(Ordering::Acquire)
    }
}

impl Drop for NextCoordinator {
    fn drop(&mut self) {
        self.cancel_flag.store(true, Ordering::Release);
        if let Ok(mut request) = self.submission.lock() {
            if let Some(previous) = request.take() {
                previous.store(true, Ordering::Release);
            }
        }
        // 先關閉發送端，讓 worker 的 recv() 返回 Err 並退出
        drop(self.pending_tx.take());
        for worker in self.workers.drain(..) {
            // A decoder may still be blocked in I/O. The request owns its permit until it exits.
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
        // completion_tx 隨 NextCoordinator 一起被 drop；worker 結束後其持有的副本也會 drop，通道隨即關閉
    }
}

/// 測試一般預載入口。
#[cfg(test)]
fn prepare_track(path: &str, song_id: i64, seq: u64, token: u64, rg_track_gain: Option<f32>, rg_track_peak: Option<f32>, rg_album_gain: Option<f32>, rg_album_peak: Option<f32>, silence_removal: bool) -> Result<PreparedTrack, String> {
    prepare_track_with_pcm_limit(path, song_id, seq, token, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, silence_removal, crate::player::PCM_MAX_BYTES)
}

struct BufferedTrackFormat {
    rate: u32,
    channels: u16,
    duration_ms: i64,
    decoder: &'static str,
    leading_frames: usize,
}

fn prepare_buffered_sym_track(
    stream: crate::audio_decode::SymStream,
    file: &mut std::fs::File,
    path_hint: &std::path::Path,
    path: String,
    seq: u64,
    token: u64,
    song_id: i64,
    gain: (Option<f32>, Option<f32>, Option<f32>, Option<f32>),
    silence_removal: bool,
    cancelled: Option<&Arc<AtomicBool>>,
    reserved_slot: Option<Arc<crate::player::PcmCaptureSlot>>,
) -> Result<PreparedTrack, String> {
    let mut format = BufferedTrackFormat {
        rate: stream.sample_rate(),
        channels: stream.channels(),
        duration_ms: stream.duration_ms().unwrap_or(0),
        decoder: "stream",
        leading_frames: 0,
    };
    let (source, leading) = crate::player::prepare_buffered_sym_stream(
        stream, file, path_hint, silence_removal, BLOCK_FRAMES, cancelled, reserved_slot,
    )?;
    let source = source.ok_or_else(|| "下一首串流沒有音訊樣本".to_string())?;
    format.leading_frames = leading;
    Ok(stage_buffered_track(path, seq, token, song_id, gain, format, source))
}

fn stage_buffered_track(
    path: String,
    seq: u64,
    token: u64,
    song_id: i64,
    gain: (Option<f32>, Option<f32>, Option<f32>, Option<f32>),
    format: BufferedTrackFormat,
    source: crate::prefetch_stream::PreparedStream,
) -> PreparedTrack {
    let BufferedTrackFormat { rate, channels, duration_ms, decoder, leading_frames } = format;
    let empty = Arc::new(Vec::new());
    let mut track = PreparedTrack {
        seq, token, song_id, path, rate, channels, duration_ms,
        decoder: decoder.to_string(),
        norm_gain: 1.0,
        replaygain_track_gain: gain.0,
        replaygain_track_peak: gain.1,
        replaygain_album_gain: gain.2,
        replaygain_album_peak: gain.3,
        data: TrackData::Memory(empty.clone()),
        source_slot: None,
        staged_memory: None,
        leading_silence_samples: leading_frames,
        trailing_silence_samples: 0,
    };
    let ready = prepare_handoff(&track, empty, Some(source));
    track.data = TrackData::Stream(Arc::new(StreamWrapper::Buffered(Mutex::new(Some(ready)))));
    track
}

fn prepare_handoff(
    track: &PreparedTrack,
    samples: Arc<Vec<f32>>,
    source: Option<crate::prefetch_stream::PreparedStream>,
) -> PreparedHandoff {
    let identity = trackchain_item(track, samples);
    let mut chain = TrackChain::new();
    chain.retain_source_slot(track.source_slot.clone());
    chain.append(identity.clone());
    let mut resampler = TrackResampler::new();
    resampler.set_track(Some(identity.clone()));
    let rate_stage = RateStage::new(INTERNAL_CHANNELS as usize, INTERNAL_SAMPLE_RATE);
    PreparedHandoff { source, identity, chain, resampler, rate_stage }
}

#[cfg(test)]
fn prepare_track_with_pcm_limit(path: &str, song_id: i64, seq: u64, token: u64, rg_track_gain: Option<f32>, rg_track_peak: Option<f32>, rg_album_gain: Option<f32>, rg_album_peak: Option<f32>, silence_removal: bool, pcm_limit: usize) -> Result<PreparedTrack, String> {
    prepare_track_for_request(path, song_id, seq, token, rg_track_gain, rg_track_peak,
        rg_album_gain, rg_album_peak, silence_removal, pcm_limit, None, None)
}

fn prepare_track_for_request(path: &str, song_id: i64, seq: u64, token: u64, rg_track_gain: Option<f32>, rg_track_peak: Option<f32>, rg_album_gain: Option<f32>, rg_album_peak: Option<f32>, silence_removal: bool, pcm_limit: usize, cancelled: Option<&Arc<AtomicBool>>, reserved_slot: Option<Arc<crate::player::PcmCaptureSlot>>) -> Result<PreparedTrack, String> {
    if let Some(slot) = &reserved_slot {
        slot.begin_native_preparation(Instant::now());
    }
    if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err("待播準備已撤銷".to_string());
    }


    let path_buf = std::path::PathBuf::from(path);
    
    // 使用 handle 持有機制：開啟並驗證檔案，回傳持有的 handle
    let mut file = open_validated_file(&path_buf)?;
        let final_path = get_final_path_from_handle(&file)?;
    
        // 複製 handle 供長檔延後串流使用（若需要）
        let file_for_stream = file.try_clone().map_err(|e| format!("複製 handle 失敗: {e}"))?;
    
        let ext = path_buf
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let backend = crate::scanner::playback_backend_for(&ext);

    match backend {
            Some(crate::scanner::Backend::Symphonia) => {
                // SE7：配置前封包上限預檢，超標則拒絕，不呼叫 open()
                if let Err(reason) = precheck_packet_budget(&mut file) {
                    return Err(format!("封包上限預檢失敗：{reason}"));
                }
                // 嘗試用 symphonia 開啟（傳入 handle 與原始路徑）
                match crate::audio_decode::SymStream::open_checked(&mut file, &path_buf, 0) {
                Ok(mut stream) => {
                    stream.observe_preparation(reserved_slot.as_ref());
                    let duration_ms = stream.duration_ms().unwrap_or(0);
                    let is_long = duration_ms > 15 * 60 * 1000 || duration_ms == 0;

                    if is_long {
                        prepare_buffered_sym_track(
                            stream, &mut file, &path_buf, final_path, seq, token, song_id,
                            (rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak),
                            silence_removal, cancelled, reserved_slot,
                        )
                    } else {
                        // 短檔：整首解碼進記憶體，以 PcmCollector 封頂 PCM_MAX_BYTES（400 MB）。
                        // 率/聲道取自目前串流，供修剪前長度與交錯偏移使用；不動 FFmpeg 與直接載入路徑。
                        let sample_rate = stream.sample_rate().max(1);
                        let channels = stream.channels().max(1);
                        let mut collector =
                            crate::player::PcmCollector::with_max_bytes(pcm_limit);

                        match collector.extend_checked(cancelled.map(Arc::as_ref), || stream.next_checked()) {
                            Ok(()) => {
                                let mut samples = collector.finish();
                                let original_frames = samples.len() / usize::from(channels);
                                let duration_ms = (original_frames as u64 * 1000 / u64::from(sample_rate)) as i64;
                                let (leading_silence, trailing_silence) =
                                    crate::player::trim_silence(&mut samples, channels, silence_removal);
                                let peak = samples.iter().map(|s| s.abs()).fold(0f32, f32::max);
                                let norm_gain = if peak > 0.0001 { (0.89 / peak).clamp(0.05, 6.0) } else { 1.0 };

                                Ok(PreparedTrack {
                                    seq,
                                    token,
                                    song_id,
                                    path: final_path,
                                    decoder: "symphonia".to_string(),
                                    rate: sample_rate,
                                    channels,
                                    duration_ms,
                                    norm_gain,
                                    replaygain_track_gain: rg_track_gain,
                                    replaygain_track_peak: rg_track_peak,
                                    replaygain_album_gain: rg_album_gain,
                                    replaygain_album_peak: rg_album_peak,
                                    data: TrackData::Memory(Arc::new(samples)),
                                    source_slot: None,
                                    staged_memory: None,
                                    leading_silence_samples: leading_silence,
                                    trailing_silence_samples: trailing_silence,
                                })
                            }
                            Err(crate::player::SymPcmError::Collect(crate::player::PcmCollectError::BudgetExceeded)) => {
                                drop(collector);
                                drop(stream);
                                if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                                    return Err(crate::player::SymPcmError::Cancelled.to_string());
                                }
                                let mut file_for_stream = file_for_stream;
                                let restarted = crate::audio_decode::SymStream::open_checked(&mut file_for_stream, &path_buf, 0)
                                    .map_err(|error| error.to_string())?;
                                prepare_buffered_sym_track(
                                    restarted, &mut file_for_stream, &path_buf, final_path, seq, token, song_id,
                                    (rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak),
                                    silence_removal, cancelled, reserved_slot,
                                )
                            }
                            Err(error) => return Err(error.to_string()),
                        }
                    }
                }
                Err(error) => {
                    if !error.allows_fallback() {
                        return Err(error.to_string());
                    }
                    drop(file_for_stream);
                    prepare_with_ffmpeg(&final_path, song_id, &ext, seq, token, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, silence_removal, pcm_limit, cancelled, reserved_slot)
                }
            }
        }
        Some(crate::scanner::Backend::Ffmpeg) => {
            // ffmpeg 路徑：傳遞 final_path
            prepare_with_ffmpeg(&final_path, song_id, &ext, seq, token, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, silence_removal, pcm_limit, cancelled, reserved_slot)
        }
        None => {
            Err(format!("這個格式尚未支援: {}", ext))
        }
    }
}

fn prepare_with_ffmpeg(path: &str, song_id: i64, _ext: &str, seq: u64, token: u64, rg_track_gain: Option<f32>, rg_track_peak: Option<f32>, rg_album_gain: Option<f32>, rg_album_peak: Option<f32>, silence_removal: bool, pcm_limit: usize, cancelled: Option<&Arc<AtomicBool>>, reserved_slot: Option<Arc<crate::player::PcmCaptureSlot>>) -> Result<PreparedTrack, String> {
    if let Some(slot) = &reserved_slot {
        slot.end_native_preparation();
    }
    let ffprobe_path = crate::tools::locate("ffprobe.exe")
        .map(|(p, _, _)| p)
        .ok_or("找不到 ffprobe")?;
    // SE6：ffprobe 64 KB / 10 秒上限
    let value = crate::audio_decode::ffprobe_playback_json_for_request(&ffprobe_path, path, cancelled)?;
    if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err("待播準備已撤銷".to_string());
    }

    let streams = value["streams"].as_array().ok_or("無音訊串流")?;
    let audio_stream = streams.iter().find(|s| s["codec_type"] == "audio").ok_or("無音訊串流")?;

    let sample_rate = audio_stream["sample_rate"].as_str().unwrap_or("0").parse().unwrap_or(0);
    let channels = audio_stream["channels"].as_u64().unwrap_or(0) as u16;
    let duration = value["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok());
    let duration_ms = duration.map(|d| (d * 1000.0) as i64).unwrap_or(0);

    let is_long = duration_ms > 15 * 60 * 1000 || duration_ms == 0;

    let ffmpeg_path = crate::tools::locate("ffmpeg.exe").map(|(p, _, _)| p).ok_or("找不到 ffmpeg")?;
    let memory = if is_long {
        None
    } else {
        match crate::player::collect_ffmpeg_pcm_with_slot(
            &ffmpeg_path, path, sample_rate, channels, 0, silence_removal, pcm_limit, cancelled, reserved_slot.clone(),
        ) {
            Ok(pcm) => Some(pcm),
            Err(crate::player::FfmpegPcmError::BudgetExceeded) => None,
            Err(_) => return Err("ffmpeg 解碼失敗".into()),
        }
    };
    if memory.is_none() {
        let codec = audio_stream["codec_name"].as_str().unwrap_or("");
        let (rate, stream_channels) = if codec.starts_with("dsd") {
            ((sample_rate / 32).min(176_400), 2)
        } else {
            (sample_rate, channels)
        };
        use crate::scanner::{SAMPLE_RATE_MAX, SAMPLE_RATE_MIN};
        if !(SAMPLE_RATE_MIN as u32..=SAMPLE_RATE_MAX as u32).contains(&rate)
            || !(1..=crate::prefetch_stream::FRAME_SIZE as u16).contains(&stream_channels)
        {
            return Err("FFmpeg 音訊格式資訊無效".to_string());
        }
        let (source, leading_frames) = crate::player::buffer_ffmpeg_stream_with_slot(
            &ffmpeg_path, path, rate, stream_channels, 0, silence_removal, BLOCK_FRAMES, cancelled, reserved_slot,
        )?;
        let source = source.ok_or_else(|| "下一首 FFmpeg 串流沒有音訊樣本".to_string())?;
        let format = BufferedTrackFormat {
            rate,
            channels: stream_channels,
            duration_ms,
            decoder: "ffmpeg-stream",
            leading_frames,
        };
        Ok(stage_buffered_track(
            path.to_string(), seq, token, song_id,
            (rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak),
            format, source,
        ))
    } else {
        let (samples, leading_silence, trailing_silence) = memory.expect("checked PCM result");
        let peak = samples.iter().map(|s| s.abs()).fold(0f32, f32::max);
        let norm_gain = if peak > 0.0001 { (0.89 / peak).clamp(0.05, 6.0) } else { 1.0 };

        Ok(PreparedTrack {
                    seq,
                    token,
                    song_id,
                    path: path.to_string(),
                    decoder: "ffmpeg".to_string(),
                    rate: sample_rate,
                    channels,
                    duration_ms,
                    norm_gain,
                    replaygain_track_gain: rg_track_gain,
                    replaygain_track_peak: rg_track_peak,
                    replaygain_album_gain: rg_album_gain,
                    replaygain_album_peak: rg_album_peak,
                    data: TrackData::Memory(Arc::new(samples)),
                    source_slot: None,
                    staged_memory: None,
                    leading_silence_samples: leading_silence,
                    trailing_silence_samples: trailing_silence,
                })
    }
}

/// 回收項目：由音訊引擎的回收執行緒統一釋放子程序與執行緒資源。
pub enum RecycleItem {
    FfmpegStream(Arc<crate::audio_decode::FfmpegStream>),
    FfmpegStreamHandle(Arc<FfmpegStreamHandle>),
    Thread(thread::JoinHandle<()>),
    Child(std::process::Child),
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_buffered_source_pending_silence_keeps_progress_and_failure_type() {
        use crate::prefetch_stream::{prepared_stream, StreamFailure, FRAME_SIZE};

        let core = core_with_eq();
        install_memory_playback(&core, sine_track(48_000, 2, 0.1, 1, 10, 1));
        core.track_chain.lock().unwrap().take();
        core.set_now_identity(1, 10);
        core.set_playback_state(PlaybackState::Playing);
        let mut frame = [0.0; FRAME_SIZE];
        frame[0] = 0.25;
        frame[1] = -0.25;
        let (writer, source) = prepared_stream(2, frame).unwrap();
        assert_eq!(core.install_buffered_stream(source, core.stream_gen(), 10), Ok(()));
        let before_wait = core.played_out_frames();
        assert_eq!(core.next(), Some(0.0));
        for _ in 1..BLOCK_FRAMES {
            assert!(writer.write_frame(frame));
        }
        assert_eq!(core.next(), Some(0.0));
        assert_eq!(core.played_out_frames(), before_wait);
        core.next().unwrap();
        core.next().unwrap();
        assert_eq!(core.played_out_frames(), before_wait + 2);

        writer.fail(StreamFailure::PacketBudget);
        let failure = (0..BLOCK_FRAMES * 4).find_map(|_| {
            core.next();
            core.take_stream_failure()
        });
        assert_eq!(failure, Some((core.stream_gen(), 10, StreamFailure::PacketBudget)));
        core.next();
        assert_eq!(core.take_stream_failure(), None);
    }

    #[test]
    fn buffered_to_memory_handoff_retires_the_old_source_outside_the_audio_callback() {
        use crate::prefetch_stream::{prepared_stream, FRAME_SIZE};

        static TEST_SOURCES: AtomicU64 = AtomicU64::new(0);
        let old_slot = Arc::new(crate::player::PcmCaptureSlot::acquire_from(&TEST_SOURCES).unwrap());
        let old_reference = Arc::downgrade(&old_slot);
        let core = core_with_eq();
        core.eq_shared.update(|settings| settings.gap_ms = 0);
        let mut old_track = sine_track(48_000, 2, 0.05, 1, 10, 1);
        old_track.samples = Arc::new(Vec::new());
        install_memory_playback(&core, old_track);
        let mut frame = [0.0; FRAME_SIZE];
        frame[..2].fill(0.25);
        let (writer, mut source) = prepared_stream(2, frame).unwrap();
        source.retain_source_slot(old_slot);
        for _ in 1..BLOCK_FRAMES {
            assert!(writer.write_frame(frame));
        }
        writer.end();
        core.set_now_identity(1, 10);
        core.install_buffered_stream(source, core.stream_gen(), 10).unwrap();
        core.set_playback_state(PlaybackState::Playing);

        let mut next = prepared_memory(48_000, 2, 0.05, 2, 20, 2);
        next.data = TrackData::Memory(Arc::new(vec![-0.25; 4_800]));
        *core.next_coordinator.slot.lock().unwrap() = Some(next);
        let saw_new_audio = (0..20_000).any(|_| {
            let sample = core.next().unwrap();
            core.now_token() == 20 && sample < -0.02
        });
        assert!(saw_new_audio);
        assert!(core.stream_source.lock().unwrap().is_none());
        assert!(old_reference.upgrade().is_some());
        core.reap_retired_handoffs();
        assert!(old_reference.upgrade().is_none());
    }

    #[test]
    fn test_prepared_memory_preserves_snapshot_until_commit() {
        let core = core_with_eq();
        let mut old = sine_track(48_000, 2, 0.1, 1, 10, 1);
        old.leading_silence_samples = 96;
        install_memory_playback(&core, old);
        core.set_now_identity(1, 10);
        core.set_seek_origin_ms(175);
        core.set_playback_state(PlaybackState::Playing);
        core.install_next_track(prepared_memory(48_000, 2, 0.1, 2, 20, 2));
        core.set_playback_state(PlaybackState::Paused);
        let stopped_at_boundary = core.read_handoff_snapshot(|| {
            let committed = core.current_track_meta().unwrap();
            (core.now_token(), core.seek_origin_ms(), committed.song_id, committed.leading_silence_samples)
        });
        assert_eq!(stopped_at_boundary, Some((10, 175, 1, 96)));
    }
    #[test]
    fn test_handoff_snapshot_rejects_mixed_identity_and_progress() {
        let core = core_with_eq();
        core.set_now_identity(1, 10);
        core.set_seek_origin_ms(1000);
        let mixed = core.read_handoff_snapshot(|| {
            let old_origin = core.seek_origin_ms();
            core.begin_handoff_publication();
            core.set_now_identity(2, 20);
            core.set_seek_origin_ms(0);
            core.finish_handoff_publication();
            (core.now_token(), old_origin)
        });
        assert_eq!(mixed, None);
        assert_eq!(
            core.read_handoff_snapshot(|| (core.now_token(), core.seek_origin_ms())),
            Some((20, 0))
        );
        core.begin_handoff_publication();
        let skipped = core.read_handoff_snapshot(|| -> () { panic!("交接尚未提交") });
        assert_eq!(skipped, None);
        core.abort_handoff();
        assert_eq!(core.read_handoff_snapshot(|| ()), Some(()));
    }

    #[test]
    fn test_clear_pending_stream_discards_previously_published_source() {
        let core = core_with_eq();
        let coordinator = core.next_coordinator();
        let gen = coordinator.current_generation();
        let meta = crate::models::StreamMeta {
            song_id: 1,
            duration_ms: 1000,
            rate: 44100,
            channels: 2,
            path: "test.mp3".to_string(),
            decoder: "pcm".to_string(),
            position_ms: 0,
            gen: core.stream_gen(),
        };

        assert!(core.deliver_stream_result_for_request(gen, Box::new([0.25_f32, 0.5].into_iter()), meta.clone()));
        assert!(core.pending_stream_source.lock().unwrap().is_some());

        coordinator.clear_next();
        core.clear_pending_stream();

        assert!(core.pending_stream_meta.lock().unwrap().is_none());
        assert!(core.pending_stream_source.lock().unwrap().is_none());
        assert!(core.fade_gate.lock().unwrap().stream_install_ready.is_none());

        assert!(!core.deliver_stream_result_for_request(gen, Box::new([0.1_f32].into_iter()), meta));
    }

    #[test]
    fn test_prefetch_delivery_drops_stale_open_result() {
        let core = core_with_eq();
        let coordinator = core.next_coordinator();
        let generation = coordinator.current_generation();
        let source_gen = core.stream_gen();

        coordinator.clear_next();

        let meta = crate::models::StreamMeta {
            song_id: 1,
            duration_ms: 1000,
            rate: 44100,
            channels: 2,
            path: "test".to_string(),
            decoder: "wav".to_string(),
            position_ms: 0,
            gen: source_gen,
        };

        assert!(!core.deliver_stream_result_for_request(generation, Box::new([0.125_f32, -0.25].into_iter()), meta));
        assert!(core.pending_stream_source().lock().unwrap().is_none());
        assert!(core.pending_stream_meta().lock().unwrap().is_none());
        assert!(core.fade_gate.lock().unwrap().stream_install_ready.is_none());
    }

    use super::*;

    fn sine_track(rate: u32, ch: u16, secs: f32, seq: u64, token: u64, song_id: i64) -> TrackChainItem {
        let n = (rate as f32 * secs) as usize;
        let samples: Vec<f32> = (0..n.saturating_mul(ch as usize)).map(|i| {
            let frame = i / ch as usize;
            (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / rate as f32).sin() * 0.3
        }).collect();
        TrackChainItem {
            samples: Arc::new(samples),
            sample_rate: rate,
            channels: ch,
            duration_ms: (secs * 1000.0) as i64,
            song_id,
            norm_gain: 1.0,
            replaygain_track_gain: None,
            replaygain_track_peak: None,
            replaygain_album_gain: None,
            replaygain_album_peak: None,
            token,
            seq,
            native_frame_pos: 0,
            path: "test".into(),
            decoder: "synth".into(),
            leading_silence_samples: 0,
            trailing_silence_samples: 0,
            effective_end_frame: None,
        }
    }

    fn prepared_memory(rate: u32, ch: u16, secs: f32, seq: u64, token: u64, song_id: i64) -> PreparedTrack {
        let t = sine_track(rate, ch, secs, seq, token, song_id);
        PreparedTrack {
            seq,
            token,
            song_id,
            path: "test".into(),
            decoder: "symphonia".into(),
            rate,
            channels: ch,
            duration_ms: t.duration_ms,
            norm_gain: 1.0,
            replaygain_track_gain: None,
            replaygain_track_peak: None,
            replaygain_album_gain: None,
            replaygain_album_peak: None,
            data: TrackData::Memory(t.samples),
            source_slot: None,
            staged_memory: None,
            leading_silence_samples: 0,
            trailing_silence_samples: 0,
        }
    }

    fn core_with_eq() -> PlaybackCore {
        let core = PlaybackCore::new(Arc::new(EqShared::new()));
        core.eq_processor
            .lock()
            .unwrap()
            .replace(EqProcessor::new(core.eq_shared.clone(), INTERNAL_SAMPLE_RATE, INTERNAL_CHANNELS, 1.0));
        core
    }


    #[test]
    fn resampler_preserves_audible_tail_at_original_duration() {
        for rate in [44100_u32, 96000] {
            let frame_count = rate as usize / 10;
            let mut resampler = TrackResampler::new();
            resampler.set_track(Some(sine_track(rate, 2, 0.1, 1, 10, 1)));
            let mut samples = Vec::new();
            for frame in 0..frame_count {
                let sample = if frame + rate as usize / 200 >= frame_count { 0.5 } else { 0.0 };
                samples.extend(resampler.process(sample));
                samples.extend(resampler.process(sample));
            }
            resampler.flush_tail();
            while let Some(sample) = resampler.pop_output() {
                samples.push(sample);
            }
            assert_eq!(samples.len(), 9600, "rate {rate}");
            assert!(samples[9500..].iter().any(|sample| *sample > 0.2), "rate {rate}");
        }
    }

    #[test]
    fn test_playback_core_seamless_handoff() {
        let core = Arc::new(core_with_eq());
        core.set_playback_state(PlaybackState::Playing);
        core.eq_shared.update(|params| {
            params.enabled = true;
            params.bands[0] = 6.0;
            params.gap_ms = 0;
        });
        install_memory_playback(&core, sine_track(44100, 2, 0.1, 1, 10, 1));

        let mut second = prepared_memory(48000, 2, 0.1, 2, 20, 2);
        let samples = (0..4800).flat_map(|_| [0.25_f32, -0.25_f32]).collect();
        second.data = TrackData::Memory(Arc::new(samples));
        core.next_coordinator().slot.lock().unwrap().replace(second);

        let weak = Arc::downgrade(&core);
        let state_at_boundary = Arc::new(Mutex::new(None));
        let observed = state_at_boundary.clone();
        *core.on_track_change.lock().unwrap() = Some(Box::new(move |seq, _, _, _| {
            if seq == 2 {
                let core = weak.upgrade().unwrap();
                let eq = core.eq_processor.lock().unwrap();
                let first_filter = &eq.as_ref().unwrap().chains[0][0];
                *observed.lock().unwrap() = Some((first_filter.z1, first_filter.z2));
            }
        }));

        let mut boundary = None;
        let mut frames = Vec::new();
        for index in 0_usize..20_000 {
            let sample = core.next().expect("no end marker through handoff");
            if core.now_seq() == 2 {
                boundary.get_or_insert(index);
                frames.push(sample);
                if frames.len() == 400 {
                    break;
                }
            }
        }
        let boundary = boundary.expect("second track installed");
        assert!(boundary.abs_diff(9_600) <= 480, "handoff at {boundary} interleaved samples");
        assert_eq!(core.now_token(), 20);
        let distinct_samples = frames.chunks_exact(2).filter(|pair| pair[0] - pair[1] > 0.25).count() * 2;
        assert!(distinct_samples >= 50, "audible samples from second track: {distinct_samples}");
        let (z1, z2) = state_at_boundary.lock().unwrap().expect("EQ state at handoff");
        assert!(z1.abs() + z2.abs() > 1e-7, "old EQ filter state must persist at handoff");
    }

    #[test]
    fn superseded_completions_release_source_capacity_before_the_next_submission() {
        use crate::models::NextRequestState;

        let coordinator = NextCoordinator::new();
        let first = coordinator.reserve_source_slot().unwrap();
        let second = coordinator.reserve_source_slot().unwrap();
        let third = coordinator.reserve_source_slot().unwrap();
        let third_ref = Arc::downgrade(&third);
        let mut obsolete = prepared_memory(48_000, 2, 0.01, 1, 10, 1);
        obsolete.source_slot = Some(third);
        coordinator.completion_tx.try_send((Ok(obsolete), 0)).unwrap();
        assert!(coordinator.reserve_source_slot().is_err());
        let before = coordinator.next_capacity_epoch();
        let generation = coordinator.set_next("missing-test-track.wav".into(), 2, 20, None, None, None, None, false);
        assert_eq!(generation, 1);
        assert_eq!(coordinator.next_request_result().unwrap().state, NextRequestState::Accepted);
        assert!(third_ref.upgrade().is_none());
        assert!(coordinator.next_capacity_epoch() > before);
        coordinator.clear_next();
        drop((first, second));

        let lease = coordinator.reserve_source_slot().unwrap();
        let reference = Arc::downgrade(&lease);
        let mut completed = prepared_memory(48_000, 2, 0.01, 2, 30, 3);
        completed.source_slot = Some(lease);
        coordinator.completion_tx.try_send((Ok(completed), coordinator.current_generation())).unwrap();
        let before_clear = coordinator.next_capacity_epoch();
        coordinator.clear_next();
        assert!(coordinator.completion_rx.is_empty());
        assert!(reference.upgrade().is_none());
        assert!(coordinator.next_capacity_epoch() > before_clear);
    }

    #[test]
    fn source_slots_cover_prepared_memory_active_memory_and_background_retirement() {
        use crate::models::NextRequestState;

        let core = core_with_eq();
        let coordinator = core.next_coordinator();
        let active = coordinator.reserve_source_slot().unwrap();
        let active_ref = Arc::downgrade(&active);
        let unrelated_cleanup = coordinator.reserve_source_slot().unwrap();
        let occupied = coordinator.reserve_source_slot().unwrap();
        install_memory_playback(&core, sine_track(48_000, 2, 0.05, 1, 10, 1));
        core.track_chain.lock().unwrap().as_mut().unwrap().retain_source_slot(Some(active));
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp_wav(&dir);
        let submit = |token| coordinator.set_next(
            path.to_str().unwrap().to_string(), 2, token, None, None, None, None, false,
        );

        assert_eq!(submit(20), 0);
        assert_eq!(coordinator.next_request_result().unwrap().state, NextRequestState::Rejected);
        assert!(coordinator.pending_rx.is_empty());
        let full_epoch = coordinator.next_capacity_epoch();
        drop(occupied);
        assert_eq!(coordinator.next_capacity_epoch(), full_epoch + 1);
        let generation = submit(21);
        assert_eq!(generation, 1);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        let prepared = loop {
            if let Some(result) = coordinator.poll_completion(generation) {
                break result.unwrap();
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(matches!(&prepared.data, TrackData::Memory(_)));
        let prepared_ref = Arc::downgrade(prepared.source_slot.as_ref().unwrap());
        drop(prepared);
        assert!(coordinator.reserve_source_slot().is_err());
        assert!(core.try_install_ready_next());
        assert!(active_ref.upgrade().is_some());
        assert!(prepared_ref.upgrade().is_some());
        assert!(coordinator.reserve_source_slot().is_err());
        core.reap_retired_handoffs();
        assert!(active_ref.upgrade().is_none());
        assert!(prepared_ref.upgrade().is_some());
        assert!(coordinator.reserve_source_slot().is_ok());
        drop(unrelated_cleanup);
    }

    #[test]
    fn prepared_memory_handoff_installs_state_before_the_first_sample() {
        let core = core_with_eq();
        install_memory_playback(&core, sine_track(48_000, 2, 0.05, 1, 10, 1));
        core.set_now_identity(1, 10);
        let play_sequence = core.increment_play_seq();
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp_wav(&dir);
        let coordinator = core.next_coordinator();
        let generation = coordinator.set_next(
            path.to_string_lossy().into_owned(), 2, 20, None, None, None, None, false,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let next = loop {
            if let Some(result) = coordinator.poll_completion(generation) {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };

        {
            let ready = next.staged_handoff().unwrap().lock().unwrap();
            let ready = ready.as_ref().unwrap();
            assert!(ready.source.is_none());
            assert!(ready.resampler.resampler.is_some());
            assert_eq!(ready.identity.token, 20);
        }
        assert!(core.try_install_ready_next());
        assert_eq!(core.now_token(), 20);
        assert_eq!(core.play_seq(), play_sequence + 1);
        assert_eq!(core.current_track_meta().unwrap().song_id, 2);
        assert!(core.rate_stage.lock().unwrap().is_some());
        assert!(core.track_resampler.lock().unwrap().as_ref().unwrap().resampler.is_some());
        core.reap_retired_handoffs();
    }

    #[test]
    fn frozen_next_request_keeps_existing_generation_and_ready_track() {
        let coordinator = NextCoordinator::new();
        let previous = coordinator.current_generation();
        coordinator.slot.lock().unwrap()
            .replace(prepared_memory(48000, 2, 0.01, 1, 40, 1));
        coordinator.set_preparing_frozen(true);
        let returned = coordinator.set_next("fixture.wav".into(), 2, 50, None, None, None, None, false);
        assert_eq!(returned, previous);
        assert_eq!(coordinator.current_generation(), previous);
        assert!(coordinator.pending_rx.is_empty());
        assert_eq!(coordinator.peek_next().unwrap().token, 40);
    }

    #[test]
    fn stalled_native_preparation_reports_failure_but_holds_capacity_until_exit() {
        let (started, blocked) = bounded(1);
        let (release, waiting) = bounded(1);
        let (exited, completed) = bounded(1);
        let coordinator = NextCoordinator::with_preparer(move |request| {
            request.source_slot.begin_native_preparation(Instant::now() - Duration::from_secs(11));
            started.send(Arc::downgrade(&request.source_slot)).unwrap();
            let _ = waiting.recv();
            assert!(request.cancelled.load(Ordering::Acquire));
            exited.send(()).unwrap();
            Err("cancelled fixture".into())
        });
        let first = coordinator.reserve_source_slot().unwrap();
        let second = coordinator.reserve_source_slot().unwrap();
        let generation = coordinator.set_next("fixture.wav".into(), 2, 20, None, None, None, None, false);
        let reference = blocked.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(coordinator.poll_completion(generation), Some(Err(error)) if error == "讀取音訊資料逾時"));
        assert!(coordinator.poll_completion(generation).is_none());
        assert!(reference.upgrade().is_some());
        assert!(coordinator.reserve_source_slot().is_err());
        release.send(()).unwrap();
        completed.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while reference.upgrade().is_some() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(coordinator.reserve_source_slot().is_ok());
        drop((first, second));
    }

    #[test]
    fn newest_preparation_finishes_while_a_superseded_decode_is_still_blocked() {
        let (started, blocked) = bounded(1);
        let (release, wait) = bounded(1);
        let coordinator = NextCoordinator::with_preparer(move |request| {
            if request.token == 20 {
                started.send(()).unwrap();
                let _ = wait.recv();
                assert!(request.cancelled.load(Ordering::Acquire));
            }
            NextCoordinator::prepare_source(request)
        });

        let folder = tempfile::tempdir().unwrap();
        let path = write_temp_wav(&folder);
        let submit = |token| coordinator.set_next(
            path.to_string_lossy().into_owned(), 42, token, None, None, None, None, false,
        );
        let first = submit(20);
        blocked.recv_timeout(Duration::from_secs(2)).unwrap();
        let latest = submit(21);
        assert!(latest > first);
        let deadline = Instant::now() + Duration::from_secs(5);
        let prepared = loop {
            if let Some(result) = coordinator.poll_completion(latest) {
                break result.expect("latest valid WAV must prepare before old decode is released");
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(prepared.token, 21);
        assert!(coordinator.reserve_source_slot().is_ok());
        release.send(()).unwrap();
    }

    #[test]
    fn test_playback_core_handoff_full_completion_channel() {
        let core = core_with_eq();
        let coordinator = core.next_coordinator();
        coordinator.completion_tx.try_send((Err("obsolete fixture result".into()), 0)).unwrap();
        assert!(coordinator.completion_tx.is_full());
    
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/formats/sample_44100_stereo.wav");
        let current = coordinator.set_next(
            path.to_string_lossy().into_owned(), 42, 123, None, None, None, None, false,
        );
        assert_eq!(current, 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let prepared = loop {
            if let Some(result) = coordinator.poll_completion(current) {
                break result.expect("latest valid WAV must prepare");
            }
            assert!(std::time::Instant::now() < deadline, "latest result timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(coordinator.current_generation(), current);
        assert_eq!((prepared.song_id, prepared.token), (42, 123));
    }

    #[test]
    fn test_playback_core_waiting_for_next_silence() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::Playing);
        // 只放一軌，協調器槽為空 → 軌結束後進入 WaitingForNext、輸出靜音而非 None
        let mut chain = TrackChain::new();
        chain.append(sine_track(44100, 2, 0.01, 1, 10, 1));
        chain.set_coordinator(core.next_coordinator());
        core.track_chain.lock().unwrap().replace(chain);
        let mut resampler = TrackResampler::new();
        resampler.set_track(Some(sine_track(44100, 2, 0.01, 1, 10, 1)));
        core.track_resampler.lock().unwrap().replace(resampler);

        let mut silence_after_end = 0u32;
        for _ in 0..10000 {
            match core.next() {
                Some(s) => {
                    if s == 0.0 { silence_after_end += 1; }
                }
                None => break,
            }
        }
        assert!(silence_after_end > 0, "軌結束後應輸出靜音（WaitingForNext）而非回傳 None");
    }

    #[test]
    fn test_next_coordinator_generation_and_clear() {
        let coord = NextCoordinator::new();
        let g1 = coord.set_next("/a.mp3".into(), 1, 10, None, None, None, None, false);
        let g2 = coord.set_next("/b.mp3".into(), 2, 20, None, None, None, None, false);
        let g3 = coord.clear_next();
        assert_eq!(g1, 1);
        assert_eq!(g2, 2);
        assert_eq!(g3, 3);
        assert_eq!(coord.current_generation(), 3);
        assert!(!coord.has_next());
        assert!(coord.poll_completion(3).is_none());
    }

    #[test]
    fn test_next_coordinator_set_next_replaces_latest() {
        let coord = NextCoordinator::new();
        let g1 = coord.set_next("/a.mp3".into(), 1, 10, None, None, None, None, false);
        let g2 = coord.set_next("/b.mp3".into(), 2, 20, None, None, None, None, false);
        // 容量 1 的待處理槽只保留最新，兩次 set_next 不應阻塞，且僅記錄最新世代
        assert_eq!(g1, 1);
        assert_eq!(g2, 2);
        assert_eq!(coord.current_generation(), 2);
    }

    #[test]
    fn test_next_coordinator_poll_completion_discards_stale() {
        let coord = NextCoordinator::new();
        coord.set_next("/a.mp3".into(), 1, 10, None, None, None, None, false); // gen 1，不存在的檔案 → worker 會產出 Err 完成通知
        coord.set_next("/b.mp3".into(), 2, 20, None, None, None, None, false); // gen 2，同樣不存在
        // 兩個檔案都不存在，絕不應回傳 Ok；過期世代（1）一律捨棄。
        // 工作執行緒是非同步的，channel 可能尚未就緒（None）或已傳達失敗（Err），皆可接受。
        assert!(
            matches!(coord.poll_completion(1), None | Some(Err(_))),
            "過期世代不應回傳 Ok"
        );
        assert!(
            matches!(coord.poll_completion(2), None | Some(Err(_))),
            "準備失敗不應回傳 Ok，而應回傳 Err 讓前端略過並重排"
        );
    }

    /// 建立一個極短的 WAV（44.1kHz 雙聲道 16-bit PCM 靜音），供 drop 測試用。
    fn write_temp_wav(dir: &tempfile::TempDir) -> std::path::PathBuf {
        use std::io::Write;
        let path = dir.path().join("test.wav");
        let sample_rate: u32 = 44100;
        let channels: u16 = 2;
        let bytes_per_sample: u16 = 2;
        let frame_count: u32 = (sample_rate as f32 * 0.1) as u32; // 0.1 秒
        let data_size = (frame_count as usize) * (channels as usize) * (bytes_per_sample as usize);
        let mut header = [0u8; 44];
        header[0..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&(36u32 + data_size as u32).to_le_bytes());
        header[8..12].copy_from_slice(b"WAVE");
        header[12..16].copy_from_slice(b"fmt ");
        header[16..20].copy_from_slice(&16u32.to_le_bytes());
        header[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
        header[22..24].copy_from_slice(&channels.to_le_bytes());
        header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
        header[28..32].copy_from_slice(&(sample_rate * channels as u32 * bytes_per_sample as u32).to_le_bytes());
        header[32..34].copy_from_slice(&(channels * bytes_per_sample).to_le_bytes());
        header[34..36].copy_from_slice(&16u16.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&(data_size as u32).to_le_bytes());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&vec![0u8; data_size]).unwrap();
        path
    }

    #[test]
    fn test_next_coordinator_drop_with_full_completion_channel() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp_wav(&dir);
        let coord = NextCoordinator::new();
        // 準備一個真實可解碼的檔案：worker 會把 Ok 結果送進容量 1 的 completion channel，
        // 使其滿；此時 drop 不得因 channel 滿或 worker 阻塞而永久等待。
        coord.set_next(path.to_string_lossy().to_string(), 1, 10, None, None, None, None, false);
        // 不 poll_completion，讓 channel 保持滿的狀態，然後 drop 驗證不會 deadlock。
        drop(coord);
    }

    #[test]
    fn test_next_coordinator_drop_does_not_deadlock() {
        let coord = NextCoordinator::new();
        coord.set_next("/a.mp3".into(), 1, 10, None, None, None, None, false);
        // worker 可能正在準備（或阻塞於 completion 通道），drop 時不得永久等待
        drop(coord);
    }

    #[test]
    fn test_playback_core_dsp_out_drain_order() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::Playing);

        // 直接將樣本放入 dsp_out，應該優先回傳（不經過 track_out/upstream）
        core.dsp_out.lock().unwrap().push_back(0.5);
        core.dsp_out.lock().unwrap().push_back(-0.3);

        let PlaybackSample::Audio(s1) = core.next_playing(core.state_gen()) else {
            panic!("expected playback sample");
        };
        assert!((s1 - 0.5).abs() < 1e-6, "dsp_out 應優先回傳第一個樣本");

        let PlaybackSample::Audio(s2) = core.next_playing(core.state_gen()) else {
            panic!("expected playback sample");
        };
        assert!((s2 - (-0.3)).abs() < 1e-6, "dsp_out 應續回傳第二個樣本");

        // dsp_out 耗盡且無 track_out/upstream，應回傳靜音並進入 WaitingForNext
        let PlaybackSample::Audio(s3) = core.next_playing(core.state_gen()) else {
            panic!("expected playback sample");
        };
        assert_eq!(s3, 0.0, "dsp_out 耗盡且無上游時應回傳靜音");
    }

    #[test]
    fn test_playback_core_rate_stage_tail_drain() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::Playing);

        // 安裝第一軌（440Hz，振幅 0.3，約 0.05 s @ 44.1kHz 雙聲道）
        let mut chain = TrackChain::new();
        chain.append(sine_track(44100, 2, 0.05, 1, 10, 1));
        chain.set_coordinator(core.next_coordinator());
        core.track_chain.lock().unwrap().replace(chain);

        let mut resampler = TrackResampler::new();
        resampler.set_track(Some(sine_track(44100, 2, 0.05, 1, 10, 1)));
        core.track_resampler.lock().unwrap().replace(resampler);
        *core.rate_stage.lock().unwrap() = Some(RateStage::new(INTERNAL_CHANNELS as usize, INTERNAL_SAMPLE_RATE));

        // 將第二軌（880Hz，振幅 1.0，約 0.1 s @ 48kHz 雙聲道，足夠長讓 TrackResampler 產出）放入待播槽
        let second_samples: Vec<f32> = (0..((48000 * 2) as f32 * 0.1) as usize)
            .map(|i| {
                let frame = i / 2;
                (2.0 * std::f32::consts::PI * 880.0 * frame as f32 / 48000.0).sin() * 1.0
            })
            .collect();
        core.next_coordinator()
            .slot
            .lock()
            .unwrap()
            .replace(PreparedTrack {
                            seq: 2,
                            token: 20,
                            song_id: 2,
                            path: "test".into(),
                            decoder: "symphonia".into(),
                            rate: 48000,
                            channels: 2,
                            duration_ms: 100,
                            norm_gain: 1.0,
                            replaygain_track_gain: None,
                            replaygain_track_peak: None,
                            replaygain_album_gain: None,
                            replaygain_album_peak: None,
                            data: TrackData::Memory(Arc::new(second_samples)),
                            source_slot: None,
                            staged_memory: None,
                            leading_silence_samples: 0,
                            trailing_silence_samples: 0,
                        });

        // 播放直到軌切換並持續消耗，確認第二軌音訊確實出現（振幅 1.0 > 第一軌 0.3）
        // 不依賴 seq（sync_identity 會在舊尾音尚存於 dsp_out 時就發布新身分），
        // 而是直接檢查樣本內容：第一軌振幅 ≤ 0.3，第二軌振幅 1.0，|s| > 0.5 僅可能來自第二軌。
        let mut found_second = false;
        let mut total = 0u64;
        for _ in 0..100000 {
            match core.next() {
                Some(s) => {
                    total += 1;
                    if s.abs() > 0.5 {
                        found_second = true;
                        break;
                    }
                }
                None => break,
            }
        }

        assert!(total > 0, "應有樣本輸出");
        assert!(found_second, "第二軌（振幅 1.0）音訊應確實出現，而非僅為舊軌尾音（振幅 ≤ 0.3）");
    }

    /// 響度補償擱架：啟用時套用增益，關閉時旁路（輸入 = 輸出）
    #[test]
    fn test_loudness_shelves_bypass() {
        use crate::dsp::Bq;
        let mut shelves = LoudnessShelves::new(INTERNAL_CHANNELS as usize);
        let eq_shared = Arc::new(EqShared::new());

        // 初始狀態：關閉，輸入應等於輸出
        let (l_in, r_in) = (0.5, -0.3);
        let (l_out, r_out) = shelves.process(l_in, r_in);
        assert_eq!(l_out, l_in, "關閉時左聲道應旁路");
        assert_eq!(r_out, r_in, "關閉時右聲道應旁路");

        // 啟用響度補償，設定 bass +6 dB, treble +3 dB
        eq_shared.update(|p| {
            p.loudness_enabled = true;
            p.loudness_bass_db = 6.0;
            p.loudness_treble_db = 3.0;
            p.loudness_bass_hz = 150.0;
            p.loudness_treble_hz = 6000.0;
        });
        shelves.maybe_retune(&eq_shared, INTERNAL_SAMPLE_RATE);
        assert!(shelves.enabled, "應啟用響度補償");

        // 低頻測試訊號（100 Hz，frame 1000 避免 sin(0)=0）應被 bass shelf 增強
        let frame = 1000usize;
        let lf = (2.0 * std::f32::consts::PI * 100.0 * frame as f32 / INTERNAL_SAMPLE_RATE as f32).sin() * 0.5;
        let (l_lf, r_lf) = shelves.process(lf, lf);
        // bass shelf 在 100 Hz (低於 150 Hz 角頻率) 應有增益 > 1
        let bass_gain = l_lf.abs() / lf.abs();
        assert!(bass_gain > 1.0, "低頻應被 bass shelf 增強，gain={bass_gain}");

        // 高頻測試訊號（8000 Hz）應被 treble shelf 增強
        let hf = (2.0 * std::f32::consts::PI * 8000.0 * frame as f32 / INTERNAL_SAMPLE_RATE as f32).sin() * 0.3;
        let (l_hf, r_hf) = shelves.process(hf, hf);
        let treble_gain = l_hf.abs() / hf.abs();
        assert!(treble_gain > 1.0, "高頻應被 treble shelf 增強，gain={treble_gain}");

        // 再次關閉：應回到旁路
        eq_shared.update(|p| p.loudness_enabled = false);
        shelves.maybe_retune(&eq_shared, INTERNAL_SAMPLE_RATE);
        assert!(!shelves.enabled, "應關閉響度補償");
        let (l_off, r_off) = shelves.process(l_in, r_in);
        assert_eq!(l_off, l_in, "關閉後左聲道應旁路");
        assert_eq!(r_off, r_in, "關閉後右聲道應旁路");
    }

    /// #10 單元測試：FadeGate 應線性插值 from_gain → to_gain，段落結束後鎖定 base_gain = to_gain
    #[test]
    fn test_fade_gate_linear_ramp() {
        let core = core_with_eq();
        // 手動建立一個淡段：from 0.0 → to 1.0，共 100 個樣本
        {
            let mut fg = core.fade_gate.lock().unwrap();
            fg.active = Some(crate::models::FadeSegment {
                gen: 1,
                target: crate::models::FadeTarget::None,
                from_gain: 0.0,
                to_gain: 1.0,
                total_samples: 100,
                consumed_samples: 0,
            });
            fg.base_gain = 0.0;
        }

        // 第 1 樣本：t = 1/100，gain = 0.0 + (1.0 - 0.0) * 0.01 = 0.01
        let s1 = core.apply_fade_gain_to_sample(Some(1.0)).unwrap();
        assert!((s1 - 0.01).abs() < 1e-6, "第 1 樣本 gain 應為 0.01，實際：{s1}");

        // 第 50 樣本：t = 50/100，gain = 0.5
        for _ in 1..49 {
            core.apply_fade_gain_to_sample(Some(1.0));
        }
        let s50 = core.apply_fade_gain_to_sample(Some(1.0)).unwrap();
        assert!((s50 - 0.5).abs() < 1e-6, "第 50 樣本 gain 應為 0.5，實際：{s50}");

        // 第 100 樣本：t = 1.0，段落結束，gain = 1.0，base_gain 鎖定為 1.0
        for _ in 50..99 {
            core.apply_fade_gain_to_sample(Some(1.0));
        }
        let s100 = core.apply_fade_gain_to_sample(Some(1.0)).unwrap();
        assert!((s100 - 1.0).abs() < 1e-6, "第 100 樣本 gain 應為 1.0，實際：{s100}");
        {
            let fg = core.fade_gate.lock().unwrap();
            assert_eq!(fg.base_gain, 1.0, "段落結束後 base_gain 應鎖定為 to_gain");
            assert!(fg.active.is_none(), "段落結束後 active 應為 None");
        }
    }

    /// #10 單元測試：gap_remaining 應按交錯樣本數遞減，歸零後不再累計
    #[test]
    fn test_gap_remaining_depletion() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::Playing);
        {
            let mut fg = core.fade_gate.lock().unwrap();
            fg.gap_remaining = 1000; // 1000 個交錯樣本
        }

        // 消耗 400 個樣本後，gap_remaining 應為 600
        for _ in 0..400 {
            core.next();
        }
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            600,
            "gap_remaining 應遞減至 600"
        );

        // 繼續消耗 600 個樣本後，gap_remaining 應歸零
        for _ in 0..600 {
            core.next();
        }
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            0,
            "gap_remaining 應歸零"
        );

        // 繼續消耗不應使 gap_remaining 為負數
        for _ in 0..100 {
            core.next();
        }
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            0,
            "gap_remaining 不應為負數"
        );
    }

    #[test]
    fn trimmed_track_origin_uses_native_frames_for_mono_and_stereo() {
        for channels in [1, 2] {
            let mut track = prepared_memory(44_100, channels, 0.1, 1, 10, 1);
            assert_eq!(track.leading_silence_samples, 0);
            assert_eq!(track.trailing_silence_samples, 0);
            track.leading_silence_samples = 441;
            let TrackData::Memory(samples) = &track.data else {
                panic!("expected a memory track");
            };
            let item = trackchain_item(&track, samples.clone());
            assert_eq!(native_frame_to_ms(item.leading_silence_samples, item.sample_rate), 10);
        }
    }

    /// #10 單元測試：stream_gen 與 pending_stream_meta.gen 匹配時，next() 應設 stream_install_ready
    #[test]
    fn test_stream_install_ready_generation_binding() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::WaitingForNext);
        // 設 gap_ms > 0，否則 WaitingForNext 分支不會進入 gap 處理
        core.eq_shared.update(|p| p.gap_ms = 100);
        {
            let mut fg = core.fade_gate.lock().unwrap();
            fg.gap_remaining = 1; // 僅剩 1 個樣本，下一次 next() 會耗盡並設 stream_install_ready
        }
        // 安裝一個 strem 來源與 meta，gen 為 5
        let meta = crate::models::StreamMeta {
            song_id: 1,
            duration_ms: 1000,
            rate: 48000,
            channels: 2,
            path: "test".into(),
            decoder: "stream".into(),
            position_ms: 0,
            gen: 5,
        };
        let source: Box<dyn Iterator<Item = f32> + Send> =
            Box::new(std::iter::repeat(0.5f32).take(100));
        core.pending_stream_source.lock().unwrap().replace(source);
        core.pending_stream_meta.lock().unwrap().replace(meta.clone());
        core.stream_gen.store(5, std::sync::atomic::Ordering::Release);

        // next() 在 WaitingForNext 且 gap 耗盡時應檢查 meta.gen == stream_gen，並設 stream_install_ready
        core.next();
        let fg = core.fade_gate.lock().unwrap();
        assert_eq!(
            fg.stream_install_ready,
            Some(5),
            "stream_install_ready 應設為 meta.gen (5)"
        );
    }

    /// 回歸測試：boundaries 上限固定為 BOUNDARY_RECORD_CAPACITY 筆；
    /// 達上限時先淘汰最舊再插入，長度恆不超過上限，順序為舊→新。
    #[test]
    fn test_boundaries_capacity_oldest_eviction() {
        let core = core_with_eq();

        let total: u64 = 1500;
        for i in 1..=total {
            core.push_boundary(BoundaryRecord {
                boundary_in_48k: i,
                boundary_out_48k: i,
                seq: i,
                token: i,
                song_id: 1,
                norm_gain: 1.0,
                duration_ms: 100,
                native_sample_rate: 48000,
                native_channels: 2,
            });
        }

        let all = core.boundaries();
        // 不得超過上限，且只保留最近的 1000 筆（501..=1500）
        assert_eq!(
            all.len(),
            BOUNDARY_RECORD_CAPACITY,
            "boundaries 長度應恆等於上限 {BOUNDARY_RECORD_CAPACITY}"
        );
        assert_eq!(
            all.first().unwrap().seq,
            total - BOUNDARY_RECORD_CAPACITY as u64 + 1,
            "最舊紀錄應為 501，前 500 筆已被淘汰"
        );
        assert_eq!(all.last().unwrap().seq, total, "最新紀錄應為最後一筆");

        // clear_expired_boundaries 語義不變：只保留 seq >= current_seq
        core.clear_expired_boundaries(total - 100);
        let after = core.boundaries();
        assert_eq!(after.len(), 101, "clear_expired_boundaries 僅保留 seq >= current_seq");
        assert_eq!(after.first().unwrap().seq, total - 100);
        assert_eq!(after.last().unwrap().seq, total);
    }

    /// C1：預檢失敗直接拒絕、不進 FFmpeg 後備。
    /// 建立不足標頭長度（8 bytes，小於預檢需讀取的 16 bytes）的普通空 WAV，
    /// 走真正的 prepare_track，斷言錯誤為預檢拒絕而非 FFmpeg 錯誤。
    #[test]
    fn test_prepare_track_short_wav_precheck_rejects_not_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.wav");
        // 標準 PCM WAV 標頭需 44 bytes；這裡僅寫 8 bytes（RIFF + 欄位，不含 WAVE/fmt/data），
        // 為不足標頭長度的普通空檔，非攻擊性或超限音檔。
        std::fs::write(&path, b"RIFF....").unwrap();

        let path_str = path.to_string_lossy().to_string();
        let result = prepare_track(&path_str, 1, 1, 10, None, None, None, None, false);

        assert!(result.is_err(), "預檢失敗應直接回傳 Err，實際：{result:?}");
        let err = result.unwrap_err();
        assert!(
            err.contains("預檢"),
            "錯誤應為預檢拒絕而非 FFmpeg 錯誤，實際：{err}"
        );
        assert!(!err.contains("ffmpeg"), "不應進入 FFmpeg 後備，實際：{err}");
    }

    /// PP9：Symphonia 短檔預載以有界 PcmCollector（PCM_MAX_BYTES = 400 MiB）收集。
    /// 一般 WAV 未超過上限，應進入記憶體軌（不降級為延後串流），樣本數與長度正確；
    /// 不使用異常輸入，亦不觸碰 FFmpeg 與直接載入路徑。
    #[test]
    fn test_prepare_track_sym_short_wav_bounded_collector_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp_wav(&dir);
        let path_str = path.to_string_lossy().to_string();

        let result = prepare_track(&path_str, 1, 1, 10, None, None, None, None, false);
        assert!(result.is_ok(), "一般 WAV 應在 400 MB 上限內整首解碼進記憶體，實際：{result:?}");
        let track = result.unwrap();

        assert_eq!(track.decoder, "symphonia");
        assert_eq!(track.rate, 44100);
        assert_eq!(track.channels, 2);
        assert_eq!(track.duration_ms, 100);
        assert_eq!(track.norm_gain, 1.0);

        match &track.data {
            TrackData::Memory(samples) => {
                assert_eq!(
                    samples.len(),
                    8820,
                    "44.1kHz 雙聲道 0.1 s 應為 8820 個 f32 樣本，實際 {}",
                    samples.len()
                );
                assert!(
                    samples.iter().all(|&s| s == 0.0),
                    "靜音 WAV 解碼樣本應全為 0"
                );
            }
            TrackData::Stream(_) => panic!("一般 WAV 不應因 PCM 上限降級為串流軌，實際：{track:?}"),
        }

        assert_eq!(track.leading_silence_samples, 0);
        assert_eq!(track.trailing_silence_samples, 0);

        // PCM_MAX_BYTES 已對子模組公開，並與 PcmCollector 上限一致（400 MiB）
        assert_eq!(crate::player::PCM_MAX_BYTES, 400 * 1024 * 1024);
        let collector = crate::player::PcmCollector::with_max_bytes(crate::player::PCM_MAX_BYTES);
        assert_eq!(collector.capacity_bytes(), 0);
    }
    #[test]
    fn test_preloaded_short_wav_retains_original_duration_after_trim() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp_wav(&dir);
        let mut wav = std::fs::read(&path).unwrap();
        let audible_start = 441;
        let audible_end = 4410 - 882;
        for frame in wav[44..].chunks_exact_mut(4).take(audible_end).skip(audible_start) {
            frame[..2].copy_from_slice(&8192i16.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();

        let track = prepare_track(path.to_str().unwrap(), 1, 2, 3, None, None, None, None, true).unwrap();
        assert_eq!(track.duration_ms, 100);
        assert_eq!(track.leading_silence_samples, 441);
        assert_eq!(track.trailing_silence_samples, 882);
        let TrackData::Memory(samples) = track.data else {
            panic!("short WAV should prepare in memory");
        };
        assert_eq!(samples.len(), (audible_end - audible_start) * 2);
        assert_eq!(&samples[..2], &[0.25, 0.0]);
    }

    #[test]
    fn test_short_wav_pcm_budget_prepares_buffered_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp_wav(&dir);
        let track = prepare_track_with_pcm_limit(path.to_str().unwrap(), 1, 2, 3, Some(-3.0), Some(0.9), Some(-2.0), Some(0.8), false, 4usize).unwrap();
        assert_eq!(track.song_id, 1);
        assert_eq!(track.seq, 2);
        assert_eq!(track.token, 3);
        assert_eq!(track.duration_ms, 100);
        assert_eq!(track.replaygain_track_gain, Some(-3.0));
        assert_eq!(track.replaygain_track_peak, Some(0.9));
        assert_eq!(track.replaygain_album_gain, Some(-2.0));
        assert_eq!(track.replaygain_album_peak, Some(0.8));
        match &track.data {
            TrackData::Stream(wrapper) => match wrapper.as_ref() {
                StreamWrapper::Buffered(staged) => {
                    let mut staged = staged.lock().unwrap();
                    let ready = staged.as_mut().unwrap();
                    assert_eq!(ready.identity.token, 3);
                    assert_eq!(ready.resampler.current_track().unwrap().song_id, 1);
                    for _ in 0..BLOCK_FRAMES * 2 {
                        let source = ready.source.as_mut().expect("buffered track has a sample source");
                        assert_eq!(source.poll_sample(), crate::prefetch_stream::StreamRead::Sample(0.0));
                    }
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn test_buffered_wav_handoff_outputs_exact_gap_and_audible_next_track() {
        assert_buffered_wav_handoff("stream", |path| {
            prepare_track_with_pcm_limit(path, 2, 2, 20, None, None, None, None, false, 4)
        });
    }

    #[test]
    #[ignore = "requires ffprobe and ffmpeg"]
    fn test_ffmpeg_buffered_wav_handoff_after_pcm_overflow() {
        assert_buffered_wav_handoff("ffmpeg-stream", |path| {
            prepare_with_ffmpeg(path, 2, "wav", 2, 20, None, None, None, None, false, 4, None, None)
        });
    }

    fn assert_buffered_wav_handoff(decoder: &str, prepare: impl Fn(&str) -> Result<PreparedTrack, String>) {
        struct EmptyWithDrop(Arc<AtomicBool>);
        impl Iterator for EmptyWithDrop {
            type Item = f32;
            fn next(&mut self) -> Option<f32> {
                None
            }
        }
        impl Drop for EmptyWithDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let folder = tempfile::tempdir().unwrap();
        let path = write_temp_wav(&folder);
        let mut wav = std::fs::read(&path).unwrap();
        for sample in wav[44..].chunks_exact_mut(2) {
            sample.copy_from_slice(&(-8192i16).to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();

        for (gap_ms, expected_silence) in [(0, 0), (100, 9600)] {
            let core = core_with_eq();
            core.eq_shared.update(|settings| settings.gap_ms = gap_ms);
            let mut first = sine_track(48_000, 2, 0.05, 1, 10, 1);
            first.samples = Arc::new(vec![0.25; first.samples.len()]);
            install_memory_playback(&core, first);
            let old_source_dropped = Arc::new(AtomicBool::new(false));
            core.set_stream_source(Some(Box::new(EmptyWithDrop(old_source_dropped.clone()))));
            core.set_now_identity(1, 10);
            let old_play = core.increment_play_seq();
            core.set_playback_state(PlaybackState::Playing);

            let next = prepare(path.to_str().unwrap()).unwrap();
            assert_eq!(next.decoder, decoder);
            assert_eq!(
                next.buffered_request_state(),
                Some(crate::models::NextRequestState::Ready),
            );
            *core.next_coordinator.slot.lock().unwrap() = Some(next);

            let mut silence = 0;
            let mut heard_first = false;
            let next_sample = (0..30_000).find_map(|_| {
                let sample = core.next().unwrap();
                if core.now_token() == 20 {
                    return Some(sample);
                }
                if core.playback_state() == PlaybackState::WaitingForNext {
                    assert_eq!(sample, 0.0);
                    silence += 1;
                } else {
                    heard_first |= sample > 0.02;
                }
                assert_eq!(core.play_seq(), old_play);
                None
            }).expect("buffered WAV did not take over");
            assert!(heard_first);
            assert_eq!(silence, expected_silence);
            assert_eq!(core.play_seq(), old_play + 1);
            assert_eq!(core.current_track_meta().unwrap().song_id, 2);
            assert!(next_sample < -0.02 || (0..256).any(|_| core.next().unwrap() < -0.02));
            assert!(!old_source_dropped.load(Ordering::Acquire));
            core.reap_retired_handoffs();
            assert!(old_source_dropped.load(Ordering::Acquire));
        }
    }

    #[test]
    fn replacing_and_clearing_next_cancel_only_prior_preparations() {
        let coordinator = NextCoordinator::new();
        let submit = |token| {
            coordinator.set_next("missing-test-track.wav".into(), 1, token, None, None, None, None, false)
        };
        let flag = || coordinator.submission.lock().unwrap().as_ref().unwrap().clone();
        let first_generation = submit(1);
        let first = flag();
        assert!(!first.load(Ordering::Acquire));
        let second_generation = submit(2);
        let second = flag();
        assert!(second_generation > first_generation);
        assert!(first.load(Ordering::Acquire));
        assert!(!second.load(Ordering::Acquire));
        assert!(coordinator.clear_next() > second_generation);
        assert!(second.load(Ordering::Acquire));
    }

    #[test]
    fn test_play_seq_starts_at_zero() {
        let core = core_with_eq();
        assert_eq!(core.play_seq(), 0);
        assert_eq!(core.now_seq(), 0);
    }

    #[test]
    fn test_buffered_prep_failure_invalidates_only_matching_queue_generation() {
        use crate::models::NextRequestState;
        use crate::prefetch_stream::{prepared_stream, StreamFailure, FRAME_SIZE};

        let core = core_with_eq();
        core.set_now_identity(1, 10);
        let coordinator = core.next_coordinator();
        let generation = coordinator.current_generation();
        let mut track = prepared_memory(48_000, 2, 0.1, 2, 20, 2);
        let (writer, source) = prepared_stream(2, [0.25; FRAME_SIZE]).unwrap();
        let ready = PreparedHandoff {
            source: Some(source),
            identity: prepared_to_trackchain_item(&track).unwrap(),
            chain: TrackChain::new(),
            resampler: TrackResampler::new(),
            rate_stage: RateStage::new(INTERNAL_CHANNELS as usize, INTERNAL_SAMPLE_RATE),
        };
        track.data = TrackData::Stream(Arc::new(StreamWrapper::Buffered(Mutex::new(Some(ready)))));
        coordinator.record_next_request(track.token, NextRequestState::Ready);
        *coordinator.slot.lock().unwrap() = Some(track);

        writer.fail(StreamFailure::PacketBudget);
        coordinator.discard_failed_buffered(generation + 1);
        assert!(coordinator.peek_next().is_some());
        coordinator.discard_failed_buffered(generation);
        assert!(coordinator.peek_next().is_none());
        let receipt = coordinator.next_request_result().unwrap();
        assert_eq!(receipt.token, 20);
        assert_eq!(receipt.state, NextRequestState::Failed);
        assert_eq!(core.now_token(), 10);
    }

    /// Prepare 後僅標記等待首次播放，不立即分配；首次 Play/Toggle 分配一次；續播不重複。
    #[test]
    fn test_play_seq_awaiting_first_play_lifecycle() {
        let core = core_with_eq();
        assert_eq!(core.play_seq(), 0);

        // 未標記等待時，不分配
        assert!(!core.try_allocate_first_play());
        assert_eq!(core.play_seq(), 0);

        // 標記後首次分配一次
        core.mark_awaiting_first_play();
        assert!(core.try_allocate_first_play());
        assert_eq!(core.play_seq(), 1);
        // 續播：旗標已歸零，不重複分配
        assert!(!core.try_allocate_first_play());
        assert_eq!(core.play_seq(), 1);

        // 再次標記後再分配一次
        core.mark_awaiting_first_play();
        assert!(core.try_allocate_first_play());
        assert_eq!(core.play_seq(), 2);

        // 清除等待後不再自動分配（直接載入由 Load 層自行分配）
        core.clear_awaiting_first_play();
        assert!(!core.try_allocate_first_play());
        assert_eq!(core.play_seq(), 2);
    }

    /// 自然軌交接：實際安裝好新軌後產生一次播放事件；同一身分不重複發布。
    #[test]
    fn test_play_seq_natural_handoff_increments_once() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::Playing);

        // 模擬已由直接載入安裝第一軌：身分已就緒，其播放事件由 Load 層分配（此處仍為 0）
        core.set_now_identity(1, 10);
        assert_eq!(core.play_seq(), 0);

        let track1 = sine_track(44100, 2, 0.1, 1, 10, 1);
        let mut chain = TrackChain::new();
        chain.append(track1);
        chain.set_coordinator(core.next_coordinator());
        core.track_chain.lock().unwrap().replace(chain);

        // 背景準備完成：第二軌進入待播槽
        core.next_coordinator()
            .slot
            .lock()
            .unwrap()
            .replace(prepared_memory(48000, 2, 0.1, 2, 20, 2));

        let mut resampler = TrackResampler::new();
        resampler.set_track(Some(sine_track(44100, 2, 0.1, 1, 10, 1)));
        core.track_resampler.lock().unwrap().replace(resampler);

        // 拉樣本直到軌交接發生（第一軌耗盡後從待播槽 take 第二軌）
        let mut handed_off = false;
        for _ in 0..40000 {
            core.next();
            if core.now_seq() == 2 {
                handed_off = true;
                break;
            }
        }
        assert!(handed_off, "應從待播槽 take 第二軌並切換身分");
        assert_eq!(core.now_token(), 20);

        // 交接後產生一次播放事件
        assert_eq!(core.play_seq(), 1, "自然軌交接應產生一次播放事件");

        // 同一軌繼續播放（含尾端排空）不得重複產生事件
        let seq_after_tail = core.play_seq();
        for _ in 0..10000 {
            core.next();
        }
        assert_eq!(core.play_seq(), seq_after_tail, "同一軌持續播放不得重複產生事件");
    }

    /// 以實際 PlaybackCore 路徑安裝單一記憶體軌（含 chain / resampler / RateStage）。
    fn install_memory_playback(core: &PlaybackCore, item: TrackChainItem) {
        let mut chain = TrackChain::new();
        chain.append(item.clone());
        chain.set_coordinator(core.next_coordinator());
        core.track_chain.lock().unwrap().replace(chain);
        let mut resampler = TrackResampler::new();
        resampler.set_track(Some(item));
        core.track_resampler.lock().unwrap().replace(resampler);
        *core.rate_stage.lock().unwrap() =
            Some(RateStage::new(INTERNAL_CHANNELS as usize, INTERNAL_SAMPLE_RATE));
    }

    /// 沿實際 PlaybackCore 播放路徑推進，直到舊軌尾端排空並進入 WaitingForNext。
    /// 回傳轉場當下的 played_out_frames（用於確認 gap 不推進舊進度）。
    fn drive_until_waiting(core: &PlaybackCore, limit: usize) -> u64 {
        for _ in 0..limit {
            let before = core.playback_state();
            let _ = core.next().expect("core.next() 應持續回傳樣本");
            let after = core.playback_state();
            if before == PlaybackState::Playing && after == PlaybackState::WaitingForNext {
                return core.played_out_frames();
            }
        }
        panic!("未在期限內進入 WaitingForNext");
    }

    /// B1：TrackChain 只耗盡目前記憶體軌，不得直接 take coordinator。
    #[test]
    fn test_track_chain_exhausts_memory_without_taking_coordinator() {
        let coord = Arc::new(NextCoordinator::new());
        coord
            .slot
            .lock()
            .unwrap()
            .replace(prepared_memory(48000, 2, 0.01, 2, 20, 2));

        let mut chain = TrackChain::new();
        chain.append(sine_track(48000, 2, 0.001, 1, 10, 1));
        chain.set_coordinator(coord.clone());

        let mut count = 0usize;
        while chain.next().is_some() {
            count += 1;
        }

        assert!(count > 0, "目前記憶體軌應先被耗盡");
        assert!(chain.is_finished(), "TrackChain 應標記結束");
        assert!(coord.has_next(), "TrackChain 不得直接從 coordinator take 下一軌");
    }

    #[test]
    fn test_explicit_no_next_finishes_and_is_scoped_to_play_sequence() {
        let core = core_with_eq();
        let first_play = core.increment_play_seq();
        install_memory_playback(&core, sine_track(48_000, 2, 0.02, 1, 10, 1));
        core.set_now_identity(1, 10);
        core.set_next_absence(first_play, true);
        core.set_playback_state(PlaybackState::Playing);
        assert!((0..10_000).any(|_| {
            core.next();
            core.playback_state() == PlaybackState::Finished
        }));

        core.increment_play_seq();
        core.set_playback_state(PlaybackState::WaitingForNext);
        assert_eq!(core.next(), Some(0.0));
        assert_eq!(core.playback_state(), PlaybackState::WaitingForNext);
    }

    #[test]
    fn test_memory_handoff_100ms_gap_outputs_exactly_9600_zeros() {
        let core = core_with_eq();
        core.eq_shared.update(|p| p.gap_ms = 100);
        core.set_playback_state(PlaybackState::Playing);
        install_memory_playback(&core, sine_track(44100, 2, 0.1, 1, 10, 1));
        core.next_coordinator()
            .slot
            .lock()
            .unwrap()
            .replace(prepared_memory(48000, 2, 0.2, 2, 20, 2));

        let played_at_transition = drive_until_waiting(&core, 50_000);
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            9_599,
            "EOF 轉場呼叫應計入 9600 個 gap 樣本的第一個"
        );

        let mut zeros = 1u64; // EOF 轉場呼叫
        for _ in 0..9_599 {
            assert_eq!(core.playback_state(), PlaybackState::WaitingForNext);
            assert_eq!(core.next().unwrap(), 0.0, "gap 內應輸出交錯零樣本");
            assert_eq!(
                core.played_out_frames(),
                played_at_transition,
                "gap 期間不得推進舊進度"
            );
            zeros += 1;
        }

        assert_eq!(zeros, 9_600, "100ms @ 48kHz 立體聲應為 9600 個交錯零樣本");
        assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 0);
        assert_eq!(core.playback_state(), PlaybackState::WaitingForNext);

        core.next().unwrap();
        assert_eq!(
            core.playback_state(),
            PlaybackState::Playing,
            "ready next 應在 gap 結束後直接播放"
        );
        assert_eq!(core.now_token(), 20);
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            0,
            "gap 不得重啟"
        );
    }

    /// B1：gap 為 0 且下一軌已 ready 時，不得多輸出一個等待樣本。
    #[test]
    fn test_memory_handoff_zero_gap_with_ready_next_has_no_extra_sample() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::Playing);
        install_memory_playback(&core, sine_track(44100, 2, 0.1, 1, 10, 1));
        core.next_coordinator()
            .slot
            .lock()
            .unwrap()
            .replace(prepared_memory(48000, 2, 0.2, 2, 20, 2));

        let mut saw_waiting = false;
        let mut reached_next = false;
        for _ in 0..50_000 {
            let before = core.playback_state();
            core.next().unwrap();
            let after = core.playback_state();
            if before == PlaybackState::WaitingForNext || after == PlaybackState::WaitingForNext {
                saw_waiting = true;
            }
            assert_eq!(
                core.fade_gate.lock().unwrap().gap_remaining,
                0,
                "gap 0 不得產生 gap 樣本"
            );
            if core.now_token() == 20 {
                reached_next = true;
                break;
            }
        }

        assert!(reached_next, "ready next 應開始播放");
        assert!(
            !saw_waiting,
            "gap 0 且有 ready next 時不得進入 WaitingForNext 或輸出額外 gap 樣本"
        );
        assert_eq!(core.playback_state(), PlaybackState::Playing);
    }

    /// B1：晚到的 ready next 不得重啟 gap；暫停也不得消耗或重啟 gap。
    #[test]
    fn test_memory_handoff_late_ready_and_pause_do_not_restart_gap() {
        let core = core_with_eq();
        core.eq_shared.update(|p| p.gap_ms = 100);
        core.set_playback_state(PlaybackState::Playing);
        install_memory_playback(&core, sine_track(44100, 2, 0.1, 1, 10, 1));

        let played_at_transition = drive_until_waiting(&core, 50_000);
        assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 9_599);

        // 暫停不得消耗或重啟 gap；恢復後從同一個 gap 繼續。
        core.set_playback_state(PlaybackState::Paused);
        for _ in 0..100 {
            assert_eq!(core.next().unwrap(), 0.0);
            assert_eq!(
                core.fade_gate.lock().unwrap().gap_remaining,
                9_599,
                "暫停不得消耗 gap"
            );
            assert_eq!(core.played_out_frames(), played_at_transition);
        }
        core.set_playback_state(PlaybackState::Playing);

        for _ in 0..9_599 {
            assert_eq!(core.next().unwrap(), 0.0);
        }
        assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 0);
        assert_eq!(core.playback_state(), PlaybackState::WaitingForNext);
        assert_eq!(
            core.played_out_frames(),
            played_at_transition,
            "gap 期間不得推進舊進度"
        );

        // 下一軌晚到：安裝後直接播放，不得重新初始化 gap。
        core.next_coordinator()
            .slot
            .lock()
            .unwrap()
            .replace(prepared_memory(48000, 2, 0.2, 2, 20, 2));
        let before = core.playback_state();
        core.next().unwrap();
        assert_eq!(before, PlaybackState::WaitingForNext);
        assert_eq!(
            core.playback_state(),
            PlaybackState::Playing,
            "晚到的 ready next 應直接播放"
        );
        assert_eq!(core.now_token(), 20);
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            0,
            "晚到的 next 不得重啟 gap"
        );

        for _ in 0..1_000 {
            core.next().unwrap();
            assert_eq!(
                core.fade_gate.lock().unwrap().gap_remaining,
                0,
                "gap 不得再次出現"
            );
        }
    }

    /// 建立可辨別的軌（880Hz、振幅 1.0；sine_track 為 440Hz、振幅 0.3）。
    fn distinct_sine_track(rate: u32, ch: u16, secs: f32, seq: u64, token: u64, song_id: i64) -> TrackChainItem {
        let n = (rate as f32 * secs) as usize;
        let samples: Vec<f32> = (0..n.saturating_mul(ch as usize)).map(|i| {
            let frame = i / ch as usize;
            (2.0 * std::f32::consts::PI * 880.0 * frame as f32 / rate as f32).sin()
        }).collect();
        TrackChainItem {
            samples: Arc::new(samples),
            sample_rate: rate,
            channels: ch,
            duration_ms: (secs * 1000.0) as i64,
            song_id,
            norm_gain: 1.0,
            replaygain_track_gain: None,
            replaygain_track_peak: None,
            replaygain_album_gain: None,
            replaygain_album_peak: None,
            token,
            seq,
            native_frame_pos: 0,
            path: "test".into(),
            decoder: "synth".into(),
            leading_silence_samples: 0,
            trailing_silence_samples: 0,
            effective_end_frame: None,
        }
    }

    /// 可辨別的記憶體下一軌（880Hz、振幅 1.0）。
    fn prepared_distinct(rate: u32, secs: f32, seq: u64, token: u64, song_id: i64) -> PreparedTrack {
        let t = distinct_sine_track(rate, 2, secs, seq, token, song_id);
        PreparedTrack {
            seq,
            token,
            song_id,
            path: "test".into(),
            decoder: "symphonia".into(),
            rate,
            channels: 2,
            duration_ms: t.duration_ms,
            norm_gain: 1.0,
            replaygain_track_gain: None,
            replaygain_track_peak: None,
            replaygain_album_gain: None,
            replaygain_album_peak: None,
            data: TrackData::Memory(t.samples),
            source_slot: None,
            staged_memory: None,
            leading_silence_samples: 0,
            trailing_silence_samples: 0,
        }
    }

    /// B1 修正：gap 耗盡仍在等待（下一軌晚到）時暫停/恢復：不得重啟 gap、不得清除
    /// 收尾階段；恢復後的重新排水走 guarded 短轉場，轉場樣本為 0；晚到下一軌的
    /// 第一個可用樣本可辨別（|s|>0.5 僅新軌振幅可達），且 gap 全程不得重啟。
    #[test]
    fn test_handoff_pause_resume_after_gap_depleted_no_restart() {
        let core = core_with_eq();
        core.eq_shared.update(|p| p.gap_ms = 100);
        core.set_playback_state(PlaybackState::Playing);
        install_memory_playback(&core, sine_track(44100, 2, 0.1, 1, 10, 1));

        let played_at_transition = drive_until_waiting(&core, 50_000);
        assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 9_599);
        assert!(core.handoff_ending_active(), "EOF 轉場應建立活動收尾階段");

        // 耗盡 gap（下一軌晚到，待播槽仍空）
        for _ in 0..9_599 {
            assert_eq!(core.next().unwrap(), 0.0);
        }
        assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 0);
        assert_eq!(core.playback_state(), PlaybackState::WaitingForNext);

        // gap 已為 0 仍在等待時暫停：不得消耗 gap、不得清除收尾階段、不得推進進度
        core.set_playback_state(PlaybackState::Paused);
        for _ in 0..50 {
            assert_eq!(core.next().unwrap(), 0.0);
            assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 0);
            assert!(core.handoff_ending_active(), "普通暫停不得清除收尾階段");
            assert_eq!(core.played_out_frames(), played_at_transition);
        }

        // 恢復：重新排水不得重啟 gap；重新轉場樣本為 0，狀態回 WaitingForNext
        core.set_playback_state(PlaybackState::Playing);
        let transition_sample = core.next().unwrap();
        assert_eq!(transition_sample, 0.0, "恢復後的重新轉場樣本應為 0");
        assert_eq!(core.playback_state(), PlaybackState::WaitingForNext);
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            0,
            "gap 已為 0 時重新排水不得重啟 gap"
        );
        assert!(core.handoff_ending_active(), "重新轉場後收尾階段應仍活動");
        assert_eq!(core.played_out_frames(), played_at_transition, "等待期間不得推進舊進度");

        // 下一軌晚到：安裝後直接播放，gap 不得重啟；第一個可用樣本應可辨別為新軌
        core.next_coordinator()
            .slot
            .lock()
            .unwrap()
            .replace(prepared_distinct(48000, 0.2, 2, 20, 2));
        let mut found_usable = false;
        for _ in 0..20_000 {
            let s = core.next().unwrap();
            if s.abs() > 0.5 {
                found_usable = true;
                break;
            }
            assert_eq!(
                core.fade_gate.lock().unwrap().gap_remaining,
                0,
                "晚到 next 不得重啟 gap"
            );
        }
        assert!(found_usable, "晚到下一軌應輸出可辨別（|s|>0.5）的第一個可用樣本");
        assert_eq!(core.now_token(), 20);
        assert_eq!(core.playback_state(), PlaybackState::Playing);
        assert!(!core.handoff_ending_active(), "新軌接手後收尾階段應結束");
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            0,
            "gap 不得再次出現"
        );
    }

    /// B1 修正：abort_handoff 明確邊界重置收尾階段/交接進度旗標與 gap；其後安裝的
    /// 新來源（模擬 Load 接線）播放時，第一個可用樣本可辨別，且其 EOF 轉場重新
    /// 初始化全新 gap、轉場樣本為 0。
    #[test]
    fn test_abort_handoff_resets_ending_phase_then_new_source() {
        let core = core_with_eq();
        core.eq_shared.update(|p| p.gap_ms = 100);
        core.set_playback_state(PlaybackState::Playing);
        install_memory_playback(&core, sine_track(44100, 2, 0.1, 1, 10, 1));

        let _played = drive_until_waiting(&core, 50_000);
        assert!(core.handoff_ending_active());

        // 部分消耗 gap 後，於明確邊界中止交接
        for _ in 0..100 {
            assert_eq!(core.next().unwrap(), 0.0);
        }
        assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 9_499);

        core.abort_handoff();
        assert!(!core.handoff_ending_active(), "abort 應結束收尾階段");
        assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 0, "abort 應歸零 gap");
        assert_eq!(core.read_handoff_snapshot(|| ()), Some(()));

        // 模擬 Load 於明確邊界安裝新來源：新鏈/新重取樣器/RateStage + Playing
        install_memory_playback(&core, distinct_sine_track(44100, 2, 0.1, 2, 20, 2));
        core.set_playback_state(PlaybackState::Playing);

        // 新來源開始播放：第一個可用樣本應可辨別（|s|>0.5 僅新軌振幅可達；舊軌振幅 ≤ 0.3）
        let mut found_usable = false;
        for _ in 0..20_000 {
            let s = core.next().unwrap();
            if s.abs() > 0.5 {
                found_usable = true;
                break;
            }
        }
        assert!(found_usable, "abort 後新來源應輸出可辨別的第一個可用樣本");
        assert_eq!(core.now_token(), 20);
        assert!(!core.handoff_ending_active(), "新軌接手後收尾階段應結束");

        // 新來源播到 EOF：應重新建立收尾階段並初始化全新 gap（證明 abort 已重置 tail 狀態），
        // 且轉場樣本為 0（gap 的第一個靜音樣本）
        let mut transition_sample = None;
        for _ in 0..50_000 {
            let before = core.playback_state();
            let s = core.next().unwrap();
            let after = core.playback_state();
            if before == PlaybackState::Playing && after == PlaybackState::WaitingForNext {
                transition_sample = Some(s);
                break;
            }
        }
        assert_eq!(transition_sample, Some(0.0), "新交接的轉場樣本應為 gap 的第一個靜音");
        assert!(core.handoff_ending_active(), "新來源 EOF 應重新建立收尾階段");
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            9_599,
            "abort 後的新交接應重新初始化全新 gap"
        );
    }

    /// B1 修正：EOF 短轉場受 guard 保護——並發 pause 不得被覆寫為 WaitingForNext，
    /// 也不得初始化 gap；恢復後 gap 從原值續用，不重啟。
    #[test]
    fn test_eof_transition_guarded_against_concurrent_pause() {
        let core = core_with_eq();
        core.eq_shared.update(|p| p.gap_ms = 100);
        core.set_playback_state(PlaybackState::Playing);
        install_memory_playback(&core, sine_track(44100, 2, 0.1, 1, 10, 1));

        let played_at_transition = drive_until_waiting(&core, 50_000);
        assert_eq!(core.fade_gate.lock().unwrap().gap_remaining, 9_599);

        // 模擬「排水開始後、轉場前」命令執行緒暫停：以 Paused 直接呼叫排水路徑，
        // guarded 轉場不得覆寫狀態、不得初始化 gap、不得變更收尾階段。
        core.set_playback_state(PlaybackState::Paused);
        assert!(matches!(core.next_playing(core.state_gen()), PlaybackSample::Audio(sample) if sample == 0.0));
        assert_eq!(core.playback_state(), PlaybackState::Paused, "guard 不得覆寫並發暫停");
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            9_599,
            "被 guard 攔下的轉場不得初始化或重啟 gap"
        );
        assert!(core.handoff_ending_active());

        // 恢復後 gap 從原值續用（下一次呼叫 9_598），證明未重啟
        core.set_playback_state(PlaybackState::Playing);
        assert_eq!(core.next().unwrap(), 0.0);
        assert_eq!(
            core.fade_gate.lock().unwrap().gap_remaining,
            9_598,
            "恢復後應續用原 gap，不得重啟"
        );
        assert_eq!(core.played_out_frames(), played_at_transition, "gap 期間不得推進舊進度");
    }
}
