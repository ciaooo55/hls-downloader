//! 下载器与本地播放器共用的分片就绪标记和播放优先级。

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
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
    segments: HashMap<String, u64>,
    dirty_ops: usize,
    last_flush: Instant,
    checked: Instant,
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

fn load_ready_index_from_disk(marker_dir: &Path) -> HashMap<String, u64> {
    let Ok(text) = fs::read_to_string(marker_dir.join(READY_INDEX_FILE)) else {
        return HashMap::new();
    };
    let mut segments = HashMap::new();
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
    segments
}

fn read_legacy_marker(marker_dir: &Path, relative: &str) -> Option<u64> {
    use std::io::Read;
    fs::File::open(marker_dir.join(relative))
        .ok()
        .and_then(|file| {
            let mut size = String::new();
            file.take(21).read_to_string(&mut size).ok()?;
            let trimmed = size.trim();
            (trimmed.len() <= 20)
                .then(|| trimmed.parse::<u64>().ok())
                .flatten()
                .filter(|&parsed| parsed > 0)
        })
}

/// Crash-recovery fallback: if `index.tsv` missed the last dirty batch before an abrupt process
/// exit, reconstruct segment readiness strictly from durable segment checkpoints (`dash_vod_segments.json`,
/// `vod_segments.json`, `audio/vod_segments.json`)—never from unverified stray files on disk.
fn read_checkpoint_ready_size(root: &Path, relative: &str) -> Option<u64> {
    if let Ok(text) = fs::read_to_string(root.join("dash_vod_segments.json")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if value.get("version").and_then(serde_json::Value::as_u64) == Some(1) {
                if let Some(record) = value.get("files").and_then(|files| files.get(relative)) {
                    let has_id = record
                        .get("identity")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| !id.is_empty());
                    let size = record
                        .get("size")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    if has_id && size > 0 {
                        return Some(size);
                    }
                }
            }
        }
    }

    let (checkpoint_path, filename) = if let Some(rest) = relative.strip_prefix("segments/") {
        (root.join("vod_segments.json"), rest)
    } else {
        let rest = relative.strip_prefix("audio/segments/")?;
        (root.join("audio").join("vod_segments.json"), rest)
    };
    if filename.contains('/') {
        return None;
    }
    let (stem, _ext) = filename.split_once('.')?;
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let slot: usize = stem.parse().ok()?;
    let text = fs::read_to_string(checkpoint_path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return None;
    }
    let record = value
        .get("segments")
        .and_then(|segments| segments.get(slot.to_string()))?;
    let has_id = record
        .get("identity")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.is_empty());
    let size = record.get("size").and_then(serde_json::Value::as_u64)?;
    (has_id && size > 0).then_some(size)
}

pub(crate) fn flush_ready(root: &Path) -> Result<(), String> {
    let marker_dir = root.join(READY_DIR);
    let mut map = ready_segments()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(entry) = map.get_mut(root) else {
        return Ok(());
    };
    if entry.dirty_ops == 0 || !marker_dir.is_dir() {
        return Ok(());
    }
    flush_ready_index(&marker_dir, &entry.segments)?;
    entry.dirty_ops = 0;
    entry.last_flush = Instant::now();
    entry.checked = Instant::now();
    Ok(())
}

fn flush_ready_index(marker_dir: &Path, segments: &HashMap<String, u64>) -> Result<(), String> {
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
    fs::write(&temporary, payload.as_bytes()).map_err(|error| error.to_string())?;
    super::hls::replace_file_atomic(&temporary, &marker_dir.join(READY_INDEX_FILE))
        .map_err(|error| error.to_string())
}

fn prune_ready_segments(map: &mut HashMap<PathBuf, ReadyTaskState>) {
    if map.len() < READY_MAX_ENTRIES {
        return;
    }
    while map.len() >= READY_MAX_ENTRIES {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, entry)| entry.checked)
            .map(|(path, _)| path.clone())
        else {
            break;
        };
        if let Some(evicted) = map.remove(&oldest) {
            let marker_dir = oldest.join(READY_DIR);
            if evicted.dirty_ops > 0 && marker_dir.is_dir() {
                let _ = flush_ready_index(&marker_dir, &evicted.segments);
            }
        }
    }
}

pub(crate) fn mark_ready(root: &Path, file: &Path) -> Result<(), String> {
    let relative = relative_file(root, file).ok_or("播放分片不属于任务目录")?;
    let size = fs::metadata(file).map_err(|error| error.to_string())?.len();
    if size == 0 {
        return Err("播放分片为空".into());
    }
    let marker_dir = root.join(READY_DIR);
    let mut map = ready_segments()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir_existed = marker_dir.is_dir();
    if !dir_existed {
        map.remove(root);
        fs::create_dir_all(&marker_dir).map_err(|error| error.to_string())?;
    }
    prune_ready_segments(&mut map);
    let now = Instant::now();
    let entry = map
        .entry(root.to_path_buf())
        .or_insert_with(|| ReadyTaskState {
            segments: if dir_existed {
                load_ready_index_from_disk(&marker_dir)
            } else {
                HashMap::new()
            },
            dirty_ops: 0,
            last_flush: now,
            checked: now,
        });
    entry.segments.insert(relative, size);
    entry.dirty_ops = entry.dirty_ops.saturating_add(1);
    entry.checked = now;
    let should_flush = !dir_existed
        || entry.segments.len() <= 2
        || entry.dirty_ops >= READY_FLUSH_BATCH
        || entry.last_flush.elapsed() >= READY_FLUSH_INTERVAL;
    if should_flush {
        flush_ready_index(&marker_dir, &entry.segments)?;
        entry.dirty_ops = 0;
        entry.last_flush = Instant::now();
    }
    Ok(())
}

pub(crate) fn reset_ready(root: &Path, file: &Path) -> Result<(), String> {
    let relative = relative_file(root, file).ok_or("播放分片不属于任务目录")?;
    let marker_dir = root.join(READY_DIR);
    {
        let mut map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !marker_dir.is_dir() {
            map.remove(root);
            return Ok(());
        }
        prune_ready_segments(&mut map);
        let now = Instant::now();
        let entry = map
            .entry(root.to_path_buf())
            .or_insert_with(|| ReadyTaskState {
                segments: load_ready_index_from_disk(&marker_dir),
                dirty_ops: 0,
                last_flush: now,
                checked: now,
            });
        entry.checked = now;
        if entry.segments.remove(&relative).is_some() {
            flush_ready_index(&marker_dir, &entry.segments)?;
            entry.dirty_ops = 0;
            entry.last_flush = Instant::now();
        }
    }
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
    let marker_dir = root.join(READY_DIR);
    let expected = {
        let mut map = ready_segments()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !marker_dir.is_dir()
            && !root.join("preview.m3u8").is_file()
            && !root.join("preview-video.m3u8").is_file()
        {
            map.remove(root);
            return false;
        }
        prune_ready_segments(&mut map);
        let now = Instant::now();
        let entry = map
            .entry(root.to_path_buf())
            .or_insert_with(|| ReadyTaskState {
                segments: if marker_dir.is_dir() {
                    load_ready_index_from_disk(&marker_dir)
                } else {
                    HashMap::new()
                },
                dirty_ops: 0,
                last_flush: now,
                checked: now,
            });
        entry.checked = now;
        entry.segments.get(&relative).copied().or_else(|| {
            let recovered = read_legacy_marker(&marker_dir, &relative)
                .or_else(|| read_checkpoint_ready_size(root, &relative))?;
            entry.segments.insert(relative.clone(), recovered);
            Some(recovered)
        })
    };
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
        // and `reset_ready` drops readiness immediately both in memory and after reload.
        let seg10 = seg_dir.join("000010.ts");
        fs::write(&seg10, b"short").unwrap();
        assert!(!is_ready(&root, "segments/000010.ts"));
        mark_ready(&root, &seg10).unwrap();
        assert!(is_ready(&root, "segments/000010.ts"));
        reset_ready(&root, &seg10).unwrap();
        assert!(!is_ready(&root, "segments/000010.ts"));

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

        forget_byte_priority(&root);
        fs::remove_dir_all(root).unwrap();
    }
}
