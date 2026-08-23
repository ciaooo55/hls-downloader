//! Local Range media server for play-while-downloading and completed files.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

static PORT: AtomicU16 = AtomicU16::new(0);

const STREAM_CHUNK: usize = 64 * 1024;

#[derive(Clone)]
pub struct MediaServer {
    inner: Arc<Mutex<MediaState>>,
    lan: Arc<AtomicBool>,
    port: u16,
}

impl Default for MediaServer {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MediaState::default())),
            lan: Arc::new(AtomicBool::new(false)),
            port: 0,
        }
    }
}

#[derive(Default)]
struct MediaState {
    mounts: Vec<(String, Mount)>,
}

#[derive(Clone)]
enum Mount {
    File(PathBuf),
    Dir(PathBuf),
    Remote(String),
}

impl MediaServer {
    pub fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let addr = listener.local_addr().map_err(|error| error.to_string())?;
        PORT.store(addr.port(), Ordering::SeqCst);
        let server = Self {
            inner: Arc::new(Mutex::new(MediaState::default())),
            lan: Arc::new(AtomicBool::new(false)),
            port: addr.port(),
        };
        let state = Arc::clone(&server.inner);
        let lan = Arc::clone(&server.lan);
        let listen_port = addr.port();
        thread::spawn(move || loop {
            match listener.accept() {
                Ok((stream, peer)) => {
                    if !peer_allowed(peer.ip(), lan.load(Ordering::SeqCst)) {
                        continue;
                    }
                    let mounts = state
                        .lock()
                        .map(|guard| guard.mounts.clone())
                        .unwrap_or_default();
                    thread::spawn(move || {
                        let _ = handle_client(stream, &mounts, listen_port);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(_) => break,
            }
        });
        Ok(server)
    }

    pub fn mount(&self, token: &str, path: PathBuf) {
        self.store(token, Mount::File(path));
    }

    pub fn mount_dir(&self, token: &str, path: PathBuf) {
        self.store(token, Mount::Dir(path));
    }

    pub fn mount_remote(&self, token: &str, url: String) {
        self.store(token, Mount::Remote(url));
    }

    pub fn unmount(&self, token: &str) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            return false;
        };
        let before = state.mounts.len();
        state.mounts.retain(|(name, _)| name != token);
        state.mounts.len() != before
    }

    pub fn enable_lan(&self) {
        self.lan.store(true, Ordering::SeqCst);
    }

    pub fn lan_enabled(&self) -> bool {
        self.lan.load(Ordering::SeqCst)
    }

    pub fn url_for(&self, token: &str) -> String {
        format!("http://127.0.0.1:{}/media/{token}", self.port)
    }

    pub fn bound_port(&self) -> u16 {
        self.port
    }

    pub fn port() -> u16 {
        PORT.load(Ordering::SeqCst)
    }

    fn store(&self, token: &str, mount: Mount) {
        if token.trim().is_empty() {
            return;
        }
        if let Ok(mut state) = self.inner.lock() {
            state.mounts.retain(|(name, _)| name != token);
            if state.mounts.len() >= 64 {
                state.mounts.remove(0);
            }
            state.mounts.push((token.to_string(), mount));
        }
    }
}

pub fn random_mount_token() -> String {
    let mut bytes = [0u8; 16];
    fill_random_bytes(&mut bytes);
    let mut out = String::from("m");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn fill_random_bytes(buf: &mut [u8]) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status == 0 {
            return;
        }
    }
    let tick = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|item| item.as_nanos() as u64)
        .unwrap_or(0);
    static COUNTER: AtomicU64 = AtomicU64::new(0x9e3779b97f4a7c15);
    let mut seed =
        COUNTER.fetch_add(tick | 1, Ordering::Relaxed) ^ tick ^ (std::process::id() as u64);
    for byte in buf {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *byte = (seed >> 24) as u8;
        seed = seed.wrapping_add(0x9e3779b97f4a7c15);
    }
}

fn peer_allowed(ip: IpAddr, lan: bool) -> bool {
    match ip {
        IpAddr::V4(addr) if addr.is_loopback() => true,
        IpAddr::V6(addr) if addr.is_loopback() => true,
        IpAddr::V4(addr) if lan && (addr.is_private() || addr.is_link_local()) => true,
        IpAddr::V6(addr)
            if lan
                && addr
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| mapped.is_private()) =>
        {
            true
        }
        _ => false,
    }
}

fn handle_client(
    mut stream: TcpStream,
    mounts: &[(String, Mount)],
    port: u16,
) -> Result<(), String> {
    let peer = stream.peer_addr().ok().map(|addr| addr.ip());
    let mut buf = [0u8; 4096];
    let count = stream.read(&mut buf).map_err(|error| error.to_string())?;
    let request = String::from_utf8_lossy(&buf[..count]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    if let Some(token) = path.strip_prefix("/tvbox/") {
        let token = token.split('/').next().unwrap_or(token);
        let Some(mount) = resolve_mount(mounts, token) else {
            return write_status(&mut stream, 404, b"not found");
        };
        let advertised = advertise_host(peer, port);
        let location = match mount {
            Mount::Remote(url) => {
                if !media_redirect_allowed(url) {
                    return write_status(&mut stream, 404, b"not found");
                }
                url.clone()
            }
            _ => format!("http://{advertised}/media/{token}"),
        };
        if !media_redirect_allowed(&location) {
            return write_status(&mut stream, 404, b"not found");
        }
        let body = format!(
            "{{\"url\":\"{}\",\"title\":\"{}\"}}",
            json_escape(&location),
            json_escape(&token)
        );
        return write_response(
            &mut stream,
            200,
            "application/json; charset=utf-8",
            body.as_bytes(),
        );
    }
    let rest = path.strip_prefix("/media/").unwrap_or("");
    let (token, sub) = rest.split_once('/').unwrap_or((rest, ""));
    let Some(mount) = resolve_mount(mounts, token) else {
        write_status(&mut stream, 404, b"not found")?;
        return Ok(());
    };
    let file_path = match mount {
        Mount::Remote(url) => {
            return write_redirect(&mut stream, url);
        }
        Mount::File(path) => {
            if !sub.is_empty() {
                write_status(&mut stream, 404, b"not found")?;
                return Ok(());
            }
            path.clone()
        }
        Mount::Dir(dir) => {
            let name = if sub.is_empty() { "local.m3u8" } else { sub };
            match safe_join(dir, name) {
                Some(path) if path.exists() => path,
                _ => {
                    write_status(&mut stream, 404, b"not found")?;
                    return Ok(());
                }
            }
        }
    };
    serve_file(&mut stream, &request, &file_path)
}

fn advertise_host(peer: Option<IpAddr>, port: u16) -> String {
    match peer {
        Some(IpAddr::V4(ip)) if ip.is_loopback() => format!("127.0.0.1:{port}"),
        Some(IpAddr::V6(ip)) if ip.is_loopback() => format!("127.0.0.1:{port}"),
        Some(_) => crate::cast::primary_lan_ipv4()
            .map(|ip| format!("{ip}:{port}"))
            .unwrap_or_else(|| format!("127.0.0.1:{port}")),
        None => format!("127.0.0.1:{port}"),
    }
}

fn write_redirect(stream: &mut TcpStream, location: &str) -> Result<(), String> {
    if !media_redirect_allowed(location) {
        return write_status(stream, 404, b"not found");
    }
    let header = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(header.as_bytes())
        .map_err(|error| error.to_string())
}

fn media_redirect_allowed(location: &str) -> bool {
    let trimmed = location.trim().trim_start_matches('\u{feff}');
    if trimmed.chars().any(|ch| ch.is_control()) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "")
}

fn resolve_mount<'a>(mounts: &'a [(String, Mount)], token: &str) -> Option<&'a Mount> {
    mounts
        .iter()
        .find(|(name, _)| name == token)
        .map(|(_, mount)| mount)
}

fn safe_join(dir: &Path, sub: &str) -> Option<PathBuf> {
    if sub.is_empty()
        || sub
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(sub).is_absolute()
    {
        return None;
    }
    Some(dir.join(sub))
}

fn serve_file(stream: &mut TcpStream, request: &str, file_path: &Path) -> Result<(), String> {
    let mut file = File::open(file_path).map_err(|error| error.to_string())?;
    let total = file.metadata().map_err(|error| error.to_string())?.len();
    let content_type = content_type(file_path);
    let range = request
        .lines()
        .find_map(|line| line.strip_prefix("Range: bytes="));
    if let Some(range) = range {
        let Some((start, end)) = parse_range(range, total) else {
            return write_status(stream, 416, b"range not satisfiable");
        };
        let length = end.saturating_sub(start).saturating_add(1);
        let header = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {length}\r\nAccept-Ranges: bytes\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(header.as_bytes())
            .map_err(|error| error.to_string())?;
        stream_bytes(stream, &mut file, start, length)
    } else {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(header.as_bytes())
            .map_err(|error| error.to_string())?;
        stream_bytes(stream, &mut file, 0, total)
    }
}

fn stream_bytes(
    stream: &mut TcpStream,
    file: &mut File,
    start: u64,
    length: u64,
) -> Result<(), String> {
    file.seek(SeekFrom::Start(start))
        .map_err(|error| error.to_string())?;
    let mut remaining = length;
    let mut buf = vec![0u8; STREAM_CHUNK];
    while remaining > 0 {
        let want = buf.len().min(remaining as usize);
        let read = file
            .read(&mut buf[..want])
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        stream
            .write_all(&buf[..read])
            .map_err(|error| error.to_string())?;
        remaining -= read as u64;
    }
    Ok(())
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "m3u8" => "application/vnd.apple.mpegurl",
        "mpd" => "application/dash+xml",
        "mp4" | "m4s" | "m4a" | "m4v" => "video/mp4",
        "ts" => "video/mp2t",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = header.trim().split('/').next().unwrap_or(header);
    let (start, end) = spec.split_once('-')?;
    let last = total - 1;
    let (start, end) = if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        (total.saturating_sub(suffix.min(total)), last)
    } else if end.is_empty() {
        (start.parse().ok()?, last)
    } else {
        (start.parse().ok()?, end.parse::<u64>().ok()?.min(last))
    };
    (start <= end && start <= last).then_some((start, end))
}

fn write_status(stream: &mut TcpStream, status: u16, body: &[u8]) -> Result<(), String> {
    write_response(stream, status, "text/plain; charset=utf-8", body)
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .map_err(|error| error.to_string())?;
    stream.write_all(body).map_err(|error| error.to_string())
}

pub fn playlist_url(task_dir: &Path) -> Option<PathBuf> {
    let playlist = task_dir.join("local.m3u8");
    playlist.exists().then_some(playlist)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn serves_range_from_local_file() {
        let server = MediaServer::start().unwrap();
        let dir = std::env::temp_dir().join(format!("hls-play-{}", std::process::id()));
        fs_create(&dir);
        let file = dir.join("a.bin");
        std::fs::write(&file, b"0123456789").unwrap();
        server.mount("task-1", file);
        let mut stream = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        stream
            .write_all(b"GET /media/task-1 HTTP/1.1\r\nRange: bytes=2-5\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("206"));
        assert!(text.contains("2345"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unmount_revokes_media_url() {
        let server = MediaServer::start().unwrap();
        let dir = std::env::temp_dir().join(format!("hls-play-unmount-{}", std::process::id()));
        fs_create(&dir);
        let file = dir.join("a.bin");
        std::fs::write(&file, b"0123456789").unwrap();
        server.mount("temporary", file);
        assert!(server.unmount("temporary"));
        assert!(!server.unmount("temporary"));
        let mut stream = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        stream
            .write_all(b"GET /media/temporary HTTP/1.1\r\nRange: bytes=0-1\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 404"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unsatisfiable_range_is_416() {
        let server = MediaServer::start().unwrap();
        let dir = std::env::temp_dir().join(format!("hls-play-416-{}", std::process::id()));
        fs_create(&dir);
        let file = dir.join("a.bin");
        std::fs::write(&file, b"0123456789").unwrap();
        server.mount("task-1", file);
        let mut stream = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        stream
            .write_all(b"GET /media/task-1 HTTP/1.1\r\nRange: bytes=99-200\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("416"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn serves_playlist_siblings_and_rejects_traversal() {
        let server = MediaServer::start().unwrap();
        let dir = std::env::temp_dir().join(format!("hls-play-dir-{}", std::process::id()));
        fs_create(&dir);
        std::fs::write(dir.join("local.m3u8"), b"#EXTM3U\nseg-0000.m4s\n").unwrap();
        std::fs::write(dir.join("seg-0000.m4s"), b"frag").unwrap();
        server.mount_dir("dash-1", dir.clone());
        let mut playlist = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        playlist
            .write_all(b"GET /media/dash-1/local.m3u8 HTTP/1.1\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        playlist.read_to_end(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("#EXTM3U"));
        let mut traversal = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        traversal
            .write_all(b"GET /media/dash-1/../secret HTTP/1.1\r\n\r\n")
            .unwrap();
        buf.clear();
        traversal.read_to_end(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("404"));
        let mut tvbox = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        tvbox
            .write_all(b"GET /tvbox/dash-1 HTTP/1.1\r\nHost: evil.example\r\n\r\n")
            .unwrap();
        buf.clear();
        tvbox.read_to_end(&mut buf).unwrap();
        let tvbox_body = String::from_utf8_lossy(&buf);
        assert!(tvbox_body.contains("\"url\""));
        assert!(!tvbox_body.contains("evil.example"));
        assert!(tvbox_body.contains("127.0.0.1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mount_tokens_are_not_sequential_task_ids() {
        let first = random_mount_token();
        let second = random_mount_token();
        assert_ne!(first, second);
        assert!(first.starts_with('m'));
        assert!(!first.starts_with("task-"));
        assert_eq!(first.len(), 33);
    }

    #[test]
    fn media_redirects_reject_header_injection() {
        assert!(media_redirect_allowed("http://127.0.0.1:9/media/m1"));
        assert!(media_redirect_allowed("HTTP://127.0.0.1:9/media/m1"));
        assert!(!media_redirect_allowed(
            "http://evil.example/\r\nLocation: http://x"
        ));
        assert!(!media_redirect_allowed("javascript:alert(1)"));
        assert!(!media_redirect_allowed("\u{feff}javascript:alert(1)"));
        assert!(!media_redirect_allowed("https://x/\0y"));
        assert_eq!(json_escape("http://x\"y"), "http://x\\\"y");
    }

    fn fs_create(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
    }
}
