//! Keep Windows awake only while a transfer or live recording is active.

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::sync::{mpsc, OnceLock};

static ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static POWER_THREAD: OnceLock<Option<mpsc::Sender<bool>>> = OnceLock::new();

#[cfg(windows)]
const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
#[cfg(windows)]
const ES_AWAYMODE_REQUIRED: u32 = 0x0000_0040;
#[cfg(windows)]
const ES_CONTINUOUS: u32 = 0x8000_0000;

pub fn set_active(active: bool) {
    if ACTIVE.swap(active, Ordering::SeqCst) == active {
        return;
    }
    #[cfg(windows)]
    {
        let Some(sender) = power_thread() else {
            if active {
                ACTIVE.store(false, Ordering::SeqCst);
            }
            return;
        };
        if sender.send(active).is_err() && active {
            ACTIVE.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(windows)]
fn power_thread() -> Option<&'static mpsc::Sender<bool>> {
    POWER_THREAD
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            std::thread::Builder::new()
                .name("v7-sleep-inhibit".into())
                .spawn(move || {
                    while let Ok(active) = receiver.recv() {
                        apply_execution_state(active);
                    }
                    apply_execution_state(false);
                })
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

#[cfg(windows)]
fn apply_execution_state(active: bool) {
    unsafe {
        use windows_sys::Win32::System::Power::SetThreadExecutionState;
        let mut flags = ES_CONTINUOUS;
        if active {
            flags |= ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED;
        }
        if SetThreadExecutionState(flags) == 0 && active {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        }
    }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_is_idempotent() {
        set_active(false);
        set_active(true);
        assert!(is_active());
        set_active(true);
        set_active(false);
        assert!(!is_active());
    }
}
