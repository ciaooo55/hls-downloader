use crate::queue_editor::QueueEditor;
use crate::resource_tools::ResourceTools;
use crate::{
    file_dialogs, native_window, settings_editor::SettingsEditor, task_list::TaskList,
    DetailsWindow, NavigationRow, TaskEditorWindow, TaskMenuRow, TaskRow, Tokens, WorkbenchWindow,
};
use hls_client::{
    CoreCommand, CoreEvent, CoreIpcClient, CorePipeRequest, CorePipeResponse, EventEnvelope,
    ResourceKind, StreamVariant, TaskSnapshot, TaskSpec,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::mpsc,
};

enum WorkbenchJob {
    Request(Box<CorePipeRequest>),
    Snapshot(u64),
    Batch(u64, Vec<CoreCommand>),
}

struct QueueSave {
    request_id: u64,
    assignment: Option<(Vec<String>, String)>,
}

pub struct Workbench {
    pub window: WorkbenchWindow,
    state: Rc<RefCell<TaskList>>,
    rows: Rc<VecModel<TaskRow>>,
    navigation: Rc<VecModel<NavigationRow>>,
    positions: RefCell<HashMap<String, usize>>,
    commands: RefCell<mpsc::SyncSender<WorkbenchJob>>,
    editor_commands: mpsc::SyncSender<WorkbenchJob>,
    resource_commands: mpsc::SyncSender<WorkbenchJob>,
    system_commands: mpsc::SyncSender<WorkbenchJob>,
    next_id: Cell<u64>,
    editor: RefCell<Option<TaskEditorWindow>>,
    editor_id: Cell<u64>,
    editor_variants: RefCell<Vec<StreamVariant>>,
    editor_queues: RefCell<Vec<hls_client::QueueProfile>>,
    details: RefCell<Option<(String, DetailsWindow)>>,
    details_full_log: Cell<bool>,
    settings: RefCell<Option<SettingsEditor>>,
    settings_response: RefCell<Option<CorePipeResponse>>,
    settings_open_id: Cell<Option<u64>>,
    settings_save_id: Cell<Option<u64>>,
    queues: RefCell<Option<QueueEditor>>,
    queues_open_id: Cell<Option<u64>>,
    queue_save: RefCell<Option<QueueSave>>,
    tools: RefCell<Option<Rc<ResourceTools>>>,
    media: RefCell<Option<Rc<crate::media_controller::MediaController>>>,
    system: RefCell<Option<Rc<crate::system_tools::SystemTools>>>,
    service_settings: RefCell<Option<Rc<crate::service_settings::ServiceSettings>>>,
    cast: RefCell<Option<Rc<crate::cast_tools::CastTools>>>,
    exports: RefCell<HashMap<u64, PathBuf>>,
    outstanding: RefCell<HashSet<u64>>,
    dropped_paths: RefCell<Vec<String>>,
    closing: Cell<bool>,
    engine_online: Cell<bool>,
    notice_expires_at: Cell<Option<std::time::Instant>>,
    status_timer: slint::Timer,
}

impl Workbench {
    const NOTICE_AUTO_DISMISS: std::time::Duration = std::time::Duration::from_secs(5);

    pub fn set_engine_online(&self, online: bool) {
        self.engine_online.set(online);
        self.window.set_engine_online(online);
        self.window.set_connection(
            if online {
                "下载引擎已连接"
            } else {
                "正在重连下载引擎…"
            }
            .into(),
        );
    }

    pub fn set_browser_connected(&self, connected: bool) {
        self.window.set_browser_status(
            if connected {
                "浏览器已连接"
            } else {
                "浏览器未连接"
            }
            .into(),
        );
    }

    pub fn show_notice(&self, tone: &str, message: impl Into<slint::SharedString>) {
        self.show_notice_at(tone, message, std::time::Instant::now());
    }

    fn show_notice_at(
        &self,
        tone: &str,
        message: impl Into<slint::SharedString>,
        now: std::time::Instant,
    ) {
        let message: slint::SharedString = message.into();
        if message.is_empty() {
            self.clear_notice();
            return;
        }
        let normalized = match tone {
            "error" | "warning" | "success" => tone,
            _ => "info",
        };
        self.window.set_notice_tone(normalized.into());
        self.window.set_notice(message);
        if matches!(normalized, "info" | "success") {
            self.notice_expires_at
                .set(Some(now + Self::NOTICE_AUTO_DISMISS));
        } else {
            self.notice_expires_at.set(None);
        }
    }

    pub fn clear_notice(&self) {
        self.notice_expires_at.set(None);
        if !self.window.get_notice().is_empty() {
            self.window.set_notice("".into());
        }
    }

    fn expire_notice_at(&self, now: std::time::Instant) {
        if let Some(deadline) = self.notice_expires_at.get() {
            if now >= deadline {
                self.clear_notice();
            }
        }
    }

    fn apply_appearance(&self, dark: bool, reduce_motion: bool) {
        native_window::apply_appearance(&self.window, dark, reduce_motion);
        if let Some(editor) = self.editor.borrow().as_ref() {
            native_window::inherit_appearance(editor, &self.window);
        }
        if let Some((_, details)) = self.details.borrow().as_ref() {
            native_window::inherit_appearance(details, &self.window);
        }
        if let Some(queues) = self.queues.borrow().as_ref() {
            native_window::inherit_appearance(&queues.window, &self.window);
        }
        if let Some(tools) = self.tools.borrow().as_ref() {
            native_window::inherit_appearance(&tools.window, &self.window);
        }
        if let Some(service) = self.service_settings.borrow().as_ref() {
            native_window::inherit_appearance(&service.window, &self.window);
        }
        if let Some(cast) = self.cast.borrow().as_ref() {
            cast.sync_appearance();
        }
        if let Some(system) = self.system.borrow().as_ref() {
            system.sync_appearance();
        }
        if let Some(media) = self.media.borrow().as_ref() {
            media.sync_appearance();
        }

        if let Some(editor) = self.settings.borrow().as_ref() {
            native_window::inherit_appearance(&editor.window, &self.window);
        }
    }

    fn refresh_theme(&self) {
        let response = self.settings_response.borrow();
        let Some(response) = response.as_ref() else {
            return;
        };
        let mode = crate::theme::ThemeMode::from_response(Some(response));
        let dark = mode.is_dark();
        let tokens = self.window.global::<Tokens>();
        if tokens.get_dark() != dark {
            self.apply_appearance(dark, tokens.get_reduce_motion());
        }
    }

    fn spawn_command_worker(
        window: &WorkbenchWindow,
        capacity: usize,
    ) -> mpsc::SyncSender<WorkbenchJob> {
        let weak = window.as_weak();
        let mut client = None;
        let mut last_request = std::time::Instant::now();
        Self::spawn_job_worker(capacity, move |job| {
            // Core closes idle command pipes after 120 s. Reconnect before sending,
            // rather than retrying a mutation after its receipt becomes uncertain.
            if last_request.elapsed() >= std::time::Duration::from_secs(60) {
                client = None;
            }
            let failure_id = match &job {
                WorkbenchJob::Snapshot(id) | WorkbenchJob::Batch(id, _) => Some(*id),
                WorkbenchJob::Request(request) => request.request_id(),
            };
            let result = (|| {
                let client = match client.as_mut() {
                    Some(existing) => existing,
                    None => client.insert(CoreIpcClient::connect()?),
                };
                match job {
                    WorkbenchJob::Request(request) => client.request(&request),
                    WorkbenchJob::Snapshot(request_id) => client.snapshot_state_with_browser().map(
                        |(tasks, latest_sequence, browser_connected)| CorePipeResponse::Snapshot {
                            request_id,
                            tasks,
                            latest_sequence,
                            browser_connected,
                        },
                    ),
                    WorkbenchJob::Batch(request_id, commands) => {
                        let mut events = Vec::new();
                        for (index, command) in commands.into_iter().enumerate() {
                            match client.request(&CorePipeRequest::Command {
                                request_id,
                                command,
                            }) {
                                Ok(CorePipeResponse::Events { events: result, .. }) => {
                                    events.extend(result)
                                }
                                Ok(CorePipeResponse::Error { message, .. }) => {
                                    events.push(EventEnvelope {
                                        sequence: 0,
                                        event: CoreEvent::Error {
                                            code: "batch_item_failed".into(),
                                            message: format!("第 {} 项：{message}", index + 1),
                                        },
                                    })
                                }
                                Ok(_) => {
                                    events.push(EventEnvelope {
                                        sequence: 0,
                                        event: CoreEvent::Error {
                                            code: "batch_interrupted".into(),
                                            message: format!(
                                                "第 {} 项响应不兼容，后续项目未提交",
                                                index + 1
                                            ),
                                        },
                                    });
                                    break;
                                }
                                Err(error) => {
                                    // Never replay the remaining batch after transport failure.
                                    events.push(EventEnvelope {
                                        sequence: 0,
                                        event: CoreEvent::Error {
                                            code: "batch_interrupted".into(),
                                            message: format!(
                                                "第 {} 项未能确认接收，后续项目未提交：{error}",
                                                index + 1
                                            ),
                                        },
                                    });
                                    break;
                                }
                            }
                        }
                        Ok(CorePipeResponse::Events { request_id, events })
                    }
                }
            })();
            last_request = std::time::Instant::now();
            // A lost reply must not repeat a mutation. The event stream
            // restores its committed state; the next action reconnects.
            if result.is_err() {
                client = None;
            }
            let response = result.unwrap_or_else(|message| CorePipeResponse::Error {
                request_id: failure_id,
                code: "ui_connection".into(),
                message,
            });
            let Ok(json) = serde_json::to_string(&response) else {
                return true;
            };
            weak.upgrade_in_event_loop(move |ui| ui.invoke_response_json(json.into()))
                .is_ok()
        })
    }

    fn spawn_job_worker(
        capacity: usize,
        mut dispatch: impl FnMut(WorkbenchJob) -> bool + Send + 'static,
    ) -> mpsc::SyncSender<WorkbenchJob> {
        let (commands, jobs) = mpsc::sync_channel(capacity);
        std::thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                if !dispatch(job) {
                    break;
                }
            }
        });
        commands
    }

    pub fn new(
        tasks: Vec<TaskSnapshot>,
        sequence: u64,
        browser_connected: bool,
        dark: bool,
        reduce_motion: bool,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let window = WorkbenchWindow::new()?;
        window.set_product_version(env!("CARGO_PKG_VERSION").into());
        window.set_browser_status(
            if browser_connected {
                "浏览器已连接"
            } else {
                "浏览器未连接"
            }
            .into(),
        );
        native_window::apply_appearance(&window, dark, reduce_motion);
        let state = Rc::new(RefCell::new(TaskList::default()));
        state.borrow_mut().replace(tasks, sequence);
        let rows = Rc::new(VecModel::default());
        window.set_tasks(ModelRc::from(rows.clone()));
        let navigation = Rc::new(VecModel::default());
        window.set_navigation(ModelRc::from(navigation.clone()));
        let commands = Self::spawn_command_worker(&window, 32);
        // Task setup and resource tools can each wait for network metadata.
        let editor_commands = Self::spawn_command_worker(&window, 8);
        let resource_commands = Self::spawn_command_worker(&window, 8);
        let system_commands = Self::spawn_command_worker(&window, 8);
        let controller = Rc::new(Self {
            window,
            state,
            rows,
            navigation,
            positions: RefCell::new(HashMap::new()),
            commands: RefCell::new(commands),
            editor_commands,
            resource_commands,
            system_commands,
            next_id: Cell::new(1),
            editor: RefCell::new(None),
            editor_id: Cell::new(0),
            editor_variants: RefCell::new(Vec::new()),
            editor_queues: RefCell::new(crate::queue_editor::ordered_profiles(&[])),
            details: RefCell::new(None),
            details_full_log: Cell::new(false),
            settings: RefCell::new(None),
            settings_response: RefCell::new(None),
            settings_open_id: Cell::new(None),
            settings_save_id: Cell::new(None),
            exports: RefCell::new(HashMap::new()),
            queues: RefCell::new(None),
            queues_open_id: Cell::new(None),
            queue_save: RefCell::new(None),
            tools: RefCell::new(None),
            media: RefCell::new(None),
            system: RefCell::new(None),
            service_settings: RefCell::new(None),
            cast: RefCell::new(None),
            status_timer: slint::Timer::default(),
            outstanding: RefCell::new(HashSet::new()),
            closing: Cell::new(false),
            engine_online: Cell::new(true),
            notice_expires_at: Cell::new(None),
            dropped_paths: RefCell::new(Vec::new()),
        });
        let request_owner = Rc::downgrade(&controller);
        let id_owner = Rc::downgrade(&controller);
        let media = crate::media_controller::MediaController::new(
            controller.window.clone_strong(),
            Rc::new(move |request| {
                request_owner
                    .upgrade()
                    .is_some_and(|this| this.request(request))
            }),
            Rc::new(move || id_owner.upgrade().map_or(0, |this| this.id())),
        );
        *controller.media.borrow_mut() = Some(media);
        let request_owner = Rc::downgrade(&controller);
        let id_owner = Rc::downgrade(&controller);
        *controller.system.borrow_mut() = Some(crate::system_tools::SystemTools::new(
            controller.window.clone_strong(),
            Rc::new(move |request| {
                request_owner.upgrade().is_some_and(|this| {
                    this.enqueue(
                        &this.system_commands,
                        WorkbenchJob::Request(Box::new(request)),
                    )
                })
            }),
            Rc::new(move || id_owner.upgrade().map_or(0, |this| this.id())),
        )?);
        let id_owner = Rc::downgrade(&controller);
        *controller.cast.borrow_mut() = Some(crate::cast_tools::CastTools::new(
            controller.window.clone_strong(),
            Rc::new(move || id_owner.upgrade().map_or(0, |this| this.id())),
        )?);
        controller.bind();
        #[cfg(windows)]
        {
            use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
            let weak = controller.window.as_weak();
            let frame_trace = std::env::var_os("HLS_V7_PRESENTER_FRAME_TRACE");
            let trace_started = std::time::Instant::now();
            let mut frame_times = Vec::new();
            controller
                .window
                .window()
                .on_winit_window_event(move |window, event| {
                    match event {
                        winit::event::WindowEvent::RedrawRequested if frame_trace.is_some() => {
                            if frame_times.len() < 4096 {
                                frame_times.push(trace_started.elapsed().as_secs_f64());
                            }
                        }
                        winit::event::WindowEvent::CloseRequested => {
                            if let Some(path) = frame_trace.as_ref() {
                                if let Ok(data) = serde_json::to_vec(&frame_times) {
                                    let _ = std::fs::write(path, data);
                                }
                            }
                        }
                        winit::event::WindowEvent::Resized(size) => {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_viewport_width(size.width as f32 / window.scale_factor());
                                ui.set_viewport_height(size.height as f32 / window.scale_factor());
                                ui.invoke_viewport_resized(
                                    size.width as f32 / window.scale_factor(),
                                );
                            }
                        }
                        winit::event::WindowEvent::HoveredFile(_) => {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_file_drop_hover(true);
                            }
                        }
                        winit::event::WindowEvent::HoveredFileCancelled => {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_file_drop_hover(false);
                            }
                        }
                        winit::event::WindowEvent::DroppedFile(path) => {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_file_drop_hover(false);
                                ui.invoke_files_dropped(ModelRc::new(VecModel::from(vec![path
                                    .to_string_lossy()
                                    .into_owned()
                                    .into()])));
                            }
                        }
                        _ => {}
                    }
                    EventResult::Propagate
                });
        }
        controller.render();
        controller.render_columns();
        controller.render_toolbar();
        let weak = Rc::downgrade(&controller);
        controller.status_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(250),
            move || {
                if let Some(this) = weak.upgrade() {
                    let paths = std::mem::take(&mut *this.dropped_paths.borrow_mut());
                    if !paths.is_empty() {
                        this.send(CoreCommand::ImportPaths { paths });
                    }
                    this.expire_notice_at(std::time::Instant::now());
                    this.refresh_theme();
                    this.render_details();
                    if let Some(tools) = this.tools.borrow().as_ref() {
                        tools.poll();
                    }
                    if this.window.window().is_visible() {
                        this.render_status();
                    } else if this.ready_to_release() {
                        this.window.invoke_request_close();
                    }
                }
            },
        );
        controller.request(CorePipeRequest::LoadSettings {
            request_id: controller.id(),
        });
        Ok(controller)
    }

    fn bind(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.on_viewport_resized(move |_| {
            if let Some(this) = weak.upgrade() {
                this.render_columns();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_inspector_opened(move || {
            if let Some(this) = weak.upgrade() {
                this.render_columns();
                this.render_inspector();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_dismiss_notice(move || {
            if let Some(this) = weak.upgrade() {
                this.clear_notice();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_place_row(move |id, target| {
            if let Some(this) = weak.upgrade() {
                let state = this.state.borrow();
                if state.sort != "queue:asc" && state.sort != "queue" {
                    this.show_notice("warning", "请先在工具中选择“队列顺序”，再拖动排序");
                    return;
                }
                if id == target {
                    return;
                }
                let Some(source) = state.tasks.get(id.as_str()) else {
                    return;
                };
                let Some(destination) = state.tasks.get(target.as_str()) else {
                    return;
                };
                if source.queue_id != destination.queue_id {
                    this.show_notice("warning", "请使用“移动到队列”跨队列调整任务");
                    return;
                }
                let before = if source.queue_index < destination.queue_index {
                    state
                        .visible
                        .iter()
                        .filter_map(|key| state.tasks.get(key))
                        .filter(|t| {
                            t.queue_id == source.queue_id && t.queue_index > destination.queue_index
                        })
                        .min_by_key(|t| t.queue_index)
                        .map_or(String::new(), |t| t.task_id.clone())
                } else {
                    target.to_string()
                };
                this.send(CoreCommand::PlaceQueue {
                    task_id: id.to_string(),
                    before_id: before,
                });
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_select_row(move |id, ctrl, shift| {
            if let Some(this) = weak.upgrade() {
                this.state.borrow_mut().select(&id, ctrl, shift);
                this.render_selection();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_filter(move |filter| {
            if let Some(this) = weak.upgrade() {
                let mut state = this.state.borrow_mut();
                state.filter = if state.filter == filter.as_str()
                    && (filter.starts_with("queue:") || filter.starts_with("category:"))
                {
                    "all".into()
                } else {
                    filter.into()
                };
                state.rebuild();
                drop(state);
                this.render();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_query_edited(move |query| {
            if let Some(this) = weak.upgrade() {
                this.state.borrow_mut().query = query.into();
                this.state.borrow_mut().rebuild();
                this.render();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_sort(move |sort| {
            if let Some(this) = weak.upgrade() {
                let mut state = this.state.borrow_mut();
                let descending = state.sort.split(':').next() == Some(sort.as_str())
                    && !state.sort.ends_with(":desc");
                state.sort = format!("{sort}:{}", if descending { "desc" } else { "asc" });
                let stored = state.sort.clone();
                state.rebuild();
                drop(state);
                this.render();
                if sort != "eta" {
                    this.send(CoreCommand::SetSetting {
                        key: "task_sort".into(),
                        value: stored.into(),
                    });
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_key_command(move |action, control, shift| {
            if let Some(this) = weak.upgrade() {
                let mut state = this.state.borrow_mut();
                match action.as_str() {
                    "select_all" => state.selected = state.visible.iter().cloned().collect(),
                    "clear_selection" => state.selected.clear(),
                    "toggle_pause" => {
                        let targets: Vec<_> = state
                            .visible
                            .iter()
                            .filter(|id| state.selected.contains(*id))
                            .filter_map(|id| state.tasks.get(id))
                            .collect();
                        let should_pause = targets
                            .iter()
                            .any(|task| task_action_allowed(task, "pause"));
                        let should_start = !should_pause
                            && targets.iter().any(|task| task_start_action(task).is_some());
                        drop(state);
                        if should_pause {
                            this.action("pause");
                        } else if should_start {
                            this.action("start");
                        }
                        return;
                    }
                    "up" | "down" => {
                        let selected = state
                            .anchor
                            .as_ref()
                            .and_then(|id| state.visible.iter().position(|value| value == id));
                        if !state.visible.is_empty() {
                            let target = match (action.as_str(), selected) {
                                ("up", Some(index)) => index.saturating_sub(1),
                                ("down", Some(index)) => (index + 1).min(state.visible.len() - 1),
                                _ => 0,
                            };
                            let id = state.visible[target].clone();
                            state.select(&id, control, shift);
                        }
                    }
                    _ => {}
                }
                drop(state);
                this.render_selection();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                this.action(action.as_str());
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_files_dropped(move |paths| {
            if let Some(this) = weak.upgrade() {
                // Winit delivers a separate event for every file in one Explorer drop.
                // Submit them together on the next tick so Core can report partial success.
                let mut pending = this.dropped_paths.borrow_mut();
                for path in paths.iter().map(String::from) {
                    if !pending.contains(&path) {
                        pending.push(path);
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.window.on_response_json(move |json| {
            if let Some(this) = weak.upgrade() {
                match serde_json::from_str::<CorePipeResponse>(&json) {
                    Ok(response) => this.response(response),
                    Err(error) => this.show_notice("error", format!("响应无法读取：{error}")),
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.window.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                this.close();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }

    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.show_for_launch(None)
    }

    pub fn show_for_launch(&self, launch_id: Option<String>) -> Result<(), slint::PlatformError> {
        self.closing.set(false);
        let visible = self.window.window().is_visible();
        if !visible {
            native_window::set_cloaked(self.window.window(), true);
        }
        self.window.show()?;
        if !visible {
            native_window::set_cloaked(self.window.window(), true);
        }
        #[cfg(windows)]
        {
            let weak = self.window.as_weak();
            let _ = slint::spawn_local(async move {
                let Some(ui) = weak.upgrade() else {
                    report_ui_ready(launch_id, "main_failed");
                    return;
                };
                use slint::winit_030::WinitWindowAccessor;
                if ui.window().winit_window().await.is_err() || !ui.window().is_visible() {
                    native_window::set_cloaked(ui.window(), false);
                    report_ui_ready(launch_id, "main_failed");
                    return;
                }
                if !visible {
                    native_window::set_cloaked(ui.window(), true);
                }
                native_window::apply_theme(&ui, ui.global::<Tokens>().get_dark());
                if !visible {
                    native_window::restore_geometry(ui.window());
                }
                native_window::set_cloaked(ui.window(), false);
                native_window::activate(ui.window());
                // Windows can refuse foreground activation. The receipt confirms
                // the window is visible and can be reached from the taskbar.
                report_ui_ready(launch_id, "main");
            });
        }
        #[cfg(not(windows))]
        {
            if !visible {
                native_window::restore_geometry(self.window.window());
            }
            native_window::set_cloaked(self.window.window(), false);
            report_ui_ready(launch_id, "main");
        }
        Ok(())
    }

    fn close(&self) {
        let was_visible = self.window.window().is_visible();
        native_window::save_geometry(self.window.window());
        if let Some(media) = self.media.borrow().as_ref() {
            media.main_closed();
        }
        let _ = self.window.hide();
        self.closing.set(true);
        if was_visible {
            notify_tray_close_once();
        }
    }
}

fn notify_tray_close_once() {
    let marker = native_window::ui_directory().join("tray-close-hint.marker");
    if marker.is_file() {
        return;
    }
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&marker, b"1\n").is_ok() {
        hls_client::notify_minimized_to_tray();
    }
}

impl Workbench {
    pub fn ready_to_release(&self) -> bool {
        self.closing.get()
            && !self.window.window().is_visible()
            && self.outstanding.borrow().is_empty()
            && self.dropped_paths.borrow().is_empty()
            && !self
                .editor
                .borrow()
                .as_ref()
                .is_some_and(|w| w.window().is_visible())
            && !self
                .details
                .borrow()
                .as_ref()
                .is_some_and(|(_, w)| w.window().is_visible())
            && !self
                .settings
                .borrow()
                .as_ref()
                .is_some_and(|w| w.window.window().is_visible())
            && !self
                .queues
                .borrow()
                .as_ref()
                .is_some_and(|w| w.window.window().is_visible())
            && !self
                .tools
                .borrow()
                .as_ref()
                .is_some_and(|w| w.keeps_ui_alive())
            && !self
                .service_settings
                .borrow()
                .as_ref()
                .is_some_and(|w| w.window.window().is_visible())
            && !self
                .media
                .borrow()
                .as_ref()
                .is_some_and(|media| media.keeps_ui_alive())
            && !self
                .system
                .borrow()
                .as_ref()
                .is_some_and(|system| system.keeps_ui_alive())
            && !self
                .cast
                .borrow()
                .as_ref()
                .is_some_and(|cast| cast.keeps_ui_alive())
    }

    pub fn replace(&self, tasks: Vec<TaskSnapshot>, sequence: u64) {
        self.state.borrow_mut().replace(tasks, sequence);
        self.render();
        if let Some(media) = self.media.borrow().as_ref() {
            media.resynchronize();
        }
        if let Some(system) = self.system.borrow().as_ref() {
            system.resynchronize();
        }
        if let Some(cast) = self.cast.borrow().as_ref() {
            cast.resynchronize();
        }
        if let Some(tools) = self.tools.borrow().as_ref() {
            tools.resynchronize();
        }
    }

    fn merge_snapshot(&self, tasks: Vec<TaskSnapshot>, sequence: u64) {
        self.state.borrow_mut().merge_snapshot(tasks, sequence);
        self.render();
    }

    pub fn apply(self: &Rc<Self>, event: &EventEnvelope) {
        if let Some(media) = self.media.borrow().as_ref() {
            media.event(event);
        }
        if let Some(system) = self.system.borrow().as_ref() {
            system.event(event);
        }
        if let Some(cast) = self.cast.borrow().as_ref() {
            cast.event(event);
        }
        if let Some(tools) = self.tools.borrow().as_ref() {
            tools.event(event);
        }
        let view_revision = self.state.borrow().view_revision;
        let changed = self.state.borrow_mut().apply(event);
        if let Some(crate::task_list::TaskChange { id, notice }) = changed {
            if let (Some(system), Some(notice)) = (self.system.borrow().as_ref(), notice) {
                system.notify(notice);
            }
            if self.state.borrow().view_revision != view_revision {
                self.render();
            } else if let Some(index) = self.positions.borrow().get(&id).copied() {
                if let Some(task) = self.state.borrow().tasks.get(&id) {
                    self.rows
                        .set_row_data(index, row(task, self.state.borrow().selected.contains(&id)));
                }
            }
            if self.state.borrow().selected.contains(&id) {
                self.render_menu();
                self.render_inspector();
            }
        }
        match &event.event {
            CoreEvent::Error { message, .. } => {
                self.show_notice("error", message.clone());
                if let Some((_, details)) = self.details.borrow().as_ref() {
                    if details.window().is_visible() {
                        details.set_footer_tone("error".into());
                        details.set_footer_status(message.clone().into());
                    }
                }
            }
            CoreEvent::Toast { message, .. } => {
                let tone = infer_toast_tone(message);
                self.show_notice(tone, message.clone());
                if let Some((_, details)) = self.details.borrow().as_ref() {
                    if details.window().is_visible() {
                        details.set_footer_tone(tone.into());
                        details.set_footer_status(message.clone().into());
                    }
                }
            }
            CoreEvent::BrowserStatus { connected, .. } => self.set_browser_connected(*connected),
            CoreEvent::PowerActionPending { .. } => {
                self.window.set_power_action_pending(true);
            }
            CoreEvent::SettingsChanged { .. } => {
                self.request(CorePipeRequest::LoadSettings {
                    request_id: self.id(),
                });
            }
            CoreEvent::UiShow { surface } if surface == "main" => {
                let _ = self.show();
            }
            CoreEvent::UiShow { surface } if surface == "new_download" => {
                let _ = self.show();
                self.open_editor("new");
            }
            CoreEvent::UiShow { surface } if surface == "hide" => self.close(),
            _ => {}
        }
    }

    pub fn open_clipboard_offer(self: &Rc<Self>, urls: &[String]) {
        if urls.is_empty() {
            return;
        }
        let _ = self.show();
        let mode_action = if urls.len() > 1 {
            "批量添加"
        } else {
            "new"
        };
        self.open_editor_with_prefill(mode_action, false);
        if let Some(editor) = self.editor.borrow().as_ref() {
            editor.set_url(urls.join("\n").into());
            update_batch_preview(editor);
            editor.set_result_tone("info".into());
            editor.set_result(
                if urls.len() > 1 {
                    format!("已从剪贴板识别 {} 个下载链接", urls.len())
                } else {
                    "已从剪贴板识别下载链接".into()
                }
                .into(),
            );
        }
    }

    pub fn open_editor_from_offer(
        self: &Rc<Self>,
        offer: &hls_client::ResourceOffer,
        filename: &str,
        download_dir: &str,
        queue_id: &str,
    ) {
        let _ = self.show();
        self.open_editor_with_prefill("new", false);
        if let Some(editor) = self.editor.borrow().as_ref() {
            editor.set_url(offer.url.clone().into());
            if !filename.trim().is_empty() {
                editor.set_filename(filename.trim().into());
            }
            if !download_dir.trim().is_empty() {
                editor.set_download_dir(download_dir.trim().into());
            }
            editor.set_kind(kind_label(offer.resource_kind).into());
            let method = offer.request_method.trim().to_ascii_uppercase();
            if matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "HEAD") {
                editor.set_method(method.into());
            }
            self.sync_editor_queues(editor, Some(queue_id));
            let has_advanced = editor.get_has_advanced_values();
            editor.set_advanced_expanded(has_advanced);
            editor.set_result_tone("info".into());
            editor.set_result("已载入当前下载信息，可调整高级参数后添加任务".into());
        }
    }

    fn queue_profiles(&self) -> Vec<hls_client::QueueProfile> {
        match self.settings_response.borrow().as_ref() {
            Some(CorePipeResponse::Settings { queue_profiles, .. }) => {
                crate::queue_editor::ordered_profiles(queue_profiles)
            }
            _ => crate::queue_editor::ordered_profiles(&[]),
        }
    }

    fn sync_editor_queues(&self, editor: &TaskEditorWindow, preferred_queue_id: Option<&str>) {
        let profiles = self.queue_profiles();
        let names = crate::queue_editor::queue_option_names(&profiles);
        let current = preferred_queue_id
            .map(str::to_owned)
            .unwrap_or_else(|| editor.get_queue_id().to_string());
        let index = crate::queue_editor::queue_index_for_id(&profiles, &current);
        let resolved_id = crate::queue_editor::queue_id_at(&profiles, index);
        *self.editor_queues.borrow_mut() = profiles;
        editor.set_queue_names(ModelRc::new(VecModel::from(names)));
        editor.set_queue_index(index);
        editor.set_queue_id(resolved_id.into());
    }

    fn render(&self) {
        let state = self.state.borrow();
        self.rows.set_vec(
            state
                .visible
                .iter()
                .filter_map(|id| {
                    state
                        .tasks
                        .get(id)
                        .map(|task| row(task, state.selected.contains(id)))
                })
                .collect::<Vec<_>>(),
        );
        *self.positions.borrow_mut() = state
            .visible
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), index))
            .collect();
        drop(state);
        self.render_sort();
        self.render_status();
        self.render_menu();
        self.render_inspector();
    }

    fn render_sort(&self) {
        let sort = self.state.borrow().sort.clone();
        let column = sort.split(':').next().unwrap_or("queue");
        let descending = sort.ends_with(":desc");
        self.window.set_sort_column(column.into());
        self.window.set_sort_descending(descending);
    }

    fn sync_speed_limit_pill(&self, enabled: bool, kib: u64) {
        self.window.set_speed_limit_enabled(enabled);
        self.window
            .set_speed_limit_kib(kib.min(i32::MAX as u64) as i32);
        self.window
            .set_speed_limit_label(format_speed_limit_pill(enabled, kib).into());
    }

    fn table_viewport(&self) -> f32 {
        let inspector_space = if self.window.get_inspector_width() > 0.0 {
            self.window.get_inspector_width() + 12.0
        } else {
            0.0
        };
        self.window.get_viewport_width()
            - self.window.get_sidebar_width()
            - self.window.get_content_inset() * 2.0
            - inspector_space
            - 19.0
    }

    fn render_columns(&self) {
        let columns = crate::table_layout::columns(self.table_viewport());
        self.window.set_columns(ModelRc::new(VecModel::from(
            columns
                .into_iter()
                .map(|c| crate::TaskColumn {
                    id: c.id.into(),
                    label: c.label.into(),
                    width: c.width,
                })
                .collect::<Vec<_>>(),
        )));
    }

    fn render_toolbar(&self) {
        self.window.set_toolbar(ModelRc::new(VecModel::from(
            [
                ("new", "新建", "new"),
                ("paste", "粘贴链接", "paste"),
                ("start", "全部开始", "play"),
                ("pause", "全部暂停", "pause"),
                ("clear_completed", "清空已完成", "clear-done"),
            ]
            .into_iter()
            .map(|(action, label, icon)| crate::ToolbarItem {
                action: action.into(),
                label: label.into(),
                icon: icon.into(),
            })
            .collect::<Vec<_>>(),
        )));
    }

    fn render_selection(&self) {
        let state = self.state.borrow();
        for index in 0..self.rows.row_count() {
            if let Some(mut row) = self.rows.row_data(index) {
                let selected = state.selected.contains(row.id.as_str());
                if row.selected != selected {
                    row.selected = selected;
                    self.rows.set_row_data(index, row);
                }
            }
        }
        self.window.set_selected_count(state.selected.len() as i32);
        self.window.set_all_selected(
            !state.visible.is_empty() && state.selected.len() == state.visible.len(),
        );
        drop(state);
        self.render_status();
        self.render_menu();
        self.render_inspector();
    }

    fn render_inspector(&self) {
        if !self.window.get_inspector_open() {
            return;
        }
        let state = self.state.borrow();
        let focused = state
            .anchor
            .as_ref()
            .filter(|id| state.selected.contains(*id))
            .or_else(|| state.visible.iter().find(|id| state.selected.contains(*id)))
            .and_then(|id| state.tasks.get(id));
        let Some(task) = focused else {
            self.window.set_inspector_title("".into());
            self.window.set_inspector_bt("".into());
            self.window.set_inspector_mirrors("".into());
            self.window.set_inspector_checksum("".into());
            self.window.set_inspector_checksum_tone("".into());
            self.window.set_inspector_av("".into());
            self.window.set_inspector_av_tone("".into());
            self.window.set_inspector_diagnostic("".into());
            self.window.set_inspector_logs("".into());
            self.window.set_inspector_segments(ModelRc::default());
            self.window.set_inspector_speed_history(ModelRc::default());
            self.window.set_inspector_can_open_folder(false);
            return;
        };
        let title = if !task.filename.is_empty() {
            task.filename.as_str()
        } else if !task.title.is_empty() {
            task.title.as_str()
        } else {
            task.task_id.as_str()
        };
        let kind = file_tile_kind(&task.filename, &task.url, task.resource_kind);
        self.window.set_inspector_title(title.into());
        self.window.set_inspector_kind(kind.into());
        self.window
            .set_inspector_status(crate::task_status_label(&task.status).into());
        self.window
            .set_inspector_status_code(task.status.clone().into());
        let active_speed = if crate::task_is_active(&task.status) {
            task.speed_bytes_per_sec
        } else {
            0
        };
        self.window.set_inspector_transfer(
            format!(
                "{} / {} · {} /s",
                crate::format_bytes(task.downloaded_bytes),
                task.total_bytes
                    .map(crate::format_bytes)
                    .unwrap_or_else(|| "未知".into()),
                crate::format_bytes(active_speed)
            )
            .into(),
        );
        let connection_header = crate::format_connection_summary(task)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        self.window
            .set_inspector_connections(connection_header.into());
        let telemetry = summarize_task_telemetry(task);
        self.window.set_inspector_bt(telemetry.bt_summary.into());
        self.window
            .set_inspector_mirrors(telemetry.mirror_summary.into());
        self.window
            .set_inspector_checksum(telemetry.checksum_badge.into());
        self.window
            .set_inspector_checksum_tone(telemetry.checksum_tone.into());
        self.window.set_inspector_av(telemetry.av_badge.into());
        self.window.set_inspector_av_tone(telemetry.av_tone.into());
        self.window.set_inspector_diagnostic(
            [
                task.error_message.clone().unwrap_or_default(),
                task.error_hint.clone(),
            ]
            .join(" ")
            .trim()
            .into(),
        );
        let speeds = crate::speed_stats::summarize(active_speed, &task.speed_history);
        self.window.set_inspector_speed_stats(
            if speeds.peak > 0 {
                format!(
                    "峰值 {}/s · 平均 {}/s",
                    crate::format_bytes(speeds.peak),
                    crate::format_bytes(speeds.average)
                )
            } else {
                String::new()
            }
            .into(),
        );
        self.window
            .set_inspector_progress(crate::task_progress(task));
        self.window
            .set_inspector_segments(ModelRc::new(VecModel::from(crate::build_segment_blocks(
                task,
            ))));
        let maximum = task.speed_history.iter().copied().max().unwrap_or(1).max(1) as f32;
        self.window
            .set_inspector_speed_history(ModelRc::new(VecModel::from(
                task.speed_history
                    .iter()
                    .rev()
                    .take(60)
                    .copied()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .map(|value| value as f32 / maximum)
                    .collect::<Vec<_>>(),
            )));
        self.window
            .set_inspector_can_open_folder(task_action_allowed(task, "open_folder"));
        let logs = if task.log_tail.is_empty() {
            String::new()
        } else {
            task.log_tail
                .iter()
                .rev()
                .take(24)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        };
        self.window.set_inspector_logs(logs.into());
    }

    fn render_menu(&self) {
        let state = self.state.borrow();
        let selected: Vec<_> = state
            .selected
            .iter()
            .filter_map(|id| state.tasks.get(id))
            .collect();
        let any_media_selected = selected.iter().any(|task| is_media_capable_task(task));
        let any_refreshable = selected
            .iter()
            .any(|task| task_action_allowed(task, "refresh_browser"));
        let entries = [
            ("开始 / 继续", "start", "play", "transfer"),
            ("暂停", "pause", "pause", "transfer"),
            ("重新下载", "task:retry", "refresh", "transfer"),
            ("取消", "cancel", "close", "transfer"),
            ("播放", "task:play", "play", "media"),
            ("投屏", "task:cast", "cast", "media"),
            ("TVBox 推送", "task:push_tvbox", "tvbox", "media"),
            ("打开文件", "task:open", "file", "file"),
            ("打开所在位置", "task:open_folder", "folder", "file"),
            ("复制文件", "task:copy_file", "paste", "file"),
            ("复制下载链接", "task:copy_url", "link", "file"),
            (
                "刷新下载地址（浏览器辅助）",
                "task:refresh_browser",
                "refresh",
                "file",
            ),
            ("详情与日志", "details", "details", "file"),
            ("移动到队列", "queues", "queue", "queue"),
            ("置顶", "task:queue_top", "up", "queue"),
            ("上移", "task:queue_up", "up", "queue"),
            ("下移", "task:queue_down", "down", "queue"),
            ("置底", "task:queue_bottom", "down", "queue"),
            ("删除任务", "delete", "delete", "danger"),
            ("删除任务和文件", "delete_files", "delete", "danger"),
        ];
        let mut prev_group: Option<&str> = None;
        let mut rows = Vec::with_capacity(entries.len());
        for (label, action, icon, group) in &entries {
            if *group == "media" && !any_media_selected {
                continue;
            }
            if *action == "task:refresh_browser" && !any_refreshable {
                continue;
            }
            let enabled = match *action {
                "details" | "queues" => !selected.is_empty(),
                "start" => selected
                    .iter()
                    .any(|task| task_start_action(task).is_some()),
                "pause" => selected.iter().any(|t| task_action_allowed(t, "pause")),
                "cancel" => selected.iter().any(|t| task_action_allowed(t, "cancel")),
                action => selected.iter().any(|t| {
                    task_action_allowed(t, action.strip_prefix("task:").unwrap_or(action))
                }),
            };
            let divider = prev_group.is_some_and(|prev| prev != *group);
            prev_group = Some(*group);
            rows.push(TaskMenuRow {
                label: (*label).into(),
                action: (*action).into(),
                icon: (*icon).into(),
                enabled,
                divider,
            });
        }
        self.window
            .set_task_menu(ModelRc::new(VecModel::from(rows)));
    }

    fn render_status(&self) {
        let state = self.state.borrow();
        let counts = state.nav_counts();
        let nav = |id: &str, label: &str, icon: &str, group: &str, heading: &str| NavigationRow {
            id: id.into(),
            label: label.into(),
            icon: icon.into(),
            group: group.into(),
            heading: heading.into(),
            count: counts.get(id).copied().unwrap_or_default(),
            active: state.filter == id || (id == "all" && state.filter.is_empty()),
        };
        let mut navigation = vec![nav("all", "全部任务", "folder", "status", "任务状态")];
        for (id, label) in [
            ("running", "进行中"),
            ("queued", "排队中"),
            ("paused", "已暂停"),
            ("completed", "已完成"),
            ("failed", "失败 / 取消"),
        ] {
            navigation.push(nav(id, label, id, "status", ""));
        }
        let mut profiles = match self.settings_response.borrow().as_ref() {
            Some(CorePipeResponse::Settings { queue_profiles, .. }) => queue_profiles.clone(),
            _ => vec![hls_client::QueueProfile::default()],
        };
        profiles.sort_by_key(|profile| profile.priority);
        for (index, profile) in profiles.iter().enumerate() {
            navigation.push(nav(
                &format!("queue:{}", profile.id),
                &profile.name,
                "queue",
                "queues",
                if index == 0 { "下载队列" } else { "" },
            ));
        }
        for (index, (id, label, icon)) in [
            ("media", "媒体", "media"),
            ("program", "程序", "program"),
            ("archive", "压缩包", "archive"),
            ("other", "其他", "file"),
        ]
        .iter()
        .enumerate()
        {
            navigation.push(nav(
                &format!("category:{id}"),
                label,
                icon,
                "categories",
                if index == 0 { "分类" } else { "" },
            ));
        }
        self.window.set_heading(
            navigation
                .iter()
                .find(|entry| entry.active)
                .map_or("全部任务".into(), |entry| entry.label.clone()),
        );
        // Keep row identity (focus and UIA nodes) across idle status ticks.
        if self.navigation.row_count() != navigation.len() {
            self.navigation.set_vec(navigation);
        } else {
            for (index, entry) in navigation.into_iter().enumerate() {
                if self.navigation.row_data(index).as_ref() != Some(&entry) {
                    self.navigation.set_row_data(index, entry);
                }
            }
        }
        self.window.set_task_count(state.tasks.len() as i32);
        self.window.set_visible_count(state.visible.len() as i32);
        let targets = state.toolbar_targets();
        self.window
            .set_can_start(targets.iter().any(|task| task_start_action(task).is_some()));
        self.window.set_can_pause(
            targets
                .iter()
                .any(|task| task_action_allowed(task, "pause")),
        );
        self.window.set_can_delete(
            !state.selected.is_empty()
                && targets
                    .iter()
                    .any(|task| task_action_allowed(task, "delete")),
        );
        let speed: u64 = state
            .tasks
            .values()
            .filter(|task| crate::task_is_active(&task.status))
            .map(|task| task.speed_bytes_per_sec)
            .sum();
        let active_count = state
            .tasks
            .values()
            .filter(|task| crate::task_is_active(&task.status))
            .count() as i32;
        let has_completed = state
            .tasks
            .values()
            .any(|task| matches!(task.status.as_str(), "completed" | "done"));
        self.window.set_active_task_count(active_count);
        self.window.set_can_clear_completed(has_completed);
        self.window
            .set_telemetry_speed(format!("{}/s", crate::format_bytes(speed)).into());
        self.window.set_summary(
            format!(
                "{} 个任务 · {} /s",
                state.visible.len(),
                crate::format_bytes(speed)
            )
            .into(),
        );
        self.window.set_selected_count(state.selected.len() as i32);
        self.window.set_all_selected(
            !state.visible.is_empty() && state.selected.len() == state.visible.len(),
        );
        let online = self.engine_online.get();
        self.window.set_engine_online(online);
        self.window.set_connection(
            if online {
                "下载引擎已连接"
            } else {
                "正在重连下载引擎…"
            }
            .into(),
        );
    }

    fn id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        id
    }

    fn job(&self, job: WorkbenchJob) -> bool {
        self.enqueue(&self.commands.borrow(), job)
    }

    fn enqueue(&self, commands: &mpsc::SyncSender<WorkbenchJob>, job: WorkbenchJob) -> bool {
        let id = match &job {
            WorkbenchJob::Snapshot(id) | WorkbenchJob::Batch(id, _) => Some(*id),
            WorkbenchJob::Request(request) => request.request_id(),
        };
        if commands.try_send(job).is_err() {
            self.show_notice("error", "操作队列已满或连接关闭，请稍后重试");
            false
        } else {
            if let Some(id) = id {
                self.outstanding.borrow_mut().insert(id);
            }
            true
        }
    }

    fn request(&self, request: CorePipeRequest) -> bool {
        self.job(WorkbenchJob::Request(Box::new(request)))
    }
    fn send(&self, command: CoreCommand) {
        self.request(CorePipeRequest::Command {
            request_id: self.id(),
            command,
        });
    }

    fn action(self: &Rc<Self>, action: &str) {
        match action {
            "paste" => self.paste(),
            "toggle_speed_limit" => {
                let current_enabled = self.window.get_speed_limit_enabled();
                let current_kib = self.window.get_speed_limit_kib().max(0) as u64;
                let (next_enabled, effective_kib, values) =
                    toggle_speed_limit_values(current_enabled, current_kib);
                self.sync_speed_limit_pill(next_enabled, effective_kib);
                self.request(CorePipeRequest::StoreSettings {
                    request_id: self.id(),
                    values,
                });
            }
            "task:copy_url" => {
                let urls: Vec<String> = {
                    let state = self.state.borrow();
                    state
                        .visible
                        .iter()
                        .filter(|id| state.selected.contains(*id))
                        .filter_map(|id| state.tasks.get(id))
                        .map(|task| task.url.trim())
                        .filter(|url| !url.is_empty())
                        .map(str::to_owned)
                        .collect()
                };
                if urls.is_empty() {
                    self.show_notice("warning", "所选任务没有可复制的下载链接");
                    return;
                }
                let joined = urls.join("\n");
                match file_dialogs::write_clipboard_text(&joined) {
                    Ok(()) => self.show_notice(
                        "success",
                        if urls.len() == 1 {
                            "已复制下载链接".into()
                        } else {
                            format!("已复制 {} 条下载链接", urls.len())
                        },
                    ),
                    Err(error) => self.show_notice("error", error),
                }
            }
            "task:refresh_browser" => {
                let target = {
                    let state = self.state.borrow();
                    state
                        .anchor
                        .as_ref()
                        .filter(|id| state.selected.contains(*id))
                        .and_then(|id| state.tasks.get(id))
                        .filter(|task| task_action_allowed(task, "refresh_browser"))
                        .or_else(|| {
                            state
                                .visible
                                .iter()
                                .filter(|id| state.selected.contains(*id))
                                .filter_map(|id| state.tasks.get(id))
                                .find(|task| task_action_allowed(task, "refresh_browser"))
                        })
                        .map(|task| (task.task_id.clone(), task.source_page_url.trim().to_owned()))
                };
                let Some((task_id, source_page_url)) = target else {
                    self.show_notice("warning", "请选择失败或已暂停的 HTTP/HLS/DASH 任务");
                    return;
                };
                self.send(CoreCommand::ArmTaskRefresh { task_id });
                if !source_page_url.is_empty() && file_dialogs::is_safe_web_url(&source_page_url) {
                    match file_dialogs::open(&source_page_url) {
                        Ok(()) => self.show_notice(
                            "info",
                            "已在浏览器打开来源页面，请重新点击下载链接以自动刷新地址并续传",
                        ),
                        Err(error) => {
                            self.show_notice(
                                "warning",
                                format!(
                                    "已开启刷新监听，但未能打开来源页面（{error}），可在详情中手动更新地址"
                                ),
                            );
                            self.open_details();
                        }
                    }
                } else {
                    self.open_details();
                    self.show_notice(
                        "info",
                        "已开启浏览器刷新监听；未记录来源页面，可在详情中手动更新下载地址",
                    );
                }
            }
            "task:copy_file" => {
                let paths = {
                    let state = self.state.borrow();
                    selected_output_paths(
                        state
                            .visible
                            .iter()
                            .filter(|id| state.selected.contains(*id))
                            .filter_map(|id| state.tasks.get(id)),
                    )
                };
                let result = paths.and_then(|paths| {
                    hls_client::file_transfer::write_files(&paths).map(|()| {
                        (
                            "success",
                            format!("已复制 {} 个文件，可粘贴到目标文件夹", paths.len()),
                        )
                    })
                });
                match result {
                    Ok((tone, text)) => self.show_notice(tone, text),
                    Err(error) => self.show_notice("warning", error),
                }
            }
            "队列顺序" => {
                self.state.borrow_mut().sort = "queue:asc".into();
                self.state.borrow_mut().rebuild();
                self.render();
                self.send(CoreCommand::SetSetting {
                    key: "task_sort".into(),
                    value: "queue:asc".into(),
                });
            }
            "task:cast" | "task:push_tvbox" => {
                let state = self.state.borrow();
                if let Some(task) = state
                    .visible
                    .iter()
                    .filter(|id| state.selected.contains(*id))
                    .filter_map(|id| state.tasks.get(id))
                    .find(|t| {
                        task_action_allowed(
                            t,
                            if action == "task:cast" {
                                "cast"
                            } else {
                                "push_tvbox"
                            },
                        )
                    })
                {
                    if let Some(cast) = self.cast.borrow().as_ref() {
                        cast.show(
                            if action == "task:cast" {
                                "cast"
                            } else {
                                "tvbox"
                            },
                            Some((&task.task_id, &task.filename)),
                        );
                    }
                }
            }
            "投屏到电视" | "TVBox 推送" => {
                if let Some(cast) = self.cast.borrow().as_ref() {
                    cast.show(
                        if action == "TVBox 推送" {
                            "tvbox"
                        } else {
                            "cast"
                        },
                        None,
                    );
                }
            }
            "delete" | "delete_files" => {
                let files = action == "delete_files";
                let action = if files { "delete_files" } else { "delete" };
                let ids: Vec<_> = {
                    let state = self.state.borrow();
                    state
                        .visible
                        .iter()
                        .filter(|id| {
                            state.selected.contains(*id)
                                && task_action_allowed(&state.tasks[*id], action)
                        })
                        .cloned()
                        .collect()
                };
                if ids.is_empty() {
                    self.show_notice("warning", "请选择可删除的任务");
                    return;
                }
                let message = format!(
                    "将处理 {} 个任务。\n{}",
                    ids.len(),
                    if files {
                        "任务记录、已下载文件和过程文件都会删除。"
                    } else {
                        "只移除任务和过程文件，已完成文件会保留。"
                    }
                );
                if let Some(system) = self.system.borrow().as_ref() {
                    let weak = Rc::downgrade(self);
                    let command_action = action.to_owned();
                    let label = if files {
                        "删除任务和文件"
                    } else {
                        "删除任务"
                    };
                    system.confirm(
                        if files { "delete_files" } else { "delete" },
                        label,
                        message,
                        label,
                        true,
                        Box::new(move || {
                            if let Some(this) = weak.upgrade() {
                                this.job(WorkbenchJob::Batch(
                                    this.id(),
                                    ids.into_iter()
                                        .map(|task_id| CoreCommand::TaskAction {
                                            task_id,
                                            action: command_action.clone(),
                                        })
                                        .collect(),
                                ));
                            }
                        }),
                    );
                }
            }
            action if action.starts_with("task:") => {
                let action = &action[5..];
                let mut ids: Vec<_> = {
                    let state = self.state.borrow();
                    state
                        .visible
                        .iter()
                        .filter(|id| {
                            state.selected.contains(*id)
                                && task_action_allowed(&state.tasks[*id], action)
                        })
                        .cloned()
                        .collect()
                };
                if matches!(action, "queue_top" | "queue_down") {
                    ids.reverse();
                }
                let commands = ids
                    .into_iter()
                    .map(|task_id| match action {
                        "play" => CoreCommand::PlayTask { task_id },
                        "cast" => CoreCommand::CastTask { task_id },
                        _ => CoreCommand::TaskAction {
                            task_id,
                            action: action.into(),
                        },
                    })
                    .collect::<Vec<_>>();
                if !commands.is_empty() {
                    self.job(WorkbenchJob::Batch(self.id(), commands));
                }
            }
            "theme" => {
                let mode = crate::theme::ThemeMode::from_response(
                    self.settings_response.borrow().as_ref(),
                )
                .next();
                self.send(CoreCommand::SetSetting {
                    key: "theme_mode".into(),
                    value: mode.code().into(),
                });
            }
            "start" | "pause" | "cancel" => {
                let state = self.state.borrow();
                let mut commands = Vec::new();
                for task in state.toolbar_targets() {
                    let actions = &task.available_actions;
                    let target = match action {
                        "pause" => "pause",
                        "cancel" => "cancel",
                        _ => match task_start_action(task) {
                            Some(action) => action,
                            None => continue,
                        },
                    };
                    if actions.iter().any(|action| action == target) {
                        commands.push(CoreCommand::TaskAction {
                            task_id: task.task_id.clone(),
                            action: target.into(),
                        });
                    }
                }
                drop(state);
                if !commands.is_empty() {
                    self.job(WorkbenchJob::Batch(self.id(), commands));
                }
            }
            "clear_completed" => {
                self.send(CoreCommand::ClearCompleted);
            }
            "new" | "批量添加" | "导入 cURL" => self.open_editor(action),
            "queues" => self.open_queues(),
            "页面抓取" => self.open_resource_tools("harvest", String::new()),
            "抓站与同步项目" => self.open_resource_tools("crawl", String::new()),
            "ZIP 内容预览" => self.open_resource_tools("zip", String::new()),
            "details" => self.open_details(),
            "浏览器插件" | "通知" | "关于" | "检查更新" => {
                if let Some(system) = self.system.borrow().as_ref() {
                    system.show(match action {
                        "浏览器插件" => "browser",
                        "通知" => "notices",
                        "关于" => "about",
                        _ => "updates",
                    });
                }
            }
            "取消完成动作" => {
                self.window.set_power_action_pending(false);
                self.send(CoreCommand::CancelPowerAction);
            }
            action if action.starts_with("focus_task:") => {
                let target_id = &action[11..];
                let needs_reset = {
                    let state = self.state.borrow();
                    state.tasks.contains_key(target_id) && !state.is_visible(target_id)
                };
                if needs_reset {
                    {
                        let mut state = self.state.borrow_mut();
                        state.filter = "all".into();
                        state.query.clear();
                        state.rebuild();
                    }
                    self.window.set_query("".into());
                    self.render();
                }
                self.state.borrow_mut().select(target_id, false, false);
                self.render_selection();
                let _ = self.show();
                self.open_details();
            }
            "settings" => {
                let request_id = self.id();
                if self.request(CorePipeRequest::LoadSettings { request_id }) {
                    self.settings_open_id.set(Some(request_id));
                } else {
                    self.settings_open_id.set(None);
                }
            }
            "refresh" => {
                self.job(WorkbenchJob::Snapshot(self.id()));
            }
            "import" => {
                match file_dialogs::files(native_window::hwnd(self.window.window()), false) {
                    Ok(paths) if !paths.is_empty() => self.send(CoreCommand::ImportPaths { paths }),
                    Err(error) => self.show_notice("error", error),
                    _ => {}
                }
            }
            "export" => {
                match file_dialogs::files(native_window::hwnd(self.window.window()), true) {
                    Ok(paths) if !paths.is_empty() => {
                        let id = self.id();
                        let path = PathBuf::from(&paths[0]);
                        let format = if path.extension().is_some_and(|ext| ext == "txt") {
                            "txt"
                        } else {
                            "json"
                        };
                        let task_ids = self.state.borrow().selected.iter().cloned().collect();
                        self.exports.borrow_mut().insert(id, path);
                        self.request(CorePipeRequest::Command {
                            request_id: id,
                            command: CoreCommand::ExportTasks {
                                task_ids,
                                format: format.into(),
                            },
                        });
                    }
                    Err(error) => self.show_notice("error", error),
                    _ => {}
                }
            }
            _ => self.show_notice("warning", format!("功能入口尚未完成迁移：{action}")),
        }
    }

    fn paste(self: &Rc<Self>) {
        use hls_client::file_transfer::{read_clipboard, ClipboardContent};
        match read_clipboard() {
            Ok(ClipboardContent::Files(paths)) if !paths.is_empty() => {
                self.send(CoreCommand::ImportPaths { paths })
            }
            Ok(ClipboardContent::Text(text)) if !text.trim().is_empty() => {
                let action = if matches!(text.split_whitespace().next(), Some("curl" | "curl.exe"))
                {
                    "导入 cURL"
                } else if text.lines().filter(|line| !line.trim().is_empty()).count() > 1 {
                    "批量添加"
                } else {
                    "new"
                };
                self.open_editor_with_prefill(action, false);
                if let Some(editor) = self.editor.borrow().as_ref() {
                    editor.set_url(text.trim().into());
                    update_batch_preview(editor);
                }
            }
            Ok(_) => self.show_notice("info", "剪贴板中没有链接或可导入的文件"),
            Err(error) => self.show_notice("error", error),
        }
    }

    fn response(self: &Rc<Self>, response: CorePipeResponse) {
        if let Some(id) = response.request_id() {
            self.outstanding.borrow_mut().remove(&id);
        }
        if let Some(media) = self.media.borrow().as_ref() {
            media.response(&response);
        }
        if let Some(system) = self.system.borrow().as_ref() {
            system.response(&response);
        }
        if let Some(service) = self.service_settings.borrow().as_ref() {
            service.response(&response);
        }
        if let Some(cast) = self.cast.borrow().as_ref() {
            cast.response(&response);
        }
        if let Some(tools) = self.tools.borrow().as_ref() {
            tools.response(&response);
        }
        match response {
            CorePipeResponse::Events { request_id, events } => {
                self.set_engine_online(true);
                for event in &events {
                    self.apply(event);
                }
                if request_id == self.editor_id.get() {
                    if let Some(editor) = self.editor.borrow().as_ref() {
                        editor.set_busy(false);
                        for event in &events {
                            match &event.event {
                                CoreEvent::ProbeResult {
                                    resource_kind,
                                    label,
                                    variants,
                                    ..
                                } => {
                                    editor.set_result_tone("info".into());
                                    editor.set_result(label.clone().into());
                                    editor.set_kind(kind_label(*resource_kind).into());
                                    *self.editor_variants.borrow_mut() = variants.clone();
                                    editor.set_variant_index(0);
                                    editor.set_variants(ModelRc::new(VecModel::from(
                                        variants
                                            .iter()
                                            .map(|variant| {
                                                format!(
                                                    "{} · {}p · {} bit/s",
                                                    variant.label,
                                                    variant.height,
                                                    variant.bandwidth
                                                )
                                                .into()
                                            })
                                            .collect::<Vec<_>>(),
                                    )));
                                }
                                CoreEvent::TaskCreated { .. } => {
                                    editor.set_result_tone("success".into());
                                    editor.set_result("任务已交给下载引擎".into())
                                }
                                CoreEvent::Error { message, .. } => {
                                    editor.set_result_tone("error".into());
                                    editor.set_result(message.clone().into());
                                    if editor.window().is_visible() {
                                        editor.set_advanced_expanded(true);
                                    }
                                }
                                _ => {}
                            }
                        }
                        if events
                            .iter()
                            .any(|event| matches!(event.event, CoreEvent::TaskCreated { .. }))
                            && !events.iter().any(|event| {
                                matches!(
                                    event.event,
                                    CoreEvent::Error { .. } | CoreEvent::DuplicateOffered { .. }
                                )
                            })
                        {
                            editor.set_cookie("".into());
                            editor.set_headers("".into());
                            if editor.get_mode().as_str() == "curl" {
                                editor.set_url("".into());
                            }
                            let _ = editor.hide();
                        }
                    }
                }
                if let Some(path) = self.exports.borrow_mut().remove(&request_id) {
                    if let Some(data) = events.iter().find_map(|event| {
                        if let CoreEvent::TaskExport { data, .. } = &event.event {
                            Some(data)
                        } else {
                            None
                        }
                    }) {
                        match file_dialogs::write_export(&path, data) {
                            Ok(()) => {
                                self.show_notice("success", format!("已导出：{}", path.display()))
                            }
                            Err(error) => self.show_notice("error", format!("导出失败：{error}")),
                        }
                    }
                }
                for event in events {
                    if let CoreEvent::TaskLog { task_id, lines } = event.event {
                        if let Some((id, window)) = self.details.borrow().as_ref() {
                            if *id == task_id {
                                self.details_full_log.set(true);
                                window.set_logs(lines.join("\n").into());
                            }
                        }
                    }
                }
            }
            response @ CorePipeResponse::Settings { request_id, .. } => {
                self.set_engine_online(true);
                if let CorePipeResponse::Settings {
                    task_sort,
                    reduce_motion,
                    speed_limit_enabled,
                    speed_limit_kib,
                    ..
                } = &response
                {
                    self.apply_appearance(
                        crate::theme::ThemeMode::from_response(Some(&response)).is_dark(),
                        *reduce_motion || hls_client::os_reduce_motion(),
                    );
                    self.sync_speed_limit_pill(*speed_limit_enabled, *speed_limit_kib);
                    let changed = self.state.borrow().sort != *task_sort;
                    if changed {
                        self.state.borrow_mut().sort = task_sort.clone();
                        self.state.borrow_mut().rebuild();
                        self.render();
                    }
                }
                let completed_queue_save = {
                    let mut pending = self.queue_save.borrow_mut();
                    if pending
                        .as_ref()
                        .is_some_and(|save| save.request_id == request_id)
                    {
                        pending.take()
                    } else {
                        None
                    }
                };
                if let Some(save) = completed_queue_save {
                    if let Some(editor) = self.queues.borrow().as_ref() {
                        editor.window.set_busy(false);
                        editor.window.set_status_tone("success".into());
                        editor.window.set_status("队列已保存".into());
                    }
                    if let Some((task_ids, queue_id)) = save.assignment {
                        self.send(CoreCommand::AssignQueue { task_ids, queue_id });
                    }
                }
                let settings_saved = self.settings_save_id.get() == Some(request_id);
                if settings_saved {
                    self.settings_save_id.set(None);
                    if let Some(editor) = self.settings.borrow_mut().as_mut() {
                        editor.receive_settings(&response, true);
                        editor.window.set_busy(false);
                        editor.window.set_status_tone("success".into());
                        editor.window.set_status("设置已保存".into());
                    }
                } else if let Some(editor) = self.settings.borrow_mut().as_mut() {
                    editor.receive_settings(&response, false);
                }
                *self.settings_response.borrow_mut() = Some(response.clone());
                if let Some(editor) = self.editor.borrow().as_ref() {
                    self.sync_editor_queues(editor, None);
                    if editor.get_download_dir().trim().is_empty() {
                        if let CorePipeResponse::Settings { download_dir, .. } = &response {
                            if !download_dir.trim().is_empty() {
                                editor.set_download_dir(download_dir.clone().into());
                            }
                        }
                    }
                }
                if let Some(tools) = self.tools.borrow().as_ref() {
                    tools.set_queues(&self.queue_profiles());
                }
                self.render_status();
                if self.settings_open_id.get() == Some(request_id) {
                    self.settings_open_id.set(None);
                    self.open_settings(&response);
                }
                if self.queues_open_id.get() == Some(request_id) {
                    self.queues_open_id.set(None);
                    self.open_queues();
                }
            }
            CorePipeResponse::Snapshot {
                tasks,
                latest_sequence,
                browser_connected,
                ..
            } => {
                self.set_engine_online(true);
                self.set_browser_connected(browser_connected);
                self.merge_snapshot(tasks, latest_sequence);
            }
            CorePipeResponse::Error {
                request_id,
                code,
                message,
            } => {
                if code == "ui_connection" {
                    self.set_engine_online(false);
                }
                self.show_notice("error", message.clone());
                if request_id == Some(self.editor_id.get()) {
                    if let Some(editor) = self.editor.borrow().as_ref() {
                        editor.set_busy(false);
                        editor.set_result_tone("error".into());
                        editor.set_result(message.clone().into());
                        if editor.window().is_visible() {
                            editor.set_advanced_expanded(true);
                        }
                    }
                }
                if request_id.is_some() && request_id == self.settings_open_id.get() {
                    self.settings_open_id.set(None);
                }
                if request_id.is_some() && request_id == self.queues_open_id.get() {
                    self.queues_open_id.set(None);
                }
                if request_id.is_some() && request_id == self.settings_save_id.get() {
                    self.settings_save_id.set(None);
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.window.set_busy(false);
                        settings.window.set_status_tone("error".into());
                        settings.window.set_status(message.into());
                    }
                }
                if self
                    .queue_save
                    .borrow()
                    .as_ref()
                    .is_some_and(|save| Some(save.request_id) == request_id)
                {
                    self.queue_save.borrow_mut().take();
                    if let Some(editor) = self.queues.borrow().as_ref() {
                        editor.window.set_busy(false);
                        editor.window.set_status_tone("error".into());
                        editor.window.set_status(self.window.get_notice());
                    }
                }
            }
            _ => {}
        }
    }

    fn open_editor(self: &Rc<Self>, action: &str) {
        self.open_editor_with_prefill(action, action == "new");
    }

    fn open_editor_with_prefill(self: &Rc<Self>, action: &str, allow_passive_clipboard: bool) {
        if let Some(previous) = self.editor.borrow().as_ref() {
            let _ = previous.hide();
        }
        let Ok(editor) = TaskEditorWindow::new() else {
            return;
        };
        native_window::inherit_appearance(&editor, &self.window);
        editor.set_mode(
            match action {
                "批量添加" => "batch",
                "导入 cURL" => "curl",
                _ => "new",
            }
            .into(),
        );
        if let Some(CorePipeResponse::Settings { download_dir, .. }) =
            self.settings_response.borrow().as_ref()
        {
            editor.set_download_dir(download_dir.clone().into());
        }
        editor.set_legal_accepted(true);
        #[cfg(not(test))]
        if allow_passive_clipboard {
            if let Ok(hls_client::file_transfer::ClipboardContent::Text(text)) =
                hls_client::file_transfer::read_clipboard()
            {
                apply_passive_clipboard_prefill(&editor, &text);
            }
        }
        #[cfg(test)]
        let _ = allow_passive_clipboard;
        self.sync_editor_queues(&editor, None);
        update_batch_preview(&editor);
        let has_advanced = editor.get_has_advanced_values();
        editor.set_advanced_expanded(has_advanced);
        self.editor_id.set(self.id());
        self.editor_variants.borrow_mut().clear();
        let weak = Rc::downgrade(self);
        editor.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                this.editor_action(action.as_str());
            }
        });
        let editor_weak_url = editor.as_weak();
        editor.on_url_edited(move |_| {
            if let Some(editor) = editor_weak_url.upgrade() {
                update_batch_preview(&editor);
            }
        });
        let weak = Rc::downgrade(self);
        editor.on_queue_selected(move |index| {
            if let Some(this) = weak.upgrade() {
                let queue_id =
                    crate::queue_editor::queue_id_at(&this.editor_queues.borrow(), index);
                if let Some(editor) = this.editor.borrow().as_ref() {
                    editor.set_queue_id(queue_id.into());
                }
            }
        });
        let editor_weak = editor.as_weak();
        editor.window().on_close_requested(move || {
            if let Some(editor) = editor_weak.upgrade() {
                editor.set_cookie("".into());
                editor.set_headers("".into());
                if editor.get_mode().as_str() == "curl" {
                    editor.set_url("".into());
                }
            }
            slint::CloseRequestResponse::HideWindow
        });
        let _ = editor.show();
        native_window::prepare_dialog(
            &editor,
            true,
            true,
            native_window::hwnd(self.window.window()),
        );
        *self.editor.borrow_mut() = Some(editor);
    }

    fn editor_action(&self, action: &str) {
        if action == "folder" || action == "body_file" {
            let owner = {
                let editor = self.editor.borrow();
                let Some(editor) = editor.as_ref() else {
                    return;
                };
                native_window::hwnd(editor.window())
            };
            if action == "folder" {
                let picked = file_dialogs::folder(owner);
                let editor = self.editor.borrow();
                let Some(editor) = editor.as_ref() else {
                    return;
                };
                match picked {
                    Ok(Some(path)) => editor.set_download_dir(path.into()),
                    Err(error) => {
                        editor.set_result_tone("error".into());
                        editor.set_result(error.into());
                    }
                    _ => {}
                }
            } else {
                let picked = file_dialogs::files(owner, false);
                let editor = self.editor.borrow();
                let Some(editor) = editor.as_ref() else {
                    return;
                };
                match picked {
                    Ok(paths) => {
                        if let Some(path) = paths.first() {
                            editor.set_body_path(path.clone().into());
                        }
                    }
                    Err(error) => {
                        editor.set_result_tone("error".into());
                        editor.set_result(error.into());
                    }
                }
            }
            return;
        }
        let editor = self.editor.borrow();
        let Some(editor) = editor.as_ref() else {
            return;
        };
        if editor.get_busy() && matches!(action, "probe" | "create") {
            return;
        }
        match action {
            "close" => {
                let _ = editor.hide();
                editor.set_cookie("".into());
                editor.set_headers("".into());
                if editor.get_mode().as_str() == "curl" {
                    editor.set_url("".into());
                }
            }
            "probe" | "create" => {
                let result = editor_spec(editor, &self.editor_variants.borrow());
                let spec = match result {
                    Ok(spec) => spec,
                    Err(error) => {
                        if error.advanced {
                            editor.set_advanced_expanded(true);
                        }
                        editor.set_result_tone("warning".into());
                        editor.set_result(error.message.into());
                        return;
                    }
                };
                let request_id = self.editor_id.get();
                let submitted = if action == "probe" {
                    let probe_url = if editor.get_mode().as_str() == "curl" {
                        let Some(extracted) = extract_curl_probe_url(&spec.url) else {
                            editor.set_result_tone("warning".into());
                            editor.set_result("未能从 cURL 命令中识别出有效链接".into());
                            return;
                        };
                        extracted
                    } else {
                        spec.url.clone()
                    };
                    let probe_spec = TaskSpec {
                        url: probe_url.clone(),
                        ..spec
                    };
                    let request = CorePipeRequest::Command {
                        request_id,
                        command: CoreCommand::ProbeUrl {
                            url: probe_url,
                            spec: Some(probe_spec),
                        },
                    };
                    self.enqueue(
                        &self.editor_commands,
                        WorkbenchJob::Request(Box::new(request)),
                    )
                } else {
                    let commands = match editor.get_mode().as_str() {
                        "curl" => vec![CoreCommand::ImportCurl {
                            command: editor.get_url().to_string(),
                            options: spec,
                        }],
                        "batch" => {
                            let expansion = crate::wildcard::expand_batch(
                                &editor.get_url(),
                                crate::wildcard::MAX_BATCH_URLS,
                            );
                            if expansion.urls.is_empty() {
                                editor.set_result_tone("warning".into());
                                editor.set_result("请输入下载地址".into());
                                return;
                            }
                            if expansion.truncated {
                                let (warning, _, _) = expansion.preview_status();
                                editor.set_result_tone("warning".into());
                                editor.set_result(warning.into());
                                return;
                            }
                            expansion
                                .urls
                                .into_iter()
                                .map(|url| CoreCommand::CreateTask {
                                    spec: TaskSpec {
                                        url,
                                        ..spec.clone()
                                    },
                                })
                                .collect()
                        }
                        _ => vec![CoreCommand::CreateTask { spec }],
                    };
                    let accept_terms = CorePipeRequest::Command {
                        request_id: self.id(),
                        command: CoreCommand::SetSetting {
                            key: "legal_terms_accepted".into(),
                            value: true.into(),
                        },
                    };
                    self.enqueue(
                        &self.editor_commands,
                        WorkbenchJob::Request(Box::new(accept_terms)),
                    ) && self.enqueue(
                        &self.editor_commands,
                        WorkbenchJob::Batch(request_id, commands),
                    )
                };
                editor.set_busy(submitted);
                if !submitted {
                    editor.set_result_tone("error".into());
                    editor.set_result(self.window.get_notice());
                }
            }
            _ => {}
        }
    }

    fn open_details(self: &Rc<Self>) {
        let Some(id) = self
            .state
            .borrow()
            .visible
            .iter()
            .find(|id| self.state.borrow().selected.contains(*id))
            .cloned()
        else {
            self.show_notice("warning", "请选择任务");
            return;
        };
        if let Some((_, previous)) = self.details.borrow().as_ref() {
            let _ = previous.hide();
        }
        let Ok(window) = DetailsWindow::new() else {
            return;
        };
        native_window::inherit_appearance(&window, &self.window);
        let weak = Rc::downgrade(self);
        window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                let details = this.details.borrow();
                let Some((id, window)) = details.as_ref() else {
                    return;
                };
                match action.as_str() {
                    "close" => {
                        let _ = window.hide();
                        window.set_cookie("".into());
                    }
                    "refresh_url" => {
                        let url: String = window.get_url().into();
                        let cookie: String = window.get_cookie().into();
                        window.set_cookie("".into());
                        window.set_footer_tone("info".into());
                        window.set_footer_status("已提交新请求信息并尝试续传".into());
                        this.send(CoreCommand::RefreshTaskRequest {
                            task_id: id.clone(),
                            url,
                            cookie,
                            auto_resume: true,
                        });
                    }
                    "speed" => {
                        let limit = window.get_speed_limit();
                        window.set_footer_tone("success".into());
                        window.set_footer_status(
                            if limit > 0 {
                                format!("已设置任务限速 {limit} KiB/s")
                            } else {
                                "已取消任务限速".into()
                            }
                            .into(),
                        );
                        this.send(CoreCommand::TaskAction {
                            task_id: id.clone(),
                            action: format!("speed:{limit}"),
                        });
                    }
                    "logs" => {
                        window.set_footer_tone("info".into());
                        window.set_footer_status("正在刷新任务日志…".into());
                        this.send(CoreCommand::GetTaskLog {
                            task_id: id.clone(),
                        });
                    }
                    "folder" => this.send(CoreCommand::OpenCompleted {
                        task_id: id.clone(),
                        folder: true,
                    }),
                    "profile" => {
                        window.set_footer_tone("success".into());
                        window.set_footer_status("已保存当前站点下载规则".into());
                        this.send(CoreCommand::SaveSiteProfile {
                            task_id: id.clone(),
                        });
                    }
                    "torrent" if window.get_can_select_torrent_files() => {
                        this.open_resource_tools("torrent", id.clone())
                    }
                    "open_source_page" => {
                        let source_url = window.get_source_page_url().trim().to_owned();
                        if !source_url.is_empty() {
                            if let Err(error) = file_dialogs::open(&source_url) {
                                window.set_footer_tone("error".into());
                                window.set_footer_status(error.clone().into());
                                this.show_notice("error", error);
                            }
                        }
                    }
                    "copy_source_page" => {
                        let source_url = window.get_source_page_url().trim().to_owned();
                        if !source_url.is_empty() {
                            match file_dialogs::write_clipboard_text(&source_url) {
                                Ok(()) => {
                                    window.set_footer_tone("success".into());
                                    window.set_footer_status("已复制来源页面链接".into());
                                    this.show_notice("success", "已复制来源页面链接");
                                }
                                Err(error) => {
                                    window.set_footer_tone("error".into());
                                    window.set_footer_status(error.clone().into());
                                    this.show_notice("error", error);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
        let details_weak = window.as_weak();
        window.window().on_close_requested(move || {
            if let Some(window) = details_weak.upgrade() {
                window.set_cookie("".into());
            }
            slint::CloseRequestResponse::HideWindow
        });
        self.details_full_log.set(false);
        *self.details.borrow_mut() = Some((id.clone(), window));
        if let Some((_, window)) = self.details.borrow().as_ref() {
            if let Some(task) = self.state.borrow().tasks.get(&id) {
                window.set_url(task.url.clone().into());
                window.set_speed_limit(task.speed_limit_kib.min(i32::MAX as u32) as i32);
                window.set_can_open_folder(task_action_allowed(task, "open_folder"));
            }
            let _ = window.show();
            native_window::prepare_dialog(
                window,
                true,
                true,
                native_window::hwnd(self.window.window()),
            );
        }
        self.render_details();
        self.send(CoreCommand::GetTaskLog { task_id: id });
    }

    fn render_details(&self) {
        let details = self.details.borrow();
        let Some((id, window)) = details.as_ref() else {
            return;
        };
        if !window.window().is_visible() {
            return;
        }
        let state = self.state.borrow();
        let Some(task) = state.tasks.get(id) else {
            let _ = window.hide();
            return;
        };
        let active_speed = if crate::task_is_active(&task.status) {
            task.speed_bytes_per_sec
        } else {
            0
        };
        window.set_filename(task.filename.clone().into());
        window.set_can_select_torrent_files(task.resource_kind == ResourceKind::Torrent);
        window.set_can_open_folder(task_action_allowed(task, "open_folder"));
        window.set_status(crate::task_status_label(&task.status).into());
        window.set_transfer(
            format!(
                "{} / {} · {} /s",
                crate::format_bytes(task.downloaded_bytes),
                task.total_bytes
                    .map(crate::format_bytes)
                    .unwrap_or_else(|| "未知".into()),
                crate::format_bytes(active_speed)
            )
            .into(),
        );
        window.set_progress(crate::task_progress(task));
        window.set_segments(ModelRc::new(VecModel::from(crate::build_segment_blocks(
            task,
        ))));
        window.set_connections(crate::format_connection_summary(task).into());
        let telemetry = summarize_task_telemetry(task);
        window.set_bt_telemetry(telemetry.bt_summary.into());
        window.set_mirror_telemetry(telemetry.mirror_summary.into());
        window.set_checksum_badge(telemetry.checksum_badge.into());
        window.set_checksum_tone(telemetry.checksum_tone.into());
        window.set_av_badge(telemetry.av_badge.into());
        window.set_av_tone(telemetry.av_tone.into());
        window.set_av_detail(telemetry.av_detail.into());
        window.set_source_page_url(task.source_page_url.trim().into());
        window.set_diagnostic(
            [
                task.error_message.clone().unwrap_or_default(),
                task.error_hint.clone(),
            ]
            .join(" ")
            .trim()
            .into(),
        );
        let speeds = crate::speed_stats::summarize(active_speed, &task.speed_history);
        window.set_current_speed(format!("{} /s", crate::format_bytes(speeds.current)).into());
        window.set_peak_speed(format!("{} /s", crate::format_bytes(speeds.peak)).into());
        window.set_average_speed(format!("{} /s", crate::format_bytes(speeds.average)).into());
        let maximum = task.speed_history.iter().copied().max().unwrap_or(1).max(1) as f32;
        window.set_speed_history(ModelRc::new(VecModel::from(
            task.speed_history
                .iter()
                .rev()
                .take(120)
                .copied()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|value| value as f32 / maximum)
                .collect::<Vec<_>>(),
        )));
        if !self.details_full_log.get() && !task.log_tail.is_empty() {
            let next = task.log_tail.join("\n");
            if window.get_logs().as_str() != next {
                window.set_logs(next.into());
            }
        }
    }

    fn open_resource_tools(self: &Rc<Self>, mode: &str, task_id: String) {
        let directory = match self.settings_response.borrow().as_ref() {
            Some(CorePipeResponse::Settings { download_dir, .. }) => download_dir.clone(),
            _ => String::new(),
        };
        let request_owner = Rc::downgrade(self);
        let batch_owner = Rc::downgrade(self);
        let id_owner = Rc::downgrade(self);
        let request = Rc::new(move |request| {
            request_owner.upgrade().is_some_and(|this| {
                this.enqueue(
                    &this.resource_commands,
                    WorkbenchJob::Request(Box::new(request)),
                )
            })
        });
        let batch = Rc::new(move |commands| {
            batch_owner.upgrade().is_some_and(|this| {
                this.enqueue(
                    &this.resource_commands,
                    WorkbenchJob::Batch(this.id(), commands),
                )
            })
        });
        let id = Rc::new(move || id_owner.upgrade().map_or(0, |this| this.id()));
        match ResourceTools::new(mode, task_id, &directory, &self.window, request, batch, id) {
            Ok(tools) => {
                tools.set_queues(&self.queue_profiles());
                if let Some(previous) = self.tools.borrow_mut().replace(tools.clone()) {
                    let _ = previous.window.hide();
                }
                let _ = tools.window.show();
                native_window::prepare_dialog(
                    &tools.window,
                    true,
                    true,
                    native_window::hwnd(self.window.window()),
                );
            }
            Err(error) => self.show_notice("error", error.to_string()),
        }
    }

    fn open_queues(self: &Rc<Self>) {
        let profiles = match self.settings_response.borrow().as_ref() {
            Some(CorePipeResponse::Settings { queue_profiles, .. }) => queue_profiles.clone(),
            _ => {
                let request_id = self.id();
                if self.request(CorePipeRequest::LoadSettings { request_id }) {
                    self.queues_open_id.set(Some(request_id));
                } else {
                    self.queues_open_id.set(None);
                }
                return;
            }
        };
        self.queues_open_id.set(None);
        if let Some(previous) = self.queues.borrow().as_ref() {
            let _ = previous.window.hide();
        }
        let Ok(editor) = QueueEditor::new(
            profiles,
            &self.window,
            !self.state.borrow().selected.is_empty(),
        ) else {
            return;
        };
        let weak = Rc::downgrade(self);
        editor.window.on_select_profile(move |index| {
            if let Some(this) = weak.upgrade() {
                if let Some(editor) = this.queues.borrow_mut().as_mut() {
                    if let Err(error) = editor.select(index.max(0) as usize) {
                        editor.window.set_status_tone("warning".into());
                        editor.window.set_status(error.into());
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_toggle_day(move |day| {
            if let Some(this) = weak.upgrade() {
                if let Some(editor) = this.queues.borrow_mut().as_mut() {
                    editor.toggle_day(day);
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_action(move |action| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let mut queues = this.queues.borrow_mut();
            let Some(editor) = queues.as_mut() else {
                return;
            };
            if editor.window.get_busy() {
                return;
            }
            if matches!(action.as_str(), "save" | "assign") {
                match editor.values() {
                    Ok((profiles, queue_id)) => {
                        let Ok(encoded_profiles) = serde_json::to_value(profiles) else {
                            editor.window.set_status_tone("error".into());
                            editor.window.set_status("无法序列化队列配置".into());
                            return;
                        };
                        let id = this.id();
                        let assignment = (action == "assign").then(|| {
                            (
                                this.state.borrow().selected.iter().cloned().collect(),
                                queue_id,
                            )
                        });
                        let values = std::collections::BTreeMap::from([(
                            "queue_profiles".to_owned(),
                            encoded_profiles,
                        )]);
                        if this.request(CorePipeRequest::StoreSettings {
                            request_id: id,
                            values,
                        }) {
                            *this.queue_save.borrow_mut() = Some(QueueSave {
                                request_id: id,
                                assignment,
                            });
                            editor.window.set_busy(true);
                        } else {
                            editor.window.set_status_tone("error".into());
                            editor.window.set_status("命令队列已满，请稍后重试".into());
                        }
                    }
                    Err(error) => {
                        editor.window.set_status_tone("warning".into());
                        editor.window.set_status(error.into())
                    }
                }
            } else if let Err(error) = editor.edit(&action) {
                editor.window.set_status_tone("warning".into());
                editor.window.set_status(error.into());
            }
        });
        let _ = editor.window.show();
        native_window::prepare_dialog(
            &editor.window,
            true,
            true,
            native_window::hwnd(self.window.window()),
        );
        *self.queues.borrow_mut() = Some(editor);
    }

    fn open_settings(self: &Rc<Self>, response: &CorePipeResponse) {
        self.settings_open_id.set(None);
        if let Some(previous) = self.settings.borrow().as_ref() {
            let _ = previous.window.hide();
        }
        let Ok(editor) = SettingsEditor::new(response, &self.window) else {
            return;
        };
        let weak = Rc::downgrade(self);
        editor.window.on_group_selected(move |group| {
            if let Some(this) = weak.upgrade() {
                if let Some(editor) = this.settings.borrow().as_ref() {
                    editor.render(&group);
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_edited(move |key, value| {
            if let Some(this) = weak.upgrade() {
                if let Some(editor) = this.settings.borrow_mut().as_mut() {
                    if let Err(error) = editor.edit(&key, &value) {
                        editor.window.set_status_tone("warning".into());
                        editor.window.set_status(error.into());
                    } else {
                        sync_settings_edit_status(editor);
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_site_rule_edited(move |index, key, value| {
            if let Some(this) = weak.upgrade() {
                if let Some(editor) = this.settings.borrow_mut().as_mut() {
                    editor.edit_rule(index, &key, &value);
                    sync_settings_edit_status(editor);
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_site_rule_action(move |index, action| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            if action.as_str() == "folder" {
                let owner = {
                    let settings = this.settings.borrow();
                    let Some(editor) = settings.as_ref() else {
                        return;
                    };
                    native_window::hwnd(editor.window.window())
                };
                // Drop RefCell borrow before Win32 modal folder loop pumps timers/IPC.
                let picked = file_dialogs::folder(owner);
                let mut settings = this.settings.borrow_mut();
                let Some(editor) = settings.as_mut() else {
                    return;
                };
                match picked {
                    Ok(Some(path)) => {
                        editor.edit_rule(index, "directory", &path);
                        sync_settings_edit_status(editor);
                    }
                    Err(error) => {
                        editor.window.set_status_tone("warning".into());
                        editor.window.set_status(error.into());
                    }
                    _ => {}
                }
                return;
            }
            let mut settings = this.settings.borrow_mut();
            let Some(editor) = settings.as_mut() else {
                return;
            };
            let is_credentials = action.as_str() == "credentials";
            let result = match action.as_str() {
                "credentials" => editor
                    .credential_host(index)
                    .map(|host| this.open_service_settings("credentials", &host)),
                action => editor.rule_action(index, action),
            };
            if let Err(error) = result {
                editor.window.set_status_tone("warning".into());
                editor.window.set_status(error.into());
            } else if !is_credentials {
                sync_settings_edit_status(editor);
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_action(move |action| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let mut settings = this.settings.borrow_mut();
            let Some(editor) = settings.as_mut() else {
                return;
            };
            match action.as_str() {
                "close" => {
                    editor.discard_unsaved_secrets();
                    let _ = editor.window.hide();
                }
                "save" => {
                    if editor.window.get_busy() {
                        return;
                    }
                    if this
                        .service_settings
                        .borrow()
                        .as_ref()
                        .is_some_and(|service| service.window.get_busy())
                    {
                        editor.window.set_status_tone("warning".into());
                        editor
                            .window
                            .set_status("请等待服务配置或登录信息保存完成".into());
                        return;
                    }
                    match editor.storage_changes() {
                        Ok(values) => {
                            let id = this.id();
                            if this.request(CorePipeRequest::StoreSettings {
                                request_id: id,
                                values,
                            }) {
                                this.settings_save_id.set(Some(id));
                                editor.window.set_busy(true);
                            } else {
                                editor.window.set_status_tone("error".into());
                                editor.window.set_status("命令队列已满，请稍后重试".into());
                            }
                        }
                        Err(error) => {
                            editor.window.set_status_tone("warning".into());
                            editor.window.set_status(error.into())
                        }
                    }
                }
                "preview_sound" => hls_client::completion_sound(),
                "update" => {
                    if let Some(system) = this.system.borrow().as_ref() {
                        system.show("updates");
                    }
                }
                "extension" => {
                    if let Some(system) = this.system.borrow().as_ref() {
                        system.show("browser");
                    }
                }
                "subtitle" => this.open_service_settings("subtitle", ""),
                "default_cookie" => this.open_service_settings("credentials", ""),
                value if value.starts_with("folder:") => {
                    let key = value.strip_prefix("folder:").unwrap_or("");
                    let owner = native_window::hwnd(editor.window.window());
                    // Drop RefCell borrow before Win32 modal folder loop pumps timers/IPC.
                    drop(settings);
                    let picked = file_dialogs::folder(owner);
                    let mut settings = this.settings.borrow_mut();
                    let Some(editor) = settings.as_mut() else {
                        return;
                    };
                    match picked {
                        Ok(Some(path)) => {
                            if let Err(error) = editor.edit(key, &path) {
                                editor.window.set_status_tone("warning".into());
                                editor.window.set_status(error.into());
                            } else {
                                sync_settings_edit_status(editor);
                            }
                            editor.render(&editor.window.get_selected_group());
                        }
                        Err(error) => {
                            editor.window.set_status_tone("warning".into());
                            editor.window.set_status(error.into());
                        }
                        _ => {}
                    }
                }
                _ => {
                    editor.window.set_status_tone("info".into());
                    editor
                        .window
                        .set_status("该设置页面的专用编辑器尚在迁移中".into())
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                if let Some(editor) = this.settings.borrow_mut().as_mut() {
                    editor.discard_unsaved_secrets();
                }
            }
            slint::CloseRequestResponse::HideWindow
        });
        let _ = editor.window.show();
        native_window::prepare_dialog(
            &editor.window,
            true,
            true,
            native_window::hwnd(self.window.window()),
        );
        *self.settings.borrow_mut() = Some(editor);
    }

    fn open_service_settings(self: &Rc<Self>, mode: &str, host: &str) {
        let request_owner = Rc::downgrade(self);
        let id_owner = Rc::downgrade(self);
        match crate::service_settings::ServiceSettings::new(
            mode,
            &self.window,
            Rc::new(move |request| {
                request_owner
                    .upgrade()
                    .is_some_and(|this| this.request(request))
            }),
            Rc::new(move || id_owner.upgrade().map_or(0, |this| this.id())),
        ) {
            Ok(service) => {
                service.window.set_host(host.into());
                service
                    .window
                    .set_default_cookie(mode == "credentials" && host.is_empty());
                if let Some(previous) = self.service_settings.borrow_mut().replace(service.clone())
                {
                    previous.window.invoke_action("close".into());
                }
                service.show(native_window::hwnd(self.window.window()));
            }
            Err(error) => self.show_notice("error", error.to_string()),
        }
    }
}

fn sync_settings_edit_status(editor: &SettingsEditor) {
    if editor.window.get_busy() || editor.has_validation_errors() {
        return;
    }
    if editor.has_unsaved_changes() {
        editor.window.set_status_tone("info".into());
        editor
            .window
            .set_status("有未保存的更改，点击「保存设置」生效".into());
    } else {
        editor.window.set_status("".into());
    }
}

fn infer_toast_tone(message: &str) -> &'static str {
    if message.contains("失败") || message.contains("错误") || message.contains("异常") {
        "error"
    } else if message.contains("警告") || message.contains("取消") || message.contains("空间不足")
    {
        "warning"
    } else if message.contains("已完成")
        || message.contains("已保存")
        || message.contains("已导入")
        || message.contains("已导出")
        || message.contains("已清理")
        || message.contains("已清空")
        || message.contains("已复制")
        || message.contains("成功")
    {
        "success"
    } else {
        "info"
    }
}

fn file_tile_kind(filename: &str, url: &str, resource_kind: ResourceKind) -> &'static str {
    if resource_kind == ResourceKind::Torrent
        || url.trim_start().to_ascii_lowercase().starts_with("magnet:")
    {
        return "torrent";
    }
    let ext = filename
        .split(['?', '#'])
        .next()
        .unwrap_or(filename)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "torrent" {
        return "torrent";
    }
    match hls_client::download_category(filename, url, resource_kind) {
        "media" => "media",
        "program" => "program",
        "archive" => "archive",
        _ => {
            if matches!(
                ext.as_str(),
                "pdf"
                    | "doc"
                    | "docx"
                    | "xls"
                    | "xlsx"
                    | "ppt"
                    | "pptx"
                    | "txt"
                    | "md"
                    | "csv"
                    | "epub"
                    | "mobi"
                    | "rtf"
                    | "json"
                    | "xml"
            ) {
                "document"
            } else {
                "file"
            }
        }
    }
}

fn percent_decode_utf8(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_owned())
}

/// Validates whether `raw` clipboard text is a single-line `<= 4096`-char
/// supported download URL (`http://`, `https://`, `ftp://`, `sftp://`, `magnet:?`)
/// and returns `(normalized_url, inferred_filename)`.
pub(crate) fn passive_clipboard_url_prefill(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 4096
        || trimmed.contains(['\r', '\n'])
        || trimmed.chars().any(char::is_whitespace)
    {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("magnet:?") {
        let query = &trimmed["magnet:?".len()..];
        if !query
            .split('&')
            .any(|kv| kv.to_ascii_lowercase().starts_with("xt=urn:"))
        {
            return None;
        }
        let dn = query
            .split('&')
            .find_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                k.eq_ignore_ascii_case("dn").then_some(v)
            })
            .map(|v| percent_decode_utf8(&v.replace('+', " ")))
            .map(|name| crate::target_check::fix_name(&name))
            .filter(|name| !name.is_empty() && name != "download")
            .unwrap_or_default();
        return Some((trimmed.to_owned(), dn));
    }

    let (scheme, rest) = trimmed.split_once("://")?;
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp" | "sftp"
    ) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("").trim();
    if host.is_empty()
        || (!host.contains('.')
            && !host.eq_ignore_ascii_case("localhost")
            && !host.starts_with('['))
    {
        return None;
    }
    let without_tail = rest.split(['?', '#']).next().unwrap_or(rest);
    let path = without_tail
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or("");
    let leaf = path.rsplit('/').find(|seg| !seg.is_empty()).unwrap_or("");
    let inferred = if let Some((stem, ext)) = leaf.rsplit_once('.') {
        if !stem.is_empty() && !ext.is_empty() && ext.len() <= 12 {
            let decoded = percent_decode_utf8(leaf);
            let clean = crate::target_check::fix_name(&decoded);
            if clean == "download" {
                String::new()
            } else {
                clean
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    Some((trimmed.to_owned(), inferred))
}

pub(crate) fn apply_passive_clipboard_prefill(editor: &TaskEditorWindow, raw: &str) -> bool {
    if editor.get_mode().as_str() != "new" || !editor.get_url().trim().is_empty() {
        return false;
    }
    let Some((url, filename)) = passive_clipboard_url_prefill(raw) else {
        return false;
    };
    let lower_url = url.to_ascii_lowercase();
    let lower_name = filename.to_ascii_lowercase();
    let kind = if lower_url.starts_with("magnet:?") || lower_name.ends_with(".torrent") {
        "种子 / 磁力"
    } else if lower_name.ends_with(".m3u8") {
        "HLS"
    } else if lower_name.ends_with(".mpd") {
        "DASH"
    } else {
        "普通文件"
    };
    editor.set_url(url.into());
    if !filename.is_empty() && editor.get_filename().trim().is_empty() {
        editor.set_filename(filename.into());
    }
    editor.set_kind(kind.into());
    editor.set_result_tone("info".into());
    editor.set_result("已自动填入剪贴板链接".into());
    true
}

fn kind_label(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Hls => "HLS",
        ResourceKind::Dash => "DASH",
        ResourceKind::Live => "直播",
        ResourceKind::Torrent => "种子 / 磁力",
        _ => "普通文件",
    }
}

fn selected_output_paths<'a>(
    tasks: impl Iterator<Item = &'a TaskSnapshot>,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for task in tasks.filter(|task| task_action_allowed(task, "copy_file")) {
        let path = PathBuf::from(&task.output_path);
        if !path.is_absolute() {
            return Err(format!("{} 的文件路径尚未就绪，请刷新任务", task.filename));
        }
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        Err("请选择已下载的文件".into())
    } else {
        Ok(paths)
    }
}

fn task_start_action(task: &TaskSnapshot) -> Option<&'static str> {
    // Batch start excludes finished tasks; an explicit retry stays available.
    if matches!(task.status.as_str(), "completed" | "done") {
        return None;
    }
    ["resume", "retry", "start"]
        .into_iter()
        .find(|action| task_action_allowed(task, action))
}

fn is_media_capable_task(task: &TaskSnapshot) -> bool {
    if matches!(
        task.resource_kind,
        ResourceKind::Hls | ResourceKind::Dash | ResourceKind::Live
    ) {
        return true;
    }
    let extension = task
        .filename
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    hls_client::media_ext::MEDIA_PLAYABLE_EXTENSIONS.contains(&extension.as_str())
        || task
            .available_actions
            .iter()
            .any(|action| matches!(action.as_str(), "play" | "cast"))
}

pub(crate) fn extract_curl_probe_url(raw: &str) -> Option<String> {
    let joined = raw
        .replace("\\\r\n", " ")
        .replace("\\\n", " ")
        .replace("^\r\n", " ")
        .replace("^\n", " ");
    let mut tokens = Vec::new();
    let mut chars = joined.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        let mut token = String::new();
        if ch == '\'' || ch == '"' {
            let quote = ch;
            chars.next();
            while let Some(c) = chars.next() {
                if c == '\\' && quote == '"' {
                    if let Some(escaped) = chars.next() {
                        token.push(escaped);
                    }
                } else if c == quote {
                    break;
                } else {
                    token.push(c);
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                token.push(c);
                chars.next();
            }
        }
        if !token.is_empty() {
            tokens.push(token);
        }
    }
    let is_supported = |candidate: &str| -> bool {
        let trimmed = candidate.trim_matches(|c| c == '\'' || c == '"').trim();
        let Some((scheme, rest)) = trimmed.split_once("://") else {
            return false;
        };
        matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ftp" | "sftp"
        ) && !rest.is_empty()
    };
    let mut i = 0;
    while i < tokens.len() {
        let current = tokens[i].as_str();
        if current == "--url" {
            if let Some(next) = tokens.get(i + 1) {
                if is_supported(next) {
                    return Some(next.trim().to_owned());
                }
            }
            i += 2;
            continue;
        }
        if let Some(rest) = current.strip_prefix("--url=") {
            let unquoted = rest.trim_matches(|c| c == '\'' || c == '"');
            if is_supported(unquoted) {
                return Some(unquoted.to_owned());
            }
        }
        if matches!(
            current,
            "-H" | "--header"
                | "-X"
                | "--request"
                | "-b"
                | "--cookie"
                | "-A"
                | "--user-agent"
                | "-e"
                | "--referer"
                | "-d"
                | "--data"
                | "--data-raw"
                | "--data-binary"
                | "-o"
                | "--output"
                | "-u"
                | "--user"
                | "-x"
                | "--proxy"
        ) {
            i += 2;
            continue;
        }
        if is_supported(current) {
            return Some(current.trim().to_owned());
        }
        i += 1;
    }
    None
}

fn task_action_allowed(task: &TaskSnapshot, action: &str) -> bool {
    let available = |action: &str| task.available_actions.iter().any(|value| value == action);
    match action {
        "play" | "cast" | "push_tvbox" => {
            // Core also marks ordinary finished files as ready. Only media can
            // use readiness for playback while downloading.
            let media_kind = matches!(
                task.resource_kind,
                ResourceKind::Hls | ResourceKind::Dash | ResourceKind::Live
            );
            let extension = task
                .filename
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_ascii_lowercase())
                .unwrap_or_default();
            let media_file =
                hls_client::media_ext::MEDIA_PLAYABLE_EXTENSIONS.contains(&extension.as_str());
            (task.playback_ready && (media_kind || media_file))
                || available(if action == "push_tvbox" {
                    "cast"
                } else {
                    action
                })
        }
        "delete_files" => available("delete"),
        "copy_file" | "drag_file" => available("open"),
        "copy_url" => !task.url.trim().is_empty(),
        "refresh_browser" => {
            matches!(task.status.as_str(), "failed" | "error" | "paused")
                && matches!(
                    task.resource_kind,
                    ResourceKind::File | ResourceKind::Hls | ResourceKind::Dash
                )
                && is_http_like_url(&task.url)
        }
        _ => available(action),
    }
}

fn is_http_like_url(url: &str) -> bool {
    let trimmed = url.trim();
    trimmed.split_once("://").is_some_and(|(scheme, rest)| {
        (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            && !rest.is_empty()
    })
}

fn format_speed_limit_pill(enabled: bool, kib: u64) -> String {
    if !enabled || kib == 0 {
        return "限速：关".into();
    }
    if kib >= 1024 {
        if kib.is_multiple_of(1024) {
            format!("限速 {} MB/s", kib / 1024)
        } else {
            format!("限速 {:.1} MB/s", kib as f64 / 1024.0)
        }
    } else {
        format!("限速 {kib} KB/s")
    }
}

fn toggle_speed_limit_values(
    current_enabled: bool,
    current_kib: u64,
) -> (
    bool,
    u64,
    std::collections::BTreeMap<String, serde_json::Value>,
) {
    let next_enabled = !(current_enabled && current_kib > 0);
    let mut values = std::collections::BTreeMap::new();
    values.insert(
        "download_speed_limit_enabled".to_owned(),
        serde_json::Value::Bool(next_enabled),
    );
    let effective_kib = if next_enabled && current_kib == 0 {
        values.insert(
            "download_speed_limit_kib".to_owned(),
            serde_json::Value::from(1024u64),
        );
        1024
    } else {
        current_kib
    };
    (next_enabled, effective_kib, values)
}

fn update_batch_preview(editor: &TaskEditorWindow) {
    if editor.get_mode().as_str() != "batch" {
        return;
    }
    let expansion =
        crate::wildcard::expand_batch(&editor.get_url(), crate::wildcard::MAX_BATCH_URLS);
    let (preview, active, warning) = expansion.preview_status();
    editor.set_batch_preview(preview.into());
    editor.set_batch_preview_active(active);
    editor.set_batch_preview_warning(warning);
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TaskTelemetrySummary {
    bt_summary: String,
    mirror_summary: String,
    checksum_badge: String,
    checksum_tone: &'static str,
    av_badge: String,
    av_tone: &'static str,
    av_detail: String,
}

fn checksum_algorithm_label(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "SHA-256".into();
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "sha256" | "sha-256" => "SHA-256".into(),
        "sha1" | "sha-1" => "SHA-1".into(),
        "md5" => "MD5".into(),
        "sha512" | "sha-512" => "SHA-512".into(),
        _ => trimmed.to_ascii_uppercase(),
    }
}

fn summarize_task_telemetry(task: &TaskSnapshot) -> TaskTelemetrySummary {
    let mut summary = TaskTelemetrySummary::default();

    let has_bt = task.resource_kind == ResourceKind::Torrent
        || task.peer_count > 0
        || task.seed_count > 0
        || task.upload_speed_bytes_per_sec > 0
        || task.uploaded_bytes > 0;
    if has_bt {
        let mut parts = vec![format!(
            "连接 {} 节点 ({} 做种)",
            task.peer_count, task.seed_count
        )];
        if task.upload_speed_bytes_per_sec > 0 {
            parts.push(format!(
                "上传 {}/s",
                crate::format_bytes(task.upload_speed_bytes_per_sec)
            ));
        }
        if task.uploaded_bytes > 0 {
            parts.push(format!(
                "已上传 {}",
                crate::format_bytes(task.uploaded_bytes)
            ));
        }
        summary.bt_summary = parts.join(" · ");
    }

    if !task.mirror_status.is_empty() {
        let total = task.mirror_status.len();
        let available = task
            .mirror_status
            .iter()
            .filter(|mirror| {
                matches!(
                    mirror.state.as_str(),
                    "active" | "pending" | "ready" | "ok" | "available"
                )
            })
            .count();
        summary.mirror_summary = format!("镜像源 {available}/{total} 可用");
    } else if !task.mirrors.is_empty() {
        summary.mirror_summary = format!("镜像源 {} 个已配置", task.mirrors.len());
    }

    if let Some(verified) = task.checksum_verified {
        let algo = checksum_algorithm_label(&task.checksum_algorithm);
        if verified {
            summary.checksum_badge = format!("{algo} 校验通过");
            summary.checksum_tone = "success";
        } else {
            summary.checksum_badge = format!("{algo} 校验不匹配");
            summary.checksum_tone = "fail";
        }
    }

    if let Some(scan) = task.av_scan.as_ref() {
        let state = scan.state.trim();
        let (badge, tone) = match state {
            "clean" | "ok" | "passed" => ("安全扫描：通过".to_owned(), "success"),
            "threat" | "infected" => ("安全扫描：发现风险".to_owned(), "fail"),
            "error" => ("安全扫描：异常".to_owned(), "warn"),
            "skipped" => ("安全扫描：已跳过".to_owned(), "muted"),
            "" => (String::new(), ""),
            other => (format!("安全扫描：{other}"), "info"),
        };
        summary.av_badge = badge;
        summary.av_tone = tone;
        summary.av_detail = scan.detail.trim().to_owned();
    }

    summary
}

#[derive(Debug, PartialEq, Eq)]
struct EditorValidationError {
    message: String,
    advanced: bool,
}

fn editor_spec(
    editor: &TaskEditorWindow,
    variants: &[StreamVariant],
) -> Result<TaskSpec, EditorValidationError> {
    let url = editor.get_url().trim().to_owned();
    if url.is_empty() {
        return Err(EditorValidationError {
            message: "请输入下载地址".into(),
            advanced: false,
        });
    }
    let kind = match editor.get_kind().as_str() {
        "HLS" => ResourceKind::Hls,
        "DASH" => ResourceKind::Dash,
        "直播" => ResourceKind::Live,
        "种子 / 磁力" => ResourceKind::Torrent,
        // Core classifies each URL independently, including signed URLs, live,
        // FTP and SFTP. A batch must not inherit a type guessed from its text.
        _ => ResourceKind::File,
    };
    let mut headers = std::collections::BTreeMap::new();
    for line in editor
        .get_headers()
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let Some((name, value)) = line.split_once(':') else {
            return Err(EditorValidationError {
                message: "请求头应为 Name: Value".into(),
                advanced: true,
            });
        };
        if name.trim().is_empty() {
            return Err(EditorValidationError {
                message: "请求头名称不能为空".into(),
                advanced: true,
            });
        }
        headers.insert(name.trim().to_owned(), value.trim().to_owned());
    }
    if !editor.get_cookie().trim().is_empty() {
        headers.insert("Cookie".into(), editor.get_cookie().trim().to_owned());
    }
    let variant = variants.get(editor.get_variant_index().max(0) as usize);
    Ok(TaskSpec {
        url,
        resource_kind: kind,
        filename: editor.get_filename().trim().into(),
        download_dir: editor.get_download_dir().trim().into(),
        request_method: editor.get_method().into(),
        headers,
        body_path: editor.get_body_path().trim().into(),
        concurrency: editor.get_concurrency().max(0) as u32,
        speed_limit_kib: editor.get_speed_limit().max(0) as u32,
        checksum: (!editor.get_checksum().trim().is_empty())
            .then(|| editor.get_checksum().trim().into()),
        mirrors: editor
            .get_mirrors()
            .split('|')
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
            .collect(),
        proxy: editor.get_proxy().trim().into(),
        preferred_bandwidth: variant.map_or(0, |value| value.bandwidth),
        preferred_height: variant.map_or(0, |value| value.height),
        preferred_audio: editor.get_audio().into(),
        scheduled_start_at: editor.get_start_at().trim().into(),
        scheduled_stop_at: editor.get_stop_at().trim().into(),
        queue_id: {
            let raw = editor.get_queue_id();
            let queue_id = raw.trim();
            if queue_id.is_empty() {
                hls_client::DEFAULT_QUEUE_ID.into()
            } else {
                queue_id.into()
            }
        },
        allow_duplicate: editor.get_allow_duplicate(),
        start_paused: editor.get_start_paused(),
        ..Default::default()
    })
}

fn row(task: &TaskSnapshot, selected: bool) -> TaskRow {
    let name = if task.filename.is_empty() {
        &task.title
    } else {
        &task.filename
    };
    let kind = file_tile_kind(&task.filename, &task.url, task.resource_kind);
    let scheme = task
        .url
        .split_once(':')
        .map_or("HTTP", |(scheme, _)| scheme)
        .to_ascii_uppercase();
    let protocol_badge: &str = match task.resource_kind {
        ResourceKind::Hls => "HLS",
        ResourceKind::Dash => "DASH",
        ResourceKind::Live => "LIVE",
        ResourceKind::Torrent => "BT",
        ResourceKind::Ftp => "FTP",
        ResourceKind::Sftp => "SFTP",
        ResourceKind::File => match scheme.as_str() {
            "FTP" => "FTP",
            "SFTP" => "SFTP",
            "MAGNET" => "BT",
            _ => "",
        },
    };
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| !extension.is_empty() && extension.len() <= 10);
    let host = crate::url_host(&task.url);
    let mut meta_parts = Vec::with_capacity(3);
    if protocol_badge.is_empty() {
        meta_parts.push(if matches!(scheme.as_str(), "HTTP" | "HTTPS") {
            scheme
        } else {
            "HTTP".into()
        });
    }
    if let Some(ext) = extension {
        meta_parts.push(ext.to_ascii_uppercase());
    }
    if !host.is_empty() {
        meta_parts.push(host);
    }
    if meta_parts.is_empty() {
        meta_parts.push("文件下载".into());
    }
    let metadata = meta_parts.join(" · ");
    let actions: &[(&str, &str)] = if matches!(task.status.as_str(), "completed" | "done") {
        &[
            ("play", "播放"),
            ("open", "打开文件"),
            ("open_folder", "打开所在位置"),
            ("retry", "重新下载"),
        ]
    } else {
        &[
            ("pause", "暂停"),
            ("resume", "继续"),
            ("retry", "重试"),
            ("start", "开始"),
            ("play", "播放"),
            ("open_folder", "打开所在位置"),
        ]
    };
    let primary = actions
        .iter()
        .copied()
        .find(|(action, _)| task_action_allowed(task, action));
    TaskRow {
        id: task.task_id.clone().into(),
        name: if task.filename.is_empty() {
            &task.title
        } else {
            &task.filename
        }
        .into(),
        status: crate::task_status_label(&task.status).into(),
        size: task
            .total_bytes
            .map(crate::format_bytes)
            .unwrap_or_else(|| "未知".into())
            .into(),
        speed: if crate::task_is_active(&task.status) && task.speed_bytes_per_sec > 0 {
            format!("{} /s", crate::format_bytes(task.speed_bytes_per_sec))
        } else {
            "—".into()
        }
        .into(),
        eta: task
            .eta_seconds
            .map(|value| format!("{}:{:02}", value / 60, value % 60))
            .unwrap_or_default()
            .into(),
        progress: crate::task_progress(task),
        selected,
        kind: kind.into(),
        protocol: protocol_badge.into(),
        metadata: metadata.into(),
        status_code: task.status.clone().into(),
        primary_action: primary.map_or("", |(action, _)| action).into(),
        primary_label: primary.map_or("", |(_, label)| label).into(),
    }
}

pub(crate) fn report_ui_ready(launch_id: Option<String>, surface: &'static str) {
    if let Some(launch_id) = launch_id {
        std::thread::spawn(move || hls_client::report_workbench_launch(launch_id, surface));
    }
}

#[cfg(test)]
mod file_selection_tests {
    #[test]
    fn blocked_system_download_does_not_delay_interactive_commands() {
        use super::{Workbench, WorkbenchJob};
        use hls_client::{CoreCommand, CorePipeRequest};
        use std::{sync::mpsc, time::Duration};

        let (started, entered) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (finished, completed) = mpsc::channel();
        let system = Workbench::spawn_job_worker(8, move |job| {
            assert!(matches!(job, WorkbenchJob::Request(request)
                if matches!(*request, CorePipeRequest::Command { command: CoreCommand::DownloadUpdate, .. })));
            started.send(()).unwrap();
            blocked.recv().unwrap();
            finished.send("system").unwrap();
            false
        });
        let (dispatched, interactive) = mpsc::channel();
        let commands = Workbench::spawn_job_worker(32, move |job| {
            assert!(matches!(job, WorkbenchJob::Request(request)
                if matches!(*request, CorePipeRequest::Command { command: CoreCommand::TaskAction { ref action, .. }, .. } if action == "pause")));
            dispatched.send(()).unwrap();
            false
        });
        system
            .send(WorkbenchJob::Request(Box::new(CorePipeRequest::Command {
                request_id: 1,
                command: CoreCommand::DownloadUpdate,
            })))
            .unwrap();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        commands
            .send(WorkbenchJob::Request(Box::new(CorePipeRequest::Command {
                request_id: 2,
                command: CoreCommand::TaskAction {
                    task_id: "task".into(),
                    action: "pause".into(),
                },
            })))
            .unwrap();
        let result = interactive.recv_timeout(Duration::from_secs(5));
        assert!(completed.try_recv().is_err());
        release.send(()).unwrap();
        result.unwrap();
        assert_eq!(
            completed.recv_timeout(Duration::from_secs(5)).unwrap(),
            "system"
        );
    }

    use super::*;

    #[test]
    fn copies_all_completed_paths_in_visible_order_without_duplicates() {
        let first = TaskSnapshot {
            output_path: std::env::temp_dir()
                .join("视频.mp4")
                .to_string_lossy()
                .into(),
            available_actions: vec!["open".into()],
            ..Default::default()
        };
        let second = TaskSnapshot {
            output_path: std::env::temp_dir()
                .join("资料.zip")
                .to_string_lossy()
                .into(),
            ..first.clone()
        };
        let paused = TaskSnapshot::default();
        assert_eq!(
            selected_output_paths([&first, &paused, &second, &first].into_iter()).unwrap(),
            vec![
                PathBuf::from(&first.output_path),
                PathBuf::from(&second.output_path)
            ]
        );
    }

    #[test]
    fn missing_snapshot_paths_do_not_silently_reduce_the_selection() {
        let ready = TaskSnapshot {
            output_path: std::env::temp_dir()
                .join("ready.bin")
                .to_string_lossy()
                .into(),
            available_actions: vec!["open".into()],
            ..Default::default()
        };
        let unresolved = TaskSnapshot {
            filename: "missing.bin".into(),
            output_path: String::new(),
            ..ready.clone()
        };
        assert!(selected_output_paths([&ready, &unresolved].into_iter())
            .unwrap_err()
            .contains("missing.bin"));
        assert!(selected_output_paths([&TaskSnapshot::default()].into_iter()).is_err());
    }

    #[test]
    fn completed_rows_open_or_play_and_batch_start_never_redownloads() {
        let mut task = TaskSnapshot {
            status: "completed".into(),
            available_actions: vec!["open".into(), "open_folder".into(), "retry".into()],
            ..Default::default()
        };
        assert_eq!(row(&task, false).primary_action.as_str(), "open");
        assert_eq!(task_start_action(&task), None);
        assert!(task_action_allowed(&task, "retry"));
        task.filename = "视频.mp4".into();
        task.playback_ready = true;
        assert_eq!(row(&task, false).primary_action.as_str(), "play");
        task.status = "done".into();
        task.output_missing = true;
        task.available_actions = vec!["retry".into(), "delete".into()];
        assert_eq!(task_start_action(&task), None);
        task.playback_ready = false;
        task.status = "failed".into();
        assert_eq!(row(&task, false).primary_action.as_str(), "retry");
        assert_eq!(task_start_action(&task), Some("retry"));
        task.status = "paused".into();
        task.available_actions = vec!["resume".into(), "retry".into(), "delete".into()];
        assert_eq!(task_start_action(&task), Some("resume"));
        task.status = "queued".into();
        task.available_actions = vec!["start".into(), "delete".into()];
        assert_eq!(task_start_action(&task), Some("start"));
        task.status = "downloading".into();
        task.available_actions = vec!["pause".into(), "cancel".into()];
        assert_eq!(task_start_action(&task), None);
    }

    #[test]
    fn ready_non_media_files_keep_open_as_the_primary_action() {
        for filename in [
            "资料.bin",
            "资料.zip",
            "照片.jpg",
            "照片.png",
            "封面.webp",
            "清单.mpd",
            "无扩展名",
        ] {
            let task = TaskSnapshot {
                filename: filename.into(),
                status: "completed".into(),
                playback_ready: true,
                available_actions: vec!["open".into(), "open_folder".into(), "retry".into()],
                ..Default::default()
            };
            assert_eq!(
                row(&task, false).primary_action.as_str(),
                "open",
                "{filename}"
            );
            for action in ["play", "cast", "push_tvbox"] {
                assert!(!task_action_allowed(&task, action), "{filename}: {action}");
            }
        }
    }

    #[test]
    fn playback_requires_ready_media_or_an_explicit_core_action() {
        for (filename, resource_kind) in [
            ("视频.MP4", ResourceKind::File),
            ("音频.mp3", ResourceKind::File),
            ("无扩展名", ResourceKind::Hls),
            ("无扩展名", ResourceKind::Dash),
            ("无扩展名", ResourceKind::Live),
        ] {
            let mut task = TaskSnapshot {
                filename: filename.into(),
                resource_kind,
                ..Default::default()
            };
            for action in ["play", "cast", "push_tvbox"] {
                assert!(!task_action_allowed(&task, action));
            }
            task.playback_ready = true;
            for action in ["play", "cast", "push_tvbox"] {
                assert!(task_action_allowed(&task, action));
            }
        }
        let mut task = TaskSnapshot {
            available_actions: vec!["play".into()],
            ..Default::default()
        };
        assert!(task_action_allowed(&task, "play"));
        assert!(!task_action_allowed(&task, "cast"));
        task.available_actions = vec!["cast".into()];
        assert!(!task_action_allowed(&task, "play"));
        assert!(task_action_allowed(&task, "cast"));
        assert!(task_action_allowed(&task, "push_tvbox"));
    }

    #[test]
    fn editor_spec_marks_header_validation_failures_as_advanced() {
        crate::offscreen::OffscreenPlatform::init_test();
        let editor = TaskEditorWindow::new().unwrap();
        let missing_url = editor_spec(&editor, &[]).unwrap_err();
        assert!(!missing_url.advanced);

        editor.set_url("https://cdn.example.test/video.mp4".into());
        editor.set_headers("MalformedHeaderWithoutColon".into());
        let bad_header = editor_spec(&editor, &[]).unwrap_err();
        assert!(bad_header.advanced);
    }

    #[test]
    fn inspector_skips_rebuilds_while_closed_and_shows_single_line_connections_when_opened() {
        crate::offscreen::OffscreenPlatform::init_test();
        let task = TaskSnapshot {
            task_id: "insp-1".into(),
            filename: "movie.mp4".into(),
            status: "downloading".into(),
            active_workers: 4,
            completed_ranges: 1,
            total_ranges: 4,
            total_bytes: Some(4 * 1024 * 1024),
            connection_hint: "动态分段传输中".into(),
            connection_parts: vec![
                hls_client::ConnectionPart {
                    start: 0,
                    end: 1024 * 1024 - 1,
                    done: 1024 * 1024,
                    state: "done".into(),
                },
                hls_client::ConnectionPart {
                    start: 1024 * 1024,
                    end: 2 * 1024 * 1024 - 1,
                    done: 512 * 1024,
                    state: "active".into(),
                },
            ],
            speed_history: vec![100_000, 200_000, 300_000],
            ..Default::default()
        };
        let workbench = Workbench::new(vec![task], 1, false, true, true).unwrap();
        workbench.state.borrow_mut().select("insp-1", false, false);
        workbench.render_selection();
        // While closed, heavy inspector VecModels must not be rebuilt.
        assert!(!workbench.window.get_inspector_open());
        assert_eq!(workbench.window.get_inspector_segments().row_count(), 0);
        assert_eq!(
            workbench.window.get_inspector_speed_history().row_count(),
            0
        );

        // Opening the inspector via Alt+I immediately renders fresh data with a single-line summary.
        workbench.window.show().unwrap();
        workbench
            .window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Alt.into(),
            });
        workbench
            .window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "i".into() });
        workbench
            .window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: "i".into() });
        workbench
            .window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased {
                text: slint::platform::Key::Alt.into(),
            });
        assert!(workbench.window.get_inspector_open());
        assert_eq!(workbench.window.get_inspector_title().as_str(), "movie.mp4");
        assert_eq!(workbench.window.get_inspector_segments().row_count(), 2);
        assert_eq!(
            workbench.window.get_inspector_speed_history().row_count(),
            3
        );
        let connections = workbench.window.get_inspector_connections();
        assert!(!connections.contains('\n'));
        assert!(connections.contains("4 个活动连接"));
        workbench.window.hide().unwrap();
    }

    #[test]
    fn ui_show_new_download_and_clipboard_offer_open_prefilled_editor_with_queue_combo() {
        crate::offscreen::OffscreenPlatform::init_test();
        let workbench = Workbench::new(Vec::new(), 1, false, true, true).unwrap();
        let settings: CorePipeResponse = serde_json::from_value(serde_json::json!({
            "type": "settings",
            "request_id": 1,
            "takeover_enabled": true,
            "takeover_minimum_bytes": 0,
            "legal_accepted": true,
            "speed_limit_kib": 0,
            "download_dir": "D:/Downloads",
            "queue_profiles": [
                {"id": "default", "name": "默认队列", "priority": 0},
                {"id": "vip", "name": "高速队列", "priority": -10}
            ]
        }))
        .unwrap();
        workbench.response(settings);

        workbench.apply(&EventEnvelope {
            sequence: 2,
            event: CoreEvent::UiShow {
                surface: "new_download".into(),
            },
        });
        {
            let editor = workbench.editor.borrow();
            let editor = editor
                .as_ref()
                .expect("new_download opens TaskEditorWindow");
            assert_eq!(editor.get_mode().as_str(), "new");
            assert_eq!(
                editor
                    .get_queue_names()
                    .iter()
                    .map(String::from)
                    .collect::<Vec<_>>(),
                vec!["高速队列", "默认队列"]
            );
            assert_eq!(editor.get_queue_id().as_str(), "default");
            assert_eq!(editor.get_queue_index(), 1);
            editor.invoke_queue_selected(0);
            assert_eq!(editor.get_queue_id().as_str(), "vip");
            editor.set_url("https://cdn.example.test/video.mp4".into());
            assert_eq!(editor_spec(editor, &[]).unwrap().queue_id, "vip");
        }

        // Single URL clipboard offer -> "new" mode.
        workbench.open_clipboard_offer(&["https://example.test/a.zip".into()]);
        {
            let editor = workbench.editor.borrow();
            let editor = editor.as_ref().unwrap();
            assert_eq!(editor.get_mode().as_str(), "new");
            assert_eq!(editor.get_url().as_str(), "https://example.test/a.zip");
        }

        // Multi-URL clipboard offer -> "batch" mode.
        workbench.open_clipboard_offer(&[
            "https://example.test/1.bin".into(),
            "https://example.test/2.bin".into(),
        ]);
        {
            let editor = workbench.editor.borrow();
            let editor = editor.as_ref().unwrap();
            assert_eq!(editor.get_mode().as_str(), "batch");
            assert_eq!(
                editor.get_url().as_str(),
                "https://example.test/1.bin\nhttps://example.test/2.bin"
            );
        }

        // ConfirmWindow "更多设置" handoff -> prefilled single editor with offer fields & queue.
        let offer = hls_client::ResourceOffer {
            url: "https://cdn.example.test/stream/index.m3u8?sig=abc".into(),
            resource_kind: ResourceKind::Hls,
            request_method: "POST".into(),
            ..Default::default()
        };
        workbench.open_editor_from_offer(&offer, "lecture.mp4", "D:/Media", "vip");
        {
            let editor = workbench.editor.borrow();
            let editor = editor.as_ref().unwrap();
            assert_eq!(editor.get_mode().as_str(), "new");
            assert_eq!(editor.get_url().as_str(), offer.url);
            assert_eq!(editor.get_filename().as_str(), "lecture.mp4");
            assert_eq!(editor.get_download_dir().as_str(), "D:/Media");
            assert_eq!(editor.get_kind().as_str(), "HLS");
            assert_eq!(editor.get_method().as_str(), "POST");
            assert_eq!(editor.get_queue_id().as_str(), "vip");
            assert_eq!(editor.get_queue_index(), 0);
            assert!(editor.get_advanced_expanded());
            let _ = editor.hide();
        }
        workbench.window.hide().unwrap();
    }

    #[test]
    fn delete_action_routes_to_themed_decision_window() {
        crate::offscreen::OffscreenPlatform::init_test();
        let task = TaskSnapshot {
            task_id: "del-1".into(),
            filename: "sample.bin".into(),
            status: "paused".into(),
            available_actions: vec!["delete".into()],
            ..Default::default()
        };
        let workbench = Workbench::new(vec![task], 1, false, true, true).unwrap();
        workbench.state.borrow_mut().select("del-1", false, false);
        workbench.render_selection();
        workbench.action("delete");
        assert!(workbench.system.borrow().as_ref().unwrap().keeps_ui_alive());
        workbench.window.hide().unwrap();
    }

    #[test]
    fn speed_limit_pill_formats_and_toggles_cleanly() {
        assert_eq!(format_speed_limit_pill(false, 512), "限速：关");
        assert_eq!(format_speed_limit_pill(true, 0), "限速：关");
        assert_eq!(format_speed_limit_pill(true, 512), "限速 512 KB/s");
        assert_eq!(format_speed_limit_pill(true, 1024), "限速 1 MB/s");
        assert_eq!(format_speed_limit_pill(true, 1536), "限速 1.5 MB/s");

        // Turning on from 0 defaults to 1024 KiB/s.
        let (enabled, kib, values) = toggle_speed_limit_values(false, 0);
        assert!(enabled);
        assert_eq!(kib, 1024);
        assert_eq!(
            values["download_speed_limit_enabled"],
            serde_json::json!(true)
        );
        assert_eq!(values["download_speed_limit_kib"], serde_json::json!(1024));

        // Turning off preserves existing speed_limit_kib without overwriting it.
        let (enabled, kib, values) = toggle_speed_limit_values(true, 512);
        assert!(!enabled);
        assert_eq!(kib, 512);
        assert_eq!(
            values["download_speed_limit_enabled"],
            serde_json::json!(false)
        );
        assert!(!values.contains_key("download_speed_limit_kib"));

        // Turning back on with a saved non-zero limit keeps that limit.
        let (enabled, kib, values) = toggle_speed_limit_values(false, 512);
        assert!(enabled);
        assert_eq!(kib, 512);
        assert_eq!(
            values["download_speed_limit_enabled"],
            serde_json::json!(true)
        );
        assert!(!values.contains_key("download_speed_limit_kib"));
    }

    #[test]
    fn context_menu_allows_copy_url_and_gates_browser_refresh_by_kind_and_status() {
        let mut task = TaskSnapshot {
            task_id: "t-http".into(),
            url: "https://cdn.example.test/file.bin".into(),
            resource_kind: ResourceKind::File,
            status: "failed".into(),
            ..Default::default()
        };
        assert!(task_action_allowed(&task, "copy_url"));
        assert!(task_action_allowed(&task, "refresh_browser"));

        task.status = "paused".into();
        task.resource_kind = ResourceKind::Hls;
        assert!(task_action_allowed(&task, "refresh_browser"));

        task.resource_kind = ResourceKind::Dash;
        assert!(task_action_allowed(&task, "refresh_browser"));

        // Active or completed tasks cannot arm browser refresh.
        task.status = "downloading".into();
        assert!(!task_action_allowed(&task, "refresh_browser"));
        task.status = "completed".into();
        assert!(!task_action_allowed(&task, "refresh_browser"));

        // Torrent / FTP cannot arm browser refresh.
        task.status = "failed".into();
        task.resource_kind = ResourceKind::Torrent;
        assert!(!task_action_allowed(&task, "refresh_browser"));
        task.resource_kind = ResourceKind::File;
        task.url = "ftp://files.example.test/archive.tar".into();
        assert!(!task_action_allowed(&task, "refresh_browser"));

        task.url.clear();
        assert!(!task_action_allowed(&task, "copy_url"));
    }

    #[test]
    fn sort_arrows_select_all_and_drop_overlay_sync_on_workbench_window() {
        crate::offscreen::OffscreenPlatform::init_test();
        let a = TaskSnapshot {
            task_id: "t-a".into(),
            filename: "alpha.bin".into(),
            status: "paused".into(),
            ..Default::default()
        };
        let b = TaskSnapshot {
            task_id: "t-b".into(),
            filename: "beta.bin".into(),
            status: "paused".into(),
            ..Default::default()
        };
        let workbench = Workbench::new(vec![a, b], 1, false, true, true).unwrap();

        // Sort indicator tracks active column and direction.
        workbench.window.invoke_sort("name".into());
        assert_eq!(workbench.window.get_sort_column().as_str(), "name");
        assert!(!workbench.window.get_sort_descending());
        workbench.window.invoke_sort("name".into());
        assert_eq!(workbench.window.get_sort_column().as_str(), "name");
        assert!(workbench.window.get_sort_descending());

        // Header select-all syncs with selection state.
        assert!(!workbench.window.get_all_selected());
        workbench
            .window
            .invoke_key_command("select_all".into(), true, false);
        assert!(workbench.window.get_all_selected());
        assert_eq!(workbench.window.get_selected_count(), 2);
        workbench
            .window
            .invoke_select_row("t-a".into(), true, false);
        assert!(!workbench.window.get_all_selected());
        assert_eq!(workbench.window.get_selected_count(), 1);
        workbench
            .window
            .invoke_key_command("clear_selection".into(), false, false);
        assert!(!workbench.window.get_all_selected());
        assert_eq!(workbench.window.get_selected_count(), 0);

        // Drag-and-drop hover overlay toggles cleanly.
        assert!(!workbench.window.get_file_drop_hover());
        workbench.window.set_file_drop_hover(true);
        assert!(workbench.window.get_file_drop_hover());
        workbench.window.set_file_drop_hover(false);
        assert!(!workbench.window.get_file_drop_hover());
        workbench.window.hide().unwrap();
    }

    #[test]
    fn rich_telemetry_populates_inspector_and_details_windows() {
        crate::offscreen::OffscreenPlatform::init_test();
        let task = TaskSnapshot {
            task_id: "tel-1".into(),
            filename: "ubuntu.iso".into(),
            url: "https://cdn.example.test/ubuntu.iso".into(),
            source_page_url: "https://releases.example.test/download".into(),
            resource_kind: ResourceKind::Torrent,
            status: "downloading".into(),
            peer_count: 12,
            seed_count: 5,
            upload_speed_bytes_per_sec: 180 * 1024,
            uploaded_bytes: 4 * 1024 * 1024,
            mirror_status: vec![
                hls_client::MirrorStatus {
                    url: "https://m1.test/ubuntu.iso".into(),
                    state: "active".into(),
                    ..Default::default()
                },
                hls_client::MirrorStatus {
                    url: "https://m2.test/ubuntu.iso".into(),
                    state: "pending".into(),
                    ..Default::default()
                },
                hls_client::MirrorStatus {
                    url: "https://m3.test/ubuntu.iso".into(),
                    state: "failed".into(),
                    ..Default::default()
                },
            ],
            checksum_algorithm: "sha256".into(),
            checksum_verified: Some(true),
            av_scan: Some(hls_client::AvScanStatus {
                state: "clean".into(),
                engine: "defender".into(),
                detail: "Windows Defender 未发现威胁".into(),
            }),
            ..Default::default()
        };

        let summary = summarize_task_telemetry(&task);
        assert!(summary.bt_summary.contains("连接 12 节点 (5 做种)"));
        assert!(summary.bt_summary.contains("上传 180.0 KB/s"));
        assert!(summary.bt_summary.contains("已上传 4.0 MB"));
        assert_eq!(summary.mirror_summary, "镜像源 2/3 可用");
        assert_eq!(summary.checksum_badge, "SHA-256 校验通过");
        assert_eq!(summary.checksum_tone, "success");
        assert_eq!(summary.av_badge, "安全扫描：通过");
        assert_eq!(summary.av_tone, "success");

        // Mismatch and threat cases.
        let mut risky = task.clone();
        risky.checksum_verified = Some(false);
        risky.av_scan = Some(hls_client::AvScanStatus {
            state: "threat".into(),
            engine: "defender".into(),
            detail: "Trojan:Test".into(),
        });
        let risky_summary = summarize_task_telemetry(&risky);
        assert!(risky_summary.checksum_badge.contains("校验不匹配"));
        assert_eq!(risky_summary.checksum_tone, "fail");
        assert_eq!(risky_summary.av_badge, "安全扫描：发现风险");
        assert_eq!(risky_summary.av_tone, "fail");

        // Verify live binding into Inspector Drawer and DetailsWindow.
        let workbench = Workbench::new(vec![task], 1, false, true, true).unwrap();
        workbench.state.borrow_mut().select("tel-1", false, false);
        workbench.window.set_inspector_open(true);
        workbench.render_inspector();
        assert!(workbench
            .window
            .get_inspector_bt()
            .contains("连接 12 节点 (5 做种)"));
        assert_eq!(
            workbench.window.get_inspector_mirrors().as_str(),
            "镜像源 2/3 可用"
        );
        assert_eq!(
            workbench.window.get_inspector_checksum().as_str(),
            "SHA-256 校验通过"
        );
        assert_eq!(
            workbench.window.get_inspector_av().as_str(),
            "安全扫描：通过"
        );

        workbench.open_details();
        {
            let details = workbench.details.borrow();
            let (_, details_win) = details.as_ref().expect("details window opened");
            assert!(details_win
                .get_bt_telemetry()
                .contains("连接 12 节点 (5 做种)"));
            assert_eq!(
                details_win.get_mirror_telemetry().as_str(),
                "镜像源 2/3 可用"
            );
            assert_eq!(
                details_win.get_checksum_badge().as_str(),
                "SHA-256 校验通过"
            );
            assert_eq!(details_win.get_av_badge().as_str(), "安全扫描：通过");
            assert_eq!(
                details_win.get_source_page_url().as_str(),
                "https://releases.example.test/download"
            );
            let _ = details_win.hide();
        }
        workbench.window.hide().unwrap();
    }

    #[test]
    fn batch_editor_live_wildcard_preview_updates_on_edit() {
        crate::offscreen::OffscreenPlatform::init_test();
        let workbench = Workbench::new(Vec::new(), 1, false, true, true).unwrap();
        workbench.open_editor("批量添加");
        let editor = workbench.editor.borrow();
        let editor = editor.as_ref().expect("batch editor opened");
        assert_eq!(
            editor.get_batch_preview().as_str(),
            crate::wildcard::DEFAULT_BATCH_HINT
        );
        assert!(!editor.get_batch_preview_active());

        editor.set_url("https://cdn.example.test/ep_[001-012].mp4".into());
        editor.invoke_url_edited(editor.get_url());
        assert!(editor.get_batch_preview_active());
        assert!(!editor.get_batch_preview_warning());
        assert!(editor.get_batch_preview().contains("将展开为 12 个任务"));

        editor.set_url("https://cdn.example.test/ep_[001-600].mp4".into());
        editor.invoke_url_edited(editor.get_url());
        assert!(editor.get_batch_preview_warning());
        let _ = editor.hide();
        workbench.window.hide().unwrap();
    }

    #[test]
    fn passive_clipboard_url_prefill_accepts_single_supported_url_and_infers_filename() {
        crate::offscreen::OffscreenPlatform::init_test();

        assert_eq!(passive_clipboard_url_prefill(""), None);
        assert_eq!(passive_clipboard_url_prefill("just plain notes"), None);
        assert_eq!(
            passive_clipboard_url_prefill("https://a.test/1.zip\nhttps://a.test/2.zip"),
            None
        );
        assert_eq!(
            passive_clipboard_url_prefill(&format!("https://a.test/{}", "x".repeat(4100))),
            None
        );
        assert_eq!(passive_clipboard_url_prefill("javascript:alert(1)"), None);

        let (http_url, http_name) = passive_clipboard_url_prefill(
            "  https://cdn.example.test/releases/app%20setup-1.2.exe?sig=abc#dl  ",
        )
        .unwrap();
        assert_eq!(
            http_url,
            "https://cdn.example.test/releases/app%20setup-1.2.exe?sig=abc#dl"
        );
        assert_eq!(http_name, "app setup-1.2.exe");

        let (_, bare_path_name) =
            passive_clipboard_url_prefill("https://example.test/watch?v=42").unwrap();
        assert_eq!(bare_path_name, "");

        let (magnet_url, magnet_name) = passive_clipboard_url_prefill(
            "magnet:?xt=urn:btih:0123456789abcdef&dn=Ubuntu+24.04.iso",
        )
        .unwrap();
        assert!(magnet_url.starts_with("magnet:?xt=urn:btih:"));
        assert_eq!(magnet_name, "Ubuntu 24.04.iso");

        let editor = TaskEditorWindow::new().unwrap();
        editor.set_mode("new".into());
        assert!(apply_passive_clipboard_prefill(
            &editor,
            "https://cdn.example.test/live/index.m3u8?token=xyz"
        ));
        assert_eq!(
            editor.get_url().as_str(),
            "https://cdn.example.test/live/index.m3u8?token=xyz"
        );
        assert_eq!(editor.get_filename().as_str(), "index.m3u8");
        assert_eq!(editor.get_kind().as_str(), "HLS");

        // Does not overwrite an already-populated URL or batch mode.
        assert!(!apply_passive_clipboard_prefill(
            &editor,
            "https://cdn.example.test/other.mp4"
        ));
    }

    #[test]
    fn engine_online_and_dismissible_semantic_notices_track_ipc_health_and_expiry() {
        use std::time::{Duration, Instant};

        crate::offscreen::OffscreenPlatform::init_test();
        let workbench = Workbench::new(Vec::new(), 1, false, true, true).unwrap();
        assert!(workbench.window.get_engine_online());
        assert_eq!(workbench.window.get_connection().as_str(), "下载引擎已连接");

        // Transport connection failure flips engine-online to false and shows an error notice
        // that does NOT auto-expire after 5s.
        workbench.response(CorePipeResponse::Error {
            request_id: Some(99),
            code: "ui_connection".into(),
            message: "命名管道连接中断".into(),
        });
        assert!(!workbench.window.get_engine_online());
        assert_eq!(
            workbench.window.get_connection().as_str(),
            "正在重连下载引擎…"
        );
        assert_eq!(workbench.window.get_notice_tone().as_str(), "error");
        assert_eq!(workbench.window.get_notice().as_str(), "命名管道连接中断");
        workbench.expire_notice_at(Instant::now() + Duration::from_secs(10));
        assert_eq!(workbench.window.get_notice().as_str(), "命名管道连接中断");

        // User clicking × dismisses the notice immediately.
        workbench.window.invoke_dismiss_notice();
        assert_eq!(workbench.window.get_notice().as_str(), "");

        // Receiving a valid snapshot restores engine-online = true.
        workbench.response(CorePipeResponse::Snapshot {
            request_id: 100,
            tasks: Vec::new(),
            latest_sequence: 2,
            browser_connected: true,
        });
        assert!(workbench.window.get_engine_online());
        assert_eq!(workbench.window.get_connection().as_str(), "下载引擎已连接");

        // Info and success notices auto-expire cleanly after 5 seconds.
        let t0 = Instant::now();
        workbench.show_notice_at("success", "已复制下载链接", t0);
        assert_eq!(workbench.window.get_notice_tone().as_str(), "success");
        workbench.expire_notice_at(t0 + Duration::from_secs(4));
        assert_eq!(workbench.window.get_notice().as_str(), "已复制下载链接");
        workbench.expire_notice_at(t0 + Duration::from_secs(5));
        assert_eq!(workbench.window.get_notice().as_str(), "");
    }

    #[test]
    fn inspector_dock_shrinks_table_viewport_and_populates_logs() {
        crate::offscreen::OffscreenPlatform::init_test();
        let task = TaskSnapshot {
            task_id: "log-1".into(),
            filename: "manual.pdf".into(),
            url: "https://docs.example.test/manual.pdf".into(),
            status: "completed".into(),
            log_tail: vec!["[12:00:01] 连接建立".into(), "[12:00:03] 校验完成".into()],
            ..Default::default()
        };
        let workbench = Workbench::new(vec![task], 1, true, false, true).unwrap();
        assert_eq!(
            workbench.window.get_browser_status().as_str(),
            "浏览器已连接"
        );
        workbench.window.set_viewport_width(1400.0);
        let closed_viewport = workbench.table_viewport();
        workbench.state.borrow_mut().select("log-1", false, false);
        workbench.window.set_inspector_open(true);
        workbench.window.invoke_inspector_opened();
        assert!(workbench.window.get_inspector_docked());
        let docked_viewport = workbench.table_viewport();
        assert!((closed_viewport - docked_viewport - 328.0).abs() < 1.0);
        assert_eq!(workbench.window.get_inspector_kind().as_str(), "document");
        assert!(workbench.window.get_inspector_logs().contains("校验完成"));
        assert!(workbench.window.get_can_clear_completed());
    }

    #[test]
    fn non_active_tasks_display_dash_speed_and_exclude_stale_speed_from_telemetry() {
        crate::offscreen::OffscreenPlatform::init_test();
        let completed = TaskSnapshot {
            task_id: "done-1".into(),
            filename: "archive.zip".into(),
            status: "completed".into(),
            speed_bytes_per_sec: 1024 * 1024,
            available_actions: vec!["open".into(), "open_folder".into(), "delete".into()],
            ..Default::default()
        };
        let paused = TaskSnapshot {
            task_id: "paused-1".into(),
            filename: "installer.exe".into(),
            status: "paused".into(),
            speed_bytes_per_sec: 512 * 1024,
            available_actions: vec!["resume".into(), "delete".into()],
            ..Default::default()
        };
        let active = TaskSnapshot {
            task_id: "run-1".into(),
            filename: "stream.mp4".into(),
            status: "downloading".into(),
            speed_bytes_per_sec: 2 * 1024 * 1024,
            available_actions: vec!["pause".into(), "cancel".into()],
            ..Default::default()
        };
        assert_eq!(row(&completed, false).speed.as_str(), "—");
        assert_eq!(row(&paused, false).speed.as_str(), "—");
        assert_eq!(row(&active, false).speed.as_str(), "2.0 MB /s");

        let workbench =
            Workbench::new(vec![completed, paused, active], 1, false, false, true).unwrap();
        assert!(!workbench
            .system
            .borrow()
            .as_ref()
            .unwrap()
            .has_allocated_windows());
        assert!(!workbench
            .cast
            .borrow()
            .as_ref()
            .unwrap()
            .has_allocated_window());
        assert_eq!(workbench.window.get_telemetry_speed().as_str(), "2.0 MB/s");

        // Context menu on non-media (.zip) hides media-only items and drag-file trap.
        workbench.state.borrow_mut().select("done-1", false, false);
        workbench.render_selection();
        let menu_actions: Vec<String> = workbench
            .window
            .get_task_menu()
            .iter()
            .map(|r| r.action.to_string())
            .collect();
        assert!(!menu_actions.contains(&"task:play".to_owned()));
        assert!(!menu_actions.contains(&"task:cast".to_owned()));
        assert!(!menu_actions.contains(&"task:push_tvbox".to_owned()));
        assert!(!menu_actions.contains(&"task:drag_file".to_owned()));

        // Selecting a media task (.mp4) reveals media actions cleanly.
        workbench.state.borrow_mut().select("run-1", false, false);
        workbench.render_selection();
        let media_actions: Vec<String> = workbench
            .window
            .get_task_menu()
            .iter()
            .map(|r| r.action.to_string())
            .collect();
        assert!(media_actions.contains(&"task:play".to_owned()));
        assert!(media_actions.contains(&"task:cast".to_owned()));
        assert!(media_actions.contains(&"task:push_tvbox".to_owned()));
    }

    #[test]
    fn extract_curl_probe_url_parses_quoted_and_multiline_curl_commands() {
        assert_eq!(
            extract_curl_probe_url(
                "curl 'https://cdn.example.test/video/master.m3u8?token=abc' \\\n  -H 'Referer: https://site.example.test/' \\\n  -H 'Cookie: sid=1'"
            )
            .as_deref(),
            Some("https://cdn.example.test/video/master.m3u8?token=abc")
        );
        assert_eq!(
            extract_curl_probe_url(
                "curl.exe -X POST --url \"https://api.example.test/v1/file.mp4\" -d \"a=1\""
            )
            .as_deref(),
            Some("https://api.example.test/v1/file.mp4")
        );
        assert_eq!(
            extract_curl_probe_url("curl -H 'Referer: https://only-header.test'"),
            None
        );
    }

    #[test]
    fn failed_load_settings_disarms_settings_and_queues_without_ghost_opening_later() {
        crate::offscreen::OffscreenPlatform::init_test();
        let workbench = Workbench::new(Vec::new(), 1, false, true, true).unwrap();

        // 1. Core error on LoadSettings for both settings and queues disarms open IDs.
        workbench.action("settings");
        let failed_settings_id = workbench
            .settings_open_id
            .get()
            .expect("settings_open_id armed after LoadSettings request");
        workbench.action("queues");
        let failed_queues_id = workbench
            .queues_open_id
            .get()
            .expect("queues_open_id armed when settings_response is None");

        workbench.response(CorePipeResponse::Error {
            request_id: Some(failed_settings_id),
            code: "ui_connection".into(),
            message: "读取设置失败".into(),
        });
        assert!(workbench.settings_open_id.get().is_none());
        assert_eq!(workbench.window.get_notice_tone().as_str(), "error");
        assert_eq!(workbench.window.get_notice().as_str(), "读取设置失败");

        workbench.response(CorePipeResponse::Error {
            request_id: Some(failed_queues_id),
            code: "ui_connection".into(),
            message: "读取队列失败".into(),
        });
        assert!(workbench.queues_open_id.get().is_none());

        // Later unrelated Settings response (e.g., background SettingsChanged reload) must NOT
        // ghost-open SettingsWindow or QueueWindow.
        let unrelated_settings: CorePipeResponse = serde_json::from_value(serde_json::json!({
            "type": "settings",
            "request_id": 999,
            "takeover_enabled": true,
            "takeover_minimum_bytes": 0,
            "legal_accepted": true,
            "speed_limit_enabled": false,
            "speed_limit_kib": 0,
            "download_dir": "D:/Downloads"
        }))
        .unwrap();
        workbench.response(unrelated_settings);
        assert!(
            workbench.settings.borrow().is_none(),
            "SettingsWindow must not ghost-open on unrelated Settings response"
        );
        assert!(
            workbench.queues.borrow().is_none(),
            "QueueWindow must not ghost-open on unrelated Settings response"
        );

        // 2. Enqueue failure (full/closed command channel) shows error notice and never arms open IDs.
        *workbench.settings_response.borrow_mut() = None;
        let (closed_tx, closed_rx) = mpsc::sync_channel(1);
        drop(closed_rx);
        *workbench.commands.borrow_mut() = closed_tx;

        workbench.clear_notice();
        workbench.action("settings");
        assert!(workbench.settings_open_id.get().is_none());
        assert_eq!(workbench.window.get_notice_tone().as_str(), "error");
        assert!(!workbench.window.get_notice().is_empty());

        workbench.clear_notice();
        workbench.action("queues");
        assert!(workbench.queues_open_id.get().is_none());
        assert_eq!(workbench.window.get_notice_tone().as_str(), "error");
        assert!(!workbench.window.get_notice().is_empty());

        let later_settings: CorePipeResponse = serde_json::from_value(serde_json::json!({
            "type": "settings",
            "request_id": 1000,
            "takeover_enabled": true,
            "takeover_minimum_bytes": 0,
            "legal_accepted": true,
            "speed_limit_enabled": false,
            "speed_limit_kib": 0,
            "download_dir": "D:/Downloads"
        }))
        .unwrap();
        workbench.response(later_settings);
        assert!(workbench.settings.borrow().is_none());
        assert!(workbench.queues.borrow().is_none());

        // 3. Matching Settings response opens SettingsWindow cleanly and tracks transactional status.
        let (live_tx, live_rx) = mpsc::sync_channel(8);
        *workbench.commands.borrow_mut() = live_tx;
        workbench.action("settings");
        let open_id = workbench.settings_open_id.get().unwrap();
        assert!(matches!(
            live_rx.try_recv().unwrap(),
            WorkbenchJob::Request(req) if req.request_id() == Some(open_id)
        ));
        let open_response: CorePipeResponse = serde_json::from_value(serde_json::json!({
            "type": "settings",
            "request_id": open_id,
            "takeover_enabled": true,
            "takeover_minimum_bytes": 0,
            "legal_accepted": true,
            "speed_limit_enabled": false,
            "speed_limit_kib": 0,
            "download_dir": "D:/Downloads"
        }))
        .unwrap();
        workbench.response(open_response);
        let settings_window = workbench
            .settings
            .borrow()
            .as_ref()
            .expect("matching Settings response opens SettingsWindow")
            .window
            .clone_strong();
        assert_eq!(settings_window.get_status().as_str(), "");

        // Toggling a checkbox stages unsaved status without enqueuing instant StoreSettings.
        settings_window.invoke_edited("speed_limit_enabled".into(), "true".into());
        assert!(
            live_rx.try_recv().is_err(),
            "checkbox edit must not immediately dispatch StoreSettings"
        );
        assert_eq!(settings_window.get_status_tone().as_str(), "info");
        assert_eq!(
            settings_window.get_status().as_str(),
            "有未保存的更改，点击「保存设置」生效"
        );

        // Reverting back to original value clears unsaved status.
        settings_window.invoke_edited("speed_limit_enabled".into(), "false".into());
        assert_eq!(settings_window.get_status().as_str(), "");

        // Site rule add + edit also surfaces the transactional unsaved status.
        settings_window.invoke_site_rule_action(0, "add".into());
        assert_eq!(settings_window.get_status_tone().as_str(), "info");
        assert_eq!(
            settings_window.get_status().as_str(),
            "有未保存的更改，点击「保存设置」生效"
        );
        settings_window.invoke_site_rule_edited(0, "host".into(), "example.com".into());
        assert_eq!(
            settings_window.get_status().as_str(),
            "有未保存的更改，点击「保存设置」生效"
        );

        let _ = settings_window.hide();
        workbench.window.hide().unwrap();
    }
}
