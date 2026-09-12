from pathlib import Path

path = Path('native_shell/src/cast.rs')
text = path.read_text(encoding='utf-8')

old = '''        match stream.read(&mut chunk[..read_len]) {
            Ok(0) => break,
            Ok(read) => {
                response_bytes.extend_from_slice(&chunk[..read]);
                if response_bytes.len() > MAX_TVBOX_HTTP_RESPONSE_BYTES {
                    return Err("TVBox 响应过大".into());
                }
            }
'''
new = '''        match stream.read(&mut chunk[..read_len]) {
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
'''
if old not in text:
    raise SystemExit('read loop anchor not found')
text = text.replace(old, new, 1)

anchor = '''#[derive(Debug)]
struct TvboxHttpResponse {
    status: u16,
    body: String,
}

fn tvbox_http_request(
'''
helper = '''#[derive(Debug)]
struct TvboxHttpResponse {
    status: u16,
    body: String,
}

fn tvbox_http_response_expected_len(response: &[u8]) -> Result<Option<usize>, String> {
    let Some(header_end) = response.windows(4).position(|window| window == b"\\r\\n\\r\\n") else {
        return Ok(None);
    };
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|_| "TVBox 返回了非 UTF-8 响应头".to_string())?;
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
    header_end
        .checked_add(4)
        .and_then(|value| value.checked_add(content_length))
        .map(Some)
        .ok_or_else(|| "TVBox 响应过大".to_string())
}

fn tvbox_http_request(
'''
if anchor not in text:
    raise SystemExit('helper anchor not found')
text = text.replace(anchor, helper, 1)

test_anchor = '''    #[test]
    fn tvbox_http_response_has_total_read_deadline() {
'''
test = '''    #[test]
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
                "HTTP/1.1 200 OK\\r\\nContent-Length: {}\\r\\nConnection: keep-alive\\r\\n\\r\\n{body}",
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
    fn tvbox_http_response_has_total_read_deadline() {
'''
if test_anchor not in text:
    raise SystemExit('test anchor not found')
text = text.replace(test_anchor, test, 1)

path.write_text(text, encoding='utf-8')
