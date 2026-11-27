use crate::{file_dialogs, native_window, DecisionWindow, SystemWindow, WorkbenchWindow};
use hls_client::{CoreCommand, CoreEvent, CorePipeRequest, CorePipeResponse, EventEnvelope};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::{HashSet, VecDeque},
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

const DEFAULT_GITHUB_RELEASES_URL: &str =
    "https://github.com/ciaooo55/hls-downloader/releases/latest";
const EMBEDDED_TERMS_MD: &str = include_str!("../../TERMS.md");
const EMBEDDED_PRIVACY_MD: &str = include_str!("../../PRIVACY.md");
const CHROMIUM_UNPACKED_GUIDE: &str =
    "已自动解压扩展目录并打开文件夹：请在浏览器扩展页开启「开发者模式」→ 点击「加载解压缩的扩展程序」选择该文件夹";
const CHROMIUM_DIRECTORY_GUIDE: &str =
    "已打开扩展目录：请在浏览器扩展页开启「开发者模式」→ 点击「加载解压缩的扩展程序」选择该文件夹";
const FIREFOX_DEBUGGING_URL: &str = "about:debugging#/runtime/this-firefox";
const MAX_EXTENSION_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTENSION_UNCOMPRESSED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_EXTENSION_ENTRIES: usize = 2048;

type PathQueryHook = Box<dyn Fn() -> Option<PathBuf>>;
type StrActionHook = Box<dyn Fn(&str) -> Result<(), String>>;
type PathActionHook = Box<dyn Fn(&Path) -> Result<(), String>>;
type BrowserPageHook = Box<dyn Fn() -> Option<&'static str>>;
type BoolActionHook = Box<dyn Fn() -> bool>;

struct SystemHooks {
    install_root: PathQueryHook,
    local_app_data: PathQueryHook,
    current_dir: PathQueryHook,
    open_target: StrActionHook,
    reveal_file: PathActionHook,
    copy_clipboard: StrActionHook,
    open_chromium_extensions: BrowserPageHook,
    open_firefox_debugging: BoolActionHook,
}

impl SystemHooks {
    fn live() -> Self {
        Self {
            install_root: Box::new(hls_client::install_root),
            local_app_data: Box::new(local_app_data_dir),
            current_dir: Box::new(|| std::env::current_dir().ok()),
            open_target: Box::new(file_dialogs::open),
            reveal_file: Box::new(reveal_file_in_explorer),
            copy_clipboard: Box::new(file_dialogs::write_clipboard_text),
            open_chromium_extensions: Box::new(open_extensions_page),
            open_firefox_debugging: Box::new(open_firefox_debugging_page),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChromiumExtensionTarget {
    Directory(PathBuf),
    UnpackedArchive(PathBuf),
}

impl ChromiumExtensionTarget {
    fn path(&self) -> &Path {
        match self {
            Self::Directory(path) | Self::UnpackedArchive(path) => path,
        }
    }

    fn was_auto_unpacked(&self) -> bool {
        matches!(self, Self::UnpackedArchive(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FirefoxExtensionTarget {
    Archive(PathBuf),
    Directory(PathBuf),
}

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
    can_open_release: Cell<bool>,
    signing_configured: Cell<bool>,
    about_showing_legal: Cell<bool>,
    busy: Cell<bool>,
    decision_busy: Cell<bool>,
    decisions: RefCell<VecDeque<Decision>>,
    notices: RefCell<VecDeque<String>>,
    seen: RefCell<VecDeque<u64>>,
    handoffs: RefCell<String>,
    update: RefCell<String>,
    release_url: RefCell<Option<String>>,
    pending: RefCell<HashSet<u64>>,
    handoff_request: Cell<Option<u64>>,
    decision_request: Cell<Option<u64>>,
    update_request: Cell<Option<u64>>,
    hooks: SystemHooks,
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
        Self::new_with_hooks(main, request, id, SystemHooks::live())
    }

    fn new_with_hooks(
        main: WorkbenchWindow,
        request: Rc<dyn Fn(CorePipeRequest) -> bool>,
        id: Rc<dyn Fn() -> u64>,
        hooks: SystemHooks,
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
            can_open_release: Cell::new(false),
            signing_configured: Cell::new(has_embedded_signing_pin()),
            about_showing_legal: Cell::new(false),
            busy: Cell::new(false),
            decision_busy: Cell::new(false),
            decisions: RefCell::new(VecDeque::new()),
            notices: RefCell::new(VecDeque::new()),
            seen: RefCell::new(VecDeque::new()),
            handoffs: RefCell::new(String::new()),
            update: RefCell::new("尚未检查更新".into()),
            release_url: RefCell::new(None),
            pending: RefCell::new(HashSet::new()),
            handoff_request: Cell::new(None),
            decision_request: Cell::new(None),
            update_request: Cell::new(None),
            hooks,
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
        window.set_can_open_release(self.can_open_release.get());
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

    fn set_can_open_release(&self, value: bool) {
        self.can_open_release.set(value);
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_can_open_release(value);
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
        self.about_showing_legal.set(false);
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

    fn send(&self, command: CoreCommand) -> Option<u64> {
        let id = (self.id)();
        self.send_request(
            id,
            CorePipeRequest::Command {
                request_id: id,
                command,
            },
        )
        .then_some(id)
    }

    fn send_request(&self, id: u64, request: CorePipeRequest) -> bool {
        if !(self.request)(request) {
            self.set_notice("error", "命令队列已满，请稍后重试");
            return false;
        }
        self.pending.borrow_mut().insert(id);
        true
    }

    fn open_chromium_onboarding(&self) {
        let install_root = (self.hooks.install_root)();
        let local_app_data = (self.hooks.local_app_data)();
        let current_dir = (self.hooks.current_dir)();
        match resolve_chromium_extension(
            install_root.as_deref(),
            local_app_data.as_deref(),
            current_dir.as_deref(),
            env!("CARGO_PKG_VERSION"),
        ) {
            Ok(target) => {
                let path_display = target.path().to_string_lossy().into_owned();
                let copied = (self.hooks.copy_clipboard)(&path_display).is_ok();
                let folder_opened = (self.hooks.open_target)(&path_display);
                let browser_page = (self.hooks.open_chromium_extensions)();
                *self.handoffs.borrow_mut() = format_chromium_onboarding_details(
                    &path_display,
                    target.was_auto_unpacked(),
                    copied,
                    browser_page,
                );
                self.render();
                match folder_opened {
                    Ok(()) => {
                        let base = if target.was_auto_unpacked() {
                            CHROMIUM_UNPACKED_GUIDE
                        } else {
                            CHROMIUM_DIRECTORY_GUIDE
                        };
                        let suffix = match (copied, browser_page) {
                            (true, Some(_)) => "（目录路径已复制到剪贴板）",
                            (true, None) => {
                                "（路径已复制；请手动打开 edge://extensions 或 chrome://extensions）"
                            }
                            (false, Some(_)) => "",
                            (false, None) => "（请手动打开 edge://extensions 或 chrome://extensions）",
                        };
                        self.set_notice("success", format!("{base}{suffix}"));
                    }
                    Err(error) => self.set_notice("error", error),
                }
            }
            Err(error) => self.set_notice("error", error),
        }
    }

    fn open_firefox_onboarding(&self) {
        let install_root = (self.hooks.install_root)();
        let current_dir = (self.hooks.current_dir)();
        match locate_firefox_extension(
            install_root.as_deref(),
            current_dir.as_deref(),
            env!("CARGO_PKG_VERSION"),
        ) {
            Ok(target) => {
                let (select_path, explorer_result, is_archive) = match &target {
                    FirefoxExtensionTarget::Archive(zip_path) => (
                        zip_path.to_string_lossy().into_owned(),
                        (self.hooks.reveal_file)(zip_path),
                        true,
                    ),
                    FirefoxExtensionTarget::Directory(dir) => {
                        let manifest = dir.join("manifest.json");
                        (
                            manifest.to_string_lossy().into_owned(),
                            (self.hooks.open_target)(&dir.to_string_lossy()),
                            false,
                        )
                    }
                };
                let copied = (self.hooks.copy_clipboard)(&select_path).is_ok();
                let firefox_opened = (self.hooks.open_firefox_debugging)();
                *self.handoffs.borrow_mut() = format_firefox_onboarding_details(
                    &select_path,
                    is_archive,
                    copied,
                    firefox_opened,
                );
                self.render();
                match explorer_result {
                    Ok(()) => {
                        let message = if is_archive {
                            format!(
                                "已定位随包 Firefox 扩展包：请在 Firefox 打开 {FIREFOX_DEBUGGING_URL} → 点击「临时载入附加组件…」选择该 ZIP 包{}",
                                if copied { "（路径已复制到剪贴板）" } else { "" }
                            )
                        } else {
                            format!(
                                "已打开 Firefox 扩展构建目录：请在 Firefox 打开 {FIREFOX_DEBUGGING_URL} → 点击「临时载入附加组件…」选择 manifest.json{}",
                                if copied { "（路径已复制到剪贴板）" } else { "" }
                            )
                        };
                        self.set_notice("success", message);
                    }
                    Err(error) => self.set_notice("error", error),
                }
            }
            Err(error) => self.set_notice("error", error),
        }
    }

    fn open_release_page(&self) {
        let candidate = self.release_url.borrow().clone();
        let Some(url) = candidate.as_deref().and_then(sanitize_github_release_url) else {
            self.set_notice("error", "没有可用的官方 GitHub 发布页链接");
            return;
        };
        match (self.hooks.open_target)(&url) {
            Ok(()) => self.set_notice("success", format!("已在浏览器打开官方发布页：{url}")),
            Err(error) => self.set_notice("error", error),
        }
    }

    fn ensure_fallback_release_url(&self) -> String {
        if let Some(existing) = self
            .release_url
            .borrow()
            .as_deref()
            .and_then(sanitize_github_release_url)
        {
            self.set_can_open_release(true);
            return existing;
        }
        let fallback = DEFAULT_GITHUB_RELEASES_URL.to_owned();
        *self.release_url.borrow_mut() = Some(fallback.clone());
        self.set_can_open_release(true);
        fallback
    }

    fn apply_signature_failure_guidance(&self, message: &str, install_log: Option<&str>) {
        self.set_can_download(false);
        self.set_can_install(false);
        let url = self.ensure_fallback_release_url();
        let mut summary = message.to_owned();
        if let Some(log) = install_log.filter(|log| !log.trim().is_empty()) {
            summary.push_str(&format!("\n日志：{log}"));
        }
        summary.push_str(
            "\n\n安装包代码签名校验未通过，无法自动升级。请点击下方「打开发布页」前往官方 GitHub Releases 下载最新安装包手动覆盖安装（自动保留配置、任务数据库与已下载文件）。",
        );
        summary.push_str(&format!("\n官方发布页：{url}"));
        *self.update.borrow_mut() = summary;
        self.set_notice(
            "error",
            format!("{message}（请点击「打开发布页」前往官方 GitHub 下载安装包手动升级）"),
        );
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
                if let Some(id) = self.send(CoreCommand::CheckUpdate { silent: false }) {
                    self.update_request.set(Some(id));
                    self.set_busy(true);
                }
            }
            "open_release" => self.open_release_page(),
            "download_update"
                if self.can_download.get() && self.signing_configured.get() && !self.busy.get() =>
            {
                if let Some(id) = self.send(CoreCommand::DownloadUpdate) {
                    self.update_request.set(Some(id));
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
                            if this.can_install.get() && !this.busy.get() {
                                if let Some(id) = this.send(CoreCommand::InstallUpdate {
                                    workbench_pid: std::process::id(),
                                }) {
                                    this.update_request.set(Some(id));
                                    this.set_busy(true);
                                }
                            }
                        }
                    }),
                );
            }
            "chromium" => self.open_chromium_onboarding(),
            "firefox" => self.open_firefox_onboarding(),
            "logs" => {
                let path = hls_client::default_v7_database_path()
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("logs");
                if let Err(error) = (self.hooks.open_target)(&path.to_string_lossy()) {
                    self.set_notice("error", error);
                }
            }
            "legal_docs" => {
                let next_showing = !self.about_showing_legal.get();
                self.about_showing_legal.set(next_showing);
                self.render();
                if next_showing {
                    self.set_notice(
                        "success",
                        "已显示《用户协议与免责声明》（TERMS.md）与《隐私政策》（PRIVACY.md）全文；再次点击可返回关于概览",
                    );
                } else {
                    self.set_notice("info", "已返回关于概览");
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
            _ if self.about_showing_legal.get() => {
                let install_root = (self.hooks.install_root)();
                let current_dir = (self.hooks.current_dir)();
                load_legal_and_privacy_text(install_root.as_deref(), current_dir.as_deref())
            }
            _ => format!(
                "HLS Downloader {}\n\nHTTP / HTTPS、FTP / SFTP、BT、HLS / DASH、直播、边播边下、字幕、投屏与 TVBox、抓站和 ZIP 预览。\n\n所有下载任务与媒体服务由后台下载引擎统一管理。\n\n用户协议与隐私政策版本：{}\n配置与任务目录：{}",
                env!("CARGO_PKG_VERSION"),
                hls_client::LEGAL_TERMS_VERSION,
                hls_client::default_v7_database_path()
                    .parent()
                    .unwrap_or(Path::new("."))
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
                if code.starts_with("update") && is_signature_verification_error(message) {
                    self.set_busy(false);
                    self.apply_signature_failure_guidance(message, None);
                    self.render();
                } else {
                    self.set_notice("error", message.clone());
                    if code.starts_with("update") {
                        self.set_busy(false);
                    }
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
                release_url,
                installer_name,
                installer_size,
                sha256_verified,
            } => {
                let valid_release_url = sanitize_github_release_url(release_url);
                *self.release_url.borrow_mut() = valid_release_url.clone();
                self.set_can_open_release(valid_release_url.is_some());
                let signing_ready = self.signing_configured.get();
                let asset_ready = *sha256_verified
                    && *installer_size > 0
                    && installer_name.to_ascii_lowercase().ends_with(".msi");
                let effective_release_url =
                    if (!signing_ready || !asset_ready) && valid_release_url.is_none() {
                        Some(self.ensure_fallback_release_url())
                    } else {
                        valid_release_url
                    };
                *self.update.borrow_mut() = format_update_available_summary(
                    current,
                    latest,
                    notes,
                    installer_name,
                    *installer_size,
                    *sha256_verified,
                    signing_ready,
                    effective_release_url.as_deref(),
                );
                self.set_can_download(signing_ready && asset_ready);
                self.set_can_install(false);
                self.set_busy(false);
                if !signing_ready {
                    if effective_release_url.is_some() {
                        self.set_notice(
                            "info",
                            "当前版本未嵌入代码签名证书，请点击「打开发布页」下载安装包手动覆盖升级",
                        );
                    } else {
                        self.set_notice(
                            "warning",
                            "当前版本未嵌入代码签名证书，自动更新已停用；请前往 GitHub Releases 手动下载",
                        );
                    }
                }
                self.show("updates");
            }
            CoreEvent::UpdateCurrent { current } => {
                *self.update.borrow_mut() = format!("当前已是最新版本：{current}");
                *self.release_url.borrow_mut() = None;
                self.set_busy(false);
                self.set_can_download(false);
                self.set_can_install(false);
                self.set_can_open_release(false);
                self.render();
            }
            CoreEvent::UpdateReady {
                latest,
                installer_path,
                product_name,
                product_version,
                ..
            } => {
                *self.update.borrow_mut() = format!(
                    "更新已下载并校验\n\n版本：{latest}\n产品：{product_name} {product_version}\n安装包：{installer_path}\n\n安装会暂停活动任务并保存断点；保留配置、任务数据库和下载文件。"
                );
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
                status,
                message,
                install_log,
                ..
            } => {
                self.set_busy(false);
                self.set_can_install(false);
                if status != "success" && is_signature_verification_error(message) {
                    self.apply_signature_failure_guidance(message, Some(install_log));
                } else {
                    *self.update.borrow_mut() = format!("{message}\n日志：{install_log}");
                }
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
                let is_update_req = self.update_request.get() == Some(*id);
                if is_update_req {
                    self.update_request.set(None);
                }
                self.set_busy(false);
                if is_update_req && is_signature_verification_error(message) {
                    self.apply_signature_failure_guidance(message, None);
                    self.render();
                } else {
                    self.set_notice("error", message.clone());
                }
            }
            CorePipeResponse::Events { request_id, events }
                if self.pending.borrow_mut().remove(request_id) =>
            {
                let is_update_req = self.update_request.get() == Some(*request_id);
                if is_update_req {
                    self.update_request.set(None);
                }
                if let Some(message) = events.iter().find_map(|e| {
                    if let CoreEvent::Error { message, .. } = &e.event {
                        Some(message)
                    } else {
                        None
                    }
                }) {
                    self.set_busy(false);
                    if is_update_req && is_signature_verification_error(message) {
                        self.apply_signature_failure_guidance(message, None);
                        self.render();
                    } else {
                        self.set_notice("error", message.clone());
                    }
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
        self.update_request.set(None);
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

fn read_bundled_doc(
    file_name: &str,
    install_root: Option<&Path>,
    current_dir: Option<&Path>,
) -> Option<String> {
    if let Some(root) = install_root {
        for ancestor in root.ancestors().take(4) {
            let candidate = ancestor.join(file_name);
            if let Ok(text) = std::fs::read_to_string(&candidate) {
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
    }
    if let Some(cwd) = current_dir {
        let candidate = cwd.join(file_name);
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn load_legal_and_privacy_text(install_root: Option<&Path>, current_dir: Option<&Path>) -> String {
    let terms = read_bundled_doc("TERMS.md", install_root, current_dir)
        .unwrap_or_else(|| EMBEDDED_TERMS_MD.to_owned());
    let privacy = read_bundled_doc("PRIVACY.md", install_root, current_dir)
        .unwrap_or_else(|| EMBEDDED_PRIVACY_MD.to_owned());
    format!(
        "{}\n\n{}\n\n{}",
        terms.trim(),
        "────────────────────────────────────────────────",
        privacy.trim()
    )
}

fn has_embedded_signing_pin() -> bool {
    is_valid_signing_thumbprint(option_env!("HLS_V7_SIGN_CERT_THUMBPRINT"))
}

fn is_valid_signing_thumbprint(raw: Option<&str>) -> bool {
    let Some(value) = raw else {
        return false;
    };
    let mut digits = 0usize;
    for ch in value.chars().filter(|ch| !ch.is_ascii_whitespace()) {
        if !ch.is_ascii_hexdigit() {
            return false;
        }
        digits += 1;
    }
    digits == 40
}

fn sanitize_github_release_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('\u{feff}');
    if !file_dialogs::is_safe_web_url(trimmed) || trimmed.contains('\\') {
        return None;
    }
    let rest = trimmed.get(8..).filter(|_| {
        trimmed
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    })?;
    let (authority, path_and_query) = rest.split_once('/')?;
    if authority.contains('@') || authority.contains(['?', '#']) {
        return None;
    }
    let host_ok = match authority.split_once(':') {
        Some((host, "443")) => host.eq_ignore_ascii_case("github.com"),
        Some(_) => false,
        None => authority.eq_ignore_ascii_case("github.com"),
    };
    if !host_ok || path_and_query.is_empty() {
        return None;
    }
    let path_only = path_and_query.split(['?', '#']).next().unwrap_or("");
    if path_only.is_empty() {
        return None;
    }
    for segment in path_only.split('/') {
        if segment == "." || segment == ".." {
            return None;
        }
        let lower = segment.to_ascii_lowercase();
        if lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c") {
            return None;
        }
    }
    Some(format!("https://github.com/{path_and_query}"))
}

fn is_signature_verification_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("winverifytrust")
        || lower.contains("authenticode")
        || message.contains("HLS_V7_SIGN_CERT_THUMBPRINT")
        || message.contains("签名")
        || message.contains("证书")
}

#[allow(clippy::too_many_arguments)]
fn format_update_available_summary(
    current: &str,
    latest: &str,
    notes: &str,
    installer_name: &str,
    installer_size: u64,
    sha256_verified: bool,
    signing_ready: bool,
    release_url: Option<&str>,
) -> String {
    let asset_line = if installer_name.trim().is_empty() {
        "安装包资源：未提供 MSI 安装包".to_owned()
    } else {
        format!("{installer_name} · {installer_size} 字节")
    };
    let digest_status = if sha256_verified {
        "已确认"
    } else {
        "缺失，自动下载不可用"
    };
    let mut summary = format!("当前：{current} → 可用：{latest}");
    if !notes.trim().is_empty() {
        summary.push_str(&format!("\n\n{}", notes.trim()));
    }
    summary.push_str(&format!("\n\n{asset_line}\n发布摘要：{digest_status}"));
    if !signing_ready {
        summary.push_str(
            "\n\n当前构建未嵌入代码签名证书指纹，应用内自动下载与安装已停用（避免下载后因系统签名校验拦截）。\n请点击下方「打开发布页」前往官方 GitHub Releases 下载最新安装包并手动覆盖安装（自动保留现有配置、任务数据库与已下载文件）。",
        );
    } else if !sha256_verified
        || installer_size == 0
        || !installer_name.to_ascii_lowercase().ends_with(".msi")
    {
        summary.push_str("\n\n当前发布未附带可自动校验的 MSI 安装包，请点击下方「打开发布页」前往 GitHub Releases 手动下载更新。");
    }
    if let Some(url) = release_url {
        summary.push_str(&format!("\n官方发布页：{url}"));
    }
    summary
}

fn format_chromium_onboarding_details(
    unpacked_dir: &str,
    auto_unpacked: bool,
    copied: bool,
    browser_page: Option<&str>,
) -> String {
    let header = if auto_unpacked {
        "已自动解压随包 Chrome / Edge 扩展目录："
    } else {
        "已定位当前版本 Chrome / Edge 扩展目录："
    };
    let copy_hint = if copied {
        "（已复制到剪贴板，可在文件夹选择框直接粘贴）"
    } else {
        ""
    };
    let page_hint = browser_page.unwrap_or("edge://extensions 或 chrome://extensions");
    format!(
        "{header}\n{unpacked_dir} {copy_hint}\n\n安装步骤：\n1. 在浏览器打开扩展管理页（{page_hint}）\n2. 开启右上角或左下角的「开发者模式」\n3. 点击「加载解压缩的扩展程序」，选择上方已打开的扩展文件夹（若已安装旧版，点击扩展卡片上的「重新加载」即可）"
    )
}

fn format_firefox_onboarding_details(
    target_path: &str,
    is_archive: bool,
    copied: bool,
    firefox_opened: bool,
) -> String {
    let header = if is_archive {
        "已定位随包 Firefox 扩展包（待 Mozilla 签名）："
    } else {
        "已定位当前版本 Firefox 扩展清单："
    };
    let copy_hint = if copied {
        "（路径已复制到剪贴板）"
    } else {
        ""
    };
    let launch_hint = if firefox_opened {
        format!("已尝试启动 Firefox 打开 {FIREFOX_DEBUGGING_URL}")
    } else {
        format!("请在 Firefox 地址栏输入并打开 {FIREFOX_DEBUGGING_URL}")
    };
    format!(
        "{header}\n{target_path} {copy_hint}\n\n加载步骤：\n1. {launch_hint}\n2. 点击「临时载入附加组件…（Load Temporary Add-on…）」，选择上方文件\n3. 说明：随包提供待 Mozilla 签名 ZIP 扩展包；标准版 Firefox 重启后需重新临时载入，Firefox Developer Edition / Nightly 或经签名后可持久安装"
    )
}

fn local_app_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            hls_client::default_v7_database_path()
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
}

fn sanitize_version_segment(version: &str) -> String {
    let cleaned: String = version
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() || is_windows_reserved_name(trimmed) {
        "current".into()
    } else {
        trimmed.to_owned()
    }
}

fn is_windows_reserved_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.']);
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM0"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT0"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn is_valid_chromium_manifest_dir(root: &Path, expected_version: &str) -> bool {
    let Ok(text) = std::fs::read(root.join("manifest.json")) else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return false;
    };
    let mv_ok = v
        .get("manifest_version")
        .is_none_or(|mv| mv.as_u64() == Some(3));
    let is_firefox_only = v.get("browser_specific_settings").is_some() && v.get("key").is_none();
    mv_ok
        && !is_firefox_only
        && v["version"].as_str() == Some(expected_version)
        && v["action"]["default_popup"].as_str() == Some("popup.html")
        && root.join("popup.html").is_file()
}

fn is_valid_firefox_manifest_dir(root: &Path, expected_version: &str) -> bool {
    let Ok(text) = std::fs::read(root.join("manifest.json")) else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return false;
    };
    v.get("manifest_version")
        .and_then(serde_json::Value::as_u64)
        == Some(3)
        && v["version"].as_str() == Some(expected_version)
        && v["browser_specific_settings"]["gecko"]["id"]
            .as_str()
            .is_some_and(|id| !id.trim().is_empty())
}

fn bundled_extension_archive_candidates(
    install_root: &Path,
    browser: &str,
    expected_version: &str,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let versioned_name = format!("HLSDownloader-{expected_version}-{browser}.zip");
    let plain_name = format!("{browser}.zip");
    for ancestor in install_root.ancestors().take(3) {
        let ext_dir = ancestor.join("extensions");
        candidates.push(ext_dir.join(&versioned_name));
        candidates.push(ext_dir.join(&plain_name));
    }
    candidates
}

fn resolve_chromium_extension(
    install_root: Option<&Path>,
    local_app_data: Option<&Path>,
    current_dir: Option<&Path>,
    expected_version: &str,
) -> Result<ChromiumExtensionTarget, String> {
    let mut unpacked_candidates = Vec::new();
    if let Some(root) = install_root {
        for ancestor in root.ancestors().take(3) {
            unpacked_candidates.push(ancestor.join("extensions").join("Chromium"));
        }
    }
    if let Some(cwd) = current_dir {
        unpacked_candidates.push(cwd.join("extension/.output/chrome-mv3"));
    }
    if let Some(dir) = unpacked_candidates
        .into_iter()
        .find(|dir| is_valid_chromium_manifest_dir(dir, expected_version))
    {
        return Ok(ChromiumExtensionTarget::Directory(dir));
    }

    let archive = install_root
        .into_iter()
        .flat_map(|root| bundled_extension_archive_candidates(root, "Chromium", expected_version))
        .find(|candidate| candidate.is_file());

    let Some(zip_path) = archive else {
        return Err(format!(
            "未找到随包 Chrome / Edge 扩展包（HLSDownloader-{expected_version}-Chromium.zip）；请确认安装包完整，或在源码环境先构建 extension 目录。"
        ));
    };

    let Some(app_data) = local_app_data.filter(|path| !path.as_os_str().is_empty()) else {
        return Err("无法定位系统 LOCALAPPDATA 目录，无法自动解压浏览器扩展。".into());
    };

    let dest_dir = app_data
        .join("HLS Downloader")
        .join("extension-unpacked")
        .join(sanitize_version_segment(expected_version))
        .join("Chromium");
    unpack_chromium_extension_archive(&zip_path, &dest_dir, expected_version)?;
    Ok(ChromiumExtensionTarget::UnpackedArchive(dest_dir))
}

fn locate_firefox_extension(
    install_root: Option<&Path>,
    current_dir: Option<&Path>,
    expected_version: &str,
) -> Result<FirefoxExtensionTarget, String> {
    if let Some(root) = install_root {
        for candidate in bundled_extension_archive_candidates(root, "Firefox", expected_version) {
            if candidate.is_file() {
                validate_extension_zip_file(&candidate)?;
                return Ok(FirefoxExtensionTarget::Archive(candidate));
            }
        }
        for ancestor in root.ancestors().take(3) {
            let unpacked = ancestor.join("extensions").join("Firefox");
            if is_valid_firefox_manifest_dir(&unpacked, expected_version) {
                return Ok(FirefoxExtensionTarget::Directory(unpacked));
            }
        }
    }
    if let Some(cwd) = current_dir {
        let dev_dir = cwd.join("extension/.output/firefox-mv3");
        if is_valid_firefox_manifest_dir(&dev_dir, expected_version) {
            return Ok(FirefoxExtensionTarget::Directory(dev_dir));
        }
    }
    Err(format!(
        "未找到随包 Firefox 扩展包（HLSDownloader-{expected_version}-Firefox.zip）；随包交付待 Mozilla 签名 ZIP 扩展包，请确认使用完整安装包或便携包。"
    ))
}

fn validate_extension_zip_file(zip_path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(zip_path)
        .map_err(|error| format!("无法读取扩展压缩包 {}: {error}", zip_path.display()))?;
    if !meta.is_file() {
        return Err("扩展压缩包路径不是普通文件".into());
    }
    if meta.len() < 22 || meta.len() > MAX_EXTENSION_ARCHIVE_BYTES {
        return Err("扩展压缩包大小异常，已拒绝读取".into());
    }
    let bytes = std::fs::read(zip_path)
        .map_err(|error| format!("读取扩展压缩包失败 {}: {error}", zip_path.display()))?;
    validate_extension_zip_entries(&bytes)?;
    Ok(bytes)
}

fn validate_zip_entry_path(raw_name: &str) -> Result<(&str, bool), String> {
    if raw_name.is_empty()
        || raw_name.len() > 240
        || raw_name.contains('\\')
        || raw_name.contains(':')
        || raw_name.starts_with('/')
        || raw_name.chars().any(char::is_control)
    {
        return Err(format!("扩展压缩包包含非法条目路径：{raw_name}"));
    }
    let (trimmed, is_dir) = match raw_name.strip_suffix('/') {
        Some(dir_path) => (dir_path, true),
        None => (raw_name, false),
    };
    if trimmed.is_empty() {
        return Err("扩展压缩包包含空目录条目".into());
    }
    for segment in trimmed.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.ends_with([' ', '.'])
            || is_windows_reserved_name(segment)
        {
            return Err(format!("扩展压缩包包含不安全的路径段：{raw_name}"));
        }
    }
    Ok((trimmed, is_dir))
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Validates a ZIP archive's EOCD, Central Directory, and matching Local File Headers
/// before invoking any OS extraction tool, blocking Zip-Slip, dual-header spoofing,
/// symlinks/reparse points, encrypted entries, and decompression bombs.
fn validate_extension_zip_entries(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 22 || bytes.len() as u64 > MAX_EXTENSION_ARCHIVE_BYTES {
        return Err("扩展压缩包格式无效或体积超限".into());
    }
    let min_eocd = bytes.len().saturating_sub(22 + 65_535);
    let mut eocd_pos = None;
    let mut scan = bytes.len() - 22;
    loop {
        if bytes.get(scan..scan + 4) == Some(&[0x50, 0x4b, 0x05, 0x06]) {
            if let Some(comment_len) = read_u16_le(bytes, scan + 20) {
                if scan + 22 + usize::from(comment_len) == bytes.len() {
                    eocd_pos = Some(scan);
                    break;
                }
            }
        }
        if scan <= min_eocd {
            break;
        }
        scan -= 1;
    }
    let eocd = eocd_pos.ok_or_else(|| "扩展压缩包缺少有效的 ZIP 目录尾记录".to_string())?;
    let disk_num = read_u16_le(bytes, eocd + 4).unwrap_or(1);
    let cd_disk = read_u16_le(bytes, eocd + 6).unwrap_or(1);
    let entries_on_disk = usize::from(read_u16_le(bytes, eocd + 8).unwrap_or(0));
    let total_entries = usize::from(read_u16_le(bytes, eocd + 10).unwrap_or(0));
    let cd_size = read_u32_le(bytes, eocd + 12).unwrap_or(u32::MAX) as usize;
    let cd_offset = read_u32_le(bytes, eocd + 16).unwrap_or(u32::MAX) as usize;
    if disk_num != 0
        || cd_disk != 0
        || entries_on_disk != total_entries
        || total_entries == 0
        || total_entries > MAX_EXTENSION_ENTRIES
        || total_entries == 0xFFFF
        || cd_size == u32::MAX as usize
        || cd_offset == u32::MAX as usize
    {
        return Err("扩展压缩包条目数量或分卷属性不受支持".into());
    }
    let cd_end = cd_offset
        .checked_add(cd_size)
        .filter(|end| *end <= eocd)
        .ok_or_else(|| "扩展压缩包中央目录越界".to_string())?;

    let mut cursor = cd_offset;
    let mut total_uncompressed = 0u64;
    let mut has_manifest = false;
    for _ in 0..total_entries {
        if cursor.checked_add(46).is_none_or(|end| end > cd_end) {
            return Err("扩展压缩包中央目录已损坏".into());
        }
        if bytes.get(cursor..cursor + 4) != Some(&[0x50, 0x4b, 0x01, 0x02]) {
            return Err("扩展压缩包中央目录签名无效".into());
        }
        let version_made_by = read_u16_le(bytes, cursor + 4).unwrap_or(0);
        let flags = read_u16_le(bytes, cursor + 8).unwrap_or(0);
        if (flags & 0x0001) != 0 {
            return Err("扩展压缩包不支持加密条目".into());
        }
        let compressed_size = read_u32_le(bytes, cursor + 20).unwrap_or(u32::MAX);
        let uncompressed_size = read_u32_le(bytes, cursor + 24).unwrap_or(u32::MAX);
        if compressed_size == u32::MAX || uncompressed_size == u32::MAX {
            return Err("扩展压缩包不支持 ZIP64 条目".into());
        }
        total_uncompressed = total_uncompressed.saturating_add(u64::from(uncompressed_size));
        if total_uncompressed > MAX_EXTENSION_UNCOMPRESSED_BYTES {
            return Err("扩展压缩包解压后总大小超出安全上限".into());
        }
        let name_len = usize::from(read_u16_le(bytes, cursor + 28).unwrap_or(0));
        let extra_len = usize::from(read_u16_le(bytes, cursor + 30).unwrap_or(0));
        let comment_len = usize::from(read_u16_le(bytes, cursor + 32).unwrap_or(0));
        let disk_start = read_u16_le(bytes, cursor + 34).unwrap_or(1);
        if disk_start != 0 {
            return Err("扩展压缩包不支持分卷条目".into());
        }
        let external_attrs = read_u32_le(bytes, cursor + 38).unwrap_or(0);
        let local_offset = read_u32_le(bytes, cursor + 42).unwrap_or(u32::MAX) as usize;

        let creator_os = version_made_by >> 8;
        if ((external_attrs >> 16) & 0o170000) == 0o120000 {
            return Err("扩展压缩包包含不安全的符号链接条目".into());
        }
        if matches!(creator_os, 0 | 10 | 14) && (external_attrs & 0x0400) != 0 {
            return Err("扩展压缩包包含不安全的重解析点条目".into());
        }

        let name_start = cursor + 46;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| "扩展压缩包文件名长度溢出".to_string())?;
        let next_cursor = name_end
            .checked_add(extra_len)
            .and_then(|pos| pos.checked_add(comment_len))
            .filter(|pos| *pos <= cd_end)
            .ok_or_else(|| "扩展压缩包条目元数据越界".to_string())?;
        let name_bytes = bytes
            .get(name_start..name_end)
            .ok_or_else(|| "扩展压缩包文件名越界".to_string())?;
        let raw_name = std::str::from_utf8(name_bytes)
            .map_err(|_| "扩展压缩包包含非 UTF-8 文件名".to_string())?;
        let (clean_name, is_dir) = validate_zip_entry_path(raw_name)?;

        // Cross-check Local File Header flags and filename so streaming extractors see the identical entry.
        let local_name_start = local_offset
            .checked_add(30)
            .filter(|end| *end <= cd_offset)
            .ok_or_else(|| "扩展压缩包本地文件头越界".to_string())?;
        if bytes.get(local_offset..local_offset + 4) != Some(&[0x50, 0x4b, 0x03, 0x04]) {
            return Err("扩展压缩包本地文件头签名无效".into());
        }
        let local_flags = read_u16_le(bytes, local_offset + 6).unwrap_or(0x0001);
        if (local_flags & 0x0001) != 0 {
            return Err("扩展压缩包本地文件头不支持加密条目".into());
        }
        let local_name_len = usize::from(read_u16_le(bytes, local_offset + 26).unwrap_or(0));
        let local_name_end = local_name_start
            .checked_add(local_name_len)
            .filter(|end| *end <= cd_offset)
            .ok_or_else(|| "扩展压缩包本地文件名越界".to_string())?;
        if bytes.get(local_name_start..local_name_end) != Some(name_bytes) {
            return Err("扩展压缩包本地文件头与中央目录文件名不一致".into());
        }

        if !is_dir && clean_name == "manifest.json" {
            has_manifest = true;
        }
        cursor = next_cursor;
    }

    if cursor != cd_end {
        return Err("扩展压缩包中央目录长度不匹配".into());
    }
    if !has_manifest {
        return Err("扩展压缩包缺少根目录 manifest.json".into());
    }
    Ok(())
}

fn archive_fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("v1:{}:{hash:016x}", bytes.len())
}

fn unpack_chromium_extension_archive(
    zip_path: &Path,
    dest_dir: &Path,
    expected_version: &str,
) -> Result<(), String> {
    let bytes = validate_extension_zip_file(zip_path)?;
    let stamp = archive_fingerprint(&bytes);
    let stamp_file = dest_dir.join(".archive-stamp");
    if std::fs::read_to_string(&stamp_file)
        .ok()
        .is_some_and(|existing| existing.trim() == stamp)
        && verify_extracted_tree(dest_dir).is_ok()
        && is_valid_chromium_manifest_dir(dest_dir, expected_version)
    {
        return Ok(());
    }

    let parent = dest_dir
        .parent()
        .ok_or_else(|| "扩展解压目标目录无效".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建扩展解压目录失败 {}: {error}", parent.display()))?;

    static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let staging_dir = parent.join(format!(".staging-chromium-{}-{seq}", std::process::id()));
    if staging_dir.exists() {
        let _ = std::fs::remove_dir_all(&staging_dir);
    }
    std::fs::create_dir_all(&staging_dir)
        .map_err(|error| format!("准备扩展临时解压目录失败: {error}"))?;

    let outcome = (|| -> Result<(), String> {
        extract_zip_archive(zip_path, &staging_dir)?;
        verify_extracted_tree(&staging_dir)?;
        if !is_valid_chromium_manifest_dir(&staging_dir, expected_version) {
            return Err(format!(
                "解压后的 Chromium 扩展缺少匹配的 v{expected_version} manifest.json 或 popup.html"
            ));
        }
        ensure_safe_publish_dest(dest_dir)?;
        // Invalidate any previous `.archive-stamp` before publishing so an interrupted
        // fallback copy can never leave a premature or stale stamp on a partial tree.
        let _ = std::fs::remove_file(&stamp_file);
        publish_extracted_directory(&staging_dir, dest_dir)?;
        verify_extracted_tree(dest_dir)?;
        if !is_valid_chromium_manifest_dir(dest_dir, expected_version) {
            return Err("扩展目录发布后校验未通过".into());
        }
        std::fs::write(&stamp_file, &stamp)
            .map_err(|error| format!("写入扩展解压标记失败: {error}"))?;
        Ok(())
    })();

    if staging_dir.exists() {
        let _ = std::fs::remove_dir_all(&staging_dir);
    }
    outcome
}

fn system_root_dir() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

fn configure_hidden_command(command: &mut std::process::Command) {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
}

fn is_symlink_or_reparse_point(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return true;
        }
    }
    false
}

fn ensure_safe_publish_dest(dest_dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(dest_dir) {
        Ok(meta) => {
            if is_symlink_or_reparse_point(&meta) || !meta.is_dir() {
                return Err(format!(
                    "扩展目标目录不是安全的普通目录，已拒绝发布：{}",
                    dest_dir.display()
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "读取扩展目标目录元数据失败 {}: {error}",
            dest_dir.display()
        )),
    }
}

fn reset_extraction_staging_dir(dest_dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(dest_dir) {
        Ok(meta) => {
            if is_symlink_or_reparse_point(&meta) || !meta.is_dir() {
                return Err("扩展临时解压目录类型异常，已拒绝清理".into());
            }
            std::fs::remove_dir_all(dest_dir)
                .map_err(|error| format!("清理残留的临时解压目录失败: {error}"))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("读取临时解压目录状态失败: {error}")),
    }
    std::fs::create_dir_all(dest_dir).map_err(|error| format!("重建扩展临时解压目录失败: {error}"))
}

fn extract_zip_archive(zip_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let sys_root = system_root_dir();
    let system_tar = sys_root.join("System32").join("tar.exe");
    #[cfg(windows)]
    let powershell = sys_root
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    extract_zip_archive_with(
        zip_path,
        dest_dir,
        |zip, dest| {
            if !system_tar.is_file() {
                return false;
            }
            let mut command = std::process::Command::new(&system_tar);
            command.arg("-xf").arg(zip).arg("-C").arg(dest);
            configure_hidden_command(&mut command);
            command.output().is_ok_and(|output| output.status.success())
        },
        #[cfg(windows)]
        powershell.as_path(),
    )
}

fn extract_zip_archive_with(
    zip_path: &Path,
    dest_dir: &Path,
    mut primary_extract: impl FnMut(&Path, &Path) -> bool,
    #[cfg(windows)] powershell: &Path,
) -> Result<(), String> {
    if primary_extract(zip_path, dest_dir) {
        return Ok(());
    }

    #[cfg(windows)]
    {
        if powershell.is_file() {
            // Clear any partial files written by a failed `tar.exe` attempt because
            // PowerShell 5.1 / .NET Framework `ZipFile::ExtractToDirectory` throws
            // `IOException` whenever a destination file already exists.
            reset_extraction_staging_dir(dest_dir)?;
            let mut command = std::process::Command::new(powershell);
            command
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    "Add-Type -AssemblyName System.IO.Compression.FileSystem; [System.IO.Compression.ZipFile]::ExtractToDirectory($env:HLS_EXT_ZIP, $env:HLS_EXT_DEST)",
                ])
                .env("HLS_EXT_ZIP", zip_path)
                .env("HLS_EXT_DEST", dest_dir);
            configure_hidden_command(&mut command);
            let output = command
                .output()
                .map_err(|error| format!("调用系统解压组件失败: {error}"))?;
            if output.status.success() {
                return Ok(());
            }
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(format!("自动解压浏览器扩展失败：{}", detail.trim()));
        }
    }

    #[cfg(not(windows))]
    {
        reset_extraction_staging_dir(dest_dir)?;
        let mut command = std::process::Command::new("tar");
        command.arg("-xf").arg(zip_path).arg("-C").arg(dest_dir);
        configure_hidden_command(&mut command);
        if let Ok(output) = command.output() {
            if output.status.success() {
                return Ok(());
            }
        }
    }

    Err("未找到可用的系统 ZIP 解压工具（System32\\tar.exe）".into())
}

fn verify_extracted_tree(root: &Path) -> Result<(), String> {
    let root_meta = std::fs::symlink_metadata(root)
        .map_err(|error| format!("读取解压目录元数据失败: {error}"))?;
    if is_symlink_or_reparse_point(&root_meta) || !root_meta.is_dir() {
        return Err("解压目标目录类型异常".into());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|error| format!("遍历解压目录失败 {}: {error}", dir.display()))?;
        for entry_res in entries {
            let entry = entry_res.map_err(|error| format!("读取解压目录项失败: {error}"))?;
            visited += 1;
            if visited > MAX_EXTENSION_ENTRIES {
                return Err("解压目录文件数量超出安全上限".into());
            }
            let entry_path = entry.path();
            let entry_meta = std::fs::symlink_metadata(&entry_path)
                .map_err(|error| format!("读取解压文件类型失败: {error}"))?;
            if is_symlink_or_reparse_point(&entry_meta) {
                return Err("解压目录包含不安全的符号链接".into());
            }
            if entry_meta.is_dir() {
                pending.push(entry_path);
            } else if !entry_meta.is_file() {
                return Err("解压目录包含不支持的特殊文件".into());
            }
        }
    }
    Ok(())
}

fn normalize_entry_name(name: &OsStr) -> OsString {
    #[cfg(windows)]
    {
        match name.to_str() {
            Some(text) => OsString::from(text.to_lowercase()),
            None => name.to_os_string(),
        }
    }
    #[cfg(not(windows))]
    {
        name.to_os_string()
    }
}

fn prune_stale_entry(path: &Path, errors: &mut Vec<String>) {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            errors.push(format!("{} ({error})", path.display()));
            return;
        }
    };
    if is_symlink_or_reparse_point(&meta) {
        // Remove the link/junction entry itself without descending into its target.
        let remove_res = if cfg!(windows) && meta.is_dir() {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(error) = remove_res {
            errors.push(format!("{} ({error})", path.display()));
        }
        return;
    }
    if meta.is_dir() {
        if std::fs::remove_dir_all(path).is_ok() {
            return;
        }
        // Best-effort cleanup of unlocked entries inside a partially locked stale directory.
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                prune_stale_entry(&entry.path(), errors);
            }
        }
        if let Err(error) = std::fs::remove_dir(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!("{} ({error})", path.display()));
            }
        }
        return;
    }
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            errors.push(format!("{} ({error})", path.display()));
        }
    }
}

fn publish_extracted_directory(staging_dir: &Path, dest_dir: &Path) -> Result<(), String> {
    ensure_safe_publish_dest(dest_dir)?;
    if dest_dir.exists() {
        let _ = std::fs::remove_dir_all(dest_dir);
    }
    if !dest_dir.exists() && std::fs::rename(staging_dir, dest_dir).is_ok() {
        return Ok(());
    }
    // Fallback when Explorer or the browser holds an open directory handle on `dest_dir`.
    sync_directory_contents(staging_dir, dest_dir)
}

fn sync_directory_contents(src: &Path, dst: &Path) -> Result<(), String> {
    let mut prune_errors = Vec::new();
    sync_directory_contents_inner(src, dst, &mut prune_errors)?;
    if !prune_errors.is_empty() {
        return Err(format!(
            "清理过期扩展文件失败（部分文件可能正被占用）：{}",
            prune_errors.join("；")
        ));
    }
    Ok(())
}

fn sync_directory_contents_inner(
    src: &Path,
    dst: &Path,
    prune_errors: &mut Vec<String>,
) -> Result<(), String> {
    ensure_safe_publish_dest(dst)?;
    std::fs::create_dir_all(dst)
        .map_err(|error| format!("创建扩展目录失败 {}: {error}", dst.display()))?;
    ensure_safe_publish_dest(dst)?;

    let mut staged_entries = Vec::new();
    let mut expected_names = HashSet::new();
    let entries = std::fs::read_dir(src)
        .map_err(|error| format!("读取临时扩展目录失败 {}: {error}", src.display()))?;
    for entry_res in entries {
        let entry = entry_res.map_err(|error| format!("读取扩展条目失败: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取扩展条目类型失败: {error}"))?;
        let name = entry.file_name();
        expected_names.insert(normalize_entry_name(&name));
        staged_entries.push((name, entry.path(), file_type));
    }

    let existing_entries = std::fs::read_dir(dst)
        .map_err(|error| format!("读取现有扩展目录失败 {}: {error}", dst.display()))?;
    for existing_res in existing_entries {
        let existing = existing_res.map_err(|error| format!("读取现有扩展条目失败: {error}"))?;
        if !expected_names.contains(&normalize_entry_name(&existing.file_name())) {
            prune_stale_entry(&existing.path(), prune_errors);
        }
    }

    for (name, src_path, file_type) in staged_entries {
        let target = dst.join(&name);
        match std::fs::symlink_metadata(&target) {
            Ok(meta) => {
                if is_symlink_or_reparse_point(&meta) {
                    return Err(format!(
                        "扩展目标包含不安全的符号链接或重解析点，已拒绝覆盖：{}",
                        target.display()
                    ));
                }
                if file_type.is_dir() && !meta.is_dir() {
                    std::fs::remove_file(&target).map_err(|error| {
                        format!("清理冲突扩展文件失败 {}: {error}", target.display())
                    })?;
                } else if file_type.is_file() && meta.is_dir() {
                    std::fs::remove_dir_all(&target).map_err(|error| {
                        format!("清理冲突扩展目录失败 {}: {error}", target.display())
                    })?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "读取扩展目标条目元数据失败 {}: {error}",
                    target.display()
                ));
            }
        }
        if file_type.is_dir() {
            sync_directory_contents_inner(&src_path, &target, prune_errors)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &target)
                .map_err(|error| format!("更新扩展文件失败 {}: {error}", target.display()))?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn default_browser_prefers_chrome() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let subkey: Vec<u16> =
        "Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice\0"
            .encode_utf16()
            .collect();
    let name: Vec<u16> = "ProgId\0".encode_utf16().collect();
    let mut buffer = [0u16; 128];
    let mut byte_len = (buffer.len() * std::mem::size_of::<u16>()) as u32;
    // SAFETY: `HKEY_CURRENT_USER` is a predefined registry handle, `subkey` and `name` are
    // NUL-terminated UTF-16 buffers outliving the synchronous call, `RRF_RT_REG_SZ` restricts the
    // output to a NUL-terminated UTF-16 string, and `buffer`/`byte_len` describe writable stack memory.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut byte_len,
        )
    };
    if status != 0 {
        return false;
    }
    let len = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
        .to_ascii_lowercase()
        .starts_with("chromehtml")
}

#[cfg(not(windows))]
fn default_browser_prefers_chrome() -> bool {
    false
}

fn ordered_chromium_extension_candidates(
    roots: &[PathBuf],
    prefer_chrome: bool,
) -> Vec<(PathBuf, &'static str)> {
    let chrome_spec = (
        "Google/Chrome/Application/chrome.exe",
        "chrome://extensions",
    );
    let edge_spec = ("Microsoft/Edge/Application/msedge.exe", "edge://extensions");
    let ordered_specs = if prefer_chrome {
        [chrome_spec, edge_spec]
    } else {
        [edge_spec, chrome_spec]
    };
    let mut candidates = Vec::with_capacity(roots.len() * ordered_specs.len());
    for (rel_exe, page) in ordered_specs {
        for root in roots {
            candidates.push((root.join(rel_exe), page));
        }
    }
    candidates
}

fn open_extensions_page() -> Option<&'static str> {
    let prefer_chrome = default_browser_prefers_chrome();
    let roots: Vec<PathBuf> = ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    ordered_chromium_extension_candidates(&roots, prefer_chrome)
        .into_iter()
        .find_map(|(path, page)| {
            if !path.is_file() {
                return None;
            }
            std::process::Command::new(path)
                .arg(page)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()
                .map(|_| page)
        })
}

fn open_firefox_debugging_page() -> bool {
    let mut candidates = Vec::new();
    for root in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
    {
        candidates.push(root.join("Firefox Developer Edition/firefox.exe"));
        candidates.push(root.join("Mozilla Firefox/firefox.exe"));
        candidates.push(root.join("Firefox Nightly/firefox.exe"));
    }
    candidates.into_iter().any(|path| {
        path.is_file()
            && std::process::Command::new(path)
                .arg(FIREFOX_DEBUGGING_URL)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok()
    })
}

fn reveal_file_in_explorer(file_path: &Path) -> Result<(), String> {
    if !file_path.is_absolute() || !file_path.is_file() {
        return Err("只能定位已存在的本地文件".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            System::Com::{
                CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
            },
            UI::Shell::{
                Common::ITEMIDLIST, ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems,
            },
        };

        let wide_path: Vec<u16> = file_path.as_os_str().encode_wide().chain(Some(0)).collect();
        if !wide_path[..wide_path.len().saturating_sub(1)].contains(&0) {
            // SAFETY: Initializes or increments the calling thread's STA count using a null reserved
            // pointer and valid `COINIT` flags; `RPC_E_CHANGED_MODE` (`hr < 0`) is handled without uninitializing.
            let com_hr = unsafe {
                CoInitializeEx(
                    std::ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                )
            };
            struct ComApartmentGuard(bool);
            impl Drop for ComApartmentGuard {
                fn drop(&mut self) {
                    if self.0 {
                        // SAFETY: Executed only when `CoInitializeEx` returned `S_OK` or `S_FALSE` (`com_hr >= 0`),
                        // balancing the thread's COM initialization count after Shell calls complete.
                        unsafe {
                            CoUninitialize();
                        }
                    }
                }
            }
            let _com_guard = ComApartmentGuard(com_hr >= 0);

            struct PidlGuard(*mut ITEMIDLIST);
            impl Drop for PidlGuard {
                fn drop(&mut self) {
                    if !self.0.is_null() {
                        // SAFETY: `self.0` was allocated by `ILCreateFromPathW` and is freed exactly once here.
                        unsafe {
                            ILFree(self.0.cast_const());
                        }
                    }
                }
            }

            // SAFETY: `wide_path` is a NUL-terminated UTF-16 path without interior NULs outliving the call.
            let pidl = PidlGuard(unsafe { ILCreateFromPathW(wide_path.as_ptr()) });
            if !pidl.0.is_null() {
                // SAFETY: Per `SHOpenFolderAndSelectItems` contract, passing a valid full item `pidl`
                // with `cidl == 0` and `apidl == null` opens its parent folder in Explorer with that item selected.
                let select_hr = unsafe {
                    SHOpenFolderAndSelectItems(pidl.0.cast_const(), 0, std::ptr::null(), 0)
                };
                if select_hr >= 0 {
                    return Ok(());
                }
            }
        }
    }
    let parent = file_path
        .parent()
        .ok_or_else(|| "无法读取扩展包所在文件夹".to_string())?;
    file_dialogs::open(&parent.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordedOps {
        opened_targets: RefCell<Vec<String>>,
        revealed_files: RefCell<Vec<PathBuf>>,
        clipboard: RefCell<Vec<String>>,
        chromium_pages_opened: Cell<u32>,
        firefox_pages_opened: Cell<u32>,
    }

    impl RecordedOps {
        fn new() -> Rc<Self> {
            Rc::new(Self {
                opened_targets: RefCell::new(Vec::new()),
                revealed_files: RefCell::new(Vec::new()),
                clipboard: RefCell::new(Vec::new()),
                chromium_pages_opened: Cell::new(0),
                firefox_pages_opened: Cell::new(0),
            })
        }

        fn hooks(
            self: &Rc<Self>,
            install_root: Option<PathBuf>,
            local_app_data: Option<PathBuf>,
            current_dir: Option<PathBuf>,
        ) -> SystemHooks {
            let opened = Rc::clone(self);
            let revealed = Rc::clone(self);
            let clip = Rc::clone(self);
            let chromium = Rc::clone(self);
            let firefox = Rc::clone(self);
            SystemHooks {
                install_root: Box::new(move || install_root.clone()),
                local_app_data: Box::new(move || local_app_data.clone()),
                current_dir: Box::new(move || current_dir.clone()),
                open_target: Box::new(move |target| {
                    opened.opened_targets.borrow_mut().push(target.to_owned());
                    Ok(())
                }),
                reveal_file: Box::new(move |path| {
                    revealed
                        .revealed_files
                        .borrow_mut()
                        .push(path.to_path_buf());
                    Ok(())
                }),
                copy_clipboard: Box::new(move |text| {
                    clip.clipboard.borrow_mut().push(text.to_owned());
                    Ok(())
                }),
                open_chromium_extensions: Box::new(move || {
                    chromium
                        .chromium_pages_opened
                        .set(chromium.chromium_pages_opened.get() + 1);
                    Some("edge://extensions")
                }),
                open_firefox_debugging: Box::new(move || {
                    firefox
                        .firefox_pages_opened
                        .set(firefox.firefox_pages_opened.get() + 1);
                    true
                }),
            }
        }
    }

    struct TempTestDir(PathBuf);

    impl TempTestDir {
        fn new(label: &str) -> Self {
            static NEXT_DIR: AtomicU64 = AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "hls-system-tools-{label}-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn crc32_ieee(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Builds a valid uncompressed (Store) ZIP archive with independent local and central names
    /// so tests can verify both legitimate `tar.exe` unpacking and dual-header attack rejection.
    fn build_test_zip(entries: &[(&str, &str, &[u8])]) -> Vec<u8> {
        let mut archive = Vec::new();
        let mut central = Vec::new();
        for &(local_name, central_name, content) in entries {
            let local_offset = archive.len() as u32;
            let crc = crc32_ieee(content);
            let size = content.len() as u32;
            let local_bytes = local_name.as_bytes();
            archive.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            archive.extend_from_slice(&20u16.to_le_bytes()); // version needed
            archive.extend_from_slice(&0u16.to_le_bytes()); // flags
            archive.extend_from_slice(&0u16.to_le_bytes()); // method: Store
            archive.extend_from_slice(&0u16.to_le_bytes()); // mtime
            archive.extend_from_slice(&0u16.to_le_bytes()); // mdate
            archive.extend_from_slice(&crc.to_le_bytes());
            archive.extend_from_slice(&size.to_le_bytes());
            archive.extend_from_slice(&size.to_le_bytes());
            archive.extend_from_slice(&(local_bytes.len() as u16).to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes()); // extra len
            archive.extend_from_slice(local_bytes);
            archive.extend_from_slice(content);

            let central_bytes = central_name.as_bytes();
            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by (DOS)
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // method: Store
            central.extend_from_slice(&0u16.to_le_bytes()); // mtime
            central.extend_from_slice(&0u16.to_le_bytes()); // mdate
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(central_bytes.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra len
            central.extend_from_slice(&0u16.to_le_bytes()); // comment len
            central.extend_from_slice(&0u16.to_le_bytes()); // disk start
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&local_offset.to_le_bytes());
            central.extend_from_slice(central_bytes);
        }
        let cd_offset = archive.len() as u32;
        let cd_size = central.len() as u32;
        archive.extend_from_slice(&central);
        archive.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        archive.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        archive.extend_from_slice(&cd_size.to_le_bytes());
        archive.extend_from_slice(&cd_offset.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive
    }

    fn chromium_manifest_json(version: &str) -> Vec<u8> {
        serde_json::json!({
            "manifest_version": 3,
            "name": "HLS Downloader",
            "version": version,
            "key": "test-chromium-public-key",
            "action": { "default_popup": "popup.html" }
        })
        .to_string()
        .into_bytes()
    }

    fn firefox_manifest_json(version: &str) -> Vec<u8> {
        serde_json::json!({
            "manifest_version": 3,
            "name": "HLS Downloader",
            "version": version,
            "action": { "default_popup": "popup.html" },
            "browser_specific_settings": {
                "gecko": { "id": "hls-downloader-store@ciaooo55.com" }
            }
        })
        .to_string()
        .into_bytes()
    }

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

    #[test]
    fn packaged_chromium_zip_auto_unpacks_to_localappdata_and_guides_user() {
        crate::offscreen::OffscreenPlatform::init_test();
        let sandbox = TempTestDir::new("chromium-unpack");
        // Simulate MSI/Portable layout where presenter lives in `<root>/app/resources`
        // and extension archive ships at `<root>/app/resources/extensions/HLSDownloader-<ver>-Chromium.zip`.
        let presenter_dir = sandbox
            .path()
            .join("HLSDownloader")
            .join("app")
            .join("resources");
        let ext_dir = presenter_dir.join("extensions");
        let local_app_data = sandbox.path().join("LocalAppData");
        let empty_cwd = sandbox.path().join("empty-cwd");
        std::fs::create_dir_all(&ext_dir).unwrap();
        std::fs::create_dir_all(&local_app_data).unwrap();
        std::fs::create_dir_all(&empty_cwd).unwrap();

        let version = env!("CARGO_PKG_VERSION");
        let manifest = chromium_manifest_json(version);
        let zip_bytes = build_test_zip(&[
            ("manifest.json", "manifest.json", &manifest),
            (
                "popup.html",
                "popup.html",
                b"<!doctype html><title>HLS</title>",
            ),
            ("chunks/bg.js", "chunks/bg.js", b"console.log('ready');"),
        ]);
        let zip_path = ext_dir.join(format!("HLSDownloader-{version}-Chromium.zip"));
        std::fs::write(&zip_path, zip_bytes).unwrap();

        let recorded = RecordedOps::new();
        let next = Cell::new(0);
        let tools = SystemTools::new_with_hooks(
            WorkbenchWindow::new().unwrap(),
            Rc::new(|_| true),
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
            recorded.hooks(
                Some(presenter_dir),
                Some(local_app_data.clone()),
                Some(empty_cwd),
            ),
        )
        .unwrap();

        tools.show("browser");
        tools.action("chromium");

        let expected_unpacked = local_app_data
            .join("HLS Downloader")
            .join("extension-unpacked")
            .join(version)
            .join("Chromium");
        assert!(expected_unpacked.join("manifest.json").is_file());
        assert!(expected_unpacked.join("popup.html").is_file());
        assert!(expected_unpacked.join("chunks").join("bg.js").is_file());

        let expected_str = expected_unpacked.to_string_lossy().into_owned();
        assert_eq!(
            recorded.opened_targets.borrow().as_slice(),
            [expected_str.as_str()]
        );
        assert_eq!(
            recorded.clipboard.borrow().as_slice(),
            [expected_str.as_str()]
        );
        assert_eq!(recorded.chromium_pages_opened.get(), 1);
        assert_eq!(tools.notice_tone.borrow().as_str(), "success");
        assert!(
            tools.notice.borrow().contains(CHROMIUM_UNPACKED_GUIDE),
            "unexpected notice: {}",
            tools.notice.borrow()
        );
        assert!(tools.handoffs.borrow().contains(&expected_str));
        assert!(tools.handoffs.borrow().contains("开发者模式"));

        // Clicking a second time reuses the verified unpacked folder cleanly.
        tools.action("chromium");
        assert_eq!(recorded.chromium_pages_opened.get(), 2);
        assert_eq!(tools.notice_tone.borrow().as_str(), "success");
        tools.close();
    }

    #[test]
    fn extension_zip_validator_rejects_traversal_backslashes_mismatch_and_bad_manifest() {
        let valid_manifest = chromium_manifest_json(env!("CARGO_PKG_VERSION"));

        // 1. Parent traversal (`..`) rejected before extraction.
        let traversal = build_test_zip(&[
            ("manifest.json", "manifest.json", &valid_manifest),
            ("../evil.dll", "../evil.dll", b"bad"),
        ]);
        assert!(validate_extension_zip_entries(&traversal).is_err());

        // 2. Backslash entry path rejected.
        let backslash = build_test_zip(&[
            ("manifest.json", "manifest.json", &valid_manifest),
            ("chunks\\bg.js", "chunks\\bg.js", b"bad"),
        ]);
        assert!(validate_extension_zip_entries(&backslash).is_err());

        // 3. Drive letter / reserved device name rejected.
        let reserved = build_test_zip(&[
            ("manifest.json", "manifest.json", &valid_manifest),
            ("NUL.txt", "NUL.txt", b"bad"),
        ]);
        assert!(validate_extension_zip_entries(&reserved).is_err());

        // 4. Dual-header spoofing (Central Directory says safe, Local Header says `../evil.dll`) rejected.
        let spoofed = build_test_zip(&[
            ("manifest.json", "manifest.json", &valid_manifest),
            ("../evil.dll", "safe.js", b"bad"),
        ]);
        assert!(validate_extension_zip_entries(&spoofed).is_err());

        // 5. Missing root `manifest.json` rejected.
        let no_manifest = build_test_zip(&[("popup.html", "popup.html", b"hi")]);
        assert!(validate_extension_zip_entries(&no_manifest).is_err());

        // 6. Wrong manifest version fails post-unpack validation and cleans up staging.
        let sandbox = TempTestDir::new("bad-version");
        let ext_dir = sandbox.path().join("extensions");
        let local_app_data = sandbox.path().join("LocalAppData");
        std::fs::create_dir_all(&ext_dir).unwrap();
        let wrong_manifest = chromium_manifest_json("0.0.1");
        let wrong_zip = build_test_zip(&[
            ("manifest.json", "manifest.json", &wrong_manifest),
            ("popup.html", "popup.html", b"hi"),
        ]);
        std::fs::write(ext_dir.join("Chromium.zip"), wrong_zip).unwrap();
        let err = resolve_chromium_extension(
            Some(sandbox.path()),
            Some(&local_app_data),
            None,
            env!("CARGO_PKG_VERSION"),
        )
        .unwrap_err();
        assert!(err.contains("manifest.json"));
    }

    #[test]
    fn firefox_action_reveals_bundled_zip_and_opens_debugging_without_amo_url() {
        crate::offscreen::OffscreenPlatform::init_test();
        let sandbox = TempTestDir::new("firefox-zip");
        // Simulate Portable top-level `<portable>/extensions/HLSDownloader-<ver>-Firefox.zip`
        // reached from presenter in `<portable>/app/resources`.
        let portable_root = sandbox.path().join("HLSDownloader");
        let presenter_dir = portable_root.join("app").join("resources");
        let ext_dir = portable_root.join("extensions");
        let empty_cwd = sandbox.path().join("empty-cwd");
        std::fs::create_dir_all(&presenter_dir).unwrap();
        std::fs::create_dir_all(&ext_dir).unwrap();
        std::fs::create_dir_all(&empty_cwd).unwrap();

        let version = env!("CARGO_PKG_VERSION");
        let ff_manifest = firefox_manifest_json(version);
        let zip_bytes = build_test_zip(&[
            ("manifest.json", "manifest.json", &ff_manifest),
            ("popup.html", "popup.html", b"<!doctype html>"),
        ]);
        let ff_zip = ext_dir.join(format!("HLSDownloader-{version}-Firefox.zip"));
        std::fs::write(&ff_zip, zip_bytes).unwrap();

        let recorded = RecordedOps::new();
        let next = Cell::new(0);
        let tools = SystemTools::new_with_hooks(
            WorkbenchWindow::new().unwrap(),
            Rc::new(|_| true),
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
            recorded.hooks(Some(presenter_dir), None, Some(empty_cwd)),
        )
        .unwrap();

        tools.action("firefox");

        assert!(
            recorded
                .opened_targets
                .borrow()
                .iter()
                .all(|url| !url.contains("addons.mozilla.org")),
            "must never open unshipped AMO store URL"
        );
        assert_eq!(
            recorded.revealed_files.borrow().as_slice(),
            std::slice::from_ref(&ff_zip)
        );
        assert_eq!(recorded.firefox_pages_opened.get(), 1);
        assert_eq!(tools.notice_tone.borrow().as_str(), "success");
        assert!(tools.notice.borrow().contains(FIREFOX_DEBUGGING_URL));
        assert!(tools.handoffs.borrow().contains("Mozilla 签名"));
    }

    #[test]
    fn unsigned_build_disables_in_app_download_and_opens_github_release_page() {
        crate::offscreen::OffscreenPlatform::init_test();
        let recorded = RecordedOps::new();
        let sent = Rc::new(RefCell::new(Vec::<CorePipeRequest>::new()));
        let sent_ref = Rc::clone(&sent);
        let next = Cell::new(0);
        let tools = SystemTools::new_with_hooks(
            WorkbenchWindow::new().unwrap(),
            Rc::new(move |req| {
                sent_ref.borrow_mut().push(req);
                true
            }),
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
            recorded.hooks(None, None, None),
        )
        .unwrap();
        tools.signing_configured.set(false);

        let release_url = "https://github.com/ciaooo55/hls-downloader/releases/tag/v10.2.0";
        tools.event(&EventEnvelope {
            sequence: 11,
            event: CoreEvent::UpdateAvailable {
                current: "10.1.0".into(),
                latest: "10.2.0".into(),
                notes: "Bug fixes".into(),
                release_url: release_url.into(),
                installer_name: "HLSDownloader-10.2.0-Windows-x64.msi".into(),
                installer_size: 154_453_055,
                sha256_verified: true,
            },
        });

        assert!(!tools.can_download.get());
        assert!(!tools.can_install.get());
        assert!(tools.can_open_release.get());
        let window = tools.window.borrow().as_ref().unwrap().clone_strong();
        assert!(!window.get_can_download());
        assert!(window.get_can_open_release());
        assert!(tools.update.borrow().contains("未嵌入代码签名证书"));
        assert!(tools.update.borrow().contains(release_url));

        // Attempting `download_update` on an unsigned build must never dispatch `DownloadUpdate`.
        tools.action("download_update");
        assert!(sent.borrow().is_empty());

        // Clicking `open_release` opens the retained GitHub Release page in the user's browser.
        tools.action("open_release");
        assert_eq!(recorded.opened_targets.borrow().as_slice(), [release_url]);
        assert_eq!(tools.notice_tone.borrow().as_str(), "success");
        tools.close();
    }

    #[test]
    fn release_url_validation_accepts_only_https_github_urls() {
        assert_eq!(
            sanitize_github_release_url(
                "https://github.com/ciaooo55/hls-downloader/releases/tag/v10.2.0"
            )
            .as_deref(),
            Some("https://github.com/ciaooo55/hls-downloader/releases/tag/v10.2.0")
        );
        assert_eq!(
            sanitize_github_release_url(
                "  HTTPS://GitHub.com/ciaooo55/hls-downloader/releases/latest  "
            )
            .as_deref(),
            Some("https://github.com/ciaooo55/hls-downloader/releases/latest")
        );
        for rejected in [
            "",
            "http://github.com/ciaooo55/hls-downloader/releases/latest",
            "https://github.com/",
            "https://github.com.evil.test/ciaooo55/hls-downloader",
            "https://github.com@evil.test/ciaooo55/hls-downloader",
            "https://github.com:8443/ciaooo55/hls-downloader/releases",
            "https://github.com/ciaooo55/../evil/releases",
            "https://github.com/ciaooo55/%2e%2e/evil",
            "https://evil.example/HLSDownloader.exe",
            "file:///C:/Windows/System32/calc.exe",
        ] {
            assert_eq!(
                sanitize_github_release_url(rejected),
                None,
                "expected {rejected} to be rejected"
            );
        }
        assert!(is_valid_signing_thumbprint(Some(
            "00112233445566778899aabbccddeeff00112233"
        )));
        assert!(!is_valid_signing_thumbprint(None));
        assert!(!is_valid_signing_thumbprint(Some("")));
        assert!(!is_valid_signing_thumbprint(Some("not-hex")));
    }

    #[test]
    fn signed_build_preserves_download_install_flow_and_recovers_on_winverifytrust_error() {
        crate::offscreen::OffscreenPlatform::init_test();
        let recorded = RecordedOps::new();
        let sent = Rc::new(RefCell::new(Vec::<CorePipeRequest>::new()));
        let sent_ref = Rc::clone(&sent);
        let next = Cell::new(0);
        let tools = SystemTools::new_with_hooks(
            WorkbenchWindow::new().unwrap(),
            Rc::new(move |req| {
                sent_ref.borrow_mut().push(req);
                true
            }),
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
            recorded.hooks(None, None, None),
        )
        .unwrap();
        tools.signing_configured.set(true);

        let release_url = "https://github.com/ciaooo55/hls-downloader/releases/tag/v10.2.0";
        tools.event(&EventEnvelope {
            sequence: 30,
            event: CoreEvent::UpdateAvailable {
                current: "10.1.0".into(),
                latest: "10.2.0".into(),
                notes: "Signed release".into(),
                release_url: release_url.into(),
                installer_name: "HLSDownloader-10.2.0-Windows-x64.msi".into(),
                installer_size: 120_000_000,
                sha256_verified: true,
            },
        });
        assert!(tools.can_download.get());
        assert!(tools.can_open_release.get());

        // Signed build dispatches `DownloadUpdate`.
        tools.action("download_update");
        assert_eq!(sent.borrow().len(), 1);
        let req_id = match &sent.borrow()[0] {
            CorePipeRequest::Command {
                request_id,
                command: CoreCommand::DownloadUpdate,
            } => *request_id,
            other => panic!("expected DownloadUpdate, got {other:?}"),
        };

        // Simulate `WinVerifyTrust 0x800B0100` failure on the downloaded installer.
        tools.response(&CorePipeResponse::Error {
            request_id: Some(req_id),
            code: "update_failed".into(),
            message: "升级安装包 Authenticode 信任校验失败（WinVerifyTrust 0x800B0100）".into(),
        });
        assert!(!tools.busy.get());
        assert!(!tools.can_download.get());
        assert!(!tools.can_install.get());
        assert!(tools.can_open_release.get());
        assert!(tools.update.borrow().contains("代码签名校验未通过"));
        assert!(tools.update.borrow().contains(release_url));

        // User can immediately open the retained GitHub Release page to upgrade manually.
        tools.action("open_release");
        assert_eq!(recorded.opened_targets.borrow().as_slice(), [release_url]);

        // Also verify normal `UpdateReady` -> `can_install` flow still works when verification passes.
        tools.event(&EventEnvelope {
            sequence: 31,
            event: CoreEvent::UpdateReady {
                latest: "10.2.0".into(),
                installer_path: r"C:\Temp\HLSDownloader-10.2.0-Windows-x64.msi".into(),
                sha256: "a".repeat(64),
                product_name: "HLS Downloader".into(),
                product_version: "10.2.0".into(),
                upgrade_code: "{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}".into(),
            },
        });
        assert!(tools.can_install.get());
        assert!(!tools.can_download.get());
        tools.close();
    }

    #[test]
    fn legal_docs_renders_terms_and_privacy_inline_without_tripping_file_dialogs_open_guard() {
        crate::offscreen::OffscreenPlatform::init_test();
        let sandbox = TempTestDir::new("legal-docs-live-open");
        let app_root = sandbox.path().join("HLSDownloader");
        let presenter_dir = app_root.join("app").join("resources");
        std::fs::create_dir_all(&presenter_dir).unwrap();
        std::fs::write(app_root.join("TERMS.md"), "# 随包用户协议测试正文\n条款一").unwrap();
        std::fs::write(
            app_root.join("PRIVACY.md"),
            "# 随包隐私政策测试正文\n隐私一",
        )
        .unwrap();

        let next = Cell::new(0);
        let hooks = SystemHooks {
            install_root: Box::new(move || Some(presenter_dir.clone())),
            local_app_data: Box::new(|| None),
            current_dir: Box::new(|| None),
            // Wire the REAL production `file_dialogs::open` guard (which rejects plain files)!
            open_target: Box::new(file_dialogs::open),
            reveal_file: Box::new(|_| Ok(())),
            copy_clipboard: Box::new(|_| Ok(())),
            open_chromium_extensions: Box::new(|| None),
            open_firefox_debugging: Box::new(|| false),
        };
        let tools = SystemTools::new_with_hooks(
            WorkbenchWindow::new().unwrap(),
            Rc::new(|_| true),
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
            hooks,
        )
        .unwrap();

        tools.show("about");
        let window = tools.window.borrow().as_ref().unwrap().clone_strong();
        assert!(window
            .get_content()
            .contains(hls_client::LEGAL_TERMS_VERSION));

        // Clicking `legal_docs` renders both TERMS.md and PRIVACY.md inline with zero error!
        tools.action("legal_docs");
        assert_eq!(tools.notice_tone.borrow().as_str(), "success");
        let legal_content = window.get_content().to_string();
        assert!(legal_content.contains("随包用户协议测试正文"));
        assert!(legal_content.contains("随包隐私政策测试正文"));

        // Clicking `legal_docs` again returns to the About overview cleanly.
        tools.action("legal_docs");
        assert_eq!(tools.notice_tone.borrow().as_str(), "info");
        assert!(window.get_content().contains("配置与任务目录"));
        assert!(!window.get_content().contains("随包用户协议测试正文"));

        // Embedded fallback works even when no TERMS.md / PRIVACY.md exist on disk.
        let fallback_text = load_legal_and_privacy_text(None, None);
        assert!(fallback_text.contains("HLS Downloader 用户协议与免责声明"));
        assert!(fallback_text.contains("HLS Downloader 隐私政策"));
        tools.close();
    }

    #[test]
    fn chromium_browser_candidate_order_exhausts_preferred_browser_across_all_roots_first_and_stamp_commits_last(
    ) {
        let sandbox = TempTestDir::new("browser-order-and-stamp");
        let pf64 = sandbox.path().join("ProgramFiles");
        let pfx86 = sandbox.path().join("ProgramFilesX86");
        let local = sandbox.path().join("LocalAppData");

        // Simulate 64-bit Windows where x86 Edge exists in Program Files (x86)
        // while Chrome is installed in 64-bit Program Files or LocalAppData!
        let x86_edge = pfx86.join("Microsoft/Edge/Application/msedge.exe");
        let local_chrome = local.join("Google/Chrome/Application/chrome.exe");
        std::fs::create_dir_all(x86_edge.parent().unwrap()).unwrap();
        std::fs::create_dir_all(local_chrome.parent().unwrap()).unwrap();
        std::fs::write(&x86_edge, b"edge").unwrap();
        std::fs::write(&local_chrome, b"chrome").unwrap();

        let roots = vec![pf64, pfx86, local];
        let chrome_preferred = ordered_chromium_extension_candidates(&roots, true);
        let first_existing_when_chrome_preferred = chrome_preferred
            .iter()
            .find(|(path, _)| path.is_file())
            .unwrap();
        assert_eq!(
            first_existing_when_chrome_preferred.1,
            "chrome://extensions"
        );

        let edge_preferred = ordered_chromium_extension_candidates(&roots, false);
        let first_existing_when_edge_preferred = edge_preferred
            .iter()
            .find(|(path, _)| path.is_file())
            .unwrap();
        assert_eq!(first_existing_when_edge_preferred.1, "edge://extensions");

        // Verify `.archive-stamp` is removed before publish and written ONLY if post-publish validation passes.
        let version = env!("CARGO_PKG_VERSION");
        let manifest = chromium_manifest_json(version);
        let zip_bytes = build_test_zip(&[
            ("manifest.json", "manifest.json", &manifest),
            ("popup.html", "popup.html", b"<!doctype html>"),
        ]);
        let zip_path = sandbox.path().join("Chromium.zip");
        std::fs::write(&zip_path, &zip_bytes).unwrap();
        let dest_dir = sandbox.path().join("unpacked").join("Chromium");
        unpack_chromium_extension_archive(&zip_path, &dest_dir, version).unwrap();
        assert!(dest_dir.join(".archive-stamp").is_file());

        // Hold an exclusive (`share_mode(0)`) handle on `popup.html` inside `dest_dir` so neither
        // `remove_dir_all(dest_dir)` nor `copy_directory_contents` can replace `popup.html`;
        // stale `.archive-stamp` MUST be removed before publish and never recreated on failure!
        std::fs::write(dest_dir.join(".archive-stamp"), "stale-stamp").unwrap();
        #[cfg(windows)]
        let _exclusive_lock = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(dest_dir.join("popup.html"))
                .unwrap()
        };
        let err = unpack_chromium_extension_archive(&zip_path, &dest_dir, version);
        assert!(err.is_err());
        assert!(
            !dest_dir.join(".archive-stamp").exists(),
            ".archive-stamp must not remain after a failed fallback publish"
        );
        #[cfg(windows)]
        drop(_exclusive_lock);
    }

    struct CustomZipEntry<'a> {
        local_name: &'a str,
        central_name: &'a str,
        content: &'a [u8],
        version_made_by: u16,
        local_flags: u16,
        central_flags: u16,
        external_attrs: u32,
        disk_start: u16,
    }

    impl<'a> CustomZipEntry<'a> {
        fn plain(name: &'a str, content: &'a [u8]) -> Self {
            Self {
                local_name: name,
                central_name: name,
                content,
                version_made_by: 20,
                local_flags: 0,
                central_flags: 0,
                external_attrs: 0,
                disk_start: 0,
            }
        }
    }

    fn build_custom_test_zip(
        entries: &[CustomZipEntry<'_>],
        eocd_entries_on_disk_override: Option<u16>,
    ) -> Vec<u8> {
        let mut archive = Vec::new();
        let mut central = Vec::new();
        for entry in entries {
            let local_offset = archive.len() as u32;
            let crc = crc32_ieee(entry.content);
            let size = entry.content.len() as u32;
            let local_bytes = entry.local_name.as_bytes();
            archive.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            archive.extend_from_slice(&20u16.to_le_bytes());
            archive.extend_from_slice(&entry.local_flags.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&crc.to_le_bytes());
            archive.extend_from_slice(&size.to_le_bytes());
            archive.extend_from_slice(&size.to_le_bytes());
            archive.extend_from_slice(&(local_bytes.len() as u16).to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(local_bytes);
            archive.extend_from_slice(entry.content);

            let central_bytes = entry.central_name.as_bytes();
            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&entry.version_made_by.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&entry.central_flags.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(central_bytes.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&entry.disk_start.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&entry.external_attrs.to_le_bytes());
            central.extend_from_slice(&local_offset.to_le_bytes());
            central.extend_from_slice(central_bytes);
        }
        let cd_offset = archive.len() as u32;
        let cd_size = central.len() as u32;
        let count = entries.len() as u16;
        let entries_on_disk = eocd_entries_on_disk_override.unwrap_or(count);
        archive.extend_from_slice(&central);
        archive.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&entries_on_disk.to_le_bytes());
        archive.extend_from_slice(&count.to_le_bytes());
        archive.extend_from_slice(&cd_size.to_le_bytes());
        archive.extend_from_slice(&cd_offset.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive
    }

    fn create_test_dir_link(link: &Path, target: &Path) -> bool {
        #[cfg(windows)]
        {
            let mut cmd = std::process::Command::new("cmd");
            cmd.args(["/C", "mklink", "/J"]).arg(link).arg(target);
            configure_hidden_command(&mut cmd);
            cmd.output().is_ok_and(|out| out.status.success())
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    fn remove_test_dir_link(link: &Path) {
        #[cfg(windows)]
        {
            let _ = std::fs::remove_dir(link);
        }
        #[cfg(not(windows))]
        {
            let _ = std::fs::remove_file(link);
        }
    }

    #[test]
    fn extension_zip_validator_rejects_darwin_symlinks_vfat_reparse_eocd_mismatch_and_lfh_encryption(
    ) {
        let valid_manifest = chromium_manifest_json(env!("CARGO_PKG_VERSION"));

        // 1. macOS/Darwin (`creator_os = 19`) POSIX symlink rejected.
        let mut darwin_link = CustomZipEntry::plain("link.js", b"../target");
        darwin_link.version_made_by = (19 << 8) | 20;
        darwin_link.external_attrs = 0o120_777u32 << 16;
        let zip = build_custom_test_zip(
            &[
                CustomZipEntry::plain("manifest.json", &valid_manifest),
                darwin_link,
            ],
            None,
        );
        let err = validate_extension_zip_entries(&zip).unwrap_err();
        assert!(err.contains("符号链接"), "err={err}");

        // 2. Non-UNIX creator_os (`0`) carrying POSIX S_IFLNK mode bits also rejected unconditionally.
        let mut dos_posix_link = CustomZipEntry::plain("link2.js", b"../target");
        dos_posix_link.version_made_by = 20;
        dos_posix_link.external_attrs = 0o120_777u32 << 16;
        let zip = build_custom_test_zip(
            &[
                CustomZipEntry::plain("manifest.json", &valid_manifest),
                dos_posix_link,
            ],
            None,
        );
        assert!(validate_extension_zip_entries(&zip).is_err());

        // 3. VFAT (`creator_os = 14`) reparse point bit (`0x0400`) rejected.
        let mut vfat_reparse = CustomZipEntry::plain("reparse.js", b"x");
        vfat_reparse.version_made_by = (14 << 8) | 20;
        vfat_reparse.external_attrs = 0x0400;
        let zip = build_custom_test_zip(
            &[
                CustomZipEntry::plain("manifest.json", &valid_manifest),
                vfat_reparse,
            ],
            None,
        );
        let err = validate_extension_zip_entries(&zip).unwrap_err();
        assert!(err.contains("重解析点"), "err={err}");

        // 4. EOCD `entries_on_disk != total_entries` rejected.
        let eocd_mismatch = build_custom_test_zip(
            &[
                CustomZipEntry::plain("manifest.json", &valid_manifest),
                CustomZipEntry::plain("popup.html", b"<!doctype html>"),
            ],
            Some(1),
        );
        assert!(validate_extension_zip_entries(&eocd_mismatch).is_err());

        // 5. LFH encrypted bit (`flags & 0x0001 != 0`) while CD flags == 0 rejected.
        let mut lfh_encrypted = CustomZipEntry::plain("popup.html", b"secret");
        lfh_encrypted.local_flags = 0x0001;
        lfh_encrypted.central_flags = 0;
        let zip = build_custom_test_zip(
            &[
                CustomZipEntry::plain("manifest.json", &valid_manifest),
                lfh_encrypted,
            ],
            None,
        );
        let err = validate_extension_zip_entries(&zip).unwrap_err();
        assert!(err.contains("加密"), "err={err}");

        // 6. Legitimate Darwin regular file and VFAT regular file are accepted.
        let mut darwin_regular = CustomZipEntry::plain("popup.html", b"<!doctype html>");
        darwin_regular.version_made_by = (19 << 8) | 20;
        darwin_regular.external_attrs = 0o100_644u32 << 16;
        let mut vfat_regular = CustomZipEntry::plain("chunks/bg.js", b"console.log(1);");
        vfat_regular.version_made_by = (14 << 8) | 20;
        vfat_regular.external_attrs = 0x0020; // FILE_ATTRIBUTE_ARCHIVE
        let ok_zip = build_custom_test_zip(
            &[
                CustomZipEntry::plain("manifest.json", &valid_manifest),
                darwin_regular,
                vfat_regular,
            ],
            None,
        );
        assert!(validate_extension_zip_entries(&ok_zip).is_ok());
    }

    #[test]
    fn extract_zip_archive_fallback_clears_partial_tar_leftovers_and_rejects_reparse_dest() {
        let sandbox = TempTestDir::new("zip-fallback-cleanup");
        let version = env!("CARGO_PKG_VERSION");
        let manifest = chromium_manifest_json(version);
        let zip_bytes = build_test_zip(&[
            ("manifest.json", "manifest.json", &manifest),
            ("popup.html", "popup.html", b"<!doctype html><h1>ok</h1>"),
            ("chunks/bg.js", "chunks/bg.js", b"console.log('clean');"),
        ]);
        let zip_path = sandbox.path().join("Chromium.zip");
        std::fs::write(&zip_path, &zip_bytes).unwrap();

        // Simulate `tar.exe` failing after writing a conflicting `manifest.json` and extra partial file.
        let dest_dir = sandbox.path().join("staging");
        std::fs::create_dir_all(&dest_dir).unwrap();

        #[cfg(windows)]
        let powershell = system_root_dir()
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");

        extract_zip_archive_with(
            &zip_path,
            &dest_dir,
            |_, dest| {
                std::fs::write(dest.join("manifest.json"), b"partial-corrupt").unwrap();
                std::fs::write(dest.join("tar-partial.tmp"), b"leftover").unwrap();
                false
            },
            #[cfg(windows)]
            powershell.as_path(),
        )
        .expect("fallback extraction should clear partial leftovers and succeed");

        assert!(
            !dest_dir.join("tar-partial.tmp").exists(),
            "stale partial file left by failed tar must be cleared before fallback"
        );
        assert_eq!(
            std::fs::read(dest_dir.join("manifest.json")).unwrap(),
            manifest
        );
        assert_eq!(
            std::fs::read(dest_dir.join("popup.html")).unwrap(),
            b"<!doctype html><h1>ok</h1>"
        );
        assert_eq!(
            std::fs::read(dest_dir.join("chunks/bg.js")).unwrap(),
            b"console.log('clean');"
        );

        // `reset_extraction_staging_dir` must refuse to follow/delete a symlink or NTFS junction.
        let victim = sandbox.path().join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("keep.txt"), b"safe").unwrap();
        let link_dest = sandbox.path().join("staging-link");
        if create_test_dir_link(&link_dest, &victim) {
            let err = reset_extraction_staging_dir(&link_dest).unwrap_err();
            assert!(err.contains("类型异常"), "err={err}");
            assert!(
                victim.join("keep.txt").is_file(),
                "victim directory must remain untouched"
            );
            remove_test_dir_link(&link_dest);
        }
    }

    #[test]
    fn publish_extracted_directory_fallback_prunes_stale_files_verifies_tree_and_rejects_link_dest()
    {
        let sandbox = TempTestDir::new("publish-fallback-prune");
        let version = env!("CARGO_PKG_VERSION");
        let manifest = chromium_manifest_json(version);
        let zip_bytes = build_test_zip(&[
            ("manifest.json", "manifest.json", &manifest),
            ("popup.html", "popup.html", b"<!doctype html>new"),
            ("chunks/bg.js", "chunks/bg.js", b"console.log('v2');"),
        ]);
        let zip_path = sandbox.path().join("Chromium.zip");
        std::fs::write(&zip_path, &zip_bytes).unwrap();

        let dest_dir = sandbox.path().join("unpacked").join("Chromium");
        std::fs::create_dir_all(dest_dir.join("chunks")).unwrap();
        std::fs::create_dir_all(dest_dir.join("legacy_dir")).unwrap();
        std::fs::write(dest_dir.join("manifest.json"), b"old-manifest").unwrap();
        std::fs::write(dest_dir.join("obsolete.js"), b"stale-root").unwrap();
        std::fs::write(dest_dir.join("chunks/old-chunk.js"), b"stale-chunk").unwrap();
        std::fs::write(dest_dir.join("legacy_dir/old.txt"), b"stale-dir").unwrap();

        // On Windows, open `dest_dir` with backup semantics and WITHOUT `FILE_SHARE_DELETE`
        // (same lock Explorer holds when viewing the folder), forcing `publish_extracted_directory`
        // into the fallback overlay-copy path while allowing files inside to be updated & pruned.
        #[cfg(windows)]
        let _dir_handle = {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            const FILE_SHARE_WRITE: u32 = 0x0000_0002;
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(&dest_dir)
                .unwrap()
        };

        #[cfg(windows)]
        {
            unpack_chromium_extension_archive(&zip_path, &dest_dir, version).expect(
                "fallback publish should prune stale entries and pass post-publish verification",
            );
        }
        #[cfg(not(windows))]
        {
            let staging = sandbox.path().join("staging-direct");
            std::fs::create_dir_all(staging.join("chunks")).unwrap();
            std::fs::write(staging.join("manifest.json"), &manifest).unwrap();
            std::fs::write(staging.join("popup.html"), b"<!doctype html>new").unwrap();
            std::fs::write(staging.join("chunks/bg.js"), b"console.log('v2');").unwrap();
            sync_directory_contents(&staging, &dest_dir).unwrap();
            verify_extracted_tree(&dest_dir).unwrap();
        }

        assert!(
            !dest_dir.join("obsolete.js").exists(),
            "stale root file must be pruned"
        );
        assert!(
            !dest_dir.join("chunks/old-chunk.js").exists(),
            "stale nested file must be pruned"
        );
        assert!(
            !dest_dir.join("legacy_dir").exists(),
            "stale directory must be pruned"
        );
        assert_eq!(
            std::fs::read(dest_dir.join("popup.html")).unwrap(),
            b"<!doctype html>new"
        );
        assert_eq!(
            std::fs::read(dest_dir.join("chunks/bg.js")).unwrap(),
            b"console.log('v2');"
        );
        assert!(verify_extracted_tree(&dest_dir).is_ok());

        #[cfg(windows)]
        {
            assert!(dest_dir.join(".archive-stamp").is_file());
            drop(_dir_handle);
        }

        // Symlink / NTFS junction `dest_dir` MUST be rejected before touching target contents.
        let victim = sandbox.path().join("victim-publish");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("important.txt"), b"keep-me").unwrap();
        let link_dest = sandbox.path().join("dest-link");
        if create_test_dir_link(&link_dest, &victim) {
            let err =
                unpack_chromium_extension_archive(&zip_path, &link_dest, version).unwrap_err();
            assert!(err.contains("不是安全的普通目录"), "err={err}");
            assert!(
                victim.join("important.txt").is_file(),
                "target behind junction/symlink must never be modified or deleted"
            );
            assert!(!victim.join("manifest.json").exists());
            remove_test_dir_link(&link_dest);
        }
    }

    #[test]
    fn update_available_falls_back_to_default_github_releases_when_release_url_is_missing() {
        crate::offscreen::OffscreenPlatform::init_test();
        let recorded = RecordedOps::new();
        let next = Cell::new(0);
        let tools = SystemTools::new_with_hooks(
            WorkbenchWindow::new().unwrap(),
            Rc::new(|_| true),
            Rc::new(move || {
                next.set(next.get() + 1);
                next.get()
            }),
            recorded.hooks(None, None, None),
        )
        .unwrap();

        // 1. Unsigned build + empty `release_url`: button must still be shown and open default URL.
        tools.signing_configured.set(false);
        tools.event(&EventEnvelope {
            sequence: 50,
            event: CoreEvent::UpdateAvailable {
                current: "10.1.0".into(),
                latest: "10.2.0".into(),
                notes: "Unsigned release notes".into(),
                release_url: String::new(),
                installer_name: "HLSDownloader-10.2.0-Windows-x64.msi".into(),
                installer_size: 120_000_000,
                sha256_verified: true,
            },
        });
        assert!(!tools.can_download.get());
        assert!(tools.can_open_release.get());
        let window = tools.window.borrow().as_ref().unwrap().clone_strong();
        assert!(window.get_can_open_release());
        assert!(tools.update.borrow().contains(DEFAULT_GITHUB_RELEASES_URL));
        assert!(tools.update.borrow().contains("打开发布页"));
        tools.action("open_release");
        assert_eq!(
            recorded.opened_targets.borrow().as_slice(),
            [DEFAULT_GITHUB_RELEASES_URL]
        );

        // 2. Signed build + missing/unverified MSI asset + invalid non-GitHub `release_url`:
        //    also falls back to `DEFAULT_GITHUB_RELEASES_URL` so button matches guidance text.
        recorded.opened_targets.borrow_mut().clear();
        tools.signing_configured.set(true);
        tools.event(&EventEnvelope {
            sequence: 51,
            event: CoreEvent::UpdateAvailable {
                current: "10.1.0".into(),
                latest: "10.2.1".into(),
                notes: "Portable only".into(),
                release_url: "https://evil.example/not-github".into(),
                installer_name: "HLSDownloader-10.2.1-Portable.zip".into(),
                installer_size: 90_000_000,
                sha256_verified: false,
            },
        });
        assert!(!tools.can_download.get());
        assert!(tools.can_open_release.get());
        assert!(window.get_can_open_release());
        assert!(tools.update.borrow().contains(DEFAULT_GITHUB_RELEASES_URL));
        tools.action("open_release");
        assert_eq!(
            recorded.opened_targets.borrow().as_slice(),
            [DEFAULT_GITHUB_RELEASES_URL]
        );
        tools.close();
    }
}
