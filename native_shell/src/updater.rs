//! GitHub release check. Download starts only after the user confirms.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
    let (status, body) =
        crate::http_engine::fetch_bytes_limited(LATEST_API, &headers, "", 1024 * 1024)
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
        curl_body_limit: None,
        range_stop: None,
        origin_boundary: String::new(),
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

pub use hls_client::update_install::{
    launch_update_helper, take_install_result, verify_installer_identity,
};
use hls_client::update_install::{sanitize_version, update_root};

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
        assert!(!is_newer_version(CURRENT_VERSION, CURRENT_VERSION));
        assert!(!is_newer_version("7.0.3-r2", "7.0.3-r3"));
        assert!(is_newer_version("7.0.3-r4", "7.0.3-r3"));
        assert!(is_newer_version("7.0.4", "7.0.3-r4"));
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
    fn compiled_release_is_current_and_does_not_offer_its_own_installer() {
        let version = env!("CARGO_PKG_VERSION");
        let payload = serde_json::json!({
            "tag_name": format!("v{version}"),
            "assets": [],
        });
        let info = parse_github_release(&payload.to_string(), CURRENT_VERSION).unwrap();
        assert_eq!(info.current, version);
        assert!(
            !info.newer,
            "the installed release must not offer itself as an upgrade"
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
