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
    let (host, port) = if let Some((host, port)) = hostport.rsplit_once(':') {
        (host, port.parse().unwrap_or(if tls { 990 } else { 21 }))
    } else {
        (hostport, if tls { 990 } else { 21 })
    };
    if host.is_empty() || !ftp_wire_ok(host) {
        return Err("FTP host missing".into());
    }
    let user = if user.is_empty() { "anonymous" } else { user };
    let path = format!("/{}", path.trim_start_matches('/'));
    if !ftp_wire_ok(user) || !ftp_wire_ok(password) || !ftp_wire_ok(&path) {
        return Err("FTP 地址不能包含控制字符".into());
    }
    Ok(FtpTarget {
        host: host.to_string(),
        port,
        user: user.into(),
        password: password.into(),
        path,
        tls,
    })
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
    read_reply(&mut ctrl)?;
    if target.tls && !implicit {
        command(&mut ctrl, "AUTH TLS")?;
        ctrl = wrap_tls(take_plain(ctrl)?, &target.host)?;
        command(&mut ctrl, "PBSZ 0")?;
        command(&mut ctrl, "PROT P")?;
    }
    command(&mut ctrl, &format!("USER {}", target.user))?;
    command(&mut ctrl, &format!("PASS {}", target.password))?;
    command(&mut ctrl, "TYPE I")?;
    let size = command(&mut ctrl, &format!("SIZE {}", target.path))
        .ok()
        .and_then(|reply| reply.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(0);
    let resume_from = if resume && output.exists() {
        std::fs::metadata(output)
            .map(|meta| meta.len())
            .unwrap_or(0)
    } else {
        0
    };
    if resume_from > 0 {
        command(&mut ctrl, &format!("REST {resume_from}"))?;
    }
    let pasv = command(&mut ctrl, "PASV")?;
    let data_port = parse_pasv_port(&pasv)?;
    let data_addr = std::net::SocketAddr::new(ctrl.peer_addr()?.ip(), data_port);
    let data_raw = TcpStream::connect(data_addr).map_err(|error| error.to_string())?;
    let mut data = if target.tls {
        wrap_tls(data_raw, &target.host)?
    } else {
        Conn::Plain(data_raw)
    };
    command(&mut ctrl, &format!("RETR {}", target.path))?;
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
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = resume_from;
    crate::http_engine::write_progress(&progress, downloaded, size, 0.0, "downloading");
    loop {
        let flag = std::fs::read_to_string(control).unwrap_or_else(|_| "run".into());
        if flag.trim() == "pause" {
            return Err("paused".into());
        }
        if flag.trim() == "cancel" {
            return Err("canceled".into());
        }
        let count = data.read(&mut buf).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        file.write_all(&buf[..count])
            .map_err(|error| error.to_string())?;
        downloaded += count as u64;
        crate::http_engine::write_progress(&progress, downloaded, size, 0.0, "downloading");
        crate::net_policy::consume(count);
    }
    let _ = size;
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

fn read_reply(stream: &mut Conn) -> Result<String, String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while buf.len() < 8192 {
        if stream.read(&mut byte).map_err(|error| error.to_string())? == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n") && buf.len() >= 4 && buf[3] == b' ' {
            break;
        }
    }
    String::from_utf8(buf).map_err(|error| error.to_string())
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
        assert!(parse_ftp_url("ftp://files.example/a.bin\r\nSITE EXEC x").is_err());
        assert!(parse_ftp_url("ftp://alice\r\nPASS x@files.example/a.bin").is_err());
        let at = parse_ftp_url("ftp://alice:p@ss@files.example:2121/pub/a.bin").unwrap();
        assert_eq!(at.user, "alice");
        assert_eq!(at.password, "p@ss");
        assert_eq!(at.host, "files.example");
        assert_eq!(at.port, 2121);
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
}
