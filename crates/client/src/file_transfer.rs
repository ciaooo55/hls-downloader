//! Windows file clipboard and OLE drag-out, shared by UI and Core clients.
//! Drag-out must run on the UI thread while the initiating button is held.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum ClipboardContent {
    Empty,
    Text(String),
    Files(Vec<String>),
}

// Bound the entire selection as well as each path before allocating its buffer.
const MAX_SELECTION_UNITS: usize = 2 * 1024 * 1024;

fn add_path_units(total: usize, length: usize) -> Result<usize, String> {
    if length == 0 || length > 32767 {
        return Err("文件路径长度无效".into());
    }
    let next = total + length + 1;
    if next > MAX_SELECTION_UNITS {
        return Err("所选文件路径总长度过大，请分批操作".into());
    }
    Ok(next)
}

/// Reads one clipboard snapshot. Explorer file selections take precedence over text.
pub fn read_clipboard() -> Result<ClipboardContent, String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::HANDLE,
            System::{DataExchange::*, Memory::*},
            UI::Shell::DragQueryFileW,
        };
        // SAFETY: Passing a null `HWND` opens the clipboard associated with the current task; if
        // `OpenClipboard` succeeds, `Clipboard` guarantees `CloseClipboard` runs once on exit.
        let opened = unsafe { OpenClipboard(std::ptr::null_mut()) };
        if opened == 0 {
            return Err("剪贴板正被其他程序使用，请稍后重试".into());
        }
        struct Clipboard;
        impl Drop for Clipboard {
            fn drop(&mut self) {
                // SAFETY: `Clipboard` is constructed only after `OpenClipboard` returned non-zero
                // on this thread and closes the clipboard exactly once on drop.
                unsafe {
                    CloseClipboard();
                }
            }
        }
        let _clipboard = Clipboard;
        // SAFETY: Clipboard is open on this thread; `15` is `CF_HDROP`.
        let has_files = unsafe { IsClipboardFormatAvailable(15) } != 0;
        if has_files {
            // CF_HDROP; clipboard owns this handle while open.
            // SAFETY: Clipboard is held open by `_clipboard`; the returned `HDROP` handle is owned
            // by the clipboard and remains valid until `_clipboard` drops.
            let handle = unsafe { GetClipboardData(15) };
            if handle.is_null() {
                return Err("无法读取剪贴板文件".into());
            }
            // SAFETY: `handle` is a valid non-null `HDROP` owned by the open clipboard; `u32::MAX`
            // with a null buffer queries the file count without writing to any buffer.
            let count = unsafe { DragQueryFileW(handle, u32::MAX, std::ptr::null_mut(), 0) };
            if count > 4096 {
                return Err("一次最多粘贴 4096 个文件，请分批导入".into());
            }
            let mut paths = Vec::with_capacity(count as usize);
            let mut total = 1;
            for index in 0..count {
                // SAFETY: `index < count`, `handle` is valid while `_clipboard` is live, and a
                // null output pointer with `cch == 0` queries the required character count.
                let length = unsafe { DragQueryFileW(handle, index, std::ptr::null_mut(), 0) };
                total = add_path_units(total, length as usize)?;
                let mut path = vec![0u16; length as usize + 1];
                // SAFETY: `path` has `length + 1` writable `u16` elements for `length` characters
                // plus the terminating NUL written by `DragQueryFileW`.
                let copied =
                    unsafe { DragQueryFileW(handle, index, path.as_mut_ptr(), length + 1) };
                if copied != length {
                    return Err(format!("无法读取第 {} 个文件路径", index + 1));
                }
                paths.push(
                    String::from_utf16(&path[..length as usize])
                        .map_err(|_| "文件路径不是有效的 Unicode")?,
                );
            }
            return Ok(ClipboardContent::Files(paths));
        }
        // SAFETY: Clipboard is open on this thread; `13` is `CF_UNICODETEXT`.
        let has_text = unsafe { IsClipboardFormatAvailable(13) } != 0;
        if !has_text {
            return Ok(ClipboardContent::Empty);
        }
        // SAFETY: Clipboard is held open by `_clipboard`; `GetClipboardData(CF_UNICODETEXT)`
        // returns a clipboard-owned `HGLOBAL` (or null on failure).
        let handle = unsafe { GetClipboardData(13) };
        if handle.is_null() {
            return Err("无法读取剪贴板文本".into());
        }
        // SAFETY: `handle` is a non-null `HGLOBAL` owned by the open clipboard.
        let capacity = unsafe { GlobalSize(handle) } / 2;
        if capacity > 2 * 1024 * 1024 {
            return Err("剪贴板文本过长，请分批导入".into());
        }
        // SAFETY: `GlobalLock` locks the clipboard `HGLOBAL` and returns an aligned pointer valid
        // for `GlobalSize(handle)` bytes (`capacity` `u16` units) until `GlobalUnlock`.
        let pointer = unsafe { GlobalLock(handle) } as *const u16;
        if pointer.is_null() {
            return Err("无法读取剪贴板文本".into());
        }
        struct GlobalLockGuard(HANDLE);
        impl Drop for GlobalLockGuard {
            fn drop(&mut self) {
                // SAFETY: `GlobalLock` succeeded on `self.0` above; `GlobalUnlock` balances that
                // lock count before `_clipboard` closes the clipboard.
                unsafe {
                    GlobalUnlock(self.0);
                }
            }
        }
        let _lock_guard = GlobalLockGuard(handle);
        // SAFETY: `pointer` is non-null, aligned to `u16` (Win32 `GlobalAlloc` is 8-byte aligned),
        // and backed by at least `capacity * 2` readable bytes while `_lock_guard` and `_clipboard`
        // are alive.
        let units = unsafe { std::slice::from_raw_parts(pointer, capacity) };
        let result = units
            .iter()
            .position(|unit| *unit == 0)
            .ok_or_else(|| "剪贴板文本格式无效".to_owned())
            .and_then(|length| {
                String::from_utf16(&units[..length])
                    .map_err(|_| "剪贴板文本不是有效的 Unicode".into())
            });
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
    {
        use windows_sys::Win32::{
            Foundation::GlobalFree,
            System::{DataExchange::*, Memory::*},
            UI::{Input::KeyboardAndMouse::GetActiveWindow, WindowsAndMessaging::*},
        };
        // EmptyClipboard with a NULL owner makes SetClipboardData fail. Core's
        // command thread has no window, so give it a temporary message-only owner.
        // SAFETY: `GetActiveWindow` queries the calling thread's active window; if null,
        // `CreateWindowExW` creates a temporary `HWND_MESSAGE` `STATIC` window using a valid
        // NUL-terminated class name, destroyed before returning below.
        let (active, owner) = unsafe {
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
            (active, owner)
        };
        if owner.is_null() {
            return Err("无法创建剪贴板窗口".into());
        }
        let result = (|| {
            // SAFETY: Allocates a moveable global memory block of `bytes.len()` (> 0) bytes, locks
            // it to copy `bytes` without overlap, unlocks it, and transfers ownership to the
            // system clipboard on `SetClipboardData` success or frees it via `GlobalFree` on any
            // failure path.
            unsafe {
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
            }
        })();
        if active.is_null() {
            // SAFETY: `owner` is the temporary message-only window created on this thread above.
            unsafe {
                DestroyWindow(owner);
            }
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
    if paths.len() > 4096 {
        return Err("一次最多选择 4096 个文件，请分批操作".into());
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
        add_path_units(wide.len() + 1, path.encode_utf16().count())?;
        wide.extend(path.encode_utf16());
        wide.push(0);
    }
    wide.push(0);
    let header = 20usize;
    let mut bytes = Vec::with_capacity(header + wide.len() * 2);
    bytes.extend_from_slice(&(header as u32).to_le_bytes());
    bytes.resize(header, 0);
    bytes[16] = 1;
    for unit in wide {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    Dropped,
    Canceled,
}

pub fn begin_file_drag(paths: &[PathBuf]) -> Result<DragOutcome, String> {
    #[cfg(windows)]
    {
        // SAFETY: `GetAsyncKeyState(VK_LBUTTON == 1)` reads hardware/thread key state with no
        // pointer or handle preconditions.
        let key_state =
            unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(1) };
        if key_state >= 0 {
            return Err("请按住“拖出文件”的鼠标左键，拖到目标文件夹".into());
        }
    }
    write_files(paths)?;
    #[cfg(windows)]
    {
        windows_drag_from_clipboard(paths)
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
fn windows_drag_from_clipboard(paths: &[PathBuf]) -> Result<DragOutcome, String> {
    use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
    #[link(name = "ole32")]
    // SAFETY: Standard Win32 OLE functions exported by `ole32.dll` with `extern "system"` ABI
    // matching `<ole2.h>`.
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

    // SAFETY: `reserved` must be null per `OleInitialize` contract; any non-negative HRESULT
    // (`S_OK` or `S_FALSE`) must be balanced by exactly one `OleUninitialize` on this thread.
    let hr = unsafe { OleInitialize(std::ptr::null_mut()) };
    if hr < 0 {
        return Err(format!("OleInitialize {hr:#x}"));
    }
    struct OleGuard;
    impl Drop for OleGuard {
        fn drop(&mut self) {
            // SAFETY: Balances the successful `OleInitialize` call on this thread.
            unsafe {
                OleUninitialize();
            }
        }
    }
    let _ole = OleGuard;

    // SAFETY: `GetClipboardSequenceNumber` has no pointer preconditions.
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let selected = matches!(read_clipboard(), Ok(ClipboardContent::Files(files))
        if files.len() == paths.len() && files.iter().zip(paths).all(|(file, path)| Path::new(file) == path));
    if !selected {
        return Err("剪贴板内容已更改，请重新拖出文件".into());
    }
    let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: OLE is initialized on this thread (`_ole`) and `&mut data` is a valid out-pointer
    // receiving an `IDataObject*` reference with refcount >= 1 on success.
    let got = unsafe { OleGetClipboard(&mut data) };
    if got < 0 || data.is_null() {
        return Err("剪贴板没有可拖出的文件".into());
    }
    // SAFETY: `data` is a valid non-null COM `IDataObject` interface pointer whose first word
    // points to an `IUnknown`-prefixed vtable where slot 2 is `IUnknown::Release`.
    let release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 = unsafe {
        let vtbl = *(data as *mut *const usize);
        std::mem::transmute(*vtbl.add(2))
    };
    // SAFETY: `GetClipboardSequenceNumber` is safe to call; on sequence mismatch `release(data)`
    // releases our `OleGetClipboard` reference before `_ole` uninitializes OLE.
    let current_sequence = unsafe { GetClipboardSequenceNumber() };
    if sequence == 0 || current_sequence != sequence {
        // SAFETY: `data` holds the owned COM reference returned by `OleGetClipboard`.
        unsafe {
            release(data);
        }
        return Err("剪贴板内容已更改，请重新拖出文件".into());
    }
    let source = Box::into_raw(Box::new(DropSource::new()));
    let mut effect: u32 = 0;
    // SAFETY: `data` is a valid `IDataObject*`, `source` is a valid `#[repr(C)]` COM `IDropSource`
    // object with initial refcount 1, and `&mut effect` is a valid out-pointer. After the modal
    // `DoDragDrop` loop returns, `drop_release(source)` and `release(data)` release our references.
    let dragged = unsafe {
        let hr = DoDragDrop(
            data,
            source as *mut core::ffi::c_void,
            DROPEFFECT_COPY | DROPEFFECT_LINK,
            &mut effect,
        );
        drop_release(source);
        release(data);
        hr
    };
    drag_drop_result(dragged, effect)
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

/// # Safety
/// COM `IUnknown::QueryInterface` callback: `this` must be null or a valid `DropSource`, `iid`
/// must be null or point to a 16-byte `GUID`, and `out` must be null or a writable pointer.
#[cfg(windows)]
unsafe extern "system" fn drop_query_interface(
    this: *mut DropSource,
    iid: *const u8,
    out: *mut *mut core::ffi::c_void,
) -> i32 {
    const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
    const E_POINTER: i32 = 0x8000_4003u32 as i32;
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `out` was verified non-null above; COM requires zeroing `*out` on failure.
    unsafe {
        *out = std::ptr::null_mut();
    }
    if this.is_null() || iid.is_null() {
        return E_NOINTERFACE;
    }
    // SAFETY: `iid` is non-null and points to a 16-byte COM `IID` supplied by the caller.
    let supported = unsafe { iid_is(iid, &IID_IUNKNOWN) || iid_is(iid, &IID_IDROPSOURCE) };
    if supported {
        // SAFETY: `this` and `out` are non-null and valid; `drop_add_ref` increments the COM
        // reference count for the returned interface pointer.
        unsafe {
            *out = this as *mut core::ffi::c_void;
            drop_add_ref(this);
        }
        return 0;
    }
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

/// # Safety
/// `iid` must be non-null and valid for reads of 16 bytes (`sizeof(GUID)`).
#[cfg(windows)]
unsafe fn iid_is(iid: *const u8, expected: &[u8; 16]) -> bool {
    // SAFETY: Caller guarantees `iid` points to 16 readable `GUID` bytes.
    unsafe { std::slice::from_raw_parts(iid, 16) == expected }
}

/// # Safety
/// `this` must be null or point to a live `DropSource` allocated by `Box::into_raw`.
#[cfg(windows)]
unsafe extern "system" fn drop_add_ref(this: *mut DropSource) -> u32 {
    if this.is_null() {
        return 0;
    }
    // SAFETY: `this` is non-null and points to a live `DropSource` whose `refs` is atomic.
    unsafe {
        (*this)
            .refs
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }
}

/// # Safety
/// `this` must be null or point to a live `DropSource` allocated by `Box::into_raw` with at least
/// one outstanding reference owned by the caller.
#[cfg(windows)]
unsafe extern "system" fn drop_release(this: *mut DropSource) -> u32 {
    if this.is_null() {
        return 0;
    }
    // SAFETY: `this` is non-null and live while at least one reference remains. `Ordering::Release`
    // paired with the `Ordering::Acquire` fence before `Box::from_raw` synchronizes all prior
    // accesses across threads before deallocation (matching `std::sync::Arc::drop`).
    let previous = unsafe {
        (*this)
            .refs
            .fetch_sub(1, std::sync::atomic::Ordering::Release)
    };
    if previous == 1 {
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        // SAFETY: `previous == 1` establishes exclusive ownership of the `Box<DropSource>`
        // originally created via `Box::into_raw`.
        unsafe {
            drop(Box::from_raw(this));
        }
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
            unsafe {
                assert!(iid_is(IID_IUNKNOWN.as_ptr(), &IID_IUNKNOWN));
                assert!(iid_is(IID_IDROPSOURCE.as_ptr(), &IID_IDROPSOURCE));
                assert!(!iid_is(IID_IUNKNOWN.as_ptr(), &IID_IDROPSOURCE));

                let raw = Box::into_raw(Box::new(DropSource::new()));
                let mut queried: *mut core::ffi::c_void = std::ptr::dangling_mut();
                assert_eq!(
                    drop_query_interface(std::ptr::null_mut(), IID_IUNKNOWN.as_ptr(), &mut queried),
                    0x8000_4002u32 as i32
                );
                assert!(queried.is_null());
                assert_eq!(
                    drop_query_interface(raw, IID_IDROPSOURCE.as_ptr(), &mut queried),
                    0
                );
                assert_eq!(queried, raw as *mut core::ffi::c_void);
                assert_eq!(drop_release(raw), 1);
                assert_eq!(drop_release(raw), 0);
            }
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
    fn clipboard_file_selection_budget_includes_terminators() {
        assert_eq!(
            add_path_units(MAX_SELECTION_UNITS - 3, 2).unwrap(),
            MAX_SELECTION_UNITS
        );
        assert!(add_path_units(MAX_SELECTION_UNITS - 2, 2).is_err());
        assert!(add_path_units(1, 32768).is_err());
    }

    #[test]
    fn missing_files_are_rejected_before_changing_the_clipboard() {
        let missing =
            std::env::temp_dir().join(format!("hls-missing-selection-{}", std::process::id()));
        assert!(!missing.exists());
        assert!(write_files(&[missing]).unwrap_err().contains("文件不存在"));
    }
}
