//! Verified MSI execution and result reporting, independent of download engines.
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

pub const EXPECTED_PRODUCT_NAME: &str = "HLS Downloader";
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
    verify_package_identity(path, expected_version, None)
}

fn verify_package_identity(
    path: &Path,
    expected_version: &str,
    package: Option<&std::fs::File>,
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
        verify_installer_authenticode(path, package)?;
        let identity = read_msi_identity(path)?;
        validate_installer_identity_fields(&identity, expected_version)?;
        Ok(identity)
    }
    #[cfg(not(windows))]
    {
        let _ = (expected_version, package);
        Err("MSI 身份校验只适用于 Windows".into())
    }
}

pub fn launch_update_helper(
    path: &Path,
    expected_version: &str,
    workbench_pid: u32,
) -> Result<UpdaterLaunch, String> {
    let absolute =
        std::path::absolute(path).map_err(|error| format!("读取升级安装包路径失败: {error}"))?;
    let path = absolute.as_path();
    let identity = verify_installer_identity(path, expected_version)?;
    if !identity.per_user {
        return Err("升级包不是当前产品使用的每用户安装方式".into());
    }
    let source = locate_updater_executable()?;
    let mut source =
        std::fs::File::open(source).map_err(|error| format!("打开独立更新助手失败: {error}"))?;
    let mut attempt = 0u32;
    let (helper_path, mut copy) = loop {
        let candidate = path.with_file_name(format!(
            "HLSDownloaderUpdater-{}-{}-{attempt}.exe",
            sanitize_version(expected_version),
            std::process::id()
        ));
        match std::fs::File::create_new(&candidate) {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 128 => {
                attempt += 1
            }
            Err(error) => return Err(format!("创建独立更新助手失败: {error}")),
        }
    };
    let copied = std::io::copy(&mut source, &mut copy);
    drop(copy);
    if let Err(error) = copied {
        let _ = std::fs::remove_file(&helper_path);
        return Err(format!("准备独立更新助手失败: {error}"));
    }
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
    if let Err(error) = command.spawn() {
        let _ = std::fs::remove_file(&helper_path);
        return Err(format!("启动独立更新助手失败: {error}"));
    }
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
fn verify_installer_authenticode(
    path: &Path,
    package: Option<&std::fs::File>,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_DISABLE_MD2_MD4,
        WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if path_wide[..path_wide.len() - 1].contains(&0) {
        return Err("升级安装包路径包含无效空字符".into());
    }
    // SAFETY: `WINTRUST_FILE_INFO` is a plain C struct for which all-zero initialization is valid.
    let mut file: WINTRUST_FILE_INFO = unsafe { std::mem::zeroed() };
    file.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
    file.pcwszFilePath = path_wide.as_ptr();
    if let Some(package) = package {
        file.hFile = package.as_raw_handle();
    }

    // SAFETY: `WINTRUST_DATA` is a plain C struct/union for which all-zero initialization is valid.
    let mut trust: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    trust.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    trust.dwUIChoice = WTD_UI_NONE;
    trust.fdwRevocationChecks = WTD_REVOKE_NONE;
    trust.dwUnionChoice = WTD_CHOICE_FILE;
    trust.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };
    trust.dwStateAction = WTD_STATEACTION_VERIFY;
    trust.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_DISABLE_MD2_MD4;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;

    // SAFETY: `action` is a valid `GUID`, `trust` is initialized for `WTD_CHOICE_FILE`, and
    // `file` (`path_wide` + optional open `package` file handle) outlives both `WinVerifyTrust`
    // calls (`WTD_STATEACTION_VERIFY` and the paired `WTD_STATEACTION_CLOSE` below).
    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            &mut trust as *mut _ as *mut core::ffi::c_void,
        )
    };
    let result = if status == 0 {
        verify_release_signer(trust.hWVTStateData)
    } else {
        Err(format!(
            "升级安装包 Authenticode 信任校验失败（WinVerifyTrust 0x{:08X}）",
            status as u32
        ))
    };
    trust.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: Releases any provider state allocated in `trust.hWVTStateData` by the preceding
    // `WTD_STATEACTION_VERIFY` call, as required on both success and failure paths.
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            &mut trust as *mut _ as *mut core::ffi::c_void,
        );
    }
    result
}

#[cfg(windows)]
fn verify_release_signer(state: windows_sys::Win32::Foundation::HANDLE) -> Result<(), String> {
    use core::ffi::c_void;
    use windows_sys::Win32::Security::Cryptography::{
        CertGetCertificateContextProperty, CERT_SHA1_HASH_PROP_ID,
    };
    use windows_sys::Win32::Security::WinTrust::CRYPT_PROVIDER_SGNR;
    // These two accessors use opaque WinTrust provider data. Linking to the DLL
    // directly avoids depending on the SDK's optional catalog/SIP structure layouts.
    #[link(name = "wintrust", kind = "raw-dylib")]
    extern "system" {
        fn WTHelperProvDataFromStateData(state: *mut c_void) -> *mut c_void;
        fn WTHelperGetProvSignerFromChain(
            provider: *mut c_void,
            signer: u32,
            countersigner: i32,
            index: u32,
        ) -> *mut CRYPT_PROVIDER_SGNR;
    }
    if state.is_null() {
        return Err("无法读取已验证的 Authenticode provider 状态".into());
    }
    // SAFETY: `state` is the non-null `hWVTStateData` from a succeeded `WinVerifyTrust` call and
    // remains open until `verify_installer_authenticode` runs `WTD_STATEACTION_CLOSE`.
    let provider = unsafe { WTHelperProvDataFromStateData(state) };
    if provider.is_null() {
        return Err("无法读取已验证的 Authenticode provider 状态".into());
    }
    // SAFETY: `provider` is a live `CRYPT_PROVIDER_DATA` pointer owned by `state`; `as_ref()`
    // converts a non-null `CRYPT_PROVIDER_SGNR` pointer into a shared reference bounded by this scope.
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0).as_ref() }
        .ok_or("无法读取已验证的 Authenticode 主签名者")?;
    if signer.csCertChain == 0 || signer.pasCertChain.is_null() {
        return Err("已验证的 Authenticode 签名者缺少证书".into());
    }
    // SAFETY: `signer.csCertChain > 0` and `signer.pasCertChain` is non-null, so element 0 of the
    // WinTrust certificate chain array (the leaf `CRYPT_PROVIDER_CERT`) is valid to read.
    let leaf = unsafe { (*signer.pasCertChain).pCert };
    let mut hash = [0u8; 20];
    let mut size = hash.len() as u32;
    if leaf.is_null() {
        return Err("无法读取 Authenticode leaf 证书指纹".into());
    }
    // SAFETY: `leaf` is a non-null `CERT_CONTEXT` owned by the live WinTrust state, and `hash` is
    // a 20-byte writable SHA-1 buffer matching `size`.
    let got_hash = unsafe {
        CertGetCertificateContextProperty(
            leaf,
            CERT_SHA1_HASH_PROP_ID,
            hash.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if got_hash == 0 || size as usize != hash.len() {
        return Err("无法读取 Authenticode leaf 证书指纹".into());
    }
    let candidate = hash
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    signer_policy::authorize_release_signer(
        &candidate,
        option_env!("HLS_V7_SIGN_CERT_THUMBPRINT"),
        signer_policy::SIGNER_TRUST_JSON,
    )
}

#[cfg(any(windows, test))]
mod signer_policy {
    use serde::Deserialize;
    use std::collections::HashSet;

    pub(super) const SIGNER_TRUST_JSON: &str =
        include_str!("../../../config/release/update-signer-trust.json");
    const EXPECTED_TRUST_IDENTITY: &str = "authenticode_leaf_certificate_sha1";
    const EXPECTED_TRUST_POLICY: &str = "formal-build-signer-or-explicit-rollover";

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SignerTrustContract {
        schema: u32,
        identity: String,
        policy: String,
        rollover_signers: Vec<RolloverSigner>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RolloverSigner {
        thumbprint: String,
        reason: String,
    }

    fn normalize_thumbprint(value: &str) -> Result<String, String> {
        let normalized: String = value
            .chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .map(|ch| ch.to_ascii_uppercase())
            .collect();
        if normalized.len() != 40 || !normalized.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err("更新签名证书指纹必须是 40 位十六进制 SHA-1".into());
        }
        Ok(normalized)
    }

    fn parse_signer_trust(json: &str) -> Result<HashSet<String>, String> {
        let contract: SignerTrustContract = serde_json::from_str(json)
            .map_err(|error| format!("解析更新签名信任配置失败: {error}"))?;
        if contract.schema != 1 {
            return Err(format!("不支持的更新签名信任 schema: {}", contract.schema));
        }
        if contract.identity != EXPECTED_TRUST_IDENTITY {
            return Err(format!("不支持的更新签名身份类型: {}", contract.identity));
        }
        if contract.policy != EXPECTED_TRUST_POLICY {
            return Err(format!("不支持的更新签名信任策略: {}", contract.policy));
        }

        let mut signers = HashSet::new();
        for signer in contract.rollover_signers {
            if signer.reason.trim().is_empty() {
                return Err("更新签名 rollover 条目缺少审核原因".into());
            }
            let thumbprint = normalize_thumbprint(&signer.thumbprint)?;
            if !signers.insert(thumbprint) {
                return Err("更新签名 rollover 指纹重复".into());
            }
        }
        Ok(signers)
    }

    pub(super) fn authorize_release_signer(
        candidate: &str,
        primary: Option<&str>,
        trust_json: &str,
    ) -> Result<(), String> {
        let candidate = normalize_thumbprint(candidate)?;
        let primary = primary
            .ok_or_else(|| "正式发布构建缺少嵌入的 HLS_V7_SIGN_CERT_THUMBPRINT".to_string())
            .and_then(normalize_thumbprint)?;
        let rollover = parse_signer_trust(trust_json)?;
        if candidate == primary || rollover.contains(&candidate) {
            return Ok(());
        }
        Err(format!(
            "升级安装包签名者不是受信任的 HLS Downloader 发布证书: {candidate}"
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const PRIMARY: &str = "00112233445566778899AABBCCDDEEFF00112233";
        const ROLLOVER: &str = "112233445566778899AABBCCDDEEFF0011223344";
        const OTHER: &str = "FFEEDDCCBBAA99887766554433221100FFEEDDCC";

        fn rollover_json() -> String {
            format!(
                r#"{{"schema":1,"identity":"{EXPECTED_TRUST_IDENTITY}","policy":"{EXPECTED_TRUST_POLICY}","rollover_signers":[{{"thumbprint":"{ROLLOVER}","reason":"planned certificate rotation"}}]}}"#
            )
        }

        #[test]
        fn primary_and_rollover_signers_are_accepted_but_unrelated_signer_is_rejected() {
            assert!(authorize_release_signer(PRIMARY, Some(PRIMARY), SIGNER_TRUST_JSON).is_ok());
            assert!(authorize_release_signer(ROLLOVER, Some(PRIMARY), &rollover_json()).is_ok());
            let error =
                authorize_release_signer(OTHER, Some(PRIMARY), &rollover_json()).unwrap_err();
            assert!(error.contains("不是受信任"));
        }

        #[test]
        fn signer_trust_requires_a_formal_primary_identity() {
            let error = authorize_release_signer(PRIMARY, None, SIGNER_TRUST_JSON).unwrap_err();
            assert!(error.contains("HLS_V7_SIGN_CERT_THUMBPRINT"));
        }

        #[test]
        fn signer_trust_rejects_malformed_duplicate_or_unexplained_rollovers() {
            assert!(normalize_thumbprint("not-a-thumbprint").is_err());
            let duplicate = format!(
                r#"{{"schema":1,"identity":"{EXPECTED_TRUST_IDENTITY}","policy":"{EXPECTED_TRUST_POLICY}","rollover_signers":[{{"thumbprint":"{ROLLOVER}","reason":"first"}},{{"thumbprint":"{ROLLOVER}","reason":"duplicate"}}]}}"#
            );
            assert!(parse_signer_trust(&duplicate).unwrap_err().contains("重复"));
            let no_reason = format!(
                r#"{{"schema":1,"identity":"{EXPECTED_TRUST_IDENTITY}","policy":"{EXPECTED_TRUST_POLICY}","rollover_signers":[{{"thumbprint":"{ROLLOVER}","reason":"   "}}]}}"#
            );
            assert!(parse_signer_trust(&no_reason)
                .unwrap_err()
                .contains("审核原因"));
        }

        #[test]
        fn signer_trust_rejects_unknown_contract_values() {
            let wrong_schema = format!(
                r#"{{"schema":2,"identity":"{EXPECTED_TRUST_IDENTITY}","policy":"{EXPECTED_TRUST_POLICY}","rollover_signers":[]}}"#
            );
            assert!(parse_signer_trust(&wrong_schema)
                .unwrap_err()
                .contains("schema"));
            let wrong_identity = format!(
                r#"{{"schema":1,"identity":"subject_name","policy":"{EXPECTED_TRUST_POLICY}","rollover_signers":[]}}"#
            );
            assert!(parse_signer_trust(&wrong_identity)
                .unwrap_err()
                .contains("身份类型"));
        }

        #[test]
        fn primary_signer_still_requires_a_valid_local_trust_contract() {
            let malformed = r#"{"schema":1,"identity":"authenticode_leaf_certificate_sha1","policy":"formal-build-signer-or-explicit-rollover","rollover_signers":["#;
            let error = authorize_release_signer(PRIMARY, Some(PRIMARY), malformed).unwrap_err();
            assert!(error.contains("解析更新签名信任配置失败"));
        }
    }
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
    read_optional_msi_property(path, property)?.ok_or_else(|| format!("MSI 缺少 {property}"))
}

#[cfg(windows)]
fn read_optional_msi_property(path: &Path, property: &str) -> Result<Option<String>, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::ApplicationInstallationAndServicing::{
        MsiCloseHandle, MsiDatabaseOpenViewW, MsiOpenDatabaseW, MsiRecordGetStringW,
        MsiViewExecute, MsiViewFetch, MSIHANDLE,
    };
    const ERROR_SUCCESS: u32 = 0;
    const ERROR_MORE_DATA: u32 = 234;

    if property.contains('\0') {
        return Err("MSI 属性名包含无效空字符".into());
    }
    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if path_wide[..path_wide.len() - 1].contains(&0) {
        return Err("MSI 路径包含无效空字符".into());
    }
    let mut database: MSIHANDLE = 0;
    // SAFETY: `path_wide` is a valid NUL-terminated UTF-16 path with no interior NULs, `null()` is
    // the Win32 `MSIDBOPEN_READONLY` (`(LPCWSTR)0`) persistence constant, and `&mut database`
    // receives an `MSIHANDLE` closed unconditionally at the end of this function.
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
        // SAFETY: `database` is an open read-only `MSIHANDLE`, `query_wide` is a NUL-terminated
        // UTF-16 SQL query without interior NULs, and `&mut view` receives an `MSIHANDLE` closed below.
        let status = unsafe { MsiDatabaseOpenViewW(database, query_wide.as_ptr(), &mut view) };
        if status != ERROR_SUCCESS {
            return Err(format!("无法读取 MSI {property}（错误 {status}）"));
        }
        // SAFETY: `view` is the open `MSIHANDLE` returned by `MsiDatabaseOpenViewW`; `0` indicates
        // no parameter record is bound.
        let status = unsafe { MsiViewExecute(view, 0) };
        if status != ERROR_SUCCESS {
            return Err(format!("无法执行 MSI {property} 查询（错误 {status}）"));
        }
        let mut record: MSIHANDLE = 0;
        // SAFETY: `view` has been executed and `&mut record` is a valid out-pointer for the fetched
        // `MSIHANDLE`, closed unconditionally when `status == ERROR_SUCCESS`.
        let status = unsafe { MsiViewFetch(view, &mut record) };
        if status == windows_sys::Win32::Foundation::ERROR_NO_MORE_ITEMS {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(format!("无法读取 MSI {property} 记录（错误 {status}）"));
        }
        let value = (|| {
            let mut required = 0u32;
            let mut empty = 0u16;
            // SAFETY: Per `MsiRecordGetStringW` contract, passing a non-null 1-`WCHAR` buffer
            // (`&mut empty`) with `required == 0` writes only the NUL terminator to `empty` and
            // stores the character count (excluding NUL) in `required`.
            let first = unsafe { MsiRecordGetStringW(record, 1, &mut empty, &mut required) };
            if first != ERROR_MORE_DATA && first != ERROR_SUCCESS {
                return Err(format!("无法读取 MSI {property}（错误 {first}）"));
            }
            // Only short identity and installation-context properties are read here.
            if required > u32::from(u16::MAX) {
                return Err(format!("MSI {property} 属性过长"));
            }
            let mut buffer = vec![0u16; required as usize + 1];
            let mut capacity = buffer.len() as u32;
            // SAFETY: `buffer` has `capacity` (`required + 1`) writable `u16` elements for the
            // string plus trailing NUL, and `record` is live.
            let status =
                unsafe { MsiRecordGetStringW(record, 1, buffer.as_mut_ptr(), &mut capacity) };
            if status != ERROR_SUCCESS {
                return Err(format!("无法读取 MSI {property}（错误 {status}）"));
            }
            let written = (capacity as usize).min(buffer.len());
            Ok(String::from_utf16_lossy(&buffer[..written]))
        })();
        // SAFETY: `record` was returned by a succeeded `MsiViewFetch` and is closed once here.
        unsafe { MsiCloseHandle(record) };
        value.map(Some)
    })();
    if view != 0 {
        // SAFETY: `view` is a live `MSIHANDLE` opened by `MsiDatabaseOpenViewW` and closed once.
        unsafe { MsiCloseHandle(view) };
    }
    // SAFETY: `database` is a live `MSIHANDLE` opened by `MsiOpenDatabaseW` and closed once.
    unsafe { MsiCloseHandle(database) };
    result
}

#[cfg(windows)]
fn msi_is_per_user(path: &Path) -> Result<bool, String> {
    let all_users = read_optional_msi_property(path, "ALLUSERS")?;
    let per_user = read_optional_msi_property(path, "MSIINSTALLPERUSER")?;
    Ok(msi_properties_are_per_user(
        all_users.as_deref(),
        per_user.as_deref(),
    ))
}

#[cfg(any(windows, test))]
fn msi_properties_are_per_user(all_users: Option<&str>, per_user: Option<&str>) -> bool {
    matches!(all_users, None | Some("")) || (all_users == Some("2") && per_user == Some("1"))
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

#[cfg(windows)]
fn lock_update_package(msi: &Path) -> Result<std::fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(msi)
        .map_err(|error| format!("无法锁定升级安装包: {error}"))
}

pub fn run_update_helper(args: impl IntoIterator<Item = OsString>) -> Result<u8, String> {
    run_helper(args, None::<fn(&Path) -> Result<(), String>>)
}

/// Adds a caller check before the mandatory release signer and MSI identity checks.
pub fn run_update_helper_with_verifier<F>(
    args: impl IntoIterator<Item = OsString>,
    verify_msi: F,
) -> Result<u8, String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    run_helper(args, Some(verify_msi))
}

fn run_helper<F>(
    args: impl IntoIterator<Item = OsString>,
    additional_check: Option<F>,
) -> Result<u8, String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let args = parse_helper_args(args)?;
    run_update_helper_with_hooks(
        &args,
        |path| {
            #[cfg(windows)]
            let package = lock_update_package(path)?;
            if let Some(check) = additional_check {
                check(path)?;
            }
            #[cfg(windows)]
            {
                Ok(package)
            }
            #[cfg(not(windows))]
            Ok(())
        },
        |path, version, package| {
            #[cfg(windows)]
            {
                verify_package_identity(path, version, Some(package))
            }
            #[cfg(not(windows))]
            {
                let _ = package;
                verify_installer_identity(path, version)
            }
        },
        wait_for_product_shutdown,
        run_msiexec,
        restart_workbench,
    )
}

fn run_update_helper_with_hooks<G, V, I, W, R, S>(
    args: &HelperArgs,
    verify_msi: V,
    verify_identity: I,
    mut wait_shutdown: W,
    run_installer: R,
    mut restart: S,
) -> Result<u8, String>
where
    V: FnOnce(&Path) -> Result<G, String>,
    I: FnOnce(&Path, &str, &G) -> Result<InstallerIdentity, String>,
    W: FnMut(&HelperArgs) -> Result<(), String>,
    R: FnOnce(&Path, &Path) -> Result<i32, String>,
    S: FnMut(&HelperArgs),
{
    let mut shutdown_attempted = false;
    let outcome: Result<u8, String> = (|| {
        // Keep the verified package locked until msiexec has returned, including during shutdown.
        let package = verify_msi(&args.msi)?;
        let identity = verify_identity(&args.msi, &args.version, &package)?;
        if !identity.per_user {
            return Err("升级包安装上下文不是每用户安装".into());
        }
        shutdown_attempted = true;
        wait_shutdown(args)?;
        let exit_code = run_installer(&args.msi, &args.log)?;
        let (status, message) = installer_exit_status(exit_code);
        let result = InstallResult {
            version: args.version.clone(),
            status: status.into(),
            exit_code,
            message: message.into(),
            install_log: args.log.display().to_string(),
        };
        write_install_result(&args.result, &result)?;
        restart(args);
        Ok(if status == "success" { 0 } else { 1 })
    })();
    match outcome {
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
            if !shutdown_attempted && (args.core_pid != 0 || args.workbench_pid != 0) {
                let _ = wait_shutdown(args);
            }
            restart(args);
            Err(error)
        }
    }
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
fn msiexec_command(args: &[OsString]) -> Result<std::process::Command, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut directory = vec![0u16; 32768];
    // SAFETY: `directory` provides `directory.len()` writable `u16` units; `GetSystemDirectoryW`
    // returns `0` on failure or `>= directory.len()` if truncated, both rejected below.
    let length =
        unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) } as usize;
    if length == 0 || length >= directory.len() {
        return Err("无法定位系统 Windows Installer".into());
    }
    let path = PathBuf::from(OsString::from_wide(&directory[..length])).join("msiexec.exe");
    let mut command = std::process::Command::new(path);
    command.args(args);
    Ok(command)
}

#[cfg(windows)]
fn run_msiexec_once(args: &[OsString]) -> Result<i32, String> {
    let status = msiexec_command(args)?
        .status()
        .map_err(|error| format!("启动 Windows Installer 失败: {error}"))?;
    Ok(status.code().unwrap_or(-1))
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

// Readers also acknowledge by deleting the record. Hold one stable OS lock
// across each transaction so an acknowledgement cannot delete a newer result.
fn lock_install_result(path: &Path) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("json.lock"))
        .map_err(|error| format!("打开升级结果锁失败: {error}"))?;
    file.lock()
        .map_err(|error| format!("锁定升级结果失败: {error}"))?;
    Ok(file)
}

fn write_install_result(path: &Path, result: &InstallResult) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建升级结果目录失败: {error}"))?;
    }
    let _lock = lock_install_result(path)?;
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
    let sidecar = path.with_extension("json.part");
    let exists = |p: &Path| {
        p.try_exists()
            .map_err(|error| format!("读取升级结果失败: {error}"))
    };
    if !exists(path)? && !exists(&sidecar)? {
        return Ok(None);
    }
    let _lock = lock_install_result(path)?;
    let source = if exists(path)? {
        path.to_path_buf()
    } else {
        sidecar
    };
    if !exists(&source)? {
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
            let ui_running =
                process_running_in_install_root(args.workbench_pid, &args.install_root);
            if ui_running {
                request_process_window_close(args.workbench_pid);
            }
            let core_running = process_running_in_install_root(args.core_pid, &args.install_root);
            terminate_auxiliary_processes(&args.install_root);
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
fn process_running_in_install_root(pid: u32, install_root: &Path) -> bool {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    if pid == 0 || pid == std::process::id() || install_root.as_os_str().is_empty() {
        return false;
    }
    // SAFETY: `OpenProcess` opens a fresh process kernel handle with non-inheritable query/sync
    // access (`0`), or returns null if `pid` does not exist or is inaccessible.
    let handle = unsafe {
        OpenProcess(
            SYNCHRONIZE_ACCESS | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return false;
    }
    // SAFETY: `handle` is non-null and open with `SYNCHRONIZE` access; timeout `0` polls exit
    // state without blocking.
    let running = unsafe { WaitForSingleObject(handle, 0) == WAIT_TIMEOUT };
    let belongs = if running {
        let mut image = vec![0u16; 32768];
        let mut length = image.len() as u32;
        // SAFETY: `handle` has `PROCESS_QUERY_LIMITED_INFORMATION`, `image` has `length` (`32768`)
        // writable `u16` elements, and on success `length` holds the character count `< 32768`.
        let got_name =
            unsafe { QueryFullProcessImageNameW(handle, 0, image.as_mut_ptr(), &mut length) };
        if got_name != 0 {
            let written = (length as usize).min(image.len());
            let path = PathBuf::from(OsString::from_wide(&image[..written]));
            path_is_under_install_root(&path, install_root)
        } else {
            false
        }
    } else {
        false
    };
    // SAFETY: `handle` is uniquely owned by this function and closed once before returning.
    unsafe { CloseHandle(handle) };
    belongs
}

#[cfg(any(windows, test))]
fn path_is_under_install_root(exe_path: &Path, install_root: &Path) -> bool {
    fn normalized(path: &Path) -> Option<Vec<String>> {
        let mut value = path.to_string_lossy().replace('\\', "/");
        if cfg!(windows) {
            value = value.to_lowercase();
        }
        let (is_unc, rest) = if let Some(tail) = value
            .strip_prefix("//?/unc/")
            .or_else(|| value.strip_prefix("//?/UNC/"))
        {
            (true, tail)
        } else if let Some(tail) = value.strip_prefix("//?/") {
            (false, tail)
        } else if let Some(tail) = value.strip_prefix("//") {
            (true, tail)
        } else {
            (false, value.as_str())
        };
        let mut parts: Vec<String> = rest
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_string)
            .collect();
        if parts.is_empty() || parts.iter().any(|part| part == "..") {
            return None;
        }
        if is_unc {
            parts.insert(0, "//unc".to_string());
        }
        Some(parts)
    }
    let (Some(exe), Some(root)) = (normalized(exe_path), normalized(install_root)) else {
        return false;
    };
    exe.len() > root.len() && exe.starts_with(&root)
}

#[cfg(windows)]
fn request_process_window_close(pid: u32) {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };
    /// # Safety
    /// Invoked synchronously by `EnumWindows` with a top-level `HWND` and an integer `pid` packed
    /// in `data` (never dereferenced as a pointer).
    unsafe extern "system" fn callback(window: HWND, data: LPARAM) -> BOOL {
        let mut owner = 0u32;
        // SAFETY: `window` was supplied by `EnumWindows` and `&mut owner` is a valid writable `u32`.
        unsafe { GetWindowThreadProcessId(window, &mut owner) };
        if owner == data as u32 {
            // SAFETY: `PostMessageW` posts an asynchronous integer-only `WM_CLOSE` message and
            // safely ignores windows that closed concurrently.
            unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
        }
        1
    }
    if pid != 0 {
        // SAFETY: `callback` is an `extern "system"` `WNDENUMPROC` that interprets `pid as isize`
        // purely as a `u32` value.
        unsafe { EnumWindows(Some(callback), pid as isize) };
    }
}

#[cfg(any(windows, test))]
fn should_terminate_auxiliary_process(
    exe_path: &Path,
    install_root: &Path,
    pid: u32,
    current_pid: u32,
) -> bool {
    if pid == 0 || pid == current_pid || install_root.as_os_str().is_empty() {
        return false;
    }
    let filename = exe_path.to_string_lossy();
    let name = filename.rsplit(['/', '\\']).next().unwrap_or_default();
    (name.eq_ignore_ascii_case("hlsdownloaderpresenter.exe")
        || name.eq_ignore_ascii_case("hlsdownloadernativehost.exe"))
        && path_is_under_install_root(exe_path, install_root)
}

#[cfg(windows)]
fn terminate_auxiliary_processes(install_root: &Path) {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    // SAFETY: `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)` takes no pointers and returns
    // `INVALID_HANDLE_VALUE` on failure or an owned snapshot handle closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot == INVALID_HANDLE_VALUE {
        return;
    }
    // SAFETY: `PROCESSENTRY32W` is a plain C struct for which all-zero initialization is valid once
    // `dwSize` is set to `size_of::<PROCESSENTRY32W>()` before `Process32FirstW`.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snapshot` is a valid process snapshot handle and `entry.dwSize` matches its layout.
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
            // SAFETY: Opens a non-inheritable handle to `entry.th32ProcessID`, returning null if
            // the process already exited or access is denied.
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                    0,
                    entry.th32ProcessID,
                )
            };
            if !handle.is_null() {
                let mut image = vec![0u16; 32768];
                let mut length = image.len() as u32;
                // SAFETY: `handle` is non-null with `PROCESS_QUERY_LIMITED_INFORMATION`, and `image`
                // provides `length` (`32768`) writable `u16` elements.
                let got_image = unsafe {
                    QueryFullProcessImageNameW(handle, 0, image.as_mut_ptr(), &mut length)
                };
                if got_image != 0 {
                    let written = (length as usize).min(image.len());
                    let path = PathBuf::from(OsString::from_wide(&image[..written]));
                    if should_terminate_auxiliary_process(
                        &path,
                        install_root,
                        entry.th32ProcessID,
                        std::process::id(),
                    ) {
                        request_process_window_close(entry.th32ProcessID);
                        // SAFETY: `handle` was opened with `PROCESS_TERMINATE` and verified to
                        // belong to an auxiliary executable inside `install_root`.
                        unsafe {
                            TerminateProcess(handle, 0);
                        }
                    }
                }
                // SAFETY: `handle` is uniquely owned by this loop iteration and closed once here.
                unsafe {
                    CloseHandle(handle);
                }
            }
        }
        // SAFETY: `snapshot` remains valid and `entry.dwSize` is preserved across iterations.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: `snapshot` is uniquely owned by this function and closed once after enumeration.
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
    if value.is_empty() || value.chars().all(|ch| ch == '.') {
        "unknown".into()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_product_contract_matches_official_wix() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/build/build-native-v7-installer.ps1");
        if !script.is_file() {
            return;
        }
        let script = std::fs::read_to_string(script).unwrap();
        let wix = script
            .lines()
            .find(|line| line.contains("<Product "))
            .unwrap();
        assert!(wix.contains(&format!("Name=\"{EXPECTED_PRODUCT_NAME}\"")));
        assert!(wix.to_ascii_uppercase().contains(&format!(
            "UPGRADECODE=\"{}\"",
            normalize_guid(EXPECTED_UPGRADE_CODE)
        )));
        assert!(script.contains("InstallScope=\"perUser\""));
    }

    #[test]
    fn installer_rejects_legacy_and_unrelated_product_names() {
        for name in ["HLSDownloader", "Unrelated Downloader"] {
            let identity = InstallerIdentity {
                product_name: name.into(),
                product_version: "10.0.0".into(),
                upgrade_code: EXPECTED_UPGRADE_CODE.into(),
                per_user: true,
            };
            assert!(validate_installer_identity_fields(&identity, "10.0.0")
                .unwrap_err()
                .contains("产品名"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn installer_command_uses_system_directory_and_preserves_unicode_arguments() {
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
        let mut directory = vec![0u16; 32768];
        let length = unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) };
        assert!(length > 0 && (length as usize) < directory.len());
        let expected =
            PathBuf::from(OsString::from_wide(&directory[..length as usize])).join("msiexec.exe");
        let args = msiexec_args(
            Path::new(r"C:\测试 更新\setup.msi"),
            Path::new(r"C:\测试 更新\install.log"),
        );
        let command = msiexec_command(&args).unwrap();
        assert_eq!(command.get_program(), expected.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            args.iter().map(OsString::as_os_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn verifier_rejection_is_recorded_before_msi_execution() {
        for error in ["rejected signer", "missing primary thumbprint"] {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/audit-tests")
                .join(format!(
                    "verifier-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            let result = root.join("last-install-result.json");
            let args = [
                OsString::from("--msi"),
                root.join("unverified.msi").into_os_string(),
                "--version".into(),
                "10.0.0".into(),
                "--core-pid".into(),
                "0".into(),
                "--workbench-pid".into(),
                "0".into(),
                "--install-root".into(),
                root.clone().into_os_string(),
                "--workbench".into(),
                root.join("absent.exe").into_os_string(),
                "--log".into(),
                root.join("install.log").into_os_string(),
                "--result".into(),
                result.clone().into_os_string(),
            ];
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("unverified.msi"), b"unsigned fixture").unwrap();
            #[cfg(windows)]
            if error == "rejected signer" {
                for failure in [
                    run_update_helper(args.clone()),
                    run_update_helper_with_verifier(args.clone(), |_| Ok(())),
                ] {
                    assert!(failure.unwrap_err().contains("Authenticode"));
                }
            }
            let failure = run_update_helper_with_verifier(args, |path| {
                assert_eq!(path, root.join("unverified.msi"));
                Err(error.into())
            })
            .unwrap_err();
            assert_eq!(failure, error);
            let recorded: InstallResult =
                serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
            assert_eq!(recorded.status, "failed");
            assert_eq!(recorded.exit_code, -1);
            assert_eq!(recorded.message, error);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn verifier_rejection_waits_for_running_product_shutdown_before_restarting() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/audit-tests")
            .join(format!(
                "verifier-shutdown-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let result = root.join("last-install-result.json");
        let args = HelperArgs {
            msi: root.join("unverified.msi"),
            version: "10.0.0".into(),
            core_pid: 4242,
            workbench_pid: 4343,
            install_root: root.clone(),
            workbench: root.join("absent.exe"),
            log: root.join("install.log"),
            result: result.clone(),
        };
        let steps = std::cell::RefCell::new(Vec::new());
        let error = run_update_helper_with_hooks(
            &args,
            |_| Err("rejected signer".into()),
            |_, _, _: &()| panic!("identity check must not run after signer rejection"),
            |_| {
                assert!(
                    result.is_file(),
                    "failure result must be persisted before waiting for shutdown"
                );
                steps.borrow_mut().push("wait_shutdown");
                Ok(())
            },
            |_, _| panic!("msiexec must not run after signer rejection"),
            |_| steps.borrow_mut().push("restart"),
        )
        .unwrap_err();
        assert_eq!(error, "rejected signer");
        assert_eq!(*steps.borrow(), vec!["wait_shutdown", "restart"]);

        let mut shutdown_calls = 0usize;
        let mut restarted = false;
        let timeout = run_update_helper_with_hooks(
            &args,
            |_| Ok(()),
            |_, _, _| {
                Ok(InstallerIdentity {
                    product_name: EXPECTED_PRODUCT_NAME.into(),
                    product_version: "10.0.0".into(),
                    upgrade_code: EXPECTED_UPGRADE_CODE.into(),
                    per_user: true,
                })
            },
            |_| {
                shutdown_calls += 1;
                Err("shutdown timeout".into())
            },
            |_, _| panic!("msiexec must not run after shutdown failure"),
            |_| restarted = true,
        )
        .unwrap_err();
        assert_eq!(timeout, "shutdown timeout");
        assert_eq!(shutdown_calls, 1);
        assert!(restarted);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auxiliary_process_selection_is_confined_to_install_components() {
        let root = Path::new(r"C:\Apps\HLS");
        for path in [
            r"C:\Apps\HLS\HLSDownloaderPresenter.exe",
            r"C:\Apps\HLS\bin\HLSDownloaderNativeHost.exe",
            r"\\?\C:\Apps\HLS\HLSDownloaderPresenter.exe",
        ] {
            assert!(
                should_terminate_auxiliary_process(Path::new(path), root, 12, 99),
                "{path}"
            );
        }
        for path in [
            r"C:\Other\HLSDownloaderPresenter.exe",
            r"C:\Apps\HLS2\HLSDownloaderPresenter.exe",
            r"C:\Apps\HLS\other.exe",
            r"C:\Apps\HLS\..\Other\HLSDownloaderPresenter.exe",
        ] {
            assert!(
                !should_terminate_auxiliary_process(Path::new(path), root, 12, 99),
                "{path}"
            );
        }
        let exe = Path::new(r"C:\Apps\HLS\HLSDownloaderPresenter.exe");
        assert!(!should_terminate_auxiliary_process(
            exe,
            Path::new(""),
            12,
            99
        ));
        assert!(!should_terminate_auxiliary_process(exe, root, 99, 99));
        assert!(!should_terminate_auxiliary_process(exe, root, 0, 99));
        #[cfg(windows)]
        {
            assert!(should_terminate_auxiliary_process(
                Path::new(r"c:/apps/hls/HLSdownloaderPRESENTER.exe"),
                root,
                12,
                99
            ));
            assert!(should_terminate_auxiliary_process(
                Path::new(r"\\?\UNC\server\share\HLS\HLSDownloaderPresenter.exe"),
                Path::new(r"\\server\share\HLS"),
                12,
                99
            ));
            assert!(should_terminate_auxiliary_process(
                Path::new(r"\\?\unc\server\share\HLS\HLSDownloaderPresenter.exe"),
                Path::new(r"\\server\share\HLS"),
                12,
                99
            ));
            assert!(!should_terminate_auxiliary_process(
                Path::new(r"server\share\HLS\HLSDownloaderPresenter.exe"),
                Path::new(r"\\server\share\HLS"),
                12,
                99
            ));
        }
    }
    #[test]
    fn cover_install_uses_passive_msi_without_forcing_a_reboot() {
        let msi = Path::new(r"C:\测试 更新\HLSDownloader-7.1.0-Windows-x64.msi");
        let log = Path::new(r"C:\测试 更新\HLSDownloader-7.1.0-Windows-x64.install.log");
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
    #[ignore = "invokes Windows Installer on the local machine"]
    fn current_user_installer_returns_the_real_exit_code() {
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
        // 未安装的产品只返回 1605，不会改动现有产品；同时覆盖参数和进程等待。
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
        let error = verify_installer_authenticode(&path, None).unwrap_err();
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

    #[cfg(windows)]
    #[test]
    fn windows_signer_extraction_rejects_unsigned_payload() {
        let path = std::env::temp_dir().join(format!(
            "hls-updater-signer-unsigned-{}-{}.msi",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"unsigned signer fixture").unwrap();
        let package = lock_update_package(&path).unwrap();
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        let error = verify_installer_authenticode(&path, Some(&package)).unwrap_err();
        drop(package);
        assert!(error.contains("Authenticode"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn helper_gate_reads_the_msi_argument_without_changing_argument_order() {
        let args = [
            "--version",
            "7.0.2",
            "--msi",
            r"C:\Temp\HLSDownloader.msi",
            "--core-pid",
            "0",
            "--workbench-pid",
            "0",
            "--install-root",
            "app",
            "--workbench",
            "app.exe",
            "--log",
            "install.log",
            "--result",
            "result.json",
        ];
        let parsed = parse_helper_args(args.into_iter().map(OsString::from)).unwrap();
        assert_eq!(parsed.msi, Path::new(r"C:\Temp\HLSDownloader.msi"));
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

    #[test]
    fn release_signer_requires_a_valid_primary_and_reviewed_rollovers() {
        let primary = "0123456789ABCDEF0123456789ABCDEF01234567";
        let other = "123456789ABCDEF0123456789ABCDEF012345678";
        use super::signer_policy::{authorize_release_signer, SIGNER_TRUST_JSON};
        let policy = SIGNER_TRUST_JSON;
        assert!(authorize_release_signer(primary, Some(primary), policy).is_ok());
        assert!(authorize_release_signer(primary, None, policy).is_err());
        let mut policy: serde_json::Value = serde_json::from_str(policy).unwrap();
        policy["rollover_signers"] = serde_json::json!([]);
        assert!(authorize_release_signer(other, Some(primary), &policy.to_string()).is_err());
        policy["rollover_signers"] = serde_json::json!([{"thumbprint":other, "reason":"rotation"}]);
        assert!(authorize_release_signer(other, Some(primary), &policy.to_string()).is_ok());
        policy["rollover_signers"][0]["reason"] = serde_json::json!("");
        assert!(authorize_release_signer(primary, Some(primary), &policy.to_string()).is_err());
    }

    #[test]
    fn per_machine_and_ambiguous_dual_context_msi_are_rejected() {
        assert!(msi_properties_are_per_user(None, None));
        assert!(msi_properties_are_per_user(Some("2"), Some("1")));
        assert!(!msi_properties_are_per_user(Some("2"), None));
        assert!(!msi_properties_are_per_user(Some("1"), Some("1")));
    }

    #[test]
    fn concurrent_consumers_acknowledge_a_result_once() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/audit-tests")
            .join(format!("result-consumers-{}", std::process::id()));
        let path = root.join("result.json");
        let result = InstallResult {
            version: "1".into(),
            status: "success".into(),
            exit_code: 0,
            message: String::new(),
            install_log: String::new(),
        };
        write_install_result(&path, &result).unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let consume = || {
                barrier.wait();
                take_install_result_at(&path).unwrap()
            };
            let other = scope.spawn(consume);
            let results = [consume(), other.join().unwrap()];
            assert_eq!(
                results.into_iter().flatten().collect::<Vec<_>>(),
                vec![result]
            );
        });
        std::fs::remove_dir_all(root).unwrap();
    }
}
