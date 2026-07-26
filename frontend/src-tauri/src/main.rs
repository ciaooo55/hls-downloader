#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Debug, Deserialize)]
struct LocalConfig {
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    token: String,
}

fn default_port() -> u16 { 8765 }

impl Default for LocalConfig {
    fn default() -> Self { Self { port: default_port(), token: String::new() } }
}

struct CoreRuntime {
    child: Mutex<Option<Child>>,
    config: LocalConfig,
}

struct DesktopPaths {
    root: PathBuf,
}

#[tauri::command]
fn get_app_root(paths: tauri::State<'_, DesktopPaths>) -> String {
    paths.root.to_string_lossy().into_owned()
}

#[tauri::command]
fn get_desktop_info(paths: tauri::State<'_, DesktopPaths>) -> serde_json::Value {
    let installed = paths.root.join("Uninstall.exe").is_file();
    serde_json::json!({
        "ok": true,
        "installed": installed,
        "mode": if installed { "installed" } else { "portable" },
        "shell": "tauri",
        "desktop_version": env!("CARGO_PKG_VERSION"),
    })
}

#[tauri::command]
fn get_core_config(runtime: tauri::State<'_, Arc<CoreRuntime>>) -> serde_json::Value {
    serde_json::json!({ "port": runtime.config.port, "credential": runtime.config.token })
}

#[tauri::command]
fn begin_uninstall(paths: tauri::State<'_, DesktopPaths>, app: tauri::AppHandle) -> serde_json::Value {
    let uninstaller = paths.root.join("Uninstall.exe");
    if !uninstaller.is_file() {
        return serde_json::json!({ "ok": false, "error": "当前版本无需卸载" });
    }
    match Command::new(&uninstaller).arg("_?=").arg(&paths.root).spawn() {
        Ok(_) => {
            app.exit(0);
            serde_json::json!({ "ok": true })
        }
        Err(error) => serde_json::json!({ "ok": false, "error": error.to_string() }),
    }
}

fn app_root() -> PathBuf {
    let working = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let executable = std::env::current_exe().ok().and_then(|path| path.parent().map(Path::to_path_buf));
    let mut candidates = vec![working.clone()];
    if let Some(parent) = working.parent() { candidates.push(parent.to_path_buf()); }
    if let Some(path) = executable {
        candidates.push(path.clone());
        if let Some(parent) = path.parent() { candidates.push(parent.to_path_buf()); }
    }
    candidates.into_iter().find(|path| {
        path.join("HLSDownloaderCore.exe").is_file() || path.join("backend").is_dir()
    }).unwrap_or(working)
}

fn load_config(root: &Path) -> LocalConfig {
    let mut candidates = Vec::new();
    if root.join("portable").is_file() {
        candidates.push(root.join("config.json"));
    } else if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("HLS Downloader").join("config.json"));
        candidates.push(root.join("config.json"));
    } else {
        candidates.push(root.join("config.json"));
    }
    candidates.into_iter().find_map(|path| {
        std::fs::read_to_string(path).ok().and_then(|value| serde_json::from_str(&value).ok())
    }).unwrap_or_default()
}

fn wait_for_runtime_config(root: &Path, port: u16) -> LocalConfig {
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        let config = load_config(root);
        if config.port == port && !config.token.is_empty() { return config; }
        std::thread::sleep(Duration::from_millis(80));
    }
    load_config(root)
}

fn core_alive(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&address, Duration::from_millis(180)).is_ok()
}

fn start_core(root: &Path, config: &LocalConfig) -> Result<Option<Child>, String> {
    if core_alive(config.port) { return Ok(None); }
    let packaged = root.join("HLSDownloaderCore.exe");
    let mut command = if packaged.is_file() {
        Command::new(packaged)
    } else {
        let python = std::env::var("PYTHON").unwrap_or_else(|_| "python".to_string());
        let mut value = Command::new(python);
        value.arg(root.join("backend").join("run_core.py"));
        value
    };
    command.current_dir(root);
    command.stdout(Stdio::from(File::create(root.join("core.log")).map_err(|e| e.to_string())?));
    command.stderr(Stdio::from(File::create(root.join("core-error.log")).map_err(|e| e.to_string())?));
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let child = command.spawn().map_err(|e| format!("Unable to start download core: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if core_alive(config.port) { return Ok(Some(child)); }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!("Download core did not open port {}", config.port))
}

fn request_core_shutdown(config: &LocalConfig) {
    if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", config.port)) {
        let request = format!(
            "POST /api/desktop/core/shutdown HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Token: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            config.port, config.token
        );
        let _ = stream.write_all(request.as_bytes());
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let mut response = [0_u8; 128];
        let _ = stream.read(&mut response);
    }
}

fn import_torrent_path(config: &LocalConfig, path: &str) {
    if !path.to_ascii_lowercase().ends_with(".torrent") { return; }
    let body = serde_json::json!({ "path": path }).to_string();
    if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", config.port)) {
        let request = format!(
            "POST /api/tasks/torrent-path HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            config.port, config.token, body.as_bytes().len(), body
        );
        let _ = stream.write_all(request.as_bytes());
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Return the clipboard text when it is a link the downloader can handle.
///
/// Deliberately conservative: one line, an http(s)/magnet scheme, and a
/// path ending in a known media/archive extension. Ordinary copied prose
/// or web-page links never trigger a suggestion.
fn downloadable_clipboard_text(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() || text.len() > 2048 || text.contains(char::is_whitespace) {
        return None;
    }
    let lowered = text.to_ascii_lowercase();
    if lowered.starts_with("magnet:?xt=") {
        return Some(text.to_string());
    }
    if !lowered.starts_with("http://") && !lowered.starts_with("https://") {
        return None;
    }
    let path = lowered.split(['?', '#']).next().unwrap_or("");
    const EXTENSIONS: [&str; 24] = [
        ".m3u8", ".mpd", ".mp4", ".mkv", ".mov", ".webm", ".flv", ".m4v", ".avi",
        ".mp3", ".m4a", ".flac", ".wav", ".zip", ".7z", ".rar", ".gz", ".iso",
        ".exe", ".msi", ".apk", ".torrent", ".pdf", ".dmg",
    ];
    if EXTENSIONS.iter().any(|extension| path.ends_with(extension)) {
        return Some(text.to_string());
    }
    None
}

fn watch_clipboard(handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let Ok(mut clipboard) = arboard::Clipboard::new() else { return };
        // Seed with the current content so a link copied before launch does
        // not immediately pop a suggestion.
        let mut last = clipboard.get_text().unwrap_or_default();
        loop {
            std::thread::sleep(Duration::from_millis(1200));
            let text = match clipboard.get_text() {
                Ok(value) => value,
                Err(_) => {
                    // Non-text content (or a busy clipboard). The frontend
                    // dedupes repeated URLs, so resetting is safe.
                    last.clear();
                    continue;
                }
            };
            if text == last {
                continue;
            }
            last = text.clone();
            if let Some(url) = downloadable_clipboard_text(&text) {
                let _ = handle.emit_to("main", "clipboard-url", url);
            }
        }
    });
}

fn main() {
    let root = app_root();
    let startup_config = load_config(&root);
    let child = start_core(&root, &startup_config).unwrap_or_else(|reason| {
        eprintln!("{reason}");
        None
    });
    let config = wait_for_runtime_config(&root, startup_config.port);
    let runtime = Arc::new(CoreRuntime { child: Mutex::new(child), config });
    let exit_runtime = Arc::clone(&runtime);
    let background = std::env::args().any(|arg| arg == "--background" || arg == "--native-host");

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            show_main(app);
            let runtime = app.state::<Arc<CoreRuntime>>();
            for arg in args.iter().skip(1) { import_torrent_path(&runtime.config, arg); }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .manage(Arc::clone(&runtime))
        .manage(DesktopPaths { root: root.clone() })
        .invoke_handler(tauri::generate_handler![get_app_root, get_desktop_info, get_core_config, begin_uninstall])
        .setup(move |app| {
            let startup_runtime = app.state::<Arc<CoreRuntime>>();
            for arg in std::env::args().skip(1) { import_torrent_path(&startup_runtime.config, &arg); }
            let open = MenuItem::with_id(app, "open", "打开下载器", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main").menu(&menu).show_menu_on_left_click(false);
            if let Some(icon) = app.default_window_icon() { tray = tray.icon(icon.clone()); }
            tray.on_menu_event(|app, event| match event.id.as_ref() {
                "open" => show_main(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                    show_main(tray.app_handle());
                }
            })
            .build(app)?;
            watch_clipboard(app.handle().clone());
            if !background { show_main(app.handle()); }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build HLS Downloader desktop shell");

    app.run(move |_handle, event| {
        if let tauri::RunEvent::Exit = event {
            request_core_shutdown(&exit_runtime.config);
            std::thread::sleep(Duration::from_millis(400));
            if let Ok(mut slot) = exit_runtime.child.lock() {
                if let Some(child) = slot.as_mut() {
                    if child.try_wait().ok().flatten().is_none() { let _ = child.kill(); }
                }
            }
        }
    });
}

#[cfg(test)]
mod clipboard_tests {
    use super::downloadable_clipboard_text;

    #[test]
    fn accepts_media_archive_and_magnet_links() {
        for text in [
            "https://cdn.example.com/video/master.m3u8",
            "https://cdn.example.com/movie.mp4?token=abc",
            "http://mirror.example.com/tool.zip",
            "magnet:?xt=urn:btih:0123456789abcdef",
            "  https://cdn.example.com/show.mkv  ",
        ] {
            assert!(downloadable_clipboard_text(text).is_some(), "{text}");
        }
    }

    #[test]
    fn rejects_prose_pages_and_multiline_text() {
        for text in [
            "",
            "普通复制的一段文字",
            "https://example.com/article/how-to-download",
            "https://example.com/watch?v=abc123",
            "https://example.com/a.mp4\nhttps://example.com/b.mp4",
            "ftp://example.com/file.zip",
        ] {
            assert!(downloadable_clipboard_text(text).is_none(), "{text:?}");
        }
    }
}
