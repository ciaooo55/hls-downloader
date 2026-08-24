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
    let engine = std::env::current_exe().map_err(|error| format!("resolve engine path: {error}"))?;
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

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--player-process") {
        return ExitCode::from(hls_native_shell::run_player_process() as u8);
    }
    if std::env::args().any(|arg| arg == "--self-test") {
        match hls_native_shell::CoreServer::in_memory() {
            Ok(server) => match server.coordinator().tasks() {
                Ok(tasks) if tasks.is_empty() => {
                    println!("HLSDownloaderEngine 7.0.0 self-test ok");
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
        if let Err(error) = hls_native_shell::claim_v7_instance() {
            if error.contains("already running") {
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

#[cfg(test)]
mod tests {
    use super::workbench_candidates;
    use std::path::Path;

    #[test]
    fn installed_engine_resolves_the_compose_launcher() {
        let candidates = workbench_candidates(Path::new(
            r"C:\Users\tester\AppData\Local\Programs\HLSDownloader\app\resources\HLSDownloaderEngine.exe",
        ));
        assert!(candidates.contains(&Path::new(
            r"C:\Users\tester\AppData\Local\Programs\HLSDownloader\HLSDownloader.exe"
        ).to_path_buf()));
    }

    #[test]
    fn engine_entry_wires_the_resident_tray() {
        let source = include_str!("hls_downloader_engine.rs");
        assert!(source.contains("start_resident_tray();"));
        assert!(source.contains("hls_native_shell::spawn_tray(sender)"));
        assert!(source.contains("focus_existing_workbench()"));
        assert!(source.contains("SunAwtFrame"));
    }
}
