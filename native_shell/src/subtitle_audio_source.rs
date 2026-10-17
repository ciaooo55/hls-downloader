//! 有界音频窗口直接解码到内存，不生成下载任务或临时分片。
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    {
        return Err("此音轨需要登录或专用请求方式，请使用播放器实时采集".into());
    }
    // FFmpeg 的全局自定义请求头会传播到清单的其他域名，不能转发登录凭据。
    for headers in std::iter::once(source.get("request_headers"))
        .chain(
            source["request_contexts"]
                .as_object()
                .into_iter()
                .flat_map(|v| v.values())
                .map(|context| context.get("request_headers")),
        )
        .flatten()
        .filter_map(Value::as_object)
    {
        if headers.keys().any(|name| {
            let name = name.to_ascii_lowercase();
            !name.starts_with("sec-fetch-")
                && !name.starts_with("sec-ch-ua")
                && !matches!(
                    name.as_str(),
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
        }) {
            return Err("此音轨需要专用鉴权请求头，请关闭提前翻译并使用播放器实时采集".into());
        }
    }
    // FFmpeg Cookie 机制不能完整表达浏览器 hostOnly / Secure / 分区语义。
    if source["cookie"].as_str().is_some_and(|v| !v.is_empty()) {
        return Err("此音轨使用浏览器 Cookie，请关闭提前翻译并使用播放器实时采集".into());
    }
    Ok(())
}

pub(crate) fn decode(
    source: &Value,
    position: f64,
    seconds: f64,
    cancelled: impl Fn() -> bool,
) -> Result<Value, String> {
    validate(source)?;
    let ffmpeg = crate::media::locate_ffmpeg().ok_or("找不到音轨解码组件，请修复桌面端安装")?;
    let mut command = Command::new(ffmpeg);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-protocol_whitelist",
        "http,https,tcp,tls,crypto",
        "-rw_timeout",
        "10000000",
    ]);
    let url = source["url"].as_str().unwrap();
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
        command.args([
            "-f",
            "hls",
            "-allowed_extensions",
            "ALL",
            "-extension_picky",
            "0",
        ]);
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
            source["url"].as_str().unwrap(),
            "-af",
            &audio_filter,
            "-t",
            &seconds.to_string(),
            "-map",
            "0:a:0",
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
    let mut child = command.spawn().map_err(|_| "无法启动音轨解码组件")?;
    let stdout = child.stdout.take().ok_or("音轨解码管道不可用")?;
    let limit = (seconds * 32000.0).ceil() as u64;
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
            json!({"url":"https://user:password@cdn.example/media"}),
            json!({"url":"https://cdn.example/media", "request_method":"POST"}),
            json!({"url":"file:///private/media.mp4"}),
        ] {
            assert!(validate(&private).is_err());
        }
    }
}
