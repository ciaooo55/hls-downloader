from pathlib import Path

path = Path("native_shell/src/cast.rs")
text = path.read_text(encoding="utf-8")

old = '''    let mut response_bytes = Vec::with_capacity(4096);
    let mut limited = stream.take((MAX_TVBOX_HTTP_RESPONSE_BYTES + 1) as u64);
    limited
        .read_to_end(&mut response_bytes)
        .map_err(|error| format!("读取 TVBox 响应: {error}"))?;
    if response_bytes.len() > MAX_TVBOX_HTTP_RESPONSE_BYTES {
        return Err("TVBox 响应过大".into());
    }
    let response =
        String::from_utf8(response_bytes).map_err(|_| "TVBox 返回了非 UTF-8 响应".to_string())?;
'''
new = '''    let deadline = Instant::now() + timeout;
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
    let response =
        String::from_utf8(response_bytes).map_err(|_| "TVBox 返回了非 UTF-8 响应".to_string())?;
'''
if old not in text:
    raise SystemExit("TVBox bounded response read anchor missing")
text = text.replace(old, new, 1)

anchor = '''    #[test]\n    fn tvbox_http_response_is_bounded() {\n'''
test = r'''    #[test]
    fn tvbox_http_response_has_total_read_deadline() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/action", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\na",
            );
            for byte in [b'b', b'c', b'd'] {
                thread::sleep(Duration::from_millis(90));
                let _ = stream.write_all(&[byte]);
            }
        });

        let started = Instant::now();
        let error = tvbox_http_request("GET", &endpoint, "", Duration::from_millis(150)).unwrap_err();
        let elapsed = started.elapsed();
        assert_eq!(error, "读取 TVBox 响应超时");
        assert!(
            elapsed < Duration::from_millis(400),
            "TVBox response read exceeded total deadline: {elapsed:?}"
        );
        server.join().unwrap();
    }

'''
if anchor not in text:
    raise SystemExit("TVBox response bound test anchor missing")
text = text.replace(anchor, test + anchor, 1)

path.write_text(text, encoding="utf-8")
