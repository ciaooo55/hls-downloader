//! Range GET into one `payload.downloading`.
//!
//! No extra process and no TLS crate: loopback/http uses the same tiny
//! TcpStream client as the core poller; Windows https uses WinHTTP.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::net::{Ipv6Addr, TcpStream};
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
const MAX_RANGE_ATTEMPTS: u32 = 5;
const ADAPTIVE_INITIAL_CONNECTIONS: usize = 2;
const CONNECT_POOL_PER_KEY: usize = 8;
const CONNECT_POOL_TOTAL: usize = 32;

#[derive(Default)]
struct IdleHandlePool {
    idle: HashMap<String, Vec<usize>>,
}

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
enum ProxyRoute<'a> {
    Direct,
    System,
    Manual(&'a str),
}

fn proxy_route(proxy: &str) -> ProxyRoute<'_> {
    let proxy = proxy.trim();
    if proxy.is_empty() || proxy == crate::net_policy::DIRECT_PROXY_SENTINEL {
        ProxyRoute::Direct
    } else if proxy == crate::net_policy::SYSTEM_PROXY_SENTINEL {
        ProxyRoute::System
    } else {
        ProxyRoute::Manual(proxy)
    }
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
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = fs::rename(tmp, path);
    }
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
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = fs::rename(tmp, path);
    }
}

pub(crate) fn write_media_merging(path: &Path) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let Ok(mut payload) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    payload["status"] = "merging".into();
    payload["speed"] = 0.into();
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = fs::rename(tmp, path);
    }
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
    let tmp = path.with_extension("json.tmp");
    let payload = serde_json::json!({
        "ranges": ranges.iter().map(|(start, end)| serde_json::json!([start, end])).collect::<Vec<_>>(),
    });
    if fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = crate::atomic_replace::replace_file(&tmp, &path);
    }
}

fn strong_etag(value: &str) -> String {
    let text = value.trim();
    if text.is_empty() || text.to_ascii_lowercase().starts_with("w/") {
        String::new()
    } else {
        text.to_string()
    }
}

fn checkpoint_matches(job: &Job, value: &serde_json::Value) -> bool {
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(2) {
        return false;
    }
    let Some(total) = value.get("total").and_then(serde_json::Value::as_u64) else {
        return false;
    };
    if total != job.total {
        return false;
    }
    let expected_key = job.resource_key.trim();
    let saved_key = value
        .get("resource_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    if expected_key.is_empty() || saved_key != expected_key {
        return false;
    }
    let expected_etag = strong_etag(&job.etag);
    if !expected_etag.is_empty() {
        return strong_etag(
            value
                .get("etag")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        ) == expected_etag;
    }
    let expected_modified = job.last_modified.trim();
    !expected_modified.is_empty()
        && value
            .get("last_modified")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            == expected_modified
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
    if !checkpoint_matches(job, &value) {
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
    file.write_all(payload.to_string().as_bytes())
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint write failed: {err}")))?;
    file.flush()
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint flush failed: {err}")))?;
    file.sync_all()
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint sync failed: {err}")))?;
    drop(file);
    crate::atomic_replace::replace_file(&tmp, &path)
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint replace failed: {err}")))
}

struct RangeCheckpoints {
    durable: Mutex<Vec<(u64, u64)>>,
    pending: Mutex<Vec<(u64, u64)>>,
}

impl RangeCheckpoints {
    fn new(durable: Vec<(u64, u64)>) -> Self {
        Self {
            durable: Mutex::new(durable),
            pending: Mutex::new(Vec::new()),
        }
    }
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
        .filter(|value| !value.trim().is_empty())
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
    let total = fetched
        .content_range
        .as_deref()
        .and_then(parse_content_range)
        .and_then(|(_, _, total)| total)
        .or(fetched.content_length);
    let accept_ranges = fetched.status == 206
        && (fetched.accept_ranges || fetched.content_range.is_some() || total.is_some());
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
    let post = job.method.eq_ignore_ascii_case("POST");
    let mut identity: Option<(Option<u64>, String)> = None;
    if job.total > 0 || !job.etag.trim().is_empty() {
        identity = Some(((job.total > 0).then_some(job.total), job.etag.clone()));
    }
    for (index, url) in urls.into_iter().enumerate() {
        let mut attempt = job.clone();
        attempt.url = url;
        let mut initial_response = None;
        if !post
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
                    }
                    // A 200 already owns the complete entity. Do not consume a
                    // one-use URL again merely because its server ignores Range.
                    if response.status == 200 && attempt.resume_from == 0 {
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
        match run_job_once(&attempt, initial_response) {
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

fn run_job_once(job: &Job, initial_response: Option<FetchResult>) -> Result<(), EngineError> {
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
        let _ = fs::remove_file(completed_ranges_path(job));
        let _ = fs::remove_file(active_ranges_path(&job.progress));
        download_sequential(job, initial_response)
    } else {
        download_ranges(job)
    }
}

pub(crate) fn fetch_bytes_limited(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    limit: u64,
) -> Result<(u16, Vec<u8>), EngineError> {
    fetch_bytes_with_transport(url, headers, proxy, false, Some(limit))
}

pub fn fetch_hls_bytes(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
) -> Result<(u16, Vec<u8>), EngineError> {
    fetch_bytes_with_transport(url, headers, proxy, true, None)
}

pub(crate) fn fetch_hls_bytes_limited(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    limit: u64,
) -> Result<(u16, Vec<u8>), EngineError> {
    fetch_bytes_with_transport(url, headers, proxy, true, Some(limit))
}

fn fetch_bytes_with_transport(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    browser_first: bool,
    limit: Option<u64>,
) -> Result<(u16, Vec<u8>), EngineError> {
    if !http_fetch_url_allowed(url) {
        return Err(EngineError::Failed("url must be http(s)".into()));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("hls-fetch-bytes-{}-{}", std::process::id(), stamp));
    fs::create_dir_all(&dir).map_err(|err| EngineError::Failed(err.to_string()))?;
    let control = dir.join("control");
    fs::write(&control, "run").map_err(|err| EngineError::Failed(err.to_string()))?;
    // 子请求继承下载任务的控制文件；不得把父任务的暂停状态重写成 run。
    let control = crate::net_policy::current_throttle_context()
        .map(|context| context.control)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(control);
    let job = Job {
        url: url.to_string(),
        headers: headers.clone(),
        output: dir.join("body"),
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
        progress: dir.join("progress.json"),
        method: "GET".into(),
        body_path: PathBuf::new(),
        mirrors: Vec::new(),
        replay_json: String::new(),
        curl_body_limit: limit,
        range_stop: None,
        origin_boundary: String::new(),
    };
    let result = (|| {
        let fetched = if browser_first {
            fetch_browser_first(&job, None)?
        } else {
            fetch(&job, None)?
        };
        let status = fetched.status;
        let expected_length = fetched.content_length;
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
        if let Err(error) = reader.read_to_end(&mut body) {
            check_job_control(&job)?;
            return Err(EngineError::Failed(error.to_string()));
        }
        if limit.is_some_and(|limit| body.len() as u64 > limit) {
            return Err(EngineError::Failed(
                "metadata response exceeds byte limit".into(),
            ));
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
        crate::net_policy::consume(body.len(), || check_job_control(&job))?;
        Ok((status, body))
    })();
    let _ = fs::remove_dir_all(dir);
    result
}

pub fn fetch_hls_bytes_range(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    start: u64,
    length: u64,
) -> Result<Vec<u8>, EngineError> {
    fetch_bytes_range_with_transport(url, headers, proxy, start, length, true)
}

fn fetch_bytes_range_with_transport(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    start: u64,
    length: u64,
    browser_first: bool,
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
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("hls-fetch-range-{}-{}", std::process::id(), stamp));
    fs::create_dir_all(&dir).map_err(|err| EngineError::Failed(err.to_string()))?;
    let result = (|| {
        let control = dir.join("control");
        fs::write(&control, "run").map_err(|err| EngineError::Failed(err.to_string()))?;
        let control = crate::net_policy::current_throttle_context()
            .map(|context| context.control)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(control);
        let job = Job {
            url: url.to_string(),
            headers: headers.clone(),
            output: dir.join("body"),
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
            progress: dir.join("progress.json"),
            method: "GET".into(),
            body_path: PathBuf::new(),
            mirrors: Vec::new(),
            replay_json: String::new(),
            curl_body_limit: Some(length),
            range_stop: None,
            origin_boundary: String::new(),
        };
        let range = format!("bytes={start}-{end}");
        let fetched = if browser_first {
            fetch_browser_first(&job, Some(&range))?
        } else {
            fetch(&job, Some(&range))?
        };
        if fetched.status != 206 {
            return Err(EngineError::Failed(format!(
                "HTTP {}: byte range requires status 206",
                fetched.status
            )));
        }
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
        if let Some(size) = fetched.content_length {
            if size != length {
                return Err(EngineError::Failed(format!(
                    "byte range Content-Length {size} != {length}"
                )));
            }
        }
        let mut body = Vec::new();
        // 多读一个字节用于判定超长；不能等错误响应结束后才验证大小。
        let mut reader = fetched.body.take(length.saturating_add(1));
        if let Err(error) = reader.read_to_end(&mut body) {
            check_job_control(&job)?;
            return Err(EngineError::Failed(error.to_string()));
        }
        if body.len() as u64 != length {
            return Err(EngineError::Failed(format!(
                "incomplete response body: byte range length {} != {length}",
                body.len()
            )));
        }
        crate::net_policy::consume(body.len(), || check_job_control(&job))?;
        Ok(body)
    })();
    let _ = fs::remove_dir_all(dir);
    result
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
    let resume_from = if job.resume_from > 0 && job.output.exists() {
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

fn download_ranges(job: &Job) -> Result<(), EngineError> {
    let _ = fs::remove_file(active_ranges_path(&job.progress));
    let total = job.total;
    if total == 0 {
        return download_sequential(job, None);
    }
    let workers = job.connections.clamp(1, 64);
    // 待办范围优先；尾部分流仍需显式启用和可靠校验器，等待真实传输验证。
    let balanced_chunk = total.div_ceil((workers * 4) as u64).max(256 * 1024);
    let chunk = job.chunk_bytes.max(64 * 1024).min(balanced_chunk);
    let mut ranges = Vec::new();
    let mut start = 0u64;
    while start < total {
        let end = (start + chunk - 1).min(total - 1);
        ranges.push((start, end));
        start = end + 1;
    }
    let output_existed =
        job.output.exists() && job.output.metadata().map(|meta| meta.len()).unwrap_or(0) > 0;
    let loaded = if output_existed {
        match load_completed_ranges(job) {
            Some(ranges) => ranges,
            None => {
                let _ = fs::remove_file(completed_ranges_path(job));
                Vec::new()
            }
        }
    } else {
        let _ = fs::remove_file(completed_ranges_path(job));
        Vec::new()
    };
    let already = covered_bytes(&loaded);
    let pending: Vec<WorkRange> = ranges
        .iter()
        .flat_map(|(range_start, range_end)| subtract_ranges(*range_start, *range_end, &loaded))
        .collect();
    {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&job.output)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        file.set_len(total)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        mark_file_sparse(&file);
    }
    if pending.is_empty() {
        if !job
            .control
            .with_file_name("playback-byte-priority")
            .exists()
        {
            let _ = fs::remove_file(completed_ranges_path(job));
        }
        write_progress(&job.progress, total, total, 0.0, "done");
        return Ok(());
    }
    // 只有"从零开始"的下载才能把 200 解释成服务器不支持 Range：此时没有任何已保留片段，
    // 续传偏移也为 0，整包 body 可以安全地作为完整资源使用。
    // 失效检查点的残留文件不会复用，也属于从零开始；文件存在本身不代表有效断点。
    let fresh_start = loaded.is_empty() && job.resume_from == 0;
    let split_minimum =
        active_split_minimum(job, std::env::var("HLS_HTTP_ACTIVE_SPLIT").ok().as_deref());
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
        let throttle = crate::net_policy::current_throttle_context();
        let scheduler = Arc::clone(&scheduler);
        let downloaded = Arc::clone(&downloaded);
        let completed = Arc::clone(&completed);
        let failed = Arc::clone(&failed);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
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
                if let Some(position) = fs::read_to_string(root.join("playback-byte-priority"))
                    .ok()
                    .and_then(|text| text.trim().parse::<u64>().ok())
                {
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
    let _ = fs::remove_file(active_ranges_path(&job.progress));
    if let Some(code) = failed.lock().unwrap_or_else(|err| err.into_inner()).take() {
        // 从零开始的下载遇到"服务器根本不支持 Range"时，退回单流整包下载：
        // 已启动的 worker 全部join 完毕，不会有并发写；download_sequential 会重建输出文件。
        // 这样用户至少能拿到文件（失去分段并发与断点续传），而不是拿着已到手的整包失败。
        if fresh_start && matches!(code, EngineErrorCode::RangeIgnored(_)) {
            check_job_control(job)?;
            let _ = fs::remove_file(completed_ranges_path(job));
            let _ = fs::remove_file(active_ranges_path(&job.progress));
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
        let _ = fs::remove_file(completed_ranges_path(job));
    }
    write_progress(&job.progress, total, total, 0.0, "done");
    Ok(())
}

fn active_split_minimum(job: &Job, setting: Option<&str>) -> Option<u64> {
    let enabled = !matches!(
        setting.map(str::trim),
        Some("0" | "false" | "FALSE" | "off" | "OFF")
    );
    (enabled && !range_validator(job).is_empty()).then_some(256 * 1024)
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
        if successes < desired.max(2) {
            return false;
        }
        if self
            .desired
            .compare_exchange(desired, desired + 1, Ordering::AcqRel, Ordering::Acquire)
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
        scheduler.complete(active.id);
        match result {
            Ok(None) => {
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
                    return Some(response);
                }
                return None;
            }
            Err(error) => {
                let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                if slot.is_none() {
                    *slot = Some(error);
                }
                stop.store(true, Ordering::SeqCst);
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
        200u64.saturating_mul(1u64 << failed_attempts.saturating_sub(1).min(3))
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
        // curl's file-backed response has already consumed the original tail.
        // Splitting that transport cannot improve network completion time.
        let splittable = !matches!(&fetched.body, Body::File { .. });
        progress
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .splittable = splittable;
        if splittable && scheduler.split_minimum.is_some() {
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
            if cursor.saturating_sub(durable_cursor) >= DURABLE_CHECKPOINT_BYTES {
                scheduler.note_storage_latency(persist_range_progress(
                    job, file, completed, start, cursor,
                )?);
                durable_cursor = cursor;
            }
            if cursor > current_stop {
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
            scheduler.note_congestion();
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
    /// curl-impersonate streams the entity straight to a temp file.  Reading it
    /// back into RAM (the only option before this variant existed) turned a
    /// multi-gigabyte media download into an equally large allocation, which
    /// ends as an OOM kill mid-download.  The temp dir has to outlive the reader,
    /// so the variant owns it and `Drop` removes it on every exit path.
    File {
        file: File,
        temp_dir: PathBuf,
    },
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
            Self::File { .. } => {}
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
            Self::File { file, .. } => file.read(buf),
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

impl Drop for Body {
    fn drop(&mut self) {
        // Only the file-backed reader owns a temp dir; the other variants hold
        // sockets and plain buffers that release themselves.
        if let Self::File { temp_dir, .. } = self {
            let _ = fs::remove_dir_all(temp_dir);
        }
    }
}

struct FetchResult {
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
    peer: TcpStream,
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
pub(crate) fn with_media_response<R>(
    spec: &crate::TaskSpec,
    replay: &str,
    request_url: &str,
    method: &str,
    range: Option<&str>,
    canceled: &Arc<AtomicBool>,
    peer: TcpStream,
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
        origin_boundary: spec.origin_boundary.clone(),
    };
    apply_browser_profile(&mut job.headers);
    check_job_control(&job).map_err(|e| e.to_string())?;
    // curl 的文件型回退会先缓存整部媒体，不适合局域网流式播放。
    let (response, final_url) =
        fetch_follow_restricted(&job, range, None, Some(request_url)).map_err(|e| e.to_string())?;
    if response.content_encoding {
        return Err("媒体服务器未返回 identity 编码".into());
    }
    consume(MediaResponse {
        status: response.status,
        final_url,
        content_type: response.content_type,
        content_length: response.content_length,
        content_range: response.content_range,
        accept_ranges: response.accept_ranges,
        body: response.body,
    })
}

fn fetch(job: &Job, range: Option<&str>) -> Result<FetchResult, EngineError> {
    let mut prepared = job.clone();
    apply_browser_profile(&mut prepared.headers);
    let first = fetch_follow(&prepared, range)?;
    if first.status != 403 {
        return Ok(first);
    }
    if let Some(via_curl) = fetch_curl_follow(&prepared, range) {
        return via_curl;
    }
    Ok(first)
}

fn fetch_browser_first(job: &Job, range: Option<&str>) -> Result<FetchResult, EngineError> {
    let browser_eligible = job.url.to_ascii_lowercase().starts_with("https://")
        && job.method.eq_ignore_ascii_case("GET");
    if browser_eligible {
        if let Some(result) = fetch_curl_follow(job, range) {
            match result {
                Ok(fetched) if (200..400).contains(&fetched.status) => return Ok(fetched),
                Err(error @ (EngineError::Pause | EngineError::Cancel)) => return Err(error),
                Ok(_) | Err(_) => {}
            }
        }
    }
    fetch(job, range)
}

fn fetch_curl_follow(job: &Job, range: Option<&str>) -> Option<Result<FetchResult, EngineError>> {
    curl_impersonate_exe()?;
    let replay = job_replay_json(job);
    let mut url = job.url.clone();
    let mut headers = headers_for_request(job, &url);
    let mut hop = job.clone();
    apply_browser_profile(&mut headers);
    for _ in 0..16 {
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
        let mut fetched = match fetch_via_curl_impersonate(&hop, range)? {
            Ok(fetched) => fetched,
            Err(error) => return Some(Err(error)),
        };
        if fetched.status == 403 {
            if let Some(stripped) = strip_stale_cloudflare_cookies(&headers) {
                drop(fetched);
                headers = stripped;
                hop.headers = headers.clone();
                fetched = match fetch_via_curl_impersonate(&hop, range)? {
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
                return Some(Err(EngineError::Failed("redirect is not http(s)".into())));
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
    fetch_follow_restricted(job, range, None, None).map(|(response, _)| response)
}

fn fetch_follow_restricted(
    job: &Job,
    range: Option<&str>,
    origin: Option<&str>,
    request_url: Option<&str>,
) -> Result<(FetchResult, String), EngineError> {
    let mut url = request_url.unwrap_or(&job.url).to_string();
    let replay = job_replay_json(job);
    let mut headers = headers_for_request(job, &job.url);
    let mut hop = job.clone();
    for _ in 0..16 {
        if origin
            .filter(|v| !v.is_empty())
            .or_else(|| (!job.origin_boundary.is_empty()).then_some(job.origin_boundary.as_str()))
            .is_some_and(|origin| http_origin(&url).as_deref() != Some(origin))
        {
            return Err(EngineError::Failed("redirect left project origin".into()));
        }
        let parsed = parse_http_url(&url)?;
        hop.url = url.clone();
        let request = |headers: &HashMap<String, String>| {
            let mut hop = hop.clone();
            hop.headers = headers.clone();
            if parsed.https || !matches!(proxy_route(&hop.proxy), ProxyRoute::Direct) {
                #[cfg(windows)]
                {
                    return winhttp::get(&hop, &url, range);
                }
                #[cfg(not(windows))]
                {
                    return Err(EngineError::Failed("https/proxy needs WinHTTP".into()));
                }
            }
            http_get(&hop, &parsed, range)
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
                return Err(EngineError::Failed("redirect is not http(s)".into()));
            }
            let status = fetched.status;
            drop(fetched);
            drop_cross_origin_secrets(&mut headers, &replay, &url, &next);
            apply_redirect_method(&mut hop, request_method(job), status, &mut headers);
            url = next;
            continue;
        }
        return Ok((fetched, url));
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
    let root = if spec.work_dir.is_empty() {
        std::env::temp_dir()
    } else {
        PathBuf::from(&spec.work_dir)
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = root.join(format!("metadata-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let result = (|| {
        let control = crate::net_policy::current_throttle_context()
            .map(|c| c.control)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| dir.join("control"));
        if !control.exists() {
            fs::write(&control, "run").map_err(|e| e.to_string())?;
        }
        let mut job = Job {
            url: spec.url.clone(),
            headers: spec.headers.clone().into_iter().collect(),
            output: dir.join("body"),
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
            progress: dir.join("progress"),
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
            origin_boundary: spec.origin_boundary.clone(),
        };
        apply_browser_profile(&mut job.headers);
        let (fetched, final_url) =
            fetch_follow_restricted(&job, range, origin, None).map_err(|e| e.to_string())?;
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
                crate::net_policy::consume(n, || check_job_control(&job))
                    .map_err(|e| e.to_string())?;
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
            final_url,
            body,
            range: parsed_range,
            etag,
            last_modified,
            content_length,
        })
    })();
    let _ = fs::remove_dir_all(dir);
    result
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
    if !job.replay_json.trim().is_empty() {
        job.replay_json.clone()
    } else {
        crate::credentials::scoped_replay_json()
    }
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
    headers.retain(|key, _| {
        !key.eq_ignore_ascii_case("cookie")
            && !key.eq_ignore_ascii_case("authorization")
            && !key.eq_ignore_ascii_case("proxy-authorization")
    });
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

fn fetch_via_curl_impersonate(
    job: &Job,
    range: Option<&str>,
) -> Option<Result<FetchResult, EngineError>> {
    let exe = curl_impersonate_exe()?;
    if matches!(proxy_route(&job.proxy), ProxyRoute::System) {
        return None;
    }
    // 逐跳入口已匹配域/路径并处理重试；底层传输不能再次重放而覆盖当前请求头。
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
        let body_path = dir.join("body");
        let header_path = dir.join("headers");
        let status_path = dir.join("status");
        let error_path = dir.join("stderr");
        let mut command = std::process::Command::new(exe);
        command
            .arg("--compressed")
            .arg("--impersonate")
            .arg("chrome146")
            .arg("-sS")
            .arg("--connect-timeout")
            .arg("15")
            // A total `--max-time` cap made every download above ~60 s on this
            // fallback path abort mid-transfer.  The rest of the engine bounds stalls
            // (Tcp read timeout, WinHttp 60 s receive timeout), so mirror that shape:
            // abort only when the transfer stalls, never because it is merely long.
            .arg("--speed-time")
            .arg("30")
            .arg("--speed-limit")
            .arg("1024")
            .arg("-D")
            .arg(&header_path)
            .arg("-o")
            .arg(&body_path)
            .arg("-w")
            .arg("%{http_code}");
        command.arg("-A").arg(request_user_agent(headers));
        if job.curl_body_limit.is_some() {
            // 让既有进程等待循环能看到已解码正文，不能留在 curl 的写缓冲中。
            command.arg("--no-buffer");
        }
        for (key, value) in headers {
            if key.eq_ignore_ascii_case("user-agent") || !header_allowed_on_request(key, value) {
                continue;
            }
            command.arg("-H").arg(format!("{key}: {value}"));
        }
        if let Some(range) = range {
            command.arg("-H").arg(format!("Range: {range}"));
        }
        command.args(curl_request_args(job, headers));
        match proxy_route(&job.proxy) {
            ProxyRoute::Direct => {
                command.arg("--noproxy").arg("*");
            }
            ProxyRoute::System => unreachable!("system route skips curl-impersonate"),
            ProxyRoute::Manual(proxy) => {
                command.arg("-x").arg(proxy);
            }
        }
        command.arg(&job.url);
        // 输出写文件，避免等待退出时 stderr 管道填满；暂停/取消须能中止在途请求。
        let status_file =
            File::create(&status_path).map_err(|error| EngineError::Failed(error.to_string()))?;
        let error_file =
            File::create(&error_path).map_err(|error| EngineError::Failed(error.to_string()))?;
        let mut child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(status_file))
            .stderr(std::process::Stdio::from(error_file))
            .spawn()
            .map_err(|error| EngineError::Failed(error.to_string()))?;
        drop(command);
        let exit = loop {
            if let Err(error) = check_job_control(job) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            if let Some(limit) = job.curl_body_limit {
                if fs::metadata(&body_path).is_ok_and(|meta| meta.len() > limit) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(EngineError::Failed(format!(
                        "byte range response body exceeds {limit} bytes"
                    )));
                }
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    // 进程退出立即唤醒，避免给每个小分片固定增加一次轮询间隔。
                    #[cfg(windows)]
                    unsafe {
                        use std::os::windows::io::AsRawHandle;
                        windows_sys::Win32::System::Threading::WaitForSingleObject(
                            child.as_raw_handle() as _,
                            100,
                        );
                    }
                    #[cfg(not(windows))]
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(EngineError::Failed(error.to_string()));
                }
            }
        };
        drop(child);
        if let Some(limit) = job.curl_body_limit {
            if fs::metadata(&body_path).is_ok_and(|meta| meta.len() > limit) {
                return Err(EngineError::Failed(format!(
                    "byte range response body exceeds {limit} bytes"
                )));
            }
        }
        // HTTP 200 只证明收到响应头；curl 非零退出仍可能表示正文截断，不能交给 HLS 当成完整分片。
        if !exit.success() {
            let error = fs::read_to_string(&error_path).unwrap_or_default();
            return Err(EngineError::Failed(format!(
                "curl-impersonate transfer failed ({exit}): {}",
                error.trim()
            )));
        }
        let status = fs::read_to_string(&status_path)
            .map_err(|error| EngineError::Failed(error.to_string()))?
            .trim()
            .parse::<u16>()
            .unwrap_or(0);
        let headers = fs::read_to_string(&header_path).unwrap_or_default();
        let meta = parse_header_meta(&headers);
        record_download_type(job, status, &headers);
        // A body read failure must not look like an empty body: `unwrap_or_default`
        // silently turned a partial transfer into a truncated-but-"successful" file.
        // The entity itself stays on disk and is streamed (see `Body::File`), so only
        // the open is checked here and nothing is buffered into RAM.
        let body_file = File::open(&body_path).map_err(|error| {
            EngineError::Failed(format!("curl-impersonate body read failed: {error}"))
        })?;
        // `--compressed` makes this branch request gzip/br and hand *decoded* bytes
        // on disk, while the header dump still reports the compressed wire length.
        // Reporting that wire length as the content length makes `download_sequential`
        // reject every complete download from a compressing server, so the length of
        // what curl actually wrote wins.
        let content_length = if meta.content_encoding {
            fs::metadata(&body_path)
                .map(|file_meta| file_meta.len())
                .ok()
        } else {
            meta.content_length
        };
        Ok(FetchResult {
            status,
            location: meta.location.clone(),
            content_range: meta.content_range,
            content_length,
            content_type: meta.content_type,
            content_encoding: false,
            retry_after: meta.retry_after,
            etag: meta.etag,
            last_modified: meta.last_modified,
            accept_ranges: meta.accept_ranges,
            // `dir` moves into the reader: `Drop for Body` removes it once the entity
            // has been consumed, including on the early `?` returns in the caller.
            body: Body::File {
                file: body_file,
                temp_dir: dir.clone(),
            },
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

fn header_is_wire_safe(key: &str, value: &str) -> bool {
    !key.is_empty()
        && !key.contains(['\r', '\n', ':'])
        && !value.contains('\r')
        && !value.contains('\n')
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
    !value.is_empty() && !value.contains(['\r', '\n', '\0'])
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
    } else {
        (false, "http://".len())
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

fn parse_header_meta(head: &str) -> HeaderMeta {
    let mut meta = HeaderMeta::default();
    for line in head.lines() {
        if let Some(value) = header_value(line, "Location") {
            meta.location = Some(value.to_string());
        }
        if let Some(value) = header_value(line, "Content-Length") {
            meta.content_length = value.parse::<u64>().ok();
        }
        if let Some(value) = header_value(line, "Content-Type") {
            meta.content_type = value.to_string();
        }
        if let Some(value) = header_value(line, "Content-Range") {
            meta.content_range = Some(value.to_string());
        }
        if let Some(value) = header_value(line, "ETag") {
            meta.etag = value.to_string();
        }
        if let Some(value) = header_value(line, "Last-Modified") {
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
            meta.chunked = value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("chunked"));
        }
        if let Some(value) = header_value(line, "Content-Encoding") {
            // `identity` and an empty value describe the unchanged entity.
            meta.content_encoding =
                !value.trim().is_empty() && !value.trim().eq_ignore_ascii_case("identity");
        }
    }
    if meta.chunked {
        meta.content_length = None;
    }
    meta
}

fn parse_content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let mut text = value.trim();
    if text
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bytes"))
    {
        text = text[5..].trim();
    }
    let (range, total_text) = text.split_once('/')?;
    let (start_text, end_text) = range.split_once('-')?;
    let start = start_text.trim().parse::<u64>().ok()?;
    let end = end_text.trim().parse::<u64>().ok()?;
    let total_text = total_text.trim();
    let total = if total_text == "*" {
        None
    } else {
        Some(total_text.parse::<u64>().ok()?)
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
) -> Result<FetchResult, EngineError> {
    let mut stream = match proxy_route(&job.proxy) {
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
            let status = String::from_utf8_lossy(&buf[header_start..split])
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|code| code.parse::<u16>().ok())
                .unwrap_or(0);
            if (100..200).contains(&status) && status != 101 {
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
    let meta = parse_header_meta(&head);
    record_download_type(job, status, &head);
    for value in head
        .lines()
        .filter_map(|line| header_value(line, "Transfer-Encoding"))
    {
        if value
            .split(',')
            .any(|coding| !coding.trim().eq_ignore_ascii_case("chunked"))
        {
            return Err(EngineError::Failed("unsupported Transfer-Encoding".into()));
        }
    }
    let accept_ranges = meta.accept_ranges || meta.content_range.is_some();
    let body = Body::Tcp {
        prefix: leftover,
        at: 0,
        stream,
        remaining: meta.content_length,
    };
    let body = if meta.chunked && !request_method(job).eq_ignore_ascii_case("HEAD") {
        Body::Chunked(ChunkedReader::new(Box::new(body)))
    } else {
        body
    };
    Ok(FetchResult {
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
        WINHTTP_FLAG_ASYNC, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_CONTEXT_VALUE,
        WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
        WINHTTP_QUERY_ACCEPT_RANGES, WINHTTP_QUERY_CONTENT_RANGE, WINHTTP_QUERY_ETAG,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LAST_MODIFIED, WINHTTP_QUERY_LOCATION,
        WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE,
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
            let connect_key = super::origin_connect_key(proxy, &parsed.host, parsed.port);
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
            if !extra.is_empty() {
                let _ = WinHttpAddRequestHeaders(
                    request,
                    wide(&extra).as_ptr(),
                    extra.encode_utf16().count() as u32,
                    WINHTTP_ADDREQ_FLAG_ADD,
                );
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
            let content_range = query_header_string(request, WINHTTP_QUERY_CONTENT_RANGE);
            let raw_headers =
                query_header_string(request, WINHTTP_QUERY_RAW_HEADERS_CRLF).unwrap_or_default();
            if !raw_headers.is_empty() {
                super::record_download_type(job, status as u16, &raw_headers);
            }
            let location = query_header_string(request, WINHTTP_QUERY_LOCATION);
            let etag = query_header_string(request, WINHTTP_QUERY_ETAG).unwrap_or_default();
            let last_modified =
                query_header_string(request, WINHTTP_QUERY_LAST_MODIFIED).unwrap_or_default();
            let meta = super::parse_header_meta(&raw_headers);
            let content_length = meta.content_length;
            let accept_ranges =
                query_header_string(request, WINHTTP_QUERY_ACCEPT_RANGES).is_some_and(|value| {
                    value
                        .split(',')
                        .any(|part| part.trim().eq_ignore_ascii_case("bytes"))
                }) || content_range.is_some();
            body.recycle_connect = true;
            Ok(super::FetchResult {
                status: status as u16,
                location,
                content_range,
                content_length,
                content_type: meta.content_type,
                content_encoding: meta.content_encoding,
                retry_after: meta.retry_after,
                etag,
                last_modified,
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
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    };

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
            origin_boundary: String::new(),
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
        let fetched = winhttp::get(&job, &job.url, None).unwrap();
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
    fn adaptive_connections_ramp_gradually_and_back_off_immediately() {
        let controller = AdaptiveConnectionController::new(8);
        assert_eq!(controller.desired(), 2);

        controller.note_success();
        assert_eq!(controller.desired(), 2);
        controller.note_success();
        assert_eq!(controller.desired(), 3);

        for _ in 0..3 {
            controller.note_success();
        }
        assert_eq!(controller.desired(), 4);

        controller.note_congestion();
        assert_eq!(controller.desired(), 2);
        controller.note_success();
        assert_eq!(controller.desired(), 2);
        controller.note_success();
        assert_eq!(controller.desired(), 3);
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
        assert_eq!(scheduler.connections.desired(), 3);

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
    fn adversarial_metadata_fetch_stops_at_the_byte_limit() {
        for (response, expected) in [
            ("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabcde", None),
            ("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabcde", None),
            ("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n0\r\n\r\n", None),
            ("HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd", Some("abcd")),
            ("HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabc", None),
            ("HTTP/1.1 304 Not Modified\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n", Some("")),
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
                let result = http_get(&job, &parse_http_url(&url).unwrap(), None);
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
            let result = http_get(&job, &parse_http_url(&url).unwrap(), None);
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
                fs::write(&job.output, b"abcd").unwrap();
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
        let plain = parse_header_meta("Content-Length: 4");
        assert!(!plain.content_encoding);
        assert_eq!(plain.content_length, Some(4));

        let encoded = parse_header_meta("Content-Length: 4\r\nContent-Encoding: gzip");
        assert!(encoded.content_encoding);
        assert!(parse_header_meta("Content-Encoding: br").content_encoding);

        // `identity` and an empty value describe the unchanged entity.
        assert!(!parse_header_meta("Content-Encoding: identity").content_encoding);
        assert!(!parse_header_meta("Content-Encoding:   ").content_encoding);
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
        let mut transports = vec!["raw"];
        #[cfg(windows)]
        transports.push("winhttp");
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
                    "HTTP/1.1 302 Found\r\nLocation: ?token=a%2Fb&b=2\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
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
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
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
            parse_header_meta("Retry-After: 12\r\n").retry_after,
            Some(Duration::from_secs(12))
        );
        assert!(parse_header_meta("Retry-After: invalid\r\n")
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
        let mut fetched = winhttp::get(&job, &url, Some("bytes=4-9")).unwrap();
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
        job.method = "POST".into();
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
    #[ignore = "Requires bundled curl-impersonate; run explicitly in Windows Sandbox"]
    fn curl_transport_403_retries_replayed_cookies() {
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
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
    #[ignore = "Requires bundled curl-impersonate; run explicitly in Windows Sandbox"]
    fn curl_transport_preserves_browser_cookies_and_rejects_truncated_body() {
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
        assert!(
            curl_impersonate_exe().is_some(),
            "bundled curl-impersonate required"
        );
        for truncated in [false, true] {
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
            if truncated {
                assert!(
                    result.is_err(),
                    "HTTP 200 with an incomplete body must fail"
                );
            } else {
                let mut fetched = result.unwrap();
                assert_eq!(fetched.status, 200);
                let mut body = Vec::new();
                fetched.body.read_to_end(&mut body).unwrap();
                assert_eq!(body, b"ok");
                let Body::File { temp_dir, .. } = &fetched.body else {
                    panic!("expected file body");
                };
                let temporary = temp_dir.clone();
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
    #[ignore = "Requires bundled curl-impersonate; run explicitly in Windows Sandbox"]
    fn curl_transport_range_body_limit_uses_decoded_bytes_and_stops_before_eof() {
        use std::sync::mpsc;
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
        assert!(curl_impersonate_exe().is_some(), "bundled curl required");
        for case in [
            "gzip-valid",
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
                let (framing, body) = match case {
                    "gzip-valid" => (
                        "Content-Encoding: gzip\r\nContent-Length: 28\r\n",
                        gzip_valid,
                    ),
                    "gzip-oversize" => (
                        "Content-Encoding: gzip\r\nContent-Length: 29\r\n",
                        gzip_oversize,
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
            let worker = thread::spawn(move || {
                let _ = done_tx.send(fetch_via_curl_impersonate(&worker_job, Some("bytes=0-7")));
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let result = done_rx.recv_timeout(Duration::from_secs(3));
            let _ = release_tx.send(());
            server.join().unwrap();
            worker.join().unwrap();
            let result = result
                .expect("curl must reject oversize before EOF")
                .expect("curl available");
            if case == "gzip-valid" {
                let mut fetched = result.unwrap();
                let mut body = Vec::new();
                fetched.body.read_to_end(&mut body).unwrap();
                assert_eq!(body, b"12345678");
                assert_eq!(fetched.content_length, Some(8));
            } else {
                let error = match result {
                    Err(error) => error,
                    Ok(_) => panic!("{case}: oversized response accepted"),
                };
                assert!(
                    error
                        .to_string()
                        .contains("byte range response body exceeds 8"),
                    "{case}: {error}"
                );
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires bundled curl-impersonate; run explicitly in Windows Sandbox"]
    fn curl_transport_stalled_headers_and_body_obey_pause_and_cancel() {
        use std::sync::mpsc;
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
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
                    let _ = done_tx.send(fetch_via_curl_impersonate(&worker_job, None));
                });
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                fs::write(&job.control, action).unwrap();
                // 服务端只有收到释放信号才结束；结果必须在网络释放之前返回。
                let result = done_rx.recv_timeout(Duration::from_secs(3));
                let _ = release_tx.send(());
                server.join().unwrap();
                worker.join().unwrap();
                let error = match result
                    .expect("stalled curl did not stop promptly")
                    .expect("curl available")
                {
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

    // The curl-impersonate path used to buffer the whole entity into RAM and only
    // then hand it to the writer, so a multi-gigabyte download became an equally
    // large allocation and the process was OOM-killed mid-transfer.  `Body::File`
    // streams from disk instead; these two properties are what keep it honest.
    #[test]
    fn a_file_backed_body_streams_from_disk_and_cleans_up_its_temp_dir() {
        let root = std::env::temp_dir().join(format!(
            "hls-body-file-{}-{}",
            std::process::id(),
            NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        // 320 KiB forces several 64 KiB reads, so the reader really is streaming
        // rather than handing back the whole buffer in one call.
        let data = vec![0x5au8; 320 * 1024];
        fs::write(root.join("body"), &data).unwrap();
        let mut body = Body::File {
            file: File::open(root.join("body")).unwrap(),
            temp_dir: root.clone(),
        };
        let mut copied = Vec::new();
        body.read_to_end(&mut copied).unwrap();
        assert_eq!(copied, data);
        drop(body);
        assert!(
            !root.exists(),
            "the temp dir must not outlive the reader that owns it"
        );
    }

    // Dropping the reader part-way is what happens on every `?` early return in
    // `download_sequential`, so the cleanup has to hold on that path too.
    #[test]
    fn a_file_backed_body_dropped_early_still_cleans_up_its_temp_dir() {
        let root = std::env::temp_dir().join(format!(
            "hls-body-file-early-{}-{}",
            std::process::id(),
            NEXT_TEMP_JOB_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("body"), b"0123456789").unwrap();
        let mut body = Body::File {
            file: File::open(root.join("body")).unwrap(),
            temp_dir: root.clone(),
        };
        let mut one = [0u8; 4];
        assert_eq!(body.read(&mut one).unwrap(), 4);
        drop(body);
        assert!(!root.exists(), "an early drop must still clean up");
    }
}
