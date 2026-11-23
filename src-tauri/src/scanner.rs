use crate::db::{covers_dir, diff_songs, upsert_song};
use crate::models::{DbState, NewSong, ScanError, ScanProgress};
use crate::open_with::validate_local_path;
use lofty::{AudioFile, ItemKey, Tag};
use pinyin::{to_pinyin_vec, Pinyin};
use rayon::prelude::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

/// 格式矩陣：掃描與右鍵開啟的副檔名清單都從這裡產生
/// P0 驗收範圍：期別欄是「P0」「AIFF 補掃描」「M4B 補」「已有」的列
const FORMAT_MATRIX: &[FormatEntry] = &[
    // MP3、MP1、MP2
    FormatEntry { ext: "mp3", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::P0 },
    FormatEntry { ext: "mp1", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "mp2", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // FLAC
    FormatEntry { ext: "flac", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::Existing },
    // WAV、AIFF
    FormatEntry { ext: "wav", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::Existing },
    FormatEntry { ext: "aiff", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::AiffScan },
    FormatEntry { ext: "aif", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::AiffScan },
    // Ogg Vorbis、Speex
    FormatEntry { ext: "ogg", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::P0 },
    FormatEntry { ext: "spx", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // Opus
    FormatEntry { ext: "opus", backend: Backend::Ffmpeg, meta: MetaBackend::Lofty, tier: Tier::P0 },
    // AAC、ALAC、M4A、M4B、MP4
    FormatEntry { ext: "aac", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::P0 },
    FormatEntry { ext: "m4a", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::P0 },
    FormatEntry { ext: "m4b", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::M4bScan },
    FormatEntry { ext: "mp4", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::P0 },
    FormatEntry { ext: "alac", backend: Backend::Symphonia, meta: MetaBackend::Lofty, tier: Tier::P0 },
    // APE
    FormatEntry { ext: "ape", backend: Backend::Ffmpeg, meta: MetaBackend::Lofty, tier: Tier::P0 },
    // WavPack
    FormatEntry { ext: "wv", backend: Backend::Ffmpeg, meta: MetaBackend::Lofty, tier: Tier::P0 },
    // TTA
    FormatEntry { ext: "tta", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // TAK
    FormatEntry { ext: "tak", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // Musepack
    FormatEntry { ext: "mpc", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // WMA
    FormatEntry { ext: "wma", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // DSD
    FormatEntry { ext: "dsf", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "dff", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // AC3、DTS
    FormatEntry { ext: "ac3", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "dts", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // Matroska、WebM 音訊
    FormatEntry { ext: "mka", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "webm", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // ATRAC
    FormatEntry { ext: "aa3", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "at3", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "oma", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // 模組音樂
    FormatEntry { ext: "mod", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "xm", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "it", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "s3m", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "mtm", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "umx", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "mo3", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    // 壓縮模組
    FormatEntry { ext: "mdz", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "s3z", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "xmz", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
    FormatEntry { ext: "itz", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::P0 },
        // OptimFROG - 受阻：本機 ffmpeg 沒有解碼器
        FormatEntry { ext: "ofr", backend: Backend::Ffmpeg, meta: MetaBackend::Ffprobe, tier: Tier::Blocked },
    ];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    Symphonia,
    Ffmpeg,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MetaBackend {
    Lofty,
    Ffprobe,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Existing,      // 已有
    P0,            // 本期驗收
    AiffScan,      // AIFF 補掃描
    M4bScan,       // M4B 補
    Blocked,       // 受阻
    Future,        // P3/P4
}

struct FormatEntry {
    ext: &'static str,
    backend: Backend,
    meta: MetaBackend,
    tier: Tier,
}

/// 取得本期驗收範圍內的所有副檔名（掃描與右鍵開啟用）
pub fn supported_extensions() -> Vec<&'static str> {
    FORMAT_MATRIX
        .iter()
        .filter(|e| matches!(e.tier, Tier::Existing | Tier::P0 | Tier::AiffScan | Tier::M4bScan))
        .map(|e| e.ext)
        .collect()
}

/// 取得本期驗收範圍內的 ffmpeg 格式副檔名
pub fn ffmpeg_extensions() -> Vec<&'static str> {
    FORMAT_MATRIX
        .iter()
        .filter(|e| matches!(e.tier, Tier::Existing | Tier::P0 | Tier::AiffScan | Tier::M4bScan) && e.backend == Backend::Ffmpeg)
        .map(|e| e.ext)
        .collect()
}

/// 取得本期驗收範圍內的 symphonia 格式副檔名
pub fn symphonia_extensions() -> Vec<&'static str> {
    FORMAT_MATRIX
        .iter()
        .filter(|e| matches!(e.tier, Tier::Existing | Tier::P0 | Tier::AiffScan | Tier::M4bScan) && e.backend == Backend::Symphonia)
        .map(|e| e.ext)
        .collect()
}

/// 檢查副檔名是否為受阻格式
pub fn is_blocked_format(ext: &str) -> bool {
    FORMAT_MATRIX.iter().any(|e| e.ext == ext && e.tier == Tier::Blocked)
}

/// 檢查副檔名是否為未來格式（P3/P4）
pub fn is_future_format(ext: &str) -> bool {
    FORMAT_MATRIX.iter().any(|e| e.ext == ext && e.tier == Tier::Future)
}

/// 取得格式的中繼資料後端（受阻格式回傳 None）
pub fn meta_backend_for(ext: &str) -> Option<MetaBackend> {
    FORMAT_MATRIX.iter()
        .find(|e| e.ext == ext && e.tier != Tier::Blocked)
        .map(|e| e.meta)
}

/// 取得格式的播放後端（受阻格式回傳 None，不會進入播放路徑）
pub fn playback_backend_for(ext: &str) -> Option<Backend> {
    FORMAT_MATRIX.iter()
        .find(|e| e.ext == ext && e.tier != Tier::Blocked)
        .map(|e| e.backend)
}

/// 封面縮圖目標尺寸
const COVER_SIZE: u32 = 200;

/// 掃描指定的音樂資料夾，建立/更新資料庫與封面縮圖。
/// 進度透過 Tauri event `scan-progress` 傳給前端。
#[tauri::command]
pub fn scan_music_folders(
    app: AppHandle,
    state: tauri::State<'_, DbState>,
    folders: Vec<String>,
) -> Result<ScanProgress, String> {
    // 第一階段：平行走訪所有音樂資料夾，收集檔案路徑
    let mut files: Vec<PathBuf> = Vec::new();
    let supported_exts = supported_extensions();
    for folder in &folders {
        let path = Path::new(folder);
        // SE1：首次檔案系統存取前必須先驗證本機路徑
        if validate_local_path(path).is_err() {
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        for entry in WalkDir::new(path)
            .into_iter()
            .filter_entry(|e| !is_hidden(e))
            .filter_map(|e| e.ok())
        {
            let p = entry.into_path();
            // SE1：在 is_file() / metadata() 之前先驗證本機路徑，避免符號連結轉往網路
            if crate::open_with::validate_local_path(&p).is_err() {
                continue;
            }
            if p.is_file() && is_audio_file(&p, &supported_exts) {
                files.push(p);
            }
        }
    }

    let total = files.len() as i64;
    let scanned = AtomicI64::new(0);

    // 通知前端掃描開始
    let _ = app.emit(
        "scan-progress",
        ScanProgress {
            scanned: 0,
            total,
            current_file: "準備掃描...".to_string(),
            stage: "scanning".to_string(),
            errors: Vec::new(),
        },
    );

    // 第二階段：取得每個檔案的 mtime + size，與 DB 做增量差比
    let file_meta: Vec<(PathBuf, i64, i64)> = files
        .par_iter()
        .filter_map(|p| {
            let md = std::fs::metadata(p).ok()?;
            let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
            let size = md.len() as i64;
            Some((p.clone(), mtime, size))
        })
        .collect();

    let mut conn = state.conn.lock().expect("資料庫 mutex 中毒");
    let meta_for_diff: Vec<(String, i64, Option<i64>)> = file_meta
        .iter()
        .map(|(p, m, s)| (p.to_string_lossy().to_string(), *m, Some(*s)))
        .collect();
    let (need_rescan, missing) = diff_songs(&conn, &meta_for_diff).map_err(|e| e.to_string())?;

    for (path, _, _) in &meta_for_diff {
        let _ = conn.execute("UPDATE songs SET missing = 0 WHERE path = ?1", [path]);
    }
    for path in &missing {
        if path_under_folders(path, &folders) {
            let _ = conn.execute("UPDATE songs SET missing = 1 WHERE path = ?1", [path]);
        }
    }
    let existing: std::collections::HashSet<String> = {
        let mut stmt = conn.prepare("SELECT path FROM songs").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    drop(conn); // 釋放鎖，後續批次寫入再取用

    let need_set: HashMap<String, bool> = need_rescan.iter().cloned().map(|p| (p, true)).collect();

    // 新增的檔案不在 need_rescan 裡。沒有這段，第一次掃描會寫入 0 首。
    let to_process: Vec<&PathBuf> = file_meta
        .iter()
        .filter(|(p, _, _)| {
            let key = p.to_string_lossy().to_string();
            !existing.contains(&key) || need_set.contains_key(&key)
        })
        .map(|(p, _, _)| p)
        .collect();

    let results: Vec<Option<(NewSong, Vec<crate::models::ScanError>)>> = to_process
        .par_iter()
        .map(|p| {
            let n = scanned.fetch_add(1, Ordering::Relaxed) + 1;
            // 每 50 個檔案發一次進度，避免事件洪水
            if n % 50 == 0 || n == total {
                let _ = app.emit(
                    "scan-progress",
                    ScanProgress {
                        scanned: n,
                        total,
                        current_file: p.file_name().unwrap_or_default().to_string_lossy().to_string(),
                        stage: "scanning".to_string(),
                        errors: Vec::new(),
                    },
                );
            }
            process_single_file(p)
        })
        .collect();

    // 第四階段：批次寫入資料庫（單一交易，快速）並彙整掃描錯誤清單
    let mut new_songs: Vec<NewSong> = Vec::new();
    let mut errors: Vec<crate::models::ScanError> = Vec::new();
    for r in results {
        if let Some((song, mut errs)) = r {
            errors.append(&mut errs);
            new_songs.push(song);
        }
    }
    let mut conn = state.conn.lock().expect("資料庫 mutex 中毒");
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for song in &new_songs {
        upsert_song(&tx, song).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(conn);

    // 通知前端掃描完成，攜帶逐檔錯誤清單
    let _ = app.emit(
        "scan-progress",
        ScanProgress {
            scanned: total,
            total,
            current_file: "掃描完成".to_string(),
            stage: "finished".to_string(),
            errors: errors.clone(),
        },
    );

    Ok(ScanProgress {
        scanned: total,
        total,
        current_file: format!("共處理 {} 首新/變更歌曲", new_songs.len()),
        stage: "finished".to_string(),
        errors,
    })
}

/// ffprobe 探測結果（SP5 後備、SE6 上限）
struct FfprobeMeta {
    duration_ms: i64,
    sample_rate: Option<i32>,
    channels: Option<i32>,
    bitrate: Option<i32>,
    tags: HashMap<String, String>,
}

/// ffprobe 後備中繼資料（SE6：64 KB / 10 秒上限）
fn ffprobe_meta(path: &Path) -> Result<FfprobeMeta, String> {
    let ffprobe_path = crate::tools::locate("ffprobe.exe")
        .map(|(p, _, _)| p)
        .ok_or("找不到 ffprobe")?;
    let value = crate::audio_decode::ffprobe_json(&ffprobe_path, &path.to_string_lossy())?;

    let streams = value["streams"].as_array().ok_or("無音訊串流")?;
    let audio_stream = streams.iter().find(|s| s["codec_type"] == "audio").ok_or("無音訊串流")?;

    let sample_rate = audio_stream["sample_rate"]
        .as_str()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|v| *v > 0);
    let channels = audio_stream["channels"].as_u64().map(|v| v as i32).filter(|v| *v > 0);
    let bitrate = audio_stream["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|v| *v > 0)
        .or_else(|| {
            value["format"]["bit_rate"]
                .as_str()
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|v| *v > 0)
        });
    let duration = value["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);
    let duration_ms = (duration * 1000.0) as i64;

    let mut tags = HashMap::new();
    if let Some(fmt_tags) = value["format"]["tags"].as_object() {
        for (k, v) in fmt_tags {
            if let Some(s) = v.as_str() {
                tags.insert(k.to_lowercase(), s.to_string());
            }
        }
    }
    if let Some(stream_tags) = audio_stream["tags"].as_object() {
        for (k, v) in stream_tags {
            if let Some(s) = v.as_str() {
                tags.entry(k.to_lowercase()).or_insert(s.to_string());
            }
        }
    }

    Ok(FfprobeMeta {
        duration_ms,
        sample_rate,
        channels,
        bitrate,
        tags,
    })
}

/// SE7：標籤大小上限（單欄 4 KB、總計 16 KB），超長則截斷並記錯誤
const TAG_FIELD_MAX: usize = 4 * 1024;
const TAG_TOTAL_MAX: usize = 16 * 1024;
const SAMPLE_RATE_MIN: i32 = 8000;
const SAMPLE_RATE_MAX: i32 = 384000;
const CHANNELS_MIN: i32 = 1;
const CHANNELS_MAX: i32 = 8;

fn limited_tag(tag: &Tag, key: ItemKey, total: &mut usize) -> Option<String> {
    let s = tag.get_string(&key).map(|s| s.to_string())?;
    if s.len() > TAG_FIELD_MAX {
        return None; // 欄位過長，視為不可用
    }
    if *total + s.len() > TAG_TOTAL_MAX {
        return None; // 總長超標
    }
    *total += s.len();
    Some(s)
}

/// 處理單一音樂檔案。沒有標籤或時長讀不到仍入庫，用檔名當歌名，失敗記入錯誤清單。
fn process_single_file(path: &Path) -> Option<(NewSong, Vec<ScanError>)> {
    // SE1：首次檔案系統存取前必須先驗證本機路徑
    if validate_local_path(path).is_ok() == false {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    let size = metadata.len() as i64;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("未命名").to_string();
    let format = path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).filter(|e| !e.is_empty());

    let mut errors = Vec::new();
    let mut total_tag_bytes: usize = 0;

    let mut duration_ms = 0i64;
    let mut sample_rate: Option<i32> = None;
    let mut channels: Option<i32> = None;
    let mut bitrate: Option<i32> = None;
    let mut tags: HashMap<String, String> = HashMap::new();

    // SE7：檔案過大時跳過 lofty 完整解析（避免載入大封面圖片到記憶體），直接用 ffprobe 後備
    const LOFTY_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
    let lofty_ok = metadata.len() <= LOFTY_MAX_FILE_SIZE && lofty::read_from_path(path).is_ok_and(|tagged| {
        let props = tagged.properties();
        duration_ms = props.duration().as_millis() as i64;
        sample_rate = props.sample_rate().filter(|v| *v > 0).map(|v| v as i32);
        channels = props.channels().filter(|v| *v > 0).map(|v| v as i32);
        bitrate = props.audio_bitrate().filter(|v| *v > 0).map(|v| v as i32);
        if let Some(tag) = tagged.primary_tag() {
            for (key, dst) in [
                (ItemKey::TrackTitle, "title"),
                (ItemKey::TrackArtist, "artist"),
                (ItemKey::AlbumTitle, "album"),
                (ItemKey::AlbumArtist, "album_artist"),
                (ItemKey::Genre, "genre"),
                (ItemKey::Year, "date"),
            ] {
                if let Some(s) = limited_tag(tag, key, &mut total_tag_bytes) {
                    tags.insert(dst.to_string(), s);
                }
            }
        }
        true
    });

    // SE5：lofty 不支援或讀失敗時，用 ffprobe 後備
    if !lofty_ok {
        match ffprobe_meta(path) {
            Ok(meta) => {
                duration_ms = meta.duration_ms;
                sample_rate = meta.sample_rate;
                channels = meta.channels;
                bitrate = meta.bitrate;
                tags = meta.tags;
            }
            Err(e) => {
                errors.push(ScanError {
                    path: path.to_string_lossy().to_string(),
                    reason: e,
                });
            }
        }
    }

    // SE7：取樣率與聲道驗證
    if let Some(sr) = sample_rate {
        if sr < SAMPLE_RATE_MIN || sr > SAMPLE_RATE_MAX {
            errors.push(ScanError {
                path: path.to_string_lossy().to_string(),
                reason: format!("取樣率 {sr} Hz 超出有效範圍"),
            });
            sample_rate = None;
        }
    }
    if let Some(ch) = channels {
        if ch < CHANNELS_MIN || ch > CHANNELS_MAX {
            errors.push(ScanError {
                path: path.to_string_lossy().to_string(),
                reason: format!("聲道 {ch} 超出有效範圍"),
            });
            channels = None;
        }
    }

    let title = tags.remove("title").filter(|s| !s.trim().is_empty()).or(Some(stem));
    let artist = tags.remove("artist");
    let album = tags.remove("album");
    let album_artist = tags.remove("album_artist");
    let genre = tags.remove("genre");
    let year = tags.remove("date").and_then(|s| s.parse::<i32>().ok());
    let track_number = tag_number(tags.get("track").map(|s| s.as_str()));
    let disc_number = tag_number(tags.get("disc").map(|s| s.as_str()));

    let (title_pinyin, title_pinyin_initial) = compute_pinyin(title.as_deref().unwrap_or(""));
    let (artist_pinyin, artist_pinyin_initial) = compute_pinyin(artist.as_deref().unwrap_or(""));

    Some((
        NewSong {
            path: path.to_string_lossy().to_string(),
            title,
            artist,
            album,
            album_artist,
            genre,
            year,
            duration_ms,
            file_mtime: mtime,
            file_size: Some(size),
            cover_path: None, // SE7：不載入嵌入封面
            title_pinyin: if title_pinyin.is_empty() { None } else { Some(title_pinyin) },
            artist_pinyin: if artist_pinyin.is_empty() { None } else { Some(artist_pinyin) },
            title_pinyin_initial: if title_pinyin_initial.is_empty() { None } else { Some(title_pinyin_initial) },
            artist_pinyin_initial: if artist_pinyin_initial.is_empty() { None } else { Some(artist_pinyin_initial) },
            track_number,
            disc_number,
            format,
            bitrate,
            sample_rate,
            channels,
        },
        errors,
    ))
}

fn tag_number(raw: Option<&str>) -> Option<i32> {
    let head = raw?.split(['/', ';']).next()?.trim();
    let n: i32 = head.parse().ok()?;
    if n <= 0 { None } else { Some(n) }
}

fn path_under_folders(path: &str, folders: &[String]) -> bool {
    let norm = path.replace('/', "\\").to_lowercase();
    folders.iter().any(|folder| {
        let mut prefix = folder.replace('/', "\\");
        if !prefix.ends_with('\\') {
            prefix.push('\\');
        }
        norm.starts_with(&prefix.to_lowercase())
    })
}

/// 從嵌入封面圖片產生 200x200 JPG 縮圖，儲存於 data/covers/，回傳縮圖路徑。
fn generate_cover_thumbnail(image_data: &[u8], source_path: &Path) -> Option<String> {
    let img = image::load_from_memory(image_data).ok()?;
    let img = img.resize(COVER_SIZE, COVER_SIZE, image::imageops::FilterType::Triangle);
    let cover_dir = covers_dir();
    let hash = path_hash(source_path);
    let out_path = cover_dir.join(format!("{}.jpg", hash));
    img.save_with_format(&out_path, image::ImageFormat::Jpeg).ok()?;
    Some(out_path.to_string_lossy().to_string())
}

/// 計算拼音與拼音首字母（例如：周杰倫 -> zhoujielun / zjl）。
pub(crate) fn compute_pinyin(text: &str) -> (String, String) {
    if text.is_empty() {
        return (String::new(), String::new());
    }
    let pinyin: String = to_pinyin_vec(text, Pinyin::plain).join(" ");
    let initial: String = to_pinyin_vec(text, Pinyin::first_letter).join("");
    (pinyin, initial)
}

fn path_hash(path: &Path) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

fn is_audio_file(path: &Path, supported_exts: &[&str]) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => supported_exts.contains(&ext.to_lowercase().as_str()),
        None => false,
    }
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with('.'))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_matrix_extensions() {
        let exts = supported_extensions();
        // 基本格式應該都在
        assert!(exts.contains(&"mp3"));
        assert!(exts.contains(&"flac"));
        assert!(exts.contains(&"wav"));
        assert!(exts.contains(&"aiff"));
        assert!(exts.contains(&"ogg"));
        assert!(exts.contains(&"opus"));
        assert!(exts.contains(&"m4a"));
        assert!(exts.contains(&"m4b"));
        assert!(exts.contains(&"ape"));
        assert!(exts.contains(&"wv"));
        assert!(exts.contains(&"tta"));
        
        // 受阻格式不應該在
        assert!(!exts.contains(&"ofr"));
        assert!(!exts.contains(&"mid"));
        assert!(!exts.contains(&"cda"));
    }

    #[test]
    fn test_meta_backend_for() {
        assert_eq!(meta_backend_for("mp3"), Some(MetaBackend::Lofty));
        assert_eq!(meta_backend_for("flac"), Some(MetaBackend::Lofty));
        assert_eq!(meta_backend_for("ape"), Some(MetaBackend::Lofty));
        assert_eq!(meta_backend_for("tta"), Some(MetaBackend::Ffprobe));
        assert_eq!(meta_backend_for("wma"), Some(MetaBackend::Ffprobe));
        assert_eq!(meta_backend_for("ofr"), None);
    }

    #[test]
    fn test_playback_backend_for() {
        assert_eq!(playback_backend_for("mp3"), Some(Backend::Symphonia));
        assert_eq!(playback_backend_for("flac"), Some(Backend::Symphonia));
        assert_eq!(playback_backend_for("ape"), Some(Backend::Ffmpeg));
        assert_eq!(playback_backend_for("wma"), Some(Backend::Ffmpeg));
    }

    #[test]
    fn test_is_blocked_format() {
        assert!(is_blocked_format("ofr"));
        assert!(!is_blocked_format("mp3"));
        assert!(!is_blocked_format("flac"));
    }
}
