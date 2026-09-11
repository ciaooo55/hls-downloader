//! Per-host download rules. Secrets stay in the credential vault and are
//! referenced by `credential_ref`; the serialized rule list is UI-safe.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteRule {
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub speed_limit_kib: u32,
    #[serde(default)]
    pub concurrency: u32,
    #[serde(default)]
    pub proxy: String,
    #[serde(default)]
    pub proxy_mode: String,
    #[serde(default)]
    pub download_dir: String,
    #[serde(default)]
    pub user_agent: String,
    #[serde(default)]
    pub referer: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub credential_ref: String,
}

impl Default for SiteRule {
    fn default() -> Self {
        Self {
            host: String::new(),
            enabled: true,
            speed_limit_kib: 0,
            concurrency: 0,
            proxy: String::new(),
            proxy_mode: String::new(),
            download_dir: String::new(),
            user_agent: String::new(),
            referer: String::new(),
            origin: String::new(),
            credential_ref: String::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

pub fn parse_site_rules(raw: &str) -> Vec<SiteRule> {
    let text = raw.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if let Ok(rules) = serde_json::from_str::<Vec<SiteRule>>(text) {
        return rules.into_iter().filter_map(sanitize_rule).collect();
    }
    text.lines().filter_map(parse_line).collect()
}

fn sanitize_rule(mut rule: SiteRule) -> Option<SiteRule> {
    rule.host = rule.host.trim().to_ascii_lowercase();
    if rule.host.is_empty() || !setting_text_ok(&rule.host) {
        return None;
    }
    if !setting_text_ok(&rule.proxy) {
        rule.proxy.clear();
    }
    if !setting_text_ok(&rule.download_dir) {
        rule.download_dir.clear();
    }
    if !setting_text_ok(&rule.user_agent) {
        rule.user_agent.clear();
    }
    if !setting_text_ok(&rule.referer) {
        rule.referer.clear();
    }
    if !setting_text_ok(&rule.origin) {
        rule.origin.clear();
    }
    if !setting_text_ok(&rule.credential_ref) {
        rule.credential_ref.clear();
    }
    if !matches!(
        rule.proxy_mode.as_str(),
        "" | "direct" | "system" | "manual"
    ) {
        rule.proxy_mode.clear();
    }
    Some(rule)
}

fn setting_text_ok(value: &str) -> bool {
    !value.contains('\r') && !value.contains('\n') && !value.contains('\0')
}

fn site_rule_match_host(host: &str) -> &str {
    host.trim()
        .strip_prefix("*.")
        .unwrap_or(host.trim())
        .trim_start_matches('.')
}

fn valid_site_rule_host(host: &str) -> bool {
    let wildcard = host.starts_with("*.");
    let host = host.strip_prefix("*.").unwrap_or(host);
    if host.is_empty()
        || host.len() > 255
        || host.chars().any(char::is_whitespace)
        || host.contains(['/', '\\', '@', '*', '\0'])
    {
        return false;
    }
    if let Some(address) = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        return !wildcard && address.parse::<std::net::Ipv6Addr>().is_ok();
    }
    !host.contains([':', '[', ']'])
}

fn parse_line(line: &str) -> Option<SiteRule> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (host, rest) = line.split_once('=')?;
    let mut rule = SiteRule {
        host: host.trim().to_ascii_lowercase(),
        enabled: true,
        ..SiteRule::default()
    };
    for part in rest.split(',') {
        let (key, value) = part.split_once(':').unwrap_or((part, ""));
        match key.trim() {
            "speed" | "kib" => rule.speed_limit_kib = value.trim().parse().unwrap_or(0),
            "conn" | "concurrency" => rule.concurrency = value.trim().parse().unwrap_or(0),
            "proxy" => {
                let proxy = value.trim().to_string();
                if setting_text_ok(&proxy) {
                    rule.proxy = proxy;
                }
            }
            "proxy_mode" => rule.proxy_mode = value.trim().to_ascii_lowercase(),
            "dir" | "download_dir" => {
                let dir = value.trim().to_string();
                if setting_text_ok(&dir) {
                    rule.download_dir = dir;
                }
            }
            "ua" | "user_agent" => {
                let ua = value.trim().to_string();
                if setting_text_ok(&ua) {
                    rule.user_agent = ua;
                }
            }
            "referer" => {
                let referer = value.trim().to_string();
                if setting_text_ok(&referer) {
                    rule.referer = referer;
                }
            }
            "origin" => {
                let origin = value.trim().to_string();
                if setting_text_ok(&origin) {
                    rule.origin = origin;
                }
            }
            _ => {}
        }
    }
    (!rule.host.is_empty() && setting_text_ok(&rule.host)).then_some(rule)
}

pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, tail)| tail).unwrap_or(url);
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim();
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed
            .split_once(']')
            .map(|(address, _)| format!("[{address}]"))
            .unwrap_or_default()
    } else {
        authority
            .split_once(':')
            .map(|(host, _)| host)
            .unwrap_or(authority)
            .to_string()
    };
    host.trim().to_ascii_lowercase()
}

pub fn upsert_site_rule(rules: &mut Vec<SiteRule>, rule: SiteRule) {
    let rule_host = site_rule_match_host(&rule.host).to_ascii_lowercase();
    if rule_host.is_empty() {
        return;
    }
    if let Some(existing) = rules
        .iter_mut()
        .find(|item| site_rule_match_host(&item.host).eq_ignore_ascii_case(&rule_host))
    {
        existing.enabled = rule.enabled;
        if rule.speed_limit_kib > 0 {
            existing.speed_limit_kib = rule.speed_limit_kib;
        }
        if rule.concurrency > 0 {
            existing.concurrency = rule.concurrency;
        }
        if !rule.proxy.is_empty() {
            existing.proxy = rule.proxy;
        }
        if !rule.proxy_mode.is_empty() {
            existing.proxy_mode = rule.proxy_mode;
        }
        if !rule.download_dir.is_empty() {
            existing.download_dir = rule.download_dir;
        }
        if !rule.user_agent.is_empty() {
            existing.user_agent = rule.user_agent;
        }
        if !rule.referer.is_empty() {
            existing.referer = rule.referer;
        }
        if !rule.origin.is_empty() {
            existing.origin = rule.origin;
        }
        if !rule.credential_ref.is_empty() {
            existing.credential_ref = rule.credential_ref;
        }
        return;
    }
    rules.insert(0, rule);
}

pub fn format_site_rules(rules: &[SiteRule]) -> String {
    if let Ok(json) = serde_json::to_string_pretty(rules) {
        return json;
    }
    rules
        .iter()
        .map(|rule| {
            format!(
                "{}=speed:{},conn:{},proxy:{}",
                rule.host, rule.speed_limit_kib, rule.concurrency, rule.proxy
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn matching_rule<'a>(rules: &'a [SiteRule], url: &str) -> Option<&'a SiteRule> {
    let host = host_of(url);
    rules.iter().filter(|rule| rule.enabled).find(|rule| {
        let needle = site_rule_match_host(&rule.host).to_ascii_lowercase();
        !needle.is_empty() && (host == needle || host.ends_with(&format!(".{needle}")))
    })
}

pub fn credential_ref_for_host(host: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in host.trim().to_ascii_lowercase().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("settings:site-rule:{hash:016x}")
}

pub fn validate_site_rules(raw: &str) -> Result<(), String> {
    if raw.len() > 256 * 1024 {
        return Err("站点规则总大小不能超过 256 KiB".into());
    }
    let text = raw.trim();
    if text.is_empty() {
        return Ok(());
    }
    let rules = if text.starts_with('[') {
        serde_json::from_str::<Vec<SiteRule>>(text)
            .map_err(|error| format!("站点规则 JSON 无效: {error}"))?
    } else {
        let mut rules = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let rule = parse_line(line).ok_or_else(|| format!("站点规则文本格式无效: {line}"))?;
            rules.push(rule);
        }
        rules
    };
    if rules.len() > 100 {
        return Err("站点规则不能超过 100 条".into());
    }
    let mut hosts = std::collections::HashSet::new();
    for rule in rules {
        let host = rule.host.trim().to_ascii_lowercase();
        if !valid_site_rule_host(&host) {
            return Err("站点规则包含无效域名".into());
        }
        let match_host = site_rule_match_host(&host).to_string();
        if !hosts.insert(match_host) {
            return Err("站点规则不能包含重复域名".into());
        }
        if rule.concurrency > 128 {
            return Err("站点规则并发数不能超过 128".into());
        }
        if !matches!(
            rule.proxy_mode.as_str(),
            "" | "direct" | "system" | "manual"
        ) {
            return Err("站点规则代理模式无效".into());
        }
        for (label, value, limit) in [
            ("代理地址", rule.proxy.as_str(), 2048usize),
            ("下载目录", rule.download_dir.as_str(), 32767usize),
            ("User-Agent", rule.user_agent.as_str(), 2048usize),
            ("Referer", rule.referer.as_str(), 4096usize),
            ("Origin", rule.origin.as_str(), 1024usize),
            ("凭据引用", rule.credential_ref.as_str(), 255usize),
        ] {
            if value.len() > limit || !setting_text_ok(value) {
                return Err(format!("站点规则的{label}无效"));
            }
        }
        if !rule.origin.trim().is_empty()
            && !(rule.origin.starts_with("http://") || rule.origin.starts_with("https://"))
        {
            return Err("站点规则 Origin 必须是 HTTP(S) 地址".into());
        }
        if rule.proxy_mode == "manual"
            && !rule.proxy.trim().is_empty()
            && !matches!(
                rule.proxy
                    .split_once("://")
                    .map(|(scheme, _)| scheme.to_ascii_lowercase())
                    .as_deref(),
                Some("http" | "https" | "socks5" | "socks5h")
            )
        {
            return Err("站点规则代理地址无效".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_suffix_host_and_line_syntax() {
        let rules = parse_site_rules("cdn.example.test=speed:256,conn:2\n# comment\n");
        let rule = matching_rule(&rules, "https://a.cdn.example.test/file.bin").unwrap();
        assert_eq!(rule.speed_limit_kib, 256);
        assert_eq!(rule.concurrency, 2);
        assert!(matching_rule(&rules, "https://other.test/a").is_none());
    }

    #[test]
    fn wildcard_host_is_a_compatible_suffix_alias() {
        let rules = parse_site_rules("*.example.test=speed:128,conn:2");
        let rule = matching_rule(&rules, "https://cdn.example.test/file.bin").unwrap();
        assert_eq!(rule.speed_limit_kib, 128);
        assert!(matching_rule(&rules, "https://example.test/file.bin").is_some());
        assert!(validate_site_rules("*.example.test=speed:128").is_ok());
        assert!(validate_site_rules("foo*bar.example.test=speed:128").is_err());
        assert!(
            validate_site_rules(r#"[{"host":"example.test"},{"host":"*.example.test"}]"#).is_err()
        );
    }

    #[test]
    fn upsert_reuses_equivalent_wildcard_host() {
        let mut rules = parse_site_rules("*.example.test=speed:128,conn:2");
        upsert_site_rule(
            &mut rules,
            SiteRule {
                host: "example.test".into(),
                concurrency: 4,
                ..SiteRule::default()
            },
        );
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].host, "*.example.test");
        assert_eq!(rules[0].concurrency, 4);
    }

    #[test]
    fn extracts_hostname_without_userinfo_or_port() {
        assert_eq!(
            host_of("https://User@Video.Example.Test:8443/watch"),
            "video.example.test"
        );
        assert_eq!(host_of("http://[2001:DB8::1]:8080/file"), "[2001:db8::1]");
        assert_eq!(host_of("https://[::1]/"), "[::1]");
    }

    #[test]
    fn supports_bracketed_ipv6_site_rules() {
        assert!(validate_site_rules(r#"[{"host":"[2001:db8::1]"}]"#).is_ok());
        assert!(validate_site_rules(r#"[{"host":"example.test:443"}]"#).is_err());
        assert!(validate_site_rules(r#"[{"host":"[not-ipv6]"}]"#).is_err());
        assert!(validate_site_rules(r#"[{"host":"[2001:db8::1"}]"#).is_err());
        let rules = parse_site_rules(r#"[{"host":"[2001:db8::1]","speed_limit_kib":64}]"#);
        let rule = matching_rule(&rules, "https://[2001:db8::1]:8443/file.bin").unwrap();
        assert_eq!(rule.speed_limit_kib, 64);
    }

    #[test]
    fn parses_json_array() {
        let rules = parse_site_rules(r#"[{"host":"files.test","proxy":"http://127.0.0.1:8080"}]"#);
        assert_eq!(rules[0].proxy, "http://127.0.0.1:8080");
        let rich = parse_site_rules(
            r#"[{"host":"cdn.test","user_agent":"UA/1","referer":"https://site.test/","download_dir":"D:\\Videos"}]"#,
        );
        assert_eq!(rich[0].user_agent, "UA/1");
        assert_eq!(rich[0].referer, "https://site.test/");
        assert_eq!(rich[0].download_dir, "D:\\Videos");
        let poisoned = parse_site_rules(
            r#"[{"host":"files.test","proxy":"http://127.0.0.1:8080\u000d\u000aX:1"}]"#,
        );
        assert_eq!(poisoned[0].host, "files.test");
        assert_eq!(poisoned[0].proxy, "");
    }

    #[test]
    fn disabled_rules_are_skipped_and_rich_fields_roundtrip() {
        let rules = parse_site_rules(
            r#"[{"host":"disabled.test","enabled":false},{"host":"cdn.test","origin":"https://site.test","proxy_mode":"direct","credential_ref":"settings:site-rule:1"}]"#,
        );
        assert!(matching_rule(&rules, "https://disabled.test/file").is_none());
        let active = matching_rule(&rules, "https://cdn.test/file").unwrap();
        assert_eq!(active.origin, "https://site.test");
        assert_eq!(active.proxy_mode, "direct");
        assert!(format_site_rules(&rules).contains("credential_ref"));
    }

    #[test]
    fn validates_duplicate_and_malformed_rules() {
        assert!(validate_site_rules(r#"[{"host":"a.test"},{"host":"a.test"}]"#).is_err());
        assert!(validate_site_rules(r#"[{"host":"bad/path"}]"#).is_err());
        assert!(validate_site_rules(r#"[{"host":"a.test","origin":"javascript:bad"}]"#).is_err());
        assert!(validate_site_rules(r#"[{"host":"a.test","proxy_mode":"direct"}]"#).is_ok());
        assert!(validate_site_rules("not-a-rule").is_err());
        assert!(validate_site_rules("# comment only\n").is_ok());
        assert!(validate_site_rules("cdn.test=speed:256,conn:2").is_ok());
        assert_eq!(
            credential_ref_for_host("A.Test"),
            credential_ref_for_host("a.test")
        );
    }
}
