//! FTP / FTPS single-stream download with version-checked SIZE+MDTM+REST resume.

use std::cell::Cell;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct FtpSocket<'a> {
    stream: TcpStream,
    control: &'a FtpControl<'a>,
}

impl<'a> FtpSocket<'a> {
    fn new(stream: TcpStream, control: &'a FtpControl<'a>) -> Result<Self, String> {
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_millis(100)))
            .map_err(|error| error.to_string())?;
        Ok(Self { stream, control })
    }

    fn io<T>(
        &mut self,
        mut operation: impl FnMut(&mut TcpStream) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let started = Instant::now();
        loop {
            self.control.check().map_err(std::io::Error::other)?;
            match operation(&mut self.stream) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) && started.elapsed() < Duration::from_secs(30) => {}
                result => return result,
            }
        }
    }
}

impl Read for FtpSocket<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // 轮询超时在 TCP 层内部续等，不能传给 Schannel 破坏握手/记录读取状态。
        self.io(|stream| stream.read(buf))
    }
}

impl Write for FtpSocket<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.io(|stream| stream.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.control.check().map_err(std::io::Error::other)?;
        self.stream.flush()
    }
}

enum Conn<'a> {
    Plain(FtpSocket<'a>),
    #[cfg(windows)]
    Tls(
        Box<schannel::tls_stream::TlsStream<FtpSocket<'a>>>,
        schannel::schannel_cred::SchannelCred,
    ),
}

impl Read for Conn<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf),
            #[cfg(windows)]
            Self::Tls(stream, _) => stream.read(buf),
        }
    }
}

impl<'a> Conn<'a> {
    fn peer_addr(&self) -> Result<std::net::SocketAddr, String> {
        match self {
            Self::Plain(stream) => stream.stream.peer_addr().map_err(|error| error.to_string()),
            #[cfg(windows)]
            Self::Tls(stream, _) => stream
                .get_ref()
                .stream
                .peer_addr()
                .map_err(|error| error.to_string()),
        }
    }

    fn secure_data(&self, stream: FtpSocket<'a>, host: &str) -> Result<Self, String> {
        #[cfg(windows)]
        if let Self::Tls(_, credentials) = self {
            // FTPS 服务器可能要求数据连接恢复控制连接的 TLS 会话；必须复用凭据句柄。
            let tls = schannel::tls_stream::Builder::new()
                .domain(host)
                .connect(credentials.clone(), stream)
                .map_err(|error| format!("FTPS data TLS handshake failed: {error:?}"))?;
            return Ok(Self::Tls(Box::new(tls), credentials.clone()));
        }
        let _ = (stream, host);
        Err("FTPS control connection is not TLS".into())
    }
}

impl Write for Conn<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buf),
            #[cfg(windows)]
            Self::Tls(stream, _) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            #[cfg(windows)]
            Self::Tls(stream, _) => stream.flush(),
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

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct ResumeState {
    version: u32,
    host: String,
    port: u16,
    user: String,
    path: String,
    tls: bool,
    total: u64,
    modified: String,
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
    let user = percent_decode(user)?;
    let password = percent_decode(password)?;
    let default_port = if tls { 990 } else { 21 };
    let parse_port = |port: &str| -> Result<u16, String> {
        let port = port
            .parse::<u16>()
            .map_err(|_| "FTP port invalid".to_string())?;
        if port == 0 {
            return Err("FTP port invalid".into());
        }
        Ok(port)
    };
    let (host, port) = if let Some(bracketed) = hostport.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or("FTP IPv6 address invalid")?;
        host.parse::<std::net::Ipv6Addr>()
            .map_err(|_| "FTP IPv6 address invalid".to_string())?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            parse_port(suffix.strip_prefix(':').ok_or("FTP port invalid")?)?
        };
        (host, port)
    } else if let Some((host, port)) = hostport.rsplit_once(':') {
        if host.contains(':') {
            return Err("FTP IPv6 address must be bracketed".into());
        }
        (host, parse_port(port)?)
    } else {
        (hostport, default_port)
    };
    if host.is_empty() || !ftp_wire_ok(host) {
        return Err("FTP host missing".into());
    }
    let user = if user.is_empty() {
        "anonymous".to_string()
    } else {
        user
    };
    let path = format!("/{}", percent_decode(path.trim_start_matches('/'))?);
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

fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err("FTP URL percent escape is incomplete".into());
            }
            let encoded = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| "FTP URL percent escape is invalid".to_string())?;
            let decoded = u8::from_str_radix(encoded, 16)
                .map_err(|_| "FTP URL percent escape is invalid".to_string())?;
            out.push(decoded);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out)
        .map_err(|_| "FTP URL percent-decoded value is not valid UTF-8".to_string())
}

fn ftp_wire_ok(value: &str) -> bool {
    !value.chars().any(|ch| matches!(ch, '\r' | '\n' | '\0'))
}

#[derive(Debug)]
struct FtpControl<'a> {
    path: &'a Path,
    reason: Cell<crate::http_engine::Control>,
    checked: Cell<Instant>,
}

impl FtpControl<'_> {
    fn check(&self) -> Result<(), String> {
        if self.checked.get().elapsed() >= Duration::from_millis(100) {
            self.poll()?;
        }
        self.stopped()
    }

    fn poll(&self) -> Result<(), String> {
        if self.reason.get() == crate::http_engine::Control::Run {
            self.reason.set(crate::http_engine::read_control(self.path));
            self.checked.set(Instant::now());
        }
        self.stopped()
    }

    fn stopped(&self) -> Result<(), String> {
        match self.reason.get() {
            crate::http_engine::Control::Run => Ok(()),
            crate::http_engine::Control::Pause => Err("paused".into()),
            crate::http_engine::Control::Cancel => Err("canceled".into()),
        }
    }
}

pub fn download_ftp(url: &str, output: &Path, control: &Path, resume: bool) -> Result<u64, String> {
    let target = parse_ftp_url(url)?;
    let control = FtpControl {
        path: control,
        reason: Cell::new(crate::http_engine::Control::Run),
        checked: Cell::new(Instant::now()),
    };
    control.poll()?;
    let result = transfer_ftp(&target, output, &control, resume);
    control.poll()?;
    result
}

fn transfer_ftp(
    target: &FtpTarget,
    output: &Path,
    control: &FtpControl<'_>,
    resume: bool,
) -> Result<u64, String> {
    let progress = output.with_extension("progress.json");
    let implicit = target.tls && target.port == 990;
    control.check()?;
    let raw = TcpStream::connect((target.host.as_str(), target.port))
        .map_err(|error| error.to_string())?;
    let raw = FtpSocket::new(raw, control)?;
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
    }
    if target.tls {
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
    let remote_size = if reply_code(&size_reply) == Some(213) {
        size_reply
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
    } else {
        None
    };
    let size = remote_size.unwrap_or(0);
    let modified_reply = command(&mut ctrl, &format!("MDTM {}", target.path))?;
    let modified = if reply_code(&modified_reply) == Some(213) {
        modified_reply.split_whitespace().nth(1).filter(|value| {
            let (seconds, fraction) = value.split_once('.').unwrap_or((value, "0"));
            seconds.len() == 14
                && seconds.bytes().all(|byte| byte.is_ascii_digit())
                && !fraction.is_empty()
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
    } else {
        None
    };
    let state_path = output.with_extension("ftp-state.json");
    let state = modified.filter(|_| size > 0).map(|modified| ResumeState {
        version: 1,
        host: target.host.to_ascii_lowercase(),
        port: target.port,
        user: target.user.clone(),
        path: target.path.clone(),
        tls: target.tls,
        total: size,
        modified: modified.to_string(),
    });
    let saved_state = std::fs::read(&state_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ResumeState>(&bytes).ok());
    // A matching length alone cannot distinguish a replaced remote file. Missing
    // or invalid metadata must never authorize appending to existing bytes.
    let mut resume_from = if resume && state.is_some() && saved_state == state {
        std::fs::metadata(output)
            .map(|meta| meta.len())
            .unwrap_or(0)
    } else {
        0
    };
    if resume_from >= size {
        resume_from = 0;
    }
    let peer = ctrl.peer_addr()?;
    let epsv = command(&mut ctrl, "EPSV")?;
    // Only an unsupported command on IPv4 authorizes PASV. A malformed
    // success or a transient/permission error must not start another path.
    let data_port = if peer.is_ipv4() && matches!(reply_code(&epsv), Some(500 | 501 | 502 | 504)) {
        let pasv = command(&mut ctrl, "PASV")?;
        require_reply_code(&pasv, &[227], "PASV")?;
        parse_pasv_port(&pasv)?
    } else {
        require_reply_code(&epsv, &[229], "EPSV")?;
        parse_epsv_port(&epsv)?
    };
    // Both passive modes use the control peer, never a server-advertised host.
    let data_addr = std::net::SocketAddr::new(peer.ip(), data_port);
    control.check()?;
    let data_raw = TcpStream::connect(data_addr).map_err(|error| error.to_string())?;
    let data_raw = FtpSocket::new(data_raw, control)?;
    // REST 必须紧挨 RETR；中间发送 EPSV/PASV 会让部分服务器取消续传偏移。
    if resume_from > 0 {
        let reply = command(&mut ctrl, &format!("REST {resume_from}"))?;
        match reply_code(&reply) {
            Some(350) => {}
            Some(500 | 501 | 502 | 504) => resume_from = 0,
            _ => require_reply_code(&reply, &[350], "REST")?,
        }
    }
    // 服务器在收到 RETR 后才启动数据通道 TLS；先握手会让客户端与服务器互相等待。
    let reply = command(&mut ctrl, &format!("RETR {}", target.path))?;
    require_reply_code(&reply, &[125, 150], "RETR")?;
    let mut data = if target.tls {
        ctrl.secure_data(data_raw, &target.host)?
    } else {
        Conn::Plain(data_raw)
    };
    control.poll()?;
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
    // Persist identity only after the output has been opened/truncated. Otherwise
    // a failed fresh transfer could incorrectly label old bytes as the new file.
    control.check()?;
    if let Some(state) = state {
        let bytes = serde_json::to_vec(&state).map_err(|error| error.to_string())?;
        std::fs::write(&state_path, bytes).map_err(|error| error.to_string())?;
    } else if state_path.exists() {
        std::fs::remove_file(&state_path).map_err(|error| error.to_string())?;
    }
    // 数据留在调用线程，直接沿用任务/队列/全局限速上下文。
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = resume_from;
    crate::http_engine::write_progress(&progress, downloaded, size, 0.0, "downloading");
    loop {
        control.check()?;
        // 多读至多一个字节来确认 SIZE 边界；超出的响应必须在落盘前拒绝。
        let limit = remote_size.map_or(buf.len(), |size| {
            size.saturating_sub(downloaded)
                .saturating_add(1)
                .min(buf.len() as u64) as usize
        });
        let count = data
            .read(&mut buf[..limit])
            .map_err(|error| error.to_string())?;
        control.check()?;
        if count == 0 {
            break;
        }
        let received = downloaded
            .checked_add(count as u64)
            .ok_or("FTP transfer length overflow")?;
        if remote_size.is_some_and(|size| received > size) {
            return Err(format!(
                "FTP transfer exceeds declared size: expected {size}, received at least {received}"
            ));
        }
        crate::net_policy::consume(count, || control.check())?;
        control.check()?;
        file.write_all(&buf[..count])
            .map_err(|error| error.to_string())?;
        downloaded = received;
        crate::http_engine::write_progress(&progress, downloaded, size, 0.0, "downloading");
    }
    if remote_size.is_some_and(|size| downloaded != size) {
        return Err(format!(
            "FTP transfer size mismatch: expected {size}, received {downloaded}"
        ));
    }
    drop(data);
    file.sync_all().map_err(|error| error.to_string())?;
    control.check()?;
    let reply = read_reply(&mut ctrl)?;
    require_reply_code(&reply, &[226, 250], "transfer completion")?;
    control.poll()?;
    let _ = std::fs::remove_file(state_path);
    Ok(downloaded)
}

fn wrap_tls<'a>(stream: FtpSocket<'a>, host: &str) -> Result<Conn<'a>, String> {
    #[cfg(windows)]
    {
        let cred = schannel::schannel_cred::SchannelCred::builder()
            // FTPS 数据通道需要恢复控制会话；本机 Schannel 的 TLS 1.3 重连被真实服务器拒绝。
            .enabled_protocols(&[schannel::schannel_cred::Protocol::Tls12])
            .acquire(schannel::schannel_cred::Direction::Outbound)
            .map_err(|error| error.to_string())?;
        let tls = schannel::tls_stream::Builder::new()
            .domain(host)
            .connect(cred.clone(), stream)
            .map_err(|error| format!("FTPS control TLS handshake failed: {error:?}"))?;
        Ok(Conn::Tls(Box::new(tls), cred))
    }
    #[cfg(not(windows))]
    {
        let _ = (stream, host);
        Err("FTPS uses Windows Schannel in v6".into())
    }
}

fn take_plain<'a>(conn: Conn<'a>) -> Result<FtpSocket<'a>, String> {
    match conn {
        Conn::Plain(stream) => Ok(stream),
        #[cfg(windows)]
        Conn::Tls(_, _) => Err("control socket already TLS".into()),
    }
}

fn command(stream: &mut Conn<'_>, line: &str) -> Result<String, String> {
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

fn read_reply(stream: &mut Conn<'_>) -> Result<String, String> {
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
    let (_, address) = reply.split_once('(').ok_or("PASV missing host")?;
    let (address, _) = address.split_once(')').ok_or("PASV missing host")?;
    let nums: Vec<u8> = address
        .split(',')
        .map(|item| item.trim().parse::<u8>())
        .collect::<Result<_, _>>()
        .map_err(|_| "PASV address invalid".to_string())?;
    if nums.len() != 6 {
        return Err("PASV address invalid".into());
    }
    let port = u16::from_be_bytes([nums[4], nums[5]]);
    if port == 0 {
        return Err("PASV port invalid".into());
    }
    Ok(port)
}

fn parse_epsv_port(reply: &str) -> Result<u16, String> {
    let (_, address) = reply.split_once('(').ok_or("EPSV missing port")?;
    let (address, _) = address.rsplit_once(')').ok_or("EPSV missing port")?;
    let bytes = address.as_bytes();
    let delimiter = *bytes.first().ok_or("EPSV address invalid")?;
    if !(33..=126).contains(&delimiter)
        || bytes.len() < 5
        || bytes[1] != delimiter
        || bytes[2] != delimiter
        || bytes.last() != Some(&delimiter)
    {
        return Err("EPSV address invalid".into());
    }
    let port_text = &address[3..address.len() - 1];
    if !port_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("EPSV port invalid".into());
    }
    let port = port_text
        .parse::<u16>()
        .map_err(|_| "EPSV port invalid".to_string())?;
    if port == 0 {
        return Err("EPSV port invalid".into());
    }
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

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
        let ipv6 = parse_ftp_url("ftp://alice:secret@[::1]:2121/file.bin").unwrap();
        assert_eq!(ipv6.host, "::1");
        assert_eq!(ipv6.port, 2121);
        assert_eq!(parse_ftp_url("ftps://[::1]/a").unwrap().port, 990);
        for invalid in [
            "ftp://[::1/a",
            "ftp://[::1]x/a",
            "ftp://[host]/a",
            "ftp://::1/a",
        ] {
            assert!(parse_ftp_url(invalid).is_err(), "{invalid}");
        }
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
        for malformed in [
            "ftp://files.example/a%",
            "ftp://files.example/a%2",
            "ftp://files.example/a%GG.bin",
            "ftp://alice%GG@files.example/a.bin",
        ] {
            let error = parse_ftp_url(malformed).unwrap_err();
            assert!(error.contains("percent escape"), "{malformed}: {error}");
        }
        let invalid_utf8 = parse_ftp_url("ftp://files.example/a%FF.bin").unwrap_err();
        assert!(invalid_utf8.contains("valid UTF-8"), "{invalid_utf8}");
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
        for tuple in [
            "1,2,3,4,5,256",
            "1,2,3,4,256,5",
            "1,2,3,4,bad,5,6",
            "1,2,3,4,5,6,7",
            "1,2,3,4,5",
            "1,2,3,4,-1,5",
        ] {
            assert!(
                parse_pasv_port(&format!("227 Passive ({tuple})")).is_err(),
                "{tuple}"
            );
        }
        assert!(parse_pasv_port("227 ) before (").is_err());
    }

    #[test]
    fn epsv_requires_empty_address_fields_and_valid_port() {
        for delimiter in ['|', '!', '(', ')', '1'] {
            let reply = format!("229 Passive ({delimiter}{delimiter}{delimiter}6446{delimiter})");
            assert_eq!(parse_epsv_port(&reply).unwrap(), 6446, "{reply}");
        }
        for tuple in [
            "",
            "|||",
            "|||0|",
            "|||65536|",
            "||127.0.0.1|6446|",
            "|1||6446|",
            "|||+6446|",
            "||| 6446|",
            "|||6446!",
            "   6446 ",
            "ééé6446é",
        ] {
            assert!(
                parse_epsv_port(&format!("229 Passive ({tuple})")).is_err(),
                "{tuple}"
            );
        }
        assert!(parse_epsv_port("229 ) before (").is_err());
    }

    #[test]
    fn passive_negotiation_prefers_epsv_and_only_falls_back_when_unsupported() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        for (case, epsv_reply, pasv_reply, success) in [
            ("epsv", "229", "", true),
            ("ipv6", "229", "", true),
            ("ipv6-no-pasv", "502 Not implemented", "", false),
            ("legacy-500", "500 Unknown command", "227", true),
            ("legacy-501", "501 Unsupported syntax", "227", true),
            ("legacy-502", "502 Not implemented", "227", true),
            ("legacy-504", "504 Unsupported command", "227", true),
            ("bad-epsv", "229 Passive (|||65536|)", "", false),
            ("epsv-unavailable", "425 Cannot open data", "", false),
            ("epsv-auth", "530 Not logged in", "", false),
            (
                "pasv-unavailable",
                "502 Not implemented",
                "425 Cannot open data",
                false,
            ),
            (
                "bad-pasv",
                "502 Not implemented",
                "227 Passive (127,0,0,1,256,1)",
                false,
            ),
        ] {
            let listener = TcpListener::bind(if case.starts_with("ipv6") {
                "[::1]:0"
            } else {
                "127.0.0.1:0"
            })
            .unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut ctrl, _) = listener.accept().unwrap();
                ctrl.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                ctrl.write_all(b"220 Ready\r\n").unwrap();
                let mut reader = BufReader::new(ctrl.try_clone().unwrap());
                let mut data_listener = None;
                let mut commands = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        break;
                    }
                    let command = line.trim_end();
                    commands.push(command.to_string());
                    match command {
                        "USER anonymous" => ctrl.write_all(b"230 Logged in\r\n").unwrap(),
                        "TYPE I" => ctrl.write_all(b"200 Binary\r\n").unwrap(),
                        "SIZE /file.bin" => ctrl.write_all(b"213 8\r\n").unwrap(),
                        "MDTM /file.bin" => ctrl.write_all(b"213 20261115000000\r\n").unwrap(),
                        "EPSV" | "PASV" => {
                            let reply = if command == "EPSV" {
                                epsv_reply
                            } else {
                                pasv_reply
                            };
                            if reply == "229" || reply == "227" {
                                let listener = TcpListener::bind((address.ip(), 0)).unwrap();
                                let port = listener.local_addr().unwrap().port();
                                data_listener = Some(listener);
                                if reply == "229" {
                                    write!(ctrl, "229 Passive (!!!{port}!)\r\n").unwrap();
                                } else {
                                    // A different advertised IP must not redirect the data socket.
                                    write!(
                                        ctrl,
                                        "227 Passive (192,0,2,1,{},{})\r\n",
                                        port / 256,
                                        port % 256
                                    )
                                    .unwrap();
                                }
                            } else {
                                write!(ctrl, "{reply}\r\n").unwrap();
                            }
                        }
                        "RETR /file.bin" => {
                            assert!(success, "{case}: failed negotiation must not retrieve");
                            ctrl.write_all(b"150 Opening data\r\n").unwrap();
                            let (mut data, _) = data_listener.take().unwrap().accept().unwrap();
                            data.write_all(b"AAAABBBB").unwrap();
                            drop(data);
                            ctrl.write_all(b"226 Complete\r\n").unwrap();
                            break;
                        }
                        _ => panic!("{case}: unexpected command {command}"),
                    }
                }
                commands
            });
            let root =
                std::env::temp_dir().join(format!("hls-ftp-passive-{}-{case}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let output = root.join("payload.downloading");
            let control = root.join("control");
            std::fs::write(&control, "run").unwrap();
            std::fs::write(&output, b"original").unwrap();
            let result = download_ftp(
                &format!("ftp://{address}/file.bin"),
                &output,
                &control,
                false,
            );
            let commands = server.join().unwrap();
            let bytes = std::fs::read(&output).unwrap();
            let has_state = output.with_extension("ftp-state.json").exists();
            std::fs::remove_dir_all(root).unwrap();
            assert_eq!(result.is_ok(), success, "{case}: {result:?}");
            assert_eq!(
                commands.iter().any(|item| item == "PASV"),
                !pasv_reply.is_empty(),
                "{case}"
            );
            assert_eq!(
                bytes,
                if success { b"AAAABBBB" } else { b"original" },
                "{case}"
            );
            assert!(
                !has_state,
                "{case}: failed negotiation must not change resource identity"
            );
        }
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

    #[test]
    fn resumes_only_the_saved_ftp_resource_version() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        for (case, mtime, body, expect_rest) in [
            (
                "unchanged",
                Some("20261114000100"),
                b"AAAABBBB".as_slice(),
                true,
            ),
            (
                "changed-time",
                Some("20261114000200"),
                b"CCCCDDDD".as_slice(),
                false,
            ),
            ("no-mdtm", None, b"CCCCDDDD".as_slice(), false),
            (
                "invalid-mdtm",
                Some("invalid"),
                b"CCCCDDDD".as_slice(),
                false,
            ),
            (
                "no-size",
                Some("20261114000100"),
                b"CCCCDDDD".as_slice(),
                false,
            ),
            (
                "missing-state",
                Some("20261114000100"),
                b"CCCCDDDD".as_slice(),
                false,
            ),
            (
                "corrupt-state",
                Some("20261114000100"),
                b"CCCCDDDD".as_slice(),
                false,
            ),
            (
                "different-path",
                Some("20261114000100"),
                b"CCCCDDDD".as_slice(),
                false,
            ),
            (
                "changed-size",
                Some("20261114000100"),
                b"CCCCDDDDEEEE".as_slice(),
                false,
            ),
            (
                "rest-unsupported",
                Some("20261114000100"),
                b"AAAABBBB".as_slice(),
                true,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let mut requests = Vec::new();
                for pass in 0..2 {
                    let (mut ctrl, _) = listener.accept().unwrap();
                    ctrl.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    ctrl.write_all(b"220 Ready\r\n").unwrap();
                    let mut reader = BufReader::new(ctrl.try_clone().unwrap());
                    let mut data_listener = None;
                    let mut offset = 0;
                    let mut commands = Vec::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 {
                            break;
                        }
                        let command = line.trim_end();
                        commands.push(command.to_string());
                        if command.starts_with("USER ") {
                            ctrl.write_all(b"230 Logged in\r\n").unwrap();
                        } else if command == "TYPE I" {
                            ctrl.write_all(b"200 Type set\r\n").unwrap();
                        } else if command.starts_with("SIZE ") {
                            if case == "no-size" {
                                ctrl.write_all(b"502 SIZE unsupported\r\n").unwrap();
                            } else {
                                let size = if pass == 0 { 8 } else { body.len() };
                                write!(ctrl, "213 {size}\r\n").unwrap();
                            }
                        } else if command.starts_with("MDTM ") {
                            if let Some(mtime) = mtime {
                                let mtime = if pass == 0 { "20261114000100" } else { mtime };
                                write!(ctrl, "213 {mtime}\r\n").unwrap();
                            } else {
                                ctrl.write_all(b"502 MDTM unsupported\r\n").unwrap();
                            }
                        } else if command == "EPSV" {
                            if case == "unchanged" {
                                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                                let port = listener.local_addr().unwrap().port();
                                data_listener = Some(listener);
                                write!(ctrl, "229 Passive (|||{port}|)\r\n").unwrap();
                            } else {
                                ctrl.write_all(b"502 EPSV unsupported\r\n").unwrap();
                            }
                        } else if command == "PASV" {
                            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                            let port = listener.local_addr().unwrap().port();
                            data_listener = Some(listener);
                            write!(
                                ctrl,
                                "227 Passive (127,0,0,1,{},{})\r\n",
                                port / 256,
                                port % 256
                            )
                            .unwrap();
                        } else if let Some(value) = command.strip_prefix("REST ") {
                            if case == "rest-unsupported" {
                                ctrl.write_all(b"502 REST unsupported\r\n").unwrap();
                            } else {
                                offset = value.parse::<usize>().unwrap();
                                ctrl.write_all(b"350 Restarting\r\n").unwrap();
                            }
                        } else if command.starts_with("RETR ") {
                            ctrl.write_all(b"150 Opening data\r\n").unwrap();
                            let (mut data, _) = data_listener.take().unwrap().accept().unwrap();
                            data.write_all(if pass == 0 { b"AAAA" } else { &body[offset..] })
                                .unwrap();
                            drop(data);
                            ctrl.write_all(if pass == 0 {
                                b"426 Transfer aborted\r\n"
                            } else {
                                b"226 Transfer complete\r\n"
                            })
                            .unwrap();
                            break;
                        } else {
                            panic!("unexpected FTP command: {command}");
                        }
                    }
                    requests.push(commands);
                }
                requests
            });
            let root =
                std::env::temp_dir().join(format!("hls-ftp-resume-{}-{case}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let output = root.join("payload.downloading");
            let control = root.join("control");
            std::fs::write(&control, "run").unwrap();
            let url = format!("ftp://alice:secret@{address}/file.bin");
            let first = download_ftp(&url, &output, &control, true);
            assert!(
                first.is_err(),
                "first response was deliberately interrupted"
            );
            assert_eq!(std::fs::read(&output).unwrap(), b"AAAA");
            let state_path = output.with_extension("ftp-state.json");
            if case == "missing-state" {
                let _ = std::fs::remove_file(&state_path);
            } else if case == "corrupt-state" {
                std::fs::write(&state_path, "{broken").unwrap();
            }
            let state = std::fs::read_to_string(&state_path).unwrap_or_default();
            let next_url = if case == "different-path" {
                url.replace("file.bin", "other.bin")
            } else {
                url
            };
            let result = download_ftp(&next_url, &output, &control, true);
            let requests = server.join().unwrap();
            let received = std::fs::read(&output).unwrap();
            let state_remains = state_path.exists();
            std::fs::remove_dir_all(root).unwrap();
            assert_eq!(result.unwrap(), body.len() as u64, "{case}");
            assert_eq!(
                received, body,
                "{case}: must not splice old and new file bytes"
            );
            assert_eq!(
                requests[1].iter().any(|line| line == "REST 4"),
                expect_rest,
                "{case}"
            );
            if expect_rest {
                let rest = requests[1]
                    .iter()
                    .position(|line| line == "REST 4")
                    .unwrap();
                assert!(
                    requests[1][rest + 1].starts_with("RETR "),
                    "{case}: REST must immediately precede RETR"
                );
            }
            assert!(
                !state.contains("secret"),
                "resume metadata must not persist FTP passwords"
            );
            assert!(
                !state_remains,
                "{case}: completed transfer must remove resume metadata"
            );
        }
    }

    fn spawn_stalled_ftp_server(
        phase: &'static str,
    ) -> (
        u16,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (stalled_tx, stalled_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut ctrl, _) = listener.accept().unwrap();
            ctrl.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let stall = || {
                stalled_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            };
            if phase == "greeting" {
                stall();
                return;
            }
            ctrl.write_all(b"220 Ready\r\n").unwrap();
            let mut reader = std::io::BufReader::new(ctrl.try_clone().unwrap());
            let mut data_listener = None;
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                    break;
                }
                let command = line.trim_end().to_ascii_uppercase();
                if command.split_whitespace().next() == Some(phase) {
                    stall();
                    break;
                }
                if command.starts_with("USER ") {
                    ctrl.write_all(b"331 Password required\r\n").unwrap();
                } else if command == "PASS" || command.starts_with("PASS ") {
                    ctrl.write_all(b"230 Logged in\r\n").unwrap();
                } else if command == "AUTH TLS" {
                    ctrl.write_all(b"234 Begin TLS\r\n").unwrap();
                    let mut tls_header = [0u8; 5];
                    reader.read_exact(&mut tls_header).unwrap();
                    assert_eq!(tls_header[0], 22, "client must start a TLS handshake");
                    stall();
                    break;
                } else if command == "TYPE I" {
                    ctrl.write_all(b"200 Type set\r\n").unwrap();
                } else if command.starts_with("SIZE ") {
                    ctrl.write_all(b"213 8\r\n").unwrap();
                } else if command.starts_with("MDTM ") {
                    ctrl.write_all(b"213 20261116000000\r\n").unwrap();
                } else if command == "EPSV" {
                    ctrl.write_all(b"502 EPSV unsupported\r\n").unwrap();
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
                } else if command.starts_with("REST ") {
                    ctrl.write_all(b"350 Restarting\r\n").unwrap();
                } else if command.starts_with("RETR ") {
                    ctrl.write_all(b"150 Opening data\r\n").unwrap();
                    let (mut data, _) = data_listener.take().unwrap().accept().unwrap();
                    if phase == "data" {
                        stall();
                    } else {
                        data.write_all(b"AAAABBBB").unwrap();
                        // Signal EOF, then wait for the client to close its data socket.
                        // This confirms the output is written before testing the final reply stall.
                        data.shutdown(std::net::Shutdown::Write).unwrap();
                        data.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                        assert_eq!(data.read(&mut [0u8; 1]).unwrap(), 0);
                        drop(data);
                        assert_eq!(phase, "completion");
                        stall();
                    }
                    break;
                } else {
                    panic!("unexpected FTP command: {command}");
                }
            }
        });
        (port, stalled_rx, release_tx, server)
    }

    fn assert_stalled_transfer_interrupts(phase: &'static str, action: &str) {
        let (port, stalled_rx, release_tx, server) = spawn_stalled_ftp_server(phase);
        let root = std::env::temp_dir().join(format!(
            "hls-v7-ftp-stall-{}-{}-{phase}-{action}",
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
        std::fs::write(&output, b"keep").unwrap();
        let worker_output = output.clone();
        let worker_control = control.clone();
        let scheme = if phase == "tls" || phase == "AUTH" {
            "ftps"
        } else {
            "ftp"
        };
        let url = format!("{scheme}://127.0.0.1:{port}/file.bin");
        if phase == "REST" {
            let target = parse_ftp_url(&url).unwrap();
            let state = ResumeState {
                version: 1,
                host: target.host,
                port: target.port,
                user: target.user,
                path: target.path,
                tls: target.tls,
                total: 8,
                modified: "20261116000000".into(),
            };
            std::fs::write(
                output.with_extension("ftp-state.json"),
                serde_json::to_vec(&state).unwrap(),
            )
            .unwrap();
        }
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = download_ftp(&url, &worker_output, &worker_control, true);
            result_tx.send(result).unwrap();
        });
        stalled_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("FTP did not enter the requested stalled phase");
        let started = std::time::Instant::now();
        std::fs::write(&control, action).unwrap();
        let observed = result_rx.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        server.join().unwrap();
        let bytes = std::fs::read(&output).unwrap();
        let _ = std::fs::remove_dir_all(root);
        let error = observed
            .unwrap_or_else(|error| {
                panic!("{phase}/{action}: FTP stop waited for the server: {error}")
            })
            .unwrap_err();
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
            "{phase}: {action} took too long to interrupt FTP: {:?}",
            started.elapsed()
        );
        if phase == "data" {
            assert!(bytes.is_empty() || bytes == b"keep");
        } else {
            assert_eq!(
                bytes,
                if phase == "completion" {
                    b"AAAABBBB".as_slice()
                } else {
                    b"keep".as_slice()
                },
                "{phase}: do not modify output before transfer acceptance"
            );
        }
    }

    #[test]
    fn stalled_data_channel_observes_pause_and_cancel_promptly() {
        assert_stalled_transfer_interrupts("data", "pause");
        assert_stalled_transfer_interrupts("data", "cancel");
    }

    #[test]
    fn stalled_control_phases_observe_pause_and_cancel_promptly() {
        for phase in [
            "greeting",
            "USER",
            "PASS",
            "TYPE",
            "SIZE",
            "MDTM",
            "EPSV",
            "PASV",
            "REST",
            "RETR",
            "completion",
        ] {
            for action in ["pause", "cancel"] {
                assert_stalled_transfer_interrupts(phase, action);
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn stalled_control_tls_handshake_observes_pause_and_cancel_promptly() {
        for phase in ["AUTH", "tls"] {
            for action in ["pause", "cancel"] {
                assert_stalled_transfer_interrupts(phase, action);
            }
        }
    }

    #[test]
    fn stopped_transfer_preserves_output_without_opening_a_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let root = std::env::temp_dir().join(format!("hls-ftp-prestop-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("payload.downloading");
        let control = root.join("control");
        std::fs::write(&output, b"original").unwrap();
        for (action, error) in [("pause", "paused"), ("cancel", "canceled")] {
            std::fs::write(&control, action).unwrap();
            assert_eq!(
                download_ftp(
                    &format!("ftp://{}/file.bin", listener.local_addr().unwrap()),
                    &output,
                    &control,
                    true,
                ),
                Err(error.into())
            );
            assert_eq!(std::fs::read(&output).unwrap(), b"original");
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_data_length_before_writing_and_requires_final_success() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        for (case, size, body, completion, success) in [
            ("exact", Some(8), b"AAAABBBB".to_vec(), Some(226), true),
            (
                "delayed-success",
                Some(8),
                b"AAAABBBB".to_vec(),
                Some(226),
                true,
            ),
            (
                "alternate-success",
                Some(8),
                b"AAAABBBB".to_vec(),
                Some(250),
                true,
            ),
            (
                "overrun",
                Some(8),
                b"AAAABBBBCCCC".to_vec(),
                Some(226),
                false,
            ),
            (
                "late-overrun",
                Some(65536),
                vec![42; 65537],
                Some(226),
                false,
            ),
            ("short", Some(8), b"AAAA".to_vec(), Some(226), false),
            (
                "short-no-completion",
                Some(8),
                b"AAAA".to_vec(),
                None,
                false,
            ),
            ("empty", Some(0), Vec::new(), Some(226), true),
            ("empty-overrun", Some(0), b"A".to_vec(), Some(226), false),
            ("unknown-size", None, b"AAAABBBB".to_vec(), Some(226), true),
            (
                "completion-failure",
                Some(8),
                b"AAAABBBB".to_vec(),
                Some(426),
                false,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server_body = body.clone();
            let (release_tx, release_rx) = mpsc::channel();
            let server = std::thread::spawn(move || {
                let (mut ctrl, _) = listener.accept().unwrap();
                ctrl.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                ctrl.write_all(b"220 Ready\r\n").unwrap();
                let mut reader = BufReader::new(ctrl.try_clone().unwrap());
                let mut data_listener = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        break;
                    }
                    match line.trim_end() {
                        "USER anonymous" => ctrl.write_all(b"230 Logged in\r\n").unwrap(),
                        "TYPE I" => ctrl.write_all(b"200 Binary\r\n").unwrap(),
                        "SIZE /file.bin" => {
                            if let Some(size) = size {
                                write!(ctrl, "213 {size}\r\n").unwrap();
                            } else {
                                ctrl.write_all(b"502 SIZE unsupported\r\n").unwrap();
                            }
                        }
                        "MDTM /file.bin" => ctrl.write_all(b"213 20261116000000\r\n").unwrap(),
                        "EPSV" => {
                            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                            let port = listener.local_addr().unwrap().port();
                            data_listener = Some(listener);
                            write!(ctrl, "229 Passive (|||{port}|)\r\n").unwrap();
                        }
                        "RETR /file.bin" => {
                            if case == "delayed-success" {
                                // Both waits cross the TCP poll interval without stopping the task.
                                std::thread::sleep(Duration::from_millis(250));
                            }
                            ctrl.write_all(b"150 Opening data\r\n").unwrap();
                            let (mut data, _) = data_listener.take().unwrap().accept().unwrap();
                            if case == "delayed-success" {
                                std::thread::sleep(Duration::from_millis(250));
                            }
                            data.write_all(&server_body).unwrap();
                            drop(data);
                            if let Some(code) = completion {
                                // Oversized responses may already have closed the control socket.
                                let _ = write!(ctrl, "{code} Transfer finished\r\n");
                            } else {
                                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                            }
                            break;
                        }
                        command => panic!("{case}: unexpected command {command}"),
                    }
                }
            });
            let root =
                std::env::temp_dir().join(format!("hls-ftp-length-{}-{case}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let output = root.join("payload.downloading");
            let control = root.join("control");
            std::fs::write(&control, "run").unwrap();
            let worker_output = output.clone();
            let (result_tx, result_rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let result = download_ftp(
                    &format!("ftp://{address}/file.bin"),
                    &worker_output,
                    &control,
                    true,
                );
                result_tx.send(result).unwrap();
            });
            let result = result_rx.recv_timeout(Duration::from_secs(2));
            let _ = release_tx.send(());
            worker.join().unwrap();
            server.join().unwrap();
            let bytes = std::fs::read(&output).unwrap();
            let has_state = output.with_extension("ftp-state.json").exists();
            std::fs::remove_dir_all(root).unwrap();
            let result = result.expect("length errors must not wait for the final reply");
            assert_eq!(result.is_ok(), success, "{case}: {result:?}");
            assert!(
                body.starts_with(&bytes),
                "{case}: output must be a valid prefix"
            );
            if let Some(size) = size {
                assert!(
                    bytes.len() as u64 <= size,
                    "{case}: no excess bytes may be written"
                );
            }
            if success {
                assert_eq!(result.unwrap(), body.len() as u64, "{case}");
                assert_eq!(bytes, body, "{case}");
                assert!(!has_state, "{case}: success removes resume identity");
            } else {
                let error = result.unwrap_err();
                assert!(
                    error.contains(if case == "completion-failure" {
                        "transfer completion rejected"
                    } else if case.starts_with("short") {
                        "size mismatch"
                    } else {
                        "exceeds declared size"
                    }),
                    "{case}: {error}"
                );
                assert_eq!(has_state, size.is_some_and(|size| size > 0), "{case}");
            }
        }
    }
}
