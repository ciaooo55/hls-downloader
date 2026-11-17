//! In-memory view of the durable range checkpoint (`native-engine.ranges.json`).
//!
//! The HTTP engine still writes and syncs the checkpoint file exactly as
//! before; this module only spares the *readers* (Core progress painting under
//! the Core lock, playback availability checks, the growing-file server) from
//! opening and parsing the file on every progress tick.
//!
//! * A value written in-process ([`record_written`]) is trusted for
//!   [`TRUST_WINDOW_MS`]; after that the file is re-validated by metadata
//!   (length + modification time) — a stat, not a read — at most that often.
//! * A value that was read from disk is re-validated by metadata on every
//!   lookup, so a file changed by anything else is picked up on the next read.
//! * A missing file drops the entry. Cleanup paths call [`remove`] /
//!   [`forget_dir`] so a removed checkpoint is never served from memory.
//!
//! The cache is bounded ([`MAX_ENTRIES`]); the oldest entry is evicted first.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

/// File name of the durable HTTP range checkpoint.
pub(crate) const FILE_NAME: &str = "native-engine.ranges.json";
/// How long an in-process write is served without touching the file.
pub(crate) const TRUST_WINDOW_MS: u64 = 1000;
const MAX_ENTRIES: usize = 512;

/// Parsed checkpoint. `ranges` are the raw inclusive `[start, end]` pairs
/// (pairs with `end < start` dropped), in file order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RangeCheckpointView {
    pub version: Option<u64>,
    pub total: Option<u64>,
    pub ranges: Vec<(u64, u64)>,
}

impl RangeCheckpointView {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let items = value.get("ranges")?.as_array()?;
        let ranges = items
            .iter()
            .filter_map(|item| {
                let pair = item.as_array()?;
                Some((pair.first()?.as_u64()?, pair.get(1)?.as_u64()?))
            })
            .filter(|(start, end)| end >= start)
            .collect();
        Some(Self {
            version: value.get("version").and_then(serde_json::Value::as_u64),
            total: value.get("total").and_then(serde_json::Value::as_u64),
            ranges,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

struct Entry {
    view: Option<Arc<RangeCheckpointView>>,
    stamp: Stamp,
    trusted_until: Option<Instant>,
    touched: Instant,
}

#[cfg(test)]
#[derive(Default)]
struct Stats {
    disk_reads: u64,
}

struct Cache {
    entries: HashMap<PathBuf, Entry>,
    #[cfg(test)]
    stats: HashMap<PathBuf, Stats>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            entries: HashMap::new(),
            #[cfg(test)]
            stats: HashMap::new(),
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    cache().lock().unwrap_or_else(|error| error.into_inner())
}

fn evict_if_full(cache: &mut Cache) {
    if cache.entries.len() < MAX_ENTRIES {
        return;
    }
    if let Some(oldest) = cache
        .entries
        .iter()
        .min_by_key(|(_, entry)| entry.touched)
        .map(|(path, _)| path.clone())
    {
        cache.entries.remove(&oldest);
        #[cfg(test)]
        cache.stats.remove(&oldest);
    }
}

/// Records a checkpoint the engine has just written (after the atomic
/// replace succeeded). `text` is exactly what was written.
pub(crate) fn record_written(path: &Path, text: &str) {
    let mut cache = lock();
    let Ok(metadata) = fs::metadata(path) else {
        cache.entries.remove(path);
        #[cfg(test)]
        cache.stats.remove(path);
        return;
    };
    let view = RangeCheckpointView::parse(text).map(Arc::new);
    let now = Instant::now();
    if !cache.entries.contains_key(path) {
        evict_if_full(&mut cache);
    }
    cache.entries.insert(
        path.to_path_buf(),
        Entry {
            view,
            stamp: Stamp::of(&metadata),
            trusted_until: Some(now + std::time::Duration::from_millis(TRUST_WINDOW_MS)),
            touched: now,
        },
    );
}

/// The parsed checkpoint at `path`, or `None` when it is missing or invalid.
pub(crate) fn load(path: &Path) -> Option<Arc<RangeCheckpointView>> {
    let now = Instant::now();
    // Serialize cache misses with record_written/forget. Otherwise an older
    // disk read can replace a newer published checkpoint after the lock is retaken.
    let mut cache = lock();
    if let Some(entry) = cache.entries.get_mut(path) {
        if entry.trusted_until.is_some_and(|until| now < until) {
            entry.touched = now;
            return entry.view.clone();
        }
    }
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => {
            cache.entries.remove(path);
            #[cfg(test)]
            cache.stats.remove(path);
            return None;
        }
    };
    let stamp = Stamp::of(&metadata);
    if let Some(entry) = cache.entries.get_mut(path) {
        if entry.stamp == stamp {
            entry.touched = now;
            if entry.trusted_until.is_some() {
                entry.trusted_until = Some(now + std::time::Duration::from_millis(TRUST_WINDOW_MS));
            }
            return entry.view.clone();
        }
    }
    let text = fs::read_to_string(path).ok();
    #[cfg(test)]
    {
        cache
            .stats
            .entry(path.to_path_buf())
            .or_default()
            .disk_reads += 1;
    }
    let Some(text) = text else {
        cache.entries.remove(path);
        #[cfg(test)]
        cache.stats.remove(path);
        return None;
    };
    let view = RangeCheckpointView::parse(&text).map(Arc::new);
    if !cache.entries.contains_key(path) {
        evict_if_full(&mut cache);
    }
    cache.entries.insert(
        path.to_path_buf(),
        Entry {
            view: view.clone(),
            stamp,
            trusted_until: None,
            touched: now,
        },
    );
    view
}

/// Loads the checkpoint of a task directory.
pub(crate) fn load_in(dir: &Path) -> Option<Arc<RangeCheckpointView>> {
    load(&dir.join(FILE_NAME))
}

/// Drops every cached checkpoint inside `dir` (task cleanup).
pub(crate) fn forget_dir(dir: &Path) {
    let mut cache = lock();
    cache.entries.retain(|path, _| path.parent() != Some(dir));
    #[cfg(test)]
    cache.stats.retain(|path, _| path.parent() != Some(dir));
}

/// Removes the checkpoint file and its cached copy.
pub(crate) fn remove(path: &Path) -> std::io::Result<()> {
    let mut cache = lock();
    cache.entries.remove(path);
    #[cfg(test)]
    cache.stats.remove(path);
    fs::remove_file(path)
}

#[cfg(test)]
pub(crate) fn disk_reads(path: &Path) -> u64 {
    lock().stats.get(path).map_or(0, |stats| stats.disk_reads)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hls-range-cache-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn repeated_reads_of_an_unchanged_file_parse_it_once() {
        let dir = temp_dir("unchanged");
        let path = dir.join(FILE_NAME);
        fs::write(&path, r#"{"version":2,"total":10,"ranges":[[0,3],[5,4]]}"#).unwrap();
        for _ in 0..20 {
            let view = load(&path).unwrap();
            assert_eq!(view.version, Some(2));
            assert_eq!(view.total, Some(10));
            assert_eq!(view.ranges, vec![(0, 3)]);
        }
        assert_eq!(disk_reads(&path), 1);
        forget_dir(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn external_rewrites_and_removal_are_seen_on_the_next_read() {
        let dir = temp_dir("external");
        let path = dir.join(FILE_NAME);
        fs::write(&path, r#"{"ranges":[[0,3]]}"#).unwrap();
        assert_eq!(load(&path).unwrap().ranges, vec![(0, 3)]);
        fs::write(&path, r#"{"ranges":[[0,3],[8,9]]}"#).unwrap();
        assert_eq!(load(&path).unwrap().ranges, vec![(0, 3), (8, 9)]);
        fs::remove_file(&path).unwrap();
        assert!(load(&path).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn in_process_writes_are_served_without_touching_the_file() {
        let dir = temp_dir("written");
        let path = dir.join(FILE_NAME);
        let text = r#"{"version":2,"total":100,"ranges":[[0,49]]}"#;
        fs::write(&path, text).unwrap();
        record_written(&path, text);
        for _ in 0..10 {
            assert_eq!(load(&path).unwrap().ranges, vec![(0, 49)]);
        }
        assert_eq!(disk_reads(&path), 0);
        // A newer in-process write replaces the view at once.
        let text = r#"{"version":2,"total":100,"ranges":[[0,99]]}"#;
        fs::write(&path, text).unwrap();
        record_written(&path, text);
        assert_eq!(load(&path).unwrap().ranges, vec![(0, 99)]);
        // Removal through the module never serves the stale value.
        remove(&path).unwrap();
        assert!(load(&path).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forget_dir_drops_the_task_entries_only() {
        let first = temp_dir("dir-a");
        let second = temp_dir("dir-b");
        let a = first.join(FILE_NAME);
        let b = second.join(FILE_NAME);
        for path in [&a, &b] {
            let text = r#"{"ranges":[[0,1]]}"#;
            fs::write(path, text).unwrap();
            record_written(path, text);
        }
        forget_dir(&first);
        fs::remove_file(&a).unwrap();
        assert!(load(&a).is_none());
        assert!(load(&b).is_some());
        assert_eq!(disk_reads(&b), 0);
        forget_dir(&second);
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }
}
