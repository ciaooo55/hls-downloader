//! Metalink 3 / 4 import. Mirrors become TaskSpec.mirrors; no Python parser.

#[derive(Debug, Clone, PartialEq)]
pub struct MetalinkFile {
    pub name: String,
    pub url: String,
    pub mirrors: Vec<String>,
    pub checksum: String,
    pub size: u64,
}

pub fn looks_like_metalink(text: &str) -> bool {
    let head = text.chars().take(4000).collect::<String>().to_ascii_lowercase();
    head.contains("<metalink") && (head.contains("<file") || head.contains("<url"))
}

pub fn parse_metalink(text: &str) -> Result<Vec<MetalinkFile>, String> {
    let body = text.trim();
    if body.is_empty() {
        return Err("metalink 文件是空的".into());
    }
    if !looks_like_metalink(body) {
        return Err("不是有效的 metalink 文件".into());
    }
    let metalink3 = body.contains("metalinker.org") || body.contains("preference=");
    let mut files = Vec::new();
    for block in file_blocks(body) {
        let parsed = if metalink3 {
            parse_file_block(&block, true)
        } else {
            parse_file_block(&block, false)
        };
        if let Some(file) = parsed {
            files.push(file);
        }
        if files.len() >= 100 {
            break;
        }
    }
    if files.is_empty() {
        return Err("metalink 里没有可下载的远程地址".into());
    }
    Ok(files)
}

fn file_blocks(xml: &str) -> Vec<String> {
    let lower = xml.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut search = 0;
    while let Some(rel) = find_file_open(&lower[search..]) {
        let start = search + rel;
        let Some(end_rel) = lower[start..].find("</file>") else {
            break;
        };
        blocks.push(xml[start..start + end_rel + 7].to_string());
        search = start + end_rel + 7;
    }
    blocks
}

fn find_file_open(lower: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(rel) = lower[search..].find("<file") {
        let start = search + rel;
        let next = *lower.as_bytes().get(start + 5)?;
        if matches!(next, b' ' | b'>' | b'\n' | b'\t' | b'/' | b'\r') {
            return Some(start);
        }
        search = start + 5;
    }
    None
}

fn parse_file_block(block: &str, metalink3: bool) -> Option<MetalinkFile> {
    let name = attr(block, "name")
        .or_else(|| tag_text(block, "name"))
        .map(|value| sanitize(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "download".into());
    let size = tag_text(block, "size")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let checksum = first_checksum(block);
    let mut ranked = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.to_ascii_lowercase().find("<url") {
        let after = &rest[start..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let open = &after[..gt];
        let Some(close_rel) = after.to_ascii_lowercase().find("</url>") else {
            break;
        };
        let url = safe_url(after[gt + 1..close_rel].trim());
        rest = &after[close_rel + 6..];
        let Some(url) = url else {
            continue;
        };
        let rank = if metalink3 {
            -(attr(open, "preference")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0))
        } else {
            attr(open, "priority")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(100)
        };
        ranked.push((rank, url));
    }
    ranked.sort_by_key(|(rank, _)| *rank);
    let (url, mirrors) = pick_urls(ranked)?;
    Some(MetalinkFile {
        name,
        url,
        mirrors,
        checksum,
        size,
    })
}

fn pick_urls(ranked: Vec<(i32, String)>) -> Option<(String, Vec<String>)> {
    let mut ordered = Vec::new();
    for (_, url) in ranked {
        if ordered.iter().any(|item: &String| item.eq_ignore_ascii_case(&url)) {
            continue;
        }
        ordered.push(url);
        if ordered.len() >= 16 {
            break;
        }
    }
    let http: Vec<_> = ordered
        .iter()
        .filter(|url| {
            let lower = url.to_ascii_lowercase();
            lower.starts_with("http://") || lower.starts_with("https://")
        })
        .cloned()
        .collect();
    if let Some(primary) = http.first() {
        return Some((primary.clone(), http.into_iter().skip(1).collect()));
    }
    ordered.into_iter().next().map(|url| (url, Vec::new()))
}

fn first_checksum(block: &str) -> String {
    let mut rest = block;
    while let Some(start) = rest.to_ascii_lowercase().find("<hash") {
        let after = &rest[start..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let kind = attr(&after[..gt], "type").unwrap_or_default().to_ascii_lowercase();
        let Some(close) = after.to_ascii_lowercase().find("</hash>") else {
            break;
        };
        let digest = after[gt + 1..close].trim().to_ascii_lowercase();
        rest = &after[close + 7..];
        let algo = match kind.as_str() {
            "sha-256" | "sha256" => "sha256",
            "sha-1" | "sha1" => "sha1",
            "md5" => "md5",
            _ => continue,
        };
        if digest.chars().all(|ch| ch.is_ascii_hexdigit()) && !digest.is_empty() {
            return format!("{algo}:{digest}");
        }
    }
    String::new()
}

fn safe_url(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty()
        || value.len() > 8192
        || value.contains('\r')
        || value.contains('\n')
        || value.contains('\0')
    {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("javascript:")
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("file:")
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
        Some(value.to_string())
    } else {
        None
    }
}

fn attr(block: &str, key: &str) -> Option<String> {
    let pattern = format!("{key}=\"");
    let start = block.find(&pattern)?;
    let rest = &block[start + pattern.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn tag_text(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let lower = block.to_ascii_lowercase();
    let start = lower.find(&open.to_ascii_lowercase())? + open.len();
    let end = lower[start..].find(&close.to_ascii_lowercase())? + start;
    Some(block[start..end].trim().to_string())
}

fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|ch| {
            if ch.is_control()
                || matches!(
                    ch,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '&' | '%' | '^' | ';'
                )
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches([' ', '.']).to_string();
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const META4: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="demo.bin">
    <size>4</size>
    <hash type="sha-256">9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08</hash>
    <url location="de" priority="1">https://cdn.example.test/demo.bin</url>
    <url priority="2">https://mirror.example.test/demo.bin</url>
    <url priority="3">ftp://ftp.example.test/demo.bin</url>
    <url>javascript:alert(1)</url>
  </file>
</metalink>"#;

    const META3: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink version="3.0" xmlns="http://www.metalinker.org/">
  <files>
    <file name="pkg.zip">
      <size>8</size>
      <verification>
        <hash type="md5">098f6bcd4621d373cade4e832627b4f6</hash>
      </verification>
      <resources>
        <url type="http" preference="90">http://old.example.test/pkg.zip</url>
        <url type="http" preference="100">https://best.example.test/pkg.zip</url>
      </resources>
    </file>
    <file name="notes.txt">
      <resources>
        <url type="ftp">ftp://ftp.example.test/notes.txt</url>
      </resources>
    </file>
  </files>
</metalink>"#;

    #[test]
    fn parses_metalink4_http_primary_and_mirrors() {
        let files = parse_metalink(META4).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "demo.bin");
        assert_eq!(files[0].url, "https://cdn.example.test/demo.bin");
        assert_eq!(files[0].mirrors, vec!["https://mirror.example.test/demo.bin"]);
        assert!(files[0].checksum.starts_with("sha256:"));
        assert_eq!(files[0].size, 4);
    }

    #[test]
    fn parses_metalink3_preference_and_ftp() {
        let files = parse_metalink(META3).unwrap();
        assert_eq!(
            files.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(),
            vec!["pkg.zip", "notes.txt"]
        );
        assert_eq!(files[0].url, "https://best.example.test/pkg.zip");
        assert_eq!(files[0].mirrors, vec!["http://old.example.test/pkg.zip"]);
        assert!(files[0].checksum.starts_with("md5:"));
        assert_eq!(files[1].url, "ftp://ftp.example.test/notes.txt");
    }

    #[test]
    fn rejects_local_only_metalink() {
        assert!(parse_metalink(
            "<metalink><file name=\"x\"><url>file:///tmp/a.bin</url></file></metalink>"
        )
        .is_err());
    }
}
