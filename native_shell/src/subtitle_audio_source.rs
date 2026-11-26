//! 有界音频窗口直接解码到内存，不生成下载任务或临时分片。
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn is_safe_browser_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("sec-fetch-")
        || lower.starts_with("sec-ch-ua")
        || matches!(
            lower.as_str(),
            "accept"
                | "accept-language"
                | "referer"
                | "origin"
                | "user-agent"
                | "range"
                | "priority"
                | "dnt"
                | "cache-control"
                | "pragma"
                | "upgrade-insecure-requests"
        )
}

fn validate_headers_field(field: Option<&Value>) -> Result<(), String> {
    let auth_err = || "此音轨需要专用鉴权请求头，请关闭提前翻译并使用播放器实时采集".to_string();
    match field {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(headers)) => {
            if headers.keys().any(|name| !is_safe_browser_header(name)) {
                return Err(auth_err());
            }
            Ok(())
        }
        Some(_) => Err(auth_err()),
    }
}

fn validate_cookie_fields(
    cookie_field: Option<&Value>,
    cookies_field: Option<&Value>,
) -> Result<(), String> {
    let cookie_err = || "此音轨使用浏览器 Cookie，请关闭提前翻译并使用播放器实时采集".to_string();
    match cookie_field {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) if text.is_empty() => {}
        Some(_) => return Err(cookie_err()),
    }
    match cookies_field {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) if items.is_empty() => {}
        Some(_) => return Err(cookie_err()),
    }
    Ok(())
}

pub(crate) fn validate(source: &Value) -> Result<(), String> {
    let url = source["url"].as_str().unwrap_or("");
    if !crate::http_engine::http_fetch_url_allowed(url) || url.contains(['\r', '\n']) {
        return Err("音轨地址必须是 HTTP/HTTPS".into());
    }
    if url
        .split("://")
        .nth(1)
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("")
        .contains('@')
        || source["request_method"]
            .as_str()
            .is_some_and(|v| v != "GET")
        || match source.get("request_body") {
            None | Some(Value::Null) => false,
            Some(Value::String(body)) => !body.is_empty(),
            Some(_) => true,
        }
    {
        return Err("此音轨需要登录或专用请求方式，请使用播放器实时采集".into());
    }
    if match source.get("audio_url") {
        None | Some(Value::Null) => false,
        Some(Value::String(audio_url)) => !audio_url.trim().is_empty() && audio_url.trim() != url,
        Some(_) => true,
    } {
        return Err("此媒体采用音视频分离轨道，请关闭提前翻译并使用播放器实时采集".into());
    }
    // FFmpeg 的全局自定义请求头会传播到清单的其他域名，不能转发登录凭据。
    validate_headers_field(source.get("request_headers"))?;
    // FFmpeg Cookie 机制不能完整表达浏览器 hostOnly / Secure / 分区语义。
    validate_cookie_fields(source.get("cookie"), source.get("cookies"))?;
    match source.get("request_contexts") {
        None | Some(Value::Null) => {}
        Some(Value::Object(contexts)) => {
            for context in contexts.values() {
                let Some(ctx_obj) = context.as_object() else {
                    return Err(
                        "此音轨需要专用鉴权请求头，请关闭提前翻译并使用播放器实时采集".into(),
                    );
                };
                validate_headers_field(ctx_obj.get("request_headers"))?;
                validate_cookie_fields(ctx_obj.get("cookie"), ctx_obj.get("cookies"))?;
                for nested_key in ["path_contexts", "url_contexts"] {
                    match ctx_obj.get(nested_key) {
                        None | Some(Value::Null) => {}
                        Some(Value::Object(entries)) => {
                            for entry in entries.values() {
                                let Some(entry_obj) = entry.as_object() else {
                                    return Err(
                                        "此音轨需要专用鉴权请求头，请关闭提前翻译并使用播放器实时采集"
                                            .into(),
                                    );
                                };
                                validate_headers_field(entry_obj.get("request_headers"))?;
                                validate_cookie_fields(
                                    entry_obj.get("cookie"),
                                    entry_obj.get("cookies"),
                                )?;
                            }
                        }
                        Some(_) => {
                            return Err(
                                "此音轨需要专用鉴权请求头，请关闭提前翻译并使用播放器实时采集"
                                    .into(),
                            );
                        }
                    }
                }
            }
        }
        Some(_) => {
            return Err("此音轨需要专用鉴权请求头，请关闭提前翻译并使用播放器实时采集".into());
        }
    }
    Ok(())
}

fn is_loopback_media_url(url: &str) -> bool {
    let Some(auth) = crate::net_policy::parse_url_authority(url) else {
        return false;
    };
    let host = auth.host.trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

fn build_ffmpeg_no_proxy(proxy_bypass: &str) -> String {
    let mut entries = vec![
        "127.0.0.1".to_string(),
        "localhost".to_string(),
        "::1".to_string(),
    ];
    for raw in proxy_bypass.split([',', ';']) {
        let token = raw.trim();
        if token.is_empty()
            || token.eq_ignore_ascii_case("<local>")
            || token.chars().any(char::is_control)
        {
            continue;
        }
        let cleaned = token.strip_prefix("*.").unwrap_or(token);
        if !cleaned.is_empty()
            && !entries
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(cleaned))
        {
            entries.push(cleaned.to_string());
        }
    }
    entries.join(",")
}

fn resolve_ffmpeg_proxy(
    url: &str,
    proxy: &str,
    proxy_bypass: &str,
    check: impl Fn() -> Result<(), String>,
) -> Result<(Option<String>, String), String> {
    let no_proxy = build_ffmpeg_no_proxy(proxy_bypass);
    let trimmed = proxy.trim();
    if trimmed == crate::net_policy::INVALID_MANUAL_PROXY {
        return Err("手动代理模式未配置代理服务器地址".into());
    }
    if is_loopback_media_url(url)
        || trimmed.is_empty()
        || trimmed == crate::net_policy::DIRECT_PROXY_SENTINEL
    {
        return Ok((None, no_proxy));
    }
    let resolved = if trimmed == crate::net_policy::SYSTEM_PROXY_SENTINEL {
        crate::proxy_tunnel::resolve_system_proxy(url, check)?
    } else {
        Some(trimmed.to_string())
    };
    let Some(candidate) = resolved
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return Ok((None, no_proxy));
    };
    if candidate.chars().any(char::is_control) {
        return Err("代理地址无效".into());
    }
    let lower = candidate.to_ascii_lowercase();
    if lower.starts_with("http://") {
        let normalized = format!("http://{}", &candidate[7..]);
        Ok((Some(normalized), no_proxy))
    } else if lower.starts_with("socks5://")
        || lower.starts_with("socks5h://")
        || lower.starts_with("socks4://")
        || lower.starts_with("socks4a://")
        || lower.starts_with("https://")
    {
        Err(
            "音轨预取解码器暂不支持 SOCKS/HTTPS 代理，请改用 HTTP 代理或关闭提前翻译使用播放器实时采集"
                .into(),
        )
    } else {
        Err("不支持的代理协议".into())
    }
}

pub(crate) fn decode(
    source: &Value,
    position: f64,
    seconds: f64,
    cancelled: impl Fn() -> bool,
) -> Result<Value, String> {
    decode_with_proxy(
        source,
        position,
        seconds,
        crate::net_policy::DIRECT_PROXY_SENTINEL,
        "",
        cancelled,
    )
}

fn ffmpeg_protocol_whitelist(use_http_proxy: bool) -> &'static str {
    if use_http_proxy {
        "http,https,httpproxy,tcp,tls,crypto"
    } else {
        "http,https,tcp,tls,crypto"
    }
}

fn build_ffmpeg_decode_command(
    ffmpeg: &std::path::Path,
    source: &Value,
    url: &str,
    position: f64,
    seconds: f64,
    http_proxy: Option<&str>,
    no_proxy: &str,
) -> Result<Command, String> {
    let mut command = Command::new(ffmpeg);
    for key in [
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ] {
        command.env_remove(key);
    }
    command.env("no_proxy", no_proxy).env("NO_PROXY", no_proxy);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-protocol_whitelist",
        ffmpeg_protocol_whitelist(http_proxy.is_some()),
        "-rw_timeout",
        "10000000",
    ]);
    if let Some(proxy_url) = http_proxy {
        // Supply proxy URL via environment variables only (`http_proxy` / `no_proxy` are
        // read natively by FFmpeg's `http` and `tls` protocols). Never pass `-http_proxy`
        // on argv, which would expose DPAPI-decrypted proxy credentials in process listings.
        command
            .env("http_proxy", proxy_url)
            .env("https_proxy", proxy_url)
            .env("HTTP_PROXY", proxy_url)
            .env("HTTPS_PROXY", proxy_url);
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if source["resource_kind"].as_str() == Some("hls")
        || source["mime_type"]
            .as_str()
            .is_some_and(|mime| mime.to_ascii_lowercase().contains("mpegurl"))
        || path.ends_with(".m3u8")
        || path.ends_with(".m3u")
    {
        // 站点可能以 jpeg/bin 命名音视频分片；输入协议仍限定 HTTP(S)，不开放本地文件。
        command.args(["-f", "hls"]);
        crate::media::hls_input_options(&mut command);
    }
    for (key, option) in [("referer", "-referer"), ("user_agent", "-user_agent")] {
        if let Some(value) = source[key].as_str().filter(|v| !v.is_empty()) {
            if value.contains(['\r', '\n']) {
                return Err("音轨请求头无效".into());
            }
            command.args([option, value]);
        }
    }
    if let Some(origin) = source["origin"].as_str().filter(|v| !v.is_empty()) {
        if origin.contains(['\r', '\n']) {
            return Err("音轨来源无效".into());
        }
        command.args(["-headers", &format!("Origin: {origin}\r\n")]);
    }
    // HLS 的快速 seek 可能落到前一个分片。保留归零后的媒体时间戳，再按目标时间裁切，
    // 不能把分片实际起点的音频直接贴上 position；预滚只读取附近几秒。
    // 分段音轨可能有时间戳空洞，只补齐静音，不通过伸缩语音来追赶时间轴。
    let audio_filter = format!("atrim=start={position},asetpts=PTS-{position}/TB,aresample=16000:async=1:min_hard_comp=0.001:first_pts=0,atrim=duration={seconds}");
    command
        .args([
            "-copyts",
            "-start_at_zero",
            "-ss",
            &(position - 4.0).max(0.0).to_string(),
            "-i",
            url,
            "-af",
            &audio_filter,
            "-t",
            &seconds.to_string(),
            "-map",
            &format!(
                "0:a:{}",
                source["audio_index"]
                    .as_u64()
                    .filter(|v| *v <= 128)
                    .unwrap_or(0)
            ),
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    Ok(command)
}

pub(crate) fn decode_with_proxy(
    source: &Value,
    position: f64,
    seconds: f64,
    proxy: &str,
    proxy_bypass: &str,
    cancelled: impl Fn() -> bool,
) -> Result<Value, String> {
    validate(source)?;
    let url = source["url"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("音轨地址必须是 HTTP/HTTPS")?;
    let (http_proxy, no_proxy) = resolve_ffmpeg_proxy(url, proxy, proxy_bypass, || {
        if cancelled() {
            Err("音轨预取已取消或超时，请检查媒体链接和网络".into())
        } else {
            Ok(())
        }
    })?;
    let ffmpeg = crate::media::locate_ffmpeg().ok_or("找不到音轨解码组件，请修复桌面端安装")?;
    let mut command = build_ffmpeg_decode_command(
        &ffmpeg,
        source,
        url,
        position,
        seconds,
        http_proxy.as_deref(),
        &no_proxy,
    )?;
    let mut child = command.spawn().map_err(|_| "无法启动音轨解码组件")?;
    crate::job_object::assign_child(&child);
    let stdout = child.stdout.take().ok_or("音轨解码管道不可用")?;
    // Round up to whole 16 kHz mono PCM16 (2-byte) samples so fractional durations
    // never produce an odd byte limit that rejects a valid aligned FFmpeg output.
    let limit = ((seconds * 16000.0).ceil() as u64).saturating_mul(2);
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(limit + 2)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    // 为 Core 的 15 秒帧读取期限留出进程退出和响应编码时间。
    let deadline = Instant::now() + Duration::from_secs(12);
    let result = loop {
        if cancelled() || Instant::now() >= deadline {
            break Err("音轨预取已取消或超时，请检查媒体链接和网络".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(_)) | Err(_) => {
                break Err("无法解码当前音轨，请检查链接是否过期、音轨及站点访问权限".into())
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    let bytes = reader
        .join()
        .map_err(|_| "音轨读取线程异常")?
        .map_err(|_| "音轨读取失败")?;
    result?;
    if bytes.is_empty() || bytes.len() % 2 != 0 || bytes.len() as u64 > limit {
        return Err("音轨解码没有返回有效的 PCM 音频".into());
    }
    Ok(
        json!({"ok": true, "audio": STANDARD.encode(&bytes), "position": position,
        "duration": bytes.len() as f64 / 32000.0}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the sandbox HTTP audio fixture and bundled FFmpeg"]
    fn decode_disguised_hls_audio() {
        assert_eq!(std::env::var("HLS_VM_ACCEPTANCE").as_deref(), Ok("1"));
        let origin = std::env::var("HLS_SUBTITLE_FIXTURE_ORIGIN").unwrap();
        for (path, kind) in [
            ("index.m3u8", "hls"),
            ("opaque", "hls"),
            ("sample.m4a", "media"),
        ] {
            let source = json!({"url": format!("{origin}/{path}"), "resource_kind": kind});
            let audio = decode(&source, 1.25, 1.0, || false)
                .unwrap_or_else(|error| panic!("{path}: {error}"));
            let bytes = STANDARD.decode(audio["audio"].as_str().unwrap()).unwrap();
            assert_eq!(bytes.len(), 32000, "{path}");
            assert!(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .any(|sample| i16::from_le_bytes(*sample).unsigned_abs() > 1000),
                "{path}"
            );
        }
        let blocked = json!({"url": format!("{origin}/local.m3u8"), "resource_kind": "hls"});
        assert!(decode(&blocked, 0.0, 1.0, || false).is_err());
    }

    #[test]
    fn prefetch_never_turns_browser_credentials_into_global_ffmpeg_headers() {
        let public = json!({"url":"https://cdn.example/media.m3u8?token=signed",
            "request_headers":{"sec-fetch-mode":"cors", "sec-ch-ua":"browser", "accept":"*/*"}});
        validate(&public).unwrap();
        for private in [
            json!({"url":"https://cdn.example/media", "request_headers":{"Authorization":"Bearer private"}}),
            json!({"url":"https://cdn.example/media", "cookie":"session=private"}),
            json!({"url":"https://cdn.example/media", "cookies":[{"name":"sid","value":"1"}]}),
            json!({
                "url":"https://cdn.example/master.m3u8",
                "request_contexts":{
                    "https://seg.example":{
                        "cookie":"seg_sid=secret"
                    }
                }
            }),
            json!({
                "url":"https://cdn.example/master.m3u8",
                "request_contexts":{
                    "https://cdn.example":{
                        "cookies":[{"name":"path_sid","value":"secret","path":"/segments"}]
                    }
                }
            }),
            json!({
                "url":"https://cdn.example/master.m3u8",
                "request_contexts":{
                    "https://cdn.example":{
                        "path_contexts":{
                            "/segments/":{"request_headers":{"Authorization":"Bearer seg-token"}}
                        }
                    }
                }
            }),
            json!({
                "url":"https://cdn.example/master.m3u8",
                "request_contexts":{
                    "https://cdn.example":{
                        "url_contexts":{
                            "https://cdn.example/key.bin":{"request_headers":{"X-Key-Token":"secret"}}
                        }
                    }
                }
            }),
            json!({"url":"https://cdn.example/media", "request_headers":"Authorization: secret"}),
            json!({"url":"https://cdn.example/video.m4s", "audio_url":"https://cdn.example/audio.m4s"}),
            json!({"url":"https://user:password@cdn.example/media"}),
            json!({"url":"https://cdn.example/media", "request_method":"POST"}),
            json!({"url":"https://cdn.example/media", "request_body":"a=1"}),
            json!({"url":"file:///private/media.mp4"}),
            json!({}),
            json!({"url": 42}),
        ] {
            assert!(validate(&private).is_err(), "expected error for {private}");
            assert!(decode(&private, 0.0, 1.0, || false).is_err());
        }
    }

    #[test]
    fn resolve_ffmpeg_proxy_enforces_direct_loopback_http_proxy_and_fail_closed_modes() {
        // Loopback local-media server URLs never route through configured external proxy.
        let (loopback_proxy, no_proxy) = resolve_ffmpeg_proxy(
            "http://127.0.0.1:43123/media/tok123",
            "http://127.0.0.1:8080",
            "*.corp.example",
            || Ok(()),
        )
        .unwrap();
        assert_eq!(loopback_proxy, None);
        assert!(no_proxy.contains("127.0.0.1") && no_proxy.contains("corp.example"));

        // Direct mode ignores ambient environment and yields None.
        let (direct_proxy, _) = resolve_ffmpeg_proxy(
            "https://cdn.example/audio.m4a",
            crate::net_policy::DIRECT_PROXY_SENTINEL,
            "",
            || Ok(()),
        )
        .unwrap();
        assert_eq!(direct_proxy, None);

        // Manual HTTP proxy normalizes scheme and forwards cleanly.
        let (manual_http, _) = resolve_ffmpeg_proxy(
            "https://cdn.example/audio.m4a",
            "HTTP://user:pass@127.0.0.1:8080",
            "",
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            manual_http.as_deref(),
            Some("http://user:pass@127.0.0.1:8080")
        );

        // Unconfigured manual proxy fails closed.
        assert!(resolve_ffmpeg_proxy(
            "https://cdn.example/audio.m4a",
            crate::net_policy::INVALID_MANUAL_PROXY,
            "",
            || Ok(()),
        )
        .is_err());

        // SOCKS5 / HTTPS proxies fail closed with actionable guidance instead of leaking direct.
        let socks_err = resolve_ffmpeg_proxy(
            "https://cdn.example/audio.m4a",
            "socks5://127.0.0.1:1080",
            "",
            || Ok(()),
        )
        .unwrap_err();
        assert!(socks_err.contains("SOCKS/HTTPS"), "{socks_err}");

        assert_eq!(
            ffmpeg_protocol_whitelist(false),
            "http,https,tcp,tls,crypto"
        );
        assert_eq!(
            ffmpeg_protocol_whitelist(true),
            "http,https,httpproxy,tcp,tls,crypto"
        );
    }

    #[test]
    fn ffmpeg_decode_command_passes_proxy_via_env_only_without_leaking_credentials_in_argv() {
        let source = json!({
            "url": "https://cdn.example/live/index.m3u8",
            "resource_kind": "hls",
            "audio_index": 1
        });
        let (proxy_url, no_proxy) = resolve_ffmpeg_proxy(
            "https://cdn.example/live/index.m3u8",
            "HTTP://proxy_user:SecretPass#99@127.0.0.1:8080",
            "*.intranet.local",
            || Ok(()),
        )
        .unwrap();
        let command = build_ffmpeg_decode_command(
            std::path::Path::new("ffmpeg"),
            &source,
            "https://cdn.example/live/index.m3u8",
            10.0,
            4.0,
            proxy_url.as_deref(),
            &no_proxy,
        )
        .unwrap();

        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|arg| arg == "-http_proxy"),
            "proxy flag must not appear on argv: {args:?}"
        );
        assert!(
            args.iter().all(|arg| {
                !arg.contains("SecretPass#99")
                    && !arg.contains("proxy_user")
                    && !arg.contains("127.0.0.1:8080")
            }),
            "proxy credentials or endpoint leaked into argv: {args:?}"
        );
        let wl_pos = args
            .iter()
            .position(|arg| arg == "-protocol_whitelist")
            .unwrap();
        assert_eq!(args[wl_pos + 1], "http,https,httpproxy,tcp,tls,crypto");

        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|val| val.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(
            envs.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("http_proxy")
                    && v.as_deref() == Some("http://proxy_user:SecretPass#99@127.0.0.1:8080")
            }),
            "expected http_proxy env to carry proxy URL: {envs:?}"
        );
        assert!(
            envs.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("no_proxy")
                    && v.as_deref().is_some_and(|np| np.contains("intranet.local"))
            }),
            "expected no_proxy env: {envs:?}"
        );
        assert!(
            envs.iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("all_proxy") && v.is_none()),
            "expected all_proxy env removed: {envs:?}"
        );

        let direct_cmd = build_ffmpeg_decode_command(
            std::path::Path::new("ffmpeg"),
            &source,
            "https://cdn.example/live/index.m3u8",
            0.0,
            2.0,
            None,
            &no_proxy,
        )
        .unwrap();
        let direct_args: Vec<String> = direct_cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let direct_wl = direct_args
            .iter()
            .position(|arg| arg == "-protocol_whitelist")
            .unwrap();
        assert_eq!(direct_args[direct_wl + 1], "http,https,tcp,tls,crypto");
        assert!(direct_cmd
            .get_envs()
            .any(|(k, v)| k.to_string_lossy().eq_ignore_ascii_case("http_proxy") && v.is_none()));
    }
}
