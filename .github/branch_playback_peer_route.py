from pathlib import Path

cast = Path("native_shell/src/cast.rs")
text = cast.read_text(encoding="utf-8")
old = "fn routed_lan_ipv4(peer: Ipv4Addr) -> Option<Ipv4Addr> {"
new = "pub fn routed_lan_ipv4(peer: Ipv4Addr) -> Option<Ipv4Addr> {"
if text.count(old) != 1:
    raise SystemExit(f"routed_lan_ipv4 visibility anchor count={text.count(old)}")
text = text.replace(old, new, 1)

marker = '''        assert_eq!(peer_ipv4_from_endpoint("http://8.8.8.8:80/action"), None);
        assert_eq!(peer_ipv4_from_endpoint("not-an-endpoint"), None);
'''
addition = marker + '''        assert_eq!(routed_lan_ipv4(Ipv4Addr::new(8, 8, 8, 8)), None);
        assert_eq!(routed_lan_ipv4(Ipv4Addr::LOCALHOST), None);
'''
if text.count(marker) != 1:
    raise SystemExit(f"route rejection test anchor count={text.count(marker)}")
text = text.replace(marker, addition, 1)
cast.write_text(text, encoding="utf-8")

playback = Path("native_shell/src/playback.rs")
text = playback.read_text(encoding="utf-8")
old = '''fn advertise_host(peer: Option<IpAddr>, port: u16) -> String {
    match peer {
        Some(IpAddr::V4(ip)) if ip.is_loopback() => format!("127.0.0.1:{port}"),
        Some(IpAddr::V6(ip)) if ip.is_loopback() => format!("127.0.0.1:{port}"),
        Some(_) => crate::cast::primary_lan_ipv4()
            .map(|ip| format!("{ip}:{port}"))
            .unwrap_or_else(|| format!("127.0.0.1:{port}")),
        None => format!("127.0.0.1:{port}"),
    }
}
'''
new = '''fn advertise_host(peer: Option<IpAddr>, port: u16) -> String {
    match peer {
        Some(IpAddr::V4(ip)) if ip.is_loopback() => format!("127.0.0.1:{port}"),
        Some(IpAddr::V6(ip)) if ip.is_loopback() => format!("127.0.0.1:{port}"),
        Some(IpAddr::V4(peer)) => crate::cast::routed_lan_ipv4(peer)
            .or_else(crate::cast::preferred_lan_ipv4)
            .map(|ip| format!("{ip}:{port}"))
            .unwrap_or_else(|| format!("127.0.0.1:{port}")),
        Some(IpAddr::V6(_)) => crate::cast::preferred_lan_ipv4()
            .map(|ip| format!("{ip}:{port}"))
            .unwrap_or_else(|| format!("127.0.0.1:{port}")),
        None => format!("127.0.0.1:{port}"),
    }
}
'''
if text.count(old) != 1:
    raise SystemExit(f"advertise_host anchor count={text.count(old)}")
text = text.replace(old, new, 1)
playback.write_text(text, encoding="utf-8")
