//! Win32 tray + pre-created HWND overlays.
//!
//! Confirm / progress / complete / main are created once at boot (`SW_HIDE`),
//! then shown with the offer snapshot. Closing a window hides it; the process
//! stays in the tray.

#![allow(non_snake_case)]

use crate::core_client::CoreClient;
use crate::surfaces::{ResidentShell, Snapshot};
use serde_json::Value;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use windows_sys::Win32::Foundation::{
    COLORREF, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, DrawTextW, EndPaint, FillRect, GetStockObject, SetBkMode, SetTextColor,
    COLOR_WINDOW, DEFAULT_GUI_FONT, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
    DT_WORDBREAK, HDC, PAINTSTRUCT, TRANSPARENT, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetMessageW, GetSystemMetrics, LoadCursorW, LoadIconW, PostMessageW,
    PostQuitMessage, RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow,
    TrackPopupMenu, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    HWND_TOPMOST, IDC_ARROW, IDI_APPLICATION, MF_STRING, MSG, SM_CXSCREEN, SM_CYSCREEN,
    SW_HIDE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WM_LBUTTONUP, WM_PAINT, WM_RBUTTONUP, WNDCLASSW, WS_CAPTION, WS_CHILD,
    WS_CLIPCHILDREN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_OVERLAPPED,
    WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, RegisterWindowMessageW,
};

pub const WM_SHELL_EVENT: u32 = WM_APP + 1;
const WM_TRAY: u32 = WM_APP + 20;
const ID_ACCEPT: usize = 1001;
const ID_REJECT: usize = 1002;
const ID_COMPLETE_CLOSE: usize = 1005;
const ID_TRAY_OPEN: usize = 2001;
const ID_TRAY_EXIT: usize = 2002;

#[derive(Clone, Copy)]
pub struct Hwnds {
    pub handoff: HWND,
    pub progress: HWND,
    pub complete: HWND,
    pub main: HWND,
    pub tray: HWND,
}

pub struct Win32Host {
    pub hwnds: Hwnds,
    pub shell: Arc<Mutex<ResidentShell>>,
    pub pending: Mutex<Vec<Value>>,
    pub core: Mutex<Option<CoreClient>>,
    nid_added: Mutex<bool>,
}

impl Win32Host {
    pub fn boot(shell: Arc<Mutex<ResidentShell>>) -> Result<&'static Self, String> {
        unsafe {
            claim_single_instance()?;
            let instance = GetModuleHandleW(null());
            if instance.is_null() {
                return Err("GetModuleHandleW failed".into());
            }
            register_class(instance, class_handoff(), Some(handoff_proc))?;
            register_class(instance, class_progress(), Some(overlay_proc))?;
            register_class(instance, class_complete(), Some(overlay_proc))?;
            register_class(instance, class_main(), Some(overlay_proc))?;
            register_class(instance, class_tray(), Some(tray_proc))?;

            let handoff = create_overlay(
                instance,
                class_handoff(),
                "浏览器下载",
                480,
                240,
                false,
                true,
            )?;
            let progress = create_overlay(
                instance,
                class_progress(),
                "下载进度",
                320,
                96,
                true,
                false,
            )?;
            let complete = create_overlay(
                instance,
                class_complete(),
                "下载完成",
                420,
                168,
                false,
                true,
            )?;
            let main = create_overlay(instance, class_main(), "HLS Downloader", 720, 480, false, true)?;
            let tray = CreateWindowExW(
                0,
                class_tray().as_ptr(),
                wide("HLS Downloader Tray").as_ptr(),
                0,
                0,
                0,
                0,
                0,
                windows_sys::Win32::UI::WindowsAndMessaging::HWND_MESSAGE,
                null_mut(),
                instance,
                null(),
            );
            if tray.is_null() {
                return Err("failed to create tray message window".into());
            }
            create_child_button(handoff, instance, ID_ACCEPT, "确认下载", 250, 170, 100, 28);
            create_child_button(handoff, instance, ID_REJECT, "取消", 360, 170, 80, 28);
            create_child_button(complete, instance, ID_COMPLETE_CLOSE, "关闭", 310, 120, 80, 28);
            place_bottom_right(progress, 320, 96);
            ShowWindow(handoff, SW_HIDE);
            ShowWindow(progress, SW_HIDE);
            ShowWindow(complete, SW_HIDE);
            ShowWindow(main, SW_HIDE);

            let host = Box::leak(Box::new(Self {
                hwnds: Hwnds {
                    handoff,
                    progress,
                    complete,
                    main,
                    tray,
                },
                shell,
                pending: Mutex::new(Vec::new()),
                core: Mutex::new(None),
                nid_added: Mutex::new(false),
            }));
            HOST = Some(host);
            // 5.0.0 keeps the existing desktop tray for Open/Quit. Adding a
            // second NIM_ADD icon here would sit next to it. Confirm/progress/
            // complete still use the pre-created HWNDs; this message-only
            // window receives offer events.
            Ok(host)
        }
    }

    pub fn enqueue(&self, event: Value) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.push(event);
        }
        unsafe {
            PostMessageW(self.hwnds.tray, WM_SHELL_EVENT, 0, 0);
        }
    }

    pub fn apply_pending(&self) {
        let events = {
            let mut pending = self.pending.lock().unwrap_or_else(|err| err.into_inner());
            std::mem::take(&mut *pending)
        };
        for event in events {
            let mut shell = self.shell.lock().unwrap_or_else(|err| err.into_inner());
            let kind = shell.apply_event(&event).unwrap_or_default();
            drop(shell);
            unsafe {
                match kind.as_str() {
                    "handoff" => {
                        Invalidate(self.hwnds.handoff);
                        ShowWindow(self.hwnds.handoff, SW_SHOWNORMAL);
                        SetForegroundWindow(self.hwnds.handoff);
                    }
                    "progress" => {
                        Invalidate(self.hwnds.progress);
                        ShowWindow(self.hwnds.progress, SW_SHOWNOACTIVATE);
                    }
                    "complete" => {
                        Invalidate(self.hwnds.complete);
                        ShowWindow(self.hwnds.complete, SW_SHOWNORMAL);
                    }
                    "shutdown" => {
                        remove_tray_icon(self);
                        PostQuitMessage(0);
                    }
                    _ => {}
                }
            }
        }
        let shell = self.shell.lock().unwrap_or_else(|err| err.into_inner());
        unsafe {
            if shell.main_open {
                ShowWindow(self.hwnds.main, SW_SHOWNORMAL);
            } else {
                ShowWindow(self.hwnds.main, SW_HIDE);
            }
            if !shell.windows.handoff.visible {
                ShowWindow(self.hwnds.handoff, SW_HIDE);
            }
        }
    }
}

static mut HOST: Option<&'static Win32Host> = None;

pub fn host() -> Option<&'static Win32Host> {
    unsafe { HOST }
}

pub fn run_loop() {
    unsafe {
        let mut msg = std::mem::zeroed::<MSG>();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn claim_single_instance() -> Result<(), String> {
    let handle = CreateMutexW(null(), 0, wide("Local\\HLSDownloaderNativeShell").as_ptr());
    if handle.is_null() {
        return Err("CreateMutexW failed".into());
    }
    if GetLastError() == ERROR_ALREADY_EXISTS {
        return Err("native shell already running".into());
    }
    Ok(())
}

fn class_handoff() -> Vec<u16> {
    wide("HLSNativeHandoff")
}
fn class_progress() -> Vec<u16> {
    wide("HLSNativeProgress")
}
fn class_complete() -> Vec<u16> {
    wide("HLSNativeComplete")
}
fn class_main() -> Vec<u16> {
    wide("HLSNativeMain")
}
fn class_tray() -> Vec<u16> {
    wide("HLSNativeTray")
}

unsafe fn register_class(
    instance: windows_sys::Win32::Foundation::HINSTANCE,
    class_name: Vec<u16>,
    proc: windows_sys::Win32::UI::WindowsAndMessaging::WNDPROC,
) -> Result<(), String> {
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: (COLOR_WINDOW + 1) as _,
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    if RegisterClassW(&class) == 0 {
        return Err("RegisterClassW failed".into());
    }
    std::mem::forget(class_name);
    Ok(())
}

unsafe fn create_overlay(
    instance: windows_sys::Win32::Foundation::HINSTANCE,
    class_name: Vec<u16>,
    title: &str,
    width: i32,
    height: i32,
    no_activate: bool,
    tool: bool,
) -> Result<HWND, String> {
    let mut ex = WS_EX_TOPMOST;
    if no_activate {
        ex |= WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
    } else if tool {
        ex |= 0;
    }
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
    let hwnd = CreateWindowExW(
        ex,
        class_name.as_ptr(),
        wide(title).as_ptr(),
        style,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        width,
        height,
        null_mut(),
        null_mut(),
        instance,
        null(),
    );
    std::mem::forget(class_name);
    if hwnd.is_null() {
        return Err(format!("CreateWindowExW failed for {title}"));
    }
    Ok(hwnd)
}

unsafe fn create_child_button(parent: HWND, instance: windows_sys::Win32::Foundation::HINSTANCE, id: usize, label: &str, x: i32, y: i32, w: i32, h: i32) {
    CreateWindowExW(
        0,
        wide("BUTTON").as_ptr(),
        wide(label).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        x,
        y,
        w,
        h,
        parent,
        id as _,
        instance,
        null(),
    );
}

unsafe fn place_bottom_right(hwnd: HWND, width: i32, height: i32) {
    let x = (GetSystemMetrics(SM_CXSCREEN) - width - 16).max(12);
    let y = (GetSystemMetrics(SM_CYSCREEN) - height - 48).max(12);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, 0);
}

#[allow(dead_code)]
unsafe fn add_tray_icon(host: &Win32Host) {
    let mut nid = std::mem::zeroed::<NOTIFYICONDATAW>();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = host.hwnds.tray;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
    let tip = wide("HLS Downloader");
    for (index, ch) in tip.iter().take(nid.szTip.len().saturating_sub(1)).enumerate() {
        nid.szTip[index] = *ch;
    }
    if Shell_NotifyIconW(NIM_ADD, &nid) != 0 {
        if let Ok(mut added) = host.nid_added.lock() {
            *added = true;
        }
    }
}

unsafe fn remove_tray_icon(host: &Win32Host) {
    let mut nid = std::mem::zeroed::<NOTIFYICONDATAW>();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = host.hwnds.tray;
    nid.uID = 1;
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn Invalidate(hwnd: HWND) {
    windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, null(), 1);
}

unsafe extern "system" fn handoff_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_handoff(hwnd);
            0
        }
        WM_COMMAND => {
            let id = (wparam as usize) & 0xffff;
            if let Some(host) = host() {
                let (handoff_id, core) = {
                    let mut shell = host.shell.lock().unwrap_or_else(|err| err.into_inner());
                    let handoff_id = shell.snapshot.as_ref().map(|item| item.id.clone()).unwrap_or_default();
                    if id == ID_ACCEPT {
                        shell.accept();
                    } else if id == ID_REJECT {
                        shell.reject();
                    }
                    let core = host.core.lock().unwrap_or_else(|err| err.into_inner()).clone();
                    (handoff_id, core)
                };
                ShowWindow(hwnd, SW_HIDE);
                if let Some(core) = core {
                    std::thread::spawn(move || {
                        if id == ID_ACCEPT {
                            let _ = core.accept(&handoff_id);
                        } else if id == ID_REJECT {
                            let _ = core.reject(&handoff_id);
                        }
                    });
                }
            }
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            if let Some(host) = host() {
                host.shell.lock().unwrap_or_else(|err| err.into_inner()).hide_handoff();
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_generic(hwnd);
            0
        }
        WM_COMMAND => {
            if ((wparam as usize) & 0xffff) == ID_COMPLETE_CLOSE {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            if let Some(host) = host() {
                let mut shell = host.shell.lock().unwrap_or_else(|err| err.into_inner());
                if hwnd == host.hwnds.main {
                    shell.hide_main();
                } else if hwnd == host.hwnds.progress {
                    shell.windows.progress.visible = false;
                } else if hwnd == host.hwnds.complete {
                    shell.windows.complete.visible = false;
                }
            }
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SHELL_EVENT => {
            if let Some(host) = host() {
                host.apply_pending();
            }
            0
        }
        WM_TRAY => {
            if lparam as u32 == WM_RBUTTONUP || lparam as u32 == WM_LBUTTONUP {
                show_tray_menu(hwnd);
            }
            0
        }
        WM_COMMAND => {
            let id = (wparam as usize) & 0xffff;
            if let Some(host) = host() {
                if id == ID_TRAY_OPEN {
                    host.shell.lock().unwrap_or_else(|err| err.into_inner()).open_main().ok();
                    ShowWindow(host.hwnds.main, SW_SHOWNORMAL);
                    SetForegroundWindow(host.hwnds.main);
                } else if id == ID_TRAY_EXIT {
                    host.shell.lock().unwrap_or_else(|err| err.into_inner()).shutdown();
                    remove_tray_icon(host);
                    PostQuitMessage(0);
                }
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    AppendMenuW(menu, MF_STRING, ID_TRAY_OPEN, wide("打开主窗口").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_TRAY_EXIT, wide("退出").as_ptr());
    let mut point = POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    SetForegroundWindow(hwnd);
    TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, 0, hwnd, null());
}

unsafe fn paint_handoff(hwnd: HWND) {
    let mut ps = std::mem::zeroed::<PAINTSTRUCT>();
    let hdc = BeginPaint(hwnd, &mut ps);
    FillRect(hdc, &ps.rcPaint, (COLOR_WINDOW + 1) as _);
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, 0x001A1A1A);
    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));
    let snapshot = host()
        .and_then(|item| item.shell.lock().ok().and_then(|shell| shell.snapshot.clone()))
        .unwrap_or_default();
    draw_line(hdc, 16, 16, 440, 24, &format!("文件：{}", snapshot.filename));
    draw_line(hdc, 16, 48, 440, 40, &format!("链接：{}", snapshot.url));
    draw_line(
        hdc,
        16,
        96,
        440,
        24,
        &format!("大小：{}", format_size(snapshot.size)),
    );
    EndPaint(hwnd, &ps);
}

unsafe fn paint_generic(hwnd: HWND) {
    let mut ps = std::mem::zeroed::<PAINTSTRUCT>();
    let hdc = BeginPaint(hwnd, &mut ps);
    FillRect(hdc, &ps.rcPaint, (COLOR_WINDOW + 1) as _);
    SetBkMode(hdc, TRANSPARENT as i32);
    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));
    if let Some(host) = host() {
        let shell = host.shell.lock().unwrap_or_else(|err| err.into_inner());
        if hwnd == host.hwnds.progress {
            let label = shell
                .progress_tasks
                .first()
                .and_then(|item| item.get("filename"))
                .and_then(Value::as_str)
                .unwrap_or("正在下载");
            draw_line(hdc, 16, 16, 280, 24, label);
            let percent = shell
                .progress_tasks
                .first()
                .and_then(|item| item.get("percent"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            draw_line(hdc, 16, 48, 280, 24, &format!("{percent}%"));
        } else if hwnd == host.hwnds.complete {
            let name = shell
                .complete_item
                .as_ref()
                .and_then(|item| item.get("filename"))
                .and_then(Value::as_str)
                .unwrap_or("下载完成");
            draw_line(hdc, 16, 16, 380, 24, &format!("已完成：{name}"));
        } else if hwnd == host.hwnds.main {
            draw_line(hdc, 16, 16, 680, 24, "任务列表将在 5.0.0-beta 接入同一套 API");
            draw_line(hdc, 16, 48, 680, 24, "关闭本窗口回到托盘，下载继续。");
        }
    }
    EndPaint(hwnd, &ps);
}

unsafe fn draw_line(hdc: HDC, x: i32, y: i32, w: i32, h: i32, text: &str) {
    let mut rect = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    let wide_text = wide(text);
    DrawTextW(
        hdc,
        wide_text.as_ptr(),
        (wide_text.len() - 1) as i32,
        &mut rect,
        DT_LEFT | DT_NOPREFIX | DT_WORDBREAK | DT_END_ELLIPSIS | DT_SINGLELINE,
    );
}

fn format_size(size: i64) -> String {
    if size <= 0 {
        "未知".into()
    } else if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

// Keep a Windows message registered so the module is obviously a UI host.
#[allow(dead_code)]
fn reserved_wakeup() -> u32 {
    unsafe { RegisterWindowMessageW(wide("HLSDownloaderNativeShellWake").as_ptr()) }
}

#[allow(dead_code)]
fn _snapshot_title(snapshot: &Snapshot) -> String {
    if snapshot.filename.is_empty() {
        "浏览器下载".into()
    } else {
        snapshot.filename.clone()
    }
}

#[allow(dead_code)]
fn _color(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF::from(r as u32 | ((g as u32) << 8) | ((b as u32) << 16))
}

#[allow(dead_code)]
unsafe fn _destroy_all(host: &Win32Host) {
    DestroyWindow(host.hwnds.handoff);
    DestroyWindow(host.hwnds.progress);
    DestroyWindow(host.hwnds.complete);
    DestroyWindow(host.hwnds.main);
    DestroyWindow(host.hwnds.tray);
}
