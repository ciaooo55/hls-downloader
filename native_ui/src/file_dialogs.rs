#[cfg(windows)]
pub fn files(owner: Option<i64>, save: bool) -> Result<Vec<String>, String> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut path = vec![0u16; 65_536];
    let filter: Vec<u16> = if save { "任务列表 (*.json)\0*.json\0文本链接 (*.txt)\0*.txt\0\0" } else { "支持的文件\0*.torrent;*.magnet;*.url;*.txt;*.json;*.metalink;*.meta4;*.mp4;*.mkv;*.m3u8;*.zip\0所有文件\0*.*\0\0" }.encode_utf16().collect();
    let extension: Vec<u16> = "json\0".encode_utf16().collect();
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
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if ok == 0 {
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
        let vtbl = unsafe { &*(*dialog.0).vtbl };
        let mut options: u32 = 0;
        let hr = unsafe { (vtbl.get_options)(dialog.0, &mut options) };
        if hr < 0 {
            return Some(Err(format!("无法配置文件夹选择器：{hr:#x}")));
        }
        let desired = options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
        let hr = unsafe { (vtbl.set_options)(dialog.0, desired) };
        if hr < 0 {
            return Some(Err(format!("无法启用文件夹选择模式：{hr:#x}")));
        }
        let title: Vec<u16> = "选择下载保存位置\0".encode_utf16().collect();
        let _ = unsafe { (vtbl.set_title)(dialog.0, title.as_ptr()) };

        let hwnd = owner.map_or(std::ptr::null_mut(), |handle| handle as HWND);
        let shown = unsafe { (vtbl.show)(dialog.0, hwnd) };
        if is_cancelled_hresult(shown) {
            return Some(Ok(None));
        }
        if shown < 0 {
            return Some(Err(format!("文件夹选择器失败：{shown:#x}")));
        }

        let mut item_raw: *mut IShellItemRaw = std::ptr::null_mut();
        let hr = unsafe { (vtbl.get_result)(dialog.0, &mut item_raw) };
        if hr < 0 || item_raw.is_null() {
            return Some(Err(format!("无法读取选中的文件夹：{hr:#x}")));
        }
        let item = ShellItemGuard(item_raw);
        let item_vtbl = unsafe { &*(*item.0).vtbl };
        let mut wide_path: *mut u16 = std::ptr::null_mut();
        let hr = unsafe { (item_vtbl.get_display_name)(item.0, SIGDN_FILESYSPATH, &mut wide_path) };
        if hr < 0 || wide_path.is_null() {
            return Some(Err("选中的项目不是本地文件夹".into()));
        }
        let _path_guard = CoTaskMemGuard(wide_path);
        match unsafe { wide_nul_to_string(wide_path) } {
            Some(path) if !path.is_empty() => Some(Ok(Some(path))),
            _ => Some(Err("选中的项目不是本地文件夹".into())),
        }
    }
}

#[cfg(windows)]
fn folder_legacy(owner: Option<i64>, changed_mode: bool) -> Result<Option<String>, String> {
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::*};
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
    let selected = unsafe { SHBrowseForFolderW(&info) };
    if selected.is_null() {
        return Ok(None);
    }
    let ok = unsafe {
        SHGetPathFromIDListEx(
            selected,
            display.as_mut_ptr(),
            display.len() as u32,
            GPFIDL_DEFAULT,
        )
    };
    unsafe { CoTaskMemFree(selected as _) };
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
    use windows_sys::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        },
    };
    let initialized = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if initialized < 0 && initialized != RPC_E_CHANGED_MODE {
        return Err(format!("无法初始化文件夹选择器：{initialized:#x}"));
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _apartment = if initialized >= 0 {
        Some(Apartment)
    } else {
        None
    };
    if let Some(result) = modern_folder_dialog::pick_folder(owner) {
        return result;
    }
    folder_legacy(owner, initialized == RPC_E_CHANGED_MODE)
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
    unsafe {
        use windows_sys::Win32::{
            Foundation::GlobalFree,
            System::{DataExchange::*, Memory::*},
            UI::{Input::KeyboardAndMouse::GetActiveWindow, WindowsAndMessaging::*},
        };
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let bytes = wide.len() * 2;
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
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if handle.is_null() {
                return Err("无法分配剪贴板内存".into());
            }
            let pointer = GlobalLock(handle) as *mut u16;
            if pointer.is_null() {
                GlobalFree(handle);
                return Err("无法锁定剪贴板内存".into());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
            GlobalUnlock(handle);
            if OpenClipboard(owner) == 0 {
                GlobalFree(handle);
                return Err("剪贴板正被其他程序使用，请稍后重试".into());
            }
            let written = EmptyClipboard() != 0 && !SetClipboardData(13, handle).is_null();
            CloseClipboard();
            if written {
                Ok(())
            } else {
                GlobalFree(handle);
                Err("无法复制文本到剪贴板".into())
            }
        })();
        if active.is_null() {
            DestroyWindow(owner);
        }
        result
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
}
