//! Task execution adapter for the resident Rust download engine.

use crate::http_engine::extract_http_status;
use crate::net_policy::queue_profile_allowed;
use crate::output_path::{
    constrain_untrusted_download_dir, logical_canonical, reject_path_escape,
    reserved_dos_device_name,
};
use crate::{
    apply_replay_json_for, run_job_report, with_replay_json, AvScanStatus, CoreCommand, CoreEvent,
    CredentialVault, EventEnvelope, MediaPushRequest, MirrorStatus, PersistentCore, QueueProfile,
    TaskSpec, TorrentSession,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

mod paths;
mod settings;

pub use paths::TaskPaths;
use paths::{initialize_torrent_selection, write_torrent_selection};
use settings::validate_queue_profiles;
pub use settings::{CoreSettings, PUBLIC_SETTING_KEYS};

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

pub(crate) fn validate_local_source_path(
    url: &str,
    source_path: &str,
    expected_size: Option<u64>,
) -> Result<PathBuf, String> {
    let lower_url = url.to_ascii_lowercase();
    if !(lower_url.starts_with("blob:") || lower_url.starts_with("data:"))
        || url.is_empty()
        || url.chars().any(char::is_control)
    {
        return Err("本地源只允许对应 blob: 或 data: 资源".into());
    }
    let path = Path::new(source_path.trim());
    if !path.is_absolute() || source_path.chars().any(char::is_control) {
        return Err("本地源路径必须是有效的绝对路径".into());
    }
    let canonical =
        fs::canonicalize(path).map_err(|error| format!("无法读取本地源文件: {error}"))?;
    let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("本地源必须是常规文件".into());
    }
    let expected = expected_size.ok_or_else(|| "本地源缺少声明的文件大小".to_string())?;
    if metadata.len() != expected {
        return Err(format!(
            "本地源大小不匹配: 声明 {expected} 字节，实际 {} 字节",
            metadata.len()
        ));
    }
    Ok(canonical)
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
    if spec.local_source_path.is_some() {
        return Err("本地文件任务不能构造 HTTP 下载作业".into());
    }
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
        curl_body_limit: None,
        origin_boundary: spec.origin_boundary.clone(),
    };
    Ok((job, paths))
}

pub(crate) fn hydrate_replay_headers(
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

#[derive(Clone)]
pub struct CoreCoordinator {
    core: Arc<Mutex<PersistentCore>>,
    active: Arc<Mutex<HashSet<String>>>,
    retries: Arc<Mutex<HashMap<String, u32>>>,
    update_shutdown: Arc<AtomicBool>,
    #[cfg(test)]
    worker_wait_started: Arc<Mutex<Option<mpsc::Sender<()>>>>,
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

    pub(crate) fn event_notify(&self) -> Result<Arc<(Mutex<u64>, std::sync::Condvar)>, String> {
        self.lock().map(|core| core.event_notify())
    }

    pub fn events_after(&self, sequence: u64, limit: usize) -> Result<Vec<EventEnvelope>, String> {
        self.lock().map(|core| core.events_after(sequence, limit))
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

    pub fn delete_credential(&self, credential_ref: &str) -> Result<(), String> {
        self.lock()?.store_mut().delete_credential(credential_ref)
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
        // The read, the credential mutation and the pointer write now share one
        // lock guard and one transaction.  Doing them separately is what made
        // this destructive: a `clear` that deleted the credential row first and
        // then failed to persist `site_rules` left a rule pointing at a password
        // that no longer existed anywhere -- the plaintext never reaches the
        // database, so the user has to retype it.  Holding the guard also closes
        // the read-modify-write window in which two concurrent callers could
        // overwrite each other's rules.
        let mut core = self.lock()?;
        let raw = core.store().setting_string("site_rules", "")?;
        let mut rules = crate::parse_site_rules(&raw);
        let rule = rules
            .iter_mut()
            .find(|rule| rule.host.trim().eq_ignore_ascii_case(&normalized_host))
            .ok_or_else(|| "保存凭据前请先保存站点规则".to_string())?;
        // Everything the transaction needs to name is owned by this function, so
        // the borrows handed to `apply_credential_with_settings` cannot outlive
        // the values they point at.
        let (stored_ref, new_ref, protected) = if clear {
            // The stored reference, not a derived one: an imported or hand-edited
            // rule may point at a value `credential_ref_for_host` would never
            // rebuild, so deleting by the derived name would orphan it.
            (
                std::mem::take(&mut rule.credential_ref),
                String::new(),
                String::new(),
            )
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
            rule.credential_ref = credential_ref.clone();
            (String::new(), credential_ref, protected)
        };
        let write = if clear {
            (!stored_ref.trim().is_empty()).then_some(crate::store::CredentialWrite::Delete {
                credential_ref: stored_ref.trim(),
            })
        } else {
            Some(crate::store::CredentialWrite::Store {
                credential_ref: &new_ref,
                protected_blob: &protected,
                kind: "site_rule",
            })
        };
        core.store_mut().apply_credential_with_settings(
            write,
            &BTreeMap::from([(
                "site_rules".to_string(),
                serde_json::json!(crate::format_site_rules(&rules)),
            )]),
        )?;
        // `set_setting` used to announce the change; going straight to the store
        // must not silence it, or the UI keeps showing the credential it thought
        // it just saved.
        core.emit(CoreEvent::SettingsChanged {
            keys: vec!["site_rules".to_string()],
        })?;
        Ok(())
    }

    pub(crate) fn admit_handoff(&self, handoff_json: &str) -> Result<String, String> {
        self.lock()?.admit_handoff(handoff_json)
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

    pub(crate) fn player_thumbnail(
        &self,
        task_id: &str,
        seconds: Option<f64>,
    ) -> Result<String, String> {
        {
            let state = player_ui_state().lock().map_err(|_| "player state lock")?;
            if !state.active || state.task_id != task_id {
                return Err("播放会话已改变".into());
            }
        }
        let Some(seconds) = seconds else {
            crate::player_thumbnail::cancel();
            return Ok(String::new());
        };
        let player = shared_player()?;
        let metadata = player.metadata();
        if metadata.session_active != Some(true) || seconds > metadata.duration_seconds {
            return Err("当前位置尚未就绪".into());
        }
        let image = crate::player_thumbnail::decode(&player.source_url()?, seconds)?;
        let state = player_ui_state().lock().map_err(|_| "player state lock")?;
        if !state.active || state.task_id != task_id {
            return Err("播放会话已改变".into());
        }
        Ok(image)
    }

    pub fn load_handoffs(&self) -> Result<Vec<String>, String> {
        self.lock()?.store().load_handoffs()
    }

    pub fn query_handoffs(
        &self,
        handoff_id: Option<&str>,
        client_request_id: Option<&str>,
        pending_only: bool,
    ) -> Result<Vec<String>, String> {
        self.lock()?
            .store()
            .query_handoffs(handoff_id, client_request_id, pending_only)
    }

    pub fn query_handoffs_page(
        &self,
        handoff_id: Option<&str>,
        client_request_id: Option<&str>,
        pending_only: bool,
        after: Option<[i64; 2]>,
    ) -> Result<(Vec<String>, Option<[i64; 2]>), String> {
        self.lock()?
            .store()
            .query_handoffs_page(handoff_id, client_request_id, pending_only, after)
    }

    fn request_media_push(&self, request: MediaPushRequest) -> Result<Vec<EventEnvelope>, String> {
        if request.id.trim().is_empty() || request.id.len() > 160 {
            return Err("媒体推送请求编号无效".into());
        }
        if !matches!(request.push_kind.as_str(), "cast" | "tvbox") {
            return Err("媒体推送类型无效".into());
        }
        if request.status != "pending" {
            return Err("媒体推送请求状态无效".into());
        }
        let lower = request.url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://"))
            || request.url.chars().any(char::is_control)
        {
            return Err("媒体推送地址无效".into());
        }
        self.lock()?
            .handle(CoreCommand::RequestMediaPush { request })
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
        self.lock()?.handle(CoreCommand::ResolveMediaPush {
            request_id: request_id.to_string(),
            status: status.to_string(),
            message: message.trim().chars().take(300).collect(),
            location: location.trim().chars().take(2048).collect(),
        })
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
                    task.output_path = resolved_task_output(&core, &task.task_id, &paths)
                        .to_string_lossy()
                        .into_owned();
                    task.playback_ready = if matches!(task.status.as_str(), "completed" | "done") {
                        Path::new(&task.output_path).is_file()
                    } else {
                        playable_task_playlist(&paths).is_some()
                            || playable_http_total(&paths, spec).is_some()
                    };
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
            let missing = {
                let core = self.lock()?;
                core.task_spec(&task_id)
                    .and_then(|spec| TaskPaths::for_task(&task_id, spec).ok())
                    .map(|paths| !resolved_task_output(&core, &task_id, &paths).exists())
                    .unwrap_or(false)
            };
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
            let ids: Vec<_> = self
                .lock()?
                .tasks()
                .into_iter()
                .filter(|task| matches!(task.status.as_str(), "completed" | "done"))
                .map(|task| task.task_id)
                .collect();
            let mut events = Vec::new();
            for task_id in ids {
                events.extend(self.dispatch_inner(CoreCommand::TaskAction {
                    task_id,
                    action: "delete".into(),
                })?);
            }
            return Ok(events);
        }
        if let CoreCommand::SaveSiteProfile { task_id } = command {
            return self.save_site_profile(&task_id);
        }
        if let CoreCommand::ImportPaths { paths } = command {
            let mut events = Vec::new();
            let mut failures = Vec::new();
            for (index, path) in paths.into_iter().enumerate() {
                match self.dispatch(CoreCommand::CreateTask {
                    spec: TaskSpec {
                        url: path,
                        ..Default::default()
                    },
                }) {
                    Ok(imported) => {
                        for event in imported {
                            if let CoreEvent::Error { message, .. } = &event.event {
                                failures.push(format!("第 {} 项：{message}", index + 1));
                            } else {
                                events.push(event);
                            }
                        }
                    }
                    Err(error) => failures.push(format!("第 {} 项：{error}", index + 1)),
                }
            }
            if !failures.is_empty() {
                let count = events
                    .iter()
                    .filter(|event| matches!(event.event, CoreEvent::TaskCreated { .. }))
                    .count();
                events.extend(self.lock()?.emit(CoreEvent::Error {
                    code: "import_partial_failure".into(),
                    message: format!(
                        "已导入 {count} 项任务，{} 项导入失败：\n{}",
                        failures.len(),
                        failures.join("\n")
                    ),
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
            self.start_next_queued(false)?;
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
                if spec.local_source_path.is_none() && !spec.allow_duplicate {
                    if let Some((task_id, status)) = self.duplicate_of(&spec)? {
                        events.extend(self.reuse_duplicate(task_id, &status)?);
                        continue;
                    }
                }
                events.extend(self.dispatch_created(spec)?);
            }
            self.start_next_queued(false)?;
            return Ok(events);
        }
        self.dispatch_inner(command)
    }

    fn expand_create(&self, spec: TaskSpec) -> Result<Vec<TaskSpec>, String> {
        if spec.local_source_path.is_some() {
            return Ok(vec![spec]);
        }
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
            let settings = self.settings()?;
            let download_dir = settings.download_dir;
            return imported
                .into_iter()
                .map(|mut imported| {
                    reject_path_escape(&imported.download_dir)?;
                    // 导出的绝对目录可能属于另一台电脑；导入不能越过当前下载根目录。
                    if constrain_untrusted_download_dir(&imported.download_dir, &download_dir)
                        .is_err()
                    {
                        imported.download_dir.clear();
                    }
                    if !settings
                        .queue_profiles
                        .iter()
                        .any(|queue| queue.id == imported.queue_id)
                    {
                        imported.queue_id = crate::DEFAULT_QUEUE_ID.into();
                    }
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
            let (_, body) = crate::http_engine::fetch_bytes_limited(
                &spec.url,
                &std::collections::HashMap::new(),
                &spec.proxy,
                1024 * 1024,
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
            let (_, body) = crate::http_engine::fetch_bytes_limited(
                &spec.url,
                &std::collections::HashMap::new(),
                &spec.proxy,
                1024 * 1024,
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
                Value::String(STANDARD.encode(parsed.body.as_bytes())),
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
                crate::net_policy::clear_scoped_limit(&format!("task:{task_id}"));
                if let Ok(mut map) = self.retries.lock() {
                    map.remove(task_id);
                }
            }
            if action == "delete" {
                self.cancel_and_wait(task_id)?;
                self.cleanup_task_cache(task_id)?;
                crate::net_policy::clear_scoped_limit(&format!("task:{task_id}"));
                if let Ok(mut map) = self.retries.lock() {
                    map.remove(task_id);
                }
            }
            if action == "delete_files" {
                return self.lock()?.handle(CoreCommand::TaskAction {
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
        if let CoreCommand::CheckUpdate { silent } = command {
            return check_update(self, silent);
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
                .snapshot(task_id)
                .is_some_and(|task| task.available_actions.iter().any(|item| item == action));
            if action_allowed && matches!(action, "resume" | "retry") {
                self.wait_for_worker_stop(task_id)?;
            }
        }
        if let (Some(task_id), Some(action)) = (task_id.as_deref(), start_action.as_deref()) {
            let mut core = self.lock()?;
            let queue_profiles = load_queue_profiles(core.store())?;
            let deferred = core
                .snapshot(task_id)
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
                    start_accepted = core.snapshot(task_id).is_some_and(|task| {
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
                    if matches!(action.as_str(), "delete" | "delete_file" | "delete_files") {
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

    pub(crate) fn start_next_queued(&self, scheduled_only: bool) -> Result<(), String> {
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
                            && (!scheduled_only || !task.scheduled_start_at.is_empty())
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
            self.start_next_queued(false)?;
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

    pub(crate) fn metadata_spec(&self, url: &str) -> Result<TaskSpec, String> {
        self.apply_defaults_to_spec(TaskSpec {
            url: url.into(),
            ..TaskSpec::default()
        })
    }

    pub(crate) fn read_metadata(
        &self,
        spec: TaskSpec,
        range: Option<&str>,
        limit: u64,
        origin: Option<&str>,
    ) -> Result<crate::http_engine::MetadataResponse, String> {
        let (spec, replay) = hydrate_replay_headers(&self.core(), spec)?;
        with_replay_json(&replay, || {
            crate::http_engine::read_metadata(&spec, range, limit, origin)
        })
    }

    fn apply_defaults_to_spec(&self, mut spec: TaskSpec) -> Result<TaskSpec, String> {
        let client_dir = spec.download_dir.trim().to_string();
        if spec.local_source_path.is_none() {
            spec = apply_site_rules_to_spec(&self.core(), spec)?;
        }
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
        if !crate::net_policy::proxy_route_sentinel(&spec.proxy) && !proxy_url_allowed(&spec.proxy)
        {
            return Err("代理地址无效".into());
        }
        if let Some(source_path) = spec.local_source_path.as_deref() {
            if spec.resource_kind != crate::ResourceKind::File {
                return Err("本地文件源只能用于普通文件任务".into());
            }
            let path = validate_local_source_path(&spec.url, source_path, spec.expected_size)?;
            spec.local_source_path = Some(path.to_string_lossy().into_owned());
            spec.proxy.clear();
        } else {
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

    fn duplicate_request_identity_matches(incoming: &TaskSpec, stored: &TaskSpec) -> bool {
        let method = |spec: &TaskSpec| {
            crate::http_engine::sanitize_http_method(&spec.request_method).to_ascii_uppercase()
        };
        let safe = |spec: &TaskSpec| {
            spec.credential_ref
                .as_deref()
                .is_none_or(|credential| credential.trim().is_empty())
                && spec.body_path.trim().is_empty()
                && method(spec) != "POST"
                && !spec.headers.keys().any(|name| sensitive_header_name(name))
        };
        let normalized_headers = |spec: &TaskSpec| {
            spec.headers
                .iter()
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        safe(incoming)
            && safe(stored)
            && method(incoming) == method(stored)
            && normalized_headers(incoming) == normalized_headers(stored)
            && incoming.preferred_bandwidth == stored.preferred_bandwidth
            && incoming.preferred_height == stored.preferred_height
            && incoming.preferred_audio == stored.preferred_audio
    }

    fn duplicate_of(&self, spec: &TaskSpec) -> Result<Option<(String, String)>, String> {
        let want = crate::duplicate::canonicalize_url(&spec.url);
        if want.is_empty() {
            return Ok(None);
        }
        let core = self.lock()?;
        for task in core.tasks() {
            if let Some(stored) = core.task_spec(&task.task_id) {
                if crate::duplicate::canonicalize_url(&stored.url) == want
                    && Self::duplicate_request_identity_matches(spec, stored)
                {
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
                self.start_next_queued(false)?;
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
            local_source_path: offer.local_source_path.clone(),
            title: if offer.title.trim().is_empty() {
                filename.clone()
            } else {
                offer.title.clone()
            },
            filename,
            download_dir,
            request_method: offer.request_method.clone(),
            mime_type: offer.mime_type.clone(),
            credential_ref: offer.credential_ref.clone(),
            replay_context_ref: offer.replay_context_ref.clone(),
            expected_size: (offer.size > 0).then_some(offer.size),
            preferred_bandwidth: offer.preferred_bandwidth,
            preferred_height: offer.preferred_height,
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
        let presenter_id = presenter_id.trim();
        if ok && presenter_id.is_empty() {
            return Err("下载确认窗口缺少身份标识".into());
        }
        let mut core = self.lock()?;
        let mut updated = false;
        for encoded in core
            .store()
            .query_handoffs(Some(&handoff_id), None, false)?
        {
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
            if ok && presentation == "fallback" {
                return Err("接管请求已由主窗口处理".into());
            }
            if ok && !owner.is_empty() && owner != presenter_id && lease_until > now {
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
                    let next = if owner == presenter_id && presentation == "presenting" {
                        "presented"
                    } else if owner == presenter_id {
                        presentation.as_str()
                    } else {
                        "presenting"
                    };
                    object.insert("presentation".into(), Value::String(next.into()));
                    object.insert(
                        "presentation_owner".into(),
                        Value::String(presenter_id.into()),
                    );
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
        for encoded in core.store().query_handoffs(Some(handoff_id), None, false)? {
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
        let (spec, published) = {
            let core = self.lock()?;
            let spec = core
                .task_spec(task_id)
                .cloned()
                .ok_or_else(|| format!("unknown task {task_id}"))?;
            let completed = core
                .snapshot(task_id)
                .is_some_and(|task| matches!(task.status.as_str(), "completed" | "done"));
            let paths = TaskPaths::for_task(task_id, &spec)?;
            validate_owned_task_directory(&paths)?;
            let published = if completed || paths.task_dir().join("published.path").is_file() {
                Some(resolved_task_output(&core, task_id, &paths))
            } else {
                None
            };
            (spec, published)
        };
        let paths = TaskPaths::for_task(task_id, &spec)?;
        // 改名后的产物属于此任务，原始同名文件属于其他任务；未发布的任务没有成品可删。
        stop_task_player(self, task_id)?;
        if let Some(published) = published {
            if !published_path_allowed(&published, &paths) {
                return Err("拒绝删除任务输出根目录".into());
            }
            remove_task_artifact(&published)?;
        }
        remove_owned_task_directory(&paths, None)
    }

    fn cleanup_task_cache(&self, task_id: &str) -> Result<(), String> {
        let (paths, published) = {
            let core = self.lock()?;
            let spec = core
                .task_spec(task_id)
                .ok_or_else(|| format!("unknown task {task_id}"))?;
            let paths = TaskPaths::for_task(task_id, spec)?;
            let published = if paths.task_dir().join("published.path").is_file()
                || core
                    .snapshot(task_id)
                    .is_some_and(|task| matches!(task.status.as_str(), "completed" | "done"))
            {
                Some(resolved_task_output(&core, task_id, &paths))
            } else {
                None
            };
            (paths, published)
        };
        stop_task_player(self, task_id)?;
        remove_owned_task_directory(&paths, published.as_deref())
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
            let mut core = self.lock()?;
            let status = core.snapshot(&task_id).map(|task| task.status.clone());
            if !matches!(status.as_deref(), Some("queued" | "downloading")) {
                return Ok(());
            }
            if status.as_deref() == Some("queued") {
                if let Some(spec) = spec.as_ref() {
                    let paths = TaskPaths::for_task(&task_id, spec)?;
                    if let Err(error) = paths.prepare().and_then(|_| paths.set_control("run")) {
                        // 无效目录只阻止当前任务，不能把它留在队列头阻塞所有后续下载。
                        core.report_failure(
                            &task_id,
                            task_failure_from_error(
                                &error,
                                "waiting",
                                &spec.url,
                                0,
                                spec.credential_ref.is_some(),
                            ),
                        )?;
                        return Ok(());
                    }
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
                        .snapshot(&task_id)
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
                    let current = core.snapshot(&task_id).cloned().unwrap_or_default();
                    let downloaded = current.downloaded_bytes;
                    let total = current.total_bytes;
                    let stage = if status == "paused" {
                        "waiting".to_string()
                    } else if status == "failed" {
                        failure_stage(&current.stage, &error)
                    } else {
                        "finished".to_string()
                    };
                    if status == "failed" {
                        let (url, has_credential_ref) = core
                            .task_spec(&task_id)
                            .map(|spec| (spec.url.clone(), spec.credential_ref.is_some()))
                            .unwrap_or_default();
                        let failure = task_failure_from_error(
                            &error,
                            &stage,
                            &url,
                            attempt,
                            has_credential_ref,
                        );
                        crate::record_runtime_log(
                            "task_failed",
                            serde_json::json!({
                                "task_id": task_id, "stage": stage, "code": failure.code,
                                "http_status": failure.http_status, "attempt": attempt,
                                "downloaded_bytes": downloaded, "total_bytes": total,
                            }),
                        );
                        let _ = core.report_failure(&task_id, failure);
                    } else {
                        let _ = core.handle(CoreCommand::UpdateProgress {
                            task_id: task_id.clone(),
                            downloaded_bytes: downloaded,
                            total_bytes: total,
                            speed_bytes_per_sec: 0,
                            stage: stage.clone(),
                            status: status.into(),
                        });
                    }
                    // 常驻 Core 的启动器可能已关闭诊断管道；日志写失败不能污染任务锁。
                    let _ = writeln!(std::io::stderr(), "v7 task {task_id} {status}: {error}");
                });
                if status == "failed" {
                    let max = coordinator
                        .lock()
                        .ok()
                        .and_then(|guard| {
                            guard.store().setting_u64("auto_retry_failed_max", 0).ok()
                        })
                        .unwrap_or(0);
                    // HTTP 引擎已经完成传输级重试。鉴权、失效地址等错误需要用户更新请求，
                    // 整任务调度不能绕过相同的判定，再用原请求重复下载。
                    if u64::from(attempt) <= max
                        && max > 0
                        && !error.starts_with("resource identity changed:")
                        && !error.starts_with("range response rejected:")
                        && extract_http_status(&error)
                            .is_none_or(crate::http_engine::retryable_http_status)
                    {
                        let (downloaded, total) = current_progress(&core, &task_id);
                        let _ =
                            mark_progress(&core, &task_id, downloaded, total, "waiting", "queued");
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
            let _ = coordinator.start_next_queued(false);
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

fn task_schedule_allowed(task: &crate::TaskSnapshot) -> bool {
    crate::net_policy::scheduled_start_reached(&task.scheduled_start_at)
        && !crate::net_policy::scheduled_stop_hit(&task.scheduled_stop_at)
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
                .snapshot(task_id)
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
    if spec.local_source_path.is_some() {
        let paths = TaskPaths::for_task(task_id, &spec)?;
        paths.prepare()?;
        let payload = copy_local_source_payload(&core, task_id, &paths, &spec)?;
        return complete_payload(&core, task_id, &paths, &payload, &spec);
    }
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
            let height = spec.preferred_height;
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
                        height,
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
                // The non-torrent transports run ensure_publish_allowed inside
                // complete_payload before the payload is moved into place; this branch
                // publishes without that check, so a pause or cancel that lands in the
                // finalization window is ignored and the files are published and the task
                // marked complete anyway.
                ensure_publish_allowed(&paths.control)?;
                let meta = crate::torrent_engine::probe_torrent_source(
                    &latest_spec.url,
                    &headers,
                    &latest_spec.proxy,
                    enable_dht,
                )?;
                ensure_publish_allowed(&paths.control)?;
                let published = crate::torrent_engine::materialize_selected_files(
                    &paths.output,
                    &paths.final_output,
                    &meta,
                    &latest_spec.torrent_selection,
                )?;
                remember_published(&paths, &paths.final_output);
                // The selected files are already in place and recorded, so this tail
                // must not fail the task either: the caller would mark it failed and
                // re-queue it, re-downloading files the user already has — the same
                // regression finish_published_task closes for every other transport.
                // `published` is the byte count materialize_selected_files wrote, not
                // a file size: final_output is the selection directory.
                finish_published_task(
                    &core,
                    task_id,
                    &paths,
                    &paths.final_output,
                    Some(published),
                    &latest_spec,
                    false,
                )
            } else {
                complete_payload(&core, task_id, &paths, &paths.output, &spec)
            }
        }
        crate::ResourceKind::File => run_http_file(core, task_id, spec, replay_json),
    }
}

fn copy_local_source_payload(
    core: &Arc<Mutex<PersistentCore>>,
    task_id: &str,
    paths: &TaskPaths,
    spec: &TaskSpec,
) -> Result<PathBuf, String> {
    let source_path = spec
        .local_source_path
        .as_deref()
        .ok_or_else(|| "本地文件任务缺少源路径".to_string())?;
    let expected_size = spec
        .expected_size
        .ok_or_else(|| "本地文件任务缺少声明的文件大小".to_string())?;
    let source_path = validate_local_source_path(&spec.url, source_path, Some(expected_size))?;
    let mut source = fs::File::open(&source_path).map_err(|error| error.to_string())?;
    let initial = source.metadata().map_err(|error| error.to_string())?;
    let initial_path = fs::metadata(&source_path).map_err(|error| error.to_string())?;
    if !same_source_metadata(&initial, &initial_path) {
        return Err("本地源文件在打开时发生变化".into());
    }

    let mut output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&paths.output)
        .map_err(|error| error.to_string())?;
    let mut buffer = [0u8; 1024 * 1024];
    let mut copied = 0u64;
    loop {
        ensure_publish_allowed(&paths.control)?;
        let count = source
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
        copied = copied.saturating_add(count as u64);
        mark_progress(
            core,
            task_id,
            copied,
            Some(expected_size),
            "transfer",
            "downloading",
        )?;
    }
    ensure_publish_allowed(&paths.control)?;
    output.flush().map_err(|error| error.to_string())?;
    let final_handle = source.metadata().map_err(|error| error.to_string())?;
    let final_path = fs::metadata(&source_path).map_err(|error| error.to_string())?;
    if !same_source_metadata(&initial, &final_handle)
        || !same_source_metadata(&initial, &final_path)
    {
        return Err("本地源文件在复制期间发生变化".into());
    }
    if copied != expected_size {
        return Err(format!(
            "本地源复制大小不匹配: 声明 {expected_size} 字节，实际 {copied} 字节"
        ));
    }
    Ok(paths.output.clone())
}

fn same_source_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.is_file()
        && right.is_file()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.created().ok() == right.created().ok()
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
    let (initial_bytes, _) = current_progress(&core, task_id);
    let mut rate = crate::http_engine::TransferRate::new(initial_bytes);
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
                let report = result.map_err(|error| match error {
                    crate::http_engine::EngineError::RangeUnsupported(message)
                        if !message.starts_with("resource identity changed:") =>
                    {
                        format!("range response rejected: {message}")
                    }
                    error => error.to_string(),
                })?;
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
                        rate.sample(progress.downloaded),
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
    if value.len() > max_bytes.div_ceil(3) * 4 + 4 {
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
    let (code, hint) = if lower.starts_with("range response rejected:") {
        (
            "RANGE_UNSUPPORTED".into(),
            "服务器未按请求返回有效的分段数据，无法安全续传。已下载的缓存会保留；请回到来源页面重新获取地址并新建任务，不要反复重试原任务".into(),
        )
    } else if let Some(status) = http_status {
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
    } else if lower.starts_with("resource identity changed:") {
        (
            "RESOURCE_CHANGED".into(),
            "服务端文件内容已变化，已保存的片段不会与新内容拼接。请新建任务下载当前文件，原任务缓存已保留".into(),
        )
    } else if lower.starts_with("ffmpeg ") {
        (
            "MEDIA_MERGE_FAILED".into(),
            "分片已保留，请查看合流日志；修正问题后重试可复用已下载的分片".into(),
        )
    } else if is_size_disagreement(&lower) {
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

/// Every transport words a size disagreement differently, so the classifier has
/// to recognise all of them.  Matching only the literal "size mismatch" silently
/// dropped HTTP body/range, SFTP and web-seed failures into the generic hint,
/// which told the user to re-check the URL instead of telling them the resource
/// itself had changed.
fn is_size_disagreement(lower: &str) -> bool {
    lower.contains("size mismatch")
        || lower.contains("length mismatch")
        || lower.contains("大小不匹配")
        || lower.contains("长度不匹配")
}
fn failure_stage(current: &str, error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.starts_with("ffmpeg ") {
        "merging".into()
    } else if lower.contains("checksum") || lower.contains("校验") {
        "checksum".into()
    } else if is_size_disagreement(&lower) {
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
    reject_html_media_payload(payload, spec)?;
    let actual = fs::metadata(payload)
        .map_err(|error| error.to_string())?
        .len();
    mark_progress(
        core,
        task_id,
        actual,
        spec.expected_size.or(Some(actual)),
        "checking",
        "checking",
    )?;
    if let Some(expected) = spec.expected_size {
        if actual != expected {
            mark_progress(core, task_id, actual, Some(expected), "size", "checking")?;
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
                    mark_progress(core, task_id, actual, Some(actual), "checksum", "checking")?;
                    return Err(format!(
                        "checksum mismatch: expected {}, got {}",
                        result.expected, result.actual
                    ));
                }
            }
            Ok(None) => {}
            Err(error) => {
                mark_progress(core, task_id, actual, Some(actual), "checksum", "checking")?;
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
        // `error` 不是扫描结果，是"扫描没跑起来"（引擎崩溃/超时、命令被校验
        // 拒绝、产物消失）。用户把 av_scan_enabled 打开，产品就不能用"没查出
        // 威胁"冒充"查过了"。这一条不受 fail_on_threat 影响：那个开关只决定
        // "发现威胁时是否阻止"，不决定"扫描状态未知时是否放行"。
        if result.state == "error" {
            mark_progress(core, task_id, actual, Some(actual), "av_scan", "checking")?;
            return Err(format!("av_scan_error: {}", result.detail));
        }
        if result.state == "threat" && fail_on_threat {
            mark_progress(core, task_id, actual, Some(actual), "av_scan", "checking")?;
            return Err(format!("av_threat: {}", result.detail));
        }
    }
    let (policy, keep_temp) = output_policy(core);
    let download_subtitles = core
        .lock()
        .ok()
        .and_then(|guard| guard.store().setting_bool("download_subtitles", true).ok())
        .unwrap_or(true);
    // Everything from here on runs after the payload is already at its final
    // location, so a store or power-action failure in this tail must not turn a
    // finished download into a failed task: the caller would mark it failed and
    // re-queue it, downloading a file the user already has while the on-disk state
    // contradicts the reported status. finish_published_task logs instead.
    ensure_publish_allowed(&paths.control)?;
    let metadata = fs::read(paths.progress.with_extension("response.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or_default();
    let destination = crate::output_path::typed_output_path(
        payload,
        &paths.final_output,
        metadata
            .get("content_type")
            .and_then(Value::as_str)
            .unwrap_or(&spec.mime_type),
        metadata
            .get("content_disposition")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )?;
    let published = crate::output_path::publish_file(payload, &destination, &policy, keep_temp)?;
    remember_published(paths, &published);
    // Returns Ok by construction; the signature keeps the "must never fail" contract
    // visible and lets the regression test assert it directly.
    finish_published_task(
        core,
        task_id,
        paths,
        &published,
        None,
        spec,
        download_subtitles,
    )
}

fn reject_html_media_payload(payload: &Path, spec: &TaskSpec) -> Result<(), String> {
    let mime = spec.mime_type.to_ascii_lowercase();
    let extension = Path::new(&spec.filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !mime.starts_with("video/")
        && !mime.starts_with("audio/")
        && !matches!(
            extension.as_str(),
            "mp4"
                | "m4v"
                | "mkv"
                | "webm"
                | "mov"
                | "avi"
                | "ts"
                | "mp3"
                | "m4a"
                | "aac"
                | "ogg"
                | "opus"
                | "flac"
                | "wav"
        )
    {
        return Ok(());
    }
    // 登录页常以 HTTP 200 返回；只读文件头，不能把它作为媒体成品发布。
    let mut prefix = [0u8; 4096];
    let size = fs::File::open(payload)
        .and_then(|mut file| file.read(&mut prefix))
        .map_err(|error| format!("读取媒体文件头失败: {error}"))?;
    let text = String::from_utf8_lossy(&prefix[..size]);
    let text = text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    if [
        "<!doctype html",
        "<html",
        "<head",
        "<body",
        "<script",
        "<form",
    ]
    .iter()
    .any(|tag| text.starts_with(tag))
    {
        return Err(
            "服务器返回了 HTML 登录页或错误页，未保存为媒体；请登录原网页并重新识别链接后重试"
                .into(),
        );
    }
    Ok(())
}

/// Finishes a task whose payload is already published at `published`.
///
/// Every step is best effort and none of them is allowed to fail the task, for the
/// reasons spelled out at the call site. The Core mutex is deliberately not held
/// across the MOTW write or the subtitle copy: `publish_file` may already have
/// copied the whole payload (a different volume, or `keep_temp_files`), and every
/// other worker plus every IPC request takes this same mutex.
///
/// `total` is the size to report when the caller already knows it. The torrent
/// selection branch publishes a *directory*, so measuring it would report the
/// directory's own metadata instead of the bytes it just wrote.
fn finish_published_task(
    core: &Arc<Mutex<PersistentCore>>,
    task_id: &str,
    paths: &TaskPaths,
    published: &Path,
    total: Option<u64>,
    spec: &TaskSpec,
    download_subtitles: bool,
) -> Result<(), String> {
    let total = total.or_else(|| fs::metadata(published).ok().map(|meta| meta.len()));
    crate::motw::mark_downloaded_file(published, &spec.url);
    let mut keep_subtitles = false;
    if download_subtitles {
        if let Err(error) = copy_subtitle_sidecars(&paths.task_dir(), published) {
            keep_subtitles = true;
            let _ = writeln!(
                std::io::stderr(),
                "v7 task {task_id} subtitle copy failed: {error}"
            );
        }
    }
    // 与开始播放串行，避免播放器刚挂载分片就被收尾删除。
    let player_state = player_ui_state()
        .lock()
        .map_err(|_| "player session lock".to_string());
    let mut thumbnail_guard = None;
    let playback_released = match &player_state {
        Ok(state) if state.active && state.task_id == task_id => {
            crate::player_thumbnail::cancel_and_wait().and_then(|guard| {
                thumbnail_guard = Some(guard);
                shared_player().and_then(|player| {
                    player.finish_download(&published.to_string_lossy(), &state.title)
                })
            })
        }
        Ok(_) => Ok(()),
        Err(error) => Err(error.clone()),
    };
    let mut recorded = false;
    // 先持久化实际成品路径；只有记录成功，才能删除 published.path 恢复信息。
    match core.lock() {
        Ok(mut guard) => {
            let saved = guard.set_output_path(task_id, published.to_string_lossy().into_owned());
            if let Err(error) = &saved {
                let _ = writeln!(
                    std::io::stderr(),
                    "v7 task {task_id} was published but recording its output path failed: {error}"
                );
            }
            let completed = guard.handle(CoreCommand::UpdateProgress {
                task_id: task_id.into(),
                downloaded_bytes: total.unwrap_or(0),
                total_bytes: total,
                speed_bytes_per_sec: 0,
                stage: "finished".into(),
                status: "completed".into(),
            });
            recorded = saved.is_ok() && completed.is_ok();
            if let Err(error) = completed {
                let _ = writeln!(
                    std::io::stderr(),
                    "v7 task {task_id} was published but publishing its completion failed: {error}"
                );
            }
        }
        Err(_) => {
            let _ = writeln!(
                std::io::stderr(),
                "v7 task {task_id} was published but the Core mutex was poisoned"
            );
        }
    }
    // 完成状态和成品路径在播放器锁内持久化；后续播放只能挂载成品。
    // 递归清理可能被慢盘或杀毒扫描拖住，不能继续占用所有播放控制共用的锁。
    drop(player_state);
    drop(thumbnail_guard);
    let keep_temp = core
        .lock()
        .ok()
        .and_then(|guard| guard.store().setting_bool("keep_temp_files", false).ok())
        .unwrap_or(true);
    if !keep_temp {
        let cleanup = playback_released.and_then(|_| {
            if !recorded {
                return Err("成品状态未能保存，保留缓存以便恢复".to_string());
            }
            #[cfg(test)]
            BEFORE_COMPLETED_CACHE_CLEANUP.with(|hook| {
                if let Some(hook) = hook.borrow_mut().take() {
                    hook();
                }
            });
            cleanup_completed_media(paths, published, keep_subtitles)?;
            if !keep_subtitles {
                cleanup_task_metadata(paths)?;
            }
            Ok(())
        });
        if let Err(error) = cleanup {
            let _ = writeln!(
                std::io::stderr(),
                "v7 task {task_id} cache cleanup failed: {error}"
            );
            if let Ok(mut guard) = core.lock() {
                let _ = guard.emit(CoreEvent::Toast {
                    level: "warning".into(),
                    message: format!(
                        "下载已完成，但缓存未完全清理：{error}。可用“清理已完成”重试。"
                    ),
                });
            }
        }
    }
    if let Err(error) = maybe_schedule_power(core, spec) {
        let _ = writeln!(
            std::io::stderr(),
            "v7 task {task_id} was published but its completion power action failed: {error}"
        );
    }
    Ok(())
}

fn is_link_metadata(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn cleanup_task_metadata(paths: &TaskPaths) -> Result<(), String> {
    let dir = paths.task_dir();
    let Some(parent) = dir.parent() else {
        return Ok(());
    };
    if !matches!(
        parent.file_name().and_then(|name| name.to_str()),
        Some(".hls-tasks")
    ) {
        return Ok(());
    }
    for name in [
        "control",
        "progress.json",
        "progress.json.tmp",
        "torrent-selection.json",
        "published.path",
        "progress.response.json",
        "native-engine.active.json.tmp",
    ] {
        remove_task_artifact(&dir.join(name))?;
    }
    match fs::remove_dir(&dir) {
        Ok(()) => {
            let _ = fs::remove_dir(parent);
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) => {}
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    }
    Ok(())
}

fn remove_task_artifact(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    // 不遍历符号链接或 Windows junction，避免缓存路径被重定向到其他目录。
    if is_link_metadata(&metadata) {
        return Err(format!("拒绝清理重解析路径: {}", path.display()));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            remove_task_artifact(&entry.map_err(|error| error.to_string())?.path())?;
        }
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|error| format!("{}: {error}", path.display()))
}

fn validate_owned_task_directory(paths: &TaskPaths) -> Result<(), String> {
    let dir = paths.task_dir();
    let parent = dir.parent().ok_or("任务缓存目录没有父目录")?;
    if !matches!(
        parent.file_name().and_then(|name| name.to_str()),
        Some(".hls-tasks")
    ) {
        return Err("拒绝清理非任务缓存目录".into());
    }
    for path in [parent, dir.as_path()] {
        match fs::symlink_metadata(path) {
            Ok(meta) if is_link_metadata(&meta) => {
                return Err(format!("拒绝清理重解析路径: {}", path.display()))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn remove_owned_task_directory(paths: &TaskPaths, published: Option<&Path>) -> Result<(), String> {
    validate_owned_task_directory(paths)?;
    let dir = paths.task_dir();
    let parent = dir.parent().ok_or("任务缓存目录没有父目录")?;
    if published.is_some_and(|path| logical_canonical(path).starts_with(logical_canonical(&dir))) {
        return Err("成品位于任务缓存目录内，请先将成品移出后再删除任务".into());
    }
    remove_task_artifact(&dir)?;
    // 仅尝试移除空的任务容器；有其他任务时不会递归删除。
    let _ = fs::remove_dir(parent);
    Ok(())
}

fn cleanup_completed_media(
    paths: &TaskPaths,
    published: &Path,
    keep_subtitles: bool,
) -> Result<(), String> {
    let task_dir = paths.task_dir();
    for path in [Some(task_dir.as_path()), task_dir.parent()]
        .into_iter()
        .flatten()
    {
        match fs::symlink_metadata(path) {
            Ok(meta) if is_link_metadata(&meta) => {
                return Err(format!("拒绝清理重解析路径: {}", path.display()))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
    }
    let root = logical_canonical(&task_dir);
    let published = logical_canonical(published);
    // 成品不存在时不能销毁仅存的分片；即使成品被配置在工作目录内，也必须保留。
    if !published.exists() {
        return Err("published media is missing; retaining temporary files".into());
    }
    let entries = match fs::read_dir(&task_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let mut first_error = None;
    for entry in entries {
        let result = (|| -> Result<(), String> {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // 只删媒体引擎生成的内容；保留成品定位、进度统计和未知文件。
            let temporary = matches!(
                name.as_ref(),
                "segments"
                    | "audio"
                    | "subs"
                    | "live-subtitles"
                    | "payload.downloading"
                    | "merged.mp4"
                    | "muxed.mp4"
                    | "init.mp4"
                    | "local.m3u8"
                    | "preview.m3u8"
                    | "preview-video.m3u8"
                    | "playback-ready"
                    | "playback-priority"
                    | ".playback-priority.tmp"
                    | "playback-byte-priority"
                    | ".playback-byte-priority.tmp"
                    | "vod_segments.json"
                    | "dash_vod_segments.json"
                    | ".vod_segments.json.tmp"
                    | ".dash_vod_segments.json.tmp"
                    | "live_state.json"
                    | "live_subtitles.json"
                    | "audio.control"
                    | "request-body.bin"
                    | "native-engine.ranges.json"
                    | "native-engine.active.json"
                    | "native-engine.ranges.json.tmp"
                    | "native-engine.active.json.tmp"
                    | "progress.response.json"
                    | "payload.progress.json"
                    | "payload.progress.response.json"
                    | "init.progress.json"
                    | "init.progress.response.json"
                    | "sftp-resume.json"
                    | "ffmpeg-merge.log"
                    | "muxed.ffmpeg.log"
                    | "merged.ffmpeg.log"
                    | ".local.m3u8.tmp"
                    | ".preview.m3u8.tmp"
                    | ".preview-video.m3u8.tmp"
                    | ".live_state.json.tmp"
            ) || name
                .strip_prefix("seg-")
                .and_then(|name| name.split_once('.'))
                .is_some_and(|(index, suffix)| {
                    !index.is_empty()
                        && index.bytes().all(|ch| ch.is_ascii_digit())
                        && matches!(suffix, "m4s" | "progress.json" | "progress.response.json")
                })
                || (name.starts_with("torrent-selection.json.") && name.ends_with(".tmp"));
            if !temporary || (keep_subtitles && name == "subs") {
                return Ok(());
            }
            let path = entry.path();
            let canonical = logical_canonical(&path);
            if !canonical.starts_with(&root) {
                return Err(format!("拒绝清理任务目录外的路径: {}", path.display()));
            }
            if published.starts_with(&canonical) {
                return Ok(());
            }
            remove_task_artifact(&path)
        })();
        // 单个文件被占用时继续清理其它分片，不让一次失败保留整个视频的缓存。
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
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
    let marker = paths.task_dir().join("published.path");
    if marker.is_file() {
        if let Ok(text) = fs::read_to_string(marker) {
            let candidate = PathBuf::from(text.trim());
            // 已记录的文件丢失时仍保留其路径，不能回退到其他任务的同名成品。
            if published_path_allowed(&candidate, paths) {
                return candidate;
            }
        }
        return paths.output.clone();
    }
    if paths.final_output.exists() {
        paths.final_output.clone()
    } else {
        paths.output.clone()
    }
}

fn resolved_task_output(core: &PersistentCore, task_id: &str, paths: &TaskPaths) -> PathBuf {
    if !paths.task_dir().join("published.path").exists() {
        if let Some(task) = core
            .snapshot(task_id)
            .filter(|task| matches!(task.status.as_str(), "completed" | "done"))
        {
            let saved = PathBuf::from(&task.output_path);
            if !task.output_path.is_empty() && published_path_allowed(&saved, paths) {
                return saved;
            }
        }
    }
    resolve_published(paths)
}

fn published_path_allowed(candidate: &Path, paths: &TaskPaths) -> bool {
    let canon = logical_canonical(candidate);
    let mut roots = vec![logical_canonical(&paths.task_dir())];
    if let Some(parent) = paths.final_output.parent() {
        roots.push(logical_canonical(parent));
    }
    roots
        .iter()
        .any(|root| canon != *root && canon.starts_with(root))
}

fn copy_subtitle_sidecars(task_dir: &Path, published: &Path) -> Result<(), String> {
    let subs = task_dir.join("subs");
    let entries = match fs::read_dir(subs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let stem = published
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let parent = published.parent().unwrap_or(Path::new("."));
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
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
            return Err(format!("invalid subtitle filename: {}", path.display()));
        }
        let dest = parent.join(format!("{stem}.{lang}.{ext}"));
        let parent_canon = logical_canonical(parent);
        let dest_canon = logical_canonical(&dest);
        if dest_canon != parent_canon && !dest_canon.starts_with(&parent_canon) {
            return Err(format!(
                "subtitle destination escapes output directory: {}",
                dest.display()
            ));
        }
        fs::copy(&path, &dest)
            .map_err(|error| format!("copy subtitle {}: {error}", path.display()))?;
    }
    Ok(())
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
    let initial_bytes = core
        .lock()
        .ok()
        .and_then(|core| core.snapshot(task_id).map(|task| task.downloaded_bytes))
        .unwrap_or(0);
    let mut rate = crate::http_engine::TransferRate::new(initial_bytes);
    let mut last_downloaded = initial_bytes;
    loop {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(item) = read_progress(progress) {
                    // 恢复时先枚举视频缓存再枚举音轨缓存，不能把未枚举完整的前缀当成进度倒退。
                    if item.downloaded < last_downloaded {
                        continue;
                    }
                    last_downloaded = item.downloaded;
                    let merging = item.status == "merging";
                    let speed = if merging {
                        0
                    } else {
                        rate.sample(item.downloaded)
                    };
                    let _ = mark_progress_speed(
                        core,
                        task_id,
                        item.downloaded,
                        (item.total > 0).then_some(item.total),
                        speed,
                        if merging { "merging" } else { "transfer" },
                        if merging {
                            "merging"
                        } else if item.status == "recording" {
                            "recording"
                        } else {
                            status
                        },
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
                "system" => spec.proxy = crate::net_policy::SYSTEM_PROXY_SENTINEL.into(),
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

pub(crate) fn task_throttle_context(
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
        control: TaskPaths::for_task(task_id, spec)?.control,
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
    // 已识别的同一资源保留媒体类型；导入/采集出来的新 URL 才重新分类。
    if url != template.url || spec.resource_kind == crate::ResourceKind::File {
        spec.resource_kind = crate::classify_url(url);
    }
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
    let paused = metadata.paused.unwrap_or(state.paused);
    let active = metadata.session_active.unwrap_or(state.active);
    let status = if !active {
        "STOPPED"
    } else if paused {
        "PAUSED"
    } else if state.status == "PAUSED" {
        "PLAYING"
    } else {
        &state.status
    };
    coordinator.lock()?.emit(CoreEvent::PlayerSession {
        active,
        title: state.title.clone(),
        task_id: state.task_id.clone(),
        status: status.into(),
        paused,
        speed: metadata.speed.unwrap_or(state.speed),
        position_seconds: metadata.position_seconds,
        duration_seconds: metadata.duration_seconds,
        position_available: metadata.position_available,
        audio_tracks: metadata.audio_tracks,
        subtitle_tracks: metadata.subtitle_tracks,
        volume: metadata.volume,
        muted: metadata.muted,
        buffered_end_seconds: metadata.buffered_end_seconds,
        buffering: metadata.buffering,
        tracks: metadata.tracks,
    })
}

fn play_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<Vec<EventEnvelope>, String> {
    let mut current = player_ui_state()
        .lock()
        .map_err(|_| "player session lock")?;
    let _thumbnails = crate::player_thumbnail::cancel_and_wait()?;
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let url = mount_task_url(coordinator, task_id, shared_local_media()?)?;
    let title = if spec.title.trim().is_empty() {
        spec.filename
    } else {
        spec.title
    };
    shared_player()?.play(&url, &title)?;
    let state = PlayerUiState {
        active: true,
        title,
        task_id: task_id.to_string(),
        status: "PLAYING".into(),
        paused: false,
        speed: 1.0,
    };
    *current = state.clone();
    drop(current);
    emit_player_session(coordinator, &state)
}

fn stop_task_player(coordinator: &CoreCoordinator, task_id: &str) -> Result<(), String> {
    let mut state = player_ui_state()
        .lock()
        .map_err(|_| "player session lock")?;
    if state.active && state.task_id == task_id {
        let _thumbnails = crate::player_thumbnail::cancel_and_wait()?;
        shared_player()?.stop();
        *state = PlayerUiState::default();
        emit_player_session(coordinator, &state)?;
    }
    Ok(())
}

pub(crate) fn playable_task_playlist(paths: &TaskPaths) -> Option<PathBuf> {
    // preview 清单只在连续分片及所需音轨均就绪时发布。
    let playlist = paths.task_dir().join("preview.m3u8");
    playlist.is_file().then_some(playlist)
}

pub(crate) fn playable_http_total(paths: &TaskPaths, spec: &TaskSpec) -> Option<u64> {
    if spec.resource_kind != crate::ResourceKind::File {
        return None;
    }
    let extension = paths
        .final_output
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "mp4"
            | "mkv"
            | "webm"
            | "mov"
            | "avi"
            | "m4v"
            | "mp3"
            | "m4a"
            | "aac"
            | "flac"
            | "ogg"
            | "opus"
            | "wav"
            | "ts"
    ) {
        return None;
    }
    let checkpoint: Value = serde_json::from_str(
        &fs::read_to_string(paths.task_dir().join("native-engine.ranges.json")).ok()?,
    )
    .ok()?;
    let total = checkpoint["total"].as_u64().filter(|total| *total > 0)?;
    if checkpoint["version"].as_u64() != Some(2)
        || checkpoint["ranges"].as_array()?.is_empty()
        || paths.output.metadata().ok()?.len() != total
    {
        return None;
    }
    Some(total)
}

fn mount_task_url(
    coordinator: &CoreCoordinator,
    task_id: &str,
    server: &crate::playback::MediaServer,
) -> Result<String, String> {
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let paths = TaskPaths::for_task(task_id, &spec)?;
    let token = crate::playback::random_mount_token();
    let (completed, published) = {
        let core = coordinator.lock()?;
        let completed = core
            .snapshot(task_id)
            .is_some_and(|task| matches!(task.status.as_str(), "completed" | "done"));
        (completed, resolved_task_output(&core, task_id, &paths))
    };
    let url = if completed {
        if !published.is_file() {
            return Err("下载成品不存在".into());
        }
        server.mount(&token, published);
        server.url_for(&token)
    } else if playable_task_playlist(&paths).is_some() {
        server.mount_downloading(&token, paths.task_dir());
        format!("{}/preview.m3u8", server.url_for(&token))
    } else if let Some(total) = playable_http_total(&paths, &spec) {
        crate::media::preview::request_byte_priority(&paths.task_dir(), 0)?;
        server.mount_growing(&token, paths.task_dir(), paths.output.clone(), total);
        server.url_for(&token)
    } else {
        return Err(if completed {
            "下载成品不存在"
        } else {
            "可播放的分片、音轨或文件字节范围尚未就绪；请稍候或等待下载完成"
        }
        .into());
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

fn cast_lan_host(device_id: &str) -> Result<String, String> {
    let address = if device_id.trim().is_empty() {
        crate::cast::preferred_lan_ipv4()
    } else {
        crate::cast::device_lan_ipv4(device_id)
    };
    address
        .map(|ip| ip.to_string())
        .ok_or_else(|| "无法确定到投屏设备的局域网出口地址".to_string())
}

fn cast_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<Vec<EventEnvelope>, String> {
    let host = cast_lan_host("")?;
    let loopback = mount_task_url(coordinator, task_id, shared_media()?)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let server = shared_media()?;
    server.enable_lan();
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
    let host = cast_lan_host(device_id)?;
    let loopback = mount_task_url(coordinator, task_id, shared_media()?)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let server = shared_media()?;
    server.enable_lan();
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
        let host = cast_lan_host(device_id)?;
        server.mount(&token, source);
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
            let host = cast_lan_host("")?;
            server.mount_remote(&token, media_url.clone());
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
                let job = crate::http_engine::Job {
                    url: candidate.to_string(),
                    headers: headers.clone(),
                    // 大小探测没有成品，留空以跳过下载后缀元数据落盘。
                    output: PathBuf::new(),
                    connections: 1,
                    chunk_bytes: 1024 * 1024,
                    total: 0,
                    sequential: true,
                    resume_from: 0,
                    proxy: proxy.clone(),
                    resource_key: candidate.to_string(),
                    etag: String::new(),
                    last_modified: String::new(),
                    control: PathBuf::new(),
                    progress: PathBuf::new(),
                    method: "GET".into(),
                    body_path: PathBuf::new(),
                    mirrors: Vec::new(),
                    replay_json: String::new(),
                    curl_body_limit: None,
                    origin_boundary: String::new(),
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
    let loopback = mount_task_url(coordinator, task_id, shared_media()?)?;
    let token = media_token_from_url(&loopback).ok_or_else(|| "播放地址无效".to_string())?;
    let server = shared_media()?;
    server.enable_lan();
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
    let host = crate::cast::endpoint_lan_ipv4(&endpoint)
        .map(|ip| ip.to_string())
        .ok_or_else(|| "无法确定到 TVBox 的局域网出口地址".to_string())?;
    let url = crate::cast::lan_media_url(server, &token, &host)?;
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

fn prepend_configured_tvbox(devices: &mut Vec<crate::CastDeviceInfo>, configured_endpoint: &str) {
    let endpoint = configured_endpoint.trim().trim_end_matches('/').to_string();
    if endpoint.is_empty()
        || devices.iter().any(|device| {
            device.service_type.eq_ignore_ascii_case("tvbox")
                && device
                    .control_url
                    .trim()
                    .trim_end_matches('/')
                    .eq_ignore_ascii_case(&endpoint)
        })
    {
        return;
    }
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
            prepend_configured_tvbox(&mut devices, &endpoint);
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
    let message = if crate::cast::tun_mode_active() {
        format!("{message}；已识别 TUN 模式并使用物理局域网")
    } else {
        message
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

fn check_update(coordinator: &CoreCoordinator, silent: bool) -> Result<Vec<EventEnvelope>, String> {
    // 网络请求完成后再获取任务/数据库共享锁，避免阻塞进度写入和其他客户端。
    let event = match crate::updater::check_for_update(crate::updater::CURRENT_VERSION) {
        Ok(info) if info.newer => CoreEvent::UpdateAvailable {
            current: info.current,
            latest: info.latest,
            notes: info.notes,
            release_url: info.html_url,
            installer_name: info.installer_name,
            installer_size: info.installer_size,
            sha256_verified: !info.expected_sha256.is_empty(),
        },
        Ok(info) if !silent => CoreEvent::UpdateCurrent {
            current: info.current,
        },
        Ok(_) => return Ok(Vec::new()),
        Err(error) if !silent => CoreEvent::Error {
            code: "update_failed".into(),
            message: error,
        },
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "silent update check failed: {error}");
            return Ok(Vec::new());
        }
    };
    coordinator.lock()?.emit(event)
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
    let published = resolved_task_output(&*coordinator.lock()?, task_id, &paths);
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
    let published = resolved_task_output(&*coordinator.lock()?, task_id, &paths);
    crate::write_clipboard_files(std::slice::from_ref(&published))?;
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
    let published = resolved_task_output(&*coordinator.lock()?, task_id, &paths);
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
        "status" => Ok(()),
        "hide" => player.hide_embed(),
        "pause" => player.pause(true),
        "resume" | "play" => player.pause(false),
        "fullscreen" => player.set_fullscreen(true),
        "windowed" => player.set_fullscreen(false),
        "pip" => player.set_pip(true),
        "unpip" => player.set_pip(false),
        "stop" => {
            let _thumbnails = crate::player_thumbnail::cancel_and_wait()?;
            player.stop();
            Ok(())
        }
        "vol_up" => player.adjust_volume(10.0),
        "vol_down" => player.adjust_volume(-10.0),
        "mute" => player.toggle_mute(),
        other if other.starts_with("volume:") => player.set_volume(
            other
                .trim_start_matches("volume:")
                .parse::<f64>()
                .map_err(|_| "音量无效")?,
        ),
        "seek_fwd" => player.seek_relative(10.0),
        "seek_back" => player.seek_relative(-10.0),
        other if other.starts_with("speed:") => {
            let speed = other
                .trim_start_matches("speed:")
                .parse::<f64>()
                .unwrap_or(1.0);
            player.set_speed(speed)
        }
        other if other.starts_with("seek_to:") => player.seek_absolute(
            other
                .trim_start_matches("seek_to:")
                .parse::<f64>()
                .map_err(|_| "播放时间无效")?,
        ),
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
        other if other.starts_with("embed_host:") || other.starts_with("embed_main:") => {
            let mut parts = other
                .split_once(':')
                .map(|(_, rect)| rect)
                .unwrap_or("")
                .split(',')
                .filter_map(|item| item.parse::<i32>().ok());
            let x = parts.next().unwrap_or(0);
            let y = parts.next().unwrap_or(48);
            let w = parts.next().unwrap_or(720);
            let h = parts.next().unwrap_or(220);
            let title = if other.starts_with("embed_main:") {
                "HLS Downloader"
            } else {
                crate::player::PLAYER_WINDOW_TITLE
            };
            player.attach_embed_host(title, x, y, w, h)
        }
        _ => Err(format!("unknown player action {action}")),
    }
}

fn player_control_events(
    coordinator: &CoreCoordinator,
    action: &str,
) -> Result<Vec<EventEnvelope>, String> {
    let state = {
        let mut state = player_ui_state()
            .lock()
            .map_err(|_| "player state lock".to_string())?;
        player_control(action)?;
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

fn shared_local_media() -> Result<&'static crate::playback::MediaServer, String> {
    static SERVER: OnceLock<Result<crate::playback::MediaServer, String>> = OnceLock::new();
    match SERVER.get_or_init(crate::playback::MediaServer::start_local) {
        Ok(server) => Ok(server),
        Err(error) => Err(error.clone()),
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
    status: String,
}

fn read_progress(path: &Path) -> Option<Progress> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    Some(Progress {
        downloaded: value.get("downloaded")?.as_u64()?,
        total: value.get("total").and_then(Value::as_u64).unwrap_or(0),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

#[cfg(test)]
std::thread_local! {
    static BEFORE_COMPLETED_CACHE_CLEANUP: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
mod tests;
