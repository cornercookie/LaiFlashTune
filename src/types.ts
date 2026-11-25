export interface Song {
  id: number;
  path: string;
  title?: string | null;
  artist?: string | null;
  album?: string | null;
  album_artist?: string | null;
  genre?: string | null;
  year?: number | null;
  duration_ms: number;
  file_mtime: number;
  file_size?: number | null;
  cover_path?: string | null;
  title_pinyin?: string | null;
  artist_pinyin?: string | null;
  title_pinyin_initial?: string | null;
  artist_pinyin_initial?: string | null;
  added_at?: string | null;
  play_count: number;
  last_played_at?: string | null;
  track_number?: number | null;
  disc_number?: number | null;
  format?: string | null;
  bitrate?: number | null;
  sample_rate?: number | null;
  channels?: number | null;
  missing?: number | null;
}

export interface Playlist {
  id: number;
  name: string;
  sort_order: number;
  created_at?: string | null;
  is_smart: number;
}

export interface PlaylistItem {
  id: number;
  playlist_id: number;
  song_id: number;
  position: number;
  added_at?: string | null;
  song?: Song | null;
}

export interface LyricLine {
  time_ms: number;
  text: string;
}

export interface Lyrics {
  song_id: number;
  source?: string | null;
  lines: LyricLine[];
  offset_ms: number;
}

export interface PlayModeValue {
  Sequence: null;
  Single: null;
  All: null;
  Random: null;
}

export type PlayMode = 'Sequence' | 'Single' | 'All' | 'Random';

export type ReplayGainMode = 'off' | 'track' | 'album' | 'auto';
export type VolumeNormalization = 'replaygain' | 'peak' | 'off';

export interface NewPlaylist {
  name: string;
  sort_order: number;
  is_smart?: number;
}

export interface NewPlaylistItem {
  playlist_id: number;
  song_id: number;
  position: number;
}

export type RadioStation = { id: number; name: string; url: string; created_at?: string | null };
export type DropPaths = { files: string[]; dirs: string[] };

export interface NewMusicFolder {
  path: string;
}

export interface PlaylistOrderUpdate {
  playlist_id: number;
  item_ids: number[];
}

export interface AppConfig {
  music_folders: string[];
  theme: string;
  lyrics_offset_ms: number;
  replaygain_mode: ReplayGainMode;
  volume_normalization: VolumeNormalization;
  rg_preamp_has: number;
  rg_preamp_none: number;
}

export interface ScanProgress {
  scanned: number;
  total: number;
  current_file: string;
  stage: string; // "scanning" | "finished" | "error"
  /// 逐檔掃描失敗清單（SP5）
  errors: ScanError[];
}

export interface ScanError {
  path: string;
  reason: string;
}

export interface EngineSnapshot {
  position_ms: number;
  duration_ms: number;
  is_playing: boolean;
  volume: number;
  song_id: number;
  ended: boolean;
  kind: string;
  stream_title: string;
  error: string;
  eq_bands: number[];
  eq_enabled: boolean;
  fx: import('./player/fx').Fx;
  preamp_db: number;
  normalize: boolean;
  crossfade_ms: number;
  applied_gain: number;
  band_hz: number[];
  mode: PlayMode;
  play_seq: number;
  now_token: number;
  next_token: number;
  next_clear_epoch: number;
  playback_state: string;
  applied_replaygain_db: number | null;
  has_replaygain: boolean;
  rg_mode: ReplayGainMode;
  volume_norm_mode: VolumeNormalization;
  rg_source: string;
}

export interface LibraryFacet {
  key: string;
  count: number;
}

export interface TagEdit {
  song_id: number;
  title: string;
  artist: string;
  album: string;
  genre: string;
  year?: number | null;
  track_number?: number | null;
  disc_number?: number | null;
}

export interface TagBatch {
  song_ids: number[];
  artist?: string | null;
  album?: string | null;
  genre?: string | null;
  year?: number | null;
}

export interface TagBatchResult {
  updated: number;
  failed: string[];
}

export interface ConvertRequest {
  song_id: number;
  format: string;
  output_dir: string;
  bitrate_kbps?: number | null;
}

