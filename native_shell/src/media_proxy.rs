//! Credential-aware LAN playback. Only URLs registered by a fetched manifest
//! are addressable; browser credentials never enter the advertised URLs.

use crate::TaskSpec;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const MAX_ROUTES: usize = 8192;
const MAX_ROUTE_BYTES: usize = 4 * 1024 * 1024;
const MAX_REFERENCES: usize = 4096;

#[derive(Default)]
struct Routes {
    by_id: HashMap<u64, (String, u64)>,
    by_url: HashMap<String, u64>,
    access: BTreeMap<u64, u64>,
    clock: u64,
    next_id: u64,
    bytes: usize,
}

impl Routes {
    fn touch(&mut self, id: u64) {
        let (_, previous) = self.by_id.get_mut(&id).expect("registered route");
        self.access.remove(previous);
        self.clock += 1;
        *previous = self.clock;
        self.access.insert(self.clock, id);
    }

    fn register(&mut self, url: String) -> u64 {
        if let Some(&id) = self.by_url.get(&url) {
            self.touch(id);
            return id;
        }
        while self.by_id.len() >= MAX_ROUTES || self.bytes + url.len() > MAX_ROUTE_BYTES {
            if let Some((_, id)) = self.access.pop_first() {
                if let Some((url, _)) = self.by_id.remove(&id) {
                    self.bytes -= url.len();
                    self.by_url.remove(&url);
                }
            }
        }
        self.next_id += 1;
        self.bytes += url.len();
        let id = self.next_id;
        self.by_url.insert(url.clone(), id);
        self.by_id.insert(id, (url, 0));
        self.touch(id);
        id
    }

    fn get(&mut self, id: u64) -> Option<String> {
        let url = self.by_id.get(&id)?.0.clone();
        self.touch(id);
        Some(url)
    }
}

pub(crate) struct MediaProxy {
    spec: TaskSpec,
    replay: String,
    pub canceled: Arc<AtomicBool>,
    routes: Mutex<Routes>,
}

impl MediaProxy {
    pub fn new(spec: TaskSpec, replay: String) -> Result<Self, String> {
        if !spec.request_method.is_empty() && !spec.request_method.eq_ignore_ascii_case("GET") {
            return Err("此媒体使用非 GET 请求，请先下载后投屏".into());
        }
        if crate::http_engine::http_origin(&spec.url).is_none() {
            return Err("媒体地址无效".into());
        }
        Ok(Self {
            spec,
            replay,
            canceled: Arc::new(AtomicBool::new(false)),
            routes: Mutex::new(Routes::default()),
        })
    }

    pub fn cast_name(&self) -> &str {
        match manifest_kind(&self.spec.url, &self.spec.mime_type, b"") {
            Some("hls") => "index.m3u8",
            Some("dash") => "index.mpd",
            _ => {
                let path = self
                    .spec
                    .url
                    .split(['?', '#'])
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                match self.spec.mime_type.split(';').next().unwrap_or("").trim() {
                    "video/webm" => "media.webm",
                    "video/x-matroska" => "media.mkv",
                    "audio/mpeg" => "media.mp3",
                    "audio/mp4" => "media.m4a",
                    "audio/aac" => "media.aac",
                    _ if path.ends_with(".webm") => "media.webm",
                    _ if path.ends_with(".mkv") => "media.mkv",
                    _ if path.ends_with(".mp3") => "media.mp3",
                    _ if path.ends_with(".m4a") => "media.m4a",
                    _ if path.ends_with(".aac") => "media.aac",
                    _ => "media.mp4",
                }
            }
        }
    }

    fn route(&self, prefix: &str, url: String) -> Result<String, String> {
        if url.len() > 16 * 1024 || crate::http_engine::http_origin(&url).is_none() {
            return Err("清单包含无效媒体地址".into());
        }
        let variables = template_variables(&url)?;
        let id = self
            .routes
            .lock()
            .map_err(|_| "media routes poisoned")?
            .register(url);
        let mut local = format!("{prefix}/r/{id}");
        for name in variables {
            local.push_str(&format!("/${name}$"));
        }
        Ok(local)
    }

    fn source(&self, sub: &str) -> Result<String, String> {
        if sub.is_empty() || sub == self.cast_name() {
            return Ok(self.spec.url.clone());
        }
        let mut parts = sub.split('/');
        if parts.next() != Some("r") {
            return Err("media route not found".into());
        }
        let id = parts
            .next()
            .and_then(|id| id.parse::<u64>().ok())
            .ok_or("media route not found")?;
        let url = self
            .routes
            .lock()
            .map_err(|_| "media routes poisoned")?
            .get(id)
            .ok_or("media route not found")?;
        instantiate_template(&url, &parts.collect::<Vec<_>>())
    }

    pub fn serve(
        &self,
        stream: &mut TcpStream,
        request: &str,
        sub: &str,
        prefix: &str,
    ) -> Result<(), String> {
        let method = request
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().next())
            .unwrap_or("");
        if !matches!(method, "GET" | "HEAD") {
            return status(stream, 405);
        }
        if self.canceled.load(Ordering::Relaxed) {
            return status(stream, 404);
        }
        let Ok(url) = self.source(sub) else {
            return status(stream, 404);
        };
        let kind = manifest_kind(
            &url,
            if url == self.spec.url {
                &self.spec.mime_type
            } else {
                ""
            },
            b"",
        );
        let query = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|target| target.split_once('?'))
            .map(|(_, query)| query)
            .unwrap_or("");
        let Some(request_url) = hls_reload_url(&url, query, kind == Some("hls")) else {
            return status(stream, 400);
        };
        let range = request
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("range"))
            .map(|(_, value)| value.trim());
        if range.is_some_and(|value| {
            value.len() > 256
                || !value.starts_with("bytes=")
                || !value[6..]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b',' | b' '))
        }) {
            return status(stream, 416);
        }
        let mut spec = self.spec.clone();
        spec.url = url;
        let range = if kind.is_some() { None } else { range };
        let mut started = false;
        let result = crate::http_engine::with_media_response(
            &spec,
            &self.replay,
            &request_url,
            method,
            range,
            &self.canceled,
            stream.try_clone().map_err(|e| e.to_string())?,
            |mut response| {
                if !(200..300).contains(&response.status) {
                    started = true;
                    return status(
                        stream,
                        if (400..600).contains(&response.status) {
                            response.status
                        } else {
                            502
                        },
                    );
                }
                let mut first = Vec::new();
                if method != "HEAD" {
                    (&mut response)
                        .take(4096)
                        .read_to_end(&mut first)
                        .map_err(|e| e.to_string())?;
                }
                let kind = manifest_kind(&response.final_url, &response.content_type, &first);
                if let Some(kind) = kind {
                    if method == "HEAD" {
                        started = true;
                        return write_head(stream, 200, mime(kind), None, None, false);
                    }
                    if response.status != 200 {
                        return Err("清单未返回完整正文".into());
                    }
                    let limit = crate::media::MAX_MANIFEST_BYTES;
                    if response.content_length.is_some_and(|n| n > limit) {
                        return Err("投屏清单超过读取上限".into());
                    }
                    (&mut response)
                        .take(limit + 1 - first.len() as u64)
                        .read_to_end(&mut first)
                        .map_err(|e| e.to_string())?;
                    if first.len() as u64 > limit {
                        return Err("投屏清单超过读取上限".into());
                    }
                    if response
                        .content_length
                        .is_some_and(|n| n != first.len() as u64)
                    {
                        return Err("投屏清单正文不完整".into());
                    }
                    let text = std::str::from_utf8(&first).map_err(|_| "投屏清单不是 UTF-8")?;
                    let mut route_bytes = 0;
                    let mut register = |url: String| {
                        route_bytes += url.len();
                        if route_bytes > MAX_ROUTE_BYTES / 2 {
                            return Err("投屏清单地址总长度超限".into());
                        }
                        self.route(prefix, url)
                    };
                    let rewritten = if kind == "hls" {
                        rewrite_hls(text, &response.final_url, &mut register)?
                    } else {
                        rewrite_dash(text, &response.final_url, &mut register)?
                    };
                    started = true;
                    write_head(
                        stream,
                        200,
                        mime(kind),
                        Some(rewritten.len() as u64),
                        None,
                        false,
                    )?;
                    return stream
                        .write_all(rewritten.as_bytes())
                        .map_err(|e| e.to_string());
                }
                started = true;
                write_head(
                    stream,
                    response.status,
                    &response.content_type,
                    response.content_length,
                    response.content_range.as_deref(),
                    response.accept_ranges,
                )?;
                if method == "HEAD" {
                    return Ok(());
                }
                stream.write_all(&first).map_err(|e| e.to_string())?;
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let n = response.read(&mut buffer).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    stream.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
                }
                Ok(())
            },
        );
        if result.is_err() && !started {
            return status(stream, 502);
        }
        result
    }
}

fn hls_reload_url(source: &str, query: &str, known_hls: bool) -> Option<String> {
    let mut directives = BTreeMap::new();
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let valid = match name {
            "_HLS_msn" | "_HLS_part" => {
                !value.is_empty()
                    && value.bytes().all(|b| b.is_ascii_digit())
                    && value.parse::<u64>().is_ok()
            }
            "_HLS_skip" => matches!(value, "YES" | "v2"),
            _ => continue,
        };
        if !valid || directives.insert(name, value).is_some() {
            return None;
        }
    }
    if directives.contains_key("_HLS_part") && !directives.contains_key("_HLS_msn") {
        return None;
    }
    if directives.is_empty() && !known_hls {
        return Some(source.into());
    }
    let (raw, fragment) = source
        .split_once('#')
        .map_or((source, None), |(raw, tail)| (raw, Some(tail)));
    let (base, existing) = raw.split_once('?').unwrap_or((raw, ""));
    let is_directive = |pair: &str| {
        matches!(
            pair.split('=').next().unwrap_or(""),
            "_HLS_msn" | "_HLS_part" | "_HLS_skip"
        )
    };
    if directives.is_empty() && !existing.split('&').any(is_directive) {
        return Some(source.into());
    }
    // Keep signed query bytes and their order; only replace delivery directives.
    // A receiver cannot override source tokens, hosts or arbitrary parameters.
    let mut pairs: Vec<_> = if existing.is_empty() {
        Vec::new()
    } else {
        existing
            .split('&')
            .filter(|pair| !is_directive(pair))
            .map(str::to_string)
            .collect()
    };
    pairs.extend(
        directives
            .into_iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    let mut url = base.to_string();
    if !pairs.is_empty() {
        url.push('?');
        url.push_str(&pairs.join("&"));
    }
    if let Some(fragment) = fragment {
        url.push('#');
        url.push_str(fragment);
    }
    Some(url)
}

fn status(stream: &mut TcpStream, code: u16) -> Result<(), String> {
    write_head(stream, code, "text/plain", Some(0), None, false)
}

fn write_head(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    length: Option<u64>,
    range: Option<&str>,
    ranges: bool,
) -> Result<(), String> {
    let content_type = if content_type.is_empty() || content_type.chars().any(char::is_control) {
        "application/octet-stream"
    } else {
        content_type
    };
    let mut head = format!("HTTP/1.1 {code} Media\r\nConnection: close\r\nCache-Control: no-store\r\nContent-Type: {content_type}\r\n");
    if let Some(length) = length {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    if let Some(range) = range.filter(|v| !v.chars().any(char::is_control)) {
        head.push_str(&format!("Content-Range: {range}\r\n"));
    }
    if ranges {
        head.push_str("Accept-Ranges: bytes\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())
}

fn mime(kind: &str) -> &str {
    if kind == "hls" {
        "application/vnd.apple.mpegurl"
    } else {
        "application/dash+xml"
    }
}

fn manifest_kind(url: &str, content_type: &str, first: &[u8]) -> Option<&'static str> {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    let kind = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let text = std::str::from_utf8(first)
        .unwrap_or("")
        .trim_start_matches('\u{feff}')
        .trim_start();
    if path.ends_with(".m3u8")
        || path.ends_with(".m3u")
        || kind.contains("mpegurl")
        || text.starts_with("#EXTM3U")
    {
        Some("hls")
    } else if path.ends_with(".mpd")
        || kind == "application/dash+xml"
        || text.starts_with('<') && (text.contains("<MPD") || text.contains(":MPD"))
    {
        Some("dash")
    } else {
        None
    }
}

fn rewrite_hls(
    text: &str,
    base: &str,
    mut route: impl FnMut(String) -> Result<String, String>,
) -> Result<String, String> {
    if !text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("#EXTM3U")
    {
        return Err("HLS 清单无效".into());
    }
    let mut out = String::new();
    let mut count = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#EXT-X-CONTENT-STEERING:") {
            continue;
        }
        let mut edits = Vec::new();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            edits.push((0..line.len(), trimmed));
        } else if trimmed.starts_with('#') {
            // Covers keys, maps, alternate tracks and LL-HLS hints without dropping tags.
            let mut pos = 0;
            while let Some(i) = line[pos..].find("URI=\"") {
                let start = pos + i + 5;
                if start > 5 && !matches!(line.as_bytes()[start - 6], b':' | b',') {
                    pos = start;
                    continue;
                }
                let end = start + line[start..].find('"').ok_or("HLS URI 未闭合")?;
                edits.push((start..end, &line[start..end]));
                pos = end + 1;
            }
        }
        let mut from = 0;
        for (span, reference) in edits {
            count += 1;
            if count > MAX_REFERENCES || reference.contains("{$") {
                return Err("HLS 清单引用超限或包含未解析变量".into());
            }
            out.push_str(&line[from..span.start]);
            // Inline keys do not need credentials or a network request.
            if reference.starts_with("data:") {
                out.push_str(reference);
            } else {
                out.push_str(&route(crate::media::resolve_http_uri(base, reference))?);
            }
            from = span.end;
        }
        out.push_str(&line[from..]);
        out.push('\n');
    }
    Ok(out)
}

fn template_variables(url: &str) -> Result<Vec<String>, String> {
    let mut variables = Vec::new();
    let mut rest = url;
    while let Some(start) = rest.find('$') {
        rest = &rest[start + 1..];
        if let Some(next) = rest.strip_prefix('$') {
            rest = next;
            continue;
        }
        let end = rest.find('$').ok_or("DASH 模板未闭合")?;
        let token = &rest[..end];
        let name = token.split('%').next().unwrap_or(token);
        if !matches!(name, "Number" | "Time" | "SubNumber") {
            return Err("DASH 模板变量无效".into());
        }
        template_width(token)?;
        if !variables.iter().any(|item| item == name) {
            variables.push(name.to_string());
        }
        rest = &rest[end + 1..];
    }
    Ok(variables)
}

fn template_width(token: &str) -> Result<usize, String> {
    let Some((_, format)) = token.split_once('%') else {
        return Ok(0);
    };
    let width = format
        .strip_prefix('0')
        .and_then(|v| v.strip_suffix('d'))
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v <= 20)
        .ok_or("DASH 模板格式无效")?;
    Ok(width)
}

fn instantiate_template(url: &str, values: &[&str]) -> Result<String, String> {
    let variables = template_variables(url)?;
    if variables.len() != values.len() {
        return Err("DASH 模板参数不匹配".into());
    }
    let mut numbers = HashMap::new();
    for (name, value) in variables.iter().zip(values) {
        if value.is_empty() || value.len() > 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err("DASH 模板参数无效".into());
        }
        numbers.insert(
            name.as_str(),
            value.parse::<u64>().map_err(|_| "DASH 模板参数超限")?,
        );
    }
    expand_template(url, |token| {
        let name = token.split('%').next().unwrap_or(token);
        let number = numbers.get(name).ok_or("DASH 模板变量缺失")?;
        Ok(format!("{number:0width$}", width = template_width(token)?))
    })
    .map(|url| url.replace("$$", "$"))
}

fn expand_template(
    text: &str,
    mut replace: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        if let Some(next) = rest.strip_prefix('$') {
            out.push_str("$$");
            rest = next;
            continue;
        }
        let end = rest.find('$').ok_or("DASH 模板未闭合")?;
        out.push_str(&replace(&rest[..end])?);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
        .replace('>', "&gt;")
}

fn rewrite_dash(
    text: &str,
    base: &str,
    mut route: impl FnMut(String) -> Result<String, String>,
) -> Result<String, String> {
    let options = roxmltree::ParsingOptions {
        nodes_limit: 100_000,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, options)
        .map_err(|_| "DASH XML 无效或节点超限")?;
    let root = doc.root_element();
    if root.tag_name().name() != "MPD" {
        return Err("DASH 根节点无效".into());
    }
    if root.descendants().any(|node| {
        node.attributes()
            .any(|a| a.namespace() == Some("http://www.w3.org/1999/xlink") && a.name() == "href")
    }) {
        return Err("暂不支持引用外部 XML 的投屏清单".into());
    }
    let namespace = root.tag_name().namespace();
    let is = |node: roxmltree::Node<'_, '_>, name: &str| {
        node.is_element()
            && node.tag_name().namespace() == namespace
            && node.tag_name().name() == name
    };
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut count = 0;
    // Flatten inherited addressing per representation before rewriting URLs. A
    // parent's template may use a different child BaseURL for every rendition.
    for representation in root
        .descendants()
        .filter(|node| is(*node, "Representation"))
    {
        let mut ancestors: Vec<_> = representation
            .ancestors()
            .filter(|n| n.is_element())
            .collect();
        ancestors.reverse();
        let mut effective_base = base.to_string();
        let mut addressing = Vec::new();
        for ancestor in &ancestors {
            if let Some(xml_base) =
                ancestor.attribute(("http://www.w3.org/XML/1998/namespace", "base"))
            {
                effective_base = crate::media::resolve_http_uri(&effective_base, xml_base);
                if effective_base.is_empty() {
                    return Err("DASH xml:base 无效".into());
                }
            }
            if let Some(node) = ancestor.children().find(|n| is(*n, "BaseURL")) {
                if let Some(xml_base) =
                    node.attribute(("http://www.w3.org/XML/1998/namespace", "base"))
                {
                    effective_base = crate::media::resolve_http_uri(&effective_base, xml_base);
                }
                effective_base =
                    crate::media::resolve_http_uri(&effective_base, node.text().unwrap_or(""));
                if effective_base.is_empty() {
                    return Err("DASH BaseURL 无效".into());
                }
            }
            if let Some(node) = ancestor.children().find(|n| {
                ["SegmentTemplate", "SegmentList", "SegmentBase"]
                    .iter()
                    .any(|name| is(*n, name))
            }) {
                if addressing
                    .last()
                    .is_some_and(|old: &roxmltree::Node<'_, '_>| {
                        old.tag_name().name() != node.tag_name().name()
                    })
                {
                    addressing.clear();
                }
                addressing.push(node);
            }
        }
        let prefix = representation
            .lookup_prefix(namespace.unwrap_or(""))
            .map(|p| format!("{p}:"))
            .unwrap_or_default();
        let mut appended = String::new();
        if let Some(nearest) = addressing.last() {
            let name = nearest.tag_name().name();
            let mut attributes = BTreeMap::new();
            let mut children = BTreeMap::new();
            for node in &addressing {
                for attribute in node.attributes() {
                    if attribute.namespace().is_none() {
                        attributes.insert(attribute.name(), attribute.value());
                    }
                }
                for child in node.children().filter(|n| n.is_element()) {
                    // SegmentURL is an ordered list; inherit the whole list from
                    // the nearest SegmentList that actually defines one.
                    let key = child.tag_name().name();
                    children.entry(key).or_insert_with(Vec::new);
                }
                for key in children.keys().copied().collect::<Vec<_>>() {
                    let found: Vec<_> = node
                        .children()
                        .filter(|n| n.is_element() && n.tag_name().name() == key)
                        .collect();
                    if !found.is_empty() {
                        children.insert(key, found);
                    }
                }
            }
            let mut rewrite = |reference: &str| -> Result<String, String> {
                count += 1;
                if count > MAX_REFERENCES {
                    return Err("DASH 清单引用超限".into());
                }
                let reference = expand_template(reference, |token| {
                    if token == "RepresentationID" {
                        return representation
                            .attribute("id")
                            .map(str::to_string)
                            .ok_or("DASH RepresentationID 缺失".into());
                    }
                    if token.split('%').next() == Some("Bandwidth") {
                        let value = representation
                            .attribute("bandwidth")
                            .and_then(|v| v.parse::<u64>().ok())
                            .ok_or("DASH Bandwidth 缺失")?;
                        return Ok(format!("{value:0width$}", width = template_width(token)?));
                    }
                    Ok(format!("${token}$"))
                })?;
                route(crate::media::resolve_http_uri(&effective_base, &reference))
            };
            appended.push_str(&format!(
                "<{prefix}BaseURL>{}</{prefix}BaseURL>",
                xml_escape(&rewrite(&effective_base)?)
            ));
            appended.push_str(&format!("<{prefix}{name}"));
            for (name, value) in attributes {
                let value = if matches!(name, "media" | "initialization" | "index") {
                    rewrite(value)?
                } else {
                    value.to_string()
                };
                appended.push_str(&format!(" {name}=\"{}\"", xml_escape(&value)));
            }
            appended.push('>');
            for nodes in children.values() {
                for node in nodes {
                    let raw = &text[node.range()];
                    let mut local_edits = Vec::new();
                    for descendant in node.descendants().filter(|n| n.is_element()) {
                        for attribute in descendant
                            .attributes()
                            .filter(|a| matches!(a.name(), "sourceURL" | "media" | "index"))
                        {
                            let span = attribute.range_value();
                            local_edits.push((
                                span.start - node.range().start..span.end - node.range().start,
                                xml_escape(&rewrite(attribute.value())?),
                            ));
                        }
                    }
                    appended.push_str(&apply_edits(raw, local_edits)?);
                }
            }
            appended.push_str(&format!("</{prefix}{name}>"));
        } else {
            count += 1;
            if count > MAX_REFERENCES {
                return Err("DASH 清单引用超限".into());
            }
            appended.push_str(&format!(
                "<{prefix}BaseURL>{}</{prefix}BaseURL>",
                xml_escape(&route(effective_base)?)
            ));
        }
        let span = representation.range();
        let raw = &text[span.clone()];
        if raw.ends_with("/>") {
            edits.push((
                span.end - 2..span.end,
                format!(">{appended}</{prefix}Representation>"),
            ));
        } else {
            let closing = span.start + raw.rfind("</").ok_or("DASH Representation 未闭合")?;
            edits.push((closing..closing, appended));
        }
    }
    for node in root.descendants().filter(|node| node.is_element()) {
        let name = node.tag_name().name();
        if node.tag_name().namespace() != namespace {
            continue;
        }
        if let Some(attribute) =
            node.attribute_node(("http://www.w3.org/XML/1998/namespace", "base"))
        {
            if matches!(name, "MPD" | "Period" | "AdaptationSet" | "Representation") {
                edits.push((attribute.range(), String::new()));
            } else if name != "BaseURL" {
                return Err("暂不支持此节点的 DASH xml:base".into());
            }
        }
        if matches!(
            name,
            "BaseURL" | "SegmentTemplate" | "SegmentList" | "SegmentBase"
        ) && node.parent().is_some_and(|p| {
            matches!(
                p.tag_name().name(),
                "MPD" | "Period" | "AdaptationSet" | "Representation"
            )
        }) {
            edits.push((node.range(), String::new()));
        }
        // Reloads stay on the local mount. Timing metadata may still be read
        // from its explicitly registered URL; no open upstream URL parameter.
        if matches!(name, "Location" | "PatchLocation") {
            edits.push((node.range(), String::new()));
        }
        if name == "UTCTiming" {
            if let Some(attr) = node.attribute_node("value").filter(|_| {
                node.attribute("schemeIdUri")
                    .is_some_and(|s| s.contains(":http-"))
            }) {
                edits.push((
                    attr.range_value(),
                    xml_escape(&route(crate::media::resolve_http_uri(base, attr.value()))?),
                ));
            }
        }
    }
    apply_edits(text, edits)
}

fn apply_edits(
    text: &str,
    mut edits: Vec<(std::ops::Range<usize>, String)>,
) -> Result<String, String> {
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut out = String::new();
    let mut from = 0;
    for (span, replacement) in edits {
        if span.start < from {
            return Err("DASH 清单节点重叠".into());
        }
        out.push_str(&text[from..span.start]);
        out.push_str(&replacement);
        from = span.end;
    }
    out.push_str(&text[from..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::MediaServer;
    use serde_json::json;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    struct Upstream {
        address: std::net::SocketAddr,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    fn read_headers(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            if stream.read(&mut byte).unwrap_or(0) == 0 {
                break;
            }
            head.push(byte[0]);
            assert!(head.len() < 64 * 1024);
        }
        String::from_utf8(head).unwrap()
    }

    impl Upstream {
        fn start(handler: impl Fn(&str) -> String + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (thread_requests, thread_stop) = (Arc::clone(&requests), Arc::clone(&stop));
            let worker = thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let request = read_headers(&mut stream);
                    thread_requests.lock().unwrap().push(request.clone());
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let _ = stream.write_all(handler(&request).as_bytes());
                }
            });
            Self {
                address,
                requests,
                stop,
                worker: Some(worker),
            }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}{path}", self.address)
        }
    }

    impl Drop for Upstream {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(self.address);
            self.worker.take().unwrap().join().unwrap();
        }
    }

    fn reply(kind: &str, body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    fn request(server: &MediaServer, method: &str, path: &str, headers: &str) -> (String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        stream
            .write_all(
                format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n").as_bytes(),
            )
            .unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let result = String::from_utf8(bytes).unwrap();
        let (head, body) = result.split_once("\r\n\r\n").unwrap();
        (head.to_string(), body.to_string())
    }

    fn mount(server: &MediaServer, url: &str, context: serde_json::Value) {
        let replay = crate::credentials::bind_replay_source_url(&context.to_string(), url);
        let mut spec = TaskSpec {
            url: url.into(),
            ..Default::default()
        };
        crate::apply_replay_json_for(&mut spec.headers, &replay, url);
        server.mount_proxy("test", spec, replay).unwrap();
    }

    #[test]
    fn hls_proxy_keeps_credentials_on_master_children_keys_and_byte_ranges() {
        let keys = Upstream::start(|_| reply("application/octet-stream", "private-key"));
        let key_url = keys.url("/keys/k");
        let source = Upstream::start(move |req| {
            if req.starts_with("GET /master.m3u8") {
                reply("application/vnd.apple.mpegurl", "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=100,AUDIO=\"a\"\nvideo.m3u8\n")
            } else if req.starts_with("GET /video.m3u8") {
                reply("application/vnd.apple.mpegurl", &format!("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"{key_url}\"\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:1,\n#EXT-X-BYTERANGE:4@0\npart.mp4\n#EXTINF:1,\n#EXT-X-BYTERANGE:4\npart.mp4\n#EXT-X-ENDLIST\n"))
            } else if req.starts_with("GET /audio.m3u8") {
                reply(
                    "application/vnd.apple.mpegurl",
                    "#EXTM3U\n#EXTINF:1,\naudio.aac\n#EXT-X-ENDLIST\n",
                )
            } else {
                reply("video/mp4", "fragment")
            }
        });
        let server = MediaServer::start_local().unwrap();
        let url = source.url("/master.m3u8?token=signed");
        let source_origin = crate::credentials::request_origin(&url);
        let keys_origin = crate::credentials::request_origin(&keys.url("/keys/k"));
        mount(
            &server,
            &url,
            json!({
                "cookie":"session=source", "referer":"https://page.test/player", "user_agent":"Browser-Cast-Test",
                "request_headers":{"Authorization":"Bearer master", "If-None-Match":"stale"},
                "request_contexts":{
                    source_origin:{"cookie":"session=source", "referer":"https://page.test/player", "url_contexts":{
                        source.url("/video.m3u8?token=signed"):{"referer":"https://page.test/player", "request_headers":{"Authorization":"Bearer video"}},
                        source.url("/audio.m3u8?token=signed"):{"referer":"https://page.test/player", "request_headers":{"Authorization":"Bearer audio"}}
                    }},
                    keys_origin:{"cookie":"session=key", "request_headers":{"Authorization":"Bearer key"}}
                }
            }),
        );
        let (head, master) = request(&server, "GET", "/media/test", "Cookie: injected=1\r\n");
        assert!(head.starts_with("HTTP/1.1 200"));
        assert!(
            !master.contains("signed")
                && !master.contains("Bearer")
                && !master.contains("127.0.0.1")
        );
        let video = master
            .lines()
            .find(|line| line.starts_with("/media/"))
            .unwrap();
        let (_, media) = request(&server, "GET", video, "");
        let uris: Vec<_> = media
            .lines()
            .filter_map(|line| {
                line.split_once("URI=\"")
                    .map(|(_, value)| value.split('"').next().unwrap())
            })
            .collect();
        assert_eq!(request(&server, "GET", uris[0], "").1, "private-key");
        assert_eq!(request(&server, "GET", uris[1], "").1, "fragment");
        let parts: Vec<_> = media
            .lines()
            .filter(|line| line.starts_with("/media/"))
            .collect();
        assert_eq!(parts[0], parts[1], "implicit byte ranges need a stable URL");
        assert!(media.contains("#EXT-X-BYTERANGE:4@0\n") && media.contains("#EXT-X-BYTERANGE:4\n"));
        let audio = master
            .split_once("URI=\"")
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap();
        assert!(request(&server, "GET", audio, "").1.contains("#EXTM3U"));
        let logs = source.requests.lock().unwrap();
        assert!(
            logs[0].contains("Cookie: session=source")
                && logs[0].contains("Authorization: Bearer master")
        );
        assert!(logs[1].contains("Authorization: Bearer video"));
        assert!(logs.last().unwrap().contains("Authorization: Bearer audio"));
        assert!(logs
            .iter()
            .all(|r| !r.contains("injected") && !r.to_ascii_lowercase().contains("if-none-match")));
        assert!(logs
            .iter()
            .all(|r| r.contains("Referer: https://page.test/player")
                && r.contains("User-Agent: Browser-Cast-Test")));
        let log = &keys.requests.lock().unwrap()[0];
        assert!(log.contains("Cookie: session=key") && log.contains("Authorization: Bearer key"));
        assert!(!log.contains("session=source") && !log.contains("Bearer master"));
    }

    #[test]
    fn ll_hls_proxy_preserves_reload_directives_and_exact_request_identities() {
        let source = Upstream::start(|req| {
            if req.starts_with("GET /master.m3u8") {
                reply("application/vnd.apple.mpegurl", "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",URI=\"live?track=audio&token=audio\"\n#EXT-X-STREAM-INF:BANDWIDTH=100,AUDIO=\"a\"\nlive?track=video&token=a%2Fb+z\n")
            } else if req.starts_with("GET /live?") {
                let skip = if req.contains("_HLS_skip=v2") {
                    "#EXT-X-SKIP:SKIPPED-SEGMENTS=2\n"
                } else {
                    ""
                };
                reply("application/vnd.apple.mpegurl", &format!("#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:10\n#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,CAN-SKIP-UNTIL=24\n#EXT-X-PART-INF:PART-TARGET=0.5\n{skip}#EXT-X-PART:DURATION=0.5,URI=\"part.m4s\"\n#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"next.m4s\"\n"))
            } else {
                reply("video/mp4", "live-fragment")
            }
        });
        let server = MediaServer::start_local().unwrap();
        let root = source.url("/master.m3u8?token=root&_HLS_msn=7");
        let video = source.url("/live?track=video&token=a%2Fb+z");
        let audio = source.url("/live?track=audio&token=audio");
        let origin = crate::credentials::request_origin(&root);
        mount(
            &server,
            &root,
            json!({
                "request_headers":{"Authorization":"Bearer root"},
                "request_contexts":{origin:{"cookie":"session=private", "request_headers":{"Authorization":"Bearer wrong"}, "url_contexts":{
                    root.clone():{"request_headers":{"Authorization":"Bearer master"}},
                    video:{"request_headers":{"Authorization":"Bearer video"}},
                    audio:{"request_headers":{"Authorization":"Bearer audio"}}
                }}}
            }),
        );
        let (head, master) = request(&server, "GET", "/media/test/index.m3u8", "");
        assert!(head.starts_with("HTTP/1.1 200"));
        assert!(!master.contains("token=") && !master.contains("Bearer"));
        let video_route = master
            .lines()
            .find(|line| line.starts_with("/media/"))
            .unwrap();
        let audio_route = master
            .split_once("URI=\"")
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap();
        let (_, full) = request(&server, "GET", video_route, "");
        let (_, delta) = request(&server, "GET", &format!("{video_route}?token=forged&url=http://outside.invalid/x&_HLS_skip=v2&_HLS_part=2&_HLS_msn=12"), "Cookie: forged=1\r\nAuthorization: Bearer forged\r\n");
        assert!(delta.contains("#EXT-X-SKIP:SKIPPED-SEGMENTS=2"));
        assert!(delta.contains("#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES"));
        let part = |text: &str| {
            text.split_once("URI=\"")
                .unwrap()
                .1
                .split('"')
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            part(&full),
            part(&delta),
            "reloads must retain stable fragment routes"
        );
        assert_eq!(
            request(&server, "GET", &part(&delta), "").1,
            "live-fragment"
        );
        assert!(request(
            &server,
            "GET",
            &format!("{audio_route}?_HLS_msn=12&_HLS_skip=YES"),
            ""
        )
        .1
        .contains("#EXTM3U"));
        let logs = source.requests.lock().unwrap();
        assert!(logs[0].starts_with("GET /master.m3u8?token=root HTTP/1.1\r\n"));
        assert!(logs[0].contains("Authorization: Bearer master"));
        assert!(logs[2].starts_with(
            "GET /live?track=video&token=a%2Fb+z&_HLS_msn=12&_HLS_part=2&_HLS_skip=v2 HTTP/1.1\r\n"
        ));
        assert!(
            logs[1].contains("Authorization: Bearer video")
                && logs[2].contains("Authorization: Bearer video")
        );
        assert!(logs[3].starts_with("GET /part.m4s?token=a%2Fb+z HTTP/1.1\r\n"));
        assert!(logs[4].starts_with(
            "GET /live?track=audio&token=audio&_HLS_msn=12&_HLS_skip=YES HTTP/1.1\r\n"
        ));
        assert!(logs[4].contains("Authorization: Bearer audio"));
        assert_eq!(logs.len(), 5);
        assert!(logs
            .iter()
            .all(|req| req.contains("Cookie: session=private")
                && !req.contains("forged")
                && !req.contains("outside.invalid")));
    }

    #[test]
    fn ll_hls_proxy_rejects_invalid_directives_before_contacting_upstream() {
        let source = Upstream::start(|_| reply("application/vnd.apple.mpegurl", "#EXTM3U\n"));
        let server = MediaServer::start_local().unwrap();
        mount(&server, &source.url("/live.m3u8?token=private"), json!({}));
        for query in [
            "_HLS_part=1",
            "_HLS_msn=-1",
            "_HLS_msn=18446744073709551616",
            "_HLS_msn=1&_HLS_msn=2",
            "_HLS_msn=1&_HLS_part=bad",
            "_HLS_skip=NO",
        ] {
            let (head, _) = request(
                &server,
                "GET",
                &format!("/media/test/index.m3u8?{query}"),
                "",
            );
            assert!(head.starts_with("HTTP/1.1 400"), "{query}: {head}");
        }
        assert!(source.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn ll_hls_proxy_preserves_upstream_reload_error_status_without_private_body() {
        let source = Upstream::start(|req| {
            let status = if req.contains("_HLS_msn=999") {
                400
            } else {
                503
            };
            format!(
                "HTTP/1.1 {status} Reload\r\nContent-Length: 7\r\nConnection: close\r\n\r\nprivate"
            )
        });
        let server = MediaServer::start_local().unwrap();
        mount(&server, &source.url("/live.m3u8"), json!({}));
        for (msn, status) in [(999, 400), (1000, 503)] {
            let (head, body) = request(
                &server,
                "GET",
                &format!("/media/test/index.m3u8?_HLS_msn={msn}"),
                "",
            );
            assert!(head.starts_with(&format!("HTTP/1.1 {status}")), "{head}");
            assert!(body.is_empty());
        }
        assert_eq!(source.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn media_proxy_forwards_range_and_head_without_body_or_redirect() {
        let upstream = Upstream::start(|req| {
            assert!(req.contains("Range: bytes=3-6"));
            let body = if req.starts_with("HEAD ") { "" } else { "3456" };
            format!("HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nContent-Range: bytes 3-6/10\r\nContent-Length: 4\r\nAccept-Ranges: bytes\r\n\r\n{body}")
        });
        let server = MediaServer::start_local().unwrap();
        mount(
            &server,
            &upstream.url("/video.mp4"),
            json!({"cookie":"sid=secret"}),
        );
        for method in ["GET", "HEAD"] {
            let (head, body) = request(&server, method, "/media/test", "Range: bytes=3-6\r\n");
            assert!(
                head.starts_with("HTTP/1.1 206") && head.contains("Content-Range: bytes 3-6/10")
            );
            assert!(
                head.contains("Content-Length: 4")
                    && !head.contains("secret")
                    && !head.contains("Location:")
            );
            assert_eq!(body, if method == "HEAD" { "" } else { "3456" });
        }
        let (head, _) = request(&server, "POST", "/media/test", "");
        assert!(head.starts_with("HTTP/1.1 405"));
        let (head, _) = request(&server, "GET", "/media/test/r/999/http://evil.test", "");
        assert!(head.starts_with("HTTP/1.1 404"));
        assert_eq!(upstream.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn media_proxy_redirect_uses_final_manifest_base_and_scoped_credentials() {
        let target = Upstream::start(|req| {
            if req.starts_with("GET /final/part.ts ") {
                reply("video/mp2t", "redirect-fragment")
            } else {
                reply(
                    "application/vnd.apple.mpegurl",
                    "#EXTM3U\n#EXTINF:1,\npart.ts\n#EXT-X-ENDLIST\n",
                )
            }
        });
        let target_url = target.url("/final/index.m3u8");
        let source = Upstream::start(move |_| {
            format!("HTTP/1.1 302 Found\r\nLocation: {target_url}\r\nContent-Length: 0\r\n\r\n")
        });
        let server = MediaServer::start_local().unwrap();
        mount(
            &server,
            &source.url("/entry"),
            json!({"cookie":"source=private", "request_headers":{"Authorization":"Bearer private"}}),
        );
        let (_, text) = request(&server, "GET", "/media/test", "");
        assert!(text.contains("/media/test/r/"));
        let part = text
            .lines()
            .find(|line| line.starts_with("/media/"))
            .unwrap();
        assert_eq!(request(&server, "GET", part, "").1, "redirect-fragment");
        let log = &target.requests.lock().unwrap()[0];
        assert!(
            !log.to_ascii_lowercase().contains("cookie:")
                && !log.to_ascii_lowercase().contains("authorization:")
        );
    }

    #[test]
    fn dash_proxy_flattens_inherited_templates_and_preserves_timeline_and_ranges() {
        let text = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011"><BaseURL>https://cdn.test/root/</BaseURL><Period><BaseURL>period/</BaseURL><AdaptationSet><SegmentTemplate timescale="1000" media="$RepresentationID$/$Number%05d$-$Time$.m4s" initialization="$RepresentationID$/init.mp4"><SegmentTimeline><S t="30" d="1000" r="2"/></SegmentTimeline></SegmentTemplate><Representation id="v" bandwidth="900"><BaseURL>video/</BaseURL><SegmentTemplate startNumber="7"/></Representation><Representation id="a" bandwidth="100"><BaseURL>audio/</BaseURL></Representation></AdaptationSet><AdaptationSet><SegmentList duration="2"><Initialization sourceURL="init.mp4" range="0-9"/><SegmentURL media="part.mp4" mediaRange="10-19"/><SegmentURL media="part.mp4" mediaRange="20-29"/></SegmentList><Representation id="l"><BaseURL>list/</BaseURL></Representation></AdaptationSet><AdaptationSet><Representation id="single"><BaseURL>movie.mp4</BaseURL><SegmentBase indexRange="100-199"><Initialization range="0-99"/></SegmentBase></Representation></AdaptationSet></Period></MPD>"#;
        let proxy = MediaProxy::new(
            TaskSpec {
                url: "https://entry.test/main.mpd".into(),
                ..Default::default()
            },
            String::new(),
        )
        .unwrap();
        let rewritten = rewrite_dash(text, "https://entry.test/main.mpd", |url| {
            proxy.route("/media/test", url)
        })
        .unwrap();
        assert!(!rewritten.contains("cdn.test") && !rewritten.contains("entry.test"));
        let doc = roxmltree::Document::parse(&rewritten).unwrap();
        for (id, directory) in [("v", "video"), ("a", "audio")] {
            let rep = doc
                .descendants()
                .find(|n| n.attribute("id") == Some(id))
                .unwrap();
            let template = rep
                .children()
                .find(|n| n.has_tag_name(("urn:mpeg:dash:schema:mpd:2011", "SegmentTemplate")))
                .unwrap();
            assert_eq!(template.attribute("timescale"), Some("1000"));
            assert!(template
                .descendants()
                .any(|n| n.attribute("t") == Some("30")));
            let local = template
                .attribute("media")
                .unwrap()
                .replace("$Number$", "7")
                .replace("$Time$", "30");
            assert_eq!(
                proxy
                    .source(local.strip_prefix("/media/test/").unwrap())
                    .unwrap(),
                format!("https://cdn.test/root/period/{directory}/{id}/00007-30.m4s")
            );
        }
        assert!(
            rewritten.contains("mediaRange=\"10-19\"")
                && rewritten.contains("mediaRange=\"20-29\"")
        );
        assert!(
            rewritten.contains("indexRange=\"100-199\"") && rewritten.contains("range=\"0-99\"")
        );
        assert!(proxy.source("r/1/../../http://other.test").is_err());
        assert!(rewrite_dash(r#"<MPD xmlns:xlink="http://www.w3.org/1999/xlink"><Period xlink:href="https://other.test"/></MPD>"#, "https://cdn.test/a.mpd", Ok).is_err());
    }

    #[test]
    fn media_proxy_streams_before_upstream_finishes_and_unmount_interrupts_a_stall() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, hold) = mpsc::channel();
        let (ready, waiting) = mpsc::channel();
        let upstream = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_headers(&mut stream);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 1000000\r\n\r\n").unwrap();
            stream.write_all(&[b'x'; 4096]).unwrap();
            ready.send(()).unwrap();
            hold.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let server = MediaServer::start_local().unwrap();
        mount(&server, &format!("http://{address}/movie.mp4"), json!({}));
        let mut client = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .write_all(b"GET /media/test HTTP/1.1\r\n\r\n")
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(read_headers(&mut client).starts_with("HTTP/1.1 200"));
        let mut first = [0; 4096];
        client.read_exact(&mut first).unwrap();
        assert_eq!(first, [b'x'; 4096]);
        assert!(server.unmount("test"));
        let mut remainder = Vec::new();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.read_to_end(&mut remainder).unwrap();
        assert!(remainder.is_empty());
        release.send(()).unwrap();
        upstream.join().unwrap();
        assert!(request(&server, "GET", "/media/test", "")
            .0
            .starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn media_proxy_releases_stalled_upstream_when_receiver_disconnects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (closed, observed) = mpsc::channel();
        let upstream = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_headers(&mut stream);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nContent-Type: video/mp4\r\n\r\n").unwrap();
            stream.write_all(&[b'x'; 4096]).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            closed
                .send(matches!(stream.read(&mut [0u8; 1]), Ok(0)))
                .unwrap();
        });
        let server = MediaServer::start_local().unwrap();
        mount(&server, &format!("http://{address}/movie.mp4"), json!({}));
        let mut receiver = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        receiver
            .write_all(b"GET /media/test HTTP/1.1\r\n\r\n")
            .unwrap();
        assert!(read_headers(&mut receiver).starts_with("HTTP/1.1 200"));
        let mut bytes = [0; 4096];
        receiver.read_exact(&mut bytes).unwrap();
        drop(receiver);
        assert!(observed.recv_timeout(Duration::from_secs(3)).unwrap());
        upstream.join().unwrap();
    }

    #[test]
    fn extensionless_dash_uses_browser_mime_and_proxies_expanded_segment_requests() {
        let upstream = Upstream::start(|request| {
            if request.starts_with("GET /manifest ") {
                reply(
                    "application/dash+xml",
                    r#"<MPD><Period><AdaptationSet><SegmentTemplate duration="1" media="seg-$Number%03d$.m4s" initialization="init.mp4"/><Representation id="video" bandwidth="100"/></AdaptationSet></Period></MPD>"#,
                )
            } else {
                reply("video/mp4", "dash-fragment")
            }
        });
        let server = MediaServer::start_local().unwrap();
        let url = upstream.url("/manifest");
        let replay = crate::credentials::bind_replay_source_url(
            &json!({"cookie":"dash=private"}).to_string(),
            &url,
        );
        server
            .mount_proxy(
                "dash",
                TaskSpec {
                    url,
                    mime_type: "application/dash+xml".into(),
                    ..Default::default()
                },
                replay,
            )
            .unwrap();
        assert!(server
            .cast_url_for("dash", "127.0.0.1")
            .ends_with("/index.mpd"));
        let (_, mpd) = request(&server, "GET", "/media/dash/index.mpd", "");
        let doc = roxmltree::Document::parse(&mpd).unwrap();
        let template = doc
            .descendants()
            .find(|node| node.has_tag_name("SegmentTemplate"))
            .unwrap();
        let local = template
            .attribute("media")
            .unwrap()
            .replace("$Number$", "9");
        assert_eq!(
            request(&server, "GET", &local, "Range: bytes=0-12\r\n").1,
            "dash-fragment"
        );
        let logs = upstream.requests.lock().unwrap();
        assert!(logs[1].starts_with("GET /seg-009.m4s "));
        assert!(logs.iter().all(|req| req.contains("Cookie: dash=private")));
    }
}
