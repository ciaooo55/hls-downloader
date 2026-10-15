//! Filesystem policy shared by the resident Core and lightweight v7 clients.
//!
//! Path selection is intentionally independent from SQLite and transfer-engine
//! modules so helpers such as the native presenter can locate their profile
//! lock without pulling persistence into their build graph.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

pub fn default_v7_database_path() -> PathBuf {
    if let Some(root) = env_override("HLS_V7_DATA_DIR") {
        return root.join("data.db");
    }
    if let Some(root) = portable_v7_root() {
        return root.join("data").join("data.db");
    }
    if let Some(root) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(root)
            .join("HLS Downloader")
            .join("v7")
            .join("data.db");
    }
    PathBuf::from("v7-data.db")
}

pub fn default_v7_download_dir() -> PathBuf {
    if let Some(root) = env_override("HLS_V7_DOWNLOAD_DIR") {
        return root;
    }
    #[cfg(windows)]
    if let Some(path) = windows_downloads_folder() {
        return path;
    }
    env_override("USERPROFILE")
        .or_else(|| env_override("HOME"))
        .unwrap_or_else(env::temp_dir)
        .join("Downloads")
}

#[cfg(windows)]
fn windows_downloads_folder() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Downloads, KF_FLAG_DONT_VERIFY};

    // 系统 API 会跟随用户移动后的“下载”文件夹，不能从用户名或安装目录拼接。
    unsafe {
        let mut path = std::ptr::null_mut();
        let result = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DONT_VERIFY as u32, std::ptr::null_mut(), &mut path);
        let folder = if result >= 0 && !path.is_null() {
            let mut len = 0;
            while *path.add(len) != 0 {
                len += 1;
            }
            (len > 0).then(|| PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(path, len))))
        } else {
            None
        };
        CoTaskMemFree(path.cast());
        folder
    }
}

fn env_override(key: &str) -> Option<PathBuf> {
    nonempty_path(env::var_os(key))
}

fn nonempty_path(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn portable_v7_root() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    let root = executable.parent()?.parent()?.parent()?;
    root.join("portable").is_file().then(|| root.to_path_buf())
}

#[cfg(all(test, feature = "full-core"))]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn empty_override_is_treated_as_unset() {
        assert_eq!(super::nonempty_path(None), None);
        assert_eq!(super::nonempty_path(Some(OsString::new())), None);
        assert_eq!(
            super::nonempty_path(Some(OsString::from(r"C:\downloads"))),
            Some(PathBuf::from(r"C:\downloads"))
        );
    }
}
