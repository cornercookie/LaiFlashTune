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
use std::io::Read;
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

    let results: Vec<ScanResult> = to_process
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
        match r {
            ScanResult::Song(song, mut errs) => {
                errors.append(&mut errs);
                new_songs.push(song);
            }
            ScanResult::Rejected(mut errs) => {
                errors.append(&mut errs);
            }
            ScanResult::Skipped(mut errs) => {
                errors.append(&mut errs);
            }
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
        .and_then(|s| s.parse::<i32>().ok());
    let channels = audio_stream["channels"].as_u64().map(|v| v as i32);
    let bitrate = audio_stream["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<i32>().ok())
        .or_else(|| {
            value["format"]["bit_rate"]
                .as_str()
                .and_then(|s| s.parse::<i32>().ok())
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
pub(crate) const SAMPLE_RATE_MIN: i32 = 8000;
pub(crate) const SAMPLE_RATE_MAX: i32 = 384000;
pub(crate) const CHANNELS_MIN: i32 = 1;
pub(crate) const CHANNELS_MAX: i32 = 8;

/// SE7：掃描單一檔案的結果契約
#[derive(Debug, Clone)]
pub enum ScanResult {
    /// 正常入庫，攜帶非阻斷錯誤（如單欄超長捨棄）
    Song(NewSong, Vec<ScanError>),
    /// 拒絕入庫（封面預檢拒絕、取樣率/聲道非法、封包超標、總標籤超標等），錯誤仍彙整
    Rejected(Vec<ScanError>),
    /// 檔案系統層級跳過（路徑驗證失敗、metadata 讀取失敗），攜帶錯誤供前端顯示
    Skipped(Vec<ScanError>),
}

/// SE7：封面預檢的三態結果。無法驗證時採拒絕路徑，不視為「沒有封面」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverStatus {
    NoCover,      // 確定沒有嵌入封面
    HasCover,     // 偵測到嵌入封面
    CheckFailed,  // 無法驗證（開檔失敗、短讀、未知格式、解析失敗）
}

fn has_embedded_cover_precheck(path: &Path) -> Result<CoverStatus, String> {
    let Ok(mut file) = std::fs::File::open(path) else {
        // 無法開啟檔案 = 檔案系統層級失敗，非業務拒絕；回傳 CheckFailed 由呼叫者決定
        return Ok(CoverStatus::CheckFailed);
    };
    let mut header = [0u8; 16];
    if file.read_exact(&mut header).is_err() {
        return Ok(CoverStatus::CheckFailed);
    }

    // FLAC：fLaC 標頭後接 metadata block，類型 6 = PICTURE。
    // 第一個 metadata block 標頭在 offset 4（fLaC 四位元組之後）。
    if &header[0..4] == b"fLaC" {
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(4)).map_err(|e| format!("FLAC 定位失敗: {e}"))?;
        return scan_flac_picture(&mut file);
    }

    // MP3 ID3v2："ID3" 標頭，掃描 frame 尋找 APIC
    if &header[0..3] == b"ID3" {
        return scan_id3v2_apic(&mut file, &header);
    }

    // MP4：ftyp box，掃描 box 尋找 covr
    if &header[4..8] == b"ftyp" {
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(0)).map_err(|e| format!("MP4 定位失敗: {e}"))?;
        return scan_mp4_covr(&mut file, 0, 2 * 1024 * 1024, 0);
    }

    // 未知格式：無法預先偵測封面，fail-open，交由 lofty/ffprobe 後續處理
    Ok(CoverStatus::NoCover)
}

/// 掃描 FLAC metadata block 尋找 PICTURE（block type 6）。
/// file 位置應已定位到第一個 metadata block 標頭（offset 4）。
fn scan_flac_picture(file: &mut std::fs::File) -> Result<CoverStatus, String> {
    use std::io::Read;
    let mut last = false;
    while !last {
        let mut bh = [0u8; 1];
        if file.read_exact(&mut bh).is_err() {
            return Err("讀取 FLAC block 標頭失敗".to_string());
        }
        last = bh[0] & 0x80 != 0;
        let block_type = bh[0] & 0x7f;
        let mut size_buf = [0u8; 3];
        if file.read_exact(&mut size_buf).is_err() {
            return Err("讀取 FLAC block 大小失敗".to_string());
        }
        let size = u32::from_be_bytes([0, size_buf[0], size_buf[1], size_buf[2]]) as u64;
        if block_type == 6 {
            // PICTURE 區塊存在，視為有嵌入封面
            return Ok(CoverStatus::HasCover);
        }
        // 跳過此 block 內容
        let mut remaining = size;
        let mut buf = [0u8; 8192];
        while remaining > 0 {
            let take = remaining.min(buf.len() as u64) as usize;
            if file.read_exact(&mut buf[..take]).is_err() {
                return Err("跳過 FLAC block 內容失敗".to_string());
            }
            remaining -= take as u64;
        }
    }
    Ok(CoverStatus::NoCover)
}

/// 掃描 ID3v2 frame 尋找 APIC（封面圖片 frame）。
/// file 位置已在檔案開頭，header 為前 16 bytes；需先 seek 到 offset 10（標頭長度）再走訪 frame。
fn scan_id3v2_apic(file: &mut std::fs::File, header: &[u8; 16]) -> Result<CoverStatus, String> {
    use std::io::{Read, Seek, SeekFrom};
    // ID3v2 標頭大小（synchsafe integer，4 bytes，7 bits per byte）
    let b0 = header[6] as u32;
    let b1 = header[7] as u32;
    let b2 = header[8] as u32;
    let b3 = header[9] as u32;
    let tag_size = (b0 << 21) | (b1 << 14) | (b2 << 7) | b3;
    let tag_size = tag_size as u64;
    // ID3v2 標頭長度固定 10 bytes，frame 從 offset 10 開始
    file.seek(SeekFrom::Start(10)).map_err(|e| format!("ID3v2 定位失敗: {e}"))?;
    let mut remaining = tag_size.saturating_sub(10);
    let mut buf = [0u8; 10];
    while remaining >= 10 {
        if file.read_exact(&mut buf).is_err() {
            return Err("讀取 ID3v2 frame 標頭失敗".to_string());
        }
        let frame_id = &buf[0..4];
        if frame_id == b"APIC" {
            return Ok(CoverStatus::HasCover);
        }
        let frame_size = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as u64;
        let _flags = u16::from_be_bytes([buf[8], buf[9]]);
        if frame_size == 0 {
            break;
        }
        let skip = frame_size.min(remaining);
        let mut skipped = 0;
        let mut tmp = [0u8; 8192];
        while skipped < skip {
            let take = (skip - skipped).min(tmp.len() as u64) as usize;
            if file.read_exact(&mut tmp[..take]).is_err() {
                return Err("跳過 ID3v2 frame 內容失敗".to_string());
            }
            skipped += take as u64;
        }
        remaining = remaining.saturating_sub(skip);
    }
    Ok(CoverStatus::NoCover)
}

/// 掃描 MP4 box 尋找 covr（封面 box），支援巢狀 box 走訪。
/// file 位置應已定位到起始位置；limit 為掃描上限位元組數；depth 為遞迴深度上限，防止惡意巢狀 box 導致棧溢位（DoS）。
fn scan_mp4_covr(file: &mut std::fs::File, start: u64, limit: u64, depth: u32) -> Result<CoverStatus, String> {
    use std::io::{Read, Seek, SeekFrom};
    const MAX_DEPTH: u32 = 16;
    if depth >= MAX_DEPTH {
        return Err("MP4 container 巢狀過深，採拒絕路徑".to_string());
    }
    let mut pos = start;
    let end = start.saturating_add(limit);
    let mut header = [0u8; 8];
    while pos < end {
        if file.seek(SeekFrom::Start(pos)).is_err() {
            return Err("MP4 box 定位失敗".to_string());
        }
        if file.read_exact(&mut header).is_err() {
            break;
        }
        let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        if size == 0 {
            // size == 0 表示本 box 佔用至檔案/父 box 結尾，無法推導下一個 box 位置，停止掃描
            break;
        }
        if size < 8 {
            return Err("MP4 box 大小異常".to_string());
        }
        // 檢查 type 是否為 covr（4 bytes ASCII）
        if header[4] == b'c' && header[5] == b'o' && header[6] == b'v' && header[7] == b'r' {
            return Ok(CoverStatus::HasCover);
        }
        // 若是 container box（moov/meta/ilst/trak 等），遞迴走訪子 box
        let box_type = &header[4..8];
        let is_container = matches!(box_type, b"moov" | b"meta" | b"ilst" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts");
        if is_container && size > 8 {
            let child_start = pos.saturating_add(8);
            let child_len = size.saturating_sub(8);
            match scan_mp4_covr(file, child_start, child_len, depth.saturating_add(1))? {
                CoverStatus::HasCover => return Ok(CoverStatus::HasCover),
                CoverStatus::CheckFailed => return Err("MP4 container 掃描失敗".to_string()),
                CoverStatus::NoCover => {}
            }
        }
        let next = size.saturating_sub(8);
        pos = pos.saturating_add(8).saturating_add(next);
        if pos > end {
            break;
        }
    }
    Ok(CoverStatus::NoCover)
}

/// SE7：配置前標籤大小預檢。在 lofty::read_from_path() 之前執行，
/// 讀取檔案標頭判斷標籤宣告總大小（外層）；超過 TAG_TOTAL_MAX 則回傳 Err（Rejected）。
/// 無法可靠解析標籤大小時回傳 Err，由呼叫者採拒絕路徑。
fn precheck_tag_budget(path: &Path) -> Result<(), String> {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return Err("無法開啟檔案".to_string());
    };
    let mut header = [0u8; 16];
    if file.read_exact(&mut header).is_err() {
        return Err("無法讀取檔案標頭".to_string());
    }

    // ID3v2：標頭第 6-9 bytes 為 tag size（synchsafe，7 bits per byte）
    if &header[0..3] == b"ID3" {
        let b0 = header[6] as u32;
        let b1 = header[7] as u32;
        let b2 = header[8] as u32;
        let b3 = header[9] as u32;
        let tag_size = (b0 << 21) | (b1 << 14) | (b2 << 7) | b3;
        if tag_size as usize > TAG_TOTAL_MAX {
            return Err(format!(
                "標籤宣告總大小 {} bytes 超過上限 {} bytes",
                tag_size, TAG_TOTAL_MAX
            ));
        }
        return Ok(());
    }

    // FLAC：解析 metadata block，累計 VORBIS_COMMENT（類型 4）大小
    if &header[0..4] == b"fLaC" {
        use std::io::{Seek, SeekFrom};
        if file.seek(SeekFrom::Start(4)).is_err() {
            return Err("FLAC 定位失敗".to_string());
        }
        let mut last = false;
        let mut total_comments = 0usize;
        while !last {
            let mut bh = [0u8; 1];
            if file.read_exact(&mut bh).is_err() {
                return Err("無法讀取 FLAC block 標頭".to_string());
            }
            last = bh[0] & 0x80 != 0;
            let block_type = bh[0] & 0x7f;
            let mut size_buf = [0u8; 3];
            if file.read_exact(&mut size_buf).is_err() {
                return Err("無法讀取 FLAC block 大小".to_string());
            }
            let size = u32::from_be_bytes([0, size_buf[0], size_buf[1], size_buf[2]]) as usize;
            if block_type == 4 {
                total_comments += size;
                if total_comments > TAG_TOTAL_MAX {
                    return Err(format!(
                        "標籤宣告總大小 {} bytes 超過上限 {} bytes",
                        total_comments, TAG_TOTAL_MAX
                    ));
                }
            }
            // 跳過此 block 內容
            let mut remaining = size;
            let mut buf = [0u8; 8192];
            while remaining > 0 {
                let take = remaining.min(buf.len()) as usize;
                if file.read_exact(&mut buf[..take]).is_err() {
                    return Err("跳過 FLAC block 內容失敗".to_string());
                }
                remaining -= take;
            }
        }
        return Ok(());
    }

    // MP4：ftyp header，標籤大小難以僅靠檔案標頭準確判斷，fail-open，交由 lofty/ffprobe 後續處理
    if &header[4..8] == b"ftyp" {
        return Ok(());
    }

    // 未知格式：無法預先判斷標籤大小，fail-open，交由 lofty/ffprobe 後續處理
    Ok(())
}

fn parse_gain_db(s: Option<&String>) -> Option<f32> {
    let v = s?.trim().trim_end_matches("dB").trim().trim_end_matches("db").trim();
    let n: f32 = v.parse().ok()?;
    if !n.is_finite() || n < -60.0 || n > 60.0 { None } else { Some(n) }
}

fn parse_r128_gain(s: &str) -> Option<f32> {
    let n: i16 = s.trim().parse().ok()?;
    let v = n as f32 / 256.0 + 5.0;
    if !v.is_finite() || v < -60.0 || v > 60.0 { None } else { Some(v) }
}

fn parse_peak(s: Option<&String>) -> Option<f32> {
    let n: f32 = s?.trim().parse().ok()?;
    if !n.is_finite() || n <= 0.0 || n > 2.0 { None } else { Some(n) }
}

fn limited_tag(tag: &Tag, key: &ItemKey, total: &mut usize) -> Result<Option<String>, bool> {
    // Ok(None)：欄位不存在，不是超限；Err(_)：超過預算，記錄 ScanError；Ok(Some(s))：正常
    let Some(s) = tag.get_string(&key).map(|s| s.to_string()) else {
        return Ok(None);
    };
    if s.len() > TAG_FIELD_MAX {
        return Err(true); // 單欄超長
    }
    if *total + s.len() > TAG_TOTAL_MAX {
        return Err(true); // 總長超標
    }
    *total += s.len();
    Ok(Some(s))
}

/// 處理單一音樂檔案。沒有標籤或時長讀不到仍入庫，用檔名當歌名，失敗記入錯誤清單。
fn native_bounded_for_scan(path: &std::path::Path, masked: &str) -> Result<Option<lofty::TaggedFile>, ScanResult> {
    use std::io::{Read, Seek};
    let skipped = || ScanResult::Skipped(vec![ScanError {path: masked.to_owned(),reason:"無法讀取音訊標頭".into()}]);

    let mut f = crate::open_with::open_validated_file(path).map_err(|_| skipped())?;

    let metadata = f.metadata().map_err(|_| skipped())?;
    if metadata.len() > 50 * 1024 * 1024 {
        return Ok(None);
    }

    let mut buf = [0u8; 4];
    f.rewind().map_err(|_| skipped())?;
    f.read_exact(&mut buf).map_err(|_| skipped())?;

    f.rewind().map_err(|_| skipped())?;
    let (result, reason) = match &buf {
        b"fLaC" => (lofty::flac::scan::read_native_flac_for_scan(&mut f).map(Some), "FLAC 中繼資料無效"),
        b"OggS" => (lofty::ogg::scan::read_ogg_for_scan(&mut f), "Ogg 中繼資料無效"),
        b"MAC " => (lofty::ape::read_ape_for_scan(&mut f).map(Some), "APE 中繼資料無效"),
        b"wvpk" => (lofty::wavpack::read_wavpack_for_scan(&mut f).map(Some), "WavPack 中繼資料無效"),
        b"RIFF" | b"FORM" => (lofty::iff::scan::read_iff_for_scan(&mut f), "WAV／AIFF 中繼資料無效"),
        _ if &buf[..3] == b"ID3" || (buf[0] == 0xff && (buf[1] & 0xe0) == 0xe0) => {
            let kind = lofty::Probe::new(&mut f).guess_file_type()
                .map_err(|_| skipped())?.file_type();
            f.rewind().map_err(|_| skipped())?;
            match kind {
                Some(lofty::FileType::MPEG) => (lofty::mpeg::read_mpeg_for_scan(&mut f).map(Some), "MP3 中繼資料無效"),
                Some(lofty::FileType::AAC) => (lofty::aac::read_adts_for_scan(&mut f).map(Some), "AAC 中繼資料無效"),
                Some(lofty::FileType::FLAC) => (lofty::flac::scan::read_native_flac_for_scan(&mut f).map(Some), "FLAC 中繼資料無效"),
                Some(lofty::FileType::APE) => (lofty::ape::read_ape_for_scan(&mut f).map(Some), "APE 中繼資料無效"),
                _ => return Ok(None),
            }
        }
        _ => {
            if metadata.len() < 8 {
                return Ok(None);
            }
            let mut header = [0; 8];
            f.read_exact(&mut header).map_err(|_| skipped())?;
            if &header[4..8] != b"ftyp" {
                return Ok(None);
            }
            f.rewind().map_err(|_| skipped())?;
            (lofty::mp4::scan::read_mp4_for_scan(&mut f).map(Some), "MP4 中繼資料無效")
        }
    };
    result.map_err(|_| ScanResult::Rejected(vec![ScanError {
        path: masked.to_owned(),
        reason: reason.to_owned(),
    }]))
}

fn process_single_file(path: &Path) -> ScanResult {
    // XF2：錯誤輸出統一使用遮蔽後的本機路徑，每檔只計算一次共用。
    // 檔案系統存取與 NewSong 的曲庫路徑仍使用原始路徑，不受遮蔽影響。
    let masked_path = crate::open_with::mask_local_path(&path.to_string_lossy());
    // SE1：首次檔案系統存取前必須先驗證本機路徑
    if validate_local_path(path).is_err() {
        return ScanResult::Skipped(vec![ScanError {
            path: masked_path.clone(),
            reason: "路徑驗證失敗，跳過".to_string(),
        }]);
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return ScanResult::Skipped(vec![ScanError {
            path: masked_path.clone(),
            reason: "無法讀取檔案 metadata，跳過".to_string(),
        }]);
    };
    let Ok(modified) = metadata.modified() else {
        return ScanResult::Skipped(vec![ScanError {
            path: masked_path.clone(),
            reason: "無法讀取檔案修改時間，跳過".to_string(),
        }]);
    };
    let Ok(duration) = modified.duration_since(UNIX_EPOCH) else {
        return ScanResult::Skipped(vec![ScanError {
            path: masked_path.clone(),
            reason: "檔案修改時間異常，跳過".to_string(),
        }]);
    };
    let mtime = duration.as_millis() as i64;
    let size = metadata.len() as i64;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("未命名").to_string();
    let format = path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).filter(|e| !e.is_empty());

    let mut errors = Vec::new();

    let native = match native_bounded_for_scan(path, &masked_path) {
        Ok(result) => result,
        Err(outcome) => return outcome,
    };
    let use_legacy_lofty = !matches!(
        format.as_deref().and_then(meta_backend_for), Some(MetaBackend::Ffprobe),
    );
    if native.is_none() && use_legacy_lofty {
        // SE7：封面預檢（在 lofty::read_from_path 之前偵測嵌入圖片；偵測到則直接拒絕入庫）
        match has_embedded_cover_precheck(path) {
            Ok(CoverStatus::HasCover) => {
                errors.push(ScanError {
                    path: masked_path.clone(),
                    reason: "檔案包含嵌入封面，拒絕入庫".to_string(),
                });
                return ScanResult::Rejected(errors);
            }
            // NoCover / CheckFailed：fail-open，交由 lofty/ffprobe 後續處理；驗證失敗不自動拒絕
            Ok(CoverStatus::NoCover) | Ok(CoverStatus::CheckFailed) | Err(_) => {}
        }

        // SE7：配置前標籤大小預檢（在 lofty::read_from_path 之前；總標籤超標則拒絕整檔）
        if let Err(reason) = precheck_tag_budget(path) {
            errors.push(ScanError {
                path: masked_path.clone(),
                reason,
            });
            return ScanResult::Rejected(errors);
        }
    }

    let mut duration_ms = 0i64;
    let mut sample_rate: Option<i32> = None;
    let mut channels: Option<i32> = None;
    let mut bitrate: Option<i32> = None;
    let mut tags: HashMap<String, String> = HashMap::new();
    let mut total_tag_bytes: usize = 0;

    const LOFTY_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB

    // 嘗試 lofty 讀取（≤ 50 MB，且未偵測到嵌入封面）
    let lofty_result = if native.is_some() {
        native
    } else if use_legacy_lofty && metadata.len() <= LOFTY_MAX_FILE_SIZE {
        lofty::read_from_path(path).ok()
    } else {
        None
    };

    let lofty_ok = if let Some(tagged) = lofty_result {
        let props = tagged.properties();
        duration_ms = props.duration().as_millis() as i64;
        // 不使用 .filter(|v| *v > 0) 過濾零值，保留原始值以便驗證
        sample_rate = props.sample_rate().map(|v| v as i32);
        channels = props.channels().map(|v| v as i32);
        bitrate = props.audio_bitrate().map(|v| v as i32);
        if let Some(tag) = tagged.primary_tag() {
            for (key, dst) in [
                (ItemKey::TrackTitle, "title"),
                (ItemKey::TrackArtist, "artist"),
                (ItemKey::AlbumTitle, "album"),
                (ItemKey::AlbumArtist, "album_artist"),
                (ItemKey::Genre, "genre"),
                (ItemKey::Year, "date"),
                (ItemKey::TrackNumber, "track"),
                (ItemKey::DiscNumber, "disc"),
            ] {
                match limited_tag(tag, &key, &mut total_tag_bytes) {
                    Ok(Some(s)) => {
                        tags.insert(dst.to_string(), s);
                    }
                    Ok(None) => {
                        // 欄位不存在，不是超限，不記錄錯誤
                    }
                    Err(_) => {
                        // 第二層裁切：單欄超長或總長超標，記錄 ScanError，繼續其他欄位
                        // XF2：reason 不回顯標籤鍵名，改用靜態訊息
                        errors.push(ScanError {
                            path: masked_path.clone(),
                            reason: "標籤欄位超過大小上限，已捨棄".to_string(),
                        });
                    }
                }
            }

            // ReplayGain 專用 ItemKey（ID3/APE/Vorbis）
            for (key, dst) in [
                (ItemKey::ReplayGainTrackGain, "replaygain_track_gain"),
                (ItemKey::ReplayGainTrackPeak, "replaygain_track_peak"),
                (ItemKey::ReplayGainAlbumGain, "replaygain_album_gain"),
                (ItemKey::ReplayGainAlbumPeak, "replaygain_album_peak"),
            ] {
                if let Some(s) = tag.get_string(&key) {
                    let s = s.to_string();
                    if !s.trim().is_empty() {
                        tags.insert(dst.to_string(), s);
                    }
                }
            }

            // MP4 / Opus 等 Unknown 鍵
            for item in tag.items() {
                if let ItemKey::Unknown(full) = item.key() {
                    let full_l = full.to_ascii_lowercase();
                    let short = full_l.split(':').last().unwrap_or(&full_l).to_string();
                    if (short.starts_with("replaygain_") || short.starts_with("r128_"))
                        && !tags.contains_key(&short)
                    {
                        let val = item
                            .value()
                            .text()
                            .map(|s| s.to_string())
                            .or_else(|| item.value().locator().map(|s| s.to_string()))
                            .unwrap_or_default();
                        if !val.trim().is_empty() {
                            tags.insert(short, val);
                        }
                    }
                }
            }
        }
        true
    } else {
        false
    };

    // ffprobe 後備（lofty 失敗或 >50 MB；標籤套用相同 4 KB/16 KB 預算，SE2）
    if !lofty_ok {
        match ffprobe_meta(path) {
            Ok(meta) => {
                for (k, v) in meta.tags {
                    if v.len() > TAG_FIELD_MAX {
                        // XF2：reason 不回顯來自檔案的標籤鍵名，改用靜態訊息
                        errors.push(ScanError {
                            path: masked_path.clone(),
                            reason: format!("ffprobe 標籤欄位超過 {TAG_FIELD_MAX} bytes 上限，已捨棄"),
                        });
                        continue;
                    }
                    if total_tag_bytes + v.len() > TAG_TOTAL_MAX {
                        errors.push(ScanError {
                            path: masked_path.clone(),
                            reason: "ffprobe 標籤總長超過上限，已捨棄後續欄位".to_string(),
                        });
                        break;
                    }
                    total_tag_bytes += v.len();
                    let mut key = k.to_lowercase();
                    let short = key.split(':').last().unwrap_or(&key).to_string();
                    if (short.starts_with("replaygain_") || short.starts_with("r128_"))
                        && (short.starts_with("replaygain_") || short.starts_with("r128_")) {
                        key = short;
                    }
                    tags.insert(key, v);
                }
                duration_ms = meta.duration_ms;
                sample_rate = meta.sample_rate;
                channels = meta.channels;
                bitrate = meta.bitrate;
            }
            Err(e) => {
                // XF2：ffprobe 錯誤字串皆為既定的通用訊息（找不到執行檔、逾時、配額、JSON 解析失敗等），
                // 不含子程序原始輸出，直接沿用
                errors.push(ScanError {
                    path: masked_path.clone(),
                    reason: e,
                });
                return ScanResult::Rejected(errors);
            }
        }
    }

    // SE7：取樣率與聲道驗證（先檢查 0 值再檢查範圍；非法值拒絕入庫）
    let mut rejected = false;
    match sample_rate {
        Some(0) => {
            errors.push(ScanError {
                path: masked_path.clone(),
                reason: "取樣率為 0，視為非法值".to_string(),
            });
            rejected = true;
        }
        Some(sr) if sr < SAMPLE_RATE_MIN || sr > SAMPLE_RATE_MAX => {
            errors.push(ScanError {
                path: masked_path.clone(),
                reason: format!("取樣率 {sr} Hz 超出有效範圍"),
            });
            rejected = true;
        }
        _ => {}
    }
    match channels {
        Some(0) => {
            errors.push(ScanError {
                path: masked_path.clone(),
                reason: "聲道為 0，視為非法值".to_string(),
            });
            rejected = true;
        }
        Some(ch) if ch < CHANNELS_MIN || ch > CHANNELS_MAX => {
            errors.push(ScanError {
                path: masked_path.clone(),
                reason: format!("聲道 {ch} 超出有效範圍"),
            });
            rejected = true;
        }
        _ => {}
    }
    if rejected {
        return ScanResult::Rejected(errors);
    }

    let title = tags.remove("title").filter(|s| !s.trim().is_empty()).or(Some(stem));
    let artist = tags.remove("artist");
    let album = tags.remove("album");
    let album_artist = tags.remove("album_artist");
    let genre = tags.remove("genre");
    let year = tags.remove("date").and_then(|s| s.parse::<i32>().ok());
    let track_number = tag_number(tags.get("track").map(|s| s.as_str()));
    let disc_number = tag_number(tags.get("disc").map(|s| s.as_str()));

    // ReplayGain 解析
    let mut rg_track_gain = parse_gain_db(tags.get("replaygain_track_gain"));
    let rg_track_peak = parse_peak(tags.get("replaygain_track_peak"));
    let mut rg_album_gain = parse_gain_db(tags.get("replaygain_album_gain"));
    let rg_album_peak = parse_peak(tags.get("replaygain_album_peak"));
    if rg_track_gain.is_none() {
        if let Some(v) = tags.get("r128_track_gain") {
            rg_track_gain = parse_r128_gain(v);
        }
    }
    if rg_album_gain.is_none() {
        if let Some(v) = tags.get("r128_album_gain") {
            rg_album_gain = parse_r128_gain(v);
        }
    }


    let (title_pinyin, title_pinyin_initial) = compute_pinyin(title.as_deref().unwrap_or(""));
    let (artist_pinyin, artist_pinyin_initial) = compute_pinyin(artist.as_deref().unwrap_or(""));

    ScanResult::Song(
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
            replaygain_track_gain: rg_track_gain,
            replaygain_track_peak: rg_track_peak,
            replaygain_album_gain: rg_album_gain,
            replaygain_album_peak: rg_album_peak,
        },
        errors,
    )
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

    #[test]
    fn test_precheck_tag_budget_id3_oversized() {
        use std::io::Write;
        use std::fs::File;
        // 建立一個 ID3v2 標頭，tag size = 20000 bytes（> 16 KB = 16384）
        // synchsafe 編碼 20000 = 0x00 0x01 0x1C 0x20
        let mut id3_header = b"ID3\x03\x00\x00\x00\x01\x1c\x20".to_vec();
        id3_header.resize(16, 0); // 補充到 16 bytes 以滿足 read_exact
        let path = std::env::temp_dir().join(format!("se7_test_oversized_{}.id3", std::process::id()));
        {
            let mut f = File::create(&path).expect("建立測試檔案失敗");
            f.write_all(&id3_header).expect("寫入測試檔案失敗");
        }
        let result = precheck_tag_budget(&path);
        assert!(result.is_err(), "超過 16 KB 的標籤應回傳 Err");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_precheck_tag_budget_id3_within_limit() {
        use std::io::Write;
        use std::fs::File;
        // 建立一個 ID3v2 標頭，tag size = 1000 bytes（< 16 KB = 16384）
        // synchsafe 編碼 1000 = 0x00 0x00 0x07 0xD8
        let mut id3_header = b"ID3\x03\x00\x00\x00\x00\x00\x07\xd8".to_vec();
        id3_header.resize(16, 0); // 補充到 16 bytes 以滿足 read_exact
        let path = std::env::temp_dir().join(format!("se7_test_within_{}.id3", std::process::id()));
        {
            let mut f = File::create(&path).expect("建立測試檔案失敗");
            f.write_all(&id3_header).expect("寫入測試檔案失敗");
        }
        let result = precheck_tag_budget(&path);
        assert!(result.is_ok(), "未超過 16 KB 的標籤應回傳 Ok");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_limited_tag_oversized_field() {
        let oversized = "x".repeat(TAG_FIELD_MAX + 1);
        let within = "ok".to_string();

        assert!(TAG_FIELD_MAX == 4 * 1024);
        assert!(TAG_TOTAL_MAX == 16 * 1024);
        assert!(oversized.len() > TAG_FIELD_MAX);
        assert!(within.len() <= TAG_FIELD_MAX);
        assert!(within.len() <= TAG_TOTAL_MAX);
    }

    /// XF2：早期拒絕（遠端路徑）時，ScanError.path 應為遮蔽後路徑，不外洩使用者目錄。
    /// 選 UNC 路徑是因為 validate_local_path 的步驟 1 在任何檔案系統存取前就以字串檢查拒絕，
    /// 測試結果與本機暫存檔或使用者檔案無關，任何環境下都一致。
    #[test]
    fn test_process_single_file_masks_path_on_early_unsupported() {
        let path = Path::new(r"\\server\Users\alice\Music\song.mp3");
        match process_single_file(path) {
            ScanResult::Skipped(errors) => {
                assert_eq!(errors.len(), 1, "遠端路徑應產生一筆 Skipped 錯誤");
                assert_eq!(
                    errors[0].path,
                    r"\\server\Users\%USER%\Music\song.mp3",
                    "使用者目錄應遮蔽為 %USER%"
                );
                assert_eq!(errors[0].reason, "路徑驗證失敗，跳過");
            }
            other => panic!("遠端路徑應走 Skipped，實際為 {:?}", other),
        }
    }

    #[test]
    #[ignore = "需要本機 FFmpeg 產生跨頁註解的 Ogg Vorbis 與 Opus"]
    fn bounded_ogg_scan_keeps_normal_tags_after_discarding_long_comments() {
        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let folder = tempfile::tempdir().unwrap();
        let metadata = folder.path().join("normal.ffmetadata");
        std::fs::write(&metadata, format!(
            ";FFMETADATA1\ncomment={}\ntitle=Ogg scan fixture\nREPLAYGAIN_TRACK_GAIN=-7.25 dB\n",
            "q".repeat(270 * 1024),
        )).unwrap();

        for (name, codec) in [("comments.ogg", "libvorbis"), ("comments.opus", "libopus")] {
            let path = folder.path().join(name);
            let mut generate = std::process::Command::new(&ffmpeg);
            generate.args([
                "-nostdin", "-y", "-loglevel", "error", "-f", "lavfi", "-i",
                "sine=frequency=440:sample_rate=48000:duration=0.3", "-f", "ffmetadata", "-i",
            ]).arg(&metadata).args(["-map_metadata", "1", "-c:a", codec]).arg(&path)
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                generate.creation_flags(0x0800_0000);
            }
            assert!(generate.status().unwrap().success(), "failed to generate {name}");
            assert!(path.metadata().unwrap().len() > 256 * 1024);
            let tagged = native_bounded_for_scan(&path, "fixture").unwrap().unwrap();
            let tag = tagged.primary_tag().unwrap();
            assert_eq!(tag.get_string(&lofty::ItemKey::TrackTitle), Some("Ogg scan fixture"));
            assert_eq!(tag.get_string(&lofty::ItemKey::Comment), None);
            assert!(tagged.properties().audio_bitrate().is_some_and(|bitrate| bitrate <= 256));

            match process_single_file(&path) {
                ScanResult::Song(song, errors) => {
                    assert!(errors.is_empty());
                    assert_eq!(song.title.as_deref(), Some("Ogg scan fixture"));
                    assert_eq!(song.sample_rate, Some(48_000));
                    assert_eq!(song.channels, Some(1));
                    assert_eq!(song.replaygain_track_gain, Some(-7.25));
                    assert!((290..=310).contains(&song.duration_ms), "{name}: {}", song.duration_ms);
                }
                other => panic!("{name}: {other:?}"),
            }
        }
    }

    #[test]
    #[ignore = "需要本機 FFmpeg 產生帶有正常 APEv2 標籤的 WavPack"]
    fn bounded_ape_tags_keep_wavpack_audio_properties_and_short_fields() {
        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("fixture.wv");
        let comment = format!("comment={}", "w".repeat(8_192));
        let mut generate = std::process::Command::new(&ffmpeg);
        generate.args([
            "-nostdin", "-y", "-loglevel", "error", "-f", "lavfi", "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.3", "-c:a", "wavpack",
            "-metadata", &comment, "-metadata", "title=WavPack scan fixture",
            "-metadata", "REPLAYGAIN_TRACK_GAIN=-6.5 dB",
        ]).arg(&path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            generate.creation_flags(0x0800_0000);
        }
        assert!(generate.status().unwrap().success());
        let tagged = native_bounded_for_scan(&path, "fixture").unwrap().unwrap();
        let tag = tagged.primary_tag().unwrap();
        assert_eq!(tag.get_string(&lofty::ItemKey::Comment), None);
        match process_single_file(&path) {
            ScanResult::Song(song, errors) => {
                assert!(errors.is_empty());
                assert_eq!(song.title.as_deref(), Some("WavPack scan fixture"));
                assert_eq!(song.sample_rate, Some(48_000));
                assert_eq!(song.channels, Some(1));
                assert_eq!(song.replaygain_track_gain, Some(-6.5));
                assert_eq!(song.duration_ms, 300);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    #[ignore = "需要本機 FFmpeg 產生合成 ID3v2.3 與 ID3v2.4 MP3"]
    fn bounded_id3_tags_keep_mpeg_title_and_properties() {
        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let folder = tempfile::tempdir().unwrap();
        for version in ["3", "4"] {
            let path = folder.path().join(format!("fixture-v{version}.mp3"));
            let comment = format!("comment={}", "m".repeat(8_192));
            let mut generate = std::process::Command::new(&ffmpeg);
            generate.args([
                "-nostdin", "-y", "-loglevel", "error", "-f", "lavfi", "-i",
                "sine=frequency=440:sample_rate=44100:duration=0.3", "-c:a", "libmp3lame",
                "-id3v2_version", version, "-metadata", &comment, "-metadata", "title=MP3 scan fixture",
                "-metadata", "REPLAYGAIN_TRACK_GAIN=-5.0 dB",
            ]).arg(&path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                generate.creation_flags(0x0800_0000);
            }
            assert!(generate.status().unwrap().success());
            let tagged = native_bounded_for_scan(&path, "fixture").unwrap().unwrap();
            let tag = tagged.primary_tag().unwrap();
            assert_eq!(tag.get_string(&lofty::ItemKey::Comment), None);
            match process_single_file(&path) {
                ScanResult::Song(song, errors) => {
                    assert!(errors.is_empty());
                    assert_eq!(song.title.as_deref(), Some("MP3 scan fixture"));
                    assert_eq!(song.sample_rate, Some(44_100));
                    assert_eq!(song.channels, Some(1));
                    assert_eq!(song.replaygain_track_gain, Some(-5.0));
                    assert!((285..=370).contains(&song.duration_ms));
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    #[ignore = "需要本機 FFmpeg 產生合成 WAV 與 AIFF"]
    fn bounded_iff_scan_keeps_audio_properties_and_short_id3_fields() {
        use lofty::TagExt;

        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let folder = tempfile::tempdir().unwrap();
        let mut tags = lofty::Tag::new(lofty::TagType::ID3v2);
        assert!(tags.insert_text(lofty::ItemKey::TrackTitle, "IFF scan fixture".into()));
        assert!(tags.insert_text(lofty::ItemKey::Comment, "t".repeat(8_192)));
        let mut tag_bytes = Vec::new();
        tags.dump_to(&mut tag_bytes).unwrap();
        assert!(tag_bytes.len() > 4_096);

        for (name, codec, little_endian) in [
            ("tagged.wav", "pcm_s16le", true),
            ("tagged.aiff", "pcm_s16be", false),
        ] {
            let path = folder.path().join(name);
            let mut generate = std::process::Command::new(&ffmpeg);
            generate.args([
                "-nostdin", "-y", "-loglevel", "error", "-f", "lavfi", "-i",
                "sine=frequency=440:sample_rate=48000:duration=0.3", "-c:a", codec,
            ]).arg(&path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                generate.creation_flags(0x0800_0000);
            }
            assert!(generate.status().unwrap().success());
            let size_bytes = |size: usize| {
                let size = u32::try_from(size).unwrap();
                if little_endian { size.to_le_bytes() } else { size.to_be_bytes() }
            };
            let mut audio = std::fs::read(&path).unwrap();
            audio.extend_from_slice(b"ID3 ");
            audio.extend_from_slice(&size_bytes(tag_bytes.len()));
            audio.extend_from_slice(&tag_bytes);
            if tag_bytes.len() % 2 != 0 {
                audio.push(0);
            }
            let size = size_bytes(audio.len() - 8);
            audio[4..8].copy_from_slice(&size);
            std::fs::write(&path, audio).unwrap();
            let tagged = native_bounded_for_scan(&path, "fixture").unwrap().unwrap();
            assert_eq!(tagged.primary_tag().unwrap().get_string(&lofty::ItemKey::Comment), None);
            match process_single_file(&path) {
                ScanResult::Song(song, errors) => {
                    assert!(errors.is_empty());
                    assert_eq!(song.title.as_deref(), Some("IFF scan fixture"));
                    assert_eq!(song.sample_rate, Some(48_000));
                    assert_eq!(song.channels, Some(1));
                    assert_eq!(song.duration_ms, 300);
                }
                other => panic!("{name}: {other:?}"),
            }
        }
    }

    #[test]
    #[ignore = "需要本機 FFmpeg 產生有標題、曲號及封面的正常 AAC"]
    fn bounded_mp4_scan_keeps_aac_properties_without_reading_cover_or_long_comment() {
        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("fixture.m4a");
        let artwork = folder.path().join("fixture.png");
        image::RgbImage::from_pixel(4, 4, image::Rgb([40, 80, 120])).save(&artwork).unwrap();
        let comment = format!("comment={}", "p".repeat(8_192));
        let mut generate = std::process::Command::new(&ffmpeg);
        generate.args([
            "-nostdin", "-y", "-loglevel", "error", "-f", "lavfi", "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.3", "-i",
        ]).arg(&artwork).args([
            "-map", "0:a", "-map", "1:v", "-c:a", "aac", "-c:v", "png",
            "-disposition:v", "attached_pic", "-movflags", "+faststart",
            "-metadata", &comment, "-metadata", "title=MP4 scan fixture", "-metadata", "track=2/7",
        ]).arg(&path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            generate.creation_flags(0x0800_0000);
        }
        assert!(generate.status().unwrap().success());
        let default_tags = lofty::read_from_path(&path).unwrap();
        let original = default_tags.primary_tag().unwrap();
        assert!(!original.pictures().is_empty());
        assert_eq!(original.get_string(&lofty::ItemKey::TrackNumber), Some("2"));
        let tagged = native_bounded_for_scan(&path, "fixture").unwrap().unwrap();
        let tag = tagged.primary_tag().unwrap();
        assert!(tag.pictures().is_empty());
        assert_eq!(tag.get_string(&lofty::ItemKey::Comment), None);
        match process_single_file(&path) {
            ScanResult::Song(song, errors) => {
                assert!(errors.is_empty());
                assert_eq!(song.title.as_deref(), Some("MP4 scan fixture"));
                assert_eq!(song.track_number, Some(2));
                assert_eq!(song.sample_rate, Some(48_000));
                assert_eq!(song.channels, Some(1));
                assert!((290..=350).contains(&song.duration_ms));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    #[ignore = "需要本機 FFmpeg 產生標準 ADTS AAC 與原生 FLAC"]
    fn bounded_scan_keeps_audio_properties_with_external_id3_prefixes() {
        use lofty::TagExt;

        let ffmpeg = crate::tools::locate("ffmpeg.exe").unwrap().0;
        let folder = tempfile::tempdir().unwrap();
        let mut tags = lofty::Tag::new(lofty::TagType::ID3v2);
        assert!(tags.insert_text(lofty::ItemKey::TrackTitle, "ID3 prefix fixture".into()));
        assert!(tags.insert_text(lofty::ItemKey::Comment, "l".repeat(8_192)));
        let mut prefix = Vec::new();
        tags.dump_to(&mut prefix).unwrap();
        for (name, codec, format) in [
            ("prefixed.aac", "aac", "adts"),
            ("prefixed.flac", "flac", "flac"),
        ] {
            let path = folder.path().join(name);
            let mut generate = std::process::Command::new(&ffmpeg);
            generate.args([
                "-nostdin", "-y", "-loglevel", "error", "-f", "lavfi", "-i",
                "sine=frequency=440:sample_rate=48000:duration=0.3", "-c:a", codec, "-f", format,
            ]).arg(&path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                generate.creation_flags(0x0800_0000);
            }
            assert!(generate.status().unwrap().success());
            let mut file_bytes = prefix.clone();
            file_bytes.extend_from_slice(&std::fs::read(&path).unwrap());
            std::fs::write(&path, file_bytes).unwrap();

            let tagged = native_bounded_for_scan(&path, "fixture").unwrap().unwrap();
            let id3 = tagged.tag(lofty::TagType::ID3v2).unwrap();
            assert_eq!(id3.get_string(&lofty::ItemKey::TrackTitle), Some("ID3 prefix fixture"));
            assert_eq!(id3.get_string(&lofty::ItemKey::Comment), None);
            match process_single_file(&path) {
                ScanResult::Song(song, errors) => {
                    assert!(errors.is_empty());
                    assert_eq!(song.sample_rate, Some(48_000));
                    assert_eq!(song.channels, Some(1));
                    assert!((240..=400).contains(&song.duration_ms), "{name}: {}", song.duration_ms);
                }
                other => panic!("{name}: {other:?}"),
            }
        }
    }

    #[test] fn native_flac_scan_reaches_song() {
        use super::*;

        use std::path::Path;
        use std::env;

        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats/sample_44100_stereo.flac");
        assert!(native_bounded_for_scan(&path, "fixture").unwrap().is_some());

        match process_single_file(&path) {
            ScanResult::Song(song, errors) => {
                assert!(errors.is_empty());
                assert_eq!(song.sample_rate, Some(44100));
                assert_eq!(song.channels, Some(2));
                assert_eq!(song.duration_ms, 3000);
                assert!(song.cover_path.is_none());
            }
            other => panic!("{other:?}"),
        }
    }
}
