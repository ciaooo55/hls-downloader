//! FTP secrets share the request-header credential table and DPAPI vault.
use crate::CoreStore;
use hls_client::url_credentials::map_ftp_passwords;
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
            _ => None,
        }
    });
    match identity {
        Some(identity) => format!("url-password:{}", serde_json::json!(identity)),
        None => format!("url-password:{authority}"),
    }
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
        self.url_passwords.borrow_mut().insert(key, password.into());
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
        #[cfg(windows)]
        for (authority, password) in &passwords {
            self.protect_url_password(authority, password)?;
        }
        let result = write(clean, &transaction)?;
        transaction.commit().map_err(|e| e.to_string())?;
        #[cfg(not(windows))]
        for (authority, password) in passwords {
            self.protect_url_password(&authority, &password)?;
        }
        Ok(result)
    }

    pub(crate) fn hydrate_url(&self, url: &str) -> Result<String, String> {
        let Some((scheme, rest)) = url.split_once("://") else {
            return Ok(url.into());
        };
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "ftp" | "ftps" | "sftp"
        ) {
            return Ok(url.into());
        }
        let authority = rest.split('/').next().unwrap_or(rest);
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
        let password = self.url_passwords.borrow().get(&key).cloned();
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
                let mut statement = self
                    .connection
                    .prepare(&format!("SELECT rowid, {column} FROM {table} WHERE rowid >= ?1 ORDER BY rowid LIMIT 1"))
                    .map_err(|e| e.to_string())?;
                let mut next = i64::MIN;
                loop {
                    use rusqlite::OptionalExtension;
                    let row = statement
                        .query_row([next], |row| {
                            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                        })
                        .optional()
                        .map_err(|e| e.to_string())?;
                    let Some((id, text)) = row else { break };
                    let mut value = serde_json::Value::String(text.clone());
                    sanitize_url_value(&mut value, &mut |authority, password| {
                        self.protect_url_password(authority, password)
                    })?;
                    let sanitized = value
                        .as_str()
                        .expect("sanitizing a string preserves its type");
                    if sanitized != text {
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
    let mut value = serde_json::to_value(value).map_err(|e| e.to_string())?;
    let mut passwords = Vec::new();
    sanitize_url_value(&mut value, &mut |authority, password| {
        passwords.push((authority.into(), password.into()));
        Ok(())
    })?;
    Ok((
        serde_json::from_value(value).map_err(|e| e.to_string())?,
        passwords,
    ))
}

fn sanitize_url_value(
    value: &mut serde_json::Value,
    protect: &mut impl FnMut(&str, &str) -> Result<(), String>,
) -> Result<(), String> {
    match value {
        serde_json::Value::String(text) => {
            // Some IPC records contain another serialized JSON document.
            if let Ok(mut nested) = serde_json::from_str::<serde_json::Value>(text) {
                if nested.is_object() || nested.is_array() {
                    let before = nested.clone();
                    sanitize_url_value(&mut nested, protect)?;
                    if nested != before {
                        *text = nested.to_string();
                    }
                    return Ok(());
                }
            }
            *text = map_ftp_passwords(text, protect)?;
        }
        serde_json::Value::Array(items) => {
            for item in items {
                sanitize_url_value(item, protect)?;
            }
        }
        serde_json::Value::Object(items) if !items.keys().any(|key| key.contains("://")) => {
            for item in items.values_mut() {
                sanitize_url_value(item, protect)?;
            }
        }
        serde_json::Value::Object(items) => {
            // URL-indexed maps need the same redaction as ordinary field values.
            for (key, mut item) in std::mem::take(items) {
                sanitize_url_value(&mut item, protect)?;
                let key = if key.contains("://") {
                    map_ftp_passwords(&key, &mut *protect)?
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
        }
        let store = CoreStore::open(&path).unwrap();
        let url = store.setting_string("url", "").unwrap();
        assert_eq!(url, "ftp://user@host/file");
        assert_eq!(
            store.hydrate_url(&url).unwrap(),
            "ftp://user:restart-secret@host/file"
        );
        drop(store);
        assert!(!String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains("restart-secret"));
        std::fs::remove_file(path).unwrap();
    }
}
