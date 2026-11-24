//! IDM-style wildcard batch URL generator with O(tokens) estimation and cap enforcement.
//!
//! Supported range tokens inside a single URL line:
//! - Numeric ranges with optional zero-padding and optional step:
//!   `[001-050]`, `[1-100:2]`, `[10-1]`, `[10-01:2]`
//! - Same-case single-letter alpha ranges with optional step:
//!   `[a-z]`, `[A-F]`, `[z-a:2]`
//! - Multiple tokens per line expand via left-to-right Cartesian product.
//! - Invalid or non-range brackets (including IPv6 literals like `[::1]`) pass through unchanged.

/// Maximum total URLs allowed in a single batch expansion.
pub const MAX_BATCH_URLS: usize = 500;

/// Default hint shown in batch mode when no wildcard range is present.
pub const DEFAULT_BATCH_HINT: &str = "支持通配符：[001-050]、[1-100:2]、[a-z]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchExpansion {
    pub urls: Vec<String>,
    pub total_estimated: usize,
    pub has_wildcards: bool,
    pub truncated: bool,
    pub cap: usize,
}

impl BatchExpansion {
    /// Returns `(preview_text, is_wildcard_active, is_warning)` for the TaskEditor live preview bar.
    pub fn preview_status(&self) -> (String, bool, bool) {
        if !self.has_wildcards {
            if self.truncated {
                return (
                    format!(
                        "共 {} 个地址，超过单批上限 {} 个，请分批添加",
                        self.total_estimated, self.cap
                    ),
                    false,
                    true,
                );
            }
            return (DEFAULT_BATCH_HINT.to_owned(), false, false);
        }
        if self.truncated {
            let count_text = if self.total_estimated == usize::MAX {
                format!("超过 {}", self.cap)
            } else {
                format!("{}", self.total_estimated)
            };
            return (
                format!(
                    "将展开为 {} 个任务，超过单批上限 {} 个，请缩小范围",
                    count_text, self.cap
                ),
                true,
                true,
            );
        }
        match self.urls.as_slice() {
            [] => (DEFAULT_BATCH_HINT.to_owned(), false, false),
            [only] => (
                format!("将展开为 1 个任务（示例：{}）", preview_snippet(only)),
                true,
                false,
            ),
            [first, .., last] => (
                format!(
                    "将展开为 {} 个任务（示例：{} … {}）",
                    self.urls.len(),
                    preview_snippet(first),
                    preview_snippet(last)
                ),
                true,
                false,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Numeric {
        start: u64,
        end: u64,
        step: u64,
        pad_width: usize,
    },
    Alpha {
        start: u8,
        end: u8,
        step: u8,
    },
}

impl Segment {
    fn is_wildcard(&self) -> bool {
        !matches!(self, Self::Literal(_))
    }

    fn count(&self) -> usize {
        match self {
            Self::Literal(_) => 1,
            Self::Numeric {
                start, end, step, ..
            } => {
                let diff = start.abs_diff(*end);
                usize::try_from(diff / *step)
                    .ok()
                    .and_then(|steps| steps.checked_add(1))
                    .unwrap_or(usize::MAX)
            }
            Self::Alpha { start, end, step } => {
                let diff = start.abs_diff(*end);
                (diff / *step) as usize + 1
            }
        }
    }

    fn format_at(&self, index: usize, out: &mut String) {
        match self {
            Self::Literal(text) => out.push_str(text),
            Self::Numeric {
                start,
                end,
                step,
                pad_width,
            } => {
                let delta = (index as u64).saturating_mul(*step);
                let value = if start <= end {
                    start.saturating_add(delta)
                } else {
                    start.saturating_sub(delta)
                };
                if *pad_width > 0 {
                    use std::fmt::Write as _;
                    let _ = write!(out, "{value:0width$}", width = *pad_width);
                } else {
                    use std::fmt::Write as _;
                    let _ = write!(out, "{value}");
                }
            }
            Self::Alpha { start, end, step } => {
                let delta = (index as u8).saturating_mul(*step);
                let byte = if start <= end {
                    start.saturating_add(delta)
                } else {
                    start.saturating_sub(delta)
                };
                out.push(byte as char);
            }
        }
    }
}

fn parse_range_token(inner: &str) -> Option<Segment> {
    if inner.is_empty() {
        return None;
    }
    let (range_part, step_part) = match inner.split_once(':') {
        Some((range, step)) => {
            if step.is_empty() || step.contains(':') {
                return None;
            }
            (range, Some(step))
        }
        None => (inner, None),
    };
    let (start_str, end_str) = range_part.split_once('-')?;
    if start_str.is_empty() || end_str.is_empty() || end_str.contains('-') {
        return None;
    }

    // Numeric range: both sides must be ASCII digits only.
    if start_str.bytes().all(|b| b.is_ascii_digit()) && end_str.bytes().all(|b| b.is_ascii_digit())
    {
        let start: u64 = start_str.parse().ok()?;
        let end: u64 = end_str.parse().ok()?;
        let step: u64 = match step_part {
            Some(raw) => {
                if !raw.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let parsed: u64 = raw.parse().ok()?;
                if parsed == 0 {
                    return None;
                }
                parsed
            }
            None => 1,
        };
        let start_padded = start_str.len() > 1 && start_str.starts_with('0');
        let end_padded = end_str.len() > 1 && end_str.starts_with('0');
        let pad_width = match (start_padded, end_padded) {
            (true, true) => start_str.len().max(end_str.len()),
            (true, false) => start_str.len(),
            (false, true) => end_str.len(),
            (false, false) => 0,
        };
        return Some(Segment::Numeric {
            start,
            end,
            step,
            pad_width,
        });
    }

    // Single-letter same-case ASCII alpha range: [a-z], [A-F], [z-a:2].
    if start_str.len() == 1 && end_str.len() == 1 {
        let start = start_str.as_bytes()[0];
        let end = end_str.as_bytes()[0];
        let same_case = (start.is_ascii_lowercase() && end.is_ascii_lowercase())
            || (start.is_ascii_uppercase() && end.is_ascii_uppercase());
        if !same_case {
            return None;
        }
        let step: u8 = match step_part {
            Some(raw) => {
                if !raw.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let parsed: u64 = raw.parse().ok()?;
                if parsed == 0 {
                    return None;
                }
                u8::try_from(parsed).ok()?
            }
            None => 1,
        };
        return Some(Segment::Alpha { start, end, step });
    }

    None
}

fn parse_line(line: &str) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut literal = String::new();
    let bytes = line.as_bytes();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        let Some(rel_open) = line[cursor..].find('[') else {
            literal.push_str(&line[cursor..]);
            break;
        };
        let open = cursor + rel_open;
        literal.push_str(&line[cursor..open]);

        // Scan forward for a closing `]` before another `[`.
        let rest = &line[open + 1..];
        let next_open = rest.find('[');
        let next_close = rest.find(']');
        match (next_open, next_close) {
            (Some(no), Some(nc)) if no < nc => {
                // Nested/unclosed `[` before `]`: treat outer `[` + prefix as literal.
                literal.push_str(&line[open..open + 1 + no]);
                cursor = open + 1 + no;
            }
            (_, Some(nc)) => {
                let close = open + 1 + nc;
                let inner = &line[open + 1..close];
                if let Some(token) = parse_range_token(inner) {
                    if !literal.is_empty() {
                        segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    }
                    segments.push(token);
                } else {
                    literal.push_str(&line[open..=close]);
                }
                cursor = close + 1;
            }
            (_, None) => {
                literal.push_str(&line[open..]);
                break;
            }
        }
    }

    if !literal.is_empty() || segments.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    segments
}

fn expand_segments_into(
    segments: &[Segment],
    index: usize,
    buffer: &mut String,
    out: &mut Vec<String>,
    cap: usize,
) {
    if out.len() >= cap {
        return;
    }
    let Some(segment) = segments.get(index) else {
        out.push(buffer.clone());
        return;
    };
    let saved_len = buffer.len();
    let count = segment.count();
    for step_idx in 0..count {
        if out.len() >= cap {
            break;
        }
        buffer.truncate(saved_len);
        segment.format_at(step_idx, buffer);
        expand_segments_into(segments, index + 1, buffer, out, cap);
    }
    buffer.truncate(saved_len);
}

/// Expands multi-line batch input using IDM-style wildcard patterns with a safety cap.
pub fn expand_batch(input: &str, cap: usize) -> BatchExpansion {
    let mut urls = Vec::new();
    let mut total_estimated = 0usize;
    let mut has_wildcards = false;

    for raw_line in input.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let segments = parse_line(line);
        let line_has_wildcards = segments.iter().any(Segment::is_wildcard);
        has_wildcards |= line_has_wildcards;

        let line_count = segments
            .iter()
            .fold(1usize, |acc, seg| acc.saturating_mul(seg.count()));
        total_estimated = total_estimated.saturating_add(line_count);

        if urls.len() < cap {
            if !line_has_wildcards {
                urls.push(line.to_owned());
            } else {
                let mut buffer = String::with_capacity(line.len());
                expand_segments_into(&segments, 0, &mut buffer, &mut urls, cap);
            }
        }
    }

    let truncated = total_estimated > cap;
    BatchExpansion {
        urls,
        total_estimated,
        has_wildcards,
        truncated,
        cap,
    }
}

fn preview_snippet(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let candidate = if let Some((_, rest)) = without_query.split_once("://") {
        rest
    } else {
        without_query
    };
    let chars: Vec<char> = candidate.chars().collect();
    if chars.len() <= 38 {
        candidate.to_owned()
    } else {
        let head: String = chars[..16].iter().collect();
        let tail: String = chars[chars.len() - 19..].iter().collect();
        format!("{head}…{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_padded_numeric_ranges_preserve_width_and_support_descending() {
        let asc = expand_batch("https://cdn.test/ep_[001-004].mp4", MAX_BATCH_URLS);
        assert!(asc.has_wildcards);
        assert!(!asc.truncated);
        assert_eq!(asc.total_estimated, 4);
        assert_eq!(
            asc.urls,
            vec![
                "https://cdn.test/ep_001.mp4",
                "https://cdn.test/ep_002.mp4",
                "https://cdn.test/ep_003.mp4",
                "https://cdn.test/ep_004.mp4",
            ]
        );

        let desc = expand_batch("https://cdn.test/img_[04-01].jpg", MAX_BATCH_URLS);
        assert_eq!(
            desc.urls,
            vec![
                "https://cdn.test/img_04.jpg",
                "https://cdn.test/img_03.jpg",
                "https://cdn.test/img_02.jpg",
                "https://cdn.test/img_01.jpg",
            ]
        );

        let plain_desc = expand_batch("https://cdn.test/n_[10-8].bin", MAX_BATCH_URLS);
        assert_eq!(
            plain_desc.urls,
            vec![
                "https://cdn.test/n_10.bin",
                "https://cdn.test/n_9.bin",
                "https://cdn.test/n_8.bin",
            ]
        );
    }

    #[test]
    fn numeric_ranges_with_steps_expand_accurately() {
        let stepped = expand_batch("https://cdn.test/p_[1-10:2].bin", MAX_BATCH_URLS);
        assert!(stepped.has_wildcards);
        assert_eq!(
            stepped.urls,
            vec![
                "https://cdn.test/p_1.bin",
                "https://cdn.test/p_3.bin",
                "https://cdn.test/p_5.bin",
                "https://cdn.test/p_7.bin",
                "https://cdn.test/p_9.bin",
            ]
        );

        let padded_step = expand_batch("https://cdn.test/p_[001-010:3].bin", MAX_BATCH_URLS);
        assert_eq!(
            padded_step.urls,
            vec![
                "https://cdn.test/p_001.bin",
                "https://cdn.test/p_004.bin",
                "https://cdn.test/p_007.bin",
                "https://cdn.test/p_010.bin",
            ]
        );

        let desc_step = expand_batch("https://cdn.test/p_[10-1:3].bin", MAX_BATCH_URLS);
        assert_eq!(
            desc_step.urls,
            vec![
                "https://cdn.test/p_10.bin",
                "https://cdn.test/p_7.bin",
                "https://cdn.test/p_4.bin",
                "https://cdn.test/p_1.bin",
            ]
        );
    }

    #[test]
    fn alpha_ranges_support_lower_upper_descending_and_steps() {
        let lower = expand_batch("https://cdn.test/part_[a-d].zip", MAX_BATCH_URLS);
        assert!(lower.has_wildcards);
        assert_eq!(
            lower.urls,
            vec![
                "https://cdn.test/part_a.zip",
                "https://cdn.test/part_b.zip",
                "https://cdn.test/part_c.zip",
                "https://cdn.test/part_d.zip",
            ]
        );

        let upper = expand_batch("https://cdn.test/vol_[A-F:2].iso", MAX_BATCH_URLS);
        assert_eq!(
            upper.urls,
            vec![
                "https://cdn.test/vol_A.iso",
                "https://cdn.test/vol_C.iso",
                "https://cdn.test/vol_E.iso",
            ]
        );

        let desc = expand_batch("https://cdn.test/[d-b].bin", MAX_BATCH_URLS);
        assert_eq!(
            desc.urls,
            vec![
                "https://cdn.test/d.bin",
                "https://cdn.test/c.bin",
                "https://cdn.test/b.bin",
            ]
        );
    }

    #[test]
    fn multi_pattern_lines_expand_cartesian_left_to_right() {
        let expanded = expand_batch("https://cdn.test/[A-B]/ch_[01-03].png", MAX_BATCH_URLS);
        assert!(expanded.has_wildcards);
        assert_eq!(expanded.total_estimated, 6);
        assert_eq!(
            expanded.urls,
            vec![
                "https://cdn.test/A/ch_01.png",
                "https://cdn.test/A/ch_02.png",
                "https://cdn.test/A/ch_03.png",
                "https://cdn.test/B/ch_01.png",
                "https://cdn.test/B/ch_02.png",
                "https://cdn.test/B/ch_03.png",
            ]
        );
    }

    #[test]
    fn literal_lines_and_invalid_brackets_pass_through_unchanged() {
        let input = "\
http://127.0.0.1:8080/batch-1.bin
http://127.0.0.1:8080/batch-2.bin
http://[::1]:8080/ipv6.bin
https://cdn.test/[invalid]/a.bin
https://cdn.test/[1-5:0]/zero-step.bin
https://cdn.test/[a-Z]/mixed-case.bin
https://cdn.test/[unclosed-1-3.bin
https://cdn.test/[]/empty.bin
";
        let expanded = expand_batch(input, MAX_BATCH_URLS);
        assert!(!expanded.has_wildcards);
        assert!(!expanded.truncated);
        assert_eq!(expanded.urls.len(), 8);
        assert_eq!(expanded.urls[0], "http://127.0.0.1:8080/batch-1.bin");
        assert_eq!(expanded.urls[2], "http://[::1]:8080/ipv6.bin");
        assert_eq!(expanded.urls[3], "https://cdn.test/[invalid]/a.bin");
        assert_eq!(expanded.urls[4], "https://cdn.test/[1-5:0]/zero-step.bin");
        assert_eq!(expanded.urls[5], "https://cdn.test/[a-Z]/mixed-case.bin");
        assert_eq!(expanded.urls[6], "https://cdn.test/[unclosed-1-3.bin");
        assert_eq!(expanded.urls[7], "https://cdn.test/[]/empty.bin");

        let (text, active, warn) = expanded.preview_status();
        assert_eq!(text, DEFAULT_BATCH_HINT);
        assert!(!active);
        assert!(!warn);

        // Unbalanced outer bracket before a valid range still expands the inner range.
        let nested = expand_batch("https://cdn.test/a[b[1-2].bin", MAX_BATCH_URLS);
        assert!(nested.has_wildcards);
        assert_eq!(
            nested.urls,
            vec!["https://cdn.test/a[b1.bin", "https://cdn.test/a[b2.bin"]
        );
    }

    #[test]
    fn cap_enforcement_bounds_allocations_and_reports_warning() {
        let over = expand_batch("https://cdn.test/f_[001-600].bin", MAX_BATCH_URLS);
        assert!(over.has_wildcards);
        assert!(over.truncated);
        assert_eq!(over.total_estimated, 600);
        assert_eq!(over.urls.len(), MAX_BATCH_URLS);
        let (preview, active, warning) = over.preview_status();
        assert!(active);
        assert!(warning);
        assert!(preview.contains("600"));
        assert!(preview.contains("500"));

        // Huge Cartesian range must not OOM or spin.
        let huge = expand_batch(
            "https://cdn.test/[1-999999]/[1-999999]/[a-z].bin",
            MAX_BATCH_URLS,
        );
        assert!(huge.truncated);
        assert_eq!(huge.urls.len(), MAX_BATCH_URLS);
        let (_, active, warning) = huge.preview_status();
        assert!(active);
        assert!(warning);
    }

    #[test]
    fn preview_status_shows_count_and_first_last_examples() {
        let expanded = expand_batch("https://cdn.test/ep_[001-050].mp4", MAX_BATCH_URLS);
        let (text, active, warning) = expanded.preview_status();
        assert!(active);
        assert!(!warning);
        assert!(text.contains("将展开为 50 个任务"), "{text}");
        assert!(text.contains("ep_001.mp4"), "{text}");
        assert!(text.contains("ep_050.mp4"), "{text}");
    }
}
