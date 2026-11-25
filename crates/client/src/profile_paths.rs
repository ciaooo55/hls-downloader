//! Filesystem policy shared by the resident Core and lightweight v7 clients.
//!
//! Path selection is intentionally independent from SQLite and transfer-engine
//! modules so helpers such as the native presenter can locate their profile
//! lock without pulling persistence into their build graph.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const USER_SHUTDOWN_MARKER_FILENAME: &str = "user-shutdown.marker";

pub fn default_v7_database_path() -> PathBuf {
    if let Some(root) = env_override("HLS_V7_DATA_DIR") {
        return root.join("data.db");
    }
    if let Some(root) = portable_v7_root() {
        return root.join("data").join("data.db");
    }
    if let Some(root) = env_override("LOCALAPPDATA") {
        return root.join("HLS Downloader").join("v7").join("data.db");
    }
    PathBuf::from("v7-data.db")
}

/// Profile-scoped marker written when the user explicitly quits the download
/// engine from the system tray so passive browser heartbeats (`ping`) do not
/// respawn `HLSDownloaderEngine` behind the user's back.
pub fn user_shutdown_marker_path() -> PathBuf {
    user_shutdown_marker_path_for(&default_v7_database_path())
}

fn user_shutdown_marker_path_for(database_path: &Path) -> PathBuf {
    match database_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            parent.join(USER_SHUTDOWN_MARKER_FILENAME)
        }
        _ => PathBuf::from(USER_SHUTDOWN_MARKER_FILENAME),
    }
}

pub fn record_user_shutdown() {
    record_user_shutdown_at(&user_shutdown_marker_path());
}

fn record_user_shutdown_at(marker_path: &Path) {
    if let Some(parent) = marker_path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let _ = std::fs::write(marker_path, b"tray-quit\n");
}

pub fn clear_user_shutdown() {
    clear_user_shutdown_at(&user_shutdown_marker_path());
}

pub(crate) fn user_shutdown_marker_path_from_exe(exe_path: &Path) -> PathBuf {
    if let Some(root) = env_override("HLS_V7_DATA_DIR") {
        return user_shutdown_marker_path_for(&root.join("data.db"));
    }
    if let Some(root) = portable_v7_root_from(exe_path).or_else(portable_v7_root) {
        return user_shutdown_marker_path_for(&root.join("data").join("data.db"));
    }
    user_shutdown_marker_path()
}

pub(crate) fn clear_user_shutdown_for_exe(exe_path: &Path) {
    clear_user_shutdown_at(&user_shutdown_marker_path_from_exe(exe_path));
}

fn clear_user_shutdown_at(marker_path: &Path) {
    let _ = std::fs::remove_file(marker_path);
}

pub fn is_user_shutdown_active() -> bool {
    is_user_shutdown_active_at(&user_shutdown_marker_path())
}

fn is_user_shutdown_active_at(marker_path: &Path) -> bool {
    marker_path.is_file()
}

pub fn default_v7_download_dir() -> PathBuf {
    default_v7_download_dir_with(
        env_override("HLS_V7_DOWNLOAD_DIR"),
        portable_v7_root(),
        system_downloads_folder,
    )
}

fn default_v7_download_dir_with(
    env_dir: Option<PathBuf>,
    portable_root: Option<PathBuf>,
    system_downloads: impl FnOnce() -> PathBuf,
) -> PathBuf {
    if let Some(root) = env_dir {
        return root;
    }
    if let Some(root) = portable_root {
        return root.join("downloads");
    }
    system_downloads()
}

fn system_downloads_folder() -> PathBuf {
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
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY,
    };

    // 系统 API 会跟随用户移动后的“下载”文件夹，不能从用户名或安装目录拼接。
    let mut path = std::ptr::null_mut();
    // SAFETY: `&FOLDERID_Downloads` is a static `KNOWNFOLDERID` reference, `hToken` is null
    // (current user), and `&mut path` is a valid out-pointer receiving a CoTaskMem-allocated
    // NUL-terminated UTF-16 string.
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_Downloads,
            KF_FLAG_DONT_VERIFY as u32,
            std::ptr::null_mut(),
            &mut path,
        )
    };
    let folder = if result >= 0 && !path.is_null() {
        let mut len = 0;
        // SAFETY: When `SHGetKnownFolderPath` succeeds (`result >= 0`) and `path` is non-null,
        // Windows guarantees `path` points to a valid NUL-terminated UTF-16 buffer allocated
        // with `CoTaskMemAlloc` that remains valid until `CoTaskMemFree` below.
        unsafe {
            while *path.add(len) != 0 {
                len += 1;
            }
            (len > 0)
                .then(|| PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(path, len))))
        }
    } else {
        None
    };
    // SAFETY: `SHGetKnownFolderPath` requires the caller to pass `path` to `CoTaskMemFree`
    // whether the call succeeded or failed (`CoTaskMemFree(null)` is a documented no-op),
    // and `folder` has already copied the UTF-16 units into an owned `PathBuf`.
    unsafe {
        CoTaskMemFree(path.cast());
    }
    folder
}

fn env_override(key: &str) -> Option<PathBuf> {
    nonempty_path(env::var_os(key))
}

fn nonempty_path(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub fn portable_v7_root_from(exe_path: &Path) -> Option<PathBuf> {
    let exe_dir = exe_path.parent()?;
    if exe_dir.join("portable").is_file() && exe_dir.join("app").join("resources").is_dir() {
        return Some(exe_dir.to_path_buf());
    }
    let app_dir = exe_dir.parent()?;
    let root = app_dir.parent()?;
    let in_app_resources = exe_dir
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("resources"))
        && app_dir
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("app"));
    (in_app_resources && root.join("portable").is_file()).then(|| root.to_path_buf())
}

pub fn portable_v7_root() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    portable_v7_root_from(&executable)
}

#[cfg(test)]
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

    #[test]
    fn portable_mode_uses_portable_downloads_unless_overridden_by_env() {
        let system = || PathBuf::from(r"C:\Users\demo\Downloads");
        assert_eq!(
            super::default_v7_download_dir_with(
                Some(PathBuf::from(r"E:\custom")),
                Some(PathBuf::from(r"D:\Portable")),
                system,
            ),
            PathBuf::from(r"E:\custom")
        );
        assert_eq!(
            super::default_v7_download_dir_with(None, Some(PathBuf::from(r"D:\Portable")), system),
            PathBuf::from(r"D:\Portable\downloads")
        );
        assert_eq!(
            super::default_v7_download_dir_with(None, None, system),
            PathBuf::from(r"C:\Users\demo\Downloads")
        );
    }

    #[test]
    fn user_shutdown_marker_roundtrips_inside_profile_directory() {
        let unique = format!(
            "hls-shutdown-marker-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let root = std::env::temp_dir().join(unique);
        let db = root.join("data").join("data.db");
        let marker = super::user_shutdown_marker_path_for(&db);
        assert_eq!(marker, root.join("data").join("user-shutdown.marker"));
        assert!(!super::is_user_shutdown_active_at(&marker));

        super::record_user_shutdown_at(&marker);
        assert!(super::is_user_shutdown_active_at(&marker));

        super::clear_user_shutdown_at(&marker);
        assert!(!super::is_user_shutdown_active_at(&marker));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn user_shutdown_marker_handles_bare_database_filename() {
        assert_eq!(
            super::user_shutdown_marker_path_for(std::path::Path::new("v7-data.db")),
            PathBuf::from("user-shutdown.marker")
        );
    }

    #[test]
    fn portable_v7_root_from_supports_top_level_launcher_and_nested_engine() {
        let unique = format!(
            "hls-portable-root-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let root = std::env::temp_dir().join(unique);
        let resources = root.join("app").join("resources");
        std::fs::create_dir_all(&resources).unwrap();

        let top_launcher = root.join("HLSDownloader.exe");
        let nested_engine = resources.join("HLSDownloaderEngine.exe");
        std::fs::write(&top_launcher, b"launcher").unwrap();
        std::fs::write(&nested_engine, b"engine").unwrap();

        // Without the `portable` marker file, neither layout resolves as portable.
        assert_eq!(super::portable_v7_root_from(&top_launcher), None);
        assert_eq!(super::portable_v7_root_from(&nested_engine), None);

        // With `<root>/portable` + `<root>/app/resources`, BOTH top-level launcher
        // and nested `<root>/app/resources/*.exe` resolve to `<root>`.
        std::fs::write(root.join("portable"), b"").unwrap();
        assert_eq!(
            super::portable_v7_root_from(&top_launcher),
            Some(root.clone())
        );
        assert_eq!(
            super::portable_v7_root_from(&nested_engine),
            Some(root.clone())
        );

        // Top-level exe without `app/resources` directory is not treated as a v7 portable root.
        let bare_dir = root.join("stray");
        std::fs::create_dir_all(&bare_dir).unwrap();
        std::fs::write(bare_dir.join("portable"), b"").unwrap();
        let bare_exe = bare_dir.join("HLSDownloader.exe");
        std::fs::write(&bare_exe, b"stray").unwrap();
        assert_eq!(super::portable_v7_root_from(&bare_exe), None);

        let _ = std::fs::remove_dir_all(&root);
    }
}
