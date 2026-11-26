use crate::db::{
    add_music_folder, add_song_to_playlist, create_playlist, db_lock, delete_playlist, delete_song,
    get_lyrics, get_song, increment_play_count, init_db, list_music_folders, list_playlist_items,
    list_playlists, list_songs, load_config, mark_missing_songs, record_play_history, remove_from_playlist,
    get_config_value, remove_music_folder, save_config_kv, save_lyrics, search_songs, set_lyrics_offset,
    update_playlist, update_playlist_order,
    add_smart_playlist_rule, list_smart_playlist_rules, delete_smart_playlist_rules, query_smart_playlist_songs,
};
use crate::models::{
    AppConfig, DbState, Lyrics, MusicFolder, NewMusicFolder, NewPlaylist, NewPlaylistItem,
    NewSong, PlayHistory, Playlist, PlaylistItem, PlaylistOrderUpdate, PlayerCommand, PlayerState,
    PlayMode, SearchResult, Song, SmartPlaylistRule,
};
use crate::open_with::{log_line, sanitize_log};
use crate::player::start_player_thread;
use crate::scanner::scan_music_folders;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

// ============================================================================
// 播放器控制命令（音訊執行緒透過 mpsc 接收，UI 執行緒只發指令）
// ============================================================================

/// 載入並播放一首歌曲（path、song_id、token）。
#[tauri::command]
pub fn player_load(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    path: String,
    song_id: i64,
    token: u64,
) -> Result<(), String> {
    crate::open_with::log_line(&format!("CMD player_load path={path} song_id={song_id} token={token}"));
    let rg = {
        let conn = db_lock(&db_state);
        crate::db::get_replaygain(&conn, song_id).unwrap_or((None, None, None, None))
    };
    let tx = state.tx.lock().map_err(|e| e.to_string())?;
    let tx = tx.as_ref().ok_or("播放器尚未初始化")?;
    tx.send(PlayerCommand::Load {
        path,
        song_id,
        token,
        rg_track_gain: rg.0,
        rg_track_peak: rg.1,
        rg_album_gain: rg.2,
        rg_album_peak: rg.3,
    })
        .map_err(|e| e.to_string())
}

/// 設定下一首（立即回傳，背景準備）。
#[tauri::command]
pub fn player_set_next(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    path: String,
    song_id: i64,
    token: u64,
) -> Result<(), String> {
    crate::open_with::log_line(&format!("CMD player_set_next path={path} song_id={song_id} token={token}"));
    let rg = {
        let conn = db_lock(&db_state);
        crate::db::get_replaygain(&conn, song_id).unwrap_or((None, None, None, None))
    };
    let cmd = PlayerCommand::SetNext {
        path,
        song_id,
        token,
        rg_track_gain: rg.0,
        rg_track_peak: rg.1,
        rg_album_gain: rg.2,
        rg_album_peak: rg.3,
    };
    send_cmd(state, cmd)
}

/// 清除下一首。
#[tauri::command]
pub fn player_clear_next(
    state: State<'_, PlayerState>,
) -> Result<(), String> {
    crate::open_with::log_line("CMD player_clear_next");
    send_cmd(state, PlayerCommand::ClearNext)
}

/// 預載下一首（用於無縫切歌，gapless）。保留相容性，內部轉為 SetNext。
#[tauri::command]
pub fn player_preload(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    path: String,
    song_id: i64,
) -> Result<(), String> {
    let token = 0; // 相容模式不帶 token
    let rg = {
        let conn = db_lock(&db_state);
        crate::db::get_replaygain(&conn, song_id).unwrap_or((None, None, None, None))
    };
    let cmd = PlayerCommand::SetNext {
        path,
        song_id,
        token,
        rg_track_gain: rg.0,
        rg_track_peak: rg.1,
        rg_album_gain: rg.2,
        rg_album_peak: rg.3,
    };
    send_cmd(state, cmd)
}

#[tauri::command]
pub fn player_play(state: State<'_, PlayerState>) -> Result<(), String> {
    crate::open_with::log_line("CMD player_play called");
    send_cmd(state, PlayerCommand::Play)
}

#[tauri::command]
pub fn player_pause(state: State<'_, PlayerState>) -> Result<(), String> {
    send_cmd(state, PlayerCommand::Pause)
}

#[tauri::command]
pub fn player_toggle(state: State<'_, PlayerState>) -> Result<(), String> {
    send_cmd(state, PlayerCommand::Toggle)
}

#[tauri::command]
pub fn player_stop(state: State<'_, PlayerState>) -> Result<(), String> {
    send_cmd(state, PlayerCommand::Stop)
}

#[tauri::command]
pub fn player_next(state: State<'_, PlayerState>) -> Result<(), String> {
    send_cmd(state, PlayerCommand::Next)
}

#[tauri::command]
pub fn player_prev(state: State<'_, PlayerState>) -> Result<(), String> {
    send_cmd(state, PlayerCommand::Prev)
}

#[tauri::command]
pub fn player_seek(
    state: State<'_, PlayerState>,
    position_ms: i64,
) -> Result<(), String> {
    send_cmd(state, PlayerCommand::Seek { position_ms })
}

#[tauri::command]
pub fn player_set_volume(
    state: State<'_, PlayerState>,
    volume: f32,
) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetVolume { volume })
}

#[tauri::command]
pub fn player_set_mode(
    state: State<'_, PlayerState>,
    mode: PlayMode,
) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetMode { mode })
}

/// 取得目前播放位置（毫秒）。
#[tauri::command]
pub fn player_position(_state: State<'_, PlayerState>) -> Result<i64, String> {
    // position 由前端透過 timer 轮询，這裡預留介面；實際位置由 Sink.get_pos() 在音訊執行緒維護。
    // 為簡化，前端使用本地音訊元素或透過事件同步；此 command 回傳 0 作為佔位。
    Ok(0)
}

fn send_cmd(state: State<'_, PlayerState>, cmd: PlayerCommand) -> Result<(), String> {
    let tx = state.tx.lock().map_err(|e| e.to_string())?;
    let tx = tx.as_ref().ok_or("播放器尚未初始化")?;
    tx.send(cmd).map_err(|e| e.to_string())
}

/// 載入歌曲並停在指定位置，不開始播放。重新開啟時接進度用，避免 player_load 先出聲。
#[tauri::command]
pub fn player_prepare(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    path: String,
    song_id: i64,
    position_ms: i64,
    token: u64,
) -> Result<(), String> {
    crate::open_with::log_line(&format!("CMD player_prepare path={path} song_id={song_id} position_ms={position_ms} token={token}"));
    let rg = {
        let conn = db_lock(&db_state);
        crate::db::get_replaygain(&conn, song_id).unwrap_or((None, None, None, None))
    };
    let cmd = PlayerCommand::Prepare {
        path,
        song_id,
        position_ms,
        token,
        rg_track_gain: rg.0,
        rg_track_peak: rg.1,
        rg_album_gain: rg.2,
        rg_album_peak: rg.3,
    };
    send_cmd(state, cmd)
}

// ============================================================================
// 音樂庫 / 掃描
// ============================================================================

/// 掃描音樂資料夾（平行讀取 Tag、產生封面縮圖、寫入資料庫）。進度經由 event 傳給前端。
#[tauri::command]
pub fn scan_folders(
    app: AppHandle,
    state: State<'_, DbState>,
    folders: Vec<String>,
) -> Result<crate::models::ScanProgress, String> {
    scan_music_folders(app, state, folders)
}

/// 取得歌曲列表（分頁、排序）。
#[tauri::command]
pub fn get_songs(
    state: State<'_, DbState>,
    limit: i64,
    offset: i64,
    order_by: String,
    order_dir: String,
) -> Result<Vec<Song>, String> {
    let conn = db_lock(&state);
    list_songs(&conn, limit, offset, &order_by, &order_dir).map_err(|e| e.to_string())
}

/// 全文搜尋（FTS5，支援中文、拼音、拼音首字母、片段）。
#[tauri::command]
pub fn search_songs_cmd(
    state: State<'_, DbState>,
    query: String,
    limit: i64,
) -> Result<SearchResult, String> {
    let conn = db_lock(&state);
    search_songs(&conn, &query, limit).map_err(|e| e.to_string())
}

/// 取得單首歌曲。
#[tauri::command]
pub fn get_song_cmd(
    state: State<'_, DbState>,
    id: i64,
) -> Result<Option<Song>, String> {
    let conn = db_lock(&state);
    get_song(&conn, id).map_err(|e| e.to_string())
}

/// 播放完成：play_count +1，更新 last_played_at，寫入播放歷史。
#[tauri::command]
pub fn song_played(
    state: State<'_, DbState>,
    id: i64,
) -> Result<(), String> {
    let conn = db_lock(&state);
    increment_play_count(&conn, id).map_err(|e| e.to_string())
}

/// 記錄播放歷史（最近播放歌單使用）。
#[tauri::command]
pub fn record_history(
    state: State<'_, DbState>,
    id: i64,
) -> Result<(), String> {
    let conn = db_lock(&state);
    record_play_history(&conn, id).map_err(|e| e.to_string())
}

// ============================================================================
// 歌單管理
// ============================================================================

#[tauri::command]
pub fn create_playlist_cmd(
    state: State<'_, DbState>,
    playlist: NewPlaylist,
) -> Result<i64, String> {
    let conn = db_lock(&state);
    create_playlist(&conn, &playlist).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_playlists_cmd(state: State<'_, DbState>) -> Result<Vec<Playlist>, String> {
    let conn = db_lock(&state);
    list_playlists(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_playlist_cmd(
    state: State<'_, DbState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let conn = db_lock(&state);
    update_playlist(&conn, id, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_playlist_cmd(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = db_lock(&state);
    delete_playlist(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_song_to_playlist_cmd(
    state: State<'_, DbState>,
    item: NewPlaylistItem,
) -> Result<i64, String> {
    let conn = db_lock(&state);
    add_song_to_playlist(&conn, &item).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_playlist_items_cmd(
    state: State<'_, DbState>,
    playlist_id: i64,
) -> Result<Vec<PlaylistItem>, String> {
    let conn = db_lock(&state);
    list_playlist_items(&conn, playlist_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_from_playlist_cmd(state: State<'_, DbState>, item_id: i64) -> Result<(), String> {
    let conn = db_lock(&state);
    remove_from_playlist(&conn, item_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_playlist_order_cmd(
    state: State<'_, DbState>,
    update: PlaylistOrderUpdate,
) -> Result<(), String> {
    let mut conn = db_lock(&state);
    update_playlist_order(&mut *conn, &update).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_smart_rule_cmd(
    state: State<'_, DbState>,
    playlist_id: i64,
    field: String,
    operator: String,
    value: String,
) -> Result<i64, String> {
    let conn = db_lock(&state);
    add_smart_playlist_rule(&conn, playlist_id, &field, &operator, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_smart_rules_cmd(
    state: State<'_, DbState>,
    playlist_id: i64,
) -> Result<Vec<SmartPlaylistRule>, String> {
    let conn = db_lock(&state);
    list_smart_playlist_rules(&conn, playlist_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_smart_rules_cmd(state: State<'_, DbState>, playlist_id: i64) -> Result<(), String> {
    let conn = db_lock(&state);
    delete_smart_playlist_rules(&conn, playlist_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn smart_playlist_songs_cmd(
    state: State<'_, DbState>,
    playlist_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<Song>, String> {
    let conn = db_lock(&state);
    query_smart_playlist_songs(&conn, playlist_id, limit, offset).map_err(|e| e.to_string())
}

// ============================================================================
// 歌詞
// ============================================================================

#[tauri::command]
pub fn save_lyrics_cmd(
    state: State<'_, DbState>,
    song_id: i64,
    lyrics: Lyrics,
) -> Result<(), String> {
    let conn = db_lock(&state);
    save_lyrics(&conn, song_id, &lyrics).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_lyrics_cmd(state: State<'_, DbState>, song_id: i64) -> Result<Option<Lyrics>, String> {
    let conn = db_lock(&state);
    if let Some(lyrics) = get_lyrics(&conn, song_id).map_err(|e| e.to_string())? {
        if !lyrics.lines.is_empty() {
            return Ok(Some(lyrics));
        }
    }
    let Some(song) = get_song(&conn, song_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let Some(raw) = read_sidecar_lrc(&song.path) else {
        return Ok(None);
    };
    let lines = parse_lrc(&raw);
    if lines.is_empty() {
        return Ok(None);
    }
    let lyrics = Lyrics {
        song_id,
        source: Some("file".into()),
        lines,
        offset_ms: 0,
    };
    save_lyrics(&conn, song_id, &lyrics).map_err(|e| e.to_string())?;
    Ok(Some(lyrics))
}

#[tauri::command]
pub fn set_lyrics_offset_cmd(
    state: State<'_, DbState>,
    song_id: i64,
    offset_ms: i32,
) -> Result<(), String> {
    let conn = db_lock(&state);
    let changed = set_lyrics_offset(&conn, song_id, offset_ms).map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("這首還沒有歌詞，無法記偏移".into());
    }
    Ok(())
}

// ============================================================================
// 音樂資料夾與設定
// ============================================================================

#[tauri::command]
pub fn add_music_folder_cmd(
    state: State<'_, DbState>,
    folder: NewMusicFolder,
) -> Result<i64, String> {
    let conn = db_lock(&state);
    add_music_folder(&conn, &folder).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_music_folders_cmd(state: State<'_, DbState>) -> Result<Vec<MusicFolder>, String> {
    let conn = db_lock(&state);
    list_music_folders(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_music_folder_cmd(state: State<'_, DbState>, path: String) -> Result<(), String> {
    let conn = db_lock(&state);
    remove_music_folder(&conn, &path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn load_config_cmd(state: State<'_, DbState>) -> Result<AppConfig, String> {
    let conn = db_lock(&state);
    load_config(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_config_cmd(
    state: State<'_, DbState>,
    key: String,
    value: String,
) -> Result<(), String> {
    let conn = db_lock(&state);
    save_config_kv(&conn, &key, &value).map_err(|e| e.to_string())
}

/// 讀取 app_config 的單一鍵。沒有該鍵時回 null，不當成錯誤。
#[tauri::command]
pub fn get_config_cmd(state: State<'_, DbState>, key: String) -> Result<Option<String>, String> {
    let conn = db_lock(&state);
    get_config_value(&conn, &key).map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppPaths {
    pub db: PathBuf,
    pub covers: PathBuf,
    pub config: PathBuf,
}

/// 取得可攜式資料路徑（讓前端知道資料放在執行檔旁的 data/）。
#[tauri::command]
pub fn app_paths() -> Result<AppPaths, String> {
    Ok(AppPaths {
        db: crate::db::db_path(),
        covers: crate::db::covers_dir(),
        config: crate::db::config_path(),
    })
}

/// 音訊執行緒的即時狀態。不要用 player_position，那個固定回 0。
#[tauri::command]
pub fn player_status(state: State<'_, PlayerState>) -> Result<crate::models::EngineSnapshot, String> {
    let status = state.status.lock().map_err(|e| e.to_string())?;
    Ok(status.clone())
}

/// 設定 18 段等化器增益（dB，-15 到 15）。本地檔與電台都會套用，即時生效不重開音源。
#[tauri::command]
pub fn player_set_eq(state: State<'_, PlayerState>, bands: Vec<f32>) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetEq { bands })
}

/// 前級增益（dB，-15 到 15）。乘在等化器之前；等化器關閉時一併略過。
#[tauri::command]
pub fn player_set_preamp(state: State<'_, PlayerState>, db: f32) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetPreamp { db })
}

/// 等化器總開關。關閉時各段與前級都不作用，推桿位置保留。
#[tauri::command]
pub fn player_set_eq_enabled(state: State<'_, PlayerState>, enabled: bool) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetEqEnabled { enabled })
}

/// 音效（速度、節奏、音高、低音、高音增強、人聲消除、合唱、鑲邊、回音、殘響）。即時生效。
/// 欄位用 camelCase（FxParams 有 rename_all）。速度/節奏/音高不作用於電台。
#[tauri::command]
pub fn player_set_fx(state: State<'_, PlayerState>, fx: crate::dsp::FxParams) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetFx { fx })
}

/// 依音訊執行緒相同的濾波器設計算出頻率響應曲線，畫面用來畫曲線。回傳 [Hz, dB]。
#[tauri::command]
pub fn eq_response_cmd(bands: Vec<f32>, preamp: f32, enabled: bool, sample_rate: Option<u32>) -> Vec<[f32; 2]> {
    let mut params = crate::eq::EqParams { enabled, preamp_db: preamp.clamp(-15.0, 15.0), ..Default::default() };
    for (i, g) in bands.into_iter().take(crate::eq::BANDS).enumerate() {
        params.bands[i] = if g.is_finite() { g.clamp(-15.0, 15.0) } else { 0.0 };
    }
    crate::eq::response_curve(&params, sample_rate.unwrap_or(44100), 160)
}

/// 列出本機輸出裝置。列不出就回錯誤，前端不要畫下拉。
#[tauri::command]
pub fn player_output_devices() -> Result<Vec<String>, String> {
    crate::player::output_device_names()
}

/// 切到指定輸出裝置，本地檔從目前位置繼續。切不動就回錯誤。
#[tauri::command]
pub fn player_set_output(state: State<'_, PlayerState>, name: String) -> Result<(), String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("沒有選擇輸出裝置".into());
    }
    send_cmd(state, PlayerCommand::SetOutput { name: trimmed })
}

/// 峰值正規化。只作用於本地檔案。
#[tauri::command]
#[deprecated(note = "改用 player_set_volume_normalization")]
pub fn player_set_normalize(state: State<'_, PlayerState>, enabled: bool) -> Result<(), String> {
    let mode = if enabled {
        crate::models::VolumeNormalization::Peak
    } else {
        crate::models::VolumeNormalization::Off
    };
    send_cmd(state, PlayerCommand::SetVolumeNormalization { mode, is_album_context: None })
}

/// 設定 ReplayGain 模式（off | track | album | auto）。
#[tauri::command]
pub fn player_set_replaygain_mode(state: State<'_, PlayerState>, mode: String) -> Result<(), String> {
    let rg_mode = match mode.as_str() {
        "off" => crate::models::ReplayGainMode::Off,
        "track" => crate::models::ReplayGainMode::Track,
        "album" => crate::models::ReplayGainMode::Album,
        "auto" => crate::models::ReplayGainMode::Auto,
        _ => return Err(format!("無效的 ReplayGain 模式: {}", mode)),
    };
    send_cmd(state, PlayerCommand::SetReplayGainMode { mode: rg_mode, is_album_context: None })
}

/// 設定音量正規化模式（replaygain | peak | off）。
#[tauri::command]
pub fn player_set_volume_normalization(state: State<'_, PlayerState>, mode: String) -> Result<(), String> {
    let vol_norm_mode = match mode.as_str() {
        "replaygain" => crate::models::VolumeNormalization::Replaygain,
        "peak" => crate::models::VolumeNormalization::Peak,
        "off" => crate::models::VolumeNormalization::Off,
        _ => return Err(format!("無效的音量正規化模式: {}", mode)),
    };
    send_cmd(state, PlayerCommand::SetVolumeNormalization { mode: vol_norm_mode, is_album_context: None })
}

/// 設定有 ReplayGain 檔案的前級。
#[tauri::command]
pub fn player_set_rg_preamp_has(state: State<'_, PlayerState>, db: f32) -> Result<(), String> {
    if !db.is_finite() || db < -20.0 || db > 20.0 {
        return Err(format!("前級值超出範圍 -20..+20 dB: {}", db));
    }
    send_cmd(state, PlayerCommand::SetRgPreampHas { db })
}

/// 設定無 ReplayGain 檔案的前級。
#[tauri::command]
pub fn player_set_rg_preamp_none(state: State<'_, PlayerState>, db: f32) -> Result<(), String> {
    if !db.is_finite() || db < -20.0 || db > 20.0 {
        return Err(format!("前級值超出範圍 -20..+20 dB: {}", db));
    }
    send_cmd(state, PlayerCommand::SetRgPreampNone { db })
}

/// 設定專輯播放上下文。
#[tauri::command]
pub fn player_set_album_context(state: State<'_, PlayerState>, is_album_context: bool) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetAlbumContext { is_album_context })
}

/// 交叉淡化毫秒數。0 為關閉。只作用於本地檔案。
#[tauri::command]
pub fn player_set_crossfade(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    ms: i64,
) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetCrossfade { ms })?;
    // 持久化（互斥：ms > 0 時 gap_ms = 0）
    let conn = db_lock(&db_state);
    save_config_kv(&conn, "crossfade_ms", &ms.to_string()).map_err(|e| e.to_string())?;
    if ms > 0 {
        save_config_kv(&conn, "gap_ms", "0").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 播放使用者貼上的 HTTP 音訊串流。不是電台目錄。
#[tauri::command]
pub fn player_play_url(state: State<'_, PlayerState>, url: String) -> Result<(), String> {
    let trimmed = url.trim().to_string();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("只接受 http 或 https 串流網址".into());
    }
    send_cmd(state, PlayerCommand::PlayUrl { url: trimmed })
}

/// S7: 設定或取消「設為預設播放器」（HKCU 檔案關聯）。僅在使用者於設定頁面
/// 明確操作時呼叫，不會自動寫入登錄檔。
#[tauri::command]
pub fn player_set_default_player(enable: bool) -> Result<(), String> {
    crate::open_with::set_windows_association(enable)
}

/// 設定音量曲線：log | linear。持久化並即時生效。
#[tauri::command]
pub fn player_set_volume_curve(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    curve: String,
) -> Result<(), String> {
    let vol_curve = match curve.as_str() {
        "log" => crate::models::VolumeCurve::Log,
        "linear" => crate::models::VolumeCurve::Linear,
        _ => return Err(format!("無效的音量曲線: {}，僅接受 log 或 linear", curve)),
    };
    send_cmd(state, PlayerCommand::SetVolumeCurve { curve: vol_curve })?;
    // 持久化
    let conn = db_lock(&db_state);
    save_config_kv(&conn, "volume_curve", &curve).map_err(|e| e.to_string())
}

/// 設定響度補償開關。持久化並即時生效。
#[tauri::command]
pub fn player_set_loudness_compensation(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    enabled: bool,
) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetLoudnessCompensation { enabled })?;
    // 持久化
    let conn = db_lock(&db_state);
    save_config_kv(&conn, "loudness_compensation", &enabled.to_string()).map_err(|e| e.to_string())
}

/// 設定六項淡入淡出時間（ms，0 為關）。參數：playMs, pauseMs, resumeMs, stopMs, seekMs, switchMs（各 0–5000）。
#[tauri::command]
pub fn player_set_fade_settings(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    playMs: i64,
    pauseMs: i64,
    resumeMs: i64,
    stopMs: i64,
    seekMs: i64,
    switchMs: i64,
) -> Result<(), String> {
    // 參數邊界驗證
    for (name, val) in [("playMs", playMs), ("pauseMs", pauseMs), ("resumeMs", resumeMs), ("stopMs", stopMs), ("seekMs", seekMs), ("switchMs", switchMs)] {
        if !(0..=5000).contains(&val) {
            return Err(format!("{} 超出範圍 0–5000: {}", name, val));
        }
    }
    send_cmd(state, PlayerCommand::SetFadeSettings { play_ms: playMs, pause_ms: pauseMs, resume_ms: resumeMs, stop_ms: stopMs, seek_ms: seekMs, switch_ms: switchMs })?;
    // 持久化六個 key
    let conn = db_lock(&db_state);
    save_config_kv(&conn, "fade_play_ms", &playMs.to_string()).map_err(|e| e.to_string())?;
    save_config_kv(&conn, "fade_pause_ms", &pauseMs.to_string()).map_err(|e| e.to_string())?;
    save_config_kv(&conn, "fade_resume_ms", &resumeMs.to_string()).map_err(|e| e.to_string())?;
    save_config_kv(&conn, "fade_stop_ms", &stopMs.to_string()).map_err(|e| e.to_string())?;
    save_config_kv(&conn, "fade_seek_ms", &seekMs.to_string()).map_err(|e| e.to_string())?;
    save_config_kv(&conn, "fade_switch_ms", &switchMs.to_string()).map_err(|e| e.to_string())?;
    Ok(())
}

/// 設定曲間停頓（ms，0 為關，1–10000）。ms > 0 時自動將 crossfade_ms 歸 0。
#[tauri::command]
pub fn player_set_gap_ms(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    ms: i64,
) -> Result<(), String> {
    if !(0..=10000).contains(&ms) {
        return Err(format!("gap_ms 超出範圍 0–10000: {}", ms));
    }
    send_cmd(state, PlayerCommand::SetGapMs { ms })?;
    // 持久化（互斥邏輯在命令 handler 內處理）
    let conn = db_lock(&db_state);
    save_config_kv(&conn, "gap_ms", &ms.to_string()).map_err(|e| e.to_string())?;
    if ms > 0 {
        save_config_kv(&conn, "crossfade_ms", "0").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 設定去除靜音開關。
#[tauri::command]
pub fn player_set_silence_removal(
    state: State<'_, PlayerState>,
    db_state: State<'_, DbState>,
    enabled: bool,
) -> Result<(), String> {
    send_cmd(state, PlayerCommand::SetSilenceRemoval { enabled })?;
    // 持久化
    let conn = db_lock(&db_state);
    save_config_kv(&conn, "silence_removal", &enabled.to_string()).map_err(|e| e.to_string())
}

/// 曲庫分面：artist / album / genre / folder。
#[tauri::command]
pub fn library_facets_cmd(state: State<'_, DbState>, field: String) -> Result<Vec<crate::models::LibraryFacet>, String> {
    crate::extras::library_facets(&state, &field)
}

/// 依分面取出歌曲。
#[tauri::command]
pub fn songs_by_field_cmd(
    state: State<'_, DbState>,
    field: String,
    value: String,
    limit: i64,
    offset: i64,
    order_by: String,
    order_dir: String,
) -> Result<Vec<crate::models::Song>, String> {
    crate::extras::songs_by_field(&state, &field, &value, limit, offset, &order_by, &order_dir)
}

/// 最近播放。
#[tauri::command]
pub fn list_recent_cmd(state: State<'_, DbState>, limit: i64) -> Result<Vec<crate::models::Song>, String> {
    crate::extras::list_recent(&state, limit)
}

/// 讀取檔案標籤。
#[tauri::command]
pub fn read_tags_cmd(state: State<'_, DbState>, song_id: i64) -> Result<crate::models::TagEdit, String> {
    crate::extras::read_tags(&state, song_id)
}

/// 寫入檔案標籤並更新資料庫。
#[tauri::command]
pub fn write_tags_cmd(state: State<'_, DbState>, edit: crate::models::TagEdit) -> Result<(), String> {
    crate::extras::write_tags(&state, &edit)
}

/// 這台電腦能不能轉檔。
#[tauri::command]
pub fn ffmpeg_available_cmd() -> bool {
    crate::tools::ffmpeg_available()
}

/// 轉檔。沒有 ffmpeg 就回錯誤，不回假路徑。
#[tauri::command]
pub fn convert_audio_cmd(state: State<'_, DbState>, request: crate::models::ConvertRequest) -> Result<String, String> {
    crate::extras::convert_audio(&state, &request)
}

#[tauri::command]
pub fn write_tags_batch_cmd(state: State<'_, DbState>, batch: crate::models::TagBatch) -> Result<crate::models::TagBatchResult, String> {
    crate::extras::write_tags_batch(&state, &batch)
}

#[tauri::command]
pub fn list_radio_cmd(state: State<'_, DbState>) -> Result<Vec<crate::models::RadioStation>, String> {
    crate::extras::list_radio(&state)
}

#[tauri::command]
pub fn save_radio_cmd(state: State<'_, DbState>, name: String, url: String) -> Result<i64, String> {
    crate::extras::save_radio(&state, &name, &url)
}

#[tauri::command]
pub fn rename_radio_cmd(state: State<'_, DbState>, id: i64, name: String) -> Result<(), String> {
    crate::extras::rename_radio(&state, id, &name)
}

#[tauri::command]
pub fn delete_radio_cmd(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    crate::extras::delete_radio(&state, id)
}

#[tauri::command]
pub fn classify_drop_cmd(paths: Vec<String>) -> crate::models::DropPaths {
    crate::extras::classify_drop(&paths)
}

/// 在檔案總管顯示檔案。
#[tauri::command]
pub fn reveal_in_explorer_cmd(path: String) -> Result<(), String> {
    crate::extras::reveal_in_explorer(&path)
}

/// 開啟資料夾選擇器。沒有裝 dialog plugin，用系統對話框。
#[tauri::command]
pub fn browse_folder_cmd() -> Result<Option<String>, String> {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-STA",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; if ($d.ShowDialog() -eq 'OK') { Write-Output $d.SelectedPath }",
        ])
        .output()
        .map_err(|e| format!("無法開啟資料夾選擇器: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let sanitized = sanitize_log(&err);
        return Err(format!("資料夾選擇器失敗: {sanitized}"));
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() { Ok(None) } else { Ok(Some(path)) }
}

/// 前端啟動後取走右鍵開啟的音檔路徑。
#[tauri::command]
pub fn take_opened_file_cmd(state: State<'_, crate::models::OpenedFile>) -> Result<Option<String>, String> {
    crate::open_with::log_line("CMD take_opened_file_cmd called");
    let mut slot = state.path.lock().map_err(|e| e.to_string())?;
    let result = slot.take();
    crate::open_with::log_line(&format!("CMD take_opened_file_cmd result={:?}", result));
    Ok(result)
}

/// 依路徑找曲庫裡的歌。沒有就回 null，前端仍會用檔名播放。
#[tauri::command]
pub fn song_by_path_cmd(state: State<'_, DbState>, path: String) -> Result<Option<crate::models::Song>, String> {
    crate::open_with::log_line(&format!("CMD song_by_path_cmd path={path}"));
    let result = crate::extras::song_by_path(&state, &path);
    let found = result.as_ref().ok().and_then(|s| s.as_ref()).is_some();
    crate::open_with::log_line(&format!("CMD song_by_path_cmd found={found}"));
    result
}

/// 把正在播放的歌名寫進 Windows 音量浮層。媒體鍵沒接上就回錯誤，畫面不畫開關。
#[tauri::command]
pub fn smtc_update_cmd(title: String, artist: String, playing: bool) -> Result<(), String> {
    crate::transport::update_smtc(&title, &artist, playing)
}

#[derive(Debug, Clone, Serialize)]
pub struct M3uImportResult {
    pub playlist_id: Option<i64>,
    pub name: String,
    pub matched: i64,
    pub missing: Vec<String>,
}

/// 選一份 UTF-8 m3u。取消就回 null，不報成功。
#[tauri::command]
pub fn pick_m3u_cmd() -> Result<Option<String>, String> {
    pick_dialog(
        "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Filter = '播放清單 (*.m3u;*.m3u8)|*.m3u;*.m3u8'; if ($d.ShowDialog() -eq 'OK') { Write-Output $d.FileName }",
        "無法開啟檔案選擇器",
    )
}

/// 選匯出路徑。取消就回 null。
#[tauri::command]
pub fn pick_m3u_save_cmd(name: String) -> Result<Option<String>, String> {
    let safe = sanitize_filename(&name);
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.SaveFileDialog; $d.Filter = '播放清單 (*.m3u8)|*.m3u8'; $d.FileName = '{}'; if ($d.ShowDialog() -eq 'OK') {{ Write-Output $d.FileName }}",
        safe.replace('\'', "")
    );
    pick_dialog(&script, "無法開啟儲存對話框")
}

fn pick_dialog(script: &str, fail: &str) -> Result<Option<String>, String> {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", script])
        .output()
        .map_err(|e| format!("{fail}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let sanitized = sanitize_log(&err);
        return Err(format!("{fail}: {sanitized}"));
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() { Ok(None) } else { Ok(Some(path)) }
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| match c {
        '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
        _ => c,
    }).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() { "歌單.m3u8".into() } else { format!("{cleaned}.m3u8") }
}

/// 匯入 UTF-8 m3u。對不到的路徑回傳，不建立空歌單。
#[tauri::command]
pub fn import_m3u_cmd(state: State<'_, DbState>, path: String) -> Result<M3uImportResult, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("讀不到清單: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "這份清單不是 UTF-8".to_string())?;
    let text = text.trim_start_matches('\u{feff}');
    let m3u_dir = std::path::Path::new(&path).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let conn = db_lock(&state);
    let mut stmt = conn.prepare("SELECT id, path FROM songs").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
    let mut by_path = std::collections::HashMap::<String, i64>::new();
    for row in rows.filter_map(Result::ok) {
        by_path.insert(normalize_path(&row.1), row.0);
    }
    drop(stmt);
    let mut matched: Vec<i64> = Vec::new();
    let mut missing = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let resolved = resolve_m3u_path(line, &m3u_dir);
        let key = normalize_path(&resolved);
        if let Some(id) = by_path.get(&key) {
            matched.push(*id);
        } else {
            missing.push(line.to_string());
        }
    }
    let name = std::path::Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("匯入的歌單")
        .to_string();
    if matched.is_empty() {
        return Ok(M3uImportResult { playlist_id: None, name, matched: 0, missing });
    }
    let playlist_id = create_playlist(&conn, &NewPlaylist { name: name.clone(), sort_order: 0, is_smart: Some(0) }).map_err(|e| e.to_string())?;
    for (idx, song_id) in matched.iter().enumerate() {
        add_song_to_playlist(&conn, &NewPlaylistItem {
            playlist_id,
            song_id: *song_id,
            position: idx as i32,
        }).map_err(|e| e.to_string())?;
    }
    Ok(M3uImportResult { playlist_id: Some(playlist_id), name, matched: matched.len() as i64, missing })
}

/// 把目前歌單寫成 #EXTM3U。路徑用資料庫原樣。
#[tauri::command]
pub fn export_m3u_cmd(state: State<'_, DbState>, playlist_id: i64, path: String) -> Result<(), String> {
    let conn = db_lock(&state);
    let items = list_playlist_items(&conn, playlist_id).map_err(|e| e.to_string())?;
    if items.is_empty() {
        return Err("這個歌單是空的，沒有寫出檔案".into());
    }
    let mut body = String::from("#EXTM3U\r\n");
    for item in items {
        let Some(song) = item.song else { continue; };
        let artist = song.artist.unwrap_or_default();
        let title = song.title.unwrap_or_else(|| song.path.clone());
        let label = if artist.is_empty() { title } else { format!("{artist} - {title}") };
        let secs = if song.duration_ms > 0 { song.duration_ms / 1000 } else { -1 };
        body.push_str(&format!("#EXTINF:{secs},{label}\r\n{}\r\n", song.path));
    }
    std::fs::write(&path, body).map_err(|e| format!("寫不進去: {e}"))
}

fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").trim().to_lowercase()
}

fn resolve_m3u_path(line: &str, m3u_dir: &std::path::Path) -> String {
    let line = line.trim().trim_start_matches("file:///").replace("%20", " ");
    let path = std::path::Path::new(&line);
    if path.is_absolute() {
        line
    } else {
        m3u_dir.join(path).to_string_lossy().to_string()
    }
}

fn read_sidecar_lrc(song_path: &str) -> Option<String> {
    let path = std::path::Path::new(song_path);
    let stem = path.file_stem()?.to_string_lossy();
    let lrc = path.with_file_name(format!("{stem}.lrc"));
    let bytes = std::fs::read(lrc).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    Some(text.trim_start_matches('\u{feff}').to_string())
}

pub fn parse_lrc(raw: &str) -> Vec<crate::models::LyricLine> {
    let mut lines = Vec::new();
    for raw_line in raw.lines() {
        let mut rest = raw_line;
        let mut times = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break; };
            let tag = &rest[1..end];
            if let Some(ms) = lrc_stamp(tag) {
                times.push(ms);
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
        if times.is_empty() { continue; }
        let text = rest.trim().to_string();
        for time_ms in times {
            lines.push(crate::models::LyricLine { time_ms, text: text.clone() });
        }
    }
    lines.sort_by_key(|line| line.time_ms);
    lines
}

fn lrc_stamp(tag: &str) -> Option<i64> {
    let mut parts = tag.split(':');
    let min: i64 = parts.next()?.parse().ok()?;
    let sec_part = parts.next()?;
    if parts.next().is_some() { return None; }
    let (sec, frac) = sec_part.split_once(['.', ':']).unwrap_or((sec_part, ""));
    let sec: i64 = sec.parse().ok()?;
    let frac_ms = if frac.is_empty() {
        0
    } else if frac.len() >= 3 {
        frac[..3].parse::<i64>().ok()?
    } else if frac.len() == 2 {
        frac.parse::<i64>().ok()? * 10
    } else {
        frac.parse::<i64>().ok()? * 100
    };
    Some(min * 60_000 + sec * 1000 + frac_ms)
}
