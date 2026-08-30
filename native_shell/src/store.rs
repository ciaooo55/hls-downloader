//! SQLite persistence for the resident Rust Core.
//!
//! Only UI-safe task snapshots are stored here. Browser credentials and replay
//! material will live in the DPAPI-backed credential vault and are referenced
//! by opaque IDs rather than being copied into task rows or diagnostics.

use crate::{CoreEvent, EventEnvelope, TaskSnapshot, TaskSpec};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CURRENT_SCHEMA_VERSION: u32 = 6;

pub struct CoreStore {
    connection: Connection,
    path: Option<PathBuf>,
}

impl CoreStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create Core data directory: {error}"))?;
        }
        let connection = Connection::open(path)
            .map_err(|error| format!("open Core database {}: {error}", path.display()))?;
        let mut store = Self {
            connection,
            path: Some(path.to_path_buf()),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory()
            .map_err(|error| format!("open in-memory Core database: {error}"))?;
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
            .map_err(|error| format!("read Core schema version: {error}"))
    }

    pub fn load_tasks(&self) -> Result<Vec<TaskSnapshot>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT snapshot_json FROM tasks ORDER BY created_at_ms, task_id")
            .map_err(|error| format!("prepare Core task restore: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("query Core tasks: {error}"))?;
        let mut tasks = Vec::new();
        for row in rows {
            let json = row.map_err(|error| format!("read Core task row: {error}"))?;
            let snapshot = serde_json::from_str(&json)
                .map_err(|error| format!("decode Core task snapshot: {error}"))?;
            tasks.push(snapshot);
        }
        Ok(tasks)
    }

    pub fn load_task_specs(&self) -> Result<Vec<(String, TaskSpec)>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT task_id, spec_json FROM task_specs ORDER BY task_id")
            .map_err(|error| format!("prepare Core spec restore: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("query Core task specs: {error}"))?;
        let mut specs = Vec::new();
        for row in rows {
            let (task_id, json) =
                row.map_err(|error| format!("read Core task spec row: {error}"))?;
            let spec = serde_json::from_str(&json)
                .map_err(|error| format!("decode Core task spec {task_id}: {error}"))?;
            specs.push((task_id, spec));
        }
        Ok(specs)
    }

    pub fn load_task_log(&self, task_id: &str, limit: usize) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT message FROM logs WHERE task_id = ?1 ORDER BY id DESC LIMIT ?2")
            .map_err(|error| format!("prepare Core task log {task_id}: {error}"))?;
        let rows = statement
            .query_map(params![task_id, limit.max(1).min(500) as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| format!("query Core task log {task_id}: {error}"))?;
        let mut lines = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read Core task log {task_id}: {error}"))?;
        lines.reverse();
        Ok(lines)
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
            .map_err(|error| format!("read Core event checkpoint: {error}"))
    }

    pub fn apply_events(&mut self, events: &[EventEnvelope]) -> Result<(), String> {
        self.apply_events_and_spec(events, None)
    }

    pub fn apply_events_and_spec(
        &mut self,
        events: &[EventEnvelope],
        spec: Option<&TaskSpec>,
    ) -> Result<(), String> {
        if events.is_empty() && spec.is_none() {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| format!("begin Core event transaction: {error}"))?;
        for envelope in events {
            match &envelope.event {
                CoreEvent::TaskCreated { snapshot } | CoreEvent::TaskUpdated { snapshot } => {
                    upsert_task(&transaction, snapshot, envelope.sequence)?;
                    if snapshot.status == "failed" {
                        if let Some(line) = snapshot.log_tail.last() {
                            append_task_log(&transaction, snapshot, "error", line)?;
                        }
                    }
                }
                CoreEvent::TaskProgress { snapshot } => {
                    upsert_task(&transaction, snapshot, envelope.sequence)?;
                    if let Some(line) = snapshot.log_tail.last() {
                        append_task_log(&transaction, snapshot, "progress", line)?;
                    }
                }
                CoreEvent::TaskDeleted { task_id } => {
                    transaction
                        .execute("DELETE FROM tasks WHERE task_id = ?1", params![task_id])
                        .map_err(|error| format!("delete Core task {task_id}: {error}"))?;
                    transaction
                        .execute(
                            "DELETE FROM task_specs WHERE task_id = ?1",
                            params![task_id],
                        )
                        .map_err(|error| format!("delete Core task spec {task_id}: {error}"))?;
                    transaction
                        .execute("DELETE FROM logs WHERE task_id = ?1", params![task_id])
                        .map_err(|error| format!("delete Core task log {task_id}: {error}"))?;
                }
                CoreEvent::Ready { .. }
                | CoreEvent::SettingsChanged { .. }
                | CoreEvent::ClipboardOffer { .. }
                | CoreEvent::HandoffOffered { .. }
                | CoreEvent::HandoffResolved { .. }
                | CoreEvent::UiShow { .. }
                | CoreEvent::Error { .. }
                | CoreEvent::ProbeResult { .. }
                | CoreEvent::TorrentProbeResult { .. }
                | CoreEvent::TorrentSelectionResult { .. }
                | CoreEvent::TaskTorrentFiles { .. }
                | CoreEvent::CastDevices { .. }
                | CoreEvent::UpdateAvailable { .. }
                | CoreEvent::UpdateCurrent { .. }
                | CoreEvent::UpdateReady { .. }
                | CoreEvent::UpdateInstallStarted { .. }
                | CoreEvent::UpdateInstallResult { .. }
                | CoreEvent::DuplicateOffered { .. }
                | CoreEvent::Toast { .. }
                | CoreEvent::HarvestResult { .. }
                | CoreEvent::HarvestProbeResult { .. }
                | CoreEvent::TaskLog { .. }
                | CoreEvent::TaskExport { .. }
                | CoreEvent::BrowserStatus { .. }
                | CoreEvent::MediaPushRequested { .. }
                | CoreEvent::MediaPushResolved { .. }
                | CoreEvent::PowerActionPending { .. }
                | CoreEvent::CastSession { .. }
                | CoreEvent::PlayerSession { .. } => {}
            }
        }
        if let Some(spec) = spec {
            let task_id = events.iter().find_map(|event| match &event.event {
                CoreEvent::TaskCreated { snapshot }
                | CoreEvent::TaskUpdated { snapshot }
                | CoreEvent::TaskProgress { snapshot } => Some(snapshot.task_id.as_str()),
                _ => None,
            });
            if let Some(task_id) = task_id {
                let json = serde_json::to_string(spec)
                    .map_err(|error| format!("encode Core task spec {task_id}: {error}"))?;
                transaction
                    .execute(
                        "INSERT INTO task_specs(task_id, spec_json) VALUES (?1, ?2)\
                         ON CONFLICT(task_id) DO UPDATE SET spec_json = excluded.spec_json",
                        params![task_id, json],
                    )
                    .map_err(|error| format!("persist Core task spec {task_id}: {error}"))?;
            }
        }
        if let Some(sequence) = events.last().map(|event| event.sequence) {
            transaction
                .execute(
                    "INSERT INTO event_checkpoints(checkpoint_id, sequence) VALUES (1, ?1)\
                     ON CONFLICT(checkpoint_id) DO UPDATE SET sequence = excluded.sequence",
                    params![sequence as i64],
                )
                .map_err(|error| format!("persist Core event checkpoint: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("commit Core event transaction: {error}"))
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
            .map_err(|error| format!("read Core setting {key}: {error}"))?;
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
            .map_err(|error| format!("read Core setting {key}: {error}"))?;
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
            .map_err(|error| format!("read Core setting {key}: {error}"))?;
        match raw {
            None => Ok(fallback.to_string()),
            Some(value) => Ok(serde_json::from_str(&value).unwrap_or(value)),
        }
    }

    pub fn set_setting<T: serde::Serialize>(&mut self, key: &str, value: T) -> Result<(), String> {
        let value = serde_json::to_string(&value)
            .map_err(|error| format!("encode Core setting {key}: {error}"))?;
        self.connection
            .execute(
                r#"INSERT INTO settings(key, value_json) VALUES (?1, ?2)
                   ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json"#,
                params![key, value],
            )
            .map_err(|error| format!("write Core setting {key}: {error}"))?;
        Ok(())
    }

    pub fn set_settings(
        &mut self,
        values: &BTreeMap<String, serde_json::Value>,
    ) -> Result<(), String> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| format!("begin Core settings transaction: {error}"))?;
        for (key, value) in values {
            let encoded = serde_json::to_string(value)
                .map_err(|error| format!("encode Core setting {key}: {error}"))?;
            transaction
                .execute(
                    r#"INSERT INTO settings(key, value_json) VALUES (?1, ?2)
                       ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json"#,
                    params![key, encoded],
                )
                .map_err(|error| format!("write Core setting {key}: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("commit Core settings transaction: {error}"))
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
            .map_err(|error| format!("store Core credential {credential_ref}: {error}"))?;
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
            .map_err(|error| format!("load Core credential {credential_ref}: {error}"))
    }

    pub fn delete_credential(&mut self, credential_ref: &str) -> Result<(), String> {
        self.connection
            .execute(
                "DELETE FROM credentials WHERE credential_ref = ?1",
                params![credential_ref],
            )
            .map_err(|error| format!("delete Core credential {credential_ref}: {error}"))?;
        Ok(())
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
            .map_err(|error| format!("save Core handoff {handoff_id}: {error}"))?;
        Ok(())
    }

    pub fn load_handoffs(&self) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT public_json FROM handoffs ORDER BY created_at_ms")
            .map_err(|error| format!("prepare Core handoff restore: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("query Core handoffs: {error}"))?;
        rows.map(|row| row.map_err(|error| format!("read Core handoff: {error}")))
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
                 CREATE INDEX IF NOT EXISTS idx_logs_task ON logs(task_id, id);\
                 PRAGMA user_version = {CURRENT_SCHEMA_VERSION};"
            ))
            .map_err(|error| format!("initialize Core schema: {error}"))
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
        .map_err(|error| format!("encode Core task {}: {error}", snapshot.task_id))?;
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
        .map_err(|error| format!("persist Core task {}: {error}", snapshot.task_id))?;
    Ok(())
}

fn append_task_log(
    transaction: &Transaction<'_>,
    snapshot: &TaskSnapshot,
    level: &str,
    message: &str,
) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    transaction
        .execute(
            "INSERT INTO logs(task_id, level, code, message, created_at_ms)\
             SELECT ?1, ?2, ?3, ?4, ?5\
             WHERE COALESCE((SELECT message FROM logs WHERE task_id = ?1 ORDER BY id DESC LIMIT 1), '') <> ?4",
            params![snapshot.task_id, level, snapshot.stage, message, now],
        )
        .map_err(|error| format!("append Core task log {}: {error}", snapshot.task_id))?;
    transaction
        .execute(
            "DELETE FROM logs WHERE task_id = ?1 AND id NOT IN (SELECT id FROM logs WHERE task_id = ?1 ORDER BY id DESC LIMIT 500)",
            params![snapshot.task_id],
        )
        .map_err(|error| format!("trim Core task log {}: {error}", snapshot.task_id))?;
    Ok(())
}

pub fn default_v7_database_path() -> PathBuf {
    if let Some(root) = env::var_os("HLS_V7_DATA_DIR") {
        return PathBuf::from(root).join("data.db");
    }
    if let Some(root) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(root)
            .join("HLS Downloader")
            .join("v7")
            .join("data.db");
    }
    PathBuf::from("v7-data.db")
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
    fn current_schema_and_task_snapshots_roundtrip() {
        let mut store = CoreStore::in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
        let mut runtime = CoreRuntime::new();
        let mut task = spec();
        task.queue_id = "night-media".into();
        let events = runtime.handle(CoreCommand::CreateTask { spec: task });
        store.apply_events(&events).unwrap();
        let restored = store.load_tasks().unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].filename, "archive.zip");
        assert_eq!(restored[0].queue_id, "night-media");
    }

    #[test]
    fn settings_are_typed_and_persistent() {
        let mut store = CoreStore::in_memory().unwrap();
        store.set_setting("takeover_enabled", false).unwrap();
        store.set_setting("minimum_bytes", 42_u64).unwrap();
        assert!(!store.setting_bool("takeover_enabled", true).unwrap());
        assert_eq!(store.setting_u64("minimum_bytes", 0).unwrap(), 42);
    }

    #[test]
    fn task_export_advances_sequence_without_persisting_payload_as_a_task() {
        let mut store = CoreStore::in_memory().unwrap();
        store
            .apply_events(&[EventEnvelope {
                sequence: 12,
                event: CoreEvent::TaskExport {
                    format: "json".into(),
                    data: "{\"tasks\":[]}".into(),
                    task_count: 0,
                },
            }])
            .unwrap();
        assert!(store.load_tasks().unwrap().is_empty());
        assert_eq!(store.latest_sequence().unwrap(), 12);
    }
}
