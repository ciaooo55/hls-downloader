//! LAN-restricted media push (DLNA SSDP search + AVTransport).

use crate::playback::MediaServer;
use crate::CastDeviceInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowserPush {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub message: String,
    pub location: String,
}

fn media_server() -> Result<&'static MediaServer, String> {
    static SERVER: OnceLock<Result<MediaServer, String>> = OnceLock::new();
    match SERVER.get_or_init(MediaServer::start) {
        Ok(server) => Ok(server),
        Err(error) => Err(error.clone()),
    }
}

fn pushes() -> &'static Mutex<HashMap<String, BrowserPush>> {
    static MAP: OnceLock<Mutex<HashMap<String, BrowserPush>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn device_cache() -> &'static Mutex<Vec<CastDeviceInfo>> {
    static CACHE: OnceLock<Mutex<Vec<CastDeviceInfo>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn start_browser_push(kind: &str, url: &str, title: &str) -> Result<BrowserPush, String> {
    let url = url.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("浏览器投送请求无效".into());
    }
    if url.contains('\r') || url.contains('\n') {
        return Err("浏览器投送地址无效".into());
    }
    let kind = if kind == "tvbox" || kind == "push_to_tv" {
        "tvbox"
    } else {
        "cast"
    };
    let server = media_server()?;
    server.enable_lan();
    let id = format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let token = crate::playback::random_mount_token();
    server.mount_remote(&token, url.to_string());
    let host = primary_lan_ipv4()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".into());
    let location = if kind == "tvbox" {
        format!("http://{host}:{}/tvbox/{token}", server.bound_port())
    } else {
        lan_media_url(server, &token, &host)?
    };
    if kind == "cast" {
        let _ = ssdp_notify(&location);
    }
    let push = BrowserPush {
        id,
        kind: kind.to_string(),
        status: "ready".into(),
        message: if title.trim().is_empty() {
            "已在局域网发布播放地址".into()
        } else {
            format!("已发布：{}", title.trim())
        },
        location,
    };
    if let Ok(mut map) = pushes().lock() {
        map.insert(push.id.clone(), push.clone());
    }
    Ok(push)
}

pub fn browser_push_status(id: &str) -> Option<BrowserPush> {
    pushes().lock().ok()?.get(id).cloned()
}

pub fn lan_media_url(server: &MediaServer, token: &str, advertise_host: &str) -> Result<String, String> {
    if advertise_host == "127.0.0.1" || advertise_host == "localhost" {
        return Ok(server.url_for(token));
    }
    if !is_lan_host(advertise_host) {
        return Err("cast host must be a private LAN address".into());
    }
    Ok(format!(
        "http://{advertise_host}:{}/media/{token}",
        server.bound_port()
    ))
}

pub fn is_lan_host(host: &str) -> bool {
    if let Ok(addr) = host.parse::<Ipv4Addr>() {
        addr.is_private() || addr.is_loopback() || addr.is_link_local()
    } else {
        false
    }
}

pub fn primary_lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("10.255.255.255:1").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if ip.is_private() || ip.is_link_local() => Some(ip),
        _ => None,
    }
}

pub fn ssdp_notify(location: &str) -> Result<(), String> {
    if location.contains('\r') || location.contains('\n') || !location.starts_with("http://") {
        return Err("投屏通告地址无效".into());
    }
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
    socket
        .set_broadcast(true)
        .map_err(|error| error.to_string())?;
    let body = format!(
        "NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nNT: upnp:rootdevice\r\nNTS: ssdp:alive\r\nLOCATION: {location}\r\nUSN: uuid:hls-downloader::upnp:rootdevice\r\nCACHE-CONTROL: max-age=30\r\n\r\n"
    );
    socket
        .send_to(body.as_bytes(), "239.255.255.250:1900")
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn tvbox_payload(location: &str, title: &str) -> String {
    format!("{{\"url\":\"{location}\",\"title\":\"{}\"}}", title.replace('"', "'"))
}

pub fn cached_devices() -> Vec<CastDeviceInfo> {
    device_cache().lock().map(|items| items.clone()).unwrap_or_default()
}

pub fn remember_devices(devices: Vec<CastDeviceInfo>) {
    if let Ok(mut cache) = device_cache().lock() {
        *cache = devices;
    }
}

pub fn discover_devices(timeout: Duration) -> Result<Vec<CastDeviceInfo>, String> {
    if std::env::var_os("HLS_V6_CAST_NULL").is_some() {
        return Ok(Vec::new());
    }
    let locations = ssdp_search(timeout)?;
    let mut devices = Vec::new();
    for location in locations {
        if let Some(device) = describe_device(&location) {
            devices.push(device);
        }
    }
    devices.extend(discover_chromecasts(timeout));
    devices.sort_by(|left, right| left.label.cmp(&right.label).then(left.id.cmp(&right.id)));
    devices.dedup_by(|left, right| left.id == right.id || left.control_url == right.control_url);
    if let Ok(mut cache) = device_cache().lock() {
        *cache = devices.clone();
    }
    Ok(devices)
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn push_tvbox(endpoint: &str, media_url: &str, _title: &str) -> Result<(), String> {
    let media_url = media_url.trim();
    if !(media_url.starts_with("http://") || media_url.starts_with("https://")) {
        return Err("待推送的视频地址必须是有效的 HTTP(S) 地址".into());
    }
    let endpoint = endpoint.trim().trim_end_matches('/');
    let action = if endpoint.to_ascii_lowercase().ends_with("/action") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/action")
    };
    let (host, port, path) = split_http_url(&action)?;
    if !is_lan_host(&host) {
        return Err("TVBox 地址必须是局域网".into());
    }
    let body = format!("do=push&url={}", percent_encode(media_url));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(SocketAddr::from((
        host.parse::<Ipv4Addr>().map_err(|error| error.to_string())?,
        port,
    )))
    .map_err(|error| format!("连接 TVBox: {error}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(8))).ok();
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok();
    if response.contains("HTTP/1.1 404") || response.contains("HTTP/1.0 404") {
        return Err("电视拒绝了推送".into());
    }
    Ok(())
}

pub fn play_on_device(device_id: &str, media_url: &str, title: &str) -> Result<String, String> {
    let device = cached_devices()
        .into_iter()
        .find(|item| item.id == device_id || item.control_url == device_id)
        .ok_or_else(|| "请先扫描并选择投屏设备".to_string())?;
    if device.service_type == "tvbox" || device.id.starts_with("tvbox:") {
        push_tvbox(&device.control_url, media_url, title)?;
        return Ok(device.label);
    }
    if device.service_type == "chromecast" || device.id.starts_with("chromecast:") {
        chromecast_play(&device.control_url, media_url, title)?;
        return Ok(device.label);
    }
    av_transport_action(&device, "SetAVTransportURI", &[
        ("InstanceID", "0"),
        ("CurrentURI", media_url),
        ("CurrentURIMetaData", &didl_lite(media_url, title)),
    ])?;
    av_transport_action(&device, "Play", &[("InstanceID", "0"), ("Speed", "1")])?;
    remember_last_device(device.clone());
    Ok(device.label)
}

fn last_cast() -> &'static Mutex<Option<CastDeviceInfo>> {
    static LAST: OnceLock<Mutex<Option<CastDeviceInfo>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn remember_last_device(device: CastDeviceInfo) {
    if let Ok(mut guard) = last_cast().lock() {
        *guard = Some(device);
    }
}

pub fn last_device_label() -> String {
    last_cast()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|item| item.label.clone()))
        .unwrap_or_default()
}

pub fn control_session(action: &str) -> Result<(), String> {
    let device = last_cast()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .ok_or_else(|| "当前没有投屏会话".to_string())?;
    match action {
        "play" => av_transport_action(&device, "Play", &[("InstanceID", "0"), ("Speed", "1")]),
        "pause" => av_transport_action(&device, "Pause", &[("InstanceID", "0")]),
        "stop" => av_transport_action(&device, "Stop", &[("InstanceID", "0")]),
        _ => Err(format!("unknown cast action {action}")),
    }
}

pub fn parse_ssdp_location(response: &str) -> Option<String> {
    for line in response.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("location") {
            let location = value.trim();
            if location.starts_with("http://") || location.starts_with("https://") {
                return Some(location.to_string());
            }
        }
    }
    None
}

pub fn parse_device_description(xml: &str, location: &str) -> Option<CastDeviceInfo> {
    let friendly = xml_local(xml, "friendlyName").unwrap_or_else(|| "DLNA 设备".into());
    let url_base = xml_local(xml, "URLBase").unwrap_or_else(|| origin_of(location));
    let mut rest = xml;
    while let Some(start) = rest.find("<service") {
        let after = &rest[start..];
        let end = after.find("</service>").or_else(|| after.find("/>"))?;
        let block = &after[..end];
        let service_type = xml_local(block, "serviceType").unwrap_or_default();
        let control = xml_local(block, "controlURL").unwrap_or_default();
        if service_type.contains("AVTransport") && !control.is_empty() {
            let control_url = resolve_url(&url_base, &control);
            if !is_lan_url(&control_url) {
                return None;
            }
            let host = host_of(&control_url).unwrap_or_default();
            return Some(CastDeviceInfo {
                id: format!("dlna:{control_url}"),
                label: if friendly.trim().is_empty() {
                    host.clone()
                } else {
                    friendly.trim().chars().take(160).collect()
                },
                location: location.to_string(),
                control_url,
                service_type,
            });
        }
        rest = &after[end.saturating_add(1)..];
    }
    None
}

fn ssdp_search(timeout: Duration) -> Result<Vec<String>, String> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
    socket.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let targets = [
        "urn:schemas-upnp-org:device:MediaRenderer:1",
        "urn:schemas-upnp-org:service:AVTransport:1",
        "ssdp:all",
    ];
    for target in targets {
        let body = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: {target}\r\nUSER-AGENT: HLSDownloader/6 UPnP/1.1\r\n\r\n"
        );
        let _ = socket.send_to(body.as_bytes(), "239.255.255.250:1900");
    }
    let mut locations = Vec::new();
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((count, _)) => {
                if let Some(location) = parse_ssdp_location(&String::from_utf8_lossy(&buf[..count])) {
                    if is_lan_url(&location) && !locations.contains(&location) {
                        locations.push(location);
                    }
                }
            }
            Err(_) => continue,
        }
    }
    Ok(locations)
}

fn describe_device(location: &str) -> Option<CastDeviceInfo> {
    if !is_lan_url(location) {
        return None;
    }
    let (_, body) = crate::http_engine::fetch_bytes(location, &HashMap::new(), "").ok()?;
    parse_device_description(&String::from_utf8_lossy(&body), location)
}

fn av_transport_action(
    device: &CastDeviceInfo,
    action: &str,
    args: &[(&str, &str)],
) -> Result<(), String> {
    if !is_lan_url(&device.control_url) {
        return Err("投屏控制地址必须是局域网".into());
    }
    let service = if device.service_type.is_empty() || !soap_name_ok(&device.service_type) {
        "urn:schemas-upnp-org:service:AVTransport:1"
    } else {
        device.service_type.as_str()
    };
    if !soap_name_ok(action) {
        return Err("投屏动作无效".into());
    }
    let mut inner = String::new();
    for (name, value) in args {
        inner.push_str(&format!(
            "<{name}>{}</{name}>",
            xml_escape(value)
        ));
    }
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service}\">{inner}</u:{action}></s:Body></s:Envelope>"
    );
    let soap_action = format!("\"{service}#{action}\"");
    let response = http_post_lan(&device.control_url, &soap_action, &body)?;
    if response.contains("s:Fault") || response.contains("UPnPError") {
        return Err(format!("投屏设备拒绝 {action}"));
    }
    Ok(())
}

fn http_post_lan(url: &str, soap_action: &str, body: &str) -> Result<String, String> {
    let (host, port, path) = split_http_url(url)?;
    if !is_lan_host(&host) {
        return Err("投屏控制地址必须是局域网".into());
    }
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|error| format!("cast address: {error}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(4))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(6)))
        .ok();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\nSOAPACTION: {soap_action}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    Ok(response)
}

fn split_http_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "投屏只允许明文 HTTP 局域网控制".to_string())?;
    if rest.contains('\r') || rest.contains('\n') || rest.contains('\0') {
        return Err("投屏控制地址无效".into());
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = if path.is_empty() {
        "/".into()
    } else {
        format!("/{path}")
    };
    if authority.contains([' ', '\t']) || path.contains([' ', '\t', '\r', '\n']) {
        return Err("投屏控制地址无效".into());
    }
    let (host, port) = if let Some((host, port)) = authority.split_once(':') {
        (
            host.to_string(),
            port.parse::<u16>().unwrap_or(80),
        )
    } else {
        (authority.to_string(), 80)
    };
    Ok((host, port, path))
}

fn didl_lite(url: &str, title: &str) -> String {
    let title = xml_escape(title);
    let url = xml_escape(url);
    format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\"><item id=\"0\" parentID=\"-1\" restricted=\"1\"><dc:title>{title}</dc:title><upnp:class>object.item.videoItem</upnp:class><res protocolInfo=\"http-get:*:video/mp4:*\">{url}</res></item></DIDL-Lite>"
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn soap_name_ok(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '.' | '-' | '_'))
}

fn xml_local(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let start = xml.find(&open)?;
    let after = &xml[start..];
    let inner_start = after.find('>')? + 1;
    let rest = &after[inner_start..];
    let end = rest.find(&close)?;
    Some(rest[..end].trim().to_string())
}

fn origin_of(url: &str) -> String {
    if let Some(scheme) = url.find("://") {
        let after = &url[scheme + 3..];
        let host_end = after.find('/').map(|index| scheme + 3 + index).unwrap_or(url.len());
        url[..host_end].to_string()
    } else {
        url.to_string()
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    Some(rest.split(['/', ':']).next()?.to_string())
}

fn resolve_url(base: &str, reference: &str) -> String {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return reference.to_string();
    }
    if reference.starts_with('/') {
        return format!("{}{reference}", origin_of(base));
    }
    let dir = base.rsplit_once('/').map(|(left, _)| left).unwrap_or(base);
    format!("{dir}/{reference}")
}

fn is_lan_url(url: &str) -> bool {
    host_of(url).is_some_and(|host| is_lan_host(&host))
}

const CAST_NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const CAST_NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const CAST_NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const CAST_NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";
const CAST_APP_DEFAULT_MEDIA: &str = "CC1AD845";

pub fn mdns_googlecast_query() -> Vec<u8> {
    let mut packet = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    packet.extend(dns_labels("_googlecast._tcp.local"));
    packet.extend_from_slice(&12u16.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet
}

pub fn parse_mdns_chromecasts(packet: &[u8]) -> Vec<CastDeviceInfo> {
    if packet.len() < 12 {
        return Vec::new();
    }
    let questions = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let answers = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    let authority = u16::from_be_bytes([packet[8], packet[9]]) as usize;
    let additional = u16::from_be_bytes([packet[10], packet[11]]) as usize;
    let mut offset = 12;
    for _ in 0..questions {
        let Some((_, next)) = dns_read_name(packet, offset) else {
            return Vec::new();
        };
        offset = next.saturating_add(4);
    }
    let mut ptrs = Vec::new();
    let mut srv: HashMap<String, (String, u16)> = HashMap::new();
    let mut txt: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut addrs: HashMap<String, Ipv4Addr> = HashMap::new();
    for _ in 0..(answers + authority + additional) {
        let Some((name, next)) = dns_read_name(packet, offset) else {
            break;
        };
        if next + 10 > packet.len() {
            break;
        }
        let rtype = u16::from_be_bytes([packet[next], packet[next + 1]]);
        let rdlen = u16::from_be_bytes([packet[next + 8], packet[next + 9]]) as usize;
        let data_at = next + 10;
        let data_end = data_at.saturating_add(rdlen);
        if data_end > packet.len() {
            break;
        }
        let data = &packet[data_at..data_end];
        match rtype {
            12 => {
                if let Some((target, _)) = dns_read_name(packet, data_at) {
                    if name.ends_with("_googlecast._tcp.local") {
                        ptrs.push(target);
                    }
                }
            }
            33 if data.len() >= 6 => {
                let port = u16::from_be_bytes([data[4], data[5]]);
                if let Some((target, _)) = dns_read_name(packet, data_at + 6) {
                    srv.insert(name, (target, port));
                }
            }
            16 => {
                txt.insert(name, parse_txt(data));
            }
            1 if data.len() == 4 => {
                addrs.insert(name, Ipv4Addr::new(data[0], data[1], data[2], data[3]));
            }
            _ => {}
        }
        offset = data_end;
    }
    let mut devices = Vec::new();
    for instance in ptrs {
        let Some((target, port)) = srv.get(&instance) else {
            continue;
        };
        let Some(ip) = addrs.get(target) else {
            continue;
        };
        if !(ip.is_private() || ip.is_loopback() || ip.is_link_local()) {
            continue;
        }
        let attrs = txt.get(&instance);
        let id = attrs
            .and_then(|map| map.get("id"))
            .cloned()
            .unwrap_or_else(|| format!("{ip}:{port}"));
        let label = attrs
            .and_then(|map| map.get("fn"))
            .cloned()
            .unwrap_or_else(|| instance.split('.').next().unwrap_or("Chromecast").to_string());
        devices.push(CastDeviceInfo {
            id: format!("chromecast:{id}"),
            label: format!("Chromecast · {label}"),
            location: format!("https://{ip}:{port}"),
            control_url: format!("{ip}:{port}"),
            service_type: "chromecast".into(),
        });
    }
    devices
}

fn parse_txt(data: &[u8]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut at = 0;
    while at < data.len() {
        let len = data[at] as usize;
        at += 1;
        if at + len > data.len() {
            break;
        }
        if let Ok(text) = std::str::from_utf8(&data[at..at + len]) {
            if let Some((key, value)) = text.split_once('=') {
                map.insert(key.to_string(), value.to_string());
            }
        }
        at += len;
    }
    map
}

fn dns_labels(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

fn dns_read_name(packet: &[u8], mut offset: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut jumped = false;
    let mut end = offset;
    for _ in 0..16 {
        let len = *packet.get(offset)? as usize;
        if len == 0 {
            if !jumped {
                end = offset + 1;
            }
            return Some((labels.join("."), end));
        }
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | (*packet.get(offset + 1)? as usize);
            if !jumped {
                end = offset + 2;
                jumped = true;
            }
            offset = ptr;
            continue;
        }
        let label = std::str::from_utf8(packet.get(offset + 1..offset + 1 + len)?).ok()?;
        labels.push(label.to_string());
        offset += 1 + len;
        if !jumped {
            end = offset;
        }
    }
    None
}

fn discover_chromecasts(timeout: Duration) -> Vec<CastDeviceInfo> {
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => socket,
        Err(_) => return Vec::new(),
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = socket.join_multicast_v4(&Ipv4Addr::new(224, 0, 0, 251), &Ipv4Addr::UNSPECIFIED);
    let query = mdns_googlecast_query();
    let _ = socket.send_to(&query, "224.0.0.251:5353");
    let deadline = Instant::now() + timeout;
    let mut devices = Vec::new();
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((count, _)) => {
                for device in parse_mdns_chromecasts(&buf[..count]) {
                    if !devices.iter().any(|item: &CastDeviceInfo| item.id == device.id) {
                        devices.push(device);
                    }
                }
            }
            Err(_) => continue,
        }
    }
    devices
}

pub fn encode_cast_message(namespace: &str, destination: &str, payload: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(proto_varint_field(1, 0));
    body.extend(proto_string_field(2, "sender-0"));
    body.extend(proto_string_field(3, destination));
    body.extend(proto_string_field(4, namespace));
    body.extend(proto_varint_field(5, 0));
    body.extend(proto_string_field(6, payload));
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend(body);
    frame
}

pub fn decode_cast_payload(frame: &[u8]) -> Option<(String, String)> {
    if frame.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes(frame[0..4].try_into().ok()?) as usize;
    let body = frame.get(4..4 + len)?;
    let mut at = 0;
    let mut namespace = String::new();
    let mut payload = String::new();
    while at < body.len() {
        let (tag, next) = proto_read_varint(body, at)?;
        at = next;
        let field = (tag >> 3) as u32;
        let wire = tag & 7;
        if wire == 0 {
            let (_, next) = proto_read_varint(body, at)?;
            at = next;
        } else if wire == 2 {
            let (nlen, next) = proto_read_varint(body, at)?;
            at = next;
            let end = at + nlen as usize;
            let text = std::str::from_utf8(body.get(at..end)?).ok()?.to_string();
            if field == 4 {
                namespace = text;
            } else if field == 6 {
                payload = text;
            }
            at = end;
        } else {
            return None;
        }
    }
    Some((namespace, payload))
}

fn proto_string_field(field: u32, value: &str) -> Vec<u8> {
    let mut out = proto_varint(((field as u64) << 3) | 2);
    let bytes = value.as_bytes();
    out.extend(proto_varint(bytes.len() as u64));
    out.extend_from_slice(bytes);
    out
}

fn proto_varint_field(field: u32, value: u64) -> Vec<u8> {
    let mut out = proto_varint(((field as u64) << 3) | 0);
    out.extend(proto_varint(value));
    out
}

fn proto_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

fn proto_read_varint(buf: &[u8], mut at: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *buf.get(at)?;
        at += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((value, at));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn chromecast_play(endpoint: &str, media_url: &str, title: &str) -> Result<String, String> {
    let (host, port) = endpoint.rsplit_once(':').ok_or_else(|| "Chromecast 地址无效".to_string())?;
    let port: u16 = port.parse().map_err(|_| "Chromecast 端口无效".to_string())?;
    if !is_lan_host(host) {
        return Err("Chromecast 必须在局域网".into());
    }
    #[cfg(windows)]
    {
        chromecast_play_windows(host, port, media_url, title)
    }
    #[cfg(not(windows))]
    {
        let _ = (port, media_url, title);
        Err("Chromecast 投屏使用 Windows Schannel".into())
    }
}

#[cfg(windows)]
fn chromecast_play_windows(host: &str, port: u16, media_url: &str, title: &str) -> Result<String, String> {
    let raw = TcpStream::connect_timeout(
        &SocketAddr::from((host.parse::<Ipv4Addr>().map_err(|error| error.to_string())?, port)),
        Duration::from_secs(4),
    )
    .map_err(|error| error.to_string())?;
    raw.set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let cred = schannel::schannel_cred::SchannelCred::builder()
        .acquire(schannel::schannel_cred::Direction::Outbound)
        .map_err(|error| error.to_string())?;
    let mut stream = schannel::tls_stream::Builder::new()
        .domain(host)
        .verify_callback(|_| Ok(()))
        .connect(cred, raw)
        .map_err(|error| error.to_string())?;
    write_cast(
        &mut stream,
        CAST_NS_CONNECTION,
        "receiver-0",
        r#"{"type":"CONNECT"}"#,
    )?;
    write_cast(
        &mut stream,
        CAST_NS_RECEIVER,
        "receiver-0",
        r#"{"type":"GET_STATUS","requestId":1}"#,
    )?;
    let mut transport = String::new();
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut launched = false;
    while Instant::now() < deadline {
        let Some((namespace, payload)) = read_cast(&mut stream)? else {
            continue;
        };
        if namespace == CAST_NS_HEARTBEAT && payload.contains("PING") {
            write_cast(&mut stream, CAST_NS_HEARTBEAT, "receiver-0", r#"{"type":"PONG"}"#)?;
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) {
            if value.get("type").and_then(|item| item.as_str()) == Some("RECEIVER_STATUS") {
                if let Some(app) = value
                    .pointer("/status/applications/0/transportId")
                    .and_then(|item| item.as_str())
                {
                    transport = app.to_string();
                    break;
                }
                if !launched {
                    launched = true;
                    let body = serde_json::json!({
                        "type": "LAUNCH",
                        "appId": CAST_APP_DEFAULT_MEDIA,
                        "requestId": 2
                    })
                    .to_string();
                    write_cast(&mut stream, CAST_NS_RECEIVER, "receiver-0", &body)?;
                }
            }
        }
    }
    if transport.is_empty() {
        return Err("Chromecast 没有返回会话".into());
    }
    write_cast(
        &mut stream,
        CAST_NS_CONNECTION,
        &transport,
        r#"{"type":"CONNECT"}"#,
    )?;
    let mime = chromecast_mime(media_url, title);
    let stream_type = if mime.contains("mpegurl") || mime.contains("dash") {
        "LIVE"
    } else {
        "BUFFERED"
    };
    let load = serde_json::json!({
        "type": "LOAD",
        "requestId": 3,
        "autoplay": true,
        "currentTime": 0,
        "media": {
            "contentId": media_url,
            "streamType": stream_type,
            "contentType": mime,
            "metadata": {
                "metadataType": 0,
                "title": title
            }
        }
    })
    .to_string();
    write_cast(&mut stream, CAST_NS_MEDIA, &transport, &load)?;
    Ok(host.to_string())
}

fn chromecast_mime(media_url: &str, title: &str) -> &'static str {
    let leaf = format!("{title} {media_url}").to_ascii_lowercase();
    if leaf.contains(".m3u8") || leaf.contains("mpegurl") {
        "application/vnd.apple.mpegurl"
    } else if leaf.contains(".mpd") {
        "application/dash+xml"
    } else if leaf.contains(".webm") {
        "video/webm"
    } else if leaf.contains(".mkv") {
        "video/x-matroska"
    } else {
        "video/mp4"
    }
}

fn write_cast<W: Write>(stream: &mut W, namespace: &str, destination: &str, payload: &str) -> Result<(), String> {
    stream
        .write_all(&encode_cast_message(namespace, destination, payload))
        .map_err(|error| error.to_string())
}

fn read_cast<R: Read>(stream: &mut R) -> Result<Option<(String, String)>, String> {
    let mut header = [0u8; 4];
    if stream.read_exact(&mut header).is_err() {
        return Ok(None);
    }
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 || len > 64 * 1024 {
        return Err("Chromecast 帧过长".into());
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).map_err(|error| error.to_string())?;
    let mut frame = header.to_vec();
    frame.extend(body);
    Ok(decode_cast_payload(&frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_public_cast_hosts() {
        assert!(is_lan_host("192.168.1.8"));
        assert!(is_lan_host("10.0.0.2"));
        assert!(!is_lan_host("8.8.8.8"));
        let server = MediaServer::start().unwrap();
        assert!(lan_media_url(&server, "t", "8.8.8.8").is_err());
        if let Some(ip) = primary_lan_ipv4() {
            assert!(ip.is_private() || ip.is_link_local());
        }
    }

    #[test]
    fn parses_ssdp_location_header() {
        let response = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=100\r\nLOCATION: http://192.168.1.20:8008/desc.xml\r\nST: urn:schemas-upnp-org:service:AVTransport:1\r\n\r\n";
        assert_eq!(
            parse_ssdp_location(response).as_deref(),
            Some("http://192.168.1.20:8008/desc.xml")
        );
        assert!(parse_ssdp_location("LOCATION: http://8.8.8.8/desc.xml").is_some());
        assert!(!is_lan_url("http://8.8.8.8/desc.xml"));
    }

    #[test]
    fn parses_avtransport_device_xml() {
        let xml = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <device>
    <friendlyName>客厅电视</friendlyName>
    <URLBase>http://192.168.1.20:8008/</URLBase>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType>
        <controlURL>/upnp/control/AVTransport</controlURL>
      </service>
    </serviceList>
  </device>
</root>"#;
        let device = parse_device_description(xml, "http://192.168.1.20:8008/desc.xml").unwrap();
        assert_eq!(device.label, "客厅电视");
        assert_eq!(device.control_url, "http://192.168.1.20:8008/upnp/control/AVTransport");
        assert!(device.service_type.contains("AVTransport"));
    }

    #[test]
    fn soap_envelope_escapes_media_url() {
        let xml = didl_lite("http://10.0.0.2/media/a&b", "A <B>");
        assert!(xml.contains("&amp;"));
        assert!(xml.contains("&lt;"));
        assert!(split_http_url("http://192.168.1.8/av\r\nHost: x").is_err());
        assert!(ssdp_notify("http://10.0.0.2/\r\nLOCATION: http://evil").is_err());
        assert!(soap_name_ok("urn:schemas-upnp-org:service:AVTransport:1"));
        assert!(!soap_name_ok("urn:x\"><evil"));
    }

    #[test]
    fn parses_mdns_googlecast_records() {
        let mut packet = vec![0, 0, 0x84, 0, 0, 0, 0, 3, 0, 0, 0, 0];
        packet.extend(dns_labels("_googlecast._tcp.local"));
        packet.extend_from_slice(&12u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&0u32.to_be_bytes());
        let ptr = dns_labels("Kitchen._googlecast._tcp.local");
        packet.extend_from_slice(&(ptr.len() as u16).to_be_bytes());
        packet.extend(ptr);
        packet.extend(dns_labels("Kitchen._googlecast._tcp.local"));
        packet.extend_from_slice(&33u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&0u32.to_be_bytes());
        let mut srv = vec![0, 0, 0, 0, 0x1F, 0x49];
        srv.extend(dns_labels("Kitchen.local"));
        packet.extend_from_slice(&(srv.len() as u16).to_be_bytes());
        packet.extend(srv);
        packet.extend(dns_labels("Kitchen.local"));
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&0u32.to_be_bytes());
        packet.extend_from_slice(&4u16.to_be_bytes());
        packet.extend_from_slice(&[192, 168, 1, 20]);
        let devices = parse_mdns_chromecasts(&packet);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].service_type, "chromecast");
        assert_eq!(devices[0].control_url, "192.168.1.20:8009");
        assert!(devices[0].label.contains("Kitchen"));
    }

    #[test]
    fn cast_v2_frame_roundtrips_json_payload() {
        let frame = encode_cast_message(
            CAST_NS_MEDIA,
            "web-1",
            r#"{"type":"LOAD","requestId":3}"#,
        );
        let (namespace, payload) = decode_cast_payload(&frame).unwrap();
        assert_eq!(namespace, CAST_NS_MEDIA);
        assert!(payload.contains("LOAD"));
        assert_eq!(chromecast_mime("http://10.0.0.2/local.m3u8", "live"), "application/vnd.apple.mpegurl");
    }
}
