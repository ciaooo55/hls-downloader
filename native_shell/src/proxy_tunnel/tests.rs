use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream as StdStream};
use std::sync::{Arc, Mutex};
use std::thread;

fn exchange(
    proxy_url: &str,
    host: &str,
    serve: impl FnOnce(StdStream) + Send + 'static,
) -> Result<StdStream, String> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("{proxy_url}{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let s = listener.accept().unwrap().0;
        s.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        serve(s);
    });
    let result = Proxy::parse(&url)
        .unwrap()
        .connect_blocking(host, 22, || Ok(()));
    server.join().unwrap();
    result
}

fn header(stream: &mut StdStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut b = [0];
        stream.read_exact(&mut b).unwrap();
        bytes.push(b[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes).unwrap();
        }
        assert!(bytes.len() < 2048);
    }
}

#[test]
fn connect_headers_auth_and_coalesced_banner() {
    for (url, auth) in [
        ("http://", ""),
        (
            "http://u:p%40ss@",
            "Proxy-Authorization: Basic dTpwQHNz\r\n",
        ),
    ] {
        let mut stream = exchange(url, "::1", move |mut s| {
            assert_eq!(
                header(&mut s),
                format!("CONNECT [::1]:22 HTTP/1.1\r\nHost: [::1]:22\r\n{auth}\r\n")
            );
            s.write_all(b"HTTP/1.1 200 OK\r\n\r\nSSH").unwrap();
        })
        .unwrap();
        let mut banner = [0; 3];
        stream.read_exact(&mut banner).unwrap();
        assert_eq!(&banner, b"SSH");
    }
}

#[test]
fn connect_rejects_407_and_oversized_headers() {
    for (response, reason) in [
        (b"HTTP/1.1 407 secret\r\n\r\n".to_vec(), "407"),
        (vec![b'x'; 17000], "过大"),
    ] {
        let err = exchange("http://u:secret@", "localhost", move |mut s| {
            header(&mut s);
            s.write_all(&response).unwrap();
        })
        .unwrap_err();
        assert!(err.contains(reason) && !err.contains("secret"), "{err}");
    }
}

#[test]
fn rejects_injection_invalid_endpoints_and_unsupported_auth_before_connecting() {
    for url in [
        "http://u%0d:p@host",
        "http://u:p%0a@host",
        "http://host\r\n",
        "http://user%3aname:pw@host",
        "http://host:99999",
        "http://host:bad",
        "http://host:0",
        "http://host/path",
        "socks4a://user:pass@host",
        "https://host",
        "other://host",
    ] {
        assert!(Proxy::parse(url).is_err(), "{url:?}");
    }
    let proxy = Proxy::parse("http://127.0.0.1:1").unwrap();
    assert!(proxy
        .connect_blocking("a\r\nHost: b", 22, || Ok(()))
        .unwrap_err()
        .contains("目标地址"));
}

#[test]
fn socks5_addresses_authentication_and_reply_lengths() {
    for (host, addr, atyp) in [
        ("127.0.0.1", vec![127, 0, 0, 1], 1),
        ("::1", std::net::Ipv6Addr::LOCALHOST.octets().to_vec(), 4),
        (
            "ftp.invalid",
            [b"\x0b".as_slice(), b"ftp.invalid"].concat(),
            3,
        ),
    ] {
        let auth = atyp != 1;
        let url = if auth { "socks5h://u:p@" } else { "socks5://" };
        let mut stream = exchange(url, host, move |mut s| {
            let method = if auth { 2 } else { 0 };
            let mut greeting = [0; 3];
            s.read_exact(&mut greeting).unwrap();
            assert_eq!(greeting, [5, 1, method]);
            s.write_all(&[5, method]).unwrap();
            if auth {
                let mut auth = [0; 5];
                s.read_exact(&mut auth).unwrap();
                assert_eq!(&auth, b"\x01\x01u\x01p");
                s.write_all(&[1, 0]).unwrap();
            }
            let mut req = vec![0; 6 + addr.len()];
            s.read_exact(&mut req).unwrap();
            assert_eq!(
                req,
                [vec![5, 1, 0, atyp], addr.clone(), vec![0, 22]].concat()
            );
            s.write_all(&[vec![5, 0, 0, atyp], addr, vec![0, 22, 42]].concat())
                .unwrap();
        })
        .unwrap();
        let mut banner = [0];
        stream.read_exact(&mut banner).unwrap();
        assert_eq!(banner, [42]);
    }
}

#[test]
fn socks5_auth_rejection_or_downgrade_aborts() {
    for method in [2, 0] {
        let err = exchange("socks5://u:p@", "127.0.0.1", move |mut s| {
            let mut greeting = [0; 3];
            s.read_exact(&mut greeting).unwrap();
            assert_eq!(greeting, [5, 1, 2]);
            s.write_all(&[5, method]).unwrap();
            if method == 2 {
                s.read_exact(&mut [0; 5]).unwrap();
                s.write_all(&[1, 1]).unwrap();
            }
        })
        .unwrap_err();
        assert!(err.contains("认证"));
    }
}

#[test]
fn socks4_local_resolution_and_socks4a_remote_name_with_userid() {
    for (url, host, address, name) in [
        ("socks4://id@", "localhost", [127, 0, 0, 1], b"".as_slice()),
        (
            "socks4a://id@",
            "ftp.invalid",
            [0, 0, 0, 1],
            b"ftp.invalid\0".as_slice(),
        ),
    ] {
        exchange(url, host, move |mut s| {
            let expected = [b"\x04\x01\x00\x16".as_slice(), &address, b"id\0", name].concat();
            let mut req = vec![0; expected.len()];
            s.read_exact(&mut req).unwrap();
            assert_eq!(req, expected);
            s.write_all(&[0, 90, 0, 0, 0, 0, 0, 0]).unwrap();
        })
        .unwrap();
    }
}

#[test]
fn stalled_handshake_is_cancellable() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = Proxy::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let mut s = listener.accept().unwrap().0;
        header(&mut s);
        tx.send(()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        assert_eq!(s.read(&mut [0]).unwrap(), 0);
    });
    let err = proxy
        .connect_blocking("host", 22, || {
            if rx.try_recv().is_ok() {
                Err("paused".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert_eq!(err, "paused");
    server.join().unwrap();
}

/// Relays only origin.invalid to a loopback server. A direct connection cannot
/// reach the fixture. Dropping the runtime also stops all accepted relays.
pub(crate) struct TestProxy {
    pub url: String,
    pub ports: Arc<Mutex<Vec<u16>>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestProxy {
    pub fn start(scheme: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("{scheme}://user:pass@{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let ports = Arc::new(Mutex::new(Vec::new()));
        let log = ports.clone();
        let thread = thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                loop {
                    tokio::select! {
                        _ = &mut stopped => break,
                        result = listener.accept() => {
                            let (stream, _) = result.unwrap();
                            let log = log.clone();
                            tokio::spawn(async move { relay(stream,scheme, log).await.unwrap(); });
                        }
                    }
                }
            });
        });
        Self {
            url,
            ports,
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for TestProxy {
    fn drop(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        self.thread.take().unwrap().join().unwrap();
    }
}

async fn relay(mut stream: TcpStream, scheme: &str, log: Arc<Mutex<Vec<u16>>>) -> io::Result<()> {
    let port = if scheme == "http" {
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") && header.len() < 2048 {
            header.push(stream.read_u8().await?);
        }
        let header = String::from_utf8(header).unwrap();
        if !header
            .lines()
            .any(|line| line == "Proxy-Authorization: Basic dXNlcjpwYXNz")
        {
            stream
                .write_all(b"HTTP/1.1 407 Auth required\r\n\r\n")
                .await?;
            return Ok(());
        }
        header
            .strip_prefix("CONNECT origin.invalid:")
            .unwrap()
            .split_once(' ')
            .unwrap()
            .0
            .parse()
            .unwrap()
    } else {
        let mut greeting = [0; 3];
        stream.read_exact(&mut greeting).await?;
        assert_eq!(greeting, [5, 1, 2]);
        stream.write_all(&[5, 2]).await?;
        assert_eq!(stream.read_u8().await?, 1);
        let mut user = vec![0; stream.read_u8().await?.into()];
        stream.read_exact(&mut user).await?;
        let mut pass = vec![0; stream.read_u8().await?.into()];
        stream.read_exact(&mut pass).await?;
        if user != b"user" || pass != b"pass" {
            stream.write_all(&[1, 1]).await?;
            return Ok(());
        }
        stream.write_all(&[1, 0]).await?;
        let mut prefix = [0; 5];
        stream.read_exact(&mut prefix).await?;
        assert_eq!(&prefix, b"\x05\x01\x00\x03\x0e");
        let mut name = [0; 14];
        stream.read_exact(&mut name).await?;
        assert_eq!(&name, b"origin.invalid");
        stream.read_u16().await?
    };
    let mut upstream = TcpStream::connect(("127.0.0.1", port)).await?;
    log.lock().unwrap().push(port);
    if scheme == "http" {
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await?;
    } else {
        stream.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await?;
    }
    let _ = tokio::io::copy_bidirectional(&mut upstream, &mut stream).await;
    Ok(())
}

#[cfg(windows)]
mod system_tests {
    use super::*;
    use crate::proxy_tunnel::system::{lookup_candidates, select_route, ProxyInfo};

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn proxy_entry(scheme: i32, host: &[u16], port: u16) -> ProxyInfo {
        ProxyInfo {
            proxy: 1,
            bypass: 0,
            scheme,
            host: host.as_ptr(),
            port,
        }
    }

    fn direct_entry() -> ProxyInfo {
        ProxyInfo {
            proxy: 0,
            bypass: 1,
            scheme: 0,
            host: std::ptr::null(),
            port: 0,
        }
    }

    #[test]
    fn select_route_falls_back_across_invalid_and_unsupported_entries() {
        let invalid = wide("bad host\r\n");
        let http_host = wide("127.0.0.1");
        let socks_host = wide("::1");
        let https_host = wide("proxy.example");

        // [unknown, invalid_host, http] -> http
        let entries = [
            proxy_entry(99, &http_host, 8080),
            proxy_entry(3, &invalid, 8080),
            proxy_entry(3, &http_host, 8080),
        ];
        assert_eq!(
            select_route(&entries, false).unwrap(),
            Some("http://127.0.0.1:8080".into())
        );

        // [port_zero, socks] -> socks4
        let entries = [
            proxy_entry(3, &http_host, 0),
            proxy_entry(8, &socks_host, 1080),
        ];
        assert_eq!(
            select_route(&entries, false).unwrap(),
            Some("socks4://[::1]:1080".into())
        );

        // [https, direct] -> DIRECT when !allow_https, https when allow_https
        let entries = [proxy_entry(4, &https_host, 443), direct_entry()];
        assert_eq!(select_route(&entries, false).unwrap(), None);
        assert_eq!(
            select_route(&entries, true).unwrap(),
            Some("https://proxy.example:443".into())
        );

        // [https, http] -> http when !allow_https, https when allow_https
        let entries = [
            proxy_entry(4, &https_host, 443),
            proxy_entry(3, &http_host, 7890),
        ];
        assert_eq!(
            select_route(&entries, false).unwrap(),
            Some("http://127.0.0.1:7890".into())
        );
        assert_eq!(
            select_route(&entries, true).unwrap(),
            Some("https://proxy.example:443".into())
        );
    }

    #[test]
    fn select_route_fails_closed_when_no_usable_entry_exists() {
        let invalid = wide("bad host");
        let https_host = wide("proxy.example");

        assert!(select_route(&[], false).is_err());

        // Remembers first_error across multiple failures.
        let entries = [
            proxy_entry(99, &https_host, 8080),
            proxy_entry(3, &https_host, 0),
        ];
        let err = select_route(&entries, false).unwrap_err();
        assert!(err.contains("协议"), "{err}");

        // HTTPS-only list fails closed when allow_https is false.
        let entries = [proxy_entry(4, &https_host, 443)];
        assert!(select_route(&entries, false).is_err());

        // Null host + invalid host fails closed.
        let entries = [
            ProxyInfo {
                proxy: 1,
                bypass: 0,
                scheme: 3,
                host: std::ptr::null(),
                port: 8080,
            },
            proxy_entry(3, &invalid, 8080),
        ];
        let err = select_route(&entries, false).unwrap_err();
        assert!(err.contains("地址无效"), "{err}");
    }

    #[test]
    fn lookup_candidates_strips_credentials_and_fragments_and_normalizes_schemes() {
        assert_eq!(
            lookup_candidates("sftp://user:p@ss@[::1]:2222/dir/file.bin?x=1#frag").unwrap(),
            vec!["https://[::1]:2222/dir/file.bin?x=1".to_string()]
        );
        assert_eq!(
            lookup_candidates("ftps://u:p@example.com:990/a?b=2#top").unwrap(),
            vec![
                "ftp://example.com:990/a?b=2".to_string(),
                "https://example.com:990/a?b=2".to_string(),
            ]
        );
        assert_eq!(
            lookup_candidates("sftp://user@example.com?q=1#frag").unwrap(),
            vec!["https://example.com/?q=1".to_string()]
        );
        assert_eq!(
            lookup_candidates("ftps://example.com").unwrap(),
            vec![
                "ftp://example.com/".to_string(),
                "https://example.com/".to_string(),
            ]
        );
        assert!(lookup_candidates("not-a-url").is_err());
        assert!(lookup_candidates("sftp://user@/path").is_err());
        assert!(lookup_candidates("https://evil.example\0.good.example/a").is_err());

        // Host buffer lacking a NUL terminator within MAX_PROXY_HOST_UTF16_UNITS fails closed
        // without over-reading past the bounded scan window.
        let unterminated = vec![u16::from(b'a'); 1030];
        let entries = [ProxyInfo {
            proxy: 1,
            bypass: 0,
            scheme: 3,
            host: unterminated.as_ptr(),
            port: 8080,
        }];
        assert!(select_route(&entries, false).is_err());
    }

    #[test]
    fn system_proxy_resolve_normalizes_sftp_and_ftps_without_scheme_error() {
        let sentinel = crate::net_policy::SYSTEM_PROXY_SENTINEL;
        for url in ["sftp://example.com/a", "ftps://example.com/a"] {
            if let Err(err) = Proxy::resolve(sentinel, url, || Ok(())) {
                assert!(
                    !err.contains("12006"),
                    "WinINet rejected normalized scheme for {url}: {err}"
                );
            }
        }
        // Calling resolve_system_proxy from inside an active Tokio runtime must not panic.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            if let Err(err) = resolve_system_proxy("https://example.com/a", || Ok(())) {
                assert!(!err.contains("12006"), "{err}");
            }
            assert_eq!(
                resolve_system_proxy("https://example.com/a", || Err("paused".into())).unwrap_err(),
                "paused"
            );
        });
    }
}
