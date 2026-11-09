use crate::SiteRuleRow;
use serde_json::{Map, Value};
use std::collections::HashSet;

// Keep unedited fields and vault references when converting the existing JSON
// or legacy line format to a form. This module never handles plaintext secrets.
pub struct SiteRuleDraft {
    pub row: SiteRuleRow,
    original: Map<String, Value>,
}

impl SiteRuleDraft {
    pub fn new() -> Self {
        Self::from_object(Map::new())
    }

    fn from_object(original: Map<String, Value>) -> Self {
        let text = |key: &str| {
            original
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into()
        };
        let number = |key: &str| {
            original
                .get(key)
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .to_string()
                .into()
        };
        Self {
            row: SiteRuleRow {
                host: text("host"),
                enabled: original
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                concurrency: number("concurrency"),
                speed: number("speed_limit_kib"),
                proxy_mode: text("proxy_mode"),
                proxy: text("proxy"),
                directory: text("download_dir"),
                user_agent: text("user_agent"),
                referer: text("referer"),
                origin: text("origin"),
                has_credential: original
                    .get("credential_ref")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty()),
            },
            original,
        }
    }

    pub fn parse(raw: &str) -> Result<Vec<Self>, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        let objects: Vec<Map<String, Value>> = if raw.starts_with('[') {
            serde_json::from_str(raw).map_err(|_| "站点规则格式无效，原配置已保留".to_owned())?
        } else {
            let mut objects = Vec::new();
            for line in raw
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
            {
                let (host, rest) = line
                    .split_once('=')
                    .ok_or("站点规则逐行格式无效，原配置已保留")?;
                let mut object = Map::from_iter([(
                    "host".into(),
                    Value::from(host.trim().to_ascii_lowercase()),
                )]);
                for part in rest.split(',') {
                    let (key, value) = part.split_once(':').unwrap_or((part, ""));
                    let key = match key.trim() {
                        "speed" | "kib" => "speed_limit_kib",
                        "conn" | "concurrency" => "concurrency",
                        "dir" | "download_dir" => "download_dir",
                        "ua" | "user_agent" => "user_agent",
                        "proxy" => "proxy",
                        "proxy_mode" => "proxy_mode",
                        "referer" => "referer",
                        "origin" => "origin",
                        _ => continue,
                    };
                    let value = if matches!(key, "speed_limit_kib" | "concurrency") {
                        Value::from(value.trim().parse::<u32>().unwrap_or(0))
                    } else if key == "proxy_mode" {
                        Value::from(value.trim().to_ascii_lowercase())
                    } else {
                        Value::from(value.trim())
                    };
                    object.insert(key.into(), value);
                }
                objects.push(object);
            }
            objects
        };
        Ok(objects.into_iter().map(Self::from_object).collect())
    }

    pub fn edit(&mut self, key: &str, value: &str) {
        match key {
            "host" => self.row.host = value.into(),
            "enabled" => self.row.enabled = value == "true",
            "concurrency" => self.row.concurrency = value.into(),
            "speed" => self.row.speed = value.into(),
            "proxy_mode" => self.row.proxy_mode = value.into(),
            "proxy" => self.row.proxy = value.into(),
            "directory" => self.row.directory = value.into(),
            "user_agent" => self.row.user_agent = value.into(),
            "referer" => self.row.referer = value.into(),
            "origin" => self.row.origin = value.into(),
            _ => {}
        }
    }
}

pub fn encode(rules: &[SiteRuleDraft], latest: &str) -> Result<String, String> {
    let latest = SiteRuleDraft::parse(latest)?;
    let mut hosts = HashSet::new();
    let mut objects = Vec::new();
    for (index, draft) in rules.iter().enumerate() {
        let row = &draft.row;
        let host = row.host.trim().to_ascii_lowercase();
        if host.is_empty() {
            return Err(format!("第 {} 条规则请填写域名", index + 1));
        }
        let identity = host.trim_start_matches("*.").trim_matches('.').to_owned();
        if !hosts.insert(identity) {
            return Err("站点规则不能包含重复域名".into());
        }
        let number = |value: &str, maximum: u32, label: &str| {
            value
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| *n <= maximum)
                .ok_or_else(|| format!("第 {} 条规则的{label}应为 0 至 {maximum}", index + 1))
        };
        let mut object = draft.original.clone();
        object.insert("host".into(), host.clone().into());
        object.insert("enabled".into(), row.enabled.into());
        object.insert(
            "concurrency".into(),
            number(&row.concurrency, 128, "并发数")?.into(),
        );
        object.insert(
            "speed_limit_kib".into(),
            number(&row.speed, u32::MAX, "限速")?.into(),
        );
        for (key, value) in [
            ("proxy_mode", &row.proxy_mode),
            ("proxy", &row.proxy),
            ("download_dir", &row.directory),
            ("user_agent", &row.user_agent),
            ("referer", &row.referer),
            ("origin", &row.origin),
        ] {
            object.insert(key.into(), value.trim().into());
        }
        // Credentials may have changed in the dedicated window since this form
        // opened. Use the latest reference for the same domain; never transfer
        // a previous domain's credential when the domain is edited.
        let credential = latest
            .iter()
            .find(|rule| rule.row.host.trim().eq_ignore_ascii_case(&host))
            .and_then(|rule| rule.original.get("credential_ref"))
            .cloned()
            .unwrap_or_else(|| Value::from(""));
        object.insert("credential_ref".into(), credential);
        objects.push(object);
    }
    serde_json::to_string(&objects).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_form_roundtrip_keeps_all_download_and_request_fields() {
        let raw = "# rules\ncdn.test=speed:256,conn:2,proxy:http://localhost:8080,proxy_mode:MANUAL,dir:D:/media,ua:fixture,referer:https://cdn.test/page,origin:https://cdn.test";
        let rules = SiteRuleDraft::parse(raw).unwrap();
        let encoded: Value = serde_json::from_str(&encode(&rules, raw).unwrap()).unwrap();
        let row = &encoded[0];
        assert_eq!(row["concurrency"], 2);
        assert_eq!(row["speed_limit_kib"], 256);
        assert_eq!(row["proxy"], "http://localhost:8080");
        assert_eq!(row["proxy_mode"], "manual");
        assert_eq!(row["download_dir"], "D:/media");
        assert_eq!(row["user_agent"], "fixture");
        assert_eq!(row["referer"], "https://cdn.test/page");
        assert_eq!(row["origin"], "https://cdn.test");
    }

    #[test]
    fn credential_updates_survive_form_save_and_never_follow_domain_edits() {
        let raw = r#"[{"host":"cdn.test","credential_ref":"vault:old","extra":42}]"#;
        let mut rules = SiteRuleDraft::parse(raw).unwrap();
        let latest = r#"[{"host":"cdn.test","credential_ref":"vault:updated"}]"#;
        let encoded: Value = serde_json::from_str(&encode(&rules, latest).unwrap()).unwrap();
        assert_eq!(encoded[0]["credential_ref"], "vault:updated");
        assert_eq!(encoded[0]["extra"], 42);
        rules[0].edit("host", "other.test");
        let encoded: Value = serde_json::from_str(&encode(&rules, latest).unwrap()).unwrap();
        assert_eq!(encoded[0]["credential_ref"], "");
        let encoded: Value = serde_json::from_str(
            &encode(
                &SiteRuleDraft::parse(raw).unwrap(),
                r#"[{"host":"cdn.test"}]"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(encoded[0]["credential_ref"], "");
    }

    #[test]
    fn invalid_drafts_are_rejected_instead_of_saving_previous_numeric_values() {
        let mut rules = SiteRuleDraft::parse(r#"[{"host":"cdn.test"}]"#).unwrap();
        rules[0].edit("concurrency", "");
        assert!(encode(&rules, "").is_err());
        rules[0].edit("concurrency", "129");
        assert!(encode(&rules, "").is_err());
        rules[0].edit("concurrency", "2");
        rules.push(
            SiteRuleDraft::parse(r#"[{"host":"*.CDN.test"}]"#)
                .unwrap()
                .remove(0),
        );
        assert!(encode(&rules, "").is_err());
        assert!(SiteRuleDraft::parse("invalid rule").is_err());
    }
}
