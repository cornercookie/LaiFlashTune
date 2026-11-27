#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod db;
mod models;
mod commands;
mod scanner;
mod audio_decode;
mod dsp;
mod eq;
mod player;
mod radio;
mod extras;
mod open_with;
mod transport;
mod tools;
mod next_track;
mod playback_pipeline;
mod prefetch_stream;

use crate::commands::{
    add_music_folder_cmd, add_song_to_playlist_cmd, app_paths, create_playlist_cmd,
    delete_playlist_cmd, get_lyrics_cmd, get_song_cmd, get_songs, list_music_folders_cmd, list_playlist_items_cmd,
    list_playlists_cmd, load_config_cmd, record_history, remove_from_playlist_cmd,
    remove_music_folder_cmd, save_config_cmd, get_config_cmd, save_lyrics_cmd, scan_folders, search_songs_cmd,
    set_lyrics_offset_cmd, song_played, update_playlist_order_cmd, update_playlist_cmd,
    add_smart_rule_cmd, list_smart_rules_cmd, clear_smart_rules_cmd, smart_playlist_songs_cmd,
};
use crate::commands::{
    player_load, player_play, player_preload, player_prepare, player_next, player_pause, player_position,
    player_prev, player_seek, player_set_mode,            player_set_volume, player_stop, player_toggle,
            player_status, player_set_eq, player_set_preamp, player_set_eq_enabled, player_set_fx, eq_response_cmd, player_output_devices, player_set_output, player_set_normalize, player_set_crossfade, player_play_url,
            player_set_next, player_clear_next, player_set_volume_curve, player_set_loudness_compensation,
            player_set_fade_settings, player_set_gap_ms, player_set_silence_removal,
    library_facets_cmd, songs_by_field_cmd, list_recent_cmd,
    read_tags_cmd, write_tags_cmd, write_tags_batch_cmd, ffmpeg_available_cmd, convert_audio_cmd, reveal_in_explorer_cmd, browse_folder_cmd, take_opened_file_cmd, song_by_path_cmd, smtc_update_cmd,
    pick_m3u_cmd, pick_m3u_save_cmd, import_m3u_cmd, export_m3u_cmd,
    list_radio_cmd, save_radio_cmd, rename_radio_cmd, delete_radio_cmd, classify_drop_cmd,
    player_set_default_player,
};
use crate::db::{db_path, init_db};
use crate::models::{DbState, OpenedFile, PlayerState};
use crate::open_with::{audio_arg, log_deliver, log_forward_result, log_line, log_load_error, forward_to_running, start_listener};
use crate::player::start_player_thread;
use crate::tools::{ffmpeg_available as tools_ffmpeg_available};
use tauri::{Builder, Manager};

include!(concat!(env!("OUT_DIR"), "/commands_manifest.rs"));

fn main() {
    log_line("=== main start ===");
    log_line(&format!("Tauri 允許命令數: {}", ALLOWED_COMMAND_COUNT));
    // S7: 預設不寫入 HKCU 檔案關聯。僅在使用者於設定頁面明確點選「設為預設播放器」時註冊。
    // open_with::register_windows_association();
    let opened = open_with::audio_arg();
    log_line(&format!("audio_arg -> {:?}", opened));
    if let Some(path) = &opened {
        log_line(&format!("trying forward_to_running for {path}"));
        if open_with::forward_to_running(path) {
            log_line("forward_to_running returned true, exiting early");
            return;
        }
        log_line("forward_to_running returned false, continuing as first instance");
    }
    // 第一次啟動：建立 data/ 資料夾與資料庫（可攜式：執行檔旁的 data/）
    let _ = std::fs::create_dir_all(db_path().parent().unwrap());

    let db_conn = crate::db::open_connection(&db_path()).expect("無法開啟資料庫");
    init_db(&db_conn).expect("資料庫初始化失敗");

    // 載入設定（在音訊執行緒啟動前讀取，啟動後套用）
    let config = crate::db::load_config(&db_conn).unwrap_or_default();

    // 啟動音訊執行緒（rodio 輸出 + symphonia 解碼，獨立於 UI 執行緒）
    let (player_tx, status_handle) = start_player_thread();

    // 套用音量曲線、響度補償、淡入淡出、曲間停頓、去除靜音設定到引擎
    let _ = player_tx.send(crate::models::PlayerCommand::SetVolumeCurve { curve: config.volume_curve });
    let _ = player_tx.send(crate::models::PlayerCommand::SetLoudnessCompensation { enabled: config.loudness_compensation });
    let _ = player_tx.send(crate::models::PlayerCommand::SetFadeSettings { 
        play_ms: config.fade_play_ms, 
        pause_ms: config.fade_pause_ms, 
        resume_ms: config.fade_resume_ms, 
        stop_ms: config.fade_stop_ms, 
        seek_ms: config.fade_seek_ms, 
        switch_ms: config.fade_switch_ms 
    });
    // gap_ms 與 crossfade_ms 互斥：啟動時以 app_config 為準，後寫者勝
    let mut gap_ms = config.gap_ms;
    let mut crossfade_ms = config.crossfade_ms;
    if crossfade_ms > 0 { gap_ms = 0; }
    else if gap_ms > 0 { crossfade_ms = 0; }
    let _ = player_tx.send(crate::models::PlayerCommand::SetGapMs { ms: gap_ms });
    let _ = player_tx.send(crate::models::PlayerCommand::SetCrossfade { ms: crossfade_ms });
    let _ = player_tx.send(crate::models::PlayerCommand::SetSilenceRemoval { enabled: config.silence_removal });

    Builder::default()
        .manage(DbState {
            conn: std::sync::Mutex::new(db_conn),
        })
        .manage(PlayerState {
            tx: std::sync::Mutex::new(Some(player_tx)),
            status: status_handle,
        })
        .manage(OpenedFile {
            path: std::sync::Mutex::new(opened),
        })
        .setup(|app| {
            open_with::start_listener(app.handle().clone());
            transport::install(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // JS 監聽 close-requested 後，Tauri 會先 prevent_close，只能靠 destroy() 收尾。
            // destroy 權限一旦漏掉，叉叉和系統匣「結束」都會沒反應。這裡在關閉請求後
            // 最多再等 3 秒，視窗還在就強制結束，避免程序關不掉。
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                let app = window.app_handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    if app.get_webview_window("main").is_some() {
                        app.exit(0);
                    }
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
                    // 播放器
                    player_load,
                    player_prepare,
                    player_preload,
                    player_set_next,
                    player_clear_next,
                    player_play,
                    player_pause,
                    player_toggle,
                    player_stop,
                    player_next,
                    player_prev,
                    player_seek,
                    player_set_volume,
                    player_set_mode,
                    player_position,
                    player_status,
                    player_set_eq,
                    player_set_preamp,
                    player_set_eq_enabled,
                    player_set_fx,
                    eq_response_cmd,
                    player_output_devices,
                    player_set_output,
                    player_set_normalize,
                    player_set_crossfade,
                    player_play_url,
                    player_set_volume_curve,
                    player_set_loudness_compensation,
                    player_set_fade_settings,
                    player_set_gap_ms,
                    player_set_silence_removal,
                    library_facets_cmd,
                    songs_by_field_cmd,
                    list_recent_cmd,
                    read_tags_cmd,
                    write_tags_cmd,
                    write_tags_batch_cmd,
                    ffmpeg_available_cmd,
                    convert_audio_cmd,
                    list_radio_cmd,
                    save_radio_cmd,
                    rename_radio_cmd,
                    delete_radio_cmd,
                    classify_drop_cmd,
                    player_set_default_player,
                    reveal_in_explorer_cmd,
                    browse_folder_cmd,
                    take_opened_file_cmd,
                    song_by_path_cmd,
                    smtc_update_cmd,
                    pick_m3u_cmd,
                    pick_m3u_save_cmd,
                    import_m3u_cmd,
                    export_m3u_cmd,
                    // 音樂庫與掃描
                    scan_folders,
                    get_songs,
                    search_songs_cmd,
                    get_song_cmd,
                    song_played,
                    record_history,
                    // 歌單
                    create_playlist_cmd,
                    list_playlists_cmd,
                    update_playlist_cmd,
                    delete_playlist_cmd,
                    add_song_to_playlist_cmd,
                    list_playlist_items_cmd,
                    remove_from_playlist_cmd,
                    update_playlist_order_cmd,
                    add_smart_rule_cmd,
                    list_smart_rules_cmd,
                    clear_smart_rules_cmd,
                    smart_playlist_songs_cmd,
                    // 歌詞
                    save_lyrics_cmd,
                    get_lyrics_cmd,
                    set_lyrics_offset_cmd,
                    // 資料夾與設定
                    add_music_folder_cmd,
                    list_music_folders_cmd,
                    remove_music_folder_cmd,
                    load_config_cmd,
                    save_config_cmd,
                    get_config_cmd,
                    // 路徑
                    app_paths,
                ])
        .run(tauri::generate_context!())
        .expect("錯誤：無法啟動 Tauri 應用程式");
}
