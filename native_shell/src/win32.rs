//! Win32 tray + pre-created HWND overlays.
//!
//! Confirm / progress / complete / main are created once at boot (`SW_HIDE`),
//! then shown with the offer snapshot. Closing a window hides it; the process
//! stays in the tray.

#![allow(non_snake_case)]

use crate::core_client::CoreClient;
use crate::surfaces::{ResidentShell, Snapshot};
use crate::task_list::{FileCategory, StatusFilter};
use serde_json::{json, Value};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use windows_sys::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject,
    SelectObject, SetBkMode, SetTextColor, COLOR_WINDOW, DEFAULT_GUI_FONT, DT_END_ELLIPSIS,
    DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_WORDBREAK, HDC, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BringWindowToTop, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetClientRect, GetCursorPos, GetDlgItem, GetMessageW, GetSystemMetrics,
    GetWindowTextW, KillTimer, LoadCursorW, LoadIconW, MoveWindow, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SendMessageW, SetForegroundWindow, SetTimer,
    SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu, TranslateMessage, CS_HREDRAW,
    CS_VREDRAW, CW_USEDEFAULT, ES_AUTOHSCROLL, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW,
    IDI_APPLICATION, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, MF_STRING, MSG, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
    TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_PAINT,
    WM_RBUTTONUP, WM_SETFONT, WM_SIZE, WM_TIMER, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_CLIPCHILDREN, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
    WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
};

pub const WM_SHELL_EVENT: u32 = WM_APP + 1;
const WM_TRAY: u32 = WM_APP + 20;
const WM_HANDOFF_RESULT: u32 = WM_APP + 2;
const HANDOFF_ACCEPT_OK: usize = 1;
const HANDOFF_REJECT_OK: usize = 2;
const HANDOFF_ACCEPT_ERR: usize = 3;
const HANDOFF_REJECT_ERR: usize = 4;
const ID_ACCEPT: usize = 1001;
const ID_REJECT: usize = 1002;
const ID_PROGRESS_PAUSE: usize = 1003;
const ID_PROGRESS_HIDE: usize = 1004;
const ID_COMPLETE_CLOSE: usize = 1005;
const ID_COMPLETE_OPEN_FOLDER: usize = 1006;
const ID_COMPLETE_OPEN_FILE: usize = 1007;
const ID_FILENAME: usize = 1010;
const ID_SAVE_DIR: usize = 1011;
const ID_HANDOFF_HINT: usize = 1012;
const ID_TRAY_OPEN: usize = 2001;
const ID_TRAY_EXIT: usize = 2002;
const ID_FILTER_ALL: usize = 3101;
const ID_FILTER_UNFINISHED: usize = 3102;
const ID_FILTER_COMPLETED: usize = 3103;
const ID_CAT_ALL: usize = 3110;
const ID_CAT_VIDEO: usize = 3111;
const ID_CAT_MUSIC: usize = 3112;
const ID_CAT_ARCHIVE: usize = 3113;
const ID_CAT_DOCUMENT: usize = 3114;
const ID_CAT_PROGRAM: usize = 3115;
const ID_CAT_GENERAL: usize = 3116;
const ID_SEARCH: usize = 3120;
const ID_LIST: usize = 3130;
const ID_STATUS: usize = 3131;
const ID_START: usize = 3140;
const ID_PAUSE: usize = 3141;
const ID_DELETE: usize = 3142;
const ID_OPEN_FOLDER: usize = 3143;
const ID_OPEN_FILE: usize = 3146;
const ID_NEW_TASK: usize = 3144;
const ID_SETTINGS: usize = 3145;
const MAIN_REFRESH_TIMER: usize = 1;
const EN_CHANGE: u32 = 0x0300;
const LBN_SELCHANGE: u32 = 1;
const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETCURSEL: u32 = 0x0188;

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
    handoff_error: Mutex<String>,
    handoff_busy: Mutex<bool>,
}

impl Win32Host {
    pub fn boot(shell: Arc<Mutex<ResidentShell>>, own_tray: bool) -> Result<&'static Self, String> {
        unsafe {
            claim_single_instance()?;
            let instance = GetModuleHandleW(null());
            if instance.is_null() {
                return Err("GetModuleHandleW failed".into());
            }
            register_class(instance, class_handoff(), Some(handoff_proc))?;
            register_class(instance, class_progress(), Some(overlay_proc))?;
            register_class(instance, class_complete(), Some(overlay_proc))?;
            register_class(instance, class_main(), Some(main_proc))?;
            register_class(instance, class_tray(), Some(tray_proc))?;

            let handoff = create_overlay(
                instance,
                class_handoff(),
                "浏览器下载",
                520,
                292,
                false,
                true,
            )?;
            let progress = create_overlay(
                instance,
                class_progress(),
                "下载进度",
                360,
                128,
                true,
                false,
            )?;
            let complete = create_overlay(
                instance,
                class_complete(),
                "下载完成",
                440,
                188,
                false,
                true,
            )?;
            let main = create_main_window(instance)?;
            create_main_children(main, instance);
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
            create_child(
                handoff,
                instance,
                "EDIT",
                "",
                ID_FILENAME,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
                0,
                16,
                36,
                488,
                24,
            );
            create_child(
                handoff,
                instance,
                "EDIT",
                "",
                ID_SAVE_DIR,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
                0,
                16,
                112,
                488,
                24,
            );
            create_child_button(handoff, instance, ID_ACCEPT, "确认下载", 290, 220, 100, 28);
            create_child_button(handoff, instance, ID_REJECT, "取消", 400, 220, 80, 28);
            create_child(
                handoff,
                instance,
                "STATIC",
                "",
                ID_HANDOFF_HINT,
                WS_CHILD | WS_VISIBLE,
                0,
                16,
                188,
                260,
                28,
            );
            create_child_button(
                progress,
                instance,
                ID_PROGRESS_PAUSE,
                "暂停",
                168,
                88,
                72,
                26,
            );
            create_child_button(
                progress,
                instance,
                ID_PROGRESS_HIDE,
                "隐藏",
                248,
                88,
                72,
                26,
            );
            create_child_button(
                complete,
                instance,
                ID_COMPLETE_OPEN_FOLDER,
                "打开目录",
                148,
                140,
                88,
                28,
            );
            create_child_button(
                complete,
                instance,
                ID_COMPLETE_OPEN_FILE,
                "打开",
                244,
                140,
                72,
                28,
            );
            create_child_button(
                complete,
                instance,
                ID_COMPLETE_CLOSE,
                "关闭",
                324,
                140,
                72,
                28,
            );
            place_center(handoff, 520, 292);
            place_bottom_right(progress, 360, 128);
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
                handoff_error: Mutex::new(String::new()),
                handoff_busy: Mutex::new(false),
            }));
            HOST = Some(host);
            if own_tray {
                add_tray_icon(host);
            }
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
                        fill_handoff_fields(self.hwnds.handoff);
                        set_handoff_busy(self.hwnds.handoff, false);
                        set_handoff_hint(self.hwnds.handoff, "");
                        Invalidate(self.hwnds.handoff);
                        place_center(self.hwnds.handoff, 520, 292);
                        show_popup(self.hwnds.handoff);
                    }
                    "progress" => {
                        Invalidate(self.hwnds.progress);
                        place_bottom_right(self.hwnds.progress, 360, 128);
                        if self
                            .shell
                            .lock()
                            .map(|state| state.windows.progress.visible)
                            .unwrap_or(false)
                        {
                            ShowWindow(self.hwnds.progress, SW_SHOWNOACTIVATE);
                        } else {
                            ShowWindow(self.hwnds.progress, SW_HIDE);
                        }
                        if self
                            .shell
                            .lock()
                            .map(|state| state.main_open)
                            .unwrap_or(false)
                        {
                            refresh_main_list(self);
                        }
                    }
                    "complete" => {
                        Invalidate(self.hwnds.complete);
                        place_center(self.hwnds.complete, 440, 188);
                        show_popup(self.hwnds.complete);
                        if self
                            .shell
                            .lock()
                            .map(|state| state.main_open)
                            .unwrap_or(false)
                        {
                            refresh_main_list(self);
                        }
                    }
                    "action_error" => {
                        let message = event
                            .get("message")
                            .and_then(Value::as_str)
                            .filter(|text| !text.is_empty())
                            .unwrap_or("操作失败");
                        SetWindowTextW(
                            GetDlgItem(self.hwnds.main, ID_STATUS as i32),
                            wide(&format!("操作失败：{message}")).as_ptr(),
                        );
                    }
                    "open_main" => {
                        place_main(self.hwnds.main);
                        ShowWindow(self.hwnds.main, SW_SHOWNORMAL);
                        SetForegroundWindow(self.hwnds.main);
                        SetTimer(self.hwnds.main, MAIN_REFRESH_TIMER, 1000, None);
                        self.request_task_refresh();
                    }
                    "hide_main" => {
                        KillTimer(self.hwnds.main, MAIN_REFRESH_TIMER);
                        ShowWindow(self.hwnds.main, SW_HIDE);
                    }
                    "tasks" => {
                        if self
                            .shell
                            .lock()
                            .map(|state| state.main_open)
                            .unwrap_or(false)
                        {
                            refresh_main_list(self);
                        }
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
            if !shell.windows.handoff.visible {
                ShowWindow(self.hwnds.handoff, SW_HIDE);
            }
            if !shell.windows.progress.visible {
                ShowWindow(self.hwnds.progress, SW_HIDE);
            }
            if !shell.windows.complete.visible {
                ShowWindow(self.hwnds.complete, SW_HIDE);
            }
        }
        let main_open = shell.main_open;
        drop(shell);
        unsafe {
            if main_open {
                ShowWindow(self.hwnds.main, SW_SHOWNORMAL);
                refresh_main_list(self);
            } else {
                KillTimer(self.hwnds.main, MAIN_REFRESH_TIMER);
                ShowWindow(self.hwnds.main, SW_HIDE);
            }
        }
    }

    pub fn request_task_refresh(&self) {
        let Some(core) = self.core.lock().ok().and_then(|slot| slot.clone()) else {
            return;
        };
        std::thread::spawn(move || {
            if let Ok(tasks) = core.list_tasks() {
                if let Some(host) = host() {
                    host.enqueue(json!({"kind": "tasks", "tasks": tasks}));
                }
            }
        });
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

unsafe fn create_main_window(
    instance: windows_sys::Win32::Foundation::HINSTANCE,
) -> Result<HWND, String> {
    let class_name = class_main();
    let style = WS_OVERLAPPED
        | WS_CAPTION
        | WS_SYSMENU
        | WS_THICKFRAME
        | WS_MINIMIZEBOX
        | WS_MAXIMIZEBOX
        | WS_CLIPCHILDREN;
    let hwnd = CreateWindowExW(
        WS_EX_APPWINDOW,
        class_name.as_ptr(),
        wide("HLS Downloader").as_ptr(),
        style,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        860,
        540,
        null_mut(),
        null_mut(),
        instance,
        null(),
    );
    std::mem::forget(class_name);
    if hwnd.is_null() {
        return Err("CreateWindowExW failed for main list".into());
    }
    place_main(hwnd);
    Ok(hwnd)
}

unsafe fn create_main_children(parent: HWND, instance: windows_sys::Win32::Foundation::HINSTANCE) {
    create_child_button(parent, instance, ID_FILTER_ALL, "全部", 12, 10, 64, 26);
    create_child_button(
        parent,
        instance,
        ID_FILTER_UNFINISHED,
        "未完成",
        80,
        10,
        72,
        26,
    );
    create_child_button(
        parent,
        instance,
        ID_FILTER_COMPLETED,
        "已完成",
        156,
        10,
        72,
        26,
    );
    create_child(
        parent,
        instance,
        "EDIT",
        "",
        ID_SEARCH,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
        0,
        480,
        10,
        360,
        26,
    );
    create_child_button(parent, instance, ID_CAT_ALL, "全部类型", 12, 42, 80, 24);
    create_child_button(parent, instance, ID_CAT_VIDEO, "视频", 96, 42, 56, 24);
    create_child_button(parent, instance, ID_CAT_MUSIC, "音乐", 156, 42, 56, 24);
    create_child_button(parent, instance, ID_CAT_ARCHIVE, "压缩包", 216, 42, 64, 24);
    create_child_button(parent, instance, ID_CAT_DOCUMENT, "文档", 284, 42, 56, 24);
    create_child_button(parent, instance, ID_CAT_PROGRAM, "程序", 344, 42, 56, 24);
    create_child_button(parent, instance, ID_CAT_GENERAL, "常规", 404, 42, 56, 24);
    create_child(
        parent,
        instance,
        "LISTBOX",
        "",
        ID_LIST,
        WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WS_TABSTOP
            | (LBS_NOTIFY as u32)
            | (LBS_NOINTEGRALHEIGHT as u32),
        WS_EX_CLIENTEDGE,
        12,
        74,
        820,
        360,
    );
    create_child(
        parent,
        instance,
        "STATIC",
        "暂无任务 · 关闭窗口回到托盘",
        ID_STATUS,
        WS_CHILD | WS_VISIBLE,
        0,
        12,
        444,
        500,
        22,
    );
    create_child_button(parent, instance, ID_START, "开始", 12, 470, 72, 28);
    create_child_button(parent, instance, ID_PAUSE, "暂停", 90, 470, 72, 28);
    create_child_button(parent, instance, ID_DELETE, "删除", 168, 470, 72, 28);
    create_child_button(
        parent,
        instance,
        ID_OPEN_FOLDER,
        "打开目录",
        246,
        470,
        88,
        28,
    );
    create_child_button(parent, instance, ID_OPEN_FILE, "打开", 342, 470, 72, 28);
    create_child_button(parent, instance, ID_NEW_TASK, "新建", 680, 470, 72, 28);
    create_child_button(parent, instance, ID_SETTINGS, "设置", 758, 470, 72, 28);
    layout_main(parent);
}

unsafe fn create_child(
    parent: HWND,
    instance: windows_sys::Win32::Foundation::HINSTANCE,
    class_name: &str,
    label: &str,
    id: usize,
    style: u32,
    ex: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let hwnd = CreateWindowExW(
        ex,
        wide(class_name).as_ptr(),
        wide(label).as_ptr(),
        style,
        x,
        y,
        w,
        h,
        parent,
        id as _,
        instance,
        null(),
    );
    if !hwnd.is_null() {
        SendMessageW(
            hwnd,
            WM_SETFONT,
            GetStockObject(DEFAULT_GUI_FONT) as WPARAM,
            1,
        );
    }
    hwnd
}

unsafe fn create_child_button(
    parent: HWND,
    instance: windows_sys::Win32::Foundation::HINSTANCE,
    id: usize,
    label: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    create_child(
        parent,
        instance,
        "BUTTON",
        label,
        id,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        0,
        x,
        y,
        w,
        h,
    );
}

unsafe fn place_main(hwnd: HWND) {
    SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
}

unsafe fn layout_main(hwnd: HWND) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut rect);
    let width = (rect.right - rect.left).max(420);
    let height = (rect.bottom - rect.top).max(280);
    let search = GetDlgItem(hwnd, ID_SEARCH as i32);
    MoveWindow(search, 244, 10, (width - 256).max(120), 26, 1);
    let list = GetDlgItem(hwnd, ID_LIST as i32);
    MoveWindow(list, 12, 74, width - 24, (height - 120).max(80), 1);
    let status = GetDlgItem(hwnd, ID_STATUS as i32);
    MoveWindow(status, 12, height - 42, (width - 430).max(120), 22, 1);
    let y = height - 40;
    MoveWindow(GetDlgItem(hwnd, ID_START as i32), 12, y, 72, 28, 1);
    MoveWindow(GetDlgItem(hwnd, ID_PAUSE as i32), 90, y, 72, 28, 1);
    MoveWindow(GetDlgItem(hwnd, ID_DELETE as i32), 168, y, 72, 28, 1);
    MoveWindow(GetDlgItem(hwnd, ID_OPEN_FOLDER as i32), 246, y, 88, 28, 1);
    MoveWindow(GetDlgItem(hwnd, ID_OPEN_FILE as i32), 342, y, 72, 28, 1);
    MoveWindow(
        GetDlgItem(hwnd, ID_NEW_TASK as i32),
        width - 164,
        y,
        72,
        28,
        1,
    );
    MoveWindow(
        GetDlgItem(hwnd, ID_SETTINGS as i32),
        width - 84,
        y,
        72,
        28,
        1,
    );
}

unsafe fn refresh_main_list(host: &Win32Host) {
    let hwnd = host.hwnds.main;
    let list = GetDlgItem(hwnd, ID_LIST as i32);
    let (lines, selected, summary, can_start, can_pause, can_open, status_filter, category) = {
        let shell = host.shell.lock().unwrap_or_else(|err| err.into_inner());
        let visible = shell.task_list.visible();
        let lines: Vec<String> = visible.iter().map(|task| task.display_line()).collect();
        let selected = shell.task_list.selected_index();
        let summary = shell.task_list.summary();
        let row = shell.task_list.selected();
        let can_start = row
            .and_then(crate::task_list::TaskRow::start_kind)
            .is_some();
        let can_pause = row.map(|task| task.has_action("pause")).unwrap_or(false);
        let can_open = row
            .map(|task| task.has_action("open") || task.has_action("launch"))
            .unwrap_or(false);
        (
            lines,
            selected,
            summary,
            can_start,
            can_pause,
            can_open,
            shell.task_list.status_filter,
            shell.task_list.category,
        )
    };
    SendMessageW(list, LB_RESETCONTENT, 0, 0);
    for line in &lines {
        let text = wide(line);
        SendMessageW(list, LB_ADDSTRING, 0, text.as_ptr() as LPARAM);
    }
    if selected >= 0 {
        SendMessageW(list, LB_SETCURSEL, selected as WPARAM, 0);
    }
    SetWindowTextW(GetDlgItem(hwnd, ID_STATUS as i32), wide(&summary).as_ptr());
    EnableWindow(GetDlgItem(hwnd, ID_START as i32), i32::from(can_start));
    EnableWindow(GetDlgItem(hwnd, ID_PAUSE as i32), i32::from(can_pause));
    EnableWindow(GetDlgItem(hwnd, ID_DELETE as i32), i32::from(selected >= 0));
    EnableWindow(GetDlgItem(hwnd, ID_OPEN_FOLDER as i32), i32::from(can_open));
    EnableWindow(GetDlgItem(hwnd, ID_OPEN_FILE as i32), i32::from(can_open));
    mark_filter(
        hwnd,
        ID_FILTER_ALL,
        status_filter == StatusFilter::All,
        "全部",
    );
    mark_filter(
        hwnd,
        ID_FILTER_UNFINISHED,
        status_filter == StatusFilter::Unfinished,
        "未完成",
    );
    mark_filter(
        hwnd,
        ID_FILTER_COMPLETED,
        status_filter == StatusFilter::Completed,
        "已完成",
    );
    mark_filter(hwnd, ID_CAT_ALL, category == FileCategory::All, "全部类型");
    mark_filter(hwnd, ID_CAT_VIDEO, category == FileCategory::Video, "视频");
    mark_filter(hwnd, ID_CAT_MUSIC, category == FileCategory::Music, "音乐");
    mark_filter(
        hwnd,
        ID_CAT_ARCHIVE,
        category == FileCategory::Archive,
        "压缩包",
    );
    mark_filter(
        hwnd,
        ID_CAT_DOCUMENT,
        category == FileCategory::Document,
        "文档",
    );
    mark_filter(
        hwnd,
        ID_CAT_PROGRAM,
        category == FileCategory::Program,
        "程序",
    );
    mark_filter(
        hwnd,
        ID_CAT_GENERAL,
        category == FileCategory::General,
        "常规",
    );
}

unsafe fn mark_filter(parent: HWND, id: usize, active: bool, label: &str) {
    let text = if active {
        format!("● {label}")
    } else {
        label.to_string()
    };
    SetWindowTextW(GetDlgItem(parent, id as i32), wide(&text).as_ptr());
}

unsafe fn search_text(parent: HWND) -> String {
    let hwnd = GetDlgItem(parent, ID_SEARCH as i32);
    let mut buffer = [0u16; 256];
    let len = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    if len <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..len as usize])
    }
}

fn run_selected_action(window_host: &Win32Host, action: &str) {
    let (task_id, mapped) = {
        let shell = window_host
            .shell
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let Some(task) = shell.task_list.selected() else {
            return;
        };
        let mapped = match action {
            "start" => task.start_kind().unwrap_or("start"),
            other => other,
        };
        (task.id.clone(), mapped.to_string())
    };
    let Some(core) = window_host.core.lock().ok().and_then(|slot| slot.clone()) else {
        window_host.enqueue(json!({"kind": "action_error", "message": "桌面核心未连接"}));
        return;
    };
    std::thread::spawn(move || {
        let result = core.run_task_action(&task_id, &mapped);
        if let Some(host) = host() {
            if mapped == "delete" && result.is_ok() {
                host.shell
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .task_list
                    .remove(&task_id);
            }
            if let Err(err) = result {
                host.enqueue(json!({"kind": "action_error", "message": err}));
            }
            host.request_task_refresh();
        }
    });
}

unsafe fn place_center(hwnd: HWND, width: i32, height: i32) {
    let x = ((GetSystemMetrics(SM_CXSCREEN) - width) / 2).max(12);
    let y = ((GetSystemMetrics(SM_CYSCREEN) - height) / 3).max(12);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, 0);
}

unsafe fn show_popup(hwnd: HWND) {
    ShowWindow(hwnd, SW_SHOWNORMAL);
    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
    );
    BringWindowToTop(hwnd);
    SetForegroundWindow(hwnd);
}

unsafe fn fill_handoff_fields(hwnd: HWND) {
    let snapshot = host()
        .and_then(|item| {
            item.shell
                .lock()
                .ok()
                .and_then(|shell| shell.snapshot.clone())
        })
        .unwrap_or_default();
    SetWindowTextW(
        GetDlgItem(hwnd, ID_FILENAME as i32),
        wide(&snapshot.filename).as_ptr(),
    );
    SetWindowTextW(
        GetDlgItem(hwnd, ID_SAVE_DIR as i32),
        wide(&snapshot.download_dir).as_ptr(),
    );
}

unsafe fn dlg_text(parent: HWND, id: usize) -> String {
    let hwnd = GetDlgItem(parent, id as i32);
    if hwnd.is_null() {
        return String::new();
    }
    let mut buffer = [0u16; 1024];
    let len = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    if len <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..len as usize])
    }
}

unsafe fn place_bottom_right(hwnd: HWND, width: i32, height: i32) {
    let x = (GetSystemMetrics(SM_CXSCREEN) - width - 16).max(12);
    let y = (GetSystemMetrics(SM_CYSCREEN) - height - 48).max(12);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, 0);
}

unsafe fn add_tray_icon(host: &Win32Host) {
    let mut nid = std::mem::zeroed::<NOTIFYICONDATAW>();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = host.hwnds.tray;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
    let tip = wide("HLS Downloader");
    for (index, ch) in tip
        .iter()
        .take(nid.szTip.len().saturating_sub(1))
        .enumerate()
    {
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

unsafe fn set_handoff_busy(hwnd: HWND, busy: bool) {
    if let Some(host) = host() {
        if let Ok(mut slot) = host.handoff_busy.lock() {
            *slot = busy;
        }
    }
    EnableWindow(GetDlgItem(hwnd, ID_ACCEPT as i32), i32::from(!busy));
    EnableWindow(GetDlgItem(hwnd, ID_REJECT as i32), i32::from(!busy));
}

unsafe fn set_handoff_hint(hwnd: HWND, text: &str) {
    SetWindowTextW(
        GetDlgItem(hwnd, ID_HANDOFF_HINT as i32),
        wide(text).as_ptr(),
    );
}

unsafe fn handoff_is_busy() -> bool {
    host()
        .and_then(|item| item.handoff_busy.lock().ok().map(|slot| *slot))
        .unwrap_or(false)
}

unsafe fn begin_handoff_core(hwnd: HWND, accept: bool) {
    if handoff_is_busy() {
        return;
    }
    let Some(window_host) = host() else {
        return;
    };
    let filename = dlg_text(hwnd, ID_FILENAME);
    let download_dir = dlg_text(hwnd, ID_SAVE_DIR);
    let (handoff_id, core) = {
        let shell = window_host
            .shell
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let handoff_id = shell
            .snapshot
            .as_ref()
            .map(|item| item.id.clone())
            .unwrap_or_default();
        let core = window_host
            .core
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        (handoff_id, core)
    };
    let Some(core) = core else {
        set_handoff_hint(hwnd, "桌面核心未连接，请重试");
        return;
    };
    set_handoff_hint(
        hwnd,
        if accept {
            "正在确认…"
        } else {
            "正在取消…"
        },
    );
    set_handoff_busy(hwnd, true);
    let hwnd_bits = hwnd as usize;
    std::thread::spawn(move || {
        let result = if accept {
            core.accept(&handoff_id, &filename, &download_dir)
        } else {
            core.reject(&handoff_id)
        };
        if let Some(host) = host() {
            let code = match (accept, result.is_ok()) {
                (true, true) => HANDOFF_ACCEPT_OK,
                (false, true) => HANDOFF_REJECT_OK,
                (true, false) => HANDOFF_ACCEPT_ERR,
                (false, false) => HANDOFF_REJECT_ERR,
            };
            if let Err(err) = result {
                if let Ok(mut slot) = host.handoff_error.lock() {
                    *slot = err;
                }
            }
            PostMessageW(hwnd_bits as HWND, WM_HANDOFF_RESULT, code, 0);
        }
    });
}

unsafe fn finish_handoff_core(hwnd: HWND, wparam: WPARAM) {
    let code = wparam as usize;
    let ok = code == HANDOFF_ACCEPT_OK || code == HANDOFF_REJECT_OK;
    set_handoff_busy(hwnd, false);
    if !ok {
        let err = host()
            .and_then(|item| item.handoff_error.lock().ok().map(|slot| slot.clone()))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "桌面端未接受请求".into());
        set_handoff_hint(hwnd, &err);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        return;
    }
    set_handoff_hint(hwnd, "");
    if let Some(host) = host() {
        let mut shell = host.shell.lock().unwrap_or_else(|err| err.into_inner());
        if code == HANDOFF_ACCEPT_OK {
            shell.accept();
        } else {
            shell.reject();
        }
    }
    ShowWindow(hwnd, SW_HIDE);
}

unsafe extern "system" fn handoff_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_handoff(hwnd);
            0
        }
        WM_COMMAND => {
            let id = (wparam as usize) & 0xffff;
            if id == ID_ACCEPT {
                begin_handoff_core(hwnd, true);
            } else if id == ID_REJECT {
                begin_handoff_core(hwnd, false);
            }
            0
        }
        WM_HANDOFF_RESULT => {
            finish_handoff_core(hwnd, wparam);
            0
        }
        WM_CLOSE => {
            begin_handoff_core(hwnd, false);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn main_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout_main(hwnd);
            0
        }
        WM_TIMER => {
            if wparam == MAIN_REFRESH_TIMER {
                if let Some(host) = host() {
                    host.request_task_refresh();
                }
            }
            0
        }
        WM_COMMAND => {
            let id = (wparam as usize) & 0xffff;
            let code = ((wparam as u32) >> 16) & 0xffff;
            if let Some(host) = host() {
                if code == EN_CHANGE && id == ID_SEARCH {
                    let query = search_text(hwnd);
                    host.shell
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .set_query(query);
                    refresh_main_list(host);
                    return 0;
                }
                if code == LBN_SELCHANGE && id == ID_LIST {
                    let list = GetDlgItem(hwnd, ID_LIST as i32);
                    let index = SendMessageW(list, LB_GETCURSEL, 0, 0) as i32;
                    host.shell
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .task_list
                        .select_visible_index(index);
                    refresh_main_list(host);
                    return 0;
                }
                if code == 0 || code == 1 {
                    match id {
                        ID_FILTER_ALL => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_status_filter(StatusFilter::All),
                        ID_FILTER_UNFINISHED => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_status_filter(StatusFilter::Unfinished),
                        ID_FILTER_COMPLETED => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_status_filter(StatusFilter::Completed),
                        ID_CAT_ALL => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_category(FileCategory::All),
                        ID_CAT_VIDEO => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_category(FileCategory::Video),
                        ID_CAT_MUSIC => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_category(FileCategory::Music),
                        ID_CAT_ARCHIVE => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_category(FileCategory::Archive),
                        ID_CAT_DOCUMENT => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_category(FileCategory::Document),
                        ID_CAT_PROGRAM => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_category(FileCategory::Program),
                        ID_CAT_GENERAL => host
                            .shell
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .set_category(FileCategory::General),
                        ID_START => {
                            run_selected_action(host, "start");
                            return 0;
                        }
                        ID_PAUSE => {
                            run_selected_action(host, "pause");
                            return 0;
                        }
                        ID_DELETE => {
                            run_selected_action(host, "delete");
                            return 0;
                        }
                        ID_OPEN_FOLDER => {
                            run_selected_action(host, "open");
                            return 0;
                        }
                        ID_OPEN_FILE => {
                            run_selected_action(host, "launch");
                            return 0;
                        }
                        ID_NEW_TASK | ID_SETTINGS => {
                            let root = crate::install_root();
                            let spawned = root
                                .as_ref()
                                .map(|path| crate::spawn_desktop_ui(path))
                                .unwrap_or(false);
                            if !spawned {
                                if let Some(core) =
                                    host.core.lock().ok().and_then(|slot| slot.clone())
                                {
                                    std::thread::spawn(move || {
                                        let _ = core.open_settings();
                                    });
                                }
                            }
                            return 0;
                        }
                        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
                    }
                    refresh_main_list(host);
                }
            }
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            KillTimer(hwnd, MAIN_REFRESH_TIMER);
            if let Some(host) = host() {
                host.shell
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .hide_main();
            }
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_generic(hwnd);
            0
        }
        WM_COMMAND => {
            let id = (wparam as usize) & 0xffff;
            if let Some(window_host) = host() {
                let (task_id, core, hide) = {
                    let mut shell = window_host
                        .shell
                        .lock()
                        .unwrap_or_else(|err| err.into_inner());
                    let task_id = if hwnd == window_host.hwnds.progress {
                        shell
                            .progress_tasks
                            .first()
                            .and_then(|item| item.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    } else {
                        shell
                            .complete_item
                            .as_ref()
                            .and_then(|item| item.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    };
                    let mut hide = false;
                    if id == ID_PROGRESS_HIDE {
                        shell.windows.progress.visible = false;
                        hide = true;
                    } else if id == ID_COMPLETE_CLOSE {
                        shell.windows.complete.visible = false;
                        hide = true;
                    }
                    let core = window_host
                        .core
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .clone();
                    (task_id, core, hide)
                };
                if hide {
                    ShowWindow(hwnd, SW_HIDE);
                }
                if (id == ID_COMPLETE_OPEN_FOLDER || id == ID_COMPLETE_OPEN_FILE)
                    && task_id.is_empty()
                {
                    window_host.enqueue(json!({
                        "kind": "action_error",
                        "message": "无法打开：任务编号缺失"
                    }));
                } else if let Some(core) = core {
                    std::thread::spawn(move || {
                        let result = match id {
                            ID_PROGRESS_PAUSE => core.pause_task(&task_id),
                            ID_COMPLETE_OPEN_FOLDER => core.open_explorer(&task_id),
                            ID_COMPLETE_OPEN_FILE => core.launch_file(&task_id, true),
                            _ => Ok(json!({"ok": true})),
                        };
                        if let (Err(err), Some(window_host)) = (result, host()) {
                            window_host.enqueue(json!({"kind": "action_error", "message": err}));
                        }
                    });
                }
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

unsafe extern "system" fn tray_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
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
                    host.shell
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .open_main()
                        .ok();
                    place_main(host.hwnds.main);
                    ShowWindow(host.hwnds.main, SW_SHOWNORMAL);
                    SetForegroundWindow(host.hwnds.main);
                    SetTimer(host.hwnds.main, MAIN_REFRESH_TIMER, 1000, None);
                    host.request_task_refresh();
                } else if id == ID_TRAY_EXIT {
                    host.shell
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .shutdown();
                    let core = host
                        .core
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .clone();
                    if let Some(core) = core {
                        let _ = core.shutdown();
                    }
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
        .and_then(|item| {
            item.shell
                .lock()
                .ok()
                .and_then(|shell| shell.snapshot.clone())
        })
        .unwrap_or_default();
    draw_line(hdc, 16, 16, 80, 18, "文件名");
    draw_line(
        hdc,
        16,
        66,
        480,
        20,
        &format!(
            "大小：{} · {}",
            format_size(snapshot.size),
            kind_label(&snapshot.resource_kind)
        ),
    );
    draw_line(hdc, 16, 92, 80, 18, "保存到");
    if !snapshot.url.is_empty() {
        draw_line(hdc, 16, 144, 480, 36, &format!("链接：{}", snapshot.url));
    }
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
            let task = shell.progress_tasks.first().cloned().unwrap_or(Value::Null);
            let label = task
                .get("filename")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("正在下载");
            draw_line(hdc, 16, 12, 328, 22, label);
            let percent = task
                .get("progress_percent")
                .and_then(Value::as_f64)
                .or_else(|| task.get("percent").and_then(Value::as_f64))
                .unwrap_or(0.0)
                .clamp(0.0, 100.0);
            let speed = task
                .get("speed_bytes_per_sec")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            draw_line(
                hdc,
                16,
                36,
                328,
                20,
                &format!("{:.0}% · {}", percent, format_speed(speed)),
            );
            draw_progress_bar(hdc, 16, 62, 328, 14, percent);
        } else if hwnd == host.hwnds.complete {
            let item = shell.complete_item.clone().unwrap_or(Value::Null);
            let name = item
                .get("filename")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("下载完成");
            let path = item
                .get("output_path")
                .and_then(Value::as_str)
                .unwrap_or("");
            draw_line(hdc, 16, 16, 400, 24, &format!("已完成：{name}"));
            if !path.is_empty() {
                draw_line(hdc, 16, 48, 400, 36, path);
            }
            let size = item
                .get("downloaded_bytes")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if size > 0 {
                draw_line(
                    hdc,
                    16,
                    88,
                    400,
                    20,
                    &format!("大小：{}", format_size(size)),
                );
            }
            if looks_executable(path) {
                draw_line(hdc, 16, 108, 400, 20, "可执行文件请核对来源后再打开");
            }
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

unsafe fn draw_progress_bar(hdc: HDC, x: i32, y: i32, w: i32, h: i32, percent: f64) {
    let mut track = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    FillRect(hdc, &track, (COLOR_WINDOW + 1) as _);
    let brush = CreateSolidBrush(0x00D47800);
    if !brush.is_null() {
        let filled = ((w as f64) * (percent / 100.0)).round() as i32;
        track.right = x + filled.max(0).min(w);
        FillRect(hdc, &track, brush);
        DeleteObject(brush);
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "hls" => "HLS",
        "dash" => "DASH",
        "torrent" => "BT",
        "media" => "媒体",
        _ => "文件",
    }
}

fn looks_executable(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".bat", ".cmd", ".com", ".exe", ".js", ".msi", ".ps1", ".scr", ".vbs",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn format_speed(speed: f64) -> String {
    if speed <= 0.0 {
        "0 B/s".into()
    } else if speed < 1024.0 {
        format!("{speed:.0} B/s")
    } else if speed < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", speed / 1024.0)
    } else {
        format!("{:.1} MB/s", speed / (1024.0 * 1024.0))
    }
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
