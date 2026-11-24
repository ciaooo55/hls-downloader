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

fn open_workbench() -> Result<(), String> {
    // 由工作台的单实例入口唤醒现有窗口，让原生工作台同步可见状态。
    // 直接 ShowWindow 只恢复 HWND，Java 仍可能认为窗口已隐藏而停止绘制和交互。
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

fn format_tray_tooltip(active_count: usize, total_speed_bps: u64) -> String {
    if active_count == 0 {
        return "HLS Downloader · 待机".to_string();
    }
    let speed = if total_speed_bps >= 1024 * 1024 * 1024 {
        format!(
            "{:.2} GB/s",
            total_speed_bps as f64 / (1024.0 * 1024.0 * 1024.0)
        )
    } else if total_speed_bps >= 1024 * 1024 {
        format!("{:.1} MB/s", total_speed_bps as f64 / (1024.0 * 1024.0))
    } else if total_speed_bps >= 1024 {
        format!("{:.0} KB/s", total_speed_bps as f64 / 1024.0)
    } else {
        format!("{total_speed_bps} B/s")
    };
    format!("HLS Downloader · {active_count} 下载中 · {speed}")
}

fn current_tray_snapshot(
    server: &hls_native_shell::CoreServer,
) -> Option<(String, hls_native_shell::TrayMenuState)> {
    let settings = server.coordinator().settings().ok()?;
    let (active_count, total_speed_bps) = server.coordinator().transfer_summary().ok()?;
    let tooltip = format_tray_tooltip(active_count, total_speed_bps);
    let state = hls_native_shell::TrayMenuState {
        speed_limited: settings.speed_limit_enabled && settings.speed_limit_kib > 0,
        takeover_enabled: settings.takeover_enabled,
        clipboard_watch: settings.clipboard_watch,
    };
    Some((tooltip, state))
}

fn toggle_speed_limit_updates(
    settings: &hls_native_shell::CoreSettings,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut updates = std::collections::BTreeMap::new();
    if settings.speed_limit_enabled && settings.speed_limit_kib > 0 {
        updates.insert(
            "download_speed_limit_enabled".into(),
            serde_json::json!(false),
        );
    } else {
        updates.insert(
            "download_speed_limit_enabled".into(),
            serde_json::json!(true),
        );
        if settings.speed_limit_kib == 0 {
            updates.insert("download_speed_limit_kib".into(), serde_json::json!(1024));
        }
    }
    updates
}

fn sync_tray_state_if_changed(
    server: &hls_native_shell::CoreServer,
    last_tooltip: &mut String,
    last_state: &mut Option<hls_native_shell::TrayMenuState>,
) {
    let Some((tooltip, state)) = current_tray_snapshot(server) else {
        return;
    };
    if tooltip != *last_tooltip || *last_state != Some(state) {
        hls_native_shell::update_tray_state(&tooltip, state);
        *last_tooltip = tooltip;
        *last_state = Some(state);
    }
}

fn wake_or_open_workbench(server: &hls_native_shell::CoreServer, surface: &str) {
    if server.has_event_subscribers() {
        match server.emit_ui_show(surface) {
            Ok(()) => return,
            Err(error) => eprintln!("tray ui show ({surface}) failed: {error}"),
        }
    }
    if let Err(error) = open_workbench() {
        eprintln!("tray action failed: {error}");
    }
}

fn start_resident_tray(server: &hls_native_shell::CoreServer) {
    let (sender, receiver) = std::sync::mpsc::channel();
    hls_native_shell::spawn_tray(sender);
    let handler = server.handler();
    let action_server = server.clone();
    std::thread::Builder::new()
        .name("v7-tray-actions".into())
        .spawn(move || {
            while let Ok(action) = receiver.recv() {
                match action {
                    hls_native_shell::TrayAction::ShowMain => {
                        eprintln!("tray action: show workbench");
                        wake_or_open_workbench(&action_server, "main");
                    }
                    hls_native_shell::TrayAction::NewDownload => {
                        eprintln!("tray action: new download");
                        wake_or_open_workbench(&action_server, "new_download");
                    }
                    hls_native_shell::TrayAction::PauseAll => {
                        eprintln!("tray action: pause all");
                        if let Err(error) = action_server.coordinator().pause_active_tasks() {
                            eprintln!("tray pause all failed: {error}");
                        }
                    }
                    hls_native_shell::TrayAction::ResumeAll => {
                        eprintln!("tray action: resume all");
                        if let Err(error) = action_server.coordinator().resume_paused_tasks() {
                            eprintln!("tray resume all failed: {error}");
                        }
                    }
                    hls_native_shell::TrayAction::ToggleSpeedLimit => {
                        if let Ok(settings) = action_server.coordinator().settings() {
                            let updates = toggle_speed_limit_updates(&settings);
                            if let Err(error) = action_server.coordinator().set_settings(updates) {
                                eprintln!("tray toggle speed limit failed: {error}");
                            } else if let Some((tooltip, state)) =
                                current_tray_snapshot(&action_server)
                            {
                                hls_native_shell::update_tray_state(&tooltip, state);
                            }
                        }
                    }
                    hls_native_shell::TrayAction::ToggleTakeover => {
                        if let Ok(settings) = action_server.coordinator().settings() {
                            let next = !settings.takeover_enabled;
                            if let Err(error) = action_server
                                .coordinator()
                                .set_setting("browser_takeover_enabled", serde_json::json!(next))
                            {
                                eprintln!("tray toggle takeover failed: {error}");
                            } else if let Some((tooltip, state)) =
                                current_tray_snapshot(&action_server)
                            {
                                hls_native_shell::update_tray_state(&tooltip, state);
                            }
                        }
                    }
                    hls_native_shell::TrayAction::ToggleClipboardWatch => {
                        if let Ok(settings) = action_server.coordinator().settings() {
                            let next = !settings.clipboard_watch;
                            if let Err(error) = action_server
                                .coordinator()
                                .set_setting("clipboard_watch", serde_json::json!(next))
                            {
                                eprintln!("tray toggle clipboard watch failed: {error}");
                            } else if let Some((tooltip, state)) =
                                current_tray_snapshot(&action_server)
                            {
                                hls_native_shell::update_tray_state(&tooltip, state);
                            }
                        }
                    }
                    hls_native_shell::TrayAction::Quit => {
                        // Write the profile-scoped user-shutdown marker BEFORE
                        // closing Core IPC so any concurrent browser heartbeat
                        // (`ping`) observing the broken pipe immediately sees
                        // the user's explicit quit and never respawns Engine.
                        hls_native_shell::record_user_shutdown();
                        let response = handler(hls_native_shell::CorePipeRequest::Command {
                            request_id: 0,
                            command: hls_native_shell::CoreCommand::Shutdown,
                        });
                        match response {
                            hls_native_shell::CorePipeResponse::Events { .. } => break,
                            response => {
                                hls_native_shell::clear_user_shutdown();
                                eprintln!("tray shutdown failed: {response:?}");
                                hls_native_shell::show_notification(
                                    "暂时无法退出",
                                    "下载检查点未能保存，请稍后重试。",
                                );
                            }
                        }
                    }
                }
            }
        })
        .ok();

    let status_server = server.clone();
    let stop = server.stop_handle();
    std::thread::Builder::new()
        .name("v7-tray-status".into())
        .spawn(move || {
            let mut last_tooltip = String::new();
            let mut last_state = None;
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                sync_tray_state_if_changed(&status_server, &mut last_tooltip, &mut last_state);
                for _ in 0..5 {
                    if stop.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
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

#[cfg(windows)]
fn start_installer_endpoint(server: &hls_native_shell::CoreServer) {
    use hls_native_shell::{CoreCommand, CorePipeRequest, CorePipeResponse, NamedPipeServer};
    if hls_native_shell::v7_pipe_name().eq_ignore_ascii_case(hls_native_shell::V7_PIPE_NAME) {
        return;
    }
    let endpoint = NamedPipeServer::new(format!(
        r"\\.\pipe\HLSDownloader.v7.maintenance.{}",
        std::process::id()
    ));
    let handler = server.transport_handler();
    let stop = server.stop_handle();
    let only_shutdown = std::sync::Arc::new(move |request| match request {
        CorePipeRequest::Command {
            command: CoreCommand::Shutdown,
            ..
        } => handler(request),
        _ => CorePipeResponse::Error {
            request_id: None,
            code: "unsupported_command".into(),
            message: "Maintenance endpoint only accepts Core shutdown".into(),
        },
    });
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    match std::thread::Builder::new()
        .name("v7-installer-endpoint".into())
        .spawn(move || endpoint.serve_loop_with_ready(stop, only_shutdown, ready_tx))
    {
        Ok(_) => {
            if !matches!(
                ready_rx.recv_timeout(std::time::Duration::from_secs(2)),
                Ok(Ok(()))
            ) {
                eprintln!("Installer maintenance endpoint failed to start");
            }
        }
        Err(error) => eprintln!("Installer maintenance endpoint failed to start: {error}"),
    }
}

fn main() -> ExitCode {
    let started = std::time::Instant::now();
    let profile = std::env::var_os("HLS_V7_STARTUP_PROFILE").is_some();
    let mark = |stage: &str| {
        if profile {
            eprintln!(
                "core_startup stage={stage} elapsed_us={}",
                started.elapsed().as_micros()
            );
        }
    };
    mark("entry");
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
        // 先取得单实例锁：重复启动（浏览器 Host 拉起、用户双击、开机项）直接退出，
        // 不再重复写清单和注册表。Native Host 注册与 Core 就绪无依赖，移出启动关键路径。
        if let Err(error) = hls_native_shell::claim_v7_instance() {
            if hls_native_shell::is_already_running_error(&error) {
                return ExitCode::SUCCESS;
            }
            eprintln!("download engine instance startup failed: {error}");
            return ExitCode::from(1);
        }
        hls_native_shell::clear_user_shutdown();
        mark("instance_lock");
        if automatic_native_host_repair_enabled() {
            let spawned = std::thread::Builder::new()
                .name("v7-native-host-repair".into())
                .spawn(|| {
                    if let Ok(engine) = std::env::current_exe() {
                        if let Err(error) = hls_native_shell::register_packaged_native_host(&engine)
                        {
                            eprintln!("Native Host automatic registration skipped: {error}");
                        }
                    }
                });
            if let Err(error) = spawned {
                eprintln!("Native Host automatic registration skipped: {error}");
            }
        }
        mark("registration_dispatched");
        let database = hls_native_shell::default_v7_database_path();
        if let Some(directory) = database.parent() {
            if let Err(error) = hls_native_shell::init_runtime_log(directory) {
                eprintln!("Core diagnostic log initialization failed: {error}");
            }
        }
        hls_native_shell::record_runtime_log(
            "core_starting",
            serde_json::json!({"version": env!("CARGO_PKG_VERSION")}),
        );
        let server = match hls_native_shell::CoreServer::open_default() {
            Ok(server) => server,
            Err(error) => {
                hls_native_shell::record_runtime_log(
                    "core_start_failed",
                    serde_json::json!({"stage":"open_store"}),
                );
                eprintln!("download engine startup failed: {error}");
                return ExitCode::from(1);
            }
        };
        mark("store_ready");
        if !args.iter().any(|arg| arg == "--no-tray") {
            start_resident_tray(&server);
        }
        mark("tray");
        match server.bind_local() {
            Ok((address, worker)) => {
                #[cfg(windows)]
                start_installer_endpoint(&server);
                mark("ipc_ready");
                hls_native_shell::record_runtime_log(
                    "core_ready",
                    serde_json::json!({"elapsed_ms": started.elapsed().as_millis()}),
                );
                if hls_native_shell::tcp_loopback_enabled() {
                    eprintln!("download engine listening on {address}");
                } else {
                    eprintln!(
                        "download engine listening on {}",
                        hls_native_shell::v7_pipe_name()
                    );
                }
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
                hls_native_shell::record_runtime_log(
                    "core_start_failed",
                    serde_json::json!({"stage":"bind_ipc"}),
                );
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
    fn installed_engine_resolves_the_native_launcher() {
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
        assert!(source.contains("start_resident_tray(&server);"));
        assert!(source.contains("hls_native_shell::spawn_tray(sender)"));
        assert!(source.contains("--prepare-native-host-manifests"));
        assert!(source.contains("register_packaged_native_host"));
        assert!(source.contains("--unregister-native-host"));
    }

    #[test]
    fn tray_tooltip_formats_idle_and_active_transfer_rates() {
        assert_eq!(super::format_tray_tooltip(0, 0), "HLS Downloader · 待机");
        assert_eq!(
            super::format_tray_tooltip(1, 512),
            "HLS Downloader · 1 下载中 · 512 B/s"
        );
        assert_eq!(
            super::format_tray_tooltip(2, 500 * 1024),
            "HLS Downloader · 2 下载中 · 500 KB/s"
        );
        assert_eq!(
            super::format_tray_tooltip(3, 5 * 1024 * 1024 + 512 * 1024),
            "HLS Downloader · 3 下载中 · 5.5 MB/s"
        );
    }

    #[test]
    fn tray_speed_limit_toggle_preserves_saved_kib_and_defaults_zero_to_1024() {
        let server = hls_native_shell::CoreServer::in_memory().unwrap();
        let coordinator = server.coordinator();

        // Initially 0 KiB / disabled -> enabling defaults speed_limit_kib to 1024.
        let (_, state) = super::current_tray_snapshot(&server).unwrap();
        assert!(!state.speed_limited);
        let initial = coordinator.settings().unwrap();
        coordinator
            .set_settings(super::toggle_speed_limit_updates(&initial))
            .unwrap();
        let enabled = coordinator.settings().unwrap();
        assert!(enabled.speed_limit_enabled);
        assert_eq!(enabled.speed_limit_kib, 1024);
        assert!(
            super::current_tray_snapshot(&server)
                .unwrap()
                .1
                .speed_limited
        );

        // User configures 768 KiB/s -> tray toggle off disables without zeroing 768.
        coordinator
            .set_setting("download_speed_limit_kib", serde_json::json!(768))
            .unwrap();
        let configured = coordinator.settings().unwrap();
        coordinator
            .set_settings(super::toggle_speed_limit_updates(&configured))
            .unwrap();
        let paused = coordinator.settings().unwrap();
        assert!(!paused.speed_limit_enabled);
        assert_eq!(paused.speed_limit_kib, 768);
        assert!(
            !super::current_tray_snapshot(&server)
                .unwrap()
                .1
                .speed_limited
        );

        // Tray toggle back on restores exact 768 KiB/s.
        coordinator
            .set_settings(super::toggle_speed_limit_updates(&paused))
            .unwrap();
        let restored = coordinator.settings().unwrap();
        assert!(restored.speed_limit_enabled);
        assert_eq!(restored.speed_limit_kib, 768);
        assert!(
            super::current_tray_snapshot(&server)
                .unwrap()
                .1
                .speed_limited
        );

        // Clean up global throttle state.
        coordinator
            .set_setting("download_speed_limit_kib", serde_json::json!(0))
            .unwrap();
    }
}
