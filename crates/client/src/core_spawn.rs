//! Locate and start the v7 Rust Core or Compose workbench.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn locate_core_executable(root: &Path) -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("HLS_ENGINE_PATH").filter(|v| !v.is_empty()) {
        let candidate = PathBuf::from(configured);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for dir in [
        Some(root.to_path_buf()),
        Some(root.join("app/resources")),
        root.parent().map(|parent| parent.join("app/resources")),
    ]
    .into_iter()
    .flatten()
    {
        for name in [
            "HLSDownloaderEngine.exe",
            "HLSDownloaderEngine",
            "hls-downloader-engine.exe",
            "hls-downloader-engine",
        ] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn locate_presenter_executable(root: &Path) -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("HLS_PRESENTER_PATH").filter(|v| !v.is_empty()) {
        let candidate = PathBuf::from(configured);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for dir in [
        Some(root.to_path_buf()),
        Some(root.join("app/resources")),
        root.parent().map(|parent| parent.join("app/resources")),
    ]
    .into_iter()
    .flatten()
    {
        for name in [
            "HLSDownloaderPresenter.exe",
            "HLSDownloaderPresenter",
            "hls-downloader-presenter.exe",
            "hls-downloader-presenter",
        ] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn spawn_with_optional_breakaway(
    command: &mut Command,
    base_flags: u32,
) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    command.creation_flags(base_flags | CREATE_BREAKAWAY_FROM_JOB);
    match command.spawn() {
        Ok(child) => Ok(child),
        // 当宿主进程所在 Windows Job 未开启 JOB_OBJECT_LIMIT_BREAKAWAY_OK 时，
        // CreateProcessW 会返回 ERROR_ACCESS_DENIED (5)，回退为普通分离标志启动。
        Err(error) if error.raw_os_error() == Some(5) => {
            command.creation_flags(base_flags);
            command.spawn()
        }
        Err(error) => Err(error),
    }
}

pub fn locate_desktop_executable(root: &Path) -> Option<PathBuf> {
    for candidate_root in [
        Some(root),
        root.parent(),
        root.parent().and_then(Path::parent),
    ]
    .into_iter()
    .flatten()
    {
        for name in ["HLSDownloader.exe", "HLSDownloader"] {
            let candidate = candidate_root.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(Path::to_path_buf)
}

pub fn spawn_core(root: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{
            GetHandleInformation, SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        };
        // The resident Core must not retain browser/CLI protocol pipes. Null
        // standard streams on Command alone do not clear the original handles.
        for handle in [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ] {
            let mut flags = 0u32;
            if !handle.is_null()
                && handle != INVALID_HANDLE_VALUE
                && unsafe { GetHandleInformation(handle, &mut flags) } != 0
                && (flags & HANDLE_FLAG_INHERIT) != 0
                && unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0
            {
                return Err(format!(
                    "Unable to isolate download core standard handles: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    }
    let started = std::time::Instant::now();
    let profile = std::env::var_os("HLS_V7_STARTUP_PROFILE").is_some();
    let executable = locate_core_executable(root)
        .ok_or_else(|| "HLSDownloaderEngine.exe is not next to the desktop UI".to_string())?;
    let mut command = Command::new(&executable);
    command.current_dir(executable.parent().unwrap_or(root));
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    // 冷启动诊断只输出阶段与耗时；继承 Native Host 的 stderr，避免污染协议 stdout。
    command.stderr(if profile {
        std::process::Stdio::inherit()
    } else {
        std::process::Stdio::null()
    });
    if profile {
        eprintln!(
            "core_spawn stage=before_spawn elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
    #[cfg(windows)]
    {
        // Firefox 会终止 Native Host 的整个 Job；下载核心必须跨插件断线继续运行。
        spawn_with_optional_breakaway(&mut command, 0x08000000) // CREATE_NO_WINDOW
            .map_err(|err| format!("Unable to start download core: {err}"))?;
    }
    #[cfg(not(windows))]
    {
        command
            .spawn()
            .map_err(|err| format!("Unable to start download core: {err}"))?;
    }
    if profile {
        eprintln!(
            "core_spawn stage=spawn_return elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
    Ok(executable)
}

pub fn spawn_desktop_ui(root: &Path) -> bool {
    let (executable, pass_workbench) = if let Some(desktop) = locate_desktop_executable(root) {
        (desktop, false)
    } else if let Some(presenter) = locate_presenter_executable(root) {
        (presenter, true)
    } else {
        return false;
    };
    // 工作台的单实例入口负责恢复自身窗口，避免跨进程焦点限制被误报成启动失败。
    let mut command = Command::new(&executable);
    command.current_dir(executable.parent().unwrap_or(root));
    if pass_workbench {
        command.arg("--workbench");
    }
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        spawn_with_optional_breakaway(&mut command, 0x00000008 | 0x00000200).is_ok()
    }
    #[cfg(not(windows))]
    command.spawn().is_ok()
}

pub fn ensure_presenter(root: &Path) -> Result<(), String> {
    if crate::instance::presenter_is_running() {
        return Ok(());
    }
    let executable = locate_presenter_executable(root)
        .ok_or_else(|| "找不到下载确认组件，请修复主程序安装".to_string())?;
    let mut command = Command::new(&executable);
    command
        .current_dir(executable.parent().unwrap_or(root))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // 多个浏览器同时点击时，弹窗进程的现有单实例锁保留唯一实例。
    #[cfg(windows)]
    {
        // 确认/完成窗口独立于浏览器通信端口，重连和关闭网页不会销毁弹窗。
        spawn_with_optional_breakaway(&mut command, 0x08000000) // CREATE_NO_WINDOW
            .map_err(|error| format!("无法启动下载确认窗口: {error}"))?;
    }
    #[cfg(not(windows))]
    {
        command
            .spawn()
            .map_err(|error| format!("无法启动下载确认窗口: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hls-core-spawn-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|item| item.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn locates_core_next_to_install_root() {
        let dir = temp_dir();
        let exe = dir.join("HLSDownloaderEngine.exe");
        fs::write(&exe, b"core").unwrap();
        assert_eq!(locate_core_executable(&dir), Some(exe));
        assert!(locate_desktop_executable(&dir).is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn locates_desktop_ui_next_to_install_root() {
        let dir = temp_dir();
        let exe = dir.join("HLSDownloader.exe");
        fs::write(&exe, b"ui").unwrap();
        assert_eq!(locate_desktop_executable(&dir), Some(exe));
        let _ = fs::remove_dir_all(dir);
    }
}
