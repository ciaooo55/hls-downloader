#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::ExitCode;
#[cfg(windows)]
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
    ) || name == "hlsdownloadernativehost.exe"
}
const CORE_PIPE_ENV: &str = "HLS_V7_PIPE";
const PLAYER_CHILD_ENV: &str = "HLS_V7_PLAYER_CHILD";

fn client_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "hlsdownloader.exe" | "hlsdownloaderpresenter.exe" | "hlsdownloadercli.exe"
    ) || name == "hlsdownloadernativehost.exe"
}
fn is_desktop_window_client(name: &str) -> bool {
    name.eq_ignore_ascii_case("HLSDownloader.exe")
        || name.eq_ignore_ascii_case("HLSDownloaderPresenter.exe")
}
fn is_player_child_env(block: &[u16]) -> bool {
    environment_value(block, PLAYER_CHILD_ENV).as_deref() == Some("1")
}
fn requires_core_checkpoint(name: &str, player_child: bool) -> bool {
    target_name(name) && !client_name(name) && !player_child
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
/// Looks up one variable in a Windows environment block (`KEY=value\0...\0\0`).
fn environment_value(block: &[u16], key: &str) -> Option<String> {
    block
        .split_inclusive(|unit| *unit == 0)
        .map_while(|entry| entry.strip_suffix(&[0]))
        .take_while(|entry| !entry.is_empty())
        .filter_map(|entry| String::from_utf16(entry).ok())
        .find_map(|entry| {
            let (name, value) = entry.split_once('=')?;
            (name.eq_ignore_ascii_case(key) && !value.is_empty()).then(|| value.to_owned())
        })
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
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
        fn SetNamedPipeHandleState(
            pipe: Handle,
            mode: *const u32,
            collection_count: *const u32,
            collection_timeout: *const u32,
        ) -> i32;
        fn PeekNamedPipe(
            pipe: Handle,
            buffer: *mut c_void,
            buffer_size: u32,
            bytes_read: *mut u32,
            bytes_available: *mut u32,
            bytes_left: *mut u32,
        ) -> i32;
        fn ReadProcessMemory(
            process: Handle,
            address: usize,
            buffer: *mut c_void,
            size: usize,
            read: *mut usize,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process: Handle,
            class: u32,
            information: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
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
        // Keep the PID reserved while classifying, closing windows and stopping this process.
        _owner: OwnedHandle,
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
                        Ok(path)
                            if belongs_to(&path, root)
                                && path
                                    .file_name()
                                    .is_some_and(|file| file.eq_ignore_ascii_case(&name)) =>
                        {
                            result.push(Process {
                                pid: entry.pid,
                                name,
                                path,
                                _owner: handle,
                            })
                        }
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
    fn read_memory(process: Handle, address: usize, buffer: &mut [u16]) -> Option<()> {
        let size = std::mem::size_of_val(buffer);
        let mut read = 0;
        let ok = unsafe {
            ReadProcessMemory(
                process,
                address,
                buffer.as_mut_ptr().cast(),
                size,
                &mut read,
            )
        };
        (ok != 0 && read == size).then_some(())
    }
    fn read_pointer(process: Handle, address: usize) -> Option<usize> {
        let mut units = [0u16; 4];
        read_memory(process, address, &mut units)?;
        Some(
            units
                .iter()
                .rev()
                .fold(0, |value, unit| value << 16 | usize::from(*unit)),
        )
    }
    /// Reads the raw UTF-16 environment block of a running x64 process from its PEB.
    #[cfg(target_pointer_width = "64")]
    fn process_env_block(pid: u32) -> Option<Vec<u16>> {
        const VM_READ: u32 = 0x0010;
        const QUERY_INFORMATION: u32 = 0x0400;
        let process = open(pid, VM_READ | QUERY_INFORMATION).ok()??;
        // PROCESS_BASIC_INFORMATION: the PEB address is the second pointer.
        let mut basic = [0usize; 6];
        let status = unsafe {
            NtQueryInformationProcess(
                process.0,
                0,
                basic.as_mut_ptr().cast(),
                std::mem::size_of_val(&basic) as u32,
                std::ptr::null_mut(),
            )
        };
        if status != 0 || basic[1] == 0 {
            return None;
        }
        // x64 PEB.ProcessParameters, then Environment and EnvironmentSize.
        let parameters = read_pointer(process.0, basic[1].checked_add(0x20)?)?;
        let environment = read_pointer(process.0, parameters.checked_add(0x80)?)?;
        let size = read_pointer(process.0, parameters.checked_add(0x3f0)?)?.min(1 << 20);
        let mut block = vec![0u16; size / 2];
        read_memory(process.0, environment, &mut block)?;
        Some(block)
    }
    #[cfg(not(target_pointer_width = "64"))]
    fn process_env_block(_pid: u32) -> Option<Vec<u16>> {
        None
    }
    pub(super) fn process_env(pid: u32, key: &str) -> Option<String> {
        let block = process_env_block(pid)?;
        environment_value(&block, key)
    }
    /// The pipe a running Core was started with (`HLS_V7_PIPE`), read from its
    /// own environment. `None` means the default pipe or an unreadable process.
    fn core_pipe(pid: u32) -> Option<String> {
        process_env(pid, CORE_PIPE_ENV)
    }
    fn is_player_child(pid: u32) -> bool {
        process_env_block(pid).is_some_and(|block| is_player_child_env(&block))
    }
    fn checkpoint_pipe(process: &Process) -> Result<Option<(OwnedHandle, File)>, String> {
        let Some(owner) = open(process.pid, 0)? else {
            return Ok(None);
        };
        let path = match image(owner.0) {
            Ok(path) => path,
            Err(_) if unsafe { WaitForSingleObject(owner.0, 500) } == 0 => return Ok(None),
            Err(error) => return Err(error),
        };
        if normalized(&path) != normalized(&process.path) {
            return Err(format!("Core {} 的进程身份已变化", process.pid));
        }
        let maintenance = format!(r"\\.\pipe\HLSDownloader.v7.maintenance.{}", process.pid);
        let configured = std::env::var("HLS_V7_PIPE").ok();
        // Default-pipe Core uses its normal endpoint instead of a maintenance pipe.
        // Read a custom endpoint from the running Core, not only this helper.
        let own = core_pipe(process.pid);
        let mut names = vec![maintenance.as_str()];
        if let Some(name) = own.as_deref().filter(|name| {
            name.len() <= 256
                && name
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\.\pipe\"))
        }) {
            names.push(name);
        }
        if let Some(name) = configured.as_deref().filter(|name| {
            name.len() <= 256
                && name
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\.\pipe\"))
        }) {
            names.push(name);
        }
        names.push(r"\\.\pipe\HLSDownloader.v7");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            for name in &names {
                // Limit impersonation even before checking the peer's identity.
                let Ok(pipe) = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(0x0011_0000) // SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION
                    .open(name)
                else {
                    continue;
                };
                let mut pid = 0;
                if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } != 0
                    && pid == process.pid
                    && unsafe { WaitForSingleObject(owner.0, 0) } == 258
                {
                    return Ok(Some((owner, pipe)));
                }
            }
            if unsafe { WaitForSingleObject(owner.0, 0) } == 0 {
                return Ok(None);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "无法核验 Core {} 的管道归属，未请求停机",
                    process.pid
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn read_checkpoint_frame(
        pipe: &mut File,
        mut buffer: &mut [u8],
        deadline: Instant,
    ) -> Result<(), String> {
        while !buffer.is_empty() {
            if Instant::now() >= deadline {
                return Err("Core 检查点超时，旧安装和用户数据已保留".into());
            }
            // std::fs::File can translate an empty nonblocking Windows pipe
            // into a zero-byte read. Wait for actual bytes before reading.
            let mut available = 0u32;
            if unsafe {
                PeekNamedPipe(
                    pipe.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(format!(
                    "Core 检查点回执连接结束: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if available == 0 {
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            let amount = buffer.len().min(available as usize);
            match pipe.read(&mut buffer[..amount]) {
                Ok(0) => return Err("Core 未确认检查点，旧安装和用户数据已保留".into()),
                Ok(count) => buffer = &mut buffer[count..],
                Err(error) if error.raw_os_error() == Some(232) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(format!("读取 Core 检查点回执失败: {error}")),
            }
        }
        Ok(())
    }

    fn checkpoint(mut pipe: File, owner: OwnedHandle, log: &mut File) -> Result<(), String> {
        const REQUEST: &[u8] =
            br#"{"type":"command","request_id":1,"command":{"kind":"shutdown"}}"#;
        let mut frame = (REQUEST.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(REQUEST);
        // The checked handle carries both the command and its reply. No second
        // connection can select a pipe from a different Core after verification.
        if unsafe {
            SetNamedPipeHandleState(pipe.as_raw_handle(), &1, std::ptr::null(), std::ptr::null())
        } == 0
        {
            return Err(format!(
                "无法设置 Core 管道等待模式: {}",
                std::io::Error::last_os_error()
            ));
        }
        writeln!(log, "helper stage=checkpoint-start")
            .and_then(|_| log.flush())
            .map_err(|error| error.to_string())?;
        if pipe.write(&frame).map_err(|error| error.to_string())? != frame.len() {
            return Err("未能发送完整的 Core 停机请求".into());
        }
        // Core itself can wait up to eight seconds for active transfers.
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut header = [0; 4];
        read_checkpoint_frame(&mut pipe, &mut header, deadline)?;
        let length = u32::from_le_bytes(header) as usize;
        if length == 0 || length > 4 * 1024 * 1024 {
            return Err("Core 停机回执长度异常".into());
        }
        let mut response = vec![0; length];
        read_checkpoint_frame(&mut pipe, &mut response, deadline)?;
        if !response.starts_with(br#"{"type":"events","request_id":1,"events":"#) {
            return Err("Core 未成功保存检查点，旧安装和用户数据已保留".into());
        }
        drop(pipe);
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u32;
        if unsafe { WaitForSingleObject(owner.0, wait) } != 0 {
            return Err("Core 保存检查点后未退出，旧安装和用户数据已保留".into());
        }
        writeln!(log, "helper stage=checkpoint-finished").map_err(|error| error.to_string())
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
            .filter(|p| is_desktop_window_client(&p.name))
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
        // Skip isolated `--player-process` children (`HLS_V7_PLAYER_CHILD=1`) which
        // never open a Core pipe and exit when their parent Core shuts down.
        let mut endpoints = Vec::new();
        for process in processes(root)? {
            let player_child = !client_name(&process.name) && is_player_child(process.pid);
            if player_child {
                let _ = writeln!(log, "helper stage=skip-player-child pid={}", process.pid);
                continue;
            }
            if requires_core_checkpoint(&process.name, false) {
                endpoints.push(checkpoint_pipe(&process)?);
            }
        }
        for (owner, pipe) in endpoints.into_iter().flatten() {
            checkpoint(pipe, owner, log)?;
        }
        let wait_started = Instant::now();
        let deadline = wait_started + Duration::from_secs(12);
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
                // Only terminate orphaned `--player-process` children after a grace window;
                // never force-terminate a resident Core.
                if wait_started.elapsed() >= Duration::from_secs(1) {
                    for process in &remaining {
                        if !client_name(&process.name) && is_player_child(process.pid) {
                            let _ = terminate(process, root);
                        }
                    }
                }
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
    fn process_names_include_current_hosts_but_not_unrelated_apps() {
        assert!(target_name("HLSDownloaderNativeHost.exe"));
        assert!(!target_name("HLSDownloaderNativeHost-other.exe"));
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
    #[test]
    fn environment_lookup_reads_only_the_named_variable() {
        let block: Vec<u16> =
            "=C:=C:\\\0HLS_V7_PIPE_OTHER=x\0hls_v7_pipe=pipe-a\0A=b\0\0HLS_V7_PIPE=late\0"
                .encode_utf16()
                .collect();
        assert_eq!(
            environment_value(&block, "HLS_V7_PIPE").as_deref(),
            Some("pipe-a")
        );
        assert_eq!(environment_value(&block, "HLS_V7_CORE_BIND"), None);
        let empty: Vec<u16> = "HLS_V7_PIPE=\0\0".encode_utf16().collect();
        assert_eq!(environment_value(&empty, "HLS_V7_PIPE"), None);
        let truncated: Vec<_> = "A=1\0HLS_V7_PLAYER_CHILD=1".encode_utf16().collect();
        assert!(!is_player_child_env(&truncated));
    }
    #[test]
    fn player_child_and_desktop_window_classification_are_accurate() {
        let player_block: Vec<u16> = "A=1\0hls_v7_player_child=1\0\0".encode_utf16().collect();
        let zero_block: Vec<u16> = "HLS_V7_PLAYER_CHILD=0\0\0".encode_utf16().collect();
        let plain_block: Vec<u16> = "HLS_V7_PIPE=\\\\.\\pipe\\test\0\0".encode_utf16().collect();
        assert!(is_player_child_env(&player_block));
        assert!(!is_player_child_env(&zero_block));
        assert!(!is_player_child_env(&plain_block));

        assert!(is_desktop_window_client("HLSDownloader.exe"));
        assert!(is_desktop_window_client("hlsdownloaderpresenter.exe"));
        assert!(!is_desktop_window_client("HLSDownloaderEngine.exe"));
        assert!(!is_desktop_window_client("HLSDownloaderNativeHost.exe"));

        assert!(requires_core_checkpoint("HLSDownloaderEngine.exe", false));
        assert!(!requires_core_checkpoint("HLSDownloaderEngine.exe", true));
        assert!(!requires_core_checkpoint(
            "HLSDownloaderPresenter.exe",
            false
        ));
    }
    #[cfg(all(windows, target_pointer_width = "64"))]
    #[test]
    fn current_process_env_lookup_reads_systemroot_from_peb() {
        assert!(windows::process_env(std::process::id(), "systemroot")
            .is_some_and(|value| !value.is_empty()));
    }
}
