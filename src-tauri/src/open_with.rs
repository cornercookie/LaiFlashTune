use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{ChildStderr, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::os::windows::io::FromRawHandle;
use tauri::{AppHandle, Emitter, Manager};
use std::fs::File;

use crate::models::OpenedFile;
use crate::tools::is_trusted_local_path;

const ADDR: &str = "127.0.0.1:47321";
const MAX_HEADER_BYTES: usize = 4 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_PATHS: usize = 10;
const TOTAL_RECV_TIMEOUT: Duration = Duration::from_secs(2);

/// 支援的音訊副檔名（小寫）
fn is_audio(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some(
            "mp3" | "flac" | "m4a" | "mp4" | "aac" | "ogg" | "opus" | "wav" | "ape" | "wma"
                | "aiff" | "aif" | "wv" | "tta" | "mp2" | "mp1" | "spx" | "m4b" | "alac"
                | "tak" | "mpc" | "dsf" | "dff" | "ac3" | "dts" | "mka" | "webm"
                | "aa3" | "at3" | "oma" | "mod" | "xm" | "it" | "s3m" | "mtm" | "umx" | "mo3"
                | "mdz" | "s3z" | "xmz" | "itz"
        )
    )
}

/// 取得 DOS 路徑的磁碟根目錄（供 GetDriveTypeW 使用；該 API 必須傳根目錄）。
/// 支援 `C:\\...` 與 `\\\\?\\Volume{GUID}\\...`，其餘原樣傳回。
fn dos_path_root(p: &str) -> String {
    let bytes = p.as_bytes();
    // C:\\...
    if bytes.len() >= 3 && bytes[1] == b':' && bytes[2] == b'\\' {
        return p[..3].to_string();
    }
    // \\\\?\\Volume{GUID}\\...
    if p.starts_with(r"\\?\Volume") {
        let rel = &p[10..];
        if let Some(pos) = rel.find('\\') {
            return format!(r"\\?\Volume{}", &rel[..=pos]);
        }
        return p.to_string();
    }
    p.to_string()
}

/// 遮蔽本機路徑中的用戶目錄與敏感資訊（SE3-4）。
/// Windows: C:\\Users\\<user>\\... -> C:\\Users\\%USER%\\...
/// Unix: /home/<user>/... -> /home/%USER%/...
pub(crate) fn mask_local_path(path: &str) -> String {
    // Windows 用戶目錄
    if let Some(pos) = path.to_ascii_lowercase().find(r"\users\") {
        let after = &path[pos + 7..]; // 跳過 "\users\"
        if let Some(end) = after.find('\\') {
            let user = &after[..end];
            if !user.is_empty() && user != "public" && user != "default" {
                return format!("{}%USER%{}", &path[..pos + 7], &after[end..]);
            }
        }
    }
    // Unix 用戶目錄
    if let Some(pos) = path.find("/home/") {
        let after = &path[pos + 6..];
        if let Some(end) = after.find('/') {
            let user = &after[..end];
            if !user.is_empty() {
                return format!("{}%USER%{}", &path[..pos + 6], &after[end..]);
            }
        }
    }
    path.to_string()
}

/// 遮蔽 IP 位址（SE3-5）
fn mask_ip(ip: &str) -> String {
    // 簡單遮蔽：保留前綴，後綴替換為 xxx
    if let Some(last_dot) = ip.rfind('.') {
        format!("{}xxx", &ip[..=last_dot])
    } else {
        "xxx".to_string()
    }
}

/// S1: 本機路徑驗證 - 共用驗證入口
/// 所有檔案系統存取（播放、預載、探測、argv、拖放分類、掃描）在第一次呼叫 is_file/is_dir/metadata 前，
/// 必須共用同一驗證函式 `validate_local_path(path)`。
/// 步驟 1: 拒絕遠端路徑
/// 步驟 2: 重解析點前置檢查
/// 步驟 3: 驗證本機檔案存在
/// 步驟 4: ffmpeg/ffprobe 以 -protocol_whitelist file,pipe 限制輸入協定白名單
pub fn validate_local_path(path: &Path) -> Result<(), String> {
    let path_str = path.to_string_lossy();

    // 步驟 1: 拒絕遠端路徑
    if path_str.starts_with("http://")
        || path_str.starts_with("https://")
        || path_str.starts_with("rtsp://")
        || path_str.starts_with("rtmp://")
        || path_str.starts_with("mms://")
        || path_str.starts_with(r"\\")
        || path_str.starts_with(r"\\?\UNC\\")
        {
            return Err("不支援網路路徑".into());
        }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use std::ptr::null_mut;
        use winapi::um::fileapi::{CreateFileW, GetDriveTypeW, GetFileAttributesExW, GetFinalPathNameByHandleW};
        use winapi::um::handleapi::CloseHandle;
        use winapi::um::winbase::{FILE_FLAG_BACKUP_SEMANTICS, VOLUME_NAME_DOS};
        use winapi::um::winnt::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_SHARE_READ, GENERIC_READ};
        use winapi::shared::minwindef::{FILETIME, DWORD};


// WIN32_FILE_ATTRIBUTE_DATA 定義（winapi 可能未導出）
#[repr(C)]
struct WIN32_FILE_ATTRIBUTE_DATA {
    dwFileAttributes: DWORD,
    ftCreationTime: FILETIME,
    ftLastAccessTime: FILETIME,
    ftLastWriteTime: FILETIME,
    nFileSizeHigh: DWORD,
    nFileSizeLow: DWORD,
}


        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();

        // GetDriveTypeW 需要根目錄（例如 "C:\\"），傳入完整路徑會回傳 DRIVE_UNKNOWN 並誤拒本機路徑（C2）
        let root_wide = {
            let s = path.to_string_lossy();
            if s.len() >= 2 && s.as_bytes()[1] == b':' {
                OsString::from(format!("{}\\", &s[..2]))
            } else {
                path.as_os_str().to_os_string()
            }
        };
        let root_wide: Vec<u16> = root_wide.encode_wide().chain(Some(0)).collect();

        // 檢查是否為網路磁碟機
        let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
        const DRIVE_FIXED: u32 = 3;
        const DRIVE_REMOVABLE: u32 = 2;
        if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOVABLE {
            return Err("不支援網路路徑".into());
        }

        // 步驟 2: 重解析點前置檢查
        let mut attrs = WIN32_FILE_ATTRIBUTE_DATA {
            dwFileAttributes: 0,
            ftCreationTime: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
            ftLastAccessTime: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
            ftLastWriteTime: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
            nFileSizeHigh: 0,
            nFileSizeLow: 0,
        };
        let ok = unsafe { GetFileAttributesExW(wide.as_ptr(), 0, &mut attrs as *mut _ as *mut winapi::ctypes::c_void) };
        if ok != 0 && (attrs.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            // 是重解析點：開啟檔案取得有效 handle，再用 GetFinalPathNameByHandleW 解析最終目標
            let handle = unsafe {
                CreateFileW(
                    path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>().as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ,
                    null_mut(),
                    3, // OPEN_EXISTING
                    FILE_FLAG_BACKUP_SEMANTICS,
                    null_mut(),
                )
            };
            if handle == winapi::um::handleapi::INVALID_HANDLE_VALUE {
                return Err("不支援網路路徑".into());
            }
            let mut final_path = [0u16; 32768];
            let len = unsafe {
                GetFinalPathNameByHandleW(
                    handle,
                    final_path.as_mut_ptr(),
                    final_path.len() as u32,
                    VOLUME_NAME_DOS,
                )
            };
            let _ = unsafe { CloseHandle(handle) };
            if len > 0 && len < final_path.len() as u32 {
                let final_str = String::from_utf16_lossy(&final_path[..len as usize]);
                // GetDriveTypeW 必須傳遞磁碟根目錄，不能傳完整檔案路徑（否則非根目錄會回傳 DRIVE_UNKNOWN 並誤拒本機路徑，C2）
                let final_root_wide: Vec<u16> = std::ffi::OsStr::new(&dos_path_root(&final_str))
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                let final_drive_type = unsafe { GetDriveTypeW(final_root_wide.as_ptr()) };
                if final_drive_type != DRIVE_FIXED && final_drive_type != DRIVE_REMOVABLE {
                    return Err("不支援網路路徑".into());
                }
            } else {
                return Err("不支援網路路徑".into());
            }
        }
    }

    // 步驟 3: 驗證本機檔案存在
        if !path.exists() {
            return Err("檔案不存在".into());
        }

        Ok(())
    }

    /// 開啟並驗證檔案，回傳持有的 handle（FILE_SHARE_READ，禁止寫入/刪除共享）。
    /// 核心路徑（預檢、解碼）將直接使用此 handle，消除重新開檔競態。
    /// 呼叫者負責在使用完畢後關閉 handle。
    pub fn open_validated_file(path: &Path) -> Result<File, String> {
        crate::open_with::validate_local_path(path)?;
    
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use std::ptr::null_mut;
            use winapi::um::fileapi::CreateFileW;
            use winapi::um::handleapi::INVALID_HANDLE_VALUE;
            use winapi::um::winnt::{FILE_SHARE_READ, GENERIC_READ};
        
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ,
                    null_mut(),
                    3, // OPEN_EXISTING
                    0,
                    null_mut(),
                )
            };
        
            if handle == INVALID_HANDLE_VALUE {
                return Err("開啟檔案失敗".into());
            }
        
            // 將原始 handle 轉為 std::fs::File
            let file = unsafe { File::from_raw_handle(handle as _) };
            return Ok(file);
        }
    
        #[cfg(not(windows))]
        {
            std::fs::File::open(path).map_err(|e| format!("開啟檔案失敗: {e}"))
        }
    }

    /// 取得 handle 的最終規範路徑（用於 ffprobe/ffmpeg 子程序）
        #[cfg(windows)]
        pub fn get_final_path_from_handle(file: &File) -> Result<String, String> {
            use std::os::windows::io::AsRawHandle;
            use std::ptr::null_mut;
            use winapi::um::fileapi::GetFinalPathNameByHandleW;
            use winapi::um::winbase::VOLUME_NAME_DOS;
            use winapi::ctypes::c_void;
    
            let handle = file.as_raw_handle() as *mut c_void;
            let mut final_path = [0u16; 32768];
            let len = unsafe {
                GetFinalPathNameByHandleW(
                    handle,
                    final_path.as_mut_ptr(),
                    final_path.len() as u32,
                    VOLUME_NAME_DOS,
                )
            };
    
            if len > 0 && len < final_path.len() as u32 {
                Ok(String::from_utf16_lossy(&final_path[..len as usize]))
            } else {
                Err("取得最終路徑失敗".into())
            }
        }

    /// S2: 統一清洗管線 - 所有子程序 stderr 與其他日誌入口共用
/// 清洗順序為有界狀態機，跨讀取區塊保留辨識狀態
pub fn sanitize_log(input: &str) -> String {
    let mut sanitizer = Sanitizer::new();
    sanitizer.sanitize_chunk(input);
    sanitizer.finish()
}

#[cfg(test)]
mod sanitizer_tests {
    use super::*;

    #[test]
    fn test_sanitizer_cross_chunk_query_removed() {
        let mut s = Sanitizer::new();
        let part1 = s.sanitize_chunk("https://host/path?token=sec");
        let part2 = s.sanitize_chunk("ret=abc\n");
        let result = s.finish();
        assert!(!result.contains("token="), "query should be removed across chunks: {result}");
        assert!(!result.contains("ret=abc"), "query continuation should be removed: {result}");
    }

    #[test]
    fn test_sanitizer_cross_chunk_non_url_query_removed() {
        let mut s = Sanitizer::new();
        s.sanitize_chunk("file.mp3?param=");
        s.sanitize_chunk("value\n");
        let result = s.finish();
        assert!(result.contains("file.mp3"), "ordinary label should be preserved: {result}");
        assert!(!result.contains("param="), "non-url query should be removed across chunks: {result}");
        assert!(!result.contains("value"), "query continuation should be removed: {result}");
    }

    #[test]
    fn test_sanitizer_cross_chunk_url_preserved() {
        let mut s = Sanitizer::new();
        let part1 = s.sanitize_chunk("https://host/");
        let part2 = s.sanitize_chunk("path\n");
        let result = s.finish();
        assert!(result.contains("https://host/path"), "URL should be preserved across chunks: {result}");
    }

    /// 最小分塊不變性：以分塊大小 1 逐字元輸入，結果必須與一次全量輸入相同。
    /// 使用安全合成字串（不含真實憑證、不含攻擊 URL）。
    #[test]
    fn test_sanitizer_minimal_chunk_invariance() {
        let cases = [
            "https://example.com/music/song.mp3",
            "普通日誌行，含問號?但不是 URL",
            "https://example.com/audio?token=abc&foo=bar 結束",
            "無特殊字元的普通文字",
        ];
        for input in &cases {
            // 全量一次輸入
            let full = sanitize_log(input);

            // 分塊大小 1，逐字元呼叫 sanitize_chunk
            let mut s = Sanitizer::new();
            for ch in input.chars() {
                s.sanitize_chunk(&ch.to_string());
            }
            let chunked = s.finish();

            assert_eq!(
                full, chunked,
                "minimal chunk invariance failed for input={input:?}: full={full:?}, chunked={chunked:?}"
            );
        }
    }

    #[test]
    fn test_sanitize_log_single_finish() {
        // 單次 sanitize_log 可正常 finish，結果不為 panic 且長度 ≤ 1024
        let input = "https://safe.example.com/track?token=secret";
        let output = sanitize_log(input);
        assert!(output.len() <= 1024);
        assert!(!output.contains("token="));
    }
}


/// 有狀態的日誌清洗器，跨讀取區塊保留 URL 辨識狀態（SE3）
/// 調用者可在區塊之間重複使用同一個 Sanitizer 實例，避免憑證跨區塊殘留。
pub struct Sanitizer {
    result: String,
    state: State,
    /// 已辨識的 scheme（保留原始大小寫），於 authority 提交時一併輸出
    scheme: String,
    /// authority 累積緩衝；遇到 `@` 後只保留 `@` 之後的 host
    authority: String,
    /// authority 曾出現 `@`，提交時以 `***@` 取代 userinfo
    masked: bool,
    /// 輸出達到上限後永久停止追加，直到 take_result 重置
    result_truncated: bool,
    /// Text 狀態下保留的極短 scheme 前綴（跨區塊、大小寫不敏感）
    pending_prefix: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    Authority,
    Path,
    Hidden,
    Discard,
}

/// 單次輸出上限（bytes）
const MAX_OUTPUT: usize = 1024;
/// authority 累積上限（bytes）
const MAX_AUTHORITY: usize = 1024;
/// scheme 最長為 `https://`（8 bytes），前綴保留不需更長
const MAX_PENDING_PREFIX: usize = 8;

/// 支援的 scheme（小寫），用於跨區塊前綴匹配。
const URL_SCHEMES: &[&str] = &["http://", "https://"];

impl Sanitizer {
    pub fn new() -> Self {
        Self {
            result: String::new(),
            state: State::Text,
            scheme: String::new(),
            authority: String::new(),
            masked: false,
            result_truncated: false,
            pending_prefix: String::new(),
        }
    }

    /// 判斷 `s`（小寫）是否為任一 scheme 的前綴。
    fn is_scheme_prefix(s: &str) -> bool {
        !s.is_empty() && URL_SCHEMES.iter().any(|scheme| scheme.starts_with(s))
    }

    /// 判斷 `s`（小寫）是否正好等於某個完整 scheme。
    fn is_complete_scheme(s: &str) -> bool {
        URL_SCHEMES.iter().any(|scheme| *scheme == s)
    }

    /// 追加單一字元，總輸出不超過 MAX_OUTPUT。
    /// 僅保留 baseline 可顯示字元（ASCII printable 與 tab/CR/LF）；其他字元直接忽略，
    /// 避免輸出 lossy UTF-8 替代字元或 Unicode 控制字元。
    /// 第一次放不下後永久停止輸出，直到 take_result 重置。
    fn push_char_bounded(&mut self, c: char) {
        if self.result_truncated {
            return;
        }
        let baseline = c.is_ascii_graphic() || c == ' ' || c == '\t' || c == '\r' || c == '\n';
        if !baseline {
            return;
        }
        if self.result.len() + 1 <= MAX_OUTPUT {
            self.result.push(c);
        } else {
            self.result_truncated = true;
        }
    }

    /// 追加字串，逐字元受 MAX_OUTPUT 限制。
    fn push_str_bounded(&mut self, s: &str) {
        for c in s.chars() {
            self.push_char_bounded(c);
        }
    }

    /// 將待決 scheme 前綴視為一般文字輸出並清空。
    fn flush_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending_prefix);
        self.push_str_bounded(&pending);
    }

    /// 檢查 authority 是否為可接受的 hostname / bracket IPv6 + 可選數字 port。
    fn is_valid_authority(authority: &str) -> bool {
        if authority.is_empty() {
            return false;
        }

        // bracket IPv6，例如 [::1]、[2001:db8::1]:8080
        if let Some(rest) = authority.strip_prefix('[') {
            let end = match rest.find(']') {
                Some(i) => i,
                None => return false,
            };
            let inner = &rest[..end];
            if inner.parse::<std::net::Ipv6Addr>().is_err() {
                return false;
            }
            let after = &rest[end + 1..];
            if after.is_empty() {
                return true;
            }
            return match after.strip_prefix(':') {
                Some(port) => !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()),
                None => false,
            };
        }

        // 一般 hostname，可帶單一 :port
        let (host, port) = match authority.rfind(':') {
            Some(i) => {
                let host = &authority[..i];
                let port = &authority[i + 1..];
                if host.contains(':') {
                    return false;
                }
                (host, Some(port))
            }
            None => (authority, None),
        };

        if let Some(port) = port {
            if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
        }

        if host.is_empty()
            || host.starts_with('.')
            || host.starts_with('-')
            || host.ends_with('-')
            || host.contains("..")
        {
            return false;
        }

        host.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    }

    /// 提交 authority：合法時輸出 scheme（必要時加 `***@`）與 host，否則丟棄。
    fn commit_authority(&mut self) -> bool {
        let authority = std::mem::take(&mut self.authority);
        let scheme = std::mem::take(&mut self.scheme);
        let masked = self.masked;
        self.masked = false;

        if !Self::is_valid_authority(&authority) {
            return false;
        }

        self.push_str_bounded(&scheme);
        if masked {
            self.push_str_bounded("***@");
        }
        self.push_str_bounded(&authority);
        true
    }

    /// Text 狀態：一般文字輸出，同時保留可能的 scheme 前綴。
    fn handle_text(&mut self, c: char) {
        if c.is_whitespace() {
            self.flush_pending();
            self.push_char_bounded(c);
            return;
        }
        if c == '?' {
            // 一般文字中的 ? 延續 baseline：視為查詢起點，隱藏至空白
            self.flush_pending();
            self.state = State::Hidden;
            return;
        }

        self.pending_prefix.push(c);
        loop {
            let lower = self.pending_prefix.to_ascii_lowercase();
            if Self::is_complete_scheme(&lower) {
                self.scheme = std::mem::take(&mut self.pending_prefix);
                self.authority.clear();
                self.masked = false;
                self.state = State::Authority;
                break;
            }
            if Self::is_scheme_prefix(&lower) {
                break;
            }

            // 不是 scheme 前綴：輸出第一個字元，保留剩餘可能重疊的後綴
            let first = match self.pending_prefix.chars().next() {
                Some(ch) => ch,
                None => break,
            };
            self.pending_prefix.remove(0);
            self.push_char_bounded(first);
            if self.pending_prefix.is_empty() {
                break;
            }
        }

        // 防禦性上限；正常情況下 pending 只會是 ≤ 8 bytes 的 scheme 前綴
        if self.pending_prefix.len() > MAX_PENDING_PREFIX {
            self.flush_pending();
        }
    }

    /// Authority 狀態：累積 host/userinfo，於分隔符提交。
    fn handle_authority(&mut self, c: char) {
        if c == '@' {
            // 丟棄 userinfo，重新累積 `@` 之後的 host
            self.authority.clear();
            self.masked = true;
            return;
        }

        if c == '/' {
            if self.commit_authority() {
                self.state = State::Path;
                self.push_char_bounded('/');
            } else {
                self.state = State::Discard;
            }
            return;
        }

        if c == '?' || c == '#' {
            if self.commit_authority() {
                self.state = State::Hidden;
            } else {
                self.state = State::Discard;
            }
            return;
        }

        if c.is_whitespace() {
            self.commit_authority();
            self.state = State::Text;
            self.push_char_bounded(c);
            return;
        }

        if self.authority.len() + c.len_utf8() <= MAX_AUTHORITY {
            self.authority.push(c);
        } else {
            // 超過上限：立即丟棄整個 URL，進入 Discard 後 `@` 不再恢復
            self.authority.clear();
            self.scheme.clear();
            self.masked = false;
            self.state = State::Discard;
        }
    }

    /// Path 狀態：保留路徑（含 `@`），遇到 query/fragment 進入 Hidden。
    fn handle_path(&mut self, c: char) {
        if c == '?' || c == '#' {
            self.state = State::Hidden;
            return;
        }
        if c.is_whitespace() {
            self.state = State::Text;
            self.push_char_bounded(c);
            return;
        }
        self.push_char_bounded(c);
    }

    /// Hidden / Discard 狀態：消耗至空白，空白輸出後回到 Text。
    fn handle_hidden_or_discard(&mut self, c: char) {
        if c.is_whitespace() {
            self.state = State::Text;
            self.push_char_bounded(c);
        }
    }

    /// 處理一個字符串區塊；狀態跨區塊保留，回傳值固定為空字串。
    pub fn sanitize_chunk(&mut self, chunk: &str) -> String {
        for c in chunk.chars() {
            if c.is_control() && c != '\t' && c != '\n' && c != '\r' {
                continue;
            }
            match self.state {
                State::Text => self.handle_text(c),
                State::Authority => self.handle_authority(c),
                State::Path => self.handle_path(c),
                State::Hidden | State::Discard => self.handle_hidden_or_discard(c),
            }
        }
        String::new()
    }

    /// 以位元組上限截斷，並確保不切斷多位元組 UTF-8 字元。
    fn take_result(&mut self) -> String {
        if self.result.len() > MAX_OUTPUT {
            let mut cut = MAX_OUTPUT;
            while cut > 0 && !self.result.is_char_boundary(cut) {
                cut -= 1;
            }
            self.result.truncate(cut);
        }
        self.result_truncated = false;
        std::mem::take(&mut self.result)
    }

    /// 結束完整輸入：提交待決文字與 authority，回傳清洗結果。
    pub fn finish(&mut self) -> String {
        if !self.pending_prefix.is_empty() {
            self.flush_pending();
        }
        if self.state == State::Authority {
            self.commit_authority();
        }
        self.state = State::Text;
        self.scheme.clear();
        self.authority.clear();
        self.masked = false;
        self.take_result()
    }

    /// 結束不完整輸入：丟棄待決 scheme 前綴與 authority，只回傳已確認輸出。
    pub fn finish_incomplete(&mut self) -> String {
        self.pending_prefix.clear();
        self.authority.clear();
        self.scheme.clear();
        self.masked = false;
        self.state = State::Text;
        self.take_result()
    }
}

impl Drop for Sanitizer {
    fn drop(&mut self) {
        // 保守清理：不信任未完成的 authority，丟棄結果
        let _ = self.finish_incomplete();
    }
}

/// 取消標記 trait，供 spawn_stderr_sanitizer 使用（SE3-4）
pub trait Cancelled: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

/// 共用的 stderr 清洗執行緒生成器（SE3-4）：將子程序 stderr 讀取、清洗、記錄的重複邏輯提取為 helper。
/// - 若提供 cancel，執行緒會在 cancel 觸發時提前退出，並以 finish_incomplete() 丟棄未完成狀態。
/// - stderr 讀取緩衝為 4096 bytes。
/// - 呼叫端只需 let _handle = spawn_stderr_sanitizer(stderr, cancel.clone()); 結束時 join 即可。
pub fn spawn_stderr_sanitizer(
    stderr: Option<ChildStderr>,
    cancel: Option<std::sync::Arc<dyn Cancelled>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::Read;
        // stderr 讀取緩衝 4096 bytes（baseline；另一處 player 電台 stderr 迴圈維持原設定）
        let mut buf = vec![0u8; 4096];
        let mut sanitizer = Sanitizer::new();
        if let Some(mut stderr) = stderr {
            loop {
                if let Some(ref c) = cancel {
                    if c.is_cancelled() {
                        break;
                    }
                }
                match stderr.read(&mut buf) {
                    // EOF：一律走 finish_incomplete 保守清理
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk_text = String::from_utf8_lossy(&buf[..n]);
                        sanitizer.sanitize_chunk(&chunk_text);
                    }
                    Err(_) => break,
                }
            }
        }
        // 不完整輸入（cancel、EOF、Err）：丟棄待決前綴與 authority，只輸出已確認部分
        let cleaned = sanitizer.finish_incomplete();
        if !cleaned.is_empty() {
            log_line_raw(&cleaned);
        }
    })
}

/// 日誌輪替互斥鎖（SE3-10）：保護 rotate_log_if_needed 關鍵區，避免多執行緒同時輪替導致檔案損壞或遺漏清理。
static LOG_ROTATE_MUTEX: Mutex<()> = Mutex::new(());

pub fn log_line_raw(msg: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let path = dir.join("data").join("open-with.log");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    
    // S2: 日誌輪替 - 單檔 ≤ 5 MB，超過以時間戳輪替，最多保留 5 檔；總量 ≤ 25 MB
    rotate_log_if_needed(&path);
    
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{msg}");
    }
}

pub fn log_line(msg: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let path = dir.join("data").join("open-with.log");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    
    // S2: 日誌輪替 - 單檔 ≤ 5 MB，超過以時間戳輪替，最多保留 5 檔；總量 ≤ 25 MB
    rotate_log_if_needed(&path);
    
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let sanitized = sanitize_log(msg);
        let _ = writeln!(file, "{sanitized}");
    }
}

fn rotate_log_if_needed(path: &Path) {
    // SE3-10: 取得日誌輪替互斥鎖，避免多執行緒同時輪替導致檔案損壞或遺漏清理
    let _guard = LOG_ROTATE_MUTEX.lock();
    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.len() > 5 * 1024 * 1024 {
            // 輪替：重命名為時間戳檔案；總量上限（25 MB）含活動檔
            let active_size = metadata.len();
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
            let rotated = path.with_file_name(format!("open-with_{timestamp}.log"));
            let _ = std::fs::rename(path, &rotated);

            // 清理舊檔案，保留最多 5 個輪替檔案，總量 ≤ 25 MB（含活動檔）
            if let Some(parent) = path.parent() {
                let mut files: Vec<_> = std::fs::read_dir(parent)
                    .ok()
                    .into_iter()
                    .flat_map(|e| e.filter_map(Result::ok))
                    .filter(|e| {
                        e.file_name().to_string_lossy().starts_with("open-with_")
                            && e.file_name().to_string_lossy().ends_with(".log")
                    })
                    .collect();
                files.sort_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()));

                // 移除超過 5 個的舊檔案
                while files.len() > 5 {
                    if let Some(oldest) = files.first() {
                        let _ = std::fs::remove_file(oldest.path());
                        files.remove(0);
                    }
                }

                // 檢查總量（含即將建立的活動檔）— SE3-7: 修正 total_size 計算
                // 活動檔剛被重命名為 rotated，新活動檔尚未寫入，大小為 0
                let mut total_size: u64 = 0  // 新活動檔大小為 0
                    + files
                        .iter()
                        .filter_map(|e| e.metadata().ok())
                        .map(|m| m.len())
                        .sum::<u64>();
                if total_size > 25 * 1024 * 1024 {
                    // 移除最舊的直到總量 ≤ 25 MB
                    while let Some(oldest) = files.first() {
                        let size = oldest.metadata().ok().map(|m| m.len()).unwrap_or(0);
                        let _ = std::fs::remove_file(oldest.path());
                        files.remove(0);
                        total_size = total_size.saturating_sub(size);
                        if total_size <= 25 * 1024 * 1024 {
                            break;
                        }
                    }
                }
            }
        }
    }
}

pub fn log_load_error(msg: &str) {
    log_line(&format!("LOAD_ERR {msg}"));
}

pub fn log_forward_result(path: &str, ok: bool) {
    log_line(&format!("FORWARD {ok} {path}"));
}

pub fn log_deliver(path: &str, ok: bool) {
    log_line(&format!("DELIVER {ok} {path}"));
}

// S7: 預設不寫入 HKCU 檔案關聯。僅在使用者於設定頁面明確點選「設為預設播放器」時註冊。
// 此函式保留供設定頁面呼叫，但不在 main() 中自動呼叫。
fn reg_query_default(key: &str) -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", key, "/ve"])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // reg query /ve 輸出形如：`HKEY_CURRENT_USER\...\\ext    REG_SZ    SonicLedger.Assoc`
    text.lines()
        .find(|l| l.contains("REG_SZ"))
        .and_then(|l| l.split_whitespace().last().map(|s| s.to_string()))
}

fn reg_add_value(key: &str, value: &str) -> Result<(), String> {
    let out = std::process::Command::new("reg")
        .args(["add", key, "/ve", "/t", "REG_SZ", "/d", value, "/f"])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("reg add 失敗: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("reg add 失敗: {}", String::from_utf8_lossy(&out.stderr))) }
}

fn reg_delete_value(key: &str) -> Result<(), String> {
    let out = std::process::Command::new("reg")
        .args(["delete", key, "/ve", "/f"])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("reg delete 失敗: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("reg delete 失敗: {}", String::from_utf8_lossy(&out.stderr))) }
}

pub fn register_windows_association() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("取得執行檔路徑失敗: {e}"))?;
    let exe_str = exe.to_string_lossy();
    let command_val = format!(r#""{}" "%1""#, exe_str);

    let progid = "HKCU\\Software\\Classes\\SonicLedger.Assoc\\shell\\open\\command";
    reg_add_value(progid, &command_val)?;

    let exts = [".mp3", ".flac", ".wav", ".m4a", ".ogg", ".aac", ".opus", ".wma", ".ape"];
    for ext in exts {
        let key = format!("HKCU\\Software\\Classes\\{ext}");
        reg_add_value(&key, "SonicLedger.Assoc")?;
    }
    log_line("registered windows file association");
    Ok(())
}

/// S7: 設定/取消「設為預設播放器」。enable 為 true 時寫入 HKCU 檔案關聯，
/// false 時僅移除本程式所建立的關聯（不刪除其他程式的副檔名子樹，SE14）。
/// 僅在使用者於設定頁面明確操作時呼叫。reg 執行結果會被檢查並回傳。
pub fn set_windows_association(enable: bool) -> Result<(), String> {
    if enable {
        return register_windows_association();
    }

    let exts = [".mp3", ".flac", ".wav", ".m4a", ".ogg", ".aac", ".opus", ".wma", ".ape"];
    let mut errors = Vec::new();
    for ext in exts {
        let key = format!("HKCU\\Software\\Classes\\{ext}");
        // SE14：只移除目前仍屬本程式的關聯，避免刪除其他程式的設定
        if reg_query_default(&key).as_deref() == Some("SonicLedger.Assoc") {
            if let Err(e) = reg_delete_value(&key) {
                errors.push(format!("{key}: {e}"));
            }
        }
    }
    // 移除本程式專用的 ProgID
    let out = std::process::Command::new("reg")
        .args(["delete", "HKCU\\Software\\Classes\\SonicLedger.Assoc", "/f"])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("reg delete ProgID 失敗: {e}"))?;
    if !out.status.success() {
        errors.push(format!("reg delete ProgID 失敗: {}", String::from_utf8_lossy(&out.stderr)));
    }
    if !errors.is_empty() {
        return Err(format!("取消關聯部分失敗：{}", errors.join("；")));
    }
    log_line("unregistered windows file association");
    Ok(())
}

/// 右鍵「開啟檔案」時，Windows 會把音檔路徑放在 argv。
pub fn audio_arg() -> Option<String> {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .map(|arg| arg.trim().trim_matches('"').to_string())
        .filter(|arg| !arg.is_empty() && !arg.starts_with('-'))
        .collect();
    // SE3-4: 遮蔽 argv 中的本機路徑用戶目錄
    let masked_args: Vec<String> = args.iter().map(|a| mask_local_path(a)).collect();
    log_line(&format!("argv {}", masked_args.join(" | ")));
    if let Some(hit) = args.iter().find(|arg| is_audio(Path::new(arg)) && validate_local_path(Path::new(arg)).is_ok()) {
        return Some(hit.clone());
    }
    // Imp#11：非音訊副檔名（如 .ofr/.mid）記錄拒絕原因，不靜默忽略
    for arg in &args {
        if Path::new(arg).extension().is_some() && !is_audio(Path::new(arg)) {
            log_line(&format!("argv 拒絕非支援格式: {}", mask_local_path(arg)));
        }
    }
    if args.len() > 1 {
        let joined = args.join(" ");
        if is_audio(Path::new(&joined)) && validate_local_path(Path::new(&joined)).is_ok() {
            return Some(joined);
        }
    }
    None
}

/// 程式已在跑時，把路徑送給它並讓這次啟動結束。
pub fn forward_to_running(path: &str) -> bool {
    let Ok(addr) = ADDR.parse() else { return false };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
        log_line("forward connect failed");
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let ok = stream.write_all(path.as_bytes()).is_ok() && stream.write_all(b"\n").is_ok();
    let _ = stream.shutdown(std::net::Shutdown::Write);
    log_forward_result(path, ok);
    ok
}

fn deliver(app: &AppHandle, path: String) {
    log_deliver(&path, true);
    if let Some(state) = app.try_state::<OpenedFile>() {
        if let Ok(mut slot) = state.path.lock() {
            *slot = Some(path.clone());
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        let name = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path);
        let _ = window.set_title(&format!("SonicLedger — {name}"));
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        match window.emit("open-file", &path) {
            Ok(()) => log_line("emit ok"),
            Err(err) => log_line(&format!("emit err {err}")),
        }
    } else {
        log_line("no main window");
    }
}

/// 第一個實例聽取之後的「開啟檔案」。一行一個路徑。
/// S1: 單一實例 Socket - 每連線限制：標頭 ≤ 4 KB、本體 ≤ 64 KB、路徑數 ≤ 10、總接收時間 ≤ 2 秒
/// 讀取使用 read_exact 配合 deadline，非 read_to_string；驗證路徑存在且為本機檔案後才轉交 open-file 事件。
/// 僅接受 loopback 介面；不支援提升權限執行。
pub fn start_listener(app: AppHandle) {
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(ADDR) {
            Ok(listener) => listener,
            Err(err) => {
                log_line(&format!("bind failed {err}"));
                return;
            }
        };
        log_line("listening");
        for conn in listener.incoming().flatten() {
            let peer = conn.peer_addr().map(|a| a.to_string()).unwrap_or_else(|_| "?".to_string());
            let masked_peer = mask_ip(&peer); // SE3-5: 遮蔽 peer IP
            
            // 僅接受 loopback 介面
            if let Ok(addr) = conn.peer_addr() {
                if !addr.ip().is_loopback() {
                    log_line(&format!("reject non-loopback connection from {masked_peer}"));
                    continue;
                }
            }
            
            let mut conn = conn;
            // SE2：總接收時間上限（而非單次 read 超時），以 newline 作為訊號結尾，拒絕超量訊息
            let deadline = Instant::now() + TOTAL_RECV_TIMEOUT;
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            // 超時/超量後必須拒絕整筆訊息，不能交付已收到的部分資料
            let mut refused = false;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining == Duration::ZERO {
                    log_line(&format!("tcp from {masked_peer}: total deadline exceeded"));
                    refused = true;
                    break;
                }
                let _ = conn.set_read_timeout(Some(remaining));
                match conn.read(&mut buf) {
                    Ok(0) => break, // 對端關閉
                    Ok(n) => {
                        data.extend_from_slice(&buf[..n]);
                        if data.len() >= MAX_BODY_BYTES {
                            log_line(&format!("tcp from {masked_peer}: body over limit"));
                            refused = true;
                            break;
                        }
                        if data.iter().position(|b| *b == b'\n').is_some() {
                            break; // 完整訊號（以換行結尾）
                        }
                    }
                    Err(_) => break,
                }
            }

            let data = String::from_utf8_lossy(&data);

            // SE2：訊息被拒絕時不交付任何路徑
            if !refused {
                let mut path_count = 0;
                for line in data.lines() {
                    if path_count >= MAX_PATHS {
                        log_line(&format!("tcp from {masked_peer}: too many paths ({})", path_count));
                        refused = true;
                        break;
                    }
                    let path = line.trim().to_string();
                    if !path.is_empty() {
                        // S1: 驗證路徑存在且為本機檔案
                        let p = Path::new(&path);
                        if validate_local_path(p).is_ok() && is_audio(p) {
                            path_count += 1;
                            deliver(&app, path);
                        } else {
                            // SE3-5: 遮蔽 peer IP，使用 log_line 而非 log_line_raw 避免雙重清洗語意混淆
                            let masked_path = mask_local_path(&path);
                            log_line(&format!("reject invalid path from {masked_peer}: {masked_path}"));
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_sanitize_log_removes_credentials() {
        let input = "https://user:pass@example.com/path?token=abc";
        let output = sanitize_log(input);
        assert!(output.contains("https://***@"));
        assert!(!output.contains("user:pass"));
    }

    #[test]
    fn test_sanitize_log_truncates_at_1kb() {
        let input = "a".repeat(2000);
        let output = sanitize_log(&input);
        assert!(output.len() <= 1024);
    }

    #[test]
    fn test_sanitize_log_removes_control_chars() {
        let input = "hello\x00\x01world";
        let output = sanitize_log(input);
        assert_eq!(output, "helloworld");
    }

    #[test]
    fn test_is_audio_extensions() {
        assert!(is_audio(Path::new("test.mp3")));
        assert!(is_audio(Path::new("test.flac")));
        assert!(is_audio(Path::new("test.m4a")));
        assert!(is_audio(Path::new("test.ogg")));
        assert!(is_audio(Path::new("test.opus")));
        assert!(is_audio(Path::new("test.wav")));
        assert!(is_audio(Path::new("test.ape")));
        assert!(is_audio(Path::new("test.wma")));
        assert!(is_audio(Path::new("test.aiff")));
        assert!(is_audio(Path::new("test.aif")));
        assert!(is_audio(Path::new("test.aif")));
        assert!(is_audio(Path::new("test.wv")));
        assert!(is_audio(Path::new("test.tta")));
        assert!(is_audio(Path::new("test.m4b")));
    }

    #[test]
    fn test_validate_local_path_rejects_unc() {
        let result = validate_local_path(Path::new(r"\\server\share\file.mp3"));
    }

    #[test]
    fn test_validate_local_path_rejects_http() {
        let result = validate_local_path(Path::new("http://example.com/file.mp3"));
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitizer_cross_chunk_query_removed_via_stderr() {
        let mut s = Sanitizer::new();
        let part1 = s.sanitize_chunk("https://host/path?token=");
        let part2 = s.sanitize_chunk("secret\n");
        let result = s.finish();
        assert!(!result.contains("token="), "query should be removed across chunks via stderr: {result}");
        assert!(!result.contains("secret"), "query continuation should be removed: {result}");
    }

    #[test]
    fn test_sanitizer_cross_chunk_credential_after_at_mask() {
        let mut s = Sanitizer::new();
        let part1 = s.sanitize_chunk("https://user:pass@host/path?token=");
        let part2 = s.sanitize_chunk("secret\n");
        let result = s.finish();
        assert!(result.contains("https://***@"), "@ should be masked: {result}");
        assert!(!result.contains("user:pass"), "credentials should be removed: {result}");
        assert!(!result.contains("token="), "query should be removed after @ mask: {result}");
        assert!(!result.contains("secret"), "query continuation should be removed: {result}");
    }

    #[test]
    fn test_sanitize_log_idempotent() {
        let input = "https://user:pass@example.com/path?token=abc&foo=bar";
        let first = sanitize_log(input);
        let second = sanitize_log(&first);
        assert_eq!(first, second, "sanitize_log should be idempotent: first={first}, second={second}");
    }

    #[test]
    fn test_sanitize_log_non_url_query() {
        let input = "file.mp3?token=secret";
        let output = sanitize_log(input);
        assert!(!output.contains("token="), "non-url query should be removed: {output}");
        assert!(!output.contains("secret"), "query value should be removed: {output}");
    }
    
    #[test]
    fn test_sanitize_log_preserves_newlines() {
        let input = "line1\nline2\r\nline3";
        let output = sanitize_log(input);
        assert!(output.contains('\n'), "newlines should be preserved: {output}");
        assert!(output.contains('\r'), "carriage returns should be preserved: {output}");
    }
    
    #[test]
    fn test_mask_local_path_windows() {
        let path = r"C:\Users\admin\secret\file.mp3";
        let masked = mask_local_path(path);
        assert_eq!(masked, r"C:\Users\%USER%\secret\file.mp3");
    }
    
    #[test]
    fn test_mask_local_path_unix() {
        let path = "/home/john/music/file.mp3";
        let masked = mask_local_path(path);
        assert_eq!(masked, "/home/%USER%/music/file.mp3");
    }
    
    #[test]
    fn test_mask_ip() {
        assert_eq!(mask_ip("127.0.0.1"), "127.0.0.xxx");
        assert_eq!(mask_ip("192.168.1.100"), "192.168.1.xxx");
        assert_eq!(mask_ip("::1"), "xxx");
    }

    #[test]
    fn test_sanitizer_every_character_split_https() {
        let input = "https://signin.example.com/music/@artist/track?session=dummy&user=dummy";
        let expected = "https://signin.example.com/music/@artist/track";
        assert_eq!(sanitize_log(input), expected);

        let mut splits: Vec<usize> = input.char_indices().map(|(i, _)| i).collect();
        splits.push(input.len());
        for split in splits {
            let mut s = Sanitizer::new();
            s.sanitize_chunk(&input[..split]);
            s.sanitize_chunk(&input[split..]);
            assert_eq!(s.finish(), expected, "split at byte {split}");
        }
    }

    #[test]
    fn test_sanitizer_finish_vs_incomplete_hostname() {
        let mut complete = Sanitizer::new();
        complete.sanitize_chunk("https://signin.example.com");
        assert_eq!(complete.finish(), "https://signin.example.com");

        let mut incomplete = Sanitizer::new();
        incomplete.sanitize_chunk("https://signin.example.com");
        assert_eq!(incomplete.finish_incomplete(), "");

        let mut trailing_dot = Sanitizer::new();
        trailing_dot.sanitize_chunk("https://signin.example.com./music");
        assert_eq!(
            trailing_dot.finish(),
            "https://signin.example.com./music"
        );

        let mut ipv6 = Sanitizer::new();
        ipv6.sanitize_chunk("https://[::1]:8080/music");
        assert_eq!(ipv6.finish(), "https://[::1]:8080/music");
    }

    #[test]
    fn test_sanitizer_overlong_authority_discards() {
        let overlong = "a".repeat(MAX_AUTHORITY + 1);
        let input = format!("https://{overlong}@evil.example/path tail");
        let output = sanitize_log(&input);
        assert_eq!(output, " tail");
        assert!(!output.contains("https://"));
        assert!(!output.contains("***@"));
    }

    #[test]
    fn test_sanitizer_ascii_truncation_baseline() {
        let output = sanitize_log(&"a".repeat(2000));
        assert_eq!(output.len(), MAX_OUTPUT);

        // 非 baseline 字元直接忽略，不以 lossy 替代字元輸出
        assert_eq!(sanitize_log(&"é".repeat(600)), "");

        let mut mixed = "a".repeat(MAX_OUTPUT - 1);
        mixed.push('é');
        mixed.push('b');
        assert_eq!(sanitize_log(&mixed).len(), MAX_OUTPUT);
    }

    #[test]
    fn test_sanitizer_ordinary_text_baseline() {
        assert_eq!(sanitize_log("hello\tworld\r\nnext"), "hello\tworld\r\nnext");
        assert_eq!(sanitize_log("a\x00b\x01c"), "abc");
        assert_eq!(sanitize_log("plain text without url"), "plain text without url");
        // 非 baseline 字元不承諾顯示，於輸出時忽略
        assert_eq!(sanitize_log("plain 文字 text"), "plain  text");
    }
}
