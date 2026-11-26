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

fn is_bilibili_or_ws_signed_context(host: &str, path: &str, query: &str) -> bool {
    let host_bare = host.trim().trim_matches('.').to_ascii_lowercase();
    let is_bili_host = ["bilivideo.com", "bilivideo.cn", "hdslb.com", "bilibili.com"]
        .into_iter()
        .any(|domain| {
            host_bare == domain
                || host_bare
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
        || host_bare.contains("upos-");
    if is_bili_host {
        return true;
    }
    let path_lower = path.to_ascii_lowercase();
    if path_lower.contains("/upgcxcode/") || path_lower.contains("upos-") {
        return true;
    }
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .any(|part| {
            let key = part.split_once('=').map_or(part, |(k, _)| k).trim();
            key.eq_ignore_ascii_case("uparams") || key.eq_ignore_ascii_case("wssecret")
        })
}

fn is_ephemeral_query_param(
    key: &str,
    host: &str,
    path: &str,
    has_scoped_stream_sig: bool,
) -> bool {
    let lower = key.trim().to_ascii_lowercase();
    if lower.starts_with("x-amz-")
        || lower.starts_with("x-oss-")
        || lower.starts_with("x-cos-")
        || lower.starts_with("x-goog-")
        || lower.starts_with("q-sign-")
    {
        return true;
    }
    if matches!(
        lower.as_str(),
        "token"
            | "sig"
            | "signature"
            | "sign"
            | "upsig"
            | "expires"
            | "expire"
            | "deadline"
            | "e"
            | "ts"
            | "timestamp"
            | "t"
            | "hdnts"
            | "hdnea"
            | "wssecret"
            | "wstime"
            | "auth_key"
            | "policy"
            | "ossaccesskeyid"
            | "awsaccesskeyid"
            | "googleaccessid"
            | "security-token"
            | "key-pair-id"
            | "sv"
            | "se"
            | "sp"
            | "sr"
            | "st"
            | "spr"
            | "sdd"
            | "skt"
            | "ske"
            | "skoid"
            | "sktid"
            | "skv"
            | "jwt"
            | "exp"
            | "nonce"
            | "q-ak"
            | "q-key-time"
            | "q-signature"
            | "q-header-list"
            | "q-url-param-list"
    ) {
        return true;
    }
    if matches!(
        lower.as_str(),
        "key"
            | "trid"
            | "mid"
            | "gen"
            | "os"
            | "og"
            | "platform"
            | "orderid"
            | "logo"
            | "uparams"
            | "uipk"
            | "nbs"
            | "oi"
            | "mcdnid"
            | "buvid"
    ) {
        return has_scoped_stream_sig || is_bilibili_or_ws_signed_context(host, path, "");
    }
    false
}

/// Derives a stable canonical identity for an HTTP(S) resource URL so that
/// refreshing an expired presigned S3/OSS/COS/GCS/CDN/Bilibili URL produces
/// the exact same `resource_key` while keeping content-identifying parameters.
pub fn canonical_resource_key(url: &str) -> String {
    let trimmed = url.trim().trim_start_matches('\u{feff}').trim();
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let (scheme, default_port, rest) = if without_fragment
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
    {
        ("http", 80u16, &without_fragment[7..])
    } else if without_fragment
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
        ("https", 443u16, &without_fragment[8..])
    } else {
        return without_fragment.to_string();
    };

    let (before_query, query_opt) = match rest.split_once('?') {
        Some((before, query)) => (before, Some(query)),
        None => (rest, None),
    };
    let (authority, path) = match before_query.find('/') {
        Some(slash) => (&before_query[..slash], &before_query[slash..]),
        None => (before_query, "/"),
    };
    if authority.is_empty() {
        return without_fragment.to_string();
    }
    let host_port = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };
    let (host, port_str) = if host_port.starts_with('[') {
        let Some(close) = host_port.find(']') else {
            return without_fragment.to_string();
        };
        let host = &host_port[..=close];
        let after = &host_port[close + 1..];
        if after.is_empty() {
            (host, None)
        } else if let Some(port) = after.strip_prefix(':') {
            (host, Some(port))
        } else {
            return without_fragment.to_string();
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    if host.is_empty() {
        return without_fragment.to_string();
    }
    let non_default_port = match port_str {
        None | Some("") => None,
        Some(raw_port) => {
            let Ok(parsed) = raw_port.parse::<u16>() else {
                return without_fragment.to_string();
            };
            (parsed != default_port).then_some(parsed)
        }
    };

    let host_lower = host.to_ascii_lowercase();
    let mut out = String::with_capacity(without_fragment.len());
    out.push_str(scheme);
    out.push_str("://");
    out.push_str(&host_lower);
    if let Some(port) = non_default_port {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push_str(path);

    if let Some(query) = query_opt {
        let has_scoped_stream_sig = is_bilibili_or_ws_signed_context(&host_lower, path, query);
        let mut kept: Vec<(&str, Option<&str>)> = query
            .split('&')
            .filter(|part| !part.is_empty())
            .filter_map(|part| {
                let (k, v) = match part.split_once('=') {
                    Some((k, v)) => (k, Some(v)),
                    None => (part, None),
                };
                (!is_ephemeral_query_param(k, &host_lower, path, has_scoped_stream_sig))
                    .then_some((k, v))
            })
            .collect();
        if !kept.is_empty() {
            kept.sort_unstable();
            out.push('?');
            for (idx, (k, v)) in kept.into_iter().enumerate() {
                if idx > 0 {
                    out.push('&');
                }
                out.push_str(k);
                if let Some(v) = v {
                    out.push('=');
                    out.push_str(v);
                }
            }
        }
    }
    out
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

    #[test]
    fn canonical_resource_key_normalizes_presigned_s3_v4_and_v2_urls() {
        let first = "https://My-Bucket.S3.amazonaws.com:443/releases/v1/installer.exe?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIA111%2F20261125%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20261125T010000Z&X-Amz-Expires=900&X-Amz-SignedHeaders=host&X-Amz-Signature=1111aaaa#part1";
        let refreshed = "  HTTPS://my-bucket.s3.amazonaws.com/releases/v1/installer.exe?X-Amz-Signature=9999zzzz&X-Amz-SignedHeaders=host&X-Amz-Expires=3600&X-Amz-Date=20261125T050000Z&X-Amz-Credential=AKIA222%2F20261125%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Security-Token=temp-sts  ";
        assert_eq!(
            canonical_resource_key(first),
            "https://my-bucket.s3.amazonaws.com/releases/v1/installer.exe"
        );
        assert_eq!(
            canonical_resource_key(first),
            canonical_resource_key(refreshed)
        );

        let v2_first = "https://bucket.s3.amazonaws.com/archive.zip?AWSAccessKeyId=AKIAOLD&Expires=1700000000&Signature=sig1&version=2";
        let v2_refreshed = "https://bucket.s3.amazonaws.com/archive.zip?version=2&Signature=sig2&Expires=1800000000&AWSAccessKeyId=AKIANEW";
        assert_eq!(
            canonical_resource_key(v2_first),
            "https://bucket.s3.amazonaws.com/archive.zip?version=2"
        );
        assert_eq!(
            canonical_resource_key(v2_first),
            canonical_resource_key(v2_refreshed)
        );
    }

    #[test]
    fn canonical_resource_key_normalizes_aliyun_oss_and_azure_sas_urls() {
        let oss_v1 = "https://media-bucket.oss-cn-shanghai.aliyuncs.com/vod/clip.mp4?OSSAccessKeyId=LTAI1&Expires=1730000100&Signature=abc123&security-token=sts1&part=3";
        let oss_v4 = "https://MEDIA-BUCKET.oss-cn-shanghai.aliyuncs.com:443/vod/clip.mp4?part=3&x-oss-signature-version=OSS4-HMAC-SHA256&x-oss-credential=cred&x-oss-date=20261125T000000Z&x-oss-expires=3600&x-oss-signature=def456";
        assert_eq!(
            canonical_resource_key(oss_v1),
            "https://media-bucket.oss-cn-shanghai.aliyuncs.com/vod/clip.mp4?part=3"
        );
        assert_eq!(
            canonical_resource_key(oss_v1),
            canonical_resource_key(oss_v4)
        );

        let azure_sas = "https://acct.blob.core.windows.net/videos/demo.mp4?sv=2022-11-02&sr=b&sp=r&st=2026-11-25T00:00:00Z&se=2026-11-25T02:00:00Z&spr=https&sig=xyz&fid=99";
        assert_eq!(
            canonical_resource_key(azure_sas),
            "https://acct.blob.core.windows.net/videos/demo.mp4?fid=99"
        );
    }

    #[test]
    fn canonical_resource_key_normalizes_bilibili_m4s_signed_urls() {
        let bili_1 = "https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/12/34/567890-1-30080.m4s?e=ig8euxZM&uipk=5&nbs=1&deadline=1730000000&gen=playurlv2&os=cosbv&oi=11111&trid=aaaa1111&mid=12345&platform=pc&og=cos&upsig=deadbeef1111&uparams=e,uipk,nbs,deadline,gen,os,oi,trid,mid,platform,og&orderid=0,3&logo=80000000";
        let bili_2 = "https://UPOS-SZ-MIRRORCOS.bilivideo.com:443/upgcxcode/12/34/567890-1-30080.m4s?e=different&uipk=5&nbs=1&deadline=1730099999&gen=playurlv2&os=bcache&oi=22222&trid=bbbb2222&mid=99999&platform=html5&og=hw&upsig=cafebabe2222&uparams=e,deadline,trid&orderid=1,3&logo=00000000#frag";
        assert_eq!(
            canonical_resource_key(bili_1),
            "https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/12/34/567890-1-30080.m4s"
        );
        assert_eq!(
            canonical_resource_key(bili_1),
            canonical_resource_key(bili_2)
        );
    }

    #[test]
    fn canonical_resource_key_keeps_and_sorts_content_identifying_query_params() {
        let yt_1 = "https://rr3---sn-a5mekn6z.googlevideo.com/videoplayback?expire=1730000000&itag=137&mime=video%2Fmp4&id=o-ABC123&sig=111&token=aaa&t=10";
        let yt_2 = "https://rr3---sn-a5mekn6z.googlevideo.com/videoplayback?token=bbb&id=o-ABC123&mime=video%2Fmp4&itag=137&signature=222&expires=1739999999";
        assert_eq!(
            canonical_resource_key(yt_1),
            "https://rr3---sn-a5mekn6z.googlevideo.com/videoplayback?id=o-ABC123&itag=137&mime=video%2Fmp4"
        );
        assert_eq!(canonical_resource_key(yt_1), canonical_resource_key(yt_2));

        let query_id_1 = "http://Example.COM:8080/download?token=abc&qn=112&cid=888&bvid=BV1xx411c7mD&v=2&file=lesson.mp4#section";
        let query_id_2 = "HTTP://example.com:8080/download?file=lesson.mp4&v=2&bvid=BV1xx411c7mD&cid=888&qn=112&token=xyz&wsSecret=sec&wsTime=999";
        assert_eq!(
            canonical_resource_key(query_id_1),
            "http://example.com:8080/download?bvid=BV1xx411c7mD&cid=888&file=lesson.mp4&qn=112&v=2"
        );
        assert_eq!(
            canonical_resource_key(query_id_1),
            canonical_resource_key(query_id_2)
        );
    }

    #[test]
    fn canonical_resource_key_handles_plain_ports_ipv6_and_non_http_schemes() {
        assert_eq!(
            canonical_resource_key("http://Example.COM:80/files/ubuntu.iso"),
            "http://example.com/files/ubuntu.iso"
        );
        assert_eq!(
            canonical_resource_key(
                "https://[::1]:8443/stream/master.m3u8?hdnts=exp=1~hmac=abc#live"
            ),
            "https://[::1]:8443/stream/master.m3u8"
        );
        assert_eq!(
            canonical_resource_key("  magnet:?xt=urn:btih:abcdef123456#tracker  "),
            "magnet:?xt=urn:btih:abcdef123456"
        );
        assert_eq!(
            canonical_resource_key(" ftp://files.example.org/pub/archive.tar.gz#readme "),
            "ftp://files.example.org/pub/archive.tar.gz"
        );
    }

    #[test]
    fn canonical_resource_key_scopes_bilibili_and_key_params_away_from_generic_hosts() {
        let win_dl = "https://example.com/download?os=win&platform=x64&token=t1";
        let mac_dl = "https://example.com/download?os=mac&platform=arm64&token=t2";
        assert_eq!(
            canonical_resource_key(win_dl),
            "https://example.com/download?os=win&platform=x64"
        );
        assert_eq!(
            canonical_resource_key(mac_dl),
            "https://example.com/download?os=mac&platform=arm64"
        );
        assert_ne!(
            canonical_resource_key(win_dl),
            canonical_resource_key(mac_dl)
        );

        let key_a = "https://example.com/get?key=fileA&expires=100";
        let key_b = "https://example.com/get?key=fileB&expires=200";
        assert_eq!(
            canonical_resource_key(key_a),
            "https://example.com/get?key=fileA"
        );
        assert_ne!(canonical_resource_key(key_a), canonical_resource_key(key_b));

        let ws_signed_1 =
            "https://cdn.example.com/vod/1080p.mp4?wsSecret=sec1&wsTime=111&key=k1&id=ep1";
        let ws_signed_2 =
            "https://cdn.example.com/vod/1080p.mp4?wsSecret=sec2&wsTime=222&key=k2&id=ep1";
        assert_eq!(
            canonical_resource_key(ws_signed_1),
            "https://cdn.example.com/vod/1080p.mp4?id=ep1"
        );
        assert_eq!(
            canonical_resource_key(ws_signed_1),
            canonical_resource_key(ws_signed_2)
        );

        let akam_bili_1 =
            "https://upos-hz-mirrorakam.akamaized.net/upgcxcode/01/02/30080.m4s?os=akam&platform=pc&trid=1&key=a&buvid=b1";
        let akam_bili_2 =
            "https://upos-hz-mirrorakam.akamaized.net/upgcxcode/01/02/30080.m4s?os=bcache&platform=html5&trid=2&key=b&buvid=b2";
        assert_eq!(
            canonical_resource_key(akam_bili_1),
            "https://upos-hz-mirrorakam.akamaized.net/upgcxcode/01/02/30080.m4s"
        );
        assert_eq!(
            canonical_resource_key(akam_bili_1),
            canonical_resource_key(akam_bili_2)
        );
    }

    #[test]
    fn canonical_resource_key_never_panics_on_multibyte_utf8_paths_or_non_http_inputs() {
        for (input, expected) in [
            ("C:\\下载\\movie.torrent", "C:\\下载\\movie.torrent"),
            (
                "  C:\\下载\\合集.torrent#section  ",
                "C:\\下载\\合集.torrent",
            ),
            ("D:\\动漫\\番剧.m3u8", "D:\\动漫\\番剧.m3u8"),
            ("中文路径\\测试.torrent", "中文路径\\测试.torrent"),
            ("一二三", "一二三"),
            ("🚀://test/资源#frag", "🚀://test/资源"),
            ("a🚀🔥✨", "a🚀🔥✨"),
            ("ééééé", "ééééé"),
        ] {
            assert_eq!(canonical_resource_key(input), expected);
        }

        for prefix_len in 0..=8 {
            let ascii_prefix = "a".repeat(prefix_len);
            for glyph in ["é", "载", "🚀"] {
                let sample = format!("{ascii_prefix}{glyph}{glyph}{glyph}");
                assert_eq!(canonical_resource_key(&sample), sample);
            }
        }

        assert_eq!(
            canonical_resource_key("https://例え.jp:443/视频/第01集.mp4?token=exp1&part=上集#章节"),
            "https://例え.jp/视频/第01集.mp4?part=上集"
        );
        assert_eq!(
            canonical_resource_key("HTTP://localhost:80/下载/测试.zip?sig=abc&id=42#top"),
            "http://localhost/下载/测试.zip?id=42"
        );
    }
}
