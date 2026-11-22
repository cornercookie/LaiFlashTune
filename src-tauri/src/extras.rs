use crate::db::{db_lock, get_song};
use crate::models::{ConvertRequest, DbState, LibraryFacet, Song, TagEdit};
use crate::scanner::compute_pinyin;
use lofty::ItemKey;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const SONG_COLS: &str = "id, path, title, artist, album, album_artist, genre, year, duration_ms, \
    file_mtime, file_size, cover_path, title_pinyin, artist_pinyin, title_pinyin_initial, \
    artist_pinyin_initial, added_at, play_count, last_played_at, track_number, disc_number, \
    format, bitrate, sample_rate, channels, missing";

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

/// 寫入標籤到檔案，並更新資料庫與 FTS。失敗就不假裝已存。
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
    conn.execute("DELETE FROM songs_fts_content WHERE id = ?1", [edit.song_id]).ok();
    conn.execute(
        "INSERT INTO songs_fts_content (id, title, artist, album, title_pinyin, artist_pinyin) VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![edit.song_id, edit.title, edit.artist, edit.album, title_py, artist_py],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// 用本機 ffmpeg 轉檔。沒有 ffmpeg 或沒有輸出檔就回錯誤，不回假路徑。
pub fn convert_audio(state: &DbState, req: &ConvertRequest) -> Result<String, String> {
    if !ffmpeg_available() {
        return Err("這台電腦找不到 ffmpeg，無法轉檔".into());
    }
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
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(&song.path).arg("-vn");
    match ext {
        "mp3" => { cmd.args(["-c:a", "libmp3lame", "-b:a", &rate]); }
        "flac" => { cmd.args(["-c:a", "flac"]); }
        "wav" => { cmd.args(["-c:a", "pcm_s16le"]); }
        "ogg" => { cmd.args(["-c:a", "libvorbis", "-q:a", "6"]); }
        _ => {}
    }
    cmd.arg(&output);
    let out = cmd.output().map_err(|e| format!("無法執行 ffmpeg: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.chars().rev().take(280).collect::<String>().chars().rev().collect();
        return Err(format!("轉檔失敗: {tail}"));
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
