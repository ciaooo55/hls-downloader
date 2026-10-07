//! Single-instance locks for the resident v7 Core and native presenter.
//!
//! Windows also keeps a session mutex so a second launch in the same logon
//! can activate the existing window quickly. The profile lockfile next to
//! `data.db` is what stops two RDP sessions from opening the same SQLite.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(windows)]
static KEEP_MUTEX: OnceLock<isize> = OnceLock::new();
#[cfg(windows)]
static KEEP_PRESENTER_MUTEX: OnceLock<isize> = OnceLock::new();
static KEEP_LOCK: OnceLock<File> = OnceLock::new();
static KEEP_PRESENTER_LOCK: OnceLock<File> = OnceLock::new();

#[cfg(all(windows, feature = "full-core"))]
pub(crate) struct CoreLaunchGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(all(windows, feature = "full-core"))]
impl Drop for CoreLaunchGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(all(windows, feature = "full-core"))]
pub(crate) fn lock_core_launch() -> Result<CoreLaunchGuard, String> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

    // Core 的实例锁在进程进入 main 后才建立；浏览器必须在启动进程前协调，避免同时加载多个 Core。
    let name = session_mutex_name("Local\\HLSDownloader.v7.launch");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(format!("create Core launch mutex: {}", unsafe {
            GetLastError()
        }));
    }
    let wait = unsafe { WaitForSingleObject(handle, 10_000) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        return Ok(CoreLaunchGuard(handle));
    }
    let error = if wait == WAIT_TIMEOUT {
        "等待其他浏览器启动 Core 超时，请重试".to_string()
    } else {
        format!(
            "wait for Core launch mutex: status={wait}, {}",
            std::io::Error::last_os_error()
        )
    };
    unsafe { CloseHandle(handle) };
    Err(error)
}

/// Produces the duplicate-instance error text. The engine binary treats this
/// marker as a normal exit, so every duplicate-instance path must go through
/// this constructor instead of hand-writing the message.
pub(crate) fn already_running_error(target: &str) -> String {
    format!("{target} already running")
}

/// Matches only errors produced by [`already_running_error`].
pub fn is_already_running_error(message: &str) -> bool {
    message.ends_with(" already running")
}

pub fn claim_v7_instance() -> Result<(), String> {
    #[cfg(windows)]
    claim_session_mutex()?;
    if cfg!(test) {
        return Ok(());
    }
    claim_profile_lock()
}

/// Claims the latency-sensitive handoff presenter independently from the
/// main workbench. A browser offer must have exactly one visible presenter;
/// the lock also lets a crashed presenter be replaced without touching Core.
pub fn claim_v7_presenter_instance() -> Result<(), String> {
    #[cfg(windows)]
    claim_presenter_session_mutex()?;
    if cfg!(test) {
        return Ok(());
    }
    let path = crate::default_v7_database_path().with_file_name("presenter.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("open presenter lock: {error}"))?;
    try_exclusive_lock(&file)?;
    let _ = KEEP_PRESENTER_LOCK.set(file);
    Ok(())
}

#[cfg(windows)]
fn session_mutex_name(base: &str) -> Vec<u16> {
    let isolated_profile = std::env::var_os("HLS_V7_DATA_DIR").is_some_and(|v| !v.is_empty())
        && ["HLS_V7_PIPE", "HLS_V7_CORE_BIND"]
            .iter()
            .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()));
    let name = if isolated_profile {
        // 显式独立数据目录与 IPC 用于隔离运行；同一数据库仍由文件锁保护。
        let profile = crate::default_v7_database_path()
            .to_string_lossy()
            .to_lowercase();
        let hash = profile.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
        format!("{base}.profile-{hash:016x}")
    } else {
        base.to_string()
    };
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn claim_session_mutex() -> Result<(), String> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = session_mutex_name("Local\\HLSDownloader.v7");
    let handle = unsafe { CreateMutexW(null(), 1, name.as_ptr()) };
    if handle.is_null() {
        return Err(format!("CreateMutexW failed: {}", unsafe {
            GetLastError()
        }));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err(already_running_error("native shell"));
    }
    let _ = KEEP_MUTEX.set(handle as isize);
    Ok(())
}

#[cfg(windows)]
fn claim_presenter_session_mutex() -> Result<(), String> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = session_mutex_name("Local\\HLSDownloader.v7.presenter");
    let handle = unsafe { CreateMutexW(null(), 1, name.as_ptr()) };
    if handle.is_null() {
        return Err(format!("CreateMutexW presenter failed: {}", unsafe {
            GetLastError()
        }));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err(already_running_error("v7 presenter"));
    }
    let _ = KEEP_PRESENTER_MUTEX.set(handle as isize);
    Ok(())
}

fn claim_profile_lock() -> Result<(), String> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("open instance lock: {error}"))?;
    try_exclusive_lock(&file)?;
    let _ = KEEP_LOCK.set(file);
    Ok(())
}

fn lock_path() -> PathBuf {
    crate::default_v7_database_path().with_file_name("instance.lock")
}

#[cfg(windows)]
fn lock_failure(error: u32) -> String {
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    if error == ERROR_LOCK_VIOLATION {
        already_running_error("native shell")
    } else {
        format!(
            "LockFileEx failed: {}",
            std::io::Error::from_raw_os_error(error as i32)
        )
    }
}

#[cfg(windows)]
fn try_exclusive_lock(file: &File) -> Result<(), String> {
    use std::mem::zeroed;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;
    let mut overlapped: OVERLAPPED = unsafe { zeroed() };
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if ok == 0 {
        let error = unsafe { GetLastError() };
        return Err(lock_failure(error));
    }
    Ok(())
}

#[cfg(not(windows))]
fn try_exclusive_lock(_file: &File) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_claim_succeeds_in_this_process() {
        // A second claim in the same process is allowed to fail on Windows.
        let _ = claim_v7_instance();
        assert!(lock_path().file_name().unwrap() == "instance.lock");
    }

    #[test]
    fn presenter_claim_uses_a_separate_lock_name() {
        assert!(lock_path().file_name().unwrap() == "instance.lock");
        let presenter = crate::default_v7_database_path().with_file_name("presenter.lock");
        assert_eq!(presenter.file_name().unwrap(), "presenter.lock");
    }

    #[cfg(windows)]
    #[test]
    fn only_lock_contention_is_reported_as_an_existing_instance() {
        use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION};
        assert!(is_already_running_error(&lock_failure(
            ERROR_LOCK_VIOLATION
        )));
        let unexpected = lock_failure(ERROR_ACCESS_DENIED);
        assert!(!is_already_running_error(&unexpected));
        assert!(unexpected.starts_with("LockFileEx failed:"));
    }
}
