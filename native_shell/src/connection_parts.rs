//! HTTP Range connection map. Mirrors 5.x `connection_parts.py` without
//! inventing extra downloads.

use crate::ConnectionPart;
use serde_json::Value;
use std::path::Path;

const PARTS_LIMIT: usize = 64;

pub fn paint_file_map(total: u64, done: &[(u64, u64)], active: &[(u64, u64)]) -> Vec<ConnectionPart> {
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

pub fn paint_from_progress(
    progress_path: &Path,
    downloaded: u64,
    total: u64,
    downloading: bool,
) -> Vec<ConnectionPart> {
    let ranges = load_completed_ranges(progress_path).unwrap_or_default();
    let total = total.max(downloaded);
    let mut active = Vec::new();
    if downloading {
        if let Some((_, end)) = ranges.last() {
            if *end + 1 < total {
                let frontier = (*end + 1).min(total.saturating_sub(1));
                active.push((frontier, (frontier + 1).min(total)));
            }
        } else if downloaded > 0 && downloaded < total {
            active.push((downloaded.saturating_sub(1), downloaded.min(total)));
        }
    }
    let parts = paint_file_map(total, &ranges, &active);
    if parts.is_empty() && total > 0 {
        return vec![ConnectionPart {
            start: 0,
            end: total.saturating_sub(1),
            done: downloaded.min(total),
            state: if downloading {
                "active"
            } else if downloaded >= total && total > 0 {
                "done"
            } else {
                "queued"
            }
            .into(),
        }];
    }
    parts
}

pub fn summarize(parts: &[ConnectionPart]) -> (u32, u64, u64, String) {
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
            format!("{}-{} {}", format_bytes(part.start), format_bytes(part.end + 1), label)
        })
        .collect::<Vec<_>>()
        .join(" · ");
    (active.max(if parts.iter().any(|part| part.state == "active") { 1 } else { 0 }), done, parts.len() as u64, hint)
}

pub fn sample_cells(parts: &[ConnectionPart], total: u64, downloaded: u64, cells: usize) -> Vec<i32> {
    let cells = cells.max(1);
    let span = total.max(downloaded).max(1);
    let mut out = vec![0; cells];
    if parts.is_empty() {
        let filled = ((downloaded as f64 / span as f64) * cells as f64).round() as usize;
        for (index, cell) in out.iter_mut().enumerate() {
            *cell = if index + 1 < filled {
                2
            } else if index < filled {
                1
            } else {
                0
            };
        }
        return out;
    }
    for (index, cell) in out.iter_mut().enumerate() {
        let pos = span.saturating_mul(index as u64) / cells as u64;
        *cell = parts
            .iter()
            .find_map(|part| {
                if part.start <= pos && pos <= part.end {
                    Some(match part.state.as_str() {
                        "done" => 2,
                        "active" => 1,
                        _ => 0,
                    })
                } else {
                    None
                }
            })
            .unwrap_or(0);
    }
    out
}

fn load_completed_ranges(progress_path: &Path) -> Option<Vec<(u64, u64)>> {
    let path = progress_path.with_file_name("native-engine.ranges.json");
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
    Some(merge(&ranges))
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

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / 1024.0 / 1024.0)
    } else if bytes >= 1024 {
        format!("{:.0}KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_done_active_and_queued() {
        let parts = paint_file_map(8, &[(0, 3)], &[(3, 5)]);
        assert_eq!(parts[0].state, "done");
        assert_eq!(parts[0].start, 0);
        assert_eq!(parts[0].end, 2);
        assert_eq!(parts[1].state, "active");
        assert_eq!(parts[2].state, "queued");
        let (workers, done, total, hint) = summarize(&parts);
        assert_eq!(workers, 1);
        assert_eq!(done, 1);
        assert_eq!(total, 3);
        assert!(hint.contains("已完成"));
        let cells = sample_cells(&parts, 8, 3, 8);
        assert_eq!(cells[0], 2);
        assert!(cells.contains(&1));
        assert!(cells.contains(&0));
    }
}
