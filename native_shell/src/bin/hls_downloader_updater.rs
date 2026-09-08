#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use serde::Deserialize;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const SIGNER_TRUST_JSON: &str =
    include_str!("../../../artifacts/v7-productization/update-signer-trust.json");
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

fn parse_signer_trust(json: &str) -> Result<Vec<String>, String> {
    let contract: SignerTrustContract =
        serde_json::from_str(json).map_err(|error| format!("解析更新签名信任配置失败: {error}"))?;
    if contract.schema != 1 {
        return Err(format!("不支持的更新签名信任 schema: {}", contract.schema));
    }
    if contract.identity != EXPECTED_TRUST_IDENTITY {
        return Err(format!("不支持的更新签名身份类型: {}", contract.identity));
    }
    if contract.policy != EXPECTED_TRUST_POLICY {
        return Err(format!("不支持的更新签名信任策略: {}", contract.policy));
    }

    let mut unique = HashSet::new();
    let mut signers = Vec::with_capacity(contract.rollover_signers.len());
    for signer in contract.rollover_signers {
        if signer.reason.trim().is_empty() {
            return Err("更新签名 rollover 条目缺少审核原因".into());
        }
        let thumbprint = normalize_thumbprint(&signer.thumbprint)?;
        if !unique.insert(thumbprint.clone()) {
            return Err(format!("更新签名 rollover 指纹重复: {thumbprint}"));
        }
        signers.push(thumbprint);
    }
    Ok(signers)
}

fn authorize_signer_thumbprint(
    candidate: &str,
    primary: Option<&str>,
    trust_json: &str,
) -> Result<(), String> {
    let candidate = normalize_thumbprint(candidate)?;
    let primary = primary
        .ok_or_else(|| "正式发布构建缺少嵌入的 HLS_V7_SIGN_CERT_THUMBPRINT".to_string())
        .and_then(normalize_thumbprint)?;
    if candidate == primary {
        return Ok(());
    }
    let rollover = parse_signer_trust(trust_json)?;
    if rollover.iter().any(|allowed| allowed == &candidate) {
        return Ok(());
    }
    Err(format!(
        "升级安装包签名者不是受信任的 HLS Downloader 发布证书: {candidate}"
    ))
}

fn embedded_primary_signer() -> Option<&'static str> {
    option_env!("HLS_V7_SIGN_CERT_THUMBPRINT").filter(|value| !value.trim().is_empty())
}

fn find_msi_argument(args: &[OsString]) -> Result<PathBuf, String> {
    let mut index = 0usize;
    while index < args.len() {
        if args[index].to_string_lossy() == "--msi" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| "更新助手参数 --msi 缺少值".to_string())?;
            return Ok(PathBuf::from(value));
        }
        index += 2;
    }
    Err("缺少更新助手参数 --msi".into())
}

fn verify_update_signer(args: &[OsString]) -> Result<(), String> {
    let msi = find_msi_argument(args)?;
    #[cfg(windows)]
    {
        let signer = verified_leaf_signer_thumbprint(&msi)?;
        authorize_signer_thumbprint(&signer, embedded_primary_signer(), SIGNER_TRUST_JSON)
    }
    #[cfg(not(windows))]
    {
        let _ = msi;
        Err("更新签名证书校验只适用于 Windows".into())
    }
}

#[cfg(windows)]
fn verified_leaf_signer_thumbprint(path: &Path) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Cryptography::{
        CertGetCertificateContextProperty, CERT_SHA1_HASH_PROP_ID,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain,
        WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2,
        WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL,
        WTD_CHOICE_FILE, WTD_DISABLE_MD2_MD4, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    if !path.is_file() {
        return Err("升级安装包不存在，无法验证签名者".into());
    }
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
    if status != 0 {
        trust.dwStateAction = WTD_STATEACTION_CLOSE;
        unsafe {
            WinVerifyTrust(
                std::ptr::null_mut(),
                &mut action,
                &mut trust as *mut _ as *mut core::ffi::c_void,
            );
        }
        return Err(format!(
            "升级安装包 Authenticode 信任校验失败（WinVerifyTrust 0x{:08X}）",
            status as u32
        ));
    }

    let result = (|| {
        let provider = unsafe { WTHelperProvDataFromStateData(trust.hWVTStateData) };
        if provider.is_null() {
            return Err("无法读取已验证的 Authenticode provider 状态".into());
        }
        let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
        if signer.is_null() {
            return Err("无法读取已验证的 Authenticode 主签名者".into());
        }
        let cert = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
        if cert.is_null() {
            return Err("无法读取已验证的 Authenticode leaf 证书".into());
        }
        let cert_context = unsafe { (*cert).pCert };
        if cert_context.is_null() {
            return Err("已验证的 Authenticode leaf 证书上下文为空".into());
        }
        let mut hash = [0u8; 20];
        let mut hash_len = hash.len() as u32;
        let ok = unsafe {
            CertGetCertificateContextProperty(
                cert_context,
                CERT_SHA1_HASH_PROP_ID,
                hash.as_mut_ptr() as *mut core::ffi::c_void,
                &mut hash_len,
            )
        };
        if ok == 0 || hash_len as usize != hash.len() {
            return Err("无法读取 Authenticode leaf 证书 SHA-1 指纹".into());
        }
        Ok(hash
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>())
    })();

    trust.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            &mut trust as *mut _ as *mut core::ffi::c_void,
        );
    }
    result
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if let Err(error) = verify_update_signer(&args) {
        eprintln!("update helper signer verification failed: {error}");
        return ExitCode::from(1);
    }
    match hls_native_shell::run_update_helper(args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("update helper failed: {error}");
            ExitCode::from(1)
        }
    }
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
        assert!(authorize_signer_thumbprint(PRIMARY, Some(PRIMARY), SIGNER_TRUST_JSON).is_ok());
        assert!(authorize_signer_thumbprint(ROLLOVER, Some(PRIMARY), &rollover_json()).is_ok());
        let error =
            authorize_signer_thumbprint(OTHER, Some(PRIMARY), &rollover_json()).unwrap_err();
        assert!(error.contains("不是受信任"));
    }

    #[test]
    fn signer_trust_requires_a_formal_primary_identity() {
        let error = authorize_signer_thumbprint(PRIMARY, None, SIGNER_TRUST_JSON).unwrap_err();
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
    fn helper_gate_reads_the_msi_argument_without_changing_argument_order() {
        let args = vec![
            OsString::from("--version"),
            OsString::from("7.0.2"),
            OsString::from("--msi"),
            OsString::from(r"C:\Temp\HLSDownloader.msi"),
        ];
        assert_eq!(
            find_msi_argument(&args).unwrap(),
            PathBuf::from(r"C:\Temp\HLSDownloader.msi")
        );
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
        let error = verified_leaf_signer_thumbprint(&path).unwrap_err();
        assert!(error.contains("Authenticode"));
        std::fs::remove_file(path).unwrap();
    }
}
