//! FTP / FTPS single-stream download with SIZE+REST resume.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

enum Conn {
    Plain(TcpStream),
    #[cfg(windows)]
    Tls(schannel::tls_stream::TlsStream<TcpStream>),
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf),
            #[cfg(windows)]
            Self::Tls(stream) => stream.read(buf),
        }
    }
}

impl Conn {
    fn peer_addr(&self) -> Result<std::net::SocketAddr, String> {
        match self {
            Self::Plain(stream) => stream.peer_addr().map_err(|error| error.to_string()),
            #[cfg(windows)]
            Self::Tls(stream) => stream
                .get_ref()
                .peer_addr()
                .map_err(|error| error.to_string()),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buf),
            #[cfg(windows)]
            Self::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            #[cfg(windows)]
            Self::Tls(stream) => stream.flush(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FtpTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub path: String,
    pub tls: bool,
}

pub fn parse_ftp_url(url: &str) -> Result<FtpTarget, String> {
    let lower = url.to_ascii_lowercase();
    let tls = lower.starts_with("ftps://");
    if !tls && !lower.starts_with("ftp://") {
        return Err("not an FTP URL".into());
    }
    let rest = url.split("://").nth(1).unwrap_or("");
    let (auth, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (userinfo, hostport) = if auth.contains('@') {
        auth.rsplit_once('@').unwrap()
    } else {
        ("anonymous:", auth)
    };
    let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let user = percent_decode(user);
    let password = percent_decode(password);
    let (host, port) = if let Some((host, port)) = hostport.rsplit_once(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| "FTP port invalid".to_string())?;
        if port == 0 {
            return Err("FTP port invalid".into());
        }
        (host, port)
    } else {
        (hostport, if tls { 990 } else { 21 })
    };
    if host.is_empty() || !ftp_wire_ok(host) {
        return Err("FTP host missing".into());
    }
    let user = if user.is_empty() {
        "anonymous".to_string()
    } else {
        user
    };
    let path = format!("/{}", percent_decode(path.trim_start_matches('/')));
    if !ftp_wire_ok(&user) || !ftp_wire_ok(&password) || !ftp_wire_ok(&path) {
        return Err("FTP 地址不能包含控制字符".into());
    }
    Ok(FtpTarget {
        host: host.to_string(),
        port,
        user,
        password,
        path,
        tls,
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(decoded) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(decoded);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn ftp_wire_ok(value: &str) -> bool {
    !value.chars().any(|ch| matches!(ch, '\r' | '\n' | '\0'))
}

pub fn download_ftp(url: &str, output: &Path, control: &Path, resume: bool) -> Result<u64, String> {
    let target = parse_ftp_url(url)?;
    let progress = output.with_extension("progress.json");
    let implicit = target.tls && target.port == 990;
    let raw = TcpStream::connect((target.host.as_str(), target.port))
        .map_err(|error| error.to_string())?;
    raw.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| error.to_string())?;
    let mut ctrl = if implicit {
        wrap_tls(raw, &target.host)?
    } else {
        Conn::Plain(raw)
    };
    let reply = read_reply(&mut ctrl)?;
    require_reply_code(&reply, &[220], "server greeting")?;
    if target.tls && !implicit {
        let reply = command(&mut ctrl, "AUTH TLS")?;
        require_reply_code(&reply, &[234], "AUTH TLS")?;
        ctrl = wrap_tls(take_plain(ctrl)?, &target.host)?;
        let reply = command(&mut ctrl, "PBSZ 0")?;
        require_reply_code(&reply, &[200], "PBSZ")?;
        let reply = command(&mut ctrl, "PROT P")?;
        require_reply_code(&reply, &[200], "PROT")?;
    }
    let reply = command(&mut ctrl, &format!("USER {}", target.user))?;
    match reply_code(&reply) {
        Some(230) => {}
        Some(331) => {
            let reply = command(&mut ctrl, &format!("PASS {}", target.password))?;
            require_reply_code(&reply, &[230], "PASS")?;
        }
        _ => require_reply_code(&reply, &[230, 331], "USER")?,
    }
    let reply = command(&mut ctrl, "TYPE I")?;
    require_reply_code(&reply, &[200], "TYPE I")?;
    let size_reply = command(&mut ctrl, &format!("SIZE {}", target.path))?;
    let size = if reply_code(&size_reply) == Some(213) {
        size_reply
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    } else {
        0
    };
    let resume_from = if resume && output.exists() {
        std::fs::metadata(output)
            .map(|meta| meta.len())
            .unwrap_or(0)
    } else {
        0
    };
    if resume_from > 0 {
        let reply = command(&mut ctrl, &format!("REST {resume_from}"))?;
        require_reply_code(&reply, &[350], "REST")?;
    }
    let pasv = command(&mut ctrl, "PASV")?;
    require_reply_code(&pasv, &[227], "PASV")?;
    let data_port = parse_pasv_port(&pasv)?;
    let data_addr = std::net::SocketAddr::new(ctrl.peer_addr()?.ip(), data_port);
    let data_raw = TcpStream::connect(data_addr).map_err(|error| error.to_string())?;
    // Keep a raw duplicate of the socket outside Schannel so pause/cancel can
    // interrupt a blocked FTP or FTPS data read without relying on short TLS
    // read timeouts or transport-specific timeout semantics.
    let data_abort = data_raw.try_clone().map_err(|error| error.to_string())?;
    let mut data = if target.tls {
        wrap_tls(data_raw, &target.host)?
    } else {
        Conn::Plain(data_raw)
    };
    let reply = command(&mut ctrl, &format!("RETR {}", target.path))?;
    require_reply_code(&reply, &[125, 150], "RETR")?;
    let mut file = if resume_from > 0 {
        std::fs::OpenOptions::new()
            .append(true)
            .open(output)
            .map_err(|error| error.to_string())?
    } else {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::File::create(output).map_err(|error| error.to_string())?
    };
    let stop_reader = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_reader_worker = std::sync::Arc::clone(&stop_reader);
    let progress_for_reader = progress.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let data_reader = std::thread::spawn(move || {
        let result = (|| -> Result<u64, String> {
            let mut buf = [0u8; 64 * 1024];
            let mut downloaded = resume_from;
            crate::http_engine::write_progress(
                &progress_for_reader,
                downloaded,
                size,
                0.0,
                "downloading",
            );
            loop {
                let count = data.read(&mut buf).map_err(|error| error.to_string())?;
                if stop_reader_worker.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("stopped".into());
                }
                if count == 0 {
                    break;
                }
                file.write_all(&buf[..count])
                    .map_err(|error| error.to_string())?;
                downloaded += count as u64;
                crate::http_engine::write_progress(
                    &progress_for_reader,
                    downloaded,
                    size,
                    0.0,
                    "downloading",
                );
                crate::net_policy::consume(count);
            }
            file.flush().map_err(|error| error.to_string())?;
            Ok(downloaded)
        })();
        let _ = result_tx.send(result);
    });

    let downloaded = loop {
        let flag = std::fs::read_to_string(control).unwrap_or_else(|_| "run".into());
        let action = flag.trim();
        if matches!(action, "pause" | "cancel") {
            stop_reader.store(true, std::sync::atomic::Ordering::Release);
            let _ = data_abort.shutdown(std::net::Shutdown::Both);
            // A normal TCP shutdown wakes both plain recv and Schannel's
            // underlying recv quickly. Bound the cleanup wait so task control
            // never regresses to the old unbounded data-channel stall.
            match result_rx.recv_timeout(Duration::from_millis(750)) {
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = data_reader.join();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Dropping JoinHandle detaches only as a last resort. The
                    // stop flag prevents any later read from being written.
                }
            }
            return Err(if action == "pause" {
                "paused"
            } else {
                "canceled"
            }
            .into());
        }
        match result_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => {
                let _ = data_reader.join();
                break result?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = data_reader.join();
                return Err("FTP data reader stopped unexpectedly".into());
            }
        }
    };
    let reply = read_reply(&mut ctrl)?;
    require_reply_code(&reply, &[226, 250], "transfer completion")?;
    if size > 0 && downloaded != size {
        return Err(format!(
            "FTP transfer size mismatch: expected {size}, received {downloaded}"
        ));
    }
    Ok(downloaded)
}

fn wrap_tls(stream: TcpStream, host: &str) -> Result<Conn, String> {
    #[cfg(windows)]
    {
        let cred = schannel::schannel_cred::SchannelCred::builder()
            .acquire(schannel::schannel_cred::Direction::Outbound)
            .map_err(|error| error.to_string())?;
        let tls = schannel::tls_stream::Builder::new()
            .domain(host)
            .connect(cred, stream)
            .map_err(|error| error.to_string())?;
        Ok(Conn::Tls(tls))
    }
    #[cfg(not(windows))]
    {
        let _ = (stream, host);
        Err("FTPS uses Windows Schannel in v6".into())
    }
}

fn take_plain(conn: Conn) -> Result<TcpStream, String> {
    match conn {
        Conn::Plain(stream) => Ok(stream),
        #[cfg(windows)]
        Conn::Tls(_) => Err("control socket already TLS".into()),
    }
}

fn command(stream: &mut Conn, line: &str) -> Result<String, String> {
    stream
        .write_all(format!("{line}\r\n").as_bytes())
        .map_err(|error| error.to_string())?;
    read_reply(stream)
}

fn reply_code(reply: &str) -> Option<u16> {
    let bytes = reply.as_bytes().get(..3)?;
    if !bytes.iter().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn require_reply_code(reply: &str, allowed: &[u16], action: &str) -> Result<(), String> {
    if reply_code(reply).is_some_and(|code| allowed.contains(&code)) {
        return Ok(());
    }
    let summary = reply.trim().chars().take(200).collect::<String>();
    let summary = if summary.is_empty() {
        "empty reply".to_string()
    } else {
        summary
    };
    Err(format!("FTP {action} rejected: {summary}"))
}

fn reply_is_complete(reply: &[u8]) -> Result<bool, String> {
    if !reply.ends_with(b"\r\n") {
        return Ok(false);
    }
    let first_end = reply
        .windows(2)
        .position(|pair| pair == b"\r\n")
        .ok_or_else(|| "FTP reply is missing a complete first line".to_string())?;
    let first = &reply[..first_end];
    if first.len() < 3 || !first[..3].iter().all(|byte| byte.is_ascii_digit()) {
        return Err("FTP reply is missing a valid status code".into());
    }
    if first.len() == 3 {
        return Ok(true);
    }
    match first[3] {
        b' ' => Ok(true),
        b'-' => {
            let code = &first[..3];
            let body = &reply[..reply.len() - 2];
            let last_start = body
                .windows(2)
                .rposition(|pair| pair == b"\r\n")
                .map(|index| index + 2)
                .unwrap_or(0);
            let last = &body[last_start..];
            Ok(last.len() >= 3
                && &last[..3] == code
                && (last.len() == 3 || last.get(3) == Some(&b' ')))
        }
        _ => Err("FTP reply status separator is invalid".into()),
    }
}

fn read_reply(stream: &mut Conn) -> Result<String, String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while buf.len() < 8192 {
        if stream.read(&mut byte).map_err(|error| error.to_string())? == 0 {
            return Err("FTP control connection closed before a complete reply".into());
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n") && reply_is_complete(&buf)? {
            return String::from_utf8(buf).map_err(|error| error.to_string());
        }
    }
    Err("FTP reply exceeds the 8192-byte safety limit".into())
}

fn parse_pasv_port(reply: &str) -> Result<u16, String> {
    let start = reply
        .find('(')
        .ok_or_else(|| "PASV missing host".to_string())?
        + 1;
    let end = reply
        .find(')')
        .ok_or_else(|| "PASV missing host".to_string())?;
    let nums: Vec<u16> = reply[start..end]
        .split(',')
        .filter_map(|item| item.trim().parse().ok())
        .collect();
    if nums.len() < 6 {
        return Err("PASV address invalid".into());
    }
    let port = nums[4].saturating_mul(256).saturating_add(nums[5]);
    if port == 0 {
        return Err("PASV port invalid".into());
    }
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_userinfo_and_ftps_port() {
        let target = parse_ftp_url("ftps://alice:secret@files.example/pub/a.bin").unwrap();
        assert_eq!(target.host, "files.example");
        assert_eq!(target.port, 990);
        assert_eq!(target.user, "alice");
        assert_eq!(target.path, "/pub/a.bin");
        assert!(target.tls);
        let explicit = parse_ftp_url("ftps://files.example:21/a.bin").unwrap();
        assert_eq!(explicit.port, 21);
        assert!(explicit.tls);
        assert!(parse_ftp_url("ftp://files.example:notaport/a.bin").is_err());
        assert!(parse_ftp_url("ftp://files.example:0/a.bin").is_err());
        assert!(parse_ftp_url("ftp://files.example/a.bin\r\nSITE EXEC x").is_err());
        assert!(parse_ftp_url("ftp://alice\r\nPASS x@files.example/a.bin").is_err());
        let at = parse_ftp_url("ftp://alice:p@ss@files.example:2121/pub/a.bin").unwrap();
        assert_eq!(at.user, "alice");
        assert_eq!(at.password, "p@ss");
        assert_eq!(at.host, "files.example");
        assert_eq!(at.port, 2121);
        let encoded =
            parse_ftp_url("ftp://alice%40team:p%3Ass@files.example/%E6%B5%8B%E8%AF%95/a%20b.bin")
                .unwrap();
        assert_eq!(encoded.user, "alice@team");
        assert_eq!(encoded.password, "p:ss");
        assert_eq!(encoded.path, "/测试/a b.bin");
        assert!(parse_ftp_url("ftp://alice%0d%0aPASS%20x@files.example/a.bin").is_err());
        assert!(parse_ftp_url("ftp://files.example/a%0d%0aSITE%20EXEC%20x").is_err());
    }

    #[test]
    fn pasv_uses_only_the_port_and_rejects_zero() {
        assert_eq!(
            parse_pasv_port("227 Entering Passive Mode (10,0,0,1,20,80)").unwrap(),
            20 * 256 + 80
        );
        assert_eq!(
            parse_pasv_port("227 Entering Passive Mode (127,0,0,1,4,1)").unwrap(),
            1025
        );
        assert!(parse_pasv_port("227 Entering Passive Mode (169,254,169,254,0,0)").is_err());
        assert!(parse_pasv_port("227 no-parens").is_err());
    }

    #[test]
    fn recognizes_single_and_multiline_replies() {
        assert_eq!(reply_code("350 Restarting at 1024\r\n"), Some(350));
        assert!(reply_is_complete(b"220 Ready\r\n").unwrap());
        assert!(!reply_is_complete(b"220-Welcome\r\nfeature line\r\n").unwrap());
        assert!(reply_is_complete(b"220-Welcome\r\nfeature line\r\n220 Ready\r\n").unwrap());
        assert!(reply_is_complete(b"broken\r\n").is_err());
    }

    #[test]
    fn transfer_reply_checks_fail_closed() {
        assert!(require_reply_code("350 Restarting\r\n", &[350], "REST").is_ok());
        assert!(require_reply_code("500 REST unsupported\r\n", &[350], "REST").is_err());
        assert!(require_reply_code("150 Opening data\r\n", &[125, 150], "RETR").is_ok());
        assert!(require_reply_code(
            "226 Transfer complete\r\n",
            &[226, 250],
            "transfer completion"
        )
        .is_ok());
        assert!(require_reply_code(
            "426 Transfer aborted\r\n",
            &[226, 250],
            "transfer completion"
        )
        .is_err());
    }

    #[test]
    fn session_reply_checks_fail_closed() {
        assert!(require_reply_code("220 Ready\r\n", &[220], "server greeting").is_ok());
        assert!(require_reply_code("120 Try later\r\n", &[220], "server greeting").is_err());
        assert!(require_reply_code("234 AUTH TLS ok\r\n", &[234], "AUTH TLS").is_ok());
        assert!(require_reply_code("534 TLS unavailable\r\n", &[234], "AUTH TLS").is_err());
        assert!(require_reply_code("200 Type set\r\n", &[200], "TYPE I").is_ok());
        assert!(require_reply_code("500 TYPE rejected\r\n", &[200], "TYPE I").is_err());
        assert!(require_reply_code(
            "227 Entering Passive Mode (1,2,3,4,5,6)\r\n",
            &[227],
            "PASV"
        )
        .is_ok());
        assert!(require_reply_code("425 No data connection\r\n", &[227], "PASV").is_err());
    }

    fn spawn_stalled_ftp_server() -> (
        u16,
        std::sync::mpsc::Receiver<()>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (stalled_tx, stalled_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut ctrl, _) = listener.accept().unwrap();
            ctrl.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            ctrl.write_all(b"220 Ready\r\n").unwrap();
            let mut reader = std::io::BufReader::new(ctrl.try_clone().unwrap());
            let mut data_listener = None;
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                    break;
                }
                let command = line.trim_end().to_ascii_uppercase();
                if command.starts_with("USER ") {
                    ctrl.write_all(b"230 Logged in\r\n").unwrap();
                } else if command == "TYPE I" {
                    ctrl.write_all(b"200 Type set\r\n").unwrap();
                } else if command.starts_with("SIZE ") {
                    ctrl.write_all(b"213 4\r\n").unwrap();
                } else if command == "PASV" {
                    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                    let data_port = listener.local_addr().unwrap().port();
                    data_listener = Some(listener);
                    ctrl.write_all(
                        format!(
                            "227 Entering Passive Mode (127,0,0,1,{},{})\r\n",
                            data_port / 256,
                            data_port % 256
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                } else if command.starts_with("RETR ") {
                    ctrl.write_all(b"150 Opening data\r\n").unwrap();
                    let (mut data, _) = data_listener.take().unwrap().accept().unwrap();
                    data.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    stalled_tx.send(()).unwrap();
                    let mut byte = [0u8; 1];
                    let _ = std::io::Read::read(&mut data, &mut byte);
                    break;
                } else {
                    panic!("unexpected FTP command: {command}");
                }
            }
        });
        (port, stalled_rx, server)
    }

    fn assert_stalled_transfer_interrupts(action: &str) {
        let (port, stalled_rx, server) = spawn_stalled_ftp_server();
        let root = std::env::temp_dir().join(format!(
            "hls-v7-ftp-stall-{}-{}-{action}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("payload.downloading");
        let control = root.join("control");
        std::fs::write(&control, "run").unwrap();
        let worker_output = output.clone();
        let worker_control = control.clone();
        let url = format!("ftp://127.0.0.1:{port}/file.bin");
        let worker =
            std::thread::spawn(move || download_ftp(&url, &worker_output, &worker_control, false));
        stalled_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("FTP data connection did not enter stalled read");
        let started = std::time::Instant::now();
        std::fs::write(&control, action).unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(
            error,
            if action == "pause" {
                "paused"
            } else {
                "canceled"
            }
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{action} took too long to interrupt a stalled FTP data read: {:?}",
            started.elapsed()
        );
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stalled_data_channel_observes_pause_and_cancel_promptly() {
        assert_stalled_transfer_interrupts("pause");
        assert_stalled_transfer_interrupts("cancel");
    }
}
