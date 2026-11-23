//! HTTP Range connection map and worker count from the same progress sample.

use crate::ConnectionPart;
use serde_json::Value;
use std::path::Path;

const PARTS_LIMIT: usize = 64;

fn paint_file_map(total: u64, done: &[(u64, u64)], active: &[(u64, u64)]) -> Vec<ConnectionPart> {
    if total == 0 {
        return Vec::new();
    }
    let done = merge(done);
    let active = merge(active);
    let mut points = std::collections::BTreeSet::new();
    points.insert(0);
    points.insert(total);
    for (start, end) in done.iter().chain(active.iter()) {
        if *start <= total {
            points.insert(*start);
        }
        if *end <= total {
            points.insert(*end);
        }
    }
    let ordered: Vec<u64> = points.into_iter().collect();
    let mut parts: Vec<ConnectionPart> = Vec::new();
    for window in ordered.windows(2) {
        let start = window[0];
        let end = window[1];
        if end <= start {
            continue;
        }
        let (state, done_bytes) = if covers(&active, start) {
            ("active", end - start)
        } else if covers(&done, start) {
            ("done", end - start)
        } else {
            ("queued", 0)
        };
        if let Some(last) = parts.last_mut() {
            if last.state == state {
                last.end = end - 1;
                last.done += done_bytes;
                continue;
            }
        }
        if parts.len() == PARTS_LIMIT {
            break;
        }
        parts.push(ConnectionPart {
            start,
            end: end.saturating_sub(1),
            done: done_bytes,
            state: state.into(),
        });
    }
    parts
}

#[cfg(test)]
pub(crate) fn paint_from_progress(
    progress_path: &Path,
    downloaded: u64,
    total: u64,
    downloading: bool,
) -> (Vec<ConnectionPart>, u32) {
    // 成品发布后检查点会清理，完整字节计数仍能证明所有区间已完成。
    if !downloading && total > 0 && downloaded >= total {
        return (paint_file_map(downloaded, &[(0, downloaded)], &[]), 0);
    }
    let ranges = load_completed_ranges(progress_path).unwrap_or_default();
    let active = if downloading {
        load_active_ranges(progress_path).unwrap_or_default()
    } else {
        Vec::new()
    };
    paint_from_ranges(ranges, active, downloaded, total, downloading)
}

/// Checkpoint ranges a progress sample is painted from, read from the task
/// directory (memory copies first) *before* the Core lock is taken; see
/// `progress_facts`.
/// `(completed, active)` end-exclusive checkpoint ranges.
pub(crate) type PaintRanges = (Vec<(u64, u64)>, Vec<(u64, u64)>);

pub(crate) fn load_paint_ranges(progress_path: &Path) -> PaintRanges {
    (
        load_completed_ranges(progress_path).unwrap_or_default(),
        load_active_ranges(progress_path).unwrap_or_default(),
    )
}

/// Pure painting step of [`paint_from_progress`]: no filesystem access, safe
/// under the Core lock.
pub(crate) fn paint_from_ranges(
    ranges: Vec<(u64, u64)>,
    active: Vec<(u64, u64)>,
    downloaded: u64,
    total: u64,
    downloading: bool,
) -> (Vec<ConnectionPart>, u32) {
    if !downloading && total > 0 && downloaded >= total {
        return (paint_file_map(downloaded, &[(0, downloaded)], &[]), 0);
    }
    let total = total.max(downloaded);
    let mut active = if downloading { active } else { Vec::new() };
    // 相邻连接绘制时会合并，连接数仍须取合并前的同一份活动检查点。
    let workers = active.len().min(u32::MAX as usize) as u32;
    if downloading && active.is_empty() {
        if let Some((_, end)) = ranges.last() {
            if *end < total {
                let frontier = *end;
                active.push((frontier, (frontier + 1).min(total)));
            }
        } else if downloaded > 0 && downloaded < total {
            active.push((downloaded.saturating_sub(1), downloaded.min(total)));
        }
    }
    (paint_file_map(total, &ranges, &active), workers)
}

pub(crate) fn summarize(parts: &[ConnectionPart]) -> (u32, u64, u64, String) {
    let mut done = 0u64;
    let mut active = 0u32;
    for part in parts {
        match part.state.as_str() {
            "done" => done += 1,
            "active" => active += 1,
            _ => {}
        }
    }
    let hint = parts
        .iter()
        .take(6)
        .map(|part| {
            let label = match part.state.as_str() {
                "done" => "已完成",
                "active" => "传输中",
                _ => "排队",
            };
            format!(
                "{}-{} {}",
                format_bytes(part.start),
                format_bytes(part.end + 1),
                label
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    (active, done, parts.len() as u64, hint)
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / 1024.0 / 1024.0)
    } else if bytes >= 1024 {
        format!("{:.0}KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

fn load_completed_ranges(progress_path: &Path) -> Option<Vec<(u64, u64)>> {
    // 持久检查点经内存缓存读取（引擎写入时直接更新，外部改动按元数据校验）。
    crate::range_cache::load(&progress_path.with_file_name(crate::range_cache::FILE_NAME))
        .map(|view| {
            view.ranges
                .iter()
                .copied()
                .map(|(start, end)| (start, end.saturating_add(1)))
                .collect::<Vec<_>>()
        })
        .map(|ranges| merge(&ranges))
}

fn load_active_ranges(progress_path: &Path) -> Option<Vec<(u64, u64)>> {
    load_ranges(&progress_path.with_file_name("native-engine.active.json")).map(|ranges| {
        ranges
            .into_iter()
            .filter(|(start, end)| end > start)
            .collect()
    })
}

fn load_ranges(path: &Path) -> Option<Vec<(u64, u64)>> {
    let text = crate::task_control::read_progress_file(path)?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let items = value.get("ranges")?.as_array()?;
    let ranges: Vec<(u64, u64)> = items
        .iter()
        .filter_map(|item| {
            let pair = item.as_array()?;
            Some((pair.first()?.as_u64()?, pair.get(1)?.as_u64()?))
        })
        .filter(|(start, end)| end >= start)
        .collect();
    Some(ranges)
}

fn merge(intervals: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut ordered: Vec<(u64, u64)> = intervals
        .iter()
        .copied()
        .filter(|(start, end)| end > start)
        .collect();
    ordered.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ordered {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn covers(intervals: &[(u64, u64)], position: u64) -> bool {
    let index = intervals.partition_point(|&(_, end)| end <= position);
    intervals
        .get(index)
        .is_some_and(|&(start, _)| start <= position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_ranges_keep_active_precedence_and_the_full_last_visible_part() {
        let active: Vec<_> = (0..1000).map(|n| (4 * n + 2, 4 * n + 4)).collect();
        let parts = paint_file_map(4000, &[(0, 127), (128, 4000)], &active);
        assert_eq!(parts.len(), PARTS_LIMIT);
        let last = parts.last().unwrap();
        assert_eq!(
            (last.state.as_str(), last.start, last.end, last.done),
            ("active", 126, 127, 2)
        );
    }

    #[test]
    fn fallback_activity_starts_at_the_first_unfinished_byte() {
        let root =
            std::env::temp_dir().join(format!("hls-v7-range-frontier-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let progress = root.join("progress.json");
        let ranges = progress.with_file_name("native-engine.ranges.json");
        std::fs::write(&ranges, r#"{"ranges":[[0,3]]}"#).unwrap();
        let (parts, workers) = paint_from_progress(&progress, 4, 8, true);
        assert_eq!(workers, 0);
        let active = parts.iter().find(|part| part.state == "active").unwrap();
        assert_eq!((active.start, active.end), (4, 4));
        std::fs::write(&ranges, format!(r#"{{"ranges":[[0,{}]]}}"#, u64::MAX)).unwrap();
        assert!(paint_from_progress(&progress, 4, 8, true)
            .0
            .iter()
            .all(|part| part.state == "done"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paints_done_active_and_queued() {
        let parts = paint_file_map(8, &[(0, 3)], &[(3, 5)]);
        assert_eq!(parts[0].state, "done");
        assert_eq!(parts[0].start, 0);
        assert_eq!(parts[0].end, 2);
        assert_eq!(parts[1].state, "active");
        assert_eq!(parts[2].state, "queued");
    }

    #[test]
    fn completed_file_remains_done_after_range_checkpoint_cleanup() {
        // 成品发布会移除分段检查点；连接图仍须反映已完成字节，而非排队。
        let (parts, workers) =
            paint_from_progress(Path::new("absent-task/progress.json"), 1024, 1024, false);
        assert_eq!(workers, 0);
        let (workers, completed, total, hint) = summarize(&parts);
        assert_eq!((workers, completed, total), (0, 1, 1));
        assert_eq!(parts[0].done, 1024);
        assert_eq!(parts[0].state, "done");
        assert!(!hint.contains("排队"));

        let (partial, _) =
            paint_from_progress(Path::new("absent-task/progress.json"), 512, 1024, false);
        assert_ne!(partial[0].state, "done");
        let (unknown, _) =
            paint_from_progress(Path::new("absent-task/progress.json"), 512, 0, false);
        assert_ne!(unknown[0].state, "done");
    }

    #[test]
    fn counts_adjacent_active_worker_ranges_before_painting() {
        let root =
            std::env::temp_dir().join(format!("hls-v7-active-ranges-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let progress = root.join("progress.json");
        std::fs::write(
            progress.with_file_name("native-engine.active.json"),
            r#"{"ranges":[[0,4],[4,8]]}"#,
        )
        .unwrap();
        let (parts, workers) = paint_from_progress(&progress, 0, 8, true);
        assert_eq!(workers, 2);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].state, "active");
        let (paused, workers) = paint_from_progress(&progress, 0, 8, false);
        assert_eq!(workers, 0);
        assert!(paused.iter().all(|part| part.state != "active"));
        let _ = std::fs::remove_dir_all(root);
    }
}
