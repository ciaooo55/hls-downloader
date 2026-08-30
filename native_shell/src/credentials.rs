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
            return protect_windows(value.as_bytes());
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
            return unprotect_windows(&decode_hex(&value[PREFIX.len()..])?);
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
    if value.len() % 2 != 0 {
        return Err("DPAPI credential hex has odd length".into());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
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
    if replay_targets_other_origin(json, request_url) {
        remove_header(headers, "Cookie");
        remove_header(headers, "Authorization");
        remove_header(headers, "Proxy-Authorization");
    }
    apply_scoped_request_context(headers, json, request_url);
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

pub(crate) fn apply_scoped_request_context(
    headers: &mut std::collections::BTreeMap<String, String>,
    json: &str,
    request_url: &str,
) {
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
    merge_header_map(headers, scoped.get("request_headers"));
    replace_header(headers, "Referer", scoped.get("referer"));
    replace_header(headers, "Origin", scoped.get("origin"));
    insert_header(headers, "User-Agent", scoped.get("user_agent"));
    let scoped_cookie = scoped
        .get("cookie")
        .and_then(value_as_header_text)
        .unwrap_or_default();
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
        assert!(headers.get("Cookie").is_none());
    }

    #[test]
    fn replay_json_does_not_send_task_secrets_to_unscoped_cross_origin_child() {
        let replay = bind_replay_source_url(
            r#"{"cookie":"manifest=1","request_headers":{"authorization":"Bearer manifest","X-Playback":"ok"}}"#,
            "https://manifest.test/master.m3u8",
        );
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json_for(&mut headers, &replay, "https://cdn.test/segment.ts");
        assert!(headers.get("Cookie").is_none());
        assert!(headers.get("Authorization").is_none());
        assert_eq!(headers.get("X-Playback").map(String::as_str), Some("ok"));
    }

    #[test]
    fn exact_scoped_context_clears_invented_identity_and_restores_its_authorization() {
        let replay = bind_replay_source_url(
            r#"{
                "cookie":"page=1",
                "referer":"https://page.test/watch",
                "origin":"https://page.test",
                "request_headers":{"authorization":"Bearer page"},
                "request_contexts":{
                    "https://cdn.test":{
                        "cookie":"",
                        "referer":"",
                        "origin":"",
                        "request_headers":{"authorization":"Bearer cdn"}
                    }
                }
            }"#,
            "https://manifest.test/master.m3u8",
        );
        let mut headers = std::collections::BTreeMap::new();
        apply_replay_json_for(&mut headers, &replay, "https://cdn.test/segment.ts");
        assert!(headers.get("Cookie").is_none());
        assert!(headers.get("Referer").is_none());
        assert!(headers.get("Origin").is_none());
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer cdn")
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
        assert!(headers.get("Cookie").is_none());
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
