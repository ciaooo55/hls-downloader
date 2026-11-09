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
    commands: mpsc::SyncSender<WorkbenchJob>,
    next_id: Cell<u64>,
    editor: RefCell<Option<TaskEditorWindow>>,
    editor_id: Cell<u64>,
    editor_variants: RefCell<Vec<StreamVariant>>,
    details: RefCell<Option<(String, DetailsWindow)>>,
    settings: RefCell<Option<SettingsEditor>>,
    settings_response: RefCell<Option<CorePipeResponse>>,
    show_settings: Cell<bool>,
    settings_save_id: Cell<Option<u64>>,
    queues: RefCell<Option<QueueEditor>>,
    pending_queues: Cell<bool>,
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
    status_timer: slint::Timer,
}

impl Workbench {
    pub fn new(
        tasks: Vec<TaskSnapshot>,
        sequence: u64,
        dark: bool,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let window = WorkbenchWindow::new()?;
        crate::native_window::apply_theme(&window, dark);
        let state = Rc::new(RefCell::new(TaskList::default()));
        state.borrow_mut().replace(tasks, sequence);
        let rows = Rc::new(VecModel::default());
        window.set_tasks(ModelRc::from(rows.clone()));
        let navigation = Rc::new(VecModel::default());
        window.set_navigation(ModelRc::from(navigation.clone()));
        let (commands, jobs) = mpsc::sync_channel::<WorkbenchJob>(32);
        let weak = window.as_weak();
        std::thread::spawn(move || {
            let mut client = None;
            let mut last_request = std::time::Instant::now();
            while let Ok(job) = jobs.recv() {
                // Core closes idle command pipes after 120 s. Reconnect before sending,
                // rather than retrying a mutation after its receipt becomes uncertain.
                if last_request.elapsed() >= std::time::Duration::from_secs(60) {
                    client = None;
                }
                let failure_id = match &job {
                    WorkbenchJob::Snapshot(id) | WorkbenchJob::Batch(id, _) => Some(*id),
                    WorkbenchJob::Request(request) => {
                        serde_json::to_value(request).ok().and_then(|value| {
                            value.get("request_id").and_then(serde_json::Value::as_u64)
                        })
                    }
                };
                let result = (|| {
                    if client.is_none() {
                        client = Some(CoreIpcClient::connect()?);
                    }
                    let client = client.as_mut().unwrap();
                    match job {
                        WorkbenchJob::Request(request) => client.request(&request),
                        WorkbenchJob::Snapshot(request_id) => {
                            client.snapshot_state().map(|(tasks, latest_sequence)| {
                                CorePipeResponse::Snapshot {
                                    request_id,
                                    tasks,
                                    latest_sequence,
                                    browser_connected: false,
                                }
                            })
                        }
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
                    continue;
                };
                if weak
                    .upgrade_in_event_loop(move |ui| ui.invoke_response_json(json.into()))
                    .is_err()
                {
                    break;
                }
            }
        });
        let controller = Rc::new(Self {
            window,
            state,
            rows,
            navigation,
            positions: RefCell::new(HashMap::new()),
            commands,
            next_id: Cell::new(1),
            editor: RefCell::new(None),
            editor_id: Cell::new(0),
            editor_variants: RefCell::new(Vec::new()),
            details: RefCell::new(None),
            settings: RefCell::new(None),
            settings_response: RefCell::new(None),
            show_settings: Cell::new(false),
            settings_save_id: Cell::new(None),
            exports: RefCell::new(HashMap::new()),
            queues: RefCell::new(None),
            pending_queues: Cell::new(false),
            queue_save: RefCell::new(None),
            tools: RefCell::new(None),
            media: RefCell::new(None),
            system: RefCell::new(None),
            service_settings: RefCell::new(None),
            cast: RefCell::new(None),
            status_timer: slint::Timer::default(),
            outstanding: RefCell::new(HashSet::new()),
            closing: Cell::new(false),
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
                request_owner
                    .upgrade()
                    .is_some_and(|this| this.request(request))
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
            controller
                .window
                .window()
                .on_winit_window_event(move |window, event| {
                    match event {
                        winit::event::WindowEvent::Resized(size) => {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_viewport_width(size.width as f32 / window.scale_factor());
                                ui.set_viewport_height(size.height as f32 / window.scale_factor());
                                ui.invoke_viewport_resized(
                                    size.width as f32 / window.scale_factor(),
                                );
                            }
                        }
                        winit::event::WindowEvent::DroppedFile(path) => {
                            if let Some(ui) = weak.upgrade() {
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
                    this.render_details();
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
        self.window.on_place_row(move |id, target| {
            if let Some(this) = weak.upgrade() {
                let state = this.state.borrow();
                if state.sort != "queue:asc" && state.sort != "queue" {
                    this.window
                        .set_notice("请先在工具中选择“队列顺序”，再拖动排序".into());
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
                    this.window
                        .set_notice("请使用“移动到队列”跨队列调整任务".into());
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
                    Err(error) => this
                        .window
                        .set_notice(format!("响应无法读取：{error}").into()),
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
        self.closing.set(false);
        let visible = self.window.window().is_visible();
        self.window.show()?;
        #[cfg(windows)]
        {
            let weak = self.window.as_weak();
            let _ = slint::spawn_local(async move {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                use slint::winit_030::WinitWindowAccessor;
                if ui.window().winit_window().await.is_err() {
                    return;
                }
                native_window::apply_theme(&ui, ui.global::<Tokens>().get_dark());
                if !visible {
                    native_window::restore_geometry(ui.window());
                }
                native_window::activate(ui.window());
            });
        }
        #[cfg(not(windows))]
        if !visible {
            native_window::restore_geometry(self.window.window());
        }
        Ok(())
    }

    fn close(&self) {
        native_window::save_geometry(self.window.window());
        if let Some(media) = self.media.borrow().as_ref() {
            media.main_closed();
        }
        let _ = self.window.hide();
        self.closing.set(true);
    }

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
    }

    pub fn apply(&self, event: &EventEnvelope) {
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
            tools.event(&event.event);
        }
        let view_revision = self.state.borrow().view_revision;
        let changed = self.state.borrow_mut().apply(event);
        if let Some(id) = changed {
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
            }
        }
        match &event.event {
            CoreEvent::Error { message, .. } | CoreEvent::Toast { message, .. } => {
                self.window.set_notice(message.clone().into())
            }
            CoreEvent::BrowserStatus { connected, .. } => self.window.set_browser_status(
                if *connected {
                    "浏览器已连接"
                } else {
                    "浏览器未连接"
                }
                .into(),
            ),
            CoreEvent::SettingsChanged { .. } => {
                self.request(CorePipeRequest::LoadSettings {
                    request_id: self.id(),
                });
            }
            CoreEvent::UiShow { surface } if surface == "main" => {
                let _ = self.show();
            }
            CoreEvent::UiShow { surface } if surface == "hide" => self.close(),
            _ => {}
        }
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
        self.render_status();
        self.render_menu();
    }

    fn table_viewport(&self) -> f32 {
        self.window.get_viewport_width()
            - self.window.get_sidebar_width()
            - self.window.get_content_inset() * 2.0
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
        drop(state);
        self.render_status();
        self.render_menu();
    }

    fn render_menu(&self) {
        let state = self.state.borrow();
        let selected: Vec<_> = state
            .selected
            .iter()
            .filter_map(|id| state.tasks.get(id))
            .collect();
        let entries = [
            ("详情与日志", "details", "details"),
            ("开始 / 继续", "start", "play"),
            ("暂停", "pause", "pause"),
            ("取消", "取消", "close"),
            ("重新下载", "task:retry", "refresh"),
            ("播放", "task:play", "play"),
            ("投屏", "task:cast", "cast"),
            ("TVBox 推送", "task:push_tvbox", "tvbox"),
            ("移动到队列", "移动到队列", "queue"),
            ("上移", "task:queue_up", "up"),
            ("下移", "task:queue_down", "down"),
            ("置顶", "task:queue_top", "up"),
            ("置底", "task:queue_bottom", "down"),
            ("打开文件", "task:open", "file"),
            ("打开所在位置", "task:open_folder", "folder"),
            ("复制文件", "task:copy_file", "paste"),
            ("按住拖出文件", "task:drag_file", "export"),
            ("删除任务", "delete", "delete"),
            ("删除任务和文件", "delete_files", "delete"),
        ];
        self.window.set_task_menu(ModelRc::new(VecModel::from(
            entries
                .iter()
                .map(|(label, action, icon)| {
                    let enabled = match *action {
                        "details" | "移动到队列" => !selected.is_empty(),
                        "start" => selected
                            .iter()
                            .any(|task| task_start_action(task).is_some()),
                        "pause" => selected.iter().any(|t| task_action_allowed(t, "pause")),
                        "取消" => selected.iter().any(|t| task_action_allowed(t, "cancel")),
                        action => selected.iter().any(|t| {
                            task_action_allowed(t, action.strip_prefix("task:").unwrap_or(action))
                        }),
                    };
                    TaskMenuRow {
                        label: (*label).into(),
                        action: (*action).into(),
                        icon: (*icon).into(),
                        enabled,
                    }
                })
                .collect::<Vec<_>>(),
        )));
    }

    fn render_status(&self) {
        let state = self.state.borrow();
        let mut counts = HashMap::<String, i32>::new();
        for task in state.tasks.values() {
            *counts.entry("all".into()).or_default() += 1;
            let status = match task.status.as_str() {
                "downloading" | "processing" | "merging" | "probing" => "running",
                "completed" | "done" => "completed",
                "canceled" | "failed" => "failed",
                status => status,
            };
            *counts.entry(status.into()).or_default() += 1;
            *counts
                .entry(format!(
                    "category:{}",
                    hls_client::download_category(&task.filename, &task.url, task.resource_kind)
                ))
                .or_default() += 1;
            *counts
                .entry(format!(
                    "queue:{}",
                    if task.queue_id.is_empty() {
                        "default"
                    } else {
                        &task.queue_id
                    }
                ))
                .or_default() += 1;
        }
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
            .map(|task| task.speed_bytes_per_sec)
            .sum();
        self.window.set_summary(
            format!(
                "{} 个任务 · {} /s",
                state.visible.len(),
                crate::format_bytes(speed)
            )
            .into(),
        );
        self.window.set_selected_count(state.selected.len() as i32);
        self.window.set_connection("下载引擎已连接".into());
    }

    fn id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        id
    }

    fn job(&self, job: WorkbenchJob) -> bool {
        let id = match &job {
            WorkbenchJob::Snapshot(id) | WorkbenchJob::Batch(id, _) => Some(*id),
            WorkbenchJob::Request(request) => serde_json::to_value(request)
                .ok()
                .and_then(|value| value["request_id"].as_u64()),
        };
        if self.commands.try_send(job).is_err() {
            self.window
                .set_notice("操作队列已满或连接关闭，请稍后重试".into());
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
            "drag_help" => self
                .window
                .set_notice("按住左键拖至目标文件夹；也可用 Ctrl+C 复制后粘贴".into()),
            "task:copy_file" | "task:drag_file" => {
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
                    if action == "task:copy_file" {
                        hls_client::file_transfer::write_files(&paths)
                            .map(|()| format!("已复制 {} 个文件，可粘贴到目标文件夹", paths.len()))
                    } else {
                        hls_client::file_transfer::begin_file_drag(&paths).map(|outcome| {
                            match outcome {
                                hls_client::file_transfer::DragOutcome::Dropped => {
                                    format!("已拖出 {} 个文件", paths.len())
                                }
                                hls_client::file_transfer::DragOutcome::Canceled => {
                                    "已取消拖出".into()
                                }
                            }
                        })
                    }
                });
                self.window
                    .set_notice(result.unwrap_or_else(|error| error).into());
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
            "delete" | "delete_files" | "删除任务" | "删除任务和文件" => {
                let files = matches!(action, "delete_files" | "删除任务和文件");
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
                    self.window.set_notice("请选择可删除的任务".into());
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
                if file_dialogs::confirm(
                    native_window::hwnd(self.window.window()),
                    if files {
                        "删除任务和文件"
                    } else {
                        "删除任务"
                    },
                    &message,
                ) {
                    self.job(WorkbenchJob::Batch(
                        self.id(),
                        ids.into_iter()
                            .map(|task_id| CoreCommand::TaskAction {
                                task_id,
                                action: action.into(),
                            })
                            .collect(),
                    ));
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
                let dark = !self.window.global::<Tokens>().get_dark();
                crate::native_window::apply_theme(&self.window, dark);
                self.send(CoreCommand::SetSetting {
                    key: "dark_mode".into(),
                    value: dark.into(),
                });
            }
            "start" | "pause" | "开始 / 继续" | "暂停" | "取消" => {
                let state = self.state.borrow();
                let mut commands = Vec::new();
                for task in state.toolbar_targets() {
                    let actions = &task.available_actions;
                    let target = match action {
                        "pause" | "暂停" => "pause",
                        "取消" => "cancel",
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
            "new" | "批量添加" | "导入 cURL" => self.open_editor(action),
            "queues" | "移动到队列" => self.open_queues(),
            "页面抓取" => self.open_resource_tools("harvest", String::new()),
            "抓站与同步项目" => self.open_resource_tools("crawl", String::new()),
            "ZIP 内容预览" => self.open_resource_tools("zip", String::new()),
            "details" | "详情与日志" => self.open_details(),
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
            "取消完成动作" => self.send(CoreCommand::CancelPowerAction),
            action if action.starts_with("focus_task:") => {
                self.state.borrow_mut().select(&action[11..], false, false);
                self.render_selection();
                let _ = self.show();
                self.open_details();
            }
            "settings" => {
                self.show_settings.set(true);
                self.request(CorePipeRequest::LoadSettings {
                    request_id: self.id(),
                });
            }
            "refresh" => {
                self.job(WorkbenchJob::Snapshot(self.id()));
            }
            "import" => match file_dialogs::files(native_window::hwnd(self.window.window()), false)
            {
                Ok(paths) if !paths.is_empty() => self.send(CoreCommand::ImportPaths { paths }),
                Err(error) => self.window.set_notice(error.into()),
                _ => {}
            },
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
                    Err(error) => self.window.set_notice(error.into()),
                    _ => {}
                }
            }
            "打开所在位置" => {
                for id in &self.state.borrow().selected {
                    self.send(CoreCommand::OpenCompleted {
                        task_id: id.clone(),
                        folder: true,
                    });
                }
            }
            _ => self
                .window
                .set_notice(format!("功能入口尚未完成迁移：{action}").into()),
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
                self.open_editor(action);
                if let Some(editor) = self.editor.borrow().as_ref() {
                    editor.set_url(text.trim().into());
                }
            }
            Ok(_) => self
                .window
                .set_notice("剪贴板中没有链接或可导入的文件".into()),
            Err(error) => self.window.set_notice(error.into()),
        }
    }

    fn response(self: &Rc<Self>, response: CorePipeResponse) {
        if let Some(id) = serde_json::to_value(&response)
            .ok()
            .and_then(|value| value["request_id"].as_u64())
        {
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
                                    editor.set_result(label.clone().into());
                                    editor.set_kind(kind_label(*resource_kind).into());
                                    *self.editor_variants.borrow_mut() = variants.clone();
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
                                    editor.set_result("任务已交给下载引擎".into())
                                }
                                CoreEvent::Error { message, .. } => {
                                    editor.set_result(message.clone().into())
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
                            Ok(()) => self
                                .window
                                .set_notice(format!("已导出：{}", path.display()).into()),
                            Err(error) => {
                                self.window.set_notice(format!("导出失败：{error}").into())
                            }
                        }
                    }
                }
                for event in events {
                    if let CoreEvent::TaskLog { task_id, lines } = event.event {
                        if let Some((id, window)) = self.details.borrow().as_ref() {
                            if *id == task_id {
                                window.set_logs(lines.join("\n").into());
                            }
                        }
                    }
                }
            }
            response @ CorePipeResponse::Settings { request_id, .. } => {
                if let CorePipeResponse::Settings {
                    task_sort,
                    dark_mode,
                    ..
                } = &response
                {
                    crate::native_window::apply_theme(&self.window, *dark_mode);
                    if let Some(editor) = self.editor.borrow().as_ref() {
                        crate::native_window::apply_theme(editor, *dark_mode);
                    }
                    if let Some((_, details)) = self.details.borrow().as_ref() {
                        crate::native_window::apply_theme(details, *dark_mode);
                    }
                    if let Some(queues) = self.queues.borrow().as_ref() {
                        crate::native_window::apply_theme(&queues.window, *dark_mode);
                    }
                    if let Some(tools) = self.tools.borrow().as_ref() {
                        crate::native_window::apply_theme(&tools.window, *dark_mode);
                    }
                    if let Some(service) = self.service_settings.borrow().as_ref() {
                        crate::native_window::apply_theme(&service.window, *dark_mode);
                    }
                    if let Some(cast) = self.cast.borrow().as_ref() {
                        cast.apply_theme(*dark_mode);
                    }
                    if let Some(system) = self.system.borrow().as_ref() {
                        system.apply_theme(*dark_mode);
                    }
                    if let Some(media) = self.media.borrow().as_ref() {
                        media.apply_theme(*dark_mode);
                    }
                    let changed = self.state.borrow().sort != *task_sort;
                    if changed {
                        self.state.borrow_mut().sort = task_sort.clone();
                        self.state.borrow_mut().rebuild();
                        self.render();
                    }
                }
                if self
                    .queue_save
                    .borrow()
                    .as_ref()
                    .is_some_and(|save| save.request_id == request_id)
                {
                    let save = self.queue_save.borrow_mut().take().unwrap();
                    if let Some(editor) = self.queues.borrow().as_ref() {
                        editor.window.set_busy(false);
                        editor.window.set_status("队列已保存".into());
                    }
                    if let Some((task_ids, queue_id)) = save.assignment {
                        self.send(CoreCommand::AssignQueue { task_ids, queue_id });
                    }
                }
                let settings_saved = self.settings_save_id.get() == Some(request_id);
                if let Some(editor) = self.settings.borrow_mut().as_mut() {
                    editor.receive_settings(&response, settings_saved);
                }
                if settings_saved {
                    self.settings_save_id.set(None);
                    if let Some(editor) = self.settings.borrow_mut().as_mut() {
                        editor.window.set_busy(false);
                        editor.window.set_status("设置已保存".into());
                    }
                }
                *self.settings_response.borrow_mut() = Some(response.clone());
                self.render_status();
                if self.show_settings.replace(false) {
                    self.open_settings(&response);
                }
                if self.pending_queues.replace(false) {
                    self.open_queues();
                }
            }
            CorePipeResponse::Snapshot {
                tasks,
                latest_sequence,
                ..
            } => self.replace(tasks, latest_sequence),
            CorePipeResponse::Error {
                request_id,
                message,
                ..
            } => {
                self.window.set_notice(message.clone().into());
                if request_id == Some(self.editor_id.get()) {
                    if let Some(editor) = self.editor.borrow().as_ref() {
                        editor.set_busy(false);
                        editor.set_result(message.clone().into());
                    }
                }
                if request_id.is_some() && request_id == self.settings_save_id.get() {
                    self.settings_save_id.set(None);
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.window.set_busy(false);
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
                        editor.window.set_status(self.window.get_notice());
                    }
                }
            }
            _ => {}
        }
    }

    fn open_editor(self: &Rc<Self>, action: &str) {
        let Ok(editor) = TaskEditorWindow::new() else {
            return;
        };
        crate::native_window::apply_theme(&editor, self.window.global::<Tokens>().get_dark());
        editor.set_mode(
            match action {
                "批量添加" => "batch",
                "导入 cURL" => "curl",
                _ => "new",
            }
            .into(),
        );
        if let Some(CorePipeResponse::Settings {
            download_dir,
            legal_accepted,
            ..
        }) = self.settings_response.borrow().as_ref()
        {
            editor.set_download_dir(download_dir.clone().into());
            editor.set_legal_accepted(*legal_accepted);
        }
        self.editor_id.set(self.id());
        self.editor_variants.borrow_mut().clear();
        let weak = Rc::downgrade(self);
        editor.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                this.editor_action(action.as_str());
            }
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
            }
            "folder" => match file_dialogs::folder(native_window::hwnd(editor.window())) {
                Ok(Some(path)) => editor.set_download_dir(path.into()),
                Err(error) => editor.set_result(error.into()),
                _ => {}
            },
            "probe" | "create" => {
                let result = editor_spec(editor, &self.editor_variants.borrow());
                let spec = match result {
                    Ok(spec) => spec,
                    Err(error) => {
                        editor.set_result(error.into());
                        return;
                    }
                };
                let request_id = self.editor_id.get();
                let submitted = if action == "probe" {
                    self.request(CorePipeRequest::Command {
                        request_id,
                        command: CoreCommand::ProbeUrl {
                            url: spec.url.clone(),
                            spec: Some(spec),
                        },
                    })
                } else {
                    self.send(CoreCommand::SetSetting {
                        key: "legal_terms_accepted".into(),
                        value: true.into(),
                    });
                    let commands = match editor.get_mode().as_str() {
                        "curl" => vec![CoreCommand::ImportCurl {
                            command: editor.get_url().to_string(),
                            options: spec,
                        }],
                        "batch" => {
                            let urls: Vec<_> = editor
                                .get_url()
                                .lines()
                                .map(str::trim)
                                .filter(|line| !line.is_empty())
                                .map(str::to_owned)
                                .collect();
                            if urls.len() > 1000 {
                                editor.set_result("每批最多 1000 个地址".into());
                                return;
                            }
                            urls.into_iter()
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
                    self.job(WorkbenchJob::Batch(request_id, commands))
                };
                editor.set_busy(submitted);
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
            self.window.set_notice("请选择任务".into());
            return;
        };
        let Ok(window) = DetailsWindow::new() else {
            return;
        };
        crate::native_window::apply_theme(&window, self.window.global::<Tokens>().get_dark());
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
                    "refresh_url" => this.send(CoreCommand::RefreshTaskRequest {
                        task_id: id.clone(),
                        url: window.get_url().into(),
                        cookie: window.get_cookie().into(),
                        auto_resume: true,
                    }),
                    "speed" => this.send(CoreCommand::TaskAction {
                        task_id: id.clone(),
                        action: format!("speed:{}", window.get_speed_limit()),
                    }),
                    "logs" => this.send(CoreCommand::GetTaskLog {
                        task_id: id.clone(),
                    }),
                    "folder" => this.send(CoreCommand::OpenCompleted {
                        task_id: id.clone(),
                        folder: true,
                    }),
                    "profile" => this.send(CoreCommand::SaveSiteProfile {
                        task_id: id.clone(),
                    }),
                    "torrent" => this.open_resource_tools("torrent", id.clone()),
                    _ => {}
                }
            }
        });
        *self.details.borrow_mut() = Some((id.clone(), window));
        if let Some((_, window)) = self.details.borrow().as_ref() {
            if let Some(task) = self.state.borrow().tasks.get(&id) {
                window.set_url(task.url.clone().into());
                window.set_speed_limit(task.speed_limit_kib.min(i32::MAX as u32) as i32);
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
        window.set_filename(task.filename.clone().into());
        window.set_status(crate::task_status_label(&task.status).into());
        window.set_transfer(
            format!(
                "{} / {} · {} /s",
                crate::format_bytes(task.downloaded_bytes),
                task.total_bytes
                    .map(crate::format_bytes)
                    .unwrap_or_else(|| "未知".into()),
                crate::format_bytes(task.speed_bytes_per_sec)
            )
            .into(),
        );
        window.set_connections(
            format!(
                "{} 个连接 · {}/{} 分段\n{}",
                task.active_workers,
                task.completed_ranges,
                task.total_ranges,
                task.connection_parts
                    .iter()
                    .map(|part| format!("{:?}", part))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
            .into(),
        );
        window.set_diagnostic(
            [
                task.error_message.clone().unwrap_or_default(),
                task.error_hint.clone(),
            ]
            .join(" ")
            .trim()
            .into(),
        );
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
        if !task.log_tail.is_empty() {
            window.set_logs(task.log_tail.join("\n").into());
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
            request_owner
                .upgrade()
                .is_some_and(|this| this.request(request))
        });
        let batch = Rc::new(move |commands| {
            batch_owner
                .upgrade()
                .is_some_and(|this| this.job(WorkbenchJob::Batch(this.id(), commands)))
        });
        let id = Rc::new(move || id_owner.upgrade().map_or(0, |this| this.id()));
        match ResourceTools::new(
            mode,
            task_id,
            &directory,
            self.window.global::<Tokens>().get_dark(),
            request,
            batch,
            id,
        ) {
            Ok(tools) => {
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
            Err(error) => self.window.set_notice(error.to_string().into()),
        }
    }

    fn open_queues(self: &Rc<Self>) {
        let profiles = match self.settings_response.borrow().as_ref() {
            Some(CorePipeResponse::Settings { queue_profiles, .. }) => queue_profiles.clone(),
            _ => {
                self.pending_queues.set(true);
                self.request(CorePipeRequest::LoadSettings {
                    request_id: self.id(),
                });
                return;
            }
        };
        let Ok(editor) = QueueEditor::new(
            profiles,
            self.window.global::<Tokens>().get_dark(),
            !self.state.borrow().selected.is_empty(),
        ) else {
            return;
        };
        let weak = Rc::downgrade(self);
        editor.window.on_select_profile(move |index| {
            if let Some(this) = weak.upgrade() {
                if let Some(editor) = this.queues.borrow_mut().as_mut() {
                    if let Err(error) = editor.select(index.max(0) as usize) {
                        editor.window.set_status(error.into());
                    }
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
                        let id = this.id();
                        let assignment = (action == "assign").then(|| {
                            (
                                this.state.borrow().selected.iter().cloned().collect(),
                                queue_id,
                            )
                        });
                        let values = std::collections::BTreeMap::from([(
                            "queue_profiles".to_owned(),
                            serde_json::to_value(profiles).unwrap(),
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
                        }
                    }
                    Err(error) => editor.window.set_status(error.into()),
                }
            } else if let Err(error) = editor.edit(&action) {
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
        let Ok(editor) = SettingsEditor::new(response, self.window.global::<Tokens>().get_dark())
        else {
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
                        editor.window.set_status(error.into());
                    } else {
                        editor.window.set_status("".into());
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_site_rule_edited(move |index, key, value| {
            if let Some(this) = weak.upgrade() {
                if let Some(editor) = this.settings.borrow_mut().as_mut() {
                    editor.edit_rule(index, &key, &value);
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_site_rule_action(move |index, action| {
            if let Some(this) = weak.upgrade() {
                let mut settings = this.settings.borrow_mut();
                let Some(editor) = settings.as_mut() else {
                    return;
                };
                let result = match action.as_str() {
                    "folder" => file_dialogs::folder(native_window::hwnd(editor.window.window()))
                        .map(|path| {
                            if let Some(path) = path {
                                editor.edit_rule(index, "directory", &path);
                            }
                        }),
                    "credentials" => editor
                        .credential_host(index)
                        .map(|host| this.open_service_settings("credentials", &host)),
                    action => editor.rule_action(index, action),
                };
                if let Err(error) = result {
                    editor.window.set_status(error.into());
                }
            }
        });
        let weak = Rc::downgrade(self);
        editor.window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                let mut settings = this.settings.borrow_mut();
                let Some(editor) = settings.as_mut() else {
                    return;
                };
                match action.as_str() {
                    "close" => {
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
                                    editor.window.set_status("命令队列已满，请稍后重试".into());
                                }
                            }
                            Err(error) => editor.window.set_status(error.into()),
                        }
                    }
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
                        let key = &value[7..];
                        if let Ok(Some(path)) =
                            file_dialogs::folder(native_window::hwnd(editor.window.window()))
                        {
                            let _ = editor.edit(key, &path);
                            editor.render(&editor.window.get_selected_group());
                        }
                    }
                    _ => editor
                        .window
                        .set_status("该设置页面的专用编辑器尚在迁移中".into()),
                }
            }
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
            self.window.global::<Tokens>().get_dark(),
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
            Err(error) => self.window.set_notice(error.to_string().into()),
        }
    }
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
        _ => available(action),
    }
}

fn editor_spec(editor: &TaskEditorWindow, variants: &[StreamVariant]) -> Result<TaskSpec, String> {
    let url = editor.get_url().trim().to_owned();
    if url.is_empty() {
        return Err("请输入下载地址".into());
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
            return Err("请求头应为 Name: Value".into());
        };
        if name.trim().is_empty() {
            return Err("请求头名称不能为空".into());
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
        queue_id: editor.get_queue_id().trim().into(),
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
    let kind = hls_client::download_category(&task.filename, &task.url, task.resource_kind);
    let protocol: String = match task.resource_kind {
        ResourceKind::Hls => "HLS".into(),
        ResourceKind::Dash => "DASH".into(),
        ResourceKind::Live => "直播".into(),
        ResourceKind::Torrent => "BT".into(),
        _ => task
            .url
            .split_once(':')
            .map_or("HTTP".into(), |(scheme, _)| scheme.to_uppercase()),
    };
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| !extension.is_empty() && extension.len() <= 10);
    let metadata = extension.map_or_else(
        || protocol.to_string(),
        |extension| format!("{protocol} · {}", extension.to_uppercase()),
    );
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
        speed: if task.speed_bytes_per_sec > 0 {
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
        kind: if kind == "other" { "file" } else { kind }.into(),
        metadata: metadata.into(),
        status_code: task.status.clone().into(),
        primary_action: primary.map_or("", |(action, _)| action).into(),
        primary_label: primary.map_or("", |(_, label)| label).into(),
    }
}

#[cfg(test)]
mod file_selection_tests {
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
}
