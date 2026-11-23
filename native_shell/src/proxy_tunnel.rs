//! TCP tunnels for the native FTP and SSH transports.
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::OnceLock;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio_tungstenite::tungstenite::http::uri::Authority;

use crate::http_engine::{proxy_route, ProxyRoute};

#[derive(Debug, Clone, Copy)]
enum Protocol {
    Http,
    Socks5 { remote_dns: bool },
    Socks4 { remote_dns: bool },
}

pub(crate) struct Proxy {
    protocol: Protocol,
    host: String,
    port: u16,
    credentials: Option<(String, String)>,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("protocol", &self.protocol)
            .finish_non_exhaustive()
    }
}

fn runtime() -> Result<&'static Runtime, String> {
    static RT: OnceLock<io::Result<Runtime>> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
    })
    .as_ref()
    .map_err(|e| format!("无法启动代理连接：{e}"))
}

pub(crate) async fn controlled<T>(
    operation: impl Future<Output = Result<T, String>>,
    check: impl Fn() -> Result<(), String>,
) -> Result<T, String> {
    check()?;
    let mut poll = tokio::time::interval(Duration::from_millis(100));
    let deadline = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(operation, deadline);
    loop {
        tokio::select! {
            biased;
            _ = &mut deadline => return Err("代理连接或握手超时".into()),
            _ = poll.tick() => check()?,
            result = &mut operation => return result,
        }
    }
}

fn valid_host(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok()
        || (!host.is_empty()
            && host.len() <= 255
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')))
}

fn authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

impl Proxy {
    pub(crate) fn resolve(
        route: &str,
        target_url: &str,
        check: impl Fn() -> Result<(), String>,
    ) -> Result<Option<Self>, String> {
        match proxy_route(route) {
            ProxyRoute::Direct => Ok(None),
            ProxyRoute::Manual(url) => Self::parse(url).map(Some),
            ProxyRoute::System => {
                #[cfg(windows)]
                {
                    // PAC receives the target URL without transfer credentials.
                    let (scheme, rest) = target_url.split_once("://").ok_or("目标地址无效")?;
                    let (endpoint, path) = rest.split_once('/').unwrap_or((rest, ""));
                    let endpoint = endpoint.rsplit('@').next().unwrap_or(endpoint);
                    let url = format!("{scheme}://{endpoint}/{path}");
                    runtime()?
                        .block_on(controlled(
                            async {
                                tokio::task::spawn_blocking(move || system::lookup(&url))
                                    .await
                                    .map_err(|_| "系统代理解析失败".to_string())?
                            },
                            check,
                        ))?
                        .map(|url| Self::parse(&url))
                        .transpose()
                }
                #[cfg(not(windows))]
                {
                    let _ = (target_url, check);
                    Ok(None)
                }
            }
        }
    }

    fn parse(url: &str) -> Result<Self, String> {
        let (scheme, rest) = url.split_once("://").ok_or("代理地址无效")?;
        let protocol = match scheme.to_ascii_lowercase().as_str() {
            "http" => Protocol::Http,
            "https" => return Err("原生 FTP/SFTP 尚不支持到 HTTPS 代理的 TLS 连接".into()),
            "socks5" => Protocol::Socks5 { remote_dns: false },
            "socks5h" => Protocol::Socks5 { remote_dns: true },
            "socks4" => Protocol::Socks4 { remote_dns: false },
            "socks4a" => Protocol::Socks4 { remote_dns: true },
            _ => return Err("不支持的代理协议".into()),
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.contains(['/', '?', '#']) || rest.chars().any(char::is_control) {
            return Err("代理地址无效".into());
        }
        let (credentials, endpoint) = if let Some((info, endpoint)) = rest.rsplit_once('@') {
            let (user, pass) = info.split_once(':').unwrap_or((info, ""));
            let decode = |s| {
                crate::url_credentials::decode_proxy_userinfo(s)
                    .filter(|s| !s.chars().any(char::is_control))
                    .ok_or("代理凭据无效")
            };
            let (user, pass) = (decode(user)?, decode(pass)?);
            match protocol {
                Protocol::Http if user.contains(':') => {
                    return Err("HTTP 代理用户名不能包含冒号".into())
                }
                Protocol::Socks5 { .. }
                    if !(1..=255).contains(&user.len()) || !(1..=255).contains(&pass.len()) =>
                {
                    return Err("SOCKS5 代理用户名和密码必须各为 1 至 255 字节".into())
                }
                Protocol::Socks4 { .. } if !pass.is_empty() => {
                    return Err("SOCKS4 代理不支持密码认证".into())
                }
                _ => {}
            }
            (Some((user, pass)), endpoint)
        } else {
            (None, rest)
        };
        let endpoint: Authority = endpoint.parse().map_err(|_| "代理主机或端口无效")?;
        let host = endpoint.host();
        // The vault and curl use 1080 for these proxy schemes when the port is omitted.
        let port = match &endpoint.as_str()[host.len()..] {
            "" => 1080,
            _ => endpoint
                .port_u16()
                .filter(|p| *p != 0)
                .ok_or("代理端口无效")?,
        };
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        if !valid_host(host) {
            return Err("代理主机无效".into());
        }
        Ok(Self {
            protocol,
            host: host.into(),
            port,
            credentials,
        })
    }

    pub(crate) fn connect_blocking(
        &self,
        host: &str,
        port: u16,
        check: impl Fn() -> Result<(), String>,
    ) -> Result<std::net::TcpStream, String> {
        let stream = runtime()?
            .block_on(self.connect(host, port, check))?
            .into_std()
            .map_err(io_error)?;
        stream.set_nonblocking(false).map_err(io_error)?;
        Ok(stream)
    }

    pub(crate) async fn connect(
        &self,
        host: &str,
        port: u16,
        check: impl Fn() -> Result<(), String>,
    ) -> Result<TcpStream, String> {
        if !valid_host(host) || port == 0 {
            return Err("代理隧道目标地址无效".into());
        }
        controlled(
            async {
                let mut stream = TcpStream::connect((self.host.as_str(), self.port))
                    .await
                    .map_err(|e| format!("连接代理服务器失败：{e}"))?;
                match self.protocol {
                    Protocol::Http => self.http(&mut stream, host, port).await?,
                    Protocol::Socks5 { remote_dns } => {
                        self.socks5(&mut stream, host, port, remote_dns).await?
                    }
                    Protocol::Socks4 { remote_dns } => {
                        self.socks4(&mut stream, host, port, remote_dns).await?
                    }
                }
                Ok(stream)
            },
            check,
        )
        .await
    }

    async fn http(&self, stream: &mut TcpStream, host: &str, port: u16) -> Result<(), String> {
        let dest = authority(host, port);
        let mut request = format!("CONNECT {dest} HTTP/1.1\r\nHost: {dest}\r\n");
        if let Some((user, pass)) = &self.credentials {
            let token = STANDARD.encode(format!("{user}:{pass}"));
            request.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
        }
        request.push_str("\r\n");
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(io_error)?;
        // The server greeting can share a packet with the CONNECT response.
        // Consume exactly the header, leaving every tunneled byte on the socket.
        let mut header = Vec::new();
        let mut buf = [0; 1024];
        loop {
            let len = stream.peek(&mut buf).await.map_err(io_error)?;
            if len == 0 {
                return Err("HTTP 代理握手提前结束".into());
            }
            let start = header.len();
            header.extend_from_slice(&buf[..len]);
            let offset = start.saturating_sub(3);
            let end = header[offset..]
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| offset + p + 4);
            let consumed = end.unwrap_or(header.len());
            if consumed > 16 * 1024 {
                return Err("HTTP 代理响应头过大".into());
            }
            stream
                .read_exact(&mut buf[..consumed - start])
                .await
                .map_err(io_error)?;
            if end.is_some() {
                break;
            }
        }
        let line = header.split(|b| *b == b'\r').next().unwrap_or_default();
        let mut fields = line.split(|b| *b == b' ');
        if !matches!(fields.next(), Some(b"HTTP/1.0" | b"HTTP/1.1")) {
            return Err("HTTP 代理响应无效".into());
        }
        match fields.next() {
            Some(b"407") => Err("HTTP 代理认证失败（407）".into()),
            Some([b'2', b'0'..=b'9', b'0'..=b'9']) => Ok(()),
            _ => Err("HTTP 代理拒绝建立 CONNECT 隧道".into()),
        }
    }

    async fn socks5(
        &self,
        stream: &mut TcpStream,
        host: &str,
        port: u16,
        remote_dns: bool,
    ) -> Result<(), String> {
        let method = if self.credentials.is_some() { 2 } else { 0 };
        stream.write_all(&[5, 1, method]).await.map_err(io_error)?;
        let mut reply = [0; 2];
        stream.read_exact(&mut reply).await.map_err(io_error)?;
        if reply != [5, method] {
            return Err("SOCKS5 代理拒绝认证方式".into());
        }
        if let Some((user, pass)) = &self.credentials {
            let mut auth = vec![1, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.push(pass.len() as u8);
            auth.extend_from_slice(pass.as_bytes());
            stream.write_all(&auth).await.map_err(io_error)?;
            stream.read_exact(&mut reply).await.map_err(io_error)?;
            if reply != [1, 0] {
                return Err("SOCKS5 代理认证失败".into());
            }
        }
        let mut request = vec![5, 1, 0];
        match target_ip(host, port, remote_dns, false).await? {
            Some(IpAddr::V4(ip)) => {
                request.push(1);
                request.extend(ip.octets());
            }
            Some(IpAddr::V6(ip)) => {
                request.push(4);
                request.extend(ip.octets());
            }
            None => {
                request.extend([3, host.len() as u8]);
                request.extend(host.as_bytes());
            }
        }
        request.extend(port.to_be_bytes());
        stream.write_all(&request).await.map_err(io_error)?;
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).await.map_err(io_error)?;
        if prefix[..3] != [5, 0, 0] {
            return Err(format!("SOCKS5 代理隧道请求失败（{}）", prefix[1]));
        }
        let addr_len = match prefix[3] {
            1 => 4,
            4 => 16,
            3 => usize::from(stream.read_u8().await.map_err(io_error)?),
            _ => return Err("SOCKS5 代理响应地址类型无效".into()),
        };
        stream
            .read_exact(&mut [0; 257][..addr_len + 2])
            .await
            .map_err(io_error)?;
        Ok(())
    }

    async fn socks4(
        &self,
        stream: &mut TcpStream,
        host: &str,
        port: u16,
        remote_dns: bool,
    ) -> Result<(), String> {
        let ip = target_ip(host, port, remote_dns, true).await?;
        let mut request = vec![4, 1];
        request.extend(port.to_be_bytes());
        match ip {
            Some(IpAddr::V4(v4)) if v4.octets()[..3] != [0, 0, 0] => request.extend(v4.octets()),
            None => request.extend(Ipv4Addr::new(0, 0, 0, 1).octets()),
            _ => return Err("SOCKS4 代理不支持该目标 IP".into()),
        }
        if let Some((user, _)) = &self.credentials {
            request.extend(user.as_bytes());
        }
        request.push(0);
        if ip.is_none() {
            request.extend(host.as_bytes());
            request.push(0);
        }
        stream.write_all(&request).await.map_err(io_error)?;
        let mut reply = [0; 8];
        stream.read_exact(&mut reply).await.map_err(io_error)?;
        if reply[..2] != [0, 90] {
            return Err(format!("SOCKS4 代理拒绝连接（{}）", reply[1]));
        }
        Ok(())
    }
}

async fn target_ip(
    host: &str,
    port: u16,
    remote_dns: bool,
    ipv4_only: bool,
) -> Result<Option<IpAddr>, String> {
    if let Ok(ip) = host.parse() {
        return Ok(Some(ip));
    }
    if remote_dns {
        return Ok(None);
    }
    tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "代理目标域名解析失败")?
        .find(|addr| !ipv4_only || addr.is_ipv4())
        .map(|addr| Some(addr.ip()))
        .ok_or_else(|| "代理目标缺少可用地址".into())
}

fn io_error(e: io::Error) -> String {
    format!("代理隧道握手失败：{e}")
}

#[cfg(windows)]
mod system;
#[cfg(test)]
pub(crate) mod tests;
