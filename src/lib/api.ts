import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import type {
  Song,
  Playlist,
  PlaylistItem,
  Lyrics,
  AppConfig,
  ScanProgress,
  PlayMode,
  NewPlaylist,
  NewPlaylistItem,
  NewMusicFolder,
  PlaylistOrderUpdate,
  RadioStation,
  DropPaths,
} from '../types';

// Tauri v2：指令參數名是 camelCase。送 snake_case 會直接被拒。
// 巢狀結構（playlist / item / edit）仍用 Rust 欄位名。

export async function invokeCmd<T = any>(cmd: string, args: Record<string, any> = {}): Promise<T> {
  return invoke<T>(cmd, args);
}

/* ---------- 播放器 ---------- */
export const playerLoad = (path: string, songId: number) => invokeCmd('player_load', { path, songId });
export const playerPrepare = (path: string, songId: number, positionMs: number) =>
  invokeCmd('player_prepare', { path, songId, positionMs });
export const playerPreload = (path: string, songId: number) => invokeCmd('player_preload', { path, songId });
export const playerPlay = () => invokeCmd('player_play');
export const playerPause = () => invokeCmd('player_pause');
export const playerToggle = () => invokeCmd('player_toggle');
export const playerStop = () => invokeCmd('player_stop');
export const playerNext = () => invokeCmd('player_next');
export const playerPrev = () => invokeCmd('player_prev');
export const playerSeek = (positionMs: number) => invokeCmd('player_seek', { positionMs });
export const playerSetVolume = (volume: number) => invokeCmd('player_set_volume', { volume });
export const playerSetMode = (mode: PlayMode) => invokeCmd('player_set_mode', { mode });
export const playerPosition = () => invokeCmd<number>('player_position');

/* ---------- 音樂庫 ---------- */
export const scanFolders = (folders: string[]) => invokeCmd<ScanProgress>('scan_folders', { folders });
export const getSongs = (limit: number, offset: number, orderBy: string, orderDir: string) =>
  invokeCmd<Song[]>('get_songs', { limit, offset, orderBy, orderDir });
export const searchSongs = (query: string, limit: number) =>
  invokeCmd<{ songs: Song[]; total: number }>('search_songs_cmd', { query, limit });
export const getSong = (id: number) => invokeCmd<Song | null>('get_song_cmd', { id });
export const songPlayed = (id: number) => invokeCmd('song_played', { id });
export const recordHistory = (id: number) => invokeCmd('record_history', { id });

/* ---------- 歌單 ---------- */
export const createPlaylist = (p: NewPlaylist) => invokeCmd<number>('create_playlist_cmd', { playlist: p });
export const listPlaylists = () => invokeCmd<Playlist[]>('list_playlists_cmd');
export const updatePlaylist = (id: number, name: string) => invokeCmd('update_playlist_cmd', { id, name });
export const deletePlaylist = (id: number) => invokeCmd('delete_playlist_cmd', { id });
export const addSongToPlaylist = (item: NewPlaylistItem) => invokeCmd<number>('add_song_to_playlist_cmd', { item });
export const listPlaylistItems = (playlistId: number) =>
  invokeCmd<PlaylistItem[]>('list_playlist_items_cmd', { playlistId });
export const removeFromPlaylist = (itemId: number) => invokeCmd('remove_from_playlist_cmd', { itemId });
export const updatePlaylistOrder = (playlistId: number, itemIds: number[]) =>
  invokeCmd('update_playlist_order_cmd', { update: { playlist_id: playlistId, item_ids: itemIds } satisfies PlaylistOrderUpdate });

/* ---------- 歌詞 ---------- */
export const saveLyrics = (songId: number, lyrics: Lyrics) => invokeCmd('save_lyrics_cmd', { songId, lyrics });
export const getLyrics = (songId: number) => invokeCmd<Lyrics | null>('get_lyrics_cmd', { songId });
export const setLyricsOffset = (songId: number, offsetMs: number) =>
  invokeCmd('set_lyrics_offset_cmd', { songId, offsetMs });
export const pickM3u = () => invokeCmd<string | null>('pick_m3u_cmd');
export const pickM3uSave = (name: string) => invokeCmd<string | null>('pick_m3u_save_cmd', { name });
export const importM3u = (path: string) => invokeCmd<{ playlist_id: number | null; name: string; matched: number; missing: string[] }>('import_m3u_cmd', { path });
export const exportM3u = (playlistId: number, path: string) => invokeCmd('export_m3u_cmd', { playlistId, path });
export const smtcUpdate = (title: string, artist: string, playing: boolean) =>
  invokeCmd('smtc_update_cmd', { title, artist, playing });

/* ---------- 資料夾與設定 ---------- */
export const addMusicFolder = (f: NewMusicFolder) => invokeCmd<number>('add_music_folder_cmd', { folder: f });
export const listMusicFolders = () => invokeCmd<{ id: number; path: string; added_at?: string | null }[]>('list_music_folders_cmd');
export const removeMusicFolder = (path: string) => invokeCmd('remove_music_folder_cmd', { path });
export const loadConfig = () => invokeCmd<AppConfig>('load_config_cmd');
export const saveConfig = (key: string, value: string) => invokeCmd('save_config_cmd', { key, value });
export const getConfig = (key: string) => invokeCmd<string | null>('get_config_cmd', { key });
export const appPaths = () => invokeCmd<{ db: string; covers: string; config: string }>('app_paths');

/* ---------- 事件 ---------- */
export const playerStatus = () => invokeCmd<import('../types').EngineSnapshot>('player_status');
export const playerSetEq = (bands: number[]) => invokeCmd('player_set_eq', { bands });
export const playerSetPreamp = (db: number) => invokeCmd('player_set_preamp', { db });
export const playerSetNormalize = (enabled: boolean) => invokeCmd('player_set_normalize', { enabled });
export const playerSetCrossfade = (ms: number) => invokeCmd('player_set_crossfade', { ms });
export const playerPlayUrl = (url: string) => invokeCmd('player_play_url', { url });
export const playerOutputDevices = () => invokeCmd<string[]>('player_output_devices');
export const playerSetOutput = (name: string) => invokeCmd('player_set_output', { name });
export const libraryFacets = (field: string) => invokeCmd<import('../types').LibraryFacet[]>('library_facets_cmd', { field });
export const songsByField = (field: string, value: string, limit: number, offset: number, orderBy: string, orderDir: string) =>
  invokeCmd<Song[]>('songs_by_field_cmd', { field, value, limit, offset, orderBy, orderDir });
export const listRecent = (limit: number) => invokeCmd<Song[]>('list_recent_cmd', { limit });
export const readTags = (songId: number) => invokeCmd<import('../types').TagEdit>('read_tags_cmd', { songId });
export const writeTags = (edit: import('../types').TagEdit) => invokeCmd('write_tags_cmd', { edit });
export const writeTagsBatch = (batch: import('../types').TagBatch) =>
  invokeCmd<import('../types').TagBatchResult>('write_tags_batch_cmd', { batch });
export const ffmpegAvailable = () => invokeCmd<boolean>('ffmpeg_available_cmd');
export const convertAudio = (request: import('../types').ConvertRequest) => invokeCmd<string>('convert_audio_cmd', { request });

export const listRadio = () => invokeCmd<RadioStation[]>('list_radio_cmd');
export const saveRadio = (name: string, url: string) => invokeCmd<number>('save_radio_cmd', { name, url });
export const renameRadio = (id: number, name: string) => invokeCmd('rename_radio_cmd', { id, name });
export const deleteRadio = (id: number) => invokeCmd('delete_radio_cmd', { id });

export const classifyDrop = (paths: string[]) => invokeCmd<DropPaths>('classify_drop_cmd', { paths });
export const addSmartRule = (playlistId: number, field: string, operator: string, value: string) =>
  invokeCmd<number>('add_smart_rule_cmd', { playlistId, field, operator, value });
export const listSmartRules = (playlistId: number) =>
  invokeCmd<{ field: string; operator: string; value: string }[]>('list_smart_rules_cmd', { playlistId });
export const smartPlaylistSongs = (playlistId: number, limit: number, offset: number) =>
  invokeCmd<Song[]>('smart_playlist_songs_cmd', { playlistId, limit, offset });
export const revealInExplorer = (path: string) => invokeCmd('reveal_in_explorer_cmd', { path });
export const browseFolder = () => invokeCmd<string | null>('browse_folder_cmd');
export const takeOpenedFile = () => invokeCmd<string | null>('take_opened_file_cmd');
export const songByPath = (path: string) => invokeCmd<Song | null>('song_by_path_cmd', { path });
export const onOpenFile = (cb: (path: string) => void): Promise<UnlistenFn> =>
  listen<string>('open-file', (event) => cb(event.payload));

export const onScanProgress = (cb: (payload: ScanProgress) => void): Promise<UnlistenFn> =>
  listen<ScanProgress>('scan-progress', (event) => cb(event.payload));
