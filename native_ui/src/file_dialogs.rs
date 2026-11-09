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

#[cfg(windows)]
pub fn folder(owner: Option<i64>) -> Result<Option<String>, String> {
    use windows_sys::Win32::{
        System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED},
        UI::Shell::*,
    };
    let initialized = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    if initialized < 0 {
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
    let _apartment = Apartment;
    let mut display = vec![0u16; 32_768];
    let title: Vec<u16> = "选择下载保存位置\0".encode_utf16().collect();
    let info = BROWSEINFOW {
        hwndOwner: owner.map_or(std::ptr::null_mut(), |handle| handle as _),
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: display.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        lpfn: None,
        lParam: 0,
        iImage: 0,
    };
    let selected = unsafe { SHBrowseForFolderW(&info) };
    if selected.is_null() {
        return Ok(None);
    }
    let ok = unsafe { SHGetPathFromIDListW(selected, display.as_mut_ptr()) };
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

#[cfg(not(windows))]
pub fn folder(_owner: Option<i64>) -> Result<Option<String>, String> {
    Err("文件夹选择器需要 Windows".into())
}

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
        if target.contains('\0') {
            return Err("打开地址包含无效字符".into());
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
