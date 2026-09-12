from pathlib import Path

path = Path("native_shell/src/cast.rs")
text = path.read_text(encoding="utf-8")

start = text.index("fn tvbox_scan_targets(networks: &[(Ipv4Addr, u8)], max_hosts: usize) -> VecDeque<SocketAddr> {")
end = text.index("\nfn discover_tvboxes(timeout: Duration) -> Vec<CastDeviceInfo> {", start)

replacement = r'''fn tvbox_network_hosts(
    local: Ipv4Addr,
    prefix: u8,
    max_hosts: usize,
) -> VecDeque<Ipv4Addr> {
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
'''

text = text[:start] + replacement + text[end:]

anchor = '''    #[test]\n    fn tvbox_scan_targets_cover_multiple_subnets_without_scanning_self() {\n'''
test = r'''    #[test]
    fn tvbox_scan_targets_share_budget_across_large_subnets() {
        let networks = [
            (Ipv4Addr::new(192, 168, 10, 42), 24),
            (Ipv4Addr::new(10, 20, 30, 40), 24),
        ];
        let targets = tvbox_scan_targets(&networks, 8);
        let hosts = targets
            .iter()
            .map(SocketAddr::ip)
            .collect::<HashSet<_>>();

        assert_eq!(targets.len(), 8 * TVBOX_PORTS.len());
        assert_eq!(hosts.len(), 8);
        assert_eq!(
            hosts
                .iter()
                .filter(|ip| matches!(ip, IpAddr::V4(value) if value.octets()[0..3] == [192, 168, 10]))
                .count(),
            4
        );
        assert_eq!(
            hosts
                .iter()
                .filter(|ip| matches!(ip, IpAddr::V4(value) if value.octets()[0..3] == [10, 20, 30]))
                .count(),
            4
        );
        assert!(hosts.iter().all(|ip| !networks.iter().any(|(local, _)| *ip == IpAddr::V4(*local))));
    }

'''
if anchor not in text:
    raise SystemExit("TVBox multi-subnet test anchor missing")
text = text.replace(anchor, test + anchor, 1)

path.write_text(text, encoding="utf-8")
