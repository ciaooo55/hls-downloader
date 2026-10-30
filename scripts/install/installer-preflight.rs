#![cfg_attr(windows, windows_subsystem = "windows")]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    let diagnostic =
        std::env::temp_dir().join(format!("HLSInstaller-{}-{nonce}.log", std::process::id()));
    let mut log = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&diagnostic)
    {
        Ok(log) => log,
        Err(_) => return ExitCode::from(1),
    };
    let result = prepare_install(&mut log);
    if let Err(error) = &result {
        let _ = writeln!(log, "helper stage=failed error={error}");
    }
    let _ = log.flush();
    drop(log);
    if result.is_ok() {
        let _ = std::fs::remove_file(diagnostic);
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn prepare_install(log: &mut File) -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return Err("升级前退出需要唯一的原安装目录".into());
    }
    let root = std::fs::canonicalize(Path::new(&args[0]))
        .map_err(|error| format!("无法读取原安装目录: {error}"))?;
    if !root.join("HLSDownloader.exe").is_file()
        || !root.join("app/resources/HLSDownloaderEngine.exe").is_file()
    {
        return Err("原安装目录缺少程序标识，未停止任何进程".into());
    }
    writeln!(log, "helper stage=entry install_dir={}", root.display())
        .and_then(|_| log.flush())
        .map_err(|error| error.to_string())?;
    #[cfg(windows)]
    return windows::shutdown(&root, log);
    #[cfg(not(windows))]
    Err("安装停机仅支持 Windows".into())
}

fn target_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "hlsdownloader.exe"
            | "hlsdownloaderengine.exe"
            | "hls-downloader-engine.exe"
            | "hlsdownloaderpresenter.exe"
            | "hlsdownloadercli.exe"
            | "hlsdownloadercore.exe"
            | "hlsnativeshell.exe"
            | "hls-native-shell.exe"
            | "hlsnativeengine.exe"
            | "hls-native-engine.exe"
    ) || name.starts_with("hlsdownloadernativehost") && name.ends_with(".exe")
}
fn client_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "hlsdownloader.exe" | "hlsdownloaderpresenter.exe" | "hlsdownloadercli.exe"
    ) || name.starts_with("hlsdownloadernativehost") && name.ends_with(".exe")
}
fn normalized(path: &Path) -> String {
    let value = path.to_string_lossy().replace('/', "\\");
    let value = if let Some(unc) = value.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        value.strip_prefix("\\\\?\\").unwrap_or(&value).to_owned()
    };
    value.trim_end_matches('\\').to_lowercase()
}
fn belongs_to(path: &Path, root: &Path) -> bool {
    normalized(path).starts_with(&(normalized(root) + "\\"))
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    type Handle = *mut c_void;
    const QUERY: u32 = 0x1000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const TERMINATE: u32 = 1;
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        name: [u16; 260],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            path: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn TerminateProcess(process: Handle, code: u32) -> i32;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn GetNamedPipeServerProcessId(pipe: Handle, pid: *mut u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(Handle, isize) -> i32,
            data: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
        fn PostMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> i32;
    }
    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    struct Process {
        pid: u32,
        name: String,
        path: PathBuf,
    }

    fn image(handle: Handle) -> Result<PathBuf, String> {
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
            return Err(format!(
                "读取进程路径失败: {}",
                std::io::Error::last_os_error()
            ));
        }
        use std::os::windows::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
        std::fs::canonicalize(path).map_err(|error| format!("无法确认进程实际路径: {error}"))
    }
    fn open(pid: u32, access: u32) -> Result<Option<OwnedHandle>, String> {
        let handle = unsafe { OpenProcess(access | QUERY | SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            // The process may exit between enumeration and opening its handle.
            if error.raw_os_error() == Some(87) {
                return Ok(None);
            }
            return Err(format!("无法检查进程 {pid}: {error}"));
        }
        let handle = OwnedHandle(handle);
        if unsafe { WaitForSingleObject(handle.0, 0) } == 0 {
            return Ok(None);
        }
        Ok(Some(handle))
    }
    fn processes(root: &Path) -> Result<Vec<Process>, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(2, 0) };
        if snapshot as isize == -1 {
            return Err(format!("无法枚举进程: {}", std::io::Error::last_os_error()));
        }
        let snapshot = OwnedHandle(snapshot);
        let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
        entry.size = std::mem::size_of::<ProcessEntry>() as u32;
        let mut available = unsafe { Process32FirstW(snapshot.0, &mut entry) };
        let mut result = Vec::new();
        while available != 0 {
            let length = entry
                .name
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.name.len());
            let name = String::from_utf16_lossy(&entry.name[..length]);
            if target_name(&name) {
                if let Some(handle) = open(entry.pid, 0)? {
                    match image(handle.0) {
                        Ok(path) if belongs_to(&path, root) => result.push(Process {
                            pid: entry.pid,
                            name,
                            path,
                        }),
                        Ok(_) => {}
                        // Process teardown can revoke image queries before the
                        // process object becomes signaled. Wait on this handle
                        // briefly rather than treating that exit as denial.
                        Err(_) if unsafe { WaitForSingleObject(handle.0, 500) } == 0 => {}
                        Err(error) => return Err(error),
                    }
                }
            }
            available = unsafe { Process32NextW(snapshot.0, &mut entry) };
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(18) {
            return Err("进程枚举未正常结束，安装已停止".into());
        }
        Ok(result)
    }
    fn terminate(process: &Process, root: &Path) -> Result<(), String> {
        if let Some(handle) = open(process.pid, TERMINATE)? {
            // Recheck the handle: a reused PID must not target another installation.
            let path = match image(handle.0) {
                Ok(path) => path,
                Err(_) if unsafe { WaitForSingleObject(handle.0, 500) } == 0 => return Ok(()),
                Err(error) => return Err(error),
            };
            if !belongs_to(&path, root) || normalized(&path) != normalized(&process.path) {
                return Err(format!("进程 {} 身份已变化，未停止该进程", process.pid));
            }
            if unsafe { TerminateProcess(handle.0, 1) } == 0
                && unsafe { WaitForSingleObject(handle.0, 0) } != 0
            {
                return Err(format!(
                    "无法停止进程 {}: {}",
                    process.pid,
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(())
    }
    unsafe extern "system" fn close_window(window: Handle, data: isize) -> i32 {
        let pids = &*(data as *const Vec<u32>);
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        if pids.contains(&pid) {
            PostMessageW(window, 0x0010, 0, 0);
        }
        1
    }
    fn checkpoint(root: &Path, log: &mut File) -> Result<(), String> {
        // A custom-pipe Core can coexist with another installation's default
        // Core. Do not send the shutdown command to that other installation.
        let pipe_name =
            std::env::var_os("HLS_V7_PIPE").unwrap_or_else(|| r"\\.\pipe\HLSDownloader.v7".into());
        let deadline = Instant::now() + Duration::from_secs(2);
        let pipe = loop {
            match OpenOptions::new().read(true).write(true).open(&pipe_name) {
                Ok(pipe) => break pipe,
                Err(error)
                    if matches!(error.raw_os_error(), Some(2 | 231))
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(format!("无法核验 Core 管道归属，未请求停机: {error}")),
            }
        };
        let mut pid = 0;
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } == 0 {
            return Err(format!(
                "无法读取 Core 管道进程: {}",
                std::io::Error::last_os_error()
            ));
        }
        let owner = open(pid, 0)?.ok_or("Core 管道进程已退出，未请求停机")?;
        let path = image(owner.0)?;
        if !belongs_to(&path, root) {
            return Err("Core 管道属于其他安装目录，未请求停机".into());
        }
        drop(pipe);
        writeln!(log, "helper stage=checkpoint-start")
            .and_then(|_| log.flush())
            .map_err(|e| e.to_string())?;
        let mut child = Command::new(root.join("app/resources/HLSDownloaderEngine.exe"))
            .arg("--shutdown")
            // Installer shutdown must use the pipe whose owner was checked.
            .env_remove("HLS_V7_CORE_TCP")
            .env_remove("HLS_V7_CORE_BIND")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(status) => {
                    writeln!(log, "helper stage=checkpoint-finished status={status}")
                        .map_err(|e| e.to_string())?;
                    if status.success() || processes(root)?.iter().all(|p| client_name(&p.name)) {
                        return Ok(());
                    }
                    return Err("任务检查点未成功保存，旧安装和用户数据已保留".into());
                }
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Core 检查点超时，未强制结束 Core 或移除旧安装".into());
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
    fn files_unlocked(root: &Path) -> Result<(), String> {
        for directory in [root.to_path_buf(), root.join("app/resources")] {
            for entry in std::fs::read_dir(&directory).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                {
                    OpenOptions::new()
                        .read(true)
                        .share_mode(0)
                        .open(&path)
                        .map_err(|e| format!("程序文件仍被占用 {}: {e}", path.display()))?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn shutdown(root: &Path, log: &mut File) -> Result<(), String> {
        let started = Instant::now();
        let initial = processes(root)?;
        writeln!(log, "helper stage=processes count={}", initial.len())
            .map_err(|e| e.to_string())?;
        for process in &initial {
            writeln!(
                log,
                "target pid={} name={} path={}",
                process.pid,
                process.name,
                process.path.display()
            )
            .map_err(|e| e.to_string())?;
        }
        let desktops: Vec<_> = initial
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case("HLSDownloader.exe"))
            .map(|p| p.pid)
            .collect();
        if !desktops.is_empty() {
            unsafe {
                EnumWindows(close_window, &desktops as *const Vec<u32> as isize);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        writeln!(log, "helper stage=close-clients")
            .and_then(|_| log.flush())
            .map_err(|e| e.to_string())?;
        for process in processes(root)?.iter().filter(|p| client_name(&p.name)) {
            terminate(process, root)?;
        }
        // Never force-stop a resident Core: its command must save the checkpoint.
        if processes(root)?.iter().any(|p| !client_name(&p.name)) {
            checkpoint(root, log)?;
        }
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut last_error = String::new();
        while Instant::now() < deadline {
            let remaining = match processes(root) {
                Ok(remaining) => remaining,
                Err(error) => {
                    last_error = error;
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            if remaining.is_empty() {
                match files_unlocked(root) {
                    Ok(()) => {
                        writeln!(
                            log,
                            "helper stage=finished elapsed_ms={}",
                            started.elapsed().as_millis()
                        )
                        .map_err(|e| e.to_string())?;
                        return Ok(());
                    }
                    Err(error) => last_error = error,
                }
            } else {
                last_error = format!(
                    "仍有安装目录所属进程: {:?}",
                    remaining.iter().map(|p| p.pid).collect::<Vec<_>>()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "安全停机超时；{last_error}；旧安装和用户数据已保留"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_names_include_versioned_hosts_but_not_unrelated_apps() {
        assert!(target_name("HLSDownloaderNativeHost-7.0.3.exe"));
        assert!(client_name("HLSDownloaderPresenter.exe"));
        assert!(client_name("HLSDownloaderCli.exe"));
        assert!(target_name("hls-downloader-engine.exe"));
        assert!(!client_name("HLSDownloaderEngine.exe"));
        assert!(!target_name("another.exe"));
    }
    #[test]
    fn installation_scope_requires_a_directory_boundary() {
        let root = Path::new(r"\\?\C:\Program Files\HLSDownloader");
        assert!(belongs_to(
            Path::new(r"c:\program files\hlsdownloader\app\engine.exe"),
            root
        ));
        assert!(!belongs_to(
            Path::new(r"C:\Program Files\HLSDownloader-old\engine.exe"),
            root
        ));
        assert!(!belongs_to(Path::new(r"D:\HLSDownloader\engine.exe"), root));
    }
}
