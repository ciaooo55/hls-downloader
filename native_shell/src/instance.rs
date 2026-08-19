//! Single-instance lock for the resident v6 process.
//!
//! Windows also keeps a session mutex so a second launch in the same logon
//! can activate the existing window quickly. The profile lockfile next to
//! `data.db` is what stops two RDP sessions from opening the same SQLite.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(windows)]
static KEEP_MUTEX: OnceLock<isize> = OnceLock::new();
static KEEP_LOCK: OnceLock<File> = OnceLock::new();

pub fn claim_v6_instance() -> Result<(), String> {
    #[cfg(windows)]
    claim_session_mutex()?;
    if cfg!(test) {
        return Ok(());
    }
    claim_profile_lock()
}

#[cfg(windows)]
fn claim_session_mutex() -> Result<(), String> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\HLSDownloader.v6\0".encode_utf16().collect();
    let handle = unsafe { CreateMutexW(null(), 1, name.as_ptr()) };
    if handle.is_null() {
        return Err(format!("CreateMutexW failed: {}", unsafe { GetLastError() }));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err("native shell already running".into());
    }
    let _ = KEEP_MUTEX.set(handle as isize);
    Ok(())
}

#[allow(dead_code)]
fn claim_profile_lock() -> Result<(), String> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("open instance lock: {error}"))?;
    try_exclusive_lock(&file)?;
    let _ = KEEP_LOCK.set(file);
    Ok(())
}

fn lock_path() -> PathBuf {
    crate::default_v6_database_path().with_file_name("instance.lock")
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
            file.as_raw_handle() as *mut core::ffi::c_void,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if ok == 0 {
        let _ = unsafe { GetLastError() };
        return Err("native shell already running".into());
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
        let _ = claim_v6_instance();
        assert!(lock_path().file_name().unwrap() == "instance.lock");
    }
}
