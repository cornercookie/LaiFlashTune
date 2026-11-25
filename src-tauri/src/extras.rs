use crate::db::{db_lock, get_song};
use crate::models::{ConvertRequest, DbState, LibraryFacet, Song, TagEdit};
use crate::open_with::sanitize_log;
use crate::open_with::validate_local_path;
use crate::scanner::compute_pinyin;
use crate::tools::{locate, ffmpeg_available as tools_ffmpeg_available};
use lofty::ItemKey;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SONG_COLS: &str = "id, path, title, artist, album, album_artist, genre, year, duration_ms, \
    file_mtime, file_size, cover_path, title_pinyin, artist_pinyin, title_pinyin_initial, \
    artist_pinyin_initial, added_at, play_count, last_played_at, track_number, disc_number, \
    format, bitrate, sample_rate, channels, missing, replaygain_track_gain, replaygain_track_peak, \
    replaygain_album_gain, replaygain_album_peak";

fn song_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Song> {
    Ok(Song {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        album_artist: row.get(5)?,
        genre: row.get(6)?,
        year: row.get(7)?,
        duration_ms: row.get(8)?,
        file_mtime: row.get(9)?,
        file_size: row.get(10)?,
        cover_path: row.get(11)?,
        title_pinyin: row.get(12)?,
        artist_pinyin: row.get(13)?,
        title_pinyin_initial: row.get(14)?,
        artist_pinyin_initial: row.get(15)?,
        added_at: row.get(16)?,
        play_count: row.get(17)?,
        last_played_at: row.get(18)?,
        track_number: row.get(19)?,
        disc_number: row.get(20)?,
        format: row.get(21)?,
        bitrate: row.get(22)?,
        sample_rate: row.get(23)?,
        channels: row.get(24)?,
        missing: row.get::<_, Option<i32>>(25)?.unwrap_or(0),
        replaygain_track_gain: row.get(26)?,
        replaygain_track_peak: row.get(27)?,
        replaygain_album_gain: row.get(28)?,
        replaygain_album_peak: row.get(29)?,
    })
}

fn order_sql(order_by: &str, order_dir: &str) -> String {
    let col = match order_by {
        "title" | "artist" | "album" | "genre" | "year" | "duration_ms" | "play_count" | "path" | "file_mtime" | "added_at" => order_by,
        _ => "title",
    };
    let dir = if order_dir.eq_ignore_ascii_case("desc") { "DESC" } else { "ASC" };
    format!("{col} {dir}")
}

fn facet_column(field: &str) -> Result<&'static str, String> {
    match field {
        "artist" => Ok("artist"),
        "album" => Ok("album"),
        "genre" => Ok("genre"),
        _ => Err("只支援 artist、album、genre".into()),
    }
}

/// 演出者 / 專輯 / 曲風計數。空值併成「未知」。
pub fn library_facets(state: &DbState, field: &str) -> Result<Vec<LibraryFacet>, String> {
    if field == "folder" {
        return folder_facets(state);
    }
    let col = facet_column(field)?;
    let conn = db_lock(state);
    let sql = format!(
        "SELECT COALESCE(NULLIF({col}, ''), '未知'), COUNT(*) FROM songs GROUP BY 1 ORDER BY 1"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok(LibraryFacet { key: row.get(0)?, count: row.get(1)? }))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn folder_facets(state: &DbState) -> Result<Vec<LibraryFacet>, String> {
    let conn = db_lock(state);
    let mut stmt = conn.prepare("SELECT path FROM songs").map_err(|e| e.to_string())?;
    let paths = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok);
    let mut map: BTreeMap<String, i64> = BTreeMap::new();
    for path in paths {
        if let Some(parent) = Path::new(&path).parent() {
            *map.entry(parent.to_string_lossy().to_string()).or_insert(0) += 1;
        }
    }
    Ok(map.into_iter().map(|(key, count)| LibraryFacet { key, count }).collect())
}

/// 依欄位取出歌曲。folder 用路徑前綴，其餘用等值。
pub fn songs_by_field(
    state: &DbState,
    field: &str,
    value: &str,
    limit: i64,
    offset: i64,
    order_by: &str,
    order_dir: &str,
) -> Result<Vec<Song>, String> {
    let conn = db_lock(state);
    let order = if field == "album" {
        "COALESCE(disc_number, 9999) ASC, COALESCE(track_number, 9999) ASC, title COLLATE NOCASE ASC".to_string()
    } else {
        order_sql(order_by, order_dir)
    };
    let (sql, param) = if field == "folder" {
        let prefix = if value.ends_with('\\') || value.ends_with('/') { value.to_string() } else { format!("{value}\\") };
        (format!("SELECT {SONG_COLS} FROM songs WHERE substr(path, 1, length(?1)) = ?1 ORDER BY {order} LIMIT ?2 OFFSET ?3"), prefix)
    } else {
        let col = facet_column(field)?;
        let stored = if value == "未知" { String::new() } else { value.to_string() };
        (format!("SELECT {SONG_COLS} FROM songs WHERE COALESCE({col}, '') = ?1 ORDER BY {order} LIMIT ?2 OFFSET ?3"), stored)
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![param, limit, offset], |row| song_from_row(row))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 最近播放，同一首只留最後一次。
pub fn list_recent(state: &DbState, limit: i64) -> Result<Vec<Song>, String> {
    let conn = db_lock(state);
    let sql = format!(
        "SELECT {SONG_COLS} FROM songs s
         JOIN (SELECT song_id, MAX(played_at) AS played FROM play_history GROUP BY song_id) h
           ON h.song_id = s.id
         ORDER BY h.played DESC LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt.query_map([limit], |row| song_from_row(row)).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 讀取檔案上的標籤，不改資料庫。
pub fn read_tags(state: &DbState, song_id: i64) -> Result<TagEdit, String> {
    let conn = db_lock(state);
    let song = get_song(&conn, song_id).map_err(|e| e.to_string())?.ok_or("找不到歌曲")?;
    Ok(TagEdit {
        song_id,
        title: song.title.unwrap_or_default(),
        artist: song.artist.unwrap_or_default(),
        album: song.album.unwrap_or_default(),
        genre: song.genre.unwrap_or_default(),
        year: song.year,
        track_number: positive(song.track_number),
        disc_number: positive(song.disc_number),
    })
}

/// 寫入標籤到檔案，並更新資料庫（搜尋索引由觸發器跟著更新）。失敗就不假裝已存。
pub fn write_tags(state: &DbState, edit: &TagEdit) -> Result<(), String> {
    let conn = db_lock(state);
    let song = get_song(&conn, edit.song_id).map_err(|e| e.to_string())?.ok_or("找不到歌曲")?;
    let path = song.path.clone();
    let mut tagged = lofty::read_from_path(&path).map_err(|e| format!("讀取標籤失敗: {e}"))?;
    let tag = tagged.primary_tag_mut().ok_or("這個檔案沒有可寫入的標籤")?;
    tag.insert_text(ItemKey::TrackTitle, edit.title.clone());
    tag.insert_text(ItemKey::TrackArtist, edit.artist.clone());
    tag.insert_text(ItemKey::AlbumTitle, edit.album.clone());
    tag.insert_text(ItemKey::Genre, edit.genre.clone());
    if let Some(year) = edit.year {
        tag.insert_text(ItemKey::Year, year.to_string());
    }
    if let Some(n) = positive(edit.track_number) {
        tag.insert_text(ItemKey::TrackNumber, n.to_string());
    }
    if let Some(n) = positive(edit.disc_number) {
        tag.insert_text(ItemKey::DiscNumber, n.to_string());
    }
    tagged.save_to_path(&path).map_err(|e| format!("寫入標籤失敗: {e}"))?;

    let (title_py, title_init) = compute_pinyin(&edit.title);
    let (artist_py, artist_init) = compute_pinyin(&edit.artist);
    conn.execute(
        "UPDATE songs SET title=?1, artist=?2, album=?3, genre=?4, year=?5, \
         title_pinyin=?6, artist_pinyin=?7, title_pinyin_initial=?8, artist_pinyin_initial=?9, \
         track_number=?11, disc_number=?12 \
         WHERE id=?10",
        rusqlite::params![
            edit.title, edit.artist, edit.album, edit.genre, edit.year,
            title_py, artist_py, title_init, artist_init, edit.song_id,
            positive(edit.track_number), positive(edit.disc_number)
        ],
    ).map_err(|e| e.to_string())?;
    // 搜尋索引由 songs 上的觸發器同步（db.rs ensure_search_index），這裡不另外寫。
    Ok(())
}

/// 用本機 ffmpeg 轉檔。沒有 ffmpeg 或沒有輸出檔就回錯誤，不回假路徑。
pub fn convert_audio(state: &DbState, req: &ConvertRequest) -> Result<String, String> {
    let ffmpeg_path = locate("ffmpeg.exe").map(|(p, _, _)| p).ok_or("這台電腦找不到 ffmpeg，無法轉檔")?;
    let ext = match req.format.as_str() {
        "mp3" | "flac" | "wav" | "ogg" => req.format.as_str(),
        _ => return Err("只支援 mp3、flac、wav、ogg".into()),
    };
    let conn = db_lock(state);
    let song = get_song(&conn, req.song_id).map_err(|e| e.to_string())?.ok_or("找不到歌曲")?;
    let dir = PathBuf::from(req.output_dir.trim());
    if req.output_dir.trim().is_empty() || !dir.is_dir() {
        return Err("輸出資料夾不存在。請填一個已經存在的路徑".into());
    }
    let stem = Path::new(&song.path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("track");
    let safe: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c > '\u{007f}' { c } else { '_' }).collect();
    let output = dir.join(format!("{safe}.{ext}"));
    let kbps = req.bitrate_kbps.unwrap_or(320).clamp(64, 320);
    let rate = format!("{kbps}k");
    let mut cmd = Command::new(&ffmpeg_path);
    cmd.arg("-y").arg("-i").arg(&song.path).arg("-vn");
    // SE11：本機轉檔只允許 file/pipe 協議
    cmd.args(["-protocol_whitelist", "file,pipe"]);
    match ext {
        "mp3" => { cmd.args(["-c:a", "libmp3lame", "-b:a", &rate]); }
        "flac" => { cmd.args(["-c:a", "flac"]); }
        "wav" => { cmd.args(["-c:a", "pcm_s16le"]); }
        "ogg" => { cmd.args(["-c:a", "libvorbis", "-q:a", "6"]); }
        _ => {}
    }
    cmd.arg(&output);
    
    // SE3-1: 使用獨立 stderr 讀取執行緒與持久化 Sanitizer，避免 ffmpeg 因 stderr 緩衝區滿而阻塞
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("無法執行 ffmpeg: {e}"))?;

    let stderr = child.stderr.take().ok_or("ffmpeg 無 stderr")?;
    let stderr_thread = crate::open_with::spawn_stderr_sanitizer(Some(stderr), None);

    let status = child.wait().map_err(|e| format!("ffmpeg 等待失敗: {e}"))?;
    let _ = stderr_thread.join();
    
    if !status.success() {
        // stderr 已在執行緒中清洗並記錄，這裡只回傳通用錯誤
        return Err("轉檔失敗（詳細錯誤請見 open-with.log）".into());
    }
    if !output.exists() {
        return Err("ffmpeg 結束了，但沒有產生檔案".into());
    }
    Ok(output.to_string_lossy().to_string())
}

/// 依路徑找已掃描的歌曲。找不到就由前端用檔名直接播。
pub fn song_by_path(state: &DbState, path: &str) -> Result<Option<Song>, String> {
    let conn = db_lock(state);
    let alt = if path.contains('/') { path.replace('/', "\\") } else { path.replace('\\', "/") };
    let sql = format!("SELECT {SONG_COLS} FROM songs WHERE path = ?1 OR path = ?2 LIMIT 1");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(rusqlite::params![path, alt], |row| song_from_row(row))
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(row) => Ok(Some(row.map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}
pub fn reveal_in_explorer(path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err("檔案不在這個路徑".into());
    }
    Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn()
        .map_err(|e| format!("無法開啟檔案總管: {e}"))?;
    Ok(())
}

fn positive(n: Option<i32>) -> Option<i32> {
    n.filter(|v| *v > 0)
}

pub fn classify_drop(paths: &[String]) -> crate::models::DropPaths {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for path in paths {
        let p = Path::new(path);
        // SE1：拖放入口必須先驗證本機路徑
        if validate_local_path(p).is_err() {
            continue;
        }
        if p.is_dir() {
            dirs.push(path.clone());
        } else if p.is_file() {
            files.push(path.clone());
        }
    }
    crate::models::DropPaths { files, dirs }
}

pub fn list_radio(state: &DbState) -> Result<Vec<crate::models::RadioStation>, String> {
    let conn = db_lock(state);
    let mut stmt = conn.prepare("SELECT id, name, url, created_at FROM radio_stations ORDER BY id ASC").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |row| {
        Ok(crate::models::RadioStation {
            id: row.get(0)?,
            name: row.get(1)?,
            url: row.get(2)?,
            created_at: row.get(3)?,
        })
    }).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn save_radio(state: &DbState, name: &str, url: &str) -> Result<i64, String> {
    let name = name.trim();
    let url = url.trim();
    if name.is_empty() {
        return Err("電台名稱不能是空的".into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("只接受 http 或 https 串流網址".into());
    }
    let conn = db_lock(state);
    conn.execute(
        "INSERT INTO radio_stations (name, url, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![name, url, chrono::Utc::now().to_rfc3339()],
    ).map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_radio(state: &DbState, id: i64, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("電台名稱不能是空的".into());
    }
    let conn = db_lock(state);
    let n = conn.execute("UPDATE radio_stations SET name = ?1 WHERE id = ?2", rusqlite::params![name, id]).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("找不到這個電台".into());
    }
    Ok(())
}

pub fn delete_radio(state: &DbState, id: i64) -> Result<(), String> {
    let conn = db_lock(state);
    conn.execute("DELETE FROM radio_stations WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn write_tags_batch(state: &DbState, batch: &crate::models::TagBatch) -> Result<crate::models::TagBatchResult, String> {
    if batch.song_ids.is_empty() {
        return Err("沒有選到歌".into());
    }
    if batch.artist.is_none() && batch.album.is_none() && batch.genre.is_none() && batch.year.is_none() {
        return Err("沒有要改的欄位".into());
    }
    let mut updated = 0i64;
    let mut failed = Vec::new();
    for id in &batch.song_ids {
        let result = read_tags(state, *id).and_then(|mut edit| {
            if let Some(v) = &batch.artist { edit.artist = v.clone(); }
            if let Some(v) = &batch.album { edit.album = v.clone(); }
            if let Some(v) = &batch.genre { edit.genre = v.clone(); }
            if batch.year.is_some() { edit.year = batch.year; }
            let label = if edit.title.is_empty() { format!("歌曲 {id}") } else { edit.title.clone() };
            write_tags(state, &edit).map_err(|e| format!("{label}: {e}"))
        });
        match result {
            Ok(()) => updated += 1,
            Err(e) => failed.push(e),
        }
    }
    Ok(crate::models::TagBatchResult { updated, failed })
}


#[cfg(test)]
mod tests {
    use super::*;
    use lofty::Tag;

    /// 產生 0.1 秒的靜音 WAV，並先放一個空標籤（write_tags 只改既有的主要標籤）。
    fn tagged_wav(dir: &std::path::Path) -> String {
        let path = dir.join("t.wav");
        let (rate, ch, n) = (8000u32, 1u16, 800u32);
        let data_len = n * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&ch.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b.extend(std::iter::repeat(0u8).take(data_len as usize));
        std::fs::write(&path, b).unwrap();
        let mut f = lofty::read_from_path(&path).unwrap();
        let kind = f.primary_tag_type();
        let mut tag = Tag::new(kind);
        tag.insert_text(ItemKey::Comment, "fixture".into()); // 空標籤不會被存檔
        f.insert_tag(tag);
        f.save_to_path(&path).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn write_tags_succeeds_and_search_follows() {
        let dir = tempfile::tempdir().unwrap();
        let wav = tagged_wav(dir.path());
        let conn = crate::db::open_connection(&dir.path().join("t.db")).unwrap();
        crate::db::init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO songs (path, title, artist, duration_ms, file_mtime) VALUES (?1, '舊歌名', '舊歌手', 100, 1)",
            [&wav],
        ).unwrap();
        let id = conn.last_insert_rowid();
        let state = DbState { conn: std::sync::Mutex::new(conn) };
        let edit = TagEdit {
            song_id: id, title: "新歌名測試".into(), artist: "新歌手".into(), album: "專輯".into(),
            genre: "Rock".into(), year: Some(2020), track_number: Some(3), disc_number: None,
        };
        write_tags(&state, &edit).expect("tag write must report success after the file and row are updated");
        let conn = state.conn.lock().unwrap();
        assert_eq!(crate::db::search_songs(&conn, "新歌名測試", 10).unwrap().total, 1);
        assert_eq!(crate::db::search_songs(&conn, "舊歌名", 10).unwrap().total, 0);
        drop(conn);

        let batch = crate::models::TagBatch { song_ids: vec![id], artist: Some("批次歌手".into()), album: None, genre: None, year: None };
        let r = write_tags_batch(&state, &batch).unwrap();
        assert_eq!((r.updated, r.failed.len()), (1, 0), "batch must not list a written song as failed: {:?}", r.failed);
        let conn = state.conn.lock().unwrap();
        assert_eq!(crate::db::search_songs(&conn, "批次歌手", 10).unwrap().total, 1);
    }
}
