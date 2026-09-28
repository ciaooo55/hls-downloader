//! Mark of the Web for published public HTTP(S) downloads.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

fn http_parts(value: &str) -> Option<(&str, &str, &str)> {
    let raw = value.trim();
    let (scheme, rest) = raw.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let boundary = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some((scheme, &rest[..boundary], &rest[boundary..]))
}

fn authority_host(authority: &str) -> String {
    let authority = authority.rsplit('@').next().unwrap_or("").trim();
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map(|(host, _)| host).unwrap_or(rest)
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    host.trim().trim_matches('.').to_ascii_lowercase()
}

pub fn is_public_download_url(value: &str) -> bool {
    let Some((_, authority, _)) = http_parts(value) else {
        return false;
    };
    let host = authority_host(authority);
    if host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
    {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(address) => ip_is_public(address),
        Err(_) => true,
    }
}

/// IPv6 里藏着 IPv4 的两种写法都要先归一化，否则"换写法"就能绕过本地地址检查：
/// `::ffff:127.0.0.1`（IPv4-mapped）和 `::127.0.0.1`（IPv4-compatible）在 socket
/// 层就是那两个 IPv4 地址。playback.rs 的 peer_allowed 对 mapped 同样处理。
fn ip_is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => ipv4_is_public(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return ipv4_is_public(mapped);
            }
            let segments = v6.segments();
            if segments[..6].iter().all(|segment| *segment == 0)
                && (segments[6] != 0 || segments[7] != 0)
            {
                let v4 = Ipv4Addr::from(((segments[6] as u32) << 16) | segments[7] as u32);
                return ipv4_is_public(v4);
            }
            !(v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local())
        }
    }
}

fn ipv4_is_public(v4: Ipv4Addr) -> bool {
    let oct = v4.octets();
    !(v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_multicast()
        || v4.is_unspecified()
        || (oct[0] == 100 && (oct[1] & 0b1100_0000) == 64)
        || oct[0] == 0)
}

pub fn redact_url(value: &str) -> String {
    let Some((scheme, authority, suffix)) = http_parts(value) else {
        return String::new();
    };
    let authority = authority.rsplit('@').next().unwrap_or("").trim();
    if authority.is_empty() {
        return String::new();
    }
    let path = if suffix.starts_with('/') {
        suffix.split(['?', '#']).next().unwrap_or("")
    } else {
        ""
    };
    if path.is_empty() {
        format!("{scheme}://{authority}/")
    } else {
        format!("{scheme}://{authority}{path}")
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
    fn public_http_is_marked_and_local_addresses_are_not() {
        assert!(is_public_download_url("https://cdn.example.test/a.bin"));
        assert!(!is_public_download_url("http://127.0.0.1/a.bin"));
        assert!(!is_public_download_url("http://192.168.1.8/a.bin"));
        assert!(!is_public_download_url("http://[::1]/a.bin"));
        assert!(!is_public_download_url("http://[fe80::1]/a.bin"));
        assert!(!is_public_download_url("http://[fd00::1]/a.bin"));
        assert!(!is_public_download_url("ftp://files.example.test/a.bin"));
    }

    /// 本地地址的 IPv6 换写法不能因为"没被 IPv4 规则认出来"就判成公网。
    /// 以前 `::ffff:127.0.0.1` / `::127.0.0.1` 直接走 IPv6 分支，被判成公网，
    /// MOTW 标记判断就和 playback/投屏侧的本地地址判定对不上了。
    #[test]
    fn ipv6_spellings_of_ipv4_are_judged_as_that_ipv4() {
        // IPv4-mapped（::ffff:0:0/96）。
        assert!(!is_public_download_url("http://[::ffff:127.0.0.1]/a.bin"));
        assert!(!is_public_download_url("http://[::ffff:192.168.1.5]/a.bin"));
        assert!(!is_public_download_url("http://[::ffff:0.0.0.1]/a.bin"));
        assert!(!is_public_download_url("http://[::ffff:100.64.1.1]/a.bin"));
        assert!(is_public_download_url("http://[::ffff:8.8.8.8]/a.bin"));
        // IPv4-compatible（::a.b.c.d）。
        assert!(!is_public_download_url("http://[::127.0.0.1]/a.bin"));
        assert!(!is_public_download_url("http://[::192.168.1.5]/a.bin"));
        // 未指定地址与真正的公网 IPv6 维持原判。
        assert!(!is_public_download_url("http://[::]/a.bin"));
        assert!(is_public_download_url(
            "http://[2606:4700:4700::1111]/a.bin"
        ));
    }

    #[test]
    fn zone_identifier_never_keeps_credentials_query_or_fragment() {
        let text = zone_identifier_text("https://user:pass@cdn.example.test/path?token=1#part");
        assert!(text.contains("ZoneId=3"));
        assert!(text.contains("HostUrl=https://cdn.example.test/path"));
        assert!(!text.contains("user:pass"));
        assert!(!text.contains("token=1"));
        assert!(!text.contains("#part"));

        let root_query = zone_identifier_text("https://cdn.example.test?token=root-secret");
        assert!(root_query.contains("HostUrl=https://cdn.example.test/"));
        assert!(!root_query.contains("root-secret"));

        let trailing_slash = zone_identifier_text("https://cdn.example.test/path/?token=1");
        assert!(trailing_slash.contains("HostUrl=https://cdn.example.test/path/"));
    }
}
