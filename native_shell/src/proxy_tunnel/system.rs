//! WinINet resolves per-user manual settings, bypass rules and PAC for
//! target URLs, falling back across the returned proxy list when needed.
use std::{ffi::c_void, ptr};

#[repr(C)]
pub(super) struct ProxyInfo {
    pub(super) proxy: i32,
    pub(super) bypass: i32,
    pub(super) scheme: i32,
    pub(super) host: *const u16,
    pub(super) port: u16,
}

#[repr(C)]
struct ProxyList {
    count: u32,
    entries: *mut ProxyInfo,
}

const MAX_PROXY_HOST_UTF16_UNITS: usize = 1024;

#[link(name = "wininet")]
// SAFETY: `InternetGetProxyForUrl` and `InternetFreeProxyInfoList` match the `wininet.dll`
// `stdcall` ABI for `INTERNET_PROXY_INFO_LIST` / `INTERNET_PROXY_INFO_EX`.
unsafe extern "system" {
    fn InternetGetProxyForUrl(session: *const c_void, url: *const u16, list: *mut ProxyList)
        -> u32;
    fn InternetFreeProxyInfoList(list: *mut ProxyList);
}

pub(super) fn lookup_candidates(target_url: &str) -> Result<Vec<String>, String> {
    if target_url.chars().any(char::is_control) {
        return Err("目标地址无效".into());
    }
    let (scheme, rest) = target_url.split_once("://").ok_or("目标地址无效")?;
    let scheme = scheme.trim().to_ascii_lowercase();
    if scheme.is_empty() {
        return Err("目标地址无效".into());
    }
    let without_fragment = rest.split_once('#').map_or(rest, |(head, _)| head);
    let (authority, tail) = match without_fragment.find(['/', '?']) {
        Some(pos) => (&without_fragment[..pos], &without_fragment[pos..]),
        None => (without_fragment, ""),
    };
    let endpoint = authority.rsplit('@').next().unwrap_or(authority);
    if endpoint.is_empty() {
        return Err("目标地址无效".into());
    }
    let suffix = if tail.is_empty() {
        "/".to_string()
    } else if tail.starts_with('/') {
        tail.to_string()
    } else {
        format!("/{tail}")
    };
    let schemes: &[&str] = match scheme.as_str() {
        "ftps" => &["ftp", "https"],
        "sftp" => &["https"],
        _ => &[scheme.as_str()],
    };
    Ok(schemes
        .iter()
        .map(|normalized| format!("{normalized}://{endpoint}{suffix}"))
        .collect())
}

pub(super) fn lookup(target_url: &str, allow_https: bool) -> Result<Option<String>, String> {
    let candidates = lookup_candidates(target_url)?;
    let mut first_error = None;
    let mut saw_direct = false;
    for candidate in &candidates {
        match lookup_url(candidate, allow_https) {
            Ok(Some(proxy)) => return Ok(Some(proxy)),
            Ok(None) => saw_direct = true,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    if saw_direct {
        Ok(None)
    } else {
        Err(first_error.unwrap_or_else(|| "Windows 系统代理未返回路由".into()))
    }
}

fn lookup_url(url: &str, allow_https: bool) -> Result<Option<String>, String> {
    if url.contains('\0') {
        return Err("目标地址无效".into());
    }
    let url: Vec<_> = url.encode_utf16().chain(Some(0)).collect();
    let mut list = ProxyList {
        count: 0,
        entries: ptr::null_mut(),
    };
    // SAFETY: Null session requests the current user's WinINet proxy configuration,
    // `url` is a NUL-terminated UTF-16 buffer without interior NULs, and `list` is a
    // valid writable `INTERNET_PROXY_INFO_LIST` struct.
    let status = unsafe { InternetGetProxyForUrl(ptr::null(), url.as_ptr(), &mut list) };
    if status != 0 {
        return Err(format!("Windows 系统代理解析失败（{status}）"));
    }
    struct ProxyListGuard(ProxyList);
    impl Drop for ProxyListGuard {
        fn drop(&mut self) {
            // SAFETY: `InternetGetProxyForUrl` succeeded (`ERROR_SUCCESS`), so WinINet owns
            // `self.0` until `InternetFreeProxyInfoList` releases it exactly once.
            unsafe {
                InternetFreeProxyInfoList(&mut self.0);
            }
        }
    }
    let guard = ProxyListGuard(list);
    if guard.0.count == 0 || guard.0.entries.is_null() {
        Err("Windows 系统代理未返回路由".into())
    } else {
        // SAFETY: WinINet populated `guard.0.entries` with `guard.0.count` contiguous
        // `ProxyInfo` elements that remain valid until `guard` drops at scope exit.
        let entries =
            unsafe { std::slice::from_raw_parts(guard.0.entries, guard.0.count as usize) };
        select_route(entries, allow_https)
    }
}

pub(super) fn select_route(
    entries: &[ProxyInfo],
    allow_https: bool,
) -> Result<Option<String>, String> {
    let mut first_error = None;
    for entry in entries {
        match route(entry, allow_https) {
            Ok(selected) => return Ok(selected),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "Windows 系统代理未返回路由".into()))
}

fn route(entry: &ProxyInfo, allow_https: bool) -> Result<Option<String>, String> {
    if entry.proxy == 0 || entry.bypass != 0 {
        return Ok(None);
    }
    let scheme = match entry.scheme {
        3 => "http", // INTERNET_SCHEME_HTTP
        4 if allow_https => "https",
        4 => return Err("原生 FTP/SFTP 尚不支持到 HTTPS 代理的 TLS 连接".into()),
        8 => "socks4", // WinINet's SOCKS setting uses SOCKSv4.
        _ => return Err("Windows 返回了不支持的系统代理协议".into()),
    };
    if entry.host.is_null() || entry.port == 0 {
        return Err("Windows 系统代理地址无效".into());
    }
    // SAFETY: `entry.host` is a non-null `PCWSTR` owned by WinINet (or a NUL-terminated
    // test fixture buffer) that stays valid for the duration of `route`. We cap the scan
    // at `MAX_PROXY_HOST_UTF16_UNITS` and reject missing NUL terminators before slicing.
    let host_units = unsafe {
        let mut len = 0usize;
        while len < MAX_PROXY_HOST_UTF16_UNITS && *entry.host.add(len) != 0 {
            len += 1;
        }
        if len == MAX_PROXY_HOST_UTF16_UNITS {
            return Err("Windows 系统代理主机名无效".into());
        }
        std::slice::from_raw_parts(entry.host, len)
    };
    let host = String::from_utf16(host_units).map_err(|_| "Windows 系统代理主机名无效")?;
    let host = host.trim_matches(['[', ']']);
    if !super::valid_host(host) {
        return Err("Windows 系统代理主机名无效".into());
    }
    Ok(Some(format!(
        "{scheme}://{}",
        super::authority(host, entry.port)
    )))
}
