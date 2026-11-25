//! FTP / FTPS single-stream download with version-checked SIZE+MDTM+REST resume.
//!
//! Data connections are passive (EPSV, PASV for IPv4) by default. When the
//! server refuses passive mode or its passive port cannot be reached, the
//! transfer falls back to active mode (EPRT, PORT for IPv4) and remembers
//! that for the rest of the transfer, so segmented helpers go active directly.
//! FTPS active mode uses the same client-side data TLS as passive mode.

use crate::proxy_tunnel::Proxy;
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
    fn ftp_socket(&self) -> &FtpSocket<'a> {
        match self {
            Self::Plain(stream) => stream,
            #[cfg(windows)]
            Self::Tls(stream, _) => stream.get_ref(),
        }
    }

    fn socket(&self) -> &TcpStream {
        &self.ftp_socket().stream
    }

    fn peer_addr(&self) -> Result<std::net::SocketAddr, String> {
        self.socket().peer_addr().map_err(|error| error.to_string())
    }

    fn local_addr(&self) -> Result<std::net::SocketAddr, String> {
        self.socket()
            .local_addr()
            .map_err(|error| error.to_string())
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

#[derive(Clone, PartialEq)]
pub struct FtpTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub path: String,
    pub tls: bool,
}

impl std::fmt::Debug for FtpTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpTarget")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &"***")
            .field("path", &self.path)
            .field("tls", &self.tls)
            .finish()
    }
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Clone, Debug)]
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
    let (_, rest) = url.split_once("://").ok_or("not an FTP URL")?;
    let (auth, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (userinfo, hostport) = auth.rsplit_once('@').unwrap_or(("anonymous:", auth));
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
    proxy: Option<&'a Proxy>,
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

/// Single-connection FTP download (the engine calls [`download_ftp_with`]).
#[cfg_attr(not(test), allow(dead_code))]
pub fn download_ftp(url: &str, output: &Path, control: &Path, resume: bool) -> Result<u64, String> {
    download_ftp_with(url, output, control, resume, 1)
}

/// FTP download with up to `connections` parallel REST segments (see
/// [`transfer_segmented`]); `1` keeps the single-connection transfer.
pub fn download_ftp_with(
    url: &str,
    output: &Path,
    control: &Path,
    resume: bool,
    connections: usize,
) -> Result<u64, String> {
    download_ftp_routed(url, output, control, resume, connections, "")
}

pub(crate) fn download_ftp_routed(
    url: &str,
    output: &Path,
    control: &Path,
    resume: bool,
    connections: usize,
    route: &str,
) -> Result<u64, String> {
    let target = parse_ftp_url(url)?;
    let mut control = FtpControl {
        proxy: None,
        path: control,
        reason: Cell::new(crate::http_engine::Control::Run),
        checked: Cell::new(Instant::now()),
    };
    control.poll()?;
    let proxy = Proxy::resolve(route, url, || control.check())?;
    control.proxy = proxy.as_ref();
    // 直连遇到被动模式被拒/不可达时切到主动模式，后续连接沿用。
    let active = std::sync::atomic::AtomicBool::new(false);
    let result = match transfer_ftp(
        &target,
        output,
        &control,
        resume,
        connections.max(1),
        &active,
    ) {
        // 服务器声明了 REST STREAM 却拒绝首个分段的 REST：回落单连接（从头下载）。
        Err(error) if error == SEGMENTED_FALLBACK => {
            transfer_ftp(&target, output, &control, false, 1, &active)
        }
        other => other,
    };
    control.poll()?;
    result
}

/// Connects, upgrades to TLS when requested, logs in and selects binary mode.
fn open_session<'a>(target: &FtpTarget, control: &'a FtpControl<'a>) -> Result<Conn<'a>, String> {
    let implicit = target.tls && target.port == 990;
    control.check()?;
    let raw = match control.proxy {
        Some(proxy) => proxy.connect_blocking(&target.host, target.port, || control.check())?,
        None => TcpStream::connect((target.host.as_str(), target.port))
            .map_err(|error| error.to_string())?,
    };
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
    Ok(ctrl)
}

/// How long to wait for a passive connect or an active-mode accept.
const DATA_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// A data connection that is ready (passive) or will be opened by the
/// server after `RETR` (active). Call [`DataChannel::establish`] after the
/// `RETR` preliminary reply.
enum DataChannel<'a> {
    Passive(FtpSocket<'a>),
    Active {
        listener: std::net::TcpListener,
        server_ip: std::net::IpAddr,
        control: &'a FtpControl<'a>,
    },
}

impl<'a> DataChannel<'a> {
    fn establish(self) -> Result<FtpSocket<'a>, String> {
        match self {
            Self::Passive(socket) => Ok(socket),
            Self::Active {
                listener,
                server_ip,
                control,
            } => {
                let started = Instant::now();
                loop {
                    control.check()?;
                    match listener.accept() {
                        // 只接受控制连接对端发起的数据连接，防止端口被第三方抢占。
                        Ok((stream, from)) if from.ip() == server_ip => {
                            stream
                                .set_nonblocking(false)
                                .map_err(|error| error.to_string())?;
                            return FtpSocket::new(stream, control);
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if started.elapsed() >= DATA_CONNECT_TIMEOUT {
                                return Err("FTP active mode: server did not connect".into());
                            }
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                }
            }
        }
    }
}

/// Why passive setup failed: `Refused` permits the active-mode fallback,
/// `Fatal` (malformed replies, login/closing errors, pause/cancel) does not.
enum PassiveError {
    Refused(String),
    Fatal(String),
}

impl From<String> for PassiveError {
    fn from(value: String) -> Self {
        Self::Fatal(value)
    }
}

/// Negative replies that mean "passive mode is not available here".
fn passive_refused(reply: &str) -> bool {
    matches!(
        reply_code(reply),
        Some(425 | 450 | 500 | 501 | 502 | 504 | 520..=529 | 550)
    )
}

/// Opens a data channel: passive first, active once passive was refused or
/// unreachable. `active` is shared by every connection of one transfer.
fn open_data<'a>(
    ctrl: &mut Conn<'a>,
    host: &str,
    control: &'a FtpControl<'a>,
    active: &std::sync::atomic::AtomicBool,
) -> Result<DataChannel<'a>, String> {
    use std::sync::atomic::Ordering;
    if !active.load(Ordering::SeqCst) {
        match open_passive_data(ctrl, host, control) {
            Ok(socket) => return Ok(DataChannel::Passive(socket)),
            Err(PassiveError::Fatal(error)) => return Err(error),
            Err(PassiveError::Refused(reason)) if control.proxy.is_some() => {
                return Err(format!("FTP 代理传输需要被动模式：{reason}"));
            }
            Err(PassiveError::Refused(reason)) => {
                active.store(true, Ordering::SeqCst);
                return open_active_data(ctrl, control)
                    .map_err(|error| format!("{reason}; active mode failed: {error}"));
            }
        }
    }
    open_active_data(ctrl, control)
}

/// Negotiates a passive data connection (EPSV, PASV only as the IPv4
/// fallback for an unsupported EPSV) and connects to the control peer.
fn open_passive_data<'a>(
    ctrl: &mut Conn<'a>,
    host: &str,
    control: &'a FtpControl<'a>,
) -> Result<FtpSocket<'a>, PassiveError> {
    let peer = ctrl.peer_addr()?;
    let epsv = command(ctrl, "EPSV")?;
    // Only an unsupported command on IPv4 authorizes PASV. A malformed
    // success or a transient/permission error must not start another path.
    let pasv_allowed = if control.proxy.is_some() {
        host.parse::<std::net::Ipv6Addr>().is_err()
    } else {
        peer.is_ipv4()
    };
    let data_port = if pasv_allowed && matches!(reply_code(&epsv), Some(500 | 501 | 502 | 504)) {
        let pasv = command(ctrl, "PASV")?;
        if passive_refused(&pasv) {
            return Err(PassiveError::Refused(format!(
                "PASV refused: {}",
                pasv.trim()
            )));
        }
        require_reply_code(&pasv, &[227], "PASV")?;
        parse_pasv_port(&pasv)?
    } else {
        if passive_refused(&epsv) {
            return Err(PassiveError::Refused(format!(
                "EPSV refused: {}",
                epsv.trim()
            )));
        }
        require_reply_code(&epsv, &[229], "EPSV")?;
        parse_epsv_port(&epsv)?
    };
    // Both passive modes use the control peer, never a server-advertised host.
    let data_addr = std::net::SocketAddr::new(peer.ip(), data_port);
    control.check()?;
    let data_raw = match control.proxy {
        Some(proxy) => proxy.connect_blocking(host, data_port, || control.check())?,
        None => TcpStream::connect_timeout(&data_addr, DATA_CONNECT_TIMEOUT)
            .map_err(|error| PassiveError::Refused(format!("passive connect failed: {error}")))?,
    };
    control.check()?;
    Ok(FtpSocket::new(data_raw, control)?)
}

/// Listens on the control connection's local address and announces it with
/// EPRT (PORT for IPv4 servers that do not implement EPRT).
fn open_active_data<'a>(
    ctrl: &mut Conn<'a>,
    control: &'a FtpControl<'a>,
) -> Result<DataChannel<'a>, String> {
    control.check()?;
    let local = ctrl.local_addr()?;
    let server_ip = ctrl.peer_addr()?.ip();
    let listener = std::net::TcpListener::bind((local.ip(), 0))
        .map_err(|error| format!("FTP active mode listen failed: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let (family, ip) = match local.ip() {
        std::net::IpAddr::V4(ip) => (1, ip.to_string()),
        std::net::IpAddr::V6(ip) => (2, ip.to_string()),
    };
    let eprt = command(ctrl, &format!("EPRT |{family}|{ip}|{port}|"))?;
    match (reply_code(&eprt), local.ip()) {
        (Some(200), _) => {}
        (Some(500 | 502), std::net::IpAddr::V4(ip)) => {
            let [a, b, c, d] = ip.octets();
            let reply = command(
                ctrl,
                &format!("PORT {a},{b},{c},{d},{},{}", port >> 8, port & 0xff),
            )?;
            require_reply_code(&reply, &[200], "PORT")?;
        }
        _ => require_reply_code(&eprt, &[200], "EPRT")?,
    }
    Ok(DataChannel::Active {
        listener,
        server_ip,
        control,
    })
}

fn transfer_ftp(
    target: &FtpTarget,
    output: &Path,
    control: &FtpControl<'_>,
    resume: bool,
    connections: usize,
    active: &std::sync::atomic::AtomicBool,
) -> Result<u64, String> {
    let progress = output.with_extension("progress.json");
    let mut ctrl = open_session(target, control)?;
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
    // 多连接：FTP/FTPS、已知 SIZE+MDTM（资源身份可校验）、文件足够大且服务器在 FEAT 中声明
    // REST STREAM 时，按 REST 偏移分段并行；否则保持原单连接路径。FTPS 的每个辅助连接各自完成
    // 控制通道 TLS（独立凭据），数据通道复用本连接的会话。
    if segmented_eligible(connections, remote_size, state.is_some()) {
        if let Some(state) = state.clone() {
            if server_supports_rest_stream(&mut ctrl)? {
                return transfer_segmented(
                    target,
                    output,
                    ctrl,
                    state,
                    connections,
                    active,
                    resume,
                );
            }
        }
    }
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
    let data_channel = open_data(&mut ctrl, &target.host, control, active)?;
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
    // 主动模式下服务器在 RETR 之后才连入；FTPS 数据 TLS 两种模式都由客户端发起。
    let data_raw = data_channel.establish()?;
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
    // 单连接写入后，旧的分段完成表不再描述该文件。
    let _ = std::fs::remove_file(output.with_extension(SEGMENT_STATE_EXTENSION));
    // 数据留在调用线程，直接沿用任务/队列/全局限速上下文。
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = resume_from;
    let mut rate = crate::http_engine::TransferRate::new(downloaded);
    let mut last_progress = Instant::now();
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
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            let speed = rate.sample(downloaded) as f64;
            crate::http_engine::write_progress(&progress, downloaded, size, speed, "downloading");
            last_progress = Instant::now();
        }
    }
    if remote_size.is_some_and(|size| downloaded != size) {
        return Err(format!(
            "FTP transfer size mismatch: expected {size}, received {downloaded}"
        ));
    }
    let final_speed = rate.sample(downloaded) as f64;
    crate::http_engine::write_progress(&progress, downloaded, size, final_speed, "downloading");
    drop(data);
    file.sync_all().map_err(|error| error.to_string())?;
    control.check()?;
    let reply = read_reply(&mut ctrl)?;
    require_reply_code(&reply, &[226, 250], "transfer completion")?;
    control.poll()?;
    let _ = std::fs::remove_file(state_path);
    Ok(downloaded)
}

// ---------------------------------------------------------------------------
// Multi-connection (segmented) transfer
// ---------------------------------------------------------------------------

/// Files smaller than this always use one connection.
const SEGMENTED_MIN_BYTES: u64 = 4 * 1024 * 1024;
const SEGMENT_MIN_BYTES: u64 = 1024 * 1024;
const SEGMENT_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Upper bound of parallel FTP connections per task.
pub(crate) const MAX_FTP_CONNECTIONS: usize = 8;
const SEGMENT_STATE_EXTENSION: &str = "ftp-ranges.json";
/// Internal marker: the server rejected REST on the first segment.
const SEGMENTED_FALLBACK: &str = "ftp segmented transfer unavailable: REST rejected";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// Completed byte ranges of a segmented transfer, bound to the remote
/// resource identity (host/port/user/path/size/MDTM).
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
struct SegmentState {
    version: u32,
    resource: ResumeState,
    /// Inclusive `[start, end]` ranges that were synced to disk before they
    /// were recorded here.
    done: Vec<(u64, u64)>,
}

/// Whether a transfer may try the segmented path (REST STREAM is checked
/// afterwards on the live connection). Applies to FTP and FTPS alike.
fn segmented_eligible(connections: usize, remote_size: Option<u64>, has_identity: bool) -> bool {
    connections > 1 && has_identity && remote_size.is_some_and(|size| size >= SEGMENTED_MIN_BYTES)
}

fn server_supports_rest_stream(ctrl: &mut Conn<'_>) -> Result<bool, String> {
    let reply = command(ctrl, "FEAT")?;
    Ok(reply_code(&reply) == Some(211)
        && reply
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case("REST STREAM")))
}

fn normalize_segments(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.retain(|(start, end)| end >= start);
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut() {
            if start <= last.1.saturating_add(1) {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

/// Splits `[0, size)` minus `done` into segments of at most `segment` bytes.
fn pending_segments(size: u64, done: &[(u64, u64)], segment: u64) -> Vec<(u64, u64)> {
    let mut gaps = Vec::new();
    let mut cursor = 0u64;
    for &(start, end) in done {
        if start > cursor {
            gaps.push((cursor, start - 1));
        }
        cursor = cursor.max(end.saturating_add(1));
    }
    if cursor < size {
        gaps.push((cursor, size - 1));
    }
    let segment = segment.max(1);
    let mut out = Vec::new();
    for (start, end) in gaps {
        let mut at = start;
        while at <= end {
            let last = at.saturating_add(segment - 1).min(end);
            out.push((at, last));
            if last == u64::MAX {
                break;
            }
            at = last + 1;
        }
    }
    out
}

fn segment_length(size: u64, connections: usize) -> u64 {
    (size / (connections.max(1) as u64 * 4)).clamp(SEGMENT_MIN_BYTES, SEGMENT_MAX_BYTES)
}

struct SegmentShared<'p> {
    queue: std::sync::Mutex<std::collections::VecDeque<(u64, u64)>>,
    in_flight: std::sync::atomic::AtomicUsize,
    done: std::sync::Mutex<Vec<(u64, u64)>>,
    downloaded: std::sync::atomic::AtomicU64,
    stop: std::sync::atomic::AtomicBool,
    fatal: std::sync::Mutex<Option<String>>,
    progress_at: std::sync::Mutex<Instant>,
    size: u64,
    output: &'p Path,
    progress: std::path::PathBuf,
    state_path: std::path::PathBuf,
    resource: ResumeState,
    /// Data connection mode shared with the primary (see [`open_data`]).
    active: &'p std::sync::atomic::AtomicBool,
}

impl SegmentShared<'_> {
    fn claim(&self) -> Option<Option<(u64, u64)>> {
        use std::sync::atomic::Ordering;
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(range) = queue.pop_front() {
            self.in_flight.fetch_add(1, Ordering::SeqCst);
            return Some(Some(range));
        }
        if self.in_flight.load(Ordering::SeqCst) == 0 {
            return None;
        }
        // Another connection still owns a segment that may come back.
        Some(None)
    }

    fn release(&self, remaining: Option<(u64, u64)>) {
        use std::sync::atomic::Ordering;
        if let Some(range) = remaining {
            self.queue
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push_front(range);
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    /// Records a synced range and rewrites the completion map atomically.
    fn record(&self, start: u64, end: u64) -> Result<(), String> {
        let mut done = self.done.lock().unwrap_or_else(|error| error.into_inner());
        let mut next = done.clone();
        next.push((start, end));
        let next = normalize_segments(next);
        write_segment_state(
            &self.state_path,
            &SegmentState {
                version: 1,
                resource: self.resource.clone(),
                done: next.clone(),
            },
        )?;
        *done = next;
        Ok(())
    }

    fn add_progress(&self, count: u64) {
        use std::sync::atomic::Ordering;
        let total = self.downloaded.fetch_add(count, Ordering::SeqCst) + count;
        let mut at = self
            .progress_at
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if at.elapsed() >= PROGRESS_INTERVAL {
            *at = Instant::now();
            drop(at);
            crate::http_engine::write_progress(
                &self.progress,
                total,
                self.size,
                0.0,
                "downloading",
            );
        }
    }
}

fn write_segment_state(path: &Path, state: &SegmentState) -> Result<(), String> {
    let bytes = serde_json::to_vec(state).map_err(|error| error.to_string())?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    {
        let mut file = std::fs::File::create(&tmp).map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    crate::atomic_replace::replace_file(&tmp, path).map_err(|error| error.to_string())
}

/// After closing a data connection before the server finished sending,
/// drain its transfer reply (226, 426 or 451, sometimes two) by sending NOOP
/// and reading until the NOOP's 200 arrives.
fn resync_after_abort(ctrl: &mut Conn<'_>) -> Result<(), String> {
    ctrl.write_all(b"NOOP\r\n")
        .map_err(|error| error.to_string())?;
    for _ in 0..4 {
        let reply = read_reply(ctrl)?;
        match reply_code(&reply) {
            Some(200) => return Ok(()),
            Some(226 | 250 | 425 | 426 | 450 | 451) => continue,
            _ => return Err(format!("FTP control resync failed: {}", reply.trim())),
        }
    }
    Err("FTP control resync failed".into())
}

#[derive(Debug, PartialEq)]
enum SegmentError {
    RestRejected,
    /// The FTPS data channel of a segment could not be secured (for example a
    /// server that demands TLS session reuse the client cannot provide).
    DataTls(String),
    Failed(String),
}

/// What the primary connection's outcome means for the whole transfer.
/// `Err(SEGMENTED_FALLBACK)` asks the caller to retry with one connection.
fn segmented_outcome(
    primary: Result<(), SegmentError>,
    done: &[(u64, u64)],
    started_this_run: bool,
) -> Result<(), String> {
    let nothing_recorded = done.is_empty() || done.iter().all(|(start, end)| start > end);
    match primary {
        Ok(()) => Ok(()),
        Err(SegmentError::RestRejected) if nothing_recorded => Err(SEGMENTED_FALLBACK.into()),
        Err(SegmentError::RestRejected) => {
            Err("FTP server rejected REST for a segmented resume".into())
        }
        // 本次运行的首个分段就无法建立数据通道 TLS（如服务器要求的会话复用握手被拒）：辅助连接
        // 尚未启动，回落单连接从头下载（已登记的分段表不再适用，否则续传会反复卡在同一处）。
        Err(SegmentError::DataTls(_)) if !started_this_run => Err(SEGMENTED_FALLBACK.into()),
        // 已有分段成功后才失败：按普通传输错误处理，重试时从已登记范围续传。
        Err(SegmentError::DataTls(error)) => Err(format!("FTPS data TLS failed: {error}")),
        Err(SegmentError::Failed(error)) => Err(error),
    }
}

impl From<String> for SegmentError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

/// Downloads `[start, end]` over one data connection of `ctrl`. Returns the
/// first byte not yet written (so a partial segment can still be recorded).
#[allow(clippy::too_many_arguments)]
fn fetch_segment<'a>(
    ctrl: &mut Conn<'a>,
    target: &FtpTarget,
    control: &'a FtpControl<'a>,
    file: &mut std::fs::File,
    shared: &SegmentShared<'_>,
    start: u64,
    end: u64,
    on_started: &mut dyn FnMut(),
) -> (u64, Result<(), SegmentError>) {
    use std::io::{Seek, SeekFrom};
    use std::sync::atomic::Ordering;
    let mut cursor = start;
    let result = (|| -> Result<(), SegmentError> {
        let data_channel = open_data(ctrl, &target.host, control, shared.active)?;
        // REST 必须紧挨 RETR；被拒绝即视为服务器不支持分段。
        let reply = command(ctrl, &format!("REST {start}"))?;
        if reply_code(&reply) != Some(350) {
            drop(data_channel);
            return Err(SegmentError::RestRejected);
        }
        let reply = command(ctrl, &format!("RETR {}", target.path))?;
        require_reply_code(&reply, &[125, 150], "RETR")?;
        let data_raw = data_channel.establish()?;
        // FTPS：服务器在 RETR 之后才启动数据通道 TLS；数据连接复用本控制连接的凭据/会话。
        // 握手成功后才算分段已启动，主连接据此决定是否开辅助连接。
        let mut data = if target.tls {
            ctrl.secure_data(data_raw, &target.host)
                .map_err(SegmentError::DataTls)?
        } else {
            Conn::Plain(data_raw)
        };
        on_started();
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        let to_eof = end + 1 == shared.size;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            control.check()?;
            if shared.stop.load(Ordering::SeqCst) {
                return Err(SegmentError::Failed("ftp segment stopped".into()));
            }
            let remaining = (end + 1).saturating_sub(cursor);
            if remaining == 0 && !to_eof {
                break;
            }
            // 末段多读一个字节确认 SIZE 边界；超出的数据在落盘前拒绝。
            let limit = (remaining + u64::from(to_eof)).min(buf.len() as u64) as usize;
            let count = data
                .read(&mut buf[..limit])
                .map_err(|error| error.to_string())?;
            control.check()?;
            if count == 0 {
                if to_eof && cursor == shared.size {
                    break;
                }
                return Err(SegmentError::Failed(format!(
                    "FTP segment truncated: expected bytes {start}-{end}, received up to {cursor}"
                )));
            }
            if cursor + count as u64 > end + 1 {
                let message = format!(
                    "FTP transfer exceeds declared size: expected {}, received at least {}",
                    shared.size,
                    cursor + count as u64
                );
                let mut fatal = shared
                    .fatal
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if fatal.is_none() {
                    *fatal = Some(message.clone());
                }
                shared.stop.store(true, Ordering::SeqCst);
                return Err(SegmentError::Failed(message));
            }
            crate::net_policy::consume(count, || control.check())?;
            control.check()?;
            file.write_all(&buf[..count])
                .map_err(|error| error.to_string())?;
            cursor += count as u64;
            shared.add_progress(count as u64);
        }
        drop(data);
        if to_eof {
            let reply = read_reply(ctrl)?;
            require_reply_code(&reply, &[226, 250], "transfer completion")?;
        } else {
            resync_after_abort(ctrl)?;
        }
        Ok(())
    })();
    (cursor, result)
}

/// One connection's loop: claim a segment, transfer it, sync and record what
/// was written (also on pause/error), put any unfinished tail back.
fn run_segment_worker<'a>(
    ctrl: &mut Conn<'a>,
    target: &FtpTarget,
    control: &'a FtpControl<'a>,
    shared: &SegmentShared<'_>,
    on_started: &mut dyn FnMut(),
) -> Result<(), SegmentError> {
    use std::sync::atomic::Ordering;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(shared.output)
        .map_err(|error| error.to_string())?;
    loop {
        control.check()?;
        if shared.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let (start, end) = match shared.claim() {
            None => return Ok(()),
            Some(None) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Some(Some(range)) => range,
        };
        let (cursor, result) = fetch_segment(
            ctrl, target, control, &mut file, shared, start, end, on_started,
        );
        let to_eof = end + 1 == shared.size;
        let has_fatal = shared
            .fatal
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some();
        // 末段只有在收到完整 EOF 与 226/250 确认后才登记最后 1 字节，避免超长或异常中断被误记为已完成。
        let effective_cursor = if has_fatal {
            start
        } else if to_eof && result.is_err() && cursor > end {
            end
        } else {
            cursor
        };
        // 先同步数据再登记完成表，与 HTTP 检查点同一语义；暂停/失败时登记已写部分。
        let recorded = if effective_cursor > start {
            file.sync_data()
                .map_err(|error| error.to_string())
                .and_then(|_| shared.record(start, effective_cursor - 1))
        } else {
            Ok(())
        };
        let remaining = (effective_cursor <= end).then_some((effective_cursor, end));
        if let Err(error) = recorded {
            // 未登记的字节也要重下：整段放回。
            shared.release(Some((start, end)));
            return Err(SegmentError::Failed(error));
        }
        match result {
            Ok(()) => shared.release(remaining),
            Err(error) => {
                shared.release(remaining);
                return Err(error);
            }
        }
    }
}

/// Parallel REST-segmented download. The primary connection (already logged
/// in) runs on the calling thread; helper connections are opened only after
/// the primary's first `REST`+`RETR` was accepted, so a server that rejects
/// REST falls back to the single-connection transfer before anything else
/// connects. Helpers that cannot connect or fail leave their segment to the
/// remaining connections; the primary's error (pause/cancel/failure) ends the
/// transfer with every written range synced and recorded for resume.
fn transfer_segmented<'a>(
    target: &FtpTarget,
    output: &Path,
    mut ctrl: Conn<'a>,
    resource: ResumeState,
    connections: usize,
    active: &std::sync::atomic::AtomicBool,
    resume: bool,
) -> Result<u64, String> {
    use std::sync::atomic::Ordering;
    let control = ctrl.ftp_socket().control;
    let size = resource.total;
    let connections = connections.clamp(1, MAX_FTP_CONNECTIONS);
    let state_path = output.with_extension(SEGMENT_STATE_EXTENSION);
    let single_state = output.with_extension("ftp-state.json");
    let saved = std::fs::read(&state_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SegmentState>(&bytes).ok())
        .filter(|_| resume)
        .filter(|saved| saved.version == 1 && saved.resource == resource)
        .filter(|_| std::fs::metadata(output).is_ok_and(|meta| meta.len() == size));
    let done = match saved {
        Some(saved) => normalize_segments(saved.done)
            .into_iter()
            .filter(|(_, end)| *end < size)
            .collect::<Vec<_>>(),
        None => {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            control.check()?;
            let file = std::fs::File::create(output).map_err(|error| error.to_string())?;
            file.set_len(size).map_err(|error| error.to_string())?;
            drop(file);
            // 单连接续传状态描述的是“追加到文件末尾”；预分配后的文件不能再按它续传。
            let _ = std::fs::remove_file(&single_state);
            write_segment_state(
                &state_path,
                &SegmentState {
                    version: 1,
                    resource: resource.clone(),
                    done: Vec::new(),
                },
            )?;
            Vec::new()
        }
    };
    let already: u64 = done.iter().map(|(start, end)| end - start + 1).sum();
    let pending = pending_segments(size, &done, segment_length(size, connections));
    let shared = SegmentShared {
        queue: std::sync::Mutex::new(pending.into()),
        in_flight: std::sync::atomic::AtomicUsize::new(0),
        done: std::sync::Mutex::new(done),
        downloaded: std::sync::atomic::AtomicU64::new(already),
        stop: std::sync::atomic::AtomicBool::new(false),
        fatal: std::sync::Mutex::new(None),
        progress_at: std::sync::Mutex::new(Instant::now()),
        size,
        output,
        progress: output.with_extension("progress.json"),
        state_path: state_path.clone(),
        resource,
        active,
    };
    crate::http_engine::write_progress(&shared.progress, already, size, 0.0, "downloading");
    let throttle = crate::net_policy::current_throttle_context();
    let control_path = control.path;
    let proxy = control.proxy;
    let started_this_run = std::cell::Cell::new(false);
    let primary = std::thread::scope(|scope| {
        let mut spawned = false;
        let shared = &shared;
        let started_this_run = &started_this_run;
        let mut spawn_helpers = || {
            if spawned {
                return;
            }
            spawned = true;
            started_this_run.set(true);
            for _ in 1..connections {
                let throttle = throttle.clone();
                scope.spawn(move || {
                    crate::net_policy::with_throttle_context(throttle, || {
                        let helper_control = FtpControl {
                            proxy,
                            path: control_path,
                            reason: Cell::new(crate::http_engine::Control::Run),
                            checked: Cell::new(Instant::now()),
                        };
                        if helper_control.poll().is_err() {
                            return;
                        }
                        // 连接数受限（421 等）或中途失败的辅助连接直接退出，分段由其余连接接手。
                        let Ok(mut helper) = open_session(target, &helper_control) else {
                            return;
                        };
                        let _ = run_segment_worker(
                            &mut helper,
                            target,
                            &helper_control,
                            shared,
                            &mut || {},
                        );
                    });
                });
            }
        };
        let result = run_segment_worker(&mut ctrl, target, control, shared, &mut spawn_helpers);
        if result.is_err() {
            shared.stop.store(true, Ordering::SeqCst);
        }
        result
    });
    let fatal = shared
        .fatal
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Some(error) = fatal {
        return Err(error);
    }
    let done = shared
        .done
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Err(error) = segmented_outcome(primary, &done, started_this_run.get()) {
        if error == SEGMENTED_FALLBACK {
            let _ = std::fs::remove_file(&state_path);
        }
        return Err(error);
    }
    control.check()?;
    if done != [(0, size - 1)] {
        return Err(format!(
            "FTP segmented transfer incomplete: {} of {size} bytes recorded",
            done.iter().map(|(start, end)| end - start + 1).sum::<u64>()
        ));
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(output)
        .map_err(|error| error.to_string())?;
    let actual = file.metadata().map_err(|error| error.to_string())?.len();
    if actual != size {
        return Err(format!(
            "FTP transfer size mismatch: expected {size}, received {actual}"
        ));
    }
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    crate::http_engine::write_progress(&shared.progress, size, size, 0.0, "downloading");
    let _ = std::fs::remove_file(&state_path);
    let _ = std::fs::remove_file(&single_state);
    Ok(size)
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
    fn ftp_path_preserves_embedded_scheme_separator() {
        assert_eq!(
            parse_ftp_url("ftp://files.test/pub/http://archive/item")
                .unwrap()
                .path,
            "/pub/http://archive/item"
        );
    }

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
                        // Active fallback is attempted but this server implements neither.
                        _ if command.starts_with("EPRT ") || command.starts_with("PORT ") => {
                            ctrl.write_all(b"500 Unknown command\r\n").unwrap()
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
            // Only a refused passive mode (not malformed or auth replies) goes active.
            assert_eq!(
                commands.iter().any(|item| item.starts_with("EPRT ")),
                matches!(
                    case,
                    "ipv6-no-pasv" | "epsv-unavailable" | "pasv-unavailable"
                ),
                "{case}: {commands:?}"
            );
            assert_eq!(
                commands.iter().any(|item| item.starts_with("PORT ")),
                matches!(case, "epsv-unavailable" | "pasv-unavailable"),
                "{case}: {commands:?}"
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

    // ---------------------------------------------------------------------
    // Segmented (multi-connection) transfer
    // ---------------------------------------------------------------------

    #[derive(Clone, Copy)]
    struct SegmentServerOptions {
        advertise_rest: bool,
        rest_ok: bool,
        /// Connections beyond this receive `421` at greeting.
        max_connections: usize,
        chunk_delay_ms: u64,
        /// Refuse EPSV/PASV so the client must use active mode (EPRT).
        passive_refused: bool,
    }

    struct SegmentServer {
        address: std::net::SocketAddr,
        log: std::sync::Arc<std::sync::Mutex<Vec<(usize, String)>>>,
        connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl SegmentServer {
        fn commands(&self, prefix: &str) -> Vec<(usize, String)> {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, line)| line.starts_with(prefix))
                .cloned()
                .collect()
        }

        fn clear(&self) {
            self.log.lock().unwrap().clear();
        }
    }

    impl Drop for SegmentServer {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = std::net::TcpStream::connect(self.address);
        }
    }

    fn segment_body(len: usize) -> std::sync::Arc<Vec<u8>> {
        std::sync::Arc::new(
            (0..len)
                .map(|index| (index.wrapping_mul(31) ^ (index >> 11)) as u8)
                .collect(),
        )
    }

    fn spawn_segment_server(
        body: std::sync::Arc<Vec<u8>>,
        options: SegmentServerOptions,
    ) -> SegmentServer {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        use std::sync::atomic::Ordering;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let log = std::sync::Arc::clone(&log);
            let connections = std::sync::Arc::clone(&connections);
            let stop = std::sync::Arc::clone(&stop);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut ctrl) = stream else { continue };
                    let index = connections.fetch_add(1, Ordering::SeqCst);
                    let log = std::sync::Arc::clone(&log);
                    let body = std::sync::Arc::clone(&body);
                    std::thread::spawn(move || {
                        ctrl.set_read_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        if index >= options.max_connections {
                            let _ = ctrl.write_all(b"421 Too many connections\r\n");
                            return;
                        }
                        ctrl.write_all(b"220 Ready\r\n").unwrap();
                        let mut reader = BufReader::new(ctrl.try_clone().unwrap());
                        let mut data_listener: Option<TcpListener> = None;
                        let mut active_target: Option<std::net::SocketAddr> = None;
                        let mut offset = 0usize;
                        loop {
                            let mut line = String::new();
                            match reader.read_line(&mut line) {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                            let command = line.trim_end().to_string();
                            log.lock().unwrap().push((index, command.clone()));
                            let reply: String = if command == "USER anonymous" {
                                "230 Logged in\r\n".into()
                            } else if command == "TYPE I" {
                                "200 Binary\r\n".into()
                            } else if command == "SIZE /big.bin" {
                                format!("213 {}\r\n", body.len())
                            } else if command == "MDTM /big.bin" {
                                "213 20261115000000\r\n".into()
                            } else if command == "FEAT" {
                                if options.advertise_rest {
                                    "211-Features:\r\n SIZE\r\n MDTM\r\n REST STREAM\r\n211 End\r\n"
                                        .into()
                                } else {
                                    "211-Features:\r\n SIZE\r\n MDTM\r\n211 End\r\n".into()
                                }
                            } else if command == "EPSV" || command == "PASV" {
                                if options.passive_refused {
                                    "502 Passive mode disabled\r\n".into()
                                } else {
                                    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                                    let port = listener.local_addr().unwrap().port();
                                    data_listener = Some(listener);
                                    format!("229 Passive (|||{port}|)\r\n")
                                }
                            } else if let Some(spec) = command.strip_prefix("EPRT ") {
                                active_target = Some(parse_test_eprt(spec));
                                "200 EPRT ok\r\n".into()
                            } else if let Some(value) = command.strip_prefix("REST ") {
                                if options.rest_ok {
                                    offset = value.parse().unwrap();
                                    "350 Restarting\r\n".into()
                                } else {
                                    "502 Not implemented\r\n".into()
                                }
                            } else if command == "RETR /big.bin" {
                                ctrl.write_all(b"150 Opening data\r\n").unwrap();
                                let mut data = match active_target.take() {
                                    Some(target) => std::net::TcpStream::connect(target).unwrap(),
                                    None => data_listener.take().unwrap().accept().unwrap().0,
                                };
                                let mut failed = false;
                                for chunk in body[offset.min(body.len())..].chunks(64 * 1024) {
                                    if options.chunk_delay_ms > 0 {
                                        std::thread::sleep(Duration::from_millis(
                                            options.chunk_delay_ms,
                                        ));
                                    }
                                    if data.write_all(chunk).is_err() {
                                        failed = true;
                                        break;
                                    }
                                }
                                drop(data);
                                offset = 0;
                                if failed {
                                    "426 Connection closed; transfer aborted\r\n".into()
                                } else {
                                    "226 Complete\r\n".into()
                                }
                            } else if command == "NOOP" {
                                "200 OK\r\n".into()
                            } else {
                                "500 Unknown\r\n".into()
                            };
                            if ctrl.write_all(reply.as_bytes()).is_err() {
                                break;
                            }
                        }
                    });
                }
            });
        }
        SegmentServer {
            address,
            log,
            connections,
            stop,
        }
    }

    fn segment_root(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "hls-ftp-segmented-{name}-{}-{}",
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
        (root, output, control)
    }

    const SEGMENT_TEST_BYTES: usize = 5 * 1024 * 1024 + 4321;

    #[test]
    fn authenticated_proxy_routes_every_segment_and_refuses_active_mode() {
        use crate::proxy_tunnel::tests::TestProxy;
        for (scheme, passive_refused) in [("http", false), ("socks5h", false), ("http", true)] {
            let body = segment_body(SEGMENT_TEST_BYTES);
            let server = spawn_segment_server(
                body.clone(),
                SegmentServerOptions {
                    advertise_rest: true,
                    rest_ok: true,
                    max_connections: 4,
                    chunk_delay_ms: 1,
                    passive_refused,
                },
            );
            let proxy = TestProxy::start(scheme);
            let (root, output, control) = segment_root("proxy");
            let port = server.address.port();
            let url = format!("ftp://origin.invalid:{port}/big.bin");
            let result = download_ftp_routed(&url, &output, &control, true, 3, &proxy.url);
            let ports = proxy.ports.lock().unwrap();
            if passive_refused {
                assert!(result.unwrap_err().contains("被动模式"));
                assert!(server.commands("PORT").is_empty() && server.commands("EPRT").is_empty());
                assert_eq!(ports.as_slice(), &[port]);
            } else {
                assert_eq!(result.unwrap(), body.len() as u64);
                assert_eq!(std::fs::read(&output).unwrap(), *body);
                assert!(ports.iter().filter(|p| **p == port).count() > 1);
                assert_eq!(
                    ports.iter().filter(|p| **p != port).count(),
                    server.commands("EPSV").len()
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// Parses `|1|127.0.0.1|4242|` (EPRT argument) in tests.
    fn parse_test_eprt(spec: &str) -> std::net::SocketAddr {
        let parts: Vec<&str> = spec.split('|').collect();
        assert_eq!(parts.len(), 5, "EPRT {spec}");
        assert!(parts[1] == "1" || parts[1] == "2", "EPRT {spec}");
        std::net::SocketAddr::new(parts[2].parse().unwrap(), parts[3].parse().unwrap())
    }

    /// Parses `127,0,0,1,16,146` (PORT argument) in tests.
    fn parse_test_port(spec: &str) -> std::net::SocketAddr {
        let parts: Vec<u16> = spec.split(',').map(|part| part.parse().unwrap()).collect();
        assert_eq!(parts.len(), 6, "PORT {spec}");
        let ip = std::net::Ipv4Addr::new(
            parts[0] as u8,
            parts[1] as u8,
            parts[2] as u8,
            parts[3] as u8,
        );
        std::net::SocketAddr::new(ip.into(), parts[4] * 256 + parts[5])
    }

    #[test]
    fn refused_or_unreachable_passive_falls_back_to_active_mode() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        // (case, EPSV reply, PASV reply, EPRT accepted, resume offset)
        for (case, epsv_reply, pasv_reply, eprt_ok, resume) in [
            (
                "eprt",
                "502 Not implemented",
                "502 Not implemented",
                true,
                false,
            ),
            (
                "port",
                "502 Not implemented",
                "425 Passive refused",
                false,
                false,
            ),
            (
                "unreachable-pasv",
                "502 Not implemented",
                "227",
                true,
                false,
            ),
            ("eprt-resume", "425 Cannot open data", "", true, true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut ctrl, _) = listener.accept().unwrap();
                ctrl.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                ctrl.write_all(b"220 Ready\r\n").unwrap();
                let mut reader = BufReader::new(ctrl.try_clone().unwrap());
                let mut target = None;
                let mut offset = 0usize;
                let mut commands = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        break;
                    }
                    let command = line.trim_end().to_string();
                    commands.push(command.clone());
                    match command.as_str() {
                        "USER anonymous" => ctrl.write_all(b"230 Logged in\r\n").unwrap(),
                        "TYPE I" => ctrl.write_all(b"200 Binary\r\n").unwrap(),
                        "SIZE /file.bin" => ctrl.write_all(b"213 8\r\n").unwrap(),
                        "MDTM /file.bin" => ctrl.write_all(b"213 20261115000000\r\n").unwrap(),
                        "EPSV" => write!(ctrl, "{epsv_reply}\r\n").unwrap(),
                        "PASV" if pasv_reply == "227" => {
                            // Advertise a port nothing listens on.
                            let closed = TcpListener::bind("127.0.0.1:0").unwrap();
                            let port = closed.local_addr().unwrap().port();
                            drop(closed);
                            write!(
                                ctrl,
                                "227 Passive (127,0,0,1,{},{})\r\n",
                                port / 256,
                                port % 256
                            )
                            .unwrap();
                        }
                        "PASV" => write!(ctrl, "{pasv_reply}\r\n").unwrap(),
                        "RETR /file.bin" => {
                            ctrl.write_all(b"150 Opening data\r\n").unwrap();
                            let mut data = std::net::TcpStream::connect(
                                target.take().expect("active address announced"),
                            )
                            .unwrap();
                            data.write_all(&b"AAAABBBB"[offset..]).unwrap();
                            drop(data);
                            ctrl.write_all(b"226 Complete\r\n").unwrap();
                            break;
                        }
                        other => {
                            if let Some(spec) = other.strip_prefix("EPRT ") {
                                if eprt_ok {
                                    let addr = parse_test_eprt(spec);
                                    assert_eq!(addr.ip(), address.ip(), "{case}: control IP");
                                    target = Some(addr);
                                    ctrl.write_all(b"200 EPRT ok\r\n").unwrap();
                                } else {
                                    ctrl.write_all(b"502 EPRT not implemented\r\n").unwrap();
                                }
                            } else if let Some(spec) = other.strip_prefix("PORT ") {
                                target = Some(parse_test_port(spec));
                                ctrl.write_all(b"200 PORT ok\r\n").unwrap();
                            } else if let Some(value) = other.strip_prefix("REST ") {
                                offset = value.parse().unwrap();
                                ctrl.write_all(b"350 Restarting\r\n").unwrap();
                            } else {
                                panic!("{case}: unexpected command {other}");
                            }
                        }
                    }
                }
                commands
            });
            let root =
                std::env::temp_dir().join(format!("hls-ftp-active-{}-{case}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let output = root.join("payload.downloading");
            let control = root.join("control");
            std::fs::write(&control, "run").unwrap();
            if resume {
                std::fs::write(&output, b"AAAA").unwrap();
                let state = ResumeState {
                    version: 1,
                    host: "127.0.0.1".into(),
                    port: address.port(),
                    user: "anonymous".into(),
                    path: "/file.bin".into(),
                    tls: false,
                    total: 8,
                    modified: "20261115000000".into(),
                };
                std::fs::write(
                    output.with_extension("ftp-state.json"),
                    serde_json::to_vec(&state).unwrap(),
                )
                .unwrap();
            }
            let result = download_ftp(
                &format!("ftp://{address}/file.bin"),
                &output,
                &control,
                resume,
            );
            let commands = server.join().unwrap();
            let bytes = std::fs::read(&output).unwrap();
            std::fs::remove_dir_all(root).unwrap();
            assert_eq!(result, Ok(8), "{case}: {commands:?}");
            assert_eq!(bytes, b"AAAABBBB", "{case}");
            assert!(
                commands
                    .iter()
                    .any(|item| item.starts_with("EPRT |1|127.0.0.1|")),
                "{case}"
            );
            assert_eq!(
                commands
                    .iter()
                    .any(|item| item.starts_with("PORT 127,0,0,1,")),
                !eprt_ok,
                "{case}: {commands:?}"
            );
            if resume {
                // REST still sits directly before RETR in active mode.
                let rest = commands.iter().position(|item| item == "REST 4").unwrap();
                assert_eq!(commands[rest + 1], "RETR /file.bin", "{case}");
            }
        }
    }

    #[test]
    fn segmented_transfer_remembers_active_mode_for_helpers() {
        let body = segment_body(SEGMENT_TEST_BYTES);
        let server = spawn_segment_server(
            std::sync::Arc::clone(&body),
            SegmentServerOptions {
                advertise_rest: true,
                rest_ok: true,
                max_connections: 16,
                chunk_delay_ms: 2,
                passive_refused: true,
            },
        );
        let (root, output, control) = segment_root("active");
        let url = format!("ftp://{}/big.bin", server.address);
        let result = download_ftp_with(&url, &output, &control, true, 4);
        let bytes = std::fs::read(&output).unwrap();
        let has_ranges = output.with_extension(SEGMENT_STATE_EXTENSION).exists();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(result.unwrap(), body.len() as u64);
        assert!(
            bytes == *body,
            "segmented active output must equal the remote file"
        );
        assert!(!has_ranges);
        // Passive is probed once by the primary; every later data connection
        // (primary and helpers) goes straight to EPRT.
        let epsv = server.commands("EPSV");
        assert_eq!(epsv, vec![(0, "EPSV".to_string())], "{epsv:?}");
        assert_eq!(
            server.commands("EPRT ").len(),
            server.commands("RETR").len()
        );
        let used: std::collections::BTreeSet<usize> = server
            .commands("RETR")
            .iter()
            .map(|(conn, _)| *conn)
            .collect();
        assert!(
            used.len() >= 2,
            "helpers transferred data in active mode: {used:?}"
        );
    }

    #[test]
    fn segment_planning_skips_done_ranges_and_bounds_segment_size() {
        assert_eq!(
            normalize_segments(vec![(10, 19), (0, 9), (30, 39), (25, 31), (50, 40)]),
            vec![(0, 19), (25, 39)]
        );
        assert_eq!(
            pending_segments(50, &[(0, 19), (25, 39)], 4),
            vec![(20, 23), (24, 24), (40, 43), (44, 47), (48, 49)]
        );
        assert_eq!(pending_segments(10, &[(0, 9)], 4), Vec::<(u64, u64)>::new());
        assert_eq!(segment_length(5 * 1024 * 1024, 4), SEGMENT_MIN_BYTES);
        assert_eq!(segment_length(4 << 30, 2), SEGMENT_MAX_BYTES);
    }

    #[test]
    fn segmented_gating_is_the_same_for_ftp_and_ftps() {
        let size = Some(SEGMENTED_MIN_BYTES);
        assert!(segmented_eligible(4, size, true));
        assert!(segmented_eligible(MAX_FTP_CONNECTIONS + 4, size, true));
        assert!(
            !segmented_eligible(1, size, true),
            "one connection stays single"
        );
        assert!(
            !segmented_eligible(4, Some(SEGMENTED_MIN_BYTES - 1), true),
            "small file"
        );
        assert!(!segmented_eligible(4, None, true), "no SIZE");
        assert!(!segmented_eligible(4, size, false), "no SIZE+MDTM identity");
        // TLS is no longer a gate: an FTPS target with the same facts is eligible.
        let target = parse_ftp_url("ftps://files.example/pub/a.bin").unwrap();
        assert!(target.tls);
        assert!(segmented_eligible(4, size, true));
        // The segmented completion map binds the TLS flag into the identity,
        // so FTP and FTPS checkpoints of the same path never mix.
        let identity = |tls| ResumeState {
            version: 1,
            host: "files.example".into(),
            port: 21,
            user: "anonymous".into(),
            path: "/pub/a.bin".into(),
            tls,
            total: SEGMENTED_MIN_BYTES,
            modified: "20260101000000".into(),
        };
        assert_ne!(identity(true), identity(false));
    }

    #[test]
    fn ftps_data_tls_failure_falls_back_only_before_any_segment_started() {
        let tls = || Err(SegmentError::DataTls("session reuse required".into()));
        // First segment of this run could not secure its data channel: retry
        // with one connection (also when an older run left ranges behind).
        assert_eq!(
            segmented_outcome(tls(), &[], false),
            Err(SEGMENTED_FALLBACK.into())
        );
        assert_eq!(
            segmented_outcome(tls(), &[(0, 1023)], false),
            Err(SEGMENTED_FALLBACK.into())
        );
        // After segments succeeded it is an ordinary, resumable failure.
        let failed = segmented_outcome(tls(), &[(0, 1023)], true).unwrap_err();
        assert_ne!(failed, SEGMENTED_FALLBACK);
        assert!(failed.contains("FTPS data TLS failed"));
        // REST handling is unchanged.
        assert_eq!(
            segmented_outcome(Err(SegmentError::RestRejected), &[], false),
            Err(SEGMENTED_FALLBACK.into())
        );
        assert!(
            segmented_outcome(Err(SegmentError::RestRejected), &[(0, 9)], false)
                .unwrap_err()
                .contains("rejected REST")
        );
        assert_eq!(
            segmented_outcome(Err(SegmentError::Failed("paused".into())), &[], true),
            Err("paused".into())
        );
        assert_eq!(segmented_outcome(Ok(()), &[], true), Ok(()));
    }

    #[cfg(not(windows))]
    #[test]
    fn ftps_without_schannel_reports_the_platform_instead_of_segmenting() {
        // FTPS uses Windows Schannel; elsewhere the control upgrade fails
        // before SIZE/MDTM, so the segmented path is never entered.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"220 ready\r\n").unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
            assert_eq!(line.trim(), "AUTH TLS");
            stream.write_all(b"234 go\r\n").unwrap();
        });
        let dir = std::env::temp_dir().join(format!(
            "hls-ftps-platform-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let control = dir.join("control");
        std::fs::write(&control, "run").unwrap();
        let error = download_ftp_with(
            &format!("ftps://127.0.0.1:{port}/a.bin"),
            &dir.join("a.bin"),
            &control,
            false,
            4,
        )
        .unwrap_err();
        assert!(error.contains("Schannel"), "{error}");
        assert!(!dir.join("a.ftp-ranges.json").exists());
        server.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fresh_segmented_transfer_ignores_a_matching_checkpoint() {
        let body = segment_body(SEGMENT_TEST_BYTES);
        let server = spawn_segment_server(
            std::sync::Arc::clone(&body),
            SegmentServerOptions {
                advertise_rest: true,
                rest_ok: true,
                max_connections: 16,
                chunk_delay_ms: 0,
                passive_refused: false,
            },
        );
        let (root, output, control) = segment_root("fresh");
        std::fs::write(&output, vec![0u8; body.len()]).unwrap();
        write_segment_state(
            &output.with_extension(SEGMENT_STATE_EXTENSION),
            &SegmentState {
                version: 1,
                resource: ResumeState {
                    version: 1,
                    host: "127.0.0.1".into(),
                    port: server.address.port(),
                    user: "anonymous".into(),
                    path: "/big.bin".into(),
                    tls: false,
                    total: body.len() as u64,
                    modified: "20261115000000".into(),
                },
                done: vec![(0, body.len() as u64 - 1)],
            },
        )
        .unwrap();
        let result = download_ftp_with(
            &format!("ftp://{}/big.bin", server.address),
            &output,
            &control,
            false,
            4,
        );
        let actual = std::fs::read(&output).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(result.unwrap(), body.len() as u64);
        assert_eq!(actual.as_slice(), body.as_slice());
        assert!(!server.commands("RETR").is_empty());
    }

    #[test]
    fn segmented_transfer_uses_parallel_rest_connections_and_cleans_up() {
        let body = segment_body(SEGMENT_TEST_BYTES);
        let server = spawn_segment_server(
            std::sync::Arc::clone(&body),
            SegmentServerOptions {
                advertise_rest: true,
                rest_ok: true,
                max_connections: 16,
                chunk_delay_ms: 2,
                passive_refused: false,
            },
        );
        let (root, output, control) = segment_root("parallel");
        let url = format!("ftp://{}/big.bin", server.address);
        let result = download_ftp_with(&url, &output, &control, true, 4);
        let bytes = std::fs::read(&output).unwrap();
        let has_ranges = output.with_extension(SEGMENT_STATE_EXTENSION).exists();
        let has_state = output.with_extension("ftp-state.json").exists();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(result.unwrap(), body.len() as u64);
        assert!(
            bytes == *body,
            "segmented output must equal the remote file"
        );
        assert!(!has_ranges && !has_state, "success removes resume state");
        let rests = server.commands("REST ");
        let segments =
            pending_segments(body.len() as u64, &[], segment_length(body.len() as u64, 4));
        assert_eq!(
            rests.len(),
            segments.len(),
            "one REST per segment: {rests:?}"
        );
        let used: std::collections::BTreeSet<usize> = server
            .commands("RETR")
            .iter()
            .map(|(conn, _)| *conn)
            .collect();
        assert!(
            used.len() >= 2,
            "more than one connection transferred data: {used:?}"
        );
        assert!(server.connections.load(std::sync::atomic::Ordering::SeqCst) <= 4);
    }

    #[test]
    fn single_connection_is_kept_without_rest_stream_or_for_small_files() {
        for (case, advertise_rest, len, connections) in [
            ("no-feat-rest", false, SEGMENT_TEST_BYTES, 4),
            ("small-file", true, 64 * 1024, 4),
            ("one-connection", true, SEGMENT_TEST_BYTES, 1),
        ] {
            let body = segment_body(len);
            let server = spawn_segment_server(
                std::sync::Arc::clone(&body),
                SegmentServerOptions {
                    advertise_rest,
                    rest_ok: true,
                    max_connections: 16,
                    chunk_delay_ms: 0,
                    passive_refused: false,
                },
            );
            let (root, output, control) = segment_root(case);
            let url = format!("ftp://{}/big.bin", server.address);
            let result = download_ftp_with(&url, &output, &control, true, connections);
            let bytes = std::fs::read(&output).unwrap();
            std::fs::remove_dir_all(&root).unwrap();
            assert_eq!(result.unwrap(), len as u64, "{case}");
            assert!(bytes == *body, "{case}");
            assert_eq!(
                server.connections.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "{case}"
            );
            assert!(server.commands("REST ").is_empty(), "{case}");
            assert_eq!(
                server.commands("FEAT").is_empty(),
                case != "no-feat-rest",
                "{case}"
            );
        }
    }

    #[test]
    fn rejected_rest_falls_back_to_one_fresh_connection() {
        let body = segment_body(SEGMENT_TEST_BYTES);
        let server = spawn_segment_server(
            std::sync::Arc::clone(&body),
            SegmentServerOptions {
                advertise_rest: true,
                rest_ok: false,
                max_connections: 16,
                chunk_delay_ms: 0,
                passive_refused: false,
            },
        );
        let (root, output, control) = segment_root("rest-rejected");
        let url = format!("ftp://{}/big.bin", server.address);
        let result = download_ftp_with(&url, &output, &control, true, 4);
        let bytes = std::fs::read(&output).unwrap();
        let has_ranges = output.with_extension(SEGMENT_STATE_EXTENSION).exists();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(result.unwrap(), body.len() as u64);
        assert!(bytes == *body);
        assert!(!has_ranges);
        // Helpers are only opened after the primary's REST succeeded.
        assert_eq!(
            server.connections.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        assert_eq!(server.commands("RETR").len(), 1);
    }

    #[test]
    fn refused_helper_connections_leave_the_work_to_the_primary() {
        let body = segment_body(SEGMENT_TEST_BYTES);
        let server = spawn_segment_server(
            std::sync::Arc::clone(&body),
            SegmentServerOptions {
                advertise_rest: true,
                rest_ok: true,
                max_connections: 1,
                chunk_delay_ms: 0,
                passive_refused: false,
            },
        );
        let (root, output, control) = segment_root("helpers-refused");
        let url = format!("ftp://{}/big.bin", server.address);
        let result = download_ftp_with(&url, &output, &control, true, 4);
        let bytes = std::fs::read(&output).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(result.unwrap(), body.len() as u64);
        assert!(bytes == *body);
        assert!(server.commands("RETR").iter().all(|(conn, _)| *conn == 0));
    }

    #[test]
    fn paused_segmented_transfer_resumes_only_unrecorded_ranges() {
        use std::sync::atomic::Ordering;
        let body = segment_body(SEGMENT_TEST_BYTES);
        let server = spawn_segment_server(
            std::sync::Arc::clone(&body),
            SegmentServerOptions {
                advertise_rest: true,
                rest_ok: true,
                max_connections: 16,
                chunk_delay_ms: 15,
                passive_refused: false,
            },
        );
        let (root, output, control) = segment_root("pause-resume");
        let url = format!("ftp://{}/big.bin", server.address);
        let worker = {
            let (url, output, control) = (url.clone(), output.clone(), control.clone());
            std::thread::spawn(move || download_ftp_with(&url, &output, &control, true, 3))
        };
        // Let every connection write part of a segment, then pause.
        let deadline = Instant::now() + Duration::from_secs(10);
        while server.commands("RETR").len() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(250));
        std::fs::write(&control, "pause").unwrap();
        let paused_at = Instant::now();
        let first = worker.join().unwrap();
        assert_eq!(first.unwrap_err(), "paused");
        assert!(paused_at.elapsed() < Duration::from_secs(3));
        let state: SegmentState = serde_json::from_slice(
            &std::fs::read(output.with_extension(SEGMENT_STATE_EXTENSION)).unwrap(),
        )
        .unwrap();
        assert!(
            !state.done.is_empty(),
            "written bytes are recorded on pause"
        );
        assert_eq!(std::fs::metadata(&output).unwrap().len(), body.len() as u64);
        for &(start, end) in &state.done {
            let file = std::fs::read(&output).unwrap();
            assert!(
                file[start as usize..=end as usize] == body[start as usize..=end as usize],
                "recorded range {start}-{end} holds remote bytes"
            );
        }
        server.clear();
        std::fs::write(&control, "run").unwrap();
        let second = download_ftp_with(&url, &output, &control, true, 3);
        let bytes = std::fs::read(&output).unwrap();
        let has_ranges = output.with_extension(SEGMENT_STATE_EXTENSION).exists();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(second.unwrap(), body.len() as u64);
        assert!(bytes == *body);
        assert!(!has_ranges);
        for (_, rest) in server.commands("REST ") {
            let offset: u64 = rest.trim_start_matches("REST ").parse().unwrap();
            assert!(
                !state
                    .done
                    .iter()
                    .any(|(start, end)| *start <= offset && offset <= *end),
                "resume re-requested recorded byte {offset}"
            );
        }
        assert!(server.connections.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn changed_remote_identity_discards_the_segment_map() {
        let (root, output, _control) = segment_root("identity");
        let resource = ResumeState {
            version: 1,
            host: "127.0.0.1".into(),
            port: 21,
            user: "anonymous".into(),
            path: "/big.bin".into(),
            tls: false,
            total: 10,
            modified: "20261115000000".into(),
        };
        let path = output.with_extension(SEGMENT_STATE_EXTENSION);
        write_segment_state(
            &path,
            &SegmentState {
                version: 1,
                resource: resource.clone(),
                done: vec![(0, 4)],
            },
        )
        .unwrap();
        let saved: SegmentState = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.done, vec![(0, 4)]);
        let mut changed = resource.clone();
        changed.modified = "20261116000000".into();
        assert_ne!(saved.resource, changed);
        std::fs::remove_dir_all(root).unwrap();
    }
}
