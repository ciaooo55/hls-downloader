//! Atomic first-run migration from the Rust v6 Core database.
//!
//! Like the 5.x importer, `HLS_V6_SKIP_MIGRATE` skips the adoption entirely and
//! lets v7 start with a fresh database; the v6 database stays untouched on disk.

use crate::{default_v7_download_dir, MediaPushRequest, ResourceOffer, TaskSnapshot, TaskSpec};
use rusqlite::{backup::Backup, params, Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIGRATED_FLAG: &str = "migrated_from_v6";

pub(crate) fn migrate_installed_v6_database(target: &Path) -> Result<(), String> {
    if target.exists() {
        return Ok(());
    }
    if std::env::var_os("HLS_V6_SKIP_MIGRATE").is_some() {
        return Ok(());
    }
    if std::env::var_os("HLS_V7_DATA_DIR").is_some() {
        return Ok(());
    }
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return Ok(());
    };
    let installed_target = local_app_data
        .join("HLS Downloader")
        .join("v7")
        .join("data.db");
    if target != installed_target {
        return Ok(());
    }
    let source = local_app_data
        .join("HLS Downloader")
        .join("v6")
        .join("data.db");
    if !source.is_file() {
        return Ok(());
    }
    migrate_v6_database_from(&source, target, &local_app_data)
}

/// Migrate one explicit v6 database into `target`. Split from the installed-path
/// discovery in [`migrate_installed_v6_database`] so tests can drive real
/// migrations with temporary directories.
pub(crate) fn migrate_v6_database_from(
    source: &Path,
    target: &Path,
    local_app_data: &Path,
) -> Result<(), String> {
    if !source.is_file() {
        return Ok(());
    }
    if target.exists() {
        return Ok(());
    }
    ensure_v6_not_running()?;

    let parent = target
        .parent()
        .ok_or_else(|| format!("v7 database has no parent: {}", target.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create v7 data directory {}: {error}", parent.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("create v6 migration nonce: {error}"))?
        .as_nanos();
    let temp = parent.join(format!(
        ".data.db.v6-import-{}-{nonce}.tmp",
        std::process::id()
    ));

    let result = migrate_to_temp(source, &temp, local_app_data).and_then(|()| {
        if target.exists() {
            return Err(format!(
                "v7 database appeared during migration: {}",
                target.display()
            ));
        }
        fs::rename(&temp, target).map_err(|error| {
            format!(
                "commit migrated v7 database {} -> {}: {error}",
                temp.display(),
                target.display()
            )
        })
    });
    if result.is_err() {
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(temp.with_extension("tmp-wal"));
        let _ = fs::remove_file(temp.with_extension("tmp-shm"));
    }
    result
}

#[cfg(windows)]
fn ensure_v6_not_running() -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_FILE_NOT_FOUND};
    use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};

    let name: Vec<u16> = "Local\\HLSDownloader.v6\0".encode_utf16().collect();
    let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        return if error == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!(
                "check running v6 Core mutex: Windows error {error}"
            ))
        };
    }
    unsafe { CloseHandle(handle) };
    Err("v6 Core is still running; exit v6 before the first v7 start".into())
}

#[cfg(not(windows))]
fn ensure_v6_not_running() -> Result<(), String> {
    Ok(())
}

fn migrate_to_temp(source: &Path, temp: &Path, local_app_data: &Path) -> Result<(), String> {
    let source_connection =
        Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("open v6 database {}: {error}", source.display()))?;
    let mut target_connection = Connection::open(temp)
        .map_err(|error| format!("create v6 migration snapshot {}: {error}", temp.display()))?;
    {
        let backup = Backup::new(&source_connection, &mut target_connection)
            .map_err(|error| format!("initialize v6 SQLite backup: {error}"))?;
        backup
            .run_to_completion(100, Duration::from_millis(25), None)
            .map_err(|error| format!("copy consistent v6 SQLite snapshot: {error}"))?;
    }
    drop(source_connection);

    target_connection
        .pragma_update(None, "journal_mode", "DELETE")
        .map_err(|error| format!("finalize v6 migration journal: {error}"))?;
    validate_schema_and_json(&target_connection)?;
    normalize_download_dirs(&mut target_connection, local_app_data, source)?;
    let integrity: String = target_connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| format!("check migrated v6 database integrity: {error}"))?;
    if integrity != "ok" {
        return Err(format!(
            "migrated v6 database integrity check failed: {integrity}"
        ));
    }
    Ok(())
}

fn validate_schema_and_json(connection: &Connection) -> Result<(), String> {
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("read v6 schema version: {error}"))?;
    if version != crate::CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported v6 schema version {version}, expected {}",
            crate::CURRENT_SCHEMA_VERSION
        ));
    }

    let task_ids = validate_typed_rows::<TaskSnapshot>(
        connection,
        "SELECT task_id, snapshot_json FROM tasks ORDER BY task_id",
        "task snapshot",
    )?;
    let spec_ids = validate_typed_rows::<TaskSpec>(
        connection,
        "SELECT task_id, spec_json FROM task_specs ORDER BY task_id",
        "task spec",
    )?;
    if task_ids != spec_ids {
        return Err("v6 tasks and task_specs do not contain the same task ids".into());
    }
    validate_json_rows(
        connection,
        "SELECT track_id, track_json FROM media_tracks ORDER BY task_id, track_id",
        "media track",
    )?;
    validate_json_rows(
        connection,
        "SELECT key, value_json FROM settings ORDER BY key",
        "setting",
    )?;

    let mut statement = connection
        .prepare("SELECT handoff_id, public_json, status FROM handoffs ORDER BY handoff_id")
        .map_err(|error| format!("prepare v6 handoff validation: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("query v6 handoffs: {error}"))?;
    for row in rows {
        let (handoff_id, encoded, status) =
            row.map_err(|error| format!("read v6 handoff: {error}"))?;
        let value: Value = serde_json::from_str(&encoded)
            .map_err(|error| format!("decode v6 handoff {handoff_id}: {error}"))?;
        if status == "pending"
            && serde_json::from_value::<MediaPushRequest>(value.clone()).is_err()
            && value
                .get("offer")
                .cloned()
                .and_then(|offer| serde_json::from_value::<ResourceOffer>(offer).ok())
                .is_none()
        {
            return Err(format!("decode pending v6 handoff {handoff_id}"));
        }
    }
    Ok(())
}

fn validate_typed_rows<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    query: &str,
    label: &str,
) -> Result<BTreeSet<String>, String> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| format!("prepare v6 {label} validation: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("query v6 {label}s: {error}"))?;
    let mut ids = BTreeSet::new();
    for row in rows {
        let (id, encoded) = row.map_err(|error| format!("read v6 {label}: {error}"))?;
        serde_json::from_str::<T>(&encoded)
            .map_err(|error| format!("decode v6 {label} {id}: {error}"))?;
        ids.insert(id);
    }
    Ok(ids)
}

fn validate_json_rows(connection: &Connection, query: &str, label: &str) -> Result<(), String> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| format!("prepare v6 {label} validation: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("query v6 {label}s: {error}"))?;
    for row in rows {
        let (id, encoded) = row.map_err(|error| format!("read v6 {label}: {error}"))?;
        serde_json::from_str::<Value>(&encoded)
            .map_err(|error| format!("decode v6 {label} {id}: {error}"))?;
    }
    Ok(())
}

fn normalize_download_dirs(
    connection: &mut Connection,
    local_app_data: &Path,
    source: &Path,
) -> Result<(), String> {
    let roots = legacy_roots(local_app_data, source);
    let default = default_v7_download_dir();
    let mut statement = connection
        .prepare(
            "SELECT s.task_id, s.spec_json, t.snapshot_json \
             FROM task_specs AS s JOIN tasks AS t ON t.task_id = s.task_id \
             ORDER BY s.task_id",
        )
        .map_err(|error| format!("prepare v6 task path migration: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("query v6 task paths: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read v6 task paths: {error}"))?;
    drop(statement);

    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin v6 path migration: {error}"))?;
    for (task_id, encoded, snapshot_json) in rows {
        let mut spec: TaskSpec = serde_json::from_str(&encoded)
            .map_err(|error| format!("decode v6 task spec {task_id}: {error}"))?;
        let original = PathBuf::from(spec.download_dir.trim());
        let relative_dir = if original.as_os_str().is_empty() {
            PathBuf::from("downloads")
        } else {
            original.clone()
        };
        let resolved_dir = if original.is_absolute() {
            original
        } else {
            let resolved = locate_legacy_download_dir(&roots, &relative_dir, &task_id, &spec.filename)
                .unwrap_or_else(|| default.clone());
            spec.download_dir = resolved.to_string_lossy().into_owned();
            let updated = serde_json::to_string(&spec)
                .map_err(|error| format!("encode migrated v6 task spec {task_id}: {error}"))?;
            transaction
                .execute(
                    "UPDATE task_specs SET spec_json = ?1 WHERE task_id = ?2",
                    params![updated, task_id],
                )
                .map_err(|error| format!("write migrated v6 task spec: {error}"))?;
            resolved
        };

        let mut snapshot: TaskSnapshot = serde_json::from_str(&snapshot_json)
            .map_err(|error| format!("decode v6 task snapshot {task_id}: {error}"))?;
        if sync_snapshot(&mut snapshot, &resolved_dir, &relative_dir) {
            let updated = serde_json::to_string(&snapshot)
                .map_err(|error| format!("encode migrated v6 task snapshot {task_id}: {error}"))?;
            transaction
                .execute(
                    "UPDATE tasks SET snapshot_json = ?1 WHERE task_id = ?2",
                    params![updated, task_id],
                )
                .map_err(|error| format!("write migrated v6 task snapshot: {error}"))?;
        }
    }

    let raw_download_dir: Option<String> = transaction
        .query_row(
            "SELECT value_json FROM settings WHERE key = 'download_dir'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("read v6 default download directory: {error}"))?;
    let configured = raw_download_dir
        .as_deref()
        .and_then(|encoded| serde_json::from_str::<String>(encoded).ok())
        .unwrap_or_default();
    let configured_path = PathBuf::from(configured.trim());
    if !configured_path.is_absolute() {
        let relative = if configured_path.as_os_str().is_empty() {
            Path::new("downloads")
        } else {
            configured_path.as_path()
        };
        let resolved = roots
            .iter()
            .map(|root| root.join(relative))
            .find(|candidate| candidate.is_dir())
            .unwrap_or(default);
        transaction
            .execute(
                "INSERT INTO settings(key, value_json) VALUES ('download_dir', ?1) \
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                params![serde_json::to_string(&resolved.to_string_lossy()).map_err(
                    |error| format!("encode migrated v6 default download directory: {error}")
                )?],
            )
            .map_err(|error| format!("write migrated v6 default download directory: {error}"))?;
    }
    transaction
        .execute(
            "INSERT INTO settings(key, value_json) VALUES (?1, 'true') \
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![MIGRATED_FLAG],
        )
        .map_err(|error| format!("mark v6 database migration complete: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("commit v6 path migration: {error}"))
}

/// Rebase relative snapshot paths onto the resolved download directory so the
/// persisted snapshot agrees with the migrated spec. Returns true when changed.
fn sync_snapshot(snapshot: &mut TaskSnapshot, resolved_dir: &Path, relative_dir: &Path) -> bool {
    let mut dirty = false;
    if !Path::new(snapshot.download_dir.trim()).is_absolute() {
        snapshot.download_dir = resolved_dir.to_string_lossy().into_owned();
        dirty = true;
    }
    let output = snapshot.output_path.trim();
    if !output.is_empty() && !Path::new(output).is_absolute() {
        let remainder = Path::new(output)
            .strip_prefix(relative_dir)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| Path::new(output).file_name().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(output));
        snapshot.output_path = resolved_dir.join(remainder).to_string_lossy().into_owned();
        dirty = true;
    }
    dirty
}

fn legacy_roots(local_app_data: &Path, source: &Path) -> Vec<PathBuf> {
    let mut roots = vec![local_app_data.join("Programs").join("HLS Downloader v6")];
    if let Some(parent) = source.parent() {
        roots.push(parent.to_path_buf());
    }
    roots
}

fn locate_legacy_download_dir(
    roots: &[PathBuf],
    relative: &Path,
    task_id: &str,
    filename: &str,
) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = roots.iter().map(|root| root.join(relative)).collect();
    candidates
        .iter()
        .find(|candidate| candidate.join(".v6-tasks").join(task_id).is_dir())
        .cloned()
        .or_else(|| {
            (!filename.trim().is_empty())
                .then(|| {
                    candidates
                        .iter()
                        .find(|candidate| candidate.join(filename).is_file())
                        .cloned()
                })
                .flatten()
        })
        .or_else(|| candidates.into_iter().find(|candidate| candidate.is_dir()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    fn test_dir(label: &str) -> PathBuf {
        let seq = TEST_SEQ.fetch_add(1, Ordering::SeqCst);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap().join("target"));
        let dir = base
            .join("v6-migrate-tests")
            .join(format!("{label}-{}-{seq}-{nonce}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seed_database(path: &Path, user_version: u32) -> Connection {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tasks (\
                   task_id TEXT PRIMARY KEY,\
                   snapshot_json TEXT NOT NULL\
                 );\
                 CREATE TABLE task_specs (\
                   task_id TEXT PRIMARY KEY,\
                   spec_json TEXT NOT NULL\
                 );\
                 CREATE TABLE media_tracks (\
                   task_id TEXT NOT NULL,\
                   track_id INTEGER NOT NULL,\
                   track_json TEXT NOT NULL\
                 );\
                 CREATE TABLE settings (\
                   key TEXT PRIMARY KEY,\
                   value_json TEXT NOT NULL\
                 );\
                 CREATE TABLE handoffs (\
                   handoff_id TEXT PRIMARY KEY,\
                   public_json TEXT NOT NULL,\
                   status TEXT NOT NULL\
                 );",
            )
            .unwrap();
        connection
            .execute_batch(&format!("PRAGMA user_version = {user_version};"))
            .unwrap();
        connection
    }

    fn sample_spec(download_dir: &str) -> TaskSpec {
        TaskSpec {
            url: "https://cdn.test/movie.mp4".into(),
            filename: "movie.mp4".into(),
            download_dir: download_dir.into(),
            concurrency: 1,
            ..Default::default()
        }
    }

    fn sample_snapshot(task_id: &str, download_dir: &str, output_path: &str) -> TaskSnapshot {
        TaskSnapshot {
            task_id: task_id.into(),
            filename: "movie.mp4".into(),
            download_dir: download_dir.into(),
            output_path: output_path.into(),
            ..Default::default()
        }
    }

    fn insert_task(
        connection: &Connection,
        task_id: &str,
        snapshot: &TaskSnapshot,
        spec: &TaskSpec,
    ) {
        connection
            .execute(
                "INSERT INTO tasks(task_id, snapshot_json) VALUES (?1, ?2)",
                params![task_id, serde_json::to_string(snapshot).unwrap()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_specs(task_id, spec_json) VALUES (?1, ?2)",
                params![task_id, serde_json::to_string(spec).unwrap()],
            )
            .unwrap();
    }

    fn read_setting(connection: &Connection, key: &str) -> Option<String> {
        connection
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    }

    fn leftover_temp_files(parent: &Path) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".data.db.v6-import-") {
                    names.push(name);
                }
            }
        }
        names
    }

    #[test]
    fn relative_dirs_rebase_onto_discovered_legacy_root() {
        let base = test_dir("rebase");
        let source = base.join("v6").join("data.db");
        let target = base.join("v7").join("data.db");
        let local_app_data = base.join("local");

        let connection = seed_database(&source, crate::CURRENT_SCHEMA_VERSION);
        let spec = sample_spec("downloads");
        let snapshot = sample_snapshot("task-1", "downloads", "downloads/movie.mp4");
        insert_task(&connection, "task-1", &snapshot, &spec);
        drop(connection);

        let resume_root = source.parent().unwrap().join("downloads");
        fs::create_dir_all(resume_root.join(".v6-tasks").join("task-1")).unwrap();

        migrate_v6_database_from(&source, &target, &local_app_data).unwrap();
        assert!(target.is_file());

        let connection =
            Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let (spec_json, snapshot_json): (String, String) = connection
            .query_row(
                "SELECT s.spec_json, t.snapshot_json \
                 FROM task_specs AS s JOIN tasks AS t ON t.task_id = s.task_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let spec: TaskSpec = serde_json::from_str(&spec_json).unwrap();
        let snapshot: TaskSnapshot = serde_json::from_str(&snapshot_json).unwrap();
        let expected = resume_root.to_string_lossy().into_owned();
        assert_eq!(spec.download_dir, expected);
        assert_eq!(snapshot.download_dir, expected);
        assert_eq!(
            snapshot.output_path,
            resume_root.join("movie.mp4").to_string_lossy()
        );
        assert_eq!(read_setting(&connection, MIGRATED_FLAG).as_deref(), Some("true"));
    }

    #[test]
    fn absolute_dirs_are_left_untouched() {
        let base = test_dir("absolute");
        let source = base.join("v6").join("data.db");
        let target = base.join("v7").join("data.db");
        let local_app_data = base.join("local");
        let absolute = base.join("v6").join("keep");
        fs::create_dir_all(&absolute).unwrap();

        let connection = seed_database(&source, crate::CURRENT_SCHEMA_VERSION);
        let spec = sample_spec(&absolute.to_string_lossy());
        let snapshot = sample_snapshot(
            "task-1",
            &absolute.to_string_lossy(),
            &absolute.join("movie.mp4").to_string_lossy(),
        );
        insert_task(&connection, "task-1", &snapshot, &spec);
        drop(connection);

        migrate_v6_database_from(&source, &target, &local_app_data).unwrap();

        let connection =
            Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let (spec_json, snapshot_json): (String, String) = connection
            .query_row(
                "SELECT s.spec_json, t.snapshot_json \
                 FROM task_specs AS s JOIN tasks AS t ON t.task_id = s.task_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let spec: TaskSpec = serde_json::from_str(&spec_json).unwrap();
        let snapshot: TaskSnapshot = serde_json::from_str(&snapshot_json).unwrap();
        assert_eq!(spec.download_dir, absolute.to_string_lossy());
        assert_eq!(snapshot.download_dir, absolute.to_string_lossy());
        assert_eq!(
            snapshot.output_path,
            absolute.join("movie.mp4").to_string_lossy()
        );
    }

    #[test]
    fn relative_default_download_dir_setting_is_resolved() {
        let base = test_dir("setting");
        let source = base.join("v6").join("data.db");
        let target = base.join("v7").join("data.db");
        let local_app_data = base.join("local");

        let connection = seed_database(&source, crate::CURRENT_SCHEMA_VERSION);
        connection
            .execute(
                "INSERT INTO settings(key, value_json) VALUES ('download_dir', '\"downloads\"')",
                [],
            )
            .unwrap();
        drop(connection);
        fs::create_dir_all(source.parent().unwrap().join("downloads")).unwrap();

        migrate_v6_database_from(&source, &target, &local_app_data).unwrap();

        let connection =
            Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let resolved: String =
            serde_json::from_str(&read_setting(&connection, "download_dir").unwrap()).unwrap();
        assert_eq!(
            Path::new(&resolved),
            &source.parent().unwrap().join("downloads")
        );
        assert_eq!(read_setting(&connection, MIGRATED_FLAG).as_deref(), Some("true"));
    }

    #[test]
    fn unsupported_schema_version_fails_without_target() {
        let base = test_dir("version");
        let source = base.join("v6").join("data.db");
        let target = base.join("v7").join("data.db");
        let local_app_data = base.join("local");
        seed_database(&source, 5);

        let error = migrate_v6_database_from(&source, &target, &local_app_data).unwrap_err();
        assert!(error.contains("unsupported v6 schema version"), "{error}");
        assert!(!target.exists());
        assert!(leftover_temp_files(target.parent().unwrap()).is_empty());
    }

    #[test]
    fn undecodable_snapshot_fails_validation() {
        let base = test_dir("snapshot");
        let source = base.join("v6").join("data.db");
        let target = base.join("v7").join("data.db");
        let local_app_data = base.join("local");

        let connection = seed_database(&source, crate::CURRENT_SCHEMA_VERSION);
        let spec = sample_spec("downloads");
        connection
            .execute(
                "INSERT INTO tasks(task_id, snapshot_json) VALUES ('task-1', '{not json')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_specs(task_id, spec_json) VALUES ('task-1', ?1)",
                params![serde_json::to_string(&spec).unwrap()],
            )
            .unwrap();
        drop(connection);

        let error = migrate_v6_database_from(&source, &target, &local_app_data).unwrap_err();
        assert!(error.contains("decode v6 task snapshot"), "{error}");
        assert!(!target.exists());
    }

    #[test]
    fn mismatched_task_and_spec_ids_fail() {
        let base = test_dir("ids");
        let source = base.join("v6").join("data.db");
        let target = base.join("v7").join("data.db");
        let local_app_data = base.join("local");

        let connection = seed_database(&source, crate::CURRENT_SCHEMA_VERSION);
        let spec = sample_spec("downloads");
        let snapshot = sample_snapshot("task-1", "downloads", "");
        connection
            .execute(
                "INSERT INTO tasks(task_id, snapshot_json) VALUES ('task-1', ?1)",
                params![serde_json::to_string(&snapshot).unwrap()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_specs(task_id, spec_json) VALUES ('task-2', ?1)",
                params![serde_json::to_string(&spec).unwrap()],
            )
            .unwrap();
        drop(connection);

        let error = migrate_v6_database_from(&source, &target, &local_app_data).unwrap_err();
        assert!(error.contains("same task ids"), "{error}");
        assert!(!target.exists());
    }

    #[test]
    fn existing_target_is_untouched() {
        let base = test_dir("existing");
        let source = base.join("v6").join("data.db");
        let target = base.join("v7").join("data.db");
        let local_app_data = base.join("local");

        let connection = seed_database(&source, crate::CURRENT_SCHEMA_VERSION);
        let spec = sample_spec("downloads");
        let snapshot = sample_snapshot("task-1", "downloads", "");
        insert_task(&connection, "task-1", &snapshot, &spec);
        drop(connection);

        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"keep").unwrap();

        migrate_v6_database_from(&source, &target, &local_app_data).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }
}
