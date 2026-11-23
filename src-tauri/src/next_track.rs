//! 背景準備與待播槽（P0-3）。
//!
//! `NextCoordinator` 持有「排入世代」計數與待播槽 `Mutex<Option<PreparedTrack>>`，不依賴輸出裝置，可以單獨測試。
//! `set_next` 與 `clear_next` 都把世代加一，並清空待播槽；被清掉的 `PreparedTrack` 交給回收執行緒。
//! `set_next`：worker 從「待處理請求」槽取出最新請求，產出 `PreparedTrack`，連同自己的世代送進完成通道。
//! 引擎迴圈收到完成通知時，世代不是最新的就丟給回收執行緒；是最新的才放進待播槽，快照的 `next_token` 變成這個 token。

use crate::audio_decode::{SymStream, FfmpegStream};
use crate::open_with::{validate_local_path, log_line};
use crate::tools::locate;
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use rodio::Source;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// 準備完成的軌資訊
#[derive(Clone, Debug)]
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

pub enum TrackData {
    Memory(Arc<Vec<f32>>),
    Stream(Arc<StreamWrapper>),
}

impl Clone for TrackData {
    fn clone(&self) -> Self {
        match self {
            TrackData::Memory(v) => TrackData::Memory(v.clone()),
            TrackData::Stream(s) => TrackData::Stream(s.clone()),
        }
    }
}

impl std::fmt::Debug for TrackData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrackData::Memory(v) => f.debug_tuple("Memory").field(&v.len()).finish(),
            TrackData::Stream(s) => f.debug_tuple("Stream").field(s).finish(),
        }
    }
}

impl StreamSource for Arc<StreamWrapper> {
    fn open(&self, start_ms: i64) -> Result<Box<dyn Iterator<Item = f32> + Send>, String> {
        StreamWrapper::open(self, start_ms)
    }
    fn sample_rate(&self) -> u32 {
        StreamWrapper::sample_rate(self)
    }
    fn channels(&self) -> u16 {
        StreamWrapper::channels(self)
    }
    fn duration_ms(&self) -> Option<i64> {
        StreamWrapper::duration_ms(self)
    }
}

/// 串流音源的統一介面（供 PreparedTrack 使用）
pub trait StreamSource: Send + Sync {
    fn open(&self, start_ms: i64) -> Result<Box<dyn Iterator<Item = f32> + Send>, String>;
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn duration_ms(&self) -> Option<i64>;
}

/// 用於串流模式的包裝器，持有路徑並能重新開啟
#[derive(Debug, Clone)]
enum StreamWrapper {
    SymStream { path: String, sample_rate: u32, channels: u16, duration_ms: i64 },
    FfmpegStream(FfmpegStream),
}

impl StreamSource for StreamWrapper {
    fn open(&self, start_ms: i64) -> Result<Box<dyn Iterator<Item = f32> + Send>, String> {
        match self {
            StreamWrapper::SymStream { path, .. } => {
                SymStream::open(Path::new(path), start_ms).map(|s| Box::new(s) as Box<dyn Iterator<Item = f32> + Send>)
            }
            StreamWrapper::FfmpegStream(s) => s.open_with_seek(start_ms),
        }
    }
    fn sample_rate(&self) -> u32 {
        match self {
            StreamWrapper::SymStream { sample_rate, .. } => *sample_rate,
            StreamWrapper::FfmpegStream(s) => s.sample_rate(),
        }
    }
    fn channels(&self) -> u16 {
        match self {
            StreamWrapper::SymStream { channels, .. } => *channels,
            StreamWrapper::FfmpegStream(s) => s.channels(),
        }
    }
    fn duration_ms(&self) -> Option<i64> {
        match self {
            StreamWrapper::SymStream { duration_ms, .. } => Some(*duration_ms),
            StreamWrapper::FfmpegStream(s) => s.duration_ms(),
        }
    }
}

/// 下一首協調器
pub struct NextCoordinator {
    /// 排入世代計數
    generation: AtomicU64,
    /// 待播槽
    slot: Mutex<Option<PreparedTrack>>,
    /// 準備工作完成通道
    completion_tx: Sender<(PreparedTrack, u64)>,
    completion_rx: Receiver<(PreparedTrack, u64)>,
    /// 待處理請求槽（最新一筆）
    pending_tx: Option<Sender<PrepareRequest>>,
    pending_rx: Receiver<PrepareRequest>,
    /// 工作執行緒 handle
    worker_handle: Option<thread::JoinHandle<()>>,
    /// 取消旗標
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Clone, Debug)]
struct PrepareRequest {
    path: String,
    song_id: i64,
    generation: u64,
}

impl NextCoordinator {
    pub fn new() -> Self {
        let (completion_tx, completion_rx) = bounded(1);
        let (pending_tx, pending_rx) = bounded(1);
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        
        let coordinator = Self {
            generation: AtomicU64::new(0),
            slot: Mutex::new(None),
            completion_tx,
            completion_rx,
            pending_tx: Some(pending_tx),
            pending_rx,
            worker_handle: None,
            cancel_flag: cancel_flag.clone(),
        };
        
        // 啟動背景準備 worker
        let worker_handle = coordinator.spawn_worker(cancel_flag.clone());
        let mut coord = coordinator;
        coord.worker_handle = Some(worker_handle);
        coord
    }
    
    fn spawn_worker(&self, cancel_flag: Arc<std::sync::atomic::AtomicBool>) -> thread::JoinHandle<()> {
        let pending_rx = self.pending_rx.clone();
        let completion_tx = self.completion_tx.clone();
        
        thread::spawn(move || {
            loop {
                // 等待新請求
                let request = match pending_rx.recv() {
                    Ok(req) => req,
                    Err(_) => break, // 通道關閉，退出
                };
                
                if cancel_flag.load(Ordering::Relaxed) {
                    continue;
                }
                
                // 執行準備工作
                if let Ok(track) = prepare_track(&request.path, request.song_id) {
                    let _ = completion_tx.send((track, request.generation));
                }
            }
        })
    }
    
    /// 設定下一首（立即回傳，背景準備）
    pub fn set_next(&self, path: String, song_id: i64) -> u64 {
        let gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        
        // 清空待播槽
        if let Ok(mut slot) = self.slot.lock() {
            if let Some(old) = slot.take() {
                // 舊的交給回收執行緒（這裡簡化：記錄日誌）
                log_line(&format!("NEXT_COORD clear slot for seq={}, token={}", old.seq, old.token));
            }
        }
        
        // 送入待處理請求（只保留最新）
                let _ = self.pending_tx.as_ref().unwrap().send(PrepareRequest {
            path,
            song_id,
            generation: gen,
        });
        
        gen
    }
    
    /// 清除下一首
    pub fn clear_next(&self) -> u64 {
        let gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        
        if let Ok(mut slot) = self.slot.lock() {
            if let Some(old) = slot.take() {
                log_line(&format!("NEXT_COORD clear slot for seq={}, token={}", old.seq, old.token));
            }
        }
        
        // 清空待處理請求
        while self.pending_rx.try_recv().is_ok() {}
        
        gen
    }
    
    /// 檢查並更新待播槽（引擎迴圈呼叫）
    pub fn poll_completion(&self, current_gen: u64) -> Option<PreparedTrack> {
        match self.completion_rx.try_recv() {
            Ok((track, gen)) => {
                if gen == current_gen {
                    // 世代仍是最新的，放入待播槽
                    if let Ok(mut slot) = self.slot.lock() {
                        *slot = Some(track.clone());
                    }
                    Some(track)
                } else {
                    // 世代已過期，交給回收
                    log_line(&format!("NEXT_COORD discard stale preparation gen={} current={}", gen, current_gen));
                    None
                }
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => None,
        }
    }
    
    /// 取出待播軌（TrackChain 呼叫，唯一交接點）
    pub fn take_next(&self) -> Option<PreparedTrack> {
        if let Ok(mut slot) = self.slot.lock() {
            slot.take()
        } else {
            None
        }
    }
    
    /// 取得當前世代
    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    
    /// 檢查槽是否有軌
    pub fn has_next(&self) -> bool {
        self.slot.lock().map(|s| s.is_some()).unwrap_or(false)
    }
}

impl Drop for NextCoordinator {
    fn drop(&mut self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
        // 先關閉發送端，讓 worker 的 recv() 返回 Err 並退出
        drop(self.pending_tx.take());
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

/// 準備單一軌（解碼或開啟串流）
fn prepare_track(path: &str, song_id: i64) -> Result<PreparedTrack, String> {
    // S1: 驗證本機路徑
    validate_local_path(Path::new(path))?;
    
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    
    // 決定解碼後端
    let backend = crate::scanner::playback_backend_for(&ext);
    let meta_backend = crate::scanner::meta_backend_for(&ext);
    
    match backend {
        Some(crate::scanner::Backend::Symphonia) => {
            // 嘗試用 symphonia 開啟
            match SymStream::open(Path::new(path), 0) {
                Ok(mut stream) => {
                    let duration_ms = stream.duration_ms().unwrap_or(0);
                    let is_long = duration_ms > 15 * 60 * 1000 || duration_ms == 0;
                    
                    if is_long {
                        // 長檔案：串流模式 - 需要重新開啟檔案
                        Ok(PreparedTrack {
                            seq: 0, // 由引擎分配
                            token: 0, // 由引擎分配
                            song_id,
                            path: path.to_string(),
                            decoder: "stream".to_string(),
                            rate: stream.sample_rate(),
                            channels: stream.channels(),
                            duration_ms,
                            norm_gain: 1.0,
                            data: TrackData::Stream(Arc::new(StreamWrapper::SymStream { 
                                                            path: path.to_string(), 
                                                            sample_rate: stream.sample_rate(), 
                                                            channels: stream.channels(), 
                                                            duration_ms 
                                                        })),
                        })
                    } else {
                        // 短檔案：整首解碼進記憶體
                        let samples: Vec<f32> = stream.by_ref().collect();
                        let duration_ms = (samples.len() as f64 / (stream.sample_rate() as f64 * stream.channels() as f64) * 1000.0) as i64;
                        let peak = samples.iter().map(|s| s.abs()).fold(0f32, f32::max);
                        let norm_gain = if peak > 0.0001 { (0.89 / peak).clamp(0.05, 6.0) } else { 1.0 };
                        
                        Ok(PreparedTrack {
                            seq: 0,
                            token: 0,
                            song_id,
                            path: path.to_string(),
                            decoder: "symphonia".to_string(),
                            rate: stream.sample_rate(),
                            channels: stream.channels(),
                            duration_ms,
                            norm_gain,
                            data: TrackData::Memory(Arc::new(samples)),
                        })
                    }
                }
                Err(e) => {
                    // symphonia 失敗，嘗試 ffmpeg
                    prepare_with_ffmpeg(path, song_id, &ext)
                }
            }
        }
        Some(crate::scanner::Backend::Ffmpeg) => {
            prepare_with_ffmpeg(path, song_id, &ext)
        }
        None => {
            // 受阻格式
            Err(format!("這個格式尚未支援: {}", ext))
        }
    }
}

fn prepare_with_ffmpeg(path: &str, song_id: i64, ext: &str) -> Result<PreparedTrack, String> {
    // 使用 ffprobe 讀取中繼資料（SE6：64 KB / 10 秒上限）
    let ffprobe_path = crate::tools::locate("ffprobe.exe")
        .map(|(p, _, _)| p)
        .ok_or("找不到 ffprobe")?;
    let value = crate::audio_decode::ffprobe_json(&ffprobe_path, path)?;

    // 解析音訊串流資訊
    let streams = value["streams"].as_array().ok_or("無音訊串流")?;
    let audio_stream = streams.iter().find(|s| s["codec_type"] == "audio").ok_or("無音訊串流")?;
    
    let _codec = audio_stream["codec_name"].as_str().unwrap_or("");
    let sample_rate = audio_stream["sample_rate"].as_str().unwrap_or("0").parse().unwrap_or(0);
    let channels = audio_stream["channels"].as_u64().unwrap_or(0) as u16;
    let duration = value["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok());
    let duration_ms = duration.map(|d| (d * 1000.0) as i64).unwrap_or(0);
    
    let is_long = duration_ms > 15 * 60 * 1000 || duration_ms == 0;
    
    if is_long {
        // 串流模式
        let ffmpeg_path = crate::tools::locate("ffmpeg.exe")
            .map(|(p, _, _)| p)
            .ok_or("找不到 ffmpeg")?;
        
        let stream = FfmpegStream::new(&ffmpeg_path, path, sample_rate, channels, duration_ms)?;
        
        Ok(PreparedTrack {
            seq: 0,
            token: 0,
            song_id,
            path: path.to_string(),
            decoder: "stream".to_string(),
            rate: sample_rate,
            channels,
            duration_ms,
            norm_gain: 1.0,
            data: TrackData::Stream(Arc::new(StreamWrapper::FfmpegStream(stream))),
        })
    } else {
            // 短檔案：用 ffmpeg 整首解碼
            let ffmpeg_path = crate::tools::locate("ffmpeg.exe")
                .map(|(p, _, _)| p)
                .ok_or("找不到 ffmpeg")?;
        
            // SE3-1: 使用獨立 stderr 讀取執行緒與持久化 Sanitizer
            let mut child = std::process::Command::new(&ffmpeg_path)
                .args(["-nostdin", "-v", "error", "-i", path, "-f", "f32le", "-ac", &channels.to_string(), "-ar", &sample_rate.to_string(), "-"])
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
        
            // 解析 f32le 輸出
            let samples: Vec<f32> = output.stdout
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();
        
            let duration_ms = (samples.len() as f64 / (sample_rate as f64 * channels as f64) * 1000.0) as i64;
            let peak = samples.iter().map(|s| s.abs()).fold(0f32, f32::max);
            let norm_gain = if peak > 0.0001 { (0.89 / peak).clamp(0.05, 6.0) } else { 1.0 };
        
            Ok(PreparedTrack {
                seq: 0,
                token: 0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use tempfile::tempdir;
    
    #[test]
    fn test_next_coordinator_basic() {
        let coord = NextCoordinator::new();
        let gen = coord.set_next("/test/path.mp3".to_string(), 1);
        assert_eq!(gen, 1);
        assert_eq!(coord.current_generation(), 1);
    }
    
    #[test]
    fn test_next_coordinator_clear() {
        let coord = NextCoordinator::new();
        coord.set_next("/test/path1.mp3".to_string(), 1);
        let gen = coord.clear_next();
        assert_eq!(gen, 2);
        assert!(!coord.has_next());
    }
    
    #[test]
    fn test_next_coordinator_generation_increments() {
        let coord = NextCoordinator::new();
        let g1 = coord.set_next("/test/path1.mp3".to_string(), 1);
        let g2 = coord.set_next("/test/path2.mp3".to_string(), 2);
        let g3 = coord.clear_next();
        assert_eq!(g1, 1);
        assert_eq!(g2, 2);
        assert_eq!(g3, 3);
    }
}
