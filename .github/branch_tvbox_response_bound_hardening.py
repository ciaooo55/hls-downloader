from pathlib import Path

path = Path("native_shell/src/cast.rs")
text = path.read_text(encoding="utf-8")

old = '''#[derive(Debug)]
struct TvboxHttpResponse {
    status: u16,
    body: String,
}
'''
new = '''const MAX_TVBOX_HTTP_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct TvboxHttpResponse {
    status: u16,
    body: String,
}
'''
if old not in text:
    raise SystemExit("TVBox response struct anchor missing")
text = text.replace(old, new, 1)

old = '''    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("读取 TVBox 响应: {error}"))?;
    let (headers, body) = response.split_once("\\r\\n\\r\\n").unwrap_or((&response, ""));
'''
new = '''    let mut response_bytes = Vec::with_capacity(4096);
    let mut limited = stream.take((MAX_TVBOX_HTTP_RESPONSE_BYTES + 1) as u64);
    limited
        .read_to_end(&mut response_bytes)
        .map_err(|error| format!("读取 TVBox 响应: {error}"))?;
    if response_bytes.len() > MAX_TVBOX_HTTP_RESPONSE_BYTES {
        return Err("TVBox 响应过大".into());
    }
    let response = String::from_utf8(response_bytes)
        .map_err(|_| "TVBox 返回了非 UTF-8 响应".to_string())?;
    let (headers, body) = response.split_once("\\r\\n\\r\\n").unwrap_or((&response, ""));
'''
if old not in text:
    raise SystemExit("TVBox response read anchor missing")
text = text.replace(old, new, 1)

anchor = '''    #[test]\n    fn tvbox_push_falls_back_from_post_to_get() {\n'''
test = r'''    #[test]
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

'''
if anchor not in text:
    raise SystemExit("TVBox response bound test anchor missing")
text = text.replace(anchor, test + anchor, 1)

path.write_text(text, encoding="utf-8")
