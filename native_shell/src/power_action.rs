//! After-complete power actions. 5.x waits 30s and lets the user cancel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

static CANCEL: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Option<String>> = Mutex::new(None);

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

pub fn label(action: &str) -> &'static str {
    match normalize(action) {
        Ok("shutdown") => "关机",
        Ok("sleep") => "睡眠",
        Ok("hibernate") => "休眠",
        _ => "",
    }
}

pub fn pending() -> Option<String> {
    PENDING.lock().ok().and_then(|guard| guard.clone())
}

pub fn cancel() -> bool {
    CANCEL.store(true, Ordering::SeqCst);
    PENDING
        .lock()
        .map(|mut guard| guard.take().is_some())
        .unwrap_or(false)
}

pub fn confirm() -> Result<bool, String> {
    CANCEL.store(true, Ordering::SeqCst);
    let action = PENDING
        .lock()
        .map_err(|_| "电源动作状态不可用".to_string())?
        .take();
    match action {
        Some(action) => {
            execute(&action)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub fn schedule(action: &str, delay_secs: u64) -> Result<(), String> {
    let action = normalize(action)?.to_string();
    let _ = cancel();
    CANCEL.store(false, Ordering::SeqCst);
    if let Ok(mut guard) = PENDING.lock() {
        *guard = Some(action.clone());
    }
    let wait = delay_secs.max(1);
    thread::spawn(move || {
        for _ in 0..wait {
            if CANCEL.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
        if CANCEL.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(mut guard) = PENDING.lock() {
            if guard.as_deref() == Some(action.as_str()) {
                guard.take();
                drop(guard);
                let _ = execute(&action);
            }
        }
    });
    Ok(())
}

fn execute(action: &str) -> Result<(), String> {
    if cfg!(test) {
        return Ok(());
    }
    execute_os(action)
}

#[cfg(windows)]
fn execute_os(action: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = match action {
        "shutdown" => {
            let mut command = std::process::Command::new("shutdown.exe");
            command.args(["/s", "/t", "0"]);
            command
        }
        "hibernate" => {
            let mut command = std::process::Command::new("shutdown.exe");
            command.args(["/h"]);
            command
        }
        "sleep" => {
            let mut command = std::process::Command::new("rundll32.exe");
            command.args(["powrprof.dll,SetSuspendState", "0,1,0"]);
            command
        }
        _ => return Err("unknown power action".into()),
    };
    command
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
        assert_eq!(label("sleep"), "睡眠");
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
}
