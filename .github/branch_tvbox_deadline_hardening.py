from pathlib import Path

path = Path("native_shell/src/cast.rs")
text = path.read_text(encoding="utf-8")

start = text.index("fn discover_tvboxes(timeout: Duration) -> Vec<CastDeviceInfo> {")
end = text.index("\npub fn play_on_device(device_id: &str, media_url: &str, title: &str) -> Result<String, String> {", start)

replacement = r'''fn tvbox_remaining_timeout(deadline: Instant, cap: Duration) -> Option<Duration> {
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
    let networks = lan_ipv4_networks();
    if networks.is_empty() {
        return Vec::new();
    }
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return Vec::new();
    };
    let targets = tvbox_scan_targets(&networks, 512);
    let queue = Arc::new(Mutex::new(targets));
    let found = Arc::new(Mutex::new(Vec::new()));
    let probe_cap = timeout.min(Duration::from_millis(140));
    let workers = (0..64)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let found = Arc::clone(&found);
            thread::spawn(move || loop {
                if tvbox_remaining_timeout(deadline, probe_cap).is_none() {
                    break;
                }
                let address = queue.lock().ok().and_then(|mut items| items.pop_front());
                let Some(address) = address else { break };
                if let Some(device) = probe_tvbox_until(address, deadline, probe_cap) {
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
    let deadline = Instant::now().checked_add(timeout)?;
    probe_tvbox_until(address, deadline, timeout)
}

fn probe_tvbox_until(
    address: SocketAddr,
    deadline: Instant,
    probe_cap: Duration,
) -> Option<CastDeviceInfo> {
    let connect_timeout = tvbox_remaining_timeout(deadline, probe_cap)?;
    let mut stream = TcpStream::connect_timeout(&address, connect_timeout).ok()?;

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
'''

text = text[:start] + replacement + text[end:]

anchor = '''    #[test]\n    fn browser_push_ids_are_unique_and_cache_is_bounded() {\n'''
test = r'''    #[test]
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

'''
if anchor not in text:
    raise SystemExit("TVBox deadline test anchor missing")
text = text.replace(anchor, test + anchor, 1)

path.write_text(text, encoding="utf-8")
