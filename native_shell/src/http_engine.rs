//! Range GET into one `payload.downloading`.
//!
//! Loopback/http uses the small TCP client; Windows https uses WinHTTP.
//! Browser identity requests use a bounded curl-impersonate response stream.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const WRITE_BATCH: usize = 256 * 1024;
const FILE_WRITE_BUFFER: usize = 1024 * 1024;
const DURABLE_CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;
// 持久检查点（payload sync_data + 记录 sync_all + 原子替换）代价高：实测 32 MiB fsync 约 0.65 s。
// 达到 DURABLE_CHECKPOINT_BYTES 后，同一任务内全部 worker 合计至多每 DURABLE_CHECKPOINT_INTERVAL 提交一次；
// 未同步量超过 DURABLE_CHECKPOINT_MAX_BYTES 时无条件提交，边播边下时保持按字节提交。
// 只影响“何时登记”，不改变“先 flush/sync 再登记”的顺序，崩溃后最多重下未登记的部分。
const DURABLE_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(1);
const DURABLE_CHECKPOINT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RANGE_ATTEMPTS: u32 = 5;
const MAX_RANGE_RETRY_AFTER: Duration = Duration::from_secs(300);
const ADAPTIVE_INITIAL_CONNECTIONS: usize = 2;
const ADAPTIVE_RAMP_SUCCESSES: usize = 2;
const ADAPTIVE_RAMP_STEP: usize = 2;
const WAF_CURL_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const WAF_CURL_CACHE_MAX_ENTRIES: usize = 256;
// The connection pool is used only by the Windows WinHTTP transport (and tests).
#[cfg_attr(not(windows), allow(dead_code))]
const CONNECT_POOL_PER_KEY: usize = 8;
#[cfg_attr(not(windows), allow(dead_code))]
const CONNECT_POOL_TOTAL: usize = 32;

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
struct IdleHandlePool {
    idle: HashMap<String, Vec<usize>>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl IdleHandlePool {
    fn take(&mut self, key: &str) -> Option<usize> {
        let bucket = self.idle.get_mut(key)?;
        let handle = bucket.pop();
        if bucket.is_empty() {
            self.idle.remove(key);
        }
        handle
    }

    fn put(&mut self, key: String, handle: usize) -> bool {
        if handle == 0 {
            return false;
        }
        let total: usize = self.idle.values().map(Vec::len).sum();
        if total >= CONNECT_POOL_TOTAL {
            return false;
        }
        let bucket = self.idle.entry(key).or_default();
        if bucket.len() >= CONNECT_POOL_PER_KEY || bucket.contains(&handle) {
            return false;
        }
        bucket.push(handle);
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyRoute<'a> {
    Direct,
    System,
    Manual(&'a str),
}

pub(crate) fn proxy_route(proxy: &str) -> ProxyRoute<'_> {
    let proxy = proxy.trim();
    if proxy.is_empty() || proxy == crate::net_policy::DIRECT_PROXY_SENTINEL {
        ProxyRoute::Direct
    } else if proxy == crate::net_policy::SYSTEM_PROXY_SENTINEL {
        ProxyRoute::System
    } else {
        ProxyRoute::Manual(proxy)
    }
}

// WinHTTP named proxies do not accept URL userinfo or SOCKS routes.
fn proxy_needs_curl(proxy: &str) -> bool {
    proxy
        .trim()
        .split_once("://")
        .is_some_and(|(scheme, rest)| {
            scheme.to_ascii_lowercase().starts_with("socks")
                || rest
                    .split(['/', '?', '#'])
                    .next()
                    .is_some_and(|a| a.contains('@'))
        })
}

#[cfg(test)]
#[test]
fn proxy_route_identity_distinguishes_transports() {
    assert_eq!(proxy_route(""), ProxyRoute::Direct);
    assert_eq!(
        proxy_route(crate::net_policy::DIRECT_PROXY_SENTINEL),
        ProxyRoute::Direct
    );
    assert_eq!(
        proxy_route(crate::net_policy::SYSTEM_PROXY_SENTINEL),
        ProxyRoute::System
    );
    assert_eq!(
        proxy_route("http://127.0.0.1:8080"),
        ProxyRoute::Manual("http://127.0.0.1:8080")
    );
}

#[cfg_attr(not(windows), allow(dead_code))]
fn origin_connect_key(proxy: &str, host: &str, port: u16) -> String {
    format!("{proxy}|{host}|{port}")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub output: PathBuf,
    #[serde(default = "default_connections")]
    pub connections: usize,
    #[serde(default = "default_chunk_bytes")]
    pub chunk_bytes: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub sequential: bool,
    #[serde(default)]
    pub resume_from: u64,
    #[serde(default)]
    pub proxy: String,
    /// Signature-free identity of the original resource URL.  This is kept
    /// separate from `url` because signed query parameters can rotate while
    /// the underlying object remains the same.
    #[serde(default)]
    pub resource_key: String,
    /// A strong ETag or Last-Modified validator captured by the v6 probe.
    /// It is sent as If-Range and persisted with the native range checkpoint.
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub last_modified: String,
    pub control: PathBuf,
    pub progress: PathBuf,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub body_path: PathBuf,
    #[serde(default)]
    pub mirrors: Vec<String>,
    /// In-memory replay JSON. Never persist this on TaskSpec; it is applied
    /// per request URL so CDN hops do not keep the page Cookie.
    #[serde(default)]
    pub replay_json: String,
    /// 已知媒体范围的浏览器传输正文上限；按解码后的临时文件大小检查。
    /// 普通文件留空，保留服务器不支持 Range 时的整包回退。
    #[serde(default)]
    pub curl_body_limit: Option<u64>,
    #[serde(default)]
    pub origin_boundary: String,
    /// 同一次范围下载的停止信号，仅在内存中传给底层传输。
    #[serde(skip)]
    pub(crate) installer_download: bool,
    #[serde(skip)]
    pub(crate) range_stop: Option<Arc<AtomicBool>>,
}

fn default_connections() -> usize {
    4
}

fn default_chunk_bytes() -> u64 {
    8 * 1024 * 1024
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Run,
    Pause,
    Cancel,
}

#[derive(Debug)]
pub enum EngineError {
    Pause,
    Cancel,
    RangeUnsupported(String),
    Failed(String),
}

impl From<String> for EngineError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpMirrorReport {
    pub url: String,
    pub final_url: String,
    pub state: String,
    pub detail: String,
    pub ranges: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRunReport {
    pub used_url: String,
    pub mirrors: Vec<HttpMirrorReport>,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pause => write!(formatter, "paused"),
            Self::Cancel => write!(formatter, "canceled"),
            Self::RangeUnsupported(message) => write!(formatter, "{message}"),
            Self::Failed(message) => write!(formatter, "{message}"),
        }
    }
}

pub fn read_control(path: &Path) -> Control {
    // 已注册的运行任务直接读进程内令牌；控制文件每秒至多重读一次（见 task_control）。
    match crate::task_control::current(path) {
        Some(crate::task_control::PAUSE) => return Control::Pause,
        Some(crate::task_control::CANCEL) => return Control::Cancel,
        Some(_) => return Control::Run,
        None => {}
    }
    let text = fs::read_to_string(path).unwrap_or_default();
    match text.trim().to_ascii_lowercase().as_str() {
        "pause" => Control::Pause,
        "cancel" => Control::Cancel,
        _ => Control::Run,
    }
}

pub fn write_progress(path: &Path, downloaded: u64, total: u64, speed: f64, status: &str) {
    let payload = serde_json::json!({
        "downloaded": downloaded,
        "total": total,
        "speed": speed,
        "status": status,
    });
    // 运行任务内的进度文件至多每秒落盘一次，进程内读者使用内存副本；终态立即落盘。
    crate::task_control::publish_progress_file(path, &payload.to_string(), status != "downloading");
}

pub(crate) fn write_media_progress(
    path: &Path,
    downloaded: u64,
    completed: u64,
    total: u64,
    live: bool,
) {
    let payload = serde_json::json!({
        "downloaded": downloaded, "total": 0, "speed": 0,
        "completed_segments": completed, "total_segments": if live { 0 } else { total },
        "status": if live { "recording" } else { "downloading" },
    });
    let finished = !live && total > 0 && completed >= total;
    crate::task_control::publish_progress_file(path, &payload.to_string(), finished);
}

pub(crate) fn write_media_merging(path: &Path) {
    let Some(text) = crate::task_control::read_progress_file(path) else {
        return;
    };
    let Ok(mut payload) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    payload["status"] = "merging".into();
    payload["speed"] = 0.into();
    crate::task_control::publish_progress_file(path, &payload.to_string(), true);
}

// 分片间隙和恢复下载的已有字节不能变成零速率闪烁或瞬时尖峰。
pub(crate) struct TransferRate {
    samples: std::collections::VecDeque<(Instant, u64)>,
}

impl TransferRate {
    pub(crate) fn new(bytes: u64) -> Self {
        Self {
            samples: std::collections::VecDeque::from([(Instant::now(), bytes)]),
        }
    }

    pub(crate) fn sample(&mut self, bytes: u64) -> u64 {
        let now = Instant::now();
        if self
            .samples
            .back()
            .is_some_and(|(_, previous)| bytes < *previous)
        {
            self.samples.clear();
            self.samples.push_back((now, bytes));
        }
        while self.samples.len() > 1
            && now.duration_since(self.samples[1].0) >= Duration::from_secs(3)
        {
            self.samples.pop_front();
        }
        let (at, baseline) = self.samples[0];
        let speed = (bytes.saturating_sub(baseline) as f64
            / now.duration_since(at).as_secs_f64().max(0.5)) as u64;
        self.samples.push_back((now, bytes));
        speed
    }
}

fn completed_ranges_path(job: &Job) -> PathBuf {
    job.progress.with_file_name("native-engine.ranges.json")
}

fn active_ranges_path(progress: &Path) -> PathBuf {
    progress.with_file_name("native-engine.active.json")
}

fn write_active_ranges(progress: &Path, ranges: &[(u64, u64)]) {
    let path = active_ranges_path(progress);
    let payload = serde_json::json!({
        "ranges": ranges.iter().map(|(start, end)| serde_json::json!([start, end])).collect::<Vec<_>>(),
    });
    crate::task_control::publish_progress_file(&path, &payload.to_string(), false);
}

fn remove_active_ranges(progress: &Path) {
    let path = active_ranges_path(progress);
    crate::task_control::forget_progress_file(&path);
    let _ = fs::remove_file(path);
}

fn strong_etag(value: &str) -> String {
    let text = value.trim();
    if text.is_empty() || text.to_ascii_lowercase().starts_with("w/") {
        String::new()
    } else {
        text.to_string()
    }
}

fn is_ephemeral_signature_query_param(raw_key: &str) -> bool {
    let lower = raw_key.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "token"
            | "sig"
            | "signature"
            | "sign"
            | "upsig"
            | "expires"
            | "expire"
            | "deadline"
            | "e"
            | "ts"
            | "timestamp"
            | "t"
            | "hdnts"
            | "hdnea"
            | "wssecret"
            | "wstime"
            | "auth_key"
            | "key"
            | "policy"
            | "ossaccesskeyid"
            | "googleaccessid"
            | "trid"
            | "mid"
            | "gen"
            | "os"
            | "og"
            | "platform"
            | "orderid"
            | "logo"
            | "sv"
            | "se"
            | "sp"
            | "sr"
            | "st"
            | "spr"
            | "sdd"
    ) || lower.starts_with("x-amz-")
        || lower.starts_with("x-oss-")
        || lower.starts_with("x-cos-")
        || lower.starts_with("x-goog-")
}

/// Normalizes an HTTP(S) resource URL into a stable checkpoint identity key by
/// canonicalizing scheme/host/default-port and stripping ephemeral presigned
/// query parameters while retaining resource-identifying path and query params.
pub(crate) fn canonical_url_key(url: &str) -> String {
    let without_fragment = url.trim().split('#').next().unwrap_or("").trim();
    let Some((raw_scheme, rest)) = without_fragment.split_once("://") else {
        return without_fragment.to_string();
    };
    let scheme = raw_scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return without_fragment.to_string();
    }
    let default_port: u16 = if scheme == "https" { 443 } else { 80 };
    let (authority, tail) = match rest.find(['/', '?']) {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, endpoint)| endpoint)
        .trim();
    if host_port.is_empty() {
        return without_fragment.to_string();
    }
    let (host, port) = if let Some(after_bracket) = host_port.strip_prefix('[') {
        match after_bracket.split_once(']') {
            Some((ipv6, remainder)) => {
                let parsed_port = remainder
                    .strip_prefix(':')
                    .and_then(|value| value.parse::<u16>().ok());
                (format!("[{}]", ipv6.to_ascii_lowercase()), parsed_port)
            }
            None => (host_port.to_ascii_lowercase(), None),
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((name, port_str)) if !name.contains(':') => match port_str.parse::<u16>() {
                Ok(parsed) => (name.to_ascii_lowercase(), Some(parsed)),
                Err(_) => (host_port.to_ascii_lowercase(), None),
            },
            _ => (host_port.to_ascii_lowercase(), None),
        }
    };
    let (raw_path, raw_query) = match tail.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (tail, None),
    };
    let path = if raw_path.is_empty() { "/" } else { raw_path };
    let mut out = format!("{scheme}://{host}");
    if let Some(port) = port.filter(|value| *value != default_port) {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push_str(path);
    if let Some(query) = raw_query {
        let kept: Vec<&str> = query
            .split('&')
            .filter(|segment| !segment.is_empty())
            .filter(|segment| {
                let key = segment.split('=').next().unwrap_or(segment);
                !is_ephemeral_signature_query_param(key)
            })
            .collect();
        if !kept.is_empty() {
            out.push('?');
            out.push_str(&kept.join("&"));
        }
    }
    out
}

fn checkpoint_matches(job: &Job, value: &serde_json::Value) -> bool {
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(2) {
        return false;
    }
    let Some(total) = value.get("total").and_then(serde_json::Value::as_u64) else {
        return false;
    };
    if total == 0 || total != job.total {
        return false;
    }
    let fallback_key;
    let expected_key = if job.resource_key.trim().is_empty() {
        fallback_key = canonical_url_key(&job.url);
        fallback_key.trim()
    } else {
        job.resource_key.trim()
    };
    let saved_key = value
        .get("resource_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    if expected_key.is_empty()
        || saved_key.is_empty()
        || (saved_key != expected_key && canonical_url_key(saved_key) != expected_key)
    {
        return false;
    }
    let expected_etag = strong_etag(&job.etag);
    let saved_etag = strong_etag(
        value
            .get("etag")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    );
    if !expected_etag.is_empty() && !saved_etag.is_empty() && expected_etag != saved_etag {
        return false;
    }
    let expected_modified = job.last_modified.trim();
    let saved_modified = value
        .get("last_modified")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    if !expected_modified.is_empty()
        && !saved_modified.is_empty()
        && expected_modified != saved_modified
    {
        return false;
    }
    true
}

fn range_covered(completed: &[(u64, u64)], start: u64, end: u64) -> bool {
    completed
        .iter()
        .any(|(done_start, done_end)| *done_start <= start && *done_end >= end)
}

fn normalize_ranges(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.retain(|(start, end)| end >= start);
    ranges.sort_unstable_by_key(|(start, _)| *start);
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        if let Some((_, previous_end)) = merged.last_mut() {
            if start <= previous_end.saturating_add(1) {
                *previous_end = (*previous_end).max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn covered_bytes(ranges: &[(u64, u64)]) -> u64 {
    ranges
        .iter()
        .map(|(start, end)| end.saturating_sub(*start).saturating_add(1))
        .sum()
}

fn subtract_ranges(base_start: u64, base_end: u64, completed: &[(u64, u64)]) -> Vec<WorkRange> {
    let mut cursor = base_start;
    let mut uncovered = Vec::new();
    for (done_start, done_end) in completed {
        if *done_end < cursor {
            continue;
        }
        if *done_start > base_end {
            break;
        }
        if *done_start > cursor {
            uncovered.push(WorkRange {
                start: cursor,
                end: done_start.saturating_sub(1).min(base_end),
            });
        }
        cursor = cursor.max(done_end.saturating_add(1));
        if cursor > base_end {
            break;
        }
    }
    if cursor <= base_end {
        uncovered.push(WorkRange {
            start: cursor,
            end: base_end,
        });
    }
    uncovered
}

fn load_completed_ranges(job: &Job) -> Option<Vec<(u64, u64)>> {
    let text = fs::read_to_string(completed_ranges_path(job)).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    completed_ranges_from_checkpoint(job, &value)
}

/// Restore local progress after a crash. This does not authorize HTTP reuse:
/// run_job still probes the server and checks its current resource identity.
pub(crate) fn checkpoint_progress(job: &Job) -> Option<(u64, u64)> {
    let text = fs::read_to_string(completed_ranges_path(job)).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let mut saved = job.clone();
    if saved.total == 0 {
        saved.total = value.get("total")?.as_u64()?;
    }
    if saved.etag.trim().is_empty() && saved.last_modified.trim().is_empty() {
        saved.etag = value
            .get("etag")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into();
        saved.last_modified = value
            .get("last_modified")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into();
    }
    let ranges = completed_ranges_from_checkpoint(&saved, &value)?;
    Some((covered_bytes(&ranges), saved.total))
}

fn completed_ranges_from_checkpoint(
    job: &Job,
    value: &serde_json::Value,
) -> Option<Vec<(u64, u64)>> {
    if !checkpoint_matches(job, value) {
        return None;
    }
    // 必须在 set_len 预分配之前校验；补长后的零字节不能充当已下载数据。
    let output_len = fs::metadata(&job.output).ok()?.len();
    let items = value.get("ranges").and_then(|item| item.as_array())?;
    let ranges: Vec<(u64, u64)> = items
        .iter()
        .filter_map(|item| {
            let pair = item.as_array()?;
            Some((pair.first()?.as_u64()?, pair.get(1)?.as_u64()?))
        })
        .filter(|(start, end)| end >= start)
        .collect();
    if job.total == 0
        || ranges
            .iter()
            .any(|(start, end)| *start >= job.total || *end >= job.total || *end >= output_len)
    {
        return None;
    }
    Some(normalize_ranges(ranges))
}

fn save_completed_ranges(job: &Job, ranges: &[(u64, u64)]) -> Result<(), EngineErrorCode> {
    let payload = serde_json::json!({
        "version": 2,
        "resource_key": job.resource_key,
        "etag": job.etag,
        "last_modified": job.last_modified,
        "total": job.total,
        "ranges": ranges.iter().map(|(start, end)| serde_json::json!([start, end])).collect::<Vec<_>>(),
    });
    let path = completed_ranges_path(job);
    let tmp = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint open failed: {err}")))?;
    let text = payload.to_string();
    file.write_all(text.as_bytes())
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint write failed: {err}")))?;
    file.flush()
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint flush failed: {err}")))?;
    file.sync_all()
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint sync failed: {err}")))?;
    drop(file);
    crate::atomic_replace::replace_file(&tmp, &path)
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint replace failed: {err}")))?;
    // 进程内读者（Core 进度绘制、边播边下）读内存副本，不再每次打开解析检查点文件。
    crate::range_cache::record_written(&path, &text);
    Ok(())
}

struct RangeCheckpoints {
    durable: Mutex<Vec<(u64, u64)>>,
    pending: Mutex<Vec<(u64, u64)>>,
    // 最近一次真正同步并写出记录的时间，所有 worker 共用，用于限制提交频率。
    last_commit: Mutex<Instant>,
}

impl RangeCheckpoints {
    fn new(durable: Vec<(u64, u64)>) -> Self {
        Self {
            durable: Mutex::new(durable),
            pending: Mutex::new(Vec::new()),
            last_commit: Mutex::new(Instant::now()),
        }
    }

    fn since_last_commit(&self) -> Duration {
        self.last_commit
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .elapsed()
    }

    fn mark_committed(&self) {
        *self
            .last_commit
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Instant::now();
    }
}

/// 传输中的周期检查点是否应提交。`playback` 仅在字节门槛已满足、时间门槛未满足时才求值。
fn durable_checkpoint_due(
    unsynced: u64,
    since_last_commit: Duration,
    playback: impl FnOnce() -> bool,
) -> bool {
    if unsynced < DURABLE_CHECKPOINT_BYTES {
        return false;
    }
    unsynced >= DURABLE_CHECKPOINT_MAX_BYTES
        || since_last_commit >= DURABLE_CHECKPOINT_INTERVAL
        || playback()
}

const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";

pub fn apply_browser_profile(headers: &mut HashMap<String, String>) {
    if !headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("user-agent"))
        || headers
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("user-agent") && value.trim().is_empty())
    {
        headers.retain(|key, _| !key.eq_ignore_ascii_case("user-agent"));
        headers.insert("User-Agent".into(), CHROME_UA.into());
    }
    insert_default_header(headers, "Accept", "*/*");
    insert_default_header(headers, "Accept-Language", "en-US,en;q=0.9");
}

fn insert_default_header(headers: &mut HashMap<String, String>, name: &str, value: &str) {
    if !headers.keys().any(|key| key.eq_ignore_ascii_case(name)) {
        headers.insert(name.to_string(), value.to_string());
    }
}

pub fn strip_stale_cloudflare_cookies(
    headers: &HashMap<String, String>,
) -> Option<HashMap<String, String>> {
    let cookie_key = headers
        .keys()
        .find(|key| key.eq_ignore_ascii_case("cookie"))?
        .clone();
    let original = headers.get(&cookie_key)?;
    let mut values = Vec::new();
    let mut changed = false;
    for item in original.split(';') {
        let name = item
            .split('=')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        // __cflb 是源站会话亲和 Cookie，不能当成过期风控凭据删除。
        if name == "__cf_bm" {
            changed = true;
            continue;
        }
        if !item.trim().is_empty() {
            values.push(item.trim().to_string());
        }
    }
    if !changed {
        return None;
    }
    let mut next = headers.clone();
    if values.is_empty() {
        next.remove(&cookie_key);
    } else {
        next.insert(cookie_key, values.join("; "));
    }
    Some(next)
}

fn request_user_agent(headers: &HashMap<String, String>) -> String {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("user-agent"))
        .map(|(_, value)| value.clone())
        .filter(|value| !value.trim().is_empty() && header_is_wire_safe("User-Agent", value))
        .unwrap_or_else(|| CHROME_UA.to_string())
}

fn curl_impersonate_exe() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("HLS_V7_CURL_IMPERSONATE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return curl_impersonate_near(dir);
        }
    }
    None
}

/// The curl executable for one request. Tests route specific URL prefixes to a
/// fake curl so the pipe/throttle/control paths run without the bundled tool.
#[cfg_attr(not(test), allow(unused_variables))]
fn curl_exe_for(url: &str) -> Option<PathBuf> {
    #[cfg(test)]
    {
        let overrides = TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((_, exe)) = overrides.iter().find(|(prefix, _)| url.starts_with(prefix)) {
            return Some(exe.clone());
        }
    }
    curl_impersonate_exe()
}

#[cfg(test)]
static TEST_CURL_OVERRIDES: std::sync::Mutex<Vec<(String, PathBuf)>> =
    std::sync::Mutex::new(Vec::new());

fn curl_impersonate_near(dir: &Path) -> Option<PathBuf> {
    [
        dir.join("tools")
            .join("curl-impersonate")
            .join("curl-impersonate.exe"),
        dir.join("curl-impersonate.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceProbe {
    pub total: Option<u64>,
    pub accept_ranges: bool,
    pub etag: String,
    pub last_modified: String,
}

pub fn probe_resource(job: &Job) -> Result<ResourceProbe, EngineError> {
    probe_resource_response(job).map(|(probe, _)| probe)
}

fn probe_resource_response(job: &Job) -> Result<(ResourceProbe, FetchResult), EngineError> {
    check_job_control(job)?;
    let fetched = fetch(job, Some("bytes=0-0"))?;
    if fetched.status == 416
        && fetched.content_range.as_deref().is_some_and(|header| {
            let mut parts = header.split_whitespace();
            parts
                .next()
                .is_some_and(|unit| unit.eq_ignore_ascii_case("bytes"))
                && parts.next() == Some("*/0")
                && parts.next().is_none()
        })
    {
        return Ok((
            ResourceProbe {
                total: Some(0),
                accept_ranges: false,
                etag: fetched.etag.clone(),
                last_modified: fetched.last_modified.clone(),
            },
            fetched,
        ));
    }
    if fetched.status != 200 && fetched.status != 206 {
        return Err(EngineError::Failed(format!("HTTP {}", fetched.status)));
    }
    let total = if fetched.content_encoding {
        None
    } else if fetched.status == 206 {
        fetched
            .content_range
            .as_deref()
            .and_then(parse_content_range)
            .and_then(|(_, _, total)| total)
    } else {
        fetched.content_length
    };
    let accept_ranges = fetched.status == 206 && total.is_some() && !fetched.content_encoding;
    Ok((
        ResourceProbe {
            total,
            accept_ranges,
            etag: fetched.etag.clone(),
            last_modified: fetched.last_modified.clone(),
        },
        fetched,
    ))
}

pub fn run_job(job: &Job) -> Result<(), EngineError> {
    run_job_report(job).map(|_| ())
}

pub fn run_job_report(job: &Job) -> Result<HttpRunReport, EngineError> {
    let _network = set_media_network_scope(Some(entry_network_scope(&job.url, &job.proxy)));
    let normalized_mirrors = crate::mirrors::normalize_mirror_urls(&job.url, &job.mirrors);
    let mut reports: Vec<HttpMirrorReport> = normalized_mirrors
        .iter()
        .map(|url| HttpMirrorReport {
            url: url.clone(),
            final_url: String::new(),
            state: "pending".into(),
            detail: String::new(),
            ranges: false,
        })
        .collect();
    let mut urls = vec![job.url.clone()];
    urls.extend(normalized_mirrors);
    let mut last = EngineError::Failed("job url missing".into());
    let probe_allowed = request_method(job) == "GET";
    let mut identity: Option<(Option<u64>, String)> = None;
    if job.total > 0 || !job.etag.trim().is_empty() {
        identity = Some(((job.total > 0).then_some(job.total), job.etag.clone()));
    }
    for (index, url) in urls.into_iter().enumerate() {
        let mut attempt = job.clone();
        attempt.url = url;
        if range_validator(job).is_empty() {
            // A newly probed validator cannot identify a prefix saved without one.
            attempt.resume_from = 0;
        }
        if index > 0 {
            let replay = job_replay_json(job);
            drop_cross_origin_secrets(&mut attempt.headers, &replay, &job.url, &attempt.url);
            attempt.replay_json = replay;
        }
        let mut initial_response = None;
        let mut session_accept_ranges = false;
        if probe_allowed
            && !attempt.sequential
            && (attempt.total == 0
                || index > 0
                || (strong_etag(&attempt.etag).is_empty()
                    && attempt.last_modified.trim().is_empty()))
        {
            match probe_resource_response(&attempt) {
                Ok((probe, response)) => {
                    if let Some((len, etag)) = &identity {
                        if !crate::mirrors::mirror_identity_compatible(
                            *len,
                            etag,
                            probe.total,
                            &probe.etag,
                        ) {
                            last = EngineError::Failed("mirror identity mismatch".into());
                            if index > 0 {
                                let report = &mut reports[index - 1];
                                report.state = "skipped".into();
                                report.detail = "资源身份不一致".into();
                            }
                            continue;
                        }
                    } else {
                        identity = Some((probe.total, probe.etag.clone()));
                    }
                    if attempt.total == 0 {
                        if let Some(total) = probe.total {
                            attempt.total = total;
                        }
                    }
                    if strong_etag(&attempt.etag).is_empty() {
                        attempt.etag = probe.etag;
                    }
                    if attempt.last_modified.is_empty() {
                        attempt.last_modified = probe.last_modified;
                    }
                    if !probe.accept_ranges || attempt.total == 0 {
                        attempt.sequential = true;
                    } else if response.accept_ranges {
                        session_accept_ranges = true;
                    }
                    // A 200 already owns the complete entity. Do not consume a
                    // one-use URL again merely because its server ignores Range.
                    if response.status == 200
                        && attempt.resume_from == 0
                        && !response.content_encoding
                    {
                        initial_response = Some(response);
                    }
                }
                Err(error) => {
                    if index > 0 {
                        let report = &mut reports[index - 1];
                        report.state = "failed".into();
                        report.detail = error.to_string();
                    }
                    last = error;
                    continue;
                }
            }
        }
        match run_job_once(&attempt, initial_response, session_accept_ranges) {
            Ok(()) => {
                if index > 0 {
                    let report = &mut reports[index - 1];
                    report.final_url = attempt.url.clone();
                    report.state = "active".into();
                    report.detail = "下载已使用此地址".into();
                    report.ranges = !attempt.sequential;
                }
                reports
                    .iter_mut()
                    .filter(|item| item.state == "pending")
                    .for_each(|item| {
                        item.state = "skipped".into();
                        item.detail = "已有可用地址，未请求".into();
                    });
                return Ok(HttpRunReport {
                    used_url: attempt.url,
                    mirrors: reports,
                });
            }
            Err(error) => {
                if index > 0 {
                    let report = &mut reports[index - 1];
                    report.state = "failed".into();
                    report.detail = error.to_string();
                    report.ranges = !attempt.sequential;
                }
                last = error;
            }
        }
    }
    Err(last)
}

fn run_job_once(
    job: &Job,
    initial_response: Option<FetchResult>,
    session_accept_ranges: bool,
) -> Result<(), EngineError> {
    if job.url.trim().is_empty() {
        return Err(EngineError::Failed("job url missing".into()));
    }
    if let Some(parent) = job.output.parent() {
        fs::create_dir_all(parent).map_err(|err| EngineError::Failed(err.to_string()))?;
    }
    match read_control(&job.control) {
        Control::Pause => return Err(EngineError::Pause),
        Control::Cancel => return Err(EngineError::Cancel),
        Control::Run => {}
    }
    let _slot = crate::net_policy::acquire(&job.url, || check_job_control(job))?;
    // A HEAD job must take the sequential path even when it carries a total: a
    // HEAD range response is 206 + Content-Range with no body, so the range
    // workers read zero bytes forever and the job dies as "made no progress
    // after 5 attempts" although nothing was ever going to arrive.  Read the
    // method through `request_method` so `" Head"` is routed exactly like the
    // wire request, matching the exemption inside `download_sequential`.
    if request_method(job).eq_ignore_ascii_case("POST")
        || request_method(job).eq_ignore_ascii_case("HEAD")
        || job.sequential
        || job.total == 0
    {
        let _ = crate::range_cache::remove(&completed_ranges_path(job));
        remove_active_ranges(&job.progress);
        download_sequential(job, initial_response)
    } else {
        download_ranges(job, session_accept_ranges)
    }
}

pub(crate) fn fetch_bytes_limited(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    limit: u64,
) -> Result<(u16, Vec<u8>), EngineError> {
    fetch_manifest_bytes(url, headers, proxy, false, limit).map(|(status, body, _)| (status, body))
}

#[cfg(test)]
pub fn fetch_hls_bytes(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
) -> Result<(u16, Vec<u8>), EngineError> {
    fetch_bytes_with_transport(url, headers, proxy, true, None)
        .map(|(status, body, _)| (status, body))
}

pub(crate) fn fetch_hls_bytes_limited(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    limit: u64,
) -> Result<(u16, Vec<u8>), EngineError> {
    fetch_manifest_bytes(url, headers, proxy, true, limit).map(|(status, body, _)| (status, body))
}

pub(crate) fn fetch_manifest_bytes(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    browser_first: bool,
    limit: u64,
) -> Result<(u16, Vec<u8>, String), EngineError> {
    fetch_bytes_with_transport(url, headers, proxy, browser_first, Some(limit))
}

fn fetch_bytes_with_transport(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    browser_first: bool,
    limit: Option<u64>,
) -> Result<(u16, Vec<u8>, String), EngineError> {
    if !http_fetch_url_allowed(url) {
        return Err(EngineError::Failed("url must be http(s)".into()));
    }
    // 子请求继承下载任务的控制文件；无任务上下文时不创建临时目录或控制文件（curl 路径按需懒创建）。
    let control = crate::net_policy::current_throttle_context()
        .map(|context| context.control)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_default();
    let job = Job {
        url: url.to_string(),
        headers: headers.clone(),
        output: PathBuf::new(),
        connections: 1,
        chunk_bytes: 64 * 1024,
        total: 0,
        sequential: true,
        resume_from: 0,
        proxy: proxy.to_string(),
        resource_key: url.to_string(),
        etag: String::new(),
        last_modified: String::new(),
        control,
        progress: PathBuf::new(),
        method: "GET".into(),
        body_path: PathBuf::new(),
        mirrors: Vec::new(),
        replay_json: String::new(),
        curl_body_limit: limit,
        range_stop: None,
        installer_download: false,
        origin_boundary: String::new(),
    };
    let fetched = if browser_first {
        fetch_browser_first(&job, None)?
    } else {
        fetch(&job, None)?
    };
    let status = fetched.status;
    if !(200..300).contains(&status) && status != 304 {
        check_job_control(&job)?;
        return Ok((status, Vec::new(), fetched.final_url));
    }
    let expected_length = if matches!(status, 200 | 206) {
        full_entity_length(&fetched)?
    } else {
        fetched.content_length
    };
    if status != 304
        && limit.is_some_and(|limit| expected_length.is_some_and(|length| length > limit))
    {
        return Err(EngineError::Failed(
            "metadata response exceeds byte limit".into(),
        ));
    }
    let mut body = Vec::new();
    let mut reader = fetched
        .body
        .take(limit.map_or(u64::MAX, |limit| limit.saturating_add(1)));
    let mut buffer = [0; 64 * 1024];
    let mut last_control = Instant::now();
    loop {
        if last_control.elapsed() >= Duration::from_millis(50) {
            check_job_control(&job)?;
            last_control = Instant::now();
        }
        let count = reader.read(&mut buffer).map_err(|error| {
            check_job_control(&job)
                .err()
                .unwrap_or_else(|| EngineError::Failed(error.to_string()))
        })?;
        if count == 0 {
            break;
        }
        if limit.is_some_and(|limit| body.len() as u64 + count as u64 > limit) {
            return Err(EngineError::Failed(
                "metadata response exceeds byte limit".into(),
            ));
        }
        crate::net_policy::consume(count, || check_job_control(&job))?;
        body.extend_from_slice(&buffer[..count]);
    }
    if matches!(status, 200 | 206)
        && expected_length.is_some_and(|expected| expected != body.len() as u64)
    {
        return Err(EngineError::Failed(format!(
            "incomplete response body: received {} bytes, expected {}",
            body.len(),
            expected_length.unwrap()
        )));
    }
    check_job_control(&job)?;
    Ok((status, body, fetched.final_url))
}

/// Distinguish retryable transport failures from terminal disk/decryption errors.
#[derive(Debug)]
pub(crate) enum MediaTransferError {
    Transfer(EngineError),
    Output(std::io::Error),
}

impl From<EngineError> for MediaTransferError {
    fn from(error: EngineError) -> Self {
        Self::Transfer(error)
    }
}

impl From<std::io::Error> for MediaTransferError {
    fn from(error: std::io::Error) -> Self {
        Self::Output(error)
    }
}

impl std::fmt::Display for MediaTransferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transfer(error) => error.fmt(formatter),
            Self::Output(error) => error.fmt(formatter),
        }
    }
}

/// Copies one HLS entity through the existing credential/transport path.
/// Output is provisional until length, decryption and durability checks succeed.
pub(crate) fn copy_hls_body(
    job: &Job,
    range: Option<(u64, u64)>,
    writer: &mut impl Write,
) -> Result<(), MediaTransferError> {
    check_job_control(job)?;
    let fetched = if let Some((start, length)) = range {
        let end = length
            .checked_sub(1)
            .and_then(|last| start.checked_add(last))
            .ok_or_else(|| {
                EngineError::RangeUnsupported("byte range is empty or overflows".into())
            })?;
        fetch_exact_range(job, start, end, true)?
    } else {
        let fetched = fetch_browser_first(job, None)?;
        if !matches!(fetched.status, 200 | 206) {
            return Err(EngineError::Failed(format!("HLS media HTTP {}", fetched.status)).into());
        }
        fetched
    };
    let expected = match range {
        Some((_, length)) => Some(length),
        None => full_entity_length(&fetched)?,
    };
    let mut body = fetched
        .body
        .take(expected.map_or(u64::MAX, |length| length.saturating_add(1)));
    let mut received = 0u64;
    let mut buffer = [0; 64 * 1024];
    let mut last_control = Instant::now();
    loop {
        if last_control.elapsed() >= Duration::from_millis(50) {
            check_job_control(job)?;
            last_control = Instant::now();
        }
        let count = body.read(&mut buffer).map_err(|error| {
            check_job_control(job)
                .err()
                .unwrap_or_else(|| EngineError::Failed(error.to_string()))
        })?;
        if count == 0 {
            break;
        }
        if expected.is_some_and(|length| count as u64 > length - received) {
            return Err(EngineError::Failed(if range.is_some() {
                "byte range response exceeds requested length".into()
            } else {
                "media response exceeds expected length".into()
            })
            .into());
        }
        crate::net_policy::consume(count, || check_job_control(job))?;
        writer.write_all(&buffer[..count])?;
        received += count as u64;
    }
    if expected.is_some_and(|length| received != length) {
        return Err(EngineError::Failed(format!(
            "incomplete response body: received {received} bytes, expected {}",
            expected.unwrap()
        ))
        .into());
    }
    check_job_control(job)?;
    Ok(())
}

#[cfg(test)]
pub fn fetch_hls_bytes_range(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    start: u64,
    length: u64,
) -> Result<Vec<u8>, EngineError> {
    fetch_bytes_range_with_transport(url, headers, proxy, start, length, true, None)
}

pub(crate) fn fetch_index_bytes_range(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    start: u64,
    length: u64,
    control: &Path,
) -> Result<Vec<u8>, EngineError> {
    check_control(control)?;
    fetch_bytes_range_with_transport(url, headers, proxy, start, length, false, Some(control))
}

fn fetch_bytes_range_with_transport(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    start: u64,
    length: u64,
    browser_first: bool,
    task_control: Option<&Path>,
) -> Result<Vec<u8>, EngineError> {
    if !http_fetch_url_allowed(url) {
        return Err(EngineError::Failed("url must be http(s)".into()));
    }
    if length == 0 {
        return Err(EngineError::RangeUnsupported("byte range is empty".into()));
    }
    let end = start
        .checked_add(length - 1)
        .ok_or_else(|| EngineError::RangeUnsupported("byte range overflows".into()))?;
    let control = task_control
        .map(Path::to_path_buf)
        .or_else(|| {
            crate::net_policy::current_throttle_context()
                .map(|context| context.control)
                .filter(|path| !path.as_os_str().is_empty())
        })
        .unwrap_or_default();
    let job = Job {
        url: url.to_string(),
        headers: headers.clone(),
        output: PathBuf::new(),
        connections: 1,
        chunk_bytes: 64 * 1024,
        total: 0,
        sequential: true,
        resume_from: 0,
        proxy: proxy.to_string(),
        resource_key: url.to_string(),
        etag: String::new(),
        last_modified: String::new(),
        control,
        progress: PathBuf::new(),
        method: "GET".into(),
        body_path: PathBuf::new(),
        mirrors: Vec::new(),
        replay_json: String::new(),
        curl_body_limit: Some(length),
        range_stop: None,
        installer_download: false,
        origin_boundary: String::new(),
    };
    let fetched = fetch_exact_range(&job, start, end, browser_first)?;
    let mut body = Vec::new();
    // 多读一个字节用于判定超长；每块读取时继续遵守暂停与限速。
    let mut reader = fetched.body.take(length.saturating_add(1));
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_job_control(&job)?;
        let count = reader.read(&mut buffer).map_err(|error| {
            check_job_control(&job)
                .err()
                .unwrap_or_else(|| EngineError::Failed(error.to_string()))
        })?;
        if count == 0 {
            break;
        }
        if count as u64 > length - body.len() as u64 {
            return Err(EngineError::Failed(
                "byte range response exceeds requested length".into(),
            ));
        }
        crate::net_policy::consume(count, || check_job_control(&job))?;
        body.extend_from_slice(&buffer[..count]);
    }
    if body.len() as u64 != length {
        return Err(EngineError::Failed(format!(
            "incomplete response body: byte range length {} != {length}",
            body.len()
        )));
    }
    check_job_control(&job)?;
    Ok(body)
}

fn fetch_exact_range(
    job: &Job,
    start: u64,
    end: u64,
    browser_first: bool,
) -> Result<FetchResult, EngineError> {
    let range = format!("bytes={start}-{end}");
    let fetched = if browser_first {
        fetch_browser_first(job, Some(&range))?
    } else {
        fetch(job, Some(&range))?
    };
    if fetched.status != 206 {
        return Err(EngineError::Failed(format!(
            "HTTP {}: byte range requires status 206",
            fetched.status
        )));
    }
    if fetched.content_encoding {
        return Err(EngineError::RangeUnsupported(
            "byte range response is not identity encoded".into(),
        ));
    }
    require_response_identity(job, &fetched)?;
    match parse_content_range(fetched.content_range.as_deref().unwrap_or("")) {
        Some((actual_start, actual_end, _)) if actual_start == start && actual_end == end => {}
        Some((actual_start, actual_end, _)) => {
            return Err(EngineError::RangeUnsupported(format!(
                "Content-Range {actual_start}-{actual_end} != {start}-{end}"
            )))
        }
        None => {
            return Err(EngineError::RangeUnsupported(
                "byte range response missing valid Content-Range".into(),
            ))
        }
    }
    let length = end - start + 1;
    if let Some(size) = fetched.content_length {
        if size != length {
            return Err(EngineError::Failed(format!(
                "byte range Content-Length {size} != {length}"
            )));
        }
    }
    Ok(fetched)
}

/// Download an exact media range without whole-resource fallback or buffering.
/// The caller may publish/checkpoint the file only after this succeeds.
pub(crate) fn download_bytes_range(job: &Job, start: u64, length: u64) -> Result<(), EngineError> {
    let end = length
        .checked_sub(1)
        .and_then(|last| start.checked_add(last))
        .ok_or_else(|| EngineError::RangeUnsupported("byte range is empty or overflows".into()))?;
    check_job_control(job)?;
    let fetched = fetch_exact_range(job, start, end, false)?;
    let mut file =
        fs::File::create(&job.output).map_err(|error| EngineError::Failed(error.to_string()))?;
    let mut body = fetched.body.take(length.saturating_add(1));
    let mut buffer = [0u8; 64 * 1024];
    let mut received = 0;
    loop {
        check_job_control(job)?;
        let count = body.read(&mut buffer).map_err(|error| {
            check_job_control(job)
                .err()
                .unwrap_or_else(|| EngineError::Failed(error.to_string()))
        })?;
        if count == 0 {
            break;
        }
        if count as u64 > length - received {
            return Err(EngineError::Failed(
                "byte range response exceeds requested length".into(),
            ));
        }
        crate::net_policy::consume(count, || check_job_control(job))?;
        file.write_all(&buffer[..count])
            .map_err(|error| EngineError::Failed(error.to_string()))?;
        received += count as u64;
    }
    if received != length {
        return Err(EngineError::Failed(format!(
            "incomplete response body: byte range length {received} != {length}"
        )));
    }
    file.sync_data()
        .map_err(|error| EngineError::Failed(error.to_string()))?;
    Ok(())
}

fn check_transfer_control(path: &Path, stop: Option<&AtomicBool>) -> Result<(), EngineError> {
    check_control(path)?;
    if stop.is_some_and(|stop| stop.load(Ordering::SeqCst)) {
        return Err(EngineError::Cancel);
    }
    Ok(())
}

fn check_job_control(job: &Job) -> Result<(), EngineError> {
    check_transfer_control(&job.control, job.range_stop.as_deref())
}

fn check_control(path: &Path) -> Result<(), EngineError> {
    if MEDIA_CANCELED.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(MediaCancellation::stopped)
    }) {
        return Err(EngineError::Cancel);
    }
    match read_control(path) {
        Control::Run => Ok(()),
        Control::Pause => Err(EngineError::Pause),
        Control::Cancel => Err(EngineError::Cancel),
    }
}

fn mark_file_sparse(_file: &File) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::IO::DeviceIoControl;
        const FSCTL_SET_SPARSE: u32 = 0x000900C4;
        unsafe {
            let mut returned = 0u32;
            let _ = DeviceIoControl(
                _file.as_raw_handle(),
                FSCTL_SET_SPARSE,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
                0,
                &mut returned,
                core::ptr::null_mut(),
            );
        }
    }
}

fn download_sequential(
    job: &Job,
    initial_response: Option<FetchResult>,
) -> Result<(), EngineError> {
    let resume_from = if request_method(job) == "GET" && job.resume_from > 0 && job.output.exists()
    {
        job.resume_from
            .min(job.output.metadata().map(|meta| meta.len()).unwrap_or(0))
    } else {
        0
    };
    let range = if resume_from > 0 {
        Some(format!("bytes={resume_from}-"))
    } else {
        None
    };
    let fetched = match initial_response {
        Some(response) => response,
        None => fetch(job, range.as_deref())?,
    };
    if resume_from > 0 && fetched.status == 200 {
        return Err(EngineError::RangeUnsupported(
            "server ignored sequential resume Range".into(),
        ));
    }
    if fetched.status != 200 && fetched.status != 206 {
        return Err(EngineError::Failed(format!("HTTP {}", fetched.status)));
    }
    if fetched.content_encoding {
        if resume_from > 0 || fetched.status == 206 {
            return Err(EngineError::RangeUnsupported(
                "server returned Content-Encoding on byte range".into(),
            ));
        }
        return Err(EngineError::Failed(
            "server returned unexpected Content-Encoding".into(),
        ));
    }
    if fetched.status == 206 {
        require_content_range_start(fetched.content_range.as_deref(), resume_from)?;
        require_content_range_total(fetched.content_range.as_deref(), job.total)?;
        if resume_from > 0 {
            require_response_identity(job, &fetched)?;
        }
    }
    // A sequential transfer must contain the remaining entity, not merely a
    // server-capped range. Content-Length alone only describes that fragment.
    let head = request_method(job).eq_ignore_ascii_case("HEAD");
    let complete_range_total = if fetched.status == 206 && !head {
        let (start, end, total) = fetched
            .content_range
            .as_deref()
            .and_then(parse_content_range)
            .and_then(|(start, end, total)| total.map(|total| (start, end, total)))
            .ok_or_else(|| EngineError::Failed("sequential Content-Range total missing".into()))?;
        if end + 1 != total {
            return Err(EngineError::Failed(
                "incomplete sequential Content-Range".into(),
            ));
        }
        if fetched
            .content_length
            .is_some_and(|length| length != total - start)
        {
            return Err(EngineError::Failed(
                "Content-Range response length mismatch".into(),
            ));
        }
        Some(total)
    } else {
        None
    };
    // Only the response may describe its own size.  `job.total` can be a stale
    // caller-supplied expected size or a `bytes=0-0` probe result, so it is trusted
    // only for a 206, where `require_content_range_total` above already proved the
    // server agrees with it.  A HEAD response carries an entity Content-Length but
    // no body at all, so it is never length-checked here.  The method must be read
    // through `request_method`, which trims and allowlists exactly what goes on the
    // wire: a job JSON carrying `" Head"` sends HEAD yet would fail this test
    // against the raw `job.method` and reintroduce the very "expected N, got 0"
    // failure this exemption exists to remove.
    let expected_downloaded = if head {
        None
    } else {
        complete_range_total.or_else(|| {
            fetched
                .content_length
                .map(|length| resume_from.saturating_add(length))
        })
    };
    let mut reader = fetched.body;
    let mut file = if resume_from > 0 {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&job.output)
            .map_err(|err| EngineError::Failed(err.to_string()))?
    } else {
        File::create(&job.output).map_err(|err| EngineError::Failed(err.to_string()))?
    };
    if resume_from > 0 {
        file.set_len(resume_from)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        file.seek(SeekFrom::Start(resume_from))
            .map_err(|err| EngineError::Failed(err.to_string()))?;
    }
    let mut buffer = vec![0u8; 64 * 1024];
    let mut downloaded = resume_from;
    let mut rate = TransferRate::new(resume_from);
    let mut last_progress = Instant::now();
    let mut last_control = Instant::now() - Duration::from_millis(50);
    loop {
        // 与分段路径一致：按时间检查控制，避免短网络读取反复打开同一控制文件。
        if last_control.elapsed() >= Duration::from_millis(50) {
            last_control = Instant::now();
            check_job_control(job)?;
        }
        let count = match reader.read(&mut buffer) {
            Ok(count) => count,
            Err(err) => {
                check_job_control(job)?;
                return Err(EngineError::Failed(err.to_string()));
            }
        };
        if count == 0 {
            break;
        }
        if expected_downloaded.is_some_and(|end| count as u64 > end.saturating_sub(downloaded)) {
            return Err(EngineError::Failed(
                "response body exceeds expected length".into(),
            ));
        }
        crate::net_policy::consume(count, || check_job_control(job))?;
        file.write_all(&buffer[..count])
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        downloaded += count as u64;
        if last_progress.elapsed() >= Duration::from_millis(200) {
            write_progress(
                &job.progress,
                downloaded,
                expected_downloaded.unwrap_or(0),
                rate.sample(downloaded) as f64,
                "downloading",
            );
            last_progress = Instant::now();
        }
    }
    check_job_control(job)?;
    file.flush()
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    if let Some(expected) = expected_downloaded {
        if downloaded != expected {
            return Err(EngineError::Failed(format!(
                "response body length mismatch: expected {expected}, got {downloaded}"
            )));
        }
    }
    write_progress(&job.progress, downloaded, downloaded, 0.0, "done");
    Ok(())
}

fn download_ranges(job: &Job, session_accept_ranges: bool) -> Result<(), EngineError> {
    remove_active_ranges(&job.progress);
    let total = job.total;
    if total == 0 {
        return download_sequential(job, None);
    }
    let workers = job.connections.clamp(1, 64);
    // 待办范围优先；尾部分流仍需显式启用和可靠校验器，等待真实传输验证。
    let chunk = range_chunk_size(total, workers, job.chunk_bytes);
    let mut ranges = Vec::new();
    let mut start = 0u64;
    while start < total {
        let end = start.saturating_add(chunk - 1).min(total - 1);
        ranges.push((start, end));
        start = end + 1;
    }
    let output_existed =
        job.output.exists() && job.output.metadata().map(|meta| meta.len()).unwrap_or(0) > 0;
    let loaded = if output_existed {
        match load_completed_ranges(job) {
            Some(ranges) => ranges,
            None => {
                let _ = crate::range_cache::remove(&completed_ranges_path(job));
                Vec::new()
            }
        }
    } else {
        let _ = crate::range_cache::remove(&completed_ranges_path(job));
        Vec::new()
    };
    let already = covered_bytes(&loaded);
    let pending: Vec<WorkRange> = ranges
        .iter()
        .flat_map(|(range_start, range_end)| {
            let first = loaded.partition_point(|(_, end)| end < range_start);
            subtract_ranges(*range_start, *range_end, &loaded[first..])
        })
        .collect();
    {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&job.output)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        mark_file_sparse(&file);
        file.set_len(total)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
    }
    if pending.is_empty() {
        if !job
            .control
            .with_file_name("playback-byte-priority")
            .exists()
        {
            let _ = crate::range_cache::remove(&completed_ranges_path(job));
        }
        write_progress(&job.progress, total, total, 0.0, "done");
        return Ok(());
    }
    // 只有"从零开始"的下载才能把 200 解释成服务器不支持 Range：此时没有任何已保留片段，
    // 续传偏移也为 0，整包 body 可以安全地作为完整资源使用。
    // 失效检查点的残留文件不会复用，也属于从零开始；文件存在本身不代表有效断点。
    let fresh_start = loaded.is_empty() && job.resume_from == 0;
    let split_minimum = active_split_minimum_for_session(
        job,
        session_accept_ranges,
        std::env::var("HLS_HTTP_ACTIVE_SPLIT").ok().as_deref(),
    );
    let max_useful_workers = if let Some(minimum) = split_minimum {
        pending
            .iter()
            .map(|range| ((range.end - range.start + 1) / minimum).max(1) as usize)
            .sum::<usize>()
            .max(pending.len())
    } else {
        pending.len()
    };
    let workers = workers.min(max_useful_workers);
    let mut scheduler = RangeScheduler::new(pending, workers);
    scheduler.split_minimum = split_minimum;
    let scheduler = Arc::new(scheduler);
    let downloaded = Arc::new(AtomicU64::new(already));
    let completed = Arc::new(RangeCheckpoints::new(loaded));
    let failed = Arc::new(Mutex::new(None::<EngineErrorCode>));
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();
    for worker_index in 0..workers {
        let job = job.clone();
        let network_scope = media_network_scope();
        let throttle = crate::net_policy::current_throttle_context();
        let scheduler = Arc::clone(&scheduler);
        let downloaded = Arc::clone(&downloaded);
        let completed = Arc::clone(&completed);
        let failed = Arc::clone(&failed);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            let _network = set_media_network_scope(network_scope);
            crate::net_policy::with_throttle_context(throttle, || {
                range_worker(
                    worker_index,
                    &job,
                    scheduler,
                    downloaded,
                    completed,
                    failed,
                    stop,
                    fresh_start,
                )
            })
        }));
    }
    let progress_stop = Arc::clone(&stop);
    let progress_downloaded = Arc::clone(&downloaded);
    let progress_path = job.progress.clone();
    let progress_control = job.control.clone();
    let progress_scheduler = Arc::clone(&scheduler);
    let mut rate = TransferRate::new(downloaded.load(Ordering::SeqCst));
    let progress = thread::spawn(move || {
        while !progress_stop.load(Ordering::SeqCst) {
            if matches!(
                read_control(&progress_control),
                Control::Pause | Control::Cancel
            ) {
                progress_stop.store(true, Ordering::SeqCst);
                break;
            }
            let bytes = progress_downloaded.load(Ordering::SeqCst);
            write_progress(
                &progress_path,
                bytes,
                total,
                rate.sample(bytes) as f64,
                "downloading",
            );
            write_active_ranges(&progress_path, &progress_scheduler.active_ranges());
            if let Some(root) = progress_control.parent() {
                // 播放路径写入时同时更新内存值；文件每任务至多每秒重读一次。
                if let Some(position) = crate::media::preview::byte_priority(root) {
                    progress_scheduler
                        .playback_priority
                        .store(position, Ordering::Relaxed);
                }
            }
            thread::sleep(Duration::from_millis(200));
        }
    });
    let mut fallback_response = None;
    for handle in handles {
        if let Ok(Some(response)) = handle.join() {
            fallback_response = Some(response);
        }
    }
    stop.store(true, Ordering::SeqCst);
    let _ = progress.join();
    remove_active_ranges(&job.progress);
    if let Some(code) = failed.lock().unwrap_or_else(|err| err.into_inner()).take() {
        // 从零开始的下载遇到"服务器根本不支持 Range"时，退回单流整包下载：
        // 已启动的 worker 全部join 完毕，不会有并发写；download_sequential 会重建输出文件。
        // 这样用户至少能拿到文件（失去分段并发与断点续传），而不是拿着已到手的整包失败。
        if fresh_start && matches!(code, EngineErrorCode::RangeIgnored(_)) {
            check_job_control(job)?;
            let _ = crate::range_cache::remove(&completed_ranges_path(job));
            remove_active_ranges(&job.progress);
            return download_sequential(job, fallback_response);
        }
        return Err(code.into_error());
    }
    match read_control(&job.control) {
        Control::Pause => return Err(EngineError::Pause),
        Control::Cancel => return Err(EngineError::Cancel),
        Control::Run => {}
    }
    let got = downloaded.load(Ordering::SeqCst);
    if got != total {
        return Err(EngineError::Failed(format!("downloaded {got} of {total}")));
    }
    let final_len = fs::metadata(&job.output)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if final_len != total {
        return Err(EngineError::Failed(format!(
            "file length mismatch, expected {total}, got {final_len}"
        )));
    }
    // 边播边下必须继续依据已落盘范围读取，直至 Core 切换成品后统一清理。
    if !job
        .control
        .with_file_name("playback-byte-priority")
        .exists()
    {
        let _ = crate::range_cache::remove(&completed_ranges_path(job));
    }
    write_progress(&job.progress, total, total, 0.0, "done");
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn active_split_minimum(job: &Job, setting: Option<&str>) -> Option<u64> {
    active_split_minimum_for_session(job, false, setting)
}

fn active_split_minimum_for_session(
    job: &Job,
    session_accept_ranges: bool,
    setting: Option<&str>,
) -> Option<u64> {
    let enabled = !matches!(
        setting.map(str::trim),
        Some("0" | "false" | "FALSE" | "off" | "OFF")
    );
    let validator_safe = (job.etag.trim().is_empty() || header_token_ok(job.etag.trim()))
        && (job.last_modified.trim().is_empty() || header_token_ok(job.last_modified.trim()));
    let can_split = !range_validator(job).is_empty()
        || (session_accept_ranges && job.total > 0 && validator_safe);
    (enabled && can_split).then_some(256 * 1024)
}

fn range_chunk_size(total: u64, workers: usize, requested: u64) -> u64 {
    let balanced = total.div_ceil((workers * 4) as u64).max(256 * 1024);
    // Limit the initial work list even when the advertised length is enormous.
    requested
        .max(64 * 1024)
        .min(balanced)
        .max(total.div_ceil(65_536))
}

#[derive(Clone, Debug)]
enum EngineErrorCode {
    Pause,
    Cancel,
    RangeUnsupported(String),
    /// 服务器对 Range 请求直接返回 200 整包，且本次下载没有已保留片段、响应身份与任务一致。
    /// 这与 If-Range 未命中（身份已变）或续传位置错位有本质区别：这里只是服务器不支持 Range，
    /// 整包 body 就是目标资源，可以退回单流下载。
    RangeIgnored(String),
    Failed(String),
}

impl EngineErrorCode {
    fn into_error(self) -> EngineError {
        match self {
            Self::Pause => EngineError::Pause,
            Self::Cancel => EngineError::Cancel,
            Self::RangeUnsupported(message) => EngineError::RangeUnsupported(message),
            // 对外语义不变：仍然是 RANGE_UNSUPPORTED 退出码与同一条消息，
            // 只有 download_ranges 会识别这个变体并决定退回单流。
            Self::RangeIgnored(message) => EngineError::RangeUnsupported(message),
            Self::Failed(message) => EngineError::Failed(message),
        }
    }
}

fn check_range_control(job: &Job, stop: &AtomicBool) -> Result<(), EngineErrorCode> {
    // 进度线程也会因暂停设置 stop；必须先保留控制文件中的暂停原因。
    check_job_control(job).map_err(|error| match error {
        EngineError::Pause => EngineErrorCode::Pause,
        EngineError::Cancel => EngineErrorCode::Cancel,
        other => EngineErrorCode::Failed(other.to_string()),
    })?;
    if stop.load(Ordering::SeqCst) {
        return Err(EngineErrorCode::Cancel);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct WorkRange {
    start: u64,
    end: u64,
}

struct ActiveProgress {
    cursor: u64,
    stop: u64,
    // Exclusive end of bytes reserved by the reader, including a pending write.
    reserved_until: u64,
    splittable: bool,
}

impl ActiveProgress {
    // 调用前旧响应必须已销毁或不再写入；新响应通过校验后才能重新开放分流。
    fn begin_fetch(&mut self) -> (u64, u64) {
        self.splittable = false;
        self.reserved_until = self.cursor;
        (self.cursor, self.stop)
    }
}

struct ActiveRange {
    id: u64,
    progress: Arc<Mutex<ActiveProgress>>,
}

struct AdaptiveConnectionController {
    maximum: usize,
    desired: AtomicUsize,
    successful_ranges: AtomicUsize,
}

impl AdaptiveConnectionController {
    fn new(maximum: usize) -> Self {
        let maximum = maximum.max(1);
        Self {
            maximum,
            desired: AtomicUsize::new(maximum.min(ADAPTIVE_INITIAL_CONNECTIONS)),
            successful_ranges: AtomicUsize::new(0),
        }
    }

    fn desired(&self) -> usize {
        self.desired.load(Ordering::Acquire)
    }

    fn permits(&self, worker_index: usize) -> bool {
        worker_index < self.desired()
    }

    fn note_success(&self) -> bool {
        let desired = self.desired();
        if desired >= self.maximum {
            return false;
        }
        let successes = self.successful_ranges.fetch_add(1, Ordering::AcqRel) + 1;
        if successes < ADAPTIVE_RAMP_SUCCESSES {
            return false;
        }
        let next = desired.saturating_add(ADAPTIVE_RAMP_STEP).min(self.maximum);
        if self
            .desired
            .compare_exchange(desired, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.successful_ranges.store(0, Ordering::Release);
            return true;
        }
        false
    }

    fn note_congestion(&self) {
        let mut current = self.desired();
        loop {
            let reduced = current.div_ceil(2).max(1);
            match self.desired.compare_exchange(
                current,
                reduced,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        self.successful_ranges.store(0, Ordering::Release);
    }
}

struct RangeScheduler {
    pending: Mutex<Vec<WorkRange>>,
    active: Mutex<Vec<ActiveRange>>,
    next_id: AtomicU64,
    playback_priority: AtomicU64,
    connections: AdaptiveConnectionController,
    wake_lock: Mutex<()>,
    wake: Condvar,
    split_minimum: Option<u64>,
    retry_not_before: Mutex<Option<Instant>>,
}

impl RangeScheduler {
    fn new(mut pending: Vec<WorkRange>, maximum_connections: usize) -> Self {
        // 普通领取从队尾取最长、起点最早的一段，避免每个请求都扫描整个队列。
        pending.sort_unstable_by_key(|item| {
            (
                item.end.saturating_sub(item.start),
                std::cmp::Reverse(item.start),
            )
        });
        Self {
            pending: Mutex::new(pending),
            active: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            playback_priority: AtomicU64::new(u64::MAX),
            connections: AdaptiveConnectionController::new(maximum_connections),
            wake_lock: Mutex::new(()),
            wake: Condvar::new(),
            split_minimum: None,
            retry_not_before: Mutex::new(None),
        }
    }

    fn defer_requests(&self, delay: Duration) {
        let Some(until) = Instant::now().checked_add(delay) else {
            return;
        };
        let mut deadline = self
            .retry_not_before
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        *deadline = Some(deadline.map_or(until, |previous| previous.max(until)));
    }

    fn wait_for_request(&self, job: &Job, stop: &AtomicBool) -> Result<(), EngineErrorCode> {
        loop {
            check_range_control(job, stop)?;
            let remaining = self
                .retry_not_before
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .and_then(|until| until.checked_duration_since(Instant::now()));
            let Some(remaining) = remaining else {
                return Ok(());
            };
            thread::sleep(remaining.min(Duration::from_millis(50)));
        }
    }

    fn claim(&self, worker_index: usize) -> Option<ActiveRange> {
        if !self.connections.permits(worker_index) {
            return None;
        }
        let mut pending = self.pending.lock().unwrap_or_else(|err| err.into_inner());
        if !self.connections.permits(worker_index) {
            return None;
        }
        let priority = self.playback_priority.swap(u64::MAX, Ordering::Relaxed);
        let priority_index = if priority == u64::MAX {
            None
        } else {
            pending
                .iter()
                .position(|item| item.start <= priority && priority <= item.end)
        };
        let mut active_ranges = self.active.lock().unwrap_or_else(|err| err.into_inner());
        let mut range = match priority_index {
            Some(index) => pending.remove(index),
            None => match pending.pop() {
                Some(range) => range,
                None => {
                    let minimum = self.split_minimum?;
                    let candidate = active_ranges
                        .iter()
                        .filter_map(|item| {
                            let p = item.progress.lock().unwrap_or_else(|err| err.into_inner());
                            let remaining = p
                                .stop
                                .saturating_add(1)
                                .saturating_sub(p.reserved_until.max(p.cursor));
                            (p.splittable && remaining >= minimum * 2).then_some((remaining, item))
                        })
                        .max_by_key(|(remaining, _)| *remaining)
                        .map(|(_, item)| item)?;
                    let mut p = candidate
                        .progress
                        .lock()
                        .unwrap_or_else(|err| err.into_inner());
                    let safe_start = p.reserved_until.max(p.cursor);
                    let remaining = p.stop.saturating_add(1).saturating_sub(safe_start);
                    if !p.splittable || remaining < minimum * 2 {
                        return None;
                    }
                    let split = safe_start + remaining / 2;
                    let range = WorkRange {
                        start: split,
                        end: p.stop,
                    };
                    p.stop = split - 1;
                    range
                }
            },
        };
        if priority_index.is_some() && priority > range.start {
            let head = WorkRange {
                start: range.start,
                end: priority - 1,
            };
            let key = (head.end - head.start, std::cmp::Reverse(head.start));
            let index = pending.partition_point(|item| {
                (
                    item.end.saturating_sub(item.start),
                    std::cmp::Reverse(item.start),
                ) < key
            });
            pending.insert(index, head);
            range.start = priority;
        }
        let active = ActiveRange {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            progress: Arc::new(Mutex::new(ActiveProgress {
                cursor: range.start,
                stop: range.end,
                reserved_until: range.start,
                splittable: false,
            })),
        };
        active_ranges.push(ActiveRange {
            id: active.id,
            progress: Arc::clone(&active.progress),
        });
        Some(active)
    }

    fn note_success(&self) {
        if self.connections.note_success() {
            self.wake.notify_all();
        }
    }

    fn note_congestion(&self) {
        self.connections.note_congestion();
    }

    fn note_storage_latency(&self, elapsed: Duration) {
        // 只用 payload 写入阻塞判断背压；串行检查点的等锁与同步耗时不代表新增连接压力。
        if elapsed >= Duration::from_millis(250) {
            self.connections.note_congestion();
        }
    }

    fn complete(&self, id: u64) {
        let mut active = self.active.lock().unwrap_or_else(|err| err.into_inner());
        active.retain(|item| item.id != id);
        let active_empty = active.is_empty();
        drop(active);
        let pending_empty = self
            .pending
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_empty();
        if (active_empty && pending_empty) || (self.split_minimum.is_some() && !active_empty) {
            self.wake.notify_all();
        }
    }

    fn wait_for_change(&self) {
        let guard = self.wake_lock.lock().unwrap_or_else(|err| err.into_inner());
        let _ = self
            .wake
            .wait_timeout(guard, Duration::from_millis(100))
            .unwrap_or_else(|err| err.into_inner());
    }

    fn is_idle(&self) -> bool {
        let active_empty = self
            .active
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_empty();
        let pending_empty = self
            .pending
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_empty();
        pending_empty && active_empty
    }

    fn active_ranges(&self) -> Vec<(u64, u64)> {
        self.active
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .iter()
            .filter_map(|item| {
                let progress = item.progress.lock().unwrap_or_else(|err| err.into_inner());
                (progress.cursor <= progress.stop)
                    .then_some((progress.cursor, progress.stop.saturating_add(1)))
            })
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn range_worker(
    worker_index: usize,
    job: &Job,
    scheduler: Arc<RangeScheduler>,
    downloaded: Arc<AtomicU64>,
    completed: Arc<RangeCheckpoints>,
    failed: Arc<Mutex<Option<EngineErrorCode>>>,
    stop: Arc<AtomicBool>,
    // 本次下载是否从零开始（无已保留片段、无续传偏移），含义见 fetch_range。
    fresh: bool,
) -> Option<FetchResult> {
    let mut transport_job = job.clone();
    transport_job.range_stop = Some(Arc::clone(&stop));
    let job = &transport_job;
    // 尚未领取范围的 worker 不打开文件或分配写缓冲；首次启用后复用同一个 writer。
    let mut writer: Option<BufWriter<File>> = None;
    loop {
        if stop.load(Ordering::SeqCst) {
            return None;
        }
        match read_control(&job.control) {
            Control::Pause => {
                let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                if slot.is_none() {
                    *slot = Some(EngineErrorCode::Pause);
                }
                stop.store(true, Ordering::SeqCst);
                return None;
            }
            Control::Cancel => {
                let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                if slot.is_none() {
                    *slot = Some(EngineErrorCode::Cancel);
                }
                stop.store(true, Ordering::SeqCst);
                return None;
            }
            Control::Run => {}
        }
        let Some(active) = scheduler.claim(worker_index) else {
            if scheduler.is_idle() {
                return None;
            }
            scheduler.wait_for_change();
            continue;
        };
        let file = match &mut writer {
            Some(file) => file,
            slot @ None => {
                let Ok(file) = OpenOptions::new().read(true).write(true).open(&job.output) else {
                    let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                    if slot.is_none() {
                        *slot = Some(EngineErrorCode::Failed("open payload failed".into()));
                    }
                    stop.store(true, Ordering::SeqCst);
                    scheduler.complete(active.id);
                    return None;
                };
                slot.insert(BufWriter::with_capacity(FILE_WRITE_BUFFER, file))
            }
        };
        let result = fetch_range(
            job,
            file,
            &active.progress,
            &downloaded,
            &completed,
            &scheduler,
            &stop,
            fresh,
        );
        match result {
            Ok(None) => {
                scheduler.complete(active.id);
                scheduler.note_success();
            }
            Ok(Some(response)) => {
                let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                // Only the first terminal result owns the response. Other
                // workers release theirs before the caller rebuilds the file.
                if slot.is_none() {
                    *slot = Some(EngineErrorCode::RangeIgnored(
                        "server ignored Range and returned 200".into(),
                    ));
                    stop.store(true, Ordering::SeqCst);
                    drop(slot);
                    scheduler.complete(active.id);
                    return Some(response);
                }
                drop(slot);
                scheduler.complete(active.id);
                return None;
            }
            Err(error) => {
                let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                if slot.is_none() {
                    *slot = Some(error);
                }
                stop.store(true, Ordering::SeqCst);
                drop(slot);
                scheduler.complete(active.id);
                return None;
            }
        }
    }
}

fn persist_range_progress(
    job: &Job,
    file: &mut BufWriter<File>,
    completed: &RangeCheckpoints,
    start: u64,
    cursor: u64,
) -> Result<Duration, EngineErrorCode> {
    if cursor <= start {
        return Ok(Duration::ZERO);
    }
    let started = Instant::now();
    file.flush()
        .map_err(|err| EngineErrorCode::Failed(err.to_string()))?;
    let flushed = started.elapsed();
    completed
        .pending
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .push((start, cursor - 1));
    // 各 worker 必须先 flush 再登记。合并同时到达的检查点，所有调用方仍等落盘及续传记录成功。
    if !cfg!(test) {
        thread::sleep(Duration::from_millis(2));
    }
    let mut durable = completed
        .durable
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let locked = started.elapsed();
    if range_covered(&durable, start, cursor - 1) {
        completed
            .pending
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .retain(|(start, end)| !range_covered(&durable, *start, *end));
        return Ok(flushed);
    }
    let pending = normalize_ranges(std::mem::take(
        &mut *completed
            .pending
            .lock()
            .unwrap_or_else(|err| err.into_inner()),
    ));
    if !range_covered(&pending, start, cursor - 1) {
        return Err(EngineErrorCode::Failed(
            "range checkpoint did not commit".into(),
        ));
    }
    file.get_ref()
        .sync_data()
        .map_err(|err| EngineErrorCode::Failed(err.to_string()))?;
    let synced = started.elapsed();
    let next = normalize_ranges(durable.iter().copied().chain(pending).collect());
    save_completed_ranges(job, &next)?;
    *durable = next;
    completed.mark_committed();
    let elapsed = started.elapsed();
    if std::env::var_os("HLS_HTTP_IO_PROFILE").is_some() {
        eprintln!(
            "http_checkpoint_us={} flush_us={} lock_us={} sync_us={} metadata_us={}",
            elapsed.as_micros(),
            flushed.as_micros(),
            locked.saturating_sub(flushed).as_micros(),
            synced.saturating_sub(locked).as_micros(),
            elapsed.saturating_sub(synced).as_micros(),
        );
    }
    Ok(flushed)
}

fn wait_before_range_retry(
    job: &Job,
    stop: &AtomicBool,
    failed_attempts: u32,
) -> Result<(), EngineErrorCode> {
    if failed_attempts >= MAX_RANGE_ATTEMPTS {
        return Ok(());
    }
    let delay_ms = if cfg!(test) {
        1
    } else {
        range_retry_delay_ms(failed_attempts, retry_jitter_seed())
    };
    let mut waited = 0u64;
    while waited < delay_ms {
        check_range_control(job, stop)?;
        let step = (delay_ms - waited).min(50);
        thread::sleep(Duration::from_millis(step));
        waited += step;
    }
    Ok(())
}

/// 指数退避加抖动：基准 200/400/800/1600 ms，实际取 [基准, 1.5×基准]。
/// 同一任务多个连接常因同一次网络抖动一起失败，固定间隔会让它们同步重连、再次同时触发限流。
fn range_retry_delay_ms(failed_attempts: u32, seed: u64) -> u64 {
    let base = 200u64.saturating_mul(1u64 << failed_attempts.saturating_sub(1).min(3));
    let spread = base / 2;
    base + seed % (spread + 1)
}

fn retry_jitter_seed() -> u64 {
    use std::hash::{BuildHasher, Hash, Hasher};
    // RandomState 每次构造带不同密钥；再混入线程与时间，不需要额外依赖。
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    thread::current().id().hash(&mut hasher);
    Instant::now().hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn extract_http_status(error: &str) -> Option<u16> {
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

pub(crate) fn retryable_http_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

// Same shared range state as range_worker; bundling it would only move the arguments.
#[allow(clippy::too_many_arguments)]
fn fetch_range(
    job: &Job,
    file: &mut BufWriter<File>,
    progress: &Arc<Mutex<ActiveProgress>>,
    downloaded: &AtomicU64,
    completed: &RangeCheckpoints,
    scheduler: &RangeScheduler,
    stop: &AtomicBool,
    // 本次下载是否从零开始（没有已保留片段、没有续传偏移）。只有为真时，
    // 服务器返回 200 才能被解释为"不支持 Range"而不是"资源身份已变"。
    fresh: bool,
) -> Result<Option<FetchResult>, EngineErrorCode> {
    let start = progress
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .cursor;
    let mut durable_cursor = start;
    let mut failed_attempts = 0u32;
    loop {
        scheduler.wait_for_request(job, stop)?;
        let (mut cursor, target_end) = progress
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .begin_fetch();
        if cursor > target_end {
            return Ok(None);
        }
        let request_start = cursor;
        let range = format!("bytes={cursor}-{target_end}");
        let mut fetched = match fetch(job, Some(&range)) {
            Ok(result) => result,
            Err(EngineError::RangeUnsupported(message)) => {
                return Err(EngineErrorCode::RangeUnsupported(message));
            }
            Err(EngineError::Pause) => return Err(EngineErrorCode::Pause),
            Err(EngineError::Cancel) => return Err(EngineErrorCode::Cancel),
            Err(err) => {
                scheduler.note_congestion();
                failed_attempts += 1;
                if failed_attempts >= MAX_RANGE_ATTEMPTS {
                    return Err(EngineErrorCode::Failed(format!(
                        "range {request_start}-{target_end} failed after {failed_attempts} attempts: {err}"
                    )));
                }
                wait_before_range_retry(job, stop, failed_attempts)?;
                continue;
            }
        };
        if fetched.content_encoding && (fetched.status == 200 || fetched.status == 206) {
            return Err(EngineErrorCode::RangeUnsupported(
                "server returned Content-Encoding on byte range".into(),
            ));
        }
        if fetched.status == 200 {
            // 服务器忽略 Range 返回整包。只有当这次下载本来没有任何已保留片段、
            // 且响应身份与任务一致（或无身份可校验）时，才能断定"这只是不支持 Range 的服务器"，
            // 整包 body 就是目标资源；否则可能是 If-Range 未命中（资源已变）或续传位置错位，
            // 拼接整包会污染已保留数据，必须按原来的 RangeUnsupported 失败。
            let identity_matches =
                fetched.etag.is_empty() || job.etag.is_empty() || fetched.etag == job.etag;
            if fresh && identity_matches && require_response_identity(job, &fetched).is_ok() {
                // 200 正文转交单流后不再属于范围 worker；仍保留用户暂停/取消检查。
                fetched.body.detach_range_stop();
                return Ok(Some(fetched));
            }
            return Err(EngineErrorCode::RangeUnsupported(
                "server ignored Range and returned 200".into(),
            ));
        }
        if fetched.status != 206 {
            if retryable_http_status(fetched.status) {
                scheduler.note_congestion();
                if let Some(delay) = fetched.retry_after {
                    if delay > MAX_RANGE_RETRY_AFTER {
                        return Err(EngineErrorCode::Failed(format!(
                            "HTTP {}: Retry-After {}s exceeds automatic retry window",
                            fetched.status,
                            delay.as_secs()
                        )));
                    }
                    scheduler.defer_requests(delay);
                }
                failed_attempts += 1;
                if failed_attempts >= MAX_RANGE_ATTEMPTS {
                    return Err(EngineErrorCode::Failed(format!(
                        "HTTP {} after {failed_attempts} attempts",
                        fetched.status
                    )));
                }
                drop(fetched);
                wait_before_range_retry(job, stop, failed_attempts)?;
                continue;
            }
            return Err(EngineErrorCode::Failed(format!("HTTP {}", fetched.status)));
        }
        if let Err(error) = require_response_identity(job, &fetched) {
            return Err(EngineErrorCode::RangeUnsupported(error.to_string()));
        }
        if let Err(err) = require_content_range_start(fetched.content_range.as_deref(), cursor) {
            return Err(match err {
                EngineError::RangeUnsupported(message) => {
                    EngineErrorCode::RangeUnsupported(message)
                }
                other => EngineErrorCode::Failed(other.to_string()),
            });
        }
        if let Err(err) = require_content_range_total(fetched.content_range.as_deref(), job.total) {
            return Err(match err {
                EngineError::RangeUnsupported(message) => {
                    EngineErrorCode::RangeUnsupported(message)
                }
                other => EngineErrorCode::Failed(other.to_string()),
            });
        }
        // A capped response is valid, but its declared span must bound every
        // byte written. Content-Length describes that span, not the whole job.
        let response_end = parse_content_range(fetched.content_range.as_deref().unwrap_or(""))
            .expect("validated Content-Range")
            .1;
        if response_end > target_end
            || fetched
                .content_length
                .is_some_and(|length| length != response_end - cursor + 1)
        {
            return Err(EngineErrorCode::RangeUnsupported(
                "Content-Range response length mismatch".into(),
            ));
        }
        progress
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .splittable = true;
        if scheduler.split_minimum.is_some() {
            scheduler.wake.notify_all();
        }
        let mut reader = fetched.body;
        if file.seek(SeekFrom::Start(cursor)).is_err() {
            return Err(EngineErrorCode::Failed("seek failed".into()));
        }
        let mut buffer = vec![0u8; WRITE_BATCH];
        let mut read_error = None;
        let mut last_control = Instant::now() - Duration::from_millis(50);
        loop {
            // 网络 read 的实际块往往远小于 WRITE_BATCH，不能每次 read 都重新打开控制文件。
            if stop.load(Ordering::SeqCst) || last_control.elapsed() >= Duration::from_millis(50) {
                last_control = Instant::now();
                if let Err(error) = check_range_control(job, stop) {
                    persist_range_progress(job, file, completed, start, cursor)?;
                    return Err(error);
                }
            }
            let read_limit = {
                let mut state = progress.lock().unwrap_or_else(|err| err.into_inner());
                let limit = state
                    .stop
                    .min(response_end)
                    .saturating_add(1)
                    .saturating_sub(cursor)
                    .min(buffer.len() as u64) as usize;
                state.reserved_until = cursor + limit as u64;
                limit
            };
            if read_limit == 0 {
                break;
            }
            let read = reader.read(&mut buffer[..read_limit]);
            // read 可能阻塞，返回后不能将协调器已停止的响应继续写盘。
            if stop.load(Ordering::SeqCst) {
                if let Err(error) = check_range_control(job, stop) {
                    persist_range_progress(job, file, completed, start, cursor)?;
                    return Err(error);
                }
            }
            let count = match read {
                Ok(count) => count,
                Err(err) => {
                    read_error = Some(err.to_string());
                    break;
                }
            };
            if count == 0 {
                break;
            }
            let current_stop = progress.lock().unwrap_or_else(|err| err.into_inner()).stop;
            let remain = current_stop.saturating_add(1).saturating_sub(cursor) as usize;
            let take = count.min(remain);
            if cursor + take as u64 == response_end + 1 && fetched.content_length.is_none() {
                // Validate the entity terminator before checkpointing its final bytes.
                match reader.read(&mut [0]) {
                    Ok(0) => {}
                    Ok(_) => {
                        return Err(EngineErrorCode::RangeUnsupported(
                            "range body exceeds Content-Range".into(),
                        ))
                    }
                    Err(error) => {
                        check_range_control(job, stop)?;
                        return Err(EngineErrorCode::Failed(error.to_string()));
                    }
                }
            }
            if take > 0 {
                if let Err(error) =
                    crate::net_policy::consume(take, || check_range_control(job, stop))
                {
                    persist_range_progress(job, file, completed, start, cursor)?;
                    return Err(error);
                }
            }
            if take > 0 {
                let write_started = Instant::now();
                if file.write_all(&buffer[..take]).is_err() {
                    return Err(EngineErrorCode::Failed("write failed".into()));
                }
                scheduler.note_storage_latency(write_started.elapsed());
            }
            cursor += take as u64;
            // 限速等待和磁盘写入不能占用进度锁，否则 active_ranges 会阻塞整个进度发布线程。
            {
                let mut state = progress.lock().unwrap_or_else(|err| err.into_inner());
                state.cursor = cursor;
                state.reserved_until = cursor;
            }
            downloaded.fetch_add(take as u64, Ordering::SeqCst);
            if durable_checkpoint_due(
                cursor.saturating_sub(durable_cursor),
                completed.since_last_commit(),
                || {
                    job.control
                        .parent()
                        .is_some_and(crate::media::preview::has_byte_priority)
                },
            ) {
                scheduler.note_storage_latency(persist_range_progress(
                    job, file, completed, start, cursor,
                )?);
                durable_cursor = cursor;
            }
            if cursor > current_stop.min(response_end) {
                break;
            }
        }
        // 重试前销毁旧响应，避免重新开放分流时仍有旧 reader 能继续写入。
        drop(reader);
        let current_stop = progress.lock().unwrap_or_else(|err| err.into_inner()).stop;
        if cursor > current_stop {
            if durable_cursor != cursor {
                scheduler.note_storage_latency(persist_range_progress(
                    job, file, completed, start, cursor,
                )?);
            }
            return Ok(None);
        }
        persist_range_progress(job, file, completed, start, cursor)?;
        durable_cursor = cursor;
        if cursor > request_start {
            failed_attempts = 0;
            if read_error.is_some() || cursor <= response_end {
                scheduler.note_congestion();
            } else {
                scheduler.note_success();
            }
            if read_error.is_some() {
                wait_before_range_retry(job, stop, 1)?;
            }
            continue;
        }
        scheduler.note_congestion();
        failed_attempts += 1;
        if failed_attempts >= MAX_RANGE_ATTEMPTS {
            let detail = read_error.unwrap_or_else(|| "range response ended without data".into());
            return Err(EngineErrorCode::Failed(format!(
                "range {request_start}-{target_end} made no progress after {failed_attempts} attempts: {detail}"
            )));
        }
        wait_before_range_retry(job, stop, failed_attempts)?;
    }
}

struct InterruptibleTcpStream {
    stream: TcpStream,
    control: PathBuf,
    last_control: Instant,
    range_stop: Option<Arc<AtomicBool>>,
}

impl Read for InterruptibleTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let started = Instant::now();
        loop {
            if self.last_control.elapsed() >= Duration::from_millis(50) {
                self.last_control = Instant::now();
                match check_transfer_control(&self.control, self.range_stop.as_deref()) {
                    Ok(()) => {}
                    Err(_) => {
                        // Interrupted 会被 read_exact 自动重试，不能用它表达任务停止。
                        return Err(std::io::Error::other("HTTP transfer stopped"));
                    }
                }
            }
            match self.stream.read(buf) {
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) && started.elapsed() < Duration::from_secs(60) => {}
                result => return result,
            }
        }
    }
}

enum Body {
    Tcp {
        prefix: Vec<u8>,
        at: usize,
        stream: InterruptibleTcpStream,
        remaining: Option<u64>,
    },
    Chunked(ChunkedReader<Box<Body>>),
    Curl(Box<CurlBody>),
    #[cfg(windows)]
    WinHttp(winhttp::WinHttpBody),
}

impl Body {
    fn detach_range_stop(&mut self) {
        match self {
            Self::Tcp { stream, .. } => stream.range_stop = None,
            Self::Chunked(body) => body.reader.get_mut().detach_range_stop(),
            #[cfg(windows)]
            Self::WinHttp(body) => body.range_stop = None,
            Self::Curl(body) => body.job.range_stop = None,
        }
    }
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp {
                prefix,
                at,
                stream,
                remaining,
            } => {
                if let Some(0) = remaining {
                    return Ok(0);
                }
                if *at < prefix.len() {
                    let take = (prefix.len() - *at).min(buf.len());
                    let take = match remaining {
                        Some(left) => take.min(*left as usize),
                        None => take,
                    };
                    buf[..take].copy_from_slice(&prefix[*at..*at + take]);
                    *at += take;
                    if let Some(left) = remaining.as_mut() {
                        *left = left.saturating_sub(take as u64);
                    }
                    return Ok(take);
                }
                let want = match remaining {
                    Some(left) => buf.len().min(*left as usize),
                    None => buf.len(),
                };
                let count = stream.read(&mut buf[..want])?;
                if let Some(left) = remaining.as_mut() {
                    *left = left.saturating_sub(count as u64);
                }
                Ok(count)
            }
            Self::Curl(body) => body.read(buf),
            Self::Chunked(body) => body.read(buf),
            #[cfg(windows)]
            Self::WinHttp(body) => body.read(buf),
        }
    }
}

// 分块定界属于传输层，写盘和进度统计只能看到解码后的实体字节。
struct ChunkedReader<R> {
    reader: BufReader<R>,
    remaining: u64,
    need_crlf: bool,
    finished: bool,
}

impl<R: Read> ChunkedReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            remaining: 0,
            need_crlf: false,
            finished: false,
        }
    }

    fn line(&mut self) -> std::io::Result<Vec<u8>> {
        let mut line = Vec::new();
        // 大小行和 trailer 有界；不按服务端声明的分块大小分配内存。
        while line.len() < 8192 {
            let mut byte = [0];
            self.reader.read_exact(&mut byte)?;
            line.push(byte[0]);
            if byte[0] == b'\n' {
                if line.ends_with(b"\r\n") {
                    line.truncate(line.len() - 2);
                    return Ok(line);
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "chunked line missing CRLF",
                ));
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "chunked line too long",
        ))
    }
}

impl<R: Read> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.finished {
            return Ok(0);
        }
        if self.remaining == 0 {
            if self.need_crlf {
                let mut crlf = [0; 2];
                self.reader.read_exact(&mut crlf)?;
                if crlf != *b"\r\n" {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "chunked data missing CRLF",
                    ));
                }
                self.need_crlf = false;
            }
            let line = self.line()?;
            let size = line.split(|byte| *byte == b';').next().unwrap_or_default();
            let size = std::str::from_utf8(size)
                .unwrap_or("")
                .trim_end_matches([' ', '\t']);
            if size.is_empty() || !size.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid chunked size",
                ));
            }
            self.remaining = u64::from_str_radix(size, 16).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "chunked size overflow")
            })?;
            if self.remaining == 0 {
                let mut trailer_bytes = 0;
                loop {
                    let line = self.line()?;
                    trailer_bytes += line.len() + 2;
                    if trailer_bytes > 64 * 1024 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "chunked trailers too long",
                        ));
                    }
                    if line.is_empty() {
                        break;
                    }
                    if !line.contains(&b':') {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid chunked trailer",
                        ));
                    }
                }
                self.finished = true;
                return Ok(0);
            }
            self.need_crlf = true;
        }
        let want = (buf.len() as u64).min(self.remaining) as usize;
        let count = self.reader.read(&mut buf[..want])?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "chunked body truncated",
            ));
        }
        self.remaining -= count as u64;
        Ok(count)
    }
}

/// One bounded reader lets the caller wait for data and observe task control.
/// Closing the receiver and killing curl also releases a blocked send/read;
/// Drop joins the reader, so redirects and abandoned tails leave no thread.
struct CurlBody {
    child: std::process::Child,
    receiver: Option<std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>>,
    reader: Option<thread::JoinHandle<()>>,
    job: Job,
    dir: PathBuf,
    buffer: Vec<u8>,
    at: usize,
    received: u64,
    limit: Option<u64>,
    expected: Option<u64>,
    last_control: Instant,
    idle_since: Instant,
    finished: bool,
    // 字段在 Drop::drop（结束子进程）之后才释放：名额覆盖整个进程生命周期。
    _permit: crate::curl_slots::CurlPermit,
}

impl CurlBody {
    fn check_control(&mut self) -> std::io::Result<()> {
        if self.last_control.elapsed() >= Duration::from_millis(50) {
            self.last_control = Instant::now();
            check_job_control(&self.job)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            if fs::metadata(self.dir.join("headers")).is_ok_and(|meta| meta.len() > 64 * 1024) {
                return Err(std::io::Error::other(
                    "curl response headers exceed byte limit",
                ));
            }
        }
        Ok(())
    }

    fn next_buffer(&mut self) -> std::io::Result<bool> {
        loop {
            self.check_control()?;
            match self
                .receiver
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(50))
            {
                Ok(result) => {
                    self.buffer = result?;
                    self.at = 0;
                    self.idle_since = Instant::now();
                    return Ok(true);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if self.idle_since.elapsed() >= Duration::from_secs(60) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "curl response stalled",
                        ));
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => loop {
                    self.check_control()?;
                    if let Some(status) = self.child.try_wait()? {
                        if !status.success() {
                            let error =
                                fs::read_to_string(self.dir.join("stderr")).unwrap_or_default();
                            return Err(std::io::Error::other(format!(
                                "curl-impersonate transfer failed ({status}): {}",
                                error.trim()
                            )));
                        }
                        self.finished = true;
                        return Ok(false);
                    }
                    if self.idle_since.elapsed() >= Duration::from_secs(60) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "curl did not exit",
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                },
            }
        }
    }
}

impl Read for CurlBody {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() || self.finished {
            return Ok(0);
        }
        self.check_control()?;
        if self.at == self.buffer.len() && !self.next_buffer()? {
            if self.expected.is_some_and(|length| length != self.received) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "curl response truncated",
                ));
            }
            return Ok(0);
        }
        let count = output.len().min(self.buffer.len() - self.at);
        let received = self.received + count as u64;
        if self.limit.is_some_and(|limit| received > limit) {
            return Err(std::io::Error::other(format!(
                "byte range response body exceeds {} bytes",
                self.limit.unwrap()
            )));
        }
        if self.expected.is_some_and(|length| received > length) {
            return Err(std::io::Error::other(
                "curl response exceeds expected length",
            ));
        }
        output[..count].copy_from_slice(&self.buffer[self.at..self.at + count]);
        self.at += count;
        self.received = received;
        // Callers may stop exactly at Content-Length/Content-Range. Validate
        // EOF and process success before releasing that final byte range.
        if self.expected == Some(received)
            && (self.at != self.buffer.len() || self.next_buffer()?)
        {
            return Err(std::io::Error::other(
                "curl response exceeds expected length",
            ));
        }
        Ok(count)
    }
}

impl Drop for CurlBody {
    fn drop(&mut self) {
        self.receiver.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

struct FetchResult {
    final_url: String,
    status: u16,
    location: Option<String>,
    content_range: Option<String>,
    content_length: Option<u64>,
    content_type: String,
    content_encoding: bool,
    retry_after: Option<Duration>,
    etag: String,
    last_modified: String,
    accept_ranges: bool,
    body: Body,
}

struct MediaCancellation {
    flag: Arc<AtomicBool>,
    peer: Arc<TcpStream>,
    last_check: Instant,
}

impl MediaCancellation {
    fn stopped(&mut self) -> bool {
        if self.flag.load(Ordering::Relaxed) {
            return true;
        }
        if self.last_check.elapsed() < Duration::from_millis(100) {
            return false;
        }
        self.last_check = Instant::now();
        match self.peer.peek(&mut [0u8; 1]) {
            Ok(0) => true,
            Ok(_) => false,
            Err(error) => !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
        }
    }
}

thread_local! {
    static MEDIA_CANCELED: std::cell::RefCell<Option<MediaCancellation>> = const { std::cell::RefCell::new(None) };
}

pub(crate) struct MediaResponse {
    pub status: u16,
    pub final_url: String,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub content_range: Option<String>,
    pub accept_ranges: bool,
    body: Body,
}

impl Read for MediaResponse {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        check_control(Path::new("")).map_err(|e| std::io::Error::other(e.to_string()))?;
        self.body.read(bytes)
    }
}

/// Open a streaming GET/HEAD with scoped replay and the existing redirect/proxy policy.
/// `spec.url` selects the initial request identity; `request_url` may add validated
/// LL-HLS delivery directives without losing that identity's exact URL headers.
/// The callback consumes the response on this thread, where cancellation also interrupts I/O.
#[allow(clippy::too_many_arguments)]
pub(crate) fn with_media_response<R>(
    spec: &crate::TaskSpec,
    replay: &str,
    request_url: &str,
    method: &str,
    range: Option<&str>,
    canceled: &Arc<AtomicBool>,
    peer: Arc<TcpStream>,
    consume: impl FnOnce(MediaResponse) -> Result<R, String>,
) -> Result<R, String> {
    struct Restore(Option<MediaCancellation>);
    impl Drop for Restore {
        fn drop(&mut self) {
            MEDIA_CANCELED.with(|slot| slot.replace(self.0.take()));
        }
    }
    peer.set_read_timeout(Some(Duration::from_millis(1)))
        .map_err(|e| e.to_string())?;
    let cancellation = MediaCancellation {
        flag: Arc::clone(canceled),
        peer,
        last_check: Instant::now() - Duration::from_secs(1),
    };
    let _restore = Restore(MEDIA_CANCELED.with(|slot| slot.replace(Some(cancellation))));
    if !matches!(method, "GET" | "HEAD") {
        return Err("媒体代理仅支持 GET/HEAD".into());
    }
    let mut job = Job {
        url: spec.url.clone(),
        headers: spec.headers.clone().into_iter().collect(),
        output: PathBuf::new(),
        connections: 1,
        chunk_bytes: 64 * 1024,
        total: 0,
        sequential: true,
        resume_from: 0,
        proxy: spec.proxy.clone(),
        resource_key: String::new(),
        etag: String::new(),
        last_modified: String::new(),
        control: PathBuf::new(),
        progress: PathBuf::new(),
        method: method.into(),
        body_path: PathBuf::new(),
        mirrors: Vec::new(),
        replay_json: replay.into(),
        curl_body_limit: None,
        range_stop: None,
        installer_download: false,
        origin_boundary: spec.origin_boundary.clone(),
    };
    apply_browser_profile(&mut job.headers);
    check_job_control(&job).map_err(|e| e.to_string())?;
    // curl 的文件型回退会先缓存整部媒体，不适合局域网流式播放。
    let response =
        fetch_follow_restricted(&job, range, None, Some(request_url)).map_err(|e| e.to_string())?;
    if response.content_encoding {
        return Err("媒体服务器未返回 identity 编码".into());
    }
    consume(MediaResponse {
        status: response.status,
        final_url: response.final_url,
        content_type: response.content_type,
        content_length: response.content_length,
        content_range: response.content_range,
        accept_ranges: response.accept_ranges,
        body: response.body,
    })
}

#[derive(Default)]
struct WafCurlCache {
    entries: HashMap<String, Instant>,
}

impl WafCurlCache {
    fn contains_at(&mut self, origin: &str, now: Instant) -> bool {
        if origin.is_empty() {
            return false;
        }
        let Some(&recorded) = self.entries.get(origin) else {
            return false;
        };
        if now.saturating_duration_since(recorded) >= WAF_CURL_CACHE_TTL {
            self.entries.remove(origin);
            return false;
        }
        true
    }

    fn record_at(&mut self, origin: String, now: Instant) {
        if origin.is_empty() {
            return;
        }
        self.entries
            .retain(|_, recorded| now.saturating_duration_since(*recorded) < WAF_CURL_CACHE_TTL);
        if !self.entries.contains_key(&origin) && self.entries.len() >= WAF_CURL_CACHE_MAX_ENTRIES {
            if let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, recorded)| **recorded)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest_key);
            }
        }
        self.entries.insert(origin, now);
    }

    fn invalidate(&mut self, origin: &str) {
        if !origin.is_empty() {
            self.entries.remove(origin);
        }
    }
}

fn waf_curl_cache() -> &'static Mutex<WafCurlCache> {
    static CACHE: std::sync::OnceLock<Mutex<WafCurlCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(WafCurlCache::default()))
}

fn waf_curl_cache_hit(origin: &str) -> bool {
    waf_curl_cache()
        .lock()
        .ok()
        .is_some_and(|mut cache| cache.contains_at(origin, Instant::now()))
}

fn waf_curl_cache_record(origin: String) {
    if let Ok(mut cache) = waf_curl_cache().lock() {
        cache.record_at(origin, Instant::now());
    }
}

fn waf_curl_cache_invalidate(origin: &str) {
    if let Ok(mut cache) = waf_curl_cache().lock() {
        cache.invalidate(origin);
    }
}

fn curl_low_speed_args(throttled: bool) -> &'static [&'static str] {
    if throttled {
        &[]
    } else {
        &["--speed-time", "30", "--speed-limit", "1024"]
    }
}

fn terminal_non_fallback_status(status: u16) -> bool {
    matches!(status, 401 | 404 | 410 | 429 | 451)
}

fn fetch(job: &Job, range: Option<&str>) -> Result<FetchResult, EngineError> {
    let _network = set_media_network_scope(Some(entry_network_scope(&job.url, &job.proxy)));
    let mut prepared = job.clone();
    apply_browser_profile(&mut prepared.headers);
    let origin = http_origin(&job.url).unwrap_or_default();
    let mut tried_curl = false;
    if !proxy_needs_curl(&job.proxy) && !origin.is_empty() && waf_curl_cache_hit(&origin) {
        match fetch_curl_follow(&prepared, range) {
            Some(Ok(fetched))
                if (200..400).contains(&fetched.status)
                    || terminal_non_fallback_status(fetched.status) =>
            {
                if !(200..400).contains(&fetched.status) {
                    waf_curl_cache_invalidate(&origin);
                }
                return Ok(fetched);
            }
            Some(Ok(_)) => {
                tried_curl = true;
                waf_curl_cache_invalidate(&origin);
            }
            Some(Err(error @ (EngineError::Pause | EngineError::Cancel))) => return Err(error),
            Some(Err(_)) | None => {
                waf_curl_cache_invalidate(&origin);
            }
        }
    }
    let first = fetch_follow(&prepared, range)?;
    if first.status != 403 || proxy_needs_curl(&job.proxy) || tried_curl {
        return Ok(first);
    }
    if let Some(via_curl) = fetch_curl_follow(&prepared, range) {
        if let Ok(ref fetched) = via_curl {
            if (200..400).contains(&fetched.status) && !origin.is_empty() {
                waf_curl_cache_record(origin);
            }
        }
        return via_curl;
    }
    Ok(first)
}

fn is_tls_handshake_error(url: &str, error: &EngineError) -> bool {
    if !url
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("https://")
    {
        return false;
    }
    let EngineError::Failed(message) = error else {
        return false;
    };
    if let Some(code) = message
        .strip_prefix("WinHttpSendRequest ")
        .and_then(|rest| rest.trim().parse::<u32>().ok())
    {
        return matches!(code, 12030 | 12037..=12062 | 12152 | 12157 | 12175);
    }
    let lower = message.to_ascii_lowercase();
    lower.contains("tls")
        || lower.contains("ssl")
        || lower.contains("handshake")
        || lower.contains("certificate")
        || lower.contains("secure")
}

fn fetch_browser_first(job: &Job, range: Option<&str>) -> Result<FetchResult, EngineError> {
    let _network = set_media_network_scope(Some(entry_network_scope(&job.url, &job.proxy)));
    let mut prepared = job.clone();
    apply_browser_profile(&mut prepared.headers);
    if proxy_needs_curl(&job.proxy) {
        return fetch_follow(&prepared, range);
    }
    let origin = http_origin(&job.url).unwrap_or_default();
    let mut tried_curl = false;
    if !origin.is_empty() && waf_curl_cache_hit(&origin) {
        match fetch_curl_follow(&prepared, range) {
            Some(Ok(fetched))
                if (200..400).contains(&fetched.status)
                    || matches!(fetched.status, 401 | 404 | 410 | 451) =>
            {
                if !(200..400).contains(&fetched.status) {
                    waf_curl_cache_invalidate(&origin);
                }
                return Ok(fetched);
            }
            Some(Ok(_)) => {
                tried_curl = true;
                waf_curl_cache_invalidate(&origin);
            }
            Some(Err(error @ (EngineError::Pause | EngineError::Cancel))) => return Err(error),
            Some(Err(_)) | None => {
                waf_curl_cache_invalidate(&origin);
            }
        }
    }
    match fetch_follow(&prepared, range) {
        Ok(first) => {
            if !matches!(first.status, 403 | 429 | 503) || tried_curl {
                return Ok(first);
            }
            match fetch_curl_follow(&prepared, range) {
                Some(Ok(via_curl)) => {
                    if (200..400).contains(&via_curl.status) && !origin.is_empty() {
                        waf_curl_cache_record(origin);
                    }
                    Ok(via_curl)
                }
                Some(Err(error @ (EngineError::Pause | EngineError::Cancel))) => Err(error),
                Some(Err(_)) | None => Ok(first),
            }
        }
        Err(first_error) => {
            if matches!(first_error, EngineError::Pause | EngineError::Cancel)
                || tried_curl
                || !is_tls_handshake_error(&job.url, &first_error)
            {
                return Err(first_error);
            }
            match fetch_curl_follow(&prepared, range) {
                Some(Ok(via_curl)) => {
                    if (200..400).contains(&via_curl.status) && !origin.is_empty() {
                        waf_curl_cache_record(origin);
                    }
                    Ok(via_curl)
                }
                Some(Err(error @ (EngineError::Pause | EngineError::Cancel))) => Err(error),
                Some(Err(_)) | None => Err(first_error),
            }
        }
    }
}

fn validate_download_destination(job: &Job, url: &str) -> Result<(), EngineError> {
    if job.installer_download && !crate::updater::installer_url_allowed(url) {
        return Err(EngineError::Failed(
            "Installer redirect outside trusted GitHub release hosts".into(),
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) enum NetworkScope {
    Public,
    Local,
    // A user-selected DNS name with only private answers. Its own connections
    // must stay private even if DNS changes after classification.
    LocalDns(String),
}

thread_local! {
    static NETWORK_SCOPE: std::cell::RefCell<Option<NetworkScope>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn media_network_scope() -> Option<NetworkScope> {
    NETWORK_SCOPE.with_borrow(Clone::clone)
}

pub(crate) fn set_media_network_scope(scope: Option<NetworkScope>) -> impl Drop {
    struct Restore(
        Option<NetworkScope>,
        std::marker::PhantomData<std::rc::Rc<()>>,
    );
    impl Drop for Restore {
        fn drop(&mut self) {
            NETWORK_SCOPE.set(self.0.take());
        }
    }
    Restore(NETWORK_SCOPE.replace(scope), std::marker::PhantomData)
}

fn private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => v6.to_ipv4().map_or_else(
            || v6.is_unique_local() || v6.is_unicast_link_local(),
            |v4| private_address(IpAddr::V4(v4)),
        ),
    }
}

fn local_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return private_address(ip);
    }
    let host = host.to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local")
}

pub(crate) fn public_entry(url: &str) -> bool {
    media_network_scope().map_or_else(
        || !parse_http_url(url).is_ok_and(|u| local_host(&u.host)),
        |scope| matches!(scope, NetworkScope::Public),
    )
}

pub(crate) fn entry_network_scope(url: &str, proxy: &str) -> NetworkScope {
    if let Some(scope) = media_network_scope() {
        return scope;
    }
    let Ok(parsed) = parse_http_url(url) else {
        return NetworkScope::Public;
    };
    if local_host(&parsed.host) {
        return NetworkScope::Local;
    }
    if proxy_route(proxy) == ProxyRoute::Direct && parsed.host.parse::<IpAddr>().is_err() {
        if let Ok(mut addresses) = (parsed.host.as_str(), parsed.port).to_socket_addrs() {
            if addresses
                .next()
                .is_some_and(|addr| private_address(addr.ip()))
                && addresses.all(|addr| private_address(addr.ip()))
            {
                return NetworkScope::LocalDns(parsed.host);
            }
        }
    }
    NetworkScope::Public
}

impl NetworkScope {
    fn destination(&self, parsed: &ParsedUrl, proxy: &str) -> Result<Vec<SocketAddr>, EngineError> {
        let public = match self {
            Self::Public => {
                validate_public_host(&parsed.host)?;
                if proxy_route(proxy) != ProxyRoute::Direct && !parsed.host.is_ascii() {
                    return Err(EngineError::Failed(
                        "proxied international host must use Punycode".into(),
                    ));
                }
                true
            }
            Self::LocalDns(host) if parsed.host.eq_ignore_ascii_case(host) => false,
            _ => return Ok(Vec::new()),
        };
        if proxy_route(proxy) != ProxyRoute::Direct || parsed.host.parse::<IpAddr>().is_ok() {
            return Ok(Vec::new());
        }
        let addresses: Vec<_> = (parsed.host.as_str(), parsed.port)
            .to_socket_addrs()
            .map_err(|e| EngineError::Failed(e.to_string()))?
            .filter(|addr| private_address(addr.ip()) != public)
            .collect();
        if addresses.is_empty() {
            return Err(EngineError::Failed(
                "DNS returned no address in the permitted network".into(),
            ));
        }
        Ok(addresses)
    }
}

fn validate_public_host(host: &str) -> Result<(), EngineError> {
    let name = host.trim_end_matches('.');
    // Some transports accept legacy numeric or escaped IPv4 spellings.
    let ambiguous = name.parse::<IpAddr>().is_err()
        && (name.contains('%')
            || name.split('.').all(|part| {
                part.bytes().all(|b| b.is_ascii_digit())
                    || part
                        .strip_prefix("0x")
                        .or_else(|| part.strip_prefix("0X"))
                        .is_some_and(|n| n.bytes().all(|b| b.is_ascii_hexdigit()))
            }));
    if local_host(host) || ambiguous {
        return Err(EngineError::Failed(
            "public URL cannot target a private network".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_media_target(public_only: bool, url: &str) -> Result<(), String> {
    if public_only && !url.is_empty() {
        let target = parse_http_url(url).map_err(|e| e.to_string())?;
        validate_public_host(&target.host).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn fetch_curl_follow(job: &Job, range: Option<&str>) -> Option<Result<FetchResult, EngineError>> {
    curl_exe_for(&job.url)?;
    let replay = job_replay_json(job);
    let mut url = job.url.clone();
    let mut headers = headers_for_request(job, &url);
    let mut hop = job.clone();
    let scope = entry_network_scope(&job.url, &job.proxy);
    apply_browser_profile(&mut headers);
    for _ in 0..16 {
        if let Err(error) = validate_download_destination(job, &url) {
            return Some(Err(error));
        }
        if !job.origin_boundary.is_empty()
            && http_origin(&url).as_deref() != Some(job.origin_boundary.as_str())
        {
            return Some(Err(EngineError::Failed(
                "redirect left project origin".into(),
            )));
        }
        hop.url = url.clone();
        hop.headers = headers.clone();
        let parsed = match parse_http_url(&url) {
            Ok(parsed) => parsed,
            Err(error) => return Some(Err(error)),
        };
        let destinations = match scope.destination(&parsed, &job.proxy) {
            Ok(destinations) => destinations,
            Err(error) => return Some(Err(error)),
        };
        let mut fetched = match fetch_via_curl_impersonate(&hop, range, &destinations)? {
            Ok(fetched) => fetched,
            Err(error) => return Some(Err(error)),
        };
        if fetched.status == 403 {
            if let Some(stripped) = strip_stale_cloudflare_cookies(&headers) {
                drop(fetched);
                headers = stripped;
                hop.headers = headers.clone();
                fetched = match fetch_via_curl_impersonate(&hop, range, &destinations)? {
                    Ok(fetched) => fetched,
                    Err(error) => return Some(Err(error)),
                };
            }
        }
        if matches!(fetched.status, 301 | 302 | 303 | 307 | 308) {
            let next = match fetched.location.clone() {
                Some(location) => resolve_location(&parsed, &location),
                None => return Some(Err(EngineError::Failed("redirect without Location".into()))),
            };
            if !http_fetch_url_allowed(&next) {
                return Some(Err(EngineError::Failed(
                    "invalid or insecure redirect".into(),
                )));
            }
            let status = fetched.status;
            drop(fetched);
            drop_cross_origin_secrets(&mut headers, &replay, &url, &next);
            apply_redirect_method(&mut hop, request_method(job), status, &mut headers);
            url = next;
            continue;
        }
        return Some(Ok(fetched));
    }
    Some(Err(EngineError::Failed("too many redirects".into())))
}

fn fetch_follow(job: &Job, range: Option<&str>) -> Result<FetchResult, EngineError> {
    fetch_follow_restricted(job, range, None, None)
}

fn fetch_follow_restricted(
    job: &Job,
    range: Option<&str>,
    origin: Option<&str>,
    request_url: Option<&str>,
) -> Result<FetchResult, EngineError> {
    let mut url = request_url.unwrap_or(&job.url).to_string();
    let scope = entry_network_scope(&job.url, &job.proxy);
    let replay = job_replay_json(job);
    let mut headers = headers_for_request(job, &job.url);
    if http_origin(&job.url) != http_origin(&url) {
        drop_cross_origin_secrets(&mut headers, &replay, &job.url, &url);
    }
    let mut hop = job.clone();
    for _ in 0..16 {
        validate_download_destination(job, &url)?;
        if [origin.unwrap_or_default(), job.origin_boundary.as_str()]
            .into_iter()
            .any(|boundary| !boundary.is_empty() && http_origin(&url).as_deref() != Some(boundary))
        {
            return Err(EngineError::Failed("redirect left project origin".into()));
        }
        let parsed = parse_http_url(&url)?;
        let destinations = scope.destination(&parsed, &job.proxy)?;
        hop.url = url.clone();
        let request = |headers: &HashMap<String, String>| {
            let mut hop = hop.clone();
            hop.headers = headers.clone();
            if proxy_needs_curl(&hop.proxy) {
                return fetch_via_curl_impersonate(&hop, range, &destinations).unwrap_or_else(
                    || {
                        Err(EngineError::Failed(
                            "代理认证或 SOCKS 路由需要 curl 传输".into(),
                        ))
                    },
                );
            }
            if parsed.https || !matches!(proxy_route(&hop.proxy), ProxyRoute::Direct) {
                #[cfg(windows)]
                {
                    return winhttp::get(&hop, &url, range, &destinations);
                }
                #[cfg(not(windows))]
                {
                    return Err(EngineError::Failed("https/proxy needs WinHTTP".into()));
                }
            }
            http_get(&hop, &parsed, range, &destinations)
        };
        let mut fetched = request(&headers)?;
        // 修正当前响应对应的 Cookie；不能重新重放上下文而把刚移除的值加回来。
        if fetched.status == 403 {
            if let Some(stripped) = strip_stale_cloudflare_cookies(&headers) {
                drop(fetched);
                headers = stripped;
                fetched = request(&headers)?;
            }
        }
        if matches!(fetched.status, 301 | 302 | 303 | 307 | 308) {
            let next = fetched
                .location
                .clone()
                .ok_or_else(|| EngineError::Failed("redirect without Location".into()))?;
            let next = resolve_location(&parsed, &next);
            if !http_fetch_url_allowed(&next) {
                return Err(EngineError::Failed("invalid or insecure redirect".into()));
            }
            let status = fetched.status;
            drop(fetched);
            drop_cross_origin_secrets(&mut headers, &replay, &url, &next);
            apply_redirect_method(&mut hop, request_method(job), status, &mut headers);
            url = next;
            continue;
        }
        return Ok(fetched);
    }
    Err(EngineError::Failed("too many redirects".into()))
}

pub(crate) fn http_origin(url: &str) -> Option<String> {
    let parsed = parse_http_url(url).ok()?;
    Some(format!(
        "{}://{}:{}",
        if parsed.https { "https" } else { "http" },
        uri_host(&parsed.host).to_ascii_lowercase(),
        parsed.port
    ))
}

pub(crate) fn request_origin(url: &str) -> String {
    let Ok(parsed) = parse_http_url(url) else {
        return String::new();
    };
    format!(
        "{}://{}",
        if parsed.https { "https" } else { "http" },
        host_header(&parsed).to_ascii_lowercase()
    )
}

pub(crate) struct MetadataResponse {
    pub status: u16,
    pub final_url: String,
    pub body: Vec<u8>,
    pub range: Option<(u64, u64, Option<u64>)>,
    pub etag: String,
    pub last_modified: String,
    pub content_length: Option<u64>,
}

/// Bounded metadata transfer using the normal streaming transports. No curl
/// fallback: that transport may buffer a whole response before returning headers.
pub(crate) fn read_metadata(
    spec: &crate::TaskSpec,
    range: Option<&str>,
    limit: u64,
    origin: Option<&str>,
) -> Result<MetadataResponse, String> {
    let control = crate::net_policy::current_throttle_context()
        .map(|c| c.control)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_default();
    let mut job = Job {
        url: spec.url.clone(),
        headers: spec.headers.clone().into_iter().collect(),
        output: PathBuf::new(),
        connections: 1,
        chunk_bytes: 64 * 1024,
        total: 0,
        sequential: true,
        resume_from: 0,
        proxy: spec.proxy.clone(),
        resource_key: spec.url.clone(),
        etag: spec.etag.clone(),
        last_modified: spec.last_modified.clone(),
        control,
        progress: PathBuf::new(),
        method: if spec.request_method == "HEAD" {
            "HEAD"
        } else {
            "GET"
        }
        .into(),
        body_path: PathBuf::new(),
        mirrors: Vec::new(),
        replay_json: String::new(),
        curl_body_limit: Some(limit),
        range_stop: None,
        installer_download: false,
        origin_boundary: spec.origin_boundary.clone(),
    };
    apply_browser_profile(&mut job.headers);
    let fetched = fetch_follow_restricted(&job, range, origin, None).map_err(|e| e.to_string())?;
    if range.is_some() && fetched.status != 206 {
        return Err("远程 ZIP 不支持 Range 或资源已变化，请先完整下载后预览".into());
    }
    require_response_identity(&job, &fetched).map_err(|e| e.to_string())?;
    let has_body = job.method != "HEAD" && fetched.status != 304;
    if has_body && fetched.content_length.is_some_and(|n| n > limit) {
        return Err("元数据超过读取上限".into());
    }
    let parsed_range = fetched
        .content_range
        .as_deref()
        .and_then(parse_content_range);
    let mut body = Vec::new();
    let status = fetched.status;
    let content_length = fetched.content_length;
    let expected = if has_body { content_length } else { None };
    let etag = fetched.etag.clone();
    let last_modified = fetched.last_modified.clone();
    let mut reader = fetched.body;
    let mut buffer = [0; 16 * 1024];
    if has_body {
        loop {
            check_job_control(&job).map_err(|e| e.to_string())?;
            let want = (limit.saturating_add(1).saturating_sub(body.len() as u64))
                .min(buffer.len() as u64) as usize;
            if want == 0 {
                return Err("元数据超过读取上限".into());
            }
            let n = reader
                .read(&mut buffer[..want])
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            crate::net_policy::consume(n, || check_job_control(&job)).map_err(|e| e.to_string())?;
            body.extend_from_slice(&buffer[..n]);
            if body.len() as u64 > limit {
                return Err("元数据超过读取上限".into());
            }
        }
    }
    if expected.is_some_and(|n| n != body.len() as u64) {
        return Err("元数据响应不完整".into());
    }
    Ok(MetadataResponse {
        status,
        final_url: fetched.final_url,
        body,
        range: parsed_range,
        etag,
        last_modified,
        content_length,
    })
}

fn apply_redirect_method(
    job: &mut Job,
    original_method: &str,
    status: u16,
    headers: &mut HashMap<String, String>,
) {
    let method = request_method(job);
    if (status == 303 && method != "HEAD") || (matches!(status, 301 | 302) && method == "POST") {
        job.method = "GET".into();
        job.body_path.clear();
    }
    // 后续跳转可能重新应用原请求上下文，不能把已丢弃的 POST 正文头加回 GET。
    if original_method == "POST" && request_method(job) == "GET" {
        headers.retain(|key, _| {
            !matches!(
                key.to_ascii_lowercase().as_str(),
                "content-encoding"
                    | "content-language"
                    | "content-location"
                    | "content-type"
                    | "content-length"
                    | "digest"
                    | "last-modified"
            )
        });
    }
}

fn job_replay_json(job: &Job) -> String {
    let replay = if !job.replay_json.trim().is_empty() {
        job.replay_json.clone()
    } else {
        crate::credentials::scoped_replay_json()
    };
    if replay.is_empty() {
        return replay;
    }
    let Ok(serde_json::Value::Object(mut object)) = serde_json::from_str(&replay) else {
        return replay;
    };
    if object
        .get("_task_url")
        .and_then(|v| v.as_str())
        .is_none_or(str::is_empty)
    {
        object.insert("_task_url".into(), job.url.clone().into());
        return serde_json::Value::Object(object).to_string();
    }
    replay
}

fn headers_for_request(job: &Job, url: &str) -> HashMap<String, String> {
    let mut map: std::collections::BTreeMap<String, String> = job
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let json = job_replay_json(job);
    if !json.trim().is_empty() {
        crate::apply_replay_json_for(&mut map, &json, url);
    }
    map.into_iter().collect()
}

fn drop_cross_origin_secrets(
    headers: &mut HashMap<String, String>,
    replay: &str,
    from: &str,
    to: &str,
) {
    let from_origin = crate::credentials::request_origin(from);
    let to_origin = crate::credentials::request_origin(to);
    if !from_origin.is_empty() && from_origin == to_origin {
        if !replay.trim().is_empty() {
            let mut map = headers
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            // 同域重定向也可能从入口跳到另一个 Cookie Path。
            crate::apply_replay_json_for(&mut map, replay, to);
            *headers = map.into_iter().collect();
        }
        return;
    }
    headers.retain(|key, _| !crate::download_worker::sensitive_header_name(key));
    if replay.trim().is_empty() || to_origin.is_empty() {
        return;
    }
    let mut map: std::collections::BTreeMap<String, String> = headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    crate::credentials::apply_scoped_request_context(&mut map, replay, to);
    *headers = map.into_iter().collect();
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CurlProxyPlan {
    Direct,
    Arg(String),
    Config(String),
}

fn curl_proxy_for_system_lookup(lookup: Result<Option<String>, String>) -> Option<CurlProxyPlan> {
    match lookup {
        Ok(None) => Some(CurlProxyPlan::Direct),
        Ok(Some(proxy)) => {
            if proxy.chars().any(char::is_control) {
                return None;
            }
            let (scheme, rest) = proxy.split_once("://")?;
            if rest.is_empty() {
                return None;
            }
            match scheme.to_ascii_lowercase().as_str() {
                "http" | "socks4" | "socks4a" | "socks5" | "socks5h" => {
                    Some(CurlProxyPlan::Config(proxy))
                }
                _ => None,
            }
        }
        Err(_) => None,
    }
}

fn apply_curl_proxy_args(
    command: &mut std::process::Command,
    plan: &CurlProxyPlan,
) -> Option<String> {
    match plan {
        CurlProxyPlan::Direct => {
            command.arg("--noproxy").arg("*");
            None
        }
        CurlProxyPlan::Arg(proxy) => {
            command.args(["--noproxy", ""]);
            command.arg("-x").arg(proxy);
            None
        }
        CurlProxyPlan::Config(proxy) => {
            command.args(["--noproxy", ""]);
            // Keep authenticated or resolved proxy endpoints out of process listings and curl diagnostics.
            let escaped = proxy.replace('\\', "\\\\").replace('"', "\\\"");
            command.args(["--config", "-"]);
            Some(format!("proxy = \"{escaped}\"\n"))
        }
    }
}

fn fetch_via_curl_impersonate(
    job: &Job,
    range: Option<&str>,
    destinations: &[SocketAddr],
) -> Option<Result<FetchResult, EngineError>> {
    let bundled = curl_exe_for(&job.url);
    let use_profile = bundled.is_some();
    let exe = bundled.or_else(|| {
        if !proxy_needs_curl(&job.proxy) {
            return None;
        }
        #[cfg(windows)]
        let path = {
            use std::os::windows::ffi::OsStringExt;
            use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
            let mut dir = vec![0u16; 32768];
            let len = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
            if len == 0 || len >= dir.len() {
                return None;
            }
            PathBuf::from(OsString::from_wide(&dir[..len])).join("curl.exe")
        };
        #[cfg(not(windows))]
        let path = PathBuf::from("/usr/bin/curl");
        path.is_file().then_some(path)
    })?;
    let proxy_plan = match proxy_route(&job.proxy) {
        ProxyRoute::Direct => CurlProxyPlan::Direct,
        ProxyRoute::Manual(proxy) => {
            if proxy_needs_curl(proxy) {
                if proxy.chars().any(char::is_control) {
                    return Some(Err(EngineError::Failed("代理地址无效".into())));
                }
                CurlProxyPlan::Config(proxy.to_string())
            } else {
                CurlProxyPlan::Arg(proxy.to_string())
            }
        }
        ProxyRoute::System => {
            if let Err(error) = check_job_control(job) {
                return Some(Err(error));
            }
            #[cfg(windows)]
            {
                let lookup = crate::proxy_tunnel::resolve_system_proxy(&job.url, || {
                    check_job_control(job).map_err(|error| error.to_string())
                });
                if let Err(error) = check_job_control(job) {
                    return Some(Err(error));
                }
                curl_proxy_for_system_lookup(lookup)?
            }
            #[cfg(not(windows))]
            {
                return None;
            }
        }
    };
    let headers = &job.headers;
    let dir = std::env::temp_dir().join(format!(
        "hls-curl-impersonate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    if fs::create_dir_all(&dir).is_err() {
        return Some(Err(EngineError::Failed("curl-impersonate temp dir".into())));
    }
    let result = (|| {
        check_job_control(job)?;
        // 浏览器身份路径每个响应一个 curl 子进程：按任务（控制文件）与全局封顶，
        // 超出上限的连接等待同任务的响应结束（等待期间照常响应暂停/取消）。
        let permit = crate::curl_slots::acquire(
            &job.control,
            crate::curl_slots::per_task_limit(),
            crate::curl_slots::GLOBAL_LIMIT,
            || check_job_control(job),
        )?;
        let header_path = dir.join("headers");
        File::create(&header_path).map_err(|error| EngineError::Failed(error.to_string()))?;
        let mut header_reader = BufReader::new(
            File::open(&header_path).map_err(|error| EngineError::Failed(error.to_string()))?,
        );
        let mut command = std::process::Command::new(exe);
        command
            // Ignore user curlrc: redirects and credential scope belong to Core.
            .arg("--disable")
            .arg("--globoff");
        if let Some(first) = destinations.first() {
            let ips = destinations
                .iter()
                .map(|addr| uri_host(&addr.ip().to_string()))
                .collect::<Vec<_>>()
                .join(",");
            // This process sends one hop. The wildcard also covers curl's IDNA normalization.
            command
                .arg("--resolve")
                .arg(format!("*:{}:{ips}", first.port()));
        }
        if range.is_none() {
            command.arg("--compressed");
        }
        if use_profile {
            command.args(["--impersonate", "chrome146"]);
        }
        command
            .arg("-sS")
            .arg("--connect-timeout")
            .arg("15")
            .args(curl_low_speed_args(
                crate::net_policy::throttle_or_quota_active(),
            ))
            .arg("--dump-header")
            .arg(&header_path)
            .arg("--suppress-connect-headers")
            .arg("--no-buffer")
            .arg("-o")
            .arg(if request_method(job) == "HEAD" {
                if cfg!(windows) {
                    "NUL"
                } else {
                    "/dev/null"
                }
            } else {
                "-"
            });
        command.arg("-A").arg(request_user_agent(headers));
        for (key, value) in headers {
            if key.eq_ignore_ascii_case("user-agent") || !header_allowed_on_request(key, value) {
                continue;
            }
            command.arg("-H").arg(format!("{key}: {value}"));
        }
        if let Some(range) = range {
            command.arg("-H").arg("Accept-Encoding: identity");
            command.arg("-H").arg(format!("Range: {range}"));
            let validator = range_validator(job);
            if !validator.is_empty() {
                command.arg("-H").arg(format!("If-Range: {validator}"));
            }
        }
        command.args(curl_request_args(job, headers));
        let proxy_config = apply_curl_proxy_args(&mut command, &proxy_plan);
        command.arg(&job.url);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let (stdin, stderr) = if proxy_config.is_some() {
            (std::process::Stdio::piped(), std::process::Stdio::null())
        } else {
            let file = File::create(dir.join("stderr"))
                .map_err(|error| EngineError::Failed(error.to_string()))?;
            (std::process::Stdio::null(), std::process::Stdio::from(file))
        };
        let mut child = command
            .stdin(stdin)
            .stdout(std::process::Stdio::piped())
            .stderr(stderr)
            .spawn()
            .map_err(|error| EngineError::Failed(error.to_string()))?;
        crate::job_object::assign_child(&child);
        drop(command);
        if let Some(config) = proxy_config {
            if let Err(error) = child
                .stdin
                .take()
                .expect("piped curl config")
                .write_all(config.as_bytes())
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(EngineError::Failed(format!("write curl config: {error}")));
            }
        }
        let mut stdout = child
            .stdout
            .take()
            .expect("curl stdout was configured as piped");
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let reader = match thread::Builder::new()
            .name("curl-response".into())
            .stack_size(256 * 1024)
            .spawn(move || loop {
                let mut buffer = vec![0; 64 * 1024];
                let result = match stdout.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        buffer.truncate(count);
                        Ok(buffer)
                    }
                    Err(error) => Err(error),
                };
                let failed = result.is_err();
                if sender.send(result).is_err() || failed {
                    break;
                }
            }) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(EngineError::Failed(error.to_string()));
            }
        };
        let mut body = CurlBody {
            child,
            receiver: Some(receiver),
            reader: Some(reader),
            job: job.clone(),
            dir: dir.clone(),
            buffer: Vec::new(),
            at: 0,
            received: 0,
            limit: None,
            expected: None,
            last_control: Instant::now() - Duration::from_millis(50),
            idle_since: Instant::now(),
            finished: false,
            _permit: permit,
        };
        // curl flushes each dumped header line. Keep headers/trailers separate
        // from stdout, otherwise HTTP trailers become part of the media body.
        // Informational responses cannot supply the final metadata.
        let mut header_bytes = 0;
        let (status, head) = loop {
            let mut head = Vec::new();
            loop {
                let mut byte = [0];
                body.check_control().map_err(|error| {
                    check_job_control(job)
                        .err()
                        .unwrap_or_else(|| EngineError::Failed(error.to_string()))
                })?;
                if header_reader
                    .read(&mut byte)
                    .map_err(|error| EngineError::Failed(error.to_string()))?
                    == 0
                {
                    if let Some(status) = body
                        .child
                        .try_wait()
                        .map_err(|error| EngineError::Failed(error.to_string()))?
                    {
                        let error = fs::read_to_string(dir.join("stderr")).unwrap_or_default();
                        return Err(EngineError::Failed(format!(
                            "curl response headers incomplete ({status}): {}",
                            error.trim()
                        )));
                    }
                    if body.idle_since.elapsed() >= Duration::from_secs(60) {
                        return Err(EngineError::Failed("curl response headers stalled".into()));
                    }
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                head.push(byte[0]);
                header_bytes += 1;
                if header_bytes > 64 * 1024 {
                    return Err(EngineError::Failed(
                        "curl response headers exceed byte limit".into(),
                    ));
                }
                if head.ends_with(b"\r\n\r\n") || head.ends_with(b"\n\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let mut fields = head.lines().next().unwrap_or_default().split_whitespace();
            let protocol = fields.next().unwrap_or_default();
            let status = fields
                .next()
                .and_then(|field| field.parse::<u16>().ok())
                .unwrap_or(0);
            if !protocol.starts_with("HTTP/") || !(100..600).contains(&status) || status == 101 {
                return Err(EngineError::Failed("invalid curl HTTP response".into()));
            }
            if status >= 200 {
                break (status, head);
            }
        };
        let meta = parse_header_meta(&head)?;
        record_download_type(job, status, &head);
        body.limit = job.curl_body_limit;
        // --compressed returns decoded bytes only when range.is_none(), so the
        // wire Content-Length cannot be applied to decoded streams. For range
        // requests we omit --compressed and keep the raw wire length + flag.
        let curl_decoded = range.is_none() && meta.content_encoding;
        let content_length = if curl_decoded {
            None
        } else {
            meta.content_length
        };
        body.expected = if request_method(job) == "HEAD" || matches!(status, 204 | 304) {
            Some(0)
        } else if let Some(length) = content_length {
            Some(length)
        } else if status == 206 {
            parse_content_range(meta.content_range.as_deref().unwrap_or_default())
                .map(|(start, end, _)| {
                    (end - start)
                        .checked_add(1)
                        .ok_or_else(|| EngineError::Failed("Content-Range length overflow".into()))
                })
                .transpose()?
        } else {
            None
        };
        let accept_ranges = meta.accept_ranges || meta.content_range.is_some();
        Ok(FetchResult {
            final_url: job.url.clone(),
            status,
            location: meta.location,
            content_range: meta.content_range,
            content_length,
            content_type: meta.content_type,
            content_encoding: range.is_some() && meta.content_encoding,
            retry_after: meta.retry_after,
            etag: meta.etag,
            last_modified: meta.last_modified,
            accept_ranges,
            body: Body::Curl(Box::new(body)),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    Some(result)
}

struct ParsedUrl {
    https: bool,
    host: String,
    port: u16,
    path: String,
}

fn is_wire_header_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn header_is_wire_safe(key: &str, value: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(is_wire_header_tchar)
        && !value.chars().any(|ch| ch.is_control() && ch != '\t')
}

fn header_forbidden_on_wire(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            | "connection"
            | "range"
            | "if-range"
            | "accept-encoding"
            | "transfer-encoding"
            | "te"
            | "upgrade"
            | "trailer"
            | "keep-alive"
            | "proxy-connection"
            | "proxy-authorization"
    )
}

fn header_allowed_on_request(key: &str, value: &str) -> bool {
    if MEDIA_CANCELED.with(|slot| slot.borrow().is_some())
        && matches!(
            key.to_ascii_lowercase().as_str(),
            "if-none-match" | "if-modified-since" | "if-match" | "if-unmodified-since"
        )
    {
        return false;
    }
    header_is_wire_safe(key, value) && !header_forbidden_on_wire(key)
}

fn header_token_ok(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

pub use hls_client::request_validation::{
    http_fetch_url_allowed, remote_resource_url_allowed, sanitize_http_method,
};

fn parse_http_url(raw: &str) -> Result<ParsedUrl, EngineError> {
    let raw = raw.trim();
    if !http_fetch_url_allowed(raw) {
        return Err(EngineError::Failed("url must be http(s)".into()));
    }
    let lower = raw.to_ascii_lowercase();
    let (https, prefix_len) = if lower.starts_with("https://") {
        (true, "https://".len())
    } else if lower.starts_with("http://") {
        (false, "http://".len())
    } else {
        return Err(EngineError::Failed("invalid URL scheme".into()));
    };
    let rest = raw[prefix_len..]
        .split_once('#')
        .map(|(head, _)| head)
        .unwrap_or(&raw[prefix_len..]);
    let authority_end = [rest.find('/'), rest.find('?')]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(rest.len());
    let (hostport, request_target) = rest.split_at(authority_end);
    // Socket hostnames are bare; HTTP authorities/WinHTTP retain brackets.
    // Canonicalize literals so equivalent IPv6 spellings share an origin.
    let (host, port) = if let Some(literal) = hostport.strip_prefix('[') {
        let (address, suffix) = literal
            .split_once(']')
            .ok_or_else(|| EngineError::Failed("invalid IPv6 host".into()))?;
        let host = ipv6_host(
            address
                .parse::<Ipv6Addr>()
                .map_err(|_| EngineError::Failed("invalid IPv6 host".into()))?,
        );
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| EngineError::Failed("invalid IPv6 authority".into()))?,
            )
        };
        (host, port)
    } else if let Some((host, port)) = hostport.rsplit_once(':') {
        if host.contains([':', '[', ']']) {
            return Err(EngineError::Failed("invalid url host".into()));
        }
        (host.to_string(), Some(port))
    } else {
        if hostport.contains(['[', ']']) {
            return Err(EngineError::Failed("invalid url host".into()));
        }
        (hostport.to_string(), None)
    };
    let port = match port {
        Some(port) if !port.is_empty() && port.bytes().all(|ch| ch.is_ascii_digit()) => port
            .parse::<u16>()
            .map_err(|_| EngineError::Failed("invalid port".into()))?,
        Some(_) => return Err(EngineError::Failed("invalid port".into())),
        None => {
            if https {
                443
            } else {
                80
            }
        }
    };
    if host.is_empty()
        || host
            .chars()
            .any(|ch| ch.is_ascii_whitespace() || matches!(ch, '/' | '\\' | '\0' | '#' | '?' | '@'))
    {
        return Err(EngineError::Failed("url host missing".into()));
    }
    if request_target
        .chars()
        .any(|ch| ch.is_ascii_control() || ch == ' ')
    {
        return Err(EngineError::Failed("url path invalid".into()));
    }
    Ok(ParsedUrl {
        https,
        host,
        port,
        path: if request_target.starts_with('?') {
            format!("/{request_target}")
        } else if request_target.is_empty() {
            "/".into()
        } else {
            request_target.into()
        },
    })
}

fn host_header(parsed: &ParsedUrl) -> String {
    let default_port = if parsed.https { 443 } else { 80 };
    if parsed.port == default_port {
        uri_host(&parsed.host)
    } else {
        format!("{}:{}", uri_host(&parsed.host), parsed.port)
    }
}

fn uri_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

pub(crate) fn ipv6_host(address: Ipv6Addr) -> String {
    // Browser URL origins serialize mapped addresses as hexadecimal pieces.
    if address.to_ipv4_mapped().is_some() {
        let segments = address.segments();
        format!("::ffff:{:x}:{:x}", segments[6], segments[7])
    } else {
        address.to_string()
    }
}

fn header_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    if key.trim().eq_ignore_ascii_case(name) {
        Some(value.trim())
    } else {
        None
    }
}

fn record_download_type(job: &Job, status: u16, headers: &str) {
    if !matches!(status, 200 | 206) || job.output.as_os_str().is_empty() {
        return;
    }
    let value = |name| {
        headers
            .lines()
            .filter_map(|line| header_value(line, name))
            .next_back()
            .unwrap_or("")
    };
    let metadata = serde_json::json!({
        "content_type": value("Content-Type"),
        "content_disposition": value("Content-Disposition"),
    });
    let _ = fs::write(
        job.progress.with_extension("response.json"),
        metadata.to_string(),
    );
}

#[derive(Default)]
struct HeaderMeta {
    location: Option<String>,
    content_length: Option<u64>,
    content_type: String,
    content_range: Option<String>,
    retry_after: Option<Duration>,
    etag: String,
    last_modified: String,
    accept_ranges: bool,
    chunked: bool,
    content_encoding: bool,
}

fn parse_header_meta(head: &str) -> Result<HeaderMeta, EngineError> {
    let mut meta = HeaderMeta::default();
    for line in head.lines() {
        if let Some(value) = header_value(line, "Location") {
            meta.location = Some(value.to_string());
        }
        if let Some(value) = header_value(line, "Content-Length") {
            for field in value.split(',') {
                let length = http_decimal(field.trim())
                    .ok_or_else(|| EngineError::Failed("invalid Content-Length".into()))?;
                if meta
                    .content_length
                    .is_some_and(|previous| previous != length)
                {
                    return Err(EngineError::Failed("conflicting Content-Length".into()));
                }
                meta.content_length = Some(length);
            }
        }
        if let Some(value) = header_value(line, "Content-Type") {
            meta.content_type = value.to_string();
        }
        if let Some(value) = header_value(line, "Content-Range") {
            if meta.content_range.replace(value.to_string()).is_some() {
                return Err(EngineError::Failed("multiple Content-Range fields".into()));
            }
        }
        if let Some(value) = header_value(line, "ETag") {
            if !meta.etag.is_empty() && value != meta.etag {
                return Err(EngineError::Failed("conflicting ETag".into()));
            }
            meta.etag = value.to_string();
        }
        if let Some(value) = header_value(line, "Last-Modified") {
            if !meta.last_modified.is_empty() && value != meta.last_modified {
                return Err(EngineError::Failed("conflicting Last-Modified".into()));
            }
            meta.last_modified = value.to_string();
        }
        if let Some(value) = header_value(line, "Retry-After") {
            // 支持 delta-seconds；无效值保留现有本地退避，不猜测日期格式。
            meta.retry_after = value.parse::<u64>().ok().map(Duration::from_secs);
        }
        if let Some(value) = header_value(line, "Accept-Ranges") {
            meta.accept_ranges = value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("bytes"));
        }
        if let Some(value) = header_value(line, "Transfer-Encoding") {
            if meta.chunked || !value.eq_ignore_ascii_case("chunked") {
                return Err(EngineError::Failed("unsupported Transfer-Encoding".into()));
            }
            meta.chunked = true;
        }
        if let Some(value) = header_value(line, "Content-Encoding") {
            // `identity` and an empty value describe the unchanged entity.
            meta.content_encoding |=
                !value.trim().is_empty() && !value.trim().eq_ignore_ascii_case("identity");
        }
    }
    if meta.chunked && meta.content_length.is_some() {
        return Err(EngineError::Failed(
            "conflicting HTTP response framing".into(),
        ));
    }
    Ok(meta)
}

fn http_decimal(value: &str) -> Option<u64> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn parse_content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let mut text = value.trim();
    if text
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bytes"))
    {
        text = text[5..].strip_prefix(' ')?.trim_start();
    }
    let (range, total_text) = text.split_once('/')?;
    let (start_text, end_text) = range.split_once('-')?;
    let start = http_decimal(start_text.trim())?;
    let end = http_decimal(end_text.trim())?;
    let total_text = total_text.trim();
    let total = if total_text == "*" {
        None
    } else {
        Some(http_decimal(total_text)?)
    };
    if end < start {
        return None;
    }
    if let Some(total) = total {
        if total == 0 || end >= total {
            return None;
        }
    }
    Some((start, end, total))
}

fn require_content_range_start(
    header: Option<&str>,
    expected_start: u64,
) -> Result<(), EngineError> {
    match parse_content_range(header.unwrap_or("")) {
        Some((start, _, _)) if start == expected_start => Ok(()),
        Some((start, _, _)) => Err(EngineError::RangeUnsupported(format!(
            "Content-Range start {start} != {expected_start}"
        ))),
        None => Err(EngineError::RangeUnsupported(
            "Range response missing valid Content-Range".into(),
        )),
    }
}

fn require_content_range_total(
    header: Option<&str>,
    expected_total: u64,
) -> Result<(), EngineError> {
    if expected_total == 0 {
        return Ok(());
    }
    match parse_content_range(header.unwrap_or("")) {
        Some((_, _, Some(total))) if total == expected_total => Ok(()),
        Some((_, _, Some(total))) => Err(EngineError::RangeUnsupported(format!(
            "Content-Range total {total} != {expected_total}"
        ))),
        Some((_, _, None)) => Err(EngineError::RangeUnsupported(
            "Content-Range total missing".into(),
        )),
        None => Err(EngineError::RangeUnsupported(
            "Range response missing valid Content-Range".into(),
        )),
    }
}

fn require_response_identity(job: &Job, response: &FetchResult) -> Result<(), EngineError> {
    let etag = strong_etag(&job.etag);
    let (field, expected, received) = if header_token_ok(&etag) {
        ("ETag", etag.as_str(), response.etag.trim())
    } else {
        (
            "Last-Modified",
            job.last_modified.trim(),
            response.last_modified.trim(),
        )
    };
    // If-Range 响应可省略表示元数据，但不能用明确冲突的身份拼接已保存的字节。
    if header_token_ok(expected) && !received.is_empty() && received != expected {
        return Err(EngineError::RangeUnsupported(format!(
            "resource identity changed: {field} differs from the saved download"
        )));
    }
    Ok(())
}

fn full_entity_length(response: &FetchResult) -> Result<Option<u64>, EngineError> {
    if response.content_encoding {
        return Err(EngineError::Failed("unexpected Content-Encoding".into()));
    }
    if response.status == 206 {
        let range = response
            .content_range
            .as_deref()
            .and_then(parse_content_range);
        if let Some((0, end, Some(total))) = range {
            if end + 1 == total && response.content_length.is_none_or(|len| len == total) {
                return Ok(Some(total));
            }
        }
        return Err(EngineError::RangeUnsupported(
            "incomplete full entity Content-Range".into(),
        ));
    }
    Ok(response.content_length)
}

fn request_method(job: &Job) -> &'static str {
    let method = job.method.trim();
    if method.eq_ignore_ascii_case("POST") {
        "POST"
    } else if method.eq_ignore_ascii_case("HEAD") {
        "HEAD"
    } else {
        "GET"
    }
}

fn post_body(job: &Job) -> Result<Vec<u8>, EngineError> {
    if !request_method(job).eq_ignore_ascii_case("POST") || job.body_path.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    fs::read(&job.body_path).map_err(|err| EngineError::Failed(err.to_string()))
}

fn curl_request_args(job: &Job, headers: &HashMap<String, String>) -> Vec<OsString> {
    let mut args = Vec::new();
    match request_method(job) {
        "POST" => {
            args.push(OsString::from("--request"));
            args.push(OsString::from("POST"));
            args.push(OsString::from("--data-binary"));
            if job.body_path.as_os_str().is_empty() {
                args.push(OsString::new());
            } else {
                let mut body = OsString::from("@");
                body.push(&job.body_path);
                args.push(body);
            }
            // --data-binary otherwise invents application/x-www-form-urlencoded.
            // Preserve native/raw semantics when the caller did not set a type.
            if !headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("content-type"))
            {
                args.push(OsString::from("-H"));
                args.push(OsString::from("Content-Type:"));
            }
        }
        "HEAD" => {
            // `-X HEAD` does not give curl HEAD response semantics; --head does.
            args.push(OsString::from("--head"));
        }
        _ => {}
    }
    args
}

fn range_validator(job: &Job) -> String {
    let etag = strong_etag(&job.etag);
    if header_token_ok(&etag) {
        return etag;
    }
    let modified = job.last_modified.trim().to_string();
    if header_token_ok(&modified) {
        modified
    } else {
        String::new()
    }
}

fn resolve_location(current: &ParsedUrl, location: &str) -> String {
    let location = location.trim();
    if location.is_empty()
        || location.contains('\r')
        || location.contains('\n')
        || location.contains('\0')
    {
        return String::new();
    }
    let scheme = if current.https { "https" } else { "http" };
    let lower = location.to_ascii_lowercase();
    if current.https && lower.starts_with("http://") {
        return String::new();
    }
    let target = if lower.starts_with("http://") || lower.starts_with("https://") {
        location.to_string()
    } else if has_absolute_scheme(location) {
        return String::new();
    } else if let Some(rest) = location.strip_prefix("//") {
        format!("{scheme}://{rest}")
    } else {
        let origin = format!("{scheme}://{}", host_header(current));
        let base_path = current.path.split('?').next().unwrap_or("/");
        if location.starts_with('/') {
            format!("{origin}{location}")
        } else if location.starts_with('?') {
            format!("{origin}{base_path}{location}")
        } else if location.starts_with('#') {
            format!("{origin}{}{location}", current.path)
        } else {
            let dir = base_path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
            format!("{origin}{dir}/{location}")
        }
    };
    // RFC 3986: only the path participates in dot-segment removal. Preserve
    // authority, query bytes and percent escapes for signed request identities.
    let authority_start = target.find("://").unwrap() + 3;
    let path_start = target[authority_start..]
        .find(['/', '?', '#'])
        .map(|index| authority_start + index)
        .unwrap_or(target.len());
    let path_end = target[path_start..]
        .find(['?', '#'])
        .map(|index| path_start + index)
        .unwrap_or(target.len());
    format!(
        "{}{}{}",
        &target[..path_start],
        crate::harvest::normalize_url_path(&target[path_start..path_end]),
        &target[path_end..]
    )
}

fn has_absolute_scheme(reference: &str) -> bool {
    let bytes = reference.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let mut index = 1;
    while index < bytes.len()
        && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'+' | b'.' | b'-'))
    {
        index += 1;
    }
    bytes.get(index) == Some(&b':')
}

fn http_get(
    job: &Job,
    parsed: &ParsedUrl,
    range: Option<&str>,
    destinations: &[SocketAddr],
) -> Result<FetchResult, EngineError> {
    let mut stream = match proxy_route(&job.proxy) {
        ProxyRoute::Direct if !destinations.is_empty() => {
            TcpStream::connect(destinations).map_err(|e| EngineError::Failed(e.to_string()))?
        }
        ProxyRoute::Direct => TcpStream::connect((parsed.host.as_str(), parsed.port))
            .map_err(|err| EngineError::Failed(err.to_string()))?,
        ProxyRoute::System | ProxyRoute::Manual(_) => {
            return Err(EngineError::Failed("http proxy uses WinHTTP".into()));
        }
    };
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    let mut header = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept-Encoding: identity\r\nUser-Agent: {}\r\n",
        request_method(job),
        parsed.path,
        host_header(parsed),
        request_user_agent(&job.headers)
    );
    if let Some(range) = range {
        header.push_str(&format!("Range: {range}\r\n"));
        let validator = range_validator(job);
        if !validator.is_empty() {
            header.push_str(&format!("If-Range: {validator}\r\n"));
        }
    }
    for (key, value) in &job.headers {
        if key.eq_ignore_ascii_case("user-agent") || !header_allowed_on_request(key, value) {
            continue;
        }
        header.push_str(&format!("{key}: {value}\r\n"));
    }
    let body = post_body(job)?;
    if !body.is_empty() {
        header.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    header.push_str("\r\n");
    stream
        .write_all(header.as_bytes())
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    if !body.is_empty() {
        stream
            .write_all(&body)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
    }
    // 短超时只用于轮询暂停/取消；持续慢网仍沿用 60 秒无数据超时。
    // 包住响应头和实体底层，使 chunked 的大小行和 trailer 等待也能停止。
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    let mut stream = InterruptibleTcpStream {
        stream,
        control: job.control.clone(),
        last_control: Instant::now() - Duration::from_millis(50),
        range_stop: job.range_stop.clone(),
    };
    let mut buf = Vec::with_capacity(4096);
    let mut block = [0u8; 4096];
    let mut header_start = 0;
    let mut scan_from = 0;
    let (split, status) = loop {
        if let Some(offset) = buf[scan_from..]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            let split = scan_from + offset;
            let head = String::from_utf8_lossy(&buf[header_start..split]);
            let mut fields = head
                .lines()
                .next()
                .unwrap_or_default()
                .split_ascii_whitespace();
            let version = fields.next();
            let status = fields
                .next()
                .filter(|code| code.len() == 3)
                .and_then(http_decimal)
                .filter(|code| (100..600).contains(code) && *code != 101)
                .ok_or_else(|| EngineError::Failed("invalid HTTP status".into()))?
                as u16;
            if !matches!(version, Some("HTTP/1.0" | "HTTP/1.1")) {
                return Err(EngineError::Failed("invalid HTTP version".into()));
            }
            if status < 200 {
                // Interim responses have no body; the final headers may already
                // share this read. Keep one cumulative cap for the entire chain.
                header_start = split + 4;
                scan_from = header_start;
                continue;
            }
            break (split, status);
        }
        if buf.len() == 64 * 1024 {
            return Err(EngineError::Failed("response headers truncated".into()));
        }
        let want = block.len().min(64 * 1024 - buf.len());
        let count = match stream.read(&mut block[..want]) {
            Ok(count) => count,
            Err(err) => {
                check_job_control(job)?;
                return Err(EngineError::Failed(err.to_string()));
            }
        };
        if count == 0 {
            return Err(EngineError::Failed("response headers truncated".into()));
        }
        // 只检查新增字节及跨块边界；同次读取的正文由 Body::Tcp 的 prefix 保留。
        scan_from = buf.len().saturating_sub(3).max(header_start);
        buf.extend_from_slice(&block[..count]);
    };
    let head = String::from_utf8_lossy(&buf[header_start..split]);
    let leftover = buf[split + 4..].to_vec();
    let meta = parse_header_meta(&head)?;
    record_download_type(job, status, &head);
    let no_body = request_method(job) == "HEAD" || matches!(status, 204 | 304);
    let accept_ranges = meta.accept_ranges || meta.content_range.is_some();
    let body = Body::Tcp {
        prefix: leftover,
        at: 0,
        stream,
        remaining: if no_body {
            Some(0)
        } else {
            meta.content_length
        },
    };
    let body = if meta.chunked && !no_body {
        Body::Chunked(ChunkedReader::new(Box::new(body)))
    } else {
        body
    };
    Ok(FetchResult {
        final_url: job.url.clone(),
        status,
        location: meta.location,
        content_range: meta.content_range,
        content_length: meta.content_length,
        content_type: meta.content_type,
        content_encoding: meta.content_encoding,
        retry_after: meta.retry_after,
        etag: meta.etag,
        last_modified: meta.last_modified,
        accept_ranges,
        body,
    })
}

#[cfg(windows)]
mod winhttp {
    use super::{Body, EngineError, Job};
    use std::io::Read;
    use std::path::PathBuf;
    use std::ptr::null_mut;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetOption, WinHttpSetStatusCallback, WinHttpSetTimeouts,
        WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_ADDREQ_FLAG_ADD,
        WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE,
        WINHTTP_CALLBACK_FLAG_READ_COMPLETE, WINHTTP_CALLBACK_FLAG_REQUEST_ERROR,
        WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
        WINHTTP_CALLBACK_STATUS_REQUEST_ERROR, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
        WINHTTP_DISABLE_COOKIES, WINHTTP_FLAG_ASYNC, WINHTTP_FLAG_SECURE,
        WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, WINHTTP_OPTION_REDIRECT_POLICY,
        WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_PROTOCOL_FLAG_HTTP2,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE,
    };

    // WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY (stable WinHTTP ABI value).
    const SYSTEM_PROXY_ACCESS_TYPE: u32 = 4;

    #[derive(Default)]
    struct RequestStatus {
        completed: Option<Result<u32, u32>>,
        closed: bool,
    }

    #[derive(Default)]
    struct RequestSignal {
        status: Mutex<RequestStatus>,
        pulse: Condvar,
    }

    unsafe extern "system" fn status_callback(
        _handle: *mut core::ffi::c_void,
        context: usize,
        notification: u32,
        information: *mut core::ffi::c_void,
        length: u32,
    ) {
        if context == 0 {
            return;
        }
        let signal = &*(context as *const RequestSignal);
        {
            let mut status = signal
                .status
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match notification {
                WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE
                | WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => {
                    status.completed = Some(Ok(0));
                }
                WINHTTP_CALLBACK_STATUS_READ_COMPLETE => status.completed = Some(Ok(length)),
                WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => {
                    status.completed =
                        Some(Err((*(information as *const WINHTTP_ASYNC_RESULT)).dwError));
                }
                WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING => status.closed = true,
                _ => return,
            }
            signal.pulse.notify_all();
        }
        if notification == WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING {
            // WinHTTP 的最后一次通知释放其专用引用；请求所有者另持一份引用。
            drop(Arc::from_raw(context as *const RequestSignal));
        }
    }

    pub struct WinHttpBody {
        connect: *mut core::ffi::c_void,
        request: *mut core::ffi::c_void,
        recycle_connect: bool,
        connect_key: String,
        signal: Option<Arc<RequestSignal>>,
        control: PathBuf,
        pub(super) range_stop: Option<Arc<std::sync::atomic::AtomicBool>>,
    }

    unsafe impl Send for WinHttpBody {}

    impl Drop for WinHttpBody {
        fn drop(&mut self) {
            self.close_request();
            unsafe {
                if !self.connect.is_null() {
                    if self.recycle_connect && put_connect(&self.connect_key, self.connect) {
                        self.connect = null_mut();
                    } else {
                        WinHttpCloseHandle(self.connect);
                        self.connect = null_mut();
                    }
                }
            }
        }
    }

    impl WinHttpBody {
        fn close_request(&mut self) {
            if self.request.is_null() {
                return;
            }
            unsafe {
                WinHttpCloseHandle(std::mem::replace(&mut self.request, null_mut()));
            }
            if let Some(signal) = &self.signal {
                // 取消异步 I/O 后仍须等最终关闭通知，才能释放正在使用的读写缓冲区。
                let mut status = signal
                    .status
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                while !status.closed {
                    status = signal
                        .pulse
                        .wait(status)
                        .unwrap_or_else(|error| error.into_inner());
                }
            }
        }

        fn wait(&self, operation: &str) -> Result<u32, EngineError> {
            let signal = self.signal.as_ref().expect("registered WinHTTP callback");
            let mut status = signal
                .status
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            loop {
                if let Some(result) = status.completed.take() {
                    return result
                        .map_err(|code| EngineError::Failed(format!("{operation} {code}")));
                }
                // 已完成的读直接返回；仅网络仍在等待时检查控制，且不持回调锁做文件 I/O。
                drop(status);
                super::check_transfer_control(&self.control, self.range_stop.as_deref())?;
                status = signal
                    .status
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if status.completed.is_none() {
                    status = signal
                        .pulse
                        .wait_timeout(status, Duration::from_millis(100))
                        .unwrap_or_else(|error| error.into_inner())
                        .0;
                }
            }
        }
    }

    impl Read for WinHttpBody {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let ok = unsafe {
                WinHttpReadData(
                    self.request,
                    buf.as_mut_ptr() as *mut _,
                    buf.len().min(u32::MAX as usize) as u32,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::other(format!(
                    "WinHttpReadData {}",
                    unsafe { GetLastError() }
                )));
            }
            match self.wait("WinHttpReadData") {
                Ok(got) => Ok(got as usize),
                Err(error) => {
                    self.recycle_connect = false;
                    self.close_request();
                    Err(std::io::Error::other(error.to_string()))
                }
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn cached_session(proxy: &str) -> Result<*mut core::ffi::c_void, EngineError> {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut map = cache
            .lock()
            .map_err(|_| EngineError::Failed("winhttp session lock".into()))?;
        if let Some(handle) = map.get(proxy) {
            return Ok(*handle as *mut core::ffi::c_void);
        }
        unsafe {
            let session = match super::proxy_route(proxy) {
                super::ProxyRoute::Direct => WinHttpOpen(
                    wide(super::CHROME_UA).as_ptr(),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    null_mut(),
                    null_mut(),
                    WINHTTP_FLAG_ASYNC,
                ),
                super::ProxyRoute::System => WinHttpOpen(
                    wide(super::CHROME_UA).as_ptr(),
                    SYSTEM_PROXY_ACCESS_TYPE,
                    null_mut(),
                    null_mut(),
                    WINHTTP_FLAG_ASYNC,
                ),
                super::ProxyRoute::Manual(proxy_url) => WinHttpOpen(
                    wide(super::CHROME_UA).as_ptr(),
                    WINHTTP_ACCESS_TYPE_NAMED_PROXY,
                    wide(proxy_url).as_ptr(),
                    wide("").as_ptr(),
                    WINHTTP_FLAG_ASYNC,
                ),
            };
            if session.is_null() {
                return Err(EngineError::Failed(format!(
                    "WinHttpOpen {}",
                    GetLastError()
                )));
            }
            let _ = WinHttpSetTimeouts(session, 15_000, 15_000, 30_000, 60_000);
            let mut http2 = WINHTTP_PROTOCOL_FLAG_HTTP2;
            let _ = WinHttpSetOption(
                session,
                WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
                &mut http2 as *mut u32 as *mut _,
                std::mem::size_of_val(&http2) as u32,
            );
            map.insert(proxy.to_string(), session as usize);
            Ok(session)
        }
    }

    fn connect_pool() -> &'static std::sync::Mutex<super::IdleHandlePool> {
        use std::sync::{Mutex, OnceLock};
        static POOL: OnceLock<Mutex<super::IdleHandlePool>> = OnceLock::new();
        POOL.get_or_init(|| Mutex::new(super::IdleHandlePool::default()))
    }

    fn take_connect(key: &str) -> Option<*mut core::ffi::c_void> {
        connect_pool()
            .lock()
            .ok()
            .and_then(|mut pool| pool.take(key))
            .map(|handle| handle as *mut core::ffi::c_void)
    }

    fn put_connect(key: &str, handle: *mut core::ffi::c_void) -> bool {
        if handle.is_null() {
            return false;
        }
        connect_pool()
            .lock()
            .ok()
            .is_some_and(|mut pool| pool.put(key.to_string(), handle as usize))
    }

    pub fn get(
        job: &Job,
        url: &str,
        range: Option<&str>,
        destinations: &[std::net::SocketAddr],
    ) -> Result<super::FetchResult, EngineError> {
        let mut addresses = destinations.iter().map(|addr| addr.ip());
        let mut result = get_at(job, url, range, addresses.next());
        for address in addresses {
            // An unreachable address failed before a request was delivered.
            if !matches!(&result, Err(EngineError::Failed(message)) if message == "WinHttpSendRequest 12029")
            {
                break;
            }
            result = get_at(job, url, range, Some(address));
        }
        result
    }

    fn get_at(
        job: &Job,
        url: &str,
        range: Option<&str>,
        address: Option<std::net::IpAddr>,
    ) -> Result<super::FetchResult, EngineError> {
        super::check_job_control(job)?;
        let parsed = super::parse_http_url(url)?;
        // 必须先于 body 声明，错误返回时先取消并关闭请求，再释放异步发送缓冲区。
        let mut request_body = super::post_body(job)?;
        let request_body_len = u32::try_from(request_body.len())
            .map_err(|_| EngineError::Failed("HTTP request body exceeds WinHTTP limit".into()))?;
        unsafe {
            let proxy = job.proxy.trim();
            let session = cached_session(proxy)?;
            let mut connect_key = super::origin_connect_key(proxy, &parsed.host, parsed.port);
            if let Some(ip) = address {
                connect_key.push_str(&format!("|{ip}"));
            }
            let connect = take_connect(&connect_key).unwrap_or_else(|| {
                WinHttpConnect(
                    session,
                    wide(&super::uri_host(&parsed.host)).as_ptr(),
                    parsed.port,
                    0,
                )
            });
            if connect.is_null() {
                return Err(EngineError::Failed(format!(
                    "WinHttpConnect {}",
                    GetLastError()
                )));
            }
            let flags = if parsed.https { WINHTTP_FLAG_SECURE } else { 0 };
            let request = WinHttpOpenRequest(
                connect,
                wide(super::request_method(job)).as_ptr(),
                wide(&parsed.path).as_ptr(),
                null_mut(),
                null_mut(),
                null_mut(),
                flags,
            );
            if request.is_null() {
                WinHttpCloseHandle(connect);
                return Err(EngineError::Failed(format!(
                    "WinHttpOpenRequest {}",
                    GetLastError()
                )));
            }
            let mut body = WinHttpBody {
                connect,
                request,
                recycle_connect: false,
                connect_key,
                signal: None,
                control: job.control.clone(),
                range_stop: job.range_stop.clone(),
            };
            let previous = WinHttpSetStatusCallback(
                request,
                Some(status_callback),
                WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE
                    | WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE
                    | WINHTTP_CALLBACK_FLAG_READ_COMPLETE
                    | WINHTTP_CALLBACK_FLAG_REQUEST_ERROR
                    | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
                0,
            );
            if previous.is_some_and(|callback| callback as usize == usize::MAX) {
                return Err(EngineError::Failed(format!(
                    "WinHttpSetStatusCallback {}",
                    GetLastError()
                )));
            }
            if let Some(address) = address {
                let mut name = wide(&address.to_string());
                if WinHttpSetOption(
                    request,
                    windows_sys::Win32::Networking::WinHttp::WINHTTP_OPTION_RESOLUTION_HOSTNAME,
                    name.as_mut_ptr().cast(),
                    ((name.len() - 1) * 2) as u32,
                ) == 0
                {
                    return Err(EngineError::Failed(format!(
                        "WinHttpSetOption resolution hostname {}",
                        GetLastError()
                    )));
                }
            }
            let signal = Arc::new(RequestSignal::default());
            let mut context = Arc::into_raw(Arc::clone(&signal)) as usize;
            if WinHttpSetOption(
                request,
                WINHTTP_OPTION_CONTEXT_VALUE,
                &mut context as *mut _ as *mut _,
                std::mem::size_of_val(&context) as u32,
            ) == 0
            {
                let error = GetLastError();
                drop(Arc::from_raw(context as *const RequestSignal));
                return Err(EngineError::Failed(format!(
                    "WinHttpSetOption context {error}"
                )));
            }
            body.signal = Some(signal);
            // Cached sessions span tasks. Only the current task may supply cookies.
            let mut disabled = WINHTTP_DISABLE_COOKIES;
            if WinHttpSetOption(
                request,
                WINHTTP_OPTION_DISABLE_FEATURE,
                &mut disabled as *mut _ as *mut _,
                std::mem::size_of_val(&disabled) as u32,
            ) == 0
            {
                return Err(EngineError::Failed(format!(
                    "WinHttpSetOption cookies {}",
                    GetLastError()
                )));
            }
            let mut policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
            if WinHttpSetOption(
                request,
                WINHTTP_OPTION_REDIRECT_POLICY,
                &mut policy as *mut _ as *mut _,
                std::mem::size_of_val(&policy) as u32,
            ) == 0
            {
                let err = GetLastError();
                return Err(EngineError::Failed(format!(
                    "WinHttpSetOption redirect {err}"
                )));
            }
            let mut http2 = WINHTTP_PROTOCOL_FLAG_HTTP2;
            let _ = WinHttpSetOption(
                request,
                WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
                &mut http2 as *mut u32 as *mut _,
                std::mem::size_of_val(&http2) as u32,
            );
            let mut extra = String::from("Accept-Encoding: identity\r\n");
            if let Some(range) = range {
                extra.push_str(&format!("Range: {range}\r\n"));
                let validator = super::range_validator(job);
                if !validator.is_empty() {
                    extra.push_str(&format!("If-Range: {validator}\r\n"));
                }
            }
            for (key, value) in &job.headers {
                if !super::header_allowed_on_request(key, value) {
                    continue;
                }
                extra.push_str(&format!("{key}: {value}\r\n"));
            }
            if WinHttpAddRequestHeaders(
                request,
                wide(&extra).as_ptr(),
                extra.encode_utf16().count() as u32,
                WINHTTP_ADDREQ_FLAG_ADD,
            ) == 0
            {
                return Err(EngineError::Failed(format!(
                    "WinHttpAddRequestHeaders {}",
                    GetLastError()
                )));
            }
            let request_body_ptr = if request_body.is_empty() {
                null_mut()
            } else {
                request_body.as_mut_ptr() as *mut _
            };
            if WinHttpSendRequest(
                request,
                null_mut(),
                0,
                request_body_ptr,
                request_body_len,
                request_body_len,
                context,
            ) == 0
            {
                let err = GetLastError();
                return Err(EngineError::Failed(format!("WinHttpSendRequest {err}")));
            }
            body.wait("WinHttpSendRequest")?;
            if WinHttpReceiveResponse(request, null_mut()) == 0 {
                let err = GetLastError();
                return Err(EngineError::Failed(format!("WinHttpReceiveResponse {err}")));
            }
            body.wait("WinHttpReceiveResponse")?;
            let mut status: u32 = 0;
            let mut status_size = std::mem::size_of::<u32>() as u32;
            if WinHttpQueryHeaders(
                request,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null_mut(),
                &mut status as *mut u32 as *mut _,
                &mut status_size,
                null_mut(),
            ) == 0
            {
                let err = GetLastError();
                return Err(EngineError::Failed(format!("WinHttpQueryHeaders {err}")));
            }
            let raw_headers = query_header_string(request, WINHTTP_QUERY_RAW_HEADERS_CRLF)
                .ok_or_else(|| EngineError::Failed("missing WinHTTP response headers".into()))?;
            let meta = super::parse_header_meta(&raw_headers)?;
            super::record_download_type(job, status as u16, &raw_headers);
            let accept_ranges = meta.accept_ranges || meta.content_range.is_some();
            body.recycle_connect = true;
            Ok(super::FetchResult {
                final_url: url.to_string(),
                status: status as u16,
                location: meta.location,
                content_range: meta.content_range,
                content_length: meta.content_length,
                content_type: meta.content_type,
                content_encoding: meta.content_encoding,
                retry_after: meta.retry_after,
                etag: meta.etag,
                last_modified: meta.last_modified,
                accept_ranges,
                body: Body::WinHttp(body),
            })
        }
    }

    fn query_header_string(request: *mut core::ffi::c_void, query: u32) -> Option<String> {
        unsafe {
            let mut size: u32 = 0;
            WinHttpQueryHeaders(
                request,
                query,
                null_mut(),
                null_mut(),
                &mut size,
                null_mut(),
            );
            if size == 0 {
                return None;
            }
            let mut buf = vec![0u16; (size as usize / 2).saturating_add(1)];
            let mut actual = (buf.len() * 2) as u32;
            if WinHttpQueryHeaders(
                request,
                query,
                null_mut(),
                buf.as_mut_ptr() as *mut _,
                &mut actual,
                null_mut(),
            ) == 0
            {
                return None;
            }
            let end = buf.iter().position(|&ch| ch == 0).unwrap_or(buf.len());
            let text = String::from_utf16_lossy(&buf[..end]);
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn serve_redirect_test_responses(
    responses: Vec<String>,
) -> (String, thread::JoinHandle<Vec<String>>) {
    use std::io::BufRead;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        responses
            .into_iter()
            .map(|response| {
                let (mut socket, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&mut socket);
                let mut head = String::new();
                while !head.ends_with("\r\n\r\n") {
                    assert_ne!(reader.read_line(&mut head).unwrap(), 0);
                }
                socket.write_all(response.as_bytes()).unwrap();
                head
            })
            .collect()
    });
    (origin, server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    };

    #[test]
    fn local_proxy_receives_vaulted_http_basic_credentials() {
        let (endpoint, proxy_thread) = serve_redirect_test_responses(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into(),
        ]);
        let mut store = crate::CoreStore::in_memory().unwrap();
        let raw = endpoint.replacen("://", "://alice:secret@", 1);
        store.set_setting("proxy_url", &raw).unwrap();
        let clean = store.setting_string("proxy_url", "").unwrap();
        let proxy = store.hydrate_proxy(&clean).unwrap();
        let (status, body) =
            fetch_bytes_limited("http://127.0.0.1:1/file", &HashMap::new(), &proxy, 16).unwrap();
        assert_eq!((status, body.as_slice()), (200, b"ok".as_slice()));
        let request = proxy_thread.join().unwrap().remove(0);
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Proxy-Authorization: Basic YWxpY2U6c2VjcmV0")));
    }

    fn post_test_job(url: String, body_path: PathBuf) -> Job {
        Job {
            url,
            headers: HashMap::from([(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )]),
            output: PathBuf::new(),
            connections: 1,
            chunk_bytes: 1,
            total: 0,
            sequential: true,
            resume_from: 0,
            proxy: crate::net_policy::DIRECT_PROXY_SENTINEL.into(),
            resource_key: String::new(),
            etag: String::new(),
            last_modified: String::new(),
            control: PathBuf::new(),
            progress: PathBuf::new(),
            method: "POST".into(),
            body_path,
            mirrors: Vec::new(),
            replay_json: String::new(),
            curl_body_limit: None,
            range_stop: None,
            installer_download: false,
            origin_boundary: String::new(),
        }
    }

    #[test]
    fn installer_transfer_rejects_untrusted_destination_before_opening_connection() {
        let mut job = post_test_job(
            "https://github.com/ciaooo55/hls-downloader/asset".into(),
            PathBuf::new(),
        );
        job.installer_download = true;
        for next in [
            "http://github.com/ciaooo55/hls-downloader/asset",
            "https://cdn.invalid/asset",
        ] {
            let result = fetch_follow_restricted(&job, None, None, Some(next));
            assert!(
                matches!(result, Err(EngineError::Failed(error)) if error.contains("Installer redirect"))
            );
        }
    }

    #[test]
    fn public_entry_rejects_private_redirect_targets_before_connecting() {
        let job = post_test_job("https://203.0.113.20/entry".into(), PathBuf::new());
        let base = parse_http_url(&job.url).unwrap();
        for host in [
            "127.0.0.1",
            "127.9.8.7",
            "169.254.169.254",
            "[::1]",
            "10.0.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "localhost.",
            "[fe80::1]",
            "[fd01::1]",
            "0.0.0.0",
            "[::]",
            "[::ffff:10.0.0.1]",
            "127.1",
            "0x7f000001",
        ] {
            let next = resolve_location(&base, &format!("https://{host}/child"));
            assert!(
                matches!(fetch_follow_restricted(&job, None, None, Some(&next)),
                Err(EngineError::Failed(message)) if message.contains("private network")),
                "{host}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn public_proxy_response_cannot_redirect_to_loopback() {
        let entry = "http://203.0.113.20/entry";
        for via_curl in [false, true] {
            if via_curl && curl_exe_for(entry).is_none() {
                continue;
            }
            let (proxy, server) = serve_redirect_test_responses(vec![
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0\r\n\r\n".into(),
            ]);
            let mut job = post_test_job(entry.into(), PathBuf::new());
            job.method = "GET".into();
            job.proxy = proxy;
            let result = if via_curl {
                fetch_curl_follow(&job, None).unwrap()
            } else {
                fetch_follow(&job, None)
            };
            assert!(
                matches!(result, Err(EngineError::Failed(error)) if error.contains("private network"))
            );
            assert!(server.join().unwrap()[0].contains("/entry"));
        }
    }

    #[test]
    fn curl_request_args_preserve_post_body_without_inventing_content_type() {
        let mut job = post_test_job(
            "https://example.test/upload".into(),
            PathBuf::from("body.bin"),
        );
        let args = curl_request_args(&job, &job.headers);
        assert_eq!(
            args,
            vec![
                OsString::from("--request"),
                OsString::from("POST"),
                OsString::from("--data-binary"),
                OsString::from("@body.bin"),
            ]
        );
        job.headers.clear();
        let args = curl_request_args(&job, &job.headers);
        assert_eq!(args[4], OsString::from("-H"));
        assert_eq!(args[5], OsString::from("Content-Type:"));
        job.body_path.clear();
        let args = curl_request_args(&job, &job.headers);
        assert_eq!(args[3], OsString::new());
    }

    #[cfg(windows)]
    #[test]
    fn winhttp_posts_the_job_body_bytes() {
        use std::net::TcpListener;
        use std::sync::mpsc;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, received) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap() == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            let split = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let head = String::from_utf8_lossy(&request[..split]).to_string();
            let length = head
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            let mut body = request[split + 4..].to_vec();
            while body.len() < length {
                let mut buf = vec![0u8; length - body.len()];
                let got = stream.read(&mut buf).unwrap();
                if got == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..got]);
            }
            sent.send((head, body)).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("hls-winhttp-post-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let body_path = dir.join("request.bin");
        fs::write(&body_path, b"alpha=1&beta=two").unwrap();
        let job = post_test_job(format!("http://{address}/upload"), body_path);
        let fetched = winhttp::get(&job, &job.url, None, &[]).unwrap();
        assert_eq!(fetched.status, 200);
        drop(fetched);
        let (head, body) = received.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(head.starts_with("POST /upload HTTP/1.1"));
        assert_eq!(body, b"alpha=1&beta=two");
        server.join().unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn origin_connect_pool_does_not_share_live_handles() {
        let mut pool = IdleHandlePool::default();
        assert_eq!(origin_connect_key("p", "cdn.test", 443), "p|cdn.test|443");
        assert!(pool.take("a").is_none());
        assert!(pool.put("a".into(), 11));
        assert!(pool.put("a".into(), 12));
        assert_eq!(pool.take("a"), Some(12));
        assert_eq!(pool.take("a"), Some(11));
        assert!(pool.take("a").is_none());
        assert!(
            pool.idle.is_empty(),
            "checked-out origins must not accumulate"
        );
        assert!(!pool.put("a".into(), 0));
        for index in 1..=CONNECT_POOL_PER_KEY {
            assert!(pool.put("full".into(), index));
        }
        assert!(!pool.put("full".into(), 99));
        for index in 0..1024 {
            let key = format!("origin-{index}");
            assert!(pool.put(key.clone(), 100));
            assert_eq!(pool.take(&key), Some(100));
        }
        assert_eq!(pool.idle.len(), 1, "only the full idle bucket remains");
    }

    #[test]
    fn hls_byte_range_fetch_validates_and_returns_only_the_requested_bytes() {
        let body = b"0123456789";
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_capped_ranges(body, 64, Arc::clone(&seen));
        let bytes = fetch_hls_bytes_range(&url, &HashMap::new(), "", 3, 4).unwrap();
        assert_eq!(bytes, b"3456");
        assert!(seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&3));
    }

    #[test]
    fn ranged_file_download_rejects_whole_files_wrong_bounds_and_incomplete_bodies() {
        for (case, response, untouched) in [
            ("ignored", "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nABCD", true),
            ("missing-range", "HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\n\r\nABCD", true),
            ("wrong-range", "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 1-4/10\r\nContent-Length: 4\r\n\r\nABCD", true),
            ("wrong-length", "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-5/10\r\nContent-Length: 5\r\n\r\nABCDE", true),
            ("changed-etag", "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-5/10\r\nETag: \"other\"\r\nContent-Length: 4\r\n\r\nABCD", true),
            ("encoded", "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-5/10\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\n\r\nABCD", true),
            ("truncated", "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-5/10\r\nContent-Length: 4\r\n\r\nABC", false),
            ("oversized", "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-5/10\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nABCDE\r\n0\r\n\r\n", false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/media", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" { break; }
                }
                socket.write_all(response.as_bytes()).unwrap();
            });
            let (job, dir) = temp_job(&url, false, 1, 4);
            fs::write(&job.output, b"keep").unwrap();
            let result = download_bytes_range(&job, 2, 4);
            server.join().unwrap();
            assert!(result.is_err(), "{case}: {result:?}");
            let bytes = fs::read(&job.output).unwrap();
            if untouched { assert_eq!(bytes, b"keep", "{case}"); }
            else { assert!(bytes.len() <= 4, "{case}"); }
            fs::remove_dir_all(dir).unwrap();
        }
        let (job, dir) = temp_job("http://127.0.0.1:1/", false, 1, 4);
        fs::write(&job.output, b"keep").unwrap();
        fs::write(&job.control, "pause").unwrap();
        assert!(matches!(
            download_bytes_range(&job, 0, 4),
            Err(EngineError::Pause)
        ));
        assert!(matches!(
            download_bytes_range(&job, 0, 0),
            Err(EngineError::RangeUnsupported(_))
        ));
        assert!(matches!(
            download_bytes_range(&job, u64::MAX, 2),
            Err(EngineError::RangeUnsupported(_))
        ));
        assert_eq!(fs::read(&job.output).unwrap(), b"keep");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hls_byte_range_rejects_oversize_body_before_eof() {
        for (use_proxy, declared_length) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            if use_proxy && !cfg!(windows) {
                continue;
            }
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let proxy = if use_proxy {
                url.clone()
            } else {
                String::new()
            };
            let (sent_tx, sent_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let framing = if declared_length {
                    "Content-Length: 64\r\n"
                } else {
                    "Transfer-Encoding: chunked\r\n"
                };
                let _ = write!(stream, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-7/8\r\n{framing}Connection: close\r\n\r\n");
                let _ = stream.write_all(if declared_length {
                    b"123456789"
                } else {
                    b"9\r\n123456789\r\n"
                });
                sent_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                if !declared_length {
                    let _ = stream.write_all(b"0\r\n\r\n");
                }
            });
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let client = thread::spawn(move || {
                let result = fetch_hls_bytes_range(&url, &HashMap::new(), &proxy, 0, 8);
                done_tx.send(result).unwrap();
            });
            sent_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let result = done_rx.recv_timeout(Duration::from_secs(3));
            release_tx.send(()).unwrap();
            server.join().unwrap();
            client.join().unwrap();
            let error = result
                .expect("oversized body must be rejected before server EOF")
                .unwrap_err();
            assert!(error.to_string().contains("byte range"), "{error}");
        }
    }

    #[test]
    fn active_split_defaults_on_only_for_validated_resources() {
        let (mut job, dir) = temp_job("http://127.0.0.1/", false, 1, 2);
        for (etag, modified, enabled) in [
            ("\"v1\"", "", true),
            (" W/\"v1\" ", "", false),
            ("w/\"v1\"", "", false),
            ("", "", false),
            ("", "Wed, 21 Oct 2015 07:28:00 GMT", true),
            ("W/\"v1\"", "Wed, 21 Oct 2015 07:28:00 GMT", true),
            ("\"v1\"\r\nInjected: yes", "", false),
            ("", "bad\nvalue", false),
        ] {
            job.etag = etag.into();
            job.last_modified = modified.into();
            for (setting, allowed) in [
                (None, true),
                (Some("1"), true),
                (Some("other"), true),
                (Some(""), true),
                (Some("0"), false),
                (Some("false"), false),
                (Some("off"), false),
            ] {
                assert_eq!(
                    active_split_minimum(&job, setting),
                    (enabled && allowed).then_some(256 * 1024),
                    "etag={etag:?}, modified={modified:?}, setting={setting:?}"
                );
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn active_split_selects_largest_safe_tail_after_pending_and_connection_gates() {
        let mut scheduler = RangeScheduler::new(
            vec![
                WorkRange {
                    start: 0,
                    end: 1999,
                },
                WorkRange {
                    start: 2000,
                    end: 2999,
                },
            ],
            3,
        );
        scheduler.split_minimum = Some(100);
        let first = scheduler.claim(0).unwrap();
        {
            let mut p = first.progress.lock().unwrap();
            p.splittable = true;
            p.cursor = 100;
            p.reserved_until = 1700;
        }
        // 尽管原范围可以拆分，先领取待办；尚未获得连接资格的 worker 不能拆分。
        let second = scheduler.claim(1).unwrap();
        {
            let mut p = second.progress.lock().unwrap();
            assert_eq!((p.cursor, p.stop), (2000, 2999));
            p.splittable = true;
            p.cursor = 2100;
            p.reserved_until = 2200;
        }
        assert!(scheduler.claim(2).is_none());
        scheduler.note_success();
        scheduler.note_success();
        let tail = scheduler.claim(2).unwrap();
        let p = tail.progress.lock().unwrap();
        assert_eq!((p.cursor, p.stop), (2600, 2999));
        assert_eq!(second.progress.lock().unwrap().stop, 2599);
        assert_eq!(first.progress.lock().unwrap().stop, 1999);
    }

    #[test]
    fn active_split_preserves_reserved_read_and_never_overlaps_tail() {
        let mut scheduler = RangeScheduler::new(vec![WorkRange { start: 0, end: 999 }], 2);
        scheduler.split_minimum = Some(100);
        let original = scheduler.claim(0).unwrap();
        assert!(
            scheduler.claim(1).is_none(),
            "a response is not splittable before identity and transport validation"
        );
        {
            let mut progress = original.progress.lock().unwrap();
            progress.splittable = true;
            progress.cursor = 100;
            progress.reserved_until = 400;
        }
        let tail = scheduler.claim(1).unwrap();
        let first = original.progress.lock().unwrap();
        let second = tail.progress.lock().unwrap();
        assert_eq!((first.cursor, first.stop), (100, 699));
        assert_eq!((second.cursor, second.stop), (700, 999));
        assert!(first.stop >= first.reserved_until);
        assert_eq!(first.stop + 1, second.cursor);
        assert_eq!(
            second.stop - first.cursor + 1,
            (first.stop - first.cursor + 1) + (second.stop - second.cursor + 1)
        );
        drop(second);
        drop(first);
        scheduler.complete(tail.id);
        {
            let mut p = original.progress.lock().unwrap();
            // 已结束的读操作可能仍保留较大的预留区；下一次 fetch 必须原子重置。
            assert_eq!(p.begin_fetch(), (100, 699));
            assert_eq!(p.reserved_until, p.cursor);
            assert!(!p.splittable);
        }
        assert!(
            scheduler.claim(1).is_none(),
            "fetch/validation must not be split"
        );
        {
            let mut p = original.progress.lock().unwrap();
            // 新流式响应校验成功后重新开放，并重新预留正在读/写的区间。
            p.splittable = true;
            p.reserved_until = 400;
        }
        let tail = scheduler.claim(1).unwrap();
        let p = tail.progress.lock().unwrap();
        assert_eq!((p.cursor, p.stop), (550, 699));
        drop(p);
        let p = original.progress.lock().unwrap();
        assert_eq!(p.stop, 549);
        assert!(p.stop >= p.reserved_until);
        drop(p);
        scheduler.complete(tail.id);
        assert!(
            scheduler.claim(1).is_none(),
            "both halves must meet the minimum"
        );
    }

    #[test]
    fn durable_checkpoints_are_paced_but_bounded() {
        let mib = 1024 * 1024;
        let never = || panic!("playback must not be probed below the byte threshold");
        assert!(!durable_checkpoint_due(
            DURABLE_CHECKPOINT_BYTES - 1,
            Duration::from_secs(60),
            never
        ));
        // 字节门槛已到但距上次提交太近：不提交，除非处于边播边下。
        assert!(!durable_checkpoint_due(
            8 * mib,
            Duration::from_millis(100),
            || false
        ));
        assert!(durable_checkpoint_due(
            8 * mib,
            Duration::from_millis(100),
            || true
        ));
        assert!(durable_checkpoint_due(
            8 * mib,
            DURABLE_CHECKPOINT_INTERVAL,
            || false
        ));
        // 未同步量的硬上限不受时间门槛影响。
        assert!(durable_checkpoint_due(
            DURABLE_CHECKPOINT_MAX_BYTES,
            Duration::ZERO,
            || false
        ));
        let checkpoints = RangeCheckpoints::new(Vec::new());
        thread::sleep(Duration::from_millis(5));
        assert!(checkpoints.since_last_commit() >= Duration::from_millis(5));
        checkpoints.mark_committed();
        assert!(checkpoints.since_last_commit() < Duration::from_millis(5));
    }

    #[test]
    fn range_retry_delay_keeps_exponential_floor_and_bounded_jitter() {
        for (attempt, base) in [(1u32, 200u64), (2, 400), (3, 800), (4, 1600), (9, 1600)] {
            assert_eq!(range_retry_delay_ms(attempt, 0), base);
            assert_eq!(range_retry_delay_ms(attempt, base / 2), base + base / 2);
            for seed in [1u64, 77, u64::MAX, retry_jitter_seed()] {
                let delay = range_retry_delay_ms(attempt, seed);
                assert!(
                    (base..=base + base / 2).contains(&delay),
                    "{attempt} {delay}"
                );
            }
        }
        let seeds: std::collections::HashSet<u64> = (0..8).map(|_| retry_jitter_seed()).collect();
        assert!(seeds.len() > 1, "jitter seed must vary between calls");
    }

    #[test]
    fn adaptive_connections_ramp_gradually_and_back_off_immediately() {
        let controller = AdaptiveConnectionController::new(12);
        assert_eq!(controller.desired(), 2);

        // Healthy transfers ramp +2 every 2 successes and reach 12 in 10 completed ranges.
        for expected in [4, 6, 8, 10, 12] {
            assert!(!controller.note_success());
            assert!(controller.note_success());
            assert_eq!(controller.desired(), expected);
        }
        assert!(!controller.note_success());
        assert_eq!(controller.desired(), 12);

        // Congestion halves immediately (div_ceil(2).max(1)) and resets success count.
        controller.note_congestion();
        assert_eq!(controller.desired(), 6);
        controller.note_congestion();
        assert_eq!(controller.desired(), 3);
        controller.note_congestion();
        assert_eq!(controller.desired(), 2);
        controller.note_success();
        assert_eq!(controller.desired(), 2);
        controller.note_success();
        assert_eq!(controller.desired(), 4);
    }

    #[test]
    fn adaptive_scheduler_parks_excess_workers_without_duplicate_ranges() {
        let scheduler = RangeScheduler::new(
            vec![
                WorkRange { start: 0, end: 9 },
                WorkRange { start: 10, end: 19 },
                WorkRange { start: 20, end: 29 },
                WorkRange { start: 30, end: 39 },
            ],
            4,
        );
        let first = scheduler.claim(0).expect("first worker should run");
        let second = scheduler.claim(1).expect("second worker should run");
        assert!(scheduler.claim(2).is_none());

        let first_range = {
            let state = first.progress.lock().unwrap_or_else(|err| err.into_inner());
            (state.cursor, state.stop)
        };
        let second_range = {
            let state = second
                .progress
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            (state.cursor, state.stop)
        };
        assert_ne!(first_range, second_range);

        scheduler.complete(first.id);
        scheduler.note_success();
        scheduler.complete(second.id);
        scheduler.note_success();
        assert_eq!(scheduler.connections.desired(), 4);

        let third = scheduler
            .claim(2)
            .expect("third worker should run after successful ramp");
        let third_range = {
            let state = third.progress.lock().unwrap_or_else(|err| err.into_inner());
            (state.cursor, state.stop)
        };
        assert_ne!(third_range, first_range);
        assert_ne!(third_range, second_range);
        scheduler.complete(third.id);
    }

    #[test]
    fn playback_seek_splits_pending_range_without_losing_or_overlapping_bytes() {
        let scheduler = RangeScheduler::new(
            vec![
                WorkRange { start: 0, end: 99 },
                WorkRange {
                    start: 200,
                    end: 299,
                },
                WorkRange {
                    start: 100,
                    end: 199,
                },
            ],
            2,
        );
        scheduler.playback_priority.store(25, Ordering::Relaxed);
        let target = scheduler.claim(0).unwrap();
        let target_range = {
            let state = target.progress.lock().unwrap();
            (state.cursor, state.stop)
        };
        assert_eq!(target_range, (25, 99));
        scheduler.complete(target.id);

        // 再次跳转后，两个前缀都必须回到普通调度；相同长度按起点先后领取。
        scheduler.playback_priority.store(125, Ordering::Relaxed);
        let mut ranges = vec![target_range];
        for expected in [(125, 199), (200, 299), (0, 24), (100, 124)] {
            let next = scheduler.claim(1).unwrap();
            let state = next.progress.lock().unwrap();
            let actual = (state.cursor, state.stop);
            assert_eq!(actual, expected);
            ranges.push(actual);
            drop(state);
            scheduler.complete(next.id);
        }
        ranges.sort_unstable();
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, 299);
        assert!(ranges.windows(2).all(|pair| pair[0].1 + 1 == pair[1].0));
        assert!(scheduler.claim(0).is_none());
    }

    #[test]
    fn adaptive_connections_respect_single_connection_limit() {
        let controller = AdaptiveConnectionController::new(1);
        assert_eq!(controller.desired(), 1);
        for _ in 0..8 {
            controller.note_success();
            controller.note_congestion();
        }
        assert_eq!(controller.desired(), 1);
    }

    #[test]
    fn slow_payload_writes_back_off_without_affecting_fast_writes() {
        let scheduler = RangeScheduler::new(Vec::new(), 8);
        scheduler.connections.desired.store(8, Ordering::Release);
        scheduler.note_storage_latency(Duration::from_millis(10));
        assert_eq!(scheduler.connections.desired(), 8);
        for expected in [4, 2, 1] {
            scheduler.note_storage_latency(Duration::from_millis(300));
            assert_eq!(scheduler.connections.desired(), expected);
        }
        scheduler.note_congestion();
        assert_eq!(scheduler.connections.desired(), 1);
        scheduler.note_storage_latency(Duration::from_secs(1));
        assert_eq!(scheduler.connections.desired(), 1);
        let single = RangeScheduler::new(Vec::new(), 1);
        single.note_storage_latency(Duration::from_secs(1));
        assert_eq!(single.connections.desired(), 1);
    }

    static NEXT_TEMP_JOB_ID: AtomicUsize = AtomicUsize::new(0);

    fn serve_body(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                        range = Some(value.trim().to_string());
                    }
                }
                let mut stream = reader.into_inner();
                if let Some(value) = range {
                    let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                    let (start_text, end_text) = spec.split_once('-').unwrap();
                    let start: usize = start_text.parse().unwrap();
                    let end: usize = if end_text.is_empty() {
                        body.len() - 1
                    } else {
                        end_text.parse().unwrap()
                    };
                    let actual_end = end.min(body.len() - 1);
                    let slice = &body[start..=actual_end];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{actual_end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len(),
                        slice.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(slice);
                } else {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                }
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn temp_job(url: &str, sequential: bool, total: u64, connections: usize) -> (Job, PathBuf) {
        let id = NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("hls-http-engine-{}-{}", std::process::id(), id));
        fs::create_dir_all(&dir).unwrap();
        let output = dir.join("payload.downloading");
        let control = dir.join("control");
        let progress = dir.join("progress.json");
        fs::write(&control, "run").unwrap();
        (
            Job {
                url: url.to_string(),
                headers: HashMap::new(),
                output,
                connections,
                chunk_bytes: 16,
                total,
                sequential,
                resume_from: 0,
                proxy: String::new(),
                resource_key: url.to_string(),
                etag: "\"native-test\"".to_string(),
                last_modified: String::new(),
                control,
                progress,
                method: String::new(),
                body_path: PathBuf::new(),
                mirrors: Vec::new(),
                replay_json: String::new(),
                curl_body_limit: None,
                range_stop: None,
                installer_download: false,
                origin_boundary: String::new(),
            },
            dir,
        )
    }

    #[test]
    fn idle_range_worker_does_not_require_payload() {
        let (job, dir) = temp_job("http://127.0.0.1:1", false, 4, 4);
        let failed = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let result = range_worker(
            2,
            &job,
            Arc::new(RangeScheduler::new(Vec::new(), 4)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(RangeCheckpoints::new(Vec::new())),
            Arc::clone(&failed),
            Arc::clone(&stop),
            true,
        );
        let error = failed.lock().unwrap().take();
        fs::remove_dir_all(dir).unwrap();
        assert!(result.is_none());
        assert!(error.is_none(), "{error:?}");
        assert!(!stop.load(Ordering::SeqCst));
    }

    #[test]
    fn range_worker_honors_control_before_opening_payload() {
        for command in ["pause", "cancel"] {
            let (job, dir) = temp_job("http://127.0.0.1:1", false, 4, 1);
            fs::write(&job.control, command).unwrap();
            let scheduler = Arc::new(RangeScheduler::new(vec![WorkRange { start: 0, end: 3 }], 1));
            let failed = Arc::new(Mutex::new(None));
            let stop = Arc::new(AtomicBool::new(false));
            let result = range_worker(
                0,
                &job,
                Arc::clone(&scheduler),
                Arc::new(AtomicU64::new(0)),
                Arc::new(RangeCheckpoints::new(Vec::new())),
                Arc::clone(&failed),
                Arc::clone(&stop),
                true,
            );
            let error = failed.lock().unwrap().take();
            fs::remove_dir_all(dir).unwrap();
            assert!(result.is_none());
            assert!(
                matches!(
                    (&error, command),
                    (Some(EngineErrorCode::Pause), "pause")
                        | (Some(EngineErrorCode::Cancel), "cancel")
                ),
                "{error:?}"
            );
            assert!(stop.load(Ordering::SeqCst));
            assert_eq!(scheduler.pending.lock().unwrap().len(), 1);
            assert!(scheduler.active.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn range_worker_open_failure_releases_claim() {
        let (job, dir) = temp_job("http://127.0.0.1:1", false, 4, 1);
        let scheduler = Arc::new(RangeScheduler::new(vec![WorkRange { start: 0, end: 3 }], 1));
        let failed = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let result = range_worker(
            0,
            &job,
            Arc::clone(&scheduler),
            Arc::new(AtomicU64::new(0)),
            Arc::new(RangeCheckpoints::new(Vec::new())),
            Arc::clone(&failed),
            Arc::clone(&stop),
            true,
        );
        let error = failed.lock().unwrap().take();
        fs::remove_dir_all(dir).unwrap();
        assert!(result.is_none());
        assert!(matches!(error, Some(EngineErrorCode::Failed(_))));
        assert!(stop.load(Ordering::SeqCst));
        assert!(
            scheduler.is_idle(),
            "failed worker retained its range claim"
        );
    }

    #[test]
    fn range_worker_stops_an_active_response_without_losing_the_terminal_reason() {
        for command in ["run", "pause", "cancel"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (job, dir) = temp_job(&url, false, 65536, 1);
            fs::write(&job.output, []).unwrap();
            let scheduler = Arc::new(RangeScheduler::new(
                vec![WorkRange {
                    start: 0,
                    end: 65535,
                }],
                1,
            ));
            let downloaded = Arc::new(AtomicU64::new(0));
            let failed = Arc::new(Mutex::new(None));
            let stop = Arc::new(AtomicBool::new(false));
            let server_stop = Arc::clone(&stop);
            let server_failed = Arc::clone(&failed);
            let control = job.control.clone();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                // The request is already active; stop before making its body available.
                if command == "run" {
                    *server_failed.lock().unwrap() =
                        Some(EngineErrorCode::Failed("peer HTTP 403".into()));
                } else {
                    fs::write(control, command).unwrap();
                }
                server_stop.store(true, Ordering::SeqCst);
                let mut stream = reader.into_inner();
                let _ = stream.write_all(b"HTTP/1.1 206 Partial Content\r\nETag: \"native-test\"\r\nContent-Range: bytes 0-65535/65536\r\nContent-Length: 65536\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(&vec![7u8; 65536]);
            });
            let response = range_worker(
                0,
                &job,
                Arc::clone(&scheduler),
                Arc::clone(&downloaded),
                Arc::new(RangeCheckpoints::new(Vec::new())),
                Arc::clone(&failed),
                Arc::clone(&stop),
                true,
            );
            server.join().unwrap();
            let error = failed.lock().unwrap().take();
            let output = fs::read(&job.output).unwrap();
            fs::remove_dir_all(dir).unwrap();
            assert!(response.is_none());
            assert!(scheduler.is_idle());
            assert_eq!(
                downloaded.load(Ordering::SeqCst),
                0,
                "{command}: stopped response was consumed"
            );
            assert!(output.is_empty(), "{command}: stopped response was written");
            assert!(
                match (&error, command) {
                    (Some(EngineErrorCode::Failed(message)), "run") => message == "peer HTTP 403",
                    (Some(EngineErrorCode::Pause), "pause")
                    | (Some(EngineErrorCode::Cancel), "cancel") => true,
                    _ => false,
                },
                "{command}: {error:?}"
            );
        }
    }

    #[test]
    fn metadata_fetch_preserves_http_errors_without_consuming_error_bodies() {
        for response in [
            "HTTP/1.1 404 Not Found\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n<html>missing key</html>",
            "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n<html>missing key</html>",
            "HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n17\r\n<html>missing key</html>\r\n0\r\n\r\n",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/key", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = reader.get_mut().write_all(response.as_bytes());
            });
            let result = fetch_hls_bytes_limited(&url, &HashMap::new(), "", 16).unwrap();
            server.join().unwrap();
            assert_eq!(result.0, 404);
            assert!(result.1.is_empty());
        }
    }

    #[test]
    fn adversarial_metadata_fetch_stops_at_the_byte_limit() {
        for (response, expected) in [
            ("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabcde", None),
            ("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabcde", None),
            ("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n0\r\n\r\n", None),
            ("HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd", Some("abcd")),
            ("HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabc", None),
            ("HTTP/1.1 304 Not Modified\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n", Some("")),
            ("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3/10\r\nContent-Length: 4\r\n\r\nabcd", None),
            ("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3/4\r\n\r\nabcd", Some("abcd")),
            ("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\n\r\nabcd", None),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = reader.get_mut().write_all(response.as_bytes());
            });
            let result = fetch_bytes_limited(&url, &HashMap::new(), "", 4);
            server.join().unwrap();
            match expected {
                Some(body) => assert_eq!(result.unwrap().1, body.as_bytes()),
                None => assert!(result.is_err(), "{response}"),
            }
        }
    }

    #[test]
    fn metadata_body_limits_preserve_head_and_not_modified_responses() {
        for (method, response, expected_body, expected_error) in [
            (
                "HEAD",
                "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n",
                "",
                "",
            ),
            (
                "GET",
                "HTTP/1.1 304 Not Modified\r\nContent-Length: 4096\r\n\r\n",
                "",
                "",
            ),
            (
                "GET",
                "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc",
                "abc",
                "",
            ),
            (
                "GET",
                "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabcde",
                "",
                "元数据超过读取上限",
            ),
            (
                "GET",
                "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabc",
                "",
                "元数据响应不完整",
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                stream.write_all(response.as_bytes()).unwrap();
            });
            let (_, dir) = temp_job(&url, true, 0, 1);
            let spec = crate::TaskSpec {
                url,
                request_method: method.into(),
                work_dir: dir.to_string_lossy().into_owned(),
                ..Default::default()
            };
            let result = read_metadata(&spec, None, 4, None);
            server.join().unwrap();
            if expected_error.is_empty() {
                assert_eq!(result.unwrap().body, expected_body.as_bytes());
            } else {
                assert!(result.err().unwrap().contains(expected_error));
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn range_workers_write_one_file_by_seek() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 3);
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sequential_writes_identity_body() {
        let body: &'static [u8] = b"one-connection-payload";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, true, 0, 1);
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn raw_http_header_boundaries_preserve_the_exact_body_and_size_limit() {
        let informational = concat!(
            "HTTP/1.1 100 Continue\r\n\r\n",
            "HTTP/1.1 103 Early Hints\r\nLink: </style.css>; rel=preload\r\n\r\n",
        );
        for prefix in ["", informational] {
            for delimiter_at in [4093, 4094, 4095, 8191, 65532, 65533] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let payload = b"body\0\r\n\r\nbytes";
                let mut response = format!(
                    "{prefix}HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"boundary\"\r\nX-Padding: ",
                    payload.len()
                )
                .into_bytes();
                response.resize(delimiter_at, b'x');
                response.extend_from_slice(b"\r\n\r\n");
                response.extend_from_slice(payload);
                let server = thread::spawn(move || {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                            break;
                        }
                    }
                    let _ = stream.write_all(&response);
                });
                let (job, dir) = temp_job(&url, true, 0, 1);
                let result = http_get(&job, &parse_http_url(&url).unwrap(), None, &[]);
                if delimiter_at == 65533 {
                    let error = result.err().expect("oversized headers must be rejected");
                    assert!(error.to_string().contains("response headers truncated"));
                } else {
                    let mut response = result.unwrap();
                    assert_eq!(response.status, 200);
                    assert_eq!(response.content_length, Some(payload.len() as u64));
                    assert_eq!(response.etag, "\"boundary\"");
                    let mut received = Vec::new();
                    response.body.read_to_end(&mut received).unwrap();
                    assert_eq!(received, payload, "delimiter at {delimiter_at}");
                }
                server.join().unwrap();
                let _ = fs::remove_dir_all(dir);
            }
        }
    }

    #[test]
    fn raw_http_requires_a_final_response_and_bounds_informational_header_chains() {
        for response in [
            "HTTP/1.1 103 Early Hints\r\n\r\n".to_string(),
            "HTTP/1.1 103 Early Hints\r\n\r\n".repeat(3000),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = reader.get_mut().write_all(response.as_bytes());
            });
            let (job, dir) = temp_job(&url, true, 0, 1);
            let result = http_get(&job, &parse_http_url(&url).unwrap(), None, &[]);
            server.join().unwrap();
            fs::remove_dir_all(dir).unwrap();
            let error = result
                .err()
                .expect("informational headers are not a final response");
            assert!(error.to_string().contains("response headers truncated"));
        }
    }

    #[test]
    fn sequential_rejects_truncated_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nshort",
                )
                .unwrap();
        });
        let (job, dir) = temp_job(&format!("http://{addr}"), true, 0, 1);
        let error = run_job(&job).unwrap_err().to_string();
        assert!(error.contains("response body length mismatch"), "{error}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sequential_partial_response_requires_the_complete_tail() {
        for (range, body, length, resume, total, succeeds) in [
            ("0-3/10", "abcd", Some(4), 0, 10, false),
            ("0-3/10", "abcd", Some(4), 0, 0, false),
            ("4-5/10", "ef", Some(2), 4, 10, false),
            ("0-3/10", "abcd", None, 0, 10, false),
            ("0-9/10", "abcdefghij", Some(10), 0, 10, true),
            ("4-9/10", "efghij", Some(6), 4, 10, true),
            ("0-9/10", "abcd", Some(4), 0, 10, false),
            ("0-9/10", "abcd", None, 0, 0, false),
            ("0-9/10", "abcdefghij", None, 0, 0, true),
            ("0-3/*", "abcd", Some(4), 0, 0, false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let length = length
                    .map(|n| format!("Content-Length: {n}\r\n"))
                    .unwrap_or_default();
                let reply = format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {range}\r\n{length}Connection: close\r\n\r\n{body}");
                let _ = reader.get_mut().write_all(reply.as_bytes());
            });
            let (mut job, dir) = temp_job(&url, true, total, 1);
            job.resume_from = resume;
            if resume > 0 {
                fs::write(&job.output, b"abcd-stale-bytes").unwrap();
            }
            let result = download_sequential(&job, None);
            server.join().unwrap();
            assert_eq!(result.is_ok(), succeeds, "{range}: {result:?}");
            if succeeds {
                assert_eq!(fs::read(&job.output).unwrap(), b"abcdefghij");
            }
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn empty_resource_probe_falls_back_to_an_unranged_get() {
        for total in [0, 8] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                for attempt in 0..if total == 0 { 2 } else { 1 } {
                    let (stream, _) = loop {
                        match listener.accept() {
                            Ok(connection) => break connection,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(
                                    Instant::now() < deadline,
                                    "expected request did not arrive"
                                );
                                thread::sleep(Duration::from_millis(10));
                            }
                            Err(error) => panic!("accept failed: {error}"),
                        }
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream);
                    let mut request = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                            break;
                        }
                        request.push_str(&line);
                    }
                    let reply = if attempt == 0 {
                        assert!(request.to_lowercase().contains("range: bytes=0-0"));
                        format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    } else {
                        assert!(!request.to_lowercase().contains("range:"));
                        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
                    };
                    reader.get_mut().write_all(reply.as_bytes()).unwrap();
                }
            });
            let (mut job, dir) = temp_job(&url, false, 0, 1);
            job.etag.clear();
            let result = run_job(&job);
            server.join().unwrap();
            assert_eq!(result.is_ok(), total == 0, "{result:?}");
            if total == 0 {
                assert_eq!(fs::metadata(&job.output).unwrap().len(), 0);
            }
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn head_response_with_content_length_completes_without_a_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        let (mut job, dir) = temp_job(&format!("http://{addr}"), true, 0, 1);
        job.method = "HEAD".into();
        // A HEAD reply carries the entity Content-Length but never a body, so the
        // byte-count check must not treat that header as an expected body length.
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), Vec::<u8>::new());
        let _ = fs::remove_dir_all(dir);
    }

    // The exemption must key off the method that actually reaches the wire, not the
    // raw string: `request_method` trims, so a job JSON carrying `" Head"` sends HEAD
    // yet would fail the byte-count check against the untrimmed `job.method`.
    #[test]
    fn head_method_with_surrounding_whitespace_still_completes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        let (mut job, dir) = temp_job(&format!("http://{addr}"), true, 0, 1);
        job.method = " Head".into();
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), Vec::<u8>::new());
        let _ = fs::remove_dir_all(dir);
    }

    // The dispatch used to send any non-POST job with a total to `download_ranges`.
    // A HEAD range response is 206 + Content-Range with no body, so the range
    // workers read zero bytes and the job died as "made no progress after 5
    // attempts" even though nothing was ever going to arrive.  HEAD now takes the
    // sequential path regardless of the total, which is what this pins.
    #[test]
    fn head_job_with_a_total_uses_the_sequential_path() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
            }
            // A HEAD reply to a Range request: 206 + Content-Range, never a body.
            let response = concat!(
                "HTTP/1.1 206 Partial Content\r\n",
                "Content-Range: bytes 0-9/10\r\n",
                "Content-Length: 10\r\n",
                "Connection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        // sequential = false and total = 10 is exactly the shape that used to be
        // routed to `download_ranges`.
        let (mut job, dir) = temp_job(&format!("http://{addr}"), false, 10, 1);
        job.method = "HEAD".into();
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), Vec::<u8>::new());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sequential_ignores_a_stale_total_when_the_body_has_no_content_length() {
        let body: &[u8] = b"0123456789";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
            }
            // Close-delimited body: the response itself declares no size, so a
            // caller-supplied (here deliberately wrong) total must not be used.
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .unwrap();
            stream.write_all(body).unwrap();
        });
        let (job, dir) = temp_job(&format!("http://{addr}"), true, 999, 1);
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn parse_header_meta_recognises_a_decoded_content_encoding() {
        let plain = parse_header_meta("Content-Length: 4").unwrap();
        assert!(!plain.content_encoding);
        assert_eq!(plain.content_length, Some(4));

        let encoded = parse_header_meta("Content-Length: 4\r\nContent-Encoding: gzip").unwrap();
        assert!(encoded.content_encoding);
        assert!(
            parse_header_meta("Content-Encoding: br")
                .unwrap()
                .content_encoding
        );

        // `identity` and an empty value describe the unchanged entity.
        assert!(
            !parse_header_meta("Content-Encoding: identity")
                .unwrap()
                .content_encoding
        );
        assert!(
            !parse_header_meta("Content-Encoding:   ")
                .unwrap()
                .content_encoding
        );
    }

    #[test]
    fn pause_control_stops_before_writing() {
        let body: &'static [u8] = b"0123456789";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.control, "pause").unwrap();
        let err = run_job(&job).unwrap_err();
        assert!(matches!(&err, EngineError::Pause));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_control_stops_before_writing() {
        let body: &'static [u8] = b"0123456789";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.control, "cancel").unwrap();
        let err = run_job(&job).unwrap_err();
        assert!(matches!(&err, EngineError::Cancel));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stalled_http_headers_body_and_chunked_reads_can_stop() {
        stalled_transport_reads_can_stop(false, &["pause", "cancel"]);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Run explicitly in Windows Sandbox"]
    fn winhttp_stalled_headers_body_and_chunked_reads_can_stop() {
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
        stalled_transport_reads_can_stop(true, &["pause", "cancel"]);
    }

    #[test]
    fn shared_range_stop_interrupts_stalled_transports_and_preserves_pause() {
        stalled_transport_reads_can_stop(false, &["worker_stop", "worker_pause"]);
        #[cfg(windows)]
        stalled_transport_reads_can_stop(true, &["worker_stop", "worker_pause"]);
    }

    fn stalled_transport_reads_can_stop(via_proxy: bool, actions: &[&str]) {
        use std::sync::mpsc;
        for phase in ["headers", "body", "chunked"] {
            for &action in actions {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let addr = listener.local_addr().unwrap();
                let (ready_tx, ready_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let server = thread::spawn(move || {
                    let (stream, _) = listener.accept().unwrap();
                    let mut reader = BufReader::new(stream);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                            break;
                        }
                    }
                    let mut stream = reader.into_inner();
                    let response: &[u8] = match phase {
                        "headers" => b"HTTP/1.1 200 OK\r\n",
                        "chunked" => {
                            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n"
                        }
                        _ => b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc",
                    };
                    stream.write_all(response).unwrap();
                    ready_tx.send(()).unwrap();
                    // 主线程在断言前释放连接，即使回归也不会留下阻塞的服务线程。
                    let _ = release_rx.recv();
                });
                let (mut job, dir) = temp_job(&format!("http://{addr}"), true, 0, 1);
                let shared_stop = Arc::new(AtomicBool::new(false));
                job.range_stop = Some(Arc::clone(&shared_stop));
                if via_proxy {
                    job.proxy = format!("http://{addr}");
                    job.url = "http://hls-control.example/media.bin".into();
                }
                let worker_job = job.clone();
                let (done_tx, done_rx) = mpsc::channel();
                let worker = thread::spawn(move || {
                    let _ = done_tx.send(run_job(&worker_job));
                });
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                if phase != "headers" {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    // WinHTTP 可能等待读缓冲区填满；输出创建表示响应头已返回，后续为正文等待。
                    while if via_proxy {
                        !job.output.exists()
                    } else {
                        fs::metadata(&job.output).map(|m| m.len()).unwrap_or(0) != 3
                    } {
                        assert!(Instant::now() < deadline, "body read not reached: {phase}");
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                if action.starts_with("worker_") {
                    if action == "worker_pause" {
                        fs::write(&job.control, "pause").unwrap();
                    }
                    shared_stop.store(true, Ordering::SeqCst);
                } else {
                    fs::write(&job.control, action).unwrap();
                }
                let result = done_rx.recv_timeout(Duration::from_secs(3));
                let _ = release_tx.send(());
                server.join().unwrap();
                worker.join().unwrap();
                let error = result
                    .expect("stalled read did not stop promptly")
                    .unwrap_err();
                assert!(
                    matches!(
                        (&error, action),
                        (EngineError::Pause, "pause" | "worker_pause")
                            | (EngineError::Cancel, "cancel" | "worker_stop")
                    ),
                    "{phase}/{action}: {error}"
                );
                if phase != "headers" && !via_proxy {
                    assert_eq!(fs::read(&job.output).unwrap(), b"abc");
                }
                let _ = fs::remove_dir_all(dir);
            }
        }
    }

    #[test]
    fn registered_control_token_stops_a_stalled_transfer_without_polling_the_file() {
        for action in ["pause", "cancel"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let mut stream = reader.into_inner();
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc")
                    .unwrap();
                ready_tx.send(()).unwrap();
                let _ = release_rx.recv();
            });
            let (job, dir) = temp_job(&format!("http://{addr}"), true, 0, 1);
            let registration = crate::task_control::register(&job.control);
            let worker_job = job.clone();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let worker = thread::spawn(move || {
                let _ = done_tx.send(run_job(&worker_job));
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while fs::metadata(&job.output).map(|m| m.len()).unwrap_or(0) != 3 {
                assert!(Instant::now() < deadline, "body read not reached");
                thread::sleep(Duration::from_millis(5));
            }
            // Stalled for 600 ms: 50 ms file polling would have opened the
            // control file about a dozen times.
            thread::sleep(Duration::from_millis(600));
            let reads = crate::task_control::file_reads(&job.control).unwrap();
            assert!(reads <= 2, "control file read {reads} times while stalled");
            let stopped_at = Instant::now();
            crate::task_control::set(&job.control, action).unwrap();
            let result = done_rx.recv_timeout(Duration::from_secs(3));
            let latency = stopped_at.elapsed();
            let _ = release_tx.send(());
            server.join().unwrap();
            worker.join().unwrap();
            let error = result.expect("stalled read did not stop").unwrap_err();
            assert!(
                matches!(
                    (&error, action),
                    (EngineError::Pause, "pause") | (EngineError::Cancel, "cancel")
                ),
                "{action}: {error}"
            );
            assert!(
                latency < Duration::from_millis(900),
                "{action} took {latency:?}"
            );
            // The file is still written for crash recovery and external tools.
            assert_eq!(fs::read_to_string(&job.control).unwrap(), action);
            drop(registration);
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn content_range_parser_accepts_rfc_and_bare_forms() {
        assert_eq!(parse_content_range("bytes 0-7/8"), Some((0, 7, Some(8))));
        assert_eq!(parse_content_range("BYTES 10-19/*"), Some((10, 19, None)));
        assert_eq!(parse_content_range("0-1/2"), Some((0, 1, Some(2))));
        assert_eq!(parse_content_range("bytes */8"), None);
        assert_eq!(parse_content_range(""), None);
        assert!(require_content_range_start(Some("bytes 4-9/10"), 4).is_ok());
        assert!(require_content_range_start(Some("bytes 0-9/10"), 4).is_err());
        assert!(require_content_range_start(None, 0).is_err());
        for header in ["byte界 0-7/8", "🙂🙂", "byte� 0-7/8"] {
            assert_eq!(parse_content_range(header), None);
        }
    }

    #[test]
    fn ipv6_authorities_preserve_host_redirect_and_origin_boundaries() {
        for (url, host, authority) in [
            ("http://[::1]?key=1#ignored", "::1", "[::1]"),
            (
                "https://[2001:DB8::1]:443/a",
                "2001:db8::1",
                "[2001:db8::1]",
            ),
            (
                "http://[::ffff:192.0.2.1]:8080/a",
                "::ffff:c000:201",
                "[::ffff:c000:201]:8080",
            ),
        ] {
            let parsed = parse_http_url(url).unwrap();
            assert_eq!(parsed.host, host);
            assert_eq!(host_header(&parsed), authority);
        }
        assert_eq!(
            parse_http_url("http://[::1]?key=1#ignored").unwrap().path,
            "/?key=1"
        );
        let current = parse_http_url("http://[::1]:8080/dir/file").unwrap();
        assert_eq!(
            resolve_location(&current, "next"),
            "http://[::1]:8080/dir/next"
        );
        assert_eq!(
            resolve_location(&current, "/file"),
            "http://[::1]:8080/file"
        );
        assert_eq!(
            http_origin("http://[0:0:0:0:0:0:0:1]/"),
            http_origin("http://[::1]:80/x")
        );
        assert_ne!(
            http_origin("http://[::1]/"),
            http_origin("http://[::1]:81/")
        );
        assert_eq!(
            request_origin("HTTPS://[2001:DB8::1]:443/a"),
            "https://[2001:db8::1]"
        );
        assert_eq!(request_origin("http://[::1]:8080/a"), "http://[::1]:8080");
        for url in [
            "http://::1/",
            "http://[::1/",
            "http://[::1]extra/",
            "http://[::1]:/",
            "http://[::1]:+80/",
            "http://[::1]:65536/",
            "http://[localhost]/",
            "http://[::1]]/",
            "http://host]/",
            "http://[fe80::1%25eth0]/",
        ] {
            assert!(parse_http_url(url).is_err(), "{url}");
            assert!(request_origin(url).is_empty(), "{url}");
        }
    }

    #[test]
    fn ipv6_loopback_range_redirect_keeps_credentials_on_same_origin() {
        #[cfg(windows)]
        let transports = vec!["raw", "winhttp"];
        #[cfg(not(windows))]
        let transports = vec!["raw"];
        for transport in transports {
            let listener = TcpListener::bind("[::1]:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                for index in 0..2 {
                    let (stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream);
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                        head.push_str(&line);
                    }
                    let head = head.to_ascii_lowercase();
                    assert!(head.contains(&format!("host: {address}\r\n")), "{head}");
                    assert!(head.contains("range: bytes=2-5\r\n"), "{head}");
                    assert!(head.contains("cookie: session=ipv6\r\n"), "{head}");
                    assert!(head.contains("authorization: bearer ipv6\r\n"), "{head}");
                    let response = if index == 0 {
                        "HTTP/1.1 307 Redirect\r\nLocation: /file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    } else {
                        assert!(head.starts_with("get /file http/1.1\r\n"), "{head}");
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-5/8\r\nContent-Length: 4\r\nConnection: close\r\n\r\ncdef"
                    };
                    reader.get_mut().write_all(response.as_bytes()).unwrap();
                }
            });
            let mut job = post_test_job(format!("http://{address}/start"), PathBuf::new());
            job.method = "GET".into();
            job.headers = HashMap::from([
                ("Cookie".into(), "session=ipv6".into()),
                ("Authorization".into(), "Bearer ipv6".into()),
            ]);
            if transport == "winhttp" {
                job.proxy = crate::net_policy::SYSTEM_PROXY_SENTINEL.into();
            }
            let mut fetched = fetch_follow(&job, Some("bytes=2-5")).unwrap();
            assert_eq!(fetched.status, 206, "{transport}");
            assert_eq!(fetched.content_range.as_deref(), Some("bytes 2-5/8"));
            let mut bytes = Vec::new();
            fetched.body.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"cdef");
            server.join().unwrap();
        }
    }

    #[test]
    fn host_header_omits_default_ports() {
        let http = parse_http_url("http://cdn.test/file.bin").unwrap();
        assert_eq!(host_header(&http), "cdn.test");
        let query_only = parse_http_url("http://cdn.test?token=abc").unwrap();
        assert_eq!(query_only.host, "cdn.test");
        assert_eq!(query_only.path, "/?token=abc");
        let query_with_url =
            parse_http_url("http://cdn.test?next=https://a.test/file#ignored/path").unwrap();
        assert_eq!(query_with_url.host, "cdn.test");
        assert_eq!(query_with_url.path, "/?next=https://a.test/file");
        let path_with_fragment = parse_http_url("http://cdn.test/file.bin#ignored").unwrap();
        assert_eq!(path_with_fragment.path, "/file.bin");
        let custom = parse_http_url("http://127.0.0.1:8765/api").unwrap();
        assert_eq!(host_header(&custom), "127.0.0.1:8765");
        let https = parse_http_url("https://cdn.test/a").unwrap();
        assert_eq!(host_header(&https), "cdn.test");
        assert!(parse_http_url("http://cdn.test/foo\r\nHost: evil").is_err());
        assert!(!http_fetch_url_allowed("javascript:alert(1)"));
        assert!(!http_fetch_url_allowed("\u{feff}javascript:alert(1)"));
        assert!(!http_fetch_url_allowed("https://cdn.test/a.bin\0.gif"));
        assert!(!http_fetch_url_allowed("http://cdn.test/x\nHost: evil"));
        assert!(http_fetch_url_allowed("HTTPS://cdn.test/a"));
        assert!(remote_resource_url_allowed("magnet:?xt=urn:btih:abc"));
        assert!(remote_resource_url_allowed("ftp://ftp.test/a.bin"));
        assert!(!remote_resource_url_allowed("javascript:alert(1)"));
        assert!(!remote_resource_url_allowed("ms-msdt:foo"));
        assert!(!remote_resource_url_allowed("file:///C:/Windows/win.ini"));
        assert_eq!(
            parse_http_url("HTTP://cdn.test/file.bin").unwrap().host,
            "cdn.test"
        );
    }

    #[test]
    fn raw_http_requests_close_connection_for_body_end_detection() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_recording_headers(b"body", Arc::clone(&seen));
        assert_eq!(
            fetch_hls_bytes_range(&url, &HashMap::new(), "", 0, 4).unwrap(),
            b"body"
        );
        assert!(seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|line| line == "connection: close"));
    }

    #[test]
    fn header_injection_values_are_not_wire_safe() {
        assert!(header_is_wire_safe("Referer", "https://cdn.test/page"));
        assert!(!header_is_wire_safe("X-Evil", "ok\r\nHost: evil.example"));
        assert!(!header_is_wire_safe("X:Smuggle", "ok"));
        let headers = HashMap::from([("user-agent".into(), "bad\r\nvalue".into())]);
        assert_eq!(request_user_agent(&headers), CHROME_UA);
        assert!(!header_allowed_on_request(
            "Proxy-Authorization",
            "credentials"
        ));
    }

    #[test]
    fn ambiguous_response_framing_is_rejected() {
        for headers in [
            "Content-Length: -1",
            "Content-Length: +1",
            "Content-Length: 18446744073709551616",
            "Content-Length: 1\r\nContent-Length: 2",
            "Content-Length: 1, 2",
            "Transfer-Encoding: chunked\r\nContent-Length: 1",
            "Transfer-Encoding: chunked, chunked",
            "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked",
            "Content-Range: bytes 0-2/3\r\nContent-Range: bytes 0-2/4",
            "ETag: \"old\"\r\nETag: \"new\"",
        ] {
            assert!(parse_header_meta(headers).is_err(), "{headers}");
        }
        let meta = parse_header_meta("Content-Length: 4, 4\r\nContent-Length: 04").unwrap();
        assert_eq!(meta.content_length, Some(4));
        assert!(
            parse_header_meta("Content-Encoding: br\r\nContent-Encoding: identity")
                .unwrap()
                .content_encoding
        );
    }

    #[test]
    fn range_plan_bounds_work_for_extreme_content_lengths() {
        let chunk = range_chunk_size(u64::MAX, 64, 64 * 1024);
        assert!(u64::MAX.div_ceil(chunk) <= 65_536);
        assert_eq!(range_chunk_size(512 * 1024, 2, 64 * 1024), 64 * 1024);
    }

    #[test]
    fn legacy_replay_keeps_the_primary_origin_across_later_hops() {
        let mut job = post_test_job("https://source.example/file".into(), PathBuf::new());
        job.replay_json = r#"{"cookie":"session=original"}"#.into();
        let replay = job_replay_json(&job);
        let mut headers = headers_for_request(&job, &job.url);
        assert!(headers.keys().any(|key| key.eq_ignore_ascii_case("cookie")));
        let mirror = "https://mirror.example/file";
        drop_cross_origin_secrets(&mut headers, &replay, &job.url, mirror);
        drop_cross_origin_secrets(&mut headers, &replay, mirror, "https://mirror.example/next");
        assert!(!headers.keys().any(|key| key.eq_ignore_ascii_case("cookie")));
    }

    #[test]
    fn relative_redirect_joins_path_directory() {
        let current = parse_http_url("http://cdn.test/dir/file.bin").unwrap();
        assert_eq!(
            resolve_location(&current, "next.bin"),
            "http://cdn.test/dir/next.bin"
        );
        assert_eq!(
            resolve_location(&current, "/abs.bin"),
            "http://cdn.test/abs.bin"
        );
        assert_eq!(
            resolve_location(&current, "https://other.test/x"),
            "https://other.test/x"
        );
        assert_eq!(
            resolve_location(&current, "HTTPS://other.test/x"),
            "HTTPS://other.test/x"
        );
        assert_eq!(
            resolve_location(
                &current,
                "https://other.test/dir/../file.bin?token=a%2Fb&x=../"
            ),
            "https://other.test/file.bin?token=a%2Fb&x=../"
        );
        assert_eq!(
            resolve_location(&current, "//other.test/dir/../file.bin?token=a&amp;b=2"),
            "http://other.test/file.bin?token=a&amp;b=2"
        );
        assert!(resolve_location(&current, "file:///C:/secret").is_empty());
        assert!(resolve_location(&current, "http://cdn.test/x\r\nHost: evil").is_empty());
        assert_eq!(
            resolve_location(&current, "//cdn2.test/x"),
            "http://cdn2.test/x"
        );
        assert!(parse_http_url("http://cdn.test/foo bar").is_err());
        let rooted = parse_http_url("http://cdn.test:8080/file.bin").unwrap();
        assert_eq!(
            resolve_location(&rooted, "next.bin"),
            "http://cdn.test:8080/next.bin"
        );
        let signed = parse_http_url("https://cdn.test/dir/export.bin?next=/wrong/path").unwrap();
        assert!(resolve_location(&signed, "HTTP://cdn.test/file").is_empty());
        for (location, expected) in [
            ("?token=a%2Fb&x=../", "/dir/export.bin?token=a%2Fb&x=../"),
            ("#part", "/dir/export.bin?next=/wrong/path#part"),
            ("next.bin", "/dir/next.bin"),
            (
                "../download/./file.bin?sig=a%2Fb&x=../",
                "/download/file.bin?sig=a%2Fb&x=../",
            ),
            ("/dir/../file.bin", "/file.bin"),
            ("../../file.bin", "/file.bin"),
            ("../", "/"),
            ("%2e%2e/file.bin", "/dir/%2e%2e/file.bin"),
            ("next//file.bin", "/dir/next//file.bin"),
        ] {
            assert_eq!(
                resolve_location(&signed, location),
                format!("https://cdn.test{expected}"),
                "{location}"
            );
        }
    }

    #[test]
    fn relative_redirects_preserve_signed_targets_and_scoped_credentials_across_transports() {
        let mut transports = vec!["raw"];
        #[cfg(windows)]
        transports.push("winhttp");
        if curl_impersonate_exe().is_some() {
            transports.push("curl");
        }
        for transport in transports {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let mut requests = Vec::new();
                for response in [
                    "HTTP/1.1 302 Found\r\nLocation: ?token=a%2Fb&b=2\r\nSet-Cookie: unscoped=one; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    "HTTP/1.1 307 Redirect\r\nLocation: ../download/./file.bin?sig=a%2Fb&x=../\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nfile",
                ] {
                    let (stream, _) = listener.accept().unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut reader = BufReader::new(stream);
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                        head.push_str(&line);
                    }
                    requests.push(head);
                    reader.get_mut().write_all(response.as_bytes()).unwrap();
                }
                requests
            });
            let mut job = post_test_job(
                format!("{origin}/dir/export.bin?next=/wrong/path"),
                PathBuf::new(),
            );
            job.method = "GET".into();
            job.headers.clear();
            if transport == "winhttp" {
                job.proxy = crate::net_policy::SYSTEM_PROXY_SENTINEL.into();
            }
            job.replay_json = serde_json::json!({
                "_task_url": job.url,
                "request_contexts": {
                    origin.clone(): {
                        "cookies": [
                            { "name": "dir", "value": "one", "domain": "127.0.0.1", "path": "/dir/" },
                            { "name": "download", "value": "two", "domain": "127.0.0.1", "path": "/download/" }
                        ],
                        "url_contexts": {
                            format!("{origin}/dir/export.bin?token=a%2Fb&b=2"): {
                                "request_headers": { "Authorization": "Bearer query" }
                            },
                            format!("{origin}/download/file.bin?sig=a%2Fb&x=../"): {
                                "request_headers": { "Authorization": "Bearer final" }
                            }
                        }
                    }
                }
            })
            .to_string();
            let mut fetched = if transport == "curl" {
                fetch_curl_follow(&job, None)
                    .expect("curl transport available")
                    .unwrap()
            } else {
                fetch_follow(&job, None).unwrap()
            };
            assert_eq!(fetched.status, 200, "{transport}");
            let mut bytes = Vec::new();
            fetched.body.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"file");
            let requests = server.join().unwrap();
            for (head, target) in requests.iter().zip([
                "/dir/export.bin?next=/wrong/path",
                "/dir/export.bin?token=a%2Fb&b=2",
                "/download/file.bin?sig=a%2Fb&x=../",
            ]) {
                assert!(
                    head.starts_with(&format!("GET {target} HTTP/1.1\r\n")),
                    "{transport}: {head}"
                );
            }
            assert!(
                requests[1]
                    .to_ascii_lowercase()
                    .contains("authorization: bearer query\r\n"),
                "{transport}: {}",
                requests[1]
            );
            let last = requests[2].to_ascii_lowercase();
            assert!(
                last.contains("authorization: bearer final\r\n"),
                "{transport}: {last}"
            );
            assert!(
                last.contains("cookie: download=two\r\n"),
                "{transport}: {last}"
            );
            assert!(!last.contains("dir=one"), "{transport}: {last}");
            assert!(!last.contains("unscoped=one"), "{transport}: {last}");
        }
    }

    #[test]
    fn post_redirects_preserve_or_discard_the_body_across_transports() {
        let mut transports = vec!["raw"];
        #[cfg(windows)]
        transports.push("winhttp");
        if curl_impersonate_exe().is_some() {
            transports.push("curl");
        }
        for transport in transports {
            for status in [301, 302, 303, 307, 308] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let address = listener.local_addr().unwrap();
                let (sent, received) = std::sync::mpsc::channel();
                let server = thread::spawn(move || {
                    let mut requests = Vec::new();
                    for index in 0..3 {
                        let (stream, _) = listener.accept().unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut reader = BufReader::new(stream);
                        let mut head = String::new();
                        loop {
                            let mut line = String::new();
                            assert!(reader.read_line(&mut line).unwrap() > 0);
                            if line == "\r\n" {
                                break;
                            }
                            head.push_str(&line);
                        }
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        let mut body = vec![0; length];
                        reader.read_exact(&mut body).unwrap();
                        requests.push((head, body));
                        let response = match index {
                            0 => format!("HTTP/1.1 {status} Redirect\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                            1 => "HTTP/1.1 308 Redirect\r\nLocation: /file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                            _ => "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nfile".into(),
                        };
                        reader.get_mut().write_all(response.as_bytes()).unwrap();
                    }
                    let _ = sent.send(requests);
                });
                let (_, dir) = temp_job("http://example.test/", true, 0, 1);
                let body_path = dir.join("request.bin");
                fs::write(&body_path, b"export=one").unwrap();
                let url = format!("http://{address}/export");
                let mut job = post_test_job(url, body_path);
                if transport == "winhttp" {
                    job.proxy = crate::net_policy::SYSTEM_PROXY_SENTINEL.into();
                }
                job.replay_json = serde_json::json!({
                    "_task_url": job.url,
                    "request_headers": { "Content-Type": "application/x-www-form-urlencoded" }
                })
                .to_string();
                let mut fetched = if transport == "curl" {
                    fetch_curl_follow(&job, None)
                        .expect("curl transport available")
                        .unwrap()
                } else {
                    fetch_follow(&job, None).unwrap()
                };
                assert_eq!(fetched.status, 200, "{transport} {status}");
                let mut output = Vec::new();
                fetched.body.read_to_end(&mut output).unwrap();
                assert_eq!(output, b"file");
                let requests = received.recv_timeout(Duration::from_secs(10)).unwrap();
                server.join().unwrap();
                assert!(requests[0].0.starts_with("POST "));
                assert_eq!(requests[0].1, b"export=one");
                for (head, body) in &requests[1..] {
                    if status <= 303 {
                        assert!(head.starts_with("GET "), "{transport} {status}: {head}");
                        assert!(body.is_empty(), "{transport} {status}: body replayed");
                        assert!(
                            !head.to_ascii_lowercase().contains("content-type:"),
                            "{transport} {status}: stale body header"
                        );
                    } else {
                        assert!(head.starts_with("POST "), "{transport} {status}: {head}");
                        assert_eq!(body, b"export=one");
                    }
                }
                fs::remove_dir_all(dir).unwrap();
            }
        }
    }

    #[test]
    fn cross_origin_redirect_does_not_forward_cookie() {
        let seen = Arc::new(Mutex::new(String::new()));
        let dest = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let seen = Arc::clone(&seen);
            thread::spawn(move || {
                if let Ok((stream, _)) = listener.accept() {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut cookie = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                            break;
                        }
                        if line.to_ascii_lowercase().starts_with("cookie:") {
                            cookie = line;
                        }
                    }
                    *seen.lock().unwrap_or_else(|err| err.into_inner()) = cookie;
                    let mut stream = reader.into_inner();
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping",
                    );
                }
            });
            format!("http://127.0.0.1:{}", addr.port())
        };
        let src = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let dest = dest.clone();
            thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf);
                    let header = format!(
                        "HTTP/1.1 302 Found\r\nLocation: {dest}/file.bin\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(header.as_bytes());
                }
            });
            format!("http://127.0.0.1:{}/start.bin", addr.port())
        };
        let (mut job, dir) = temp_job(&src, true, 4, 1);
        job.headers.insert("Cookie".into(), "sid=secret".into());
        job.etag.clear();
        run_job(&job).unwrap();
        let cookie = seen.lock().unwrap_or_else(|err| err.into_inner()).clone();
        assert!(
            !cookie.to_ascii_lowercase().contains("sid=secret"),
            "redirect forwarded cookie: {cookie}"
        );
        assert_eq!(fs::read(&job.output).unwrap(), b"ping");
        let _ = fs::remove_dir_all(dir);
    }

    fn serve_206_without_range(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
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
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn serve_206_wrong_range(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
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
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 1-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() - 1,
                    body.len(),
                    body.len().saturating_sub(1)
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn serve_206_wrong_total(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut range_start = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                        let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                        range_start = spec
                            .split_once('-')
                            .and_then(|(start, _)| start.parse().ok())
                            .unwrap_or(0);
                    }
                }
                let slice = &body[range_start.min(body.len() - 1)..];
                let mut stream = reader.into_inner();
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {range_start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() - 1,
                    body.len() + 1,
                    slice.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(slice);
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[test]
    fn missing_content_range_on_206_is_range_unsupported() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let url = serve_206_without_range(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 3);
        let err = run_job(&job).unwrap_err();
        assert!(matches!(&err, EngineError::RangeUnsupported(_)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mismatched_content_range_start_is_range_unsupported() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let url = serve_206_wrong_range(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 3);
        let err = run_job(&job).unwrap_err();
        assert!(matches!(&err, EngineError::RangeUnsupported(_)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mismatched_content_range_total_is_range_unsupported() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let url = serve_206_wrong_total(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 3);
        let err = run_job(&job).unwrap_err();
        assert!(matches!(&err, EngineError::RangeUnsupported(_)));
        assert!(err.to_string().contains("Content-Range total"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn range_worker_rejects_inconsistent_partial_response_before_checkpoint() {
        for (range, length) in [("0-3/10", 10), ("0-9/10", 4), ("0-9/10", 0)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let mut stream = reader.into_inner();
                let _ = write!(stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {range}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(&b"abcdefghij"[..length]);
            });
            let (job, dir) = temp_job(&url, false, 10, 1);
            let result = run_job(&job);
            server.join().unwrap();
            let saved = load_completed_ranges(&job).unwrap_or_default();
            fs::remove_dir_all(dir).unwrap();
            assert!(
                matches!(result, Err(EngineError::RangeUnsupported(_))),
                "{range}, length={length}: {result:?}"
            );
            assert!(
                saved.is_empty(),
                "invalid metadata must never become a durable range"
            );
        }
    }

    #[test]
    fn chunked_range_requires_a_complete_terminator_before_checkpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            reader.get_mut().write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3/4\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ndata").unwrap();
        });
        let (job, dir) = temp_job(&url, false, 4, 1);
        assert!(run_job(&job).is_err());
        server.join().unwrap();
        assert!(load_completed_ranges(&job).unwrap_or_default().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    fn serve_chunked(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                }
                let mut stream = reader.into_inner();
                let header =
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(format!("{:x}\r\n", body.len()).as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.write_all(b"\r\n0\r\n\r\n");
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[test]
    fn chunked_body_is_decoded_without_writing_framing() {
        let body: &'static [u8] = b"hello-chunked";
        let url = serve_chunked(body);
        let (job, dir) = temp_job(&url, true, 0, 1);
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let _ = fs::remove_dir_all(dir);
    }

    fn serve_recording_ranges(body: &'static [u8], seen: Arc<Mutex<Vec<String>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                        range = Some(value.trim().to_string());
                    }
                }
                if let Some(value) = range.clone() {
                    seen.lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .push(value);
                }
                let mut stream = reader.into_inner();
                if let Some(value) = range {
                    let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                    let (start_text, end_text) = spec.split_once('-').unwrap();
                    let start: usize = start_text.parse().unwrap();
                    let end: usize = if end_text.is_empty() {
                        body.len() - 1
                    } else {
                        end_text.parse().unwrap()
                    };
                    let actual_end = end.min(body.len() - 1);
                    let slice = &body[start..=actual_end];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{actual_end}/{}\r\nContent-Length: {}\r\nETag: \"native-test\"\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        body.len(),
                        slice.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(slice);
                } else {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                }
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn serve_counting_range_payload(body: &'static [u8], payload_bytes: Arc<AtomicU64>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                        range = Some(value.trim().to_string());
                    }
                }
                let mut stream = reader.into_inner();
                if let Some(value) = range {
                    let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                    let (start_text, end_text) = spec.split_once('-').unwrap();
                    let start: usize = start_text.parse().unwrap();
                    let end: usize = if end_text.is_empty() {
                        body.len() - 1
                    } else {
                        end_text.parse().unwrap()
                    };
                    let slice = &body[start..=end.min(body.len() - 1)];
                    payload_bytes.fetch_add(slice.len() as u64, Ordering::SeqCst);
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len() - 1,
                        body.len(),
                        slice.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(slice);
                } else {
                    payload_bytes.fetch_add(body.len() as u64, Ordering::SeqCst);
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                }
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn serve_recording_headers(body: &'static [u8], seen: Arc<Mutex<Vec<String>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    let normalized = line.trim().to_ascii_lowercase();
                    if let Some(value) = normalized.strip_prefix("range:") {
                        range = Some(value.trim().to_string());
                    }
                    seen.lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .push(normalized);
                }
                let mut stream = reader.into_inner();
                if let Some(value) = range {
                    let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                    let (start_text, end_text) = spec.split_once('-').unwrap();
                    let start: usize = start_text.parse().unwrap();
                    let end: usize = end_text.parse().unwrap();
                    let slice = &body[start..=end.min(body.len() - 1)];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len() - 1,
                        body.len(),
                        slice.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(slice);
                }
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn serve_capped_ranges(
        body: &'static [u8],
        cap: usize,
        seen: Arc<Mutex<Vec<usize>>>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut requested: Option<(usize, usize)> = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                        let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                        requested = spec.split_once('-').and_then(|(start, end)| {
                            Some((start.parse().ok()?, end.parse().ok()?))
                        });
                    }
                }
                let Some((start, requested_end)) = requested else {
                    continue;
                };
                seen.lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .push(start);
                let actual_end = requested_end
                    .min(start.saturating_add(cap.saturating_sub(1)))
                    .min(body.len() - 1);
                let slice = &body[start..=actual_end];
                let mut stream = reader.into_inner();
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{actual_end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len(),
                    slice.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(slice);
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn serve_empty_ranges(total: usize, attempts: Arc<AtomicUsize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut start = 0usize;
                let mut end = total.saturating_sub(1);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                        let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                        if let Some((from, to)) = spec.split_once('-') {
                            start = from.parse().unwrap_or(0);
                            end = to.parse().unwrap_or(end);
                        }
                    }
                }
                attempts.fetch_add(1, Ordering::SeqCst);
                let mut stream = reader.into_inner();
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(header.as_bytes());
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn serve_transient_429_ranges(
        body: &'static [u8],
        attempts: Arc<AtomicUsize>,
        payload_bytes: Arc<AtomicU64>,
        retry_times: Arc<Mutex<Vec<Instant>>>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut requested = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                        let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                        requested = spec.split_once('-').and_then(|(start, end)| {
                            Some((start.parse::<usize>().ok()?, end.parse::<usize>().ok()?))
                        });
                    }
                }
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                retry_times.lock().unwrap().push(Instant::now());
                let mut stream = reader.into_inner();
                if attempt < 2 {
                    let _ = stream.write_all(
                        b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                let Some((start, requested_end)) = requested else {
                    continue;
                };
                let actual_end = requested_end.min(body.len() - 1);
                let slice = &body[start..=actual_end];
                payload_bytes.fetch_add(slice.len() as u64, Ordering::SeqCst);
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{actual_end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len(),
                    slice.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(slice);
                if payload_bytes.load(Ordering::SeqCst) >= body.len() as u64 {
                    break;
                }
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[test]
    fn range_resume_skips_sidecar_covered_chunks() {
        let body: &'static [u8] = Box::leak(
            (0..70 * 1024)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_recording_ranges(body, Arc::clone(&seen));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        let first_end = 64 * 1024 - 1;
        fs::write(&job.output, &body[..=first_end]).unwrap();
        fs::write(
            job.progress.with_file_name("native-engine.ranges.json"),
            format!(
                r#"{{"version":2,"resource_key":"{}","etag":"\"native-test\"","total":{},"ranges":[[0,65535]]}}"#,
                job.resource_key, job.total
            ),
        )
        .unwrap();
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let ranges = seen.lock().unwrap_or_else(|err| err.into_inner()).clone();
        assert!(
            ranges.iter().all(|item| !item.starts_with("bytes=0-")),
            "covered first chunk was requested again: {ranges:?}"
        );
        assert!(
            ranges.iter().any(|item| item.starts_with("bytes=65536-")),
            "remaining chunk was not requested: {ranges:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn range_request_sends_strong_etag_as_if_range() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_recording_headers(body, Arc::clone(&seen));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        run_job(&job).unwrap();
        let headers = seen.lock().unwrap_or_else(|err| err.into_inner()).clone();
        assert!(
            headers
                .iter()
                .any(|item| item == "if-range: \"native-test\""),
            "If-Range header missing: {headers:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn poisoned_etag_is_not_sent_as_if_range() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_recording_headers(body, Arc::clone(&seen));
        let (mut job, dir) = temp_job(&url, false, body.len() as u64, 2);
        job.etag = "\"native-test\"\r\nX-Injected: 1".into();
        run_job(&job).unwrap();
        let headers = seen.lock().unwrap_or_else(|err| err.into_inner()).clone();
        assert!(
            headers.iter().all(|item| !item.contains("x-injected")),
            "If-Range smuggled a header: {headers:?}"
        );
        assert!(
            headers.iter().all(|item| !item.starts_with("if-range:")),
            "poisoned ETag must not become If-Range: {headers:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn request_method_is_allowlisted() {
        assert_eq!(sanitize_http_method("POST"), "POST");
        assert_eq!(sanitize_http_method("head"), "HEAD");
        assert_eq!(sanitize_http_method("CONNECT"), "GET");
        assert_eq!(sanitize_http_method("GET\r\nHost: evil"), "GET");
        assert_eq!(sanitize_http_method(""), "GET");
        let mut job = Job {
            url: "http://127.0.0.1/".into(),
            headers: HashMap::new(),
            output: PathBuf::new(),
            connections: 1,
            chunk_bytes: 1,
            total: 0,
            sequential: false,
            resume_from: 0,
            proxy: String::new(),
            resource_key: String::new(),
            etag: String::new(),
            last_modified: String::new(),
            control: PathBuf::new(),
            progress: PathBuf::new(),
            method: "CONNECT".into(),
            body_path: PathBuf::new(),
            mirrors: Vec::new(),
            replay_json: String::new(),
            curl_body_limit: None,
            range_stop: None,
            installer_download: false,
            origin_boundary: String::new(),
        };
        assert_eq!(request_method(&job), "GET");
        job.method = "POST".into();
        assert_eq!(request_method(&job), "POST");
    }

    #[test]
    fn capped_range_responses_resume_at_the_next_byte() {
        let body: &'static [u8] = Box::leak(
            (0..70 * 1024)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_capped_ranges(body, 4096, Arc::clone(&seen));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 1);
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let starts = seen.lock().unwrap_or_else(|err| err.into_inner()).clone();
        assert_eq!(starts.first(), Some(&0));
        assert!(starts.windows(2).all(|pair| pair[1] > pair[0]));
        assert!(starts.contains(&(64 * 1024)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_range_responses_stop_after_bounded_retries() {
        let body: &'static [u8] = b"never-sent";
        let attempts = Arc::new(AtomicUsize::new(0));
        let url = serve_empty_ranges(body.len(), Arc::clone(&attempts));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 1);
        let err = run_job(&job).unwrap_err();
        assert!(matches!(&err, EngineError::Failed(_)));
        assert!(err.to_string().contains("made no progress"));
        assert_eq!(attempts.load(Ordering::SeqCst), MAX_RANGE_ATTEMPTS as usize);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn transient_429_backoff_recovers_without_duplicate_payload_bytes() {
        let body: &'static [u8] = Box::leak(
            (0..256 * 1024)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let payload_bytes = Arc::new(AtomicU64::new(0));
        let retry_times = Arc::new(Mutex::new(Vec::new()));
        let url = serve_transient_429_ranges(
            body,
            Arc::clone(&attempts),
            Arc::clone(&payload_bytes),
            Arc::clone(&retry_times),
        );
        let (job, dir) = temp_job(&url, false, body.len() as u64, 1);

        run_job(&job).unwrap();

        assert_eq!(fs::read(&job.output).unwrap(), body);
        assert!(attempts.load(Ordering::SeqCst) >= 6);
        assert_eq!(payload_bytes.load(Ordering::SeqCst), body.len() as u64);
        let times = retry_times.lock().unwrap();
        assert!(times[1].duration_since(times[0]) >= Duration::from_secs(1));
        assert!(times[2].duration_since(times[1]) >= Duration::from_secs(1));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn shared_retry_deadline_never_shortens_and_remains_interruptible() {
        let (job, dir) = temp_job("http://127.0.0.1/fixture", false, 1, 1);
        let scheduler = RangeScheduler::new(Vec::new(), 2);
        scheduler.defer_requests(Duration::from_secs(60));
        let deadline = *scheduler.retry_not_before.lock().unwrap();
        scheduler.defer_requests(Duration::from_secs(1));
        assert_eq!(*scheduler.retry_not_before.lock().unwrap(), deadline);
        let stop = AtomicBool::new(false);
        for shared_stop in [false, true] {
            stop.store(shared_stop, Ordering::SeqCst);
            fs::write(&job.control, "pause").unwrap();
            assert!(matches!(
                scheduler.wait_for_request(&job, &stop),
                Err(EngineErrorCode::Pause)
            ));
            assert!(matches!(
                wait_before_range_retry(&job, &stop, 1),
                Err(EngineErrorCode::Pause)
            ));
            fs::write(&job.control, "cancel").unwrap();
            assert!(matches!(
                scheduler.wait_for_request(&job, &stop),
                Err(EngineErrorCode::Cancel)
            ));
            assert!(matches!(
                wait_before_range_retry(&job, &stop, 1),
                Err(EngineErrorCode::Cancel)
            ));
        }
        fs::write(&job.control, "run").unwrap();
        assert!(matches!(
            scheduler.wait_for_request(&job, &stop),
            Err(EngineErrorCode::Cancel)
        ));
        assert!(matches!(
            wait_before_range_retry(&job, &stop, 1),
            Err(EngineErrorCode::Cancel)
        ));
        assert_eq!(
            parse_header_meta("Retry-After: 12\r\n")
                .unwrap()
                .retry_after,
            Some(Duration::from_secs(12))
        );
        assert!(parse_header_meta("Retry-After: invalid\r\n")
            .unwrap()
            .retry_after
            .is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_range_checkpoint_tracks_durable_cursor() {
        let body: &'static [u8] = b"durable-partial-range";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 1);
        let mut file =
            BufWriter::with_capacity(FILE_WRITE_BUFFER, File::create(&job.output).unwrap());
        file.write_all(&body[..8]).unwrap();
        let completed = RangeCheckpoints::new(Vec::new());
        persist_range_progress(&job, &mut file, &completed, 0, 8).unwrap();
        assert_eq!(load_completed_ranges(&job), Some(vec![(0, 7)]));
        assert_eq!(fs::read(&job.output).unwrap(), body[..8]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_checkpoint_prefixes_commit_together_and_failed_commit_stays_untrusted() {
        let body = b"two-range-prefix";
        let (job, dir) = temp_job("http://127.0.0.1/fixture", false, body.len() as u64, 2);
        let mut first = BufWriter::new(File::create(&job.output).unwrap());
        let mut second = BufWriter::new(OpenOptions::new().write(true).open(&job.output).unwrap());
        first.write_all(&body[..8]).unwrap();
        second.seek(SeekFrom::Start(8)).unwrap();
        second.write_all(&body[8..]).unwrap();
        second.flush().unwrap();
        let checkpoints = RangeCheckpoints::new(Vec::new());
        checkpoints
            .pending
            .lock()
            .unwrap()
            .push((8, body.len() as u64 - 1));
        persist_range_progress(&job, &mut first, &checkpoints, 0, 8).unwrap();
        assert_eq!(
            load_completed_ranges(&job),
            Some(vec![(0, body.len() as u64 - 1)])
        );
        assert_eq!(fs::read(&job.output).unwrap(), body);
        persist_range_progress(&job, &mut second, &checkpoints, 8, body.len() as u64).unwrap();
        assert!(checkpoints.pending.lock().unwrap().is_empty());

        fs::remove_file(completed_ranges_path(&job)).unwrap();
        fs::create_dir(completed_ranges_path(&job)).unwrap();
        let failed = RangeCheckpoints::new(Vec::new());
        assert!(persist_range_progress(&job, &mut first, &failed, 0, 8).is_err());
        assert!(failed.durable.lock().unwrap().is_empty());
        assert!(load_completed_ranges(&job).is_none());
        drop(first);
        drop(second);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mismatched_resource_checkpoint_is_discarded() {
        let body: &'static [u8] = Box::leak(
            (0..70 * 1024)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_recording_ranges(body, Arc::clone(&seen));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        let first_end = 64 * 1024 - 1;
        fs::write(&job.output, &body[..=first_end]).unwrap();
        fs::write(
            completed_ranges_path(&job),
            format!(
                r#"{{"version":2,"resource_key":"different-resource","etag":"\"native-test\"","total":{},"ranges":[[0,65535]]}}"#,
                job.total
            ),
        )
        .unwrap();
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let ranges = seen.lock().unwrap_or_else(|err| err.into_inner()).clone();
        assert!(
            ranges.iter().any(|item| item.starts_with("bytes=0-")),
            "stale checkpoint incorrectly skipped byte zero: {ranges:?}"
        );
        assert!(!completed_ranges_path(&job).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn truncated_checkpoint_downloads_missing_data_instead_of_publishing_zeroes() {
        let body: &'static [u8] = Box::leak(
            (0..70 * 1024)
                .map(|index| (index % 251 + 1) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_recording_ranges(body, Arc::clone(&seen));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.output, &body[..16 * 1024]).unwrap();
        save_completed_ranges(&job, &[(0, job.total - 1)]).unwrap();

        run_job(&job).unwrap();

        assert_eq!(fs::read(&job.output).unwrap(), body);
        assert!(!seen.lock().unwrap().is_empty());
        assert!(!completed_ranges_path(&job).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn known_size_without_strong_validator_probes_before_reusing_checkpoint() {
        let body: &'static [u8] = Box::leak(vec![42; 70 * 1024].into_boxed_slice());
        for etag in ["", "W/\"native-test\""] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let url = serve_recording_ranges(body, Arc::clone(&seen));
            let (mut job, dir) = temp_job(&url, false, body.len() as u64, 2);
            let prefix = 64 * 1024;
            fs::write(&job.output, &body[..prefix]).unwrap();
            save_completed_ranges(&job, &[(0, prefix as u64 - 1)]).unwrap();
            job.etag = etag.to_string();

            run_job(&job).unwrap();

            assert_eq!(fs::read(&job.output).unwrap(), body);
            let ranges = seen.lock().unwrap();
            assert_eq!(ranges.first().map(String::as_str), Some("bytes=0-0"));
            let transferred: usize = ranges
                .iter()
                .map(|range| {
                    let (start, end) = range
                        .strip_prefix("bytes=")
                        .unwrap()
                        .split_once('-')
                        .unwrap();
                    end.parse::<usize>().unwrap() - start.parse::<usize>().unwrap() + 1
                })
                .sum();
            assert_eq!(transferred, 1 + body.len() - prefix);
            assert!(!completed_ranges_path(&job).exists());
            drop(ranges);
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn completed_checkpoint_is_removed_without_network_work() {
        let body: &'static [u8] = b"already-complete-payload";
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_recording_ranges(body, Arc::clone(&seen));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.output, body).unwrap();
        fs::write(
            completed_ranges_path(&job),
            format!(
                r#"{{"version":2,"resource_key":"{}","etag":"\"native-test\"","total":{},"ranges":[[0,{}]]}}"#,
                job.resource_key,
                job.total,
                job.total - 1
            ),
        )
        .unwrap();
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        assert!(seen
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_empty());
        assert!(!completed_ranges_path(&job).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn range_success_path_transfers_zero_extra_payload_bytes() {
        let body: &'static [u8] = Box::leak(
            (0..70 * 1024)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let payload = Arc::new(AtomicU64::new(0));
        let url = serve_counting_range_payload(body, Arc::clone(&payload));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        let first_end = 64 * 1024 - 1;
        fs::write(&job.output, &body[..=first_end]).unwrap();
        fs::write(
            completed_ranges_path(&job),
            format!(
                r#"{{"version":2,"resource_key":"{}","etag":"\"native-test\"","total":{},"ranges":[[0,65535]]}}"#,
                job.resource_key, job.total
            ),
        )
        .unwrap();
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let transferred = payload.load(Ordering::SeqCst);
        assert_eq!(
            transferred,
            (body.len() - 65536) as u64,
            "covered prefix was downloaded again: {transferred}"
        );
        payload.store(0, Ordering::SeqCst);
        fs::write(
            completed_ranges_path(&job),
            format!(
                r#"{{"version":2,"resource_key":"{}","etag":"\"native-test\"","total":{},"ranges":[[0,{}]]}}"#,
                job.resource_key,
                job.total,
                job.total - 1
            ),
        )
        .unwrap();
        run_job(&job).unwrap();
        assert_eq!(
            payload.load(Ordering::SeqCst),
            0,
            "complete Range checkpoint must transfer 0 extra payload bytes"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn checkpoint_replace_keeps_latest_ranges() {
        let body: &'static [u8] = b"checkpoint-replace";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 1);
        save_completed_ranges(&job, &[(0, 3)]).unwrap();
        save_completed_ranges(&job, &[(0, 7)]).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(completed_ranges_path(&job)).unwrap())
                .unwrap();
        assert_eq!(saved["ranges"], serde_json::json!([[0, 7]]));
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn winhttp_reads_content_range_header() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 1);
        let mut fetched = winhttp::get(&job, &url, Some("bytes=4-9"), &[]).unwrap();
        let mut received = Vec::new();
        fetched.body.read_to_end(&mut received).unwrap();
        assert_eq!(fetched.status, 206);
        assert_eq!(fetched.content_range.as_deref(), Some("bytes 4-9/36"));
        assert_eq!(received, b"efghij");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sequential_job_deletes_range_sidecar() {
        let body: &'static [u8] = b"sequential-clears-sidecar";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, true, 0, 1);
        let sidecar = job.progress.with_file_name("native-engine.ranges.json");
        fs::write(&sidecar, r#"{"ranges":[[0,1]]}"#).unwrap();
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        assert!(!sidecar.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn post_job_stays_sequential_and_does_not_send_range() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let body: &'static [u8] = b"post-body-ok";
        let url = serve_recording_ranges(body, Arc::clone(&seen));
        let (mut job, dir) = temp_job(&url, false, body.len() as u64, 4);
        job.method = " Post ".into();
        job.total = 0;
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        assert!(seen
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    fn serve_full_body_ignoring_range(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
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
                    "HTTP/1.1 200 OK\r\nETag: \"changed\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    /// 模拟 Python http.server 一类"完全不支持 Range"的服务器：返回 200 整包且不给 ETag。
    fn serve_full_body_ignoring_range_without_etag(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
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
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[test]
    fn fresh_download_falls_back_to_sequential_when_server_ignores_range() {
        let body: &'static [u8] = b"range-less-server-payload-0123456789";
        let url = serve_full_body_ignoring_range_without_etag(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 3);
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn range_worker_preserves_the_only_full_download_response() {
        for (known_length, connections) in [(true, 1), (false, 1), (true, 2), (false, 2)] {
            let body: Vec<u8> = (0..192 * 1024).map(|index| (index % 251) as u8).collect();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!(
                "http://{}/export?token=a%2Fb+z",
                listener.local_addr().unwrap()
            );
            let served = body.clone();
            let server = thread::spawn(move || {
                let mut responses = Vec::new();
                let mut requests = Vec::new();
                // All workers reach the server before any can stop its peers.
                for _ in 0..connections {
                    let (stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream);
                    let mut request = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                            break;
                        }
                        request.push_str(&line);
                    }
                    requests.push(request);
                    responses.push(reader.into_inner());
                }
                drop(listener);
                let length = if known_length {
                    format!("Content-Length: {}\r\n", served.len())
                } else {
                    String::new()
                };
                for mut response in responses {
                    let _ = write!(response,
                        "HTTP/1.1 200 OK\r\n{length}ETag: \"native-test\"\r\nConnection: close\r\n\r\n");
                    let _ = response.write_all(&served);
                }
                requests
            });
            // Known length and a strong validator skip the initial Range probe.
            let (mut job, dir) = temp_job(&url, false, body.len() as u64, connections);
            job.headers
                .insert("Authorization".into(), "Bearer one-use".into());
            let result = run_job(&job);
            let requests = server.join().unwrap();
            let output = fs::read(&job.output).ok();
            fs::remove_dir_all(dir).unwrap();
            assert!(
                result.is_ok(),
                "known_length={known_length}, connections={connections}: {result:?}"
            );
            assert_eq!(output.as_deref(), Some(body.as_slice()));
            assert_eq!(requests.len(), connections);
            assert!(requests.iter().any(|request| request
                .to_ascii_lowercase()
                .contains("range: bytes=0-65535\r\n")));
            for request in requests {
                assert!(request.starts_with("GET /export?token=a%2Fb+z HTTP/1.1\r\n"));
                assert!(request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer one-use\r\n"));
            }
        }
    }

    #[test]
    fn range_worker_full_response_rejects_changed_validators() {
        for etag in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nETag: \"changed\"\r\nLast-Modified: Wed, 04 Nov 2026 00:00:00 GMT\r\nConnection: close\r\n\r\nnew!");
            });
            let (mut job, dir) = temp_job(&url, false, 4, 1);
            if !etag {
                job.etag.clear();
                job.last_modified = "Tue, 03 Nov 2026 00:00:00 GMT".into();
            }
            let result = run_job(&job);
            server.join().unwrap();
            let output = fs::read(&job.output).unwrap();
            fs::remove_dir_all(dir).unwrap();
            assert!(
                matches!(result, Err(EngineError::RangeUnsupported(_))),
                "{result:?}"
            );
            assert_eq!(output, vec![0; 4]);
        }
    }

    #[test]
    fn range_worker_full_response_honors_pause_and_cancel() {
        for command in ["pause", "cancel"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (job, dir) = temp_job(&url, false, 4, 1);
            let control = job.control.clone();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                fs::write(control, command).unwrap();
                let _ = reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nETag: \"native-test\"\r\nConnection: close\r\n\r\nbody");
            });
            let result = run_job(&job);
            server.join().unwrap();
            let output = fs::read(&job.output).unwrap();
            fs::remove_dir_all(dir).unwrap();
            assert!(
                matches!(
                    (&result, command),
                    (Err(EngineError::Pause), "pause") | (Err(EngineError::Cancel), "cancel")
                ),
                "{result:?}"
            );
            assert_eq!(output, vec![0; 4]);
        }
    }

    #[test]
    fn non_range_probe_preserves_the_only_download_response() {
        for known_length in [true, false] {
            let body: Vec<u8> = (0..192 * 1024).map(|index| (index % 251) as u8).collect();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!(
                "http://{}/export?token=a%2Fb+z",
                listener.local_addr().unwrap()
            );
            let served = body.clone();
            // The endpoint disappears after its first response, like a consumed token.
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                    request.push_str(&line);
                }
                let length = if known_length {
                    format!("Content-Length: {}\r\n", served.len())
                } else {
                    String::new()
                };
                let _ = write!(
                    reader.get_mut(),
                    "HTTP/1.1 200 OK\r\n{length}Connection: close\r\n\r\n"
                );
                let _ = reader.get_mut().write_all(&served);
                request
            });
            let (mut job, dir) = temp_job(&url, false, 0, 4);
            job.etag.clear();
            job.headers
                .insert("Authorization".into(), "Bearer one-use".into());
            let result = run_job(&job);
            let request = server.join().unwrap();
            let output = fs::read(&job.output).ok();
            fs::remove_dir_all(dir).unwrap();
            assert!(result.is_ok(), "known_length={known_length}: {result:?}");
            assert_eq!(output.as_deref(), Some(body.as_slice()));
            assert!(request.starts_with("GET /export?token=a%2Fb+z HTTP/1.1\r\n"));
            assert!(request
                .to_ascii_lowercase()
                .contains("range: bytes=0-0\r\n"));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer one-use\r\n"));
        }
    }

    #[test]
    fn invalid_checkpoint_restarts_sequentially_when_server_ignores_range() {
        let body: &'static [u8] = b"range-less-server-payload-0123456789";
        let url = serve_full_body_ignoring_range_without_etag(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.output, b"stale-prefix").unwrap();
        save_completed_ranges(&job, &[(0, job.total - 1)]).unwrap();

        run_job(&job).unwrap();

        assert_eq!(fs::read(&job.output).unwrap(), body);
        assert!(!completed_ranges_path(&job).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_download_still_fails_when_server_ignores_range() {
        // 已有保留片段时，200 整包无法按续传偏移拼接，必须继续失败而不是污染文件。
        let body: &'static [u8] = b"range-less-server-payload-0123456789";
        let url = serve_full_body_ignoring_range_without_etag(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.output, &body[..8]).unwrap();
        fs::write(
            completed_ranges_path(&job),
            format!(
                r#"{{"version":2,"resource_key":"{}","etag":"\"native-test\"","total":{},"ranges":[[0,7]]}}"#,
                job.resource_key, job.total
            ),
        )
        .unwrap();
        let err = run_job(&job).unwrap_err();
        assert!(matches!(&err, EngineError::RangeUnsupported(_)));
        let output = fs::read(&job.output).unwrap();
        // download_ranges 会先 set_len(total) 预分配，所以文件长度就是 total；
        // 关键是已保留前缀未被 200 整包覆盖，其余部分仍是零填充。
        assert_eq!(output.len(), body.len());
        assert_eq!(&output[..8], &body[..8], "reserved prefix must survive");
        assert!(
            output[8..].iter().all(|byte| *byte == 0),
            "200 body must not be stitched into the reserved file: {output:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn etag_change_range_200_does_not_stitch_new_body() {
        let old: &'static [u8] = b"old-payload-aaaaaaaaaaaaaaaaaaaa";
        let new_body: &'static [u8] = b"NEW-PAYLOAD-bbbbbbbbbbbbbbbbbbbb";
        let url = serve_full_body_ignoring_range(new_body);
        let (job, dir) = temp_job(&url, false, old.len() as u64, 2);
        let prefix = &old[..16];
        fs::write(&job.output, prefix).unwrap();
        fs::write(
            completed_ranges_path(&job),
            format!(
                r#"{{"version":2,"resource_key":"{}","etag":"\"native-test\"","total":{},"ranges":[[0,15]]}}"#,
                job.resource_key, job.total
            ),
        )
        .unwrap();
        let err = run_job(&job).unwrap_err();
        assert!(matches!(&err, EngineError::RangeUnsupported(_)));
        let output = fs::read(&job.output).unwrap();
        assert_eq!(&output[..prefix.len()], prefix);
        assert_ne!(
            output.as_slice(),
            new_body,
            "If-Range miss must not replace the reserved payload with a 200 body"
        );
        assert!(
            !output
                .windows(b"NEW-PAYLOAD".len())
                .any(|window| window == b"NEW-PAYLOAD"),
            "If-Range miss stitched the new identity into the reserved file: {output:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn changed_206_identity_never_stitches_into_a_saved_prefix() {
        for (validator, response_validator, changed) in [
            ("etag", "\"new-entity\"", true),
            ("etag", "W/\"native-test\"", true),
            ("etag", "\"native-test\"", false),
            ("etag", "", false),
            ("modified", "Thu, 22 Oct 2026 10:00:00 GMT", true),
            ("modified", "Wed, 21 Oct 2026 10:00:00 GMT", false),
            ("modified", "", false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
            let (job, dir) = temp_job(&url, false, 96 * 1024, 1);
            let mut job = job;
            let prefix_len = 64 * 1024;
            let old_prefix = vec![b'a'; prefix_len];
            let tail = vec![b'b'; 32 * 1024];
            if validator == "modified" {
                job.etag.clear();
                job.last_modified = "Wed, 21 Oct 2026 10:00:00 GMT".into();
            }
            fs::write(&job.output, &old_prefix).unwrap();
            save_completed_ranges(&job, &[(0, prefix_len as u64 - 1)]).unwrap();
            let server_tail = tail.clone();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    headers.push_str(&line);
                }
                assert!(headers.contains("Range: bytes=65536-98303"), "{headers}");
                assert!(headers.contains("If-Range:"), "{headers}");
                let field = if validator == "etag" {
                    "ETag"
                } else {
                    "Last-Modified"
                };
                let header = if response_validator.is_empty() {
                    String::new()
                } else {
                    format!("{field}: {response_validator}\r\n")
                };
                let mut stream = reader.into_inner();
                let _ = write!(stream, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 65536-98303/98304\r\nContent-Length: 32768\r\n{header}Connection: close\r\n\r\n");
                let _ = stream.write_all(&server_tail);
            });
            let result = run_job(&job);
            server.join().unwrap();
            let output = fs::read(&job.output).unwrap();
            assert_eq!(&output[..prefix_len], old_prefix);
            if changed {
                assert!(matches!(result, Err(EngineError::RangeUnsupported(ref message)) if message.starts_with("resource identity changed:")),
                    "{validator}/{response_validator}: changed entity must not be accepted: {result:?}");
                assert!(
                    output[prefix_len..].iter().all(|byte| *byte == 0),
                    "changed entity bytes were written"
                );
                assert_eq!(
                    load_completed_ranges(&job),
                    Some(vec![(0, prefix_len as u64 - 1)])
                );
            } else {
                result.unwrap();
                assert_eq!(&output[prefix_len..], tail);
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn mirrors_fall_back_when_primary_fails() {
        let body: &'static [u8] = b"mirror-payload";
        let good = serve_body(body);
        let (mut job, dir) = temp_job(
            "http://127.0.0.1:1/missing.bin",
            false,
            body.len() as u64,
            1,
        );
        job.mirrors = vec![good];
        job.sequential = true;
        job.total = 0;
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_size_probe_enables_range_workers() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = serve_recording_ranges(body, Arc::clone(&seen));
        let (mut job, dir) = temp_job(&url, false, 0, 3);
        job.etag.clear();
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let ranges = seen.lock().unwrap_or_else(|err| err.into_inner()).clone();
        assert!(
            ranges.iter().any(|item| item == "bytes=0-0"),
            "probe Range missing: {ranges:?}"
        );
        assert!(
            ranges
                .iter()
                .any(|item| item.starts_with("bytes=") && item.as_str() != "bytes=0-0"),
            "Range workers never ran: {ranges:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mirrors_skip_identity_mismatch() {
        let good: &'static [u8] = b"0123456789";
        let bad: &'static [u8] = b"nope";
        let bad_url = serve_body(bad);
        let good_url = serve_body(good);
        let (mut job, dir) = temp_job(
            "http://127.0.0.1:1/missing.bin",
            false,
            good.len() as u64,
            1,
        );
        job.mirrors = vec![bad_url, good_url];
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), good);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn browser_profile_fills_chrome_ua() {
        let mut headers = HashMap::new();
        apply_browser_profile(&mut headers);
        assert!(headers.get("User-Agent").unwrap().contains("Chrome/146"));
        assert_eq!(headers.get("Accept").map(String::as_str), Some("*/*"));
        headers.insert("User-Agent".into(), "CustomAgent/1".into());
        apply_browser_profile(&mut headers);
        assert_eq!(headers.get("User-Agent").unwrap(), "CustomAgent/1");
    }

    #[test]
    fn cloudflare_403_retries_without_cf_bm() {
        check_cloudflare_replay_retry("http");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires bundled curl-impersonate; isolated loopback fixture"]
    fn curl_transport_403_retries_replayed_cookies() {
        assert!(curl_impersonate_exe().is_some(), "bundled curl required");
        check_cloudflare_replay_retry("curl");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Run explicitly in Windows Sandbox"]
    fn winhttp_403_retries_replayed_cookies() {
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
        check_cloudflare_replay_retry("winhttp");
    }

    fn check_cloudflare_replay_retry(transport: &str) {
        for context in [
            "headers",
            "cookie",
            "request_headers",
            "scoped",
            "cookie_jar",
        ] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let recorded = Arc::clone(&seen);
            thread::spawn(move || {
                while let Ok((stream, _)) = listener.accept() {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        continue;
                    }
                    let mut cookie = String::new();
                    let mut ua = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                            break;
                        }
                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("cookie:") {
                            cookie = value.trim().to_string();
                        }
                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("user-agent:") {
                            ua = value.trim().to_string();
                        }
                    }
                    recorded
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .push((cookie.clone(), ua));
                    let mut stream = reader.into_inner();
                    if cookie.contains("__cf_bm") {
                        let _ = stream.write_all(
                            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    } else {
                        let body = b"ok";
                        let header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(header.as_bytes());
                        let _ = stream.write_all(body);
                    }
                }
            });
            let url = format!("http://127.0.0.1:{}/file.bin", addr.port());
            let mut headers = HashMap::new();
            headers.insert(
                "Cookie".into(),
                "__cf_bm=stale; __cflb=pool-a; session=ok".into(),
            );
            let (mut job, dir) = temp_job(&url, true, 0, 1);
            job.headers = headers.clone();
            if context != "headers" {
                let cookie = headers.get("Cookie").unwrap();
                let origin = crate::credentials::request_origin(&url);
                let value = match context {
                    "cookie" => serde_json::json!({"cookie": cookie}),
                    "request_headers" => serde_json::json!({"request_headers": {"Cookie": cookie}}),
                    "scoped" => {
                        serde_json::json!({"request_contexts": {origin: {"cookie": cookie}}})
                    }
                    "cookie_jar" => serde_json::json!({"request_contexts": {origin: {"cookies": [
                        {"name":"__cf_bm", "value":"stale", "domain":"127.0.0.1", "path":"/"},
                        {"name":"__cflb", "value":"pool-a", "domain":"127.0.0.1", "path":"/"},
                        {"name":"session", "value":"ok", "domain":"127.0.0.1", "path":"/"}
                    ]}}}),
                    _ => unreachable!(),
                };
                job.replay_json =
                    crate::credentials::bind_replay_source_url(&value.to_string(), &url);
            }
            let replay = job.replay_json.clone();
            if transport == "winhttp" {
                job.proxy = format!("http://{addr}");
            }
            let result = if transport == "curl" {
                fetch_curl_follow(&job, None).expect("curl transport available")
            } else {
                fetch_follow(&job, None)
            };
            let mut fetched =
                result.unwrap_or_else(|error| panic!("{transport}/{context}: {error:?}"));
            assert_eq!(fetched.status, 200, "{transport}/{context}");
            let mut body = Vec::new();
            fetched.body.read_to_end(&mut body).unwrap();
            assert_eq!(body, b"ok", "{transport}/{context}");
            assert_eq!(
                job.replay_json, replay,
                "saved replay must remain unchanged"
            );
            let seen = seen.lock().unwrap_or_else(|err| err.into_inner());
            assert_eq!(seen.len(), 2, "{transport}/{context}");
            assert!(seen[0].0.contains("__cf_bm"));
            assert!(!seen[1].0.contains("__cf_bm"));
            assert!(seen[1].0.contains("__cflb=pool-a"));
            assert!(seen[1].0.contains("session=ok"));
            assert!(seen[0].1.contains("chrome/146") || seen[0].1.contains("Chrome/146"));
            let stripped = strip_stale_cloudflare_cookies(&headers).unwrap();
            assert_eq!(stripped.get("Cookie").unwrap(), "__cflb=pool-a; session=ok");
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires bundled curl-impersonate; isolated loopback fixture"]
    fn curl_transport_preserves_browser_cookies_and_rejects_truncated_body() {
        assert!(
            curl_impersonate_exe().is_some(),
            "bundled curl-impersonate required"
        );
        for case in ["complete", "truncated", "trailer", "range-overflow"] {
            let truncated = case == "truncated";
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/media.ts", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                    request.push_str(&line);
                }
                let response: &[u8] = if truncated {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nok"
                } else if case == "trailer" {
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nETag: initial\r\nTrailer: ETag\r\n\r\n2\r\nok\r\n0\r\nETag: trailer\r\n\r\n"
                } else if case == "range-overflow" {
                    b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-18446744073709551615/*\r\nConnection: close\r\n\r\nok"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                };
                reader.get_mut().write_all(response).unwrap();
                request
            });
            let (mut job, dir) = temp_job(&url, true, 0, 1);
            // 使用下载时才解密注入的请求上下文，而不只检查预先填好的 Job 头。
            job.replay_json = serde_json::json!({
                "cookie": "__cf_bm=valid; __cflb=pool-a; session=ok",
                "referer": "https://page.test/watch",
                "user_agent": "Browser fixture",
            })
            .to_string();
            let result = fetch_curl_follow(&job, None).expect("curl transport available");
            let request = server.join().unwrap().to_ascii_lowercase();
            assert!(
                request.contains("cookie: __cf_bm=valid; __cflb=pool-a; session=ok\r\n"),
                "{request}"
            );
            assert!(
                request.contains("referer: https://page.test/watch\r\n"),
                "{request}"
            );
            if case == "range-overflow" {
                assert!(
                    matches!(result, Err(EngineError::Failed(ref error)) if error.contains("Content-Range length overflow"))
                );
            } else if truncated {
                let mut fetched = result.unwrap();
                assert!(
                    fetched.body.read_to_end(&mut Vec::new()).is_err(),
                    "HTTP 200 with an incomplete body must fail"
                );
            } else {
                let mut fetched = result.unwrap();
                assert_eq!(fetched.status, 200);
                if case == "trailer" {
                    assert_eq!(fetched.etag, "initial");
                }
                let mut body = Vec::new();
                fetched.body.read_to_end(&mut body).unwrap();
                assert_eq!(body, b"ok");
                let Body::Curl(body) = &fetched.body else {
                    panic!("expected curl stream");
                };
                let temporary = body.dir.clone();
                drop(fetched);
                assert!(
                    !temporary.exists(),
                    "successful curl temp directory must be released"
                );
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires bundled curl-impersonate; isolated loopback fixture"]
    fn curl_transport_range_body_limit_uses_decoded_bytes_and_stops_before_eof() {
        use std::sync::mpsc;
        assert!(curl_impersonate_exe().is_some(), "bundled curl required");
        for case in [
            "gzip-valid",
            "gzip-bad-checksum",
            "gzip-oversize",
            "chunked-oversize",
            "length-oversize",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/media.ts", listener.local_addr().unwrap());
            let (ready_tx, ready_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let mut stream = reader.into_inner();
                // 固定 gzip 夹具：线上压缩长度大于范围长度，合法响应不可误判。
                let gzip_valid: &[u8] = &[
                    31, 139, 8, 0, 0, 0, 0, 0, 2, 10, 51, 52, 50, 54, 49, 53, 51, 183, 0, 0, 175,
                    218, 224, 154, 8, 0, 0, 0,
                ];
                let gzip_oversize: &[u8] = &[
                    31, 139, 8, 0, 0, 0, 0, 0, 2, 10, 51, 52, 50, 54, 49, 53, 51, 183, 176, 4, 0,
                    38, 57, 244, 203, 9, 0, 0, 0,
                ];
                let mut bad_checksum = gzip_valid.to_vec();
                bad_checksum[20] ^= 1;
                let (framing, body) = match case {
                    "gzip-valid" => (
                        "Content-Encoding: gzip\r\nContent-Length: 28\r\n",
                        gzip_valid,
                    ),
                    "gzip-oversize" => (
                        "Content-Encoding: gzip\r\nContent-Length: 29\r\n",
                        gzip_oversize,
                    ),
                    "gzip-bad-checksum" => (
                        "Content-Encoding: gzip\r\nContent-Length: 28\r\n",
                        bad_checksum.as_slice(),
                    ),
                    "chunked-oversize" => {
                        ("Transfer-Encoding: chunked\r\n", &b"9\r\n123456789\r\n"[..])
                    }
                    _ => ("Content-Length: 64\r\n", &b"123456789"[..]),
                };
                write!(stream, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-7/8\r\n{framing}Connection: close\r\n\r\n").unwrap();
                stream.write_all(body).unwrap();
                ready_tx.send(()).unwrap();
                if !case.starts_with("gzip-") {
                    let _ = release_rx.recv();
                }
            });
            let (mut job, dir) = temp_job(&url, true, 0, 1);
            job.curl_body_limit = Some(8);
            let worker_job = job.clone();
            let (done_tx, done_rx) = mpsc::channel();
            let worker_range = if case.starts_with("gzip-") {
                None
            } else {
                Some("bytes=0-7")
            };
            let worker = thread::spawn(move || {
                let result = fetch_via_curl_impersonate(&worker_job, worker_range, &[])
                    .unwrap()
                    .and_then(|mut fetched| {
                        let mut bytes = Vec::new();
                        fetched
                            .body
                            .read_to_end(&mut bytes)
                            .map_err(|error| EngineError::Failed(error.to_string()))?;
                        Ok((fetched.content_length, bytes))
                    });
                let _ = done_tx.send(result);
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let result = done_rx.recv_timeout(Duration::from_secs(3));
            let _ = release_tx.send(());
            server.join().unwrap();
            worker.join().unwrap();
            let result = result.expect("curl must reject oversize before EOF");
            if case == "gzip-valid" {
                let (length, body) = result.unwrap();
                assert_eq!(body, b"12345678");
                assert_eq!(length, None);
            } else {
                let error = match result {
                    Err(error) => error,
                    Ok(_) => panic!("{case}: oversized response accepted"),
                };
                assert!(
                    error.to_string().contains(if case == "gzip-bad-checksum" {
                        "transfer failed"
                    } else {
                        "byte range response body exceeds 8"
                    }),
                    "{case}: {error}"
                );
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires bundled curl-impersonate; isolated loopback fixture"]
    fn curl_transport_stalled_headers_and_body_obey_pause_and_cancel() {
        use std::sync::mpsc;
        assert!(
            curl_impersonate_exe().is_some(),
            "bundled curl-impersonate required"
        );
        for action in ["pause", "cancel"] {
            for phase in ["headers", "body"] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let url = format!("http://{}/media.ts", listener.local_addr().unwrap());
                let (ready_tx, ready_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let server = thread::spawn(move || {
                    let (stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                            break;
                        }
                    }
                    let mut stream = reader.into_inner();
                    let response: &[u8] = if phase == "headers" {
                        b"HTTP/1.1 200 OK\r\n"
                    } else {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc"
                    };
                    stream.write_all(response).unwrap();
                    ready_tx.send(()).unwrap();
                    let _ = release_rx.recv();
                });
                let (job, dir) = temp_job(&url, true, 0, 1);
                let worker_job = job.clone();
                let (done_tx, done_rx) = mpsc::channel();
                let worker = thread::spawn(move || {
                    let result = fetch_via_curl_impersonate(&worker_job, None, &[])
                        .unwrap()
                        .and_then(|mut fetched| {
                            fetched
                                .body
                                .read_to_end(&mut Vec::new())
                                .map(|_| ())
                                .map_err(|error| {
                                    check_job_control(&worker_job)
                                        .err()
                                        .unwrap_or_else(|| EngineError::Failed(error.to_string()))
                                })
                        });
                    let _ = done_tx.send(result);
                });
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                fs::write(&job.control, action).unwrap();
                // 服务端只有收到释放信号才结束；结果必须在网络释放之前返回。
                let result = done_rx.recv_timeout(Duration::from_secs(3));
                let _ = release_tx.send(());
                server.join().unwrap();
                worker.join().unwrap();
                let error = match result.expect("stalled curl did not stop promptly") {
                    Err(error) => error,
                    Ok(_) => panic!("control signal was ignored"),
                };
                assert!(
                    matches!(
                        (&error, action),
                        (EngineError::Pause, "pause") | (EngineError::Cancel, "cancel")
                    ),
                    "{phase}/{action}: {error}"
                );
                fs::remove_dir_all(dir).unwrap();
            }
        }
    }

    #[test]
    fn hls_entity_fetch_rejects_a_truncated_successful_http_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/media.ts", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
            }
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });
        let result = fetch_hls_bytes(&url, &HashMap::new(), "");
        server.join().unwrap();
        assert!(
            result.is_err(),
            "a truncated media entity cannot be published as a complete segment"
        );
    }

    #[test]
    fn media_entity_stop_preserves_pause_and_cancel() {
        media_entity_stop(false);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Run explicitly in Windows Sandbox"]
    fn winhttp_media_entity_stop_preserves_pause_and_cancel() {
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
        media_entity_stop(true);
    }

    fn media_entity_stop(via_proxy: bool) {
        use std::sync::mpsc;
        for ranged in [false, true] {
            for action in ["pause", "cancel"] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let address = listener.local_addr().unwrap();
                let (ready_tx, ready_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let server = thread::spawn(move || {
                    let (stream, _) = listener.accept().unwrap();
                    let mut reader = BufReader::new(stream);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                            break;
                        }
                    }
                    let response: &[u8] = if ranged {
                        b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-5/6\r\nContent-Length: 6\r\n\r\nabc"
                    } else {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc"
                    };
                    reader.get_mut().write_all(response).unwrap();
                    ready_tx.send(()).unwrap();
                    let _ = release_rx.recv();
                });
                let (job, dir) = temp_job(&format!("http://{address}/media.ts"), true, 0, 1);
                let control = job.control.clone();
                let proxy = if via_proxy {
                    format!("http://{address}")
                } else {
                    String::new()
                };
                let url = if via_proxy {
                    "http://hls-control.example/media.ts".to_string()
                } else {
                    job.url
                };
                let (done_tx, done_rx) = mpsc::channel();
                let worker = thread::spawn(move || {
                    let context = crate::net_policy::ThrottleContext {
                        control,
                        ..Default::default()
                    };
                    let result = crate::net_policy::with_throttle_context(Some(context), || {
                        if ranged {
                            fetch_hls_bytes_range(&url, &HashMap::new(), &proxy, 0, 6)
                        } else {
                            fetch_hls_bytes(&url, &HashMap::new(), &proxy).map(|(_, bytes)| bytes)
                        }
                    });
                    let _ = done_tx.send(result);
                });
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                fs::write(&job.control, action).unwrap();
                let result = done_rx.recv_timeout(Duration::from_secs(3));
                let _ = release_tx.send(());
                server.join().unwrap();
                worker.join().unwrap();
                let error = result
                    .expect("media entity did not stop before server release")
                    .unwrap_err();
                assert!(
                    matches!(
                        (&error, action),
                        (EngineError::Pause, "pause") | (EngineError::Cancel, "cancel")
                    ),
                    "range={ranged}, proxy={via_proxy}, action={action}: {error}"
                );
                fs::remove_dir_all(dir).unwrap();
            }
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Run explicitly in Windows Sandbox"]
    fn winhttp_proxy_downloads_ranges_posts_and_head_without_changing_bytes() {
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
        let bytes: &'static [u8] = Box::leak(
            (0..180_000)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        for sequential in [true, false] {
            let proxy = serve_body(bytes);
            let (mut job, dir) = temp_job(
                "http://hls-control.example/file.bin",
                sequential,
                bytes.len() as u64,
                4,
            );
            job.proxy = proxy;
            run_job(&job).unwrap();
            assert_eq!(fs::read(&job.output).unwrap(), bytes);
            fs::remove_dir_all(dir).unwrap();
        }
        for method in ["POST", "HEAD"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                assert!(first.starts_with(method));
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = header_value(&line, "Content-Length") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                if method == "POST" {
                    let mut sent = vec![0; length];
                    reader.read_exact(&mut sent).unwrap();
                    assert_eq!(sent, b"payload=actual-request");
                }
                reader
                    .get_mut()
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n")
                    .unwrap();
                if method == "POST" {
                    reader.get_mut().write_all(b"ok").unwrap();
                }
            });
            let (mut job, dir) = temp_job("http://hls-control.example/method", true, 0, 1);
            job.proxy = format!("http://{address}");
            job.method = method.into();
            if method == "POST" {
                job.body_path = dir.join("request-body");
                fs::write(&job.body_path, b"payload=actual-request").unwrap();
            }
            let result = run_job(&job);
            server.join().unwrap();
            result.unwrap();
            assert_eq!(
                fs::read(&job.output).unwrap(),
                if method == "POST" {
                    b"ok".as_slice()
                } else {
                    b""
                }
            );
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn curl_impersonate_uses_the_bundled_v7_layout() {
        let root = std::env::temp_dir().join(format!(
            "hls-v7-curl-layout-{}-{}",
            std::process::id(),
            NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let tool = root
            .join("tools")
            .join("curl-impersonate")
            .join("curl-impersonate.exe");
        fs::create_dir_all(tool.parent().unwrap()).unwrap();
        fs::write(&tool, b"fixture").unwrap();
        assert_eq!(
            curl_impersonate_near(&root).as_deref(),
            Some(tool.as_path())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires bundled curl-impersonate; isolated loopback fixture"]
    fn curl_transport_streams_before_eof_and_drop_releases_the_connection() {
        use std::sync::mpsc;
        assert!(curl_impersonate_exe().is_some(), "bundled curl required");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/media.ts", listener.local_addr().unwrap());
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            drop(reader);
            stream.write_all(b"HTTP/1.1 103 Early Hints\r\nETag: ignored\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: final\r\n\r\nabc").unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            stream.read(&mut [0]).unwrap_or(0)
        });
        let (job, dir) = temp_job(&url, true, 0, 1);
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut fetched = fetch_curl_follow(&job, None).unwrap().unwrap();
            let mut first = [0; 3];
            fetched.body.read_exact(&mut first).unwrap();
            let Body::Curl(body) = &fetched.body else {
                panic!("expected curl pipe");
            };
            let temporary = body.dir.clone();
            let status = fetched.status;
            let etag = fetched.etag.clone();
            drop(fetched);
            let _ = done_tx.send((status, etag, first, temporary.exists()));
        });
        let result = done_rx.recv_timeout(Duration::from_secs(3));
        let _ = release_tx.send(());
        let remaining = server.join().unwrap();
        worker.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
        let (status, etag, first, temporary_exists) =
            result.expect("must stream before server EOF");
        assert_eq!(status, 200);
        assert_eq!(etag, "final");
        assert_eq!(&first, b"abc");
        assert!(!temporary_exists);
        assert_eq!(
            remaining, 0,
            "early drop must close curl's server connection"
        );
    }

    /// Fake curl for the browser-identity pipe: the last argument's final path
    /// segment names a case directory beside the script holding `head` (the
    /// dumped headers), `chunk` × `count` (stdout body) and `tail` (`hang`
    /// execs a long sleep so the pid stays the body writer; `wait` exits once
    /// `release` exists). The script is written once so concurrent forks in
    /// other tests cannot keep a write handle open on it (ETXTBSY).
    #[cfg(unix)]
    fn fake_curl_script() -> &'static Path {
        static SCRIPT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        SCRIPT.get_or_init(|| {
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!("hls-fake-curl-{}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            let script = root.join("curl");
            fs::write(
                &script,
                "#!/bin/sh\n\
                 h=; u=; p=\n\
                 for a in \"$@\"; do [ \"$p\" = \"--dump-header\" ] && h=\"$a\"; p=\"$a\"; u=\"$a\"; done\n\
                 d=\"$(dirname \"$0\")/${u##*/}\"\n\
                 echo $$ > \"$d/pid\"\n\
                 cat \"$d/head\" > \"$h\"\n\
                 n=$(cat \"$d/count\"); i=0\n\
                 while [ \"$i\" -lt \"$n\" ]; do cat \"$d/chunk\"; i=$((i+1)); done\n\
                 case \"$(cat \"$d/tail\")\" in\n\
                 hang) exec sleep 30 ;;\n\
                 wait) while [ ! -f \"$d/release\" ]; do sleep 0.02; done ;;\n\
                 esac\n\
                 exit 0\n",
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            thread::sleep(Duration::from_millis(100));
            script
        })
    }

    /// Native requests to this origin get 403, so `fetch` falls back to curl,
    /// which the override routes to the fake script.
    #[cfg(unix)]
    fn fake_curl_case(head: &str, chunk: &[u8], count: usize, tail: &str) -> (String, PathBuf) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!(
            "http://127.0.0.1:{}/",
            listener.local_addr().unwrap().port()
        );
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = reader.into_inner().write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let script = fake_curl_script();
        let name = format!("case-{}", NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed));
        let case = script.parent().unwrap().join(&name);
        fs::create_dir_all(&case).unwrap();
        fs::write(case.join("head"), head).unwrap();
        fs::write(case.join("chunk"), chunk).unwrap();
        fs::write(case.join("count"), count.to_string()).unwrap();
        fs::write(case.join("tail"), tail).unwrap();
        TEST_CURL_OVERRIDES
            .lock()
            .unwrap()
            .push((origin.clone(), script.to_path_buf()));
        (format!("{origin}{name}"), case)
    }

    #[cfg(unix)]
    fn pid_alive(case: &Path) -> bool {
        let pid = fs::read_to_string(case.join("pid")).unwrap();
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[cfg(unix)]
    fn wait_for_len(path: &Path, len: u64) -> bool {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if fs::metadata(path).is_ok_and(|meta| meta.len() >= len) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[cfg(unix)]
    #[test]
    fn curl_pipe_streams_before_exit_and_rejects_short_entity() {
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 8192\r\nETag: \"native-test\"\r\n\r\n";
        let (url, case) = fake_curl_case(head, &[7u8; 4096], 1, "wait");
        let (job, dir) = temp_job(&url, true, 0, 1);
        let output = job.output.clone();
        let worker = thread::spawn(move || run_job(&job));
        // Half the declared body is on disk while curl is still running.
        assert!(
            wait_for_len(&output, 4096),
            "first chunk not streamed to disk"
        );
        assert!(pid_alive(&case), "curl must still be running");
        assert!(!worker.is_finished());
        // curl then exits cleanly with only half the declared entity.
        fs::write(case.join("release"), "").unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert!(
            error.to_string().contains("truncated") || error.to_string().contains("mismatch"),
            "{error}"
        );
        fs::remove_dir_all(dir).unwrap();
        let _ = fs::remove_dir_all(case);
    }

    #[cfg(unix)]
    #[test]
    fn curl_pipe_streams_full_entity_and_validates_exit() {
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 12288\r\nETag: \"native-test\"\r\n\r\n";
        let (url, case) = fake_curl_case(head, &[3u8; 4096], 3, "wait");
        let (job, dir) = temp_job(&url, true, 0, 1);
        let output = job.output.clone();
        let worker = thread::spawn(move || run_job(&job));
        // The final bytes are held until curl's exit status is known, but the
        // earlier ones are already written.
        assert!(wait_for_len(&output, 8192), "body not streamed before exit");
        assert!(pid_alive(&case));
        fs::write(case.join("release"), "").unwrap();
        worker.join().unwrap().unwrap();
        assert_eq!(fs::read(&output).unwrap(), vec![3u8; 12288]);
        fs::remove_dir_all(dir).unwrap();
        let _ = fs::remove_dir_all(case);
    }

    #[cfg(unix)]
    #[test]
    fn curl_pipe_obeys_task_rate_limit_per_chunk() {
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 49152\r\n\r\n";
        let (url, case) = fake_curl_case(head, &[9u8; 8192], 6, "none");
        let (job, dir) = temp_job(&url, true, 0, 1);
        let task_id = format!(
            "fake-curl-{}",
            NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed)
        );
        crate::net_policy::configure_scoped_limit(&format!("task:{task_id}"), 32);
        let context = crate::net_policy::ThrottleContext {
            task_id: task_id.clone(),
            control: job.control.clone(),
            ..Default::default()
        };
        let started = Instant::now();
        let result = crate::net_policy::with_throttle_context(Some(context), || run_job(&job));
        let elapsed = started.elapsed();
        crate::net_policy::clear_scoped_limit(&format!("task:{task_id}"));
        result.unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), vec![9u8; 49152]);
        // 48 KiB at 32 KiB/s from an empty bucket takes ~1.5 s; the fake curl
        // itself finishes immediately, so the wait is the limiter's.
        assert!(elapsed >= Duration::from_millis(1200), "{elapsed:?}");
        fs::remove_dir_all(dir).unwrap();
        let _ = fs::remove_dir_all(case);
    }

    #[cfg(unix)]
    #[test]
    fn curl_pipe_cancel_mid_stream_kills_curl_and_frees_slot() {
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 65536\r\n\r\n";
        let (url, case) = fake_curl_case(head, &[1u8; 8192], 1, "hang");
        let (job, dir) = temp_job(&url, true, 0, 1);
        let control = job.control.clone();
        let output = job.output.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let _ = done_tx.send(run_job(&job));
        });
        assert!(wait_for_len(&output, 8192), "chunk not streamed");
        assert!(pid_alive(&case));
        fs::write(&control, "cancel").unwrap();
        let result = done_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("cancel must not wait for curl to finish");
        worker.join().unwrap();
        assert!(matches!(result, Err(EngineError::Cancel)), "{result:?}");
        assert!(!pid_alive(&case), "curl child must be killed and reaped");
        // The task's curl slot is free again: a cap-1 acquire succeeds at once.
        let permit = thread::spawn(move || {
            crate::curl_slots::acquire(&control, 1, usize::MAX, || Err::<(), _>(()))
        })
        .join()
        .unwrap();
        assert!(permit.is_ok(), "curl slot leaked after cancel");
        drop(permit);
        fs::remove_dir_all(dir).unwrap();
        let _ = fs::remove_dir_all(case);
    }

    #[cfg(unix)]
    #[test]
    fn curl_pipe_identity_mismatch_writes_nothing() {
        let head = "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-8195/8196\r\nContent-Length: 8192\r\nETag: \"changed\"\r\n\r\n";
        let (url, case) = fake_curl_case(head, &[5u8; 8192], 1, "none");
        let (mut job, dir) = temp_job(&url, true, 8196, 1);
        fs::write(&job.output, b"keep").unwrap();
        job.resume_from = 4;
        let error = download_sequential(&job, None).unwrap_err();
        assert!(matches!(error, EngineError::RangeUnsupported(_)), "{error}");
        assert_eq!(fs::read(&job.output).unwrap(), b"keep");
        fs::remove_dir_all(dir).unwrap();
        let _ = fs::remove_dir_all(case);
    }

    #[test]
    fn probe_206_unknown_total_falls_back_to_sequential_instead_of_one_byte_total() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/*\r\nContent-Length: 1\r\nConnection: close\r\n\r\nX"
                );
            }
        });
        let (job, dir) = temp_job(&format!("http://{addr}/stream.bin"), false, 0, 4);
        let (probe, _fetched) = probe_resource_response(&job).unwrap();
        server.join().unwrap();
        assert_eq!(
            probe.total, None,
            "206 with Content-Range bytes 0-0/* must not treat 1-byte Content-Length as total size"
        );
        assert!(
            !probe.accept_ranges,
            "206 with unknown total length must disable parallel range splitting"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn curl_proxy_for_system_lookup_covers_all_variants() {
        assert_eq!(
            curl_proxy_for_system_lookup(Ok(None)),
            Some(CurlProxyPlan::Direct)
        );
        for url in [
            "http://127.0.0.1:7890",
            "HTTP://proxy.local:8080",
            "socks4://127.0.0.1:1080",
            "socks4a://127.0.0.1:1080",
            "socks5://user:pass@127.0.0.1:1080",
            "socks5h://[::1]:1080",
        ] {
            assert_eq!(
                curl_proxy_for_system_lookup(Ok(Some(url.into()))),
                Some(CurlProxyPlan::Config(url.into())),
                "{url}"
            );
        }
        for rejected in [
            Ok(Some("https://proxy.example:443".into())),
            Ok(Some("HTTPS://proxy.example:443".into())),
            Ok(Some("http://proxy.example:8080\r\n".into())),
            Ok(Some("http://".into())),
            Ok(Some("ftp://proxy.example:21".into())),
            Ok(Some("not-a-url".into())),
            Err("WinINet lookup failed".into()),
        ] {
            assert_eq!(
                curl_proxy_for_system_lookup(rejected.clone()),
                None,
                "{rejected:?}"
            );
        }
    }

    #[test]
    fn apply_curl_proxy_args_formats_direct_arg_and_escaped_config() {
        let args_of = |plan: &CurlProxyPlan| {
            let mut cmd = std::process::Command::new("curl");
            let config = apply_curl_proxy_args(&mut cmd, plan);
            let args: Vec<String> = cmd
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            (args, config)
        };

        assert_eq!(
            args_of(&CurlProxyPlan::Direct),
            (vec!["--noproxy".into(), "*".into()], None)
        );
        assert_eq!(
            args_of(&CurlProxyPlan::Arg("http://127.0.0.1:8080".into())),
            (
                vec![
                    "--noproxy".into(),
                    "".into(),
                    "-x".into(),
                    "http://127.0.0.1:8080".into(),
                ],
                None
            )
        );
        assert_eq!(
            args_of(&CurlProxyPlan::Config(
                r#"http://u"name:p\w@127.0.0.1:8080"#.into()
            )),
            (
                vec!["--noproxy".into(), "".into(), "--config".into(), "-".into(),],
                Some("proxy = \"http://u\\\"name:p\\\\w@127.0.0.1:8080\"\n".into())
            )
        );
    }

    #[test]
    fn system_proxy_curl_impersonate_propagates_pause_and_cancel() {
        let prefix = format!(
            "https://system-proxy-control-{}.invalid/",
            NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed)
        );
        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((prefix.clone(), PathBuf::from("fake-curl-unused")));

        for (command, expect_pause) in [("pause", true), ("cancel", false)] {
            let (mut job, dir) = temp_job(&format!("{prefix}file.bin"), true, 0, 1);
            job.proxy = crate::net_policy::SYSTEM_PROXY_SENTINEL.into();
            fs::write(&job.control, command).unwrap();
            let status_or_err = fetch_via_curl_impersonate(&job, None, &[])
                .map(|result| result.map(|fetched| fetched.status));
            fs::remove_dir_all(dir).unwrap();
            if expect_pause {
                assert!(
                    matches!(status_or_err, Some(Err(EngineError::Pause))),
                    "expected Pause, got {status_or_err:?}"
                );
            } else {
                assert!(
                    matches!(status_or_err, Some(Err(EngineError::Cancel))),
                    "expected Cancel, got {status_or_err:?}"
                );
            }
        }

        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(item, _)| item != &prefix);
    }

    #[test]
    fn active_split_allows_same_session_splitting_when_server_accepts_ranges_without_etag() {
        let (mut job, dir) = temp_job("http://127.0.0.1/", false, 1024 * 1024, 4);
        for (etag, modified, expected) in [
            ("", "", true),
            ("W/\"weak-v1\"", "", true),
            ("\"strong-v1\"", "", true),
            ("\"poisoned\"\r\nInjected: 1", "", false),
            ("", "bad\nmodified", false),
        ] {
            job.etag = etag.into();
            job.last_modified = modified.into();
            assert_eq!(
                active_split_minimum_for_session(&job, true, None),
                expected.then_some(256 * 1024),
                "etag={etag:?}, modified={modified:?}"
            );
            assert_eq!(
                active_split_minimum_for_session(&job, true, Some("0")),
                None
            );
        }
        job.etag.clear();
        job.last_modified.clear();
        job.total = 0;
        assert_eq!(active_split_minimum_for_session(&job, true, None), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_session_tail_split_without_etag_completes_exact_body() {
        let total_len = 1024 * 1024usize;
        let body: &'static [u8] = Box::leak(
            (0..total_len)
                .map(|index| ((index * 31 + 7) % 251) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let range_requests = Arc::new(Mutex::new(Vec::<(usize, usize)>::new()));
        let recorded = Arc::clone(&range_requests);
        let tail_split_seen = Arc::new(AtomicBool::new(false));
        let server_split_seen = Arc::clone(&tail_split_seen);

        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let recorded = Arc::clone(&recorded);
                let split_seen = Arc::clone(&server_split_seen);
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                        return;
                    }
                    let mut range = None;
                    let mut saw_if_range = false;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        let lower = line.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("range:") {
                            range = Some(value.trim().to_string());
                        }
                        if lower.starts_with("if-range:") {
                            saw_if_range = true;
                        }
                    }
                    assert!(
                        !saw_if_range,
                        "empty validator must not synthesize an If-Range header"
                    );
                    let mut stream = reader.into_inner();
                    let Some(spec) =
                        range.and_then(|v| v.strip_prefix("bytes=").map(str::to_string))
                    else {
                        return;
                    };
                    let (start_s, end_s) = spec.split_once('-').unwrap();
                    let start: usize = start_s.parse().unwrap();
                    let end: usize = end_s.parse::<usize>().unwrap().min(body.len() - 1);
                    recorded.lock().unwrap().push((start, end));
                    if start > 0 && end == body.len() - 1 {
                        split_seen.store(true, Ordering::SeqCst);
                    }
                    let slice = &body[start..=end];
                    // Explicitly advertise Accept-Ranges: bytes + Content-Length, NO ETag / Last-Modified.
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len(),
                        slice.len()
                    );
                    if stream.write_all(header.as_bytes()).is_err() {
                        return;
                    }
                    // Pace the first full-file chunk briefly so worker 1 can tail-split it.
                    if start == 0 && end == body.len() - 1 {
                        let mut offset = 0;
                        while offset < slice.len() {
                            let step = (64 * 1024).min(slice.len() - offset);
                            if stream.write_all(&slice[offset..offset + step]).is_err() {
                                return;
                            }
                            offset += step;
                            if !split_seen.load(Ordering::SeqCst) {
                                thread::sleep(Duration::from_millis(15));
                            }
                        }
                    } else {
                        let _ = stream.write_all(slice);
                    }
                });
            }
        });

        let url = format!("http://{addr}/no-etag.bin");
        let (mut job, dir) = temp_job(&url, false, 0, 2);
        job.etag.clear();
        job.last_modified.clear();
        // Force a single initial pending chunk so worker 1 MUST use dynamic tail-splitting.
        job.chunk_bytes = total_len as u64;
        run_job(&job).unwrap();

        assert_eq!(fs::read(&job.output).unwrap(), body);
        assert!(
            tail_split_seen.load(Ordering::SeqCst),
            "expected dynamic tail-split without ETag, requests={:?}",
            range_requests.lock().unwrap()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn curl_low_speed_args_omits_speed_limit_when_throttled() {
        assert_eq!(
            curl_low_speed_args(false),
            &["--speed-time", "30", "--speed-limit", "1024"]
        );
        assert!(curl_low_speed_args(true).is_empty());
    }

    #[test]
    fn waf_curl_cache_hits_expires_and_bounds_capacity() {
        let start = Instant::now();
        let mut cache = WafCurlCache::default();
        assert!(!cache.contains_at("https://cdn.example.test:443", start));

        cache.record_at("https://cdn.example.test:443".into(), start);
        assert!(cache.contains_at(
            "https://cdn.example.test:443",
            start + Duration::from_secs(60)
        ));
        assert!(!cache.contains_at("https://cdn.example.test:443", start + WAF_CURL_CACHE_TTL));
        // Entry was pruned upon expiry.
        assert!(cache.entries.is_empty());

        // Bounded at WAF_CURL_CACHE_MAX_ENTRIES (256), evicting oldest entries.
        for index in 0..(WAF_CURL_CACHE_MAX_ENTRIES + 8) {
            cache.record_at(
                format!("https://host-{index}.example.test:443"),
                start + Duration::from_millis(index as u64),
            );
        }
        assert_eq!(cache.entries.len(), WAF_CURL_CACHE_MAX_ENTRIES);
        assert!(!cache.contains_at(
            "https://host-0.example.test:443",
            start + Duration::from_secs(1)
        ));
        assert!(cache.contains_at(
            &format!(
                "https://host-{}.example.test:443",
                WAF_CURL_CACHE_MAX_ENTRIES + 7
            ),
            start + Duration::from_secs(1)
        ));

        let newest = format!(
            "https://host-{}.example.test:443",
            WAF_CURL_CACHE_MAX_ENTRIES + 7
        );
        cache.invalidate(&newest);
        assert!(!cache.contains_at(&newest, start + Duration::from_secs(1)));
    }

    #[test]
    fn waf_curl_cache_skips_repeat_native_403_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let native_hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&native_hits);
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                server_hits.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = reader.into_inner().write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });

        let origin_url = format!("http://{addr}/");
        let case_dir = std::env::temp_dir().join(format!(
            "hls-waf-cache-test-{}-{}",
            std::process::id(),
            NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&case_dir).unwrap();
        fs::write(
            case_dir.join("head"),
            "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n",
        )
        .unwrap();
        fs::write(case_dir.join("body"), b"ok!!").unwrap();

        #[cfg(windows)]
        let script = {
            let path = case_dir.join("fake-curl.cmd");
            fs::write(
                &path,
                "@echo off\r\n\
                 set \"dir=%~dp0\"\r\n\
                 :loop\r\n\
                 if \"%~1\"==\"\" exit /b 1\r\n\
                 if \"%~1\"==\"--dump-header\" (\r\n\
                   type \"%dir%head\" > \"%~2\"\r\n\
                   type \"%dir%body\"\r\n\
                   exit /b 0\r\n\
                 )\r\n\
                 shift /1\r\n\
                 goto loop\r\n",
            )
            .unwrap();
            path
        };
        #[cfg(not(windows))]
        let script = {
            use std::os::unix::fs::PermissionsExt;
            let path = case_dir.join("fake-curl.sh");
            fs::write(
                &path,
                "#!/bin/sh\n\
                 h=; p=\n\
                 for a in \"$@\"; do [ \"$p\" = \"--dump-header\" ] && h=\"$a\"; p=\"$a\"; done\n\
                 d=\"$(dirname \"$0\")\"\n\
                 cat \"$d/head\" > \"$h\"\n\
                 cat \"$d/body\"\n\
                 exit 0\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };

        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((origin_url.clone(), script));

        let url = format!("{origin_url}chunk.bin");
        let origin_key = http_origin(&url).unwrap();
        waf_curl_cache_invalidate(&origin_key);
        let (job, dir) = temp_job(&url, true, 0, 1);

        // First fetch hits native HTTP (gets 403), falls back to curl (200), and caches origin.
        let mut first = fetch(&job, None).unwrap();
        assert_eq!(first.status, 200);
        let mut body1 = Vec::new();
        first.body.read_to_end(&mut body1).unwrap();
        assert_eq!(body1, b"ok!!");
        assert_eq!(native_hits.load(Ordering::SeqCst), 1);
        assert!(waf_curl_cache_hit(&origin_key));

        // Second fetch to the same origin skips native 403 completely and goes straight to curl.
        let mut second = fetch(&job, None).unwrap();
        assert_eq!(second.status, 200);
        let mut body2 = Vec::new();
        second.body.read_to_end(&mut body2).unwrap();
        assert_eq!(body2, b"ok!!");
        assert_eq!(
            native_hits.load(Ordering::SeqCst),
            1,
            "second fetch must skip wasted native 403 round-trip"
        );

        // After cache invalidation, fetch probes native again.
        waf_curl_cache_invalidate(&origin_key);
        let mut third = fetch(&job, None).unwrap();
        assert_eq!(third.status, 200);
        let mut body3 = Vec::new();
        third.body.read_to_end(&mut body3).unwrap();
        assert_eq!(body3, b"ok!!");
        assert_eq!(native_hits.load(Ordering::SeqCst), 2);

        waf_curl_cache_invalidate(&origin_key);
        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(item, _)| item != &origin_url);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(case_dir);
    }

    #[test]
    fn waf_curl_cache_falls_back_to_native_when_cached_curl_gets_403() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let native_hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&native_hits);
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let hit = server_hits.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let response = if hit == 0 {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnat!"
                        .as_slice()
                } else {
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .as_slice()
                };
                let _ = reader.into_inner().write_all(response);
            }
        });

        let origin_url = format!("http://{addr}/");
        let case_dir = std::env::temp_dir().join(format!(
            "hls-waf-cache-fallback-test-{}-{}",
            std::process::id(),
            NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&case_dir).unwrap();
        fs::write(
            case_dir.join("head"),
            "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap();
        fs::write(case_dir.join("body"), b"").unwrap();
        let curl_log = case_dir.join("curl-hits.txt");

        #[cfg(windows)]
        let script = {
            let path = case_dir.join("fake-curl-403.cmd");
            fs::write(
                &path,
                "@echo off\r\n\
                 set \"dir=%~dp0\"\r\n\
                 echo hit>> \"%dir%curl-hits.txt\"\r\n\
                 :loop\r\n\
                 if \"%~1\"==\"\" exit /b 1\r\n\
                 if \"%~1\"==\"--dump-header\" (\r\n\
                   type \"%dir%head\" > \"%~2\"\r\n\
                   exit /b 0\r\n\
                 )\r\n\
                 shift /1\r\n\
                 goto loop\r\n",
            )
            .unwrap();
            path
        };
        #[cfg(not(windows))]
        let script = {
            use std::os::unix::fs::PermissionsExt;
            let path = case_dir.join("fake-curl-403.sh");
            fs::write(
                &path,
                "#!/bin/sh\n\
                 d=\"$(dirname \"$0\")\"\n\
                 echo hit >> \"$d/curl-hits.txt\"\n\
                 h=; p=\n\
                 for a in \"$@\"; do [ \"$p\" = \"--dump-header\" ] && h=\"$a\"; p=\"$a\"; done\n\
                 cat \"$d/head\" > \"$h\"\n\
                 exit 0\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };

        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((origin_url.clone(), script));

        let url = format!("{origin_url}seg.ts");
        let origin_key = http_origin(&url).unwrap();
        let (job, dir) = temp_job(&url, true, 0, 1);

        // 1) Cached origin where curl gets 403 must invalidate cache and fall back to native HTTP.
        waf_curl_cache_record(origin_key.clone());
        let mut recovered = fetch(&job, None).unwrap();
        assert_eq!(recovered.status, 200);
        let mut body = Vec::new();
        recovered.body.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"nat!");
        assert!(!waf_curl_cache_hit(&origin_key));
        assert_eq!(native_hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            fs::read_to_string(&curl_log).unwrap().lines().count(),
            1,
            "curl should run once before falling back to native"
        );

        // 2) Cached origin where BOTH curl and native return 403 must not invoke curl twice.
        waf_curl_cache_record(origin_key.clone());
        let denied = fetch(&job, None).unwrap();
        assert_eq!(denied.status, 403);
        assert!(!waf_curl_cache_hit(&origin_key));
        assert_eq!(native_hits.load(Ordering::SeqCst), 2);
        assert_eq!(
            fs::read_to_string(&curl_log).unwrap().lines().count(),
            2,
            "tried_curl guard must prevent invoking curl a second time after native 403"
        );

        waf_curl_cache_invalidate(&origin_key);
        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(item, _)| item != &origin_url);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(case_dir);
    }

    #[test]
    fn canonical_url_key_strips_signatures_and_keeps_resource_identity() {
        let first = canonical_url_key(
            "HTTPS://UPOS-SZ-MIRRORCOS.BILIVIDEO.COM:443/upgcxcode/12/34/30280.m4s?e=ig8euxZM2rNcNbdlhoNvNC8BqJIzNbfqXBvEqxTEto8BTrNvN0GvT90W5JZMkX_YN0MvXg8gNEV4NC8xNEV4N03eN0B5tZlqNxTEto8BTrNvNeZVuJ10Kj_g2UB02J0mN0B5tZlqNCNEto8BTrNvNC7MTX502C8f2jmMQJ6mqF2fka1mqx6gqj0eN0B599M=&uipk=5&nbs=1&deadline=1700000000&gen=playurlv2&os=cosbv&oi=123&trid=abcd&mid=999&platform=pc&upsig=11111111111111111111111111111111&uparams=e,uipk,nbs,deadline,gen,os,oi,trid,mid,platform#frag",
        );
        let second = canonical_url_key(
            "https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/12/34/30280.m4s?e=NEW_E&uipk=5&nbs=1&deadline=1999999999&gen=playurlv2&os=cosbv&oi=123&trid=efgh&mid=999&platform=pc&upsig=22222222222222222222222222222222&uparams=e,uipk,nbs,deadline,gen,os,oi,trid,mid,platform",
        );
        assert_eq!(first, second);
        assert!(first.contains("uipk=5&nbs=1&oi=123"));
        assert!(!first.contains("upsig="));
        assert!(!first.contains("deadline="));

        let s3_a = canonical_url_key(
            "https://bucket.s3.amazonaws.com/archive.iso?versionId=7&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIA&X-Amz-Date=20260101T000000Z&X-Amz-Expires=900&X-Amz-Signature=deadbeef",
        );
        let s3_b = canonical_url_key(
            "https://bucket.s3.amazonaws.com/archive.iso?versionId=7&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIA2&X-Amz-Date=20260102T000000Z&X-Amz-Expires=3600&X-Amz-Signature=cafebabe",
        );
        assert_eq!(
            s3_a,
            "https://bucket.s3.amazonaws.com/archive.iso?versionId=7"
        );
        assert_eq!(s3_a, s3_b);

        let different = canonical_url_key(
            "https://bucket.s3.amazonaws.com/archive.iso?versionId=8&X-Amz-Signature=cafebabe",
        );
        assert_ne!(s3_a, different);
    }

    #[test]
    fn checkpoint_matches_allows_missing_validators_but_rejects_validator_mismatch() {
        let (mut job, dir) = temp_job(
            "https://cdn.example.test/video.mp4?token=new_sig&expires=999999",
            false,
            1024,
            2,
        );
        job.resource_key = canonical_url_key(&job.url);
        job.etag.clear();
        job.last_modified.clear();

        let saved_without_validators = serde_json::json!({
            "version": 2,
            "resource_key": "https://cdn.example.test/video.mp4",
            "etag": "",
            "last_modified": "",
            "total": 1024,
            "ranges": [[0, 511]]
        });
        assert!(checkpoint_matches(&job, &saved_without_validators));

        // Both checkpoint and probed response have matching strong ETag -> allowed.
        job.etag = "\"same-v1\"".into();
        let saved_v1 = serde_json::json!({
            "version": 2,
            "resource_key": "https://cdn.example.test/video.mp4",
            "etag": "\"same-v1\"",
            "last_modified": "",
            "total": 1024,
            "ranges": [[0, 511]]
        });
        assert!(checkpoint_matches(&job, &saved_v1));

        // Strong ETag changed -> rejected!
        job.etag = "\"changed-v2\"".into();
        assert!(!checkpoint_matches(&job, &saved_v1));

        // Last-Modified changed -> rejected!
        job.etag.clear();
        job.last_modified = "Thu, 22 Oct 2026 10:00:00 GMT".into();
        let saved_mod = serde_json::json!({
            "version": 2,
            "resource_key": "https://cdn.example.test/video.mp4",
            "etag": "",
            "last_modified": "Wed, 21 Oct 2026 10:00:00 GMT",
            "total": 1024,
            "ranges": [[0, 511]]
        });
        assert!(!checkpoint_matches(&job, &saved_mod));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn server_capped_206_partial_content_does_not_halve_desired_workers() {
        let body: &'static [u8] = Box::leak(
            (0..512 * 1024)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peak_inflight = Arc::new(AtomicUsize::new(0));
        let active_inflight = Arc::new(AtomicUsize::new(0));
        let peak_srv = Arc::clone(&peak_inflight);
        let active_srv = Arc::clone(&active_inflight);
        thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let peak = Arc::clone(&peak_srv);
                let active = Arc::clone(&active_srv);
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut req = String::new();
                    let mut range_hdr = None;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        if line.to_ascii_lowercase().starts_with("range:") {
                            range_hdr = Some(line.split_once(':').unwrap().1.trim().to_string());
                        }
                        req.push_str(&line);
                    }
                    if req.is_empty() {
                        return;
                    }
                    let mut out = reader.into_inner();
                    if let Some(spec) = range_hdr.and_then(|r| {
                        r.strip_prefix("bytes=")
                            .and_then(|s| s.split_once('-'))
                            .and_then(|(a, b)| {
                                Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                            })
                    }) {
                        let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = peak.fetch_max(now_active, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(25));
                        let start = spec.0.min(body.len() - 1);
                        // Cap each 206 response window to at most 64 KiB regardless of requested end.
                        let capped_end = spec.1.min(start + 64 * 1024 - 1).min(body.len() - 1);
                        let slice = &body[start..=capped_end];
                        let header = format!(
                            "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nETag: \"capped\"\r\nContent-Range: bytes {start}-{capped_end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len(),
                            slice.len()
                        );
                        let _ = out.write_all(header.as_bytes());
                        let _ = out.write_all(slice);
                        active.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });
        let url = format!("http://{addr}/capped.bin");
        let (mut job, dir) = temp_job(&url, false, body.len() as u64, 4);
        job.etag = "\"capped\"".into();
        job.chunk_bytes = 256 * 1024;
        run_job(&job).unwrap();
        assert_eq!(fs::read(&job.output).unwrap(), body);
        let observed_peak = peak_inflight.load(Ordering::SeqCst);
        // Verify multi-worker splitting stayed active (>= 2 concurrent workers) and was not crushed to 1 by congestion penalties.
        assert!(
            observed_peak >= 2,
            "expected concurrent multi-range workers without congestion collapse, peak_inflight={observed_peak}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fetch_browser_first_does_not_retry_across_transports_on_terminal_404() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let native_hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&native_hits);
        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut scratch = [0u8; 512];
                let _ = stream.read(&mut scratch);
                server_hits.fetch_add(1, Ordering::SeqCst);
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let case_dir = std::env::temp_dir().join(format!("hls-browser-first-404-{stamp}"));
        fs::create_dir_all(&case_dir).unwrap();
        let curl_log = case_dir.join("curl-hits.txt");
        fs::write(&curl_log, "").unwrap();
        fs::write(
            case_dir.join("head"),
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap();

        #[cfg(windows)]
        let script = {
            let path = case_dir.join("fake-curl.cmd");
            fs::write(
                &path,
                "@echo off\r\n\
                 setlocal\r\n\
                 set \"D=%~dp0\"\r\n\
                 echo hit>>\"%D%curl-hits.txt\"\r\n\
                 :loop\r\n\
                 if \"%~1\"==\"\" goto done\r\n\
                 if \"%~1\"==\"--dump-header\" (\r\n\
                   copy /y \"%D%head\" \"%~2\" >nul\r\n\
                   goto done\r\n\
                 )\r\n\
                 shift\r\n\
                 goto loop\r\n\
                 :done\r\n\
                 exit /b 0\r\n",
            )
            .unwrap();
            path
        };
        #[cfg(not(windows))]
        let script = {
            use std::os::unix::fs::PermissionsExt;
            let path = case_dir.join("fake-curl.sh");
            fs::write(
                &path,
                "#!/bin/sh\n\
                 d=\"$(dirname \"$0\")\"\n\
                 echo hit >> \"$d/curl-hits.txt\"\n\
                 h=; p=\n\
                 for a in \"$@\"; do [ \"$p\" = \"--dump-header\" ] && h=\"$a\"; p=\"$a\"; done\n\
                 cat \"$d/head\" > \"$h\"\n\
                 exit 0\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };

        let http_origin_url = format!("http://127.0.0.1:{}/", addr.port());
        let https_origin_url = format!("https://127.0.0.1:{}/", addr.port());
        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((http_origin_url.clone(), script.clone()));
        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((https_origin_url.clone(), script));

        // 1. Uncached origin: native transport runs first, receives terminal 404,
        //    and must NOT spawn curl-impersonate.
        let uncached_url = format!("{http_origin_url}missing.m4s");
        let (mut uncached_job, dir1) = temp_job(&uncached_url, true, 0, 1);
        uncached_job.method = "GET".into();
        let fetched = fetch_browser_first(&uncached_job, None).unwrap();
        assert_eq!(fetched.status, 404);
        assert_eq!(
            native_hits.load(Ordering::SeqCst),
            1,
            "uncached segment fetch must try fast in-process native HTTP first"
        );
        assert_eq!(
            fs::read_to_string(&curl_log).unwrap().lines().count(),
            0,
            "terminal 404 from primary native transport must not fall back to curl"
        );

        // 2. WAF-cached origin: curl runs first, receives terminal 404,
        //    and must NOT fall back to native HTTP.
        let https_origin_key = http_origin(&https_origin_url).unwrap();
        waf_curl_cache_record(https_origin_key.clone());
        let cached_url = format!("{https_origin_url}missing.m4s");
        let (mut cached_job, dir2) = temp_job(&cached_url, true, 0, 1);
        cached_job.method = "GET".into();
        let fetched_cached = fetch_browser_first(&cached_job, None).unwrap();
        assert_eq!(fetched_cached.status, 404);
        assert_eq!(
            fs::read_to_string(&curl_log).unwrap().lines().count(),
            1,
            "WAF-cached origin must execute curl once on terminal 404"
        );
        assert_eq!(
            native_hits.load(Ordering::SeqCst),
            1,
            "terminal 404 from WAF-cached curl must not fall back to native HTTP"
        );
        waf_curl_cache_invalidate(&https_origin_key);

        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(item, _)| item != &http_origin_url && item != &https_origin_url);
        let _ = fs::remove_dir_all(dir1);
        let _ = fs::remove_dir_all(dir2);
        let _ = fs::remove_dir_all(case_dir);
    }

    #[test]
    fn fetch_browser_first_prefers_native_and_caches_waf_curl_fallback() {
        let mode = Arc::new(AtomicUsize::new(200));
        let server_mode = Arc::clone(&mode);
        let native_hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&native_hits);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut scratch = [0u8; 512];
                let _ = stream.read(&mut scratch);
                server_hits.fetch_add(1, Ordering::SeqCst);
                match server_mode.load(Ordering::SeqCst) {
                    200 => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nnative",
                        );
                    }
                    code => {
                        let reply = format!(
                            "HTTP/1.1 {code} Challenge\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(reply.as_bytes());
                    }
                }
            }
        });

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let case_dir = std::env::temp_dir().join(format!("hls-browser-first-waf-{stamp}"));
        fs::create_dir_all(&case_dir).unwrap();
        let curl_log = case_dir.join("curl-hits.txt");
        fs::write(&curl_log, "").unwrap();
        fs::write(
            case_dir.join("head"),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n",
        )
        .unwrap();

        #[cfg(windows)]
        let script = {
            let path = case_dir.join("fake-curl.cmd");
            fs::write(
                &path,
                "@echo off\r\n\
                 setlocal\r\n\
                 set \"D=%~dp0\"\r\n\
                 echo hit>>\"%D%curl-hits.txt\"\r\n\
                 :loop\r\n\
                 if \"%~1\"==\"\" goto out\r\n\
                 if \"%~1\"==\"--dump-header\" (\r\n\
                   copy /y \"%D%head\" \"%~2\" >nul\r\n\
                 )\r\n\
                 shift\r\n\
                 goto loop\r\n\
                 :out\r\n\
                 <nul set /p =curl\r\n\
                 exit /b 0\r\n",
            )
            .unwrap();
            path
        };
        #[cfg(not(windows))]
        let script = {
            use std::os::unix::fs::PermissionsExt;
            let path = case_dir.join("fake-curl.sh");
            fs::write(
                &path,
                "#!/bin/sh\n\
                 d=\"$(dirname \"$0\")\"\n\
                 echo hit >> \"$d/curl-hits.txt\"\n\
                 h=; p=\n\
                 for a in \"$@\"; do [ \"$p\" = \"--dump-header\" ] && h=\"$a\"; p=\"$a\"; done\n\
                 cat \"$d/head\" > \"$h\"\n\
                 printf 'curl'\n\
                 exit 0\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };

        let origin_url = format!("http://127.0.0.1:{}/", addr.port());
        let origin_key = http_origin(&origin_url).unwrap();
        waf_curl_cache_invalidate(&origin_key);
        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((origin_url.clone(), script));

        // Normal 200 OK segment: zero curl processes spawned!
        let seg1 = format!("{origin_url}seg1.ts");
        let (status, body, _) =
            fetch_bytes_with_transport(&seg1, &HashMap::new(), "", true, Some(1024)).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"native");
        assert_eq!(native_hits.load(Ordering::SeqCst), 1);
        assert_eq!(fs::read_to_string(&curl_log).unwrap().lines().count(), 0);

        // WAF 429/503/403 challenge: native hits 503, falls back to curl, caches origin!
        mode.store(503, Ordering::SeqCst);
        let seg2 = format!("{origin_url}seg2.ts");
        let (status, body, _) =
            fetch_bytes_with_transport(&seg2, &HashMap::new(), "", true, Some(1024)).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"curl");
        assert_eq!(native_hits.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read_to_string(&curl_log).unwrap().lines().count(), 1);
        assert!(waf_curl_cache_hit(&origin_key));

        // Next segment on the WAF-flagged origin goes straight to curl with 0 native hits!
        let seg3 = format!("{origin_url}seg3.ts");
        let (status, body, _) =
            fetch_bytes_with_transport(&seg3, &HashMap::new(), "", true, Some(1024)).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"curl");
        assert_eq!(native_hits.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read_to_string(&curl_log).unwrap().lines().count(), 2);

        waf_curl_cache_invalidate(&origin_key);
        TEST_CURL_OVERRIDES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(item, _)| item != &origin_url);
        let _ = fs::remove_dir_all(case_dir);
    }
}
