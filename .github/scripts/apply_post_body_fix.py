from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PATH = ROOT / "native_shell/src/http_engine.rs"
text = PATH.read_text(encoding="utf-8")


def replace_once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    text = text.replace(old, new, 1)


replace_once(
    "use std::fs::{self, File, OpenOptions};\n",
    "use std::ffi::OsString;\nuse std::fs::{self, File, OpenOptions};\n",
    "OsString import",
)

post_body = '''fn post_body(job: &Job) -> Result<Vec<u8>, EngineError> {
    if !request_method(job).eq_ignore_ascii_case("POST") || job.body_path.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    fs::read(&job.body_path).map_err(|err| EngineError::Failed(err.to_string()))
}
'''
post_body_plus = post_body + '''
fn curl_request_args(job: &Job, headers: &HashMap<String, String>) -> Vec<OsString> {
    let mut args = Vec::new();
    match request_method(job) {
        "POST" => {
            args.push(OsString::from("--request"));
            args.push(OsString::from("POST"));
            args.push(OsString::from("--data-binary"));
            if job.body_path.as_os_str().is_empty() {
                args.push(OsString::new());
            } else {
                let mut body = OsString::from("@");
                body.push(&job.body_path);
                args.push(body);
            }
            // --data-binary otherwise invents application/x-www-form-urlencoded.
            // Preserve native/raw semantics when the caller did not set a type.
            if !headers.keys().any(|key| key.eq_ignore_ascii_case("content-type")) {
                args.push(OsString::from("-H"));
                args.push(OsString::from("Content-Type:"));
            }
        }
        "HEAD" => {
            // `-X HEAD` does not give curl HEAD response semantics; --head does.
            args.push(OsString::from("--head"));
        }
        _ => {}
    }
    args
}
'''
replace_once(post_body, post_body_plus, "curl request args helper")

replace_once(
    '''    if let Some(range) = range {
        command.arg("-H").arg(format!("Range: {range}"));
    }
    match proxy_route(&job.proxy) {
''',
    '''    if let Some(range) = range {
        command.arg("-H").arg(format!("Range: {range}"));
    }
    command.args(curl_request_args(job, &headers));
    match proxy_route(&job.proxy) {
''',
    "curl method/body forwarding",
)

replace_once(
    '''            if WinHttpSendRequest(request, null_mut(), 0, null_mut(), 0, 0, 0) == 0 {
                let err = GetLastError();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                return Err(EngineError::Failed(format!("WinHttpSendRequest {err}")));
            }
''',
    '''            let mut request_body = super::post_body(job)?;
            let request_body_len = u32::try_from(request_body.len())
                .map_err(|_| EngineError::Failed("HTTP request body exceeds WinHTTP limit".into()))?;
            let request_body_ptr = if request_body.is_empty() {
                null_mut()
            } else {
                request_body.as_mut_ptr() as *mut _
            };
            if WinHttpSendRequest(
                request,
                null_mut(),
                0,
                request_body_ptr,
                request_body_len,
                request_body_len,
                0,
            ) == 0
            {
                let err = GetLastError();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                return Err(EngineError::Failed(format!("WinHttpSendRequest {err}")));
            }
''',
    "WinHTTP body forwarding",
)

anchor = '''    #[test]
    fn origin_connect_pool_does_not_share_live_handles() {
'''
tests = '''    fn post_test_job(url: String, body_path: PathBuf) -> Job {
        Job {
            url,
            headers: HashMap::from([(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )]),
            output: PathBuf::new(),
            connections: 1,
            chunk_bytes: 1,
            total: 0,
            sequential: true,
            resume_from: 0,
            proxy: crate::net_policy::DIRECT_PROXY_SENTINEL.into(),
            resource_key: String::new(),
            etag: String::new(),
            last_modified: String::new(),
            control: PathBuf::new(),
            progress: PathBuf::new(),
            method: "POST".into(),
            body_path,
            mirrors: Vec::new(),
            replay_json: String::new(),
        }
    }

    #[test]
    fn curl_request_args_preserve_post_body_without_inventing_content_type() {
        let mut job = post_test_job("https://example.test/upload".into(), PathBuf::from("body.bin"));
        let args = curl_request_args(&job, &job.headers);
        assert_eq!(
            args,
            vec![
                OsString::from("--request"),
                OsString::from("POST"),
                OsString::from("--data-binary"),
                OsString::from("@body.bin"),
            ]
        );
        job.headers.clear();
        let args = curl_request_args(&job, &job.headers);
        assert_eq!(args[4], OsString::from("-H"));
        assert_eq!(args[5], OsString::from("Content-Type:"));
        job.body_path.clear();
        let args = curl_request_args(&job, &job.headers);
        assert_eq!(args[3], OsString::new());
    }

    #[cfg(windows)]
    #[test]
    fn winhttp_posts_the_job_body_bytes() {
        use std::net::TcpListener;
        use std::sync::mpsc;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, received) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.windows(4).any(|window| window == b"\\r\\n\\r\\n") {
                if stream.read(&mut byte).unwrap() == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            let split = request
                .windows(4)
                .position(|window| window == b"\\r\\n\\r\\n")
                .unwrap();
            let head = String::from_utf8_lossy(&request[..split]).to_string();
            let length = head
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            let mut body = request[split + 4..].to_vec();
            while body.len() < length {
                let mut buf = vec![0u8; length - body.len()];
                let got = stream.read(&mut buf).unwrap();
                if got == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..got]);
            }
            sent.send((head, body)).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\\r\\nContent-Length: 2\\r\\nConnection: close\\r\\n\\r\\nok")
                .unwrap();
        });

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hls-winhttp-post-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let body_path = dir.join("request.bin");
        fs::write(&body_path, b"alpha=1&beta=two").unwrap();
        let job = post_test_job(format!("http://{address}/upload"), body_path);
        let fetched = winhttp::get(&job, &job.url, None).unwrap();
        assert_eq!(fetched.status, 200);
        drop(fetched);
        let (head, body) = received.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(head.starts_with("POST /upload HTTP/1.1"));
        assert_eq!(body, b"alpha=1&beta=two");
        server.join().unwrap();
        let _ = fs::remove_dir_all(dir);
    }

'''
replace_once(anchor, tests + anchor, "POST regression tests")

PATH.write_text(text, encoding="utf-8")
print("POST body patch applied")
