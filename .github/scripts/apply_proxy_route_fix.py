from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def sub_once(text: str, pattern: str, replacement: str, label: str, flags: int = 0) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=flags)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return updated


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def patch_net_policy() -> None:
    path = "native_shell/src/net_policy.rs"
    text = read(path)
    core_pattern = r'''pub fn effective_proxy\(\n.*?\n\}\n\npub const DIRECT_PROXY_SENTINEL: &str = "hls-downloader://direct-proxy";'''
    core_replacement = '''pub const DIRECT_PROXY_SENTINEL: &str = "hls-downloader://direct-proxy";
pub const SYSTEM_PROXY_SENTINEL: &str = "hls-downloader://system-proxy";

pub fn proxy_route_sentinel(proxy: &str) -> bool {
    matches!(proxy.trim(), DIRECT_PROXY_SENTINEL | SYSTEM_PROXY_SENTINEL)
}

pub fn effective_proxy(
    mode: &str,
    configured: &str,
    bypass: &str,
    url: &str,
    spec_proxy: &str,
) -> String {
    let (host, _) = url_host_port(url);
    if host_bypassed(&host, bypass) {
        return DIRECT_PROXY_SENTINEL.into();
    }
    let spec_proxy = spec_proxy.trim();
    if proxy_route_sentinel(spec_proxy) || !spec_proxy.is_empty() {
        return spec_proxy.to_string();
    }
    match mode.trim().to_ascii_lowercase().as_str() {
        "direct" => DIRECT_PROXY_SENTINEL.into(),
        "manual" => configured.trim().to_string(),
        _ => SYSTEM_PROXY_SENTINEL.into(),
    }
}'''
    text = sub_once(text, core_pattern, core_replacement, "net_policy effective_proxy", re.S)

    test_pattern = r'''    #\[test\]\n    fn weekday_and_proxy_bypass\(\) \{.*?\n    \}\n(?=\n    #\[test\]\n    fn allowed_hosts_support_exact_and_subdomain_patterns)'''
    test_replacement = '''    #[test]
    fn weekday_and_proxy_bypass() {
        assert!(weekday_allowed_at("", 3));
        assert!(weekday_allowed_at("1,2,3,4,5,6,7", 7));
        assert!(!weekday_allowed_at("1,2,3,4,5", 6));
        assert!(weekday_allowed_at("6,7", 7));
        assert!(host_bypassed("intranet", "<local>"));
        assert!(host_bypassed("cdn.example.test", "example.test"));
        assert!(!host_bypassed("cdn.example.test", "other.test"));
        assert_eq!(
            effective_proxy("direct", "http://127.0.0.1:9", "", "https://cdn.test/a", ""),
            DIRECT_PROXY_SENTINEL
        );
        for url in [
            "https://cdn.test/a",
            "https://cdn.test:8443/a",
            "https://user:secret@cdn.test/a",
        ] {
            assert_eq!(
                effective_proxy("manual", "http://127.0.0.1:9", "cdn.test", url, ""),
                DIRECT_PROXY_SENTINEL
            );
        }
        assert_eq!(
            effective_proxy("manual", "http://127.0.0.1:9", "", "https://cdn.test/a", ""),
            "http://127.0.0.1:9"
        );
        assert_eq!(
            effective_proxy("system", "http://127.0.0.1:9", "", "https://cdn.test/a", ""),
            SYSTEM_PROXY_SENTINEL
        );
        assert_eq!(
            effective_proxy(
                "direct",
                "http://127.0.0.1:9",
                "",
                "https://cdn.test/a",
                SYSTEM_PROXY_SENTINEL,
            ),
            SYSTEM_PROXY_SENTINEL
        );
        assert_eq!(
            effective_proxy(
                "direct",
                "http://127.0.0.1:9",
                "",
                "https://cdn.test/a",
                "http://127.0.0.1:7777",
            ),
            "http://127.0.0.1:7777"
        );
    }
'''
    text = sub_once(text, test_pattern, test_replacement, "net_policy proxy test", re.S)
    write(path, text)


def patch_download_worker() -> None:
    path = "native_shell/src/download_worker.rs"
    text = read(path)

    text = sub_once(
        text,
        r'''\n\s*if spec\.proxy\.trim\(\)\.is_empty\(\) \{\s*spec\.proxy = settings\.proxy_url\.clone\(\);\s*\}''',
        "",
        "download_worker global proxy prefill",
        re.S,
    )
    text = sub_once(
        text,
        r'''if spec\.proxy != crate::net_policy::DIRECT_PROXY_SENTINEL\s*&& !proxy_url_allowed\(&spec\.proxy\)\s*\{\s*return Err\("代理地址无效"\.into\(\)\);\s*\}''',
        '''if !crate::net_policy::proxy_route_sentinel(&spec.proxy)
            && !proxy_url_allowed(&spec.proxy)
        {
            return Err("代理地址无效".into());
        }''',
        "download_worker proxy validation",
        re.S,
    )
    text = sub_once(
        text,
        r'''(?m)^(\s*)"direct" => spec\.proxy = crate::net_policy::DIRECT_PROXY_SENTINEL\.into\(\),\n\1"manual" if !rule\.proxy\.trim\(\)\.is_empty\(\) => spec\.proxy = rule\.proxy\.clone\(\),''',
        r'''\1"direct" => spec.proxy = crate::net_policy::DIRECT_PROXY_SENTINEL.into(),
\1"system" => spec.proxy = crate::net_policy::SYSTEM_PROXY_SENTINEL.into(),
\1"manual" if !rule.proxy.trim().is_empty() => spec.proxy = rule.proxy.clone(),''',
        "download_worker site system route",
    )

    test_anchor = '''    #[test]
    fn advanced_defaults_are_applied_and_host_scope_is_enforced() {'''
    if text.count(test_anchor) != 1:
        raise SystemExit(f"download_worker test anchor: expected one match, found {text.count(test_anchor)}")
    route_test = '''    #[test]
    fn proxy_route_identity_preserves_global_and_site_modes() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_settings(BTreeMap::from([
                ("proxy_mode".into(), serde_json::json!("system")),
                ("proxy_url".into(), serde_json::json!("http://127.0.0.1:9999")),
            ]))
            .unwrap();
        let global_system = coordinator
            .apply_defaults_to_spec(TaskSpec {
                url: "https://other.test/system.bin".into(),
                filename: "system.bin".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            global_system.proxy,
            crate::net_policy::SYSTEM_PROXY_SENTINEL
        );

        let system_rule = crate::format_site_rules(&[crate::SiteRule {
            host: "cdn.test".into(),
            proxy_mode: "system".into(),
            ..Default::default()
        }]);
        coordinator
            .set_settings(BTreeMap::from([
                ("proxy_mode".into(), serde_json::json!("manual")),
                ("proxy_url".into(), serde_json::json!("http://127.0.0.1:9999")),
                ("site_rules".into(), serde_json::json!(system_rule)),
                ("proxy_bypass".into(), serde_json::json!("")),
            ]))
            .unwrap();
        let site_system = coordinator
            .apply_defaults_to_spec(TaskSpec {
                url: "https://cdn.test/system.bin".into(),
                filename: "system.bin".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(site_system.proxy, crate::net_policy::SYSTEM_PROXY_SENTINEL);

        let direct_rule = crate::format_site_rules(&[crate::SiteRule {
            host: "cdn.test".into(),
            proxy_mode: "direct".into(),
            ..Default::default()
        }]);
        coordinator
            .set_setting("site_rules", serde_json::json!(direct_rule))
            .unwrap();
        let site_direct = coordinator
            .apply_defaults_to_spec(TaskSpec {
                url: "https://cdn.test/direct.bin".into(),
                filename: "direct.bin".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(site_direct.proxy, crate::net_policy::DIRECT_PROXY_SENTINEL);

        let manual_rule = crate::format_site_rules(&[crate::SiteRule {
            host: "cdn.test".into(),
            proxy_mode: "manual".into(),
            proxy: "http://127.0.0.1:7777".into(),
            ..Default::default()
        }]);
        coordinator
            .set_settings(BTreeMap::from([
                ("proxy_mode".into(), serde_json::json!("direct")),
                ("site_rules".into(), serde_json::json!(manual_rule)),
            ]))
            .unwrap();
        let site_manual = coordinator
            .apply_defaults_to_spec(TaskSpec {
                url: "https://cdn.test/manual.bin".into(),
                filename: "manual.bin".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(site_manual.proxy, "http://127.0.0.1:7777");

        coordinator
            .set_setting("proxy_bypass", serde_json::json!("cdn.test"))
            .unwrap();
        let bypassed = coordinator
            .apply_defaults_to_spec(TaskSpec {
                url: "https://cdn.test/bypass.bin".into(),
                filename: "bypass.bin".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(bypassed.proxy, crate::net_policy::DIRECT_PROXY_SENTINEL);
    }

'''
    text = text.replace(test_anchor, route_test + test_anchor, 1)
    write(path, text)


def patch_http_engine() -> None:
    path = "native_shell/src/http_engine.rs"
    text = read(path)

    origin_anchor = '''fn origin_connect_key(proxy: &str, host: &str, port: u16) -> String {
    format!("{proxy}|{host}|{port}")
}'''
    route_helpers = '''#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProxyRoute<'a> {
    Direct,
    System,
    Manual(&'a str),
}

fn proxy_route(proxy: &str) -> ProxyRoute<'_> {
    let proxy = proxy.trim();
    if proxy.is_empty() || proxy == crate::net_policy::DIRECT_PROXY_SENTINEL {
        ProxyRoute::Direct
    } else if proxy == crate::net_policy::SYSTEM_PROXY_SENTINEL {
        ProxyRoute::System
    } else {
        ProxyRoute::Manual(proxy)
    }
}

#[cfg(test)]
#[test]
fn proxy_route_identity_distinguishes_transports() {
    assert_eq!(proxy_route(""), ProxyRoute::Direct);
    assert_eq!(
        proxy_route(crate::net_policy::DIRECT_PROXY_SENTINEL),
        ProxyRoute::Direct
    );
    assert_eq!(
        proxy_route(crate::net_policy::SYSTEM_PROXY_SENTINEL),
        ProxyRoute::System
    );
    assert_eq!(
        proxy_route("http://127.0.0.1:8080"),
        ProxyRoute::Manual("http://127.0.0.1:8080")
    );
}

''' + origin_anchor
    text = replace_once(text, origin_anchor, route_helpers, "http_engine route helper")

    text = replace_once(
        text,
        '''        let use_winhttp = parsed.https || !hop.proxy.trim().is_empty();''',
        '''        let use_winhttp =
            parsed.https || !matches!(proxy_route(&hop.proxy), ProxyRoute::Direct);''',
        "http_engine WinHTTP selection",
    )
    text = replace_once(
        text,
        '''    let exe = curl_impersonate_exe()?;
    let mut headers = headers_for_request(job, &job.url);''',
        '''    let exe = curl_impersonate_exe()?;
    if matches!(proxy_route(&job.proxy), ProxyRoute::System) {
        return None;
    }
    let mut headers = headers_for_request(job, &job.url);''',
        "http_engine curl system bypass",
    )
    text = replace_once(
        text,
        '''    if !job.proxy.trim().is_empty() {
        command.arg("-x").arg(&job.proxy);
    }''',
        '''    match proxy_route(&job.proxy) {
        ProxyRoute::Direct => {
            command.arg("--noproxy").arg("*");
        }
        ProxyRoute::System => unreachable!("system route skips curl-impersonate"),
        ProxyRoute::Manual(proxy) => {
            command.arg("-x").arg(proxy);
        }
    }''',
        "http_engine curl route",
    )
    text = replace_once(
        text,
        '''    let mut stream = if job.proxy.trim().is_empty() {
        TcpStream::connect((parsed.host.as_str(), parsed.port))
            .map_err(|err| EngineError::Failed(err.to_string()))?
    } else {
        return Err(EngineError::Failed("http proxy uses WinHTTP".into()));
    };''',
        '''    let mut stream = match proxy_route(&job.proxy) {
        ProxyRoute::Direct => TcpStream::connect((parsed.host.as_str(), parsed.port))
            .map_err(|err| EngineError::Failed(err.to_string()))?,
        ProxyRoute::System | ProxyRoute::Manual(_) => {
            return Err(EngineError::Failed("http proxy uses WinHTTP".into()));
        }
    };''',
        "http_engine raw HTTP route",
    )

    text = replace_once(
        text,
        '''    pub struct WinHttpBody {''',
        '''    // WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY (stable WinHTTP ABI value).
    const SYSTEM_PROXY_ACCESS_TYPE: u32 = 4;

    pub struct WinHttpBody {''',
        "http_engine system proxy constant",
    )

    session_pattern = r'''let session = if proxy\.is_empty\(\) \{\s*WinHttpOpen\(\s*wide\(super::CHROME_UA\)\.as_ptr\(\),\s*WINHTTP_ACCESS_TYPE_NO_PROXY,\s*null_mut\(\),\s*null_mut\(\),\s*0,\s*\)\s*\} else \{\s*WinHttpOpen\(\s*wide\(super::CHROME_UA\)\.as_ptr\(\),\s*WINHTTP_ACCESS_TYPE_NAMED_PROXY,\s*wide\(proxy\)\.as_ptr\(\),\s*wide\(""\)\.as_ptr\(\),\s*0,\s*\)\s*\};'''
    session_replacement = '''let session = match super::proxy_route(proxy) {
                super::ProxyRoute::Direct => WinHttpOpen(
                    wide(super::CHROME_UA).as_ptr(),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    null_mut(),
                    null_mut(),
                    0,
                ),
                super::ProxyRoute::System => WinHttpOpen(
                    wide(super::CHROME_UA).as_ptr(),
                    SYSTEM_PROXY_ACCESS_TYPE,
                    null_mut(),
                    null_mut(),
                    0,
                ),
                super::ProxyRoute::Manual(proxy_url) => WinHttpOpen(
                    wide(super::CHROME_UA).as_ptr(),
                    WINHTTP_ACCESS_TYPE_NAMED_PROXY,
                    wide(proxy_url).as_ptr(),
                    wide("").as_ptr(),
                    0,
                ),
            };'''
    text = sub_once(text, session_pattern, session_replacement, "http_engine WinHTTP session", re.S)
    write(path, text)


def write_docs() -> None:
    path = ROOT / "docs/architecture/proxy-routing.md"
    path.write_text(
        """# Proxy routing identity

The v7 core preserves routing intent instead of collapsing routes into an empty proxy string.

- `direct` -> `hls-downloader://direct-proxy`
- `system` -> `hls-downloader://system-proxy`
- `manual` -> the configured proxy URL
- bypass matches -> forced `direct`

Site rules are resolved before the global route. Explicit site `direct`, `system`, or `manual` routes override the global mode, while bypass remains the final safety override.

At the HTTP transport boundary, direct routes disable proxying, system routes use WinHTTP automatic proxy selection on Windows, and manual routes use the named proxy URL. `curl-impersonate` is skipped for system routing so it cannot silently bypass the OS proxy decision; direct curl requests explicitly disable environment proxying with `--noproxy *`.
""",
        encoding="utf-8",
    )


if __name__ == "__main__":
    patch_net_policy()
    patch_download_worker()
    patch_http_engine()
    write_docs()
    print("proxy route patch applied")
