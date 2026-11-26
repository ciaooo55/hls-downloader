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
const CHECKPOINT_BYTES: u64 = 4 * 1024 * 1024;
const CHECKPOINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

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
        // SSH accounts may expose different files at the same host and path.
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("sftp://{}@{}:{}{}", self.user, host, self.port, self.path)
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
    if url.chars().any(char::is_control) {
        return Err("SFTP 地址不能包含控制字符".into());
    }
    let trimmed = url.trim().trim_start_matches('\u{feff}').trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err("链接必须是 sftp:// 地址".into());
    };
    if !scheme.eq_ignore_ascii_case("sftp") {
        return Err("链接必须是 sftp:// 地址".into());
    }
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
    // Preserve trailing ASCII space normalization on bare hostnames required by
    // `url_credentials` vault key canonicalization while rejecting leading,
    // interior, port-adjacent, bracketed, or non-ASCII whitespace.
    let hostport = if !hostport.starts_with('[') && !hostport.contains(':') {
        hostport.trim_end_matches(' ')
    } else {
        hostport
    };
    if hostport.is_empty() || hostport.chars().any(char::is_whitespace) {
        return Err("SFTP 地址缺少有效主机名".into());
    }
    let (host, port) = if let Some(after_bracket) = hostport.strip_prefix('[') {
        let (ipv6_raw, tail) = after_bracket
            .split_once(']')
            .ok_or_else(|| "SFTP 地址缺少有效主机名".to_string())?;
        let ip = ipv6_raw
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| "SFTP 地址缺少有效主机名".to_string())?;
        let port = if tail.is_empty() {
            22
        } else {
            let port_str = tail
                .strip_prefix(':')
                .ok_or_else(|| "SFTP 端口无效".to_string())?;
            if port_str.is_empty() || !port_str.bytes().all(|b| b.is_ascii_digit()) {
                return Err("SFTP 端口无效".into());
            }
            let port = port_str
                .parse::<u16>()
                .map_err(|_| "SFTP 端口无效".to_string())?;
            if port == 0 {
                return Err("SFTP 端口无效".into());
            }
            port
        };
        (ip.to_string(), port)
    } else {
        let (raw_host, port) = match hostport.rsplit_once(':') {
            Some((host, port_str)) => {
                if host.contains(':') {
                    return Err("SFTP 地址缺少有效主机名".into());
                }
                if port_str.is_empty() || !port_str.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("SFTP 端口无效".into());
                }
                let port = port_str
                    .parse::<u16>()
                    .map_err(|_| "SFTP 端口无效".to_string())?;
                if port == 0 {
                    return Err("SFTP 端口无效".into());
                }
                (host, port)
            }
            None => (hostport, 22),
        };
        (raw_host.trim_end_matches('.').to_ascii_lowercase(), port)
    };
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
    if host.is_empty() || host.starts_with('-') {
        return false;
    }
    if host.contains(':') {
        host.parse::<std::net::Ipv6Addr>().is_ok()
    } else {
        host.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
    }
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

fn locked_host_file(path: &Path) -> Result<(File, String), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.lock().map_err(|e| e.to_string())?;
    let mut existing = String::new();
    file.read_to_string(&mut existing)
        .map_err(|e| e.to_string())?;
    Ok((file, existing))
}

pub fn tofu_record(host: &str, port: u16, fingerprint: &str) -> Result<(), String> {
    if fingerprint.is_empty() || fingerprint == "pending" {
        return Err("SFTP host key missing".into());
    }
    record_host_key(&known_hosts_path(), host, port, fingerprint)
}

fn tofu_host_key(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn record_host_key(path: &Path, host: &str, port: u16, fingerprint: &str) -> Result<(), String> {
    let (mut file, existing) = locked_host_file(path)?;
    let key = tofu_host_key(host, port);
    for line in existing.lines() {
        let (stored_host, stored_fp) = line.split_once(' ').ok_or("SFTP 主机密钥记录损坏")?;
        if stored_host == key {
            if stored_fp != fingerprint {
                return Err("SFTP 主机密钥不匹配，可能不是原来的服务器".into());
            }
            return Ok(());
        }
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    writeln!(file, "{separator}{key} {fingerprint}")
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}

pub fn fingerprint_bytes(bytes: &[u8]) -> String {
    crate::crypto_lite::sha256_hex(bytes)
}

fn stored_tofu_fingerprint(host: &str, port: u16) -> Option<String> {
    let key = tofu_host_key(host, port);
    let (_lock, existing) = locked_host_file(&known_hosts_path()).ok()?;
    existing.lines().find_map(|line| {
        let (stored_host, stored_fp) = line.split_once(' ')?;
        (stored_host == key && !stored_fp.trim().is_empty()).then(|| stored_fp.trim().to_string())
    })
}

pub fn parse_keyscan_lines(output: &str) -> Vec<(String, String, String)> {
    let mut entries = Vec::new();
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
            entries.push((
                line.to_string(),
                algorithm.to_string(),
                crate::sftp_live::fingerprint_openssh_material(algorithm, material),
            ));
        }
    }
    entries
}

#[cfg(test)]
pub fn parse_keyscan_line(output: &str) -> Result<(String, String), String> {
    parse_keyscan_lines(output)
        .into_iter()
        .next()
        .map(|(line, _, fingerprint)| (line, fingerprint))
        .ok_or_else(|| "ssh-keyscan returned no host key".into())
}

fn pin_remote_host(host: &str, port: u16) -> Result<PathBuf, String> {
    let output = keyscan_output(host, port)?;
    let entries = parse_keyscan_lines(&output);
    if entries.is_empty() {
        return Err("ssh-keyscan returned no host key".into());
    }
    let selected = if let Some(stored) = stored_tofu_fingerprint(host, port) {
        entries
            .iter()
            .find(|(_, _, fingerprint)| *fingerprint == stored)
            .or_else(|| entries.iter().find(|(_, algo, _)| algo == "ssh-ed25519"))
            .unwrap_or(&entries[0])
    } else {
        entries
            .iter()
            .find(|(_, algo, _)| algo == "ssh-ed25519")
            .unwrap_or(&entries[0])
    };
    let (openssh_line, _, fingerprint) = selected;
    tofu_record(host, port, fingerprint)?;
    let ssh_path = openssh_known_hosts_path();
    upsert_openssh_known_host(&ssh_path, openssh_line)?;
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
    let mut command = Command::new(scanner);
    command
        .args([
            "-p",
            &port.to_string(),
            "-T",
            "8",
            "-t",
            "rsa,ecdsa,ed25519",
            host,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let child = command.spawn().map_err(|error| error.to_string())?;
    crate::job_object::assign_child(&child);
    let scanned = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&scanned.stdout);
    let stderr = String::from_utf8_lossy(&scanned.stderr);
    if stdout.trim().is_empty() {
        return Err(format!("ssh-keyscan failed: {stderr}"));
    }
    Ok(stdout.into_owned())
}

fn upsert_openssh_known_host(path: &Path, line: &str) -> Result<(), String> {
    let (mut file, existing) = locked_host_file(path)?;
    let line = line.trim();
    let host = line.split_whitespace().next().unwrap_or("");
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|old| old.split_whitespace().next().unwrap_or("") != host)
        .map(str::to_string)
        .collect();
    lines.push(line.to_string());
    file.rewind()
        .and_then(|()| file.set_len(0))
        .and_then(|()| writeln!(file, "{}", lines.join("\n")))
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = serde_json::json!({
        "version": state.version,
        "resource_key": state.resource_key,
        "total": state.total,
        "mtime": state.mtime,
        "offset": state.offset,
    });
    let tmp = path.with_extension("json.tmp");
    let mut file = File::create(&tmp).map_err(|error| error.to_string())?;
    file.write_all(value.to_string().as_bytes())
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    drop(file);
    crate::atomic_replace::replace_file(&tmp, path).map_err(|error| error.to_string())
}

fn resume_offset(
    current_size: u64,
    stat: &SftpStat,
    resource_key: &str,
    state: &ResumeState,
) -> u64 {
    if stat.size > 0
        && current_size > 0
        && current_size <= stat.size
        && state.offset == current_size
        && !stat.mtime.is_empty()
        && stat.mtime != "0"
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
    // 运行任务读进程内令牌，未注册路径仍读控制文件（行为不变）。
    crate::task_control::read_word(path)
}

#[cfg(test)]
fn transfer_from_session(
    session: &dyn SftpSession,
    target: &SftpTarget,
    output: &Path,
    control: &Path,
    offset: u64,
    total: u64,
) -> Result<u64, String> {
    transfer_from_session_checkpointed(session, target, output, control, offset, total, None)
}

fn transfer_from_session_checkpointed(
    session: &dyn SftpSession,
    target: &SftpTarget,
    output: &Path,
    control: &Path,
    offset: u64,
    total: u64,
    checkpoint: Option<(&Path, &str)>,
) -> Result<u64, String> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut remote = session.open(&target.path)?;
    if offset > 0 {
        remote.seek(offset)?;
    }
    let mut local = if offset > 0 {
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
    let mut checkpointed = offset;
    let mut last_checkpoint = std::time::Instant::now();
    let resource_key = target.resource_key();
    let persist_checkpoint = |local: &mut File, at: u64, truncate: bool| -> Result<(), String> {
        let Some((state_file, mtime)) = checkpoint else {
            return Ok(());
        };
        local.flush().map_err(|error| error.to_string())?;
        if truncate {
            local.set_len(at).map_err(|error| error.to_string())?;
        }
        local.sync_data().map_err(|error| error.to_string())?;
        save_state(
            state_file,
            &ResumeState {
                version: STATE_VERSION,
                resource_key: resource_key.clone(),
                total,
                mtime: mtime.to_string(),
                offset: at,
            },
        )
    };
    let progress = progress_path(output);
    let mut rate = crate::http_engine::TransferRate::new(written);
    let mut last_progress = std::time::Instant::now();
    crate::http_engine::write_progress(&progress, written, total, 0.0, "downloading");
    loop {
        match read_control(control).as_str() {
            "cancel" => return Err("canceled".into()),
            "pause" => {
                if written > offset {
                    persist_checkpoint(&mut local, written, false)?;
                }
                return Err("paused".into());
            }
            _ => {}
        }
        let count = match remote.read(&mut buf) {
            Ok(count) => count,
            Err(error) => {
                let ctrl = read_control(control);
                if ctrl == "cancel" {
                    return Err("canceled".into());
                }
                if written > offset {
                    if ctrl == "pause" {
                        persist_checkpoint(&mut local, written, false)?;
                        return Err("paused".into());
                    }
                    let _ = persist_checkpoint(&mut local, written, false);
                } else if ctrl == "pause" {
                    return Err("paused".into());
                }
                return Err(error);
            }
        };
        if count == 0 {
            break;
        }
        if count > buf.len() || count as u64 > total.saturating_sub(written) {
            if written > offset && read_control(control) != "cancel" {
                let _ = persist_checkpoint(&mut local, written, false);
            }
            return Err("SFTP 数据超过远程 STAT 声明的长度".into());
        }
        if let Err(ctrl_err) =
            crate::net_policy::consume(count, || match read_control(control).as_str() {
                "cancel" => Err("canceled".to_string()),
                "pause" => Err("paused".to_string()),
                _ => Ok(()),
            })
        {
            if ctrl_err != "canceled" && written > offset {
                persist_checkpoint(&mut local, written, false)?;
            }
            return Err(ctrl_err);
        }
        if let Err(error) = local.write_all(&buf[..count]) {
            if written > offset && read_control(control) != "cancel" {
                let _ = persist_checkpoint(&mut local, written, true);
            }
            return Err(error.to_string());
        }
        written += count as u64;
        if written.saturating_sub(checkpointed) >= CHECKPOINT_BYTES
            || (written > checkpointed && last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL)
        {
            persist_checkpoint(&mut local, written, false)?;
            checkpointed = written;
            last_checkpoint = std::time::Instant::now();
        }
        if last_progress.elapsed() >= std::time::Duration::from_millis(200) {
            let speed = rate.sample(written) as f64;
            crate::http_engine::write_progress(&progress, written, total, speed, "downloading");
            last_progress = std::time::Instant::now();
        }
    }
    local.flush().map_err(|error| error.to_string())?;
    if written != total && written > offset && read_control(control) != "cancel" {
        let _ = persist_checkpoint(&mut local, written, false);
    }
    let final_speed = rate.sample(written) as f64;
    crate::http_engine::write_progress(&progress, written, total, final_speed, "downloading");
    Ok(written)
}

#[cfg(test)]
struct FixtureSession {
    root: PathBuf,
}

#[cfg(test)]
struct FixtureFile {
    file: File,
}

#[cfg(test)]
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

#[cfg(test)]
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
    if offset == stat.size && stat.size > 0 {
        let progress = progress_path(output);
        crate::http_engine::write_progress(&progress, stat.size, stat.size, 0.0, "downloading");
        let _ = fs::remove_file(&state_file);
        return Ok(stat.size);
    }
    if offset == 0 {
        match fs::remove_file(&state_file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
        match fs::remove_file(output) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
    }
    let written = transfer_from_session_checkpointed(
        session,
        target,
        output,
        control,
        offset,
        stat.size,
        Some((&state_file, &stat.mtime)),
    )?;
    if written != stat.size {
        return Err(format!(
            "文件长度不匹配，期望 {}，实际 {written}",
            stat.size
        ));
    }
    let _ = fs::remove_file(state_file);
    Ok(written)
}

#[cfg(test)]
pub fn download_sftp(url: &str, output: &Path, control: &Path) -> Result<u64, String> {
    download_sftp_routed(url, output, control, "")
}

pub(crate) fn download_sftp_routed(
    url: &str,
    output: &Path,
    control: &Path,
    route: &str,
) -> Result<u64, String> {
    check_sftp_control(control)?;
    if crate::net_policy::parse_url_authority(url).is_none() {
        return Err("SFTP 地址缺少有效主机名".into());
    }
    let allowed = crate::net_policy::active_allowed_hosts();
    if !allowed.trim().is_empty() && !crate::net_policy::url_allowed(url, &allowed) {
        return Err("下载地址不在允许的域名范围内".into());
    }
    let target = parse_sftp_url(url)?;
    let effective_route = crate::net_policy::resolve_context_proxy(url, route);
    let proxy =
        crate::proxy_tunnel::Proxy::resolve(&effective_route, url, || check_sftp_control(control))?;
    #[cfg(test)]
    if let Some(root) = std::env::var("HLS_V7_SFTP_FIXTURE")
        .ok()
        .filter(|_| proxy.is_none())
    {
        let session = FixtureSession {
            root: PathBuf::from(root),
        };
        if session.stat(&target.path).is_ok() {
            tofu_record(&target.host, target.port, &fingerprint_bytes(b"fixture"))?;
            return download_with_session(&target, output, control, &session);
        }
    }
    match crate::sftp_live::open_live_session(&target, proxy.as_ref(), control) {
        Ok(session) => return download_with_session(&target, output, control, &session),
        Err(error)
            if proxy.is_none() && target.password.is_empty() && openssh_fallback_ok(&error) => {}
        Err(error) => return Err(error),
    }
    let known_hosts = pin_remote_host(&target.host, target.port)?;
    openssh_get(&target, output, control, &known_hosts)
}

fn openssh_fallback_ok(error: &str) -> bool {
    error.contains("登录失败") || error.contains("连接超时") || error.contains("拒绝连接")
}

pub(crate) fn check_sftp_control(control: &Path) -> Result<(), String> {
    match read_control(control).as_str() {
        "cancel" => Err("canceled".into()),
        "pause" => Err("paused".into()),
        _ => Ok(()),
    }
}

fn openssh_destination(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
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
    let local = output.to_str().ok_or("OpenSSH 无法准确编码本地文件路径")?;
    #[cfg(windows)]
    let local = local.replace('\\', "/");
    // OpenSSH `sftp -b -` cannot verify the remote file's exact `mtime` or
    // size before deciding whether to run `get -a`, and `ls -l` only reports
    // human-formatted timestamps after the batch finishes. Appending to a
    // partial file without verified remote identity risks silently splicing
    // bytes from two different revisions, so this fallback always fetches a
    // clean copy and drops stale resume metadata.
    #[cfg(windows)]
    let local = local.as_str();
    let batch = openssh_batch_script(&target.path, local)?;
    check_sftp_control(control)?;
    for path in [state_path(output).as_path(), output] {
        match fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
    }
    let destination = openssh_destination(&target.host);
    let mut command = Command::new(sftp);
    command
        .args([
            "-P",
            &target.port.to_string(),
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=20",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "VerifyHostKeyDNS=no",
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
            &destination,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    crate::job_object::assign_child(&child);
    // Output from an SSH server is untrusted. Keep diagnostics bounded while
    // draining both pipes so a full pipe cannot block the transfer.
    fn capture(mut pipe: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut prefix = Vec::new();
            let _ = (&mut pipe).take(64 * 1024).read_to_end(&mut prefix);
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
            let _ = tx.send(prefix);
        });
        rx
    }
    let stdout = capture(child.stdout.take().expect("piped stdout"));
    let stderr = capture(child.stderr.take().expect("piped stderr"));
    let status = (|| {
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(batch.as_bytes())
            .map_err(|e| e.to_string())?;
        loop {
            check_sftp_control(control)?;
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Ok::<_, String>(status);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    })();
    let status = status.inspect_err(|_| {
        let _ = child.kill();
        let _ = child.wait();
    })?;
    let wait = std::time::Duration::from_secs(1);
    if !status.success() {
        let bytes = stderr.recv_timeout(wait).unwrap_or_default();
        return Err(format!("sftp failed: {}", String::from_utf8_lossy(&bytes)));
    }
    let listing = stdout.recv_timeout(wait).map_err(|e| e.to_string())?;
    let written = fs::metadata(output)
        .map(|meta| meta.len())
        .map_err(|error| error.to_string())?;
    // The returned byte count is treated as success by every caller, so a short
    // or mis-targeted transfer must surface as an error.  The native path already
    // enforces `stat`; this one reports its own length, which is exactly why it
    // needs the same check before it claims the download fit.
    match parse_ls_size(&String::from_utf8_lossy(&listing), &target.path) {
        Some(remote) if remote == written => {
            let _ = fs::remove_file(state_path(output));
            Ok(remote)
        }
        Some(remote) => Err(format!("文件长度不匹配，期望 {remote}，实际 {written}")),
        None => Err("OpenSSH sftp 未返回远程文件大小，无法校验下载结果".to_string()),
    }
}

fn openssh_batch_script(remote: &str, local: &str) -> Result<String, String> {
    // OpenSSH batch commands are line-oriented. Never silently strip quotes
    // or let a platform-specific local filename introduce another command.
    if [remote, local]
        .iter()
        .any(|s| s.chars().any(char::is_control) || s.contains('"'))
    {
        return Err("OpenSSH 批处理无法准确表示文件路径".into());
    }
    Ok(format!(
        "ls -l \"{remote}\"\nget \"{remote}\" \"{local}\"\n"
    ))
}

/// Parse the remote size out of one `ls -l` line from the OpenSSH sftp client.
///
/// A line looks like `-rw-r--r--    0 1000     1000        12345 Sep 24 15:00
/// file.mp4`: permissions, links, uid, gid, size, then the date.  OpenSSH prints
/// numeric uid/gid, so the size is always whitespace field 4 after the
/// permissions.  Anything whose permissions do not start with `-` is a directory
/// or a symlink, neither of which is a downloadable entity, so such a line is
/// rejected rather than guessed at, and a listing that never mentions the
/// requested path yields `None` so the caller fails closed.
fn parse_ls_size(listing: &str, remote_path: &str) -> Option<u64> {
    listing
        .lines()
        .filter_map(|line| line.strip_suffix(remote_path))
        .filter(|prefix| {
            prefix.ends_with(char::is_whitespace) && prefix.split_whitespace().count() == 8
        })
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let permissions = fields.next()?;
            if !permissions.starts_with('-') {
                return None;
            }
            fields.nth(3)?.parse::<u64>().ok()
        })
}
/// Resolve an OpenSSH tool by name.
///
/// `sftp` and `ssh-keyscan` are spawned with the user's own privileges, so the
/// resolver decides which binary actually runs.  On Windows a PATH-first lookup
/// hands execution to any attacker-writable directory that happens to precede
/// the real OpenSSH, so the admin-writable `%SystemRoot%\System32\OpenSSH` copy
/// is preferred and PATH is only consulted when the tool is not installed
/// there.  That ordering keeps a custom/portable OpenSSH working, which a
/// SystemRoot-only resolver would break.
#[cfg(windows)]
fn which(name: &str) -> Option<PathBuf> {
    let system_root = PathBuf::from(std::env::var_os("SystemRoot")?);
    let preferred = [
        system_root.join("System32").join("OpenSSH"),
        system_root.join("System32"),
    ];
    resolve_in(&preferred, name).or_else(|| resolve_in_path(name))
}

#[cfg(not(windows))]
fn which(name: &str) -> Option<PathBuf> {
    resolve_in_path(name)
}

/// Search a fixed directory list, preferring `<name>.exe` over the bare name.
/// Split out from `which` so directory ordering and the `.exe` preference can
/// be exercised without mutating the process environment.
fn resolve_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|dir| [dir.join(format!("{name}.exe")), dir.join(name)])
        .find(|candidate| candidate.is_file())
}

fn resolve_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    resolve_in(&dirs, name)
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
        assert_eq!(
            target.resource_key(),
            "sftp://lee@nas.local:2222/video/a.mp4"
        );
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
        assert!(!target.resource_key().contains("p@ss"));
        let renewed = parse_sftp_url("sftp://lee:renewed@nas.local/pub/a.bin").unwrap();
        assert_eq!(target.resource_key(), renewed.resource_key());
    }

    #[test]
    fn rejects_invalid_explicit_ports() {
        assert!(parse_sftp_url("sftp://lee@nas.local:not-a-port/pub/a.bin").is_err());
        assert!(parse_sftp_url("sftp://lee@nas.local:0/pub/a.bin").is_err());
    }

    #[test]
    fn percent_decoding_preserves_utf8_userinfo_and_paths() {
        let encoded =
            parse_sftp_url("sftp://%E6%9D%8E:p%E4%B8%AD@nas.local/%E4%B8%AD%E6%96%87.txt").unwrap();
        assert_eq!(encoded.user, "李");
        assert_eq!(encoded.password, "p中");
        assert_eq!(encoded.path, "/中文.txt");

        let raw = parse_sftp_url("sftp://李@nas.local/中文.txt").unwrap();
        assert_eq!(raw.user, "李");
        assert_eq!(raw.path, "/中文.txt");
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
        assert_eq!(
            resume_offset(42, &stat, "sftp://nas.local:22/a.bin", &state),
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
    fn stat_overrun_fails_before_extra_bytes_reach_disk() {
        let root = std::env::temp_dir().join(format!("hls-sftp-stat-limit-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("remote.bin"), b"abc").unwrap();
        let target = parse_sftp_url("sftp://user@nas.local/remote.bin").unwrap();
        let session = FixtureSession { root: root.clone() };
        let output = root.join("out.bin");
        for declared in [0, 2] {
            assert!(transfer_from_session(
                &session,
                &target,
                &output,
                &root.join("control"),
                0,
                declared
            )
            .is_err());
            assert_eq!(fs::metadata(&output).unwrap().len(), 0);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_first_use_pins_only_one_key_and_rejects_a_corrupt_store() {
        let dir = std::env::temp_dir().join(format!("hls-sftp-tofu-lock-{}", std::process::id()));
        let pins = dir.join("known_hosts");
        let barrier = std::sync::Barrier::new(2);
        let attempt = |key: &str| {
            barrier.wait();
            record_host_key(&pins, "nas.local", 22, key).is_ok()
        };
        let accepted = std::thread::scope(|scope| {
            let other = scope.spawn(|| attempt("first"));
            usize::from(attempt("second")) + usize::from(other.join().unwrap())
        });
        assert_eq!(accepted, 1);
        fs::write(&pins, [0xff]).unwrap();
        assert!(record_host_key(&pins, "nas.local", 22, "third").is_err());
        assert_eq!(fs::read(&pins).unwrap(), [0xff]);
        fs::remove_dir_all(dir).unwrap();
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
                resource_key: "sftp://lee@nas.local:22/pub/a.bin".into(),
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
                resource_key: "sftp://lee@nas.local:22/pub/a.bin".into(),
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
    fn changed_account_restarts_instead_of_appending_another_users_file() {
        let root = std::env::temp_dir().join(format!("hls-sftp-account-{}", std::process::id()));
        fs::create_dir_all(root.join("pub")).unwrap();
        fs::write(root.join("pub/a.bin"), b"CCCCDDDD").unwrap();
        let session = FixtureSession { root: root.clone() };
        let stat = session.stat("/pub/a.bin").unwrap();
        let original = parse_sftp_url("sftp://alice:old-password@nas.local/pub/a.bin").unwrap();
        let replacement = parse_sftp_url("sftp://bob:new-password@nas.local/pub/a.bin").unwrap();
        let dest = root.join("payload.downloading");
        fs::write(&dest, b"AAAA").unwrap();
        save_state(
            &state_path(&dest),
            &ResumeState {
                version: STATE_VERSION,
                resource_key: original.resource_key(),
                total: stat.size,
                mtime: stat.mtime,
                offset: 4,
            },
        )
        .unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let bytes = download_with_session(&replacement, &dest, &control, &session).unwrap();
        let actual = fs::read(&dest).unwrap();
        let _ = fs::remove_dir_all(root);
        assert_eq!(bytes, 8);
        assert_eq!(actual, b"CCCCDDDD");
    }

    #[test]
    fn authenticated_proxies_carry_live_ssh_and_sftp() {
        let _guard = env_lock();
        let root = std::env::temp_dir().join(format!("hls-sftp-proxy-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("source.bin"), b"tunneled").unwrap();
        let previous = std::env::var_os("HLS_V7_DATA_DIR");
        std::env::set_var("HLS_V7_DATA_DIR", root.join("data"));
        let server = sftp_loopback::start(root.clone(), "lee", "secret").unwrap();
        for scheme in ["http", "socks5h"] {
            let proxy = crate::proxy_tunnel::tests::TestProxy::start(scheme);
            let url = format!(
                "sftp://lee:secret@origin.invalid:{}/source.bin",
                server.port
            );
            let output = root.join(scheme);
            let size =
                download_sftp_routed(&url, &output, &root.join("control"), &proxy.url).unwrap();
            assert_eq!(size, 8);
            assert_eq!(fs::read(output).unwrap(), b"tunneled");
            assert_eq!(proxy.ports.lock().unwrap().as_slice(), &[server.port]);
        }
        drop(server);
        if let Some(value) = previous {
            std::env::set_var("HLS_V7_DATA_DIR", value);
        } else {
            std::env::remove_var("HLS_V7_DATA_DIR");
        }
        fs::remove_dir_all(root).unwrap();
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
        let session =
            crate::sftp_live::open_live_session(&parse_sftp_url(&url).unwrap(), None, &control)
                .unwrap();
        let stat = session.stat("/pub/a.bin").unwrap();
        save_state(
            &state_path(&dest),
            &ResumeState {
                version: STATE_VERSION,
                resource_key: format!("sftp://lee@127.0.0.1:{}/pub/a.bin", server.port),
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

    // The OpenSSH fallback has no `SftpSession`, so the client's own `ls -l`
    // output is the only authority it has over the remote entity.  A wrong parse
    // here either fails a good download or, worse, accepts a partial or
    // mis-targeted one.
    #[test]
    fn parses_the_size_out_of_an_openssh_ls_line() {
        let line = "-rw-r--r--    0 1000     1000        12345 Sep 24 15:00 /video/a.mp4";
        assert_eq!(parse_ls_size(line, "/video/a.mp4"), Some(12345));
        assert_eq!(parse_ls_size(line, "/video/a.mp"), None);
        // Directories and symlinks are not downloadable entities.
        assert_eq!(
            parse_ls_size(
                "drwxr-xr-x    2 1000     1000         4096 Sep 24 15:00 /video",
                "/video"
            ),
            None
        );
        assert_eq!(
            parse_ls_size(
                "lrwxrwxrwx    1 1000     1000           12 Sep 24 15:00 /video/a.mp4",
                "/video/a.mp4"
            ),
            None
        );
        // A listing that never mentions the requested path must be guessed at by
        // nobody, so the caller fails closed instead of trusting its own bytes.
        assert_eq!(parse_ls_size(line, "/video/other.mp4"), None);
        assert_eq!(
            parse_ls_size(
                "Couldn't stat remote file: No such file or directory",
                "/video/a.mp4"
            ),
            None
        );
    }

    // `resolve_in` decides which binary actually runs with the user's privileges,
    // so its ordering is part of the fix.  The `.exe` preference and the
    // first-match-wins rule are pinned here without touching the real environment.
    #[test]
    fn openssh_resolution_prefers_the_exe_and_the_first_match() {
        let root = std::env::temp_dir().join(format!("hls-which-{}", std::process::id()));
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        // Only the later directory has the tool: the earlier one is skipped.
        fs::write(second.join("sftp.exe"), b"tool").unwrap();
        assert_eq!(
            resolve_in(&[first.clone(), second.clone()], "sftp").as_deref(),
            Some(second.join("sftp.exe").as_path())
        );
        // Now both have it: the first directory wins, and `.exe` beats the bare
        // name inside it even though the bare name is checked first in source.
        fs::write(first.join("sftp"), b"bare").unwrap();
        fs::write(first.join("sftp.exe"), b"exe").unwrap();
        assert_eq!(
            resolve_in(&[first.clone(), second.clone()], "sftp").as_deref(),
            Some(first.join("sftp.exe").as_path())
        );
        // A directory alone is never a match: `is_file` keeps dirs out.
        fs::create_dir_all(first.join("ssh-keyscan")).unwrap();
        assert_eq!(resolve_in(&[first, second], "ssh-keyscan"), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn keyscan_matches_existing_pinned_algorithm_across_reordered_keys() {
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!("hls-sftp-multikey-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var("HLS_V7_DATA_DIR", &dir);
        let rsa_line = "nas.local ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQFixtureRsaKey";
        let ed_line = "nas.local ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureEdKey";
        let rsa_fp = parse_keyscan_line(rsa_line).unwrap().1;
        // Pre-pin the RSA fingerprint (e.g. from native libssh2/russh session).
        tofu_record("nas.local", 22, &rsa_fp).unwrap();
        // ssh-keyscan returns ed25519 first and rsa second: must match pinned rsa key.
        std::env::set_var("HLS_V7_SFTP_KEYSCAN", format!("{ed_line}\n{rsa_line}\n"));
        let ssh_path = pin_remote_host("nas.local", 22).unwrap();
        let stored = fs::read_to_string(ssh_path).unwrap();
        assert!(
            stored.contains("ssh-rsa"),
            "expected pinned rsa line in {stored}"
        );
        // When neither returned key matches the pinned fingerprint, fail closed.
        std::env::set_var(
            "HLS_V7_SFTP_KEYSCAN",
            "nas.local ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMismatchedKey\n",
        );
        assert!(pin_remote_host("nas.local", 22).is_err());
        std::env::remove_var("HLS_V7_SFTP_KEYSCAN");
        std::env::remove_var("HLS_V7_DATA_DIR");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn openssh_fallback_batch_never_appends_without_verified_mtime() {
        assert!(openssh_batch_script("/pub/a.bin", "/tmp/a\nb").is_err());
        assert!(openssh_batch_script("/pub/\"a.bin", "/tmp/a").is_err());
        let script = openssh_batch_script("/pub/a.bin", "/tmp/a.bin").unwrap();
        assert!(script.contains("ls -l \"/pub/a.bin\""));
        assert!(script.contains("\nget \"/pub/a.bin\" \"/tmp/a.bin\"\n"));
        assert!(!script.contains("get -a"));
    }

    #[test]
    fn exact_size_resume_returns_immediately_without_reopening_or_truncating() {
        struct PanicOnOpenSession {
            stat: SftpStat,
        }
        impl SftpSession for PanicOnOpenSession {
            fn stat(&self, _path: &str) -> Result<SftpStat, String> {
                Ok(self.stat.clone())
            }
            fn open(&self, _path: &str) -> Result<Box<dyn SftpFile>, String> {
                panic!("exact-size resume must not reopen or re-download remote file");
            }
        }

        let root = std::env::temp_dir().join(format!(
            "hls-sftp-exact-resume-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let target = parse_sftp_url("sftp://lee@nas.local/pub/exact.bin").unwrap();
        let output = root.join("payload.downloading");
        let payload = b"already-complete-payload";
        fs::write(&output, payload).unwrap();
        let stat = SftpStat {
            size: payload.len() as u64,
            mtime: "1700000000".into(),
        };
        assert_eq!(
            resume_offset(
                payload.len() as u64,
                &stat,
                &target.resource_key(),
                &ResumeState {
                    version: STATE_VERSION,
                    resource_key: target.resource_key(),
                    total: stat.size,
                    mtime: stat.mtime.clone(),
                    offset: stat.size,
                },
            ),
            stat.size
        );
        save_state(
            &state_path(&output),
            &ResumeState {
                version: STATE_VERSION,
                resource_key: target.resource_key(),
                total: stat.size,
                mtime: stat.mtime.clone(),
                offset: stat.size,
            },
        )
        .unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let session = PanicOnOpenSession { stat };
        let got = download_with_session(&target, &output, &control, &session).unwrap();
        assert_eq!(got, payload.len() as u64);
        assert_eq!(fs::read(&output).unwrap(), payload);
        assert!(!state_path(&output).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn periodic_and_read_error_checkpoints_persist_resume_state() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        struct InterruptingFile {
            data: Arc<Vec<u8>>,
            pos: usize,
            fail_at: Option<usize>,
            saw_periodic_checkpoint: Arc<AtomicBool>,
            state_file: PathBuf,
        }
        impl SftpFile for InterruptingFile {
            fn seek(&mut self, offset: u64) -> Result<(), String> {
                self.pos = offset as usize;
                Ok(())
            }
            fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
                if self.pos >= CHECKPOINT_BYTES as usize && self.state_file.exists() {
                    self.saw_periodic_checkpoint.store(true, Ordering::SeqCst);
                }
                if let Some(limit) = self.fail_at {
                    if self.pos >= limit {
                        return Err("simulated SFTP transport disconnect".into());
                    }
                    let remain = (limit - self.pos).min(buf.len());
                    buf[..remain].copy_from_slice(&self.data[self.pos..self.pos + remain]);
                    self.pos += remain;
                    return Ok(remain);
                }
                let remain = (self.data.len() - self.pos).min(buf.len());
                buf[..remain].copy_from_slice(&self.data[self.pos..self.pos + remain]);
                self.pos += remain;
                Ok(remain)
            }
        }
        struct InterruptingSession {
            data: Arc<Vec<u8>>,
            fail_at: Option<usize>,
            saw_periodic_checkpoint: Arc<AtomicBool>,
            state_file: PathBuf,
        }
        impl SftpSession for InterruptingSession {
            fn stat(&self, _path: &str) -> Result<SftpStat, String> {
                Ok(SftpStat {
                    size: self.data.len() as u64,
                    mtime: "1700000123".into(),
                })
            }
            fn open(&self, _path: &str) -> Result<Box<dyn SftpFile>, String> {
                Ok(Box::new(InterruptingFile {
                    data: Arc::clone(&self.data),
                    pos: 0,
                    fail_at: self.fail_at,
                    saw_periodic_checkpoint: Arc::clone(&self.saw_periodic_checkpoint),
                    state_file: self.state_file.clone(),
                }))
            }
        }

        let root = std::env::temp_dir().join(format!(
            "hls-sftp-err-ckpt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let total_len = (CHECKPOINT_BYTES as usize) + 256 * 1024;
        let fail_offset = (CHECKPOINT_BYTES as usize) + 128 * 1024;
        let payload = Arc::new((0..total_len).map(|i| (i % 251) as u8).collect::<Vec<_>>());
        let output = root.join("payload.downloading");
        let state_file = state_path(&output);
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let target = parse_sftp_url("sftp://lee@nas.local/pub/large.bin").unwrap();
        let saw_periodic = Arc::new(AtomicBool::new(false));

        let failing = InterruptingSession {
            data: Arc::clone(&payload),
            fail_at: Some(fail_offset),
            saw_periodic_checkpoint: Arc::clone(&saw_periodic),
            state_file: state_file.clone(),
        };
        let err = download_with_session(&target, &output, &control, &failing).unwrap_err();
        assert!(err.contains("disconnect"), "{err}");
        assert!(
            saw_periodic.load(Ordering::SeqCst),
            "expected periodic checkpoint at >= 4 MiB during transfer"
        );
        let saved = load_state(&state_file);
        assert_eq!(saved.offset, fail_offset as u64);
        assert_eq!(fs::metadata(&output).unwrap().len(), fail_offset as u64);

        // Second attempt resumes cleanly from `fail_offset` and finishes without corrupting bytes.
        let succeeding = InterruptingSession {
            data: Arc::clone(&payload),
            fail_at: None,
            saw_periodic_checkpoint: saw_periodic,
            state_file: state_file.clone(),
        };
        let done = download_with_session(&target, &output, &control, &succeeding).unwrap();
        assert_eq!(done, total_len as u64);
        assert_eq!(fs::read(&output).unwrap(), *payload);
        assert!(!state_file.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parse_sftp_url_supports_case_insensitive_scheme_ipv6_and_strict_host_whitespace() {
        let v6_default = parse_sftp_url(" \u{feff}SFTP://alice@[::1]/pub/video.mp4 ").unwrap();
        assert_eq!(v6_default.host, "::1");
        assert_eq!(v6_default.port, 22);
        assert_eq!(v6_default.user, "alice");
        assert_eq!(v6_default.path, "/pub/video.mp4");
        assert_eq!(
            v6_default.resource_key(),
            "sftp://alice@[::1]:22/pub/video.mp4"
        );
        assert_eq!(openssh_destination(&v6_default.host), "[::1]");
        assert_eq!(tofu_host_key(&v6_default.host, v6_default.port), "[::1]:22");

        let v6_port =
            parse_sftp_url("Sftp://bob:s3cr%3At@[2001:0DB8:0:0::1]:2222/archive.tar").unwrap();
        assert_eq!(v6_port.host, "2001:db8::1");
        assert_eq!(v6_port.port, 2222);
        assert_eq!(v6_port.user, "bob");
        assert_eq!(v6_port.password, "s3cr:t");
        assert_eq!(
            v6_port.resource_key(),
            "sftp://bob@[2001:db8::1]:2222/archive.tar"
        );
        assert_eq!(
            tofu_host_key(&v6_port.host, v6_port.port),
            "[2001:db8::1]:2222"
        );

        for invalid in [
            "sftp://alice@::1/pub/a.bin",
            "sftp://alice@2001:db8::1:22/pub/a.bin",
            "sftp://alice@[::gggg]/pub/a.bin",
            "sftp://alice@[::1/pub/a.bin",
            "sftp://alice@[::1]:0/pub/a.bin",
            "sftp://alice@[::1]:99999/pub/a.bin",
            "sftp://alice@[::1] /pub/a.bin",
            "sftp://alice@ host/pub/a.bin",
            "sftp://alice@ho st/pub/a.bin",
            "sftp://alice@host :22/pub/a.bin",
            "sftp://alice@host\u{00a0}/pub/a.bin",
            "sftp://alice@host\t/pub/a.bin",
        ] {
            assert!(parse_sftp_url(invalid).is_err(), "must reject {invalid:?}");
        }

        // Downloads reject authority whitespace so it can never bypass `net_policy`.
        let root = std::env::temp_dir().join(format!(
            "hls-sftp-ws-guard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let err = download_sftp_routed(
            "sftp://alice:secret@host /file.bin",
            &root.join("out.bin"),
            &control,
            "",
        )
        .unwrap_err();
        assert!(err.contains("主机名"), "{err}");
        let _ = fs::remove_dir_all(root);
    }
}
