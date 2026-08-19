//! Merge segmented WebVTT into one sidecar, matching Python `subtitles.py`.

use std::collections::HashSet;

const MPEGTS_CLOCK: f64 = 90_000.0;

#[derive(Clone, Debug)]
struct Cue {
    start: f64,
    end: f64,
    settings: String,
    payload: String,
}

pub fn merge_webvtt_segments(texts: &[String]) -> String {
    let mut merged = Vec::new();
    let mut seen = HashSet::new();
    for text in texts {
        let offset = timestamp_offset(text);
        for cue in parse_cues(text) {
            let start = (cue.start + offset).max(0.0);
            let end = (cue.end + offset).max(start);
            let key = (
                (start * 1000.0).round() as i64,
                (end * 1000.0).round() as i64,
                cue.payload.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            merged.push(Cue {
                start,
                end,
                settings: cue.settings,
                payload: cue.payload,
            });
        }
    }
    merged.sort_by(|left, right| {
        left.start
            .partial_cmp(&right.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                left.end
                    .partial_cmp(&right.end)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    let mut lines = vec!["WEBVTT".to_string(), String::new()];
    for cue in merged {
        let mut timing = format!(
            "{} --> {}",
            format_timestamp(cue.start, '.'),
            format_timestamp(cue.end, '.')
        );
        if !cue.settings.is_empty() {
            timing.push(' ');
            timing.push_str(&cue.settings);
        }
        lines.push(timing);
        lines.push(cue.payload);
        lines.push(String::new());
    }
    lines.join("\n")
}

pub fn webvtt_to_srt(vtt_text: &str) -> String {
    let mut lines = Vec::new();
    for (number, cue) in parse_cues(vtt_text).into_iter().enumerate() {
        lines.push((number + 1).to_string());
        lines.push(format!(
            "{} --> {}",
            format_timestamp(cue.start, ','),
            format_timestamp(cue.end, ',')
        ));
        lines.push(strip_vtt_only_tags(&cue.payload));
        lines.push(String::new());
    }
    lines.join("\n")
}

pub fn has_cues(vtt_text: &str) -> bool {
    !parse_cues(vtt_text).is_empty()
}

fn timestamp_offset(text: &str) -> f64 {
    let upper = text.to_ascii_uppercase();
    let Some(start) = upper.find("X-TIMESTAMP-MAP=") else {
        return 0.0;
    };
    let body = text[start + 16..].split(['\r', '\n']).next().unwrap_or("");
    let mut local = 0.0;
    let mut mpegts = 0.0;
    for part in body.split(',') {
        let (key, rest) = part.split_once(':').unwrap_or((part, ""));
        match key.trim().to_ascii_uppercase().as_str() {
            "LOCAL" => {
                if let Some(parsed) = parse_timestamp(rest) {
                    local = parsed;
                }
            }
            "MPEGTS" => {
                mpegts = rest.trim().parse().unwrap_or(0.0);
            }
            _ => {}
        }
    }
    mpegts / MPEGTS_CLOCK - local
}

fn parse_cues(text: &str) -> Vec<Cue> {
    let cleaned = text.replace('\u{feff}', "").replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in cleaned.split("\n\n") {
        let mut lines: Vec<&str> = block
            .split('\n')
            .map(|line| line.trim_end_matches('\r'))
            .filter(|line| !line.trim().is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        lines.retain(|line| {
            let upper = line.trim().to_ascii_uppercase();
            !(upper.starts_with("WEBVTT")
                || upper.starts_with("NOTE")
                || upper.starts_with("STYLE")
                || upper.starts_with("REGION")
                || upper.starts_with("X-TIMESTAMP-MAP"))
        });
        let Some(timing_at) = lines.iter().position(|line| is_timing_line(line)) else {
            continue;
        };
        let timing_line = lines[timing_at];
        let Some((start_raw, rest)) = timing_line.split_once("-->") else {
            continue;
        };
        let mut end_parts = rest.trim().splitn(2, char::is_whitespace);
        let end_raw = end_parts.next().unwrap_or("");
        let settings = end_parts.next().unwrap_or("").trim().to_string();
        let Some(start) = parse_timestamp(start_raw) else {
            continue;
        };
        let Some(end) = parse_timestamp(end_raw) else {
            continue;
        };
        let payload = lines[timing_at + 1..].join("\n").trim().to_string();
        if payload.is_empty() {
            continue;
        }
        cues.push(Cue {
            start,
            end,
            settings,
            payload,
        });
    }
    cues
}

fn is_timing_line(line: &str) -> bool {
    line.contains("-->") && parse_timestamp(line.split("-->").next().unwrap_or("")).is_some()
}

fn parse_timestamp(value: &str) -> Option<f64> {
    let text = value.trim();
    let (hms, millis) = text.split_once('.')?;
    let millis: f64 = millis.parse().ok()?;
    let parts: Vec<&str> = hms.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0, minutes.parse::<u32>().ok()?, seconds.parse::<u32>().ok()?),
        [hours, minutes, seconds] => (
            hours.parse::<u32>().ok()?,
            minutes.parse::<u32>().ok()?,
            seconds.parse::<u32>().ok()?,
        ),
        _ => return None,
    };
    Some(hours as f64 * 3600.0 + minutes as f64 * 60.0 + seconds as f64 + millis / 1000.0)
}

fn format_timestamp(value: f64, decimal: char) -> String {
    let total_ms = (value * 1000.0).round().max(0.0) as u64;
    let hours = total_ms / 3_600_000;
    let minutes = (total_ms % 3_600_000) / 60_000;
    let seconds = (total_ms % 60_000) / 1000;
    let millis = total_ms % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02}{decimal}{millis:03}")
}

fn strip_vtt_only_tags(payload: &str) -> String {
    let mut out = String::with_capacity(payload.len());
    let bytes = payload.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'<' {
            if let Some(end) = payload[index..].find('>') {
                let tag = &payload[index + 1..index + end];
                let name = tag
                    .trim_start_matches('/')
                    .split(|ch: char| ch == '.' || ch == ' ' || ch == '\t')
                    .next()
                    .unwrap_or("");
                if matches!(
                    name.to_ascii_lowercase().as_str(),
                    "v" | "c" | "lang" | "ruby" | "rt"
                ) {
                    index += end + 1;
                    continue;
                }
            }
        }
        out.push(payload[index..].chars().next().unwrap());
        index += payload[index..].chars().next().unwrap().len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_applies_timestamp_map_offset_and_dedupes_boundary_cues() {
        let first = "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:900000\n\n00:00.000 --> 00:02.000\n第一句\n\n00:02.000 --> 00:04.000\n第二句\n";
        let second = "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:900000\n\n00:02.000 --> 00:04.000\n第二句\n\n00:04.000 --> 00:06.000\n第三句\n";
        let merged = merge_webvtt_segments(&[first.into(), second.into()]);
        assert!(merged.starts_with("WEBVTT"));
        assert!(merged.contains("00:00:10.000 --> 00:00:12.000"));
        assert!(merged.contains("00:00:14.000 --> 00:00:16.000"));
        assert_eq!(merged.matches("第二句").count(), 1);
        assert!(has_cues(&merged));
    }

    #[test]
    fn merge_without_timestamp_map_keeps_original_times() {
        let merged = merge_webvtt_segments(&["WEBVTT\n\n00:00:01.500 --> 00:00:03.000\nhello\n".into()]);
        assert!(merged.contains("00:00:01.500 --> 00:00:03.000"));
    }

    #[test]
    fn webvtt_to_srt_strips_vtt_only_tags_and_uses_comma_decimals() {
        let vtt = "WEBVTT\n\n00:00:01.000 --> 00:00:02.500 align:center\n<v Speaker><i>你好</i></v>\n\n00:00:03.000 --> 00:00:04.000\n<c.yellow>world</c>\n";
        let srt = webvtt_to_srt(vtt);
        assert!(srt.contains("1\n00:00:01,000 --> 00:00:02,500\n<i>你好</i>"));
        assert!(srt.contains("2\n00:00:03,000 --> 00:00:04,000\nworld"));
        assert!(!srt.contains("<v"));
        assert!(!srt.contains("<c"));
    }
}
