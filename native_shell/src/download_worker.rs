//! v6 task execution adapter for the existing Rust HTTP Range engine.

use crate::{
    apply_replay_json_for, run_job, with_replay_json, CoreCommand, CoreEvent, CredentialVault,
    EventEnvelope, PersistentCore, TaskSpec, TorrentSession,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct TaskPaths {
    pub output: PathBuf,
    pub final_output: PathBuf,
    pub control: PathBuf,
    pub progress: PathBuf,
}

impl TaskPaths {
    pub fn for_task(task_id: &str, spec: &TaskSpec) -> Result<Self, String> {
        let root = if !spec.download_dir.trim().is_empty() {
            PathBuf::from(&spec.download_dir)
        } else if let Some(root) = std::env::var_os("HLS_V6_DOWNLOAD_DIR") {
            PathBuf::from(root)
        } else {
            PathBuf::from("downloads")
        };
        let filename = safe_filename(&spec.filename, &spec.url);
        let task_dir = root.join(".v6-tasks").join(task_id);
        Ok(Self {
            output: task_dir.join("payload.downloading"),
            final_output: root.join(filename),
            control: task_dir.join("control"),
            progress: task_dir.join("progress.json"),
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
        fs::write(&self.control, "run").map_err(|error| format!("write task control: {error}"))?;
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

pub fn constrain_untrusted_download_dir(requested: &str, configured: &str) -> Result<String, String> {
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
        && !key.contains(['\r', '\n', ':'])
        && !value.contains('\r')
        && !value.contains('\n')
}

fn reject_task_url(url: &str) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("javascript:")
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("file:")
    {
        return Err("链接协议不受支持".into());
    }
    if url.contains('\r') || url.contains('\n') {
        return Err("链接不能包含换行".into());
    }
    Ok(())
}

fn validate_helper_executable(path: &str, names: &[&str]) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(());
    }
    reject_path_escape(path)?;
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
    let command = command.trim();
    if command.is_empty() {
        return Ok(());
    }
    let lower = command.to_ascii_lowercase();
    if lower.contains("cmd.exe")
        || lower.contains("powershell")
        || lower.contains("pwsh")
        || lower.contains("wscript")
        || lower.contains("cscript")
    {
        return Err("扫描命令不能调用系统脚本解释器".into());
    }
    Ok(())
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
    crate::net_policy::configure_limit_kib(u64::from(spec.speed_limit_kib));
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
            .map_err(|_| "v6 Core mutex poisoned".to_string())?;
        locked.store().load_credential(&credential_ref)?
    };
    let Some(blob) = blob else {
        return Ok((spec, String::new()));
    };
    let plain = CredentialVault.unprotect(&blob).unwrap_or(blob);
    apply_replay_json_for(&mut spec.headers, &plain, &spec.url);
    Ok((spec, plain))
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
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned
    }
}

#[derive(Clone)]
pub struct CoreCoordinator {
    core: Arc<Mutex<PersistentCore>>,
    active: Arc<Mutex<HashSet<String>>>,
    retries: Arc<Mutex<HashMap<String, u32>>>,
}

#[derive(Debug, Clone)]
pub struct CoreSettings {
    pub takeover_enabled: bool,
    pub takeover_minimum_bytes: u64,
    pub legal_accepted: bool,
    pub speed_limit_kib: u64,
    pub schedule_enabled: bool,
    pub schedule_start: String,
    pub schedule_end: String,
    pub schedule_kib: u64,
    pub auto_category: bool,
    pub category_dirs: crate::category::CategoryDirs,
    pub queue_max: u64,
    pub site_rules: String,
    pub av_scan_enabled: bool,
    pub av_scan_command: String,
    pub torrent_watch: String,
    pub download_dir: String,
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
    pub http_chunk_size_mb: u64,
    pub completion_power_action: String,
    pub start_on_login: bool,
    pub queue_active_days: String,
    pub proxy_mode: String,
    pub proxy_bypass: String,
    pub legal_terms_version: String,
    pub reduce_motion: bool,
    pub harvest_minimum_bytes: u64,
}

impl CoreCoordinator {
    pub fn new(core: PersistentCore) -> Self {
        Self {
            core: Arc::new(Mutex::new(core)),
            active: Arc::new(Mutex::new(HashSet::new())),
            retries: Arc::new(Mutex::new(HashMap::new())),
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
        Ok(CoreSettings {
            takeover_enabled: core.store().setting_bool("browser_takeover_enabled", true)?,
            takeover_minimum_bytes: core
                .store()
                .setting_u64("browser_takeover_minimum_bytes", 0)?,
            legal_accepted: core.store().setting_bool("legal_terms_accepted", false)?,
            speed_limit_kib: core.store().setting_u64("download_speed_limit_kib", 0)?,
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
            site_rules: core.store().setting_string("site_rules", "")?,
            av_scan_enabled: core.store().setting_bool("av_scan_enabled", false)?,
            av_scan_command: core.store().setting_string("av_scan_command", "")?,
            torrent_watch: core.store().setting_string("torrent_watch_dir", "")?,
            download_dir: core.store().setting_string("download_dir", "downloads")?,
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
            live_record_max_minutes: core
                .store()
                .setting_u64("live_record_max_minutes", 0)?,
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
            http_chunk_size_mb: core.store().setting_u64("http_chunk_size_mb", 8)?.clamp(1, 64),
            completion_power_action: core
                .store()
                .setting_string("completion_power_action", "none")?,
            start_on_login: core.store().setting_bool("start_on_login", false)?,
            queue_active_days: core
                .store()
                .setting_string("queue_active_days", "1,2,3,4,5,6,7")?,
            proxy_mode: core.store().setting_string("proxy_mode", "manual")?,
            proxy_bypass: core.store().setting_string("proxy_bypass", "")?,
            legal_terms_version: core.store().setting_string("legal_terms_version", "")?,
            reduce_motion: core.store().setting_bool("reduce_motion", false)?,
            harvest_minimum_bytes: core
                .store()
                .setting_u64("harvest_minimum_bytes", 0)?,
        })
    }

    pub fn set_setting(&self, key: &str, value: Value) -> Result<(), String> {
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
        if key == "download_dir" {
            if let Some(path) = value.as_str() {
                reject_path_escape(path)?;
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
        if matches!(
            key,
            "proxy_url"
                | "default_referer"
                | "default_user_agent"
                | "tvbox_endpoint"
                | "proxy_mode"
                | "proxy_bypass"
                | "queue_active_days"
                | "legal_terms_version"
        ) {
            if let Some(text) = value.as_str() {
                if text.contains('\r') || text.contains('\n') {
                    return Err("设置值不能包含换行".into());
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
        let start_login = if key == "start_on_login" {
            value.as_bool()
        } else {
            None
        };
        let stamp_legal = key == "legal_terms_accepted" && value.as_bool() == Some(true);
        {
            let mut core = self.lock()?;
            match value {
                Value::Bool(flag) => core.store_mut().set_setting(key, flag),
                Value::Number(number) => {
                    if let Some(int) = number.as_u64() {
                        core.store_mut().set_setting(key, int)
                    } else {
                        core.store_mut().set_setting(key, Value::Number(number))
                    }
                }
                other => core.store_mut().set_setting(key, other),
            }?;
        }
        if let Some(flag) = start_login {
            let _ = crate::startup::apply(flag);
        }
        if stamp_legal {
            let _ = self.set_setting(
                "legal_terms_version",
                Value::String(crate::LEGAL_TERMS_VERSION.into()),
            );
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

    pub fn save_handoff(
        &self,
        handoff_id: &str,
        handoff_json: &str,
        status: &str,
        task_id: Option<&str>,
        created_at_ms: u64,
    ) -> Result<(), String> {
        self.lock()?
            .store_mut()
            .save_handoff(handoff_id, handoff_json, status, task_id, created_at_ms)
    }

    pub fn load_handoffs(&self) -> Result<Vec<String>, String> {
        self.lock()?.store().load_handoffs()
    }

    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, PersistentCore>, String> {
        self.core
            .lock()
            .map_err(|_| "v6 Core mutex poisoned".to_string())
    }

    pub fn tasks(&self) -> Result<Vec<crate::TaskSnapshot>, String> {
        self.refresh_output_flags()?;
        self.core
            .lock()
            .map_err(|_| "v6 Core mutex poisoned".to_string())
            .map(|core| core.tasks())
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
                .map(|paths| !paths.final_output.exists())
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
                ..Default::default()
            },
        );
        let encoded = crate::format_site_rules(&rules);
        self.set_setting("site_rules", serde_json::json!(encoded))?;
        self.lock()?.emit(CoreEvent::Toast {
            level: "site_profile".into(),
            message: format!("已保存 {} 的站点规则", crate::site_rules::host_of(&spec.url)),
        })
    }

    pub fn dispatch(&self, command: CoreCommand) -> Result<Vec<EventEnvelope>, String> {
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
        if let CoreCommand::HarvestPage { url } = command {
            return harvest_page(self, &url);
        }
        if let CoreCommand::AcceptHandoff {
            handoff_id,
            filename,
            download_dir,
        } = command
        {
            return self.accept_handoff_command(handoff_id, filename, download_dir);
        }
        if let CoreCommand::RejectHandoff { handoff_id } = command {
            return self.dispatch_inner(CoreCommand::RejectHandoff { handoff_id });
        }
        if let CoreCommand::PresentHandoff { handoff_id, ok } = command {
            return self.present_handoff_command(handoff_id, ok);
        }
        if let CoreCommand::ControlCast { action } = command {
            crate::cast::control_session(&action)?;
            return self.lock()?.emit(CoreEvent::CastSession {
                active: action != "stop",
                title: String::new(),
                device: crate::cast::last_device_label(),
                status: match action.as_str() {
                    "pause" => "已暂停投屏".into(),
                    "play" => "继续投屏".into(),
                    "stop" => "已停止投屏".into(),
                    _ => action,
                },
            });
        }
        if let CoreCommand::CreateTask { spec } = command {
            self.require_legal()?;
            let spec = self.apply_defaults_to_spec(spec)?;
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
        if let Some(urls) = crate::link_file::expand_source(&spec.url)? {
            let mut specs = Vec::new();
            for url in urls {
                if reject_task_url(&url).is_err() {
                    continue;
                }
                if crate::looks_like_metalink(&url) || url.contains("<metalink") {
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
            let (_, body) = crate::fetch_bytes(
                &spec.url,
                &std::collections::HashMap::new(),
                &spec.proxy,
            )
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
            let (_, body) = crate::fetch_bytes(
                &spec.url,
                &std::collections::HashMap::new(),
                &spec.proxy,
            )
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

    fn dispatch_inner(&self, command: CoreCommand) -> Result<Vec<EventEnvelope>, String> {
        if let CoreCommand::TaskAction { task_id, action } = &command {
            if matches!(action.as_str(), "start" | "resume" | "retry") {
                self.require_legal()?;
            }
            if action == "open" {
                return open_completed(self, task_id, false).map(|_| Vec::new());
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
                return refresh_task_url(self, task_id, url);
            }
            if action == "push_tvbox" {
                return push_task_tvbox(self, task_id).map(|_| Vec::new());
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
                self.delete_task_files(task_id)?;
                return self.dispatch_inner(CoreCommand::TaskAction {
                    task_id: task_id.clone(),
                    action: "delete".into(),
                });
            }
        }
        if let CoreCommand::SetSetting { key, value } = command {
            self.set_setting(&key, value)?;
            return Ok(Vec::new());
        }
        if let CoreCommand::PlayTask { task_id } = &command {
            return play_task(self, task_id).map(|_| Vec::new());
        }
        if let CoreCommand::CastTask { task_id } = &command {
            return cast_task(self, task_id);
        }
        if let CoreCommand::CastToDevice { task_id, device_id } = &command {
            return cast_to_device(self, task_id, device_id);
        }
        if let CoreCommand::PlayerControl { action } = &command {
            return player_control(action).map(|_| Vec::new());
        }
        if let CoreCommand::ProbeUrl { url } = &command {
            return probe_command(self, url);
        }
        if let CoreCommand::DiscoverCastDevices = command {
            return discover_cast(self);
        }
        if let CoreCommand::DownloadUpdate = command {
            return download_update(self);
        }
        if let CoreCommand::OpenCompleted { task_id, folder } = &command {
            return open_completed(self, task_id, *folder).map(|_| Vec::new());
        }
        if matches!(command, CoreCommand::CancelPowerAction) {
            let canceled = crate::power_action::cancel();
            return self.lock()?.emit(CoreEvent::Error {
                code: if canceled {
                    "power_canceled".into()
                } else {
                    "power_idle".into()
                },
                message: if canceled {
                    "已取消完成后电源动作".into()
                } else {
                    "没有待执行的电源动作".into()
                },
            });
        }
        let (task_id, should_start) = match &command {
            CoreCommand::TaskAction { task_id, action } => (
                Some(task_id.clone()),
                matches!(action.as_str(), "start" | "resume" | "retry"),
            ),
            _ => (None, false),
        };
        let events = {
            let mut core = self
                .core
                .lock()
                .map_err(|_| "v6 Core mutex poisoned".to_string())?;
            if let Some(task_id) = task_id.as_deref() {
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
                }
            }
            core.handle(command)?
        };
        if should_start {
            if let Some(task_id) = task_id {
                self.spawn(task_id)?;
            }
        }
        Ok(events)
    }

    pub(crate) fn start_next_queued(&self) -> Result<(), String> {
        if self.require_legal().is_err() {
            return Ok(());
        }
        loop {
            let max = self
                .lock()?
                .store()
                .setting_u64("queue_max_active", 3)?
                .max(1) as usize;
            let active_ids = self
                .active
                .lock()
                .map_err(|_| "v6 worker registry poisoned".to_string())?
                .clone();
            if active_ids.len() >= max {
                return Ok(());
            }
            if !self.queue_allowed()? {
                return Ok(());
            }
            let mut queued: Vec<_> = self
                .tasks()?
                .into_iter()
                .filter(|task| task.status == "queued" && !active_ids.contains(&task.task_id))
                .collect();
            queued.sort_by_key(|task| (task.queue_index, task.task_id.clone()));
            let Some(next) = queued.into_iter().next() else {
                return Ok(());
            };
            self.spawn(next.task_id)?;
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
                let _ = mark_progress(
                    &self.core,
                    &task.task_id,
                    task.downloaded_bytes,
                    task.total_bytes,
                    "waiting",
                    status,
                );
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
                let _ = self.dispatch_inner(CoreCommand::TaskAction {
                    task_id: task.task_id,
                    action: "pause".into(),
                });
            }
        }
        Ok(())
    }

    fn apply_defaults_to_spec(&self, mut spec: TaskSpec) -> Result<TaskSpec, String> {
        let settings = self.settings()?;
        if spec.download_dir.trim().is_empty() {
            spec.download_dir = settings.download_dir.clone();
        }
        reject_path_escape(&spec.download_dir)?;
        if !spec.body_path.trim().is_empty() {
            reject_path_escape(&spec.body_path)?;
            if !Path::new(&spec.body_path).is_file() {
                return Err("请求体文件不存在".into());
            }
        }
        if spec.proxy.trim().is_empty() {
            spec.proxy = settings.proxy_url.clone();
        }
        spec.headers.retain(|key, value| header_value_allowed(key, value));
        spec.request_method = crate::http_engine::sanitize_http_method(&spec.request_method);
        if !header_value_allowed("ETag", &spec.etag) {
            spec.etag.clear();
        }
        if !header_value_allowed("Last-Modified", &spec.last_modified) {
            spec.last_modified.clear();
        }
        spec.mirrors.retain(|url| {
            !url.contains('\r') && !url.contains('\n') && !url.contains('\0')
        });
        if spec.proxy.contains('\r') || spec.proxy.contains('\n') {
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
    ) -> Result<Vec<EventEnvelope>, String> {
        self.require_legal()?;
        let settings = self.settings()?;
        let download_dir =
            constrain_untrusted_download_dir(&download_dir, &settings.download_dir)?;
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
        let _ = self.lock()?.take_pending_handoff(&handoff_id);
        let task_id = events.iter().find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } | CoreEvent::TaskUpdated { snapshot } => {
                Some(snapshot.task_id.clone())
            }
            CoreEvent::DuplicateOffered { task_id, .. } => Some(task_id.clone()),
            _ => None,
        });
        events.extend(self.lock()?.emit(CoreEvent::HandoffResolved {
            handoff_id,
            task_id,
        })?);
        Ok(events)
    }

    fn present_handoff_command(
        &self,
        handoff_id: String,
        ok: bool,
    ) -> Result<Vec<EventEnvelope>, String> {
        if handoff_id.trim().is_empty() {
            return Err("接管请求缺少编号".into());
        }
        let mut core = self.lock()?;
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
            if matches!(current, "accepted" | "rejected" | "failed") {
                return Ok(Vec::new());
            }
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "presentation".into(),
                    Value::String(if ok {
                        "presented".into()
                    } else {
                        "failed".into()
                    }),
                );
                if !ok {
                    object.insert("status".into(), Value::String("failed".into()));
                }
            }
            let json = serde_json::to_string(&value).map_err(|error| {
                format!("encode handoff presentation {handoff_id}: {error}")
            })?;
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
            break;
        }
        if !ok {
            let _ = core.take_pending_handoff(&handoff_id);
        }
        Ok(Vec::new())
    }

    fn queue_allowed(&self) -> Result<bool, String> {
        let settings = self.settings()?;
        if !crate::net_policy::weekday_allowed(&settings.queue_active_days) {
            return Ok(false);
        }
        if settings.queue_auto_start_enabled && settings.queue_auto_stop_enabled {
            Ok(crate::net_policy::schedule_window_active(
                &settings.queue_auto_start_time,
                &settings.queue_auto_stop_time,
            ))
        } else {
            Ok(true)
        }
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
        if let Some(spec) = self.lock()?.task_spec(&task_id).cloned() {
            if !crate::net_policy::scheduled_start_reached(&spec.scheduled_start_at)
                || crate::net_policy::scheduled_stop_hit(&spec.scheduled_stop_at)
            {
                return Ok(());
            }
        }
        let max = self
            .lock()?
            .store()
            .setting_u64("queue_max_active", 3)?
            .max(1) as usize;
        {
            let mut active = self
                .active
                .lock()
                .map_err(|_| "v6 worker registry poisoned".to_string())?;
            if active.len() >= max && !active.contains(&task_id) {
                drop(active);
                if let Ok(mut core) = self.lock() {
                    let (downloaded, total) = core
                        .tasks()
                        .iter()
                        .find(|task| task.task_id == task_id)
                        .map(|task| (task.downloaded_bytes, task.total_bytes))
                        .unwrap_or((0, None));
                    let _ = core.handle(CoreCommand::UpdateProgress {
                        task_id: task_id.clone(),
                        downloaded_bytes: downloaded,
                        total_bytes: total,
                        speed_bytes_per_sec: 0,
                        stage: "waiting".into(),
                        status: "queued".into(),
                    });
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
                let _ = core.lock().map(|mut core| {
                    let (downloaded, total) = core
                        .tasks()
                        .iter()
                        .find(|task| task.task_id == task_id)
                        .map(|task| (task.downloaded_bytes, task.total_bytes))
                        .unwrap_or((0, None));
                    let stage = if status == "paused" {
                        "waiting"
                    } else {
                        "finished"
                    };
                    let _ = core.handle(CoreCommand::UpdateProgress {
                        task_id: task_id.clone(),
                        downloaded_bytes: downloaded,
                        total_bytes: total,
                        speed_bytes_per_sec: 0,
                        stage: stage.into(),
                        status: status.into(),
                    });
                    eprintln!("v6 task {task_id} failed: {error}");
                });
                if status == "failed" {
                    let max = coordinator
                        .lock()
                        .ok()
                        .and_then(|guard| {
                            guard.store().setting_u64("auto_retry_failed_max", 0).ok()
                        })
                        .unwrap_or(0);
                    let attempt = {
                        let mut map = retries.lock().unwrap_or_else(|error| error.into_inner());
                        let slot = map.entry(task_id.clone()).or_insert(0);
                        *slot += 1;
                        *slot
                    };
                    if u64::from(attempt) <= max && max > 0 {
                        let _ = mark_progress(
                            &core,
                            &task_id,
                            0,
                            None,
                            "waiting",
                            "queued",
                        );
                    }
                } else if status != "paused" {
                    if let Ok(mut map) = retries.lock() {
                        map.remove(&task_id);
                    }
                }
            } else if let Ok(mut map) = retries.lock() {
                map.remove(&task_id);
            }
            if let Ok(mut active) = active.lock() {
                active.remove(&task_id);
                crate::sleep_inhibit::set_active(!active.is_empty());
            }
            let _ = coordinator.start_next_queued();
        });
        Ok(())
    }
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
        .map_err(|_| "v6 Core mutex poisoned".to_string())?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let (spec, replay_json) = hydrate_replay_headers(&core, spec)?;
    let spec = apply_site_rules_to_spec(&core, spec)?;
    apply_speed_policy(&core, spec.speed_limit_kib)?;
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
                let guard = core.lock().map_err(|_| "v6 Core mutex poisoned".to_string())?;
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
                .map_err(|_| "v6 Core mutex poisoned".to_string())?
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
            crate::ftp_engine::download_ftp(&spec.url, &paths.output, &paths.control, true)?;
            complete_payload(&core, task_id, &paths, &paths.output, &spec)
        }
        crate::ResourceKind::Sftp => {
            let paths = TaskPaths::for_task(task_id, &spec)?;
            paths.prepare()?;
            crate::sftp_engine::download_sftp(&spec.url, &paths.output, &paths.control)?;
            complete_payload(&core, task_id, &paths, &paths.output, &spec)
        }
        crate::ResourceKind::Torrent => {
            let paths = TaskPaths::for_task(task_id, &spec)?;
            paths.prepare()?;
            crate::torrent_engine::torrent_session().download(
                &spec.url,
                &paths.output,
                &paths.control,
                &headers,
                &spec.proxy,
            )?;
            complete_payload(&core, task_id, &paths, &paths.output, &spec)
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
    if let Ok(mb) = core
        .lock()
        .map_err(|_| "v6 Core mutex poisoned".to_string())?
        .store()
        .setting_u64("http_chunk_size_mb", 8)
    {
        job.chunk_bytes = mb.clamp(1, 64) * 1024 * 1024;
    }
    let (sender, receiver) = mpsc::channel();
    let worker_job = job.clone();
    thread::spawn(move || {
        let result = run_job(&worker_job);
        let _ = sender.send(result);
    });
    loop {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(result) => {
                result.map_err(|error| error.to_string())?;
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
                return Err("v6 HTTP worker disconnected".into())
            }
        }
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
        .map_err(|_| "v6 Core mutex poisoned".to_string())?
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
    if let Some(checksum) = spec.checksum.as_deref().filter(|value| !value.trim().is_empty()) {
        crate::checksum::verify_file(payload, checksum).map_err(|error| {
            let _ = mark_progress(core, task_id, 0, None, "checksum", "failed");
            error
        })?;
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
        if result.state == "threat" {
            mark_progress(core, task_id, 0, None, "av_scan", "failed")?;
            return Err(format!("av_threat: {}", result.detail));
        }
    }
    let (policy, keep_temp) = output_policy(core);
    let published =
        crate::output_path::publish_file(payload, &paths.final_output, &policy, keep_temp)?;
    remember_published(paths, &published);
    crate::motw::mark_downloaded_file(&published, &spec.url);
    let download_subtitles = core
        .lock()
        .ok()
        .and_then(|guard| guard.store().setting_bool("download_subtitles", true).ok())
        .unwrap_or(true);
    if download_subtitles {
        copy_subtitle_sidecars(&paths.task_dir(), &published);
    }
    let total = fs::metadata(&published).ok().map(|meta| meta.len());
    mark_progress(
        core,
        task_id,
        total.unwrap_or(0),
        total,
        "finished",
        "completed",
    )?;
    maybe_schedule_power(core, spec)
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
    roots.iter().any(|root| canon == *root || canon.starts_with(root))
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

fn maybe_schedule_power(
    core: &Arc<Mutex<PersistentCore>>,
    spec: &TaskSpec,
) -> Result<(), String> {
    let action = if !spec.completion_action.trim().is_empty() {
        spec.completion_action.clone()
    } else {
        core.lock()
            .ok()
            .and_then(|guard| {
                guard
                    .store()
                    .setting_string("completion_power_action", "none")
                    .ok()
            })
            .unwrap_or_else(|| "none".into())
    };
    if !crate::power_action::is_armed(&action) {
        return Ok(());
    }
    crate::power_action::schedule(&action, 30)?;
    let title = if spec.filename.trim().is_empty() {
        spec.url.clone()
    } else {
        spec.filename.clone()
    };
    let _ = core.lock().map_err(|_| "v6 Core mutex poisoned".to_string())?.emit(CoreEvent::Error {
        code: "power_pending".into(),
        message: format!(
            "30 秒后将{}（{}），可在完成窗口取消",
            crate::power_action::label(&action),
            title
        ),
    });
    Ok(())
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
    thread::spawn(move || {
        let _ = sender.send(work());
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
                return Err("v6 media worker disconnected".into());
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
        .map_err(|_| "v6 Core mutex poisoned".to_string())?
        .store()
        .setting_string("site_rules", "")?;
    if let Some(rule) = crate::site_rules::matching_rule(&crate::parse_site_rules(&raw), &spec.url) {
        if rule.speed_limit_kib > 0 {
            spec.speed_limit_kib = rule.speed_limit_kib;
        }
        if rule.concurrency > 0 {
            spec.concurrency = rule.concurrency;
        }
        if !rule.proxy.trim().is_empty() && spec.proxy.trim().is_empty() {
            spec.proxy = rule.proxy.clone();
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
    }
    Ok(spec)
}

fn apply_speed_policy(core: &Arc<Mutex<PersistentCore>>, task_limit_kib: u32) -> Result<(), String> {
    let core = core
        .lock()
        .map_err(|_| "v6 Core mutex poisoned".to_string())?;
    let global = core.store().setting_u64("download_speed_limit_kib", 0)?;
    let scheduled = crate::net_policy::effective_limit_kib(
        global,
        core.store()
            .setting_bool("download_speed_schedule_enabled", false)?,
        &core
            .store()
            .setting_string("download_speed_schedule_start", "22:00")?,
        &core
            .store()
            .setting_string("download_speed_schedule_end", "08:00")?,
        core.store().setting_u64("download_speed_schedule_kib", 0)?,
    );
    let effective = if task_limit_kib > 0 {
        let task = u64::from(task_limit_kib);
        if scheduled == 0 {
            task
        } else {
            scheduled.min(task)
        }
    } else {
        scheduled
    };
    crate::net_policy::configure_limit_kib(effective);
    Ok(())
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
    let cookie = spec.headers.remove("Cookie").unwrap_or_default();
    let authorization = spec.headers.remove("Authorization").unwrap_or_default();
    if cookie.is_empty() && authorization.is_empty() {
        return Ok(spec);
    }
    let json = serde_json::json!({
        "cookie": cookie,
        "authorization": authorization,
    })
    .to_string();
    let blob = if cfg!(windows) {
        CredentialVault.protect(&json)?
    } else {
        json
    };
    let credential_ref = spec
        .credential_ref
        .clone()
        .unwrap_or_else(|| format!("ui-{:x}", simple_hash(&spec.url)));
    coordinator.store_credential(&credential_ref, &blob, "browser_replay")?;
    spec.credential_ref = Some(credential_ref);
    Ok(spec)
}

fn simple_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn play_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<(), String> {
    let url = mount_task_url(coordinator, task_id)?;
    shared_player()?.play(&url)
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

fn cast_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<Vec<EventEnvelope>, String> {
    let loopback = mount_task_url(coordinator, task_id)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let server = shared_media()?;
    server.enable_lan();
    let host = crate::cast::primary_lan_ipv4()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".into());
    let location = crate::cast::lan_media_url(server, &token, &host)?;
    let _ = crate::cast::ssdp_notify(&location);
    coordinator.lock()?.emit(CoreEvent::CastSession {
        active: true,
        title: location,
        device: "局域网".into(),
        status: "已在局域网发布播放地址".into(),
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
    if device_id.trim().is_empty() {
        let _ = crate::cast::ssdp_notify(&location);
        return coordinator.lock()?.emit(CoreEvent::CastSession {
            active: true,
            title: location,
            device: "局域网".into(),
            status: "已发出局域网投屏通知".into(),
        });
    }
    let title = if spec.title.is_empty() {
        spec.filename.clone()
    } else {
        spec.title
    };
    let device = crate::cast::play_on_device(device_id, &location, &title)?;
    coordinator.lock()?.emit(CoreEvent::CastSession {
        active: true,
        title,
        device,
        status: "正在投屏".into(),
    })
}

fn probe_command(coordinator: &CoreCoordinator, url: &str) -> Result<Vec<EventEnvelope>, String> {
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
                events.extend(coordinator.lock()?.emit(CoreEvent::HarvestResult {
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
                })?);
            }
            Ok(events)
        }
        Err(error) => coordinator.lock()?.emit(CoreEvent::Error {
            code: "probe_failed".into(),
            message: error,
        }),
    }
}

fn harvest_page(coordinator: &CoreCoordinator, url: &str) -> Result<Vec<EventEnvelope>, String> {
    probe_command(coordinator, url)
}

fn push_task_tvbox(coordinator: &CoreCoordinator, task_id: &str) -> Result<(), String> {
    let url = mount_task_url(coordinator, task_id)?;
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
    crate::cast::push_tvbox(&endpoint, &url, &title)
}

fn discover_cast(coordinator: &CoreCoordinator) -> Result<Vec<EventEnvelope>, String> {
    let timeout = if std::env::var_os("HLS_V6_CAST_NULL").is_some() {
        Duration::from_millis(1)
    } else {
        Duration::from_millis(2500)
    };
    let mut devices = crate::cast::discover_devices(timeout)?;
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
    let message = if devices.is_empty() {
        "没有发现投屏设备，仍可发局域网通知".to_string()
    } else {
        format!("发现 {} 台投屏设备", devices.len())
    };
    crate::cast::remember_devices(devices.clone());
    let mut core = coordinator.lock()?;
    let mut events = core.emit(CoreEvent::CastDevices { devices })?;
    events.extend(core.emit(CoreEvent::Error {
        code: "cast_scan".into(),
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
        return coordinator.lock()?.emit(CoreEvent::Error {
            code: "update_current".into(),
            message: format!("已是最新版本 {}", info.current),
        });
    }
    let path = crate::updater::download_installer(&info)?;
    open_path(&path)?;
    coordinator.lock()?.emit(CoreEvent::Error {
        code: "update_downloaded".into(),
        message: format!("已下载安装包 {}", path.display()),
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
        published
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(published)
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
    apply_speed_policy(&coordinator.core(), kib)?;
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

fn open_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("打开目标为空".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let file: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
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
            let speed = other.trim_start_matches("speed:").parse::<f64>().unwrap_or(1.0);
            player.set_speed(speed)
        }
        other if other.starts_with("preview:") => {
            let percent = other
                .trim_start_matches("preview:")
                .parse::<f64>()
                .unwrap_or(0.0);
            player.preview_percent(percent)
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

    #[test]
    fn task_paths_keep_payload_and_final_output_separate() {
        let paths = TaskPaths::for_task("task-1", &spec()).unwrap();
        assert!(paths.output.ends_with("payload.downloading"));
        assert!(paths.final_output.ends_with("_bad_name_.bin"));
        assert_ne!(paths.output, paths.final_output);
        assert_eq!(safe_filename("report&calc.exe", "https://cdn.test/a.bin"), "report_calc.exe");
        assert_eq!(safe_filename("a%PATH%.txt", "https://cdn.test/a.bin"), "a_PATH_.txt");
        assert!(!safe_filename("evil\nnotepad.exe", "https://cdn.test/a.bin").contains('\n'));
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
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
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
                "task did not complete: {task:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(fs::read(download_dir.join("fixture.bin")).unwrap(), body);
        let _ = fs::remove_dir_all(download_dir);
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
        template
            .headers
            .insert("Cookie".into(), "sid=1".into());
        template
            .headers
            .insert("Authorization".into(), "Bearer x".into());
        template
            .headers
            .insert("Referer".into(), "https://site.test/watch".into());
        template.credential_ref = Some("cred-page".into());
        let dirs = crate::category::CategoryDirs::default();
        let same = spec_from_url(&template, "https://site.test/clip.mp4", "clip.mp4", false, &dirs);
        assert_eq!(same.headers.get("Cookie").unwrap(), "sid=1");
        assert_eq!(same.credential_ref.as_deref(), Some("cred-page"));
        let other = spec_from_url(&template, "https://cdn.test/clip.mp4", "clip.mp4", false, &dirs);
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
        std::env::set_var("HLS_V6_PLAYER_NULL", "1");
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
    fn metalink_body_expands_to_http_task_with_mirrors() {
        std::env::set_var("HLS_V6_SKIP_LEGAL", "1");
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
        let snapshot = events.iter().find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        }).unwrap();
        assert_eq!(snapshot.filename, "demo.bin");
        let spec = coordinator.lock().unwrap().task_spec(&snapshot.task_id).cloned().unwrap();
        assert_eq!(spec.url, "https://cdn.example.test/demo.bin");
        assert_eq!(spec.mirrors, vec!["https://mirror.example.test/demo.bin"]);
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
        let snapshot = events.iter().find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        }).unwrap();
        let spec = coordinator.lock().unwrap().task_spec(&snapshot.task_id).cloned().unwrap();
        assert!(spec.headers.get("Cookie").is_none());
        assert_eq!(spec.headers.get("Referer").unwrap(), "https://cdn.test/page");
        let blob = coordinator
            .load_credential(spec.credential_ref.as_ref().unwrap())
            .unwrap()
            .unwrap();
        let plain = crate::CredentialVault.unprotect(&blob).unwrap_or(blob);
        assert!(plain.contains("sid=9"));
    }

    #[test]
    fn discover_cast_with_null_timeout_emits_devices_event() {
        std::env::set_var("HLS_V6_CAST_NULL", "1");
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let events = coordinator
            .dispatch(CoreCommand::DiscoverCastDevices)
            .unwrap();
        assert!(events.iter().any(|envelope| matches!(
            envelope.event,
            crate::CoreEvent::CastDevices { .. } | crate::CoreEvent::Error { .. }
        )));
    }

    #[test]
    fn local_url_shortcut_expands_to_http_task() {
        std::env::set_var("HLS_V6_SKIP_LEGAL", "1");
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
        assert!(constrain_untrusted_download_dir("", &root)
            .unwrap()
            .replace('\\', "/")
            .ends_with("hls-v6-dl-root")
            || constrain_untrusted_download_dir("", &root).unwrap() == root);
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
        std::env::set_var("HLS_V6_SKIP_LEGAL", "1");
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
            })
            .is_err());
        let events = coordinator
            .dispatch(CoreCommand::AcceptHandoff {
                handoff_id: "handoff-coord".into(),
                filename: "a.bin".into(),
                download_dir: String::new(),
            })
            .unwrap();
        assert!(events.iter().any(|envelope| matches!(
            envelope.event,
            crate::CoreEvent::HandoffResolved { .. }
        )));
        assert_eq!(coordinator.tasks().unwrap().len(), 1);
    }

    #[test]
    fn present_handoff_failure_fails_pending_row() {
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
        coordinator
            .dispatch(CoreCommand::PresentHandoff {
                handoff_id: "handoff-ui".into(),
                ok: false,
            })
            .unwrap();
        let json = coordinator
            .lock()
            .unwrap()
            .store()
            .load_handoffs()
            .unwrap()
            .join("\n");
        assert!(json.contains("failed"));
        assert!(coordinator
            .lock()
            .unwrap()
            .pending_handoff("handoff-ui")
            .is_none());
    }

    #[test]
    fn present_handoff_success_keeps_pending_row() {
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
    }

    #[test]
    fn published_path_outside_download_root_is_ignored() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|item| item.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "hls-pub-root-{}-{stamp}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&dir);
        let spec = TaskSpec {
            filename: "clip.bin".into(),
            download_dir: dir.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let paths = TaskPaths::for_task("task-pub", &spec).unwrap();
        paths.prepare().unwrap();
        let outside = std::env::temp_dir().join(format!(
            "hls-pub-escape-{}-{stamp}",
            std::process::id()
        ));
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
}
