use slint::Window;

pub fn apply_theme<C: slint::ComponentHandle>(component: &C, dark: bool)
where
    for<'a> crate::Tokens<'a>: slint::Global<'a, C>,
{
    let tokens = component.global::<crate::Tokens>();
    tokens.invoke_apply_theme(dark);
    apply_caption_theme(
        component.window(),
        dark,
        tokens.get_surface(),
        tokens.get_ink(),
        tokens.get_line(),
    );
}

pub fn apply_dialog_theme<C: slint::ComponentHandle>(component: &C, dark: bool)
where
    for<'a> crate::Tokens<'a>: slint::Global<'a, C>,
{
    let tokens = component.global::<crate::Tokens>();
    tokens.invoke_apply_theme(dark);
    apply_caption_theme(
        component.window(),
        dark,
        tokens.get_canvas(),
        tokens.get_ink(),
        tokens.get_line(),
    );
}

pub fn apply_appearance<C: slint::ComponentHandle>(component: &C, dark: bool, reduce_motion: bool)
where
    for<'a> crate::Tokens<'a>: slint::Global<'a, C>,
{
    component
        .global::<crate::Tokens>()
        .set_reduce_motion(reduce_motion);
    apply_theme(component, dark);
}

pub fn inherit_appearance<C: slint::ComponentHandle>(component: &C, parent: &crate::WorkbenchWindow)
where
    for<'a> crate::Tokens<'a>: slint::Global<'a, C>,
{
    use slint::ComponentHandle;
    let tokens = parent.global::<crate::Tokens>();
    component
        .global::<crate::Tokens>()
        .set_reduce_motion(tokens.get_reduce_motion());
    apply_dialog_theme(component, tokens.get_dark());
}

#[cfg(any(windows, test))]
pub(crate) fn dwm_color_ref(color: slint::Color) -> u32 {
    u32::from(color.red()) | (u32::from(color.green()) << 8) | (u32::from(color.blue()) << 16)
}

#[cfg(windows)]
pub(crate) fn set_window_cloaked(handle: i64, cloaked: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    let value: u32 = u32::from(cloaked);
    let _ = unsafe {
        DwmSetWindowAttribute(
            handle as _,
            DWMWA_CLOAK as u32,
            std::ptr::from_ref(&value).cast(),
            std::mem::size_of_val(&value) as u32,
        )
    };
}

pub fn set_cloaked(window: &Window, cloaked: bool) {
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        set_window_cloaked(handle, cloaked);
    }
    #[cfg(not(windows))]
    let _ = (window, cloaked);
}

fn apply_caption_theme(
    window: &Window,
    dark: bool,
    background: slint::Color,
    text: slint::Color,
    border: slint::Color,
) {
    #[cfg(windows)]
    {
        use slint::winit_030::{winit::window::Theme, WinitWindowAccessor};
        window.with_winit_window(|native| {
            native.set_theme(Some(if dark { Theme::Dark } else { Theme::Light }));
        });
        if let Some(handle) = hwnd(window) {
            use windows_sys::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
                DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            };
            // Windows 11 supports explicit caption/border colors and rounded corners
            // even when the application and system themes differ. Older Windows keeps defaults.
            for (attribute, value) in [
                (DWMWA_USE_IMMERSIVE_DARK_MODE, u32::from(dark)),
                (DWMWA_CAPTION_COLOR, dwm_color_ref(background)),
                (DWMWA_TEXT_COLOR, dwm_color_ref(text)),
                (DWMWA_BORDER_COLOR, dwm_color_ref(border)),
                (DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND as u32),
            ] {
                let result = unsafe {
                    DwmSetWindowAttribute(
                        handle as _,
                        attribute as u32,
                        std::ptr::from_ref(&value).cast(),
                        std::mem::size_of_val(&value) as u32,
                    )
                };
                crate::trace(&format!("native caption dark={dark} attribute={attribute} value={value:#010x} hresult={result:#010x}"));
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (window, dark, background, text, border);
}

pub fn initialize_app_id() {
    #[cfg(windows)]
    {
        // Launchers may be packaged apps; do not inherit their taskbar/UIA
        // identity for the downloader's own native windows.
        let identity: Vec<u16> = "HLSDownloader\0".encode_utf16().collect();
        unsafe {
            windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(
                identity.as_ptr(),
            );
        }
    }
}

pub fn hwnd(window: &Window) -> Option<i64> {
    #[cfg(windows)]
    {
        use slint::winit_030::{
            winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
            WinitWindowAccessor,
        };
        window
            .with_winit_window(|native| match native.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as i64),
                _ => None,
            })
            .flatten()
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

pub fn activate(window: &Window) {
    if let Some(handle) = hwnd(window) {
        hls_client::activate_window(handle);
    }
}

pub fn is_cursor_inside(window: &Window) -> bool {
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        use windows_sys::Win32::{
            Foundation::{HWND, POINT, RECT},
            UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect},
        };
        unsafe {
            let mut cursor = POINT { x: 0, y: 0 };
            let mut rect: RECT = std::mem::zeroed();
            if GetCursorPos(&mut cursor) != 0 && GetWindowRect(handle as HWND, &mut rect) != 0 {
                return cursor.x >= rect.left
                    && cursor.x < rect.right
                    && cursor.y >= rect.top
                    && cursor.y < rect.bottom;
            }
        }
    }
    #[cfg(not(windows))]
    let _ = window;
    false
}

/// Returns `true` when `SHQueryUserNotificationState` reports a busy,
/// exclusive D3D fullscreen, or presentation session (`QUNS_BUSY = 2`,
/// `QUNS_RUNNING_D3D_FULL_SCREEN = 3`, `QUNS_PRESENTATION_MODE = 4`).
pub fn quns_is_fullscreen_or_busy(state: i32) -> bool {
    matches!(state, 2..=4)
}

/// Returns `true` when a foreground window's outer rect covers its monitor's
/// full bounds (borderless fullscreen video, game, or presentation).
pub fn rect_covers_monitor(
    window_rect: (i32, i32, i32, i32),
    monitor_rect: (i32, i32, i32, i32),
) -> bool {
    monitor_rect.2 > monitor_rect.0
        && monitor_rect.3 > monitor_rect.1
        && window_rect.0 <= monitor_rect.0
        && window_rect.1 <= monitor_rect.1
        && window_rect.2 >= monitor_rect.2
        && window_rect.3 >= monitor_rect.3
}

/// Returns `true` when an HWND is parked off-screen (e.g. `(-32000, -32000)`
/// prewarm/minimized) or unpositioned at `(0, 0)` / degenerate, meaning monitor
/// queries should follow the active cursor monitor rather than `(0, 0)`.
pub fn is_parked_window_rect(rect: (i32, i32, i32, i32)) -> bool {
    rect.2 <= rect.0
        || rect.3 <= rect.1
        || rect.0 <= -16_000
        || rect.1 <= -16_000
        || (rect.0 == 0 && rect.1 == 0)
}

/// Clamps both outer window size and `(x, y)` origin so the window stays fully
/// inside `work_area = (left, top, right, bottom)`. Intentionally parked
/// off-screen windows (`x <= -16000 || y <= -16000`) keep their off-screen origin.
pub fn clamp_window_to_work_area(
    work_area: (i32, i32, i32, i32),
    origin: (i32, i32),
    outer_size: (u32, u32),
) -> ((i32, i32), (u32, u32)) {
    let (left, top, right, bottom) = work_area;
    let area_width = (right - left).max(1) as u32;
    let area_height = (bottom - top).max(1) as u32;
    let width = outer_size.0.clamp(1, area_width);
    let height = outer_size.1.clamp(1, area_height);
    if origin.0 <= -16_000 || origin.1 <= -16_000 {
        return (origin, (width, height));
    }
    let max_x = (right - width as i32).max(left);
    let max_y = (bottom - height as i32).max(top);
    let x = origin.0.clamp(left, max_x);
    let y = origin.1.clamp(top, max_y);
    ((x, y), (width, height))
}

/// Detects whether the user is currently in a fullscreen game, video player, or
/// presentation so background download notifications avoid interrupting or
/// stealing focus.
pub fn is_foreground_fullscreen_or_busy() -> bool {
    #[cfg(windows)]
    {
        #[link(name = "shell32")]
        extern "system" {
            fn SHQueryUserNotificationState(pquns: *mut i32) -> i32;
        }
        use windows_sys::Win32::{
            Foundation::RECT,
            Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            },
            UI::WindowsAndMessaging::{
                GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow,
                GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            },
        };

        unsafe {
            let mut state = 0_i32;
            if SHQueryUserNotificationState(&mut state) >= 0 && quns_is_fullscreen_or_busy(state) {
                return true;
            }
            let fg = GetForegroundWindow();
            if fg.is_null()
                || fg == GetDesktopWindow()
                || fg == GetShellWindow()
                || IsWindowVisible(fg) == 0
                || IsIconic(fg) != 0
            {
                return false;
            }
            let mut pid = 0_u32;
            GetWindowThreadProcessId(fg, &mut pid);
            if pid == std::process::id() {
                return false;
            }
            let mut class_buf = [0_u16; 64];
            let class_len = GetClassNameW(fg, class_buf.as_mut_ptr(), class_buf.len() as i32);
            if class_len > 0 {
                let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);
                if matches!(
                    class_name.as_str(),
                    "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
                ) {
                    return false;
                }
            }
            let mut win_rect: RECT = std::mem::zeroed();
            if GetWindowRect(fg, &mut win_rect) == 0 {
                return false;
            }
            let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            if monitor.is_null() {
                return false;
            }
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return false;
            }
            rect_covers_monitor(
                (win_rect.left, win_rect.top, win_rect.right, win_rect.bottom),
                (
                    info.rcMonitor.left,
                    info.rcMonitor.top,
                    info.rcMonitor.right,
                    info.rcMonitor.bottom,
                ),
            )
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Resolves the physical usable work area `(left, top, right, bottom)` for a
/// window, falling back to the monitor under the mouse cursor when `handle` is
/// `None` or parked at `(-32000, -32000)` / `(0, 0)`.
pub fn work_area(handle: Option<i64>) -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{HWND, POINT, RECT},
            Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO,
                MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
            },
            UI::WindowsAndMessaging::{
                GetCursorPos, GetWindowRect, SystemParametersInfoW, SPI_GETWORKAREA,
            },
        };

        unsafe {
            let monitor = handle
                .and_then(|raw| {
                    let hwnd = raw as HWND;
                    let mut rect: RECT = std::mem::zeroed();
                    if GetWindowRect(hwnd, &mut rect) != 0
                        && !is_parked_window_rect((rect.left, rect.top, rect.right, rect.bottom))
                    {
                        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                        (!mon.is_null()).then_some(mon)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    let mut cursor = POINT { x: 0, y: 0 };
                    if GetCursorPos(&mut cursor) != 0 {
                        let mon = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
                        (!mon.is_null()).then_some(mon)
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| {
                    MonitorFromWindow(std::ptr::null_mut(), MONITOR_DEFAULTTOPRIMARY)
                });

            if !monitor.is_null() {
                let mut info: MONITORINFO = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(monitor, &mut info) != 0
                    && info.rcWork.right > info.rcWork.left
                    && info.rcWork.bottom > info.rcWork.top
                {
                    return Some((
                        info.rcWork.left,
                        info.rcWork.top,
                        info.rcWork.right,
                        info.rcWork.bottom,
                    ));
                }
            }

            let mut work: RECT = std::mem::zeroed();
            if SystemParametersInfoW(SPI_GETWORKAREA, 0, std::ptr::from_mut(&mut work).cast(), 0)
                != 0
                && work.right > work.left
                && work.bottom > work.top
            {
                return Some((work.left, work.top, work.right, work.bottom));
            }
            None
        }
    }
    #[cfg(not(windows))]
    {
        let _ = handle;
        None
    }
}

/// Computes the top-left physical position for a bottom-right toast inside
/// `work_area = (left, top, right, bottom)` with physical `margin`.
/// Never pushes the window above `top` or left of `left`.
pub fn bottom_right_toast_position(
    work_area: (i32, i32, i32, i32),
    window_size: (i32, i32),
    margin: i32,
) -> (i32, i32) {
    let (left, top, right, bottom) = work_area;
    let (width, height) = window_size;
    let margin = margin.max(0);
    let x = (right - width - margin).max(left);
    let y = (bottom - height - margin).max(top);
    (x, y)
}

/// Sets or clears `WS_EX_NOACTIVATE` on an already-created HWND.
pub fn set_no_activate(window: &Window, no_activate: bool) {
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE,
        };
        unsafe {
            let hwnd = handle as windows_sys::Win32::Foundation::HWND;
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let flag = WS_EX_NOACTIVATE as isize;
            let next = if no_activate {
                style | flag
            } else {
                style & !flag
            };
            if style != next {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (window, no_activate);
}

/// Adds `WS_EX_NOACTIVATE` to an already-created HWND so showing or clicking the
/// toast never steals keyboard focus from active user applications.
pub fn mark_no_activate(window: &Window) {
    set_no_activate(window, true);
}

/// Places a toast window in the bottom-right corner above the taskbar with
/// `logical_margin` logical pixels of breathing room, keeping it topmost and
/// never stealing keyboard focus.
pub fn position_bottom_right_toast(window: &Window, logical_margin: f32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{HWND, RECT},
            UI::WindowsAndMessaging::{
                GetWindowRect, IsWindowVisible, SetWindowPos, ShowWindow, HWND_TOPMOST,
                SWP_NOACTIVATE, SWP_NOSIZE, SW_SHOWNOACTIVATE,
            },
        };
        let Some(handle) = hwnd(window) else {
            return false;
        };
        let Some(work) = work_area(Some(handle)) else {
            return false;
        };
        let hwnd = handle as HWND;
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let (width, height) = if unsafe { GetWindowRect(hwnd, &mut rect) } != 0
            && rect.right > rect.left
            && rect.bottom > rect.top
        {
            (rect.right - rect.left, rect.bottom - rect.top)
        } else {
            let size = window.size();
            (size.width as i32, size.height as i32)
        };
        let scale = window.scale_factor();
        let margin = (logical_margin.max(0.0)
            * if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            })
        .round() as i32;
        let (x, y) = bottom_right_toast_position(work, (width, height), margin);
        unsafe {
            let positioned =
                SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) != 0;
            if window.is_visible() && IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            positioned
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (window, logical_margin);
        false
    }
}

/// Computes the top-left physical position for a bottom-right HUD window inside
/// `work_area = (left, top, right, bottom)` with physical `(margin_x, margin_y)`.
pub fn bottom_right_hud_position(
    work_area: (i32, i32, i32, i32),
    window_size: (i32, i32),
    margins: (i32, i32),
) -> (i32, i32) {
    let (left, top, right, bottom) = work_area;
    let (width, height) = window_size;
    let mx = margins.0.max(0);
    let my = margins.1.max(0);
    let x = (right - width - mx).max(left);
    let y = (bottom - height - my).max(top);
    (x, y)
}

/// Positions a floating HUD window near the bottom-right of the monitor work area
/// with `(logical_margin_x, logical_margin_y)` breathing room above the taskbar/toast
/// zone without stealing focus or resurrecting hidden windows.
pub fn position_bottom_right_hud(
    window: &Window,
    logical_margin_x: f32,
    logical_margin_y: f32,
) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{HWND, RECT},
            UI::WindowsAndMessaging::{
                GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            },
        };
        if !window.is_visible() {
            return false;
        }
        let Some(handle) = hwnd(window) else {
            return false;
        };
        let Some(work) = work_area(Some(handle)) else {
            return false;
        };
        let hwnd = handle as HWND;
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let (width, height) = if unsafe { GetWindowRect(hwnd, &mut rect) } != 0
            && rect.right > rect.left
            && rect.bottom > rect.top
        {
            (rect.right - rect.left, rect.bottom - rect.top)
        } else {
            let size = window.size();
            (size.width as i32, size.height as i32)
        };
        let scale = window.scale_factor();
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let mx = (logical_margin_x.max(0.0) * scale).round() as i32;
        let my = (logical_margin_y.max(0.0) * scale).round() as i32;
        let (x, y) = bottom_right_hud_position(work, (width, height), (mx, my));
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            ) != 0
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (window, logical_margin_x, logical_margin_y);
        false
    }
}

/// Shows a popup or HUD window without stealing foreground focus: cloaks via DWM
/// before showing when an HWND already exists, applies `WS_EX_NOACTIVATE`,
/// positions/clamps inside the monitor work area, shows via `SW_SHOWNOACTIVATE`,
/// and uncloaks cleanly.
pub fn show_non_activating<C: slint::ComponentHandle + 'static>(
    component: &C,
    center: bool,
    topmost: bool,
) -> Result<(), slint::PlatformError>
where
    for<'a> crate::Tokens<'a>: slint::Global<'a, C>,
{
    #[cfg(windows)]
    if let Some(handle) = hwnd(component.window()) {
        set_window_cloaked(handle, true);
        hls_client::hide_window_from_taskbar(handle);
        set_no_activate(component.window(), true);
    }
    component.show()?;
    #[cfg(windows)]
    {
        if let Some(handle) = hwnd(component.window()) {
            set_window_cloaked(handle, true);
            hls_client::hide_window_from_taskbar(handle);
            set_no_activate(component.window(), true);
        }
        let weak = component.as_weak();
        let _ = slint::spawn_local(async move {
            let Some(component) = weak.upgrade() else {
                return;
            };
            use slint::winit_030::WinitWindowAccessor;
            if component.window().winit_window().await.is_err() {
                return;
            }
            if !component.window().is_visible() {
                if let Some(handle) = hwnd(component.window()) {
                    set_window_cloaked(handle, false);
                }
                return;
            }
            if let Some(handle) = hwnd(component.window()) {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                    SWP_NOZORDER, SW_SHOWNOACTIVATE,
                };
                set_window_cloaked(handle, true);
                let tokens = component.global::<crate::Tokens>();
                apply_caption_theme(
                    component.window(),
                    tokens.get_dark(),
                    tokens.get_surface(),
                    tokens.get_ink(),
                    tokens.get_line(),
                );
                hls_client::hide_window_from_taskbar(handle);
                set_no_activate(component.window(), true);
                fit_to_work_area(component.window());
                if center {
                    if let Some((left, top, right, bottom)) = work_area(Some(handle)) {
                        let size = component.window().size();
                        let w = size.width as i32;
                        let h = size.height as i32;
                        let x = left + ((right - left - w).max(0) / 2);
                        let y = top + ((bottom - top - h).max(0) / 2);
                        unsafe {
                            SetWindowPos(
                                handle as _,
                                if topmost {
                                    HWND_TOPMOST
                                } else {
                                    std::ptr::null_mut()
                                },
                                x,
                                y,
                                0,
                                0,
                                SWP_NOSIZE
                                    | SWP_NOACTIVATE
                                    | if topmost { 0 } else { SWP_NOZORDER },
                            );
                        }
                    } else {
                        hls_client::center_window(handle);
                    }
                } else if topmost {
                    unsafe {
                        SetWindowPos(
                            handle as _,
                            HWND_TOPMOST,
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                }
                unsafe {
                    ShowWindow(handle as _, SW_SHOWNOACTIVATE);
                }
                set_window_cloaked(handle, false);
            }
        });
    }
    #[cfg(not(windows))]
    let _ = (center, topmost);
    Ok(())
}

/// Prepares a frameless notification toast: cloaks during setup, hides it from
/// the taskbar, marks it non-activating, clamps to the work area, and anchors it
/// in the bottom-right corner above the taskbar without ever stealing focus.
pub fn prepare_bottom_right_toast<C: slint::ComponentHandle + 'static>(
    component: &C,
    logical_margin: f32,
) where
    for<'a> crate::Tokens<'a>: slint::Global<'a, C>,
{
    #[cfg(windows)]
    {
        if let Some(handle) = hwnd(component.window()) {
            set_window_cloaked(handle, true);
            hls_client::hide_window_from_taskbar(handle);
            mark_no_activate(component.window());
        }
        let weak = component.as_weak();
        let _ = slint::spawn_local(async move {
            let Some(component) = weak.upgrade() else {
                return;
            };
            use slint::winit_030::WinitWindowAccessor;
            if component.window().winit_window().await.is_err() {
                return;
            }
            let handle = hwnd(component.window());
            if !component.window().is_visible() {
                if let Some(handle) = handle {
                    set_window_cloaked(handle, false);
                }
                return;
            }
            if let Some(handle) = handle {
                set_window_cloaked(handle, true);
            }
            let tokens = component.global::<crate::Tokens>();
            apply_caption_theme(
                component.window(),
                tokens.get_dark(),
                tokens.get_surface(),
                tokens.get_ink(),
                tokens.get_line(),
            );
            if let Some(handle) = handle {
                hls_client::hide_window_from_taskbar(handle);
            }
            mark_no_activate(component.window());
            fit_to_work_area(component.window());
            position_bottom_right_toast(component.window(), logical_margin);
            if let Some(handle) = handle {
                set_window_cloaked(handle, false);
            }
        });
    }
    #[cfg(not(windows))]
    let _ = (component, logical_margin);
}

pub fn prepare_dialog<C: slint::ComponentHandle + 'static>(
    component: &C,
    center: bool,
    activate: bool,
    owner: Option<i64>,
) where
    for<'a> crate::Tokens<'a>: slint::Global<'a, C>,
{
    #[cfg(windows)]
    {
        let weak = component.as_weak();
        let _ = slint::spawn_local(async move {
            let Some(component) = weak.upgrade() else {
                return;
            };
            use slint::winit_030::WinitWindowAccessor;
            if component.window().winit_window().await.is_err() {
                return;
            }
            if !component.window().is_visible() {
                if let Some(handle) = hwnd(component.window()) {
                    set_window_cloaked(handle, false);
                }
                return;
            }
            // The HWND is created on show; initial form styling alone cannot theme its caption.
            let tokens = component.global::<crate::Tokens>();
            apply_caption_theme(
                component.window(),
                tokens.get_dark(),
                if owner.is_some() {
                    tokens.get_canvas()
                } else {
                    tokens.get_surface()
                },
                tokens.get_ink(),
                tokens.get_line(),
            );
            if let Some(handle) = hwnd(component.window()) {
                // Cloak during owner/style/position adjustments to prevent any visible show→hide→show flash.
                set_window_cloaked(handle, true);
                if let Some(owner) = owner {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        GetWindow, GetWindowLongPtrW, IsWindowVisible, SetWindowLongPtrW,
                        SetWindowPos, ShowWindow, GWLP_HWNDPARENT, GWL_EXSTYLE, GW_OWNER,
                        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
                        SW_HIDE, SW_SHOWNOACTIVATE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
                    };
                    unsafe {
                        let handle = handle as windows_sys::Win32::Foundation::HWND;
                        let style = GetWindowLongPtrW(handle, GWL_EXSTYLE);
                        let next = style & !((WS_EX_APPWINDOW | WS_EX_TOOLWINDOW) as isize);
                        if GetWindow(handle, GW_OWNER) != owner as _ || style != next {
                            let visible = IsWindowVisible(handle) != 0;
                            if visible {
                                ShowWindow(handle, SW_HIDE);
                            }
                            // Owned dialogs already share the main taskbar entry. TOOLWINDOW
                            // would also shrink their caption and close button.
                            SetWindowLongPtrW(handle, GWLP_HWNDPARENT, owner as isize);
                            SetWindowLongPtrW(handle, GWL_EXSTYLE, next);
                            SetWindowPos(
                                handle,
                                std::ptr::null_mut(),
                                0,
                                0,
                                0,
                                0,
                                SWP_FRAMECHANGED
                                    | SWP_NOACTIVATE
                                    | SWP_NOMOVE
                                    | SWP_NOSIZE
                                    | SWP_NOZORDER,
                            );
                            if visible {
                                ShowWindow(handle, SW_SHOWNOACTIVATE);
                            }
                        }
                    }
                } else {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        GetWindowLongPtrW, GWL_STYLE, WS_CAPTION,
                    };
                    // Frameless handoff popups are transient. A standalone
                    // player keeps its normal caption and taskbar identity.
                    if unsafe { GetWindowLongPtrW(handle as _, GWL_STYLE) } & WS_CAPTION as isize
                        == 0
                    {
                        hls_client::hide_window_from_taskbar(handle);
                    }
                }
                fit_to_work_area(component.window());
                if center {
                    hls_client::center_window(handle);
                }
                set_window_cloaked(handle, false);
                if activate && component.window().is_visible() {
                    hls_client::activate_window(handle);
                }
            }
        });
    }
    #[cfg(not(windows))]
    let _ = (component, center, activate, owner);
}

pub fn set_window_logical_size(window: &Window, logical_width: f32, logical_height: f32) {
    window.set_size(slint::LogicalSize::new(logical_width, logical_height));
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        use slint::winit_030::{winit::dpi::PhysicalSize, WinitWindowAccessor};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        };
        let scale = window.scale_factor();
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let inner_width = (logical_width * scale).round().max(1.0) as u32;
        let inner_height = (logical_height * scale).round().max(1.0) as u32;
        let border = window
            .with_winit_window(|native| {
                let _ = native.request_inner_size(PhysicalSize::new(inner_width, inner_height));
                let outer = native.outer_size();
                let inner = native.inner_size();
                (
                    outer.width.saturating_sub(inner.width),
                    outer.height.saturating_sub(inner.height),
                )
            })
            .unwrap_or_default();
        unsafe {
            SetWindowPos(
                handle as _,
                std::ptr::null_mut(),
                0,
                0,
                inner_width.saturating_add(border.0) as i32,
                inner_height.saturating_add(border.1) as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

pub fn fit_to_work_area(window: &Window) {
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        use slint::winit_030::WinitWindowAccessor;
        use windows_sys::Win32::{
            Foundation::{HWND, RECT},
            UI::WindowsAndMessaging::{
                GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            },
        };
        let Some(work) = work_area(Some(handle)) else {
            return;
        };
        let border = window
            .with_winit_window(|native| {
                let outer = native.outer_size();
                let inner = native.inner_size();
                (
                    outer.width.saturating_sub(inner.width),
                    outer.height.saturating_sub(inner.height),
                )
            })
            .unwrap_or_default();
        let size = window.size();
        let outer_width = size.width.saturating_add(border.0).max(1);
        let outer_height = size.height.saturating_add(border.1).max(1);
        let hwnd = handle as HWND;
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let origin = if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
            (rect.left, rect.top)
        } else {
            let pos = window.position();
            (pos.x, pos.y)
        };
        let ((clamped_x, clamped_y), (clamped_outer_w, clamped_outer_h)) =
            clamp_window_to_work_area(work, origin, (outer_width, outer_height));
        let width = clamped_outer_w.saturating_sub(border.0).max(1);
        let height = clamped_outer_h.saturating_sub(border.1).max(1);
        if width != size.width || height != size.height {
            window.set_size(slint::PhysicalSize::new(width, height));
        }
        if (clamped_x, clamped_y) != origin {
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    clamped_x,
                    clamped_y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

pub fn ui_directory() -> std::path::PathBuf {
    let database = hls_client::default_v7_database_path();
    let data = database.parent().unwrap_or(std::path::Path::new("."));
    if std::env::var_os("HLS_V7_DATA_DIR").is_some_and(|value| !value.is_empty())
        || data
            .parent()
            .is_some_and(|root| root.join("portable").is_file())
    {
        data.join("ui")
    } else {
        std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| data.to_path_buf())
            .join("HLSDownloader")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SavedWindowGeometry {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl Default for SavedWindowGeometry {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: 1400,
            height: 820,
            maximized: false,
        }
    }
}

pub(crate) fn parse_geometry_properties(text: &str) -> SavedWindowGeometry {
    let mut geometry = SavedWindowGeometry::default();
    for line in text.lines() {
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let raw = raw.trim();
        match key.trim() {
            "x" => geometry.x = raw.parse::<i32>().ok(),
            "y" => geometry.y = raw.parse::<i32>().ok(),
            "width" => {
                if let Some(w) = raw.parse::<u32>().ok().filter(|w| *w >= 100) {
                    geometry.width = w;
                }
            }
            "height" => {
                if let Some(h) = raw.parse::<u32>().ok().filter(|h| *h >= 100) {
                    geometry.height = h;
                }
            }
            "maximized" => {
                geometry.maximized = matches!(raw, "1" | "true");
            }
            _ => {}
        }
    }
    geometry
}

pub(crate) fn format_geometry_properties(geometry: SavedWindowGeometry) -> String {
    let mut out = String::new();
    if let (Some(x), Some(y)) = (geometry.x, geometry.y) {
        out.push_str(&format!("x={x}\ny={y}\n"));
    }
    out.push_str(&format!(
        "width={}\nheight={}\nmaximized={}\n",
        geometry.width,
        geometry.height,
        u8::from(geometry.maximized)
    ));
    out
}

pub fn restore_geometry(window: &Window) {
    let text = std::fs::read_to_string(ui_directory().join("window-geometry.properties"))
        .unwrap_or_default();
    let saved = parse_geometry_properties(&text);
    let mut width = saved.width;
    let mut height = saved.height;
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        if let Some((left, top, right, bottom)) = work_area(Some(handle)) {
            let scale = window.scale_factor();
            let scale = if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            };
            use slint::winit_030::WinitWindowAccessor;
            let border = window
                .with_winit_window(|native| {
                    let outer = native.outer_size();
                    let inner = native.inner_size();
                    (
                        outer.width.saturating_sub(inner.width),
                        outer.height.saturating_sub(inner.height),
                    )
                })
                .unwrap_or_default();
            let area_width = (((right - left) as u32).saturating_sub(border.0) as f32 / scale)
                .floor()
                .max(1.0) as u32;
            let area_height = (((bottom - top) as u32).saturating_sub(border.1) as f32 / scale)
                .floor()
                .max(1.0) as u32;
            width = width.clamp(560.min(area_width), area_width);
            height = height.clamp(360.min(area_height), area_height);
            let outer_width = ((width as f32 * scale).round() as u32).saturating_add(border.0);
            let outer_height = ((height as f32 * scale).round() as u32).saturating_add(border.1);
            let target_origin = match (saved.x, saved.y) {
                (Some(x), Some(y)) if x > -16_000 && y > -16_000 => {
                    clamp_window_to_work_area(
                        (left, top, right, bottom),
                        (x, y),
                        (outer_width, outer_height),
                    )
                    .0
                }
                _ => (
                    left + ((area_width - width) as f32 * scale / 2.0) as i32,
                    top + ((area_height - height) as f32 * scale / 2.0) as i32,
                ),
            };
            set_window_logical_size(window, width as f32, height as f32);
            window.set_position(slint::PhysicalPosition::new(
                target_origin.0,
                target_origin.1,
            ));
            if saved.maximized {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                        handle as _,
                        windows_sys::Win32::UI::WindowsAndMessaging::SW_MAXIMIZE,
                    );
                }
            }
            return;
        }
    }
    window.set_size(slint::LogicalSize::new(width as f32, height as f32));
}

pub fn save_geometry(window: &Window) {
    let directory = ui_directory();
    let path = directory.join("window-geometry.properties");
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        use windows_sys::Win32::{
            Foundation::RECT,
            UI::WindowsAndMessaging::{
                GetWindowPlacement, GetWindowRect, IsIconic, IsZoomed, WINDOWPLACEMENT,
            },
        };
        unsafe {
            if IsIconic(handle as _) != 0 {
                return;
            }
            let maximized = IsZoomed(handle as _) != 0;
            if maximized {
                let mut previous =
                    parse_geometry_properties(&std::fs::read_to_string(&path).unwrap_or_default());
                if previous.x.is_none() || previous.y.is_none() {
                    let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
                    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                    if GetWindowPlacement(handle as _, &mut placement) != 0 {
                        let normal = placement.rcNormalPosition;
                        if normal.right > normal.left && normal.bottom > normal.top {
                            let scale = window.scale_factor();
                            let scale = if scale.is_finite() && scale > 0.0 {
                                scale
                            } else {
                                1.0
                            };
                            previous.x = Some(normal.left);
                            previous.y = Some(normal.top);
                            let nw = ((normal.right - normal.left) as f32 / scale).round() as u32;
                            let nh = ((normal.bottom - normal.top) as f32 / scale).round() as u32;
                            if nw >= 300 && nh >= 200 {
                                previous.width = nw;
                                previous.height = nh;
                            }
                        }
                    }
                }
                previous.maximized = true;
                if std::fs::create_dir_all(&directory).is_ok() {
                    let _ = std::fs::write(&path, format_geometry_properties(previous));
                }
                return;
            }
            let size = window.size().to_logical(window.scale_factor());
            if !size.width.is_finite()
                || !size.height.is_finite()
                || size.width < 100.0
                || size.height < 100.0
            {
                return;
            }
            let mut rect: RECT = std::mem::zeroed();
            let (x, y) = if GetWindowRect(handle as _, &mut rect) != 0
                && !is_parked_window_rect((rect.left, rect.top, rect.right, rect.bottom))
            {
                (Some(rect.left), Some(rect.top))
            } else {
                (None, None)
            };
            let geometry = SavedWindowGeometry {
                x,
                y,
                width: size.width.round() as u32,
                height: size.height.round() as u32,
                maximized: false,
            };
            if std::fs::create_dir_all(&directory).is_ok() {
                let _ = std::fs::write(&path, format_geometry_properties(geometry));
            }
            return;
        }
    }
    let size = window.size().to_logical(window.scale_factor());
    if !size.width.is_finite()
        || !size.height.is_finite()
        || size.width < 100.0
        || size.height < 100.0
    {
        return;
    }
    if std::fs::create_dir_all(&directory).is_ok() {
        let _ = std::fs::write(
            &path,
            format_geometry_properties(SavedWindowGeometry {
                x: None,
                y: None,
                width: size.width.round() as u32,
                height: size.height.round() as u32,
                maximized: false,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bottom_right_hud_position, bottom_right_toast_position, clamp_window_to_work_area,
        dwm_color_ref, format_geometry_properties, is_parked_window_rect,
        parse_geometry_properties, quns_is_fullscreen_or_busy, rect_covers_monitor,
        SavedWindowGeometry,
    };

    #[test]
    fn dwm_color_ref_packs_rgb_in_win32_bgr_order() {
        assert_eq!(
            dwm_color_ref(slint::Color::from_rgb_u8(0x12, 0x34, 0x56)),
            0x0056_3412
        );
        assert_eq!(
            dwm_color_ref(slint::Color::from_rgb_u8(0xff, 0x00, 0x80)),
            0x0080_00ff
        );
    }

    #[test]
    fn fullscreen_and_parked_rect_predicates_classify_window_states() {
        assert!(!quns_is_fullscreen_or_busy(1)); // QUNS_NOT_PRESENT
        assert!(quns_is_fullscreen_or_busy(2)); // QUNS_BUSY
        assert!(quns_is_fullscreen_or_busy(3)); // QUNS_RUNNING_D3D_FULL_SCREEN
        assert!(quns_is_fullscreen_or_busy(4)); // QUNS_PRESENTATION_MODE
        assert!(!quns_is_fullscreen_or_busy(5)); // QUNS_ACCEPTS_NOTIFICATIONS

        assert!(rect_covers_monitor((0, 0, 1920, 1080), (0, 0, 1920, 1080)));
        assert!(rect_covers_monitor(
            (-1920, -8, 0, 1088),
            (-1920, 0, 0, 1080)
        ));
        assert!(!rect_covers_monitor((0, 0, 1920, 1040), (0, 0, 1920, 1080)));

        assert!(is_parked_window_rect((-32_000, -32_000, -31_480, -31_600)));
        assert!(is_parked_window_rect((0, 0, 520, 400)));
        assert!(is_parked_window_rect((100, 100, 100, 100)));
        assert!(!is_parked_window_rect((240, 160, 760, 560)));
        assert!(!is_parked_window_rect((-1600, 120, -1080, 520)));
    }

    #[test]
    fn clamp_window_to_work_area_shifts_expanded_hud_above_taskbar_and_keeps_prewarm_offscreen() {
        let work = (0, 0, 1920, 1040);
        // ProgressWindow near bottom-right expanding from 256px to 468px shifts up cleanly.
        assert_eq!(
            clamp_window_to_work_area(work, (1500, 760), (380, 468)),
            ((1500, 572), (380, 468))
        );
        // Window partially off left/top on secondary display clamps into work area.
        assert_eq!(
            clamp_window_to_work_area((-1920, 0, 0, 1040), (-2050, -40), (520, 400)),
            ((-1920, 0), (520, 400))
        );
        // Oversized window shrinks to fit work area and pins to top-left of work area.
        assert_eq!(
            clamp_window_to_work_area((100, 50, 500, 350), (200, 200), (800, 600)),
            ((100, 50), (400, 300))
        );
        // Off-screen prewarm at (-32000, -32000) must never be pulled onto the visible screen.
        assert_eq!(
            clamp_window_to_work_area(work, (-32_000, -32_000), (520, 400)),
            ((-32_000, -32_000), (520, 400))
        );
    }

    #[test]
    fn bottom_right_toast_anchors_above_taskbar_with_margin_and_clamps_small_screens() {
        // 1920x1080 with 40px taskbar (work area height 1040), 330x156 toast, 16px margin.
        assert_eq!(
            bottom_right_toast_position((0, 0, 1920, 1040), (330, 156), 16),
            (1574, 868)
        );
        // Secondary monitor with negative left coordinate and 150% DPI (24px physical margin).
        assert_eq!(
            bottom_right_toast_position((-1920, 0, 0, 1040), (495, 234), 24),
            (-519, 782)
        );
        // Very small work area clamps to top-left without pushing off-screen.
        assert_eq!(
            bottom_right_toast_position((100, 50, 300, 180), (330, 200), 16),
            (100, 50)
        );
        // Negative margin is treated as 0.
        assert_eq!(
            bottom_right_toast_position((0, 0, 1000, 600), (300, 150), -10),
            (700, 450)
        );
        assert_eq!(
            bottom_right_hud_position((0, 0, 1920, 1040), (380, 256), (16, 36)),
            (1524, 748)
        );
    }

    #[test]
    fn geometry_properties_roundtrip_normal_origin_size_and_maximized_flag() {
        let normal = SavedWindowGeometry {
            x: Some(180),
            y: Some(92),
            width: 1360,
            height: 800,
            maximized: false,
        };
        let encoded = format_geometry_properties(normal);
        assert_eq!(parse_geometry_properties(&encoded), normal);

        let mut maximized = parse_geometry_properties(&encoded);
        maximized.maximized = true;
        let max_encoded = format_geometry_properties(maximized);
        let restored = parse_geometry_properties(&max_encoded);
        assert!(restored.maximized);
        assert_eq!(restored.width, 1360);
        assert_eq!(restored.height, 800);
        assert_eq!(restored.x, Some(180));
        assert_eq!(restored.y, Some(92));

        // Legacy properties with only width/height fall back cleanly.
        assert_eq!(
            parse_geometry_properties("width=1280\nheight=720\n"),
            SavedWindowGeometry {
                x: None,
                y: None,
                width: 1280,
                height: 720,
                maximized: false,
            }
        );
    }
}
