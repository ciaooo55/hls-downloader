//! Windows file clipboard and OLE drag-out, shared by UI and Core clients.
//! Drag-out must run on the UI thread while the initiating button is held.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum ClipboardContent {
    Empty,
    Text(String),
    Files(Vec<String>),
}

/// Reads one clipboard snapshot. Explorer file selections take precedence over text.
pub fn read_clipboard() -> Result<ClipboardContent, String> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            System::{DataExchange::*, Memory::*},
            UI::Shell::DragQueryFileW,
        };
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("剪贴板正被其他程序使用，请稍后重试".into());
        }
        struct Clipboard;
        impl Drop for Clipboard {
            fn drop(&mut self) {
                unsafe {
                    CloseClipboard();
                }
            }
        }
        let _clipboard = Clipboard;
        if IsClipboardFormatAvailable(15) != 0 {
            // CF_HDROP; clipboard owns this handle.
            let handle = GetClipboardData(15);
            if handle.is_null() {
                return Err("无法读取剪贴板文件".into());
            }
            let count = DragQueryFileW(handle, u32::MAX, std::ptr::null_mut(), 0);
            if count > 4096 {
                return Err("一次最多粘贴 4096 个文件，请分批导入".into());
            }
            let mut paths = Vec::with_capacity(count as usize);
            for index in 0..count {
                let length = DragQueryFileW(handle, index, std::ptr::null_mut(), 0);
                if length == 0 || length > 32767 {
                    return Err(format!("第 {} 个文件路径无效", index + 1));
                }
                let mut path = vec![0u16; length as usize + 1];
                if DragQueryFileW(handle, index, path.as_mut_ptr(), length + 1) != length {
                    return Err(format!("无法读取第 {} 个文件路径", index + 1));
                }
                paths.push(
                    String::from_utf16(&path[..length as usize])
                        .map_err(|_| "文件路径不是有效的 Unicode")?,
                );
            }
            return Ok(ClipboardContent::Files(paths));
        }
        if IsClipboardFormatAvailable(13) == 0 {
            return Ok(ClipboardContent::Empty);
        }
        let handle = GetClipboardData(13); // CF_UNICODETEXT
        if handle.is_null() {
            return Err("无法读取剪贴板文本".into());
        }
        let capacity = GlobalSize(handle) / 2;
        if capacity > 2 * 1024 * 1024 {
            return Err("剪贴板文本过长，请分批导入".into());
        }
        let pointer = GlobalLock(handle) as *const u16;
        if pointer.is_null() {
            return Err("无法读取剪贴板文本".into());
        }
        let units = std::slice::from_raw_parts(pointer, capacity);
        let result = units
            .iter()
            .position(|unit| *unit == 0)
            .ok_or_else(|| "剪贴板文本格式无效".to_owned())
            .and_then(|length| {
                String::from_utf16(&units[..length])
                    .map_err(|_| "剪贴板文本不是有效的 Unicode".into())
            });
        GlobalUnlock(handle);
        result.map(ClipboardContent::Text)
    }
    #[cfg(not(windows))]
    {
        Err("当前系统不支持读取剪贴板".into())
    }
}

/// Copies the complete selection in one CF_HDROP operation. Source files stay intact.
pub fn write_files(paths: &[PathBuf]) -> Result<(), String> {
    let bytes = hdrop_bytes(paths)?;
    for path in paths {
        if !path.exists() {
            return Err(format!("文件不存在：{}", path.display()));
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::GlobalFree,
            System::{DataExchange::*, Memory::*},
            UI::{Input::KeyboardAndMouse::GetActiveWindow, WindowsAndMessaging::*},
        };
        // EmptyClipboard with a NULL owner makes SetClipboardData fail. Core's
        // command thread has no window, so give it a temporary message-only owner.
        let active = GetActiveWindow();
        let owner = if active.is_null() {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } else {
            active
        };
        if owner.is_null() {
            return Err("无法创建剪贴板窗口".into());
        }
        let result = (|| {
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if handle.is_null() {
                return Err("无法分配剪贴板内存".into());
            }
            let pointer = GlobalLock(handle) as *mut u8;
            if pointer.is_null() {
                GlobalFree(handle);
                return Err("无法锁定剪贴板内存".into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            GlobalUnlock(handle);
            if OpenClipboard(owner) == 0 {
                GlobalFree(handle);
                return Err("剪贴板正被其他程序使用，请稍后重试".into());
            }
            let written = EmptyClipboard() != 0 && !SetClipboardData(15, handle).is_null();
            CloseClipboard();
            if written {
                Ok(())
            } else {
                GlobalFree(handle);
                Err("无法复制文件到剪贴板".into())
            }
        })();
        if active.is_null() {
            DestroyWindow(owner);
        }
        result
    }
    #[cfg(not(windows))]
    {
        let _ = bytes;
        Err("当前系统不支持复制文件到剪贴板".into())
    }
}

pub fn hdrop_bytes(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
    if paths.is_empty() {
        return Err("没有可拖出的文件".into());
    }
    let mut wide: Vec<u16> = Vec::new();
    for path in paths {
        if !path.is_absolute() {
            return Err("拖出路径必须是绝对路径".into());
        }
        let path = path.to_str().ok_or("文件路径不是有效的 Unicode")?;
        if path.contains('\0') {
            return Err("文件路径包含无效字符".into());
        }
        wide.extend(path.encode_utf16());
        wide.push(0);
    }
    wide.push(0);
    let header = 20usize;
    let mut bytes = vec![0u8; header + wide.len() * 2];
    bytes[..4].copy_from_slice(&(header as u32).to_le_bytes());
    bytes[16] = 1;
    let payload = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    bytes[header..].copy_from_slice(payload);
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    Dropped,
    Canceled,
}

pub fn begin_file_drag(paths: &[PathBuf]) -> Result<DragOutcome, String> {
    #[cfg(windows)]
    if unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(1) } >= 0 {
        return Err("请按住“拖出文件”的鼠标左键，拖到目标文件夹".into());
    }
    write_files(paths)?;
    #[cfg(windows)]
    {
        windows_drag_from_clipboard()
    }
    #[cfg(not(windows))]
    {
        Err("当前系统不支持文件拖出".into())
    }
}

pub fn completed_file_drag(path: &Path) -> Result<DragOutcome, String> {
    if !path.exists() {
        return Err("文件不存在，无法拖出".into());
    }
    begin_file_drag(&[path.to_path_buf()])
}

#[cfg(windows)]
fn drag_drop_result(hr: i32, effect: u32) -> Result<DragOutcome, String> {
    match hr {
        0 | 0x0004_0100 if effect != 0 => Ok(DragOutcome::Dropped),
        0 | 0x0004_0100 | 0x0004_0101 => Ok(DragOutcome::Canceled),
        _ => Err(format!("DoDragDrop {hr:#x}")),
    }
}

#[cfg(windows)]
fn windows_drag_from_clipboard() -> Result<DragOutcome, String> {
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn OleInitialize(reserved: *mut core::ffi::c_void) -> i32;
        fn OleUninitialize();
        fn OleGetClipboard(ppdataobj: *mut *mut core::ffi::c_void) -> i32;
        fn DoDragDrop(
            pdataobj: *mut core::ffi::c_void,
            pdropsource: *mut core::ffi::c_void,
            dwokeffects: u32,
            pdweffect: *mut u32,
        ) -> i32;
    }
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_LINK: u32 = 4;
    unsafe {
        let hr = OleInitialize(std::ptr::null_mut());
        if hr < 0 && hr != 1 {
            return Err(format!("OleInitialize {hr:#x}"));
        }
        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        let got = OleGetClipboard(&mut data);
        if got < 0 || data.is_null() {
            OleUninitialize();
            return Err("剪贴板没有可拖出的文件".into());
        }
        let source = Box::into_raw(Box::new(DropSource::new()));
        let mut effect: u32 = 0;
        let dragged = DoDragDrop(
            data,
            source as *mut core::ffi::c_void,
            DROPEFFECT_COPY | DROPEFFECT_LINK,
            &mut effect,
        );
        drop_release(source);
        let vtbl = *(data as *mut *const usize);
        let release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 =
            std::mem::transmute(*vtbl.add(2));
        release(data);
        OleUninitialize();
        drag_drop_result(dragged, effect)
    }
}

#[cfg(windows)]
#[repr(C)]
struct DropSource {
    vtbl: *const DropSourceVtbl,
    refs: std::sync::atomic::AtomicU32,
}

#[cfg(windows)]
#[repr(C)]
struct DropSourceVtbl {
    query_interface: unsafe extern "system" fn(
        this: *mut DropSource,
        iid: *const u8,
        out: *mut *mut core::ffi::c_void,
    ) -> i32,
    add_ref: unsafe extern "system" fn(this: *mut DropSource) -> u32,
    release: unsafe extern "system" fn(this: *mut DropSource) -> u32,
    query_continue_drag:
        unsafe extern "system" fn(this: *mut DropSource, escape: i32, key_state: u32) -> i32,
    give_feedback: unsafe extern "system" fn(this: *mut DropSource, effect: u32) -> i32,
}

#[cfg(windows)]
static DROP_SOURCE_VTBL: DropSourceVtbl = DropSourceVtbl {
    query_interface: drop_query_interface,
    add_ref: drop_add_ref,
    release: drop_release,
    query_continue_drag: drop_query_continue,
    give_feedback: drop_give_feedback,
};

#[cfg(windows)]
impl DropSource {
    fn new() -> Self {
        Self {
            vtbl: &DROP_SOURCE_VTBL,
            refs: std::sync::atomic::AtomicU32::new(1),
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn drop_query_interface(
    this: *mut DropSource,
    iid: *const u8,
    out: *mut *mut core::ffi::c_void,
) -> i32 {
    const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
    if this.is_null() || iid.is_null() || out.is_null() {
        return E_NOINTERFACE;
    }
    if iid_is(iid, &IID_IUNKNOWN) || iid_is(iid, &IID_IDROPSOURCE) {
        *out = this as *mut core::ffi::c_void;
        drop_add_ref(this);
        return 0;
    }
    *out = std::ptr::null_mut();
    E_NOINTERFACE
}

#[cfg(windows)]
const IID_IUNKNOWN: [u8; 16] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

#[cfg(windows)]
const IID_IDROPSOURCE: [u8; 16] = [
    0x21, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

#[cfg(windows)]
fn iid_is(iid: *const u8, expected: &[u8; 16]) -> bool {
    unsafe { std::slice::from_raw_parts(iid, 16) == expected }
}

#[cfg(windows)]
unsafe extern "system" fn drop_add_ref(this: *mut DropSource) -> u32 {
    (*this)
        .refs
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1
}

#[cfg(windows)]
unsafe extern "system" fn drop_release(this: *mut DropSource) -> u32 {
    if this.is_null() {
        return 0;
    }
    let previous = (*this)
        .refs
        .fetch_sub(1, std::sync::atomic::Ordering::Release);
    if previous == 1 {
        drop(Box::from_raw(this));
        0
    } else {
        previous.saturating_sub(1)
    }
}

#[cfg(windows)]
unsafe extern "system" fn drop_query_continue(
    _this: *mut DropSource,
    escape: i32,
    key_state: u32,
) -> i32 {
    const DRAGDROP_S_DROP: i32 = 0x0004_0100;
    const DRAGDROP_S_CANCEL: i32 = 0x0004_0101;
    const MK_LBUTTON: u32 = 0x0001;
    if escape != 0 {
        return DRAGDROP_S_CANCEL;
    }
    if key_state & MK_LBUTTON == 0 {
        return DRAGDROP_S_DROP;
    }
    0
}

#[cfg(windows)]
unsafe extern "system" fn drop_give_feedback(_this: *mut DropSource, _effect: u32) -> i32 {
    const DRAGDROP_S_USEDEFAULTCURSORS: i32 = 0x0004_0102;
    DRAGDROP_S_USEDEFAULTCURSORS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdrop_is_unicode_dropfiles() {
        let path = std::env::temp_dir().join("下载 文件😀.bin");
        let bytes = hdrop_bytes(std::slice::from_ref(&path)).unwrap();
        assert_eq!(&bytes[..4], &20u32.to_le_bytes());
        assert_eq!(bytes[16], 1);
        #[cfg(windows)]
        {
            assert!(iid_is(IID_IUNKNOWN.as_ptr(), &IID_IUNKNOWN));
            assert!(iid_is(IID_IDROPSOURCE.as_ptr(), &IID_IDROPSOURCE));
            assert!(!iid_is(IID_IUNKNOWN.as_ptr(), &IID_IDROPSOURCE));
            assert_eq!(
                drag_drop_result(0x0004_0100, 1).unwrap(),
                DragOutcome::Dropped
            );
            assert_eq!(
                drag_drop_result(0x0004_0100, 0).unwrap(),
                DragOutcome::Canceled
            );
            assert_eq!(
                drag_drop_result(0x0004_0101, 0).unwrap(),
                DragOutcome::Canceled
            );
            assert!(drag_drop_result(0x8000_4005u32 as i32, 0).is_err());
        }
        let wide: Vec<u16> = bytes[20..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16_lossy(&wide);
        assert!(text.contains(&path.to_string_lossy().to_string()));
        assert!(hdrop_bytes(&[]).is_err());
    }

    #[test]
    fn hdrop_preserves_a_multi_file_selection_and_rejects_embedded_nul() {
        let root = std::env::temp_dir();
        let paths = [root.join("视频 一.mp4"), root.join("资料 二.zip")];
        let bytes = hdrop_bytes(&paths).unwrap();
        let units: Vec<_> = bytes[20..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let expected = format!("{}\0{}\0\0", paths[0].display(), paths[1].display());
        assert_eq!(String::from_utf16(&units).unwrap(), expected);
        assert!(hdrop_bytes(&[root.join("first\0second.bin")]).is_err());
        assert!(hdrop_bytes(&[PathBuf::from("relative.bin")]).is_err());
    }

    #[test]
    fn missing_files_are_rejected_before_changing_the_clipboard() {
        let missing =
            std::env::temp_dir().join(format!("hls-missing-selection-{}", std::process::id()));
        assert!(!missing.exists());
        assert!(write_files(&[missing]).unwrap_err().contains("文件不存在"));
    }
}
