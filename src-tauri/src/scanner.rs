use crate::db::{covers_dir, diff_songs, upsert_song};
use crate::models::{DbState, NewSong, ScanProgress};
use lofty::{AudioFile, ItemKey};
use pinyin::{to_pinyin_vec, Pinyin};
use rayon::prelude::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

/// 支援的音訊副檔名（小寫，至少前 5 種：MP3 / FLAC / AAC / M4A / OGG / WAV / APE）
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "aac", "m4a", "ogg", "wav", "ape"];

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
    for folder in &folders {
        let path = Path::new(folder);
        if !path.is_dir() {
            continue;
        }
        for entry in WalkDir::new(path)
            .into_iter()
            .filter_entry(|e| !is_hidden(e))
            .filter_map(|e| e.ok())
        {
            let p = entry.into_path();
            if p.is_file() && is_audio_file(&p) {
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

    let results: Vec<Option<NewSong>> = to_process
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
                    },
                );
            }
            process_single_file(p)
        })
        .collect();

    // 第四階段：批次寫入資料庫（單一交易，快速）
    let new_songs: Vec<NewSong> = results.into_iter().filter_map(|s| s).collect();
    let mut conn = state.conn.lock().expect("資料庫 mutex 中毒");
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for song in &new_songs {
        upsert_song(&tx, song).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(conn);

    // 通知前端掃描完成
    let _ = app.emit(
        "scan-progress",
        ScanProgress {
            scanned: total,
            total,
            current_file: "掃描完成".to_string(),
            stage: "finished".to_string(),
        },
    );

    Ok(ScanProgress {
        scanned: total,
        total,
        current_file: format!("共處理 {} 首新/變更歌曲", new_songs.len()),
        stage: "finished".to_string(),
    })
}

/// 處理單一音樂檔案。沒有標籤或時長讀不到仍入庫，用檔名當歌名，否則掃描後曲庫是空的。
fn process_single_file(path: &Path) -> Option<NewSong> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
    let size = metadata.len() as i64;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("未命名").to_string();

    let tagged = lofty::read_from_path(path).ok();
    let duration_ms = tagged
        .as_ref()
        .map(|t| t.properties().duration().as_millis() as i64)
        .unwrap_or(0)
        .max(0);
    let tag = tagged.as_ref().and_then(|t| t.primary_tag());
    let get = |key: ItemKey| tag.and_then(|t| t.get_string(&key).map(|s| s.to_string()));
    let title = get(ItemKey::TrackTitle).filter(|s| !s.trim().is_empty()).or(Some(stem));
    let artist = get(ItemKey::TrackArtist);
    let album = get(ItemKey::AlbumTitle);
    let album_artist = get(ItemKey::AlbumArtist);
    let genre = get(ItemKey::Genre);
    let year = tag.and_then(|t| t.get_string(&ItemKey::Year)).and_then(|s| s.parse::<i32>().ok());
    let track_number = tag_number(tag.and_then(|t| t.get_string(&ItemKey::TrackNumber)));
    let disc_number = tag_number(tag.and_then(|t| t.get_string(&ItemKey::DiscNumber)));
    let props = tagged.as_ref().map(|t| t.properties());
    let bitrate = props.and_then(|p| p.audio_bitrate()).filter(|n| *n > 0).map(|n| n as i32);
    let sample_rate = props.and_then(|p| p.sample_rate()).filter(|n| *n > 0).map(|n| n as i32);
    let channels = props.and_then(|p| p.channels()).filter(|n| *n > 0).map(|n| n as i32);
    let format = path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).filter(|e| !e.is_empty());
    let cover_path = tag.and_then(|t| t.pictures().first()).and_then(|pic| generate_cover_thumbnail(pic.data(), path));

    let (title_pinyin, title_pinyin_initial) = compute_pinyin(title.as_deref().unwrap_or(""));
    let (artist_pinyin, artist_pinyin_initial) = compute_pinyin(artist.as_deref().unwrap_or(""));

    Some(NewSong {
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
        cover_path,
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
    })
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

fn is_audio_file(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()),
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


