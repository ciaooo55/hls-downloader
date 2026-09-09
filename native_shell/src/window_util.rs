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
            windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
                std::ptr::null(),
                wide.as_ptr(),
            )
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

pub fn center_window_by_title(title: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{HWND, RECT};
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        };

        let Some(raw_hwnd) = window_handle_by_title(title) else {
            return false;
        };
        let hwnd = raw_hwnd as HWND;
        let empty_rect = || RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut rect = empty_rect();
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return false;
        }
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return false;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: empty_rect(),
            rcWork: empty_rect(),
            dwFlags: 0,
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return false;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let x = info.rcWork.left + (info.rcWork.right - info.rcWork.left - width) / 2;
        let y = info.rcWork.top + (info.rcWork.bottom - info.rcWork.top - height) / 2;
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            ) != 0
        }
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}

/// Bring a transient window to the foreground without leaving the application
/// globally always-on-top. Windows may ignore a plain `show()` when another
/// process owns the foreground; a TOPMOST -> NOTOPMOST pulse makes the user
/// initiated browser confirmation or completion notice visible, then restores
/// normal z-order immediately.
pub fn activate_window_by_title(title: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            BringWindowToTop, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_NOTOPMOST,
            HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE,
        };

        let Some(raw_hwnd) = window_handle_by_title(title) else {
            return false;
        };
        let hwnd = raw_hwnd as windows_sys::Win32::Foundation::HWND;
        unsafe {
            ShowWindow(hwnd, SW_RESTORE);
            let raised = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            ) != 0;
            let _ = BringWindowToTop(hwnd);
            let focused = SetForegroundWindow(hwnd) != 0;
            let restored = SetWindowPos(
                hwnd,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            ) != 0;
            raised && restored && focused
        }
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}

/// Keep the transient presenter window out of the taskbar while retaining
/// normal foreground activation when a real handoff is shown.
pub fn hide_window_from_taskbar_by_title(title: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_NOTOPMOST,
            SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
            WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
        };

        let Some(raw_hwnd) = window_handle_by_title(title) else {
            return false;
        };
        let hwnd = raw_hwnd as windows_sys::Win32::Foundation::HWND;
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let next = (style & !(WS_EX_APPWINDOW as isize)) | WS_EX_TOOLWINDOW as isize;
            if next != style {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
                SetWindowPos(
                    hwnd,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                ) != 0
            } else {
                true
            }
        }
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
        assert!(!center_window_by_title(
            "HLSDownloader-no-such-window-title"
        ));
        assert!(!activate_window_by_title(
            "HLSDownloader-no-such-window-title"
        ));
        assert!(!hide_window_from_taskbar_by_title(
            "HLSDownloader-no-such-window-title"
        ));
    }
}
