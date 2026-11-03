//! HTTP Range connection map and worker count from the same progress sample.

use crate::ConnectionPart;
use serde_json::Value;
use std::path::Path;

const PARTS_LIMIT: usize = 64;

fn paint_file_map(total: u64, done: &[(u64, u64)], active: &[(u64, u64)]) -> Vec<ConnectionPart> {
    if total == 0 {
        return Vec::new();
    }
    let done = subtract(&merge(done), active);
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
    let mut parts = Vec::new();
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
        parts.push(ConnectionPart {
            start,
            end: end.saturating_sub(1),
            done: done_bytes,
            state: state.into(),
        });
    }
    coalesce(&mut parts);
    if parts.len() > PARTS_LIMIT {
        parts.truncate(PARTS_LIMIT);
    }
    parts
}

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
    let total = total.max(downloaded);
    let ranges = load_completed_ranges(progress_path).unwrap_or_default();
    let mut active = if downloading {
        load_active_ranges(progress_path).unwrap_or_default()
    } else {
        Vec::new()
    };
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
    load_ranges(&progress_path.with_file_name("native-engine.ranges.json"))
        .map(|ranges| {
            ranges
                .into_iter()
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
    let text = std::fs::read_to_string(path).ok()?;
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

fn subtract(base: &[(u64, u64)], holes: &[(u64, u64)]) -> Vec<(u64, u64)> {
    if base.is_empty() {
        return Vec::new();
    }
    if holes.is_empty() {
        return base.to_vec();
    }
    let mut remaining = Vec::new();
    for &(start, end) in base {
        let mut pieces = vec![(start, end)];
        for &(hole_start, hole_end) in holes {
            let mut next = Vec::new();
            for (piece_start, piece_end) in pieces {
                if hole_end <= piece_start || hole_start >= piece_end {
                    next.push((piece_start, piece_end));
                    continue;
                }
                if piece_start < hole_start {
                    next.push((piece_start, hole_start));
                }
                if hole_end < piece_end {
                    next.push((hole_end, piece_end));
                }
            }
            pieces = next;
        }
        remaining.extend(pieces);
    }
    merge(&remaining)
}

fn covers(intervals: &[(u64, u64)], position: u64) -> bool {
    intervals
        .iter()
        .any(|(start, end)| *start <= position && position < *end)
}

fn coalesce(parts: &mut Vec<ConnectionPart>) {
    if parts.len() < 2 {
        return;
    }
    let mut out: Vec<ConnectionPart> = Vec::with_capacity(parts.len());
    for part in parts.drain(..) {
        if let Some(last) = out.last_mut() {
            if last.state == part.state && last.end + 1 == part.start {
                last.end = part.end;
                last.done = last.done.saturating_add(part.done);
                continue;
            }
        }
        out.push(part);
    }
    *parts = out;
}

#[cfg(test)]
mod tests {
    use super::*;

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
