//! Pre-flight checks for the confirm window's save target.
//!
//! Core remains the authority for the final write; these checks only turn the
//! most common avoidable failures (path too long, no write permission, a file
//! name Windows refuses) into an actionable message *before* the handoff is
//! accepted. The browser handoff state determines whether a browser source file still exists.
use hls_client::request_validation::reserved_dos_device_name;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Windows MAX_PATH including the terminating NUL; many players and Explorer
/// operations still fail beyond it even when long paths are enabled.
pub const MAX_PATH_UNITS: usize = 259;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetIssue {
    EmptyName,
    InvalidName(char),
    ReservedName,
    TrailingDot,
    NameTooLong,
    TooLong { units: usize },
    NoPermission,
    NotADirectory,
    ParentDirectory,
}

impl TargetIssue {
    /// Message plus the two recovery commands the confirm window offers.
    pub fn presentation(&self) -> (String, [(&'static str, &'static str); 2]) {
        self.presentation_for(false)
    }

    pub fn presentation_for(&self, copy: bool) -> (String, [(&'static str, &'static str); 2]) {
        match self {
            Self::EmptyName => (
                "请输入文件名。".into(),
                [("", ""), ("", "")],
            ),
            Self::InvalidName(ch) => (
                format!("文件名不能包含“{ch}”。已可一键替换为下划线。"),
                [("修正文件名", "fix_name"), ("", "")],
            ),
            Self::TrailingDot => ("文件名不能以句点或空格结尾。".into(), [("修正文件名", "fix_name"), ("", "")]),
            Self::ReservedName => (
                "该文件名是 Windows 保留名称，无法保存。".into(),
                [("修正文件名", "fix_name"), ("", "")],
            ),
            Self::NameTooLong => (
                "文件名超过磁盘允许的长度。".into(),
                [("缩短文件名", "shorten"), ("", "")],
            ),
            Self::TooLong { units } => (
                format!("完整路径过长（{units} 个 UTF-16 单位），部分播放器和文件管理工具可能无法打开。"),
                [("缩短文件名", "shorten"), ("仍然保存", if copy { "copy_long_path" } else { "accept_long_path" })],
            ),
            Self::NoPermission => (
                "没有写入此文件夹的权限。请选择其他位置，或使用默认下载目录。".into(),
                [("选择其他位置", "browse"), ("使用默认目录", "default_dir")],
            ),
            Self::NotADirectory => (
                "保存位置不是文件夹。".into(),
                [("选择其他位置", "browse"), ("使用默认目录", "default_dir")],
            ),
            Self::ParentDirectory => (
                "保存路径不能包含上级目录（..）。".into(),
                [("选择其他位置", "browse"), ("使用默认目录", "default_dir")],
            ),
        }
    }
}

const INVALID: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

pub fn check_name(filename: &str) -> Result<(), TargetIssue> {
    let name = filename;
    if name.trim().is_empty() {
        return Err(TargetIssue::EmptyName);
    }
    if let Some(ch) = name
        .chars()
        .find(|ch| INVALID.contains(ch) || ch.is_control())
    {
        return Err(TargetIssue::InvalidName(ch));
    }
    if name.encode_utf16().count() > 255 {
        return Err(TargetIssue::NameTooLong);
    }
    if name.ends_with(['.', ' ']) {
        return Err(TargetIssue::TrailingDot);
    }
    if reserved_dos_device_name(name) {
        return Err(TargetIssue::ReservedName);
    }
    Ok(())
}

pub fn path_units(directory: &str, filename: &str) -> usize {
    let directory = directory.trim().trim_end_matches(['\\', '/']);
    // directory + separator + name, counted as UTF-16 like the Win32 API.
    directory.encode_utf16().count() + 1 + filename.encode_utf16().count()
}

/// Checks name, length and (for an existing or creatable folder) write access.
pub fn check_target(directory: &str, filename: &str, allow_long: bool) -> Result<(), TargetIssue> {
    check_name(filename)?;
    if directory.trim().split(['/', '\\']).any(|part| part == "..") {
        return Err(TargetIssue::ParentDirectory);
    }
    if directory.trim().is_empty() {
        // Core falls back to its default download directory.
        return Ok(());
    }
    let units = path_units(directory, filename);
    if units > MAX_PATH_UNITS && !allow_long {
        return Err(TargetIssue::TooLong { units });
    }
    probe_writable(Path::new(directory.trim()))
}

fn probe_writable(directory: &Path) -> Result<(), TargetIssue> {
    // A missing folder is created by Core; probe the nearest existing ancestor.
    let mut existing: PathBuf = directory.to_path_buf();
    while !existing.exists() {
        match existing.parent() {
            Some(parent) if parent != existing => existing = parent.to_path_buf(),
            _ => return Ok(()),
        }
    }
    if !existing.is_dir() {
        return Err(TargetIssue::NotADirectory);
    }
    let probe = existing.join(format!(".hls-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error)
            if error.kind() == ErrorKind::PermissionDenied
                || error.kind() == ErrorKind::ReadOnlyFilesystem =>
        {
            Err(TargetIssue::NoPermission)
        }
        // Unknown I/O errors (offline share, AV hold) stay with Core's own report.
        Err(_) => Ok(()),
    }
}

/// Replaces characters Windows rejects and escapes reserved device names.
pub fn fix_name(filename: &str) -> String {
    let mut name: String = filename
        .trim()
        .trim_end_matches([' ', '.'])
        .chars()
        .map(|ch| {
            if INVALID.contains(&ch) || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .collect();
    if reserved_dos_device_name(&name) {
        name.insert(name.find('.').unwrap_or(name.len()), '_');
    }
    if name.is_empty() {
        name = "download".into();
    }
    name
}

/// Shortens the stem (keeping the extension) so the full path fits.
pub fn shorten_to_fit(directory: &str, filename: &str) -> Option<String> {
    let filename = filename.trim();
    let (stem, extension) = match filename.rfind('.') {
        Some(index) if index > 0 && filename.len() - index <= 12 => filename.split_at(index),
        _ => (filename, ""),
    };
    let fixed = if directory.trim().is_empty() {
        extension.encode_utf16().count()
    } else {
        path_units(directory, extension)
    };
    let budget = MAX_PATH_UNITS
        .checked_sub(fixed)?
        .min(255 - extension.encode_utf16().count());
    if budget == 0 {
        return None;
    }
    let mut kept = String::new();
    let mut units = 0;
    for ch in stem.chars() {
        let width = ch.len_utf16();
        if units + width > budget {
            if units == budget {
                kept.pop();
            }
            kept.push('…');
            break;
        }
        units += width;
        kept.push(ch);
    }
    let shortened = format!("{kept}{extension}");
    check_name(&shortened).ok()?;
    Some(shortened)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSpaceStatus {
    pub hint: String,
    pub low_space: bool,
}

pub fn format_disk_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Formats free disk space for `ConfirmWindow`, raising `low_space = true` when
/// `expected_size > 0 && free_bytes < expected_size`.
pub fn format_disk_space_status(free_bytes: u64, expected_size: u64) -> DiskSpaceStatus {
    if expected_size > 0 && free_bytes < expected_size {
        DiskSpaceStatus {
            hint: format!(
                "磁盘剩余空间不足（需 {}，剩余 {}）",
                format_disk_bytes(expected_size),
                format_disk_bytes(free_bytes)
            ),
            low_space: true,
        }
    } else {
        DiskSpaceStatus {
            hint: format!("磁盘可用 {}", format_disk_bytes(free_bytes)),
            low_space: false,
        }
    }
}

/// Queries free disk bytes available to the current user for `directory`,
/// walking up to the nearest existing ancestor directory when leaf folders have
/// not been created yet.
pub fn query_available_space(directory: &str) -> Option<u64> {
    let trimmed = directory.trim();
    if trimmed.split(['/', '\\']).any(|part| part == "..") {
        return None;
    }
    let mut existing = if trimmed.is_empty() {
        hls_client::default_v7_download_dir()
    } else {
        PathBuf::from(trimmed)
    };
    while !existing.exists() {
        match existing.parent() {
            Some(parent) if parent != existing && !parent.as_os_str().is_empty() => {
                existing = parent.to_path_buf();
            }
            _ => return None,
        }
    }
    if !existing.is_dir() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetDiskFreeSpaceExW(
                lpdirectoryname: *const u16,
                lpfreebytesavailabletocaller: *mut u64,
                lptotalnumberofbytes: *mut u64,
                lptotalnumberoffreebytes: *mut u64,
            ) -> i32;
        }
        let wide: Vec<u16> = existing.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free_available = 0_u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free_available)
    }
    #[cfg(not(windows))]
    {
        let _ = existing;
        None
    }
}

/// `name.ext` → `name (2).ext`, skipping names already in `taken`.
pub fn unique_name(directory: &str, filename: &str, taken: &[String]) -> Result<String, String> {
    let directory = if directory.is_empty() {
        hls_client::default_v7_download_dir()
    } else {
        PathBuf::from(directory)
    };
    let (stem, extension) = match filename.rfind('.') {
        Some(index) if index > 0 => filename.split_at(index),
        _ => (filename, ""),
    };
    for index in 2..1000 {
        let suffix = format!(" ({index}){extension}");
        let budget = 255usize.saturating_sub(suffix.encode_utf16().count());
        let mut units = 0;
        let base: String = stem
            .chars()
            .take_while(|ch| {
                units += ch.len_utf16();
                units <= budget
            })
            .collect();
        let candidate = format!("{base}{suffix}");
        check_name(&candidate).map_err(|issue| issue.presentation().0)?;
        if !taken
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&candidate))
            && !directory
                .join(&candidate)
                .try_exists()
                .map_err(|error| format!("无法检查保存位置：{error}"))?
        {
            return Ok(candidate);
        }
    }
    Err("找不到可用的副本文件名。".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_and_reserved_names() {
        assert_eq!(check_name("  "), Err(TargetIssue::EmptyName));
        assert_eq!(check_name("a:b.mp4"), Err(TargetIssue::InvalidName(':')));
        assert_eq!(check_name("CON.txt"), Err(TargetIssue::ReservedName));
        assert_eq!(check_name("com1"), Err(TargetIssue::ReservedName));
        for name in ["COM0.mp4", "lpt².txt", "CONIN$.txt", "CONOUT$.txt"] {
            assert_eq!(check_name(name), Err(TargetIssue::ReservedName));
            assert!(check_name(&fix_name(name)).is_ok());
        }
        assert!(check_name("COM10.mp4").is_ok());
        assert!(check_name("示例视频 · 1080p.mp4").is_ok());
        assert_eq!(fix_name("a:b?.mp4"), "a_b_.mp4");
        assert_eq!(fix_name("con.txt"), "con_.txt");
        assert_eq!(check_name("video.mp4 "), Err(TargetIssue::TrailingDot));
        assert_eq!(check_name("video\n"), Err(TargetIssue::InvalidName('\n')));
        assert_eq!(check_name("../file"), Err(TargetIssue::InvalidName('/')));
        assert_eq!(
            check_target(r"base\..\other", "file", true),
            Err(TargetIssue::ParentDirectory)
        );
    }

    #[test]
    fn detects_and_fixes_overlong_paths() {
        let directory = format!(r"D:\{}", "子目录".repeat(60));
        let name = format!("{}.mp4", "很长的标题".repeat(20));
        let units = path_units(&directory, &name);
        assert!(units > MAX_PATH_UNITS);
        assert_eq!(
            check_target(&directory, &name, false),
            Err(TargetIssue::TooLong { units })
        );
        let short = shorten_to_fit(&directory, &name).unwrap();
        assert!(short.ends_with(".mp4"));
        assert!(path_units(&directory, &short) <= MAX_PATH_UNITS, "{short}");
        let no_room = format!(r"D:\{}", "a".repeat(251));
        assert!(shorten_to_fit(&no_room, &name).is_none());
        let one_unit = format!(r"D:\{}", "a".repeat(250));
        assert_eq!(shorten_to_fit(&one_unit, &name).as_deref(), Some("….mp4"));
        let exact = format!("{}x.mp4", "😀".repeat(125));
        assert_eq!(shorten_to_fit("", &exact).as_deref(), Some(exact.as_str()));
    }

    #[test]
    fn missing_folder_probes_existing_ancestor() {
        let base = std::env::temp_dir().join(format!("hls-target-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let missing = base.join("a").join("b");
        assert!(check_target(missing.to_str().unwrap(), "x.mp4", false).is_ok());
        let file = base.join("file.bin");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            check_target(file.to_str().unwrap(), "x.mp4", false),
            Err(TargetIssue::NotADirectory)
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn read_only_folder_reports_no_permission() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("hls-ro-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = check_target(base.to_str().unwrap(), "x.mp4", false);
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&base);
        // root ignores mode bits; only assert when the probe could observe them.
        if unsafe { libc_geteuid() } != 0 {
            assert_eq!(result, Err(TargetIssue::NoPermission));
        }
    }

    #[cfg(unix)]
    unsafe fn libc_geteuid() -> u32 {
        extern "C" {
            fn geteuid() -> u32;
        }
        geteuid()
    }

    #[test]
    fn copy_name_skips_files_on_disk_and_known_tasks() {
        let base = std::env::temp_dir().join(format!("hls-copy-name-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("v (2).mp4"), b"keep the original").unwrap();
        let taken = vec!["v (3).mp4".into()];
        assert_eq!(
            unique_name(base.to_str().unwrap(), "v.mp4", &taken).unwrap(),
            "v (4).mp4"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn disk_space_status_formats_available_and_warns_when_insufficient() {
        let gib = 1024_u64 * 1024 * 1024;
        let mib = 1024_u64 * 1024;

        let plenty = format_disk_space_status(42 * gib + 512 * mib, 2 * gib);
        assert!(!plenty.low_space);
        assert_eq!(plenty.hint, "磁盘可用 42.5 GB");

        let unknown_size = format_disk_space_status(512 * mib, 0);
        assert!(!unknown_size.low_space);
        assert_eq!(unknown_size.hint, "磁盘可用 512.0 MB");

        let tight = format_disk_space_status(800 * mib, 2 * gib);
        assert!(tight.low_space);
        assert_eq!(tight.hint, "磁盘剩余空间不足（需 2.0 GB，剩余 800.0 MB）");
    }

    #[cfg(windows)]
    #[test]
    fn query_available_space_resolves_existing_and_missing_nested_directories() {
        let base = std::env::temp_dir().join(format!("hls-space-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let direct = query_available_space(base.to_str().unwrap());
        assert!(direct.is_some_and(|bytes| bytes > 0));

        let nested_missing = base.join("uncreated").join("child");
        let inherited = query_available_space(nested_missing.to_str().unwrap());
        assert!(inherited.is_some_and(|bytes| bytes > 0));

        assert_eq!(query_available_space(r"D:\safe\..\escape"), None);
        let _ = std::fs::remove_dir_all(base);
    }
}
