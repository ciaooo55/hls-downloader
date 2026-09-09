//! SFTP: one stream, STAT identity, seek-resume. Matches Python `sftp_file.py`.
//!
//! The live path is an in-process russh session: password (or default keys),
//! TOFU host key, STAT, then seek. OpenSSH `sftp get -a` is only the key-agent
//! fallback when the URL has no password. URL passwords never go to OpenSSH.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const STATE_VERSION: u32 = 1;
const BLOCK_SIZE: usize = 64 * 1024;

#[derive(Clone, PartialEq)]
pub struct SftpTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub path: String,
}

impl SftpTarget {
    pub fn resource_key(&self) -> String {
        format!("sftp://{}:{}{}", self.host, self.port, self.path)
    }
}

impl std::fmt::Debug for SftpTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SftpTarget")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field(
                "password",
                &if self.password.is_empty() { "" } else { "***" },
            )
            .field("path", &self.path)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SftpStat {
    pub size: u64,
    pub mtime: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct ResumeState {
    version: u32,
    resource_key: String,
    total: u64,
    mtime: String,
    offset: u64,
}

pub(crate) trait SftpFile {
    fn seek(&mut self, offset: u64) -> Result<(), String>;
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String>;
}

pub(crate) trait SftpSession {
    fn stat(&self, path: &str) -> Result<SftpStat, String>;
    fn open(&self, path: &str) -> Result<Box<dyn SftpFile>, String>;
}

pub(crate) fn map_sftp_io(error: impl std::fmt::Display) -> String {
    let text = error.to_string();
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("no such file") || lowered.contains("not found") {
        "SFTP 远程文件不存在".into()
    } else if lowered.contains("auth") || lowered.contains("permission denied") {
        "SFTP 登录失败，请检查用户名、密码或私钥".into()
    } else {
        format!(
            "SFTP 下载失败：{}",
            text.chars().take(200).collect::<String>()
        )
    }
}

pub fn parse_sftp_url(url: &str) -> Result<SftpTarget, String> {
    let rest = url
        .strip_prefix("sftp://")
        .ok_or_else(|| "链接必须是 sftp:// 地址".to_string())?;
    let (auth, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (userinfo, hostport) = if let Some((userinfo, hostport)) = auth.rsplit_once('@') {
        (userinfo, hostport)
    } else {
        ("", auth)
    };
    let (user, password) = match userinfo.split_once(':') {
        Some((user, password)) => (percent_decode(user), percent_decode(password)),
        None if !userinfo.is_empty() => (percent_decode(userinfo), String::new()),
        None => (String::new(), String::new()),
    };
    let (host, port) = hostport
        .rsplit_once(':')
        .map(|(host, port)| (host, port.parse().unwrap_or(22)))
        .unwrap_or((hostport, 22));
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if !ssh_host_ok(&host) {
        return Err("SFTP 地址缺少有效主机名".into());
    }
    let path = percent_decode(path);
    if path.is_empty() || path.ends_with('/') {
        return Err("SFTP 地址必须指向具体文件，不能是目录".into());
    }
    if path.contains('\\') || path.contains('\0') || path.chars().any(|ch| (ch as u32) < 32) {
        return Err("SFTP 远程路径无效".into());
    }
    let user = if user.is_empty() {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .map_err(|_| "SFTP 地址需要用户名".to_string())?
    } else {
        user
    };
    if !ssh_user_ok(&user) {
        return Err("SFTP 用户名无效".into());
    }
    Ok(SftpTarget {
        host,
        port,
        user,
        password,
        path: format!("/{path}"),
    })
}

fn ssh_host_ok(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
}

fn ssh_user_ok(user: &str) -> bool {
    !user.is_empty()
        && !user.starts_with('-')
        && !user.contains([
            '@', ' ', '\t', '\r', '\n', '"', '\'', '\\', ',', '=', '\0', '/',
        ])
}

fn ssh_option(name: &str, value: &str) -> String {
    let escaped = value.replace('\\', "/").replace('"', "");
    format!("{name}=\"{escaped}\"")
}

fn percent_decode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(decoded) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(decoded as char);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

pub fn known_hosts_path() -> PathBuf {
    if let Some(root) = std::env::var_os("HLS_V7_DATA_DIR") {
        return PathBuf::from(root).join("known_hosts");
    }
    crate::default_v7_database_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("known_hosts")
}

fn openssh_known_hosts_path() -> PathBuf {
    known_hosts_path().with_extension("ssh")
}

fn state_path(output: &Path) -> PathBuf {
    output.with_file_name("sftp-resume.json")
}

fn progress_path(output: &Path) -> PathBuf {
    output.with_extension("progress.json")
}

pub fn tofu_record(host: &str, port: u16, fingerprint: &str) -> Result<(), String> {
    if fingerprint.is_empty() || fingerprint == "pending" {
        return Err("SFTP host key missing".into());
    }
    let path = known_hosts_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let key = format!("{host}:{port}");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    for line in existing.lines() {
        if let Some((stored_host, stored_fp)) = line.split_once(' ') {
            if stored_host == key && stored_fp != fingerprint {
                return Err("SFTP 主机密钥不匹配，可能不是原来的服务器".into());
            }
            if stored_host == key {
                return Ok(());
            }
        }
    }
    let mut text = existing;
    if !text.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&format!("{key} {fingerprint}\n"));
    fs::write(path, text).map_err(|error| error.to_string())
}

pub fn fingerprint_bytes(bytes: &[u8]) -> String {
    crate::crypto_lite::sha256_hex(bytes)
}

pub fn parse_keyscan_line(output: &str) -> Result<(String, String), String> {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let _name = parts.next();
        let algorithm = parts.next().unwrap_or("");
        let material = parts.next().unwrap_or("");
        if algorithm.starts_with("ssh-")
            || algorithm.starts_with("ecdsa-")
            || algorithm == "rsa-sha2-256"
            || algorithm == "rsa-sha2-512"
        {
            if material.is_empty() {
                continue;
            }
            return Ok((
                line.to_string(),
                crate::sftp_live::fingerprint_openssh_material(algorithm, material),
            ));
        }
    }
    Err("ssh-keyscan returned no host key".into())
}

fn pin_remote_host(host: &str, port: u16) -> Result<PathBuf, String> {
    let output = keyscan_output(host, port)?;
    let (openssh_line, fingerprint) = parse_keyscan_line(&output)?;
    tofu_record(host, port, &fingerprint)?;
    let ssh_path = openssh_known_hosts_path();
    upsert_openssh_known_host(&ssh_path, &openssh_line)?;
    Ok(ssh_path)
}

fn keyscan_output(host: &str, port: u16) -> Result<String, String> {
    #[cfg(test)]
    if let Ok(fixture) = std::env::var("HLS_V7_SFTP_KEYSCAN") {
        return Ok(fixture);
    }

    let scanner = which("ssh-keyscan").ok_or_else(|| {
        "OpenSSH ssh-keyscan not found; cannot TOFU the SFTP host key".to_string()
    })?;
    let scanned = Command::new(scanner)
        .args([
            "-p",
            &port.to_string(),
            "-T",
            "8",
            "-t",
            "rsa,ecdsa,ed25519",
            host,
        ])
        .output()
        .map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&scanned.stdout);
    let stderr = String::from_utf8_lossy(&scanned.stderr);
    if stdout.trim().is_empty() {
        return Err(format!("ssh-keyscan failed: {stderr}"));
    }
    Ok(stdout.into_owned())
}

fn upsert_openssh_known_host(path: &Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let line = line.trim();
    let host = line.split_whitespace().next().unwrap_or("");
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|old| old.split_whitespace().next().unwrap_or("") != host)
        .map(str::to_string)
        .collect();
    lines.push(line.to_string());
    fs::write(path, lines.join("\n") + "\n").map_err(|error| error.to_string())
}

fn load_state(path: &Path) -> ResumeState {
    let Ok(text) = fs::read_to_string(path) else {
        return ResumeState::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return ResumeState::default();
    };
    ResumeState {
        version: value
            .get("version")
            .and_then(|item| item.as_u64())
            .unwrap_or(0) as u32,
        resource_key: value
            .get("resource_key")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string(),
        total: value
            .get("total")
            .and_then(|item| item.as_u64())
            .unwrap_or(0),
        mtime: value
            .get("mtime")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string(),
        offset: value
            .get("offset")
            .and_then(|item| item.as_u64())
            .unwrap_or(0),
    }
}

fn save_state(path: &Path, state: &ResumeState) -> Result<(), String> {
    let value = serde_json::json!({
        "version": state.version,
        "resource_key": state.resource_key,
        "total": state.total,
        "mtime": state.mtime,
        "offset": state.offset,
    });
    fs::write(path, value.to_string()).map_err(|error| error.to_string())
}

fn resume_offset(
    current_size: u64,
    stat: &SftpStat,
    resource_key: &str,
    state: &ResumeState,
) -> u64 {
    if stat.size > 0
        && current_size > 0
        && current_size < stat.size
        && state.version == STATE_VERSION
        && state.resource_key == resource_key
        && state.total == stat.size
        && state.mtime == stat.mtime
    {
        current_size
    } else {
        0
    }
}

fn read_control(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|_| "run".into())
        .trim()
        .to_ascii_lowercase()
}

fn transfer_from_session(
    session: &dyn SftpSession,
    target: &SftpTarget,
    output: &Path,
    control: &Path,
    offset: u64,
    total: u64,
) -> Result<u64, String> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut remote = session.open(&target.path)?;
    if offset > 0 {
        remote.seek(offset)?;
    }
    let mut local = if offset > 0 && output.exists() {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(output)
            .map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| error.to_string())?;
        file
    } else {
        File::create(output).map_err(|error| error.to_string())?
    };
    let mut buf = vec![0u8; BLOCK_SIZE];
    let mut written = offset;
    let progress = progress_path(output);
    loop {
        match read_control(control).as_str() {
            "cancel" => return Err("canceled".into()),
            "pause" => return Err("paused".into()),
            _ => {}
        }
        let count = remote.read(&mut buf)?;
        if count == 0 {
            break;
        }
        local
            .write_all(&buf[..count])
            .map_err(|error| error.to_string())?;
        written += count as u64;
        crate::net_policy::consume(count);
        crate::http_engine::write_progress(&progress, written, total, 0.0, "downloading");
    }
    local.flush().map_err(|error| error.to_string())?;
    Ok(written)
}

struct FixtureSession {
    root: PathBuf,
}

struct FixtureFile {
    file: File,
}

impl SftpFile for FixtureFile {
    fn seek(&mut self, offset: u64) -> Result<(), String> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        self.file.read(buf).map_err(|error| error.to_string())
    }
}

impl SftpSession for FixtureSession {
    fn stat(&self, path: &str) -> Result<SftpStat, String> {
        let source = self.root.join(path.trim_start_matches('/'));
        let meta = fs::metadata(&source).map_err(|_| "SFTP 远程文件不存在".to_string())?;
        if meta.is_dir() {
            return Err("SFTP 地址必须指向具体文件，不能是目录".into());
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|| "0".into());
        Ok(SftpStat {
            size: meta.len(),
            mtime,
        })
    }

    fn open(&self, path: &str) -> Result<Box<dyn SftpFile>, String> {
        let source = self.root.join(path.trim_start_matches('/'));
        let file = File::open(&source).map_err(|_| "SFTP 远程文件不存在".to_string())?;
        Ok(Box::new(FixtureFile { file }))
    }
}

fn download_with_session(
    target: &SftpTarget,
    output: &Path,
    control: &Path,
    session: &dyn SftpSession,
) -> Result<u64, String> {
    match read_control(control).as_str() {
        "cancel" => return Err("canceled".into()),
        "pause" => return Err("paused".into()),
        _ => {}
    }
    let stat = session.stat(&target.path)?;
    let state_file = state_path(output);
    let current = if output.exists() {
        fs::metadata(output).map(|meta| meta.len()).unwrap_or(0)
    } else {
        0
    };
    let state = load_state(&state_file);
    let offset = resume_offset(current, &stat, &target.resource_key(), &state);
    if offset == 0 && output.exists() {
        let _ = fs::remove_file(output);
    }
    let written = match transfer_from_session(session, target, output, control, offset, stat.size) {
        Ok(bytes) => bytes,
        Err(error) if error == "paused" => {
            let size = fs::metadata(output)
                .map(|meta| meta.len())
                .unwrap_or(offset);
            save_state(
                &state_file,
                &ResumeState {
                    version: STATE_VERSION,
                    resource_key: target.resource_key(),
                    total: stat.size,
                    mtime: stat.mtime,
                    offset: size,
                },
            )?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    if stat.size > 0 && written != stat.size {
        return Err(format!(
            "文件长度不匹配，期望 {}，实际 {written}",
            stat.size
        ));
    }
    let _ = fs::remove_file(state_file);
    Ok(written)
}

pub fn download_sftp(url: &str, output: &Path, control: &Path) -> Result<u64, String> {
    if read_control(control) == "cancel" {
        return Err("canceled".into());
    }
    let target = parse_sftp_url(url)?;
    #[cfg(test)]
    if let Ok(root) = std::env::var("HLS_V7_SFTP_FIXTURE") {
        let session = FixtureSession {
            root: PathBuf::from(root),
        };
        if session.stat(&target.path).is_ok() {
            tofu_record(&target.host, target.port, &fingerprint_bytes(b"fixture"))?;
            return download_with_session(&target, output, control, &session);
        }
    }
    match crate::sftp_live::open_live_session(&target) {
        Ok(session) => return download_with_session(&target, output, control, &session),
        Err(error) if target.password.is_empty() && openssh_fallback_ok(&error) => {}
        Err(error) => return Err(error),
    }
    let known_hosts = pin_remote_host(&target.host, target.port)?;
    openssh_get(&target, output, control, &known_hosts)
}

fn openssh_fallback_ok(error: &str) -> bool {
    error.contains("登录失败") || error.contains("连接超时") || error.contains("拒绝连接")
}

fn openssh_get(
    target: &SftpTarget,
    output: &Path,
    control: &Path,
    known_hosts: &Path,
) -> Result<u64, String> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let sftp = which("sftp").ok_or_else(|| "OpenSSH sftp not found".to_string())?;
    let local = output.display().to_string().replace('\\', "/");
    let current = if output.exists() {
        fs::metadata(output).map(|meta| meta.len()).unwrap_or(0)
    } else {
        0
    };
    let batch = format!(
        "{} \"{}\" \"{}\"\n",
        if current > 0 { "get -a" } else { "get" },
        target.path.replace('"', ""),
        local.replace('"', "")
    );
    let status = Command::new(sftp)
        .args([
            "-P",
            &target.port.to_string(),
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &ssh_option("UserKnownHostsFile", &known_hosts.display().to_string()),
            "-o",
            &ssh_option(
                "GlobalKnownHostsFile",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            ),
            "-o",
            &ssh_option("User", &target.user),
            "-b",
            "-",
            "--",
            &target.host,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(batch.as_bytes())?;
            }
            child.wait_with_output()
        })
        .map_err(|error| error.to_string())?;
    if read_control(control) == "cancel" {
        return Err("canceled".into());
    }
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(format!("sftp failed: {stderr}"));
    }
    fs::metadata(output)
        .map(|meta| meta.len())
        .map_err(|error| error.to_string())
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        let exe = dir.join(format!("{name}.exe"));
        if exe.exists() {
            return Some(exe);
        }
    }
    None
}

#[cfg(test)]
#[path = "sftp_loopback.rs"]
mod sftp_loopback;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn parses_sftp_and_rejects_host_key_change() {
        let _guard = env_lock();
        let target = parse_sftp_url("sftp://lee@nas.local:2222/video/a.mp4").unwrap();
        assert_eq!(target.host, "nas.local");
        assert_eq!(target.port, 2222);
        assert_eq!(target.path, "/video/a.mp4");
        assert_eq!(target.resource_key(), "sftp://nas.local:2222/video/a.mp4");
        assert!(target.password.is_empty());
        assert!(parse_sftp_url("sftp://lee@nas.local/video/").is_err());
        assert!(parse_sftp_url("sftp://-oProxyCommand=notepad@nas.local/a.bin").is_err());
        assert!(parse_sftp_url("sftp://lee@-oleak.example/a.bin").is_err());
        let dir = std::env::temp_dir().join(format!("hls-sftp-{}", std::process::id()));
        std::env::set_var("HLS_V7_DATA_DIR", &dir);
        tofu_record("nas.local", 2222, "abc").unwrap();
        let error = tofu_record("nas.local", 2222, "xyz").unwrap_err();
        assert!(error.contains("密钥") || error.contains("changed"));
        assert!(tofu_record("nas.local", 2222, "pending")
            .unwrap_err()
            .contains("missing"));
        let _ = fs::remove_dir_all(dir);
        std::env::remove_var("HLS_V7_DATA_DIR");
    }

    #[test]
    fn parses_password_userinfo_without_echoing_it() {
        let target = parse_sftp_url("sftp://lee:p%40ss@nas.local/pub/a.bin").unwrap();
        assert_eq!(target.user, "lee");
        assert_eq!(target.password, "p@ss");
        assert_eq!(target.port, 22);
        let debug = format!("{target:?}");
        assert!(debug.contains("***"));
        assert!(!debug.contains("p@ss"));
    }

    #[test]
    fn resume_requires_matching_identity() {
        let stat = SftpStat {
            size: 100,
            mtime: "10".into(),
        };
        let state = ResumeState {
            version: STATE_VERSION,
            resource_key: "sftp://nas.local:22/a.bin".into(),
            total: 100,
            mtime: "10".into(),
            offset: 40,
        };
        assert_eq!(
            resume_offset(40, &stat, "sftp://nas.local:22/a.bin", &state),
            40
        );
        assert_eq!(
            resume_offset(40, &stat, "sftp://nas.local:22/b.bin", &state),
            0
        );
        let changed = SftpStat {
            size: 100,
            mtime: "11".into(),
        };
        assert_eq!(
            resume_offset(40, &changed, "sftp://nas.local:22/a.bin", &state),
            0
        );
    }

    #[test]
    fn keyscan_pins_fingerprint_and_openssh_file() {
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!("hls-sftp-scan-{}", std::process::id()));
        std::env::set_var("HLS_V7_DATA_DIR", &dir);
        std::env::set_var(
            "HLS_V7_SFTP_KEYSCAN",
            "nas.local ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureKeyMaterial\n",
        );
        let ssh_path = pin_remote_host("nas.local", 22).unwrap();
        assert!(ssh_path.is_file());
        let fingerprint =
            parse_keyscan_line("nas.local ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureKeyMaterial")
                .unwrap()
                .1;
        assert!(tofu_record("nas.local", 22, &fingerprint).is_ok());
        std::env::set_var(
            "HLS_V7_SFTP_KEYSCAN",
            "other.local ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOtherKeyMaterial\n",
        );
        pin_remote_host("other.local", 22).unwrap();
        let stored = fs::read_to_string(ssh_path).unwrap();
        assert!(stored.contains("nas.local"));
        assert!(stored.contains("other.local"));
        std::env::remove_var("HLS_V7_SFTP_KEYSCAN");
        std::env::remove_var("HLS_V7_DATA_DIR");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fixture_copy_does_not_spawn_scp() {
        let _guard = env_lock();
        let root = std::env::temp_dir().join(format!("hls-sftp-fix-{}", std::process::id()));
        let src_dir = root.join("video");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("a.mp4"), b"sftp-bytes").unwrap();
        std::env::set_var("HLS_V7_SFTP_FIXTURE", &root);
        std::env::set_var("HLS_V7_DATA_DIR", root.join("data"));
        let dest = root.join("out").join("a.mp4");
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let size = download_sftp("sftp://lee@nas.local/video/a.mp4", &dest, &control).unwrap();
        assert_eq!(size, 10);
        assert_eq!(fs::read(&dest).unwrap(), b"sftp-bytes");
        std::env::remove_var("HLS_V7_SFTP_FIXTURE");
        std::env::remove_var("HLS_V7_DATA_DIR");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fixture_seeks_when_resume_identity_matches() {
        let _guard = env_lock();
        let root = std::env::temp_dir().join(format!("hls-sftp-resume-{}", std::process::id()));
        let src_dir = root.join("pub");
        fs::create_dir_all(&src_dir).unwrap();
        let payload = (0u8..80).collect::<Vec<_>>();
        fs::write(src_dir.join("a.bin"), &payload).unwrap();
        std::env::set_var("HLS_V7_SFTP_FIXTURE", &root);
        std::env::set_var("HLS_V7_DATA_DIR", root.join("data"));
        let dest = root.join("out").join("payload.downloading");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, &payload[..30]).unwrap();
        let session = FixtureSession { root: root.clone() };
        let stat = session.stat("/pub/a.bin").unwrap();
        save_state(
            &state_path(&dest),
            &ResumeState {
                version: STATE_VERSION,
                resource_key: "sftp://nas.local:22/pub/a.bin".into(),
                total: stat.size,
                mtime: stat.mtime,
                offset: 30,
            },
        )
        .unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let size = download_sftp("sftp://lee@nas.local/pub/a.bin", &dest, &control).unwrap();
        assert_eq!(size, 80);
        assert_eq!(fs::read(&dest).unwrap(), payload);
        assert!(!state_path(&dest).exists());
        std::env::remove_var("HLS_V7_SFTP_FIXTURE");
        std::env::remove_var("HLS_V7_DATA_DIR");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fixture_restarts_when_remote_mtime_changes() {
        let _guard = env_lock();
        let root = std::env::temp_dir().join(format!("hls-sftp-id-{}", std::process::id()));
        let src_dir = root.join("pub");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("a.bin"), b"abcdefghij").unwrap();
        std::env::set_var("HLS_V7_SFTP_FIXTURE", &root);
        std::env::set_var("HLS_V7_DATA_DIR", root.join("data"));
        let dest = root.join("out").join("payload.downloading");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"XXXX").unwrap();
        save_state(
            &state_path(&dest),
            &ResumeState {
                version: STATE_VERSION,
                resource_key: "sftp://nas.local:22/pub/a.bin".into(),
                total: 10,
                mtime: "1".into(),
                offset: 4,
            },
        )
        .unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let size = download_sftp("sftp://lee@nas.local/pub/a.bin", &dest, &control).unwrap();
        assert_eq!(size, 10);
        assert_eq!(fs::read(&dest).unwrap(), b"abcdefghij");
        std::env::remove_var("HLS_V7_SFTP_FIXTURE");
        std::env::remove_var("HLS_V7_DATA_DIR");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_password_stat_seek_and_tofu() {
        let _guard = env_lock();
        let root = std::env::temp_dir().join(format!("hls-sftp-live-{}", std::process::id()));
        let src_dir = root.join("pub");
        fs::create_dir_all(&src_dir).unwrap();
        let payload = (0u8..90).collect::<Vec<_>>();
        fs::write(src_dir.join("a.bin"), &payload).unwrap();
        std::env::remove_var("HLS_V7_SFTP_FIXTURE");
        std::env::set_var("HLS_V7_DATA_DIR", root.join("data"));
        let server = sftp_loopback::start(root.clone(), "lee", "s3cret").expect("loopback");
        let url = format!("sftp://lee:s3cret@127.0.0.1:{}/pub/a.bin", server.port);
        let dest = root.join("out").join("a.bin");
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let size = download_sftp(&url, &dest, &control).unwrap();
        assert_eq!(size, 90);
        assert_eq!(fs::read(&dest).unwrap(), payload);

        fs::write(&dest, &payload[..25]).unwrap();
        let session = crate::sftp_live::open_live_session(&parse_sftp_url(&url).unwrap()).unwrap();
        let stat = session.stat("/pub/a.bin").unwrap();
        save_state(
            &state_path(&dest),
            &ResumeState {
                version: STATE_VERSION,
                resource_key: format!("sftp://127.0.0.1:{}/pub/a.bin", server.port),
                total: stat.size,
                mtime: stat.mtime,
                offset: 25,
            },
        )
        .unwrap();
        let size = download_sftp(&url, &dest, &control).unwrap();
        assert_eq!(size, 90);
        assert_eq!(fs::read(&dest).unwrap(), payload);

        tofu_record("127.0.0.1", server.port, "wrong-host-key").unwrap_err();
        let pinned = known_hosts_path();
        let existing = fs::read_to_string(&pinned).unwrap_or_default();
        fs::write(&pinned, existing.replace('\n', "\n127.0.0.1:9999 other\n")).ok();
        let mismatch = tofu_record("127.0.0.1", server.port, "another-key").unwrap_err();
        assert!(mismatch.contains("密钥") || mismatch.contains("不匹配"));

        let bad = format!("sftp://lee:nope@127.0.0.1:{}/pub/a.bin", server.port);
        let error = download_sftp(&bad, &root.join("bad.bin"), &control).unwrap_err();
        assert!(
            error.contains("登录失败"),
            "wrong password should fail auth, got {error}"
        );

        drop(server);
        std::env::remove_var("HLS_V7_DATA_DIR");
        let _ = fs::remove_dir_all(root);
    }
}
