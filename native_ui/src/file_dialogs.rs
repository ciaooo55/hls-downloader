#[cfg(windows)]
pub fn files(owner: Option<i64>, save: bool) -> Result<Vec<String>, String> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut path = vec![0u16; 65_536];
    let filter: Vec<u16> = if save { "任务列表 (*.json)\0*.json\0文本链接 (*.txt)\0*.txt\0\0" } else { "支持的文件\0*.torrent;*.magnet;*.url;*.txt;*.json;*.metalink;*.meta4;*.mp4;*.mkv;*.m3u8;*.zip\0所有文件\0*.*\0\0" }.encode_utf16().collect();
    let extension: Vec<u16> = "json\0".encode_utf16().collect();
    // SAFETY: `OPENFILENAMEW` is a `#[repr(C)]` struct of plain integers and pointers;
    // all-zero bits represent null pointers and zero flags prior to field initialization.
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner.map_or(std::ptr::null_mut(), |handle| handle as _);
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = path.as_mut_ptr();
    dialog.nMaxFile = path.len() as u32;
    dialog.lpstrDefExt = if save {
        extension.as_ptr()
    } else {
        std::ptr::null()
    };
    dialog.Flags = OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_ALLOWMULTISELECT | OFN_FILEMUSTEXIST
        };
    // SAFETY: `dialog.lStructSize` matches `OPENFILENAMEW`, `filter` is double-NUL-terminated,
    // `extension` is NUL-terminated, and `path` is a writable `nMaxFile`-unit UTF-16 buffer;
    // all pointed-to buffers outlive the synchronous common-dialog call.
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if ok == 0 {
        // SAFETY: `CommDlgExtendedError` reads thread-local common-dialog status with no pointer arguments.
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(Vec::new())
        } else {
            Err(format!("文件选择器失败：{error:#x}"))
        };
    }
    let parts: Vec<String> = path
        .split(|unit| *unit == 0)
        .take_while(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect();
    if parts.len() <= 1 {
        return Ok(parts);
    }
    Ok(parts[1..]
        .iter()
        .map(|name| {
            std::path::Path::new(&parts[0])
                .join(name)
                .to_string_lossy()
                .into_owned()
        })
        .collect())
}

#[cfg(not(windows))]
pub fn files(_owner: Option<i64>, _save: bool) -> Result<Vec<String>, String> {
    Err("文件选择器需要 Windows".into())
}

#[cfg(any(windows, test))]
const HRESULT_CANCELLED: i32 = 0x8007_04C7u32 as i32; // HRESULT_FROM_WIN32(ERROR_CANCELLED)

#[cfg(any(windows, test))]
fn is_cancelled_hresult(hr: i32) -> bool {
    hr == HRESULT_CANCELLED
}

#[cfg(any(windows, test))]
const RPC_E_CHANGED_MODE_HR: i32 = 0x8001_0106u32 as i32;

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComApartmentMode {
    /// `CoInitializeEx` returned `S_OK` or `S_FALSE` (`hr >= 0`); the caller incremented
    /// the current STA's initialization count and MUST balance it with `CoUninitialize`.
    StaInitialized,
    /// The thread is already initialized as MTA (`RPC_E_CHANGED_MODE`); the caller MUST NOT
    /// call `CoUninitialize` and must skip STA-only `IFileOpenDialog`.
    ExistingMta,
}

#[cfg(any(windows, test))]
fn com_apartment_disposition(hr: i32) -> Result<ComApartmentMode, i32> {
    if hr >= 0 {
        Ok(ComApartmentMode::StaInitialized)
    } else if hr == RPC_E_CHANGED_MODE_HR {
        Ok(ComApartmentMode::ExistingMta)
    } else {
        Err(hr)
    }
}

/// Converts a NUL-terminated UTF-16 pointer into a `String`, bounded to 32,768 code units.
///
/// # Safety
/// `ptr` must either be null or be properly aligned for `u16` and valid for reads up to and
/// including the first NUL code unit (or `32_768` `u16` elements if no NUL occurs earlier)
/// within a single allocated object.
#[cfg(any(windows, test))]
unsafe fn wide_nul_to_string(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    while len < 32_768 && *ptr.add(len) != 0 {
        len += 1;
    }
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(
        ptr, len,
    )))
}

#[cfg(windows)]
mod modern_folder_dialog {
    use super::{is_cancelled_hresult, wide_nul_to_string};
    use core::ffi::c_void;
    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::{
        Foundation::HWND,
        System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER},
        UI::Shell::{
            FileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
            SIGDN_FILESYSPATH,
        },
    };

    // IID_IFileOpenDialog = {d57c7288-d4ad-4768-be02-9d969532d960}
    pub const IID_IFILE_OPEN_DIALOG: GUID =
        GUID::from_u128(0xd57c_7288_d4ad_4768_be02_9d96_9532_d960);

    #[repr(C)]
    pub struct IShellItemRaw {
        pub vtbl: *const IShellItemVtbl,
    }

    #[repr(C)]
    pub struct IShellItemVtbl {
        pub query_interface:
            unsafe extern "system" fn(*mut IShellItemRaw, *const GUID, *mut *mut c_void) -> HRESULT,
        pub add_ref: unsafe extern "system" fn(*mut IShellItemRaw) -> u32,
        pub release: unsafe extern "system" fn(*mut IShellItemRaw) -> u32,
        pub bind_to_handler: *const c_void,
        pub get_parent: *const c_void,
        pub get_display_name:
            unsafe extern "system" fn(*mut IShellItemRaw, i32, *mut *mut u16) -> HRESULT,
    }

    #[repr(C)]
    pub struct IFileOpenDialogRaw {
        pub vtbl: *const IFileDialogVtbl,
    }

    // IFileOpenDialog inherits directly from IFileDialog (slots 0..=20 match IFileDialog).
    #[repr(C)]
    pub struct IFileDialogVtbl {
        pub query_interface: unsafe extern "system" fn(
            *mut IFileOpenDialogRaw,
            *const GUID,
            *mut *mut c_void,
        ) -> HRESULT,
        pub add_ref: unsafe extern "system" fn(*mut IFileOpenDialogRaw) -> u32,
        pub release: unsafe extern "system" fn(*mut IFileOpenDialogRaw) -> u32,
        pub show: unsafe extern "system" fn(*mut IFileOpenDialogRaw, HWND) -> HRESULT,
        pub set_file_types: *const c_void,
        pub set_file_type_index: *const c_void,
        pub get_file_type_index: *const c_void,
        pub advise: *const c_void,
        pub unadvise: *const c_void,
        pub set_options: unsafe extern "system" fn(*mut IFileOpenDialogRaw, u32) -> HRESULT,
        pub get_options: unsafe extern "system" fn(*mut IFileOpenDialogRaw, *mut u32) -> HRESULT,
        pub set_default_folder: *const c_void,
        pub set_folder: *const c_void,
        pub get_folder: *const c_void,
        pub get_current_selection: *const c_void,
        pub set_file_name: *const c_void,
        pub get_file_name: *const c_void,
        pub set_title: unsafe extern "system" fn(*mut IFileOpenDialogRaw, *const u16) -> HRESULT,
        pub set_ok_button_label: *const c_void,
        pub set_file_name_label: *const c_void,
        pub get_result:
            unsafe extern "system" fn(*mut IFileOpenDialogRaw, *mut *mut IShellItemRaw) -> HRESULT,
    }

    struct DialogGuard(*mut IFileOpenDialogRaw);
    impl Drop for DialogGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` is a non-null `IFileOpenDialog` COM interface pointer owned
                // exclusively by this guard after `CoCreateInstance` succeeded.
                unsafe {
                    ((*(*self.0).vtbl).release)(self.0);
                }
            }
        }
    }

    struct ShellItemGuard(*mut IShellItemRaw);
    impl Drop for ShellItemGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` is a non-null `IShellItem` COM interface pointer returned by
                // `IFileDialog::GetResult` and owned exclusively by this guard.
                unsafe {
                    ((*(*self.0).vtbl).release)(self.0);
                }
            }
        }
    }

    struct CoTaskMemGuard(*mut u16);
    impl Drop for CoTaskMemGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was allocated by `IShellItem::GetDisplayName` via the COM task
                // allocator (`CoTaskMemAlloc`) and ownership was transferred to the caller.
                unsafe {
                    CoTaskMemFree(self.0.cast());
                }
            }
        }
    }

    /// Returns `None` only when `CoCreateInstance` fails so callers can fall back to
    /// `SHBrowseForFolderW` on exotic environments.
    pub fn pick_folder(owner: Option<i64>) -> Option<Result<Option<String>, String>> {
        let mut raw: *mut c_void = std::ptr::null_mut();
        // SAFETY: `folder()` verified the current thread is in an STA; `FileOpenDialog` and
        // `IID_IFILE_OPEN_DIALOG` are valid static GUIDs and `raw` is a valid out-pointer.
        let created = unsafe {
            CoCreateInstance(
                &FileOpenDialog,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_IFILE_OPEN_DIALOG,
                &mut raw,
            )
        };
        if created < 0 || raw.is_null() {
            return None;
        }
        let dialog = DialogGuard(raw.cast());
        // SAFETY: `CoCreateInstance` succeeded with a non-null `IFileOpenDialog` pointer whose
        // vtable layout matches `IFileDialogVtbl` (verified by `offset_of!` unit tests).
        let vtbl = unsafe { &*(*dialog.0).vtbl };
        let mut options: u32 = 0;
        // SAFETY: `dialog.0` is a live `IFileOpenDialog` instance and `options` is a valid out-pointer.
        let hr = unsafe { (vtbl.get_options)(dialog.0, &mut options) };
        if hr < 0 {
            return Some(Err(format!("无法配置文件夹选择器：{hr:#x}")));
        }
        let desired = options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
        // SAFETY: `dialog.0` is a live `IFileOpenDialog` instance and `desired` holds valid `FILEOPENDIALOGOPTIONS` flags.
        let hr = unsafe { (vtbl.set_options)(dialog.0, desired) };
        if hr < 0 {
            return Some(Err(format!("无法启用文件夹选择模式：{hr:#x}")));
        }
        let title: Vec<u16> = "选择下载保存位置\0".encode_utf16().collect();
        // SAFETY: `dialog.0` is valid and `title` is a NUL-terminated UTF-16 buffer outliving the call.
        let _ = unsafe { (vtbl.set_title)(dialog.0, title.as_ptr()) };

        let hwnd = owner.map_or(std::ptr::null_mut(), |handle| handle as HWND);
        // SAFETY: `dialog.0` is a live STA `IFileOpenDialog` instance; `hwnd` is null or the caller's owner window handle.
        let shown = unsafe { (vtbl.show)(dialog.0, hwnd) };
        if is_cancelled_hresult(shown) {
            return Some(Ok(None));
        }
        if shown < 0 {
            return Some(Err(format!("文件夹选择器失败：{shown:#x}")));
        }

        let mut item_raw: *mut IShellItemRaw = std::ptr::null_mut();
        // SAFETY: `Show` succeeded (`S_OK`), `dialog.0` is valid, and `item_raw` is a valid out-pointer.
        let hr = unsafe { (vtbl.get_result)(dialog.0, &mut item_raw) };
        if hr < 0 || item_raw.is_null() {
            return Some(Err(format!("无法读取选中的文件夹：{hr:#x}")));
        }
        let item = ShellItemGuard(item_raw);
        // SAFETY: `get_result` succeeded with a non-null `IShellItem` pointer whose vtable matches `IShellItemVtbl`.
        let item_vtbl = unsafe { &*(*item.0).vtbl };
        let mut wide_path: *mut u16 = std::ptr::null_mut();
        // SAFETY: `item.0` is a live `IShellItem` and `wide_path` receives a `CoTaskMem`-allocated UTF-16 string on success.
        let hr = unsafe { (item_vtbl.get_display_name)(item.0, SIGDN_FILESYSPATH, &mut wide_path) };
        if hr < 0 || wide_path.is_null() {
            return Some(Err("选中的项目不是本地文件夹".into()));
        }
        let _path_guard = CoTaskMemGuard(wide_path);
        // SAFETY: `wide_path` is a non-null, `CoTaskMem`-aligned, NUL-terminated UTF-16 string kept alive by `_path_guard`.
        match unsafe { wide_nul_to_string(wide_path) } {
            Some(path) if !path.is_empty() => Some(Ok(Some(path))),
            _ => Some(Err("选中的项目不是本地文件夹".into())),
        }
    }
}

#[cfg(windows)]
fn folder_legacy(owner: Option<i64>, changed_mode: bool) -> Result<Option<String>, String> {
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::*};
    struct PidlGuard(*mut Common::ITEMIDLIST);
    impl Drop for PidlGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` is a non-null PIDL allocated by `SHBrowseForFolderW` via the Shell task allocator.
                unsafe {
                    CoTaskMemFree(self.0.cast());
                }
            }
        }
    }
    let mut display = vec![0u16; 32_768];
    let title: Vec<u16> = "选择下载保存位置\0".encode_utf16().collect();
    let info = BROWSEINFOW {
        hwndOwner: owner.map_or(std::ptr::null_mut(), |handle| handle as _),
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: display.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        // The newer dialog requires STA. Keep the caller's existing apartment.
        ulFlags: BIF_RETURNONLYFSDIRS | if changed_mode { 0 } else { BIF_NEWDIALOGSTYLE },
        lpfn: None,
        lParam: 0,
        iImage: 0,
    };
    // SAFETY: `info` points to valid stack-owned NUL-terminated `title` and a 32,768-unit `display`
    // buffer (`>= MAX_PATH`) that outlive the synchronous `SHBrowseForFolderW` call.
    let selected = unsafe { SHBrowseForFolderW(&info) };
    if selected.is_null() {
        return Ok(None);
    }
    let _selected_guard = PidlGuard(selected);
    // SAFETY: `selected` is a non-null PIDL kept alive by `_selected_guard`, and `display` is a
    // writable UTF-16 buffer of `display.len()` code units.
    let ok = unsafe {
        SHGetPathFromIDListEx(
            selected,
            display.as_mut_ptr(),
            display.len() as u32,
            GPFIDL_DEFAULT,
        )
    };
    if ok == 0 {
        return Err("选中的项目不是本地文件夹".into());
    }
    let length = display
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(display.len());
    Ok(Some(String::from_utf16_lossy(&display[..length])))
}

#[cfg(windows)]
pub fn folder(owner: Option<i64>) -> Result<Option<String>, String> {
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    // SAFETY: Calling `CoInitializeEx` with a null reserved pointer and valid `COINIT` flags
    // initializes or queries the current thread's COM apartment.
    let initialized = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    let mode = com_apartment_disposition(initialized)
        .map_err(|hr| format!("无法初始化文件夹选择器：{hr:#x}"))?;
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: Constructed exclusively when `CoInitializeEx` returned `S_OK` or `S_FALSE`,
            // balancing one STA initialization count after all COM objects in scope have dropped.
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _apartment = (mode == ComApartmentMode::StaInitialized).then_some(Apartment);
    if mode == ComApartmentMode::StaInitialized {
        if let Some(result) = modern_folder_dialog::pick_folder(owner) {
            return result;
        }
    }
    folder_legacy(owner, mode == ComApartmentMode::ExistingMta)
}

#[cfg(not(windows))]
pub fn folder(_owner: Option<i64>) -> Result<Option<String>, String> {
    Err("文件夹选择器需要 Windows".into())
}

#[allow(dead_code)]
pub fn confirm(owner: Option<i64>, title: &str, message: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO,
        };
        let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `title` and `message` are valid NUL-terminated UTF-16 buffers outliving `MessageBoxW`;
        // `owner` is either null or the caller's window handle.
        unsafe {
            MessageBoxW(
                owner.map_or(std::ptr::null_mut(), |h| h as _),
                message.as_ptr(),
                title.as_ptr(),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            ) == IDYES
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (owner, title, message);
        false
    }
}

pub fn open(target: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        if !safe_open_target(target) {
            return Err("只能打开网页链接或现有文件夹".into());
        }
        let target: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let operation: Vec<u16> = "open\0".encode_utf16().collect();
        // SAFETY: `safe_open_target` verified `target` is an `http(s)` URL or existing absolute directory,
        // and both `operation` and `target` are NUL-terminated UTF-16 buffers outliving `ShellExecuteW`.
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as isize <= 32 {
            Err(format!("无法打开目标：{}", result as isize))
        } else {
            Ok(())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        Err("系统打开操作需要 Windows".into())
    }
}

pub fn is_safe_web_url(target: &str) -> bool {
    if target.is_empty() || target != target.trim() || target.chars().any(char::is_control) {
        return false;
    }
    if let Some((scheme, rest)) = target.split_once("://") {
        return (scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http"))
            && !rest.starts_with(['/', '?', '#'])
            && !rest.is_empty()
            && !rest.contains('\\');
    }
    false
}

#[cfg(any(windows, test))]
fn safe_open_target(target: &str) -> bool {
    if is_safe_web_url(target) {
        return true;
    }
    if target.is_empty() || target != target.trim() || target.chars().any(char::is_control) {
        return false;
    }
    let path = std::path::Path::new(target);
    path.is_absolute() && path.is_dir()
}

pub fn write_clipboard_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err("没有可复制的文本".into());
    }
    if text.contains('\0') {
        return Err("文本包含无效字符".into());
    }
    if text.len() > 2 * 1024 * 1024 {
        return Err("复制内容过长".into());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{GlobalFree, HGLOBAL, HWND},
            System::{DataExchange::*, Memory::*},
            UI::{Input::KeyboardAndMouse::GetActiveWindow, WindowsAndMessaging::*},
        };
        const CF_UNICODETEXT: u32 = 13;

        struct TempWindowGuard(HWND);
        impl Drop for TempWindowGuard {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    // SAFETY: `self.0` is a temporary message-only `STATIC` window created on this
                    // thread solely for `OpenClipboard`, destroyed after `CloseClipboard` runs.
                    unsafe {
                        DestroyWindow(self.0);
                    }
                }
            }
        }

        struct GlobalMemGuard(HGLOBAL);
        impl GlobalMemGuard {
            fn disarm(&mut self) {
                self.0 = std::ptr::null_mut();
            }
        }
        impl Drop for GlobalMemGuard {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    // SAFETY: `self.0` was allocated by `GlobalAlloc(GMEM_MOVEABLE, ...)` and is
                    // unlocked; `disarm()` clears `self.0` once `SetClipboardData` takes OS ownership.
                    unsafe {
                        GlobalFree(self.0);
                    }
                }
            }
        }

        struct ClipboardGuard;
        impl Drop for ClipboardGuard {
            fn drop(&mut self) {
                // SAFETY: Constructed only after `OpenClipboard` returned non-zero on this thread.
                unsafe {
                    CloseClipboard();
                }
            }
        }

        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let bytes = wide.len() * std::mem::size_of::<u16>();
        // SAFETY: `GetActiveWindow` queries the calling thread's active HWND with no pointer arguments.
        let active = unsafe { GetActiveWindow() };
        let mut temp_window = TempWindowGuard(std::ptr::null_mut());
        let owner = if active.is_null() {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: `"STATIC\0"` is a built-in system class and `HWND_MESSAGE` creates a
            // lightweight message-only window so `EmptyClipboard` sets a non-null clipboard owner.
            let created = unsafe {
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
            };
            temp_window.0 = created;
            created
        } else {
            active
        };
        if owner.is_null() {
            return Err("无法创建剪贴板窗口".into());
        }

        // SAFETY: `bytes` is non-zero (`wide` has at least the NUL unit) and bounded (`<= 4 MiB + 2`).
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if handle.is_null() {
            return Err("无法分配剪贴板内存".into());
        }
        let mut mem_guard = GlobalMemGuard(handle);

        // SAFETY: `handle` is a newly allocated moveable global block not yet handed to the OS.
        let pointer = unsafe { GlobalLock(handle) } as *mut u16;
        if pointer.is_null() {
            return Err("无法锁定剪贴板内存".into());
        }
        // SAFETY: `GlobalAlloc` reserved `wide.len() * size_of::<u16>()` bytes, `GlobalLock` returns
        // memory aligned at least to 8 bytes (valid for `u16`), and `wide` does not overlap `pointer`.
        unsafe {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
            GlobalUnlock(handle);
        }

        // SAFETY: `owner` is a non-null window handle owned by the calling thread.
        if unsafe { OpenClipboard(owner) } == 0 {
            return Err("剪贴板正被其他程序使用，请稍后重试".into());
        }
        let _clipboard_guard = ClipboardGuard;

        // SAFETY: The clipboard is open on this thread with a non-null `owner` HWND, and `handle`
        // is an unlocked `GMEM_MOVEABLE` block containing NUL-terminated UTF-16 data.
        let transferred =
            unsafe { EmptyClipboard() != 0 && !SetClipboardData(CF_UNICODETEXT, handle).is_null() };
        if transferred {
            // Once `SetClipboardData` succeeds, the OS owns `handle`; caller MUST NOT `GlobalFree` it.
            mem_guard.disarm();
            Ok(())
        } else {
            Err("无法复制文本到剪贴板".into())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        Err("当前系统不支持写入剪贴板".into())
    }
}

pub fn write_export(path: &std::path::Path, data: &str) -> std::io::Result<()> {
    use std::io::Write;
    // Publish from the same directory so a short write or sync failure leaves
    // the user's previous export intact. create_new never overwrites a sibling.
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("导出路径缺少文件名"))?;
    let mut temporary = name.to_os_string();
    temporary.push(format!(".{}.pending", std::process::id()));
    let temporary = path.with_file_name(temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(data.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_open_accepts_web_links_and_directories_only() {
        assert!(safe_open_target("https://addons.mozilla.org/"));
        assert!(safe_open_target(env!("CARGO_MANIFEST_DIR")));
        for target in [
            "file:///C:/app.exe",
            "shell:AppsFolder",
            "https:///path",
            "foo.cmd",
            "https://site.test/\0x",
        ] {
            assert!(!safe_open_target(target));
        }
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(!safe_open_target(source.to_str().unwrap()));
    }

    #[test]
    fn web_url_check_and_clipboard_validation_reject_unsafe_inputs() {
        assert!(is_safe_web_url("https://example.test/page?a=1"));
        assert!(is_safe_web_url("http://127.0.0.1:8080/index.html"));
        assert!(!is_safe_web_url("file:///C:/test.txt"));
        assert!(!is_safe_web_url(env!("CARGO_MANIFEST_DIR")));
        assert!(!is_safe_web_url(" https://example.test "));
        assert!(write_clipboard_text("").is_err());
        assert!(write_clipboard_text("bad\0text").is_err());
    }

    #[test]
    fn modern_folder_dialog_handles_cancel_hresult_and_utf16_paths() {
        assert!(is_cancelled_hresult(0x8007_04C7u32 as i32));
        assert!(!is_cancelled_hresult(0));
        assert!(!is_cancelled_hresult(0x8000_4005u32 as i32));
        let path: Vec<u16> = r"D:\下载\媒体".encode_utf16().chain(Some(0)).collect();
        assert_eq!(
            unsafe { wide_nul_to_string(path.as_ptr()) }.as_deref(),
            Some(r"D:\下载\媒体")
        );
        assert_eq!(unsafe { wide_nul_to_string(std::ptr::null()) }, None);
        #[cfg(windows)]
        {
            use core::mem::offset_of;
            let ptr_size = std::mem::size_of::<usize>();
            assert_eq!(
                offset_of!(modern_folder_dialog::IFileDialogVtbl, show),
                3 * ptr_size
            );
            assert_eq!(
                offset_of!(modern_folder_dialog::IFileDialogVtbl, set_options),
                9 * ptr_size
            );
            assert_eq!(
                offset_of!(modern_folder_dialog::IFileDialogVtbl, get_options),
                10 * ptr_size
            );
            assert_eq!(
                offset_of!(modern_folder_dialog::IFileDialogVtbl, set_title),
                17 * ptr_size
            );
            assert_eq!(
                offset_of!(modern_folder_dialog::IFileDialogVtbl, get_result),
                20 * ptr_size
            );
            assert_eq!(
                offset_of!(modern_folder_dialog::IShellItemVtbl, get_display_name),
                5 * ptr_size
            );
        }
    }

    #[test]
    fn com_apartment_disposition_balances_uninit_only_for_sta_success_codes() {
        // S_OK (0) and S_FALSE (1) must uninitialize COM and allow STA IFileOpenDialog.
        assert_eq!(
            com_apartment_disposition(0),
            Ok(ComApartmentMode::StaInitialized)
        );
        assert_eq!(
            com_apartment_disposition(1),
            Ok(ComApartmentMode::StaInitialized)
        );
        // RPC_E_CHANGED_MODE (0x80010106) must NOT call CoUninitialize or STA-only IFileOpenDialog.
        assert_eq!(
            com_apartment_disposition(0x8001_0106u32 as i32),
            Ok(ComApartmentMode::ExistingMta)
        );
        // Hard COM initialization failures propagate as Err without calling CoUninitialize.
        let e_outofmemory = 0x8007_000Eu32 as i32;
        assert_eq!(com_apartment_disposition(e_outofmemory), Err(e_outofmemory));
    }
}
