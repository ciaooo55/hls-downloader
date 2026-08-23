//! GitHub release check. Download starts only after the user confirms.

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
    let (installer_url, installer_name) = pick_installer_asset(&value);
    let info = UpdateInfo {
        newer: is_newer_version(&latest, current),
        current: current.trim().trim_start_matches('v').to_string(),
        latest,
        html_url,
        notes,
        installer_url,
        installer_name,
    };
    remember_update(info.clone());
    Ok(info)
}

pub fn pick_installer_asset(release: &serde_json::Value) -> (String, String) {
    let Some(assets) = release.get("assets").and_then(|item| item.as_array()) else {
        return (String::new(), String::new());
    };
    let mut ranked: Vec<(u8, String, String)> = Vec::new();
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
        if url.is_empty() || name.is_empty() || !installer_url_allowed(&url) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let rank = if lower.contains("setup") && lower.ends_with(".exe") {
            0
        } else if lower.contains("hlsdownloader") && lower.ends_with(".exe") {
            1
        } else if lower.ends_with(".msi") {
            2
        } else if lower.ends_with(".exe") {
            3
        } else {
            continue;
        };
        ranked.push((rank, url, name));
    }
    ranked.sort_by_key(|(rank, _, _)| *rank);
    ranked
        .into_iter()
        .next()
        .map(|(_, url, name)| (url, name))
        .unwrap_or_default()
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
    let mut headers = std::collections::HashMap::new();
    headers.insert("User-Agent".into(), "hls-downloader-v7".into());
    headers.insert("Accept".into(), "application/octet-stream".into());
    let (status, body) = crate::http_engine::fetch_bytes(&info.installer_url, &headers, "")
        .map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Err(format!("下载安装包失败 HTTP {status}"));
    }
    let name = sanitize_installer_name(&info.installer_name, &info.latest);
    if std::path::Path::new(&name).components().count() != 1 {
        return Err("安装包文件名无效".into());
    }
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, body).map_err(|error| error.to_string())?;
    Ok(path)
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
        "HLSDownloader-{}.exe",
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
    if lower.ends_with(".exe") || lower.ends_with(".msi") {
        cleaned
    } else if cleaned.is_empty() {
        fallback
    } else {
        format!("{cleaned}.exe")
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
        let json = r#"{"tag_name":"v6.1.0","html_url":"https://github.com/ciaooo55/hls-downloader/releases/tag/v6.1.0","body":"fixes","assets":[{"name":"notes.txt","browser_download_url":"https://example/notes.txt"},{"name":"HLSDownloader-v6.1.0-Setup.exe","browser_download_url":"https://github.com/ciaooo55/hls-downloader/releases/download/v6.1.0/Setup.exe"}]}"#;
        let info = parse_github_release(json, "6.0.0-dev").unwrap();
        assert_eq!(info.latest, "6.1.0");
        assert!(info.newer);
        assert!(info.installer_name.contains("Setup"));
        assert_eq!(
            info.installer_url,
            "https://github.com/ciaooo55/hls-downloader/releases/download/v6.1.0/Setup.exe"
        );
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
            sanitize_installer_name(r"..\..\Startup\evil.exe", "6.1.0"),
            "evil.exe"
        );
        assert_eq!(
            sanitize_installer_name(r"C:\Windows\notepad.exe", "6.1.0"),
            "notepad.exe"
        );
        assert_eq!(
            std::path::Path::new(&sanitize_installer_name(r"..\..\Startup\evil.exe", "6.1.0"))
                .components()
                .count(),
            1
        );
    }
}
