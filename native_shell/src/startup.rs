//! HKCU Run key so the v7 Core starts with Windows. User-scope only.

#[cfg(windows)]
use crate::win_reg::{registry_error, wide};

#[cfg(windows)]
const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const NAME: &str = "HLSDownloader";

#[cfg(windows)]
fn quoted_executable(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::iter::once(u16::from(b'"'))
        .chain(path.as_os_str().encode_wide())
        .chain([u16::from(b'"'), 0])
        .collect()
}

#[cfg(windows)]
fn enable_startup() -> Result<(), String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let value = quoted_executable(&exe);
    let mut handle = null_mut();
    let create = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(SUBKEY).as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut handle,
            null_mut(),
        )
    };
    if create != 0 {
        return Err(registry_error("打开开机启动注册表失败", create));
    }
    let set = unsafe {
        RegSetValueExW(
            handle,
            wide(NAME).as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            std::mem::size_of_val(value.as_slice()) as u32,
        )
    };
    unsafe { RegCloseKey(handle) };
    if set == 0 {
        Ok(())
    } else {
        Err(registry_error("写入开机启动失败", set))
    }
}

#[cfg(windows)]
fn disable_startup() -> Result<(), String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_SET_VALUE,
    };

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
        if enabled {
            enable_startup()
        } else {
            disable_startup()
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn startup_quotes_the_exact_windows_path() {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<_> = r"C:\Program Files\HLS\".encode_utf16().collect();
        units.push(0xd800);
        units.extend(".exe".encode_utf16());
        let path = std::ffi::OsString::from_wide(&units);
        let quoted = super::quoted_executable(std::path::Path::new(&path));
        assert_eq!(&quoted[1..quoted.len() - 2], &units);
        assert_eq!(quoted[0], u16::from(b'"'));
        assert_eq!(&quoted[quoted.len() - 2..], &[u16::from(b'"'), 0]);
    }

    #[test]
    fn apply_is_noop_in_unit_tests() {
        super::apply(false).unwrap();
    }
}
