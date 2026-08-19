//! Single-page link harvest. No JS, no crawl, no HEAD probes.

#[derive(Debug, Clone, PartialEq)]
pub struct HarvestLink {
    pub url: String,
    pub filename: String,
    pub extension: String,
    pub category: String,
}

const DEFAULT_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "flv", "mp3", "m4a", "aac", "flac", "wav",
    "ogg", "opus", "zip", "7z", "rar", "tar", "gz", "bz2", "xz", "iso", "exe", "msi", "msix",
    "appx", "dmg", "apk", "deb", "rpm", "pdf", "epub", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "m3u8", "mpd", "m3u", "torrent", "bin", "meta4", "metalink",
];

pub fn harvest_html(html: &str, base: &str) -> Vec<HarvestLink> {
    let mut links = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for raw in extract_urls(html, base) {
        if !seen.insert(raw.to_ascii_lowercase()) {
            continue;
        }
        if let Some(link) = to_link(&raw) {
            links.push(link);
            if links.len() >= 100 {
                break;
            }
        }
    }
    links
}

fn extract_urls(html: &str, base: &str) -> Vec<String> {
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
        if let Some(url) = resolve(base, value) {
            urls.push(url);
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
            urls.push(url);
        }
        search = &slice[end.max(1)..];
        if urls.len() >= 512 {
            break;
        }
    }
    urls
}

fn find_abs(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    ["https://", "http://", "ftp://", "ftps://", "sftp://", "magnet:?"]
        .into_iter()
        .filter_map(|needle| lower.find(needle))
        .min()
}

fn resolve(base: &str, reference: &str) -> Option<String> {
    let value = html_unescape(reference.trim());
    let lower = value.to_ascii_lowercase();
    if value.is_empty()
        || value.starts_with('#')
        || lower.starts_with("javascript:")
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("file:")
        || value.contains('\r')
        || value.contains('\n')
    {
        return None;
    }
    if lower.starts_with("magnet:?")
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("sftp://")
    {
        return Some(value);
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
            let origin_end = after.find('/').map(|index| scheme + 3 + index).unwrap_or(base.len());
            return Some(format!("{}{value}", &base[..origin_end]));
        }
    }
    None
}

fn to_link(url: &str) -> Option<HarvestLink> {
    if url.to_ascii_lowercase().starts_with("magnet:") {
        return Some(HarvestLink {
            url: url.to_string(),
            filename: "torrent".into(),
            extension: "torrent".into(),
            category: "torrent".into(),
        });
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let filename = path.rsplit('/').find(|part| !part.is_empty()).unwrap_or("download");
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
        assert!(links.iter().all(|item| item.url != "https://site.test/about"));
        let html = r#"<a href="javascript:alert(1)">x</a><a href="JAVASCRIPT:alert(1)">y</a><a href="file:///C:/secret.mp4">z</a>"#;
        let links = harvest_html(html, "https://site.test/page");
        assert!(links.is_empty());
    }
}
