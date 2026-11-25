use crate::models::{
    AppConfig, DbState, LyricLine, Lyrics, MusicFolder, NewMusicFolder, NewPlaylist,
    NewPlaylistItem, NewSong, PlayHistory, Playlist, PlaylistItem, PlaylistOrderUpdate,
    ScanProgress, SearchResult, Song,
};
use chrono::{Utc};
use rusqlite::params;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

/// 取得可攜式資料庫路徑：<執行檔所在目錄>/data/app.db
/// 隨身碟盤符變更 (D:/E:/F:) 不影響，因為相對於執行檔所在目錄。
pub fn db_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let data_dir = exe_dir.join("data");
    let _ = std::fs::create_dir_all(&data_dir);
    data_dir.join("app.db")
}

/// 封面縮圖目錄：<執行檔所在目錄>/data/covers/
pub fn covers_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let dir = exe_dir.join("data").join("covers");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 設定檔路徑：<執行檔所在目錄>/data/config.json
pub fn config_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    exe_dir.join("data").join("config.json")
}

/// 開啟單一 SQLite 連線（可攜式場景不使用 WAL，避免隨身碟拔除不正常卸載損壞 -wal/-shm）
pub fn open_connection(path: &Path) -> rusqlite::Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "DELETE")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// 初始化資料庫：建立表格、FTS5 索引與索引。第一次啟動時自動建立 data/ 與資料庫。
pub fn init_db(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS songs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT UNIQUE NOT NULL,
            title TEXT,
            artist TEXT,
            album TEXT,
            album_artist TEXT,
            genre TEXT,
            year INTEGER,
            duration_ms INTEGER NOT NULL,
            file_mtime INTEGER NOT NULL,
            file_size INTEGER,
            cover_path TEXT,
            title_pinyin TEXT,
            artist_pinyin TEXT,
            title_pinyin_initial TEXT,
            artist_pinyin_initial TEXT,
            added_at TEXT,
            play_count INTEGER DEFAULT 0,
            last_played_at TEXT,
            track_number INTEGER,
            disc_number INTEGER,
            format TEXT,
            bitrate INTEGER,
            sample_rate INTEGER,
            channels INTEGER,
            missing INTEGER DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS songs_fts (
            id INTEGER PRIMARY KEY AUTOINCREMENT
        );

        CREATE TABLE IF NOT EXISTS playlists (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            sort_order INTEGER DEFAULT 0,
            created_at TEXT,
            is_smart INTEGER DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS playlist_items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            playlist_id INTEGER NOT NULL,
            song_id INTEGER NOT NULL,
            position INTEGER DEFAULT 0,
            added_at TEXT,
            FOREIGN KEY (playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
            FOREIGN KEY (song_id) REFERENCES songs(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS lyrics (
            song_id INTEGER PRIMARY KEY,
            source TEXT,
            raw_lrc TEXT,
            offset_ms INTEGER DEFAULT 0,
            FOREIGN KEY (song_id) REFERENCES songs(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS play_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            song_id INTEGER NOT NULL,
            played_at TEXT,
            FOREIGN KEY (song_id) REFERENCES songs(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS music_folders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT UNIQUE NOT NULL,
            added_at TEXT
        );

        CREATE TABLE IF NOT EXISTS app_config (
            key TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE TABLE IF NOT EXISTS radio_stations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            url TEXT NOT NULL,
            created_at TEXT
        );

        CREATE TABLE IF NOT EXISTS smart_playlist_rules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            playlist_id INTEGER NOT NULL,
            field TEXT NOT NULL,
            operator TEXT NOT NULL,
            value TEXT NOT NULL,
            FOREIGN KEY (playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_songs_path ON songs(path);
        CREATE INDEX IF NOT EXISTS idx_songs_artist ON songs(artist);
        CREATE INDEX IF NOT EXISTS idx_songs_album ON songs(album);
        CREATE INDEX IF NOT EXISTS idx_songs_added_at ON songs(added_at);
        CREATE INDEX IF NOT EXISTS idx_playlist_items_playlist ON playlist_items(playlist_id);
        CREATE INDEX IF NOT EXISTS idx_playlist_items_position ON playlist_items(playlist_id, position);
        CREATE INDEX IF NOT EXISTS idx_play_history_song ON play_history(song_id);
    "#,
    )?;

    // 確保預設歌單存在（空表時才插入，避免重複）
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM playlists",
        [],
        |row| row.get(0),
    )?;
    if count == 0 {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO playlists (name, sort_order, created_at, is_smart) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["全部歌曲", 0, &now, 0],
        )?;
        conn.execute(
            "INSERT INTO playlists (name, sort_order, created_at, is_smart) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["最近播放", 1, &now, 0],
        )?;
        conn.execute(
            "INSERT INTO playlists (name, sort_order, created_at, is_smart) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["我的最愛", 2, &now, 0],
        )?;
    }

    ensure_song_columns(conn)?;
    ensure_search_index(conn)?;
    Ok(())
}

/// 搜尋索引：獨立的 FTS5（自己存一份文字），rowid = songs.id，由觸發器跟 songs 同步。
/// 舊版用 content='songs' 外部內容表，卻用不存在的 `id` 欄寫入與 JOIN，索引永遠是空的、查詢直接報錯。
/// 拼音另存一份去掉空白的版本，打「qili」也找得到「qi li xiang」。
const FTS_DDL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS songs_fts_content USING fts5(
    title, artist, album, title_pinyin, artist_pinyin, pinyin_compact, initials,
    tokenize='trigram'
)";

const FTS_ROW: &str = "coalesce(title,''), coalesce(artist,''), coalesce(album,''),
    coalesce(title_pinyin,''), coalesce(artist_pinyin,''),
    replace(coalesce(title_pinyin,'') || ' ' || coalesce(artist_pinyin,''), ' ', ''),
    coalesce(title_pinyin_initial,'') || ' ' || coalesce(artist_pinyin_initial,'')";

fn ensure_search_index(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let sql: Option<String> = conn
        .query_row("SELECT sql FROM sqlite_master WHERE name = 'songs_fts_content'", [], |r| r.get(0))
        .ok();
    let outdated = sql.as_deref().map(|s| s.contains("content='songs'") || !s.contains("initials")).unwrap_or(false);
    if outdated {
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS songs_fts_ai; DROP TRIGGER IF EXISTS songs_fts_au; DROP TRIGGER IF EXISTS songs_fts_ad;
             DROP TABLE IF EXISTS songs_fts_content;",
        )?;
    }
    conn.execute_batch(FTS_DDL)?;
    let row_new = FTS_ROW.replace("title", "new.title").replace("artist", "new.artist").replace("album", "new.album");
    conn.execute_batch(&format!(
        "CREATE TRIGGER IF NOT EXISTS songs_fts_ai AFTER INSERT ON songs BEGIN
             INSERT INTO songs_fts_content(rowid, title, artist, album, title_pinyin, artist_pinyin, pinyin_compact, initials)
             VALUES (new.id, {row_new});
         END;
         CREATE TRIGGER IF NOT EXISTS songs_fts_ad AFTER DELETE ON songs BEGIN
             DELETE FROM songs_fts_content WHERE rowid = old.id;
         END;
         CREATE TRIGGER IF NOT EXISTS songs_fts_au AFTER UPDATE OF title, artist, album, title_pinyin, artist_pinyin,
             title_pinyin_initial, artist_pinyin_initial ON songs BEGIN
             DELETE FROM songs_fts_content WHERE rowid = old.id;
             INSERT INTO songs_fts_content(rowid, title, artist, album, title_pinyin, artist_pinyin, pinyin_compact, initials)
             VALUES (new.id, {row_new});
         END;"
    ))?;
    // 索引筆數對不上（舊版從沒寫進去過，或升級剛重建）就整個重灌一次。
    let songs: i64 = conn.query_row("SELECT COUNT(*) FROM songs", [], |r| r.get(0))?;
    let indexed: i64 = conn.query_row("SELECT COUNT(*) FROM songs_fts_content", [], |r| r.get(0))?;
    if songs != indexed {
        conn.execute_batch(&format!(
            "DELETE FROM songs_fts_content;
             INSERT INTO songs_fts_content(rowid, title, artist, album, title_pinyin, artist_pinyin, pinyin_compact, initials)
             SELECT id, {FTS_ROW} FROM songs;"
        ))?;
    }
    Ok(())
}

/// 既有 data/app.db 不能重建。缺欄才 ALTER，每欄只加一次。
fn ensure_song_columns(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(songs)")?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .collect();
    for (name, decl) in [
        ("track_number", "INTEGER"),
        ("disc_number", "INTEGER"),
        ("format", "TEXT"),
        ("bitrate", "INTEGER"),
        ("sample_rate", "INTEGER"),
        ("channels", "INTEGER"),
        ("missing", "INTEGER DEFAULT 0"),
        ("replaygain_track_gain", "REAL"),
        ("replaygain_track_peak", "REAL"),
        ("replaygain_album_gain", "REAL"),
        ("replaygain_album_peak", "REAL"),
    ] {
        if !names.iter().any(|n| n == name) {
            conn.execute(&format!("ALTER TABLE songs ADD COLUMN {name} {decl}"), [])?;
        }
    }
    Ok(())
}

/// 將 Song 資料寫入資料庫（掃描使用）。若 path 已存在則更新。
pub fn upsert_song(conn: &rusqlite::Connection, song: &NewSong) -> rusqlite::Result<i64> {
    let now = Utc::now().to_rfc3339();
    // 嘗試更新，若無列受影響則插入
    let changed = conn.execute(
        r#"
        UPDATE songs SET
            title = COALESCE(?2, title),
            artist = COALESCE(?3, artist),
            album = COALESCE(?4, album),
            album_artist = COALESCE(?5, album_artist),
            genre = COALESCE(?6, genre),
            year = COALESCE(?7, year),
            duration_ms = ?8,
            file_mtime = ?9,
            file_size = ?10,
            cover_path = COALESCE(?11, cover_path),
            title_pinyin = ?12,
            artist_pinyin = ?13,
            title_pinyin_initial = ?14,
            artist_pinyin_initial = ?15,
            track_number = ?16,
            disc_number = ?17,
            format = ?18,
            bitrate = ?19,
            sample_rate = ?20,
            channels = ?21,
            replaygain_track_gain = ?22,
            replaygain_track_peak = ?23,
            replaygain_album_gain = ?24,
            replaygain_album_peak = ?25,
            missing = 0
        WHERE path = ?1
        "#,
        rusqlite::params![
            song.path,
            song.title,
            song.artist,
            song.album,
            song.album_artist,
            song.genre,
            song.year,
            song.duration_ms,
            song.file_mtime,
            song.file_size,
            song.cover_path,
            song.title_pinyin,
            song.artist_pinyin,
            song.title_pinyin_initial,
            song.artist_pinyin_initial,
            song.track_number,
            song.disc_number,
            song.format,
            song.bitrate,
            song.sample_rate,
            song.channels,
            song.replaygain_track_gain,
            song.replaygain_track_peak,
            song.replaygain_album_gain,
            song.replaygain_album_peak,
        ],
    )?;
    if changed > 0 {
        let id: i64 = conn.query_row("SELECT id FROM songs WHERE path = ?1", [&song.path], |r| r.get(0))?;
        return Ok(id);
    }

    conn.execute(
        r#"
        INSERT INTO songs
            (path, title, artist, album, album_artist, genre, year, duration_ms, file_mtime, file_size,
             cover_path, title_pinyin, artist_pinyin, title_pinyin_initial, artist_pinyin_initial, added_at,
             track_number, disc_number, format, bitrate, sample_rate, channels,
             replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak, missing)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,0)
        "#,
        rusqlite::params![
            song.path,
            song.title,
            song.artist,
            song.album,
            song.album_artist,
            song.genre,
            song.year,
            song.duration_ms,
            song.file_mtime,
            song.file_size,
            song.cover_path,
            song.title_pinyin,
            song.artist_pinyin,
            song.title_pinyin_initial,
            song.artist_pinyin_initial,
            now,
            song.track_number,
            song.disc_number,
            song.format,
            song.bitrate,
            song.sample_rate,
            song.channels,
            song.replaygain_track_gain,
            song.replaygain_track_peak,
            song.replaygain_album_gain,
            song.replaygain_album_peak,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 從資料庫刪除歌曲（連同 FTS 與歌詞）
pub fn delete_song(conn: &rusqlite::Connection, path: &str) -> rusqlite::Result<()> {
    let id: Option<i64> = conn.query_row("SELECT id FROM songs WHERE path = ?1", [path], |r| r.get(0)).ok();
    if let Some(id) = id {
        let _ = conn.execute("DELETE FROM lyrics WHERE song_id = ?1", [id]);
        let _ = conn.execute("DELETE FROM play_history WHERE song_id = ?1", [id]);
        let _ = conn.execute("DELETE FROM playlist_items WHERE song_id = ?1", [id]);
    }
    conn.execute("DELETE FROM songs WHERE path = ?1", [path])?;
    Ok(())
}

/// 根據 file_mtime + file_size 做增量掃描：找出需要重新讀取（變更/新增）與已遺失的歌曲。
/// 回傳 (need_rescan: Vec<path>, missing: Vec<path>)
pub fn diff_songs(
    conn: &rusqlite::Connection,
    current: &[(String, i64, Option<i64>)], // (path, mtime, size)
) -> rusqlite::Result<(Vec<String>, Vec<String>)> {
    let mut stmt = conn.prepare("SELECT path, file_mtime, file_size FROM songs")?;
    let rows: Vec<(String, i64, Option<i64>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .filter_map(Result::ok)
        .collect();

    let current_map: std::collections::HashMap<String, (i64, Option<i64>)> =
        current.iter().cloned().map(|(p, m, s)| (p, (m, s))).collect();

    let mut need_rescan = Vec::new();
    let mut missing = Vec::new();

    for (path, mtime, size) in rows {
        match current_map.get(&path) {
            Some((cm, cs)) if *cm == mtime && *cs == size => {} // 未變更，跳過
            Some(_) => need_rescan.push(path.clone()), // 變更
            None => missing.push(path.clone()), // 檔案不見
        }
    }
    // 新增的檔案在 current_map 但不在 DB 中，由掃描端偵測（這裡只回傳 DB 端差異）
    Ok((need_rescan, missing))
}

/// 取得歌曲列表（分頁、排序）
pub fn list_songs(
    conn: &rusqlite::Connection,
    limit: i64,
    offset: i64,
    order_by: &str,
    order_dir: &str,
) -> rusqlite::Result<Vec<Song>> {
    let order = if order_dir.eq_ignore_ascii_case("desc") { "DESC" } else { "ASC" };
    let col = match order_by {
        "title" | "artist" | "album" | "genre" | "year" | "duration_ms" | "play_count" | "path" | "file_mtime" | "added_at" | "last_played_at" => order_by,
        _ => "title",
    };
    let sql = format!(
        "SELECT id, path, title, artist, album, album_artist, genre, year, duration_ms,
         file_mtime, file_size, cover_path, title_pinyin, artist_pinyin,
         title_pinyin_initial, artist_pinyin_initial, added_at, play_count, last_played_at,
         track_number, disc_number, format, bitrate, sample_rate, channels, missing,
         replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak
         FROM songs ORDER BY {} {} LIMIT ?1 OFFSET ?2",
        col, order
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![limit, offset], |row| Ok(song_from_row(row)?))?;
    Ok(rows.filter_map(Result::ok).collect::<Vec<_>>())
}

/// 透過 FTS5 全文搜尋（10 萬首下應在 50ms 內回傳，限 50 筆）。
/// 支援中文、拼音、拼音首字母、片段搜尋（關鍵字* 前綴比對）。
pub fn search_songs(
    conn: &rusqlite::Connection,
    query: &str,
    limit: i64,
) -> rusqlite::Result<SearchResult> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM songs", [], |r| r.get(0))?;
        let songs = list_songs(conn, limit, 0, "added_at", "desc")?;
        return Ok(SearchResult { songs, total });
    }
    // trigram 斷詞：3 字以上走 MATCH（任意位置子字串、不分大小寫）；
    // 1–2 字（「七里」「周」）trigram 索引用不上，改用 LIKE 掃索引表，結果一樣、只是慢一點。
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let long: Vec<String> = terms
        .iter()
        .filter(|t| t.chars().count() >= 3)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if !long.is_empty() {
        params.push(long.join(" "));
        clauses.push(format!("songs_fts_content MATCH ?{}", params.len()));
    }
    for t in terms.iter().filter(|t| t.chars().count() < 3) {
        let esc = t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        params.push(format!("%{esc}%"));
        let n = params.len();
        clauses.push(format!(
            "(f.title LIKE ?{n} ESCAPE '\\' OR f.artist LIKE ?{n} ESCAPE '\\' OR f.album LIKE ?{n} ESCAPE '\\' \
             OR f.pinyin_compact LIKE ?{n} ESCAPE '\\' OR f.initials LIKE ?{n} ESCAPE '\\')"
        ));
    }
    let where_sql = clauses.join(" AND ");
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM songs_fts_content f WHERE {where_sql}"),
        rusqlite::params_from_iter(params.iter()),
        |r| r.get(0),
    )?;
    params.push(limit.to_string());
    let sql = format!(
        "SELECT s.id, s.path, s.title, s.artist, s.album, s.album_artist, s.genre, s.year,
         s.duration_ms, s.file_mtime, s.file_size, s.cover_path, s.title_pinyin, s.artist_pinyin,
         s.title_pinyin_initial, s.artist_pinyin_initial, s.added_at, s.play_count, s.last_played_at,
         s.track_number, s.disc_number, s.format, s.bitrate, s.sample_rate, s.channels, s.missing,
         s.replaygain_track_gain, s.replaygain_track_peak, s.replaygain_album_gain, s.replaygain_album_peak
         FROM songs_fts_content f
         JOIN songs s ON s.id = f.rowid
         WHERE {where_sql}
         ORDER BY s.play_count DESC, s.added_at DESC
         LIMIT CAST(?{} AS INTEGER)",
        params.len()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| song_from_row(row))?;
    let songs = rows.filter_map(Result::ok).collect();
    Ok(SearchResult { songs, total })
}

/// 取得單首歌曲
pub fn get_song(conn: &rusqlite::Connection, id: i64) -> rusqlite::Result<Option<Song>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, title, artist, album, album_artist, genre, year, duration_ms,
         file_mtime, file_size, cover_path, title_pinyin, artist_pinyin,
         title_pinyin_initial, artist_pinyin_initial, added_at, play_count, last_played_at,
         track_number, disc_number, format, bitrate, sample_rate, channels, missing,
         replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak
         FROM songs WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![id], |row| song_from_row(row))?;
    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// 增加播放次數並更新最後播放時間
pub fn increment_play_count(conn: &rusqlite::Connection, id: i64) -> rusqlite::Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE songs SET play_count = play_count + 1, last_played_at = ?1 WHERE id = ?2",
        rusqlite::params![now, id],
    )?;
    conn.execute(
        "INSERT INTO play_history (song_id, played_at) VALUES (?1, ?2)",
        rusqlite::params![id, now],
    )?;
    Ok(())
}

/// 記錄播放歷史（不增加 play_count，供最近播放歌單使用）
pub fn record_play_history(conn: &rusqlite::Connection, id: i64) -> rusqlite::Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO play_history (song_id, played_at) VALUES (?1, ?2)",
        rusqlite::params![id, now],
    )?;
    Ok(())
}

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

// ---------------- 歌單 ----------------

pub fn create_playlist(conn: &rusqlite::Connection, playlist: &NewPlaylist) -> rusqlite::Result<i64> {
    let now = Utc::now().to_rfc3339();
    let is_smart = playlist.is_smart.unwrap_or(0);
    conn.execute(
        "INSERT INTO playlists (name, sort_order, created_at, is_smart) VALUES (?1,?2,?3,?4)",
        rusqlite::params![playlist.name, playlist.sort_order, now, is_smart],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_playlists(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, sort_order, created_at, is_smart FROM playlists ORDER BY sort_order ASC, id ASC"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Playlist {
            id: row.get(0)?,
            name: row.get(1)?,
            sort_order: row.get(2)?,
            created_at: row.get(3)?,
            is_smart: row.get(4)?,
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub fn update_playlist(conn: &rusqlite::Connection, id: i64, name: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE playlists SET name = ?1 WHERE id = ?2", rusqlite::params![name, id])?;
    Ok(())
}

pub fn delete_playlist(conn: &rusqlite::Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
    Ok(())
}

pub fn add_smart_playlist_rule(
    conn: &rusqlite::Connection,
    playlist_id: i64,
    field: &str,
    operator: &str,
    value: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO smart_playlist_rules (playlist_id, field, operator, value) VALUES (?1,?2,?3,?4)",
        rusqlite::params![playlist_id, field, operator, value],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_smart_playlist_rules(
    conn: &rusqlite::Connection,
    playlist_id: i64,
) -> rusqlite::Result<Vec<crate::models::SmartPlaylistRule>> {
    let mut stmt = conn.prepare(
        "SELECT field, operator, value FROM smart_playlist_rules WHERE playlist_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![playlist_id], |row| {
        Ok(crate::models::SmartPlaylistRule {
            field: row.get(0)?,
            operator: row.get(1)?,
            value: row.get(2)?,
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub fn delete_smart_playlist_rules(
    conn: &rusqlite::Connection,
    playlist_id: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM smart_playlist_rules WHERE playlist_id = ?1",
        [playlist_id],
    )?;
    Ok(())
}

/// 根據智慧歌單規則查詢符合條件的歌曲。field 白名單防止 SQL 注入。
pub fn query_smart_playlist_songs(
    conn: &rusqlite::Connection,
    playlist_id: i64,
    limit: i64,
    offset: i64,
) -> rusqlite::Result<Vec<crate::models::Song>> {
    let rules = list_smart_playlist_rules(conn, playlist_id)?;
    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    for rule in &rules {
        let field = match rule.field.as_str() {
            "play_count" | "added_at" | "year" | "duration_ms" | "bitrate" | "sample_rate" | "channels" => rule.field.as_str(),
            "title" | "artist" | "album" | "genre" => rule.field.as_str(),
            _ => continue,
        };
        let op = match rule.operator.as_str() {
            ">" | "<" | "=" | "like" | "!=" => rule.operator.as_str(),
            _ => continue,
        };
        let (sql_frag, param) = if op == "like" {
            (format!("{} LIKE ?{}", field, params.len() + 1), format!("%{}%", rule.value))
        } else {
            (format!("{} {} ?{}", field, op, params.len() + 1), rule.value.clone())
        };
        where_clauses.push(sql_frag);
        params.push(param);
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let order = if rules.iter().any(|rule| rule.field == "play_count") {
        "play_count DESC, title ASC"
    } else {
        "added_at DESC, title ASC"
    };
    let limit_idx = params.len() + 1;
    let offset_idx = params.len() + 2;
    let sql = format!(
        "SELECT id, path, title, artist, album, album_artist, genre, year, duration_ms,
         file_mtime, file_size, cover_path, title_pinyin, artist_pinyin,
         title_pinyin_initial, artist_pinyin_initial, added_at, play_count, last_played_at,
         track_number, disc_number, format, bitrate, sample_rate, channels, missing,
         replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak
         FROM songs {where_sql} ORDER BY {order} LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );
    let mut bound: Vec<rusqlite::types::Value> = params.into_iter().map(rusqlite::types::Value::Text).collect();
    bound.push(rusqlite::types::Value::Integer(limit));
    bound.push(rusqlite::types::Value::Integer(offset));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bound.iter()), song_from_row)?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub fn add_song_to_playlist(
    conn: &rusqlite::Connection,
    item: &NewPlaylistItem,
) -> rusqlite::Result<i64> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO playlist_items (playlist_id, song_id, position, added_at) VALUES (?1,?2,?3,?4)",
        rusqlite::params![item.playlist_id, item.song_id, item.position, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_playlist_items(
    conn: &rusqlite::Connection,
    playlist_id: i64,
) -> rusqlite::Result<Vec<PlaylistItem>> {
    let mut stmt = conn.prepare(
        "SELECT pi.id, pi.playlist_id, pi.song_id, pi.position, pi.added_at
         FROM playlist_items pi WHERE pi.playlist_id = ?1 ORDER BY pi.position ASC, pi.id ASC"
    )?;
    let rows = stmt.query_map(rusqlite::params![playlist_id], |row| {
        let id: i64 = row.get(0)?;
        let song = get_song(conn, row.get(2)?)?;
        Ok(PlaylistItem {
            id,
            playlist_id: row.get(1)?,
            song_id: row.get(2)?,
            position: row.get(3)?,
            added_at: row.get(4)?,
            song,
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub fn remove_from_playlist(conn: &rusqlite::Connection, item_id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM playlist_items WHERE id = ?1", [item_id])?;
    Ok(())
}

pub fn update_playlist_order(
    conn: &mut rusqlite::Connection,
    update: &PlaylistOrderUpdate,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for (idx, item_id) in update.item_ids.iter().enumerate() {
        tx.execute(
            "UPDATE playlist_items SET position = ?1 WHERE id = ?2 AND playlist_id = ?3",
            rusqlite::params![idx as i32, item_id, update.playlist_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ---------------- 歌詞 ----------------

pub fn save_lyrics(conn: &rusqlite::Connection, song_id: i64, lyrics: &Lyrics) -> rusqlite::Result<()> {
    let raw = serde_json::to_string(&lyrics.lines).unwrap_or_default();
    let _ = conn.execute(
        "DELETE FROM lyrics WHERE song_id = ?1",
        [song_id],
    );
    conn.execute(
        "INSERT INTO lyrics (song_id, source, raw_lrc, offset_ms) VALUES (?1,?2,?3,?4)",
        rusqlite::params![song_id, lyrics.source.as_deref().unwrap_or("none"), raw, lyrics.offset_ms],
    )?;
    Ok(())
}

pub fn get_lyrics(conn: &rusqlite::Connection, song_id: i64) -> rusqlite::Result<Option<Lyrics>> {
    let mut stmt = conn.prepare(
        "SELECT source, raw_lrc, offset_ms FROM lyrics WHERE song_id = ?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![song_id], |row| {
        let source: String = row.get(0)?;
        let raw_lrc: String = row.get(1)?;
        let offset_ms: i32 = row.get(2)?;
        let lines: Vec<LyricLine> = serde_json::from_str(&raw_lrc).unwrap_or_default();
        Ok(Lyrics {
            song_id,
            source: if source.is_empty() { None } else { Some(source) },
            lines,
            offset_ms,
        })
    })?;
    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

pub fn set_lyrics_offset(conn: &rusqlite::Connection, song_id: i64, offset_ms: i32) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE lyrics SET offset_ms = ?1 WHERE song_id = ?2",
        rusqlite::params![offset_ms, song_id],
    )
}

// ---------------- 音樂資料夾 ----------------

pub fn add_music_folder(
    conn: &rusqlite::Connection,
    folder: &NewMusicFolder,
) -> rusqlite::Result<i64> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO music_folders (path, added_at) VALUES (?1, ?2)",
        rusqlite::params![folder.path, now],
    )?;
    let id: i64 = conn.query_row("SELECT id FROM music_folders WHERE path = ?1", [&folder.path], |r| r.get(0))?;
    Ok(id)
}

pub fn list_music_folders(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<MusicFolder>> {
    let mut stmt = conn.prepare("SELECT id, path, added_at FROM music_folders ORDER BY id ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(MusicFolder {
            id: row.get(0)?,
            path: row.get(1)?,
            added_at: row.get(2)?,
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub fn remove_music_folder(conn: &rusqlite::Connection, path: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM music_folders WHERE path = ?1", [path])?;
    Ok(())
}

// ---------------- 設定 ----------------

pub fn load_config(conn: &rusqlite::Connection) -> rusqlite::Result<AppConfig> {
    let mut config = AppConfig::default();
    // 音樂資料夾
    let folders = list_music_folders(conn)?;
    config.music_folders = folders.into_iter().map(|f| f.path).collect();

    let get = |key: &str| -> Option<String> {
        conn.query_row("SELECT value FROM app_config WHERE key = ?1", [key], |r| r.get::<_, String>(0)).ok()
    };
    if let Some(theme) = get("theme") { config.theme = theme; }
    if let Some(offset) = get("lyrics_offset_ms") {
        config.lyrics_offset_ms = offset.parse().unwrap_or(0);
    }
    if let Some(v) = get("replaygain_mode") {
        config.replaygain_mode = match v.as_str() {
            "off" => crate::models::ReplayGainMode::Off,
            "track" => crate::models::ReplayGainMode::Track,
            "album" => crate::models::ReplayGainMode::Album,
            "auto" => crate::models::ReplayGainMode::Auto,
            _ => crate::models::ReplayGainMode::default(),
        };
    }
    if let Some(v) = get("volume_normalization") {
        config.volume_normalization = match v.as_str() {
            "replaygain" => crate::models::VolumeNormalization::Replaygain,
            "peak" => crate::models::VolumeNormalization::Peak,
            "off" => crate::models::VolumeNormalization::Off,
            _ => crate::models::VolumeNormalization::default(),
        };
    }
    if let Some(v) = get("rg_preamp_has") { config.rg_preamp_has = v.parse().unwrap_or(0.0); }
    if let Some(v) = get("rg_preamp_none") { config.rg_preamp_none = v.parse().unwrap_or(-6.0); }
    Ok(config)
}

pub fn get_replaygain(conn: &rusqlite::Connection, song_id: i64) -> rusqlite::Result<(Option<f32>, Option<f32>, Option<f32>, Option<f32>)> {
    if song_id == 0 {
        return Ok((None, None, None, None));
    }
    match conn.query_row(
        "SELECT replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak FROM songs WHERE id = ?1",
        [song_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ) {
        Ok(v) => Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok((None, None, None, None)),
        Err(e) => Err(e),
    }
}

pub fn get_config_value(conn: &rusqlite::Connection, key: &str) -> rusqlite::Result<Option<String>> {
    match conn.query_row(
        "SELECT value FROM app_config WHERE key = ?1",
        [key],
        |row| row.get::<_, String>(0),
    ) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn save_config_kv(conn: &rusqlite::Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

/// 取得連線的 Mutex 保護（內部輔助，公開函式透過 DbState 呼叫）
pub fn db_lock(state: &DbState) -> MutexGuard<'_, rusqlite::Connection> {
    state.conn.lock().expect("資料庫 mutex 中毒")
}

/// 掃描進度：標記歌曲為遺失（可選，這裡僅提供介面，實際由 commands 呼叫 delete_song）
pub fn mark_missing_songs(conn: &rusqlite::Connection, paths: &[String]) -> rusqlite::Result<usize> {
    let mut count = 0;
    for path in paths {
        count += conn.execute("UPDATE songs SET missing = 1 WHERE path = ?1", [path])?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_init_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = open_connection(&path).unwrap();
        init_db(&conn).unwrap();

        let song = NewSong {
            path: "/Music/周杰倫 - 七里香.mp3".to_string(),
            title: Some("七里香".to_string()),
            artist: Some("周杰倫".to_string()),
            album: Some("七里香".to_string()),
            album_artist: None,
            genre: None,
            year: Some(2004),
            duration_ms: 250000,
            file_mtime: 1,
            file_size: Some(5_000_000),
            cover_path: None,
            title_pinyin: Some("qi li xiang".to_string()),
            artist_pinyin: Some("zhou jie lun".to_string()),
            title_pinyin_initial: Some("qlx".to_string()),
            artist_pinyin_initial: Some("zjl".to_string()),
            track_number: Some(7),
            disc_number: Some(1),
            format: Some("mp3".to_string()),
            bitrate: Some(320),
            sample_rate: Some(44100),
            channels: Some(2),
        };
        upsert_song(&conn, &song).unwrap();

        let result = search_songs(&conn, "zjl", 50).unwrap();
        assert_eq!(result.songs.len(), 1);
        assert_eq!(result.songs[0].title.as_deref(), Some("七里香"));

        let result = search_songs(&conn, "七里", 50).unwrap();
        assert_eq!(result.songs.len(), 1, "two-char Chinese falls back to LIKE");
        assert_eq!(search_songs(&conn, "周", 50).unwrap().songs.len(), 1);
        assert_eq!(search_songs(&conn, "qili", 50).unwrap().songs.len(), 1, "pinyin without spaces");
        assert_eq!(search_songs(&conn, "周杰倫 七里香", 50).unwrap().total, 1, "terms are AND");
        assert_eq!(search_songs(&conn, "周杰倫 晴天", 50).unwrap().total, 0);
        assert_eq!(search_songs(&conn, "50%_\"x", 50).unwrap().total, 0, "special chars do not break SQL");

        // 標籤改名後索引跟著換（觸發器），刪除後不再出現。
        conn.execute("UPDATE songs SET title = '晴天' WHERE path = ?1", [&song.path]).unwrap();
        assert_eq!(search_songs(&conn, "晴天", 50).unwrap().total, 1);
        assert_eq!(search_songs(&conn, "七里香 周杰倫", 50).unwrap().total, 1, "album still 七里香");
        delete_song(&conn, &song.path).unwrap();
        assert_eq!(search_songs(&conn, "晴天", 50).unwrap().total, 0);
    }

    #[test]
    fn old_external_content_index_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_connection(&dir.path().join("old.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE songs (id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT UNIQUE NOT NULL, title TEXT, artist TEXT,
               album TEXT, album_artist TEXT, genre TEXT, year INTEGER, duration_ms INTEGER NOT NULL, file_mtime INTEGER NOT NULL,
               file_size INTEGER, cover_path TEXT, title_pinyin TEXT, artist_pinyin TEXT, title_pinyin_initial TEXT,
               artist_pinyin_initial TEXT, added_at TEXT, play_count INTEGER DEFAULT 0, last_played_at TEXT);
             CREATE VIRTUAL TABLE songs_fts_content USING fts5(title, artist, album, title_pinyin, artist_pinyin,
               content='songs', content_rowid='id', tokenize='trigram');
             INSERT INTO songs (path, title, artist, duration_ms, file_mtime) VALUES ('/a.mp3', '稻香', '周杰倫', 1, 1);",
        ).unwrap();
        init_db(&conn).unwrap();
        assert_eq!(search_songs(&conn, "稻香", 50).unwrap().total, 1, "existing library becomes searchable after upgrade");
        init_db(&conn).unwrap();
        assert_eq!(search_songs(&conn, "周杰倫", 50).unwrap().total, 1, "second launch does not duplicate rows");
    }
}
