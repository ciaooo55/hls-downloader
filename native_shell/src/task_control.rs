//! In-process control and progress channel for running tasks.
//!
//! Core and its engine threads live in one process, yet pause/cancel and
//! progress used to travel through small files that every range worker opened
//! every 50 ms. This module keeps the hot values in memory:
//!
//! * **Control tokens.** While a task runs, the worker registers its control
//!   file path. Readers get the state from a shared atomic; Core command
//!   handlers update the atomic *and* still write the control file (crash
//!   recovery, external tools). A registered token re-reads the file at most
//!   once per [`FILE_RECONCILE_INTERVAL_MS`], so a value written by something
//!   other than [`set`] is still honoured, just not within 50 ms.
//! * **Progress mirror.** Progress JSON files inside a registered task
//!   directory are kept in memory and written to disk at most once per
//!   [`PROGRESS_FILE_INTERVAL_MS`]; in-process readers use the memory copy.
//!   When the last registration of a directory ends, any value newer than the
//!   file is flushed (only if the file still exists, so cleanup that removed
//!   it is not undone).
//!
//! Paths that are not registered behave exactly as before: every read and
//! write goes to the file.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

pub(crate) const RUN: u8 = 0;
pub(crate) const PAUSE: u8 = 1;
pub(crate) const CANCEL: u8 = 2;
/// A value the token cannot represent (for example a transient empty file or
/// an engine-private word). Readers fall back to the file for it.
pub(crate) const OTHER: u8 = 3;

/// How often a registered token re-reads its control file.
pub(crate) const FILE_RECONCILE_INTERVAL_MS: u64 = 1000;
/// Minimum spacing of progress file writes inside a registered task directory.
pub(crate) const PROGRESS_FILE_INTERVAL_MS: u64 = 1000;

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    // +1 so that "never checked" (0) is always older than the first reading.
    epoch().elapsed().as_millis() as u64 + 1
}

pub(crate) fn parse(text: &str) -> u8 {
    match text.trim().to_ascii_lowercase().as_str() {
        "run" => RUN,
        "pause" => PAUSE,
        "cancel" => CANCEL,
        _ => OTHER,
    }
}

pub(crate) fn state_name(state: u8) -> &'static str {
    match state {
        PAUSE => "pause",
        CANCEL => "cancel",
        _ => "run",
    }
}

struct ControlEntry {
    state: AtomicU8,
    /// The last value known to be in the file (written by [`set`] or read by
    /// reconciliation). A file value different from this one came from
    /// another writer and is adopted.
    file_seen: AtomicU8,
    last_file_check_ms: AtomicU64,
    file_reads: AtomicU64,
    refs: AtomicU64,
}

fn controls() -> &'static RwLock<HashMap<PathBuf, Arc<ControlEntry>>> {
    static CONTROLS: OnceLock<RwLock<HashMap<PathBuf, Arc<ControlEntry>>>> = OnceLock::new();
    CONTROLS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Keeps a control token registered while a task runs.
#[must_use]
pub(crate) struct ControlRegistration {
    path: PathBuf,
}

impl Drop for ControlRegistration {
    fn drop(&mut self) {
        let mut map = controls()
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let last = map
            .get(&self.path)
            .is_some_and(|entry| entry.refs.fetch_sub(1, Ordering::SeqCst) <= 1);
        if last {
            map.remove(&self.path);
        }
        // Every registration retained the directory, including nested ones.
        // Keep registration serialized until the last progress flush finishes.
        if let Some(dir) = self.path.parent() {
            release_progress_dir(dir);
        }
    }
}

/// Registers the control file of a running task. The token starts from the
/// file's current value (a missing file means run). Registrations nest: the
/// token lives until the last one is dropped.
pub(crate) fn register(path: &Path) -> ControlRegistration {
    let mut map = controls()
        .write()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(entry) = map.get(path) {
        entry.refs.fetch_add(1, Ordering::SeqCst);
    } else {
        let initial = match fs::read_to_string(path) {
            Ok(text) => parse(&text),
            Err(_) => RUN,
        };
        map.insert(
            path.to_path_buf(),
            Arc::new(ControlEntry {
                state: AtomicU8::new(initial),
                file_seen: AtomicU8::new(initial),
                last_file_check_ms: AtomicU64::new(now_ms()),
                file_reads: AtomicU64::new(1),
                refs: AtomicU64::new(1),
            }),
        );
    }
    if let Some(dir) = path.parent() {
        retain_progress_dir(dir);
    }
    ControlRegistration {
        path: path.to_path_buf(),
    }
}

/// Writes a control value: the in-memory token first (so running workers see
/// it on their next poll), then the file. The registry lock is held across
/// both so a concurrent [`register`] cannot read the file between them.
pub(crate) fn set(path: &Path, value: &str) -> Result<(), String> {
    let map = controls()
        .write()
        .unwrap_or_else(|error| error.into_inner());
    let parsed = parse(value);
    if let Some(entry) = map.get(path) {
        entry.state.store(parsed, Ordering::SeqCst);
        entry.file_seen.store(parsed, Ordering::SeqCst);
    }
    fs::write(path, value).map_err(|error| format!("write task control: {error}"))
}

fn current_at(path: &Path, now: u64) -> Option<u8> {
    // A stale file read must not overwrite a newer pause/cancel from set().
    let map = controls().read().unwrap_or_else(|error| error.into_inner());
    let entry = map.get(path)?;
    let last = entry.last_file_check_ms.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= FILE_RECONCILE_INTERVAL_MS
        && entry
            .last_file_check_ms
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        entry.file_reads.fetch_add(1, Ordering::Relaxed);
        if let Ok(text) = fs::read_to_string(path) {
            // An empty read is a writer between truncate and write; skip it.
            if !text.trim().is_empty() {
                let parsed = parse(&text);
                if parsed != entry.file_seen.load(Ordering::SeqCst) {
                    entry.file_seen.store(parsed, Ordering::SeqCst);
                    entry.state.store(parsed, Ordering::SeqCst);
                }
            }
        }
    }
    match entry.state.load(Ordering::SeqCst) {
        OTHER => None,
        state => Some(state),
    }
}

/// The in-memory control state of a registered task, or `None` when the path
/// is not registered (or holds a value only the file can express).
pub(crate) fn current(path: &Path) -> Option<u8> {
    current_at(path, now_ms())
}

/// Reads a control value as a lowercase word, preferring the token and
/// falling back to the file. A missing file reads as `"run"`.
pub(crate) fn read_word(path: &Path) -> String {
    if let Some(state) = current(path) {
        return state_name(state).to_string();
    }
    fs::read_to_string(path)
        .unwrap_or_else(|_| "run".into())
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
pub(crate) fn file_reads(path: &Path) -> Option<u64> {
    let map = controls().read().unwrap_or_else(|error| error.into_inner());
    map.get(path)
        .map(|entry| entry.file_reads.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Progress mirror
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ProgressDir {
    refs: u64,
    files: HashMap<PathBuf, ProgressFile>,
}

struct ProgressFile {
    text: String,
    written_ms: u64,
    dirty: bool,
}

fn progress_dirs() -> &'static Mutex<HashMap<PathBuf, ProgressDir>> {
    static DIRS: OnceLock<Mutex<HashMap<PathBuf, ProgressDir>>> = OnceLock::new();
    DIRS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn retain_progress_dir(dir: &Path) {
    let mut dirs = progress_dirs()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    dirs.entry(dir.to_path_buf()).or_default().refs += 1;
}

fn release_progress_dir(dir: &Path) {
    let mut dirs = progress_dirs()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let pending = {
        let Some(entry) = dirs.get_mut(dir) else {
            return;
        };
        entry.refs = entry.refs.saturating_sub(1);
        if entry.refs > 0 {
            return;
        }
        dirs.remove(dir)
            .map(|entry| entry.files)
            .unwrap_or_default()
    };
    for (path, file) in pending {
        // A cleanup that already removed the file must not be undone.
        if file.dirty && path.exists() {
            write_file_atomically(&path, &file.text);
        }
    }
}

fn write_file_atomically(path: &Path, text: &str) {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    if fs::write(&tmp, text).is_ok() && crate::atomic_replace::replace_file(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Publishes a progress file. Inside a registered task directory the value is
/// kept in memory and the file is rewritten at most once per
/// [`PROGRESS_FILE_INTERVAL_MS`] unless `force` is set (terminal states).
/// Elsewhere the file is written every time, as before.
pub(crate) fn publish_progress_file(path: &Path, text: &str, force: bool) {
    publish_progress_file_at(path, text, force, now_ms());
}

fn publish_progress_file_at(path: &Path, text: &str, force: bool, now: u64) {
    // Serialize mirror publication and the atomic replacement (shared .tmp path).
    let mut dirs = progress_dirs()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let write = {
        match path.parent().and_then(|dir| dirs.get_mut(dir)) {
            None => true,
            Some(dir) => {
                let file = dir
                    .files
                    .entry(path.to_path_buf())
                    .or_insert_with(|| ProgressFile {
                        text: String::new(),
                        written_ms: 0,
                        dirty: false,
                    });
                file.text.clear();
                file.text.push_str(text);
                let due = force
                    || file.written_ms == 0
                    || now.saturating_sub(file.written_ms) >= PROGRESS_FILE_INTERVAL_MS;
                if due {
                    file.written_ms = now;
                }
                file.dirty = !due;
                due
            }
        }
    };
    if write {
        write_file_atomically(path, text);
    }
}

/// Reads a progress file, preferring the in-memory copy of a registered task.
pub(crate) fn read_progress_file(path: &Path) -> Option<String> {
    {
        let dirs = progress_dirs()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(file) = path
            .parent()
            .and_then(|dir| dirs.get(dir))
            .and_then(|dir| dir.files.get(path))
        {
            return Some(file.text.clone());
        }
    }
    fs::read_to_string(path).ok()
}

/// Drops the in-memory copy of a progress file that is being removed.
pub(crate) fn forget_progress_file(path: &Path) {
    let mut dirs = progress_dirs()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(dir) = path.parent().and_then(|dir| dirs.get_mut(dir)) {
        dir.files.remove(path);
    }
}

/// Drops every in-memory progress copy of a task directory (task cleanup).
pub(crate) fn forget_progress_dir(dir: &Path) {
    {
        let mut dirs = progress_dirs()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(dir) = dirs.get_mut(dir) {
            dir.files.clear();
        }
    }
    // 同一任务目录的检查点缓存与播放字节优先级内存值一并丢弃。
    crate::range_cache::forget_dir(dir);
    crate::media::preview::forget_byte_priority(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hls-task-control-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unregistered_paths_read_the_file() {
        let dir = temp_dir("unregistered");
        let control = dir.join("control");
        assert_eq!(current(&control), None);
        assert_eq!(read_word(&control), "run");
        fs::write(&control, "pause").unwrap();
        assert_eq!(read_word(&control), "pause");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn set_updates_token_and_file_and_hot_reads_skip_the_file() {
        let dir = temp_dir("set");
        let control = dir.join("control");
        fs::write(&control, "run").unwrap();
        let registration = register(&control);
        let start = now_ms();
        for _ in 0..1000 {
            assert_eq!(current_at(&control, start), Some(RUN));
        }
        // The registration read is the only file access inside one interval.
        assert_eq!(file_reads(&control), Some(1));
        set(&control, "pause").unwrap();
        assert_eq!(current_at(&control, start), Some(PAUSE));
        assert_eq!(fs::read_to_string(&control).unwrap(), "pause");
        set(&control, "cancel").unwrap();
        assert_eq!(read_word(&control), "cancel");
        drop(registration);
        assert_eq!(current(&control), None);
        assert_eq!(read_word(&control), "cancel");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn external_file_writes_are_adopted_at_most_once_per_interval() {
        let dir = temp_dir("external");
        let control = dir.join("control");
        fs::write(&control, "run").unwrap();
        let _registration = register(&control);
        let start = now_ms();
        fs::write(&control, "pause").unwrap();
        // Within the interval the token is authoritative.
        assert_eq!(current_at(&control, start + 10), Some(RUN));
        let later = start + FILE_RECONCILE_INTERVAL_MS + 1;
        assert_eq!(current_at(&control, later), Some(PAUSE));
        assert_eq!(file_reads(&control), Some(2));
        // An empty (mid-write) file never flips the token.
        fs::write(&control, "").unwrap();
        let later = later + FILE_RECONCILE_INTERVAL_MS + 1;
        assert_eq!(current_at(&control, later), Some(PAUSE));
        // A word the token cannot represent falls back to the file.
        fs::write(&control, "finish").unwrap();
        let later = later + FILE_RECONCILE_INTERVAL_MS + 1;
        assert_eq!(current_at(&control, later), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_readers_use_the_token_not_the_file() {
        let dir = temp_dir("engine");
        let control = dir.join("control");
        fs::write(&control, "run").unwrap();
        let _registration = register(&control);
        set(&control, "pause").unwrap();
        // A stale file inside the reconcile window does not override the token:
        // the engines read memory, not the file.
        fs::write(&control, "pause").unwrap();
        assert_eq!(
            crate::http_engine::read_control(&control),
            crate::http_engine::Control::Pause
        );
        set(&control, "run").unwrap();
        assert_eq!(
            crate::http_engine::read_control(&control),
            crate::http_engine::Control::Run
        );
        set(&control, "cancel").unwrap();
        assert_eq!(read_word(&control), "cancel");
        assert_eq!(
            crate::http_engine::read_control(&control),
            crate::http_engine::Control::Cancel
        );
        // Registration read, plus at most one reconcile if this test was slow.
        assert!(file_reads(&control).unwrap() <= 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn registration_starts_from_the_file_and_nests() {
        let dir = temp_dir("nest");
        let control = dir.join("control");
        fs::write(&control, "pause").unwrap();
        let first = register(&control);
        let second = register(&control);
        let progress = dir.join("progress.json");
        let start = now_ms();
        publish_progress_file_at(&progress, "first", false, start);
        publish_progress_file_at(&progress, "latest", false, start + 1);
        assert_eq!(current(&control), Some(PAUSE));
        drop(first);
        assert_eq!(current(&control), Some(PAUSE));
        assert_eq!(fs::read_to_string(&progress).unwrap(), "first");
        drop(second);
        assert_eq!(current(&control), None);
        assert_eq!(fs::read_to_string(&progress).unwrap(), "latest");
        fs::write(&progress, "external").unwrap();
        assert_eq!(read_progress_file(&progress).as_deref(), Some("external"));
        let missing = dir.join("missing-control");
        let registration = register(&missing);
        assert_eq!(current(&missing), Some(RUN));
        drop(registration);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn progress_files_are_throttled_inside_registered_tasks_and_flushed_on_release() {
        let dir = temp_dir("progress");
        let control = dir.join("control");
        let progress = dir.join("progress.json");
        let registration = register(&control);
        let start = now_ms();
        publish_progress_file_at(&progress, "{\"downloaded\":1}", false, start);
        assert_eq!(fs::read_to_string(&progress).unwrap(), "{\"downloaded\":1}");
        publish_progress_file_at(&progress, "{\"downloaded\":2}", false, start + 200);
        // File is still the first value; in-process readers see the newest.
        assert_eq!(fs::read_to_string(&progress).unwrap(), "{\"downloaded\":1}");
        assert_eq!(
            read_progress_file(&progress).as_deref(),
            Some("{\"downloaded\":2}")
        );
        publish_progress_file_at(
            &progress,
            "{\"downloaded\":3}",
            false,
            start + PROGRESS_FILE_INTERVAL_MS + 1,
        );
        assert_eq!(fs::read_to_string(&progress).unwrap(), "{\"downloaded\":3}");
        publish_progress_file_at(&progress, "{\"downloaded\":4}", true, start + 1100);
        assert_eq!(fs::read_to_string(&progress).unwrap(), "{\"downloaded\":4}");
        publish_progress_file_at(&progress, "{\"downloaded\":5}", false, start + 1200);
        drop(registration);
        // Release flushes the unwritten value and returns to plain file reads.
        assert_eq!(fs::read_to_string(&progress).unwrap(), "{\"downloaded\":5}");
        fs::write(&progress, "{\"downloaded\":6}").unwrap();
        assert_eq!(
            read_progress_file(&progress).as_deref(),
            Some("{\"downloaded\":6}")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removed_progress_files_are_not_recreated() {
        let dir = temp_dir("progress-removed");
        let control = dir.join("control");
        let active = dir.join("native-engine.active.json");
        let registration = register(&control);
        let start = now_ms();
        publish_progress_file_at(&active, "{\"ranges\":[]}", false, start);
        publish_progress_file_at(&active, "{\"ranges\":[[0,1]]}", false, start + 10);
        forget_progress_file(&active);
        fs::remove_file(&active).unwrap();
        assert_eq!(read_progress_file(&active), None);
        publish_progress_file_at(&active, "{\"ranges\":[[0,2]]}", false, start + 20);
        fs::remove_file(&active).unwrap();
        drop(registration);
        assert!(!active.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unregistered_progress_writes_go_straight_to_disk() {
        let dir = temp_dir("progress-plain");
        let progress = dir.join("progress.json");
        publish_progress_file(&progress, "a", false);
        publish_progress_file(&progress, "b", false);
        assert_eq!(fs::read_to_string(&progress).unwrap(), "b");
        assert_eq!(read_progress_file(&progress).as_deref(), Some("b"));
        fs::remove_dir_all(dir).unwrap();
    }
}
