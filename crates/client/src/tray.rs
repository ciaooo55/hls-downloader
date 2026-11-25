//! Win32 tray icon for the resident v7 Core. This is not a second UI toolkit.

use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

const TRAY_THREAD_NAME: &str = "v7-tray";
const DEFAULT_TRAY_TOOLTIP: &str = "HLS Downloader";

const FLAG_SPEED_LIMITED: u8 = 1 << 0;
const FLAG_TAKEOVER_ENABLED: u8 = 1 << 1;
const FLAG_CLIPBOARD_WATCH: u8 = 1 << 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowMain,
    NewDownload,
    PauseAll,
    ResumeAll,
    ToggleSpeedLimit,
    ToggleTakeover,
    ToggleClipboardWatch,
    Quit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrayMenuState {
    pub speed_limited: bool,
    pub takeover_enabled: bool,
    pub clipboard_watch: bool,
}

impl TrayMenuState {
    fn to_bits(self) -> u8 {
        let mut bits = 0u8;
        if self.speed_limited {
            bits |= FLAG_SPEED_LIMITED;
        }
        if self.takeover_enabled {
            bits |= FLAG_TAKEOVER_ENABLED;
        }
        if self.clipboard_watch {
            bits |= FLAG_CLIPBOARD_WATCH;
        }
        bits
    }

    fn from_bits(bits: u8) -> Self {
        Self {
            speed_limited: (bits & FLAG_SPEED_LIMITED) != 0,
            takeover_enabled: (bits & FLAG_TAKEOVER_ENABLED) != 0,
            clipboard_watch: (bits & FLAG_CLIPBOARD_WATCH) != 0,
        }
    }
}

#[cfg(windows)]
pub fn spawn_tray(tx: Sender<TrayAction>) {
    std::thread::Builder::new()
        .name(TRAY_THREAD_NAME.into())
        .spawn(move || unsafe { tray_loop(tx) })
        .ok();
}

#[cfg(not(windows))]
pub fn spawn_tray(_tx: Sender<TrayAction>) {}

pub fn completion_sound() {
    #[cfg(windows)]
    unsafe {
        MessageBeep(0x0000_0000);
    }
}

static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static TRAY_MENU_STATE: AtomicU8 = AtomicU8::new(FLAG_TAKEOVER_ENABLED);
static TRAY_TOOLTIP: OnceLock<Mutex<String>> = OnceLock::new();

fn tray_tooltip_store() -> &'static Mutex<String> {
    TRAY_TOOLTIP.get_or_init(|| Mutex::new(DEFAULT_TRAY_TOOLTIP.to_string()))
}

fn current_menu_state() -> TrayMenuState {
    TrayMenuState::from_bits(TRAY_MENU_STATE.load(Ordering::Acquire))
}

fn current_tooltip() -> String {
    let tip = tray_tooltip_store()
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    if tip.trim().is_empty() {
        DEFAULT_TRAY_TOOLTIP.to_string()
    } else {
        tip
    }
}

pub fn update_tray_state(tooltip: &str, state: TrayMenuState) {
    TRAY_MENU_STATE.store(state.to_bits(), Ordering::Release);
    let next_tip = if tooltip.trim().is_empty() {
        DEFAULT_TRAY_TOOLTIP
    } else {
        tooltip
    };
    if let Ok(mut guard) = tray_tooltip_store().lock() {
        guard.clear();
        guard.push_str(next_tip);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIM_MODIFY};
        let hwnd = TRAY_HWND.load(Ordering::SeqCst);
        if hwnd == 0 {
            return;
        }
        let data = build_tooltip_data(hwnd, next_tip);
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}
#[cfg(windows)]
struct TrayIcon {
    handle: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
    owned: bool,
}

#[cfg(windows)]
impl Drop for TrayIcon {
    fn drop(&mut self) {
        if self.owned {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(self.handle) };
        }
    }
}

#[cfg(windows)]
struct TrayWindow {
    sender: Sender<TrayAction>,
    icon: TrayIcon,
}
#[cfg(windows)]
static TASKBAR_CREATED_MESSAGE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(windows)]
unsafe fn load_product_icon() -> TrayIcon {
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        LoadIconW, LoadImageW, HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE,
    };

    let mut candidates = Vec::<PathBuf>::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("app-icon.ico"));
            candidates.push(parent.join("resources").join("app-icon.ico"));
        }
    }
    if let Ok(directory) = std::env::current_dir() {
        candidates.push(directory.join("app-icon.ico"));
        candidates.push(directory.join("assets").join("app-icon.ico"));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/app-icon.ico"));

    let icon = candidates.into_iter().find_map(|path| {
        if !path.is_file() {
            return None;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            LoadImageW(
                std::ptr::null_mut(),
                wide.as_ptr(),
                IMAGE_ICON,
                0,
                0,
                LR_LOADFROMFILE | LR_DEFAULTSIZE,
            )
        } as HICON;
        (!handle.is_null()).then_some(handle)
    });
    TrayIcon {
        owned: icon.is_some(),
        handle: icon.unwrap_or_else(|| unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) }),
    }
}

#[cfg(windows)]
const WM_TRAY_MINIMIZED_HINT: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 33;

pub fn show_notification(title: &str, body: &str) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIM_MODIFY};
        let hwnd = TRAY_HWND.load(std::sync::atomic::Ordering::SeqCst);
        if hwnd == 0 {
            return;
        }
        let data = build_notification_data(hwnd, title, body);
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
    }
}

pub fn notify_minimized_to_tray() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
        let class_name: Vec<u16> = "HLSDownloader.v7.Tray\0".encode_utf16().collect();
        let hwnd = FindWindowW(class_name.as_ptr(), std::ptr::null());
        if !hwnd.is_null() {
            let _ = PostMessageW(hwnd, WM_TRAY_MINIMIZED_HINT, 0, 0);
        }
    }
}

#[cfg(windows)]
fn build_notification_data(
    hwnd: isize,
    title: &str,
    body: &str,
) -> windows_sys::Win32::UI::Shell::NOTIFYICONDATAW {
    use windows_sys::Win32::UI::Shell::{NIF_INFO, NIIF_INFO, NOTIFYICONDATAW};
    // Only modify the balloon info fields; setting NIF_MESSAGE/NIF_ICON/NIF_TIP
    // with zeroed fields would overwrite uCallbackMessage (killing tray clicks).
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd as _;
    data.uID = 1;
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    copy_utf16(&mut data.szInfoTitle, title);
    copy_utf16(&mut data.szInfo, body);
    data
}

#[cfg(windows)]
fn build_tooltip_data(
    hwnd: isize,
    tooltip: &str,
) -> windows_sys::Win32::UI::Shell::NOTIFYICONDATAW {
    use windows_sys::Win32::UI::Shell::{NIF_TIP, NOTIFYICONDATAW};
    // Only modify NIF_TIP; never set NIF_MESSAGE or NIF_ICON here so callback
    // routing and the loaded product icon are never clobbered.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd as _;
    data.uID = 1;
    data.uFlags = NIF_TIP;
    copy_utf16(&mut data.szTip, tooltip);
    data
}

#[cfg(windows)]
fn copy_utf16(dest: &mut [u16], text: &str) {
    if dest.is_empty() {
        return;
    }
    let max_units = dest.len() - 1;
    let mut written = 0usize;
    for ch in text.chars() {
        let len = ch.len_utf16();
        if written + len > max_units {
            break;
        }
        let mut buf = [0u16; 2];
        let encoded = ch.encode_utf16(&mut buf);
        dest[written..written + len].copy_from_slice(encoded);
        written += len;
    }
    dest[written] = 0;
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBeep(utype: u32) -> i32;
}

#[cfg(windows)]
unsafe fn tray_loop(tx: Sender<TrayAction>) {
    use std::mem::size_of;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, RegisterClassW,
        RegisterWindowMessageW, SetWindowLongPtrW, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
        CW_USEDEFAULT, GWLP_USERDATA, WM_APP, WNDCLASSW, WS_OVERLAPPED,
    };

    const WM_TRAY: u32 = WM_APP + 32;
    let class_name: Vec<u16> = "HLSDownloader.v7.Tray\0".encode_utf16().collect();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(tray_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: null_mut(),
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    RegisterClassW(&class);
    // Explorer broadcasts TaskbarCreated only to top-level windows.
    let hwnd = CreateWindowExW(
        0,
        class_name.as_ptr(),
        class_name.as_ptr(),
        WS_OVERLAPPED,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        0,
        0,
        null_mut(),
        null_mut(),
        null_mut(),
        null_mut(),
    );
    if hwnd.is_null() {
        return;
    }
    let window = TrayWindow {
        sender: tx,
        icon: load_product_icon(),
    };
    windows_sys::Win32::Foundation::SetLastError(0);
    if SetWindowLongPtrW(hwnd, GWLP_USERDATA, &window as *const TrayWindow as isize) == 0
        && windows_sys::Win32::Foundation::GetLastError() != 0
    {
        DestroyWindow(hwnd);
        return;
    }
    let taskbar_created: Vec<u16> = "TaskbarCreated\0".encode_utf16().collect();
    TASKBAR_CREATED_MESSAGE.store(
        RegisterWindowMessageW(taskbar_created.as_ptr()),
        std::sync::atomic::Ordering::Release,
    );
    TRAY_HWND.store(hwnd as isize, std::sync::atomic::Ordering::SeqCst);

    let mut data: NOTIFYICONDATAW = std::mem::zeroed();
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = window.icon.handle;
    let tip = current_tooltip();
    copy_utf16(&mut data.szTip, &tip);
    Shell_NotifyIconW(NIM_ADD, &data);

    let mut msg = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    Shell_NotifyIconW(NIM_DELETE, &data);
    TRAY_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    DestroyWindow(hwnd);
}

#[cfg(windows)]
unsafe extern "system" fn tray_wnd_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DefWindowProcW, DestroyMenu, GetCursorPos, GetWindowLongPtrW,
        PostMessageW, PostQuitMessage, SetForegroundWindow, SetMenuDefaultItem, TrackPopupMenu,
        GWLP_USERDATA, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_LEFTALIGN,
        TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    };

    const WM_TRAY: u32 = WM_APP + 32;
    const ID_SHOW: usize = 1;
    const ID_NEW: usize = 2;
    const ID_PAUSE_ALL: usize = 3;
    const ID_QUIT: usize = 4;
    const ID_RESUME_ALL: usize = 5;
    const ID_SPEED_LIMIT: usize = 6;
    const ID_TAKEOVER: usize = 7;
    const ID_CLIPBOARD: usize = 8;

    let window = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TrayWindow).as_ref();
    let taskbar_created = TASKBAR_CREATED_MESSAGE.load(std::sync::atomic::Ordering::Acquire);
    if taskbar_created != 0 && msg == taskbar_created {
        let Some(window) = window else { return 0 };
        use windows_sys::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NOTIFYICONDATAW,
        };
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = window.icon.handle;
        let tip = current_tooltip();
        copy_utf16(&mut data.szTip, &tip);
        Shell_NotifyIconW(NIM_ADD, &data);
        return 0;
    }
    if msg == WM_TRAY_MINIMIZED_HINT {
        show_notification(
            "HLS Downloader 仍在后台运行",
            "主窗口已收起到系统托盘，正在进行的下载与浏览器接管不会中断。",
        );
        return 0;
    }
    if msg == WM_TRAY {
        let event = lparam as u32;
        if event == WM_LBUTTONUP {
            send_action(window, TrayAction::ShowMain);
            return 0;
        }
        if event == WM_RBUTTONUP {
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            let menu = CreatePopupMenu();
            let state = current_menu_state();
            let append_item = |id: usize, label: &str, checked: Option<bool>| {
                let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
                let flags = match checked {
                    Some(true) => MF_STRING | MF_CHECKED,
                    Some(false) => MF_STRING | MF_UNCHECKED,
                    None => MF_STRING,
                };
                AppendMenuW(menu, flags, id, wide.as_ptr());
            };
            append_item(ID_SHOW, "打开工作台", None);
            SetMenuDefaultItem(menu, ID_SHOW as u32, 0);
            append_item(ID_NEW, "新建下载", None);
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            append_item(ID_PAUSE_ALL, "全部暂停", None);
            append_item(ID_RESUME_ALL, "全部开始", None);
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            append_item(ID_SPEED_LIMIT, "速度限制", Some(state.speed_limited));
            append_item(ID_TAKEOVER, "浏览器接管", Some(state.takeover_enabled));
            append_item(ID_CLIPBOARD, "监视剪贴板", Some(state.clipboard_watch));
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            append_item(ID_QUIT, "退出下载引擎", None);
            SetForegroundWindow(hwnd);
            TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                hwnd,
                std::ptr::null(),
            );
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            return 0;
        }
    }
    if msg == WM_COMMAND {
        match wparam & 0xffff {
            ID_SHOW => send_action(window, TrayAction::ShowMain),
            ID_NEW => send_action(window, TrayAction::NewDownload),
            ID_PAUSE_ALL => send_action(window, TrayAction::PauseAll),
            ID_RESUME_ALL => send_action(window, TrayAction::ResumeAll),
            ID_SPEED_LIMIT => send_action(window, TrayAction::ToggleSpeedLimit),
            ID_TAKEOVER => send_action(window, TrayAction::ToggleTakeover),
            ID_CLIPBOARD => send_action(window, TrayAction::ToggleClipboardWatch),
            // Keep the tray available if Core cannot save its exit checkpoint.
            ID_QUIT => send_action(window, TrayAction::Quit),
            _ => {}
        }
        return 0;
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(windows)]
fn send_action(window: Option<&TrayWindow>, action: TrayAction) {
    if let Some(window) = window {
        let _ = window.sender.send(action);
    }
}

#[cfg(test)]
mod tests {
    use super::TRAY_THREAD_NAME;

    #[test]
    fn resident_tray_uses_the_v7_identity() {
        assert_eq!(TRAY_THREAD_NAME, "v7-tray");
        let source = include_str!("tray.rs");
        assert!(source.contains("app-icon.ico"));
        assert!(source.contains("LoadImageW"));
        assert!(source.contains("TaskbarCreated"));
        assert!(source.contains("RegisterWindowMessageW"));
        assert!(!source.contains("name(\"v6-tray\""));
    }

    #[cfg(windows)]
    #[test]
    fn product_tray_icon_loads_instead_of_the_windows_default() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{LoadIconW, IDI_APPLICATION};
        let icon = unsafe { super::load_product_icon() };
        let fallback = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
        assert!(!icon.handle.is_null());
        assert!(icon.owned);
        assert_ne!(icon.handle, fallback);
    }

    #[cfg(windows)]
    #[test]
    fn notification_does_not_overwrite_callback_message_and_always_nul_terminates() {
        use windows_sys::Win32::UI::Shell::{NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP};
        let long_title = "标题".repeat(50);
        let long_body = "🚀".repeat(200);
        let data = super::build_notification_data(1234, &long_title, &long_body);
        assert_eq!(data.uFlags, NIF_INFO);
        assert_eq!(data.uFlags & (NIF_MESSAGE | NIF_ICON | NIF_TIP), 0);
        assert_eq!(*data.szInfoTitle.last().unwrap(), 0);
        assert_eq!(*data.szInfo.last().unwrap(), 0);
        let mut small = [0xFFFFu16; 4];
        super::copy_utf16(&mut small, "a🚀b");
        // 'a' (1) + '🚀' (2) = 3 units + NUL fits in 4.
        assert_eq!(small[3], 0);
        let mut tiny = [0xFFFFu16; 3];
        super::copy_utf16(&mut tiny, "a🚀b");
        // '🚀' needs 2 units; must not leave an unpaired high surrogate at index 1.
        assert_eq!(tiny[0], 'a' as u16);
        assert_eq!(tiny[1], 0);
    }

    #[cfg(windows)]
    #[test]
    fn tooltip_update_sets_only_nif_tip_and_nul_terminates() {
        use windows_sys::Win32::UI::Shell::{NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP};
        let long_tip = "HLS Downloader · 下载中 🚀".repeat(20);
        let data = super::build_tooltip_data(5678, &long_tip);
        assert_eq!(data.uFlags, NIF_TIP);
        assert_eq!(data.uFlags & (NIF_MESSAGE | NIF_ICON | NIF_INFO), 0);
        assert_eq!(*data.szTip.last().unwrap(), 0);
    }

    #[test]
    fn tray_menu_state_and_tooltip_update_atomically() {
        let state = super::TrayMenuState {
            speed_limited: true,
            takeover_enabled: false,
            clipboard_watch: true,
        };
        super::update_tray_state("HLS Downloader · 2 下载中 · 5.4 MB/s", state);
        assert_eq!(super::current_menu_state(), state);
        assert_eq!(
            super::current_tooltip(),
            "HLS Downloader · 2 下载中 · 5.4 MB/s"
        );
        let idle = super::TrayMenuState {
            speed_limited: false,
            takeover_enabled: true,
            clipboard_watch: false,
        };
        super::update_tray_state("", idle);
        assert_eq!(super::current_menu_state(), idle);
        assert_eq!(super::current_tooltip(), super::DEFAULT_TRAY_TOOLTIP);
    }
}
