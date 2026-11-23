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

pub(crate) fn mark_ready(root: &Path, file: &Path) -> Result<(), String> {
    let relative = relative_file(root, file).ok_or("播放分片不属于任务目录")?;
    let size = fs::metadata(file).map_err(|error| error.to_string())?.len();
    if size == 0 {
        return Err("播放分片为空".into());
    }
    let marker = root.join("playback-ready").join(relative);
    super::hls::write_text_atomic(&marker, &size.to_string())
}

pub(crate) fn reset_ready(root: &Path, file: &Path) -> Result<(), String> {
    let relative = relative_file(root, file).ok_or("播放分片不属于任务目录")?;
    match fs::remove_file(root.join("playback-ready").join(relative)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn is_ready(root: &Path, relative: &str) -> bool {
    use std::io::Read;
    let expected = fs::File::open(root.join("playback-ready").join(relative))
        .ok()
        .and_then(|file| {
            let mut size = String::new();
            file.take(21).read_to_string(&mut size).ok()?;
            (size.len() <= 20)
                .then(|| size.parse::<u64>().ok())
                .flatten()
        });
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
}
