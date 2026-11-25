//! Transfer and proxy passwords share the request-header DPAPI vault.
use crate::CoreStore;
use hls_client::url_credentials::{map_url_passwords, proxy_scheme};
use serde::{de::DeserializeOwned, Serialize};

fn url_password_key(authority: &str) -> String {
    let identity = authority.split_once("://").and_then(|(scheme, rest)| {
        let scheme = scheme.to_ascii_lowercase();
        let url = format!("{scheme}://{rest}/credential");
        match scheme.as_str() {
            "ftp" | "ftps" => crate::ftp_engine::parse_ftp_url(&url)
                .ok()
                .map(|target| (scheme, target.host, target.port, target.user)),
            "sftp" => crate::sftp_engine::parse_sftp_url(&url)
                .ok()
                .map(|target| (scheme, target.host, target.port, target.user)),
            _ if proxy_scheme(&scheme) => {
                let (user, endpoint) = rest.rsplit_once('@')?;
                let endpoint = endpoint
                    .trim_end()
                    .parse::<tokio_tungstenite::tungstenite::http::uri::Authority>()
                    .ok()?;
                let host = endpoint.host().to_ascii_lowercase();
                let host = host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .map_or(host, |ip| ip.to_string());
                // Match the effective destination chosen by libcurl.
                let port = match endpoint.as_str().strip_prefix(endpoint.host())? {
                    "" if scheme == "https" => 443,
                    "" => 1080,
                    suffix => suffix.strip_prefix(':')?.parse::<u16>().ok()?,
                };
                Some((scheme, host, port, decode_proxy_userinfo(user)?))
            }
            _ => None,
        }
    });
    match identity {
        Some(identity) => format!("url-password:{}", serde_json::json!(identity)),
        None => format!("url-password:{authority}"),
    }
}

pub(crate) fn decode_proxy_userinfo(user: &str) -> Option<String> {
    let mut bytes = user.bytes();
    let mut decoded = Vec::with_capacity(user.len());
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            (high * 16 + low) as u8
        } else {
            byte
        });
    }
    String::from_utf8(decoded).ok()
}

pub(super) fn proxy_password_key(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let (userinfo, endpoint) = authority.rsplit_once('@')?;
    let user = userinfo.split(':').next()?;
    proxy_scheme(&scheme.to_ascii_lowercase())
        .then(|| url_password_key(&format!("{scheme}://{user}@{endpoint}")))
}

pub(crate) fn redact_urls<T: Serialize + DeserializeOwned>(value: T) -> Result<T, String> {
    collect_url_passwords(value).map(|(clean, _)| clean)
}

impl CoreStore {
    fn protect_url_password(&self, authority: &str, password: &str) -> Result<(), String> {
        let key = url_password_key(authority);
        #[cfg(windows)]
        {
            let encoded = serde_json::to_string(password).map_err(|e| e.to_string())?;
            let blob = crate::credentials::CredentialVault.protect(&encoded)?;
            self.connection.execute(
                "INSERT INTO credentials(credential_ref, protected_blob, kind, created_at_ms) VALUES (?1, ?2, 'url_password', 0) ON CONFLICT(credential_ref) DO UPDATE SET protected_blob=excluded.protected_blob",
                rusqlite::params![key, blob],
            ).map_err(|e| e.to_string())?;
        }
        #[cfg(not(windows))]
        self.connection.execute(
            "INSERT OR REPLACE INTO volatile_vault.url_passwords(credential_ref,password) VALUES (?1,?2)",
            rusqlite::params![key, password],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub(crate) fn sanitize_urls<T: Serialize + DeserializeOwned>(
        &self,
        value: T,
    ) -> Result<T, String> {
        let (clean, passwords) = collect_url_passwords(value)?;
        if passwords.is_empty() {
            return Ok(clean);
        }
        self.store_sanitized(clean, passwords, |clean, _| Ok(clean))
    }

    pub(super) fn write_sanitized<T: Serialize + DeserializeOwned, R>(
        &self,
        value: T,
        write: impl FnOnce(T, &rusqlite::Transaction<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        let (clean, passwords) = collect_url_passwords(value)?;
        self.store_sanitized(clean, passwords, write)
    }

    fn store_sanitized<T, R>(
        &self,
        clean: T,
        passwords: Vec<(String, String)>,
        write: impl FnOnce(T, &rusqlite::Transaction<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        for (authority, password) in &passwords {
            self.protect_url_password(authority, password)?;
        }
        let result = write(clean, &transaction)?;
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }

    pub(crate) fn hydrate_url(&self, url: &str) -> Result<String, String> {
        self.hydrate_url_field(url, false)
    }

    pub(crate) fn hydrate_proxy(&self, url: &str) -> Result<String, String> {
        self.hydrate_url_field(url.trim(), true)
    }

    fn hydrate_url_field(&self, url: &str, proxy: bool) -> Result<String, String> {
        let Some((scheme, rest)) = url.split_once("://") else {
            return Ok(url.into());
        };
        let lower = scheme.to_ascii_lowercase();
        let supported = if proxy {
            proxy_scheme(&lower)
        } else {
            matches!(lower.as_str(), "ftp" | "ftps" | "sftp")
        };
        if !supported {
            return Ok(url.into());
        }
        let authority = rest
            .split(|c| c == '/' || (proxy && matches!(c, '?' | '#')))
            .next()
            .unwrap_or(rest);
        let Some(at) = authority.rfind('@') else {
            return Ok(url.into());
        };
        if authority[..at].contains(':') {
            return Ok(url.into());
        }
        let key = url_password_key(&format!("{scheme}://{authority}"));
        #[cfg(windows)]
        let password: Option<String> = self
            .load_credential(&key)?
            .map(|blob| {
                let plain = crate::credentials::CredentialVault.unprotect(&blob)?;
                serde_json::from_str(&plain).map_err(|_| "Invalid URL credential".to_string())
            })
            .transpose()?;
        #[cfg(not(windows))]
        let password: Option<String> = {
            use rusqlite::OptionalExtension;
            self.connection
                .query_row(
                    "SELECT password FROM volatile_vault.url_passwords WHERE credential_ref=?1",
                    [&key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?
        };
        let Some(password) = password else {
            return Ok(url.into());
        };
        let position = scheme.len() + 3 + at;
        Ok(format!(
            "{}:{}{}",
            &url[..position],
            password,
            &url[position..]
        ))
    }

    pub(crate) fn migrate_url_passwords(&self) -> Result<(), String> {
        self.connection
            .execute_batch("PRAGMA secure_delete=ON")
            .map_err(|e| e.to_string())?;
        self.connection
            .execute_batch("SAVEPOINT url_password_migration")
            .map_err(|e| e.to_string())?;
        let mut changed = false;
        let result = (|| {
            for (table, column) in [
                ("tasks", "snapshot_json"),
                ("task_specs", "spec_json"),
                ("handoffs", "public_json"),
                ("logs", "message"),
                ("settings", "value_json"),
                ("media_tracks", "track_json"),
                ("crawl_projects", "state_json"),
            ] {
                let proxy_field = if table == "settings" {
                    "key IN ('proxy_url', 'proxy')"
                } else {
                    "0"
                };
                let mut statement = self
                    .connection
                    .prepare(&format!("SELECT rowid, {column}, {proxy_field} FROM {table} WHERE rowid >= ?1 ORDER BY rowid LIMIT 1"))
                    .map_err(|e| e.to_string())?;
                let mut next = i64::MIN;
                loop {
                    use rusqlite::OptionalExtension;
                    let row = statement
                        .query_row([next], |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, bool>(2)?,
                            ))
                        })
                        .optional()
                        .map_err(|e| e.to_string())?;
                    let Some((id, text, proxy_field)) = row else {
                        break;
                    };
                    let decoded = if table == "logs" {
                        None
                    } else {
                        serde_json::from_str(&text).ok()
                    };
                    let is_json = decoded.is_some();
                    let mut value =
                        decoded.unwrap_or_else(|| serde_json::Value::String(text.clone()));
                    let before = value.clone();
                    if table == "logs" {
                        redact_diagnostic(&mut value)?;
                    } else {
                        sanitize_url_value(&mut value, proxy_field, &mut |authority, password| {
                            self.protect_url_password(authority, password)
                        })?;
                    }
                    if value != before {
                        let sanitized = if is_json {
                            value.to_string()
                        } else {
                            value.as_str().unwrap().to_owned()
                        };
                        changed = true;
                        self.connection
                            .execute(
                                &format!("UPDATE {table} SET {column}=?1 WHERE rowid=?2"),
                                rusqlite::params![sanitized, id],
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    let Some(following) = id.checked_add(1) else {
                        break;
                    };
                    next = following;
                }
            }
            self.connection
                .execute(
                    "INSERT INTO settings(key, value_json) VALUES ('url_passwords_migrated_v1', '\"1\"')\
                     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                    [],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if result.is_ok() {
            self.connection
                .execute_batch("RELEASE url_password_migration")
                .map_err(|e| e.to_string())?;
            if changed {
                self.connection
                    .execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE)")
                    .map_err(|e| e.to_string())?;
            }
        } else {
            let _ = self.connection.execute_batch(
                "ROLLBACK TO url_password_migration; RELEASE url_password_migration",
            );
        }
        result
    }
}

type UrlPasswords = Vec<(String, String)>;

fn collect_url_passwords<T: Serialize + DeserializeOwned>(
    value: T,
) -> Result<(T, UrlPasswords), String> {
    let serialized = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    if !serialized.contains('@') {
        return Ok((value, Vec::new()));
    }
    let mut value: serde_json::Value =
        serde_json::from_str(&serialized).map_err(|e| e.to_string())?;
    let mut passwords = Vec::new();
    sanitize_url_value(&mut value, false, &mut |authority, password| {
        passwords.push((authority.into(), password.into()));
        Ok(())
    })?;
    Ok((
        serde_json::from_value(value).map_err(|e| e.to_string())?,
        passwords,
    ))
}

// Diagnostics have no authority to update a proxy account's password.
fn redact_diagnostic(value: &mut serde_json::Value) -> Result<(), String> {
    match value {
        serde_json::Value::String(text) => *text = map_url_passwords(text, true, |_, _| Ok(()))?,
        serde_json::Value::Array(lines) => {
            for line in lines {
                redact_diagnostic(line)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn sanitize_field(
    key: &str,
    value: &mut serde_json::Value,
    protect: &mut impl FnMut(&str, &str) -> Result<(), String>,
) -> Result<(), String> {
    if matches!(key, "message" | "error_message" | "log_tail" | "lines") {
        redact_diagnostic(value)?;
    }
    sanitize_url_value(value, matches!(key, "proxy" | "proxy_url"), protect)
}

fn sanitize_url_value(
    value: &mut serde_json::Value,
    proxy_field: bool,
    protect: &mut impl FnMut(&str, &str) -> Result<(), String>,
) -> Result<(), String> {
    match value {
        serde_json::Value::String(text) => {
            // Some IPC records contain another serialized JSON document.
            if let Ok(mut nested) = serde_json::from_str::<serde_json::Value>(text) {
                if nested.is_object() || nested.is_array() {
                    let before = nested.clone();
                    sanitize_url_value(&mut nested, proxy_field, protect)?;
                    if nested != before {
                        *text = nested.to_string();
                    }
                    return Ok(());
                }
            }
            *text = map_url_passwords(text, proxy_field, protect)?;
        }
        serde_json::Value::Array(items) => {
            for item in items {
                sanitize_url_value(item, proxy_field, protect)?;
            }
        }
        serde_json::Value::Object(items) if !items.keys().any(|key| key.contains("://")) => {
            for (key, item) in items {
                sanitize_field(key, item, protect)?;
            }
        }
        serde_json::Value::Object(items) => {
            // URL-indexed maps need the same redaction as ordinary field values.
            for (key, mut item) in std::mem::take(items) {
                sanitize_field(&key, &mut item, protect)?;
                let key = if key.contains("://") {
                    map_url_passwords(&key, false, &mut *protect)?
                } else {
                    key
                };
                if items.insert(key, item).is_some() {
                    return Err("URL 凭据脱敏后出现重复键".into());
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, PersistentCore, ResourceKind, TaskSpec};

    #[test]
    fn url_password_admission_events_and_resume_roundtrip() {
        for (scheme, kind) in [
            ("ftp", ResourceKind::Ftp),
            ("ftps", ResourceKind::Ftp),
            ("sftp", ResourceKind::Sftp),
        ] {
            let mut core = PersistentCore::in_memory().unwrap();
            let url = format!("{scheme}://alice:p%40ss@host/file.bin");
            let events = core
                .handle(CoreCommand::CreateTask {
                    spec: TaskSpec {
                        url: url.clone(),
                        resource_kind: kind,
                        filename: "file.bin".into(),
                        start_paused: true,
                        ..Default::default()
                    },
                })
                .unwrap();
            assert!(!serde_json::to_string(&events).unwrap().contains("p%40ss"));
            let specs = core.store().load_task_specs().unwrap();
            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].1.url, format!("{scheme}://alice@host/file.bin"));
            assert_eq!(core.store().hydrate_url(&specs[0].1.url).unwrap(), url);
            assert!(!format!("{:?}", core.tasks()).contains("p%40ss"));
            assert!(!serde_json::to_string(&core.store().load_tasks().unwrap())
                .unwrap()
                .contains("p%40ss"));
        }
    }

    #[test]
    fn proxy_settings_keep_shared_credentials_and_rollback_rejected_changes() {
        let mut store = CoreStore::in_memory().unwrap();
        let clean = "http://alice@proxy.example:8080";
        let first = "http://alice:accepted@proxy.example:8080";
        store.set_setting("proxy_url", first).unwrap();
        store.set_setting("proxy_url", clean).unwrap();
        assert_eq!(store.hydrate_proxy(clean).unwrap(), first);
        store.connection.execute_batch("CREATE TRIGGER reject_proxy BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(store
            .set_setting("proxy_url", "http://alice:wrong@proxy.example:8080")
            .is_err());
        assert_eq!(store.hydrate_proxy(clean).unwrap(), first);
        store
            .connection
            .execute_batch("DROP TRIGGER reject_proxy")
            .unwrap();
        store
            .set_setting(
                "site_rules",
                serde_json::json!([{"host":"files.example","proxy": clean}]).to_string(),
            )
            .unwrap();
        store.set_setting("proxy_url", "").unwrap();
        assert_eq!(store.hydrate_proxy(clean).unwrap(), first);
        store.set_setting("site_rules", "[]").unwrap();
        assert_eq!(store.hydrate_proxy(clean).unwrap(), clean);
    }

    #[test]
    fn task_proxy_vaulting_preserves_download_userinfo_and_rejected_commands() {
        let mut core = PersistentCore::in_memory().unwrap();
        let spec = TaskSpec {
            url: "https://alice:download-secret@example.com/file.mp4".into(),
            proxy: "https://alice:proxy-secret@PROXY.example:443".into(),
            filename: "file.mp4".into(),
            start_paused: true,
            ..Default::default()
        };
        let events = core
            .handle(CoreCommand::CreateTask { spec: spec.clone() })
            .unwrap();
        assert!(!serde_json::to_string(&events)
            .unwrap()
            .contains("proxy-secret"));
        let stored = core.store().load_task_specs().unwrap().remove(0).1;
        assert_eq!(stored.url, spec.url);
        assert_eq!(stored.proxy, "https://alice@PROXY.example:443");
        let rejected = TaskSpec {
            filename: "a".repeat(256),
            proxy: "https://alice:wrong@proxy.example:443".into(),
            ..spec
        };
        assert!(core
            .handle(CoreCommand::CreateTask { spec: rejected })
            .is_err());
        assert_eq!(
            core.store()
                .hydrate_proxy("https://alice@proxy.example:443")
                .unwrap(),
            "https://alice:proxy-secret@proxy.example:443"
        );
        assert_eq!(
            core.store()
                .hydrate_url("https://alice@proxy.example:443")
                .unwrap(),
            "https://alice@proxy.example:443"
        );
    }

    #[test]
    fn url_password_migration_rewrites_legacy_json_and_logs() {
        let store = CoreStore::in_memory().unwrap();
        let url = "sftp://alice:old%3Asecret@host/file.bin";
        let spec = TaskSpec {
            url: url.into(),
            resource_kind: ResourceKind::Sftp,
            ..Default::default()
        };
        store
            .connection
            .execute(
                "INSERT INTO task_specs(task_id,spec_json) VALUES ('legacy',?1)",
                [serde_json::to_string(&spec).unwrap()],
            )
            .unwrap();
        store
            .connection
            .execute(
                "INSERT INTO settings(key,value_json) VALUES ('legacy',?1)",
                [serde_json::json!({"url":url}).to_string()],
            )
            .unwrap();
        store.connection.execute("INSERT INTO logs(task_id,level,code,message,created_at_ms) VALUES ('legacy','info','legacy',?1,0)", [format!("Downloading {url}")]).unwrap();
        store.migrate_url_passwords().unwrap();
        let clean = store.load_task_specs().unwrap().remove(0).1.url;
        assert_eq!(clean, "sftp://alice@host/file.bin");
        assert_eq!(store.hydrate_url(&clean).unwrap(), url);
        assert!(!store
            .setting_string("legacy", "")
            .unwrap()
            .contains("old%3Asecret"));
        assert_eq!(
            store.load_task_log("legacy", 10).unwrap(),
            ["Downloading sftp://alice@host/file.bin"]
        );
        store.migrate_url_passwords().unwrap();
        assert_eq!(store.hydrate_url(&clean).unwrap(), url);
    }

    #[test]
    fn url_password_engine_accepted_userinfo_roundtrips() {
        let store = CoreStore::in_memory().unwrap();
        for scheme in ["ftp", "ftps", "sftp"] {
            for secret in [
                "pa'ss", "pa\"ss", "pa ss", "pa?ss", "pa#ss", "密碼", "pa@ss",
            ] {
                let url = format!("{scheme}://alice:{secret}@host/file.bin");
                if scheme == "sftp" {
                    assert_eq!(
                        crate::sftp_engine::parse_sftp_url(&url).unwrap().password,
                        secret
                    );
                } else {
                    assert_eq!(
                        crate::ftp_engine::parse_ftp_url(&url).unwrap().password,
                        secret
                    );
                }
                let clean = store.sanitize_urls(url.clone()).unwrap();
                assert_eq!(clean, format!("{scheme}://alice@host/file.bin"));
                assert_eq!(store.hydrate_url(&clean).unwrap(), url);
            }
        }
    }

    #[test]
    fn url_password_migration_accepts_multibyte_text_before_urls() {
        let store = CoreStore::in_memory().unwrap();
        store.connection.execute("INSERT INTO settings(key,value_json) VALUES ('source',?1)", [serde_json::json!({"source":"来源：https://host/file", "url":"来源：ftp://alice:secret@host/file"}).to_string()]).unwrap();
        store.migrate_url_passwords().unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&store.setting_string("source", "").unwrap()).unwrap();
        assert_eq!(value["source"], "来源：https://host/file");
        assert_eq!(value["url"], "来源：ftp://alice@host/file");
        assert_eq!(
            store.hydrate_url("ftp://alice@host/file").unwrap(),
            "ftp://alice:secret@host/file"
        );
    }

    #[test]
    fn url_password_whole_url_with_spaces_in_username_roundtrips() {
        let store = CoreStore::in_memory().unwrap();
        let url = "ftp://first last:secret@host/file";
        assert_eq!(
            crate::ftp_engine::parse_ftp_url(url).unwrap().user,
            "first last"
        );
        let clean = store.sanitize_urls(url.to_string()).unwrap();
        assert_eq!(clean, "ftp://first last@host/file");
        assert_eq!(store.hydrate_url(&clean).unwrap(), url);
    }

    #[test]
    fn url_password_keys_follow_engine_host_and_user_normalization() {
        let store = CoreStore::in_memory().unwrap();
        for (url, equivalent) in [
            ("sftp://alice:secret@host /file", "sftp://alice@host/file"),
            (
                "sftp://a%6Cice:secret@HOST.:22/file",
                "sftp://alice@host/file",
            ),
            ("ftp://a%6Cice:secret@host:21/file", "ftp://alice@host/file"),
        ] {
            let clean = store.sanitize_urls(url.to_string()).unwrap();
            assert!(!clean.contains("secret"));
            assert_eq!(store.hydrate_url(&clean).unwrap(), url);
            let hydrated = store.hydrate_url(equivalent).unwrap();
            if url.starts_with("sftp") {
                let expected = crate::sftp_engine::parse_sftp_url(url).unwrap();
                let actual = crate::sftp_engine::parse_sftp_url(&hydrated).unwrap();
                assert_eq!(
                    (actual.host, actual.port, actual.user, actual.password),
                    (
                        expected.host,
                        expected.port,
                        expected.user,
                        expected.password
                    )
                );
            } else {
                let expected = crate::ftp_engine::parse_ftp_url(url).unwrap();
                let actual = crate::ftp_engine::parse_ftp_url(&hydrated).unwrap();
                assert_eq!(
                    (actual.host, actual.port, actual.user, actual.password),
                    (
                        expected.host,
                        expected.port,
                        expected.user,
                        expected.password
                    )
                );
            }
        }
    }

    #[test]
    fn url_password_empty_absent_and_corrupt_credentials() {
        let store = CoreStore::in_memory().unwrap();
        for url in ["", "not a URL", "ftp://host/file", "sftp://alice@host/file"] {
            assert_eq!(store.hydrate_url(url).unwrap(), url);
            assert_eq!(store.sanitize_urls(url.to_string()).unwrap(), url);
        }
        let clean = store
            .sanitize_urls("ftp://alice:@host/file".to_string())
            .unwrap();
        assert_eq!(clean, "ftp://alice@host/file");
        assert_eq!(store.hydrate_url(&clean).unwrap(), "ftp://alice:@host/file");
        let supplied = "ftp://alice:explicit%40pass@host/path@file?mail=a@b";
        assert_eq!(store.hydrate_url(supplied).unwrap(), supplied);
        #[cfg(windows)]
        {
            store
                .connection
                .execute("UPDATE credentials SET protected_blob='dpapi:invalid'", [])
                .unwrap();
            assert!(store.hydrate_url(&clean).is_err());
        }
    }

    #[test]
    fn failed_setting_write_preserves_prior_password_and_redacts_url_map_keys() {
        let mut store = CoreStore::in_memory().unwrap();
        let first = serde_json::json!({"ftp://user:first@host/file":true});
        store.set_setting("urls", first).unwrap();
        let stored = store
            .connection
            .query_row(
                "SELECT value_json FROM settings WHERE key='urls'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(stored, r#"{"ftp://user@host/file":true}"#);
        store.connection.execute_batch("CREATE TRIGGER reject_update BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(store
            .set_setting("urls", "ftp://user:second@host/file")
            .is_err());
        assert_eq!(
            store.hydrate_url("ftp://user@host/file").unwrap(),
            "ftp://user:first@host/file"
        );
    }

    #[test]
    fn url_password_nested_ipc_and_non_url_json() {
        let store = CoreStore::in_memory().unwrap();
        let untouched = "{ \"untouched\": true }".to_string();
        assert_eq!(store.sanitize_urls(untouched.clone()).unwrap(), untouched);
        let encoded = serde_json::json!({"payload": serde_json::json!({"url":"ftp://user:secret@host/file"}).to_string()});
        let clean = store.sanitize_urls(encoded).unwrap();
        assert!(!clean.to_string().contains("secret"));
        assert_eq!(
            store.hydrate_url("ftp://user@host/file").unwrap(),
            "ftp://user:secret@host/file"
        );
        let target = crate::ftp_engine::parse_ftp_url("ftp://user:secret@host/file").unwrap();
        assert!(!format!("{target:?}").contains("secret"));
    }

    #[cfg(windows)]
    #[test]
    fn url_password_survives_restart_only_as_dpapi() {
        let path = std::env::temp_dir().join(format!(
            "ftp-vault-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut store = CoreStore::open(&path).unwrap();
            store
                .set_setting("url", "ftp://user:restart-secret@host/file")
                .unwrap();
            let blob: String = store
                .connection
                .query_row(
                    "SELECT protected_blob FROM credentials WHERE kind='url_password'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(blob.starts_with("dpapi:"));
            assert!(!blob.contains("restart-secret"));
            let proxy = "http://alice:legacy-setting-secret@proxy.example:8080";
            let rules = serde_json::json!([{"host":"files.example","proxy":"https://r:legacy-rule-secret@rules:443"}]).to_string();
            for (key, raw) in [("proxy_url", proxy), ("site_rules", rules.as_str())] {
                store
                    .connection
                    .execute(
                        "INSERT INTO settings(key,value_json) VALUES (?1,?2)",
                        rusqlite::params![key, serde_json::to_string(raw).unwrap()],
                    )
                    .unwrap();
            }
            let spec = TaskSpec {
                proxy: "socks5://bob:legacy-task-secret@127.0.0.1:1080".into(),
                ..Default::default()
            };
            store
                .connection
                .execute(
                    "INSERT INTO task_specs(task_id,spec_json) VALUES ('legacy',?1)",
                    [serde_json::to_string(&spec).unwrap()],
                )
                .unwrap();
            store
                .connection
                .execute(
                    "DELETE FROM settings WHERE key='url_passwords_migrated_v1'",
                    [],
                )
                .unwrap();
        }
        let store = CoreStore::open(&path).unwrap();
        let proxy = store.setting_string("proxy_url", "").unwrap();
        assert_eq!(proxy, "http://alice@proxy.example:8080");
        assert_eq!(
            store.hydrate_proxy(&proxy).unwrap(),
            "http://alice:legacy-setting-secret@proxy.example:8080"
        );
        let rules = crate::parse_site_rules(&store.setting_string("site_rules", "").unwrap());
        assert_eq!(rules[0].proxy, "https://r@rules:443");
        assert_eq!(
            store.hydrate_proxy(&rules[0].proxy).unwrap(),
            "https://r:legacy-rule-secret@rules:443"
        );
        let proxy = store.load_task_specs().unwrap().remove(0).1.proxy;
        assert_eq!(proxy, "socks5://bob@127.0.0.1:1080");
        assert_eq!(
            store.hydrate_proxy(&proxy).unwrap(),
            "socks5://bob:legacy-task-secret@127.0.0.1:1080"
        );
        let url = store.setting_string("url", "").unwrap();
        assert_eq!(url, "ftp://user@host/file");
        assert_eq!(
            store.hydrate_url(&url).unwrap(),
            "ftp://user:restart-secret@host/file"
        );
        drop(store);
        let bytes = std::fs::read(&path).unwrap();
        for secret in [
            "restart-secret",
            "legacy-task-secret",
            "legacy-setting-secret",
            "legacy-rule-secret",
        ] {
            assert!(!String::from_utf8_lossy(&bytes).contains(secret));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn migrated_flag_skips_cold_boot_table_scan_and_redact_fast_path_roundtrips() {
        let path = std::env::temp_dir().join(format!(
            "url-migrated-flag-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let store = CoreStore::open(&path).unwrap();
            assert_eq!(
                store
                    .setting_string("url_passwords_migrated_v1", "")
                    .unwrap(),
                "1"
            );
            // Inject a trigger that would abort if `migrate_url_passwords` scanned and updated `task_specs`.
            let spec = TaskSpec {
                url: "https://cdn.example.test/video.mp4?token=abc".into(),
                ..Default::default()
            };
            store
                .connection
                .execute(
                    "INSERT INTO task_specs(task_id, spec_json) VALUES ('t1', ?1)",
                    [serde_json::to_string(&spec).unwrap()],
                )
                .unwrap();
        }
        let observer = rusqlite::Connection::open(&path).unwrap();
        let before: i64 = observer
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .unwrap();
        let reopened = CoreStore::open(&path).unwrap();
        let after: i64 = observer
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, after, "cold boot with migration flag set must not write");
        assert_eq!(reopened.load_task_specs().unwrap().len(), 1);
        drop(reopened);
        drop(observer);
        let _ = std::fs::remove_file(path);

        // Verify fast-path `redact_urls` without `@` and slow-path with `@`.
        let plain_cmd = crate::CoreCommand::UpdateProgress {
            task_id: "task-fast".into(),
            downloaded_bytes: 512,
            total_bytes: Some(1024),
            speed_bytes_per_sec: 256,
            stage: "transfer".into(),
            status: "downloading".into(),
        };
        assert_eq!(redact_urls(plain_cmd.clone()).unwrap(), plain_cmd);
    }
}
