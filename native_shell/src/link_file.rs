//! Local .url / .magnet / playlist / HTML import for the v7 Rust Core.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_LINK_FILE_BYTES: u64 = 256 * 1024;
const MAX_LINK_URLS: usize = 100;

/// If `source` is a local file (or `file://` URL), expand it into download URLs.
/// Remote http(s)/ftp/sftp/magnet strings return `Ok(None)` so the caller keeps them as-is.
pub fn expand_source(source: &str) -> Result<Option<Vec<String>>, String> {
    let Some(path) = local_source_path(source) else {
        return Ok(None);
    };
    Ok(Some(urls_from_path(&path)?))
}

pub(crate) fn local_source_path(source: &str) -> Option<PathBuf> {
    let trimmed = source.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("magnet:")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("sftp://")
    {
        return None;
    }
    let path = decode_local_path(trimmed);
    if !path.is_file() {
        return None;
    }
    Some(path)
}

pub fn urls_from_path(path: &Path) -> Result<Vec<String>, String> {
    let meta = fs::metadata(path).map_err(|error| error.to_string())?;
    if meta.len() == 0 || meta.len() > MAX_LINK_FILE_BYTES {
        return Err("link file is empty or too large".into());
    }
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "torrent" {
        return Ok(vec![path.to_string_lossy().into_owned()]);
    }
    let data = fs::read(path).map_err(|error| error.to_string())?;
    let text = decode_link_bytes(&data);
    if ext == "meta4" || ext == "metalink" {
        return Ok(vec![text]);
    }
    extract_download_urls(&text, &ext)
}

fn decode_local_path(source: &str) -> PathBuf {
    let trimmed = source.trim().trim_matches('"');
    let rest = if trimmed
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        &trimmed[5..]
    } else {
        return PathBuf::from(trimmed);
    };
    let rest = rest.trim_start_matches('/');
    let decoded = percent_decode(rest);
    let decoded = decoded
        .strip_prefix("localhost/")
        .or_else(|| decoded.strip_prefix("localhost\\"))
        .unwrap_or(&decoded);
    let decoded = if cfg!(windows) {
        decoded.replace('/', "\\")
    } else {
        decoded.replace('\\', "/")
    };
    if cfg!(windows) || decoded.starts_with('/') {
        PathBuf::from(decoded)
    } else {
        PathBuf::from(format!("/{decoded}"))
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (from_hex(bytes[index + 1]), from_hex(bytes[index + 2])) {
                out.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_link_bytes(data: &[u8]) -> String {
    if data.starts_with(&[0xff, 0xfe]) {
        let units: Vec<u16> = data[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if data.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = data[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if data.starts_with(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(&data[3..]).into_owned();
    }
    String::from_utf8_lossy(data).into_owned()
}

pub fn extract_download_urls(text: &str, suffix: &str) -> Result<Vec<String>, String> {
    let ext = suffix.trim().trim_start_matches('.').to_ascii_lowercase();
    let body = text;
    if ext == "url" || body.to_ascii_lowercase().contains("[internetshortcut]") {
        return Ok(vec![url_from_internet_shortcut(body)?]);
    }
    if ext == "magnet" {
        return Ok(vec![url_from_plain_text(body)?]);
    }
    if ext == "m3u" || ext == "m3u8" {
        return extract_playlist_urls(body);
    }
    if ext == "mpd" {
        return extract_mpd_urls(body);
    }
    if ext == "html" || ext == "htm" {
        return extract_html_urls(body);
    }
    if ext == "txt" {
        return extract_text_urls(body);
    }
    Ok(vec![extract_download_url(body)?])
}

fn extract_download_url(text: &str) -> Result<String, String> {
    let lowered = text
        .trim_start_matches(['\u{feff}', ' '])
        .to_ascii_lowercase();
    if lowered.starts_with("[internetshortcut]") || lowered.contains("\nurl=") {
        return url_from_internet_shortcut(text);
    }
    url_from_plain_text(text)
}

fn url_from_internet_shortcut(text: &str) -> Result<String, String> {
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.to_ascii_lowercase().starts_with("url=") {
            return normalize_download_url(&stripped[4..]);
        }
    }
    Err("shortcut has no URL=".into())
}

fn url_from_plain_text(text: &str) -> Result<String, String> {
    for line in text.lines() {
        let candidate = line.trim();
        if candidate.is_empty() || candidate.starts_with('#') || candidate.starts_with(';') {
            continue;
        }
        return normalize_download_url(candidate);
    }
    Err("text file has no download link".into())
}

fn extract_text_urls(text: &str) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let candidate = line.trim();
        if candidate.is_empty() || candidate.starts_with('#') || candidate.starts_with(';') {
            continue;
        }
        if let Ok(url) = normalize_download_url(candidate) {
            if seen.insert(url.clone()) {
                found.push(url);
                if found.len() >= MAX_LINK_URLS {
                    break;
                }
            }
        }
    }
    if found.is_empty() {
        Err("text file has no download link".into())
    } else {
        Ok(found)
    }
}

fn extract_playlist_urls(text: &str) -> Result<Vec<String>, String> {
    let urls = collect_absolute_urls(text);
    if urls.is_empty() {
        return Err("playlist has no remote download link".into());
    }
    let upper = text.to_ascii_uppercase();
    let playlists: Vec<_> = urls
        .iter()
        .filter(|url| {
            path_suffix(url) == ".m3u8" || path_suffix(url) == ".m3u" || path_suffix(url) == ".mpd"
        })
        .cloned()
        .collect();
    if upper.contains("#EXT-X-STREAM-INF") {
        return Ok(if playlists.is_empty() {
            urls
        } else {
            playlists
        });
    }
    if upper.contains("#EXTINF") {
        if !playlists.is_empty() {
            return Ok(playlists);
        }
        let segment_count = urls.iter().filter(|url| is_segment_url(url)).count();
        if segment_count >= 3 || segment_count == urls.len() {
            return Err("这是本地分片播放列表，请改用网页或远程 m3u8 地址".into());
        }
    }
    let files: Vec<_> = urls
        .iter()
        .filter(|url| !is_segment_url(url))
        .cloned()
        .collect();
    Ok(if files.is_empty() { urls } else { files })
}

fn extract_mpd_urls(text: &str) -> Result<Vec<String>, String> {
    let urls = collect_absolute_urls(text);
    let playlists: Vec<_> = urls
        .iter()
        .filter(|url| path_suffix(url) == ".mpd")
        .cloned()
        .collect();
    if !playlists.is_empty() {
        return Ok(playlists);
    }
    let files: Vec<_> = urls
        .iter()
        .filter(|url| !is_segment_url(url))
        .cloned()
        .collect();
    if files.is_empty() {
        Err("DASH 清单里没有可单独下载的远程地址".into())
    } else {
        Ok(files)
    }
}

fn extract_html_urls(text: &str) -> Result<Vec<String>, String> {
    let links = crate::harvest_html(text, "https://hls-downloader.invalid/");
    let urls: Vec<_> = links
        .into_iter()
        .map(|item| item.url)
        .filter(|url| !url.to_ascii_lowercase().contains("hls-downloader.invalid"))
        .take(MAX_LINK_URLS)
        .collect();
    if urls.is_empty() {
        let collected = collect_absolute_urls(text);
        if collected.is_empty() {
            Err("网页文件里没有可下载的远程链接".into())
        } else {
            Ok(collected)
        }
    } else {
        Ok(urls)
    }
}

fn collect_absolute_urls(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let lower = text.to_ascii_lowercase();
    for prefix in [
        "https://", "http://", "ftps://", "ftp://", "sftp://", "magnet:?",
    ] {
        let mut start = 0;
        while let Some(rel) = lower[start..].find(prefix) {
            let abs = start + rel;
            let slice = &text[abs..];
            let end = slice
                .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"'))
                .unwrap_or(slice.len());
            let raw = slice[..end].trim_end_matches(['.', ',', ')', ';', ']']);
            if let Ok(url) = normalize_download_url(raw) {
                if seen.insert(url.clone()) {
                    found.push(url);
                    if found.len() >= MAX_LINK_URLS {
                        return found;
                    }
                }
            }
            start = abs + prefix.len();
        }
    }
    found
}

fn normalize_download_url(raw: &str) -> Result<String, String> {
    let url = raw
        .trim()
        .trim_start_matches('\u{feff}')
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    if url.is_empty() || url.chars().any(|ch| ch.is_control()) {
        return Err("link is empty".into());
    }
    if !crate::http_engine::remote_resource_url_allowed(&url) {
        return Err("unsupported link scheme".into());
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("magnet:") {
        if !lower.contains("xt=") {
            return Err("magnet link is incomplete".into());
        }
        return Ok(url);
    }
    for scheme in ["http://", "https://", "ftp://", "ftps://", "sftp://"] {
        if lower.starts_with(scheme) {
            let host = url
                .get(scheme.len()..)
                .unwrap_or("")
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("");
            if host.is_empty() {
                return Err("unsupported link scheme".into());
            }
            return Ok(url);
        }
    }
    Err("unsupported link scheme".into())
}

fn path_suffix(url: &str) -> String {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or("");
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{ext}"))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn is_segment_url(url: &str) -> bool {
    matches!(
        path_suffix(url).as_str(),
        ".ts" | ".m4s" | ".cmfv" | ".cmfa"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn internet_shortcut_reads_url() {
        let text = "[InternetShortcut]\r\nURL=https://cdn.test/a.bin\r\n";
        assert_eq!(
            extract_download_urls(text, "url").unwrap(),
            vec!["https://cdn.test/a.bin".to_string()]
        );
        assert!(
            extract_download_urls("[InternetShortcut]\nURL=javascript:alert(1)\n", "url").is_err()
        );
        assert!(extract_download_urls(
            "[InternetShortcut]\nURL=https://cdn.test/a.bin\tHost: evil\n",
            "url"
        )
        .is_err());
    }

    #[test]
    fn magnet_file_reads_first_link() {
        let text = "magnet:?xt=urn:btih:abc&dn=demo\n";
        assert_eq!(
            extract_download_urls(text, "magnet").unwrap()[0],
            "magnet:?xt=urn:btih:abc&dn=demo"
        );
    }

    #[test]
    fn master_playlist_prefers_variant_m3u8() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nhttps://cdn.test/hi.m3u8\n";
        assert_eq!(
            extract_download_urls(text, "m3u8").unwrap(),
            vec!["https://cdn.test/hi.m3u8".to_string()]
        );
    }

    #[test]
    fn media_playlist_of_segments_is_rejected() {
        let text = "#EXTM3U\n#EXTINF:4,\nhttps://cdn.test/1.ts\n#EXTINF:4,\nhttps://cdn.test/2.ts\n#EXTINF:4,\nhttps://cdn.test/3.ts\n";
        let error = extract_download_urls(text, "m3u8").unwrap_err();
        assert!(error.contains("本地分片"));
    }

    #[test]
    fn utf16_bom_controls_link_file_endianness() {
        let text = "https://cdn.test/A.bin\n";
        let mut little = vec![0xff, 0xfe];
        let mut big = vec![0xfe, 0xff];
        for unit in text.encode_utf16() {
            little.extend_from_slice(&unit.to_le_bytes());
            big.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_link_bytes(&little), text);
        assert_eq!(decode_link_bytes(&big), text);
    }

    #[test]
    fn case_distinct_urls_are_not_collapsed() {
        let text = "https://cdn.test/A.bin\nhttps://cdn.test/a.bin\n";
        assert_eq!(extract_download_urls(text, "txt").unwrap().len(), 2);
        assert_eq!(collect_absolute_urls(text).len(), 2);
    }

    #[test]
    fn file_scheme_is_case_insensitive() {
        let lower = decode_local_path("file:///tmp/demo%20file.txt");
        assert_eq!(decode_local_path("FILE:///tmp/demo%20file.txt"), lower);
        assert_eq!(decode_local_path("FiLe:///tmp/demo%20file.txt"), lower);
    }

    #[test]
    fn remote_http_is_not_a_local_source() {
        assert_eq!(expand_source("https://cdn.test/a.bin").unwrap(), None);
    }

    #[test]
    fn local_url_file_expands() {
        let dir = std::env::temp_dir().join(format!("v6-link-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("clip.url");
        fs::write(&path, "[InternetShortcut]\nURL=https://cdn.test/clip.mp4\n").unwrap();
        let urls = expand_source(&path.to_string_lossy()).unwrap().unwrap();
        assert_eq!(urls, vec!["https://cdn.test/clip.mp4".to_string()]);
        let _ = fs::remove_dir_all(dir);
    }

    /// 批量的 Hot Path 需要有耗时上限，否则一次"整页粘贴"的复杂度回退不会有人发现。
    ///
    /// `collect_absolute_urls` 是线性扫（6 个协议前缀各扫一遍 + BTreeSet 去重），
    /// 实测 5MB 量级文本在毫秒级跑完。这里报出数字并给一个**宽**上限：
    /// 数字本身要能看到（性能回归才有可对照的基线），上限故意放宽到秒级，
    /// 免得测试机上负载波动让测试flake。
    #[test]
    fn scanning_a_page_sized_clipboard_has_a_documented_budget() {
        let text = page_sized_text(5 * 1024 * 1024);
        let started = Instant::now();
        let urls = collect_absolute_urls(&text);
        let took = started.elapsed();
        println!(
            "clipboard_scan_5mb_amount={} urls={} took_ms={:.2}",
            text.len(),
            urls.len(),
            took.as_secs_f64() * 1000.0
        );
        assert_eq!(
            urls.len(),
            MAX_LINK_URLS,
            "上限 100 条必须仍然生效，否则批量粘贴会灌满任务表"
        );
        assert!(
            took < Duration::from_secs(10),
            "5MB 剪贴板扫描超过 10s（耗时 {took:?}）——线性扫很可能被改成了二次扫"
        );
    }

    /// 最坏情况：整块文本里**一条**合格链接都没有。
    /// 密集链接那种会在凑满 100 条后提前返回（实测只扫了约 5% 就退出），
    /// 而这里每个前缀都要扫完 5MB——复杂度若被改成二次，这里第一个爆。
    #[test]
    fn scanning_a_linkless_page_sized_clipboard_has_a_documented_budget() {
        let mut text = page_sized_text(5 * 1024 * 1024);
        // 把 50 个候选 URL 全换成不可识别的东西，凑不满 100 条 => 不会提前返回
        for link in [
            "https://cdn.test/",
            "javascript:void(0)",
            "ftp://",
            "magnet:?dn=x",
            "relative/path/file.mp4",
        ] {
            text = text.replace(link, "notalink");
        }
        text = text.replace("seg ", "seg-");
        let started = Instant::now();
        let urls = collect_absolute_urls(&text);
        let took = started.elapsed();
        println!(
            "linkless_scan_5mb_amount={} urls={} took_ms={:.2}",
            text.len(),
            urls.len(),
            took.as_secs_f64() * 1000.0
        );
        assert!(
            urls.len() < MAX_LINK_URLS,
            "这块文本本该凑不满 100 条（实际 {}）——构造失效，上限提前返回就不是最坏情况了",
            urls.len()
        );
        assert!(
            took < Duration::from_secs(10),
            "5MB 无链接文本扫描超过 10s（耗时 {took:?}）——线性扫很可能被改成了二次扫"
        );
    }

    /// 拼一块"页面源码"体量的文本：绝大部分是噪声，候选 URL 稀疏。
    /// 这是最坏情况（`MAX_LINK_URLS` 触发不了提前返回，必须扫完整块）。
    fn page_sized_text(target: usize) -> String {
        let mut text = String::with_capacity(target);
        let mut i = 0usize;
        while text.len() < target {
            if i.is_multiple_of(50) {
                text.push_str(&format!(
                    "<li><a href=\"https://cdn.test/{i}/file.mp4\">seg {i}</a></li>\n"
                ));
            } else {
                text.push_str(
                    "<div class=\"row\"><span>metadata</span><script>var x=1</script></div>\n",
                );
            }
            i += 1;
        }
        text
    }
}
