//! SQLite persistence for the Python-free v6 core.
//!
//! Only UI-safe task snapshots are stored here. Browser credentials and replay
//! material will live in the DPAPI-backed credential vault and are referenced
//! by opaque IDs rather than being copied into task rows or diagnostics.

use crate::{CoreEvent, EventEnvelope, TaskSnapshot, TaskSpec};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const V6_SCHEMA_VERSION: u32 = 6;

pub struct V6Store {
    connection: Connection,
    path: Option<PathBuf>,
}

impl V6Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create v6 data directory: {error}"))?;
        }
        let connection = Connection::open(path)
            .map_err(|error| format!("open v6 database {}: {error}", path.display()))?;
        let mut store = Self {
            connection,
            path: Some(path.to_path_buf()),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory()
            .map_err(|error| format!("open in-memory v6 database: {error}"))?;
        let mut store = Self {
            connection,
            path: None,
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn schema_version(&self) -> Result<u32, String> {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| format!("read v6 schema version: {error}"))
    }

    pub fn load_tasks(&self) -> Result<Vec<TaskSnapshot>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT snapshot_json FROM tasks ORDER BY created_at_ms, task_id")
            .map_err(|error| format!("prepare v6 task restore: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("query v6 tasks: {error}"))?;
        let mut tasks = Vec::new();
        for row in rows {
            let json = row.map_err(|error| format!("read v6 task row: {error}"))?;
            let snapshot = serde_json::from_str(&json)
                .map_err(|error| format!("decode v6 task snapshot: {error}"))?;
            tasks.push(snapshot);
        }
        Ok(tasks)
    }

    pub fn load_task_specs(&self) -> Result<Vec<(String, TaskSpec)>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT task_id, spec_json FROM task_specs ORDER BY task_id")
            .map_err(|error| format!("prepare v6 spec restore: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("query v6 task specs: {error}"))?;
        let mut specs = Vec::new();
        for row in rows {
            let (task_id, json) = row.map_err(|error| format!("read v6 task spec row: {error}"))?;
            let spec = serde_json::from_str(&json)
                .map_err(|error| format!("decode v6 task spec {task_id}: {error}"))?;
            specs.push((task_id, spec));
        }
        Ok(specs)
    }

    pub fn latest_sequence(&self) -> Result<u64, String> {
        self.connection
            .query_row(
                "SELECT sequence FROM event_checkpoints WHERE checkpoint_id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map(|value| value.unwrap_or(0).max(0) as u64)
            .map_err(|error| format!("read v6 event checkpoint: {error}"))
    }

    pub fn apply_events(&mut self, events: &[EventEnvelope]) -> Result<(), String> {
        self.apply_events_and_spec(events, None)
    }

    pub fn save_spec(&mut self, task_id: &str, spec: &TaskSpec) -> Result<(), String> {
        if task_id.trim().is_empty() {
            return Err("missing task id".into());
        }
        let json = serde_json::to_string(spec)
            .map_err(|error| format!("encode v6 task spec {task_id}: {error}"))?;
        self.connection
            .execute(
                "INSERT INTO task_specs(task_id, spec_json) VALUES (?1, ?2)\
                 ON CONFLICT(task_id) DO UPDATE SET spec_json = excluded.spec_json",
                params![task_id, json],
            )
            .map_err(|error| format!("persist v6 task spec {task_id}: {error}"))?;
        Ok(())
    }

    pub fn apply_events_and_spec(
        &mut self,
        events: &[EventEnvelope],
        created_spec: Option<&TaskSpec>,
    ) -> Result<(), String> {
        if events.is_empty() && created_spec.is_none() {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| format!("begin v6 event transaction: {error}"))?;
        for envelope in events {
            match &envelope.event {
                CoreEvent::TaskCreated { snapshot }
                | CoreEvent::TaskUpdated { snapshot }
                | CoreEvent::TaskProgress { snapshot } => {
                    upsert_task(&transaction, snapshot, envelope.sequence)?;
                }
                CoreEvent::TaskDeleted { task_id } => {
                    transaction
                        .execute("DELETE FROM tasks WHERE task_id = ?1", params![task_id])
                        .map_err(|error| format!("delete v6 task {task_id}: {error}"))?;
                    transaction
                        .execute(
                            "DELETE FROM task_specs WHERE task_id = ?1",
                            params![task_id],
                        )
                        .map_err(|error| format!("delete v6 task spec {task_id}: {error}"))?;
                }
                CoreEvent::Ready { .. }
                | CoreEvent::HandoffOffered { .. }
                | CoreEvent::HandoffResolved { .. }
                | CoreEvent::UiShow { .. }
                | CoreEvent::Error { .. }
                | CoreEvent::ProbeResult { .. }
                |                 CoreEvent::CastDevices { .. }
                | CoreEvent::DuplicateOffered { .. }
                | CoreEvent::Toast { .. }
                | CoreEvent::HarvestResult { .. }
                | CoreEvent::TaskLog { .. }
                | CoreEvent::BrowserStatus { .. }
                | CoreEvent::CastSession { .. } => {}
            }
        }
        if let Some(spec) = created_spec {
            let task_id = events.iter().find_map(|event| match &event.event {
                CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.as_str()),
                _ => None,
            });
            if let Some(task_id) = task_id {
                let json = serde_json::to_string(spec)
                    .map_err(|error| format!("encode v6 task spec {task_id}: {error}"))?;
                transaction
                    .execute(
                        "INSERT INTO task_specs(task_id, spec_json) VALUES (?1, ?2)\
                         ON CONFLICT(task_id) DO UPDATE SET spec_json = excluded.spec_json",
                        params![task_id, json],
                    )
                    .map_err(|error| format!("persist v6 task spec {task_id}: {error}"))?;
            }
        }
        if let Some(sequence) = events.last().map(|event| event.sequence) {
            transaction
                .execute(
                    "INSERT INTO event_checkpoints(checkpoint_id, sequence) VALUES (1, ?1)\
                     ON CONFLICT(checkpoint_id) DO UPDATE SET sequence = excluded.sequence",
                    params![sequence as i64],
                )
                .map_err(|error| format!("persist v6 event checkpoint: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("commit v6 event transaction: {error}"))
    }

    pub fn setting_bool(&self, key: &str, fallback: bool) -> Result<bool, String> {
        let raw: Option<String> = self
            .connection
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("read v6 setting {key}: {error}"))?;
        raw.map(|value| serde_json::from_str(&value).unwrap_or(fallback))
            .map_or(Ok(fallback), Ok)
    }

    pub fn setting_u64(&self, key: &str, fallback: u64) -> Result<u64, String> {
        let raw: Option<String> = self
            .connection
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("read v6 setting {key}: {error}"))?;
        raw.map(|value| serde_json::from_str(&value).unwrap_or(fallback))
            .map_or(Ok(fallback), Ok)
    }

    pub fn setting_string(&self, key: &str, fallback: &str) -> Result<String, String> {
        let raw: Option<String> = self
            .connection
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("read v6 setting {key}: {error}"))?;
        match raw {
            None => Ok(fallback.to_string()),
            Some(value) => Ok(serde_json::from_str(&value).unwrap_or(value)),
        }
    }

    pub fn set_setting<T: serde::Serialize>(&mut self, key: &str, value: T) -> Result<(), String> {
        let value = serde_json::to_string(&value)
            .map_err(|error| format!("encode v6 setting {key}: {error}"))?;
        self.connection
            .execute(
                r#"INSERT INTO settings(key, value_json) VALUES (?1, ?2)
                   ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json"#,
                params![key, value],
            )
            .map_err(|error| format!("write v6 setting {key}: {error}"))?;
        Ok(())
    }

    pub fn store_credential(
        &mut self,
        credential_ref: &str,
        protected_blob: &str,
        kind: &str,
    ) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        self.connection
            .execute(
                "INSERT INTO credentials(credential_ref, protected_blob, kind, created_at_ms)\
                 VALUES (?1, ?2, ?3, ?4)\
                 ON CONFLICT(credential_ref) DO UPDATE SET protected_blob = excluded.protected_blob, kind = excluded.kind",
                params![credential_ref, protected_blob, kind, now],
            )
            .map_err(|error| format!("store v6 credential {credential_ref}: {error}"))?;
        Ok(())
    }

    pub fn load_credential(&self, credential_ref: &str) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT protected_blob FROM credentials WHERE credential_ref = ?1",
                params![credential_ref],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("load v6 credential {credential_ref}: {error}"))
    }

    pub fn save_handoff(
        &mut self,
        handoff_id: &str,
        handoff_json: &str,
        status: &str,
        task_id: Option<&str>,
        created_at_ms: u64,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO handoffs(handoff_id, public_json, status, task_id, created_at_ms)\
                 VALUES (?1, ?2, ?3, ?4, ?5)\
                 ON CONFLICT(handoff_id) DO UPDATE SET public_json = excluded.public_json, status = excluded.status, task_id = excluded.task_id",
                params![handoff_id, handoff_json, status, task_id, created_at_ms as i64],
            )
            .map_err(|error| format!("save v6 handoff {handoff_id}: {error}"))?;
        Ok(())
    }

    pub fn load_handoffs(&self) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT public_json FROM handoffs ORDER BY created_at_ms")
            .map_err(|error| format!("prepare v6 handoff restore: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("query v6 handoffs: {error}"))?;
        rows.map(|row| row.map_err(|error| format!("read v6 handoff: {error}")))
            .collect()
    }

    fn initialize(&mut self) -> Result<(), String> {
        self.connection
            .execute_batch(&format!(
                "PRAGMA foreign_keys = ON;\
                 PRAGMA journal_mode = WAL;\
                 PRAGMA synchronous = NORMAL;\
                 CREATE TABLE IF NOT EXISTS tasks (\
                   task_id TEXT PRIMARY KEY,\
                   snapshot_json TEXT NOT NULL,\
                   created_at_ms INTEGER NOT NULL,\
                   updated_at_ms INTEGER NOT NULL,\
                   event_sequence INTEGER NOT NULL\
                 );\
                 CREATE TABLE IF NOT EXISTS task_specs (\
                   task_id TEXT PRIMARY KEY,\
                   spec_json TEXT NOT NULL\
                 );\
                 CREATE TABLE IF NOT EXISTS task_ranges (\
                   task_id TEXT NOT NULL,\
                   start_byte INTEGER NOT NULL,\
                   end_byte INTEGER NOT NULL,\
                   validator TEXT NOT NULL DEFAULT '',\
                   PRIMARY KEY(task_id, start_byte, end_byte),\
                   FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE\
                 );\
                 CREATE TABLE IF NOT EXISTS handoffs (\
                   handoff_id TEXT PRIMARY KEY,\
                   public_json TEXT NOT NULL,\
                   status TEXT NOT NULL,\
                   task_id TEXT,\
                   created_at_ms INTEGER NOT NULL\
                 );\
                 CREATE TABLE IF NOT EXISTS media_tracks (\
                   task_id TEXT NOT NULL,\
                   track_id TEXT NOT NULL,\
                   track_json TEXT NOT NULL,\
                   PRIMARY KEY(task_id, track_id),\
                   FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE\
                 );\
                 CREATE TABLE IF NOT EXISTS event_checkpoints (\
                   checkpoint_id INTEGER PRIMARY KEY CHECK(checkpoint_id = 1),\
                   sequence INTEGER NOT NULL\
                 );\
                 CREATE TABLE IF NOT EXISTS logs (\
                   id INTEGER PRIMARY KEY AUTOINCREMENT,\
                   task_id TEXT,\
                   level TEXT NOT NULL,\
                   code TEXT NOT NULL,\
                   message TEXT NOT NULL,\
                   created_at_ms INTEGER NOT NULL\
                 );\
                 CREATE TABLE IF NOT EXISTS settings (\
                   key TEXT PRIMARY KEY,\
                   value_json TEXT NOT NULL\
                 );\
                 CREATE TABLE IF NOT EXISTS credentials (\
                   credential_ref TEXT PRIMARY KEY,\
                   protected_blob TEXT NOT NULL,\
                   kind TEXT NOT NULL,\
                   created_at_ms INTEGER NOT NULL\
                 );\
                 CREATE INDEX IF NOT EXISTS idx_tasks_updated ON tasks(updated_at_ms DESC);\
                 CREATE INDEX IF NOT EXISTS idx_task_ranges_task ON task_ranges(task_id);\
                 PRAGMA user_version = {V6_SCHEMA_VERSION};"
            ))
            .map_err(|error| format!("initialize v6 schema: {error}"))
    }
}

fn upsert_task(
    transaction: &Transaction<'_>,
    snapshot: &TaskSnapshot,
    sequence: u64,
) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let json = serde_json::to_string(snapshot)
        .map_err(|error| format!("encode v6 task {}: {error}", snapshot.task_id))?;
    transaction
        .execute(
            r#"INSERT INTO tasks(task_id, snapshot_json, created_at_ms, updated_at_ms, event_sequence)
               VALUES (?1, ?2, ?3, ?3, ?4)
               ON CONFLICT(task_id) DO UPDATE SET
                 snapshot_json = excluded.snapshot_json,
                 updated_at_ms = excluded.updated_at_ms,
                 event_sequence = excluded.event_sequence"#,
            params![snapshot.task_id, json, now, sequence],
        )
        .map_err(|error| format!("persist v6 task {}: {error}", snapshot.task_id))?;
    Ok(())
}

pub fn default_v6_database_path() -> PathBuf {
    if let Some(root) = env::var_os("HLS_V6_DATA_DIR") {
        return PathBuf::from(root).join("data.db");
    }
    if let Some(root) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(root)
            .join("HLS Downloader")
            .join("v6")
            .join("data.db");
    }
    PathBuf::from("v6-data.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, CoreRuntime, ResourceKind, TaskSpec};

    fn spec() -> TaskSpec {
        TaskSpec {
            url: "https://example.test/archive.zip".into(),
            resource_kind: ResourceKind::File,
            title: "Archive".into(),
            filename: "archive.zip".into(),
            download_dir: String::new(),
            request_method: "GET".into(),
            credential_ref: None,
            replay_context_ref: None,
            concurrency: 8,
            checksum: None,
            expected_size: None,
            etag: String::new(),
            last_modified: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn schema_is_v6_and_task_snapshots_roundtrip() {
        let mut store = V6Store::in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), V6_SCHEMA_VERSION);
        let mut runtime = CoreRuntime::new();
        let events = runtime.handle(CoreCommand::CreateTask { spec: spec() });
        store.apply_events(&events).unwrap();
        let restored = store.load_tasks().unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].filename, "archive.zip");
    }

    #[test]
    fn settings_are_typed_and_persistent() {
        let mut store = V6Store::in_memory().unwrap();
        store.set_setting("takeover_enabled", false).unwrap();
        store.set_setting("minimum_bytes", 42_u64).unwrap();
        assert!(!store.setting_bool("takeover_enabled", true).unwrap());
        assert_eq!(store.setting_u64("minimum_bytes", 0).unwrap(), 42);
    }
}
