//! Keep Windows awake only while a transfer or live recording is active.

use std::sync::atomic::{AtomicBool, Ordering};

static ACTIVE: AtomicBool = AtomicBool::new(false);

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
