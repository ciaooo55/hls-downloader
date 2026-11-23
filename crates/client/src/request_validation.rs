//! Shared input checks; no network or Core state.
use std::fs;
use std::path::{Path, PathBuf};

/// Whether the file name has a reserved Windows device name before its extension.
pub fn reserved_dos_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim();
    let stem = stem.to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) || ["COM", "LPT"].into_iter().any(|prefix| {
        matches!(
            stem.strip_prefix(prefix),
            Some("0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        )
    })
}

pub fn validate_local_source_path(
    url: &str,
    source_path: &str,
    expected_size: Option<u64>,
) -> Result<PathBuf, String> {
    let lower_url = url.to_ascii_lowercase();
    if !(lower_url.starts_with("blob:") || lower_url.starts_with("data:"))
        || url.is_empty()
        || url.chars().any(char::is_control)
    {
        return Err("本地源只允许对应 blob: 或 data: 资源".into());
    }
    let path = Path::new(source_path.trim());
    if !path.is_absolute() || source_path.chars().any(char::is_control) {
        return Err("本地源路径必须是有效的绝对路径".into());
    }
    let canonical =
        fs::canonicalize(path).map_err(|error| format!("无法读取本地源文件: {error}"))?;
    let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("本地源必须是常规文件".into());
    }
    let expected = expected_size.ok_or_else(|| "本地源缺少声明的文件大小".to_string())?;
    if metadata.len() != expected {
        return Err(format!(
            "本地源大小不匹配: 声明 {expected} 字节，实际 {} 字节",
            metadata.len()
        ));
    }
    Ok(canonical)
}

pub fn sanitize_http_method(raw: &str) -> String {
    let method = raw.trim();
    if method.eq_ignore_ascii_case("POST") {
        "POST".into()
    } else if method.eq_ignore_ascii_case("HEAD") {
        "HEAD".into()
    } else {
        "GET".into()
    }
}

pub fn http_fetch_url_allowed(url: &str) -> bool {
    if url.chars().any(char::is_control) {
        return false;
    }
    let lower = url
        .trim()
        .trim_start_matches('\u{feff}')
        .to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

pub fn remote_resource_url_allowed(url: &str) -> bool {
    if url.chars().any(char::is_control) {
        return false;
    }
    let lower = url
        .trim()
        .trim_start_matches('\u{feff}')
        .to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("sftp://")
        || lower.starts_with("magnet:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_validation_rejects_controls_before_whitespace_normalization() {
        for value in [
            "https://cdn.test/a\r\n",
            "\nftp://server/file",
            "\tmagnet:?xt=x",
        ] {
            assert!(!http_fetch_url_allowed(value));
            assert!(!remote_resource_url_allowed(value));
        }
        assert!(http_fetch_url_allowed(" https://cdn.test/a "));
    }
}
