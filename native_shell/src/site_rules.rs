//! Per-host download rules (concurrency, speed, proxy).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteRule {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub speed_limit_kib: u32,
    #[serde(default)]
    pub concurrency: u32,
    #[serde(default)]
    pub proxy: String,
    #[serde(default)]
    pub download_dir: String,
    #[serde(default)]
    pub user_agent: String,
    #[serde(default)]
    pub referer: String,
}

pub fn parse_site_rules(raw: &str) -> Vec<SiteRule> {
    let text = raw.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if let Ok(rules) = serde_json::from_str::<Vec<SiteRule>>(text) {
        return rules.into_iter().filter_map(sanitize_rule).collect();
    }
    text.lines()
        .filter_map(|line| parse_line(line))
        .collect()
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
        if rule.speed_limit_kib > 0 {
            existing.speed_limit_kib = rule.speed_limit_kib;
        }
        if rule.concurrency > 0 {
            existing.concurrency = rule.concurrency;
        }
        if !rule.proxy.is_empty() {
            existing.proxy = rule.proxy;
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
    rules.iter().find(|rule| {
        let needle = rule.host.trim().trim_start_matches('.').to_ascii_lowercase();
        host == needle || host.ends_with(&format!(".{needle}"))
    })
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
}
