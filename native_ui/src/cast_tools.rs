use crate::{file_dialogs, native_window, CastWindow, Tokens, WorkbenchWindow};
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

pub struct CastTools {
    main: WorkbenchWindow,
    window: CastWindow,
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
    next_poll: Cell<Instant>,
    refresh_when_idle: Cell<bool>,
    timer: slint::Timer,
}

impl CastTools {
    pub fn apply_theme(&self, dark: bool) {
        crate::native_window::apply_theme(&self.window, dark);
    }

    pub fn keeps_ui_alive(&self) -> bool {
        self.window.window().is_visible()
            || self.current_push.borrow().is_some()
            || !self.pushes.borrow().is_empty()
            || !self.pending.borrow().is_empty()
            || self.refresh_when_idle.get()
    }

    pub fn new(
        main: WorkbenchWindow,
        id: Rc<dyn Fn() -> u64>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let window = CastWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(720.0, 640.0));
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
                let request_id = serde_json::to_value(&request)
                    .ok()
                    .and_then(|v| v["request_id"].as_u64());
                let result = (|| {
                    if client.is_none() {
                        client = Some(CoreIpcClient::connect()?);
                    }
                    client.as_mut().unwrap().request(&request)
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
        let this = Rc::new(Self {
            main,
            window,
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
            next_poll: Cell::new(Instant::now()),
            refresh_when_idle: Cell::new(false),
            timer: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&this);
        this.window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                this.action(&action)
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.on_seek(move |seconds| {
            if let Some(this) = weak.upgrade() {
                this.control(&format!("seek_to:{}", seconds.max(0.0) as u64));
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                this.action("close");
            }
            slint::CloseRequestResponse::KeepWindowShown
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
                    if this.window.window().is_visible()
                        && this.window.get_page() == "session"
                        && this.window.get_active()
                        && !this.window.get_busy()
                        && Instant::now() >= this.next_poll.get()
                    {
                        this.control("status");
                    }
                }
            },
        );
        this.restore(None);
        this.send(
            CorePipeRequest::Capabilities {
                request_id: (this.id)(),
            },
            Pending::Capabilities,
        );
        this.control("status");
        Ok(this)
    }

    fn send(&self, request: CorePipeRequest, kind: Pending) -> bool {
        let id = serde_json::to_value(&request)
            .ok()
            .and_then(|v| v["request_id"].as_u64())
            .unwrap_or(0);
        if self.jobs.try_send(request).is_err() {
            self.window.set_notice("操作队列已满，请稍后重试".into());
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

    pub fn show(&self, mode: &str, task: Option<(&str, &str)>) {
        if self.window.get_busy()
            || self.window.get_discovering()
            || self.current_push.borrow().is_some()
        {
            self.show_window();
            return;
        }
        *self.task.borrow_mut() = task.map_or(String::new(), |(id, _)| id.to_owned());
        self.window.set_locked_source(task.is_some());
        self.window
            .set_source_label(task.map_or("", |(_, title)| title).into());
        self.window.set_browser_request(false);
        self.window.set_mode(mode.into());
        self.window.set_page("source".into());
        self.window.set_notice("".into());
        self.clear_devices();
        self.show_window();
        if task.is_some() {
            self.scan();
        }
    }

    fn show_window(&self) {
        crate::native_window::apply_theme(&self.window, self.main.global::<Tokens>().get_dark());
        let _ = self.window.show();
        native_window::prepare_dialog(
            &self.window,
            true,
            true,
            native_window::hwnd(self.main.window()),
        );
    }

    fn clear_devices(&self) {
        self.devices.borrow_mut().clear();
        self.window.set_devices(ModelRc::new(VecModel::default()));
        self.window.set_selected_device(-1);
    }

    fn scan(&self) {
        if self.window.get_busy() || self.window.get_discovering() {
            return;
        }
        self.clear_devices();
        if self.command(
            CoreCommand::DiscoverCastDevices {
                mode: self.window.get_mode().to_string(),
            },
            Pending::Discover,
        ) {
            self.window.set_discovering(true);
        }
    }

    fn action(&self, action: &str) {
        match action {
            "close" if !self.window.get_busy() => {
                if self.current_push.borrow().is_some() {
                    self.resolve("canceled", "已取消设备选择", "");
                } else {
                    let _ = self.window.hide();
                    self.next_push();
                }
            }
            "cancel_push" if !self.window.get_busy() => {
                self.resolve("canceled", "已取消设备选择", "")
            }
            "scan" => self.scan(),
            "mode" => self.clear_devices(),
            "file" if !self.window.get_busy() => {
                match file_dialogs::files(native_window::hwnd(self.window.window()), false) {
                    Ok(paths) => {
                        if let Some(path) = paths.first() {
                            self.window.set_path(path.clone().into());
                        }
                    }
                    Err(error) => self.window.set_notice(error.into()),
                }
            }
            "connect" | "publish" if !self.window.get_busy() && !self.window.get_discovering() => {
                self.connect(action == "publish")
            }
            value if value.starts_with("control:") => self.control(&value[8..]),
            _ => {}
        }
    }

    fn connect(&self, publish: bool) {
        let device = if publish {
            String::new()
        } else {
            let Some(device) = self
                .devices
                .borrow()
                .get(self.window.get_selected_device().max(0) as usize)
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
            } else if self.window.get_source_kind() == "file" {
                (self.window.get_path().trim().to_owned(), String::new())
            } else {
                (String::new(), self.window.get_url().trim().to_owned())
            };
            if path.is_empty() && url.is_empty() {
                self.window
                    .set_notice("请选择媒体文件或输入媒体链接".into());
                return;
            }
            CoreCommand::ShareMedia {
                path,
                url,
                title: self.window.get_media_title().to_string(),
                device_id: device.clone(),
                request_id: push.clone(),
            }
        };
        if self.command(command, Pending::Connect { push, device }) {
            self.window.set_busy(true);
            self.window.set_notice("正在连接接收端…".into());
        }
    }

    fn control(&self, action: &str) {
        if self.window.get_busy()
            || self
                .pending
                .borrow()
                .values()
                .any(|p| matches!(p, Pending::Control))
        {
            return;
        }
        if !self.window.get_active() && action != "status" {
            return;
        }
        if action != "status" && !self.controls_supported.get() {
            self.window
                .set_notice("Core 不支持此版本的投屏控制，请使用同版本组件".into());
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
                self.window.set_busy(true);
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
            self.window.set_busy(true);
        }
    }

    fn offer(&self, request: MediaPushRequest) {
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

    fn next_push(&self) {
        if self.current_push.borrow().is_some()
            || self.window.get_busy()
            || self.window.get_discovering()
        {
            return;
        }
        if self.window.window().is_visible() && self.window.get_page() == "source" {
            return;
        }
        let Some(request) = self.pushes.borrow_mut().pop_front() else {
            return;
        };
        self.task.borrow_mut().clear();
        self.window.set_locked_source(true);
        self.window.set_browser_request(true);
        self.window.set_source_label(request.title.clone().into());
        self.window.set_media_title(request.title.clone().into());
        self.window.set_mode(
            if request.push_kind == "tvbox" {
                "tvbox"
            } else {
                "cast"
            }
            .into(),
        );
        self.window.set_page("source".into());
        self.window.set_notice("".into());
        *self.current_push.borrow_mut() = Some(request);
        self.show_window();
        self.scan();
    }

    pub fn event(&self, envelope: &EventEnvelope) {
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
                    self.window.set_browser_request(false);
                    self.window.set_busy(false);
                    if request.status != "done" {
                        let _ = self.window.hide();
                    }
                    self.window.set_notice(request.message.clone().into());
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
                if self.session.replace(*session_generation) != *session_generation {
                    self.window.set_session_label(device.clone().into());
                    self.window.set_session_url("".into());
                }
                self.window.set_active(*active);
                if !title.is_empty() {
                    self.window
                        .set_session_label(format!("{title} · {device}").into());
                }
                self.window.set_session_status(status.clone().into());
                if !media_url.is_empty() || !active {
                    self.window.set_session_url(media_url.clone().into());
                }
                self.window
                    .set_can_play(supported_actions.iter().any(|s| s == "play"));
                self.window
                    .set_can_pause(supported_actions.iter().any(|s| s == "pause"));
                self.window.set_can_seek(
                    *position_available && supported_actions.iter().any(|s| s == "seek_to"),
                );
                self.window.set_can_stop(*active);
                self.window.set_position(*position_seconds as f32);
                self.window.set_duration(*duration_seconds as f32);
                self.window.set_time(
                    if *position_available {
                        format!(
                            "{} / {}",
                            clock(*position_seconds),
                            clock(*duration_seconds)
                        )
                    } else {
                        String::new()
                    }
                    .into(),
                );
            }
            _ => {}
        }
    }

    pub fn response(&self, response: &CorePipeResponse) {
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
            if matches!(kind, Pending::Discover) {
                self.window.set_discovering(false);
            } else {
                self.window.set_busy(false);
            }
            let message = if matches!(response,CorePipeResponse::Error{code,..} if code=="cast_connection")
                && matches!(kind, Pending::Connect { .. })
            {
                format!("未能确认投送结果：{error}。请先核对当前会话和接收端，勿重复投送。")
            } else {
                error
            };
            self.window.set_notice(message.into());
            if matches!(response,CorePipeResponse::Error{code,..} if code=="cast_connection")
                && matches!(kind, Pending::Connect { .. } | Pending::Resolve { .. })
            {
                self.restore_current();
                self.control("status");
            }
            return;
        }
        match kind {
            Pending::Discover => {
                self.window.set_discovering(false);
                if let CorePipeResponse::Events { events, .. } = response {
                    if let Some(devices) = events.iter().find_map(|e| {
                        if let CoreEvent::CastDevices { devices } = &e.event {
                            Some(devices)
                        } else {
                            None
                        }
                    }) {
                        *self.devices.borrow_mut() = devices.clone();
                        self.window.set_devices(ModelRc::new(VecModel::from(
                            devices
                                .iter()
                                .map(|d| format!("{} · {}", d.label, d.service_type).into())
                                .collect::<Vec<_>>(),
                        )));
                        self.window.set_selected_device(
                            devices
                                .iter()
                                .position(|d| d.id == *self.preferred.borrow())
                                .map_or(if devices.is_empty() { -1 } else { 0 }, |n| n as i32),
                        );
                    }
                }
                self.next_push();
            }
            Pending::Connect { push, device } => {
                self.window.set_busy(false);
                self.window.set_page("session".into());
                self.window.set_notice("已提交到接收端".into());
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
                    self.resolve("done", "媒体已投送", &self.window.get_session_url());
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
                    self.window.set_browser_request(false);
                    self.window.set_busy(false);
                    if status != "done" {
                        let _ = self.window.hide();
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
                        self.window.set_notice("未找到这次浏览器推送的保存记录，请核对接收端状态后关闭窗口；不会自动重复投送。".into());
                    }
                }
            }
            Pending::Control => self.window.set_busy(false),
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
        self.window.set_active(false);
        self.refresh_when_idle.set(true);
        if self.pending.borrow().is_empty() {
            self.refresh();
        }
    }

    fn refresh(&self) {
        self.refresh_when_idle.set(false);
        self.window.set_busy(false);
        self.window.set_discovering(false);
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
