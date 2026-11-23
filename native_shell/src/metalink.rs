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
    let trimmed = text.trim().trim_start_matches('\u{feff}');
    if !trimmed.starts_with('<') {
        return false;
    }
    let head = trimmed
        .chars()
        .take(4000)
        .collect::<String>()
        .to_ascii_lowercase();
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
    if text.len() > 8 * 1024 * 1024 {
        return Err("metalink 文件过大".into());
    }
    let options = roxmltree::ParsingOptions {
        nodes_limit: 100_000,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(body.trim_start_matches('\u{feff}'), options)
        .map_err(|error| format!("metalink XML 无效: {error}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "metalink" {
        return Err("不是有效的 metalink 文件".into());
    }
    let same_tag = |n: roxmltree::Node<'_, '_>, tag: &str| {
        n.is_element()
            && n.tag_name().namespace() == root.tag_name().namespace()
            && n.tag_name().name() == tag
    };
    let files: Vec<_> = root
        .descendants()
        .filter(|n| {
            same_tag(*n, "file")
                && n.parent_element().is_some_and(|p| {
                    p == root || (same_tag(p, "files") && p.parent_element() == Some(root))
                })
        })
        .filter_map(parse_file_block)
        .take(100)
        .collect();
    if files.is_empty() {
        return Err("metalink 里没有可下载的远程地址".into());
    }
    Ok(files)
}

fn parse_file_block(file: roxmltree::Node<'_, '_>) -> Option<MetalinkFile> {
    let ns = file.tag_name().namespace();
    let matches = |n: roxmltree::Node<'_, '_>, tag: &str| {
        n.is_element() && n.tag_name().namespace() == ns && n.tag_name().name() == tag
    };
    let field = |tag: &str| {
        file.children()
            .find(|n| matches(*n, tag))
            .and_then(|n| n.text())
            .map(str::trim)
    };
    let name = sanitize(
        file.attribute("name")
            .or_else(|| field("name"))
            .unwrap_or("download"),
    );
    let size = field("size").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut ranked = Vec::new();
    let mut checksum = String::new();
    for node in file.descendants().filter(|n| n.is_element()) {
        let Some(parent) = node.parent_element() else {
            continue;
        };
        let Some(value) = node.text().map(str::trim) else {
            continue;
        };
        if matches(node, "url")
            && (parent == file
                || (matches(parent, "resources") && parent.parent_element() == Some(file)))
        {
            if let Some(url) = safe_url(value) {
                let rank = if let Some(preference) = node.attribute("preference") {
                    preference.parse::<i32>().unwrap_or(0).saturating_neg()
                } else {
                    node.attribute("priority")
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(100)
                };
                ranked.push((rank, url));
            }
        } else if checksum.is_empty()
            && matches(node, "hash")
            && (parent == file
                || (matches(parent, "verification") && parent.parent_element() == Some(file)))
        {
            let (algo, len) = match node
                .attribute("type")
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "sha-256" | "sha256" => ("sha256", 64),
                "sha-1" | "sha1" => ("sha1", 40),
                "md5" => ("md5", 32),
                _ => continue,
            };
            if value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit()) {
                checksum = format!("{algo}:{}", value.to_ascii_lowercase());
            }
        }
    }
    ranked.sort_by_key(|(rank, _)| *rank);
    let (url, mirrors) = pick_urls(ranked)?;
    Some(MetalinkFile {
        name,
        size,
        checksum,
        url,
        mirrors,
    })
}

fn pick_urls(ranked: Vec<(i32, String)>) -> Option<(String, Vec<String>)> {
    let mut ordered = Vec::new();
    for (_, url) in ranked {
        if ordered.iter().any(|item: &String| item == &url) {
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

fn safe_url(value: &str) -> Option<String> {
    if value.len() > 8192
        || value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return None;
    }
    let (scheme, rest) = value.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp"
    ) || authority.is_empty()
        || authority.contains('\\')
    {
        return None;
    }
    Some(value.to_string())
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
    } else if hls_client::request_validation::reserved_dos_device_name(&cleaned) {
        format!("_{cleaned}")
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
        assert_eq!(
            files[0].mirrors,
            vec!["https://mirror.example.test/demo.bin"]
        );
        assert!(files[0].checksum.starts_with("sha256:"));
        assert_eq!(files[0].size, 4);
    }

    #[test]
    fn preserves_case_distinct_resource_urls() {
        let files = parse_metalink(
            r#"<metalink><file name="case.bin"><url priority="1">https://cdn.test/File.bin</url><url priority="2">https://cdn.test/file.bin</url><url priority="3">https://cdn.test/File.bin</url></file></metalink>"#,
        )
        .unwrap();
        assert_eq!(files[0].url, "https://cdn.test/File.bin");
        assert_eq!(files[0].mirrors, vec!["https://cdn.test/file.bin"]);
    }

    #[test]
    fn decodes_xml_entities_in_signed_urls() {
        let files = parse_metalink(
            r#"<metalink><file name="signed.bin"><url priority="&#49;">https://cdn.test/file.bin?token=a&amp;part=1&#38;hex=&#x32;</url></file></metalink>"#,
        )
        .unwrap();
        assert_eq!(
            files[0].url,
            "https://cdn.test/file.bin?token=a&part=1&hex=2"
        );
    }

    #[test]
    fn parses_metalink3_preference_and_ftp() {
        let files = parse_metalink(META3).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
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
        assert!(!looks_like_metalink(
            "https://cdn.test/a.bin?<metalink><file><url>https://evil.test/x.bin</url></file></metalink>"
        ));
        assert!(looks_like_metalink(
            "<metalink>\n<file name=\"x\"><url>https://cdn.test/a.bin</url></file></metalink>"
        ));
    }

    #[test]
    fn handles_malicious_closing_tag_in_attributes_and_min_preference_without_panic() {
        let xml = r#"<metalink>
  <file name="edge.bin">
    <hash type="</hash>sha-256">9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08</hash>
    <url a="</url>">https://ignored.example.test/bad.bin</url>
    <url preference="-2147483648">https://cdn.example.test/edge.bin</url>
    <url preference="10">https://mirror.example.test/edge.bin</url>
  </file>
</metalink>"#;
        assert!(parse_metalink(xml).is_err());
        let valid = r#"<metalink><file name='dir/CON.txt'>
          <!-- <url>https://ignored.test/</url> -->
          <pieces><hash type='md5'>098f6bcd4621d373cade4e832627b4f6</hash></pieces>
          <url preference='-2147483648'>sftp://ignored.test/a</url>
          <url>https://cdn.test/a</url></file></metalink>"#;
        let file = parse_metalink(valid).unwrap().remove(0);
        assert_eq!(file.name, "_CON.txt");
        assert!(file.checksum.is_empty());
        assert!(file.mirrors.is_empty());
        assert_eq!(file.url, "https://cdn.test/a");
        assert!(safe_url("magnet:?xt=test").is_none());
    }
}
