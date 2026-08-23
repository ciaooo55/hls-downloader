//! LAN-restricted media push (DLNA SSDP search + AVTransport).

use crate::playback::MediaServer;
use crate::CastDeviceInfo;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowserPush {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub message: String,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CastPlaybackStatus {
    pub label: String,
    pub device_kind: String,
    pub supported_actions: Vec<String>,
    pub playing: bool,
    pub paused: bool,
    pub position_seconds: u64,
    pub duration_seconds: u64,
    pub position_available: bool,
    pub state: String,
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
    let url = url.trim().trim_start_matches('\u{feff}');
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err("浏览器投送请求无效".into());
    }
    if url.chars().any(|ch| ch.is_control()) {
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

pub fn lan_media_url(
    server: &MediaServer,
    token: &str,
    advertise_host: &str,
) -> Result<String, String> {
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

#[cfg(windows)]
fn lan_ipv4_networks() -> Vec<(Ipv4Addr, u8)> {
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    unsafe fn wide_text(pointer: *const u16) -> String {
        if pointer.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        while len < 512 && unsafe { *pointer.add(len) } != 0 {
            len += 1;
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, len) })
    }

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut bytes = 16 * 1024u32;
    let mut buffer = vec![0u8; bytes as usize];
    let mut status = unsafe {
        GetAdaptersAddresses(
            AF_INET as u32,
            flags,
            std::ptr::null(),
            buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
            &mut bytes,
        )
    };
    if status == ERROR_BUFFER_OVERFLOW {
        buffer.resize(bytes as usize, 0);
        status = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                flags,
                std::ptr::null(),
                buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut bytes,
            )
        };
    }
    if status != NO_ERROR {
        return primary_lan_ipv4()
            .map(|address| vec![(address, 24)])
            .unwrap_or_default();
    }

    let mut networks = Vec::new();
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !adapter.is_null() {
        let item = unsafe { &*adapter };
        let name = unsafe { wide_text(item.FriendlyName) }.to_ascii_lowercase();
        let ignored_name = [
            "virtual", "vpn", "tunnel", "loopback", "mihomo", "wsl", "hyper-v", "虚拟",
        ]
        .iter()
        .any(|marker| name.contains(marker));
        if item.OperStatus == IfOperStatusUp
            && item.IfType != IF_TYPE_SOFTWARE_LOOPBACK
            && item.IfType != IF_TYPE_TUNNEL
            && !ignored_name
        {
            let mut unicast = item.FirstUnicastAddress;
            while !unicast.is_null() {
                let address = unsafe { &*unicast };
                let socket = address.Address.lpSockaddr;
                if !socket.is_null() && unsafe { (*socket).sa_family } == AF_INET {
                    let ipv4 = unsafe { &*(socket.cast::<SOCKADDR_IN>()) };
                    let octets = unsafe { ipv4.sin_addr.S_un.S_un_b };
                    let value = Ipv4Addr::new(octets.s_b1, octets.s_b2, octets.s_b3, octets.s_b4);
                    let prefix = address.OnLinkPrefixLength.min(32);
                    if value.is_private()
                        && !value.is_loopback()
                        && !value.is_link_local()
                        && prefix <= 30
                        && !networks.contains(&(value, prefix))
                    {
                        networks.push((value, prefix));
                    }
                }
                unicast = address.Next;
            }
        }
        adapter = item.Next;
    }
    if networks.is_empty() {
        if let Some(address) = primary_lan_ipv4() {
            networks.push((address, 24));
        }
    }
    networks
}

#[cfg(not(windows))]
fn lan_ipv4_networks() -> Vec<(Ipv4Addr, u8)> {
    primary_lan_ipv4()
        .map(|address| vec![(address, 24)])
        .unwrap_or_default()
}

fn discovery_interface_addresses() -> Vec<Ipv4Addr> {
    let mut addresses = lan_ipv4_networks()
        .into_iter()
        .map(|(address, _)| address)
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        addresses.push(Ipv4Addr::UNSPECIFIED);
    }
    addresses
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
    format!(
        "{{\"url\":\"{location}\",\"title\":\"{}\"}}",
        title.replace('"', "'")
    )
}

pub fn cached_devices() -> Vec<CastDeviceInfo> {
    device_cache()
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default()
}

pub fn remember_devices(devices: Vec<CastDeviceInfo>) {
    if let Ok(mut cache) = device_cache().lock() {
        *cache = devices;
    }
}

pub fn discover_devices_for_mode(
    timeout: Duration,
    mode: &str,
) -> Result<Vec<CastDeviceInfo>, String> {
    if std::env::var_os("HLS_V6_CAST_NULL").is_some() {
        return Ok(Vec::new());
    }
    let scan_cast = mode != "tvbox";
    let scan_tvbox = mode != "cast";
    let ssdp_worker = scan_cast.then(|| {
        thread::spawn(move || {
            ssdp_search(timeout)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|location| describe_device(&location))
                .collect::<Vec<_>>()
        })
    });
    let chromecast_worker = scan_cast.then(|| thread::spawn(move || discover_chromecasts(timeout)));
    let tvbox_worker = scan_tvbox.then(|| thread::spawn(move || discover_tvboxes(timeout)));
    let mut devices = ssdp_worker
        .and_then(|worker| worker.join().ok())
        .unwrap_or_default();
    devices.extend(
        chromecast_worker
            .and_then(|worker| worker.join().ok())
            .unwrap_or_default(),
    );
    devices.extend(
        tvbox_worker
            .and_then(|worker| worker.join().ok())
            .unwrap_or_default(),
    );
    devices.sort_by(|left, right| left.label.cmp(&right.label).then(left.id.cmp(&right.id)));
    devices.dedup_by(|left, right| left.id == right.id || left.control_url == right.control_url);
    if let Ok(mut cache) = device_cache().lock() {
        *cache = devices.clone();
    }
    Ok(devices)
}

pub fn discover_devices(timeout: Duration) -> Result<Vec<CastDeviceInfo>, String> {
    discover_devices_for_mode(timeout, "")
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
    let body = format!("do=push&url={}", percent_encode(media_url));
    let mut response = tvbox_http_request("POST", &action, &body, Duration::from_secs(8))?;
    if response.status == 404 || response.status == 405 {
        response = tvbox_http_request(
            "GET",
            &format!("{action}?{body}"),
            "",
            Duration::from_secs(8),
        )?;
    }
    validate_tvbox_response(&response)?;
    Ok(())
}

#[derive(Debug)]
struct TvboxHttpResponse {
    status: u16,
    body: String,
}

fn tvbox_http_request(
    method: &str,
    url: &str,
    body: &str,
    timeout: Duration,
) -> Result<TvboxHttpResponse, String> {
    let (host, port, path) = split_http_url(url)?;
    if !is_lan_host(&host) {
        return Err("TVBox 地址必须是局域网".into());
    }
    let address = SocketAddr::from((
        host.parse::<Ipv4Addr>()
            .map_err(|error| error.to_string())?,
        port,
    ));
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("连接 TVBox: {error}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let content_headers = if method == "POST" {
        format!(
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            body.len()
        )
    } else {
        String::new()
    };
    let request = format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\n{content_headers}Connection: close\r\n\r\n{body}");
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("读取 TVBox 响应: {error}"))?;
    let (headers, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "TVBox 返回了无效响应".to_string())?;
    Ok(TvboxHttpResponse {
        status,
        body: body.trim().to_string(),
    })
}

fn validate_tvbox_response(response: &TvboxHttpResponse) -> Result<(), String> {
    if response.status >= 400 {
        return Err(format!("电视拒绝了推送（HTTP {}）", response.status));
    }
    if response.body.is_empty() {
        return Ok(());
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&response.body) {
        let failed_code = value
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|code| code >= 400);
        let failed = value.get("ok") == Some(&serde_json::Value::Bool(false))
            || value.get("success") == Some(&serde_json::Value::Bool(false))
            || value
                .get("error")
                .is_some_and(|item| !item.is_null() && item.as_str() != Some(""))
            || failed_code;
        if failed {
            let message = ["error", "message", "msg"]
                .into_iter()
                .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
                .filter(|item| !item.trim().is_empty())
                .unwrap_or("电视拒绝了推送");
            return Err(message.to_string());
        }
    }
    let lower = response.body.trim().to_ascii_lowercase();
    if lower.starts_with("error") || lower.starts_with("fail") || lower.starts_with("failed") {
        return Err(response.body.clone());
    }
    Ok(())
}

const TVBOX_PORTS: [u16; 4] = [9978, 9979, 9977, 9976];

fn tvbox_scan_targets(networks: &[(Ipv4Addr, u8)], max_hosts: usize) -> VecDeque<SocketAddr> {
    let mut targets = VecDeque::new();
    let mut remaining_hosts = max_hosts;
    for &(local, prefix) in networks {
        if remaining_hosts == 0 {
            break;
        }
        let local_u32 = u32::from(local);
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        let network = local_u32 & mask;
        let broadcast = network | !mask;
        for candidate in network.saturating_add(1)..broadcast {
            if candidate == local_u32 {
                continue;
            }
            let host = Ipv4Addr::from(candidate);
            for port in TVBOX_PORTS {
                targets.push_back(SocketAddr::from((host, port)));
            }
            remaining_hosts -= 1;
            if remaining_hosts == 0 {
                break;
            }
        }
    }
    targets
}

fn discover_tvboxes(timeout: Duration) -> Vec<CastDeviceInfo> {
    let networks = lan_ipv4_networks();
    if networks.is_empty() {
        return Vec::new();
    }
    let targets = tvbox_scan_targets(&networks, 512);
    let queue = Arc::new(Mutex::new(targets));
    let found = Arc::new(Mutex::new(Vec::new()));
    let probe_timeout = timeout
        .min(Duration::from_millis(140))
        .max(Duration::from_millis(60));
    let workers = (0..64)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let found = Arc::clone(&found);
            thread::spawn(move || loop {
                let address = queue.lock().ok().and_then(|mut items| items.pop_front());
                let Some(address) = address else { break };
                if let Some(device) = probe_tvbox(address, probe_timeout) {
                    if let Ok(mut devices) = found.lock() {
                        devices.push(device);
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        let _ = worker.join();
    }
    Arc::try_unwrap(found)
        .ok()
        .and_then(|items| items.into_inner().ok())
        .unwrap_or_default()
}

fn probe_tvbox(address: SocketAddr, timeout: Duration) -> Option<CastDeviceInfo> {
    let mut stream = TcpStream::connect_timeout(&address, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        address
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut body = Vec::new();
    stream.take(8192).read_to_end(&mut body).ok()?;
    let text = String::from_utf8_lossy(&body);
    let status = text
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u16>()
        .ok()?;
    if status >= 500 {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let matched = ["tvbox", "vod", "player", "/action", "push"]
        .into_iter()
        .any(|marker| lower.contains(marker))
        || text.contains("影视");
    if !matched {
        return None;
    }
    let endpoint = format!("http://{address}");
    Some(CastDeviceInfo {
        id: format!("tvbox:{endpoint}"),
        label: "TVBox / 影视盒子".into(),
        location: endpoint.clone(),
        control_url: endpoint,
        service_type: "tvbox".into(),
    })
}

pub fn play_on_device(device_id: &str, media_url: &str, title: &str) -> Result<String, String> {
    let device = cached_devices()
        .into_iter()
        .find(|item| item.id == device_id || item.control_url == device_id)
        .ok_or_else(|| "请先扫描并选择投屏设备".to_string())?;
    if device.service_type == "tvbox" || device.id.starts_with("tvbox:") {
        push_tvbox(&device.control_url, media_url, title)?;
        remember_last_device(device.clone());
        return Ok(device.label);
    }
    if device.service_type == "chromecast" || device.id.starts_with("chromecast:") {
        chromecast_play(&device.control_url, media_url, title)?;
        remember_last_device(device.clone());
        return Ok(device.label);
    }
    av_transport_action(
        &device,
        "SetAVTransportURI",
        &[
            ("InstanceID", "0"),
            ("CurrentURI", media_url),
            ("CurrentURIMetaData", &didl_lite(media_url, title)),
        ],
    )?;
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

pub fn remember_tvbox(endpoint: &str) {
    remember_last_device(CastDeviceInfo {
        id: "tvbox:configured".into(),
        label: format!("TVBox · {}", endpoint.trim()),
        location: endpoint.trim().to_string(),
        control_url: endpoint.trim().to_string(),
        service_type: "tvbox".into(),
    });
}

pub fn remember_lan_share(label: &str) {
    remember_last_device(CastDeviceInfo {
        id: "lan:published".into(),
        label: label.to_string(),
        location: String::new(),
        control_url: String::new(),
        service_type: "lan".into(),
    });
}

pub fn last_session_status() -> CastPlaybackStatus {
    let device = last_cast().lock().ok().and_then(|guard| guard.clone());
    device
        .as_ref()
        .map(|item| CastPlaybackStatus {
            label: item.label.clone(),
            device_kind: device_kind(item).to_string(),
            supported_actions: supported_actions(item),
            playing: !matches!(device_kind(item), "tvbox" | "lan"),
            state: if matches!(device_kind(item), "tvbox" | "lan") {
                "PUBLISHED"
            } else {
                "PLAYING"
            }
            .into(),
            ..Default::default()
        })
        .unwrap_or_default()
}

pub fn control_session(action: &str) -> Result<CastPlaybackStatus, String> {
    let device = last_cast()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .ok_or_else(|| "当前没有投屏会话".to_string())?;
    let kind = device_kind(&device);
    if kind == "tvbox" || kind == "lan" {
        if action != "stop" && action != "status" {
            return Err(if kind == "tvbox" {
                "TVBox 没有统一的远程播放控制协议，请在电视端操作"
            } else {
                "局域网播放地址不提供远程播放控制"
            }
            .into());
        }
        if action == "stop" {
            clear_last_device();
        }
        return Ok(CastPlaybackStatus {
            label: device.label,
            device_kind: kind.into(),
            state: if action == "stop" {
                "STOPPED"
            } else {
                "PUBLISHED"
            }
            .into(),
            ..Default::default()
        });
    }
    let result = if kind == "chromecast" {
        chromecast_control(&device.control_url, action)
    } else {
        dlna_control(&device, action)
    }?;
    if action == "stop" {
        clear_last_device();
    }
    Ok(result)
}

fn clear_last_device() {
    if let Ok(mut guard) = last_cast().lock() {
        *guard = None;
    }
}

fn device_kind(device: &CastDeviceInfo) -> &'static str {
    if device.service_type == "lan" || device.id.starts_with("lan:") {
        "lan"
    } else if device.service_type == "tvbox" || device.id.starts_with("tvbox:") {
        "tvbox"
    } else if device.service_type == "chromecast" || device.id.starts_with("chromecast:") {
        "chromecast"
    } else {
        "dlna"
    }
}

fn supported_actions(device: &CastDeviceInfo) -> Vec<String> {
    if matches!(device_kind(device), "tvbox" | "lan") {
        vec!["stop".into()]
    } else {
        [
            "status",
            "play",
            "pause",
            "seek_back",
            "seek_forward",
            "seek_to",
            "stop",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }
}

fn parse_control_action(action: &str) -> (&str, i64) {
    if let Some(value) = action.strip_prefix("seek_to:") {
        return ("seek_to", value.parse().unwrap_or(0));
    }
    if let Some(value) = action.strip_prefix("seek:") {
        return ("seek", value.parse().unwrap_or(0));
    }
    match action {
        "seek_back" => ("seek", -10),
        "seek_forward" => ("seek", 10),
        other => (other, 0),
    }
}

fn format_duration(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

fn parse_duration(value: &str) -> Option<u64> {
    let value = value.trim().split('.').next()?;
    let parts: Vec<_> = value.split(':').collect();
    let (hours, minutes, seconds): (u64, u64, u64) = match parts.as_slice() {
        [minutes, seconds] => (0, minutes.parse().ok()?, seconds.parse().ok()?),
        [hours, minutes, seconds] => (
            hours.parse().ok()?,
            minutes.parse().ok()?,
            seconds.parse().ok()?,
        ),
        _ => return None,
    };
    (minutes < 60 && seconds < 60).then_some(hours * 3600 + minutes * 60 + seconds)
}

fn dlna_status(device: &CastDeviceInfo) -> CastPlaybackStatus {
    let position_response =
        av_transport_action_response(device, "GetPositionInfo", &[("InstanceID", "0")]);
    let transport_response =
        av_transport_action_response(device, "GetTransportInfo", &[("InstanceID", "0")]);
    let position_seconds = position_response
        .as_ref()
        .ok()
        .and_then(|body| xml_local(body, "RelTime"))
        .and_then(|value| parse_duration(&value));
    let duration_seconds = position_response
        .as_ref()
        .ok()
        .and_then(|body| xml_local(body, "TrackDuration"))
        .and_then(|value| parse_duration(&value))
        .unwrap_or(0);
    let state = transport_response
        .as_ref()
        .ok()
        .and_then(|body| xml_local(body, "CurrentTransportState"))
        .unwrap_or_else(|| "UNKNOWN".into())
        .to_ascii_uppercase();
    CastPlaybackStatus {
        label: device.label.clone(),
        device_kind: "dlna".into(),
        supported_actions: supported_actions(device),
        playing: matches!(state.as_str(), "PLAYING" | "TRANSITIONING"),
        paused: state == "PAUSED_PLAYBACK",
        position_seconds: position_seconds.unwrap_or(0),
        duration_seconds,
        position_available: position_seconds.is_some(),
        state,
    }
}

fn dlna_control(device: &CastDeviceInfo, action: &str) -> Result<CastPlaybackStatus, String> {
    let (action, seconds) = parse_control_action(action);
    match action {
        "play" => av_transport_action(device, "Play", &[("InstanceID", "0"), ("Speed", "1")])?,
        "pause" => av_transport_action(device, "Pause", &[("InstanceID", "0")])?,
        "stop" => av_transport_action(device, "Stop", &[("InstanceID", "0")])?,
        "seek" | "seek_to" => {
            let current = if action == "seek" {
                dlna_status(device).position_seconds as i64
            } else {
                0
            };
            let target = if action == "seek" {
                current.saturating_add(seconds).max(0)
            } else {
                seconds.max(0)
            } as u64;
            let target = format_duration(target);
            av_transport_action(
                device,
                "Seek",
                &[
                    ("InstanceID", "0"),
                    ("Unit", "REL_TIME"),
                    ("Target", &target),
                ],
            )?;
        }
        "status" => {}
        _ => return Err(format!("不支持的投屏控制操作: {action}")),
    }
    let mut status = dlna_status(device);
    if action == "stop" {
        status.playing = false;
        status.paused = false;
        status.state = "STOPPED".into();
    }
    Ok(status)
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

fn ssdp_search_on(interface: Ipv4Addr, timeout: Duration) -> Vec<String> {
    let socket = match UdpSocket::bind((interface, 0)) {
        Ok(socket) => socket,
        Err(_) => return Vec::new(),
    };
    let _ = socket.set_nonblocking(true);
    let targets = [
        "urn:schemas-upnp-org:device:MediaRenderer:1",
        "urn:schemas-upnp-org:service:AVTransport:1",
        "ssdp:all",
    ];
    for _ in 0..2 {
        for target in targets {
            let body = format!(
                "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: {target}\r\nUSER-AGENT: HLSDownloader/7 UPnP/1.1\r\n\r\n"
            );
            let _ = socket.send_to(body.as_bytes(), "239.255.255.250:1900");
        }
        thread::sleep(Duration::from_millis(25));
    }
    let mut locations = Vec::new();
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((count, _)) => {
                if let Some(location) = parse_ssdp_location(&String::from_utf8_lossy(&buf[..count]))
                {
                    if is_lan_url(&location) && !locations.contains(&location) {
                        locations.push(location);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(8));
            }
            Err(_) => break,
        }
    }
    locations
}

fn ssdp_search(timeout: Duration) -> Result<Vec<String>, String> {
    let workers = discovery_interface_addresses()
        .into_iter()
        .map(|interface| thread::spawn(move || ssdp_search_on(interface, timeout)))
        .collect::<Vec<_>>();
    let mut locations = Vec::new();
    for worker in workers {
        for location in worker.join().unwrap_or_default() {
            if !locations.contains(&location) {
                locations.push(location);
            }
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
    av_transport_action_response(device, action, args).map(|_| ())
}

fn av_transport_action_response(
    device: &CastDeviceInfo,
    action: &str,
    args: &[(&str, &str)],
) -> Result<String, String> {
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
        inner.push_str(&format!("<{name}>{}</{name}>", xml_escape(value)));
    }
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service}\">{inner}</u:{action}></s:Body></s:Envelope>"
    );
    let soap_action = format!("\"{service}#{action}\"");
    let response = http_post_lan(&device.control_url, &soap_action, &body)?;
    if response.contains("s:Fault") || response.contains("UPnPError") {
        return Err(format!("投屏设备拒绝 {action}"));
    }
    Ok(response)
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
    stream.set_read_timeout(Some(Duration::from_secs(6))).ok();
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
        (host.to_string(), port.parse::<u16>().unwrap_or(80))
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
        let host_end = after
            .find('/')
            .map(|index| scheme + 3 + index)
            .unwrap_or(url.len());
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
    // Ask for a unicast reply so discovery can use a per-interface ephemeral port
    // even when another application already owns the shared mDNS port.
    packet.extend_from_slice(&0x8001u16.to_be_bytes());
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
            .unwrap_or_else(|| {
                instance
                    .split('.')
                    .next()
                    .unwrap_or("Chromecast")
                    .to_string()
            });
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

fn discover_chromecasts_on(interface: Ipv4Addr, timeout: Duration) -> Vec<CastDeviceInfo> {
    let socket = match UdpSocket::bind((interface, 0)) {
        Ok(socket) => socket,
        Err(_) => return Vec::new(),
    };
    let _ = socket.set_nonblocking(true);
    let _ = socket.join_multicast_v4(&Ipv4Addr::new(224, 0, 0, 251), &interface);
    let query = mdns_googlecast_query();
    for _ in 0..2 {
        let _ = socket.send_to(&query, "224.0.0.251:5353");
        thread::sleep(Duration::from_millis(25));
    }
    let deadline = Instant::now() + timeout;
    let mut devices = Vec::new();
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((count, _)) => {
                for device in parse_mdns_chromecasts(&buf[..count]) {
                    if !devices
                        .iter()
                        .any(|item: &CastDeviceInfo| item.id == device.id)
                    {
                        devices.push(device);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(8));
            }
            Err(_) => break,
        }
    }
    devices
}

fn discover_chromecasts(timeout: Duration) -> Vec<CastDeviceInfo> {
    let workers = discovery_interface_addresses()
        .into_iter()
        .map(|interface| thread::spawn(move || discover_chromecasts_on(interface, timeout)))
        .collect::<Vec<_>>();
    let mut devices = Vec::new();
    for worker in workers {
        for device in worker.join().unwrap_or_default() {
            if !devices
                .iter()
                .any(|item: &CastDeviceInfo| item.id == device.id)
            {
                devices.push(device);
            }
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
    let (host, port) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| "Chromecast 地址无效".to_string())?;
    let port: u16 = port
        .parse()
        .map_err(|_| "Chromecast 端口无效".to_string())?;
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

fn chromecast_control(endpoint: &str, action: &str) -> Result<CastPlaybackStatus, String> {
    let (host, port) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| "Chromecast 地址无效".to_string())?;
    let port: u16 = port
        .parse()
        .map_err(|_| "Chromecast 端口无效".to_string())?;
    if !is_lan_host(host) {
        return Err("Chromecast 必须在局域网".into());
    }
    #[cfg(windows)]
    {
        chromecast_control_windows(host, port, action)
    }
    #[cfg(not(windows))]
    {
        let _ = (port, action);
        Err("Chromecast 投屏使用 Windows Schannel".into())
    }
}

#[cfg(windows)]
fn chromecast_connect_windows(
    host: &str,
    port: u16,
) -> Result<schannel::tls_stream::TlsStream<TcpStream>, String> {
    let raw = TcpStream::connect_timeout(
        &SocketAddr::from((
            host.parse::<Ipv4Addr>()
                .map_err(|error| error.to_string())?,
            port,
        )),
        Duration::from_secs(4),
    )
    .map_err(|error| error.to_string())?;
    raw.set_read_timeout(Some(Duration::from_millis(900)))
        .map_err(|error| error.to_string())?;
    let cred = schannel::schannel_cred::SchannelCred::builder()
        .acquire(schannel::schannel_cred::Direction::Outbound)
        .map_err(|error| error.to_string())?;
    schannel::tls_stream::Builder::new()
        .domain(host)
        .verify_callback(|_| Ok(()))
        .connect(cred, raw)
        .map_err(|error| error.to_string())
}

#[cfg(windows)]
fn chromecast_transport<W: Read + Write>(stream: &mut W, launch: bool) -> Result<String, String> {
    write_cast(
        stream,
        CAST_NS_CONNECTION,
        "receiver-0",
        r#"{"type":"CONNECT"}"#,
    )?;
    write_cast(
        stream,
        CAST_NS_RECEIVER,
        "receiver-0",
        r#"{"type":"GET_STATUS","requestId":1}"#,
    )?;
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut launched = false;
    while Instant::now() < deadline {
        let Some((namespace, payload)) = read_cast(stream)? else {
            continue;
        };
        if namespace == CAST_NS_HEARTBEAT && payload.contains("PING") {
            write_cast(
                stream,
                CAST_NS_HEARTBEAT,
                "receiver-0",
                r#"{"type":"PONG"}"#,
            )?;
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        if value.get("type").and_then(|item| item.as_str()) != Some("RECEIVER_STATUS") {
            continue;
        }
        if let Some(transport) = value
            .pointer("/status/applications/0/transportId")
            .and_then(|item| item.as_str())
        {
            return Ok(transport.to_string());
        }
        if launch && !launched {
            launched = true;
            let body = serde_json::json!({
                "type": "LAUNCH",
                "appId": CAST_APP_DEFAULT_MEDIA,
                "requestId": 2
            })
            .to_string();
            write_cast(stream, CAST_NS_RECEIVER, "receiver-0", &body)?;
        }
    }
    Err("Chromecast 没有返回会话".into())
}

fn chromecast_status_from_value(
    label: &str,
    value: &serde_json::Value,
) -> Option<CastPlaybackStatus> {
    let status = value.pointer("/status/0")?;
    let state = status
        .get("playerState")
        .and_then(|item| item.as_str())
        .unwrap_or("UNKNOWN")
        .to_ascii_uppercase();
    let position = status
        .get("currentTime")
        .and_then(|item| item.as_f64())
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let duration = status
        .pointer("/media/duration")
        .and_then(|item| item.as_f64())
        .unwrap_or(0.0)
        .max(0.0) as u64;
    Some(CastPlaybackStatus {
        label: label.to_string(),
        device_kind: "chromecast".into(),
        supported_actions: [
            "status",
            "play",
            "pause",
            "seek_back",
            "seek_forward",
            "seek_to",
            "stop",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        playing: matches!(state.as_str(), "PLAYING" | "BUFFERING"),
        paused: state == "PAUSED",
        position_seconds: position,
        duration_seconds: duration,
        position_available: true,
        state,
    })
}

#[cfg(windows)]
fn chromecast_read_status<W: Read + Write>(
    stream: &mut W,
    transport: &str,
    label: &str,
) -> Result<(CastPlaybackStatus, i64), String> {
    write_cast(
        stream,
        CAST_NS_MEDIA,
        transport,
        r#"{"type":"GET_STATUS","requestId":10}"#,
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let Some((namespace, payload)) = read_cast(stream)? else {
            continue;
        };
        if namespace == CAST_NS_HEARTBEAT && payload.contains("PING") {
            write_cast(
                stream,
                CAST_NS_HEARTBEAT,
                "receiver-0",
                r#"{"type":"PONG"}"#,
            )?;
            continue;
        }
        if namespace != CAST_NS_MEDIA {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        if value.get("type").and_then(|item| item.as_str()) != Some("MEDIA_STATUS") {
            continue;
        }
        if let Some(status) = chromecast_status_from_value(label, &value) {
            let session_id = value
                .pointer("/status/0/mediaSessionId")
                .and_then(|item| item.as_i64())
                .unwrap_or(0);
            return Ok((status, session_id));
        }
    }
    Err("Chromecast 没有返回播放状态".into())
}

#[cfg(windows)]
fn chromecast_control_windows(
    host: &str,
    port: u16,
    action: &str,
) -> Result<CastPlaybackStatus, String> {
    let mut stream = chromecast_connect_windows(host, port)?;
    let transport = chromecast_transport(&mut stream, false)?;
    write_cast(
        &mut stream,
        CAST_NS_CONNECTION,
        &transport,
        r#"{"type":"CONNECT"}"#,
    )?;
    let label = last_device_label();
    let (mut status, session_id) = chromecast_read_status(&mut stream, &transport, &label)?;
    let (operation, seconds) = parse_control_action(action);
    if operation == "status" {
        return Ok(status);
    }
    if !matches!(operation, "play" | "pause" | "stop" | "seek" | "seek_to") {
        return Err(format!("不支持的投屏控制操作: {operation}"));
    }
    let mut body = serde_json::json!({
        "type": operation.to_ascii_uppercase(),
        "requestId": 11,
        "mediaSessionId": session_id,
    });
    if operation == "seek" || operation == "seek_to" {
        let current = if operation == "seek" {
            status.position_seconds as i64
        } else {
            0
        };
        body["type"] = serde_json::Value::String("SEEK".into());
        body["currentTime"] = serde_json::json!((if operation == "seek" {
            current.saturating_add(seconds)
        } else {
            seconds
        })
        .max(0));
        body["resumeState"] = serde_json::Value::String("PLAYBACK_UNCHANGED".into());
    }
    write_cast(&mut stream, CAST_NS_MEDIA, &transport, &body.to_string())?;
    if operation == "stop" {
        status.playing = false;
        status.paused = false;
        status.position_seconds = 0;
        status.state = "STOPPED".into();
        return Ok(status);
    }
    chromecast_read_status(&mut stream, &transport, &label)
        .map(|(status, _)| status)
        .or_else(|_| {
            status.playing =
                operation == "play" || (operation.starts_with("seek") && status.playing);
            status.paused = operation == "pause";
            Ok(status)
        })
}

#[cfg(windows)]
fn chromecast_play_windows(
    host: &str,
    port: u16,
    media_url: &str,
    title: &str,
) -> Result<String, String> {
    let mut stream = chromecast_connect_windows(host, port)?;
    let transport = chromecast_transport(&mut stream, true)?;
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

fn write_cast<W: Write>(
    stream: &mut W,
    namespace: &str,
    destination: &str,
    payload: &str,
) -> Result<(), String> {
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
    stream
        .read_exact(&mut body)
        .map_err(|error| error.to_string())?;
    let mut frame = header.to_vec();
    frame.extend(body);
    Ok(decode_cast_payload(&frame))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn rejects_public_cast_hosts() {
        assert!(is_lan_host("192.168.1.8"));
        assert!(is_lan_host("10.0.0.2"));
        assert!(!is_lan_host("8.8.8.8"));
        assert!(start_browser_push("cast", "javascript:alert(1)", "x").is_err());
        assert!(start_browser_push("cast", "https://cdn.test/a.mp4\0", "x").is_err());
        assert!(start_browser_push("cast", "file:///C:/Windows/video.mp4", "x").is_err());
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
        assert_eq!(
            device.control_url,
            "http://192.168.1.20:8008/upnp/control/AVTransport"
        );
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
        let query = mdns_googlecast_query();
        assert_eq!(&query[query.len() - 2..], &[0x80, 0x01]);

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
        let frame = encode_cast_message(CAST_NS_MEDIA, "web-1", r#"{"type":"LOAD","requestId":3}"#);
        let (namespace, payload) = decode_cast_payload(&frame).unwrap();
        assert_eq!(namespace, CAST_NS_MEDIA);
        assert!(payload.contains("LOAD"));
        assert_eq!(
            chromecast_mime("http://10.0.0.2/local.m3u8", "live"),
            "application/vnd.apple.mpegurl"
        );
    }

    #[test]
    fn parses_cast_actions_and_playback_durations() {
        assert_eq!(parse_control_action("seek:-15"), ("seek", -15));
        assert_eq!(parse_control_action("seek_to:125"), ("seek_to", 125));
        assert_eq!(parse_control_action("seek_forward"), ("seek", 10));
        assert_eq!(parse_duration("01:02:03.250"), Some(3723));
        assert_eq!(parse_duration("02:05"), Some(125));
        assert_eq!(parse_duration("00:99:00"), None);
        assert_eq!(format_duration(3723), "01:02:03");
    }

    #[test]
    fn rejects_explicit_tvbox_failure_bodies() {
        assert!(validate_tvbox_response(&TvboxHttpResponse {
            status: 500,
            body: String::new()
        })
        .is_err());
        assert!(validate_tvbox_response(&TvboxHttpResponse {
            status: 200,
            body: r#"{"ok":false,"message":"busy"}"#.into()
        })
        .is_err());
        assert!(validate_tvbox_response(&TvboxHttpResponse {
            status: 200,
            body: r#"{"code":503,"msg":"offline"}"#.into()
        })
        .is_err());
        assert!(validate_tvbox_response(&TvboxHttpResponse {
            status: 200,
            body: "FAILED unavailable".into()
        })
        .is_err());
        assert!(validate_tvbox_response(&TvboxHttpResponse {
            status: 200,
            body: r#"{"ok":true}"#.into()
        })
        .is_ok());
    }

    #[test]
    fn tvbox_push_falls_back_from_post_to_get() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for status in ["405 Method Not Allowed", "200 OK"] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(300)))
                    .ok();
                let mut buffer = [0u8; 4096];
                let count = stream.read(&mut buffer).unwrap();
                requests.push(String::from_utf8_lossy(&buffer[..count]).to_string());
                let body = if status.starts_with("200") {
                    r#"{"ok":true}"#
                } else {
                    ""
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        push_tvbox(&endpoint, "https://cdn.test/video.mp4", "video").unwrap();
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("POST /action HTTP/1.1"));
        assert!(requests[1]
            .starts_with("GET /action?do=push&url=https%3A%2F%2Fcdn.test%2Fvideo.mp4 HTTP/1.1"));
    }

    #[test]
    fn tvbox_probe_rejects_generic_http_and_accepts_push_service() {
        fn serve(body: &'static str) -> (SocketAddr, thread::JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            (address, worker)
        }
        let (generic, generic_worker) = serve("plain web service");
        assert!(probe_tvbox(generic, Duration::from_secs(1)).is_none());
        generic_worker.join().unwrap();
        let (tvbox, tvbox_worker) = serve("TVBox player /action push");
        let device = probe_tvbox(tvbox, Duration::from_secs(1)).unwrap();
        assert_eq!(device.service_type, "tvbox");
        tvbox_worker.join().unwrap();
    }

    #[test]
    fn tvbox_scan_targets_cover_multiple_subnets_without_scanning_self() {
        let networks = [
            (Ipv4Addr::new(192, 168, 1, 4), 30),
            (Ipv4Addr::new(10, 0, 0, 1), 30),
        ];
        let targets = tvbox_scan_targets(&networks, 3);
        assert_eq!(targets.len(), 3 * TVBOX_PORTS.len());
        assert!(targets.iter().all(|target| target.ip() != networks[0].0));
        assert!(targets.iter().all(|target| target.ip() != networks[1].0));
        assert!(targets
            .iter()
            .any(|target| target.ip() == Ipv4Addr::new(192, 168, 1, 5)));
        assert!(targets
            .iter()
            .any(|target| target.ip() == Ipv4Addr::new(10, 0, 0, 2)));
    }
}
