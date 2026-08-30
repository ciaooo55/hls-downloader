#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

use hls_native_shell::{
    activate_window_by_title, begin_caption_drag, center_window_by_title,
    claim_v7_presenter_instance, completion_sound, hide_window_from_taskbar_by_title, install_root,
    os_reduce_motion, spawn_core, spawn_desktop_ui, CoreCommand, CoreEvent, CoreIpcClient,
    CorePipeResponse, EventEnvelope, ResourceKind, ResourceOffer, TaskSnapshot,
};
use serde::Deserialize;
use slint::{ComponentHandle, RenderingState, Timer, TimerMode};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::env;
use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

#[derive(Deserialize)]
struct PersistedHandoff {
    #[serde(default)]
    status: String,
    #[serde(default)]
    presentation: String,
    offer: ResourceOffer,
}

#[derive(Clone, Default)]
struct PresenterSettings {
    download_dir: String,
    category_media: String,
    category_program: String,
    category_archive: String,
    category_other: String,
}

fn presenter_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        format!("presenter-{}-{started}", std::process::id())
    })
}

impl PresenterSettings {
    fn from_response(response: Option<&CorePipeResponse>) -> Self {
        let Some(CorePipeResponse::Settings {
            download_dir,
            category_dir_media,
            category_dir_program,
            category_dir_archive,
            category_dir_other,
            ..
        }) = response
        else {
            return Self::default();
        };
        Self {
            download_dir: download_dir.clone(),
            category_media: category_dir_media.clone(),
            category_program: category_dir_program.clone(),
            category_archive: category_dir_archive.clone(),
            category_other: category_dir_other.clone(),
        }
    }

    fn directory_for(&self, category: &str) -> String {
        let configured = match category {
            "media" => &self.category_media,
            "program" => &self.category_program,
            "archive" => &self.category_archive,
            _ => &self.category_other,
        };
        if configured.trim().is_empty() {
            self.download_dir.clone()
        } else {
            configured.clone()
        }
    }

    fn remember(&mut self, category: &str, directory: String) {
        match category {
            "media" => self.category_media = directory,
            "program" => self.category_program = directory,
            "archive" => self.category_archive = directory,
            _ => self.category_other = directory,
        }
    }

    fn category_dirs_json(&self) -> String {
        serde_json::json!({
            "media": self.category_media,
            "program": self.category_program,
            "archive": self.category_archive,
            "other": self.category_other,
        })
        .to_string()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if let Some(kind) = args
        .windows(2)
        .find(|pair| pair[0] == "--visual-fixture")
        .map(|pair| pair[1].as_str())
    {
        return run_visual_fixture(kind, args.iter().any(|arg| arg == "--dark"));
    }
    if args.iter().any(|arg| arg == "--self-test") {
        let confirm = ConfirmWindow::new()?;
        confirm.hide()?;
        attach_parent_console();
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "hls-v7-presenter/1 ok");
        let _ = stdout.flush();
        return Ok(());
    }
    let lock_test = args.iter().any(|arg| arg == "--lock-test");
    if let Err(error) = claim_v7_presenter_instance() {
        if error.contains("already running") && !lock_test {
            return Ok(());
        }
        return Err(error.into());
    }
    if lock_test {
        thread::sleep(Duration::from_secs(1));
        return Ok(());
    }

    let mut initial_client = connect_or_start_core()?;
    let (initial_tasks, initial_sequence) = initial_client.snapshot_state()?;
    let initial_pending = load_pending_offers(&mut initial_client);
    let settings = initial_client.load_settings().ok();
    let presenter_settings = Arc::new(Mutex::new(PresenterSettings::from_response(
        settings.as_ref(),
    )));
    let known_tasks = Arc::new(Mutex::new(initial_tasks));
    let runtime_settings = Rc::new(RefCell::new(PresenterRuntimeSettings::from_response(
        settings.as_ref(),
    )));
    let initial_runtime = *runtime_settings.borrow();

    let confirm = ConfirmWindow::new()?;
    let progress = ProgressWindow::new()?;
    let complete = CompleteWindow::new()?;
    confirm.hide()?;
    progress.hide()?;
    complete.hide()?;
    apply_runtime_settings(&confirm, &progress, &complete, initial_runtime);

    let prewarm_rendered = Arc::new(AtomicBool::new(false));
    confirm.window().set_rendering_notifier({
        let prewarm_rendered = Arc::clone(&prewarm_rendered);
        move |state, _| {
            if matches!(state, RenderingState::AfterRendering) {
                prewarm_rendered.store(true, Ordering::Release);
            }
        }
    })?;

    let client = Rc::new(RefCell::new(initial_client));
    let pending = Arc::new(Mutex::new(initial_pending));
    let active_handoff = Arc::new(Mutex::new(None::<String>));
    let active_task = Rc::new(RefCell::new(None::<String>));
    let completed_task = Rc::new(RefCell::new(None::<String>));
    let completed_notified = Rc::new(RefCell::new(HashSet::<String>::new()));
    let prewarm_finished = Rc::new(RefCell::new(false));

    progress.on_command({
        let client = Rc::clone(&client);
        let active_task = Rc::clone(&active_task);
        let window = progress.as_weak();
        move |command| {
            let action = command.to_string();
            if action == "hide_progress" {
                if let Some(item) = window.upgrade() {
                    let _ = item.hide();
                }
                return;
            }
            if matches!(action.as_str(), "pause" | "cancel") {
                if let Some(task_id) = active_task.borrow().clone() {
                    let _ = command_with_reconnect(
                        &client,
                        CoreCommand::TaskAction { task_id, action },
                    );
                }
            }
        }
    });
    confirm.on_command({
        let client = Rc::clone(&client);
        let pending = Arc::clone(&pending);
        let active_handoff = Arc::clone(&active_handoff);
        let presenter_settings = Arc::clone(&presenter_settings);
        let known_tasks = Arc::clone(&known_tasks);
        let window = confirm.as_weak();
        move |command| {
            let command = command.to_string();
            match command.as_str() {
                "drag" => {
                    let _ = begin_caption_drag("确认下载");
                    return;
                }
                "accept" => {
                    let Some(offer) = pending.lock().ok().and_then(|items| items.front().cloned())
                    else {
                        return;
                    };
                    let Some(item) = window.upgrade() else {
                        return;
                    };
                    let filename = item.get_filename().trim().to_string();
                    let download_dir = item.get_download_dir().trim().to_string();
                    let category = item.get_category().to_string();
                    if filename.is_empty() {
                        item.set_error_text("请输入文件名".into());
                        return;
                    }
                    item.set_busy(true);
                    item.set_error_text("".into());
                    match command_with_reconnect(
                        &client,
                        CoreCommand::AcceptHandoff {
                            handoff_id: offer.handoff_id.clone(),
                            filename,
                            download_dir: download_dir.clone(),
                            trusted_ui: true,
                        },
                    ) {
                        Ok(_) => {
                            if item.get_remember_directory() && !download_dir.is_empty() {
                                if let Ok(mut settings) = presenter_settings.lock() {
                                    settings.remember(&category, download_dir);
                                    let _ = command_with_reconnect(
                                        &client,
                                        CoreCommand::SetSetting {
                                            key: "browser_category_dirs".into(),
                                            value: serde_json::Value::String(
                                                settings.category_dirs_json(),
                                            ),
                                        },
                                    );
                                }
                            }
                            pending.lock().ok().and_then(|mut items| items.pop_front());
                        }
                        Err(error) => {
                            item.set_busy(false);
                            item.set_error_text(error.into());
                            return;
                        }
                    }
                }
                "reject" => {
                    if let Some(offer) =
                        pending.lock().ok().and_then(|items| items.front().cloned())
                    {
                        let result = command_with_reconnect(
                            &client,
                            CoreCommand::RejectHandoff {
                                handoff_id: offer.handoff_id.clone(),
                                suppress_site_kind: window
                                    .upgrade()
                                    .map(|item| item.get_suppress_site_kind())
                                    .unwrap_or(false),
                            },
                        );
                        match result {
                            Ok(_) => {
                                pending.lock().ok().and_then(|mut items| items.pop_front());
                            }
                            Err(error) => {
                                if let Some(item) = window.upgrade() {
                                    item.set_error_text(error.into());
                                }
                            }
                        }
                    }
                }
                "browse" => {
                    let Some(item) = window.upgrade() else {
                        return;
                    };
                    match pick_folder(&item.get_download_dir()) {
                        Ok(Some(path)) => item.set_download_dir(path.into()),
                        Ok(None) => {}
                        Err(error) => item.set_error_text(error.into()),
                    }
                    return;
                }
                "more" => {
                    let opened = install_root()
                        .map(|root| spawn_desktop_ui(&root))
                        .unwrap_or(false);
                    let _ = command_with_reconnect(&client, CoreCommand::OpenMain);
                    if let Some(item) = window.upgrade() {
                        item.set_error_text(if opened {
                            "已打开主窗口；当前请求仍可在此确认".into()
                        } else {
                            "主窗口已收到显示请求".into()
                        });
                    }
                    return;
                }
                _ if command.starts_with("category:") => {
                    let category = command.trim_start_matches("category:");
                    if let (Some(item), Ok(settings)) =
                        (window.upgrade(), presenter_settings.lock())
                    {
                        item.set_download_dir(settings.directory_for(category).into());
                    }
                    return;
                }
                _ => return,
            }
            show_next_offer(
                &window,
                &pending,
                &active_handoff,
                &presenter_settings,
                &known_tasks,
            );
        }
    });
    complete.on_command({
        let client = Rc::clone(&client);
        let completed_task = Rc::clone(&completed_task);
        let window = complete.as_weak();
        move |command| {
            if command == "cancel_power" {
                let _ = command_with_reconnect(&client, CoreCommand::CancelPowerAction);
                if let Some(item) = window.upgrade() {
                    item.set_power_hint("".into());
                }
                return;
            }
            if let Some(task_id) = completed_task.borrow().clone() {
                let folder = command == "open_folder";
                if folder || command == "open_file" {
                    let _ = command_with_reconnect(
                        &client,
                        CoreCommand::OpenCompleted { task_id, folder },
                    );
                }
            }
            if let Some(item) = window.upgrade() {
                let _ = item.hide();
            }
        }
    });

    let (tx, rx) = mpsc::channel::<Vec<EventEnvelope>>();
    thread::spawn({
        let confirm = confirm.as_weak();
        let pending = Arc::clone(&pending);
        let active_handoff = Arc::clone(&active_handoff);
        let presenter_settings = Arc::clone(&presenter_settings);
        let known_tasks = Arc::clone(&known_tasks);
        move || {
            event_loop(
                tx,
                initial_sequence,
                confirm,
                pending,
                active_handoff,
                presenter_settings,
                known_tasks,
            )
        }
    });
    let rx = Rc::new(RefCell::new(rx));
    let event_timer = Timer::default();
    event_timer.start(TimerMode::Repeated, Duration::from_millis(4), {
        let rx = Rc::clone(&rx);
        let pending = Arc::clone(&pending);
        let active_task = Rc::clone(&active_task);
        let completed_task = Rc::clone(&completed_task);
        let completed_notified = Rc::clone(&completed_notified);
        let client = Rc::clone(&client);
        let runtime_settings = Rc::clone(&runtime_settings);
        let confirm = confirm.as_weak();
        let progress = progress.as_weak();
        let complete = complete.as_weak();
        let prewarm_rendered = Arc::clone(&prewarm_rendered);
        let prewarm_finished = Rc::clone(&prewarm_finished);
        move || {
            if prewarm_rendered.load(Ordering::Acquire) && !*prewarm_finished.borrow() {
                let _ = hide_window_from_taskbar_by_title("确认下载");
                let _ = center_window_by_title("确认下载");
                if let Some(item) = confirm.upgrade() {
                    if pending.lock().map(|items| items.is_empty()).unwrap_or(true) {
                        let _ = item.hide();
                    }
                }
                *prewarm_finished.borrow_mut() = true;
                write_ready_marker();
            }
            while let Ok(events) = rx.borrow_mut().try_recv() {
                for envelope in events {
                    match envelope.event {
                        CoreEvent::HandoffOffered { .. } | CoreEvent::HandoffResolved { .. } => {}
                        CoreEvent::SettingsChanged { .. } => {
                            if let Ok(response) = client.borrow_mut().load_settings() {
                                let next = PresenterRuntimeSettings::from_response(Some(&response));
                                *runtime_settings.borrow_mut() = next;
                                if let (Some(confirm), Some(progress), Some(complete)) =
                                    (confirm.upgrade(), progress.upgrade(), complete.upgrade())
                                {
                                    apply_runtime_settings(&confirm, &progress, &complete, next);
                                    if !next.show_progress {
                                        let _ = progress.hide();
                                    }
                                    if !next.show_complete {
                                        let _ = complete.hide();
                                    }
                                }
                            }
                        }
                        CoreEvent::TaskCreated { snapshot }
                        | CoreEvent::TaskUpdated { snapshot }
                        | CoreEvent::TaskProgress { snapshot } => update_task_windows(
                            snapshot,
                            *runtime_settings.borrow(),
                            &progress,
                            &complete,
                            &active_task,
                            &completed_task,
                            &completed_notified,
                        ),
                        CoreEvent::PowerActionPending {
                            action,
                            title,
                            delay_seconds,
                        } => {
                            if let Some(item) = complete.upgrade() {
                                item.set_filename(title.into());
                                item.set_power_hint(
                                    format!(
                                        "{} 将在 {} 秒后执行",
                                        power_label(&action),
                                        delay_seconds
                                    )
                                    .into(),
                                );
                                if item.show().is_ok() {
                                    let _ = center_window_by_title("下载完成");
                                    let _ = activate_window_by_title("下载完成");
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // Creating a hidden Slint window does not initialize its renderer. Present one
    // frame off-screen so the first browser handoff only pays IPC and
    // visibility latency, not font/layout/graphics initialization.
    let prewarm_timer = Timer::default();
    // A zero-duration Slint timer is treated as stopped by some backends.
    // Schedule the prewarm on the first real event-loop tick instead.
    prewarm_timer.start(TimerMode::SingleShot, Duration::from_millis(1), {
        let confirm = confirm.as_weak();
        let pending = Arc::clone(&pending);
        let prewarm_finished = Rc::clone(&prewarm_finished);
        move || {
            if !pending.lock().map(|items| items.is_empty()).unwrap_or(true) {
                return;
            }
            let Some(item) = confirm.upgrade() else {
                return;
            };
            item.window()
                .set_position(slint::PhysicalPosition::new(-32_000, -32_000));
            if item.show().is_ok() {
                let _ = hide_window_from_taskbar_by_title("确认下载");
                // `show()` initializes the native window and renderer. Some
                // Windows/driver combinations deliberately skip rendering
                // notifications for a fully off-screen window, which used to
                // leave the presenter permanently "not ready" even though the
                // first real popup worked. The latency smoke still measures
                // the first real visible frame, so marking the completed
                // off-screen show here is both deterministic and honest.
                *prewarm_finished.borrow_mut() = true;
                write_ready_marker();
                let _ = item.hide();
            }
        }
    });

    show_next_offer(
        &confirm.as_weak(),
        &pending,
        &active_handoff,
        &presenter_settings,
        &known_tasks,
    );
    slint::run_event_loop_until_quit()?;
    event_timer.stop();
    prewarm_timer.stop();
    Ok(())
}

/// Render the real production popup components without Core IPC so automated
/// visual tests can inspect the first visible frame.  This closes the gap left
/// by `--self-test`, which intentionally checks only component construction.
fn run_visual_fixture(kind: &str, dark: bool) -> Result<(), Box<dyn std::error::Error>> {
    let quit_timer = Timer::default();
    quit_timer.start(TimerMode::SingleShot, Duration::from_secs(20), || {
        let _ = slint::quit_event_loop();
    });

    match kind {
        "confirm" => {
            let window = ConfirmWindow::new()?;
            window.global::<Tokens>().set_dark(dark);
            window.global::<Tokens>().set_reduce_motion(true);
            window.set_filename("示例视频 · 1080p.mp4".into());
            window.set_download_dir(r"D:\下载\媒体".into());
            window.set_url("media.example.test/library/example-video.mp4".into());
            window.set_source_url("video.example.test/watch/42".into());
            window.set_source_host("video.example.test".into());
            window.set_download_host("media.example.test".into());
            window.set_resource_meta("HTTP · .mp4 · video/mp4 · 128.0 MB".into());
            window.set_request_context("已安全继承网页凭据".into());
            window.set_request_details(
                "GET · 支持 Referer / Origin / User-Agent / Cookie / Authorization".into(),
            );
            window.set_category("media".into());
            window.set_duplicate_text("已有同一地址的任务：示例视频.mp4（已暂停）".into());
            window.set_remaining("  ·  后面还有 2 个".into());
            window.on_command(|command| {
                if command != "drag" {
                    let _ = slint::quit_event_loop();
                }
            });
            window.show()?;
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "progress" => {
            let window = ProgressWindow::new()?;
            window.global::<Tokens>().set_dark(dark);
            window.global::<Tokens>().set_reduce_motion(true);
            window.set_headline("下载进度".into());
            window.set_filename("示例视频 · 1080p.mp4".into());
            window.set_speed("18.6 MB/s".into());
            window.set_progress(0.64);
            window.on_command(|_| {
                let _ = slint::quit_event_loop();
            });
            window.show()?;
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "complete" => {
            let window = CompleteWindow::new()?;
            window.global::<Tokens>().set_dark(dark);
            window.global::<Tokens>().set_reduce_motion(true);
            window.set_filename("示例视频 · 1080p.mp4".into());
            window.set_power_hint("将在 5 分钟后关机".into());
            window.on_command(|_| {
                let _ = slint::quit_event_loop();
            });
            window.show()?;
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        _ => return Err(format!("unknown visual fixture: {kind}").into()),
    }
    quit_timer.stop();
    Ok(())
}

fn write_ready_marker() {
    let Some(path) = env::var_os("HLS_V7_PRESENTER_READY_FILE") else {
        return;
    };
    let _ = std::fs::write(path, b"ready\n");
}

fn connect_or_start_core() -> Result<CoreIpcClient, String> {
    if let Ok(client) = CoreIpcClient::connect() {
        return Ok(client);
    }
    let root = install_root().ok_or_else(|| "找不到应用安装目录".to_string())?;
    spawn_core(&root)?;
    for _ in 0..40 {
        if let Ok(client) = CoreIpcClient::connect() {
            return Ok(client);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("下载引擎启动超时".into())
}

fn command_with_reconnect(
    client: &RefCell<CoreIpcClient>,
    command: CoreCommand,
) -> Result<Vec<EventEnvelope>, String> {
    let first_result = client.borrow_mut().command(command.clone());
    if first_result.is_ok() {
        return first_result;
    }
    let mut replacement = CoreIpcClient::connect()?;
    let result = replacement.command(command);
    *client.borrow_mut() = replacement;
    result
}

fn event_loop(
    tx: mpsc::Sender<Vec<EventEnvelope>>,
    mut after: u64,
    confirm: slint::Weak<ConfirmWindow>,
    pending: Arc<Mutex<VecDeque<ResourceOffer>>>,
    active_handoff: Arc<Mutex<Option<String>>>,
    presenter_settings: Arc<Mutex<PresenterSettings>>,
    known_tasks: Arc<Mutex<Vec<TaskSnapshot>>>,
) {
    loop {
        let Ok(mut client) = CoreIpcClient::connect() else {
            thread::sleep(Duration::from_millis(200));
            continue;
        };
        let Ok((tasks, latest_sequence)) = client.snapshot_state() else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        if let Ok(mut known_tasks) = known_tasks.lock() {
            *known_tasks = tasks;
        }
        let restored = load_pending_offers(&mut client);
        let resync_pending = Arc::clone(&pending);
        let resync_active_handoff = Arc::clone(&active_handoff);
        let resync_settings = Arc::clone(&presenter_settings);
        let resync_tasks = Arc::clone(&known_tasks);
        if confirm
            .upgrade_in_event_loop(move |item| {
                if let Ok(mut items) = resync_pending.lock() {
                    *items = restored;
                }
                show_next_offer(
                    &item.as_weak(),
                    &resync_pending,
                    &resync_active_handoff,
                    &resync_settings,
                    &resync_tasks,
                );
            })
            .is_err()
        {
            return;
        }
        if latest_sequence < after {
            trace(&format!(
                "core sequence reset from {after} to {latest_sequence}; resynchronizing event cursor"
            ));
            after = latest_sequence;
        }
        trace(&format!("event client connected after={after}"));
        let mut last_lease_renewal = std::time::Instant::now();
        loop {
            match client.wait_events(after, 5_000) {
                Ok(events) => {
                    if let Some(last) = events.last() {
                        after = last.sequence;
                    }
                    if !events.is_empty() {
                        trace(&format!("received {} events through {after}", events.len()));
                    }
                    let mut batched = Vec::new();
                    for envelope in events {
                        match envelope.event {
                            CoreEvent::HandoffOffered { offer } => {
                                let pending = Arc::clone(&pending);
                                let active_handoff = Arc::clone(&active_handoff);
                                let presenter_settings = Arc::clone(&presenter_settings);
                                let known_tasks = Arc::clone(&known_tasks);
                                if confirm
                                    .upgrade_in_event_loop(move |item| {
                                        if let Ok(mut items) = pending.lock() {
                                            if !items
                                                .iter()
                                                .any(|entry| entry.handoff_id == offer.handoff_id)
                                            {
                                                items.push_back(offer);
                                            }
                                        }
                                        show_next_offer(
                                            &item.as_weak(),
                                            &pending,
                                            &active_handoff,
                                            &presenter_settings,
                                            &known_tasks,
                                        );
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            CoreEvent::HandoffResolved { handoff_id, .. } => {
                                let pending = Arc::clone(&pending);
                                let active_handoff = Arc::clone(&active_handoff);
                                let presenter_settings = Arc::clone(&presenter_settings);
                                let known_tasks = Arc::clone(&known_tasks);
                                if confirm
                                    .upgrade_in_event_loop(move |item| {
                                        if let Ok(mut items) = pending.lock() {
                                            items.retain(|entry| entry.handoff_id != handoff_id);
                                        }
                                        show_next_offer(
                                            &item.as_weak(),
                                            &pending,
                                            &active_handoff,
                                            &presenter_settings,
                                            &known_tasks,
                                        );
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            event => {
                                update_known_tasks(&known_tasks, &event);
                                batched.push(EventEnvelope {
                                    sequence: envelope.sequence,
                                    event,
                                });
                            }
                        }
                    }
                    if !batched.is_empty() {
                        if tx.send(batched).is_err() {
                            return;
                        }
                        let _ = slint::invoke_from_event_loop(|| {});
                    }
                    if last_lease_renewal.elapsed() >= Duration::from_secs(5) {
                        let active = active_handoff.lock().ok().and_then(|value| value.clone());
                        if let Some(handoff_id) = active {
                            if client
                                .command(CoreCommand::PresentHandoff {
                                    handoff_id,
                                    ok: true,
                                    presenter_id: presenter_id().into(),
                                })
                                .is_err()
                            {
                                if let Ok(mut value) = active_handoff.lock() {
                                    *value = None;
                                }
                                let _ = confirm.upgrade_in_event_loop(|item| {
                                    let _ = item.hide();
                                });
                                break;
                            }
                        } else if pending
                            .lock()
                            .map(|items| !items.is_empty())
                            .unwrap_or(false)
                        {
                            let pending = Arc::clone(&pending);
                            let active_handoff = Arc::clone(&active_handoff);
                            let presenter_settings = Arc::clone(&presenter_settings);
                            let known_tasks = Arc::clone(&known_tasks);
                            let _ = confirm.upgrade_in_event_loop(move |item| {
                                show_next_offer(
                                    &item.as_weak(),
                                    &pending,
                                    &active_handoff,
                                    &presenter_settings,
                                    &known_tasks,
                                );
                            });
                        }
                        last_lease_renewal = std::time::Instant::now();
                    }
                }
                Err(_) => break,
            }
        }
    }
}

fn load_pending_offers(client: &mut CoreIpcClient) -> VecDeque<ResourceOffer> {
    client
        .load_handoffs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|encoded| serde_json::from_str::<PersistedHandoff>(&encoded).ok())
        .filter(|handoff| handoff.status == "pending" && handoff.presentation != "fallback")
        .map(|handoff| handoff.offer)
        .fold(VecDeque::new(), |mut pending, offer| {
            if !pending
                .iter()
                .any(|item| item.handoff_id == offer.handoff_id)
            {
                pending.push_back(offer);
            }
            pending
        })
}

fn show_next_offer(
    window: &slint::Weak<ConfirmWindow>,
    pending: &Arc<Mutex<VecDeque<ResourceOffer>>>,
    active_handoff: &Arc<Mutex<Option<String>>>,
    presenter_settings: &Arc<Mutex<PresenterSettings>>,
    known_tasks: &Arc<Mutex<Vec<TaskSnapshot>>>,
) {
    let Some(item) = window.upgrade() else {
        return;
    };
    let Some(offer) = pending.lock().ok().and_then(|items| items.front().cloned()) else {
        if let Ok(mut active) = active_handoff.lock() {
            *active = None;
        }
        let _ = item.hide();
        return;
    };
    trace(&format!("showing handoff {}", offer.handoff_id));
    let filename = if offer.filename.trim().is_empty() {
        filename_from_url(&offer.url)
    } else {
        offer.filename.clone()
    };
    let category = download_category(&filename, offer.resource_kind);
    let download_dir = presenter_settings
        .lock()
        .map(|settings| settings.directory_for(category))
        .unwrap_or_default();
    let duplicate_text = known_tasks
        .lock()
        .ok()
        .and_then(|tasks| {
            tasks
                .iter()
                .find(|task| canonical_url(&task.url) == canonical_url(&offer.url))
                .map(|task| {
                    format!(
                        "已有同一地址的任务：{}（{}）",
                        task.filename,
                        task_status_label(&task.status)
                    )
                })
        })
        .unwrap_or_default();
    item.set_filename(filename.clone().into());
    item.set_download_dir(download_dir.into());
    item.set_category(category.into());
    item.set_url(safe_display_url(&offer.url).into());
    item.set_source_url(safe_display_url(&offer.source_page_url).into());
    item.set_source_host(url_host(&offer.source_page_url).into());
    item.set_download_host(url_host(&offer.url).into());
    item.set_resource_meta(format_resource_meta(&offer, &filename).into());
    item.set_request_context(
        if offer.credential_ref.is_some() || offer.replay_context_ref.is_some() {
            "已安全继承网页凭据".into()
        } else {
            "已继承来源页面".into()
        },
    );
    item.set_request_details(format_request_details(&offer).into());
    item.set_duplicate_text(duplicate_text.into());
    item.set_remember_directory(true);
    item.set_suppress_site_kind(false);
    item.set_busy(false);
    item.set_error_text("".into());
    let remaining = pending
        .lock()
        .map(|items| items.len().saturating_sub(1))
        .unwrap_or_default();
    item.set_remaining(if remaining > 0 {
        format!("  ·  还有 {remaining} 个待确认").into()
    } else {
        "".into()
    });
    let claimed = CoreIpcClient::connect().and_then(|mut client| {
        client.command(CoreCommand::PresentHandoff {
            handoff_id: offer.handoff_id.clone(),
            ok: true,
            presenter_id: presenter_id().into(),
        })
    });
    if claimed.is_err() {
        if let Ok(mut client) = CoreIpcClient::connect() {
            let restored = load_pending_offers(&mut client);
            if let Ok(mut items) = pending.lock() {
                *items = restored;
            }
        }
        if let Ok(mut active) = active_handoff.lock() {
            *active = None;
        }
        let _ = item.hide();
        return;
    }
    let shown = item.show().is_ok();
    if shown {
        if let Ok(mut active) = active_handoff.lock() {
            *active = Some(offer.handoff_id.clone());
        }
        let _ = hide_window_from_taskbar_by_title("确认下载");
        let _ = center_window_by_title("确认下载");
        let _ = activate_window_by_title("确认下载");
    } else if let Ok(mut active) = active_handoff.lock() {
        *active = None;
    }
    let reported = CoreIpcClient::connect()
        .and_then(|mut client| {
            client.command(CoreCommand::PresentHandoff {
                handoff_id: offer.handoff_id.clone(),
                ok: shown,
                presenter_id: presenter_id().into(),
            })
        })
        .is_ok();
    if !shown && reported {
        if let Ok(mut items) = pending.lock() {
            items.retain(|item| item.handoff_id != offer.handoff_id);
        }
        if let Some(root) = install_root() {
            let _ = spawn_desktop_ui(&root);
        }
    }
}

fn update_known_tasks(tasks: &Arc<Mutex<Vec<TaskSnapshot>>>, event: &CoreEvent) {
    let Ok(mut tasks) = tasks.lock() else {
        return;
    };
    match event {
        CoreEvent::TaskCreated { snapshot }
        | CoreEvent::TaskUpdated { snapshot }
        | CoreEvent::TaskProgress { snapshot } => {
            if let Some(index) = tasks
                .iter()
                .position(|item| item.task_id == snapshot.task_id)
            {
                tasks[index] = snapshot.clone();
            } else {
                tasks.push(snapshot.clone());
            }
        }
        CoreEvent::TaskDeleted { task_id } => tasks.retain(|item| &item.task_id != task_id),
        _ => {}
    }
}

fn trace(message: &str) {
    if env::var_os("HLS_V7_PRESENTER_TRACE").is_some() {
        eprintln!("[presenter] {message}");
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PresenterRuntimeSettings {
    show_progress: bool,
    show_complete: bool,
    sound_enabled: bool,
    dark: bool,
    reduce_motion: bool,
}

impl PresenterRuntimeSettings {
    fn from_response(response: Option<&CorePipeResponse>) -> Self {
        match response {
            Some(CorePipeResponse::Settings {
                progress_window_enabled,
                complete_popup_enabled,
                completion_sound_enabled,
                dark_mode,
                reduce_motion,
                ..
            }) => Self {
                show_progress: *progress_window_enabled,
                show_complete: *complete_popup_enabled,
                sound_enabled: *completion_sound_enabled,
                dark: *dark_mode,
                reduce_motion: *reduce_motion || os_reduce_motion(),
            },
            _ => Self {
                reduce_motion: os_reduce_motion(),
                ..Self::default()
            },
        }
    }
}

fn apply_runtime_settings(
    confirm: &ConfirmWindow,
    progress: &ProgressWindow,
    complete: &CompleteWindow,
    settings: PresenterRuntimeSettings,
) {
    confirm.global::<Tokens>().set_dark(settings.dark);
    confirm
        .global::<Tokens>()
        .set_reduce_motion(settings.reduce_motion);
    progress.global::<Tokens>().set_dark(settings.dark);
    progress
        .global::<Tokens>()
        .set_reduce_motion(settings.reduce_motion);
    complete.global::<Tokens>().set_dark(settings.dark);
    complete
        .global::<Tokens>()
        .set_reduce_motion(settings.reduce_motion);
}

#[allow(clippy::too_many_arguments)]
fn update_task_windows(
    snapshot: TaskSnapshot,
    settings: PresenterRuntimeSettings,
    progress: &slint::Weak<ProgressWindow>,
    complete: &slint::Weak<CompleteWindow>,
    active_task: &RefCell<Option<String>>,
    completed_task: &RefCell<Option<String>>,
    completed_notified: &RefCell<HashSet<String>>,
) {
    if matches!(
        snapshot.status.as_str(),
        "downloading" | "recording" | "merging" | "checking"
    ) {
        *active_task.borrow_mut() = Some(snapshot.task_id.clone());
        if settings.show_progress {
            if let Some(item) = progress.upgrade() {
                item.set_headline(
                    if matches!(snapshot.status.as_str(), "merging" | "checking") {
                        "本地处理中".into()
                    } else {
                        "下载进度".into()
                    },
                );
                item.set_filename(snapshot.filename.clone().into());
                item.set_speed(if snapshot.speed_bytes_per_sec > 0 {
                    format_speed(snapshot.speed_bytes_per_sec).into()
                } else {
                    snapshot.stage.clone().into()
                });
                item.set_progress(task_progress(&snapshot));
                let _ = item.show();
            }
        }
        return;
    }
    if active_task.borrow().as_deref() == Some(snapshot.task_id.as_str()) {
        *active_task.borrow_mut() = None;
        if let Some(item) = progress.upgrade() {
            let _ = item.hide();
        }
    }
    if matches!(snapshot.status.as_str(), "completed" | "done")
        && completed_notified
            .borrow_mut()
            .insert(snapshot.task_id.clone())
    {
        *completed_task.borrow_mut() = Some(snapshot.task_id.clone());
        if settings.sound_enabled {
            completion_sound();
        }
        if settings.show_complete {
            if let Some(item) = complete.upgrade() {
                item.set_filename(snapshot.filename.into());
                item.set_power_hint("".into());
                if item.show().is_ok() {
                    let _ = center_window_by_title("下载完成");
                    let _ = activate_window_by_title("下载完成");
                }
            }
        }
    }
}

fn filename_from_url(url: &str) -> String {
    url.split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .find(|item| !item.is_empty())
        .unwrap_or("download")
        .to_string()
}

fn safe_display_url(value: &str) -> String {
    let without_tail = value.split(['?', '#']).next().unwrap_or(value).trim();
    let Some((_, location)) = without_tail.split_once("://") else {
        return without_tail.chars().take(180).collect();
    };
    let (authority, path) = location.split_once('/').unwrap_or((location, ""));
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let tail = path
        .split('/')
        .filter(|part| !part.is_empty())
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("/");
    let shown = if tail.is_empty() {
        host.to_string()
    } else {
        format!("{host}/{tail}")
    };
    shown.chars().take(180).collect()
}

fn url_host(value: &str) -> String {
    let location = value
        .split_once("://")
        .map(|(_, tail)| tail)
        .unwrap_or(value);
    location
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn download_category(filename: &str, kind: ResourceKind) -> &'static str {
    if matches!(
        kind,
        ResourceKind::Hls | ResourceKind::Dash | ResourceKind::Live
    ) {
        return "media";
    }
    let extension = file_extension(filename);
    if matches!(
        extension.as_str(),
        "mp4"
            | "mkv"
            | "webm"
            | "mov"
            | "avi"
            | "m4v"
            | "ts"
            | "mp3"
            | "m4a"
            | "flac"
            | "wav"
            | "jpg"
            | "png"
            | "gif"
            | "webp"
    ) {
        "media"
    } else if matches!(
        extension.as_str(),
        "exe" | "msi" | "msix" | "appx" | "bat" | "cmd"
    ) {
        "program"
    } else if matches!(
        extension.as_str(),
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "iso"
    ) {
        "archive"
    } else {
        "other"
    }
}

fn file_extension(filename: &str) -> String {
    filename
        .split(['?', '#'])
        .next()
        .unwrap_or(filename)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .filter(|extension| !extension.is_empty() && extension.len() <= 12)
        .unwrap_or_else(|| "未知后缀".into())
}

fn resource_kind_label(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Hls => "HLS",
        ResourceKind::Dash => "DASH",
        ResourceKind::Live => "直播",
        ResourceKind::Ftp => "FTP",
        ResourceKind::Sftp => "SFTP",
        ResourceKind::Torrent => "BT",
        ResourceKind::File => "HTTP",
    }
}

fn format_resource_meta(offer: &ResourceOffer, filename: &str) -> String {
    let extension = file_extension(filename);
    let extension = if extension == "未知后缀" {
        extension
    } else {
        format!(".{extension}")
    };
    let size = if offer.size > 0 {
        format_bytes(offer.size)
    } else {
        "大小未知".into()
    };
    let mime = offer.mime_type.trim();
    let mut parts = vec![
        resource_kind_label(offer.resource_kind).to_string(),
        extension,
    ];
    if !mime.is_empty() {
        parts.push(mime.chars().take(64).collect());
    }
    parts.push(size);
    parts.join(" · ")
}

fn format_request_details(offer: &ResourceOffer) -> String {
    let method = offer.request_method.trim().to_ascii_uppercase();
    format!(
        "{} · 支持 Referer / Origin / User-Agent / Cookie / Authorization",
        if method.is_empty() { "GET" } else { &method }
    )
}

fn canonical_url(value: &str) -> &str {
    value
        .split('#')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/')
}

fn task_status_label(status: &str) -> &str {
    match status {
        "downloading" => "下载中",
        "paused" => "已暂停",
        "completed" | "done" => "已完成",
        "failed" | "error" => "失败",
        "queued" => "排队中",
        _ => status,
    }
}

#[cfg(windows)]
fn pick_folder(initial: &str) -> Result<Option<String>, String> {
    let script = r#"[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); Add-Type -AssemblyName System.Windows.Forms; $dialog = New-Object System.Windows.Forms.FolderBrowserDialog; $dialog.Description = '选择下载保存位置'; if ($env:HLS_V7_INITIAL_DIR -and (Test-Path -LiteralPath $env:HLS_V7_INITIAL_DIR)) { $dialog.SelectedPath = $env:HLS_V7_INITIAL_DIR }; if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Out.Write($dialog.SelectedPath) }"#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-STA", "-Command", script])
        .env("HLS_V7_INITIAL_DIR", initial)
        .output()
        .map_err(|error| format!("无法打开文件夹选择器：{error}"))?;
    if !output.status.success() {
        return Err("文件夹选择器未能正常打开".into());
    }
    let path =
        String::from_utf8(output.stdout).map_err(|_| "文件夹选择器返回了无效路径".to_string())?;
    let path = path.trim().to_string();
    Ok((!path.is_empty()).then_some(path))
}

#[cfg(not(windows))]
fn pick_folder(_initial: &str) -> Result<Option<String>, String> {
    Err("当前系统不支持文件夹选择器".into())
}

fn task_progress(task: &TaskSnapshot) -> f32 {
    task.total_bytes
        .filter(|total| *total > 0)
        .map(|total| task.downloaded_bytes as f32 / total as f32)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0)
}

fn format_speed(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB/s", bytes as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.1} KB/s", bytes as f64 / 1024.0)
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn power_label(action: &str) -> &'static str {
    match action {
        "shutdown" => "关机",
        "sleep" => "睡眠",
        _ => "系统操作",
    }
}

#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    unsafe {
        let _ = AttachConsole(0xFFFF_FFFF);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

#[cfg(test)]
mod tests {
    use super::{
        download_category, file_extension, format_request_details, format_resource_meta,
        safe_display_url,
    };
    use hls_native_shell::{ResourceKind, ResourceOffer};

    #[test]
    fn displayed_handoff_location_hides_credentials_and_signed_query() {
        let shown = safe_display_url(
            "https://name:secret@cdn.example.test/media/1080/movie.mp4?token=private#track",
        );
        assert_eq!(shown, "cdn.example.test/1080/movie.mp4");
        assert!(!shown.contains("secret"));
        assert!(!shown.contains("token"));
    }

    #[test]
    fn browser_handoff_summary_uses_real_kind_suffix_and_category() {
        let offer = ResourceOffer {
            resource_kind: ResourceKind::Hls,
            size: 128 * 1024 * 1024,
            mime_type: "application/vnd.apple.mpegurl".into(),
            ..Default::default()
        };
        assert_eq!(
            download_category("movie.m3u8", offer.resource_kind),
            "media"
        );
        assert_eq!(file_extension("movie.M3U8?token=secret"), "m3u8");
        assert_eq!(
            format_resource_meta(&offer, "movie.m3u8"),
            "HLS · .m3u8 · application/vnd.apple.mpegurl · 128.0 MB"
        );
        assert!(format_request_details(&offer).contains("Referer / Origin / User-Agent"));
        assert_eq!(
            download_category("setup.exe", ResourceKind::File),
            "program"
        );
        assert_eq!(
            download_category("bundle.7z", ResourceKind::File),
            "archive"
        );
    }
}
