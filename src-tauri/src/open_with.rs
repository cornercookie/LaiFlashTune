use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;
use std::os::windows::process::CommandExt;
use tauri::{AppHandle, Emitter, Manager};

use crate::models::OpenedFile;

const ADDR: &str = "127.0.0.1:47321";

fn is_audio(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some(
                "mp3" | "flac" | "m4a" | "mp4" | "aac" | "ogg" | "opus" | "wav" | "ape" | "wma"
                    | "aiff" | "aif" | "wv" | "tta"
            )
        )
}

pub fn log_line(msg: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let path = dir.join("data").join("open-with.log");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{msg}");
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

pub fn register_windows_association() {
    if let Ok(exe) = std::env::current_exe() {
        let exe_str = exe.to_string_lossy();
        let command_val = format!("\"{}\" \"%1\"", exe_str);

        let _ = std::process::Command::new("reg")
            .args(["add", "HKCU\\Software\\Classes\\SonicLedger.Assoc\\shell\\open\\command", "/ve", "/t", "REG_SZ", "/d", &command_val, "/f"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();

        for ext in &[".mp3", ".flac", ".wav", ".m4a", ".ogg", ".aac", ".opus", ".wma", ".ape"] {
            let key = format!("HKCU\\Software\\Classes\\{ext}");
            let _ = std::process::Command::new("reg")
                .args(["add", &key, "/ve", "/t", "REG_SZ", "/d", "SonicLedger.Assoc", "/f"])
                .creation_flags(0x0800_0000)
                .output();
        }
        log_line("registered windows file association");
    }
}



/// 右鍵「開啟檔案」時，Windows 會把音檔路徑放在 argv。
pub fn audio_arg() -> Option<String> {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .map(|arg| arg.trim().trim_matches('"').to_string())
        .filter(|arg| !arg.is_empty() && !arg.starts_with('-'))
        .collect();
    log_line(&format!("argv {}", args.join(" | ")));
    if let Some(hit) = args.iter().find(|arg| is_audio(Path::new(arg))) {
        return Some(hit.clone());
    }
    if args.len() > 1 {
        let joined = args.join(" ");
        if is_audio(Path::new(&joined)) {
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
            let mut conn = conn;
            let _ = conn.set_read_timeout(Some(Duration::from_secs(3)));
            let mut buf = String::new();
            match conn.read_to_string(&mut buf) {
                Ok(_) => log_line(&format!("tcp from {peer}: {buf}")),
                Err(err) if buf.is_empty() => {
                    log_line(&format!("read err {err}"));
                    continue;
                }
                Err(err) => log_line(&format!("read partial {err}")),
            }
            for line in buf.lines() {
                let path = line.trim().to_string();
                if !path.is_empty() {
                    deliver(&app, path);
                }
            }
        }
    });
}
