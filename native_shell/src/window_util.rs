//! HWND helpers: overlay caption drag, player parent, OS reduce-motion.

pub fn window_handle_by_title(title: &str) -> Option<i64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new(title)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let hwnd = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(std::ptr::null(), wide.as_ptr())
        };
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd as i64)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        None
    }
}

pub fn begin_caption_drag(title: &str) -> bool {
    #[cfg(windows)]
    {
        let Some(hwnd) = window_handle_by_title(title) else {
            return false;
        };
        const WM_SYSCOMMAND: u32 = 0x0112;
        const SC_MOVE: usize = 0xF010;
        const HTCAPTION: usize = 2;
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                hwnd as windows_sys::Win32::Foundation::HWND,
                WM_SYSCOMMAND,
                SC_MOVE | HTCAPTION,
                0,
            );
        }
        true
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}

pub fn os_reduce_motion() -> bool {
    #[cfg(windows)]
    {
        const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
        let mut enabled: i32 = 1;
        let ok = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                &mut enabled as *mut i32 as *mut _,
                0,
            )
        };
        ok != 0 && enabled == 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_title_does_not_drag() {
        assert!(window_handle_by_title("HLSDownloader-no-such-window-title").is_none());
        assert!(!begin_caption_drag("HLSDownloader-no-such-window-title"));
    }
}
