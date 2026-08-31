//! Resident v7 Core: unique SQLite owner serving UI and Native Messaging.

use crate::{
    default_core_bind, default_v7_database_path, serve_tcp_listener, CoreCommand, CoreCoordinator,
    CoreEvent, CorePipeRequest, CorePipeResponse, EventEnvelope, PersistentCore, V7_PROTOCOL_NAME,
    V7_PROTOCOL_VERSION,
};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

pub struct CoreServer {
    coordinator: CoreCoordinator,
    notify: Arc<(Mutex<u64>, Condvar)>,
    stop: Arc<AtomicBool>,
    torrent_probe_active: Arc<AtomicBool>,
}

impl CoreServer {
    pub fn open_default() -> Result<Self, String> {
        let database_path = default_v7_database_path();
        crate::v6_migrate::migrate_installed_v6_database(&database_path)?;
        let server = Self::open_path(database_path)?;
        server.restore_install_result();
        Ok(server)
    }

    pub fn open_path(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let coordinator = CoreCoordinator::new(PersistentCore::open(path)?);
        bootstrap_store(&coordinator)?;
        Self::from_coordinator(coordinator)
    }

    pub fn in_memory() -> Result<Self, String> {
        Self::from_coordinator(CoreCoordinator::new(PersistentCore::in_memory()?))
    }

    pub fn from_coordinator(coordinator: CoreCoordinator) -> Result<Self, String> {
        let sequence = coordinator.latest_sequence()?;
        let stop = Arc::new(AtomicBool::new(false));
        spawn_torrent_watch(coordinator.clone(), Arc::clone(&stop));
        spawn_clipboard_watch(coordinator.clone(), Arc::clone(&stop));
        let _ = coordinator.recover_startup();
        spawn_queue_scheduler(coordinator.clone(), Arc::clone(&stop));
        Ok(Self {
            coordinator,
            notify: Arc::new((Mutex::new(sequence), Condvar::new())),
            stop,
            torrent_probe_active: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn coordinator(&self) -> &CoreCoordinator {
        &self.coordinator
    }

    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.notify.1.notify_all();
    }

    fn restore_install_result(&self) {
        if let Ok(Some(result)) = crate::updater::take_install_result() {
            let _ = self.coordinator.lock().and_then(|mut core| {
                core.emit(CoreEvent::UpdateInstallResult {
                    latest: result.version,
                    status: result.status,
                    exit_code: result.exit_code,
                    message: result.message,
                    install_log: result.install_log,
                })
            });
        }
    }

    pub fn bind_local(
        &self,
    ) -> Result<(std::net::SocketAddr, thread::JoinHandle<Result<(), String>>), String> {
        #[cfg(windows)]
        {
            let stop = self.stop_handle();
            let handler = self.handler();
            let stop_on_startup_failure = self.stop_handle();
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                let server = crate::NamedPipeServer::new(crate::v7_pipe_name());
                server.serve_loop_with_ready(stop, handler, ready_tx)
            });
            match ready_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let _ = worker.join();
                    return Err(format!("bind v7 Core named pipe: {error}"));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    stop_on_startup_failure.store(true, Ordering::SeqCst);
                    return Err("v7 Core named pipe startup timed out".into());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("v7 Core named pipe startup thread exited before ready".into());
                }
            }
        }
        if !crate::tcp_loopback_enabled() {
            let stop = self.stop_handle();
            let worker = thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(())
            });
            return Ok((default_core_bind(), worker));
        }
        let listener = TcpListener::bind(default_core_bind())
            .or_else(|_| TcpListener::bind("127.0.0.1:0"))
            .map_err(|error| format!("bind v7 Core: {error}"))?;
        let addr = listener
            .local_addr()
            .map_err(|error| format!("v7 Core local addr: {error}"))?;
        Ok((addr, self.serve(listener)))
    }

    pub fn serve(&self, listener: TcpListener) -> thread::JoinHandle<Result<(), String>> {
        let stop = Arc::clone(&self.stop);
        let handler = self.handler();
        thread::spawn(move || serve_tcp_listener(listener, stop, handler))
    }

    pub fn handler(&self) -> Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> {
        let coordinator = self.coordinator.clone();
        let notify = Arc::clone(&self.notify);
        let stop = Arc::clone(&self.stop);
        let torrent_probe_active = Arc::clone(&self.torrent_probe_active);
        Arc::new(move |request| {
            dispatch(&coordinator, &notify, &stop, &torrent_probe_active, request)
        })
    }
}

struct TorrentProbeSlot(Arc<AtomicBool>);

impl TorrentProbeSlot {
    fn acquire(active: &Arc<AtomicBool>) -> Option<Self> {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(Arc::clone(active)))
    }
}

impl Drop for TorrentProbeSlot {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn bootstrap_store(coordinator: &CoreCoordinator) -> Result<(), String> {
    coordinator.set_setting("legal_terms_accepted", serde_json::json!(true))?;
    coordinator.set_setting(
        "legal_terms_version",
        serde_json::json!(crate::LEGAL_TERMS_VERSION),
    )?;
    let core = coordinator.core();
    let mut core = core.lock().map_err(|_| "Core mutex poisoned".to_string())?;
    if let Err(error) = crate::maybe_migrate_from_5x(&mut core) {
        if crate::migrate::migration_requested_explicitly() {
            return Err(error);
        }
        eprintln!("legacy migration skipped: {error}");
    }
    Ok(())
}

fn dispatch(
    coordinator: &CoreCoordinator,
    notify: &Arc<(Mutex<u64>, Condvar)>,
    stop: &Arc<AtomicBool>,
    torrent_probe_active: &Arc<AtomicBool>,
    request: CorePipeRequest,
) -> CorePipeResponse {
    match request {
        CorePipeRequest::Hello { protocol, version } => {
            if protocol == V7_PROTOCOL_NAME && version == V7_PROTOCOL_VERSION {
                CorePipeResponse::Hello {
                    protocol,
                    version,
                    pid: std::process::id(),
                }
            } else {
                CorePipeResponse::Error {
                    request_id: None,
                    code: "protocol_mismatch".into(),
                    message: "v7 Core protocol mismatch".into(),
                }
            }
        }
        CorePipeRequest::Command {
            request_id,
            command,
        } => {
            if matches!(&command, CoreCommand::ProbeTorrent { .. }) {
                let Some(slot) = TorrentProbeSlot::acquire(torrent_probe_active) else {
                    return CorePipeResponse::Error {
                        request_id: Some(request_id),
                        code: "torrent_probe_busy".into(),
                        message: "已有种子探测正在执行".into(),
                    };
                };
                let coordinator = coordinator.clone();
                let notify = Arc::clone(notify);
                thread::spawn(move || {
                    let _slot = slot;
                    if let Err(error) = coordinator.dispatch(command) {
                        let _ = coordinator.lock().and_then(|mut core| {
                            core.emit(CoreEvent::Error {
                                code: "torrent_probe_failed".into(),
                                message: error,
                            })
                        });
                    }
                    bump_notify(&notify, &coordinator);
                });
                // Probe completion is delivered through the normal event stream. Returning
                // immediately keeps slow tracker/DHT/metadata I/O off the IPC request thread.
                return CorePipeResponse::Events {
                    request_id,
                    events: Vec::new(),
                };
            }
            let should_shutdown = matches!(&command, CoreCommand::Shutdown);
            match coordinator.dispatch(command) {
                Ok(events) => {
                    bump_notify(notify, coordinator);
                    if should_shutdown {
                        stop.store(true, Ordering::SeqCst);
                        notify.1.notify_all();
                    }
                    if events
                        .iter()
                        .any(|item| matches!(item.event, CoreEvent::UpdateInstallStarted { .. }))
                    {
                        let stop = Arc::clone(stop);
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(500));
                            stop.store(true, Ordering::SeqCst);
                        });
                    }
                    CorePipeResponse::Events { request_id, events }
                }
                Err(error) => CorePipeResponse::Error {
                    request_id: Some(request_id),
                    code: "core_command_failed".into(),
                    message: error,
                },
            }
        }
        CorePipeRequest::Snapshot { request_id } => match coordinator.tasks() {
            Ok(tasks) => CorePipeResponse::Snapshot {
                request_id,
                tasks,
                latest_sequence: coordinator.latest_sequence().unwrap_or_default(),
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "snapshot_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::Capabilities { request_id } => CorePipeResponse::Capabilities {
            request_id,
            product_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: V7_PROTOCOL_VERSION,
            commands: vec![
                "create_task",
                "import_curl",
                "task_action",
                "open_main",
                "hide_main",
                "shutdown",
                "set_setting",
                "accept_handoff",
                "reject_handoff",
                "present_handoff",
                "play_task",
                "cast_task",
                "player_control",
                "reorder_queue",
                "assign_queue",
                "check_update",
                "download_update",
                "install_update",
                "probe_url",
                "probe_torrent",
                "select_torrent_files",
                "discover_cast_devices",
                "cast_to_device",
                "share_media",
                "request_media_push",
                "resolve_media_push",
                "open_completed",
                "confirm_power_action",
                "cancel_power_action",
                "clear_completed",
                "save_site_profile",
                "import_paths",
                "export_tasks",
                "place_queue",
                "harvest_page",
                "get_task_log",
                "browser_hello",
                "control_cast",
                "set_default_cookie",
                "set_site_rule_credential",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            settings: crate::download_worker::PUBLIC_SETTING_KEYS
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            max_frame_bytes: crate::core_ipc::V7_PIPE_MAX_FRAME as u64,
        },
        CorePipeRequest::WaitEvents {
            request_id,
            after_sequence,
            timeout_ms,
        } => {
            let events = wait_events(coordinator, notify, after_sequence, timeout_ms);
            CorePipeResponse::Events { request_id, events }
        }
        CorePipeRequest::StoreSetting {
            request_id,
            key,
            value,
        } => match coordinator.set_setting(&key, value) {
            Ok(()) => {
                bump_notify(notify, coordinator);
                settings_response(coordinator, request_id)
            }
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "setting_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::StoreSettings { request_id, values } => {
            match coordinator.set_settings(values) {
                Ok(()) => {
                    bump_notify(notify, coordinator);
                    settings_response(coordinator, request_id)
                }
                Err(error) => CorePipeResponse::Error {
                    request_id: Some(request_id),
                    code: "settings_failed".into(),
                    message: error,
                },
            }
        }
        CorePipeRequest::LoadSettings { request_id } => settings_response(coordinator, request_id),
        CorePipeRequest::SetDefaultCookie { request_id, cookie } => {
            match coordinator.set_default_cookie(&cookie) {
                Ok(()) => settings_response(coordinator, request_id),
                Err(error) => CorePipeResponse::Error {
                    request_id: Some(request_id),
                    code: "credential_failed".into(),
                    message: error,
                },
            }
        }
        CorePipeRequest::SetSiteRuleCredential {
            request_id,
            host,
            cookie,
            request_headers,
            clear,
        } => match coordinator.set_site_rule_credential(&host, &cookie, &request_headers, clear) {
            Ok(()) => settings_response(coordinator, request_id),
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "credential_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::StoreCredential {
            request_id,
            credential_ref,
            protected_blob,
            kind,
        } => match coordinator.store_credential(&credential_ref, &protected_blob, &kind) {
            Ok(()) => CorePipeResponse::Credential {
                request_id,
                protected_blob: Some(protected_blob),
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "credential_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::LoadCredential {
            request_id,
            credential_ref,
        } => match coordinator.load_credential(&credential_ref) {
            Ok(protected_blob) => CorePipeResponse::Credential {
                request_id,
                protected_blob,
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "credential_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::SaveHandoff {
            request_id,
            handoff_id,
            handoff_json,
            status,
            task_id,
            created_at_ms,
        } => match coordinator.save_handoff(
            &handoff_id,
            &handoff_json,
            &status,
            task_id.as_deref(),
            created_at_ms,
        ) {
            Ok(()) => CorePipeResponse::Handoffs {
                request_id,
                items: coordinator.load_handoffs().unwrap_or_default(),
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "handoff_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::LoadHandoffs { request_id } => match coordinator.load_handoffs() {
            Ok(items) => CorePipeResponse::Handoffs { request_id, items },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "handoff_failed".into(),
                message: error,
            },
        },
    }
}

fn settings_response(coordinator: &CoreCoordinator, request_id: u64) -> CorePipeResponse {
    match coordinator.settings() {
        Ok(settings) => CorePipeResponse::Settings {
            request_id,
            takeover_enabled: settings.takeover_enabled,
            takeover_minimum_bytes: settings.takeover_minimum_bytes,
            legal_accepted: settings.legal_accepted,
            speed_limit_kib: settings.speed_limit_kib,
            hourly_quota_mib: settings.hourly_quota_mib,
            schedule_enabled: settings.schedule_enabled,
            schedule_start: settings.schedule_start,
            schedule_end: settings.schedule_end,
            schedule_kib: settings.schedule_kib,
            auto_category: settings.auto_category,
            category_dir_media: settings.category_dirs.media,
            category_dir_program: settings.category_dirs.program,
            category_dir_archive: settings.category_dirs.archive,
            category_dir_other: settings.category_dirs.other,
            queue_max: settings.queue_max,
            queue_profiles: settings.queue_profiles,
            site_rules: settings.site_rules,
            av_scan_enabled: settings.av_scan_enabled,
            av_scan_command: settings.av_scan_command,
            torrent_watch: settings.torrent_watch,
            torrent_watch_enabled: settings.torrent_watch_enabled,
            download_dir: settings.download_dir,
            temp_dir: settings.temp_dir,
            default_concurrency: settings.default_concurrency,
            proxy_url: settings.proxy_url,
            ffmpeg_path: settings.ffmpeg_path,
            clipboard_watch: settings.clipboard_watch,
            completion_sound_enabled: settings.completion_sound_enabled,
            progress_window_enabled: settings.progress_window_enabled,
            complete_popup_enabled: settings.complete_popup_enabled,
            resume_interrupted: settings.resume_interrupted,
            auto_retry_max: settings.auto_retry_max,
            existing_file_policy: settings.existing_file_policy,
            live_record_max_minutes: settings.live_record_max_minutes,
            download_subtitles: settings.download_subtitles,
            skip_ad_segments: settings.skip_ad_segments,
            keep_temp_files: settings.keep_temp_files,
            default_user_agent: settings.default_user_agent,
            tvbox_endpoint: settings.tvbox_endpoint,
            dark_mode: settings.dark_mode,
            allow_duplicate: settings.allow_duplicate,
            queue_auto_start_enabled: settings.queue_auto_start_enabled,
            queue_auto_start_time: settings.queue_auto_start_time,
            queue_auto_stop_enabled: settings.queue_auto_stop_enabled,
            queue_auto_stop_time: settings.queue_auto_stop_time,
            default_referer: settings.default_referer,
            default_origin: settings.default_origin,
            allowed_hosts: settings.allowed_hosts,
            http_chunk_size_mb: settings.http_chunk_size_mb,
            completion_power_action: settings.completion_power_action,
            start_on_login: settings.start_on_login,
            queue_active_days: settings.queue_active_days,
            proxy_mode: settings.proxy_mode,
            proxy_bypass: settings.proxy_bypass,
            legal_terms_version: settings.legal_terms_version,
            reduce_motion: settings.reduce_motion,
            harvest_minimum_bytes: settings.harvest_minimum_bytes,
            av_scan_fail_on_threat: settings.av_scan_fail_on_threat,
            bt_upload_limit_kib: settings.bt_upload_limit_kib,
            bt_max_connections: settings.bt_max_connections,
            bt_enable_dht: settings.bt_enable_dht,
            preferred_cast_device_id: settings.preferred_cast_device_id,
            task_column_layout: settings.task_column_layout,
            toolbar_actions: settings.toolbar_actions,
            task_sort: settings.task_sort,
            default_cookie_configured: coordinator.default_cookie_configured().unwrap_or(false),
        },
        Err(error) => CorePipeResponse::Error {
            request_id: Some(request_id),
            code: "settings_failed".into(),
            message: error,
        },
    }
}

fn bump_notify(notify: &Arc<(Mutex<u64>, Condvar)>, coordinator: &CoreCoordinator) {
    if let Ok(sequence) = coordinator.latest_sequence() {
        if let Ok(mut current) = notify.0.lock() {
            *current = sequence;
        }
        notify.1.notify_all();
    }
}

fn wait_events(
    coordinator: &CoreCoordinator,
    notify: &Arc<(Mutex<u64>, Condvar)>,
    after_sequence: u64,
    timeout_ms: u64,
) -> Vec<EventEnvelope> {
    if let Ok(events) = coordinator.events_after(after_sequence, 256) {
        if !events.is_empty() {
            return events;
        }
    }
    let timeout = Duration::from_millis(timeout_ms.clamp(1, 60_000));
    let (lock, condvar) = notify.as_ref();
    if let Ok(guard) = lock.lock() {
        let _ = condvar.wait_timeout_while(guard, timeout, |sequence| *sequence <= after_sequence);
    }
    coordinator
        .events_after(after_sequence, 256)
        .unwrap_or_default()
}

fn spawn_torrent_watch(coordinator: CoreCoordinator, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut watch = crate::torrent_engine::TorrentWatch::default();
        let mut primed_dir = String::new();
        while !stop.load(Ordering::SeqCst) {
            let enabled = coordinator
                .lock()
                .ok()
                .and_then(|core| core.store().setting_bool("watch_torrents", false).ok())
                .unwrap_or(false);
            let dir = coordinator
                .lock()
                .ok()
                .and_then(|core| core.store().setting_string("torrent_watch_dir", "").ok())
                .unwrap_or_default();
            let dir = if dir.trim().is_empty() {
                std::env::var("HLS_V7_TORRENT_WATCH").unwrap_or_default()
            } else {
                dir
            };
            let dir = if enabled {
                dir.trim().to_string()
            } else {
                String::new()
            };
            if dir.is_empty() {
                watch = crate::torrent_engine::TorrentWatch::default();
                primed_dir.clear();
            } else {
                let path = std::path::Path::new(&dir);
                if path.is_dir() {
                    if primed_dir != dir {
                        watch = crate::torrent_engine::TorrentWatch::default();
                        if watch.prime(path).is_ok() {
                            primed_dir = dir;
                        }
                    } else if let Ok(files) = watch.scan(path) {
                        for file in files {
                            let url = file.to_string_lossy().into_owned();
                            if url.trim().is_empty() {
                                continue;
                            }
                            let _ = coordinator
                                .dispatch(CoreCommand::CreateTask {
                                    spec: crate::TaskSpec {
                                        url: url.clone(),
                                        resource_kind: crate::classify_url(&url),
                                        filename: file
                                            .file_stem()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into(),
                                        ..Default::default()
                                    },
                                })
                                .or_else(|error| {
                                    coordinator.lock().and_then(|mut core| {
                                        core.emit(CoreEvent::Toast {
                                            level: "warn".into(),
                                            message: format!("监视目录未导入 {url}: {error}"),
                                        })
                                    })
                                });
                        }
                    }
                }
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}

fn spawn_clipboard_watch(coordinator: CoreCoordinator, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut previous = String::new();
        while !stop.load(Ordering::SeqCst) {
            let enabled = coordinator
                .settings()
                .map(|settings| settings.clipboard_watch && settings.legal_accepted)
                .unwrap_or(false);
            if enabled {
                if let Some(text) = crate::read_clipboard() {
                    if text != previous {
                        previous = text.clone();
                        let urls = crate::clipboard_all_urls(&text);
                        if !urls.is_empty() {
                            let _ = coordinator
                                .lock()
                                .and_then(|mut core| core.emit(CoreEvent::ClipboardOffer { urls }));
                        }
                    }
                }
            } else {
                previous.clear();
            }
            thread::sleep(Duration::from_millis(750));
        }
    });
}

fn spawn_queue_scheduler(coordinator: CoreCoordinator, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut last_start = String::new();
        let mut last_stop = String::new();
        while !stop.load(Ordering::SeqCst) {
            let settings = coordinator.settings().ok();
            if let Some(settings) = settings {
                let stamp = crate::net_policy::local_hhmm();
                if settings.queue_auto_stop_enabled
                    && !settings.queue_auto_stop_time.is_empty()
                    && stamp == settings.queue_auto_stop_time
                    && last_stop != stamp
                {
                    last_stop = stamp.clone();
                    let _ = coordinator.pause_active_tasks();
                }
                let global_start_due = settings.queue_auto_start_enabled
                    && !settings.queue_auto_start_time.is_empty()
                    && stamp == settings.queue_auto_start_time
                    && last_start != stamp;
                if global_start_due {
                    last_start = stamp.clone();
                }
                if global_start_due
                    || settings
                        .queue_profiles
                        .iter()
                        .any(|profile| profile.schedule_enabled)
                {
                    let _ = coordinator.start_next_queued();
                }
            }
            thread::sleep(Duration::from_secs(10));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreIpcClient, ResourceKind, ResourceOffer, TaskSpec};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::net::TcpListener;

    #[test]
    fn torrent_probe_slot_allows_only_one_active_probe() {
        let active = Arc::new(AtomicBool::new(false));
        let first = TorrentProbeSlot::acquire(&active).unwrap();
        assert!(TorrentProbeSlot::acquire(&active).is_none());
        drop(first);
        assert!(TorrentProbeSlot::acquire(&active).is_some());
    }

    #[test]
    fn in_memory_core_does_not_bootstrap_from_disk() {
        let server = CoreServer::in_memory().unwrap();
        let core = server.coordinator().core();
        let legal = core
            .lock()
            .unwrap()
            .store()
            .setting_bool("legal_terms_accepted", false)
            .unwrap();
        assert!(
            !legal,
            "in_memory Core must not import config.json or force the legal gate"
        );
        server.shutdown();
    }

    #[test]
    fn product_bootstrap_removes_the_obsolete_first_run_legal_blocker() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        bootstrap_store(&coordinator).unwrap();
        let core = coordinator.core();
        let core = core.lock().unwrap();
        assert!(core
            .store()
            .setting_bool("legal_terms_accepted", false)
            .unwrap());
        assert_eq!(
            core.store()
                .setting_string("legal_terms_version", "")
                .unwrap(),
            crate::LEGAL_TERMS_VERSION
        );
    }

    #[test]
    fn shutdown_command_stops_the_core_after_acknowledging() {
        let server = CoreServer::in_memory().unwrap();
        let response = (server.handler())(CorePipeRequest::Command {
            request_id: 7,
            command: CoreCommand::Shutdown,
        });
        assert!(matches!(
            response,
            CorePipeResponse::Events { request_id: 7, .. }
        ));
        assert!(server.stop_handle().load(Ordering::SeqCst));
    }

    #[test]
    fn capabilities_publish_the_v7_command_and_setting_contract() {
        let server = CoreServer::in_memory().unwrap();
        let response = (server.handler())(CorePipeRequest::Capabilities { request_id: 91 });
        match response {
            CorePipeResponse::Capabilities {
                request_id,
                product_version,
                protocol_version,
                commands,
                settings,
                max_frame_bytes,
            } => {
                assert_eq!(request_id, 91);
                assert_eq!(product_version, "7.0.0");
                assert_eq!(protocol_version, V7_PROTOCOL_VERSION);
                assert!(commands.contains(&"probe_url".to_string()));
                assert!(commands.contains(&"discover_cast_devices".to_string()));
                assert!(commands.contains(&"import_paths".to_string()));
                assert!(commands.contains(&"set_default_cookie".to_string()));
                assert!(commands.contains(&"export_tasks".to_string()));
                assert!(settings.contains(&"browser_takeover_enabled".to_string()));
                assert!(settings.contains(&"reduce_motion".to_string()));
                assert!(settings.contains(&"temp_dir".to_string()));
                assert!(settings.contains(&"bt_enable_dht".to_string()));
                assert_eq!(max_frame_bytes, crate::core_ipc::V7_PIPE_MAX_FRAME as u64);
            }
            other => panic!("unexpected capabilities response: {other:?}"),
        }
        server.shutdown();
    }

    #[test]
    fn invalid_batch_setting_does_not_partially_commit() {
        let server = CoreServer::in_memory().unwrap();
        let response = (server.handler())(CorePipeRequest::StoreSettings {
            request_id: 92,
            values: BTreeMap::from([
                ("dark_mode".to_string(), Value::Bool(true)),
                ("proxy_mode".to_string(), Value::String("invalid".into())),
            ]),
        });
        assert!(matches!(
            &response,
            CorePipeResponse::Error {
                request_id: Some(92),
                ref code,
                ..
            } if code == "settings_failed"
        ));
        let settings = (server.handler())(CorePipeRequest::LoadSettings { request_id: 93 });
        match settings {
            CorePipeResponse::Settings {
                dark_mode,
                proxy_mode,
                ..
            } => {
                assert!(
                    !dark_mode,
                    "the valid first item must roll back with the batch"
                );
                assert_eq!(proxy_mode, "system");
            }
            other => panic!("unexpected settings response: {other:?}"),
        }
        server.shutdown();
    }

    #[test]
    fn default_cookie_is_write_only_over_the_ui_protocol() {
        let server = CoreServer::in_memory().unwrap();
        let response = (server.handler())(CorePipeRequest::SetDefaultCookie {
            request_id: 96,
            cookie: "session=private".into(),
        });
        assert!(matches!(
            &response,
            CorePipeResponse::Settings {
                request_id: 96,
                default_cookie_configured: true,
                ..
            }
        ));
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(!encoded.contains("session=private"));
        assert!(!encoded.contains("dpapi:"));
        server.shutdown();
    }

    #[test]
    fn snapshot_exposes_the_latest_event_sequence() {
        let server = CoreServer::in_memory().unwrap();
        server
            .coordinator()
            .set_setting("legal_terms_accepted", Value::Bool(true))
            .unwrap();
        let create = (server.handler())(CorePipeRequest::Command {
            request_id: 94,
            command: CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://example.test/sequence.bin".into(),
                    resource_kind: ResourceKind::File,
                    filename: "sequence.bin".into(),
                    ..Default::default()
                },
            },
        });
        assert!(matches!(
            create,
            CorePipeResponse::Events { request_id: 94, .. }
        ));
        let snapshot = (server.handler())(CorePipeRequest::Snapshot { request_id: 95 });
        match snapshot {
            CorePipeResponse::Snapshot {
                request_id,
                tasks,
                latest_sequence,
            } => {
                assert_eq!(request_id, 95);
                assert_eq!(tasks.len(), 1);
                assert!(latest_sequence > 0);
            }
            other => panic!("unexpected snapshot response: {other:?}"),
        }
        server.shutdown();
    }

    #[test]
    fn ui_and_native_host_share_one_core_over_ipc() {
        let server = CoreServer::in_memory().unwrap();
        server
            .coordinator()
            .set_setting("legal_terms_accepted", Value::Bool(true))
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let mut ui = CoreIpcClient::connect_addr(addr).unwrap();
        let mut host = CoreIpcClient::connect_addr(addr).unwrap();
        ui.command(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://example.test/shared.bin".into(),
                resource_kind: ResourceKind::File,
                filename: "shared.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
        let tasks = host.snapshot().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].filename, "shared.bin");
        host.store_setting("browser_takeover_enabled", Value::Bool(false))
            .unwrap();
        match ui.load_settings().unwrap() {
            CorePipeResponse::Settings {
                takeover_enabled, ..
            } => assert!(!takeover_enabled),
            other => panic!("{other:?}"),
        }
        server.shutdown();
    }

    #[test]
    fn ui_accept_handoff_is_visible_to_native_host_over_ipc() {
        let server = CoreServer::in_memory().unwrap();
        server
            .coordinator()
            .set_setting("legal_terms_accepted", Value::Bool(true))
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let mut ui = CoreIpcClient::connect_addr(addr).unwrap();
        let mut host = CoreIpcClient::connect_addr(addr).unwrap();
        let offer = ResourceOffer {
            url: "https://cdn.test/setup.exe".into(),
            handoff_id: "handoff-ipc".into(),
            filename: "setup.exe".into(),
            title: "Setup".into(),
            size: 2048,
            ..Default::default()
        };
        let encoded = serde_json::json!({
            "id": "handoff-ipc",
            "offer": offer,
            "filename": "setup.exe",
            "title": "Setup",
            "mime_type": "",
            "size": 2048,
            "status": "pending",
            "presentation": "queued",
            "task_id": null,
            "created_at_ms": 1,
            "request_id": ""
        })
        .to_string();
        host.save_handoff("handoff-ipc", &encoded, "pending", None, 1)
            .unwrap();
        host.command(CoreCommand::OfferResource { offer }).unwrap();
        ui.command(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-ipc".into(),
            filename: "setup.exe".into(),
            download_dir: String::new(),
            trusted_ui: true,
        })
        .unwrap();
        let rows = host.load_handoffs().unwrap();
        assert!(
            rows.iter()
                .any(|row| row.contains("\"status\":\"accepted\"")
                    && row.contains("\"task_id\":\"task-")),
            "native host must observe the UI accept: {rows:?}"
        );
        let tasks = host.snapshot().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].filename, "setup.exe");
        server.shutdown();
    }

    #[test]
    fn ui_present_handoff_failure_is_visible_to_native_host_over_ipc() {
        let server = CoreServer::in_memory().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let mut ui = CoreIpcClient::connect_addr(addr).unwrap();
        let mut host = CoreIpcClient::connect_addr(addr).unwrap();
        let offer = ResourceOffer {
            url: "https://cdn.test/clip.bin".into(),
            handoff_id: "handoff-present".into(),
            filename: "clip.bin".into(),
            ..Default::default()
        };
        let encoded = serde_json::json!({
            "id": "handoff-present",
            "offer": offer,
            "filename": "clip.bin",
            "title": "",
            "mime_type": "",
            "size": 1,
            "status": "pending",
            "presentation": "queued",
            "task_id": null,
            "created_at_ms": 1,
            "request_id": ""
        })
        .to_string();
        host.save_handoff("handoff-present", &encoded, "pending", None, 1)
            .unwrap();
        host.command(CoreCommand::OfferResource { offer }).unwrap();
        ui.command(CoreCommand::PresentHandoff {
            handoff_id: "handoff-present".into(),
            ok: false,
            presenter_id: String::new(),
        })
        .unwrap();
        let rows = host.load_handoffs().unwrap();
        assert!(
            rows.iter().any(|row| {
                row.contains("\"presentation\":\"fallback\"")
                    && row.contains("\"status\":\"pending\"")
            }),
            "native host must observe a fallback presentation that remains recoverable: {rows:?}"
        );
        server.shutdown();
    }

    #[test]
    fn present_handoff_roundtrip_p95_under_100ms() {
        let server = CoreServer::in_memory().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let mut ui = CoreIpcClient::connect_addr(addr).unwrap();
        let mut host = CoreIpcClient::connect_addr(addr).unwrap();
        let offer = ResourceOffer {
            url: "https://cdn.test/p95.bin".into(),
            handoff_id: "handoff-p95".into(),
            filename: "p95.bin".into(),
            ..Default::default()
        };
        let encoded = serde_json::json!({
            "id": "handoff-p95",
            "offer": offer,
            "filename": "p95.bin",
            "title": "",
            "mime_type": "",
            "size": 1,
            "status": "pending",
            "presentation": "queued",
            "task_id": null,
            "created_at_ms": 1,
            "request_id": ""
        })
        .to_string();
        host.save_handoff("handoff-p95", &encoded, "pending", None, 1)
            .unwrap();
        host.command(CoreCommand::OfferResource { offer }).unwrap();
        for _ in 0..3 {
            ui.command(CoreCommand::PresentHandoff {
                handoff_id: "handoff-p95".into(),
                ok: true,
                presenter_id: "presenter-p95".into(),
            })
            .unwrap();
        }
        let mut samples = Vec::new();
        for _ in 0..20 {
            let started = std::time::Instant::now();
            ui.command(CoreCommand::PresentHandoff {
                handoff_id: "handoff-p95".into(),
                ok: true,
                presenter_id: "presenter-p95".into(),
            })
            .unwrap();
            samples.push(started.elapsed());
        }
        assert!(
            samples
                .iter()
                .all(|sample| *sample < Duration::from_millis(100)),
            "confirm IPC must stay under 100ms when Core is already running; samples={samples:?}"
        );
        server.shutdown();
    }

    #[test]
    fn warm_core_ipc_command_p95_stays_under_75ms() {
        let server = CoreServer::in_memory().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let mut client = CoreIpcClient::connect_addr(addr).unwrap();
        for _ in 0..5 {
            client.command(CoreCommand::Ping).unwrap();
        }
        let mut samples = (0..64)
            .map(|_| {
                let started = std::time::Instant::now();
                client.command(CoreCommand::Ping).unwrap();
                started.elapsed()
            })
            .collect::<Vec<_>>();
        samples.sort_unstable();
        let p95 = samples[samples.len() * 95 / 100];
        println!("warm_core_ipc_p95_ms={:.3}", p95.as_secs_f64() * 1000.0);
        assert!(
            p95 <= Duration::from_millis(75),
            "warm Core IPC P95 exceeded 75ms: {p95:?}; samples={samples:?}"
        );
        server.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn named_pipe_is_the_product_ipc() {
        let server = CoreServer::in_memory().unwrap();
        server
            .coordinator()
            .set_setting("legal_terms_accepted", Value::Bool(true))
            .unwrap();
        let name = format!(
            r"\\.\pipe\HLSDownloader.v7.t{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let stop = server.stop_handle();
        let handler = server.handler();
        let serve_name = name.clone();
        thread::spawn(move || {
            let _ = crate::NamedPipeServer::new(serve_name).serve_loop(stop, handler);
        });
        let mut ui = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if let Ok(client) = CoreIpcClient::connect_pipe(&name) {
                ui = Some(client);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let mut ui = ui.expect("named pipe server should accept the UI client");
        let mut host = CoreIpcClient::connect_pipe(&name).unwrap();
        ui.command(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://example.test/pipe.bin".into(),
                resource_kind: ResourceKind::File,
                filename: "pipe.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
        let tasks = host.snapshot().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].filename, "pipe.bin");
        server.shutdown();
    }
}
