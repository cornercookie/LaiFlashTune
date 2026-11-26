use std::env;
use std::fs;
use std::path::PathBuf;

/// 所有 main 視窗可呼叫的 Tauri 命令（S6 權限政策：AppManifest::commands）。
/// build.rs 產生此允許清單，main.rs 以 include! 引入，供安全審查與執行期檢查用。
const COMMANDS: &[&str] = &[
    // 播放器
    "player_load", "player_prepare", "player_preload", "player_set_next", "player_clear_next",
    "player_play", "player_pause", "player_toggle", "player_stop", "player_next", "player_prev",
    "player_seek", "player_set_volume", "player_set_mode", "player_position", "player_status",
    "player_set_eq", "player_set_preamp", "player_set_eq_enabled", "player_set_fx",
    "eq_response_cmd",
    "player_output_devices", "player_set_output", "player_set_normalize", "player_set_crossfade",
    "player_play_url",
    "player_set_replaygain_mode", "player_set_volume_normalization", "player_set_rg_preamp_has",
    "player_set_rg_preamp_none", "player_set_album_context",
    "player_set_volume_curve", "player_set_loudness_compensation",
    "player_set_fade_settings", "player_set_gap_ms", "player_set_silence_removal",
    "player_set_default_player",
    // 音樂庫 / 掃描
    "library_facets_cmd", "songs_by_field_cmd", "list_recent_cmd",
    "scan_folders", "get_songs", "search_songs_cmd", "get_song_cmd", "song_played", "record_history",
    // 標籤
    "read_tags_cmd", "write_tags_cmd", "write_tags_batch_cmd",
    // 歌單
    "create_playlist_cmd", "list_playlists_cmd", "update_playlist_cmd", "delete_playlist_cmd",
    "add_song_to_playlist_cmd", "list_playlist_items_cmd", "remove_from_playlist_cmd",
    "update_playlist_order_cmd",
    // 智慧歌單
    "add_smart_rule_cmd", "list_smart_rules_cmd", "clear_smart_rules_cmd", "smart_playlist_songs_cmd",
    // 歌詞
    "save_lyrics_cmd", "get_lyrics_cmd", "set_lyrics_offset_cmd",
    // 資料夾與設定
    "add_music_folder_cmd", "list_music_folders_cmd", "remove_music_folder_cmd",
    "load_config_cmd", "save_config_cmd", "get_config_cmd",
    // 檔案/路徑
    "app_paths", "ffmpeg_available_cmd", "convert_audio_cmd",
    // 電台
    "list_radio_cmd", "save_radio_cmd", "rename_radio_cmd", "delete_radio_cmd",
    // 拖放與視窗
    "classify_drop_cmd", "reveal_in_explorer_cmd", "browse_folder_cmd", "take_opened_file_cmd",
    "song_by_path_cmd", "smtc_update_cmd", "pick_m3u_cmd", "pick_m3u_save_cmd",
    "import_m3u_cmd", "export_m3u_cmd",
];

fn main() {
    let attributes = tauri_build::Attributes::default()
        .app_manifest(tauri_build::AppManifest::new().commands(&COMMANDS));
    tauri_build::try_build(attributes).expect("tauri build failed");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let manifest = out_dir.join("commands_manifest.rs");
    let mut content = String::from("/// Tauri 命令允許清單（由 build.rs 產生，對應 tauri.conf.json CSP / S6 政策）。\n");
    content.push_str("pub const ALLOWED_COMMANDS: &[&str] = &[\n");
    for cmd in COMMANDS {
        content.push_str(&format!("    {:?},\n", cmd));
    }
    content.push_str("];\n");
    content.push_str(&format!(
        "pub const ALLOWED_COMMAND_COUNT: usize = {};\n",
        COMMANDS.len()
    ));
    fs::write(&manifest, content).expect("寫入 commands_manifest.rs 失敗");
    println!("cargo:rerun-if-changed=build.rs");
}
