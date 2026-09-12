from pathlib import Path

path = Path('native_shell/src/cast.rs')
text = path.read_text(encoding='utf-8')

old_helper = r'''fn tvbox_http_response_expected_len(response: &[u8]) -> Result<Option<usize>, String> {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
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
'''

new_helper = r'''fn tvbox_http_headers(response: &[u8]) -> Result<Option<(usize, &str)>, String> {
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
        for encoding in value.split(',').map(str::trim).filter(|item| !item.is_empty()) {
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
'''

if old_helper not in text:
    raise SystemExit('expected helper block not found')
text = text.replace(old_helper, new_helper, 1)

old_parse = r'''    let response =
        String::from_utf8(response_bytes).map_err(|_| "TVBox 返回了非 UTF-8 响应".to_string())?;
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
'''

new_parse = r'''    let Some((header_end, headers)) = tvbox_http_headers(&response_bytes)? else {
        return Err("TVBox 返回了无效响应".into());
    };
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
    let body = String::from_utf8(body_bytes)
        .map_err(|_| "TVBox 返回了非 UTF-8 响应".to_string())?;
    Ok(TvboxHttpResponse {
        status,
        body: body.trim().to_string(),
    })
'''

if old_parse not in text:
    raise SystemExit('expected response parse block not found')
text = text.replace(old_parse, new_parse, 1)

marker = r'''    #[test]
    fn tvbox_http_response_has_total_read_deadline() {
'''

test = r'''    #[test]
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

'''

if marker not in text:
    raise SystemExit('test insertion marker not found')
text = text.replace(marker, test + marker, 1)
path.write_text(text, encoding='utf-8')
