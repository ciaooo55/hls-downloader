//! HWND helpers: overlay caption drag, player parent, OS reduce-motion.

#[cfg(any(windows, test))]
fn window_long_ptr_call_succeeded(value: isize, last_error: u32) -> bool {
    value != 0 || last_error == 0
}

/// 把一个轴上居中到工作区，**并把偏移夹到非负**。
///
/// 不夹会怎样：窗口比工作区还高时 `(工作区高 − 窗口高) / 2` 是负数，
/// 窗口顶边被推到工作区上方，标题栏和关闭按钮都够不到，用户只能改缩放或拖任务栏。
/// presenter 的窗口尺寸是写死的逻辑值（`hot.slint` 里 620×584 / 400×204 / 440×188），
/// 在 1366×768@150%（工作区约 911×480）、1600×900@150%（约 1067×568）这类档位上确实会大于工作区。
///
/// 夹取后窗口仍可能比工作区大，但至少**顶边/左边贴着工作区**，标题栏可见可点。
#[cfg(any(windows, test))]
fn clamp_centered_axis(work_start: i32, work_size: i32, window_size: i32) -> i32 {
    work_start + ((work_size - window_size) / 2).max(0)
}

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

/// 只读查询窗口最近显示器的工作区物理尺寸；不改变窗口位置、大小或激活状态。
pub fn window_work_area_size_by_title(title: &str) -> Option<(u32, u32)> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{HWND, RECT};
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };

        let hwnd = window_handle_by_title(title)? as HWND;
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return None;
        }
        let empty_rect = || RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: empty_rect(),
            rcWork: empty_rect(),
            dwFlags: 0,
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return None;
        }
        let width = u32::try_from(info.rcWork.right - info.rcWork.left).ok()?;
        let height = u32::try_from(info.rcWork.bottom - info.rcWork.top).ok()?;
        (width > 0 && height > 0).then_some((width, height))
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
        let x = clamp_centered_axis(
            info.rcWork.left,
            info.rcWork.right - info.rcWork.left,
            width,
        );
        let y = clamp_centered_axis(
            info.rcWork.top,
            info.rcWork.bottom - info.rcWork.top,
            height,
        );
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
        use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
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
            SetLastError(0);
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if !window_long_ptr_call_succeeded(style, GetLastError()) {
                return false;
            }
            let next = (style & !(WS_EX_APPWINDOW as isize)) | WS_EX_TOOLWINDOW as isize;
            if next != style {
                SetLastError(0);
                let previous = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
                if !window_long_ptr_call_succeeded(previous, GetLastError()) {
                    return false;
                }
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
        assert!(window_work_area_size_by_title("HLSDownloader-no-such-window-title").is_none());
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

    #[test]
    fn zero_window_long_result_only_fails_with_a_win32_error() {
        assert!(window_long_ptr_call_succeeded(0, 0));
        assert!(window_long_ptr_call_succeeded(1, 5));
        assert!(!window_long_ptr_call_succeeded(0, 5));
    }

    #[test]
    fn centering_never_pushes_a_window_above_or_left_of_the_work_area() {
        // 放得下：正常居中（这些值必须和旧写法完全一致，不能引入行为变化）。
        assert_eq!(clamp_centered_axis(0, 1920, 620), 650);
        assert_eq!(clamp_centered_axis(0, 1080, 584), 248);
        assert_eq!(clamp_centered_axis(100, 1000, 400), 400);

        // 放不下：偏移必须夹到 0，窗口顶边/左边贴住工作区，而不是被推到工作区外面。
        // 旧写法 `start + (size - window)/2` 在第一种情形会得到 -52。
        assert_eq!(clamp_centered_axis(0, 480, 584), 0); // 1366×768@150% vs presenter 620×584
        assert_eq!(clamp_centered_axis(0, 568, 584), 0); // 1600×900@150%
        assert_eq!(clamp_centered_axis(0, 516, 584), 0); // 1920×1080@200%
        assert_eq!(clamp_centered_axis(200, 911, 1024), 200);

        // 恰好相等：偏移 0，不越界也不留缝。
        assert_eq!(clamp_centered_axis(0, 584, 584), 0);
    }
}
