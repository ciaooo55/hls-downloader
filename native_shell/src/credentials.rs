//! Windows DPAPI-backed credential blobs for browser replay contexts.

const PREFIX: &str = "dpapi:";

#[derive(Debug, Default, Clone, Copy)]
pub struct CredentialVault;

impl CredentialVault {
    /// DPAPI on Windows; other platforms refuse so credential-bearing flows
    /// fail closed instead of storing plaintext.
    pub fn protect(&self, value: &str) -> Result<String, String> {
        if value.is_empty() {
            return Ok(String::new());
        }
        if value.starts_with(PREFIX) {
            return Ok(value.to_string());
        }
        #[cfg(windows)]
        {
            protect_windows(value.as_bytes())
        }
        #[cfg(not(windows))]
        {
            let _ = value;
            Err("DPAPI credentials require Windows".into())
        }
    }

    pub fn unprotect(&self, value: &str) -> Result<String, String> {
        if value.is_empty() || !value.starts_with(PREFIX) {
            return Ok(value.to_string());
        }
        #[cfg(windows)]
        {
            unprotect_windows(&decode_hex(&value[PREFIX.len()..])?)
        }
        #[cfg(not(windows))]
        {
            let _ = value;
            Err("DPAPI credentials require Windows".into())
        }
    }
}

#[cfg(windows)]
fn protect_windows(value: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let mut input = value.to_vec();
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &source,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!("CryptProtectData failed: {}", unsafe {
            GetLastError()
        }));
    }
    let encrypted = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let encoded = format!("{PREFIX}{}", encode_hex(encrypted));
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(encoded)
}

#[cfg(windows)]
fn unprotect_windows(value: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let mut input = value.to_vec();
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &source,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!("CryptUnprotectData failed: {}", unsafe {
            GetLastError()
        }));
    }
    let plaintext = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let value = String::from_utf8(plaintext.to_vec())
        .map_err(|error| format!("DPAPI credential is not UTF-8: {error}"))?;
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(value)
}

fn encode_hex(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("DPAPI credential hex has odd length".into());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let high =
            hex_digit(pair[0]).ok_or_else(|| "DPAPI credential hex is invalid".to_string())?;
        let low =
            hex_digit(pair[1]).ok_or_else(|| "DPAPI credential hex is invalid".to_string())?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

thread_local! {
    static REPLAY_JSON: std::cell::RefCell<String> =
        const { std::cell::RefCell::new(String::new()) };
}

/// Run `f` with replay JSON visible to HTTP fetches on this thread (HLS/DASH hops).
pub fn with_replay_json<R>(json: &str, f: impl FnOnce() -> R) -> R {
    REPLAY_JSON.with(|slot| {
        let previous = slot.replace(json.to_string());
        let result = f();
        slot.replace(previous);
        result
    })
}

pub(crate) fn scoped_replay_json() -> String {
    REPLAY_JSON.with(|slot| slot.borrow().clone())
}

pub fn apply_replay_json(headers: &mut std::collections::BTreeMap<String, String>, json: &str) {
    apply_replay_json_for(headers, json, "");
}

/// Merge a Native Messaging / 5.x replay-context JSON into HTTP headers.
/// Cookie and Authorization stay out of TaskSpec until this runs at download time.
/// `request_url` selects `request_contexts[origin]` so CDN cookies are not replaced
/// by page cookies, matching 5.x `build_task_headers`.
pub fn apply_replay_json_for(
    headers: &mut std::collections::BTreeMap<String, String>,
    json: &str,
    request_url: &str,
) {
    apply_base_replay(headers, json);
    let cross_origin = replay_targets_other_origin(json, request_url);
    if cross_origin {
        remove_header(headers, "Cookie");
        remove_header(headers, "Authorization");
        remove_header(headers, "Proxy-Authorization");
    }
    apply_scoped_request_context_inner(headers, json, request_url, cross_origin);
}

/// Bind an in-memory replay context to the task URL. Child HLS/DASH requests
/// then inherit credentials only when they stay on that origin.
pub(crate) fn bind_replay_source_url(json: &str, source_url: &str) -> String {
    if request_origin(source_url).is_empty() {
        return json.to_string();
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(json) else {
        return json.to_string();
    };
    let Some(object) = value.as_object_mut() else {
        return json.to_string();
    };
    object.insert(
        "_task_url".into(),
        serde_json::Value::String(source_url.into()),
    );
    value.to_string()
}

fn replay_targets_other_origin(json: &str, request_url: &str) -> bool {
    let target = request_origin(request_url);
    if target.is_empty() {
        return false;
    }
    let source = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|value| value.get("_task_url").and_then(value_as_header_text))
        .map(|url| request_origin(&url))
        .unwrap_or_default();
    !source.is_empty() && source != target
}

fn apply_base_replay(headers: &mut std::collections::BTreeMap<String, String>, json: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    insert_header(headers, "Cookie", value.get("cookie"));
    insert_header(headers, "Referer", value.get("referer"));
    insert_header(headers, "Origin", value.get("origin"));
    insert_header(headers, "User-Agent", value.get("user_agent"));
    merge_header_map(headers, value.get("request_headers"));
}

fn replay_request_header_names(json: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut headers = std::collections::BTreeMap::new();
    merge_header_map(&mut headers, value.get("request_headers"));
    if let Some(contexts) = value
        .get("request_contexts")
        .and_then(|contexts| contexts.as_object())
    {
        for scoped in contexts.values() {
            merge_header_map(&mut headers, scoped.get("request_headers"));
            if let Some(paths) = scoped.get("path_contexts").and_then(|v| v.as_object()) {
                for path in paths.values() {
                    merge_header_map(&mut headers, path.get("request_headers"));
                }
            }
        }
    }
    headers.into_keys().collect()
}

fn apply_base_navigation_context(
    headers: &mut std::collections::BTreeMap<String, String>,
    json: &str,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    insert_header(headers, "Referer", value.get("referer"));
    insert_header(headers, "Origin", value.get("origin"));
    insert_header(headers, "User-Agent", value.get("user_agent"));
}

/// Apply a scoped replay context at a redirect boundary. The HTTP engine calls
/// this only after proving the redirect changed origin, so replay-controlled
/// custom headers must be cleared even when the redirect returns to `_task_url`.
pub(crate) fn apply_scoped_request_context(
    headers: &mut std::collections::BTreeMap<String, String>,
    json: &str,
    request_url: &str,
) {
    apply_scoped_request_context_inner(headers, json, request_url, true);
}

fn apply_scoped_request_context_inner(
    headers: &mut std::collections::BTreeMap<String, String>,
    json: &str,
    request_url: &str,
    clear_replay_headers: bool,
) {
    if clear_replay_headers {
        for name in replay_request_header_names(json) {
            remove_header(headers, &name);
        }
        apply_base_navigation_context(headers, json);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    let origin = request_origin(request_url);
    if origin.is_empty() {
        return;
    }
    let Some(scoped) = value
        .get("request_contexts")
        .and_then(|contexts| contexts.get(&origin))
    else {
        return;
    };
    let request_path = replay_request_path(request_url);
    let path_context = scoped
        .get("path_contexts")
        .and_then(|v| v.as_object())
        .and_then(|paths| {
            paths
                .iter()
                .filter(|(path, _)| {
                    path.starts_with('/')
                        && path.ends_with('/')
                        && request_path.starts_with(path.as_str())
                })
                .max_by_key(|(path, _)| path.len())
                .map(|(_, context)| context)
        });
    if path_context.is_some() {
        // 音轨、密钥和分片可能使用不同 Token；切换目录时替换浏览器请求头。
        for name in replay_request_header_names(json) {
            remove_header(headers, &name);
        }
    }
    let identity = path_context.unwrap_or(scoped);
    merge_header_map(headers, identity.get("request_headers"));
    replace_header(headers, "Referer", identity.get("referer"));
    replace_header(headers, "Origin", identity.get("origin"));
    insert_header(headers, "User-Agent", identity.get("user_agent"));
    let scoped_cookie = if let Some(cookies) = scoped.get("cookies").and_then(|v| v.as_array()) {
        replay_cookie_header(cookies, request_url)
    } else {
        // 已保存的任务仍可能只有旧格式的 Cookie 字符串。
        scoped
            .get("cookie")
            .and_then(value_as_header_text)
            .unwrap_or_default()
    };
    if scoped_cookie.trim().is_empty() {
        remove_header(headers, "Cookie");
    } else {
        replace_header(
            headers,
            "Cookie",
            Some(&serde_json::Value::String(scoped_cookie)),
        );
    }
}

fn replay_cookie_header(cookies: &[serde_json::Value], request_url: &str) -> String {
    let origin = request_origin(request_url);
    let Some((scheme, authority)) = origin.split_once("://") else {
        return String::new();
    };
    let host = authority.split(':').next().unwrap_or("");
    let path = replay_request_path(request_url);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let mut matching = Vec::new();
    for cookie in cookies.iter().take(256) {
        let text = |key| cookie.get(key).and_then(|v| v.as_str()).unwrap_or("");
        let domain = text("domain").trim_start_matches('.').to_ascii_lowercase();
        let cookie_path = text("path");
        let name = text("name");
        let value = text("value");
        let domain_matches = host == domain
            || (cookie.get("hostOnly").and_then(|v| v.as_bool()) == Some(false)
                && host.ends_with(&format!(".{domain}")));
        let path_matches = !cookie_path.is_empty()
            && (path == cookie_path
                || (path.starts_with(cookie_path)
                    && (cookie_path.ends_with('/')
                        || path.as_bytes().get(cookie_path.len()) == Some(&b'/'))));
        if domain.is_empty()
            || !domain_matches
            || !path_matches
            || (cookie.get("secure").and_then(|v| v.as_bool()) == Some(true) && scheme != "https")
            || cookie
                .get("expirationDate")
                .and_then(|v| v.as_f64())
                .is_some_and(|expires| expires <= now)
            || name.contains(['\r', '\n', '\0', ';'])
            || value.contains(['\r', '\n', '\0', ';'])
        {
            continue;
        }
        matching.push((cookie_path.len(), format!("{name}={value}")));
    }
    matching.sort_by_key(|(length, _)| std::cmp::Reverse(*length));
    matching
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>()
        .join("; ")
}

fn replay_request_path(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    let start = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let target = &rest[start..];
    if !target.starts_with('/') {
        return "/";
    }
    target.split(['?', '#']).next().unwrap_or("/")
}

pub(crate) fn request_origin(url: &str) -> String {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return String::new();
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return String::new();
    }
    let hostport = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if hostport.is_empty() {
        return String::new();
    }
    let hostport = if scheme == "https" {
        hostport
            .strip_suffix(":443")
            .unwrap_or(&hostport)
            .to_string()
    } else {
        hostport
            .strip_suffix(":80")
            .unwrap_or(&hostport)
            .to_string()
    };
    format!("{scheme}://{hostport}")
}

fn insert_header(
    headers: &mut std::collections::BTreeMap<String, String>,
    name: &str,
    value: Option<&serde_json::Value>,
) {
    let Some(text) = value.and_then(value_as_header_text) else {
        return;
    };
    if text.trim().is_empty() || text.contains('\r') || text.contains('\n') {
        return;
    }
    headers.insert(name.to_string(), text);
}

fn replace_header(
    headers: &mut std::collections::BTreeMap<String, String>,
    name: &str,
    value: Option<&serde_json::Value>,
) {
    remove_header(headers, name);
    insert_header(headers, name, value);
}

fn remove_header(headers: &mut std::collections::BTreeMap<String, String>, name: &str) {
    let keys: Vec<String> = headers
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(name))
        .cloned()
        .collect();
    for key in keys {
        headers.remove(&key);
    }
}

fn value_as_header_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn merge_header_map(
    headers: &mut std::collections::BTreeMap<String, String>,
    value: Option<&serde_json::Value>,
) {
    let Some(value) = value else {
        return;
    };
    let object = match value {
        serde_json::Value::Object(map) => Some(map.clone()),
        serde_json::Value::String(text) => {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text).ok()
        }
        _ => None,
    };
    let Some(object) = object else {
        return;
    };
    for (key, val) in object {
        if !replay_header_name_ok(&key) {
            continue;
        }
        if let Some(text) = value_as_header_text(&val) {
            if !text.trim().is_empty() && !text.contains('\r') && !text.contains('\n') {
                headers.insert(canonical_header_name(&key), text);
            }
        }
    }
}

fn replay_header_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\r', '\n', ':', '\0'])
        && !matches!(
            name.to_ascii_lowercase().as_str(),
            "host"
                | "content-length"
                | "connection"
                | "range"
                | "if-range"
                | "accept-encoding"
                | "transfer-encoding"
                | "te"
                | "upgrade"
                | "trailer"
                | "keep-alive"
                | "proxy-connection"
        )
}

fn canonical_header_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "cookie" => "Cookie".into(),
        "referer" | "referrer" => "Referer".into(),
        "origin" => "Origin".into(),
        "user-agent" => "User-Agent".into(),
        "authorization" => "Authorization".into(),
        _ => name.to_string(),
    }
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_selects_cookie_path_for_each_adaptive_media_request() {
        let json = r#"{"cookie":"page=wrong","request_contexts":{"https://cdn.test":{
            "cookie":"legacy=wrong","referer":"https://site.test/watch","cookies":[
                {"name":"session","value":"root","domain":"cdn.test","path":"/","hostOnly":true},
                {"name":"session","value":"chunks","domain":"cdn.test","path":"/chunks","hostOnly":true},
                {"name":"session","value":"init","domain":"cdn.test","path":"/init","hostOnly":true},
                {"name":"session","value":"foreign","domain":"other.test","path":"/","hostOnly":true},
                {"name":"expired","value":"no","domain":"cdn.test","path":"/","expirationDate":1}
            ]}}}"#;
        for (url, expected) in [
            (
                "https://cdn.test/chunks/1.ts?token=1",
                "session=chunks; session=root",
            ),
            (
                "https://cdn.test/init/video.mp4",
                "session=init; session=root",
            ),
            ("https://cdn.test/chunksevil/1.ts", "session=root"),
        ] {
            let mut headers = std::collections::BTreeMap::new();
            apply_replay_json_for(&mut headers, json, url);
            assert_eq!(headers.get("Cookie").map(String::as_str), Some(expected));
        }
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json_for(
            &mut headers,
            r#"{"request_contexts":{"https://cdn.test":{"cookie":"stale=1","cookies":[]}}}"#,
            "https://cdn.test/file",
        );
        assert!(!headers.contains_key("Cookie"));
    }

    #[test]
    fn replay_cookie_snapshot_enforces_secure_and_host_only() {
        let cookies = serde_json::json!([
            {"name":"a","value":"1","domain":".cdn.test","path":"/","hostOnly":false},
            {"name":"b","value":"2","domain":"cdn.test","path":"/","hostOnly":true},
            {"name":"c","value":"3","domain":"cdn.test","path":"/","secure":true}
        ]);
        let cookies = cookies.as_array().unwrap();
        assert_eq!(
            replay_cookie_header(cookies, "http://cdn.test/file"),
            "a=1; b=2"
        );
        assert_eq!(
            replay_cookie_header(cookies, "https://sub.cdn.test/file"),
            "a=1"
        );
        assert_eq!(
            replay_cookie_header(cookies, "https://cdn.test/file"),
            "a=1; b=2; c=3"
        );
    }

    #[test]
    fn replay_uses_directory_request_identity_without_leaking_manifest_token() {
        let json = r#"{"request_headers":{"Authorization":"Bearer manifest","X-Manifest":"private"},
            "request_contexts":{"https://cdn.test":{"cookies":[],"path_contexts":{
                "/init/":{"referer":"https://frame.test/player","origin":"","request_headers":{"Authorization":"Bearer init"}},
                "/chunks/":{"referer":"https://frame.test/player","origin":"https://frame.test","request_headers":{"X-Playback-Token":"chunks"}},
                "/chunks/audio/":{"referer":"https://frame.test/player","origin":"","request_headers":{"Authorization":"Bearer audio"}}
            }}}}"#;
        for (url, auth, token, origin) in [
            (
                "https://cdn.test/init/video.mp4",
                Some("Bearer init"),
                None,
                None,
            ),
            (
                "https://cdn.test/chunks/1.m4s",
                None,
                Some("chunks"),
                Some("https://frame.test"),
            ),
            (
                "https://cdn.test/chunks/audio/1.m4s",
                Some("Bearer audio"),
                None,
                None,
            ),
        ] {
            let mut headers = std::collections::BTreeMap::new();
            apply_replay_json_for(&mut headers, json, url);
            assert_eq!(headers.get("Authorization").map(String::as_str), auth);
            assert_eq!(headers.get("X-Playback-Token").map(String::as_str), token);
            assert_eq!(headers.get("Origin").map(String::as_str), origin);
            assert_eq!(
                headers.get("Referer").map(String::as_str),
                Some("https://frame.test/player")
            );
            assert!(!headers.contains_key("X-Manifest"));
        }
    }

    #[test]
    fn empty_and_plain_values_are_stable() {
        let vault = CredentialVault;
        assert_eq!(vault.protect("").unwrap(), "");
        #[cfg(not(windows))]
        assert!(vault.protect("secret").is_err());
    }

    #[test]
    fn hex_codec_roundtrips() {
        let value = b"credential\0blob";
        assert_eq!(decode_hex(&encode_hex(value)).unwrap(), value);
    }

    #[test]
    fn replay_json_promotes_cookie_and_request_headers() {
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json(
            &mut headers,
            r#"{"cookie":"sid=1","referer":"https://site.test/","request_headers":{"User-Agent":"UA","authorization":"Bearer x"}}"#,
        );
        assert_eq!(headers.get("Cookie").unwrap(), "sid=1");
        assert_eq!(headers.get("Referer").unwrap(), "https://site.test/");
        assert_eq!(headers.get("User-Agent").unwrap(), "UA");
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer x");
    }

    #[test]
    fn replay_json_uses_origin_scoped_cookie_for_cdn_url() {
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json_for(
            &mut headers,
            r#"{
                "cookie":"page=1",
                "referer":"https://site.test/watch",
                "request_contexts":{
                    "https://cdn.test":{"cookie":"cdn=2","referer":"https://site.test/watch"}
                }
            }"#,
            "https://cdn.test/video.bin",
        );
        assert_eq!(headers.get("Cookie").unwrap(), "cdn=2");
        assert_eq!(headers.get("Referer").unwrap(), "https://site.test/watch");
    }

    #[test]
    fn replay_json_does_not_send_page_cookie_to_scoped_cdn_without_cookie() {
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json_for(
            &mut headers,
            r#"{"cookie":"page=1","request_contexts":{"https://cdn.test":{"cookie":""}}}"#,
            "https://cdn.test/video.bin",
        );
        assert!(!headers.contains_key("Cookie"));
    }

    #[test]
    fn replay_json_keeps_base_request_headers_on_same_origin() {
        let replay = bind_replay_source_url(
            r#"{"cookie":"manifest=1","request_headers":{"authorization":"Bearer manifest","X-Playback":"same-origin"}}"#,
            "https://manifest.test/master.m3u8",
        );
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json_for(&mut headers, &replay, "https://manifest.test/segment.ts");
        assert_eq!(
            headers.get("Cookie").map(String::as_str),
            Some("manifest=1")
        );
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer manifest")
        );
        assert_eq!(
            headers.get("X-Playback").map(String::as_str),
            Some("same-origin")
        );
    }

    #[test]
    fn replay_json_does_not_send_base_request_headers_to_unscoped_cross_origin_child() {
        let replay = bind_replay_source_url(
            r#"{
                "cookie":"manifest=1",
                "referer":"https://page.test/watch",
                "origin":"https://page.test",
                "user_agent":"Browser UA",
                "request_headers":{
                    "authorization":"Bearer manifest",
                    "X-Playback":"secret",
                    "X-Api-Key":"private"
                }
            }"#,
            "https://manifest.test/master.m3u8",
        );
        let mut headers = std::collections::BTreeMap::from([(
            "X-Task-Header".to_string(),
            "task-owned".to_string(),
        )]);
        apply_replay_json_for(&mut headers, &replay, "https://cdn.test/segment.ts");
        assert!(!headers.contains_key("Cookie"));
        assert!(!headers.contains_key("Authorization"));
        assert!(!headers.contains_key("X-Playback"));
        assert!(!headers.contains_key("X-Api-Key"));
        assert_eq!(
            headers.get("X-Task-Header").map(String::as_str),
            Some("task-owned")
        );
        assert_eq!(
            headers.get("Referer").map(String::as_str),
            Some("https://page.test/watch")
        );
        assert_eq!(
            headers.get("Origin").map(String::as_str),
            Some("https://page.test")
        );
        assert_eq!(
            headers.get("User-Agent").map(String::as_str),
            Some("Browser UA")
        );
    }

    #[test]
    fn scoped_context_filter_covers_cross_origin_redirect_handoff() {
        let replay = bind_replay_source_url(
            r#"{
                "referer":"https://page.test/watch",
                "request_headers":{"X-Api-Key":"source-secret"},
                "request_contexts":{
                    "https://cdn.test":{
                        "referer":"https://page.test/watch",
                        "request_headers":{"X-Cdn-Token":"target-secret"}
                    }
                }
            }"#,
            "https://manifest.test/master.m3u8",
        );
        let mut headers = std::collections::BTreeMap::from([
            ("Referer".to_string(), "https://page.test/watch".to_string()),
            ("X-Api-Key".to_string(), "source-secret".to_string()),
        ]);
        apply_scoped_request_context(&mut headers, &replay, "https://cdn.test/redirected.ts");
        assert!(!headers.contains_key("X-Api-Key"));
        assert_eq!(
            headers.get("X-Cdn-Token").map(String::as_str),
            Some("target-secret")
        );
        assert_eq!(
            headers.get("Referer").map(String::as_str),
            Some("https://page.test/watch")
        );
    }

    #[test]
    fn scoped_context_filter_drops_previous_origin_headers_on_redirect_chain() {
        let replay = bind_replay_source_url(
            r#"{
                "request_contexts":{
                    "https://cdn-a.test":{
                        "request_headers":{"X-Cdn-A-Token":"a-secret"}
                    },
                    "https://cdn-b.test":{
                        "request_headers":{"X-Cdn-B-Token":"b-secret"}
                    }
                }
            }"#,
            "https://manifest.test/master.m3u8",
        );
        let mut headers = std::collections::BTreeMap::from([(
            "X-Task-Header".to_string(),
            "task-owned".to_string(),
        )]);
        apply_scoped_request_context(&mut headers, &replay, "https://cdn-a.test/first.ts");
        assert_eq!(
            headers.get("X-Cdn-A-Token").map(String::as_str),
            Some("a-secret")
        );
        apply_scoped_request_context(&mut headers, &replay, "https://cdn-b.test/second.ts");
        assert!(!headers.contains_key("X-Cdn-A-Token"));
        assert_eq!(
            headers.get("X-Cdn-B-Token").map(String::as_str),
            Some("b-secret")
        );
        assert_eq!(
            headers.get("X-Task-Header").map(String::as_str),
            Some("task-owned")
        );
    }

    #[test]
    fn scoped_context_filter_clears_previous_origin_on_return_to_source() {
        let replay = bind_replay_source_url(
            r#"{
                "request_contexts":{
                    "https://cdn.test":{
                        "request_headers":{"X-Cdn-Token":"cdn-secret"}
                    }
                }
            }"#,
            "https://manifest.test/master.m3u8",
        );
        let mut headers = std::collections::BTreeMap::from([
            ("X-Cdn-Token".to_string(), "cdn-secret".to_string()),
            ("X-Task-Header".to_string(), "task-owned".to_string()),
        ]);
        apply_scoped_request_context(&mut headers, &replay, "https://manifest.test/return.ts");
        assert!(!headers.contains_key("X-Cdn-Token"));
        assert_eq!(
            headers.get("X-Task-Header").map(String::as_str),
            Some("task-owned")
        );
    }

    #[test]
    fn exact_scoped_context_clears_base_identity_and_restores_target_headers() {
        let replay = bind_replay_source_url(
            r#"{
                "cookie":"page=1",
                "referer":"https://page.test/watch",
                "origin":"https://page.test",
                "request_headers":{
                    "authorization":"Bearer page",
                    "X-Playback":"page-secret"
                },
                "request_contexts":{
                    "https://cdn.test":{
                        "cookie":"",
                        "referer":"",
                        "origin":"",
                        "request_headers":{
                            "authorization":"Bearer cdn",
                            "X-Playback":"cdn-scoped",
                            "X-Cdn-Token":"cdn-only"
                        }
                    }
                }
            }"#,
            "https://manifest.test/master.m3u8",
        );
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json_for(&mut headers, &replay, "https://cdn.test/segment.ts");
        assert!(!headers.contains_key("Cookie"));
        assert!(!headers.contains_key("Referer"));
        assert!(!headers.contains_key("Origin"));
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer cdn")
        );
        assert_eq!(
            headers.get("X-Playback").map(String::as_str),
            Some("cdn-scoped")
        );
        assert_eq!(
            headers.get("X-Cdn-Token").map(String::as_str),
            Some("cdn-only")
        );
    }

    #[test]
    fn browser_page_identity_is_fallback_and_exact_origin_context_wins() {
        let replay = r#"{
            "cookie":"page=1",
            "referer":"https://site.test/watch/42",
            "origin":"https://site.test",
            "user_agent":"Browser UA",
            "request_contexts":{
                "https://cdn.test":{
                    "cookie":"cdn=2",
                    "referer":"https://embed.test/player",
                    "origin":"https://embed.test",
                    "user_agent":"Browser UA"
                }
            }
        }"#;
        let mut page_headers = std::collections::BTreeMap::new();
        apply_replay_json_for(
            &mut page_headers,
            replay,
            "https://site.test/download/file.mp4",
        );
        assert_eq!(
            page_headers.get("Referer").unwrap(),
            "https://site.test/watch/42"
        );
        assert_eq!(page_headers.get("Origin").unwrap(), "https://site.test");
        assert_eq!(page_headers.get("Cookie").unwrap(), "page=1");

        let mut cdn_headers = std::collections::BTreeMap::new();
        apply_replay_json_for(
            &mut cdn_headers,
            replay,
            "https://cdn.test/video/segment-1.m4s",
        );
        assert_eq!(
            cdn_headers.get("Referer").unwrap(),
            "https://embed.test/player"
        );
        assert_eq!(cdn_headers.get("Origin").unwrap(), "https://embed.test");
        assert_eq!(cdn_headers.get("Cookie").unwrap(), "cdn=2");
    }

    #[test]
    fn replay_json_drops_crlf_header_names_and_hop_by_hop() {
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json(
            &mut headers,
            r#"{"request_headers":{"X-Foo\r\nCookie":"stolen=1","Transfer-Encoding":"chunked","X-Trace":"ok"}}"#,
        );
        assert!(!headers.contains_key("Cookie"));
        assert!(headers
            .keys()
            .all(|key| !key.eq_ignore_ascii_case("transfer-encoding")));
        assert_eq!(headers.get("X-Trace").unwrap(), "ok");
        assert!(headers
            .keys()
            .all(|key| !key.contains('\r') && !key.contains('\n')));
    }

    #[test]
    fn with_replay_json_restores_previous_scope() {
        with_replay_json("outer", || {
            assert_eq!(scoped_replay_json(), "outer");
            with_replay_json("inner", || {
                assert_eq!(scoped_replay_json(), "inner");
            });
            assert_eq!(scoped_replay_json(), "outer");
        });
        assert!(scoped_replay_json().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_is_user_bound() {
        let vault = CredentialVault;
        let protected = vault.protect("session=secret").unwrap();
        assert!(protected.starts_with(PREFIX));
        assert_eq!(vault.unprotect(&protected).unwrap(), "session=secret");
    }
}
