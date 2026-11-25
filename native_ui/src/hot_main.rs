#![cfg_attr(
    all(windows, not(debug_assertions), not(test)),
    windows_subsystem = "windows"
)]
#![cfg_attr(not(test), warn(clippy::undocumented_unsafe_blocks))]

slint::include_modules!();
mod activation;
mod cast_tools;
mod core_feedback;
mod file_dialogs;
mod media_controller;
mod native_window;
mod offscreen;
mod player_subtitles;
mod queue_editor;
mod resource_tools;
mod service_settings;
mod settings_editor;
mod site_rules;
mod speed_stats;
mod system_tools;
mod table_layout;
mod target_check;
mod task_list;
mod theme;
mod wildcard;
#[cfg(test)]
mod wiring_tests;
mod workbench;

use hls_client::{
    claim_v7_presenter_instance, completion_sound, download_category, install_root,
    is_already_running_error, os_reduce_motion, spawn_core, CoreCommand, CoreEvent, CoreIpcClient,
    CorePipeRequest, CorePipeResponse, EventEnvelope, ResourceKind, ResourceOffer, TaskSnapshot,
};
use serde::Deserialize;
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::io::Write;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

// User-visible titles; native window operations use the actual component HWND.
mod window_titles {
    pub const CONFIRM: &str = "确认下载";
    pub const COMPLETE: &str = "下载完成";
    pub const PROGRESS_HEADLINE: &str = "下载进度";
    pub const LOCAL_PROCESSING: &str = "本地处理中";
}

use window_titles::{COMPLETE, CONFIRM, LOCAL_PROCESSING, PROGRESS_HEADLINE};

const PROGRESS_HUD_WIDTH: f32 = 380.0;
const PROGRESS_HUD_COMPACT_HEIGHT: f32 = 256.0;
const PROGRESS_HUD_EXPANDED_HEIGHT: f32 = 468.0;
const PROGRESS_SPEED_HISTORY_LIMIT: usize = 40;
const PROGRESS_CONNECTION_ROWS_LIMIT: usize = 8;

// GUI 的 stderr 通常不可见，先落盘崩溃现场。
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
    #[serde(default)]
    task_id: Option<String>,
    offer: ResourceOffer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct VariantSelection {
    preferred_variant_id: String,
    preferred_height: u32,
    preferred_bandwidth: u64,
}

impl VariantSelection {
    fn from_offer(offer: &ResourceOffer) -> Self {
        Self {
            preferred_variant_id: offer.preferred_variant_id.trim().to_string(),
            preferred_height: offer.preferred_height,
            preferred_bandwidth: offer.preferred_bandwidth,
        }
    }

    fn is_explicit(&self) -> bool {
        !self.preferred_variant_id.is_empty()
            || self.preferred_height > 0
            || self.preferred_bandwidth > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DuplicateMatchKind {
    SameUrl,
    SameTargetPath,
}

#[derive(Clone, Copy, Debug)]
struct DuplicateMatch<'a> {
    task: &'a TaskSnapshot,
    kind: DuplicateMatchKind,
}

fn is_cancelled_task(status: &str) -> bool {
    matches!(status, "canceled" | "cancelled")
}

fn is_adaptive_kind(kind: ResourceKind) -> bool {
    matches!(kind, ResourceKind::Hls | ResourceKind::Dash)
}

fn normalized_target_path_key(dir: &str, filename: &str) -> Option<String> {
    let dir = dir.trim().replace('/', "\\");
    let dir = dir.trim_end_matches('\\');
    let filename = filename.trim();
    if dir.is_empty() || filename.is_empty() {
        return None;
    }
    Some(format!(
        "{}\\{}",
        dir.to_lowercase(),
        filename.to_lowercase()
    ))
}

fn task_target_path_key(task: &TaskSnapshot) -> Option<String> {
    if let Some(key) = normalized_target_path_key(&task.download_dir, &task.filename) {
        return Some(key);
    }
    let output = task.output_path.trim();
    if output.is_empty() {
        return None;
    }
    let split = output.rfind(['\\', '/'])?;
    let (dir, name) = output.split_at(split);
    normalized_target_path_key(dir, &name[1..])
}

fn explicit_filename_from_url(url: &str) -> Option<&str> {
    let without_tail = url.split(['?', '#']).next().unwrap_or(url).trim();
    let path = match without_tail.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map(|(_, path)| path).unwrap_or(""),
        None => without_tail,
    };
    path.rsplit('/').find(|part| !part.is_empty())
}

fn is_rotating_signing_param(key: &str) -> bool {
    let lower = key.trim().to_ascii_lowercase();
    if lower.starts_with("x-amz-")
        || lower.starts_with("x-goog-")
        || lower.starts_with("x-oss-")
        || lower.starts_with("q-sign-")
    {
        return true;
    }
    matches!(
        lower.as_str(),
        "sig"
            | "signature"
            | "token"
            | "jwt"
            | "expires"
            | "exp"
            | "se"
            | "sp"
            | "sv"
            | "st"
            | "skt"
            | "ske"
            | "hdnts"
            | "auth_key"
            | "sign"
            | "nonce"
            | "timestamp"
            | "awsaccesskeyid"
            | "googleaccessid"
            | "ossaccesskeyid"
            | "key-pair-id"
            | "policy"
            | "q-key-time"
            | "q-signature"
            | "q-header-list"
            | "q-url-param-list"
            | "q-ak"
    )
}

fn url_without_signing_params(value: &str) -> String {
    let without_fragment = value.split('#').next().unwrap_or(value).trim();
    let (base, query) = match without_fragment.split_once('?') {
        Some((base, query)) => (base.trim_end_matches('/'), Some(query)),
        None => (without_fragment.trim_end_matches('/'), None),
    };
    let Some(query) = query else {
        return base.to_owned();
    };
    let mut kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let key = pair.split('=').next().unwrap_or("");
            !is_rotating_signing_param(key)
        })
        .collect();
    if kept.is_empty() {
        return base.to_owned();
    }
    kept.sort_unstable();
    format!("{base}?{}", kept.join("&"))
}

fn find_duplicate_match<'a>(
    tasks: &'a [TaskSnapshot],
    task_variants: &HashMap<String, VariantSelection>,
    offer: &ResourceOffer,
    target_dir: &str,
    target_filename: &str,
) -> Option<DuplicateMatch<'a>> {
    let want_url = canonical_url(&offer.url);
    let offer_variant = VariantSelection::from_offer(offer);
    let is_adaptive = is_adaptive_kind(offer.resource_kind);
    if !want_url.is_empty() {
        if let Some(task) = tasks.iter().find(|task| {
            if canonical_url(&task.url) != want_url {
                return false;
            }
            if let Some(existing) = task_variants.get(&task.task_id) {
                return *existing == offer_variant;
            }
            if is_adaptive && offer_variant.is_explicit() {
                return false;
            }
            true
        }) {
            return Some(DuplicateMatch {
                task,
                kind: DuplicateMatchKind::SameUrl,
            });
        }
    }
    if is_adaptive {
        return None;
    }
    let offer_name = if !target_filename.trim().is_empty() {
        target_filename.trim()
    } else if !offer.filename.trim().is_empty() {
        offer.filename.trim()
    } else {
        explicit_filename_from_url(&offer.url).unwrap_or("")
    };
    let offer_name_lower = offer_name.to_lowercase();
    let eligible = |task: &&TaskSnapshot| {
        !is_adaptive_kind(task.resource_kind) && !is_cancelled_task(&task.status)
    };

    // 4. Same URL after stripping rotating presigned CDN query params + same filename.
    let offer_stripped_url = url_without_signing_params(&offer.url);
    if !offer_stripped_url.is_empty() && !offer_name_lower.is_empty() {
        if let Some(task) = tasks.iter().filter(eligible).find(|task| {
            task.filename.trim().to_lowercase() == offer_name_lower
                && url_without_signing_params(&task.url) == offer_stripped_url
        }) {
            return Some(DuplicateMatch {
                task,
                kind: DuplicateMatchKind::SameUrl,
            });
        }
    }

    // 3. Same non-empty canonical source_page_url + same filename (e.g. GitHub Release clicks).
    let offer_page = canonical_url(&offer.source_page_url).trim();
    if !offer_page.is_empty() && !offer_name_lower.is_empty() {
        if let Some(task) = tasks.iter().filter(eligible).find(|task| {
            task.filename.trim().to_lowercase() == offer_name_lower
                && canonical_url(&task.source_page_url).trim() == offer_page
        }) {
            return Some(DuplicateMatch {
                task,
                kind: DuplicateMatchKind::SameUrl,
            });
        }
    }

    // 2. Same resolved target file path (download_dir + filename, case-insensitive).
    if let Some(want_path) = normalized_target_path_key(target_dir, offer_name) {
        if let Some(task) = tasks
            .iter()
            .filter(eligible)
            .find(|task| task_target_path_key(task).as_deref() == Some(want_path.as_str()))
        {
            return Some(DuplicateMatch {
                task,
                kind: DuplicateMatchKind::SameTargetPath,
            });
        }
    }

    None
}

#[cfg(test)]
fn find_duplicate_task<'a>(
    tasks: &'a [TaskSnapshot],
    task_variants: &HashMap<String, VariantSelection>,
    offer: &ResourceOffer,
) -> Option<&'a TaskSnapshot> {
    find_duplicate_match(tasks, task_variants, offer, "", "").map(|found| found.task)
}

fn format_duplicate_notice(found: DuplicateMatch<'_>) -> (String, String) {
    let prefix = match found.kind {
        DuplicateMatchKind::SameUrl => "已有同一地址的任务",
        DuplicateMatchKind::SameTargetPath => "已有同一文件的任务",
    };
    (
        format!(
            "{prefix}：{}（{}）",
            found.task.filename,
            task_status_label(&found.task.status)
        ),
        found.task.task_id.clone(),
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PresenterSettings {
    download_dir: String,
    category_media: String,
    category_program: String,
    category_archive: String,
    category_other: String,
    queue_profiles: Vec<hls_client::QueueProfile>,
    recent_dirs: Vec<String>,
    legal_accepted: bool,
    speed_limit_kib: u64,
    speed_limit_enabled: bool,
    quick_download_enabled: bool,
    completion_power_action: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QuickDownloadPlan {
    handoff_id: String,
    filename: String,
    download_dir: String,
    queue_id: String,
}

fn evaluate_quick_download(
    offer: &ResourceOffer,
    settings: &PresenterSettings,
    known_tasks: &[TaskSnapshot],
    known_task_variants: &HashMap<String, VariantSelection>,
) -> Option<QuickDownloadPlan> {
    if !settings.quick_download_enabled || !settings.legal_accepted {
        return None;
    }
    if offer.handoff_id.trim().is_empty() {
        return None;
    }
    let raw_filename = if offer.filename.trim().is_empty() {
        filename_from_url(&offer.url)
    } else {
        offer.filename.clone()
    };
    let filename = confirmed_filename(&raw_filename).ok()?.to_owned();
    let category = download_category(&filename, "", offer.resource_kind);
    let download_dir = settings.directory_for(category);
    if find_duplicate_match(
        known_tasks,
        known_task_variants,
        offer,
        &download_dir,
        &filename,
    )
    .is_some()
    {
        return None;
    }
    target_check::check_target(&download_dir, &filename, false).ok()?;
    Some(QuickDownloadPlan {
        handoff_id: offer.handoff_id.clone(),
        filename,
        download_dir,
        queue_id: hls_client::DEFAULT_QUEUE_ID.into(),
    })
}

struct ClipboardOfferFilter {
    recent: VecDeque<(Vec<String>, std::time::Instant)>,
    cooldown: Duration,
    max_entries: usize,
}

impl Default for ClipboardOfferFilter {
    fn default() -> Self {
        Self {
            recent: VecDeque::new(),
            cooldown: Duration::from_secs(30),
            max_entries: 16,
        }
    }
}

impl ClipboardOfferFilter {
    fn accept(&mut self, urls: &[String], now: std::time::Instant) -> Option<Vec<String>> {
        let mut normalized = Vec::new();
        for raw in urls {
            let trimmed = raw.trim();
            if !trimmed.is_empty() && !normalized.iter().any(|existing| existing == trimmed) {
                normalized.push(trimmed.to_owned());
            }
        }
        if normalized.is_empty() {
            return None;
        }
        while let Some((_, at)) = self.recent.front() {
            if now.saturating_duration_since(*at) >= self.cooldown {
                self.recent.pop_front();
            } else {
                break;
            }
        }
        if self.recent.iter().any(|(batch, _)| batch == &normalized) {
            return None;
        }
        self.recent.push_back((normalized.clone(), now));
        while self.recent.len() > self.max_entries {
            self.recent.pop_front();
        }
        Some(normalized)
    }
}

fn normalized_dir_key(path: &str) -> String {
    let trimmed = path.trim();
    let stripped = trimmed.trim_end_matches(['\\', '/']);
    if stripped.is_empty() {
        trimmed.to_ascii_lowercase()
    } else {
        stripped.to_ascii_lowercase()
    }
}

struct SettingsReload<C> {
    client: Option<C>,
    last_request: std::time::Instant,
}

impl<C> SettingsReload<C> {
    fn reload(
        &mut self,
        now: std::time::Instant,
        mut connect: impl FnMut() -> Result<C, String>,
        mut load: impl FnMut(&mut C) -> Result<CorePipeResponse, String>,
    ) -> Option<CorePipeResponse> {
        if now.saturating_duration_since(self.last_request) >= Duration::from_secs(60) {
            self.client = None;
        }
        self.last_request = now;
        // Settings reads are safe to retry once if an idle/broken pipe fails.
        for _ in 0..2 {
            if self.client.is_none() {
                self.client = connect().ok();
            }
            let Some(client) = self.client.as_mut() else {
                continue;
            };
            match load(client) {
                Ok(response) => {
                    return matches!(response, CorePipeResponse::Settings { .. })
                        .then_some(response);
                }
                Err(_) => self.client = None,
            }
        }
        None
    }
}

fn update_presenter_settings(
    response: &CorePipeResponse,
    presenter: &mut PresenterSettings,
    runtime: &mut PresenterRuntimeSettings,
) -> Option<PresenterRuntimeSettings> {
    if !matches!(response, CorePipeResponse::Settings { .. }) {
        return None;
    }
    let previous_recent = std::mem::take(&mut presenter.recent_dirs);
    *presenter = PresenterSettings::from_response(Some(response));
    presenter.recent_dirs = previous_recent;
    *runtime = PresenterRuntimeSettings::from_response(Some(response));
    Some(*runtime)
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
            queue_profiles,
            legal_accepted,
            speed_limit_kib,
            speed_limit_enabled,
            quick_download_enabled,
            completion_power_action,
            ..
        }) = response
        else {
            return Self {
                queue_profiles: queue_editor::ordered_profiles(&[]),
                ..Self::default()
            };
        };
        Self {
            download_dir: download_dir.clone(),
            category_media: category_dir_media.clone(),
            category_program: category_dir_program.clone(),
            category_archive: category_dir_archive.clone(),
            category_other: category_dir_other.clone(),
            queue_profiles: queue_editor::ordered_profiles(queue_profiles),
            recent_dirs: Vec::new(),
            legal_accepted: *legal_accepted,
            speed_limit_kib: *speed_limit_kib,
            speed_limit_enabled: *speed_limit_enabled,
            quick_download_enabled: *quick_download_enabled,
            completion_power_action: completion_power_action.clone(),
        }
    }

    fn ordered_queues(&self) -> Vec<hls_client::QueueProfile> {
        queue_editor::ordered_profiles(&self.queue_profiles)
    }

    fn queue_names(&self) -> Vec<slint::SharedString> {
        queue_editor::queue_option_names(&self.ordered_queues())
    }

    fn queue_index_for_id(&self, queue_id: &str) -> i32 {
        queue_editor::queue_index_for_id(&self.ordered_queues(), queue_id)
    }

    fn queue_id_at(&self, index: i32) -> String {
        queue_editor::queue_id_at(&self.ordered_queues(), index)
    }

    fn record_recent_dir(&mut self, directory: &str) {
        let trimmed = directory.trim();
        if trimmed.is_empty() {
            return;
        }
        let key = normalized_dir_key(trimmed);
        self.recent_dirs
            .retain(|entry| normalized_dir_key(entry) != key);
        self.recent_dirs.insert(0, trimmed.to_owned());
        self.recent_dirs.truncate(8);
    }

    fn recent_dirs_for(&self, current: &str) -> (Vec<String>, i32) {
        let mut dirs = Vec::new();
        let mut keys = HashSet::new();
        let mut push = |candidate: &str| {
            let trimmed = candidate.trim();
            if trimmed.is_empty() || dirs.len() >= 12 {
                return;
            }
            let key = normalized_dir_key(trimmed);
            if keys.insert(key) {
                dirs.push(trimmed.to_owned());
            }
        };
        for recent in &self.recent_dirs {
            push(recent);
        }
        push(&self.download_dir);
        push(&self.category_media);
        push(&self.category_program);
        push(&self.category_archive);
        push(&self.category_other);
        push(current);
        let current_key = normalized_dir_key(current);
        let index = if current_key.is_empty() {
            -1
        } else {
            dirs.iter()
                .position(|entry| normalized_dir_key(entry) == current_key)
                .map_or(-1, |idx| idx as i32)
        };
        (dirs, index)
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

fn main() -> std::process::ExitCode {
    install_panic_log();
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let message = error.to_string();
            let _ = writeln!(std::io::stderr(), "{message}");
            if !env::args_os().any(|arg| {
                arg == "--self-test" || arg == "--lock-test" || arg == "--visual-fixture"
            }) {
                // 图形后端初始化失败等早于工作台创建的错误，也要告诉等待中的插件
                // “工作台启动失败”，否则插件只能等到超时。必须同步发送：进程随即退出。
                if let Some(launch_id) = hls_client::workbench_launch_id() {
                    hls_client::report_workbench_launch(launch_id, "main_failed");
                }
                report_startup_error(&message);
            }
            std::process::ExitCode::FAILURE
        }
    }
}

fn report_startup_error(error: &str) {
    // Launcher disconnects stderr. Keep one bounded local startup record and
    // show the failure even when the Slint backend cannot create any window.
    let error: String = error.chars().take(4096).collect();
    let log = hls_client::default_v7_database_path()
        .with_file_name("logs")
        .join("presenter-startup.json");
    let record = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "timestamp_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(),
        "error": error,
    });
    let logged = log.parent().is_some_and(|directory| {
        std::fs::create_dir_all(directory)
            .and_then(|_| std::fs::write(&log, record.to_string()))
            .is_ok()
    });
    #[cfg(windows)]
    // SAFETY: `title` and `message` are valid NUL-terminated UTF-16 buffers outliving the
    // synchronous `MessageBoxW` call, and a null owner HWND creates a standalone error dialog.
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let title: Vec<u16> = "HLS Downloader\0".encode_utf16().collect();
        let mut message = format!("无法打开 HLS Downloader。\n\n{error}");
        if logged {
            message.push_str(&format!("\n\n启动日志：{}", log.display()));
        }
        let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
    #[cfg(not(windows))]
    let _ = logged;
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    native_window::initialize_app_id();
    let args: Vec<String> = env::args().collect();
    if let Some(kind) = args
        .windows(2)
        .find(|pair| pair[0] == "--visual-fixture")
        .map(|pair| pair[1].as_str())
    {
        if let Some(platform) = offscreen::OffscreenPlatform::from_env() {
            slint::platform::set_platform(Box::new(platform))
                .map_err(|error| format!("offscreen renderer: {error}"))?;
        }
        return run_visual_fixture(kind, args.iter().any(|arg| arg == "--dark"));
    }
    // 确认弹窗必须在沙盒、远程桌面和无 OpenGL 驱动的电脑上正常显示。
    #[cfg(windows)]
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
    if args.iter().any(|arg| arg == "--self-test") {
        let confirm = ConfirmWindow::new()?;
        confirm.on_move_window({
            let weak = confirm.as_weak();
            move |dx, dy| {
                if let Some(item) = weak.upgrade() {
                    move_window(item.window(), dx, dy);
                }
            }
        });
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
            if args.iter().any(|arg| arg == "--workbench") {
                activation::forward_workbench(hls_client::workbench_launch_id())?;
            }
            return Ok(());
        }
        return Err(error.into());
    }
    if lock_test {
        thread::sleep(Duration::from_secs(1));
        return Ok(());
    }
    let activation = activation::ActivationServer::start()?;

    let mut initial_client = connect_or_start_core()?;
    let (initial_tasks, initial_sequence, initial_browser_connected) =
        initial_client.snapshot_state_with_browser()?;
    let (initial_pending, initial_task_variants) = load_handoff_state(&mut initial_client);
    let settings = initial_client.load_settings().ok();
    let presenter_settings = Arc::new(Mutex::new(PresenterSettings::from_response(
        settings.as_ref(),
    )));
    let known_tasks = Arc::new(Mutex::new(initial_tasks));
    let known_task_variants = Arc::new(Mutex::new(initial_task_variants));
    let runtime_settings = Rc::new(RefCell::new(PresenterRuntimeSettings::from_response(
        settings.as_ref(),
    )));
    let initial_runtime = *runtime_settings.borrow();

    let confirm = ConfirmWindow::new()?;
    confirm.on_move_window({
        let weak = confirm.as_weak();
        move |dx, dy| {
            if let Some(item) = weak.upgrade() {
                move_window(item.window(), dx, dy);
            }
        }
    });
    let progress_user_moved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let progress = ProgressWindow::new()?;
    progress.on_move_window({
        let weak = progress.as_weak();
        let moved = Arc::clone(&progress_user_moved);
        move |dx, dy| {
            moved.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(item) = weak.upgrade() {
                move_window(item.window(), dx, dy);
            }
        }
    });
    let complete = CompleteWindow::new()?;
    complete.on_move_window({
        let weak = complete.as_weak();
        move |dx, dy| {
            if let Some(item) = weak.upgrade() {
                move_window(item.window(), dx, dy);
            }
        }
    });
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
    let open_folder_on_complete = Rc::new(std::cell::Cell::new(false));
    if let Ok(settings) = presenter_settings.lock() {
        sync_progress_settings(&progress, &settings, open_folder_on_complete.get());
    }
    let theme_timer = Timer::default();
    theme_timer.start(TimerMode::Repeated, Duration::from_secs(1), {
        let settings = Rc::clone(&runtime_settings);
        let confirm = confirm.as_weak();
        let progress = progress.as_weak();
        let complete = complete.as_weak();
        move || {
            let mut settings = settings.borrow_mut();
            let dark = settings.theme_mode.is_dark();
            if settings.dark != dark {
                settings.dark = dark;
                if let (Some(confirm), Some(progress), Some(complete)) =
                    (confirm.upgrade(), progress.upgrade(), complete.upgrade())
                {
                    apply_runtime_settings(&confirm, &progress, &complete, *settings);
                }
            }
        }
    });

    let (settings_reload, settings_requests) = mpsc::channel::<()>();
    let (settings_tx, settings_responses) = mpsc::channel();
    thread::spawn({
        let confirm = confirm.as_weak();
        move || {
            let mut reload = SettingsReload {
                client: Some(initial_client),
                last_request: std::time::Instant::now(),
            };
            while settings_requests.recv().is_ok() {
                while settings_requests.try_recv().is_ok() {}
                if let Some(response) = reload.reload(
                    std::time::Instant::now(),
                    CoreIpcClient::connect,
                    CoreIpcClient::load_settings,
                ) {
                    if settings_tx.send(response).is_err()
                        || confirm
                            .upgrade_in_event_loop(|ui| ui.invoke_core_events_ready())
                            .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    let workbench = Rc::new(RefCell::new(None::<Rc<workbench::Workbench>>));
    let ui_sequence = Rc::new(std::cell::Cell::new(initial_sequence));
    let native_workbench = Rc::new(std::cell::Cell::new(true));
    let core_online = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let browser_connected = Arc::new(std::sync::atomic::AtomicBool::new(
        initial_browser_connected,
    ));
    confirm.on_open_workbench({
        let workbench = Rc::clone(&workbench);
        let known_tasks = Arc::clone(&known_tasks);
        let runtime_settings = Rc::clone(&runtime_settings);
        let ui_sequence = Rc::clone(&ui_sequence);
        let native_workbench = Rc::clone(&native_workbench);
        let core_online = Arc::clone(&core_online);
        let browser_connected = Arc::clone(&browser_connected);
        move |explicit_native, launch_id: slint::SharedString| {
            let launch_id = (!launch_id.is_empty()).then(|| launch_id.to_string());
            if explicit_native {
                native_workbench.set(true);
            }
            if workbench.borrow().is_none() {
                let tasks = known_tasks
                    .lock()
                    .map(|tasks| tasks.clone())
                    .unwrap_or_default();
                let settings = *runtime_settings.borrow();
                match workbench::Workbench::new(
                    tasks,
                    ui_sequence.get(),
                    browser_connected.load(std::sync::atomic::Ordering::Relaxed),
                    settings.dark,
                    settings.reduce_motion,
                ) {
                    Ok(window) => {
                        window.set_engine_online(
                            core_online.load(std::sync::atomic::Ordering::Relaxed),
                        );
                        let owner = Rc::downgrade(&workbench);
                        window.window.on_request_close(move || {
                            let owner = owner.clone();
                            // Drop the controller after its close/IPC callback
                            // returns, so no live RefCell borrow is invalidated.
                            Timer::single_shot(Duration::from_millis(1), move || {
                                if let Some(owner) = owner.upgrade() {
                                    let release = owner
                                        .borrow()
                                        .as_ref()
                                        .is_some_and(|window| window.ready_to_release());
                                    if release {
                                        owner.borrow_mut().take();
                                        trace("native workbench released");
                                    }
                                }
                            });
                        });
                        *workbench.borrow_mut() = Some(window);
                        trace("native workbench created");
                    }
                    Err(error) => {
                        trace(&format!("workbench creation failed: {error}"));
                        workbench::report_ui_ready(launch_id, "main_failed");
                        report_startup_error(&error.to_string());
                        return;
                    }
                }
            }
            let show_error = workbench
                .borrow()
                .as_ref()
                .and_then(|window| window.show_for_launch(launch_id.clone()).err());
            if let Some(error) = show_error {
                workbench::report_ui_ready(launch_id, "main_failed");
                report_startup_error(&error.to_string());
            }
        }
    });
    let active_hud = Arc::new(Mutex::new(ActiveTaskHud::default()));
    let rendered_hud_task = Arc::new(Mutex::new(None::<String>));
    let complete_shown_at = Rc::new(std::cell::Cell::new(None::<std::time::Instant>));
    let processing_timer = Timer::default();
    processing_timer.start(TimerMode::Repeated, Duration::from_secs(1), {
        let active_hud = Arc::clone(&active_hud);
        let complete_shown_at = Rc::clone(&complete_shown_at);
        let progress = progress.as_weak();
        let complete = complete.as_weak();
        move || {
            if let Some(complete_item) = complete.upgrade() {
                if complete_item.window().is_visible() {
                    let now = std::time::Instant::now();
                    if should_hold_complete_toast(
                        complete_item.get_action_busy(),
                        complete_item.get_power_seconds(),
                        complete_item.get_power_hint().as_str(),
                        native_window::is_cursor_inside(complete_item.window()),
                    ) {
                        complete_shown_at.set(Some(now));
                    } else if let Some(shown_at) = complete_shown_at.get() {
                        if now.saturating_duration_since(shown_at) >= COMPLETE_AUTO_DISMISS_DURATION
                        {
                            complete_shown_at.set(None);
                            let _ = complete_item.hide();
                        }
                    } else {
                        complete_shown_at.set(Some(now));
                    }
                } else if complete_shown_at.get().is_some() {
                    complete_shown_at.set(None);
                }
            }
            let Some(item) = progress.upgrade() else {
                return;
            };
            if item.get_hidden_by_user() || !item.window().is_visible() {
                return;
            }
            let update = active_hud.lock().ok().and_then(|hud| {
                let snapshot = hud.current_snapshot()?;
                if !is_processing_status(&snapshot.status) {
                    return None;
                }
                let elapsed =
                    hud.processing_elapsed_secs(&snapshot.task_id, std::time::Instant::now());
                Some(format_processing_headline(&snapshot.status, elapsed))
            });
            if let Some(headline) = update {
                if item.get_speed().as_str() != headline {
                    item.set_speed(headline.into());
                    item.window().request_redraw();
                }
            }
        }
    });
    confirm.on_core_connection_changed({
        let workbench = Rc::clone(&workbench);
        let core_online = Arc::clone(&core_online);
        move |online| {
            core_online.store(online, std::sync::atomic::Ordering::Relaxed);
            if let Some(window) = workbench.borrow().as_ref() {
                window.set_engine_online(online);
            }
        }
    });
    confirm.on_core_resynced({
        let settings_reload = settings_reload.clone();
        let workbench = Rc::clone(&workbench);
        let known_tasks = Arc::clone(&known_tasks);
        let presenter_settings = Arc::clone(&presenter_settings);
        let ui_sequence = Rc::clone(&ui_sequence);
        let active_hud = Arc::clone(&active_hud);
        let rendered_hud_task = Arc::clone(&rendered_hud_task);
        let runtime_settings = Rc::clone(&runtime_settings);
        let core_online = Arc::clone(&core_online);
        let browser_connected = Arc::clone(&browser_connected);
        let progress_user_moved = Arc::clone(&progress_user_moved);
        let progress = progress.as_weak();
        move |sequence| {
            core_online.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = settings_reload.send(());
            let sequence = sequence.parse().unwrap_or_default();
            ui_sequence.set(sequence);
            let tasks = known_tasks
                .lock()
                .map(|tasks| tasks.clone())
                .unwrap_or_default();
            if let Some(window) = workbench.borrow().as_ref() {
                window.set_engine_online(true);
                window.set_browser_connected(
                    browser_connected.load(std::sync::atomic::Ordering::Relaxed),
                );
                window.replace(tasks.clone(), sequence);
            }
            let (primary, position, processing_elapsed) = active_hud
                .lock()
                .map(|mut hud| {
                    let primary = hud.reconcile(&tasks);
                    let pos = hud.task_position();
                    let elapsed = primary.as_ref().and_then(|task| {
                        hud.processing_elapsed_secs(&task.task_id, std::time::Instant::now())
                    });
                    (primary, pos, elapsed)
                })
                .unwrap_or((None, (0, 0), None));
            let fallback_dir = presenter_settings
                .lock()
                .map(|settings| settings.download_dir.clone())
                .unwrap_or_default();
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
                        position,
                        &fallback_dir,
                        processing_elapsed,
                        Some(&progress_user_moved),
                    );
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
    });
    thread::spawn({
        let confirm = confirm.as_weak();
        move || {
            while let Ok(launch_id) = activation.requests.recv() {
                if confirm
                    .upgrade_in_event_loop(move |item| {
                        item.invoke_open_workbench(true, launch_id.unwrap_or_default().into())
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    });
    let pending = Arc::new(Mutex::new(initial_pending));
    let active_handoff = Arc::new(Mutex::new(None::<String>));
    let quick_failed = Arc::new(Mutex::new(HashSet::<String>::new()));
    let offer_state = EventLoopState {
        pending: Arc::clone(&pending),
        active_handoff: Arc::clone(&active_handoff),
        presenter_settings: Arc::clone(&presenter_settings),
        known_tasks: Arc::clone(&known_tasks),
        known_task_variants: Arc::clone(&known_task_variants),
        quick_failed: Arc::clone(&quick_failed),
        confirm_dir_overridden: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        core_online: Arc::clone(&core_online),
        browser_connected: Arc::clone(&browser_connected),
    };
    let completed_task = Rc::new(RefCell::new(None::<String>));
    let completed_notified = Rc::new(RefCell::new(HashSet::<String>::new()));

    progress.on_command({
        let active_hud = Arc::clone(&active_hud);
        let rendered_hud_task = Arc::clone(&rendered_hud_task);
        let presenter_settings = Arc::clone(&presenter_settings);
        let runtime_settings = Rc::clone(&runtime_settings);
        let open_folder_on_complete = Rc::clone(&open_folder_on_complete);
        let settings_reload = settings_reload.clone();
        let workbench = Rc::clone(&workbench);
        let progress_user_moved = Arc::clone(&progress_user_moved);
        let confirm = confirm.as_weak();
        let window = progress.as_weak();
        move |command| {
            let action = command.to_string();
            trace(&format!("progress command: {action}"));
            match action.as_str() {
                "hide_progress" => {
                    let remaining = active_hud
                        .lock()
                        .ok()
                        .and_then(|mut hud| hud.dismiss_paused());
                    if remaining.is_none() {
                        if let Ok(mut rendered) = rendered_hud_task.lock() {
                            *rendered = None;
                        }
                    }
                    if let Some(item) = window.upgrade() {
                        item.set_hidden_by_user(true);
                        let _ = item.hide();
                    }
                }
                "toggle_expand" => {
                    if let Some(item) = window.upgrade() {
                        let next = !item.get_expanded();
                        resize_progress_window(&item, next);
                    }
                }
                "prev_task" | "next_task" => {
                    let delta = if action == "prev_task" { -1 } else { 1 };
                    let (selected, position, processing_elapsed) = active_hud
                        .lock()
                        .map(|mut hud| {
                            let next = hud.cycle(delta);
                            let pos = hud.task_position();
                            let elapsed = next.as_ref().and_then(|task| {
                                hud.processing_elapsed_secs(
                                    &task.task_id,
                                    std::time::Instant::now(),
                                )
                            });
                            (next, pos, elapsed)
                        })
                        .unwrap_or((None, (0, 0), None));
                    if let Some(snapshot) = selected {
                        let fallback_dir = presenter_settings
                            .lock()
                            .map(|settings| settings.download_dir.clone())
                            .unwrap_or_default();
                        reset_progress_action_for_new_task(
                            &window,
                            &rendered_hud_task,
                            &snapshot.task_id,
                        );
                        render_progress_hud(
                            &window,
                            *runtime_settings.borrow(),
                            &snapshot,
                            position,
                            &fallback_dir,
                            processing_elapsed,
                            Some(&progress_user_moved),
                        );
                    }
                }
                "open_workbench" => {
                    let task_id = active_hud.lock().ok().and_then(|hud| hud.primary_task_id());
                    if let Some(confirm_win) = confirm.upgrade() {
                        confirm_win.invoke_open_workbench(true, "".into());
                    }
                    if let Some(task_id) = task_id {
                        let current = workbench.borrow().clone();
                        if let Some(wb) = current.as_ref() {
                            wb.window
                                .invoke_action(format!("focus_task:{task_id}").into());
                        }
                    }
                }
                "open_folder" => {
                    let fallback_dir = presenter_settings
                        .lock()
                        .map(|settings| settings.download_dir.clone())
                        .unwrap_or_default();
                    let target_dir = active_hud
                        .lock()
                        .ok()
                        .and_then(|hud| hud.current_snapshot())
                        .and_then(|snapshot| snapshot_folder(&snapshot, &fallback_dir))
                        .or_else(|| (!fallback_dir.trim().is_empty()).then_some(fallback_dir));
                    if let Some(item) = window.upgrade() {
                        match target_dir {
                            Some(dir) => match file_dialogs::open(&dir) {
                                Ok(()) => {
                                    item.set_action_error(false);
                                    item.set_action_text("已打开保存目录".into());
                                }
                                Err(error) => {
                                    item.set_action_error(true);
                                    item.set_action_text(format!("无法打开目录：{error}").into());
                                }
                            },
                            None => {
                                item.set_action_error(true);
                                item.set_action_text("当前任务未设置保存目录".into());
                            }
                        }
                    }
                }
                "speed_limit_toggle" => {
                    let Some(item) = window.upgrade() else {
                        return;
                    };
                    let current_enabled = item.get_speed_limit_enabled();
                    let current_kib = item.get_speed_limit_kib().max(0) as u64;
                    let (next_enabled, effective_kib, values) =
                        toggle_hud_speed_limit(current_enabled, current_kib);
                    item.set_speed_limit_enabled(next_enabled);
                    item.set_speed_limit_kib(effective_kib.min(i32::MAX as u64) as i32);
                    if let Ok(mut settings) = presenter_settings.lock() {
                        settings.speed_limit_enabled = next_enabled;
                        settings.speed_limit_kib = effective_kib;
                    }
                    let label = format_hud_speed_limit_feedback(next_enabled, effective_kib);
                    let result_window = window.clone();
                    let reload = settings_reload.clone();
                    thread::spawn(move || {
                        let result = store_settings_via_ipc(values);
                        let _ = result_window.upgrade_in_event_loop(move |item| match result {
                            Ok(()) => {
                                item.set_action_error(false);
                                item.set_action_text(label.into());
                            }
                            Err(error) => {
                                item.set_action_error(true);
                                item.set_action_text(format!("限速保存失败：{error}").into());
                                let _ = reload.send(());
                            }
                        });
                    });
                }
                value if value.starts_with("speed_limit:") => {
                    let Some(kib) = value
                        .strip_prefix("speed_limit:")
                        .and_then(|raw| raw.parse::<u64>().ok())
                    else {
                        return;
                    };
                    let enabled = kib > 0;
                    let values = build_speed_limit_settings(enabled, kib);
                    if let Some(item) = window.upgrade() {
                        item.set_speed_limit_enabled(enabled);
                        item.set_speed_limit_kib(kib.min(i32::MAX as u64) as i32);
                    }
                    if let Ok(mut settings) = presenter_settings.lock() {
                        settings.speed_limit_enabled = enabled;
                        settings.speed_limit_kib = kib;
                    }
                    let label = format_hud_speed_limit_feedback(enabled, kib);
                    let result_window = window.clone();
                    let reload = settings_reload.clone();
                    thread::spawn(move || {
                        let result = store_settings_via_ipc(values);
                        let _ = result_window.upgrade_in_event_loop(move |item| match result {
                            Ok(()) => {
                                item.set_action_error(false);
                                item.set_action_text(label.into());
                            }
                            Err(error) => {
                                item.set_action_error(true);
                                item.set_action_text(format!("限速保存失败：{error}").into());
                                let _ = reload.send(());
                            }
                        });
                    });
                }
                value if value.starts_with("completion_action:") => {
                    let Some(index) = value
                        .strip_prefix("completion_action:")
                        .and_then(|raw| raw.parse::<i32>().ok())
                    else {
                        return;
                    };
                    let (wire_action, open_folder) = completion_action_from_index(index);
                    open_folder_on_complete.set(open_folder);
                    if let Some(item) = window.upgrade() {
                        item.set_completion_action_index(index);
                    }
                    if let Ok(mut settings) = presenter_settings.lock() {
                        settings.completion_power_action = wire_action.to_owned();
                    }
                    let feedback = format!(
                        "完成后：{}",
                        COMPLETION_ACTION_LABELS
                            .get(index.max(0) as usize)
                            .copied()
                            .unwrap_or("无操作")
                    );
                    let result_window = window.clone();
                    let reload = settings_reload.clone();
                    thread::spawn(move || {
                        let result = CoreIpcClient::connect().and_then(|mut client| {
                            client.store_setting(
                                "completion_power_action",
                                serde_json::Value::String(wire_action.to_owned()),
                            )
                        });
                        let _ = result_window.upgrade_in_event_loop(move |item| match result {
                            Ok(()) => {
                                item.set_action_error(false);
                                item.set_action_text(feedback.into());
                            }
                            Err(error) => {
                                item.set_action_error(true);
                                item.set_action_text(format!("完成动作保存失败：{error}").into());
                                let _ = reload.send(());
                            }
                        });
                    });
                }
                "pause" | "resume" | "cancel" => {
                    let task_id = active_hud.lock().ok().and_then(|hud| hud.primary_task_id());
                    if let Some(task_id) = task_id {
                        if action == "pause" {
                            if let Ok(mut hud) = active_hud.lock() {
                                hud.mark_pause_requested(&task_id);
                            }
                        }
                        if let Some(item) = window.upgrade() {
                            item.set_action_busy(true);
                            item.set_action_error(false);
                            item.set_action_text(
                                match action.as_str() {
                                    "pause" => "正在暂停…",
                                    "resume" => "正在继续…",
                                    _ => "正在取消…",
                                }
                                .into(),
                            );
                        }
                        let result_window = window.clone();
                        let result_task_id = task_id.clone();
                        let result_hud_task = Arc::clone(&rendered_hud_task);
                        let result_active_hud = Arc::clone(&active_hud);
                        let sent_action = action.clone();
                        thread::spawn(move || {
                            let result = CoreIpcClient::connect().and_then(|mut client| {
                                client
                                    .command(CoreCommand::TaskAction { task_id, action })
                                    .and_then(core_feedback::command_result)
                            });
                            let _ = result_window.upgrade_in_event_loop(move |item| {
                                if result.is_err() && sent_action == "pause" {
                                    if let Ok(mut hud) = result_active_hud.lock() {
                                        hud.clear_pause_pin(&result_task_id);
                                    }
                                }
                                if result_hud_task.lock().ok().and_then(|task| task.clone())
                                    != Some(result_task_id)
                                {
                                    return;
                                }
                                item.set_action_busy(false);
                                item.set_action_error(result.is_err());
                                item.set_action_text(match result {
                                    Ok(()) if sent_action == "pause" || sent_action == "resume" => {
                                        "".into()
                                    }
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
                _ => {}
            }
        }
    });
    confirm.on_notice({
        let workbench = Rc::clone(&workbench);
        move |message| {
            if let Some(window) = workbench.borrow().as_ref() {
                window.show_notice("warning", message);
            }
        }
    });
    confirm.on_command({
        let pending = Arc::clone(&pending);
        let active_handoff = Arc::clone(&active_handoff);
        let presenter_settings = Arc::clone(&presenter_settings);
        let known_tasks = Arc::clone(&known_tasks);
        let offer_state = offer_state.clone();
        let workbench = Rc::clone(&workbench);
        let window = confirm.as_weak();
        move |command| {
            let command = command.to_string();
            trace(&format!("confirm command: {command}"));
            match command.as_str() {
                "accept"
                | "accept_paused"
                | "rename_copy"
                | "accept_long_path"
                | "accept_paused_long_path"
                | "copy_long_path" => {
                    let copy = matches!(command.as_str(), "rename_copy" | "copy_long_path");
                    let start_paused = matches!(
                        command.as_str(),
                        "accept_paused" | "accept_paused_long_path"
                    );
                    let allow_long = matches!(
                        command.as_str(),
                        "accept_long_path" | "accept_paused_long_path" | "copy_long_path"
                    );
                    let Some(offer) = pending.lock().ok().and_then(|items| items.front().cloned())
                    else {
                        return;
                    };
                    let Some(item) = window.upgrade() else {
                        return;
                    };
                    if item.get_busy() {
                        return;
                    }
                    let filename = item.get_filename().to_string();
                    let download_dir = item.get_download_dir().trim().to_string();
                    let category = item.get_category().to_string();
                    let remember = item.get_remember_directory() && !download_dir.is_empty();
                    let queue_id = presenter_settings
                        .lock()
                        .map(|settings| settings.queue_id_at(item.get_queue_index()))
                        .unwrap_or_else(|_| hls_client::DEFAULT_QUEUE_ID.to_string());
                    let taken: Vec<String> = if copy {
                        known_tasks
                            .lock()
                            .map(|tasks| tasks.iter().map(|task| task.filename.clone()).collect())
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    item.set_rejecting(false);
                    item.set_busy_action(
                        if start_paused {
                            "accept_paused"
                        } else {
                            "accept"
                        }
                        .into(),
                    );
                    item.set_busy(true);
                    if copy {
                        item.set_duplicate_text("".into());
                        item.set_duplicate_task_id("".into());
                    }
                    clear_confirm_issue(&item);
                    let pending = Arc::clone(&pending);
                    let active_handoff = Arc::clone(&active_handoff);
                    let presenter_settings = Arc::clone(&presenter_settings);
                    let known_tasks = Arc::clone(&known_tasks);
                    let weak = item.as_weak();
                    thread::spawn(move || {
                        let map_issue = |issue: target_check::TargetIssue| {
                            let (text, mut actions) = issue.presentation_for(copy);
                            if start_paused {
                                for (_, cmd) in &mut actions {
                                    if *cmd == "accept_long_path" {
                                        *cmd = "accept_paused_long_path";
                                    }
                                }
                            }
                            (text, actions)
                        };
                        let retry_command = if copy {
                            "rename_copy"
                        } else if start_paused {
                            "accept_paused"
                        } else {
                            "accept"
                        };
                        let mut renamed_filename: Option<String> = None;
                        let result = (|| {
                            let filename = confirmed_filename(&filename).map_err(map_issue)?;
                            let filename = if copy {
                                let unique =
                                    target_check::unique_name(&download_dir, filename, &taken)
                                        .map_err(|error| {
                                            (error, [("选择其他位置", "browse"), ("", "")])
                                        })?;
                                renamed_filename = Some(unique.clone());
                                unique
                            } else {
                                filename.to_owned()
                            };
                            target_check::check_target(&download_dir, &filename, allow_long)
                                .map_err(map_issue)?;
                            (|| {
                                let mut client = CoreIpcClient::connect()?;
                                let events = client.command(CoreCommand::AcceptHandoff {
                                    handoff_id: offer.handoff_id.clone(),
                                    filename,
                                    download_dir: download_dir.clone(),
                                    allow_duplicate: copy,
                                    trusted_ui: true,
                                    start_paused,
                                    queue_id,
                                })?;
                                for envelope in &events {
                                    update_known_tasks(&known_tasks, &envelope.event);
                                }
                                core_feedback::command_result(events)?;
                                if !download_dir.is_empty() {
                                    if let Ok(mut settings) = presenter_settings.lock() {
                                        settings.record_recent_dir(&download_dir);
                                    }
                                }
                                if remember {
                                    if let Err(error) = remember_category_directory(
                                        &mut client,
                                        &presenter_settings,
                                        &category,
                                        download_dir,
                                    ) {
                                        let _ = weak.upgrade_in_event_loop(move |item| {
                                            item.invoke_open_workbench(true, "".into());
                                            item.invoke_notice(
                                                format!("下载已提交，但保存分类目录失败：{error}")
                                                    .into(),
                                            );
                                        });
                                    }
                                }
                                Ok::<(), String>(())
                            })()
                            .map_err(|error| {
                                (
                                    format!("未能开始下载：{error}"),
                                    [("重试", retry_command), ("选择其他位置", "browse")],
                                )
                            })
                        })();
                        // Core 的终态事件按 ID 移除队列项。处理较晚的失败时不能改到下一个确认窗。
                        if let Err((text, actions)) = result {
                            let _ = weak.upgrade_in_event_loop(move |item| {
                                let current = active_handoff.lock().ok().is_some_and(|id| {
                                    id.as_deref() == Some(offer.handoff_id.as_str())
                                });
                                if current
                                    && pending.lock().ok().is_some_and(|items| {
                                        items.front().is_some_and(|entry| {
                                            entry.handoff_id == offer.handoff_id
                                        })
                                    })
                                {
                                    if let Some(renamed) = renamed_filename {
                                        item.set_filename(renamed.into());
                                        sync_confirm_save_preview(&item);
                                        item.set_duplicate_text("".into());
                                        item.set_duplicate_task_id("".into());
                                    }
                                    item.set_busy(false);
                                    item.set_busy_action("".into());
                                    set_confirm_issue(&item, &text, actions);
                                }
                            });
                        }
                    });
                }
                "reject" => {
                    let Some(item) = window.upgrade() else { return };
                    if item.get_busy() {
                        return;
                    }
                    let Some(offer) = pending.lock().ok().and_then(|items| items.front().cloned())
                    else {
                        return;
                    };
                    let suppress_site_kind = item.get_suppress_site_kind();
                    item.set_rejecting(true);
                    item.set_busy_action("reject".into());
                    item.set_busy(true);
                    clear_confirm_issue(&item);
                    let weak = item.as_weak();
                    let active_handoff = Arc::clone(&active_handoff);
                    thread::spawn(move || {
                        let handoff_id = offer.handoff_id;
                        let result = CoreIpcClient::connect().and_then(|mut client| {
                            client
                                .command(CoreCommand::RejectHandoff {
                                    handoff_id: handoff_id.clone(),
                                    suppress_site_kind,
                                })
                                .and_then(core_feedback::command_result)
                        });
                        if let Err(error) = result {
                            let _ =
                                weak.upgrade_in_event_loop(move |item| {
                                    if active_handoff.lock().ok().is_some_and(|id| {
                                        id.as_deref() == Some(handoff_id.as_str())
                                    }) {
                                        item.set_busy(false);
                                        item.set_busy_action("".into());
                                        set_confirm_issue(
                                            &item,
                                            &format!("取消未送达主程序：{error}"),
                                            [("重试取消", "reject"), ("", "")],
                                        );
                                    }
                                });
                        }
                    });
                }
                "browse" => {
                    let Some(item) = window.upgrade() else {
                        return;
                    };
                    let Some(handoff_id) = active_handoff.lock().ok().and_then(|id| id.clone())
                    else {
                        return;
                    };
                    let picked = file_dialogs::folder(native_window::hwnd(item.window()));
                    // The modal picker pumps events, including resolution of the displayed offer.
                    if !active_handoff
                        .lock()
                        .ok()
                        .is_some_and(|id| id.as_ref() == Some(&handoff_id))
                    {
                        return;
                    }
                    match picked {
                        Ok(Some(path)) => {
                            offer_state
                                .confirm_dir_overridden
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                            item.set_download_dir(path.clone().into());
                            sync_confirm_save_preview(&item);
                            if let Ok(mut settings) = presenter_settings.lock() {
                                settings.record_recent_dir(&path);
                                sync_confirm_selectors(&item, &settings, &path, None);
                            }
                            clear_confirm_issue(&item);
                            refresh_confirm_duplicate(&item, &offer_state);
                            refresh_confirm_disk_space(&item, current_pending_size(&pending));
                        }
                        Ok(None) => {}
                        Err(error) => set_confirm_issue(&item, &error, [("", ""), ("", "")]),
                    }
                    return;
                }
                "recent_dir" => {
                    if let Some(item) = window.upgrade() {
                        offer_state
                            .confirm_dir_overridden
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        sync_confirm_save_preview(&item);
                        clear_confirm_issue(&item);
                        refresh_confirm_duplicate(&item, &offer_state);
                        refresh_confirm_disk_space(&item, current_pending_size(&pending));
                    }
                    return;
                }
                "dir_edited" => {
                    if let Some(item) = window.upgrade() {
                        offer_state
                            .confirm_dir_overridden
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        sync_confirm_save_preview(&item);
                        clear_confirm_issue(&item);
                        refresh_confirm_duplicate(&item, &offer_state);
                        refresh_confirm_disk_space(&item, current_pending_size(&pending));
                    }
                    return;
                }
                "toggle_details" => {
                    if let Some(item) = window.upgrade() {
                        item.set_details_expanded(!item.get_details_expanded());
                        fit_confirm_window(&item);
                    }
                    return;
                }
                "filename_edited" => {
                    if let Some(item) = window.upgrade() {
                        let current_offer =
                            pending.lock().ok().and_then(|items| items.front().cloned());
                        let resource_kind = current_offer
                            .as_ref()
                            .map_or(ResourceKind::File, |offer| offer.resource_kind);
                        let expected_size = current_offer.as_ref().map_or(0, |offer| offer.size);
                        if let Ok(settings) = presenter_settings.lock() {
                            sync_confirm_filename_category(
                                &item,
                                &settings,
                                resource_kind,
                                offer_state
                                    .confirm_dir_overridden
                                    .load(std::sync::atomic::Ordering::Relaxed),
                            );
                        }
                        sync_confirm_save_preview(&item);
                        if let Some(offer) = current_offer.as_ref() {
                            item.set_resource_meta(
                                format_resource_meta(offer, &item.get_filename()).into(),
                            );
                        }
                        refresh_confirm_duplicate(&item, &offer_state);
                        refresh_confirm_disk_space(&item, expected_size);
                    }
                    return;
                }
                "recheck_duplicate" => {
                    if let Some(item) = window.upgrade() {
                        refresh_confirm_duplicate(&item, &offer_state);
                        refresh_confirm_disk_space(&item, current_pending_size(&pending));
                    }
                    return;
                }
                "default_dir" | "fix_name" | "shorten" => {
                    let Some(item) = window.upgrade() else {
                        return;
                    };
                    let filename = item.get_filename().to_string();
                    let directory = item.get_download_dir().to_string();
                    match command.as_str() {
                        "default_dir" => {
                            offer_state
                                .confirm_dir_overridden
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                            let default_dir = hls_client::default_v7_download_dir()
                                .to_string_lossy()
                                .into_owned();
                            item.set_download_dir(default_dir.clone().into());
                            if let Ok(settings) = presenter_settings.lock() {
                                sync_confirm_selectors(&item, &settings, &default_dir, None);
                            }
                        }
                        "fix_name" => item.set_filename(target_check::fix_name(&filename).into()),
                        "shorten" => {
                            let Some(short) = target_check::shorten_to_fit(&directory, &filename)
                            else {
                                set_confirm_issue(
                                    &item,
                                    "目录太长，无法保留文件类型。请选择较短的保存位置。",
                                    [("选择其他位置", "browse"), ("", "")],
                                );
                                return;
                            };
                            item.set_filename(short.into());
                        }
                        _ => unreachable!(),
                    }
                    if matches!(command.as_str(), "fix_name" | "shorten") {
                        let resource_kind = pending
                            .lock()
                            .ok()
                            .and_then(|items| items.front().map(|offer| offer.resource_kind))
                            .unwrap_or(ResourceKind::File);
                        if let Ok(settings) = presenter_settings.lock() {
                            sync_confirm_filename_category(
                                &item,
                                &settings,
                                resource_kind,
                                offer_state
                                    .confirm_dir_overridden
                                    .load(std::sync::atomic::Ordering::Relaxed),
                            );
                        }
                    }
                    sync_confirm_save_preview(&item);
                    clear_confirm_issue(&item);
                    refresh_confirm_duplicate(&item, &offer_state);
                    refresh_confirm_disk_space(&item, current_pending_size(&pending));
                    return;
                }
                "open_existing" => {
                    if let Some(item) = window.upgrade() {
                        item.invoke_open_workbench(true, "".into());
                        let dup_id = item.get_duplicate_task_id().to_string();
                        if !dup_id.is_empty() {
                            let current = workbench.borrow().clone();
                            if let Some(wb) = current.as_ref() {
                                wb.window
                                    .invoke_action(format!("focus_task:{dup_id}").into());
                            }
                        }
                    }
                    return;
                }
                "more" => {
                    let Some(item) = window.upgrade() else {
                        return;
                    };
                    if item.get_busy() {
                        return;
                    }
                    let offer = pending.lock().ok().and_then(|items| items.front().cloned());
                    let filename = item.get_filename().to_string();
                    let download_dir = item.get_download_dir().trim().to_string();
                    let queue_id = presenter_settings
                        .lock()
                        .map(|settings| settings.queue_id_at(item.get_queue_index()))
                        .unwrap_or_else(|_| hls_client::DEFAULT_QUEUE_ID.to_string());
                    item.invoke_open_workbench(true, "".into());
                    if let Some(offer) = offer {
                        let current = workbench.borrow().clone();
                        if let Some(wb) = current.as_ref() {
                            wb.open_editor_from_offer(&offer, &filename, &download_dir, &queue_id);
                        }
                        item.set_rejecting(true);
                        item.set_busy_action("reject".into());
                        item.set_busy(true);
                        clear_confirm_issue(&item);
                        let weak = item.as_weak();
                        let active_handoff = Arc::clone(&active_handoff);
                        thread::spawn(move || {
                            let handoff_id = offer.handoff_id;
                            let result = CoreIpcClient::connect().and_then(|mut client| {
                                client
                                    .command(CoreCommand::RejectHandoff {
                                        handoff_id: handoff_id.clone(),
                                        suppress_site_kind: false,
                                    })
                                    .and_then(core_feedback::command_result)
                            });
                            if let Err(error) = result {
                                let _ = weak.upgrade_in_event_loop(move |item| {
                                    if active_handoff.lock().ok().is_some_and(|id| {
                                        id.as_deref() == Some(handoff_id.as_str())
                                    }) {
                                        item.set_busy(false);
                                        item.set_busy_action("".into());
                                        set_confirm_issue(
                                            &item,
                                            &format!("未能关闭确认窗口：{error}"),
                                            [("重试关闭", "reject"), ("", "")],
                                        );
                                    }
                                });
                            }
                        });
                    }
                    return;
                }
                _ if command.starts_with("category:") => {
                    let category = command.trim_start_matches("category:");
                    if let (Some(item), Ok(settings)) =
                        (window.upgrade(), presenter_settings.lock())
                    {
                        offer_state
                            .confirm_dir_overridden
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        item.set_category(category.into());
                        let dir = settings.directory_for(category);
                        item.set_download_dir(dir.clone().into());
                        sync_confirm_save_preview(&item);
                        sync_confirm_selectors(&item, &settings, &dir, None);
                        refresh_confirm_duplicate(&item, &offer_state);
                        refresh_confirm_disk_space(&item, current_pending_size(&pending));
                    }
                    return;
                }
                _ => return,
            }
            show_next_offer(&window, &offer_state);
        }
    });
    confirm.window().on_close_requested({
        let weak = confirm.as_weak();
        move || {
            if let Some(item) = weak.upgrade() {
                if !item.get_busy() {
                    item.invoke_command("reject".into());
                }
            }
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
    complete.on_command({
        let completed_task = Rc::clone(&completed_task);
        let complete_shown_at = Rc::clone(&complete_shown_at);
        let window = complete.as_weak();
        move |command| {
            let Some(item) = window.upgrade() else { return };
            if command == "hide_complete" {
                complete_shown_at.set(None);
                let _ = item.hide();
                return;
            }
            if item.get_action_busy() {
                return;
            }
            let cancel_power = command == "cancel_power";
            let confirm_power = command == "confirm_power";
            let power_action = cancel_power || confirm_power;
            let task_id = completed_task.borrow().clone();
            let request = if cancel_power {
                CoreCommand::CancelPowerAction
            } else if confirm_power {
                CoreCommand::ConfirmPowerAction
            } else if command == "open_file" || command == "open_folder" {
                let Some(task_id) = task_id.clone() else {
                    item.set_action_text("无法找到已完成任务，请在主窗口打开任务详情".into());
                    return;
                };
                CoreCommand::OpenCompleted {
                    task_id,
                    folder: command == "open_folder",
                }
            } else {
                return;
            };
            item.set_action_busy(true);
            item.set_action_text("".into());
            let result_window = window.clone();
            thread::spawn(move || {
                let result = CoreIpcClient::connect()
                    .and_then(|mut client| client.command(request))
                    .and_then(core_feedback::command_result);
                let _ = result_window.upgrade_in_event_loop(move |item| {
                    // 下一项下载完成后，旧窗口的打开文件结果不能隐藏新通知；
                    // 但取消/立即执行电源动作是全局指令，不绑定特定任务 ID。
                    if !power_action
                        && item.get_task_id().as_str() != task_id.as_deref().unwrap_or("")
                    {
                        return;
                    }
                    item.set_action_busy(false);
                    match result {
                        Ok(_) if cancel_power => {
                            item.set_power_hint("".into());
                            item.set_power_action_label("".into());
                            item.set_power_seconds(0);
                            if item.get_task_id().is_empty() {
                                let _ = item.hide();
                            }
                        }
                        Ok(_) if confirm_power => {
                            item.set_power_hint("".into());
                            item.set_power_action_label("".into());
                            item.set_power_seconds(0);
                            let _ = item.hide();
                        }
                        Ok(_) => {
                            let _ = item.hide();
                        }
                        Err(error) => item.set_action_text(format!("操作失败：{error}").into()),
                    }
                });
            });
        }
    });

    let (tx, rx) = mpsc::sync_channel::<Vec<EventEnvelope>>(8);
    thread::spawn({
        let confirm = confirm.as_weak();
        let event_state = offer_state.clone();
        move || event_loop(tx, initial_sequence, confirm, event_state)
    });
    let clipboard_filter = Rc::new(RefCell::new(ClipboardOfferFilter::default()));
    // IPC 线程只在收到事件后唤醒界面；空闲时不再每秒轮询 250 次。
    confirm.on_core_events_ready({
        let workbench = Rc::clone(&workbench);
        let native_workbench = Rc::clone(&native_workbench);
        let ui_sequence = Rc::clone(&ui_sequence);
        let active_hud = Arc::clone(&active_hud);
        let rendered_hud_task = Arc::clone(&rendered_hud_task);
        let completed_task = Rc::clone(&completed_task);
        let completed_notified = Rc::clone(&completed_notified);
        let complete_shown_at = Rc::clone(&complete_shown_at);
        let clipboard_filter = Rc::clone(&clipboard_filter);
        let settings_reload = settings_reload.clone();
        let runtime_settings = Rc::clone(&runtime_settings);
        let presenter_settings = Arc::clone(&presenter_settings);
        let open_folder_on_complete = Rc::clone(&open_folder_on_complete);
        let progress_user_moved = Arc::clone(&progress_user_moved);
        let confirm = confirm.as_weak();
        let progress = progress.as_weak();
        let complete = complete.as_weak();
        move || {
            while let Ok(response) = settings_responses.try_recv() {
                let next = presenter_settings.lock().ok().and_then(|mut settings| {
                    update_presenter_settings(
                        &response,
                        &mut settings,
                        &mut runtime_settings.borrow_mut(),
                    )
                });
                if let Some(next) = next {
                    if let (Some(confirm), Some(progress), Some(complete)) =
                        (confirm.upgrade(), progress.upgrade(), complete.upgrade())
                    {
                        apply_runtime_settings(&confirm, &progress, &complete, next);
                        if let Ok(settings) = presenter_settings.lock() {
                            if !matches!(settings.completion_power_action.trim(), "" | "none") {
                                open_folder_on_complete.set(false);
                            }
                            sync_progress_settings(
                                &progress,
                                &settings,
                                open_folder_on_complete.get(),
                            );
                            if confirm.window().is_visible() {
                                let current_dir = confirm.get_download_dir().to_string();
                                sync_confirm_selectors(&confirm, &settings, &current_dir, None);
                            }
                        }
                        if !next.show_progress {
                            let _ = progress.hide();
                        }
                        if !next.show_complete {
                            let _ = complete.hide();
                        }
                    }
                }
            }
            // 先全部取出再分发，避免嵌套模态消息循环重入时借用冲突。
            let mut batches = Vec::new();
            while let Ok(events) = rx.try_recv() {
                batches.push(events);
            }
            for events in batches {
                for envelope in events {
                    ui_sequence.set(ui_sequence.get().max(envelope.sequence));
                    if matches!(envelope.event, CoreEvent::Shutdown) {
                        trace("received CoreEvent::Shutdown; exiting presenter event loop");
                        let _ = slint::quit_event_loop();
                        return;
                    }
                    if native_workbench.get()
                        && workbench.borrow().is_none()
                        && (matches!(
                            &envelope.event,
                            CoreEvent::UiShow { surface }
                                if surface == "main" || surface == "new_download"
                        ) || matches!(
                            &envelope.event,
                            CoreEvent::MediaPushRequested { .. }
                                | CoreEvent::DuplicateOffered { .. }
                        ))
                    {
                        if let Some(confirm) = confirm.upgrade() {
                            confirm.invoke_open_workbench(true, "".into());
                        }
                    }
                    let current_workbench = workbench.borrow().clone();
                    if let Some(window) = current_workbench.as_ref() {
                        window.apply(&envelope);
                    }
                    match envelope.event {
                        CoreEvent::HandoffOffered { .. } | CoreEvent::HandoffResolved { .. } => {}
                        CoreEvent::SettingsChanged { .. } => {
                            let _ = settings_reload.send(());
                        }
                        CoreEvent::ClipboardOffer { urls } => {
                            if let Some(urls) = clipboard_filter
                                .borrow_mut()
                                .accept(&urls, std::time::Instant::now())
                            {
                                if native_workbench.get() && workbench.borrow().is_none() {
                                    if let Some(confirm) = confirm.upgrade() {
                                        confirm.invoke_open_workbench(true, "".into());
                                    }
                                }
                                let current_workbench = workbench.borrow().clone();
                                if let Some(window) = current_workbench.as_ref() {
                                    window.open_clipboard_offer(&urls);
                                }
                            }
                        }
                        CoreEvent::TaskCreated { snapshot }
                        | CoreEvent::TaskUpdated { snapshot }
                        | CoreEvent::TaskProgress { snapshot } => {
                            let fallback_dir = presenter_settings
                                .lock()
                                .map(|settings| settings.download_dir.clone())
                                .unwrap_or_default();
                            let workbench_visible = current_workbench
                                .as_ref()
                                .is_some_and(|window| window.window.window().is_visible());
                            update_task_windows(
                                snapshot,
                                PresenterRuntimeSettings {
                                    workbench_visible,
                                    ..*runtime_settings.borrow()
                                },
                                open_folder_on_complete.get(),
                                &fallback_dir,
                                &progress,
                                &complete,
                                &active_hud,
                                &rendered_hud_task,
                                &completed_task,
                                &completed_notified,
                                Some(&complete_shown_at),
                                Some(&progress_user_moved),
                            );
                        }
                        CoreEvent::TaskDeleted { task_id } => {
                            completed_notified.borrow_mut().remove(&task_id);
                            if completed_task.borrow().as_deref() == Some(task_id.as_str()) {
                                *completed_task.borrow_mut() = None;
                                complete_shown_at.set(None);
                                if let Some(item) = complete.upgrade() {
                                    item.set_task_id("".into());
                                    let _ = item.hide();
                                }
                            }
                            let (primary, position, processing_elapsed) = active_hud
                                .lock()
                                .map(|mut hud| {
                                    let primary = hud.remove(&task_id);
                                    let pos = hud.task_position();
                                    let elapsed = primary.as_ref().and_then(|task| {
                                        hud.processing_elapsed_secs(
                                            &task.task_id,
                                            std::time::Instant::now(),
                                        )
                                    });
                                    (primary, pos, elapsed)
                                })
                                .unwrap_or((None, (0, 0), None));
                            let fallback_dir = presenter_settings
                                .lock()
                                .map(|settings| settings.download_dir.clone())
                                .unwrap_or_default();
                            match primary {
                                Some(primary) => {
                                    reset_progress_action_for_new_task(
                                        &progress,
                                        &rendered_hud_task,
                                        &primary.task_id,
                                    );
                                    render_progress_hud(
                                        &progress,
                                        PresenterRuntimeSettings {
                                            workbench_visible: current_workbench
                                                .as_ref()
                                                .is_some_and(|w| w.window.window().is_visible()),
                                            ..*runtime_settings.borrow()
                                        },
                                        &primary,
                                        position,
                                        &fallback_dir,
                                        processing_elapsed,
                                        Some(&progress_user_moved),
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
                                if item.get_details().is_empty() {
                                    item.set_details("全部下载任务已完成".into());
                                }
                                item.set_power_hint(
                                    format!(
                                        "{} 将在 {} 秒后执行",
                                        power_label(&action),
                                        delay_seconds
                                    )
                                    .into(),
                                );
                                item.set_power_action_label(power_label(&action).into());
                                item.set_power_seconds(
                                    i32::try_from(delay_seconds).unwrap_or(i32::MAX),
                                );
                                complete_shown_at.set(Some(std::time::Instant::now()));
                                present_complete_toast(&item);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // Keep popup windows strictly hidden at startup so MainWindowTitle on
    // `--workbench` launch is always the Workbench window and zero ghost
    // topmost windows exist. Signal readiness on the first event-loop tick.
    let ready_timer = Timer::default();
    ready_timer.start(TimerMode::SingleShot, Duration::from_millis(1), || {
        write_ready_marker();
    });

    show_next_offer(&confirm.as_weak(), &offer_state);
    if args.iter().any(|arg| arg == "--workbench") {
        confirm.invoke_open_workbench(
            true,
            hls_client::workbench_launch_id().unwrap_or_default().into(),
        );
    }
    slint::run_event_loop_until_quit()?;
    processing_timer.stop();
    ready_timer.stop();
    Ok(())
}

/// Render the real production popup components without Core IPC so automated
/// visual tests can inspect the first visible frame.  This closes the gap left
/// by `--self-test`, which intentionally checks only component construction.
fn run_visual_fixture(kind: &str, dark: bool) -> Result<(), Box<dyn std::error::Error>> {
    let quit_timer = Timer::default();
    let fixture_seconds = env::var("HLS_V7_PRESENTER_FIXTURE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(20)
        .clamp(1, 600);
    quit_timer.start(
        TimerMode::SingleShot,
        Duration::from_secs(fixture_seconds),
        || {
            let _ = slint::quit_event_loop();
        },
    );

    match kind {
        "cast" => {
            let window = CastWindow::new()?;
            crate::native_window::apply_dialog_theme(&window, dark);
            window.global::<Tokens>().set_reduce_motion(true);
            window.show()?;
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "settings" => {
            let window = SettingsWindow::new()?;
            crate::native_window::apply_dialog_theme(&window, dark);
            window
                .global::<Tokens>()
                .set_reduce_motion(env::var_os("HLS_V7_RENDER_MOTION").is_none());
            window.set_groups(slint::ModelRc::new(slint::VecModel::from(vec![
                "通用".into(),
                "网络".into(),
            ])));
            window.set_site_rules(slint::ModelRc::new(slint::VecModel::from(vec![
                SiteRuleRow {
                    host: "media.example.test".into(),
                    enabled: true,
                    ..Default::default()
                },
            ])));
            window.set_selected_group("网络".into());
            window.show()?;
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "workbench" => {
            let tasks = (0..1000)
                .map(|index| TaskSnapshot {
                    task_id: format!("fixture-{index:04}"),
                    filename: format!("示例视频 {index:04}.mp4"),
                    status: if index % 3 == 0 {
                        "completed"
                    } else {
                        "downloading"
                    }
                    .into(),
                    queue_index: index,
                    downloaded_bytes: if index % 3 == 0 {
                        128 * 1024 * 1024
                    } else {
                        32 * 1024 * 1024
                    },
                    total_bytes: Some(128 * 1024 * 1024),
                    speed_bytes_per_sec: if index % 3 == 0 { 0 } else { 1024 * 1024 },
                    available_actions: if index % 3 == 0 {
                        vec!["open".into(), "open_folder".into(), "delete".into()]
                    } else {
                        vec!["pause".into(), "cancel".into()]
                    },
                    ..Default::default()
                })
                .collect();
            let workbench = workbench::Workbench::new(
                tasks,
                1,
                true,
                dark,
                env::var_os("HLS_V7_RENDER_MOTION").is_none(),
            )?;
            offscreen::OffscreenPlatform::sync_workbench_viewport(&workbench.window);
            workbench.show()?;
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "confirm" | "confirm-error" | "confirm-long" | "confirm-details" | "confirm-busy" => {
            let window = ConfirmWindow::new()?;
            window.on_move_window({
                let weak = window.as_weak();
                move |dx, dy| {
                    if let Some(item) = weak.upgrade() {
                        move_window(item.window(), dx, dy);
                    }
                }
            });
            window.set_window_title(CONFIRM.into());
            crate::native_window::apply_theme(&window, dark);
            window
                .global::<Tokens>()
                .set_reduce_motion(env::var_os("HLS_V7_RENDER_MOTION").is_none());
            window.set_filename("示例视频 · 1080p.mp4".into());
            window.set_download_dir(r"D:\下载\媒体".into());
            window.set_url("media.example.test/library/example-video.mp4".into());
            window.set_source_url("video.example.test/watch/42".into());
            window.set_source_host("video.example.test".into());
            window.set_download_host("media.example.test".into());
            window.set_resource_meta("HTTP · .mp4 · 128.0 MB".into());
            window.set_request_context("已安全继承网页凭据".into());
            window.set_request_details("请求方式：GET · 内容类型：video/mp4".into());
            window.set_category("media".into());
            window.set_queue_names(slint::ModelRc::new(slint::VecModel::from(vec![
                "默认队列".into(),
                "夜间队列".into(),
            ])));
            window.set_queue_index(0);
            window.set_recent_dirs(slint::ModelRc::new(slint::VecModel::from(vec![
                r"D:\下载\媒体".into(),
                r"D:\下载".into(),
            ])));
            window.set_recent_dir_index(0);
            window.set_duplicate_text("已有同一地址的任务：示例视频.mp4（已暂停）".into());
            window.set_remaining("后面还有 2 个".into());
            window.set_free_space_hint("磁盘可用 186.4 GB".into());
            window.set_free_space_low(false);
            sync_confirm_save_preview(&window);
            if kind == "confirm-error" {
                let (text, actions) = target_check::TargetIssue::NoPermission.presentation();
                set_confirm_issue(&window, &text, actions);
            }
            if kind == "confirm-long" {
                let directory = r"D:\下载\媒体\课程资料\2026 秋季学期\第十二周 · 网络与分布式系统\课堂录像与讲义（含字幕与补充材料）\2026-11-18 课堂实录与课后答疑汇总\已整理归档\分布式系统课程（2026 秋）完整录像、讲义、作业与参考资料备份——请勿删除或移动此文件夹，否则同步脚本会失败（第二版修订稿）";
                let filename = "分布式一致性协议实战：从 Paxos 到 Raft 的工程化落地与故障演练全程录像（高清 1080p · 含中英双语字幕、课后答疑、助教板书整理、往年期末真题讲解与实验环境搭建步骤的完整合集，第一部分至第三部分，另附全部课堂讨论记录与小组项目展示录像）.mp4";
                window.set_download_dir(directory.into());
                window.set_filename(filename.into());
                sync_confirm_save_preview(&window);
                window.set_duplicate_text("".into());
                let units = target_check::path_units(directory, filename);
                debug_assert!(units > 259);
                let (text, actions) = target_check::TargetIssue::TooLong { units }.presentation();
                set_confirm_issue(&window, &text, actions);
            }
            if kind == "confirm-busy" {
                // 点击「开始下载」后、主程序回执前的状态。
                window.set_duplicate_text("".into());
                window.set_busy(true);
            }
            if kind == "confirm-details" {
                window.set_details_expanded(true);
                window.set_duplicate_text("".into());
            }
            window.on_command({
                let weak = window.as_weak();
                move |command| {
                    if command == "toggle_details" {
                        if let Some(item) = weak.upgrade() {
                            item.set_details_expanded(!item.get_details_expanded());
                            fit_confirm_window(&item);
                        }
                        return;
                    }
                    if command != "drag" {
                        let _ = slint::quit_event_loop();
                    }
                }
            });
            correct_native_click_position(window.window());
            window.window().set_size(slint::LogicalSize::new(
                520.0,
                if window.get_details_expanded() {
                    560.0
                } else {
                    468.0
                },
            ));
            window.show()?;
            round_native_window(window.window());
            fit_confirm_window(&window);
            // Standalone visual fixtures keep their taskbar identity so a
            // desktop reviewer can select the real production component.
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "progress" | "progress-expanded" => {
            let window = ProgressWindow::new()?;
            window.on_move_window({
                let weak = window.as_weak();
                move |dx, dy| {
                    if let Some(item) = weak.upgrade() {
                        move_window(item.window(), dx, dy);
                    }
                }
            });
            crate::native_window::apply_theme(&window, dark);
            window
                .global::<Tokens>()
                .set_reduce_motion(env::var_os("HLS_V7_RENDER_MOTION").is_none());
            populate_progress_fixture(&window, kind == "progress-expanded");
            correct_native_click_position(window.window());
            window.on_command({
                let weak = window.as_weak();
                move |command| {
                    if command == "toggle_expand" {
                        if let Some(item) = weak.upgrade() {
                            let next = !item.get_expanded();
                            resize_progress_window(&item, next);
                        }
                        return;
                    }
                    if command == "prev_task" || command == "next_task" {
                        if let Some(item) = weak.upgrade() {
                            let count = item.get_task_count().max(1);
                            let current = item.get_task_index().clamp(1, count) - 1;
                            let delta = if command == "prev_task" { -1 } else { 1 };
                            let next = (current + delta).rem_euclid(count) + 1;
                            item.set_task_index(next);
                        }
                        return;
                    }
                    let _ = slint::quit_event_loop();
                }
            });
            window.show()?;
            if kind == "progress-expanded" {
                resize_progress_window(&window, true);
            } else {
                round_native_window(window.window());
            }
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        "complete" => {
            let window = CompleteWindow::new()?;
            window.on_move_window({
                let weak = window.as_weak();
                move |dx, dy| {
                    if let Some(item) = weak.upgrade() {
                        move_window(item.window(), dx, dy);
                    }
                }
            });
            window.set_window_title(COMPLETE.into());
            crate::native_window::apply_theme(&window, dark);
            window
                .global::<Tokens>()
                .set_reduce_motion(env::var_os("HLS_V7_RENDER_MOTION").is_none());
            window.set_filename("示例视频 · 1080p.mp4".into());
            window.set_details("128.0 MB · 已保存到本地".into());
            window.set_power_hint("关机 将在 300 秒后执行".into());
            window.set_power_action_label("关机".into());
            window.set_power_seconds(300);
            correct_native_click_position(window.window());
            window.on_command(|_| {
                let _ = slint::quit_event_loop();
            });
            window.set_entered(false);
            window.show()?;
            play_complete_entrance(&window);
            round_native_window(window.window());
            write_ready_marker();
            slint::run_event_loop_until_quit()?;
        }
        _ => return Err(format!("unknown visual fixture: {kind}").into()),
    }
    quit_timer.stop();
    Ok(())
}

/// 完成窗口复用同一个实例：每次显示后下一帧再把 entered 置真，完成标记才会真正播放一次进场。
fn play_complete_entrance(item: &CompleteWindow) {
    let weak = item.as_weak();
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        if let Some(item) = weak.upgrade() {
            item.set_entered(true);
        }
    });
}

fn write_ready_marker() {
    let Some(path) = env::var_os("HLS_V7_PRESENTER_READY_FILE") else {
        return;
    };
    let _ = std::fs::write(path, b"ready\n");
}

fn connect_or_start_core() -> Result<CoreIpcClient, String> {
    if let Ok(client) = CoreIpcClient::connect_existing(Duration::from_millis(25)) {
        return Ok(client);
    }
    #[cfg(windows)]
    let _launch_guard = hls_client::instance::lock_core_launch()?;
    if let Ok(client) = CoreIpcClient::connect_existing(Duration::from_millis(25)) {
        return Ok(client);
    }
    let root = install_root().ok_or_else(|| "找不到应用安装目录".to_string())?;
    hls_client::clear_user_shutdown();
    spawn_core(&root)?;
    // One connection deadline includes all pipe retries. Repeating connect()
    // would multiply its two-second timeout and hide startup errors for minutes.
    CoreIpcClient::connect_existing(Duration::from_secs(10))
        .map_err(|error| format!("下载引擎启动失败：{error}"))
}

/// Persist a remembered category directory without clobbering concurrent
/// workbench edits: fetch fresh settings from the Core, merge only the single
/// category being remembered into the fresh map, then write the whole setting
/// back. The presenter-local cache always records the user's choice so the
/// rest of this session offers the same directory.
fn remember_category_directory(
    client: &mut CoreIpcClient,
    presenter_settings: &Arc<Mutex<PresenterSettings>>,
    category: &str,
    directory: String,
) -> Result<(), String> {
    if let Ok(mut settings) = presenter_settings.lock() {
        settings.remember(category, directory.clone());
    }
    let response = client.load_settings()?;
    // Never write back a derived default: a non-Settings response must not
    // blank the other three categories in the stored map.
    let CorePipeResponse::Settings { .. } = &response else {
        return Err(core_feedback::response_error(&response)
            .unwrap_or("下载引擎返回了无效的设置响应")
            .to_owned());
    };
    let mut fresh = PresenterSettings::from_response(Some(&response));
    fresh.remember(category, directory);
    client
        .command(CoreCommand::SetSetting {
            key: "browser_category_dirs".into(),
            value: serde_json::Value::String(fresh.category_dirs_json()),
        })
        .and_then(core_feedback::command_result)
}

#[derive(Clone)]
struct EventLoopState {
    pending: Arc<Mutex<VecDeque<ResourceOffer>>>,
    active_handoff: Arc<Mutex<Option<String>>>,
    presenter_settings: Arc<Mutex<PresenterSettings>>,
    known_tasks: Arc<Mutex<Vec<TaskSnapshot>>>,
    known_task_variants: Arc<Mutex<HashMap<String, VariantSelection>>>,
    quick_failed: Arc<Mutex<HashSet<String>>>,
    confirm_dir_overridden: Arc<std::sync::atomic::AtomicBool>,
    core_online: Arc<std::sync::atomic::AtomicBool>,
    browser_connected: Arc<std::sync::atomic::AtomicBool>,
}

fn event_loop(
    tx: mpsc::SyncSender<Vec<EventEnvelope>>,
    mut after: u64,
    confirm: slint::Weak<ConfirmWindow>,
    state: EventLoopState,
) {
    loop {
        let Ok(mut client) = CoreIpcClient::connect() else {
            if state
                .core_online
                .swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                let _ = confirm
                    .upgrade_in_event_loop(|item| item.invoke_core_connection_changed(false));
            }
            thread::sleep(Duration::from_millis(200));
            continue;
        };
        let Ok((tasks, latest_sequence)) = client.snapshot_state() else {
            if state
                .core_online
                .swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                let _ = confirm
                    .upgrade_in_event_loop(|item| item.invoke_core_connection_changed(false));
            }
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        state
            .core_online
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Ok(page) = client.task_page(None, 1) {
            state
                .browser_connected
                .store(page.browser_connected, std::sync::atomic::Ordering::Relaxed);
        }
        if let Ok(mut known_tasks) = state.known_tasks.lock() {
            *known_tasks = tasks.clone();
        }
        if confirm
            .upgrade_in_event_loop(move |item| {
                item.invoke_core_connection_changed(true);
                item.invoke_core_resynced(latest_sequence.to_string().into());
            })
            .is_err()
        {
            return;
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
        if !active.is_empty()
            && (tx.send(active).is_err()
                || confirm
                    .upgrade_in_event_loop(|item| item.invoke_core_events_ready())
                    .is_err())
        {
            return;
        }
        let (restored, restored_variants) = load_handoff_state(&mut client);
        let resync_state = state.clone();
        if confirm
            .upgrade_in_event_loop(move |item| {
                if let Ok(mut failed) = resync_state.quick_failed.lock() {
                    let active_ids: HashSet<&str> =
                        restored.iter().map(|o| o.handoff_id.as_str()).collect();
                    failed.retain(|id| active_ids.contains(id.as_str()));
                }
                if let Ok(mut items) = resync_state.pending.lock() {
                    *items = restored;
                }
                if let Ok(mut variants) = resync_state.known_task_variants.lock() {
                    *variants = restored_variants;
                }
                show_next_offer(&item.as_weak(), &resync_state);
            })
            .is_err()
        {
            return;
        }
        if latest_sequence != after {
            trace(&format!(
                "snapshot advanced event cursor from {after} to {latest_sequence}"
            ));
        }
        // The snapshot replaces the preceding history, including a covered ring
        // or a restarted Core. Reusing the stale cursor would resync forever.
        after = latest_sequence;
        trace(&format!("event client connected after={after}"));
        let mut last_lease_renewal = std::time::Instant::now();
        while let Ok(events) = client.wait_events(after, 5_000) {
            if let Some(last) = events.last() {
                after = last.sequence;
            }
            if !events.is_empty() {
                trace(&format!("received {} events through {after}", events.len()));
            }
            let mut batched = Vec::new();
            let mut shutting_down = false;
            for envelope in events {
                match envelope.event {
                    CoreEvent::HandoffOffered { offer } => {
                        let offer_state = state.clone();
                        if confirm
                            .upgrade_in_event_loop(move |item| {
                                if let Ok(mut items) = offer_state.pending.lock() {
                                    if !items
                                        .iter()
                                        .any(|entry| entry.handoff_id == offer.handoff_id)
                                    {
                                        items.push_back(offer);
                                    }
                                }
                                show_next_offer(&item.as_weak(), &offer_state);
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    CoreEvent::HandoffResolved {
                        handoff_id,
                        task_id,
                        ..
                    } => {
                        let offer_state = state.clone();
                        if confirm
                            .upgrade_in_event_loop(move |item| {
                                if let Ok(mut failed) = offer_state.quick_failed.lock() {
                                    failed.remove(&handoff_id);
                                }
                                let resolved_variant =
                                    offer_state.pending.lock().ok().and_then(|mut items| {
                                        let variant = items
                                            .iter()
                                            .find(|entry| entry.handoff_id == handoff_id)
                                            .map(VariantSelection::from_offer);
                                        items.retain(|entry| entry.handoff_id != handoff_id);
                                        variant
                                    });
                                if let (Some(task_id), Some(variant)) = (task_id, resolved_variant)
                                {
                                    if let Ok(mut variants) = offer_state.known_task_variants.lock()
                                    {
                                        variants.insert(task_id, variant);
                                    }
                                }
                                show_next_offer(&item.as_weak(), &offer_state);
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    event => {
                        if matches!(event, CoreEvent::Shutdown) {
                            shutting_down = true;
                        }
                        if let CoreEvent::BrowserStatus { connected, .. } = &event {
                            state
                                .browser_connected
                                .store(*connected, std::sync::atomic::Ordering::Relaxed);
                        }
                        if let CoreEvent::TaskDeleted { ref task_id } = event {
                            if let Ok(mut variants) = state.known_task_variants.lock() {
                                variants.remove(task_id);
                            }
                        }
                        update_known_tasks(&state.known_tasks, &event);
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
                if confirm
                    .upgrade_in_event_loop(|item| item.invoke_core_events_ready())
                    .is_err()
                {
                    return;
                }
            }
            if shutting_down {
                trace("event loop stopping cleanly after CoreEvent::Shutdown");
                return;
            }
            if last_lease_renewal.elapsed() >= Duration::from_secs(5) {
                let active = state
                    .active_handoff
                    .lock()
                    .ok()
                    .and_then(|value| value.clone());
                if let Some(handoff_id) = active {
                    if client
                        .command(CoreCommand::PresentHandoff {
                            handoff_id,
                            ok: true,
                            presenter_id: presenter_id().into(),
                        })
                        .is_err()
                    {
                        if let Ok(mut value) = state.active_handoff.lock() {
                            *value = None;
                        }
                        let _ = confirm.upgrade_in_event_loop(|item| {
                            let _ = item.hide();
                        });
                        break;
                    }
                } else if state
                    .pending
                    .lock()
                    .map(|items| !items.is_empty())
                    .unwrap_or(false)
                {
                    let offer_state = state.clone();
                    let _ = confirm.upgrade_in_event_loop(move |item| {
                        show_next_offer(&item.as_weak(), &offer_state);
                    });
                }
                last_lease_renewal = std::time::Instant::now();
            }
        }
        if state
            .core_online
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            let _ =
                confirm.upgrade_in_event_loop(|item| item.invoke_core_connection_changed(false));
        }
    }
}

fn load_handoff_state(
    client: &mut CoreIpcClient,
) -> (VecDeque<ResourceOffer>, HashMap<String, VariantSelection>) {
    let encoded = match client.query_handoffs(None, None, false) {
        Ok(encoded) => encoded,
        Err(error) => {
            trace(&format!("恢复浏览器接管请求失败: {error}"));
            return (VecDeque::new(), HashMap::new());
        }
    };
    let mut pending = VecDeque::new();
    let mut variants = HashMap::new();
    for encoded in encoded {
        let handoff = match serde_json::from_str::<PersistedHandoff>(&encoded) {
            Ok(handoff) => handoff,
            Err(error) => {
                trace(&format!("忽略损坏的持久化接管请求: {error}"));
                continue;
            }
        };
        if let Some(task_id) = handoff.task_id.filter(|id| !id.is_empty()) {
            variants.insert(task_id, VariantSelection::from_offer(&handoff.offer));
        }
        if handoff.status == "pending"
            && handoff.presentation != "fallback"
            && !pending
                .iter()
                .any(|item: &ResourceOffer| item.handoff_id == handoff.offer.handoff_id)
        {
            pending.push_back(handoff.offer);
        }
    }
    (pending, variants)
}

// 输入为 rcWork 的物理尺寸；紧凑窗口不能超出可用工作区。
#[cfg(test)]
fn confirm_window_logical_size(work_area: (u32, u32), scale_factor: f32) -> Option<(f32, f32)> {
    confirm_window_logical_size_for(work_area, scale_factor, false)
}

fn confirm_window_logical_size_for(
    work_area: (u32, u32),
    scale_factor: f32,
    details_expanded: bool,
) -> Option<(f32, f32)> {
    if work_area.0 == 0 || work_area.1 == 0 || !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    let target_height = if details_expanded { 560.0 } else { 468.0 };
    Some((
        (work_area.0 as f32 / scale_factor).min(520.0),
        (work_area.1 as f32 / scale_factor).min(target_height),
    ))
}

fn format_save_preview(download_dir: &str, filename: &str) -> String {
    let dir = download_dir.trim();
    if dir.is_empty() {
        return filename.to_owned();
    }
    let trimmed = dir.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        format!("\\{filename}")
    } else {
        format!("{trimmed}\\{filename}")
    }
}

fn sync_confirm_save_preview(item: &ConfirmWindow) {
    let preview = format_save_preview(&item.get_download_dir(), &item.get_filename());
    item.set_save_preview(preview.into());
}

// 仅在真实确认或 fixture 的 show() 成功后调用；离屏预热不参与尺寸适配。
fn fit_confirm_window(confirm: &ConfirmWindow) {
    #[cfg(windows)]
    {
        let weak = confirm.as_weak();
        // show() 和首个计时器都可能早于 HWND 创建，等待 Winit 的实际创建通知。
        let _ = slint::spawn_local(async move {
            let Some(confirm) = weak.upgrade() else {
                return;
            };
            use slint::winit_030::{
                winit::{
                    dpi::{PhysicalPosition, PhysicalSize},
                    raw_window_handle::{HasWindowHandle, RawWindowHandle},
                },
                WinitWindowAccessor,
            };
            let native = match confirm.window().winit_window().await {
                Ok(native) => native,
                Err(error) => {
                    trace(&format!("fit confirm: {error}"));
                    return;
                }
            };
            if !confirm.window().is_visible() {
                return;
            }
            let Ok(handle) = native.window_handle() else {
                return;
            };
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return;
            };
            let Some((work_left, work_top, work_right, work_bottom)) =
                native_window::work_area(Some(handle.hwnd.get() as i64))
            else {
                trace(&format!(
                    "fit confirm: monitor query failed {}",
                    std::io::Error::last_os_error()
                ));
                return;
            };
            let work_width = (work_right - work_left) as u32;
            let work_height = (work_bottom - work_top) as u32;
            let scale = native.scale_factor() as f32;
            let Some((width, height)) = confirm_window_logical_size_for(
                (work_width, work_height),
                scale,
                confirm.get_details_expanded(),
            ) else {
                return;
            };
            let width = (width * scale).round() as u32;
            let height = (height * scale).round() as u32;
            trace(&format!(
                "fit confirm work={work_width}x{work_height} scale={scale} size={width}x{height}"
            ));
            let _ = native.request_inner_size(PhysicalSize::new(width, height));
            native.set_outer_position(PhysicalPosition::new(
                work_left + ((work_width - width) / 2) as i32,
                work_top + ((work_height - height) / 2) as i32,
            ));
            round_native_window(confirm.window());
        });
    }
    #[cfg(not(windows))]
    let _ = confirm;
}

fn sync_confirm_selectors(
    item: &ConfirmWindow,
    settings: &PresenterSettings,
    current_dir: &str,
    preferred_queue_id: Option<&str>,
) {
    let queue_names = settings.queue_names();
    let queue_index = match preferred_queue_id {
        Some(queue_id) => settings.queue_index_for_id(queue_id),
        None => {
            let current = item.get_queue_index();
            if current >= 0 && (current as usize) < queue_names.len() {
                current
            } else {
                settings.queue_index_for_id(hls_client::DEFAULT_QUEUE_ID)
            }
        }
    };
    item.set_queue_names(slint::ModelRc::new(slint::VecModel::from(queue_names)));
    item.set_queue_index(queue_index);
    let (recent_dirs, recent_index) = settings.recent_dirs_for(current_dir);
    item.set_recent_dirs(slint::ModelRc::new(slint::VecModel::from(
        recent_dirs
            .into_iter()
            .map(slint::SharedString::from)
            .collect::<Vec<_>>(),
    )));
    item.set_recent_dir_index(recent_index);
}

fn refresh_confirm_duplicate(item: &ConfirmWindow, state: &EventLoopState) {
    let Some(offer) = state
        .pending
        .lock()
        .ok()
        .and_then(|items| items.front().cloned())
    else {
        item.set_duplicate_text("".into());
        item.set_duplicate_task_id("".into());
        return;
    };
    let tasks = state
        .known_tasks
        .lock()
        .map(|tasks| tasks.clone())
        .unwrap_or_default();
    let variants = state
        .known_task_variants
        .lock()
        .map(|variants| variants.clone())
        .unwrap_or_default();
    let download_dir = item.get_download_dir().to_string();
    let filename = item.get_filename().to_string();
    let (duplicate_text, duplicate_task_id) =
        find_duplicate_match(&tasks, &variants, &offer, &download_dir, &filename)
            .map(format_duplicate_notice)
            .unwrap_or_default();
    item.set_duplicate_text(duplicate_text.into());
    item.set_duplicate_task_id(duplicate_task_id.into());
}

fn current_pending_size(pending: &Arc<Mutex<VecDeque<ResourceOffer>>>) -> u64 {
    pending
        .lock()
        .ok()
        .and_then(|items| items.front().map(|offer| offer.size))
        .unwrap_or(0)
}

fn refresh_confirm_disk_space(item: &ConfirmWindow, expected_size: u64) {
    let dir = item.get_download_dir().to_string();
    match target_check::query_available_space(&dir) {
        Some(free_bytes) => {
            let status = target_check::format_disk_space_status(free_bytes, expected_size);
            item.set_free_space_hint(status.hint.into());
            item.set_free_space_low(status.low_space);
        }
        None => {
            item.set_free_space_hint("".into());
            item.set_free_space_low(false);
        }
    }
}

fn sync_confirm_filename_category(
    item: &ConfirmWindow,
    settings: &PresenterSettings,
    resource_kind: ResourceKind,
    dir_overridden: bool,
) {
    let filename = item.get_filename().to_string();
    let next_category = download_category(&filename, "", resource_kind);
    if item.get_category().as_str() != next_category {
        item.set_category(next_category.into());
    }
    if !dir_overridden {
        let next_dir = settings.directory_for(next_category);
        if item.get_download_dir().as_str() != next_dir {
            item.set_download_dir(next_dir.clone().into());
            sync_confirm_selectors(item, settings, &next_dir, None);
        }
    }
}

fn show_next_offer(window: &slint::Weak<ConfirmWindow>, state: &EventLoopState) {
    let Some(item) = window.upgrade() else {
        return;
    };
    let Some(offer) = state
        .pending
        .lock()
        .ok()
        .and_then(|items| items.front().cloned())
    else {
        if let Ok(mut active) = state.active_handoff.lock() {
            *active = None;
        }
        let _ = item.hide();
        return;
    };
    if state
        .active_handoff
        .lock()
        .map(|active| active.as_deref() == Some(offer.handoff_id.as_str()))
        .unwrap_or(false)
    {
        return;
    }
    let settings_snapshot = state
        .presenter_settings
        .lock()
        .map(|settings| settings.clone())
        .unwrap_or_default();
    let variants_snapshot = state
        .known_task_variants
        .lock()
        .map(|variants| variants.clone())
        .unwrap_or_default();
    let tasks_snapshot = state
        .known_tasks
        .lock()
        .map(|tasks| tasks.clone())
        .unwrap_or_default();
    let already_failed_quick = state
        .quick_failed
        .lock()
        .map(|failed| failed.contains(&offer.handoff_id))
        .unwrap_or(false);
    if !already_failed_quick {
        if let Some(plan) = evaluate_quick_download(
            &offer,
            &settings_snapshot,
            &tasks_snapshot,
            &variants_snapshot,
        ) {
            trace(&format!(
                "quick download auto-accepting handoff {} ({})",
                plan.handoff_id, plan.filename
            ));
            if let Ok(mut active) = state.active_handoff.lock() {
                *active = Some(plan.handoff_id.clone());
            }
            let _ = item.hide();
            let weak = item.as_weak();
            let offer_state = state.clone();
            thread::spawn(move || {
                let result = CoreIpcClient::connect().and_then(|mut client| {
                    let events = client.command(CoreCommand::AcceptHandoff {
                        handoff_id: plan.handoff_id.clone(),
                        filename: plan.filename.clone(),
                        download_dir: plan.download_dir.clone(),
                        allow_duplicate: false,
                        trusted_ui: true,
                        start_paused: false,
                        queue_id: plan.queue_id.clone(),
                    })?;
                    for envelope in &events {
                        update_known_tasks(&offer_state.known_tasks, &envelope.event);
                    }
                    if events
                        .iter()
                        .any(|e| matches!(e.event, CoreEvent::DuplicateOffered { .. }))
                    {
                        return Err("duplicate task detected".into());
                    }
                    core_feedback::command_result(events)
                });
                match result {
                    Ok(()) => {
                        if !plan.download_dir.is_empty() {
                            if let Ok(mut settings) = offer_state.presenter_settings.lock() {
                                settings.record_recent_dir(&plan.download_dir);
                            }
                        }
                    }
                    Err(error) => {
                        trace(&format!(
                            "quick download failed for {}: {error}; falling back to confirm window",
                            plan.handoff_id
                        ));
                        if let Ok(mut failed) = offer_state.quick_failed.lock() {
                            failed.insert(plan.handoff_id.clone());
                        }
                        if let Ok(mut active) = offer_state.active_handoff.lock() {
                            if active.as_deref() == Some(plan.handoff_id.as_str()) {
                                *active = None;
                            }
                        }
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            show_next_offer(&ui.as_weak(), &offer_state);
                        });
                    }
                }
            });
            return;
        }
    }
    trace(&format!("showing handoff {}", offer.handoff_id));
    let filename = if offer.filename.trim().is_empty() {
        filename_from_url(&offer.url)
    } else {
        offer.filename.clone()
    };
    let category = download_category(&filename, "", offer.resource_kind);
    let download_dir = settings_snapshot.directory_for(category);
    let (duplicate_text, duplicate_task_id) = find_duplicate_match(
        &tasks_snapshot,
        &variants_snapshot,
        &offer,
        &download_dir,
        &filename,
    )
    .map(format_duplicate_notice)
    .unwrap_or_default();
    state
        .confirm_dir_overridden
        .store(false, std::sync::atomic::Ordering::Relaxed);
    item.set_is_local_payload(
        offer
            .local_source_path
            .as_deref()
            .is_some_and(|path| !path.trim().is_empty()),
    );
    item.set_filename(filename.clone().into());
    item.set_download_dir(download_dir.clone().into());
    sync_confirm_save_preview(&item);
    item.set_category(category.into());
    sync_confirm_selectors(
        &item,
        &settings_snapshot,
        &download_dir,
        Some(hls_client::DEFAULT_QUEUE_ID),
    );
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
    item.set_duplicate_task_id(duplicate_task_id.into());
    item.set_details_expanded(false);
    item.set_remember_directory(false);
    item.set_suppress_site_kind(false);
    item.set_busy(false);
    item.set_busy_action("".into());
    clear_confirm_issue(&item);
    refresh_confirm_disk_space(&item, offer.size);
    let remaining = state
        .pending
        .lock()
        .map(|items| items.len().saturating_sub(1))
        .unwrap_or_default();
    item.set_remaining(if remaining > 0 {
        format!("后面还有 {remaining} 个").into()
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
            let (restored, restored_variants) = load_handoff_state(&mut client);
            if let Ok(mut items) = state.pending.lock() {
                *items = restored;
            }
            if let Ok(mut variants) = state.known_task_variants.lock() {
                *variants = restored_variants;
            }
        }
        if let Ok(mut active) = state.active_handoff.lock() {
            *active = None;
        }
        let _ = item.hide();
        return;
    }
    let quiet_fullscreen = native_window::is_foreground_fullscreen_or_busy();
    let shown = if quiet_fullscreen {
        native_window::show_non_activating(&item, false, true).is_ok()
    } else {
        native_window::set_cloaked(item.window(), true);
        native_window::set_no_activate(item.window(), false);
        item.show().is_ok()
    };
    if shown {
        if let Ok(mut active) = state.active_handoff.lock() {
            *active = Some(offer.handoff_id.clone());
        }
        if quiet_fullscreen {
            fit_confirm_window(&item);
        } else {
            fit_confirm_window(&item);
            native_window::prepare_dialog(&item, false, true, None);
        }
    } else {
        native_window::set_cloaked(item.window(), false);
        if let Ok(mut active) = state.active_handoff.lock() {
            *active = None;
        }
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
        if let Ok(mut items) = state.pending.lock() {
            items.retain(|item| item.handoff_id != offer.handoff_id);
        }
        item.invoke_open_workbench(true, "".into());
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

/// Subclass procedure suppressing standard non-client chrome on frameless Slint popups.
///
/// # Safety
/// Must only be invoked by `comctl32` subclass dispatch for an `HWND` subclassed via
/// `SetWindowSubclass` (or directly in unit tests with a valid or zero `WM_STYLECHANGING`
/// `STYLESTRUCT` pointer when `message == WM_STYLECHANGING` and `wparam as i32 == GWL_STYLE`).
#[cfg(windows)]
unsafe extern "system" fn frameless_window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    subclass_id: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass},
        WindowsAndMessaging::{
            GWL_STYLE, STYLESTRUCT, WM_NCACTIVATE, WM_NCDESTROY, WM_NCPAINT, WM_STYLECHANGING,
            WS_CAPTION,
        },
    };
    match message {
        // 保留 Winit 的焦点状态更新，但禁止 DefWindowProc 在失焦时绘制系统标题和白边。
        WM_NCACTIVATE => DefSubclassProc(hwnd, message, wparam, -1),
        WM_NCPAINT => 0,
        // Winit 在更新置顶/可见状态时会重新加上 WS_CAPTION；这些自绘浮窗不使用系统标题栏。
        WM_STYLECHANGING if wparam as i32 == GWL_STYLE => {
            let style = lparam as *mut STYLESTRUCT;
            if !style.is_null() {
                (*style).styleNew &= !WS_CAPTION;
            }
            0
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(frameless_window_proc), subclass_id);
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

/// 与 `Tokens.card-radius` 保持一致的无边框窗口圆角（逻辑像素）。
#[cfg_attr(not(windows), allow(dead_code))]
const CARD_RADIUS: f32 = 8.0;

#[cfg(windows)]
fn ready_size(native: &slint::winit_030::winit::window::Window, window: &slint::Window) -> bool {
    let actual = native.inner_size();
    let rendered = window.size();
    actual.width > 0
        && actual.height > 0
        && actual.width == rendered.width
        && actual.height == rendered.height
}

fn round_native_window(window: &slint::Window) {
    #[cfg(windows)]
    {
        use slint::winit_030::{
            winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
            WinitWindowAccessor,
        };
        use windows_sys::Win32::{
            Foundation::RECT,
            Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND},
            Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn},
            UI::WindowsAndMessaging::{
                GetWindowRect, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
                SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION,
            },
        };
        let ready_to_repaint = window.with_winit_window(|native| {
            let Ok(handle) = native.window_handle() else {
                return false;
            };
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return false;
            };
            let hwnd = handle.hwnd.get() as _;
            // SAFETY: `hwnd` belongs to `native` on the current GUI thread. `SetWindowSubclass`
            // with fixed `(frameless_window_proc, 1)` updates in place if already installed and
            // unregisters itself on `WM_NCDESTROY`. For the Windows 10 fallback, `SetWindowRgn`
            // takes OS ownership of `region` when it returns non-zero (so `DeleteObject` MUST NOT
            // be called), whereas on `SetWindowRgn == 0` `DeleteObject(region)` frees the `HRGN`.
            unsafe {
                windows_sys::Win32::UI::Shell::SetWindowSubclass(
                    hwnd,
                    Some(frameless_window_proc),
                    1,
                    0,
                );
                if let Some(style) = native_window::get_window_long_ptr(hwnd, GWL_STYLE) {
                    if style & WS_CAPTION as isize != 0
                        && native_window::set_window_long_ptr(
                            hwnd,
                            GWL_STYLE,
                            style & !(WS_CAPTION as isize),
                        )
                    {
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            0,
                            0,
                            0,
                            0,
                            SWP_FRAMECHANGED
                                | SWP_NOMOVE
                                | SWP_NOSIZE
                                | SWP_NOZORDER
                                | SWP_NOACTIVATE,
                        );
                    }
                }
                // Windows 11：交给 DWM 画抗锯齿圆角、系统描边和阴影，半径 8 与
                // Tokens.card-radius 一致；此时不能再设窗口区域，否则 DWM 圆角失效。
                let preference = DWMWCP_ROUND;
                let dwm_rounded = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    std::ptr::from_ref(&preference).cast(),
                    std::mem::size_of_val(&preference) as u32,
                ) >= 0;
                if dwm_rounded {
                    SetWindowRgn(hwnd, std::ptr::null_mut(), 1);
                    return ready_size(native, window);
                }
                // Windows 10 没有 DWM 圆角：用窗口区域裁出同样 8 逻辑像素的圆角。
                let mut bounds: RECT = std::mem::zeroed();
                if GetWindowRect(hwnd, &mut bounds) == 0 {
                    return false;
                }
                let diameter = (2.0 * CARD_RADIUS * window.scale_factor()).round() as i32;
                let region = CreateRoundRectRgn(
                    0,
                    0,
                    bounds.right - bounds.left + 1,
                    bounds.bottom - bounds.top + 1,
                    diameter,
                    diameter,
                );
                if !region.is_null() && SetWindowRgn(hwnd, region, 1) == 0 {
                    DeleteObject(region);
                }
            }
            ready_size(native, window)
        });
        if ready_to_repaint == Some(true) {
            // 隐藏后 Win32 画布会被清空，但 softbuffer 仍可能报告缓冲可复用。
            // 只在表面尺寸已同步后清缓存；首次高 DPI 创建时立即快照会使渲染缓冲越界。
            let _ = window.take_snapshot();
            window.request_redraw();
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

fn move_window(window: &slint::Window, dx: f32, dy: f32) {
    let position = window.position();
    let scale = window.scale_factor();
    trace(&format!(
        "move window by {dx},{dy} from {},{}",
        position.x, position.y
    ));
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
            if matches!(
                event,
                winit::event::WindowEvent::Resized(_)
                    | winit::event::WindowEvent::ScaleFactorChanged { .. }
            ) {
                round_native_window(window);
            }
            if let winit::event::WindowEvent::CursorMoved { position, .. } = event {
                let position = slint::LogicalPosition::new(
                    position.x as f32 / window.scale_factor(),
                    position.y as f32 / window.scale_factor(),
                );
                cursor_position.set(Some(position));
                let _ = window
                    .try_dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
                return EventResult::PreventDefault;
            }
            if matches!(event, winit::event::WindowEvent::CursorLeft { .. }) {
                if left_pressed.get() {
                    // 鼠标已捕获，离开窗口不代表取消拖动；Winit 的默认按下状态被拦截了。
                    return EventResult::PreventDefault;
                }
                cursor_position.set(None);
                return EventResult::Propagate;
            }
            if matches!(event, winit::event::WindowEvent::Focused(false)) {
                if left_pressed.replace(false) {
                    // SAFETY: `ReleaseCapture` takes no pointer arguments and releases mouse
                    // capture held by the calling GUI thread, if any.
                    unsafe {
                        ReleaseCapture();
                    }
                }
                cursor_position.set(None);
                return EventResult::Propagate;
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
                        // SAFETY: `point` is a writable stack `POINT`, and `handle.hwnd` is the
                        // live Win32 `HWND` borrowed from `native` on this GUI thread.
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
                    Some((
                        handle.hwnd.get() as windows_sys::Win32::Foundation::HWND,
                        position,
                    ))
                })
                .flatten();
            let Some((hwnd, position)) = native_position else {
                return EventResult::Propagate;
            };
            if *state == winit::event::ElementState::Pressed {
                // 默认鼠标事件被下方拦截，需自行捕获才能在拖出窗口后继续收到移动和释放。
                // SAFETY: `hwnd` was obtained from `with_winit_window` on the current GUI thread.
                unsafe {
                    SetCapture(hwnd);
                }
                left_pressed.set(true);
                let _ = window
                    .try_dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
            }
            let button = slint::platform::PointerEventButton::Left;
            let event = if *state == winit::event::ElementState::Pressed {
                slint::platform::WindowEvent::PointerPressed { position, button }
            } else {
                slint::platform::WindowEvent::PointerReleased { position, button }
            };
            if *state == winit::event::ElementState::Released {
                // 先释放鼠标捕获，再派发点击，避免回调里打开系统文件夹模态窗时仍占着捕获。
                // SAFETY: `ReleaseCapture` takes no pointer arguments and releases mouse capture on this thread.
                unsafe {
                    ReleaseCapture();
                }
                left_pressed.set(false);
                cursor_position.set(None);
            }
            trace(&format!(
                "native left button: {state:?} at {},{}",
                position.x, position.y
            ));
            let _ = window.try_dispatch_event(event);
            if *state == winit::event::ElementState::Released {
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
    theme_mode: theme::ThemeMode,
    reduce_motion: bool,
    workbench_visible: bool,
}

impl PresenterRuntimeSettings {
    fn from_response(response: Option<&CorePipeResponse>) -> Self {
        match response {
            Some(CorePipeResponse::Settings {
                progress_window_enabled,
                complete_popup_enabled,
                completion_sound_enabled,
                reduce_motion,
                ..
            }) => Self {
                show_progress: *progress_window_enabled,
                show_complete: *complete_popup_enabled,
                sound_enabled: *completion_sound_enabled,
                dark: theme::ThemeMode::from_response(response).is_dark(),
                theme_mode: theme::ThemeMode::from_response(response),
                reduce_motion: *reduce_motion || os_reduce_motion(),
                workbench_visible: false,
            },
            _ => Self {
                dark: theme::ThemeMode::System.is_dark(),
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
    native_window::apply_appearance(confirm, settings.dark, settings.reduce_motion);
    native_window::apply_appearance(progress, settings.dark, settings.reduce_motion);
    native_window::apply_appearance(complete, settings.dark, settings.reduce_motion);
}

fn is_processing_status(status: &str) -> bool {
    matches!(
        status,
        "merging" | "checking" | "processing" | "remuxing" | "finalizing"
    )
}

fn task_is_active(status: &str) -> bool {
    matches!(status, "downloading" | "recording") || is_processing_status(status)
}

fn format_elapsed_duration(seconds: u64) -> String {
    if seconds >= 3600 {
        format!(
            "{}h{}m{}s",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )
    } else if seconds >= 60 {
        format!("{}m{}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn processing_badge_label(status: &str) -> &'static str {
    match status {
        "merging" | "remuxing" => "合并中",
        "checking" => "校验中",
        _ => "处理中",
    }
}

fn format_processing_headline(status: &str, elapsed_secs: Option<u64>) -> String {
    let base = match status {
        "merging" | "remuxing" => "合并音视频中",
        "checking" => "校验文件中",
        _ => "本地处理中",
    };
    match elapsed_secs {
        Some(seconds) => format!("{base} · 已耗时 {}", format_elapsed_duration(seconds)),
        None => base.to_owned(),
    }
}

const COMPLETION_ACTION_LABELS: &[&str] = &["无操作", "打开目录", "睡眠", "休眠", "关机"];

fn progress_window_logical_size(expanded: bool) -> (f32, f32) {
    (
        PROGRESS_HUD_WIDTH,
        if expanded {
            PROGRESS_HUD_EXPANDED_HEIGHT
        } else {
            PROGRESS_HUD_COMPACT_HEIGHT
        },
    )
}

fn resize_progress_window(item: &ProgressWindow, expanded: bool) {
    let (width, height) = progress_window_logical_size(expanded);
    item.set_expanded(expanded);
    native_window::set_window_logical_size(item.window(), width, height);
    round_native_window(item.window());
    native_window::fit_to_work_area(item.window());
    item.window().request_redraw();
    let weak = item.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(item) = weak.upgrade() else {
            return;
        };
        let (width, height) = progress_window_logical_size(item.get_expanded());
        native_window::set_window_logical_size(item.window(), width, height);
        round_native_window(item.window());
        native_window::fit_to_work_area(item.window());
        item.window().request_redraw();
    });
}

fn build_speed_limit_settings(
    enabled: bool,
    kib: u64,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut values = std::collections::BTreeMap::new();
    values.insert(
        "download_speed_limit_enabled".into(),
        serde_json::Value::Bool(enabled && kib > 0),
    );
    values.insert(
        "download_speed_limit_kib".into(),
        serde_json::Value::from(kib),
    );
    values
}

fn toggle_hud_speed_limit(
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
        "download_speed_limit_enabled".into(),
        serde_json::Value::Bool(next_enabled),
    );
    let effective_kib = if next_enabled && current_kib == 0 {
        values.insert(
            "download_speed_limit_kib".into(),
            serde_json::Value::from(1024u64),
        );
        1024
    } else {
        current_kib
    };
    (next_enabled, effective_kib, values)
}

fn format_hud_speed_limit_feedback(enabled: bool, kib: u64) -> String {
    if !enabled || kib == 0 {
        return "已取消限速".into();
    }
    if kib >= 1024 {
        if kib.is_multiple_of(1024) {
            format!("已限速 {} MB/s", kib / 1024)
        } else {
            format!("已限速 {:.1} MB/s", kib as f64 / 1024.0)
        }
    } else {
        format!("已限速 {kib} KB/s")
    }
}

fn store_settings_via_ipc(
    values: std::collections::BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    let mut client = CoreIpcClient::connect()?;
    match client.request(&CorePipeRequest::StoreSettings {
        request_id: 1,
        values,
    })? {
        CorePipeResponse::Settings { .. } | CorePipeResponse::Events { .. } => Ok(()),
        CorePipeResponse::Error { message, .. } => Err(message),
        other => Err(format!("意外的设置响应：{other:?}")),
    }
}

fn completion_action_index_for(power_action: &str, open_folder_on_complete: bool) -> i32 {
    match power_action.trim() {
        "sleep" => 2,
        "hibernate" => 3,
        "shutdown" => 4,
        _ if open_folder_on_complete => 1,
        _ => 0,
    }
}

/// Maps HUD combo box index to `(core_completion_power_action, open_folder_on_complete)`.
/// Core only accepts `"none"`, `"sleep"`, `"hibernate"`, `"shutdown"`.
fn completion_action_from_index(index: i32) -> (&'static str, bool) {
    match index {
        1 => ("none", true),
        2 => ("sleep", false),
        3 => ("hibernate", false),
        4 => ("shutdown", false),
        _ => ("none", false),
    }
}

fn sync_progress_settings(
    item: &ProgressWindow,
    settings: &PresenterSettings,
    open_folder_on_complete: bool,
) {
    let active = settings.speed_limit_enabled && settings.speed_limit_kib > 0;
    item.set_speed_limit_enabled(active);
    item.set_speed_limit_kib(settings.speed_limit_kib.min(i32::MAX as u64) as i32);
    let effective_open_folder =
        open_folder_on_complete && matches!(settings.completion_power_action.trim(), "" | "none");
    item.set_completion_action_index(completion_action_index_for(
        &settings.completion_power_action,
        effective_open_folder,
    ));
}

fn snapshot_folder(snapshot: &TaskSnapshot, fallback_dir: &str) -> Option<String> {
    let dir = snapshot.download_dir.trim();
    if !dir.is_empty() {
        return Some(dir.to_owned());
    }
    let output = snapshot.output_path.trim();
    if !output.is_empty() {
        if let Some(parent) = std::path::Path::new(output)
            .parent()
            .and_then(|path| path.to_str())
            .filter(|path| !path.trim().is_empty())
        {
            return Some(parent.to_owned());
        }
    }
    let fallback = fallback_dir.trim();
    if !fallback.is_empty() {
        return Some(fallback.to_owned());
    }
    None
}

fn build_progress_speed_data(task: &TaskSnapshot) -> (Vec<f32>, String) {
    let speeds = crate::speed_stats::summarize(task.speed_bytes_per_sec, &task.speed_history);
    let stats_text = if speeds.peak > 0 {
        format!(
            "峰值 {} · 平均 {}",
            format_speed(speeds.peak),
            format_speed(speeds.average)
        )
    } else {
        String::new()
    };
    let mut raw: Vec<u64> = task
        .speed_history
        .iter()
        .rev()
        .take(PROGRESS_SPEED_HISTORY_LIMIT)
        .copied()
        .collect();
    raw.reverse();
    if raw.is_empty() && task.speed_bytes_per_sec > 0 {
        raw.push(task.speed_bytes_per_sec);
    }
    if raw.is_empty() {
        return (Vec::new(), stats_text);
    }
    let maximum = raw.iter().copied().max().unwrap_or(1).max(1) as f32;
    let normalized = raw
        .into_iter()
        .map(|sample| (sample as f32 / maximum).clamp(0.0, 1.0))
        .collect();
    (normalized, stats_text)
}

fn build_connection_rows(task: &TaskSnapshot) -> Vec<ConnectionRow> {
    if task.connection_parts.is_empty() {
        return Vec::new();
    }
    let mut indices: Vec<usize> = if task.connection_parts.len() <= PROGRESS_CONNECTION_ROWS_LIMIT {
        (0..task.connection_parts.len()).collect()
    } else {
        let mut selected: Vec<usize> = task
            .connection_parts
            .iter()
            .enumerate()
            .filter(|(_, part)| {
                matches!(
                    part.state.as_str(),
                    "active" | "downloading" | "retrying" | "retry"
                )
            })
            .map(|(index, _)| index)
            .take(PROGRESS_CONNECTION_ROWS_LIMIT)
            .collect();
        if selected.len() < PROGRESS_CONNECTION_ROWS_LIMIT {
            for index in (0..task.connection_parts.len()).rev() {
                if selected.len() >= PROGRESS_CONNECTION_ROWS_LIMIT {
                    break;
                }
                if !selected.contains(&index) {
                    selected.push(index);
                }
            }
        }
        selected.sort_unstable();
        selected
    };
    indices.truncate(PROGRESS_CONNECTION_ROWS_LIMIT);
    indices
        .into_iter()
        .map(|index| {
            let part = &task.connection_parts[index];
            let span = part.end.saturating_sub(part.start).saturating_add(1).max(1);
            let (state, state_label) = match part.state.as_str() {
                "active" | "downloading" => ("active", "下载中"),
                "done" | "completed" => ("done", "已完成"),
                "retrying" | "retry" => ("retrying", "重试中"),
                _ => ("queued", "等待"),
            };
            let done = if state == "done" {
                span
            } else {
                part.done.min(span)
            };
            let progress = (done as f32 / span as f32).clamp(0.0, 1.0);
            ConnectionRow {
                index: format!("#{}", index + 1).into(),
                progress,
                transferred: format!("{} / {}", format_bytes(done), format_bytes(span)).into(),
                state: state.into(),
                state_label: state_label.into(),
            }
        })
        .collect()
}

fn format_hud_connection_summary(task: &TaskSnapshot) -> String {
    if task.connection_parts.is_empty() {
        return if task.active_workers > 0 {
            format!("{} 个活动连接", task.active_workers)
        } else if task.status == "paused" {
            "连接已暂停".into()
        } else {
            "单连接传输".into()
        };
    }
    let done = task
        .connection_parts
        .iter()
        .filter(|part| matches!(part.state.as_str(), "done" | "completed"))
        .count();
    let active_parts = task
        .connection_parts
        .iter()
        .filter(|part| matches!(part.state.as_str(), "active" | "downloading"))
        .count() as u32;
    let active = task.active_workers.max(active_parts);
    format!(
        "{active} 活动 · {done}/{} 分段",
        task.connection_parts.len()
    )
}

fn populate_progress_fixture(window: &ProgressWindow, expanded: bool) {
    let mb = 1024 * 1024;
    let sample_task = TaskSnapshot {
        task_id: "fixture-progress-1".into(),
        filename: "示例视频 · 1080p.mp4".into(),
        status: "downloading".into(),
        downloaded_bytes: 82 * mb,
        total_bytes: Some(128 * mb),
        speed_bytes_per_sec: (18.6 * mb as f64) as u64,
        active_workers: 3,
        completed_ranges: 64,
        total_ranges: 100,
        download_dir: r"D:\下载\媒体".into(),
        speed_history: vec![
            6 * mb,
            9 * mb,
            12 * mb,
            15 * mb,
            14 * mb,
            17 * mb,
            20 * mb,
            22 * mb,
            19 * mb,
            16 * mb,
            18 * mb,
            21 * mb,
            23 * mb,
            20 * mb,
            18 * mb,
            17 * mb,
            19 * mb,
            21 * mb,
            18 * mb,
            19 * mb,
        ],
        connection_parts: vec![
            hls_client::ConnectionPart {
                start: 0,
                end: 24 * mb - 1,
                done: 24 * mb,
                state: "done".into(),
            },
            hls_client::ConnectionPart {
                start: 24 * mb,
                end: 48 * mb - 1,
                done: 18 * mb,
                state: "active".into(),
            },
            hls_client::ConnectionPart {
                start: 48 * mb,
                end: 72 * mb - 1,
                done: 24 * mb,
                state: "done".into(),
            },
            hls_client::ConnectionPart {
                start: 72 * mb,
                end: 96 * mb - 1,
                done: 11 * mb,
                state: "active".into(),
            },
            hls_client::ConnectionPart {
                start: 96 * mb,
                end: 112 * mb - 1,
                done: 5 * mb,
                state: "retrying".into(),
            },
            hls_client::ConnectionPart {
                start: 112 * mb,
                end: 128 * mb - 1,
                done: 0,
                state: "queued".into(),
            },
        ],
        ..Default::default()
    };
    window.set_headline(PROGRESS_HEADLINE.into());
    window.set_filename(sample_task.filename.clone().into());
    window.set_speed("18.6 MB/s".into());
    window.set_progress(0.64);
    window.set_progress_known(true);
    window.set_paused(false);
    window.set_processing(false);
    window.set_segments(slint::ModelRc::new(slint::VecModel::from(
        build_segment_blocks(&sample_task),
    )));
    window.set_transferred("81.9 MB / 128.0 MB · 分片 64 / 100".into());
    window.set_stage_label("正在下载".into());
    window.set_remaining("剩余约 3 秒".into());
    let (speed_history, speed_stats) = build_progress_speed_data(&sample_task);
    window.set_speed_history(slint::ModelRc::new(slint::VecModel::from(speed_history)));
    window.set_speed_stats(speed_stats.into());
    window.set_connection_rows(slint::ModelRc::new(slint::VecModel::from(
        build_connection_rows(&sample_task),
    )));
    window.set_connection_summary(format_hud_connection_summary(&sample_task).into());
    window.set_task_index(1);
    window.set_task_count(3);
    window.set_speed_limit_enabled(false);
    window.set_speed_limit_kib(0);
    window.set_completion_action_index(0);
    window.set_can_open_folder(true);
    window.set_expanded(expanded);
}

/// Ordered view of the tasks currently shown by the progress HUD. Entries are
/// kept sorted by (activation, task_id): the default selection is the task
/// activated earliest, with simultaneous activations broken by lowest task_id.
/// Users can also cycle between tracked tasks in-place and pin a HUD-paused
/// task so Pause/Resume works directly without hiding the HUD.
#[derive(Default)]
struct ActiveTaskHud {
    entries: Vec<ActiveTaskEntry>,
    next_activation: u64,
    selected_task_id: Option<String>,
    pinned_paused: HashSet<String>,
    processing_started_at: HashMap<String, std::time::Instant>,
}

struct ActiveTaskEntry {
    task_id: String,
    activation: u64,
    snapshot: TaskSnapshot,
}

impl ActiveTaskHud {
    fn sync_selection(&mut self) {
        if self.entries.is_empty() {
            self.selected_task_id = None;
            return;
        }
        let still_present = self
            .selected_task_id
            .as_deref()
            .is_some_and(|selected| self.entries.iter().any(|entry| entry.task_id == selected));
        if !still_present {
            self.selected_task_id = self.entries.first().map(|entry| entry.task_id.clone());
        }
    }

    fn current_snapshot(&self) -> Option<TaskSnapshot> {
        if let Some(selected) = self.selected_task_id.as_deref() {
            if let Some(entry) = self.entries.iter().find(|entry| entry.task_id == selected) {
                return Some(entry.snapshot.clone());
            }
        }
        self.entries.first().map(|entry| entry.snapshot.clone())
    }

    /// Pin a tracked task so pausing it from the HUD keeps it visible in place.
    fn mark_pause_requested(&mut self, task_id: &str) {
        if self.entries.iter().any(|entry| entry.task_id == task_id) {
            self.pinned_paused.insert(task_id.to_owned());
        }
    }

    /// Remove a pause pin (e.g. if the pause command failed).
    fn clear_pause_pin(&mut self, task_id: &str) {
        self.pinned_paused.remove(task_id);
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.task_id == task_id && !task_is_active(&entry.snapshot.status))
        {
            self.entries.remove(index);
            self.sync_selection();
        }
    }

    /// Drop any HUD-pinned paused tasks (e.g. when the user explicitly closes the HUD).
    fn dismiss_paused(&mut self) -> Option<TaskSnapshot> {
        self.pinned_paused.clear();
        self.entries
            .retain(|entry| task_is_active(&entry.snapshot.status));
        self.sync_selection();
        self.current_snapshot()
    }

    fn sync_processing_clock(&mut self, task_id: &str, status: &str, now: std::time::Instant) {
        if is_processing_status(status) {
            self.processing_started_at
                .entry(task_id.to_owned())
                .or_insert(now);
        } else {
            self.processing_started_at.remove(task_id);
        }
    }

    fn processing_elapsed_secs(&self, task_id: &str, now: std::time::Instant) -> Option<u64> {
        self.processing_started_at
            .get(task_id)
            .map(|started| now.saturating_duration_since(*started).as_secs())
    }

    /// Fold a task snapshot into the view and return the snapshot the HUD
    /// should display afterwards (None when no task is active or pinned anymore).
    #[cfg(test)]
    fn observe(&mut self, snapshot: TaskSnapshot) -> Option<TaskSnapshot> {
        self.observe_at(snapshot, std::time::Instant::now())
    }

    fn observe_at(
        &mut self,
        snapshot: TaskSnapshot,
        now: std::time::Instant,
    ) -> Option<TaskSnapshot> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.task_id == snapshot.task_id);
        if task_is_active(&snapshot.status) {
            self.pinned_paused.remove(&snapshot.task_id);
            self.sync_processing_clock(&snapshot.task_id, &snapshot.status, now);
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
        } else if snapshot.status == "paused" && self.pinned_paused.contains(&snapshot.task_id) {
            self.processing_started_at.remove(&snapshot.task_id);
            if let Some(index) = index {
                self.entries[index].snapshot = snapshot;
            } else {
                self.pinned_paused.remove(&snapshot.task_id);
            }
        } else {
            self.pinned_paused.remove(&snapshot.task_id);
            self.processing_started_at.remove(&snapshot.task_id);
            if let Some(index) = index {
                self.entries.remove(index);
            }
        }
        self.sync_selection();
        self.current_snapshot()
    }

    /// Drop a task deleted while tracked and return the snapshot the HUD should
    /// display afterwards.
    fn remove(&mut self, task_id: &str) -> Option<TaskSnapshot> {
        self.pinned_paused.remove(task_id);
        self.processing_started_at.remove(task_id);
        self.entries.retain(|entry| entry.task_id != task_id);
        self.sync_selection();
        self.current_snapshot()
    }

    /// Reconcile HUD entries against a fresh full snapshot after reconnect/resync.
    fn reconcile(&mut self, tasks: &[TaskSnapshot]) -> Option<TaskSnapshot> {
        let now = std::time::Instant::now();
        let by_id: HashMap<&str, &TaskSnapshot> = tasks
            .iter()
            .map(|task| (task.task_id.as_str(), task))
            .collect();
        self.entries.retain(|entry| {
            let Some(snapshot) = by_id.get(entry.task_id.as_str()) else {
                return false;
            };
            task_is_active(&snapshot.status)
                || (snapshot.status == "paused" && self.pinned_paused.contains(&entry.task_id))
        });
        for entry in &mut self.entries {
            if let Some(snapshot) = by_id.get(entry.task_id.as_str()) {
                entry.snapshot = (*snapshot).clone();
                if task_is_active(&snapshot.status) {
                    self.pinned_paused.remove(&entry.task_id);
                }
            }
        }
        self.pinned_paused
            .retain(|id| self.entries.iter().any(|entry| &entry.task_id == id));
        for snapshot in tasks.iter().filter(|task| task_is_active(&task.status)) {
            if !self
                .entries
                .iter()
                .any(|entry| entry.task_id == snapshot.task_id)
            {
                self.observe_at(snapshot.clone(), now);
            }
        }
        self.processing_started_at.retain(|id, _| {
            self.entries
                .iter()
                .any(|entry| &entry.task_id == id && is_processing_status(&entry.snapshot.status))
        });
        for entry in &self.entries {
            if is_processing_status(&entry.snapshot.status) {
                self.processing_started_at
                    .entry(entry.task_id.clone())
                    .or_insert(now);
            }
        }
        self.sync_selection();
        self.current_snapshot()
    }

    /// Switch the displayed HUD task forward/backward with wrap-around.
    fn cycle(&mut self, delta: i32) -> Option<TaskSnapshot> {
        if self.entries.is_empty() {
            self.selected_task_id = None;
            return None;
        }
        let len = self.entries.len();
        let current = self
            .selected_task_id
            .as_deref()
            .and_then(|selected| {
                self.entries
                    .iter()
                    .position(|entry| entry.task_id == selected)
            })
            .unwrap_or(0);
        let next = (current as isize + delta as isize).rem_euclid(len as isize) as usize;
        self.selected_task_id = Some(self.entries[next].task_id.clone());
        Some(self.entries[next].snapshot.clone())
    }

    /// 1-based `(current, total)` position for the multi-task switcher badge.
    fn task_position(&self) -> (usize, usize) {
        if self.entries.is_empty() {
            return (0, 0);
        }
        let current = self
            .selected_task_id
            .as_deref()
            .and_then(|selected| {
                self.entries
                    .iter()
                    .position(|entry| entry.task_id == selected)
            })
            .unwrap_or(0);
        (current + 1, self.entries.len())
    }

    fn primary_task_id(&self) -> Option<String> {
        self.current_snapshot().map(|snapshot| snapshot.task_id)
    }
}

const COMPLETE_AUTO_DISMISS_DURATION: Duration = Duration::from_secs(10);

pub(crate) fn should_hold_complete_toast(
    action_busy: bool,
    power_seconds: i32,
    power_hint: &str,
    cursor_inside: bool,
) -> bool {
    action_busy || power_seconds > 0 || !power_hint.is_empty() || cursor_inside
}

fn present_complete_toast(item: &CompleteWindow) {
    if item.get_power_seconds() <= 0 && native_window::is_foreground_fullscreen_or_busy() {
        trace("suppressing complete toast while foreground fullscreen/busy");
        return;
    }
    let reopening = !item.window().is_visible();
    if reopening {
        item.set_entered(false);
        native_window::set_cloaked(item.window(), true);
    }
    if item.show().is_ok() {
        if reopening {
            play_complete_entrance(item);
        }
        round_native_window(item.window());
        native_window::prepare_bottom_right_toast(item, 16.0);
    } else {
        native_window::set_cloaked(item.window(), false);
    }
}

fn apply_completed_snapshot_to_toast(item: &CompleteWindow, snapshot: &TaskSnapshot) {
    item.set_details(format!("{} · 已保存到本地", format_bytes(snapshot.downloaded_bytes)).into());
    item.set_filename(snapshot.filename.clone().into());
    if item.get_power_seconds() <= 0 && item.get_power_hint().is_empty() {
        item.set_power_hint("".into());
        item.set_power_action_label("".into());
        item.set_power_seconds(0);
    }
}

#[allow(clippy::too_many_arguments)]
fn update_task_windows(
    snapshot: TaskSnapshot,
    settings: PresenterRuntimeSettings,
    open_folder_on_complete: bool,
    fallback_dir: &str,
    progress: &slint::Weak<ProgressWindow>,
    complete: &slint::Weak<CompleteWindow>,
    active_hud: &Mutex<ActiveTaskHud>,
    rendered_hud_task: &Mutex<Option<String>>,
    completed_task: &RefCell<Option<String>>,
    completed_notified: &RefCell<HashSet<String>>,
    complete_shown_at: Option<&std::cell::Cell<Option<std::time::Instant>>>,
    progress_user_moved: Option<&Arc<std::sync::atomic::AtomicBool>>,
) {
    if task_is_active(&snapshot.status) {
        completed_notified.borrow_mut().remove(&snapshot.task_id);
    }
    let (primary, position, processing_elapsed) = active_hud
        .lock()
        .map(|mut hud| {
            let now = std::time::Instant::now();
            let primary = hud.observe_at(snapshot.clone(), now);
            let pos = hud.task_position();
            let elapsed = primary
                .as_ref()
                .and_then(|task| hud.processing_elapsed_secs(&task.task_id, now));
            (primary, pos, elapsed)
        })
        .unwrap_or((None, (0, 0), None));
    match primary {
        Some(primary) => {
            reset_progress_action_for_new_task(progress, rendered_hud_task, &primary.task_id);
            render_progress_hud(
                progress,
                settings,
                &primary,
                position,
                fallback_dir,
                processing_elapsed,
                progress_user_moved,
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
    if matches!(snapshot.status.as_str(), "completed" | "done")
        && completed_notified
            .borrow_mut()
            .insert(snapshot.task_id.clone())
    {
        *completed_task.borrow_mut() = Some(snapshot.task_id.clone());
        if open_folder_on_complete {
            let folder = snapshot_folder(&snapshot, fallback_dir);
            if let Some(dir) = folder {
                let _ = file_dialogs::open(&dir);
            } else {
                let completed_id = snapshot.task_id.clone();
                thread::spawn(move || {
                    let _ = CoreIpcClient::connect().and_then(|mut client| {
                        client.command(CoreCommand::OpenCompleted {
                            task_id: completed_id,
                            folder: true,
                        })
                    });
                });
            }
        }
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
                apply_completed_snapshot_to_toast(&item, &snapshot);
                if let Some(shown_at) = complete_shown_at {
                    shown_at.set(Some(std::time::Instant::now()));
                }
                present_complete_toast(&item);
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
    position: (usize, usize),
    fallback_dir: &str,
    processing_elapsed: Option<u64>,
    user_moved: Option<&Arc<std::sync::atomic::AtomicBool>>,
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
    let paused = snapshot.status == "paused";
    let processing = is_processing_status(&snapshot.status);
    item.set_headline(if processing {
        LOCAL_PROCESSING.into()
    } else {
        PROGRESS_HEADLINE.into()
    });
    item.set_filename(snapshot.filename.clone().into());
    item.set_paused(paused);
    item.set_processing(processing);
    item.set_processing_badge(processing_badge_label(&snapshot.status).into());
    if (paused && item.get_action_text().as_str() == "正在暂停…")
        || (!paused && item.get_action_text().as_str() == "正在继续…")
    {
        item.set_action_busy(false);
        item.set_action_text("".into());
    }
    item.set_speed(if processing {
        format_processing_headline(&snapshot.status, processing_elapsed).into()
    } else if paused {
        "已暂停".into()
    } else {
        format_speed(snapshot.speed_bytes_per_sec).into()
    });
    item.set_remaining(if processing {
        "传输已完成，文件尚未就绪".into()
    } else if paused {
        "点击继续恢复下载".into()
    } else if snapshot.speed_bytes_per_sec > 0 {
        snapshot
            .total_bytes
            .filter(|total| *total > snapshot.downloaded_bytes)
            .map(|total| {
                let seconds =
                    (total - snapshot.downloaded_bytes).div_ceil(snapshot.speed_bytes_per_sec);
                if seconds >= 60 {
                    format!("剩余约 {} 分 {} 秒", seconds / 60, seconds % 60)
                } else {
                    format!("剩余约 {seconds} 秒")
                }
            })
            .unwrap_or_default()
            .into()
    } else {
        "".into()
    });
    item.set_stage_label(
        match snapshot.status.as_str() {
            "merging" | "remuxing" => "正在合并音视频",
            "checking" => "正在校验文件",
            "processing" | "finalizing" => "正在本地处理",
            "recording" => "正在录制",
            "paused" => "已暂停",
            _ => "正在下载",
        }
        .into(),
    );
    item.set_progress(task_progress(snapshot));
    item.set_segments(slint::ModelRc::new(slint::VecModel::from(
        build_segment_blocks(snapshot),
    )));
    item.set_progress_known(
        !processing
            && (snapshot.total_bytes.is_some_and(|total| total > 0)
                || (!snapshot.is_live
                    && matches!(
                        snapshot.resource_kind,
                        ResourceKind::Hls | ResourceKind::Dash
                    )
                    && snapshot.total_ranges > 0)),
    );
    item.set_transferred(match snapshot.total_bytes.filter(|total| *total > 0) {
        Some(total) => format!(
            "已下载 {} / {}",
            format_bytes(snapshot.downloaded_bytes),
            format_bytes(total)
        )
        .into(),
        None if !snapshot.is_live && snapshot.total_ranges > 0 => format!(
            "{} · 分片 {} / {}",
            format_bytes(snapshot.downloaded_bytes),
            snapshot.completed_ranges,
            snapshot.total_ranges
        )
        .into(),
        None => format!("已下载 {}", format_bytes(snapshot.downloaded_bytes)).into(),
    });
    item.set_task_index(position.0.max(1) as i32);
    item.set_task_count(position.1 as i32);
    let (speed_history, speed_stats) = build_progress_speed_data(snapshot);
    item.set_speed_history(slint::ModelRc::new(slint::VecModel::from(speed_history)));
    item.set_speed_stats(speed_stats.into());
    item.set_connection_rows(slint::ModelRc::new(slint::VecModel::from(
        build_connection_rows(snapshot),
    )));
    item.set_connection_summary(format_hud_connection_summary(snapshot).into());
    item.set_can_open_folder(snapshot_folder(snapshot, fallback_dir).is_some());
    let first_show = !item.window().is_visible();
    if first_show {
        if settings.workbench_visible {
            trace("suppressing floating progress HUD while main workbench is visible");
            return;
        }
        if native_window::is_foreground_fullscreen_or_busy() {
            trace("suppressing background progress HUD while foreground fullscreen/busy");
            return;
        }
        item.invoke_enter();
    }
    if first_show && native_window::show_non_activating(&item, false, false).is_ok() {
        round_native_window(item.window());
        let moved = user_moved.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed));
        if !moved {
            native_window::position_bottom_right_hud(item.window(), 16.0, 36.0);
            #[cfg(windows)]
            {
                let weak = item.as_weak();
                let moved_flag = user_moved.cloned();
                let _ = slint::spawn_local(async move {
                    let Some(item) = weak.upgrade() else {
                        return;
                    };
                    use slint::winit_030::WinitWindowAccessor;
                    if item.window().winit_window().await.is_ok()
                        && item.window().is_visible()
                        && !moved_flag
                            .as_ref()
                            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
                    {
                        native_window::position_bottom_right_hud(item.window(), 16.0, 36.0);
                    }
                });
            }
        } else {
            native_window::fit_to_work_area(item.window());
        }
    }
    // show/resize 由 round_native_window 清缓存；普通进度更新保留局部重绘。
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

fn format_bitrate(bits_per_sec: u64) -> String {
    if bits_per_sec >= 1_000_000 {
        format!("{:.1} Mbps", bits_per_sec as f64 / 1_000_000.0)
    } else if bits_per_sec >= 1_000 {
        format!("{} kbps", bits_per_sec / 1_000)
    } else {
        format!("{bits_per_sec} bps")
    }
}

fn concise_variant_hint(variant_id: &str) -> Option<String> {
    let trimmed = variant_id.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") {
        let clean = safe_display_url(trimmed);
        let leaf = clean
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or(&clean);
        return Some(leaf.chars().take(40).collect());
    }
    Some(trimmed.chars().take(40).collect())
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
    let mut parts = vec![
        resource_kind_label(offer.resource_kind).to_string(),
        extension,
        size,
    ];
    if offer.preferred_height > 0 {
        parts.push(format!("{}p", offer.preferred_height));
    }
    if offer.preferred_bandwidth > 0 {
        parts.push(format_bitrate(offer.preferred_bandwidth));
    }
    if offer.preferred_height == 0 && offer.preferred_bandwidth == 0 {
        if let Some(hint) = concise_variant_hint(&offer.preferred_variant_id) {
            parts.push(format!("轨道 {hint}"));
        }
    }
    parts.join(" · ")
}

fn format_request_details(offer: &ResourceOffer) -> String {
    let method = offer.request_method.trim().to_ascii_uppercase();
    let mut parts = vec![format!(
        "请求方式：{}",
        if method.is_empty() { "GET" } else { &method }
    )];
    let mime = offer.mime_type.trim();
    if !mime.is_empty() {
        parts.push(format!("内容类型：{mime}"));
    }
    if offer.preferred_height > 0 {
        parts.push(format!("清晰度：{}p", offer.preferred_height));
    }
    if offer.preferred_bandwidth > 0 {
        parts.push(format!(
            "码率：{}",
            format_bitrate(offer.preferred_bandwidth)
        ));
    }
    if let Some(hint) = concise_variant_hint(&offer.preferred_variant_id) {
        parts.push(format!("轨道：{hint}"));
    }
    parts.join(" · ")
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
        "recording" => "录制中",
        "merging" => "合并中",
        "remuxing" => "封装中",
        "processing" => "处理中",
        "finalizing" => "收尾中",
        "checking" => "校验中",
        "parsing" => "解析中",
        "paused" => "已暂停",
        "canceled" | "cancelled" => "已取消",
        "completed" | "done" => "已完成",
        "failed" | "error" => "失败",
        "queued" => "排队中",
        _ => status,
    }
}

fn task_progress(task: &TaskSnapshot) -> f32 {
    if matches!(task.status.as_str(), "completed" | "done") {
        return 1.0;
    }
    if !task.is_live
        && matches!(task.resource_kind, ResourceKind::Hls | ResourceKind::Dash)
        && task.total_ranges > 0
    {
        return (task.completed_ranges as f32 / task.total_ranges as f32).clamp(0.0, 1.0);
    }
    task.total_bytes
        .filter(|total| *total > 0)
        .map(|total| task.downloaded_bytes as f32 / total as f32)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0)
}

pub(crate) fn build_segment_blocks(task: &TaskSnapshot) -> Vec<SegmentBlock> {
    if !task.connection_parts.is_empty() {
        let total = task
            .total_bytes
            .unwrap_or_else(|| {
                task.connection_parts
                    .last()
                    .map(|part| part.end.saturating_add(1))
                    .unwrap_or(0)
            })
            .max(1) as f32;
        return task
            .connection_parts
            .iter()
            .map(|part| {
                let span = part.end.saturating_sub(part.start).saturating_add(1).max(1);
                let width_ratio = span as f32 / total;
                let fill_ratio = match part.state.as_str() {
                    "done" => 1.0,
                    "active" => part.done.min(span) as f32 / span as f32,
                    _ => 0.0,
                };
                SegmentBlock {
                    width_ratio,
                    fill_ratio,
                    state: part.state.clone().into(),
                }
            })
            .collect();
    }
    if !task.is_live && task.total_ranges > 1 {
        let buckets = task.total_ranges.min(32);
        let done_buckets = ((task.completed_ranges.min(task.total_ranges) * buckets)
            / task.total_ranges)
            .min(buckets);
        let active_buckets =
            u64::from(task.active_workers).min(buckets.saturating_sub(done_buckets));
        return (0..buckets)
            .map(|index| {
                let (state, fill_ratio) = if index < done_buckets {
                    ("done", 1.0)
                } else if index < done_buckets + active_buckets {
                    ("active", 0.0)
                } else {
                    ("queued", 0.0)
                };
                SegmentBlock {
                    width_ratio: 1.0 / buckets as f32,
                    fill_ratio,
                    state: state.into(),
                }
            })
            .collect();
    }
    Vec::new()
}

pub(crate) fn format_connection_summary(task: &TaskSnapshot) -> String {
    let hint = task.connection_hint.trim();
    let header = if hint.is_empty() {
        format!(
            "{} 个活动连接 · 已完成 {}/{} 分段",
            task.active_workers, task.completed_ranges, task.total_ranges
        )
    } else {
        format!(
            "{} 个活动连接 · 已完成 {}/{} 分段 · {hint}",
            task.active_workers, task.completed_ranges, task.total_ranges
        )
    };
    if task.connection_parts.is_empty() {
        return header;
    }
    let max_shown = 16usize;
    let mut lines = Vec::with_capacity(task.connection_parts.len().min(max_shown) + 2);
    lines.push(header);
    for (index, part) in task.connection_parts.iter().enumerate().take(max_shown) {
        let state_label = match part.state.as_str() {
            "done" => "已完成",
            "active" => "传输中",
            "queued" => "待分配",
            other => other,
        };
        lines.push(format!(
            "分段 #{:02}  {} – {}（已落盘 {}）· {}",
            index + 1,
            format_bytes(part.start),
            format_bytes(part.end),
            format_bytes(part.done),
            state_label
        ));
    }
    if task.connection_parts.len() > max_shown {
        lines.push(format!(
            "… 其余 {} 个分段已在上方动态分段条中汇总显示",
            task.connection_parts.len() - max_shown
        ));
    }
    lines.join("\n")
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
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    // SAFETY: `AttachConsole(ATTACH_PARENT_PROCESS)` attaches stdio handles to the parent console
    // when invoked from a terminal (`--self-test`) and fails harmlessly with 0 when none exists.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

fn confirmed_filename(input: &str) -> Result<&str, target_check::TargetIssue> {
    // Show the existing repair action for trailing spaces and control characters.
    target_check::check_name(input)?;
    let name = input.trim();
    target_check::check_name(name)?;
    Ok(name)
}

fn set_confirm_issue(item: &ConfirmWindow, text: &str, actions: [(&str, &str); 2]) {
    item.set_error_text(text.into());
    item.set_error_primary_label(actions[0].0.into());
    item.set_error_primary_command(actions[0].1.into());
    item.set_error_secondary_label(actions[1].0.into());
    item.set_error_secondary_command(actions[1].1.into());
}

fn clear_confirm_issue(item: &ConfirmWindow) {
    set_confirm_issue(item, "", [("", ""), ("", "")]);
}

#[cfg(test)]
mod tests {
    use super::{
        update_presenter_settings, PresenterRuntimeSettings, PresenterSettings, SettingsReload,
    };
    use hls_client::CorePipeResponse;

    #[test]
    fn confirm_validates_the_typed_filename_before_normalization() {
        use super::{confirmed_filename, target_check::TargetIssue};
        assert_eq!(confirmed_filename(" clip.mp4"), Ok("clip.mp4"));
        assert_eq!(
            confirmed_filename("clip.mp4 "),
            Err(TargetIssue::TrailingDot)
        );
        assert_eq!(
            confirmed_filename("clip.mp4\t"),
            Err(TargetIssue::InvalidName('\t'))
        );
        assert_eq!(confirmed_filename(" CON"), Err(TargetIssue::ReservedName));
    }

    fn settings_response(directory: &str) -> CorePipeResponse {
        serde_json::from_value(serde_json::json!({
            "type": "settings", "request_id": 1,
            "takeover_enabled": true, "takeover_minimum_bytes": 0,
            "legal_accepted": true, "speed_limit_kib": 0,
            "download_dir": directory, "theme_mode": "dark",
            "progress_window_enabled": false, "complete_popup_enabled": false,
            "completion_sound_enabled": true, "reduce_motion": true
        }))
        .unwrap()
    }

    #[test]
    fn settings_reload_reconnects_after_disconnect_and_delivers_consecutive_changes() {
        let now = std::time::Instant::now();
        let mut reload = SettingsReload {
            client: Some(false),
            last_request: now,
        };
        let mut connections = 0;
        for directory in ["first", "second"] {
            let response = reload
                .reload(
                    now,
                    || {
                        connections += 1;
                        Ok(true)
                    },
                    |connected| {
                        if *connected {
                            Ok(settings_response(directory))
                        } else {
                            Err("disconnected".into())
                        }
                    },
                )
                .unwrap();
            assert_eq!(
                PresenterSettings::from_response(Some(&response)).download_dir,
                directory
            );
        }
        assert_eq!(connections, 1);
        let response = reload
            .reload(
                now + std::time::Duration::from_secs(121),
                || {
                    connections += 1;
                    Ok(true)
                },
                |_| Ok(settings_response("after idle")),
            )
            .unwrap();
        assert_eq!(connections, 2);
        assert_eq!(
            PresenterSettings::from_response(Some(&response)).download_dir,
            "after idle"
        );
    }

    #[test]
    fn non_settings_responses_preserve_presenter_and_runtime_settings() {
        let initial = settings_response("kept");
        let mut presenter = PresenterSettings::from_response(Some(&initial));
        let mut runtime = PresenterRuntimeSettings::from_response(Some(&initial));
        let original = (presenter.clone(), runtime);
        let mut reload = SettingsReload {
            client: Some(()),
            last_request: std::time::Instant::now(),
        };
        for response in [
            CorePipeResponse::Error {
                request_id: Some(1),
                code: "unavailable".into(),
                message: "try later".into(),
            },
            CorePipeResponse::Events {
                request_id: 1,
                events: vec![],
            },
        ] {
            assert!(reload
                .reload(
                    std::time::Instant::now(),
                    || Ok(()),
                    |_| Ok(response.clone())
                )
                .is_none());
            assert!(update_presenter_settings(&response, &mut presenter, &mut runtime).is_none());
            assert_eq!((&presenter, runtime), (&original.0, original.1));
        }
        assert!(update_presenter_settings(
            &settings_response("updated"),
            &mut presenter,
            &mut runtime
        )
        .is_some());
        assert_eq!(presenter.download_dir, "updated");
    }

    use super::{
        build_segment_blocks, confirm_window_logical_size, download_category, file_extension,
        format_connection_summary, format_request_details, format_resource_meta, safe_display_url,
        task_status_label, ActiveTaskHud,
    };
    use hls_client::{ConnectionPart, ResourceKind, ResourceOffer, TaskSnapshot};

    #[test]
    fn task_status_labels_cover_core_states_and_preserve_unknown_values() {
        for (status, label) in [
            ("queued", "排队中"),
            ("downloading", "下载中"),
            ("recording", "录制中"),
            ("merging", "合并中"),
            ("checking", "校验中"),
            ("parsing", "解析中"),
            ("paused", "已暂停"),
            ("canceled", "已取消"),
            ("cancelled", "已取消"),
            ("completed", "已完成"),
            ("done", "已完成"),
            ("failed", "失败"),
            ("error", "失败"),
            ("", ""),
            ("future_status", "future_status"),
        ] {
            assert_eq!(task_status_label(status), label, "{status}");
        }
    }

    #[test]
    fn confirm_window_size_matches_work_area_height_at_each_dpi() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            for (logical_height, expected_height) in [
                (240.0_f32, 240.0),
                (320.0, 320.0),
                (360.0, 360.0),
                (400.0, 400.0),
                (468.0, 468.0),
                (816.0, 468.0),
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
            for (width, height) in [(520.0, 468.0), (960.0, 816.0), (1920.0, 1080.0)] {
                assert_eq!(
                    confirm_window_logical_size(
                        ((width * scale) as u32, (height * scale) as u32),
                        scale,
                    ),
                    Some((520.0, 468.0))
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
                Some((480.0, 468.0)),
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
            ((1600, 852), 1.5, (520.0, 468.0)),
            ((650, 450), 1.25, (520.0, 360.0)),
            ((1920, 1020), 1.25, (520.0, 468.0)),
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
    fn progress_hud_reconcile_drops_stale_tasks_missing_from_resync_snapshot() {
        let mut hud = ActiveTaskHud::default();
        hud.observe(hud_snapshot("stale-1", "downloading"));
        hud.observe(hud_snapshot("live-2", "recording"));
        assert_eq!(hud.primary_task_id().as_deref(), Some("stale-1"));

        let next = hud.reconcile(&[
            hud_snapshot("stale-1", "completed"),
            hud_snapshot("live-2", "recording"),
        ]);
        assert_eq!(next.as_ref().map(|t| t.task_id.as_str()), Some("live-2"));
        assert_eq!(hud.primary_task_id().as_deref(), Some("live-2"));

        assert!(hud.reconcile(&[]).is_none());
        assert_eq!(hud.primary_task_id(), None);
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
            download_category("movie.m3u8", "", offer.resource_kind),
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
            download_category("setup.exe", "", ResourceKind::File),
            "program"
        );
        assert_eq!(
            download_category("bundle.7z", "", ResourceKind::File),
            "archive"
        );
    }

    #[test]
    fn segment_blocks_and_connection_summary_format_clean_chinese_telemetry() {
        let task = TaskSnapshot {
            active_workers: 2,
            completed_ranges: 1,
            total_ranges: 3,
            total_bytes: Some(1024 * 1024),
            connection_hint: "动态分段传输中".into(),
            connection_parts: vec![
                ConnectionPart {
                    start: 0,
                    end: 512 * 1024 - 1,
                    done: 512 * 1024,
                    state: "done".into(),
                },
                ConnectionPart {
                    start: 512 * 1024,
                    end: 768 * 1024 - 1,
                    done: 128 * 1024,
                    state: "active".into(),
                },
                ConnectionPart {
                    start: 768 * 1024,
                    end: 1024 * 1024 - 1,
                    done: 0,
                    state: "queued".into(),
                },
            ],
            ..Default::default()
        };
        let blocks = build_segment_blocks(&task);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].state.as_str(), "done");
        assert_eq!(blocks[1].state.as_str(), "active");
        assert_eq!(blocks[2].state.as_str(), "queued");
        assert_eq!(blocks[0].width_ratio, 0.5);
        assert_eq!(blocks[1].fill_ratio, 0.5);
        let mut starting = task.clone();
        starting.connection_parts[1].done = 1;
        let starting_blocks = build_segment_blocks(&starting);
        assert_eq!(starting_blocks[1].fill_ratio, 1.0 / (256 * 1024) as f32);

        let summary = format_connection_summary(&task);
        assert!(summary.contains("2 个活动连接 · 已完成 1/3 分段 · 动态分段传输中"));
        assert!(summary.contains("分段 #01"));
        assert!(summary.contains("已完成"));
        assert!(summary.contains("传输中"));
        assert!(summary.contains("待分配"));
        assert!(!summary.contains("ConnectionPart {"));
    }

    #[test]
    fn confirm_duplicate_detection_is_variant_aware_for_adaptive_masters() {
        use super::{find_duplicate_task, VariantSelection};
        use std::collections::HashMap;

        let master = "https://cdn.example.test/video/master.m3u8";
        let existing = TaskSnapshot {
            task_id: "task-720".into(),
            url: master.into(),
            filename: "movie-720p.mp4".into(),
            resource_kind: ResourceKind::Hls,
            status: "downloading".into(),
            ..Default::default()
        };
        let mut variants = HashMap::new();
        variants.insert(
            "task-720".to_string(),
            VariantSelection {
                preferred_variant_id: "v720".into(),
                preferred_height: 720,
                preferred_bandwidth: 2_500_000,
            },
        );

        let same_variant = ResourceOffer {
            url: master.into(),
            resource_kind: ResourceKind::Hls,
            preferred_variant_id: "v720".into(),
            preferred_height: 720,
            preferred_bandwidth: 2_500_000,
            ..Default::default()
        };
        assert!(
            find_duplicate_task(std::slice::from_ref(&existing), &variants, &same_variant)
                .is_some()
        );

        let different_height = ResourceOffer {
            preferred_variant_id: "v1080".into(),
            preferred_height: 1080,
            preferred_bandwidth: 5_000_000,
            ..same_variant.clone()
        };
        assert!(find_duplicate_task(
            std::slice::from_ref(&existing),
            &variants,
            &different_height
        )
        .is_none());

        let different_variant_id_only = ResourceOffer {
            preferred_variant_id: "v720-alt".into(),
            ..same_variant.clone()
        };
        assert!(find_duplicate_task(
            std::slice::from_ref(&existing),
            &variants,
            &different_variant_id_only
        )
        .is_none());

        // Even without recorded handoff variant history, an explicit adaptive variant
        // does not falsely warn against a master-URL task.
        assert!(find_duplicate_task(&[existing], &HashMap::new(), &different_height).is_none());
    }

    #[test]
    fn confirm_meta_and_details_include_chosen_quality_and_sanitized_variant_hint() {
        let offer = ResourceOffer {
            resource_kind: ResourceKind::Dash,
            size: 64 * 1024 * 1024,
            mime_type: "application/dash+xml".into(),
            preferred_height: 1080,
            preferred_bandwidth: 4_800_000,
            preferred_variant_id:
                "https://user:secret@cdn.example.test/hls/1080p.m3u8?token=private".into(),
            ..Default::default()
        };
        assert_eq!(
            format_resource_meta(&offer, "movie.mpd"),
            "DASH · .mpd · 64.0 MB · 1080p · 4.8 Mbps"
        );
        let details = format_request_details(&offer);
        assert!(details.contains("清晰度：1080p"));
        assert!(details.contains("码率：4.8 Mbps"));
        assert!(details.contains("轨道：1080p.m3u8"));
        assert!(!details.contains("secret"));
        assert!(!details.contains("token"));

        let id_only = ResourceOffer {
            resource_kind: ResourceKind::Dash,
            preferred_variant_id: "rep-avc-high".into(),
            ..Default::default()
        };
        assert_eq!(
            format_resource_meta(&id_only, "stream.mpd"),
            "DASH · .mpd · 大小未知 · 轨道 rep-avc-high"
        );
    }

    #[test]
    fn clipboard_offer_filter_deduplicates_empty_repeats_and_expires_after_cooldown() {
        use super::ClipboardOfferFilter;
        use std::time::{Duration, Instant};

        let mut filter = ClipboardOfferFilter::default();
        let t0 = Instant::now();
        assert_eq!(filter.accept(&[], t0), None);
        assert_eq!(filter.accept(&["  ".into(), "\t".into()], t0), None);

        let batch = vec![
            " https://example.test/a.mp4 ".into(),
            "https://example.test/a.mp4".into(),
            "https://example.test/b.mp4".into(),
        ];
        assert_eq!(
            filter.accept(&batch, t0),
            Some(vec![
                "https://example.test/a.mp4".into(),
                "https://example.test/b.mp4".into(),
            ])
        );
        // Immediate repeat is suppressed to avoid dialog spam.
        assert_eq!(filter.accept(&batch, t0 + Duration::from_secs(5)), None);
        // Distinct batch is accepted immediately.
        assert_eq!(
            filter.accept(
                &["https://example.test/c.mp4".into()],
                t0 + Duration::from_secs(6)
            ),
            Some(vec!["https://example.test/c.mp4".into()])
        );
        // After cooldown elapses, re-copying the earlier batch is accepted again.
        assert_eq!(
            filter.accept(&batch, t0 + Duration::from_secs(31)),
            Some(vec![
                "https://example.test/a.mp4".into(),
                "https://example.test/b.mp4".into(),
            ])
        );
    }

    #[test]
    fn presenter_settings_preserves_mru_dirs_and_maps_queue_profiles_across_reloads() {
        let initial: CorePipeResponse = serde_json::from_value(serde_json::json!({
            "type": "settings",
            "request_id": 1,
            "takeover_enabled": true,
            "takeover_minimum_bytes": 0,
            "legal_accepted": true,
            "speed_limit_kib": 0,
            "download_dir": "D:\\Downloads",
            "category_dir_media": "D:\\Media",
            "category_dir_archive": "D:\\Archives",
            "queue_profiles": [
                {"id": "default", "name": "默认队列", "priority": 0},
                {"id": "night", "name": "夜间队列", "priority": 10}
            ]
        }))
        .unwrap();
        let mut presenter = PresenterSettings::from_response(Some(&initial));
        let mut runtime = PresenterRuntimeSettings::from_response(Some(&initial));
        presenter.record_recent_dir(r"E:\Projects\");
        presenter.record_recent_dir(r"E:\Projects");
        let (dirs, idx) = presenter.recent_dirs_for(r"D:\Media");
        assert_eq!(dirs[0], r"E:\Projects");
        assert_eq!(dirs[1], r"D:\Downloads");
        assert_eq!(dirs[2], r"D:\Media");
        assert_eq!(idx, 2);
        assert_eq!(presenter.queue_id_at(0), "default");
        assert_eq!(presenter.queue_id_at(1), "night");

        // Settings reload keeps session MRU directories intact.
        let reloaded = settings_response(r"D:\Downloads2");
        assert!(update_presenter_settings(&reloaded, &mut presenter, &mut runtime).is_some());
        let (dirs_after, _) = presenter.recent_dirs_for(r"D:\Downloads2");
        assert_eq!(dirs_after[0], r"E:\Projects");
        assert_eq!(dirs_after[1], r"D:\Downloads2");
    }

    #[test]
    fn confirm_window_selectors_populate_queues_and_recent_dirs_offscreen() {
        use super::{sync_confirm_selectors, ConfirmWindow};
        use slint::Model;

        crate::offscreen::OffscreenPlatform::init_test();
        let confirm = ConfirmWindow::new().unwrap();
        let response: CorePipeResponse = serde_json::from_value(serde_json::json!({
            "type": "settings",
            "request_id": 1,
            "takeover_enabled": true,
            "takeover_minimum_bytes": 0,
            "legal_accepted": true,
            "speed_limit_kib": 0,
            "download_dir": "D:\\Downloads",
            "category_dir_media": "D:\\Media",
            "queue_profiles": [
                {"id": "fast", "name": "高速队列", "priority": -5},
                {"id": "default", "name": "默认队列", "priority": 0}
            ]
        }))
        .unwrap();
        let mut settings = PresenterSettings::from_response(Some(&response));
        settings.record_recent_dir(r"F:\Custom");
        sync_confirm_selectors(&confirm, &settings, r"D:\Media", Some("default"));
        assert_eq!(
            confirm
                .get_queue_names()
                .iter()
                .map(String::from)
                .collect::<Vec<_>>(),
            vec!["高速队列", "默认队列"]
        );
        assert_eq!(confirm.get_queue_index(), 1);
        assert_eq!(
            confirm
                .get_recent_dirs()
                .iter()
                .map(String::from)
                .collect::<Vec<_>>(),
            vec![r"F:\Custom", r"D:\Downloads", r"D:\Media"]
        );
        assert_eq!(confirm.get_recent_dir_index(), 2);
    }

    #[test]
    fn presenter_settings_parses_quick_download_and_speed_limit_flags() {
        let response: CorePipeResponse = serde_json::from_value(serde_json::json!({
            "type": "settings",
            "request_id": 1,
            "takeover_enabled": true,
            "takeover_minimum_bytes": 0,
            "legal_accepted": true,
            "speed_limit_kib": 512,
            "speed_limit_enabled": true,
            "quick_download_enabled": true,
            "download_dir": "D:\\Downloads"
        }))
        .unwrap();
        let parsed = PresenterSettings::from_response(Some(&response));
        assert!(parsed.legal_accepted);
        assert_eq!(parsed.speed_limit_kib, 512);
        assert!(parsed.speed_limit_enabled);
        assert!(parsed.quick_download_enabled);

        let disabled = settings_response("D:\\Downloads");
        let parsed_disabled = PresenterSettings::from_response(Some(&disabled));
        assert!(!parsed_disabled.speed_limit_enabled);
        assert!(!parsed_disabled.quick_download_enabled);
    }

    #[test]
    fn quick_download_auto_accept_gates_on_settings_legal_duplicates_and_valid_target() {
        use super::{evaluate_quick_download, QuickDownloadPlan};
        use std::collections::HashMap;

        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let mut settings = PresenterSettings {
            download_dir: dir.clone(),
            legal_accepted: true,
            quick_download_enabled: true,
            ..Default::default()
        };
        let offer = ResourceOffer {
            handoff_id: "h-quick-1".into(),
            url: "https://cdn.example.test/archive.zip".into(),
            filename: "archive.zip".into(),
            resource_kind: ResourceKind::File,
            ..Default::default()
        };

        assert_eq!(
            evaluate_quick_download(&offer, &settings, &[], &HashMap::new()),
            Some(QuickDownloadPlan {
                handoff_id: "h-quick-1".into(),
                filename: "archive.zip".into(),
                download_dir: dir.clone(),
                queue_id: "default".into(),
            })
        );

        // Duplicate task falls back to ConfirmWindow.
        let existing = TaskSnapshot {
            task_id: "t-1".into(),
            url: offer.url.clone(),
            filename: "archive.zip".into(),
            status: "downloading".into(),
            ..Default::default()
        };
        assert_eq!(
            evaluate_quick_download(&offer, &settings, &[existing], &HashMap::new()),
            None
        );

        // Invalid filename falls back to ConfirmWindow.
        let bad_name = ResourceOffer {
            filename: "CON.zip".into(),
            ..offer.clone()
        };
        assert_eq!(
            evaluate_quick_download(&bad_name, &settings, &[], &HashMap::new()),
            None
        );

        // Legal terms not accepted falls back to ConfirmWindow.
        settings.legal_accepted = false;
        assert_eq!(
            evaluate_quick_download(&offer, &settings, &[], &HashMap::new()),
            None
        );

        // Quick download disabled falls back to ConfirmWindow.
        settings.legal_accepted = true;
        settings.quick_download_enabled = false;
        assert_eq!(
            evaluate_quick_download(&offer, &settings, &[], &HashMap::new()),
            None
        );
    }

    #[test]
    fn progress_hud_pause_in_place_pins_task_and_resumes_smoothly() {
        let mut hud = ActiveTaskHud::default();
        assert_eq!(
            hud.observe(hud_snapshot("t-1", "downloading"))
                .as_ref()
                .map(|t| t.status.as_str()),
            Some("downloading")
        );
        hud.mark_pause_requested("t-1");
        let paused = hud
            .observe(hud_snapshot("t-1", "paused"))
            .expect("HUD-initiated pause should stay pinned");
        assert_eq!(paused.task_id, "t-1");
        assert_eq!(paused.status, "paused");
        assert_eq!(hud.primary_task_id().as_deref(), Some("t-1"));
        assert_eq!(hud.task_position(), (1, 1));

        // Reconcile preserves a HUD-pinned paused task while still paused.
        let reconciled = hud
            .reconcile(&[hud_snapshot("t-1", "paused")])
            .expect("reconcile should keep HUD-pinned paused task");
        assert_eq!(reconciled.status, "paused");

        // Resuming in place returns to active state and clears the pause pin.
        let resumed = hud
            .observe(hud_snapshot("t-1", "downloading"))
            .expect("resumed task stays visible");
        assert_eq!(resumed.status, "downloading");
        // After resume, a later external pause (without mark_pause_requested) drops cleanly.
        assert!(hud.observe(hud_snapshot("t-1", "paused")).is_none());
        assert_eq!(hud.primary_task_id(), None);
    }

    #[test]
    fn progress_hud_external_pause_never_pops_or_pins_untracked_task() {
        let mut hud = ActiveTaskHud::default();
        // Pausing externally when HUD wasn't showing the task must never pop a HUD.
        assert!(hud.observe(hud_snapshot("ext-1", "paused")).is_none());
        assert_eq!(hud.task_position(), (0, 0));

        // Calling mark_pause_requested on an untracked task does not pin it either.
        hud.mark_pause_requested("ext-2");
        assert!(hud.observe(hud_snapshot("ext-2", "paused")).is_none());
        assert_eq!(hud.primary_task_id(), None);
    }

    #[test]
    fn progress_hud_dismiss_cancel_complete_and_delete_clear_pinned_paused_task() {
        // 1. Closing/dismissing HUD drops pinned paused tasks.
        let mut hud = ActiveTaskHud::default();
        hud.observe(hud_snapshot("t-1", "downloading"));
        hud.mark_pause_requested("t-1");
        hud.observe(hud_snapshot("t-1", "paused"));
        assert!(hud.dismiss_paused().is_none());
        assert_eq!(hud.primary_task_id(), None);

        // 2. Canceling clears pinned paused task.
        hud.observe(hud_snapshot("t-2", "downloading"));
        hud.mark_pause_requested("t-2");
        hud.observe(hud_snapshot("t-2", "paused"));
        assert!(hud.observe(hud_snapshot("t-2", "canceled")).is_none());

        // 3. Completing clears pinned paused task.
        hud.observe(hud_snapshot("t-3", "downloading"));
        hud.mark_pause_requested("t-3");
        hud.observe(hud_snapshot("t-3", "paused"));
        assert!(hud.observe(hud_snapshot("t-3", "completed")).is_none());

        // 4. Deleting clears pinned paused task.
        hud.observe(hud_snapshot("t-4", "downloading"));
        hud.mark_pause_requested("t-4");
        hud.observe(hud_snapshot("t-4", "paused"));
        assert!(hud.remove("t-4").is_none());

        // 5. Failed pause IPC clears pin cleanly.
        hud.observe(hud_snapshot("t-5", "downloading"));
        hud.mark_pause_requested("t-5");
        hud.clear_pause_pin("t-5");
        assert!(hud.observe(hud_snapshot("t-5", "paused")).is_none());
    }

    #[test]
    fn progress_hud_multi_task_switcher_cycles_and_tracks_position() {
        let mut hud = ActiveTaskHud::default();
        assert_eq!(hud.task_position(), (0, 0));
        assert!(hud.cycle(1).is_none());

        hud.observe(hud_snapshot("task-a", "downloading"));
        hud.observe(hud_snapshot("task-b", "downloading"));
        hud.observe(hud_snapshot("task-c", "recording"));
        assert_eq!(hud.primary_task_id().as_deref(), Some("task-a"));
        assert_eq!(hud.task_position(), (1, 3));

        // Cycle forward through all three tasks and wrap around.
        assert_eq!(
            hud.cycle(1).as_ref().map(|t| t.task_id.as_str()),
            Some("task-b")
        );
        assert_eq!(hud.task_position(), (2, 3));
        assert_eq!(
            hud.cycle(1).as_ref().map(|t| t.task_id.as_str()),
            Some("task-c")
        );
        assert_eq!(hud.task_position(), (3, 3));
        assert_eq!(
            hud.cycle(1).as_ref().map(|t| t.task_id.as_str()),
            Some("task-a")
        );
        assert_eq!(hud.task_position(), (1, 3));

        // Cycle backward wraps to the last task.
        assert_eq!(
            hud.cycle(-1).as_ref().map(|t| t.task_id.as_str()),
            Some("task-c")
        );
        assert_eq!(hud.task_position(), (3, 3));

        // Progress updates on other tasks do not steal the user's selected task.
        let shown = hud.observe(hud_snapshot("task-a", "downloading")).unwrap();
        assert_eq!(shown.task_id, "task-c");
        assert_eq!(hud.task_position(), (3, 3));

        // When the selected task finishes, selection falls back cleanly to the earliest task.
        let fallback = hud.observe(hud_snapshot("task-c", "completed")).unwrap();
        assert_eq!(fallback.task_id, "task-a");
        assert_eq!(hud.task_position(), (1, 2));
    }

    #[test]
    fn progress_hud_connection_rows_and_speed_curve_format_and_cap_at_eight() {
        use super::{
            build_connection_rows, build_progress_speed_data, format_hud_connection_summary,
        };

        let mb = 1024 * 1024;
        let parts: Vec<ConnectionPart> = (0..12)
            .map(|i| ConnectionPart {
                start: i * mb,
                end: (i + 1) * mb - 1,
                done: if i % 4 == 0 {
                    mb
                } else if i % 4 == 1 {
                    mb / 2
                } else if i % 4 == 2 {
                    mb / 4
                } else {
                    0
                },
                state: match i % 4 {
                    0 => "done",
                    1 => "active",
                    2 => "retrying",
                    _ => "queued",
                }
                .into(),
            })
            .collect();
        let task = TaskSnapshot {
            status: "downloading".into(),
            speed_bytes_per_sec: 4 * mb,
            active_workers: 3,
            speed_history: (1..=50).map(|n| n * 100 * 1024).collect(),
            connection_parts: parts,
            ..Default::default()
        };

        let rows = build_connection_rows(&task);
        assert_eq!(rows.len(), 8);
        // Active & retrying parts are prioritized (6 of them in 0..12), plus 2 recent others.
        assert!(rows.iter().any(|row| row.state_label.as_str() == "下载中"));
        assert!(rows.iter().any(|row| row.state_label.as_str() == "重试中"));
        assert!(rows.iter().all(|row| row.index.starts_with('#')));
        let active_row = rows
            .iter()
            .find(|row| row.state.as_str() == "active")
            .unwrap();
        assert!((active_row.progress - 0.5).abs() < 0.01);
        assert_eq!(active_row.transferred.as_str(), "512.0 KB / 1.0 MB");

        let (curve, stats) = build_progress_speed_data(&task);
        assert_eq!(curve.len(), 40);
        assert!((curve[39] - 1.0).abs() < 0.001);
        assert!(stats.contains("峰值"));
        assert!(stats.contains("平均"));

        let summary = format_hud_connection_summary(&task);
        assert!(summary.contains("3 活动"));
        assert!(summary.contains("3/12 分段"));
    }

    #[test]
    fn progress_hud_speed_limit_and_completion_action_helpers_emit_valid_core_values() {
        use super::{
            build_speed_limit_settings, completion_action_from_index, completion_action_index_for,
            format_hud_speed_limit_feedback, progress_window_logical_size, toggle_hud_speed_limit,
        };

        let (w_compact, h_compact) = progress_window_logical_size(false);
        let (w_expanded, h_expanded) = progress_window_logical_size(true);
        assert_eq!(w_compact, w_expanded);
        assert!(h_expanded > h_compact + 150.0);

        let unlimited = build_speed_limit_settings(false, 0);
        assert_eq!(
            unlimited["download_speed_limit_enabled"],
            serde_json::json!(false)
        );
        assert_eq!(unlimited["download_speed_limit_kib"], serde_json::json!(0));

        let preset_2m = build_speed_limit_settings(true, 2048);
        assert_eq!(
            preset_2m["download_speed_limit_enabled"],
            serde_json::json!(true)
        );
        assert_eq!(
            preset_2m["download_speed_limit_kib"],
            serde_json::json!(2048)
        );
        assert_eq!(format_hud_speed_limit_feedback(true, 2048), "已限速 2 MB/s");
        assert_eq!(format_hud_speed_limit_feedback(false, 0), "已取消限速");

        // Toggle from 0 defaults to 1024 KiB/s; toggling off preserves configured KiB.
        let (enabled, kib, values) = toggle_hud_speed_limit(false, 0);
        assert!(enabled);
        assert_eq!(kib, 1024);
        assert_eq!(values["download_speed_limit_kib"], serde_json::json!(1024));
        let (enabled_off, kib_kept, values_off) = toggle_hud_speed_limit(true, 512);
        assert!(!enabled_off);
        assert_eq!(kib_kept, 512);
        assert!(!values_off.contains_key("download_speed_limit_kib"));

        // Every combo index maps to a valid Core completion_power_action wire value.
        let valid_core = ["none", "sleep", "hibernate", "shutdown"];
        for index in 0..=4 {
            let (wire, open_folder) = completion_action_from_index(index);
            assert!(valid_core.contains(&wire), "invalid wire value {wire}");
            assert_eq!(completion_action_index_for(wire, open_folder), index);
        }
        assert_eq!(completion_action_from_index(1), ("none", true));
    }

    #[test]
    fn progress_window_fixture_populates_compact_and_expanded_offscreen() {
        use super::{populate_progress_fixture, ProgressWindow};
        use slint::Model;

        crate::offscreen::OffscreenPlatform::init_test();
        let compact = ProgressWindow::new().unwrap();
        populate_progress_fixture(&compact, false);
        assert!(!compact.get_expanded());
        assert_eq!(compact.get_task_index(), 1);
        assert_eq!(compact.get_task_count(), 3);
        assert_eq!(compact.get_connection_rows().row_count(), 6);
        assert!(compact.get_speed_history().row_count() >= 10);
        assert!(!compact.get_speed_stats().is_empty());

        let expanded = ProgressWindow::new().unwrap();
        populate_progress_fixture(&expanded, true);
        assert!(expanded.get_expanded());
        assert_eq!(expanded.get_connection_rows().row_count(), 6);
        assert_eq!(expanded.get_headline().as_str(), "下载进度");
    }

    #[test]
    fn duplicate_detection_catches_rotating_github_release_urls_and_target_path_matches() {
        use super::{
            find_duplicate_match, format_duplicate_notice, url_without_signing_params,
            DuplicateMatchKind,
        };
        use std::collections::HashMap;

        let paused_github_task = TaskSnapshot {
            task_id: "task-ytdlp-paused".into(),
            url: "https://objects.githubusercontent.com/github-production-release-asset-2e65be/307260205/asset-1?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Date=20261125T050000Z&X-Amz-Expires=300&X-Amz-Signature=aaaa1111&repo_id=307260205".into(),
            source_page_url: "https://github.com/yt-dlp/yt-dlp/releases".into(),
            download_dir: r"C:\Users\Admin\Downloads".into(),
            filename: "yt-dlp.exe".into(),
            resource_kind: ResourceKind::File,
            status: "paused".into(),
            ..Default::default()
        };

        // 1. Rotating GitHub Release signed URL (different X-Amz-Date & X-Amz-Signature).
        let second_click = ResourceOffer {
            url: "https://objects.githubusercontent.com/github-production-release-asset-2e65be/307260205/asset-1?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Date=20261125T050215Z&X-Amz-Expires=300&X-Amz-Signature=bbbb2222&repo_id=307260205".into(),
            source_page_url: "https://github.com/yt-dlp/yt-dlp/releases/".into(),
            filename: "yt-dlp.exe".into(),
            resource_kind: ResourceKind::File,
            ..Default::default()
        };
        assert_eq!(
            url_without_signing_params(&paused_github_task.url),
            url_without_signing_params(&second_click.url)
        );
        let found = find_duplicate_match(
            std::slice::from_ref(&paused_github_task),
            &HashMap::new(),
            &second_click,
            r"C:\Users\Admin\Downloads",
            "yt-dlp.exe",
        )
        .expect("rotating GitHub Release signed URL must be detected as duplicate");
        assert_eq!(found.task.task_id, "task-ytdlp-paused");
        assert_eq!(found.kind, DuplicateMatchKind::SameUrl);
        let (notice_text, dup_id) = format_duplicate_notice(found);
        assert_eq!(notice_text, "已有同一地址的任务：yt-dlp.exe（已暂停）");
        assert_eq!(dup_id, "task-ytdlp-paused");

        // 2. Rotating CDN host/path on the same GitHub Release source page.
        let different_cdn_host = ResourceOffer {
            url: "https://release-assets.githubusercontent.com/other-blob/yt-dlp.exe?se=2026-11-25&sig=zzzz".into(),
            source_page_url: "https://github.com/yt-dlp/yt-dlp/releases".into(),
            filename: "YT-DLP.EXE".into(),
            resource_kind: ResourceKind::File,
            ..Default::default()
        };
        let found_page = find_duplicate_match(
            std::slice::from_ref(&paused_github_task),
            &HashMap::new(),
            &different_cdn_host,
            r"D:\OtherDir",
            "YT-DLP.EXE",
        )
        .expect("same source_page_url + filename should detect duplicate");
        assert_eq!(found_page.task.task_id, "task-ytdlp-paused");
        assert_eq!(found_page.kind, DuplicateMatchKind::SameUrl);

        // 3. Different URL and no source_page_url, but exact same target folder + filename.
        let same_target_offer = ResourceOffer {
            url: "https://mirror.example.test/releases/yt-dlp.exe".into(),
            source_page_url: String::new(),
            filename: "yt-dlp.exe".into(),
            resource_kind: ResourceKind::File,
            ..Default::default()
        };
        let found_path = find_duplicate_match(
            std::slice::from_ref(&paused_github_task),
            &HashMap::new(),
            &same_target_offer,
            "c:/users/admin/downloads/",
            "YT-DLP.EXE",
        )
        .expect("same download_dir + filename must detect duplicate");
        assert_eq!(found_path.task.task_id, "task-ytdlp-paused");
        assert_eq!(found_path.kind, DuplicateMatchKind::SameTargetPath);
        let (path_notice, _) = format_duplicate_notice(found_path);
        assert_eq!(path_notice, "已有同一文件的任务：yt-dlp.exe（已暂停）");

        // 4. Renaming copy to "yt-dlp (2).exe" clears duplicate match cleanly.
        assert!(find_duplicate_match(
            std::slice::from_ref(&paused_github_task),
            &HashMap::new(),
            &second_click,
            r"C:\Users\Admin\Downloads",
            "yt-dlp (2).exe",
        )
        .is_none());

        // 5. Cancelled tasks do not trigger heuristic path/source/signed-URL matches.
        let mut cancelled = paused_github_task.clone();
        cancelled.status = "canceled".into();
        assert!(find_duplicate_match(
            &[cancelled],
            &HashMap::new(),
            &second_click,
            r"C:\Users\Admin\Downloads",
            "yt-dlp.exe",
        )
        .is_none());

        // 6. Adaptive HLS/DASH streams with different variants are never falsely matched.
        let hls_task = TaskSnapshot {
            task_id: "hls-720".into(),
            url: "https://cdn.example.test/live/master.m3u8?token=aaa".into(),
            source_page_url: "https://video.example.test/watch/1".into(),
            download_dir: r"C:\Users\Admin\Videos".into(),
            filename: "show.mp4".into(),
            resource_kind: ResourceKind::Hls,
            status: "downloading".into(),
            ..Default::default()
        };
        let hls_1080 = ResourceOffer {
            url: "https://cdn.example.test/live/master.m3u8?token=bbb".into(),
            source_page_url: "https://video.example.test/watch/1".into(),
            filename: "show.mp4".into(),
            resource_kind: ResourceKind::Hls,
            preferred_height: 1080,
            preferred_variant_id: "v1080".into(),
            ..Default::default()
        };
        assert!(find_duplicate_match(
            &[hls_task],
            &HashMap::new(),
            &hls_1080,
            r"C:\Users\Admin\Videos",
            "show.mp4",
        )
        .is_none());
    }

    #[test]
    fn progress_hud_processing_state_tracks_elapsed_and_avoids_misleading_100_percent() {
        use super::{
            format_processing_headline, processing_badge_label, render_progress_hud,
            resize_progress_window, PresenterRuntimeSettings, ProgressWindow,
            PROGRESS_HUD_COMPACT_HEIGHT, PROGRESS_HUD_EXPANDED_HEIGHT,
        };
        use slint::ComponentHandle;
        use std::time::{Duration, Instant};

        crate::offscreen::OffscreenPlatform::init_test();

        let mut hud = ActiveTaskHud::default();
        let t0 = Instant::now();
        hud.observe_at(hud_snapshot("hls-1", "downloading"), t0);
        assert_eq!(hud.processing_elapsed_secs("hls-1", t0), None);

        let mut merging = TaskSnapshot {
            task_id: "hls-1".into(),
            filename: "episode.mp4".into(),
            resource_kind: ResourceKind::Hls,
            status: "merging".into(),
            downloaded_bytes: 50 * 1024 * 1024,
            total_bytes: Some(50 * 1024 * 1024),
            completed_ranges: 100,
            total_ranges: 100,
            ..Default::default()
        };
        let t1 = t0 + Duration::from_secs(5);
        hud.observe_at(merging.clone(), t1);
        assert_eq!(hud.processing_elapsed_secs("hls-1", t1), Some(0));

        // Subsequent merging snapshots preserve the original merge start timestamp.
        let t2 = t1 + Duration::from_secs(12);
        hud.observe_at(merging.clone(), t2);
        assert_eq!(hud.processing_elapsed_secs("hls-1", t2), Some(12));
        assert_eq!(
            format_processing_headline("merging", Some(12)),
            "合并音视频中 · 已耗时 12s"
        );
        assert_eq!(processing_badge_label("merging"), "合并中");
        assert_eq!(
            format_processing_headline("checking", Some(65)),
            "校验文件中 · 已耗时 1m5s"
        );
        assert_eq!(processing_badge_label("checking"), "校验中");

        let progress = ProgressWindow::new().unwrap();
        let settings = PresenterRuntimeSettings {
            show_progress: true,
            ..Default::default()
        };
        render_progress_hud(
            &progress.as_weak(),
            settings,
            &merging,
            (1, 1),
            "",
            Some(12),
            None,
        );
        assert!(progress.get_processing());
        assert!(!progress.get_progress_known());
        assert_eq!(progress.get_processing_badge().as_str(), "合并中");
        assert_eq!(progress.get_speed().as_str(), "合并音视频中 · 已耗时 12s");
        assert_eq!(progress.get_stage_label().as_str(), "正在合并音视频");
        assert_eq!(
            progress.get_remaining().as_str(),
            "传输已完成，文件尚未就绪"
        );

        // Compact <-> Expanded resize cleanly grows and shrinks back to compact height.
        resize_progress_window(&progress, true);
        assert!(progress.get_expanded());
        let expanded_size = progress
            .window()
            .size()
            .to_logical(progress.window().scale_factor());
        assert!((expanded_size.height - PROGRESS_HUD_EXPANDED_HEIGHT).abs() < 1.5);

        resize_progress_window(&progress, false);
        assert!(!progress.get_expanded());
        let compact_size = progress
            .window()
            .size()
            .to_logical(progress.window().scale_factor());
        assert!((compact_size.height - PROGRESS_HUD_COMPACT_HEIGHT).abs() < 1.5);

        // Finishing the task clears the processing clock.
        merging.status = "completed".into();
        hud.observe_at(merging, t2 + Duration::from_secs(3));
        assert_eq!(
            hud.processing_elapsed_secs("hls-1", t2 + Duration::from_secs(3)),
            None
        );
    }

    #[test]
    fn confirm_window_defaults_remember_off_reresolves_category_until_overridden_and_checks_space()
    {
        use super::{
            refresh_confirm_disk_space, sync_confirm_filename_category, ConfirmWindow,
            PresenterSettings,
        };

        crate::offscreen::OffscreenPlatform::init_test();
        let confirm = ConfirmWindow::new().unwrap();
        assert!(
            !confirm.get_remember_directory(),
            "ConfirmWindow must not default remember_directory to true"
        );

        let settings = PresenterSettings {
            download_dir: r"D:\Downloads".into(),
            category_media: r"D:\Downloads\Media".into(),
            category_archive: r"D:\Downloads\Archives".into(),
            category_program: r"D:\Downloads\Programs".into(),
            category_other: r"D:\Downloads\Other".into(),
            ..Default::default()
        };

        // Initial media filename resolves media category & folder.
        confirm.set_filename("lecture.mp4".into());
        sync_confirm_filename_category(&confirm, &settings, ResourceKind::File, false);
        assert_eq!(confirm.get_category().as_str(), "media");
        assert_eq!(confirm.get_download_dir().as_str(), r"D:\Downloads\Media");

        // Changing extension to .zip dynamically switches category & save directory.
        confirm.set_filename("lecture-materials.zip".into());
        sync_confirm_filename_category(&confirm, &settings, ResourceKind::File, false);
        assert_eq!(confirm.get_category().as_str(), "archive");
        assert_eq!(
            confirm.get_download_dir().as_str(),
            r"D:\Downloads\Archives"
        );

        // Once the user manually overrides the directory, editing filename updates category
        // without clobbering their chosen folder.
        confirm.set_download_dir(r"F:\CustomProject".into());
        confirm.set_filename("installer.exe".into());
        sync_confirm_filename_category(&confirm, &settings, ResourceKind::File, true);
        assert_eq!(confirm.get_category().as_str(), "program");
        assert_eq!(confirm.get_download_dir().as_str(), r"F:\CustomProject");

        #[cfg(windows)]
        {
            let temp = std::env::temp_dir().to_string_lossy().into_owned();
            confirm.set_download_dir(temp.into());
            refresh_confirm_disk_space(&confirm, 1024);
            assert!(!confirm.get_free_space_low());
            assert!(confirm.get_free_space_hint().starts_with("磁盘可用 "));

            refresh_confirm_disk_space(&confirm, u64::MAX);
            assert!(confirm.get_free_space_low());
            assert!(confirm
                .get_free_space_hint()
                .starts_with("磁盘剩余空间不足"));
        }
    }

    #[test]
    fn complete_toast_preserves_active_power_countdown_and_confirm_preview_strips_duplicate_slash()
    {
        use super::{apply_completed_snapshot_to_toast, format_save_preview, CompleteWindow};

        assert_eq!(format_save_preview(r"D:\", "movie.mp4"), r"D:\movie.mp4");
        assert_eq!(
            format_save_preview(r"D:\下载\媒体", "movie.mp4"),
            r"D:\下载\媒体\movie.mp4"
        );
        assert_eq!(
            format_save_preview(r"D:\下载\媒体\", "movie.mp4"),
            r"D:\下载\媒体\movie.mp4"
        );
        assert_eq!(format_save_preview("", "movie.mp4"), "movie.mp4");

        crate::offscreen::OffscreenPlatform::init_test();
        let complete = CompleteWindow::new().unwrap();
        complete.set_power_hint("关机 将在 180 秒后执行".into());
        complete.set_power_action_label("关机".into());
        complete.set_power_seconds(180);

        let finished = TaskSnapshot {
            task_id: "late-finish".into(),
            filename: "bonus.mp4".into(),
            status: "completed".into(),
            downloaded_bytes: 64 * 1024 * 1024,
            ..Default::default()
        };
        apply_completed_snapshot_to_toast(&complete, &finished);
        assert_eq!(complete.get_filename().as_str(), "bonus.mp4");
        assert_eq!(complete.get_power_seconds(), 180);
        assert_eq!(complete.get_power_action_label().as_str(), "关机");
        assert_eq!(complete.get_power_hint().as_str(), "关机 将在 180 秒后执行");

        // When no power countdown is active, power properties stay clear.
        let plain = CompleteWindow::new().unwrap();
        apply_completed_snapshot_to_toast(&plain, &finished);
        assert_eq!(plain.get_power_seconds(), 0);
        assert_eq!(plain.get_power_hint().as_str(), "");

        // CompleteWindow auto-dismiss holds while busy, hovered, or showing power countdown.
        assert!(!super::should_hold_complete_toast(false, 0, "", false));
        assert!(super::should_hold_complete_toast(true, 0, "", false));
        assert!(super::should_hold_complete_toast(false, 180, "", false));
        assert!(super::should_hold_complete_toast(
            false,
            0,
            "关机 将在 180 秒后执行",
            false
        ));
        assert!(super::should_hold_complete_toast(false, 0, "", true));
    }

    #[cfg(windows)]
    #[test]
    fn frameless_window_proc_strips_ws_caption_and_ignores_null_stylestruct() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GWL_STYLE, STYLESTRUCT, WM_STYLECHANGING, WS_CAPTION, WS_POPUP, WS_VISIBLE,
        };
        let mut style = STYLESTRUCT {
            styleOld: WS_POPUP,
            styleNew: WS_POPUP | WS_VISIBLE | WS_CAPTION,
        };
        let rc = unsafe {
            super::frameless_window_proc(
                std::ptr::null_mut(),
                WM_STYLECHANGING,
                GWL_STYLE as usize,
                std::ptr::from_mut(&mut style) as isize,
                1,
                0,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(style.styleNew, WS_POPUP | WS_VISIBLE);

        // Synthetic WM_STYLECHANGING with a null lparam must not dereference null.
        let null_rc = unsafe {
            super::frameless_window_proc(
                std::ptr::null_mut(),
                WM_STYLECHANGING,
                GWL_STYLE as usize,
                0,
                1,
                0,
            )
        };
        assert_eq!(null_rc, 0);
    }
}
