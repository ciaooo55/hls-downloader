use crate::{file_dialogs, native_window, CastWindow, WorkbenchWindow};
use hls_client::{
    CastDeviceInfo, CoreCommand, CoreEvent, CoreIpcClient, CorePipeRequest, CorePipeResponse,
    EventEnvelope, MediaPushRequest,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

enum Pending {
    Discover,
    Connect { push: String, device: String },
    Resolve { push: String, status: String },
    Restore,
    RestoreCurrent(String),
    Control,
    Preference,
    Capabilities,
}

#[derive(Clone)]
struct CastUiState {
    page: String,
    mode: String,
    source_kind: String,
    path: String,
    url: String,
    media_title: String,
    locked_source: bool,
    source_label: String,
    selected_device: i32,
    discovering: bool,
    busy: bool,
    notice: String,
    notice_tone: String,
    active: bool,
    session_label: String,
    session_status: String,
    session_url: String,
    can_pause: bool,
    can_play: bool,
    can_seek: bool,
    can_stop: bool,
    position: f32,
    duration: f32,
    time: String,
    browser_request: bool,
}

impl Default for CastUiState {
    fn default() -> Self {
        Self {
            page: "source".into(),
            mode: "cast".into(),
            source_kind: "url".into(),
            path: String::new(),
            url: String::new(),
            media_title: String::new(),
            locked_source: false,
            source_label: String::new(),
            selected_device: -1,
            discovering: false,
            busy: false,
            notice: String::new(),
            notice_tone: "warning".into(),
            active: false,
            session_label: String::new(),
            session_status: String::new(),
            session_url: String::new(),
            can_pause: false,
            can_play: false,
            can_seek: false,
            can_stop: false,
            position: 0.0,
            duration: 0.0,
            time: String::new(),
            browser_request: false,
        }
    }
}

pub struct CastTools {
    main: WorkbenchWindow,
    window: RefCell<Option<CastWindow>>,
    state: RefCell<CastUiState>,
    jobs: mpsc::SyncSender<CorePipeRequest>,
    id: Rc<dyn Fn() -> u64>,
    pending: RefCell<HashMap<u64, Pending>>,
    devices: RefCell<Vec<CastDeviceInfo>>,
    task: RefCell<String>,
    pushes: RefCell<VecDeque<MediaPushRequest>>,
    resolved: RefCell<VecDeque<String>>,
    current_push: RefCell<Option<MediaPushRequest>>,
    preferred: RefCell<String>,
    sequence: Cell<u64>,
    session: Cell<u64>,
    controls_supported: Cell<bool>,
    rescan_after_discover: Cell<bool>,
    next_poll: Cell<Instant>,
    refresh_when_idle: Cell<bool>,
    timer: slint::Timer,
}

impl CastTools {
    #[cfg(test)]
    pub(crate) fn has_allocated_window(&self) -> bool {
        self.window.borrow().is_some()
    }

    pub fn sync_appearance(&self) {
        if let Some(window) = self.window.borrow().as_ref() {
            native_window::inherit_appearance(window, &self.main);
        }
    }

    pub fn keeps_ui_alive(&self) -> bool {
        self.window
            .borrow()
            .as_ref()
            .is_some_and(|w| w.window().is_visible())
            || self.current_push.borrow().is_some()
            || !self.pushes.borrow().is_empty()
            || !self.pending.borrow().is_empty()
            || self.refresh_when_idle.get()
    }

    pub fn new(
        main: WorkbenchWindow,
        id: Rc<dyn Fn() -> u64>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        // Discovery and receiver requests can wait for the network. A separate
        // bounded connection keeps download and local-player controls usable.
        let (jobs, receiver) = mpsc::sync_channel::<CorePipeRequest>(4);
        let weak = main.as_weak();
        std::thread::spawn(move || {
            let mut client = None;
            let mut last_request = Instant::now();
            while let Ok(request) = receiver.recv() {
                if last_request.elapsed() >= Duration::from_secs(60) {
                    client = None;
                }
                let request_id = request.request_id();
                let result = (|| {
                    let active = match client.as_mut() {
                        Some(existing) => existing,
                        None => client.insert(CoreIpcClient::connect()?),
                    };
                    active.request(&request)
                })();
                last_request = Instant::now();
                if result.is_err() {
                    client = None;
                }
                let response = result.unwrap_or_else(|message| CorePipeResponse::Error {
                    request_id,
                    code: "cast_connection".into(),
                    message,
                });
                if let Ok(json) = serde_json::to_string(&response) {
                    if weak
                        .upgrade_in_event_loop(move |ui| ui.invoke_response_json(json.into()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self::with_sender(main, jobs, id, true)
    }

    fn with_sender(
        main: WorkbenchWindow,
        jobs: mpsc::SyncSender<CorePipeRequest>,
        id: Rc<dyn Fn() -> u64>,
        bootstrap: bool,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let this = Rc::new(Self {
            main,
            window: RefCell::new(None),
            state: RefCell::new(CastUiState::default()),
            jobs,
            id,
            pending: RefCell::new(HashMap::new()),
            devices: RefCell::new(Vec::new()),
            task: RefCell::new(String::new()),
            pushes: RefCell::new(VecDeque::new()),
            resolved: RefCell::new(VecDeque::new()),
            current_push: RefCell::new(None),
            preferred: RefCell::new(String::new()),
            sequence: Cell::new(0),
            session: Cell::new(0),
            controls_supported: Cell::new(false),
            rescan_after_discover: Cell::new(false),
            next_poll: Cell::new(Instant::now()),
            refresh_when_idle: Cell::new(false),
            timer: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&this);
        this.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(1500),
            move || {
                if let Some(this) = weak.upgrade() {
                    if this.refresh_when_idle.get() && this.pending.borrow().is_empty() {
                        this.refresh();
                    }
                    let should_poll = this
                        .window
                        .borrow()
                        .as_ref()
                        .is_some_and(|w| w.window().is_visible() && w.get_page() == "session")
                        && this.state.borrow().active
                        && !this.state.borrow().busy
                        && Instant::now() >= this.next_poll.get();
                    if should_poll {
                        this.control("status");
                    }
                }
            },
        );
        if bootstrap {
            this.restore(None);
            this.send(
                CorePipeRequest::Capabilities {
                    request_id: (this.id)(),
                },
                Pending::Capabilities,
            );
            this.control("status");
        }
        Ok(this)
    }

    fn ensure_window(self: &Rc<Self>) -> Result<CastWindow, slint::PlatformError> {
        if let Some(existing) = self.window.borrow().as_ref() {
            return Ok(existing.clone_strong());
        }
        let window = CastWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(720.0, 610.0));
        native_window::inherit_appearance(&window, &self.main);
        self.apply_state_to_window(&window);
        let weak = Rc::downgrade(self);
        window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                this.action(&action);
            }
        });
        let weak = Rc::downgrade(self);
        window.on_seek(move |seconds| {
            if let Some(this) = weak.upgrade() {
                this.control(&format!("seek_to:{}", seconds.max(0.0) as u64));
            }
        });
        let weak = Rc::downgrade(self);
        window.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                this.action("close");
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        *self.window.borrow_mut() = Some(window.clone_strong());
        Ok(window)
    }

    fn pull_window_inputs(&self) {
        if let Some(window) = self.window.borrow().as_ref() {
            let mut st = self.state.borrow_mut();
            st.page = window.get_page().to_string();
            st.mode = window.get_mode().to_string();
            st.source_kind = window.get_source_kind().to_string();
            st.path = window.get_path().to_string();
            st.url = window.get_url().to_string();
            st.media_title = window.get_media_title().to_string();
            st.selected_device = window.get_selected_device();
        }
    }

    fn apply_state_to_window(&self, window: &CastWindow) {
        let st = self.state.borrow();
        window.set_page(st.page.clone().into());
        window.set_mode(st.mode.clone().into());
        window.set_source_kind(st.source_kind.clone().into());
        window.set_path(st.path.clone().into());
        window.set_url(st.url.clone().into());
        window.set_media_title(st.media_title.clone().into());
        window.set_locked_source(st.locked_source);
        window.set_source_label(st.source_label.clone().into());
        window.set_devices(ModelRc::new(VecModel::from(
            self.devices
                .borrow()
                .iter()
                .map(|d| format!("{} · {}", d.label, d.service_type).into())
                .collect::<Vec<_>>(),
        )));
        window.set_selected_device(st.selected_device);
        window.set_discovering(st.discovering);
        window.set_busy(st.busy);
        window.set_notice(st.notice.clone().into());
        window.set_notice_tone(st.notice_tone.clone().into());
        window.set_active(st.active);
        window.set_session_label(st.session_label.clone().into());
        window.set_session_status(st.session_status.clone().into());
        window.set_session_url(st.session_url.clone().into());
        window.set_can_pause(st.can_pause);
        window.set_can_play(st.can_play);
        window.set_can_seek(st.can_seek);
        window.set_can_stop(st.can_stop);
        window.set_position(st.position);
        window.set_duration(st.duration);
        window.set_time(st.time.clone().into());
        window.set_browser_request(st.browser_request);
    }

    fn update_state(&self, f: impl FnOnce(&mut CastUiState)) {
        self.pull_window_inputs();
        {
            let mut st = self.state.borrow_mut();
            f(&mut st);
        }
        if let Some(window) = self.window.borrow().as_ref() {
            self.apply_state_to_window(window);
        }
    }

    fn set_notice(&self, tone: &str, text: impl Into<String>) {
        let text = text.into();
        self.update_state(|st| {
            st.notice_tone = tone.to_owned();
            st.notice = text;
        });
    }

    fn send(&self, request: CorePipeRequest, kind: Pending) -> bool {
        let id = request.request_id().unwrap_or(0);
        if self.jobs.try_send(request).is_err() {
            self.set_notice("error", "操作队列已满，请稍后重试");
            return false;
        }
        self.pending.borrow_mut().insert(id, kind);
        true
    }

    fn command(&self, command: CoreCommand, kind: Pending) -> bool {
        self.send(
            CorePipeRequest::Command {
                request_id: (self.id)(),
                command,
            },
            kind,
        )
    }

    fn restore(&self, after: Option<[i64; 2]>) {
        self.send(
            CorePipeRequest::LoadHandoffs {
                request_id: (self.id)(),
                handoff_id: None,
                client_request_id: None,
                pending_only: true,
                paged: true,
                after,
            },
            Pending::Restore,
        );
    }

    fn restore_current(&self) {
        let Some(push) = self
            .current_push
            .borrow()
            .as_ref()
            .map(|push| push.id.clone())
        else {
            return;
        };
        self.send(
            CorePipeRequest::LoadHandoffs {
                request_id: (self.id)(),
                handoff_id: Some(push.clone()),
                client_request_id: None,
                pending_only: false,
                paged: true,
                after: None,
            },
            Pending::RestoreCurrent(push),
        );
    }

    pub fn show(self: &Rc<Self>, mode: &str, task: Option<(&str, &str)>) {
        self.pull_window_inputs();
        if self.state.borrow().busy || self.current_push.borrow().is_some() {
            self.show_window();
            return;
        }
        let was_discovering = self.state.borrow().discovering;
        let mode_changed = self.state.borrow().mode != mode;
        *self.task.borrow_mut() = task.map_or(String::new(), |(id, _)| id.to_owned());
        self.update_state(|st| {
            st.locked_source = task.is_some();
            st.source_label = task.map_or(String::new(), |(_, title)| title.to_owned());
            st.browser_request = false;
            st.mode = mode.to_owned();
            st.page = "source".into();
            st.notice.clear();
        });
        if !was_discovering || mode_changed {
            self.clear_devices();
        }
        self.show_window();
        if was_discovering {
            if mode_changed || task.is_some() {
                self.rescan_after_discover.set(true);
            }
        } else if task.is_some() {
            self.scan();
        }
    }

    fn show_window(self: &Rc<Self>) {
        let Ok(window) = self.ensure_window() else {
            return;
        };
        self.sync_appearance();
        self.apply_state_to_window(&window);
        let _ = window.show();
        native_window::prepare_dialog(&window, true, true, native_window::hwnd(self.main.window()));
    }

    fn clear_devices(&self) {
        self.devices.borrow_mut().clear();
        self.update_state(|st| {
            st.selected_device = -1;
        });
    }

    fn scan(&self) {
        self.pull_window_inputs();
        if self.state.borrow().busy || self.state.borrow().discovering {
            return;
        }
        self.clear_devices();
        let mode = self.state.borrow().mode.clone();
        if self.command(CoreCommand::DiscoverCastDevices { mode }, Pending::Discover) {
            self.update_state(|st| st.discovering = true);
        }
    }

    fn action(self: &Rc<Self>, action: &str) {
        self.pull_window_inputs();
        let busy = self.state.borrow().busy;
        let discovering = self.state.borrow().discovering;
        match action {
            "close" if !busy => {
                if self.current_push.borrow().is_some() {
                    self.resolve("canceled", "已取消设备选择", "");
                } else {
                    if let Some(window) = self.window.borrow().as_ref() {
                        let _ = window.hide();
                    }
                    self.next_push();
                }
            }
            "cancel_push" if !busy => self.resolve("canceled", "已取消设备选择", ""),
            "scan" => self.scan(),
            "mode" => self.clear_devices(),
            "file" if !busy => {
                let owner = self
                    .window
                    .borrow()
                    .as_ref()
                    .and_then(|w| native_window::hwnd(w.window()));
                match file_dialogs::files(owner, false) {
                    Ok(paths) => {
                        if let Some(path) = paths.first() {
                            let path = path.clone();
                            self.update_state(|st| st.path = path);
                        }
                    }
                    Err(error) => self.set_notice("error", error),
                }
            }
            "connect" | "publish" if !busy && !discovering => self.connect(action == "publish"),
            value if value.starts_with("control:") => self.control(&value[8..]),
            _ => {}
        }
    }

    fn connect(&self, publish: bool) {
        self.pull_window_inputs();
        let st_snapshot = self.state.borrow().clone();
        let device = if publish {
            String::new()
        } else {
            let Some(device) = self
                .devices
                .borrow()
                .get(st_snapshot.selected_device.max(0) as usize)
                .cloned()
            else {
                return;
            };
            device.id
        };
        let push = self
            .current_push
            .borrow()
            .as_ref()
            .map_or(String::new(), |p| p.id.clone());
        let command = if !self.task.borrow().is_empty() {
            CoreCommand::CastToDevice {
                task_id: self.task.borrow().clone(),
                device_id: device.clone(),
            }
        } else {
            let (path, url) = if let Some(push) = self.current_push.borrow().as_ref() {
                (String::new(), push.url.clone())
            } else if st_snapshot.source_kind == "file" {
                (st_snapshot.path.trim().to_owned(), String::new())
            } else {
                (String::new(), st_snapshot.url.trim().to_owned())
            };
            if path.is_empty() && url.is_empty() {
                self.set_notice("warning", "请选择媒体文件或输入媒体链接");
                return;
            }
            CoreCommand::ShareMedia {
                path,
                url,
                title: st_snapshot.media_title,
                device_id: device.clone(),
                request_id: push.clone(),
            }
        };
        if self.command(command, Pending::Connect { push, device }) {
            self.update_state(|st| {
                st.busy = true;
                st.notice_tone = "info".into();
                st.notice = "正在连接接收端…".into();
            });
        }
    }

    fn control(&self, action: &str) {
        self.pull_window_inputs();
        if self.state.borrow().busy
            || self
                .pending
                .borrow()
                .values()
                .any(|p| matches!(p, Pending::Control))
        {
            return;
        }
        if !self.state.borrow().active && action != "status" {
            return;
        }
        if action != "status" && !self.controls_supported.get() {
            self.set_notice(
                "warning",
                "下载引擎版本不匹配，暂不支持投屏控制，请重启或更新全部组件",
            );
            return;
        }
        let request = if action == "status" {
            CorePipeRequest::Command {
                request_id: (self.id)(),
                command: CoreCommand::ControlCast {
                    action: action.into(),
                },
            }
        } else {
            CorePipeRequest::CastControl {
                request_id: (self.id)(),
                session_generation: self.session.get(),
                action: action.into(),
            }
        };
        if self.send(request, Pending::Control) {
            self.next_poll.set(Instant::now() + Duration::from_secs(2));
            if action != "status" {
                self.update_state(|st| st.busy = true);
            }
        }
    }

    fn resolve(&self, status: &str, message: &str, location: &str) {
        let Some(push) = self.current_push.borrow().clone() else {
            return;
        };
        if self.command(
            CoreCommand::ResolveMediaPush {
                request_id: push.id.clone(),
                status: status.into(),
                message: message.into(),
                location: location.into(),
            },
            Pending::Resolve {
                push: push.id,
                status: status.into(),
            },
        ) {
            self.update_state(|st| st.busy = true);
        }
    }

    fn offer(self: &Rc<Self>, request: MediaPushRequest) {
        if request.status != "pending"
            || self.resolved.borrow().contains(&request.id)
            || self
                .current_push
                .borrow()
                .as_ref()
                .is_some_and(|p| p.id == request.id)
            || self.pushes.borrow().iter().any(|p| p.id == request.id)
        {
            return;
        }
        if self.pushes.borrow().len() < 256 {
            self.pushes.borrow_mut().push_back(request);
        }
        self.next_push();
    }

    fn remember_resolved(&self, id: &str) {
        let mut resolved = self.resolved.borrow_mut();
        if !resolved.iter().any(|value| value == id) {
            resolved.push_back(id.into());
            if resolved.len() > 256 {
                resolved.pop_front();
            }
        }
        self.pushes.borrow_mut().retain(|push| push.id != id);
    }

    fn next_push(self: &Rc<Self>) {
        self.pull_window_inputs();
        if self.current_push.borrow().is_some()
            || self.state.borrow().busy
            || self.state.borrow().discovering
        {
            return;
        }
        if self
            .window
            .borrow()
            .as_ref()
            .is_some_and(|w| w.window().is_visible() && w.get_page() == "source")
        {
            return;
        }
        let Some(request) = self.pushes.borrow_mut().pop_front() else {
            return;
        };
        self.task.borrow_mut().clear();
        let mode = if request.push_kind == "tvbox" {
            "tvbox"
        } else {
            "cast"
        }
        .to_owned();
        let title = request.title.clone();
        self.update_state(|st| {
            st.locked_source = true;
            st.browser_request = true;
            st.source_label = title.clone();
            st.media_title = title;
            st.mode = mode;
            st.page = "source".into();
            st.notice.clear();
        });
        *self.current_push.borrow_mut() = Some(request);
        self.show_window();
        self.scan();
    }

    pub fn event(self: &Rc<Self>, envelope: &EventEnvelope) {
        match &envelope.event {
            CoreEvent::MediaPushRequested { request } => self.offer(request.clone()),
            CoreEvent::MediaPushResolved { request } => {
                if request.status == "pending" {
                    return;
                }
                self.remember_resolved(&request.id);
                if self
                    .current_push
                    .borrow()
                    .as_ref()
                    .is_some_and(|p| p.id == request.id)
                    && request.status != "pending"
                {
                    self.current_push.borrow_mut().take();
                    if request.status != "done" {
                        if let Some(window) = self.window.borrow().as_ref() {
                            let _ = window.hide();
                        }
                    }
                    let tone = if request.status == "done" {
                        "success"
                    } else {
                        "warning"
                    };
                    self.update_state(|st| {
                        st.browser_request = false;
                        st.busy = false;
                        st.notice_tone = tone.into();
                        st.notice = request.message.clone();
                    });
                    self.next_push();
                }
            }
            CoreEvent::CastSession {
                active,
                session_generation,
                title,
                device,
                status,
                media_url,
                supported_actions,
                position_seconds,
                duration_seconds,
                position_available,
                ..
            } => {
                if envelope.sequence != 0 && envelope.sequence <= self.sequence.get() {
                    return;
                }
                self.sequence.set(envelope.sequence);
                let generation_changed =
                    self.session.replace(*session_generation) != *session_generation;
                self.update_state(|st| {
                    if generation_changed {
                        st.session_label = device.clone();
                        st.session_url.clear();
                    }
                    st.active = *active;
                    if !title.is_empty() {
                        st.session_label = format!("{title} · {device}");
                    }
                    st.session_status = status.clone();
                    if !media_url.is_empty() || !active {
                        st.session_url = media_url.clone();
                    }
                    st.can_play = supported_actions.iter().any(|s| s == "play");
                    st.can_pause = supported_actions.iter().any(|s| s == "pause");
                    st.can_seek =
                        *position_available && supported_actions.iter().any(|s| s == "seek_to");
                    st.can_stop = *active;
                    st.position = *position_seconds as f32;
                    st.duration = *duration_seconds as f32;
                    st.time = if *position_available {
                        format!(
                            "{} / {}",
                            clock(*position_seconds),
                            clock(*duration_seconds)
                        )
                    } else {
                        String::new()
                    };
                });
            }
            _ => {}
        }
    }

    pub fn response(self: &Rc<Self>, response: &CorePipeResponse) {
        if let CorePipeResponse::Settings {
            preferred_cast_device_id,
            ..
        } = response
        {
            *self.preferred.borrow_mut() = preferred_cast_device_id.clone();
        }
        let id = match response {
            CorePipeResponse::Events { request_id, .. }
            | CorePipeResponse::Handoffs { request_id, .. }
            | CorePipeResponse::Capabilities { request_id, .. } => Some(*request_id),
            CorePipeResponse::Error { request_id, .. } => *request_id,
            _ => None,
        };
        let Some(kind) = id.and_then(|id| self.pending.borrow_mut().remove(&id)) else {
            return;
        };
        let error = match response {
            CorePipeResponse::Error { message, .. } => Some(message.clone()),
            CorePipeResponse::Events { events, .. } => events.iter().find_map(|e| {
                if let CoreEvent::Error { message, .. } = &e.event {
                    Some(message.clone())
                } else {
                    None
                }
            }),
            _ => None,
        };
        if let CorePipeResponse::Events { events, .. } = response {
            for event in events {
                self.event(event);
            }
        }
        if let Some(error) = error {
            let message = if matches!(response, CorePipeResponse::Error { code, .. } if code == "cast_connection")
                && matches!(kind, Pending::Connect { .. })
            {
                format!("未能确认投送结果：{error}。请先核对当前会话和接收端，勿重复投送。")
            } else {
                error
            };
            self.update_state(|st| {
                if matches!(kind, Pending::Discover) {
                    st.discovering = false;
                } else {
                    st.busy = false;
                }
                st.notice_tone = "error".into();
                st.notice = message;
            });
            if matches!(kind, Pending::Discover) && self.rescan_after_discover.replace(false) {
                self.scan();
                return;
            }
            let push_gone = matches!(
                response,
                CorePipeResponse::Error { code, .. }
                    if matches!(code.as_str(), "media_push_not_found" | "media_push_resolved")
            ) || matches!(
                response,
                CorePipeResponse::Events { events, .. }
                    if events.iter().any(|item| matches!(
                        &item.event,
                        CoreEvent::Error { code, .. }
                            if matches!(code.as_str(), "media_push_not_found" | "media_push_resolved")
                    ))
            );
            if push_gone {
                if let Pending::Resolve { push, status } = &kind {
                    self.remember_resolved(push);
                    if self
                        .current_push
                        .borrow()
                        .as_ref()
                        .is_some_and(|p| p.id == *push)
                    {
                        self.current_push.borrow_mut().take();
                        self.update_state(|st| {
                            st.browser_request = false;
                            st.busy = false;
                        });
                        if status != "done" {
                            if let Some(window) = self.window.borrow().as_ref() {
                                let _ = window.hide();
                            }
                        }
                    }
                    self.next_push();
                    return;
                }
            }
            if matches!(response, CorePipeResponse::Error { code, .. } if code == "cast_connection")
                && matches!(kind, Pending::Connect { .. } | Pending::Resolve { .. })
            {
                self.restore_current();
                self.control("status");
            }
            return;
        }
        match kind {
            Pending::Discover => {
                self.update_state(|st| st.discovering = false);
                if self.rescan_after_discover.replace(false) {
                    self.scan();
                    return;
                }
                if let CorePipeResponse::Events { events, .. } = response {
                    if let Some(devices) = events.iter().find_map(|e| {
                        if let CoreEvent::CastDevices { devices } = &e.event {
                            Some(devices)
                        } else {
                            None
                        }
                    }) {
                        *self.devices.borrow_mut() = devices.clone();
                        let selected = devices
                            .iter()
                            .position(|d| d.id == *self.preferred.borrow())
                            .map_or(if devices.is_empty() { -1 } else { 0 }, |n| n as i32);
                        self.update_state(|st| st.selected_device = selected);
                    }
                }
                self.next_push();
            }
            Pending::Connect { push, device } => {
                self.update_state(|st| {
                    st.busy = false;
                    st.page = "session".into();
                    st.notice_tone = "success".into();
                    st.notice = "已提交到接收端".into();
                });
                if !device.is_empty() {
                    self.command(
                        CoreCommand::SetSetting {
                            key: "preferred_cast_device_id".into(),
                            value: device.into(),
                        },
                        Pending::Preference,
                    );
                }
                if !push.is_empty() {
                    let session_url = self.state.borrow().session_url.clone();
                    self.resolve("done", "媒体已投送", &session_url);
                }
            }
            Pending::Resolve { push, status } => {
                // A command reply confirms durable resolution even if its
                // matching event was already consumed by the event connection.
                self.remember_resolved(&push);
                if self
                    .current_push
                    .borrow()
                    .as_ref()
                    .is_some_and(|p| p.id == push)
                {
                    self.current_push.borrow_mut().take();
                    self.update_state(|st| {
                        st.browser_request = false;
                        st.busy = false;
                    });
                    if status != "done" {
                        if let Some(window) = self.window.borrow().as_ref() {
                            let _ = window.hide();
                        }
                    }
                }
                self.next_push();
            }
            Pending::Restore => {
                if let CorePipeResponse::Handoffs { items, next, .. } = response {
                    for item in items {
                        if let Ok(push) = serde_json::from_str::<MediaPushRequest>(item) {
                            self.offer(push);
                        }
                    }
                    if next.is_some() && self.pushes.borrow().len() < 256 {
                        self.restore(*next);
                    }
                }
            }
            Pending::RestoreCurrent(id) => {
                if let CorePipeResponse::Handoffs { items, .. } = response {
                    if let Some(push) = items
                        .iter()
                        .filter_map(|item| serde_json::from_str::<MediaPushRequest>(item).ok())
                        .find(|push| push.id == id)
                    {
                        if push.status != "pending" {
                            self.event(&EventEnvelope {
                                sequence: 0,
                                event: CoreEvent::MediaPushResolved { request: push },
                            });
                        }
                    } else if self
                        .current_push
                        .borrow()
                        .as_ref()
                        .is_some_and(|push| push.id == id)
                    {
                        self.remember_resolved(&id);
                        self.current_push.borrow_mut().take();
                        self.update_state(|st| {
                            st.browser_request = false;
                            st.busy = false;
                            st.notice_tone = "warning".into();
                            st.notice = "未找到这次浏览器推送的保存记录，请核对接收端状态后关闭窗口；不会自动重复投送。".into();
                        });
                        self.next_push();
                    }
                }
            }
            Pending::Control => self.update_state(|st| st.busy = false),
            Pending::Preference => {}
            Pending::Capabilities => {
                if let CorePipeResponse::Capabilities { commands, .. } = response {
                    self.controls_supported
                        .set(commands.iter().any(|s| s == "cast_control_generation"));
                }
            }
        }
    }

    pub fn resynchronize(&self) {
        // Let in-flight replies drain before queuing the refresh. Clearing the
        // map does not drain the bounded worker queue and can lose these reads.
        self.sequence.set(0);
        self.controls_supported.set(false);
        self.rescan_after_discover.set(false);
        self.update_state(|st| st.active = false);
        self.refresh_when_idle.set(true);
        if self.pending.borrow().is_empty() {
            self.refresh();
        }
    }

    fn refresh(&self) {
        self.refresh_when_idle.set(false);
        self.rescan_after_discover.set(false);
        self.update_state(|st| {
            st.busy = false;
            st.discovering = false;
        });
        self.restore_current();
        self.restore(None);
        self.send(
            CorePipeRequest::Capabilities {
                request_id: (self.id)(),
            },
            Pending::Capabilities,
        );
        self.control("status");
    }
}

fn clock(seconds: u64) -> String {
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_window_is_lazy_and_show_during_discover_updates_mode_and_task() {
        crate::offscreen::OffscreenPlatform::init_test();
        let (tx, rx) = mpsc::sync_channel::<CorePipeRequest>(8);
        let next = Cell::new(0u64);
        let cast = CastTools::with_sender(
            WorkbenchWindow::new().unwrap(),
            tx,
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
            false,
        )
        .unwrap();

        // Lazy initialization: no CastWindow allocated until show() is called.
        assert!(cast.window.borrow().is_none());

        // First show with task-1 in DLNA/Cast mode starts discovery (req #1).
        cast.show("cast", Some(("task-1", "lecture.mp4")));
        assert!(cast.window.borrow().is_some());
        assert!(cast.state.borrow().discovering);
        assert_eq!(cast.state.borrow().mode, "cast");
        assert_eq!(cast.task.borrow().as_str(), "task-1");
        let first_req = rx.try_recv().expect("discover request sent");
        assert!(matches!(
            first_req,
            CorePipeRequest::Command {
                request_id: 1,
                command: CoreCommand::DiscoverCastDevices { ref mode }
            } if mode == "cast"
        ));

        // Calling show("tvbox", Some(("task-2", "episode.mp4"))) while discovering is true
        // must update mode and task immediately and queue a follow-up tvbox scan!
        cast.show("tvbox", Some(("task-2", "episode.mp4")));
        assert_eq!(cast.state.borrow().mode, "tvbox");
        assert_eq!(cast.task.borrow().as_str(), "task-2");
        assert_eq!(cast.state.borrow().source_label, "episode.mp4");
        assert!(cast.state.borrow().locked_source);
        let win = cast.window.borrow().as_ref().unwrap().clone_strong();
        assert_eq!(win.get_mode().as_str(), "tvbox");
        assert_eq!(win.get_source_label().as_str(), "episode.mp4");

        // Completing stale discover #1 triggers fresh discover #2 for "tvbox".
        cast.response(&CorePipeResponse::Events {
            request_id: 1,
            events: vec![EventEnvelope {
                sequence: 1,
                event: CoreEvent::CastDevices {
                    devices: vec![CastDeviceInfo {
                        id: "dlna-1".into(),
                        label: "Living Room TV".into(),
                        service_type: "DLNA".into(),
                        location: String::new(),
                        control_url: String::new(),
                    }],
                },
            }],
        });
        assert!(cast.state.borrow().discovering);
        let second_req = rx.try_recv().expect("follow-up tvbox discover sent");
        assert!(matches!(
            second_req,
            CorePipeRequest::Command {
                request_id: 2,
                command: CoreCommand::DiscoverCastDevices { ref mode }
            } if mode == "tvbox"
        ));
        let _ = win.hide();
    }
}
