use crate::{ResourceKind, StreamVariant};

pub fn classify_url(url: &str) -> ResourceKind {
    let lower = url.trim().to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    if lower.starts_with("magnet:") || path.ends_with(".torrent") {
        ResourceKind::Torrent
    } else if lower.ends_with(".metalink") || lower.ends_with(".meta4") {
        ResourceKind::File
    } else if lower.starts_with("sftp://") {
        ResourceKind::Sftp
    } else if lower.starts_with("ftp://") || lower.starts_with("ftps://") {
        ResourceKind::Ftp
    } else if path.ends_with(".m3u8") || lower.contains("vnd.apple.mpegurl") {
        if path_marks_live(path) || query_marks_live(&lower) {
            ResourceKind::Live
        } else {
            ResourceKind::Hls
        }
    } else if path.ends_with(".mpd") || lower.contains("dash+xml") {
        ResourceKind::Dash
    } else {
        ResourceKind::File
    }
}

fn path_marks_live(url_without_query: &str) -> bool {
    let path = if let Some((_, authority_and_path)) = url_without_query.split_once("://") {
        authority_and_path
            .find('/')
            .map(|index| &authority_and_path[index..])
            .unwrap_or("")
    } else {
        url_without_query
    };
    path.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token.eq_ignore_ascii_case("live"))
}

fn query_marks_live(url: &str) -> bool {
    let Some((_, query)) = url.split_once('?') else {
        return false;
    };
    let query = query.split('#').next().unwrap_or(query);
    query.split('&').any(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if !key.eq_ignore_ascii_case("live") {
            return false;
        }
        let value = value.trim().to_ascii_lowercase();
        value.is_empty() || !matches!(value.as_str(), "0" | "false" | "no" | "off")
    })
}

pub fn kind_label(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Hls => "HLS 点播",
        ResourceKind::Live => "HLS 直播",
        ResourceKind::Dash => "DASH",
        ResourceKind::Ftp => "FTP",
        ResourceKind::Sftp => "SFTP",
        ResourceKind::Torrent => "种子 / Magnet",
        ResourceKind::File => "普通文件",
    }
}

pub fn probe_url(url: &str) -> Result<(ResourceKind, String, Vec<StreamVariant>), String> {
    let (kind, label, variants, _) = probe_with_harvest(url)?;
    Ok((kind, label, variants))
}

pub fn probe_with_harvest(
    url: &str,
) -> Result<
    (
        ResourceKind,
        String,
        Vec<StreamVariant>,
        Vec<crate::HarvestLink>,
    ),
    String,
> {
    probe_with_harvest_context(url, &Default::default(), "")
}

pub fn probe_with_harvest_context(
    url: &str,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
) -> Result<
    (
        ResourceKind,
        String,
        Vec<StreamVariant>,
        Vec<crate::HarvestLink>,
    ),
    String,
> {
    let url = url.trim();
    if url.is_empty() {
        return Err("请输入链接".into());
    }
    if !crate::http_engine::remote_resource_url_allowed(url) {
        return Err("链接协议不受支持".into());
    }
    let path_lower = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    let mut kind = classify_url(url);
    let mut variants = Vec::new();
    let mut harvest = Vec::new();
    if matches!(kind, ResourceKind::Hls | ResourceKind::Live) {
        let (status, body) = crate::http_engine::fetch_bytes(url, headers, proxy)
            .map_err(|error| error.to_string())?;
        if status != 200 && status != 206 {
            return Err(format!("识别播放列表失败 HTTP {status}"));
        }
        let playlist = crate::media::parse_playlist(&String::from_utf8_lossy(&body), url)?;
        variants = crate::media::variant_choices(&playlist);
        variants.extend(crate::media::audio_choices(&playlist));
    } else if kind == ResourceKind::Dash {
        let (status, body) = crate::http_engine::fetch_bytes(url, headers, proxy)
            .map_err(|error| error.to_string())?;
        if status != 200 && status != 206 {
            return Err(format!("识别 DASH 失败 HTTP {status}"));
        }
        let manifest = crate::media::parse_mpd(&String::from_utf8_lossy(&body), url)?;
        variants = crate::media::representation_choices(&manifest);
        variants.extend(crate::media::dash_audio_choices(&manifest));
    } else if kind == ResourceKind::File
        && (url.starts_with("http://") || url.starts_with("https://"))
    {
        if let Ok((status, body)) = crate::http_engine::fetch_bytes(url, headers, proxy) {
            if status == 200 || status == 206 {
                let text = String::from_utf8_lossy(&body);
                if text.contains("#EXTM3U") {
                    kind = if path_marks_live(&path_lower) || query_marks_live(url) {
                        ResourceKind::Live
                    } else {
                        ResourceKind::Hls
                    };
                    if let Ok(playlist) = crate::media::parse_playlist(&text, url) {
                        variants = crate::media::variant_choices(&playlist);
                        variants.extend(crate::media::audio_choices(&playlist));
                    }
                } else if text.contains("<MPD") || text.contains("urn:mpeg:dash") {
                    kind = ResourceKind::Dash;
                    if let Ok(manifest) = crate::media::parse_mpd(&text, url) {
                        variants = crate::media::representation_choices(&manifest);
                        variants.extend(crate::media::dash_audio_choices(&manifest));
                    }
                } else if looks_html(&text) {
                    harvest = crate::harvest_html(&text, url);
                }
            }
        }
    }
    let label = if !harvest.is_empty() {
        format!("页面 · {} 个链接", harvest.len())
    } else if variants.is_empty() {
        kind_label(kind).to_string()
    } else {
        format!("{} · {} 个画质", kind_label(kind), variants.len())
    };
    Ok((kind, label, variants, harvest))
}

fn looks_html(text: &str) -> bool {
    let start = text
        .get(..512.min(text.len()))
        .unwrap_or(text)
        .to_ascii_lowercase();
    start.contains("<html")
        || start.contains("<!doctype")
        || start.contains("href=")
        || start.contains("<a ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_schemes() {
        assert_eq!(classify_url("https://cdn/a.m3u8"), ResourceKind::Hls);
        assert_eq!(classify_url("https://cdn/a.mpd"), ResourceKind::Dash);
        assert_eq!(
            classify_url("magnet:?xt=urn:btih:abc"),
            ResourceKind::Torrent
        );
        assert_eq!(
            classify_url("https://cdn/a.torrent?token=signed#download"),
            ResourceKind::Torrent
        );
        assert_eq!(classify_url("sftp://nas/a.bin"), ResourceKind::Sftp);
        assert_eq!(classify_url("https://cdn/a.zip"), ResourceKind::File);
        assert_eq!(kind_label(ResourceKind::Hls), "HLS 点播");
        assert!(looks_html(
            "<!doctype html><a href=\"https://x/a.bin\">x</a>"
        ));
        assert!(!looks_html("PK\u{3}\u{4}binary"));
        assert!(probe_url("javascript:alert(1)").is_err());
        assert!(probe_url("file:///C:/Windows/win.ini").is_err());
        assert!(probe_url("ms-msdt:foo").is_err());
    }

    #[test]
    fn query_values_do_not_reclassify_the_resource_path() {
        assert_eq!(
            classify_url("https://cdn/file.mp4?next=https://edge/live.m3u8"),
            ResourceKind::File
        );
        assert_eq!(
            classify_url("https://cdn/file.mp4?manifest=clip.mpd"),
            ResourceKind::File
        );
        assert_eq!(
            classify_url("https://cdn/movie.m3u8?token=live-session"),
            ResourceKind::Hls
        );
        assert_eq!(
            classify_url("https://cdn/live/channel.m3u8?token=session"),
            ResourceKind::Live
        );
        assert_eq!(
            classify_url("https://cdn/movie.m3u8?live=1"),
            ResourceKind::Live
        );
        assert_eq!(
            classify_url("https://cdn/movie.m3u8?live=false"),
            ResourceKind::Hls
        );
        assert_eq!(
            classify_url("https://cdn/archive.m3u8.txt"),
            ResourceKind::File
        );
        assert_eq!(
            classify_url("https://cdn/manifest.mpd.json"),
            ResourceKind::File
        );
        for value in ["FALSE", "False", "NO", "Off"] {
            assert!(
                !query_marks_live(&format!("https://cdn/movie.m3u8?live={value}")),
                "live={value} must remain false"
            );
        }
        assert_eq!(
            classify_url("https://cdn/opaque?content_type=application/vnd.apple.mpegurl&live=1"),
            ResourceKind::Live
        );
        assert_eq!(
            classify_url("https://cdn/opaque?content_type=application/vnd.apple.mpegurl"),
            ResourceKind::Hls
        );
    }

    #[test]
    fn live_path_detection_uses_path_tokens_not_hostname_or_substrings() {
        assert_eq!(
            classify_url("https://live.example.com/movie.m3u8"),
            ResourceKind::Hls
        );
        assert_eq!(
            classify_url("https://cdn/deliver/movie.m3u8"),
            ResourceKind::Hls
        );
        assert_eq!(
            classify_url("https://cdn/olive/movie.m3u8"),
            ResourceKind::Hls
        );
        assert_eq!(
            classify_url("https://cdn/live/channel.m3u8"),
            ResourceKind::Live
        );
        assert_eq!(
            classify_url("https://cdn/events/live.m3u8"),
            ResourceKind::Live
        );
        assert_eq!(
            classify_url("https://cdn/events/my-live-stream.m3u8"),
            ResourceKind::Live
        );
    }
}
