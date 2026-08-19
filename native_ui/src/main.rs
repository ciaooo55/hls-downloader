#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ui_model;

slint::include_modules!();

use hls_native_shell::{
    claim_v6_instance, classify_url, clipboard_all_urls, clipboard_first_url, completion_sound,
    parse_curl_command, pick_export_path, pick_import_paths, read_clipboard, show_notification,
    spawn_tray, write_clipboard, attach_file_drop, sample_cells, CastDeviceInfo, CoreCommand, CoreEvent,
    CoreIpcClient, CorePipeResponse, CoreServer, HarvestCandidate, ResourceOffer, StreamVariant, TaskSnapshot,
    TaskSpec, TrayAction,
};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use std::{
    cell::RefCell, collections::{HashSet, VecDeque}, env, rc::Rc, sync::mpsc, thread,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--self-test") {
        let confirm = ConfirmWindow::new()?;
        confirm.hide()?;
        println!("hls-native-ui/1 ok");
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--native-host")
        || env::current_exe()
            .ok()
            .and_then(|path| path.file_name().map(|name| name.to_string_lossy().contains("NativeHost")))
            .unwrap_or(false)
    {
        std::process::exit(hls_native_shell::run_native_host());
    }
    if let Err(error) = claim_v6_instance() {
        if error.contains("already running") {
            if let Ok(mut client) = CoreIpcClient::connect() {
                let _ = client.command(CoreCommand::OpenMain);
            }
            return Ok(());
        }
        return Err(error.into());
    }

    let server = CoreServer::open_default()?;
    let (_addr, _worker) = server.bind_local()?;
    let mut client = CoreIpcClient::connect()?;

    let ui = MainWindow::new()?;
    let confirm = ConfirmWindow::new()?;
    let progress = ProgressWindow::new()?;
    let complete = CompleteWindow::new()?;
    let settings = SettingsWindow::new()?;
    let legal = LegalWindow::new()?;
    let new_task = NewTaskWindow::new()?;
    let player = PlayerWindow::new()?;
    let cast_hud = CastHudWindow::new()?;
    let log_window = LogWindow::new()?;
    let extension = ExtensionWindow::new()?;
    let harvest = HarvestWindow::new()?;
    confirm.hide()?;
    progress.hide()?;
    complete.hide()?;
    settings.hide()?;
    legal.hide()?;
    new_task.hide()?;
    player.hide()?;
    cast_hud.hide()?;
    log_window.hide()?;
    extension.hide()?;
    harvest.hide()?;

    new_task.set_variants(ModelRc::new(VecModel::from(vec!["自动最高画质".into()])));
    new_task.set_selected_variant("自动最高画质".into());
    new_task.set_audio_tracks(ModelRc::new(VecModel::from(vec!["默认音轨".into()])));
    new_task.set_selected_audio("默认音轨".into());
    player.set_devices(ModelRc::new(VecModel::from(vec!["局域网通知（未扫描）".into()])));
    player.set_selected_device("局域网通知（未扫描）".into());

    if std::env::var_os("HLS_V6_SKIP_LEGAL").is_none() {
        let accepted = matches!(
            client.load_settings(),
            Ok(CorePipeResponse::Settings {
                legal_accepted: true,
                ..
            })
        );
        if !accepted {
            legal.show()?;
        }
    }

    let bridge = Rc::new(RefCell::new(ui_model::UiBridge::default()));
    if let Ok(tasks) = client.snapshot() {
        bridge.borrow_mut().replace(tasks);
        refresh_tasks(&ui, &bridge.borrow(), "", "全部", None, &HashSet::new(), &mut vec![0.0; 48]);
    }

    let selected_task = Rc::new(RefCell::new(None::<String>));
    let pending_offer = Rc::new(RefCell::new(VecDeque::<ResourceOffer>::new()));
    let completed_beeped = Rc::new(RefCell::new(HashSet::<String>::new()));
    let last_completed = Rc::new(RefCell::new(None::<String>));
    let last_progress_task = Rc::new(RefCell::new(None::<String>));
    let probe_variants = Rc::new(RefCell::new(Vec::<StreamVariant>::new()));
    let harvest_links = Rc::new(RefCell::new(Vec::<HarvestCandidate>::new()));
    let harvest_picked = Rc::new(RefCell::new(HashSet::<String>::new()));
    let harvest_filter = Rc::new(RefCell::new("全部".to_string()));
    let harvest_page = Rc::new(RefCell::new(String::new()));
    let cast_devices = Rc::new(RefCell::new(Vec::<CastDeviceInfo>::new()));
    let query = Rc::new(RefCell::new(String::new()));
    let filter = Rc::new(RefCell::new("全部".to_string()));
    let picked = Rc::new(RefCell::new(HashSet::<String>::new()));
    let speed_samples = Rc::new(RefCell::new(vec![0.0f32; 48]));
    let clipboard_watch = Rc::new(RefCell::new(false));
    let last_clipboard = Rc::new(RefCell::new(String::new()));
    let sound_enabled = Rc::new(RefCell::new(false));
    let progress_window_enabled = Rc::new(RefCell::new(true));
    let complete_popup_enabled = Rc::new(RefCell::new(true));
    if let Ok(CorePipeResponse::Settings {
        clipboard_watch: watch,
        completion_sound_enabled,
        progress_window_enabled: show_progress,
        complete_popup_enabled: show_complete,
        dark_mode,
        ..
    }) = client.load_settings()
    {
        *clipboard_watch.borrow_mut() = watch;
        *sound_enabled.borrow_mut() = completion_sound_enabled;
        *progress_window_enabled.borrow_mut() = show_progress;
        *complete_popup_enabled.borrow_mut() = show_complete;
        ui.set_dark_mode(dark_mode);
        ui.global::<Tokens>().set_dark(dark_mode);
    }
    let client = Rc::new(RefCell::new(client));
    let (tx, rx) = mpsc::channel::<Vec<hls_native_shell::EventEnvelope>>();
    let rx = Rc::new(RefCell::new(rx));
    let (tray_tx, tray_rx) = mpsc::channel::<TrayAction>();
    spawn_tray(tray_tx);
    let (tray_ui_tx, tray_ui_rx) = mpsc::channel::<TrayAction>();
    let tray_ui_rx = Rc::new(RefCell::new(tray_ui_rx));
    let (drop_tx, drop_rx) = mpsc::channel::<Vec<String>>();
    let drop_rx = Rc::new(RefCell::new(drop_rx));

    let weak_ui = ui.as_weak();
    let weak_progress = progress.as_weak();
    let weak_complete = complete.as_weak();
    let weak_confirm = confirm.as_weak();
    let weak_settings = settings.as_weak();
    let weak_new_task = new_task.as_weak();
    let weak_player = player.as_weak();
    let weak_cast_hud = cast_hud.as_weak();
    let weak_log = log_window.as_weak();
    let weak_harvest = harvest.as_weak();
    let weak_legal = legal.as_weak();
    ui.window().on_close_requested(|| CloseRequestResponse::HideWindow);

    ui.on_wake({
        let bridge = Rc::clone(&bridge);
        let query = Rc::clone(&query);
        let filter = Rc::clone(&filter);
        let pending_offer = Rc::clone(&pending_offer);
        let selected_task = Rc::clone(&selected_task);
        let last_completed = Rc::clone(&last_completed);
        let last_progress_task = Rc::clone(&last_progress_task);
        let probe_variants = Rc::clone(&probe_variants);
        let harvest_links = Rc::clone(&harvest_links);
        let harvest_picked = Rc::clone(&harvest_picked);
        let harvest_filter = Rc::clone(&harvest_filter);
        let harvest_page = Rc::clone(&harvest_page);
        let cast_devices = Rc::clone(&cast_devices);
        let sound_enabled = Rc::clone(&sound_enabled);
        let progress_window_enabled = Rc::clone(&progress_window_enabled);
        let complete_popup_enabled = Rc::clone(&complete_popup_enabled);
        let picked = Rc::clone(&picked);
        let speed_samples = Rc::clone(&speed_samples);
        let client = Rc::clone(&client);
        let rx = Rc::clone(&rx);
        let tray_ui_rx = Rc::clone(&tray_ui_rx);
        let drop_rx = Rc::clone(&drop_rx);
        let weak_ui = weak_ui.clone();
        let weak_progress = weak_progress.clone();
        let weak_complete = weak_complete.clone();
        let weak_confirm = weak_confirm.clone();
        let weak_settings = weak_settings.clone();
        let weak_new_task = weak_new_task.clone();
        let weak_player = weak_player.clone();
        let weak_cast_hud = weak_cast_hud.clone();
        let weak_log = weak_log.clone();
        let weak_harvest = weak_harvest.clone();
        let weak_legal = weak_legal.clone();
        move || {
            while let Ok(action) = tray_ui_rx.borrow_mut().try_recv() {
                match action {
                    TrayAction::ShowMain => {
                        if let Some(window) = weak_ui.upgrade() {
                            let _ = window.show();
                        }
                    }
                    TrayAction::NewTask => {
                        if let Some(window) = weak_new_task.upgrade() {
                            let _ = window.show();
                        }
                    }
                    TrayAction::Settings => {
                        if let Some(window) = weak_settings.upgrade() {
                            fill_settings(&mut client.borrow_mut(), &window);
                            let _ = window.show();
                        }
                    }
                    TrayAction::Quit => {
                        slint::quit_event_loop().ok();
                    }
                }
            }
            while let Ok(paths) = drop_rx.borrow_mut().try_recv() {
                if paths.is_empty() {
                    continue;
                }
                if legal_blocked(&mut client.borrow_mut(), &weak_legal) {
                    continue;
                }
                let count = paths.len();
                match client.borrow_mut().command(CoreCommand::ImportPaths { paths }) {
                    Ok(events) => {
                        for envelope in events {
                            bridge.borrow_mut().apply(envelope.event);
                        }
                        if let Some(window) = weak_ui.upgrade() {
                            window.set_toast_text(format!("已导入拖入的 {} 个文件", count).into());
                        }
                    }
                    Err(error) => {
                        if let Some(window) = weak_ui.upgrade() {
                            window.set_toast_text(error.into());
                        }
                    }
                }
            }
            let mut dirty = false;
            while let Ok(events) = rx.borrow_mut().try_recv() {
                dirty = true;
                for envelope in events {
                    if let CoreEvent::UiShow { surface } = &envelope.event {
                        if let Some(window) = weak_ui.upgrade() {
                            if surface == "main" {
                                let _ = window.show();
                            } else if surface == "hide" {
                                let _ = window.hide();
                            }
                        }
                    }
                    if let CoreEvent::HandoffOffered { offer } = &envelope.event {
                        pending_offer.borrow_mut().push_back(offer.clone());
                        if pending_offer.borrow().len() == 1 {
                            present_queued_offer(
                                weak_confirm.clone(),
                                &pending_offer,
                                &mut client.borrow_mut(),
                            );
                        } else if let Some(window) = weak_ui.upgrade() {
                            window.set_toast_text(
                                format!("还有 {} 个接管请求排队", pending_offer.borrow().len() - 1)
                                    .into(),
                            );
                            if let Some(confirm) = weak_confirm.upgrade() {
                                confirm.set_remaining(
                                    format!("后面还有 {} 个", pending_offer.borrow().len() - 1)
                                        .into(),
                                );
                            }
                        }
                    }
                    if let CoreEvent::Toast { message, .. }
                    | CoreEvent::DuplicateOffered { message, .. } = &envelope.event
                    {
                        if let Some(window) = weak_ui.upgrade() {
                            window.set_toast_text(message.clone().into());
                            window.set_core_status(message.clone().into());
                        }
                    }
                    if let CoreEvent::DuplicateOffered {
                        task_id, action, ..
                    } = &envelope.event
                    {
                        *selected_task.borrow_mut() = Some(task_id.clone());
                        if action == "open" || action == "focus" {
                            if let Some(window) = weak_ui.upgrade() {
                                window.set_selected_filter("全部".into());
                            }
                            if action == "open" {
                                let _ = client.borrow_mut().command(CoreCommand::TaskAction {
                                    task_id: task_id.clone(),
                                    action: "open".into(),
                                });
                            }
                        } else if action == "retry" {
                            let _ = client.borrow_mut().command(CoreCommand::TaskAction {
                                task_id: task_id.clone(),
                                action: "retry".into(),
                            });
                        }
                    }
                    if let hls_native_shell::CoreEvent::TaskProgress { snapshot } = &envelope.event {
                        if *progress_window_enabled.borrow()
                            && (ui_model::is_active_transfer(&snapshot.status)
                                || ui_model::is_local_processing(&snapshot.status))
                        {
                            *last_progress_task.borrow_mut() = Some(snapshot.task_id.clone());
                            if let Some(window) = weak_progress.upgrade() {
                                window.set_filename(snapshot.filename.clone().into());
                                if ui_model::is_local_processing(&snapshot.status) {
                                    window.set_headline("本地处理中".into());
                                    window.set_speed("合并 / 校验".into());
                                } else {
                                    window.set_headline("下载进度".into());
                                    window.set_speed(format_speed(snapshot.speed_bytes_per_sec).into());
                                }
                                let progress = snapshot
                                    .total_bytes
                                    .filter(|total| *total > 0)
                                    .map(|total| snapshot.downloaded_bytes as f32 / total as f32)
                                    .unwrap_or(0.0);
                                window.set_progress(progress);
                                let _ = window.show();
                            }
                        }
                        if snapshot.status == "completed" {
                            if let Some(window) = weak_progress.upgrade() {
                                let _ = window.hide();
                            }
                            *last_completed.borrow_mut() = Some(snapshot.task_id.clone());
                            if *sound_enabled.borrow()
                                && completed_beeped.borrow_mut().insert(snapshot.task_id.clone())
                            {
                                completion_sound();
                                show_notification("下载完成", &snapshot.filename);
                            }
                            if *complete_popup_enabled.borrow() {
                                if let Some(window) = weak_complete.upgrade() {
                                    window.set_filename(snapshot.filename.clone().into());
                                    window.set_power_hint(String::new().into());
                                    let _ = window.show();
                                }
                            }
                        }
                    }
                    if let CoreEvent::ProbeResult {
                        label,
                        variants,
                        ..
                    } = &envelope.event
                    {
                        *probe_variants.borrow_mut() = variants.clone();
                        if let Some(window) = weak_new_task.upgrade() {
                            window.set_recognize_label(label.clone().into());
                            let video: Vec<slint::SharedString> = std::iter::once("自动最高画质".into())
                                .chain(
                                    variants
                                        .iter()
                                        .filter(|item| item.kind != "audio")
                                        .map(|item| item.label.clone().into()),
                                )
                                .collect();
                            window.set_variants(ModelRc::new(VecModel::from(video)));
                            window.set_selected_variant("自动最高画质".into());
                            let audio: Vec<slint::SharedString> = std::iter::once("默认音轨".into())
                                .chain(
                                    variants
                                        .iter()
                                        .filter(|item| item.kind == "audio")
                                        .map(|item| item.label.clone().into()),
                                )
                                .collect();
                            window.set_audio_tracks(ModelRc::new(VecModel::from(audio)));
                            window.set_selected_audio("默认音轨".into());
                        }
                    }
                    if let CoreEvent::CastDevices { devices } = &envelope.event {
                        *cast_devices.borrow_mut() = devices.clone();
                        if let Some(window) = weak_player.upgrade() {
                            let labels: Vec<slint::SharedString> = if devices.is_empty() {
                                vec!["局域网通知（未发现设备）".into()]
                            } else {
                                devices.iter().map(|item| item.label.clone().into()).collect()
                            };
                            window.set_devices(ModelRc::new(VecModel::from(labels)));
                            if let Some(first) = devices.first() {
                                window.set_selected_device(first.label.clone().into());
                            }
                            window.set_status(
                                if devices.is_empty() {
                                    "没有发现 DLNA 设备"
                                } else {
                                    "已扫描到投屏设备"
                                }
                                .into(),
                            );
                        }
                    }
                    if let CoreEvent::HarvestResult { links, url, .. } = &envelope.event {
                        *harvest_links.borrow_mut() = links.clone();
                        *harvest_picked.borrow_mut() =
                            links.iter().map(|link| link.url.clone()).collect();
                        *harvest_filter.borrow_mut() = "全部".into();
                        *harvest_page.borrow_mut() = url.clone();
                        if let Some(window) = weak_harvest.upgrade() {
                            fill_harvest_window(
                                &window,
                                links,
                                &harvest_picked.borrow(),
                                "全部",
                                url,
                            );
                            let _ = window.show();
                        }
                        if let Some(window) = weak_new_task.upgrade() {
                            window.set_recognize_label(
                                format!("页面抓取 {} · {} 条", url, links.len()).into(),
                            );
                        }
                        if let Some(ui) = weak_ui.upgrade() {
                            ui.set_toast_text(format!("抓到 {} 条下载链接", links.len()).into());
                        }
                    }
                    if let CoreEvent::BrowserStatus { message, .. } = &envelope.event {
                        if let Some(ui) = weak_ui.upgrade() {
                            ui.set_browser_status(message.clone().into());
                        }
                    }
                    if let CoreEvent::CastSession {
                        status, device, ..
                    } = &envelope.event
                    {
                        if let Some(window) = weak_cast_hud.upgrade() {
                            window.set_status(status.clone().into());
                            window.set_device(device.clone().into());
                            let _ = window.show();
                        }
                    }
                    if let CoreEvent::TaskLog { lines, .. } = &envelope.event {
                        if let Some(window) = weak_log.upgrade() {
                            window.set_text(lines.join("\n").into());
                            let _ = window.show();
                        }
                    }
                    if let CoreEvent::Error { code, message } = &envelope.event {
                        if let Some(window) = weak_ui.upgrade() {
                            if code.starts_with("update_")
                                || code.starts_with("probe_")
                                || code.starts_with("cast_")
                                || code.starts_with("power_")
                            {
                                window.set_core_status(message.lines().next().unwrap_or(message).into());
                            }
                        }
                        if code.starts_with("probe_") {
                            if let Some(window) = weak_new_task.upgrade() {
                                window.set_recognize_label(message.clone().into());
                            }
                        }
                        if code.starts_with("cast_") {
                            if let Some(window) = weak_player.upgrade() {
                                window.set_status(message.clone().into());
                            }
                        }
                        if code == "power_pending" {
                            if let Some(window) = weak_complete.upgrade() {
                                window.set_power_hint(message.clone().into());
                                let _ = window.show();
                            }
                        }
                        if code == "power_canceled" || code == "power_idle" {
                            if let Some(window) = weak_complete.upgrade() {
                                window.set_power_hint(String::new().into());
                            }
                        }
                    }
                    bridge.borrow_mut().apply(envelope.event);
                }
            }
            if dirty {
                if let Some(ui) = weak_ui.upgrade() {
                    refresh_tasks(
                        &ui,
                        &bridge.borrow(),
                        &query.borrow(),
                        &filter.borrow(),
                        selected_task.borrow().as_deref(),
                        &picked.borrow(),
                        &mut speed_samples.borrow_mut(),
                    );
                }
            }
        }
    });

    {
        let weak_ui = ui.as_weak();
        thread::spawn(move || {
            while let Ok(action) = tray_rx.recv() {
                if tray_ui_tx.send(action).is_err() {
                    break;
                }
                let weak_ui = weak_ui.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak_ui.upgrade() {
                        window.invoke_wake();
                    }
                });
            }
        });
    }
    {
        let weak_ui = ui.as_weak();
        thread::spawn(move || {
            let mut ipc = CoreIpcClient::connect().ok();
            let mut after = 0u64;
            while let Some(ref mut ipc) = ipc {
                match ipc.wait_events(after, 5_000) {
                    Ok(events) => {
                        if let Some(last) = events.last() {
                            after = last.sequence;
                        }
                        if events.is_empty() {
                            continue;
                        }
                        if tx.send(events).is_err() {
                            break;
                        }
                        let weak_ui = weak_ui.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(window) = weak_ui.upgrade() {
                                window.invoke_wake();
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        });
    }

    let clipboard_timer = Timer::default();
    {
        let clipboard_watch = Rc::clone(&clipboard_watch);
        let last_clipboard = Rc::clone(&last_clipboard);
        let client = Rc::clone(&client);
        let weak_ui = weak_ui.clone();
        let weak_new_task = weak_new_task.clone();
        clipboard_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(750),
            move || {
                if !*clipboard_watch.borrow() {
                    return;
                }
                let Some(text) = read_clipboard() else {
                    return;
                };
                if text == *last_clipboard.borrow() {
                    return;
                }
                *last_clipboard.borrow_mut() = text.clone();
                if let Ok(Some(curl)) = parse_curl_command(&text) {
                    if let Some(window) = weak_new_task.upgrade() {
                        apply_curl_to_window(&window, curl);
                        let _ = window.show();
                    }
                    if let Some(ui) = weak_ui.upgrade() {
                        ui.set_toast_text("已从剪贴板导入 cURL".into());
                    }
                    return;
                }
                let urls = clipboard_all_urls(&text);
                if urls.len() > 1 {
                    if let Some(window) = weak_new_task.upgrade() {
                        window.set_url(urls.join("\n").into());
                        let _ = window.show();
                    }
                    if let Some(ui) = weak_ui.upgrade() {
                        ui.set_toast_text(format!("剪贴板有 {} 条链接，已填入新建", urls.len()).into());
                    }
                    return;
                }
                let Some(url) = clipboard_first_url(&text) else {
                    return;
                };
                let _ = client.borrow_mut().command(CoreCommand::OfferResource {
                    offer: ResourceOffer {
                        url: url.clone(),
                        resource_kind: classify_url(&url),
                        owner: "clipboard".into(),
                        evidence: vec!["clipboard".into()],
                        confidence: 0.4,
                        filename: url
                            .split(['?', '#'])
                            .next()
                            .unwrap_or(&url)
                            .rsplit('/')
                            .find(|part| !part.is_empty())
                            .unwrap_or("download")
                            .to_string(),
                        ..Default::default()
                    },
                });
            },
        );
    }
    let ui_for_commands = ui.as_weak();
    ui.on_command({
        let client = Rc::clone(&client);
        let selected_task = Rc::clone(&selected_task);
        let query = Rc::clone(&query);
        let filter = Rc::clone(&filter);
        let picked = Rc::clone(&picked);
        let speed_samples = Rc::clone(&speed_samples);
        let bridge = Rc::clone(&bridge);
        let confirm = confirm.as_weak();
        let settings = settings.as_weak();
        let legal = legal.as_weak();
        let new_task = new_task.as_weak();
        let player = player.as_weak();
        let log_window = log_window.as_weak();
        let extension = extension.as_weak();
        move |command| {
            let command = command.to_string();
            if let Some(task_id) = command.strip_prefix("select:") {
                *selected_task.borrow_mut() = Some(task_id.to_string());
            } else if let Some(task_id) = command.strip_prefix("toggle:") {
                let mut picked = picked.borrow_mut();
                if !picked.remove(task_id) {
                    picked.insert(task_id.to_string());
                }
            } else if command == "toggle" {
                if let Some(task_id) = selected_task.borrow().clone() {
                    let mut picked = picked.borrow_mut();
                    if !picked.remove(&task_id) {
                        picked.insert(task_id);
                    }
                }
            } else if let Some(delta) = command.strip_prefix("reorder:") {
                if let Some(task_id) = selected_task.borrow().clone() {
                    if let Ok(delta) = delta.parse::<i32>() {
                        if let Ok(events) = client.borrow_mut().command(CoreCommand::ReorderQueue {
                            task_id,
                            delta,
                        }) {
                            for envelope in events {
                                bridge.borrow_mut().apply(envelope.event);
                            }
                        }
                    }
                }
            } else if let Some(data) = command.strip_prefix("drop:") {
                let paths = parse_drop_payload(data);
                if !paths.is_empty() {
                    if legal_blocked(&mut client.borrow_mut(), &legal) {
                        return;
                    }
                    match client.borrow_mut().command(CoreCommand::ImportPaths { paths }) {
                        Ok(events) => {
                            for envelope in events {
                                bridge.borrow_mut().apply(envelope.event);
                            }
                        }
                        Err(error) => {
                            if let Some(window) = ui_for_commands.upgrade() {
                                window.set_toast_text(error.into());
                            }
                        }
                    }
                }
            } else if let Some(value) = command.strip_prefix("search:") {
                *query.borrow_mut() = value.to_string();
            } else if let Some(value) = command.strip_prefix("filter:") {
                *filter.borrow_mut() = value.to_string();
            } else if command == "new_task" || command == "open_harvest" {
                let accepted = matches!(
                    client.borrow_mut().load_settings(),
                    Ok(CorePipeResponse::Settings {
                        legal_accepted: true,
                        ..
                    })
                );
                if !accepted && std::env::var_os("HLS_V6_SKIP_LEGAL").is_none() {
                    let _ = legal.upgrade().map(|window| window.show());
                } else if let Some(window) = new_task.upgrade() {
                    window.set_harvest(command == "open_harvest");
                    let _ = window.show();
                }
            } else if command == "paste_new" {
                let accepted = matches!(
                    client.borrow_mut().load_settings(),
                    Ok(CorePipeResponse::Settings {
                        legal_accepted: true,
                        ..
                    })
                );
                if !accepted && std::env::var_os("HLS_V6_SKIP_LEGAL").is_none() {
                    let _ = legal.upgrade().map(|window| window.show());
                } else if let Some(window) = new_task.upgrade() {
                    if let Some(text) = read_clipboard() {
                        if let Ok(Some(curl)) = parse_curl_command(&text) {
                            apply_curl_to_window(&window, curl);
                        } else {
                            window.set_url(text.into());
                        }
                    }
                    let _ = window.show();
                }
            } else if command == "refresh" {
                if let Ok(tasks) = client.borrow_mut().snapshot() {
                    bridge.borrow_mut().replace(tasks);
                    if let Some(ui) = ui_for_commands.upgrade() {
                        refresh_tasks(
                            &ui,
                            &bridge.borrow(),
                            &query.borrow(),
                            &filter.borrow(),
                            selected_task.borrow().as_deref(),
                            &picked.borrow(),
                            &mut speed_samples.borrow_mut(),
                        );
                        ui.set_toast_text("已刷新任务列表".into());
                    }
                }
            } else if command == "extension" {
                if let Some(window) = extension.upgrade() {
                    let status = ui_for_commands
                        .upgrade()
                        .map(|ui| ui.get_browser_status().to_string())
                        .unwrap_or_else(|| "插件未连接".into());
                    window.set_status(status.into());
                    let _ = window.show();
                }
            } else if command == "check_update" {
                let _ = client.borrow_mut().command(CoreCommand::CheckUpdate);
            } else if command == "settings" {
                if let Some(window) = settings.upgrade() {
                    fill_settings(&mut client.borrow_mut(), &window);
                    let _ = window.show();
                }
            } else if command == "play" || command == "cast" {
                let mut task_ids = target_task_ids(&selected_task.borrow(), &picked.borrow());
                if command == "cast" && task_ids.is_empty() {
                    task_ids = create_tasks_from_local_files(&mut client.borrow_mut(), &bridge);
                }
                if let Some(task_id) = task_ids.first().cloned() {
                    if command == "play" {
                        if let Some(window) = player.upgrade() {
                            let _ = window.show();
                            attach_player_embed(&mut client.borrow_mut(), &window);
                        }
                    }
                    let kind = if command == "play" {
                        CoreCommand::PlayTask { task_id }
                    } else {
                        CoreCommand::CastTask { task_id }
                    };
                    match client.borrow_mut().command(kind) {
                        Ok(_) => {
                            let _ = player.upgrade().map(|window| {
                                window.set_status(
                                    if command == "play" {
                                        "正在本地播放（libmpv 内嵌）"
                                    } else {
                                        "已发出投屏通知"
                                    }
                                    .into(),
                                );
                                window.show()
                            });
                        }
                        Err(error) => {
                            if let Some(window) = player.upgrade() {
                                window.set_status(error.clone().into());
                                let _ = window.show();
                            }
                            if let Some(ui) = ui_for_commands.upgrade() {
                                ui.set_core_status(error.into());
                            }
                        }
                    }
                }
            } else if command == "clear_completed" {
                if let Ok(events) = client.borrow_mut().command(CoreCommand::ClearCompleted) {
                    for envelope in events {
                        bridge.borrow_mut().apply(envelope.event);
                    }
                }
            } else if command == "export_urls" {
                let text = export_task_urls(&bridge.borrow().all());
                if text.is_empty() {
                    if let Some(ui) = ui_for_commands.upgrade() {
                        ui.set_toast_text("当前没有可导出的链接".into());
                    }
                } else if let Some(path) = pick_export_path() {
                    match std::fs::write(&path, text) {
                        Ok(()) => {
                            if let Some(ui) = ui_for_commands.upgrade() {
                                ui.set_toast_text(format!("已导出 {}", path.display()).into());
                            }
                        }
                        Err(error) => {
                            if let Some(ui) = ui_for_commands.upgrade() {
                                ui.set_core_status(error.to_string().into());
                            }
                        }
                    }
                } else if write_clipboard(&text).is_ok() {
                    if let Some(ui) = ui_for_commands.upgrade() {
                        ui.set_toast_text("已复制任务链接列表".into());
                    }
                }
            } else if command == "save_site_profile" {
                if let Some(task_id) = selected_task.borrow().clone() {
                    if let Ok(events) = client
                        .borrow_mut()
                        .command(CoreCommand::SaveSiteProfile { task_id })
                    {
                        for envelope in events {
                            if let CoreEvent::Toast { message, .. } = &envelope.event {
                                if let Some(ui) = ui_for_commands.upgrade() {
                                    ui.set_toast_text(message.clone().into());
                                }
                            }
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                }
            } else if command == "log" {
                if let Some(task_id) = selected_task.borrow().clone() {
                    if let Ok(events) = client.borrow_mut().command(CoreCommand::GetTaskLog { task_id }) {
                        for envelope in events {
                            if let CoreEvent::TaskLog { lines, .. } = &envelope.event {
                                if let Some(window) = log_window.upgrade() {
                                    window.set_text(lines.join("\n").into());
                                    let _ = window.show();
                                }
                            }
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                }
            } else if command == "push_tvbox" {
                let mut task_ids = target_task_ids(&selected_task.borrow(), &picked.borrow());
                if task_ids.is_empty() {
                    task_ids = create_tasks_from_local_files(&mut client.borrow_mut(), &bridge);
                }
                for task_id in task_ids {
                    if let Ok(events) = client.borrow_mut().command(CoreCommand::TaskAction {
                        task_id,
                        action: "push_tvbox".into(),
                    }) {
                        for envelope in events {
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                }
            } else if matches!(
                command.as_str(),
                "start" | "pause" | "delete" | "retry" | "open" | "delete_files" | "cancel"
                    | "launch" | "copy_file" | "queue_top" | "queue_bottom" | "resume"
            ) {
                if matches!(command.as_str(), "start" | "resume" | "retry")
                    && legal_blocked(&mut client.borrow_mut(), &legal)
                {
                    return;
                }
                for task_id in target_task_ids(&selected_task.borrow(), &picked.borrow()) {
                    if let Ok(events) = client.borrow_mut().command(CoreCommand::TaskAction {
                        task_id,
                        action: command.clone(),
                    }) {
                        for envelope in events {
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                }
            } else if command == "start_all" {
                if legal_blocked(&mut client.borrow_mut(), &legal) {
                    return;
                }
                for task in bridge.borrow().all() {
                    let action = match task.status.as_str() {
                        "queued" => "start",
                        "paused" | "failed" | "canceled" => {
                            if task.status == "paused" {
                                "resume"
                            } else {
                                "retry"
                            }
                        }
                        _ => continue,
                    };
                    if let Ok(events) = client.borrow_mut().command(CoreCommand::TaskAction {
                        task_id: task.task_id,
                        action: action.into(),
                    }) {
                        for envelope in events {
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                }
            } else if command == "pause_all" {
                for task in bridge.borrow().all() {
                    if matches!(
                        task.status.as_str(),
                        "downloading" | "recording" | "merging" | "checking"
                    ) {
                        if let Ok(events) = client.borrow_mut().command(CoreCommand::TaskAction {
                            task_id: task.task_id,
                            action: "pause".into(),
                        }) {
                            for envelope in events {
                                bridge.borrow_mut().apply(envelope.event);
                            }
                        }
                    }
                }
            } else if command == "dark" {
                if let Some(window) = ui_for_commands.upgrade() {
                    let _ = client.borrow_mut().store_setting(
                        "dark_mode",
                        serde_json::json!(window.get_dark_mode()),
                    );
                }
            } else if command == "queue_up" || command == "queue_down" {
                if let Some(task_id) = selected_task.borrow().clone() {
                    if let Ok(events) = client.borrow_mut().command(CoreCommand::ReorderQueue {
                        task_id,
                        delta: if command == "queue_up" { -1 } else { 1 },
                    }) {
                        for envelope in events {
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                }
            } else if let Some(before_id) = command.strip_prefix("place:") {
                if let Some(task_id) = selected_task.borrow().clone() {
                    if let Ok(events) = client.borrow_mut().command(CoreCommand::PlaceQueue {
                        task_id,
                        before_id: before_id.to_string(),
                    }) {
                        for envelope in events {
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                }
            } else if command == "copy_url" {
                if let Some(task_id) = selected_task.borrow().clone() {
                    if let Some(url) = bridge
                        .borrow()
                        .snapshot(&task_id)
                        .map(|snapshot| snapshot.url.clone())
                        .filter(|url| !url.is_empty())
                    {
                        match write_clipboard(&url) {
                            Ok(()) => {
                                if let Some(ui) = ui_for_commands.upgrade() {
                                    ui.set_core_status("已复制链接".into());
                                }
                            }
                            Err(error) => {
                                if let Some(ui) = ui_for_commands.upgrade() {
                                    ui.set_core_status(error.into());
                                }
                            }
                        }
                    }
                }
            }
            if let Some(ui) = ui_for_commands.upgrade() {
                refresh_tasks(
                    &ui,
                    &bridge.borrow(),
                    &query.borrow(),
                    &filter.borrow(),
                    selected_task.borrow().as_deref(),
                    &picked.borrow(),
                    &mut speed_samples.borrow_mut(),
                );
            }
            let _ = confirm;
        }
    });

    new_task.on_command({
        let client = Rc::clone(&client);
        let new_task = new_task.as_weak();
        let probe_variants = Rc::clone(&probe_variants);
        let legal = legal.as_weak();
        let ui_weak = ui.as_weak();
        move |command| {
            if command == "probe" || command == "harvest" {
                if let Some(window) = new_task.upgrade() {
                    let url = window
                        .get_url()
                        .to_string()
                        .lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                        .unwrap_or("")
                        .to_string();
                    window.set_recognize_label(
                        if command == "harvest" {
                            "正在抓取页面链接…"
                        } else {
                            "正在识别…"
                        }
                        .into(),
                    );
                    let result = if command == "harvest" || window.get_harvest() {
                        client.borrow_mut().command(CoreCommand::HarvestPage { url })
                    } else {
                        client.borrow_mut().command(CoreCommand::ProbeUrl { url })
                    };
                    if let Err(error) = result {
                        window.set_recognize_label(error.into());
                    }
                }
                return;
            }
            if command == "paste" {
                if let Some(window) = new_task.upgrade() {
                    if let Some(text) = read_clipboard() {
                        if let Ok(Some(curl)) = parse_curl_command(&text) {
                            apply_curl_to_window(&window, curl);
                            return;
                        }
                        let current = window.get_url().to_string();
                        let merged = if current.trim().is_empty() {
                            text
                        } else {
                            format!("{}\n{}", current.trim_end(), text.trim())
                        };
                        window.set_url(merged.into());
                    }
                }
                return;
            }
            if command == "import_files" {
                if legal_blocked(&mut client.borrow_mut(), &legal) {
                    return;
                }
                let paths = pick_import_paths();
                if paths.is_empty() {
                    return;
                }
                match client.borrow_mut().command(CoreCommand::ImportPaths {
                    paths: paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                }) {
                    Ok(_) => {
                        if let Some(window) = new_task.upgrade() {
                            let _ = window.hide();
                        }
                    }
                    Err(error) => {
                        if let Some(window) = new_task.upgrade() {
                            window.set_recognize_label(error.into());
                        }
                    }
                }
                return;
            }
            if command != "create_task" {
                return;
            }
            if legal_blocked(&mut client.borrow_mut(), &legal) {
                return;
            }
            if let Some(window) = new_task.upgrade() {
                let urls = window.get_url().to_string();
                let filename = window.get_filename().to_string();
                let selected = window.get_selected_variant().to_string();
                let selected_audio = window.get_selected_audio().to_string();
                let (preferred_bandwidth, preferred_height) = probe_variants
                    .borrow()
                    .iter()
                    .find(|item| item.label == selected && item.kind != "audio")
                    .map(|item| (item.bandwidth, item.height))
                    .unwrap_or((0, 0));
                let preferred_audio = probe_variants
                    .borrow()
                    .iter()
                    .find(|item| item.kind == "audio" && item.label == selected_audio)
                    .map(|item| {
                        if item.name.is_empty() {
                            item.label.clone()
                        } else {
                            item.name.clone()
                        }
                    })
                    .unwrap_or_default();
                for (index, url) in urls
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .enumerate()
                {
                    let mut headers = std::collections::BTreeMap::new();
                    let referer = window.get_referer().to_string();
                    let cookie = window.get_cookie().to_string();
                    if !referer.trim().is_empty() {
                        headers.insert("Referer".into(), referer.trim().to_string());
                    }
                    if index == 0 && !cookie.trim().is_empty() {
                        headers.insert("Cookie".into(), cookie.trim().to_string());
                    }
                    for line in window.get_extra_headers().to_string().lines() {
                        if let Some((name, value)) = line.split_once(':') {
                            if !name.trim().is_empty() {
                                headers.insert(name.trim().to_string(), value.trim().to_string());
                            }
                        }
                    }
                    match client.borrow_mut().command(CoreCommand::CreateTask {
                        spec: TaskSpec {
                            url: url.to_string(),
                            resource_kind: classify_url(url),
                            filename: if index == 0 {
                                filename.clone()
                            } else {
                                String::new()
                            },
                            harvest: window.get_harvest()
                                && index == 0
                                && urls.lines().filter(|line| !line.trim().is_empty()).count() == 1,
                            headers,
                            preferred_bandwidth,
                            preferred_height,
                            preferred_audio: preferred_audio.clone(),
                            checksum: {
                                let value = window.get_checksum().to_string();
                                (!value.trim().is_empty()).then_some(value)
                            },
                            allow_duplicate: window.get_allow_duplicate(),
                            mirrors: window
                                .get_mirrors()
                                .to_string()
                                .lines()
                                .map(str::trim)
                                .filter(|line| !line.is_empty())
                                .map(str::to_string)
                                .collect(),
                            concurrency: window
                                .get_concurrency()
                                .to_string()
                                .trim()
                                .parse()
                                .unwrap_or(0),
                            body_path: window.get_body_path().to_string(),
                            request_method: if window.get_body_path().to_string().trim().is_empty() {
                                "GET".into()
                            } else {
                                "POST".into()
                            },
                            ..Default::default()
                        },
                    }) {
                        Ok(_) => {}
                        Err(error) => {
                            window.set_recognize_label(error.clone().into());
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.set_toast_text(error.into());
                            }
                            return;
                        }
                    }
                }
                let _ = window.hide();
            }
        }
    });
    settings.on_command({
        let client = Rc::clone(&client);
        let settings = settings.as_weak();
        let clipboard_watch = Rc::clone(&clipboard_watch);
        let sound_enabled = Rc::clone(&sound_enabled);
        let progress_window_enabled = Rc::clone(&progress_window_enabled);
        let complete_popup_enabled = Rc::clone(&complete_popup_enabled);
        move |command| {
            if command == "check_update" {
                let _ = client.borrow_mut().command(CoreCommand::CheckUpdate);
                return;
            }
            if command == "download_update" {
                let _ = client.borrow_mut().command(CoreCommand::DownloadUpdate);
                return;
            }
            if command == "add_site_rule" {
                if let Some(window) = settings.upgrade() {
                    let host = window.get_site_host().to_string();
                    if host.trim().is_empty() {
                        return;
                    }
                    let mut rules = hls_native_shell::parse_site_rules(&window.get_site_rules().to_string());
                    hls_native_shell::upsert_site_rule(
                        &mut rules,
                        hls_native_shell::SiteRule {
                            host: host.trim().to_ascii_lowercase(),
                            speed_limit_kib: window.get_site_speed().parse().unwrap_or(0),
                            concurrency: window.get_site_conn().parse().unwrap_or(0),
                            proxy: window.get_site_proxy().to_string(),
                        },
                    );
                    let encoded = hls_native_shell::format_site_rules(&rules);
                    window.set_site_rules(encoded.clone().into());
                    let _ = client
                        .borrow_mut()
                        .store_setting("site_rules", serde_json::json!(encoded));
                }
                return;
            }
            if command != "save_settings" {
                return;
            }
            if let Some(window) = settings.upgrade() {
                let _ = client.borrow_mut().store_setting(
                    "legal_terms_accepted",
                    serde_json::json!(window.get_legal_accepted()),
                );
                let _ = client.borrow_mut().store_setting(
                    "browser_takeover_enabled",
                    serde_json::json!(window.get_takeover_enabled()),
                );
                if let Ok(limit) = window.get_speed_limit().parse::<u64>() {
                    let _ = client
                        .borrow_mut()
                        .store_setting("download_speed_limit_kib", serde_json::json!(limit));
                }
                let _ = client.borrow_mut().store_setting(
                    "download_speed_schedule_enabled",
                    serde_json::json!(window.get_schedule_enabled()),
                );
                let _ = client.borrow_mut().store_setting(
                    "download_speed_schedule_start",
                    serde_json::json!(window.get_schedule_start().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "download_speed_schedule_end",
                    serde_json::json!(window.get_schedule_end().to_string()),
                );
                if let Ok(limit) = window.get_schedule_kib().parse::<u64>() {
                    let _ = client
                        .borrow_mut()
                        .store_setting("download_speed_schedule_kib", serde_json::json!(limit));
                }
                let _ = client.borrow_mut().store_setting(
                    "auto_category_dirs",
                    serde_json::json!(window.get_auto_category()),
                );
                let _ = client.borrow_mut().store_setting(
                    "browser_category_dirs",
                    serde_json::json!({
                        "media": window.get_category_dir_media().to_string(),
                        "program": window.get_category_dir_program().to_string(),
                        "archive": window.get_category_dir_archive().to_string(),
                        "other": window.get_category_dir_other().to_string(),
                    }),
                );
                if let Ok(max) = window.get_queue_max().parse::<u64>() {
                    let _ = client
                        .borrow_mut()
                        .store_setting("queue_max_active", serde_json::json!(max.max(1)));
                }
                let _ = client.borrow_mut().store_setting(
                    "site_rules",
                    serde_json::json!(window.get_site_rules().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "av_scan_enabled",
                    serde_json::json!(window.get_av_scan()),
                );
                let _ = client.borrow_mut().store_setting(
                    "av_scan_command",
                    serde_json::json!(window.get_av_scan_command().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "torrent_watch_dir",
                    serde_json::json!(window.get_torrent_watch().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "download_dir",
                    serde_json::json!(window.get_download_dir().to_string()),
                );
                if let Ok(value) = window.get_default_concurrency().parse::<u64>() {
                    let _ = client.borrow_mut().store_setting(
                        "default_concurrency",
                        serde_json::json!(value.max(1)),
                    );
                }
                let _ = client.borrow_mut().store_setting(
                    "proxy_url",
                    serde_json::json!(window.get_proxy_url().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "ffmpeg_path",
                    serde_json::json!(window.get_ffmpeg_path().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "clipboard_watch",
                    serde_json::json!(window.get_clipboard_watch()),
                );
                let _ = client.borrow_mut().store_setting(
                    "completion_sound_enabled",
                    serde_json::json!(window.get_completion_sound()),
                );
                let _ = client.borrow_mut().store_setting(
                    "download_progress_window_enabled",
                    serde_json::json!(window.get_progress_window()),
                );
                let _ = client.borrow_mut().store_setting(
                    "download_complete_popup_enabled",
                    serde_json::json!(window.get_complete_popup()),
                );
                let _ = client.borrow_mut().store_setting(
                    "resume_interrupted_on_startup",
                    serde_json::json!(window.get_resume_interrupted()),
                );
                if let Ok(value) = window.get_auto_retry_max().parse::<u64>() {
                    let _ = client
                        .borrow_mut()
                        .store_setting("auto_retry_failed_max", serde_json::json!(value));
                }
                let _ = client.borrow_mut().store_setting(
                    "existing_file_policy",
                    serde_json::json!(window.get_existing_file_policy().to_string()),
                );
                if let Ok(value) = window.get_live_max_minutes().parse::<u64>() {
                    let _ = client
                        .borrow_mut()
                        .store_setting("live_record_max_minutes", serde_json::json!(value));
                }
                let _ = client.borrow_mut().store_setting(
                    "download_subtitles",
                    serde_json::json!(window.get_download_subtitles()),
                );
                let _ = client.borrow_mut().store_setting(
                    "skip_ad_segments",
                    serde_json::json!(window.get_skip_ad_segments()),
                );
                let _ = client.borrow_mut().store_setting(
                    "keep_temp_files",
                    serde_json::json!(window.get_keep_temp_files()),
                );
                let _ = client.borrow_mut().store_setting(
                    "default_user_agent",
                    serde_json::json!(window.get_default_user_agent().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "tvbox_endpoint",
                    serde_json::json!(window.get_tvbox_endpoint().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "default_referer",
                    serde_json::json!(window.get_default_referer().to_string()),
                );
                if let Ok(value) = window.get_http_chunk_size_mb().parse::<u64>() {
                    let _ = client.borrow_mut().store_setting(
                        "http_chunk_size_mb",
                        serde_json::json!(value.clamp(1, 64)),
                    );
                }
                let _ = client.borrow_mut().store_setting(
                    "completion_power_action",
                    serde_json::json!(window.get_completion_power_action().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "start_on_login",
                    serde_json::json!(window.get_start_on_login()),
                );
                let _ = client.borrow_mut().store_setting(
                    "allow_duplicate",
                    serde_json::json!(window.get_allow_duplicate()),
                );
                let _ = client.borrow_mut().store_setting(
                    "queue_auto_start_enabled",
                    serde_json::json!(window.get_queue_auto_start()),
                );
                let _ = client.borrow_mut().store_setting(
                    "queue_auto_start_time",
                    serde_json::json!(window.get_queue_auto_start_time().to_string()),
                );
                let _ = client.borrow_mut().store_setting(
                    "queue_auto_stop_enabled",
                    serde_json::json!(window.get_queue_auto_stop()),
                );
                let _ = client.borrow_mut().store_setting(
                    "queue_auto_stop_time",
                    serde_json::json!(window.get_queue_auto_stop_time().to_string()),
                );
                *clipboard_watch.borrow_mut() = window.get_clipboard_watch();
                *sound_enabled.borrow_mut() = window.get_completion_sound();
                *progress_window_enabled.borrow_mut() = window.get_progress_window();
                *complete_popup_enabled.borrow_mut() = window.get_complete_popup();
                let _ = window.hide();
            }
        }
    });
    player.on_command({
        let client = Rc::clone(&client);
        let selected_task = Rc::clone(&selected_task);
        let player = player.as_weak();
        let cast_devices = Rc::clone(&cast_devices);
        move |command| {
            let command = command.to_string();
            if command == "scan_cast" {
                if let Some(window) = player.upgrade() {
                    window.set_status("正在扫描局域网设备…".into());
                }
                let _ = client.borrow_mut().command(CoreCommand::DiscoverCastDevices);
                return;
            }
            if command == "play" || command == "cast" || command == "cast_device" {
                if let Some(task_id) = selected_task.borrow().clone() {
                    if command == "play" {
                        if let Some(window) = player.upgrade() {
                            let _ = window.show();
                            attach_player_embed(&mut client.borrow_mut(), &window);
                        }
                    }
                    let kind = if command == "play" {
                        CoreCommand::PlayTask { task_id }
                    } else if command == "cast_device" {
                        let label = player
                            .upgrade()
                            .map(|window| window.get_selected_device().to_string())
                            .unwrap_or_default();
                        let device_id = cast_devices
                            .borrow()
                            .iter()
                            .find(|item| item.label == label)
                            .map(|item| item.id.clone())
                            .unwrap_or_default();
                        CoreCommand::CastToDevice { task_id, device_id }
                    } else {
                        CoreCommand::CastTask { task_id }
                    };
                    match client.borrow_mut().command(kind) {
                        Ok(_) => {
                            if let Some(window) = player.upgrade() {
                                window.set_status(
                                    if command == "play" {
                                        "正在本地播放（libmpv 内嵌）"
                                    } else if command == "cast_device" {
                                        "已向选中设备发送 AVTransport"
                                    } else {
                                        "已发出投屏通知"
                                    }
                                    .into(),
                                );
                            }
                        }
                        Err(error) => {
                            if let Some(window) = player.upgrade() {
                                window.set_status(error.into());
                            }
                        }
                    }
                }
                return;
            }
            let action = command;
            let _ = client
                .borrow_mut()
                .command(CoreCommand::PlayerControl { action });
        }
    });
    progress.on_command({
        let client = Rc::clone(&client);
        let last_progress_task = Rc::clone(&last_progress_task);
        move |command| {
            let action = command.to_string();
            if !matches!(action.as_str(), "pause" | "cancel") {
                return;
            }
            if let Some(task_id) = last_progress_task.borrow().clone() {
                let _ = client.borrow_mut().command(CoreCommand::TaskAction {
                    task_id,
                    action,
                });
            }
        }
    });
    confirm.on_command({
        let client = Rc::clone(&client);
        let confirm = confirm.as_weak();
        let pending_offer = Rc::clone(&pending_offer);
        let legal = legal.as_weak();
        let bridge = Rc::clone(&bridge);
        let ui_weak = ui.as_weak();
        move |command| {
            if command == "accept" {
                if legal_blocked(&mut client.borrow_mut(), &legal) {
                    return;
                }
                let Some(offer) = pending_offer.borrow().front().cloned() else {
                    if let Some(window) = confirm.upgrade() {
                        let _ = window.hide();
                    }
                    return;
                };
                if offer.handoff_id.trim().is_empty() {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_toast_text("接管请求缺少编号，无法确认".into());
                    }
                    return;
                }
                pending_offer.borrow_mut().pop_front();
                let edited = confirm
                    .upgrade()
                    .map(|window| window.get_filename().to_string())
                    .unwrap_or_default();
                let filename = if edited.trim().is_empty() {
                    if offer.filename.trim().is_empty() {
                        offer
                            .url
                            .split(['?', '#'])
                            .next()
                            .unwrap_or(&offer.url)
                            .rsplit('/')
                            .find(|part| !part.is_empty())
                            .unwrap_or("download")
                            .to_string()
                    } else {
                        offer.filename.clone()
                    }
                } else {
                    edited
                };
                match client.borrow_mut().command(CoreCommand::AcceptHandoff {
                    handoff_id: offer.handoff_id.clone(),
                    filename,
                    download_dir: String::new(),
                }) {
                    Ok(events) => {
                        for envelope in events {
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                    Err(error) => {
                        pending_offer.borrow_mut().push_front(offer);
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_toast_text(error.into());
                        }
                        return;
                    }
                }
            } else if command == "reject" {
                if let Some(offer) = pending_offer.borrow_mut().pop_front() {
                    if offer.handoff_id.trim().is_empty() {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_toast_text("接管请求缺少编号，已关闭确认窗".into());
                        }
                    } else {
                        let _ = client.borrow_mut().command(CoreCommand::RejectHandoff {
                            handoff_id: offer.handoff_id,
                        });
                    }
                }
            }
            present_queued_offer(confirm.clone(), &pending_offer, &mut client.borrow_mut());
        }
    });
    complete.on_command({
        let client = Rc::clone(&client);
        let complete = complete.as_weak();
        let last_completed = Rc::clone(&last_completed);
        move |command| {
            if command == "cancel_power" {
                let _ = client.borrow_mut().command(CoreCommand::CancelPowerAction);
                if let Some(window) = complete.upgrade() {
                    window.set_power_hint(String::new().into());
                }
                return;
            }
            if let Some(task_id) = last_completed.borrow().clone() {
                if command == "open_file" {
                    let _ = client.borrow_mut().command(CoreCommand::OpenCompleted {
                        task_id,
                        folder: false,
                    });
                } else if command == "open_folder" {
                    let _ = client.borrow_mut().command(CoreCommand::OpenCompleted {
                        task_id,
                        folder: true,
                    });
                }
            }
            if let Some(window) = complete.upgrade() {
                let _ = window.hide();
            }
        }
    });
    legal.on_command({
        let client = Rc::clone(&client);
        let legal = legal.as_weak();
        move |command| {
            if command != "accept_legal" {
                return;
            }
            let _ = client
                .borrow_mut()
                .store_setting("legal_terms_accepted", serde_json::json!(true));
            if let Some(window) = legal.upgrade() {
                let _ = window.hide();
            }
        }
    });

    cast_hud.on_command({
        let client = Rc::clone(&client);
        let cast_hud = cast_hud.as_weak();
        move |command| {
            if command == "hide" {
                let _ = cast_hud.upgrade().map(|window| window.hide());
                return;
            }
            let _ = client.borrow_mut().command(CoreCommand::ControlCast {
                action: command.to_string(),
            });
            if command == "stop" {
                let _ = cast_hud.upgrade().map(|window| window.hide());
            }
        }
    });
    log_window.on_command({
        let log_window = log_window.as_weak();
        move |command| {
            if command == "hide" {
                let _ = log_window.upgrade().map(|window| window.hide());
            }
        }
    });
    extension.on_command({
        let extension = extension.as_weak();
        move |command| {
            if command == "hide" {
                let _ = extension.upgrade().map(|window| window.hide());
                return;
            }
            if command == "open_dir" {
                let message = match open_extension_dir() {
                    Ok(path) => format!("已打开插件目录 {}", path.display()),
                    Err(error) => error,
                };
                if let Some(window) = extension.upgrade() {
                    window.set_status(message.into());
                }
                return;
            }
            if command == "firefox" {
                open_firefox_addon_page();
            }
        }
    });
    harvest.on_command({
        let client = Rc::clone(&client);
        let harvest = harvest.as_weak();
        let new_task = new_task.as_weak();
        let weak_ui = weak_ui.clone();
        let harvest_links = Rc::clone(&harvest_links);
        let harvest_picked = Rc::clone(&harvest_picked);
        let harvest_filter = Rc::clone(&harvest_filter);
        let harvest_page = Rc::clone(&harvest_page);
        let legal = legal.as_weak();
        let bridge = Rc::clone(&bridge);
        let query = Rc::clone(&query);
        let filter = Rc::clone(&filter);
        let selected_task = Rc::clone(&selected_task);
        let picked = Rc::clone(&picked);
        let speed_samples = Rc::clone(&speed_samples);
        move |command| {
            let refill = || {
                if let Some(window) = harvest.upgrade() {
                    fill_harvest_window(
                        &window,
                        &harvest_links.borrow(),
                        &harvest_picked.borrow(),
                        harvest_filter.borrow().as_str(),
                        harvest_page.borrow().as_str(),
                    );
                }
            };
            if command == "hide" {
                let _ = harvest.upgrade().map(|window| window.hide());
                return;
            }
            if let Some(next) = command.strip_prefix("harvest_filter:") {
                *harvest_filter.borrow_mut() = next.to_string();
                refill();
                return;
            }
            if let Some(url) = command.strip_prefix("harvest_toggle:") {
                let mut picked = harvest_picked.borrow_mut();
                if !picked.remove(url) {
                    picked.insert(url.to_string());
                }
                drop(picked);
                refill();
                return;
            }
            if command == "harvest_all" || command == "harvest_none" {
                let current = harvest_filter.borrow().clone();
                let urls: Vec<String> = harvest_links
                    .borrow()
                    .iter()
                    .filter(|link| harvest_matches(&link.category, &current))
                    .map(|link| link.url.clone())
                    .collect();
                let mut picked = harvest_picked.borrow_mut();
                if command == "harvest_all" {
                    picked.extend(urls);
                } else {
                    for url in urls {
                        picked.remove(&url);
                    }
                }
                drop(picked);
                refill();
                return;
            }
            if command != "harvest_add" {
                return;
            }
            if legal_blocked(&mut client.borrow_mut(), &legal) {
                return;
            }
            let current = harvest_filter.borrow().clone();
            let chosen: Vec<HarvestCandidate> = harvest_links
                .borrow()
                .iter()
                .filter(|link| {
                    harvest_picked.borrow().contains(&link.url)
                        && harvest_matches(&link.category, &current)
                })
                .cloned()
                .collect();
            if chosen.is_empty() {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.set_toast_text("请先勾选要添加的链接".into());
                }
                return;
            }
            let referer = new_task
                .upgrade()
                .map(|window| window.get_referer().to_string())
                .unwrap_or_default();
            let cookie = new_task
                .upgrade()
                .map(|window| window.get_cookie().to_string())
                .unwrap_or_default();
            let mut added = 0usize;
            for (index, link) in chosen.iter().enumerate() {
                let mut headers = std::collections::BTreeMap::new();
                if !referer.trim().is_empty() {
                    headers.insert("Referer".into(), referer.trim().to_string());
                }
                if index == 0 && !cookie.trim().is_empty() {
                    headers.insert("Cookie".into(), cookie.trim().to_string());
                }
                match client.borrow_mut().command(CoreCommand::CreateTask {
                    spec: TaskSpec {
                        url: link.url.clone(),
                        resource_kind: classify_url(&link.url),
                        filename: link.filename.clone(),
                        headers,
                        allow_duplicate: true,
                        ..Default::default()
                    },
                }) {
                    Ok(events) => {
                        added += 1;
                        for envelope in events {
                            bridge.borrow_mut().apply(envelope.event);
                        }
                    }
                    Err(error) => {
                        if let Some(ui) = weak_ui.upgrade() {
                            ui.set_core_status(error.into());
                        }
                    }
                }
            }
            if let Some(window) = harvest.upgrade() {
                let _ = window.hide();
            }
            if let Some(ui) = weak_ui.upgrade() {
                ui.set_toast_text(format!("已添加 {added} 个抓取任务").into());
                refresh_tasks(
                    &ui,
                    &bridge.borrow(),
                    &query.borrow(),
                    &filter.borrow(),
                    selected_task.borrow().as_deref(),
                    &picked.borrow(),
                    &mut speed_samples.borrow_mut(),
                );
            }
        }
    });

    let drop_timer = Timer::default();
    {
        let drop_tx = drop_tx;
        let weak_ui = weak_ui.clone();
        let attached = Rc::new(RefCell::new(false));
        drop_timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(200),
            move || {
                if *attached.borrow() {
                    return;
                }
                let weak_ui = weak_ui.clone();
                if attach_file_drop("HLS Downloader", drop_tx.clone(), move || {
                    let weak_ui = weak_ui.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = weak_ui.upgrade() {
                            window.invoke_wake();
                        }
                    });
                }) {
                    *attached.borrow_mut() = true;
                }
            },
        );
    }

    ui.run()?;
    server.shutdown();
    clipboard_timer.stop();
    drop_timer.stop();
    Ok(())
}

fn legal_blocked(client: &mut CoreIpcClient, legal: &slint::Weak<LegalWindow>) -> bool {
    if std::env::var_os("HLS_V6_SKIP_LEGAL").is_some() {
        return false;
    }
    let accepted = matches!(
        client.load_settings(),
        Ok(CorePipeResponse::Settings {
            legal_accepted: true,
            ..
        })
    );
    if accepted {
        return false;
    }
    let _ = legal.upgrade().map(|window| window.show());
    true
}

fn harvest_matches(category: &str, filter: &str) -> bool {
    filter == "全部" || filter == "all" || category == filter
}

fn harvest_category_label(category: &str) -> &'static str {
    match category {
        "video" => "视频",
        "audio" => "音频",
        "archive" => "压缩包",
        "document" => "文档",
        "program" => "程序",
        "playlist" => "清单",
        "torrent" => "种子",
        _ => "其他",
    }
}

fn fill_harvest_window(
    window: &HarvestWindow,
    links: &[HarvestCandidate],
    picked: &HashSet<String>,
    filter: &str,
    page_url: &str,
) {
    let mut video = 0u32;
    let mut audio = 0u32;
    let mut archive = 0u32;
    let mut document = 0u32;
    let mut program = 0u32;
    let mut playlist = 0u32;
    let mut torrent = 0u32;
    for link in links {
        match link.category.as_str() {
            "video" => video += 1,
            "audio" => audio += 1,
            "archive" => archive += 1,
            "document" => document += 1,
            "program" => program += 1,
            "playlist" => playlist += 1,
            "torrent" => torrent += 1,
            _ => {}
        }
    }
    window.set_all_count(links.len().to_string().into());
    window.set_video_count(video.to_string().into());
    window.set_audio_count(audio.to_string().into());
    window.set_archive_count(archive.to_string().into());
    window.set_document_count(document.to_string().into());
    window.set_program_count(program.to_string().into());
    window.set_playlist_count(playlist.to_string().into());
    window.set_torrent_count(torrent.to_string().into());
    window.set_filter(filter.into());
    window.set_status(
        if page_url.trim().is_empty() {
            format!("{} 条静态链接", links.len())
        } else {
            format!("{} · {} 条", page_url, links.len())
        }
        .into(),
    );
    let rows: Vec<HarvestRow> = links
        .iter()
        .filter(|link| harvest_matches(&link.category, filter))
        .map(|link| {
            let label = if link.filename.trim().is_empty() {
                link.url.clone()
            } else {
                link.filename.clone()
            };
            let zh = harvest_category_label(&link.category);
            let detail = if link.extension.is_empty() {
                zh.to_string()
            } else {
                format!("{zh} · .{}", link.extension)
            };
            HarvestRow {
                url: link.url.clone().into(),
                label: label.into(),
                category: zh.into(),
                detail: detail.into(),
                picked: picked.contains(&link.url),
            }
        })
        .collect();
    window.set_links(ModelRc::new(VecModel::from(rows)));
}

fn refresh_tasks(
    ui: &MainWindow,
    bridge: &ui_model::UiBridge,
    query: &str,
    filter: &str,
    selected: Option<&str>,
    picked: &HashSet<String>,
    speed_samples: &mut Vec<f32>,
) {
    let mut rows = bridge.filtered_rows(query, filter);
    for row in rows.iter_mut() {
        row.picked = picked.contains(row.task_id.as_str());
    }
    ui.set_tasks(ModelRc::new(VecModel::from(rows)));
    let counts = bridge.counts();
    ui.set_all_count(counts.all.to_string().into());
    ui.set_downloading_count(counts.downloading.to_string().into());
    ui.set_queued_count(counts.queued.to_string().into());
    ui.set_paused_count(counts.paused.to_string().into());
    ui.set_processing_count(counts.processing.to_string().into());
    ui.set_completed_count(counts.completed.to_string().into());
    ui.set_failed_count(counts.failed.to_string().into());
    ui.set_media_count(counts.media.to_string().into());
    ui.set_program_count(counts.programs.to_string().into());
    ui.set_archive_count(counts.archives.to_string().into());
    ui.set_other_count(counts.other.to_string().into());
    ui.set_detail_line(bridge.detail_line(selected.unwrap_or("")).into());
    ui.set_selected_task(selected.unwrap_or("").into());
    let detail_map = selected
        .and_then(|task_id| bridge.snapshot(task_id))
        .map(|snapshot| {
            sample_cells(
                &snapshot.connection_parts,
                snapshot.total_bytes.unwrap_or(0),
                snapshot.downloaded_bytes,
                48,
            )
        })
        .unwrap_or_default();
    ui.set_detail_map(ModelRc::new(VecModel::from(detail_map)));
    let total: u64 = bridge
        .all()
        .iter()
        .filter(|task| ui_model::is_active_transfer(&task.status))
        .map(|task| task.speed_bytes_per_sec)
        .sum();
    ui.set_total_speed(format_speed(total).into());
    if !speed_samples.is_empty() {
        speed_samples.remove(0);
        let peak = speed_samples.iter().copied().fold(total as f32, f32::max).max(1.0);
        speed_samples.push((total as f32 / peak).clamp(0.0, 1.0));
        ui.set_speed_bars(ModelRc::new(VecModel::from(speed_samples.clone())));
    }
    if let Some(status) = bridge.last_status() {
        if status.starts_with("update_") {
            ui.set_core_status(status.split_once(": ").map(|(_, rest)| rest).unwrap_or(status).into());
        }
    }
}

fn target_task_ids(selected: &Option<String>, picked: &HashSet<String>) -> Vec<String> {
    if !picked.is_empty() {
        picked.iter().cloned().collect()
    } else {
        selected.iter().cloned().collect()
    }
}

fn parse_drop_payload(data: &str) -> Vec<String> {
    let mut values = Vec::new();
    for raw in data.split(|ch: char| ch == '\n' || ch == '\r' || ch == '\0') {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }
        if let Some(path) = item.strip_prefix("file:///") {
            values.push(urlencoding_decode(&path.replace('/', "\\")));
        } else if let Some(path) = item.strip_prefix("file://") {
            values.push(urlencoding_decode(path));
        } else if item.starts_with("http://")
            || item.starts_with("https://")
            || item.starts_with("magnet:")
            || item.starts_with("ftp://")
            || std::path::Path::new(item).exists()
        {
            values.push(item.to_string());
        }
    }
    values
}

fn urlencoding_decode(value: &str) -> String {
    let mut out = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""), 16)
            {
                out.push(byte as char);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

fn fill_settings(client: &mut CoreIpcClient, window: &SettingsWindow) {
    let Ok(CorePipeResponse::Settings {
        takeover_enabled,
        legal_accepted,
        speed_limit_kib,
        schedule_enabled,
        schedule_start,
        schedule_end,
        schedule_kib,
        auto_category,
        category_dir_media,
        category_dir_program,
        category_dir_archive,
        category_dir_other,
        queue_max,
        site_rules,
        av_scan_enabled,
        av_scan_command,
        torrent_watch,
        download_dir,
        default_concurrency,
        proxy_url,
        ffmpeg_path,
        clipboard_watch,
        completion_sound_enabled,
        progress_window_enabled,
        complete_popup_enabled,
        resume_interrupted,
        auto_retry_max,
        existing_file_policy,
        live_record_max_minutes,
        download_subtitles,
        skip_ad_segments,
        keep_temp_files,
        default_user_agent,
        tvbox_endpoint,
        allow_duplicate,
        queue_auto_start_enabled,
        queue_auto_start_time,
        queue_auto_stop_enabled,
        queue_auto_stop_time,
        default_referer,
        http_chunk_size_mb,
        completion_power_action,
        start_on_login,
        ..
    }) = client.load_settings()
    else {
        return;
    };
    window.set_takeover_enabled(takeover_enabled);
    window.set_legal_accepted(legal_accepted);
    window.set_speed_limit(speed_limit_kib.to_string().into());
    window.set_schedule_enabled(schedule_enabled);
    window.set_schedule_start(schedule_start.into());
    window.set_schedule_end(schedule_end.into());
    window.set_schedule_kib(schedule_kib.to_string().into());
    window.set_auto_category(auto_category);
    window.set_category_dir_media(category_dir_media.into());
    window.set_category_dir_program(category_dir_program.into());
    window.set_category_dir_archive(category_dir_archive.into());
    window.set_category_dir_other(category_dir_other.into());
    window.set_queue_max(queue_max.to_string().into());
    window.set_site_rules(site_rules.into());
    window.set_av_scan(av_scan_enabled);
    window.set_av_scan_command(av_scan_command.into());
    window.set_torrent_watch(torrent_watch.into());
    window.set_download_dir(download_dir.into());
    window.set_default_concurrency(default_concurrency.to_string().into());
    window.set_proxy_url(proxy_url.into());
    window.set_ffmpeg_path(ffmpeg_path.into());
    window.set_clipboard_watch(clipboard_watch);
    window.set_completion_sound(completion_sound_enabled);
    window.set_progress_window(progress_window_enabled);
    window.set_complete_popup(complete_popup_enabled);
    window.set_resume_interrupted(resume_interrupted);
    window.set_auto_retry_max(auto_retry_max.to_string().into());
    window.set_existing_file_policy(existing_file_policy.into());
    window.set_live_max_minutes(live_record_max_minutes.to_string().into());
    window.set_download_subtitles(download_subtitles);
    window.set_skip_ad_segments(skip_ad_segments);
    window.set_keep_temp_files(keep_temp_files);
    window.set_default_user_agent(default_user_agent.into());
    window.set_tvbox_endpoint(tvbox_endpoint.into());
    window.set_allow_duplicate(allow_duplicate);
    window.set_queue_auto_start(queue_auto_start_enabled);
    window.set_queue_auto_start_time(queue_auto_start_time.into());
    window.set_queue_auto_stop(queue_auto_stop_enabled);
    window.set_queue_auto_stop_time(queue_auto_stop_time.into());
    window.set_default_referer(default_referer.into());
    window.set_http_chunk_size_mb(http_chunk_size_mb.to_string().into());
    window.set_completion_power_action(completion_power_action.into());
    window.set_start_on_login(start_on_login);
}

fn attach_player_embed(client: &mut CoreIpcClient, window: &PlayerWindow) {
    let size = window.window().size();
    let width = size.width as i32;
    let height = size.height as i32;
    let host_h = (height - 48 - 176).max(80);
    let _ = client.command(CoreCommand::PlayerControl {
        action: format!("embed_host:0,48,{width},{host_h}"),
    });
}

fn apply_curl_to_window(window: &NewTaskWindow, curl: hls_native_shell::CurlDownload) {
    window.set_url(curl.url.into());
    window.set_referer(curl.referer.into());
    window.set_cookie(curl.cookie.into());
    if !curl.headers.is_empty() {
        let extra = curl
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        window.set_extra_headers(extra.into());
    }
    if !curl.body.is_empty() {
        let path = std::env::temp_dir().join(format!(
            "hls-curl-body-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|item| item.as_millis())
                .unwrap_or(0)
        ));
        if std::fs::write(&path, curl.body).is_ok() {
            window.set_body_path(path.to_string_lossy().to_string().into());
        }
    }
    window.set_recognize_label("已解析 cURL".into());
}

fn show_confirm_offer(confirm: slint::Weak<ConfirmWindow>, offer: &ResourceOffer, queued: usize) -> bool {
    let Some(window) = confirm.upgrade() else {
        return false;
    };
    window.set_url(offer.url.clone().into());
    let filename = if offer.filename.trim().is_empty() {
        offer
            .url
            .split(['?', '#'])
            .next()
            .unwrap_or(&offer.url)
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("download")
            .to_string()
    } else {
        offer.filename.clone()
    };
    window.set_filename(filename.into());
    window.set_size_text(
        if offer.size > 0 {
            format_bytes(offer.size)
        } else {
            "浏览器接管".into()
        }
        .into(),
    );
    window.set_remaining(if queued > 1 {
        format!("后面还有 {} 个", queued - 1).into()
    } else {
        String::new().into()
    });
    window.show().is_ok()
}

fn report_handoff_presentation(
    client: &mut CoreIpcClient,
    offer: &ResourceOffer,
    ok: bool,
) {
    if offer.handoff_id.trim().is_empty() {
        return;
    }
    let _ = client.command(CoreCommand::PresentHandoff {
        handoff_id: offer.handoff_id.clone(),
        ok,
    });
}

fn present_queued_offer(
    confirm: slint::Weak<ConfirmWindow>,
    pending_offer: &RefCell<VecDeque<ResourceOffer>>,
    client: &mut CoreIpcClient,
) {
    loop {
        let Some(offer) = pending_offer.borrow().front().cloned() else {
            if let Some(window) = confirm.upgrade() {
                let _ = window.hide();
            }
            return;
        };
        let queued = pending_offer.borrow().len();
        let shown = show_confirm_offer(confirm.clone(), &offer, queued);
        report_handoff_presentation(client, &offer, shown);
        if shown {
            return;
        }
        pending_offer.borrow_mut().pop_front();
    }
}

fn export_task_urls(tasks: &[TaskSnapshot]) -> String {
    let mut lines = Vec::new();
    for task in tasks {
        if task.url.trim().is_empty() {
            continue;
        }
        let name = if task.filename.trim().is_empty() {
            task.title.trim()
        } else {
            task.filename.trim()
        };
        if !name.is_empty() {
            lines.push(format!("# {name}"));
        }
        lines.push(task.url.clone());
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn format_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1024 * 1024 {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1024.0 / 1024.0)
    } else if bytes_per_sec >= 1024 {
        format!("{:.1} KB/s", bytes_per_sec as f64 / 1024.0)
    } else {
        format!("{bytes_per_sec} B/s")
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

fn create_tasks_from_local_files(
    client: &mut CoreIpcClient,
    bridge: &RefCell<ui_model::UiBridge>,
) -> Vec<String> {
    let mut ids = Vec::new();
    for path in pick_import_paths() {
        match client.command(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: path.to_string_lossy().into_owned(),
                ..Default::default()
            },
        }) {
            Ok(events) => {
                for envelope in events {
                    match &envelope.event {
                        CoreEvent::TaskCreated { snapshot }
                        | CoreEvent::TaskUpdated { snapshot } => {
                            ids.push(snapshot.task_id.clone());
                        }
                        _ => {}
                    }
                    bridge.borrow_mut().apply(envelope.event);
                }
            }
            Err(_) => {}
        }
    }
    ids
}

fn locate_extension_unpacked_dir() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..8 {
        let chrome = dir.join("extension").join(".output").join("chrome-mv3");
        if chrome.is_dir() {
            return Some(chrome);
        }
        let unpacked = dir.join("extension");
        if unpacked.join("wxt.config.ts").is_file() || unpacked.join("entrypoints").is_dir() {
            return Some(unpacked);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

fn open_extension_dir() -> Result<std::path::PathBuf, String> {
    let dir = locate_extension_unpacked_dir().ok_or_else(|| "找不到插件目录".to_string())?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(dir)
}

fn open_firefox_addon_page() {
    let _ = std::process::Command::new("explorer")
        .arg("https://addons.mozilla.org/zh-CN/firefox/addon/hls_downloader/")
        .spawn();
}
