//! GitHub release check. Download starts only after the user confirms.

use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_API: &str = "https://api.github.com/repos/ciaooo55/hls-downloader/releases/latest";

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub newer: bool,
    pub html_url: String,
    pub notes: String,
    pub installer_url: String,
    pub installer_name: String,
    pub expected_sha256: String,
    pub installer_size: u64,
}

fn last_info() -> &'static Mutex<Option<UpdateInfo>> {
    static LAST: OnceLock<Mutex<Option<UpdateInfo>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

pub fn remember_update(info: UpdateInfo) {
    if let Ok(mut slot) = last_info().lock() {
        *slot = Some(info);
    }
}

pub fn last_update() -> Option<UpdateInfo> {
    last_info().lock().ok().and_then(|slot| slot.clone())
}

pub fn is_newer_version(remote: &str, current: &str) -> bool {
    let remote = parse_version(remote);
    let current = parse_version(current);
    remote > current
}

pub fn parse_github_release(json: &str, current: &str) -> Result<UpdateInfo, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("update JSON: {error}"))?;
    let latest = value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .trim_start_matches('v')
        .to_string();
    if latest.is_empty() {
        return Err("GitHub 最新版本信息中缺少版本号".into());
    }
    let html_url = value
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("https://github.com/ciaooo55/hls-downloader/releases/latest")
        .to_string();
    let notes = value
        .get("body")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .chars()
        .take(400)
        .collect();
    let (installer_url, installer_name, expected_sha256, installer_size) =
        pick_installer_asset(&value, &latest);
    let info = UpdateInfo {
        newer: is_newer_version(&latest, current),
        current: current.trim().trim_start_matches('v').to_string(),
        latest,
        html_url,
        notes,
        installer_url,
        installer_name,
        expected_sha256,
        installer_size,
    };
    remember_update(info.clone());
    Ok(info)
}

pub fn pick_installer_asset(
    release: &serde_json::Value,
    latest: &str,
) -> (String, String, String, u64) {
    let Some(assets) = release.get("assets").and_then(|item| item.as_array()) else {
        return Default::default();
    };
    let expected_name = format!("HLSDownloader-{latest}-Windows-x64.msi").to_ascii_lowercase();
    let mut ranked: Vec<(u8, String, String, String, u64)> = Vec::new();
    for asset in assets {
        let name = asset
            .get("name")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string();
        let url = asset
            .get("browser_download_url")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string();
        let size = asset
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let digest = asset
            .get("digest")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_sha256_digest)
            .unwrap_or_default();
        if url.is_empty()
            || name.is_empty()
            || !installer_url_allowed(&url)
            || size == 0
            || digest.is_empty()
        {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".msi") || !lower.contains("windows-x64") {
            continue;
        }
        let rank = if lower == expected_name {
            0
        } else if lower.starts_with("hlsdownloader-") {
            1
        } else {
            2
        };
        ranked.push((rank, url, name, digest, size));
    }
    ranked.sort_by_key(|(rank, _, _, _, _)| *rank);
    ranked
        .into_iter()
        .next()
        .map(|(_, url, name, digest, size)| (url, name, digest, size))
        .unwrap_or_default()
}

fn parse_sha256_digest(value: &str) -> Option<String> {
    let digest = value.trim().strip_prefix("sha256:")?.to_ascii_lowercase();
    (digest.len() == 64 && digest.chars().all(|ch| ch.is_ascii_hexdigit())).then_some(digest)
}

pub fn check_for_update(current: &str) -> Result<UpdateInfo, String> {
    let mut headers = std::collections::HashMap::new();
    headers.insert("User-Agent".into(), "hls-downloader-v7".into());
    headers.insert("Accept".into(), "application/vnd.github+json".into());
    let (status, body) = crate::http_engine::fetch_bytes(LATEST_API, &headers, "")
        .map_err(|error| error.to_string())?;
    if status != 200 {
        return Err(format!("GitHub releases HTTP {status}"));
    }
    parse_github_release(&String::from_utf8_lossy(&body), current)
}

pub fn download_installer(info: &UpdateInfo) -> Result<std::path::PathBuf, String> {
    if info.installer_url.is_empty() {
        return Err("GitHub 发布没有 Windows 安装包。请打开发布页手动下载。".into());
    }
    if !installer_url_allowed(&info.installer_url) {
        return Err("安装包地址不是 GitHub 发布资源".into());
    }
    if info.installer_size == 0
        || parse_sha256_digest(&format!("sha256:{}", info.expected_sha256)).is_none()
    {
        return Err(
            "发布资源缺少有效的大小或 SHA-256，已阻止自动升级。请打开发布页手动检查。".into(),
        );
    }
    let name = sanitize_installer_name(&info.installer_name, &info.latest);
    if std::path::Path::new(&name).components().count() != 1
        || !name.to_ascii_lowercase().ends_with(".msi")
    {
        return Err("自动升级只接受 Windows x64 MSI 安装包".into());
    }
    let version_dir = update_root().join(sanitize_version(&info.latest));
    std::fs::create_dir_all(&version_dir).map_err(|error| format!("创建更新目录失败: {error}"))?;
    let path = version_dir.join(&name);
    if installer_matches(&path, info.installer_size, &info.expected_sha256)? {
        return Ok(path);
    }
    let _ = std::fs::remove_file(&path);
    let part = version_dir.join(format!("{name}.part"));
    let _ = std::fs::remove_file(&part);
    let control = version_dir.join("download.control");
    let progress = version_dir.join("download-progress.json");
    std::fs::write(&control, "run").map_err(|error| format!("初始化更新下载失败: {error}"))?;
    let mut headers = std::collections::HashMap::new();
    headers.insert("User-Agent".into(), "hls-downloader-v7".into());
    headers.insert("Accept".into(), "application/octet-stream".into());
    let job = crate::http_engine::Job {
        url: info.installer_url.clone(),
        headers,
        output: part.clone(),
        connections: 1,
        chunk_bytes: 8 * 1024 * 1024,
        total: info.installer_size,
        sequential: true,
        resume_from: 0,
        proxy: String::new(),
        resource_key: info.installer_url.clone(),
        etag: String::new(),
        last_modified: String::new(),
        control,
        progress,
        method: "GET".into(),
        body_path: PathBuf::new(),
        mirrors: Vec::new(),
        replay_json: String::new(),
    };
    if let Err(error) = crate::http_engine::run_job(&job) {
        let _ = std::fs::remove_file(&part);
        return Err(format!("下载安装包失败: {error}"));
    }
    if !installer_matches(&part, info.installer_size, &info.expected_sha256)? {
        let _ = std::fs::remove_file(&part);
        return Err("安装包大小或 SHA-256 校验失败，未启动升级".into());
    }
    std::fs::rename(&part, &path).map_err(|error| format!("发布更新安装包失败: {error}"))?;
    mark_downloaded_from_internet(&path);
    Ok(path)
}

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
    if identity.product_version != expected_version.trim().trim_start_matches('v') {
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
        || {
            let status = std::process::Command::new("msiexec.exe")
                .args(msiexec_args(path, log_path))
                .status()
                .map_err(|error| format!("运行覆盖安装失败: {error}"))?;
            Ok(status.code().unwrap_or(-1))
        },
        || std::thread::sleep(Duration::from_secs(10)),
    )
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
    let _ = std::fs::remove_file(path);
    std::fs::rename(&temporary, path).map_err(|error| format!("发布升级结果失败: {error}"))
}

pub fn take_install_result() -> Result<Option<InstallResult>, String> {
    let path = update_root().join("last-install-result.json");
    if !path.is_file() {
        return Ok(None);
    }
    let data = std::fs::read(&path).map_err(|error| format!("读取升级结果失败: {error}"))?;
    let result =
        serde_json::from_slice(&data).map_err(|error| format!("解析升级结果失败: {error}"))?;
    std::fs::remove_file(&path).map_err(|error| format!("确认升级结果失败: {error}"))?;
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

fn update_root() -> PathBuf {
    std::env::temp_dir().join("HLSDownloader").join("updates")
}

fn sanitize_version(version: &str) -> String {
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

fn installer_matches(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let size = std::fs::metadata(path)
        .map_err(|error| format!("读取安装包大小失败: {error}"))?
        .len();
    if size != expected_size {
        return Ok(false);
    }
    Ok(crate::checksum::verify_file(path, &format!("sha256:{expected_sha256}")).is_ok())
}

fn mark_downloaded_from_internet(path: &Path) {
    #[cfg(windows)]
    {
        let zone = format!("{}:Zone.Identifier", path.display());
        let _ = std::fs::write(zone, "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
}

pub(crate) fn installer_url_allowed(url: &str) -> bool {
    let url = url.trim().trim_start_matches('\u{feff}');
    if !crate::http_engine::http_fetch_url_allowed(url) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://github.com/ciaooo55/hls-downloader/")
        || lower.starts_with("https://objects.githubusercontent.com/")
        || lower.starts_with("https://github-releases.githubusercontent.com/")
        || lower.starts_with("https://release-assets.githubusercontent.com/")
}

pub(crate) fn sanitize_installer_name(name: &str, latest: &str) -> String {
    let fallback = format!(
        "HLSDownloader-{}-Windows-x64.msi",
        latest
            .chars()
            .map(
                |ch| if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                    ch
                } else {
                    '_'
                }
            )
            .collect::<String>()
    );
    let base = name
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .filter(|item| !item.is_empty())
        .unwrap_or(&fallback);
    let cleaned: String = base
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let lower = cleaned.to_ascii_lowercase();
    if lower.ends_with(".msi") {
        cleaned
    } else if cleaned.is_empty() {
        fallback
    } else {
        format!("{cleaned}.msi")
    }
}

fn parse_version(value: &str) -> Vec<u32> {
    value
        .trim()
        .trim_start_matches('v')
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_dotted_versions() {
        assert!(is_newer_version("v1.2.0", "1.1.9"));
        assert!(!is_newer_version("1.2", "1.2.0"));
        assert!(!is_newer_version("1.1.9", "1.2.0"));
    }

    #[test]
    fn parses_github_payload() {
        let json = r#"{"tag_name":"v7.1.0","html_url":"https://github.com/ciaooo55/hls-downloader/releases/tag/v7.1.0","body":"fixes","assets":[{"name":"HLSDownloader-7.1.0-Windows-x64-Setup.exe","browser_download_url":"https://github.com/ciaooo55/hls-downloader/releases/download/v7.1.0/Setup.exe","size":5,"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"name":"HLSDownloader-7.1.0-Windows-x64.msi","browser_download_url":"https://github.com/ciaooo55/hls-downloader/releases/download/v7.1.0/HLSDownloader.msi","size":154453055,"digest":"sha256:a761c958bffea479f736c987be5e335bce6914ddb0267ef6fdd1a3673ac661b0"}]}"#;
        let info = parse_github_release(json, "6.0.0-dev").unwrap();
        assert_eq!(info.latest, "7.1.0");
        assert!(info.newer);
        assert!(info.installer_name.ends_with(".msi"));
        assert_eq!(info.installer_size, 154453055);
        assert_eq!(
            info.expected_sha256,
            "a761c958bffea479f736c987be5e335bce6914ddb0267ef6fdd1a3673ac661b0"
        );
        assert_eq!(
            info.installer_url,
            "https://github.com/ciaooo55/hls-downloader/releases/download/v7.1.0/HLSDownloader.msi"
        );
    }

    #[test]
    fn automatic_update_rejects_assets_without_digest_or_size() {
        let json = r#"{"tag_name":"v7.1.0","assets":[{"name":"HLSDownloader-7.1.0-Windows-x64.msi","browser_download_url":"https://github.com/ciaooo55/hls-downloader/releases/download/v7.1.0/HLSDownloader.msi","size":0},{"name":"HLSDownloader-7.1.0-Windows-x64-Setup.exe","browser_download_url":"https://github.com/ciaooo55/hls-downloader/releases/download/v7.1.0/Setup.exe","size":9,"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
        let info = parse_github_release(json, "7.0.0").unwrap();
        assert!(info.newer);
        assert!(info.installer_url.is_empty());
        assert_eq!(info.installer_size, 0);
    }

    #[test]
    fn cached_installer_is_reused_only_when_size_and_sha256_match() {
        let path =
            std::env::temp_dir().join(format!("hls-updater-cache-test-{}.msi", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let sha256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(installer_matches(&path, 3, sha256).unwrap());
        assert!(!installer_matches(&path, 4, sha256).unwrap());
        assert!(!installer_matches(&path, 3, &"0".repeat(64)).unwrap());
        std::fs::remove_file(path).unwrap();
    }

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

    #[test]
    fn installer_identity_rejects_another_product_version_or_upgrade_family() {
        let valid = InstallerIdentity {
            product_name: EXPECTED_PRODUCT_NAME.into(),
            product_version: "7.1.0".into(),
            upgrade_code: EXPECTED_UPGRADE_CODE.to_ascii_lowercase(),
            per_user: true,
        };
        assert!(validate_installer_identity_fields(&valid, "v7.1.0").is_ok());
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

    #[test]
    fn installer_assets_stay_in_temp_and_on_github() {
        assert!(installer_url_allowed(
            "https://github.com/ciaooo55/hls-downloader/releases/download/v6.1.0/Setup.exe"
        ));
        assert!(!installer_url_allowed(
            "https://github.com/evil/malware/releases/download/v1/Setup.exe"
        ));
        assert!(!installer_url_allowed("https://evil.example/Setup.exe"));
        assert_eq!(
            sanitize_installer_name(r"..\..\Startup\evil.msi", "7.1.0"),
            "evil.msi"
        );
        assert_eq!(
            sanitize_installer_name(r"C:\Windows\payload.msi", "7.1.0"),
            "payload.msi"
        );
        assert_eq!(
            std::path::Path::new(&sanitize_installer_name(r"..\..\Startup\evil.msi", "7.1.0"))
                .components()
                .count(),
            1
        );
    }
}
