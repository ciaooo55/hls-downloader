use crate::{file_dialogs, native_window, DecisionWindow, SystemWindow, WorkbenchWindow};
use hls_client::{CoreCommand, CoreEvent, CorePipeRequest, CorePipeResponse, EventEnvelope};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::{HashSet, VecDeque},
    path::PathBuf,
    rc::Rc,
};

enum Decision {
    Duplicate {
        task_id: String,
        action: String,
        message: String,
    },
    Confirm {
        tag: String,
        heading: String,
        message: String,
        confirm_label: String,
        danger: bool,
        owner_system: bool,
        on_confirm: Option<Box<dyn FnOnce()>>,
    },
}

pub struct SystemTools {
    main: WorkbenchWindow,
    window: RefCell<Option<SystemWindow>>,
    decision: RefCell<Option<DecisionWindow>>,
    page: RefCell<String>,
    browser_status: RefCell<String>,
    notice: RefCell<String>,
    notice_tone: RefCell<String>,
    can_download: Cell<bool>,
    can_install: Cell<bool>,
    busy: Cell<bool>,
    decision_busy: Cell<bool>,
    decisions: RefCell<VecDeque<Decision>>,
    notices: RefCell<VecDeque<String>>,
    seen: RefCell<VecDeque<u64>>,
    handoffs: RefCell<String>,
    update: RefCell<String>,
    pending: RefCell<HashSet<u64>>,
    handoff_request: Cell<Option<u64>>,
    decision_request: Cell<Option<u64>>,
    request: Rc<dyn Fn(CorePipeRequest) -> bool>,
    id: Rc<dyn Fn() -> u64>,
}

impl SystemTools {
    #[cfg(test)]
    pub(crate) fn has_allocated_windows(&self) -> bool {
        self.window.borrow().is_some() || self.decision.borrow().is_some()
    }

    pub fn sync_appearance(&self) {
        if let Some(window) = self.window.borrow().as_ref() {
            native_window::inherit_appearance(window, &self.main);
        }
        if let Some(decision) = self.decision.borrow().as_ref() {
            native_window::inherit_appearance(decision, &self.main);
        }
    }

    pub fn keeps_ui_alive(&self) -> bool {
        self.window
            .borrow()
            .as_ref()
            .is_some_and(|w| w.window().is_visible())
            || self
                .decision
                .borrow()
                .as_ref()
                .is_some_and(|d| d.window().is_visible())
            || !self.decisions.borrow().is_empty()
            || !self.pending.borrow().is_empty()
    }

    pub fn new(
        main: WorkbenchWindow,
        request: Rc<dyn Fn(CorePipeRequest) -> bool>,
        id: Rc<dyn Fn() -> u64>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let initial_browser = main.get_browser_status().to_string();
        Ok(Rc::new(Self {
            main,
            window: RefCell::new(None),
            decision: RefCell::new(None),
            page: RefCell::new("browser".into()),
            browser_status: RefCell::new(initial_browser),
            notice: RefCell::new(String::new()),
            notice_tone: RefCell::new("warning".into()),
            can_download: Cell::new(false),
            can_install: Cell::new(false),
            busy: Cell::new(false),
            decision_busy: Cell::new(false),
            decisions: RefCell::new(VecDeque::new()),
            notices: RefCell::new(VecDeque::new()),
            seen: RefCell::new(VecDeque::new()),
            handoffs: RefCell::new(String::new()),
            update: RefCell::new("尚未检查更新".into()),
            pending: RefCell::new(HashSet::new()),
            handoff_request: Cell::new(None),
            decision_request: Cell::new(None),
            request,
            id,
        }))
    }

    fn ensure_window(self: &Rc<Self>) -> Result<SystemWindow, slint::PlatformError> {
        if let Some(existing) = self.window.borrow().as_ref() {
            return Ok(existing.clone_strong());
        }
        let window = SystemWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(780.0, 560.0));
        native_window::inherit_appearance(&window, &self.main);
        window.set_page(self.page.borrow().clone().into());
        window.set_browser_status(self.browser_status.borrow().clone().into());
        window.set_notice(self.notice.borrow().clone().into());
        window.set_notice_tone(self.notice_tone.borrow().clone().into());
        window.set_can_download(self.can_download.get());
        window.set_can_install(self.can_install.get());
        window.set_busy(self.busy.get());
        window.set_content(self.current_content().into());
        self.bind_window(&window);
        *self.window.borrow_mut() = Some(window.clone_strong());
        Ok(window)
    }

    fn ensure_decision(self: &Rc<Self>) -> Result<DecisionWindow, slint::PlatformError> {
        if let Some(existing) = self.decision.borrow().as_ref() {
            return Ok(existing.clone_strong());
        }
        let decision = DecisionWindow::new()?;
        decision
            .window()
            .set_size(slint::LogicalSize::new(480.0, 220.0));
        decision.set_busy(self.decision_busy.get());
        let weak = Rc::downgrade(self);
        decision.on_decided(move |accepted| {
            if let Some(this) = weak.upgrade() {
                this.decide(accepted);
            }
        });
        let weak = Rc::downgrade(self);
        decision.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                this.decide(false);
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        *self.decision.borrow_mut() = Some(decision.clone_strong());
        Ok(decision)
    }

    fn bind_window(self: &Rc<Self>, window: &SystemWindow) {
        let weak = Rc::downgrade(self);
        window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                this.action(&action);
            }
        });
        let weak = Rc::downgrade(self);
        window.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                this.close();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_busy(busy);
        }
    }

    fn set_can_download(&self, value: bool) {
        self.can_download.set(value);
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_can_download(value);
        }
    }

    fn set_can_install(&self, value: bool) {
        self.can_install.set(value);
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_can_install(value);
        }
    }

    fn set_notice(&self, tone: &str, text: impl Into<String>) {
        let text = text.into();
        *self.notice_tone.borrow_mut() = tone.to_owned();
        *self.notice.borrow_mut() = text.clone();
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_notice_tone(tone.into());
            window.set_notice(text.into());
        }
    }

    fn clear_notice(&self) {
        self.notice.borrow_mut().clear();
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_notice("".into());
        }
    }

    fn set_browser_status(&self, status: String) {
        *self.browser_status.borrow_mut() = status.clone();
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_browser_status(status.into());
        }
    }

    fn set_decision_busy(&self, busy: bool) {
        self.decision_busy.set(busy);
        if let Some(decision) = self.decision.borrow().as_ref() {
            decision.set_busy(busy);
        }
    }

    fn close(&self) {
        if let Some(window) = self.window.borrow().as_ref() {
            let _ = window.hide();
        }
    }

    pub fn show(self: &Rc<Self>, page: &str) {
        *self.page.borrow_mut() = page.to_owned();
        if self.browser_status.borrow().is_empty()
            || self.browser_status.borrow().as_str() == "浏览器未连接"
            || self.browser_status.borrow().as_str() == "浏览器已连接"
        {
            *self.browser_status.borrow_mut() = self.main.get_browser_status().to_string();
        }
        let Ok(window) = self.ensure_window() else {
            return;
        };
        native_window::inherit_appearance(&window, &self.main);
        window.set_page(page.into());
        window.set_browser_status(self.browser_status.borrow().clone().into());
        self.render();
        let _ = window.show();
        native_window::prepare_dialog(&window, true, true, native_window::hwnd(self.main.window()));
    }

    fn send(&self, command: CoreCommand) -> bool {
        let id = (self.id)();
        self.send_request(
            id,
            CorePipeRequest::Command {
                request_id: id,
                command,
            },
        )
    }

    fn send_request(&self, id: u64, request: CorePipeRequest) -> bool {
        if !(self.request)(request) {
            self.set_notice("error", "命令队列已满，请稍后重试");
            return false;
        }
        self.pending.borrow_mut().insert(id);
        true
    }

    fn action(self: &Rc<Self>, action: &str) {
        self.clear_notice();
        match action {
            "浏览器插件" => self.show("browser"),
            "通知" => self.show("notices"),
            "更新" => self.show("updates"),
            "关于" => self.show("about"),
            "close" => self.close(),
            "clear_notices" => {
                self.notices.borrow_mut().clear();
                self.render();
            }
            "handoffs" => {
                let id = (self.id)();
                if self.send_request(
                    id,
                    CorePipeRequest::LoadHandoffs {
                        request_id: id,
                        handoff_id: None,
                        client_request_id: None,
                        pending_only: true,
                        paged: true,
                        after: None,
                    },
                ) {
                    self.handoff_request.set(Some(id));
                }
            }
            "check_update" => {
                if self.send(CoreCommand::CheckUpdate { silent: false }) {
                    self.set_busy(true);
                }
            }
            "download_update" if self.can_download.get() && !self.busy.get() => {
                if self.send(CoreCommand::DownloadUpdate) {
                    self.set_busy(true);
                }
            }
            "install_update" if self.can_install.get() && !self.busy.get() => {
                let weak = Rc::downgrade(self);
                self.push_confirm(
                    "install_update",
                    "安装更新",
                    "将暂停活动任务并保存断点，退出当前程序后运行已校验的安装包。继续安装？".into(),
                    "安装更新",
                    false,
                    true,
                    Box::new(move || {
                        if let Some(this) = weak.upgrade() {
                            if this.can_install.get()
                                && !this.busy.get()
                                && this.send(CoreCommand::InstallUpdate {
                                    workbench_pid: std::process::id(),
                                })
                            {
                                this.set_busy(true);
                            }
                        }
                    }),
                );
            }
            "chromium" => match chromium_directory() {
                Ok(path) => {
                    let result = file_dialogs::open(&path.to_string_lossy());
                    let browser = open_extensions_page();
                    let tone = if result.is_ok() { "success" } else { "error" };
                    let message = match result {
                        Ok(()) => format!(
                            "已打开插件目录：{}。{}",
                            path.display(),
                            if browser {
                                "请在扩展管理页加载或重新加载扩展"
                            } else {
                                "请手动打开 chrome://extensions 或 edge://extensions"
                            }
                        ),
                        Err(error) => error,
                    };
                    self.set_notice(tone, message);
                }
                Err(error) => self.set_notice("error", error),
            },
            "firefox" => {
                if let Err(error) = file_dialogs::open(
                    "https://addons.mozilla.org/zh-CN/firefox/addon/hls_downloader/",
                ) {
                    self.set_notice("error", error);
                }
            }
            "logs" => {
                let path = hls_client::default_v7_database_path()
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join("logs");
                if let Err(error) = file_dialogs::open(&path.to_string_lossy()) {
                    self.set_notice("error", error);
                }
            }
            _ => {}
        }
    }

    fn current_content(&self) -> String {
        match self.page.borrow().as_str() {
            "browser" => self.handoffs.borrow().clone(),
            "notices" => self
                .notices
                .borrow()
                .iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n"),
            "updates" => self.update.borrow().clone(),
            _ => format!(
                "HLS Downloader {}\n\nHTTP / HTTPS、FTP / SFTP、BT、HLS / DASH、直播、边播边下、字幕、投屏与 TVBox、抓站和 ZIP 预览。\n\n所有下载任务与媒体服务由后台下载引擎统一管理。\n\n配置与任务目录：{}",
                env!("CARGO_PKG_VERSION"),
                hls_client::default_v7_database_path()
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .display()
            ),
        }
    }

    fn render(&self) {
        if let Some(window) = self.window.borrow().as_ref() {
            let page = window.get_page().to_string();
            *self.page.borrow_mut() = page;
            window.set_content(self.current_content().into());
        }
    }

    pub fn notify(&self, text: String) {
        let mut notices = self.notices.borrow_mut();
        notices.push_back(text.chars().take(8192).collect());
        while notices.len() > 128 {
            notices.pop_front();
        }
        drop(notices);
        self.render();
    }

    pub fn confirm(
        self: &Rc<Self>,
        tag: &str,
        heading: &str,
        message: String,
        confirm_label: &str,
        danger: bool,
        on_confirm: Box<dyn FnOnce()>,
    ) {
        self.push_confirm(
            tag,
            heading,
            message,
            confirm_label,
            danger,
            false,
            on_confirm,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn push_confirm(
        self: &Rc<Self>,
        tag: &str,
        heading: &str,
        message: String,
        confirm_label: &str,
        danger: bool,
        owner_system: bool,
        on_confirm: Box<dyn FnOnce()>,
    ) {
        let mut decisions = self.decisions.borrow_mut();
        if decisions.len() >= 32
            || decisions
                .iter()
                .any(|d| matches!(d, Decision::Confirm { tag: existing, .. } if existing == tag))
        {
            return;
        }
        decisions.push_back(Decision::Confirm {
            tag: tag.to_owned(),
            heading: heading.to_owned(),
            message,
            confirm_label: confirm_label.to_owned(),
            danger,
            owner_system,
            on_confirm: Some(on_confirm),
        });
        drop(decisions);
        self.show_decision();
    }

    pub fn event(self: &Rc<Self>, envelope: &EventEnvelope) {
        if envelope.sequence != 0 {
            let mut seen = self.seen.borrow_mut();
            if seen.contains(&envelope.sequence) {
                return;
            }
            seen.push_back(envelope.sequence);
            if seen.len() > 256 {
                seen.pop_front();
            }
        }
        match &envelope.event {
            CoreEvent::BrowserStatus {
                connected,
                version,
                browser,
                message,
            } => self.set_browser_status(format!(
                "{} · {} {}\n{}",
                if *connected { "已连接" } else { "未连接" },
                browser,
                version,
                message
            )),
            CoreEvent::Error { code, message } => {
                self.notify(message.clone());
                self.set_notice("error", message.clone());
                if code.starts_with("update") {
                    self.set_busy(false);
                }
            }
            CoreEvent::Toast { level, message } => {
                self.notify(message.clone());
                self.set_notice(toast_tone(level), message.clone());
            }
            CoreEvent::UpdateAvailable {
                current,
                latest,
                notes,
                installer_name,
                installer_size,
                sha256_verified,
                ..
            } => {
                *self.update.borrow_mut() = format!("当前：{current} → 可用：{latest}\n\n{notes}\n\n{installer_name} · {installer_size} 字节\n发布摘要：{}", if *sha256_verified { "已确认" } else { "缺失，自动下载不可用" });
                self.set_can_download(
                    *sha256_verified
                        && *installer_size > 0
                        && installer_name.to_ascii_lowercase().ends_with(".msi"),
                );
                self.set_can_install(false);
                self.set_busy(false);
                self.show("updates");
            }
            CoreEvent::UpdateCurrent { current } => {
                *self.update.borrow_mut() = format!("当前已是最新版本：{current}");
                self.set_busy(false);
                self.set_can_download(false);
                self.render();
            }
            CoreEvent::UpdateReady {
                latest,
                installer_path,
                product_name,
                product_version,
                ..
            } => {
                *self.update.borrow_mut() = format!("更新已下载并校验\n\n版本：{latest}\n产品：{product_name} {product_version}\n安装包：{installer_path}\n\n安装会暂停活动任务并保存断点；保留配置、任务数据库和下载文件。");
                self.set_busy(false);
                self.set_can_download(false);
                self.set_can_install(true);
                self.show("updates");
            }
            CoreEvent::UpdateInstallStarted {
                latest,
                install_log,
                ..
            } => {
                *self.update.borrow_mut() = format!("正在安装 {latest}\n日志：{install_log}");
                self.set_busy(true);
                self.render();
            }
            CoreEvent::UpdateInstallResult {
                message,
                install_log,
                ..
            } => {
                *self.update.borrow_mut() = format!("{message}\n日志：{install_log}");
                self.set_busy(false);
                self.set_can_install(false);
                self.show("updates");
            }
            CoreEvent::DuplicateOffered {
                task_id,
                action,
                message,
                ..
            } => {
                if self.decisions.borrow().len() < 32
                    && !self.decisions.borrow().iter().any(
                        |d| matches!(d, Decision::Duplicate { task_id: id, .. } if id == task_id),
                    )
                {
                    self.decisions.borrow_mut().push_back(Decision::Duplicate {
                        task_id: task_id.clone(),
                        action: action.clone(),
                        message: message.clone(),
                    });
                    self.show_decision();
                }
            }
            CoreEvent::PowerActionPending {
                action,
                title,
                delay_seconds,
            } => {
                // Power countdown is owned exclusively by CompleteWindow in hot_main.rs;
                // record a passive entry in Notification Center without popping a second dialog.
                let label = match action.as_str() {
                    "shutdown" => "关机",
                    "sleep" => "睡眠",
                    "hibernate" => "休眠",
                    _ => "系统操作",
                };
                self.notify(format!("{title} · 将在 {delay_seconds} 秒后执行{label}"));
            }
            CoreEvent::HandoffResolved { task_id, .. } => self.notify(
                if task_id.is_some() {
                    "已接管浏览器下载"
                } else {
                    "已取消本次浏览器下载接管"
                }
                .into(),
            ),
            _ => {}
        }
    }

    fn show_decision(self: &Rc<Self>) {
        let decisions = self.decisions.borrow();
        let Some(next) = decisions.front() else {
            if let Some(decision) = self.decision.borrow().as_ref() {
                let _ = decision.hide();
            }
            return;
        };
        let (title, message, confirm, danger, owner_system) = match next {
            Decision::Duplicate {
                message, action, ..
            } => {
                let (message, label) = match action.as_str() {
                    "open" => (
                        "该链接已下载完成，文件仍在本地。是否打开已有文件？".into(),
                        "打开已有文件",
                    ),
                    "focus" => (
                        "已有相同链接的任务正在处理中，可以在任务列表中查看进度。".into(),
                        "查看已有任务",
                    ),
                    "none" => (
                        "已有相同链接的任务，可以在任务列表中查看详情。".into(),
                        "查看已有任务",
                    ),
                    _ => (message.clone(), "继续已有任务"),
                };
                (
                    "发现重复任务".to_owned(),
                    message,
                    label.to_owned(),
                    false,
                    false,
                )
            }
            Decision::Confirm {
                heading,
                message,
                confirm_label,
                danger,
                owner_system,
                ..
            } => (
                heading.clone(),
                message.clone(),
                confirm_label.clone(),
                *danger,
                *owner_system,
            ),
        };
        drop(decisions);
        let Ok(decision) = self.ensure_decision() else {
            return;
        };
        native_window::inherit_appearance(&decision, &self.main);
        decision.set_heading(title.into());
        decision.set_message(message.into());
        decision.set_confirm_label(confirm.into());
        decision.set_danger(danger);
        if !decision.window().is_visible() {
            decision.invoke_enter();
        }
        let _ = decision.show();
        let owner = if owner_system {
            self.window
                .borrow()
                .as_ref()
                .filter(|w| w.window().is_visible())
                .and_then(|w| native_window::hwnd(w.window()))
                .or_else(|| native_window::hwnd(self.main.window()))
        } else {
            native_window::hwnd(self.main.window())
        };
        native_window::prepare_dialog(&decision, true, true, owner);
    }

    fn decide(self: &Rc<Self>, accepted: bool) {
        if self.decision_request.get().is_some() {
            return;
        }
        if matches!(
            self.decisions.borrow().front(),
            Some(Decision::Confirm { .. })
        ) {
            let entry = self.decisions.borrow_mut().pop_front();
            self.show_decision();
            if accepted {
                if let Some(Decision::Confirm {
                    on_confirm: Some(callback),
                    ..
                }) = entry
                {
                    callback();
                }
            }
            return;
        }
        let command = match self.decisions.borrow().front() {
            Some(Decision::Duplicate {
                task_id, action, ..
            }) if accepted && !matches!(action.as_str(), "focus" | "none") => {
                Some(CoreCommand::TaskAction {
                    task_id: task_id.clone(),
                    action: action.clone(),
                })
            }
            Some(Decision::Duplicate {
                task_id, action, ..
            }) if accepted && matches!(action.as_str(), "focus" | "none") => {
                self.main
                    .invoke_action(format!("focus_task:{task_id}").into());
                None
            }
            _ => None,
        };
        if let Some(command) = command {
            let id = (self.id)();
            if self.send_request(
                id,
                CorePipeRequest::Command {
                    request_id: id,
                    command,
                },
            ) {
                self.decision_request.set(Some(id));
                self.set_decision_busy(true);
            } else if let Some(decision) = self.decision.borrow().as_ref() {
                decision.set_message("命令队列已满，操作尚未提交，请稍后重试。".into());
            }
            return;
        }
        self.decisions.borrow_mut().pop_front();
        self.show_decision();
    }

    pub fn response(self: &Rc<Self>, response: &CorePipeResponse) {
        let id = match response {
            CorePipeResponse::Events { request_id, .. } => Some(*request_id),
            CorePipeResponse::Error { request_id, .. } => *request_id,
            _ => None,
        };
        if self.decision_request.get().is_some() && id == self.decision_request.get() {
            self.decision_request.set(None);
            self.set_decision_busy(false);
            let error = match response {
                CorePipeResponse::Error { message, .. } => Some(message.as_str()),
                CorePipeResponse::Events { events, .. } => events.iter().find_map(|e| {
                    if let CoreEvent::Error { message, .. } = &e.event {
                        Some(message.as_str())
                    } else {
                        None
                    }
                }),
                _ => None,
            };
            if let Some(message) = error {
                if let Some(decision) = self.decision.borrow().as_ref() {
                    decision.set_message(
                        format!("操作未确认成功：{message}\n请检查当前状态后重试。").into(),
                    );
                }
            } else {
                self.decisions.borrow_mut().pop_front();
                self.show_decision();
            }
        }
        match response {
            CorePipeResponse::Error {
                request_id: Some(id),
                message,
                ..
            } if self.pending.borrow_mut().remove(id) => {
                if self.handoff_request.get() == Some(*id) {
                    self.handoff_request.set(None);
                }
                self.set_notice("error", message.clone());
                self.set_busy(false);
            }
            CorePipeResponse::Events { request_id, events }
                if self.pending.borrow_mut().remove(request_id) =>
            {
                if let Some(message) = events.iter().find_map(|e| {
                    if let CoreEvent::Error { message, .. } = &e.event {
                        Some(message)
                    } else {
                        None
                    }
                }) {
                    self.set_busy(false);
                    self.set_notice("error", message.clone());
                }
            }
            CorePipeResponse::Handoffs {
                request_id,
                items,
                next,
            } if self.pending.borrow_mut().remove(request_id) => {
                if self.handoff_request.get() != Some(*request_id) {
                    return;
                }
                self.handoff_request.set(None);
                *self.handoffs.borrow_mut() = if items.is_empty() {
                    "没有待恢复的浏览器交接".into()
                } else {
                    items
                        .iter()
                        .filter_map(|text| {
                            let item: serde_json::Value = serde_json::from_str(text).ok()?;
                            Some(format!(
                                "{} · {}",
                                format_handoff_term(item["status"].as_str()),
                                format_handoff_term(item["presentation"].as_str())
                            ))
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                        + if next.is_some() {
                            "\n尚有更多待处理记录"
                        } else {
                            ""
                        }
                };
                self.render();
            }
            _ => {}
        }
    }

    pub fn resynchronize(&self) {
        self.seen.borrow_mut().clear();
        self.set_busy(false);
        self.pending.borrow_mut().clear();
        self.handoff_request.set(None);
        self.decision_request.set(None);
        self.set_decision_busy(false);
    }
}

fn format_handoff_term(raw: Option<&str>) -> &str {
    match raw.map(str::trim).unwrap_or("") {
        "" | "pending" => "等待确认",
        "presented" => "已展示",
        "accepted" | "done" => "已接管",
        "rejected" | "canceled" => "已取消",
        "fallback" => "已回退浏览器",
        other => other,
    }
}

/// Core 提示级别 → 页脚状态语气；未知级别按说明处理。
fn toast_tone(level: &str) -> &'static str {
    match level {
        "success" => "success",
        "warn" | "warning" => "warning",
        "error" => "error",
        _ => "info",
    }
}

fn chromium_directory() -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(root) = hls_client::install_root() {
        for root in root.ancestors().take(3) {
            candidates.push(root.join("extensions/Chromium"));
        }
    }
    if let Ok(root) = std::env::current_dir() {
        candidates.push(root.join("extension/.output/chrome-mv3"));
    }
    candidates.into_iter().find(|root| {
        std::fs::read(root.join("manifest.json")).ok().and_then(|text|serde_json::from_slice::<serde_json::Value>(&text).ok()).is_some_and(|v| v["version"] == env!("CARGO_PKG_VERSION") && v["action"]["default_popup"] == "popup.html")
    }).ok_or_else(|| "没有找到当前版本已解压的 Chromium 插件；请先从随包 extensions 目录解压插件包，再加载到浏览器。".into())
}

fn open_extensions_page() -> bool {
    let mut candidates = Vec::new();
    for root in ["PROGRAMFILES(X86)", "PROGRAMFILES", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
    {
        candidates.push((
            root.join("Microsoft/Edge/Application/msedge.exe"),
            "edge://extensions",
        ));
        candidates.push((
            root.join("Google/Chrome/Application/chrome.exe"),
            "chrome://extensions",
        ));
    }
    candidates
        .into_iter()
        .find(|(path, _)| path.is_file())
        .is_some_and(|(path, page)| std::process::Command::new(path).arg(page).spawn().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Rc<SystemTools> {
        crate::offscreen::OffscreenPlatform::init_test();
        let next = Cell::new(0);
        SystemTools::new(
            WorkbenchWindow::new().unwrap(),
            Rc::new(|_| true),
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
        )
        .unwrap()
    }

    fn handoffs(request_id: u64, status: &str) -> CorePipeResponse {
        CorePipeResponse::Handoffs {
            request_id,
            items: vec![
                serde_json::json!({"status": status, "presentation": "pending"}).to_string(),
            ],
            next: None,
        }
    }

    fn overlapping_handoffs(order: [u64; 2]) {
        let tools = tools();
        assert!(tools.window.borrow().is_none());
        assert!(tools.decision.borrow().is_none());
        tools.action("handoffs");
        tools.action("handoffs");
        assert_eq!(tools.handoff_request.get(), Some(2));
        assert!(tools.keeps_ui_alive());
        for id in order {
            tools.response(&handoffs(id, if id == 1 { "stale" } else { "current" }));
            assert!(!tools.pending.borrow().contains(&id));
            if id == 1 {
                assert!(!tools.handoffs.borrow().contains("stale"));
            }
        }
        assert!(tools.pending.borrow().is_empty());
        assert_eq!(tools.handoff_request.get(), None);
        assert_eq!(tools.handoffs.borrow().as_str(), "current · 等待确认");
        tools.show("browser");
        assert_eq!(
            tools
                .window
                .borrow()
                .as_ref()
                .unwrap()
                .get_content()
                .as_str(),
            "current · 等待确认"
        );
        tools.close();
        assert!(!tools.keeps_ui_alive());
    }

    #[test]
    fn superseded_handoff_response_before_current_releases_pending_id() {
        overlapping_handoffs([1, 2]);
    }

    #[test]
    fn superseded_handoff_response_after_current_cannot_overwrite_display() {
        overlapping_handoffs([2, 1]);
    }

    #[test]
    fn handoff_error_and_resync_clear_active_request() {
        let tools = tools();
        tools.action("handoffs");
        tools.response(&CorePipeResponse::Error {
            request_id: Some(1),
            code: "failed".into(),
            message: "failed".into(),
        });
        assert_eq!(tools.handoff_request.get(), None);
        assert!(!tools.keeps_ui_alive());
        tools.action("handoffs");
        tools.resynchronize();
        assert_eq!(tools.handoff_request.get(), None);
        assert!(!tools.keeps_ui_alive());
    }

    #[test]
    fn power_action_pending_does_not_open_decision_window() {
        let tools = tools();
        tools.event(&EventEnvelope {
            sequence: 7,
            event: CoreEvent::PowerActionPending {
                action: "shutdown".into(),
                title: "全部任务已完成".into(),
                delay_seconds: 30,
            },
        });
        assert!(tools.decisions.borrow().is_empty());
        assert!(tools.decision.borrow().is_none());
        assert!(tools
            .notices
            .borrow()
            .back()
            .is_some_and(|n| n.contains("关机") && n.contains("30")));
    }

    #[test]
    fn themed_confirm_runs_callback_only_on_accept_and_deduplicates_by_tag() {
        let tools = tools();
        let fired = Rc::new(Cell::new(0u32));
        for _ in 0..2 {
            let fired = Rc::clone(&fired);
            tools.confirm(
                "delete",
                "删除任务",
                "将处理 1 个任务。".into(),
                "删除任务",
                true,
                Box::new(move || fired.set(fired.get() + 1)),
            );
        }
        assert_eq!(tools.decisions.borrow().len(), 1);
        let decision = tools.decision.borrow().as_ref().unwrap().clone_strong();
        assert!(decision.window().is_visible());
        assert_eq!(decision.get_heading().as_str(), "删除任务");
        assert_eq!(decision.get_confirm_label().as_str(), "删除任务");
        assert!(decision.get_danger());

        // Cancelling drops the pending confirmation without firing the callback.
        decision.invoke_decided(false);
        assert_eq!(fired.get(), 0);
        assert!(!decision.window().is_visible());

        // Confirming invokes the callback exactly once and closes DecisionWindow.
        let fired_accept = Rc::clone(&fired);
        tools.confirm(
            "delete",
            "删除任务",
            "确认删除？".into(),
            "删除任务",
            true,
            Box::new(move || fired_accept.set(fired_accept.get() + 1)),
        );
        decision.invoke_decided(true);
        assert_eq!(fired.get(), 1);
        assert!(!decision.window().is_visible());
    }

    #[test]
    fn install_update_uses_themed_decision_window() {
        crate::offscreen::OffscreenPlatform::init_test();
        let sent = Rc::new(RefCell::new(Vec::<CorePipeRequest>::new()));
        let sent_ref = Rc::clone(&sent);
        let next = Cell::new(0);
        let tools = SystemTools::new(
            WorkbenchWindow::new().unwrap(),
            Rc::new(move |req| {
                sent_ref.borrow_mut().push(req);
                true
            }),
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
        )
        .unwrap();
        tools.set_can_install(true);
        tools.action("install_update");
        assert!(sent.borrow().is_empty());
        let decision = tools.decision.borrow().as_ref().unwrap().clone_strong();
        assert!(decision.window().is_visible());
        assert_eq!(decision.get_heading().as_str(), "安装更新");
        decision.invoke_decided(true);
        assert_eq!(sent.borrow().len(), 1);
        assert!(matches!(
            sent.borrow()[0],
            CorePipeRequest::Command {
                command: CoreCommand::InstallUpdate { .. },
                ..
            }
        ));
        assert!(tools.busy.get());
    }
}
