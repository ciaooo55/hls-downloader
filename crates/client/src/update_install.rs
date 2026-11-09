//! Verified MSI execution and result reporting, independent of download engines.
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const EXPECTED_PRODUCT_NAME: &str = "HLSDownloader";
pub const EXPECTED_UPGRADE_CODE: &str = "{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerIdentity {
    pub product_name: String,
    pub product_version: String,
    pub upgrade_code: String,
    pub per_user: bool,
}

pub struct UpdaterLaunch {
    pub log_path: PathBuf,
    pub result_path: PathBuf,
}

pub fn verify_installer_identity(
    path: &Path,
    expected_version: &str,
) -> Result<InstallerIdentity, String> {
    if !path.is_file()
        || !path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".msi")
    {
        return Err("升级安装包不是有效的 MSI 文件".into());
    }
    #[cfg(windows)]
    {
        verify_installer_authenticode(path)?;
        let identity = read_msi_identity(path)?;
        validate_installer_identity_fields(&identity, expected_version)?;
        Ok(identity)
    }
    #[cfg(not(windows))]
    {
        let _ = expected_version;
        Err("MSI 身份校验只适用于 Windows".into())
    }
}

pub fn launch_update_helper(
    path: &Path,
    expected_version: &str,
    workbench_pid: u32,
) -> Result<UpdaterLaunch, String> {
    let identity = verify_installer_identity(path, expected_version)?;
    if !identity.per_user {
        return Err("升级包不是当前产品使用的每用户安装方式".into());
    }
    let source = locate_updater_executable()?;
    let helper_path = path.with_file_name(format!(
        "HLSDownloaderUpdater-{}-{}.exe",
        sanitize_version(expected_version),
        std::process::id()
    ));
    std::fs::copy(&source, &helper_path)
        .map_err(|error| format!("准备独立更新助手失败: {error}"))?;
    let log_path = path.with_extension("install.log");
    let result_path = update_root().join("last-install-result.json");
    let install_root = std::env::current_exe()
        .ok()
        .and_then(|value| value.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let workbench = crate::locate_desktop_executable(&install_root)
        .or_else(|| {
            install_root
                .parent()
                .and_then(crate::locate_desktop_executable)
        })
        .unwrap_or_default();
    let mut command = std::process::Command::new(&helper_path);
    command
        .arg("--msi")
        .arg(path)
        .arg("--version")
        .arg(expected_version)
        .arg("--core-pid")
        .arg(std::process::id().to_string())
        .arg("--workbench-pid")
        .arg(workbench_pid.to_string())
        .arg("--install-root")
        .arg(&install_root)
        .arg("--workbench")
        .arg(&workbench)
        .arg("--log")
        .arg(&log_path)
        .arg("--result")
        .arg(&result_path)
        .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command
        .spawn()
        .map_err(|error| format!("启动独立更新助手失败: {error}"))?;
    Ok(UpdaterLaunch {
        log_path,
        result_path,
    })
}

fn msiexec_args(path: &Path, log_path: &Path) -> Vec<std::ffi::OsString> {
    vec![
        "/i".into(),
        path.as_os_str().to_owned(),
        "/passive".into(),
        "/norestart".into(),
        "REBOOT=ReallySuppress".into(),
        "/L*v".into(),
        log_path.as_os_str().to_owned(),
    ]
}

fn locate_updater_executable() -> Result<PathBuf, String> {
    let current =
        std::env::current_exe().map_err(|error| format!("读取 Core 路径失败: {error}"))?;
    let mut roots = vec![current
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()];
    if let Some(parent) = roots[0].parent() {
        roots.push(parent.to_path_buf());
    }
    if let Ok(configured) = std::env::var("HLS_V7_UPDATER") {
        let value = PathBuf::from(configured);
        if value.is_file() {
            return Ok(value);
        }
    }
    roots
        .into_iter()
        .map(|root| root.join("HLSDownloaderUpdater.exe"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "安装目录缺少 HLSDownloaderUpdater.exe".into())
}

fn validate_installer_identity_fields(
    identity: &InstallerIdentity,
    expected_version: &str,
) -> Result<(), String> {
    if identity.product_name != EXPECTED_PRODUCT_NAME {
        return Err(format!("安装包产品名不匹配: {}", identity.product_name));
    }
    let release_version = expected_version.trim().trim_start_matches('v');
    let product_version = match release_version.rsplit_once("-r") {
        Some((base, revision))
            if !revision.is_empty() && revision.chars().all(|ch| ch.is_ascii_digit()) =>
        {
            base
        }
        _ => release_version,
    };
    if identity.product_version != product_version {
        return Err(format!(
            "安装包版本不匹配: 预期 {}，实际 {}",
            expected_version, identity.product_version
        ));
    }
    if normalize_guid(&identity.upgrade_code) != normalize_guid(EXPECTED_UPGRADE_CODE) {
        return Err(format!(
            "安装包 UpgradeCode 不匹配: {}",
            identity.upgrade_code
        ));
    }
    if !identity.per_user {
        return Err("升级包不是当前产品要求的每用户安装上下文".into());
    }
    Ok(())
}

fn normalize_guid(value: &str) -> String {
    value.trim().trim_matches(['{', '}']).to_ascii_uppercase()
}

#[cfg(windows)]
fn verify_installer_authenticode(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_DISABLE_MD2_MD4,
        WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut file: WINTRUST_FILE_INFO = unsafe { std::mem::zeroed() };
    file.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
    file.pcwszFilePath = path_wide.as_ptr();

    let mut trust: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    trust.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    trust.dwUIChoice = WTD_UI_NONE;
    trust.fdwRevocationChecks = WTD_REVOKE_NONE;
    trust.dwUnionChoice = WTD_CHOICE_FILE;
    trust.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };
    trust.dwStateAction = WTD_STATEACTION_VERIFY;
    trust.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_DISABLE_MD2_MD4;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;

    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            &mut trust as *mut _ as *mut core::ffi::c_void,
        )
    };
    trust.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            &mut trust as *mut _ as *mut core::ffi::c_void,
        );
    }
    if status != 0 {
        return Err(format!(
            "升级安装包 Authenticode 信任校验失败（WinVerifyTrust 0x{:08X}）",
            status as u32
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn read_msi_identity(path: &Path) -> Result<InstallerIdentity, String> {
    Ok(InstallerIdentity {
        product_name: read_msi_property(path, "ProductName")?,
        product_version: read_msi_property(path, "ProductVersion")?,
        upgrade_code: read_msi_property(path, "UpgradeCode")?,
        per_user: msi_is_per_user(path)?,
    })
}

#[cfg(windows)]
fn read_msi_property(path: &Path, property: &str) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::ApplicationInstallationAndServicing::{
        MsiCloseHandle, MsiDatabaseOpenViewW, MsiOpenDatabaseW, MsiRecordGetStringW,
        MsiViewExecute, MsiViewFetch, MSIHANDLE,
    };
    const ERROR_SUCCESS: u32 = 0;
    const ERROR_MORE_DATA: u32 = 234;

    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut database: MSIHANDLE = 0;
    let opened = unsafe { MsiOpenDatabaseW(path_wide.as_ptr(), std::ptr::null(), &mut database) };
    if opened != ERROR_SUCCESS {
        return Err(format!("无法读取 MSI 数据库（错误 {opened}）"));
    }
    let query = format!(
        "SELECT `Value` FROM `Property` WHERE `Property`='{}'",
        property.replace('\'', "''")
    );
    let query_wide: Vec<u16> = OsStr::new(&query).encode_wide().chain(Some(0)).collect();
    let mut view: MSIHANDLE = 0;
    let result = (|| {
        let status = unsafe { MsiDatabaseOpenViewW(database, query_wide.as_ptr(), &mut view) };
        if status != ERROR_SUCCESS {
            return Err(format!("无法读取 MSI {property}（错误 {status}）"));
        }
        let status = unsafe { MsiViewExecute(view, 0) };
        if status != ERROR_SUCCESS {
            return Err(format!("无法执行 MSI {property} 查询（错误 {status}）"));
        }
        let mut record: MSIHANDLE = 0;
        let status = unsafe { MsiViewFetch(view, &mut record) };
        if status != ERROR_SUCCESS {
            return Err(format!("MSI 缺少 {property}（错误 {status}）"));
        }
        let value = (|| {
            let mut required = 0u32;
            let first =
                unsafe { MsiRecordGetStringW(record, 1, std::ptr::null_mut(), &mut required) };
            if first != ERROR_MORE_DATA && first != ERROR_SUCCESS {
                return Err(format!("无法读取 MSI {property}（错误 {first}）"));
            }
            let mut buffer = vec![0u16; required.saturating_add(1) as usize];
            let mut capacity = buffer.len() as u32;
            let status =
                unsafe { MsiRecordGetStringW(record, 1, buffer.as_mut_ptr(), &mut capacity) };
            if status != ERROR_SUCCESS {
                return Err(format!("无法读取 MSI {property}（错误 {status}）"));
            }
            Ok(String::from_utf16_lossy(&buffer[..capacity as usize]))
        })();
        unsafe { MsiCloseHandle(record) };
        value
    })();
    if view != 0 {
        unsafe { MsiCloseHandle(view) };
    }
    unsafe { MsiCloseHandle(database) };
    result
}

#[cfg(windows)]
fn msi_is_per_user(path: &Path) -> Result<bool, String> {
    let all_users = read_optional_msi_property(path, "ALLUSERS")?;
    let per_user = read_optional_msi_property(path, "MSIINSTALLPERUSER")?;
    Ok(
        all_users.trim().is_empty()
            || all_users.trim() == "2"
            || per_user.eq_ignore_ascii_case("1"),
    )
}

#[cfg(windows)]
fn read_optional_msi_property(path: &Path, property: &str) -> Result<String, String> {
    match read_msi_property(path, property) {
        Ok(value) => Ok(value),
        Err(error) if error.contains("MSI 缺少") => Ok(String::new()),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallResult {
    pub version: String,
    pub status: String,
    pub exit_code: i32,
    pub message: String,
    pub install_log: String,
}

#[derive(Debug)]
struct HelperArgs {
    msi: PathBuf,
    version: String,
    core_pid: u32,
    workbench_pid: u32,
    install_root: PathBuf,
    workbench: PathBuf,
    log: PathBuf,
    result: PathBuf,
}

pub fn run_update_helper(args: impl IntoIterator<Item = OsString>) -> Result<u8, String> {
    let args = parse_helper_args(args)?;
    match run_update_helper_inner(&args) {
        Ok(code) => Ok(code),
        Err(error) => {
            let result = InstallResult {
                version: args.version.clone(),
                status: "failed".into(),
                exit_code: -1,
                message: error.clone(),
                install_log: args.log.display().to_string(),
            };
            let _ = write_install_result(&args.result, &result);
            restart_workbench(&args);
            Err(error)
        }
    }
}

fn run_update_helper_inner(args: &HelperArgs) -> Result<u8, String> {
    let identity = verify_installer_identity(&args.msi, &args.version)?;
    if !identity.per_user {
        return Err("升级包安装上下文不是每用户安装".into());
    }
    wait_for_product_shutdown(args)?;
    let exit_code = run_msiexec(&args.msi, &args.log)?;
    let (status, message) = installer_exit_status(exit_code);
    let result = InstallResult {
        version: args.version.clone(),
        status: status.into(),
        exit_code,
        message: message.into(),
        install_log: args.log.display().to_string(),
    };
    write_install_result(&args.result, &result)?;
    restart_workbench(args);
    Ok(if status == "success" { 0 } else { 1 })
}

fn restart_workbench(args: &HelperArgs) {
    let workbench = if args.workbench.is_file() {
        args.workbench.clone()
    } else {
        crate::locate_desktop_executable(&args.install_root).unwrap_or(args.workbench.clone())
    };
    if workbench.is_file() {
        let _ = std::process::Command::new(&workbench)
            .current_dir(workbench.parent().unwrap_or_else(|| Path::new(".")))
            .spawn();
    }
}

fn parse_helper_args(args: impl IntoIterator<Item = OsString>) -> Result<HelperArgs, String> {
    let mut values = std::collections::HashMap::<String, OsString>::new();
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        let name = flag.to_string_lossy().to_string();
        if !name.starts_with("--") {
            return Err(format!("无法识别更新助手参数: {name}"));
        }
        let value = iter.next().ok_or_else(|| format!("参数 {name} 缺少值"))?;
        values.insert(name, value);
    }
    let value = |name: &str| {
        values
            .get(name)
            .cloned()
            .ok_or_else(|| format!("缺少更新助手参数 {name}"))
    };
    let number = |name: &str| -> Result<u32, String> {
        value(name)?
            .to_string_lossy()
            .parse()
            .map_err(|_| format!("更新助手参数 {name} 不是有效进程号"))
    };
    Ok(HelperArgs {
        msi: PathBuf::from(value("--msi")?),
        version: value("--version")?.to_string_lossy().into_owned(),
        core_pid: number("--core-pid")?,
        workbench_pid: number("--workbench-pid")?,
        install_root: PathBuf::from(value("--install-root")?),
        workbench: PathBuf::from(value("--workbench")?),
        log: PathBuf::from(value("--log")?),
        result: PathBuf::from(value("--result")?),
    })
}

fn run_msiexec(path: &Path, log_path: &Path) -> Result<i32, String> {
    run_installer_with(
        || run_msiexec_once(&msiexec_args(path, log_path)),
        || std::thread::sleep(Duration::from_secs(10)),
    )
}

#[cfg(windows)]
fn run_msiexec_once(args: &[OsString]) -> Result<i32, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let mut directory = [0u16; 260];
    let length =
        unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) } as usize;
    if length == 0 || length >= directory.len() {
        return Err("无法定位系统 Windows Installer".into());
    }
    let mut executable = directory[..length].to_vec();
    executable.extend("\\msiexec.exe\0".encode_utf16());
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let mut parameters = Vec::new();
    for argument in args {
        if !parameters.is_empty() {
            parameters.push(b' ' as u16);
        }
        // Installer 的开关不能加引号；仅包含空白的已验证文件路径需要引用。
        let wide: Vec<u16> = argument.encode_wide().collect();
        let quoted = wide
            .iter()
            .any(|ch| *ch == b' ' as u16 || *ch == b'\t' as u16);
        if quoted {
            parameters.push(b'"' as u16);
        }
        parameters.extend(wide);
        if quoted {
            parameters.push(b'"' as u16);
        }
    }
    parameters.push(0);
    let com = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    let result = (|| {
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = executable.as_ptr();
        info.lpParameters = parameters.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        // 只提升 Installer，更新助手及重启后的工作台仍以原用户权限运行。
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let error = unsafe { GetLastError() };
            return if error == ERROR_CANCELLED {
                Ok(1602)
            } else {
                Err(format!("启动升级安装失败（Windows {error}）"))
            };
        }
        if info.hProcess.is_null() {
            return Err("Windows Installer 未返回安装进程".into());
        }
        let wait = unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
        let mut exit = 0;
        let read =
            wait == WAIT_OBJECT_0 && unsafe { GetExitCodeProcess(info.hProcess, &mut exit) } != 0;
        unsafe { CloseHandle(info.hProcess) };
        if !read {
            return Err("无法读取 Windows Installer 安装结果".into());
        }
        Ok(exit as i32)
    })();
    if com >= 0 {
        unsafe { CoUninitialize() };
    }
    result
}

#[cfg(not(windows))]
fn run_msiexec_once(args: &[OsString]) -> Result<i32, String> {
    let status = std::process::Command::new("msiexec.exe")
        .args(args)
        .status()
        .map_err(|error| format!("运行覆盖安装失败: {error}"))?;
    Ok(status.code().unwrap_or(-1))
}

fn run_installer_with(
    mut run: impl FnMut() -> Result<i32, String>,
    mut wait: impl FnMut(),
) -> Result<i32, String> {
    for attempt in 0..4 {
        let exit_code = run()?;
        if exit_code != 1618 || attempt == 3 {
            return Ok(exit_code);
        }
        wait();
    }
    unreachable!()
}

fn installer_exit_status(exit_code: i32) -> (&'static str, &'static str) {
    match exit_code {
        1602 => ("failed", "已取消升级安装，原版本将重新启动"),
        0 => ("success", "HLS Downloader 已完成覆盖升级"),
        3010 | 1641 => ("success", "覆盖升级已完成，Windows 稍后需要重新启动"),
        1618 => ("failed", "另一项 Windows 安装仍在进行，请稍后重试"),
        1603 => ("failed", "Windows Installer 未能完成升级，请查看安装日志"),
        _ => ("failed", "覆盖升级未完成，请查看安装日志"),
    }
}

fn write_install_result(path: &Path, result: &InstallResult) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建升级结果目录失败: {error}"))?;
    }
    let temporary = path.with_extension("json.part");
    let data =
        serde_json::to_vec_pretty(result).map_err(|error| format!("编码升级结果失败: {error}"))?;
    std::fs::write(&temporary, data).map_err(|error| format!("写入升级结果失败: {error}"))?;
    // `rename` on Windows already replaces the destination. Removing the old file
    // first opens a window in which neither the old result nor the new one exists,
    // so a helper killed in that window loses the only record of how the upgrade
    // ended: take_install_result then reports nothing and the workbench is never
    // told the upgrade failed. Publish in one step.
    std::fs::rename(&temporary, path).map_err(|error| format!("发布升级结果失败: {error}"))
}

pub fn take_install_result() -> Result<Option<InstallResult>, String> {
    take_install_result_at(&update_root().join("last-install-result.json"))
}

/// Reads and consumes the last recorded upgrade result at `path`.
///
/// An older build published with remove-then-rename, so an upgrade interrupted in
/// that window left the result in its `.part` sidecar while the published file was
/// already gone. Reading the sidecar is what makes such an upgrade report a
/// failure instead of silently reporting nothing at all.
fn take_install_result_at(path: &Path) -> Result<Option<InstallResult>, String> {
    let source = if path.is_file() {
        path.to_path_buf()
    } else {
        path.with_extension("json.part")
    };
    if !source.is_file() {
        return Ok(None);
    }
    let data = std::fs::read(&source).map_err(|error| format!("读取升级结果失败: {error}"))?;
    let result =
        serde_json::from_slice(&data).map_err(|error| format!("解析升级结果失败: {error}"))?;
    std::fs::remove_file(&source).map_err(|error| format!("确认升级结果失败: {error}"))?;
    Ok(Some(result))
}

fn wait_for_product_shutdown(args: &HelperArgs) -> Result<(), String> {
    #[cfg(windows)]
    {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            request_process_window_close(args.workbench_pid);
            let core_running = process_running(args.core_pid);
            let ui_running = process_running(args.workbench_pid);
            terminate_auxiliary_processes();
            if !core_running && !ui_running {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("工作台或下载引擎未能安全退出，已取消覆盖升级".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        Err("覆盖升级只适用于 Windows".into())
    }
}

#[cfg(windows)]
fn process_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let running = unsafe { WaitForSingleObject(handle, 0) == WAIT_TIMEOUT };
    unsafe { CloseHandle(handle) };
    running
}

#[cfg(windows)]
fn request_process_window_close(pid: u32) {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };
    unsafe extern "system" fn callback(window: HWND, data: LPARAM) -> BOOL {
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(window, &mut owner) };
        if owner == data as u32 {
            unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
        }
        1
    }
    if pid != 0 {
        unsafe { EnumWindows(Some(callback), pid as isize) };
    }
}

#[cfg(windows)]
fn terminate_auxiliary_processes() {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return;
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let end = entry
            .szExeFile
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]).to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "hlsdownloaderpresenter.exe" | "hlsdownloadernativehost.exe"
        ) {
            request_process_window_close(entry.th32ProcessID);
            let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, entry.th32ProcessID) };
            if !handle.is_null() {
                unsafe {
                    TerminateProcess(handle, 0);
                    CloseHandle(handle);
                }
            }
        }
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
}

pub fn update_root() -> PathBuf {
    std::env::temp_dir().join("HLSDownloader").join("updates")
}

pub fn sanitize_version(version: &str) -> String {
    let value: String = version
        .trim()
        .trim_start_matches('v')
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if value.is_empty() {
        "unknown".into()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cover_install_uses_passive_msi_without_forcing_a_reboot() {
        let msi = Path::new(r"C:\Temp\HLSDownloader-7.1.0-Windows-x64.msi");
        let log = Path::new(r"C:\Temp\HLSDownloader-7.1.0-Windows-x64.install.log");
        let args: Vec<String> = msiexec_args(msi, log)
            .into_iter()
            .map(|item| item.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "/i");
        assert_eq!(args[1], msi.display().to_string());
        assert!(args.contains(&"/passive".into()));
        assert!(args.contains(&"/norestart".into()));
        assert!(args.contains(&"REBOOT=ReallySuppress".into()));
        assert_eq!(args.last().unwrap(), &log.display().to_string());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires interactive Windows UAC approval"]
    fn elevated_installer_returns_the_real_exit_code() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::ApplicationInstallationAndServicing::{
            MsiQueryProductStateW, INSTALLSTATE_UNKNOWN,
        };
        let product = OsString::from("{68FD814E-9C02-44F9-996D-0C0712157452}");
        let wide: Vec<u16> = product.encode_wide().chain(Some(0)).collect();
        assert_eq!(
            unsafe { MsiQueryProductStateW(wide.as_ptr()) },
            INSTALLSTATE_UNKNOWN
        );
        // 未安装的产品只返回 1605，不会改动现有产品；同时覆盖 UAC、参数和进程等待。
        assert_eq!(
            run_msiexec_once(&["/x".into(), product, "/qn".into(), "/norestart".into()]).unwrap(),
            1605
        );
    }

    #[cfg(windows)]
    #[test]
    fn authenticode_rejects_unsigned_update_payload() {
        let path = std::env::temp_dir().join(format!(
            "hls-updater-unsigned-{}-{}.msi",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"unsigned update fixture").unwrap();
        let error = verify_installer_authenticode(&path).unwrap_err();
        assert!(error.contains("Authenticode"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn installer_identity_rejects_another_product_version_or_upgrade_family() {
        let valid = InstallerIdentity {
            product_name: EXPECTED_PRODUCT_NAME.into(),
            product_version: "7.1.0".into(),
            upgrade_code: EXPECTED_UPGRADE_CODE.to_ascii_lowercase(),
            per_user: true,
        };
        assert!(validate_installer_identity_fields(&valid, "v7.1.0").is_ok());
        assert!(validate_installer_identity_fields(&valid, "v7.1.0-r3").is_ok());
        assert!(validate_installer_identity_fields(&valid, "v7.1.0-preview").is_err());
        let mut invalid = valid.clone();
        invalid.product_name = "AnotherDownloader".into();
        assert!(validate_installer_identity_fields(&invalid, "7.1.0").is_err());
        invalid = valid.clone();
        invalid.product_version = "7.2.0".into();
        assert!(validate_installer_identity_fields(&invalid, "7.1.0").is_err());
        invalid = valid;
        invalid.upgrade_code = "{00000000-0000-0000-0000-000000000000}".into();
        assert!(validate_installer_identity_fields(&invalid, "7.1.0").is_err());
        let invalid_context = InstallerIdentity {
            product_name: EXPECTED_PRODUCT_NAME.into(),
            product_version: "7.1.0".into(),
            upgrade_code: EXPECTED_UPGRADE_CODE.into(),
            per_user: false,
        };
        assert!(validate_installer_identity_fields(&invalid_context, "7.1.0").is_err());
    }

    #[test]
    fn installer_exit_codes_preserve_success_reboot_busy_and_fatal_results() {
        assert_eq!(installer_exit_status(0).0, "success");
        assert_eq!(installer_exit_status(3010).0, "success");
        assert_eq!(installer_exit_status(1641).0, "success");
        assert!(installer_exit_status(1618).1.contains("另一项"));
        assert!(installer_exit_status(1603).1.contains("安装日志"));
        assert_eq!(installer_exit_status(5).0, "failed");
    }

    #[test]
    fn installer_waits_for_busy_windows_installer_and_stops_on_fatal_exit() {
        let mut busy_then_success = vec![1618, 1618, 0].into_iter();
        let mut waits = 0;
        let result =
            run_installer_with(|| Ok(busy_then_success.next().unwrap()), || waits += 1).unwrap();
        assert_eq!(result, 0);
        assert_eq!(waits, 2);

        let mut calls = 0;
        let result = run_installer_with(
            || {
                calls += 1;
                Ok(1603)
            },
            || panic!("fatal installer exit must not be retried"),
        )
        .unwrap();
        assert_eq!(result, 1603);
        assert_eq!(calls, 1);
    }

    #[test]
    fn helper_arguments_keep_unicode_paths_and_reject_missing_values() {
        let args = parse_helper_args(
            [
                "--msi",
                r"A:\测试\更新.msi",
                "--version",
                "7.1.0",
                "--core-pid",
                "10",
                "--workbench-pid",
                "11",
                "--install-root",
                r"A:\测试\应用",
                "--workbench",
                r"A:\测试\应用\HLSDownloader.exe",
                "--log",
                r"A:\测试\更新.log",
                "--result",
                r"A:\测试\result.json",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(args.msi, PathBuf::from(r"A:\测试\更新.msi"));
        assert_eq!(args.core_pid, 10);
        assert_eq!(args.workbench_pid, 11);
        assert!(parse_helper_args([OsString::from("--msi")]).is_err());
    }

    #[test]
    fn install_result_is_utf8_without_bom_and_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "hls-updater-result-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("result.json");
        let expected = InstallResult {
            version: "7.1.0".into(),
            status: "failed".into(),
            exit_code: 1603,
            message: "安装失败".into(),
            install_log: r"C:\Temp\安装.log".into(),
        };
        write_install_result(&path, &expected).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        assert_eq!(
            serde_json::from_slice::<InstallResult>(&bytes).unwrap(),
            expected
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_install_result_interrupted_mid_publish_is_still_reported() {
        // The old publish order was remove-then-rename. A helper killed between
        // those two steps left the published file already deleted and the result
        // only in its `.part` sidecar, so take_install_result reported nothing and
        // the workbench was never told the upgrade had failed. Rebuild exactly that
        // on-disk state and require the result to survive it.
        let root = std::env::temp_dir().join(format!(
            "hls-updater-part-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("last-install-result.json");
        let expected = InstallResult {
            version: "7.1.0".into(),
            status: "failed".into(),
            exit_code: 1603,
            message: "覆盖升级未完成".into(),
            install_log: r"C:\Temp\upgrade.log".into(),
        };
        write_install_result(&path, &expected).unwrap();
        // Recreate the interrupted state: the published file is gone and the result
        // is left only in the `.part` sidecar the rename would have consumed.
        let sidecar = path.with_extension("json.part");
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&sidecar, serde_json::to_vec_pretty(&expected).unwrap()).unwrap();
        assert!(sidecar.is_file(), "the sidecar must hold the result");

        assert_eq!(
            take_install_result_at(&path).unwrap(),
            Some(expected),
            "an upgrade interrupted mid-publish must still report its outcome"
        );
        // Consuming the result must not leave the sidecar behind for a re-read.
        assert!(!sidecar.is_file());
        assert_eq!(take_install_result_at(&path).unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn real_msi_identity_is_verified_when_fixture_is_configured() {
        let Some(path) = std::env::var_os("HLS_TEST_UPDATE_MSI").map(PathBuf::from) else {
            return;
        };
        let version = std::env::var("HLS_TEST_UPDATE_VERSION").unwrap_or_else(|_| "7.0.0".into());
        let identity = read_msi_identity(&path).unwrap();
        assert_eq!(identity.product_name, EXPECTED_PRODUCT_NAME);
        assert_eq!(identity.product_version, version);
        if std::env::var("HLS_TEST_EXPECT_PER_USER").ok().as_deref() == Some("1") {
            assert!(verify_installer_identity(&path, &version).is_ok());
        } else {
            assert!(!identity.per_user);
            assert!(verify_installer_identity(&path, &version)
                .unwrap_err()
                .contains("每用户安装上下文"));
        }
    }
}
