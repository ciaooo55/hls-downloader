//! Win32 tray icon for the resident v6 process. This is not a second UI toolkit.

use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowMain,
    NewTask,
    Settings,
    Quit,
}

#[cfg(windows)]
pub fn spawn_tray(tx: Sender<TrayAction>) {
    std::thread::Builder::new()
        .name("v6-tray".into())
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

static TRAY_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn show_notification(title: &str, body: &str) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_INFO, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_MODIFY,
            NOTIFYICONDATAW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{LoadIconW, IDI_APPLICATION};
        let hwnd = TRAY_HWND.load(std::sync::atomic::Ordering::SeqCst);
        if hwnd == 0 {
            return;
        }
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd as _;
        data.uID = 1;
        data.uFlags = NIF_INFO | NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.dwInfoFlags = NIIF_INFO;
        data.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        copy_utf16(&mut data.szInfoTitle, title);
        copy_utf16(&mut data.szInfo, body);
        copy_utf16(&mut data.szTip, "HLS Downloader");
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
    }
}

#[cfg(windows)]
fn copy_utf16(dest: &mut [u16], text: &str) {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    for (index, unit) in units.iter().take(dest.len()).enumerate() {
        dest[index] = *unit;
    }
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBeep(utype: u32) -> i32;
}

#[cfg(windows)]
#[allow(unused_imports, unused_variables)]
unsafe fn tray_loop(tx: Sender<TrayAction>) {
    use std::mem::size_of;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
        NIM_MODIFY, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyWindow,
        DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostQuitMessage, RegisterClassW,
        SetForegroundWindow, TrackPopupMenu, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
        CW_USEDEFAULT, HWND_MESSAGE, IDI_APPLICATION, MF_STRING, TPM_LEFTALIGN, TPM_RIGHTBUTTON,
        WM_APP, WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
    };

    const WM_TRAY: u32 = WM_APP + 32;
    const ID_SHOW: usize = 1;
    const ID_NEW: usize = 2;
    const ID_SETTINGS: usize = 3;
    const ID_QUIT: usize = 4;

    let class_name: Vec<u16> = "HLSDownloader.v6.Tray\0".encode_utf16().collect();
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
    let mut hwnd = CreateWindowExW(
        0,
        class_name.as_ptr(),
        class_name.as_ptr(),
        WS_OVERLAPPED,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        HWND_MESSAGE,
        null_mut(),
        null_mut(),
        null_mut(),
    );
    if hwnd.is_null() {
        hwnd = CreateWindowExW(
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
    }
    if hwnd.is_null() {
        return;
    }
    TRAY_HWND.store(hwnd as isize, std::sync::atomic::Ordering::SeqCst);
    let boxed = Box::new(tx);
    windows_sys::Win32::Foundation::SetLastError(0);
    windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
        hwnd,
        windows_sys::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
        Box::into_raw(boxed) as isize,
    );

    let mut data: NOTIFYICONDATAW = std::mem::zeroed();
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
    let tip: Vec<u16> = "HLS Downloader\0".encode_utf16().collect();
    for (index, unit) in tip.iter().take(data.szTip.len()).enumerate() {
        data.szTip[index] = *unit;
    }
    Shell_NotifyIconW(NIM_ADD, &data);

    let mut msg = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    Shell_NotifyIconW(NIM_DELETE, &data);
    let _ = (WM_COMMAND, WM_LBUTTONUP, WM_RBUTTONUP, ID_SHOW, ID_NEW, ID_SETTINGS, ID_QUIT);
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
    #[allow(unused_imports)]
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DefWindowProcW, GetCursorPos, GetWindowLongPtrW,
        PostQuitMessage, SetForegroundWindow, TrackPopupMenu, GWLP_USERDATA, MF_STRING,
        TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP,
    };

    const WM_TRAY: u32 = WM_APP + 32;
    const ID_SHOW: usize = 1;
    const ID_NEW: usize = 2;
    const ID_SETTINGS: usize = 3;
    const ID_QUIT: usize = 4;

    let sender = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Sender<TrayAction>;
    if msg == WM_TRAY {
        let event = lparam as u32;
        if event == WM_LBUTTONUP {
            send_action(sender, TrayAction::ShowMain);
            return 0;
        }
        if event == WM_RBUTTONUP {
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            let menu = CreatePopupMenu();
            let show: Vec<u16> = "打开工作台\0".encode_utf16().collect();
            let new_task: Vec<u16> = "新建任务\0".encode_utf16().collect();
            let settings: Vec<u16> = "设置\0".encode_utf16().collect();
            let quit: Vec<u16> = "退出\0".encode_utf16().collect();
            AppendMenuW(menu, MF_STRING, ID_SHOW, show.as_ptr());
            AppendMenuW(menu, MF_STRING, ID_NEW, new_task.as_ptr());
            AppendMenuW(menu, MF_STRING, ID_SETTINGS, settings.as_ptr());
            AppendMenuW(menu, MF_STRING, ID_QUIT, quit.as_ptr());
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
            return 0;
        }
    }
    if msg == WM_COMMAND {
        match wparam & 0xffff {
            ID_SHOW => send_action(sender, TrayAction::ShowMain),
            ID_NEW => send_action(sender, TrayAction::NewTask),
            ID_SETTINGS => send_action(sender, TrayAction::Settings),
            ID_QUIT => {
                send_action(sender, TrayAction::Quit);
                PostQuitMessage(0);
            }
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
fn send_action(sender: *const Sender<TrayAction>, action: TrayAction) {
    if sender.is_null() {
        return;
    }
    let _ = unsafe { &*sender }.send(action);
}
