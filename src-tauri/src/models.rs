use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// 音樂歌曲主表（對應資料庫 songs 表）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Song {
    pub id: i64,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub duration_ms: i64,
    pub file_mtime: i64,
    pub file_size: Option<i64>,
    pub cover_path: Option<String>,
    pub title_pinyin: Option<String>,
    pub artist_pinyin: Option<String>,
    pub title_pinyin_initial: Option<String>,
    pub artist_pinyin_initial: Option<String>,
    pub added_at: Option<String>,
    pub play_count: i64,
    pub last_played_at: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub format: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub missing: i32,
    pub replaygain_track_gain: Option<f32>,
    pub replaygain_track_peak: Option<f32>,
    pub replaygain_album_gain: Option<f32>,
    pub replaygain_album_peak: Option<f32>,
}

/// 新增/更新歌曲的輸入（掃描時使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSong {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub duration_ms: i64,
    pub file_mtime: i64,
    pub file_size: Option<i64>,
    pub cover_path: Option<String>,
    pub title_pinyin: Option<String>,
    pub artist_pinyin: Option<String>,
    pub title_pinyin_initial: Option<String>,
    pub artist_pinyin_initial: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub format: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub replaygain_track_gain: Option<f32>,
    pub replaygain_track_peak: Option<f32>,
    pub replaygain_album_gain: Option<f32>,
    pub replaygain_album_peak: Option<f32>,
}

/// 歌單
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub sort_order: i32,
    pub created_at: Option<String>,
    pub is_smart: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewPlaylist {
    pub name: String,
    pub sort_order: i32,
    pub is_smart: Option<i32>,
}

/// 歌單項目（歌曲在歌單中的位置）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistItem {
    pub id: i64,
    pub playlist_id: i64,
    pub song_id: i64,
    pub position: i32,
    pub added_at: Option<String>,
    #[serde(flatten)]
    pub song: Option<Song>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewPlaylistItem {
    pub playlist_id: i64,
    pub song_id: i64,
    pub position: i32,
}

/// 搜尋結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub songs: Vec<Song>,
    pub total: i64,
}

/// 掃描進度事件（透過 Tauri event 傳給前端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    pub scanned: i64,
    pub total: i64,
    pub current_file: String,
    pub stage: String, // "scanning" | "finished" | "error"
    /// 逐檔掃描失敗清單（SP5）
    pub errors: Vec<ScanError>,
}

/// 掃描單一檔案失敗的原因（SP5 錯誤清單）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanError {
    pub path: String,
    pub reason: String,
}

/// 播放佇列項目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: Uuid,
    pub song_id: i64,
    pub song: Song,
}

/// 音量曲線
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VolumeCurve {
    Log,
    Linear,
}

impl Default for VolumeCurve {
    fn default() -> Self { VolumeCurve::Log }
}

/// 設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub music_folders: Vec<String>,
    pub theme: String, // "dark" | "light"
    pub lyrics_offset_ms: i32,
    pub replaygain_mode: ReplayGainMode,
    pub volume_normalization: VolumeNormalization,
    pub rg_preamp_has: f32,
    pub rg_preamp_none: f32,
    pub volume_curve: VolumeCurve,
    pub loudness_compensation: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            music_folders: Vec::new(),
            theme: "dark".to_string(),
            lyrics_offset_ms: 0,
            replaygain_mode: ReplayGainMode::default(),
            volume_normalization: VolumeNormalization::default(),
            rg_preamp_has: 0.0,
            rg_preamp_none: -6.0,
            volume_curve: VolumeCurve::default(),
            loudness_compensation: false,
        }
    }
}

/// 播放中歌曲資訊（傳給前端顯示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NowPlaying {
    pub song: Option<Song>,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub is_playing: bool,
    pub volume: f32,
    pub mode: PlayMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PlayMode {
    Sequence,
    Single,
    All,
    Random,
}

impl Default for PlayMode {
    fn default() -> Self {
        PlayMode::Sequence
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGainMode {
    Off,
    Track,
    Album,
    Auto,
}

impl Default for ReplayGainMode {
    fn default() -> Self { ReplayGainMode::Track }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VolumeNormalization {
    Replaygain,
    Peak,
    Off,
}

impl Default for VolumeNormalization {
    fn default() -> Self { VolumeNormalization::Off }
}

/// 歌詞行
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricLine {
    pub time_ms: i64,
    pub text: String,
}

/// 歌詞資訊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lyrics {
    pub song_id: i64,
    pub source: Option<String>,
    pub lines: Vec<LyricLine>,
    pub offset_ms: i32,
}

/// 資料庫狀態（單一連線，由 Tauri manage 掛載）
pub struct DbState {
    pub conn: std::sync::Mutex<rusqlite::Connection>,
}

/// 音訊執行緒回報的即時狀態。player_position 固定回 0，前端改讀這個。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    pub position_ms: i64,
    pub duration_ms: i64,
    pub is_playing: bool,
    pub volume: f32,
    pub song_id: i64,
    pub ended: bool,
    pub kind: String,
    pub stream_title: String,
    pub error: String,
    pub eq_bands: Vec<f32>,
    pub eq_enabled: bool,
    pub fx: crate::dsp::FxParams,
    pub preamp_db: f32,
    pub normalize: bool,
    pub crossfade_ms: i64,
    pub applied_gain: f32,
    pub band_hz: Vec<i32>,
    pub mode: PlayMode,
    pub applied_replaygain_db: Option<f32>,
    pub applied_preamp_db: f32,
    pub has_replaygain: bool,
    pub rg_mode: ReplayGainMode,
    pub volume_norm_mode: VolumeNormalization,
    pub rg_source: String,
    /// 播放序號（每次自然切換或手動載入遞增）
    pub play_seq: u64,
    /// 當前軌 token
    pub now_token: u64,
    /// 下一首 token（已排入待播槽）
    pub next_token: u64,
    /// 播放狀態機
    pub playback_state: String, // "Playing" | "WaitingForNext" | "Paused" | "Finished"
    /// 下一首清除 epoch（引擎自行清除時遞增）
    pub next_clear_epoch: u64,
    /// 真實音量增益 dB（靜音為 None，前端顯示「靜音」）
    pub volume_db: Option<f32>,
    /// 音量曲線
    pub volume_curve: VolumeCurve,
    /// 響度補償
    pub loudness_compensation: bool,
}

impl Default for EngineSnapshot {
    fn default() -> Self {
        Self {
            position_ms: 0,
            duration_ms: 0,
            is_playing: false,
            volume: 0.7,
            song_id: 0,
            ended: false,
            kind: "idle".into(),
            stream_title: String::new(),
            error: String::new(),
            eq_bands: vec![0.0; crate::eq::BANDS],
            eq_enabled: true,
            fx: crate::dsp::FxParams::default(),
            preamp_db: 0.0,
            normalize: false,
            crossfade_ms: 0,
            applied_gain: 1.0,
            band_hz: crate::eq::EQ_HZ.iter().map(|h| *h as i32).collect(),
            mode: PlayMode::Sequence,
            applied_replaygain_db: None,
            applied_preamp_db: 0.0,
            has_replaygain: false,
            rg_mode: ReplayGainMode::default(),
            volume_norm_mode: VolumeNormalization::default(),
            rg_source: "Unknown".to_string(),
            play_seq: 0,
            now_token: 0,
            next_token: 0,
            playback_state: "Finished".into(),
            next_clear_epoch: 0,
            volume_db: None,
            volume_curve: VolumeCurve::default(),
            loudness_compensation: false,
        }
    }
}

/// 播放器狀態（由 Tauri manage 掛載）
pub struct PlayerState {
    pub tx: std::sync::Mutex<Option<std::sync::mpsc::Sender<PlayerCommand>>>,
    pub status: std::sync::Arc<std::sync::Mutex<EngineSnapshot>>,
}

/// 播放控制命令（音訊執行緒透過 mpsc 接收，避免在 UI 執行緒解碼）
#[derive(Debug, Clone)]
pub enum PlayerCommand {
    Load { path: String, song_id: i64, token: u64, rg_track_gain: Option<f32>, rg_track_peak: Option<f32>, rg_album_gain: Option<f32>, rg_album_peak: Option<f32> },
    /// 載入並停在指定位置，不開始播放。重新開啟時接進度用。
    Prepare { path: String, song_id: i64, position_ms: i64, token: u64, rg_track_gain: Option<f32>, rg_track_peak: Option<f32>, rg_album_gain: Option<f32>, rg_album_peak: Option<f32> },
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    Prev,
    /// 設定下一首（立即回傳，背景準備）
    SetNext { path: String, song_id: i64, token: u64, rg_track_gain: Option<f32>, rg_track_peak: Option<f32>, rg_album_gain: Option<f32>, rg_album_peak: Option<f32> },
    /// 清除下一首
    ClearNext,
    Seek { position_ms: i64 },
    SetVolume { volume: f32 },
    SetMode { mode: PlayMode },
    SetEq { bands: Vec<f32> },
    SetPreamp { db: f32 },
    SetEqEnabled { enabled: bool },
    SetFx { fx: crate::dsp::FxParams },
    SetOutput { name: String },
    SetNormalize { enabled: bool },
    SetReplayGainMode { mode: ReplayGainMode, is_album_context: Option<bool> },
    SetVolumeNormalization { mode: VolumeNormalization, is_album_context: Option<bool> },
    SetRgPreampHas { db: f32 },
    SetRgPreampNone { db: f32 },
    SetAlbumContext { is_album_context: bool },
    SetCrossfade { ms: i64 },
    PlayUrl { url: String },
    SetVolumeCurve { curve: VolumeCurve },
    SetLoudnessCompensation { enabled: bool },
}

/// 歌詞解析結果
#[derive(Debug, Clone)]
pub struct ParsedLyrics {
    pub source: String,
    pub lines: Vec<LyricLine>,
    pub offset_ms: i32,
}

/// 音樂資料夾設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicFolder {
    pub id: i64,
    pub path: String,
    pub added_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMusicFolder {
    pub path: String,
}

/// 歌單排序更新請求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistOrderUpdate {
    pub playlist_id: i64,
    pub item_ids: Vec<i64>, // playlist_items id，依新順序排列
}

/// 播放歷史紀錄
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayHistory {
    pub id: i64,
    pub song_id: i64,
    pub played_at: Option<String>,
    #[serde(flatten)]
    pub song: Option<Song>,
}

/// 智慧歌單過濾條件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartPlaylistRule {
    pub field: String,    // "play_count" | "added_at" | ...
    pub operator: String, // ">" | "<" | "=" | "like"
    pub value: String,
}

/// 右鍵開啟時暫存的音檔路徑，前端取走後就清空。
#[derive(Debug, Default)]
pub struct OpenedFile {
    pub path: std::sync::Mutex<Option<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryFacet {
    pub key: String,
    pub count: i64,
}

/// 標籤寫入
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagEdit {
    pub song_id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub year: Option<i32>,
    #[serde(default)]
    pub track_number: Option<i32>,
    #[serde(default)]
    pub disc_number: Option<i32>,
}

/// 同一批只改歌手、專輯、曲風、年份。歌名與音軌號不在這裡。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagBatch {
    pub song_ids: Vec<i64>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagBatchResult {
    pub updated: i64,
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadioStation {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropPaths {
    pub files: Vec<String>,
    pub dirs: Vec<String>,
}

/// 轉檔請求。format 只接受 mp3 / flac / wav / ogg。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertRequest {
    pub song_id: i64,
    pub format: String,
    pub output_dir: String,
    #[serde(default)]
    pub bitrate_kbps: Option<i32>,
}
