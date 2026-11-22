use crate::db::db_path;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

static CONTROLS: Mutex<Option<MediaControls>> = Mutex::new(None);
static SMTC_ERROR: Mutex<Option<String>> = Mutex::new(None);

pub fn install(app: &AppHandle) {
    if let Some(dir) = db_path().parent() {
        let _ = std::fs::write(dir.join("smtc.txt"), "");
    }
    if let Err(err) = install_tray(app) {
        remember_error(&format!("系統匣: {err}"));
    } else {
        remember_error("系統匣: ok");
    }
    match install_smtc(app) {
        Ok(()) => remember_error("媒體鍵: ok"),
        Err(err) => remember_error(&err),
    }
}

fn remember_error(text: &str) {
    if let Ok(mut slot) = SMTC_ERROR.lock() {
        *slot = Some(text.to_string());
    }
    if let Some(dir) = db_path().parent() {
        let path = dir.join("smtc.txt");
        let prev = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::write(path, format!("{prev}{text}\n"));
    }
}

fn install_tray(app: &AppHandle) -> Result<(), String> {
    let play = MenuItem::with_id(app, "toggle", "播放／暫停", true, None::<&str>).map_err(|e| e.to_string())?;
    let prev = MenuItem::with_id(app, "prev", "上一首", true, None::<&str>).map_err(|e| e.to_string())?;
    let next = MenuItem::with_id(app, "next", "下一首", true, None::<&str>).map_err(|e| e.to_string())?;
    let show = MenuItem::with_id(app, "show", "顯示視窗", true, None::<&str>).map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(app, "quit", "結束", true, None::<&str>).map_err(|e| e.to_string())?;
    let menu = Menu::with_items(app, &[&play, &prev, &next, &show, &quit]).map_err(|e| e.to_string())?;
    let icon = TrayIconBuilder::with_id("sonicledger")
        .icon(tray_icon())
        .tooltip("SonicLedger")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => emit(app, "toggle"),
            "prev" => emit(app, "prev"),
            "next" => emit(app, "next"),
            "show" => show_window(app),
            "quit" => quit_window(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_window(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|e| e.to_string())?;
    std::mem::forget(icon);
    Ok(())
}

fn install_smtc(app: &AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("找不到主視窗")?;
    let hwnd = window.hwnd().map_err(|e| format!("取不到視窗代號: {e}"))?;
    let config = PlatformConfig {
        dbus_name: "sonicledger",
        display_name: "SonicLedger",
        hwnd: Some(hwnd.0 as *mut std::ffi::c_void),
    };
    let mut controls = MediaControls::new(config).map_err(|e| format!("媒體鍵接不上: {e:?}"))?;
    let handle = app.clone();
    controls
        .attach(move |event| {
            let payload = match event {
                MediaControlEvent::Play => "play",
                MediaControlEvent::Pause => "pause",
                MediaControlEvent::Toggle => "toggle",
                MediaControlEvent::Next => "next",
                MediaControlEvent::Previous => "prev",
                _ => return,
            };
            emit(&handle, payload);
        })
        .map_err(|e| format!("媒體鍵事件接不上: {e:?}"))?;
    if let Ok(mut slot) = CONTROLS.lock() {
        *slot = Some(controls);
    }
    Ok(())
}

fn emit(app: &AppHandle, payload: &str) {
    let _ = app.emit("transport", payload);
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn quit_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.close();
    } else {
        app.exit(0);
    }
}

fn tray_icon() -> Image<'static> {
    let (w, h) = (32u32, 32u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let edge = x < 2 || y < 2 || x + 2 >= w || y + 2 >= h;
            if edge {
                rgba[i] = 0xe2;
                rgba[i + 1] = 0x3b;
                rgba[i + 2] = 0x2f;
            } else {
                rgba[i] = 0x12;
                rgba[i + 1] = 0x11;
                rgba[i + 2] = 0x0f;
            }
            rgba[i + 3] = 255;
        }
    }
    Image::new_owned(rgba, w, h)
}

pub fn update_smtc(title: &str, artist: &str, playing: bool) -> Result<(), String> {
    let mut slot = CONTROLS.lock().map_err(|e| e.to_string())?;
    let Some(controls) = slot.as_mut() else {
        let why = SMTC_ERROR.lock().ok().and_then(|s| s.clone()).unwrap_or_else(|| "媒體鍵沒接上".into());
        return Err(why);
    };
    controls
        .set_metadata(MediaMetadata {
            title: Some(title),
            artist: Some(artist),
            ..Default::default()
        })
        .map_err(|e| format!("寫不進音量浮層: {e:?}"))?;
    let playback = if playing {
        MediaPlayback::Playing { progress: None }
    } else {
        MediaPlayback::Paused { progress: None }
    };
    controls.set_playback(playback).map_err(|e| format!("寫不進播放狀態: {e:?}"))
}
