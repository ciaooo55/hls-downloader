//! WinINet resolves per-user manual settings, bypass rules and PAC for
//! non-HTTP target URLs. Only the first route selected by Windows is used.
use std::{ffi::c_void, ptr};

#[repr(C)]
struct ProxyInfo {
    proxy: i32,
    bypass: i32,
    scheme: i32,
    host: *const u16,
    port: u16,
}

#[repr(C)]
struct ProxyList {
    count: u32,
    entries: *mut ProxyInfo,
}

#[link(name = "wininet")]
unsafe extern "system" {
    fn InternetGetProxyForUrl(session: *const c_void, url: *const u16, list: *mut ProxyList)
        -> u32;
    fn InternetFreeProxyInfoList(list: *mut ProxyList);
}

pub(super) fn lookup(url: &str) -> Result<Option<String>, String> {
    let url: Vec<_> = url.encode_utf16().chain(Some(0)).collect();
    let mut list = ProxyList {
        count: 0,
        entries: ptr::null_mut(),
    };
    // A null session asks WinINet to use the current user's default configuration.
    let status = unsafe { InternetGetProxyForUrl(ptr::null(), url.as_ptr(), &mut list) };
    if status != 0 {
        return Err(format!("Windows 系统代理解析失败（{status}）"));
    }
    let result = if list.count == 0 || list.entries.is_null() {
        Err("Windows 系统代理未返回路由".into())
    } else {
        // WinINet owns the entries and their NUL-terminated host names until free.
        route(unsafe { &*list.entries })
    };
    unsafe {
        InternetFreeProxyInfoList(&mut list);
    }
    result
}

fn route(entry: &ProxyInfo) -> Result<Option<String>, String> {
    if entry.proxy == 0 || entry.bypass != 0 {
        return Ok(None);
    }
    let scheme = match entry.scheme {
        3 => "http", // INTERNET_SCHEME_HTTP
        4 => "https",
        8 => "socks4", // WinINet's SOCKS setting uses SOCKSv4.
        _ => return Err("Windows 返回了不支持的系统代理协议".into()),
    };
    if entry.host.is_null() || entry.port == 0 {
        return Err("Windows 系统代理地址无效".into());
    }
    let host = unsafe {
        let mut len = 0;
        while *entry.host.add(len) != 0 {
            len += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(entry.host, len))
    }
    .map_err(|_| "Windows 系统代理主机名无效")?;
    let host = host.trim_matches(['[', ']']);
    if !super::valid_host(host) {
        return Err("Windows 系统代理主机名无效".into());
    }
    Ok(Some(format!(
        "{scheme}://{}",
        super::authority(host, entry.port)
    )))
}
