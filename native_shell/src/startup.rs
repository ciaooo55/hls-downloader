//! HKCU Run key so the v7 Core starts with Windows. User-scope only.

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn registry_error(action: &str, code: u32) -> String {
    format!(
        "{action}: {}",
        std::io::Error::from_raw_os_error(code as i32)
    )
}

#[cfg(windows)]
fn disable_startup() -> Result<(), String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_SET_VALUE,
    };

    const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "HLSDownloader";
    let subkey = wide(SUBKEY);
    let name = wide(NAME);
    let mut handle = null_mut();
    let open = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut handle,
        )
    };
    if open == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if open != 0 {
        return Err(registry_error("打开开机启动注册表失败", open));
    }

    let delete = unsafe { RegDeleteValueW(handle, name.as_ptr()) };
    unsafe {
        RegCloseKey(handle);
    }
    if delete == 0 || delete == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(registry_error("删除开机启动失败", delete))
    }
}

pub fn apply(enabled: bool) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(())
    }
    #[cfg(windows)]
    {
        if cfg!(test) {
            return Ok(());
        }
        const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        const NAME: &str = "HLSDownloader";
        if enabled {
            let exe = std::env::current_exe().map_err(|error| error.to_string())?;
            let value = format!("\"{}\"", exe.display());
            let status = std::process::Command::new("reg")
                .args(["add", KEY, "/v", NAME, "/t", "REG_SZ", "/d", &value, "/f"])
                .status()
                .map_err(|error| error.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("写入开机启动失败".into())
            }
        } else {
            disable_startup()
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn apply_is_noop_in_unit_tests() {
        super::apply(false).unwrap();
    }
}
