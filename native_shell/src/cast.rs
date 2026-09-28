//! LAN-restricted media push (DLNA SSDP search + AVTransport).

use crate::playback::MediaServer;
use crate::CastDeviceInfo;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

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

fn device_cache() -> &'static Mutex<Vec<CastDeviceInfo>> {
    static CACHE: OnceLock<Mutex<Vec<CastDeviceInfo>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn lan_media_url(
    server: &MediaServer,
    token: &str,
    advertise_host: &str,
) -> Result<String, String> {
    if advertise_host == "127.0.0.1" || advertise_host == "localhost" {
        return Ok(server.cast_url_for(token, "127.0.0.1"));
    }
    if !is_lan_host(advertise_host) {
        return Err("cast host must be a private LAN address".into());
    }
    Ok(server.cast_url_for(token, advertise_host))
}

/// 局域网成员的唯一定义：对端是否落在**本机真实网卡自己持有的「地址 + 前缀」**上。
///
/// 这里刻意**不维护任何网段表**（RFC1918、100.64/10、192.0.2.0/24……）。这是个要
/// 装到别人机器上的产品：用户的局域网可能是手机热点分下来的 100.64/10、公司内网的
/// 8.8.0.0/12、实验室的 192.0.2.0/24。任何一张写死的表都会在某一类人手上失灵，而
/// 表越长越像占卜。和"我这台机器"真正相关的答案只有一个：**这个地址是否就在我某块
/// 网卡的同一个网段上**。所以规则从网卡来，不从数字来。
///
/// 顺带比"是否私有地址"更严也更准：
///   * 8.8.8.8 / 1.1.1.1 不在我任何网段内 → 拒绝，SSRF 边界**没有放宽**；
///   * 云元数据 169.254.169.254 同样不在（网卡枚举本来就把 link-local 排除了）；
///   * 本机确实在 100.64/10 上时，同段的热点设备顺理成章地可用，不需要为它加白名单。
///
/// `networks` 由调用方传入，是为了让规则本身可单测：真实网卡列表随机器而变，
/// 判据不能跟着变。
fn address_is_on_link(networks: &[(Ipv4Addr, u8)], peer: Ipv4Addr) -> bool {
    // 这几类地址**自己先出局**，不依赖网卡枚举：本函数是纯函数，任何人拿到一份
    // networks 就能判定，所以边界必须自带。link-local 出局尤其重要——它是云元数据
    // 169.254.169.254 所在的段，一旦某块网卡的列表里混进了 169.254.x，"同网段"
    // 这个判据就会把它放进来。
    if peer.is_link_local() || peer.is_multicast() || peer.is_unspecified() || peer.is_broadcast() {
        return false;
    }
    networks
        .iter()
        .any(|(local, prefix)| *local == peer || ipv4_network_contains(*local, *prefix, peer))
}

/// 上面那条规则配上本机此刻的真实网卡列表——局域网判定的唯一事实来源。
pub fn is_lan_ipv4(peer: Ipv4Addr) -> bool {
    address_is_on_link(&lan_ipv4_networks(), peer)
}

/// 字符串形态的局域网判定，供 URL 与手工端点校验使用。本机回环仍然算合法：
/// 媒体服务器本来就监听在 127.0.0.1 上，界面上也允许填 localhost。
pub fn is_lan_host(host: &str) -> bool {
    match host.parse::<Ipv4Addr>() {
        Ok(addr) => addr.is_loopback() || is_lan_ipv4(addr),
        Err(_) => false,
    }
}

/// 路由探测：问操作系统"要到 10.255.255.255 该从哪块网卡出去"，拿到的就是本机
/// 在这个方向上的地址。**不看地址落在哪个段**——同一个理由：用户在哪个段是未知的，
/// 这个函数只负责"操作系统挑中的那个地址"。是不是局域网，交给
/// [address_is_on_link] 按真实网卡判。
pub fn primary_lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("10.255.255.255:1").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

/// Prefer a physical LAN adapter for address publication. On Windows the
/// discovery list already excludes loopback, tunnels, common VPNs, WSL and
/// Hyper-V adapters; the legacy route probe remains a fallback when adapter
/// enumeration is unavailable.
pub fn preferred_lan_ipv4() -> Option<Ipv4Addr> {
    #[cfg(windows)]
    {
        lan_ipv4_networks()
            .into_iter()
            .map(|(address, _)| address)
            .next()
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
    is_lan_ipv4(peer).then_some(peer)
}

/// 解析出"这个设备/端点在我哪块网卡上"，用于给电视通告本机地址、给推送选出口。
/// 先按 [address_is_on_link] 判是否局域网成员，再让操作系统选源地址。
pub fn routed_lan_ipv4(peer: Ipv4Addr) -> Option<Ipv4Addr> {
    let networks = lan_ipv4_networks();
    if !address_is_on_link(&networks, peer) {
        return None;
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(SocketAddr::from((peer, 9))).ok()?;
    let routed = match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() => Some(ip),
        _ => None,
    };
    #[cfg(windows)]
    {
        physical_lan_source(&networks, peer, routed)
    }
    #[cfg(not(windows))]
    {
        routed
    }
}

fn ipv4_network_contains(local: Ipv4Addr, prefix: u8, peer: Ipv4Addr) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix.min(32))
    };
    u32::from(local) & mask == u32::from(peer) & mask
}

fn physical_lan_source(
    networks: &[(Ipv4Addr, u8)],
    peer: Ipv4Addr,
    routed: Option<Ipv4Addr>,
) -> Option<Ipv4Addr> {
    networks
        .iter()
        .find(|(local, prefix)| ipv4_network_contains(*local, *prefix, peer))
        .map(|(local, _)| *local)
        .or_else(|| routed.filter(|route| networks.iter().any(|(local, _)| local == route)))
}

fn tunnel_adapter_name(name: &str, description: &str) -> bool {
    let identity = format!("{name} {description}").to_ascii_lowercase();
    [
        "tun",
        "tap",
        "tunnel",
        "wintun",
        "wireguard",
        "mihomo",
        "clash",
        "sing-box",
        "tailscale",
        "zerotier",
    ]
    .iter()
    .any(|marker| identity.contains(marker))
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

#[cfg(windows)]
fn lan_ipv4_state() -> (Vec<(Ipv4Addr, u8)>, bool) {
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
        return (
            primary_lan_ipv4()
                .map(|address| vec![(address, 24)])
                .unwrap_or_default(),
            false,
        );
    }

    let mut networks = Vec::new();
    let mut tun_active = false;
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !adapter.is_null() {
        let item = unsafe { &*adapter };
        let name = unsafe { wide_text(item.FriendlyName) }.to_ascii_lowercase();
        let description = unsafe { wide_text(item.Description) }.to_ascii_lowercase();
        let tunnel = item.IfType == IF_TYPE_TUNNEL || tunnel_adapter_name(&name, &description);
        if item.OperStatus == IfOperStatusUp && tunnel {
            tun_active = true;
        }
        let ignored_name = ["virtual", "vpn", "loopback", "wsl", "hyper-v", "虚拟"]
            .iter()
            .any(|marker| name.contains(marker) || description.contains(marker));
        if item.OperStatus == IfOperStatusUp
            && item.IfType != IF_TYPE_SOFTWARE_LOOPBACK
            && !tunnel
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
                    // 这里**不按地址的数字范围挑**，只按适配器本身挑——是不是环回口、
                    // 是不是隧道口（TUN/TAP/Wintun/VPN/WSL/Hyper-V）、名字是不是虚拟，
                    // 这些在上面已经判完了。理由：用户的局域网在哪个段是未知的，热点
                    // 会发 100.64/10，公司会用 8.8.0.0/12，实验室甚至有 192.0.2.0/24。
                    // 以前这里写着 `value.is_private()`，等于替所有用户断言"你的局域网
                    // 一定在 RFC1918 里"——断言错的那天，表现就是"插着网线、电视开着，
                    // 却一个设备都扫不到"，而界面只会说"没有发现设备"。
                    // 前缀上限 30 保留：/8、/0 这类会把"扫同网段"变成永远做不完的动作。
                    if !value.is_loopback()
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
    if networks.is_empty() && !tun_active {
        if let Some(address) = primary_lan_ipv4() {
            networks.push((address, 24));
        }
    }
    (networks, tun_active)
}

#[cfg(windows)]
/// 本机真实网卡的「地址 + 前缀」列表。**[is_lan_ipv4] 的事实来源**，也是
/// 投屏/TVBox 发现流程挑网卡的那张表。公开是为了让集成测试能按本机推导
/// 局域网正例，而不是写死一段 RFC1918。
pub fn lan_ipv4_networks() -> Vec<(Ipv4Addr, u8)> {
    lan_ipv4_state().0
}

#[cfg(windows)]
pub fn tun_mode_active() -> bool {
    lan_ipv4_state().1
}

#[cfg(not(windows))]
/// 见 windows 版本：同样必须公开，理由同上（不为测试单独造一个私有副本）。
pub fn lan_ipv4_networks() -> Vec<(Ipv4Addr, u8)> {
    primary_lan_ipv4()
        .map(|address| vec![(address, 24)])
        .unwrap_or_default()
}

#[cfg(not(windows))]
pub fn tun_mode_active() -> bool {
    false
}

#[cfg(windows)]
fn set_multicast_interface(socket: &UdpSocket, interface: Ipv4Addr) -> Result<(), String> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        setsockopt, IPPROTO_IP, IP_MULTICAST_IF, SOCKET_ERROR,
    };

    let address = interface.octets();
    let status = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            IPPROTO_IP,
            IP_MULTICAST_IF,
            address.as_ptr(),
            address.len() as i32,
        )
    };
    if status == SOCKET_ERROR {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn set_multicast_interface(_socket: &UdpSocket, _interface: Ipv4Addr) -> Result<(), String> {
    Ok(())
}

/// 算出"该在哪些本机地址上发发现请求"。
///
/// 抽成纯函数（不读网卡）是为了能对"物理地址有/无"两种组合做单元测试——
/// 这块逻辑以前一行测试都没有，而它恰好是"有没有 TUN 网卡都能不能找到设备"的分岔点。
///
/// 规则：
/// - 有物理网卡地址就逐个绑，每个地址一个 socket（`IP_MULTICAST_IF` 指到该网卡），
///   这样 Wi-Fi + 有线双网卡的机器两条路都能找到设备；
/// - 一个都没有时**必须**退回 `0.0.0.0`，让操作系统自己挑出口。
///
/// 以前这里是 `addresses.is_empty() && !tun_mode_active()`：TUN 一开就把兜底也关掉。
/// 而"TUN 开着、物理地址又枚举不到"（局域网在 100.64/10 这类非 RFC1918 段、只有
/// vEthernet/Hyper-V 虚拟交换机带地址、`GetAdaptersAddresses` 探测失败……）恰恰是
/// 最需要兜底的场景，结果是一条 M-SEARCH / mDNS 查询都发不出去，电视和盒子必然
/// 找不到，而界面只会告诉用户"没有发现设备"。所以这里不再看 TUN 状态。
fn effective_discovery_interfaces(networks: &[(Ipv4Addr, u8)]) -> Vec<Ipv4Addr> {
    let mut addresses = networks
        .iter()
        .map(|(address, _)| *address)
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        addresses.push(Ipv4Addr::UNSPECIFIED);
    }
    addresses
}

fn discovery_interface_addresses() -> Vec<Ipv4Addr> {
    #[cfg(windows)]
    {
        let (networks, _tun_active) = lan_ipv4_state();
        effective_discovery_interfaces(&networks)
    }
    #[cfg(not(windows))]
    {
        effective_discovery_interfaces(&lan_ipv4_networks())
    }
}

/// 供 TCP 扫描用的本机「地址 + 前缀」列表。与 [discovery_interface_addresses]
/// 分开：扫描必须知道前缀才能推出同网段的候选主机。
///
/// 同样不允许空：物理网卡枚举不到时退回路由探测拿到的本机地址并假定 /24。
/// 以前 `discover_tvboxes` 在这里 `networks.is_empty()` 就 `return Vec::new()`，
/// 一台连接都不发——和上面那条是同一个病根。
fn effective_scan_networks(
    networks: &[(Ipv4Addr, u8)],
    routed: Option<Ipv4Addr>,
) -> Vec<(Ipv4Addr, u8)> {
    if !networks.is_empty() {
        return networks.to_vec();
    }
    routed
        .map(|address| vec![(address, 24)])
        .unwrap_or_default()
}

fn discovery_scan_networks() -> Vec<(Ipv4Addr, u8)> {
    // 兜底用的路由探测地址在"当前有 TUN"时不要用：探测器只看数字，TUN 的
    // 198.18.0.1 现在是合法返回值（见 primary_lan_ipv4），拿它当 /24 去扫，
    // 扫的会是隧道网段而不是局域网。和 lan_ipv4_state() 内部那条兜底保持同一条
    // 判断：有隧道就不猜。
    let routed = if tun_mode_active() {
        None
    } else {
        primary_lan_ipv4()
    };
    effective_scan_networks(&lan_ipv4_networks(), routed)
}

pub fn ssdp_notify(location: &str) -> Result<(), String> {
    if location.contains('\r') || location.contains('\n') || !location.starts_with("http://") {
        return Err("投屏通告地址无效".into());
    }
    let body = format!(
        "NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nNT: upnp:rootdevice\r\nNTS: ssdp:alive\r\nLOCATION: {location}\r\nUSN: uuid:hls-downloader::upnp:rootdevice\r\nCACHE-CONTROL: max-age=30\r\n\r\n"
    );
    let mut sent = false;
    for interface in discovery_interface_addresses() {
        let Ok(socket) = UdpSocket::bind((interface, 0)) else {
            continue;
        };
        if interface != Ipv4Addr::UNSPECIFIED {
            let _ = set_multicast_interface(&socket, interface);
        }
        sent |= socket
            .send_to(body.as_bytes(), "239.255.255.250:1900")
            .is_ok();
    }
    sent.then_some(())
        .ok_or_else(|| "投屏通告未能通过物理局域网发送".to_string())
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

/// 一次设备发现要开哪几路扫描。
///
/// [DiscoveryPlan::ssdp] **永远为真**：SSDP 是 DLNA / 投屏接收端自己的通告通道，
/// TVBox 模式一样要听。以前用一个 `scan_cast = mode != "tvbox"` 同时管住 SSDP 和
/// Chromecast 的 mDNS，于是 tvbox 模式下只做裸端口扫描（9976-9979 四个口），
/// 凡是靠 SSDP 通告的盒子一概看不见——这正是"投屏能找到、TVBox 找不到"的直接原因。
///
/// 抽成纯函数同样是为了能对三种 mode 做单元测试。
#[derive(Debug, PartialEq, Eq)]
struct DiscoveryPlan {
    ssdp: bool,
    chromecast: bool,
    tvbox_sweep: bool,
}

fn discovery_scan_plan(mode: &str) -> DiscoveryPlan {
    let normalized = mode.trim().to_ascii_lowercase();
    DiscoveryPlan {
        ssdp: true,
        chromecast: normalized != "tvbox",
        tvbox_sweep: normalized != "cast",
    }
}

pub fn discover_devices_for_mode(
    timeout: Duration,
    mode: &str,
) -> Result<Vec<CastDeviceInfo>, String> {
    if std::env::var_os("HLS_V7_CAST_NULL").is_some() {
        return Ok(Vec::new());
    }
    let plan = discovery_scan_plan(mode);
    let ssdp_worker = plan.ssdp.then(|| {
        thread::spawn(move || {
            ssdp_search(timeout)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|location| describe_device(&location))
                .collect::<Vec<_>>()
        })
    });
    let chromecast_worker = plan
        .chromecast
        .then(|| thread::spawn(move || discover_chromecasts(timeout)));
    let tvbox_worker = plan
        .tvbox_sweep
        .then(|| thread::spawn(move || discover_tvboxes(timeout)));
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

const MAX_TVBOX_HTTP_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct TvboxHttpResponse {
    status: u16,
    body: String,
}

fn tvbox_http_headers(response: &[u8]) -> Result<Option<(usize, &str)>, String> {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Ok(None);
    };
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|_| "TVBox 返回了非 UTF-8 响应头".to_string())?;
    Ok(Some((header_end, headers)))
}

fn tvbox_http_transfer_is_chunked(headers: &str) -> Result<bool, String> {
    let mut chunked = false;
    for value in headers.lines().skip(1).filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("transfer-encoding")
            .then_some(value)
    }) {
        for encoding in value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            if !encoding.eq_ignore_ascii_case("chunked") {
                return Err("TVBox 返回了不支持的 Transfer-Encoding".into());
            }
            if chunked {
                return Err("TVBox 返回了无效 Transfer-Encoding".into());
            }
            chunked = true;
        }
    }
    Ok(chunked)
}

fn tvbox_http_chunked_complete_len(body: &[u8]) -> Result<Option<usize>, String> {
    let mut cursor = 0usize;
    loop {
        let Some(line_end_offset) = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
        else {
            return Ok(None);
        };
        let line_end = cursor + line_end_offset;
        let size_line = std::str::from_utf8(&body[cursor..line_end])
            .map_err(|_| "TVBox 返回了无效 chunked 响应".to_string())?;
        let size_text = size_line.split(';').next().unwrap_or("").trim();
        if size_text.is_empty() {
            return Err("TVBox 返回了无效 chunked 响应".into());
        }
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| "TVBox 返回了无效 chunked 响应".to_string())?;
        cursor = line_end + 2;
        if size == 0 {
            loop {
                let Some(trailer_end_offset) = body[cursor..]
                    .windows(2)
                    .position(|window| window == b"\r\n")
                else {
                    return Ok(None);
                };
                let trailer_end = cursor + trailer_end_offset;
                if trailer_end == cursor {
                    return Ok(Some(trailer_end + 2));
                }
                cursor = trailer_end + 2;
            }
        }
        let chunk_end = cursor
            .checked_add(size)
            .ok_or_else(|| "TVBox 响应过大".to_string())?;
        let framed_end = chunk_end
            .checked_add(2)
            .ok_or_else(|| "TVBox 响应过大".to_string())?;
        if body.len() < framed_end {
            return Ok(None);
        }
        if &body[chunk_end..framed_end] != b"\r\n" {
            return Err("TVBox 返回了无效 chunked 响应".into());
        }
        cursor = framed_end;
    }
}

fn tvbox_http_decode_chunked_body(body: &[u8]) -> Result<Vec<u8>, String> {
    if tvbox_http_chunked_complete_len(body)?.is_none() {
        return Err("TVBox 返回了不完整 chunked 响应".into());
    }
    let mut decoded = Vec::new();
    let mut cursor = 0usize;
    loop {
        let line_end_offset = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "TVBox 返回了无效 chunked 响应".to_string())?;
        let line_end = cursor + line_end_offset;
        let size_line = std::str::from_utf8(&body[cursor..line_end])
            .map_err(|_| "TVBox 返回了无效 chunked 响应".to_string())?;
        let size_text = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| "TVBox 返回了无效 chunked 响应".to_string())?;
        cursor = line_end + 2;
        if size == 0 {
            break;
        }
        let chunk_end = cursor
            .checked_add(size)
            .ok_or_else(|| "TVBox 响应过大".to_string())?;
        decoded.extend_from_slice(&body[cursor..chunk_end]);
        if decoded.len() > MAX_TVBOX_HTTP_RESPONSE_BYTES {
            return Err("TVBox 响应过大".into());
        }
        cursor = chunk_end + 2;
    }
    Ok(decoded)
}

fn tvbox_http_response_expected_len(response: &[u8]) -> Result<Option<usize>, String> {
    let Some((header_end, headers)) = tvbox_http_headers(response)? else {
        return Ok(None);
    };
    let body_start = header_end + 4;
    if tvbox_http_transfer_is_chunked(headers)? {
        let Some(body_len) = tvbox_http_chunked_complete_len(&response[body_start..])? else {
            return Ok(None);
        };
        return body_start
            .checked_add(body_len)
            .map(Some)
            .ok_or_else(|| "TVBox 响应过大".to_string());
    }
    let content_length = headers.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then_some(value.trim())
    });
    let Some(content_length) = content_length else {
        return Ok(None);
    };
    let content_length = content_length
        .parse::<usize>()
        .map_err(|_| "TVBox 返回了无效 Content-Length".to_string())?;
    body_start
        .checked_add(content_length)
        .map(Some)
        .ok_or_else(|| "TVBox 响应过大".to_string())
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
    let deadline = Instant::now() + timeout;
    let mut response_bytes = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("读取 TVBox 响应超时".into());
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|error| format!("设置 TVBox 读取超时: {error}"))?;
        let remaining_capacity = MAX_TVBOX_HTTP_RESPONSE_BYTES + 1 - response_bytes.len();
        let read_len = chunk.len().min(remaining_capacity);
        match stream.read(&mut chunk[..read_len]) {
            Ok(0) => break,
            Ok(read) => {
                response_bytes.extend_from_slice(&chunk[..read]);
                if let Some(expected_len) = tvbox_http_response_expected_len(&response_bytes)? {
                    if expected_len > MAX_TVBOX_HTTP_RESPONSE_BYTES {
                        return Err("TVBox 响应过大".into());
                    }
                    if response_bytes.len() >= expected_len {
                        response_bytes.truncate(expected_len);
                        break;
                    }
                }
                if response_bytes.len() > MAX_TVBOX_HTTP_RESPONSE_BYTES {
                    return Err("TVBox 响应过大".into());
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return Err("读取 TVBox 响应超时".into());
            }
            Err(error) => return Err(format!("读取 TVBox 响应: {error}")),
        }
    }
    let Some((header_end, headers)) = tvbox_http_headers(&response_bytes)? else {
        return Err("TVBox 返回了无效响应".into());
    };
    // EOF completes only an unframed response. A shorter Content-Length body
    // must not turn a truncated (possibly empty) receiver reply into success.
    if let Some(expected_len) = tvbox_http_response_expected_len(&response_bytes)? {
        if response_bytes.len() < expected_len {
            return Err("TVBox 返回了不完整 Content-Length 响应".into());
        }
    }
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "TVBox 返回了无效响应".to_string())?;
    let body_start = header_end + 4;
    let body_bytes = if tvbox_http_transfer_is_chunked(headers)? {
        tvbox_http_decode_chunked_body(&response_bytes[body_start..])?
    } else {
        response_bytes[body_start..].to_vec()
    };
    let body =
        String::from_utf8(body_bytes).map_err(|_| "TVBox 返回了非 UTF-8 响应".to_string())?;
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

fn tvbox_network_hosts(local: Ipv4Addr, prefix: u8, max_hosts: usize) -> VecDeque<Ipv4Addr> {
    let mut hosts = VecDeque::new();
    if max_hosts == 0 || prefix > 30 {
        return hosts;
    }

    let local_u32 = u32::from(local);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let network = local_u32 & mask;
    let broadcast = network | !mask;
    let mut ranges = Vec::with_capacity(2);
    let mut seen = HashSet::new();

    // A /16 or wider interface can contain tens of thousands of hosts. Keep
    // each interface's own /24 first so round-robin scanning stays local.
    if prefix < 24 {
        let local_24_network = local_u32 & 0xFFFF_FF00;
        let local_24_start = local_24_network
            .saturating_add(1)
            .max(network.saturating_add(1));
        let local_24_end = local_24_network.saturating_add(255).min(broadcast);
        if local_24_start < local_24_end {
            ranges.push((local_24_start, local_24_end));
        }
    }
    ranges.push((network.saturating_add(1), broadcast));

    for (start, end) in ranges {
        for candidate in start..end {
            if hosts.len() >= max_hosts {
                break;
            }
            if candidate == local_u32 {
                continue;
            }
            let host = Ipv4Addr::from(candidate);
            if seen.insert(host) {
                hosts.push_back(host);
            }
        }
        if hosts.len() >= max_hosts {
            break;
        }
    }
    hosts
}

fn tvbox_scan_targets(networks: &[(Ipv4Addr, u8)], max_hosts: usize) -> VecDeque<SocketAddr> {
    let mut targets = VecDeque::new();
    if max_hosts == 0 {
        return targets;
    }

    let local_addresses = networks
        .iter()
        .map(|(local, _)| *local)
        .collect::<HashSet<_>>();
    let mut queues = networks
        .iter()
        .map(|&(local, prefix)| tvbox_network_hosts(local, prefix, max_hosts))
        .filter(|queue| !queue.is_empty())
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let mut remaining_hosts = max_hosts;

    // A bounded scan must not let the first physical adapter consume the whole
    // budget. Take one unique host per adapter per round so Wi-Fi + Ethernet
    // machines still probe every usable LAN before spending deeper budget.
    while remaining_hosts > 0 {
        let mut progressed = false;
        for queue in &mut queues {
            while let Some(host) = queue.pop_front() {
                if local_addresses.contains(&host) || !seen.insert(host) {
                    continue;
                }
                for port in TVBOX_PORTS {
                    targets.push_back(SocketAddr::from((host, port)));
                }
                remaining_hosts -= 1;
                progressed = true;
                break;
            }
            if remaining_hosts == 0 {
                break;
            }
        }
        if !progressed {
            break;
        }
    }
    targets
}

fn tvbox_remaining_timeout(deadline: Instant, cap: Duration) -> Option<Duration> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    if remaining.is_zero() {
        None
    } else {
        Some(remaining.min(cap))
    }
}

fn discover_tvboxes(timeout: Duration) -> Vec<CastDeviceInfo> {
    if timeout.is_zero() {
        return Vec::new();
    }
    // 走 discovery_scan_networks()：物理网卡地址枚举不到时退回路由探测拿到的本机
    // 地址并假定 /24。以前这里用的是 lan_ipv4_networks()，空表就整个放弃扫描——
    // 一个 TCP 连接都不发，和"TUN 一开就关掉组播兜底"是同一个病根。
    let networks = discovery_scan_networks();
    if networks.is_empty() {
        return Vec::new();
    }
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return Vec::new();
    };
    let targets = tvbox_scan_targets(&networks, 512);
    let queue = Arc::new(Mutex::new(targets));
    let found = Arc::new(Mutex::new(Vec::new()));
    // 单个探测的上限对齐 v5 的 0.45s：电视这类慢设备回一个根页面经常超过 140ms。
    // 之前不敢放宽是因为幻影主机会把这个上限整个吃光；现在非局域网连接在读之前就被
    // 放弃，扫描预算已经花在真实主机上，2500ms 的共享截止仍然由 tvbox_remaining_timeout 把关。
    let probe_cap = timeout.min(Duration::from_millis(450));
    // 只有从本机自己的局域网地址出去的连接才可能真的到达局域网接收端。
    let lan_sources: Arc<Vec<Ipv4Addr>> =
        Arc::new(networks.iter().map(|(item, _)| *item).collect());
    let workers = (0..64)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let found = Arc::clone(&found);
            let lan_sources = Arc::clone(&lan_sources);
            thread::spawn(move || loop {
                if tvbox_remaining_timeout(deadline, probe_cap).is_none() {
                    break;
                }
                let address = queue.lock().ok().and_then(|mut items| items.pop_front());
                let Some(address) = address else { break };
                if let Some(device) = probe_tvbox_until(address, &lan_sources, deadline, probe_cap)
                {
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

fn probe_tvbox_until(
    address: SocketAddr,
    lan_sources: &[Ipv4Addr],
    deadline: Instant,
    probe_cap: Duration,
) -> Option<CastDeviceInfo> {
    let connect_timeout = tvbox_remaining_timeout(deadline, probe_cap)?;
    let mut stream = TcpStream::connect_timeout(&address, connect_timeout).ok()?;
    // Clash/Mihomo 之类的 TUN 适配器会替不存在的目标在本地完成 TCP 握手，所以
    // "连上了"根本不能证明对端在局域网上：它可能只是一个幻影主机，甚至可能是隧道
    // 代理返回的页面。只有确认这条连接是从本机自己的某个局域网地址出去的，才继续
    // 当接收端探测；否则立刻放弃，既不浪费扫描预算，也不会误报出一个 TVBox。
    match stream.local_addr().ok()?.ip() {
        IpAddr::V4(local) if lan_sources.contains(&local) => {}
        _ => return None,
    }

    let write_timeout = tvbox_remaining_timeout(deadline, probe_cap)?;
    stream.set_write_timeout(Some(write_timeout)).ok()?;
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        address
    );
    stream.write_all(request.as_bytes()).ok()?;

    let mut body = Vec::with_capacity(1024);
    let mut buffer = [0u8; 1024];
    while body.len() < 8192 {
        let read_timeout = tvbox_remaining_timeout(deadline, probe_cap)?;
        stream.set_read_timeout(Some(read_timeout)).ok()?;
        let want = buffer.len().min(8192 - body.len());
        let count = stream.read(&mut buffer[..want]).ok()?;
        if count == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..count]);
    }

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
    // v5 起就有的一条规则：已知 TVBox 端口上的设备，即使根页面没有返回任何标记也要
    // 报出来。不少分支只回一个空白或极简页面，只靠标记匹配会把它们全部漏掉。
    // 反过来，非 TVBox 端口上的普通 HTTP 服务仍然必须有标记才会被当成接收端。
    if !matched && !TVBOX_PORTS.contains(&address.port()) {
        return None;
    }
    let label = if matched {
        "TVBox / 影视盒子"
    } else {
        "局域网设备"
    };
    let endpoint = format!("http://{address}");
    Some(CastDeviceInfo {
        id: format!("tvbox:{endpoint}"),
        label: label.into(),
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
            // 这里**不判局域网**。理由同 parse_mdns_chromecasts：这是个纯解析函数，
            // 掺进"算不算局域网"就得读本机网卡，于是同一段 XML 换个机器解析结果不同
            // （换一台不在该网段的机器，本来能用的电视就被解析成 None）。
            // 边界已经由真正的 I/O 关口守着：describe_device 抓取前判 LOCATION，
            // http_post_lan 发出 SOAP 前判 control_url。
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
    let _ = set_multicast_interface(&socket, interface);
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
    let dir = if base == origin_of(base) {
        base
    } else {
        base.rsplit_once('/').map(|(left, _)| left).unwrap_or(base)
    };
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
        // 注意：这里**不做**局域网过滤。解析器只负责把 DNS 报文翻译成设备，
        // 一旦掺进"这个 IP 算不算局域网"的判断，它就得去读网卡列表，单测会随机器
        // 变脸（换一台不在 192.168/16 的机器，以前能过的用例就红）。局域网过滤
        // 归调用方 discover_chromecasts_on 拿到设备之后统一做。
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
    let _ = set_multicast_interface(&socket, interface);
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
                    // 局域网过滤在这里做（而不是在解析器里，理由见 parse_mdns_chromecasts）。
                    // 只信 mDNS 报文里带的 A 记录地址——它就在我刚查询的那块网卡的
                    // 网段上，除非这机器还有第二块网卡也收到了同一份应答。
                    let reachable = device
                        .control_url
                        .rsplit_once(':')
                        .and_then(|(host, _)| host.parse::<Ipv4Addr>().ok())
                        .is_some_and(is_lan_ipv4);
                    if !reachable {
                        continue;
                    }
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
    let body = frame.get(4..4usize.checked_add(len)?)?;
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
            // `nlen` is a network-supplied varint. Without a checked add a length
            // near usize::MAX wraps in release builds and survives only because
            // `body.get(..)` then returns None; an overflow-checked build panics on
            // the add instead. Fail closed instead of relying on the wrap.
            let end = at.checked_add(nlen as usize)?;
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
    let mut out = proto_varint((field as u64) << 3);
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
    // The Chromecast certificate is self-signed, so chain verification can never
    // succeed and the leaf has to be accepted. It must not be accepted blindly:
    // pin the leaf, and refuse an endpoint whose certificate changed. See
    // check_cast_certificate.
    let endpoint = cast_tls_endpoint(host, port);
    let pinned = pinned_cast_fingerprint(&endpoint);
    let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stream = {
        let endpoint = endpoint.clone();
        let observed = Arc::clone(&observed);
        schannel::tls_stream::Builder::new()
            .domain(host)
            .verify_callback(move |validation| {
                let Some(fingerprint) = cast_certificate_fingerprint(&validation) else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Chromecast 未提供可用于固定的 TLS 证书",
                    ));
                };
                if let Err(error) =
                    check_cast_certificate(pinned.as_deref(), &endpoint, &fingerprint)
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        error,
                    ));
                }
                if let Ok(mut slot) = observed.lock() {
                    *slot = Some(fingerprint);
                }
                Ok(())
            })
            .connect(cred, raw)
    };
    let stream = stream.map_err(|error| error.to_string())?;
    if let Ok(guard) = observed.lock() {
        if let Some(fingerprint) = guard.as_ref() {
            remember_cast_fingerprint(&endpoint, fingerprint);
        }
    }
    Ok(stream)
}

/// SHA-256 fingerprint of the leaf certificate the server presented, uppercase hex.
#[cfg(windows)]
fn cast_certificate_fingerprint(
    validation: &schannel::tls_stream::CertValidationResult,
) -> Option<String> {
    let leaf = validation.chain()?.get(0)?;
    let digest = leaf
        .fingerprint(schannel::cert_context::HashAlgorithm::sha256())
        .ok()?;
    Some(digest.iter().map(|byte| format!("{byte:02X}")).collect())
}

/// Trust-on-first-use TLS pinning for Chromecast connections.
///
/// A Chromecast presents a self-signed certificate, so normal chain verification
/// can never succeed and the leaf has to be accepted. Accepting *every*
/// certificate is not the same thing: an on-path LAN attacker needs no trusted root
/// to impersonate a receiver, and the media URL, transportId and playback status all
/// travel over that channel.
///
/// The leaf certificate is therefore fingerprinted per endpoint. The first
/// connection pins it; a later connection to the same endpoint that presents a
/// different certificate is refused. See `check_cast_certificate`.
const CAST_TLS_PIN_FILE: &str = "cast-tls-pins.json";

fn cast_tls_pins() -> &'static Mutex<BTreeMap<String, String>> {
    static PINS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    PINS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The directory the Core database lives in, so a pin travels with its database.
fn cast_profile_root() -> Option<PathBuf> {
    crate::profile_paths::default_v7_database_path()
        .parent()
        .map(Path::to_path_buf)
}

fn cast_tls_endpoint(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

fn read_cast_pins(root: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(root.join(CAST_TLS_PIN_FILE)) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<BTreeMap<String, String>>(&text).unwrap_or_default()
}

/// Pins `fingerprint` for `endpoint`, replacing any previous value.
fn remember_cast_tls_pin(root: &Path, endpoint: &str, fingerprint: &str) {
    let mut pins = read_cast_pins(root);
    pins.insert(endpoint.to_string(), fingerprint.to_string());
    let Ok(text) = serde_json::to_string_pretty(&pins) else {
        return;
    };
    let path = root.join(CAST_TLS_PIN_FILE);
    if let Err(error) = std::fs::create_dir_all(root).and_then(|()| std::fs::write(&path, text)) {
        // Not fatal: the in-process map still refuses a certificate change for the
        // rest of this run.
        eprintln!(
            "v7 could not record the Chromecast TLS pin in {}: {error}",
            path.display()
        );
    }
}

/// The fingerprint pinned for `endpoint`, if this Core or a previous one saw it.
fn pinned_cast_fingerprint(endpoint: &str) -> Option<String> {
    if let Ok(guard) = cast_tls_pins().lock() {
        if let Some(fingerprint) = guard.get(endpoint) {
            return Some(fingerprint.clone());
        }
    }
    cast_profile_root().and_then(|root| read_cast_pins(&root).remove(endpoint))
}

fn remember_cast_fingerprint(endpoint: &str, fingerprint: &str) {
    if let Ok(mut guard) = cast_tls_pins().lock() {
        guard.insert(endpoint.to_string(), fingerprint.to_string());
    }
    if let Some(root) = cast_profile_root() {
        remember_cast_tls_pin(&root, endpoint, fingerprint);
    }
}

/// Decides whether a freshly observed Chromecast certificate may be trusted.
///
/// Kept free of I/O so the decision itself is testable. `pinned` is the previously
/// recorded fingerprint for the endpoint, if any.
fn check_cast_certificate(
    pinned: Option<&str>,
    endpoint: &str,
    fingerprint: &str,
) -> Result<(), String> {
    match pinned {
        Some(pinned) if pinned == fingerprint => Ok(()),
        Some(pinned) => Err(format!(
            "Chromecast {endpoint} 的 TLS 证书已变化（已固定 {pinned}，当前 {fingerprint}）。\
             若这是您重新配台的设备，请删除 Core 同目录下的 {CAST_TLS_PIN_FILE} 后重试。"
        )),
        None => Ok(()),
    }
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
        let server = MediaServer::start().unwrap();
        // 公网地址永远不能当投屏主机 / 被通告给电视 —— 这条判据和本机在哪个段无关。
        assert!(!is_lan_host("8.8.8.8"));
        assert!(!is_lan_host("1.1.1.1"));
        assert!(!is_lan_host("203.0.113.9"));
        // 云元数据地址同样拒绝：它不在任何一块真实网卡的网段上（link-local 本来就被
        // 排除在网卡列表之外），所以这条 SSRF 边界是按网卡收紧的，不是照抄私有段。
        assert!(!is_lan_host("169.254.169.254"));
        assert!(lan_media_url(&server, "t", "8.8.8.8").is_err());
    }

    // ===== 局域网判据：从网卡来，不从写死的网段来 =====
    //
    // 以前这些用例里写满了 192.168.1.8 / 10.0.0.2 / 172.16.1.9：它们能过，只是因为
    // 开发机恰好在 192.168.2.0/24 上。换个在 100.64/10（手机热点）或者 8.8.0.0/12
    // （公司内网）上跑 CI 的人，同一批断言全红。局域网判据必须变成"本机网卡说了算"，
    // 否则就是在用开发机的网络形态给所有用户下断言。

    #[test]
    fn lan_membership_comes_from_the_machine_not_a_range_table() {
        let networks = lan_ipv4_networks();
        if networks.is_empty() {
            eprintln!(
                "skip: this machine reports no usable LAN adapter, so there is nothing \
                 to assert membership against"
            );
            return;
        }
        // 同网段里的一个邻居（翻转最后一位，一定不是本机自己，也一定在 /30 以内）。
        let (own, _) = networks[0];
        let neighbour = Ipv4Addr::from(u32::from(own) ^ 1);
        assert!(
            is_lan_host(&own.to_string()),
            "本机自己的地址必须是局域网成员：{own}"
        );
        assert!(
            is_lan_host(&neighbour.to_string()),
            "同一网段的邻居必须是局域网成员：{neighbour}（本机 {own}）"
        );
        assert!(is_lan_ipv4(neighbour));
        assert!(endpoint_lan_ipv4(&format!("{neighbour}:9978")).is_some());
    }
    #[test]
    fn a_tunnel_only_machine_is_not_treated_as_a_dead_end() {
        // lan_ipv4_state() 会把隧道网卡整个过滤掉，所以"本机只剩 TUN"时送进
        // 来的就是空表。以前空表还要再看一眼 !tun_mode_active() 才兜底，于是
        // TUN 一开兜底也没了。现在兜底与 TUN 状态彻底脱钩：只要没有可用的
        // 物理地址，就一定退回 0.0.0.0。
        let only_a_tunnel_adapter: Vec<(Ipv4Addr, u8)> = Vec::new();
        assert_eq!(
            effective_discovery_interfaces(&only_a_tunnel_adapter),
            vec![Ipv4Addr::UNSPECIFIED],
            "只剩隧道网卡也必须有一个能发发现请求的地址"
        );
        // 198.18.0.0/15 是各家 TUN 客户端最爱用的段，Rust 的 is_private() 不认它，
        // 所以它本来就进不来；这里把"它偶然出现在输入里"也一并钉住：照样不改变
        // "空表必须有兜底"这条规则——有地址就按地址发，没有就兜底。
        let tun_address = [(Ipv4Addr::new(198, 18, 0, 1), 24u8)];
        assert_eq!(
            effective_discovery_interfaces(&tun_address),
            vec![Ipv4Addr::new(198, 18, 0, 1)]
        );
    }

    // ===== 设备发现的网卡来源：TUN 开关都不能让它失灵 =====
    //
    // 这三个用例盯的是同一处病根的两半：以前 `discovery_interface_addresses()` 是
    // `addresses.is_empty() && !tun_mode_active()` 才兜底，`discover_tvboxes()` 又是
    // `networks.is_empty()` 就 return。于是"TUN 开着 + 物理地址枚举不到"时，
    // M-SEARCH、mDNS 查询、SSDP 通告、TCP 扫描四条路同时归零——电视和盒子
    // 一个都找不到，而 UI 只说"没有发现设备"。

    #[test]
    fn a_machine_with_no_usable_physical_address_still_has_a_discovery_interface() {
        // 空表必须兜到 0.0.0.0，让操作系统挑出口，而不是一条请求都不发。
        assert_eq!(
            effective_discovery_interfaces(&[]),
            vec![Ipv4Addr::UNSPECIFIED]
        );
    }

    #[test]
    fn discovery_interfaces_cover_every_physical_address_in_a_stable_order() {
        // Wi-Fi + 有线双网卡：两个地址都要各建一个 socket，重复项去重，顺序固定
        // （测试才对结果稳定，不用依赖网卡枚举顺序）。
        let networks = [
            (Ipv4Addr::new(192, 168, 1, 20), 24u8),
            (Ipv4Addr::new(10, 0, 0, 5), 16u8),
            (Ipv4Addr::new(192, 168, 1, 20), 24u8),
        ];
        assert_eq!(
            effective_discovery_interfaces(&networks),
            vec![Ipv4Addr::new(10, 0, 0, 5), Ipv4Addr::new(192, 168, 1, 20)]
        );
    }

    #[test]
    fn tvbox_sweep_keeps_targets_when_no_physical_interface_qualifies() {
        // 物理地址为空时必须退回路由探测拿到的本机地址（按 /24 推同网段主机），
        // 不能一个 TCP 连接都不发。
        let routed = Some(Ipv4Addr::new(192, 168, 31, 7));
        assert_eq!(
            effective_scan_networks(&[], routed),
            vec![(Ipv4Addr::new(192, 168, 31, 7), 24u8)]
        );
        // 有物理地址时优先用物理地址，路由探测只作兜底。
        let physical = vec![(Ipv4Addr::new(10, 1, 2, 3), 24u8)];
        assert_eq!(effective_scan_networks(&physical, routed), physical);
        // 连路由探测都没有时允许为空——此时 SSDP/mDNS 的 0.0.0.0 兜底仍然在跑。
        assert!(effective_scan_networks(&[], None).is_empty());
    }

    #[test]
    fn tvbox_mode_still_listens_to_ssdp() {
        // "投屏能找到、TVBox 找不到"的直接原因：tvbox 模式把 SSDP 也关了，
        // 只剩 9976-9979 四个裸端口。靠 SSDP 通告的盒子因此一律看不见。
        let tvbox = discovery_scan_plan("tvbox");
        assert!(
            tvbox.ssdp,
            "tvbox 模式必须听 SSDP，盒子的通告就在这条通道上"
        );
        assert!(!tvbox.chromecast);
        assert!(tvbox.tvbox_sweep);
        assert_eq!(
            discovery_scan_plan("tvbox"),
            DiscoveryPlan {
                ssdp: true,
                chromecast: false,
                tvbox_sweep: true
            }
        );
    }

    #[test]
    fn cast_and_default_modes_keep_the_full_scan() {
        assert_eq!(
            discovery_scan_plan("cast"),
            DiscoveryPlan {
                ssdp: true,
                chromecast: true,
                tvbox_sweep: false
            }
        );
        assert_eq!(
            discovery_scan_plan(""),
            DiscoveryPlan {
                ssdp: true,
                chromecast: true,
                tvbox_sweep: true
            }
        );
        assert_eq!(
            discovery_scan_plan(" TVBox "),
            DiscoveryPlan {
                ssdp: true,
                chromecast: false,
                tvbox_sweep: true
            }
        );
    }

    // 这条只验"地址从哪种端点写法里被抠出来"，不掺局域网判断（那部分由
    // lan_membership_comes_from_the_machine_not_a_range_table 覆盖）。
    // 端点里的具体数字换成本机真实网段，否则又是拿开发机的网络形态当通用断言。
    #[test]
    fn extracts_receiver_addresses_across_cast_protocols() {
        let networks = lan_ipv4_networks();
        let Some((own, _)) = networks.first().copied() else {
            eprintln!("skip: no LAN adapter to derive a peer address from");
            return;
        };
        let peer = Ipv4Addr::from(u32::from(own) ^ 1);
        assert_eq!(
            peer_ipv4_from_endpoint(&format!("http://{peer}:8008/upnp/control/AVTransport")),
            Some(peer)
        );
        assert_eq!(peer_ipv4_from_endpoint(&format!("{peer}:8009")), Some(peer));
        assert_eq!(
            peer_ipv4_from_endpoint(&format!("http://{peer}:9978/action")),
            Some(peer)
        );
        // 公网地址：不解析成本地对端（和本机在哪一段无关，永远成立）。
        assert_eq!(peer_ipv4_from_endpoint("http://8.8.8.8:80/action"), None);
        assert_eq!(peer_ipv4_from_endpoint("not-an-endpoint"), None);
        assert_eq!(routed_lan_ipv4(Ipv4Addr::new(8, 8, 8, 8)), None);
        assert_eq!(routed_lan_ipv4(Ipv4Addr::LOCALHOST), None);

        let chromecast = CastDeviceInfo {
            id: "chromecast:kitchen".into(),
            label: "Kitchen".into(),
            location: format!("https://{peer}:8009"),
            control_url: format!("{peer}:8009"),
            service_type: "chromecast".into(),
        };
        assert_eq!(device_peer_ipv4(&chromecast), Some(peer));
    }

    // address_is_on_link 是纯函数，所以下面这批**与机器无关**：无论 CI 跑在哪里，
    // 断言结果都一样。这才配得上叫回归测试。
    #[test]
    fn a_host_on_a_hotspot_segment_counts_as_lan_while_that_host_is_the_machine() {
        // 手机热点 / 蜂窝 CGNAT 分下来的 100.64/10。以前整条链路都拒绝它。
        let hotspot = [(Ipv4Addr::new(100, 64, 12, 5), 10u8)];
        assert!(address_is_on_link(&hotspot, Ipv4Addr::new(100, 64, 12, 5)));
        assert!(address_is_on_link(&hotspot, Ipv4Addr::new(100, 64, 99, 7)));
        assert!(address_is_on_link(
            &hotspot,
            Ipv4Addr::new(100, 127, 255, 254)
        ));
        assert!(!address_is_on_link(&hotspot, Ipv4Addr::new(100, 128, 0, 1)));
        assert!(!address_is_on_link(&hotspot, Ipv4Addr::new(192, 168, 1, 5)));
        assert!(!address_is_on_link(&hotspot, Ipv4Addr::new(8, 8, 8, 8)));
    }

    #[test]
    fn an_ordinary_home_router_segment_still_works() {
        let home = [(Ipv4Addr::new(192, 168, 2, 6), 24u8)];
        assert!(address_is_on_link(&home, Ipv4Addr::new(192, 168, 2, 5)));
        assert!(address_is_on_link(&home, Ipv4Addr::new(192, 168, 2, 6)));
        // 隔壁网段的电视：不在我任何网卡上，就不算局域网。
        assert!(!address_is_on_link(&home, Ipv4Addr::new(192, 168, 1, 5)));
        assert!(!address_is_on_link(&home, Ipv4Addr::new(192, 168, 3, 5)));
    }

    #[test]
    fn every_network_this_machine_holds_is_listed_not_just_the_first() {
        // Wi-Fi + 有线同时在线：两块网卡的网段都要算局域网。
        let dual = [
            (Ipv4Addr::new(192, 168, 2, 6), 24u8),
            (Ipv4Addr::new(10, 5, 0, 7), 24u8),
        ];
        assert!(address_is_on_link(&dual, Ipv4Addr::new(192, 168, 2, 9)));
        assert!(address_is_on_link(&dual, Ipv4Addr::new(10, 5, 0, 200)));
        assert!(!address_is_on_link(&dual, Ipv4Addr::new(10, 6, 0, 1)));
        // 空表：没有任何网卡时谁都不是局域网（而不是"谁都算"）。
        assert!(!address_is_on_link(&[], Ipv4Addr::new(192, 168, 2, 5)));
    }

    #[test]
    fn cloud_metadata_and_other_link_local_peers_are_never_lan_members() {
        // 169.254.169.254 是云元数据地址。它既不在用户网段里，也不该被
        // "link-local 算局域网"这种历史判断放进来。
        let machine = [(Ipv4Addr::new(100, 64, 12, 5), 10u8)];
        assert!(!address_is_on_link(
            &machine,
            Ipv4Addr::new(169, 254, 169, 254)
        ));
        // 即便网卡列表里混进了一个 link-local 地址，元数据地址也不算同网段 ——
        // 这条由判据自己出局保证，不依赖上游过滤（判据是纯函数，边界必须自带）。
        let polluted = [
            (Ipv4Addr::new(192, 168, 2, 6), 24u8),
            (Ipv4Addr::new(169, 254, 1, 1), 16u8),
        ];
        assert!(!address_is_on_link(
            &polluted,
            Ipv4Addr::new(169, 254, 169, 254)
        ));
        // 多播组地址和 0.0.0.0 也一样：不能因为某块网卡在 224.0.0.251 邻域就放行。
        assert!(!address_is_on_link(&polluted, Ipv4Addr::UNSPECIFIED));
        assert!(!address_is_on_link(&polluted, Ipv4Addr::BROADCAST));
    }

    #[test]
    fn prefix_length_changes_what_counts_as_the_same_network() {
        // /10 和 /24 的同一个起点，覆盖范围完全不同——判据用的是网卡报的前缀，
        // 不是猜一个"常见掩码"。10.0.0.0/10 覆盖 10.0.0.0–10.63.255.255，
        // 所以 10.63.x 在里面、10.64.x 在外面（第二段 63=0b00111111 / 64=0b01000000，
        // 头两位就是分界）。
        let wide = [(Ipv4Addr::new(10, 1, 2, 3), 10u8)];
        assert!(address_is_on_link(&wide, Ipv4Addr::new(10, 63, 7, 7)));
        assert!(address_is_on_link(&wide, Ipv4Addr::new(10, 1, 2, 99)));
        assert!(!address_is_on_link(&wide, Ipv4Addr::new(10, 64, 0, 1)));
        assert!(!address_is_on_link(&wide, Ipv4Addr::new(10, 200, 7, 7)));
        let narrow = [(Ipv4Addr::new(10, 1, 2, 3), 24u8)];
        assert!(address_is_on_link(&narrow, Ipv4Addr::new(10, 1, 2, 99)));
        assert!(!address_is_on_link(&narrow, Ipv4Addr::new(10, 1, 3, 99)));
    }

    #[test]
    fn tun_mode_keeps_receiver_traffic_on_the_physical_lan() {
        let networks = [(Ipv4Addr::new(192, 168, 2, 6), 24)];
        assert!(tunnel_adapter_name("Mihomo", "Meta Tunnel"));
        assert!(tunnel_adapter_name("Ethernet 3", "WireGuard Tunnel"));
        assert!(!tunnel_adapter_name(
            "WLAN",
            "Intel(R) Dual Band Wireless-AC 3168"
        ));
        assert_eq!(
            physical_lan_source(
                &networks,
                Ipv4Addr::new(192, 168, 2, 5),
                Some(Ipv4Addr::new(198, 18, 0, 1))
            ),
            Some(Ipv4Addr::new(192, 168, 2, 6))
        );
        assert_eq!(
            physical_lan_source(
                &networks,
                Ipv4Addr::new(10, 0, 0, 8),
                Some(Ipv4Addr::new(198, 18, 0, 1))
            ),
            None
        );
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

        let bare_origin = xml
            .replace("http://192.168.1.20:8008/", "http://192.168.1.20:8008")
            .replace(
                "/upnp/control/AVTransport",
                "_urn:schemas-upnp-org:service:AVTransport_control",
            );
        let device =
            parse_device_description(&bare_origin, "http://192.168.1.20:8008/desc.xml").unwrap();
        assert_eq!(
            device.control_url,
            "http://192.168.1.20:8008/_urn:schemas-upnp-org:service:AVTransport_control"
        );
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
    fn cast_v2_frame_rejects_a_length_that_would_overflow() {
        // A field length near usize::MAX must be refused, not wrapped. `at + nlen`
        // wraps in release builds and survives only because `body.get(..)` then
        // returns None; an overflow-checked build panics on the add instead. The
        // checked add makes the refusal deliberate.
        let mut frame = 15u32.to_be_bytes().to_vec();
        // tag: field 4, wire type 2 (0x22)
        frame.push(0x22);
        // a complete varint for u64::MAX
        frame.extend_from_slice(&[0xFF; 9]);
        frame.push(0x01);
        frame.extend_from_slice(b"abcd");
        assert_eq!(frame.len() - 4, 15);
        assert!(decode_cast_payload(&frame).is_none());

        // A frame body that claims to be longer than the buffer is refused too.
        let mut frame = 64u32.to_be_bytes().to_vec();
        frame.extend_from_slice(b"short");
        assert!(decode_cast_payload(&frame).is_none());
    }

    #[test]
    fn a_changed_chromecast_certificate_is_refused() {
        // The whole point of pinning: blanket acceptance accepted any certificate,
        // which is exactly what an on-path LAN attacker presents.
        let endpoint = "192.168.2.11:8009";
        assert!(check_cast_certificate(None, endpoint, "AAAA").is_ok());
        assert!(check_cast_certificate(Some("AAAA"), endpoint, "AAAA").is_ok());
        let error = check_cast_certificate(Some("AAAA"), endpoint, "BBBB").unwrap_err();
        assert!(error.contains("TLS 证书已变化"), "{error}");
        assert!(error.contains(CAST_TLS_PIN_FILE), "{error}");
        // A re-paired device must be recoverable, and the message has to say how.
        assert!(error.contains("删除"), "{error}");
    }

    #[test]
    fn the_chromecast_handshake_does_not_blanket_accept_certificates() {
        // A canary for the security invariant itself, which cannot be exercised
        // without a real receiver: the Schannel callback must consult the pin, and
        // must never be a bare `Ok(())`. Reintroducing blanket acceptance fails here
        // instead of silently opening the cast channel to an on-path LAN attacker.
        let source = include_str!("cast.rs");
        // The needles are assembled at runtime so this test cannot match its own
        // literals, which would make every assertion vacuously true.
        let blanket_accept = ["verify_callback(|_| Ok(", "())"].concat();
        let pin_check = ["check_cast_", "certificate("].concat();
        let fingerprint = ["cast_certificate_", "fingerprint("].concat();
        assert!(
            !source.contains(&blanket_accept),
            "the Chromecast TLS handshake accepts every certificate again"
        );
        assert!(
            source.contains(&pin_check),
            "the Chromecast TLS handshake no longer checks the pinned certificate"
        );
        assert!(
            source.contains(&fingerprint),
            "the Chromecast TLS handshake no longer fingerprints the certificate"
        );
    }

    #[test]
    fn chromecast_pins_survive_a_restart_and_ignore_corruption() {
        let dir = std::env::temp_dir().join(format!(
            "hls-cast-pins-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read_cast_pins(&dir).is_empty());

        remember_cast_tls_pin(&dir, "192.168.2.11:8009", "AAAA");
        remember_cast_tls_pin(&dir, "192.168.2.12:8009", "BBBB");
        let pins = read_cast_pins(&dir);
        assert_eq!(
            pins.get("192.168.2.11:8009").map(String::as_str),
            Some("AAAA")
        );

        // Re-pinning replaces, never duplicates.
        remember_cast_tls_pin(&dir, "192.168.2.11:8009", "CCCC");
        assert_eq!(read_cast_pins(&dir).len(), 2);
        assert_eq!(
            read_cast_pins(&dir)
                .get("192.168.2.11:8009")
                .map(String::as_str),
            Some("CCCC")
        );

        // A truncated write must not wedge the next connection.
        std::fs::write(dir.join(CAST_TLS_PIN_FILE), b"{ not json").unwrap();
        assert!(read_cast_pins(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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
    fn tvbox_http_response_rejects_truncated_content_length_at_eof() {
        use std::net::TcpListener;

        // A valid-looking success body (or an empty one) is not a complete
        // response when the receiver promised more bytes before closing.
        for body in ["", r#"{"ok":true}"#] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/action", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len() + 1
                )
                .unwrap();
            });

            let response = tvbox_http_request("GET", &endpoint, "", Duration::from_secs(2));
            server.join().unwrap();
            assert_eq!(
                response.unwrap_err(),
                "TVBox 返回了不完整 Content-Length 响应"
            );
        }
    }

    #[test]
    fn tvbox_http_response_preserves_zero_length_and_eof_framing() {
        use std::net::TcpListener;

        for (length_header, body) in [("Content-Length: 0\r\n", ""), ("", r#"{"ok":true}"#)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/action", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\n{length_header}Connection: close\r\n\r\n{body}"
                )
                .unwrap();
            });

            let response = tvbox_http_request("GET", &endpoint, "", Duration::from_secs(2));
            server.join().unwrap();
            let response = response.unwrap();
            assert_eq!(response.status, 200);
            assert_eq!(response.body, body);
        }
    }

    #[test]
    fn tvbox_http_response_finishes_at_content_length_without_eof() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/action", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let body = r#"{"ok":true}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            thread::sleep(Duration::from_millis(450));
        });

        let started = Instant::now();
        let response =
            tvbox_http_request("GET", &endpoint, "", Duration::from_millis(180)).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, r#"{"ok":true}"#);
        assert!(
            elapsed < Duration::from_millis(350),
            "TVBox response waited for EOF after Content-Length: {elapsed:?}"
        );
        server.join().unwrap();
    }

    #[test]
    fn tvbox_http_response_decodes_chunked_without_eof() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/action", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Transfer-Encoding: Chunked\r\n",
                "Connection: keep-alive\r\n",
                "\r\n",
                "6;source=tvbox\r\n{\"ok\":\r\n",
                "4\r\ntrue\r\n",
                "1\r\n}\r\n",
                "0\r\nX-TVBox-Test: complete\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).unwrap();
            thread::sleep(Duration::from_millis(450));
        });

        let started = Instant::now();
        let response =
            tvbox_http_request("GET", &endpoint, "", Duration::from_millis(180)).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, r#"{"ok":true}"#);
        assert!(
            elapsed < Duration::from_millis(350),
            "TVBox chunked response waited for EOF: {elapsed:?}"
        );
        server.join().unwrap();
    }

    #[test]
    fn tvbox_http_response_rejects_unknown_transfer_encoding() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/action", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\nConnection: close\r\n\r\nbody",
                )
                .unwrap();
        });

        let error = tvbox_http_request("GET", &endpoint, "", Duration::from_secs(1)).unwrap_err();
        assert_eq!(error, "TVBox 返回了不支持的 Transfer-Encoding");
        server.join().unwrap();
    }

    #[test]
    fn tvbox_http_response_has_total_read_deadline() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/action", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\na");
            for byte in *b"bcd" {
                thread::sleep(Duration::from_millis(90));
                let _ = stream.write_all(&[byte]);
            }
        });

        let started = Instant::now();
        let error =
            tvbox_http_request("GET", &endpoint, "", Duration::from_millis(150)).unwrap_err();
        let elapsed = started.elapsed();
        assert_eq!(error, "读取 TVBox 响应超时");
        assert!(
            elapsed < Duration::from_millis(400),
            "TVBox response read exceeded total deadline: {elapsed:?}"
        );
        server.join().unwrap();
    }

    #[test]
    fn tvbox_http_response_is_bounded() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/action", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let body = vec![b'x'; MAX_TVBOX_HTTP_RESPONSE_BYTES + 4096];
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(&body);
        });

        let error = tvbox_http_request("GET", &endpoint, "", Duration::from_secs(1)).unwrap_err();
        assert_eq!(error, "TVBox 响应过大");
        server.join().unwrap();
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
        let deadline = Instant::now().checked_add(Duration::from_secs(1)).unwrap();
        assert!(probe_tvbox_until(
            generic,
            &[Ipv4Addr::LOCALHOST],
            deadline,
            Duration::from_secs(1)
        )
        .is_none());
        generic_worker.join().unwrap();
        let (tvbox, tvbox_worker) = serve("TVBox player /action push");
        let device = probe_tvbox_until(
            tvbox,
            &[Ipv4Addr::LOCALHOST],
            deadline,
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(device.service_type, "tvbox");
        tvbox_worker.join().unwrap();
    }

    #[test]
    fn tvbox_probe_budget_uses_shared_deadline_and_cap() {
        let expired = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        assert!(tvbox_remaining_timeout(expired, Duration::from_millis(140)).is_none());

        let future = Instant::now().checked_add(Duration::from_secs(2)).unwrap();
        let budget = tvbox_remaining_timeout(future, Duration::from_millis(140)).unwrap();
        assert!(budget > Duration::ZERO);
        assert!(budget <= Duration::from_millis(140));
    }

    #[test]
    fn tvbox_scan_targets_prioritize_local_24_on_broad_subnet() {
        let local = Ipv4Addr::new(192, 168, 50, 42);
        let targets = tvbox_scan_targets(&[(local, 16)], 128);
        assert_eq!(targets.len(), 128 * TVBOX_PORTS.len());
        assert!(targets.iter().all(|target| match target.ip() {
            IpAddr::V4(ip) => {
                let octets = ip.octets();
                octets[0] == 192 && octets[1] == 168 && octets[2] == 50
            }
            IpAddr::V6(_) => false,
        }));
        assert!(targets
            .iter()
            .any(|target| target.ip() == IpAddr::V4(Ipv4Addr::new(192, 168, 50, 100))));
        assert!(targets
            .iter()
            .all(|target| target.ip() != IpAddr::V4(local)));
    }

    #[test]
    fn tvbox_scan_targets_share_budget_across_large_subnets() {
        let networks = [
            (Ipv4Addr::new(192, 168, 10, 42), 24),
            (Ipv4Addr::new(10, 20, 30, 40), 24),
        ];
        let targets = tvbox_scan_targets(&networks, 8);
        let hosts = targets.iter().map(SocketAddr::ip).collect::<HashSet<_>>();

        assert_eq!(targets.len(), 8 * TVBOX_PORTS.len());
        assert_eq!(hosts.len(), 8);
        assert_eq!(
            hosts
                .iter()
                .filter(|ip| match **ip {
                    IpAddr::V4(value) => value.octets()[0..3] == [192, 168, 10],
                    IpAddr::V6(_) => false,
                })
                .count(),
            4
        );
        assert_eq!(
            hosts
                .iter()
                .filter(|ip| match **ip {
                    IpAddr::V4(value) => value.octets()[0..3] == [10, 20, 30],
                    IpAddr::V6(_) => false,
                })
                .count(),
            4
        );
        assert!(!hosts.contains(&IpAddr::V4(networks[0].0)));
        assert!(!hosts.contains(&IpAddr::V4(networks[1].0)));
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

    /// 起一个只回一次的最小 HTTP 响应，响应体里带 TVBox 标记。
    fn spawn_tvbox_responder() -> (SocketAddr, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // 先读请求再回包：和真实 HTTP 服务端一致，也避免探测端的 write
                // 撞上"对端已关闭"而失败。
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let body = "tvbox";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (address, worker)
    }

    #[test]
    fn tvbox_probe_accepts_a_connection_that_left_via_the_lan() {
        let (address, worker) = spawn_tvbox_responder();
        let deadline = Instant::now() + Duration::from_secs(5);
        let device = probe_tvbox_until(
            address,
            &[Ipv4Addr::LOCALHOST],
            deadline,
            Duration::from_secs(2),
        );
        assert!(
            device.is_some(),
            "从本机局域网地址出去的探测必须被当成接收端"
        );
        assert!(device.unwrap().id.starts_with("tvbox:"));
        worker.join().unwrap();
    }

    #[test]
    fn tvbox_probe_rejects_a_connection_that_did_not_leave_via_the_lan() {
        // Clash/Mihomo TUN 会替不存在的目标在本地完成握手：连接确实建立，但本地端点
        // 不是本机的局域网地址。这种情况必须放弃，否则会把隧道/代理误报成 TVBox，
        // 并且让整个扫描预算耗在幻影主机上。
        let (address, worker) = spawn_tvbox_responder();
        let declared_lan = Ipv4Addr::new(192, 168, 1, 10);
        let deadline = Instant::now() + Duration::from_secs(5);
        let device = probe_tvbox_until(address, &[declared_lan], deadline, Duration::from_secs(2));
        assert!(
            device.is_none(),
            "没有从本机局域网地址出去的连接不能被当成接收端"
        );
        worker.join().unwrap();
    }

    /// 绑到某个已知 TVBox 端口上，回一个没有任何标记的 200 页面。
    fn serve_tvbox_port_without_marker() -> Option<(SocketAddr, thread::JoinHandle<()>)> {
        let mut listener = None;
        for port in TVBOX_PORTS {
            if let Ok(bound) = TcpListener::bind(("127.0.0.1", port)) {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener?;
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let body = "";
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        Some((address, worker))
    }

    #[test]
    fn tvbox_probe_reports_a_known_tvbox_port_without_a_marker() {
        // v5 起就有这条规则：已知 TVBox 端口上的设备即使根页面没有任何标记也要报出来，
        // 标签降级成"局域网设备"。只靠标记匹配会把只回空白页的分支全部漏掉，
        // 这正是用户反馈"以前的版本能找到"的那一条。
        let Some((address, worker)) = serve_tvbox_port_without_marker() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let device = probe_tvbox_until(
            address,
            &[Ipv4Addr::LOCALHOST],
            deadline,
            Duration::from_secs(2),
        );
        assert!(
            device.is_some(),
            "已知 TVBox 端口上的设备不能被空白页面漏掉"
        );
        let device = device.unwrap();
        assert_eq!(device.service_type, "tvbox");
        assert_eq!(device.label, "局域网设备");
        assert!(device.id.starts_with("tvbox:"));
        worker.join().unwrap();
    }

    #[test]
    fn tvbox_probe_still_rejects_a_plain_http_service_on_another_port() {
        // 非 TVBox 端口上的普通 HTTP 服务仍然必须有标记，否则整个局域网的服务端
        // 都会被当成投屏目标列出来。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        assert!(!TVBOX_PORTS.contains(&address.port()));
        let worker = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let device = probe_tvbox_until(
            address,
            &[Ipv4Addr::LOCALHOST],
            deadline,
            Duration::from_secs(2),
        );
        assert!(
            device.is_none(),
            "非 TVBox 端口且没有标记的服务不能被当成接收端"
        );
        worker.join().unwrap();
    }
}
