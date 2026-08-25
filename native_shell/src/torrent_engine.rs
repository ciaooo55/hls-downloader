//! Magnet / .torrent import. Web seeds use the HTTP engine; watch-folder imports new files.
//!
//! Swarm protocol work is frozen. Core talks only to [`TorrentSession`] so a
//! later libtorrent/librqbit backend can replace [`BuiltinTorrentEngine`].

use crate::http_engine::{run_job, Job};
use crate::{TorrentFileEntry, TorrentFileSelection};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Stable BT entry used by Core. Do not call swarm helpers from UI or NM.
pub trait TorrentSession: Send + Sync {
    fn download(
        &self,
        source: &str,
        output: &Path,
        control: &Path,
        headers: &std::collections::HashMap<String, String>,
        proxy: &str,
    ) -> Result<u64, String>;

    fn download_with_options(
        &self,
        source: &str,
        output: &Path,
        control: &Path,
        headers: &std::collections::HashMap<String, String>,
        proxy: &str,
        options: TorrentOptions,
    ) -> Result<u64, String>;

    fn download_with_telemetry(
        &self,
        source: &str,
        output: &Path,
        control: &Path,
        headers: &std::collections::HashMap<String, String>,
        proxy: &str,
        options: TorrentOptions,
        reporter: &mut dyn FnMut(TorrentTelemetry),
    ) -> Result<u64, String> {
        let result = self.download_with_options(source, output, control, headers, proxy, options);
        reporter(TorrentTelemetry::default());
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TorrentTelemetry {
    pub peer_count: u32,
    pub seed_count: u32,
    pub uploaded_bytes: u64,
    pub upload_speed_bytes_per_sec: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentOptions {
    /// The built-in client stops immediately after completion and never seeds,
    /// so its effective upload rate is always zero and therefore below this cap.
    pub upload_limit_kib: u64,
    pub max_connections: usize,
    pub enable_dht: bool,
    pub selection_path: PathBuf,
}

impl Default for TorrentOptions {
    fn default() -> Self {
        Self {
            upload_limit_kib: 1024,
            max_connections: 200,
            enable_dht: true,
            selection_path: PathBuf::new(),
        }
    }
}

/// Built-in engine. Keep this as the default until a native BT library lands.
#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltinTorrentEngine;

impl TorrentSession for BuiltinTorrentEngine {
    fn download(
        &self,
        source: &str,
        output: &Path,
        control: &Path,
        headers: &std::collections::HashMap<String, String>,
        proxy: &str,
    ) -> Result<u64, String> {
        download_torrent(source, output, control, headers, proxy)
    }

    fn download_with_options(
        &self,
        source: &str,
        output: &Path,
        control: &Path,
        headers: &std::collections::HashMap<String, String>,
        proxy: &str,
        options: TorrentOptions,
    ) -> Result<u64, String> {
        download_torrent_with_options(source, output, control, headers, proxy, options)
    }

    fn download_with_telemetry(
        &self,
        source: &str,
        output: &Path,
        control: &Path,
        headers: &std::collections::HashMap<String, String>,
        proxy: &str,
        options: TorrentOptions,
        reporter: &mut dyn FnMut(TorrentTelemetry),
    ) -> Result<u64, String> {
        download_torrent_with_telemetry(source, output, control, headers, proxy, options, reporter)
    }
}

/// Returns the BT backend Core should use. Swap the type here to change engines.
pub fn torrent_session() -> BuiltinTorrentEngine {
    BuiltinTorrentEngine
}

#[derive(Debug, Clone, PartialEq)]
pub struct TorrentMeta {
    pub name: String,
    pub magnet: bool,
    pub web_seeds: Vec<String>,
    pub info_hash: String,
    pub announce: Vec<String>,
    pub hint_peers: Vec<String>,
    pub piece_length: u64,
    pub pieces: Vec<[u8; 20]>,
    pub length: u64,
    pub files: Vec<TorrentFileEntry>,
}

pub fn parse_magnet(uri: &str) -> Result<TorrentMeta, String> {
    let rest = uri
        .strip_prefix("magnet:?")
        .ok_or_else(|| "not a magnet URI".to_string())?;
    let mut name = "torrent".to_string();
    let mut info_hash = String::new();
    let mut web_seeds = Vec::new();
    let mut announce = Vec::new();
    let mut hint_peers = Vec::new();
    for pair in rest.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decoded = url_decode(value);
        match key {
            "dn" => name = decoded,
            "xt" if decoded.to_ascii_lowercase().starts_with("urn:btih:") => {
                info_hash = decoded[9..].to_string();
            }
            "ws" | "as" => push_http_seed(&mut web_seeds, &decoded),
            "xs" => push_http_seed(&mut web_seeds, &decoded),
            "tr" => announce.push(decoded),
            "x.pe" => hint_peers.push(decoded),
            _ => {}
        }
    }
    if info_hash.is_empty() && web_seeds.is_empty() {
        return Err("magnet missing info hash and web seed".into());
    }
    Ok(TorrentMeta {
        name,
        magnet: true,
        web_seeds,
        info_hash,
        announce,
        hint_peers,
        piece_length: 0,
        pieces: Vec::new(),
        length: 0,
        files: Vec::new(),
    })
}

fn push_http_seed(seeds: &mut Vec<String>, url: &str) {
    if crate::http_engine::http_fetch_url_allowed(url) {
        seeds.push(url.to_string());
    }
}

pub fn parse_torrent_file(bytes: &[u8]) -> Result<TorrentMeta, String> {
    let value = bencode::parse(bytes).map_err(|error| error.to_string())?;
    let dict = value
        .as_dict()
        .ok_or_else(|| "torrent is not a dict".to_string())?;
    let info = dict.get(b"info".as_ref()).and_then(BValue::as_dict);
    let name = info
        .and_then(|info| info.get(b"name".as_ref()))
        .and_then(BValue::as_str)
        .unwrap_or("torrent")
        .to_string();
    let mut web_seeds = Vec::new();
    if let Some(list) = dict.get(b"url-list".as_ref()) {
        match list {
            BValue::Bytes(bytes) => {
                if let Ok(url) = std::str::from_utf8(bytes) {
                    push_http_seed(&mut web_seeds, url);
                }
            }
            BValue::List(items) => {
                for item in items {
                    if let Some(url) = item.as_str() {
                        push_http_seed(&mut web_seeds, url);
                    }
                }
            }
            _ => {}
        }
    }
    let mut announce = Vec::new();
    if let Some(url) = dict.get(b"announce".as_ref()).and_then(BValue::as_str) {
        announce.push(url.to_string());
    }
    if let Some(BValue::List(tiers)) = dict.get(b"announce-list".as_ref()) {
        for tier in tiers {
            if let BValue::List(urls) = tier {
                for url in urls {
                    if let Some(value) = url.as_str() {
                        announce.push(value.to_string());
                    }
                }
            }
        }
    }
    let piece_length = info
        .and_then(|info| info.get(b"piece length".as_ref()))
        .and_then(BValue::as_int)
        .unwrap_or(0)
        .max(0) as u64;
    let mut pieces = Vec::new();
    if let Some(raw) = info
        .and_then(|info| info.get(b"pieces".as_ref()))
        .and_then(BValue::as_bytes)
    {
        if raw.len() % 20 != 0 {
            return Err("torrent piece hash list is malformed".into());
        }
        for chunk in raw.chunks_exact(20) {
            let mut hash = [0u8; 20];
            hash.copy_from_slice(chunk);
            pieces.push(hash);
        }
    }
    let files = torrent_files(info);
    let length = if let Some(len) = info
        .and_then(|info| info.get(b"length".as_ref()))
        .and_then(BValue::as_int)
    {
        len.max(0) as u64
    } else if let Some(BValue::List(files)) = info.and_then(|info| info.get(b"files".as_ref())) {
        files
            .iter()
            .filter_map(|file| file.as_dict()?.get(b"length".as_ref())?.as_int())
            .map(|len| len.max(0) as u64)
            .sum()
    } else {
        0
    };
    if length > 0 {
        if piece_length == 0 {
            return Err("torrent piece length is missing".into());
        }
        let expected_pieces = length.div_ceil(piece_length) as usize;
        if pieces.len() != expected_pieces {
            return Err(format!(
                "torrent piece hash count mismatch: expected {expected_pieces}, got {}",
                pieces.len()
            ));
        }
    }
    Ok(TorrentMeta {
        name,
        magnet: false,
        web_seeds,
        info_hash: info_hash_from_torrent(bytes).unwrap_or_default(),
        announce,
        hint_peers: Vec::new(),
        piece_length,
        pieces,
        length,
        files,
    })
}

pub fn probe_torrent_source(
    source: &str,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
) -> Result<TorrentMeta, String> {
    let source = source.trim();
    if source.starts_with("magnet:") {
        let magnet = parse_magnet(source)?;
        return if magnet.pieces.is_empty() {
            fetch_magnet_metadata(&magnet, headers, proxy).or_else(|_| Ok(magnet))
        } else {
            Ok(magnet)
        };
    }
    let bytes = if source.starts_with("http://") || source.starts_with("https://") {
        crate::http_engine::fetch_bytes(source, headers, proxy)
            .map_err(|error| error.to_string())?
            .1
    } else {
        fs::read(source).map_err(|error| error.to_string())?
    };
    parse_torrent_file(&bytes)
}

fn torrent_files(
    info: Option<&std::collections::BTreeMap<Vec<u8>, BValue>>,
) -> Vec<TorrentFileEntry> {
    let Some(info) = info else { return Vec::new() };
    if let Some(length) = info.get(b"length".as_ref()).and_then(BValue::as_int) {
        let path = info
            .get(b"name".as_ref())
            .and_then(BValue::as_str)
            .unwrap_or("torrent")
            .to_string();
        return vec![TorrentFileEntry {
            index: 0,
            path,
            size: length.max(0) as u64,
            offset: 0,
        }];
    }
    let Some(BValue::List(files)) = info.get(b"files".as_ref()) else {
        return Vec::new();
    };
    let mut offset = 0u64;
    let mut entries = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let Some(dict) = file.as_dict() else { continue };
        let size = dict
            .get(b"length".as_ref())
            .and_then(BValue::as_int)
            .unwrap_or(0)
            .max(0) as u64;
        let components = dict
            .get(b"path".as_ref())
            .and_then(BValue::as_list)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(BValue::as_str)
                    .filter(|part| !part.is_empty() && *part != "." && *part != "..")
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if components.is_empty() || size == 0 {
            continue;
        }
        entries.push(TorrentFileEntry {
            index: index as u32,
            path: components.join("/"),
            size,
            offset,
        });
        offset = offset.saturating_add(size);
    }
    entries
}

pub fn validate_torrent_selection(
    meta: &TorrentMeta,
    selections: &[TorrentFileSelection],
) -> Result<Vec<TorrentFileSelection>, String> {
    if selections.is_empty() {
        return Ok(meta
            .files
            .iter()
            .map(|file| TorrentFileSelection {
                index: file.index,
                path: file.path.clone(),
                selected: true,
            })
            .collect());
    }
    let mut selected = Vec::with_capacity(selections.len());
    for item in selections {
        let Some(file) = meta
            .files
            .iter()
            .find(|file| file.index == item.index && file.path == item.path)
        else {
            return Err(format!("种子文件不存在: {}", item.path));
        };
        if item.path.contains('\\')
            || item
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(format!("种子文件路径无效: {}", item.path));
        }
        selected.push(TorrentFileSelection {
            index: file.index,
            path: file.path.clone(),
            selected: item.selected,
        });
    }
    if !selected.iter().any(|item| item.selected) {
        return Err("至少选择一个种子文件".into());
    }
    Ok(selected)
}

pub fn materialize_selected_files(
    payload: &Path,
    destination: &Path,
    meta: &TorrentMeta,
    selections: &[TorrentFileSelection],
) -> Result<u64, String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let checked = validate_torrent_selection(meta, selections)?;
    let mut source = fs::File::open(payload).map_err(|error| error.to_string())?;
    let mut total = 0u64;
    for selection in checked.iter().filter(|item| item.selected) {
        let file = meta
            .files
            .iter()
            .find(|file| file.index == selection.index && file.path == selection.path)
            .ok_or_else(|| format!("种子文件不存在: {}", selection.path))?;
        let target = destination.join(file.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        source
            .seek(SeekFrom::Start(file.offset))
            .map_err(|error| error.to_string())?;
        let mut output = fs::File::create(&target).map_err(|error| error.to_string())?;
        let mut remaining = file.size;
        let mut buffer = [0u8; 64 * 1024];
        while remaining > 0 {
            let chunk = remaining.min(buffer.len() as u64) as usize;
            let read_size = source
                .read(&mut buffer[..chunk])
                .map_err(|error| error.to_string())?;
            if read_size == 0 {
                return Err(format!("种子临时数据不完整: {}", selection.path));
            }
            output
                .write_all(&buffer[..read_size])
                .map_err(|error| error.to_string())?;
            remaining -= read_size as u64;
            total = total.saturating_add(read_size as u64);
        }
    }
    Ok(total)
}

pub fn download_torrent(
    spec_url: &str,
    output: &Path,
    control: &Path,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
) -> Result<u64, String> {
    download_torrent_with_options(
        spec_url,
        output,
        control,
        headers,
        proxy,
        TorrentOptions::default(),
    )
}

pub fn download_torrent_with_options(
    spec_url: &str,
    output: &Path,
    control: &Path,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
    options: TorrentOptions,
) -> Result<u64, String> {
    download_torrent_with_telemetry(
        spec_url,
        output,
        control,
        headers,
        proxy,
        options,
        &mut |_| {},
    )
}

fn download_torrent_with_telemetry(
    spec_url: &str,
    output: &Path,
    control: &Path,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
    options: TorrentOptions,
    reporter: &mut dyn FnMut(TorrentTelemetry),
) -> Result<u64, String> {
    let meta = if spec_url.starts_with("magnet:") {
        let mut meta = parse_magnet(spec_url)?;
        if meta.pieces.is_empty() || meta.piece_length == 0 {
            match fetch_magnet_metadata(&meta, headers, proxy) {
                Ok(fetched) => meta = fetched,
                Err(error) => {
                    if meta.web_seeds.is_empty() {
                        return Err(error);
                    }
                }
            }
        }
        meta
    } else if spec_url.ends_with(".torrent") || spec_url.starts_with("http") {
        let bytes = if spec_url.starts_with("http") {
            crate::http_engine::fetch_bytes(spec_url, headers, proxy)
                .map_err(|error| error.to_string())?
                .1
        } else {
            fs::read(spec_url).map_err(|error| error.to_string())?
        };
        parse_torrent_file(&bytes)?
    } else {
        return Err("unsupported torrent source".into());
    };
    let http_seeds: Vec<String> = meta
        .web_seeds
        .iter()
        .filter(|url| crate::http_engine::http_fetch_url_allowed(url))
        .cloned()
        .collect();
    if let Some(seed) = http_seeds.first().cloned() {
        reporter(TorrentTelemetry::default());
        let job = Job {
            url: seed,
            headers: headers.clone(),
            output: output.to_path_buf(),
            connections: 8,
            chunk_bytes: 8 * 1024 * 1024,
            total: meta.length,
            sequential: true,
            resume_from: 0,
            proxy: proxy.to_string(),
            resource_key: meta.info_hash.clone(),
            etag: String::new(),
            last_modified: String::new(),
            control: control.to_path_buf(),
            progress: output.with_extension("progress.json"),
            method: "GET".into(),
            body_path: PathBuf::new(),
            mirrors: http_seeds,
            replay_json: String::new(),
        };
        run_job(&job).map_err(|error| error.to_string())?;
        return fs::metadata(output)
            .map(|meta| meta.len())
            .map_err(|error| error.to_string());
    }
    if meta.pieces.is_empty() || meta.piece_length == 0 {
        return Err(format!(
            "种子没有 HTTP web seed 且缺少 piece 元数据（info_hash={}）",
            meta.info_hash
        ));
    }
    download_swarm(&meta, output, control, headers, proxy, options, reporter)
}

#[derive(Default)]
pub struct TorrentWatch {
    seen: BTreeSet<PathBuf>,
}

impl TorrentWatch {
    pub fn scan(&mut self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut fresh = Vec::new();
        for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            let ext = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(ext.as_str(), "torrent" | "magnet" | "url") {
                continue;
            }
            if self.seen.insert(path.clone()) {
                fresh.push(path);
            }
        }
        Ok(fresh)
    }

    pub fn prime(&mut self, dir: &Path) -> Result<(), String> {
        let _ = self.scan(dir)?;
        Ok(())
    }
}

fn fetch_magnet_metadata(
    magnet: &TorrentMeta,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
) -> Result<TorrentMeta, String> {
    let hash = canonical_info_hash(&magnet.info_hash)
        .ok_or_else(|| format!("magnet info_hash 无效: {}", magnet.info_hash))?;
    let mut last = "未能取得种子元数据".to_string();
    for url in [
        format!("https://itorrents.org/torrent/{hash}.torrent"),
        format!("https://itorrent.ws/torrent/{hash}.torrent"),
        format!("https://btcache.me/torrent/{hash}"),
        format!("https://thetorrent.org/torrent/{hash}.torrent"),
    ] {
        match crate::http_engine::fetch_bytes(&url, headers, proxy) {
            Ok((status, body)) if status == 200 || status == 206 => {
                if let Ok(mut parsed) = parse_torrent_file(&body) {
                    if parsed.pieces.is_empty() {
                        last = "元数据没有 piece 列表".into();
                        continue;
                    }
                    if parsed.name == "torrent" && magnet.name != "torrent" {
                        parsed.name = magnet.name.clone();
                    }
                    for seed in &magnet.web_seeds {
                        if !parsed.web_seeds.contains(seed) {
                            parsed.web_seeds.push(seed.clone());
                        }
                    }
                    for announce in &magnet.announce {
                        if !parsed.announce.contains(announce) {
                            parsed.announce.push(announce.clone());
                        }
                    }
                    parsed.hint_peers.extend(magnet.hint_peers.iter().cloned());
                    return Ok(parsed);
                }
            }
            Ok((status, _)) => last = format!("{url} HTTP {status}"),
            Err(error) => last = error.to_string(),
        }
    }
    if let Ok(from_swarm) = fetch_metadata_from_swarm(magnet, headers, proxy) {
        return Ok(from_swarm);
    }
    Err(last)
}

fn canonical_info_hash(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches("urn:btih:");
    let upper = value.to_ascii_uppercase();
    if upper.len() == 40 && upper.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Some(upper);
    }
    if upper.len() == 32 {
        let bytes = decode_base32(&upper)?;
        if bytes.len() == 20 {
            return Some(bytes.iter().map(|byte| format!("{byte:02X}")).collect());
        }
    }
    None
}

fn decode_base32(value: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits: u32 = 0;
    let mut bit_count = 0;
    let mut out = Vec::new();
    for ch in value.chars() {
        let index = ALPHABET.iter().position(|&item| item == ch as u8)?;
        bits = (bits << 5) | index as u32;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
            bits &= (1 << bit_count) - 1;
        }
    }
    Some(out)
}

fn info_hash_from_torrent(bytes: &[u8]) -> Option<String> {
    let needle = b"4:info";
    let pos = bytes
        .windows(needle.len())
        .position(|window| window == needle)?;
    let start = pos + needle.len();
    let (_, rest) = bencode::parse_value(&bytes[start..]).ok()?;
    let consumed = bytes[start..].len().checked_sub(rest.len())?;
    Some(crate::crypto_lite::sha1_hex(
        &bytes[start..start + consumed],
    ))
}

fn download_swarm(
    meta: &TorrentMeta,
    output: &Path,
    control: &Path,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
    options: TorrentOptions,
    reporter: &mut dyn FnMut(TorrentTelemetry),
) -> Result<u64, String> {
    let mut peers = announce_peers(meta, headers, proxy, options.enable_dht)?;
    if peers.is_empty() {
        return Err("tracker returned no peers".into());
    }
    let mut telemetry = TorrentTelemetry {
        peer_count: peers.len().min(u32::MAX as usize) as u32,
        ..TorrentTelemetry::default()
    };
    reporter(telemetry);
    let mut seen = BTreeSet::new();
    let mut last = "all peers failed".to_string();
    let mut index = 0;
    while index < peers.len() && index < options.max_connections.clamp(10, 1000) {
        let peer = peers[index];
        index += 1;
        if !seen.insert(peer) {
            continue;
        }
        let mut extra = Vec::new();
        match download_from_peer_ex_with_telemetry(
            peer,
            meta,
            output,
            control,
            &mut extra,
            &mut telemetry,
            reporter,
            &options.selection_path,
        ) {
            Ok(len) => return Ok(len),
            Err(error) => last = error,
        }
        for addr in extra {
            if !peers.contains(&addr) {
                peers.push(addr);
            }
        }
        if fs::read_to_string(control).unwrap_or_default().trim() != "run" {
            return Err(fs::read_to_string(control).unwrap_or_else(|_| "paused".into()));
        }
    }
    Err(last)
}

fn announce_peers(
    meta: &TorrentMeta,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
    enable_dht: bool,
) -> Result<Vec<std::net::SocketAddr>, String> {
    let info_hash = decode_info_hash(&meta.info_hash)?;
    let mut peers = resolve_hint_peers(&meta.hint_peers);
    let announces: Vec<String> = if meta.announce.is_empty() {
        DEFAULT_TRACKERS
            .iter()
            .map(|url| (*url).to_string())
            .collect()
    } else {
        meta.announce.clone()
    };
    for announce in &announces {
        if announce.starts_with("udp://") {
            if let Ok(found) = announce_udp(announce, &info_hash, meta.length) {
                peers.extend(found);
            }
            continue;
        }
        if !announce.starts_with("http://") && !announce.starts_with("https://") {
            continue;
        }
        let sep = if announce.contains('?') { "&" } else { "?" };
        let url = format!(
            "{announce}{sep}info_hash={}&peer_id={}&port=6881&uploaded=0&downloaded=0&left={}&compact=1",
            percent_encode(&info_hash),
            percent_encode(b"-HL0001-0123456789ab"),
            meta.length
        );
        if let Ok((_, body)) = crate::http_engine::fetch_bytes(&url, headers, proxy) {
            peers.extend(parse_compact_peers(&body));
        }
    }
    if enable_dht && dht_enabled() {
        if let Ok(found) = dht_get_peers(&info_hash) {
            peers.extend(found);
        }
    }
    peers.sort_by_key(|addr| addr.to_string());
    peers.dedup();
    Ok(peers)
}

const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "http://tracker.openbittorrent.com:80/announce",
];

fn dht_enabled() -> bool {
    !matches!(
        std::env::var("HLS_V6_DHT").ok().as_deref(),
        Some("0") | Some("off") | Some("false")
    )
}

fn resolve_hint_peers(hints: &[String]) -> Vec<std::net::SocketAddr> {
    let mut peers = Vec::new();
    for hint in hints {
        if let Ok(addr) = hint.parse() {
            peers.push(addr);
            continue;
        }
        if let Ok(resolved) = hint.parse::<std::net::SocketAddr>() {
            peers.push(resolved);
        }
    }
    peers
}

fn fetch_metadata_from_swarm(
    magnet: &TorrentMeta,
    headers: &std::collections::HashMap<String, String>,
    proxy: &str,
) -> Result<TorrentMeta, String> {
    let peers = announce_peers(magnet, headers, proxy, dht_enabled())?;
    if peers.is_empty() {
        return Err("magnet 没有 tracker 返回的节点".into());
    }
    let mut last = "节点没有提供 ut_metadata".to_string();
    let mut queue = peers;
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < queue.len() && index < 24 {
        let peer = queue[index];
        index += 1;
        if !seen.insert(peer) {
            continue;
        }
        let mut extra = Vec::new();
        match fetch_ut_metadata_from_peer(peer, magnet, &mut extra) {
            Ok(meta) => return Ok(meta),
            Err(error) => last = error,
        }
        for addr in extra {
            if !queue.contains(&addr) {
                queue.push(addr);
            }
        }
    }
    Err(last)
}

pub fn parse_udp_tracker(url: &str) -> Option<(String, u16)> {
    let rest = url.strip_prefix("udp://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    if let Some((host, port)) = authority.rsplit_once(':') {
        Some((host.to_string(), port.parse().ok()?))
    } else {
        Some((authority.to_string(), 80))
    }
}

pub fn udp_connect_request(transaction_id: u32) -> [u8; 16] {
    let mut packet = [0u8; 16];
    packet[..8].copy_from_slice(&0x4172_7101_980u64.to_be_bytes());
    packet[8..12].copy_from_slice(&0u32.to_be_bytes());
    packet[12..16].copy_from_slice(&transaction_id.to_be_bytes());
    packet
}

fn announce_udp(
    url: &str,
    info_hash: &[u8],
    left: u64,
) -> Result<Vec<std::net::SocketAddr>, String> {
    use std::net::UdpSocket;
    let (host, port) = parse_udp_tracker(url).ok_or_else(|| "udp tracker URL 无效".to_string())?;
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(4)))
        .map_err(|error| error.to_string())?;
    let target = format!("{host}:{port}");
    let tx = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        & 0xffff_ffff) as u32;
    socket
        .send_to(&udp_connect_request(tx), &target)
        .map_err(|error| error.to_string())?;
    let mut buf = [0u8; 1024];
    let (count, _) = socket
        .recv_from(&mut buf)
        .map_err(|error| error.to_string())?;
    if count < 16 || u32::from_be_bytes(buf[0..4].try_into().unwrap()) != 0 {
        return Err("udp tracker connect 失败".into());
    }
    if u32::from_be_bytes(buf[4..8].try_into().unwrap()) != tx {
        return Err("udp tracker transaction mismatch".into());
    }
    let connection_id = &buf[8..16];
    let tx2 = tx.wrapping_add(1);
    let mut announce = Vec::with_capacity(98);
    announce.extend_from_slice(connection_id);
    announce.extend_from_slice(&1u32.to_be_bytes());
    announce.extend_from_slice(&tx2.to_be_bytes());
    announce.extend_from_slice(info_hash);
    announce.extend_from_slice(b"-HL0001-0123456789ab");
    announce.extend_from_slice(&0u64.to_be_bytes());
    announce.extend_from_slice(&left.to_be_bytes());
    announce.extend_from_slice(&0u64.to_be_bytes());
    announce.extend_from_slice(&0u32.to_be_bytes());
    announce.extend_from_slice(&0u32.to_be_bytes());
    announce.extend_from_slice(&tx2.to_be_bytes());
    announce.extend_from_slice(&((-1i32) as u32).to_be_bytes());
    announce.extend_from_slice(&6881u16.to_be_bytes());
    socket
        .send_to(&announce, &target)
        .map_err(|error| error.to_string())?;
    let (count, _) = socket
        .recv_from(&mut buf)
        .map_err(|error| error.to_string())?;
    if count < 20 || u32::from_be_bytes(buf[0..4].try_into().unwrap()) != 1 {
        return Err("udp tracker announce 失败".into());
    }
    Ok(decode_compact(&buf[20..count]))
}

pub fn krpc_get_peers_query(txid: &[u8], node_id: &[u8], info_hash: &[u8]) -> Vec<u8> {
    let mut out = b"d1:ad2:id20:".to_vec();
    out.extend_from_slice(node_id);
    out.extend_from_slice(b"9:info_hash20:");
    out.extend_from_slice(info_hash);
    out.extend_from_slice(b"e1:q9:get_peers1:t");
    out.extend_from_slice(format!("{}:", txid.len()).as_bytes());
    out.extend_from_slice(txid);
    out.extend_from_slice(b"1:y1:qe");
    out
}

pub fn parse_krpc_peers(body: &[u8]) -> (Vec<std::net::SocketAddr>, Vec<std::net::SocketAddr>) {
    let Ok(value) = bencode::parse(body) else {
        return (Vec::new(), Vec::new());
    };
    let Some(dict) = value.as_dict() else {
        return (Vec::new(), Vec::new());
    };
    let Some(reply) = dict.get(b"r".as_ref()).and_then(BValue::as_dict) else {
        return (Vec::new(), Vec::new());
    };
    let mut peers = Vec::new();
    if let Some(values) = reply.get(b"values".as_ref()) {
        match values {
            BValue::List(items) => {
                for item in items {
                    if let Some(bytes) = item.as_bytes() {
                        peers.extend(decode_compact(bytes));
                    }
                }
            }
            BValue::Bytes(bytes) => peers.extend(decode_compact(bytes)),
            _ => {}
        }
    }
    let nodes = reply
        .get(b"nodes".as_ref())
        .and_then(BValue::as_bytes)
        .map(decode_compact_nodes)
        .unwrap_or_default();
    (peers, nodes)
}

pub fn decode_compact_nodes(raw: &[u8]) -> Vec<std::net::SocketAddr> {
    raw.chunks_exact(26)
        .map(|chunk| {
            let ip = std::net::Ipv4Addr::new(chunk[20], chunk[21], chunk[22], chunk[23]);
            let port = u16::from_be_bytes([chunk[24], chunk[25]]);
            std::net::SocketAddr::from((ip, port))
        })
        .collect()
}

fn dht_get_peers(info_hash: &[u8]) -> Result<Vec<std::net::SocketAddr>, String> {
    use std::net::{ToSocketAddrs, UdpSocket};
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|error| error.to_string())?;
    let node_id = crate::crypto_lite::sha1(format!("hls-v6-{}", std::process::id()).as_bytes());
    let mut bootstrap = Vec::new();
    for host in [
        "router.bittorrent.com:6881",
        "dht.transmissionbt.com:6881",
        "router.utorrent.com:6881",
        "dht.libtorrent.org:25401",
    ] {
        if let Ok(addrs) = host.to_socket_addrs() {
            bootstrap.extend(addrs.filter(std::net::SocketAddr::is_ipv4).take(1));
        }
    }
    let mut queried = 0u16;
    for target in bootstrap.iter().copied() {
        let tx = queried.to_be_bytes();
        let _ = socket.send_to(&krpc_get_peers_query(&tx, &node_id, info_hash), target);
        queried = queried.saturating_add(1);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut peers = Vec::new();
    while Instant::now() < deadline {
        let mut buf = [0u8; 2048];
        match socket.recv_from(&mut buf) {
            Ok((count, _)) => {
                let (found, closer) = parse_krpc_peers(&buf[..count]);
                peers.extend(found);
                for node in closer.into_iter().take(8) {
                    if queried >= 24 {
                        break;
                    }
                    let tx = queried.to_be_bytes();
                    let _ = socket.send_to(&krpc_get_peers_query(&tx, &node_id, info_hash), node);
                    queried = queried.saturating_add(1);
                }
            }
            Err(_) => continue,
        }
    }
    peers.sort_by_key(|addr| addr.to_string());
    peers.dedup();
    Ok(peers)
}

fn fetch_ut_metadata_from_peer(
    addr: std::net::SocketAddr,
    magnet: &TorrentMeta,
    extra_peers: &mut Vec<std::net::SocketAddr>,
) -> Result<TorrentMeta, String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(8))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(12)))
        .map_err(|error| error.to_string())?;
    let info_hash = decode_info_hash(&magnet.info_hash)?;
    let mut handshake = Vec::from([19u8]);
    handshake.extend_from_slice(b"BitTorrent protocol");
    let mut reserved = [0u8; 8];
    reserved[5] |= 0x10;
    handshake.extend_from_slice(&reserved);
    handshake.extend_from_slice(&info_hash);
    handshake.extend_from_slice(b"-HL0001-0123456789ab");
    stream
        .write_all(&handshake)
        .map_err(|error| error.to_string())?;
    let mut peer_hs = [0u8; 68];
    stream
        .read_exact(&mut peer_hs)
        .map_err(|error| error.to_string())?;
    if &peer_hs[28..48] != info_hash.as_slice() {
        return Err("peer info_hash mismatch".into());
    }
    if peer_hs[25] & 0x10 == 0 {
        return Err("peer 不支持扩展协议".into());
    }
    let handshake_body = b"d1:md11:ut_metadatai1e6:ut_pexi2ee1:pi6881ee";
    send_extended(&mut stream, 0, handshake_body)?;
    let mut metadata_id = 0u8;
    let mut pex_id = 0u8;
    let mut metadata_size = 0usize;
    for _ in 0..24 {
        let (id, body) = read_message(&mut stream)?;
        take_extended(id, &body, &mut pex_id, extra_peers);
        if id != 20 || body.is_empty() {
            continue;
        }
        if body[0] == 0 {
            let (value, _) = bencode::parse_value(&body[1..])?;
            let dict = value
                .as_dict()
                .ok_or_else(|| "extended handshake 不是 dict".to_string())?;
            if let Some(size) = dict.get(b"metadata_size".as_ref()).and_then(BValue::as_int) {
                metadata_size = size.max(0) as usize;
            }
            if let Some(m) = dict.get(b"m".as_ref()).and_then(BValue::as_dict) {
                if let Some(id) = m.get(b"ut_metadata".as_ref()).and_then(BValue::as_int) {
                    metadata_id = id.max(0) as u8;
                }
            }
            break;
        }
    }
    if metadata_id == 0 {
        return Err("peer 没有 ut_metadata".into());
    }
    if metadata_size == 0 {
        return Err("peer 没有 metadata_size".into());
    }
    let pieces = metadata_size.div_ceil(16 * 1024);
    let mut info = vec![0u8; metadata_size];
    for index in 0..pieces {
        let request = format!("d8:msg_typei0e5:piecei{index}ee");
        send_extended(&mut stream, metadata_id, request.as_bytes())?;
        loop {
            let (id, body) = read_message(&mut stream)?;
            take_extended(id, &body, &mut pex_id, extra_peers);
            if id != 20 || body.len() < 2 || body[0] == 0 {
                continue;
            }
            let (header, rest) = bencode::parse_value(&body[1..])?;
            let dict = header
                .as_dict()
                .ok_or_else(|| "ut_metadata 不是 dict".to_string())?;
            let msg_type = dict
                .get(b"msg_type".as_ref())
                .and_then(BValue::as_int)
                .unwrap_or(-1);
            if msg_type == 2 {
                return Err("peer 拒绝提供元数据".into());
            }
            if msg_type != 1 {
                continue;
            }
            let piece = dict
                .get(b"piece".as_ref())
                .and_then(BValue::as_int)
                .unwrap_or(0) as usize;
            if piece != index {
                continue;
            }
            let start = piece * 16 * 1024;
            let end = (start + rest.len()).min(info.len());
            if end > start {
                info[start..end].copy_from_slice(&rest[..end - start]);
            }
            break;
        }
    }
    if crate::crypto_lite::sha1_hex(&info).to_ascii_uppercase()
        != canonical_info_hash(&magnet.info_hash).unwrap_or_default()
    {
        return Err("ut_metadata info_hash 不匹配".into());
    }
    let mut torrent = b"d4:info".to_vec();
    torrent.extend_from_slice(&info);
    torrent.push(b'e');
    let mut parsed = parse_torrent_file(&torrent)?;
    if parsed.name == "torrent" && magnet.name != "torrent" {
        parsed.name = magnet.name.clone();
    }
    parsed.web_seeds.extend(magnet.web_seeds.iter().cloned());
    parsed.announce.extend(magnet.announce.iter().cloned());
    parsed.hint_peers.extend(magnet.hint_peers.iter().cloned());
    parsed.announce.sort();
    parsed.announce.dedup();
    parsed.web_seeds.sort();
    parsed.web_seeds.dedup();
    parsed.hint_peers.sort();
    parsed.hint_peers.dedup();
    Ok(parsed)
}

fn send_extended(
    stream: &mut std::net::TcpStream,
    ext_id: u8,
    payload: &[u8],
) -> Result<(), String> {
    let mut body = Vec::with_capacity(payload.len() + 1);
    body.push(ext_id);
    body.extend_from_slice(payload);
    send_message(stream, 20, &body)
}

fn take_extended(
    id: u8,
    body: &[u8],
    pex_id: &mut u8,
    extra_peers: &mut Vec<std::net::SocketAddr>,
) {
    if id != 20 || body.is_empty() {
        return;
    }
    if body[0] == 0 {
        if let Ok((value, _)) = bencode::parse_value(&body[1..]) {
            if let Some(m) = value
                .as_dict()
                .and_then(|dict| dict.get(b"m".as_ref()))
                .and_then(BValue::as_dict)
            {
                if let Some(id) = m.get(b"ut_pex".as_ref()).and_then(BValue::as_int) {
                    *pex_id = id.max(0) as u8;
                }
            }
        }
        return;
    }
    if *pex_id != 0 && body[0] != *pex_id {
        return;
    }
    extra_peers.extend(parse_ut_pex(&body[1..]));
}

pub fn parse_ut_pex(payload: &[u8]) -> Vec<std::net::SocketAddr> {
    let Ok(value) = bencode::parse(payload) else {
        return Vec::new();
    };
    let Some(dict) = value.as_dict() else {
        return Vec::new();
    };
    dict.get(b"added".as_ref())
        .and_then(BValue::as_bytes)
        .map(decode_compact)
        .unwrap_or_default()
}

pub fn parse_compact_peers(body: &[u8]) -> Vec<std::net::SocketAddr> {
    if let Ok(value) = bencode::parse(body) {
        if let Some(dict) = value.as_dict() {
            if let Some(raw) = dict.get(b"peers".as_ref()).and_then(BValue::as_bytes) {
                return decode_compact(raw);
            }
        }
    }
    decode_compact(body)
}

fn decode_compact(raw: &[u8]) -> Vec<std::net::SocketAddr> {
    raw.chunks_exact(6)
        .map(|chunk| {
            let ip = std::net::Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            std::net::SocketAddr::from((ip, port))
        })
        .collect()
}

fn decode_info_hash(value: &str) -> Result<Vec<u8>, String> {
    if value.len() == 40 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        let mut out = Vec::with_capacity(20);
        for index in (0..40).step_by(2) {
            out.push(
                u8::from_str_radix(&value[index..index + 2], 16)
                    .map_err(|error| error.to_string())?,
            );
        }
        return Ok(out);
    }
    Err("info_hash must be 40 hex chars".into())
}

fn percent_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("%{byte:02X}")).collect()
}

fn download_from_peer(
    addr: std::net::SocketAddr,
    meta: &TorrentMeta,
    output: &Path,
    control: &Path,
) -> Result<u64, String> {
    let mut extra = Vec::new();
    download_from_peer_ex(addr, meta, output, control, &mut extra)
}

fn download_from_peer_ex(
    addr: std::net::SocketAddr,
    meta: &TorrentMeta,
    output: &Path,
    control: &Path,
    extra_peers: &mut Vec<std::net::SocketAddr>,
) -> Result<u64, String> {
    let mut telemetry = TorrentTelemetry::default();
    download_from_peer_ex_with_telemetry(
        addr,
        meta,
        output,
        control,
        extra_peers,
        &mut telemetry,
        &mut |_| {},
        Path::new(""),
    )
}

fn download_from_peer_ex_with_telemetry(
    addr: std::net::SocketAddr,
    meta: &TorrentMeta,
    output: &Path,
    control: &Path,
    extra_peers: &mut Vec<std::net::SocketAddr>,
    telemetry: &mut TorrentTelemetry,
    reporter: &mut dyn FnMut(TorrentTelemetry),
    selection_path: &Path,
) -> Result<u64, String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::net::TcpStream;
    use std::time::Duration;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(output)
        .map_err(|error| error.to_string())?;
    file.set_len(meta.length)
        .map_err(|error| error.to_string())?;
    let initial_pending: Vec<usize> = meta
        .pieces
        .iter()
        .enumerate()
        .filter(|(index, hash)| {
            let start = *index as u64 * meta.piece_length;
            let len = ((meta.length - start).min(meta.piece_length)) as usize;
            !piece_is_complete(&mut file, start, len, hash)
        })
        .map(|(index, _)| index)
        .collect();
    if initial_pending.is_empty() {
        return Ok(meta.length);
    }
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(8))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|error| error.to_string())?;
    let info_hash = decode_info_hash(&meta.info_hash)?;
    let mut handshake = Vec::from([19u8]);
    handshake.extend_from_slice(b"BitTorrent protocol");
    let mut reserved = [0u8; 8];
    reserved[5] |= 0x10;
    handshake.extend_from_slice(&reserved);
    handshake.extend_from_slice(&info_hash);
    handshake.extend_from_slice(b"-HL0001-0123456789ab");
    stream
        .write_all(&handshake)
        .map_err(|error| error.to_string())?;
    let mut peer_hs = [0u8; 68];
    stream
        .read_exact(&mut peer_hs)
        .map_err(|error| error.to_string())?;
    if &peer_hs[28..48] != info_hash.as_slice() {
        return Err("peer info_hash mismatch".into());
    }
    telemetry.peer_count = telemetry.peer_count.max(1);
    reporter(*telemetry);
    let mut pex_id = 0u8;
    if peer_hs[25] & 0x10 != 0 {
        send_extended(
            &mut stream,
            0,
            b"d1:md11:ut_metadatai1e6:ut_pexi2ee1:pi6881ee",
        )?;
    }
    send_message(&mut stream, 2, &[])?; // interested
    let reader = PeerMessageReader::start(&stream)?;
    let mut unchoked = false;
    let mut peer_pieces = vec![false; meta.pieces.len()];
    loop {
        let pending: Vec<usize> = meta
            .pieces
            .iter()
            .enumerate()
            .filter(|(index, hash)| {
                let start = *index as u64 * meta.piece_length;
                let len = ((meta.length - start).min(meta.piece_length)) as usize;
                !piece_is_complete(&mut file, start, len, hash)
                    && piece_is_selected(meta, *index, selection_path)
            })
            .map(|(index, _)| index)
            .collect();
        if pending.is_empty() {
            break;
        }
        'pieces: for index in pending {
            if !unchoked {
                while !unchoked {
                    if !control_is_running(control) {
                        return Err("paused".into());
                    }
                    if !piece_is_selected(meta, index, selection_path) {
                        continue 'pieces;
                    }
                    let Some(message) = reader.recv_timeout(Duration::from_millis(100))? else {
                        continue;
                    };
                    match message {
                        (1, _) => unchoked = true,
                        (0, _) => unchoked = false,
                        (id, body) => {
                            note_peer_availability(id, &body, &mut peer_pieces);
                            take_extended(id, &body, &mut pex_id, extra_peers);
                        }
                    }
                }
            }
            if !control_is_running(control) {
                return Err("paused".into());
            }
            let hash = &meta.pieces[index];
            let start = index as u64 * meta.piece_length;
            let len = ((meta.length - start).min(meta.piece_length)) as usize;
            let mut piece = vec![0u8; len];
            let mut filled = 0;
            while filled < len {
                while !unchoked {
                    if !control_is_running(control) {
                        return Err("paused".into());
                    }
                    if !piece_is_selected(meta, index, selection_path) {
                        continue 'pieces;
                    }
                    let Some((id, body)) = reader.recv_timeout(Duration::from_millis(100))? else {
                        continue;
                    };
                    match id {
                        1 => unchoked = true,
                        0 => unchoked = false,
                        _ => {
                            note_peer_availability(id, &body, &mut peer_pieces);
                            take_extended(id, &body, &mut pex_id, extra_peers);
                        }
                    }
                }
                if !piece_is_selected(meta, index, selection_path) {
                    continue 'pieces;
                }
                let block = (len - filled).min(16 * 1024);
                let mut payload = Vec::new();
                payload.extend_from_slice(&(index as u32).to_be_bytes());
                payload.extend_from_slice(&(filled as u32).to_be_bytes());
                payload.extend_from_slice(&(block as u32).to_be_bytes());
                send_message(&mut stream, 6, &payload)?;
                loop {
                    if !control_is_running(control) {
                        let _ = send_message(&mut stream, 8, &payload);
                        return Err("paused".into());
                    }
                    if !piece_is_selected(meta, index, selection_path) {
                        let _ = send_message(&mut stream, 8, &payload);
                        continue 'pieces;
                    }
                    let Some((id, body)) = reader.recv_timeout(Duration::from_millis(100))? else {
                        continue;
                    };
                    if id == 1 {
                        unchoked = true;
                    } else if id == 0 {
                        unchoked = false;
                        let _ = send_message(&mut stream, 8, &payload);
                        break;
                    } else if id == 7 {
                        if body.len() < 8 {
                            return Err("truncated peer piece message".into());
                        }
                        let piece_index =
                            u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
                        let begin = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
                        let data = &body[8..];
                        if piece_index != index || begin != filled {
                            continue; // late response for a canceled or superseded request
                        }
                        if data.len() != block || begin + data.len() > piece.len() {
                            return Err(format!(
                                "invalid peer piece block: index={piece_index} begin={begin} length={}",
                                data.len()
                            ));
                        }
                        piece[begin..begin + data.len()].copy_from_slice(data);
                        filled += data.len();
                        break;
                    } else {
                        note_peer_availability(id, &body, &mut peer_pieces);
                        take_extended(id, &body, &mut pex_id, extra_peers);
                    }
                }
            }
            if crate::crypto_lite::sha1(&piece) != *hash {
                return Err(format!("piece {index} hash mismatch"));
            }
            file.seek(SeekFrom::Start(start))
                .map_err(|error| error.to_string())?;
            file.write_all(&piece).map_err(|error| error.to_string())?;
            crate::net_policy::consume(piece.len());
        }
    }
    telemetry.seed_count =
        u32::from(!peer_pieces.is_empty() && peer_pieces.iter().all(|available| *available));
    reporter(*telemetry);
    Ok(meta.length)
}

fn piece_is_selected(meta: &TorrentMeta, piece_index: usize, selection_path: &Path) -> bool {
    if selection_path.as_os_str().is_empty() || !selection_path.is_file() {
        return true;
    }
    let Ok(encoded) = fs::read(selection_path) else {
        return false;
    };
    let Ok(selections) = serde_json::from_slice::<Vec<TorrentFileSelection>>(&encoded) else {
        return false;
    };
    if selections.is_empty() {
        return true;
    }
    let piece_start = piece_index as u64 * meta.piece_length;
    let piece_end = (piece_start + meta.piece_length).min(meta.length);
    meta.files.iter().any(|file| {
        selections
            .iter()
            .any(|selection| selection.index == file.index && selection.selected)
            && piece_start < file.offset.saturating_add(file.size)
            && file.offset < piece_end
    })
}

fn note_peer_availability(id: u8, body: &[u8], pieces: &mut [bool]) {
    match id {
        4 if body.len() >= 4 => {
            let index = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
            if let Some(available) = pieces.get_mut(index) {
                *available = true;
            }
        }
        5 => {
            for (index, available) in pieces.iter_mut().enumerate() {
                let byte = body.get(index / 8).copied().unwrap_or(0);
                *available = byte & (0x80 >> (index % 8)) != 0;
            }
        }
        _ => {}
    }
}

fn piece_is_complete(file: &mut fs::File, start: u64, len: usize, hash: &[u8; 20]) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut buf = vec![0u8; len];
    if file.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    if file.read_exact(&mut buf).is_err() {
        return false;
    }
    crate::crypto_lite::sha1(&buf) == *hash
}

fn send_message(stream: &mut std::net::TcpStream, id: u8, payload: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let len = (payload.len() as u32) + 1;
    stream
        .write_all(&len.to_be_bytes())
        .map_err(|error| error.to_string())?;
    stream.write_all(&[id]).map_err(|error| error.to_string())?;
    stream.write_all(payload).map_err(|error| error.to_string())
}

fn control_is_running(control: &Path) -> bool {
    fs::read_to_string(control).unwrap_or_default().trim() == "run"
}

struct PeerMessageReader {
    receiver: std::sync::mpsc::Receiver<Result<(u8, Vec<u8>), String>>,
    shutdown: std::net::TcpStream,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PeerMessageReader {
    fn start(stream: &std::net::TcpStream) -> Result<Self, String> {
        let mut read_stream = stream.try_clone().map_err(|error| error.to_string())?;
        read_stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .map_err(|error| error.to_string())?;
        let shutdown = read_stream.try_clone().map_err(|error| error.to_string())?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || loop {
            let message = read_message(&mut read_stream);
            let failed = message.is_err();
            if sender.send(message).is_err() || failed {
                break;
            }
        });
        Ok(Self {
            receiver,
            shutdown,
            handle: Some(handle),
        })
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<Option<(u8, Vec<u8>)>, String> {
        match self.receiver.recv_timeout(timeout) {
            Ok(message) => message.map(Some),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("peer reader disconnected".into())
            }
        }
    }
}

impl Drop for PeerMessageReader {
    fn drop(&mut self) {
        let _ = self.shutdown.shutdown(std::net::Shutdown::Both);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_message(stream: &mut std::net::TcpStream) -> Result<(u8, Vec<u8>), String> {
    use std::io::Read;
    loop {
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .map_err(|error| error.to_string())?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len == 0 {
            continue; // keep-alive
        }
        if len > 1024 * 1024 {
            return Err("peer message too large".into());
        }
        let mut body = vec![0u8; len];
        stream
            .read_exact(&mut body)
            .map_err(|error| error.to_string())?;
        return Ok((body[0], body[1..].to_vec()));
    }
}

fn url_decode(value: &str) -> String {
    let mut out = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let Ok(byte) = u8::from_str_radix(
                    std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                    16,
                ) {
                    out.push(byte as char);
                    index += 3;
                } else {
                    out.push('%');
                    index += 1;
                }
            }
            byte => {
                out.push(byte as char);
                index += 1;
            }
        }
    }
    out
}

mod bencode {
    use super::BValue;
    use std::collections::BTreeMap;

    pub fn parse(input: &[u8]) -> Result<BValue, String> {
        let (value, rest) = parse_value(input)?;
        if !rest.is_empty() && rest.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Err("trailing torrent bytes".into());
        }
        Ok(value)
    }

    pub(super) fn parse_value(input: &[u8]) -> Result<(BValue, &[u8]), String> {
        match input.first() {
            Some(b'i') => parse_int(input),
            Some(b'l') => parse_list(input),
            Some(b'd') => parse_dict(input),
            Some(b'0'..=b'9') => parse_bytes(input),
            _ => Err("invalid bencode".into()),
        }
    }

    fn parse_int(input: &[u8]) -> Result<(BValue, &[u8]), String> {
        let end = input
            .iter()
            .position(|byte| *byte == b'e')
            .ok_or("truncated int")?;
        let number = std::str::from_utf8(&input[1..end]).map_err(|error| error.to_string())?;
        Ok((BValue::Int(number.parse().unwrap_or(0)), &input[end + 1..]))
    }

    fn parse_bytes(input: &[u8]) -> Result<(BValue, &[u8]), String> {
        let colon = input
            .iter()
            .position(|byte| *byte == b':')
            .ok_or("truncated bytes")?;
        let len: usize = std::str::from_utf8(&input[..colon])
            .map_err(|error| error.to_string())?
            .parse()
            .map_err(|error: std::num::ParseIntError| error.to_string())?;
        let start = colon + 1;
        let end = start + len;
        if end > input.len() {
            return Err("truncated byte string".into());
        }
        Ok((BValue::Bytes(input[start..end].to_vec()), &input[end..]))
    }

    fn parse_list(input: &[u8]) -> Result<(BValue, &[u8]), String> {
        let mut rest = &input[1..];
        let mut items = Vec::new();
        while rest.first() != Some(&b'e') {
            let (value, next) = parse_value(rest)?;
            items.push(value);
            rest = next;
        }
        Ok((BValue::List(items), &rest[1..]))
    }

    fn parse_dict(input: &[u8]) -> Result<(BValue, &[u8]), String> {
        let mut rest = &input[1..];
        let mut map = BTreeMap::new();
        while rest.first() != Some(&b'e') {
            let (key, next) = parse_bytes(rest)?;
            let BValue::Bytes(key) = key else {
                return Err("dict key must be bytes".into());
            };
            let (value, next) = parse_value(next)?;
            map.insert(key, value);
            rest = next;
        }
        Ok((BValue::Dict(map), &rest[1..]))
    }
}

#[derive(Debug)]
enum BValue {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<BValue>),
    Dict(std::collections::BTreeMap<Vec<u8>, BValue>),
}

impl BValue {
    fn as_dict(&self) -> Option<&std::collections::BTreeMap<Vec<u8>, BValue>> {
        match self {
            Self::Dict(map) => Some(map),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Bytes(bytes) => std::str::from_utf8(bytes).ok(),
            _ => None,
        }
    }

    fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    fn as_list(&self) -> Option<&[BValue]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }
}

pub fn watch_delay() -> Duration {
    Duration::from_secs(2)
}

pub fn is_fresh(path: &Path, now: SystemTime) -> bool {
    path.metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .map(|stamp| {
            now.duration_since(stamp).unwrap_or_default() < Duration::from_secs(60 * 60 * 24 * 30)
        })
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn benc_str(value: &str) -> Vec<u8> {
        let mut out = format!("{}:", value.len()).into_bytes();
        out.extend_from_slice(value.as_bytes());
        out
    }

    #[test]
    fn magnet_extracts_web_seed_and_hash() {
        let meta =
            parse_magnet("magnet:?xt=urn:btih:abc123&dn=Demo&ws=http://cdn.test/file.bin").unwrap();
        assert_eq!(meta.name, "Demo");
        assert_eq!(meta.web_seeds, vec!["http://cdn.test/file.bin"]);
        assert_eq!(meta.info_hash, "abc123");
        let filtered = parse_magnet(
            "magnet:?xt=urn:btih:abc123&ws=javascript:alert(1)&ws=http://cdn.test/ok.bin",
        )
        .unwrap();
        assert_eq!(filtered.web_seeds, vec!["http://cdn.test/ok.bin"]);
    }

    #[test]
    fn torrent_selection_rejects_escape_and_requires_one_file() {
        let meta = TorrentMeta {
            name: "demo".into(),
            magnet: false,
            web_seeds: Vec::new(),
            info_hash: String::new(),
            announce: Vec::new(),
            hint_peers: Vec::new(),
            piece_length: 16,
            pieces: Vec::new(),
            length: 5,
            files: vec![
                TorrentFileEntry {
                    index: 0,
                    path: "one.bin".into(),
                    size: 3,
                    offset: 0,
                },
                TorrentFileEntry {
                    index: 1,
                    path: "dir/two.bin".into(),
                    size: 2,
                    offset: 3,
                },
            ],
        };
        let all = validate_torrent_selection(&meta, &[]).unwrap();
        assert_eq!(all.len(), 2);
        assert!(validate_torrent_selection(
            &meta,
            &[TorrentFileSelection {
                index: 9,
                path: "missing".into(),
                selected: true
            }]
        )
        .is_err());
        assert!(validate_torrent_selection(
            &meta,
            &[TorrentFileSelection {
                index: 0,
                path: "../escape".into(),
                selected: true
            }]
        )
        .is_err());
        assert!(validate_torrent_selection(
            &meta,
            &[TorrentFileSelection {
                index: 0,
                path: "one.bin".into(),
                selected: false
            }]
        )
        .is_err());
    }

    #[test]
    fn selected_files_materialize_without_unselected_paths() {
        let root = std::env::temp_dir().join(format!("hls-torrent-select-{}", std::process::id()));
        let payload = root.join("payload");
        let destination = root.join("published");
        fs::create_dir_all(&root).unwrap();
        fs::write(&payload, b"abcde").unwrap();
        let meta = TorrentMeta {
            name: "demo".into(),
            magnet: false,
            web_seeds: Vec::new(),
            info_hash: String::new(),
            announce: Vec::new(),
            hint_peers: Vec::new(),
            piece_length: 16,
            pieces: Vec::new(),
            length: 5,
            files: vec![
                TorrentFileEntry {
                    index: 0,
                    path: "one.bin".into(),
                    size: 3,
                    offset: 0,
                },
                TorrentFileEntry {
                    index: 1,
                    path: "dir/two.bin".into(),
                    size: 2,
                    offset: 3,
                },
            ],
        };
        let total = materialize_selected_files(
            &payload,
            &destination,
            &meta,
            &[TorrentFileSelection {
                index: 1,
                path: "dir/two.bin".into(),
                selected: true,
            }],
        )
        .unwrap();
        assert_eq!(total, 2);
        assert_eq!(fs::read(destination.join("dir/two.bin")).unwrap(), b"de");
        assert!(!destination.join("one.bin").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn magnet_keeps_peer_hints() {
        let meta = parse_magnet(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&x.pe=10.0.0.8:6881",
        )
        .unwrap();
        assert_eq!(meta.hint_peers, vec!["10.0.0.8:6881"]);
        assert_eq!(
            resolve_hint_peers(&meta.hint_peers)[0].to_string(),
            "10.0.0.8:6881"
        );
        assert!(!DEFAULT_TRACKERS.is_empty());
    }

    #[test]
    fn krpc_get_peers_roundtrip_compact_values_and_nodes() {
        let node_id = [1u8; 20];
        let info_hash = [2u8; 20];
        let query = krpc_get_peers_query(b"aa", &node_id, &info_hash);
        let parsed = bencode::parse(&query).unwrap();
        let dict = parsed.as_dict().unwrap();
        assert_eq!(
            dict.get(b"q".as_ref()).and_then(BValue::as_str),
            Some("get_peers")
        );
        let compact = vec![10, 0, 0, 1, 0x1A, 0xE1];
        let mut node = vec![0u8; 20];
        node.extend_from_slice(&[192, 168, 1, 9, 0x1A, 0xE9]);
        let mut reply = b"d1:rd2:id20:".to_vec();
        reply.extend_from_slice(&node_id);
        reply.extend_from_slice(b"5:nodes26:");
        reply.extend_from_slice(&node);
        reply.extend_from_slice(b"6:valuesl6:");
        reply.extend_from_slice(&compact);
        reply.extend_from_slice(b"ee1:t2:aa1:y1:re");
        let (peers, nodes) = parse_krpc_peers(&reply);
        assert_eq!(peers[0].to_string(), "10.0.0.1:6881");
        assert_eq!(nodes[0].to_string(), "192.168.1.9:6889");
    }

    #[test]
    fn canonicalizes_hex_and_base32_info_hash() {
        assert_eq!(
            canonical_info_hash("0123456789abcdef0123456789abcdef01234567").as_deref(),
            Some("0123456789ABCDEF0123456789ABCDEF01234567")
        );
        let decoded = decode_base32("MFRGGZDFMZTWQ2LK").unwrap();
        assert!(!decoded.is_empty());
    }

    #[test]
    fn watch_folder_reports_new_torrent_once() {
        let dir = std::env::temp_dir().join(format!("hls-watch-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let torrent = dir.join("demo.torrent");
        fs::write(&torrent, b"d4:infod4:name4:demeee").unwrap();
        let mut watch = TorrentWatch::default();
        let first = watch.scan(&dir).unwrap();
        assert_eq!(first.len(), 1);
        assert!(watch.scan(&dir).unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn watch_folder_prime_records_existing_without_reporting() {
        let dir = std::env::temp_dir().join(format!("hls-watch-prime-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("old.torrent");
        fs::write(&existing, b"d4:infod4:name3:oldeee").unwrap();
        let mut watch = TorrentWatch::default();
        watch.prime(&dir).unwrap();
        assert!(watch.scan(&dir).unwrap().is_empty());
        let fresh = dir.join("new.torrent");
        fs::write(&fresh, b"d4:infod4:name3:neweee").unwrap();
        let reported = watch.scan(&dir).unwrap();
        assert_eq!(reported, vec![fresh]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn torrent_file_hashes_info_dict() {
        let mut bytes = b"d".to_vec();
        bytes.extend(benc_str("announce"));
        bytes.extend(benc_str("http://tracker.test/announce"));
        bytes.extend(benc_str("info"));
        bytes.push(b'd');
        bytes.extend(benc_str("name"));
        bytes.extend(benc_str("demo"));
        bytes.extend(b"ee");
        let meta = parse_torrent_file(&bytes).unwrap();
        assert_eq!(meta.name, "demo");
        assert_eq!(meta.info_hash.len(), 40);
    }

    #[test]
    fn compact_peers_decode_six_byte_rows() {
        let peers = parse_compact_peers(&[10, 0, 0, 1, 0x1A, 0xE1]);
        assert_eq!(peers[0].to_string(), "10.0.0.1:6881");
    }

    #[test]
    fn swarm_fetches_verified_piece_from_loopback_peer() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let payload = b"hello-swarm-piece";
        let hash = crate::crypto_lite::sha1(payload);
        let mut torrent = format!(
            "d8:announce17:http://127.0.0.1/4:infod6:lengthi{}e4:name4:demo12:piece lengthi{}e6:pieces20:",
            payload.len(),
            payload.len()
        )
        .into_bytes();
        torrent.extend_from_slice(&hash);
        torrent.extend_from_slice(b"ee");
        let meta = parse_torrent_file(&torrent).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut hs = [0u8; 68];
            stream.read_exact(&mut hs).unwrap();
            stream.write_all(&hs).unwrap();
            stream.write_all(&1u32.to_be_bytes()).unwrap();
            stream.write_all(&[1u8]).unwrap();
            loop {
                let mut header = [0u8; 4];
                stream.read_exact(&mut header).unwrap();
                let len = u32::from_be_bytes(header) as usize;
                let mut msg = vec![0u8; len];
                stream.read_exact(&mut msg).unwrap();
                if msg.first() == Some(&6) {
                    let mut body = vec![7u8];
                    body.extend_from_slice(&0u32.to_be_bytes());
                    body.extend_from_slice(&0u32.to_be_bytes());
                    body.extend_from_slice(payload);
                    stream
                        .write_all(&(body.len() as u32).to_be_bytes())
                        .unwrap();
                    stream.write_all(&body).unwrap();
                    return;
                }
            }
        });
        let dir = std::env::temp_dir().join(format!("hls-swarm-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let output = dir.join("demo.bin");
        let control = dir.join("control");
        fs::write(&control, "run").unwrap();
        download_from_peer(addr, &meta, &output, &control).unwrap();
        assert_eq!(fs::read(&output).unwrap(), payload);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ut_pex_decodes_added_compact_peers() {
        let mut payload = b"d5:added6:".to_vec();
        payload.extend_from_slice(&[10, 0, 0, 2, 0x1A, 0xE1]);
        payload.push(b'e');
        let peers = parse_ut_pex(&payload);
        assert_eq!(peers[0].to_string(), "10.0.0.2:6881");
    }

    #[test]
    fn swarm_skips_already_hashed_piece() {
        let payload = b"already-on-disk!";
        let hash = crate::crypto_lite::sha1(payload);
        let mut torrent = format!(
            "d8:announce17:http://127.0.0.1/4:infod6:lengthi{}e4:name4:demo12:piece lengthi{}e6:pieces20:",
            payload.len(),
            payload.len()
        )
        .into_bytes();
        torrent.extend_from_slice(&hash);
        torrent.extend_from_slice(b"ee");
        let meta = parse_torrent_file(&torrent).unwrap();
        assert_eq!(meta.length, payload.len() as u64);
        assert_eq!(meta.pieces[0], hash);
        let dir = std::env::temp_dir().join(format!("hls-swarm-resume-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let output = dir.join("demo.bin");
        fs::write(&output, payload).unwrap();
        let control = dir.join("control");
        fs::write(&control, "run").unwrap();
        let dummy = "127.0.0.1:1".parse().unwrap();
        download_from_peer(dummy, &meta, &output, &control).unwrap();
        assert_eq!(fs::read(&output).unwrap(), payload);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn multifile_swarm_resumes_without_refetching_and_materializes_selection() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;

        fn serve(
            listener: TcpListener,
            payload: Vec<u8>,
            piece_length: usize,
            stop_before_piece: Option<u32>,
            requested: mpsc::Sender<Vec<u32>>,
        ) {
            let (mut stream, _) = listener.accept().unwrap();
            let mut handshake = [0u8; 68];
            stream.read_exact(&mut handshake).unwrap();
            stream.write_all(&handshake).unwrap();
            stream.write_all(&1u32.to_be_bytes()).unwrap();
            stream.write_all(&[1u8]).unwrap();
            let mut indexes = Vec::new();
            loop {
                let mut header = [0u8; 4];
                if stream.read_exact(&mut header).is_err() {
                    let _ = requested.send(indexes);
                    return;
                }
                let len = u32::from_be_bytes(header) as usize;
                let mut message = vec![0u8; len];
                if stream.read_exact(&mut message).is_err() {
                    let _ = requested.send(indexes);
                    return;
                }
                if message.first() != Some(&6) || message.len() < 13 {
                    continue;
                }
                let index = u32::from_be_bytes(message[1..5].try_into().unwrap());
                indexes.push(index);
                if stop_before_piece == Some(index) {
                    requested.send(indexes).unwrap();
                    return;
                }
                let begin = u32::from_be_bytes(message[5..9].try_into().unwrap()) as usize;
                let block = u32::from_be_bytes(message[9..13].try_into().unwrap()) as usize;
                let start = index as usize * piece_length + begin;
                let end = (start + block).min(payload.len());
                let mut body = vec![7u8];
                body.extend_from_slice(&index.to_be_bytes());
                body.extend_from_slice(&(begin as u32).to_be_bytes());
                body.extend_from_slice(&payload[start..end]);
                stream
                    .write_all(&(body.len() as u32).to_be_bytes())
                    .unwrap();
                stream.write_all(&body).unwrap();
                if end == payload.len() {
                    requested.send(indexes).unwrap();
                    return;
                }
            }
        }

        let payload = b"alphaBRAVO-charlie-DELTA".to_vec();
        let piece_length = 8usize;
        let pieces = payload
            .chunks(piece_length)
            .map(crate::crypto_lite::sha1)
            .collect::<Vec<_>>();
        let meta = TorrentMeta {
            name: "resume-selection".into(),
            magnet: false,
            web_seeds: Vec::new(),
            info_hash: "0123456789abcdef0123456789abcdef01234567".into(),
            announce: Vec::new(),
            hint_peers: Vec::new(),
            piece_length: piece_length as u64,
            pieces,
            length: payload.len() as u64,
            files: vec![
                TorrentFileEntry {
                    index: 0,
                    path: "keep/alpha.bin".into(),
                    size: 5,
                    offset: 0,
                },
                TorrentFileEntry {
                    index: 1,
                    path: "skip/bravo.bin".into(),
                    size: 6,
                    offset: 5,
                },
                TorrentFileEntry {
                    index: 2,
                    path: "keep/charlie-delta.bin".into(),
                    size: payload.len() as u64 - 11,
                    offset: 11,
                },
            ],
        };
        let root = std::env::temp_dir().join(format!(
            "hls-swarm-multifile-resume-{}-{:?}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("payload.part");
        let control = root.join("control");
        fs::write(&control, "run").unwrap();

        let first_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let first_addr = first_listener.local_addr().unwrap();
        let (first_tx, first_rx) = mpsc::channel();
        let first_payload = payload.clone();
        std::thread::spawn(move || {
            serve(
                first_listener,
                first_payload,
                piece_length,
                Some(1),
                first_tx,
            )
        });
        assert!(download_from_peer(first_addr, &meta, &output, &control).is_err());
        assert_eq!(first_rx.recv().unwrap(), vec![0, 1]);
        assert_eq!(
            &fs::read(&output).unwrap()[..piece_length],
            &payload[..piece_length]
        );

        let second_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let second_addr = second_listener.local_addr().unwrap();
        let (second_tx, second_rx) = mpsc::channel();
        let second_payload = payload.clone();
        std::thread::spawn(move || {
            serve(
                second_listener,
                second_payload,
                piece_length,
                None,
                second_tx,
            )
        });
        download_from_peer(second_addr, &meta, &output, &control).unwrap();
        assert_eq!(second_rx.recv().unwrap(), vec![1, 2]);
        assert_eq!(fs::read(&output).unwrap(), payload);

        let destination = root.join("published");
        let selected = [
            TorrentFileSelection {
                index: 0,
                path: "keep/alpha.bin".into(),
                selected: true,
            },
            TorrentFileSelection {
                index: 2,
                path: "keep/charlie-delta.bin".into(),
                selected: true,
            },
        ];
        let written = materialize_selected_files(&output, &destination, &meta, &selected).unwrap();
        assert_eq!(written, meta.files[0].size + meta.files[2].size);
        assert_eq!(
            fs::read(destination.join("keep/alpha.bin")).unwrap(),
            b"alpha"
        );
        assert_eq!(
            fs::read(destination.join("keep/charlie-delta.bin")).unwrap(),
            &payload[11..]
        );
        assert!(!destination.join("skip/bravo.bin").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn udp_tracker_url_and_connect_packet() {
        assert_eq!(
            parse_udp_tracker("udp://tracker.example:1337/announce"),
            Some(("tracker.example".into(), 1337))
        );
        let packet = udp_connect_request(0xAABBCCDD);
        assert_eq!(&packet[..8], &0x4172_7101_980u64.to_be_bytes());
        assert_eq!(&packet[8..12], &0u32.to_be_bytes());
        assert_eq!(&packet[12..], &0xAABBCCDDu32.to_be_bytes());
    }

    #[test]
    fn ut_metadata_rebuilds_info_from_loopback_peer() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let payload = b"hello-metadata";
        let hash = crate::crypto_lite::sha1(payload);
        let mut info = format!(
            "d6:lengthi{}e4:name4:demo12:piece lengthi{}e6:pieces20:",
            payload.len(),
            payload.len()
        )
        .into_bytes();
        info.extend_from_slice(&hash);
        info.push(b'e');
        let info_hash = crate::crypto_lite::sha1_hex(&info);
        let magnet = parse_magnet(&format!(
            "magnet:?xt=urn:btih:{info_hash}&dn=Demo&tr=http://127.0.0.1/announce"
        ))
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let served = info.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut hs = [0u8; 68];
            stream.read_exact(&mut hs).unwrap();
            stream.write_all(&hs).unwrap();
            let mut handshake = Vec::from(*b"d1:md11:ut_metadatai3ee13:metadata_sizei");
            handshake.extend_from_slice(format!("{}ee", served.len()).as_bytes());
            let mut msg = vec![20u8, 0];
            msg.extend_from_slice(&handshake);
            stream.write_all(&(msg.len() as u32).to_be_bytes()).unwrap();
            stream.write_all(&msg).unwrap();
            loop {
                let mut header = [0u8; 4];
                if stream.read_exact(&mut header).is_err() {
                    return;
                }
                let len = u32::from_be_bytes(header) as usize;
                let mut body = vec![0u8; len];
                if stream.read_exact(&mut body).is_err() {
                    return;
                }
                if body.first() != Some(&20) || body.get(1) != Some(&3) {
                    continue;
                }
                let mut reply = vec![20u8, 1];
                reply.extend_from_slice(b"d8:msg_typei1e5:piecei0ee");
                reply.extend_from_slice(&served);
                stream
                    .write_all(&(reply.len() as u32).to_be_bytes())
                    .unwrap();
                stream.write_all(&reply).unwrap();
                return;
            }
        });
        let parsed = fetch_ut_metadata_from_peer(addr, &magnet, &mut Vec::new()).unwrap();
        assert_eq!(parsed.name, "demo");
        assert_eq!(parsed.length, payload.len() as u64);
        assert_eq!(parsed.pieces.len(), 1);
        assert_eq!(
            parsed.info_hash.to_ascii_uppercase(),
            info_hash.to_ascii_uppercase()
        );
    }

    #[test]
    fn torrent_session_trait_is_the_core_entry() {
        let engine = crate::torrent_session();
        let _session: &dyn crate::TorrentSession = &engine;
        assert_eq!(
            std::any::type_name_of_val(&engine),
            std::any::type_name::<crate::BuiltinTorrentEngine>()
        );
        let err = engine
            .download(
                "not-a-torrent",
                std::path::Path::new("nul"),
                std::path::Path::new("nul"),
                &std::collections::HashMap::new(),
                "",
            )
            .unwrap_err();
        assert!(err.contains("unsupported"));
        assert!(
            !err.to_ascii_lowercase().contains("libtorrent"),
            "BT backend must not pretend to be libtorrent: {err}"
        );
    }

    #[test]
    fn torrent_options_keep_v3_limits_without_enabling_seeding() {
        let defaults = TorrentOptions::default();
        assert_eq!(defaults.upload_limit_kib, 1024);
        assert_eq!(defaults.max_connections, 200);
        assert!(defaults.enable_dht);
        let _: BuiltinTorrentEngine = torrent_session();
    }

    #[test]
    fn peer_bitfield_reports_seed_only_when_every_piece_is_available() {
        let mut pieces = vec![false; 10];
        note_peer_availability(5, &[0xff, 0xc0], &mut pieces);
        assert!(pieces.iter().all(|available| *available));

        let mut partial = vec![false; 10];
        note_peer_availability(5, &[0x80, 0x00], &mut partial);
        note_peer_availability(4, &3u32.to_be_bytes(), &mut partial);
        assert!(partial[0]);
        assert!(partial[3]);
        assert!(!partial.iter().all(|available| *available));
    }

    #[test]
    fn piece_selection_sidecar_skips_unselected_file_ranges() {
        let meta = TorrentMeta {
            name: "demo".into(),
            magnet: false,
            web_seeds: Vec::new(),
            info_hash: String::new(),
            announce: Vec::new(),
            hint_peers: Vec::new(),
            piece_length: 4,
            pieces: vec![[0; 20], [0; 20]],
            length: 8,
            files: vec![
                TorrentFileEntry {
                    index: 0,
                    path: "one.bin".into(),
                    size: 4,
                    offset: 0,
                },
                TorrentFileEntry {
                    index: 1,
                    path: "two.bin".into(),
                    size: 4,
                    offset: 4,
                },
            ],
        };
        let path = std::env::temp_dir().join(format!(
            "hls-v7-piece-selection-{}.json",
            std::process::id()
        ));
        fs::write(
            &path,
            serde_json::to_vec(&vec![
                TorrentFileSelection {
                    index: 0,
                    path: "one.bin".into(),
                    selected: false,
                },
                TorrentFileSelection {
                    index: 1,
                    path: "two.bin".into(),
                    selected: true,
                },
            ])
            .unwrap(),
        )
        .unwrap();

        assert!(!piece_is_selected(&meta, 0, &path));
        assert!(piece_is_selected(&meta, 1, &path));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn swarm_cancels_an_inflight_block_when_selection_changes() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;

        let payload = b"aaaabbbb".to_vec();
        let meta = TorrentMeta {
            name: "cancel-selection".into(),
            magnet: false,
            web_seeds: Vec::new(),
            info_hash: "0123456789abcdef0123456789abcdef01234567".into(),
            announce: Vec::new(),
            hint_peers: Vec::new(),
            piece_length: 4,
            pieces: payload.chunks(4).map(crate::crypto_lite::sha1).collect(),
            length: payload.len() as u64,
            files: vec![
                TorrentFileEntry {
                    index: 0,
                    path: "one.bin".into(),
                    size: 4,
                    offset: 0,
                },
                TorrentFileEntry {
                    index: 1,
                    path: "two.bin".into(),
                    size: 4,
                    offset: 4,
                },
            ],
        };
        let root = std::env::temp_dir().join(format!(
            "hls-v7-swarm-cancel-selection-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("payload.bin");
        let control = root.join("control");
        let selection = root.join("selection.json");
        fs::write(&control, "run").unwrap();
        let write_selection = |first: bool, second: bool| {
            fs::write(
                &selection,
                serde_json::to_vec(&vec![
                    TorrentFileSelection {
                        index: 0,
                        path: "one.bin".into(),
                        selected: first,
                    },
                    TorrentFileSelection {
                        index: 1,
                        path: "two.bin".into(),
                        selected: second,
                    },
                ])
                .unwrap(),
            )
            .unwrap();
        };
        write_selection(true, false);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (requested_tx, requested_rx) = mpsc::channel();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let server_payload = payload.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut handshake = [0u8; 68];
            stream.read_exact(&mut handshake).unwrap();
            stream.write_all(&handshake).unwrap();
            stream.write_all(&1u32.to_be_bytes()).unwrap();
            stream.write_all(&[1u8]).unwrap();
            loop {
                let mut header = [0u8; 4];
                if stream.read_exact(&mut header).is_err() {
                    break;
                }
                let mut message = vec![0u8; u32::from_be_bytes(header) as usize];
                if stream.read_exact(&mut message).is_err() || message.is_empty() {
                    break;
                }
                if message[0] != 6 && message[0] != 8 {
                    continue;
                }
                let index = u32::from_be_bytes(message[1..5].try_into().unwrap()) as usize;
                let begin = u32::from_be_bytes(message[5..9].try_into().unwrap()) as usize;
                let block = u32::from_be_bytes(message[9..13].try_into().unwrap()) as usize;
                if message[0] == 8 {
                    cancel_tx.send((index, begin, block)).unwrap();
                    continue;
                }
                if index == 0 {
                    requested_tx.send(()).unwrap();
                    continue;
                }
                let start = index * 4 + begin;
                let mut response = vec![7u8];
                response.extend_from_slice(&(index as u32).to_be_bytes());
                response.extend_from_slice(&(begin as u32).to_be_bytes());
                response.extend_from_slice(&server_payload[start..start + block]);
                stream
                    .write_all(&(response.len() as u32).to_be_bytes())
                    .unwrap();
                stream.write_all(&response).unwrap();
            }
        });

        let selection_for_update = selection.clone();
        let updater = std::thread::spawn(move || {
            requested_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            fs::write(
                selection_for_update,
                serde_json::to_vec(&vec![
                    TorrentFileSelection {
                        index: 0,
                        path: "one.bin".into(),
                        selected: false,
                    },
                    TorrentFileSelection {
                        index: 1,
                        path: "two.bin".into(),
                        selected: true,
                    },
                ])
                .unwrap(),
            )
            .unwrap();
        });

        let mut telemetry = TorrentTelemetry::default();
        download_from_peer_ex_with_telemetry(
            addr,
            &meta,
            &output,
            &control,
            &mut Vec::new(),
            &mut telemetry,
            &mut |_| {},
            &selection,
        )
        .unwrap();
        updater.join().unwrap();
        assert_eq!(
            cancel_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            (0, 0, 4)
        );
        let written = fs::read(&output).unwrap();
        assert_eq!(&written[..4], &[0, 0, 0, 0]);
        assert_eq!(&written[4..], b"bbbb");
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }
}
