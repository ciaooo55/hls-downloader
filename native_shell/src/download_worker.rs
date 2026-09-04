//! Task execution adapter for the resident Rust download engine.

use crate::{
    apply_replay_json_for, run_job_report, with_replay_json, AvScanStatus, CoreCommand, CoreEvent,
    CredentialVault, EventEnvelope, MediaPushRequest, MirrorStatus, PersistentCore, QueueProfile,
    TaskSpec, TorrentSession,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_COOKIE_CREDENTIAL_REF: &str = "settings:default-cookie";
const MAX_REPLAY_BODY_BYTES: usize = 128 * 1024;
const HANDOFF_PRESENTER_LEASE_MS: u64 = 15_000;

fn handoff_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

struct TemporaryRequestBody(Option<PathBuf>);

impl Drop for TemporaryRequestBody {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskPaths {
    pub output: PathBuf,
    pub final_output: PathBuf,
    pub control: PathBuf,
    pub progress: PathBuf,
    pub torrent_selection: PathBuf,
}

impl TaskPaths {
    pub fn for_task(task_id: &str, spec: &TaskSpec) -> Result<Self, String> {
        let root = if !spec.download_dir.trim().is_empty() {
            PathBuf::from(&spec.download_dir)
        } else if let Some(root) = std::env::var_os("HLS_V7_DOWNLOAD_DIR") {
            PathBuf::from(root)
        } else {
            PathBuf::from("downloads")
        };
        let work_root = if spec.work_dir.trim().is_empty() {
            root.clone()
        } else {
            PathBuf::from(&spec.work_dir)
        };
        let current_task_dir = work_root.join(".hls-tasks").join(task_id);
        let legacy_task_dir = root.join(".v6-tasks").join(task_id);
        let task_dir = if !current_task_dir.exists() && legacy_task_dir.exists() {
            legacy_task_dir
        } else {
            current_task_dir
        };
        let final_name = if spec.resource_kind == crate::ResourceKind::Torrent
            && !spec.torrent_selection.is_empty()
        {
            format!("{}.files", safe_filename(&spec.filename, &spec.url))
        } else {
            safe_filename(&spec.filename, &spec.url)
        };
        Ok(Self {
            output: task_dir.join("payload.downloading"),
            final_output: root.join(final_name),
            control: task_dir.join("control"),
            progress: task_dir.join("progress.json"),
            torrent_selection: task_dir.join("torrent-selection.json"),
        })
    }

    pub fn prepare(&self) -> Result<(), String> {
        if let Some(parent) = self.output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create task directory: {error}"))?;
        }
        if let Some(parent) = self.final_output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create output directory: {error}"))?;
        }
        if !self.control.exists() {
            fs::write(&self.control, "run")
                .map_err(|error| format!("write task control: {error}"))?;
        }
        Ok(())
    }

    pub fn set_control(&self, value: &str) -> Result<(), String> {
        fs::write(&self.control, value).map_err(|error| format!("write task control: {error}"))
    }

    pub fn task_dir(&self) -> PathBuf {
        self.output
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.output.clone())
    }

    pub fn publish(&self) -> Result<(), String> {
        self.publish_with("overwrite", false).map(|_| ())
    }

    pub fn publish_with(&self, policy: &str, keep_temp: bool) -> Result<PathBuf, String> {
        crate::output_path::publish_file(&self.output, &self.final_output, policy, keep_temp)
    }
}

static TORRENT_SELECTION_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static TORRENT_SELECTION_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(windows)]
fn replace_torrent_selection_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_torrent_selection_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

fn write_torrent_selection_locked(
    path: &Path,
    selections: &[crate::TorrentFileSelection],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create BT selection directory: {error}"))?;
    }
    let encoded = serde_json::to_vec(selections)
        .map_err(|error| format!("encode BT file selection: {error}"))?;
    let mut temporary_name = path.file_name().unwrap_or_default().to_os_string();
    temporary_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        TORRENT_SELECTION_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = path.with_file_name(temporary_name);
    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        replace_torrent_selection_file(&temporary, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result.map_err(|error| format!("publish BT file selection: {error}"))
}

fn write_torrent_selection(
    path: &Path,
    selections: &[crate::TorrentFileSelection],
) -> Result<(), String> {
    let _guard = TORRENT_SELECTION_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "BT file selection write lock poisoned".to_string())?;
    write_torrent_selection_locked(path, selections)
}

fn initialize_torrent_selection(
    path: &Path,
    selections: &[crate::TorrentFileSelection],
) -> Result<(), String> {
    let _guard = TORRENT_SELECTION_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "BT file selection write lock poisoned".to_string())?;
    if path.exists() {
        return Ok(());
    }
    write_torrent_selection_locked(path, selections)
}

pub fn constrain_untrusted_download_dir(
    requested: &str,
    configured: &str,
) -> Result<String, String> {
    reject_path_escape(requested)?;
    let configured = configured.trim();
    let root = PathBuf::from(if configured.is_empty() {
        "downloads"
    } else {
        configured
    });
    let requested = requested.trim();
    if requested.is_empty() {
        return Ok(root.to_string_lossy().into_owned());
    }
    let path = Path::new(requested);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let root_canon = logical_canonical(&root);
    let candidate_canon = logical_canonical(&candidate);
    if candidate_canon == root_canon || candidate_canon.starts_with(&root_canon) {
        return Ok(candidate.to_string_lossy().into_owned());
    }
    Err("下载目录必须位于默认下载根目录内".into())
}

fn logical_canonical(path: &Path) -> PathBuf {
    let mut cur = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        if let Ok(canon) = std::fs::canonicalize(&cur) {
            let mut out = canon;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match cur.file_name() {
            Some(name) => {
                suffix.push(OsString::from(name));
                if !cur.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    path.to_path_buf()
}

fn reject_path_escape(path: &str) -> Result<(), String> {
    if Path::new(path)
        .components()
        .any(|item| matches!(item, Component::ParentDir))
    {
        return Err("下载目录不能包含 ..".into());
    }
    Ok(())
}

fn header_value_allowed(key: &str, value: &str) -> bool {
    !key.is_empty()
        && !key.contains(['\r', '\n', '\0', ':'])
        && !value.chars().any(|ch| ch.is_control())
}

fn reject_task_url(url: &str) -> Result<(), String> {
    let url = url.trim().trim_start_matches('\u{feff}');
    if url.is_empty() {
        return Err("链接为空".into());
    }
    if crate::looks_like_metalink(url) {
        return Ok(());
    }
    if url.chars().any(|ch| ch.is_control()) {
        return Err("链接不能包含控制字符".into());
    }
    if is_importable_local_path(url) {
        return Ok(());
    }
    if crate::http_engine::remote_resource_url_allowed(url) {
        return Ok(());
    }
    Err("链接协议不受支持".into())
}

fn is_importable_local_path(url: &str) -> bool {
    let bytes = url.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        return true;
    }
    if cfg!(unix) && url.starts_with('/') && !url.starts_with("//") {
        return true;
    }
    if let Some(rest) = url.strip_prefix("\\\\") {
        let server = rest.split(['\\', '/']).next().unwrap_or("");
        return !server.is_empty() && server != "." && server != "?";
    }
    false
}

fn proxy_url_allowed(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() {
        return true;
    }
    if url.chars().any(|ch| ch.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("socks5://")
        || lower.starts_with("socks5h://")
}

fn validate_helper_executable(path: &str, names: &[&str]) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(());
    }
    reject_path_escape(path)?;
    if path
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '&' | '|' | ';' | '<' | '>'))
    {
        return Err("外部工具路径无效".into());
    }
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !names
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
    {
        return Err("外部工具路径必须指向 ffmpeg 可执行文件".into());
    }
    Ok(())
}

fn reject_scan_shell(command: &str) -> Result<(), String> {
    crate::av_scan::validate_custom_command(command)
}

pub fn build_job(
    task_id: &str,
    spec: &TaskSpec,
) -> Result<(crate::http_engine::Job, TaskPaths), String> {
    let paths = TaskPaths::for_task(task_id, spec)?;
    paths.prepare()?;
    let method = crate::http_engine::sanitize_http_method(&spec.request_method);
    let job = crate::http_engine::Job {
        url: spec.url.clone(),
        headers: spec
            .headers
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        output: paths.output.clone(),
        connections: spec.concurrency.max(1) as usize,
        chunk_bytes: 8 * 1024 * 1024,
        total: spec.expected_size.unwrap_or(0),
        sequential: method.eq_ignore_ascii_case("POST"),
        resume_from: 0,
        proxy: spec.proxy.clone(),
        resource_key: spec.url.clone(),
        etag: spec.etag.clone(),
        last_modified: spec.last_modified.clone(),
        control: paths.control.clone(),
        progress: paths.progress.clone(),
        method,
        body_path: PathBuf::from(&spec.body_path),
        mirrors: spec.mirrors.clone(),
        replay_json: String::new(),
    };
    Ok((job, paths))
}

fn hydrate_replay_headers(
    core: &Arc<Mutex<PersistentCore>>,
    mut spec: TaskSpec,
) -> Result<(TaskSpec, String), String> {
    let Some(credential_ref) = spec.credential_ref.clone() else {
        return Ok((spec, String::new()));
    };
    let blob = {
        let locked = core
            .lock()
            .map_err(|_| "v7 Core mutex poisoned".to_string())?;
        locked.store().load_credential(&credential_ref)?
    };
    let Some(blob) = blob else {
        return Ok((spec, String::new()));
    };
    let plain = CredentialVault.unprotect(&blob).unwrap_or(blob);
    let replay = crate::credentials::bind_replay_source_url(&plain, &spec.url);
    apply_replay_json_for(&mut spec.headers, &replay, &spec.url);
    Ok((spec, replay))
}

fn safe_filename(filename: &str, url: &str) -> String {
    let candidate = if filename.trim().is_empty() {
        url.split(['?', '#'])
            .next()
            .unwrap_or(url)
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("download")
    } else {
        filename.trim()
    };
    let cleaned: String = candidate
        .chars()
        .map(|ch| {
            if ch.is_control()
                || matches!(
                    ch,
                    '<' | '>'
                        | ':'
                        | '"'
                        | '/'
                        | '\\'
                        | '|'
                        | '?'
                        | '*'
                        | '&'
                        | '%'
                        | '^'
                        | ';'
                        | '`'
                        | '\u{7f}'
                )
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches([' ', '.']).to_string();
    let cleaned = if cleaned.is_empty() {
        "download".into()
    } else if cleaned.chars().count() > 200 {
        cleaned.chars().take(200).collect()
    } else {
        cleaned
    };
    if reserved_dos_device_name(&cleaned) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn reserved_dos_device_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "CONIN$"
            | "CONOUT$"
            | "CLOCK$"
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

#[derive(Clone)]
pub struct CoreCoordinator {
    core: Arc<Mutex<PersistentCore>>,
    active: Arc<Mutex<HashSet<String>>>,
    retries: Arc<Mutex<HashMap<String, u32>>>,
    update_shutdown: Arc<AtomicBool>,
    #[cfg(test)]
    worker_wait_started: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

#[derive(Debug, Clone)]
pub struct CoreSettings {
    pub takeover_enabled: bool,
    pub takeover_minimum_bytes: u64,
    pub legal_accepted: bool,
    pub speed_limit_kib: u64,
    pub hourly_quota_mib: u64,
    pub schedule_enabled: bool,
    pub schedule_start: String,
    pub schedule_end: String,
    pub schedule_kib: u64,
    pub auto_category: bool,
    pub category_dirs: crate::category::CategoryDirs,
    pub queue_max: u64,
    pub queue_profiles: Vec<QueueProfile>,
    pub site_rules: String,
    pub av_scan_enabled: bool,
    pub av_scan_command: String,
    pub torrent_watch: String,
    pub torrent_watch_enabled: bool,
    pub download_dir: String,
    pub temp_dir: String,
    pub default_concurrency: u64,
    pub proxy_url: String,
    pub ffmpeg_path: String,
    pub clipboard_watch: bool,
    pub completion_sound_enabled: bool,
    pub progress_window_enabled: bool,
    pub complete_popup_enabled: bool,
    pub resume_interrupted: bool,
    pub auto_retry_max: u64,
    pub existing_file_policy: String,
    pub live_record_max_minutes: u64,
    pub download_subtitles: bool,
    pub skip_ad_segments: bool,
    pub keep_temp_files: bool,
    pub default_user_agent: String,
    pub tvbox_endpoint: String,
    pub dark_mode: bool,
    pub allow_duplicate: bool,
    pub queue_auto_start_enabled: bool,
    pub queue_auto_start_time: String,
    pub queue_auto_stop_enabled: bool,
    pub queue_auto_stop_time: String,
    pub default_referer: String,
    pub default_origin: String,
    pub allowed_hosts: String,
    pub http_chunk_size_mb: u64,
    pub completion_power_action: String,
    pub start_on_login: bool,
    pub queue_active_days: String,
    pub proxy_mode: String,
    pub proxy_bypass: String,
    pub legal_terms_version: String,
    pub reduce_motion: bool,
    pub harvest_minimum_bytes: u64,
    pub av_scan_fail_on_threat: bool,
    pub bt_upload_limit_kib: u64,
    pub bt_max_connections: u64,
    pub bt_enable_dht: bool,
    pub preferred_cast_device_id: String,
    pub task_column_layout: String,
    pub toolbar_actions: String,
    pub task_sort: String,
}

impl CoreCoordinator {
    pub fn new(core: PersistentCore) -> Self {
        Self {
            core: Arc::new(Mutex::new(core)),
            active: Arc::new(Mutex::new(HashSet::new())),
            retries: Arc::new(Mutex::new(HashMap::new())),
            update_shutdown: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            worker_wait_started: Arc::new(Mutex::new(None)),
        }
    }

    pub fn core(&self) -> Arc<Mutex<PersistentCore>> {
        Arc::clone(&self.core)
    }

    pub fn latest_sequence(&self) -> Result<u64, String> {
        self.lock().map(|core| core.latest_sequence())
    }

    pub fn events_after(&self, sequence: u64, limit: usize) -> Result<Vec<EventEnvelope>, String> {
        self.lock().map(|core| core.events_after(sequence, limit))
    }

    pub fn settings(&self) -> Result<CoreSettings, String> {
        let core = self.lock()?;
        let configured_download_dir = core.store().setting_string("download_dir", "")?;
        let download_dir = if configured_download_dir.trim().is_empty()
            || Path::new(&configured_download_dir).is_relative()
        {
            crate::default_v7_download_dir()
                .to_string_lossy()
                .into_owned()
        } else {
            configured_download_dir
        };
        Ok(CoreSettings {
            takeover_enabled: core
                .store()
                .setting_bool("browser_takeover_enabled", true)?,
            takeover_minimum_bytes: core
                .store()
                .setting_u64("browser_takeover_minimum_bytes", 0)?,
            legal_accepted: core.store().setting_bool("legal_terms_accepted", false)?,
            speed_limit_kib: core.store().setting_u64("download_speed_limit_kib", 0)?,
            hourly_quota_mib: core.store().setting_u64("download_hourly_quota_mib", 0)?,
            schedule_enabled: core
                .store()
                .setting_bool("download_speed_schedule_enabled", false)?,
            schedule_start: core
                .store()
                .setting_string("download_speed_schedule_start", "22:00")?,
            schedule_end: core
                .store()
                .setting_string("download_speed_schedule_end", "08:00")?,
            schedule_kib: core.store().setting_u64("download_speed_schedule_kib", 0)?,
            auto_category: core.store().setting_bool("auto_category_dirs", false)?,
            category_dirs: crate::category::parse_category_dirs(
                &core.store().setting_string("browser_category_dirs", "")?,
            ),
            queue_max: core.store().setting_u64("queue_max_active", 3)?.max(1),
            queue_profiles: load_queue_profiles(core.store())?,
            site_rules: core.store().setting_string("site_rules", "")?,
            av_scan_enabled: core.store().setting_bool("av_scan_enabled", false)?,
            av_scan_command: core.store().setting_string("av_scan_command", "")?,
            torrent_watch: core.store().setting_string("torrent_watch_dir", "")?,
            torrent_watch_enabled: core.store().setting_bool("watch_torrents", false)?,
            download_dir,
            temp_dir: core.store().setting_string("temp_dir", "")?,
            default_concurrency: core.store().setting_u64("default_concurrency", 12)?.max(1),
            proxy_url: core.store().setting_string("proxy_url", "")?,
            ffmpeg_path: core.store().setting_string("ffmpeg_path", "")?,
            clipboard_watch: core.store().setting_bool("clipboard_watch", false)?,
            completion_sound_enabled: core
                .store()
                .setting_bool("completion_sound_enabled", false)?,
            progress_window_enabled: core
                .store()
                .setting_bool("download_progress_window_enabled", true)?,
            complete_popup_enabled: core
                .store()
                .setting_bool("download_complete_popup_enabled", true)?,
            resume_interrupted: core
                .store()
                .setting_bool("resume_interrupted_on_startup", false)?,
            auto_retry_max: core.store().setting_u64("auto_retry_failed_max", 0)?,
            existing_file_policy: core
                .store()
                .setting_string("existing_file_policy", "rename")?,
            live_record_max_minutes: core.store().setting_u64("live_record_max_minutes", 0)?,
            download_subtitles: core.store().setting_bool("download_subtitles", true)?,
            skip_ad_segments: core.store().setting_bool("skip_ad_segments", true)?,
            keep_temp_files: core.store().setting_bool("keep_temp_files", false)?,
            default_user_agent: core.store().setting_string("default_user_agent", "")?,
            tvbox_endpoint: core.store().setting_string("tvbox_endpoint", "")?,
            dark_mode: core.store().setting_bool("dark_mode", false)?,
            allow_duplicate: core.store().setting_bool("allow_duplicate", false)?,
            queue_auto_start_enabled: core
                .store()
                .setting_bool("queue_auto_start_enabled", false)?,
            queue_auto_start_time: core
                .store()
                .setting_string("queue_auto_start_time", "00:00")?,
            queue_auto_stop_enabled: core
                .store()
                .setting_bool("queue_auto_stop_enabled", false)?,
            queue_auto_stop_time: core
                .store()
                .setting_string("queue_auto_stop_time", "07:30")?,
            default_referer: core.store().setting_string("default_referer", "")?,
            default_origin: core.store().setting_string("default_origin", "")?,
            allowed_hosts: core.store().setting_string("allowed_hosts", "")?,
            http_chunk_size_mb: core
                .store()
                .setting_u64("http_chunk_size_mb", 8)?
                .clamp(1, 64),
            completion_power_action: core
                .store()
                .setting_string("completion_power_action", "none")?,
            start_on_login: core.store().setting_bool("start_on_login", false)?,
            queue_active_days: core
                .store()
                .setting_string("queue_active_days", "1,2,3,4,5,6,7")?,
            proxy_mode: core.store().setting_string("proxy_mode", "system")?,
            proxy_bypass: core.store().setting_string("proxy_bypass", "")?,
            legal_terms_version: core.store().setting_string("legal_terms_version", "")?,
            reduce_motion: core.store().setting_bool("reduce_motion", false)?,
            harvest_minimum_bytes: core.store().setting_u64("harvest_minimum_bytes", 0)?,
            av_scan_fail_on_threat: core.store().setting_bool("av_scan_fail_on_threat", true)?,
            bt_upload_limit_kib: core.store().setting_u64("bt_upload_limit_kib", 1024)?,
            bt_max_connections: core
                .store()
                .setting_u64("bt_max_connections", 200)?
                .clamp(10, 1000),
            bt_enable_dht: core.store().setting_bool("bt_enable_dht", true)?,
            preferred_cast_device_id: core
                .store()
                .setting_string("preferred_cast_device_id", "")?,
            task_column_layout: core.store().setting_string("task_column_layout", "")?,
            toolbar_actions: core.store().setting_string("toolbar_actions", "")?,
            task_sort: core.store().setting_string("task_sort", "queue:asc")?,
        })
    }

    pub fn set_setting(&self, key: &str, value: Value) -> Result<(), String> {
        self.set_settings(BTreeMap::from([(key.to_string(), value)]))
    }

    pub fn set_settings(&self, mut values: BTreeMap<String, Value>) -> Result<(), String> {
        for (key, value) in &values {
            Self::validate_setting(key, value)?;
        }
        let queue_profiles = if let Some(value) = values.get("queue_profiles") {
            let profiles: Vec<QueueProfile> = serde_json::from_value(value.clone())
                .map_err(|error| format!("队列配置格式无效: {error}"))?;
            Some(profiles)
        } else {
            None
        };
        if values.get("legal_terms_accepted").and_then(Value::as_bool) == Some(true) {
            values.insert(
                "legal_terms_version".into(),
                Value::String(crate::LEGAL_TERMS_VERSION.into()),
            );
        }
        let start_login = values.get("start_on_login").and_then(Value::as_bool);
        if let Some(flag) = start_login {
            crate::startup::apply(flag)?;
        }
        if let Some(profiles) = &queue_profiles {
            let ids: HashSet<_> = profiles.iter().map(|profile| profile.id.as_str()).collect();
            let mut core = self.lock()?;
            let orphaned = core
                .tasks()
                .into_iter()
                .filter(|task| !ids.contains(task.queue_id.as_str()))
                .map(|task| task.task_id)
                .collect();
            core.assign_queue_and_set_settings(orphaned, crate::DEFAULT_QUEUE_ID.into(), &values)?;
        } else {
            self.lock()?.store_mut().set_settings(&values)?;
            self.lock()?.emit(CoreEvent::SettingsChanged {
                keys: values.keys().cloned().collect(),
            })?;
        }
        if values.keys().any(|key| {
            matches!(
                key.as_str(),
                "download_speed_limit_kib"
                    | "download_speed_schedule_enabled"
                    | "download_speed_schedule_start"
                    | "download_speed_schedule_end"
                    | "download_speed_schedule_kib"
            )
        }) {
            let core = self.lock()?;
            crate::net_policy::configure_global_schedule(
                core.store().setting_u64("download_speed_limit_kib", 0)?,
                core.store()
                    .setting_bool("download_speed_schedule_enabled", false)?,
                &core.store()
                    .setting_string("download_speed_schedule_start", "22:00")?,
                &core.store()
                    .setting_string("download_speed_schedule_end", "08:00")?,
                core.store().setting_u64("download_speed_schedule_kib", 0)?,
            );
        }
        if let Some(limit) = values
            .get("download_hourly_quota_mib")
            .and_then(Value::as_u64)
        {
            crate::net_policy::configure_hourly_quota_mib(limit);
        }
        if let Some(profiles) = queue_profiles {
            crate::net_policy::sync_queue_limits(
                profiles
                    .iter()
                    .map(|profile| (profile.id.as_str(), profile.speed_limit_kib)),
            );
        }
        Ok(())
    }

    fn validate_setting(key: &str, value: &Value) -> Result<(), String> {
        if !PUBLIC_SETTING_KEYS.contains(&key) && key != "legal_terms_version" {
            return Err(format!("未知设置项: {key}"));
        }
        if key == "ffmpeg_path" {
            if let Some(path) = value.as_str() {
                validate_helper_executable(path, &["ffmpeg", "ffmpeg.exe"])?;
            }
        }
        if key == "av_scan_command" {
            if let Some(command) = value.as_str() {
                reject_scan_shell(command)?;
            }
        }
        if key == "download_dir" || key == "temp_dir" || key == "torrent_watch_dir" {
            if let Some(path) = value.as_str() {
                if !path.trim().is_empty() {
                    reject_path_escape(path)?;
                }
            }
        }
        if key == "proxy_url" {
            if let Some(url) = value.as_str() {
                if !proxy_url_allowed(url) {
                    return Err("代理地址无效".into());
                }
            }
        }
        if key == "browser_category_dirs" {
            if let Some(raw) = value.as_str() {
                let dirs = crate::category::parse_category_dirs(raw);
                reject_path_escape(&dirs.media)?;
                reject_path_escape(&dirs.program)?;
                reject_path_escape(&dirs.archive)?;
                reject_path_escape(&dirs.other)?;
            }
        }
        if key == "tvbox_endpoint" {
            if let Some(url) = value.as_str() {
                let url = url.trim();
                if !url.is_empty()
                    && (!url.to_ascii_lowercase().starts_with("http://")
                        || url.chars().any(|ch| ch.is_control())
                        || url.contains('\\'))
                {
                    return Err("TVBox 地址必须是局域网 HTTP 地址".into());
                }
            }
        }
        if matches!(
            key,
            "proxy_url"
                | "default_referer"
                | "default_origin"
                | "allowed_hosts"
                | "default_user_agent"
                | "tvbox_endpoint"
                | "proxy_mode"
                | "proxy_bypass"
                | "queue_active_days"
                | "legal_terms_version"
        ) {
            if let Some(text) = value.as_str() {
                if text.chars().any(|ch| ch.is_control()) {
                    return Err("设置值不能包含控制字符".into());
                }
            }
        }
        if key == "proxy_mode" {
            if let Some(mode) = value.as_str() {
                if !matches!(mode, "direct" | "manual" | "system" | "") {
                    return Err("代理模式无效".into());
                }
            }
        }
        if key == "bt_max_connections" {
            let value = value
                .as_u64()
                .ok_or_else(|| "BT 最大连接数必须是整数".to_string())?;
            if !(10..=1000).contains(&value) {
                return Err("BT 最大连接数必须在 10 到 1000 之间".into());
            }
        }
        if key == "bt_upload_limit_kib" {
            let value = value
                .as_u64()
                .ok_or_else(|| "BT 上传限制必须是整数".to_string())?;
            if value > 1_048_576 {
                return Err("BT 上传限制不能超过 1048576 KiB/s".into());
            }
        }
        if key == "download_hourly_quota_mib" {
            let value = value
                .as_u64()
                .ok_or_else(|| "每小时流量配额必须是整数".to_string())?;
            if value > 1_048_576 {
                return Err("每小时流量配额不能超过 1048576 MiB".into());
            }
        }
        if key == "queue_profiles" {
            validate_queue_profiles(value)?;
        }
        if key == "default_origin" {
            if let Some(origin) = value.as_str() {
                let origin = origin.trim().to_ascii_lowercase();
                if !origin.is_empty()
                    && !(origin.starts_with("http://") || origin.starts_with("https://"))
                {
                    return Err("默认 Origin 必须是 HTTP(S) 地址".into());
                }
            }
        }
        if key == "allowed_hosts" {
            if let Some(hosts) = value.as_str() {
                if hosts.chars().any(char::is_control)
                    || hosts
                        .split([',', ';'])
                        .any(|item| item.trim().contains("//"))
                {
                    return Err("允许的域名列表无效".into());
                }
            }
        }
        if key == "site_rules" {
            let raw = value
                .as_str()
                .ok_or_else(|| "站点规则必须是文本".to_string())?;
            crate::site_rules::validate_site_rules(raw)?;
        }
        if key == "task_column_layout" {
            validate_ui_layout(
                value,
                &["name", "progress", "status", "speed", "size", "actions"],
                true,
                "name",
            )?;
        }
        if key == "toolbar_actions" {
            validate_ui_layout(
                value,
                &[
                    "new",
                    "paste",
                    "batch",
                    "harvest",
                    "start_all",
                    "pause_all",
                    "cast",
                    "tvbox",
                    "extension",
                ],
                false,
                "new",
            )?;
        }
        if key == "task_sort" {
            let sort = value
                .as_str()
                .ok_or_else(|| "任务排序设置必须是文本".to_string())?;
            let (field, direction) = sort.split_once(':').unwrap_or((sort, "asc"));
            if !matches!(
                field,
                "queue" | "name" | "progress" | "status" | "speed" | "size"
            ) || !matches!(direction, "asc" | "desc")
            {
                return Err("任务排序设置无效".into());
            }
        }
        Ok(())
    }

    pub fn store_credential(
        &self,
        credential_ref: &str,
        protected_blob: &str,
        kind: &str,
    ) -> Result<(), String> {
        self.lock()?
            .store_mut()
            .store_credential(credential_ref, protected_blob, kind)
    }

    pub fn load_credential(&self, credential_ref: &str) -> Result<Option<String>, String> {
        self.lock()?.store().load_credential(credential_ref)
    }

    pub fn default_cookie_configured(&self) -> Result<bool, String> {
        Ok(self
            .load_credential(DEFAULT_COOKIE_CREDENTIAL_REF)?
            .is_some_and(|value| !value.is_empty()))
    }

    pub fn set_default_cookie(&self, cookie: &str) -> Result<(), String> {
        if cookie.len() > 16 * 1024 || cookie.contains(['\r', '\n', '\0']) {
            return Err("默认 Cookie 格式无效或长度超过 16 KiB".into());
        }
        let protected = if cookie.trim().is_empty() {
            String::new()
        } else {
            let replay = serde_json::json!({ "cookie": cookie.trim() }).to_string();
            CredentialVault.protect(&replay)?
        };
        self.store_credential(DEFAULT_COOKIE_CREDENTIAL_REF, &protected, "default_cookie")
    }

    pub fn set_site_rule_credential(
        &self,
        host: &str,
        cookie: &str,
        request_headers: &BTreeMap<String, String>,
        clear: bool,
    ) -> Result<(), String> {
        let normalized_host = host.trim().to_ascii_lowercase();
        if normalized_host.is_empty() || normalized_host.len() > 255 {
            return Err("站点规则域名无效".into());
        }
        if cookie.len() > 16 * 1024 || cookie.contains(['\r', '\n', '\0']) {
            return Err("站点 Cookie 格式无效或长度超过 16 KiB".into());
        }
        if request_headers.len() > 64
            || request_headers.iter().any(|(name, value)| {
                name.len() > 128 || value.len() > 16 * 1024 || !header_value_allowed(name, value)
            })
        {
            return Err("站点自定义请求头无效".into());
        }
        let raw = self.lock()?.store().setting_string("site_rules", "")?;
        let mut rules = crate::parse_site_rules(&raw);
        let rule = rules
            .iter_mut()
            .find(|rule| rule.host.trim().eq_ignore_ascii_case(&normalized_host))
            .ok_or_else(|| "保存凭据前请先保存站点规则".to_string())?;
        if clear {
            if !rule.credential_ref.trim().is_empty() {
                self.lock()?
                    .store_mut()
                    .delete_credential(&rule.credential_ref)?;
            }
            rule.credential_ref.clear();
        } else {
            let replay = serde_json::json!({
                "cookie": cookie.trim(),
                "request_headers": request_headers,
            })
            .to_string();
            let protected = if cfg!(windows) {
                CredentialVault.protect(&replay)?
            } else {
                replay
            };
            let credential_ref = crate::site_rules::credential_ref_for_host(&normalized_host);
            self.store_credential(&credential_ref, &protected, "site_rule")?;
            rule.credential_ref = credential_ref;
        }
        self.set_setting(
            "site_rules",
            serde_json::json!(crate::format_site_rules(&rules)),
        )
    }

    pub fn save_handoff(
        &self,
        handoff_id: &str,
        handoff_json: &str,
        status: &str,
        task_id: Option<&str>,
        created_at_ms: u64,
    ) -> Result<(), String> {
        self.lock()?.store_mut().save_handoff(
            handoff_id,
            handoff_json,
            status,
            task_id,
            created_at_ms,
        )
    }

    pub fn load_handoffs(&self) -> Result<Vec<String>, String> {
        self.lock()?.store().load_handoffs()
    }

    fn request_media_push(&self, request: MediaPushRequest) -> Result<Vec<EventEnvelope>, String> {
        if request.id.trim().is_empty() || request.id.len() > 160 {
            return Err("媒体推送请求编号无效".into());
        }
        if !matches!(request.push_kind.as_str(), "cast" | "tvbox") {
            return Err("媒体推送类型无效".into());
        }
        let lower = request.url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://"))
            || request.url.chars().any(char::is_control)
        {
            return Err("媒体推送地址无效".into());
        }
        let json = serde_json::to_string(&request)
            .map_err(|error| format!("encode media push {}: {error}", request.id))?;
        self.save_handoff(
            &request.id,
            &json,
            &request.status,
            None,
            request.created_at_ms,
        )?;
        self.lock()?.emit(CoreEvent::MediaPushRequested { request })
    }

    fn resolve_media_push(
        &self,
        request_id: &str,
        status: &str,
        message: &str,
        location: &str,
    ) -> Result<Vec<EventEnvelope>, String> {
        if !matches!(status, "done" | "failed" | "canceled") {
            return Err("媒体推送结果状态无效".into());
        }
        let mut request = self
            .load_handoffs()?
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<MediaPushRequest>(&encoded).ok())
            .find(|item| item.id == request_id)
            .ok_or_else(|| "媒体推送请求不存在或已过期".to_string())?;
        request.status = status.to_string();
        request.message = message.trim().chars().take(300).collect();
        request.location = location.trim().chars().take(2048).collect();
        let json = serde_json::to_string(&request)
            .map_err(|error| format!("encode media push {}: {error}", request.id))?;
        self.save_handoff(
            &request.id,
            &json,
            &request.status,
            None,
            request.created_at_ms,
        )?;
        self.lock()?.emit(CoreEvent::MediaPushResolved { request })
    }

    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, PersistentCore>, String> {
        self.core
            .lock()
            .map_err(|_| "v7 Core mutex poisoned".to_string())
    }

    pub fn tasks(&self) -> Result<Vec<crate::TaskSnapshot>, String> {
        self.refresh_output_flags()?;
        let core = self
            .core
            .lock()
            .map_err(|_| "v7 Core mutex poisoned".to_string())?;
        let mut tasks = core.tasks();
        for task in &mut tasks {
            if let Some(spec) = core.task_spec(&task.task_id) {
                if let Ok(paths) = TaskPaths::for_task(&task.task_id, spec) {
                    task.output_path = resolve_published(&paths).to_string_lossy().into_owned();
                }
            }
        }
        Ok(tasks)
    }

    fn refresh_output_flags(&self) -> Result<(), String> {
        let completed: Vec<String> = self
            .lock()?
            .tasks()
            .into_iter()
            .filter(|task| matches!(task.status.as_str(), "completed" | "done"))
            .map(|task| task.task_id)
            .collect();
        for task_id in completed {
            let missing = self
                .lock()?
                .task_spec(&task_id)
                .cloned()
                .and_then(|spec| TaskPaths::for_task(&task_id, &spec).ok())
                .map(|paths| !resolve_published(&paths).exists())
                .unwrap_or(false);
            self.lock()?.mark_output_missing(&task_id, missing)?;
        }
        Ok(())
    }

    fn save_site_profile(&self, task_id: &str) -> Result<Vec<EventEnvelope>, String> {
        let spec = self
            .lock()?
            .task_spec(task_id)
            .cloned()
            .ok_or_else(|| format!("unknown task {task_id}"))?;
        let raw = self.lock()?.store().setting_string("site_rules", "")?;
        let mut rules = crate::parse_site_rules(&raw);
        crate::upsert_site_rule(
            &mut rules,
            crate::SiteRule {
                host: crate::site_rules::host_of(&spec.url),
                speed_limit_kib: spec.speed_limit_kib,
                concurrency: spec.concurrency,
                proxy: spec.proxy.clone(),
                referer: spec
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("referer"))
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default(),
                origin: spec
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("origin"))
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default(),
                user_agent: spec
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default(),
                credential_ref: spec.credential_ref.clone().unwrap_or_default(),
                ..Default::default()
            },
        );
        let encoded = crate::format_site_rules(&rules);
        self.set_setting("site_rules", serde_json::json!(encoded))?;
        self.lock()?.emit(CoreEvent::Toast {
            level: "site_profile".into(),
            message: format!(
                "已保存 {} 的站点规则",
                crate::site_rules::host_of(&spec.url)
            ),
        })
    }

    pub fn dispatch(&self, command: CoreCommand) -> Result<Vec<EventEnvelope>, String> {
        if matches!(&command, CoreCommand::Shutdown) {
            // A process exit is also an update boundary: stop workers and wait
            // for their latest resume/checkpoint state before closing IPC.
            self.prepare_for_update(Duration::from_secs(8))?;
            return self.dispatch_inner(command);
        }
        if let CoreCommand::ClearCompleted = command {
            return self.dispatch_inner(command);
        }
        if let CoreCommand::SaveSiteProfile { task_id } = command {
            return self.save_site_profile(&task_id);
        }
        if let CoreCommand::ImportPaths { paths } = command {
            let mut events = Vec::new();
            for path in paths {
                events.extend(self.dispatch(CoreCommand::CreateTask {
                    spec: TaskSpec {
                        url: path,
                        ..Default::default()
                    },
                })?);
            }
            return Ok(events);
        }
        if let CoreCommand::ImportCurl { command, options } = command {
            return self.import_curl_command(&command, options);
        }
        if let CoreCommand::ExportTasks { task_ids, format } = command {
            let (format, data, task_count) =
                crate::task_export::export_tasks(&self.tasks()?, &task_ids, &format)?;
            return self.lock()?.emit(CoreEvent::TaskExport {
                format,
                data,
                task_count,
            });
        }
        if let CoreCommand::HarvestPage {
            url,
            referer,
            probe_urls,
        } = command
        {
            return harvest_page(self, &url, &referer, &probe_urls);
        }
        if let CoreCommand::RefreshTaskRequest {
            task_id,
            url,
            cookie,
            auto_resume,
        } = command
        {
            return refresh_task_request(self, &task_id, &url, &cookie, auto_resume);
        }
        if let CoreCommand::ProbeTorrent { source } = command {
            return probe_torrent_command(self, &source);
        }
        if let CoreCommand::SelectTorrentFiles { source, selections } = command {
            return select_torrent_files_command(self, &source, &selections);
        }
        if let CoreCommand::GetTaskTorrentFiles { task_id } = command {
            return task_torrent_files(self, &task_id);
        }
        if let CoreCommand::SetTaskTorrentFiles {
            task_id,
            selections,
        } = command
        {
            return set_task_torrent_files(self, &task_id, &selections);
        }
        if let CoreCommand::AcceptHandoff {
            handoff_id,
            filename,
            download_dir,
            trusted_ui,
        } = command
        {
            return self.accept_handoff_command(handoff_id, filename, download_dir, trusted_ui);
        }
        if let CoreCommand::RejectHandoff {
            handoff_id,
            suppress_site_kind,
        } = command
        {
            if suppress_site_kind {
                self.persist_handoff_suppression(&handoff_id)?;
            }
            return self.dispatch_inner(CoreCommand::RejectHandoff {
                handoff_id,
                suppress_site_kind,
            });
        }
        if let CoreCommand::PresentHandoff {
            handoff_id,
            ok,
            presenter_id,
        } = command
        {
            return self.present_handoff_command(handoff_id, ok, presenter_id);
        }
        if let CoreCommand::RequestMediaPush { request } = command {
            return self.request_media_push(request);
        }
        if let CoreCommand::ResolveMediaPush {
            request_id,
            status,
            message,
            location,
        } = command
        {
            return self.resolve_media_push(&request_id, &status, &message, &location);
        }
        if let CoreCommand::AssignQueue { task_ids, queue_id } = command {
            let queue_id = queue_id.trim().to_string();
            if task_ids.is_empty() || task_ids.len() > 10_000 {
                return Err("请选择要移动的任务".into());
            }
            if !self
                .settings()?
                .queue_profiles
                .iter()
                .any(|profile| profile.id == queue_id)
            {
                return Err("目标队列不存在".into());
            }
            let events = self.dispatch_inner(CoreCommand::AssignQueue { task_ids, queue_id })?;
            self.start_next_queued()?;
            return Ok(events);
        }
        if let CoreCommand::ControlCast { action } = command {
            let playback = crate::cast::control_session(&action)?;
            if action == "stop" {
                clear_cast_mount();
            }
            return self.lock()?.emit(CoreEvent::CastSession {
                active: action != "stop",
                title: String::new(),
                device: playback.label,
                status: match action.as_str() {
                    "pause" => "已暂停投屏".into(),
                    "play" => "继续投屏".into(),
                    "stop" => "已停止投屏".into(),
                    "status" => playback.state.clone(),
                    "seek_back" => "已后退 10 秒".into(),
                    "seek_forward" => "已快进 10 秒".into(),
                    _ if action.starts_with("seek_to:") => "已跳转".into(),
                    _ if action.starts_with("seek:") => "已调整播放位置".into(),
                    _ => action,
                },
                task_id: String::new(),
                media_url: String::new(),
                device_kind: playback.device_kind,
                supported_actions: playback.supported_actions,
                playing: playback.playing,
                paused: playback.paused,
                position_seconds: playback.position_seconds,
                duration_seconds: playback.duration_seconds,
                position_available: playback.position_available,
            });
        }
        if let CoreCommand::CreateTask { spec } = command {
            self.require_legal()?;
            let spec = self.apply_defaults_to_spec(spec)?;
            let spec = validate_torrent_spec(spec, self.settings()?.bt_enable_dht)?;
            let mut events = Vec::new();
            for spec in self.expand_create(spec)? {
                if !spec.allow_duplicate {
                    if let Some((task_id, status)) = self.duplicate_of(&spec)? {
                        events.extend(self.reuse_duplicate(task_id, &status)?);
                        continue;
                    }
                }
                events.extend(self.dispatch_created(spec)?);
            }
            self.start_next_queued()?;
            return Ok(events);
        }
        self.dispatch_inner(command)
    }

    fn expand_create(&self, spec: TaskSpec) -> Result<Vec<TaskSpec>, String> {
        let (auto, dirs) = {
            let core = self.lock()?;
            (
                core.store().setting_bool("auto_category_dirs", false)?,
                crate::category::parse_category_dirs(
                    &core.store().setting_string("browser_category_dirs", "")?,
                ),
            )
        };
        if let Some(imported) = crate::task_export::import_tasks_from_source(&spec.url)? {
            return imported
                .into_iter()
                .map(|mut imported| {
                    imported.allow_duplicate = spec.allow_duplicate;
                    self.apply_defaults_to_spec(imported)
                })
                .collect();
        }
        if let Some(urls) = crate::link_file::expand_source(&spec.url)? {
            let mut specs = Vec::new();
            for url in urls {
                if reject_task_url(&url).is_err() {
                    continue;
                }
                if crate::looks_like_metalink(&url) {
                    specs.extend(specs_from_metalink(&url, &spec, auto, &dirs)?);
                } else {
                    specs.push(spec_from_url(&spec, &url, &spec.filename, auto, &dirs));
                }
            }
            if specs.is_empty() {
                return Err("本地文件里没有可下载链接".into());
            }
            return Ok(specs);
        }
        if spec.harvest {
            let (_, body) =
                crate::fetch_bytes(&spec.url, &std::collections::HashMap::new(), &spec.proxy)
                    .map_err(|error| error.to_string())?;
            let text = String::from_utf8_lossy(&body);
            if crate::looks_like_metalink(&text) {
                return specs_from_metalink(&text, &spec, auto, &dirs);
            }
            let links = crate::harvest_html(&text, &spec.url);
            if links.is_empty() {
                return Err("页面没有可下载链接".into());
            }
            return Ok(links
                .into_iter()
                .filter(|link| reject_task_url(&link.url).is_ok())
                .map(|link| spec_from_url(&spec, &link.url, &link.filename, auto, &dirs))
                .collect());
        }
        if crate::looks_like_metalink(&spec.url) {
            return specs_from_metalink(&spec.url, &spec, auto, &dirs);
        }
        let lower = spec.url.to_ascii_lowercase();
        if lower.ends_with(".meta4") || lower.ends_with(".metalink") {
            let (_, body) =
                crate::fetch_bytes(&spec.url, &std::collections::HashMap::new(), &spec.proxy)
                    .map_err(|error| error.to_string())?;
            return specs_from_metalink(&String::from_utf8_lossy(&body), &spec, auto, &dirs);
        }
        Ok(vec![spec_from_url(
            &spec,
            &spec.url,
            &spec.filename,
            auto,
            &dirs,
        )])
    }

    fn dispatch_created(&self, spec: TaskSpec) -> Result<Vec<EventEnvelope>, String> {
        let spec = seal_spec_secrets(self, spec)?;
        self.dispatch_inner(CoreCommand::CreateTask { spec })
    }

    fn import_curl_command(
        &self,
        command: &str,
        mut spec: TaskSpec,
    ) -> Result<Vec<EventEnvelope>, String> {
        let parsed = crate::parse_curl_command(command)?
            .ok_or_else(|| "输入内容不是 cURL 命令".to_string())?;
        let mut request_headers = parsed.headers;
        request_headers.extend(spec.headers);
        spec.headers = BTreeMap::new();
        let mut replay = serde_json::Map::new();
        if !parsed.body.is_empty() {
            replay.insert(
                "request_body".into(),
                Value::String(crate::curl_import::base64_encode(parsed.body.as_bytes())),
            );
        }
        let referer = take_header(&mut request_headers, "referer")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(parsed.referer);
        if !referer.is_empty() {
            replay.insert("referer".into(), Value::String(referer));
        }
        let origin = take_header(&mut request_headers, "origin")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(parsed.origin);
        if !origin.is_empty() {
            replay.insert("origin".into(), Value::String(origin));
        }
        let cookie = take_header(&mut request_headers, "cookie")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(parsed.cookie);
        if !cookie.is_empty() {
            replay.insert("cookie".into(), Value::String(cookie));
        }
        let user_agent = take_header(&mut request_headers, "user-agent")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(parsed.user_agent);
        if !user_agent.is_empty() {
            replay.insert("user_agent".into(), Value::String(user_agent));
        }
        if !request_headers.is_empty() {
            replay.insert(
                "request_headers".into(),
                serde_json::to_value(request_headers)
                    .map_err(|error| format!("编码 cURL 请求头失败: {error}"))?,
            );
        }
        let credential_ref = if replay.is_empty() {
            None
        } else {
            let json = Value::Object(replay).to_string();
            let blob = if cfg!(windows) {
                CredentialVault.protect(&json)?
            } else {
                json
            };
            let credential_ref = format!(
                "curl-{:x}-{:x}",
                simple_hash(&parsed.url),
                simple_hash(command)
            );
            self.store_credential(&credential_ref, &blob, "browser_replay")?;
            Some(credential_ref)
        };
        spec.url = parsed.url;
        spec.request_method = parsed.method;
        spec.credential_ref = credential_ref;
        self.dispatch(CoreCommand::CreateTask { spec })
    }

    fn worker_is_active(&self, task_id: &str) -> Result<bool, String> {
        Ok(self
            .active
            .lock()
            .map_err(|_| "v7 worker registry poisoned".to_string())?
            .contains(task_id))
    }

    fn cancel_and_wait(&self, task_id: &str) -> Result<(), String> {
        if !self.worker_is_active(task_id)? {
            return Ok(());
        }
        self.dispatch_inner(CoreCommand::TaskAction {
            task_id: task_id.to_string(),
            action: "cancel".into(),
        })?;
        self.wait_for_worker_stop(task_id)
    }

    fn wait_for_worker_stop(&self, task_id: &str) -> Result<(), String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while self.worker_is_active(task_id)? {
            #[cfg(test)]
            if let Ok(mut sender) = self.worker_wait_started.lock() {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(());
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("等待下载 worker 停止超时".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Ok(())
    }

    fn dispatch_inner(&self, command: CoreCommand) -> Result<Vec<EventEnvelope>, String> {
        if let CoreCommand::TaskAction { task_id, action } = &command {
            if matches!(action.as_str(), "start" | "resume" | "retry") {
                self.require_legal()?;
            }
            if action == "open" {
                return open_completed(self, task_id, false).map(|_| Vec::new());
            }
            if action == "open_folder" {
                return open_completed(self, task_id, true).map(|_| Vec::new());
            }
            if action == "launch" {
                return open_completed(self, task_id, false).map(|_| Vec::new());
            }
            if action == "copy_file" {
                return copy_completed_file(self, task_id);
            }
            if action == "drag_file" {
                return drag_completed_file(self, task_id);
            }
            if let Some(limit) = action.strip_prefix("speed:") {
                return set_task_speed(self, task_id, limit.parse().unwrap_or(0));
            }
            if let Some(url) = action.strip_prefix("refresh:") {
                let next_action = self
                    .tasks()?
                    .into_iter()
                    .find(|task| task.task_id == *task_id)
                    .and_then(|task| {
                        ["resume", "retry", "start"].into_iter().find(|candidate| {
                            task.available_actions.iter().any(|item| item == candidate)
                        })
                    });
                let mut events = refresh_task_url(self, task_id, url)?;
                if let Some(next_action) = next_action {
                    events.extend(self.dispatch_inner(CoreCommand::TaskAction {
                        task_id: task_id.clone(),
                        action: next_action.into(),
                    })?);
                }
                return Ok(events);
            }
            if action == "push_tvbox" {
                return push_task_tvbox(self, task_id);
            }
            if action == "queue_top" {
                return self.dispatch_inner(CoreCommand::PlaceQueue {
                    task_id: task_id.clone(),
                    before_id: "^".into(),
                });
            }
            if action == "queue_bottom" {
                return self.dispatch_inner(CoreCommand::PlaceQueue {
                    task_id: task_id.clone(),
                    before_id: String::new(),
                });
            }
            if action == "queue_up" {
                return self.dispatch_inner(CoreCommand::ReorderQueue {
                    task_id: task_id.clone(),
                    delta: -1,
                });
            }
            if action == "queue_down" {
                return self.dispatch_inner(CoreCommand::ReorderQueue {
                    task_id: task_id.clone(),
                    delta: 1,
                });
            }
            if action == "delete_files" {
                self.cancel_and_wait(task_id)?;
                self.delete_task_files(task_id)?;
                return self.dispatch_inner(CoreCommand::TaskAction {
                    task_id: task_id.clone(),
                    action: "delete".into(),
                });
            }
            if action == "delete" {
                self.cancel_and_wait(task_id)?;
            }
        }
        if let CoreCommand::SetSetting { key, value } = command {
            self.set_setting(&key, value)?;
            return Ok(Vec::new());
        }
        if let CoreCommand::PlayTask { task_id } = &command {
            return play_task(self, task_id);
        }
        if let CoreCommand::CastTask { task_id } = &command {
            return cast_task(self, task_id);
        }
        if let CoreCommand::CastToDevice { task_id, device_id } = &command {
            return cast_to_device(self, task_id, device_id);
        }
        if let CoreCommand::ShareMedia {
            path,
            url,
            title,
            device_id,
        } = &command
        {
            return share_media(self, path, url, title, device_id);
        }
        if let CoreCommand::PlayerControl { action } = &command {
            return player_control_events(self, action);
        }
        if let CoreCommand::ProbeUrl { url, spec } = &command {
            return probe_command(self, url, spec.as_ref());
        }
        if let CoreCommand::DiscoverCastDevices { mode } = command {
            return discover_cast(self, &mode);
        }
        if let CoreCommand::DownloadUpdate = command {
            return download_update(self);
        }
        if let CoreCommand::InstallUpdate { workbench_pid } = command {
            return install_update(self, workbench_pid);
        }
        if let CoreCommand::OpenCompleted { task_id, folder } = &command {
            return open_completed(self, task_id, *folder).map(|_| Vec::new());
        }
        if matches!(command, CoreCommand::ConfirmPowerAction) {
            let confirmed = crate::power_action::confirm()?;
            return self.lock()?.emit(CoreEvent::Toast {
                level: if confirmed { "success" } else { "info" }.into(),
                message: if confirmed {
                    "已执行完成后电源动作"
                } else {
                    "没有待执行的电源动作"
                }
                .into(),
            });
        }
        if matches!(command, CoreCommand::CancelPowerAction) {
            let canceled = crate::power_action::cancel();
            return self.lock()?.emit(CoreEvent::Toast {
                level: "info".into(),
                message: if canceled {
                    "已取消完成后电源动作".into()
                } else {
                    "没有待执行的电源动作".into()
                },
            });
        }
        let (task_id, start_action) = match &command {
            CoreCommand::TaskAction { task_id, action } => {
                let start_action =
                    matches!(action.as_str(), "start" | "resume" | "retry").then(|| action.clone());
                (Some(task_id.clone()), start_action)
            }
            _ => (None, None),
        };
        if let (Some(task_id), Some(action @ ("resume" | "retry"))) =
            (task_id.as_deref(), start_action.as_deref())
        {
            let action_allowed = self
                .lock()?
                .tasks()
                .iter()
                .find(|task| task.task_id == task_id)
                .is_some_and(|task| task.available_actions.iter().any(|item| item == action));
            if action_allowed && matches!(action, "resume" | "retry") {
                self.wait_for_worker_stop(task_id)?;
            }
        }
        if let (Some(task_id), Some(action)) = (task_id.as_deref(), start_action.as_deref()) {
            let mut core = self.lock()?;
            let queue_profiles = load_queue_profiles(core.store())?;
            let deferred = core
                .tasks()
                .iter()
                .find(|task| task.task_id == task_id)
                .filter(|task| task.available_actions.iter().any(|item| item == action))
                .filter(|task| {
                    !task_schedule_allowed(task)
                        || !queue_profiles
                            .iter()
                            .find(|profile| profile.id == task.queue_id)
                            .is_some_and(queue_profile_allowed)
                })
                .map(|task| (task.downloaded_bytes, task.total_bytes));
            if let Some((downloaded_bytes, total_bytes)) = deferred {
                return core.handle(CoreCommand::UpdateProgress {
                    task_id: task_id.into(),
                    downloaded_bytes,
                    total_bytes,
                    speed_bytes_per_sec: 0,
                    stage: "waiting".into(),
                    status: "queued".into(),
                });
            }
        }
        let mut start_accepted = false;
        let events = {
            let mut core = self
                .core
                .lock()
                .map_err(|_| "v7 Core mutex poisoned".to_string())?;
            if let Some(task_id) = task_id.as_deref() {
                if let Some(action) = start_action.as_deref() {
                    start_accepted = core
                        .tasks()
                        .iter()
                        .find(|task| task.task_id == task_id)
                        .is_some_and(|task| {
                            task.available_actions.iter().any(|item| item == action)
                        });
                }
                if start_accepted {
                    if let Some(spec) = core.task_spec(task_id).cloned() {
                        let paths = TaskPaths::for_task(task_id, &spec)?;
                        paths.prepare()?;
                        paths.set_control("run")?;
                    }
                }
                if let CoreCommand::TaskAction { action, .. } = &command {
                    if matches!(action.as_str(), "pause" | "cancel") {
                        if let Some(spec) = core.task_spec(task_id).cloned() {
                            let paths = TaskPaths::for_task(task_id, &spec)?;
                            paths.set_control(if action == "pause" {
                                "pause"
                            } else {
                                "cancel"
                            })?;
                        }
                    }
                    if matches!(action.as_str(), "delete" | "delete_file") {
                        crate::net_policy::clear_scoped_limit(&format!("task:{task_id}"));
                    }
                }
            }
            core.handle(command)?
        };
        if start_accepted {
            if let Some(task_id) = task_id {
                self.spawn(task_id)?;
            }
        }
        Ok(events)
    }

    pub(crate) fn start_next_queued(&self) -> Result<(), String> {
        if self.update_shutdown.load(Ordering::SeqCst) {
            return Ok(());
        }
        if self.require_legal().is_err() {
            return Ok(());
        }
        loop {
            let active_ids = self
                .active
                .lock()
                .map_err(|_| "download worker registry poisoned".to_string())?
                .clone();
            let tasks = self.tasks()?;
            let mut profiles = self.settings()?.queue_profiles;
            profiles.sort_by_key(|profile| std::cmp::Reverse(profile.priority));
            let next = profiles.into_iter().find_map(|profile| {
                if !queue_profile_allowed(&profile) {
                    return None;
                }
                let active_count = tasks
                    .iter()
                    .filter(|task| {
                        task.queue_id == profile.id && active_ids.contains(&task.task_id)
                    })
                    .count();
                if active_count >= profile.max_active.max(1) as usize {
                    return None;
                }
                let mut queued: Vec<_> = tasks
                    .iter()
                    .filter(|task| {
                        task.queue_id == profile.id
                            && task.status == "queued"
                            && !active_ids.contains(&task.task_id)
                            && task_schedule_allowed(task)
                    })
                    .collect();
                queued.sort_by_key(|task| (task.queue_index, task.task_id.clone()));
                queued.first().map(|task| task.task_id.clone())
            });
            let Some(next) = next else {
                return Ok(());
            };
            self.spawn(next)?;
        }
    }

    pub fn recover_startup(&self) -> Result<(), String> {
        let resume = self
            .lock()?
            .store()
            .setting_bool("resume_interrupted_on_startup", false)?;
        let tasks = self.tasks()?;
        for task in tasks {
            if matches!(
                task.status.as_str(),
                "downloading" | "recording" | "merging" | "checking"
            ) {
                let status = if resume { "queued" } else { "paused" };
                mark_progress(
                    &self.core,
                    &task.task_id,
                    task.downloaded_bytes,
                    task.total_bytes,
                    "waiting",
                    status,
                )?;
            }
        }
        if resume {
            self.start_next_queued()?;
        }
        Ok(())
    }

    pub fn pause_active_tasks(&self) -> Result<(), String> {
        let tasks = self.tasks()?;
        for task in tasks {
            if matches!(
                task.status.as_str(),
                "downloading" | "recording" | "merging" | "checking"
            ) {
                self.dispatch_inner(CoreCommand::TaskAction {
                    task_id: task.task_id,
                    action: "pause".into(),
                })?;
            }
        }
        Ok(())
    }

    pub fn prepare_for_update(&self, timeout: Duration) -> Result<(), String> {
        self.update_shutdown.store(true, Ordering::SeqCst);
        if let Err(error) = self.pause_active_tasks() {
            self.update_shutdown.store(false, Ordering::SeqCst);
            return Err(error);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let active = self
                .active
                .lock()
                .map_err(|_| "下载任务注册表已损坏".to_string())?
                .len();
            if active == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.update_shutdown.store(false, Ordering::SeqCst);
                return Err(format!(
                    "仍有 {active} 个任务未完成断点保存，已取消升级；请稍后重试"
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn apply_defaults_to_spec(&self, mut spec: TaskSpec) -> Result<TaskSpec, String> {
        let client_dir = spec.download_dir.trim().to_string();
        spec = apply_site_rules_to_spec(&self.core(), spec)?;
        let settings = self.settings()?;
        if spec.queue_id.trim().is_empty() {
            spec.queue_id = crate::DEFAULT_QUEUE_ID.into();
        }
        if !settings
            .queue_profiles
            .iter()
            .any(|profile| profile.id == spec.queue_id)
        {
            return Err(format!("任务所属队列不存在: {}", spec.queue_id));
        }
        if !crate::net_policy::schedule_value_valid(&spec.scheduled_start_at)
            || !crate::net_policy::schedule_value_valid(&spec.scheduled_stop_at)
        {
            return Err("任务计划时间必须使用 HH:mm 或 RFC 3339 时间".into());
        }
        if spec.download_dir.trim().is_empty() {
            spec.download_dir = settings.download_dir.clone();
        }
        if spec.work_dir.trim().is_empty() {
            spec.work_dir = if settings.temp_dir.trim().is_empty() {
                spec.download_dir.clone()
            } else {
                settings.temp_dir.clone()
            };
        }
        reject_path_escape(&spec.work_dir)?;
        if client_dir.is_empty() {
            reject_path_escape(&spec.download_dir)?;
        } else {
            spec.download_dir =
                constrain_untrusted_download_dir(&client_dir, &settings.download_dir)?;
        }
        if !spec.body_path.trim().is_empty() {
            reject_path_escape(&spec.body_path)?;
            if !Path::new(&spec.body_path).is_file() {
                return Err("请求体文件不存在".into());
            }
        }
        if spec.proxy.trim().is_empty() {
            spec.proxy = settings.proxy_url.clone();
        }
        spec.headers
            .retain(|key, value| header_value_allowed(key, value));
        spec.request_method = crate::http_engine::sanitize_http_method(&spec.request_method);
        if !header_value_allowed("ETag", &spec.etag) {
            spec.etag.clear();
        }
        if !header_value_allowed("Last-Modified", &spec.last_modified) {
            spec.last_modified.clear();
        }
        spec.mirrors
            .retain(|url| crate::http_engine::http_fetch_url_allowed(url));
        if spec.proxy != crate::net_policy::DIRECT_PROXY_SENTINEL && !proxy_url_allowed(&spec.proxy)
        {
            return Err("代理地址无效".into());
        }
        spec.proxy = crate::net_policy::effective_proxy(
            &settings.proxy_mode,
            &settings.proxy_url,
            &settings.proxy_bypass,
            &spec.url,
            &spec.proxy,
        );
        reject_task_url(&spec.url)?;
        if !crate::net_policy::url_allowed(&spec.url, &settings.allowed_hosts) {
            return Err("下载地址不在允许的域名范围内".into());
        }
        if spec.concurrency == 0 {
            spec.concurrency = settings.default_concurrency.max(1) as u32;
        }
        if spec.allow_duplicate {
            // caller asked to keep a second copy
        } else if settings.allow_duplicate {
            spec.allow_duplicate = true;
        }
        if !settings.ffmpeg_path.trim().is_empty() {
            validate_helper_executable(&settings.ffmpeg_path, &["ffmpeg", "ffmpeg.exe"])?;
            std::env::set_var("HLS_FFMPEG", &settings.ffmpeg_path);
        }
        if !settings.default_user_agent.trim().is_empty()
            && header_value_allowed("User-Agent", &settings.default_user_agent)
            && !spec
                .headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("user-agent"))
        {
            spec.headers
                .insert("User-Agent".into(), settings.default_user_agent);
        }
        if !settings.default_referer.trim().is_empty()
            && header_value_allowed("Referer", &settings.default_referer)
            && !spec
                .headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("referer"))
        {
            spec.headers
                .insert("Referer".into(), settings.default_referer);
        }
        if !settings.default_origin.trim().is_empty()
            && header_value_allowed("Origin", &settings.default_origin)
            && !spec
                .headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("origin"))
        {
            spec.headers
                .insert("Origin".into(), settings.default_origin);
        }
        if spec.credential_ref.is_none() && self.default_cookie_configured()? {
            spec.credential_ref = Some(DEFAULT_COOKIE_CREDENTIAL_REF.into());
        }
        Ok(spec)
    }

    fn duplicate_of(&self, spec: &TaskSpec) -> Result<Option<(String, String)>, String> {
        let want = crate::duplicate::canonicalize_url(&spec.url);
        if want.is_empty() {
            return Ok(None);
        }
        let core = self.lock()?;
        for task in core.tasks() {
            if let Some(stored) = core.task_spec(&task.task_id) {
                if crate::duplicate::canonicalize_url(&stored.url) == want {
                    return Ok(Some((task.task_id, task.status)));
                }
            }
        }
        Ok(None)
    }

    fn reuse_duplicate(&self, task_id: String, status: &str) -> Result<Vec<EventEnvelope>, String> {
        let output_missing = self
            .lock()?
            .task_spec(&task_id)
            .and_then(|spec| TaskPaths::for_task(&task_id, spec).ok())
            .map(|paths| !paths.final_output.exists())
            .unwrap_or(true);
        let action = crate::duplicate::suggest_duplicate_action(status, output_missing);
        match action {
            "resume" | "retry" | "start" => {
                let events = self.dispatch_inner(CoreCommand::TaskAction {
                    task_id: task_id.clone(),
                    action: action.into(),
                })?;
                self.start_next_queued()?;
                Ok(events)
            }
            other => self.lock()?.emit(CoreEvent::DuplicateOffered {
                task_id,
                action: other.into(),
                output_missing,
                message: format!("已有相同链接任务（{status} / {other}）"),
            }),
        }
    }

    fn require_legal(&self) -> Result<(), String> {
        let settings = self.settings()?;
        if settings.legal_accepted
            && (settings.legal_terms_version.is_empty()
                || settings.legal_terms_version == crate::LEGAL_TERMS_VERSION)
        {
            if settings.legal_terms_version.is_empty() {
                let _ = self.set_setting(
                    "legal_terms_version",
                    Value::String(crate::LEGAL_TERMS_VERSION.into()),
                );
            }
            return Ok(());
        }
        Err("legal terms not accepted".into())
    }

    fn accept_handoff_command(
        &self,
        handoff_id: String,
        filename: String,
        download_dir: String,
        trusted_ui: bool,
    ) -> Result<Vec<EventEnvelope>, String> {
        self.require_legal()?;
        let settings = self.settings()?;
        let download_dir = if trusted_ui {
            let requested = download_dir.trim();
            if requested.is_empty() {
                settings.download_dir.clone()
            } else {
                reject_path_escape(requested)?;
                requested.to_string()
            }
        } else {
            constrain_untrusted_download_dir(&download_dir, &settings.download_dir)?
        };
        let Some(offer) = self.lock()?.pending_handoff(&handoff_id) else {
            return Err("接管请求不存在或已过期".into());
        };
        let filename = if filename.trim().is_empty() {
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
            filename
        };
        let spec = self.apply_defaults_to_spec(TaskSpec {
            url: offer.url.clone(),
            resource_kind: offer.resource_kind,
            title: if offer.title.trim().is_empty() {
                filename.clone()
            } else {
                offer.title.clone()
            },
            filename,
            download_dir,
            request_method: offer.request_method.clone(),
            credential_ref: offer.credential_ref.clone(),
            replay_context_ref: offer.replay_context_ref.clone(),
            expected_size: (offer.size > 0).then_some(offer.size),
            ..Default::default()
        })?;
        let mut events = self.dispatch(CoreCommand::CreateTask { spec })?;
        let task_id = events.iter().find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } | CoreEvent::TaskUpdated { snapshot } => {
                Some(snapshot.task_id.clone())
            }
            CoreEvent::DuplicateOffered { task_id, .. } => Some(task_id.clone()),
            _ => None,
        });
        let mut core = self.lock()?;
        events.extend(core.emit(CoreEvent::HandoffResolved {
            handoff_id: handoff_id.clone(),
            task_id,
        })?);
        // Only remove the live offer after its resolved row and event checkpoint commit.
        // A persistence failure must leave the offer retryable in this process.
        let _ = core.take_pending_handoff(&handoff_id);
        Ok(events)
    }

    fn present_handoff_command(
        &self,
        handoff_id: String,
        ok: bool,
        presenter_id: String,
    ) -> Result<Vec<EventEnvelope>, String> {
        if handoff_id.trim().is_empty() {
            return Err("接管请求缺少编号".into());
        }
        let mut core = self.lock()?;
        let mut updated = false;
        for encoded in core.store().load_handoffs()? {
            let Ok(mut value) = serde_json::from_str::<Value>(&encoded) else {
                continue;
            };
            if value.get("id").and_then(Value::as_str) != Some(handoff_id.as_str()) {
                continue;
            }
            let current = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            if matches!(
                current,
                "accepted" | "rejected" | "canceled" | "expired" | "failed"
            ) {
                return Err("接管请求已结束".into());
            }
            let now = handoff_now_ms();
            let presentation = value
                .get("presentation")
                .and_then(Value::as_str)
                .unwrap_or("queued")
                .to_string();
            let owner = value
                .get("presentation_owner")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let lease_until = value
                .get("presentation_lease_until_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let presenter_id = presenter_id.trim();
            let claimant = if presenter_id.is_empty() {
                "legacy-presenter"
            } else {
                presenter_id
            };
            if ok && presentation == "fallback" {
                return Err("接管请求已由主窗口处理".into());
            }
            if ok && !owner.is_empty() && owner != claimant && lease_until > now {
                return Err("接管请求已由另一个确认窗口处理".into());
            }
            if !ok && presenter_id.is_empty() && lease_until > now {
                return Err("下载确认窗口仍在处理该请求".into());
            }
            if !ok && !presenter_id.is_empty() && owner != presenter_id {
                return Err("接管请求不属于当前确认窗口".into());
            }
            if let Some(object) = value.as_object_mut() {
                if ok {
                    let next = if owner == claimant && presentation == "presenting" {
                        "presented"
                    } else if presenter_id.is_empty() {
                        "presented"
                    } else if owner == claimant {
                        presentation.as_str()
                    } else {
                        "presenting"
                    };
                    object.insert("presentation".into(), Value::String(next.into()));
                    object.insert("presentation_owner".into(), Value::String(claimant.into()));
                    object.insert(
                        "presentation_lease_until_ms".into(),
                        Value::from(now.saturating_add(HANDOFF_PRESENTER_LEASE_MS)),
                    );
                } else {
                    object.insert("presentation".into(), Value::String("fallback".into()));
                    object.insert("presentation_owner".into(), Value::String("compose".into()));
                    object.insert("presentation_lease_until_ms".into(), Value::from(0));
                    object.insert("status".into(), Value::String("pending".into()));
                }
            }
            let json = serde_json::to_string(&value)
                .map_err(|error| format!("encode handoff presentation {handoff_id}: {error}"))?;
            let created = value
                .get("created_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            let task_id = value.get("task_id").and_then(Value::as_str);
            core.store_mut()
                .save_handoff(&handoff_id, &json, status, task_id, created)?;
            updated = true;
            break;
        }
        if !updated {
            return Err("接管请求不存在或已过期".into());
        }
        if !ok && updated {
            return core.emit(CoreEvent::UiShow {
                surface: "main".into(),
            });
        }
        Ok(Vec::new())
    }

    fn persist_handoff_suppression(&self, handoff_id: &str) -> Result<(), String> {
        let offer = self
            .lock()?
            .pending_handoff(handoff_id)
            .ok_or_else(|| "接管请求不存在或已过期".to_string())?;
        let host = offer
            .source_page_url
            .split_once("://")
            .map(|(_, tail)| tail)
            .unwrap_or(&offer.source_page_url)
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('@')
            .next()
            .unwrap_or("")
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if host.is_empty() {
            return Err("来源网页地址无效，不能保存站点提示规则".into());
        }
        let kind = serde_json::to_value(offer.resource_kind)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "file".into());
        let mut core = self.lock()?;
        for encoded in core.store().load_handoffs()? {
            let Ok(mut value) = serde_json::from_str::<Value>(&encoded) else {
                continue;
            };
            if value.get("id").and_then(Value::as_str) != Some(handoff_id) {
                continue;
            }
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "suppression".into(),
                    serde_json::json!({ "host": host, "kind": kind }),
                );
            }
            let json = serde_json::to_string(&value)
                .map_err(|error| format!("encode handoff suppression {handoff_id}: {error}"))?;
            let created = value
                .get("created_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            let task_id = value.get("task_id").and_then(Value::as_str);
            core.store_mut()
                .save_handoff(handoff_id, &json, status, task_id, created)?;
            return Ok(());
        }
        Err("接管请求持久记录不存在".into())
    }

    fn delete_task_files(&self, task_id: &str) -> Result<(), String> {
        let spec = self
            .lock()?
            .task_spec(task_id)
            .cloned()
            .ok_or_else(|| format!("unknown task {task_id}"))?;
        let paths = TaskPaths::for_task(task_id, &spec)?;
        let _ = fs::remove_file(&paths.final_output);
        let _ = fs::remove_file(&paths.output);
        let _ = fs::remove_dir_all(paths.task_dir());
        Ok(())
    }

    fn spawn(&self, task_id: String) -> Result<(), String> {
        if self.update_shutdown.load(Ordering::SeqCst) {
            return Err("下载引擎正在准备覆盖升级，暂不启动新任务".into());
        }
        let spec = self.lock()?.task_spec(&task_id).cloned();
        if let Some(spec) = spec.as_ref() {
            if !crate::net_policy::scheduled_start_reached(&spec.scheduled_start_at)
                || crate::net_policy::scheduled_stop_hit(&spec.scheduled_stop_at)
            {
                return Ok(());
            }
        }
        let tasks = self.tasks()?;
        let queue_id = tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .map(|task| task.queue_id.clone())
            .unwrap_or_else(|| crate::DEFAULT_QUEUE_ID.into());
        let profile = self
            .settings()?
            .queue_profiles
            .into_iter()
            .find(|profile| profile.id == queue_id)
            .ok_or_else(|| format!("任务所属队列不存在: {queue_id}"))?;
        if !queue_profile_allowed(&profile) {
            return Ok(());
        }
        {
            let core = self.lock()?;
            let status = core
                .tasks()
                .iter()
                .find(|task| task.task_id == task_id)
                .map(|task| task.status.clone());
            if !matches!(status.as_deref(), Some("queued" | "downloading")) {
                return Ok(());
            }
            if status.as_deref() == Some("queued") {
                if let Some(spec) = spec.as_ref() {
                    let paths = TaskPaths::for_task(&task_id, spec)?;
                    paths.prepare()?;
                    paths.set_control("run")?;
                }
            }
        }
        let max = profile.max_active.max(1) as usize;
        {
            let mut active = self
                .active
                .lock()
                .map_err(|_| "download worker registry poisoned".to_string())?;
            let active_in_queue = tasks
                .iter()
                .filter(|task| task.queue_id == queue_id && active.contains(&task.task_id))
                .count();
            if active_in_queue >= max && !active.contains(&task_id) {
                drop(active);
                if let Ok(mut core) = self.lock() {
                    let progress = core
                        .tasks()
                        .iter()
                        .find(|task| task.task_id == task_id)
                        .filter(|task| task.status == "downloading")
                        .map(|task| (task.downloaded_bytes, task.total_bytes));
                    if let Some((downloaded_bytes, total_bytes)) = progress {
                        let _ = core.handle(CoreCommand::UpdateProgress {
                            task_id: task_id.clone(),
                            downloaded_bytes,
                            total_bytes,
                            speed_bytes_per_sec: 0,
                            stage: "waiting".into(),
                            status: "queued".into(),
                        });
                    }
                }
                return Ok(());
            }
            if !active.insert(task_id.clone()) {
                return Ok(());
            }
        }
        crate::sleep_inhibit::set_active(true);
        let core = Arc::clone(&self.core);
        let active = Arc::clone(&self.active);
        let retries = Arc::clone(&self.retries);
        let coordinator = self.clone();
        thread::spawn(move || {
            let result = run_task_with_progress(Arc::clone(&core), &task_id);
            if let Err(error) = result {
                let status = if error == "paused" {
                    "paused"
                } else if error == "canceled" {
                    "canceled"
                } else {
                    "failed"
                };
                let attempt = if status == "failed" {
                    let mut map = retries.lock().unwrap_or_else(|error| error.into_inner());
                    let slot = map.entry(task_id.clone()).or_insert(0);
                    *slot += 1;
                    *slot
                } else {
                    0
                };
                let _ = core.lock().map(|mut core| {
                    let current = core
                        .tasks()
                        .iter()
                        .find(|task| task.task_id == task_id)
                        .cloned()
                        .unwrap_or_default();
                    let downloaded = current.downloaded_bytes;
                    let total = current.total_bytes;
                    let stage = if status == "paused" {
                        "waiting".to_string()
                    } else if status == "failed" {
                        failure_stage(&current.stage, &error)
                    } else {
                        "finished".to_string()
                    };
                    let _ = core.handle(CoreCommand::UpdateProgress {
                        task_id: task_id.clone(),
                        downloaded_bytes: downloaded,
                        total_bytes: total,
                        speed_bytes_per_sec: 0,
                        stage: stage.clone(),
                        status: status.into(),
                    });
                    if status == "failed" {
                        let (url, has_credential_ref) = core
                            .task_spec(&task_id)
                            .map(|spec| (spec.url.clone(), spec.credential_ref.is_some()))
                            .unwrap_or_default();
                        let _ = core.report_failure(
                            &task_id,
                            task_failure_from_error(
                                &error,
                                &stage,
                                &url,
                                attempt,
                                has_credential_ref,
                            ),
                        );
                    }
                    eprintln!("v7 task {task_id} {status}: {error}");
                });
                if status == "failed" {
                    let max = coordinator
                        .lock()
                        .ok()
                        .and_then(|guard| {
                            guard.store().setting_u64("auto_retry_failed_max", 0).ok()
                        })
                        .unwrap_or(0);
                    if u64::from(attempt) <= max && max > 0 {
                        let _ = mark_progress(&core, &task_id, 0, None, "waiting", "queued");
                    }
                } else if status != "paused" {
                    if let Ok(mut map) = retries.lock() {
                        map.remove(&task_id);
                    }
                }
            } else if let Ok(mut map) = retries.lock() {
                map.remove(&task_id);
            }
            crate::net_policy::clear_scoped_limit(&format!("task:{task_id}"));
            if let Ok(mut active) = active.lock() {
                active.remove(&task_id);
                crate::sleep_inhibit::set_active(!active.is_empty());
            }
            let _ = coordinator.start_next_queued();
        });
        Ok(())
    }
}

fn load_queue_profiles(store: &crate::CoreStore) -> Result<Vec<QueueProfile>, String> {
    let raw = store.setting_string("queue_profiles", "")?;
    if !raw.trim().is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
            validate_queue_profiles(&value)?;
            return serde_json::from_value(value)
                .map_err(|error| format!("读取队列配置失败: {error}"));
        }
    }
    Ok(vec![QueueProfile {
        max_active: store.setting_u64("queue_max_active", 3)?.clamp(1, 64) as u32,
        schedule_enabled: store.setting_bool("queue_auto_start_enabled", false)?
            || store.setting_bool("queue_auto_stop_enabled", false)?,
        start_time: store.setting_string("queue_auto_start_time", "00:00")?,
        stop_time: store.setting_string("queue_auto_stop_time", "07:30")?,
        active_days: store.setting_string("queue_active_days", "1,2,3,4,5,6,7")?,
        completion_action: store.setting_string("completion_power_action", "none")?,
        ..QueueProfile::default()
    }])
}

fn queue_profile_allowed(profile: &QueueProfile) -> bool {
    profile.enabled
        && crate::net_policy::weekday_allowed(&profile.active_days)
        && (!profile.schedule_enabled
            || crate::net_policy::schedule_window_active(&profile.start_time, &profile.stop_time))
}

fn task_schedule_allowed(task: &crate::TaskSnapshot) -> bool {
    crate::net_policy::scheduled_start_reached(&task.scheduled_start_at)
        && !crate::net_policy::scheduled_stop_hit(&task.scheduled_stop_at)
}

fn validate_queue_profiles(value: &Value) -> Result<(), String> {
    let profiles: Vec<QueueProfile> = serde_json::from_value(value.clone())
        .map_err(|error| format!("队列配置格式无效: {error}"))?;
    if profiles.is_empty() || profiles.len() > 32 {
        return Err("必须保留 1 到 32 个队列".into());
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for profile in &profiles {
        let id = profile.id.trim();
        if id.is_empty()
            || id.len() > 40
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !ids.insert(id.to_ascii_lowercase())
        {
            return Err("队列编号必须唯一，且只能包含字母、数字、连字符和下划线".into());
        }
        let name = profile.name.trim();
        if name.is_empty()
            || name.chars().count() > 40
            || name.chars().any(char::is_control)
            || !names.insert(name.to_lowercase())
        {
            return Err("队列名称必须为 1 到 40 个可见字符".into());
        }
        if !(-100..=100).contains(&profile.priority) {
            return Err("队列优先级必须在 -100 到 100 之间".into());
        }
        if !(1..=64).contains(&profile.max_active) {
            return Err("队列并发数必须在 1 到 64 之间".into());
        }
        if profile.speed_limit_kib > 1_048_576 {
            return Err("队列限速不能超过 1048576 KiB/s".into());
        }
        if !valid_clock(&profile.start_time) || !valid_clock(&profile.stop_time) {
            return Err("队列计划时间必须使用 HH:mm".into());
        }
        let days: Vec<_> = profile.active_days.split(',').map(str::trim).collect();
        let unique_days: HashSet<_> = days.iter().copied().collect();
        if days.is_empty()
            || unique_days.len() != days.len()
            || !days.iter().all(|day| {
                day.parse::<u8>()
                    .is_ok_and(|value| (1..=7).contains(&value))
            })
        {
            return Err("队列活动星期必须使用 1 到 7".into());
        }
        if !matches!(
            profile.completion_action.as_str(),
            "" | "none" | "shutdown" | "sleep" | "hibernate"
        ) {
            return Err("队列完成动作无效".into());
        }
    }
    if !profiles
        .iter()
        .any(|profile| profile.id == crate::DEFAULT_QUEUE_ID)
    {
        return Err("默认队列不能删除".into());
    }
    Ok(())
}

fn valid_clock(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u8>().is_ok_and(|value| value < 24)
        && minute.parse::<u8>().is_ok_and(|value| value < 60)
}

fn validate_torrent_spec(spec: TaskSpec, enable_dht: bool) -> Result<TaskSpec, String> {
    if spec.resource_kind != crate::ResourceKind::Torrent {
        return Ok(spec);
    }
    let headers = spec
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let meta =
        crate::torrent_engine::probe_torrent_source(&spec.url, &headers, &spec.proxy, enable_dht)?;
    let mut checked = spec;
    if checked.filename.trim().is_empty() {
        checked.filename = safe_filename(&meta.name, &checked.url);
    }
    if checked.title.trim().is_empty() {
        checked.title = checked.filename.clone();
    }
    checked.torrent_piece_count = meta.pieces.len() as u64;
    if !checked.torrent_selection.is_empty() {
        checked.torrent_selection =
            crate::torrent_engine::validate_torrent_selection(&meta, &checked.torrent_selection)?;
        checked.expected_size = Some(selected_torrent_bytes(
            &meta.files,
            &checked.torrent_selection,
        ));
    } else if meta.length > 0 {
        checked.expected_size = Some(meta.length);
    }
    Ok(checked)
}

fn current_progress(core: &Arc<Mutex<PersistentCore>>, task_id: &str) -> (u64, Option<u64>) {
    core.lock()
        .ok()
        .and_then(|locked| {
            locked
                .tasks()
                .into_iter()
                .find(|task| task.task_id == task_id)
                .map(|task| (task.downloaded_bytes, task.total_bytes))
        })
        .unwrap_or((0, None))
}

fn run_task_with_progress(core: Arc<Mutex<PersistentCore>>, task_id: &str) -> Result<(), String> {
    let spec = core
        .lock()
        .map_err(|_| "v7 Core mutex poisoned".to_string())?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let (spec, replay_json) = hydrate_replay_headers(&core, spec)?;
    let spec = apply_site_rules_to_spec(&core, spec)?;
    let throttle = task_throttle_context(&core, task_id, &spec)?;
    crate::net_policy::configure_throttle_context(&throttle);
    crate::net_policy::with_throttle_context(Some(throttle), || {
        run_task_with_throttle(core, task_id, spec, replay_json)
    })
}

fn run_task_with_throttle(
    core: Arc<Mutex<PersistentCore>>,
    task_id: &str,
    spec: TaskSpec,
    replay_json: String,
) -> Result<(), String> {
    let headers: std::collections::HashMap<_, _> = spec
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    match spec.resource_kind {
        crate::ResourceKind::Hls | crate::ResourceKind::Live => {
            let paths = TaskPaths::for_task(task_id, &spec)?;
            paths.prepare()?;
            let (downloaded, total) = current_progress(&core, task_id);
            mark_progress(&core, task_id, downloaded, total, "transfer", "downloading")?;
            let live = matches!(spec.resource_kind, crate::ResourceKind::Live);
            let (skip_ads, download_subtitles, live_max) = {
                let guard = core
                    .lock()
                    .map_err(|_| "v7 Core mutex poisoned".to_string())?;
                (
                    guard.store().setting_bool("skip_ad_segments", true)?,
                    guard.store().setting_bool("download_subtitles", true)?,
                    guard.store().setting_u64("live_record_max_minutes", 0)?,
                )
            };
            let url = spec.url.clone();
            let proxy = spec.proxy.clone();
            let headers = headers.clone();
            let task_dir = paths
                .output
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf();
            let control = paths.control.clone();
            let progress = paths.progress.clone();
            let options = crate::media::HlsDownloadOptions {
                live,
                concurrency: spec.concurrency.max(1) as usize,
                preferred_bandwidth: spec.preferred_bandwidth,
                preferred_height: spec.preferred_height,
                preferred_audio: spec.preferred_audio.clone(),
                skip_ads,
                download_subtitles,
                live_max_minutes: live_max,
                progress: Some(progress.clone()),
            };
            let replay_json = replay_json.clone();
            let merged = poll_media_progress(&core, task_id, &progress, live, move || {
                with_replay_json(&replay_json, || {
                    crate::media::download_hls_with(
                        &url, &headers, &proxy, &task_dir, &control, options,
                    )
                })
            })?;
            let (downloaded, total) = current_progress(&core, task_id);
            mark_progress(&core, task_id, downloaded, total, "merging", "merging")?;
            complete_payload(&core, task_id, &paths, &merged, &spec)
        }
        crate::ResourceKind::Dash => {
            let paths = TaskPaths::for_task(task_id, &spec)?;
            paths.prepare()?;
            let (downloaded, total) = current_progress(&core, task_id);
            mark_progress(&core, task_id, downloaded, total, "transfer", "downloading")?;
            let url = spec.url.clone();
            let proxy = spec.proxy.clone();
            let headers = headers.clone();
            let task_dir = paths
                .output
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf();
            let control = paths.control.clone();
            let progress = paths.progress.clone();
            let bandwidth = spec.preferred_bandwidth;
            let download_subtitles = core
                .lock()
                .map_err(|_| "v7 Core mutex poisoned".to_string())?
                .store()
                .setting_bool("download_subtitles", true)?;
            let audio_name = spec.preferred_audio.clone();
            let replay_json = replay_json.clone();
            let merged = poll_media_progress(&core, task_id, &progress, false, move || {
                with_replay_json(&replay_json, || {
                    crate::media::download_dash_selected(
                        &url,
                        &headers,
                        &proxy,
                        &task_dir,
                        &control,
                        bandwidth,
                        download_subtitles,
                        &audio_name,
                    )
                })
            })?;
            let (downloaded, total) = current_progress(&core, task_id);
            mark_progress(&core, task_id, downloaded, total, "merging", "merging")?;
            complete_payload(&core, task_id, &paths, &merged, &spec)
        }
        crate::ResourceKind::Ftp => {
            let paths = TaskPaths::for_task(task_id, &spec)?;
            paths.prepare()?;
            let url = spec.url.clone();
            let output = paths.output.clone();
            let control = paths.control.clone();
            let progress = paths.output.with_extension("progress.json");
            let _ = poll_media_progress(&core, task_id, &progress, false, move || {
                crate::ftp_engine::download_ftp(&url, &output, &control, true)
                    .map(|_| output.clone())
            })?;
            complete_payload(&core, task_id, &paths, &paths.output, &spec)
        }
        crate::ResourceKind::Sftp => {
            let paths = TaskPaths::for_task(task_id, &spec)?;
            paths.prepare()?;
            let url = spec.url.clone();
            let output = paths.output.clone();
            let control = paths.control.clone();
            let progress = paths.output.with_extension("progress.json");
            let _ = poll_media_progress(&core, task_id, &progress, false, move || {
                crate::sftp_engine::download_sftp(&url, &output, &control).map(|_| output.clone())
            })?;
            complete_payload(&core, task_id, &paths, &paths.output, &spec)
        }
        crate::ResourceKind::Torrent => {
            let paths = TaskPaths::for_task(task_id, &spec)?;
            paths.prepare()?;
            initialize_torrent_selection(&paths.torrent_selection, &spec.torrent_selection)?;
            let torrent_options = {
                let guard = core
                    .lock()
                    .map_err(|_| "v7 Core mutex poisoned".to_string())?;
                crate::torrent_engine::TorrentOptions {
                    upload_limit_kib: guard.store().setting_u64("bt_upload_limit_kib", 1024)?,
                    max_connections: guard
                        .store()
                        .setting_u64("bt_max_connections", 200)?
                        .clamp(10, 1000) as usize,
                    enable_dht: guard.store().setting_bool("bt_enable_dht", true)?,
                    selection_path: paths.torrent_selection.clone(),
                }
            };
            let telemetry_core = Arc::clone(&core);
            let telemetry_task_id = task_id.to_string();
            let mut telemetry_reporter =
                move |telemetry: crate::torrent_engine::TorrentTelemetry| {
                    let Ok(mut guard) = telemetry_core.lock() else {
                        return;
                    };
                    let _ = guard.set_torrent_telemetry(
                        &telemetry_task_id,
                        telemetry.peer_count,
                        telemetry.seed_count,
                        telemetry.uploaded_bytes,
                        telemetry.upload_speed_bytes_per_sec,
                    );
                };
            let torrent_url = spec.url.clone();
            let torrent_output = paths.output.clone();
            let torrent_control = paths.control.clone();
            let torrent_headers = headers.clone();
            let torrent_proxy = spec.proxy.clone();
            let torrent_progress = paths.progress.clone();
            let _ = poll_media_progress(&core, task_id, &torrent_progress, false, move || {
                crate::torrent_engine::torrent_session()
                    .download_with_telemetry(
                        &torrent_url,
                        &torrent_output,
                        &torrent_control,
                        &torrent_headers,
                        &torrent_proxy,
                        torrent_options,
                        &mut telemetry_reporter,
                    )
                    .map(|_| torrent_output.clone())
            })?;
            let (latest_spec, enable_dht) = {
                let guard = core
                    .lock()
                    .map_err(|_| "v7 Core mutex poisoned".to_string())?;
                (
                    guard
                        .task_spec(task_id)
                        .cloned()
                        .unwrap_or_else(|| spec.clone()),
                    guard.store().setting_bool("bt_enable_dht", true)?,
                )
            };
            if !latest_spec.torrent_selection.is_empty() {
                let meta = crate::torrent_engine::probe_torrent_source(
                    &latest_spec.url,
                    &headers,
                    &latest_spec.proxy,
                    enable_dht,
                )?;
                let published = crate::torrent_engine::materialize_selected_files(
                    &paths.output,
                    &paths.final_output,
                    &meta,
                    &latest_spec.torrent_selection,
                )?;
                remember_published(&paths, &paths.final_output);
                core.lock()
                    .map_err(|_| "v7 Core mutex poisoned".to_string())?
                    .set_output_path(task_id, paths.final_output.to_string_lossy().into_owned())?;
                mark_progress(
                    &core,
                    task_id,
                    published,
                    Some(published),
                    "finished",
                    "completed",
                )?;
                maybe_schedule_power(&core, &latest_spec)
            } else {
                complete_payload(&core, task_id, &paths, &paths.output, &spec)
            }
        }
        crate::ResourceKind::File => run_http_file(core, task_id, spec, replay_json),
    }
}

fn run_http_file(
    core: Arc<Mutex<PersistentCore>>,
    task_id: &str,
    spec: TaskSpec,
    replay_json: String,
) -> Result<(), String> {
    let (mut job, paths) = build_job(task_id, &spec)?;
    job.replay_json = replay_json;
    let temporary_body = materialize_replay_request_body(&job.replay_json, &paths)?;
    if let Some(path) = &temporary_body.0 {
        job.body_path = path.clone();
    }
    if let Ok(mb) = core
        .lock()
        .map_err(|_| "v7 Core mutex poisoned".to_string())?
        .store()
        .setting_u64("http_chunk_size_mb", 8)
    {
        job.chunk_bytes = mb.clamp(1, 64) * 1024 * 1024;
    }
    let (sender, receiver) = mpsc::channel();
    let worker_job = job.clone();
    let throttle = crate::net_policy::current_throttle_context();
    thread::spawn(move || {
        let result =
            crate::net_policy::with_throttle_context(throttle, || run_job_report(&worker_job));
        let _ = sender.send(result);
    });
    loop {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(result) => {
                let report = result.map_err(|error| error.to_string())?;
                if !report.mirrors.is_empty() {
                    core.lock()
                        .map_err(|_| "v7 Core mutex poisoned".to_string())?
                        .set_mirror_result(
                            task_id,
                            report
                                .mirrors
                                .into_iter()
                                .map(|item| MirrorStatus {
                                    url: item.url,
                                    final_url: item.final_url,
                                    state: item.state,
                                    detail: item.detail,
                                    ranges: item.ranges,
                                })
                                .collect(),
                        )?;
                }
                return complete_payload(&core, task_id, &paths, &paths.output, &spec);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(progress) = read_progress(&paths.progress) {
                    let _ = mark_progress_speed(
                        &core,
                        task_id,
                        progress.downloaded,
                        (progress.total > 0).then_some(progress.total),
                        progress.speed,
                        "transfer",
                        "downloading",
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("download HTTP worker disconnected".into())
            }
        }
    }
}

fn materialize_replay_request_body(
    replay_json: &str,
    paths: &TaskPaths,
) -> Result<TemporaryRequestBody, String> {
    if replay_json.trim().is_empty() {
        return Ok(TemporaryRequestBody(None));
    }
    let value: Value =
        serde_json::from_str(replay_json).map_err(|error| format!("请求上下文损坏: {error}"))?;
    let Some(encoded) = value.get("request_body").and_then(Value::as_str) else {
        return Ok(TemporaryRequestBody(None));
    };
    let body = decode_base64_bounded(encoded, MAX_REPLAY_BODY_BYTES)?;
    let path = paths.task_dir().join("request-body.bin");
    fs::write(&path, body).map_err(|error| format!("写入临时请求体失败: {error}"))?;
    Ok(TemporaryRequestBody(Some(path)))
}

fn decode_base64_bounded(value: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    if value.len() > ((max_bytes + 2) / 3) * 4 + 4 {
        return Err("POST 请求体超过 128 KiB 限制".into());
    }
    let mut output = Vec::with_capacity(value.len() / 4 * 3);
    let mut block = [0u8; 4];
    let mut count = 0usize;
    let mut padded = false;
    for byte in value.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if padded {
            return Err("POST 请求体 Base64 格式无效".into());
        }
        block[count] = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err("POST 请求体 Base64 格式无效".into()),
        };
        count += 1;
        if count != 4 {
            continue;
        }
        if block[0] == 64 || block[1] == 64 || (block[2] == 64 && block[3] != 64) {
            return Err("POST 请求体 Base64 填充无效".into());
        }
        output.push((block[0] << 2) | (block[1] >> 4));
        if block[2] != 64 {
            output.push((block[1] << 4) | (block[2] >> 2));
        }
        if block[3] != 64 {
            output.push((block[2] << 6) | block[3]);
        }
        padded = block[2] == 64 || block[3] == 64;
        count = 0;
        if output.len() > max_bytes {
            return Err("POST 请求体超过 128 KiB 限制".into());
        }
    }
    if count != 0 {
        return Err("POST 请求体 Base64 长度无效".into());
    }
    Ok(output)
}

fn task_failure_from_error(
    error: &str,
    stage: &str,
    url: &str,
    attempt: u32,
    has_credential_ref: bool,
) -> crate::TaskFailure {
    let lower = error.to_ascii_lowercase();
    let http_status = extract_http_status(error);
    let (code, hint) = if let Some(status) = http_status {
        let hint = match status {
            401 if !has_credential_ref => "服务器要求登录：手工新建任务请在“请求”页填写 Cookie 或 Authorization 请求头；也可从已登录页面通过浏览器插件重新发送",
            401 => "服务器拒绝了当前凭据，请回到原网页刷新登录状态后重新发送，或更新任务的请求头/Cookie",
            403 => "访问凭据或短效签名可能已过期，请回到原网页刷新后重新发送资源",
            404 => "资源地址已失效或文件已被移动，请回到来源页面重新识别",
            408 | 425 | 429 => "服务器暂时限制请求，请降低并发并稍后重试",
            500..=599 => "服务器暂时不可用，请稍后重试或切换备用地址",
            _ => "服务器拒绝了下载请求，请检查资源地址和站点规则",
        };
        (format!("HTTP_{status}"), hint.to_string())
    } else if lower.contains("size mismatch") {
        (
            "SIZE_MISMATCH".into(),
            "服务端文件内容已变化，请重新识别后再下载".into(),
        )
    } else if lower.contains("checksum") || lower.contains("校验") {
        (
            "CHECKSUM_FAILED".into(),
            "文件校验未通过，请重新下载或确认发布方提供的校验值".into(),
        )
    } else if lower.contains("av_threat") || lower.contains("virus") || lower.contains("病毒") {
        (
            "AV_THREAT".into(),
            "安全扫描发现风险，文件不会发布到最终目录".into(),
        )
    } else if lower.contains("no space")
        || lower.contains("disk full")
        || lower.contains("磁盘空间")
    {
        (
            "DISK_FULL".into(),
            "清理保存盘空间或更换下载目录后重试".into(),
        )
    } else if lower.contains("access denied")
        || lower.contains("permission denied")
        || lower.contains("拒绝访问")
    {
        (
            "OUTPUT_PERMISSION".into(),
            "更换到当前用户可写的下载目录后重试".into(),
        )
    } else if lower.contains("timed out") || lower.contains("timeout") || lower.contains("超时") {
        (
            "NETWORK_TIMEOUT".into(),
            "检查网络或代理设置，降低并发后重试".into(),
        )
    } else if lower.contains("invalid url")
        || lower.contains("invalid uri")
        || lower.contains("地址无效")
    {
        (
            "INVALID_URL".into(),
            "检查下载地址格式，或回到来源页面重新识别".into(),
        )
    } else {
        (
            "DOWNLOAD_FAILED".into(),
            "查看任务日志确认失败位置，修正网络或站点设置后重试".into(),
        )
    };
    crate::TaskFailure {
        code,
        message: error.trim().chars().take(800).collect(),
        stage: stage.to_string(),
        url: url.to_string(),
        hint,
        http_status,
        attempt,
    }
}

fn extract_http_status(error: &str) -> Option<u16> {
    let upper = error.to_ascii_uppercase();
    for marker in ["HTTP ", "HTTP_", "STATUS ", "STATUS="] {
        let mut start = 0;
        while let Some(offset) = upper[start..].find(marker) {
            let value_start = start + offset + marker.len();
            let digits: String = upper[value_start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .take(3)
                .collect();
            if let Ok(status) = digits.parse::<u16>() {
                if (100..=599).contains(&status) {
                    return Some(status);
                }
            }
            start = value_start;
        }
    }
    None
}

fn failure_stage(current: &str, error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("checksum") || lower.contains("校验") {
        "checksum".into()
    } else if lower.contains("size mismatch") {
        "size".into()
    } else if lower.contains("av_threat") || lower.contains("virus") || lower.contains("病毒") {
        "av_scan".into()
    } else if current.trim().is_empty() || current == "finished" {
        "transfer".into()
    } else {
        current.to_string()
    }
}

fn mark_progress(
    core: &Arc<Mutex<PersistentCore>>,
    task_id: &str,
    downloaded: u64,
    total: Option<u64>,
    stage: &str,
    status: &str,
) -> Result<(), String> {
    mark_progress_speed(core, task_id, downloaded, total, 0, stage, status)
}

fn mark_progress_speed(
    core: &Arc<Mutex<PersistentCore>>,
    task_id: &str,
    downloaded: u64,
    total: Option<u64>,
    speed_bytes_per_sec: u64,
    stage: &str,
    status: &str,
) -> Result<(), String> {
    core.lock()
        .map_err(|_| "v7 Core mutex poisoned".to_string())?
        .handle(CoreCommand::UpdateProgress {
            task_id: task_id.into(),
            downloaded_bytes: downloaded,
            total_bytes: total,
            speed_bytes_per_sec,
            stage: stage.into(),
            status: status.into(),
        })?;
    Ok(())
}

fn output_policy(core: &Arc<Mutex<PersistentCore>>) -> (String, bool) {
    let Ok(guard) = core.lock() else {
        return ("rename".into(), false);
    };
    (
        guard
            .store()
            .setting_string("existing_file_policy", "rename")
            .unwrap_or_else(|_| "rename".into()),
        guard
            .store()
            .setting_bool("keep_temp_files", false)
            .unwrap_or(false),
    )
}

fn complete_payload(
    core: &Arc<Mutex<PersistentCore>>,
    task_id: &str,
    paths: &TaskPaths,
    payload: &Path,
    spec: &TaskSpec,
) -> Result<(), String> {
    ensure_publish_allowed(&paths.control)?;
    mark_progress(
        core,
        task_id,
        spec.expected_size.unwrap_or(0),
        spec.expected_size,
        "checking",
        "checking",
    )?;
    if let Some(expected) = spec.expected_size {
        let actual = fs::metadata(payload).map(|meta| meta.len()).unwrap_or(0);
        if actual != expected {
            mark_progress(core, task_id, actual, Some(expected), "size", "failed")?;
            return Err(format!("size mismatch: expected {expected}, got {actual}"));
        }
    }
    if let Some(checksum) = spec
        .checksum
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        match crate::checksum::verify_file_result(payload, checksum) {
            Ok(Some(result)) => {
                core.lock()
                    .map_err(|_| "v7 Core mutex poisoned".to_string())?
                    .set_checksum_result(
                        task_id,
                        result.algorithm,
                        result.actual.clone(),
                        result.verified,
                    )?;
                if !result.verified {
                    mark_progress(core, task_id, 0, None, "checksum", "failed")?;
                    return Err(format!(
                        "checksum mismatch: expected {}, got {}",
                        result.expected, result.actual
                    ));
                }
            }
            Ok(None) => {}
            Err(error) => {
                mark_progress(core, task_id, 0, None, "checksum", "failed")?;
                return Err(error);
            }
        }
    }
    let scan_enabled = core
        .lock()
        .ok()
        .and_then(|guard| guard.store().setting_bool("av_scan_enabled", false).ok())
        .unwrap_or(false);
    if scan_enabled {
        let template = core
            .lock()
            .ok()
            .and_then(|guard| guard.store().setting_string("av_scan_command", "").ok())
            .unwrap_or_default();
        let result = crate::av_scan::scan_file(payload, &template);
        core.lock()
            .map_err(|_| "v7 Core mutex poisoned".to_string())?
            .set_av_scan_result(
                task_id,
                AvScanStatus {
                    state: result.state.clone(),
                    engine: result.engine.clone(),
                    detail: result.detail.clone(),
                },
            )?;
        let fail_on_threat = core
            .lock()
            .ok()
            .and_then(|guard| {
                guard
                    .store()
                    .setting_bool("av_scan_fail_on_threat", true)
                    .ok()
            })
            .unwrap_or(true);
        if result.state == "threat" && fail_on_threat {
            mark_progress(core, task_id, 0, None, "av_scan", "failed")?;
            return Err(format!("av_threat: {}", result.detail));
        }
    }
    let (policy, keep_temp) = output_policy(core);
    let download_subtitles = core
        .lock()
        .ok()
        .and_then(|guard| guard.store().setting_bool("download_subtitles", true).ok())
        .unwrap_or(true);
    let mut core_guard = core
        .lock()
        .map_err(|_| "v7 Core mutex poisoned".to_string())?;
    ensure_publish_allowed(&paths.control)?;
    let published =
        crate::output_path::publish_file(payload, &paths.final_output, &policy, keep_temp)?;
    remember_published(paths, &published);
    core_guard.set_output_path(task_id, published.to_string_lossy().into_owned())?;
    crate::motw::mark_downloaded_file(&published, &spec.url);
    if download_subtitles {
        copy_subtitle_sidecars(&paths.task_dir(), &published);
    }
    let total = fs::metadata(&published).ok().map(|meta| meta.len());
    core_guard.handle(CoreCommand::UpdateProgress {
        task_id: task_id.into(),
        downloaded_bytes: total.unwrap_or(0),
        total_bytes: total,
        speed_bytes_per_sec: 0,
        stage: "finished".into(),
        status: "completed".into(),
    })?;
    drop(core_guard);
    maybe_schedule_power(core, spec)
}

fn ensure_publish_allowed(control: &Path) -> Result<(), String> {
    match fs::read_to_string(control)
        .unwrap_or_else(|_| "run".into())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "pause" => Err("paused".into()),
        "cancel" => Err("canceled".into()),
        _ => Ok(()),
    }
}

fn remember_published(paths: &TaskPaths, published: &Path) {
    let _ = fs::write(
        paths.task_dir().join("published.path"),
        published.to_string_lossy().as_bytes(),
    );
}

fn resolve_published(paths: &TaskPaths) -> PathBuf {
    if let Ok(text) = fs::read_to_string(paths.task_dir().join("published.path")) {
        let candidate = PathBuf::from(text.trim());
        if published_path_allowed(&candidate, paths) && candidate.exists() {
            return candidate;
        }
    }
    if paths.final_output.exists() {
        paths.final_output.clone()
    } else {
        paths.output.clone()
    }
}

fn published_path_allowed(candidate: &Path, paths: &TaskPaths) -> bool {
    let canon = logical_canonical(candidate);
    let mut roots = vec![logical_canonical(&paths.task_dir())];
    if let Some(parent) = paths.final_output.parent() {
        roots.push(logical_canonical(parent));
    }
    roots
        .iter()
        .any(|root| canon == *root || canon.starts_with(root))
}

fn copy_subtitle_sidecars(task_dir: &Path, published: &Path) {
    let subs = task_dir.join("subs");
    let Ok(entries) = fs::read_dir(subs) else {
        return;
    };
    let stem = published
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let parent = published.parent().unwrap_or(Path::new("."));
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "vtt" | "srt" | "ass" | "ssa" | "ttml") {
            continue;
        }
        let lang = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("und");
        if lang.contains(['/', '\\', ':']) || lang.contains("..") {
            continue;
        }
        let dest = parent.join(format!("{stem}.{lang}.{ext}"));
        let parent_canon = logical_canonical(parent);
        let dest_canon = logical_canonical(&dest);
        if dest_canon != parent_canon && !dest_canon.starts_with(&parent_canon) {
            continue;
        }
        let _ = fs::copy(&path, dest);
    }
}

fn maybe_schedule_power(core: &Arc<Mutex<PersistentCore>>, spec: &TaskSpec) -> Result<(), String> {
    let (action, title) = if crate::power_action::is_armed(&spec.completion_action) {
        let title = if spec.filename.trim().is_empty() {
            spec.url.clone()
        } else {
            spec.filename.clone()
        };
        (spec.completion_action.clone(), title)
    } else {
        let guard = core
            .lock()
            .map_err(|_| "v7 Core mutex poisoned".to_string())?;
        let profiles = load_queue_profiles(guard.store())?;
        let Some(decision) = queue_completion_decision(&guard.tasks(), &profiles, spec) else {
            return Ok(());
        };
        decision
    };
    if crate::power_action::pending().is_some() {
        return Ok(());
    }
    crate::power_action::schedule(&action, 30)?;
    let _ = core
        .lock()
        .map_err(|_| "v7 Core mutex poisoned".to_string())?
        .emit(CoreEvent::PowerActionPending {
            action,
            title,
            delay_seconds: 30,
        });
    Ok(())
}

fn queue_completion_decision(
    tasks: &[crate::TaskSnapshot],
    profiles: &[QueueProfile],
    spec: &TaskSpec,
) -> Option<(String, String)> {
    let profile = profiles
        .iter()
        .find(|profile| profile.id == spec.queue_id)?;
    (crate::power_action::is_armed(&profile.completion_action)
        && queue_is_successfully_drained(tasks, &profile.id))
    .then(|| {
        (
            profile.completion_action.clone(),
            format!("队列：{}", profile.name),
        )
    })
}

fn queue_is_successfully_drained(tasks: &[crate::TaskSnapshot], queue_id: &str) -> bool {
    let mut matching = tasks.iter().filter(|task| task.queue_id == queue_id);
    let Some(first) = matching.next() else {
        return false;
    };
    first.status == "completed" && matching.all(|task| task.status == "completed")
}

fn poll_media_progress<F>(
    core: &Arc<Mutex<PersistentCore>>,
    task_id: &str,
    progress: &Path,
    live: bool,
    work: F,
) -> Result<PathBuf, String>
where
    F: FnOnce() -> Result<PathBuf, String> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let throttle = crate::net_policy::current_throttle_context();
    thread::spawn(move || {
        let result = crate::net_policy::with_throttle_context(throttle, work);
        let _ = sender.send(result);
    });
    let status = if live { "recording" } else { "downloading" };
    let mut last = 0u64;
    let mut last_at = Instant::now();
    loop {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(item) = read_progress(progress) {
                    let elapsed = last_at.elapsed().as_secs_f64().max(0.001);
                    let speed = if item.speed > 0 {
                        item.speed
                    } else {
                        (item.downloaded.saturating_sub(last) as f64 / elapsed) as u64
                    };
                    last = item.downloaded;
                    last_at = Instant::now();
                    let _ = mark_progress_speed(
                        core,
                        task_id,
                        item.downloaded,
                        (item.total > 0).then_some(item.total),
                        speed,
                        "transfer",
                        status,
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("v7 media worker disconnected".into());
            }
        }
    }
}

fn apply_site_rules_to_spec(
    core: &Arc<Mutex<PersistentCore>>,
    mut spec: TaskSpec,
) -> Result<TaskSpec, String> {
    let raw = core
        .lock()
        .map_err(|_| "v7 Core mutex poisoned".to_string())?
        .store()
        .setting_string("site_rules", "")?;
    if let Some(rule) = crate::site_rules::matching_rule(&crate::parse_site_rules(&raw), &spec.url)
    {
        if rule.speed_limit_kib > 0 {
            spec.speed_limit_kib = rule.speed_limit_kib;
        }
        if rule.concurrency > 0 {
            spec.concurrency = rule.concurrency;
        }
        if spec.proxy.trim().is_empty() {
            match rule.proxy_mode.as_str() {
                "direct" => spec.proxy = crate::net_policy::DIRECT_PROXY_SENTINEL.into(),
                "manual" if !rule.proxy.trim().is_empty() => spec.proxy = rule.proxy.clone(),
                _ if !rule.proxy.trim().is_empty() => spec.proxy = rule.proxy.clone(),
                _ => {}
            }
        }
        if !rule.download_dir.trim().is_empty() && spec.download_dir.trim().is_empty() {
            spec.download_dir = rule.download_dir.clone();
        }
        if !rule.user_agent.trim().is_empty()
            && !spec
                .headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("user-agent"))
        {
            spec.headers
                .insert("User-Agent".into(), rule.user_agent.clone());
        }
        if !rule.referer.trim().is_empty()
            && !spec
                .headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("referer"))
        {
            spec.headers.insert("Referer".into(), rule.referer.clone());
        }
        if !rule.origin.trim().is_empty()
            && !spec
                .headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("origin"))
        {
            spec.headers.insert("Origin".into(), rule.origin.clone());
        }
        if spec.credential_ref.is_none() && !rule.credential_ref.trim().is_empty() {
            spec.credential_ref = Some(rule.credential_ref.clone());
        }
    }
    Ok(spec)
}

fn task_throttle_context(
    core: &Arc<Mutex<PersistentCore>>,
    task_id: &str,
    spec: &TaskSpec,
) -> Result<crate::net_policy::ThrottleContext, String> {
    let core = core
        .lock()
        .map_err(|_| "v7 Core mutex poisoned".to_string())?;
    let global = core.store().setting_u64("download_speed_limit_kib", 0)?;
    let schedule_enabled = core
        .store()
        .setting_bool("download_speed_schedule_enabled", false)?;
    let schedule_start = core
        .store()
        .setting_string("download_speed_schedule_start", "22:00")?;
    let schedule_end = core
        .store()
        .setting_string("download_speed_schedule_end", "08:00")?;
    let schedule_limit_kib = core.store().setting_u64("download_speed_schedule_kib", 0)?;
    let profile = load_queue_profiles(core.store())?
        .into_iter()
        .find(|profile| profile.id == spec.queue_id)
        .ok_or_else(|| format!("任务所属队列不存在: {}", spec.queue_id))?;
    Ok(crate::net_policy::ThrottleContext {
        global_limit_kib: global,
        schedule_enabled,
        schedule_start,
        schedule_end,
        schedule_limit_kib,
        hourly_quota_mib: core.store().setting_u64("download_hourly_quota_mib", 0)?,
        queue_id: profile.id,
        queue_limit_kib: profile.speed_limit_kib,
        task_id: task_id.to_string(),
        task_limit_kib: u64::from(spec.speed_limit_kib),
    })
}

fn specs_from_metalink(
    text: &str,
    template: &TaskSpec,
    auto: bool,
    dirs: &crate::category::CategoryDirs,
) -> Result<Vec<TaskSpec>, String> {
    Ok(crate::parse_metalink(text)?
        .into_iter()
        .map(|file| {
            let mut spec = spec_from_url(template, &file.url, &file.name, auto, dirs);
            spec.mirrors = file.mirrors;
            spec.checksum = (!file.checksum.is_empty()).then_some(file.checksum);
            spec.expected_size = (file.size > 0).then_some(file.size);
            spec
        })
        .collect())
}

fn spec_from_url(
    template: &TaskSpec,
    url: &str,
    filename: &str,
    auto: bool,
    dirs: &crate::category::CategoryDirs,
) -> TaskSpec {
    let mut spec = template.clone();
    spec.url = url.to_string();
    spec.resource_kind = crate::classify_url(url);
    if !filename.trim().is_empty() {
        spec.filename = filename.to_string();
    }
    spec.harvest = false;
    drop_cross_origin_task_secrets(&mut spec, &template.url, url);
    spec.download_dir = crate::category::resolve_category_dir(
        &template.download_dir,
        &spec.filename,
        &spec.url,
        spec.resource_kind,
        auto,
        dirs,
    );
    spec
}

fn drop_cross_origin_task_secrets(spec: &mut TaskSpec, from: &str, to: &str) {
    let from_origin = crate::credentials::request_origin(from);
    let to_origin = crate::credentials::request_origin(to);
    if from_origin.is_empty() || to_origin.is_empty() || from_origin == to_origin {
        return;
    }
    spec.headers.retain(|key, _| {
        !key.eq_ignore_ascii_case("cookie") && !key.eq_ignore_ascii_case("authorization")
    });
    spec.credential_ref = None;
}

fn seal_spec_secrets(
    coordinator: &CoreCoordinator,
    mut spec: TaskSpec,
) -> Result<TaskSpec, String> {
    let sensitive_keys: Vec<String> = spec
        .headers
        .keys()
        .filter(|name| sensitive_header_name(name))
        .cloned()
        .collect();
    if sensitive_keys.is_empty() {
        return Ok(spec);
    }
    let mut protected_headers = serde_json::Map::new();
    for key in sensitive_keys {
        if let Some(value) = spec.headers.remove(&key) {
            protected_headers.insert(key, Value::String(value));
        }
    }
    let mut context = spec
        .credential_ref
        .as_deref()
        .and_then(|credential_ref| coordinator.load_credential(credential_ref).ok().flatten())
        .map(|blob| CredentialVault.unprotect(&blob).unwrap_or(blob))
        .and_then(|plain| serde_json::from_str::<Value>(&plain).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut existing_headers = context
        .remove("request_headers")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    existing_headers.extend(protected_headers);
    context.insert("request_headers".into(), Value::Object(existing_headers));
    let json = Value::Object(context).to_string();
    let blob = if cfg!(windows) {
        CredentialVault.protect(&json)?
    } else {
        json
    };
    let credential_ref = format!("ui-{:x}-{:x}", simple_hash(&spec.url), simple_hash(&blob));
    coordinator.store_credential(&credential_ref, &blob, "browser_replay")?;
    spec.credential_ref = Some(credential_ref);
    Ok(spec)
}

fn sensitive_header_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    normalized == "cookie"
        || normalized == "authorization"
        || normalized == "proxy-authorization"
        || normalized.contains("token")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("api-key")
        || normalized.contains("apikey")
}

fn simple_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn take_header(headers: &mut BTreeMap<String, String>, name: &str) -> Option<String> {
    let key = headers
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .cloned()?;
    headers.remove(&key)
}

#[derive(Clone)]
struct PlayerUiState {
    active: bool,
    title: String,
    task_id: String,
    status: String,
    paused: bool,
    speed: f64,
}

impl Default for PlayerUiState {
    fn default() -> Self {
        Self {
            active: false,
            title: String::new(),
            task_id: String::new(),
            status: "STOPPED".into(),
            paused: false,
            speed: 1.0,
        }
    }
}

fn player_ui_state() -> &'static Mutex<PlayerUiState> {
    static STATE: OnceLock<Mutex<PlayerUiState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(PlayerUiState::default()))
}

fn emit_player_session(
    coordinator: &CoreCoordinator,
    state: &PlayerUiState,
) -> Result<Vec<EventEnvelope>, String> {
    let metadata = shared_player()
        .map(|player| player.metadata())
        .unwrap_or_default();
    coordinator.lock()?.emit(CoreEvent::PlayerSession {
        active: state.active,
        title: state.title.clone(),
        task_id: state.task_id.clone(),
        status: state.status.clone(),
        paused: state.paused,
        speed: state.speed,
        position_seconds: metadata.position_seconds,
        duration_seconds: metadata.duration_seconds,
        position_available: metadata.position_available,
        audio_tracks: metadata.audio_tracks,
        subtitle_tracks: metadata.subtitle_tracks,
    })
}

fn play_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<Vec<EventEnvelope>, String> {
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let url = mount_task_url(coordinator, task_id)?;
    shared_player()?.play(&url)?;
    let state = PlayerUiState {
        active: true,
        title: if spec.title.trim().is_empty() {
            spec.filename
        } else {
            spec.title
        },
        task_id: task_id.to_string(),
        status: "PLAYING".into(),
        paused: false,
        speed: 1.0,
    };
    if let Ok(mut current) = player_ui_state().lock() {
        *current = state.clone();
    }
    emit_player_session(coordinator, &state)
}

fn mount_task_url(coordinator: &CoreCoordinator, task_id: &str) -> Result<String, String> {
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let paths = TaskPaths::for_task(task_id, &spec)?;
    let server = shared_media()?;
    let token = crate::playback::random_mount_token();
    let url = if paths.final_output.exists() {
        server.mount(&token, paths.final_output);
        server.url_for(&token)
    } else if crate::playback::playlist_url(&paths.task_dir()).is_some() {
        server.mount_dir(&token, paths.task_dir());
        format!("{}/local.m3u8", server.url_for(&token))
    } else {
        server.mount(&token, paths.output);
        server.url_for(&token)
    };
    Ok(url)
}

fn media_token_from_url(url: &str) -> Option<String> {
    url.split("/media/")
        .nth(1)
        .map(|rest| rest.split('/').next().unwrap_or(rest).to_string())
        .filter(|token| !token.is_empty())
}

fn active_cast_token() -> &'static Mutex<Option<String>> {
    static TOKEN: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    TOKEN.get_or_init(|| Mutex::new(None))
}

fn remember_cast_mount(token: &str) {
    let previous = active_cast_token()
        .lock()
        .ok()
        .and_then(|mut current| current.replace(token.to_string()));
    if let Some(previous) = previous.filter(|item| item != token) {
        if let Ok(server) = shared_media() {
            server.unmount(&previous);
        }
    }
}

fn clear_cast_mount() {
    let token = active_cast_token()
        .lock()
        .ok()
        .and_then(|mut current| current.take());
    if let Some(token) = token {
        if let Ok(server) = shared_media() {
            server.unmount(&token);
        }
    }
}

fn cast_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<Vec<EventEnvelope>, String> {
    let loopback = mount_task_url(coordinator, task_id)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let server = shared_media()?;
    server.enable_lan();
    let host = crate::cast::primary_lan_ipv4()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".into());
    let location = crate::cast::lan_media_url(server, &token, &host)?;
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let title = if spec.title.trim().is_empty() {
        spec.filename
    } else {
        spec.title
    };
    let _ = crate::cast::ssdp_notify(&location);
    crate::cast::remember_lan_share("局域网播放地址");
    remember_cast_mount(&token);
    coordinator.lock()?.emit(CoreEvent::CastSession {
        active: true,
        title,
        device: "局域网".into(),
        status: "已在局域网发布播放地址".into(),
        task_id: task_id.to_string(),
        media_url: location,
        device_kind: "lan".into(),
        supported_actions: vec!["stop".into()],
        playing: false,
        paused: false,
        position_seconds: 0,
        duration_seconds: 0,
        position_available: false,
    })
}

fn cast_to_device(
    coordinator: &CoreCoordinator,
    task_id: &str,
    device_id: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let loopback = mount_task_url(coordinator, task_id)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let server = shared_media()?;
    server.enable_lan();
    let host = crate::cast::primary_lan_ipv4()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".into());
    let location = crate::cast::lan_media_url(server, &token, &host)?;
    let title = if spec.title.is_empty() {
        spec.filename.clone()
    } else {
        spec.title
    };
    if device_id.trim().is_empty() {
        let _ = crate::cast::ssdp_notify(&location);
        crate::cast::remember_lan_share("局域网播放地址");
        remember_cast_mount(&token);
        return coordinator.lock()?.emit(CoreEvent::CastSession {
            active: true,
            title,
            device: "局域网".into(),
            status: "已发出局域网投屏通知".into(),
            task_id: task_id.to_string(),
            media_url: location,
            device_kind: "lan".into(),
            supported_actions: vec!["stop".into()],
            playing: false,
            paused: false,
            position_seconds: 0,
            duration_seconds: 0,
            position_available: false,
        });
    }
    let device = match crate::cast::play_on_device(device_id, &location, &title) {
        Ok(device) => device,
        Err(error) => {
            server.unmount(&token);
            return Err(error);
        }
    };
    let playback = crate::cast::last_session_status();
    remember_cast_mount(&token);
    coordinator.lock()?.emit(CoreEvent::CastSession {
        active: true,
        title,
        device,
        status: "正在投屏".into(),
        task_id: task_id.to_string(),
        media_url: location,
        device_kind: playback.device_kind,
        supported_actions: playback.supported_actions,
        playing: playback.playing,
        paused: playback.paused,
        position_seconds: playback.position_seconds,
        duration_seconds: playback.duration_seconds,
        position_available: playback.position_available,
    })
}

fn share_media(
    coordinator: &CoreCoordinator,
    path: &str,
    url: &str,
    title: &str,
    device_id: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let path = path.trim();
    let url = url.trim();
    if path.is_empty() == url.is_empty() {
        return Err("请选择一个本机媒体文件或媒体链接".into());
    }
    let server = shared_media()?;
    server.enable_lan();
    let token = crate::playback::random_mount_token();
    let media_title;
    let media_url = if !path.is_empty() {
        let source = PathBuf::from(path);
        if !source.is_absolute() || !source.is_file() {
            return Err("本机媒体文件不存在".into());
        }
        media_title = if title.trim().is_empty() {
            source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("本机媒体")
                .to_string()
        } else {
            title.trim().to_string()
        };
        server.mount(&token, source);
        let host = crate::cast::primary_lan_ipv4()
            .map(|ip| ip.to_string())
            .ok_or_else(|| "没有可用于投屏的局域网地址".to_string())?;
        crate::cast::lan_media_url(server, &token, &host)?
    } else {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://"))
            || url.chars().any(char::is_control)
        {
            return Err("媒体链接必须是有效的 HTTP(S) 地址".into());
        }
        media_title = if title.trim().is_empty() {
            "网页媒体".into()
        } else {
            title.trim().to_string()
        };
        url.to_string()
    };
    if device_id.trim().is_empty() {
        let published_url = if url.is_empty() {
            let _ = crate::cast::ssdp_notify(&media_url);
            media_url.clone()
        } else {
            server.mount_remote(&token, media_url.clone());
            let host = crate::cast::primary_lan_ipv4()
                .map(|ip| ip.to_string())
                .ok_or_else(|| "没有可用于投屏的局域网地址".to_string())?;
            let redirect = crate::cast::lan_media_url(server, &token, &host)?;
            let _ = crate::cast::ssdp_notify(&redirect);
            redirect
        };
        crate::cast::remember_lan_share("局域网播放地址");
        remember_cast_mount(&token);
        return coordinator.lock()?.emit(CoreEvent::CastSession {
            active: true,
            title: media_title,
            device: "局域网".into(),
            status: "已发布局域网播放地址".into(),
            task_id: String::new(),
            media_url: published_url,
            device_kind: "lan".into(),
            supported_actions: vec!["stop".into()],
            playing: false,
            paused: false,
            position_seconds: 0,
            duration_seconds: 0,
            position_available: false,
        });
    }
    let device = match crate::cast::play_on_device(device_id, &media_url, &media_title) {
        Ok(device) => device,
        Err(error) => {
            server.unmount(&token);
            return Err(error);
        }
    };
    let playback = crate::cast::last_session_status();
    remember_cast_mount(&token);
    coordinator.lock()?.emit(CoreEvent::CastSession {
        active: true,
        title: media_title,
        device,
        status: if playback.device_kind == "tvbox" {
            "已推送到 TVBox".into()
        } else {
            "正在投屏".into()
        },
        task_id: String::new(),
        media_url: String::new(),
        device_kind: playback.device_kind,
        supported_actions: playback.supported_actions,
        playing: playback.playing,
        paused: playback.paused,
        position_seconds: playback.position_seconds,
        duration_seconds: playback.duration_seconds,
        position_available: playback.position_available,
    })
}

fn probe_command(
    coordinator: &CoreCoordinator,
    url: &str,
    _spec: Option<&TaskSpec>,
) -> Result<Vec<EventEnvelope>, String> {
    reject_task_url(url)?;
    match crate::recognize::probe_with_harvest(url) {
        Ok((kind, label, variants, harvest)) => {
            let mut events = coordinator.lock()?.emit(CoreEvent::ProbeResult {
                url: url.to_string(),
                resource_kind: kind,
                label: label.clone(),
                variants,
            })?;
            if !harvest.is_empty() {
                let min = coordinator
                    .settings()
                    .map(|item| item.harvest_minimum_bytes)
                    .unwrap_or(0);
                events.extend(
                    coordinator.lock()?.emit(CoreEvent::HarvestResult {
                        url: url.to_string(),
                        links: harvest
                            .into_iter()
                            .filter(|link| min == 0 || link.size_hint == 0 || link.size_hint >= min)
                            .map(|link| crate::HarvestCandidate {
                                url: link.url,
                                filename: link.filename,
                                extension: link.extension,
                                category: link.category,
                                size: link.size_hint,
                            })
                            .collect(),
                    })?,
                );
            }
            Ok(events)
        }
        Err(error) => coordinator.lock()?.emit(CoreEvent::Error {
            code: "probe_failed".into(),
            message: error,
        }),
    }
}

fn probe_torrent_command(
    coordinator: &CoreCoordinator,
    source: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let meta = crate::torrent_engine::probe_torrent_source(
        source,
        &std::collections::HashMap::new(),
        "",
        coordinator.settings()?.bt_enable_dht,
    )?;
    coordinator.lock()?.emit(CoreEvent::TorrentProbeResult {
        source: source.to_string(),
        name: meta.name,
        total_size: meta.length,
        files: meta.files,
        magnet: meta.magnet,
    })
}

fn select_torrent_files_command(
    coordinator: &CoreCoordinator,
    source: &str,
    selections: &[crate::TorrentFileSelection],
) -> Result<Vec<EventEnvelope>, String> {
    let meta = crate::torrent_engine::probe_torrent_source(
        source,
        &std::collections::HashMap::new(),
        "",
        coordinator.settings()?.bt_enable_dht,
    )?;
    let selections = crate::torrent_engine::validate_torrent_selection(&meta, selections)?;
    let total_size = meta
        .files
        .iter()
        .filter(|file| {
            selections
                .iter()
                .any(|item| item.index == file.index && item.selected)
        })
        .map(|file| file.size)
        .sum();
    coordinator.lock()?.emit(CoreEvent::TorrentSelectionResult {
        source: source.to_string(),
        selections,
        total_size,
    })
}

fn task_torrent_files(
    coordinator: &CoreCoordinator,
    task_id: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    if spec.resource_kind != crate::ResourceKind::Torrent {
        return Err("该任务不是 BT 任务".into());
    }
    let (hydrated, _) = hydrate_replay_headers(&coordinator.core, spec)?;
    let headers = hydrated
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let meta = crate::torrent_engine::probe_torrent_source(
        &hydrated.url,
        &headers,
        &hydrated.proxy,
        coordinator.settings()?.bt_enable_dht,
    )?;
    let selections =
        crate::torrent_engine::validate_torrent_selection(&meta, &hydrated.torrent_selection)?;
    let total_size = selected_torrent_bytes(&meta.files, &selections);
    coordinator.lock()?.emit(CoreEvent::TaskTorrentFiles {
        task_id: task_id.to_string(),
        source: hydrated.url,
        files: meta.files,
        selections,
        total_size,
    })
}

fn set_task_torrent_files(
    coordinator: &CoreCoordinator,
    task_id: &str,
    selections: &[crate::TorrentFileSelection],
) -> Result<Vec<EventEnvelope>, String> {
    let task = coordinator
        .tasks()?
        .into_iter()
        .find(|task| task.task_id == task_id)
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let mut spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    if spec.resource_kind != crate::ResourceKind::Torrent {
        return Err("该任务不是 BT 任务".into());
    }
    let (hydrated, _) = hydrate_replay_headers(&coordinator.core, spec.clone())?;
    let headers = hydrated
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let meta = crate::torrent_engine::probe_torrent_source(
        &hydrated.url,
        &headers,
        &hydrated.proxy,
        coordinator.settings()?.bt_enable_dht,
    )?;
    spec.torrent_selection = crate::torrent_engine::validate_torrent_selection(&meta, selections)?;
    if !spec
        .torrent_selection
        .iter()
        .any(|selection| selection.selected)
    {
        return Err("至少选择一个 BT 文件".into());
    }
    let paths = TaskPaths::for_task(task_id, &spec)?;
    write_torrent_selection(&paths.torrent_selection, &spec.torrent_selection)?;
    coordinator.lock()?.replace_spec(task_id, spec.clone())?;
    let total_size = selected_torrent_bytes(&meta.files, &spec.torrent_selection);
    let mut events = coordinator.lock()?.emit(CoreEvent::TaskTorrentFiles {
        task_id: task_id.to_string(),
        source: spec.url,
        files: meta.files,
        selections: spec.torrent_selection,
        total_size,
    })?;
    events.extend(coordinator.lock()?.emit(CoreEvent::Toast {
        level: "torrent_selection".into(),
        message: if matches!(task.status.as_str(), "downloading" | "recording") {
            "BT 文件选择已更新；取消项停止后续请求，新增项已进入 Piece 调度".into()
        } else {
            "BT 文件选择已保存，将在开始或恢复时生效".into()
        },
    })?);
    Ok(events)
}

fn selected_torrent_bytes(
    files: &[crate::TorrentFileEntry],
    selections: &[crate::TorrentFileSelection],
) -> u64 {
    files
        .iter()
        .filter(|file| {
            selections.iter().any(|selection| {
                selection.index == file.index && selection.path == file.path && selection.selected
            })
        })
        .map(|file| file.size)
        .sum()
}

fn harvest_page(
    coordinator: &CoreCoordinator,
    url: &str,
    referer: &str,
    probe_urls: &[String],
) -> Result<Vec<EventEnvelope>, String> {
    reject_task_url(url)?;
    let mut headers = std::collections::HashMap::new();
    if !referer.trim().is_empty() {
        reject_task_url(referer)?;
        headers.insert("Referer".into(), referer.trim().to_string());
    }
    let proxy = coordinator
        .settings()
        .map(|settings| settings.proxy_url)
        .unwrap_or_default();
    if !probe_urls.is_empty() {
        if probe_urls.len() > 100 {
            return Err("一次最多读取 100 个链接的大小".into());
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let links = probe_urls
            .iter()
            .filter_map(|candidate| {
                let candidate = candidate.trim();
                if !candidate.starts_with("http://") && !candidate.starts_with("https://") {
                    return None;
                }
                if reject_task_url(candidate).is_err() {
                    return None;
                }
                let base = std::env::temp_dir().join(format!(
                    "hls-v7-harvest-probe-{}-{stamp}",
                    simple_hash(candidate)
                ));
                let job = crate::http_engine::Job {
                    url: candidate.to_string(),
                    headers: headers.clone(),
                    output: base.with_extension("bin"),
                    connections: 1,
                    chunk_bytes: 1024 * 1024,
                    total: 0,
                    sequential: true,
                    resume_from: 0,
                    proxy: proxy.clone(),
                    resource_key: candidate.to_string(),
                    etag: String::new(),
                    last_modified: String::new(),
                    control: base.with_extension("control"),
                    progress: base.with_extension("progress"),
                    method: "GET".into(),
                    body_path: PathBuf::new(),
                    mirrors: Vec::new(),
                    replay_json: String::new(),
                };
                let size = crate::http_engine::probe_resource(&job)
                    .ok()
                    .and_then(|probe| probe.total)
                    .unwrap_or(0);
                Some(crate::HarvestCandidate {
                    url: candidate.to_string(),
                    size,
                    ..Default::default()
                })
            })
            .collect();
        return coordinator.lock()?.emit(CoreEvent::HarvestProbeResult {
            url: url.to_string(),
            links,
        });
    }
    let (_, _, _, harvest) = crate::recognize::probe_with_harvest_context(url, &headers, &proxy)?;
    coordinator.lock()?.emit(CoreEvent::HarvestResult {
        url: url.to_string(),
        links: harvest
            .into_iter()
            .map(|link| crate::HarvestCandidate {
                url: link.url,
                filename: link.filename,
                extension: link.extension,
                category: link.category,
                size: link.size_hint,
            })
            .collect(),
    })
}

fn push_task_tvbox(
    coordinator: &CoreCoordinator,
    task_id: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let loopback = mount_task_url(coordinator, task_id)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let server = shared_media()?;
    server.enable_lan();
    let host = crate::cast::primary_lan_ipv4()
        .map(|ip| ip.to_string())
        .ok_or_else(|| "没有可用于 TVBox 的局域网地址".to_string())?;
    let url = crate::cast::lan_media_url(server, &token, &host)?;
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let endpoint = coordinator
        .lock()?
        .store()
        .setting_string("tvbox_endpoint", "")?;
    if endpoint.trim().is_empty() {
        return Err("请先在设置里填写 TVBox 地址".into());
    }
    let title = if spec.title.is_empty() {
        spec.filename
    } else {
        spec.title
    };
    if let Err(error) = crate::cast::push_tvbox(&endpoint, &url, &title) {
        server.unmount(&token);
        return Err(error);
    }
    crate::cast::remember_tvbox(&endpoint);
    remember_cast_mount(&token);
    coordinator.lock()?.emit(CoreEvent::CastSession {
        active: true,
        title,
        device: format!("TVBox · {endpoint}"),
        status: "已推送到 TVBox".into(),
        task_id: task_id.to_string(),
        media_url: url,
        device_kind: "tvbox".into(),
        supported_actions: vec!["stop".into()],
        playing: false,
        paused: false,
        position_seconds: 0,
        duration_seconds: 0,
        position_available: false,
    })
}

fn discover_cast(coordinator: &CoreCoordinator, mode: &str) -> Result<Vec<EventEnvelope>, String> {
    let timeout = Duration::from_millis(2500);
    #[cfg(test)]
    let timeout = if std::env::var_os("HLS_V7_CAST_NULL").is_some() {
        Duration::from_millis(1)
    } else {
        timeout
    };
    let normalized_mode = if mode.eq_ignore_ascii_case("tvbox") {
        "tvbox"
    } else if mode.eq_ignore_ascii_case("cast") {
        "cast"
    } else {
        ""
    };
    let mut devices = crate::cast::discover_devices_for_mode(timeout, normalized_mode)?;
    if normalized_mode != "cast" {
        if let Ok(endpoint) = coordinator
            .lock()
            .and_then(|core| core.store().setting_string("tvbox_endpoint", ""))
        {
            if !endpoint.trim().is_empty() {
                devices.insert(
                    0,
                    crate::CastDeviceInfo {
                        id: "tvbox:configured".into(),
                        label: format!("TVBox · {endpoint}"),
                        location: endpoint.clone(),
                        control_url: endpoint,
                        service_type: "tvbox".into(),
                    },
                );
            }
        }
    }
    let message = if devices.is_empty() {
        if normalized_mode == "tvbox" {
            "没有发现 TVBox 接收端，可填写手工地址".to_string()
        } else {
            "没有发现 DLNA / Chromecast，仍可发布局域网播放地址".to_string()
        }
    } else {
        format!(
            "发现 {} 个{}",
            devices.len(),
            if normalized_mode == "tvbox" {
                "TVBox 接收端"
            } else {
                "投屏设备"
            }
        )
    };
    crate::cast::remember_devices(devices.clone());
    let mut core = coordinator.lock()?;
    let mut events = core.emit(CoreEvent::CastDevices { devices })?;
    events.extend(core.emit(CoreEvent::Toast {
        level: "info".into(),
        message,
    })?);
    Ok(events)
}

fn download_update(coordinator: &CoreCoordinator) -> Result<Vec<EventEnvelope>, String> {
    let info = match crate::updater::last_update() {
        Some(info) => info,
        None => crate::updater::check_for_update(crate::updater::CURRENT_VERSION)?,
    };
    if !info.newer {
        return coordinator.lock()?.emit(CoreEvent::UpdateCurrent {
            current: info.current,
        });
    }
    let path = crate::updater::download_installer(&info)?;
    let identity = crate::updater::verify_installer_identity(&path, &info.latest)?;
    coordinator.lock()?.emit(CoreEvent::UpdateReady {
        latest: info.latest,
        installer_path: path.display().to_string(),
        sha256: info.expected_sha256,
        product_name: identity.product_name,
        product_version: identity.product_version,
        upgrade_code: identity.upgrade_code,
    })
}

fn install_update(
    coordinator: &CoreCoordinator,
    workbench_pid: u32,
) -> Result<Vec<EventEnvelope>, String> {
    let info = crate::updater::last_update()
        .ok_or_else(|| "没有已确认的新版本，请重新检查更新".to_string())?;
    if !info.newer {
        return Err("当前已经是最新版本".into());
    }
    let path = crate::updater::download_installer(&info)?;
    crate::updater::verify_installer_identity(&path, &info.latest)?;
    coordinator.prepare_for_update(Duration::from_secs(15))?;
    let launch = match crate::updater::launch_update_helper(&path, &info.latest, workbench_pid) {
        Ok(launch) => launch,
        Err(error) => {
            coordinator.update_shutdown.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    coordinator.lock()?.emit(CoreEvent::UpdateInstallStarted {
        latest: info.latest,
        install_log: launch.log_path.display().to_string(),
        result_path: launch.result_path.display().to_string(),
    })
}

fn open_completed(
    coordinator: &CoreCoordinator,
    task_id: &str,
    folder: bool,
) -> Result<(), String> {
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let paths = TaskPaths::for_task(task_id, &spec)?;
    let published = resolve_published(&paths);
    let target = if folder {
        if published.is_dir() {
            published
        } else {
            published
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or(published)
        }
    } else {
        published
    };
    open_path(&target)
}

fn copy_completed_file(
    coordinator: &CoreCoordinator,
    task_id: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let paths = TaskPaths::for_task(task_id, &spec)?;
    let published = resolve_published(&paths);
    crate::write_clipboard_files(&[published.clone()])?;
    coordinator.lock()?.emit(CoreEvent::Toast {
        level: "copy_file".into(),
        message: format!("已复制文件 {}", published.display()),
    })
}

fn drag_completed_file(
    coordinator: &CoreCoordinator,
    task_id: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let paths = TaskPaths::for_task(task_id, &spec)?;
    let published = resolve_published(&paths);
    crate::completed_file_drag(&published)?;
    coordinator.lock()?.emit(CoreEvent::Toast {
        level: "drag_file".into(),
        message: format!("可拖到资源管理器 {}", published.display()),
    })
}

fn set_task_speed(
    coordinator: &CoreCoordinator,
    task_id: &str,
    kib: u32,
) -> Result<Vec<EventEnvelope>, String> {
    let mut spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    spec.speed_limit_kib = kib;
    coordinator.lock()?.replace_spec(task_id, spec)?;
    crate::net_policy::configure_scoped_limit(&format!("task:{task_id}"), u64::from(kib));
    coordinator.lock()?.emit(CoreEvent::Toast {
        level: "speed".into(),
        message: if kib == 0 {
            "已取消任务限速".into()
        } else {
            format!("任务限速 {kib} KiB/s")
        },
    })
}

fn refresh_task_url(
    coordinator: &CoreCoordinator,
    task_id: &str,
    url: &str,
) -> Result<Vec<EventEnvelope>, String> {
    reject_task_url(url)?;
    let mut spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    spec.url = url.trim().to_string();
    coordinator.lock()?.replace_spec(task_id, spec)?;
    coordinator.lock()?.emit(CoreEvent::Toast {
        level: "refresh".into(),
        message: "已更新下载地址".into(),
    })
}

fn refresh_task_request(
    coordinator: &CoreCoordinator,
    task_id: &str,
    url: &str,
    cookie: &str,
    auto_resume: bool,
) -> Result<Vec<EventEnvelope>, String> {
    reject_task_url(url)?;
    if cookie.len() > 16 * 1024 || cookie.contains(['\r', '\n', '\0']) {
        return Err("Cookie 格式无效或长度超过 16 KiB".into());
    }
    let current = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let next_action = coordinator
        .tasks()?
        .into_iter()
        .find(|task| task.task_id == task_id)
        .and_then(|task| {
            ["resume", "retry", "start"]
                .into_iter()
                .find(|candidate| task.available_actions.iter().any(|item| item == candidate))
        });
    let mut spec = current.clone();
    drop_cross_origin_task_secrets(&mut spec, &current.url, url);
    spec.url = url.trim().to_string();
    if !cookie.trim().is_empty() {
        spec.headers.insert("Cookie".into(), cookie.trim().into());
        spec = seal_spec_secrets(coordinator, spec)?;
    }
    coordinator.lock()?.replace_spec(task_id, spec)?;
    let mut events = coordinator.lock()?.emit(CoreEvent::Toast {
        level: "refresh".into(),
        message: if cookie.trim().is_empty() {
            "已更新下载地址".into()
        } else {
            "已更新下载地址和站点凭据".into()
        },
    })?;
    if auto_resume {
        if let Some(action) = next_action {
            events.extend(coordinator.dispatch_inner(CoreCommand::TaskAction {
                task_id: task_id.to_string(),
                action: action.into(),
            })?);
        }
    }
    Ok(events)
}

fn open_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("打开目标为空".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let file: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let operation: Vec<u16> = "open\0".encode_utf16().collect();
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
            )
        };
        if result as isize <= 32 {
            return Err(format!("打开文件失败 ({})", result as isize));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("当前系统不支持打开文件".into())
    }
}

fn player_control(action: &str) -> Result<(), String> {
    let player = shared_player()?;
    match action {
        "pause" => player.pause(true),
        "resume" | "play" => player.pause(false),
        "fullscreen" => player.set_fullscreen(true),
        "windowed" => player.set_fullscreen(false),
        "pip" => player.set_pip(true),
        "unpip" => player.set_pip(false),
        "stop" => {
            player.stop();
            Ok(())
        }
        "vol_up" => player.adjust_volume(10.0),
        "vol_down" => player.adjust_volume(-10.0),
        "seek_fwd" => player.seek_relative(10.0),
        "seek_back" => player.seek_relative(-10.0),
        other if other.starts_with("speed:") => {
            let speed = other
                .trim_start_matches("speed:")
                .parse::<f64>()
                .unwrap_or(1.0);
            player.set_speed(speed)
        }
        other if other.starts_with("preview:") => {
            let percent = other
                .trim_start_matches("preview:")
                .parse::<f64>()
                .unwrap_or(0.0);
            player.preview_percent(percent)
        }
        other if other.starts_with("audio:") => {
            player.set_audio_track(other.trim_start_matches("audio:"))
        }
        other if other.starts_with("subtitle:") => {
            player.set_subtitle_track(other.trim_start_matches("subtitle:"))
        }
        other if other.starts_with("embed_hwnd:") => {
            let rest = other.trim_start_matches("embed_hwnd:");
            let (hwnd_text, rect) = rest.split_once(':').unwrap_or((rest, "0,48,720,220"));
            let parent = hwnd_text.parse::<i64>().unwrap_or(0);
            let mut parts = rect.split(',').filter_map(|item| item.parse::<i32>().ok());
            let x = parts.next().unwrap_or(0);
            let y = parts.next().unwrap_or(48);
            let w = parts.next().unwrap_or(720);
            let h = parts.next().unwrap_or(220);
            player.attach_embed_hwnd(parent, x, y, w, h)
        }
        other if other.starts_with("embed_host:") => {
            let mut parts = other
                .trim_start_matches("embed_host:")
                .split(',')
                .filter_map(|item| item.parse::<i32>().ok());
            let x = parts.next().unwrap_or(0);
            let y = parts.next().unwrap_or(48);
            let w = parts.next().unwrap_or(720);
            let h = parts.next().unwrap_or(220);
            player.attach_embed_host(crate::player::PLAYER_WINDOW_TITLE, x, y, w, h)
        }
        _ => Err(format!("unknown player action {action}")),
    }
}

fn player_control_events(
    coordinator: &CoreCoordinator,
    action: &str,
) -> Result<Vec<EventEnvelope>, String> {
    player_control(action)?;
    let state = {
        let mut state = player_ui_state()
            .lock()
            .map_err(|_| "player state lock".to_string())?;
        match action {
            "pause" => {
                state.paused = true;
                state.status = "PAUSED".into();
            }
            "resume" | "play" => {
                state.active = true;
                state.paused = false;
                state.status = "PLAYING".into();
            }
            "stop" => {
                state.active = false;
                state.paused = false;
                state.status = "STOPPED".into();
            }
            "fullscreen" => state.status = "FULLSCREEN".into(),
            "windowed" => state.status = "PLAYING".into(),
            "pip" => state.status = "PIP".into(),
            "unpip" => state.status = "PLAYING".into(),
            other if other.starts_with("speed:") => {
                state.speed = other
                    .trim_start_matches("speed:")
                    .parse::<f64>()
                    .unwrap_or(1.0)
                    .clamp(0.25, 4.0);
                state.status = if state.paused {
                    "PAUSED".into()
                } else {
                    "PLAYING".into()
                };
            }
            _ => {}
        }
        state.clone()
    };
    emit_player_session(coordinator, &state)
}

fn shared_media() -> Result<&'static crate::playback::MediaServer, String> {
    static SERVER: std::sync::OnceLock<Result<crate::playback::MediaServer, String>> =
        std::sync::OnceLock::new();
    match SERVER.get_or_init(crate::playback::MediaServer::start) {
        Ok(server) => Ok(server),
        Err(error) => Err(error.clone()),
    }
}

fn shared_player() -> Result<&'static crate::player::Player, String> {
    static PLAYER: std::sync::OnceLock<crate::player::Player> = std::sync::OnceLock::new();
    Ok(PLAYER.get_or_init(crate::player::Player::default))
}

#[derive(Debug)]
struct Progress {
    downloaded: u64,
    total: u64,
    speed: u64,
}

fn read_progress(path: &Path) -> Option<Progress> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    Some(Progress {
        downloaded: value.get("downloaded")?.as_u64()?,
        total: value.get("total").and_then(Value::as_u64).unwrap_or(0),
        speed: value
            .get("speed")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .max(0.0) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ResourceKind, TaskSpec};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn spec() -> TaskSpec {
        TaskSpec {
            url: "https://example.test/path/file.bin".into(),
            resource_kind: ResourceKind::File,
            title: "File".into(),
            filename: "../bad:name?.bin".into(),
            download_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            request_method: "GET".into(),
            credential_ref: None,
            replay_context_ref: None,
            concurrency: 4,
            checksum: None,
            expected_size: Some(100),
            etag: String::new(),
            last_modified: String::new(),
            ..Default::default()
        }
    }

    fn serve_harvest_response(response: String) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.is_empty() {
                        break;
                    }
                    request.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let _ = sender.send(request);
                let mut stream = reader.into_inner();
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{address}"), receiver)
    }

    #[test]
    fn harvest_page_sends_referer_and_returns_resource_metadata() {
        let body = r#"<!doctype html><a href="/media/movie.mp4">Movie</a>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let (origin, request) = serve_harvest_response(response);
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let events = coordinator
            .dispatch(CoreCommand::HarvestPage {
                url: format!("{origin}/watch"),
                referer: "https://site.test/watch/42".into(),
                probe_urls: Vec::new(),
            })
            .unwrap();
        let links = events
            .iter()
            .find_map(|event| match &event.event {
                CoreEvent::HarvestResult { links, .. } => Some(links),
                _ => None,
            })
            .expect("harvest result");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].filename, "movie.mp4");
        assert_eq!(links[0].extension, "mp4");
        let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            request
                .to_ascii_lowercase()
                .contains("referer: https://site.test/watch/42\r\n"),
            "request did not preserve Referer: {request:?}"
        );
    }

    #[test]
    fn harvest_size_probe_is_bounded_and_skips_invalid_candidates() {
        let response = concat!(
            "HTTP/1.1 206 Partial Content\r\n",
            "Content-Range: bytes 0-0/4096\r\n",
            "Content-Length: 1\r\n",
            "Accept-Ranges: bytes\r\n",
            "Connection: close\r\n\r\n",
            "x"
        )
        .to_string();
        let (origin, request) = serve_harvest_response(response);
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let candidate = format!("{origin}/movie.mp4");
        let events = coordinator
            .dispatch(CoreCommand::HarvestPage {
                url: format!("{origin}/watch"),
                referer: format!("{origin}/watch"),
                probe_urls: vec![candidate.clone(), "javascript:alert(1)".into()],
            })
            .unwrap();
        let links = events
            .iter()
            .find_map(|event| match &event.event {
                CoreEvent::HarvestProbeResult { links, .. } => Some(links),
                _ => None,
            })
            .expect("harvest probe result");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, candidate);
        assert_eq!(links[0].size, 4096);
        let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("range: bytes=0-0\r\n"));
        assert!(lower.contains(&format!("referer: {origin}/watch\r\n").to_ascii_lowercase()));

        let error = coordinator
            .dispatch(CoreCommand::HarvestPage {
                url: format!("{origin}/watch"),
                referer: String::new(),
                probe_urls: (0..101)
                    .map(|index| format!("https://cdn.test/{index}.bin"))
                    .collect(),
            })
            .unwrap_err();
        assert!(error.contains("100"));
    }

    #[test]
    fn task_paths_keep_payload_and_final_output_separate() {
        let paths = TaskPaths::for_task("task-1", &spec()).unwrap();
        assert!(paths.output.ends_with("payload.downloading"));
        assert!(paths.final_output.ends_with("_bad_name_.bin"));
        assert_ne!(paths.output, paths.final_output);
        assert_eq!(
            safe_filename("report&calc.exe", "https://cdn.test/a.bin"),
            "report_calc.exe"
        );
        assert_eq!(
            safe_filename("a%PATH%.txt", "https://cdn.test/a.bin"),
            "a_PATH_.txt"
        );
        assert!(!safe_filename("evil\nnotepad.exe", "https://cdn.test/a.bin").contains('\n'));
        assert_eq!(safe_filename("CON", "https://cdn.test/a.bin"), "_CON");
        assert_eq!(
            safe_filename("con.txt", "https://cdn.test/a.bin"),
            "_con.txt"
        );
        assert_eq!(
            safe_filename("COM1.dat", "https://cdn.test/a.bin"),
            "_COM1.dat"
        );
        assert_eq!(safe_filename("NUL", "https://cdn.test/a.bin"), "_NUL");
    }

    #[test]
    fn task_paths_prepare_preserves_an_active_pause_or_cancel_request() {
        let root = std::env::temp_dir().join(format!(
            "hls-v7-control-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut task = spec();
        task.download_dir = root.to_string_lossy().into_owned();
        let paths = TaskPaths::for_task("task-1", &task).unwrap();
        paths.prepare().unwrap();
        for control in ["pause", "cancel"] {
            paths.set_control(control).unwrap();
            paths.prepare().unwrap();
            assert_eq!(fs::read_to_string(&paths.control).unwrap(), control);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_like_actions_wait_then_keep_future_tasks_queued() {
        for (terminal_status, action, old_control) in [
            ("paused", "resume", "pause"),
            ("canceled", "retry", "cancel"),
        ] {
            let root = std::env::temp_dir().join(format!(
                "hls-v7-worker-restart-{}-{}-{action}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
            coordinator
                .set_setting("legal_terms_accepted", serde_json::json!(true))
                .unwrap();
            coordinator
                .set_setting(
                    "download_dir",
                    serde_json::json!(root.to_string_lossy().into_owned()),
                )
                .unwrap();
            coordinator
                .dispatch(CoreCommand::CreateTask {
                    spec: TaskSpec {
                        url: "https://cdn.test/restart.bin".into(),
                        filename: "restart.bin".into(),
                        scheduled_start_at: "2999-01-01T00:00:00Z".into(),
                        ..Default::default()
                    },
                })
                .unwrap();
            coordinator
                .lock()
                .unwrap()
                .handle(CoreCommand::UpdateProgress {
                    task_id: "task-1".into(),
                    downloaded_bytes: 7,
                    total_bytes: Some(10),
                    speed_bytes_per_sec: 0,
                    stage: "waiting".into(),
                    status: terminal_status.into(),
                })
                .unwrap();
            let paths = TaskPaths::for_task(
                "task-1",
                coordinator.lock().unwrap().task_spec("task-1").unwrap(),
            )
            .unwrap();
            paths.prepare().unwrap();
            paths.set_control(old_control).unwrap();
            coordinator.active.lock().unwrap().insert("task-1".into());

            let (wait_tx, wait_rx) = mpsc::channel();
            *coordinator.worker_wait_started.lock().unwrap() = Some(wait_tx);
            let runner = coordinator.clone();
            let action = action.to_string();
            let dispatch = thread::spawn(move || {
                runner.dispatch(CoreCommand::TaskAction {
                    task_id: "task-1".into(),
                    action,
                })
            });
            wait_rx.recv_timeout(Duration::from_secs(2)).unwrap();

            assert_eq!(fs::read_to_string(&paths.control).unwrap(), old_control);
            assert_eq!(coordinator.tasks().unwrap()[0].status, terminal_status);

            coordinator
                .lock()
                .unwrap()
                .handle(CoreCommand::UpdateProgress {
                    task_id: "task-1".into(),
                    downloaded_bytes: 7,
                    total_bytes: Some(10),
                    speed_bytes_per_sec: 0,
                    stage: "waiting".into(),
                    status: terminal_status.into(),
                })
                .unwrap();
            coordinator.active.lock().unwrap().remove("task-1");
            dispatch.join().unwrap().unwrap();

            assert_eq!(coordinator.tasks().unwrap()[0].status, "queued");
            assert_eq!(fs::read_to_string(&paths.control).unwrap(), old_control);
            assert!(!coordinator.worker_is_active("task-1").unwrap());
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn paused_or_canceled_control_blocks_publish() {
        let control =
            std::env::temp_dir().join(format!("hls-v7-publish-control-{}", std::process::id()));
        for (value, expected) in [("pause", "paused"), ("cancel", "canceled")] {
            fs::write(&control, value).unwrap();
            assert_eq!(ensure_publish_allowed(&control).unwrap_err(), expected);
        }
        let _ = fs::remove_file(control);
    }

    #[test]
    fn illegal_start_like_action_does_not_reset_worker_control() {
        let root = std::env::temp_dir().join(format!(
            "hls-v7-illegal-worker-start-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "download_dir",
                serde_json::json!(root.to_string_lossy().into_owned()),
            )
            .unwrap();
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/queued.bin".into(),
                    filename: "queued.bin".into(),
                    scheduled_start_at: "2999-01-01T00:00:00Z".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let paths = TaskPaths::for_task(
            "task-1",
            coordinator.lock().unwrap().task_spec("task-1").unwrap(),
        )
        .unwrap();
        paths.prepare().unwrap();
        paths.set_control("pause").unwrap();

        let events = coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "resume".into(),
            })
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            &event.event,
            CoreEvent::Error { code, .. } if code == "illegal_task_action"
        )));
        assert_eq!(coordinator.tasks().unwrap()[0].status, "queued");
        assert_eq!(fs::read_to_string(&paths.control).unwrap(), "pause");
        assert!(!coordinator.worker_is_active("task-1").unwrap());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_in_a_disabled_queue_stays_queued_without_resetting_control() {
        let root = std::env::temp_dir().join(format!(
            "hls-v7-disabled-queue-start-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "download_dir",
                serde_json::json!(root.to_string_lossy().into_owned()),
            )
            .unwrap();
        coordinator
            .set_setting(
                "queue_profiles",
                serde_json::json!([{
                    "id": "default",
                    "name": "默认队列",
                    "enabled": false,
                    "priority": 0,
                    "max_active": 3,
                    "speed_limit_kib": 0,
                    "schedule_enabled": false,
                    "start_time": "00:00",
                    "stop_time": "23:59",
                    "active_days": "1,2,3,4,5,6,7",
                    "completion_action": "none"
                }]),
            )
            .unwrap();
        coordinator
            .lock()
            .unwrap()
            .handle(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/disabled.bin".into(),
                    filename: "disabled.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let paths = TaskPaths::for_task(
            "task-1",
            coordinator.lock().unwrap().task_spec("task-1").unwrap(),
        )
        .unwrap();
        paths.prepare().unwrap();
        paths.set_control("pause").unwrap();

        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "start".into(),
            })
            .unwrap();

        assert_eq!(coordinator.tasks().unwrap()[0].status, "queued");
        assert_eq!(fs::read_to_string(&paths.control).unwrap(), "pause");
        assert!(!coordinator.worker_is_active("task-1").unwrap());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_reuses_an_active_worker_without_waiting_for_it() {
        let root = std::env::temp_dir().join(format!(
            "hls-v7-active-start-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "download_dir",
                serde_json::json!(root.to_string_lossy().into_owned()),
            )
            .unwrap();
        let spec = coordinator
            .apply_defaults_to_spec(TaskSpec {
                url: "https://cdn.test/active-start.bin".into(),
                filename: "active-start.bin".into(),
                ..Default::default()
            })
            .unwrap();
        coordinator.dispatch_created(spec).unwrap();
        coordinator.active.lock().unwrap().insert("task-1".into());

        let (result_tx, result_rx) = mpsc::channel();
        let runner = coordinator.clone();
        let dispatch = thread::spawn(move || {
            let _ = result_tx.send(runner.dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "start".into(),
            }));
        });
        let result = match result_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result,
            Err(error) => {
                coordinator.active.lock().unwrap().remove("task-1");
                dispatch.join().unwrap();
                panic!("start waited for the existing worker: {error}");
            }
        };
        result.unwrap();
        dispatch.join().unwrap();

        assert_eq!(coordinator.tasks().unwrap()[0].status, "downloading");
        assert!(coordinator.worker_is_active("task-1").unwrap());
        coordinator.active.lock().unwrap().remove("task-1");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn advanced_defaults_are_applied_and_host_scope_is_enforced() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_settings(BTreeMap::from([
                ("legal_terms_accepted".into(), serde_json::json!(true)),
                ("temp_dir".into(), serde_json::json!(r"D:\HLS\Cache")),
                (
                    "default_origin".into(),
                    serde_json::json!("https://player.example.test"),
                ),
                ("allowed_hosts".into(), serde_json::json!("*.example.test")),
            ]))
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.example.test/video.mp4".into(),
                    filename: "video.mp4".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let task_id = events
            .iter()
            .find_map(|event| match &event.event {
                CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            })
            .unwrap();
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec(&task_id)
            .cloned()
            .unwrap();
        assert_eq!(spec.work_dir, r"D:\HLS\Cache");
        assert_eq!(
            spec.headers.get("Origin").map(String::as_str),
            Some("https://player.example.test")
        );
        let error = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://outside.invalid/video.mp4".into(),
                    filename: "blocked.mp4".into(),
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(error.contains("允许的域名"));
    }

    #[test]
    fn default_cookie_is_protected_and_attached_by_reference_only() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator.set_default_cookie("session=private").unwrap();
        assert!(coordinator.default_cookie_configured().unwrap());
        let spec = coordinator
            .apply_defaults_to_spec(TaskSpec {
                url: "https://example.test/file.bin".into(),
                filename: "file.bin".into(),
                ..TaskSpec::default()
            })
            .unwrap();
        assert_eq!(
            spec.credential_ref.as_deref(),
            Some(DEFAULT_COOKIE_CREDENTIAL_REF)
        );
        assert!(!serde_json::to_string(&spec)
            .unwrap()
            .contains("session=private"));
        let blob = coordinator
            .load_credential(DEFAULT_COOKIE_CREDENTIAL_REF)
            .unwrap()
            .unwrap();
        assert!(blob.starts_with("dpapi:"));
        assert!(!blob.contains("session=private"));
        coordinator.set_default_cookie("").unwrap();
        assert!(!coordinator.default_cookie_configured().unwrap());
        assert!(coordinator
            .set_default_cookie("a=b\r\nX-Test: yes")
            .is_err());
    }

    #[test]
    fn new_task_paths_use_the_version_neutral_work_directory() {
        let root = std::env::temp_dir().join(format!(
            "hls-task-path-current-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut task = spec();
        task.download_dir = root.to_string_lossy().into_owned();
        let paths = TaskPaths::for_task("new-task", &task).unwrap();
        assert!(paths
            .task_dir()
            .ends_with(Path::new(".hls-tasks").join("new-task")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn existing_legacy_task_directory_is_resumed_in_place() {
        let root = std::env::temp_dir().join(format!(
            "hls-task-path-legacy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = root.join(".v6-tasks").join("restored-task");
        fs::create_dir_all(&legacy).unwrap();
        let mut task = spec();
        task.download_dir = root.to_string_lossy().into_owned();
        let paths = TaskPaths::for_task("restored-task", &task).unwrap();
        assert_eq!(paths.task_dir(), legacy);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn get_job_without_size_is_not_forced_sequential() {
        let mut spec = spec();
        spec.expected_size = None;
        let (job, paths) = build_job("task-probe", &spec).unwrap();
        assert!(!job.sequential);
        assert_eq!(job.total, 0);
        spec.request_method = "POST".into();
        let (post_job, post_paths) = build_job("task-post", &spec).unwrap();
        assert!(post_job.sequential);
        let _ = fs::remove_dir_all(paths.task_dir());
        let _ = fs::remove_dir_all(post_paths.task_dir());
    }

    #[test]
    fn coordinator_runs_http_task_and_atomically_publishes_output() {
        let body: &'static [u8] = b"v6-core-http-fixture";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut stream = reader.into_inner();
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() - 1,
                    body.len(),
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        });

        let download_dir = std::env::temp_dir().join(format!(
            "hls-v6-worker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&download_dir).unwrap();
        fs::write(download_dir.join("fixture.bin"), b"existing").unwrap();
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "download_dir",
                serde_json::json!(download_dir.to_string_lossy()),
            )
            .unwrap();
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: format!("http://{address}/fixture.bin"),
                    resource_kind: ResourceKind::File,
                    title: "Fixture".into(),
                    filename: "fixture.bin".into(),
                    download_dir: download_dir.to_string_lossy().into_owned(),
                    request_method: "GET".into(),
                    credential_ref: None,
                    replay_context_ref: None,
                    concurrency: 1,
                    checksum: None,
                    expected_size: Some(body.len() as u64),
                    etag: String::new(),
                    last_modified: String::new(),
                    ..Default::default()
                },
            })
            .unwrap();
        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "start".into(),
            })
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let published = loop {
            let task = coordinator
                .tasks()
                .unwrap()
                .into_iter()
                .find(|task| task.task_id == "task-1")
                .unwrap();
            if task.status == "completed" {
                break PathBuf::from(task.output_path);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "task did not complete: {task:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        assert_eq!(
            fs::read(download_dir.join("fixture.bin")).unwrap(),
            b"existing"
        );
        assert_eq!(published.file_name().unwrap(), "fixture_1.bin");
        assert_eq!(fs::read(&published).unwrap(), body);
        let _ = fs::remove_dir_all(download_dir);
    }

    #[test]
    fn refreshed_url_resumes_paused_task_and_publishes_new_response() {
        let body: &'static [u8] = b"refreshed-signed-url";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut stream = reader.into_inner();
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() - 1,
                    body.len(),
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        });

        let download_dir = std::env::temp_dir().join(format!(
            "hls-v7-refresh-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "download_dir",
                serde_json::json!(download_dir.to_string_lossy()),
            )
            .unwrap();
        coordinator
            .dispatch_created(TaskSpec {
                url: "https://expired.test/signed.bin".into(),
                resource_kind: ResourceKind::File,
                filename: "refreshed.bin".into(),
                download_dir: download_dir.to_string_lossy().into_owned(),
                expected_size: Some(body.len() as u64),
                concurrency: 1,
                ..Default::default()
            })
            .unwrap();
        let original = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .cloned()
            .unwrap();
        TaskPaths::for_task("task-1", &original)
            .unwrap()
            .prepare()
            .unwrap();
        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "pause".into(),
            })
            .unwrap();

        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: format!("refresh:http://{address}/signed.bin"),
            })
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let task = coordinator
                .tasks()
                .unwrap()
                .into_iter()
                .find(|task| task.task_id == "task-1")
                .unwrap();
            if task.status == "completed" {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "refreshed task did not resume: {}",
                task.status
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let refreshed = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .cloned()
            .unwrap();
        let output = TaskPaths::for_task("task-1", &refreshed)
            .unwrap()
            .final_output;
        assert_eq!(fs::read(output).unwrap(), body);
        let _ = fs::remove_dir_all(download_dir);
    }

    #[test]
    fn structured_request_refresh_vaults_cookie_and_never_puts_it_in_snapshot() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .dispatch_created(TaskSpec {
                url: "https://cdn.test/expired.bin".into(),
                resource_kind: ResourceKind::File,
                filename: "signed.bin".into(),
                ..Default::default()
            })
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::RefreshTaskRequest {
                task_id: "task-1".into(),
                url: "https://cdn.test/refreshed.bin?signature=new".into(),
                cookie: "session=private".into(),
                auto_resume: false,
            })
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.event,
            CoreEvent::Toast { message, .. } if message.contains("站点凭据")
        )));
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .cloned()
            .unwrap();
        assert_eq!(spec.url, "https://cdn.test/refreshed.bin?signature=new");
        assert!(!spec
            .headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("cookie")));
        let reference = spec.credential_ref.expect("cookie should be vaulted");
        let protected = coordinator.load_credential(&reference).unwrap().unwrap();
        let plain = CredentialVault.unprotect(&protected).unwrap_or(protected);
        assert!(plain.contains("session=private"));
        let snapshot = coordinator.tasks().unwrap().remove(0);
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("session=private"));
    }

    #[test]
    fn task_torrent_file_contract_reads_and_persists_selection_before_start() {
        let root = std::env::temp_dir().join(format!("hls-v7-task-torrent-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("multi.torrent");
        let bytes = b"d4:infod5:filesld6:lengthi3e4:pathl7:one.bineed6:lengthi2e4:pathl3:dir7:two.bineee4:name4:demo12:piece lengthi4e6:pieces40:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaee";
        let parsed = crate::torrent_engine::parse_torrent_file(bytes).unwrap();
        assert_eq!(parsed.files.len(), 2);
        fs::write(&source, bytes).unwrap();

        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting("download_dir", serde_json::json!(root.to_string_lossy()))
            .unwrap();
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: source.to_string_lossy().into_owned(),
                    resource_kind: ResourceKind::Torrent,
                    filename: String::new(),
                    download_dir: root.to_string_lossy().into_owned(),
                    ..Default::default()
                },
            })
            .unwrap();

        let listed = coordinator
            .dispatch(CoreCommand::GetTaskTorrentFiles {
                task_id: "task-1".into(),
            })
            .unwrap();
        assert!(listed.iter().any(|event| matches!(
            &event.event,
            CoreEvent::TaskTorrentFiles { files, selections, total_size, .. }
                if files.len() == 2 && selections.len() == 2 && *total_size == 5
        )));
        let selected = vec![
            crate::TorrentFileSelection {
                index: 0,
                path: "one.bin".into(),
                selected: false,
            },
            crate::TorrentFileSelection {
                index: 1,
                path: "dir/two.bin".into(),
                selected: true,
            },
        ];
        coordinator
            .dispatch(CoreCommand::SetTaskTorrentFiles {
                task_id: "task-1".into(),
                selections: selected.clone(),
            })
            .unwrap();
        let stored = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .unwrap()
            .torrent_selection
            .clone();
        assert_eq!(stored, selected);
        let stored_spec = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .unwrap()
            .clone();
        let paths = TaskPaths::for_task("task-1", &stored_spec).unwrap();
        let sidecar: Vec<crate::TorrentFileSelection> =
            serde_json::from_slice(&fs::read(paths.torrent_selection).unwrap()).unwrap();
        assert_eq!(sidecar, selected);
        let snapshot = coordinator.tasks().unwrap().remove(0);
        assert_eq!(snapshot.filename, "demo");
        assert_eq!(snapshot.total_bytes, Some(5));
        assert_eq!(snapshot.total_ranges, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_torrent_selection_update_cancels_requested_file_and_publishes_remaining_file() {
        use std::io::Read;
        use std::net::TcpStream;
        use std::sync::mpsc;

        let payload = b"aaaabbbb".to_vec();
        let pieces: Vec<[u8; 20]> = payload.chunks(4).map(crate::crypto_lite::sha1).collect();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "hls-v7-live-torrent-selection-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();

        let peer_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer_addr = peer_listener.local_addr().unwrap();
        let tracker_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let tracker_addr = tracker_listener.local_addr().unwrap();
        let announce = format!("http://{tracker_addr}/announce");

        let mut info = format!(
            "d5:filesld6:lengthi4e4:pathl7:one.bineed6:lengthi4e4:pathl7:two.bineee4:name4:demo12:piece lengthi4e6:pieces40:"
        )
        .into_bytes();
        for piece in &pieces {
            info.extend_from_slice(piece);
        }
        info.push(b'e');
        let mut torrent = format!("d8:announce{}:{}4:info", announce.len(), announce).into_bytes();
        torrent.extend_from_slice(&info);
        torrent.push(b'e');
        let source = root.join("demo.torrent");
        fs::write(&source, &torrent).unwrap();
        let parsed = crate::torrent_engine::parse_torrent_file(&torrent).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.length, payload.len() as u64);

        let tracker = std::thread::spawn(move || {
            let (stream, _) = tracker_listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            assert!(request.starts_with("GET /announce?"));
            let mut stream = reader.into_inner();
            let mut body = b"d5:peers6:".to_vec();
            body.extend_from_slice(&[
                127,
                0,
                0,
                1,
                (peer_addr.port() >> 8) as u8,
                peer_addr.port() as u8,
            ]);
            body.push(b'e');
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });

        let (requested_tx, requested_rx) = mpsc::channel();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let peer_payload = payload.clone();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = peer_listener.accept().unwrap();
            let mut handshake = [0u8; 68];
            stream.read_exact(&mut handshake).unwrap();
            stream.write_all(&handshake).unwrap();
            stream.write_all(&1u32.to_be_bytes()).unwrap();
            stream.write_all(&[1u8]).unwrap();

            let read_message = |stream: &mut TcpStream| -> Option<Vec<u8>> {
                let mut header = [0u8; 4];
                stream.read_exact(&mut header).ok()?;
                let length = u32::from_be_bytes(header) as usize;
                let mut message = vec![0u8; length];
                stream.read_exact(&mut message).ok()?;
                Some(message)
            };
            loop {
                let Some(message) = read_message(&mut stream) else {
                    return;
                };
                if message.len() < 13 || message[0] != 6 {
                    continue;
                }
                let index = u32::from_be_bytes(message[1..5].try_into().unwrap()) as usize;
                if index != 0 {
                    continue;
                }
                requested_tx.send(()).unwrap();
                loop {
                    let Some(message) = read_message(&mut stream) else {
                        return;
                    };
                    if message.first() == Some(&8) {
                        let canceled = (
                            u32::from_be_bytes(message[1..5].try_into().unwrap()),
                            u32::from_be_bytes(message[5..9].try_into().unwrap()),
                            u32::from_be_bytes(message[9..13].try_into().unwrap()),
                        );
                        cancel_tx.send(canceled).unwrap();
                        break;
                    }
                }
                loop {
                    let Some(message) = read_message(&mut stream) else {
                        return;
                    };
                    if message.len() < 13 || message[0] != 6 {
                        continue;
                    }
                    let index = u32::from_be_bytes(message[1..5].try_into().unwrap()) as usize;
                    let begin = u32::from_be_bytes(message[5..9].try_into().unwrap()) as usize;
                    let block = u32::from_be_bytes(message[9..13].try_into().unwrap()) as usize;
                    if index != 1 {
                        continue;
                    }
                    let start = index * 4 + begin;
                    let mut response = vec![7u8];
                    response.extend_from_slice(&(index as u32).to_be_bytes());
                    response.extend_from_slice(&(begin as u32).to_be_bytes());
                    response.extend_from_slice(&peer_payload[start..start + block]);
                    stream
                        .write_all(&(response.len() as u32).to_be_bytes())
                        .unwrap();
                    stream.write_all(&response).unwrap();
                    return;
                }
            }
        });

        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting("download_dir", serde_json::json!(root.to_string_lossy()))
            .unwrap();
        coordinator
            .set_setting("bt_enable_dht", serde_json::json!(false))
            .unwrap();
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: source.to_string_lossy().into_owned(),
                    resource_kind: ResourceKind::Torrent,
                    filename: String::new(),
                    download_dir: root.to_string_lossy().into_owned(),
                    ..Default::default()
                },
            })
            .unwrap();
        let initial = vec![
            crate::TorrentFileSelection {
                index: 0,
                path: "one.bin".into(),
                selected: true,
            },
            crate::TorrentFileSelection {
                index: 1,
                path: "two.bin".into(),
                selected: false,
            },
        ];
        coordinator
            .dispatch(CoreCommand::SetTaskTorrentFiles {
                task_id: "task-1".into(),
                selections: initial,
            })
            .unwrap();
        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "start".into(),
            })
            .unwrap();

        requested_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let in_flight = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-1")
            .unwrap();
        assert_eq!(in_flight.status, "downloading");
        let updated = vec![
            crate::TorrentFileSelection {
                index: 0,
                path: "one.bin".into(),
                selected: false,
            },
            crate::TorrentFileSelection {
                index: 1,
                path: "two.bin".into(),
                selected: true,
            },
        ];
        let events = coordinator
            .dispatch(CoreCommand::SetTaskTorrentFiles {
                task_id: "task-1".into(),
                selections: updated.clone(),
            })
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.event,
            CoreEvent::Toast { message, .. } if message.contains("BT 文件选择已更新")
        )));
        assert_eq!(
            cancel_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            (0, 0, 4)
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let completed = loop {
            let task = coordinator
                .tasks()
                .unwrap()
                .into_iter()
                .find(|task| task.task_id == "task-1")
                .unwrap();
            if task.status == "completed" {
                break task;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "live torrent task did not complete: {task:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        assert_eq!(completed.downloaded_bytes, 4);
        assert_eq!(completed.total_bytes, Some(4));
        let stored_spec = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .cloned()
            .unwrap();
        assert_eq!(stored_spec.torrent_selection, updated);
        let paths = TaskPaths::for_task("task-1", &stored_spec).unwrap();
        let sidecar: Vec<crate::TorrentFileSelection> =
            serde_json::from_slice(&fs::read(&paths.torrent_selection).unwrap()).unwrap();
        assert_eq!(sidecar, updated);
        assert_eq!(fs::read(&paths.output).unwrap(), b"\0\0\0\0bbbb");
        let published_dir = PathBuf::from(&completed.output_path);
        assert!(published_dir.starts_with(&root));
        assert_eq!(fs::read(published_dir.join("two.bin")).unwrap(), b"bbbb");
        assert!(!published_dir.join("one.bin").exists());

        peer.join().unwrap();
        tracker.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_torrent_selection_update_wins_over_task_initialization() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "hls-v7-torrent-selection-race-{}-{stamp}",
            std::process::id()
        ));
        let path = root.join("torrent-selection.json");
        let initial = vec![crate::TorrentFileSelection {
            index: 0,
            path: "one.bin".into(),
            selected: false,
        }];
        let updated = vec![crate::TorrentFileSelection {
            index: 0,
            path: "one.bin".into(),
            selected: true,
        }];
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let initialize_path = path.clone();
        let initialize_barrier = Arc::clone(&barrier);
        let initialize = std::thread::spawn(move || {
            initialize_barrier.wait();
            initialize_torrent_selection(&initialize_path, &initial)
        });
        let update_path = path.clone();
        let update_barrier = Arc::clone(&barrier);
        let expected = updated.clone();
        let update = std::thread::spawn(move || {
            update_barrier.wait();
            write_torrent_selection(&update_path, &expected)
        });

        barrier.wait();
        initialize.join().unwrap().unwrap();
        update.join().unwrap().unwrap();
        let stored: Vec<crate::TorrentFileSelection> =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored, updated);
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            1,
            "unique temporary files must be removed after publication"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn checksum_mismatch_does_not_publish_to_download_dir() {
        let body: &'static [u8] = b"v6-checksum-payload";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                }
                let mut stream = reader.into_inner();
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() - 1,
                    body.len(),
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        let download_dir = std::env::temp_dir().join(format!(
            "hls-v6-checksum-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "download_dir",
                serde_json::json!(download_dir.to_string_lossy()),
            )
            .unwrap();
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: format!("http://{address}/payload.bin"),
                    filename: "payload.bin".into(),
                    download_dir: download_dir.to_string_lossy().into_owned(),
                    checksum: Some(
                        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                            .into(),
                    ),
                    expected_size: Some(body.len() as u64),
                    concurrency: 1,
                    ..Default::default()
                },
            })
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let task = coordinator
                .tasks()
                .unwrap()
                .into_iter()
                .find(|task| task.task_id == "task-1")
                .unwrap();
            if task.status == "failed" {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "checksum mismatch did not fail: {task:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!download_dir.join("payload.bin").exists());
        let _ = fs::remove_dir_all(download_dir);
    }

    #[test]
    fn spec_from_url_drops_cross_origin_cookies() {
        let mut template = spec();
        template.url = "https://site.test/watch".into();
        template.headers.insert("Cookie".into(), "sid=1".into());
        template
            .headers
            .insert("Authorization".into(), "Bearer x".into());
        template
            .headers
            .insert("Referer".into(), "https://site.test/watch".into());
        template.credential_ref = Some("cred-page".into());
        let dirs = crate::category::CategoryDirs::default();
        let same = spec_from_url(
            &template,
            "https://site.test/clip.mp4",
            "clip.mp4",
            false,
            &dirs,
        );
        assert_eq!(same.headers.get("Cookie").unwrap(), "sid=1");
        assert_eq!(same.credential_ref.as_deref(), Some("cred-page"));
        let other = spec_from_url(
            &template,
            "https://cdn.test/clip.mp4",
            "clip.mp4",
            false,
            &dirs,
        );
        assert!(other.headers.get("Cookie").is_none());
        assert!(other.headers.get("Authorization").is_none());
        assert_eq!(
            other.headers.get("Referer").unwrap(),
            "https://site.test/watch"
        );
        assert!(other.credential_ref.is_none());
    }

    #[test]
    fn player_control_accepts_speed_and_pause_on_null_backend() {
        std::env::set_var("HLS_V7_PLAYER_NULL", "1");
        player_control("pause").unwrap();
        player_control("resume").unwrap();
        player_control("speed:1.5").unwrap();
        player_control("fullscreen").unwrap();
        player_control("pip").unwrap();
        player_control("unpip").unwrap();
        player_control("vol_up").unwrap();
        player_control("seek_back").unwrap();
        player_control("preview:42").unwrap();
        player_control("embed_host:0,48,640,200").unwrap();
        player_control("embed_hwnd:42:0,48,640,200").unwrap();
        player_control("stop").unwrap();
    }

    #[test]
    fn player_session_reports_play_pause_speed_and_stop() {
        std::env::set_var("HLS_V7_PLAYER_NULL", "1");
        let dir = std::env::temp_dir().join(format!("hls-player-session-{}", std::process::id()));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .dispatch_created(TaskSpec {
                url: "https://cdn.test/video.mp4".into(),
                resource_kind: ResourceKind::File,
                title: "测试影片".into(),
                filename: "video.mp4".into(),
                download_dir: dir.to_string_lossy().into_owned(),
                ..Default::default()
            })
            .unwrap();
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .cloned()
            .unwrap();
        let paths = TaskPaths::for_task("task-1", &spec).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&paths.final_output, b"local-media").unwrap();

        let play = coordinator
            .dispatch(CoreCommand::PlayTask {
                task_id: "task-1".into(),
            })
            .unwrap();
        assert!(matches!(
            play.last().map(|item| &item.event),
            Some(CoreEvent::PlayerSession { active: true, paused: false, title, .. }) if title == "测试影片"
        ));
        let pause = coordinator
            .dispatch(CoreCommand::PlayerControl {
                action: "pause".into(),
            })
            .unwrap();
        assert!(matches!(
            pause.last().map(|item| &item.event),
            Some(CoreEvent::PlayerSession { paused: true, status, .. }) if status == "PAUSED"
        ));
        let speed = coordinator
            .dispatch(CoreCommand::PlayerControl {
                action: "speed:1.5".into(),
            })
            .unwrap();
        assert!(matches!(
            speed.last().map(|item| &item.event),
            Some(CoreEvent::PlayerSession { speed, .. }) if (*speed - 1.5).abs() < f64::EPSILON
        ));
        let stop = coordinator
            .dispatch(CoreCommand::PlayerControl {
                action: "stop".into(),
            })
            .unwrap();
        assert!(matches!(
            stop.last().map(|item| &item.event),
            Some(CoreEvent::PlayerSession { active: false, status, .. }) if status == "STOPPED"
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn metalink_body_expands_to_http_task_with_mirrors() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: r#"<metalink><file name="demo.bin"><url priority="1">https://cdn.example.test/demo.bin</url><url priority="2">https://mirror.example.test/demo.bin</url></file></metalink>"#.into(),
                    resource_kind: ResourceKind::File,
                    filename: String::new(),
                    ..Default::default()
                },
            })
            .unwrap();
        let snapshot = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                _ => None,
            })
            .unwrap();
        assert_eq!(snapshot.filename, "demo.bin");
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec(&snapshot.task_id)
            .cloned()
            .unwrap();
        assert_eq!(spec.url, "https://cdn.example.test/demo.bin");
        assert_eq!(spec.mirrors, vec!["https://mirror.example.test/demo.bin"]);
    }

    #[test]
    fn metalink_query_string_is_not_treated_as_metalink_body() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "legal_terms_version",
                serde_json::json!(crate::LEGAL_TERMS_VERSION),
            )
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.example.test/demo.bin?<metalink><file name=\"x.bin\"><url>https://evil.test/malware.exe</url></file></metalink>".into(),
                    filename: "demo.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let id = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            })
            .unwrap();
        let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
        assert!(spec.url.starts_with("https://cdn.example.test/demo.bin"));
        assert_ne!(spec.url, "https://evil.test/malware.exe");
        assert_eq!(spec.filename, "demo.bin");
        assert!(spec.mirrors.is_empty());
    }

    #[test]
    fn multiline_metalink_paste_still_expands() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "legal_terms_version",
                serde_json::json!(crate::LEGAL_TERMS_VERSION),
            )
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "<metalink>\n<file name=\"demo.bin\">\n<url>https://cdn.example.test/demo.bin</url>\n</file>\n</metalink>".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let snapshot = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                _ => None,
            })
            .unwrap();
        assert_eq!(snapshot.filename, "demo.bin");
    }

    #[test]
    fn site_rules_override_task_speed_and_proxy() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting(
                "site_rules",
                serde_json::json!("example.test=speed:64,conn:2,proxy:http://127.0.0.1:9"),
            )
            .unwrap();
        let spec = apply_site_rules_to_spec(
            &coordinator.core(),
            TaskSpec {
                url: "https://cdn.example.test/a.bin".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(spec.speed_limit_kib, 64);
        assert_eq!(spec.concurrency, 2);
        assert_eq!(spec.proxy, "http://127.0.0.1:9");
    }

    #[test]
    fn site_rule_credentials_are_vaulted_applied_and_deleted() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting(
                "site_rules",
                serde_json::json!(r#"[{"host":"cdn.test","enabled":true}]"#),
            )
            .unwrap();
        let headers = BTreeMap::from([
            ("Authorization".into(), "Bearer private-token".into()),
            ("X-Site-Key".into(), "private-key".into()),
        ]);

        coordinator
            .set_site_rule_credential("cdn.test", "sid=private", &headers, false)
            .unwrap();
        let raw = coordinator
            .lock()
            .unwrap()
            .store()
            .setting_string("site_rules", "")
            .unwrap();
        assert!(!raw.contains("sid=private"));
        assert!(!raw.contains("private-token"));
        assert!(!raw.contains("private-key"));
        let rule = crate::parse_site_rules(&raw).remove(0);
        assert!(!rule.credential_ref.is_empty());
        let protected = coordinator
            .load_credential(&rule.credential_ref)
            .unwrap()
            .unwrap();
        assert!(!protected.contains("sid=private"));

        let ruled = apply_site_rules_to_spec(
            &coordinator.core(),
            TaskSpec {
                url: "https://cdn.test/video.m3u8".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            ruled.credential_ref.as_deref(),
            Some(rule.credential_ref.as_str())
        );
        let (hydrated, _) = hydrate_replay_headers(&coordinator.core(), ruled).unwrap();
        assert_eq!(
            hydrated.headers.get("Cookie").map(String::as_str),
            Some("sid=private")
        );
        assert_eq!(
            hydrated.headers.get("Authorization").map(String::as_str),
            Some("Bearer private-token")
        );

        coordinator
            .set_site_rule_credential("cdn.test", "", &BTreeMap::new(), true)
            .unwrap();
        assert!(coordinator
            .load_credential(&rule.credential_ref)
            .unwrap()
            .is_none());
        let cleared = apply_site_rules_to_spec(
            &coordinator.core(),
            TaskSpec {
                url: "https://cdn.test/video.m3u8".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(cleared.credential_ref.is_none());
    }

    #[test]
    fn hydrate_replay_headers_applies_cookie() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .store_credential("cred-1", r#"{"cookie":"a=b"}"#, "browser_replay")
            .unwrap();
        let (spec, json) = hydrate_replay_headers(
            &coordinator.core(),
            TaskSpec {
                url: "https://example.test/a.bin".into(),
                credential_ref: Some("cred-1".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(spec.headers.get("Cookie").unwrap(), "a=b");
        assert!(json.contains("a=b"));
    }

    #[test]
    fn create_task_moves_cookie_header_into_credential_ref() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("Cookie".into(), "sid=9".into());
        headers.insert("Referer".into(), "https://cdn.test/page".into());
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/a.bin".into(),
                    headers,
                    ..Default::default()
                },
            })
            .unwrap();
        let snapshot = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                _ => None,
            })
            .unwrap();
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec(&snapshot.task_id)
            .cloned()
            .unwrap();
        assert!(spec.headers.get("Cookie").is_none());
        assert_eq!(
            spec.headers.get("Referer").unwrap(),
            "https://cdn.test/page"
        );
        let blob = coordinator
            .load_credential(spec.credential_ref.as_ref().unwrap())
            .unwrap()
            .unwrap();
        let plain = crate::CredentialVault.unprotect(&blob).unwrap_or(blob);
        assert!(plain.contains("sid=9"));
    }

    #[test]
    fn discover_cast_with_null_timeout_emits_devices_event() {
        std::env::set_var("HLS_V7_CAST_NULL", "1");
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let events = coordinator
            .dispatch(CoreCommand::DiscoverCastDevices {
                mode: "cast".into(),
            })
            .unwrap();
        assert!(events.iter().any(|envelope| matches!(
            envelope.event,
            crate::CoreEvent::CastDevices { .. } | crate::CoreEvent::Error { .. }
        )));
    }

    #[test]
    fn browser_media_push_is_persisted_and_resolved_through_core() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let request = MediaPushRequest {
            id: "media-push-test".into(),
            push_kind: "tvbox".into(),
            url: "https://cdn.test/video.mp4".into(),
            title: "测试视频".into(),
            status: "pending".into(),
            message: "等待选择设备".into(),
            location: String::new(),
            created_at_ms: 42,
        };
        let events = coordinator
            .dispatch(CoreCommand::RequestMediaPush {
                request: request.clone(),
            })
            .unwrap();
        assert!(events.iter().any(|envelope| matches!(
            &envelope.event,
            CoreEvent::MediaPushRequested { request } if request.id == "media-push-test"
        )));

        let resolved = coordinator
            .dispatch(CoreCommand::ResolveMediaPush {
                request_id: request.id.clone(),
                status: "done".into(),
                message: "已推送到客厅电视".into(),
                location: "http://192.168.1.8/media/video".into(),
            })
            .unwrap();
        assert!(resolved.iter().any(|envelope| matches!(
            &envelope.event,
            CoreEvent::MediaPushResolved { request }
                if request.status == "done" && request.message.contains("客厅电视")
        )));
        let record = coordinator
            .load_handoffs()
            .unwrap()
            .into_iter()
            .find_map(|encoded| serde_json::from_str::<MediaPushRequest>(&encoded).ok())
            .unwrap();
        assert_eq!(record.status, "done");
        assert!(record.location.starts_with("http://192.168.1.8/"));
        assert!(coordinator
            .dispatch(CoreCommand::ResolveMediaPush {
                request_id: request.id,
                status: "pending".into(),
                message: String::new(),
                location: String::new(),
            })
            .is_err());
    }

    #[test]
    fn local_url_shortcut_expands_to_http_task() {
        let dir = std::env::temp_dir().join(format!("v6-url-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("clip.url");
        fs::write(&path, "[InternetShortcut]\nURL=https://cdn.test/clip.mp4\n").unwrap();
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: path.to_string_lossy().into_owned(),
                    filename: "from-shortcut".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let snapshot = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                _ => None,
            })
            .unwrap();
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec(&snapshot.task_id)
            .cloned()
            .unwrap();
        assert_eq!(spec.url, "https://cdn.test/clip.mp4");
        assert_eq!(spec.resource_kind, ResourceKind::File);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn untrusted_download_dir_must_stay_under_configured_root() {
        let root = std::env::temp_dir().join("hls-v6-dl-root");
        let root = root.to_string_lossy().into_owned();
        assert!(
            constrain_untrusted_download_dir("", &root)
                .unwrap()
                .replace('\\', "/")
                .ends_with("hls-v6-dl-root")
                || constrain_untrusted_download_dir("", &root).unwrap() == root
        );
        assert!(constrain_untrusted_download_dir("nested", &root)
            .unwrap()
            .contains("nested"));
        assert!(constrain_untrusted_download_dir("../escape", &root).is_err());
        #[cfg(windows)]
        assert!(constrain_untrusted_download_dir(r"C:\Windows", &root).is_err());
        #[cfg(not(windows))]
        assert!(constrain_untrusted_download_dir("/etc", &root).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn untrusted_download_dir_rejects_junction_escape() {
        use std::process::Command;
        let stamp = std::process::id();
        let root = std::env::temp_dir().join(format!("hls-v6-junc-root-{stamp}"));
        let outside = std::env::temp_dir().join(format!("hls-v6-junc-out-{stamp}"));
        let junction = root.join("escape");
        let _ = fs::create_dir_all(&root);
        let _ = fs::create_dir_all(&outside);
        let _ = fs::remove_dir(&junction);
        let ok = Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &junction.to_string_lossy(),
                &outside.to_string_lossy(),
            ])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !ok {
            let _ = fs::remove_dir_all(&root);
            let _ = fs::remove_dir_all(&outside);
            return;
        }
        let requested = junction.to_string_lossy().into_owned();
        let configured = root.to_string_lossy().into_owned();
        assert!(constrain_untrusted_download_dir(&requested, &configured).is_err());
        let _ = fs::remove_dir(&junction);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn coordinator_accept_handoff_creates_task_and_rejects_escaped_dir() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .dispatch(CoreCommand::OfferResource {
                offer: crate::ResourceOffer {
                    url: "https://cdn.test/a.bin".into(),
                    handoff_id: "handoff-coord".into(),
                    filename: "a.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        assert!(coordinator
            .dispatch(CoreCommand::AcceptHandoff {
                handoff_id: "handoff-coord".into(),
                filename: "a.bin".into(),
                download_dir: "../escape".into(),
                trusted_ui: false,
            })
            .is_err());
        let events = coordinator
            .dispatch(CoreCommand::AcceptHandoff {
                handoff_id: "handoff-coord".into(),
                filename: "a.bin".into(),
                download_dir: String::new(),
                trusted_ui: false,
            })
            .unwrap();
        assert!(events
            .iter()
            .any(|envelope| matches!(envelope.event, crate::CoreEvent::HandoffResolved { .. })));
        assert_eq!(coordinator.tasks().unwrap().len(), 1);
    }

    #[test]
    fn expired_presenter_lease_transfers_pending_row_to_fallback() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .lock()
            .unwrap()
            .store_mut()
            .save_handoff(
                "handoff-ui",
                &serde_json::json!({
                    "id": "handoff-ui",
                    "status": "pending",
                    "presentation": "presented",
                    "presentation_owner": "crashed-presenter",
                    "presentation_lease_until_ms": 1,
                    "created_at_ms": 1
                })
                .to_string(),
                "pending",
                None,
                1,
            )
            .unwrap();
        coordinator
            .dispatch(CoreCommand::OfferResource {
                offer: crate::ResourceOffer {
                    url: "https://cdn.test/a.bin".into(),
                    handoff_id: "handoff-ui".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        assert!(coordinator
            .lock()
            .unwrap()
            .pending_handoff("handoff-ui")
            .is_some());
        let events = coordinator
            .dispatch(CoreCommand::PresentHandoff {
                handoff_id: "handoff-ui".into(),
                ok: false,
                presenter_id: String::new(),
            })
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event.event,
            crate::CoreEvent::UiShow { ref surface } if surface == "main"
        )));
        let json = coordinator
            .lock()
            .unwrap()
            .store()
            .load_handoffs()
            .unwrap()
            .join("\n");
        assert!(json.contains("\"presentation\":\"fallback\""));
        assert!(json.contains("\"status\":\"pending\""));
        assert!(coordinator
            .lock()
            .unwrap()
            .pending_handoff("handoff-ui")
            .is_some());
    }

    #[test]
    fn presenter_lease_blocks_compose_and_owner_can_release() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .lock()
            .unwrap()
            .store_mut()
            .save_handoff(
                "handoff-shown",
                &serde_json::json!({
                    "id": "handoff-shown",
                    "status": "pending",
                    "presentation": "queued",
                    "created_at_ms": 1
                })
                .to_string(),
                "pending",
                None,
                1,
            )
            .unwrap();
        coordinator
            .dispatch(CoreCommand::OfferResource {
                offer: crate::ResourceOffer {
                    url: "https://cdn.test/b.bin".into(),
                    handoff_id: "handoff-shown".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        coordinator
            .dispatch(CoreCommand::PresentHandoff {
                handoff_id: "handoff-shown".into(),
                ok: true,
                presenter_id: "presenter-test".into(),
            })
            .unwrap();
        let claimed = coordinator
            .lock()
            .unwrap()
            .store()
            .load_handoffs()
            .unwrap()
            .join("\n");
        assert!(claimed.contains("\"presentation\":\"presenting\""));
        assert!(claimed.contains("\"presentation_owner\":\"presenter-test\""));
        assert!(coordinator
            .dispatch(CoreCommand::PresentHandoff {
                handoff_id: "handoff-shown".into(),
                ok: true,
                presenter_id: "presenter-other".into(),
            })
            .is_err());
        assert!(coordinator
            .dispatch(CoreCommand::PresentHandoff {
                handoff_id: "handoff-shown".into(),
                ok: false,
                presenter_id: String::new(),
            })
            .is_err());
        coordinator
            .dispatch(CoreCommand::PresentHandoff {
                handoff_id: "handoff-shown".into(),
                ok: true,
                presenter_id: "presenter-test".into(),
            })
            .unwrap();
        let json = coordinator
            .lock()
            .unwrap()
            .store()
            .load_handoffs()
            .unwrap()
            .join("\n");
        assert!(json.contains("\"presentation\":\"presented\"") || json.contains("presented"));
        assert!(json.contains("pending"));
        assert!(!json.contains("\"status\":\"failed\""));
        assert!(coordinator
            .lock()
            .unwrap()
            .pending_handoff("handoff-shown")
            .is_some());
        coordinator
            .dispatch(CoreCommand::PresentHandoff {
                handoff_id: "handoff-shown".into(),
                ok: false,
                presenter_id: "presenter-test".into(),
            })
            .unwrap();
        let released = coordinator
            .lock()
            .unwrap()
            .store()
            .load_handoffs()
            .unwrap()
            .join("\n");
        assert!(released.contains("\"presentation\":\"fallback\""));
    }

    #[test]
    fn rejected_handoff_can_persist_source_site_kind_suppression() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let offer = crate::ResourceOffer {
            url: "https://cdn.test/movie.m3u8".into(),
            resource_kind: ResourceKind::Hls,
            source_page_url: "https://Video.Example.Test/watch/42".into(),
            handoff_id: "handoff-suppress".into(),
            filename: "movie.m3u8".into(),
            ..Default::default()
        };
        let encoded = serde_json::json!({
            "id": "handoff-suppress",
            "offer": offer,
            "filename": "movie.m3u8",
            "title": "Movie",
            "mime_type": "application/vnd.apple.mpegurl",
            "size": 0,
            "status": "pending",
            "presentation": "presented",
            "task_id": null,
            "created_at_ms": 1,
            "request_id": ""
        })
        .to_string();
        coordinator
            .save_handoff("handoff-suppress", &encoded, "pending", None, 1)
            .unwrap();
        coordinator
            .dispatch(CoreCommand::OfferResource { offer })
            .unwrap();
        coordinator
            .dispatch(CoreCommand::RejectHandoff {
                handoff_id: "handoff-suppress".into(),
                suppress_site_kind: true,
            })
            .unwrap();
        let rows = coordinator.load_handoffs().unwrap();
        assert!(rows.iter().any(|row| {
            row.contains("\"host\":\"video.example.test\"") && row.contains("\"kind\":\"hls\"")
        }));
    }

    #[test]
    fn published_path_outside_download_root_is_ignored() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|item| item.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("hls-pub-root-{}-{stamp}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let spec = TaskSpec {
            filename: "clip.bin".into(),
            download_dir: dir.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let paths = TaskPaths::for_task("task-pub", &spec).unwrap();
        paths.prepare().unwrap();
        let outside =
            std::env::temp_dir().join(format!("hls-pub-escape-{}-{stamp}", std::process::id()));
        fs::write(&outside, b"secret").unwrap();
        fs::write(
            paths.task_dir().join("published.path"),
            outside.to_string_lossy().as_bytes(),
        )
        .unwrap();
        let resolved = resolve_published(&paths);
        assert_ne!(resolved, outside);
        let outside_canon = logical_canonical(&outside);
        assert_ne!(logical_canonical(&resolved), outside_canon);
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legal_gate_blocks_create_and_start_until_accepted() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let create = coordinator.dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/gated.bin".into(),
                filename: "gated.bin".into(),
                ..Default::default()
            },
        });
        assert!(
            create.unwrap_err().contains("legal"),
            "CreateTask must not run before the legal gate"
        );
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/gated.bin".into(),
                    filename: "gated.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(false))
            .unwrap();
        let start = coordinator.dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        });
        assert!(
            start.unwrap_err().contains("legal"),
            "start must not run after the legal flag is cleared"
        );
        let resume = coordinator.dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "resume".into(),
        });
        assert!(resume.unwrap_err().contains("legal"));
        let retry = coordinator.dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "retry".into(),
        });
        assert!(retry.unwrap_err().contains("legal"));
    }

    #[test]
    fn create_task_rejects_javascript_url() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        let error = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "javascript:alert(1)".into(),
                    filename: "x.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(error.contains("协议") || error.contains("换行") || error.contains("不受支持"));
        let file_url = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "file:///C:/Windows/win.ini".into(),
                    filename: "x.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(file_url.contains("协议") || file_url.contains("不受支持"));
        assert!(coordinator
            .set_setting("proxy_url", serde_json::json!("http://127.0.0.1\r\nX: 1"))
            .is_err());
        assert!(coordinator
            .set_setting("download_dir", serde_json::json!("../escape"))
            .is_err());
    }

    #[test]
    fn create_task_coerces_unsafe_method_and_etag() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/a.bin".into(),
                    filename: "a.bin".into(),
                    request_method: "CONNECT\r\nHost: evil".into(),
                    etag: "\"ok\"\r\nX: 1".into(),
                    last_modified: "Wed, 01 Jan 2020 00:00:00 GMT\nInjected".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let snapshot = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                _ => None,
            })
            .unwrap();
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec(&snapshot.task_id)
            .cloned()
            .unwrap();
        assert_eq!(spec.request_method, "GET");
        assert!(spec.etag.is_empty());
        assert!(spec.last_modified.is_empty());
        let post = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/b.bin".into(),
                    filename: "b.bin".into(),
                    request_method: "POST".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let post_id = post
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            })
            .unwrap();
        let post_spec = coordinator
            .lock()
            .unwrap()
            .task_spec(&post_id)
            .cloned()
            .unwrap();
        assert_eq!(post_spec.request_method, "POST");
    }

    #[test]
    fn adversarial_rejects_script_data_blob_and_refresh() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "legal_terms_version",
                serde_json::json!(crate::LEGAL_TERMS_VERSION),
            )
            .unwrap();
        for url in [
            "  DATA:text/plain,hi",
            "blob:https://cdn.test/1",
            "vbscript:msgbox(1)",
            "http://cdn.test/a.bin\r\nX: 1",
        ] {
            let error = coordinator
                .dispatch(CoreCommand::CreateTask {
                    spec: TaskSpec {
                        url: url.into(),
                        filename: "x.bin".into(),
                        ..Default::default()
                    },
                })
                .unwrap_err();
            assert!(
                error.contains("协议")
                    || error.contains("不受支持")
                    || error.contains("换行")
                    || error.contains("控制"),
                "url {url:?} leaked through: {error}"
            );
        }
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/ok.bin".into(),
                    filename: "ok.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let refresh = coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "refresh:javascript:alert(1)".into(),
            })
            .unwrap_err();
        assert!(
            refresh.contains("不受支持") || refresh.contains("协议"),
            "refresh leaked: {refresh}"
        );
    }

    #[test]
    fn adversarial_rejects_helper_and_scan_interpreters() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        assert!(coordinator
            .set_setting(
                "av_scan_command",
                serde_json::json!("powershell -Command Start-Process calc")
            )
            .is_err());
        assert!(coordinator
            .set_setting("av_scan_command", serde_json::json!("cmd /c calc {file}"))
            .is_err());
        assert!(coordinator
            .set_setting(
                "av_scan_command",
                serde_json::json!(r"C:\Windows\System32\mshta.exe {file}")
            )
            .is_err());
        assert!(coordinator
            .set_setting(
                "av_scan_command",
                serde_json::json!(r"C:\Windows\explorer.exe {file}")
            )
            .is_err());
        assert!(coordinator
            .set_setting(
                "av_scan_command",
                serde_json::json!("MpCmdRun.exe -Scan -File {file}")
            )
            .is_ok());
        assert!(coordinator
            .set_setting(
                "ffmpeg_path",
                serde_json::json!(r"C:\Windows\System32\cmd.exe")
            )
            .is_err());
        assert!(coordinator
            .set_setting("ffmpeg_path", serde_json::json!(r"..\ffmpeg.exe"))
            .is_err());
        assert!(coordinator
            .set_setting(
                "ffmpeg_path",
                serde_json::json!(r"C:\tools\ffmpeg.exe & calc.exe")
            )
            .is_err());
        assert!(coordinator
            .set_setting("default_user_agent", serde_json::json!("UA\r\nCookie: x"))
            .is_err());
        assert!(coordinator
            .set_setting("torrent_watch_dir", serde_json::json!(r"..\Windows"))
            .is_err());
        assert!(coordinator
            .set_setting("proxy_url", serde_json::json!("javascript:alert(1)"))
            .is_err());
        assert!(coordinator
            .set_setting("tvbox_endpoint", serde_json::json!("javascript:alert(1)"))
            .is_err());
        assert!(coordinator
            .set_setting(
                "tvbox_endpoint",
                serde_json::json!("https://8.8.8.8/action")
            )
            .is_err());
    }

    #[test]
    fn adversarial_rejects_bom_null_ms_schemes_and_reserved_names() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "legal_terms_version",
                serde_json::json!(crate::LEGAL_TERMS_VERSION),
            )
            .unwrap();
        for url in [
            "\u{feff}javascript:alert(1)",
            "https://cdn.test/a.bin\0.gif",
            "ms-msdt:foo",
            "shell:AppsFolder",
            "search-ms:query=x",
            "about:blank",
            "view-source:https://cdn.test/a.bin",
            "file:C:/Windows/win.ini",
            "\\\\.\\pipe\\HLSDownloader.v7",
        ] {
            let error = coordinator
                .dispatch(CoreCommand::CreateTask {
                    spec: TaskSpec {
                        url: url.into(),
                        filename: "x.bin".into(),
                        ..Default::default()
                    },
                })
                .unwrap_err();
            assert!(
                error.contains("协议")
                    || error.contains("不受支持")
                    || error.contains("控制")
                    || error.contains("链接"),
                "url {url:?} leaked through: {error}"
            );
        }
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/con.bin".into(),
                    filename: "CON".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let id = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            })
            .unwrap();
        let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
        let paths = TaskPaths::for_task(&id, &spec).unwrap();
        assert_eq!(
            paths
                .final_output
                .file_name()
                .and_then(|name| name.to_str()),
            Some("_CON")
        );
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/conin.bin".into(),
                    filename: "CONIN$.txt".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let id = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            })
            .unwrap();
        let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
        let paths = TaskPaths::for_task(&id, &spec).unwrap();
        assert_eq!(
            paths
                .final_output
                .file_name()
                .and_then(|name| name.to_str()),
            Some("_CONIN$.txt")
        );
    }

    #[test]
    fn adversarial_drops_injected_headers_and_non_http_mirrors() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "legal_terms_version",
                serde_json::json!(crate::LEGAL_TERMS_VERSION),
            )
            .unwrap();
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("X-Injected\r\nX".into(), "1".into());
        headers.insert("X-Ok".into(), "safe".into());
        headers.insert("X-Bad".into(), "a\nb".into());
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/a.bin".into(),
                    filename: "a.bin".into(),
                    headers,
                    mirrors: vec![
                        "javascript:alert(1)".into(),
                        "file:///C:/Windows/win.ini".into(),
                        "https://mirror.test/a.bin".into(),
                    ],
                    ..Default::default()
                },
            })
            .unwrap();
        let id = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            })
            .unwrap();
        let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
        assert!(!spec
            .headers
            .keys()
            .any(|key| key.contains('\r') || key.contains('\n')));
        assert_eq!(spec.headers.get("X-Ok").map(String::as_str), Some("safe"));
        assert!(!spec.headers.contains_key("X-Bad"));
        let normalized = crate::mirrors::normalize_mirror_urls(&spec.url, &spec.mirrors);
        assert_eq!(normalized, vec!["https://mirror.test/a.bin".to_string()]);
        assert_eq!(spec.mirrors, vec!["https://mirror.test/a.bin".to_string()]);
    }

    #[test]
    fn create_task_rejects_client_dir_outside_download_root() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "legal_terms_version",
                serde_json::json!(crate::LEGAL_TERMS_VERSION),
            )
            .unwrap();
        coordinator
            .set_setting("download_dir", serde_json::json!("downloads"))
            .unwrap();
        let escaped = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/a.bin".into(),
                    filename: "a.bin".into(),
                    download_dir: r"C:\Windows".into(),
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(
            escaped.contains("下载目录") || escaped.contains("根目录"),
            "absolute escape leaked: {escaped}"
        );
        coordinator
            .set_setting(
                "site_rules",
                serde_json::json!(r#"[{"host":"cdn.test","download_dir":"site-cache"}]"#),
            )
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/b.bin".into(),
                    filename: "b.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let id = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            })
            .unwrap();
        let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
        assert_eq!(spec.download_dir, "site-cache");
    }

    #[test]
    fn coordinator_exports_normalized_tasks_through_the_core_event_contract() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .dispatch_created(TaskSpec {
                url: "https://cdn.test/archive.zip".into(),
                resource_kind: ResourceKind::File,
                title: "Archive".into(),
                filename: "archive.zip".into(),
                ..Default::default()
            })
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::ExportTasks {
                task_ids: vec!["task-1".into()],
                format: "json".into(),
            })
            .unwrap();
        let (data, count) = events
            .iter()
            .find_map(|envelope| match &envelope.event {
                CoreEvent::TaskExport {
                    format,
                    data,
                    task_count,
                } if format == "json" => Some((data, *task_count)),
                _ => None,
            })
            .expect("task export event");
        assert_eq!(count, 1);
        let document: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(document["schema"], "hls-downloader.tasks.v1");
        assert_eq!(document["tasks"][0]["id"], "task-1");
        assert_eq!(document["tasks"][0]["filename"], "archive.zip");
    }

    #[test]
    fn coordinator_reimports_its_exported_json_as_new_tasks() {
        let source = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        source
            .dispatch_created(TaskSpec {
                url: "https://cdn.test/archive.zip".into(),
                resource_kind: ResourceKind::File,
                title: "Archive".into(),
                filename: "archive.zip".into(),
                speed_limit_kib: 256,
                scheduled_start_at: "2999-01-01T00:00:00Z".into(),
                ..Default::default()
            })
            .unwrap();
        let events = source
            .dispatch(CoreCommand::ExportTasks {
                task_ids: Vec::new(),
                format: "json".into(),
            })
            .unwrap();
        let data = events
            .iter()
            .find_map(|event| match &event.event {
                CoreEvent::TaskExport { data, .. } => Some(data),
                _ => None,
            })
            .unwrap();
        let path =
            std::env::temp_dir().join(format!("hls-v7-task-import-{}.json", std::process::id()));
        fs::write(&path, data.as_bytes()).unwrap();

        let target = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        target
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        let imported = target
            .dispatch(CoreCommand::ImportPaths {
                paths: vec![path.to_string_lossy().into_owned()],
            })
            .unwrap();
        let snapshot = imported
            .iter()
            .find_map(|event| match &event.event {
                CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                _ => None,
            })
            .unwrap();
        assert_eq!(snapshot.filename, "archive.zip");
        assert_eq!(snapshot.speed_limit_kib, 256);
        assert_eq!(snapshot.scheduled_start_at, "2999-01-01T00:00:00Z");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn queue_profile_contract_validates_schedule_and_identity() {
        let valid = serde_json::json!([
            {
                "id": "default",
                "name": "默认队列",
                "enabled": true,
                "priority": 0,
                "max_active": 3,
                "speed_limit_kib": 0,
                "schedule_enabled": false,
                "start_time": "00:00",
                "stop_time": "23:59",
                "active_days": "1,2,3,4,5,6,7",
                "completion_action": "none"
            },
            {
                "id": "night-media",
                "name": "夜间媒体",
                "enabled": true,
                "priority": 20,
                "max_active": 2,
                "speed_limit_kib": 4096,
                "schedule_enabled": true,
                "start_time": "23:00",
                "stop_time": "07:00",
                "active_days": "1,2,3,4,5",
                "completion_action": "sleep"
            }
        ]);
        assert!(validate_queue_profiles(&valid).is_ok());
        assert!(validate_queue_profiles(&serde_json::json!([])).is_err());
        assert!(validate_queue_profiles(&serde_json::json!([{
            "id": "default", "name": "默认队列", "max_active": 0
        }]))
        .is_err());
        assert!(validate_queue_profiles(&serde_json::json!([{
            "id": "other", "name": "其他队列"
        }]))
        .is_err());
        assert!(validate_queue_profiles(&serde_json::json!([
            { "id": "default", "name": "重复" },
            { "id": "other", "name": "重复" }
        ]))
        .is_err());
        assert!(validate_queue_profiles(&serde_json::json!([{
            "id": "default", "name": "默认队列", "active_days": "1,1"
        }]))
        .is_err());
        assert!(validate_queue_profiles(&serde_json::json!([{
            "id": "default", "name": "默认队列", "priority": 101
        }]))
        .is_err());
    }

    #[test]
    fn task_schedule_and_queue_membership_fail_closed_before_worker_spawn() {
        let future = crate::TaskSnapshot {
            scheduled_start_at: "2999-01-01T00:00:00Z".into(),
            ..Default::default()
        };
        assert!(!task_schedule_allowed(&future));

        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let error = coordinator
            .apply_defaults_to_spec(TaskSpec {
                queue_id: "missing-queue".into(),
                ..spec()
            })
            .unwrap_err();
        assert!(error.contains("任务所属队列不存在"));
    }

    #[test]
    fn queue_completion_action_requires_every_task_to_succeed() {
        let profile = QueueProfile {
            id: "night-media".into(),
            name: "夜间媒体".into(),
            completion_action: "sleep".into(),
            ..QueueProfile::default()
        };
        let spec = TaskSpec {
            queue_id: profile.id.clone(),
            ..spec()
        };
        let completed = |id: &str| crate::TaskSnapshot {
            task_id: id.into(),
            queue_id: profile.id.clone(),
            status: "completed".into(),
            ..Default::default()
        };
        let unrelated = crate::TaskSnapshot {
            task_id: "other".into(),
            queue_id: "default".into(),
            status: "queued".into(),
            ..Default::default()
        };
        let tasks = vec![completed("one"), completed("two"), unrelated];
        assert_eq!(
            queue_completion_decision(&tasks, std::slice::from_ref(&profile), &spec),
            Some(("sleep".into(), "队列：夜间媒体".into()))
        );

        let mut failed = tasks;
        failed[1].status = "failed".into();
        assert!(queue_completion_decision(&failed, &[profile], &spec).is_none());
    }
}

#[test]
fn replay_request_body_is_bounded_materialized_and_removed() {
    assert_eq!(
        decode_base64_bounded("cG9zdC1ib2R5", 128).unwrap(),
        b"post-body"
    );
    assert!(decode_base64_bounded("%%%", 128).is_err());
    assert!(decode_base64_bounded("QUJDRA==", 3).is_err());

    let dir = std::env::temp_dir().join(format!("hls-replay-body-{}", std::process::id()));
    let spec = TaskSpec {
        url: "https://cdn.test/post".into(),
        filename: "post.bin".into(),
        download_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let paths = TaskPaths::for_task("replay-body", &spec).unwrap();
    paths.prepare().unwrap();
    let guard =
        materialize_replay_request_body(r#"{"request_body":"cG9zdC1ib2R5"}"#, &paths).unwrap();
    let path = guard.0.as_ref().unwrap().clone();
    assert_eq!(std::fs::read(&path).unwrap(), b"post-body");
    drop(guard);
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn curl_import_creates_a_task_with_encrypted_replay_context() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )
        .unwrap();
    let mut option_headers = BTreeMap::new();
    option_headers.insert("Origin".into(), "https://override.test".into());
    let events = coordinator
        .dispatch(CoreCommand::ImportCurl {
            command: r#"curl -X POST -H "Authorization: Bearer abc" -H "Origin: https://site.test" -b "sid=secret" --data-raw "id=42" https://cdn.test/file.bin"#.into(),
            options: TaskSpec {
                filename: "saved.bin".into(),
                headers: option_headers,
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&task_id)
        .cloned()
        .unwrap();
    assert_eq!(spec.url, "https://cdn.test/file.bin");
    assert_eq!(spec.request_method, "POST");
    assert_eq!(spec.filename, "saved.bin");
    assert!(spec.headers.is_empty());
    assert!(spec.credential_ref.is_some());
    let (hydrated, replay) = hydrate_replay_headers(&coordinator.core(), spec).unwrap();
    assert_eq!(
        hydrated.headers.get("Cookie").map(String::as_str),
        Some("sid=secret")
    );
    assert_eq!(
        hydrated.headers.get("Origin").map(String::as_str),
        Some("https://override.test")
    );
    assert_eq!(
        hydrated.headers.get("Authorization").map(String::as_str),
        Some("Bearer abc")
    );
    assert_eq!(
        decode_base64_bounded(
            serde_json::from_str::<Value>(&replay).unwrap()["request_body"]
                .as_str()
                .unwrap(),
            MAX_REPLAY_BODY_BYTES,
        )
        .unwrap(),
        b"id=42",
    );
}
fn validate_ui_layout(
    value: &Value,
    allowed: &[&str],
    widths: bool,
    required_visible: &str,
) -> Result<(), String> {
    let raw = value
        .as_str()
        .ok_or_else(|| "界面布局设置必须是文本".to_string())?;
    if raw.is_empty() {
        return Ok(());
    }
    if raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err("界面布局设置过长或包含控制字符".into());
    }
    let mut seen = HashSet::new();
    let mut required_enabled = false;
    for entry in raw.split(',') {
        let parts: Vec<_> = entry.split(':').collect();
        if parts.len() != if widths { 3 } else { 2 } {
            return Err("界面布局设置格式无效".into());
        }
        let id = parts[0];
        if !allowed.contains(&id) || !seen.insert(id) {
            return Err("界面布局包含未知或重复项目".into());
        }
        let enabled = parts.last().is_some_and(|flag| matches!(*flag, "0" | "1"));
        if !enabled {
            return Err("界面布局启用状态无效".into());
        }
        if widths {
            let width = parts[1]
                .parse::<u32>()
                .map_err(|_| "任务列宽度必须是整数".to_string())?;
            if !(48..=800).contains(&width) {
                return Err("任务列宽度必须在 48 到 800 之间".into());
            }
        }
        if id == required_visible && parts.last() == Some(&"1") {
            required_enabled = true;
        }
    }
    if !required_enabled {
        return Err(format!("界面布局必须保留 {required_visible}"));
    }
    Ok(())
}

#[test]
fn ui_layout_settings_validate_required_entries_widths_and_persistence() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let columns = "name:320:1,progress:180:1,status:120:1,speed:120:0,size:120:1,actions:96:0";
    coordinator
        .set_setting("task_column_layout", serde_json::json!(columns))
        .unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .store()
            .setting_string("task_column_layout", "")
            .unwrap(),
        columns
    );

    assert!(coordinator
        .set_setting("task_column_layout", serde_json::json!("name:47:1"))
        .is_err());
    assert!(coordinator
        .set_setting(
            "task_column_layout",
            serde_json::json!("name:120:0,progress:120:1")
        )
        .is_err());
    assert!(coordinator
        .set_setting("toolbar_actions", serde_json::json!("new:0,paste:1"))
        .is_err());
}

pub const PUBLIC_SETTING_KEYS: &[&str] = &[
    "browser_takeover_enabled",
    "browser_takeover_minimum_bytes",
    "legal_terms_accepted",
    "download_speed_limit_kib",
    "download_hourly_quota_mib",
    "download_speed_schedule_enabled",
    "download_speed_schedule_start",
    "download_speed_schedule_end",
    "download_speed_schedule_kib",
    "auto_category_dirs",
    "browser_category_dirs",
    "queue_max_active",
    "queue_profiles",
    "site_rules",
    "av_scan_enabled",
    "av_scan_command",
    "torrent_watch_dir",
    "watch_torrents",
    "download_dir",
    "temp_dir",
    "default_concurrency",
    "proxy_url",
    "ffmpeg_path",
    "clipboard_watch",
    "completion_sound_enabled",
    "download_progress_window_enabled",
    "download_complete_popup_enabled",
    "resume_interrupted_on_startup",
    "auto_retry_failed_max",
    "existing_file_policy",
    "live_record_max_minutes",
    "download_subtitles",
    "skip_ad_segments",
    "keep_temp_files",
    "default_user_agent",
    "tvbox_endpoint",
    "dark_mode",
    "allow_duplicate",
    "queue_auto_start_enabled",
    "queue_auto_start_time",
    "queue_auto_stop_enabled",
    "queue_auto_stop_time",
    "default_referer",
    "default_origin",
    "allowed_hosts",
    "http_chunk_size_mb",
    "completion_power_action",
    "start_on_login",
    "queue_active_days",
    "proxy_mode",
    "proxy_bypass",
    "reduce_motion",
    "harvest_minimum_bytes",
    "av_scan_fail_on_threat",
    "bt_upload_limit_kib",
    "bt_max_connections",
    "bt_enable_dht",
    "preferred_cast_device_id",
    "task_column_layout",
    "toolbar_actions",
    "task_sort",
];
#[test]
fn failure_diagnostics_preserve_http_status_stage_and_actionable_hint() {
    let failure = task_failure_from_error(
        "HTTP 404",
        "transfer",
        "https://cdn.test/missing.mp4?token=secret",
        3,
        false,
    );
    assert_eq!(failure.code, "HTTP_404");
    assert_eq!(failure.http_status, Some(404));
    assert_eq!(failure.stage, "transfer");
    assert_eq!(failure.attempt, 3);
    assert!(failure.hint.contains("重新识别"));
    assert_eq!(
        extract_http_status("request failed with status=503"),
        Some(503)
    );
    assert_eq!(failure_stage("checking", "checksum mismatch"), "checksum");
}

#[test]
fn failure_diagnostics_classify_local_output_failures_without_false_http_codes() {
    let failure = task_failure_from_error(
        "write failed: no space left on device",
        "transfer",
        r"C:\downloads\movie.mp4",
        1,
        false,
    );
    assert_eq!(failure.code, "DISK_FULL");
    assert_eq!(failure.http_status, None);
    assert!(failure.hint.contains("空间"));
}

#[test]
fn share_media_rejects_ambiguous_and_untrusted_sources() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    assert!(share_media(&coordinator, "", "", "", "").is_err());
    assert!(share_media(&coordinator, "relative-video.mp4", "", "video", "").is_err());
    assert!(share_media(&coordinator, "", "javascript:alert(1)", "video", "").is_err());
    assert!(share_media(
        &coordinator,
        r"C:\missing.mp4",
        "https://cdn.test/video.mp4",
        "video",
        ""
    )
    .is_err());
}

#[test]
fn stopping_cast_revokes_the_active_media_mount() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let server = shared_media().unwrap();
    let token = crate::playback::random_mount_token();
    let dir = std::env::temp_dir().join(format!("hls-cast-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("video.bin");
    std::fs::write(&file, b"cast-lifecycle").unwrap();
    server.mount(&token, file);
    remember_cast_mount(&token);

    let request = |token: &str| {
        let mut stream = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        stream
            .write_all(
                format!("GET /media/{token} HTTP/1.1\r\nRange: bytes=0-3\r\n\r\n").as_bytes(),
            )
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8_lossy(&response).into_owned()
    };
    assert!(request(&token).starts_with("HTTP/1.1 206"));
    clear_cast_mount();
    assert!(request(&token).starts_with("HTTP/1.1 404"));
    let _ = std::fs::remove_dir_all(dir);
}
