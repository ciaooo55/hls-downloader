//! Mark of the Web for published public HTTP(S) downloads.

use std::net::IpAddr;
use std::path::Path;

pub fn is_public_download_url(value: &str) -> bool {
    let raw = value.trim();
    let Some((scheme, rest)) = raw.split_once("://") else {
        return false;
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return false;
    }
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local")
    {
        return false;
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        return match address {
            IpAddr::V4(v4) => {
                let oct = v4.octets();
                !(v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_multicast()
                    || v4.is_unspecified()
                    || (oct[0] == 100 && (oct[1] & 0b1100_0000) == 64)
                    || oct[0] == 0)
            }
            IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_multicast() || v6.is_unspecified()),
        };
    }
    true
}

pub fn redact_url(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return String::new();
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.split('@').next_back().unwrap_or(authority);
    if path.is_empty() {
        format!("{scheme}://{host}/")
    } else {
        format!("{scheme}://{host}/{}", path.split('?').next().unwrap_or(path))
    }
}

pub fn zone_identifier_text(source_url: &str) -> String {
    let mut lines = vec!["[ZoneTransfer]".into(), "ZoneId=3".into()];
    let host = redact_url(source_url);
    if !host.is_empty() {
        lines.push(format!("HostUrl={host}"));
    }
    lines.join("\r\n") + "\r\n"
}

pub fn mark_downloaded_file(path: &Path, source_url: &str) {
    if !is_public_download_url(source_url) || !path.is_file() {
        return;
    }
    #[cfg(windows)]
    {
        let ads = format!("{}:Zone.Identifier", path.display());
        let _ = std::fs::write(ads, zone_identifier_text(source_url));
    }
    #[cfg(not(windows))]
    {
        let _ = (path, source_url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_http_is_marked_and_loopback_is_not() {
        assert!(is_public_download_url("https://cdn.example.test/a.bin"));
        assert!(!is_public_download_url("http://127.0.0.1/a.bin"));
        assert!(!is_public_download_url("http://192.168.1.8/a.bin"));
        assert!(!is_public_download_url("ftp://files.example.test/a.bin"));
        let text = zone_identifier_text("https://user:pass@cdn.example.test/path?token=1");
        assert!(text.contains("ZoneId=3"));
        assert!(text.contains("HostUrl=https://cdn.example.test/path"));
        assert!(!text.contains("user:pass"));
        assert!(!text.contains("token=1"));
    }
}
