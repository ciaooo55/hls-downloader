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
    text.lines().filter_map(|line| parse_line(line)).collect()
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
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

pub fn upsert_site_rule(rules: &mut Vec<SiteRule>, rule: SiteRule) {
    if rule.host.trim().is_empty() {
        return;
    }
    if let Some(existing) = rules.iter_mut().find(|item| item.host == rule.host) {
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
        let needle = rule
            .host
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        host == needle || host.ends_with(&format!(".{needle}"))
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
        parse_site_rules(text)
    };
    if rules.len() > 100 {
        return Err("站点规则不能超过 100 条".into());
    }
    let mut hosts = std::collections::HashSet::new();
    for rule in rules {
        let host = rule.host.trim().to_ascii_lowercase();
        if host.is_empty()
            || host.len() > 255
            || host.chars().any(char::is_whitespace)
            || host.contains(['/', '\\', ':', '@', '\0'])
        {
            return Err("站点规则包含无效域名".into());
        }
        if !hosts.insert(host) {
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
        assert_eq!(
            credential_ref_for_host("A.Test"),
            credential_ref_for_host("a.test")
        );
    }
}
