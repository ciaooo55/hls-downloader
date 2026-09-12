from pathlib import Path

cast = Path("native_shell/src/cast.rs")
text = cast.read_text(encoding="utf-8")

old = '''    let host = primary_lan_ipv4()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".into());'''
new = '''    let host = preferred_lan_ipv4()
        .map(|ip| ip.to_string())
        .ok_or_else(|| "没有可用于投屏的局域网地址".to_string())?;'''
if text.count(old) != 1:
    raise SystemExit(f"start_browser_push host anchor count={text.count(old)}")
text = text.replace(old, new, 1)

marker = '''pub fn primary_lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("10.255.255.255:1").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if ip.is_private() || ip.is_link_local() => Some(ip),
        _ => None,
    }
}
'''
addition = marker + '''
/// Prefer a physical LAN adapter for address publication. On Windows the
/// discovery list already excludes loopback, tunnels, common VPNs, WSL and
/// Hyper-V adapters; the legacy route probe remains a fallback when adapter
/// enumeration is unavailable.
pub fn preferred_lan_ipv4() -> Option<Ipv4Addr> {
    #[cfg(windows)]
    {
        lan_ipv4_networks().into_iter().map(|(address, _)| address).next()
    }
    #[cfg(not(windows))]
    {
        primary_lan_ipv4()
    }
}

fn peer_ipv4_from_endpoint(endpoint: &str) -> Option<Ipv4Addr> {
    let endpoint = endpoint.trim();
    let host = if endpoint.starts_with("http://") {
        split_http_url(endpoint).ok()?.0
    } else if endpoint.contains("://") {
        host_of(endpoint)?
    } else if let Some((host, port)) = endpoint.rsplit_once(':') {
        port.parse::<u16>().ok()?;
        host.to_string()
    } else {
        endpoint.to_string()
    };
    let peer = host.parse::<Ipv4Addr>().ok()?;
    (peer.is_private() || peer.is_link_local()).then_some(peer)
}

fn routed_lan_ipv4(peer: Ipv4Addr) -> Option<Ipv4Addr> {
    if !(peer.is_private() || peer.is_link_local()) {
        return None;
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(SocketAddr::from((peer, 9))).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if (ip.is_private() || ip.is_link_local()) && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

fn device_peer_ipv4(device: &CastDeviceInfo) -> Option<Ipv4Addr> {
    peer_ipv4_from_endpoint(&device.control_url)
        .or_else(|| peer_ipv4_from_endpoint(&device.location))
}

/// Resolve the local IPv4 address that Windows/Linux would use to reach the
/// selected receiver. This avoids advertising a VPN/virtual-adapter address
/// to a TV that was discovered on another interface.
pub fn device_lan_ipv4(device_id: &str) -> Option<Ipv4Addr> {
    let device = cached_devices()
        .into_iter()
        .find(|item| item.id == device_id || item.control_url == device_id)?;
    routed_lan_ipv4(device_peer_ipv4(&device)?)
}

pub fn endpoint_lan_ipv4(endpoint: &str) -> Option<Ipv4Addr> {
    routed_lan_ipv4(peer_ipv4_from_endpoint(endpoint)?)
}
'''
if text.count(marker) != 1:
    raise SystemExit(f"primary_lan_ipv4 anchor count={text.count(marker)}")
text = text.replace(marker, addition, 1)

marker = '''    #[test]
    fn parses_ssdp_location_header() {'''
addition = '''    #[test]
    fn extracts_private_receiver_addresses_across_cast_protocols() {
        assert_eq!(
            peer_ipv4_from_endpoint("http://192.168.1.20:8008/upnp/control/AVTransport"),
            Some(Ipv4Addr::new(192, 168, 1, 20))
        );
        assert_eq!(
            peer_ipv4_from_endpoint("10.0.0.8:8009"),
            Some(Ipv4Addr::new(10, 0, 0, 8))
        );
        assert_eq!(
            peer_ipv4_from_endpoint("http://172.16.1.9:9978/action"),
            Some(Ipv4Addr::new(172, 16, 1, 9))
        );
        assert_eq!(peer_ipv4_from_endpoint("http://8.8.8.8:80/action"), None);
        assert_eq!(peer_ipv4_from_endpoint("not-an-endpoint"), None);

        let chromecast = CastDeviceInfo {
            id: "chromecast:kitchen".into(),
            label: "Kitchen".into(),
            location: "https://192.168.50.9:8009".into(),
            control_url: "192.168.50.9:8009".into(),
            service_type: "chromecast".into(),
        };
        assert_eq!(
            device_peer_ipv4(&chromecast),
            Some(Ipv4Addr::new(192, 168, 50, 9))
        );
    }

'''
if text.count(marker) != 1:
    raise SystemExit(f"cast test anchor count={text.count(marker)}")
text = text.replace(marker, addition + marker, 1)
cast.write_text(text, encoding="utf-8")

worker = Path("native_shell/src/download_worker.rs")
text = worker.read_text(encoding="utf-8")

marker = '''fn cast_task(coordinator: &CoreCoordinator, task_id: &str) -> Result<Vec<EventEnvelope>, String> {'''
helper = '''fn cast_lan_host(device_id: &str) -> Result<String, String> {
    let address = if device_id.trim().is_empty() {
        crate::cast::preferred_lan_ipv4()
    } else {
        crate::cast::device_lan_ipv4(device_id)
    };
    address
        .map(|ip| ip.to_string())
        .ok_or_else(|| "无法确定到投屏设备的局域网出口地址".to_string())
}

'''
if text.count(marker) != 1:
    raise SystemExit(f"cast_task marker count={text.count(marker)}")
text = text.replace(marker, helper + marker, 1)

old = '''    let host = crate::cast::primary_lan_ipv4()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".into());'''
if text.count(old) != 2:
    raise SystemExit(f"legacy loopback cast host count={text.count(old)}")
text = text.replace(old, '''    let host = cast_lan_host("")?;''', 1)
text = text.replace(old, '''    let host = cast_lan_host(device_id)?;''', 1)

old = '''        let host = crate::cast::primary_lan_ipv4()
            .map(|ip| ip.to_string())
            .ok_or_else(|| "没有可用于投屏的局域网地址".to_string())?;'''
if text.count(old) != 2:
    raise SystemExit(f"share media LAN host count={text.count(old)}")
text = text.replace(old, '''        let host = cast_lan_host(device_id)?;''', 1)
text = text.replace(old, '''            let host = cast_lan_host("")?;''', 1)

old = '''    let server = shared_media()?;
    server.enable_lan();
    let host = crate::cast::primary_lan_ipv4()
        .map(|ip| ip.to_string())
        .ok_or_else(|| "没有可用于 TVBox 的局域网地址".to_string())?;
    let url = crate::cast::lan_media_url(server, &token, &host)?;
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let endpoint = coordinator
        .lock()?
        .store()
        .setting_string("tvbox_endpoint", "")?;
    if endpoint.trim().is_empty() {
        return Err("请先在设置里填写 TVBox 地址".into());
    }'''
new = '''    let server = shared_media()?;
    server.enable_lan();
    let spec = coordinator
        .lock()?
        .task_spec(task_id)
        .cloned()
        .ok_or_else(|| format!("unknown task {task_id}"))?;
    let endpoint = coordinator
        .lock()?
        .store()
        .setting_string("tvbox_endpoint", "")?;
    if endpoint.trim().is_empty() {
        return Err("请先在设置里填写 TVBox 地址".into());
    }
    let host = crate::cast::endpoint_lan_ipv4(&endpoint)
        .map(|ip| ip.to_string())
        .ok_or_else(|| "无法确定到 TVBox 的局域网出口地址".to_string())?;
    let url = crate::cast::lan_media_url(server, &token, &host)?;'''
if text.count(old) != 1:
    raise SystemExit(f"TVBox host block count={text.count(old)}")
text = text.replace(old, new, 1)

if "crate::cast::primary_lan_ipv4()" in text:
    raise SystemExit("unexpected primary_lan_ipv4 remains in download_worker.rs")
worker.write_text(text, encoding="utf-8")
