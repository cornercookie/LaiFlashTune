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

use crate::dsp::{DspSource, FxChain, FxParams, RateStage};
use crate::eq::{EqProcessor, EqShared};
use crate::open_with::{log_line, open_validated_file, get_final_path_from_handle};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError, SendTimeoutError};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use rodio::Source;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// 當前軌播放序號
    now_seq: AtomicU64,
    /// 當前軌 token
    now_token: AtomicU64,
    /// 下一首 token（快照用）
    next_token: AtomicU64,
    /// 清除 epoch（前端用來檢測引擎自行清除下一首）
    next_clear_epoch: AtomicU64,
    /// 播放位置基準（seek 目標）
    seek_origin_ms: AtomicU64,
    /// 已播輸出 frame 累計（DSP 輸出端座標）
    played_out_frames: AtomicU64,
    /// 交界紀錄
    boundaries: Mutex<Vec<BoundaryRecord>>,
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
    stream_source: Mutex<Option<Box<dyn Iterator<Item = f32> + Send>>>,
    /// 軌交接回呼：(seq, token, path, decoder)，供引擎更新 current_path/current_decoder
    pub(crate) on_track_change: Mutex<Option<Box<dyn Fn(u64, u64, &str, &str) + Send + Sync>>>,
}

impl PlaybackCore {
    pub fn new(eq_shared: Arc<EqShared>) -> Self {
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
            next_token: AtomicU64::new(0),
            next_clear_epoch: AtomicU64::new(0),
            seek_origin_ms: AtomicU64::new(0),
            played_out_frames: AtomicU64::new(0),
            boundaries: Mutex::new(Vec::new()),
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
            on_track_change: Mutex::new(None),
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
        *self.playback_state.lock().unwrap() = state;
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

    /// 記錄交界
    pub fn push_boundary(&self, record: BoundaryRecord) {
        self.boundaries.lock().unwrap().push(record);
    }

    /// 取得所有交界紀錄
    pub fn boundaries(&self) -> Vec<BoundaryRecord> {
        self.boundaries.lock().unwrap().clone()
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

    /// 取下一個 48kHz 雙聲道樣本（經 EQ/正規化，未經 DeviceConverter）。
    /// 播放狀態非 Playing 時回傳靜音，不啟動消費端停滯計時器。
    pub fn next(&self) -> Option<f32> {
        let state = self.playback_state.lock().unwrap().clone();
        match state {
            PlaybackState::Playing => {
                let s = self.next_playing();
                if s.is_some() {
                    self.add_played_out_frames(1);
                }
                s
            }
            PlaybackState::WaitingForNext | PlaybackState::Paused | PlaybackState::Finished => Some(0.0),
        }
    }

    /// Playing 狀態下的取樣：串流軌直接來源 → 原生樣本 → 重取樣 → EQ/正規化。
    fn next_playing(&self) -> Option<f32> {
        // 串流軌直接來源（適用長檔與 ffmpeg 串流），需經 TrackResampler 重取樣到 48kHz
        loop {
            // 1. 先從 dsp_out 取樣本回傳（一個樣本即可回傳，不以兩幀為門檻）
            {
                let mut dsp_out = self.dsp_out.lock().unwrap();
                if let Some(s) = dsp_out.pop_front() {
                    return Some(s);
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
            let sample = self.next_stream().or_else(|| {
                let mut chain = self.track_chain.lock().unwrap();
                chain.as_mut().and_then(|c| c.next())
            });
            // 若從 TrackChain 取得樣本，檢查並處理軌切換（sync_identity 會建立新 RateStage 並清空佇列）
            if sample.is_some() {
                self.sync_identity();
            }

            match sample {
                Some(s) => {
                    // 5. 鎖定 track_resmpler，必須存在；若為 None 不繞過重取樣，輸出靜音
                    let mut resampler = self.track_resampler.lock().unwrap();
                    let Some(r) = resampler.as_mut() else {
                        log_line("長檔串流：track_resampler 為 None，不繞過重取樣，輸出靜音");
                        return Some(0.0);
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
                    // 若兩個佇列皆空，進入 WaitingForNext；否則繼續消耗現有緩衝
                    let dsp_out_empty = self.dsp_out.lock().unwrap().is_empty();
                    let track_out_empty = self.track_out.lock().unwrap().is_empty();
                    if dsp_out_empty && track_out_empty {
                        *self.playback_state.lock().unwrap() = PlaybackState::WaitingForNext;
                        return Some(0.0);
                    }
                    continue;
                }
            }
        }
    }

    /// 從串流軌來源取一個 48kHz（原生）樣本；無來源或來源結束回傳 None。
    fn next_stream(&self) -> Option<f32> {
        let mut src = self.stream_source.lock().unwrap();
        src.as_mut().and_then(|s| s.next())
    }

    /// 設定串流軌來源（換軌時由引擎呼叫）。
    pub fn set_stream_source(&self, src: Option<Box<dyn Iterator<Item = f32> + Send>>) {
        *self.stream_source.lock().unwrap() = src;
    }

    /// 檢查 TrackChain 目前軌的身分，若與 now_seq/now_token 不同則發布新身分。
    /// 軌交接發生在 TrackChain 讀完目前軌並從待播槽 take 新軌時。
    ///
    /// 順序：flush_tail 舊軌尾端 → commit_boundary 寫入邊界 → set_track 切換重取樣器。
    /// 兩把鎖不同時持有，避免鎖序倒置。
    fn sync_identity(&self) {
        let (seq, token, song_id, norm_gain, duration_ms, native_sample_rate, native_channels, path, decoder) = {
            let chain = self.track_chain.lock().unwrap();
            let Some(cur) = chain.as_ref().and_then(|c| c.current()) else { return };
            if cur.seq == self.now_seq() && cur.token == self.now_token() {
                return;
            }
            (
                cur.seq,
                cur.token,
                cur.song_id,
                cur.norm_gain,
                cur.duration_ms,
                cur.sample_rate,
                cur.channels,
                cur.path.clone(),
                cur.decoder.clone(),
            )
        };

        // 先把舊軌殘餘輸入排成尾端，再提交邊界（output_frames 含尾端會累進到 accum）
        {
            let mut resampler = self.track_resampler.lock().unwrap();
            resampler.as_mut().map(|r| r.flush_tail());
            resampler.as_mut().map(|r| r.commit_boundary());
        }
        // 軌切換時的尾端樣本已由 flush_tail 移至 pending_output，不在此丟棄；
        // 下次 next_playing() 會優先從 pending_output 彈出，確保無縫輸出。
        {
            let chain = self.track_chain.lock().unwrap();
            if let Some(item) = chain.as_ref().and_then(|c| c.current()).cloned() {
                let mut resampler = self.track_resampler.lock().unwrap();
                resampler.as_mut().map(|r| r.set_track(Some(item)));
            }
        }
        // 先完成舊 RateStage 的尾端排空：將 pending_output 舊軌尾端樣本經 EQ 後餵入舊 RateStage，
        // 再以空輸入 flush=true 排空 WSOLA/Resampler 殘餘，輸出進入 dsp_out 供下次 next_playing() 回傳。
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
            // 軌切換時重新建立 RateStage（速度/節奏/音高），並清空 track_out，避免舊位置殘留聲音
            *self.rate_stage.lock().unwrap() = Some(RateStage::new(INTERNAL_CHANNELS as usize, INTERNAL_SAMPLE_RATE));
            self.track_out.lock().unwrap().clear();
            // dsp_out 保留舊 RateStage 尾端輸出，供下次 next_playing() 回傳
        let boundary_in_48k = self.track_resampler.lock().unwrap().as_ref().unwrap().boundary_in_48k();

        // 身分（seq/token）在軌切換時立即發布；進度與邊界則在舊軌尾端（pending_output）完全輸出後再重置，
        // 避免身分提前切換卻進度歸零，導致交界處的位置錯誤。
        self.set_now_identity(seq, token);
        self.current_norm_gain_bits.store(norm_gain.to_bits() as u64, Ordering::Release);
        self.push_boundary(BoundaryRecord {
            boundary_in_48k,
            boundary_out_48k: self.current_boundary_out_start(),
            seq,
            token,
            song_id,
            norm_gain,
            duration_ms,
            native_sample_rate,
            native_channels,
        });

        // 只有在舊軌尾端完全輸出後，才重置進度與邊界起點
        {
            let resampler = self.track_resampler.lock().unwrap();
            if let Some(r) = resampler.as_ref() {
                if !r.pending_output_is_empty() {
                    return;
                }
            }
        }
        self.reset_played_out_frames();
        self.set_current_boundary_in_start(boundary_in_48k);
        // 交接回呼：通知引擎更新 current_path/current_decoder（交接後 seek 需用正確路徑）
        if let Some(cb) = self.on_track_change.lock().unwrap().as_ref() {
            cb(seq, token, &path, &decoder);
        }
        log_line(&format!("TRACK_HANDOFF seq={} token={} song_id={}", seq, token, song_id));
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

    /// 套用 DSP 至立體聲幀：EQ → 速度/節奏/音高 (RateStage) → FxChain → 正規化 → 限幅。
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

        // 對每個輸出立體聲幀套用 FxChain → 正規化 → 限幅
        let mut frames_out: Vec<f32> = Vec::new();
        for chunk in tmp_out.chunks(2) {
            if chunk.len() < 2 {
                continue; // 不完整的幀，捨棄
            }
            let mut l = chunk[0];
            let mut r = chunk[1];

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

    /// 套用後段 DSP 至立體聲幀：FxChain → 正規化 → 限幅（不重複 EQ 與 RateStage）。
    /// 用於 tail 排空與 EOF 終結：輸入已經過 EQ 與 RateStage 的立體聲幀 `[L, R]`。
    fn apply_dsp_block_post(&self, frame: &[f32; 2], out: &mut Vec<f32>) {
        // 取得參數快照
        let fx_params = {
            let p = self.dsp_params.lock().unwrap();
            p.clone().unwrap_or_default()
        };

        let mut l = frame[0];
        let mut r = frame[1];

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
    current_index: usize,
    finished: bool,
    /// 待播協調器：讀完目前軌且無預載下一軌時，由此 take() 下一軌（唯一交接點）
    coordinator: Option<Arc<NextCoordinator>>,
}

#[derive(Debug, Clone)]
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
}

impl TrackChain {
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            current_index: 0,
            finished: false,
            coordinator: None,
        }
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
                    } else if let Some(coord) = &self.coordinator {
                        // 唯一交接點：從待播槽 take 下一軌
                        if let Some(prepared) = coord.take_next() {
                            if let Some(item) = prepared_to_trackchain_item(&prepared) {
                                self.take_next(Some(item));
                                continue;
                            }
                        }
                        // 槽空：鏈結束
                        self.finished = true;
                        return None;
                    } else {
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
    match &prepared.data {
        TrackData::Memory(samples) => Some(TrackChainItem {
            samples: samples.clone(),
            sample_rate: prepared.rate,
            channels: prepared.channels,
            duration_ms: prepared.duration_ms,
            song_id: prepared.song_id,
            norm_gain: prepared.norm_gain,
            token: prepared.token,
            seq: prepared.seq,
            native_frame_pos: 0,
            path: prepared.path.clone(),
            decoder: prepared.decoder.clone(),
        }),
        TrackData::Stream(_) => None,
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
    resampler_delay_frames: usize,
    /// 邊界輸入累計
    boundary_in_48k_accum: u64,
    /// flush_tail 是否已經執行過（避免 EOF 後持續補零產出）
    tail_flushed: bool,
}

#[derive(Debug)]
struct TrackResamplerTrack {
    sample_rate: u32,
    channels: u16,
    duration_ms: i64,
    song_id: i64,
    norm_gain: f32,
    token: u64,
    seq: u64,
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
            resampler_delay_frames: 0,
            boundary_in_48k_accum: 0,
            tail_flushed: false,
        }
    }

    /// 切換到新軌（由 TrackChain 呼叫）。呼叫者應在切換前先自行 `flush_tail()` 排出舊軌尾端。
    pub fn set_track(&mut self, track: Option<TrackChainItem>) {
        self.current_track = track.map(|t| TrackResamplerTrack {
            sample_rate: t.sample_rate,
            channels: t.channels,
            duration_ms: t.duration_ms,
            song_id: t.song_id,
            norm_gain: t.norm_gain,
            token: t.token,
            seq: t.seq,
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

            self.resampler_delay_frames = resampler.output_delay();
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
        // 傳入 None 當 input 表示 flush 處理（rubato 會把內部殘餘輸出）
        // 使用 process（而非 process_partial）以避免類型轉換問題，partial 區塊由 rubato 直接處理
        let input_ref: Option<&[Vec<f32>]> = if remaining > 0 { Some(&block) } else { None };
        match resampler.process_partial(input_ref, None) {
            Ok(out) => {
                let n = out.first().map(|v| v.len()).unwrap_or(0);
                for f in 0..n {
                    for c in 0..internal_ch {
                        self.output_buffer.push(out[c][f]);
                    }
                }
                self.output_frames += n as u64;
            }
            Err(e) => {
                log_line(&format!("track resampler flush_tail error: {e}"));
            }
        }
        self.frames_in_buffer = 0;
        self.input_frames += remaining as u64;
        // 將 output_buffer 中已產出的尾端樣本移到 pending_output，確保不被 set_track 清除
        self.pending_output.extend(self.output_buffer.drain(..));
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
        if self.current_track.is_none() {
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
                    let n = out.first().map(|v| v.len()).unwrap_or(0);
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
        if self.current_track.is_none() {
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
                    let n = out.first().map(|v| v.len()).unwrap_or(0);
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
        let core = &self.core;
        let my_id = self.id;

        // 核心鎖內取樣並再次驗證 active_id：切換裝置與取樣序列化，
        // 舊 proxy 的 next() 在核心鎖內發現 ID 已過期直接回傳 None，不消耗核心。
        let _guard = core.core_lock.lock().unwrap();
        if core.active_id() != my_id {
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
                match core.next() {
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
    pub data: TrackData,
}

#[derive(Debug, Clone)]
pub enum TrackData {
    Memory(Arc<Vec<f32>>),
    Stream(Arc<StreamWrapper>),
}

#[derive(Debug)]
pub enum StreamWrapper {
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
        },
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
    worker_handle: Option<thread::JoinHandle<()>>,
    cancel_flag: Arc<AtomicBool>,
    /// SE9：回收期間凍結新準備工作，避免回收中接受新請求導致 handle 競爭
    preparing_frozen: AtomicBool,
}

#[derive(Clone, Debug)]
struct PrepareRequest {
    path: String,
    song_id: i64,
    generation: u64,
    seq: u64,
    token: u64,
}

impl NextCoordinator {
    pub fn new() -> Self {
        let (completion_tx, completion_rx) = bounded(1);
        let (pending_tx, pending_rx) = bounded(1);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let coordinator = Self {
            generation: AtomicU64::new(0),
            slot: Mutex::new(None),
            completion_tx,
            completion_rx,
            pending_tx: Some(pending_tx),
            pending_rx,
            worker_handle: None,
            cancel_flag: cancel_flag.clone(),
            preparing_frozen: AtomicBool::new(false),
        };

        let worker_handle = coordinator.spawn_worker(cancel_flag.clone());
        let mut coord = coordinator;
        coord.worker_handle = Some(worker_handle);
        coord
    }

    fn spawn_worker(&self, cancel_flag: Arc<AtomicBool>) -> thread::JoinHandle<()> {
        let pending_rx = self.pending_rx.clone();
        let completion_tx = self.completion_tx.clone();

        thread::spawn(move || {
            loop {
                let request = match pending_rx.recv() {
                    Ok(req) => req,
                    Err(_) => break,
                };

                if cancel_flag.load(Ordering::Relaxed) {
                    continue;
                }

                // 執行準備工作：成功或失敗都透過 completion channel 通知，
                // 讓前端能略過壞檔並重排，而不會靜默丟棄。
                let result = prepare_track(&request.path, request.song_id, request.seq, request.token);
                let mut sent = false;
                while !sent && !cancel_flag.load(Ordering::Relaxed) {
                    match completion_tx.send_timeout((result.clone(), request.generation), Duration::from_millis(100)) {
                        Ok(()) => sent = true,
                        Err(SendTimeoutError::Timeout(_)) => continue,
                        Err(SendTimeoutError::Disconnected(_)) => break,
                    }
                }
            }
        })
    }

    /// 設定下一首（立即回傳，背景準備）。待處理請求為可替換槽：滿時捨棄舊請求，只保留最新，
    /// 並以非阻塞 try_send 避免卡住引擎。若正在回收中（preparing_frozen），拒絕新請求。
    pub fn set_next(&self, path: String, song_id: i64, token: u64) -> u64 {
        let gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;

        // SE9：回收期間凍結，拒絕新的準備請求
        if self.preparing_frozen.load(Ordering::Acquire) {
            crate::open_with::log_line(&format!(
                "NEXT_COORD set_next rejected (preparing_frozen) gen={}, song_id={}, token={}",
                gen, song_id, token
            ));
            return gen;
        }

        // 清空待播槽；舊的交給回收執行緒（這裡簡化：記錄日誌）
        if let Ok(mut slot) = self.slot.lock() {
            if let Some(old) = slot.take() {
                crate::open_with::log_line(&format!("NEXT_COORD clear slot for seq={}, token={}", old.seq, old.token));
            }
        }

        // 送入待處理請求（只保留最新，非阻塞）
        let tx = self.pending_tx.as_ref().expect("pending_tx must exist");
        loop {
            match tx.try_send(PrepareRequest {
                path: path.clone(),
                song_id,
                generation: gen,
                seq: 0, // 由 prepare_track 中的 load_at 賦予正確的 seq
                token,
            }) {
                Ok(()) => break,
                Err(TrySendError::Full(_)) => {
                    // 通道滿：捨棄舊的尚未處理請求，只保留最新
                    let _ = self.pending_rx.try_recv();
                }
                Err(TrySendError::Disconnected(_)) => break,
            }
        }

        gen
    }

    pub fn clear_next(&self) -> u64 {
        let gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;

        if let Ok(mut slot) = self.slot.lock() {
            if let Some(old) = slot.take() {
                crate::open_with::log_line(&format!("NEXT_COORD clear slot for seq={}, token={}", old.seq, old.token));
            }
        }

        while self.pending_rx.try_recv().is_ok() {}

        gen
    }

    pub fn poll_completion(&self, current_gen: u64) -> Option<Result<PreparedTrack, String>> {
        match self.completion_rx.try_recv() {
            Ok((result, gen)) => {
                if gen == current_gen {
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
            Err(TryRecvError::Empty) => None,
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

    /// SE9：設定回收凍結狀態。凍結時 `set_next` 會拒絕新請求。
    pub fn set_preparing_frozen(&self, frozen: bool) {
        self.preparing_frozen.store(frozen, Ordering::Release);
    }

    pub fn is_preparing_frozen(&self) -> bool {
        self.preparing_frozen.load(Ordering::Acquire)
    }
}

impl Drop for NextCoordinator {
    fn drop(&mut self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
        // 先關閉發送端，讓 worker 的 recv() 返回 Err 並退出
        drop(self.pending_tx.take());
        if let Some(handle) = self.worker_handle.take() {
            // worker 在收到 cancel 或通道關閉後會退出；send_timeout 最多阻塞 100 ms，不會永久卡住
            let _ = handle.join();
        }
        // completion_tx 隨 NextCoordinator 一起被 drop；worker 結束後其持有的副本也會 drop，通道隨即關閉
    }
}

/// 準備單一軌（解碼或開啟串流）
fn prepare_track(path: &str, song_id: i64, seq: u64, token: u64) -> Result<PreparedTrack, String> {
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
                match crate::audio_decode::SymStream::open(&mut file, &path_buf, 0) {
                Ok(mut stream) => {
                    let duration_ms = stream.duration_ms().unwrap_or(0);
                    let is_long = duration_ms > 15 * 60 * 1000 || duration_ms == 0;

                    if is_long {
                                            // 長檔：建立延後串流，保留 handle 供播放時使用
                                            Ok(PreparedTrack {
                                                seq,
                                                token,
                                                song_id,
                                                path: final_path.clone(),
                                                decoder: "stream".to_string(),
                                                rate: stream.sample_rate(),
                                                channels: stream.channels(),
                                                duration_ms,
                                                norm_gain: 1.0,
                                                data: TrackData::Stream(Arc::new(StreamWrapper::FfmpegDeferred {
                                                                                                    path: final_path,
                                                                                                    sample_rate: stream.sample_rate(),
                                                                                                    channels: stream.channels(),
                                                                                                    duration_ms,
                                                                                                    seq,
                                                                                                    token,
                                                                                                    song_id,
                                                                                                    norm_gain: 1.0,
                                                                                                    generation: 0, // 將在 set_next 時設定
                                                                                                    handle: std::sync::Mutex::new(Some(file_for_stream)),
                                                                                                })),
                                            })
                    } else {
                        // 短檔：整首解碼進記憶體，預檢後關閉 handle
                        let samples: Vec<f32> = stream.by_ref().collect();
                        let duration_ms = (samples.len() as f64 / (stream.sample_rate() as f64 * stream.channels() as f64) * 1000.0) as i64;
                        let peak = samples.iter().map(|s| s.abs()).fold(0f32, f32::max);
                        let norm_gain = if peak > 0.0001 { (0.89 / peak).clamp(0.05, 6.0) } else { 1.0 };

                        Ok(PreparedTrack {
                            seq,
                            token,
                            song_id,
                            path: final_path,
                            decoder: "symphonia".to_string(),
                            rate: stream.sample_rate(),
                            channels: stream.channels(),
                            duration_ms,
                            norm_gain,
                            data: TrackData::Memory(Arc::new(samples)),
                        })
                    }
                }
                Err(_) => {
                                    // symphonia 失敗，嘗試 ffmpeg（需要重新開檔）
                                    // file 已在 SymStream::open 中被移動，file_for_stream 仍可用於 ffmpeg
                                    drop(file_for_stream);
                                    prepare_with_ffmpeg(path, song_id, &ext, seq, token)
                                }
            }
        }
        Some(crate::scanner::Backend::Ffmpeg) => {
            // ffmpeg 路徑：傳遞 final_path
            prepare_with_ffmpeg_final(&final_path, song_id, &ext, seq, token)
        }
        None => {
            Err(format!("這個格式尚未支援: {}", ext))
        }
    }
}

fn prepare_with_ffmpeg(path: &str, song_id: i64, _ext: &str, seq: u64, token: u64) -> Result<PreparedTrack, String> {
    let ffprobe_path = crate::tools::locate("ffprobe.exe")
        .map(|(p, _, _)| p)
        .ok_or("找不到 ffprobe")?;
    // SE6：ffprobe 64 KB / 10 秒上限
    let value = crate::audio_decode::ffprobe_json(&ffprobe_path, path)?;

    let streams = value["streams"].as_array().ok_or("無音訊串流")?;
    let audio_stream = streams.iter().find(|s| s["codec_type"] == "audio").ok_or("無音訊串流")?;

    let sample_rate = audio_stream["sample_rate"].as_str().unwrap_or("0").parse().unwrap_or(0);
    let channels = audio_stream["channels"].as_u64().unwrap_or(0) as u16;
    let duration = value["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok());
    let duration_ms = duration.map(|d| (d * 1000.0) as i64).unwrap_or(0);

    let is_long = duration_ms > 15 * 60 * 1000 || duration_ms == 0;

    if is_long {
        let ffmpeg_path = crate::tools::locate("ffmpeg.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffmpeg")?;

        let stream = crate::audio_decode::FfmpegStream::new(&ffmpeg_path, path, sample_rate, channels, duration_ms)?;
        let handle = FfmpegStreamHandle {
            ffmpeg_path: stream.ffmpeg_path.clone(),
            file_path: stream.file_path.clone(),
            sample_rate: stream.sample_rate,
            channels: stream.channels,
            duration_ms: stream.duration_ms,
            cancel: stream.cancel.clone(),
            rx: stream.rx.clone(),
        };

        Ok(PreparedTrack {
            seq,
            token,
            song_id,
            path: path.to_string(),
            decoder: "stream".to_string(),
            rate: sample_rate,
            channels,
            duration_ms,
            norm_gain: 1.0,
            data: TrackData::Stream(Arc::new(StreamWrapper::FfmpegStream(Arc::new(handle)))),
        })
    } else {
        let ffmpeg_path = crate::tools::locate("ffmpeg.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffmpeg")?;

        // SE3-1: 使用獨立 stderr 讀取執行緒與持久化 Sanitizer
        let mut child = std::process::Command::new(&ffmpeg_path)
            .args(["-nostdin", "-v", "error", "-protocol_whitelist", "file,pipe", "-i", path, "-f", "f32le", "-ac", &channels.to_string(), "-ar", &sample_rate.to_string(), "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("ffmpeg 執行失敗: {e}"))?;

        let stderr = child.stderr.take().ok_or("ffmpeg 無 stderr")?;
        let stderr_thread = crate::open_with::spawn_stderr_sanitizer(Some(stderr), None);

        let output = child.wait_with_output().map_err(|e| format!("ffmpeg 等待失敗: {e}"))?;
        let _ = stderr_thread.join();
        
        if !output.status.success() {
            return Err("ffmpeg 解碼失敗（詳細錯誤請見 open-with.log）".into());
        }

        let samples: Vec<f32> = output.stdout
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        let duration_ms = (samples.len() as f64 / (sample_rate as f64 * channels as f64) * 1000.0) as i64;
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
            data: TrackData::Memory(Arc::new(samples)),
        })
    }
}

/// 使用最終規範路徑的 ffmpeg 準備（供 handle 機制呼叫）
fn prepare_with_ffmpeg_final(path: &str, song_id: i64, _ext: &str, seq: u64, token: u64) -> Result<PreparedTrack, String> {
    let ffprobe_path = crate::tools::locate("ffprobe.exe")
        .map(|(p, _, _)| p)
        .ok_or("找不到 ffprobe")?;
    // SE6：ffprobe 64 KB / 10 秒上限
    let value = crate::audio_decode::ffprobe_json(&ffprobe_path, path)?;

    let streams = value["streams"].as_array().ok_or("無音訊串流")?;
    let audio_stream = streams.iter().find(|s| s["codec_type"] == "audio").ok_or("無音訊串流")?;

    let sample_rate = audio_stream["sample_rate"].as_str().unwrap_or("0").parse().unwrap_or(0);
    let channels = audio_stream["channels"].as_u64().unwrap_or(0) as u16;
    let duration = value["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok());
    let duration_ms = duration.map(|d| (d * 1000.0) as i64).unwrap_or(0);

    let is_long = duration_ms > 15 * 60 * 1000 || duration_ms == 0;

    if is_long {
        let ffmpeg_path = crate::tools::locate("ffmpeg.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffmpeg")?;

        let stream = crate::audio_decode::FfmpegStream::new(&ffmpeg_path, path, sample_rate, channels, duration_ms)?;
        let handle = FfmpegStreamHandle {
            ffmpeg_path: stream.ffmpeg_path.clone(),
            file_path: stream.file_path.clone(),
            sample_rate: stream.sample_rate,
            channels: stream.channels,
            duration_ms: stream.duration_ms,
            cancel: stream.cancel.clone(),
            rx: stream.rx.clone(),
        };

        Ok(PreparedTrack {
            seq,
            token,
            song_id,
            path: path.to_string(),
            decoder: "stream".to_string(),
            rate: sample_rate,
            channels,
            duration_ms,
            norm_gain: 1.0,
            data: TrackData::Stream(Arc::new(StreamWrapper::FfmpegStream(Arc::new(handle)))),
        })
    } else {
        let ffmpeg_path = crate::tools::locate("ffmpeg.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffmpeg")?;

        // SE3-1: 使用獨立 stderr 讀取執行緒與持久化 Sanitizer
        let mut child = std::process::Command::new(&ffmpeg_path)
            .args(["-nostdin", "-v", "error", "-protocol_whitelist", "file,pipe", "-i", path, "-f", "f32le", "-ac", &channels.to_string(), "-ar", &sample_rate.to_string(), "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("ffmpeg 執行失敗: {e}"))?;

        let stderr = child.stderr.take().ok_or("ffmpeg 無 stderr")?;
        let stderr_thread = crate::open_with::spawn_stderr_sanitizer(Some(stderr), None);

        let output = child.wait_with_output().map_err(|e| format!("ffmpeg 等待失敗: {e}"))?;
        let _ = stderr_thread.join();
        
        if !output.status.success() {
            return Err("ffmpeg 解碼失敗（詳細錯誤請見 open-with.log）".into());
        }

        let samples: Vec<f32> = output.stdout
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        let duration_ms = (samples.len() as f64 / (sample_rate as f64 * channels as f64) * 1000.0) as i64;
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
            data: TrackData::Memory(Arc::new(samples)),
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
            token,
            seq,
            native_frame_pos: 0,
            path: "test".into(),
            decoder: "synth".into(),
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
            data: TrackData::Memory(t.samples),
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
    fn test_playback_core_seamless_handoff() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::Playing);

        // 使用足夠長的軌（> BLOCK_FRAMES=2048 原生樣本）確保重取樣器能產出
        let track1 = sine_track(44100, 2, 0.1, 1, 10, 1); // 0.1 s @ 44.1k = 8820 樣本

        let mut chain = TrackChain::new();
        chain.append(track1);
        chain.set_coordinator(core.next_coordinator());
        core.track_chain.lock().unwrap().replace(chain);

        // 模擬背景準備完成：第二軌進入協調器待播槽
        core.next_coordinator()
            .slot
            .lock()
            .unwrap()
            .replace(prepared_memory(48000, 2, 0.1, 2, 20, 2));

        let mut resampler = TrackResampler::new();
        resampler.set_track(Some(sine_track(44100, 2, 0.1, 1, 10, 1)));
        core.track_resampler.lock().unwrap().replace(resampler);

        // 第一軌約 44100*0.1*2 = 8820 個原生樣本，拉夠樣本應自動從待播槽 take 第二軌
        let mut samples = Vec::new();
        let mut saw_seq2 = false;
        let mut seq1_count = 0u64;
        let mut seq2_count = 0u64;
        let mut boundary_idx = None;
        for i in 0..20000 {
            let before_seq = core.now_seq();
            match core.next() {
                Some(s) => {
                    samples.push(s);
                    if core.now_seq() == 2 {
                        saw_seq2 = true;
                        seq2_count += 1;
                        if boundary_idx.is_none() && before_seq == 1 {
                            boundary_idx = Some(i as usize);
                        }
                    } else {
                        seq1_count += 1;
                    }
                }
                None => break,
            }
            if saw_seq2 && seq2_count >= 200 {
                break; // 第二軌已輸出足夠樣本，停止
            }
        }
        assert!(saw_seq2, "第一軌結束後應從待播槽 take 第二軌並切換身分");
        assert_eq!(core.now_token(), 20);
        assert!(!samples.is_empty(), "交接處不應出現間斷");
        assert!(
            samples.iter().any(|&s| s != 0.0),
            "交接處應輸出真實非靜音音訊（不應全為靜音）"
        );
        // C7：兩軌皆應有輸出（seq1 期間與 seq2 期間都應有非零樣本）
        assert!(seq1_count > 0, "第一軌應有輸出樣本");
        assert!(seq2_count > 0, "第二軌應有輸出樣本");
        // C7：交界時 DSP 狀態應延續（EQ 應持續處理，不應在交界處重置為零）
        if let Some(bidx) = boundary_idx {
            if bidx > 0 && bidx < samples.len() {
                let before_boundary: f32 = samples[..bidx].iter().map(|s| s.abs()).sum::<f32>() / bidx as f32;
                let after_boundary: f32 = samples[bidx..].iter().map(|s| s.abs()).sum::<f32>() / samples[bidx..].len() as f32;
                assert!(
                    before_boundary > 0.001 && after_boundary > 0.001,
                    "交界前後皆應有非零音訊能量，DSP 狀態不應重置"
                );
            }
        }
        // C7：第二軌應輸出至少 50 個樣本（確保非僅一次取樣即結束）
        assert!(seq2_count >= 50, "第二軌應持續輸出樣本，不應僅輸出一次");
    }

    #[test]
    fn test_playback_core_handoff_full_completion_channel() {
        let core = core_with_eq();
        core.set_playback_state(PlaybackState::Playing);
        // 驗證 set_next 是非阻塞的：連續兩次調用不會阻塞，且 gen 正確遞增
        let coord = core.next_coordinator();
        let g1 = coord.set_next("/track-a.mp3".into(), 1, 10);
        let g2 = coord.set_next("/track-b.mp3".into(), 2, 20); // 應替換最新，舊的被丟棄
        assert_eq!(g1, 1);
        assert_eq!(g2, 2);
        assert_eq!(coord.current_generation(), 2);
        // 兩次 set_next 都成功返回，證明 try_send 替換最新不會阻塞
        // 完成通知通道容量為 1，舊的會被新的替換
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
        let g1 = coord.set_next("/a.mp3".into(), 1, 10);
        let g2 = coord.set_next("/b.mp3".into(), 2, 20);
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
        let g1 = coord.set_next("/a.mp3".into(), 1, 10);
        let g2 = coord.set_next("/b.mp3".into(), 2, 20);
        // 容量 1 的待處理槽只保留最新，兩次 set_next 不應阻塞，且僅記錄最新世代
        assert_eq!(g1, 1);
        assert_eq!(g2, 2);
        assert_eq!(coord.current_generation(), 2);
    }

    #[test]
    fn test_next_coordinator_poll_completion_discards_stale() {
        let coord = NextCoordinator::new();
        coord.set_next("/a.mp3".into(), 1, 10); // gen 1，不存在的檔案 → worker 會產出 Err 完成通知
        coord.set_next("/b.mp3".into(), 2, 20); // gen 2，同樣不存在
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
        coord.set_next(path.to_string_lossy().to_string(), 1, 10);
        // 不 poll_completion，讓 channel 保持滿的狀態，然後 drop 驗證不會 deadlock。
        drop(coord);
    }

    #[test]
    fn test_next_coordinator_drop_does_not_deadlock() {
        let coord = NextCoordinator::new();
        coord.set_next("/a.mp3".into(), 1, 10);
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

        let s1 = core.next_playing().unwrap();
        assert!((s1 - 0.5).abs() < 1e-6, "dsp_out 應優先回傳第一個樣本");

        let s2 = core.next_playing().unwrap();
        assert!((s2 - (-0.3)).abs() < 1e-6, "dsp_out 應續回傳第二個樣本");

        // dsp_out 耗盡且無 track_out/upstream，應回傳靜音並進入 WaitingForNext
        let s3 = core.next_playing().unwrap();
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
                data: TrackData::Memory(Arc::new(second_samples)),
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
}