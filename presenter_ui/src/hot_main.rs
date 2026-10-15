#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

use hls_native_shell::{
    activate_window_by_title, center_window_by_title,
    claim_v7_presenter_instance, completion_sound, hide_window_from_taskbar_by_title, install_root,
    is_already_running_error, os_reduce_motion, spawn_core, spawn_desktop_ui,
    CoreCommand, CoreEvent, CoreIpcClient, CorePipeResponse,
    EventEnvelope, ResourceKind, ResourceOffer, TaskSnapshot,
};
use serde::Deserialize;
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::env;
use std::io::Write;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

// Win32 辅助函数按窗口标题查找窗口；所有顶层窗口由这些常量设置实际标题。
mod window_titles {
    pub const CONFIRM: &str = "确认下载";
    pub const COMPLETE: &str = "下载完成";
    pub const PROGRESS_HEADLINE: &str = "下载进度";
    pub const LOCAL_PROCESSING: &str = "本地处理中";
}

use window_titles::{COMPLETE, CONFIRM, LOCAL_PROCESSING, PROGRESS_HEADLINE};

// release 构建使用 panic=abort：默认 hook 的输出会随进程消失，先落盘崩溃现场。
fn install_panic_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0);
        let text = format!("hls-v7-presenter panic (unix {}):\n{info}\n", seconds);
        let path = env::temp_dir().join("hls-downloader-presenter-crash.log");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(text.as_bytes());
        }
        previous(info);
    }));
}

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
    install_panic_log();
    // 确认弹窗必须在沙盒、远程桌面和无 OpenGL 驱动的电脑上正常显示。
    #[cfg(windows)]
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
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
    confirm.on_move_window({ let weak = confirm.as_weak(); move |dx, dy| { if let Some(item) = weak.upgrade() { move_window(item.window(), dx, dy); } } });
        confirm.set_window_title(CONFIRM.into());
        confirm.hide()?;
        attach_parent_console();
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "hls-v7-presenter/1 ok");
        let _ = stdout.flush();
        return Ok(());
    }
    let lock_test = args.iter().any(|arg| arg == "--lock-test");
    if let Err(error) = claim_v7_presenter_instance() {
        if is_already_running_error(&error) && !lock_test {
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
    confirm.on_move_window({ let weak = confirm.as_weak(); move |dx, dy| { if let Some(item) = weak.upgrade() { move_window(item.window(), dx, dy); } } });
    let progress = ProgressWindow::new()?;
    progress.on_move_window({ let weak = progress.as_weak(); move |dx, dy| { if let Some(item) = weak.upgrade() { move_window(item.window(), dx, dy); } } });
    let complete = CompleteWindow::new()?;
    complete.on_move_window({ let weak = complete.as_weak(); move |dx, dy| { if let Some(item) = weak.upgrade() { move_window(item.window(), dx, dy); } } });
    correct_native_click_position(confirm.window());
    correct_native_click_position(progress.window());
    correct_native_click_position(complete.window());
    confirm.set_window_title(CONFIRM.into());
    progress.set_headline(PROGRESS_HEADLINE.into());
    complete.set_window_title(COMPLETE.into());
    confirm.hide()?;
    progress.hide()?;
    complete.hide()?;
    apply_runtime_settings(&confirm, &progress, &complete, initial_runtime);

    let client = Rc::new(RefCell::new(initial_client));
    let pending = Arc::new(Mutex::new(initial_pending));
    let active_handoff = Arc::new(Mutex::new(None::<String>));
    let active_hud = Rc::new(RefCell::new(ActiveTaskHud::default()));
    let rendered_hud_task = Arc::new(Mutex::new(None::<String>));
    let completed_task = Rc::new(RefCell::new(None::<String>));
    let completed_notified = Rc::new(RefCell::new(HashSet::<String>::new()));

    progress.on_command({
        let active_hud = Rc::clone(&active_hud);
        let rendered_hud_task = Arc::clone(&rendered_hud_task);
        let window = progress.as_weak();
        move |command| {
            let action = command.to_string();
            trace(&format!("progress command: {action}"));
            if action == "hide_progress" {
                if let Some(item) = window.upgrade() {
                    item.set_hidden_by_user(true);
                    let _ = item.hide();
                }
                return;
            }
            if matches!(action.as_str(), "pause" | "cancel") {
                if let Some(task_id) = active_hud.borrow().primary_task_id() {
                    if let Some(item) = window.upgrade() {
                        item.set_action_busy(true);
                        item.set_action_error(false);
                        item.set_action_text(
                            if action == "pause" {
                                "正在暂停…"
                            } else {
                                "正在取消…"
                            }
                            .into(),
                        );
                    }
                    let result_window = window.clone();
                    let result_task_id = task_id.clone();
                    let result_hud_task = Arc::clone(&rendered_hud_task);
                    thread::spawn(move || {
                        let result = CoreIpcClient::connect().and_then(|mut client| {
                            client
                                .command(CoreCommand::TaskAction { task_id, action })
                                .map(|_| ())
                        });
                        let _ = result_window.upgrade_in_event_loop(move |item| {
                            if result_hud_task.lock().ok().and_then(|task| task.clone())
                                != Some(result_task_id)
                            {
                                return;
                            }
                            item.set_action_busy(false);
                            item.set_action_error(result.is_err());
                            item.set_action_text(match result {
                                Ok(()) => "指令已发送".into(),
                                Err(error) => format!("操作失败：{error}").into(),
                            });
                        });
                    });
                } else if let Some(item) = window.upgrade() {
                    item.set_action_error(true);
                    item.set_action_text("当前没有可操作的下载任务".into());
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
            trace(&format!("confirm command: {command}"));
            match command.as_str() {
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
                                remember_category_directory(
                                    &client,
                                    &presenter_settings,
                                    &category,
                                    download_dir.clone(),
                                );
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
                    // 先释放队列锁；成功后还需要再次获取它移除请求。
                    let offer = pending.lock().ok().and_then(|items| items.front().cloned());
                    if let Some(offer) = offer {
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
                    if let Some(item) = window.upgrade() {
                        item.set_busy(true);
                        item.set_error_text("正在打开主窗口…".into());
                    }
                    let result_window = window.clone();
                    thread::spawn(move || {
                        let started = install_root()
                            .map(|root| spawn_desktop_ui(&root))
                            .unwrap_or(false);
                        let signalled = CoreIpcClient::connect()
                            .and_then(|mut client| client.command(CoreCommand::OpenMain))
                            .is_ok();
                        let _ = result_window.upgrade_in_event_loop(move |item| {
                            item.set_busy(false);
                            item.set_error_text(match (started, signalled) {
                                (true, _) => "主窗口启动请求已发送；当前请求仍可在此确认".into(),
                                (false, true) => {
                                    "主窗口已收到显示请求；当前请求仍可在此确认".into()
                                }
                                (false, false) => "无法打开主窗口，请稍后重试".into(),
                            });
                        });
                    });
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
        let completed_task = Rc::clone(&completed_task);
        let window = complete.as_weak();
        move |command| {
            let Some(item) = window.upgrade() else { return };
            if command == "hide_complete" {
                let _ = item.hide();
                return;
            }
            if item.get_action_busy() { return; }
            let cancel_power = command == "cancel_power";
            let task_id = completed_task.borrow().clone();
            let request = if cancel_power {
                CoreCommand::CancelPowerAction
            } else if command == "open_file" || command == "open_folder" {
                let Some(task_id) = task_id.clone() else {
                    item.set_action_text("无法找到已完成任务，请在主窗口打开任务详情".into());
                    return;
                };
                CoreCommand::OpenCompleted { task_id, folder: command == "open_folder" }
            } else {
                return;
            };
            item.set_action_busy(true);
            item.set_action_text("".into());
            let result_window = window.clone();
            thread::spawn(move || {
                let result = CoreIpcClient::connect().and_then(|mut client| client.command(request));
                let _ = result_window.upgrade_in_event_loop(move |item| {
                    // 下一项下载完成后，旧窗口的操作结果不能隐藏新通知。
                    if item.get_task_id().as_str() != task_id.as_deref().unwrap_or("") { return; }
                    item.set_action_busy(false);
                    match result {
                        Ok(_) if cancel_power => item.set_power_hint("".into()),
                        Ok(_) => { let _ = item.hide(); }
                        Err(error) => item.set_action_text(format!("操作失败：{error}").into()),
                    }
                });
            });
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
        let active_hud = Rc::clone(&active_hud);
        let rendered_hud_task = Arc::clone(&rendered_hud_task);
        let completed_task = Rc::clone(&completed_task);
        let completed_notified = Rc::clone(&completed_notified);
        let client = Rc::clone(&client);
        let runtime_settings = Rc::clone(&runtime_settings);
        let confirm = confirm.as_weak();
        let progress = progress.as_weak();
        let complete = complete.as_weak();
        move || {
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
                            &active_hud,
                            &rendered_hud_task,
                            &completed_task,
                            &completed_notified,
                        ),
                        CoreEvent::TaskDeleted { task_id } => {
                            completed_notified.borrow_mut().remove(&task_id);
                            if completed_task.borrow().as_deref() == Some(task_id.as_str()) {
                                *completed_task.borrow_mut() = None;
                                if let Some(item) = complete.upgrade() {
                                    let _ = item.hide();
                                }
                            }
                            let primary = active_hud.borrow_mut().remove(&task_id);
                            match primary {
                                Some(primary) => {
                                    reset_progress_action_for_new_task(
                                        &progress,
                                        &rendered_hud_task,
                                        &primary.task_id,
                                    );
                                    render_progress_hud(
                                        &progress,
                                        *runtime_settings.borrow(),
                                        &primary,
                                    )
                                }
                                None => {
                                    if let Ok(mut rendered) = rendered_hud_task.lock() {
                                        *rendered = None;
                                    }
                                    if let Some(item) = progress.upgrade() {
                                        item.set_action_busy(false);
                                        item.set_action_error(false);
                                        item.set_action_text("".into());
                                        let _ = item.hide();
                                    }
                                }
                            }
                        }
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
                                    let _ = hide_window_from_taskbar_by_title(COMPLETE);
                                    let _ = center_window_by_title(COMPLETE);
                                    let _ = activate_window_by_title(COMPLETE);
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
                let _ = hide_window_from_taskbar_by_title(CONFIRM);
                // `show()` initializes the native window and renderer. Some
                // Windows/driver combinations deliberately skip rendering
                // notifications for a fully off-screen window, which used to
                // leave the presenter permanently "not ready" even though the
                // first real popup worked. The latency smoke still measures
                // the first real visible frame, so marking the completed
                // off-screen show here is both deterministic and honest.
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
    let fixture_seconds = env::var("HLS_V7_PRESENTER_FIXTURE_SECONDS").ok()
        .and_then(|value| value.parse::<u64>().ok()).unwrap_or(20).clamp(1, 600);
    quit_timer.start(TimerMode::SingleShot, Duration::from_secs(fixture_seconds), || {
        let _ = slint::quit_event_loop();
    });

    match kind {
        "confirm" | "confirm-error" => {
            let window = ConfirmWindow::new()?;
    window.on_move_window({ let weak = window.as_weak(); move |dx, dy| { if let Some(item) = weak.upgrade() { move_window(item.window(), dx, dy); } } });
            window.set_window_title(CONFIRM.into());
            window.global::<Tokens>().set_dark(dark);
            window.global::<Tokens>().set_reduce_motion(true);
            window.set_filename("示例视频 · 1080p.mp4".into());
            window.set_download_dir(r"D:\下载\媒体".into());
            window.set_url("media.example.test/library/example-video.mp4".into());
            window.set_source_url("video.example.test/watch/42".into());
            window.set_source_host("video.example.test".into());
            window.set_download_host("media.example.test".into());
            window.set_resource_meta("HTTP · .mp4 · 128.0 MB".into());
            window.set_request_context("已安全继承网页凭据".into());
            window.set_request_details(
                "请求方式：GET · 内容类型：video/mp4".into(),
            );
            window.set_category("media".into());
            window.set_duplicate_text("已有同一地址的任务：示例视频.mp4（已暂停）".into());
            window.set_remaining("  ·  后面还有 2 个".into());
            if kind == "confirm-error" {
                window.set_error_text(
                    "保存位置不可用，请选择其他目录后重试。\n当前请求仍可保留浏览器下载。".into(),
                );
            }
            window.on_command(|command| {
                if command != "drag" {
                    let _ = slint::quit_event_loop();
                }
            });
            correct_native_click_position(window.window());
            window.show()?;
            round_native_window(window.window());
            fit_confirm_window(&window);
            let _ = activate_window_by_title(CONFIRM);
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "progress" => {
            let window = ProgressWindow::new()?;
    window.on_move_window({ let weak = window.as_weak(); move |dx, dy| { if let Some(item) = weak.upgrade() { move_window(item.window(), dx, dy); } } });
            window.global::<Tokens>().set_dark(dark);
            window.global::<Tokens>().set_reduce_motion(true);
            window.set_headline(PROGRESS_HEADLINE.into());
            window.set_filename("示例视频 · 1080p.mp4".into());
            window.set_speed("18.6 MB/s".into());
            window.set_progress(0.64);
            window.set_progress_known(true);
            window.set_transferred("81.9 MB / 128.0 MB · 分片 64 / 100".into());
            window.set_stage_label("正在下载".into());
            window.set_remaining("剩余约 3 秒".into());
            correct_native_click_position(window.window());
            window.on_command(|_| { let _ = slint::quit_event_loop(); });
            window.show()?;
            round_native_window(window.window());
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "complete" => {
            let window = CompleteWindow::new()?;
    window.on_move_window({ let weak = window.as_weak(); move |dx, dy| { if let Some(item) = weak.upgrade() { move_window(item.window(), dx, dy); } } });
            window.set_window_title(COMPLETE.into());
            window.global::<Tokens>().set_dark(dark);
            window.global::<Tokens>().set_reduce_motion(true);
            window.set_filename("示例视频 · 1080p.mp4".into());
            window.set_details("128.0 MB · 已保存到本地".into());
            window.set_power_hint("将在 5 分钟后关机".into());
            correct_native_click_position(window.window());
            window.on_command(|_| { let _ = slint::quit_event_loop(); });
            window.show()?;
            round_native_window(window.window());
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
    // A cold start may spend seconds opening the database, migrating legacy
    // data or being scanned by antivirus software, so wait up to 10 s before
    // failing the presenter outright.
    for _ in 0..100 {
        if let Ok(client) = CoreIpcClient::connect() {
            return Ok(client);
        }
        thread::sleep(Duration::from_millis(100));
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

/// Persist a remembered category directory without clobbering concurrent
/// workbench edits: fetch fresh settings from the Core, merge only the single
/// category being remembered into the fresh map, then write the whole setting
/// back. The presenter-local cache always records the user's choice so the
/// rest of this session offers the same directory.
fn remember_category_directory(
    client: &Rc<RefCell<CoreIpcClient>>,
    presenter_settings: &Arc<Mutex<PresenterSettings>>,
    category: &str,
    directory: String,
) {
    if let Ok(mut settings) = presenter_settings.lock() {
        settings.remember(category, directory.clone());
    }
    let Ok(response) = client.borrow_mut().load_settings() else {
        return;
    };
    // Never write back a derived default: a non-Settings response must not
    // blank the other three categories in the stored map.
    let CorePipeResponse::Settings { .. } = &response else {
        return;
    };
    let mut fresh = PresenterSettings::from_response(Some(&response));
    fresh.remember(category, directory);
    let _ = client.borrow_mut().command(CoreCommand::SetSetting {
        key: "browser_category_dirs".into(),
        value: serde_json::Value::String(fresh.category_dirs_json()),
    });
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
            *known_tasks = tasks.clone();
        }
        // 重连后用快照恢复正在传输的任务，不能只等下一次增量事件。
        let active: Vec<_> = tasks
            .into_iter()
            .filter(|task| task_is_active(&task.status))
            .map(|snapshot| EventEnvelope {
                sequence: latest_sequence,
                event: CoreEvent::TaskUpdated { snapshot },
            })
            .collect();
        if !active.is_empty() && tx.send(active).is_err() {
            return;
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
    let encoded = match client.load_handoffs() {
        Ok(encoded) => encoded,
        Err(error) => {
            trace(&format!("恢复浏览器接管请求失败: {error}"));
            return VecDeque::new();
        }
    };
    encoded
        .into_iter()
        .filter_map(
            |encoded| match serde_json::from_str::<PersistedHandoff>(&encoded) {
                Ok(handoff) => Some(handoff),
                Err(error) => {
                    trace(&format!("忽略损坏的持久化接管请求: {error}"));
                    None
                }
            },
        )
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

// 输入为 rcWork 的物理尺寸；紧凑窗口不能超出可用工作区。
fn confirm_window_logical_size(work_area: (u32, u32), scale_factor: f32) -> Option<(f32, f32)> {
    if work_area.0 == 0 || work_area.1 == 0 || !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    Some((
        (work_area.0 as f32 / scale_factor).min(520.0),
        (work_area.1 as f32 / scale_factor).min(400.0),
    ))
}

// 仅在真实确认或 fixture 的 show() 成功后调用；离屏预热不参与尺寸适配。
fn fit_confirm_window(confirm: &ConfirmWindow) {
    #[cfg(windows)] {
        let weak = confirm.as_weak();
        // show() 和首个计时器都可能早于 HWND 创建，等待 Winit 的实际创建通知。
        let _ = slint::spawn_local(async move {
            let Some(confirm) = weak.upgrade() else { return; };
            use slint::winit_030::{winit::{dpi::{PhysicalPosition, PhysicalSize}, raw_window_handle::{HasWindowHandle, RawWindowHandle}}, WinitWindowAccessor};
            use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
            let native = match confirm.window().winit_window().await {
                Ok(native) => native,
                Err(error) => { trace(&format!("fit confirm: {error}")); return; }
            };
            let Ok(handle) = native.window_handle() else { return; };
            let RawWindowHandle::Win32(handle) = handle.as_raw() else { return; };
            let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            let monitor = unsafe { MonitorFromWindow(handle.hwnd.get() as _, MONITOR_DEFAULTTONEAREST) };
            if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 { trace(&format!("fit confirm: monitor query failed {}", std::io::Error::last_os_error())); return; }
            let work_width = (info.rcWork.right - info.rcWork.left) as u32;
            let work_height = (info.rcWork.bottom - info.rcWork.top) as u32;
            let scale = native.scale_factor() as f32;
            let Some((width, height)) = confirm_window_logical_size((work_width, work_height), scale) else { return; };
            let width = (width * scale).round() as u32;
            let height = (height * scale).round() as u32;
            trace(&format!("fit confirm work={work_width}x{work_height} scale={scale} size={width}x{height}"));
            let _ = native.request_inner_size(PhysicalSize::new(width, height));
            native.set_outer_position(PhysicalPosition::new(
                info.rcWork.left + ((work_width - width) / 2) as i32,
                info.rcWork.top + ((work_height - height) / 2) as i32,
            ));
            round_native_window(confirm.window());
        });
    }
    #[cfg(not(windows))] let _ = confirm;
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
    if active_handoff
        .lock()
        .map(|active| active.as_deref() == Some(offer.handoff_id.as_str()))
        .unwrap_or(false)
    {
        return;
    }
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
        } else if !offer.source_page_url.trim().is_empty() {
            "来自浏览器页面".into()
        } else {
            "直接下载链接".into()
        },
    );
    item.set_request_details(format_request_details(&offer).into());
    item.set_duplicate_text(duplicate_text.into());
    item.set_details_expanded(false);
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
        let _ = hide_window_from_taskbar_by_title(CONFIRM);
        fit_confirm_window(&item);
        let _ = activate_window_by_title(CONFIRM);
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
    if let Some(path) = env::var_os("HLS_V7_PRESENTER_TRACE_FILE") {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "[presenter {}] {message}", std::process::id());
        }
    } else if env::var_os("HLS_V7_PRESENTER_TRACE").is_some() {
        let _ = writeln!(std::io::stderr(), "[presenter] {message}");
    }
}

#[cfg(windows)]
unsafe extern "system" fn frameless_window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    subclass_id: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::UI::{Shell::{DefSubclassProc, RemoveWindowSubclass}, WindowsAndMessaging::{GWL_STYLE, STYLESTRUCT, WM_STYLECHANGING, WM_NCACTIVATE, WM_NCPAINT, WM_NCDESTROY, WS_CAPTION}};
    match message {
        // 保留 Winit 的焦点状态更新，但禁止 DefWindowProc 在失焦时绘制系统标题和白边。
        WM_NCACTIVATE => DefSubclassProc(hwnd, message, wparam, -1),
        WM_NCPAINT => 0,
        // Winit 在更新置顶/可见状态时会重新加上 WS_CAPTION；这些自绘浮窗不使用系统标题栏。
        WM_STYLECHANGING if wparam as i32 == GWL_STYLE => {
            (*(lparam as *mut STYLESTRUCT)).styleNew &= !WS_CAPTION;
            0
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(frameless_window_proc), subclass_id);
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

fn round_native_window(window: &slint::Window) {
    #[cfg(windows)] {
        use slint::winit_030::{winit::raw_window_handle::{HasWindowHandle, RawWindowHandle}, WinitWindowAccessor};
        use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn}, UI::WindowsAndMessaging::{GetWindowRect, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, WS_CAPTION, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_NOACTIVATE}};
        let _ = window.with_winit_window(|native| {
            let Ok(handle) = native.window_handle() else { return; };
            let RawWindowHandle::Win32(handle) = handle.as_raw() else { return; };
            unsafe {
                let hwnd = handle.hwnd.get() as _;
                windows_sys::Win32::UI::Shell::SetWindowSubclass(hwnd, Some(frameless_window_proc), 1, 0);
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                if style & WS_CAPTION as isize != 0 {
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_CAPTION as isize));
                    SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0,
                        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
                let mut bounds: RECT = std::mem::zeroed();
                if GetWindowRect(hwnd, &mut bounds) == 0 { return; }
                let diameter = (28.0 * window.scale_factor()).round() as i32;
                let region = CreateRoundRectRgn(0, 0, bounds.right - bounds.left + 1, bounds.bottom - bounds.top + 1, diameter, diameter);
                if !region.is_null() && SetWindowRgn(hwnd, region, 1) == 0 {
                    DeleteObject(region);
                }
            }
        });
    }
    #[cfg(not(windows))] let _ = window;
}

fn move_window(window: &slint::Window, dx: f32, dy: f32) {
    let position = window.position();
    let scale = window.scale_factor();
    trace(&format!("move window by {dx},{dy} from {},{}", position.x, position.y));
    window.set_position(slint::PhysicalPosition::new(
        position.x + (dx * scale).round() as i32,
        position.y + (dy * scale).round() as i32,
    ));
}

fn correct_native_click_position(window: &slint::Window) {
    #[cfg(windows)]
    {
        use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        // Winit 跨窗口复用并合并鼠标位置。每个窗口立即派发移动，避免按钮坐标串窗，
        // 也避免事件排队时 GetCursorPos 已变成拖动终点，导致按下位置落在标题栏外。
        let cursor_position = std::cell::Cell::new(None::<slint::LogicalPosition>);
        let left_pressed = std::cell::Cell::new(false);
        window.on_winit_window_event(move |window, event| {
            if matches!(event, winit::event::WindowEvent::Resized(_) | winit::event::WindowEvent::ScaleFactorChanged { .. }) {
                round_native_window(window);
            }
            if let winit::event::WindowEvent::CursorMoved { position, .. } = event {
                let position = slint::LogicalPosition::new(
                    position.x as f32 / window.scale_factor(),
                    position.y as f32 / window.scale_factor(),
                );
                cursor_position.set(Some(position));
                let _ = window.try_dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
                return EventResult::PreventDefault;
            }
            if matches!(event, winit::event::WindowEvent::CursorLeft { .. }) && left_pressed.get() {
                // 鼠标已捕获，离开窗口不代表取消拖动；Winit 的默认按下状态被拦截了。
                return EventResult::PreventDefault;
            }
            let winit::event::WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } = event
            else {
                return EventResult::Propagate;
            };
            let native_position = window
                .with_winit_window(|native| {
                    let RawWindowHandle::Win32(handle) = native.window_handle().ok()?.as_raw()
                    else {
                        return None;
                    };
                    let position = if let Some(position) = cursor_position.get() {
                        position
                    } else {
                        let mut point = POINT { x: 0, y: 0 };
                        unsafe {
                            if GetCursorPos(&mut point) == 0
                                || ScreenToClient(handle.hwnd.get() as _, &mut point) == 0
                            {
                                return None;
                            }
                        }
                        slint::LogicalPosition::new(
                            point.x as f32 / window.scale_factor(),
                            point.y as f32 / window.scale_factor(),
                        )
                    };
                    Some((handle.hwnd.get() as windows_sys::Win32::Foundation::HWND, position))
                })
                .flatten();
            let Some((hwnd, position)) = native_position else {
                return EventResult::Propagate;
            };
            if *state == winit::event::ElementState::Pressed {
                // 默认鼠标事件被下方拦截，需自行捕获才能在拖出窗口后继续收到移动和释放。
                unsafe { SetCapture(hwnd); }
                left_pressed.set(true);
                let _ = window.try_dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
            }
            let button = slint::platform::PointerEventButton::Left;
            let event = if *state == winit::event::ElementState::Pressed {
                slint::platform::WindowEvent::PointerPressed { position, button }
            } else {
                slint::platform::WindowEvent::PointerReleased { position, button }
            };
            trace(&format!(
                "native left button: {state:?} at {},{}",
                position.x, position.y
            ));
            let _ = window.try_dispatch_event(event);
            if *state == winit::event::ElementState::Released {
                unsafe { ReleaseCapture(); }
                left_pressed.set(false);
                cursor_position.set(None);
                let _ = window.take_snapshot();
                window.request_redraw();
            }
            EventResult::PreventDefault
        });
    }
    #[cfg(not(windows))]
    let _ = window;
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

fn task_is_active(status: &str) -> bool {
    matches!(status, "downloading" | "recording" | "merging" | "checking")
}

/// Ordered view of the tasks currently shown by the progress HUD. Entries are
/// kept sorted by (activation, task_id): the primary is the task activated
/// earliest, with simultaneous activations broken by lowest task_id. The
/// primary only changes when it actually stops, so concurrent downloads never
/// flip the HUD between filenames, and the HUD hides only once no task is
/// active anymore.
#[derive(Default)]
struct ActiveTaskHud {
    entries: Vec<ActiveTaskEntry>,
    next_activation: u64,
}

struct ActiveTaskEntry {
    task_id: String,
    activation: u64,
    snapshot: TaskSnapshot,
}

impl ActiveTaskHud {
    /// Fold a task snapshot into the view and return the snapshot the HUD
    /// should display afterwards (None when no task is active anymore).
    fn observe(&mut self, snapshot: TaskSnapshot) -> Option<TaskSnapshot> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.task_id == snapshot.task_id);
        if task_is_active(&snapshot.status) {
            match index {
                Some(index) => self.entries[index].snapshot = snapshot,
                None => {
                    let activation = self.next_activation;
                    self.next_activation += 1;
                    let entry = ActiveTaskEntry {
                        task_id: snapshot.task_id.clone(),
                        activation,
                        snapshot,
                    };
                    let insert_at = self.entries.partition_point(|existing| {
                        existing.activation < entry.activation
                            || (existing.activation == entry.activation
                                && existing.task_id < entry.task_id)
                    });
                    self.entries.insert(insert_at, entry);
                }
            }
        } else if let Some(index) = index {
            self.entries.remove(index);
        }
        self.entries.first().map(|entry| entry.snapshot.clone())
    }

    /// Drop a task deleted while active and return the snapshot the HUD should
    /// display afterwards.
    fn remove(&mut self, task_id: &str) -> Option<TaskSnapshot> {
        self.entries.retain(|entry| entry.task_id != task_id);
        self.entries.first().map(|entry| entry.snapshot.clone())
    }

    fn primary_task_id(&self) -> Option<String> {
        self.entries.first().map(|entry| entry.task_id.clone())
    }
}

#[allow(clippy::too_many_arguments)]
fn update_task_windows(
    snapshot: TaskSnapshot,
    settings: PresenterRuntimeSettings,
    progress: &slint::Weak<ProgressWindow>,
    complete: &slint::Weak<CompleteWindow>,
    active_hud: &RefCell<ActiveTaskHud>,
    rendered_hud_task: &Mutex<Option<String>>,
    completed_task: &RefCell<Option<String>>,
    completed_notified: &RefCell<HashSet<String>>,
) {
    let primary = active_hud.borrow_mut().observe(snapshot.clone());
    match primary {
        Some(primary) => {
            reset_progress_action_for_new_task(progress, rendered_hud_task, &primary.task_id);
            render_progress_hud(progress, settings, &primary)
        }
        None => {
            if let Ok(mut rendered) = rendered_hud_task.lock() {
                *rendered = None;
            }
            if let Some(item) = progress.upgrade() {
                item.set_action_busy(false);
                item.set_action_error(false);
                item.set_action_text("".into());
                let _ = item.hide();
            }
        }
    }
    if matches!(snapshot.status.as_str(), "completed" | "done")
        && completed_notified
            .borrow_mut()
            .insert(snapshot.task_id.clone())
    {
        *completed_task.borrow_mut() = Some(snapshot.task_id.clone());
        if let Some(item) = complete.upgrade() {
            item.set_task_id(snapshot.task_id.clone().into());
            item.set_action_busy(false);
            item.set_action_text("".into());
        }
        if settings.sound_enabled {
            completion_sound();
        }
        if settings.show_complete {
            if let Some(item) = complete.upgrade() {
                item.set_details(format!("{} · 已保存到本地", format_bytes(snapshot.downloaded_bytes)).into());
                item.set_filename(snapshot.filename.into());
                item.set_power_hint("".into());
                if item.show().is_ok() {
                    round_native_window(item.window());
                    let _ = hide_window_from_taskbar_by_title(COMPLETE);
                    let _ = center_window_by_title(COMPLETE);
                    let _ = activate_window_by_title(COMPLETE);
                }
            }
        }
    }
}

fn reset_progress_action_for_new_task(
    progress: &slint::Weak<ProgressWindow>,
    rendered_hud_task: &Mutex<Option<String>>,
    task_id: &str,
) {
    let Ok(mut rendered) = rendered_hud_task.lock() else {
        return;
    };
    if rendered.as_deref() == Some(task_id) {
        return;
    }
    *rendered = Some(task_id.to_string());
    if let Some(item) = progress.upgrade() {
        item.set_hidden_by_user(false);
        item.set_action_busy(false);
        item.set_action_error(false);
        item.set_action_text("".into());
    }
}

fn render_progress_hud(
    progress: &slint::Weak<ProgressWindow>,
    settings: PresenterRuntimeSettings,
    snapshot: &TaskSnapshot,
) {
    if !settings.show_progress {
        return;
    }
    let Some(item) = progress.upgrade() else {
        return;
    };
    if item.get_hidden_by_user() {
        return;
    }
    trace(&format!(
        "render progress: {} {} {}",
        snapshot.task_id, snapshot.status, snapshot.downloaded_bytes
    ));
    item.set_headline(
        if matches!(snapshot.status.as_str(), "merging" | "checking") {
            LOCAL_PROCESSING.into()
        } else {
            PROGRESS_HEADLINE.into()
        },
    );
    item.set_filename(snapshot.filename.clone().into());
    let processing = matches!(snapshot.status.as_str(), "merging" | "checking");
    item.set_processing(processing);
    item.set_speed(if processing { "本地处理中".into() } else { format_speed(snapshot.speed_bytes_per_sec).into() });
    item.set_remaining(if processing {
        "请稍候".into()
    } else if snapshot.speed_bytes_per_sec > 0 {
        snapshot.total_bytes.filter(|total| *total > snapshot.downloaded_bytes)
            .map(|total| {
                let seconds = (total - snapshot.downloaded_bytes).div_ceil(snapshot.speed_bytes_per_sec);
                if seconds >= 60 { format!("剩余约 {} 分 {} 秒", seconds / 60, seconds % 60) }
                else { format!("剩余约 {seconds} 秒") }
            }).unwrap_or_default().into()
    } else { "".into() });
    item.set_stage_label(match snapshot.status.as_str() {
        "merging" => "正在合并音视频",
        "checking" => "正在校验文件",
        "recording" => "正在录制",
        _ => "正在下载",
    }.into());
    item.set_progress(task_progress(snapshot));
    item.set_progress_known(snapshot.total_bytes.is_some_and(|total| total > 0)
        || (!snapshot.is_live && matches!(snapshot.resource_kind, ResourceKind::Hls | ResourceKind::Dash) && snapshot.total_ranges > 0));
    item.set_transferred(match snapshot.total_bytes.filter(|total| *total > 0) {
        Some(total) => format!(
            "已下载 {} / {}",
            format_bytes(snapshot.downloaded_bytes),
            format_bytes(total)
        )
        .into(),
        None if !snapshot.is_live && snapshot.total_ranges > 0 => format!("{} · 分片 {} / {}", format_bytes(snapshot.downloaded_bytes), snapshot.completed_ranges, snapshot.total_ranges).into(),
        None => format!("已下载 {}", format_bytes(snapshot.downloaded_bytes)).into(),
    });
    let first_show = !item.window().is_visible();
    if first_show && item.show().is_ok() {
        round_native_window(item.window());
        let _ = center_window_by_title(item.get_headline().as_str());
        let _ = hide_window_from_taskbar_by_title(item.get_headline().as_str());
    }
    // Win32 移动/隐藏后 softbuffer 的缓冲年龄可能仍为 1，局部绘制会丢掉标题和按钮。
    // 软件快照重置绘制缓存；小浮窗随后重画完整一帧，不能只刷新变化的速度区域。
    let _ = item.window().take_snapshot();
    item.window().request_redraw();
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
    let parts = [
        resource_kind_label(offer.resource_kind).to_string(),
        extension,
        size,
    ];
    parts.join(" · ")
}

fn format_request_details(offer: &ResourceOffer) -> String {
    let method = offer.request_method.trim().to_ascii_uppercase();
    let request = format!(
        "请求方式：{}",
        if method.is_empty() { "GET" } else { &method }
    );
    let mime = offer.mime_type.trim();
    if mime.is_empty() {
        request
    } else {
        format!("{request} · 内容类型：{mime}")
    }
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
    if matches!(task.status.as_str(), "completed" | "done") {
        return 1.0;
    }
    if !task.is_live && matches!(task.resource_kind, ResourceKind::Hls | ResourceKind::Dash) && task.total_ranges > 0 {
        return (task.completed_ranges as f32 / task.total_ranges as f32).clamp(0.0, 1.0);
    }
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
        "hibernate" => "休眠",
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
        confirm_window_logical_size, download_category, file_extension, format_request_details,
        format_resource_meta, safe_display_url, ActiveTaskHud,
    };
    use hls_native_shell::{ResourceKind, ResourceOffer, TaskSnapshot};

    #[test]
    fn confirm_window_size_matches_work_area_height_at_each_dpi() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            for (logical_height, expected_height) in [
                (240.0_f32, 240.0),
                (320.0, 320.0),
                (360.0, 360.0),
                (400.0, 400.0),
                (816.0, 400.0),
            ] {
                let physical = ((960.0 * scale) as u32, (logical_height * scale) as u32);
                assert_eq!(
                    confirm_window_logical_size(physical, scale),
                    Some((520.0, expected_height)),
                    "工作区={physical:?}，缩放={scale}"
                );
            }
        }
    }

    #[test]
    fn confirm_window_size_keeps_preferred_size_when_it_fits() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            for (width, height) in [(520.0, 400.0), (960.0, 816.0), (1920.0, 1080.0)] {
                assert_eq!(
                    confirm_window_logical_size(
                        ((width * scale) as u32, (height * scale) as u32),
                        scale,
                    ),
                    Some((520.0, 400.0))
                );
            }
        }
    }

    #[test]
    fn confirm_window_size_clamps_both_axes_without_raising_the_work_area() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            assert_eq!(
                confirm_window_logical_size(
                    ((480.0 * scale) as u32, (320.0 * scale) as u32),
                    scale
                ),
                Some((480.0, 320.0))
            );
        }
        // 物理像素不能整除缩放时，也不能向上取整越过工作区。
        let (width, height) = confirm_window_logical_size((751, 479), 1.5).unwrap();
        assert!((width - 751.0 / 1.5).abs() < 0.001);
        assert!((height - 479.0 / 1.5).abs() < 0.001);
        assert!(width * 1.5 <= 751.001 && height * 1.5 <= 479.001);
    }

    #[test]
    fn confirm_window_size_clamps_width_without_shrinking_fitting_height() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            assert_eq!(
                confirm_window_logical_size(
                    ((480.0 * scale) as u32, (816.0 * scale) as u32),
                    scale
                ),
                Some((480.0, 400.0)),
                "仅宽度受限，缩放={scale}"
            );
        }
    }

    #[test]
    fn confirm_window_size_converts_physical_work_area_before_clamping() {
        // 直接给出物理工作区，防止把物理像素当成逻辑尺寸或重复应用 DPI。
        for (physical, scale, expected) in [
            ((750, 480), 1.5_f32, (500.0, 320.0)),
            ((960, 640), 2.0, (480.0, 320.0)),
            ((1600, 852), 1.5, (520.0, 400.0)),
            ((650, 450), 1.25, (520.0, 360.0)),
            ((1920, 1020), 1.25, (520.0, 400.0)),
        ] {
            assert_eq!(
                confirm_window_logical_size(physical, scale),
                Some(expected),
                "物理工作区={physical:?}，缩放={scale}"
            );
        }
    }

    #[test]
    fn confirm_window_size_rejects_invalid_work_area_or_scale() {
        assert_eq!(confirm_window_logical_size((0, 584), 1.0), None);
        assert_eq!(confirm_window_logical_size((620, 0), 1.0), None);
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(confirm_window_logical_size((620, 584), scale), None);
        }
    }

    fn hud_snapshot(task_id: &str, status: &str) -> TaskSnapshot {
        TaskSnapshot {
            task_id: task_id.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    #[test]
    fn progress_hud_primary_is_earliest_activated_and_stable_until_it_stops() {
        let mut hud = ActiveTaskHud::default();
        hud.observe(hud_snapshot("b-2", "downloading"));
        hud.observe(hud_snapshot("a-1", "downloading"));
        assert_eq!(hud.primary_task_id().as_deref(), Some("b-2"));
        hud.observe(hud_snapshot("a-1", "completed"));
        assert_eq!(hud.primary_task_id().as_deref(), Some("b-2"));
        hud.remove("b-2");
        assert_eq!(hud.primary_task_id().as_deref(), None);
    }

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
            "HLS · .m3u8 · 128.0 MB"
        );
        assert_eq!(
            format_request_details(&offer),
            "请求方式：GET · 内容类型：application/vnd.apple.mpegurl"
        );
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
