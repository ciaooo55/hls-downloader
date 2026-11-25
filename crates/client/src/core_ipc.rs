//! Versioned named-pipe transport for the single resident v7 Core.
//!
//! Native Messaging and native UI must not open SQLite independently in the
//! final product. They connect to this service using a bounded little-endian
//! length frame carrying versioned JSON.

use crate::{CoreCommand, EventEnvelope, V7_PROTOCOL_NAME, V7_PROTOCOL_VERSION};
pub use hls_protocol::ipc::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// Clients keep pooled connections idle for at most 60 seconds. Keep the server
// bound finite without invalidating a connection that the client may still reuse.
const V7_IDLE_READ_TIMEOUT: Duration = Duration::from_secs(120);
const V7_FRAME_READ_TIMEOUT: Duration = Duration::from_secs(15);
const V7_HELLO_MIN_TIMEOUT: Duration = Duration::from_millis(1_500);
const V7_HELLO_MAX_TIMEOUT: Duration = Duration::from_millis(5_000);
// 报完协议违规后收连接前，等对端把残留输入收尾的最长时间。见 linger_close。
const V7_LINGER_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

fn hello_handshake_timeout(open_timeout: Duration) -> Duration {
    open_timeout.clamp(V7_HELLO_MIN_TIMEOUT, V7_HELLO_MAX_TIMEOUT)
}

// Fallback wakeup interval for legacy callers that only flip `stop: AtomicBool`
// without signaling `NamedPipeServer`'s manual-reset stop event handle.
#[cfg(windows)]
const V7_ACCEPT_STOP_CHECK_MS: u32 = 50;

// 每连接一线程模型下的并发连接上限。命名管道在接收前等待槽位，
// TCP 对超限连接立即关闭；客户端连接过程受自身超时约束。
pub const MAX_IPC_CONNECTIONS: usize = 64;

struct ConnectionSlots {
    active: Mutex<usize>,
    freed: Condvar,
}

impl ConnectionSlots {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            active: Mutex::new(0),
            freed: Condvar::new(),
        })
    }

    fn try_claim(self: &Arc<Self>) -> Option<ConnectionSlot> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if *active < MAX_IPC_CONNECTIONS {
            *active += 1;
            Some(ConnectionSlot(Arc::clone(self)))
        } else {
            None
        }
    }

    #[cfg(windows)]
    fn claim_or_stop(self: &Arc<Self>, is_stopped: impl Fn() -> bool) -> Option<ConnectionSlot> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if is_stopped() {
                return None;
            }
            if *active < MAX_IPC_CONNECTIONS {
                *active += 1;
                return Some(ConnectionSlot(Arc::clone(self)));
            }
            let (guard, _) = self
                .freed
                .wait_timeout(
                    active,
                    Duration::from_millis(u64::from(V7_ACCEPT_STOP_CHECK_MS)),
                )
                .unwrap_or_else(|e| e.into_inner());
            active = guard;
        }
    }

    #[cfg(windows)]
    fn notify_all(&self) {
        let _guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        self.freed.notify_all();
    }

    #[cfg(all(test, windows))]
    fn count(&self) -> usize {
        *self.active.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// 声明一个连接线程槽位；Drop 保证 panic 路径也不泄漏计数并立即唤醒等待者。
struct ConnectionSlot(Arc<ConnectionSlots>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        let mut active = self.0.active.lock().unwrap_or_else(|e| e.into_inner());
        *active = active.saturating_sub(1);
        self.0.freed.notify_one();
    }
}

pub fn v7_pipe_name() -> String {
    std::env::var("HLS_V7_PIPE").unwrap_or_else(|_| V7_PIPE_NAME.to_string())
}

/// Loopback TCP is the test/Linux transport. The Windows product talks on the
/// named pipe unless the v7 loopback environment opts back in.
pub fn tcp_loopback_enabled() -> bool {
    if cfg!(not(windows)) {
        return true;
    }
    std::env::var_os("HLS_V7_CORE_TCP").is_some() || std::env::var_os("HLS_V7_CORE_BIND").is_some()
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
        return Err("Core pipe frame too large".into());
    }
    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|error| format!("Core pipe read payload: {error}"))?;
    let mut frame = header.to_vec();
    frame.extend_from_slice(&payload);
    decode_message(&frame).map(Some)
}

#[cfg(windows)]
struct OwnedWinHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
// SAFETY: `OwnedWinHandle` owns a Win32 kernel event or named-pipe `HANDLE`; transferring exclusive
// handle ownership to another thread is supported by the Windows kernel.
unsafe impl Send for OwnedWinHandle {}
#[cfg(windows)]
// SAFETY: Shared references to `OwnedWinHandle` only expose thread-safe kernel event signaling
// (`SetEvent`/`ResetEvent`) and raw handle inspection while `Drop` requires `&mut self`.
unsafe impl Sync for OwnedWinHandle {}

#[cfg(windows)]
impl OwnedWinHandle {
    fn from_raw(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<Self, String> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(format!(
                "invalid Win32 handle: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(Self(handle))
        }
    }

    fn new_manual_reset_event() -> Result<Self, String> {
        use windows_sys::Win32::System::Threading::CreateEventW;
        // SAFETY: Null security attributes and null name are valid optional inputs to `CreateEventW`
        // for creating an anonymous manual-reset (`bManualReset == 1`), initially non-signaled event.
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        Self::from_raw(handle).map_err(|error| format!("CreateEventW failed: {error}"))
    }

    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0
    }

    fn set_event(&self) {
        // SAFETY: `self.0` was validated non-null/non-`INVALID_HANDLE_VALUE` on construction and
        // remains open until `Drop`; `SetEvent` is thread-safe on manual-reset event handles.
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.0);
        }
    }

    fn reset_event(&self) -> Result<(), String> {
        // SAFETY: `self.0` is a live manual-reset event handle owned by `self`.
        let ok = unsafe { windows_sys::Win32::System::Threading::ResetEvent(self.0) };
        if ok == 0 {
            Err(format!(
                "ResetEvent failed: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
impl Drop for OwnedWinHandle {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: `self` is the sole owner of `self.0` and all pending overlapped operations
            // using it complete before `OverlappedPipe`/`NamedPipeServer` drops `OwnedWinHandle`.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Immediate shutdown signal for [`NamedPipeServer`], backed by a Windows
/// manual-reset event handle so idle overlapped `ConnectNamedPipe` and idle
/// connection reads wake without polling delays.
#[cfg(windows)]
#[derive(Clone)]
pub struct NamedPipeStopHandle {
    signaled: Arc<AtomicBool>,
    event: Option<Arc<OwnedWinHandle>>,
    slots: Arc<ConnectionSlots>,
}

#[cfg(windows)]
impl NamedPipeStopHandle {
    pub fn signal(&self) {
        self.signaled.store(true, Ordering::SeqCst);
        if let Some(event) = &self.event {
            event.set_event();
        }
        self.slots.notify_all();
    }

    fn is_signaled(&self) -> bool {
        self.signaled.load(Ordering::SeqCst)
    }

    fn event_raw(&self) -> Option<windows_sys::Win32::Foundation::HANDLE> {
        self.event.as_ref().map(|event| event.raw())
    }
}

#[cfg(windows)]
enum OverlappedOutcome {
    Completed(u32),
    TimedOut,
    Stopped,
    Eof,
    OsError(u32),
}

/// Completes a pending Win32 overlapped operation (`ERROR_IO_PENDING`), enforcing
/// strict cancellation safety: whenever a wait times out, stops, or fails, this
/// function ALWAYS calls `CancelIoEx` followed by `GetOverlappedResult(..., TRUE)`
/// before returning so the kernel never touches stack `OVERLAPPED` or I/O buffers
/// after they are dropped.
#[cfg(windows)]
fn finish_pending_overlapped(
    handle: windows_sys::Win32::Foundation::HANDLE,
    io_event: windows_sys::Win32::Foundation::HANDLE,
    overlapped: &mut windows_sys::Win32::System::IO::OVERLAPPED,
    deadline: Option<Instant>,
    stop_event: Option<windows_sys::Win32::Foundation::HANDLE>,
    stop_flag: Option<&AtomicBool>,
    poll_interval_ms: Option<u32>,
) -> OverlappedOutcome {
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_BROKEN_PIPE, ERROR_MORE_DATA, ERROR_OPERATION_ABORTED,
        ERROR_PIPE_NOT_CONNECTED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        WaitForMultipleObjects, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult};

    let mut cancel_reason: Option<OverlappedOutcome> = None;

    loop {
        if stop_flag.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            cancel_reason = Some(OverlappedOutcome::Stopped);
            break;
        }

        let wait_ms = match deadline {
            Some(dl) => {
                let now = Instant::now();
                if now >= dl {
                    cancel_reason = Some(OverlappedOutcome::TimedOut);
                    break;
                }
                let remaining = dl.saturating_duration_since(now);
                let rem_ms = remaining.as_millis().max(1).min((INFINITE - 1) as u128) as u32;
                match poll_interval_ms {
                    Some(poll) => rem_ms.min(poll),
                    None => rem_ms,
                }
            }
            None => poll_interval_ms.unwrap_or(INFINITE),
        };

        let wait_result = match stop_event {
            Some(stop_h) => {
                let handles = [io_event, stop_h];
                // SAFETY: Both `io_event` and `stop_h` are live manual-reset event handles owned
                // by the caller/stop-handle for the duration of this call, and `handles` has len 2.
                unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, wait_ms) }
            }
            // SAFETY: `io_event` is a live manual-reset event handle owned by the caller.
            None => unsafe { WaitForSingleObject(io_event, wait_ms) },
        };

        if wait_result == WAIT_OBJECT_0 {
            break;
        }
        if stop_event.is_some() && wait_result == WAIT_OBJECT_0 + 1 {
            cancel_reason = Some(OverlappedOutcome::Stopped);
            break;
        }
        if wait_result == WAIT_TIMEOUT {
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    cancel_reason = Some(OverlappedOutcome::TimedOut);
                    break;
                }
            }
            continue;
        }
        if wait_result == WAIT_FAILED {
            // SAFETY: `GetLastError` reads thread-local Win32 error state without pointer arguments.
            let err = unsafe { GetLastError() };
            cancel_reason = Some(OverlappedOutcome::OsError(err));
            break;
        }
        cancel_reason = Some(OverlappedOutcome::OsError(wait_result));
        break;
    }

    if cancel_reason.is_some() {
        // SAFETY: `handle` is the live overlapped pipe handle and `overlapped` points to the exact
        // `OVERLAPPED` structure passed to the in-flight `ReadFile`/`WriteFile`/`ConnectNamedPipe`.
        unsafe {
            CancelIoEx(handle, overlapped);
        }
    }

    let mut transferred = 0u32;
    // SAFETY: Passing `bWait = 1` (`TRUE`) blocks until the kernel completes the overlapped IRP
    // (whether naturally or via `CancelIoEx` above), guaranteeing the kernel never accesses stack
    // `overlapped` or caller I/O buffers after `finish_pending_overlapped` returns.
    let ok = unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 1) };
    if ok != 0 {
        if matches!(cancel_reason, Some(OverlappedOutcome::Stopped)) {
            return OverlappedOutcome::Stopped;
        }
        return OverlappedOutcome::Completed(transferred);
    }

    // SAFETY: `GetLastError` reads thread-local Win32 error state after `GetOverlappedResult`.
    let err = unsafe { GetLastError() };
    if err == ERROR_MORE_DATA {
        if matches!(cancel_reason, Some(OverlappedOutcome::Stopped)) {
            return OverlappedOutcome::Stopped;
        }
        return OverlappedOutcome::Completed(transferred);
    }
    if err == ERROR_OPERATION_ABORTED {
        if let Some(reason) = cancel_reason {
            return reason;
        }
    }
    if err == ERROR_BROKEN_PIPE || err == ERROR_PIPE_NOT_CONNECTED {
        return OverlappedOutcome::Eof;
    }
    OverlappedOutcome::OsError(err)
}

#[cfg(windows)]
struct OverlappedPipe {
    handle: OwnedWinHandle,
    io_event: OwnedWinHandle,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
}

#[cfg(windows)]
impl OverlappedPipe {
    fn new(handle: OwnedWinHandle) -> Result<Self, String> {
        let io_event = OwnedWinHandle::new_manual_reset_event()?;
        Ok(Self {
            handle,
            io_event,
            read_timeout: None,
            write_timeout: None,
        })
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.read_timeout = timeout;
    }

    fn set_write_timeout(&mut self, timeout: Option<Duration>) {
        self.write_timeout = timeout;
    }

    fn read_once(
        &mut self,
        buf: &mut [u8],
        deadline: Option<Instant>,
        stop_handle: Option<&NamedPipeStopHandle>,
    ) -> OverlappedOutcome {
        use windows_sys::Win32::Foundation::{
            GetLastError, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_MORE_DATA,
            ERROR_PIPE_NOT_CONNECTED,
        };
        use windows_sys::Win32::Storage::FileSystem::ReadFile;
        use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

        if buf.is_empty() {
            return OverlappedOutcome::Completed(0);
        }
        if stop_handle.is_some_and(NamedPipeStopHandle::is_signaled) {
            return OverlappedOutcome::Stopped;
        }
        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                return OverlappedOutcome::TimedOut;
            }
        }
        if self.io_event.reset_event().is_err() {
            // SAFETY: `GetLastError` reads thread-local Win32 error state after `ResetEvent`.
            return OverlappedOutcome::OsError(unsafe { GetLastError() });
        }

        // SAFETY: `OVERLAPPED` is a plain C struct valid when zero-initialized with `hEvent` bound
        // to `self.io_event` (uniquely used via `&mut self`).
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = self.io_event.raw();
        let to_read = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: `self.handle` is a live `FILE_FLAG_OVERLAPPED` pipe handle, `buf` is writable for
        // `to_read` bytes, `lpNumberOfBytesRead` is null as recommended for overlapped I/O, and
        // both `buf` and `overlapped` stay alive until synchronous completion or `finish_pending_overlapped`
        // awaits kernel IRP completion via `GetOverlappedResult(..., 1)`.
        let ok = unsafe {
            ReadFile(
                self.handle.raw(),
                buf.as_mut_ptr(),
                to_read,
                std::ptr::null_mut(),
                &mut overlapped,
            )
        };
        if ok != 0 {
            let mut transferred = 0u32;
            // SAFETY: `ReadFile` completed synchronously on `self.handle` with `overlapped`.
            let got =
                unsafe { GetOverlappedResult(self.handle.raw(), &overlapped, &mut transferred, 1) };
            if got != 0 {
                return OverlappedOutcome::Completed(transferred);
            }
            // SAFETY: `GetLastError` reads thread-local Win32 error state.
            let err = unsafe { GetLastError() };
            if err == ERROR_MORE_DATA {
                return OverlappedOutcome::Completed(transferred);
            }
            if err == ERROR_BROKEN_PIPE || err == ERROR_PIPE_NOT_CONNECTED {
                return OverlappedOutcome::Eof;
            }
            return OverlappedOutcome::OsError(err);
        }

        // SAFETY: `GetLastError` reads thread-local Win32 error state immediately after `ReadFile`.
        let err = unsafe { GetLastError() };
        match err {
            ERROR_IO_PENDING => finish_pending_overlapped(
                self.handle.raw(),
                self.io_event.raw(),
                &mut overlapped,
                deadline,
                stop_handle.and_then(NamedPipeStopHandle::event_raw),
                None,
                None,
            ),
            ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED => OverlappedOutcome::Eof,
            ERROR_MORE_DATA => {
                let mut transferred = 0u32;
                // SAFETY: `ERROR_MORE_DATA` indicates synchronous partial completion; `bWait = 1`
                // reads the completed byte count from `overlapped`.
                let _ = unsafe {
                    GetOverlappedResult(self.handle.raw(), &overlapped, &mut transferred, 1)
                };
                OverlappedOutcome::Completed(transferred)
            }
            other => OverlappedOutcome::OsError(other),
        }
    }

    fn write_once(&mut self, buf: &[u8], deadline: Option<Instant>) -> OverlappedOutcome {
        use windows_sys::Win32::Foundation::{
            GetLastError, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA,
            ERROR_PIPE_NOT_CONNECTED,
        };
        use windows_sys::Win32::Storage::FileSystem::WriteFile;
        use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

        if buf.is_empty() {
            return OverlappedOutcome::Completed(0);
        }
        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                return OverlappedOutcome::TimedOut;
            }
        }
        if self.io_event.reset_event().is_err() {
            // SAFETY: `GetLastError` reads thread-local Win32 error state after `ResetEvent`.
            return OverlappedOutcome::OsError(unsafe { GetLastError() });
        }

        // SAFETY: `OVERLAPPED` is a plain C struct valid when zero-initialized with `hEvent` bound
        // to `self.io_event` (uniquely used via `&mut self`).
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = self.io_event.raw();
        let to_write = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: `self.handle` is a live `FILE_FLAG_OVERLAPPED` pipe handle, `buf` is readable for
        // `to_write` bytes, and both `buf` and `overlapped` outlive the kernel IRP via either
        // synchronous completion or `finish_pending_overlapped`'s `GetOverlappedResult(..., 1)`.
        let ok = unsafe {
            WriteFile(
                self.handle.raw(),
                buf.as_ptr(),
                to_write,
                std::ptr::null_mut(),
                &mut overlapped,
            )
        };
        if ok != 0 {
            let mut transferred = 0u32;
            // SAFETY: `WriteFile` completed synchronously on `self.handle` with `overlapped`.
            let got =
                unsafe { GetOverlappedResult(self.handle.raw(), &overlapped, &mut transferred, 1) };
            if got != 0 {
                return OverlappedOutcome::Completed(transferred);
            }
            // SAFETY: `GetLastError` reads thread-local Win32 error state.
            let err = unsafe { GetLastError() };
            if err == ERROR_BROKEN_PIPE || err == ERROR_PIPE_NOT_CONNECTED || err == ERROR_NO_DATA {
                return OverlappedOutcome::Eof;
            }
            return OverlappedOutcome::OsError(err);
        }

        // SAFETY: `GetLastError` reads thread-local Win32 error state immediately after `WriteFile`.
        let err = unsafe { GetLastError() };
        match err {
            ERROR_IO_PENDING => finish_pending_overlapped(
                self.handle.raw(),
                self.io_event.raw(),
                &mut overlapped,
                deadline,
                None,
                None,
                None,
            ),
            ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA => OverlappedOutcome::Eof,
            other => OverlappedOutcome::OsError(other),
        }
    }

    fn write_all_deadline(
        &mut self,
        data: &[u8],
        deadline: Option<Instant>,
    ) -> std::io::Result<()> {
        let mut offset = 0;
        while offset < data.len() {
            match self.write_once(&data[offset..], deadline) {
                OverlappedOutcome::Completed(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "Core pipe write returned zero bytes",
                    ));
                }
                OverlappedOutcome::Completed(written) => offset += written as usize,
                OverlappedOutcome::Eof => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "Core pipe closed",
                    ));
                }
                OverlappedOutcome::TimedOut => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Core pipe write timed out",
                    ));
                }
                OverlappedOutcome::Stopped => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "Core pipe stopped",
                    ));
                }
                OverlappedOutcome::OsError(code) => {
                    return Err(std::io::Error::from_raw_os_error(code as i32));
                }
            }
        }
        Ok(())
    }

    fn write_message_timeout<T: Serialize>(
        &mut self,
        message: &T,
        timeout: Duration,
    ) -> Result<(), String> {
        let encoded = encode_message(message)?;
        let deadline = Instant::now() + timeout;
        self.write_all_deadline(&encoded, Some(deadline))
            .map_err(|error| format!("Core pipe write: {error}"))
    }

    fn read_message_timeout<T: for<'de> Deserialize<'de>>(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<T>, String> {
        let deadline = Instant::now() + timeout;
        let mut header = [0u8; 4];
        let mut offset = 0;
        while offset < header.len() {
            match self.read_once(&mut header[offset..], Some(deadline), None) {
                OverlappedOutcome::Completed(0) | OverlappedOutcome::Eof => {
                    return if offset == 0 {
                        Ok(None)
                    } else {
                        Err("Core pipe closed in frame header".into())
                    };
                }
                OverlappedOutcome::Completed(count) => offset += count as usize,
                OverlappedOutcome::TimedOut => {
                    return Err(format!(
                        "Core pipe read timed out after {} ms",
                        timeout.as_millis()
                    ));
                }
                OverlappedOutcome::Stopped => return Ok(None),
                OverlappedOutcome::OsError(code) => {
                    let error = std::io::Error::from_raw_os_error(code as i32);
                    return Err(format!("Core pipe read header: {error}"));
                }
            }
        }
        let length = u32::from_le_bytes(header) as usize;
        if length > V7_PIPE_MAX_FRAME {
            return Err("Core pipe frame too large".into());
        }
        let mut payload = vec![0u8; length];
        let mut offset = 0;
        while offset < length {
            match self.read_once(&mut payload[offset..], Some(deadline), None) {
                OverlappedOutcome::Completed(0) | OverlappedOutcome::Eof => {
                    return Err("Core pipe closed during frame payload".into());
                }
                OverlappedOutcome::Completed(count) => offset += count as usize,
                OverlappedOutcome::TimedOut => {
                    return Err(format!(
                        "Core pipe read timed out after {} ms",
                        timeout.as_millis()
                    ));
                }
                OverlappedOutcome::Stopped => return Ok(None),
                OverlappedOutcome::OsError(code) => {
                    let error = std::io::Error::from_raw_os_error(code as i32);
                    return Err(format!("Core pipe read payload: {error}"));
                }
            }
        }
        let mut frame = header.to_vec();
        frame.extend_from_slice(&payload);
        decode_message(&frame).map(Some)
    }
}

#[cfg(windows)]
impl Read for OverlappedPipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let deadline = self.read_timeout.map(|timeout| Instant::now() + timeout);
        match self.read_once(buf, deadline, None) {
            OverlappedOutcome::Completed(count) => Ok(count as usize),
            OverlappedOutcome::Eof | OverlappedOutcome::Stopped => Ok(0),
            OverlappedOutcome::TimedOut => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Core pipe read timed out",
            )),
            OverlappedOutcome::OsError(code) => Err(std::io::Error::from_raw_os_error(code as i32)),
        }
    }
}

#[cfg(windows)]
impl Write for OverlappedPipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let deadline = self.write_timeout.map(|timeout| Instant::now() + timeout);
        match self.write_once(buf, deadline) {
            OverlappedOutcome::Completed(count) => Ok(count as usize),
            OverlappedOutcome::Eof => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Core pipe closed",
            )),
            OverlappedOutcome::TimedOut => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Core pipe write timed out",
            )),
            OverlappedOutcome::Stopped => Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Core pipe stopped",
            )),
            OverlappedOutcome::OsError(code) => Err(std::io::Error::from_raw_os_error(code as i32)),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let deadline = self.write_timeout.map(|timeout| Instant::now() + timeout);
        self.write_all_deadline(buf, deadline)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Overlapped WriteFile already commits bytes into the kernel pipe buffer;
        // do not call synchronous FlushFileBuffers, which blocks until the peer reads.
        Ok(())
    }
}

#[cfg(windows)]
fn read_server_pipe_message<T: for<'de> Deserialize<'de>>(
    pipe: &mut OverlappedPipe,
    stop_handle: &NamedPipeStopHandle,
    idle_timeout: Duration,
    frame_timeout: Duration,
) -> Result<Option<T>, String> {
    // True overlapped ReadFile wakes immediately when data arrives or stop fires,
    // without PeekNamedPipe polling sleeps.
    let mut header_deadline = Instant::now() + idle_timeout;
    let mut header_timeout_budget = idle_timeout;
    let mut header = [0u8; 4];
    let mut offset = 0;
    while offset < header.len() {
        match pipe.read_once(
            &mut header[offset..],
            Some(header_deadline),
            Some(stop_handle),
        ) {
            OverlappedOutcome::Completed(0) | OverlappedOutcome::Eof => {
                return if offset == 0 {
                    Ok(None)
                } else {
                    Err("Core pipe closed in frame header".into())
                };
            }
            OverlappedOutcome::Completed(count) => {
                if offset == 0 {
                    // Once the first header byte arrives, tighten the remaining header
                    // deadline to the frame budget so partial headers cannot pin a slot
                    // for the full idle timeout.
                    let tightened = Instant::now() + frame_timeout;
                    if tightened < header_deadline {
                        header_deadline = tightened;
                        header_timeout_budget = frame_timeout;
                    }
                }
                offset += count as usize;
            }
            OverlappedOutcome::Stopped => return Ok(None),
            OverlappedOutcome::TimedOut => {
                return Err(format!(
                    "Core pipe frame read timed out after {} ms",
                    header_timeout_budget.as_millis()
                ));
            }
            OverlappedOutcome::OsError(code) => {
                let error = std::io::Error::from_raw_os_error(code as i32);
                return Err(format!("Core pipe read header: {error}"));
            }
        }
    }
    let length = u32::from_le_bytes(header) as usize;
    if length > V7_PIPE_MAX_FRAME {
        return Err("Core pipe frame too large".into());
    }
    // A complete header starts one absolute body budget; progress cannot renew it.
    let body_deadline = Instant::now() + frame_timeout;
    let mut payload = vec![0u8; length];
    let mut offset = 0;
    while offset < length {
        match pipe.read_once(
            &mut payload[offset..],
            Some(body_deadline),
            Some(stop_handle),
        ) {
            OverlappedOutcome::Completed(0) | OverlappedOutcome::Eof => {
                return Err("Core pipe closed during frame payload".into());
            }
            OverlappedOutcome::Completed(count) => offset += count as usize,
            OverlappedOutcome::Stopped => return Ok(None),
            OverlappedOutcome::TimedOut => {
                return Err(format!(
                    "Core pipe frame read timed out after {} ms",
                    frame_timeout.as_millis()
                ));
            }
            OverlappedOutcome::OsError(code) => {
                let error = std::io::Error::from_raw_os_error(code as i32);
                return Err(format!("Core pipe read payload: {error}"));
            }
        }
    }
    let mut frame = header.to_vec();
    frame.extend_from_slice(&payload);
    decode_message(&frame).map(Some)
}

fn read_server_tcp_message<T: for<'de> Deserialize<'de>>(
    stream: &mut TcpStream,
) -> Result<Option<T>, String> {
    let mut header = [0u8; 4];
    let header_deadline = std::time::Instant::now() + V7_IDLE_READ_TIMEOUT;
    let mut header_offset = 0;
    while header_offset < header.len() {
        let remaining = header_deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "Core stream header read timed out after {} ms",
                V7_IDLE_READ_TIMEOUT.as_millis()
            ));
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|error| format!("Core stream idle timeout: {error}"))?;
        match stream.read(&mut header[header_offset..]) {
            Ok(0) if header_offset == 0 => return Ok(None),
            Ok(0) => return Err("Core stream closed during frame header".into()),
            Ok(read) => header_offset += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Core stream read header: {error}")),
        }
    }
    let length = u32::from_le_bytes(header) as usize;
    if length > V7_PIPE_MAX_FRAME {
        return Err("Core pipe frame too large".into());
    }

    let deadline = std::time::Instant::now() + V7_FRAME_READ_TIMEOUT;
    let mut payload = vec![0u8; length];
    let mut offset = 0;
    while offset < length {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "Core stream frame read timed out after {} ms",
                V7_FRAME_READ_TIMEOUT.as_millis()
            ));
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|error| format!("Core stream frame timeout: {error}"))?;
        match stream.read(&mut payload[offset..]) {
            Ok(0) => return Err("Core stream closed during frame payload".into()),
            Ok(read) => offset += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Core stream read payload: {error}")),
        }
    }
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
            // SAFETY: `self.0` was allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`
            // via `LocalAlloc` and is freed once when `OwnerPipeSd` drops after `serve_loop_inner` exits.
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
    // SAFETY: `sddl` is a static NUL-terminated UTF-16 SDDL string, `1` is `SDDL_REVISION_1`, and
    // `&mut sd` receives a `LocalAlloc`-backed security descriptor freed by `OwnerPipeSd::drop`.
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
enum PipeAcceptOutcome {
    Connected(OverlappedPipe),
    ClientDisconnectedEarly,
    Stopped,
}

#[cfg(windows)]
enum PipeAcceptError {
    StartupFailed(String),
    Transient(String),
}

#[cfg(windows)]
pub struct NamedPipeServer {
    name: String,
    idle_read_timeout: Duration,
    frame_read_timeout: Duration,
    stop_event: Option<Arc<OwnedWinHandle>>,
    stop_signaled: Arc<AtomicBool>,
    slots: Arc<ConnectionSlots>,
}

#[cfg(windows)]
impl NamedPipeServer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            idle_read_timeout: V7_IDLE_READ_TIMEOUT,
            frame_read_timeout: V7_FRAME_READ_TIMEOUT,
            stop_event: OwnedWinHandle::new_manual_reset_event().ok().map(Arc::new),
            stop_signaled: Arc::new(AtomicBool::new(false)),
            slots: ConnectionSlots::new(),
        }
    }

    /// Returns a cloneable stop handle that sets this server's manual-reset
    /// Windows stop event so an idle `ConnectNamedPipe` wakes immediately.
    pub fn stop_handle(&self) -> NamedPipeStopHandle {
        NamedPipeStopHandle {
            signaled: Arc::clone(&self.stop_signaled),
            event: self.stop_event.as_ref().map(Arc::clone),
            slots: Arc::clone(&self.slots),
        }
    }

    /// Signals this server's manual-reset stop event immediately.
    pub fn signal_stop(&self) {
        self.stop_handle().signal();
    }

    #[cfg(test)]
    fn with_timeouts(mut self, idle_read_timeout: Duration, frame_read_timeout: Duration) -> Self {
        self.idle_read_timeout = idle_read_timeout;
        self.frame_read_timeout = frame_read_timeout;
        self
    }

    #[cfg(test)]
    fn claimed_slots(&self) -> usize {
        self.slots.count()
    }

    fn is_stopped(&self, stop: &AtomicBool) -> bool {
        stop.load(Ordering::SeqCst) || self.stop_signaled.load(Ordering::SeqCst)
    }

    fn wait_backoff_or_stop(&self, stop: &AtomicBool, millis: u32) {
        if self.is_stopped(stop) {
            return;
        }
        if let Some(event) = &self.stop_event {
            // SAFETY: `event.raw()` is a live manual-reset event handle owned by `self.stop_event`.
            unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(event.raw(), millis);
            }
        } else {
            thread::sleep(Duration::from_millis(u64::from(millis)));
        }
    }

    pub fn serve_loop(
        &self,
        stop: Arc<AtomicBool>,
        handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync>,
    ) -> Result<(), String> {
        self.serve_loop_inner(stop, handler, None)
    }

    pub fn serve_loop_with_ready(
        &self,
        stop: Arc<AtomicBool>,
        handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync>,
        ready: std::sync::mpsc::SyncSender<Result<(), String>>,
    ) -> Result<(), String> {
        self.serve_loop_inner(stop, handler, Some(ready))
    }

    fn serve_loop_inner(
        &self,
        stop: Arc<AtomicBool>,
        handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync>,
        mut ready: Option<std::sync::mpsc::SyncSender<Result<(), String>>>,
    ) -> Result<(), String> {
        struct ServerExitGuard(NamedPipeStopHandle);
        impl Drop for ServerExitGuard {
            fn drop(&mut self) {
                self.0.signal();
            }
        }

        if self.is_stopped(&stop) {
            let error = "named pipe startup canceled before ready".to_string();
            if let Some(ready) = ready {
                let _ = ready.send(Err(error.clone()));
            }
            return Err(error);
        }

        let owner_sd = match owner_pipe_sd() {
            Some(value) => value,
            None => {
                let error = "named pipe owner DACL unavailable".to_string();
                if let Some(ready) = ready {
                    let _ = ready.send(Err(error.clone()));
                }
                return Err(error);
            }
        };
        let connect_event = match OwnedWinHandle::new_manual_reset_event() {
            Ok(event) => event,
            Err(error) => {
                if let Some(ready) = ready {
                    let _ = ready.send(Err(error.clone()));
                }
                return Err(error);
            }
        };
        let wide_name = match checked_wide(&self.name) {
            Ok(wide_name) => wide_name,
            Err(reason) => {
                let error = format!("CreateNamedPipeW failed: {reason}");
                if let Some(ready) = ready {
                    let _ = ready.send(Err(error.clone()));
                }
                return Err(error);
            }
        };
        let _exit_guard = ServerExitGuard(self.stop_handle());
        let mut is_first_instance = true;

        while !self.is_stopped(&stop) {
            // Reserve capacity before accepting. Recently closed clients may still
            // occupy reader threads; accepting then dropping loses valid requests.
            let Some(slot) = self.slots.claim_or_stop(|| self.is_stopped(&stop)) else {
                return Ok(());
            };
            let first_ready = ready.take();
            let first_instance = is_first_instance;
            match self.serve_once_inner(
                &wide_name,
                &owner_sd,
                &connect_event,
                first_ready.as_ref(),
                first_instance,
                &stop,
            ) {
                Ok(PipeAcceptOutcome::Connected(stream)) => {
                    is_first_instance = false;
                    if self.is_stopped(&stop) {
                        break;
                    }
                    let handler = Arc::clone(&handler);
                    let stop = Arc::clone(&stop);
                    let stop_handle = self.stop_handle();
                    let idle_timeout = self.idle_read_timeout;
                    let frame_timeout = self.frame_read_timeout;
                    thread::spawn(move || {
                        let _slot = slot;
                        handle_pipe_stream(
                            stream,
                            handler.as_ref(),
                            &stop,
                            &stop_handle,
                            idle_timeout,
                            frame_timeout,
                        );
                    });
                }
                Ok(PipeAcceptOutcome::ClientDisconnectedEarly) => {
                    is_first_instance = false;
                }
                Ok(PipeAcceptOutcome::Stopped) => {
                    break;
                }
                Err(PipeAcceptError::StartupFailed(error)) => {
                    return Err(error);
                }
                Err(PipeAcceptError::Transient(error)) => {
                    if self.is_stopped(&stop) {
                        break;
                    }
                    eprintln!("Core named pipe accept: {error}");
                    self.wait_backoff_or_stop(&stop, 40);
                }
            }
        }
        Ok(())
    }

    fn serve_once_inner(
        &self,
        wide_name: &[u16],
        owner_sd: &OwnerPipeSd,
        connect_event: &OwnedWinHandle,
        ready: Option<&std::sync::mpsc::SyncSender<Result<(), String>>>,
        first_instance: bool,
        stop: &AtomicBool,
    ) -> Result<PipeAcceptOutcome, PipeAcceptError> {
        use windows_sys::Win32::Foundation::{
            GetLastError, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
            ERROR_PIPE_NOT_CONNECTED, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
        };
        use windows_sys::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        };
        use windows_sys::Win32::System::IO::OVERLAPPED;

        let attrs = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>()
                as u32,
            lpSecurityDescriptor: owner_sd.0,
            bInheritHandle: 0,
        };
        let open_mode = if first_instance {
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED
        };
        // SAFETY: `wide_name` is a valid NUL-terminated UTF-16 pipe name without interior NULs, and
        // `attrs` references `owner_sd.0`, which remains alive across `CreateNamedPipeW`.
        let raw_handle = unsafe {
            CreateNamedPipeW(
                wide_name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                V7_PIPE_MAX_FRAME as u32,
                V7_PIPE_MAX_FRAME as u32,
                0,
                &attrs,
            )
        };
        if raw_handle == INVALID_HANDLE_VALUE {
            // SAFETY: `GetLastError` reads thread-local Win32 error state after `CreateNamedPipeW`.
            let last_error = unsafe { GetLastError() };
            let error = format!("CreateNamedPipeW failed: {last_error}");
            if let Some(ready) = ready {
                let _ = ready.send(Err(error.clone()));
            }
            return Err(if ready.is_some() {
                PipeAcceptError::StartupFailed(error)
            } else {
                PipeAcceptError::Transient(error)
            });
        }
        let handle = OwnedWinHandle::from_raw(raw_handle)
            .map_err(|error| PipeAcceptError::Transient(format!("CreateNamedPipeW: {error}")))?;

        connect_event.reset_event().map_err(|error| {
            if let Some(ready) = ready {
                let _ = ready.send(Err(error.clone()));
            }
            if ready.is_some() {
                PipeAcceptError::StartupFailed(error)
            } else {
                PipeAcceptError::Transient(error)
            }
        })?;

        // SAFETY: `OVERLAPPED` is valid when zero-initialized with `hEvent` set to `connect_event`.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = connect_event.raw();
        // SAFETY: `handle` is a live `FILE_FLAG_OVERLAPPED` server pipe handle; when `ERROR_IO_PENDING`
        // is returned, `finish_pending_overlapped` cancels and awaits `GetOverlappedResult(..., 1)`
        // before `overlapped` or `handle` can be dropped.
        let connected = unsafe { ConnectNamedPipe(handle.raw(), &mut overlapped) } != 0;
        if connected {
            if let Some(ready) = ready {
                let _ = ready.send(Ok(()));
            }
            let pipe = OverlappedPipe::new(handle).map_err(PipeAcceptError::Transient)?;
            return Ok(PipeAcceptOutcome::Connected(pipe));
        }

        // SAFETY: `GetLastError` reads thread-local Win32 error state after `ConnectNamedPipe`.
        let err = unsafe { GetLastError() };
        match err {
            ERROR_PIPE_CONNECTED => {
                if let Some(ready) = ready {
                    let _ = ready.send(Ok(()));
                }
                let pipe = OverlappedPipe::new(handle).map_err(PipeAcceptError::Transient)?;
                Ok(PipeAcceptOutcome::Connected(pipe))
            }
            ERROR_IO_PENDING => {
                if let Some(ready) = ready {
                    let _ = ready.send(Ok(()));
                }
                let stop_event_raw = self.stop_event.as_ref().map(|event| event.raw());
                match finish_pending_overlapped(
                    handle.raw(),
                    connect_event.raw(),
                    &mut overlapped,
                    None,
                    stop_event_raw,
                    Some(stop),
                    Some(V7_ACCEPT_STOP_CHECK_MS),
                ) {
                    OverlappedOutcome::Completed(_) => {
                        let pipe =
                            OverlappedPipe::new(handle).map_err(PipeAcceptError::Transient)?;
                        Ok(PipeAcceptOutcome::Connected(pipe))
                    }
                    OverlappedOutcome::Stopped => Ok(PipeAcceptOutcome::Stopped),
                    OverlappedOutcome::Eof => Ok(PipeAcceptOutcome::ClientDisconnectedEarly),
                    OverlappedOutcome::TimedOut => Err(PipeAcceptError::Transient(
                        "ConnectNamedPipe unexpectedly timed out".into(),
                    )),
                    OverlappedOutcome::OsError(code)
                        if code == ERROR_NO_DATA
                            || code == ERROR_BROKEN_PIPE
                            || code == ERROR_PIPE_NOT_CONNECTED =>
                    {
                        Ok(PipeAcceptOutcome::ClientDisconnectedEarly)
                    }
                    OverlappedOutcome::OsError(code) => Err(PipeAcceptError::Transient(format!(
                        "ConnectNamedPipe failed: {code}"
                    ))),
                }
            }
            ERROR_NO_DATA | ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED => {
                if let Some(ready) = ready {
                    let _ = ready.send(Ok(()));
                }
                Ok(PipeAcceptOutcome::ClientDisconnectedEarly)
            }
            other => {
                let error = format!("ConnectNamedPipe failed: {other}");
                if let Some(ready) = ready {
                    let _ = ready.send(Err(error.clone()));
                    return Err(PipeAcceptError::StartupFailed(error));
                }
                Err(PipeAcceptError::Transient(error))
            }
        }
    }
}

#[cfg(windows)]
fn handle_pipe_stream(
    mut stream: OverlappedPipe,
    handler: &(dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync),
    stop: &AtomicBool,
    stop_handle: &NamedPipeStopHandle,
    idle_timeout: Duration,
    frame_timeout: Duration,
) {
    stream.set_write_timeout(Some(V7_FRAME_READ_TIMEOUT));
    loop {
        if stop.load(Ordering::SeqCst) || stop_handle.is_signaled() {
            break;
        }
        match read_server_pipe_message::<CorePipeRequest>(
            &mut stream,
            stop_handle,
            idle_timeout,
            frame_timeout,
        ) {
            Ok(Some(request)) => {
                if stop.load(Ordering::SeqCst) || stop_handle.is_signaled() {
                    break;
                }
                let result = dispatch_and_reply(&mut stream, request, handler, stop);
                if stop.load(Ordering::SeqCst) {
                    stop_handle.signal();
                }
                if result.is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                stream.set_write_timeout(Some(V7_LINGER_CLOSE_TIMEOUT));
                report_protocol_violation(&mut stream, &error);
                break;
            }
        }
    }
}

#[cfg(windows)]
fn token_user_sid(token: windows_sys::Win32::Foundation::HANDLE) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::{
        GetLengthSid, GetTokenInformation, IsValidSid, TokenUser, TOKEN_USER,
    };
    let mut size = 0u32;
    // SAFETY: Null output buffer with length `0` queries the required byte size in `size` for the
    // open `TOKEN_QUERY` handle `token`.
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
    }
    if size < std::mem::size_of::<TOKEN_USER>() as u32 {
        return None;
    }
    let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    let capacity_bytes = buffer.len() * std::mem::size_of::<usize>();
    // SAFETY: `buffer` is pointer-aligned (`usize`) and holds at least `size` writable bytes.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: `GetTokenInformation` populated `buffer` with a valid, pointer-aligned `TOKEN_USER`.
    let user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    if user.User.Sid.is_null() {
        return None;
    }
    // SAFETY: `user.User.Sid` points into `buffer`; `IsValidSid` verifies SID header/revision
    // validity as required before calling `GetLengthSid`.
    let valid_sid = unsafe { IsValidSid(user.User.Sid) } != 0;
    if !valid_sid {
        return None;
    }
    // SAFETY: `IsValidSid` succeeded for `user.User.Sid`.
    let sid_len = unsafe { GetLengthSid(user.User.Sid) } as usize;
    let buf_start = buffer.as_ptr() as usize;
    let sid_start = user.User.Sid as usize;
    if sid_len == 0
        || sid_start < buf_start
        || sid_start.saturating_add(sid_len) > buf_start.saturating_add(capacity_bytes)
    {
        return None;
    }
    // SAFETY: `user.User.Sid` is non-null, validated by `IsValidSid`, and its `sid_len` bytes lie
    // wholly within the initialized `buffer` allocation.
    let sid = unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), sid_len) };
    Some(sid.to_vec())
}

#[cfg(windows)]
fn verify_pipe_server_owner(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<(), String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::TOKEN_QUERY;
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let mut server_pid = 0u32;
    // SAFETY: `handle` is an open client named-pipe handle and `&mut server_pid` is writable.
    let got_pid = unsafe { GetNamedPipeServerProcessId(handle, &mut server_pid) };
    if got_pid == 0 || server_pid == 0 {
        return Err(format!(
            "GetNamedPipeServerProcessId failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    if server_pid == std::process::id() {
        return Ok(());
    }
    let mut current_token = std::ptr::null_mut();
    // SAFETY: `GetCurrentProcess()` returns the current process pseudo-handle, and `&mut current_token`
    // receives an owned `TOKEN_QUERY` handle closed immediately after reading its user SID.
    let opened_current =
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut current_token) };
    if opened_current == 0 {
        return Err("Unable to inspect current process token".into());
    }
    let current_sid = token_user_sid(current_token);
    // SAFETY: `current_token` was opened by `OpenProcessToken` above and is closed once here.
    unsafe { CloseHandle(current_token) };
    let current_sid = current_sid.ok_or_else(|| "Unable to read current user SID".to_string())?;

    // SAFETY: Opens a non-inheritable query handle to `server_pid`, or returns null on failure.
    let server_process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, server_pid) };
    if server_process.is_null() {
        return Err(format!(
            "Core pipe server PID {server_pid} owner verification failed"
        ));
    }
    let mut server_token = std::ptr::null_mut();
    // SAFETY: `server_process` is non-null and closed immediately after `OpenProcessToken`.
    let opened = unsafe { OpenProcessToken(server_process, TOKEN_QUERY, &mut server_token) } != 0;
    // SAFETY: `server_process` is uniquely owned here and closed once.
    unsafe { CloseHandle(server_process) };
    if !opened {
        return Err(format!(
            "Core pipe server PID {server_pid} token unavailable"
        ));
    }
    let server_sid = token_user_sid(server_token);
    // SAFETY: `server_token` was opened by a succeeded `OpenProcessToken` call and closed once here.
    unsafe { CloseHandle(server_token) };
    if server_sid.as_deref() != Some(current_sid.as_slice()) {
        return Err("Core pipe server belongs to a different user".into());
    }
    Ok(())
}

#[cfg(windows)]
pub struct NamedPipeClient {
    stream: OverlappedPipe,
}

#[cfg(windows)]
impl NamedPipeClient {
    pub fn connect(name: &str, timeout_ms: u32) -> Result<Self, String> {
        use windows_sys::Win32::Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ,
            FILE_GENERIC_WRITE, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
        };
        use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
        let wide_name =
            checked_wide(name).map_err(|reason| format!("CreateFileW pipe failed: {reason}"))?;
        let started = Instant::now();
        loop {
            // SAFETY: `wide_name` is a valid NUL-terminated UTF-16 pipe path without interior NULs;
            // `CreateFileW` returns `INVALID_HANDLE_VALUE` on failure or a fresh overlapped handle.
            let raw_handle = unsafe {
                CreateFileW(
                    wide_name.as_ptr(),
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL
                        | FILE_FLAG_OVERLAPPED
                        | SECURITY_SQOS_PRESENT
                        | SECURITY_IDENTIFICATION,
                    std::ptr::null_mut(),
                )
            };
            if raw_handle != INVALID_HANDLE_VALUE {
                let handle = OwnedWinHandle::from_raw(raw_handle)?;
                verify_pipe_server_owner(handle.raw())?;
                return Ok(Self {
                    stream: OverlappedPipe::new(handle)?,
                });
            }
            let mut error = std::io::Error::last_os_error();
            let mut operation = "CreateFileW pipe";
            let remaining = timeout_ms
                .saturating_sub(started.elapsed().as_millis().min(u32::MAX as u128) as u32);
            // WaitNamedPipe 不预留实例；其他客户端可能抢先打开，必须在同一总时限内重试。
            if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && remaining > 0 {
                // SAFETY: `wide_name` is a valid NUL-terminated UTF-16 pipe path live across the call.
                if unsafe { WaitNamedPipeW(wide_name.as_ptr(), remaining) } != 0 {
                    continue;
                }
                error = std::io::Error::last_os_error();
                operation = "WaitNamedPipeW";
            }
            let remaining = timeout_ms
                .saturating_sub(started.elapsed().as_millis().min(u32::MAX as u128) as u32);
            if error.raw_os_error() != Some(ERROR_FILE_NOT_FOUND as i32) || remaining == 0 {
                return Err(format!("{operation} failed: {error}"));
            }
            // 最后一个旧实例关闭到服务端创建下一实例之间，WaitNamedPipe 会立即返回找不到文件。
            // 等待原期限内的新实例，不能在这个窗口提前报错，也不能空转占满 CPU。
            thread::sleep(Duration::from_millis(u64::from(remaining.min(5))));
        }
    }

    fn into_stream(self) -> OverlappedPipe {
        self.stream
    }

    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.stream.set_read_timeout(timeout);
    }

    pub fn set_write_timeout(&mut self, timeout: Option<Duration>) {
        self.stream.set_write_timeout(timeout);
    }

    pub fn request(&mut self, request: &CorePipeRequest) -> Result<CorePipeResponse, String> {
        self.stream
            .write_message_timeout(request, V7_FRAME_READ_TIMEOUT)?;
        self.stream
            .read_message_timeout(client_request_read_timeout(request))?
            .ok_or_else(|| "Core pipe closed".into())
    }

    pub fn request_with_timeout(
        &mut self,
        request: &CorePipeRequest,
        timeout: Duration,
    ) -> Result<CorePipeResponse, String> {
        self.stream.write_message_timeout(request, timeout)?;
        self.stream
            .read_message_timeout(timeout)?
            .ok_or_else(|| "Core pipe closed".into())
    }
}

#[cfg(windows)]
impl Read for NamedPipeClient {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(buf)
    }
}

#[cfg(windows)]
impl Write for NamedPipeClient {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(windows)]
fn checked_wide(value: &str) -> Result<Vec<u16>, &'static str> {
    if value.is_empty() || value.contains('\0') {
        return Err("pipe name is empty or contains interior NUL");
    }
    use std::os::windows::ffi::OsStrExt;
    Ok(std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect())
}

pub fn hello_request() -> CorePipeRequest {
    CorePipeRequest::Hello {
        protocol: V7_PROTOCOL_NAME.into(),
        version: V7_PROTOCOL_VERSION,
    }
}

pub const V7_TCP_PORT: u16 = 18765;

fn parse_core_bind(raw: Option<&str>) -> Result<std::net::SocketAddr, String> {
    let addr = match raw {
        Some(raw) => raw
            .parse::<std::net::SocketAddr>()
            .map_err(|error| format!("HLS_V7_CORE_BIND is invalid: {error}"))?,
        None => std::net::SocketAddr::from(([127, 0, 0, 1], V7_TCP_PORT)),
    };
    if !addr.ip().is_loopback() {
        return Err(format!(
            "HLS_V7_CORE_BIND must use a loopback address, got {addr}"
        ));
    }
    Ok(addr)
}

pub fn default_core_bind() -> Result<std::net::SocketAddr, String> {
    let configured = std::env::var("HLS_V7_CORE_BIND").ok();
    parse_core_bind(configured.as_deref())
}

pub fn serve_tcp_listener(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync>,
) -> Result<(), String> {
    let local_addr = listener
        .local_addr()
        .map_err(|error| format!("Core listener address: {error}"))?;
    if !local_addr.ip().is_loopback() {
        return Err(format!(
            "Core TCP listener requires loopback, got {local_addr}"
        ));
    }
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Core listener nonblocking: {error}"))?;
    let slots = ConnectionSlots::new();
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let Some(slot) = slots.try_claim() else {
                    continue;
                };
                let handler = Arc::clone(&handler);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let _slot = slot;
                    let _ = stream.set_nonblocking(false);
                    let _ = handle_stream(stream, handler.as_ref(), &stop);
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
/// 收连接前的优雅收尾。
///
/// Windows 上 close() 一个接收缓冲区里还有没读掉的字节的 socket 会发 RST，
/// 把刚要送出去的错误帧一起冲掉——客户端于是看到"连接被重置"而不是那条
/// protocol_violation，等于白报。所以先把写端 shutdown 送出 FIN，再把对端
/// 残留的输入读丢掉，等对端自己收尾；等待上限 V7_LINGER_CLOSE_TIMEOUT，
/// 对端不配合也不会把这个线程挂住。
fn linger_close(stream: &mut TcpStream) {
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut sink = [0u8; 512];
    let deadline = std::time::Instant::now() + V7_LINGER_CLOSE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        if stream.set_read_timeout(Some(remaining)).is_err() {
            return;
        }
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => return,
            Ok(_) => continue,
        }
    }
}

/// 收连接前把 framing 违规报给对端。
///
/// 读不出下一帧（超长、半截帧头、载荷不是 CorePipeRequest、读超时）时，以前是直接
/// 收连接：客户端只看到"对端没动静"，分不清是对端崩了还是"我发的东西对端不认"。
/// 后者是协议违规，报出来才定位得了。写不进去也无妨——连接本来就要收了。
fn report_protocol_violation(stream: &mut impl Write, error: &str) {
    let _ = write_message(
        stream,
        &CorePipeResponse::Error {
            request_id: None,
            code: "protocol_violation".into(),
            message: error.to_string(),
        },
    );
}

// Stop after the write attempt so a disconnected caller cannot strand a prepared Core.
fn dispatch_and_reply(
    writer: &mut impl Write,
    request: CorePipeRequest,
    handler: &dyn Fn(CorePipeRequest) -> CorePipeResponse,
    stop: &AtomicBool,
) -> Result<(), String> {
    let shutdown = matches!(
        request,
        CorePipeRequest::Command {
            command: CoreCommand::Shutdown,
            ..
        }
    );
    let response = handler(request);
    let result = write_message(writer, &response);
    if shutdown && matches!(response, CorePipeResponse::Events { .. }) {
        stop.store(true, Ordering::SeqCst);
    }
    result
}

fn handle_stream(
    mut stream: TcpStream,
    handler: &dyn Fn(CorePipeRequest) -> CorePipeResponse,
    stop: &AtomicBool,
) -> Result<(), String> {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(V7_FRAME_READ_TIMEOUT));
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match read_server_tcp_message::<CorePipeRequest>(&mut stream) {
            Ok(Some(request)) => {
                if stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                dispatch_and_reply(&mut stream, request, handler, stop)?;
            }
            Ok(None) => return Ok(()),
            Err(error) => {
                // 读不出下一帧（超长、半截帧头、载荷不是 CorePipeRequest、读超时）
                // 时，以前是直接 return Err：连接一关，客户端只看到"对端没动静"，
                // 分不清是对端崩了还是"我发的东西对端不认"。后者是协议违规，报出来
                // 才定位得了。
                report_protocol_violation(&mut stream, &error);
                linger_close(&mut stream);
                return Err(error);
            }
        }
    }
}

pub struct CoreIpcClient {
    transport: IpcTransport,
}

enum IpcTransport {
    Tcp(TcpStream),
    #[cfg(windows)]
    Pipe(OverlappedPipe),
}

fn client_request_read_timeout(request: &CorePipeRequest) -> Duration {
    match request {
        CorePipeRequest::WaitEvents { timeout_ms, .. }
        | CorePipeRequest::WaitEventBatch { timeout_ms, .. } => {
            Duration::from_millis(timeout_ms.clamp(&1, &60_000) + 30_000)
        }
        _ => Duration::from_secs(180),
    }
}

impl IpcTransport {
    fn request_with_timeout(
        &mut self,
        request: &CorePipeRequest,
        timeout: Option<Duration>,
    ) -> Result<CorePipeResponse, String> {
        match self {
            Self::Tcp(stream) => {
                let write_timeout = timeout.unwrap_or(V7_FRAME_READ_TIMEOUT);
                let read_timeout = timeout.unwrap_or_else(|| client_request_read_timeout(request));
                stream
                    .set_write_timeout(Some(write_timeout))
                    .map_err(|e| format!("Core stream write timeout: {e}"))?;
                stream
                    .set_read_timeout(Some(read_timeout))
                    .map_err(|e| format!("Core stream read timeout: {e}"))?;
                write_message(stream, request)?;
                read_message(stream)?.ok_or_else(|| "Core IPC closed".into())
            }
            #[cfg(windows)]
            Self::Pipe(stream) => {
                let write_timeout = timeout.unwrap_or(V7_FRAME_READ_TIMEOUT);
                let read_timeout = timeout.unwrap_or_else(|| client_request_read_timeout(request));
                stream.write_message_timeout(request, write_timeout)?;
                stream
                    .read_message_timeout(read_timeout)?
                    .ok_or_else(|| "Core pipe closed".into())
            }
        }
    }

    fn request(&mut self, request: &CorePipeRequest) -> Result<CorePipeResponse, String> {
        self.request_with_timeout(request, None)
    }
}

/// 三个"写入类"客户端方法的应答分类。单独抽成纯函数是为了可测：transport
/// 要在管道/套接字上才能构造，而这些分类规则正是版本偏差会栽倒的地方。
///
/// 以前这三个方法的兜底分支是 `_ => Ok(())`：对端换了构建、回了别的形状
/// （Hello、Snapshot、残缺的 Credential，或者干脆是另一类请求的应答），上层只看到
/// "设置保存成功"。往库里写设置失败而 UI 不报错，用户以为偏好已经生效。
fn store_setting_response(response: CorePipeResponse) -> Result<(), String> {
    match response {
        CorePipeResponse::Settings { .. } | CorePipeResponse::Events { .. } => Ok(()),
        CorePipeResponse::Error { message, .. } => Err(message),
        other => Err(format!("unexpected store-setting response: {other:?}")),
    }
}

fn store_credential_response(response: CorePipeResponse) -> Result<(), String> {
    match response {
        CorePipeResponse::Credential {
            protected_blob: Some(_),
            ..
        } => Ok(()),
        CorePipeResponse::Error { message, .. } => Err(message),
        other => Err(format!("unexpected store-credential response: {other:?}")),
    }
}

fn save_handoff_response(response: CorePipeResponse) -> Result<(), String> {
    match response {
        CorePipeResponse::Handoffs { .. } => Ok(()),
        CorePipeResponse::Error { message, .. } => Err(message),
        other => Err(format!("unexpected save-handoff response: {other:?}")),
    }
}

impl CoreIpcClient {
    pub fn connect_addr(addr: std::net::SocketAddr) -> Result<Self, String> {
        Self::connect_addr_timeout(addr, Duration::from_secs(5))
    }

    /// TCP 回环连接，总时限由调用方给出（包含所有重试）。
    pub fn connect_addr_timeout(
        addr: std::net::SocketAddr,
        timeout: Duration,
    ) -> Result<Self, String> {
        if !addr.ip().is_loopback() {
            return Err(format!("Core TCP client requires loopback, got {addr}"));
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let attempt = remaining.clamp(Duration::from_millis(1), Duration::from_millis(200));
            match TcpStream::connect_timeout(&addr, attempt) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    let hello_timeout = hello_handshake_timeout(timeout);
                    let mut client = Self {
                        transport: IpcTransport::Tcp(stream),
                    };
                    match client
                        .transport
                        .request_with_timeout(&hello_request(), Some(hello_timeout))?
                    {
                        CorePipeResponse::Hello {
                            protocol, version, ..
                        } if protocol == V7_PROTOCOL_NAME && version == V7_PROTOCOL_VERSION => {
                            return Ok(client);
                        }
                        other => {
                            return Err(format!("Core hello rejected: {other:?}"));
                        }
                    }
                }
                Err(error) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(format!("Core connect {addr}: {error}"));
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
        let deadline = std::time::Instant::now() + timeout;
        #[cfg(windows)]
        {
            let last_error = loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                let pipe_timeout = remaining.as_millis().clamp(1, 40) as u32;
                match Self::connect_pipe_timeout(&v7_pipe_name(), pipe_timeout) {
                    Ok(client) => return Ok(client),
                    Err(error) => {
                        let remaining =
                            deadline.saturating_duration_since(std::time::Instant::now());
                        if remaining.is_zero() {
                            break error;
                        }
                        thread::sleep(Duration::from_millis(10).min(remaining));
                    }
                }
            };
            // 所有管道超时出口都必须执行策略检查，不能在时限边界意外进入 TCP 的五秒重试。
            if !tcp_loopback_enabled() {
                return Err(last_error);
            }
        }
        // 调用方给的时限覆盖整个探测：管道用剩的时间才留给 TCP，不再固定等 5 秒。
        // 至少留 200ms，保证回环端口有一次完整的连接尝试。
        let remaining = deadline
            .saturating_duration_since(std::time::Instant::now())
            .max(Duration::from_millis(200));
        Self::connect_addr_timeout(default_core_bind()?, remaining)
    }

    #[cfg(windows)]
    pub fn connect_pipe(name: &str) -> Result<Self, String> {
        Self::connect_pipe_timeout(name, 2_000)
    }

    #[cfg(windows)]
    fn connect_pipe_timeout(name: &str, timeout_ms: u32) -> Result<Self, String> {
        let pipe = NamedPipeClient::connect(name, timeout_ms)?;
        // `timeout_ms` bounds waiting for the pipe instance to open; once opened,
        // give the `Hello` RPC its own bounded handshake deadline so a Core under
        // CPU/I/O load never fails `Hello` due to only ~50ms remaining.
        let hello_timeout = hello_handshake_timeout(Duration::from_millis(u64::from(timeout_ms)));
        let mut client = Self {
            transport: IpcTransport::Pipe(pipe.into_stream()),
        };
        match client
            .transport
            .request_with_timeout(&hello_request(), Some(hello_timeout))?
        {
            CorePipeResponse::Hello {
                protocol, version, ..
            } if protocol == V7_PROTOCOL_NAME && version == V7_PROTOCOL_VERSION => Ok(client),
            other => Err(format!("Core pipe hello rejected: {other:?}")),
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
        self.snapshot_state_with_browser()
            .map(|(tasks, latest_sequence, _)| (tasks, latest_sequence))
    }

    pub fn snapshot_state_with_browser(
        &mut self,
    ) -> Result<(Vec<crate::TaskSnapshot>, u64, bool), String> {
        if matches!(self.request(&CorePipeRequest::Capabilities { request_id: 1 })?,
            CorePipeResponse::Capabilities { commands, .. }
                if commands.iter().any(|name| name == "list_tasks_paged")
                    && commands.iter().any(|name| name == "event_batch_resync"))
        {
            return self.paged_snapshot_state_with_browser();
        }
        match self.request(&CorePipeRequest::Snapshot { request_id: 1 })? {
            CorePipeResponse::Snapshot {
                tasks,
                latest_sequence,
                browser_connected,
                ..
            } => Ok((tasks, latest_sequence, browser_connected)),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected Core snapshot: {other:?}")),
        }
    }

    /// Read bounded pages and reconcile changes made while the pages were read.
    /// A row's page sequence prevents older replay events from overwriting it.
    #[cfg(test)]
    fn paged_snapshot_state(&mut self) -> Result<(Vec<crate::TaskSnapshot>, u64), String> {
        self.paged_snapshot_state_with_browser()
            .map(|(tasks, sequence, _)| (tasks, sequence))
    }

    fn paged_snapshot_state_with_browser(
        &mut self,
    ) -> Result<(Vec<crate::TaskSnapshot>, u64, bool), String> {
        use std::collections::BTreeMap;
        'retry: for _ in 0..3 {
            let mut tasks = BTreeMap::<String, (crate::TaskSnapshot, u64)>::new();
            let mut cursor: Option<String> = None;
            let mut baseline = None;
            let (target, mut browser_connected) = loop {
                let page = self.task_page(cursor.as_deref(), 128)?;
                baseline.get_or_insert(page.sequence);
                let page_browser_connected = page.browser_connected;
                for task in page.tasks {
                    tasks.insert(task.task_id.clone(), (task, page.sequence));
                }
                match page.next {
                    Some(next) if cursor.as_ref().is_none_or(|current| next > *current) => {
                        cursor = Some(next)
                    }
                    Some(_) => return Err("Core task cursor did not advance".into()),
                    None => break (page.sequence, page_browser_connected),
                }
            };
            let mut after = baseline.unwrap_or(target);
            while after < target {
                let batch = self.wait_event_batch(after, 1)?;
                if batch.resync_required {
                    continue 'retry;
                }
                if batch.events.is_empty() {
                    return Err("Core event history ended before task pages".into());
                }
                for envelope in batch.events {
                    if envelope.sequence <= after {
                        return Err("Core event sequence did not advance".into());
                    }
                    after = envelope.sequence;
                    match envelope.event {
                        crate::CoreEvent::TaskCreated { snapshot }
                        | crate::CoreEvent::TaskUpdated { snapshot }
                        | crate::CoreEvent::TaskProgress { snapshot } => {
                            if tasks
                                .get(&snapshot.task_id)
                                .is_none_or(|(_, version)| after > *version)
                            {
                                tasks.insert(snapshot.task_id.clone(), (snapshot, after));
                            }
                        }
                        crate::CoreEvent::TaskDeleted { task_id }
                            if tasks
                                .get(&task_id)
                                .is_some_and(|(_, version)| after > *version) =>
                        {
                            tasks.remove(&task_id);
                        }
                        crate::CoreEvent::BrowserStatus { connected, .. } => {
                            browser_connected = connected;
                        }
                        _ => {}
                    }
                }
            }
            let mut tasks: Vec<_> = tasks.into_values().map(|(snapshot, _)| snapshot).collect();
            tasks.sort_by(|a, b| (a.queue_index, &a.task_id).cmp(&(b.queue_index, &b.task_id)));
            return Ok((tasks, after, browser_connected));
        }
        Err("任务变化过快，需重新同步".into())
    }

    pub fn task_page(
        &mut self,
        after_task_id: Option<&str>,
        limit: usize,
    ) -> Result<hls_protocol::TaskPage, String> {
        match self.request(&CorePipeRequest::ListTasks {
            request_id: 1,
            after_task_id: after_task_id.map(str::to_owned),
            limit,
        })? {
            CorePipeResponse::TaskPage { page, .. } => Ok(page),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected Core task page: {other:?}")),
        }
    }

    pub fn wait_event_batch(
        &mut self,
        after_sequence: u64,
        timeout_ms: u64,
    ) -> Result<hls_protocol::EventBatch, String> {
        match self.request(&CorePipeRequest::WaitEventBatch {
            request_id: 1,
            after_sequence,
            timeout_ms,
        })? {
            CorePipeResponse::EventBatch { batch, .. } => Ok(batch),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected Core event batch: {other:?}")),
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
        let response = self.request(&CorePipeRequest::StoreSetting {
            request_id: 1,
            key: key.into(),
            value,
        })?;
        store_setting_response(response)
    }

    pub fn store_settings(
        &mut self,
        values: std::collections::BTreeMap<String, Value>,
    ) -> Result<(), String> {
        let response = self.request(&CorePipeRequest::StoreSettings {
            request_id: 1,
            values,
        })?;
        store_setting_response(response)
    }

    pub fn store_credential(
        &mut self,
        credential_ref: &str,
        protected_blob: &str,
        kind: &str,
    ) -> Result<(), String> {
        let response = self.request(&CorePipeRequest::StoreCredential {
            request_id: 1,
            credential_ref: credential_ref.into(),
            protected_blob: protected_blob.into(),
            kind: kind.into(),
        })?;
        store_credential_response(response)
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

    pub fn delete_credential(&mut self, credential_ref: &str) -> Result<(), String> {
        match self.request(&CorePipeRequest::DeleteCredential {
            request_id: 1,
            credential_ref: credential_ref.into(),
        })? {
            CorePipeResponse::Credential {
                protected_blob: None,
                ..
            } => Ok(()),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected credential delete response: {other:?}")),
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
        let response = self.request(&CorePipeRequest::SaveHandoff {
            request_id: 1,
            handoff_id: handoff_id.into(),
            handoff_json: handoff_json.into(),
            status: status.into(),
            task_id: task_id.map(str::to_string),
            created_at_ms,
        })?;
        save_handoff_response(response)
    }

    pub fn load_handoffs(&mut self) -> Result<Vec<String>, String> {
        self.query_handoffs(None, None, false)
    }

    pub fn query_handoffs(
        &mut self,
        handoff_id: Option<&str>,
        client_request_id: Option<&str>,
        pending_only: bool,
    ) -> Result<Vec<String>, String> {
        let mut result = Vec::new();
        let mut after = None;
        loop {
            match self.request(&CorePipeRequest::LoadHandoffs {
                request_id: 1,
                handoff_id: handoff_id.map(str::to_owned),
                client_request_id: client_request_id.map(str::to_owned),
                pending_only,
                paged: true,
                after,
            })? {
                CorePipeResponse::Handoffs { items, next, .. } => {
                    result.extend(items);
                    match next {
                        Some(key) if after.is_none_or(|previous| key > previous) => {
                            after = Some(key)
                        }
                        Some(_) => return Err("接管分页游标未前进".into()),
                        None => return Ok(result),
                    }
                }
                CorePipeResponse::Error { message, .. } => return Err(message),
                other => return Err(format!("unexpected handoff response: {other:?}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, ResourceKind, TaskSpec};

    #[test]
    fn shutdown_reply_is_flushed_before_stop_becomes_visible() {
        struct Reply<'a>(&'a AtomicBool, bool);
        impl Write for Reply<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                assert!(!self.0.load(Ordering::SeqCst));
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                assert!(!self.0.load(Ordering::SeqCst));
                self.1 = true;
                Ok(())
            }
        }
        let stop = AtomicBool::new(false);
        let mut reply = Reply(&stop, false);
        dispatch_and_reply(
            &mut reply,
            CorePipeRequest::Command {
                request_id: 1,
                command: CoreCommand::Shutdown,
            },
            &|_| CorePipeResponse::Events {
                request_id: 1,
                events: vec![],
            },
            &stop,
        )
        .unwrap();
        assert!(reply.1 && stop.load(Ordering::SeqCst));
    }

    #[test]
    fn task_pages_reconcile_concurrent_deletion_and_reject_older_row_events() {
        use hls_protocol::{CoreEvent, EventBatch, TaskPage, TaskSnapshot};
        let task = |id: &str, title: &str| TaskSnapshot {
            task_id: id.into(),
            title: title.into(),
            ..Default::default()
        };
        let replies = vec![
            CorePipeResponse::TaskPage {
                request_id: 1,
                page: TaskPage {
                    tasks: vec![task("task-a", "before")],
                    next: Some("task-a".into()),
                    sequence: 10,
                    browser_connected: false,
                },
            },
            CorePipeResponse::TaskPage {
                request_id: 1,
                page: TaskPage {
                    tasks: vec![task("task-b", "new page")],
                    next: None,
                    sequence: 15,
                    browser_connected: false,
                },
            },
            CorePipeResponse::EventBatch {
                request_id: 1,
                batch: EventBatch {
                    latest_sequence: 15,
                    resync_required: false,
                    events: vec![
                        EventEnvelope {
                            sequence: 11,
                            event: CoreEvent::TaskUpdated {
                                snapshot: task("task-a", "updated"),
                            },
                        },
                        EventEnvelope {
                            sequence: 12,
                            event: CoreEvent::TaskUpdated {
                                snapshot: task("task-b", "old event"),
                            },
                        },
                        EventEnvelope {
                            sequence: 13,
                            event: CoreEvent::TaskDeleted {
                                task_id: "task-c".into(),
                            },
                        },
                        EventEnvelope {
                            sequence: 14,
                            event: CoreEvent::TaskCreated {
                                snapshot: task("task-0", "created behind cursor"),
                            },
                        },
                        EventEnvelope {
                            sequence: 15,
                            event: CoreEvent::TaskDeleted {
                                task_id: "task-a".into(),
                            },
                        },
                    ],
                },
            },
        ];
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            assert_eq!(
                read_message::<CorePipeRequest>(&mut stream).unwrap(),
                Some(hello_request())
            );
            write_message(
                &mut stream,
                &CorePipeResponse::Hello {
                    protocol: V7_PROTOCOL_NAME.into(),
                    version: V7_PROTOCOL_VERSION,
                    pid: 1,
                },
            )
            .unwrap();
            for reply in replies {
                assert!(read_message::<CorePipeRequest>(&mut stream)
                    .unwrap()
                    .is_some());
                write_message(&mut stream, &reply).unwrap();
            }
        });
        let mut client = CoreIpcClient::connect_addr(addr).unwrap();
        let (tasks, sequence) = client.paged_snapshot_state().unwrap();
        assert_eq!(sequence, 15);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].task_id, "task-0");
        assert_eq!(tasks[1].title, "new page");
        server.join().unwrap();
    }

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
    fn delete_credential_request_roundtrip_preserves_identity() {
        let request = CorePipeRequest::DeleteCredential {
            request_id: 11,
            credential_ref: "cred-owned".into(),
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

    #[test]
    fn core_bind_accepts_only_loopback_ipv4_and_ipv6() {
        assert_eq!(
            parse_core_bind(None).unwrap(),
            std::net::SocketAddr::from(([127, 0, 0, 1], V7_TCP_PORT))
        );
        for raw in ["127.0.0.1:18765", "127.42.0.9:4567", "[::1]:18765"] {
            let addr = parse_core_bind(Some(raw)).unwrap();
            assert!(addr.ip().is_loopback(), "expected loopback: {raw}");
        }
    }

    #[test]
    fn core_bind_rejects_wildcard_lan_and_public_addresses() {
        for raw in [
            "0.0.0.0:18765",
            "[::]:18765",
            "192.168.1.25:18765",
            "10.0.0.25:18765",
            "203.0.113.7:18765",
            "[2001:db8::7]:18765",
        ] {
            let error = parse_core_bind(Some(raw)).unwrap_err();
            assert!(
                error.contains("loopback"),
                "unexpected error for {raw}: {error}"
            );
        }
        assert!(parse_core_bind(Some("not-an-address")).is_err());
    }

    #[test]
    fn prebound_non_loopback_listener_is_rejected_at_server_boundary() {
        let listener = TcpListener::bind("0.0.0.0:0").unwrap();
        assert!(!listener.local_addr().unwrap().ip().is_loopback());
        let stop = Arc::new(AtomicBool::new(true));
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(|_| CorePipeResponse::Error {
                request_id: None,
                code: "test".into(),
                message: "test".into(),
            });
        let error = serve_tcp_listener(listener, stop, handler).unwrap_err();
        assert!(error.contains("loopback"), "unexpected error: {error}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_tcp_loopback_is_opt_in() {
        assert!(!tcp_loopback_enabled());
    }

    #[test]
    fn hello_handshake_timeout_gives_opened_pipes_bounded_headroom() {
        assert_eq!(
            hello_handshake_timeout(Duration::from_millis(10)),
            Duration::from_millis(1_500)
        );
        assert_eq!(
            hello_handshake_timeout(Duration::from_millis(25)),
            Duration::from_millis(1_500)
        );
        assert_eq!(
            hello_handshake_timeout(Duration::from_millis(2_500)),
            Duration::from_millis(2_500)
        );
        assert_eq!(
            hello_handshake_timeout(Duration::from_secs(10)),
            Duration::from_millis(5_000)
        );
    }

    #[cfg(windows)]
    #[test]
    fn connect_pipe_timeout_completes_hello_when_server_is_busy_past_open_budget() {
        let pipe_name = format!(
            r"\\.\pipe\HLSDownloader.v7.hello-budget-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let server = Arc::new(NamedPipeServer::new(pipe_name.clone()));
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let server_worker = {
            let server = Arc::clone(&server);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                server.serve_loop_with_ready(
                    stop,
                    Arc::new(|request| match request {
                        CorePipeRequest::Hello { .. } => {
                            // Exceeds both a 25ms probe budget and the old 50ms floor.
                            thread::sleep(Duration::from_millis(150));
                            CorePipeResponse::Hello {
                                protocol: V7_PROTOCOL_NAME.into(),
                                version: V7_PROTOCOL_VERSION,
                                pid: std::process::id(),
                            }
                        }
                        other => CorePipeResponse::Error {
                            request_id: other.request_id(),
                            code: "unexpected".into(),
                            message: format!("{other:?}"),
                        },
                    }),
                    ready_tx,
                )
            })
        };
        ready_rx.recv().unwrap().unwrap();

        let client = CoreIpcClient::connect_pipe_timeout(&pipe_name, 25);
        assert!(
            client.is_ok(),
            "expected opened pipe to finish Hello within dedicated handshake budget: {:?}",
            client.err()
        );
        drop(client);
        server.signal_stop();
        stop.store(true, Ordering::SeqCst);
        server_worker.join().unwrap().unwrap();
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

    #[test]
    fn adversarial_pipe_frame_rejects_valid_frame_with_unparseable_payload() {
        // 帧头合法、载荷不是 CorePipeRequest：第九十轮记录过 handle_stream 用 `?`
        // 传播这个错误、于是**关掉连接**而不是回一条错误帧。这里把"分类为解析错误"
        // 钉住，让将来改动传输层时不会悄悄把它变成别的东西。
        let payload = br#"{"type":"command","request_id":1,"command":{"kind":"nope"}}"#;
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(payload);
        let err = decode_message::<CorePipeRequest>(&frame).unwrap_err();
        assert!(err.contains("JSON invalid"), "unexpected error: {err}");

        // 载荷不是对象而是合法 JSON 的其它类型，同样必须被分类成解析错误。
        let payload = b"1234";
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(payload);
        assert!(decode_message::<CorePipeRequest>(&frame).is_err());

        // 空载荷与仅空白载荷：长度合法但没有任何 JSON；带尾随字节的帧必须被拒绝。
        let mut frame = 0u32.to_le_bytes().to_vec();
        let err = decode_message::<CorePipeRequest>(&frame).unwrap_err();
        assert!(err.contains("JSON invalid"), "unexpected error: {err}");
        let mut blank_frame = 1u32.to_le_bytes().to_vec();
        blank_frame.extend_from_slice(b" ");
        let err = decode_message::<CorePipeRequest>(&blank_frame).unwrap_err();
        assert!(err.contains("JSON invalid"), "unexpected error: {err}");
        frame.extend_from_slice(b" ");
        let err = decode_message::<CorePipeRequest>(&frame).unwrap_err();
        assert!(err.contains("invalid length"), "unexpected error: {err}");
    }

    // 第五轮发现 6 的回归。这三个"写入类"方法原来用 `_ => Ok(())` 兜底：对端换了构建、
    // 回了别的形状（Value、Hello、残缺的 Credential），上层只看到"保存成功"。设置其实没
    // 落库而 UI 不报错，用户会以为偏好已经生效。样本走和真实管线同一条 serde 路径。
    fn response(json: &str) -> CorePipeResponse {
        serde_json::from_str(json).expect("fixture must be a valid CorePipeResponse")
    }

    fn error_response() -> CorePipeResponse {
        response(
            r#"{"type":"error","request_id":1,"code":"setting_failed","message":"database is locked"}"#,
        )
    }

    #[test]
    fn store_setting_accepts_only_settings_and_events() {
        let ok = response(
            r#"{"type":"settings","request_id":1,"takeover_enabled":false,"takeover_minimum_bytes":0,"legal_accepted":true,"speed_limit_kib":0}"#,
        );
        assert!(store_setting_response(ok).is_ok());
        let events = response(r#"{"type":"events","request_id":1,"events":[]}"#);
        assert!(store_setting_response(events).is_ok());

        // 对端错误必须原样带到上层，不能被包成"意外应答"。
        assert_eq!(
            store_setting_response(error_response()).unwrap_err(),
            "database is locked"
        );

        for json in [
            r#"{"type":"hello","protocol":"hls-downloader-v7-core","version":1,"pid":1}"#,
            r#"{"type":"credential","request_id":1,"protected_blob":"blob"}"#,
            r#"{"type":"handoffs","request_id":1,"items":[]}"#,
        ] {
            let err = store_setting_response(response(json)).unwrap_err();
            assert!(
                err.starts_with("unexpected store-setting response"),
                "unexpected message: {err}"
            );
        }
    }

    #[test]
    fn store_credential_requires_a_protected_blob() {
        let ok = response(r#"{"type":"credential","request_id":1,"protected_blob":"blob"}"#);
        assert!(store_credential_response(ok).is_ok());
        assert_eq!(
            store_credential_response(error_response()).unwrap_err(),
            "database is locked"
        );

        // 没有 protected_blob 说明凭据没写进凭据库；换了形状同样不算保存成功。
        for json in [
            r#"{"type":"credential","request_id":1,"protected_blob":null}"#,
            r#"{"type":"settings","request_id":1,"takeover_enabled":false,"takeover_minimum_bytes":0,"legal_accepted":true,"speed_limit_kib":0}"#,
        ] {
            let err = store_credential_response(response(json)).unwrap_err();
            assert!(
                err.starts_with("unexpected store-credential response"),
                "unexpected message: {err}"
            );
        }
    }

    #[test]
    fn save_handoff_accepts_only_handoffs() {
        let ok = response(r#"{"type":"handoffs","request_id":1,"items":[]}"#);
        assert!(save_handoff_response(ok).is_ok());
        assert_eq!(
            save_handoff_response(error_response()).unwrap_err(),
            "database is locked"
        );

        for json in [
            r#"{"type":"hello","protocol":"hls-downloader-v7-core","version":1,"pid":1}"#,
            r#"{"type":"credential","request_id":1,"protected_blob":"blob"}"#,
        ] {
            let err = save_handoff_response(response(json)).unwrap_err();
            assert!(
                err.starts_with("unexpected save-handoff response"),
                "unexpected message: {err}"
            );
        }
    }

    /// 第五轮发现 5：framing 违规以前只是把连接关掉，客户端看到"对端没动静"，
    /// 分不清是对端崩了还是自己发的东西对端不认。现在对端必须在收连接之前回一条
    /// protocol_violation 错误帧。
    #[test]
    fn a_framing_violation_answers_with_a_protocol_violation_frame() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let handler = |_request: CorePipeRequest| CorePipeResponse::Hello {
                protocol: V7_PROTOCOL_NAME.into(),
                version: V7_PROTOCOL_VERSION,
                pid: 0,
            };
            // 两条连接：一条发能解析但语义不对的载荷，一条发超长帧头。
            let unparseable = handle_stream(
                listener.accept().unwrap().0,
                &handler,
                &AtomicBool::new(false),
            );
            let oversize = handle_stream(
                listener.accept().unwrap().0,
                &handler,
                &AtomicBool::new(false),
            );
            (unparseable, oversize)
        });

        let mut client = TcpStream::connect(addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();

        // 正常路径不能被改坏。
        write_message(&mut client, &hello_request()).unwrap();
        match read_message::<CorePipeResponse>(&mut client)
            .unwrap()
            .unwrap()
        {
            CorePipeResponse::Hello { version, .. } => assert_eq!(version, V7_PROTOCOL_VERSION),
            other => panic!("expected hello, got {other:?}"),
        }

        // 帧头合法、载荷不是 CorePipeRequest。
        let payload = b"1234";
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(payload);
        client.write_all(&frame).unwrap();
        match read_message::<CorePipeResponse>(&mut client)
            .unwrap()
            .unwrap()
        {
            CorePipeResponse::Error { code, message, .. } => {
                assert_eq!(code, "protocol_violation");
                assert!(
                    message.contains("JSON invalid"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected a protocol violation frame, got {other:?}"),
        }
        // 错误帧之后是对端直接收连接：干净的 EOF，而不是又一条沉默。
        assert!(matches!(
            read_message::<CorePipeResponse>(&mut client),
            Ok(None)
        ));
        drop(client);

        // 超长帧：帧头就过不了，同样要报成协议违规。
        let mut client = TcpStream::connect(addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let mut frame = (V7_PIPE_MAX_FRAME as u32 + 1).to_le_bytes().to_vec();
        frame.extend_from_slice(&[0u8; 8]);
        client.write_all(&frame).unwrap();
        match read_message::<CorePipeResponse>(&mut client)
            .unwrap()
            .unwrap()
        {
            CorePipeResponse::Error { code, message, .. } => {
                assert_eq!(code, "protocol_violation");
                assert!(
                    message.contains("too large"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected a protocol violation frame, got {other:?}"),
        }
        drop(client);

        let (unparseable, oversize) = server.join().unwrap();
        assert!(
            unparseable.is_err(),
            "handle_stream must still report the violation: {unparseable:?}"
        );
        assert!(
            oversize.is_err(),
            "handle_stream must still report the violation: {oversize:?}"
        );
    }

    /// 命名管道才是产品 IPC（TCP 环回是 opt-in，见 windows_tcp_loopback_is_opt_in），
    /// 所以这条路径上的 framing 违规也必须报成 protocol_violation，而不是让客户端把
    /// "连接消失"当成对端崩溃。
    #[cfg(windows)]
    #[test]
    fn a_pipe_framing_violation_answers_with_a_protocol_violation_frame() {
        let name = format!(
            r"\\.\pipe\HLSDownloader.v7.t{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(|_request: CorePipeRequest| CorePipeResponse::Hello {
                protocol: V7_PROTOCOL_NAME.into(),
                version: V7_PROTOCOL_VERSION,
                pid: 0,
            });
        let serve_name = name.clone();
        let serve_stop = Arc::clone(&stop);
        let serve_handler = Arc::clone(&handler);
        thread::spawn(move || {
            let _ = NamedPipeServer::new(serve_name).serve_loop(serve_stop, serve_handler);
        });

        // serve_loop 的管道实例是异步建的，照 core_server 的测试那样轮询等到它出现。
        let mut client = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(pipe) = NamedPipeClient::connect(&name, 100) {
                client = Some(pipe);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let mut client = client.expect("the serve_loop instance never appeared");
        write_message(&mut client, &hello_request()).unwrap();
        match read_message::<CorePipeResponse>(&mut client)
            .unwrap()
            .unwrap()
        {
            CorePipeResponse::Hello { version, .. } => assert_eq!(version, V7_PROTOCOL_VERSION),
            other => panic!("expected hello, got {other:?}"),
        }

        // 帧头合法、载荷不是 CorePipeRequest。
        let payload = b"1234";
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(payload);
        client.write_all(&frame).unwrap();
        match read_message::<CorePipeResponse>(&mut client)
            .unwrap()
            .unwrap()
        {
            CorePipeResponse::Error { code, message, .. } => {
                assert_eq!(code, "protocol_violation");
                assert!(
                    message.contains("JSON invalid"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected a protocol violation frame, got {other:?}"),
        }

        stop.store(true, Ordering::SeqCst);
    }

    #[cfg(windows)]
    #[test]
    fn serve_loop_with_ready_answers_framing_violations_with_protocol_violation_frame() {
        let name = format!(
            r"\\.\pipe\HLSDownloader.v7.r{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(|_request: CorePipeRequest| CorePipeResponse::Hello {
                protocol: V7_PROTOCOL_NAME.into(),
                version: V7_PROTOCOL_VERSION,
                pid: 0,
            });
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let serve_name = name.clone();
        let serve_stop = Arc::clone(&stop);
        let serve_handler = Arc::clone(&handler);
        thread::spawn(move || {
            let _ = NamedPipeServer::new(serve_name).serve_loop_with_ready(
                serve_stop,
                serve_handler,
                ready_tx,
            );
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("ready signal timed out")
            .expect("ready signal failed");

        for round in 0..2 {
            let mut client = None;
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if let Ok(pipe) = NamedPipeClient::connect(&name, 100) {
                    client = Some(pipe);
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let mut client = client.unwrap_or_else(|| {
                panic!("serve_loop_with_ready connection {round} never appeared")
            });
            let payload = b"1234";
            let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(payload);
            client.write_all(&frame).unwrap();
            match read_message::<CorePipeResponse>(&mut client)
                .unwrap()
                .unwrap()
            {
                CorePipeResponse::Error { code, message, .. } => {
                    assert_eq!(code, "protocol_violation");
                    assert!(
                        message.contains("JSON invalid"),
                        "unexpected message on round {round}: {message}"
                    );
                }
                other => panic!("expected protocol violation on round {round}, got {other:?}"),
            }
        }

        stop.store(true, Ordering::SeqCst);
    }

    #[cfg(windows)]
    #[test]
    fn competing_pipe_clients_connect_within_their_deadline() {
        let name = format!(r"\\.\pipe\HLSDownloader.contention.{}", std::process::id());
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(|_| CorePipeResponse::Hello {
                protocol: V7_PROTOCOL_NAME.into(),
                version: V7_PROTOCOL_VERSION,
                pid: 0,
            });
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let server_stop = Arc::clone(&stop);
        let server_name = name.clone();
        let server = thread::spawn(move || {
            NamedPipeServer::new(server_name).serve_loop_with_ready(server_stop, handler, ready_tx)
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let clients: Vec<_> = (0..16)
            .map(|_| {
                let name = name.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut errors = Vec::new();
                    for _ in 0..8 {
                        barrier.wait();
                        let result = NamedPipeClient::connect(&name, 2_000)
                            .and_then(|mut client| client.request(&hello_request()));
                        if let Err(error) = result {
                            errors.push(error);
                        }
                    }
                    errors
                })
            })
            .collect();
        let errors: Vec<_> = clients
            .into_iter()
            .flat_map(|client| client.join().unwrap())
            .collect();
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap().unwrap();
        assert!(
            errors.is_empty(),
            "competing connections failed: {errors:?}"
        );
        let missing = format!("{name}.missing");
        let started = std::time::Instant::now();
        assert!(NamedPipeClient::connect(&missing, 40).is_err());
        assert!(started.elapsed() >= Duration::from_millis(40));
    }

    #[test]
    fn short_probe_to_absent_core_respects_its_deadline() {
        // 端口先占用再释放，确保没有 Core 在监听。
        let addr = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let started = std::time::Instant::now();
        let result = CoreIpcClient::connect_addr_timeout(addr, Duration::from_millis(300));
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "probe took {:?}",
            started.elapsed()
        );
    }

    #[cfg(windows)]
    fn unique_test_pipe_name(tag: &str) -> String {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        format!(
            r"\\.\pipe\HLSDownloader.s3c.{tag}.{}.{}.{}",
            std::process::id(),
            id,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[cfg(windows)]
    #[test]
    fn overlapped_named_pipe_roundtrip_latency_has_no_polling_delay() {
        let name = unique_test_pipe_name("latency");
        let stop = Arc::new(AtomicBool::new(false));
        let server = NamedPipeServer::new(name.clone());
        let stop_handle = server.stop_handle();
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(|request| match request {
                CorePipeRequest::Hello { .. } => CorePipeResponse::Hello {
                    protocol: V7_PROTOCOL_NAME.into(),
                    version: V7_PROTOCOL_VERSION,
                    pid: std::process::id(),
                },
                CorePipeRequest::Command { request_id, .. } => CorePipeResponse::Events {
                    request_id,
                    events: Vec::new(),
                },
                other => CorePipeResponse::Error {
                    request_id: None,
                    code: "unexpected".into(),
                    message: format!("unexpected request: {other:?}"),
                },
            });
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let server_stop = Arc::clone(&stop);
        let worker =
            thread::spawn(move || server.serve_loop_with_ready(server_stop, handler, ready_tx));
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("ready timed out")
            .expect("ready failed");

        let mut client = CoreIpcClient::connect_pipe(&name).expect("connect_pipe failed");
        let started = Instant::now();
        let mut samples = Vec::with_capacity(20);
        for _ in 0..20 {
            let call_start = Instant::now();
            let events = client.command(CoreCommand::Ping).expect("ping failed");
            assert!(events.is_empty());
            samples.push(call_start.elapsed());
        }
        let total = started.elapsed();
        samples.sort_unstable();
        let avg_ms = total.as_secs_f64() * 1000.0 / 20.0;
        let max_ms = samples.last().copied().unwrap_or_default().as_secs_f64() * 1000.0;
        println!(
            "overlapped_named_pipe_20_roundtrips total_ms={:.3} avg_ms={:.3} max_ms={:.3}",
            total.as_secs_f64() * 1000.0,
            avg_ms,
            max_ms
        );
        assert!(
            total < Duration::from_millis(200),
            "20 sequential named-pipe round-trips took {total:?} (expected < 200ms without 10ms polling); samples={samples:?}"
        );

        drop(client);
        stop.store(true, Ordering::SeqCst);
        stop_handle.signal();
        worker.join().unwrap().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn overlapped_named_pipe_shutdown_wakes_idle_accept_promptly() {
        // Case 1: Stop event handle wakes an idle ConnectNamedPipe immediately.
        {
            let name = unique_test_pipe_name("stop-event");
            let stop = Arc::new(AtomicBool::new(false));
            let server = NamedPipeServer::new(name);
            let stop_handle = server.stop_handle();
            let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
                Arc::new(|_| CorePipeResponse::Hello {
                    protocol: V7_PROTOCOL_NAME.into(),
                    version: V7_PROTOCOL_VERSION,
                    pid: std::process::id(),
                });
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let server_stop = Arc::clone(&stop);
            let worker =
                thread::spawn(move || server.serve_loop_with_ready(server_stop, handler, ready_tx));
            ready_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("ready timed out")
                .expect("ready failed");

            // Allow ConnectNamedPipe to enter its kernel wait state.
            thread::sleep(Duration::from_millis(20));

            let started = Instant::now();
            stop.store(true, Ordering::SeqCst);
            stop_handle.signal();
            worker.join().unwrap().unwrap();
            let elapsed = started.elapsed();
            println!(
                "overlapped_named_pipe_stop_event_wake_ms={:.3}",
                elapsed.as_secs_f64() * 1000.0
            );
            assert!(
                elapsed < Duration::from_millis(200),
                "stop_handle.signal() took {elapsed:?} to wake idle ConnectNamedPipe"
            );
        }

        // Case 2: Legacy AtomicBool-only stop still wakes idle ConnectNamedPipe promptly.
        {
            let name = unique_test_pipe_name("stop-flag");
            let stop = Arc::new(AtomicBool::new(false));
            let server = NamedPipeServer::new(name);
            let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
                Arc::new(|_| CorePipeResponse::Hello {
                    protocol: V7_PROTOCOL_NAME.into(),
                    version: V7_PROTOCOL_VERSION,
                    pid: std::process::id(),
                });
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let server_stop = Arc::clone(&stop);
            let worker =
                thread::spawn(move || server.serve_loop_with_ready(server_stop, handler, ready_tx));
            ready_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("ready timed out")
                .expect("ready failed");

            thread::sleep(Duration::from_millis(20));

            let started = Instant::now();
            stop.store(true, Ordering::SeqCst);
            worker.join().unwrap().unwrap();
            let elapsed = started.elapsed();
            println!(
                "overlapped_named_pipe_stop_flag_wake_ms={:.3}",
                elapsed.as_secs_f64() * 1000.0
            );
            assert!(
                elapsed < Duration::from_millis(200),
                "AtomicBool-only stop took {elapsed:?} to wake idle ConnectNamedPipe"
            );
        }

        // Case 3: In-band CoreCommand::Shutdown flushes reply, wakes idle accept & idle clients, and frees all slots.
        {
            let name = unique_test_pipe_name("stop-cmd");
            let stop = Arc::new(AtomicBool::new(false));
            let server = Arc::new(NamedPipeServer::new(name.clone()));
            let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
                Arc::new(|request| match request {
                    CorePipeRequest::Hello { .. } => CorePipeResponse::Hello {
                        protocol: V7_PROTOCOL_NAME.into(),
                        version: V7_PROTOCOL_VERSION,
                        pid: std::process::id(),
                    },
                    CorePipeRequest::Command { request_id, .. } => CorePipeResponse::Events {
                        request_id,
                        events: Vec::new(),
                    },
                    other => CorePipeResponse::Error {
                        request_id: None,
                        code: "unexpected".into(),
                        message: format!("unexpected: {other:?}"),
                    },
                });
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let server_for_thread = Arc::clone(&server);
            let server_stop = Arc::clone(&stop);
            let worker = thread::spawn(move || {
                server_for_thread.serve_loop_with_ready(server_stop, handler, ready_tx)
            });
            ready_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("ready timed out")
                .expect("ready failed");

            let _idle_client = CoreIpcClient::connect_pipe(&name).expect("idle client connect");
            let mut controller = CoreIpcClient::connect_pipe(&name).expect("controller connect");

            let started = Instant::now();
            let events = controller
                .command(CoreCommand::Shutdown)
                .expect("shutdown command reply");
            assert!(events.is_empty());
            worker.join().unwrap().unwrap();
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_millis(200),
                "CoreCommand::Shutdown took {elapsed:?} to stop server"
            );

            let slot_deadline = Instant::now() + Duration::from_secs(1);
            while server.claimed_slots() > 0 && Instant::now() < slot_deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(
                server.claimed_slots(),
                0,
                "idle client thread must exit and release its slot on shutdown"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn overlapped_named_pipe_partial_frame_timeout_cancels_and_frees_slot() {
        let name = unique_test_pipe_name("partial-timeout");
        let stop = Arc::new(AtomicBool::new(false));
        let frame_timeout = Duration::from_millis(120);
        let server = Arc::new(
            NamedPipeServer::new(name.clone())
                .with_timeouts(Duration::from_millis(500), frame_timeout),
        );
        let stop_handle = server.stop_handle();
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(|request| match request {
                CorePipeRequest::Hello { .. } => CorePipeResponse::Hello {
                    protocol: V7_PROTOCOL_NAME.into(),
                    version: V7_PROTOCOL_VERSION,
                    pid: std::process::id(),
                },
                CorePipeRequest::Command { request_id, .. } => CorePipeResponse::Events {
                    request_id,
                    events: Vec::new(),
                },
                other => CorePipeResponse::Error {
                    request_id: None,
                    code: "unexpected".into(),
                    message: format!("unexpected: {other:?}"),
                },
            });
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let server_for_thread = Arc::clone(&server);
        let server_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            server_for_thread.serve_loop_with_ready(server_stop, handler, ready_tx)
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("ready timed out")
            .expect("ready failed");

        // While the accept loop is waiting in ConnectNamedPipe, it holds 1 pre-claimed
        // listening slot; each active client handler claims 1 additional slot.
        assert_eq!(server.claimed_slots(), 1);
        let wait_idle_slots = |label: &str| {
            let deadline = Instant::now() + Duration::from_secs(1);
            while server.claimed_slots() > 1 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(
                server.claimed_slots(),
                1,
                "connection slot was not freed after {label}"
            );
        };

        // 1. Incomplete payload: 4-byte length claims 64 bytes, only 4 bytes sent.
        {
            let mut client = NamedPipeClient::connect(&name, 2_000).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(2)));
            let mut partial = 64u32.to_le_bytes().to_vec();
            partial.extend_from_slice(b"part");
            let started = Instant::now();
            client.write_all(&partial).unwrap();
            let occupied_deadline = Instant::now() + Duration::from_millis(100);
            while server.claimed_slots() < 2 && Instant::now() < occupied_deadline {
                thread::sleep(Duration::from_millis(2));
            }
            assert!(
                server.claimed_slots() >= 2,
                "expected active handler slot while partial frame is in flight"
            );
            match read_message::<CorePipeResponse>(&mut client)
                .expect("expected protocol_violation read to succeed")
                .expect("expected protocol_violation frame")
            {
                CorePipeResponse::Error { code, message, .. } => {
                    assert_eq!(code, "protocol_violation");
                    assert!(
                        message.contains("timed out"),
                        "unexpected timeout message: {message}"
                    );
                }
                other => panic!("expected protocol_violation, got {other:?}"),
            }
            let elapsed = started.elapsed();
            assert!(
                elapsed >= Duration::from_millis(90) && elapsed < Duration::from_secs(1),
                "unexpected incomplete-payload timeout duration: {elapsed:?}"
            );
            assert!(matches!(
                read_message::<CorePipeResponse>(&mut client),
                Ok(None)
            ));
            drop(client);
            wait_idle_slots("incomplete payload timeout");
        }

        // 2. Drip-feed payload: progress MUST NOT renew the absolute body deadline.
        {
            let mut client = NamedPipeClient::connect(&name, 2_000).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(2)));
            client.set_write_timeout(Some(Duration::from_millis(200)));
            let started = Instant::now();
            client.write_all(&32u32.to_le_bytes()).unwrap();
            for _ in 0..8 {
                if client.write_all(b"x").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(30));
            }
            match read_message::<CorePipeResponse>(&mut client)
                .expect("expected protocol_violation read")
                .expect("expected protocol_violation frame")
            {
                CorePipeResponse::Error { code, message, .. } => {
                    assert_eq!(code, "protocol_violation");
                    assert!(
                        message.contains("timed out"),
                        "unexpected drip-feed message: {message}"
                    );
                }
                other => panic!("expected protocol_violation, got {other:?}"),
            }
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_secs(1),
                "drip-feed renewed body deadline: {elapsed:?}"
            );
            drop(client);
            wait_idle_slots("drip-feed body timeout");
        }

        // 3. Partial header: only 2 of 4 header bytes sent.
        {
            let mut client = NamedPipeClient::connect(&name, 2_000).expect("connect");
            client.set_read_timeout(Some(Duration::from_secs(2)));
            let started = Instant::now();
            client.write_all(&[10u8, 0u8]).unwrap();
            match read_message::<CorePipeResponse>(&mut client)
                .expect("expected protocol_violation read")
                .expect("expected protocol_violation frame")
            {
                CorePipeResponse::Error { code, message, .. } => {
                    assert_eq!(code, "protocol_violation");
                    assert!(
                        message.contains("timed out"),
                        "unexpected partial-header message: {message}"
                    );
                }
                other => panic!("expected protocol_violation, got {other:?}"),
            }
            let elapsed = started.elapsed();
            assert!(
                elapsed >= Duration::from_millis(90) && elapsed < Duration::from_secs(1),
                "unexpected partial-header timeout duration: {elapsed:?}"
            );
            drop(client);
            wait_idle_slots("partial header timeout");
        }

        // 4. Server remains completely healthy and serves a normal client afterwards.
        {
            let mut client = CoreIpcClient::connect_pipe(&name).expect("healthy client connect");
            assert!(client.command(CoreCommand::Ping).unwrap().is_empty());
            drop(client);
            wait_idle_slots("healthy follow-up client");
        }

        stop.store(true, Ordering::SeqCst);
        stop_handle.signal();
        worker.join().unwrap().unwrap();
        assert_eq!(
            server.claimed_slots(),
            0,
            "all slots must be released after server shutdown"
        );
    }

    #[cfg(windows)]
    #[test]
    fn overlapped_named_pipe_client_timeout_cancels_cleanly() {
        let name = unique_test_pipe_name("client-timeout");
        let stop = Arc::new(AtomicBool::new(false));
        let server = NamedPipeServer::new(name.clone());
        let stop_handle = server.stop_handle();
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(|request| {
                thread::sleep(Duration::from_millis(350));
                match request {
                    CorePipeRequest::Command { request_id, .. } => CorePipeResponse::Events {
                        request_id,
                        events: Vec::new(),
                    },
                    _ => CorePipeResponse::Hello {
                        protocol: V7_PROTOCOL_NAME.into(),
                        version: V7_PROTOCOL_VERSION,
                        pid: std::process::id(),
                    },
                }
            });
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let server_stop = Arc::clone(&stop);
        let worker =
            thread::spawn(move || server.serve_loop_with_ready(server_stop, handler, ready_tx));
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("ready timed out")
            .expect("ready failed");

        let mut client = NamedPipeClient::connect(&name, 2_000).expect("connect");
        let started = Instant::now();
        let err = client
            .request_with_timeout(
                &CorePipeRequest::Command {
                    request_id: 1,
                    command: CoreCommand::Ping,
                },
                Duration::from_millis(80),
            )
            .unwrap_err();
        let elapsed = started.elapsed();
        assert!(
            err.contains("timed out"),
            "expected client read timeout error, got: {err}"
        );
        assert!(
            elapsed >= Duration::from_millis(60) && elapsed < Duration::from_millis(300),
            "client timeout took unexpected duration: {elapsed:?}"
        );

        drop(client);
        stop.store(true, Ordering::SeqCst);
        stop_handle.signal();
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn snapshot_state_with_browser_reports_connection_flag_across_paged_and_legacy_paths() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let paged_mode = Arc::new(AtomicBool::new(true));
        let server_paged = Arc::clone(&paged_mode);
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(move |request| match request {
                CorePipeRequest::Hello { .. } => CorePipeResponse::Hello {
                    protocol: V7_PROTOCOL_NAME.into(),
                    version: V7_PROTOCOL_VERSION,
                    pid: std::process::id(),
                },
                CorePipeRequest::Capabilities { request_id } => CorePipeResponse::Capabilities {
                    request_id,
                    product_version: "10.1.0".into(),
                    protocol_version: V7_PROTOCOL_VERSION,
                    commands: if server_paged.load(Ordering::SeqCst) {
                        vec!["list_tasks_paged".into(), "event_batch_resync".into()]
                    } else {
                        Vec::new()
                    },
                    settings: Vec::new(),
                    max_frame_bytes: V7_PIPE_MAX_FRAME as u64,
                },
                CorePipeRequest::ListTasks { request_id, .. } => CorePipeResponse::TaskPage {
                    request_id,
                    page: hls_protocol::TaskPage {
                        tasks: Vec::new(),
                        next: None,
                        sequence: 5,
                        browser_connected: true,
                    },
                },
                CorePipeRequest::Snapshot { request_id } => CorePipeResponse::Snapshot {
                    request_id,
                    tasks: Vec::new(),
                    latest_sequence: 9,
                    browser_connected: false,
                },
                _ => CorePipeResponse::Error {
                    request_id: None,
                    code: "unexpected".into(),
                    message: "unexpected request".into(),
                },
            });
        let server_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || serve_tcp_listener(listener, server_stop, handler));

        let mut client = CoreIpcClient::connect_addr(addr).unwrap();
        let (tasks, seq, browser_connected) = client.snapshot_state_with_browser().unwrap();
        assert!(tasks.is_empty());
        assert_eq!(seq, 5);
        assert!(browser_connected);

        paged_mode.store(false, Ordering::SeqCst);
        let (tasks, seq, browser_connected) = client.snapshot_state_with_browser().unwrap();
        assert!(tasks.is_empty());
        assert_eq!(seq, 9);
        assert!(!browser_connected);
        assert_eq!(client.snapshot_state().unwrap().1, 9);

        drop(client);
        stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(50));
        worker.join().unwrap().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn pipe_endpoints_reject_empty_or_interior_nul_names() {
        assert!(NamedPipeClient::connect("", 10).is_err());
        assert!(NamedPipeClient::connect("\\\\.\\pipe\\HLSDownloader\0trunc", 10).is_err());
        let server = NamedPipeServer::new("\\\\.\\pipe\\HLSDownloader\0trunc");
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(|_: CorePipeRequest| CorePipeResponse::Error {
            request_id: None,
            code: "unused".into(),
            message: String::new(),
        });
        assert!(server.serve_loop(stop, handler).is_err());
    }
}
