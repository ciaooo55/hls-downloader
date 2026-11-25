//! After-complete power actions. 5.x waits 30s and lets the user cancel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

struct PendingAction {
    action: String,
    title: String,
    deadline: Instant,
    generation: u64,
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
static PENDING: Mutex<Option<PendingAction>> = Mutex::new(None);

pub fn normalize(action: &str) -> Result<&'static str, String> {
    match action.trim().to_ascii_lowercase().as_str() {
        "" | "none" | "off" => Err("none".into()),
        "shutdown" => Ok("shutdown"),
        "sleep" => Ok("sleep"),
        "hibernate" => Ok("hibernate"),
        other => Err(format!("不支持的完成后电源动作: {other}")),
    }
}

pub fn is_armed(action: &str) -> bool {
    normalize(action).is_ok()
}

pub fn pending() -> Option<String> {
    PENDING
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|pending| pending.action.clone()))
}

pub fn pending_details() -> Option<(String, String, u64)> {
    PENDING.lock().ok().and_then(|guard| {
        guard.as_ref().map(|pending| {
            let remaining = pending
                .deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                .max(1);
            (pending.action.clone(), pending.title.clone(), remaining)
        })
    })
}

pub fn cancel() -> bool {
    PENDING
        .lock()
        .map(|mut guard| guard.take().is_some())
        .unwrap_or(false)
}

pub fn confirm() -> Result<bool, String> {
    let action = PENDING
        .lock()
        .map_err(|_| "电源动作状态不可用".to_string())?
        .take()
        .map(|pending| pending.action);
    match action {
        Some(action) => {
            execute(&action)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn schedule(action: &str, delay_secs: u64) -> Result<(), String> {
    schedule_with_title(action, "", delay_secs)
}

pub fn schedule_with_title(action: &str, title: &str, delay_secs: u64) -> Result<(), String> {
    let action = normalize(action)?.to_string();
    let wait = delay_secs.max(1);
    let generation = replace_pending(
        action.clone(),
        title.to_string(),
        Instant::now() + Duration::from_secs(wait),
    )?;
    thread::spawn(move || {
        for _ in 0..wait {
            if !pending_generation_matches(generation) {
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
        if let Some(action) = take_pending_generation(generation) {
            let _ = execute(&action);
        }
    });
    Ok(())
}

fn replace_pending(action: String, title: String, deadline: Instant) -> Result<u64, String> {
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);
    let mut guard = PENDING
        .lock()
        .map_err(|_| "电源动作状态不可用".to_string())?;
    *guard = Some(PendingAction {
        action,
        title,
        deadline,
        generation,
    });
    Ok(generation)
}

fn pending_generation_matches(generation: u64) -> bool {
    PENDING
        .lock()
        .map(|guard| {
            guard
                .as_ref()
                .is_some_and(|pending| pending.generation == generation)
        })
        .unwrap_or(false)
}

fn take_pending_generation(generation: u64) -> Option<String> {
    PENDING.lock().ok().and_then(|mut guard| {
        if guard
            .as_ref()
            .is_some_and(|pending| pending.generation == generation)
        {
            guard.take().map(|pending| pending.action)
        } else {
            None
        }
    })
}

fn execute(action: &str) -> Result<(), String> {
    if cfg!(test) {
        return Ok(());
    }
    execute_os(action)
}

#[cfg(windows)]
fn system_executable(name: &str) -> Result<std::path::PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buffer = vec![0u16; 260];
    let len = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if len == 0 {
        return Err(format!(
            "GetSystemDirectoryW failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let dir = if len >= buffer.len() {
        let mut larger = vec![0u16; len + 1];
        let actual =
            unsafe { GetSystemDirectoryW(larger.as_mut_ptr(), larger.len() as u32) } as usize;
        if actual == 0 || actual >= larger.len() {
            return Err("GetSystemDirectoryW returned invalid path".into());
        }
        std::path::PathBuf::from(OsString::from_wide(&larger[..actual]))
    } else {
        std::path::PathBuf::from(OsString::from_wide(&buffer[..len]))
    };
    Ok(dir.join(name))
}

#[cfg(windows)]
fn suspend() -> Result<(), String> {
    // The process token is shared; overlapping suspend requests must restore it in order.
    static SUSPEND_LOCK: Mutex<()> = Mutex::new(());
    let _guard = SUSPEND_LOCK.lock().map_err(|_| "suspend lock poisoned")?;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = std::ptr::null_mut();
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    } == 0
    {
        return Err(format!(
            "OpenProcessToken failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let name: Vec<u16> = "SeShutdownPrivilege\0".encode_utf16().collect();
    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    let ok = unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } != 0;
    if !ok {
        let err = std::io::Error::last_os_error();
        unsafe { CloseHandle(token) };
        return Err(format!("LookupPrivilegeValueW failed: {err}"));
    }
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let mut previous = privileges;
    let mut size = 0;
    let adjusted = unsafe {
        AdjustTokenPrivileges(
            token,
            0,
            &privileges,
            std::mem::size_of_val(&previous) as u32,
            &mut previous,
            &mut size,
        )
    } != 0;
    let last_error = unsafe { GetLastError() };
    if !adjusted || last_error == ERROR_NOT_ALL_ASSIGNED {
        unsafe { CloseHandle(token) };
        return Err(format!("AdjustTokenPrivileges failed: {last_error}"));
    }
    let ok = unsafe { windows_sys::Win32::System::Power::SetSuspendState(0, 0, 0) };
    let suspend_error = std::io::Error::last_os_error();
    let restored = unsafe {
        AdjustTokenPrivileges(
            token,
            0,
            &previous,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    let restore_error = unsafe { GetLastError() };
    unsafe { CloseHandle(token) };
    if restored == 0 || restore_error == ERROR_NOT_ALL_ASSIGNED {
        return Err(format!("restore shutdown privilege: {restore_error}"));
    }
    if ok == 0 {
        return Err(format!("SetSuspendState failed: {suspend_error}"));
    }
    Ok(())
}

#[cfg(windows)]
fn execute_os(action: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if action == "sleep" {
        return suspend();
    }
    let shutdown_exe = system_executable("shutdown.exe")?;
    let mut command = match action {
        "shutdown" => {
            let mut command = std::process::Command::new(&shutdown_exe);
            command.args(["/s", "/t", "0"]);
            command
        }
        "hibernate" => {
            let mut command = std::process::Command::new(&shutdown_exe);
            command.args(["/h"]);
            command
        }
        _ => return Err("unknown power action".into()),
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn execute_os(_action: &str) -> Result<(), String> {
    Err("电源动作仅支持 Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn none_is_not_armed() {
        assert!(!is_armed("none"));
        assert!(!is_armed(""));
        assert!(is_armed("shutdown"));
    }

    #[test]
    fn cancel_clears_pending() {
        let _guard = TEST_LOCK.lock().unwrap();
        schedule("shutdown", 30).unwrap();
        assert_eq!(pending().as_deref(), Some("shutdown"));
        assert!(cancel());
        assert!(pending().is_none());
    }

    #[test]
    fn confirm_executes_only_one_pending_action() {
        let _guard = TEST_LOCK.lock().unwrap();
        schedule("sleep", 30).unwrap();
        assert_eq!(pending().as_deref(), Some("sleep"));
        assert!(confirm().unwrap());
        assert!(pending().is_none());
        assert!(!confirm().unwrap());
    }

    #[test]
    fn rescheduling_same_action_invalidates_the_old_generation() {
        let _guard = TEST_LOCK.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let old_generation =
            replace_pending("shutdown".into(), "first.mp4".into(), deadline).unwrap();
        let new_generation =
            replace_pending("shutdown".into(), "second.mp4".into(), deadline).unwrap();
        assert_ne!(old_generation, new_generation);
        assert_eq!(take_pending_generation(old_generation), None);
        assert_eq!(pending().as_deref(), Some("shutdown"));
        let (_, title, remaining) = pending_details().unwrap();
        assert_eq!(title, "second.mp4");
        assert!(remaining >= 1);
        assert_eq!(
            take_pending_generation(new_generation).as_deref(),
            Some("shutdown")
        );
        assert!(pending().is_none());
    }
}
