//! Locate and start the v7 Rust Core or Compose workbench.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn locate_core_executable(root: &Path) -> Option<PathBuf> {
    for name in ["HLSDownloaderEngine.exe", "HLSDownloaderEngine"] {
        let candidate = root.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
    let executable = locate_core_executable(root)
        .ok_or_else(|| "HLSDownloaderEngine.exe is not next to the desktop UI".to_string())?;
    let mut command = Command::new(&executable);
    command.current_dir(root);
    command.env("HLS_STARTED_BY_V7_PRESENTER", "1");
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command
        .spawn()
        .map_err(|err| format!("Unable to start download core: {err}"))?;
    Ok(executable)
}

pub fn spawn_desktop_ui(root: &Path) -> bool {
    let Some(executable) = locate_desktop_executable(root) else {
        return false;
    };
    let mut command = Command::new(&executable);
    command.arg("--settings");
    command.current_dir(root);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x00000200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    command.spawn().is_ok()
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
