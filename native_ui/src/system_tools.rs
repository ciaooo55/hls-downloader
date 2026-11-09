use crate::{file_dialogs, native_window, DecisionWindow, SystemWindow, Tokens, WorkbenchWindow};
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
    Power {
        title: String,
        seconds: u64,
    },
}

pub struct SystemTools {
    main: WorkbenchWindow,
    window: SystemWindow,
    decision: DecisionWindow,
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
    pub fn apply_theme(&self, dark: bool) {
        crate::native_window::apply_theme(&self.window, dark);
        crate::native_window::apply_theme(&self.decision, dark);
    }

    pub fn keeps_ui_alive(&self) -> bool {
        self.window.window().is_visible()
            || self.decision.window().is_visible()
            || !self.decisions.borrow().is_empty()
            || !self.pending.borrow().is_empty()
    }

    pub fn new(
        main: WorkbenchWindow,
        request: Rc<dyn Fn(CorePipeRequest) -> bool>,
        id: Rc<dyn Fn() -> u64>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let window = SystemWindow::new()?;
        let decision = DecisionWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(780.0, 620.0));
        decision
            .window()
            .set_size(slint::LogicalSize::new(520.0, 320.0));
        let this = Rc::new(Self {
            main,
            window,
            decision,
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
        });
        let weak = Rc::downgrade(&this);
        this.window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                this.action(&action);
            }
        });
        let weak = Rc::downgrade(&this);
        this.decision.on_decided(move |accepted| {
            if let Some(this) = weak.upgrade() {
                this.decide(accepted);
            }
        });
        let weak = Rc::downgrade(&this);
        this.decision.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                this.decide(false);
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        Ok(this)
    }

    pub fn show(&self, page: &str) {
        crate::native_window::apply_theme(&self.window, self.main.global::<Tokens>().get_dark());
        self.window.set_page(page.into());
        self.window
            .set_browser_status(self.main.get_browser_status());
        self.render();
        let _ = self.window.show();
        native_window::prepare_dialog(
            &self.window,
            true,
            true,
            native_window::hwnd(self.main.window()),
        );
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
            self.window.set_notice("命令队列已满，请稍后重试".into());
            return false;
        }
        self.pending.borrow_mut().insert(id);
        true
    }

    fn action(&self, action: &str) {
        self.window.set_notice("".into());
        match action {
            "浏览器插件" => self.show("browser"),
            "通知" => self.show("notices"),
            "更新" => self.show("updates"),
            "关于" => self.show("about"),
            "close" => {
                let _ = self.window.hide();
            }
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
                    self.window.set_busy(true);
                }
            }
            "download_update" if self.window.get_can_download() && !self.window.get_busy() => {
                if self.send(CoreCommand::DownloadUpdate) {
                    self.window.set_busy(true);
                }
            }
            "install_update" if self.window.get_can_install() && !self.window.get_busy() => {
                if file_dialogs::confirm(
                    native_window::hwnd(self.window.window()),
                    "安装更新",
                    "将暂停活动任务并保存断点，退出当前程序后运行已校验的安装包。继续安装？",
                ) && self.send(CoreCommand::InstallUpdate {
                    workbench_pid: std::process::id(),
                }) {
                    self.window.set_busy(true);
                }
            }
            "chromium" => match chromium_directory() {
                Ok(path) => {
                    let result = file_dialogs::open(&path.to_string_lossy());
                    let browser = open_extensions_page();
                    self.window.set_notice(match result {
                        Ok(()) => format!(
                            "已打开插件目录：{}。{}",
                            path.display(),
                            if browser {
                                "请在扩展管理页加载或重新加载扩展"
                            } else {
                                "请手动打开 chrome://extensions 或 edge://extensions"
                            }
                        )
                        .into(),
                        Err(error) => error.into(),
                    });
                }
                Err(error) => self.window.set_notice(error.into()),
            },
            "firefox" => {
                if let Err(error) = file_dialogs::open(
                    "https://addons.mozilla.org/zh-CN/firefox/addon/hls_downloader/",
                ) {
                    self.window.set_notice(error.into());
                }
            }
            "logs" => {
                let path = hls_client::default_v7_database_path()
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join("logs");
                if let Err(error) = file_dialogs::open(&path.to_string_lossy()) {
                    self.window.set_notice(error.into());
                }
            }
            _ => {}
        }
    }

    fn render(&self) {
        let text = match self.window.get_page().as_str() {
            "browser" => self.handoffs.borrow().clone(),
            "notices" => self.notices.borrow().iter().rev().cloned().collect::<Vec<_>>().join("\n\n"),
            "updates" => self.update.borrow().clone(),
            _ => format!("HLS Downloader {}\n\nHTTP / HTTPS、FTP / SFTP、BT、HLS / DASH、直播、边播边下、字幕、投屏与 TVBox、抓站和 ZIP 预览。\n\n下载与任务由单一 Rust Core 管理。\n本工作台使用 Rust + Slint。\n\n配置与任务目录：{}", env!("CARGO_PKG_VERSION"), hls_client::default_v7_database_path().parent().unwrap_or(std::path::Path::new(".")).display()),
        };
        self.window.set_content(text.into());
    }

    fn notify(&self, text: String) {
        let mut notices = self.notices.borrow_mut();
        notices.push_back(text.chars().take(8192).collect());
        while notices.len() > 128 {
            notices.pop_front();
        }
        drop(notices);
        self.render();
    }

    pub fn event(&self, envelope: &EventEnvelope) {
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
            } => self.window.set_browser_status(
                format!(
                    "{} · {} {}\n{}",
                    if *connected { "已连接" } else { "未连接" },
                    browser,
                    version,
                    message
                )
                .into(),
            ),
            CoreEvent::Error { code, message } => {
                self.notify(message.clone());
                self.window.set_notice(message.clone().into());
                if code.starts_with("update") {
                    self.window.set_busy(false);
                }
            }
            CoreEvent::Toast { message, .. } => {
                self.notify(message.clone());
                self.window.set_notice(message.clone().into());
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
                *self.update.borrow_mut() = format!("当前：{current} → 可用：{latest}\n\n{notes}\n\n{installer_name} · {installer_size} 字节\n发布摘要：{}",if *sha256_verified { "已确认" } else { "缺失，自动下载不可用" });
                self.window.set_can_download(
                    *sha256_verified
                        && *installer_size > 0
                        && installer_name.to_ascii_lowercase().ends_with(".msi"),
                );
                self.window.set_can_install(false);
                self.window.set_busy(false);
                self.show("updates");
            }
            CoreEvent::UpdateCurrent { current } => {
                *self.update.borrow_mut() = format!("当前已是最新版本：{current}");
                self.window.set_busy(false);
                self.window.set_can_download(false);
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
                self.window.set_busy(false);
                self.window.set_can_download(false);
                self.window.set_can_install(true);
                self.show("updates");
            }
            CoreEvent::UpdateInstallStarted {
                latest,
                install_log,
                ..
            } => {
                *self.update.borrow_mut() = format!("正在安装 {latest}\n日志：{install_log}");
                self.window.set_busy(true);
                self.render();
            }
            CoreEvent::UpdateInstallResult {
                message,
                install_log,
                ..
            } => {
                *self.update.borrow_mut() = format!("{message}\n日志：{install_log}");
                self.window.set_busy(false);
                self.window.set_can_install(false);
                self.show("updates");
            }
            CoreEvent::DuplicateOffered {
                task_id,
                action,
                message,
                ..
            } => {
                if self.decisions.borrow().len() < 32
                    && !self
                        .decisions
                        .borrow()
                        .iter()
                        .any(|d| matches!(d,Decision::Duplicate{task_id:id,..} if id==task_id))
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
                self.decisions
                    .borrow_mut()
                    .retain(|d| !matches!(d, Decision::Power { .. }));
                let action = match action.as_str() {
                    "shutdown" => "关机",
                    "sleep" => "睡眠",
                    "hibernate" => "休眠",
                    "exit" => "退出程序",
                    _ => action,
                };
                let power = Decision::Power {
                    title: format!("{title}\n完成后动作：{action}"),
                    seconds: *delay_seconds,
                };
                if self.decision_request.get().is_some() {
                    self.decisions.borrow_mut().push_back(power);
                } else {
                    self.decisions.borrow_mut().push_front(power);
                }
                self.show_decision();
            }
            CoreEvent::HandoffResolved { task_id, .. } => self.notify(
                if task_id.is_some() {
                    "浏览器交接裁决已记录，已关联桌面任务"
                } else {
                    "浏览器交接裁决已记录，当前无关联任务"
                }
                .into(),
            ),
            _ => {}
        }
    }

    fn show_decision(&self) {
        let decisions = self.decisions.borrow();
        let Some(next) = decisions.front() else {
            let _ = self.decision.hide();
            return;
        };
        let (title, message, confirm) = match next {
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
                ("发现重复任务", message, label)
            }
            Decision::Power { title, seconds } => (
                "完成后电源动作",
                format!("{title}\n将按 Core 记录的 {seconds} 秒倒计时执行，可在此取消。"),
                "立即执行",
            ),
        };
        crate::native_window::apply_theme(&self.decision, self.main.global::<Tokens>().get_dark());
        self.decision.set_heading(title.into());
        self.decision.set_message(message.into());
        self.decision.set_confirm_label(confirm.into());
        let _ = self.decision.show();
        native_window::prepare_dialog(
            &self.decision,
            true,
            true,
            native_window::hwnd(self.main.window()),
        );
    }

    fn decide(&self, accepted: bool) {
        if self.decision_request.get().is_some() {
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
            Some(Decision::Power { .. }) => Some(if accepted {
                CoreCommand::ConfirmPowerAction
            } else {
                CoreCommand::CancelPowerAction
            }),
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
                self.decision.set_busy(true);
            } else {
                self.decision
                    .set_message("命令队列已满，操作尚未提交，请稍后重试。".into());
            }
            return;
        }
        self.decisions.borrow_mut().pop_front();
        self.show_decision();
    }

    pub fn response(&self, response: &CorePipeResponse) {
        let id = match response {
            CorePipeResponse::Events { request_id, .. } => Some(*request_id),
            CorePipeResponse::Error { request_id, .. } => *request_id,
            _ => None,
        };
        if self.decision_request.get().is_some() && id == self.decision_request.get() {
            self.decision_request.set(None);
            self.decision.set_busy(false);
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
                self.decision.set_message(format!("操作未确认成功：{message}\n请检查当前状态后重试。电源倒计时仍由 Core 管理。").into());
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
                self.window.set_notice(message.clone().into());
                self.window.set_busy(false);
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
                    self.window.set_busy(false);
                    self.window.set_notice(message.clone().into());
                }
            }
            CorePipeResponse::Handoffs {
                request_id,
                items,
                next,
            } if self.handoff_request.get() == Some(*request_id) => {
                self.pending.borrow_mut().remove(request_id);
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
                                item["status"].as_str().unwrap_or("未决"),
                                item["presentation"].as_str().unwrap_or("待呈现")
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
        self.window.set_busy(false);
        self.pending.borrow_mut().clear();
        self.decision_request.set(None);
        self.decision.set_busy(false);
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
