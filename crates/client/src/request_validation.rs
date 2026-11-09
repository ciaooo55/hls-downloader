//! Shared input checks; no network or Core state.
use std::fs;
use std::path::{Path, PathBuf};

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
    let url = url.trim().trim_start_matches('\u{feff}');
    if url.is_empty() || url.chars().any(|ch| ch.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

pub fn remote_resource_url_allowed(url: &str) -> bool {
    let url = url.trim().trim_start_matches('\u{feff}');
    if url.is_empty() || url.chars().any(|ch| ch.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("sftp://")
        || lower.starts_with("magnet:")
}
