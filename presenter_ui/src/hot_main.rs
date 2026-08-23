#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

use hls_native_shell::{
    begin_caption_drag, center_window_by_title, claim_v7_presenter_instance, completion_sound,
    install_root, os_reduce_motion, spawn_core, CoreCommand, CoreEvent, CoreIpcClient,
    CorePipeResponse, EventEnvelope, ResourceOffer, TaskSnapshot,
};
use serde::Deserialize;
use slint::{ComponentHandle, RenderingState, Timer, TimerMode};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::env;
use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Deserialize)]
struct PersistedHandoff {
    #[serde(default)]
    status: String,
    offer: ResourceOffer,
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
    let (_, initial_sequence) = initial_client.snapshot_state()?;
    let initial_pending = load_pending_offers(&mut initial_client);
    let settings = initial_client.load_settings().ok();
    let show_progress = matches!(
        &settings,
        Some(CorePipeResponse::Settings {
            progress_window_enabled: true,
            ..
        })
    );
    let show_complete = matches!(
        &settings,
        Some(CorePipeResponse::Settings {
            complete_popup_enabled: true,
            ..
        })
    );
    let sound_enabled = matches!(
        &settings,
        Some(CorePipeResponse::Settings {
            completion_sound_enabled: true,
            ..
        })
    );
    let dark = matches!(
        &settings,
        Some(CorePipeResponse::Settings {
            dark_mode: true,
            ..
        })
    );
    let reduce_motion = matches!(
        &settings,
        Some(CorePipeResponse::Settings {
            reduce_motion: true,
            ..
        })
    ) || os_reduce_motion();

    let confirm = ConfirmWindow::new()?;
    let progress = ProgressWindow::new()?;
    let complete = CompleteWindow::new()?;
    confirm.hide()?;
    progress.hide()?;
    complete.hide()?;
    confirm.global::<Tokens>().set_dark(dark);
    confirm.global::<Tokens>().set_reduce_motion(reduce_motion);
    progress.global::<Tokens>().set_dark(dark);
    progress.global::<Tokens>().set_reduce_motion(reduce_motion);
    complete.global::<Tokens>().set_dark(dark);
    complete.global::<Tokens>().set_reduce_motion(reduce_motion);

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
    let active_task = Rc::new(RefCell::new(None::<String>));
    let completed_task = Rc::new(RefCell::new(None::<String>));
    let completed_notified = Rc::new(RefCell::new(HashSet::<String>::new()));
    let prewarm_finished = Rc::new(RefCell::new(false));

    progress.on_command({
        let client = Rc::clone(&client);
        let active_task = Rc::clone(&active_task);
        move |command| {
            let action = command.to_string();
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
                    let filename = window
                        .upgrade()
                        .map(|item| item.get_filename().to_string())
                        .unwrap_or_default();
                    match command_with_reconnect(
                        &client,
                        CoreCommand::AcceptHandoff {
                            handoff_id: offer.handoff_id.clone(),
                            filename,
                            download_dir: String::new(),
                        },
                    ) {
                        Ok(_) => {
                            pending.lock().ok().and_then(|mut items| items.pop_front());
                        }
                        Err(_) => return,
                    }
                }
                "reject" => {
                    if let Some(offer) = pending.lock().ok().and_then(|mut items| items.pop_front())
                    {
                        let _ = command_with_reconnect(
                            &client,
                            CoreCommand::RejectHandoff {
                                handoff_id: offer.handoff_id,
                            },
                        );
                    }
                }
                _ => return,
            }
            show_next_offer(&window, &pending);
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
        move || event_loop(tx, initial_sequence, confirm, pending)
    });
    let rx = Rc::new(RefCell::new(rx));
    let event_timer = Timer::default();
    event_timer.start(TimerMode::Repeated, Duration::from_millis(4), {
        let rx = Rc::clone(&rx);
        let pending = Arc::clone(&pending);
        let active_task = Rc::clone(&active_task);
        let completed_task = Rc::clone(&completed_task);
        let completed_notified = Rc::clone(&completed_notified);
        let confirm = confirm.as_weak();
        let progress = progress.as_weak();
        let complete = complete.as_weak();
        let prewarm_rendered = Arc::clone(&prewarm_rendered);
        let prewarm_finished = Rc::clone(&prewarm_finished);
        move || {
            if prewarm_rendered.load(Ordering::Acquire) && !*prewarm_finished.borrow() {
                let _ = center_window_by_title("确认下载");
                if let Some(item) = confirm.upgrade() {
                    if pending.lock().map(|items| items.is_empty()).unwrap_or(true) {
                        item.window()
                            .set_position(slint::PhysicalPosition::new(-32_000, -32_000));
                    } else {
                        item.set_intro(1.0);
                    }
                }
                *prewarm_finished.borrow_mut() = true;
                write_ready_marker();
            }
            while let Ok(events) = rx.borrow_mut().try_recv() {
                for envelope in events {
                    match envelope.event {
                        CoreEvent::HandoffOffered { .. } | CoreEvent::HandoffResolved { .. } => {}
                        CoreEvent::TaskCreated { snapshot }
                        | CoreEvent::TaskUpdated { snapshot }
                        | CoreEvent::TaskProgress { snapshot } => update_task_windows(
                            snapshot,
                            show_progress,
                            show_complete,
                            sound_enabled,
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
                                let _ = item.show();
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
            item.set_intro(0.0);
            item.window()
                .set_position(slint::PhysicalPosition::new(-32_000, -32_000));
            if item.show().is_ok() {
                // `show()` initializes the native window and renderer. Some
                // Windows/driver combinations deliberately skip rendering
                // notifications for a fully off-screen window, which used to
                // leave the presenter permanently "not ready" even though the
                // first real popup worked. The latency smoke still measures
                // the first real visible frame, so marking the completed
                // off-screen show here is both deterministic and honest.
                *prewarm_finished.borrow_mut() = true;
                write_ready_marker();
            }
        }
    });

    show_next_offer(&confirm.as_weak(), &pending);
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
            window.set_url("media.example.test/library/example-video.mp4".into());
            window.set_size_text("128.0 MB · HTTP".into());
            window.set_remaining("后面还有 2 个".into());
            window.set_intro(1.0);
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
) {
    loop {
        let Ok(mut client) = CoreIpcClient::connect() else {
            thread::sleep(Duration::from_millis(200));
            continue;
        };
        let Ok((_, latest_sequence)) = client.snapshot_state() else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        if latest_sequence < after {
            trace(&format!(
                "core sequence reset from {after} to {latest_sequence}; resynchronizing pending handoffs"
            ));
            after = latest_sequence;
            let restored = load_pending_offers(&mut client);
            let pending = Arc::clone(&pending);
            if confirm
                .upgrade_in_event_loop(move |item| {
                    if let Ok(mut items) = pending.lock() {
                        *items = restored;
                    }
                    show_next_offer(&item.as_weak(), &pending);
                })
                .is_err()
            {
                return;
            }
        }
        trace(&format!("event client connected after={after}"));
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
                                        show_next_offer(&item.as_weak(), &pending);
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            CoreEvent::HandoffResolved { handoff_id, .. } => {
                                let pending = Arc::clone(&pending);
                                if confirm
                                    .upgrade_in_event_loop(move |item| {
                                        if let Ok(mut items) = pending.lock() {
                                            items.retain(|entry| entry.handoff_id != handoff_id);
                                        }
                                        show_next_offer(&item.as_weak(), &pending);
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            event => batched.push(EventEnvelope {
                                sequence: envelope.sequence,
                                event,
                            }),
                        }
                    }
                    if !batched.is_empty() {
                        if tx.send(batched).is_err() {
                            return;
                        }
                        let _ = slint::invoke_from_event_loop(|| {});
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
        .filter(|handoff| handoff.status == "pending")
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
) {
    let Some(item) = window.upgrade() else {
        return;
    };
    let Some(offer) = pending.lock().ok().and_then(|items| items.front().cloned()) else {
        item.window()
            .set_position(slint::PhysicalPosition::new(-32_000, -32_000));
        return;
    };
    trace(&format!("showing handoff {}", offer.handoff_id));
    let filename = if offer.filename.trim().is_empty() {
        filename_from_url(&offer.url)
    } else {
        offer.filename.clone()
    };
    item.set_filename(filename.into());
    item.set_url(safe_display_url(&offer.url).into());
    item.set_size_text(if offer.size > 0 {
        format_bytes(offer.size).into()
    } else {
        "浏览器接管".into()
    });
    let remaining = pending
        .lock()
        .map(|items| items.len().saturating_sub(1))
        .unwrap_or_default();
    item.set_remaining(if remaining > 0 {
        format!("后面还有 {remaining} 个").into()
    } else {
        "".into()
    });
    item.set_intro(0.0);
    let shown = item.show().is_ok();
    if shown {
        let _ = center_window_by_title("确认下载");
        item.set_intro(1.0);
    }
    if let Ok(mut client) = CoreIpcClient::connect() {
        let _ = client.command(CoreCommand::PresentHandoff {
            handoff_id: offer.handoff_id,
            ok: shown,
        });
    }
}

fn trace(message: &str) {
    if env::var_os("HLS_V7_PRESENTER_TRACE").is_some() {
        eprintln!("[presenter] {message}");
    }
}

#[allow(clippy::too_many_arguments)]
fn update_task_windows(
    snapshot: TaskSnapshot,
    show_progress: bool,
    show_complete: bool,
    sound_enabled: bool,
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
        if show_progress {
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
        if sound_enabled {
            completion_sound();
        }
        if show_complete {
            if let Some(item) = complete.upgrade() {
                item.set_filename(snapshot.filename.into());
                item.set_power_hint("".into());
                let _ = item.show();
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
    use super::safe_display_url;

    #[test]
    fn displayed_handoff_location_hides_credentials_and_signed_query() {
        let shown = safe_display_url(
            "https://name:secret@cdn.example.test/media/1080/movie.mp4?token=private#track",
        );
        assert_eq!(shown, "cdn.example.test/1080/movie.mp4");
        assert!(!shown.contains("secret"));
        assert!(!shown.contains("token"));
    }
}
