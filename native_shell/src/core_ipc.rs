//! Versioned named-pipe transport for the single resident v7 Core.
//!
//! Native Messaging, Compose UI and native presenter must not open SQLite independently in the
//! final product. They connect to this service using a bounded little-endian
//! length frame carrying JSON so protocol traces remain inspectable during the
//! migration. The payload is versioned separately from the legacy shell frame.

use crate::{CoreCommand, EventEnvelope, V7_PROTOCOL_NAME, V7_PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub const V7_PIPE_NAME: &str = r"\\.\pipe\HLSDownloader.v7";
pub const V7_PIPE_MAX_FRAME: usize = 4 * 1024 * 1024;

pub fn v7_pipe_name() -> String {
    std::env::var("HLS_V7_PIPE")
        .or_else(|_| std::env::var("HLS_V6_PIPE"))
        .unwrap_or_else(|_| V7_PIPE_NAME.to_string())
}

/// Loopback TCP is the test/Linux transport. The Windows product talks on the
/// named pipe unless the v7 loopback environment opts back in.
pub fn tcp_loopback_enabled() -> bool {
    if cfg!(not(windows)) {
        return true;
    }
    std::env::var_os("HLS_V7_CORE_TCP").is_some()
        || std::env::var_os("HLS_V7_CORE_BIND").is_some()
        || std::env::var_os("HLS_V6_CORE_TCP").is_some()
        || std::env::var_os("HLS_V6_CORE_BIND").is_some()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CorePipeRequest {
    Hello {
        protocol: String,
        version: u32,
    },
    Command {
        request_id: u64,
        command: CoreCommand,
    },
    Snapshot {
        request_id: u64,
    },
    Capabilities {
        request_id: u64,
    },
    WaitEvents {
        request_id: u64,
        after_sequence: u64,
        timeout_ms: u64,
    },
    StoreSetting {
        request_id: u64,
        key: String,
        value: Value,
    },
    StoreSettings {
        request_id: u64,
        values: std::collections::BTreeMap<String, Value>,
    },
    LoadSettings {
        request_id: u64,
    },
    SetDefaultCookie {
        request_id: u64,
        cookie: String,
    },
    SetSiteRuleCredential {
        request_id: u64,
        host: String,
        #[serde(default)]
        cookie: String,
        #[serde(default)]
        request_headers: std::collections::BTreeMap<String, String>,
        #[serde(default)]
        clear: bool,
    },
    StoreCredential {
        request_id: u64,
        credential_ref: String,
        protected_blob: String,
        kind: String,
    },
    LoadCredential {
        request_id: u64,
        credential_ref: String,
    },
    SaveHandoff {
        request_id: u64,
        handoff_id: String,
        handoff_json: String,
        status: String,
        task_id: Option<String>,
        created_at_ms: u64,
    },
    LoadHandoffs {
        request_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CorePipeResponse {
    Hello {
        protocol: String,
        version: u32,
        pid: u32,
    },
    Events {
        request_id: u64,
        events: Vec<EventEnvelope>,
    },
    Snapshot {
        request_id: u64,
        tasks: Vec<crate::TaskSnapshot>,
        #[serde(default)]
        latest_sequence: u64,
    },
    Capabilities {
        request_id: u64,
        product_version: String,
        protocol_version: u32,
        commands: Vec<String>,
        settings: Vec<String>,
        max_frame_bytes: u64,
    },
    Settings {
        request_id: u64,
        takeover_enabled: bool,
        takeover_minimum_bytes: u64,
        legal_accepted: bool,
        speed_limit_kib: u64,
        #[serde(default)]
        hourly_quota_mib: u64,
        #[serde(default)]
        schedule_enabled: bool,
        #[serde(default = "default_schedule_start")]
        schedule_start: String,
        #[serde(default = "default_schedule_end")]
        schedule_end: String,
        #[serde(default)]
        schedule_kib: u64,
        #[serde(default)]
        auto_category: bool,
        #[serde(default)]
        category_dir_media: String,
        #[serde(default)]
        category_dir_program: String,
        #[serde(default)]
        category_dir_archive: String,
        #[serde(default)]
        category_dir_other: String,
        #[serde(default = "default_queue_max")]
        queue_max: u64,
        #[serde(default)]
        queue_profiles: Vec<crate::QueueProfile>,
        #[serde(default)]
        site_rules: String,
        #[serde(default)]
        av_scan_enabled: bool,
        #[serde(default)]
        av_scan_command: String,
        #[serde(default)]
        torrent_watch: String,
        #[serde(default)]
        torrent_watch_enabled: bool,
        #[serde(default)]
        download_dir: String,
        #[serde(default)]
        temp_dir: String,
        #[serde(default = "default_concurrency")]
        default_concurrency: u64,
        #[serde(default)]
        proxy_url: String,
        #[serde(default)]
        ffmpeg_path: String,
        #[serde(default)]
        clipboard_watch: bool,
        #[serde(default)]
        completion_sound_enabled: bool,
        #[serde(default = "default_true")]
        progress_window_enabled: bool,
        #[serde(default = "default_true")]
        complete_popup_enabled: bool,
        #[serde(default)]
        resume_interrupted: bool,
        #[serde(default)]
        auto_retry_max: u64,
        #[serde(default = "default_rename")]
        existing_file_policy: String,
        #[serde(default)]
        live_record_max_minutes: u64,
        #[serde(default = "default_true")]
        download_subtitles: bool,
        #[serde(default = "default_true")]
        skip_ad_segments: bool,
        #[serde(default)]
        keep_temp_files: bool,
        #[serde(default)]
        default_user_agent: String,
        #[serde(default)]
        tvbox_endpoint: String,
        #[serde(default)]
        dark_mode: bool,
        #[serde(default)]
        allow_duplicate: bool,
        #[serde(default)]
        queue_auto_start_enabled: bool,
        #[serde(default)]
        queue_auto_start_time: String,
        #[serde(default)]
        queue_auto_stop_enabled: bool,
        #[serde(default)]
        queue_auto_stop_time: String,
        #[serde(default)]
        default_referer: String,
        #[serde(default)]
        default_origin: String,
        #[serde(default)]
        allowed_hosts: String,
        #[serde(default = "default_chunk_mb")]
        http_chunk_size_mb: u64,
        #[serde(default = "default_none")]
        completion_power_action: String,
        #[serde(default)]
        start_on_login: bool,
        #[serde(default = "default_queue_days")]
        queue_active_days: String,
        #[serde(default = "default_proxy_mode")]
        proxy_mode: String,
        #[serde(default)]
        proxy_bypass: String,
        #[serde(default)]
        legal_terms_version: String,
        #[serde(default)]
        reduce_motion: bool,
        #[serde(default)]
        harvest_minimum_bytes: u64,
        #[serde(default = "default_true")]
        av_scan_fail_on_threat: bool,
        #[serde(default = "default_bt_upload_limit")]
        bt_upload_limit_kib: u64,
        #[serde(default = "default_bt_connections")]
        bt_max_connections: u64,
        #[serde(default = "default_true")]
        bt_enable_dht: bool,
        #[serde(default)]
        preferred_cast_device_id: String,
        #[serde(default)]
        task_column_layout: String,
        #[serde(default)]
        toolbar_actions: String,
        #[serde(default = "default_task_sort")]
        task_sort: String,
        #[serde(default)]
        default_cookie_configured: bool,
    },
    Credential {
        request_id: u64,
        protected_blob: Option<String>,
    },
    Handoffs {
        request_id: u64,
        items: Vec<String>,
    },
    Error {
        request_id: Option<u64>,
        code: String,
        message: String,
    },
}

fn default_schedule_start() -> String {
    "22:00".into()
}

fn default_schedule_end() -> String {
    "08:00".into()
}

fn default_queue_max() -> u64 {
    3
}

fn default_concurrency() -> u64 {
    12
}

fn default_true() -> bool {
    true
}

fn default_rename() -> String {
    "rename".into()
}

fn default_chunk_mb() -> u64 {
    8
}

fn default_none() -> String {
    "none".into()
}

fn default_queue_days() -> String {
    "1,2,3,4,5,6,7".into()
}

fn default_proxy_mode() -> String {
    "system".into()
}

fn default_task_sort() -> String {
    "queue:asc".into()
}

fn default_bt_upload_limit() -> u64 {
    1024
}

fn default_bt_connections() -> u64 {
    200
}

pub fn encode_message<T: Serialize>(message: &T) -> Result<Vec<u8>, String> {
    let payload = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    if payload.len() > V7_PIPE_MAX_FRAME {
        return Err("v7 Core pipe frame too large".into());
    }
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_message<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> Result<T, String> {
    if frame.len() < 4 {
        return Err("Core pipe frame truncated".into());
    }
    let length = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
    if length > V7_PIPE_MAX_FRAME || frame.len() < length + 4 {
        return Err("v7 Core pipe frame invalid length".into());
    }
    serde_json::from_slice(&frame[4..4 + length])
        .map_err(|error| format!("v7 Core pipe JSON invalid: {error}"))
}

fn read_message<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> Result<Option<T>, String> {
    let mut header = [0u8; 4];
    let mut read = 0;
    while read < header.len() {
        let count = reader
            .read(&mut header[read..])
            .map_err(|error| format!("Core pipe read header: {error}"))?;
        if count == 0 {
            return if read == 0 {
                Ok(None)
            } else {
                Err("Core pipe closed in frame header".into())
            };
        }
        read += count;
    }
    let length = u32::from_le_bytes(header) as usize;
    if length > V7_PIPE_MAX_FRAME {
        return Err("v7 Core pipe frame too large".into());
    }
    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|error| format!("Core pipe read payload: {error}"))?;
    let mut frame = header.to_vec();
    frame.extend_from_slice(&payload);
    decode_message(&frame).map(Some)
}

fn write_message<T: Serialize>(writer: &mut impl Write, message: &T) -> Result<(), String> {
    writer
        .write_all(&encode_message(message)?)
        .and_then(|_| writer.flush())
        .map_err(|error| format!("Core pipe write: {error}"))
}

#[cfg(windows)]
struct OwnerPipeSd(*mut core::ffi::c_void);

#[cfg(windows)]
impl Drop for OwnerPipeSd {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.0);
            }
        }
    }
}

#[cfg(windows)]
fn owner_pipe_sd() -> Option<OwnerPipeSd> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    let sddl: Vec<u16> = "D:P(A;;GA;;;OW)(A;;GA;;;SY)\0".encode_utf16().collect();
    let mut sd = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || sd.is_null() {
        None
    } else {
        Some(OwnerPipeSd(sd))
    }
}

#[cfg(windows)]
pub struct NamedPipeServer {
    name: String,
}

#[cfg(windows)]
impl NamedPipeServer {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    pub fn serve_once<F>(&self, mut handler: F) -> Result<(), String>
    where
        F: FnMut(CorePipeRequest) -> CorePipeResponse,
    {
        let mut stream = self.serve_once_inner()?;
        while let Some(request) = read_message::<CorePipeRequest>(&mut stream)? {
            let response = handler(request);
            write_message(&mut stream, &response)?;
        }
        Ok(())
    }

    pub fn serve_loop(
        &self,
        stop: Arc<AtomicBool>,
        handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync>,
    ) -> Result<(), String> {
        while !stop.load(Ordering::SeqCst) {
            match self.accept() {
                Ok(mut stream) => {
                    let handler = Arc::clone(&handler);
                    thread::spawn(move || {
                        while let Ok(Some(request)) = read_message::<CorePipeRequest>(&mut stream) {
                            let response = handler(request);
                            if write_message(&mut stream, &response).is_err() {
                                break;
                            }
                        }
                    });
                }
                Err(error) => {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    eprintln!("Core named pipe accept: {error}");
                    thread::sleep(Duration::from_millis(40));
                }
            }
        }
        Ok(())
    }

    fn accept(&self) -> Result<std::fs::File, String> {
        self.serve_once_inner()
    }

    fn serve_once_inner(&self) -> Result<std::fs::File, String> {
        use std::fs::File;
        use std::os::windows::io::FromRawHandle;
        use std::os::windows::raw::HANDLE;
        use std::ptr::null_mut;
        use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows_sys::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        };

        let name = wide(&self.name);
        let owner_sd =
            owner_pipe_sd().ok_or_else(|| "named pipe owner DACL unavailable".to_string())?;
        let mut attrs = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>()
                as u32,
            lpSecurityDescriptor: owner_sd.0,
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                V7_PIPE_MAX_FRAME as u32,
                V7_PIPE_MAX_FRAME as u32,
                0,
                &mut attrs,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!("CreateNamedPipeW failed: {}", unsafe {
                GetLastError()
            }));
        }
        let connected = unsafe { ConnectNamedPipe(handle, null_mut()) } != 0;
        if !connected {
            let err = unsafe { GetLastError() };
            if err != 535 {
                unsafe {
                    windows_sys::Win32::Foundation::CloseHandle(handle);
                }
                return Err(format!("ConnectNamedPipe failed: {err}"));
            }
        }
        Ok(unsafe { File::from_raw_handle(handle as HANDLE) })
    }
}

#[cfg(windows)]
pub struct NamedPipeClient {
    stream: std::fs::File,
}

#[cfg(windows)]
impl NamedPipeClient {
    pub fn connect(name: &str, timeout_ms: u32) -> Result<Self, String> {
        use std::fs::File;
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            OPEN_EXISTING,
        };
        use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
        let wide_name = wide(name);
        if unsafe { WaitNamedPipeW(wide_name.as_ptr(), timeout_ms) } == 0 {
            return Err(format!(
                "WaitNamedPipeW failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let handle = unsafe {
            CreateFileW(
                wide_name.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "CreateFileW pipe failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self {
            stream: unsafe { File::from_raw_handle(handle as _) },
        })
    }

    pub fn into_file(self) -> std::fs::File {
        self.stream
    }

    pub fn request(&mut self, request: &CorePipeRequest) -> Result<CorePipeResponse, String> {
        write_message(&mut self.stream, request)?;
        read_message(&mut self.stream)?.ok_or_else(|| "Core pipe closed".into())
    }
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn hello_request() -> CorePipeRequest {
    CorePipeRequest::Hello {
        protocol: V7_PROTOCOL_NAME.into(),
        version: V7_PROTOCOL_VERSION,
    }
}

pub const V7_TCP_PORT: u16 = 18765;
pub fn default_core_bind() -> std::net::SocketAddr {
    if let Ok(raw) =
        std::env::var("HLS_V7_CORE_BIND").or_else(|_| std::env::var("HLS_V6_CORE_BIND"))
    {
        if let Ok(addr) = raw.parse() {
            return addr;
        }
    }
    std::net::SocketAddr::from(([127, 0, 0, 1], V7_TCP_PORT))
}

pub fn serve_tcp_listener(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync>,
) -> Result<(), String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Core listener nonblocking: {error}"))?;
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let handler = Arc::clone(&handler);
                thread::spawn(move || {
                    let _ = stream.set_nonblocking(false);
                    let _ = handle_stream(stream, handler.as_ref());
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("Core accept: {error}")),
        }
    }
    Ok(())
}

fn handle_stream(
    mut stream: TcpStream,
    handler: &dyn Fn(CorePipeRequest) -> CorePipeResponse,
) -> Result<(), String> {
    let _ = stream.set_nodelay(true);
    while let Some(request) = read_message::<CorePipeRequest>(&mut stream)? {
        write_message(&mut stream, &handler(request))?;
    }
    Ok(())
}

pub struct CoreIpcClient {
    transport: IpcTransport,
}

enum IpcTransport {
    Tcp(TcpStream),
    #[cfg(windows)]
    Pipe(std::fs::File),
}

impl IpcTransport {
    fn request(&mut self, request: &CorePipeRequest) -> Result<CorePipeResponse, String> {
        match self {
            Self::Tcp(stream) => {
                write_message(stream, request)?;
                read_message(stream)?.ok_or_else(|| "Core IPC closed".into())
            }
            #[cfg(windows)]
            Self::Pipe(stream) => {
                write_message(stream, request)?;
                read_message(stream)?.ok_or_else(|| "Core pipe closed".into())
            }
        }
    }
}

impl CoreIpcClient {
    pub fn connect_addr(addr: std::net::SocketAddr) -> Result<Self, String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    let mut client = Self {
                        transport: IpcTransport::Tcp(stream),
                    };
                    match client.request(&hello_request())? {
                        CorePipeResponse::Hello {
                            protocol, version, ..
                        } if protocol == V7_PROTOCOL_NAME && version == V7_PROTOCOL_VERSION => {
                            return Ok(client);
                        }
                        other => {
                            return Err(format!("v7 Core hello rejected: {other:?}"));
                        }
                    }
                }
                Err(error) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(format!("v7 Core connect {addr}: {error}"));
                    }
                    thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }

    pub fn connect() -> Result<Self, String> {
        Self::connect_existing(Duration::from_secs(2))
    }

    /// Connect to an already-running Core without imposing the full product
    /// startup wait. Native Messaging uses this short probe before it decides
    /// whether it needs to launch the single-instance engine.
    pub fn connect_existing(timeout: Duration) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let started = std::time::Instant::now();
            let deadline = started + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                let pipe_timeout = remaining.as_millis().clamp(1, 40) as u32;
                match Self::connect_pipe_timeout(&v7_pipe_name(), pipe_timeout) {
                    Ok(client) => return Ok(client),
                    Err(error) if std::time::Instant::now() >= deadline => {
                        if !tcp_loopback_enabled() {
                            return Err(error);
                        }
                        break;
                    }
                    Err(_) => {}
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(10).min(remaining));
            }
        }
        Self::connect_addr(default_core_bind())
    }

    #[cfg(windows)]
    pub fn connect_pipe(name: &str) -> Result<Self, String> {
        Self::connect_pipe_timeout(name, 2_000)
    }

    #[cfg(windows)]
    fn connect_pipe_timeout(name: &str, timeout_ms: u32) -> Result<Self, String> {
        let pipe = NamedPipeClient::connect(name, timeout_ms)?;
        let mut client = Self {
            transport: IpcTransport::Pipe(pipe.into_file()),
        };
        match client.request(&hello_request())? {
            CorePipeResponse::Hello {
                protocol, version, ..
            } if protocol == V7_PROTOCOL_NAME && version == V7_PROTOCOL_VERSION => Ok(client),
            other => Err(format!("v7 Core pipe hello rejected: {other:?}")),
        }
    }

    pub fn request(&mut self, request: &CorePipeRequest) -> Result<CorePipeResponse, String> {
        self.transport.request(request)
    }

    pub fn command(&mut self, command: CoreCommand) -> Result<Vec<EventEnvelope>, String> {
        match self.request(&CorePipeRequest::Command {
            request_id: 1,
            command,
        })? {
            CorePipeResponse::Events { events, .. } => Ok(events),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected Core command response: {other:?}")),
        }
    }

    pub fn snapshot(&mut self) -> Result<Vec<crate::TaskSnapshot>, String> {
        self.snapshot_state().map(|(tasks, _)| tasks)
    }

    pub fn snapshot_state(&mut self) -> Result<(Vec<crate::TaskSnapshot>, u64), String> {
        match self.request(&CorePipeRequest::Snapshot { request_id: 1 })? {
            CorePipeResponse::Snapshot {
                tasks,
                latest_sequence,
                ..
            } => Ok((tasks, latest_sequence)),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected Core snapshot: {other:?}")),
        }
    }

    pub fn wait_events(
        &mut self,
        after_sequence: u64,
        timeout_ms: u64,
    ) -> Result<Vec<EventEnvelope>, String> {
        match self.request(&CorePipeRequest::WaitEvents {
            request_id: 1,
            after_sequence,
            timeout_ms,
        })? {
            CorePipeResponse::Events { events, .. } => Ok(events),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected Core wait: {other:?}")),
        }
    }

    pub fn load_settings(&mut self) -> Result<CorePipeResponse, String> {
        self.request(&CorePipeRequest::LoadSettings { request_id: 1 })
    }

    pub fn store_setting(&mut self, key: &str, value: Value) -> Result<(), String> {
        match self.request(&CorePipeRequest::StoreSetting {
            request_id: 1,
            key: key.into(),
            value,
        })? {
            CorePipeResponse::Settings { .. } | CorePipeResponse::Events { .. } => Ok(()),
            CorePipeResponse::Error { message, .. } => Err(message),
            _ => Ok(()),
        }
    }

    pub fn store_credential(
        &mut self,
        credential_ref: &str,
        protected_blob: &str,
        kind: &str,
    ) -> Result<(), String> {
        match self.request(&CorePipeRequest::StoreCredential {
            request_id: 1,
            credential_ref: credential_ref.into(),
            protected_blob: protected_blob.into(),
            kind: kind.into(),
        })? {
            CorePipeResponse::Error { message, .. } => Err(message),
            _ => Ok(()),
        }
    }

    pub fn load_credential(&mut self, credential_ref: &str) -> Result<Option<String>, String> {
        match self.request(&CorePipeRequest::LoadCredential {
            request_id: 1,
            credential_ref: credential_ref.into(),
        })? {
            CorePipeResponse::Credential { protected_blob, .. } => Ok(protected_blob),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected credential response: {other:?}")),
        }
    }

    pub fn save_handoff(
        &mut self,
        handoff_id: &str,
        handoff_json: &str,
        status: &str,
        task_id: Option<&str>,
        created_at_ms: u64,
    ) -> Result<(), String> {
        match self.request(&CorePipeRequest::SaveHandoff {
            request_id: 1,
            handoff_id: handoff_id.into(),
            handoff_json: handoff_json.into(),
            status: status.into(),
            task_id: task_id.map(str::to_string),
            created_at_ms,
        })? {
            CorePipeResponse::Error { message, .. } => Err(message),
            _ => Ok(()),
        }
    }

    pub fn load_handoffs(&mut self) -> Result<Vec<String>, String> {
        match self.request(&CorePipeRequest::LoadHandoffs { request_id: 1 })? {
            CorePipeResponse::Handoffs { items, .. } => Ok(items),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected handoff response: {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, ResourceKind, TaskSpec};

    #[test]
    fn pipe_frame_roundtrip_preserves_versioned_command() {
        let request = CorePipeRequest::Command {
            request_id: 7,
            command: CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://example.test/file.bin".into(),
                    resource_kind: ResourceKind::File,
                    title: "File".into(),
                    filename: "file.bin".into(),
                    download_dir: String::new(),
                    request_method: "GET".into(),
                    credential_ref: None,
                    replay_context_ref: None,
                    concurrency: 4,
                    checksum: None,
                    expected_size: None,
                    etag: String::new(),
                    last_modified: String::new(),
                    ..Default::default()
                },
            },
        };
        let decoded: CorePipeRequest = decode_message(&encode_message(&request).unwrap()).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn hello_uses_the_v7_protocol_identity() {
        assert_eq!(
            hello_request(),
            CorePipeRequest::Hello {
                protocol: V7_PROTOCOL_NAME.into(),
                version: V7_PROTOCOL_VERSION,
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_tcp_loopback_is_opt_in() {
        if std::env::var_os("HLS_V6_CORE_TCP").is_some()
            || std::env::var_os("HLS_V6_CORE_BIND").is_some()
        {
            return;
        }
        assert!(!tcp_loopback_enabled());
    }

    #[test]
    fn adversarial_pipe_frame_rejects_oversize_and_truncated() {
        let huge = (V7_PIPE_MAX_FRAME as u32 + 1).to_le_bytes();
        let mut frame = huge.to_vec();
        frame.extend_from_slice(&[0u8; 8]);
        let err = decode_message::<CorePipeRequest>(&frame).unwrap_err();
        assert!(err.contains("invalid length") || err.contains("too large"));
        assert!(decode_message::<CorePipeRequest>(&[1, 0]).is_err());
        let hello = encode_message(&hello_request()).unwrap();
        let decoded: CorePipeRequest = decode_message(&hello).unwrap();
        assert_eq!(decoded, hello_request());
    }
}
