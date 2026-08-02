#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Clone, Debug, Deserialize)]
struct LocalConfig {
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    token: String,
}

fn default_port() -> u16 {
    8765
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            token: String::new(),
        }
    }
}

struct CoreRuntime {
    child: Mutex<Option<Child>>,
    config: Mutex<LocalConfig>,
    root: PathBuf,
    primary: AtomicBool,
    stopping: AtomicBool,
}

struct DesktopPaths {
    root: PathBuf,
}

const CORE_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const CORE_LOG_BACKUPS: usize = 3;

fn log_backup_path(path: &Path, index: usize) -> PathBuf {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "core.log".to_string());
    path.with_file_name(format!("{name}.{index}"))
}

fn rotate_core_log(path: &Path) -> std::io::Result<()> {
    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if size <= CORE_LOG_MAX_BYTES {
        return Ok(());
    }
    let oldest = log_backup_path(path, CORE_LOG_BACKUPS);
    if oldest.exists() {
        fs::remove_file(oldest)?;
    }
    for index in (1..CORE_LOG_BACKUPS).rev() {
        let source = log_backup_path(path, index);
        if source.exists() {
            fs::rename(source, log_backup_path(path, index + 1))?;
        }
    }
    fs::rename(path, log_backup_path(path, 1))
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
fn get_core_config(
    runtime: tauri::State<'_, Arc<CoreRuntime>>,
) -> Result<serde_json::Value, String> {
    let config = runtime
        .config
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let credential = request_scoped_credential(&config, "/api/desktop/credential")?;
    Ok(serde_json::json!({ "port": config.port, "credential": credential }))
}

#[tauri::command]
fn open_browser_extension_installer(paths: tauri::State<'_, DesktopPaths>) -> serde_json::Value {
    let extension = paths.root.join("browser-extension").join("chrome");
    if !extension.join("manifest.json").is_file() {
        return serde_json::json!({ "ok": false, "error": "安装包中缺少浏览器扩展，请重新安装最新版" });
    }
    let program_files = std::env::var_os("PROGRAMFILES").map(PathBuf::from);
    let program_files_x86 = std::env::var_os("PROGRAMFILES(X86)").map(PathBuf::from);
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let candidates = [
        program_files_x86.as_ref().map(|root| {
            root.join("Microsoft")
                .join("Edge")
                .join("Application")
                .join("msedge.exe")
        }),
        program_files.as_ref().map(|root| {
            root.join("Microsoft")
                .join("Edge")
                .join("Application")
                .join("msedge.exe")
        }),
        program_files.as_ref().map(|root| {
            root.join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe")
        }),
        program_files_x86.as_ref().map(|root| {
            root.join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe")
        }),
        local_app_data.as_ref().map(|root| {
            root.join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe")
        }),
    ];
    let mut browser_opened = false;
    for browser in candidates.into_iter().flatten() {
        if !browser.is_file() {
            continue;
        }
        let internal_url =
            if browser.file_name().and_then(|name| name.to_str()) == Some("msedge.exe") {
                "edge://extensions"
            } else {
                "chrome://extensions"
            };
        browser_opened = Command::new(browser).arg(internal_url).spawn().is_ok();
        if browser_opened {
            break;
        }
    }
    match Command::new("explorer.exe").arg(&extension).spawn() {
        Ok(_) => {
            serde_json::json!({ "ok": true, "path": extension, "browser_opened": browser_opened })
        }
        Err(error) => serde_json::json!({ "ok": false, "error": error.to_string() }),
    }
}

#[tauri::command]
fn begin_uninstall(
    paths: tauri::State<'_, DesktopPaths>,
    app: tauri::AppHandle,
) -> serde_json::Value {
    let uninstaller = paths.root.join("Uninstall.exe");
    if !uninstaller.is_file() {
        return serde_json::json!({ "ok": false, "error": "当前版本无需卸载" });
    }
    match Command::new(&uninstaller)
        .arg(uninstall_in_place_argument(&paths.root))
        .spawn()
    {
        Ok(_) => {
            app.exit(0);
            serde_json::json!({ "ok": true })
        }
        Err(error) => serde_json::json!({ "ok": false, "error": error.to_string() }),
    }
}

fn uninstall_in_place_argument(root: &Path) -> String {
    format!("_?={}", root.display())
}

fn app_root() -> PathBuf {
    let working = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let mut candidates = vec![working.clone()];
    if let Some(parent) = working.parent() {
        candidates.push(parent.to_path_buf());
    }
    if let Some(path) = executable {
        candidates.push(path.clone());
        if let Some(parent) = path.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    candidates
        .into_iter()
        .find(|path| path.join("HLSDownloaderCore.exe").is_file() || path.join("backend").is_dir())
        .unwrap_or(working)
}

fn load_config(root: &Path) -> LocalConfig {
    let mut candidates = Vec::new();
    if root.join("portable").is_file() {
        candidates.push(root.join("config.json"));
    } else if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("HLS Downloader")
                .join("config.json"),
        );
        candidates.push(root.join("config.json"));
    } else {
        candidates.push(root.join("config.json"));
    }
    candidates
        .into_iter()
        .find_map(|path| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|value| serde_json::from_str(&value).ok())
        })
        .unwrap_or_default()
}

fn wait_for_runtime_config(root: &Path, port: u16) -> LocalConfig {
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        let config = load_config(root);
        if config.port == port && !config.token.is_empty() {
            return config;
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    load_config(root)
}

fn port_open(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&address, Duration::from_millis(180)).is_ok()
}

fn core_alive(config: &LocalConfig) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], config.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(350)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(700)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(350)));
    if config.token.contains(['\r', '\n']) {
        return false;
    }
    let request = format!(
        "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Token: {}\r\nConnection: close\r\n\r\n",
        config.port, config.token
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return false;
    }
    response.starts_with("HTTP/1.1 200")
        && response.contains("\"app_id\":\"com.ciaooo55.hls-downloader\"")
        && response.contains("\"protocol_version\":3")
        && response.contains("\"authenticated\":true")
}

fn request_scoped_credential(config: &LocalConfig, path: &str) -> Result<String, String> {
    if config.token.is_empty() || config.token.contains(['\r', '\n']) {
        return Err("Core control credential is unavailable".to_string());
    }
    let mut stream = TcpStream::connect(("127.0.0.1", config.port))
        .map_err(|_| "Unable to connect to download core".to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Token: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        config.port, config.token
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|_| "Unable to request a desktop session".to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|_| "Unable to read the desktop session response".to_string())?;
    if !response.starts_with("HTTP/1.1 200") {
        return Err("Download core rejected the desktop session".to_string());
    }
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, value)| value)
        .ok_or_else(|| "Desktop session response is malformed".to_string())?;
    let payload: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| "Desktop session response is invalid".to_string())?;
    payload
        .get("credential")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Desktop session credential is missing".to_string())
}

fn start_core(root: &Path, config: &LocalConfig) -> Result<Option<Child>, String> {
    if core_alive(config) {
        return Ok(None);
    }
    if port_open(config.port) {
        return Err(format!(
            "Port {} is occupied by another process or an incompatible HLS Downloader Core",
            config.port
        ));
    }
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
    let stdout_path = root.join("core.log");
    let stderr_path = root.join("core-error.log");
    // Rotation is best-effort: inability to rename a diagnostic log must not
    // prevent downloads from starting, while normal launches keep a bounded
    // 20 MiB maximum across the active file and three backups.
    let _ = rotate_core_log(&stdout_path);
    let _ = rotate_core_log(&stderr_path);
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(stdout_path)
        .map_err(|e| e.to_string())?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_path)
        .map_err(|e| e.to_string())?;
    command.stdout(Stdio::from(stdout));
    command.stderr(Stdio::from(stderr));
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let child = command
        .spawn()
        .map_err(|e| format!("Unable to start download core: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let refreshed = load_config(root);
        if core_alive(&refreshed) {
            return Ok(Some(child));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!("Download core did not open port {}", config.port))
}

fn runtime_config(runtime: &CoreRuntime) -> LocalConfig {
    runtime
        .config
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default()
}

fn ensure_core(runtime: &CoreRuntime) -> Result<(), String> {
    let current = runtime_config(runtime);
    if core_alive(&current) {
        return Ok(());
    }

    let mut slot = runtime
        .child
        .lock()
        .map_err(|_| "Download core lock is poisoned".to_string())?;
    if let Some(child) = slot.as_mut() {
        match child.try_wait() {
            Ok(Some(_)) => {
                *slot = None;
            }
            Ok(None) => {
                // A live child can briefly be between process creation and bind.
                for _ in 0..12 {
                    if core_alive(&current) {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
                *slot = None;
            }
            Err(_) => {
                *slot = None;
            }
        }
    }

    let child = start_core(&runtime.root, &current)?;
    *slot = child;
    drop(slot);
    let refreshed = wait_for_runtime_config(&runtime.root, current.port);
    if let Ok(mut config) = runtime.config.lock() {
        *config = refreshed;
    }
    Ok(())
}

fn supervise_core(runtime: Arc<CoreRuntime>) {
    std::thread::spawn(move || {
        let mut failures = 0_u8;
        while !runtime.stopping.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(2));
            if runtime.stopping.load(Ordering::Relaxed) {
                break;
            }
            let config = runtime_config(&runtime);
            if core_alive(&config) {
                failures = 0;
                continue;
            }
            failures = failures.saturating_add(1);
            if failures >= 3 {
                let _ = ensure_core(&runtime);
                failures = 0;
            }
        }
    });
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
    if !path.to_ascii_lowercase().ends_with(".torrent") {
        return;
    }
    let body = serde_json::json!({ "path": path }).to_string();
    if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", config.port)) {
        let request = format!(
            "POST /api/tasks/torrent-path HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            config.port, config.token, body.len(), body
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

fn background_launch(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--background" || arg == "--native-host")
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
        ".m3u8", ".mpd", ".mp4", ".mkv", ".mov", ".webm", ".flv", ".m4v", ".avi", ".mp3", ".m4a",
        ".flac", ".wav", ".zip", ".7z", ".rar", ".gz", ".iso", ".exe", ".msi", ".apk", ".torrent",
        ".pdf", ".dmg",
    ];
    if EXTENSIONS.iter().any(|extension| path.ends_with(extension)) {
        return Some(text.to_string());
    }
    None
}

fn watch_clipboard(handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            return;
        };
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
            } else if text.lines().count() > 1 {
                // A copied list of links goes to batch import in one step.
                let urls: Vec<String> = text
                    .lines()
                    .filter_map(downloadable_clipboard_text)
                    .collect();
                if urls.len() >= 2 {
                    let _ = handle.emit_to("main", "clipboard-url-batch", urls.join("\n"));
                }
            }
        }
    });
}

fn main() {
    let root = app_root();
    let startup_config = load_config(&root);
    // Core startup is deliberately deferred until Tauri's single-instance
    // plugin has established that this process is the primary desktop shell.
    let runtime = Arc::new(CoreRuntime {
        child: Mutex::new(None),
        config: Mutex::new(startup_config),
        root: root.clone(),
        primary: AtomicBool::new(false),
        stopping: AtomicBool::new(false),
    });
    let exit_runtime = Arc::clone(&runtime);
    let launch_args: Vec<String> = std::env::args().collect();
    let background = background_launch(&launch_args);
    // Packaging smoke tests run a staged copy alongside a user's installed
    // application. Keep the production single-instance guard intact, but let
    // the staged process use its isolated core/port when explicitly requested.
    let build_smoke = std::env::var_os("HLS_DOWNLOADER_BUILD_SMOKE").is_some();

    let mut builder = tauri::Builder::default();
    if !build_smoke {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let runtime = app.state::<Arc<CoreRuntime>>();
            let _ = ensure_core(runtime.inner());
            let config = runtime_config(runtime.inner());
            for arg in args.iter().skip(1) {
                import_torrent_path(&config, arg);
            }
            if !background_launch(&args) {
                show_main(app);
            }
        }));
    }

    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .manage(Arc::clone(&runtime))
        .manage(DesktopPaths { root: root.clone() })
        .invoke_handler(tauri::generate_handler![
            get_app_root,
            get_desktop_info,
            get_core_config,
            open_browser_extension_installer,
            begin_uninstall
        ])
        .setup(move |app| {
            let startup_runtime = app.state::<Arc<CoreRuntime>>();
            startup_runtime.primary.store(true, Ordering::Relaxed);
            ensure_core(startup_runtime.inner()).map_err(std::io::Error::other)?;
            supervise_core(Arc::clone(startup_runtime.inner()));
            let config = runtime_config(startup_runtime.inner());
            for arg in std::env::args().skip(1) {
                import_torrent_path(&config, &arg);
            }
            let open = MenuItem::with_id(app, "open", "打开下载器", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .menu(&menu)
                .show_menu_on_left_click(false);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, event| match event.id.as_ref() {
                "open" => show_main(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main(tray.app_handle());
                }
            })
            .build(app)?;
            watch_clipboard(app.handle().clone());
            if !background {
                show_main(app.handle());
            }
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
            if !exit_runtime.primary.load(Ordering::Relaxed) {
                return;
            }
            exit_runtime.stopping.store(true, Ordering::Relaxed);
            let config = runtime_config(&exit_runtime);
            request_core_shutdown(&config);
            std::thread::sleep(Duration::from_millis(400));
            if let Ok(mut slot) = exit_runtime.child.lock() {
                if let Some(child) = slot.as_mut() {
                    if child.try_wait().ok().flatten().is_none() {
                        let _ = child.kill();
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod clipboard_tests {
    use super::{
        background_launch, downloadable_clipboard_text, log_backup_path, rotate_core_log,
        uninstall_in_place_argument, CORE_LOG_MAX_BYTES,
    };
    use std::fs;
    use std::path::Path;

    #[test]
    fn nsis_uninstall_directory_is_one_argument() {
        assert_eq!(
            uninstall_in_place_argument(Path::new(r"E:\HLS Downloader")),
            r"_?=E:\HLS Downloader"
        );
    }

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
    fn background_native_launch_does_not_request_foreground() {
        assert!(background_launch(&[
            "app.exe".into(),
            "--background".into()
        ]));
        assert!(background_launch(&[
            "app.exe".into(),
            "--native-host".into()
        ]));
        assert!(!background_launch(&["app.exe".into()]));
        assert!(!background_launch(&[
            "app.exe".into(),
            "movie.torrent".into()
        ]));
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

    #[test]
    fn rotates_oversized_core_logs() {
        let root = std::env::temp_dir().join(format!(
            "hls-downloader-log-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp log directory");
        let log = root.join("core.log");
        let file = fs::File::create(&log).expect("create core log");
        file.set_len(CORE_LOG_MAX_BYTES + 1)
            .expect("size core log");
        drop(file);

        rotate_core_log(&log).expect("rotate core log");

        assert!(!log.exists());
        assert!(log_backup_path(&log, 1).exists());
        fs::remove_dir_all(root).expect("clean temp log directory");
    }
}
