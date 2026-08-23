//! Explorer file drop onto the main workbench HWND.

use std::sync::mpsc::Sender;

pub fn attach_file_drop(
    title: &str,
    tx: Sender<Vec<String>>,
    wake: impl Fn() + Send + 'static,
) -> bool {
    #[cfg(windows)]
    unsafe {
        return install(title, tx, Box::new(wake));
    }
    #[cfg(not(windows))]
    {
        let _ = (title, tx, wake);
        false
    }
}

#[cfg(windows)]
unsafe fn install(title: &str, tx: Sender<Vec<String>>, wake: Box<dyn Fn() + Send>) -> bool {
    use windows_sys::Win32::UI::Shell::DragAcceptFiles;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowLongPtrW, SetWindowLongPtrW, GWLP_WNDPROC,
    };
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
    if hwnd.is_null() {
        return false;
    }
    DragAcceptFiles(hwnd, 1);
    let previous = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
    if previous == 0 {
        return false;
    }
    {
        let mut slot = DROP_SLOT.lock().unwrap_or_else(|error| error.into_inner());
        *slot = Some(DropSlot {
            hwnd: hwnd as isize,
            previous,
            tx,
            wake,
        });
    }
    SetWindowLongPtrW(
        hwnd,
        GWLP_WNDPROC,
        drop_wnd_proc as *const () as usize as isize,
    );
    true
}

#[cfg(windows)]
struct DropSlot {
    hwnd: isize,
    previous: isize,
    tx: Sender<Vec<String>>,
    wake: Box<dyn Fn() + Send>,
}

#[cfg(windows)]
static DROP_SLOT: std::sync::Mutex<Option<DropSlot>> = std::sync::Mutex::new(None);

#[cfg(windows)]
unsafe extern "system" fn drop_wnd_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW};
    use windows_sys::Win32::UI::WindowsAndMessaging::CallWindowProcW;
    const WM_DROPFILES: u32 = 0x0233;
    if msg == WM_DROPFILES {
        let mut paths = Vec::new();
        let drop = wparam as isize;
        let count = DragQueryFileW(drop as _, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
        for index in 0..count {
            let mut buffer = vec![0u16; 32_768];
            let len = DragQueryFileW(drop as _, index, buffer.as_mut_ptr(), buffer.len() as u32);
            if len > 0 {
                use std::os::windows::ffi::OsStringExt;
                paths.push(
                    std::path::PathBuf::from(std::ffi::OsString::from_wide(
                        &buffer[..len as usize],
                    ))
                    .to_string_lossy()
                    .into_owned(),
                );
            }
        }
        DragFinish(drop as _);
        if !paths.is_empty() {
            let mut wake = None;
            if let Ok(slot) = DROP_SLOT.lock() {
                if let Some(slot) = slot.as_ref() {
                    if slot.hwnd == hwnd as isize {
                        let _ = slot.tx.send(paths);
                        wake = Some(());
                    }
                }
            }
            if wake.is_some() {
                if let Ok(slot) = DROP_SLOT.lock() {
                    if let Some(slot) = slot.as_ref() {
                        (slot.wake)();
                    }
                }
            }
        }
        return 0;
    }
    let previous = DROP_SLOT
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|item| item.previous))
        .unwrap_or(0);
    CallWindowProcW(
        std::mem::transmute::<isize, windows_sys::Win32::UI::WindowsAndMessaging::WNDPROC>(
            previous,
        ),
        hwnd,
        msg,
        wparam,
        lparam,
    )
}
