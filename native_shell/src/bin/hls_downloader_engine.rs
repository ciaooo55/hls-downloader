#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn workbench_candidates(engine: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(configured) = std::env::var("HLS_V7_WORKBENCH") {
        if !configured.trim().is_empty() {
            candidates.push(PathBuf::from(configured));
        }
    }
    if let Some(resources) = engine.parent() {
        candidates.push(resources.join("HLSDownloader.exe"));
        if let Some(app) = resources.parent() {
            candidates.push(app.join("HLSDownloader.exe"));
            if let Some(root) = app.parent() {
                candidates.push(root.join("HLSDownloader.exe"));
            }
        }
    }
    candidates
}

#[cfg(windows)]
fn focus_existing_workbench() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    // Windows 11 keeps an Explorer TabProxyWindow with the product title after
    // the real UI closes. Match Compose/AWT's actual top-level class as well as
    // the title so a stale taskbar proxy cannot swallow a tray click.
    let class_name: Vec<u16> = "SunAwtFrame\0".encode_utf16().collect();
    let title: Vec<u16> = "HLS Downloader\0".encode_utf16().collect();
    let hwnd = unsafe { FindWindowW(class_name.as_ptr(), title.as_ptr()) };
    if hwnd.is_null() {
        return false;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
    true
}

#[cfg(not(windows))]
fn focus_existing_workbench() -> bool {
    false
}

fn open_workbench() -> Result<(), String> {
    if focus_existing_workbench() {
        eprintln!("tray action: restored the running workbench");
        return Ok(());
    }
    let engine =
        std::env::current_exe().map_err(|error| format!("resolve engine path: {error}"))?;
    let launcher = workbench_candidates(&engine)
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "HLSDownloader.exe was not found next to the installed Core".to_string())?;
    eprintln!("tray action: opening {}", launcher.display());
    Command::new(&launcher)
        .current_dir(launcher.parent().unwrap_or_else(|| Path::new(".")))
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("open workbench: {error}"))
}

fn start_resident_tray() {
    let (sender, receiver) = std::sync::mpsc::channel();
    hls_native_shell::spawn_tray(sender);
    std::thread::Builder::new()
        .name("v7-tray-actions".into())
        .spawn(move || {
            while let Ok(action) = receiver.recv() {
                match action {
                    hls_native_shell::TrayAction::ShowMain => {
                        eprintln!("tray action: show workbench");
                        if let Err(error) = open_workbench() {
                            eprintln!("tray action failed: {error}");
                        }
                    }
                    hls_native_shell::TrayAction::Quit => std::process::exit(0),
                }
            }
        })
        .ok();
}

fn automatic_native_host_repair_enabled() -> bool {
    // Isolated verification and portable-candidate runs use an explicit IPC
    // endpoint. They must not overwrite the user's browser registration with
    // a manifest inside a temporary test directory.
    !["HLS_V7_PIPE", "HLS_V7_CORE_TCP", "HLS_V7_CORE_BIND"]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
}

fn shutdown_core() -> Result<(), String> {
    let mut client =
        hls_native_shell::CoreIpcClient::connect_existing(std::time::Duration::from_secs(2))?;
    client
        .command(hls_native_shell::CoreCommand::Shutdown)
        .map(|_| ())
}

fn main() -> ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--player-process") {
        return ExitCode::from(hls_native_shell::run_player_process() as u8);
    }
    if args.iter().any(|arg| arg == "--register-native-host") {
        return native_host_registration(false, native_host_user_sid(&args));
    }
    if args
        .iter()
        .any(|arg| arg == "--prepare-native-host-manifests")
    {
        return native_host_manifest_preparation();
    }
    if args.iter().any(|arg| arg == "--unregister-native-host") {
        return native_host_registration(true, native_host_user_sid(&args));
    }
    if args.iter().any(|arg| arg == "--shutdown") {
        return match shutdown_core() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("download engine shutdown failed: {error}");
                ExitCode::from(1)
            }
        };
    }
    if args.iter().any(|arg| arg == "--self-test") {
        match hls_native_shell::CoreServer::in_memory() {
            Ok(server) => match server.coordinator().tasks() {
                Ok(tasks) if tasks.is_empty() => {
                    println!(
                        "HLSDownloaderEngine {} self-test ok",
                        env!("CARGO_PKG_VERSION")
                    );
                    ExitCode::SUCCESS
                }
                Ok(_) => ExitCode::from(1),
                Err(error) => {
                    eprintln!("engine self-test failed: {error}");
                    ExitCode::from(1)
                }
            },
            Err(error) => {
                eprintln!("engine self-test failed: {error}");
                ExitCode::from(1)
            }
        }
    } else {
        if automatic_native_host_repair_enabled() {
            if let Ok(engine) = std::env::current_exe() {
                if let Err(error) = hls_native_shell::register_packaged_native_host(&engine) {
                    eprintln!("Native Host automatic registration skipped: {error}");
                }
            }
        }
        if let Err(error) = hls_native_shell::claim_v7_instance() {
            if hls_native_shell::is_already_running_error(&error) {
                return ExitCode::SUCCESS;
            }
            eprintln!("download engine instance startup failed: {error}");
            return ExitCode::from(1);
        }
        let server = match hls_native_shell::CoreServer::open_default() {
            Ok(server) => server,
            Err(error) => {
                eprintln!("download engine startup failed: {error}");
                return ExitCode::from(1);
            }
        };
        start_resident_tray();
        match server.bind_local() {
            Ok((address, worker)) => {
                eprintln!("download engine listening on {address}");
                match worker.join() {
                    Ok(Ok(())) => ExitCode::SUCCESS,
                    Ok(Err(error)) => {
                        eprintln!("{error}");
                        ExitCode::from(1)
                    }
                    Err(_) => ExitCode::from(1),
                }
            }
            Err(error) => {
                eprintln!("download engine bind failed: {error}");
                ExitCode::from(1)
            }
        }
    }
}

fn native_host_manifest_preparation() -> ExitCode {
    let engine = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("resolve Engine path for Native Host manifests: {error}");
            return ExitCode::from(1);
        }
    };
    match hls_native_shell::prepare_packaged_native_host_manifests(&engine) {
        Ok(count) => {
            println!("Native Host manifest preparation complete: {count} manifest(s)");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Native Host manifest preparation failed: {error}");
            ExitCode::from(1)
        }
    }
}

fn native_host_user_sid(args: &[String]) -> Option<&str> {
    args.iter()
        .find_map(|arg| arg.strip_prefix("--user-sid="))
        .filter(|sid| sid.starts_with("S-1-") && sid.len() < 184)
}

fn native_host_registration(unregister: bool, user_sid: Option<&str>) -> ExitCode {
    let engine = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("resolve Engine path for Native Host registration: {error}");
            return ExitCode::from(1);
        }
    };
    let result = if unregister {
        hls_native_shell::unregister_packaged_native_host_for_sid(&engine, user_sid)
    } else {
        hls_native_shell::register_packaged_native_host_for_sid(&engine, user_sid)
    };
    match result {
        Ok(count) => {
            println!(
                "Native Host {} complete: {count} registration(s)",
                if unregister { "cleanup" } else { "repair" }
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Native Host registration failed: {error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::workbench_candidates;
    /// 按**当前平台的分隔符**拼路径。`r"E:\h\app\resources\HLSDownloaderEngine.exe"` 这种字面量
    /// 在 Windows 上是一条四级路径，在 Linux 上却只是一个文件名（backslash 不是分隔符），
    /// `parent()` 会返回 None——于是同一条断言在 Windows 上是绿的、在 Linux 上是红的。
    fn platform_path(parts: &[&str]) -> std::path::PathBuf {
        let mut path = std::path::PathBuf::from(parts[0]);
        for part in &parts[1..] {
            path.push(part);
        }
        path
    }

    #[test]
    fn installed_engine_resolves_the_compose_launcher() {
        let engine = platform_path(&["E:", "h", "app", "resources", "HLSDownloaderEngine.exe"]);
        let candidates = workbench_candidates(&engine);
        // 引擎装在 resources 下，工作台 exe 允许出现在三处：同级、上一级、再上一级。
        for expected in [
            platform_path(&["E:", "h", "app", "resources", "HLSDownloader.exe"]),
            platform_path(&["E:", "h", "app", "HLSDownloader.exe"]),
            platform_path(&["E:", "h", "HLSDownloader.exe"]),
        ] {
            assert!(
                candidates.contains(&expected),
                "launcher candidate {} missing from {candidates:?}",
                expected.display()
            );
        }
    }

    #[test]
    fn engine_entry_wires_the_resident_tray() {
        let source = include_str!("hls_downloader_engine.rs");
        assert!(source.contains("start_resident_tray();"));
        assert!(source.contains("hls_native_shell::spawn_tray(sender)"));
        assert!(source.contains("focus_existing_workbench()"));
        assert!(source.contains("SunAwtFrame"));
        assert!(source.contains("--prepare-native-host-manifests"));
        assert!(source.contains("register_packaged_native_host"));
        assert!(source.contains("--unregister-native-host"));
    }
}
