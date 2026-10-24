//! 下载器与本地播放器共用的分片就绪标记和播放优先级。

use std::fs;
use std::path::{Component, Path, PathBuf};

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
    let expected = fs::read_to_string(root.join("playback-ready").join(relative))
        .ok()
        .and_then(|text| text.parse::<u64>().ok());
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
    write_priority(root, "playback-byte-priority", &position.to_string())
}

fn write_priority(root: &Path, name: &str, value: &str) -> Result<(), String> {
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITER.lock().map_err(|_| "播放优先级锁已失效")?;
    // 请求可能晚于任务删除到达；不能让播放器重新创建刚清理掉的缓存目录。
    let temporary = root.join(format!(".{name}.tmp"));
    fs::write(&temporary, value).map_err(|error| error.to_string())?;
    super::hls::replace_file_atomic(&temporary, &root.join(name)).map_err(|error| error.to_string())
}
