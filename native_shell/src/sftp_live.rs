//! In-process SSH+SFTP session. Matches paramiko in `sftp_file.py`:
//! password (or default keys), TOFU host key, STAT, then seek.

use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg, PublicKey};
use russh_sftp::client::SftpSession as RusshSftp;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::sftp_engine::{
    fingerprint_bytes, map_sftp_io, SftpFile, SftpSession, SftpStat, SftpTarget,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) struct LiveSession {
    rt: tokio::runtime::Runtime,
    sftp: RusshSftp,
    _ssh: Handle<TofuHandler>,
}

struct LiveFile {
    handle: tokio::runtime::Handle,
    file: russh_sftp::client::fs::File,
}

struct TofuHandler {
    host: String,
    port: u16,
    error: Arc<Mutex<Option<String>>>,
}

impl client::Handler for TofuHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        match crate::sftp_engine::tofu_record(
            &self.host,
            self.port,
            &fingerprint_host_key(server_public_key),
        ) {
            Ok(()) => Ok(true),
            Err(message) => {
                if let Ok(mut slot) = self.error.lock() {
                    *slot = Some(message);
                }
                Ok(false)
            }
        }
    }
}

pub(crate) fn fingerprint_host_key(key: &PublicKey) -> String {
    match key.to_bytes() {
        Ok(bytes) => fingerprint_bytes(&bytes),
        Err(_) => fingerprint_bytes(key.to_openssh().unwrap_or_default().as_bytes()),
    }
}

pub(crate) fn fingerprint_openssh_material(algorithm: &str, material: &str) -> String {
    let encoded = format!("{algorithm} {material}");
    match PublicKey::from_openssh(&encoded) {
        Ok(key) => fingerprint_host_key(&key),
        Err(_) => fingerprint_bytes(material.as_bytes()),
    }
}

fn map_ssh_error(error: impl std::fmt::Display) -> String {
    let text = error.to_string();
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("auth")
        || lowered.contains("permission denied")
        || lowered.contains("password")
    {
        "SFTP 登录失败，请检查用户名、密码或私钥".into()
    } else if lowered.contains("timed out") || lowered.contains("timeout") {
        "SFTP 连接超时".into()
    } else if lowered.contains("connection refused") || lowered.contains("os error 10061") {
        "SFTP 服务器拒绝连接".into()
    } else if lowered.contains("no such file") || lowered.contains("not found") {
        "SFTP 远程文件不存在".into()
    } else {
        format!("SFTP 下载失败：{}", text.chars().take(200).collect::<String>())
    }
}

impl SftpFile for LiveFile {
    fn seek(&mut self, offset: u64) -> Result<(), String> {
        self.handle
            .block_on(self.file.seek(SeekFrom::Start(offset)))
            .map(|_| ())
            .map_err(map_sftp_io)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        self.handle
            .block_on(self.file.read(buf))
            .map_err(map_sftp_io)
    }
}

impl SftpSession for LiveSession {
    fn stat(&self, path: &str) -> Result<SftpStat, String> {
        let meta = self
            .rt
            .block_on(self.sftp.metadata(path.to_string()))
            .map_err(map_sftp_io)?;
        if meta.is_dir() {
            return Err("SFTP 地址必须指向具体文件，不能是目录".into());
        }
        let mtime = meta
            .mtime
            .map(|value| value.to_string())
            .unwrap_or_else(|| "0".into());
        Ok(SftpStat {
            size: meta.len(),
            mtime,
        })
    }

    fn open(&self, path: &str) -> Result<Box<dyn SftpFile>, String> {
        let file = self
            .rt
            .block_on(self.sftp.open(path.to_string()))
            .map_err(map_sftp_io)?;
        Ok(Box::new(LiveFile {
            handle: self.rt.handle().clone(),
            file,
        }))
    }
}

pub(crate) fn open_live_session(target: &SftpTarget) -> Result<LiveSession, String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let tofu_error = Arc::new(Mutex::new(None));
    let handler = TofuHandler {
        host: target.host.clone(),
        port: target.port,
        error: tofu_error.clone(),
    };
    let opened = rt.block_on(connect_and_open(target, handler));
    if let Some(message) = tofu_error.lock().ok().and_then(|slot| slot.clone()) {
        return Err(message);
    }
    let (sftp, ssh) = opened?;
    Ok(LiveSession {
        rt,
        sftp,
        _ssh: ssh,
    })
}

async fn connect_and_open(
    target: &SftpTarget,
    handler: TofuHandler,
) -> Result<(RusshSftp, Handle<TofuHandler>), String> {
    let config = russh::client::Config {
        inactivity_timeout: Some(Duration::from_secs(90)),
        ..Default::default()
    };
    let mut session = tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(
            Arc::new(config),
            (target.host.as_str(), target.port),
            handler,
        ),
    )
    .await
    .map_err(|_| "SFTP 连接超时".to_string())?
    .map_err(map_ssh_error)?;

    if !target.password.is_empty() {
        let result = session
            .authenticate_password(target.user.clone(), target.password.clone())
            .await
            .map_err(map_ssh_error)?;
        if !result.success() {
            return Err("SFTP 登录失败，请检查用户名、密码或私钥".into());
        }
    } else if !authenticate_default_keys(&mut session, &target.user).await?
        && !authenticate_agent(&mut session, &target.user).await?
    {
        return Err("SFTP 登录失败，请检查用户名、密码或私钥".into());
    }

    let channel = session
        .channel_open_session()
        .await
        .map_err(map_ssh_error)?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(map_ssh_error)?;
    let sftp = RusshSftp::new(channel.into_stream())
        .await
        .map_err(map_sftp_io)?;
    Ok((sftp, session))
}

async fn authenticate_default_keys(
    session: &mut Handle<TofuHandler>,
    user: &str,
) -> Result<bool, String> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    if home.is_empty() {
        return Ok(false);
    }
    let ssh_dir = PathBuf::from(home).join(".ssh");
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let path = ssh_dir.join(name);
        if !path.is_file() {
            continue;
        }
        let Ok(key) = load_secret_key(&path, None) else {
            continue;
        };
        let hash = session
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten();
        let wrapped = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        if session
            .authenticate_publickey(user, wrapped)
            .await
            .map(|result| result.success())
            .unwrap_or(false)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn authenticate_agent(
    session: &mut Handle<TofuHandler>,
    user: &str,
) -> Result<bool, String> {
    #[cfg(windows)]
    {
        use russh::keys::agent::client::AgentClient;
        use russh::keys::agent::AgentIdentity;
        let Ok(mut agent) = AgentClient::connect_pageant().await else {
            return Ok(false);
        };
        let Ok(identities) = agent.request_identities().await else {
            return Ok(false);
        };
        let hash = session
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten();
        for identity in identities {
            if let AgentIdentity::PublicKey { key, .. } = identity {
                if session
                    .authenticate_publickey_with(user, key, hash, &mut agent)
                    .await
                    .map(|result| result.success())
                    .unwrap_or(false)
                {
                    return Ok(true);
                }
            }
        }
    }
    let _ = (session, user);
    Ok(false)
}
