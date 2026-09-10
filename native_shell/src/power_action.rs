//! After-complete power actions. 5.x waits 30s and lets the user cancel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

struct PendingAction {
    action: String,
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

pub fn label(action: &str) -> &'static str {
    match normalize(action) {
        Ok("shutdown") => "关机",
        Ok("sleep") => "睡眠",
        Ok("hibernate") => "休眠",
        _ => "",
    }
}

pub fn pending() -> Option<String> {
    PENDING
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|pending| pending.action.clone()))
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

pub fn schedule(action: &str, delay_secs: u64) -> Result<(), String> {
    let action = normalize(action)?.to_string();
    let generation = replace_pending(action.clone())?;
    let wait = delay_secs.max(1);
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

fn replace_pending(action: String) -> Result<u64, String> {
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);
    let mut guard = PENDING
        .lock()
        .map_err(|_| "电源动作状态不可用".to_string())?;
    *guard = Some(PendingAction { action, generation });
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

    #[test]
    fn rescheduling_same_action_invalidates_the_old_generation() {
        let _guard = TEST_LOCK.lock().unwrap();
        let old_generation = replace_pending("shutdown".into()).unwrap();
        let new_generation = replace_pending("shutdown".into()).unwrap();
        assert_ne!(old_generation, new_generation);
        assert_eq!(take_pending_generation(old_generation), None);
        assert_eq!(pending().as_deref(), Some("shutdown"));
        assert_eq!(
            take_pending_generation(new_generation).as_deref(),
            Some("shutdown")
        );
        assert!(pending().is_none());
    }
}
