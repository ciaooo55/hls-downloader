//! Resident v7 Core: unique SQLite owner serving UI and Native Messaging.

use crate::core_service::{PresenterBridge, PresenterWakeMode};
use crate::{
    default_core_bind, default_v7_database_path, serve_tcp_listener, CoreCommand, CoreCoordinator,
    CoreEvent, CorePipeRequest, CorePipeResponse, PersistentCore, V7_PROTOCOL_NAME,
    V7_PROTOCOL_VERSION,
};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone)]
pub struct CoreServer {
    coordinator: CoreCoordinator,
    notify: Arc<(Mutex<u64>, Condvar)>,
    stop: Arc<AtomicBool>,
    stop_signal: Arc<(Mutex<bool>, Condvar)>,
    #[cfg(windows)]
    pipe_stops: Arc<Mutex<Vec<crate::NamedPipeStopHandle>>>,
    torrent_probe_active: Arc<AtomicBool>,
    presenter_bridge: Arc<PresenterBridge>,
    subtitles: Arc<crate::subtitle_translate::SubtitleService>,
}

impl CoreServer {
    pub fn open_default() -> Result<Self, String> {
        let database_path = default_v7_database_path();
        let server = Self::open_path(database_path)?;
        server.set_presenter_waker(Some(Arc::new(|mode| {
            let Some(root) = crate::install_root() else {
                return;
            };
            match mode {
                PresenterWakeMode::BackgroundToast => {
                    if let Err(error) = crate::core_spawn::ensure_presenter(&root) {
                        eprintln!("presenter wake (background) failed: {error}");
                    }
                }
                PresenterWakeMode::Workbench => {
                    if !crate::spawn_desktop_ui(&root, None) {
                        eprintln!("presenter wake (workbench) failed");
                    }
                }
            }
        })));
        if hls_client::update_install::has_pending_install_result() {
            server.presenter_bridge.mark_install_result_pending();
        }
        Ok(server)
    }

    pub fn open_path(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let started = std::time::Instant::now();
        let profile = std::env::var_os("HLS_V7_STARTUP_PROFILE").is_some();
        let mark = |stage: &str| {
            if profile {
                eprintln!(
                    "core_store_startup stage={stage} elapsed_us={}",
                    started.elapsed().as_micros()
                );
            }
        };
        let coordinator = CoreCoordinator::new(PersistentCore::open(path)?);
        mark("database_open");
        bootstrap_store(&coordinator)?;
        mark("bootstrap");
        let server = Self::from_coordinator(coordinator)?;
        mark("recovery_watchers");
        Ok(server)
    }

    pub fn in_memory() -> Result<Self, String> {
        Self::from_coordinator(CoreCoordinator::new(PersistentCore::in_memory()?))
    }

    pub fn from_coordinator(coordinator: CoreCoordinator) -> Result<Self, String> {
        coordinator.recover_startup()?;
        let notify = coordinator.event_notify()?;
        let presenter_bridge = coordinator.lock()?.presenter_bridge();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_signal = Arc::new((Mutex::new(false), Condvar::new()));
        spawn_torrent_watch(coordinator.clone(), Arc::clone(&stop));
        spawn_clipboard_watch(
            coordinator.clone(),
            Arc::clone(&presenter_bridge),
            Arc::clone(&stop),
        );
        spawn_queue_scheduler(coordinator.clone(), Arc::clone(&stop));
        crate::crawl::spawn(coordinator.clone(), Arc::clone(&stop));
        Ok(Self {
            coordinator,
            notify,
            stop,
            stop_signal,
            #[cfg(windows)]
            pipe_stops: Arc::new(Mutex::new(Vec::new())),
            torrent_probe_active: Arc::new(AtomicBool::new(false)),
            presenter_bridge,
            subtitles: Arc::new(crate::subtitle_translate::SubtitleService::default()),
        })
    }

    pub fn coordinator(&self) -> &CoreCoordinator {
        &self.coordinator
    }

    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    pub fn wait_until_stopped(&self, timeout: Duration) -> bool {
        if self.stop.load(Ordering::Acquire) {
            return true;
        }
        let Ok(guard) = self.stop_signal.0.lock() else {
            return self.stop.load(Ordering::Acquire);
        };
        if *guard || self.stop.load(Ordering::Acquire) {
            return true;
        }
        let (guard, _) = self
            .stop_signal
            .1
            .wait_timeout(guard, timeout)
            .unwrap_or_else(|e| e.into_inner());
        *guard || self.stop.load(Ordering::Acquire)
    }

    pub fn has_event_subscribers(&self) -> bool {
        self.presenter_bridge.has_subscribers()
    }

    pub fn has_ui_subscribers(&self) -> bool {
        self.presenter_bridge.has_ui_subscribers()
    }

    pub fn set_presenter_waker(&self, waker: Option<Arc<dyn Fn(PresenterWakeMode) + Send + Sync>>) {
        self.presenter_bridge.set_waker(waker);
    }

    pub fn queue_ui_show(&self, surface: &str) {
        self.presenter_bridge
            .queue_ui_show(surface, Duration::from_secs(15));
    }

    pub fn emit_ui_show(&self, surface: &str) -> Result<(), String> {
        self.coordinator
            .lock()?
            .emit(CoreEvent::UiShow {
                surface: surface.to_string(),
            })
            .map(|_| ())
    }

    #[cfg(windows)]
    pub fn register_pipe_stop_handle(&self, handle: crate::NamedPipeStopHandle) {
        if self.stop.load(Ordering::SeqCst) {
            handle.signal();
        }
        if let Ok(mut stops) = self.pipe_stops.lock() {
            stops.push(handle);
        }
    }

    pub fn shutdown(&self) {
        self.subtitles.stop_all();
        stop_event_waiters(
            &self.notify,
            &self.stop,
            &self.stop_signal,
            #[cfg(windows)]
            &self.pipe_stops,
        );
    }

    pub fn shutdown_and_flush(&self) {
        let already_stopped = self.stop.load(Ordering::Acquire);
        if !already_stopped {
            let _ = self
                .coordinator
                .lock()
                .and_then(|mut core| core.emit(CoreEvent::Shutdown));
        }
        let _ = self
            .coordinator
            .lock()
            .and_then(|mut core| core.flush_for_shutdown());
        self.shutdown();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn queue_install_result(&self, result: hls_client::update_install::InstallResult) {
        self.presenter_bridge
            .queue_install_result(CoreEvent::UpdateInstallResult {
                latest: result.version,
                status: result.status,
                exit_code: result.exit_code,
                message: result.message,
                install_log: result.install_log,
            });
    }

    #[cfg(test)]
    fn trigger_update_install_shutdown_for_test(&self) {
        spawn_update_install_shutdown(
            self.coordinator.clone(),
            Arc::clone(&self.presenter_bridge),
            Arc::clone(&self.notify),
            Arc::clone(&self.stop),
            Arc::clone(&self.stop_signal),
            #[cfg(windows)]
            Arc::clone(&self.pipe_stops),
        );
    }

    #[cfg(windows)]
    fn start_named_pipe_listener(
        &self,
        pipe_name: String,
    ) -> Result<thread::JoinHandle<Result<(), String>>, String> {
        let stop = self.stop_handle();
        let handler = self.transport_handler();
        let stop_on_startup_failure = self.stop_handle();
        let server = crate::NamedPipeServer::new(pipe_name);
        let pipe_stop = server.stop_handle();
        self.register_pipe_stop_handle(pipe_stop.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = thread::spawn(move || server.serve_loop_with_ready(stop, handler, ready_tx));
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(worker),
            Ok(Err(error)) => {
                pipe_stop.signal();
                let _ = worker.join();
                Err(format!("bind Core named pipe: {error}"))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                stop_on_startup_failure.store(true, Ordering::SeqCst);
                pipe_stop.signal();
                let _ = worker.join();
                Err("Core named pipe startup timed out".into())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                pipe_stop.signal();
                let _ = worker.join();
                Err("Core named pipe startup thread exited before ready".into())
            }
        }
    }

    pub fn bind_local(
        &self,
    ) -> Result<(std::net::SocketAddr, thread::JoinHandle<Result<(), String>>), String> {
        #[cfg(windows)]
        let pipe_worker = self.start_named_pipe_listener(crate::v7_pipe_name())?;
        if !crate::tcp_loopback_enabled() {
            if self.presenter_bridge.has_install_result_pending()
                && !self.presenter_bridge.has_ui_subscribers()
            {
                self.presenter_bridge.wake(PresenterWakeMode::Workbench);
            }
            #[cfg(windows)]
            {
                let stop = self.stop_handle();
                let stop_signal = Arc::clone(&self.stop_signal);
                let notify = Arc::clone(&self.notify);
                let pipe_stops = Arc::clone(&self.pipe_stops);
                let worker = thread::spawn(move || {
                    let result = pipe_worker
                        .join()
                        .map_err(|_| "Core named pipe listener panicked".to_string())?;
                    stop_event_waiters(&notify, &stop, &stop_signal, &pipe_stops);
                    result
                });
                return Ok((default_core_bind()?, worker));
            }
            #[cfg(not(windows))]
            {
                let stop = self.stop_handle();
                let stop_signal = Arc::clone(&self.stop_signal);
                let notify = Arc::clone(&self.notify);
                let worker = thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(50));
                    }
                    stop_event_waiters(&notify, &stop, &stop_signal);
                    Ok(())
                });
                return Ok((default_core_bind()?, worker));
            }
        }
        let bind = default_core_bind()?;
        // TCP loopback clients deliberately use the same configured endpoint.
        // Falling back to an ephemeral port would make the Core appear healthy
        // while every client keeps retrying the original address.
        let listener = TcpListener::bind(bind).map_err(|error| format!("bind Core: {error}"))?;
        let addr = listener
            .local_addr()
            .map_err(|error| format!("Core local addr: {error}"))?;
        if self.presenter_bridge.has_install_result_pending()
            && !self.presenter_bridge.has_ui_subscribers()
        {
            self.presenter_bridge.wake(PresenterWakeMode::Workbench);
        }
        #[cfg(windows)]
        {
            let tcp_worker = self.serve(listener);
            let pipe_stops = Arc::clone(&self.pipe_stops);
            let worker = thread::spawn(move || {
                let tcp_result = tcp_worker
                    .join()
                    .map_err(|_| "Core TCP listener panicked".to_string())?;
                signal_pipe_stops(&pipe_stops);
                let pipe_result = pipe_worker
                    .join()
                    .map_err(|_| "Core named pipe listener panicked".to_string())?;
                tcp_result.and(pipe_result)
            });
            Ok((addr, worker))
        }
        #[cfg(not(windows))]
        Ok((addr, self.serve(listener)))
    }

    pub fn serve(&self, listener: TcpListener) -> thread::JoinHandle<Result<(), String>> {
        let stop = Arc::clone(&self.stop);
        let stop_signal = Arc::clone(&self.stop_signal);
        let handler = self.transport_handler();
        let notify = Arc::clone(&self.notify);
        #[cfg(windows)]
        let pipe_stops = Arc::clone(&self.pipe_stops);
        thread::spawn(move || {
            let result = serve_tcp_listener(listener, Arc::clone(&stop), handler);
            stop_event_waiters(
                &notify,
                &stop,
                &stop_signal,
                #[cfg(windows)]
                &pipe_stops,
            );
            result
        })
    }

    pub fn handler(&self) -> Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> {
        let handler = self.transport_handler();
        let stop = self.stop_handle();
        let stop_signal = Arc::clone(&self.stop_signal);
        let notify = Arc::clone(&self.notify);
        #[cfg(windows)]
        let pipe_stops = Arc::clone(&self.pipe_stops);
        Arc::new(move |request| {
            let shutdown = matches!(
                request,
                CorePipeRequest::Command {
                    command: CoreCommand::Shutdown,
                    ..
                }
            );
            let response = handler(request);
            if shutdown && matches!(response, CorePipeResponse::Events { .. }) {
                stop_event_waiters(
                    &notify,
                    &stop,
                    &stop_signal,
                    #[cfg(windows)]
                    &pipe_stops,
                );
            }
            response
        })
    }

    pub fn transport_handler(
        &self,
    ) -> Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> {
        let coordinator = self.coordinator.clone();
        let notify = Arc::clone(&self.notify);
        let stop = Arc::clone(&self.stop);
        let stop_signal = Arc::clone(&self.stop_signal);
        #[cfg(windows)]
        let pipe_stops = Arc::clone(&self.pipe_stops);
        let torrent_probe_active = Arc::clone(&self.torrent_probe_active);
        let presenter_bridge = Arc::clone(&self.presenter_bridge);
        let subtitles = Arc::clone(&self.subtitles);
        Arc::new(move |request| {
            if matches!(
                &request,
                CorePipeRequest::Command {
                    command: CoreCommand::Shutdown,
                    ..
                }
            ) {
                subtitles.stop_all();
            }
            if let CorePipeRequest::Subtitle {
                request_id,
                request,
            } = request
            {
                return CorePipeResponse::Subtitle {
                    request_id,
                    result: subtitles
                        .dispatch(&coordinator, &request)
                        .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error})),
                };
            }
            dispatch(
                &coordinator,
                &notify,
                &stop,
                &stop_signal,
                #[cfg(windows)]
                &pipe_stops,
                &torrent_probe_active,
                &presenter_bridge,
                request,
            )
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
    let settings = coordinator.settings()?;
    let (stored_accepted, stored_version) = {
        let core = coordinator.lock()?;
        (
            core.store().setting_bool("legal_terms_accepted", false)?,
            core.store().setting_string("legal_terms_version", "")?,
        )
    };
    if !settings.legal_accepted || !stored_accepted {
        coordinator.set_setting("legal_terms_accepted", serde_json::json!(true))?;
    } else if stored_version != crate::LEGAL_TERMS_VERSION {
        coordinator.set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )?;
    }
    Ok(())
}

fn queue_reply_fits(events: &[crate::EventEnvelope]) -> bool {
    let mut remaining = hls_protocol::V7_PAGE_PAYLOAD_BUDGET;
    for event in events {
        let Ok(bytes) = serde_json::to_vec(event) else {
            return false;
        };
        let Some(next) = remaining.checked_sub(bytes.len() + 1) else {
            return false;
        };
        remaining = next;
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    coordinator: &CoreCoordinator,
    notify: &Arc<(Mutex<u64>, Condvar)>,
    stop: &Arc<AtomicBool>,
    stop_signal: &Arc<(Mutex<bool>, Condvar)>,
    #[cfg(windows)] pipe_stops: &Arc<Mutex<Vec<crate::NamedPipeStopHandle>>>,
    torrent_probe_active: &Arc<AtomicBool>,
    presenter_bridge: &Arc<PresenterBridge>,
    request: CorePipeRequest,
) -> CorePipeResponse {
    match request {
        CorePipeRequest::CastControl {
            request_id,
            session_generation,
            action,
        } => match coordinator.cast_control(session_generation, &action) {
            Ok(events) => CorePipeResponse::Events { request_id, events },
            Err(message) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "cast_control_failed".into(),
                message,
            },
        },
        CorePipeRequest::PlayerControl {
            request_id,
            session_generation,
            action,
        } => match coordinator.player_control(session_generation, &action) {
            Ok(events) => CorePipeResponse::Events { request_id, events },
            Err(message) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "player_control_failed".into(),
                message,
            },
        },
        CorePipeRequest::PlayerEmbed { request_id, embed } => {
            match coordinator.player_embed(embed) {
                Ok(events) => CorePipeResponse::Events { request_id, events },
                Err(message) => CorePipeResponse::Error {
                    request_id: Some(request_id),
                    code: "player_embed_failed".into(),
                    message,
                },
            }
        }
        CorePipeRequest::Crawl {
            request_id,
            request,
        } => match crate::crawl::dispatch(coordinator, request) {
            Ok(projects) => CorePipeResponse::Crawl {
                request_id,
                projects,
            },
            Err(message) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "crawl_failed".into(),
                message,
            },
        },
        CorePipeRequest::PreviewZip { request_id, source } => {
            let result = if crate::http_engine::http_fetch_url_allowed(&source) {
                coordinator
                    .metadata_spec(&source)
                    .and_then(|spec| {
                        crate::download_worker::hydrate_replay_headers(&coordinator.core(), spec)
                    })
                    .and_then(|(spec, replay)| {
                        crate::with_replay_json(&replay, || {
                            crate::zip_preview::preview(&source, Some(spec))
                        })
                    })
            } else {
                crate::zip_preview::preview(&source, None)
            };
            match result {
                Ok(preview) => CorePipeResponse::ZipPreview {
                    request_id,
                    preview,
                },
                Err(message) => CorePipeResponse::Error {
                    request_id: Some(request_id),
                    code: "zip_preview_failed".into(),
                    message,
                },
            }
        }
        CorePipeRequest::PlayerThumbnail {
            request_id,
            task_id,
            seconds,
        } => match coordinator.player_thumbnail(&task_id, seconds) {
            Ok(jpeg_base64) => CorePipeResponse::PlayerThumbnail {
                request_id,
                jpeg_base64,
            },
            Err(message) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "player_thumbnail_unavailable".into(),
                message,
            },
        },
        CorePipeRequest::Subtitle { request_id, .. } => CorePipeResponse::Error {
            request_id: Some(request_id),
            code: "subtitle_unavailable".into(),
            message: "字幕服务尚未初始化".into(),
        },
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
                    message: "Core protocol mismatch".into(),
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
                });
                // Probe completion is delivered through the normal event stream. Returning
                // immediately keeps slow tracker/DHT/metadata I/O off the IPC request thread.
                return CorePipeResponse::Events {
                    request_id,
                    events: Vec::new(),
                };
            }
            let bulk_queue = matches!(
                &command,
                CoreCommand::AssignQueue { .. }
                    | CoreCommand::ReorderQueue { .. }
                    | CoreCommand::PlaceQueue { .. }
                    | CoreCommand::ClearCompleted
            );
            let should_shutdown = matches!(&command, CoreCommand::Shutdown);
            match coordinator.dispatch(command) {
                Ok(mut events) => {
                    if should_shutdown {
                        let had_subscribers = presenter_bridge.has_subscribers();
                        match coordinator.lock().and_then(|mut core| {
                            let shutdown_events = core.emit(CoreEvent::Shutdown)?;
                            core.flush_for_shutdown()?;
                            Ok(shutdown_events)
                        }) {
                            Ok(shutdown_events) => events.extend(shutdown_events),
                            Err(message) => {
                                return CorePipeResponse::Error {
                                    request_id: Some(request_id),
                                    code: "shutdown_flush_failed".into(),
                                    message,
                                };
                            }
                        }
                        if had_subscribers {
                            // Give attached presenters a brief flush window to receive
                            // CoreEvent::Shutdown before stopping the IPC listener.
                            let deadline = std::time::Instant::now() + Duration::from_millis(60);
                            while presenter_bridge.has_subscribers()
                                && std::time::Instant::now() < deadline
                            {
                                thread::sleep(Duration::from_millis(5));
                            }
                            thread::sleep(Duration::from_millis(25));
                        }
                    }
                    if events
                        .iter()
                        .any(|item| matches!(item.event, CoreEvent::UpdateInstallStarted { .. }))
                    {
                        spawn_update_install_shutdown(
                            coordinator.clone(),
                            Arc::clone(presenter_bridge),
                            Arc::clone(notify),
                            Arc::clone(stop),
                            Arc::clone(stop_signal),
                            #[cfg(windows)]
                            Arc::clone(pipe_stops),
                        );
                    }
                    // Subscribers receive every committed update through event batches or a snapshot.
                    // A bulk acknowledgement must still fit the protocol frame.
                    let events = if bulk_queue && !queue_reply_fits(&events) {
                        Vec::new()
                    } else {
                        events
                    };
                    CorePipeResponse::Events { request_id, events }
                }
                Err(error) => CorePipeResponse::Error {
                    request_id: Some(request_id),
                    code: "core_command_failed".into(),
                    message: error,
                },
            }
        }
        CorePipeRequest::ListTasks {
            request_id,
            after_task_id,
            limit,
        } => match coordinator.task_page(after_task_id.as_deref(), limit) {
            Ok(page) => CorePipeResponse::TaskPage { request_id, page },
            Err(message) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "task_page_failed".into(),
                message,
            },
        },
        CorePipeRequest::WaitEventBatch {
            request_id,
            after_sequence,
            timeout_ms,
        } => match wait_events(
            coordinator,
            notify,
            stop,
            presenter_bridge,
            false,
            after_sequence,
            timeout_ms,
        ) {
            Ok(batch) => CorePipeResponse::EventBatch { request_id, batch },
            Err(message) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "events_failed".into(),
                message,
            },
        },
        CorePipeRequest::Snapshot { request_id } => match coordinator.lock() {
            Ok(core) => CorePipeResponse::Snapshot {
                request_id,
                tasks: core.tasks(),
                latest_sequence: core.latest_sequence(),
                browser_connected: core.browser_connected(),
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
                "list_tasks_paged",
                "event_batch_resync",
                "player_embed_generation",
                "player_control_generation",
                "preview_zip",
                "crawl_projects",
                "create_task_paused",
                "create_task",
                "import_curl",
                "task_action",
                "open_main",
                "hide_main",
                "workbench_launch_receipt",
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
                "cast_control_generation",
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
                "get_task",
                "get_task_log",
                "refresh_task_request",
                "arm_task_refresh",
                "get_task_torrent_files",
                "set_task_torrent_files",
                "browser_hello",
                "control_cast",
                "set_default_cookie",
                "set_site_rule_credential",
                "subtitle",
                "player_thumbnail",
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
        } => match wait_events(
            coordinator,
            notify,
            stop,
            presenter_bridge,
            true,
            after_sequence,
            timeout_ms,
        ) {
            Ok(batch) if batch.resync_required => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "resync_required".into(),
                message: "事件历史已变化，请重新读取任务快照".into(),
            },
            Ok(batch) => CorePipeResponse::Events {
                request_id,
                events: batch.events,
            },
            Err(message) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "events_failed".into(),
                message,
            },
        },
        CorePipeRequest::StoreSetting {
            request_id,
            key,
            value,
        } => match coordinator.set_setting(&key, value) {
            Ok(()) => settings_response(coordinator, request_id),
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "setting_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::StoreSettings { request_id, values } => {
            match coordinator.set_settings(values) {
                Ok(()) => settings_response(coordinator, request_id),
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
        CorePipeRequest::DeleteCredential {
            request_id,
            credential_ref,
        } => match coordinator.delete_credential(&credential_ref) {
            Ok(()) => CorePipeResponse::Credential {
                request_id,
                protected_blob: None,
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "credential_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::AdmitHandoff {
            request_id,
            handoff_json,
        } => match coordinator.admit_handoff(&handoff_json) {
            Ok(encoded) => CorePipeResponse::Handoffs {
                request_id,
                items: vec![encoded],
                next: None,
            },
            Err(message) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "handoff_failed".into(),
                message,
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
                // 保存调用只需要持久化回执，不应重传无关历史或让回执超过帧上限。
                items: Vec::new(),
                next: None,
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "handoff_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::LoadHandoffs {
            request_id,
            handoff_id,
            client_request_id,
            pending_only,
            paged,
            after,
        } => match if paged {
            coordinator.query_handoffs_page(
                handoff_id.as_deref(),
                client_request_id.as_deref(),
                pending_only,
                after,
            )
        } else {
            coordinator
                .query_handoffs(
                    handoff_id.as_deref(),
                    client_request_id.as_deref(),
                    pending_only,
                )
                .map(|items| (items, None))
        } {
            Ok((items, next)) => CorePipeResponse::Handoffs {
                request_id,
                items,
                next,
            },
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
            speed_limit_enabled: settings.speed_limit_enabled,
            quick_download_enabled: settings.quick_download_enabled,
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
            hls_remux_ts_to_mp4: settings.hls_remux_ts_to_mp4,
            keep_temp_files: settings.keep_temp_files,
            default_user_agent: settings.default_user_agent,
            tvbox_endpoint: settings.tvbox_endpoint,
            theme_mode: settings.theme_mode,
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

#[cfg(windows)]
fn signal_pipe_stops(pipe_stops: &Mutex<Vec<crate::NamedPipeStopHandle>>) {
    if let Ok(stops) = pipe_stops.lock() {
        for handle in stops.iter() {
            handle.signal();
        }
    }
}

fn stop_event_waiters(
    notify: &Arc<(Mutex<u64>, Condvar)>,
    stop: &AtomicBool,
    stop_signal: &Arc<(Mutex<bool>, Condvar)>,
    #[cfg(windows)] pipe_stops: &Mutex<Vec<crate::NamedPipeStopHandle>>,
) {
    {
        let _guard = notify.0.lock();
        stop.store(true, Ordering::SeqCst);
        notify.1.notify_all();
    }
    if let Ok(mut stopped) = stop_signal.0.lock() {
        *stopped = true;
        stop_signal.1.notify_all();
    }
    #[cfg(windows)]
    signal_pipe_stops(pipe_stops);
}

fn spawn_update_install_shutdown(
    coordinator: CoreCoordinator,
    presenter_bridge: Arc<PresenterBridge>,
    notify: Arc<(Mutex<u64>, Condvar)>,
    stop: Arc<AtomicBool>,
    stop_signal: Arc<(Mutex<bool>, Condvar)>,
    #[cfg(windows)] pipe_stops: Arc<Mutex<Vec<crate::NamedPipeStopHandle>>>,
) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(120));
        let had_ui = presenter_bridge.has_ui_subscribers();
        let _ = coordinator
            .lock()
            .and_then(|mut core| core.emit(CoreEvent::Shutdown));
        if had_ui {
            let deadline = std::time::Instant::now() + Duration::from_millis(350);
            while presenter_bridge.has_ui_subscribers() && std::time::Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = coordinator
            .lock()
            .and_then(|mut core| core.flush_for_shutdown());
        stop_event_waiters(
            &notify,
            &stop,
            &stop_signal,
            #[cfg(windows)]
            &pipe_stops,
        );
    });
}

fn flush_pending_presenter_events(
    coordinator: &CoreCoordinator,
    presenter_bridge: &PresenterBridge,
) -> Result<(), String> {
    let pending = presenter_bridge.take_ready_events();
    if pending.is_empty() {
        return Ok(());
    }
    let mut core = coordinator.lock()?;
    for event in pending {
        let _ = core.emit(event)?;
    }
    Ok(())
}

fn deliver_clipboard_offer(
    coordinator: &CoreCoordinator,
    presenter_bridge: &PresenterBridge,
    urls: Vec<String>,
) {
    if urls.is_empty() {
        return;
    }
    if presenter_bridge.has_ui_subscribers() {
        let _ = coordinator
            .lock()
            .and_then(|mut core| core.emit(CoreEvent::ClipboardOffer { urls }));
    } else {
        presenter_bridge.queue_clipboard_offer(urls, Duration::from_secs(30));
        presenter_bridge.wake(PresenterWakeMode::BackgroundToast);
    }
}

fn wait_events(
    coordinator: &CoreCoordinator,
    notify: &Arc<(Mutex<u64>, Condvar)>,
    stop: &AtomicBool,
    presenter_bridge: &Arc<PresenterBridge>,
    flush_ui_queue: bool,
    after_sequence: u64,
    timeout_ms: u64,
) -> Result<hls_protocol::EventBatch, String> {
    let _subscriber = presenter_bridge.enter_subscriber(flush_ui_queue);
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms.clamp(1, 60_000));
    loop {
        if flush_ui_queue {
            flush_pending_presenter_events(coordinator, presenter_bridge)?;
        }
        // 不能持有通知锁读取 Core：提交事件的锁顺序是 Core → 通知锁。
        let observed = match notify.0.lock() {
            Ok(sequence) => *sequence,
            Err(_) => return Err("事件通知锁不可用".into()),
        };
        let batch = coordinator.lock()?.event_batch_after(after_sequence)?;
        if batch.resync_required || !batch.events.is_empty() {
            return Ok(batch);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(batch);
        }
        let (lock, condvar) = notify.as_ref();
        let Ok(guard) = lock.lock() else {
            return Ok(batch);
        };
        if stop.load(Ordering::SeqCst) {
            return Ok(batch);
        }
        // 查询和等待之间若有提交，直接重查，避免丢失唤醒。
        if *guard != observed {
            continue;
        }
        let _ = condvar.wait_timeout(guard, remaining);
    }
}

fn spawn_torrent_watch(coordinator: CoreCoordinator, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut watch = crate::torrent_engine::TorrentWatch::default();
        let mut primed_dir = String::new();
        while !stop.load(Ordering::SeqCst) {
            let (enabled, dir) = coordinator
                .settings()
                .map(|settings| (settings.torrent_watch_enabled, settings.torrent_watch))
                .unwrap_or((false, String::new()));
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

fn spawn_clipboard_watch(
    coordinator: CoreCoordinator,
    presenter_bridge: Arc<PresenterBridge>,
    stop: Arc<AtomicBool>,
) {
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
                        let urls = crate::clipboard_download_urls(&text);
                        if !urls.is_empty() {
                            deliver_clipboard_offer(&coordinator, &presenter_bridge, urls);
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

#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct QueueSchedulerLatch {
    last_start: Option<(u32, String)>,
    last_stop: Option<(u32, String)>,
}

impl QueueSchedulerLatch {
    /// Evaluates edge-triggered global auto-stop / auto-start transitions for `(day_stamp, hhmm)`.
    /// Returns `(stop_due, start_due)`.
    fn step(
        &mut self,
        (day_stamp, hhmm): (u32, &str),
        weekday_ok: bool,
        auto_start_enabled: bool,
        auto_start_time: &str,
        auto_stop_enabled: bool,
        auto_stop_time: &str,
    ) -> (bool, bool) {
        if !auto_stop_enabled || auto_stop_time.is_empty() {
            self.last_stop = None;
        }
        if !auto_start_enabled || auto_start_time.is_empty() {
            self.last_start = None;
        }
        if !weekday_ok || hhmm.is_empty() {
            return (false, false);
        }
        let key = (day_stamp, hhmm.to_string());
        let stop_due = auto_stop_enabled
            && !auto_stop_time.is_empty()
            && hhmm == auto_stop_time
            && self.last_stop.as_ref() != Some(&key);
        if stop_due {
            self.last_stop = Some(key.clone());
        }
        let start_due = auto_start_enabled
            && !auto_start_time.is_empty()
            && hhmm == auto_start_time
            && self.last_start.as_ref() != Some(&key);
        if start_due {
            self.last_start = Some(key);
        }
        (stop_due, start_due)
    }
}

fn spawn_queue_scheduler(coordinator: CoreCoordinator, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut latch = QueueSchedulerLatch::default();
        while !stop.load(Ordering::SeqCst) {
            if let Ok(settings) = coordinator.settings() {
                let any_queue_schedule = settings
                    .queue_profiles
                    .iter()
                    .any(|profile| profile.schedule_enabled);
                let any_global_schedule =
                    settings.queue_auto_start_enabled || settings.queue_auto_stop_enabled;
                if !settings.queue_auto_stop_enabled {
                    latch.last_stop = None;
                }
                if !settings.queue_auto_start_enabled {
                    latch.last_start = None;
                }
                let (has_active, has_queued) =
                    coordinator.scheduler_task_flags().unwrap_or((false, false));
                if has_active || has_queued || any_global_schedule || any_queue_schedule {
                    if has_active {
                        // 每秒复用暂停控制，保存独立任务和定时队列失去窗口时的断点。
                        let _ = coordinator.pause_tasks_outside_schedule();
                    }
                    if any_global_schedule || has_queued {
                        let (global_stop_due, global_start_due) = if any_global_schedule {
                            let (day_stamp, hhmm) = crate::net_policy::local_day_stamp_and_hhmm();
                            let weekday_ok =
                                crate::net_policy::weekday_allowed(&settings.queue_active_days);
                            latch.step(
                                (day_stamp, &hhmm),
                                weekday_ok,
                                settings.queue_auto_start_enabled,
                                &settings.queue_auto_start_time,
                                settings.queue_auto_stop_enabled,
                                &settings.queue_auto_stop_time,
                            )
                        } else {
                            (false, false)
                        };
                        if global_stop_due && (has_active || has_queued) {
                            let _ = coordinator.pause_active_tasks();
                        }
                        if global_start_due {
                            let _ = coordinator.resume_paused_tasks();
                        }
                        if has_queued || global_start_due {
                            let queue_start_due = global_start_due || any_queue_schedule;
                            // 独立任务的开始时间不依赖队列定时开关；仅释放已设开始时间的任务。
                            let _ = coordinator.start_next_queued(!queue_start_due);
                        }
                    }
                }
            }
            thread::sleep(Duration::from_secs(1));
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
        let persisted_version = core
            .lock()
            .unwrap()
            .store()
            .setting_string("legal_terms_version", "")
            .unwrap();
        assert!(
            persisted_version.is_empty(),
            "in_memory Core must not import config.json or persist bootstrap rows"
        );
        assert!(server.coordinator().settings().unwrap().legal_accepted);
        server.shutdown();
    }

    #[test]
    fn product_bootstrap_removes_the_obsolete_first_run_legal_blocker() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        bootstrap_store(&coordinator).unwrap();
        let seq_after_first = coordinator.lock().unwrap().latest_sequence();
        assert!(seq_after_first >= 1);
        // Second cold-start bootstrap with up-to-date settings must skip redundant SQLite writes.
        bootstrap_store(&coordinator).unwrap();
        let seq_after_second = coordinator.lock().unwrap().latest_sequence();
        assert_eq!(
            seq_after_first, seq_after_second,
            "warm-start bootstrap_store must not re-write unchanged legal settings"
        );
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
                assert_eq!(product_version, env!("CARGO_PKG_VERSION"));
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
    fn hls_remux_setting_round_trips_through_presenter_protocol() {
        let server = CoreServer::in_memory().unwrap();
        let initial = (server.handler())(CorePipeRequest::LoadSettings { request_id: 90 });
        assert!(matches!(
            initial,
            CorePipeResponse::Settings {
                hls_remux_ts_to_mp4: true,
                ..
            }
        ));
        let disabled = (server.handler())(CorePipeRequest::StoreSettings {
            request_id: 91,
            values: BTreeMap::from([("hls_remux_ts_to_mp4".into(), Value::Bool(false))]),
        });
        assert!(matches!(
            disabled,
            CorePipeResponse::Settings {
                hls_remux_ts_to_mp4: false,
                ..
            }
        ));
        let reenabled = (server.handler())(CorePipeRequest::StoreSettings {
            request_id: 92,
            values: BTreeMap::from([("hls_remux_ts_to_mp4".into(), Value::Bool(true))]),
        });
        assert!(matches!(
            reenabled,
            CorePipeResponse::Settings {
                hls_remux_ts_to_mp4: true,
                ..
            }
        ));
        let loaded = (server.handler())(CorePipeRequest::LoadSettings { request_id: 93 });
        assert!(matches!(
            loaded,
            CorePipeResponse::Settings {
                hls_remux_ts_to_mp4: true,
                ..
            }
        ));
        server.shutdown();
    }

    #[test]
    fn cold_start_ui_events_wake_presenter_and_replay_on_first_wait_events() {
        let _ = crate::power_action::cancel();
        let server = CoreServer::in_memory().unwrap();
        let wakes = Arc::new(Mutex::new(Vec::new()));
        let wakes_clone = Arc::clone(&wakes);
        server.set_presenter_waker(Some(Arc::new(move |mode| {
            wakes_clone.lock().unwrap().push(mode);
        })));

        // 1. ClipboardOffer when no Presenter subscriber is attached:
        deliver_clipboard_offer(
            server.coordinator(),
            &server.presenter_bridge,
            vec!["https://cdn.test/clip.mp4".into()],
        );
        // 2. Cold-start tray NewDownload queued before workbench attaches:
        server.queue_ui_show("new_download");
        // 3. PowerActionPending scheduled when no Presenter subscriber is attached:
        crate::power_action::schedule_with_title("shutdown", "movie.mp4", 30).unwrap();
        server.presenter_bridge.mark_power_action_pending();
        server
            .presenter_bridge
            .wake(PresenterWakeMode::BackgroundToast);

        assert_eq!(
            *wakes.lock().unwrap(),
            vec![
                PresenterWakeMode::BackgroundToast,
                PresenterWakeMode::BackgroundToast
            ]
        );

        // Simulate Presenter booting, reading snapshot_state(), and entering WaitEvents:
        let baseline = server.coordinator().latest_sequence().unwrap();
        let response = (server.handler())(CorePipeRequest::WaitEvents {
            request_id: 55,
            after_sequence: baseline,
            timeout_ms: 50,
        });
        let events = match response {
            CorePipeResponse::Events { events, .. } => events,
            other => panic!("unexpected WaitEvents response: {other:?}"),
        };
        assert!(events.iter().any(|e| matches!(
            &e.event,
            CoreEvent::PowerActionPending { action, title, .. }
                if action == "shutdown" && title == "movie.mp4"
        )));
        assert!(events.iter().any(|e| matches!(
            &e.event,
            CoreEvent::ClipboardOffer { urls }
                if urls == &vec!["https://cdn.test/clip.mp4".to_string()]
        )));
        assert!(events.iter().any(|e| matches!(
            &e.event,
            CoreEvent::UiShow { surface } if surface == "new_download"
        )));

        let _ = crate::power_action::cancel();
        server.shutdown();
    }

    #[test]
    fn theme_settings_validate_and_preserve_explicit_system() {
        let server = CoreServer::in_memory().unwrap();
        let handler = server.handler();
        let mode = |response| match response {
            CorePipeResponse::Settings { theme_mode, .. } => theme_mode,
            other => panic!("unexpected settings response: {other:?}"),
        };
        assert_eq!(
            mode(handler(CorePipeRequest::LoadSettings { request_id: 1 })),
            "system"
        );
        for (index, (key, value, expected)) in [
            ("theme_mode", Value::String("light".into()), "light"),
            ("theme_mode", Value::String("dark".into()), "dark"),
            ("theme_mode", Value::String("system".into()), "system"),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                mode(handler(CorePipeRequest::StoreSetting {
                    request_id: index as u64 + 2,
                    key: key.into(),
                    value,
                })),
                expected
            );
        }
        for value in [
            Value::String("invalid".into()),
            Value::Bool(false),
            Value::Null,
        ] {
            assert!(matches!(
                handler(CorePipeRequest::StoreSetting {
                    request_id: 20,
                    key: "theme_mode".into(),
                    value,
                }),
                CorePipeResponse::Error { .. }
            ));
        }
        assert_eq!(
            mode(handler(CorePipeRequest::LoadSettings { request_id: 21 })),
            "system"
        );
        server.shutdown();
    }

    #[test]
    fn proxy_settings_responses_and_events_are_passwordless() {
        let server = CoreServer::in_memory().unwrap();
        let handler = server.handler();
        for (raw, clean) in [
            (
                "http://alice:secret@proxy.example:8080",
                "http://alice@proxy.example:8080",
            ),
            (
                "socks5://bob:pw@127.0.0.1:1080",
                "socks5://bob@127.0.0.1:1080",
            ),
        ] {
            let rules = serde_json::json!([{"host":"media.example","proxy":"https://rule:rule-password@rules.example:443"}]);
            let written = handler(CorePipeRequest::StoreSettings {
                request_id: 1,
                values: BTreeMap::from([
                    ("proxy_mode".into(), Value::String("manual".into())),
                    ("proxy_url".into(), Value::String(raw.into())),
                    ("site_rules".into(), Value::String(rules.to_string())),
                ]),
            });
            for response in [
                written,
                handler(CorePipeRequest::LoadSettings { request_id: 2 }),
            ] {
                let CorePipeResponse::Settings {
                    proxy_url,
                    site_rules,
                    ..
                } = response
                else {
                    panic!("expected settings");
                };
                assert_eq!(proxy_url, clean);
                assert_eq!(
                    crate::parse_site_rules(&site_rules)[0].proxy,
                    "https://rule@rules.example:443"
                );
            }
            for (target, expected) in [
                ("https://other.example/file", raw),
                (
                    "https://media.example/file",
                    "https://rule:rule-password@rules.example:443",
                ),
            ] {
                let spec = server.coordinator.metadata_spec(target).unwrap();
                let (transfer, _) = crate::download_worker::hydrate_replay_headers(
                    &server.coordinator.core(),
                    spec,
                )
                .unwrap();
                assert_eq!(transfer.proxy, expected);
            }
            let core = server.coordinator.lock().unwrap();
            assert_eq!(core.store().setting_string("proxy_url", "").unwrap(), clean);
            let events = serde_json::to_string(&core.events_after(0, 100)).unwrap();
            assert!(!events.contains(raw) && !events.contains("rule-password"));
        }
        server.shutdown();
    }

    #[test]
    fn invalid_batch_setting_does_not_partially_commit() {
        let server = CoreServer::in_memory().unwrap();
        let response = (server.handler())(CorePipeRequest::StoreSettings {
            request_id: 92,
            values: BTreeMap::from([
                ("theme_mode".to_string(), Value::String("dark".into())),
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
                theme_mode,
                proxy_mode,
                ..
            } => {
                assert_eq!(
                    theme_mode, "system",
                    "the valid first item must roll back with the batch"
                );
                assert_eq!(proxy_mode, "system");
            }
            other => panic!("unexpected settings response: {other:?}"),
        }
        server.shutdown();
    }

    #[cfg(windows)]
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
                browser_connected,
            } => {
                assert_eq!(request_id, 95);
                assert_eq!(tasks.len(), 1);
                assert!(latest_sequence > 0);
                assert!(!browser_connected);
            }
            other => panic!("unexpected snapshot response: {other:?}"),
        }
        (server.handler())(CorePipeRequest::Command {
            request_id: 96,
            command: CoreCommand::BrowserHello {
                version: "7.0.3".into(),
                browser: "Firefox".into(),
            },
        });
        assert!(matches!(
            (server.handler())(CorePipeRequest::Snapshot { request_id: 97 }),
            CorePipeResponse::Snapshot {
                browser_connected: true,
                ..
            }
        ));
        server.shutdown();
    }

    #[test]
    fn credential_delete_roundtrips_over_ipc() {
        let server = CoreServer::in_memory().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let mut client = CoreIpcClient::connect_addr(addr).unwrap();
        client
            .store_credential("rollback-test", "protected", "browser_replay")
            .unwrap();
        assert_eq!(
            client.load_credential("rollback-test").unwrap().as_deref(),
            Some("protected")
        );
        client.delete_credential("rollback-test").unwrap();
        assert_eq!(client.load_credential("rollback-test").unwrap(), None);
        server.shutdown();
    }

    #[test]
    fn handoff_admission_ipc_retry_returns_same_receipt_and_one_task() {
        let server = CoreServer::in_memory().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let mut first = CoreIpcClient::connect_addr(addr).unwrap();
        let mut retry = CoreIpcClient::connect_addr(addr).unwrap();
        let admission = |id: &str| {
            CorePipeRequest::AdmitHandoff {
            request_id: 1,
            handoff_json: serde_json::json!({
                "id": id, "request_id": "same-request",
                "offer": ResourceOffer { handoff_id: id.into(), url: "https://cdn.test/setup.exe".into(), ..Default::default() },
                "filename": "setup.exe", "title": "", "mime_type": "", "size": 0,
                "status": "pending", "presentation": "queued", "task_id": null, "created_at_ms": 1
            }).to_string(),
        }
        };
        let original = first.request(&admission("first")).unwrap();
        assert_eq!(retry.request(&admission("duplicate")).unwrap(), original);
        assert_eq!(first.load_handoffs().unwrap().len(), 1);
        // Exercise the runtime half as well as the durable row without starting
        // network transfer: direct Core acceptance creates one task only.
        server
            .coordinator()
            .lock()
            .unwrap()
            .handle(CoreCommand::AcceptHandoff {
                handoff_id: "first".into(),
                filename: "setup.exe".into(),
                download_dir: String::new(),
                allow_duplicate: false,
                trusted_ui: true,
                start_paused: false,
                queue_id: String::new(),
                spec_override: None,
            })
            .unwrap();
        let receipt = retry.request(&admission("late")).unwrap();
        match receipt {
            CorePipeResponse::Handoffs { items, .. } => {
                let row: Value = serde_json::from_str(&items[0]).unwrap();
                assert_eq!(row["id"], "first");
                assert_eq!(row["status"], "accepted");
            }
            other => panic!("unexpected admission receipt: {other:?}"),
        }
        assert_eq!(retry.snapshot().unwrap().len(), 1);
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
            allow_duplicate: false,
            trusted_ui: true,
            start_paused: false,
            queue_id: String::new(),
            spec_override: None,
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

    #[test]
    fn capabilities_lists_task_inspection_and_thumbnail_commands() {
        let server = CoreServer::in_memory().unwrap();
        let handler = server.handler();
        match handler(CorePipeRequest::Capabilities { request_id: 7 }) {
            CorePipeResponse::Capabilities { commands, .. } => {
                for expected in [
                    "get_task",
                    "get_task_log",
                    "refresh_task_request",
                    "get_task_torrent_files",
                    "set_task_torrent_files",
                    "player_thumbnail",
                ] {
                    assert!(
                        commands.iter().any(|cmd| cmd == expected),
                        "Capabilities missing command {expected}: {commands:?}"
                    );
                }
            }
            other => panic!("expected Capabilities response, got {other:?}"),
        }
    }

    #[test]
    fn shutdown_emits_shutdown_event_to_connected_subscribers_and_tracks_count() {
        let server = CoreServer::in_memory().unwrap();
        assert!(!server.has_event_subscribers());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let worker = server.serve(listener);

        let mut subscriber = CoreIpcClient::connect_addr(addr).unwrap();
        let seq = server.coordinator().latest_sequence().unwrap();
        let sub_thread = thread::spawn(move || {
            subscriber.request(&CorePipeRequest::WaitEventBatch {
                request_id: 1,
                after_sequence: seq,
                timeout_ms: 5_000,
            })
        });

        // Wait until the subscriber enters wait_events.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !server.has_event_subscribers() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(server.has_event_subscribers());

        // Emit direct UiShow while subscriber is connected, then verify Shutdown delivery.
        let mut controller = CoreIpcClient::connect_addr(addr).unwrap();
        let shutdown_events = controller.command(CoreCommand::Shutdown).unwrap();
        assert!(shutdown_events
            .iter()
            .any(|env| matches!(env.event, CoreEvent::Shutdown)));

        let response = sub_thread.join().unwrap().unwrap();
        match response {
            CorePipeResponse::EventBatch { batch, .. } => {
                assert!(
                    batch
                        .events
                        .iter()
                        .any(|env| matches!(env.event, CoreEvent::Shutdown)),
                    "connected subscriber must receive CoreEvent::Shutdown before IPC closes: {batch:?}"
                );
            }
            other => panic!("expected EventBatch, got {other:?}"),
        }
        assert!(worker.join().unwrap().is_ok());
        assert!(!server.has_event_subscribers());
    }

    #[cfg(windows)]
    #[test]
    fn shutdown_wakes_blocked_named_pipe_listener_via_stop_handle() {
        let server = CoreServer::in_memory().unwrap();
        let pipe_name = format!(
            r"\\.\pipe\HLSDownloader.v7.stoptest.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let worker = server.start_named_pipe_listener(pipe_name.clone()).unwrap();

        let mut client = crate::NamedPipeClient::connect(&pipe_name, 2_000).unwrap();
        let events = client
            .request(&CorePipeRequest::Command {
                request_id: 1,
                command: CoreCommand::Ping,
            })
            .unwrap();
        assert!(matches!(events, CorePipeResponse::Events { .. }));
        drop(client);

        let started = std::time::Instant::now();
        server.shutdown();
        let joined = worker.join().expect("named pipe listener should not panic");
        assert!(
            joined.is_ok(),
            "named pipe listener returned error: {joined:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "NamedPipeStopHandle should wake ConnectNamedPipe promptly, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn get_task_is_read_only_and_preserves_sequence_and_sqlite_data_version() {
        let root = std::env::temp_dir().join(format!(
            "hls-v7-get-task-readonly-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("epoch")
                .as_nanos()
        ));
        let database = root.join("data.db");
        let server = CoreServer::open_path(&database).expect("open server");
        server
            .coordinator()
            .set_setting("legal_terms_accepted", Value::Bool(true))
            .expect("accept legal");
        server
            .coordinator()
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.example.test/read-only.bin".into(),
                    filename: "read-only.bin".into(),
                    start_paused: true,
                    ..Default::default()
                },
            })
            .expect("create paused task");

        let observer = rusqlite::Connection::open(&database).expect("open observer");
        let data_version = || {
            observer
                .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
                .expect("read data_version")
        };
        let base_seq = server.coordinator().latest_sequence().expect("base seq");
        let base_ver = data_version();

        let found = server
            .coordinator()
            .dispatch(CoreCommand::GetTask {
                task_id: "task-1".into(),
            })
            .expect("GetTask existing");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sequence, base_seq);
        assert!(matches!(found[0].event, CoreEvent::TaskUpdated { .. }));

        let missing = server
            .coordinator()
            .dispatch(CoreCommand::GetTask {
                task_id: "sftp://alice:secret@example.test/missing".into(),
            })
            .expect("GetTask missing");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].sequence, base_seq);
        match &missing[0].event {
            CoreEvent::Error { code, message } => {
                assert_eq!(code, "task_not_found");
                assert!(!message.contains("secret"));
            }
            other => panic!("expected task_not_found error, got {other:?}"),
        }

        assert_eq!(
            server.coordinator().latest_sequence().expect("after seq"),
            base_seq,
            "GetTask must not advance runtime sequence"
        );
        assert_eq!(data_version(), base_ver, "GetTask must not write to SQLite");
        drop(observer);
        server.shutdown_and_flush();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn non_ui_wait_event_batch_does_not_suppress_ui_wake_or_steal_install_result() {
        let server = CoreServer::in_memory().expect("in_memory server");
        let woke_modes = Arc::new(Mutex::new(Vec::new()));
        {
            let woke_modes = Arc::clone(&woke_modes);
            server.set_presenter_waker(Some(Arc::new(move |mode| {
                if let Ok(mut list) = woke_modes.lock() {
                    list.push(mode);
                }
            })));
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let worker = server.serve(listener);

        let mut host_sub = CoreIpcClient::connect_addr(addr).expect("connect host subscriber");
        let seq = server.coordinator().latest_sequence().expect("initial seq");
        let host_wait = thread::spawn(move || {
            host_sub.request(&CorePipeRequest::WaitEventBatch {
                request_id: 1,
                after_sequence: seq,
                timeout_ms: 400,
            })
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !server.has_event_subscribers() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(server.has_event_subscribers());
        assert!(
            !server.has_ui_subscribers(),
            "WaitEventBatch must not register as a UI subscriber"
        );

        server.queue_install_result(hls_client::update_install::InstallResult {
            version: "10.1.0".into(),
            status: "installed".into(),
            exit_code: 0,
            message: "升级成功".into(),
            install_log: String::new(),
        });
        deliver_clipboard_offer(
            server.coordinator(),
            &server.presenter_bridge,
            vec!["https://cdn.example.test/video.mp4".into()],
        );

        let host_resp = host_wait
            .join()
            .expect("join host wait")
            .expect("host WaitEventBatch reply");
        match host_resp {
            CorePipeResponse::EventBatch { batch, .. } => {
                assert!(
                    batch.events.is_empty(),
                    "non-UI WaitEventBatch must not steal queued UI events: {batch:?}"
                );
            }
            other => panic!("unexpected WaitEventBatch reply: {other:?}"),
        }
        assert_eq!(
            woke_modes.lock().expect("lock woke_modes").as_slice(),
            &[PresenterWakeMode::BackgroundToast],
            "clipboard offer must wake presenter when only non-UI subscriber is attached"
        );

        // Now a real UI presenter connects, takes Snapshot, then calls WaitEvents (flush_ui_queue=true).
        let mut ui = CoreIpcClient::connect_addr(addr).expect("connect ui");
        let snap_seq = match ui
            .request(&CorePipeRequest::Snapshot { request_id: 2 })
            .expect("ui snapshot")
        {
            CorePipeResponse::Snapshot {
                latest_sequence, ..
            } => latest_sequence,
            other => panic!("expected Snapshot, got {other:?}"),
        };
        let ui_events = match ui
            .request(&CorePipeRequest::WaitEvents {
                request_id: 3,
                after_sequence: snap_seq,
                timeout_ms: 1_000,
            })
            .expect("ui WaitEvents")
        {
            CorePipeResponse::Events { events, .. } => events,
            other => panic!("expected Events, got {other:?}"),
        };
        assert!(
            ui_events.iter().any(|env| matches!(
                &env.event,
                CoreEvent::UpdateInstallResult {
                    latest,
                    status,
                    ..
                } if latest == "10.1.0" && status == "installed"
            )),
            "UI WaitEvents must receive deferred UpdateInstallResult after Snapshot: {ui_events:?}"
        );
        assert!(
            ui_events.iter().any(
                |env| matches!(&env.event, CoreEvent::ClipboardOffer { urls } if urls.len() == 1)
            ),
            "UI WaitEvents must receive queued ClipboardOffer: {ui_events:?}"
        );

        server.shutdown();
        assert!(worker.join().expect("join server").is_ok());
    }

    #[test]
    fn shutdown_and_update_install_started_deliver_shutdown_to_ui_and_flush_checkpoint() {
        for via_install_update in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "hls-v7-shutdown-order-{}-{via_install_update}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("epoch")
                    .as_nanos()
            ));
            let database = root.join("data.db");
            let server = CoreServer::open_path(&database).expect("open server");
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
            let addr = listener.local_addr().expect("local addr");
            let worker = server.serve(listener);

            let mut ui_sub = CoreIpcClient::connect_addr(addr).expect("connect ui subscriber");
            let initial_seq = server.coordinator().latest_sequence().expect("initial seq");
            let ui_thread = thread::spawn(move || {
                ui_sub.request(&CorePipeRequest::WaitEvents {
                    request_id: 10,
                    after_sequence: initial_seq,
                    timeout_ms: 5_000,
                })
            });

            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !server.has_ui_subscribers() && std::time::Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(server.has_ui_subscribers());

            if via_install_update {
                server.trigger_update_install_shutdown_for_test();
            } else {
                let mut ctl = CoreIpcClient::connect_addr(addr).expect("connect ctl");
                ctl.command(CoreCommand::Shutdown)
                    .expect("send Shutdown command");
            }

            let shutdown_seq = match ui_thread
                .join()
                .expect("join ui_thread")
                .expect("WaitEvents response")
            {
                CorePipeResponse::Events { events, .. } => events
                    .iter()
                    .find_map(|env| {
                        matches!(env.event, CoreEvent::Shutdown).then_some(env.sequence)
                    })
                    .expect("UI subscriber must receive CoreEvent::Shutdown"),
                other => panic!("expected Events with Shutdown, got {other:?}"),
            };

            assert!(
                server.wait_until_stopped(Duration::from_secs(2)),
                "CoreServer must stop promptly (via_install_update={via_install_update})"
            );
            assert!(worker.join().expect("join server worker").is_ok());
            drop(server);

            let reopened = crate::CoreStore::open(&database).expect("reopen store");
            let persisted_seq = reopened
                .latest_sequence()
                .expect("read persisted checkpoint");
            assert!(
                persisted_seq >= shutdown_seq,
                "flush_for_shutdown must persist checkpoint >= Shutdown sequence ({persisted_seq} < {shutdown_seq})"
            );
            drop(reopened);
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn queue_scheduler_latch_fires_across_calendar_days_and_honors_weekday_and_toggle_reset() {
        let mut latch = QueueSchedulerLatch::default();

        // Day 1 at 01:00 fires start once, then suppresses duplicate ticks in the same minute.
        assert_eq!(
            latch.step((20261126, "01:00"), true, true, "01:00", true, "07:30"),
            (false, true)
        );
        assert_eq!(
            latch.step((20261126, "01:00"), true, true, "01:00", true, "07:30"),
            (false, false)
        );

        // Day 1 at 07:30 fires stop once.
        assert_eq!(
            latch.step((20261126, "07:30"), true, true, "01:00", true, "07:30"),
            (true, false)
        );
        assert_eq!(
            latch.step((20261126, "07:30"), true, true, "01:00", true, "07:30"),
            (false, false)
        );

        // Day 2 at the exact same HH:mm MUST fire again (no multi-day permanent lock-up).
        assert_eq!(
            latch.step((20261127, "01:00"), true, true, "01:00", true, "07:30"),
            (false, true)
        );
        assert_eq!(
            latch.step((20261127, "07:30"), true, true, "01:00", true, "07:30"),
            (true, false)
        );

        // Disallowed weekday suppresses both edges without poisoning a later allowed day.
        assert_eq!(
            latch.step((20261128, "01:00"), false, true, "01:00", true, "07:30"),
            (false, false)
        );
        assert_eq!(
            latch.step((20261128, "01:00"), true, true, "01:00", true, "07:30"),
            (false, true)
        );

        // Toggling off and back on within the same minute clears the latch so re-enable works.
        assert_eq!(
            latch.step((20261128, "01:00"), true, false, "01:00", true, "07:30"),
            (false, false)
        );
        assert_eq!(
            latch.step((20261128, "01:00"), true, true, "01:00", true, "07:30"),
            (false, true)
        );
    }
}
