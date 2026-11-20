//! Native DASH downloader. Compatible tracks are stitched in Period order.

use crate::http_engine::{fetch_bytes_limited, run_job, Job};
use crate::media::merge::merge_with_ffmpeg;
use crate::media::subtitles::{has_cues, merge_webvtt_segments, webvtt_to_srt};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct DashManifest {
    pub dynamic: bool,
    pub period_count: usize,
    pub representations: Vec<Representation>,
    periods: Vec<DashPeriod>,
    expired_periods: HashSet<usize>,
}

#[derive(Debug, Clone, PartialEq)]
struct DashPeriod {
    id: Option<String>,
    start: Option<f64>,
}

impl DashPeriod {
    fn same_as(&self, other: &Self) -> bool {
        match (&self.id, &other.id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.start == other.start,
            _ => self.start.is_some() && self.start == other.start,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Representation {
    pub id: String,
    pub bandwidth: u64,
    pub height: u32,
    pub mime: String,
    pub lang: String,
    pub codecs: String,
    pub content_type: String,
    pub label: String,
    pub base_url: String,
    pub init: Option<String>,
    pub init_range: Option<ByteRange>,
    pub index_range: Option<ByteRange>,
    pub media: Vec<String>,
    pub media_ranges: Vec<Option<ByteRange>>,
    pub media_durations: Vec<f64>,
    /// Zero-based index of the Period this representation belongs to.
    pub period: usize,
    /// Indices in `media` that are a later Period's initialization segment
    /// (stitched multi-period tracks only).
    pub init_markers: Vec<usize>,
    /// Later Period boundaries that reuse the current initialization (or have none).
    pub period_markers: Vec<usize>,
    /// SegmentBase without indexRange: the whole BaseURL is one segment.
    pub whole_file: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteRange {
    pub start: u64,
    pub length: u64,
}

// Bound the fetched index, using the original CSidxParser's buffer budget.
const MAX_SIDX_BYTES: u64 = 3_000_000;

fn sidx_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or("truncated SIDX field")?;
    Ok(u32::from_be_bytes(
        value.try_into().map_err(|_| "truncated SIDX field")?,
    ))
}

fn parse_sidx(bytes: &[u8], file_offset: u64) -> Result<Vec<(ByteRange, f64)>, String> {
    if bytes.len() as u64 > MAX_SIDX_BYTES {
        return Err("DASH SIDX exceeds index byte limit".into());
    }
    let mut cursor = 0;
    while cursor < bytes.len() {
        let header = bytes
            .get(cursor..cursor + 8)
            .ok_or("truncated SIDX box header")?;
        let short_size = sidx_u32(header, 0)?;
        let (size, header_size) = match short_size {
            0 => return Err("unbounded SIDX box size".into()),
            1 => (
                (u64::from(sidx_u32(bytes, cursor + 8)?) << 32)
                    | u64::from(sidx_u32(bytes, cursor + 12)?),
                16,
            ),
            size => (u64::from(size), 8),
        };
        if size < header_size as u64 || size > (bytes.len() - cursor) as u64 {
            return Err("invalid or truncated SIDX box size".into());
        }
        let end = cursor + size as usize;
        if &header[4..8] != b"sidx" {
            cursor = end;
            continue;
        }
        let payload = &bytes[cursor + header_size..end];
        let flags = sidx_u32(payload, 0)?;
        let version = flags >> 24;
        if version > 1 || flags & 0x00ff_ffff != 0 {
            return Err("unsupported SIDX version or flags".into());
        }
        let timescale = sidx_u32(payload, 8)?;
        if timescale == 0 {
            return Err("invalid SIDX timescale".into());
        }
        let (first_offset, count_offset) = if version == 0 {
            (u64::from(sidx_u32(payload, 16)?), 22)
        } else {
            (
                (u64::from(sidx_u32(payload, 20)?) << 32) | u64::from(sidx_u32(payload, 24)?),
                30,
            )
        };
        let count_bytes = payload
            .get(count_offset..count_offset + 2)
            .ok_or("truncated SIDX reference count")?;
        let count = u16::from_be_bytes([count_bytes[0], count_bytes[1]]) as usize;
        if count == 0 {
            return Err("empty SIDX references".into());
        }
        let entries = &payload[count_offset + 2..];
        if entries.len() != count * 12 {
            return Err("invalid SIDX reference table length".into());
        }
        // first_offset is relative to the end of this box, not the requested
        // index range. A range can contain preceding or trailing boxes.
        let mut start = file_offset
            .checked_add(end as u64)
            .and_then(|offset| offset.checked_add(first_offset))
            .ok_or("SIDX offset overflow")?;
        let mut segments = Vec::with_capacity(count);
        for entry in entries.as_chunks::<12>().0 {
            let reference = sidx_u32(entry, 0)?;
            if reference & 0x8000_0000 != 0 {
                return Err("unsupported hierarchical SIDX reference".into());
            }
            let length = u64::from(reference & 0x7fff_ffff);
            let duration = sidx_u32(entry, 4)?;
            if length == 0 || duration == 0 {
                return Err("empty SIDX media reference".into());
            }
            let next = start
                .checked_add(length)
                .ok_or("SIDX media offset overflow")?;
            segments.push((
                ByteRange { start, length },
                f64::from(duration) / f64::from(timescale),
            ));
            start = next;
        }
        return Ok(segments);
    }
    Err("DASH index contains no SIDX box".into())
}

fn expand_segment_base(
    track: &mut Representation,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
) -> Result<(), String> {
    let Some(range) = track.index_range else {
        return Ok(());
    };
    if range.length > MAX_SIDX_BYTES {
        return Err("DASH SIDX exceeds index byte limit".into());
    }
    if read_control(control) != "run" {
        return Err(control_stop_error(control));
    }
    let bytes = crate::http_engine::fetch_index_bytes_range(
        &track.base_url,
        headers,
        proxy,
        range.start,
        range.length,
        control,
    )
    .map_err(|error| error.to_string())?;
    let segments = parse_sidx(&bytes, range.start)?;
    track.media = vec![track.base_url.clone(); segments.len()];
    track.media_ranges = segments.iter().map(|(range, _)| Some(*range)).collect();
    track.media_durations = segments.into_iter().map(|(_, duration)| duration).collect();
    track.index_range = None;
    Ok(())
}

fn parse_byte_range(value: &str) -> Result<ByteRange, String> {
    let (start, end) = value.split_once('-').ok_or("invalid DASH byte range")?;
    if start.is_empty()
        || end.is_empty()
        || !start.bytes().chain(end.bytes()).all(|b| b.is_ascii_digit())
    {
        return Err("invalid DASH byte range".into());
    }
    let start = start
        .parse::<u64>()
        .map_err(|_| "invalid DASH byte range")?;
    let length = end
        .parse::<u64>()
        .ok()
        .and_then(|end| end.checked_sub(start))
        .and_then(|n| n.checked_add(1))
        .ok_or("invalid DASH byte range")?;
    Ok(ByteRange { start, length })
}

pub fn parse_mpd(xml: &str, base: &str) -> Result<DashManifest, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs_f64();
    parse_mpd_at(xml, base, now)
}

fn parse_mpd_at(xml: &str, base: &str, now: f64) -> Result<DashManifest, String> {
    let document =
        roxmltree::Document::parse(xml).map_err(|error| format!("invalid DASH XML: {error}"))?;
    let root = document.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case("MPD") {
        return Err("not a DASH manifest".into());
    }
    let dynamic = root.attribute("type") == Some("dynamic");
    let periods: Vec<_> = root
        .children()
        .filter(|node| mpd_element(*node, "Period", root))
        .collect();
    let period_durations = period_durations(&periods, root);
    let mut representations = Vec::new();
    let mut live_segment_budget = 1200;
    let mut expired_periods = HashSet::new();
    for node in root
        .descendants()
        .filter(|node| mpd_element(*node, "Representation", root))
    {
        let period = node
            .ancestors()
            .find(|node| mpd_element(*node, "Period", root));
        let set = node
            .ancestors()
            .find(|node| mpd_element(*node, "AdaptationSet", root));
        let period_index = period
            .and_then(|period| periods.iter().position(|item| *item == period))
            .unwrap_or(0);
        let period_duration = period_durations.get(period_index).copied().flatten();
        let id = node.attribute("id").unwrap_or("0").to_string();
        let bandwidth = node
            .attribute("bandwidth")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let height = node
            .attribute("height")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let inherited = |key: &str| {
            node.attribute(key)
                .or_else(|| set.and_then(|set| set.attribute(key)))
                .unwrap_or_default()
                .to_string()
        };
        let mime = node
            .attribute("mimeType")
            .filter(|value| !value.is_empty())
            .or_else(|| set.and_then(|set| set.attribute("mimeType")))
            .unwrap_or_default()
            .to_string();
        // 每层只取直接子节点，避免把相邻 Representation 的 CDN 地址串到当前轨道。
        let scopes = [Some(root), period, set, Some(node)];
        let mut base_url = base.to_string();
        for scope in scopes.into_iter().flatten() {
            if let Some(child) = mpd_child(scope, "BaseURL") {
                let value: String = child
                    .children()
                    .filter(|node| node.is_text())
                    .filter_map(|node| node.text())
                    .collect();
                base_url = resolve(&base_url, &value);
                if base_url.is_empty() {
                    return Err("invalid DASH BaseURL".into());
                }
            }
        }
        let mut segment_lists = Vec::new();
        let mut templates = Vec::new();
        let mut segment_bases = Vec::new();
        for scope in scopes.into_iter().rev().flatten() {
            if let Some(list) = mpd_child(scope, "SegmentList") {
                if templates.is_empty() && segment_bases.is_empty() {
                    segment_lists.push(list);
                    continue;
                }
                break;
            }
            if let Some(template) = mpd_child(scope, "SegmentTemplate") {
                if !segment_bases.is_empty() || !segment_lists.is_empty() {
                    break;
                }
                templates.push(template);
            }
            if let Some(base) = mpd_child(scope, "SegmentBase") {
                if !templates.is_empty() || !segment_lists.is_empty() {
                    break;
                }
                segment_bases.push(base);
            }
        }
        let template_attr = |key: &str| {
            templates
                .iter()
                .find_map(|template| template.attribute(key))
        };
        let list_attr = |key: &str| segment_lists.iter().find_map(|list| list.attribute(key));
        let has_timeline = templates
            .iter()
            .any(|template| mpd_child(*template, "SegmentTimeline").is_some());
        let index_range = segment_bases
            .iter()
            .find_map(|base| base.attribute("indexRange"))
            .map(parse_byte_range)
            .transpose()?;
        if segment_bases
            .iter()
            .any(|base| mpd_child(*base, "RepresentationIndex").is_some())
        {
            return Err("unsupported external DASH RepresentationIndex".into());
        }
        let whole_file = !segment_bases.is_empty() && index_range.is_none();
        let initialization = segment_lists
            .iter()
            .find_map(|list| mpd_child(*list, "Initialization"))
            .or_else(|| {
                segment_bases
                    .iter()
                    .find_map(|base| mpd_child(*base, "Initialization"))
            });
        let init_range = initialization
            .and_then(|node| node.attribute("range"))
            .map(parse_byte_range)
            .transpose()?;
        let init = if let Some(initialization) = initialization {
            let source = initialization.attribute("sourceURL").unwrap_or("");
            Some(if source.is_empty() {
                base_url.clone()
            } else {
                resolve(&base_url, source)
            })
        } else {
            template_attr("initialization")
                .or_else(|| node.attribute("initialization"))
                .map(|value| {
                    apply_template(value, &id, 0, 0, bandwidth)
                        .map(|expanded| resolve(&base_url, &expanded))
                })
                .transpose()?
        };
        if init_range.is_some() && init.as_ref().is_none_or(|url| url.is_empty()) {
            return Err("invalid DASH initialization URL".into());
        }
        // SegmentURL 的 media 是显式地址，不能当作 SegmentTemplate 重复展开。
        let media_template = if !segment_lists.is_empty() {
            None
        } else {
            template_attr("media")
        };
        let mut media = Vec::new();
        let mut media_ranges = Vec::new();
        let mut media_durations = Vec::new();
        if let Some(template) = media_template {
            if dynamic && !has_timeline {
                let mut expired = false;
                let mut start = Some(0.0);
                for (index, period) in periods.iter().enumerate().take(period_index + 1) {
                    start = period
                        .attribute("start")
                        .and_then(parse_duration_seconds)
                        .or(start);
                    if index < period_index {
                        start = start
                            .zip(period_durations[index])
                            .map(|(start, duration)| start + duration);
                    }
                }
                media.extend(expand_live_duration(
                    root,
                    &templates,
                    template,
                    &base_url,
                    &id,
                    bandwidth,
                    start.ok_or("dynamic DASH requires a resolvable Period start")?,
                    period_duration,
                    now,
                    &mut media_durations,
                    &mut live_segment_budget,
                    &mut expired,
                )?);
                if expired {
                    expired_periods.insert(period_index);
                }
            } else {
                media.extend(expand_timeline(
                    &templates,
                    template,
                    &base_url,
                    &id,
                    bandwidth,
                    period_duration.filter(|_| !dynamic || has_timeline),
                    &mut media_durations,
                )?);
            }
            media_ranges.resize(media.len(), None);
        }
        let list = segment_lists
            .iter()
            .copied()
            .find(|list| {
                list.children()
                    .any(|child| mpd_element(child, "SegmentURL", root))
            })
            .unwrap_or(node);
        for child in list
            .children()
            .filter(|child| mpd_element(*child, "SegmentURL", root))
        {
            let range = child
                .attribute("mediaRange")
                .map(parse_byte_range)
                .transpose()?;
            if let Some(source) = child.attribute("media").or(range.map(|_| "")) {
                let url = if source.is_empty() && range.is_some() {
                    base_url.clone()
                } else {
                    resolve(&base_url, source)
                };
                if url.is_empty() {
                    return Err("invalid DASH segment URL".into());
                }
                media.push(url);
                media_ranges.push(range);
            }
        }
        let whole_file = whole_file && media.is_empty();
        let (init, init_range) = if whole_file {
            // SegmentBase 没有 indexRange 时整份 BaseURL 就是一个分片；同址的
            // Initialization 已包含在整文件里，不能重复拼接。
            media.push(base_url.clone());
            media_ranges.push(None);
            if let Some(seconds) = period_duration.filter(|value| value.is_finite() && *value > 0.0)
            {
                media_durations = vec![seconds];
            }
            if init.as_deref() == Some(base_url.as_str()) {
                (None, None)
            } else {
                (init, init_range)
            }
        } else {
            (init, init_range)
        };
        if !segment_lists.is_empty() {
            let duration = list_attr("duration").and_then(|value| value.parse::<f64>().ok());
            let timescale = list_attr("timescale")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(1.0);
            for key in ["presentationTimeOffset", "startNumber"] {
                if list_attr(key).is_some_and(|value| value.parse::<u64>().is_err()) {
                    return Err(format!("invalid DASH SegmentList {key}"));
                }
            }
            if timescale <= 0.0 || !timescale.is_finite() {
                return Err("invalid DASH SegmentList timescale".into());
            }
            if let Some(seconds) = duration
                .map(|duration| duration / timescale)
                .filter(|value| value.is_finite() && *value > 0.0)
            {
                media_durations = vec![seconds; media.len()];
            }
        }
        representations.push(Representation {
            id,
            bandwidth,
            height,
            mime,
            lang: inherited("lang"),
            codecs: inherited("codecs"),
            content_type: inherited("contentType"),
            label: inherited("label"),
            base_url,
            init: init.filter(|url| !url.is_empty()),
            init_range,
            index_range,
            media,
            media_ranges,
            media_durations,
            period: period_index,
            init_markers: Vec::new(),
            period_markers: Vec::new(),
            whole_file,
        });
    }
    if representations.is_empty() {
        return Err("DASH manifest has no representations".into());
    }
    Ok(DashManifest {
        dynamic,
        expired_periods,
        period_count: periods.len(),
        periods: periods
            .iter()
            .map(|period| DashPeriod {
                id: period
                    .attribute("id")
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned),
                start: period.attribute("start").and_then(parse_duration_seconds),
            })
            .collect(),
        representations,
    })
}

pub fn representation_choices(manifest: &DashManifest) -> Vec<crate::StreamVariant> {
    let mut choices: Vec<_> = manifest
        .representations
        .iter()
        .filter(|item| item.period == 0 && is_video(item))
        .map(|item| crate::StreamVariant {
            label: if item.height > 0 {
                format!("{}p · {} kbps", item.height, item.bandwidth / 1000)
            } else {
                format!("{} kbps", item.bandwidth / 1000)
            },
            bandwidth: item.bandwidth,
            height: item.height,
            kind: "video".into(),
            name: item.id.clone(),
        })
        .collect();
    choices.sort_by_key(|item| u64::MAX - item.bandwidth);
    choices
}

pub fn audio_choices(manifest: &DashManifest) -> Vec<crate::StreamVariant> {
    let mut choices: Vec<_> = manifest
        .representations
        .iter()
        .filter(|item| item.period == 0 && is_audio(item))
        .map(|item| crate::StreamVariant {
            label: {
                let title = if item.label.is_empty() {
                    item.id.as_str()
                } else {
                    item.label.as_str()
                };
                if item.lang.is_empty() {
                    title.to_string()
                } else {
                    format!("{title} · {}", item.lang)
                }
            },
            bandwidth: item.bandwidth,
            height: 0,
            kind: "audio".into(),
            name: if item.label.is_empty() {
                item.id.clone()
            } else {
                item.label.clone()
            },
        })
        .collect();
    choices.sort_by_key(|item| item.label.clone());
    choices
}

fn select_video(
    representations: &[Representation],
    preferred_bandwidth: u64,
    preferred_height: u32,
) -> Option<&Representation> {
    let videos: Vec<_> = representations
        .iter()
        .filter(|item| is_video(item))
        .collect();
    if preferred_bandwidth > 0 {
        videos.into_iter().min_by_key(|item| {
            (
                item.bandwidth.abs_diff(preferred_bandwidth),
                item.height.abs_diff(preferred_height),
            )
        })
    } else if preferred_height > 0 {
        videos.into_iter().min_by_key(|item| {
            (
                item.height.abs_diff(preferred_height),
                std::cmp::Reverse(item.bandwidth),
            )
        })
    } else {
        videos.into_iter().max_by_key(|item| item.bandwidth)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn download_dash_selected(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    preferred_bandwidth: u64,
    preferred_height: u32,
    download_subtitles: bool,
    preferred_audio: &str,
) -> Result<PathBuf, String> {
    let (status, body) = fetch_bytes_limited(url, headers, proxy, super::MAX_MANIFEST_BYTES)
        .map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Err(format!("DASH manifest HTTP {status}"));
    }
    let xml = String::from_utf8_lossy(&body).into_owned();
    let manifest = parse_mpd(&xml, url)?;
    let mut vod = DashCheckpoint::load(task_dir);
    fs::create_dir_all(task_dir).map_err(|error| error.to_string())?;
    if manifest.dynamic {
        return download_live_dash(
            manifest,
            xml,
            url,
            headers,
            proxy,
            task_dir,
            control,
            preferred_bandwidth,
            preferred_height,
            download_subtitles,
            preferred_audio,
            &mut vod,
        );
    }

    // 后续 Period 跟随首个 Period 的轨道选择。
    let first_period: Vec<_> = manifest
        .representations
        .iter()
        .filter(|item| item.period == 0)
        .cloned()
        .collect();
    let video = select_video(&first_period, preferred_bandwidth, preferred_height)
        .cloned()
        .map(|track| expand_periods(&manifest, track, headers, proxy, control))
        .transpose()?;
    let audio = select_audio(&first_period, preferred_audio)
        .cloned()
        .map(|track| expand_periods(&manifest, track, headers, proxy, control))
        .transpose()?;
    let subtitles = if download_subtitles {
        select_subtitles(&first_period)
            .into_iter()
            .filter_map(|track| expand_periods(&manifest, track, headers, proxy, control).ok())
            .filter(|track| track.init_markers.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    if read_control(control) != "run" {
        return Err(control_stop_error(control));
    }
    let track = video
        .as_ref()
        .or(audio.as_ref())
        .ok_or("DASH has no audio or video representation")?;
    if video
        .iter()
        .chain(audio.iter())
        .any(|item| item.media.is_empty())
    {
        return Err("DASH representation produced no media segments".into());
    }
    let total = video
        .iter()
        .chain(audio.iter())
        .map(|track| (usize::from(track.init.is_some()) + track.media.len()) as u64)
        .sum();
    let (files, merged_audio) = download_dash_primary(
        track,
        video.is_some(),
        task_dir,
        headers,
        proxy,
        control,
        &mut vod,
        audio.as_ref().filter(|_| video.is_some()),
        total,
    )?;
    write_dash_playlist(task_dir, &files)?;
    finish_dash(
        task_dir,
        merged_audio,
        subtitles,
        headers,
        proxy,
        control,
        &mut vod,
    )
}

#[derive(Default)]
struct LiveDashTrack {
    files: Vec<PathBuf>,
    durations: Vec<f64>,
    seen: HashSet<(String, Option<ByteRange>)>,
    init: Option<(String, Option<ByteRange>)>,
    bytes: u64,
}

impl LiveDashTrack {
    #[allow(clippy::too_many_arguments)]
    fn append(
        &mut self,
        track: &Representation,
        next_period: bool,
        kind: &str,
        dir: &Path,
        task_dir: &Path,
        headers: &HashMap<String, String>,
        proxy: &str,
        control: &Path,
        vod: &mut DashCheckpoint,
        checkpoint_scope: Option<&str>,
        other: &Self,
    ) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        let init = track.init.clone().map(|url| (url, track.init_range));
        let initial = self.files.is_empty();
        let change_init = init != self.init;
        if !initial && ((!next_period && change_init) || (change_init && init.is_none())) {
            return Err("直播 DASH 初始化段改变，无法安全接续当前轨道".into());
        }
        if next_period {
            self.seen.clear();
        }
        let first_media = track
            .media
            .iter()
            .enumerate()
            .rfind(|(index, url)| {
                self.seen
                    .contains(&((*url).clone(), track.media_ranges[*index]))
            })
            .map_or(0, |(index, _)| index + 1);
        let checkpoint_id = checkpoint_scope.map(|scope| format!("{scope}:{}", track.id));
        let mut index = self.files.len();
        let mut push =
            |url: &str, range, name: String, duration: Option<f64>| -> Result<(), String> {
                if read_control(control) != "run" {
                    return Err(control_stop_error(control));
                }
                let path = resume_or_fetch(
                    url,
                    range,
                    headers,
                    proxy,
                    &dir.join(&name),
                    control,
                    task_dir,
                    kind,
                    &name,
                    checkpoint_id.as_deref().unwrap_or(&track.id),
                    &track.mime,
                    checkpoint_id.is_some(),
                    vod,
                )?;
                self.bytes = self
                    .bytes
                    .saturating_add(path.metadata().map_err(|error| error.to_string())?.len());
                self.files.push(path);
                if let Some(duration) = duration {
                    self.durations.push(duration);
                }
                crate::http_engine::write_media_progress(
                    &task_dir.join("progress.json"),
                    self.bytes.saturating_add(other.bytes),
                    (self.files.len() + other.files.len()) as u64,
                    0,
                    true,
                );
                Ok(())
            };

        if change_init {
            if let Some((url, range)) = &init {
                let (name, duration) = if initial {
                    ("init.mp4".into(), None)
                } else {
                    (format!("seg-{index:04}-init.mp4"), Some(0.0))
                };
                push(url, *range, name, duration)?;
                index += 1;
            }
            self.init = init;
        }
        let mut boundary = next_period && !change_init;
        for (media_index, url) in track.media.iter().enumerate().skip(first_media) {
            let range = track.media_ranges[media_index];
            if !self.seen.insert((url.clone(), range)) {
                continue;
            }
            let name = if boundary {
                format!("seg-{index:04}-period.m4s")
            } else {
                format!("seg-{index:04}.m4s")
            };
            let duration = track
                .media_durations
                .get(media_index)
                .copied()
                .unwrap_or(0.0);
            push(url, range, name, Some(duration))?;
            index += 1;
            boundary = false;
        }
        Ok(())
    }

    fn write_playlist(&self, dir: &Path) -> Result<(), String> {
        if !write_dash_preview_track(&dir.join("local.m3u8"), &self.files, &self.durations, true)? {
            write_dash_playlist(dir, &self.files)?;
        }
        Ok(())
    }
}

fn live_period_begin(periods: &[DashPeriod], active: Option<&DashPeriod>) -> Result<usize, String> {
    // DASH-IF: MPD refreshes may remove expired Periods. Their list indices
    // can change, so follow Period@id (or an explicit start if the ID is absent).
    let mut ids = HashSet::new();
    let mut previous_start = None;
    for period in periods {
        if let Some(id) = &period.id {
            if !ids.insert(id) {
                return Err("直播 DASH 清单包含重复的 Period ID".into());
            }
        } else if periods.len() > 1 && period.start.is_none() {
            return Err("直播 DASH 有多个 Period，但缺少可用于跟踪的 ID 或开始时间".into());
        }
        if let Some(start) = period.start {
            if previous_start.is_some_and(|previous| previous >= start) {
                return Err("直播 DASH 清单的 Period 开始时间未按顺序排列".into());
            }
            previous_start = Some(start);
        }
    }
    let Some(active) = active else {
        return Ok(0);
    };
    if let Some(index) = periods.iter().position(|period| period.same_as(active)) {
        return Ok(index);
    }
    if let Some(start) = active.start {
        if periods.iter().all(|period| period.start.is_some()) {
            return Ok(periods
                .iter()
                .position(|period| period.start.unwrap() > start)
                .unwrap_or(periods.len()));
        }
    }
    Err("直播 DASH 更换了 Period，无法确定新旧时间轴顺序".into())
}

fn continuing_live_track<'a>(
    representations: &'a [Representation],
    index: usize,
    selected: &Representation,
    same_period: bool,
) -> Result<&'a Representation, String> {
    let part = if same_period {
        representations.iter().find(|item| {
            item.period == index
                && item.id == selected.id
                && track_kind(item) == track_kind(selected)
        })
    } else {
        match_period_track(representations, index, selected)
    }
    .ok_or("直播 DASH 的新清单缺少选定音视频轨道")?;
    if !part.mime.eq_ignore_ascii_case(&selected.mime)
        || (!selected.lang.is_empty() && !part.lang.eq_ignore_ascii_case(&selected.lang))
        || (!part.codecs.is_empty()
            && !selected.codecs.is_empty()
            && codec_family(&part.codecs) != codec_family(&selected.codecs))
    {
        return Err("直播 DASH 轨道格式变化，无法无损拼接".into());
    }
    Ok(part)
}

#[allow(clippy::too_many_arguments)]
fn download_live_dash(
    mut manifest: DashManifest,
    mut xml: String,
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    preferred_bandwidth: u64,
    preferred_height: u32,
    download_subtitles: bool,
    preferred_audio: &str,
    vod: &mut DashCheckpoint,
) -> Result<PathBuf, String> {
    let mut active = None;
    let mut primary = LiveDashTrack::default();
    let mut audio = LiveDashTrack::default();
    let mut choices: Option<(Representation, Option<Representation>)> = None;
    let mut first_period = None;
    let mut subtitles = Vec::new();
    let audio_dir = task_dir.join("audio");

    loop {
        let start = live_period_begin(&manifest.periods, active.as_ref())?;
        for (index, period) in manifest.periods.iter().enumerate().skip(start) {
            if manifest.expired_periods.contains(&index) {
                continue;
            }
            let same_period = active.as_ref().is_some_and(|old| period.same_as(old));
            if let (Some(old_start), Some(new_start)) =
                (active.as_ref().and_then(|old| old.start), period.start)
            {
                if (same_period && new_start != old_start)
                    || (!same_period && new_start <= old_start)
                {
                    return Err("直播 DASH 的 Period 开始时间不连续".into());
                }
            }
            let (mut track, mut secondary) = if let Some((track, secondary)) = &choices {
                let track =
                    continuing_live_track(&manifest.representations, index, track, same_period)?;
                let secondary = secondary
                    .as_ref()
                    .map(|item| {
                        continuing_live_track(&manifest.representations, index, item, same_period)
                    })
                    .transpose()?;
                (track.clone(), secondary.cloned())
            } else {
                let candidates: Vec<_> = manifest
                    .representations
                    .iter()
                    .filter(|item| item.period == index)
                    .cloned()
                    .collect();
                let video = select_video(&candidates, preferred_bandwidth, preferred_height);
                let music = select_audio(&candidates, preferred_audio);
                let Some(track) = video.or(music) else {
                    continue;
                };
                (track.clone(), music.filter(|_| video.is_some()).cloned())
            };
            expand_segment_base(&mut track, headers, proxy, control)?;
            if let Some(secondary) = &mut secondary {
                expand_segment_base(secondary, headers, proxy, control)?;
            }
            if track.media.is_empty()
                || secondary.as_ref().is_some_and(|item| item.media.is_empty())
            {
                // Future Periods can be present before they advertise both tracks.
                break;
            }
            if track.whole_file || secondary.as_ref().is_some_and(|item| item.whole_file) {
                return Err("直播 DASH 无法使用没有分片索引的整文件轨道".into());
            }
            let next_period = active.is_some() && !same_period;
            let kind = if is_video(&track) { "video" } else { "audio" };
            primary.append(
                &track,
                next_period,
                kind,
                task_dir,
                task_dir,
                headers,
                proxy,
                control,
                vod,
                None,
                &audio,
            )?;
            if let Some(item) = &secondary {
                let scope = period
                    .id
                    .clone()
                    .or_else(|| period.start.map(|time| time.to_string()));
                audio.append(
                    item,
                    next_period,
                    "audio",
                    &audio_dir,
                    task_dir,
                    headers,
                    proxy,
                    control,
                    vod,
                    Some(scope.as_deref().unwrap_or("implicit")),
                    &primary,
                )?;
            }
            if !same_period {
                // Keep only the selected representation metadata across refreshes.
                for item in std::iter::once(&mut track).chain(secondary.iter_mut()) {
                    item.media.clear();
                    item.media_ranges.clear();
                    item.media_durations.clear();
                }
                choices = Some((track, secondary));
            }
            active = Some(period.clone());
            // Sidecars currently keep their original timeline. Do not append
            // later Periods' cues without rebasing their timestamps.
            if download_subtitles
                && period.same_as(first_period.get_or_insert_with(|| period.clone()))
            {
                let tracks: Vec<_> = manifest
                    .representations
                    .iter()
                    .filter(|item| item.period == index)
                    .cloned()
                    .collect();
                for mut item in select_subtitles(&tracks) {
                    if expand_segment_base(&mut item, headers, proxy, control).is_ok() {
                        merge_sub_track(&mut subtitles, &item);
                    }
                }
                save_dash_subtitles(task_dir, &subtitles, headers, proxy, control, vod);
            }
        }
        if !manifest.dynamic {
            break;
        }
        let wait = minimum_update_period(&xml).unwrap_or(3.0).clamp(0.4, 8.0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(wait);
        loop {
            match read_control(control).as_str() {
                "run" => {}
                "pause" => return Err("paused".into()),
                "cancel" => return Err("canceled".into()),
                _ => break,
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(remaining.min(std::time::Duration::from_millis(100)));
        }
        if read_control(control) != "run" {
            return Err(control_stop_error(control));
        }
        let body = refresh_live_manifest(url, headers, proxy, control)?;
        xml = String::from_utf8_lossy(&body).into_owned();
        manifest = parse_mpd(&xml, url)?;
    }
    if primary.durations.is_empty() {
        return Err("DASH representation produced no segments".into());
    }
    primary.write_playlist(task_dir)?;
    let merged_audio = if audio.durations.is_empty() {
        None
    } else {
        audio.write_playlist(&audio_dir)?;
        crate::http_engine::write_media_merging(&task_dir.join("progress.json"));
        let output = audio_dir.join("merged.m4a");
        merge_with_ffmpeg(&audio_dir, &output, control)?;
        Some(output)
    };
    finish_dash(
        task_dir,
        merged_audio,
        subtitles,
        headers,
        proxy,
        control,
        vod,
    )
}

fn track_kind(item: &Representation) -> u8 {
    if is_subtitle(item) {
        2
    } else if is_audio(item) {
        1
    } else if is_video(item) {
        0
    } else {
        3
    }
}

fn refresh_live_manifest(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
) -> Result<Vec<u8>, String> {
    for attempt in 0..4 {
        if read_control(control) != "run" {
            return Err(control_stop_error(control));
        }
        let (status, body) = fetch_bytes_limited(url, headers, proxy, super::MAX_MANIFEST_BYTES)
            .map_err(|error| error.to_string())?;
        if status == 200 || status == 206 {
            return Ok(body);
        }
        if !matches!(status, 408 | 429 | 500..=599) || attempt == 3 {
            return Err(format!(
                "DASH live manifest HTTP {status} after {} attempt(s)",
                attempt + 1
            ));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250 << attempt);
        while std::time::Instant::now() < deadline {
            if read_control(control) != "run" {
                return Err(control_stop_error(control));
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .min(std::time::Duration::from_millis(50)),
            );
        }
    }
    Err("DASH live manifest retries exhausted".into())
}

/// Picks the representation in `period` that continues `chosen`: same kind,
/// then same contentType / mimeType / lang, then the closest bandwidth/height.
fn match_period_track<'a>(
    representations: &'a [Representation],
    period: usize,
    chosen: &Representation,
) -> Option<&'a Representation> {
    let kind = track_kind(chosen);
    let chosen_codec = codec_family(&chosen.codecs);
    representations
        .iter()
        .filter(|item| item.period == period && track_kind(item) == kind)
        .filter(|item| {
            kind != 2 || chosen.lang.is_empty() || item.lang.eq_ignore_ascii_case(&chosen.lang)
        })
        .min_by_key(|item| {
            let codec = codec_family(&item.codecs);
            (
                !chosen_codec.is_empty() && !codec.is_empty() && codec != chosen_codec,
                !item.content_type.eq_ignore_ascii_case(&chosen.content_type),
                !item.lang.eq_ignore_ascii_case(&chosen.lang),
                !item.mime.eq_ignore_ascii_case(&chosen.mime),
                item.bandwidth.abs_diff(chosen.bandwidth),
                item.height.abs_diff(chosen.height),
                item.id != chosen.id,
            )
        })
}

/// Resolves `chosen` (from Period 0) in every Period of a static manifest and
/// returns the per-Period parts in document order.
fn period_parts(
    manifest: &DashManifest,
    chosen: &Representation,
) -> Result<Vec<Representation>, String> {
    let mut parts = vec![chosen.clone()];
    for period in 1..manifest.period_count {
        let part =
            match_period_track(&manifest.representations, period, chosen).ok_or_else(|| {
                format!(
                    "多 Period DASH 第 {} 个 Period 缺少与所选轨道（{}）匹配的轨道",
                    period + 1,
                    chosen.id
                )
            })?;
        parts.push(part.clone());
    }
    Ok(parts)
}

fn codec_family(codecs: &str) -> String {
    codecs
        .split(['.', ','])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Appends later Periods' segments to the first Period's track. A Period whose
/// initialization differs keeps its init as an in-line segment (listed in
/// `init_markers`), which the local playlist turns into a discontinuity with a
/// new EXT-X-MAP so FFmpeg merges it without re-encoding.
fn concat_period_parts(parts: Vec<Representation>) -> Result<Representation, String> {
    let mut parts = parts.into_iter();
    let mut out = parts.next().ok_or("DASH Period 为空")?;
    let first_codec = codec_family(&out.codecs);
    let mut aligned = out.media_durations.len() == out.media.len();
    let mut current_init = (out.init.clone(), out.init_range);
    for (offset, part) in parts.enumerate() {
        let number = offset + 2;
        if out.whole_file || part.whole_file {
            return Err(
                "多 Period DASH 中无 indexRange 的 SegmentBase 整文件轨道暂不支持拼接".into(),
            );
        }
        let codec = codec_family(&part.codecs);
        if !first_codec.is_empty() && !codec.is_empty() && codec != first_codec {
            return Err(format!(
                "多 Period DASH 第 {number} 个 Period 的编码（{}）与第 1 个 Period（{}）不同，无法无损拼接",
                part.codecs, out.codecs
            ));
        }
        if part.media.is_empty() {
            return Err(format!(
                "多 Period DASH 第 {number} 个 Period 没有可下载的分片"
            ));
        }
        let init = (part.init.clone(), part.init_range);
        if init != current_init {
            let Some(url) = part.init.clone() else {
                return Err(format!(
                    "多 Period DASH 第 {number} 个 Period 缺少初始化段，无法与前一个 Period 拼接"
                ));
            };
            out.init_markers.push(out.media.len());
            out.media.push(url);
            out.media_ranges.push(part.init_range);
            out.media_durations.push(0.0);
            current_init = init;
        } else {
            // tfdt may restart even when the initialization URL is unchanged.
            out.period_markers.push(out.media.len());
        }
        aligned &= part.media_durations.len() == part.media.len();
        out.media.extend(part.media);
        out.media_ranges.extend(part.media_ranges);
        out.media_durations.extend(part.media_durations);
    }
    if !aligned {
        // 缺少准确时间轴时不伪造预览时长，与单 Period 行为一致。
        out.media_durations.clear();
    }
    Ok(out)
}

fn expand_periods(
    manifest: &DashManifest,
    chosen: Representation,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
) -> Result<Representation, String> {
    if manifest.period_count <= 1 || manifest.dynamic {
        let mut track = chosen;
        expand_segment_base(&mut track, headers, proxy, control)?;
        return Ok(track);
    }
    let mut parts = period_parts(manifest, &chosen)?;
    for part in &mut parts {
        expand_segment_base(part, headers, proxy, control)?;
    }
    concat_period_parts(parts)
}

fn segment_file_name(index: usize, track: &Representation, marker_index: usize) -> String {
    if track.init_markers.contains(&marker_index) {
        format!("seg-{index:04}-init.mp4")
    } else if track.period_markers.contains(&marker_index) {
        format!("seg-{index:04}-period.m4s")
    } else {
        format!("seg-{index:04}.m4s")
    }
}

#[allow(clippy::too_many_arguments)]
fn download_dash_primary(
    track: &Representation,
    video: bool,
    task_dir: &Path,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    vod: &mut DashCheckpoint,
    audio: Option<&Representation>,
    total: u64,
) -> Result<(Vec<PathBuf>, Option<PathBuf>), String> {
    let mut files = Vec::new();
    if let Some(init) = &track.init {
        files.push(resume_or_fetch(
            init,
            track.init_range,
            headers,
            proxy,
            &task_dir.join("init.mp4"),
            control,
            task_dir,
            "init",
            if video { "init" } else { "audio-init" },
            &track.id,
            &track.mime,
            true,
            vod,
        )?);
    }
    let offset = files.len();
    let planned: Vec<_> = (0..track.media.len())
        .map(|index| task_dir.join(segment_file_name(index + offset, track, index)))
        .collect();
    let audio_dir = task_dir.join("audio");
    let mut audio_files = Vec::new();
    if let Some(audio) = audio {
        fs::create_dir_all(&audio_dir).map_err(|error| error.to_string())?;
        if let Some(init) = &audio.init {
            audio_files.push(resume_or_fetch(
                init,
                audio.init_range,
                headers,
                proxy,
                &audio_dir.join("init.mp4"),
                control,
                task_dir,
                "init",
                "audio-init",
                &audio.id,
                &audio.mime,
                true,
                vod,
            )?);
        }
    }
    let audio_planned: Vec<_> = audio
        .map(|track| {
            (0..track.media.len())
                .map(|index| audio_dir.join(segment_file_name(index, track, index)))
                .collect()
        })
        .unwrap_or_default();
    let kind = if video { "video" } else { "audio" };
    for (index, path) in planned.iter().enumerate() {
        let rel = relative_posix(task_dir, path);
        let identity = dash_file_identity(
            kind,
            &(index + offset).to_string(),
            &track.id,
            &track.media[index],
            track.media_ranges[index],
            &track.mime,
        );
        let size = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        if !vod.can_reuse(&rel, &identity, size) {
            super::preview::reset_ready(task_dir, path)?;
        } else {
            super::preview::mark_ready(task_dir, path)?;
        }
    }
    if let Some(audio) = audio {
        for (index, path) in audio_planned.iter().enumerate() {
            let identity = dash_file_identity(
                "audio",
                &index.to_string(),
                &audio.id,
                &audio.media[index],
                audio.media_ranges[index],
                &audio.mime,
            );
            let size = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            if vod.can_reuse(&relative_posix(task_dir, path), &identity, size) {
                super::preview::mark_ready(task_dir, path)?;
            } else {
                super::preview::reset_ready(task_dir, path)?;
            }
        }
    }
    let all: Vec<_> = files.iter().chain(planned.iter()).cloned().collect();
    let mut preview = write_dash_preview_track(
        &task_dir.join("preview-video.m3u8"),
        &all,
        &track.media_durations,
        true,
    )?;
    let all_audio: Vec<_> = audio_files.iter().chain(&audio_planned).cloned().collect();
    if let Some(audio) = audio {
        preview &= write_dash_preview_track(
            &audio_dir.join("preview-audio.m3u8"),
            &all_audio,
            &audio.media_durations,
            true,
        )?;
    }
    let durations: Vec<_> = (0..planned.len())
        .map(|index| track.media_durations.get(index).copied().unwrap_or(1.0))
        .collect();
    let audio_durations: Vec<_> = (0..audio_planned.len())
        .map(|index| {
            audio
                .and_then(|track| track.media_durations.get(index))
                .copied()
                .unwrap_or(1.0)
        })
        .collect();
    let mut pending: std::collections::VecDeque<_> =
        super::preview::segment_order(&durations, &audio_durations).into();
    while !pending.is_empty() {
        if read_control(control) != "run" {
            return Err(control_stop_error(control));
        }
        if let Some(priority) = super::preview::priority_file(task_dir) {
            if let Some(position) = pending.iter().position(|(is_audio, index)| {
                (if *is_audio {
                    &audio_planned[*index]
                } else {
                    &planned[*index]
                }) == &priority
            }) {
                pending.rotate_left(position);
            }
        }
        let (is_audio, index) = pending.pop_front().unwrap();
        if is_audio {
            let audio = audio.unwrap();
            audio_files.push(resume_or_fetch(
                &audio.media[index],
                audio.media_ranges[index],
                headers,
                proxy,
                &audio_planned[index],
                control,
                task_dir,
                "audio",
                &index.to_string(),
                &audio.id,
                &audio.mime,
                true,
                vod,
            )?);
        } else {
            files.push(resume_or_fetch(
                &track.media[index],
                track.media_ranges[index],
                headers,
                proxy,
                &planned[index],
                control,
                task_dir,
                kind,
                &(index + offset).to_string(),
                &track.id,
                &track.mime,
                true,
                vod,
            )?);
        }
        report_dash_tracks_progress(task_dir, &files, &audio_files, total, false);
        let first_video_ready =
            super::preview::is_ready(task_dir, &relative_posix(task_dir, &planned[0]));
        let first_audio_ready = audio.is_none()
            || audio_planned.first().is_some_and(|path| {
                super::preview::is_ready(task_dir, &relative_posix(task_dir, path))
            });
        if preview && first_video_ready && first_audio_ready {
            write_dash_preview_master(task_dir, audio.is_some())?;
        }
    }
    // 下载优先级不改变最终拼接顺序。
    let merged_audio = if audio.is_some() {
        write_dash_playlist(&audio_dir, &all_audio)?;
        crate::http_engine::write_media_merging(&task_dir.join("progress.json"));
        let output = audio_dir.join("merged.m4a");
        merge_with_ffmpeg(&audio_dir, &output, control)?;
        Some(output)
    } else {
        None
    };
    Ok((all, merged_audio))
}

#[allow(clippy::too_many_arguments)]
fn finish_dash(
    task_dir: &Path,
    audio_merged: Option<PathBuf>,
    subtitles: Vec<Representation>,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    vod: &mut DashCheckpoint,
) -> Result<PathBuf, String> {
    crate::http_engine::write_media_merging(&task_dir.join("progress.json"));
    let output = task_dir.join("merged.mp4");
    merge_with_ffmpeg(task_dir, &output, control)?;
    let published = if let Some(audio) = audio_merged {
        let muxed = task_dir.join("muxed.mp4");
        crate::media::merge::mux_av(&output, Some(&audio), &[], &muxed, control)?;
        muxed
    } else {
        output
    };
    save_dash_subtitles(task_dir, &subtitles, headers, proxy, control, vod);
    Ok(published)
}

fn merge_sub_track(tracks: &mut Vec<Representation>, incoming: &Representation) {
    if let Some(existing) = tracks.iter_mut().find(|item| {
        item.id == incoming.id && item.lang == incoming.lang && item.mime == incoming.mime
    }) {
        if existing.init.is_none() {
            existing.init = incoming.init.clone();
            existing.init_range = incoming.init_range;
        }
        for (index, media) in incoming.media.iter().enumerate() {
            let range = incoming.media_ranges[index];
            if !existing
                .media
                .iter()
                .zip(&existing.media_ranges)
                .any(|(url, saved)| url == media && *saved == range)
            {
                existing.media_durations.resize(existing.media.len(), 1.0);
                existing.media.push(media.clone());
                existing.media_ranges.push(range);
                existing
                    .media_durations
                    .push(incoming.media_durations.get(index).copied().unwrap_or(1.0));
            }
        }
        return;
    }
    tracks.push(incoming.clone());
}

fn download_one(
    url: &str,
    range: Option<ByteRange>,
    headers: &HashMap<String, String>,
    proxy: &str,
    output: &Path,
    control: &Path,
) -> Result<PathBuf, String> {
    let job = Job {
        url: url.to_string(),
        headers: headers.clone(),
        output: output.to_path_buf(),
        connections: 1,
        chunk_bytes: 64 * 1024,
        total: 0,
        sequential: true,
        resume_from: 0,
        proxy: proxy.to_string(),
        resource_key: url.to_string(),
        etag: String::new(),
        last_modified: String::new(),
        control: control.to_path_buf(),
        progress: output.with_extension("progress.json"),
        method: "GET".into(),
        body_path: PathBuf::new(),
        mirrors: Vec::new(),
        replay_json: crate::credentials::scoped_replay_json(),
        curl_body_limit: range.map(|range| range.length),
        range_stop: None,
        origin_boundary: String::new(),
    };
    if let Some(range) = range {
        crate::http_engine::download_bytes_range(&job, range.start, range.length)
            .map_err(|error| error.to_string())?;
    } else {
        run_job(&job).map_err(|error| error.to_string())?;
    }
    Ok(output.to_path_buf())
}

fn relative_posix(task_dir: &Path, dest: &Path) -> String {
    dest.strip_prefix(task_dir)
        .unwrap_or(dest)
        .to_string_lossy()
        .replace('\\', "/")
}

pub(crate) fn dash_file_identity(
    kind: &str,
    slot: &str,
    representation: &str,
    url: &str,
    range: Option<ByteRange>,
    mime: &str,
) -> String {
    let mut payload = serde_json::json!({
        "kind": kind,
        "mime": mime,
        "representation": representation,
        "slot": slot,
        "url": super::hls::stable_media_url(url),
    });
    // Keep existing whole-segment checkpoint identities compatible.
    if let Some(range) = range {
        payload["range"] = serde_json::json!([range.start, range.length]);
    }
    crate::crypto_lite::sha256_hex(payload.to_string().as_bytes())
}

struct DashCheckpoint {
    records: HashMap<String, (String, u64)>,
}

impl DashCheckpoint {
    fn load(task_dir: &Path) -> Self {
        let Ok(text) = fs::read_to_string(task_dir.join("dash_vod_segments.json")) else {
            return Self {
                records: HashMap::new(),
            };
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Self {
                records: HashMap::new(),
            };
        };
        if value.get("version").and_then(|item| item.as_u64()) != Some(1) {
            return Self {
                records: HashMap::new(),
            };
        };
        let mut records = HashMap::new();
        if let Some(map) = value.get("files").and_then(|item| item.as_object()) {
            for (slot, record) in map {
                let identity = record
                    .get("identity")
                    .and_then(|item| item.as_str())
                    .unwrap_or("")
                    .to_string();
                let size = record
                    .get("size")
                    .and_then(|item| item.as_u64())
                    .unwrap_or(0);
                if !identity.is_empty() && size > 0 {
                    records.insert(slot.clone(), (identity, size));
                }
            }
        }
        Self { records }
    }

    fn can_reuse(&self, slot: &str, identity: &str, size: u64) -> bool {
        if size == 0 {
            return false;
        }
        match self.records.get(slot) {
            Some((saved, saved_size)) => saved == identity && *saved_size == size,
            None => false,
        }
    }

    fn remember(&mut self, slot: &str, identity: &str, size: u64) {
        if identity.is_empty() || size == 0 {
            return;
        }
        self.records
            .insert(slot.to_string(), (identity.to_string(), size));
    }

    fn save(&self, task_dir: &Path) -> Result<(), String> {
        let mut files = serde_json::Map::new();
        for (slot, (identity, size)) in &self.records {
            files.insert(
                slot.clone(),
                serde_json::json!({ "identity": identity, "size": size }),
            );
        }
        let payload = serde_json::json!({
            "version": 1,
            "files": files,
        });
        super::hls::write_text_atomic(
            &task_dir.join("dash_vod_segments.json"),
            &payload.to_string(),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn resume_or_fetch(
    url: &str,
    range: Option<ByteRange>,
    headers: &HashMap<String, String>,
    proxy: &str,
    dest: &Path,
    control: &Path,
    task_dir: &Path,
    kind: &str,
    slot: &str,
    representation: &str,
    mime: &str,
    checkpoint: bool,
    vod: &mut DashCheckpoint,
) -> Result<PathBuf, String> {
    let rel = relative_posix(task_dir, dest);
    let identity = dash_file_identity(kind, slot, representation, url, range, mime);
    let size = dest.metadata().map(|meta| meta.len()).unwrap_or(0);
    if dest.is_file() && vod.can_reuse(&rel, &identity, size) {
        if checkpoint {
            vod.remember(&rel, &identity, size);
            vod.save(task_dir)?;
        }
        super::preview::mark_ready(task_dir, dest)?;
        return Ok(dest.to_path_buf());
    }
    // Invalidate the old identity on disk before replacing any bytes. A failed
    // transfer can leave a full-length partial file; an older checkpoint must
    // never authorize it after a restart or a manifest/range change.
    if vod.records.remove(&rel).is_some() {
        vod.save(task_dir)?;
    }
    super::preview::reset_ready(task_dir, dest)?;
    download_one(url, range, headers, proxy, dest, control)?;
    if checkpoint {
        vod.remember(
            &rel,
            &identity,
            dest.metadata().map(|meta| meta.len()).unwrap_or(0),
        );
        vod.save(task_dir)?;
    }
    super::preview::mark_ready(task_dir, dest)?;
    Ok(dest.to_path_buf())
}

fn mpd_element(node: roxmltree::Node<'_, '_>, name: &str, scope: roxmltree::Node<'_, '_>) -> bool {
    node.is_element()
        && node.tag_name().name() == name
        && node.tag_name().namespace() == scope.tag_name().namespace()
}

fn mpd_child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|child| mpd_element(*child, name, node))
}

fn resolve(base: &str, reference: &str) -> String {
    super::resolve_http_uri(base, reference)
}

fn minimum_update_period(xml: &str) -> Option<f64> {
    let document = roxmltree::Document::parse(xml).ok()?;
    document
        .root_element()
        .attribute("minimumUpdatePeriod")
        .and_then(parse_duration_seconds)
}

fn parse_duration_seconds(value: &str) -> Option<f64> {
    let text = value.trim();
    if let Some(rest) = text.strip_prefix('P') {
        let mut total = 0.0;
        let mut number = String::new();
        let mut in_time = false;
        let mut has_component = false;
        for ch in rest.chars() {
            if ch == 'T' {
                in_time = true;
                continue;
            }
            if ch.is_ascii_digit() || ch == '.' {
                number.push(ch);
                continue;
            }
            let parsed = number.parse::<f64>().ok()?;
            number.clear();
            has_component = true;
            total += match (in_time, ch) {
                (false, 'Y') => parsed * 31_536_000.0,
                (false, 'M') => parsed * 2_592_000.0,
                (false, 'W') => parsed * 604_800.0,
                (false, 'D') => parsed * 86_400.0,
                (true, 'H') => parsed * 3600.0,
                (true, 'M') => parsed * 60.0,
                (true, 'S') => parsed,
                _ => return None,
            };
        }
        return (number.is_empty() && has_component && total.is_finite()).then_some(total);
    }
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn read_control(path: &Path) -> String {
    // 运行任务读进程内令牌，未注册路径仍读控制文件（行为不变）。
    crate::task_control::read_word(path)
}

/// A stopped control value is not a valid completion.
///
/// The HLS segment loop returns `Err("paused")` / `Err("canceled")`; DASH used
/// to `break` out of its segment loops and then fall straight into
/// `finish_dash`, which merges only the segments fetched so far and returns
/// `Ok`.  A pause or cancel on a static VOD task therefore produced a truncated
/// `merged.mp4` reported as a successful download.
fn control_stop_error(control: &Path) -> String {
    match read_control(control).as_str() {
        "pause" => "paused".to_string(),
        _ => "canceled".to_string(),
    }
}

fn report_dash_tracks_progress(
    task_dir: &Path,
    files: &[PathBuf],
    audio_files: &[PathBuf],
    total: u64,
    live: bool,
) {
    let downloaded: u64 = files
        .iter()
        .chain(audio_files.iter())
        .map(|path| fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
        .sum();
    crate::http_engine::write_media_progress(
        &task_dir.join("progress.json"),
        downloaded,
        (files.len() + audio_files.len()) as u64,
        total,
        live,
    );
}

fn write_dash_preview_track(
    path: &Path,
    files: &[PathBuf],
    durations: &[f64],
    complete: bool,
) -> Result<bool, String> {
    let skip_init = files
        .first()
        .and_then(|path| path.file_name())
        .is_some_and(|name| name.to_string_lossy().contains("init"));
    let count = files.len().saturating_sub(usize::from(skip_init));
    // 清单未提供准确时间轴时等待成品，不能用固定 4 秒伪造可播放范围。
    // 多 Period 拼接时内嵌的初始化段不占播放时长。
    if count == 0
        || durations.len() < count
        || files
            .iter()
            .skip(usize::from(skip_init))
            .zip(durations)
            .any(|(file, duration)| {
                !is_init_file(file) && (!duration.is_finite() || *duration <= 0.0)
            })
    {
        return Ok(false);
    }
    let target = durations
        .iter()
        .copied()
        .filter(|duration| duration.is_finite())
        .fold(1.0, f64::max)
        .ceil() as u64;
    let mut text = format!("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:{}\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:0\n", if complete { "VOD" } else { "EVENT" });
    if skip_init {
        text.push_str(&format!(
            "#EXT-X-MAP:URI=\"{}\"\n",
            playlist_leaf(&files[0])
        ));
    }
    for (file, duration) in files.iter().skip(usize::from(skip_init)).zip(durations) {
        if is_init_file(file) {
            push_period_map(&mut text, file);
            continue;
        }
        if is_period_start(file) {
            text.push_str("#EXT-X-DISCONTINUITY\n");
        }
        text.push_str(&format!(
            "#EXTINF:{duration:.6},\n{}\n",
            playlist_leaf(file)
        ));
    }
    if complete {
        text.push_str("#EXT-X-ENDLIST\n");
    }
    super::hls::write_text_atomic(path, &text)?;
    Ok(true)
}

fn write_dash_preview_master(task_dir: &Path, audio: bool) -> Result<(), String> {
    if audio && !task_dir.join("audio/preview-audio.m3u8").is_file() {
        return Ok(());
    }
    let master = if audio {
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Audio\",DEFAULT=YES,AUTOSELECT=YES,URI=\"audio/preview-audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1,AUDIO=\"audio\"\npreview-video.m3u8\n"
    } else {
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-STREAM-INF:BANDWIDTH=1\npreview-video.m3u8\n"
    };
    super::hls::write_text_atomic(&task_dir.join("preview.m3u8"), master)
}

fn write_dash_playlist(task_dir: &Path, files: &[PathBuf]) -> Result<(), String> {
    let mut text = String::from(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n",
    );
    let skip_init = files
        .first()
        .and_then(|path| path.file_name())
        .is_some_and(|name| name.to_string_lossy().contains("init"));
    if skip_init {
        text.push_str(&format!(
            "#EXT-X-MAP:URI=\"{}\"\n",
            playlist_leaf(&files[0])
        ));
    }
    for file in files.iter().skip(usize::from(skip_init)) {
        if is_init_file(file) {
            push_period_map(&mut text, file);
            continue;
        }
        if is_period_start(file) {
            text.push_str("#EXT-X-DISCONTINUITY\n");
        }
        text.push_str(&format!("#EXTINF:4.000,\n{}\n", playlist_leaf(file)));
    }
    text.push_str("#EXT-X-ENDLIST\n");
    fs::write(task_dir.join("local.m3u8"), text).map_err(|error| error.to_string())
}

fn is_init_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().contains("init"))
}

fn is_period_start(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("-period.m4s"))
}

// 后续 Period 的初始化段与前一个不同：标记不连续并切换 EXT-X-MAP。
fn push_period_map(text: &mut String, file: &Path) {
    text.push_str(&format!(
        "#EXT-X-DISCONTINUITY\n#EXT-X-MAP:URI=\"{}\"\n",
        playlist_leaf(file)
    ));
}

fn playlist_leaf(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("seg.bin");
    if name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        && !name.contains("..")
    {
        name.to_string()
    } else {
        "seg.bin".into()
    }
}

fn is_subtitle(item: &Representation) -> bool {
    let content = item.content_type.to_ascii_lowercase();
    let mime = item.mime.to_ascii_lowercase();
    let codecs = item.codecs.to_ascii_lowercase();
    matches!(content.as_str(), "text" | "subtitle" | "subtitles")
        || mime.starts_with("text/")
        || mime == "application/ttml+xml"
        || (mime == "application/mp4" && (codecs.starts_with("stpp") || codecs.starts_with("wvtt")))
        || codecs.starts_with("stpp")
        || codecs.starts_with("wvtt")
}

fn is_audio(item: &Representation) -> bool {
    if is_subtitle(item) {
        return false;
    }
    let mime = item.mime.to_ascii_lowercase();
    if mime.starts_with("audio/") {
        return true;
    }
    if !mime.is_empty() {
        return false;
    }
    if item.content_type.eq_ignore_ascii_case("audio") {
        return true;
    }
    let codecs = item.codecs.to_ascii_lowercase();
    codecs.starts_with("mp4a")
        || codecs.starts_with("opus")
        || codecs.starts_with("vorbis")
        || codecs.starts_with("ac-3")
        || codecs.starts_with("ec-3")
        || codecs.starts_with("flac")
}

fn is_video(item: &Representation) -> bool {
    if is_subtitle(item) || is_audio(item) {
        return false;
    }
    if item.content_type.eq_ignore_ascii_case("image") {
        return false;
    }
    let mime = item.mime.to_ascii_lowercase();
    if mime.starts_with("video/") {
        return true;
    }
    if mime.starts_with("audio/")
        || mime.starts_with("text/")
        || mime.starts_with("application/")
        || mime.starts_with("image/")
    {
        return false;
    }
    item.height > 0 || mime.is_empty()
}

fn select_audio<'a>(
    representations: &'a [Representation],
    preferred: &str,
) -> Option<&'a Representation> {
    let tracks: Vec<&Representation> = representations
        .iter()
        .filter(|item| is_audio(item))
        .collect();
    if !preferred.trim().is_empty() {
        if let Some(hit) = tracks.iter().copied().find(|item| {
            item.id.eq_ignore_ascii_case(preferred)
                || item.label.eq_ignore_ascii_case(preferred)
                || item.lang.eq_ignore_ascii_case(preferred)
        }) {
            return Some(hit);
        }
    }
    tracks.into_iter().max_by_key(|item| item.bandwidth)
}

fn select_subtitles(representations: &[Representation]) -> Vec<Representation> {
    representations
        .iter()
        .filter(|item| is_subtitle(item))
        .cloned()
        .collect()
}

fn subtitle_label(item: &Representation, used: &mut std::collections::BTreeSet<String>) -> String {
    let raw = if !item.lang.is_empty() {
        item.lang.as_str()
    } else if !item.label.is_empty() {
        item.label.as_str()
    } else if !item.id.is_empty() {
        item.id.as_str()
    } else {
        "und"
    };
    let mut label: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if label.trim_matches(|ch| ch == '_' || ch == '.').is_empty() {
        label = "und".into();
    }
    let mut candidate = label.clone();
    let mut suffix = 2;
    while used.contains(&candidate.to_ascii_lowercase()) {
        candidate = format!("{label}.{suffix}");
        suffix += 1;
    }
    used.insert(candidate.to_ascii_lowercase());
    candidate
}

fn save_dash_subtitles(
    task_dir: &Path,
    tracks: &[Representation],
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    vod: &mut DashCheckpoint,
) {
    if tracks.is_empty() {
        return;
    }
    let subs_dir = task_dir.join("subs");
    let Ok(()) = fs::create_dir_all(&subs_dir) else {
        return;
    };
    let mut used = std::collections::BTreeSet::new();
    for track in tracks {
        let label = subtitle_label(track, &mut used);
        let _ = download_dash_subtitle_track(
            track, &subs_dir, &label, headers, proxy, control, task_dir, vod,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn download_dash_subtitle_track(
    track: &Representation,
    subs_dir: &Path,
    label: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    task_dir: &Path,
    vod: &mut DashCheckpoint,
) -> Result<PathBuf, String> {
    let work = subs_dir.join(label);
    fs::create_dir_all(&work).map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    if let Some(init) = &track.init {
        files.push(resume_or_fetch(
            init,
            track.init_range,
            headers,
            proxy,
            &work.join("init.bin"),
            control,
            task_dir,
            "init",
            &format!("sub-{label}-init"),
            &track.id,
            &track.mime,
            true,
            vod,
        )?);
    }
    for (index, media) in track.media.iter().enumerate() {
        if read_control(control) != "run" {
            break;
        }
        files.push(resume_or_fetch(
            media,
            track.media_ranges[index],
            headers,
            proxy,
            &work.join(format!("{index:06}.bin")),
            control,
            task_dir,
            "subtitle",
            &format!("sub-{label}-{index}"),
            &track.id,
            &track.mime,
            true,
            vod,
        )?);
    }
    if files.is_empty() {
        return Err("empty subtitle track".into());
    }
    let mime = track.mime.to_ascii_lowercase();
    let codecs = track.codecs.to_ascii_lowercase();
    let vtt_path = subs_dir.join(format!("{label}.vtt"));
    if mime.starts_with("text/vtt") || (track.init.is_none() && codecs.starts_with("wvtt")) {
        let texts: Vec<String> = files
            .iter()
            .filter_map(|path| fs::read_to_string(path).ok())
            .collect();
        let merged = merge_webvtt_segments(&texts);
        if !has_cues(&merged) {
            return Err("subtitle track has no cues".into());
        }
        fs::write(&vtt_path, merged.as_bytes()).map_err(|error| error.to_string())?;
    } else if mime == "application/ttml+xml" {
        let ttml_path = subs_dir.join(format!("{label}.ttml"));
        crate::media::merge::concat_files(&files, &ttml_path)?;
        return Ok(ttml_path);
    } else if codecs.starts_with("wvtt") || codecs.starts_with("stpp") || mime == "application/mp4"
    {
        let joined = work.join("subtitle.mp4");
        crate::media::merge::concat_files(&files, &joined)?;
        let ffmpeg =
            crate::media::merge::locate_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())?;
        let status = Command::new(ffmpeg)
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-protocol_whitelist",
                "file,crypto",
                "-i",
            ])
            .arg(&joined)
            .arg(&vtt_path)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() || !vtt_path.is_file() {
            return Err("ffmpeg could not convert fMP4 DASH subtitles".into());
        }
    } else {
        return Err(format!("unsupported DASH subtitle format {mime} {codecs}"));
    }
    if let Ok(merged) = fs::read_to_string(&vtt_path) {
        if has_cues(&merged) {
            let _ = fs::write(vtt_path.with_extension("srt"), webvtt_to_srt(&merged));
        }
    }
    Ok(vtt_path)
}

/// Period@start / @duration per ISO 23009-1 5.3.2: a missing start follows the
/// previous Period's end; a missing duration runs to the next Period's start or
/// to mediaPresentationDuration for the last Period.
fn period_durations(
    periods: &[roxmltree::Node<'_, '_>],
    root: roxmltree::Node<'_, '_>,
) -> Vec<Option<f64>> {
    let explicit_start = |period: roxmltree::Node<'_, '_>| {
        period.attribute("start").and_then(parse_duration_seconds)
    };
    let mut out = Vec::with_capacity(periods.len());
    let mut previous_end = Some(0.0);
    for (index, period) in periods.iter().enumerate() {
        let start = explicit_start(*period).or(if index == 0 { Some(0.0) } else { previous_end });
        let duration = period
            .attribute("duration")
            .and_then(parse_duration_seconds)
            .or_else(|| {
                let end = match periods.get(index + 1) {
                    Some(next) => explicit_start(*next)?,
                    None => root
                        .attribute("mediaPresentationDuration")
                        .and_then(parse_duration_seconds)?,
                };
                Some(end - start?)
            })
            .filter(|duration| duration.is_finite() && *duration > 0.0);
        previous_end = start
            .zip(duration)
            .map(|(start, duration)| start + duration);
        out.push(duration);
    }
    out
}

// 十进制时长转 f64 后可能略高于整数刻度，不能因此多请求一个分片。
fn ceil_segment_units(value: f64) -> f64 {
    let nearest = value.round();
    if (value - nearest).abs() <= value.abs() * f64::EPSILON * 4.0 {
        nearest
    } else {
        value.ceil()
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_live_duration(
    root: roxmltree::Node<'_, '_>,
    templates: &[roxmltree::Node<'_, '_>],
    template: &str,
    base_url: &str,
    id: &str,
    bandwidth: u64,
    period_start: f64,
    period_duration: Option<f64>,
    now: f64,
    durations: &mut Vec<f64>,
    segment_budget: &mut u64,
    expired: &mut bool,
) -> Result<Vec<String>, String> {
    const MAX_LIVE_SEGMENTS: u64 = 1200;
    let attr = |key: &str| templates.iter().find_map(|node| node.attribute(key));
    let integer = |key: &str, default: Option<u64>| -> Result<u64, String> {
        match attr(key) {
            Some(value) => value
                .parse()
                .map_err(|_| format!("invalid dynamic DASH {key}")),
            None => default.ok_or_else(|| format!("dynamic DASH requires SegmentTemplate {key}")),
        }
    };
    let duration = integer("duration", None)?;
    let timescale = integer("timescale", Some(1))?;
    let offset = integer("presentationTimeOffset", Some(0))?;
    let start_number = integer("startNumber", Some(1))?;
    if duration == 0 || timescale == 0 {
        return Err("dynamic DASH duration and timescale must be positive".into());
    }
    let stamp = root
        .attribute("availabilityStartTime")
        .ok_or("dynamic DASH duration template requires availabilityStartTime")?;
    let bytes = stamp.as_bytes();
    if bytes.len() < 20
        || !bytes[..19]
            .iter()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                10 => *byte == b'T',
                13 | 16 => *byte == b':',
                _ => byte.is_ascii_digit(),
            })
    {
        return Err("invalid DASH availabilityStartTime".into());
    }
    let zone = stamp[19..].trim_start_matches(|ch: char| ch == '.' || ch.is_ascii_digit());
    if zone != "Z"
        && !(zone.len() == 6
            && matches!(zone.as_bytes()[0], b'+' | b'-')
            && zone.as_bytes()[3] == b':'
            && zone.as_bytes()[1..3]
                .iter()
                .chain(&zone.as_bytes()[4..])
                .all(u8::is_ascii_digit))
    {
        return Err("invalid DASH availabilityStartTime timezone".into());
    }
    let epoch = crate::net_policy::parse_rfc3339_epoch(stamp)
        .ok_or("invalid DASH availabilityStartTime")? as f64;
    let fraction = stamp
        .split_once('.')
        .map(|(_, tail)| {
            let digits = tail.split(['Z', '+', '-']).next().unwrap_or("");
            if digits.is_empty() || !digits.bytes().all(|ch| ch.is_ascii_digit()) {
                return Err("invalid DASH availabilityStartTime fraction");
            }
            format!("0.{digits}")
                .parse::<f64>()
                .map_err(|_| "invalid DASH availabilityStartTime fraction")
        })
        .transpose()?
        .unwrap_or(0.0);
    let depth = root
        .attribute("timeShiftBufferDepth")
        .map(|value| {
            parse_duration_seconds(value)
                .filter(|value| *value > 0.0)
                .ok_or("invalid DASH timeShiftBufferDepth")
        })
        .transpose()?
        .unwrap_or(60.0);
    let elapsed = now - epoch - fraction - period_start;
    if !elapsed.is_finite() || !period_start.is_finite() || period_start < 0.0 {
        return Err("invalid dynamic DASH clock or Period start".into());
    }
    *expired = period_duration.is_some_and(|duration| elapsed - depth >= duration);
    if *expired {
        return Ok(Vec::new());
    }
    let edge = period_duration.map_or(elapsed, |duration| elapsed.min(duration));
    if edge <= 0.0 {
        return Ok(Vec::new());
    }
    let units = edge * timescale as f64;
    if !units.is_finite() || units >= (1u64 << 53) as f64 {
        return Err("dynamic DASH timeline exceeds precise integer range".into());
    }
    let count = if period_duration.is_some_and(|duration| elapsed >= duration) {
        ceil_segment_units(units / duration as f64)
    } else {
        units / duration as f64
    }
    .floor() as u64;
    let first = (((elapsed - depth).max(0.0) * timescale as f64) / duration as f64).floor() as u64;
    let first = first.max(count.saturating_sub(MAX_LIVE_SEGMENTS));
    *segment_budget = segment_budget
        .checked_sub(count.saturating_sub(first))
        .ok_or("dynamic DASH manifest exceeds live segment budget")?;
    let mut media = Vec::new();
    for index in first..count {
        let number = start_number
            .checked_add(index)
            .ok_or("DASH segment number overflow")?;
        let time = index
            .checked_mul(duration)
            .and_then(|time| time.checked_add(offset))
            .ok_or("DASH timeline overflow")?;
        media.push(resolve(
            base_url,
            &apply_template(template, id, number, time, bandwidth)?,
        ));
        let end = (index + 1) as f64 * duration as f64 / timescale as f64;
        let begin = index as f64 * duration as f64 / timescale as f64;
        durations.push((end.min(edge) - begin.max(0.0)).max(0.0));
    }
    Ok(media)
}

fn expand_timeline(
    templates: &[roxmltree::Node<'_, '_>],
    template: &str,
    base_url: &str,
    id: &str,
    bandwidth: u64,
    period_duration: Option<f64>,
    durations: &mut Vec<f64>,
) -> Result<Vec<String>, String> {
    let template_attr = |key: &str| {
        templates
            .iter()
            .find_map(|template| template.attribute(key))
    };
    let timeline = templates
        .iter()
        .find_map(|template| mpd_child(*template, "SegmentTimeline"));
    let start_number = template_attr("startNumber")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1);
    apply_template(template, id, start_number, 0, bandwidth)?;
    let mut media = Vec::new();
    let mut number = start_number;
    let mut clock = 0u64;
    // 固定时长模板没有 SegmentTimeline，按 Period 的边界展开，保留最后一个短分片。
    if timeline.is_none() {
        if let (Some(seconds), Some(duration)) = (
            period_duration,
            template_attr("duration").and_then(|value| value.parse::<u64>().ok()),
        ) {
            let timescale = template_attr("timescale")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(1);
            let count = ceil_segment_units(seconds * timescale as f64 / duration as f64);
            if duration > 0 && timescale > 0 && count.is_finite() && count <= 1_000_000.0 {
                let offset = template_attr("presentationTimeOffset")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                for index in 0..count as u64 {
                    durations.push((duration as f64 / timescale as f64).min(
                        (seconds - index as f64 * duration as f64 / timescale as f64).max(0.0),
                    ));
                    media.push(resolve(
                        base_url,
                        &apply_template(
                            template,
                            id,
                            start_number.saturating_add(index),
                            offset.saturating_add(index.saturating_mul(duration)),
                            bandwidth,
                        )?,
                    ));
                }
            }
        }
        return Ok(media);
    }
    let timeline = timeline.unwrap();
    let timescale = template_attr("timescale")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1)
        .max(1);
    for entry in timeline
        .children()
        .filter(|node| mpd_element(*node, "S", timeline))
    {
        let duration = entry
            .attribute("d")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|duration| *duration > 0)
            .ok_or("invalid DASH timeline duration")?;
        let time = entry
            .attribute("t")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(clock);
        let repeat = entry
            .attribute("r")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        let repeat = if repeat == -1 {
            let next_time = entry
                .next_siblings()
                .skip(1)
                .find(|node| mpd_element(*node, "S", timeline))
                .and_then(|next| next.attribute("t")?.parse::<u64>().ok());
            let boundary = next_time
                .or_else(|| {
                    period_duration.map(|seconds| {
                        let offset = template_attr("presentationTimeOffset")
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(0);
                        offset.saturating_add(ceil_segment_units(seconds * timescale as f64) as u64)
                    })
                })
                .filter(|boundary| *boundary > time)
                .ok_or("unbounded DASH negative timeline repeat")?;
            (boundary - time).div_ceil(duration) - 1
        } else {
            u64::try_from(repeat).map_err(|_| "invalid DASH timeline repeat")?
        };
        if repeat >= 1_000_000 || media.len() as u64 + repeat + 1 > 1_000_000 {
            return Err("DASH timeline exceeds segment limit".into());
        }
        clock = time;
        for _ in 0..=repeat {
            durations.push(duration as f64 / timescale as f64);
            media.push(resolve(
                base_url,
                &apply_template(template, id, number, clock, bandwidth)?,
            ));
            number = number
                .checked_add(1)
                .ok_or("DASH segment number overflow")?;
            clock = clock
                .checked_add(duration)
                .ok_or("DASH timeline overflow")?;
        }
    }
    Ok(media)
}

const MAX_DASH_TEMPLATE_WIDTH: usize = 32;
const MAX_DASH_TEMPLATE_URL_BYTES: usize = 8192;

fn apply_template(
    template: &str,
    id: &str,
    number: u64,
    time: u64,
    bandwidth: u64,
) -> Result<String, String> {
    fn append(out: &mut String, value: &str) -> Result<(), String> {
        if value.len() > MAX_DASH_TEMPLATE_URL_BYTES - out.len() {
            return Err("DASH template URL exceeds byte limit".into());
        }
        out.push_str(value);
        Ok(())
    }

    let mut out = String::new();
    let mut remaining = template;
    while let Some(start) = remaining.find('$') {
        append(&mut out, &remaining[..start])?;
        remaining = &remaining[start + 1..];
        if let Some(rest) = remaining.strip_prefix('$') {
            append(&mut out, "$")?;
            remaining = rest;
            continue;
        }
        let end = remaining
            .find('$')
            .ok_or("unterminated DASH template identifier")?;
        let token = &remaining[..end];
        remaining = &remaining[end + 1..];
        let (identifier, format) = token
            .split_once('%')
            .map_or((token, None), |(name, format)| (name, Some(format)));
        if identifier == "RepresentationID" {
            if format.is_some() {
                return Err("DASH RepresentationID cannot have a format tag".into());
            }
            append(&mut out, id)?;
            continue;
        }
        let value = match identifier {
            "Number" => number,
            "Bandwidth" => bandwidth,
            "Time" => time,
            "SubNumber" => {
                return Err("DASH SubNumber requires unsupported subsegment addressing".into())
            }
            _ => return Err("unknown DASH template identifier".into()),
        };
        let width = match format {
            None => 0,
            Some(format) => {
                let digits = format
                    .strip_prefix('0')
                    .and_then(|value| value.strip_suffix('d'))
                    .filter(|value| value.bytes().all(|ch| ch.is_ascii_digit()))
                    .ok_or("invalid DASH template format tag")?;
                let width = if digits.is_empty() {
                    0
                } else {
                    digits
                        .parse::<usize>()
                        .map_err(|_| "DASH template width exceeds limit")?
                };
                if width > MAX_DASH_TEMPLATE_WIDTH {
                    return Err("DASH template width exceeds limit".into());
                }
                width
            }
        };
        let length = value.to_string().len().max(width);
        if length > MAX_DASH_TEMPLATE_URL_BYTES - out.len() {
            return Err("DASH template URL exceeds byte limit".into());
        }
        append(&mut out, &format!("{value:0width$}"))?;
    }
    append(&mut out, remaining)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpd_xml_namespaces_comments_and_signed_urls_keep_the_same_tracks() {
        let plain = r#"<MPD mediaPresentationDuration="PT4S"><Period><AdaptationSet mimeType="video/mp4">
<BaseURL>https://cdn.test/video/</BaseURL><SegmentTemplate initialization="init.mp4?key=a&amp;sig=b" media="$Number$.m4s?key=a&amp;sig=b" duration="2"/>
<Representation id="v" bandwidth="1000" height="720"/>
</AdaptationSet><AdaptationSet mimeType="audio/mp4" lang="zh"><Representation id="a"><BaseURL>audio.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>"#;
        let namespaced = plain.replace('<', "<d:").replace("<d:/", "</d:").replacen(
            "<d:MPD ",
            "<d:MPD xmlns:d=\"urn:mpeg:dash:schema:mpd:2011\" ",
            1,
        );
        let namespaced = format!(
            "<?xml version=\"1.0\"?><!-- <Period><Representation id=\"fake\"/> -->{namespaced}"
        );
        let expected = parse_mpd(plain, "https://origin.test/manifest.mpd").unwrap();
        assert_eq!(
            parse_mpd(&namespaced, "https://origin.test/manifest.mpd").unwrap(),
            expected
        );
        assert_eq!(
            expected.representations[0].media,
            [
                "https://cdn.test/video/1.m4s?key=a&sig=b",
                "https://cdn.test/video/2.m4s?key=a&sig=b",
            ]
        );
        assert_eq!(expected.representations[1].lang, "zh");
        for xml in [
            format!("<html>{plain}</html>"),
            format!("<!-- {plain} -->"),
            plain.replace("</MPD>", ""),
        ] {
            assert!(parse_mpd(&xml, "https://origin.test/manifest.mpd").is_err());
        }
    }

    #[test]
    fn core_completion_dash_inherits_bases_lists_and_decodes_signed_urls() {
        let xml = r#"<MPD><BaseURL serviceLocation="primary">https://cdn.test/media/</BaseURL><Period>
<BaseURL>episode/</BaseURL><AdaptationSet mimeType="video/mp4"><BaseURL>video/</BaseURL>
<SegmentList duration="4"><Initialization sourceURL="init.mp4?part=0&amp;sig=abc"/><SegmentURL media="first.m4s?part=1&#38;sig=abc"/></SegmentList>
<Representation id="one"><BaseURL>low/</BaseURL></Representation>
<Representation id="two"><BaseURL><![CDATA[high/]]></BaseURL></Representation>
<Representation id="own"><SegmentTemplate initialization="own-init.mp4" media="own-$Number$.m4s"><SegmentTimeline><S d="2" r="1"/></SegmentTimeline></SegmentTemplate></Representation>
</AdaptationSet><AdaptationSet mimeType="audio/mp4"><Representation id="audio"><BaseURL>audio.mp4?part=0&amp;sig=abc</BaseURL></Representation></AdaptationSet>
</Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://origin.test/manifest.mpd").unwrap();
        for (representation, directory) in
            parsed.representations.iter().take(2).zip(["low", "high"])
        {
            let base = format!("https://cdn.test/media/episode/video/{directory}/");
            assert_eq!(representation.base_url, base);
            assert_eq!(
                representation.init.as_deref(),
                Some(format!("{base}init.mp4?part=0&sig=abc").as_str())
            );
            assert_eq!(
                representation.media,
                [format!("{base}first.m4s?part=1&sig=abc")]
            );
            assert_eq!(representation.media_durations, [4.0]);
        }
        assert_eq!(
            parsed.representations[2].media,
            [
                "https://cdn.test/media/episode/video/own-1.m4s",
                "https://cdn.test/media/episode/video/own-2.m4s"
            ]
        );
        assert_eq!(
            parsed.representations[2].init.as_deref(),
            Some("https://cdn.test/media/episode/video/own-init.mp4")
        );
        assert_eq!(
            parsed.representations[3].base_url,
            "https://cdn.test/media/episode/audio.mp4?part=0&sig=abc"
        );
        assert!(parsed.representations[3].media.is_empty());

        let xml = r#"<MPD><Period><AdaptationSet mimeType="video/mp4">
<SegmentTemplate initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Number$-$Time$.m4s" timescale="100" startNumber="1"><SegmentTimeline><S t="100" d="100" r="1"/></SegmentTimeline></SegmentTemplate>
<Representation id="own"><SegmentTemplate timescale="10" startNumber="9"><SegmentTimeline><S t="20" d="20" r="1"/></SegmentTimeline></SegmentTemplate></Representation>
<Representation id="inherited"><SegmentTemplate startNumber="4"/></Representation>
</AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(
            parsed.representations[0].media,
            [
                "https://cdn.test/own/9-20.m4s",
                "https://cdn.test/own/10-40.m4s"
            ]
        );
        assert_eq!(parsed.representations[0].media_durations, [2.0, 2.0]);
        assert_eq!(
            parsed.representations[0].init.as_deref(),
            Some("https://cdn.test/own/init.mp4")
        );
        assert_eq!(
            parsed.representations[1].media,
            [
                "https://cdn.test/inherited/4-100.m4s",
                "https://cdn.test/inherited/5-200.m4s"
            ]
        );
        assert_eq!(parsed.representations[1].media_durations, [1.0, 1.0]);
    }

    #[test]
    fn dynamic_timeline_uses_explicit_period_end_without_rounding_extra_segment() {
        let xml = r#"<MPD type = 'dynamic'><Period duration='PT0.07S'><Representation id='v'><SegmentTemplate timescale='100' media='$Time$.m4s'><SegmentTimeline><S t='0' d='1' r='-1'/></SegmentTimeline></SegmentTemplate></Representation></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/list.mpd").unwrap();
        assert!(parsed.dynamic);
        assert_eq!(
            parsed.representations[0].media,
            (0..7)
                .map(|n| format!("https://cdn.test/{n}.m4s"))
                .collect::<Vec<_>>()
        );
        assert_eq!(ceil_segment_units(0.07 * 100.0), 7.0);
        assert_eq!(ceil_segment_units(7.1), 8.0);
    }

    #[test]
    fn negative_timeline_repeat_uses_next_time_or_period_end() {
        for (timeline, offset) in [
            ("<S t=\"0\" d=\"4\" r=\"-1\"/><S\n t=\"12\" d=\"4\"/>", 0),
            ("<S t=\"8\" d=\"4\" r=\"-1\"/>", 8),
        ] {
            let xml = format!(
                r#"<MPD type="static"><Period duration="PT16S"><AdaptationSet><Representation id="v"><SegmentTemplate timescale="1" presentationTimeOffset="{offset}" media="$Time$.m4s"><SegmentTimeline>{timeline}</SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#
            );
            let parsed = parse_mpd(&xml, "https://cdn.test/list.mpd").unwrap();
            assert_eq!(
                parsed.representations[0].media,
                (0..4)
                    .map(|n| format!("https://cdn.test/{}.m4s", offset + n * 4))
                    .collect::<Vec<_>>()
            );
            assert_eq!(parsed.representations[0].media_durations, vec![4.0; 4]);
        }
        let xml = r#"<MPD><Period><Representation id="v"><SegmentTemplate media="$Time$.m4s"><SegmentTimeline><S d="4" r="-1"/></SegmentTimeline></SegmentTemplate></Representation></Period></MPD>"#;
        assert!(parse_mpd(xml, "https://cdn.test/list.mpd")
            .unwrap_err()
            .contains("unbounded"));
    }

    #[test]
    fn byte_range_segment_lists_are_not_treated_as_whole_files() {
        let xml = r#"<MPD><Period><Representation id="v"><BaseURL>all.mp4?token=signed</BaseURL><SegmentList duration="2"><Initialization range="0-99"/><SegmentURL mediaRange="100-199"/><SegmentURL media="all.mp4?token=signed" mediaRange="200-299"/></SegmentList></Representation></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/list.mpd").unwrap();
        let track = &parsed.representations[0];
        assert_eq!(
            track.init.as_deref(),
            Some("https://cdn.test/all.mp4?token=signed")
        );
        assert_eq!(
            track.init_range,
            Some(ByteRange {
                start: 0,
                length: 100
            })
        );
        assert_eq!(track.media, vec![track.init.clone().unwrap(); 2]);
        assert_eq!(
            track.media_ranges,
            vec![
                Some(ByteRange {
                    start: 100,
                    length: 100
                }),
                Some(ByteRange {
                    start: 200,
                    length: 100
                })
            ]
        );
        assert_eq!(track.media_durations, vec![2.0, 2.0]);
        for value in [
            "",
            "99-0",
            "0-18446744073709551615",
            "18446744073709551616-18446744073709551617",
            "0-",
            "-1",
            "+0-99",
            "0-99-100",
        ] {
            for (element, field) in [("Initialization", "range"), ("SegmentURL", "mediaRange")] {
                let xml = format!("<MPD><Period><Representation><SegmentList><{element} {field}=\"{value}\"/></SegmentList></Representation></Period></MPD>");
                assert!(
                    parse_mpd(&xml, "https://cdn.test/list.mpd")
                        .unwrap_err()
                        .contains("byte range"),
                    "{xml}"
                );
            }
        }
    }

    fn serve_dash_files(
        files: HashMap<String, Vec<u8>>,
        requests: usize,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..requests {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                let path = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let body = files
                    .get(path)
                    .unwrap_or_else(|| panic!("unexpected request: {request}"));
                let range = request.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("Range").then_some(value.trim())
                });
                if let Some(range) = range {
                    let (start, end) = range
                        .strip_prefix("bytes=")
                        .unwrap()
                        .split_once('-')
                        .unwrap();
                    let start = start.parse::<usize>().unwrap();
                    let end = end.parse::<usize>().unwrap();
                    let bytes = &body[start..=end];
                    write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len(), bytes.len()).unwrap();
                    socket.write_all(bytes).unwrap();
                } else {
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    socket.write_all(body).unwrap();
                }
                seen.push(request);
            }
            seen
        });
        (url, worker)
    }

    fn serve_byte_ranges(
        body: Vec<u8>,
        requests: usize,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let (base, worker) = serve_dash_files(
            HashMap::from([("/all.mp4?token=signed".into(), body)]),
            requests,
        );
        (format!("{base}/all.mp4?token=signed"), worker)
    }

    fn sidx_fixture(
        version: u8,
        first_offset: u64,
        wide_box: bool,
        entries: &[(u32, u32)],
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&(u32::from(version) << 24).to_be_bytes());
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&1000u32.to_be_bytes());
        if version == 0 {
            payload.extend_from_slice(&0u32.to_be_bytes());
            payload.extend_from_slice(&(first_offset as u32).to_be_bytes());
        } else {
            payload.extend_from_slice(&0x1_0000_0020u64.to_be_bytes());
            payload.extend_from_slice(&first_offset.to_be_bytes());
        }
        payload.extend_from_slice(&0u16.to_be_bytes());
        payload.extend_from_slice(&(entries.len() as u16).to_be_bytes());
        for (size, duration) in entries {
            payload.extend_from_slice(&size.to_be_bytes());
            payload.extend_from_slice(&duration.to_be_bytes());
            payload.extend_from_slice(&0x9000_0000u32.to_be_bytes());
        }
        let mut bytes = Vec::new();
        if wide_box {
            bytes.extend_from_slice(&1u32.to_be_bytes());
            bytes.extend_from_slice(b"sidx");
            bytes.extend_from_slice(&(payload.len() as u64 + 16).to_be_bytes());
        } else {
            bytes.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
            bytes.extend_from_slice(b"sidx");
        }
        bytes.extend(payload);
        bytes
    }

    #[test]
    fn sidx_versions_and_box_sizes_use_the_box_end_as_the_offset_anchor() {
        for version in [0, 1] {
            for wide in [false, true] {
                let offset = if version == 0 { 16 } else { 0x1_0000_0010 };
                let sidx = sidx_fixture(version, offset, wide, &[(20, 1000), (30, 1500)]);
                let free = [0, 0, 0, 8, b'f', b'r', b'e', b'e'];
                let bytes = [free.as_slice(), &sidx, &free].concat();
                let start = 100 + free.len() as u64 + sidx.len() as u64 + offset;
                assert_eq!(
                    parse_sidx(&bytes, 100).unwrap(),
                    vec![
                        (ByteRange { start, length: 20 }, 1.0),
                        (
                            ByteRange {
                                start: start + 20,
                                length: 30
                            },
                            1.5
                        ),
                    ]
                );
            }
        }
    }

    #[test]
    fn sidx_rejects_malformed_nested_and_overflowing_indices() {
        let good = sidx_fixture(0, 0, false, &[(20, 1000)]);
        for end in 0..good.len() {
            assert!(parse_sidx(&good[..end], 0).is_err(), "prefix {end}");
        }
        for (offset, value) in [
            (0, 0),
            (0, 4),
            (8, 2 << 24),
            (8, 1),
            (16, 0),
            (28, 0),
            (28, 2),
            (32, 0),
            (36, 0),
            (32, 0x8000_0014),
        ] {
            let mut bad = good.clone();
            bad[offset..offset + 4].copy_from_slice(&u32::to_be_bytes(value));
            assert!(parse_sidx(&bad, 0).is_err(), "field {offset}={value}");
        }
        assert!(
            parse_sidx(&sidx_fixture(1, u64::MAX, false, &[(20, 1000)]), 1)
                .unwrap_err()
                .contains("overflow")
        );
        assert!(parse_sidx(&good, u64::MAX - good.len() as u64 - 10)
            .unwrap_err()
            .contains("overflow"));
        let mut oversized = vec![0; MAX_SIDX_BYTES as usize + 1];
        oversized[..good.len()].copy_from_slice(&good);
        assert!(parse_sidx(&oversized, 0).unwrap_err().contains("limit"));
    }

    #[test]
    fn segment_base_inherits_overrides_and_respects_nearer_addressing_types() {
        let xml = r#"<MPD><Period><SegmentBase indexRange="100-199"><Initialization range="0-99"/></SegmentBase><AdaptationSet mimeType="video/mp4"><BaseURL>https://cdn.test/</BaseURL>
<Representation id="inherit"><BaseURL>in.mp4?sig=a&amp;b=c</BaseURL><SegmentBase/></Representation>
<Representation id="override"><BaseURL>out.mp4</BaseURL><SegmentBase indexRange="200-299"><Initialization sourceURL="init.mp4" range="10-49"/></SegmentBase></Representation>
<Representation id="list"><SegmentList><SegmentURL media="seg.m4s"/></SegmentList></Representation>
<Representation id="template"><SegmentTemplate media="$Number$.m4s"><SegmentTimeline><S d="2"/></SegmentTimeline></SegmentTemplate></Representation>
</AdaptationSet></Period></MPD>"#;
        let tracks = parse_mpd(xml, "https://origin.test/mpd")
            .unwrap()
            .representations;
        assert_eq!(
            tracks[0].index_range,
            Some(ByteRange {
                start: 100,
                length: 100
            })
        );
        assert_eq!(
            tracks[0].init.as_deref(),
            Some("https://cdn.test/in.mp4?sig=a&b=c")
        );
        assert_eq!(
            tracks[0].init_range,
            Some(ByteRange {
                start: 0,
                length: 100
            })
        );
        assert_eq!(
            tracks[1].index_range,
            Some(ByteRange {
                start: 200,
                length: 100
            })
        );
        assert_eq!(tracks[1].init.as_deref(), Some("https://cdn.test/init.mp4"));
        assert_eq!(
            tracks[1].init_range,
            Some(ByteRange {
                start: 10,
                length: 40
            })
        );
        assert_eq!(tracks[2].index_range, None);
        assert_eq!(tracks[2].media, vec!["https://cdn.test/seg.m4s"]);
        assert_eq!(tracks[3].index_range, None);
        assert_eq!(tracks[3].media, vec!["https://cdn.test/1.m4s"]);
        for ancestor in [
            "<SegmentList><Initialization sourceURL=\"wrong.mp4\"/></SegmentList>",
            "<SegmentTemplate initialization=\"wrong.mp4\"/>",
        ] {
            let xml = format!(
                r#"<MPD><Period>{ancestor}<Representation><SegmentBase indexRange="50-99"><Initialization range="0-49"/></SegmentBase></Representation></Period></MPD>"#
            );
            let track = parse_mpd(&xml, "https://cdn.test/all.mp4")
                .unwrap()
                .representations
                .remove(0);
            assert!(track.media.is_empty());
            assert_eq!(track.init.as_deref(), Some("https://cdn.test/all.mp4"));
            assert_eq!(
                track.index_range,
                Some(ByteRange {
                    start: 50,
                    length: 50
                })
            );
        }
        for base in ["<SegmentBase><RepresentationIndex sourceURL=\"other.idx\"/></SegmentBase>", "<SegmentBase indexRange=\"1-10\"><RepresentationIndex sourceURL=\"other.idx\"/></SegmentBase>"] {
            let xml = format!("<MPD><Period><Representation>{base}</Representation></Period></MPD>");
            assert!(parse_mpd(&xml, "https://cdn.test/all.mp4").unwrap_err().contains("unsupported"));
        }
    }

    #[test]
    fn segment_base_without_index_range_is_one_whole_file_segment() {
        let xml = r#"<MPD type="static" mediaPresentationDuration="PT6.5S"><Period><BaseURL>https://cdn.test/v/</BaseURL>
<Representation id="bare" mimeType="video/mp4"><BaseURL>bare.mp4?sig=1</BaseURL><SegmentBase/></Representation>
<Representation id="same" mimeType="video/mp4"><BaseURL>same.mp4</BaseURL><SegmentBase><Initialization range="0-99"/></SegmentBase></Representation>
<Representation id="split" mimeType="video/mp4"><BaseURL>body.mp4</BaseURL><SegmentBase timescale="1000"><Initialization sourceURL="head.mp4" range="0-49"/></SegmentBase></Representation>
</Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        let [bare, same, split] = &parsed.representations[..] else {
            panic!("three representations")
        };
        assert!(bare.whole_file && same.whole_file && split.whole_file);
        assert_eq!(bare.media, ["https://cdn.test/v/bare.mp4?sig=1"]);
        assert_eq!(bare.media_ranges, [None]);
        assert_eq!(bare.media_durations, [6.5]);
        assert_eq!(bare.init, None);
        // The ranged init lives inside the whole file, so it is not fetched twice.
        assert_eq!(
            (same.media.len(), same.init.clone(), same.init_range),
            (1, None, None)
        );
        assert_eq!(split.init.as_deref(), Some("https://cdn.test/v/head.mp4"));
        assert_eq!(
            split.init_range,
            Some(ByteRange {
                start: 0,
                length: 50
            })
        );
        assert_eq!(split.media, ["https://cdn.test/v/body.mp4"]);
        // expand_segment_base leaves a whole-file track untouched (no index fetch).
        let mut track = bare.clone();
        expand_segment_base(
            &mut track,
            &HashMap::new(),
            "",
            Path::new("/nonexistent-control"),
        )
        .unwrap();
        assert_eq!(&track, bare);
        // Whole-file tracks embed their own moov; stitching periods of them is refused.
        let two = r#"<MPD type="static"><Period duration="PT1S"><Representation id="v" mimeType="video/mp4"><BaseURL>a.mp4</BaseURL><SegmentBase/></Representation></Period><Period duration="PT1S"><Representation id="v" mimeType="video/mp4"><BaseURL>b.mp4</BaseURL><SegmentBase/></Representation></Period></MPD>"#;
        let parsed = parse_mpd(two, "https://cdn.test/m.mpd").unwrap();
        assert!(
            concat_period_parts(period_parts(&parsed, &parsed.representations[0]).unwrap())
                .unwrap_err()
                .contains("SegmentBase")
        );
    }

    #[test]
    fn whole_file_segment_base_downloads_whole_resource() {
        let body = b"ftypmoovmdat-whole-file".to_vec();
        let (base, server) =
            serve_dash_files(HashMap::from([("/all.mp4".into(), body.clone())]), 1);
        let xml = r#"<MPD><Period duration="PT2S"><Representation id="v" mimeType="video/mp4"><SegmentBase/></Representation></Period></MPD>"#;
        let track = parse_mpd(xml, &format!("{base}/all.mp4"))
            .unwrap()
            .representations
            .remove(0);
        let root = std::env::temp_dir().join(format!("hls-dash-whole-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let (files, _) = download_dash_primary(
            &track,
            true,
            &root,
            &HashMap::new(),
            "",
            &control,
            &mut DashCheckpoint::load(&root),
            None,
            1,
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(fs::read(&files[0]).unwrap(), body);
        let request = server.join().unwrap().remove(0).to_ascii_lowercase();
        assert!(
            !request.contains("range:"),
            "whole resource must not be ranged"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multi_period_primary_writes_period_maps_into_local_playlists() {
        let files = HashMap::from([
            ("/p1/init.mp4".to_string(), b"I1".to_vec()),
            ("/p1/1.m4s".to_string(), b"A1".to_vec()),
            ("/p2/init.mp4".to_string(), b"I2".to_vec()),
            ("/p2/1.m4s".to_string(), b"B1".to_vec()),
            ("/p2/2.m4s".to_string(), b"B2".to_vec()),
        ]);
        let (base, server) = serve_dash_files(files, 5);
        let xml = r#"<MPD type="static"><Period duration="PT2S"><BaseURL>p1/</BaseURL><Representation id="v" mimeType="video/mp4"><SegmentTemplate duration="2" initialization="init.mp4" media="$Number$.m4s"/></Representation></Period><Period duration="PT2S"><BaseURL>p2/</BaseURL><Representation id="v" mimeType="video/mp4"><SegmentList duration="1"><Initialization sourceURL="init.mp4"/><SegmentURL media="1.m4s"/><SegmentURL media="2.m4s"/></SegmentList></Representation></Period></MPD>"#;
        let parsed = parse_mpd(xml, &format!("{base}/manifest.mpd")).unwrap();
        let track = concat_period_parts(period_parts(&parsed, &parsed.representations[0]).unwrap())
            .unwrap();
        let root = std::env::temp_dir().join(format!("hls-dash-periods-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let (files, _) = download_dash_primary(
            &track,
            true,
            &root,
            &HashMap::new(),
            "",
            &control,
            &mut DashCheckpoint::load(&root),
            None,
            5,
        )
        .unwrap();
        assert_eq!(
            files
                .iter()
                .flat_map(|path| fs::read(path).unwrap())
                .collect::<Vec<_>>(),
            b"I1A1I2B1B2"
        );
        write_dash_playlist(&root, &files).unwrap();
        let local = fs::read_to_string(root.join("local.m3u8")).unwrap();
        assert!(local.contains("#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.000,\nseg-0001.m4s\n#EXT-X-DISCONTINUITY\n#EXT-X-MAP:URI=\"seg-0002-init.mp4\"\n#EXTINF:4.000,\nseg-0003.m4s\n#EXTINF:4.000,\nseg-0004.m4s\n"), "{local}");
        let preview = fs::read_to_string(root.join("preview-video.m3u8")).unwrap();
        assert!(preview.contains("#EXTINF:2.000000,\nseg-0001.m4s\n#EXT-X-DISCONTINUITY\n#EXT-X-MAP:URI=\"seg-0002-init.mp4\"\n#EXTINF:1.000000,\nseg-0003.m4s"), "{preview}");
        assert_eq!(server.join().unwrap().len(), 5);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_multi_period_av_download_merges_through_ffmpeg() {
        let Some(ffmpeg) = crate::media::merge::locate_ffmpeg() else {
            eprintln!("skipped: no FFmpeg on PATH/HLS_FFMPEG");
            return;
        };
        for shared_init in [false, true] {
            let root =
                std::env::temp_dir().join(format!("hls-dash-real-periods-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let mut files = HashMap::new();
            let mut periods = Vec::new();
            let mut requests = 1;
            for (period, tone) in [("p1", 440), ("p2", 880)] {
                let dir = root.join("src").join(period);
                fs::create_dir_all(&dir).unwrap();
                let status = Command::new(&ffmpeg)
                    .current_dir(&dir)
                    .args([
                        "-v",
                        "error",
                        "-nostdin",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "testsrc=size=160x120:rate=25:duration=2",
                        "-f",
                        "lavfi",
                        "-i",
                    ])
                    .arg(format!(
                        "sine=frequency={tone}:sample_rate=48000:duration=2"
                    ))
                    .args([
                        "-map",
                        "0:v",
                        "-map",
                        "1:a",
                        "-c:v",
                        "libx264",
                        "-g",
                        "25",
                        "-pix_fmt",
                        "yuv420p",
                        "-c:a",
                        "aac",
                        "-f",
                        "dash",
                        "-seg_duration",
                        "1",
                        "-use_template",
                        "1",
                        "-use_timeline",
                        "1",
                        "-init_seg_name",
                        "init-$RepresentationID$.m4s",
                        "-media_seg_name",
                        "chunk-$RepresentationID$-$Number%05d$.m4s",
                        "out.mpd",
                    ])
                    .status()
                    .unwrap();
                if !status.success() {
                    eprintln!("skipped: FFmpeg cannot build the fixture (libx264/aac missing?)");
                    let _ = fs::remove_dir_all(&root);
                    return;
                }
                let mut sets = String::new();
                for (id, kind) in [("0", "video"), ("1", "audio")] {
                    let mut segments: Vec<_> = fs::read_dir(&dir)
                        .unwrap()
                        .filter_map(|entry| {
                            let name = entry.unwrap().file_name().into_string().unwrap();
                            name.starts_with(&format!("chunk-{id}-")).then_some(name)
                        })
                        .collect();
                    segments.sort();
                    for name in segments.iter().chain([&format!("init-{id}.m4s")]) {
                        let bytes = fs::read(dir.join(name)).unwrap();
                        if shared_init && name.starts_with("init-") {
                            let key = format!("/{name}");
                            if let Some(first) = files.get(&key) {
                                assert_eq!(
                                    first, &bytes,
                                    "shared initialization must be identical"
                                );
                            } else {
                                files.insert(key, bytes);
                                requests += 1;
                            }
                        } else {
                            files.insert(format!("/{period}/{name}"), bytes);
                            requests += 1;
                        }
                    }
                    let list: String = segments
                        .iter()
                        .map(|name| format!(r#"<SegmentURL media="{name}"/>"#))
                        .collect();
                    let init_prefix = if shared_init { "/" } else { "" };
                    sets.push_str(&format!(r#"<AdaptationSet contentType="{kind}" mimeType="{kind}/mp4"><Representation id="{period}-{kind}" bandwidth="1000"><SegmentList><Initialization sourceURL="{init_prefix}init-{id}.m4s"/>{list}</SegmentList></Representation></AdaptationSet>"#));
                }
                let start = if period == "p1" { 0 } else { 2 };
                periods.push(format!(
                    r#"<Period id="{period}" start="PT{start}S" duration="PT2S"><BaseURL>{period}/</BaseURL>{sets}</Period>"#
                ));
            }
            files.insert(
                "/manifest.mpd".into(),
                format!(
                    r#"<MPD type="static" mediaPresentationDuration="PT4S">{}</MPD>"#,
                    periods.concat()
                )
                .into_bytes(),
            );
            let (base, server) = serve_dash_files(files.clone(), requests);
            let task = root.join("task");
            let control = root.join("control");
            fs::write(&control, "run").unwrap();
            let output = download_dash_selected(
                &format!("{base}/manifest.mpd"),
                &HashMap::new(),
                "",
                &task,
                &control,
                0,
                0,
                false,
                "",
            )
            .unwrap();
            assert_eq!(server.join().unwrap().len(), requests);
            let decode = |output: &Path| {
                let frames = Command::new(&ffmpeg)
                    .args(["-v", "error", "-nostdin", "-i"])
                    .arg(output)
                    .args(["-f", "framecrc", "-"])
                    .output()
                    .unwrap();
                assert!(
                    frames.status.success(),
                    "{}",
                    String::from_utf8_lossy(&frames.stderr)
                );
                String::from_utf8(frames.stdout).unwrap()
            };
            let text = decode(&output);
            let video = text.lines().filter(|line| line.starts_with("0,")).count();
            let audio = text.lines().filter(|line| line.starts_with("1,")).count();
            assert_eq!(video, 100, "both Periods' video frames survive the merge");
            assert!(
                audio >= 180,
                "both Periods' audio survive the merge: {audio}"
            );
            // Keep the media URLs identical across Periods to catch URL-only
            // deduplication. Initialization URLs still cover both variants.
            let periods: Vec<_> = periods
                .iter()
                .map(|period| period.replace(r#"media="chunk-"#, r#"media="/reused-chunk-"#))
                .collect();
            let manifest_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let hits = manifest_hits.clone();
            let origin =
                super::super::harness::serve_dynamic(std::sync::Arc::new(move |path, hit| {
                    if path == "/manifest.mpd" {
                        hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let (kind, periods) = match hit {
                            1 => ("dynamic", periods[0].clone()),
                            2 if shared_init => ("dynamic", periods[1].clone()), // no overlap
                            2 => ("dynamic", periods.concat()),
                            3 => ("dynamic", periods[1].clone()),
                            4 => ("dynamic", periods[0].clone()), // stale cache after the rollover
                            _ => ("static", periods[1].clone()),
                        };
                        return Some(
                            format!(
                                r#"<MPD type="{kind}" minimumUpdatePeriod="PT0.4S">{periods}</MPD>"#
                            )
                            .into_bytes(),
                        );
                    }
                    if let Some(name) = path.strip_prefix("/reused-") {
                        assert!(hit <= 2, "duplicate segment requested: {path}");
                        let period = if hit == 1 { "p1" } else { "p2" };
                        return files.get(&format!("/{period}/{name}")).cloned();
                    }
                    files.get(path).cloned()
                }));
            let live_task = root.join("live");
            let live_output = download_dash_selected(
                &format!("{}/manifest.mpd", origin.base),
                &HashMap::new(),
                "",
                &live_task,
                &control,
                0,
                0,
                false,
                "",
            )
            .unwrap();
            assert_eq!(manifest_hits.load(std::sync::atomic::Ordering::Relaxed), 5);
            let live_frames = decode(&live_output);
            assert_eq!(live_frames.lines().count(), text.lines().count());
            for (live, vod) in live_frames.lines().zip(text.lines()) {
                assert_eq!(live, vod, "live and VOD decoded timestamps and checksums");
            }
            for playlist in ["local.m3u8", "audio/local.m3u8"] {
                let listed = fs::read_to_string(live_task.join(playlist)).unwrap();
                assert_eq!(listed.matches("#EXT-X-DISCONTINUITY").count(), 1);
                assert_eq!(
                    listed.matches("#EXT-X-MAP:").count(),
                    if shared_init { 1 } else { 2 }
                );
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn segment_base_downloads_with_credentials_and_reuses_checked_media() {
        let index = sidx_fixture(0, 0, false, &[(4, 1000), (4, 2000)]);
        let start = 4 + index.len();
        let (url, server) =
            serve_byte_ranges([b"INIT".as_slice(), &index, b"AAAABBBB"].concat(), 4);
        let xml = format!(
            r#"<MPD><Period><Representation mimeType="video/mp4"><SegmentBase indexRange="4-{}"><Initialization range="0-3"/></SegmentBase></Representation></Period></MPD>"#,
            start - 1
        );
        let mut track = parse_mpd(&xml, &url).unwrap().representations.remove(0);
        let root = std::env::temp_dir().join(format!("dash-sidx-download-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let headers = HashMap::from([
            ("Cookie".into(), "session=sidx".into()),
            ("Authorization".into(), "Bearer sidx".into()),
        ]);
        expand_segment_base(&mut track, &headers, "", &control).unwrap();
        assert_eq!(track.media_durations, vec![1.0, 2.0]);
        assert_eq!(
            track.media_ranges,
            vec![
                Some(ByteRange {
                    start: start as u64,
                    length: 4
                }),
                Some(ByteRange {
                    start: start as u64 + 4,
                    length: 4
                })
            ]
        );
        for _ in 0..2 {
            expand_segment_base(&mut track, &headers, "", &control).unwrap();
            let (files, _) = download_dash_primary(
                &track,
                true,
                &root,
                &headers,
                "",
                &control,
                &mut DashCheckpoint::load(&root),
                None,
                3,
            )
            .unwrap();
            assert_eq!(
                files
                    .iter()
                    .flat_map(|path| fs::read(path).unwrap())
                    .collect::<Vec<_>>(),
                b"INITAAAABBBB"
            );
        }
        for request in server.join().unwrap() {
            assert!(request.starts_with("GET /all.mp4?token=signed HTTP/1.1"));
            let request = request.to_ascii_lowercase();
            assert!(request.contains("cookie: session=sidx\r\n"));
            assert!(request.contains("authorization: bearer sidx\r\n"));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn segment_base_obeys_explicit_control_without_a_throttle_context() {
        use std::io::{Read, Write};
        let root = std::env::temp_dir().join(format!("dash-sidx-control-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        for (command, expected) in [("pause", "paused"), ("cancel", "canceled")] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/all.mp4", listener.local_addr().unwrap());
            let index = sidx_fixture(0, 0, false, &[(4, 1000)]);
            let xml = format!(
                r#"<MPD><Period><Representation><SegmentBase indexRange="0-{}"/></Representation></Period></MPD>"#,
                index.len() - 1
            );
            let mut track = parse_mpd(&xml, &url).unwrap().representations.remove(0);
            let original = track.clone();
            fs::write(&control, command).unwrap();
            assert_eq!(
                expand_segment_base(&mut track, &HashMap::new(), "", &control).unwrap_err(),
                expected
            );
            fs::write(&control, "run").unwrap();
            let server_control = control.clone();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut header = [0u8; 1];
                let mut received = Vec::new();
                while !received.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut header).unwrap();
                    received.push(header[0]);
                }
                fs::write(server_control, command).unwrap();
                let _ = write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", index.len() - 1, index.len() + 4, index.len());
                let _ = socket.write_all(&index);
            });
            assert_eq!(
                expand_segment_base(&mut track, &HashMap::new(), "", &control).unwrap_err(),
                expected
            );
            assert_eq!(track, original);
            server.join().unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ranged_primary_downloads_exact_bytes_and_resumes_only_matching_ranges() {
        let (url, server) = serve_byte_ranges(b"INITAAAABBBBCCCC".to_vec(), 4);
        let root = std::env::temp_dir().join(format!("dash-ranges-primary-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let xml = r#"<MPD><Period><Representation id="v" mimeType="video/mp4"><SegmentList duration="1"><Initialization range="0-3"/><SegmentURL mediaRange="4-7"/><SegmentURL mediaRange="8-11"/></SegmentList></Representation></Period></MPD>"#;
        let mut track = parse_mpd(xml, &url).unwrap().representations.remove(0);
        let headers = HashMap::from([
            ("Cookie".into(), "session=test".into()),
            ("Authorization".into(), "Bearer test".into()),
        ]);
        let (files, audio) = download_dash_primary(
            &track,
            true,
            &root,
            &headers,
            "",
            &control,
            &mut DashCheckpoint::load(&root),
            None,
            3,
        )
        .unwrap();
        assert!(audio.is_none());
        assert_eq!(
            files
                .iter()
                .map(|p| fs::read(p).unwrap())
                .collect::<Vec<_>>(),
            vec![b"INIT", b"AAAA", b"BBBB"]
        );
        download_dash_primary(
            &track,
            true,
            &root,
            &headers,
            "",
            &control,
            &mut DashCheckpoint::load(&root),
            None,
            3,
        )
        .unwrap();
        track.media_ranges[1] = Some(ByteRange {
            start: 12,
            length: 4,
        });
        download_dash_primary(
            &track,
            true,
            &root,
            &headers,
            "",
            &control,
            &mut DashCheckpoint::load(&root),
            None,
            3,
        )
        .unwrap();
        assert_eq!(fs::read(&files[2]).unwrap(), b"CCCC");
        let seen = server.join().unwrap();
        for (request, range) in seen.iter().zip(["0-3", "4-7", "8-11", "12-15"]) {
            assert!(
                request.starts_with("GET /all.mp4?token=signed HTTP/1.1"),
                "{request}"
            );
            let request = request.to_ascii_lowercase();
            assert!(
                request.contains(&format!("range: bytes={range}\r\n")),
                "{request}"
            );
            assert!(request.contains("cookie: session=test\r\n"), "{request}");
            assert!(
                request.contains("authorization: bearer test\r\n"),
                "{request}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_range_replacement_cannot_reuse_an_old_checkpoint_after_restart() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/all.mp4", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for (start, body, broken) in [(0, "AAAA", false), (4, "BBBB", true), (0, "AAAA", false)]
            {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                assert!(request
                    .to_ascii_lowercase()
                    .contains(&format!("range: bytes={start}-{}\r\n", start + 3)));
                if broken {
                    // Exactly four file bytes arrive, then chunk framing fails.
                    write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\n{body}\r\nZ\r\n", start + 3).unwrap();
                } else {
                    write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/8\r\nContent-Length: 4\r\nConnection: close\r\n\r\n{body}", start + 3).unwrap();
                }
            }
        });
        let root = std::env::temp_dir().join(format!("dash-ranges-restart-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let dest = root.join("seg-0001.m4s");
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let fetch = |start| {
            resume_or_fetch(
                &url,
                Some(ByteRange { start, length: 4 }),
                &HashMap::new(),
                "",
                &dest,
                &control,
                &root,
                "video",
                "1",
                "v",
                "video/mp4",
                true,
                &mut DashCheckpoint::load(&root),
            )
        };
        fetch(0).unwrap();
        assert!(fetch(4).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"BBBB");
        let old_identity = dash_file_identity(
            "video",
            "1",
            "v",
            &url,
            Some(ByteRange {
                start: 0,
                length: 4,
            }),
            "video/mp4",
        );
        assert!(!DashCheckpoint::load(&root).can_reuse("seg-0001.m4s", &old_identity, 4));
        assert!(!super::super::preview::is_ready(&root, "seg-0001.m4s"));
        fetch(0).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"AAAA");
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ranged_audio_and_subtitles_use_the_same_checked_download_path() {
        let body = b"INITAAAABBBB";
        let (url, audio_server) = serve_byte_ranges(body.to_vec(), 3);
        let root = std::env::temp_dir().join(format!("dash-ranges-tracks-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let xml = r#"<MPD><Period><Representation id="a" mimeType="audio/mp4"><SegmentList duration="1"><Initialization range="0-3"/><SegmentURL mediaRange="4-7"/><SegmentURL mediaRange="8-11"/></SegmentList></Representation></Period></MPD>"#;
        let track = parse_mpd(xml, &url).unwrap().representations.remove(0);
        let mut output = LiveDashTrack::default();
        output
            .append(
                &track,
                false,
                "audio",
                &root,
                &root,
                &HashMap::new(),
                "",
                &control,
                &mut DashCheckpoint::load(&root),
                None,
                &LiveDashTrack::default(),
            )
            .unwrap();
        assert_eq!(
            output
                .files
                .iter()
                .map(|p| fs::read(p).unwrap())
                .collect::<Vec<_>>(),
            vec![b"INIT", b"AAAA", b"BBBB"]
        );
        audio_server.join().unwrap();
        let cue = b"WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nrange subtitle\n";
        let (url, sub_server) =
            serve_byte_ranges([b"unused".as_slice(), cue, b"trailer"].concat(), 1);
        let xml = format!(
            r#"<MPD><Period><Representation id="s" mimeType="text/vtt"><SegmentList><SegmentURL mediaRange="6-{}"/></SegmentList></Representation></Period></MPD>"#,
            6 + cue.len() - 1
        );
        let track = parse_mpd(&xml, &url).unwrap().representations.remove(0);
        let output = download_dash_subtitle_track(
            &track,
            &root.join("subs"),
            "en",
            &HashMap::new(),
            "",
            &control,
            &root,
            &mut DashCheckpoint::load(&root),
        )
        .unwrap();
        assert!(fs::read_to_string(output)
            .unwrap()
            .contains("range subtitle"));
        sub_server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires real FFmpeg; configure HLS_FFMPEG and run explicitly"]
    fn real_ffmpeg_segment_base_preserves_selected_av_and_timestamps() {
        let ffmpeg = crate::media::merge::locate_ffmpeg().expect("configure HLS_FFMPEG");
        let root = std::env::temp_dir().join(format!("dash-sidx-real-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut files = HashMap::new();
        let mut representations = String::new();
        let mut sources = Vec::new();
        let mut expected_tracks = Vec::new();
        // Two manifests, four AV indices, two init ranges and one optional subtitle index.
        let mut request_count = 9;
        for (kind, input, codec) in [
            ("video", "testsrc2=size=128x72:rate=10", "mpeg4"),
            ("audio", "sine=frequency=440:sample_rate=48000", "aac"),
        ] {
            let source = root.join(format!("{kind}.mp4"));
            let mut command = Command::new(&ffmpeg);
            command.args([
                "-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", input, "-t", "2",
            ]);
            if kind == "video" {
                command.args(["-an", "-c:v", codec, "-g", "10"]);
            } else {
                command.args(["-vn", "-c:a", codec, "-ac", "1"]);
            }
            let generated = command
                .args([
                    "-movflags",
                    "empty_moov+default_base_moof+frag_keyframe+global_sidx",
                    "-frag_duration",
                    "1000000",
                ])
                .arg(&source)
                .output()
                .unwrap();
            assert!(
                generated.status.success(),
                "{}",
                String::from_utf8_lossy(&generated.stderr)
            );
            let bytes = fs::read(&source).unwrap();
            let mut cursor = 0;
            let mut index = None;
            while cursor < bytes.len() {
                let size =
                    u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
                assert!(size >= 8 && size <= bytes.len() - cursor);
                if &bytes[cursor + 4..cursor + 8] == b"sidx" {
                    index = Some((cursor, size));
                    break;
                }
                cursor += size;
            }
            let (start, size) = index.expect("FFmpeg must generate a global SIDX");
            let references = parse_sidx(&bytes[start..start + size], start as u64).unwrap();
            assert!(references.len() >= 2);
            let mut expected = bytes[..start].to_vec();
            for (range, _) in &references {
                expected.extend_from_slice(
                    &bytes[range.start as usize..(range.start + range.length) as usize],
                );
            }
            expected_tracks.push((kind, references.len(), expected));
            request_count += references.len();
            representations.push_str(&format!(r#"<AdaptationSet mimeType="{kind}/mp4" contentType="{kind}"><Representation id="{kind}" bandwidth="1000"><BaseURL>{kind}.mp4?token=signed</BaseURL><SegmentBase indexRange="{start}-{}"><Initialization range="0-{}"/></SegmentBase></Representation></AdaptationSet>"#, start + size - 1, start - 1));
            files.insert(format!("/{kind}.mp4?token=signed"), bytes);
            sources.push(source);
        }
        // Unselected video is never fetched; disabled subtitles are not fetched either.
        representations.push_str(r#"<AdaptationSet mimeType="video/mp4"><Representation id="unused" bandwidth="1"><BaseURL>unused.mp4</BaseURL><SegmentBase indexRange="0-43"/></Representation></AdaptationSet><AdaptationSet mimeType="text/vtt"><Representation id="sub"><BaseURL>optional-sub.mp4</BaseURL><SegmentBase indexRange="0-43"/></Representation></AdaptationSet>"#);
        files.insert("/optional-sub.mp4".into(), vec![0; 44]);
        files.insert(
            "/manifest.mpd".into(),
            format!("<MPD><Period>{representations}</Period></MPD>").into_bytes(),
        );
        let (base, server) = serve_dash_files(files, request_count);
        let task = root.join("task");
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let headers = HashMap::from([
            ("Cookie".into(), "session=real".into()),
            ("Authorization".into(), "Bearer real".into()),
        ]);
        let url = format!("{base}/manifest.mpd");
        let output =
            download_dash_selected(&url, &headers, "", &task, &control, 0, 0, false, "audio")
                .unwrap();
        // A restart rechecks indices but reuses persisted media. A malformed
        // optional subtitle index must not fail an otherwise complete AV task.
        assert_eq!(
            download_dash_selected(&url, &headers, "", &task, &control, 0, 0, true, "audio")
                .unwrap(),
            output
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), request_count);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.contains("/optional-sub.mp4 "))
                .count(),
            1
        );
        for request in requests {
            assert!(!request.contains("unused"));
            let request = request.to_ascii_lowercase();
            assert!(request.contains("cookie: session=real\r\n"));
            assert!(request.contains("authorization: bearer real\r\n"));
        }
        for (kind, count, expected) in expected_tracks {
            let dir = if kind == "video" {
                task.clone()
            } else {
                task.join("audio")
            };
            let mut downloaded = fs::read(dir.join("init.mp4")).unwrap();
            for index in 0..count {
                let slot = index + usize::from(kind == "video");
                downloaded.extend(fs::read(dir.join(format!("seg-{slot:04}.m4s"))).unwrap());
            }
            assert!(
                downloaded == expected,
                "downloaded {kind} ranges must match source bytes"
            );
        }
        // Compare AAC against an independent whole-file remux: MP4 muxing
        // trims declared terminal padding that the fragmented source decoder
        // can still emit. Compare all PCM samples and timestamps after the
        // same container transition, rather than ignoring the final frame.
        let reference = root.join("reference.mp4");
        let remuxed = Command::new(&ffmpeg)
            .args(["-v", "error", "-nostdin", "-y", "-i"])
            .arg(&sources[0])
            .arg("-i")
            .arg(&sources[1])
            .args([
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c",
                "copy",
                "-movflags",
                "+faststart",
            ])
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            remuxed.status.success(),
            "{}",
            String::from_utf8_lossy(&remuxed.stderr)
        );
        let decode = |path: &Path, stream: &str| {
            let result = Command::new(&ffmpeg)
                .args(["-v", "error", "-nostdin", "-copyts", "-i"])
                .arg(path)
                .args(["-map", stream, "-f", "framemd5", "-"])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let decoded = String::from_utf8(result.stdout).unwrap();
            let timebase = decoded
                .lines()
                .find(|line| line.starts_with("#tb "))
                .unwrap()
                .to_string();
            let frames = decoded
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            (timebase, frames)
        };
        let video = decode(&sources[0], "0:v:0");
        assert_eq!(video.1.len(), 20);
        assert_eq!(decode(&output, "0:v:0"), video);
        let audio = decode(&reference, "0:a:0");
        assert!(audio.1.len() >= 90);
        assert_eq!(decode(&output, "0:a:0"), audio);
        eprintln!("SIDX AV: {} video frames, {} audio frames; exact downloaded ranges, decoded hashes and timestamps match whole-file reference", video.1.len(), audio.1.len());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires real FFmpeg; configure HLS_FFMPEG and run explicitly"]
    fn real_ffmpeg_ranged_fmp4_preserves_decoded_frames() {
        let ffmpeg = crate::media::merge::locate_ffmpeg().expect("configure HLS_FFMPEG");
        let root = std::env::temp_dir().join(format!("dash-ranges-real-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.mp4");
        let generated = Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x72:rate=10",
                "-t",
                "2",
                "-an",
                "-c:v",
                "mpeg4",
                "-g",
                "10",
                "-movflags",
                "empty_moov+default_base_moof+frag_keyframe",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let bytes = fs::read(&source).unwrap();
        let mut moofs = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            assert!(size >= 8 && size <= bytes.len() - offset);
            if &bytes[offset + 4..offset + 8] == b"moof" {
                moofs.push(offset);
            }
            offset += size;
        }
        assert_eq!(moofs.len(), 2);
        let (url, server) = serve_byte_ranges(bytes.clone(), 3);
        let xml = format!(
            r#"<MPD><Period><Representation id="v" mimeType="video/mp4"><SegmentList duration="1"><Initialization range="0-{}"/><SegmentURL mediaRange="{}-{}"/><SegmentURL mediaRange="{}-{}"/></SegmentList></Representation></Period></MPD>"#,
            moofs[0] - 1,
            moofs[0],
            moofs[1] - 1,
            moofs[1],
            bytes.len() - 1
        );
        let track = parse_mpd(&xml, &url).unwrap().representations.remove(0);
        let task = root.join("task");
        fs::create_dir_all(&task).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let mut vod = DashCheckpoint::load(&task);
        let (files, _) = download_dash_primary(
            &track,
            true,
            &task,
            &HashMap::new(),
            "",
            &control,
            &mut vod,
            None,
            3,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(
            files
                .iter()
                .flat_map(|path| fs::read(path).unwrap())
                .collect::<Vec<_>>(),
            bytes
        );
        write_dash_playlist(&task, &files).unwrap();
        let output = finish_dash(
            &task,
            None,
            Vec::new(),
            &HashMap::new(),
            "",
            &control,
            &mut vod,
        )
        .unwrap();
        let decode = |path: &Path| {
            let result = Command::new(&ffmpeg)
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(path)
                .args(["-map", "0:v:0", "-f", "framemd5", "-"])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout)
                .unwrap()
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| line.rsplit(',').next().unwrap().trim().to_string())
                .collect::<Vec<_>>()
        };
        let expected = decode(&source);
        assert_eq!(expected.len(), 20);
        assert_eq!(decode(&output), expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn separate_vod_audio_opens_preview_before_track_completion() {
        use std::sync::{Arc, Mutex};
        let root = std::env::temp_dir().join(format!(
            "dash-interleaved-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let preview_seen = Arc::new(Mutex::new(false));
        let server_preview_seen = Arc::clone(&preview_seen);
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_requests = Arc::clone(&requests);
        let server_root = root.clone();
        let origin = super::super::harness::serve_dynamic(Arc::new(move |path, _| {
            server_requests.lock().unwrap().push(path.to_string());
            if path == "/a1.m4s" {
                *server_preview_seen.lock().unwrap() = server_root.join("preview.m3u8").is_file();
                fs::write(server_root.join("control"), "pause").unwrap();
            }
            Some(if path == "/manifest.mpd" {
                br#"<MPD type="static" mediaPresentationDuration="PT8S"><Period>
<AdaptationSet mimeType="video/mp4" contentType="video"><Representation id="v" bandwidth="800000"><SegmentList duration="4" timescale="1"><Initialization sourceURL="vi.mp4"/><SegmentURL media="v0.m4s"/><SegmentURL media="v1.m4s"/></SegmentList></Representation></AdaptationSet>
<AdaptationSet mimeType="audio/mp4" contentType="audio"><Representation id="a" bandwidth="64000"><SegmentList duration="1" timescale="1"><Initialization sourceURL="ai.mp4"/><SegmentURL media="a0.m4s"/><SegmentURL media="a1.m4s"/><SegmentURL media="a2.m4s"/></SegmentList></Representation></AdaptationSet>
</Period></MPD>"#.to_vec()
            } else {
                b"fragment bytes".to_vec()
            })
        }));
        let error = download_dash_selected(
            &format!("{}/manifest.mpd", origin.base),
            &HashMap::new(),
            "",
            &root,
            &control,
            0,
            0,
            false,
            "",
        )
        .unwrap_err();
        assert_eq!(error, "paused");
        assert!(*preview_seen.lock().unwrap());
        assert_eq!(
            fs::read(root.join("seg-0001.m4s")).unwrap(),
            b"fragment bytes"
        );
        assert_eq!(
            fs::read(root.join("audio/seg-0000.m4s")).unwrap(),
            b"fragment bytes"
        );
        assert!(!root.join("audio/seg-0002.m4s").exists());
        let requests = requests.lock().unwrap();
        let video = requests.iter().position(|path| path == "/v0.m4s").unwrap();
        let audio = requests.iter().position(|path| path == "/a1.m4s").unwrap();
        assert!(video < audio);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progressive_dash_playlist_keeps_real_durations_and_only_ends_when_complete() {
        let root = std::env::temp_dir().join(format!(
            "dash-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let files = vec![
            root.join("init.mp4"),
            root.join("seg-0001.m4s"),
            root.join("seg-0002.m4s"),
        ];
        let durations = [1.25, 5.5, 0.75];
        let path = root.join("preview-video.m3u8");
        assert!(!write_dash_preview_track(&path, &files, &[], false).unwrap());
        assert!(!path.exists());
        assert!(write_dash_preview_track(&path, &files[..2], &durations, false).unwrap());
        let first = fs::read_to_string(&path).unwrap();
        assert!(first.contains("#EXTINF:1.250000,") && first.contains("#EXT-X-TARGETDURATION:6"));
        assert!(!first.contains("#EXT-X-ENDLIST"));
        assert!(write_dash_preview_track(&path, &files, &durations, true).unwrap());
        let last = fs::read_to_string(&path).unwrap();
        assert!(last.contains("#EXTINF:5.500000,") && last.contains("#EXT-X-TARGETDURATION:6"));
        assert!(last.contains("#EXT-X-ENDLIST"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn self_closing_video_representations_do_not_consume_following_audio_template() {
        let xml = r#"<MPD type="static" mediaPresentationDuration="PT8S"><Period>
<AdaptationSet mimeType="video/mp4" contentType="video">
<SegmentTemplate duration="120" timescale="30" media="$RepresentationID$/$Number$.m4v" initialization="$RepresentationID$/0.m4v"/>
<Representation id="low" height="180" bandwidth="250000"/>
<Representation id="high" height="720" bandwidth="4000000"/>
</AdaptationSet><AdaptationSet mimeType="audio/mp4" contentType="audio">
<SegmentTemplate duration="192512" timescale="48000" media="$RepresentationID$/$Number$.m4a" initialization="$RepresentationID$/0.m4a"/>
<Representation id="audio" bandwidth="64000"><AudioChannelConfiguration value="2"/></Representation>
</AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(parsed.representations.len(), 3);
        let low = &parsed.representations[0];
        assert_eq!(low.mime, "video/mp4");
        assert_eq!(low.init.as_deref(), Some("https://cdn.test/low/0.m4v"));
        assert_eq!(
            low.media,
            ["https://cdn.test/low/1.m4v", "https://cdn.test/low/2.m4v"]
        );
        let audio = &parsed.representations[2];
        assert_eq!(audio.mime, "audio/mp4");
        assert_eq!(audio.init.as_deref(), Some("https://cdn.test/audio/0.m4a"));
    }

    #[test]
    fn fixed_duration_templates_download_every_video_and_audio_segment() {
        let xml = r#"<?xml version="1.0"?><MPD type="static" mediaPresentationDuration="PT4.0S"><Period start="PT0S">
<AdaptationSet contentType="video"><Representation id="low" mimeType="video/mp4" height="180" bandwidth="300000"><SegmentTemplate timescale="1000000" duration="1000000" initialization="init-$RepresentationID$.m4s" media="chunk-$RepresentationID$-$Number%05d$.m4s" startNumber="1"></SegmentTemplate></Representation></AdaptationSet>
<AdaptationSet contentType="audio"><SegmentTemplate timescale="48000" duration="48000" initialization="init-$RepresentationID$.m4s" media="chunk-$RepresentationID$-$Number%05d$.m4s" startNumber="3"/><Representation id="audio" mimeType="audio/mp4" bandwidth="69000"/></AdaptationSet>
</Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        for (representation, first) in parsed.representations.iter().zip([1, 3]) {
            assert_eq!(representation.media.len(), 4);
            assert_eq!(
                representation.media,
                (first..first + 4)
                    .map(|number| format!(
                        "https://cdn.test/chunk-{}-{number:05}.m4s",
                        representation.id
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn fixed_duration_templates_respect_period_boundaries_and_partial_last_segment() {
        for period in ["start=\"PT2S\" duration=\"PT2.5S\"", "start=\"PT2S\""] {
            let xml = format!(
                r#"<MPD type="static" mediaPresentationDuration="PT4.5S"><Period {period}><AdaptationSet><Representation id="v" mimeType="video/mp4"><SegmentTemplate duration="10" timescale="10" presentationTimeOffset="20" media="$Time$.m4s"/></Representation></AdaptationSet></Period></MPD>"#
            );
            let parsed = parse_mpd(&xml, "https://cdn.test/manifest.mpd").unwrap();
            assert_eq!(
                parsed.representations[0].media_durations,
                vec![1.0, 1.0, 0.5]
            );
            assert_eq!(
                parsed.representations[0].media,
                [
                    "https://cdn.test/20.m4s",
                    "https://cdn.test/30.m4s",
                    "https://cdn.test/40.m4s"
                ]
            );
        }
        let xml = r#"<MPD type="static" mediaPresentationDuration="PT20S"><Period start="PT2S"><AdaptationSet><Representation id="v"><SegmentTemplate duration="1" media="$Number$.m4s"/></Representation></AdaptationSet></Period><Period start="PT4.5S"/></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(parsed.period_count, 2);
        assert_eq!(
            parsed.representations[0].media_durations,
            vec![1.0, 1.0, 0.5]
        );
        // The empty second Period cannot continue the track: fail with a clear error.
        assert!(period_parts(&parsed, &parsed.representations[0])
            .unwrap_err()
            .contains("第 2 个 Period"));
    }

    #[test]
    fn selected_height_distinguishes_equal_bandwidth_video_tracks() {
        let xml = r#"<MPD><Period><AdaptationSet mimeType="video/mp4"><Representation id="high" height="720" bandwidth="500000"/><Representation id="low" height="360" bandwidth="500000"/></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(
            select_video(&parsed.representations, 500000, 360)
                .unwrap()
                .id,
            "low"
        );
        assert_eq!(
            select_video(&parsed.representations, 0, 720).unwrap().id,
            "high"
        );
    }

    #[test]
    fn parses_static_representation_and_segment_list() {
        let xml = r#"<MPD type="static"><Period><AdaptationSet><Representation id="v" bandwidth="2000000" mimeType="video/mp4"><BaseURL>https://cdn.test/v/</BaseURL><SegmentList><SegmentURL media="init.mp4"/><SegmentURL media="1.m4s"/></SegmentList></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(parsed.period_count, 1);
        assert!(!parsed.dynamic);
        assert_eq!(
            parsed.representations[0].media[0],
            "https://cdn.test/v/init.mp4"
        );
    }

    #[test]
    fn representation_level_segment_template_segments_are_all_expanded() {
        // The old `block.split("<SegmentURL")` loop also inspected chunk 0 --
        // the text before the first `<SegmentURL` -- which for a
        // Representation-level `<SegmentTemplate>` still carries that
        // template's own `media` attribute.  The raw `$...$` template was
        // pushed as a segment URL, so every fetch of it failed and aborted the
        // download.
        let xml = r#"<MPD type="static"><Period><AdaptationSet mimeType="video/mp4"><Representation id="v1" bandwidth="800000"><BaseURL>https://cdn.test/v1/</BaseURL><SegmentTemplate timescale="90000" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Number%05d$.m4s" startNumber="1"><SegmentTimeline><S t="0" d="180000"/><S t="180000" d="180000"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(
            parsed.representations[0].media,
            vec![
                "https://cdn.test/v1/v1/00001.m4s",
                "https://cdn.test/v1/v1/00002.m4s",
            ],
            "a raw $...$ template must never reach the segment list"
        );
    }

    #[test]
    fn a_stopped_control_sentinel_surfaces_as_an_error_not_a_completion() {
        // The static branch used to `break` out of its segment loop and then
        // merge only the segments fetched so far into a "successful"
        // merged.mp4, so the sentinel itself has to name the failure the way
        // the HLS path does.
        let root = std::env::temp_dir().join(format!("dash-control-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        std::fs::write(&control, "pause").unwrap();
        assert_eq!(control_stop_error(&control), "paused");
        std::fs::write(&control, "cancel").unwrap();
        assert_eq!(control_stop_error(&control), "canceled");
        // A missing control file reads as "run", which must never be a stop.
        assert_eq!(control_stop_error(&root.join("absent")), "canceled");
        assert_eq!(read_control(&control), "cancel");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn expands_adaptation_set_segment_timeline() {
        let xml = r#"<MPD type="static"><Period>
<AdaptationSet mimeType="video/mp4">
<SegmentTemplate timescale="90000" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Number%05d$.m4s" startNumber="1">
<SegmentTimeline><S t="0" d="180000" r="2"/><S d="90000"/></SegmentTimeline>
</SegmentTemplate>
<Representation id="v1" bandwidth="800000"/>
</AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(
            parsed.representations[0].init.as_deref(),
            Some("https://cdn.test/v1/init.mp4")
        );
        assert_eq!(parsed.representations[0].mime, "video/mp4");
        assert_eq!(
            parsed.representations[0].media,
            vec![
                "https://cdn.test/v1/00001.m4s",
                "https://cdn.test/v1/00002.m4s",
                "https://cdn.test/v1/00003.m4s",
                "https://cdn.test/v1/00004.m4s",
            ]
        );
    }

    #[test]
    fn multi_period_stitches_same_codec_and_rejects_codec_changes() {
        for codec in ["avc1.64001f", "hvc1"] {
            let xml = format!(
                r#"<MPD type="static"><Period><AdaptationSet><Representation id="v" bandwidth="1" codecs="avc1.4d401e" mimeType="video/mp4"><SegmentURL media="a.m4s"/></Representation></AdaptationSet></Period><Period><AdaptationSet><Representation id="v2" bandwidth="1" codecs="{codec}" mimeType="video/mp4"><SegmentURL media="b.m4s"/></Representation></AdaptationSet></Period></MPD>"#
            );
            let parsed = parse_mpd(&xml, "https://cdn.test/manifest.mpd").unwrap();
            let stitched =
                period_parts(&parsed, &parsed.representations[0]).and_then(concat_period_parts);
            if codec == "hvc1" {
                assert!(stitched.unwrap_err().contains("编码"));
                let mut options = parsed.clone();
                let mut compatible = options.representations[1].clone();
                compatible.codecs = "avc1.64001f".into();
                compatible.bandwidth = 2;
                compatible.media = vec!["https://cdn.test/compatible.m4s".into()];
                options.representations.push(compatible);
                let selected = period_parts(&options, &options.representations[0]).unwrap();
                assert_eq!(
                    concat_period_parts(selected).unwrap().media.last().unwrap(),
                    "https://cdn.test/compatible.m4s"
                );
            } else {
                assert_eq!(
                    stitched.unwrap().media,
                    ["https://cdn.test/a.m4s", "https://cdn.test/b.m4s"]
                );
            }
        }
    }

    #[test]
    fn multi_period_template_and_list_match_tracks_and_inline_period_inits() {
        // Period 1: SegmentTemplate (number, fixed duration) under its own BaseURL.
        // Period 2: no @start (follows Period 1), SegmentList with a different init.
        // Period 3: SegmentTemplate + SegmentTimeline reusing Period 1's init URL.
        let xml = r#"<MPD type="static" mediaPresentationDuration="PT9S"><BaseURL>https://cdn.test/root/</BaseURL>
<Period id="p1" start="PT0S" duration="PT4S"><BaseURL>main/</BaseURL>
 <AdaptationSet contentType="video" mimeType="video/mp4" codecs="avc1.64001f">
  <SegmentTemplate timescale="1" duration="2" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Number$.m4s"/>
  <Representation id="hi" bandwidth="3000000" height="720"/><Representation id="lo" bandwidth="800000" height="360"/>
 </AdaptationSet>
 <AdaptationSet contentType="audio" mimeType="audio/mp4" lang="en" codecs="mp4a.40.2"><Representation id="en" bandwidth="128000"><SegmentTemplate duration="2" initialization="a-en/init.mp4" media="a-en/$Number$.m4s"/></Representation></AdaptationSet>
 <AdaptationSet contentType="audio" mimeType="audio/mp4" lang="ja" codecs="mp4a.40.2"><Representation id="ja" bandwidth="128000"><SegmentTemplate duration="2" initialization="a-ja/init.mp4" media="a-ja/$Number$.m4s"/></Representation></AdaptationSet>
</Period>
<Period id="ad"><BaseURL>https://ads.test/break/</BaseURL>
 <AdaptationSet mimeType="video/mp4" codecs="avc1.4d401e"><Representation id="ad-hi" bandwidth="2500000" height="720"><SegmentList duration="1"><Initialization sourceURL="ad-init.mp4"/><SegmentURL media="ad1.m4s"/><SegmentURL media="ad2.m4s"/></SegmentList></Representation><Representation id="ad-lo" bandwidth="700000" height="360"><SegmentList duration="1"><Initialization sourceURL="ad-lo-init.mp4"/><SegmentURL media="lo1.m4s"/><SegmentURL media="lo2.m4s"/></SegmentList></Representation></AdaptationSet>
 <AdaptationSet mimeType="audio/mp4" lang="ja"><Representation id="ad-ja" bandwidth="96000"><SegmentList duration="2"><Initialization sourceURL="ja-init.mp4"/><SegmentURL media="ja1.m4s"/></SegmentList></Representation></AdaptationSet>
 <AdaptationSet mimeType="audio/mp4" lang="en"><Representation id="ad-en" bandwidth="96000"><SegmentList duration="2"><Initialization sourceURL="en-init.mp4"/><SegmentURL media="en1.m4s"/></SegmentList></Representation></AdaptationSet>
</Period>
<Period id="p2" start="PT6S"><BaseURL>main/</BaseURL>
 <AdaptationSet contentType="video" mimeType="video/mp4"><Representation id="lo" bandwidth="800000" height="360"><SegmentTemplate timescale="10" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/t$Time$.m4s" presentationTimeOffset="60"><SegmentTimeline><S t="60" d="20" r="-1"/></SegmentTimeline></SegmentTemplate></Representation><Representation id="hi" bandwidth="3000000" height="720"><SegmentTemplate timescale="10" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/t$Time$.m4s" presentationTimeOffset="60"><SegmentTimeline><S t="60" d="20" r="-1"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet>
 <AdaptationSet contentType="audio" mimeType="audio/mp4" lang="en"><Representation id="en" bandwidth="128000"><SegmentTemplate duration="3" initialization="a-en/init.mp4" media="a-en/$Number$.m4s"/></Representation></AdaptationSet>
</Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(parsed.period_count, 3);
        // Track pickers only offer the first Period's choices.
        assert_eq!(representation_choices(&parsed).len(), 2);
        assert_eq!(audio_choices(&parsed).len(), 2);
        let first: Vec<_> = parsed
            .representations
            .iter()
            .filter(|item| item.period == 0)
            .cloned()
            .collect();
        let video = select_video(&first, 3_000_000, 720).unwrap();
        let video = concat_period_parts(period_parts(&parsed, video).unwrap()).unwrap();
        assert_eq!(
            video.media,
            [
                "https://cdn.test/root/main/hi/1.m4s",
                "https://cdn.test/root/main/hi/2.m4s",
                "https://ads.test/break/ad-init.mp4",
                "https://ads.test/break/ad1.m4s",
                "https://ads.test/break/ad2.m4s",
                "https://cdn.test/root/main/hi/init.mp4",
                "https://cdn.test/root/main/hi/t60.m4s",
                "https://cdn.test/root/main/hi/t80.m4s",
            ]
        );
        assert_eq!(
            video.init.as_deref(),
            Some("https://cdn.test/root/main/hi/init.mp4")
        );
        assert_eq!(video.init_markers, vec![2, 5]);
        assert_eq!(
            video.media_durations,
            vec![2.0, 2.0, 0.0, 1.0, 1.0, 0.0, 2.0, 2.0]
        );
        assert_eq!(video.media_ranges.len(), video.media.len());
        // Audio follows the chosen language rather than document order.
        let audio = select_audio(&first, "ja").unwrap();
        let ja_parts = period_parts(&parsed, audio).unwrap();
        assert_eq!(ja_parts[1].id, "ad-ja");
        assert_eq!(
            ja_parts[2].id, "en",
            "only remaining audio continues the track"
        );
        let audio = select_audio(&first, "en").unwrap();
        let audio = concat_period_parts(period_parts(&parsed, audio).unwrap()).unwrap();
        assert_eq!(
            audio.media,
            [
                "https://cdn.test/root/main/a-en/1.m4s",
                "https://cdn.test/root/main/a-en/2.m4s",
                "https://ads.test/break/en-init.mp4",
                "https://ads.test/break/en1.m4s",
                "https://cdn.test/root/main/a-en/init.mp4",
                "https://cdn.test/root/main/a-en/1.m4s",
            ]
        );
    }

    #[test]
    fn period_timing_inherits_start_and_presentation_duration() {
        let xml = r#"<MPD type="static" mediaPresentationDuration="PT7S"><Period duration="PT2S"><Representation id="v" mimeType="video/mp4"><SegmentTemplate duration="1" media="a$Number$.m4s"/></Representation></Period><Period><Representation id="v" mimeType="video/mp4"><SegmentTemplate duration="1" media="b$Number$.m4s"/></Representation></Period><Period start="PT4S"><Representation id="v" mimeType="video/mp4"><SegmentTemplate duration="2" media="c$Number$.m4s"/></Representation></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/m.mpd").unwrap();
        let counts: Vec<_> = parsed
            .representations
            .iter()
            .map(|item| (item.period, item.media.len()))
            .collect();
        assert_eq!(counts, [(0, 2), (1, 2), (2, 2)]);
        let stitched =
            concat_period_parts(period_parts(&parsed, &parsed.representations[0]).unwrap())
                .unwrap();
        assert!(stitched.init_markers.is_empty());
        assert_eq!(stitched.period_markers, vec![2, 4]);
        assert_eq!(stitched.media_durations, vec![1.0, 1.0, 1.0, 1.0, 2.0, 1.0]);
    }

    #[test]
    fn formats_padded_number_templates() {
        assert_eq!(
            apply_template("seg_$Number%05d$.m4s", "v", 12, 0, 1).unwrap(),
            "seg_00012.m4s"
        );
        assert_eq!(
            apply_template("$RepresentationID$_$Number$.m4s", "v1", 3, 0, 1).unwrap(),
            "v1_3.m4s"
        );
    }

    #[test]
    fn dash_template_tokens_formats_and_escapes() {
        for (template, expected) in [
            ("$Time%010d$", "0000000042"),
            ("$Bandwidth%08d$", "00000123"),
            ("$Number%05d$", "00007"),
            ("$$Number$$", "$Number$"),
            (
                "$RepresentationID$/$Number$/$Time$/$Bandwidth$",
                "v/7/42/123",
            ),
            ("$Number%01d$", "7"),
            ("$Number%00d$", "7"),
            ("$Number%0d$", "7"),
            ("plain.m4s", "plain.m4s"),
            ("", ""),
        ] {
            assert_eq!(apply_template(template, "v", 7, 42, 123).unwrap(), expected);
        }
        assert_eq!(
            apply_template("$RepresentationID$", "$$Number$", 7, 0, 0).unwrap(),
            "$$Number$"
        );
        assert_eq!(
            apply_template("$Time%01d$", "v", 0, u64::MAX, 0).unwrap(),
            u64::MAX.to_string()
        );
    }

    #[test]
    fn dash_template_rejects_invalid_and_unbounded_expansion() {
        for template in [
            "$Number%01000000000d$",
            "$Number%099999999999999999999999999999999999d$",
            "$Number%033d$",
            "$Unknown$",
            "$SubNumber$",
            "$Number",
            "$",
            "$RepresentationID%05d$",
            "$Time%5d$",
            "$Bandwidth%05x$",
            "$Number%01dd$",
            "$Number%01%d$",
        ] {
            assert!(
                apply_template(template, "v", 1, 2, 3).is_err(),
                "{template}"
            );
        }
        let limit = "a".repeat(MAX_DASH_TEMPLATE_URL_BYTES);
        assert_eq!(apply_template(&limit, "v", 0, 0, 0).unwrap(), limit);
        assert!(apply_template(&format!("{limit}a"), "v", 0, 0, 0).is_err());
        assert!(apply_template(&format!("{limit}$Number$"), "v", 0, 0, 0).is_err());
        assert!(apply_template("$RepresentationID$", &format!("{limit}a"), 0, 0, 0).is_err());
        assert_eq!(
            apply_template("$Time%032d$", "v", 0, 1, 0).unwrap().len(),
            32
        );
    }

    #[test]
    fn dash_template_errors_propagate_from_manifest_parsing() {
        for template in [
            "<SegmentTemplate initialization=\"$Unknown$\" media=\"$Number$\" duration=\"1\"/>",
            "<SegmentTemplate media=\"$Number%01000000000d$\" duration=\"1\"/>",
            "<SegmentTemplate media=\"$Unknown$\"><SegmentTimeline><S d=\"1\"/></SegmentTimeline></SegmentTemplate>",
        ] {
            let xml = format!("<MPD mediaPresentationDuration=\"PT1S\"><Period><AdaptationSet>{template}<Representation id=\"v\"/></AdaptationSet></Period></MPD>");
            assert!(parse_mpd(&xml, "https://cdn.test/video.mpd").is_err());
        }
        let xml = "<MPD type=\"dynamic\"><Period><AdaptationSet><SegmentTemplate media=\"$Unknown$\" duration=\"1\"/><Representation id=\"v\"/></AdaptationSet></Period></MPD>";
        assert!(parse_mpd(xml, "https://cdn.test/live.mpd").is_err());
    }

    #[test]
    fn dynamic_manifest_and_update_period() {
        let xml = r#"<MPD type="dynamic" minimumUpdatePeriod="PT2.5S"><Period><AdaptationSet><Representation id="v" bandwidth="1" mimeType="video/mp4"><SegmentURL media="1.m4s"/></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/live.mpd").unwrap();
        assert!(parsed.dynamic);
        assert_eq!(minimum_update_period(xml), Some(2.5));
    }

    #[test]
    fn dash_live_duration_window_clock_offset_and_refresh() {
        let xml = r#"<MPD type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z" timeShiftBufferDepth="PT6S"><Period start="PT10S"><AdaptationSet><SegmentTemplate duration="4" timescale="2" presentationTimeOffset="20" startNumber="7" media="$Number$-$Time$.m4s"/><Representation id="v"/></AdaptationSet></Period></MPD>"#;
        let first = parse_mpd_at(xml, "https://cdn.test/live.mpd", 20.0).unwrap();
        let track = &first.representations[0];
        assert_eq!(
            track.media,
            [
                "https://cdn.test/9-28.m4s",
                "https://cdn.test/10-32.m4s",
                "https://cdn.test/11-36.m4s"
            ]
        );
        assert_eq!(track.media_durations, [2.0; 3]);
        let next = parse_mpd_at(xml, "https://cdn.test/live.mpd", 22.0).unwrap();
        let mut seen: HashSet<_> = track.media.iter().cloned().collect();
        let added: Vec<_> = next.representations[0]
            .media
            .iter()
            .filter(|url| seen.insert((*url).clone()))
            .collect();
        assert_eq!(added, [&"https://cdn.test/12-40.m4s".to_string()]);
        let without_offset = xml.replace(
            "presentationTimeOffset=\"20\"",
            "presentationTimeOffset=\"0\"",
        );
        let unshifted = parse_mpd_at(&without_offset, "https://cdn.test/live.mpd", 20.0).unwrap();
        assert_eq!(
            unshifted.representations[0].media_durations,
            track.media_durations
        );
        assert!(unshifted.representations[0].media[0].ends_with("/9-8.m4s"));
        assert!(parse_mpd_at(xml, "https://cdn.test/live.mpd", 9.0)
            .unwrap()
            .representations[0]
            .media
            .is_empty());
        let files = ["9-28", "10-32", "11-36", "12-40"]
            .into_iter()
            .map(|name| (format!("/{name}.m4s"), b"data".to_vec()))
            .collect();
        let (base, server) = serve_dash_files(files, 4);
        let root = std::env::temp_dir().join(format!("dash-live-window-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let mut live = LiveDashTrack::default();
        let other = LiveDashTrack::default();
        let mut checkpoint = DashCheckpoint::load(&root);
        for now in [20.0, 22.0] {
            let parsed = parse_mpd_at(xml, &format!("{base}/live.mpd"), now).unwrap();
            live.append(
                &parsed.representations[0],
                false,
                "video",
                &root,
                &root,
                &HashMap::new(),
                "",
                &control,
                &mut checkpoint,
                None,
                &other,
            )
            .unwrap();
        }
        assert_eq!(live.files.len(), 4);
        assert_eq!(server.join().unwrap().len(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dash_live_duration_defaults_limits_and_invalid_inputs() {
        let xml = r#"<MPD type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z"><Period><SegmentTemplate duration="2" media="$Number$.m4s"/><Representation id="v"/></Period></MPD>"#;
        let parsed = parse_mpd_at(xml, "https://cdn.test/live.mpd", 100.0).unwrap();
        assert_eq!(parsed.representations[0].media.len(), 30);
        assert!(parsed.representations[0].media[0].ends_with("/21.m4s"));
        let tiny = xml.replace("duration=\"2\"", "duration=\"1\" timescale=\"1000\"");
        assert_eq!(
            parse_mpd_at(&tiny, "https://cdn.test/live.mpd", 100.0)
                .unwrap()
                .representations[0]
                .media
                .len(),
            1200
        );
        let fraction = xml.replace("00:00:00Z", "00:00:00.500+00:00");
        assert!(parse_mpd_at(&fraction, "https://cdn.test/live.mpd", 2.4)
            .unwrap()
            .representations[0]
            .media
            .is_empty());
        for invalid in [
            xml.replace(" availabilityStartTime=\"1970-01-01T00:00:00Z\"", ""),
            xml.replace("1970-01-01T00:00:00Z", "bad-date"),
            xml.replace("duration=\"2\"", "duration=\"0\""),
            xml.replace("duration=\"2\"", "duration=\"2\" timescale=\"0\""),
            xml.replace("<Period>", "<Period/><Period>"),
        ] {
            assert!(parse_mpd_at(&invalid, "https://cdn.test/live.mpd", 100.0).is_err());
        }
    }

    #[test]
    fn dash_live_duration_budget_is_shared_across_representations() {
        let xml = r#"<MPD type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z"><Period><SegmentTemplate duration="1" timescale="1000" media="$RepresentationID$-$Number$.m4s"/><Representation id="v"/><Representation id="a"/></Period></MPD>"#;
        assert!(parse_mpd_at(xml, "https://cdn.test/live.mpd", 100.0)
            .unwrap_err()
            .contains("live segment budget"));
        assert!(parse_mpd_at(xml, "https://cdn.test/live.mpd", 0.5).is_ok());
        assert!(parse_mpd_at(xml, "https://cdn.test/live.mpd", 0.0).is_ok());
    }

    #[test]
    fn dash_live_static_transition_does_not_append_older_segments() {
        let files = (1..=5).map(|n| (format!("/{n}.m4s"), vec![n])).collect();
        let (base, server) = serve_dash_files(files, 8);
        let root =
            std::env::temp_dir().join(format!("dash-static-transition-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let mut live = LiveDashTrack::default();
        let other = LiveDashTrack::default();
        let mut checkpoint = DashCheckpoint::load(&root);
        let dynamic = r#"<MPD type="dynamic"><Period><Representation id="v"><SegmentTemplate media="$Number$.m4s" startNumber="3"><SegmentTimeline><S d="1" r="1"/></SegmentTimeline></SegmentTemplate></Representation></Period></MPD>"#;
        let ended = r#"<MPD type="static" mediaPresentationDuration="PT5S"><Period><Representation id="v"><SegmentTemplate duration="1" media="$Number$.m4s"/></Representation></Period></MPD>"#;
        for (xml, next_period) in [
            (dynamic, false),
            (ended, false),
            (ended, false),
            (ended, true),
        ] {
            let parsed = parse_mpd(xml, &format!("{base}/live.mpd")).unwrap();
            live.append(
                &parsed.representations[0],
                next_period,
                "video",
                &root,
                &root,
                &HashMap::new(),
                "",
                &control,
                &mut checkpoint,
                None,
                &other,
            )
            .unwrap();
        }
        let bytes: Vec<_> = live
            .files
            .iter()
            .map(|path| fs::read(path).unwrap()[0])
            .collect();
        assert_eq!(bytes, [3, 4, 5, 1, 2, 3, 4, 5]);
        assert_eq!(server.join().unwrap().len(), 8);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dash_segment_list_inherits_attributes_and_nearest_children() {
        let xml = r#"<MPD><Period><SegmentList timescale="10" duration="20" presentationTimeOffset="5" startNumber="9"><Initialization sourceURL="parent.mp4"/><SegmentURL media="parent.m4s"/></SegmentList><AdaptationSet><SegmentList duration="30"/><Representation id="v"><SegmentList startNumber="10"/></Representation><Representation id="a"><SegmentList duration="40"><Initialization sourceURL="child.mp4"/><SegmentURL media="child1.m4s"/><SegmentURL media="child2.m4s"/></SegmentList></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/live.mpd").unwrap();
        let video = &parsed.representations[0];
        assert_eq!(video.init.as_deref(), Some("https://cdn.test/parent.mp4"));
        assert_eq!(video.media, ["https://cdn.test/parent.m4s"]);
        assert_eq!(video.media_durations, [3.0]);
        let audio = &parsed.representations[1];
        assert_eq!(audio.init.as_deref(), Some("https://cdn.test/child.mp4"));
        assert_eq!(
            audio.media,
            ["https://cdn.test/child1.m4s", "https://cdn.test/child2.m4s"]
        );
        assert_eq!(audio.media_durations, [4.0, 4.0]);
        assert!(parse_mpd(
            &xml.replace("timescale=\"10\"", "timescale=\"0\""),
            "https://cdn.test/live.mpd"
        )
        .is_err());
        let empty = r#"<MPD><Period><SegmentList duration="2"/><Representation><SegmentList/></Representation></Period></MPD>"#;
        assert!(parse_mpd(empty, "https://cdn.test/live.mpd")
            .unwrap()
            .representations[0]
            .media
            .is_empty());
    }

    fn dash_status_server(statuses: Vec<u16>) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/live.mpd", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for status in statuses {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                write!(
                    socket,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                )
                .unwrap();
            }
        });
        (url, server)
    }

    #[test]
    fn dash_live_refresh_retries_or_fails_instead_of_succeeding() {
        let root = std::env::temp_dir().join(format!("dash-refresh-status-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        for statuses in [
            vec![408, 429, 599, 200],
            vec![503; 4],
            vec![401],
            vec![403],
            vec![404],
            vec![400],
        ] {
            let success = statuses.last() == Some(&200);
            let last = *statuses.last().unwrap();
            let (url, server) = dash_status_server(statuses);
            let result = refresh_live_manifest(&url, &HashMap::new(), "", &control);
            if success {
                assert_eq!(result.unwrap(), b"ok");
            } else {
                assert!(result.unwrap_err().contains(&format!("HTTP {last}")));
            }
            server.join().unwrap();
        }
        fs::write(&control, "pause").unwrap();
        assert_eq!(
            refresh_live_manifest("http://127.0.0.1:1", &HashMap::new(), "", &control).unwrap_err(),
            "paused"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dash_live_recording_does_not_publish_success_after_refresh_failure() {
        let (url, server) = dash_status_server(vec![200, 403]);
        let xml = format!(
            r#"<MPD type="dynamic" minimumUpdatePeriod="PT0.4S"><Period><Representation id="v" mimeType="video/mp4"><SegmentList duration="1"><SegmentURL media="{url}"/></SegmentList></Representation></Period></MPD>"#
        );
        let manifest = parse_mpd(&xml, &url).unwrap();
        let root =
            std::env::temp_dir().join(format!("dash-live-refresh-failure-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let result = download_live_dash(
            manifest,
            xml,
            &url,
            &HashMap::new(),
            "",
            &root,
            &control,
            0,
            0,
            false,
            "",
            &mut DashCheckpoint::load(&root),
        );
        assert!(result.unwrap_err().contains("HTTP 403"));
        assert_eq!(fs::read(root.join("seg-0000.m4s")).unwrap(), b"ok");
        assert!(!root.join("merged.mp4").exists());
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dash_live_recording_skips_expired_but_waits_for_future_periods() {
        let (url, server) = dash_status_server(vec![200, 403]);
        let xml = format!(
            r#"<MPD type="dynamic" minimumUpdatePeriod="PT0.4S" availabilityStartTime="1970-01-01T00:00:00Z" timeShiftBufferDepth="PT1S"><SegmentTemplate duration="1" media="{url}?n=$Number$"/><Period start="PT0S" duration="PT2S"><Representation id="v" mimeType="video/mp4"/></Period><Period start="PT2S"><Representation id="v" mimeType="video/mp4"/></Period></MPD>"#
        );
        let manifest = parse_mpd_at(&xml, &url, 4.0).unwrap();
        assert!(manifest.expired_periods.contains(&0));
        assert!(manifest.representations[0].media.is_empty());
        assert_eq!(manifest.representations[1].media.len(), 1);
        let future = parse_mpd_at(&xml, &url, 1.0).unwrap();
        assert!(!future.expired_periods.contains(&1));
        assert!(future.representations[1].media.is_empty());
        let root = std::env::temp_dir().join(format!("dash-expired-period-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let result = download_live_dash(
            manifest,
            xml,
            &url,
            &HashMap::new(),
            "",
            &root,
            &control,
            0,
            0,
            false,
            "",
            &mut DashCheckpoint::load(&root),
        );
        assert!(result.unwrap_err().contains("HTTP 403"));
        assert_eq!(fs::read(root.join("seg-0000.m4s")).unwrap(), b"ok");
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dash_identity_skips_matching_and_rejects_mismatch() {
        let dir = std::env::temp_dir().join(format!("dash-vod-id-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("seg-0001.m4s");
        std::fs::write(&dest, b"ABC").unwrap();
        let identity = dash_file_identity(
            "video",
            "1",
            "v",
            "https://cdn.test/1.m4s?token=old",
            None,
            "video/mp4",
        );
        let legacy = crate::crypto_lite::sha256_hex(
            serde_json::json!({
                "kind": "video", "mime": "video/mp4", "representation": "v",
                "slot": "1", "url": "/1.m4s"
            })
            .to_string()
            .as_bytes(),
        );
        std::fs::write(
            dir.join("dash_vod_segments.json"),
            serde_json::json!({
                "version": 1,
                "files": {
                    "seg-0001.m4s": {"identity": identity, "size": 3},
                    "legacy.m4s": {"identity": legacy, "size": 3}
                }
            })
            .to_string(),
        )
        .unwrap();
        let vod = DashCheckpoint::load(&dir);
        assert!(vod.can_reuse("seg-0001.m4s", &identity, 3));
        assert!(!vod.can_reuse("legacy.m4s", &identity, 3));
        for url in [
            "https://cdn.test/1.m4s?token=new",
            "HTTPS://CDN.TEST:443/1.m4s?token=new",
        ] {
            let refreshed = dash_file_identity("video", "1", "v", url, None, "video/mp4");
            assert!(vod.can_reuse("seg-0001.m4s", &refreshed, 3), "{url}");
        }
        for url in [
            "https://other.test/1.m4s?token=new",
            "http://cdn.test/1.m4s?token=new",
            "https://cdn.test:8443/1.m4s?token=new",
        ] {
            let refreshed = dash_file_identity("video", "1", "v", url, None, "video/mp4");
            assert!(!vod.can_reuse("seg-0001.m4s", &refreshed, 3), "{url}");
            assert!(!vod.can_reuse("legacy.m4s", &refreshed, 3), "{url}");
        }
        let other = dash_file_identity(
            "video",
            "1",
            "v",
            "https://cdn.test/2.m4s",
            None,
            "video/mp4",
        );
        assert!(!vod.can_reuse("seg-0001.m4s", &other, 3));
        assert_eq!(
            dash_file_identity(
                "video",
                "1",
                "v",
                "https://cdn.test/1.m4s?token=old",
                None,
                "video/mp4"
            ),
            dash_file_identity(
                "video",
                "1",
                "v",
                "https://cdn.test/1.m4s?token=new",
                None,
                "video/mp4"
            ),
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn collects_webvtt_subtitle_tracks_without_calling_them_video() {
        let xml = r#"<MPD type="static"><Period>
<AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="800000" height="720"><SegmentURL media="v.m4s"/></Representation></AdaptationSet>
<AdaptationSet mimeType="text/vtt" lang="en" label="English"><Representation id="sub"><SegmentURL media="en-0.vtt"/><SegmentURL media="en-1.vtt"/></Representation></AdaptationSet>
<AdaptationSet contentType="audio"><Representation id="a" bandwidth="128000"><SegmentURL media="a.m4s"/></Representation></AdaptationSet>
</Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        let subs = select_subtitles(&parsed.representations);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].lang, "en");
        assert_eq!(subs[0].label, "English");
        assert_eq!(subs[0].mime, "text/vtt");
        assert_eq!(
            subs[0].media,
            vec![
                "https://cdn.test/en-0.vtt".to_string(),
                "https://cdn.test/en-1.vtt".to_string()
            ]
        );
        assert_eq!(select_video(&parsed.representations, 0, 0).unwrap().id, "v");
        assert_eq!(select_audio(&parsed.representations, "").unwrap().id, "a");
        assert_eq!(representation_choices(&parsed).len(), 1);
    }

    #[test]
    fn live_subtitle_urls_accumulate_across_mpd_refreshes() {
        let mut subs = Vec::new();
        merge_sub_track(
            &mut subs,
            &Representation {
                id: "sub".into(),
                bandwidth: 0,
                height: 0,
                mime: "text/vtt".into(),
                lang: "en".into(),
                codecs: String::new(),
                content_type: "text".into(),
                label: "English".into(),
                base_url: String::new(),
                init: None,
                init_range: None,
                index_range: None,
                media: vec!["https://cdn.test/en-0.vtt".into()],
                media_ranges: vec![None],
                media_durations: vec![4.0],
                period: 0,
                init_markers: Vec::new(),
                period_markers: Vec::new(),
                whole_file: false,
            },
        );
        merge_sub_track(
            &mut subs,
            &Representation {
                id: "sub".into(),
                bandwidth: 0,
                height: 0,
                mime: "text/vtt".into(),
                lang: "en".into(),
                codecs: String::new(),
                content_type: "text".into(),
                label: "English".into(),
                base_url: String::new(),
                init: None,
                init_range: None,
                index_range: None,
                media: vec!["https://cdn.test/en-1.vtt".into()],
                media_ranges: vec![None],
                media_durations: vec![4.0],
                period: 0,
                init_markers: Vec::new(),
                period_markers: Vec::new(),
                whole_file: false,
            },
        );
        assert_eq!(subs.len(), 1);
        assert_eq!(
            subs[0].media,
            vec![
                "https://cdn.test/en-0.vtt".to_string(),
                "https://cdn.test/en-1.vtt".to_string()
            ]
        );
    }

    #[test]
    fn audio_only_dash_is_selectable() {
        let xml = r#"<MPD type="static"><Period><AdaptationSet mimeType="audio/mp4"><Representation id="a" bandwidth="192000"><SegmentURL media="a.m4s"/></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/audio.mpd").unwrap();
        assert!(select_video(&parsed.representations, 0, 0).is_none());
        assert_eq!(select_audio(&parsed.representations, "").unwrap().id, "a");
        let xml = r#"<MPD type="static"><Period><AdaptationSet mimeType="audio/mp4"><Representation id="en" bandwidth="128000" lang="en"><SegmentURL media="en.m4s"/></Representation><Representation id="ja" bandwidth="192000" lang="ja"><SegmentURL media="ja.m4s"/></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/audio.mpd").unwrap();
        let choices = audio_choices(&parsed);
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().all(|item| item.kind == "audio"));
        assert_eq!(
            select_audio(&parsed.representations, "ja").unwrap().id,
            "ja"
        );
    }
}
