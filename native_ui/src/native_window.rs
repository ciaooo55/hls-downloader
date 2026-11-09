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
    );
}

fn apply_caption_theme(window: &Window, dark: bool, background: slint::Color, text: slint::Color) {
    #[cfg(windows)]
    {
        use slint::winit_030::{winit::window::Theme, WinitWindowAccessor};
        window.with_winit_window(|native| {
            native.set_theme(Some(if dark { Theme::Dark } else { Theme::Light }));
        });
        if let Some(handle) = hwnd(window) {
            use windows_sys::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
            };
            let color_ref = |color: slint::Color| {
                u32::from(color.red())
                    | (u32::from(color.green()) << 8)
                    | (u32::from(color.blue()) << 16)
            };
            // Windows 11 supports explicit caption colors even when the application
            // and system themes differ. Older Windows keeps Winit's native theme.
            for (attribute, value) in [
                (DWMWA_USE_IMMERSIVE_DARK_MODE, u32::from(dark)),
                (DWMWA_CAPTION_COLOR, color_ref(background)),
                (DWMWA_TEXT_COLOR, color_ref(text)),
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
    let _ = (window, dark, background, text);
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
            // The HWND is created on show; initial form styling alone cannot theme its caption.
            let tokens = component.global::<crate::Tokens>();
            apply_caption_theme(
                component.window(),
                tokens.get_dark(),
                tokens.get_surface(),
                tokens.get_ink(),
            );
            if let Some(handle) = hwnd(component.window()) {
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
                if activate && component.window().is_visible() {
                    hls_client::activate_window(handle);
                }
            }
        });
    }
    #[cfg(not(windows))]
    let _ = (component, center, activate, owner);
}

pub fn fit_to_work_area(window: &Window) {
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        use slint::winit_030::WinitWindowAccessor;
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe {
            GetMonitorInfoW(
                MonitorFromWindow(handle as _, MONITOR_DEFAULTTONEAREST),
                &mut info,
            )
        } == 0
        {
            return;
        }
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
        let width = size
            .width
            .min(((info.rcWork.right - info.rcWork.left) as u32).saturating_sub(border.0))
            .max(1);
        let height = size
            .height
            .min(((info.rcWork.bottom - info.rcWork.top) as u32).saturating_sub(border.1))
            .max(1);
        window.set_size(slint::PhysicalSize::new(width, height));
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

pub fn restore_geometry(window: &Window) {
    let text = std::fs::read_to_string(ui_directory().join("window-geometry.properties"))
        .unwrap_or_default();
    let value = |name: &str, fallback| {
        text.lines()
            .find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key == name)
                    .then(|| value.trim().parse::<u32>().ok())
                    .flatten()
            })
            .unwrap_or(fallback)
    };
    let mut width = value("width", 1400);
    let mut height = value("height", 820);
    #[cfg(windows)]
    if let Some(handle) = hwnd(window) {
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let monitor = unsafe { MonitorFromWindow(handle as _, MONITOR_DEFAULTTONEAREST) };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let scale = window.scale_factor();
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
            let area_width = (((info.rcWork.right - info.rcWork.left) as u32)
                .saturating_sub(border.0) as f32
                / scale)
                .floor()
                .max(1.0) as u32;
            let area_height = (((info.rcWork.bottom - info.rcWork.top) as u32)
                .saturating_sub(border.1) as f32
                / scale)
                .floor()
                .max(1.0) as u32;
            width = width.clamp(560.min(area_width), area_width);
            height = height.clamp(360.min(area_height), area_height);
            window.set_position(slint::PhysicalPosition::new(
                info.rcWork.left + ((area_width - width) as f32 * scale / 2.0) as i32,
                info.rcWork.top + ((area_height - height) as f32 * scale / 2.0) as i32,
            ));
        }
    }
    window.set_size(slint::LogicalSize::new(width as f32, height as f32));
}

pub fn save_geometry(window: &Window) {
    let size = window.size().to_logical(window.scale_factor());
    let directory = ui_directory();
    if std::fs::create_dir_all(&directory).is_ok() {
        let _ = std::fs::write(
            directory.join("window-geometry.properties"),
            format!(
                "width={}\nheight={}\n",
                size.width.round() as u32,
                size.height.round() as u32
            ),
        );
    }
}
