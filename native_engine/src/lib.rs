//! Compiled HTTP file engine.
//!
//! IDM and AB Download Manager both download ordinary files with a compiled
//! runtime (C++ / JVM) and write Range parts into one payload by seek. This
//! crate is that class of engine: multi-connection `Range` + `seek` into a
//! single `payload.downloading`. HLS/DASH/BT stay in the Python core.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
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

fn agent(job: &Job) -> Result<ureq::Agent, EngineError> {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(30))
        .redirects(16);
    if let Some(ua) = job.headers.get("User-Agent").filter(|value| !value.is_empty()) {
        builder = builder.user_agent(ua);
    }
    if !job.proxy.trim().is_empty() {
        let proxy = ureq::Proxy::new(&job.proxy)
            .map_err(|err| EngineError::Failed(err.to_string()))?;
        builder = builder.proxy(proxy);
    }
    Ok(builder.build())
}

fn apply_headers(mut request: ureq::Request, job: &Job) -> ureq::Request {
    for (key, value) in &job.headers {
        if key.eq_ignore_ascii_case("host")
            || key.eq_ignore_ascii_case("content-length")
            || key.eq_ignore_ascii_case("connection")
            || key.eq_ignore_ascii_case("range")
            || key.eq_ignore_ascii_case("user-agent")
        {
            continue;
        }
        request = request.set(key, value);
    }
    request
}

fn check_control(path: &Path) -> Result<(), EngineError> {
    match read_control(path) {
        Control::Run => Ok(()),
        Control::Pause => Err(EngineError::Pause),
        Control::Cancel => Err(EngineError::Cancel),
    }
}

fn download_sequential(job: &Job) -> Result<(), EngineError> {
    let agent = agent(job)?;
    let resume_from = if job.resume_from > 0 && job.output.exists() {
        job.resume_from.min(job.output.metadata().map(|meta| meta.len()).unwrap_or(0))
    } else {
        0
    };
    let mut request = apply_headers(agent.get(&job.url), job);
    if resume_from > 0 {
        request = request.set("Range", &format!("bytes={resume_from}-"));
    }
    let response = request
        .call()
        .map_err(|err| EngineError::Failed(err.to_string()))?;
    let status = response.status();
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
    let mut reader = response.into_reader();
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
        return Err(EngineError::Failed(format!(
            "downloaded {got} of {total}"
        )));
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
    let Ok(agent) = agent(job) else {
        return;
    };
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
        if let Err(error) = fetch_range(job, &agent, &mut file, start, end, &downloaded) {
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
    agent: &ureq::Agent,
    file: &mut File,
    start: u64,
    end: u64,
    downloaded: &AtomicU64,
) -> Result<(), EngineErrorCode> {
    let mut cursor = start;
    while cursor <= end {
        let request = apply_headers(agent.get(&job.url), job)
            .set("Range", &format!("bytes={cursor}-{end}"));
        let response = request
            .call()
            .map_err(|err| EngineErrorCode::Failed(err.to_string()))?;
        let status = response.status();
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
        let mut reader = response.into_reader();
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
            // CDN truncated this 206; continue the same handle from the next byte.
            continue;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

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
            "hls-native-engine-{}-{}",
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
