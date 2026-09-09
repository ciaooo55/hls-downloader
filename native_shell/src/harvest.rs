//! Single-page link harvest. No JS, no crawl, no HEAD probes.

#[derive(Debug, Clone, PartialEq)]
pub struct HarvestLink {
    pub url: String,
    pub filename: String,
    pub extension: String,
    pub category: String,
    pub size_hint: u64,
}

const DEFAULT_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "flv", "mp3", "m4a", "aac", "flac", "wav",
    "ogg", "opus", "zip", "7z", "rar", "tar", "gz", "bz2", "xz", "iso", "exe", "msi", "msix",
    "appx", "dmg", "apk", "deb", "rpm", "pdf", "epub", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "m3u8", "mpd", "m3u", "torrent", "bin", "meta4", "metalink",
];

pub fn harvest_html(html: &str, base: &str) -> Vec<HarvestLink> {
    harvest_html_filtered(html, base, 0)
}

pub fn harvest_html_filtered(html: &str, base: &str, min_bytes: u64) -> Vec<HarvestLink> {
    let mut links = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (raw, tag_size) in extract_urls(html, base) {
        if !seen.insert(raw.to_ascii_lowercase()) {
            continue;
        }
        if let Some(mut link) = to_link(&raw) {
            if link.size_hint == 0 {
                link.size_hint = tag_size;
            }
            if min_bytes > 0 && link.size_hint > 0 && link.size_hint < min_bytes {
                continue;
            }
            links.push(link);
            if links.len() >= 100 {
                break;
            }
        }
    }
    links
}

fn extract_urls(html: &str, base: &str) -> Vec<(String, u64)> {
    let mut urls = Vec::new();
    let mut rest = html;
    while let Some(index) = rest.find("href=") {
        let after = &rest[index + 5..];
        let quote = after.chars().next();
        rest = &after[1.min(after.len())..];
        let value = match quote {
            Some('"') => after[1..].split('"').next().unwrap_or(""),
            Some('\'') => after[1..].split('\'').next().unwrap_or(""),
            _ => continue,
        };
        let lookahead = after.get(..200.min(after.len())).unwrap_or(after);
        let tag_size = parse_data_size(lookahead);
        if let Some(url) = resolve(base, value) {
            urls.push((url, tag_size));
        }
        if urls.len() >= 512 {
            break;
        }
    }
    let mut search = html;
    while let Some(index) = find_abs(search) {
        let slice = &search[index..];
        let end = slice
            .find(|ch: char| ch.is_ascii_whitespace() || matches!(ch, '<' | '"' | '\'' | ')'))
            .unwrap_or(slice.len().min(2048));
        let raw = slice[..end].trim_end_matches(['.', ',', ';']);
        if let Some(url) = resolve(base, raw) {
            urls.push((url, 0));
        }
        search = &slice[end.max(1)..];
        if urls.len() >= 512 {
            break;
        }
    }
    urls
}

fn parse_data_size(tag: &str) -> u64 {
    let lower = tag.to_ascii_lowercase();
    for key in ["data-size=\"", "data-size='", "datasize=\"", "size=\""] {
        if let Some(index) = lower.find(key) {
            let rest = &tag[index + key.len()..];
            let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
            if let Ok(value) = digits.parse::<u64>() {
                return value;
            }
        }
    }
    0
}

fn size_hint_from_url(url: &str) -> u64 {
    let query = url.split_once('?').map(|(_, rest)| rest).unwrap_or("");
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or(("", ""));
        if matches!(key, "size" | "filesize" | "clen") {
            if let Ok(parsed) = value.parse::<u64>() {
                return parsed;
            }
        }
    }
    0
}

fn find_abs(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    [
        "https://", "http://", "ftp://", "ftps://", "sftp://", "magnet:?",
    ]
    .into_iter()
    .filter_map(|needle| lower.find(needle))
    .min()
}

fn resolve(base: &str, reference: &str) -> Option<String> {
    let value = html_unescape(reference.trim().trim_start_matches('\u{feff}'));
    if value.is_empty() || value.starts_with('#') || value.chars().any(|ch| ch.is_control()) {
        return None;
    }
    if crate::http_engine::remote_resource_url_allowed(&value) {
        return Some(value);
    }
    if has_absolute_scheme(&value) {
        return None;
    }
    if value.starts_with("//") {
        let scheme = base
            .split("://")
            .next()
            .unwrap_or("https")
            .to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https" | "ftp" | "ftps" | "sftp") {
            return None;
        }
        return Some(format!("{scheme}:{value}"));
    }
    if value.starts_with('/') {
        if let Some(scheme) = base.find("://") {
            let after = &base[scheme + 3..];
            let origin_end = after
                .find('/')
                .map(|index| scheme + 3 + index)
                .unwrap_or(base.len());
            return Some(format!("{}{value}", &base[..origin_end]));
        }
    }
    None
}

fn has_absolute_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let mut index = 1;
    while index < bytes.len()
        && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'+' | b'.' | b'-'))
    {
        index += 1;
    }
    bytes.get(index) == Some(&b':')
}

fn to_link(url: &str) -> Option<HarvestLink> {
    if url.to_ascii_lowercase().starts_with("magnet:") {
        return Some(HarvestLink {
            url: url.to_string(),
            filename: "torrent".into(),
            extension: "torrent".into(),
            category: "torrent".into(),
            size_hint: 0,
        });
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let filename = path
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("download");
    let extension = filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if !DEFAULT_EXTS.iter().any(|item| *item == extension) {
        return None;
    }
    Some(HarvestLink {
        url: url.to_string(),
        filename: filename.to_string(),
        extension: extension.clone(),
        category: category_for(&extension).into(),
        size_hint: size_hint_from_url(url),
    })
}

pub fn category_for(extension: &str) -> &'static str {
    match extension {
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "ts" | "flv" => "video",
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "opus" => "audio",
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "iso" => "archive",
        "pdf" | "epub" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" => "document",
        "exe" | "msi" | "msix" | "appx" | "dmg" | "apk" | "deb" | "rpm" => "program",
        "m3u8" | "mpd" | "m3u" => "playlist",
        "torrent" => "torrent",
        _ => "other",
    }
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harvests_href_and_absolute_file_links() {
        let html = r#"<html><a href="/files/a.mp4">a</a><a href="https://cdn.test/b.zip">b</a><a href="/about">no</a> magnet:?xt=urn:btih:abc</html>"#;
        let links = harvest_html(html, "https://site.test/page");
        assert!(links.iter().any(|item| item.url.ends_with("/files/a.mp4")));
        assert!(links.iter().any(|item| item.url.ends_with("/b.zip")));
        assert!(links.iter().any(|item| item.category == "video"));
        assert!(links.iter().any(|item| item.category == "archive"));
        assert!(links.iter().any(|item| item.category == "torrent"));
        assert!(links
            .iter()
            .all(|item| item.url != "https://site.test/about"));
        let sized = harvest_html_filtered(
            r#"<a href="/files/tiny.mp4" data-size="100">t</a><a href="/files/big.mp4" data-size="9000">b</a>"#,
            "https://site.test/page",
            1000,
        );
        assert!(sized
            .iter()
            .any(|item| item.url.ends_with("/files/big.mp4")));
        assert!(sized
            .iter()
            .all(|item| !item.url.ends_with("/files/tiny.mp4")));
        let html = r#"<a href="javascript:alert(1)">x</a><a href="JAVASCRIPT:alert(1)">y</a><a href="file:///C:/secret.mp4">z</a><a href="&#xFEFF;javascript:alert(1)">b</a><a href="ms-msdt:foo.mp4">m</a>"#;
        let links = harvest_html(html, "https://site.test/page");
        assert!(links.is_empty());
    }
}
