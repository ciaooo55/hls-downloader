//! 下载器与本地播放器共用的分片就绪标记和播放优先级。

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// 两轨按播放时间领取分片，避免先下载整条音轨；未知时长仍按分片交错。
pub(crate) fn segment_order(primary: &[f64], audio: &[f64]) -> Vec<(bool, usize)> {
    let mut order = Vec::with_capacity(primary.len() + audio.len());
    let (mut video_index, mut audio_index) = (0, 0);
    let (mut video_time, mut audio_time) = (0.0, 0.0);
    while video_index < primary.len() || audio_index < audio.len() {
        let is_audio =
            audio_index < audio.len() && (video_index == primary.len() || audio_time < video_time);
        let (durations, index, time) = if is_audio {
            (audio, &mut audio_index, &mut audio_time)
        } else {
            (primary, &mut video_index, &mut video_time)
        };
        let duration = durations[*index];
        order.push((is_audio, *index));
        *time += if duration.is_finite() && duration > 0.0 {
            duration
        } else {
            1.0
        };
        *index += 1;
    }
    order
}

pub(crate) fn relative_file(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(relative.to_string_lossy().replace('\\', "/"))
}

const READY_DIR: &str = "playback-ready";
const READY_INDEX_FILE: &str = "index.tsv";
const READY_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const READY_FLUSH_BATCH: usize = 64;
const READY_MAX_ENTRIES: usize = 256;

struct ReadyTaskState {
    /// Authoritative in-memory segment readiness (`relative_posix -> byte_len`).
    /// Cold-initialized once from `dash_vod_segments.json`, legacy per-segment marker files,
    /// and `playback-ready/index.tsv`; kept up to date by `mark_ready` and `reset_ready`.
    segments: HashMap<String, u64>,
    /// Cold-loaded crash-recovery slots from `vod_segments.json` (which indexes by slot number
    /// without storing file extensions). Never re-read from disk on hot `is_ready` misses, and
    /// immediately pruned on `reset_ready` / superseded on `mark_ready`.
    hls_checkpoint_slots: HashMap<usize, u64>,
    /// Cold-loaded crash-recovery slots from `audio/vod_segments.json`.
    audio_checkpoint_slots: HashMap<usize, u64>,
    marker_dir_created: bool,
    /// Monotonic counter bumped on every in-memory mutation of `segments` so concurrent
    /// flushes never clear dirty mutations landed while disk I/O was in flight.
    generation: u64,
    flushed_generation: u64,
    dirty_ops: usize,
    last_flush: Instant,
    checked: Instant,
    /// Per-task flush lock acquired strictly AFTER releasing `ready_segments()` so disk I/O
    /// and Windows atomic-replace retries never block unrelated tasks.
    flush_lock: Arc<Mutex<()>>,
    #[cfg(test)]
    disk_loads: u64,
    #[cfg(test)]
    disk_flushes: u64,
}

impl ReadyTaskState {
    fn expected_size(&self, relative: &str) -> Option<u64> {
        if let Some(&size) = self.segments.get(relative) {
            return Some(size);
        }
        let (is_audio, slot) = hls_slot_key(relative)?;
        let slots = if is_audio {
            &self.audio_checkpoint_slots
        } else {
            &self.hls_checkpoint_slots
        };
        slots.get(&slot).copied()
    }

    fn clear_checkpoint_slot(&mut self, relative: &str) {
        if let Some((is_audio, slot)) = hls_slot_key(relative) {
            if is_audio {
                self.audio_checkpoint_slots.remove(&slot);
            } else {
                self.hls_checkpoint_slots.remove(&slot);
            }
        }
    }
}

fn ready_segments() -> &'static Mutex<HashMap<PathBuf, ReadyTaskState>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, ReadyTaskState>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn valid_relative_str(relative: &str) -> Option<String> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(relative.replace('\\', "/"))
}

fn hls_slot_key(relative: &str) -> Option<(bool, usize)> {
    let (is_audio, filename) = if let Some(rest) = relative.strip_prefix("segments/") {
        (false, rest)
    } else {
        (true, relative.strip_prefix("audio/segments/")?)
    };
    if filename.contains('/') {
        return None;
    }
    let (stem, _ext) = filename.split_once('.')?;
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let slot: usize = stem.parse().ok()?;
    Some((is_audio, slot))
}

fn load_ready_index_from_disk(marker_dir: &Path, segments: &mut HashMap<String, u64>) {
    let Ok(text) = fs::read_to_string(marker_dir.join(READY_INDEX_FILE)) else {
        return;
    };
    for line in text.lines() {
        let Some((raw_rel, raw_size)) = line.split_once('\t') else {
            continue;
        };
        let Some(rel) = valid_relative_str(raw_rel.trim()) else {
            continue;
        };
        if let Ok(size) = raw_size.trim().parse::<u64>() {
            if size > 0 {
                segments.insert(rel, size);
            }
        }
    }
}

fn read_legacy_marker_file(path: &Path) -> Option<u64> {
    use std::io::Read;
    fs::File::open(path).ok().and_then(|file| {
        let mut size = String::new();
        file.take(21).read_to_string(&mut size).ok()?;
        let trimmed = size.trim();
        (trimmed.len() <= 20)
            .then(|| trimmed.parse::<u64>().ok())
            .flatten()
            .filter(|&parsed| parsed > 0)
    })
}

fn load_legacy_markers_from_dir(
    marker_root: &Path,
    dir: &Path,
    depth: usize,
    segments: &mut HashMap<String, u64>,
) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let tmp_prefix = format!(".{READY_INDEX_FILE}");
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            load_legacy_markers_from_dir(marker_root, &path, depth + 1, segments);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if depth == 0 {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == READY_INDEX_FILE || name.starts_with(&tmp_prefix) {
                continue;
            }
        }
        let Some(rel) = relative_file(marker_root, &path) else {
            continue;
        };
        if let Some(size) = read_legacy_marker_file(&path) {
            segments.insert(rel, size);
        }
    }
}

/// Crash-recovery cold loader for `dash_vod_segments.json`: reconstructs verified DASH segment
/// sizes if `index.tsv` missed a dirty tail batch before an abrupt exit.
fn load_dash_checkpoint_into(root: &Path, segments: &mut HashMap<String, u64>) {
    let Ok(text) = fs::read_to_string(root.join("dash_vod_segments.json")) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return;
    }
    let Some(files) = value.get("files").and_then(serde_json::Value::as_object) else {
        return;
    };
    for (raw_rel, record) in files {
        let Some(rel) = valid_relative_str(raw_rel) else {
            continue;
        };
        let has_id = record
            .get("identity")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| !id.is_empty());
        let size = record
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if has_id && size > 0 {
            segments.insert(rel, size);
        }
    }
}

/// Crash-recovery cold loader for `vod_segments.json` / `audio/vod_segments.json`.
fn load_hls_checkpoint_slots(checkpoint_path: &Path) -> HashMap<usize, u64> {
    let Ok(text) = fs::read_to_string(checkpoint_path) else {
        return HashMap::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return HashMap::new();
    };
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return HashMap::new();
    }
    let Some(map) = value.get("segments").and_then(serde_json::Value::as_object) else {
        return HashMap::new();
    };
    let mut slots = HashMap::new();
    for (raw_slot, record) in map {
        if raw_slot.is_empty() || !raw_slot.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(slot) = raw_slot.parse::<usize>() else {
            continue;
        };
        let has_id = record
            .get("identity")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| !id.is_empty());
        let size = record
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if has_id && size > 0 {
            slots.insert(slot, size);
        }
    }
    slots
}

/// Cold-initializes `ReadyTaskState` completely outside the global mutex so hot `is_ready`
/// polling performs zero syscalls under lock and never resurrects invalidated segments.
fn load_ready_state_from_disk(root: &Path) -> ReadyTaskState {
    let marker_dir = root.join(READY_DIR);
    let marker_dir_created = marker_dir.is_dir();
    let mut segments = HashMap::new();
    load_dash_checkpoint_into(root, &mut segments);
    if marker_dir_created {
        load_legacy_markers_from_dir(&marker_dir, &marker_dir, 0, &mut segments);
        load_ready_index_from_disk(&marker_dir, &mut segments);
    }
    let hls_checkpoint_slots = load_hls_checkpoint_slots(&root.join("vod_segments.json"));
    let audio_checkpoint_slots =
        load_hls_checkpoint_slots(&root.join("audio").join("vod_segments.json"));
    let now = Instant::now();
    ReadyTaskState {
        segments,
        hls_checkpoint_slots,
        audio_checkpoint_slots,
        marker_dir_created,
        generation: 0,
        flushed_generation: 0,
        dirty_ops: 0,
        last_flush: now,
        checked: now,
        flush_lock: Arc::new(Mutex::new(())),
        #[cfg(test)]
        disk_loads: 1,
        #[cfg(test)]
        disk_flushes: 0,
    }
}

struct EvictedReadyTask {
    root: PathBuf,
    segments: HashMap<String, u64>,
    flush_lock: Arc<Mutex<()>>,
}

pub(crate) fn flush_ready(root: &Path) -> Result<(), String> {
    let flush_lock = {
        let map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(entry) = map.get(root) else {
            return Ok(());
        };
        if entry.generation == entry.flushed_generation || !entry.marker_dir_created {
            return Ok(());
        }
        Arc::clone(&entry.flush_lock)
    };
    flush_ready_locked(root, flush_lock, false)
}

/// Serializes disk persistence for a single task under `flush_lock`, keeping all file I/O
/// strictly outside the global `ready_segments()` lock. Lock order is always
/// `flush_lock -> ready_segments()`, never the reverse.
fn flush_ready_locked(
    root: &Path,
    flush_lock: Arc<Mutex<()>>,
    create_dir_if_missing: bool,
) -> Result<(), String> {
    let _flush_guard = flush_lock.lock().unwrap_or_else(|error| error.into_inner());
    let (snapshot, snapshot_gen, snapshot_dirty, need_dir) = {
        let map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(entry) = map.get(root) else {
            return Ok(());
        };
        if !Arc::ptr_eq(&entry.flush_lock, &flush_lock) {
            return Ok(());
        }
        if entry.generation == entry.flushed_generation {
            return Ok(());
        }
        if !create_dir_if_missing && !entry.marker_dir_created {
            return Ok(());
        }
        (
            entry.segments.clone(),
            entry.generation,
            entry.dirty_ops,
            create_dir_if_missing && !entry.marker_dir_created,
        )
    };

    let marker_dir = root.join(READY_DIR);
    if need_dir {
        fs::create_dir_all(&marker_dir).map_err(|error| error.to_string())?;
    } else if !create_dir_if_missing && !marker_dir.is_dir() {
        let mut map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = map
            .get_mut(root)
            .filter(|entry| Arc::ptr_eq(&entry.flush_lock, &flush_lock))
        {
            entry.marker_dir_created = false;
        }
        return Ok(());
    }

    match flush_ready_index(&marker_dir, &snapshot) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_dir_if_missing => {
            fs::create_dir_all(&marker_dir).map_err(|err| err.to_string())?;
            flush_ready_index(&marker_dir, &snapshot).map_err(|err| err.to_string())?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut map = ready_segments()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = map
                .get_mut(root)
                .filter(|entry| Arc::ptr_eq(&entry.flush_lock, &flush_lock))
            {
                entry.marker_dir_created = false;
            }
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    }

    let mut map = ready_segments()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(entry) = map
        .get_mut(root)
        .filter(|entry| Arc::ptr_eq(&entry.flush_lock, &flush_lock))
    {
        entry.marker_dir_created = true;
        entry.flushed_generation = snapshot_gen;
        if entry.generation == entry.flushed_generation {
            entry.dirty_ops = 0;
        } else {
            entry.dirty_ops = entry.dirty_ops.saturating_sub(snapshot_dirty).max(1);
        }
        let now = Instant::now();
        entry.last_flush = now;
        entry.checked = now;
        #[cfg(test)]
        {
            entry.disk_flushes += 1;
        }
    }
    Ok(())
}

fn flush_ready_index(marker_dir: &Path, segments: &HashMap<String, u64>) -> std::io::Result<()> {
    let mut payload = String::with_capacity(segments.len().saturating_mul(28));
    for (relative, size) in segments {
        if *size > 0 {
            payload.push_str(relative);
            payload.push('\t');
            payload.push_str(&size.to_string());
            payload.push('\n');
        }
    }
    let temporary = marker_dir.join(format!(".{READY_INDEX_FILE}.tmp"));
    // Preview markers are ephemeral UI playback hints, never durability checkpoints:
    // coalesce into one un-fsynced index file to avoid per-segment NTFS/AV write storms.
    fs::write(&temporary, payload.as_bytes())?;
    super::hls::replace_file_atomic(&temporary, &marker_dir.join(READY_INDEX_FILE))
}

fn take_evicted_ready_segments(
    map: &mut HashMap<PathBuf, ReadyTaskState>,
) -> Vec<EvictedReadyTask> {
    if map.len() < READY_MAX_ENTRIES {
        return Vec::new();
    }
    let mut evicted_dirty = Vec::new();
    while map.len() >= READY_MAX_ENTRIES {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, entry)| entry.checked)
            .map(|(path, _)| path.clone())
        else {
            break;
        };
        if let Some(evicted) = map.remove(&oldest) {
            if evicted.generation != evicted.flushed_generation && evicted.marker_dir_created {
                evicted_dirty.push(EvictedReadyTask {
                    root: oldest,
                    segments: evicted.segments,
                    flush_lock: evicted.flush_lock,
                });
            }
        }
    }
    evicted_dirty
}

fn flush_evicted_ready_tasks(evicted: Vec<EvictedReadyTask>) {
    for item in evicted {
        let _flush_guard = item
            .flush_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let superseded = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&item.root);
        if superseded {
            continue;
        }
        let marker_dir = item.root.join(READY_DIR);
        if marker_dir.is_dir() {
            let _ = flush_ready_index(&marker_dir, &item.segments);
        }
    }
}

pub(crate) fn mark_ready(root: &Path, file: &Path) -> Result<(), String> {
    let relative = relative_file(root, file).ok_or("播放分片不属于任务目录")?;
    let size = fs::metadata(file).map_err(|error| error.to_string())?.len();
    if size == 0 {
        return Err("播放分片为空".into());
    }
    let loaded = {
        let map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if map.contains_key(root) {
            None
        } else {
            drop(map);
            Some(load_ready_state_from_disk(root))
        }
    };
    let (evicted, flush_lock) = {
        let mut map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let evicted = take_evicted_ready_segments(&mut map);
        let now = Instant::now();
        let entry = match map.entry(root.to_path_buf()) {
            std::collections::hash_map::Entry::Occupied(occupied) => occupied.into_mut(),
            std::collections::hash_map::Entry::Vacant(vacant) => {
                vacant.insert(loaded.unwrap_or_else(|| load_ready_state_from_disk(root)))
            }
        };
        let needs_create_dir = !entry.marker_dir_created;
        entry.clear_checkpoint_slot(&relative);
        entry.segments.insert(relative, size);
        entry.generation = entry.generation.wrapping_add(1);
        entry.dirty_ops = entry.dirty_ops.saturating_add(1);
        entry.checked = now;
        let should_flush = needs_create_dir
            || entry.segments.len() <= 2
            || entry.dirty_ops >= READY_FLUSH_BATCH
            || entry.last_flush.elapsed() >= READY_FLUSH_INTERVAL;
        let flush_lock = should_flush.then(|| Arc::clone(&entry.flush_lock));
        (evicted, flush_lock)
    };
    flush_evicted_ready_tasks(evicted);
    if let Some(flush_lock) = flush_lock {
        flush_ready_locked(root, flush_lock, true)?;
    }
    Ok(())
}

pub(crate) fn reset_ready(root: &Path, file: &Path) -> Result<(), String> {
    let relative = relative_file(root, file).ok_or("播放分片不属于任务目录")?;
    let marker_dir = root.join(READY_DIR);
    if !root.is_dir() {
        ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(root);
        return Ok(());
    }
    let loaded = {
        let map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if map.contains_key(root) {
            None
        } else {
            drop(map);
            Some(load_ready_state_from_disk(root))
        }
    };
    let evicted = {
        let mut map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let evicted = take_evicted_ready_segments(&mut map);
        let now = Instant::now();
        let entry = match map.entry(root.to_path_buf()) {
            std::collections::hash_map::Entry::Occupied(occupied) => occupied.into_mut(),
            std::collections::hash_map::Entry::Vacant(vacant) => {
                vacant.insert(loaded.unwrap_or_else(|| load_ready_state_from_disk(root)))
            }
        };
        entry.checked = now;
        let removed_segment = entry.segments.remove(&relative).is_some();
        entry.clear_checkpoint_slot(&relative);
        if removed_segment {
            entry.generation = entry.generation.wrapping_add(1);
            entry.dirty_ops = entry.dirty_ops.saturating_add(1);
        }
        evicted
    };
    flush_evicted_ready_tasks(evicted);
    match fs::remove_file(marker_dir.join(relative)) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn is_ready(root: &Path, relative: &str) -> bool {
    let Some(relative) = valid_relative_str(relative) else {
        return false;
    };
    let (expected, evicted) = {
        let mut map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = map.get_mut(root) {
            entry.checked = Instant::now();
            (entry.expected_size(&relative), Vec::new())
        } else {
            drop(map);
            if !root.is_dir() {
                return false;
            }
            let loaded = load_ready_state_from_disk(root);
            let mut map = ready_segments()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let evicted = take_evicted_ready_segments(&mut map);
            let now = Instant::now();
            let entry = map.entry(root.to_path_buf()).or_insert(loaded);
            entry.checked = now;
            (entry.expected_size(&relative), evicted)
        }
    };
    flush_evicted_ready_tasks(evicted);
    expected.is_some_and(|size| {
        size > 0
            && fs::metadata(root.join(relative))
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == size)
    })
}

pub(crate) fn request_priority(root: &Path, relative: &str) -> Result<(), String> {
    write_priority(root, "playback-priority", relative)
}

pub(crate) fn priority_file(root: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(root.join("playback-priority")).ok()?;
    let relative = Path::new(text.trim());
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(relative))
}

pub(crate) fn request_byte_priority(root: &Path, position: u64) -> Result<(), String> {
    let mut map = byte_priorities()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // 写入与对账串行，旧的磁盘读取不能覆盖刚提交的播放位置。
    write_priority(root, BYTE_PRIORITY_FILE, &position.to_string())?;
    prune_byte_priorities(&mut map);
    let entry = map.entry(root.to_path_buf()).or_insert(BytePriority {
        present: false,
        position: None,
        checked: Instant::now(),
        file_reads: 0,
    });
    entry.present = true;
    entry.position = Some(position);
    entry.checked = Instant::now();
    Ok(())
}

/// 边播边下字节优先级文件名（HTTP 文件任务）。
pub(crate) const BYTE_PRIORITY_FILE: &str = "playback-byte-priority";
/// 内存值之外，至多每秒重读一次文件（外部写入、清理删除）。
pub(crate) const BYTE_PRIORITY_RECHECK: Duration = Duration::from_secs(1);
const BYTE_PRIORITY_MAX_ENTRIES: usize = 256;

struct BytePriority {
    present: bool,
    position: Option<u64>,
    checked: Instant,
    file_reads: u64,
}

fn byte_priorities() -> &'static Mutex<HashMap<PathBuf, BytePriority>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, BytePriority>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prune_byte_priorities(map: &mut HashMap<PathBuf, BytePriority>) {
    if map.len() < BYTE_PRIORITY_MAX_ENTRIES {
        return;
    }
    map.retain(|_, entry| entry.checked.elapsed() < Duration::from_secs(60));
    while map.len() >= BYTE_PRIORITY_MAX_ENTRIES {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, entry)| entry.checked)
            .map(|(path, _)| path.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

/// 当前边播边下字节优先级：`(文件存在, 解析出的位置)`。播放路径写入时同时更新
/// 内存值，下载线程热路径只读内存；文件每任务至多每秒重读一次。
fn byte_priority_state(root: &Path) -> (bool, Option<u64>) {
    let mut map = byte_priorities()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(entry) = map.get(root) {
        if entry.checked.elapsed() < BYTE_PRIORITY_RECHECK {
            return (entry.present, entry.position);
        }
    }
    let text = fs::read_to_string(root.join(BYTE_PRIORITY_FILE));
    let present = text.is_ok();
    let position = text.ok().and_then(|text| text.trim().parse::<u64>().ok());
    prune_byte_priorities(&mut map);
    let entry = map.entry(root.to_path_buf()).or_insert(BytePriority {
        present,
        position,
        checked: Instant::now(),
        file_reads: 0,
    });
    entry.present = present;
    entry.position = position;
    entry.checked = Instant::now();
    entry.file_reads += 1;
    (present, position)
}

/// 播放请求的字节位置（无请求或无法解析时为 `None`）。
pub(crate) fn byte_priority(root: &Path) -> Option<u64> {
    byte_priority_state(root).1
}

/// 是否存在边播边下字节优先级请求（等价于旧的“文件存在”判断，至多 1 s 滞后于外部改动）。
pub(crate) fn has_byte_priority(root: &Path) -> bool {
    byte_priority_state(root).0
}

/// 任务清理时丢弃内存值。
pub(crate) fn forget_byte_priority(root: &Path) {
    byte_priorities()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(root);
    ready_segments()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(root);
}

#[cfg(test)]
pub(crate) fn byte_priority_file_reads(root: &Path) -> u64 {
    byte_priorities()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(root)
        .map_or(0, |entry| entry.file_reads)
}

#[cfg(test)]
fn ready_disk_loads(root: &Path) -> u64 {
    ready_segments()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(root)
        .map_or(0, |entry| entry.disk_loads)
}

#[cfg(test)]
fn ready_disk_flushes(root: &Path) -> u64 {
    ready_segments()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(root)
        .map_or(0, |entry| entry.disk_flushes)
}

fn write_priority(root: &Path, name: &str, value: &str) -> Result<(), String> {
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITER.lock().map_err(|_| "播放优先级锁已失效")?;
    // 请求可能晚于任务删除到达；不能让播放器重新创建刚清理掉的缓存目录。
    let temporary = root.join(format!(".{name}.tmp"));
    fs::write(&temporary, value).map_err(|error| error.to_string())?;
    super::hls::replace_file_atomic(&temporary, &root.join(name)).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "hls-preview-priority-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn byte_priority_is_served_from_memory_and_rechecks_the_file_at_most_once_per_second() {
        let root = temp_root("memory");
        assert_eq!(byte_priority(&root), None);
        assert!(!has_byte_priority(&root));
        let reads_after_miss = byte_priority_file_reads(&root);
        assert_eq!(reads_after_miss, 1);
        // The playback path updates memory at once, without waiting for a recheck.
        request_byte_priority(&root, 4096).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(BYTE_PRIORITY_FILE)).unwrap(),
            "4096"
        );
        for _ in 0..50 {
            assert_eq!(byte_priority(&root), Some(4096));
        }
        assert_eq!(byte_priority_file_reads(&root), reads_after_miss);
        // An external rewrite is picked up after the recheck interval.
        fs::write(root.join(BYTE_PRIORITY_FILE), "8192").unwrap();
        std::thread::sleep(BYTE_PRIORITY_RECHECK + Duration::from_millis(50));
        assert_eq!(byte_priority(&root), Some(8192));
        assert_eq!(byte_priority_file_reads(&root), reads_after_miss + 1);
        // Removal by cleanup plus forget never serves the stale value.
        fs::remove_file(root.join(BYTE_PRIORITY_FILE)).unwrap();
        forget_byte_priority(&root);
        assert_eq!(byte_priority(&root), None);
        assert!(!has_byte_priority(&root));
        forget_byte_priority(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn request_for_a_removed_task_directory_leaves_no_memory_value() {
        let root = temp_root("removed");
        fs::remove_dir_all(&root).unwrap();
        assert!(request_byte_priority(&root, 1).is_err());
        assert!(!root.exists());
        assert_eq!(byte_priority_file_reads(&root), 0);
        assert_eq!(byte_priority(&root), None);
        forget_byte_priority(&root);
    }

    #[test]
    fn ready_markers_coalesce_into_single_index_without_per_segment_files() {
        let root = temp_root("coalesced-ready");
        let seg_dir = root.join("segments");
        fs::create_dir_all(&seg_dir).unwrap();

        let empty_seg = seg_dir.join("empty.ts");
        fs::write(&empty_seg, b"").unwrap();
        assert!(mark_ready(&root, &empty_seg).is_err());
        assert!(!is_ready(&root, "segments/empty.ts"));

        for idx in 0..200 {
            let seg = seg_dir.join(format!("{idx:06}.ts"));
            fs::write(&seg, format!("segment-payload-{idx:06}").as_bytes()).unwrap();
            mark_ready(&root, &seg).unwrap();
        }

        // All 200 segments are immediately ready in memory without creating 200 marker files.
        for idx in 0..200 {
            assert!(is_ready(&root, &format!("segments/{idx:06}.ts")));
        }
        assert!(!is_ready(&root, "../outside.ts"));
        assert!(!root.join("playback-ready/segments").exists());
        assert!(root.join("playback-ready/index.tsv").is_file());
        let marker_entries: Vec<_> = fs::read_dir(root.join("playback-ready"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(marker_entries, vec!["index.tsv".to_string()]);

        // Mutating segment length invalidates readiness until `mark_ready` is called again,
        // and `reset_ready` drops readiness immediately in memory; persisting the reset across
        // restart happens on `flush_ready` (avoiding per-segment disk rewrites during batch reset).
        let seg10 = seg_dir.join("000010.ts");
        fs::write(&seg10, b"short").unwrap();
        assert!(!is_ready(&root, "segments/000010.ts"));
        mark_ready(&root, &seg10).unwrap();
        assert!(is_ready(&root, "segments/000010.ts"));
        reset_ready(&root, &seg10).unwrap();
        assert!(!is_ready(&root, "segments/000010.ts"));
        flush_ready(&root).unwrap();

        // Simulated process restart reloads flushed index entries and legacy per-segment files.
        fs::create_dir_all(root.join("playback-ready/legacy")).unwrap();
        fs::create_dir_all(root.join("legacy")).unwrap();
        fs::write(root.join("legacy/old.ts"), b"ABCD").unwrap();
        fs::write(root.join("playback-ready/legacy/old.ts"), b"4").unwrap();
        forget_byte_priority(&root);
        assert!(is_ready(&root, "segments/000000.ts"));
        assert!(!is_ready(&root, "segments/000010.ts"));
        assert!(is_ready(&root, "legacy/old.ts"));

        forget_byte_priority(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tail_batch_flushes_on_pause_and_crash_recovery_reconstructs_only_checkpointed_segments() {
        let root = temp_root("tail-flush-and-checkpoint-recovery");
        let seg_dir = root.join("segments");
        let audio_seg_dir = root.join("audio/segments");
        let dash_dir = root.join("video");
        fs::create_dir_all(&seg_dir).unwrap();
        fs::create_dir_all(&audio_seg_dir).unwrap();
        fs::create_dir_all(&dash_dir).unwrap();

        // 1. Complete 4 segments rapidly (< 500ms, < 64 ops): segments 2 & 3 are dirty in RAM
        // until `flush_ready` is called on pause/stop.
        for idx in 0..4 {
            let seg = seg_dir.join(format!("{idx:06}.ts"));
            fs::write(&seg, format!("payload-{idx}").as_bytes()).unwrap();
            mark_ready(&root, &seg).unwrap();
        }
        flush_ready(&root).unwrap();
        forget_byte_priority(&root);
        for idx in 0..4 {
            assert!(
                is_ready(&root, &format!("segments/{idx:06}.ts")),
                "segment {idx} must survive restart after flush_ready"
            );
        }

        // 2. Simulate abrupt crash BEFORE `flush_ready`: segments 4 & 5 and DASH/audio segments
        // are NOT in `index.tsv`, while segment 6 is an uncheckpointed stray file on disk!
        let seg4 = seg_dir.join("000004.ts");
        let seg5_stray = seg_dir.join("000005.ts");
        let audio0 = audio_seg_dir.join("000000.aac");
        let dash1 = dash_dir.join("seg-1.m4s");
        fs::write(&seg4, b"durable-hls-4").unwrap();
        fs::write(&seg5_stray, b"stray-uncheckpointed").unwrap();
        fs::write(&audio0, b"durable-audio-0").unwrap();
        fs::write(&dash1, b"durable-dash-1").unwrap();

        fs::write(
            root.join("vod_segments.json"),
            serde_json::json!({
                "version": 1,
                "segments": {
                    "4": { "identity": "id-4", "size": 13 }
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            root.join("audio/vod_segments.json"),
            serde_json::json!({
                "version": 1,
                "segments": {
                    "0": { "identity": "aud-0", "size": 15 }
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            root.join("dash_vod_segments.json"),
            serde_json::json!({
                "version": 1,
                "files": {
                    "video/seg-1.m4s": { "identity": "dash-1", "size": 14 }
                }
            })
            .to_string(),
        )
        .unwrap();

        forget_byte_priority(&root);
        assert!(is_ready(&root, "segments/000004.ts"));
        assert!(is_ready(&root, "audio/segments/000000.aac"));
        assert!(is_ready(&root, "video/seg-1.m4s"));
        assert!(
            !is_ready(&root, "segments/000005.ts"),
            "uncheckpointed file on disk must never be treated as ready"
        );
        assert_eq!(ready_disk_loads(&root), 1);

        // 3. Hot-polling missing segments (like `playback.rs` 50ms waits) must never re-read
        // checkpoints or legacy markers from disk.
        for _ in 0..100 {
            assert!(!is_ready(&root, "segments/000005.ts"));
            assert!(!is_ready(&root, "audio/segments/000009.aac"));
            assert!(!is_ready(&root, "video/seg-99.m4s"));
        }
        assert_eq!(ready_disk_loads(&root), 1);

        // 4. `reset_ready` must invalidate checkpoint-recovered segments immediately and never let
        // a stale on-disk checkpoint resurrect them before `vod.save` runs.
        reset_ready(&root, &seg4).unwrap();
        reset_ready(&root, &audio0).unwrap();
        reset_ready(&root, &dash1).unwrap();
        assert!(!is_ready(&root, "segments/000004.ts"));
        assert!(!is_ready(&root, "audio/segments/000000.aac"));
        assert!(!is_ready(&root, "video/seg-1.m4s"));
        assert_eq!(ready_disk_loads(&root), 1);

        // 5. Cold `reset_ready` even without a `playback-ready` directory must still suppress
        // stale on-disk checkpoints before `vod.save` completes, and a subsequent `mark_ready`
        // for a sibling segment must not undo those invalidations.
        fs::remove_dir_all(root.join(READY_DIR)).unwrap();
        forget_byte_priority(&root);
        reset_ready(&root, &seg4).unwrap();
        reset_ready(&root, &audio0).unwrap();
        reset_ready(&root, &dash1).unwrap();
        let seg0 = seg_dir.join("000000.ts");
        mark_ready(&root, &seg0).unwrap();
        assert!(is_ready(&root, "segments/000000.ts"));
        assert!(!is_ready(&root, "segments/000004.ts"));
        assert!(!is_ready(&root, "audio/segments/000000.aac"));
        assert!(!is_ready(&root, "video/seg-1.m4s"));
        assert_eq!(ready_disk_loads(&root), 1);

        forget_byte_priority(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_reset_ready_coalesces_into_single_flush_and_persists_across_reload() {
        let root = temp_root("batch-reset-coalesced");
        let seg_dir = root.join("segments");
        fs::create_dir_all(&seg_dir).unwrap();

        for idx in 0..128 {
            let seg = seg_dir.join(format!("{idx:06}.ts"));
            fs::write(&seg, format!("payload-{idx:06}").as_bytes()).unwrap();
            mark_ready(&root, &seg).unwrap();
        }
        flush_ready(&root).unwrap();
        let flushes_before = ready_disk_flushes(&root);
        assert!(flushes_before > 0);

        // Resetting 100 segments in a loop must invalidate RAM immediately without per-segment disk flushes.
        for idx in 0..100 {
            let seg = seg_dir.join(format!("{idx:06}.ts"));
            reset_ready(&root, &seg).unwrap();
            assert!(!is_ready(&root, &format!("segments/{idx:06}.ts")));
        }
        assert_eq!(
            ready_disk_flushes(&root),
            flushes_before,
            "reset_ready must not flush index.tsv synchronously per segment"
        );

        // One trailing flush_ready persists the entire batch in O(1) disk writes.
        flush_ready(&root).unwrap();
        assert_eq!(ready_disk_flushes(&root), flushes_before + 1);
        // Clean flush_ready is a no-op.
        flush_ready(&root).unwrap();
        assert_eq!(ready_disk_flushes(&root), flushes_before + 1);

        // Simulate process restart and verify persisted invalidations vs surviving entries.
        forget_byte_priority(&root);
        for idx in 0..100 {
            assert!(
                !is_ready(&root, &format!("segments/{idx:06}.ts")),
                "reset segment {idx} must stay invalid after reload"
            );
        }
        for idx in 100..128 {
            assert!(
                is_ready(&root, &format!("segments/{idx:06}.ts")),
                "untouched segment {idx} must stay ready after reload"
            );
        }

        // Verify concurrent flush of one task never blocks another task's ready map operations
        // and that flush_lock -> ready_segments lock ordering cannot deadlock under contention.
        let other = temp_root("batch-reset-concurrent-other");
        let other_seg_dir = other.join("segments");
        fs::create_dir_all(&other_seg_dir).unwrap();
        let other_seg = other_seg_dir.join("000000.ts");
        fs::write(&other_seg, b"other-payload").unwrap();
        mark_ready(&other, &other_seg).unwrap();

        let root_a = root.clone();
        let root_b = other.clone();
        let seg_a = seg_dir.join("000100.ts");
        let seg_b = other_seg.clone();
        let t1 = std::thread::spawn(move || {
            for _ in 0..32 {
                reset_ready(&root_a, &seg_a).unwrap();
                mark_ready(&root_a, &seg_a).unwrap();
                flush_ready(&root_a).unwrap();
            }
        });
        let t2 = std::thread::spawn(move || {
            for _ in 0..32 {
                assert!(is_ready(&root_b, "segments/000000.ts"));
                mark_ready(&root_b, &seg_b).unwrap();
                flush_ready(&root_b).unwrap();
            }
        });
        t1.join().unwrap();
        t2.join().unwrap();
        assert!(is_ready(&root, "segments/000100.ts"));
        assert!(is_ready(&other, "segments/000000.ts"));

        forget_byte_priority(&root);
        forget_byte_priority(&other);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(other).unwrap();
    }
}
