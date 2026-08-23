//! Range GET into one `payload.downloading`.
//!
//! No extra process and no TLS crate: loopback/http uses the same tiny
//! TcpStream client as the core poller; Windows https uses WinHTTP.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_PAUSE: i32 = 20;
pub const EXIT_CANCEL: i32 = 21;
pub const EXIT_RANGE_UNSUPPORTED: i32 = 30;

const WRITE_BATCH: usize = 256 * 1024;
const DURABLE_CHECKPOINT_BYTES: u64 = 4 * 1024 * 1024;
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
        self.idle.get_mut(key).and_then(Vec::pop)
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

impl EngineError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Pause => EXIT_PAUSE,
            Self::Cancel => EXIT_CANCEL,
            Self::RangeUnsupported(_) => EXIT_RANGE_UNSUPPORTED,
            Self::Failed(_) => EXIT_ERROR,
        }
    }
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

pub fn load_job(path: &Path) -> Result<Job, EngineError> {
    let text = fs::read_to_string(path).map_err(|err| EngineError::Failed(err.to_string()))?;
    serde_json::from_str(&text).map_err(|err| EngineError::Failed(err.to_string()))
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
    write_progress_status(path, downloaded, total, speed, status, None, None);
}

pub fn write_progress_status(
    path: &Path,
    downloaded: u64,
    total: u64,
    speed: f64,
    status: &str,
    code: Option<i32>,
    error: Option<&str>,
) {
    let mut payload = serde_json::json!({
        "downloaded": downloaded,
        "total": total,
        "speed": speed,
        "status": status,
    });
    if let Some(code) = code {
        payload["code"] = serde_json::json!(code);
    }
    if let Some(error) = error {
        payload["error"] = serde_json::json!(error);
    }
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = fs::rename(tmp, path);
    }
}

fn last_progress_bytes(path: &Path) -> (u64, u64) {
    let Ok(text) = fs::read_to_string(path) else {
        return (0, 0);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (0, 0);
    };
    (
        value
            .get("downloaded")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        value
            .get("total")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    )
}

fn completed_ranges_path(job: &Job) -> PathBuf {
    job.progress.with_file_name("native-engine.ranges.json")
}

#[cfg(windows)]
fn replace_checkpoint_file(source: &Path, destination: &Path) -> std::io::Result<()> {
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
fn replace_checkpoint_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
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
    let Some(items) = value.get("ranges").and_then(|item| item.as_array()) else {
        return None;
    };
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
            .any(|(start, end)| *start >= job.total || *end >= job.total)
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
    replace_checkpoint_file(&tmp, &path)
        .map_err(|err| EngineErrorCode::Failed(format!("checkpoint replace failed: {err}")))
}

fn record_completed_range(
    job: &Job,
    completed: &Mutex<Vec<(u64, u64)>>,
    start: u64,
    end: u64,
) -> Result<(), EngineErrorCode> {
    let mut list = completed.lock().unwrap_or_else(|err| err.into_inner());
    if range_covered(&list, start, end) {
        return Ok(());
    }
    list.push((start, end));
    *list = normalize_ranges(std::mem::take(&mut *list));
    save_completed_ranges(job, &list)
}

fn report_terminal(job: &Job, error: &EngineError) {
    let (downloaded, total) = last_progress_bytes(&job.progress);
    let status = match error {
        EngineError::Pause => "paused",
        EngineError::Cancel => "canceled",
        EngineError::RangeUnsupported(_) => "error",
        EngineError::Failed(_) => "error",
    };
    write_progress_status(
        &job.progress,
        downloaded,
        total,
        0.0,
        status,
        Some(error.exit_code()),
        Some(&error.to_string()),
    );
}

/// Run a job and always leave a terminal progress JSON (pause/cancel/error too).
/// `--job` and the resident supervisor share this so Python can wait without a child process.
pub fn finish_job(job: &Job) -> Result<(), EngineError> {
    match run_job(job) {
        Ok(()) => Ok(()),
        Err(error) => {
            report_terminal(job, &error);
            Err(error)
        }
    }
}

pub fn run_queued_job(job_path: &Path, progress_path: Option<&Path>) {
    match load_job(job_path) {
        Ok(job) => {
            let _ = finish_job(&job);
        }
        Err(error) => {
            let fallback = job_path.with_file_name("native-engine.progress.json");
            let progress = progress_path.unwrap_or(fallback.as_path());
            write_progress_status(
                progress,
                0,
                0,
                0.0,
                "error",
                Some(error.exit_code()),
                Some(&error.to_string()),
            );
        }
    }
}

const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

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
        if name == "__cf_bm" || name == "__cflb" {
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
    if let Ok(path) = std::env::var("HLS_V6_CURL_IMPERSONATE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let names = [
        "curl-impersonate.exe",
        "curl_chrome131.exe",
        "curl-impersonate-chrome.exe",
    ];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in names {
                let path = dir.join(name);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceProbe {
    pub total: Option<u64>,
    pub accept_ranges: bool,
    pub etag: String,
    pub last_modified: String,
}

pub fn probe_resource(job: &Job) -> Result<ResourceProbe, EngineError> {
    check_control(&job.control)?;
    let fetched = fetch(job, Some("bytes=0-0"))?;
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
    Ok(ResourceProbe {
        total,
        accept_ranges,
        etag: fetched.etag,
        last_modified: fetched.last_modified,
    })
}

pub fn run_job(job: &Job) -> Result<(), EngineError> {
    let mut urls = vec![job.url.clone()];
    urls.extend(crate::mirrors::normalize_mirror_urls(
        &job.url,
        &job.mirrors,
    ));
    let mut last = EngineError::Failed("job url missing".into());
    let post = job.method.eq_ignore_ascii_case("POST");
    let mut identity: Option<(Option<u64>, String)> = None;
    if job.total > 0 || !job.etag.trim().is_empty() {
        identity = Some(((job.total > 0).then_some(job.total), job.etag.clone()));
    }
    for (index, url) in urls.into_iter().enumerate() {
        let mut attempt = job.clone();
        attempt.url = url;
        if !post && !attempt.sequential && (attempt.total == 0 || index > 0) {
            match probe_resource(&attempt) {
                Ok(probe) => {
                    if let Some((len, etag)) = &identity {
                        if !crate::mirrors::mirror_identity_compatible(
                            *len,
                            etag,
                            probe.total,
                            &probe.etag,
                        ) {
                            last = EngineError::Failed("mirror identity mismatch".into());
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
                    if attempt.etag.is_empty() {
                        attempt.etag = probe.etag;
                    }
                    if attempt.last_modified.is_empty() {
                        attempt.last_modified = probe.last_modified;
                    }
                    if !probe.accept_ranges || attempt.total == 0 {
                        attempt.sequential = true;
                    }
                }
                Err(error) => {
                    last = error;
                    continue;
                }
            }
        }
        match run_job_once(&attempt) {
            Ok(()) => return Ok(()),
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn run_job_once(job: &Job) -> Result<(), EngineError> {
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
    let _slot = crate::net_policy::acquire(&job.url).map_err(EngineError::Failed)?;
    if job.method.eq_ignore_ascii_case("POST") || job.sequential || job.total == 0 {
        let _ = fs::remove_file(completed_ranges_path(job));
        download_sequential(job)
    } else {
        download_ranges(job)
    }
}

pub fn fetch_bytes(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
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
    };
    let fetched = fetch(&job, None)?;
    let status = fetched.status;
    let mut body = Vec::new();
    let mut reader = fetched.body;
    reader
        .read_to_end(&mut body)
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    crate::net_policy::consume(body.len());
    let _ = fs::remove_dir_all(dir);
    Ok((status, body))
}

fn check_control(path: &Path) -> Result<(), EngineError> {
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
                _file.as_raw_handle() as *mut core::ffi::c_void,
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

fn download_sequential(job: &Job) -> Result<(), EngineError> {
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
    let fetched = fetch(job, range.as_deref())?;
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
    }
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
    let started = Instant::now();
    let mut last_progress = Instant::now();
    loop {
        check_control(&job.control)?;
        let count = reader
            .read(&mut buffer)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        if count == 0 {
            break;
        }
        crate::net_policy::consume(count);
        file.write_all(&buffer[..count])
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        downloaded += count as u64;
        if last_progress.elapsed() >= Duration::from_millis(200) {
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            write_progress(
                &job.progress,
                downloaded,
                job.total.max(downloaded),
                downloaded as f64 / elapsed,
                "downloading",
            );
            last_progress = Instant::now();
        }
    }
    file.flush()
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    write_progress(&job.progress, downloaded, downloaded, 0.0, "done");
    Ok(())
}

fn download_ranges(job: &Job) -> Result<(), EngineError> {
    let total = job.total;
    if total == 0 {
        return download_sequential(job);
    }
    let workers = job.connections.clamp(1, 64);
    // Keep several unrequested ranges available per worker. Splitting an HTTP
    // request after it has been sent only shortens the file write; the origin
    // still transmits the old tail, wasting bandwidth. Queue-level balancing
    // lets idle workers help without overlapping any network request.
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
        let _ = fs::remove_file(completed_ranges_path(job));
        write_progress(&job.progress, total, total, 0.0, "done");
        return Ok(());
    }
    let scheduler = Arc::new(RangeScheduler::new(
        pending.iter().copied().collect(),
        workers,
    ));
    let downloaded = Arc::new(AtomicU64::new(already));
    let completed = Arc::new(Mutex::new(loaded));
    let failed = Arc::new(Mutex::new(None::<EngineErrorCode>));
    let stop = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
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
                );
            });
        }));
    }
    let progress_stop = Arc::clone(&stop);
    let progress_downloaded = Arc::clone(&downloaded);
    let progress_path = job.progress.clone();
    let progress_control = job.control.clone();
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
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            write_progress(
                &progress_path,
                bytes,
                total,
                bytes as f64 / elapsed,
                "downloading",
            );
            thread::sleep(Duration::from_millis(200));
        }
    });
    for handle in handles {
        let _ = handle.join();
    }
    stop.store(true, Ordering::SeqCst);
    let _ = progress.join();
    if let Some(code) = failed.lock().unwrap_or_else(|err| err.into_inner()).take() {
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
    let _ = fs::remove_file(completed_ranges_path(job));
    write_progress(&job.progress, total, total, 0.0, "done");
    Ok(())
}

#[derive(Clone, Debug)]
enum EngineErrorCode {
    Pause,
    Cancel,
    RangeUnsupported(String),
    Failed(String),
}

impl EngineErrorCode {
    fn into_error(self) -> EngineError {
        match self {
            Self::Pause => EngineError::Pause,
            Self::Cancel => EngineError::Cancel,
            Self::RangeUnsupported(message) => EngineError::RangeUnsupported(message),
            Self::Failed(message) => EngineError::Failed(message),
        }
    }
}

#[derive(Clone, Copy)]
struct WorkRange {
    start: u64,
    end: u64,
}

struct ActiveProgress {
    cursor: u64,
    stop: u64,
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
        if successes < desired {
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
    connections: AdaptiveConnectionController,
    wake_lock: Mutex<()>,
    wake: Condvar,
}

impl RangeScheduler {
    fn new(pending: Vec<WorkRange>, maximum_connections: usize) -> Self {
        Self {
            pending: Mutex::new(pending),
            active: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            connections: AdaptiveConnectionController::new(maximum_connections),
            wake_lock: Mutex::new(()),
            wake: Condvar::new(),
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
        let index = pending
            .iter()
            .enumerate()
            .max_by_key(|(_, item)| item.end.saturating_sub(item.start))
            .map(|(index, _)| index)?;
        let range = pending.swap_remove(index);
        drop(pending);
        let active = ActiveRange {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            progress: Arc::new(Mutex::new(ActiveProgress {
                cursor: range.start,
                stop: range.end,
            })),
        };
        self.active
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(ActiveRange {
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

    fn complete(&self, id: u64) {
        let mut active = self.active.lock().unwrap_or_else(|err| err.into_inner());
        active.retain(|item| item.id != id);
        let active_empty = active.is_empty();
        drop(active);
        if active_empty
            && self
                .pending
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .is_empty()
        {
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
}

fn range_worker(
    worker_index: usize,
    job: &Job,
    scheduler: Arc<RangeScheduler>,
    downloaded: Arc<AtomicU64>,
    completed: Arc<Mutex<Vec<(u64, u64)>>>,
    failed: Arc<Mutex<Option<EngineErrorCode>>>,
    stop: Arc<AtomicBool>,
) {
    let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&job.output) else {
        let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
        if slot.is_none() {
            *slot = Some(EngineErrorCode::Failed("open payload failed".into()));
        }
        stop.store(true, Ordering::SeqCst);
        return;
    };
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match read_control(&job.control) {
            Control::Pause => {
                let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                if slot.is_none() {
                    *slot = Some(EngineErrorCode::Pause);
                }
                stop.store(true, Ordering::SeqCst);
                return;
            }
            Control::Cancel => {
                let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                if slot.is_none() {
                    *slot = Some(EngineErrorCode::Cancel);
                }
                stop.store(true, Ordering::SeqCst);
                return;
            }
            Control::Run => {}
        }
        let Some(active) = scheduler.claim(worker_index) else {
            if scheduler.is_idle() {
                return;
            }
            scheduler.wait_for_change();
            continue;
        };
        let result = fetch_range(
            job,
            &mut file,
            &active.progress,
            &downloaded,
            &completed,
            &scheduler,
        );
        scheduler.complete(active.id);
        match result {
            Ok((start, end)) => {
                if let Err(error) = record_completed_range(job, &completed, start, end) {
                    let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                    if slot.is_none() {
                        *slot = Some(error);
                    }
                    stop.store(true, Ordering::SeqCst);
                    return;
                }
                scheduler.note_success();
            }
            Err(error) => {
                let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
                if slot.is_none() {
                    *slot = Some(error);
                }
                stop.store(true, Ordering::SeqCst);
                return;
            }
        }
    }
}

fn persist_range_progress(
    job: &Job,
    file: &mut File,
    completed: &Mutex<Vec<(u64, u64)>>,
    start: u64,
    cursor: u64,
) -> Result<(), EngineErrorCode> {
    if cursor <= start {
        return Ok(());
    }
    file.flush()
        .map_err(|err| EngineErrorCode::Failed(err.to_string()))?;
    file.sync_data()
        .map_err(|err| EngineErrorCode::Failed(err.to_string()))?;
    record_completed_range(job, completed, start, cursor.saturating_sub(1))
}

fn wait_before_range_retry(job: &Job, failed_attempts: u32) -> Result<(), EngineErrorCode> {
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
        match read_control(&job.control) {
            Control::Pause => return Err(EngineErrorCode::Pause),
            Control::Cancel => return Err(EngineErrorCode::Cancel),
            Control::Run => {}
        }
        let step = (delay_ms - waited).min(50);
        thread::sleep(Duration::from_millis(step));
        waited += step;
    }
    Ok(())
}

fn retryable_http_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn fetch_range(
    job: &Job,
    file: &mut File,
    progress: &Arc<Mutex<ActiveProgress>>,
    downloaded: &AtomicU64,
    completed: &Mutex<Vec<(u64, u64)>>,
    scheduler: &RangeScheduler,
) -> Result<(u64, u64), EngineErrorCode> {
    let start = progress
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .cursor;
    let mut durable_cursor = start;
    let mut failed_attempts = 0u32;
    loop {
        let (mut cursor, target_end) = {
            let state = progress.lock().unwrap_or_else(|err| err.into_inner());
            (state.cursor, state.stop)
        };
        if cursor > target_end {
            return Ok((start, cursor.saturating_sub(1)));
        }
        let request_start = cursor;
        let range = format!("bytes={cursor}-{target_end}");
        let fetched = match fetch(job, Some(&range)) {
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
                wait_before_range_retry(job, failed_attempts)?;
                continue;
            }
        };
        if fetched.status == 200 {
            return Err(EngineErrorCode::RangeUnsupported(
                "server ignored Range and returned 200".into(),
            ));
        }
        if fetched.status != 206 {
            if retryable_http_status(fetched.status) {
                scheduler.note_congestion();
                failed_attempts += 1;
                if failed_attempts >= MAX_RANGE_ATTEMPTS {
                    return Err(EngineErrorCode::Failed(format!(
                        "HTTP {} after {failed_attempts} attempts",
                        fetched.status
                    )));
                }
                wait_before_range_retry(job, failed_attempts)?;
                continue;
            }
            return Err(EngineErrorCode::Failed(format!("HTTP {}", fetched.status)));
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
        let mut reader = fetched.body;
        if file.seek(SeekFrom::Start(cursor)).is_err() {
            return Err(EngineErrorCode::Failed("seek failed".into()));
        }
        let mut buffer = vec![0u8; WRITE_BATCH];
        let mut read_error = None;
        loop {
            match read_control(&job.control) {
                Control::Pause => {
                    persist_range_progress(job, file, completed, start, cursor)?;
                    return Err(EngineErrorCode::Pause);
                }
                Control::Cancel => {
                    persist_range_progress(job, file, completed, start, cursor)?;
                    return Err(EngineErrorCode::Cancel);
                }
                Control::Run => {}
            }
            let count = match reader.read(&mut buffer) {
                Ok(count) => count,
                Err(err) => {
                    read_error = Some(err.to_string());
                    break;
                }
            };
            if count == 0 {
                break;
            }
            let mut state = progress.lock().unwrap_or_else(|err| err.into_inner());
            let current_stop = state.stop;
            let remain = current_stop.saturating_add(1).saturating_sub(cursor) as usize;
            let take = count.min(remain);
            if take > 0 {
                crate::net_policy::consume(take);
            }
            if take > 0 && file.write_all(&buffer[..take]).is_err() {
                return Err(EngineErrorCode::Failed("write failed".into()));
            }
            cursor += take as u64;
            state.cursor = cursor;
            drop(state);
            downloaded.fetch_add(take as u64, Ordering::SeqCst);
            if cursor.saturating_sub(durable_cursor) >= DURABLE_CHECKPOINT_BYTES {
                persist_range_progress(job, file, completed, start, cursor)?;
                durable_cursor = cursor;
            }
            if cursor > current_stop {
                break;
            }
        }
        let current_stop = progress.lock().unwrap_or_else(|err| err.into_inner()).stop;
        if cursor > current_stop {
            persist_range_progress(job, file, completed, start, cursor)?;
            return Ok((start, cursor.saturating_sub(1)));
        }
        persist_range_progress(job, file, completed, start, cursor)?;
        durable_cursor = cursor;
        if cursor > request_start {
            failed_attempts = 0;
            scheduler.note_congestion();
            if read_error.is_some() {
                wait_before_range_retry(job, 1)?;
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
        wait_before_range_retry(job, failed_attempts)?;
    }
}

enum Body {
    Tcp {
        prefix: Vec<u8>,
        at: usize,
        stream: TcpStream,
        remaining: Option<u64>,
    },
    Memory {
        data: Vec<u8>,
        at: usize,
    },
    #[cfg(windows)]
    WinHttp(winhttp::WinHttpBody),
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
            Self::Memory { data, at } => {
                if *at >= data.len() {
                    return Ok(0);
                }
                let take = (data.len() - *at).min(buf.len());
                buf[..take].copy_from_slice(&data[*at..*at + take]);
                *at += take;
                Ok(take)
            }
            #[cfg(windows)]
            Self::WinHttp(body) => body.read(buf),
        }
    }
}

struct FetchResult {
    status: u16,
    location: Option<String>,
    content_range: Option<String>,
    content_length: Option<u64>,
    etag: String,
    last_modified: String,
    accept_ranges: bool,
    body: Body,
}

fn fetch(job: &Job, range: Option<&str>) -> Result<FetchResult, EngineError> {
    let mut prepared = job.clone();
    apply_browser_profile(&mut prepared.headers);
    let first = fetch_follow(&prepared, range)?;
    if first.status != 403 {
        return Ok(first);
    }
    if let Some(stripped) = strip_stale_cloudflare_cookies(&prepared.headers) {
        drop(first);
        prepared.headers = stripped;
        apply_browser_profile(&mut prepared.headers);
        let second = fetch_follow(&prepared, range)?;
        if second.status != 403 {
            return Ok(second);
        }
        if let Some(via_curl) = fetch_via_curl_impersonate(&prepared, range) {
            return via_curl;
        }
        return Ok(second);
    }
    if let Some(via_curl) = fetch_via_curl_impersonate(&prepared, range) {
        return via_curl;
    }
    Ok(first)
}

fn fetch_follow(job: &Job, range: Option<&str>) -> Result<FetchResult, EngineError> {
    let mut url = job.url.clone();
    let replay = job_replay_json(job);
    let mut headers = headers_for_request(job, &url);
    for _ in 0..16 {
        let mut hop = job.clone();
        hop.url = url.clone();
        hop.headers = headers.clone();
        hop.replay_json = replay.clone();
        let parsed = parse_http_url(&url)?;
        let use_winhttp = parsed.https || !hop.proxy.trim().is_empty();
        if use_winhttp {
            #[cfg(windows)]
            {
                let fetched = winhttp::get(&hop, &url, range)?;
                if matches!(fetched.status, 301 | 302 | 303 | 307 | 308) {
                    let next = fetched
                        .location
                        .clone()
                        .ok_or_else(|| EngineError::Failed("redirect without Location".into()))?;
                    let next = resolve_location(&parsed, &next);
                    if !http_fetch_url_allowed(&next) {
                        return Err(EngineError::Failed("redirect is not http(s)".into()));
                    }
                    drop(fetched);
                    drop_cross_origin_secrets(&mut headers, &replay, &url, &next);
                    url = next;
                    continue;
                }
                return Ok(fetched);
            }
            #[cfg(not(windows))]
            {
                return Err(EngineError::Failed("https/proxy needs WinHTTP".into()));
            }
        }
        let (
            status,
            location,
            content_range,
            content_length,
            etag,
            last_modified,
            accept_ranges,
            body,
        ) = http_get(&hop, &parsed, range)?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let next =
                location.ok_or_else(|| EngineError::Failed("redirect without Location".into()))?;
            let next = resolve_location(&parsed, &next);
            if !http_fetch_url_allowed(&next) {
                return Err(EngineError::Failed("redirect is not http(s)".into()));
            }
            drop_cross_origin_secrets(&mut headers, &replay, &url, &next);
            url = next;
            continue;
        }
        return Ok(FetchResult {
            status,
            location: None,
            content_range,
            content_length,
            etag,
            last_modified,
            accept_ranges,
            body,
        });
    }
    Err(EngineError::Failed("too many redirects".into()))
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
        return;
    }
    headers.retain(|key, _| {
        !key.eq_ignore_ascii_case("cookie") && !key.eq_ignore_ascii_case("authorization")
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
    let body_path = dir.join("body");
    let header_path = dir.join("headers");
    let mut command = std::process::Command::new(exe);
    command
        .arg("-sS")
        .arg("-D")
        .arg(&header_path)
        .arg("-o")
        .arg(&body_path)
        .arg("-w")
        .arg("%{http_code}")
        .arg("--http2");
    command.arg("-A").arg(request_user_agent(&job.headers));
    for (key, value) in &job.headers {
        if key.eq_ignore_ascii_case("user-agent") || !header_allowed_on_request(key, value) {
            continue;
        }
        command.arg("-H").arg(format!("{key}: {value}"));
    }
    if let Some(range) = range {
        command.arg("-H").arg(format!("Range: {range}"));
    }
    if !job.proxy.trim().is_empty() {
        command.arg("-x").arg(&job.proxy);
    }
    command.arg(&job.url);
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_dir_all(&dir);
            return Some(Err(EngineError::Failed(error.to_string())));
        }
    };
    let status = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u16>()
        .unwrap_or(0);
    let headers = fs::read_to_string(&header_path).unwrap_or_default();
    let meta = parse_header_meta(&headers);
    let data = fs::read(&body_path).unwrap_or_default();
    let _ = fs::remove_dir_all(&dir);
    if !output.status.success() && status == 0 {
        return Some(Err(EngineError::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )));
    }
    Some(Ok(FetchResult {
        status,
        location: meta.location.clone(),
        content_range: meta.content_range,
        content_length: meta.content_length,
        etag: meta.etag,
        last_modified: meta.last_modified,
        accept_ranges: meta.accept_ranges,
        body: Body::Memory { data, at: 0 },
    }))
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
    header_is_wire_safe(key, value) && !header_forbidden_on_wire(key)
}

fn header_token_ok(value: &str) -> bool {
    !value.is_empty() && !value.contains(['\r', '\n', '\0'])
}

pub(crate) fn sanitize_http_method(raw: &str) -> String {
    let method = raw.trim();
    if method.eq_ignore_ascii_case("POST") {
        "POST".into()
    } else if method.eq_ignore_ascii_case("HEAD") {
        "HEAD".into()
    } else {
        "GET".into()
    }
}

pub fn http_fetch_url_allowed(url: &str) -> bool {
    let url = url.trim().trim_start_matches('\u{feff}');
    if url.is_empty() || url.chars().any(|ch| ch.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

pub fn remote_resource_url_allowed(url: &str) -> bool {
    let url = url.trim().trim_start_matches('\u{feff}');
    if url.is_empty() || url.chars().any(|ch| ch.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("sftp://")
        || lower.starts_with("magnet:")
}

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
    let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = if let Some((host, port)) = hostport.rsplit_once(':') {
        if host.starts_with('[') {
            return Err(EngineError::Failed("ipv6 url unsupported".into()));
        }
        (
            host.to_string(),
            port.parse::<u16>()
                .map_err(|_| EngineError::Failed("invalid port".into()))?,
        )
    } else {
        (hostport.to_string(), if https { 443 } else { 80 })
    };
    if host.is_empty()
        || host
            .chars()
            .any(|ch| ch.is_ascii_whitespace() || matches!(ch, '/' | '\\' | '\0' | '#' | '?' | '@'))
    {
        return Err(EngineError::Failed("url host missing".into()));
    }
    if path.chars().any(|ch| ch.is_ascii_control() || ch == ' ') {
        return Err(EngineError::Failed("url path invalid".into()));
    }
    Ok(ParsedUrl {
        https,
        host,
        port,
        path: format!("/{path}"),
    })
}

fn host_header(parsed: &ParsedUrl) -> String {
    let default_port = if parsed.https { 443 } else { 80 };
    if parsed.port == default_port {
        parsed.host.clone()
    } else {
        format!("{}:{}", parsed.host, parsed.port)
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

#[derive(Default)]
struct HeaderMeta {
    location: Option<String>,
    content_length: Option<u64>,
    content_range: Option<String>,
    etag: String,
    last_modified: String,
    accept_ranges: bool,
    chunked: bool,
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
        if let Some(value) = header_value(line, "Content-Range") {
            meta.content_range = Some(value.to_string());
        }
        if let Some(value) = header_value(line, "ETag") {
            meta.etag = value.to_string();
        }
        if let Some(value) = header_value(line, "Last-Modified") {
            meta.last_modified = value.to_string();
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
    }
    meta
}

fn parse_content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let mut text = value.trim();
    if text.len() >= 5 && text[..5].eq_ignore_ascii_case("bytes") {
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
    let lower = location.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return location.to_string();
    }
    if has_absolute_scheme(location) {
        return String::new();
    }
    let scheme = if current.https { "https" } else { "http" };
    if let Some(rest) = location.strip_prefix("//") {
        return format!("{scheme}://{rest}");
    }
    let origin = format!("{scheme}://{}", host_header(current));
    if location.starts_with('/') {
        return format!("{origin}{location}");
    }
    let dir = match current.path.rfind('/') {
        Some(index) => &current.path[..=index],
        None => "/",
    };
    format!("{origin}{dir}{location}")
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
) -> Result<
    (
        u16,
        Option<String>,
        Option<String>,
        Option<u64>,
        String,
        String,
        bool,
        Body,
    ),
    EngineError,
> {
    let mut stream = if job.proxy.trim().is_empty() {
        TcpStream::connect((parsed.host.as_str(), parsed.port))
            .map_err(|err| EngineError::Failed(err.to_string()))?
    } else {
        return Err(EngineError::Failed("http proxy uses WinHTTP".into()));
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    let mut header = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\nAccept-Encoding: identity\r\nUser-Agent: {}\r\n",
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
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while buf.len() < 64 * 1024 {
        let count = stream
            .read(&mut byte)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        if count == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let split = buf
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| EngineError::Failed("response headers truncated".into()))?;
    let head = String::from_utf8_lossy(&buf[..split]);
    let leftover = buf[split + 4..].to_vec();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let meta = parse_header_meta(&head);
    if meta.chunked && !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Err(EngineError::Failed(
            "chunked Transfer-Encoding unsupported".into(),
        ));
    }
    let accept_ranges = meta.accept_ranges || meta.content_range.is_some();
    let remaining = meta
        .content_length
        .map(|total| total.saturating_sub(leftover.len() as u64));
    Ok((
        status,
        meta.location,
        meta.content_range,
        meta.content_length,
        meta.etag,
        meta.last_modified,
        accept_ranges,
        Body::Tcp {
            prefix: leftover,
            at: 0,
            stream,
            remaining,
        },
    ))
}

#[cfg(windows)]
mod winhttp {
    use super::{Body, EngineError, Job};
    use std::io::Read;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_NAMED_PROXY,
        WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE,
        WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
        WINHTTP_QUERY_ACCEPT_RANGES, WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_QUERY_CONTENT_RANGE,
        WINHTTP_QUERY_ETAG, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LAST_MODIFIED,
        WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE,
    };

    pub struct WinHttpBody {
        session: *mut core::ffi::c_void,
        connect: *mut core::ffi::c_void,
        request: *mut core::ffi::c_void,
        keep_session: bool,
        recycle_connect: bool,
        connect_key: String,
    }

    unsafe impl Send for WinHttpBody {}

    impl Drop for WinHttpBody {
        fn drop(&mut self) {
            unsafe {
                if !self.request.is_null() {
                    WinHttpCloseHandle(self.request);
                    self.request = null_mut();
                }
                if !self.connect.is_null() {
                    if self.recycle_connect && put_connect(&self.connect_key, self.connect) {
                        self.connect = null_mut();
                    } else {
                        WinHttpCloseHandle(self.connect);
                        self.connect = null_mut();
                    }
                }
                if !self.keep_session && !self.session.is_null() {
                    WinHttpCloseHandle(self.session);
                }
            }
        }
    }

    impl Read for WinHttpBody {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut got: u32 = 0;
            let ok = unsafe {
                WinHttpReadData(
                    self.request,
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                    &mut got,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::other(format!(
                    "WinHttpReadData {}",
                    unsafe { GetLastError() }
                )));
            }
            Ok(got as usize)
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
            let session = if proxy.is_empty() {
                WinHttpOpen(
                    wide(super::CHROME_UA).as_ptr(),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    null_mut(),
                    null_mut(),
                    0,
                )
            } else {
                WinHttpOpen(
                    wide(super::CHROME_UA).as_ptr(),
                    WINHTTP_ACCESS_TYPE_NAMED_PROXY,
                    wide(proxy).as_ptr(),
                    wide("").as_ptr(),
                    0,
                )
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
        let parsed = super::parse_http_url(url)?;
        unsafe {
            let proxy = job.proxy.trim();
            let session = cached_session(proxy)?;
            let connect_key = super::origin_connect_key(proxy, &parsed.host, parsed.port);
            let connect = take_connect(&connect_key).unwrap_or_else(|| {
                WinHttpConnect(session, wide(&parsed.host).as_ptr(), parsed.port, 0)
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
            let mut policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
            if WinHttpSetOption(
                request,
                WINHTTP_OPTION_REDIRECT_POLICY,
                &mut policy as *mut _ as *mut _,
                std::mem::size_of_val(&policy) as u32,
            ) == 0
            {
                let err = GetLastError();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
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
            if WinHttpSendRequest(request, null_mut(), 0, null_mut(), 0, 0, 0) == 0 {
                let err = GetLastError();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                return Err(EngineError::Failed(format!("WinHttpSendRequest {err}")));
            }
            if WinHttpReceiveResponse(request, null_mut()) == 0 {
                let err = GetLastError();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                return Err(EngineError::Failed(format!("WinHttpReceiveResponse {err}")));
            }
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
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                return Err(EngineError::Failed(format!("WinHttpQueryHeaders {err}")));
            }
            let content_range = query_header_string(request, WINHTTP_QUERY_CONTENT_RANGE);
            let location = query_header_string(request, WINHTTP_QUERY_LOCATION);
            let etag = query_header_string(request, WINHTTP_QUERY_ETAG).unwrap_or_default();
            let last_modified =
                query_header_string(request, WINHTTP_QUERY_LAST_MODIFIED).unwrap_or_default();
            let content_length = query_header_string(request, WINHTTP_QUERY_CONTENT_LENGTH)
                .and_then(|value| value.parse::<u64>().ok());
            let accept_ranges =
                query_header_string(request, WINHTTP_QUERY_ACCEPT_RANGES).is_some_and(|value| {
                    value
                        .split(',')
                        .any(|part| part.trim().eq_ignore_ascii_case("bytes"))
                }) || content_range.is_some();
            Ok(super::FetchResult {
                status: status as u16,
                location,
                content_range,
                content_length,
                etag,
                last_modified,
                accept_ranges,
                body: Body::WinHttp(WinHttpBody {
                    session,
                    connect,
                    request,
                    keep_session: true,
                    recycle_connect: true,
                    connect_key,
                }),
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
        assert!(!pool.put("a".into(), 0));
        for index in 1..=CONNECT_POOL_PER_KEY {
            assert!(pool.put("full".into(), index));
        }
        assert!(!pool.put("full".into(), 99));
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
    fn adaptive_connections_respect_single_connection_limit() {
        let controller = AdaptiveConnectionController::new(1);
        assert_eq!(controller.desired(), 1);
        for _ in 0..8 {
            controller.note_success();
            controller.note_congestion();
        }
        assert_eq!(controller.desired(), 1);
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
            },
            dir,
        )
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
    fn pause_control_stops_before_writing() {
        let body: &'static [u8] = b"0123456789";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.control, "pause").unwrap();
        let err = run_job(&job).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_PAUSE);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_control_stops_before_writing() {
        let body: &'static [u8] = b"0123456789";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.control, "cancel").unwrap();
        let err = run_job(&job).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_CANCEL);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finish_job_writes_terminal_progress_code() {
        let body: &'static [u8] = b"0123456789";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.control, "cancel").unwrap();
        let err = finish_job(&job).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_CANCEL);
        let payload: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&job.progress).unwrap()).unwrap();
        assert_eq!(payload["status"], "canceled");
        assert_eq!(payload["code"], EXIT_CANCEL);
        let _ = fs::remove_dir_all(dir);
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
    }

    #[test]
    fn host_header_omits_default_ports() {
        let http = parse_http_url("http://cdn.test/file.bin").unwrap();
        assert_eq!(host_header(&http), "cdn.test");
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
        assert_eq!(err.exit_code(), EXIT_RANGE_UNSUPPORTED);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mismatched_content_range_start_is_range_unsupported() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let url = serve_206_wrong_range(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 3);
        let err = run_job(&job).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_RANGE_UNSUPPORTED);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mismatched_content_range_total_is_range_unsupported() {
        let body: &'static [u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let url = serve_206_wrong_total(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 3);
        let err = run_job(&job).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_RANGE_UNSUPPORTED);
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
    fn chunked_body_is_rejected_instead_of_writing_framing() {
        let body: &'static [u8] = b"hello-chunked";
        let url = serve_chunked(body);
        let (job, dir) = temp_job(&url, true, 0, 1);
        let err = run_job(&job).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_ERROR);
        assert!(err.to_string().contains("chunked"));
        assert!(!job.output.exists());
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
                    let slice = &body[start..=end.min(body.len() - 1)];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len() - 1,
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
                let mut stream = reader.into_inner();
                if attempt < 2 {
                    let _ = stream.write_all(
                        b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
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
        assert_eq!(err.exit_code(), EXIT_ERROR);
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
        let url =
            serve_transient_429_ranges(body, Arc::clone(&attempts), Arc::clone(&payload_bytes));
        let (job, dir) = temp_job(&url, false, body.len() as u64, 4);

        run_job(&job).unwrap();

        assert_eq!(fs::read(&job.output).unwrap(), body);
        assert!(attempts.load(Ordering::SeqCst) >= 6);
        assert_eq!(payload_bytes.load(Ordering::SeqCst), body.len() as u64);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_range_checkpoint_tracks_durable_cursor() {
        let body: &'static [u8] = b"durable-partial-range";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 1);
        let mut file = File::create(&job.output).unwrap();
        file.write_all(&body[..8]).unwrap();
        let completed = Mutex::new(Vec::new());
        persist_range_progress(&job, &mut file, &completed, 0, 8).unwrap();
        assert_eq!(load_completed_ranges(&job), Some(vec![(0, 7)]));
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
        assert_eq!(err.exit_code(), EXIT_RANGE_UNSUPPORTED);
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
        assert!(headers.get("User-Agent").unwrap().contains("Chrome/131"));
        assert_eq!(headers.get("Accept").map(String::as_str), Some("*/*"));
        headers.insert("User-Agent".into(), "CustomAgent/1".into());
        apply_browser_profile(&mut headers);
        assert_eq!(headers.get("User-Agent").unwrap(), "CustomAgent/1");
    }

    #[test]
    fn cloudflare_403_retries_without_cf_bm() {
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
        headers.insert("Cookie".into(), "__cf_bm=stale; session=ok".into());
        let (status, body) = fetch_bytes(&url, &headers, "").unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"ok");
        let seen = seen.lock().unwrap_or_else(|err| err.into_inner());
        assert!(seen.len() >= 2);
        assert!(seen[0].0.contains("__cf_bm"));
        assert!(!seen[1].0.contains("__cf_bm"));
        assert!(seen[1].0.contains("session=ok"));
        assert!(seen[0].1.contains("chrome/131") || seen[0].1.contains("Chrome/131"));
        let stripped = strip_stale_cloudflare_cookies(&headers).unwrap();
        assert_eq!(stripped.get("Cookie").unwrap(), "session=ok");
    }
}
