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
use tauri::{Manager, WindowEvent};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Debug, Deserialize)]
struct LocalConfig {
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default = "default_token")]
    token: String,
}

fn default_port() -> u16 { 8765 }
fn default_token() -> String { "55555".to_string() }

impl Default for LocalConfig {
    fn default() -> Self { Self { port: default_port(), token: default_token() } }
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
    serde_json::json!({ "port": runtime.config.port, "token": runtime.config.token })
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
    std::fs::read_to_string(root.join("config.json"))
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
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

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn main() {
    let root = app_root();
    let config = load_config(&root);
    let child = start_core(&root, &config).unwrap_or_else(|reason| {
        eprintln!("{reason}");
        None
    });
    let runtime = Arc::new(CoreRuntime { child: Mutex::new(child), config });
    let exit_runtime = Arc::clone(&runtime);
    let background = std::env::args().any(|arg| arg == "--background" || arg == "--native-host");

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .manage(Arc::clone(&runtime))
        .manage(DesktopPaths { root: root.clone() })
        .invoke_handler(tauri::generate_handler![get_app_root, get_desktop_info, get_core_config, begin_uninstall])
        .setup(move |app| {
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
