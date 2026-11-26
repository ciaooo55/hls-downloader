//! Durable static discovery and synchronization. File bodies are ordinary Core
//! tasks; discovery uses bounded reads through the same HTTP policy/transports.
use crate::{
    CoreCommand, CoreCoordinator, CoreEvent, CrawlProject, CrawlRequest, CrawlResource, CrawlSpec,
    TaskSpec,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_RESOURCES: usize = 2000;
const MAX_SEEN: usize = 4096;
const MAX_STATE_URL_BYTES: usize = 2 * 1024 * 1024;
const DOCUMENT_LIMIT: u64 = 1024 * 1024;
const PROJECT_LIMIT: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Page {
    url: String,
    depth: u32,
    kind: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SyncState {
    generation: u64,
    pending: bool,
    etag: String,
    modified: String,
    size: Option<u64>,
    digest: String,
    final_url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CrawlState {
    pub project: CrawlProject,
    pending: VecDeque<Page>,
    seen: BTreeSet<String>,
    cache_dir: String,
    metadata_bytes: u64,
    sync: BTreeMap<String, SyncState>,
    rewrite_needed: bool,
    next_resource: usize,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn canonical(url: &str) -> Option<String> {
    if !crate::http_engine::http_fetch_url_allowed(url) {
        return None;
    }
    // HTML entity decoding belongs to reference resolution, exactly once.
    let resolved = url;
    let origin = crate::http_engine::http_origin(resolved)?;
    let rest = resolved.split_once("://")?.1;
    let target = rest
        .find(['/', '?', '#'])
        .map(|i| &rest[i..])
        .unwrap_or("/")
        .split('#')
        .next()?;
    let (path, query) = target
        .split_once('?')
        .map(|(p, q)| (p, format!("?{q}")))
        .unwrap_or((target, String::new()));
    let mut decoded = String::new();
    let bytes = path.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && at + 2 < bytes.len()
            && bytes[at + 1].is_ascii_hexdigit()
            && bytes[at + 2].is_ascii_hexdigit()
        {
            if let Ok(n) = u8::from_str_radix(&path[at + 1..at + 3], 16) {
                if n.is_ascii_alphanumeric() || matches!(n, b'-' | b'.' | b'_' | b'~') {
                    decoded.push(n as char);
                    at += 3;
                    continue;
                }
            }
        }
        let ch = path[at..].chars().next()?;
        decoded.push(ch);
        at += ch.len_utf8();
    }
    Some(format!(
        "{origin}{}{query}",
        crate::harvest::normalize_url_path(if decoded.is_empty() { "/" } else { &decoded })
    ))
}
fn hash(url: &str) -> String {
    crate::crypto_lite::sha256_hex(url.as_bytes())
}
fn local_name(url: &str, kind: &str) -> String {
    let ext = match kind {
        "page" => "html",
        "style" => "css",
        _ => url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .rsplit('.')
            .next()
            .filter(|s| s.len() <= 10 && s.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("bin"),
    };
    format!("{}.{}", &hash(url)[..32], ext)
}
fn allowed(spec: &CrawlSpec, url: &str) -> bool {
    if spec.same_origin
        && crate::http_engine::http_origin(url) != crate::http_engine::http_origin(&spec.entry_url)
    {
        return false;
    }
    let rest = url.split_once("://").map(|(_, v)| v).unwrap_or("");
    let path = rest.find('/').map(|at| &rest[at..]).unwrap_or("/");
    (spec.include_paths.is_empty() || spec.include_paths.iter().any(|p| path.starts_with(p)))
        && !spec.exclude_paths.iter().any(|p| path.starts_with(p))
}
fn resource_allowed(spec: &CrawlSpec, url: &str, kind: &str) -> bool {
    if spec.offline || spec.extensions.is_empty() {
        return true;
    }
    let extension = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('.')
        .next()
        .unwrap_or("");
    kind != "page"
        && spec
            .extensions
            .iter()
            .any(|e| extension.eq_ignore_ascii_case(e.trim_start_matches('.')))
}
fn load(coordinator: &CoreCoordinator, id: &str) -> Result<CrawlState, String> {
    coordinator.lock()?.store().crawl_state(id)
}

pub(crate) fn dispatch(
    coordinator: &CoreCoordinator,
    request: CrawlRequest,
) -> Result<Vec<CrawlProject>, String> {
    if let CrawlRequest::Create { mut spec } = request {
        spec.entry_url = canonical(&spec.entry_url).ok_or("入口必须是有效 HTTP(S) 地址")?;
        if spec.max_depth > 10
            || spec.max_pages == 0
            || spec.max_pages > 500
            || !(1..=2).contains(&spec.page_concurrency)
            || (spec.sync_interval_secs != 0 && spec.sync_interval_secs < 60)
        {
            return Err("深度上限 10、页面上限 500、并发上限 2；同步周期至少 60 秒".into());
        }
        if spec.extensions.len() > 64
            || spec.include_paths.len() > 32
            || spec.exclude_paths.len() > 32
            || spec
                .include_paths
                .iter()
                .chain(&spec.exclude_paths)
                .any(|p| !p.starts_with('/') || p.len() > 1024)
            || spec.entry_url.len() > 8192
        {
            return Err("抓站过滤条件无效或超限".into());
        }
        let prepared = coordinator.metadata_spec(&spec.entry_url)?;
        let settings = coordinator.settings()?;
        let default_download_dir = if settings.download_dir.trim().is_empty() {
            prepared.download_dir.clone()
        } else {
            settings.download_dir.clone()
        };
        let root = if spec.output_dir.is_empty() {
            default_download_dir
        } else {
            crate::output_path::constrain_untrusted_download_dir(
                &spec.output_dir,
                &default_download_dir,
            )?
        };
        if !settings
            .queue_profiles
            .iter()
            .any(|q| q.id == spec.queue_id)
        {
            return Err("队列不存在".into());
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!("crawl-{stamp}");
        spec.output_dir = Path::new(&root).join(&id).to_string_lossy().into_owned();
        let cache_dir = Path::new(&prepared.work_dir)
            .join(&id)
            .to_string_lossy()
            .into_owned();
        let pending = VecDeque::from([Page {
            url: spec.entry_url.clone(),
            depth: 0,
            kind: "page".into(),
        }]);
        let seen = BTreeSet::from([spec.entry_url.clone()]);
        let state = CrawlState {
            project: CrawlProject {
                id,
                spec,
                status: "discovering".into(),
                reason: String::new(),
                visited_pages: 0,
                resources: Vec::new(),
            },
            pending,
            seen,
            cache_dir,
            metadata_bytes: 0,
            sync: BTreeMap::new(),
            rewrite_needed: false,
            next_resource: 0,
        };
        let core = coordinator.lock()?;
        if core.store().crawl_ids()?.len() >= 32 {
            return Err("最多保存 32 个抓站项目".into());
        }
        core.store().save_crawl(&state)?;
        return Ok(vec![state.project]);
    }
    if matches!(request, CrawlRequest::List) {
        return Ok(coordinator
            .lock()?
            .store()
            .crawl_states()?
            .into_iter()
            .map(|s| {
                let mut p = s.project;
                p.resources.clear();
                p
            })
            .collect());
    }
    let id = match &request {
        CrawlRequest::Get { project_id }
        | CrawlRequest::Pause { project_id }
        | CrawlRequest::Resume { project_id }
        | CrawlRequest::Enqueue { project_id, .. }
        | CrawlRequest::Sync { project_id } => project_id,
        _ => unreachable!(),
    };
    let core = coordinator.lock()?;
    let mut state = core.store().crawl_state(id)?;
    let control = Path::new(&state.cache_dir).join("control");
    match request {
        CrawlRequest::Pause { .. } => {
            state.project.status = "paused".into();
            if !state.project.reason.starts_with("达到") {
                state.project.reason = "用户暂停".into();
            }
            if control.exists() {
                std::fs::write(control, "pause").map_err(|e| e.to_string())?;
            }
        }
        CrawlRequest::Resume { .. } => {
            if state.project.status != "paused"
                && state.pending.is_empty()
                && state.project.reason.starts_with("达到")
            {
                return Err("已达到项目边界；请创建范围更窄的项目".into());
            }
            state.project.status = if state.pending.is_empty() {
                "ready"
            } else {
                "discovering"
            }
            .into();
            if !state.project.reason.starts_with("达到") {
                state.project.reason.clear();
            }
            if control.exists() {
                std::fs::write(control, "run").map_err(|e| e.to_string())?;
            }
        }
        CrawlRequest::Enqueue { urls, .. } => {
            let urls: BTreeSet<_> = urls.into_iter().collect();
            if urls
                .iter()
                .any(|url| !state.project.resources.iter().any(|r| &r.url == url))
            {
                return Err("只能下载当前项目发现的资源".into());
            }
            for resource in &mut state.project.resources {
                if urls.is_empty() || urls.contains(&resource.url) {
                    resource.selected = true;
                }
            }
        }
        CrawlRequest::Sync { .. } => {
            for resource in &mut state.project.resources {
                if resource.selected {
                    resource.last_checked_ms = 0;
                    resource.error.clear();
                    if let Some(sync) = state.sync.get_mut(&resource.url) {
                        if sync.pending {
                            let key = format!(
                                "{}:{}:{}",
                                state.project.id,
                                hash(&resource.url),
                                sync.generation
                            );
                            let has_active_task = core.tasks().into_iter().any(|task| {
                                !matches!(
                                    task.status.as_str(),
                                    "completed" | "failed" | "cancelled" | "canceled"
                                ) && core
                                    .task_spec(&task.task_id)
                                    .is_some_and(|s| s.crawl_resource_key == key)
                            });
                            if !has_active_task {
                                sync.pending = false;
                                sync.digest.clear();
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    core.store().save_crawl(&state)?;
    Ok(vec![state.project])
}

pub(crate) fn spawn(coordinator: CoreCoordinator, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            let ids = coordinator
                .lock()
                .and_then(|c| c.store().crawl_ids())
                .unwrap_or_default();
            for id in ids {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) = tick(&coordinator, &id, &stop) {
                    if let Ok(core) = coordinator.lock() {
                        if let Ok(mut latest) = core.store().crawl_state(&id) {
                            latest.project.reason = error;
                            latest.project.status = "paused".into();
                            let _ = core.store().save_crawl(&latest);
                        }
                    }
                }
            }
            for _ in 0..5 {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });
}

fn context(
    coordinator: &CoreCoordinator,
    state: &CrawlState,
    url: &str,
) -> Result<(TaskSpec, crate::net_policy::ThrottleContext), String> {
    let mut spec = coordinator.metadata_spec(url)?;
    spec.queue_id = state.project.spec.queue_id.clone();
    let mut throttle = crate::download_worker::task_throttle_context(
        &coordinator.core(),
        &state.project.id,
        &spec,
    )?;
    throttle.control = Path::new(&state.cache_dir).join("control");
    let core = coordinator.lock()?;
    if core.store().crawl_state(&state.project.id)?.project.status == "paused" {
        return Err("项目已暂停".into());
    }
    std::fs::create_dir_all(&state.cache_dir).map_err(|e| e.to_string())?;
    // Two page workers may initialize this together. Never truncate a pause
    // written by the other worker or by the project command.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&throttle.control)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(b"run").map_err(|e| e.to_string())?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok((spec, throttle))
}

fn tick(coordinator: &CoreCoordinator, id: &str, stop: &AtomicBool) -> Result<(), String> {
    let state = load(coordinator, id)?;
    if state.project.status == "paused" {
        return Ok(());
    }
    let settings = coordinator.settings()?;
    let queue = settings
        .queue_profiles
        .iter()
        .find(|q| q.id == state.project.spec.queue_id)
        .ok_or("项目队列已被移除")?;
    if !crate::net_policy::queue_profile_allowed(queue) {
        return Ok(());
    }
    let active = coordinator
        .raw_tasks()?
        .iter()
        .filter(|t| {
            t.queue_id == queue.id
                && matches!(t.status.as_str(), "downloading" | "checking" | "merging")
        })
        .count();
    let capacity = (queue.max_active as usize)
        .saturating_sub(active)
        .min(state.project.spec.page_concurrency as usize);
    if state.project.status == "discovering" && capacity > 0 {
        let pages: Vec<_> = state
            .pending
            .iter()
            .filter(|p| {
                p.kind != "page" || state.project.visited_pages < state.project.spec.max_pages
            })
            .take(capacity)
            .cloned()
            .collect();
        if pages.is_empty() && !state.pending.is_empty() {
            let core = coordinator.lock()?;
            let mut latest = core.store().crawl_state(id)?;
            if latest.project.visited_pages >= latest.project.spec.max_pages {
                latest.pending.retain(|p| p.kind != "page");
                latest.project.reason = "达到页面数量上限".into();
            }
            if latest.pending.is_empty() && latest.project.status == "discovering" {
                latest.project.status = "ready".into();
            }
            core.store().save_crawl(&latest)?;
        }
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = pages
                .iter()
                .map(|page| {
                    scope.spawn(|| {
                        let (spec, throttle) = context(coordinator, &state, &page.url)?;
                        let origin = state
                            .project
                            .spec
                            .same_origin
                            .then(|| crate::http_engine::http_origin(&state.project.spec.entry_url))
                            .flatten();
                        crate::net_policy::with_throttle_context(Some(throttle), || {
                            coordinator.read_metadata(spec, None, DOCUMENT_LIMIT, origin.as_deref())
                        })
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err("抓站工作线程退出".into())))
                .collect::<Vec<_>>()
        });
        for (page, result) in pages.into_iter().zip(results) {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let response = result?;
            if response.status != 200 {
                return Err(format!(
                    "页面请求返回 HTTP {}，待访问项已保留",
                    response.status
                ));
            }
            let body_len = response.body.len() as u64;
            let (refs, valid_utf8) = match String::from_utf8(response.body) {
                Ok(text) => {
                    let refs = references(&text, &response.final_url, &page.kind);
                    (refs, Some(text))
                }
                Err(non_utf8) => {
                    let lossy = String::from_utf8_lossy(non_utf8.as_bytes());
                    let refs = references(&lossy, &response.final_url, &page.kind);
                    (refs, None)
                }
            };
            let core = coordinator.lock()?;
            let mut latest = core.store().crawl_state(id)?;
            if !latest.pending.iter().any(|p| p.url == page.url) {
                continue;
            }
            latest.metadata_bytes += body_len;
            if latest.metadata_bytes > PROJECT_LIMIT {
                latest.pending.clear();
                latest.project.status = "ready".into();
                latest.project.reason = "达到 32 MiB 页面元数据上限".into();
                core.store().save_crawl(&latest)?;
                break;
            }
            if latest.project.spec.offline {
                let source_cache =
                    Path::new(&latest.cache_dir).join(format!("{}.source", hash(&page.url)));
                if let Some(ref text) = valid_utf8 {
                    atomic_write(&source_cache, text.as_bytes())?;
                } else {
                    let _ = std::fs::remove_file(&source_cache);
                }
            }
            latest.pending.retain(|p| p.url != page.url);
            if page.kind == "page" {
                latest.project.visited_pages += 1;
            }
            let mut url_bytes = state_url_bytes(&latest);
            if url_bytes.saturating_add(page.url.len()) <= MAX_STATE_URL_BYTES
                && add_resource(&mut latest, &page.url, &page.kind)
            {
                url_bytes = url_bytes.saturating_add(page.url.len());
            }
            let mut queued_pages =
                latest.pending.iter().filter(|p| p.kind == "page").count() as u32;
            for reference in refs {
                if reference.kind == "base" || !allowed(&latest.project.spec, &reference.url) {
                    continue;
                }
                let depth = page.depth + u32::from(reference.kind == "page");
                if reference.kind == "page" && depth > latest.project.spec.max_depth {
                    continue;
                }
                if url_bytes.saturating_add(reference.url.len()) <= MAX_STATE_URL_BYTES
                    && add_resource(&mut latest, &reference.url, &reference.kind)
                {
                    url_bytes = url_bytes.saturating_add(reference.url.len());
                }
                if (reference.kind == "page"
                    || latest.project.spec.offline && reference.kind == "style")
                    && !latest.seen.contains(&reference.url)
                {
                    if reference.kind == "page"
                        && latest.project.visited_pages + queued_pages
                            >= latest.project.spec.max_pages
                    {
                        latest.project.reason = "达到页面数量上限".into();
                    } else if latest.project.resources.len() < MAX_RESOURCES
                        && latest.seen.len() < MAX_SEEN
                        && url_bytes.saturating_add(reference.url.len().saturating_mul(2))
                            <= MAX_STATE_URL_BYTES
                    {
                        url_bytes = url_bytes.saturating_add(reference.url.len().saturating_mul(2));
                        latest.seen.insert(reference.url.clone());
                        if reference.kind == "page" {
                            queued_pages += 1;
                        }
                        latest.pending.push_back(Page {
                            url: reference.url,
                            depth,
                            kind: reference.kind,
                        });
                    }
                }
            }
            if latest.project.resources.len() >= MAX_RESOURCES {
                latest.pending.clear();
                latest.project.status = "ready".into();
                latest.project.reason = "达到 2000 个资源上限".into();
            }
            if latest.pending.is_empty() && latest.project.status == "discovering" {
                latest.project.status = "ready".into();
            }
            core.store().save_crawl(&latest)?;
        }
    }
    let latest = load(coordinator, id)?;
    if latest.project.status == "paused" || stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let resources = &latest.project.resources;
    for offset in 0..resources.len().min(8) {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let resource = &resources[(latest.next_resource + offset) % resources.len()];
        if resource.selected {
            sync_resource(coordinator, id, &resource.url)?;
        }
    }
    if resources.len() > 8 && resources.iter().any(|r| r.selected) {
        edit_state(coordinator, id, |s| {
            s.next_resource = (s.next_resource + 8) % s.project.resources.len().max(1);
            Ok(())
        })?;
    }
    if latest.project.spec.offline && load(coordinator, id)?.rewrite_needed {
        rewrite_offline(coordinator, id)?;
    }
    Ok(())
}

fn state_url_bytes(state: &CrawlState) -> usize {
    state
        .project
        .resources
        .iter()
        .map(|r| r.url.len())
        .sum::<usize>()
        .saturating_add(state.seen.iter().map(String::len).sum::<usize>())
        .saturating_add(state.pending.iter().map(|p| p.url.len()).sum::<usize>())
        .saturating_add(state.sync.keys().map(String::len).sum::<usize>())
}

fn add_resource(state: &mut CrawlState, url: &str, kind: &str) -> bool {
    if state.project.resources.len() >= MAX_RESOURCES
        || !resource_allowed(&state.project.spec, url, kind)
        || kind == "page" && !state.project.spec.offline
        || state.project.resources.iter().any(|r| r.url == url)
    {
        return false;
    }
    state.project.resources.push(CrawlResource {
        url: url.into(),
        kind: kind.into(),
        local_name: local_name(url, kind),
        selected: false,
        task_id: None,
        saved: false,
        etag: String::new(),
        last_modified: String::new(),
        last_checked_ms: 0,
        error: String::new(),
    });
    true
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(path.parent().ok_or("文件路径无效")?).map_err(|e| e.to_string())?;
    let temp = path.with_extension("writing");
    use std::io::Write;
    let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    crate::atomic_replace::replace_file(&temp, path).map_err(|e| e.to_string())
}

fn sync_resource(coordinator: &CoreCoordinator, id: &str, url: &str) -> Result<(), String> {
    let state = load(coordinator, id)?;
    let index = state
        .project
        .resources
        .iter()
        .position(|r| r.url == url)
        .ok_or("资源不存在")?;
    let resource = state.project.resources[index].clone();
    if !resource.error.is_empty() && now().saturating_sub(resource.last_checked_ms) < 60_000 {
        return Ok(());
    }
    let sync = state.sync.get(url).cloned().unwrap_or_default();
    let target = Path::new(&state.project.spec.output_dir).join(&resource.local_name);
    if sync.pending {
        // The key is persisted before task admission. A restart between task
        // commit and receipt recording finds that same ordinary task again.
        let key = format!("{id}:{}:{}", hash(url), sync.generation);
        let task = {
            let core = coordinator.lock()?;
            core.tasks().into_iter().find(|task| {
                core.task_spec(&task.task_id)
                    .is_some_and(|s| s.crawl_resource_key == key)
            })
        };
        if let Some(task) = task {
            if task.status == "completed" {
                let source = Path::new(&task.output_path);
                if !source.is_file() {
                    if !sync.digest.is_empty()
                        && target.is_file()
                        && crate::checksum::hash_file(&target, crate::checksum::Algorithm::Sha256)?
                            == sync.digest
                    {
                        return finish_sync(coordinator, id, url);
                    }
                    return resource_error(coordinator, id, url, "下载成品已移动，旧文件保留");
                }
                if sync
                    .size
                    .is_some_and(|size| source.metadata().map(|m| m.len()).ok() != Some(size))
                {
                    return resource_error(coordinator, id, url, "同步长度不符，旧文件保留");
                }
                let digest =
                    crate::checksum::hash_file(source, crate::checksum::Algorithm::Sha256)?;
                if state.project.spec.offline && matches!(resource.kind.as_str(), "page" | "style")
                {
                    if source.metadata().map_err(|e| e.to_string())?.len() > DOCUMENT_LIMIT {
                        return resource_error(
                            coordinator,
                            id,
                            url,
                            "离线文档超过 1 MiB 上限，旧文件保留",
                        );
                    }
                    let raw = std::fs::read(source).map_err(|e| e.to_string())?;
                    let source_cache =
                        Path::new(&state.cache_dir).join(format!("{}.source", hash(url)));
                    if std::str::from_utf8(&raw).is_ok() {
                        atomic_write(&source_cache, &raw)?;
                    } else {
                        let _ = std::fs::remove_file(&source_cache);
                    }
                }
                // Store the digest before rename so a crash after publication can
                // distinguish the new complete file from the previous version.
                edit_state(coordinator, id, |state| {
                    state.project.resources[index].task_id = Some(task.task_id.clone());
                    state.sync.get_mut(url).ok_or("同步状态丢失")?.digest = digest;
                    Ok(())
                })?;
                // FlushFileBuffers on Windows requires a writable handle.
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(source)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
                std::fs::create_dir_all(target.parent().ok_or("目标路径无效")?)
                    .map_err(|e| e.to_string())?;
                if source != target && crate::atomic_replace::replace_file(source, &target).is_err()
                {
                    crate::output_path::stage_copy_and_replace(source, &target)?;
                    let _ = std::fs::remove_file(source);
                }
                finish_sync(coordinator, id, url)?;
            } else if matches!(task.status.as_str(), "failed" | "cancelled" | "canceled") {
                return resource_error(
                    coordinator,
                    id,
                    url,
                    "同步下载失败；旧文件保留，可在任务列表重试",
                );
            } else {
                update_resource(coordinator, id, url, |r| r.task_id = Some(task.task_id))?;
            }
            return Ok(());
        }
        if !sync.digest.is_empty()
            && target.is_file()
            && crate::checksum::hash_file(&target, crate::checksum::Algorithm::Sha256)?
                == sync.digest
        {
            return finish_sync(coordinator, id, url);
        }
        let mut spec = coordinator.metadata_spec(url)?;
        spec.queue_id = state.project.spec.queue_id.clone();
        spec.download_dir = Path::new(&state.project.spec.output_dir)
            .join(".updates")
            .join(&hash(url)[..32])
            .to_string_lossy()
            .into_owned();
        spec.work_dir.clear();
        spec.filename = resource.local_name;
        spec.allow_duplicate = true;
        spec.crawl_resource_key = key;
        spec.origin_boundary = if state.project.spec.same_origin {
            crate::http_engine::http_origin(&state.project.spec.entry_url).unwrap_or_default()
        } else {
            String::new()
        };
        spec.expected_size = sync.size;
        spec.etag = sync.etag;
        spec.last_modified = sync.modified;
        let events = coordinator.dispatch(CoreCommand::CreateTask { spec })?;
        if events
            .iter()
            .any(|e| matches!(e.event, CoreEvent::Error { .. }))
        {
            return resource_error(coordinator, id, url, "无法创建同步下载，旧文件保留");
        }
        return Ok(());
    }
    let interval = state.project.spec.sync_interval_secs.saturating_mul(1000);
    if resource.saved
        && resource.last_checked_ms != 0
        && (interval == 0 || now().saturating_sub(resource.last_checked_ms) < interval)
    {
        return Ok(());
    }
    let (mut spec, throttle) = context(coordinator, &state, url)?;
    spec.request_method = "HEAD".into();
    if resource.saved {
        if !resource.etag.is_empty() {
            spec.headers
                .insert("If-None-Match".into(), resource.etag.clone());
        } else if !resource.last_modified.is_empty() {
            spec.headers
                .insert("If-Modified-Since".into(), resource.last_modified.clone());
        }
    }
    let origin = state
        .project
        .spec
        .same_origin
        .then(|| crate::http_engine::http_origin(&state.project.spec.entry_url))
        .flatten();
    let response = crate::net_policy::with_throttle_context(Some(throttle), || {
        coordinator.read_metadata(spec, None, 0, origin.as_deref())
    });
    let response = match response {
        Ok(r) => r,
        Err(_) => return resource_error(coordinator, id, url, "同步检查失败，旧文件保留"),
    };
    if response.status == 304
        || resource.saved
            && response.status == 200
            && ((!resource.etag.is_empty() && resource.etag == response.etag)
                || resource.etag.is_empty()
                    && !resource.last_modified.is_empty()
                    && resource.last_modified == response.last_modified)
    {
        update_resource(coordinator, id, url, |r| {
            r.last_checked_ms = now();
            r.error.clear();
        })?;
        return Ok(());
    }
    if response.status != 200 {
        return resource_error(
            coordinator,
            id,
            url,
            &format!("同步检查 HTTP {}，旧文件保留", response.status),
        );
    }
    if resource.saved && response.etag.is_empty() && response.last_modified.is_empty() {
        return resource_error(
            coordinator,
            id,
            url,
            "远程无 ETag/Last-Modified，无法可靠判定变化；旧文件保留",
        );
    }
    let core = coordinator.lock()?;
    let mut latest = core.store().crawl_state(id)?;
    let record = latest.sync.entry(url.into()).or_default();
    record.generation += 1;
    record.pending = true;
    record.etag = response.etag;
    record.modified = response.last_modified;
    record.size = response.content_length;
    record.digest.clear();
    record.final_url = response.final_url;
    core.store().save_crawl(&latest)?;
    Ok(())
}

fn update_resource(
    coordinator: &CoreCoordinator,
    id: &str,
    url: &str,
    edit: impl FnOnce(&mut CrawlResource),
) -> Result<(), String> {
    edit_state(coordinator, id, |state| {
        edit(
            state
                .project
                .resources
                .iter_mut()
                .find(|r| r.url == url)
                .ok_or("资源不存在")?,
        );
        Ok(())
    })
}
fn edit_state(
    coordinator: &CoreCoordinator,
    id: &str,
    edit: impl FnOnce(&mut CrawlState) -> Result<(), String>,
) -> Result<(), String> {
    let core = coordinator.lock()?;
    let mut state = core.store().crawl_state(id)?;
    edit(&mut state)?;
    core.store().save_crawl(&state)
}
fn resource_error(
    coordinator: &CoreCoordinator,
    id: &str,
    url: &str,
    error: &str,
) -> Result<(), String> {
    edit_state(coordinator, id, |state| {
        if let Some(sync) = state.sync.get_mut(url) {
            sync.pending = false;
            sync.digest.clear();
        }
        let resource = state
            .project
            .resources
            .iter_mut()
            .find(|r| r.url == url)
            .ok_or("资源不存在")?;
        resource.error = error.into();
        resource.last_checked_ms = now();
        Ok(())
    })
}
fn finish_sync(coordinator: &CoreCoordinator, id: &str, url: &str) -> Result<(), String> {
    let mut core = coordinator.lock()?;
    let mut state = core.store().crawl_state(id)?;
    let sync = state.sync.get_mut(url).ok_or("同步状态丢失")?;
    sync.pending = false;
    let resource = state
        .project
        .resources
        .iter_mut()
        .find(|r| r.url == url)
        .ok_or("资源不存在")?;
    resource.saved = true;
    resource.etag = sync.etag.clone();
    resource.last_modified = sync.modified.clone();
    resource.last_checked_ms = now();
    resource.error.clear();
    state.rewrite_needed = true;
    let task_id = resource.task_id.clone();
    let output = Path::new(&state.project.spec.output_dir)
        .join(&resource.local_name)
        .to_string_lossy()
        .into_owned();
    if let Some(task_id) = task_id {
        core.set_output_path(&task_id, output)?;
    }
    core.store().save_crawl(&state)?;
    Ok(())
}

struct Reference {
    start: usize,
    end: usize,
    url: String,
    fragment: String,
    kind: String,
}
struct RawReference {
    start: usize,
    end: usize,
    navigation: bool,
    base: bool,
}
fn classify(url: &str, navigation: bool) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "css" {
        "style"
    } else if navigation
        && (ext.is_empty()
            || matches!(
                ext.as_str(),
                "html" | "htm" | "php" | "asp" | "aspx" | "jsp"
            ))
    {
        "page"
    } else {
        "file"
    }
    .into()
}
fn css_spans(text: &str) -> Vec<RawReference> {
    let lower = text.to_ascii_lowercase();
    let mut cursor = 0;
    let mut spans = Vec::new();
    while let Some(pos) = lower[cursor..].find("url(") {
        let start = cursor + pos + 4;
        let Some(end) = text[start..].find(')').map(|n| start + n) else {
            break;
        };
        let value = text[start..end].trim().trim_matches(['\'', '"']);
        if let Some(rel) = text[start..end].find(value) {
            spans.push(RawReference {
                start: start + rel,
                end: start + rel + value.len(),
                navigation: false,
                base: false,
            });
        }
        cursor = end + 1;
        if spans.len() >= MAX_RESOURCES {
            break;
        }
    }
    let mut cursor = 0;
    while let Some(pos) = lower[cursor..].find("@import") {
        let start = cursor + pos + 7;
        let tail = text[start..].trim_start();
        if let Some(quote) = tail.chars().next().filter(|c| matches!(c, '\'' | '"')) {
            let value_start = text.len() - tail.len() + 1;
            if let Some(end) = text[value_start..].find(quote).map(|n| value_start + n) {
                spans.push(RawReference {
                    start: value_start,
                    end,
                    navigation: false,
                    base: false,
                });
                cursor = end + 1;
            } else {
                cursor = value_start;
            }
        } else {
            cursor = start;
        }
        if cursor >= text.len() || spans.len() >= MAX_RESOURCES {
            break;
        }
    }
    let mut comments = Vec::new();
    let mut cursor = 0;
    while let Some(start) = lower[cursor..].find("/*").map(|n| cursor + n) {
        let end = lower[start + 2..]
            .find("*/")
            .map(|n| start + 4 + n)
            .unwrap_or(lower.len());
        comments.push((start, end));
        cursor = end;
    }
    spans.retain(|s| !comments.iter().any(|&(a, b)| a <= s.start && s.start < b));
    spans.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end)));
    let mut non_overlapping: Vec<RawReference> = Vec::with_capacity(spans.len());
    let mut last_end = 0usize;
    for span in spans {
        if span.start >= last_end {
            last_end = span.end;
            non_overlapping.push(span);
        }
    }
    non_overlapping
}
fn references(text: &str, base: &str, kind: &str) -> Vec<Reference> {
    let mut spans = Vec::new();
    if kind == "style" {
        spans = css_spans(text);
    } else {
        let bytes = text.as_bytes();
        let lower = text.to_ascii_lowercase();
        let mut at = 0;
        while at < bytes.len() && spans.len() < MAX_RESOURCES {
            if bytes[at] != b'<' {
                at += 1;
                continue;
            }
            if text[at..].starts_with("<!--") {
                at = text[at + 4..]
                    .find("-->")
                    .map(|n| at + 7 + n)
                    .unwrap_or(text.len());
                continue;
            }
            at += 1;
            let closing = bytes.get(at) == Some(&b'/');
            if closing {
                at += 1;
            }
            let tag_start = at;
            while at < bytes.len() && bytes[at].is_ascii_alphanumeric() {
                at += 1;
            }
            let tag = lower[tag_start..at].to_string();
            while at < bytes.len() && bytes[at] != b'>' {
                if !bytes[at].is_ascii_alphabetic() {
                    at += 1;
                    continue;
                }
                let start = at;
                while at < bytes.len()
                    && (bytes[at].is_ascii_alphanumeric() || matches!(bytes[at], b'-' | b':'))
                {
                    at += 1;
                }
                let name = lower[start..at].to_string();
                while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                    at += 1;
                }
                if bytes.get(at) != Some(&b'=') {
                    continue;
                }
                at += 1;
                while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                    at += 1;
                }
                let quote = bytes.get(at).copied().filter(|c| matches!(c, b'\'' | b'"'));
                if quote.is_some() {
                    at += 1;
                }
                let start = at;
                while at < bytes.len()
                    && if let Some(q) = quote {
                        bytes[at] != q
                    } else {
                        !bytes[at].is_ascii_whitespace() && bytes[at] != b'>'
                    }
                {
                    at += 1;
                }
                if matches!(name.as_str(), "href" | "src" | "poster") {
                    spans.push(RawReference {
                        start,
                        end: at,
                        navigation: name == "href",
                        base: tag == "base" && name == "href",
                    });
                } else if name == "style" {
                    for mut span in css_spans(&text[start..at]) {
                        span.start += start;
                        span.end += start;
                        spans.push(span);
                    }
                } else if name == "srcset" {
                    let mut offset = start;
                    for part in text[start..at].split(',') {
                        let raw = part.trim_start();
                        let value = raw.split_whitespace().next().unwrap_or("");
                        let begin = offset + part.len() - raw.len();
                        spans.push(RawReference {
                            start: begin,
                            end: begin + value.len(),
                            navigation: false,
                            base: false,
                        });
                        offset += part.len() + 1;
                    }
                }
                if quote.is_some() && at < bytes.len() {
                    at += 1;
                }
            }
            if !closing && matches!(tag.as_str(), "script" | "style") && at < bytes.len() {
                let start = at + 1;
                let end = lower[start..]
                    .find(&format!("</{tag}"))
                    .map(|n| start + n)
                    .unwrap_or(text.len());
                if tag == "style" {
                    for mut span in css_spans(&text[start..end]) {
                        span.start += start;
                        span.end += start;
                        spans.push(span);
                    }
                }
                at = end;
            } else if at < bytes.len() {
                at += 1;
            }
        }
    }
    let effective_base = spans
        .iter()
        .find(|r| r.base)
        .and_then(|r| crate::harvest::resolve(base, &text[r.start..r.end]))
        .unwrap_or_else(|| base.into());
    spans
        .into_iter()
        .filter_map(|span| {
            let resolved = crate::harvest::resolve(
                if span.base { base } else { &effective_base },
                &text[span.start..span.end],
            )?;
            let fragment = resolved
                .find('#')
                .map(|at| resolved[at..].to_string())
                .unwrap_or_default();
            let url = canonical(&resolved)?;
            let kind = if span.base {
                "base".into()
            } else {
                classify(&url, span.navigation)
            };
            Some(Reference {
                start: span.start,
                end: span.end,
                url,
                fragment,
                kind,
            })
        })
        .collect()
}

fn rewrite_offline(coordinator: &CoreCoordinator, id: &str) -> Result<(), String> {
    let state = load(coordinator, id)?;
    let saved: BTreeMap<_, _> = state
        .project
        .resources
        .iter()
        .filter(|r| {
            r.saved
                && Path::new(&state.project.spec.output_dir)
                    .join(&r.local_name)
                    .is_file()
        })
        .map(|r| (r.url.clone(), r.local_name.clone()))
        .collect();
    for resource in state
        .project
        .resources
        .iter()
        .filter(|r| r.saved && matches!(r.kind.as_str(), "page" | "style"))
    {
        let raw = Path::new(&state.cache_dir).join(format!("{}.source", hash(&resource.url)));
        if !raw.is_file() {
            continue;
        }
        let raw_bytes = std::fs::read(&raw).map_err(|e| e.to_string())?;
        let Ok(mut text) = String::from_utf8(raw_bytes) else {
            continue;
        };
        let base = state
            .sync
            .get(&resource.url)
            .map(|s| s.final_url.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&resource.url);
        let mut refs = references(&text, base, &resource.kind);
        refs.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
        let has_base = refs.iter().any(|r| r.kind == "base");
        let mut changed = false;
        let mut last_start = usize::MAX;
        for reference in refs.into_iter().rev() {
            if reference.start > reference.end
                || reference.end > last_start
                || reference.end > text.len()
                || !text.is_char_boundary(reference.start)
                || !text.is_char_boundary(reference.end)
            {
                continue;
            }
            if reference.kind == "base" {
                text.replace_range(reference.start..reference.end, "./");
                last_start = reference.start;
                changed = true;
            } else if let Some(local) = saved.get(&reference.url) {
                text.replace_range(
                    reference.start..reference.end,
                    &format!("{local}{}", reference.fragment),
                );
                last_start = reference.start;
                changed = true;
            } else if has_base {
                // Removing <base> must not turn an unsaved remote reference into
                // a broken relative local path.
                let absolute = format!("{}{}", reference.url, reference.fragment)
                    .replace('&', "&amp;")
                    .replace('"', "&quot;")
                    .replace('\'', "&#39;");
                text.replace_range(reference.start..reference.end, &absolute);
                last_start = reference.start;
                changed = true;
            }
        }
        let target = Path::new(&state.project.spec.output_dir).join(&resource.local_name);
        if changed && std::fs::read(&target).ok().as_deref() != Some(text.as_bytes()) {
            atomic_write(&target, text.as_bytes())?;
        }
    }
    edit_state(coordinator, id, |s| {
        s.rewrite_needed = false;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_import_with_url_text_produces_disjoint_replacements() {
        let css = r#"@import "dir/url(inner).css"; body { background: url(bg.png) }"#;
        let spans: Vec<_> = css_spans(css)
            .into_iter()
            .map(|s| &css[s.start..s.end])
            .collect();
        assert_eq!(spans, ["dir/url(inner).css", "bg.png"]);
    }

    #[test]
    fn canonical_removes_loops_without_decoding_path_separators() {
        assert_eq!(
            canonical("http://EXAMPLE.test/a/%2e%2e/b#one"),
            canonical("http://example.test/b")
        );
        assert_ne!(
            canonical("http://example.test/a%2fb"),
            canonical("http://example.test/a/b")
        );
        assert!(canonical("http://example.test/%中文").is_some());
        assert!(canonical("javascript:alert(1)").is_none());
        assert_eq!(
            canonical("http://example.test/?q=&amp;"),
            Some("http://example.test:80/?q=&amp;".into())
        );
    }

    #[test]
    fn static_references_respect_base_and_skip_script_comments() {
        let text = r#"<!-- <a href="/comment"> --><base href="/assets/">
            <script>const x='<a href="/script">';</script>
            <style>/*url('/comment.png')*/ @import 'theme.css'; .x{background:url("bg.png")}</style>
            <a href="../next#fragment">next</a><img srcset="one.png 1x, two.png 2x" style="background:url(three.png)">"#;
        let refs = references(text, "http://example.test/index.html", "page");
        let urls: Vec<_> = refs.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            refs.iter()
                .find(|r| r.url.ends_with("/next"))
                .unwrap()
                .fragment,
            "#fragment"
        );
        assert!(!urls
            .iter()
            .any(|s| s.contains("comment") || s.contains("script")));
        for path in [
            "/assets/theme.css",
            "/assets/bg.png",
            "/next",
            "/assets/one.png",
            "/assets/two.png",
            "/assets/three.png",
        ] {
            assert!(
                urls.contains(&format!("http://example.test:80{path}").as_str()),
                "{urls:?}"
            );
        }
        for r in refs {
            assert!(text.get(r.start..r.end).is_some());
        }
    }

    #[test]
    fn sync_resource_succeeds_with_empty_temp_dir_and_nested_update_download_dir() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server_stop = Arc::new(AtomicBool::new(false));
        let server_stop_thread = Arc::clone(&server_stop);
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let page = b"<html><body><a href=\"/asset.bin\">asset</a></body></html>";
            let payload = b"crawl-sync-payload";
            while !server_stop_thread.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first_line = String::new();
                if reader.read_line(&mut first_line).unwrap_or(0) == 0 {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("GET");
                let path = parts.next().unwrap_or("/");
                if path == "/index.html" {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        page.len()
                    );
                    if method != "HEAD" {
                        let _ = stream.write_all(page);
                    }
                } else if path == "/asset.bin" {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nETag: \"crawl-v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    if method != "HEAD" {
                        let _ = stream.write_all(payload);
                    }
                } else {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
            }
        });

        let root = std::env::temp_dir().join(format!(
            "hls-v7-crawl-workdir-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let coordinator = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
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
            .set_setting("temp_dir", serde_json::json!(""))
            .unwrap();
        coordinator
            .set_setting("proxy_mode", serde_json::json!("direct"))
            .unwrap();

        let created = dispatch(
            &coordinator,
            CrawlRequest::Create {
                spec: CrawlSpec {
                    entry_url: format!("http://{addr}/index.html"),
                    output_dir: String::new(),
                    max_depth: 1,
                    max_pages: 5,
                    page_concurrency: 1,
                    same_origin: true,
                    ..Default::default()
                },
            },
        )
        .unwrap();
        let project_id = created[0].id.clone();
        let stop = AtomicBool::new(false);

        // Tick 1: discover /index.html -> finds /asset.bin.
        tick(&coordinator, &project_id, &stop).unwrap();
        let project = dispatch(
            &coordinator,
            CrawlRequest::Enqueue {
                project_id: project_id.clone(),
                urls: Vec::new(),
            },
        )
        .unwrap()
        .pop()
        .unwrap();
        assert_eq!(project.resources.len(), 1);
        assert!(project.resources[0].selected);

        // Tick 2: HEAD /asset.bin -> marks sync.pending = true.
        tick(&coordinator, &project_id, &stop).unwrap();
        // Tick 3: sync_resource dispatches CoreCommand::CreateTask with nested .updates/<hash>
        // download_dir and cleared work_dir (must not fail work_dir boundary check).
        tick(&coordinator, &project_id, &stop).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut saved = false;
        while std::time::Instant::now() < deadline {
            tick(&coordinator, &project_id, &stop).unwrap();
            let current = load(&coordinator, &project_id).unwrap();
            let resource = &current.project.resources[0];
            assert!(
                resource.error.is_empty(),
                "sync_resource must not fail boundary validation: {}",
                resource.error
            );
            if resource.saved {
                let target = Path::new(&current.project.spec.output_dir).join(&resource.local_name);
                assert_eq!(std::fs::read(&target).unwrap(), b"crawl-sync-payload");
                saved = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(saved, "crawl resource should complete and publish cleanly");

        server_stop.store(true, Ordering::Release);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn max_pages_cap_transitions_to_ready_and_still_downloads_selected_resources() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let server_stop = Arc::new(AtomicBool::new(false));
        let stop_flag = server_stop.clone();
        let server = std::thread::spawn(move || {
            while !stop_flag.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 2048];
                        let n = stream.read(&mut buf).unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]);
                        let first = req.lines().next().unwrap_or("");
                        let head = first.starts_with("HEAD ");
                        let path = first.split_whitespace().nth(1).unwrap_or("/");
                        let (ct, body, etag) = match path {
                            "/index.html" => (
                                "text/html; charset=utf-8",
                                b"<a href=\"/p2.html\">2</a><a href=\"/p3.html\">3</a><a href=\"/file.bin\">dl</a>".as_slice(),
                                None,
                            ),
                            "/p2.html" => ("text/html; charset=utf-8", b"<p>page 2</p>".as_slice(), None),
                            "/p3.html" => ("text/html; charset=utf-8", b"<p>page 3</p>".as_slice(), None),
                            "/file.bin" => ("application/octet-stream", b"cap-ok".as_slice(), Some("\"v1\"")),
                            _ => ("text/plain", b"missing".as_slice(), None),
                        };
                        let status = if body == b"missing" {
                            "404 Not Found"
                        } else {
                            "200 OK"
                        };
                        let etag_hdr = etag.map(|e| format!("ETag: {e}\r\n")).unwrap_or_default();
                        let hdr = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n{etag_hdr}Connection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(hdr.as_bytes());
                        if !head {
                            let _ = stream.write_all(body);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        let root = std::env::temp_dir().join(format!("hls-v7-crawl-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let coordinator = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
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
            .set_setting("temp_dir", serde_json::json!(""))
            .unwrap();
        coordinator
            .set_setting("proxy_mode", serde_json::json!("direct"))
            .unwrap();
        let project = dispatch(
            &coordinator,
            CrawlRequest::Create {
                spec: CrawlSpec {
                    entry_url: format!("http://{addr}/index.html"),
                    output_dir: String::new(),
                    max_depth: 2,
                    max_pages: 2,
                    page_concurrency: 1,
                    same_origin: true,
                    offline: false,
                    extensions: vec!["bin".into()],
                    queue_id: crate::DEFAULT_QUEUE_ID.into(),
                    ..Default::default()
                },
            },
        )
        .unwrap()
        .pop()
        .unwrap();
        let project_id = project.id;
        let stop = AtomicBool::new(false);

        // Tick 1 visits /index.html (1 page), queues /p2.html (reaching max_pages=2), and skips /p3.html without pausing!
        tick(&coordinator, &project_id, &stop).unwrap();
        // Tick 2 visits /p2.html (visited_pages=2) and transitions cleanly to "ready" with boundary reason preserved.
        tick(&coordinator, &project_id, &stop).unwrap();
        let after_discovery = load(&coordinator, &project_id).unwrap().project;
        assert_eq!(after_discovery.visited_pages, 2);
        assert_eq!(after_discovery.status, "ready");
        assert_eq!(after_discovery.reason, "达到页面数量上限");

        // Enqueue discovered resource after hitting max_pages: must still sync & download!
        dispatch(
            &coordinator,
            CrawlRequest::Enqueue {
                project_id: project_id.clone(),
                urls: Vec::new(),
            },
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut saved = false;
        while std::time::Instant::now() < deadline {
            tick(&coordinator, &project_id, &stop).unwrap();
            let current = load(&coordinator, &project_id).unwrap();
            if current.project.resources[0].saved {
                saved = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        assert!(
            saved,
            "cap-completed crawl project must still download enqueued resources"
        );

        // User Pause -> Resume must also recover to "ready" without erroring on boundary reason.
        dispatch(
            &coordinator,
            CrawlRequest::Pause {
                project_id: project_id.clone(),
            },
        )
        .unwrap();
        let resumed = dispatch(
            &coordinator,
            CrawlRequest::Resume {
                project_id: project_id.clone(),
            },
        )
        .unwrap()
        .pop()
        .unwrap();
        assert_eq!(resumed.status, "ready");
        // Calling Resume again when already "ready" at boundary still informs user cleanly.
        assert!(dispatch(&coordinator, CrawlRequest::Resume { project_id }).is_err());

        server_stop.store(true, Ordering::Release);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn offline_crawl_handles_non_utf8_documents_and_unwedges_failed_sync_tasks() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let server_stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&server_stop);
        let gbk_css: &'static [u8] = b"/* \xC4\xE3\xBA\xC3 GBK */ body { color: #123456; }";
        let asset_v1: &'static [u8] = b"asset-generation-1";
        let asset_v2: &'static [u8] = b"asset-generation-2-ok";
        let use_v2 = Arc::new(AtomicBool::new(false));
        let use_v2_server = Arc::clone(&use_v2);

        let server = std::thread::spawn(move || {
            let page = b"<html><head><link rel=\"stylesheet\" href=\"/gbk.css\"></head><body><a href=\"/flaky.bin\">dl</a></body></html>";
            while !stop_flag.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first_line = String::new();
                if reader.read_line(&mut first_line).unwrap_or(0) == 0 {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("GET");
                let path = parts.next().unwrap_or("/");
                let (ct, etag, body): (&str, &str, &[u8]) = match path {
                    "/index.html" => ("text/html", "\"idx-1\"", page.as_slice()),
                    "/gbk.css" => ("text/css; charset=gbk", "\"gbk-1\"", gbk_css),
                    "/flaky.bin" => {
                        if use_v2_server.load(Ordering::Acquire) {
                            ("application/octet-stream", "\"flaky-2\"", asset_v2)
                        } else {
                            ("application/octet-stream", "\"flaky-1\"", asset_v1)
                        }
                    }
                    _ => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        continue;
                    }
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nETag: {etag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if method != "HEAD" {
                    let _ = stream.write_all(body);
                }
            }
        });

        let root = std::env::temp_dir().join(format!(
            "hls-v7-crawl-g7-6-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let coordinator = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
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
            .set_setting("temp_dir", serde_json::json!(""))
            .unwrap();
        coordinator
            .set_setting("proxy_mode", serde_json::json!("direct"))
            .unwrap();

        let project = dispatch(
            &coordinator,
            CrawlRequest::Create {
                spec: CrawlSpec {
                    entry_url: format!("http://{addr}/index.html"),
                    output_dir: String::new(),
                    max_depth: 1,
                    max_pages: 5,
                    page_concurrency: 1,
                    same_origin: true,
                    offline: true,
                    queue_id: crate::DEFAULT_QUEUE_ID.into(),
                    ..Default::default()
                },
            },
        )
        .unwrap()
        .pop()
        .unwrap();
        let project_id = project.id;
        let stop = AtomicBool::new(false);

        // Discover /index.html and /gbk.css without pausing on non-UTF-8 CSS bytes.
        tick(&coordinator, &project_id, &stop).unwrap();
        tick(&coordinator, &project_id, &stop).unwrap();
        let discovered = load(&coordinator, &project_id).unwrap();
        assert_eq!(discovered.project.status, "ready");
        assert_eq!(discovered.project.resources.len(), 3);

        let index_url = discovered
            .project
            .resources
            .iter()
            .find(|r| r.url.ends_with("/index.html"))
            .unwrap()
            .url
            .clone();
        let css_url = discovered
            .project
            .resources
            .iter()
            .find(|r| r.url.ends_with("/gbk.css"))
            .unwrap()
            .url
            .clone();
        let flaky_url = discovered
            .project
            .resources
            .iter()
            .find(|r| r.url.ends_with("/flaky.bin"))
            .unwrap()
            .url
            .clone();

        // Select /index.html and /gbk.css first: non-UTF-8 CSS must sync and save verbatim without aborting!
        dispatch(
            &coordinator,
            CrawlRequest::Enqueue {
                project_id: project_id.clone(),
                urls: vec![index_url.clone(), css_url.clone()],
            },
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            tick(&coordinator, &project_id, &stop).unwrap();
            let cur = load(&coordinator, &project_id).unwrap();
            assert_ne!(
                cur.project.status, "paused",
                "reason: {}",
                cur.project.reason
            );
            let idx_res = cur
                .project
                .resources
                .iter()
                .find(|r| r.url == index_url)
                .unwrap();
            let css_res = cur
                .project
                .resources
                .iter()
                .find(|r| r.url == css_url)
                .unwrap();
            if idx_res.saved && css_res.saved && !cur.rewrite_needed {
                let css_target = Path::new(&cur.project.spec.output_dir).join(&css_res.local_name);
                assert_eq!(std::fs::read(&css_target).unwrap(), gbk_css);
                let idx_target = Path::new(&cur.project.spec.output_dir).join(&idx_res.local_name);
                let rewritten_html = std::fs::read_to_string(&idx_target).unwrap();
                assert!(rewritten_html.contains(&css_res.local_name));
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        let after_css = load(&coordinator, &project_id).unwrap();
        assert!(
            after_css
                .project
                .resources
                .iter()
                .find(|r| r.url == css_url)
                .unwrap()
                .saved
        );

        // Now enqueue /flaky.bin, let generation-1 task complete on disk, then move its staged file
        // BEFORE sync_resource harvests it so line 627 validation fails.
        dispatch(
            &coordinator,
            CrawlRequest::Enqueue {
                project_id: project_id.clone(),
                urls: vec![flaky_url.clone()],
            },
        )
        .unwrap();
        // HEAD check -> pending=true, generation=1
        sync_resource(&coordinator, &project_id, &flaky_url).unwrap();
        // CreateTask for generation=1
        sync_resource(&coordinator, &project_id, &flaky_url).unwrap();
        let gen1_key = format!("{project_id}:{}:1", hash(&flaky_url));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut gen1_output = String::new();
        while std::time::Instant::now() < deadline {
            let maybe_done = {
                let core = coordinator.lock().unwrap();
                core.tasks().into_iter().find(|t| {
                    t.status == "completed"
                        && core
                            .task_spec(&t.task_id)
                            .is_some_and(|s| s.crawl_resource_key == gen1_key)
                })
            };
            if let Some(done) = maybe_done {
                gen1_output = done.output_path;
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(!gen1_output.is_empty(), "gen1 sync task did not complete");
        std::fs::remove_file(&gen1_output).unwrap();

        // Harvesting now fails validation and MUST clear sync.pending so future syncs aren't wedged!
        sync_resource(&coordinator, &project_id, &flaky_url).unwrap();
        let failed_state = load(&coordinator, &project_id).unwrap();
        assert!(!failed_state.sync[&flaky_url].pending);
        assert!(failed_state
            .project
            .resources
            .iter()
            .find(|r| r.url == flaky_url)
            .unwrap()
            .error
            .contains("下载成品已移动"));

        // User clicks Sync after server updates to v2: must advance to generation 2 and succeed!
        use_v2.store(true, Ordering::Release);
        dispatch(
            &coordinator,
            CrawlRequest::Sync {
                project_id: project_id.clone(),
            },
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut recovered = false;
        while std::time::Instant::now() < deadline {
            tick(&coordinator, &project_id, &stop).unwrap();
            let cur = load(&coordinator, &project_id).unwrap();
            let res = cur
                .project
                .resources
                .iter()
                .find(|r| r.url == flaky_url)
                .unwrap();
            if res.saved && res.etag == "\"flaky-2\"" {
                assert_eq!(cur.sync[&flaky_url].generation, 2);
                let target = Path::new(&cur.project.spec.output_dir).join(&res.local_name);
                assert_eq!(std::fs::read(&target).unwrap(), asset_v2);
                recovered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        assert!(recovered, "failed sync task stayed wedged after user Sync");

        server_stop.store(true, Ordering::Release);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn crawl_seen_set_is_bounded_by_max_pages_and_max_seen_under_sqlite_limit() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let server_stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&server_stop);

        let mut index_html = String::from("<html><body>");
        for i in 0..1500 {
            index_html.push_str(&format!(
                "<a href=\"/p{i}.html?pad={}\">{i}</a>",
                "x".repeat(120)
            ));
        }
        index_html.push_str("</body></html>");
        let index_bytes = index_html.into_bytes();

        let server = std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            while !stop_flag.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut first_line = String::new();
                        if reader.read_line(&mut first_line).unwrap_or(0) == 0 {
                            continue;
                        }
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                                break;
                            }
                        }
                        let path = first_line.split_whitespace().nth(1).unwrap_or("/");
                        let body: &[u8] = if path == "/index.html" {
                            &index_bytes
                        } else {
                            b"<html><body>leaf</body></html>"
                        };
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(hdr.as_bytes());
                        let _ = stream.write_all(body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        let root = std::env::temp_dir().join(format!(
            "hls-v7-crawl-seen-cap-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let coordinator = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
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
            .set_setting("temp_dir", serde_json::json!(""))
            .unwrap();
        coordinator
            .set_setting("proxy_mode", serde_json::json!("direct"))
            .unwrap();

        let project = dispatch(
            &coordinator,
            CrawlRequest::Create {
                spec: CrawlSpec {
                    entry_url: format!("http://{addr}/index.html"),
                    output_dir: String::new(),
                    max_depth: 2,
                    max_pages: 2,
                    page_concurrency: 1,
                    same_origin: true,
                    offline: false,
                    queue_id: crate::DEFAULT_QUEUE_ID.into(),
                    ..Default::default()
                },
            },
        )
        .unwrap()
        .pop()
        .unwrap();
        let project_id = project.id;
        let stop = AtomicBool::new(false);

        tick(&coordinator, &project_id, &stop).unwrap();
        tick(&coordinator, &project_id, &stop).unwrap();
        let state = load(&coordinator, &project_id).unwrap();
        assert_eq!(state.project.status, "ready");
        assert_eq!(state.project.visited_pages, 2);
        assert_eq!(state.project.reason, "达到页面数量上限");
        assert_eq!(
            state.seen.len(),
            2,
            "unvisited links past max_pages must not bloat state.seen"
        );
        assert!(state.seen.len() <= MAX_SEEN);
        let serialized = serde_json::to_string(&state).unwrap();
        assert!(serialized.len() < 3 * 1024 * 1024);

        server_stop.store(true, Ordering::Release);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(&root);
    }
}
