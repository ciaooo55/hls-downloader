//! Loopback russh+SFTP server for in-process client tests.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use russh::keys::PrivateKey;
use russh::server::{Auth, Handler as ServerHandler, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, File as SftpName, FileAttributes, Handle, Name, Status, StatusCode, Version,
};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const LOOPBACK_HOST_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACA1Aedkt4FMirH+9FdB6r/iDnkE6uNyz6cIZ+cm9KZ5egAAAJhkZwkkZGcJ
JAAAAAtzc2gtZWQyNTUxOQAAACA1Aedkt4FMirH+9FdB6r/iDnkE6uNyz6cIZ+cm9KZ5eg
AAAECGA4wHJNIxRouF3K6ifsQ64I9q4FwE8C60TbCvLwEYZTUB52S3gUyKsf70V0Hqv+IO
eQTq43LPpwhn5yb0pnl6AAAAFGhscy12Ni1zZnRwLWxvb3BiYWNrAQ==
-----END OPENSSH PRIVATE KEY-----
";

pub(super) struct Loopback {
    pub port: u16,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Loopback {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
struct ServerState {
    root: PathBuf,
    user: String,
    password: String,
}

struct Server {
    state: ServerState,
}

struct SshSession {
    state: ServerState,
    channels: Arc<tokio::sync::Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

struct FileSftp {
    root: PathBuf,
    files: HashMap<String, PathBuf>,
}

impl russh::server::Server for Server {
    type Handler = SshSession;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        SshSession {
            state: self.state.clone(),
            channels: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
}

impl ServerHandler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == self.state.user && password == self.state.password {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let id = channel.id();
        self.channels.lock().await.insert(id, channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let Some(channel) = self.channels.lock().await.remove(&channel_id) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        session.channel_success(channel_id)?;
        let sftp = FileSftp {
            root: self.state.root.clone(),
            files: HashMap::new(),
        };
        russh_sftp::server::run(channel.into_stream(), sftp).await;
        Ok(())
    }
}

impl russh_sftp::server::Handler for FileSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.attrs(id, &path)
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.attrs(id, &path)
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let path = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        let meta = fs::metadata(path).map_err(|_| StatusCode::Failure)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![SftpName::dummy(&path)],
        })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        _pflags: russh_sftp::protocol::OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.resolve(&filename);
        let meta = fs::metadata(&path).map_err(|_| StatusCode::NoSuchFile)?;
        if meta.is_dir() {
            return Err(StatusCode::Failure);
        }
        let handle = format!("h{id}");
        self.files.insert(handle.clone(), path);
        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.files.remove(&handle);
        Ok(ok_status(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let path = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        let mut file = File::open(path).map_err(|_| StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| StatusCode::Failure)?;
        let mut buf = vec![0u8; len as usize];
        let count = file.read(&mut buf).map_err(|_| StatusCode::Failure)?;
        if count == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(count);
        Ok(Data { id, data: buf })
    }
}

impl FileSftp {
    fn resolve(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    fn attrs(&self, id: u32, path: &str) -> Result<Attrs, StatusCode> {
        let meta = fs::metadata(self.resolve(path)).map_err(|_| StatusCode::NoSuchFile)?;
        if meta.is_dir() {
            return Err(StatusCode::Failure);
        }
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }
}

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

pub(super) fn start(root: PathBuf, user: &str, password: &str) -> Result<Loopback, String> {
    let key = PrivateKey::from_openssh(LOOPBACK_HOST_KEY).map_err(|error| error.to_string())?;
    let config = russh::server::Config {
        auth_rejection_time: Duration::from_millis(20),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        keys: vec![key],
        ..Default::default()
    };
    let state = ServerState {
        root,
        user: user.to_string(),
        password: password.to_string(),
    };
    let (port_tx, port_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = oneshot::channel();
    let thread = thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let Ok(listener) = TcpListener::bind("127.0.0.1:0").await else {
                return;
            };
            let Ok(addr) = listener.local_addr() else {
                return;
            };
            let _ = port_tx.send(addr.port());
            let mut server = Server { state };
            let running = server.run_on_socket(Arc::new(config), &listener);
            tokio::select! {
                _ = running => {}
                _ = stop_rx => {}
            }
        });
    });
    let port = port_rx
        .recv_timeout(Duration::from_secs(8))
        .map_err(|_| "loopback SFTP server failed to bind".to_string())?;
    Ok(Loopback {
        port,
        stop: Some(stop_tx),
        thread: Some(thread),
    })
}
