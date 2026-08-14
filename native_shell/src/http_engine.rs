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
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_PAUSE: i32 = 20;
pub const EXIT_CANCEL: i32 = 21;
pub const EXIT_RANGE_UNSUPPORTED: i32 = 30;

const WRITE_BATCH: usize = 256 * 1024;

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
    pub control: PathBuf,
    pub progress: PathBuf,
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

pub fn run_job(job: &Job) -> Result<(), EngineError> {
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
    if job.sequential || job.total == 0 {
        download_sequential(job)
    } else {
        download_ranges(job)
    }
}

fn check_control(path: &Path) -> Result<(), EngineError> {
    match read_control(path) {
        Control::Run => Ok(()),
        Control::Pause => Err(EngineError::Pause),
        Control::Cancel => Err(EngineError::Cancel),
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
    let (status, mut reader) = fetch(job, range.as_deref())?;
    if resume_from > 0 && status == 200 {
        return Err(EngineError::RangeUnsupported(
            "server ignored sequential resume Range".into(),
        ));
    }
    if status != 200 && status != 206 {
        return Err(EngineError::Failed(format!("HTTP {status}")));
    }
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
    let chunk = job.chunk_bytes.max(64 * 1024);
    let mut ranges = Vec::new();
    let mut start = 0u64;
    while start < total {
        let end = (start + chunk - 1).min(total - 1);
        ranges.push((start, end));
        start = end + 1;
    }
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
    }
    let workers = job.connections.clamp(1, 64).min(ranges.len());
    let next = Arc::new(Mutex::new(0usize));
    let downloaded = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(Mutex::new(None::<EngineErrorCode>));
    let stop = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..workers {
        let job = job.clone();
        let ranges = ranges.clone();
        let next = Arc::clone(&next);
        let downloaded = Arc::clone(&downloaded);
        let failed = Arc::clone(&failed);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            range_worker(&job, &ranges, next, downloaded, failed, stop);
        }));
    }
    let progress_stop = Arc::clone(&stop);
    let progress_downloaded = Arc::clone(&downloaded);
    let progress_path = job.progress.clone();
    let progress_control = job.control.clone();
    let progress = thread::spawn(move || {
        while !progress_stop.load(Ordering::SeqCst) {
            if matches!(read_control(&progress_control), Control::Pause | Control::Cancel) {
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
    write_progress(&job.progress, total, total, 0.0, "done");
    Ok(())
}

#[derive(Clone)]
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

fn range_worker(
    job: &Job,
    ranges: &[(u64, u64)],
    next: Arc<Mutex<usize>>,
    downloaded: Arc<AtomicU64>,
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
        let index = {
            let mut guard = next.lock().unwrap_or_else(|err| err.into_inner());
            let index = *guard;
            if index >= ranges.len() {
                return;
            }
            *guard += 1;
            index
        };
        let (start, end) = ranges[index];
        if let Err(error) = fetch_range(job, &mut file, start, end, &downloaded) {
            let mut slot = failed.lock().unwrap_or_else(|err| err.into_inner());
            if slot.is_none() {
                *slot = Some(error);
            }
            stop.store(true, Ordering::SeqCst);
            return;
        }
    }
}

fn fetch_range(
    job: &Job,
    file: &mut File,
    start: u64,
    end: u64,
    downloaded: &AtomicU64,
) -> Result<(), EngineErrorCode> {
    let mut cursor = start;
    while cursor <= end {
        let range = format!("bytes={cursor}-{end}");
        let (status, mut reader) =
            fetch(job, Some(&range)).map_err(|err| EngineErrorCode::Failed(err.to_string()))?;
        if status == 200 {
            return Err(EngineErrorCode::RangeUnsupported(
                "server ignored Range and returned 200".into(),
            ));
        }
        if status != 206 {
            return Err(EngineErrorCode::Failed(format!("HTTP {status}")));
        }
        if file.seek(SeekFrom::Start(cursor)).is_err() {
            return Err(EngineErrorCode::Failed("seek failed".into()));
        }
        let mut buffer = vec![0u8; WRITE_BATCH];
        loop {
            match read_control(&job.control) {
                Control::Pause => return Err(EngineErrorCode::Pause),
                Control::Cancel => return Err(EngineErrorCode::Cancel),
                Control::Run => {}
            }
            let count = reader
                .read(&mut buffer)
                .map_err(|err| EngineErrorCode::Failed(err.to_string()))?;
            if count == 0 {
                break;
            }
            let remain = (end + 1).saturating_sub(cursor) as usize;
            let take = count.min(remain);
            if file.write_all(&buffer[..take]).is_err() {
                return Err(EngineErrorCode::Failed("write failed".into()));
            }
            cursor += take as u64;
            downloaded.fetch_add(take as u64, Ordering::SeqCst);
            if cursor > end {
                break;
            }
        }
        if cursor <= end {
            continue;
        }
    }
    Ok(())
}

enum Body {
    Tcp {
        prefix: Vec<u8>,
        at: usize,
        stream: TcpStream,
        remaining: Option<u64>,
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
            #[cfg(windows)]
            Self::WinHttp(body) => body.read(buf),
        }
    }
}

fn fetch(job: &Job, range: Option<&str>) -> Result<(u16, Body), EngineError> {
    let mut url = job.url.clone();
    for _ in 0..16 {
        let parsed = parse_http_url(&url)?;
        let use_winhttp = parsed.https || !job.proxy.trim().is_empty();
        if use_winhttp {
            #[cfg(windows)]
            {
                return winhttp::get(job, &url, range);
            }
            #[cfg(not(windows))]
            {
                return Err(EngineError::Failed("https/proxy needs WinHTTP".into()));
            }
        }
        let (status, location, body) = http_get(job, &parsed, range)?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let next = location.ok_or_else(|| EngineError::Failed("redirect without Location".into()))?;
            url = resolve_location(&parsed, &next);
            continue;
        }
        return Ok((status, body));
    }
    Err(EngineError::Failed("too many redirects".into()))
}

struct ParsedUrl {
    https: bool,
    host: String,
    port: u16,
    path: String,
}

fn parse_http_url(raw: &str) -> Result<ParsedUrl, EngineError> {
    let (https, rest) = if let Some(rest) = raw.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = raw.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(EngineError::Failed("url must be http(s)".into()));
    };
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
    if host.is_empty() {
        return Err(EngineError::Failed("url host missing".into()));
    }
    Ok(ParsedUrl {
        https,
        host,
        port,
        path: format!("/{path}"),
    })
}

fn resolve_location(current: &ParsedUrl, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    let scheme = if current.https { "https" } else { "http" };
    if location.starts_with('/') {
        format!("{scheme}://{}:{}{location}", current.host, current.port)
    } else {
        format!(
            "{scheme}://{}:{}{}",
            current.host, current.port, location
        )
    }
}

fn http_get(
    job: &Job,
    parsed: &ParsedUrl,
    range: Option<&str>,
) -> Result<(u16, Option<String>, Body), EngineError> {
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
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept-Encoding: identity\r\n",
        parsed.path, parsed.host, parsed.port
    );
    if let Some(ua) = job
        .headers
        .get("User-Agent")
        .filter(|value| !value.is_empty())
    {
        header.push_str(&format!("User-Agent: {ua}\r\n"));
    }
    if let Some(range) = range {
        header.push_str(&format!("Range: {range}\r\n"));
    }
    for (key, value) in &job.headers {
        if key.eq_ignore_ascii_case("host")
            || key.eq_ignore_ascii_case("content-length")
            || key.eq_ignore_ascii_case("connection")
            || key.eq_ignore_ascii_case("range")
            || key.eq_ignore_ascii_case("user-agent")
        {
            continue;
        }
        header.push_str(&format!("{key}: {value}\r\n"));
    }
    header.push_str("\r\n");
    stream
        .write_all(header.as_bytes())
        .map_err(|err| EngineError::Failed(err.to_string()))?;
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
    let mut location = None;
    let mut content_length = None;
    for line in head.lines().skip(1) {
        if let Some(value) = line.strip_prefix("Location:").or_else(|| line.strip_prefix("location:"))
        {
            location = Some(value.trim().to_string());
        }
        if let Some(value) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = value.trim().parse::<u64>().ok();
        }
    }
    let remaining = content_length.map(|total| total.saturating_sub(leftover.len() as u64));
    Ok((
        status,
        location,
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
        WinHttpSendRequest, WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_NAMED_PROXY,
        WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    };

    pub struct WinHttpBody {
        session: *mut core::ffi::c_void,
        connect: *mut core::ffi::c_void,
        request: *mut core::ffi::c_void,
    }

    unsafe impl Send for WinHttpBody {}

    impl Drop for WinHttpBody {
        fn drop(&mut self) {
            unsafe {
                if !self.request.is_null() {
                    WinHttpCloseHandle(self.request);
                }
                if !self.connect.is_null() {
                    WinHttpCloseHandle(self.connect);
                }
                if !self.session.is_null() {
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

    pub fn get(job: &Job, url: &str, range: Option<&str>) -> Result<(u16, Body), EngineError> {
        let parsed = super::parse_http_url(url)?;
        unsafe {
            let proxy = job.proxy.trim();
            let session = if proxy.is_empty() {
                WinHttpOpen(
                    wide("HLS Downloader").as_ptr(),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    null_mut(),
                    null_mut(),
                    0,
                )
            } else {
                WinHttpOpen(
                    wide("HLS Downloader").as_ptr(),
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
            let connect = WinHttpConnect(session, wide(&parsed.host).as_ptr(), parsed.port, 0);
            if connect.is_null() {
                WinHttpCloseHandle(session);
                return Err(EngineError::Failed(format!(
                    "WinHttpConnect {}",
                    GetLastError()
                )));
            }
            let flags = if parsed.https { WINHTTP_FLAG_SECURE } else { 0 };
            let request = WinHttpOpenRequest(
                connect,
                wide("GET").as_ptr(),
                wide(&parsed.path).as_ptr(),
                null_mut(),
                null_mut(),
                null_mut(),
                flags,
            );
            if request.is_null() {
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err(EngineError::Failed(format!(
                    "WinHttpOpenRequest {}",
                    GetLastError()
                )));
            }
            let mut extra = String::from("Accept-Encoding: identity\r\n");
            if let Some(range) = range {
                extra.push_str(&format!("Range: {range}\r\n"));
            }
            for (key, value) in &job.headers {
                if key.eq_ignore_ascii_case("host")
                    || key.eq_ignore_ascii_case("content-length")
                    || key.eq_ignore_ascii_case("connection")
                    || key.eq_ignore_ascii_case("range")
                {
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
                WinHttpCloseHandle(session);
                return Err(EngineError::Failed(format!("WinHttpSendRequest {err}")));
            }
            if WinHttpReceiveResponse(request, null_mut()) == 0 {
                let err = GetLastError();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
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
                WinHttpCloseHandle(session);
                return Err(EngineError::Failed(format!("WinHttpQueryHeaders {err}")));
            }
            Ok((
                status as u16,
                Body::WinHttp(WinHttpBody {
                    session,
                    connect,
                    request,
                }),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

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

    fn temp_job(url: &str, sequential: bool, total: u64, connections: usize) -> (Job, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "hls-http-engine-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
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
                control,
                progress,
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
    fn cancel_control_stops_before_writing() {
        let body: &'static [u8] = b"0123456789";
        let url = serve_body(body);
        let (job, dir) = temp_job(&url, false, body.len() as u64, 2);
        fs::write(&job.control, "cancel").unwrap();
        let err = run_job(&job).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_CANCEL);
        let _ = fs::remove_dir_all(dir);
    }
}
