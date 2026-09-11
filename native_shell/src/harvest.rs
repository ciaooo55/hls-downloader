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
        if !seen.insert(raw.clone()) {
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
    while let Some(index) = find_href(rest) {
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

fn find_href(text: &str) -> Option<usize> {
    text.as_bytes()
        .windows(5)
        .position(|window| window.eq_ignore_ascii_case(b"href="))
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
    let decoded = html_unescape(reference.trim());
    let value = decoded.trim_start_matches('\u{feff}');
    if value.is_empty() || value.starts_with('#') || value.chars().any(|ch| ch.is_control()) {
        return None;
    }
    if crate::http_engine::remote_resource_url_allowed(value) {
        return Some(value.to_string());
    }
    if has_absolute_scheme(value) {
        return None;
    }
    let clean_base = base.split(['?', '#']).next().unwrap_or(base);
    let scheme_end = clean_base.find("://")?;
    let scheme = clean_base[..scheme_end].to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "ftp" | "ftps" | "sftp") {
        return None;
    }
    if value.starts_with("//") {
        return Some(format!("{scheme}:{value}"));
    }
    let authority_start = scheme_end + 3;
    let path_start = clean_base[authority_start..]
        .find('/')
        .map(|index| authority_start + index);
    let origin_end = path_start.unwrap_or(clean_base.len());
    let origin = &clean_base[..origin_end];
    let (reference_path, suffix) = split_reference_suffix(value);
    if reference_path.is_empty() {
        let current_path = path_start.map(|index| &clean_base[index..]).unwrap_or("/");
        return Some(format!("{origin}{current_path}{suffix}"));
    }
    let joined_path = if reference_path.starts_with('/') {
        reference_path.to_string()
    } else {
        let base_path = path_start.map(|index| &clean_base[index..]).unwrap_or("/");
        let directory = base_path
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .unwrap_or("");
        format!("{directory}/{reference_path}")
    };
    let path = normalize_url_path(&joined_path);
    Some(format!("{origin}{path}{suffix}"))
}

fn split_reference_suffix(value: &str) -> (&str, &str) {
    value
        .find(['?', '#'])
        .map(|index| (&value[..index], &value[index..]))
        .unwrap_or((value, ""))
}

fn normalize_url_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let preserve_trailing_slash =
        path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "." => {}
            ".." => {
                let at_absolute_root = absolute && parts.len() == 1 && parts[0].is_empty();
                if !at_absolute_root {
                    parts.pop();
                }
            }
            _ => parts.push(part),
        }
    }
    let mut normalized = parts.join("/");
    if absolute && !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    if absolute && normalized.is_empty() {
        normalized.push('/');
    }
    if preserve_trailing_slash && !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
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
    let mut decoded = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative) = value[cursor..].find('&') {
        let start = cursor + relative;
        decoded.push_str(&value[cursor..start]);
        let rest = &value[start..];
        if let Some((character, consumed)) = decode_html_entity(rest) {
            decoded.push(character);
            cursor = start + consumed;
        } else {
            decoded.push('&');
            cursor = start + 1;
        }
    }
    decoded.push_str(&value[cursor..]);
    decoded
}

fn decode_html_entity(value: &str) -> Option<(char, usize)> {
    for (entity, character) in [
        ("&amp;", '&'),
        ("&quot;", '"'),
        ("&apos;", '\''),
        ("&lt;", '<'),
        ("&gt;", '>'),
    ] {
        if value.starts_with(entity) {
            return Some((character, entity.len()));
        }
    }
    let numeric = value.strip_prefix("&#")?;
    let end = numeric.find(';')?;
    if end == 0 || end > 8 {
        return None;
    }
    let token = &numeric[..end];
    let (digits, radix) = token
        .strip_prefix('x')
        .or_else(|| token.strip_prefix('X'))
        .map(|digits| (digits, 16))
        .unwrap_or((token, 10));
    if digits.is_empty() {
        return None;
    }
    let scalar = u32::from_str_radix(digits, radix).ok()?;
    let character = char::from_u32(scalar)?;
    Some((character, 2 + end + 1))
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

    #[test]
    fn harvests_case_insensitive_and_relative_hrefs() {
        let html = r#"<a HREF="child.zip">child</a><a HrEf="../parent.mp4?download=1">parent</a>"#;
        let links = harvest_html(html, "https://site.test/dir/page.html?token=secret");
        assert!(links
            .iter()
            .any(|item| item.url == "https://site.test/dir/child.zip"));
        assert!(links
            .iter()
            .any(|item| item.url == "https://site.test/parent.mp4?download=1"));
        assert!(harvest_html(r#"<a HREF="child.zip">x</a>"#, "file:///C:/page.html").is_empty());
    }

    #[test]
    fn harvests_query_only_href_against_current_document() {
        let links = harvest_html(
            r#"<a href="?download=1">download</a>"#,
            "https://site.test/dir/file.zip?token=secret",
        );
        assert!(links
            .iter()
            .any(|item| item.url == "https://site.test/dir/file.zip?download=1"));

        assert_eq!(
            resolve("https://site.test/dir/?token=secret", "?download=1"),
            Some("https://site.test/dir/?download=1".to_string())
        );
    }

    #[test]
    fn preserves_repeated_path_separators_when_resolving_links() {
        assert_eq!(
            resolve("https://site.test/dir/page.html", "/cdn//signed/file.zip"),
            Some("https://site.test/cdn//signed/file.zip".to_string())
        );
        assert_eq!(
            resolve(
                "https://site.test/dir/sub/page.html",
                "../assets//signed/./file.zip"
            ),
            Some("https://site.test/dir/assets//signed/file.zip".to_string())
        );
        assert_eq!(normalize_url_path("/a//b/../c/"), "/a//c/");
        assert_eq!(normalize_url_path("/a/b/.."), "/a/");
    }

    #[test]
    fn decodes_numeric_html_entities_without_double_decoding() {
        for entity in ["&#38;", "&#x26;", "&#X26;"] {
            assert_eq!(
                resolve(
                    "https://site.test/page",
                    &format!("https://cdn.test/file.zip?x=1{entity}y=2")
                ),
                Some("https://cdn.test/file.zip?x=1&y=2".to_string())
            );
        }
        assert_eq!(html_unescape("&amp;#38;"), "&#38;");
        assert!(resolve(
            "https://site.test/page",
            "&#xFEFF;javascript:alert(1).mp4"
        )
        .is_none());
    }

    #[test]
    fn case_distinct_paths_survive_harvest_dedup() {
        let html =
            r#"<a href="https://cdn.test/A.zip">A</a><a href="https://cdn.test/a.zip">a</a>"#;
        let links = harvest_html(html, "https://site.test/page");
        assert!(links
            .iter()
            .any(|item| item.url == "https://cdn.test/A.zip"));
        assert!(links
            .iter()
            .any(|item| item.url == "https://cdn.test/a.zip"));
    }
}
