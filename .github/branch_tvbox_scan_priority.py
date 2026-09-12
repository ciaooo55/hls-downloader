from pathlib import Path

path = Path("native_shell/src/cast.rs")
text = path.read_text(encoding="utf-8")

old_import = "use std::collections::{HashMap, VecDeque};"
new_import = "use std::collections::{HashMap, HashSet, VecDeque};"
if text.count(old_import) != 1:
    raise SystemExit(f"import anchor count: {text.count(old_import)}")
text = text.replace(old_import, new_import, 1)

old_function = '''fn tvbox_scan_targets(networks: &[(Ipv4Addr, u8)], max_hosts: usize) -> VecDeque<SocketAddr> {
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
'''
new_function = '''fn tvbox_scan_targets(networks: &[(Ipv4Addr, u8)], max_hosts: usize) -> VecDeque<SocketAddr> {
    let mut targets = VecDeque::new();
    let mut seen = HashSet::new();
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
        let mut ranges = Vec::with_capacity(2);

        // A /16 or wider interface can contain tens of thousands of hosts. The
        // bounded TVBox probe must start near the machine instead of spending
        // its entire host budget at the beginning of that large subnet.
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
                if remaining_hosts == 0 {
                    break;
                }
                if candidate == local_u32 {
                    continue;
                }
                let host = Ipv4Addr::from(candidate);
                if !seen.insert(host) {
                    continue;
                }
                for port in TVBOX_PORTS {
                    targets.push_back(SocketAddr::from((host, port)));
                }
                remaining_hosts -= 1;
            }
            if remaining_hosts == 0 {
                break;
            }
        }
    }
    targets
}
'''
if text.count(old_function) != 1:
    raise SystemExit(f"function anchor count: {text.count(old_function)}")
text = text.replace(old_function, new_function, 1)

marker = '''    #[test]
    fn tvbox_scan_targets_cover_multiple_subnets_without_scanning_self() {
'''
regression = '''    #[test]
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

'''
if text.count(marker) != 1:
    raise SystemExit(f"test marker count: {text.count(marker)}")
text = text.replace(marker, regression + marker, 1)

path.write_text(text, encoding="utf-8")
