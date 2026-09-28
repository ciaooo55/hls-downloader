//! 在**没有真实电视/机顶盒**的机器上，用一台"假局域网设备"把 `browser.media_push_device_selection`
//! 这条链路的协议层全部走通：
//!
//! 1. 起一个 SSDP 响应者：收到 M-SEARCH 就回 `LOCATION: http://127.0.0.1:<port>/desc.xml`；
//! 2. 起一个 HTTP 服务：给出 UPnP description（friendlyName + URLBase + AVTransport service）；
//! 3. `discover_devices_for_mode` 走真实的 M-SEARCH → LOCATION → 抓取 description → 解析；
//! 4. `play_on_device` 走真实的 SOAP `SetAVTransportURI` + `Play`，落到假设备的 control URL 上；
//! 5. 断言会话状态被记住（`last_device_label` / `last_session_status`）。
//!
//! 这样"设备选择 + 推送"这条链上，除**真实无线电与厂商私有实现**以外的部分都在本机可复现地验证了。
//! 仍需实机门禁的只剩下：真实 SSDP/mDNS 设备响应、真实 DIAL/Chromecast 握手、
//! 以及局域网媒体 URL 在电视侧能否真正播放。这条边界在
//! `docs/architecture/browser-media-push-release-gate.md` 里单独记账。
//!
//! 运行前提：假设备必须监听 UDP 1900（SSDP 多播组端口）。Windows 的 SSDPSRV
//! （SSDP Discovery 服务）常驻时会独占 1900，那种机器上链路无从跑起——
//! 设备用例**跳过并打印原因**，而不是带着 panic 把整个测试二进制染红；
//! 纯解析用例（无 socket）不受影响，始终执行。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hls_native_shell::cast::{
    discover_devices_for_mode, last_device_label, last_session_status, play_on_device,
};

const LABEL: &str = "模拟客厅电视";
const MEDIA_URL: &str = "http://127.0.0.1:18765/simulated/movie.mp4";
const TITLE: &str = "模拟影片";

/// 假设备收到的 SOAP 动作（按到达顺序），测试结束后断言。
#[derive(Default)]
struct DeviceLog {
    actions: Vec<String>,
    set_uri_bodies: Vec<String>,
}

/// 一台可编程的假 DLNA/UPnP 设备：SSDP 应答 + UPnP description + AVTransport 控制端点。
struct SimulatedCastDevice {
    port: u16,
    log: Arc<Mutex<DeviceLog>>,
    shutdown: Arc<AtomicBool>,
    m_searches: Arc<AtomicUsize>,
}

impl SimulatedCastDevice {
    /// 起假设备。`None` = UDP 1900 被占用（典型：Windows SSDPSRV），调用方跳过用例。
    fn start() -> Option<SimulatedCastDevice> {
        // 1) description + control 端点
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind device http");
        listener
            .set_nonblocking(true)
            .expect("set_nonblocking device http");
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(DeviceLog::default()));
        let m_searches = Arc::new(AtomicUsize::new(0));
        // 2) SSDP 响应者：加入 SSDP 多播组，收到 M-SEARCH 就单播回 LOCATION。
        //    先绑 UDP 再起任何线程——失败时不留孤儿线程。
        let ssdp = match UdpSocket::bind(("0.0.0.0", 1900)) {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!(
                    "skip: UDP 1900 unavailable ({error}); the simulated SSDP device cannot listen"
                );
                return None;
            }
        };
        ssdp.set_nonblocking(true).expect("set_nonblocking ssdp");
        let group: std::net::Ipv4Addr = "239.255.255.250".parse().expect("ssdp group");
        // 关键：多播组必须按"发现流程真正会用的那个物理局域网口"加入。
        // 以前用 0.0.0.0（任意口），Windows 依据路由度量把组固定到默认路由；
        // 机器上同时有 TUN 且 TUN 是默认路由时（本仓库的开发机就是这样），
        // M-SEARCH 从 WLAN 发出去、多播组却挂在 TUN 上，假设备时收时不收，
        // 用例就变成了随机红/绿。固定到同一口之后收发两侧始终一致。
        let announced =
            hls_native_shell::cast::preferred_lan_ipv4().unwrap_or(std::net::Ipv4Addr::UNSPECIFIED);
        ssdp.join_multicast_v4(&group, &announced)
            .expect("join ssdp group");
        let http_log = log.clone();
        let shutdown_tx = Arc::new(AtomicBool::new(false));
        let http_shutdown = Arc::clone(&shutdown_tx);
        let ssdp_searches = Arc::clone(&m_searches);

        thread::spawn(move || loop {
            if http_shutdown.load(Ordering::SeqCst) {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let mut stream = stream;
                    stream.set_read_timeout(Some(Duration::from_secs(4))).ok();
                    handle_http(&mut stream, port, &http_log);
                }
                Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        });

        let ssdp_shutdown = Arc::clone(&shutdown_tx);
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                if ssdp_shutdown.load(Ordering::SeqCst) {
                    break;
                }
                match ssdp.recv_from(&mut buf) {
                    Ok((count, peer)) => {
                        let request = String::from_utf8_lossy(&buf[..count]);
                        if !request.contains("M-SEARCH") {
                            continue;
                        }
                        ssdp_searches.fetch_add(1, Ordering::SeqCst);
                        let reply = format!(
                            "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=30\r\nEXT:\r\nLOCATION: http://127.0.0.1:{port}/desc.xml\r\nSERVER: simulated/7 UPnP/1.1\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\nUSN: uuid:simulated-device::urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n"
                        );
                        let _ = ssdp.send_to(reply.as_bytes(), peer);
                    }
                    Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Some(SimulatedCastDevice {
            port,
            log,
            shutdown: shutdown_tx,
            m_searches,
        })
    }

    fn location(&self) -> String {
        format!("http://127.0.0.1:{}/desc.xml", self.port)
    }

    /// 扫描窗口内假设备实际收到的 M-SEARCH 次数。0 次 = 环境没把多播送到本机
    /// （典型：Windows SSDPSRV 抢占了 239.255.255.250:1900），本轮扫描证明不了任何事。
    fn m_searches(&self) -> usize {
        self.m_searches.load(Ordering::SeqCst)
    }
}

impl Drop for SimulatedCastDevice {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(80));
    }
}

fn handle_http(stream: &mut std::net::TcpStream, port: u16, log: &Arc<Mutex<DeviceLog>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone device stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut content_length = 0usize;
    let mut action = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        } else if lower.starts_with("soapaction:") {
            // "urn:upnp-org:serviceId:AVTransport#SetAVTransportURI" -> SetAVTransportURI。
            // 注意动作名大小写敏感，只能 trim 大小写/引号，不能把小写后的整行拿来切。
            action = trimmed
                .trim_matches('"')
                .split_once(':')
                .map(|(_, rest)| rest.trim().to_string())
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .to_string();
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).to_string();

    let (status, payload) = if request_line.starts_with("GET /desc.xml") {
        (
            "200 OK",
            format!(
                "<?xml version=\"1.0\"?><root xmlns=\"urn:schemas-upnp-org:device-1-0\"><specVersion><major>1</major><minor>0</minor></specVersion><URLBase>http://127.0.0.1:{port}</URLBase><device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType><friendlyName>{LABEL}</friendlyName><manufacturer>Simulated</manufacturer><modelName>SimDLNA</modelName><serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><serviceId>urn:upnp-org:serviceId:AVTransport</serviceId><controlURL>/ctrl</controlURL><eventSubURL>/evt</eventSubURL><SCPDURL>/avt.xml</SCPDURL></service></serviceList></device></root>"
            ),
        )
    } else if request_line.starts_with("POST /ctrl") {
        let recorded = action.clone();
        let mut guard = log.lock().expect("lock device log");
        guard.actions.push(recorded);
        if action == "SetAVTransportURI" {
            guard.set_uri_bodies.push(body);
        }
        if action.is_empty() {
            (
                "400 Bad Request",
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>missing SOAPACTION</faultstring></s:Fault></s:Body></s:Envelope>".to_string(),
            )
        } else {
            // 一个规规矩矩的设备应答：可选的 OUT 参数 + 一个 SOAP 信封，**不含 s:Fault / UPnPError**。
            (
                "200 OK",
                format!(
                    "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action}Response xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\"><InstanceID>0</InstanceID><CurrentTransportActions>Play,Stop</CurrentTransportActions></u:{action}Response></s:Body></s:Envelope>"
                ),
            )
        }
    } else {
        ("404 Not Found", String::new())
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// 1900/多播组是进程级唯一资源，同一测试二进制里的并行用例会互相抢。
/// 需要假设备的用例全程持有这把锁，其它用例（纯解析）不持有，因此仍能并行跑。
static SSDP_GUARD: Mutex<()> = Mutex::new(());

fn soap_actions(log: &Arc<Mutex<DeviceLog>>) -> Vec<String> {
    log.lock().expect("lock device log").actions.clone()
}

fn soap_bodies(log: &Arc<Mutex<DeviceLog>>) -> Vec<String> {
    log.lock().expect("lock device log").set_uri_bodies.clone()
}

#[test]
fn a_simulated_lan_device_can_be_discovered_from_ssdp_and_description() {
    let _ssdp = SSDP_GUARD.lock().unwrap_or_else(|error| error.into_inner());
    let Some(device) = SimulatedCastDevice::start() else {
        return;
    };
    // 两次 M-SEARCH 轮次 + description 抓取，给足 4 秒。同一进程里 mDNS/TVBox 扫描并行跑，
    // 不设 HLS_V7_CAST_NULL（那会把整个发现流程短路掉，本测试要的就是真实流程）。
    let started = Instant::now();
    let found = discover_devices_for_mode(Duration::from_secs(4), "cast");
    let elapsed = started.elapsed();

    let found = found.expect("discover_cast_devices should succeed");

    if device.m_searches() == 0 {
        eprintln!(
            "skip: the simulated device received no M-SEARCH in the scan window (the OS SSDP stack owns 239.255.255.250:1900 here); this run proves nothing"
        );
        return;
    }
    println!("discovery took {elapsed:?}");
    let mine: Vec<_> = found
        .iter()
        .filter(|item| item.location == device.location())
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "假设备应被 SSDP 发现且解析出 description；实际发现 {:?}",
        found
            .iter()
            .map(|i| (i.id.clone(), i.label.clone(), i.location.clone()))
            .collect::<Vec<_>>()
    );
    let mine = mine[0];
    assert_eq!(mine.label, LABEL, "friendlyName 应成为用户看到的设备名");
    assert!(
        mine.control_url.starts_with("http://127.0.0.1:"),
        "control_url 应由 description 的 URLBase + controlURL 解析而来，实际 {}",
        mine.control_url
    );
    assert!(
        mine.service_type.contains("AVTransport"),
        "DLNA 设备应标记为 AVTransport，实际 {}",
        mine.service_type
    );
    assert!(
        mine.id.starts_with("dlna:"),
        "DLNA 设备 id 应以 dlna: 开头，实际 {}",
        mine.id
    );
}

#[test]
fn play_on_device_sends_real_soap_actions_and_remembers_the_session() {
    let _ssdp = SSDP_GUARD.lock().unwrap_or_else(|error| error.into_inner());
    let Some(device) = SimulatedCastDevice::start() else {
        return;
    };
    let found = discover_devices_for_mode(Duration::from_secs(4), "cast").expect("discovery");

    if device.m_searches() == 0 {
        eprintln!(
            "skip: the simulated device received no M-SEARCH in the scan window (the OS SSDP stack owns 239.255.255.250:1900 here); this run proves nothing"
        );
        return;
    }
    let mine = found
        .iter()
        .find(|item| item.location == device.location())
        .cloned()
        .unwrap_or_else(|| panic!("假设备应被发现；实际 {:?}", found.len()));
    assert!(soap_actions(&device.log).is_empty(), "发现阶段不应推流");

    let pushed = play_on_device(&mine.id, MEDIA_URL, TITLE).expect("play_on_device");
    assert_eq!(
        pushed, LABEL,
        "推送成功应返回设备名（上层弹 Toast 用的就是它）"
    );

    let actions = soap_actions(&device.log);
    assert_eq!(
        actions,
        vec!["SetAVTransportURI".to_string(), "Play".to_string()],
        "DLNA 推送必须先 SetAVTransportURI 再 Play，顺序也应在这一步定死"
    );
    let bodies = soap_bodies(&device.log);
    assert_eq!(bodies.len(), 1, "只有 SetAVTransportURI 带媒体地址");
    let body = &bodies[0];
    assert!(
        body.contains(MEDIA_URL),
        "SetAVTransportURI 的 CurrentURI 应是被推送的媒体 URL：{body}"
    );
    assert!(
        body.contains(TITLE),
        "CurrentURIMetaData 应带 DIDL-Lite 元数据（标题至少在 XML 里出现）：{body}"
    );

    assert_eq!(last_device_label(), LABEL, "会话应记住这台设备");
    let status = last_session_status();
    assert_eq!(status.label, LABEL);
    assert_eq!(
        status.state, "PLAYING",
        "DLNA 会话应是 PLAYING；{:?}",
        status
    );
    assert!(status.playing, "播放中标志应为真；{:?}", status);
}

/// 会向上层 UI 假设备是否存在：扫描时另一台设备（例如 Chromecast/TVBox）也在响应，
/// 不该影响我这一台被发现、被选中、被推送。
#[test]
fn simulated_device_survives_a_second_scan_round() {
    let _ssdp = SSDP_GUARD.lock().unwrap_or_else(|error| error.into_inner());
    let Some(device) = SimulatedCastDevice::start() else {
        return;
    };
    let first = discover_devices_for_mode(Duration::from_secs(3), "cast").expect("first scan");
    if device.m_searches() == 0 {
        eprintln!(
            "skip: the simulated device received no M-SEARCH in the scan window (the OS SSDP stack owns 239.255.255.250:1900 here); this run proves nothing"
        );
        return;
    }
    let second = discover_devices_for_mode(Duration::from_secs(3), "cast").expect("second scan");
    let in_first = first
        .iter()
        .filter(|i| i.location == device.location())
        .count();
    let in_second = second
        .iter()
        .filter(|i| i.location == device.location())
        .count();
    assert_eq!(in_first, 1, "第一轮扫描应发现 1 次，实际 {in_first}");
    assert_eq!(
        in_second, 1,
        "第二轮扫描应仍只发现 1 次（去重/缓存不应重复登记），实际 {in_second}"
    );
    let mine = second
        .iter()
        .find(|i| i.location == device.location())
        .cloned()
        .expect("still discoverable");
    play_on_device(&mine.id, MEDIA_URL, TITLE).expect("push after rescan");
    let actions = soap_actions(&device.log);
    assert_eq!(
        actions,
        vec!["SetAVTransportURI".to_string(), "Play".to_string()]
    );
}

/// 单测里把"设备不存在"这条错误路径也钉住：选择前不让推，错误信息要可读。
#[test]
fn pushing_to_an_unknown_device_asks_the_user_to_scan_first() {
    let error = play_on_device("dlna:http://127.0.0.1:1/ctrl", MEDIA_URL, TITLE)
        .expect_err("未扫描过的设备不应推送成功");
    let message = error.to_string();
    assert!(
        message.contains("扫描") || message.contains("选择"),
        "应提示先扫描并选择设备，实际 “{message}”"
    );
}

/// 给上面的错误路径一个反证：随手编的 control_url 也不能糊过去（必须是局域网地址）。
#[test]
fn a_non_lan_control_url_is_rejected_before_any_socket_work() {
    // 这条不依赖假设备，验证 is_lan_host 的边界：公网地址不被当成 LAN。
    assert!(hls_native_shell::cast::is_lan_host("127.0.0.1"));
    assert!(hls_native_shell::cast::is_lan_host("192.168.1.50"));
    assert!(hls_native_shell::cast::is_lan_host("172.16.9.9"));
    assert!(!hls_native_shell::cast::is_lan_host("8.8.8.8"));
    assert!(!hls_native_shell::cast::is_lan_host("example.com"));
}

/// SSDP 响应的解析：LOCATION 大小写、http/https 都收，别的协议不收。
#[test]
fn ssdp_location_parsing_accepts_only_http_urls() {
    use hls_native_shell::cast::{parse_device_description, parse_ssdp_location};
    assert_eq!(
        parse_ssdp_location("HTTP/1.1 200 OK\r\nlocation: http://127.0.0.1:8080/desc.xml\r\n"),
        Some("http://127.0.0.1:8080/desc.xml".to_string())
    );
    assert_eq!(
        parse_ssdp_location("LOCATIon:https://10.0.0.2/d.xml\r\n"),
        Some("https://10.0.0.2/d.xml".to_string())
    );
    assert_eq!(
        parse_ssdp_location("LOCATION: ftp://10.0.0.2/d.xml\r\n"),
        None
    );
    assert_eq!(
        parse_ssdp_location("HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\n"),
        None
    );

    let xml = "<root><device><friendlyName>办公电视</friendlyName><URLBase>http://10.0.0.5</URLBase><serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/control</controlURL></service></serviceList></device></root>";
    let device = parse_device_description(xml, "http://10.0.0.5/desc.xml").expect("parsed");
    assert_eq!(device.label, "办公电视");
    assert_eq!(device.control_url, "http://10.0.0.5/control");
    assert_eq!(
        device.service_type,
        "urn:schemas-upnp-org:service:AVTransport:1"
    );
}

/// description 里没有 AVTransport 就不算一台可投设备（例如只支持 ContentDirectory 的 NAS）。
#[test]
fn a_device_without_av_transport_is_not_a_cast_target() {
    use hls_native_shell::cast::parse_device_description;
    let xml = "<root><device><friendlyName>NAS</friendlyName><serviceList><service><serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType><controlURL>/cd</controlURL></service></serviceList></device></root>";
    assert!(
        parse_device_description(xml, "http://10.0.0.9/desc.xml").is_none(),
        "不支持 AVTransport 的设备不应被当成投屏目标"
    );
}
