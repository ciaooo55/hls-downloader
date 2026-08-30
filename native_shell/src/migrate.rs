//! One-shot import of 5.x config.json and SQLite task rows into the current store.
//!
//! Does not start downloads. In-progress HTTP parts are copied into the v6
//! task directory so Range resume can reuse already-written bytes.

use crate::{
    apply_replay_json, CredentialVault, PersistentCore, ResourceKind, TaskPaths, TaskSpec,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MIGRATED_FLAG: &str = "migrated_from_5x";

/// Import once from discovered 5.x paths. Never deletes 5.x files.
pub fn maybe_migrate_from_5x(core: &mut PersistentCore) -> Result<u32, String> {
    if std::env::var_os("HLS_V6_SKIP_MIGRATE").is_some() {
        return Ok(0);
    }
    if core.store().setting_bool("migrated_from_v6", false)? {
        return Ok(0);
    }
    let force = std::env::var_os("HLS_V6_MIGRATE_FORCE").is_some();
    if !force && core.store().setting_bool(MIGRATED_FLAG, false)? {
        return Ok(0);
    }
    let (config, db) = resolve_legacy_paths();
    if !config.exists() && !db.exists() {
        return Ok(0);
    }
    let imported = migrate_from_5x(core, &config, &db)?;
    core.store_mut().set_setting(MIGRATED_FLAG, true)?;
    Ok(imported)
}

pub fn resolve_legacy_paths() -> (PathBuf, PathBuf) {
    let explicit_config = env_path("HLS_V6_MIGRATE_CONFIG");
    let explicit_db = env_path("HLS_V6_MIGRATE_DB");
    if explicit_config.is_some() || explicit_db.is_some() {
        return (
            explicit_config.unwrap_or_default(),
            explicit_db.unwrap_or_default(),
        );
    }
    let candidates = legacy_location_candidates();
    candidates
        .iter()
        .find(|(config, db)| config.is_file() && db.is_file())
        .cloned()
        .or_else(|| {
            candidates
                .into_iter()
                .find(|(config, db)| config.is_file() || db.is_file())
        })
        .unwrap_or_else(|| {
            (
                PathBuf::from("config.json"),
                PathBuf::from("backend/data.db"),
            )
        })
}

pub(crate) fn migration_requested_explicitly() -> bool {
    env_path("HLS_V6_MIGRATE_CONFIG").is_some()
        || env_path("HLS_V6_MIGRATE_DB").is_some()
        || env_path("HLS_V6_MIGRATE_FORCE").is_some()
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn legacy_location_candidates() -> Vec<(PathBuf, PathBuf)> {
    let mut roots = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        roots.push(local.join("HLS Downloader"));
        roots.push(local.join("Programs").join("HLS Downloader"));
        roots.push(local.join("Programs").join("HLS Downloader v6"));
    }
    roots.push(PathBuf::from(r"E:\HLS Downloader"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    let mut out = Vec::new();
    for root in roots {
        out.push((root.join("config.json"), root.join("data.db")));
        out.push((
            root.join("config.json"),
            root.join("backend").join("data.db"),
        ));
    }
    out
}

pub fn migrate_from_5x(
    core: &mut PersistentCore,
    config_path: &Path,
    db_path: &Path,
) -> Result<u32, String> {
    let mut imported = 0u32;
    let mut default_download_dir = String::new();
    if config_path.exists() {
        let text = std::fs::read_to_string(config_path).map_err(|error| error.to_string())?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("parse legacy config {}: {error}", config_path.display()))?;
        if !value.is_object() {
            return Err(format!(
                "legacy config is not an object: {}",
                config_path.display()
            ));
        }
        import_settings(core, &value)?;
        default_download_dir = value
            .get("download_dir")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(|path| resolve_legacy_path(config_path, path))
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    if !db_path.exists() {
        return Ok(imported);
    }
    let connection = rusqlite::Connection::open(db_path).map_err(|error| error.to_string())?;
    let mut existing_urls: std::collections::BTreeSet<String> = core
        .tasks()
        .into_iter()
        .filter_map(|task| core.task_spec(&task.task_id).map(|spec| spec.url.clone()))
        .collect();
    for row in load_legacy_tasks(&connection)? {
        if row.url.trim().is_empty() || existing_urls.contains(&row.url) {
            continue;
        }
        let download_dir = row
            .output_path
            .as_ref()
            .map(|path| resolve_legacy_path(db_path, path))
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .map(|parent| parent.to_string_lossy().into_owned())
            .filter(|dir| !dir.is_empty())
            .unwrap_or_else(|| default_download_dir.clone());
        let mut spec = TaskSpec {
            url: row.url.clone(),
            resource_kind: classify_task(&row.task_type, &row.url),
            filename: row.filename.clone(),
            title: if row.title.is_empty() {
                row.url.clone()
            } else {
                row.title.clone()
            },
            download_dir,
            request_method: crate::http_engine::sanitize_http_method(&row.request_method),
            expected_size: row.total_bytes.filter(|size| *size > 0),
            speed_limit_kib: row.speed_limit_kib.unwrap_or(0).max(0) as u32,
            checksum: row.checksum.clone(),
            concurrency: row.concurrency.unwrap_or(8).max(1) as u32,
            headers: public_headers(&row),
            ..Default::default()
        };
        if let Some(credential_ref) = store_row_credential(core, &row)? {
            spec.credential_ref = Some(credential_ref);
        }
        let events = core.handle(crate::CoreCommand::CreateTask { spec: spec.clone() })?;
        let Some(task_id) = events.iter().find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        }) else {
            continue;
        };
        existing_urls.insert(row.url.clone());
        let (status, stage, downloaded) = mapped_progress(&row);
        let _ = core.handle(crate::CoreCommand::UpdateProgress {
            task_id: task_id.clone(),
            downloaded_bytes: downloaded,
            total_bytes: row.total_bytes.filter(|size| *size > 0),
            speed_bytes_per_sec: 0,
            stage: stage.into(),
            status: status.into(),
        });
        if status != "completed" {
            import_http_partial(config_path, db_path, &row, &task_id, &spec);
            import_media_partial(config_path, db_path, &row, &task_id, &spec);
        }
        imported += 1;
    }
    Ok(imported)
}

fn import_settings(core: &mut PersistentCore, value: &Value) -> Result<(), String> {
    let legal = value
        .get("legal_terms_accepted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || value.as_object().is_some_and(|map| {
            map.keys()
                .any(|key| key.starts_with("legal_terms_accepted"))
                && map.iter().any(|(key, item)| {
                    key.starts_with("legal_terms_accepted")
                        && match item {
                            Value::Bool(true) => true,
                            Value::String(text) => !text.trim().is_empty(),
                            _ => false,
                        }
                })
        });
    if legal {
        core.store_mut().set_setting("legal_terms_accepted", true)?;
    }
    set_u64(
        core,
        "download_speed_limit_kib",
        first_u64(value, &["download_speed_limit_kib"]),
    )?;
    set_bool(
        core,
        "browser_takeover_enabled",
        first_bool(value, &["browser_takeover_enabled"]),
    )?;
    if let Some(bytes) = first_u64(value, &["browser_takeover_minimum_bytes"]) {
        set_u64(core, "browser_takeover_minimum_bytes", Some(bytes))?;
    } else if let Some(mb) = first_u64(value, &["browser_takeover_min_mb"]) {
        set_u64(
            core,
            "browser_takeover_minimum_bytes",
            Some(mb.saturating_mul(1024 * 1024)),
        )?;
    }
    set_bool(
        core,
        "download_speed_schedule_enabled",
        first_bool(
            value,
            &["download_speed_schedule_enabled", "speed_schedule_enabled"],
        ),
    )?;
    set_string(
        core,
        "download_speed_schedule_start",
        first_str(
            value,
            &["download_speed_schedule_start", "speed_schedule_start"],
        ),
    )?;
    set_string(
        core,
        "download_speed_schedule_end",
        first_str(
            value,
            &["download_speed_schedule_end", "speed_schedule_end"],
        ),
    )?;
    set_u64(
        core,
        "download_speed_schedule_kib",
        first_u64(
            value,
            &["download_speed_schedule_kib", "speed_schedule_limit_kib"],
        ),
    )?;
    set_u64(
        core,
        "queue_max_active",
        first_u64(value, &["queue_max_active", "max_concurrent_tasks"]).map(|value| value.max(1)),
    )?;
    set_string(core, "site_rules", first_str(value, &["site_rules"]))?;
    set_bool(
        core,
        "auto_category_dirs",
        first_bool(value, &["auto_category_dirs"]),
    )?;
    if let Some(dirs) = value.get("browser_category_dirs") {
        if dirs.is_object() {
            set_string(core, "browser_category_dirs", Some(&dirs.to_string()))?;
        }
    }
    set_bool(
        core,
        "av_scan_enabled",
        first_bool(value, &["av_scan_enabled"]),
    )?;
    set_string(
        core,
        "av_scan_command",
        first_str(value, &["av_scan_command"]),
    )?;
    set_string(
        core,
        "torrent_watch_dir",
        first_str(value, &["torrent_watch_dir", "watch_dir"]),
    )?;
    set_bool(
        core,
        "watch_torrents",
        first_bool(value, &["watch_torrents"]),
    )?;
    set_string(core, "download_dir", first_str(value, &["download_dir"]))?;
    set_string(core, "temp_dir", first_str(value, &["temp_dir"]))?;
    set_string(
        core,
        "default_origin",
        first_str(value, &["default_origin"]),
    )?;
    if let Some(raw_cookie) =
        first_str(value, &["default_cookie"]).filter(|value| !value.is_empty())
    {
        let cookie = CredentialVault.unprotect(raw_cookie)?;
        if !cookie.contains(['\r', '\n', '\0']) && cookie.len() <= 16 * 1024 {
            let replay = serde_json::json!({ "cookie": cookie }).to_string();
            let protected = if cfg!(windows) {
                CredentialVault.protect(&replay)?
            } else {
                replay
            };
            core.store_mut().store_credential(
                "settings:default-cookie",
                &protected,
                "default_cookie",
            )?;
        }
    }
    if let Some(hosts) = value.get("allowed_hosts") {
        let encoded = hosts
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .or_else(|| hosts.as_str().map(str::to_string));
        set_string(core, "allowed_hosts", encoded.as_deref())?;
    }
    set_bool(
        core,
        "av_scan_fail_on_threat",
        first_bool(value, &["av_scan_fail_on_threat"]),
    )?;
    set_u64(
        core,
        "bt_upload_limit_kib",
        first_u64(value, &["bt_upload_limit_kib"]),
    )?;
    set_u64(
        core,
        "bt_max_connections",
        first_u64(value, &["bt_max_connections"]),
    )?;
    set_bool(core, "bt_enable_dht", first_bool(value, &["bt_enable_dht"]))?;
    set_string(core, "proxy_url", first_str(value, &["proxy_url"]))?;
    Ok(())
}

fn set_u64(core: &mut PersistentCore, key: &str, value: Option<u64>) -> Result<(), String> {
    if let Some(value) = value {
        core.store_mut().set_setting(key, value)?;
    }
    Ok(())
}

fn set_bool(core: &mut PersistentCore, key: &str, value: Option<bool>) -> Result<(), String> {
    if let Some(value) = value {
        core.store_mut().set_setting(key, value)?;
    }
    Ok(())
}

fn set_string(core: &mut PersistentCore, key: &str, value: Option<&str>) -> Result<(), String> {
    if let Some(value) = value {
        core.store_mut().set_setting(key, value)?;
    }
    Ok(())
}

fn first_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
}

fn first_bool(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn first_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

#[derive(Default)]
struct LegacyTask {
    id: String,
    url: String,
    filename: String,
    title: String,
    task_type: String,
    request_method: String,
    total_bytes: Option<u64>,
    downloaded_bytes: u64,
    speed_limit_kib: Option<i64>,
    output_path: Option<String>,
    status: String,
    referer: String,
    origin: String,
    user_agent: String,
    cookie: String,
    request_headers: String,
    checksum: Option<String>,
    concurrency: Option<i64>,
}

fn load_legacy_tasks(connection: &rusqlite::Connection) -> Result<Vec<LegacyTask>, String> {
    let columns: std::collections::BTreeSet<String> = {
        let mut statement = connection
            .prepare("PRAGMA table_info(tasks)")
            .map_err(|error| error.to_string())?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()
            .map_err(|error| error.to_string())?;
        names
    };
    if !columns.contains("url") {
        return Err("legacy tasks table does not contain url".into());
    }
    let mut sql = String::from("SELECT url");
    let extras = [
        "id",
        "filename",
        "title",
        "task_type",
        "request_method",
        "total_bytes",
        "downloaded_bytes",
        "speed_limit_kib",
        "output_path",
        "status",
        "referer",
        "origin",
        "user_agent",
        "cookie",
        "request_headers",
        "expected_checksum",
        "concurrency",
    ];
    for name in extras {
        sql.push_str(", ");
        if columns.contains(name) {
            sql.push_str(name);
        } else {
            sql.push_str("NULL");
        }
    }
    sql.push_str(" FROM tasks ORDER BY rowid");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(LegacyTask {
                url: row.get::<_, String>(0).unwrap_or_default(),
                id: optional_string(row, 1),
                filename: optional_string(row, 2),
                title: optional_string(row, 3),
                task_type: optional_string(row, 4),
                request_method: optional_string(row, 5),
                total_bytes: optional_u64(row, 6),
                downloaded_bytes: optional_u64(row, 7).unwrap_or(0),
                speed_limit_kib: row.get::<_, i64>(8).ok(),
                output_path: row.get::<_, String>(9).ok().filter(|path| !path.is_empty()),
                status: optional_string(row, 10),
                referer: optional_string(row, 11),
                origin: optional_string(row, 12),
                user_agent: optional_string(row, 13),
                cookie: optional_string(row, 14),
                request_headers: optional_string(row, 15),
                checksum: row
                    .get::<_, String>(16)
                    .ok()
                    .filter(|value| !value.trim().is_empty()),
                concurrency: row.get::<_, i64>(17).ok(),
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())
}

fn optional_string(row: &rusqlite::Row<'_>, index: usize) -> String {
    row.get::<_, String>(index).unwrap_or_default()
}

fn optional_u64(row: &rusqlite::Row<'_>, index: usize) -> Option<u64> {
    row.get::<_, i64>(index).ok().and_then(
        |value| {
            if value > 0 {
                Some(value as u64)
            } else {
                None
            }
        },
    )
}

fn public_headers(row: &LegacyTask) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    let mut replay = serde_json::Map::new();
    if !row.referer.trim().is_empty() {
        replay.insert("referer".into(), Value::String(row.referer.clone()));
    }
    if !row.origin.trim().is_empty() {
        replay.insert("origin".into(), Value::String(row.origin.clone()));
    }
    if !row.user_agent.trim().is_empty() {
        replay.insert("user_agent".into(), Value::String(row.user_agent.clone()));
    }
    if !row.request_headers.trim().is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(&row.request_headers) {
            replay.insert("request_headers".into(), value);
        }
    }
    if !replay.is_empty() {
        apply_replay_json(&mut headers, &Value::Object(replay).to_string());
        headers.remove("Cookie");
        headers.remove("Authorization");
    }
    headers
}

fn store_row_credential(
    core: &mut PersistentCore,
    row: &LegacyTask,
) -> Result<Option<String>, String> {
    if row.cookie.trim().is_empty()
        && !row.request_headers.to_ascii_lowercase().contains("cookie")
        && !row
            .request_headers
            .to_ascii_lowercase()
            .contains("authorization")
    {
        return Ok(None);
    }
    let mut context = serde_json::Map::new();
    if !row.cookie.trim().is_empty() {
        context.insert("cookie".into(), Value::String(row.cookie.clone()));
    }
    if !row.referer.trim().is_empty() {
        context.insert("referer".into(), Value::String(row.referer.clone()));
    }
    if !row.origin.trim().is_empty() {
        context.insert("origin".into(), Value::String(row.origin.clone()));
    }
    if !row.user_agent.trim().is_empty() {
        context.insert("user_agent".into(), Value::String(row.user_agent.clone()));
    }
    if !row.request_headers.trim().is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(&row.request_headers) {
            context.insert("request_headers".into(), value);
        }
    }
    let json = Value::Object(context).to_string();
    let protected = if cfg!(windows) {
        CredentialVault.protect(&json)?
    } else {
        json
    };
    let credential_ref = if row.id.trim().is_empty() {
        format!("migrated-{:x}", fxhash(&row.url))
    } else {
        format!("migrated-{}", row.id)
    };
    core.store_mut()
        .store_credential(&credential_ref, &protected, "browser_replay")?;
    Ok(Some(credential_ref))
}

fn fxhash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn mapped_progress(row: &LegacyTask) -> (&'static str, &'static str, u64) {
    let status = row.status.trim().to_ascii_lowercase();
    match status.as_str() {
        "completed" | "done" => (
            "completed",
            "finished",
            row.downloaded_bytes.max(row.total_bytes.unwrap_or(0)),
        ),
        "failed" | "error" => ("failed", "finished", row.downloaded_bytes),
        "canceled" | "cancelled" => ("canceled", "finished", row.downloaded_bytes),
        "paused" => ("paused", "waiting", row.downloaded_bytes),
        "downloading" | "recording" | "merging" | "checking" | "probing" | "waiting" => {
            ("paused", "waiting", row.downloaded_bytes)
        }
        _ => ("queued", "waiting", row.downloaded_bytes),
    }
}

fn import_http_partial(
    config_path: &Path,
    db_path: &Path,
    row: &LegacyTask,
    task_id: &str,
    spec: &TaskSpec,
) {
    let Ok(paths) = TaskPaths::for_task(task_id, spec) else {
        return;
    };
    if paths.prepare().is_err() {
        return;
    }
    let Some(source_dir) = find_legacy_task_dir(config_path, db_path, row, spec) else {
        return;
    };
    copy_if_present(&source_dir.join("payload.downloading"), &paths.output);
    let ranges_dest = paths.progress.with_file_name("native-engine.ranges.json");
    if !copy_if_present(&source_dir.join("native-engine.ranges.json"), &ranges_dest) {
        if let Some(converted) = convert_http_resume(&source_dir.join("http-resume.json"), spec) {
            let _ = std::fs::write(ranges_dest, converted);
        }
    }
}

fn import_media_partial(
    config_path: &Path,
    db_path: &Path,
    row: &LegacyTask,
    task_id: &str,
    spec: &TaskSpec,
) {
    if !matches!(
        spec.resource_kind,
        ResourceKind::Hls | ResourceKind::Live | ResourceKind::Dash
    ) {
        return;
    }
    let Ok(paths) = TaskPaths::for_task(task_id, spec) else {
        return;
    };
    if paths.prepare().is_err() {
        return;
    }
    let Some(source_dir) = find_legacy_task_dir(config_path, db_path, row, spec) else {
        return;
    };
    let dest_dir = paths.task_dir();
    copy_tree(&source_dir.join("segments"), &dest_dir.join("segments"));
    copy_tree(&source_dir.join("maps"), &dest_dir.join("maps"));
    copy_tree(&source_dir.join("audio"), &dest_dir.join("audio"));
    copy_tree(&source_dir.join("subs"), &dest_dir.join("subs"));
    for name in [
        "vod_segments.json",
        "live_state.json",
        "dash_vod_segments.json",
        "local.m3u8",
        "init.mp4",
    ] {
        copy_if_present(&source_dir.join(name), &dest_dir.join(name));
    }
    for entry in std::fs::read_dir(&source_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("seg-") && name.ends_with(".m4s") {
            copy_if_present(&entry.path(), &dest_dir.join(name.as_ref()));
        }
    }
}

fn find_legacy_task_dir(
    config_path: &Path,
    db_path: &Path,
    row: &LegacyTask,
    spec: &TaskSpec,
) -> Option<PathBuf> {
    if row.id.trim().is_empty() {
        return None;
    }
    let mut roots = Vec::new();
    if let Some(parent) = db_path.parent() {
        roots.push(parent.to_path_buf());
    }
    if let Some(parent) = config_path.parent() {
        roots.push(parent.to_path_buf());
        roots.push(parent.join("Cache"));
    }
    if !spec.download_dir.trim().is_empty() {
        roots.push(PathBuf::from(&spec.download_dir));
    }
    if let Ok(temp) = std::env::var("HLS_V6_MIGRATE_TEMP") {
        roots.push(PathBuf::from(temp));
    }
    for root in roots {
        let candidate = root.join(".tasks").join(&row.id);
        if candidate.join("payload.downloading").is_file()
            || candidate.join("native-engine.ranges.json").is_file()
            || candidate.join("http-resume.json").is_file()
            || candidate.join("vod_segments.json").is_file()
            || candidate.join("live_state.json").is_file()
            || candidate.join("dash_vod_segments.json").is_file()
            || candidate.join("segments").is_dir()
        {
            return Some(candidate);
        }
    }
    None
}

fn resolve_legacy_path(source_file: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        source_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn copy_if_present(source: &Path, dest: &Path) -> bool {
    if !source.is_file() {
        return false;
    }
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::copy(source, dest).is_ok()
}

fn copy_tree(source: &Path, dest: &Path) {
    if !source.is_dir() {
        return;
    }
    let _ = std::fs::create_dir_all(dest);
    let Ok(entries) = std::fs::read_dir(source) else {
        return;
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            let _ = std::fs::copy(&from, &to);
        }
    }
}

fn convert_http_resume(path: &Path, spec: &TaskSpec) -> Option<String> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let items = value.get("ranges")?.as_array()?;
    let mut ranges = Vec::new();
    for item in items {
        let start = item.get("from").and_then(Value::as_u64)?;
        let current = item.get("current").and_then(Value::as_u64)?;
        if current > start {
            ranges.push(serde_json::json!([start, current - 1]));
        }
    }
    if ranges.is_empty() {
        return None;
    }
    Some(
        serde_json::json!({
            "version": 2,
            "resource_key": spec.url,
            "etag": spec.etag,
            "last_modified": spec.last_modified,
            "total": spec.expected_size.unwrap_or(0),
            "ranges": ranges,
        })
        .to_string(),
    )
}

fn classify_task(task_type: &str, url: &str) -> ResourceKind {
    match task_type.trim().to_ascii_lowercase().as_str() {
        "hls" | "m3u8" => ResourceKind::Hls,
        "dash" | "mpd" => ResourceKind::Dash,
        "live" => ResourceKind::Live,
        "ftp" | "ftps" => ResourceKind::Ftp,
        "sftp" => ResourceKind::Sftp,
        "torrent" | "magnet" | "bt" => ResourceKind::Torrent,
        "file" | "http" => ResourceKind::File,
        _ => crate::recognize::classify_url(url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PersistentCore;

    #[test]
    fn imports_legal_flag_and_skips_missing_db() {
        let dir = std::env::temp_dir().join(format!("hls-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.json");
        std::fs::write(
            &config,
            r#"{"legal_terms_accepted":true,"download_speed_limit_kib":512,"browser_takeover_enabled":false,"max_concurrent_tasks":5,"speed_schedule_enabled":true,"watch_dir":"D:\\torrents","browser_category_dirs":{"media":"E:\\Videos"},"default_cookie":"session=legacy"}"#,
        )
        .unwrap();
        let mut core = PersistentCore::in_memory().unwrap();
        let count = migrate_from_5x(&mut core, &config, &dir.join("missing.db")).unwrap();
        assert_eq!(count, 0);
        assert!(core
            .store()
            .setting_bool("legal_terms_accepted", false)
            .unwrap());
        assert_eq!(
            core.store()
                .setting_u64("download_speed_limit_kib", 0)
                .unwrap(),
            512
        );
        assert!(!core
            .store()
            .setting_bool("browser_takeover_enabled", true)
            .unwrap());
        assert_eq!(core.store().setting_u64("queue_max_active", 3).unwrap(), 5);
        assert!(core
            .store()
            .setting_bool("download_speed_schedule_enabled", false)
            .unwrap());
        assert_eq!(
            core.store()
                .setting_string("torrent_watch_dir", "")
                .unwrap(),
            "D:\\torrents"
        );
        assert!(core
            .store()
            .setting_string("browser_category_dirs", "")
            .unwrap()
            .contains("Videos"));
        let cookie_blob = core
            .store()
            .load_credential("settings:default-cookie")
            .unwrap()
            .unwrap();
        let cookie = CredentialVault
            .unprotect(&cookie_blob)
            .unwrap_or(cookie_blob);
        assert!(cookie.contains("session=legacy"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn imports_5x_task_rows_without_deleting_source() {
        let dir = std::env::temp_dir().join(format!(
            "hls-migrate-db-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("data.db");
        {
            let connection = rusqlite::Connection::open(&db).unwrap();
            connection
                .execute_batch(
                    r#"CREATE TABLE tasks (
                        id TEXT PRIMARY KEY,
                        task_type TEXT DEFAULT 'file',
                        title TEXT DEFAULT '',
                        url TEXT NOT NULL,
                        request_method TEXT DEFAULT 'GET',
                        filename TEXT DEFAULT '',
                        total_bytes INTEGER DEFAULT 0,
                        downloaded_bytes INTEGER DEFAULT 0,
                        speed_limit_kib INTEGER DEFAULT 0,
                        output_path TEXT DEFAULT '',
                        status TEXT DEFAULT 'queued',
                        cookie TEXT DEFAULT '',
                        referer TEXT DEFAULT ''
                    );
                    INSERT INTO tasks(id, task_type, title, url, filename, total_bytes, output_path, status, downloaded_bytes, cookie, referer)
                    VALUES ('t1', 'hls', 'Show', 'https://cdn.test/a.m3u8', 'a.mp4', 42, 'D:\dl\a.mp4', 'completed', 42, '', ''),
                           ('t2', 'file', 'Part', 'https://cdn.test/b.bin', 'b.bin', 100, 'D:\dl\b.bin', 'downloading', 40, 'sid=1', 'https://cdn.test/page');"#,
                )
                .unwrap();
        }
        let mut core = PersistentCore::in_memory().unwrap();
        let count = migrate_from_5x(&mut core, &dir.join("missing.json"), &db).unwrap();
        assert_eq!(count, 2);
        assert!(db.exists());
        let tasks = core.tasks();
        let done = tasks.iter().find(|task| task.filename == "a.mp4").unwrap();
        assert_eq!(done.resource_kind, crate::ResourceKind::Hls);
        assert_eq!(done.status, "completed");
        assert_eq!(done.downloaded_bytes, 42);
        let paused = tasks.iter().find(|task| task.filename == "b.bin").unwrap();
        assert_eq!(paused.status, "paused");
        assert_eq!(paused.downloaded_bytes, 40);
        let spec = core.task_spec(&paused.task_id).unwrap();
        assert_eq!(spec.expected_size, Some(100));
        assert_eq!(
            spec.headers.get("Referer").unwrap(),
            "https://cdn.test/page"
        );
        assert!(spec.credential_ref.as_ref().unwrap().contains("t2"));
        let blob = core
            .store()
            .load_credential(spec.credential_ref.as_ref().unwrap())
            .unwrap()
            .unwrap();
        let plain = crate::CredentialVault.unprotect(&blob).unwrap_or(blob);
        assert!(plain.contains("sid=1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn maybe_migrate_is_one_shot() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.store_mut().set_setting(MIGRATED_FLAG, true).unwrap();
        assert_eq!(maybe_migrate_from_5x(&mut core).unwrap(), 0);
    }

    #[test]
    fn copies_http_partial_and_converts_python_resume() {
        let dir = std::env::temp_dir().join(format!(
            "hls-migrate-part-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let task_dir = dir.join(".tasks").join("legacy-1");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(task_dir.join("payload.downloading"), b"hello-partial").unwrap();
        std::fs::write(
            task_dir.join("http-resume.json"),
            r#"{"ranges":[{"from":0,"to":99,"current":13}]}"#,
        )
        .unwrap();
        let db = dir.join("data.db");
        let output = dir.join("out").join("c.bin");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        {
            let connection = rusqlite::Connection::open(&db).unwrap();
            connection
                .execute_batch(&format!(
                    "CREATE TABLE tasks (
                        id TEXT PRIMARY KEY,
                        url TEXT NOT NULL,
                        filename TEXT DEFAULT '',
                        status TEXT DEFAULT 'paused',
                        downloaded_bytes INTEGER DEFAULT 0,
                        total_bytes INTEGER DEFAULT 0,
                        output_path TEXT DEFAULT ''
                    );
                    INSERT INTO tasks(id, url, filename, status, downloaded_bytes, total_bytes, output_path)
                    VALUES ('legacy-1', 'https://cdn.test/c.bin', 'c.bin', 'paused', 13, 100, '{}');",
                    output.to_string_lossy().replace('\\', "\\\\")
                ))
                .unwrap();
        }
        let mut core = PersistentCore::in_memory().unwrap();
        migrate_from_5x(&mut core, &dir.join("missing.json"), &db).unwrap();
        let task = &core.tasks()[0];
        let spec = core.task_spec(&task.task_id).unwrap();
        let paths = TaskPaths::for_task(&task.task_id, spec).unwrap();
        assert_eq!(std::fs::read(&paths.output).unwrap(), b"hello-partial");
        let ranges =
            std::fs::read_to_string(paths.progress.with_file_name("native-engine.ranges.json"))
                .unwrap();
        assert!(ranges.contains("[0,12]"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn copies_hls_segment_checkpoint() {
        let dir = std::env::temp_dir().join(format!(
            "hls-migrate-media-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let task_dir = dir.join(".tasks").join("legacy-hls");
        std::fs::create_dir_all(task_dir.join("segments")).unwrap();
        std::fs::write(task_dir.join("segments").join("000000.seg"), b"TS").unwrap();
        std::fs::write(
            task_dir.join("vod_segments.json"),
            r#"{"version":1,"segments":{}}"#,
        )
        .unwrap();
        let db = dir.join("data.db");
        let output = dir.join("out").join("show.mp4");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        {
            let connection = rusqlite::Connection::open(&db).unwrap();
            connection
                .execute_batch(&format!(
                    "CREATE TABLE tasks (
                        id TEXT PRIMARY KEY,
                        task_type TEXT DEFAULT 'hls',
                        url TEXT NOT NULL,
                        filename TEXT DEFAULT '',
                        status TEXT DEFAULT 'paused',
                        downloaded_bytes INTEGER DEFAULT 0,
                        output_path TEXT DEFAULT ''
                    );
                    INSERT INTO tasks(id, task_type, url, filename, status, downloaded_bytes, output_path)
                    VALUES ('legacy-hls', 'hls', 'https://cdn.test/a.m3u8', 'show.mp4', 'paused', 10, '{}');",
                    output.to_string_lossy().replace('\\', "\\\\")
                ))
                .unwrap();
        }
        std::env::set_var("HLS_V6_MIGRATE_TEMP", &dir);
        let mut core = PersistentCore::in_memory().unwrap();
        migrate_from_5x(&mut core, &dir.join("missing.json"), &db).unwrap();
        std::env::remove_var("HLS_V6_MIGRATE_TEMP");
        let task = core
            .tasks()
            .into_iter()
            .find(|item| item.filename == "show.mp4")
            .unwrap();
        let spec = core.task_spec(&task.task_id).unwrap();
        let paths = TaskPaths::for_task(&task.task_id, spec).unwrap();
        assert_eq!(
            std::fs::read(paths.task_dir().join("segments").join("000000.seg")).unwrap(),
            b"TS"
        );
        assert!(paths.task_dir().join("vod_segments.json").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }
}
