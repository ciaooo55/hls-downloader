//! HLS / LL-HLS downloader. Segments use the HTTP engine; mux is local.

use crate::http_engine::{
    copy_hls_body, entry_network_scope, extract_http_status, fetch_hls_bytes_limited,
    fetch_manifest_bytes, media_network_scope, public_entry, retryable_http_status,
    set_media_network_scope, validate_media_target, EngineError, Job, MediaTransferError,
};
use crate::media::merge::{concat_files, mux_av};
use crate::media::subtitles::{has_cues, merge_webvtt_segments, webvtt_to_srt};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    document_url: String,
    unfiltered_tail: Option<(u64, Option<u64>)>,
    pub is_master: bool,
    pub end_list: bool,
    pub media_sequence: u64,
    pub target_duration: f64,
    pub part_target: f64,
    pub map_uri: Option<String>,
    pub map_byterange: Option<(u64, u64)>,
    pub key: Option<MediaKey>,
    pub can_block_reload: bool,
    pub skipped_segments: u64,
    pub variants: Vec<Variant>,
    pub segments: Vec<Segment>,
    pub renditions: Vec<Rendition>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rendition {
    pub kind: String,
    pub group_id: String,
    pub name: String,
    pub language: String,
    pub uri: Option<String>,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub bandwidth: u64,
    pub uri: String,
    pub codecs: String,
    pub resolution: (u32, u32),
    pub audio_group: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub uri: String,
    pub media_sequence: u64,
    pub part_index: Option<u64>,
    pub key: Option<MediaKey>,
    pub init_map: Option<InitMap>,
    pub gap: bool,
    pub duration: f64,
    pub discontinuity: bool,
    pub byterange: Option<(u64, u64)>,
    pub is_part: bool,
    pub is_ad: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InitMap {
    pub uri: String,
    pub byterange: Option<(u64, u64)>,
    pub key: Option<MediaKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaKey {
    pub method: String,
    pub uri: String,
    pub iv: Option<[u8; 16]>,
}

pub fn parse_playlist(text: &str, base: &str) -> Result<Playlist, String> {
    let public_only = public_entry(base);
    let resolve = |reference: &str| -> Result<String, String> {
        let url = super::resolve_http_uri(base, reference);
        validate_media_target(public_only, &url)?;
        Ok(url)
    };
    let mut lines = text
        .trim_start_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}')
        .lines();
    if lines.next().map(str::trim) != Some("#EXTM3U") {
        return Err("not an HLS playlist".into());
    }
    let mut playlist = Playlist {
        document_url: base.into(),
        unfiltered_tail: None,
        is_master: false,
        end_list: false,
        media_sequence: 0,
        target_duration: 6.0,
        part_target: 0.0,
        map_uri: None,
        map_byterange: None,
        key: None,
        can_block_reload: false,
        skipped_segments: 0,
        variants: Vec::new(),
        segments: Vec::new(),
        renditions: Vec::new(),
    };
    let mut remaining_bytes = super::MAX_EXPANDED_MEDIA_BYTES;
    let mut pending_bandwidth = 0u64;
    let mut pending_codecs = String::new();
    let mut pending_resolution = (0u32, 0u32);
    let mut pending_audio_group = String::new();
    let mut pending_duration = 0.0;
    let mut pending_range: Option<(Option<u64>, u64)> = None;
    let mut media_range_ends = HashMap::new();
    let mut part_range_ends = HashMap::new();
    let mut map_range_ends = HashMap::new();
    let mut current_key: Option<MediaKey> = None;
    let mut current_map: Option<InitMap> = None;
    let mut complete_segments = 0u64;
    let mut part_index = 0u64;
    let mut discontinuity = false;
    let mut part_discontinuity = false;
    let mut pending_gap = false;
    let mut ad_cue_active = false;
    let mut pending_daterange_ad = false;
    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(value) = tag_value(line, "#EXT-X-TARGETDURATION:") {
            playlist.target_duration = parse_hls_duration(value)?;
        } else if let Some(value) = tag_value(line, "#EXT-X-MEDIA-SEQUENCE:") {
            playlist.media_sequence = value.parse().unwrap_or(0);
        } else if let Some(attrs) = tag_value(line, "#EXT-X-SERVER-CONTROL:") {
            playlist.can_block_reload = attr_str(attrs, "CAN-BLOCK-RELOAD")
                .is_some_and(|value| value.eq_ignore_ascii_case("YES"));
        } else if let Some(attrs) = tag_value(line, "#EXT-X-SKIP:") {
            playlist.skipped_segments = attr_u64(attrs, "SKIPPED-SEGMENTS").unwrap_or(0);
        } else if let Some(attrs) = tag_value(line, "#EXT-X-PART-INF:") {
            playlist.part_target = attr_str(attrs, "PART-TARGET")
                .map(|value| parse_hls_duration(&value))
                .transpose()?
                .unwrap_or(0.0);
        } else if line == "#EXT-X-ENDLIST" {
            playlist.end_list = true;
        } else if line == "#EXT-X-DISCONTINUITY" {
            discontinuity = true;
            part_discontinuity = true;
        } else if line == "#EXT-X-GAP" {
            pending_gap = true;
        } else if line.starts_with("#EXT-X-CUE-IN") {
            ad_cue_active = false;
        } else if line.starts_with("#EXT-X-CUE-OUT") {
            ad_cue_active = true;
        } else if let Some(attrs) = tag_value(line, "#EXT-X-DATERANGE:") {
            if daterange_is_ad(attrs) {
                pending_daterange_ad = true;
            }
        } else if let Some(attrs) = tag_value(line, "#EXT-X-STREAM-INF:") {
            playlist.is_master = true;
            pending_bandwidth = attr_u64(attrs, "BANDWIDTH").unwrap_or(0);
            pending_codecs = attr_str(attrs, "CODECS").unwrap_or_default();
            pending_resolution =
                parse_resolution(&attr_str(attrs, "RESOLUTION").unwrap_or_default());
            pending_audio_group = attr_str(attrs, "AUDIO").unwrap_or_default();
        } else if let Some(attrs) = tag_value(line, "#EXT-X-MAP:") {
            let uri = attr_str(attrs, "URI")
                .map(|uri| resolve(&uri))
                .transpose()?
                .filter(|uri| !uri.is_empty());
            let uri = uri.ok_or("HLS MAP 缺少有效 URI")?;
            if current_key.as_ref().is_some_and(|key| key.iv.is_none()) {
                return Err("AES-128 HLS MAP 缺少显式 IV".into());
            }
            let byterange = match attr_str(attrs, "BYTERANGE") {
                Some(value) => Some(resolve_byterange(
                    parse_byterange(&value)?,
                    &uri,
                    &mut map_range_ends,
                )?),
                None => {
                    map_range_ends.clear();
                    None
                }
            };
            playlist.map_uri = Some(uri.clone());
            playlist.map_byterange = byterange;
            current_map = Some(InitMap {
                uri,
                byterange,
                key: current_key.clone(),
            });
        } else if let Some(attrs) = tag_value(line, "#EXT-X-KEY:") {
            let method = attr_str(attrs, "METHOD").unwrap_or_default();
            if method.eq_ignore_ascii_case("NONE") {
                current_key = None;
                playlist.key = None;
                continue;
            }
            if !method.eq_ignore_ascii_case("AES-128") {
                return Err(format!("不支持的 HLS 加密方法: {method}"));
            }
            if let Some(format) = attr_str(attrs, "KEYFORMAT") {
                if !format.is_empty() && !format.eq_ignore_ascii_case("identity") {
                    return Err(format!("不支持 KEYFORMAT={format} / DRM 加密"));
                }
            }
            let key = MediaKey {
                method,
                uri: attr_str(attrs, "URI")
                    .map(|uri| resolve(&uri))
                    .transpose()?
                    .filter(|uri| !uri.is_empty())
                    .unwrap_or_default(),
                iv: attr_str(attrs, "IV")
                    .map(|hex| parse_iv(&hex))
                    .transpose()?,
            };
            if key.uri.is_empty() {
                return Err("AES-128 KEY 缺少有效 URI".into());
            }
            current_key = Some(key.clone());
            playlist.key = Some(key);
        } else if let Some(value) = tag_value(line, "#EXTINF:") {
            pending_duration = parse_hls_duration(value.split(',').next().unwrap_or(value))?;
        } else if let Some(value) = tag_value(line, "#EXT-X-BYTERANGE:") {
            pending_range = Some(parse_byterange(value)?);
        } else if let Some(attrs) = tag_value(line, "#EXT-X-PART:") {
            if let Some(uri) = attr_str(attrs, "URI") {
                let resolved = resolve(&uri)?;
                if resolved.is_empty() {
                    continue;
                }
                charge_segment(
                    &mut remaining_bytes,
                    &resolved,
                    current_key.as_ref(),
                    current_map.as_ref(),
                )?;
                playlist.segments.push(Segment {
                    uri: resolved.clone(),
                    media_sequence: playlist
                        .media_sequence
                        .saturating_add(playlist.skipped_segments)
                        .saturating_add(complete_segments),
                    part_index: Some(part_index),
                    key: current_key.clone(),
                    init_map: current_map.clone(),
                    gap: pending_gap
                        || attr_str(attrs, "GAP")
                            .is_some_and(|value| value.eq_ignore_ascii_case("YES")),
                    duration: attr_str(attrs, "DURATION")
                        .map(|value| parse_hls_duration(&value))
                        .transpose()?
                        .unwrap_or(0.0),
                    discontinuity: std::mem::take(&mut part_discontinuity),
                    byterange: match attr_str(attrs, "BYTERANGE") {
                        Some(value) => Some(resolve_byterange(
                            parse_byterange(&value)?,
                            &resolved,
                            &mut part_range_ends,
                        )?),
                        None => {
                            part_range_ends.clear();
                            None
                        }
                    },
                    is_part: true,
                    is_ad: ad_cue_active || pending_daterange_ad || url_is_ad(&resolved),
                });
                part_index = part_index.saturating_add(1);
            }
        } else if let Some(attrs) = tag_value(line, "#EXT-X-MEDIA:") {
            let rendition = Rendition {
                kind: attr_str(attrs, "TYPE").unwrap_or_default(),
                group_id: attr_str(attrs, "GROUP-ID").unwrap_or_default(),
                name: attr_str(attrs, "NAME").unwrap_or_default(),
                language: attr_str(attrs, "LANGUAGE").unwrap_or_default(),
                uri: attr_str(attrs, "URI")
                    .map(|uri| resolve(&uri))
                    .transpose()?
                    .filter(|uri| !uri.is_empty()),
                default: attr_str(attrs, "DEFAULT")
                    .is_some_and(|value| value.eq_ignore_ascii_case("YES")),
            };
            remaining_bytes = remaining_bytes
                .checked_sub(
                    std::mem::size_of::<Rendition>()
                        + rendition.uri.as_ref().map_or(0, String::len),
                )
                .ok_or("HLS 清单展开超过长度上限")?;
            playlist.renditions.push(rendition);
        } else if !line.starts_with('#') {
            let uri = resolve(line)?;
            if uri.is_empty() {
                pending_bandwidth = 0;
                pending_codecs.clear();
                pending_resolution = (0, 0);
                pending_audio_group.clear();
                pending_daterange_ad = false;
                pending_duration = 0.0;
                pending_range = None;
                discontinuity = false;
                part_discontinuity = false;
                pending_gap = false;
                continue;
            }
            if playlist.is_master || pending_bandwidth > 0 {
                playlist.is_master = true;
                remaining_bytes = remaining_bytes
                    .checked_sub(std::mem::size_of::<Variant>() + uri.len())
                    .ok_or("HLS 清单展开超过长度上限")?;
                playlist.variants.push(Variant {
                    bandwidth: pending_bandwidth,
                    uri,
                    codecs: pending_codecs.clone(),
                    resolution: pending_resolution,
                    audio_group: pending_audio_group.clone(),
                });
                pending_bandwidth = 0;
                pending_codecs.clear();
                pending_resolution = (0, 0);
                pending_audio_group.clear();
            } else {
                let byterange = pending_range
                    .take()
                    .map(|range| resolve_byterange(range, &uri, &mut media_range_ends))
                    .transpose()?;
                if byterange.is_none() {
                    media_range_ends.clear();
                }
                charge_segment(
                    &mut remaining_bytes,
                    &uri,
                    current_key.as_ref(),
                    current_map.as_ref(),
                )?;
                playlist.segments.push(Segment {
                    is_ad: ad_cue_active || pending_daterange_ad || url_is_ad(&uri),
                    uri,
                    media_sequence: playlist
                        .media_sequence
                        .saturating_add(playlist.skipped_segments)
                        .saturating_add(complete_segments),
                    part_index: None,
                    key: current_key.clone(),
                    init_map: current_map.clone(),
                    gap: pending_gap,
                    duration: pending_duration,
                    discontinuity,
                    byterange,
                    is_part: false,
                });
                pending_daterange_ad = false;
                pending_duration = 0.0;
                discontinuity = false;
                part_discontinuity = false;
                pending_gap = false;
                complete_segments = complete_segments.saturating_add(1);
                part_index = 0;
            }
        }
    }
    Ok(playlist)
}

pub fn select_variant_for<'a>(
    playlist: &'a Playlist,
    preferred_bandwidth: u64,
    preferred_height: u32,
    preferred_variant_id: &str,
) -> Option<&'a Variant> {
    if !preferred_variant_id.is_empty() {
        let resolved = super::resolve_http_uri(&playlist.document_url, preferred_variant_id);
        let variants = &playlist.variants;
        if let Some(exact) = variants
            .iter()
            .find(|v| v.uri == preferred_variant_id)
            .or_else(|| variants.iter().find(|v| v.uri == resolved))
        {
            return Some(exact);
        }
        if let Some(target) = variant_origin_path(&resolved) {
            let mut best = None;
            let mut score = None;
            for variant in variants
                .iter()
                .filter(|v| variant_origin_path(&v.uri).as_ref() == Some(&target))
            {
                let distance = (
                    if preferred_bandwidth > 0 {
                        variant.bandwidth.abs_diff(preferred_bandwidth)
                    } else {
                        0
                    },
                    if preferred_height > 0 {
                        variant.resolution.1.abs_diff(preferred_height)
                    } else {
                        0
                    },
                );
                if score.is_none_or(|previous| distance < previous) {
                    best = Some(variant);
                    score = Some(distance);
                } else if score == Some(distance) {
                    best = None;
                }
            }
            if best.is_some() {
                return best;
            }
        }
    }
    if preferred_bandwidth > 0 {
        return playlist.variants.iter().min_by_key(|variant| {
            (
                u8::from(is_audio_only(variant)),
                variant.bandwidth.abs_diff(preferred_bandwidth),
            )
        });
    }
    if preferred_height > 0 {
        return playlist.variants.iter().min_by_key(|variant| {
            let height = if variant.resolution.1 == 0 {
                u32::MAX
            } else {
                variant.resolution.1.abs_diff(preferred_height)
            };
            (
                u8::from(is_audio_only(variant)),
                height,
                u64::MAX - variant.bandwidth,
            )
        });
    }
    playlist.variants.iter().max_by_key(|variant| {
        (
            u8::from(!is_audio_only(variant)),
            variant.resolution.1,
            variant.resolution.0,
            variant.bandwidth,
        )
    })
}

fn variant_origin_path(uri: &str) -> Option<(String, String)> {
    let clean = uri.split(['?', '#']).next()?;
    let (_, authority_path) = clean.split_once("://")?;
    let origin = crate::http_engine::request_origin(clean);
    let path = authority_path
        .find('/')
        .map_or("/", |i| &authority_path[i..]);
    (!origin.is_empty()).then(|| (origin, crate::harvest::normalize_url_path(path)))
}

fn is_audio_only(variant: &Variant) -> bool {
    let codecs = variant.codecs.to_ascii_lowercase();
    codecs.starts_with("mp4a")
        || (variant.resolution == (0, 0)
            && codecs.contains("mp4a")
            && !codecs.contains("avc")
            && !codecs.contains("hvc"))
}

pub fn variant_choices(playlist: &Playlist) -> Vec<crate::StreamVariant> {
    let mut choices: Vec<_> = playlist
        .variants
        .iter()
        .filter(|variant| !is_audio_only(variant))
        .map(|variant| crate::StreamVariant {
            label: variant_label(variant),
            bandwidth: variant.bandwidth,
            height: variant.resolution.1,
            kind: "video".into(),
            name: String::new(),
        })
        .collect();
    if choices.is_empty() {
        choices = playlist
            .variants
            .iter()
            .map(|variant| crate::StreamVariant {
                label: variant_label(variant),
                bandwidth: variant.bandwidth,
                height: variant.resolution.1,
                kind: "video".into(),
                name: String::new(),
            })
            .collect();
    }
    choices.sort_by_key(|item| (u64::MAX - item.bandwidth, item.height));
    choices
}

pub fn audio_choices(playlist: &Playlist) -> Vec<crate::StreamVariant> {
    let has_external_audio = playlist
        .renditions
        .iter()
        .any(|item| item.kind.eq_ignore_ascii_case("AUDIO") && item.uri.is_some());
    if !has_external_audio {
        return Vec::new();
    }
    let mut choices: Vec<_> = playlist
        .renditions
        .iter()
        .filter(|item| {
            item.kind.eq_ignore_ascii_case("AUDIO")
                && (item.uri.is_some() || !item.name.is_empty() || !item.language.is_empty())
        })
        .map(|item| crate::StreamVariant {
            label: if item.language.is_empty() {
                item.name.clone()
            } else if item.name.is_empty() {
                item.language.clone()
            } else {
                format!("{} · {}", item.name, item.language)
            },
            bandwidth: 0,
            height: 0,
            kind: "audio".into(),
            name: item.name.clone(),
        })
        .collect();
    choices.sort_by_key(|item| item.label.clone());
    choices
}

fn variant_label(variant: &Variant) -> String {
    let height = if variant.resolution.1 > 0 {
        format!("{}p", variant.resolution.1)
    } else {
        "自适应".into()
    };
    if variant.bandwidth >= 1_000_000 {
        format!(
            "{height} · {:.1} Mbps",
            variant.bandwidth as f64 / 1_000_000.0
        )
    } else if variant.bandwidth > 0 {
        format!("{height} · {} kbps", variant.bandwidth / 1000)
    } else {
        height
    }
}

pub fn select_audio_track<'a>(
    playlist: &'a Playlist,
    variant: &Variant,
    preferred: &str,
) -> Option<&'a Rendition> {
    let mut tracks: Vec<&Rendition> = playlist
        .renditions
        .iter()
        .filter(|item| {
            item.kind.eq_ignore_ascii_case("AUDIO")
                && (variant.audio_group.is_empty() || item.group_id == variant.audio_group)
        })
        .collect();
    tracks.sort_by_key(|item| (!item.default, item.language.clone(), item.name.clone()));
    let preferred = preferred.trim();
    if !preferred.is_empty() {
        if let Some(hit) = tracks
            .iter()
            .copied()
            .find(|item| item.name.eq_ignore_ascii_case(preferred))
            .or_else(|| {
                tracks
                    .iter()
                    .copied()
                    .find(|item| item.language.eq_ignore_ascii_case(preferred))
            })
        {
            return Some(hit);
        }
    }
    tracks.into_iter().next()
}

pub fn select_subtitles(playlist: &Playlist) -> Vec<Rendition> {
    let mut tracks: Vec<Rendition> = playlist
        .renditions
        .iter()
        .filter(|item| item.kind.eq_ignore_ascii_case("SUBTITLES") && item.uri.is_some())
        .cloned()
        .collect();
    tracks.sort_by_key(|item| (!item.default, item.language.clone(), item.name.clone()));
    tracks
}

#[derive(Debug, Clone)]
pub struct HlsDownloadOptions {
    pub live: bool,
    pub concurrency: usize,
    pub preferred_bandwidth: u64,
    pub preferred_height: u32,
    pub preferred_variant_id: String,
    pub preferred_audio: String,
    pub skip_ads: bool,
    pub download_subtitles: bool,
    pub live_max_minutes: u64,
    pub progress: Option<PathBuf>,
    /// 纯 TS 点播（可直接拼接）时，拼接后用 FFmpeg 无损重封装为真正的 MP4。
    /// 默认开启；若未安装 FFmpeg 或重封装失败则自动回退保留已拼接 TS。
    pub remux_ts_to_mp4: bool,
}

impl Default for HlsDownloadOptions {
    fn default() -> Self {
        Self {
            live: false,
            concurrency: 8,
            preferred_bandwidth: 0,
            preferred_height: 0,
            preferred_variant_id: String::new(),
            preferred_audio: String::new(),
            skip_ads: true,
            download_subtitles: true,
            live_max_minutes: 0,
            progress: None,
            remux_ts_to_mp4: true,
        }
    }
}

#[derive(Debug, Clone)]
struct TimelineEntry {
    identity: String,
    uri: String,
    media_sequence: u64,
    part_index: Option<u64>,
    path: PathBuf,
    duration: f64,
    discontinuity: bool,
    init_map_path: Option<PathBuf>,
}

#[derive(Default)]
struct LiveTimeline {
    entries: Vec<TimelineEntry>,
    next_slot: usize,
    sequence_offset: u64,
    pending_epoch_discontinuity: bool,
}

struct PreparedSegment {
    segment: Segment,
    identity: String,
    timeline_sequence: u64,
    slot: usize,
    path: PathBuf,
    init_map_path: Option<PathBuf>,
    key_bytes: Option<Vec<u8>>,
    needs_download: bool,
}

#[derive(Clone, Copy)]
struct LiveControl<'a> {
    control: &'a Path,
    parent_control: Option<&'a Path>,
    finishing: Option<&'a AtomicBool>,
}

impl<'a> LiveControl<'a> {
    fn direct(control: &'a Path) -> Self {
        Self {
            control,
            parent_control: None,
            finishing: None,
        }
    }

    fn effective_action(&self) -> String {
        if let Some(parent) = self.parent_control {
            match read_control(parent).as_str() {
                "cancel" => return "cancel".into(),
                "pause" => return "pause".into(),
                _ => {}
            }
        }
        let raw = read_control(self.control);
        if raw == "cancel"
            && self
                .finishing
                .is_some_and(|finishing| finishing.load(Ordering::SeqCst))
        {
            return "finish".into();
        }
        raw
    }

    fn merge_control(&self) -> &'a Path {
        self.parent_control.unwrap_or(self.control)
    }
}

struct SegmentWork {
    segment: Segment,
    path: PathBuf,
    key_bytes: Option<Vec<u8>>,
}

#[cfg(test)]
pub fn download_hls(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    live: bool,
) -> Result<PathBuf, String> {
    download_hls_with(
        url,
        headers,
        proxy,
        task_dir,
        control,
        HlsDownloadOptions {
            live,
            ..HlsDownloadOptions::default()
        },
    )
}

/// Staging name of the concatenated TS before the optional MP4 remux.
pub(crate) const REMUX_STAGING_TS: &str = "merged.ts";

pub fn download_hls_with(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    options: HlsDownloadOptions,
) -> Result<PathBuf, String> {
    let _network = set_media_network_scope(Some(entry_network_scope(url, proxy)));
    super::with_media_headers(url, headers, || {
        download_hls_scoped(url, headers, proxy, task_dir, control, options)
    })
}

fn download_hls_scoped(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    options: HlsDownloadOptions,
) -> Result<PathBuf, String> {
    download_hls_scoped_with_control(
        url,
        headers,
        proxy,
        task_dir,
        LiveControl::direct(control),
        options,
    )
}

fn download_hls_scoped_with_control(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    live_ctl: LiveControl<'_>,
    options: HlsDownloadOptions,
) -> Result<PathBuf, String> {
    let control = live_ctl.control;
    fs::create_dir_all(task_dir).map_err(|error| error.to_string())?;
    let saved_timeline = LiveTimeline::load(task_dir);
    let load_initial = |target: &str| -> Result<Option<Playlist>, String> {
        match load_playlist(target, headers, proxy) {
            Ok(pl) => Ok(Some(pl)),
            Err(error) => match live_ctl.effective_action().as_str() {
                "cancel" => Err("canceled".into()),
                "pause" => Err("paused".into()),
                "finish" if options.live || !saved_timeline.entries.is_empty() => Ok(None),
                _ => Err(error),
            },
        }
    };
    let mut current = url.to_string();
    let mut initial_playlist = load_initial(&current)?;
    let mut audio_uri = None;
    let mut subtitle_tracks = Vec::new();
    if let Some(ref master) = initial_playlist {
        if master.is_master {
            let variant = select_variant_for(
                master,
                options.preferred_bandwidth,
                options.preferred_height,
                &options.preferred_variant_id,
            )
            .ok_or_else(|| "HLS master has no variants".to_string())?;
            audio_uri = select_audio_track(master, variant, &options.preferred_audio)
                .and_then(|track| track.uri.clone());
            if options.download_subtitles {
                subtitle_tracks = select_subtitles(master);
            }
            current = variant.uri.clone();
            initial_playlist = load_initial(&current)?;
        }
    }
    let finish_before_playlist = initial_playlist.is_none();
    let mut playlist = initial_playlist.unwrap_or(Playlist {
        document_url: current.clone(),
        unfiltered_tail: None,
        is_master: false,
        end_list: false,
        media_sequence: 0,
        target_duration: 6.0,
        part_target: 0.0,
        map_uri: None,
        map_byterange: None,
        key: None,
        can_block_reload: false,
        skipped_segments: 0,
        variants: Vec::new(),
        segments: Vec::new(),
        renditions: Vec::new(),
    });
    // 主清单和 URL 都不能证明直播类型；以最终媒体清单及已有录制状态为准。
    let live = options.live || !playlist.end_list || !saved_timeline.entries.is_empty();
    apply_ad_policy(&mut playlist, options.skip_ads);
    let has_external_audio = audio_uri.is_some();
    let mut audio_merged = None;
    let audio_dir = task_dir.join("audio");
    let mut audio_vod = VodCheckpoint::load(&audio_dir);
    let mut audio_prepared = Vec::new();
    let mut audio_target_duration = 1.0;
    if !live {
        if let Some(audio) = audio_uri.take() {
            let mut audio_playlist = load_playlist(&audio, headers, proxy)?;
            if audio_playlist.is_master {
                let variant = select_variant_for(&audio_playlist, 0, 0, "")
                    .ok_or("HLS audio master has no variants")?;
                audio_playlist = load_playlist(&variant.uri, headers, proxy)?;
            }
            apply_ad_policy(&mut audio_playlist, options.skip_ads);
            fs::create_dir_all(audio_dir.join("segments")).map_err(|error| error.to_string())?;
            audio_target_duration = audio_playlist.target_duration;
            audio_prepared = prepare_segments(
                pending_segments(&audio_playlist, &[]),
                headers,
                proxy,
                &audio_dir,
                control,
                false,
                0,
                &audio_vod,
                &mut 0,
                &mut HashMap::new(),
                &mut HashMap::new(),
            )?;
            if audio_prepared.is_empty() {
                return Err("HLS audio track has no downloadable segments".into());
            }
        }
    }
    let seg_dir = task_dir.join("segments");
    fs::create_dir_all(&seg_dir).map_err(|error| error.to_string())?;
    let mut vod = VodCheckpoint::load(task_dir);
    let state_path = task_dir.join("live_state.json");
    let mut timeline = if live {
        saved_timeline
    } else {
        LiveTimeline::default()
    };
    let mut key_cache = HashMap::new();
    let mut map_cache = HashMap::new();
    for entry in &timeline.entries {
        if let Some(path) = &entry.init_map_path {
            map_cache.insert(path.to_string_lossy().to_string(), path.clone());
        }
    }
    let mut recorded_duration: f64 = timeline.entries.iter().map(|entry| entry.duration).sum();
    let live_limit = if live && options.live_max_minutes > 0 {
        Some(options.live_max_minutes as f64 * 60.0)
    } else {
        None
    };
    let mut live_audio = if live && !finish_before_playlist {
        audio_uri
            .as_ref()
            .map(|uri| LiveAudioRecorder::start_with_parent(uri, headers, proxy, task_dir, control))
    } else {
        None
    };
    let mut live_subs = if live && options.download_subtitles {
        LiveSubSession::load(task_dir, &subtitle_tracks)
    } else {
        LiveSubSession::default()
    };
    loop {
        if finish_before_playlist {
            break;
        }
        match live_ctl.effective_action().as_str() {
            "cancel" => return Err("canceled".into()),
            "pause" => {
                if live {
                    timeline.save(&state_path)?;
                    write_timeline_playlist(
                        task_dir,
                        playlist.target_duration,
                        &timeline.entries,
                        false,
                    )?;
                }
                return Err("paused".into());
            }
            "finish" if live => break,
            _ => {}
        }
        if live && options.download_subtitles {
            live_subs.capture(headers, proxy, task_dir, control);
        }
        if live {
            timeline.observe_playlist(&playlist);
        }
        let mut pending = if live {
            pending_live_segments(&playlist, &timeline)
        } else {
            pending_segments(&playlist, &timeline.entries)
        };
        if let Some(limit) = live_limit {
            let mut remaining = (limit - recorded_duration).max(0.0);
            pending.retain(|segment| {
                if remaining <= 0.0 {
                    return false;
                }
                remaining -= segment.duration.max(0.0);
                true
            });
        }
        let prepared = match prepare_segments(
            pending,
            headers,
            proxy,
            task_dir,
            control,
            live,
            timeline.sequence_offset,
            &vod,
            &mut timeline.next_slot,
            &mut key_cache,
            &mut map_cache,
        ) {
            Ok(prepared) => prepared,
            Err(_) if live_ctl.effective_action() == "cancel" => return Err("canceled".into()),
            Err(stop) if live && (stop == "finish" || live_ctl.effective_action() == "finish") => {
                break
            }
            Err(stop) if live && (stop == "paused" || live_ctl.effective_action() == "pause") => {
                timeline.save(&state_path)?;
                write_timeline_playlist(
                    task_dir,
                    playlist.target_duration,
                    &timeline.entries,
                    false,
                )?;
                return Err("paused".into());
            }
            Err(error) => return Err(error),
        };
        let previewable = !live
            && !prepared
                .iter()
                .chain(&audio_prepared)
                .any(|item| item.segment.is_part);
        if previewable {
            let marker_root = control.parent().unwrap_or(task_dir);
            for item in prepared.iter().chain(&audio_prepared) {
                if item.needs_download {
                    super::preview::reset_ready(marker_root, &item.path)?;
                } else {
                    super::preview::mark_ready(marker_root, &item.path)?;
                }
                if let Some(map) = &item.init_map_path {
                    super::preview::mark_ready(marker_root, map)?;
                }
            }
            let entries: Vec<_> = prepared
                .iter()
                .map(|item| TimelineEntry {
                    identity: item.identity.clone(),
                    uri: stable_media_url(&item.segment.uri),
                    media_sequence: item.timeline_sequence,
                    part_index: item.segment.part_index,
                    path: item.path.clone(),
                    duration: item.segment.duration.max(0.0),
                    discontinuity: item.segment.discontinuity,
                    init_map_path: item.init_map_path.clone(),
                })
                .collect();
            // 完整时间轴供播放器定位；服务器在目标分片就绪前等待，不返回半个文件。
            write_text_atomic(
                &task_dir.join("preview-video.m3u8"),
                &timeline_playlist(playlist.target_duration, &entries, true),
            )?;
            if has_external_audio {
                let audio_entries = buffered_entries(&[], &audio_prepared, &HashSet::new(), false);
                write_text_atomic(
                    &audio_dir.join("preview-audio.m3u8"),
                    &timeline_playlist(audio_target_duration, &audio_entries, true),
                )?;
            }
        }
        if !live {
            // A slot about to be downloaded again must not keep an old record:
            // its new file is only synced at group commit, and an old record of
            // the same identity and size would vouch for an unsynced file.
            let mut forgot = false;
            for item in prepared.iter().filter(|item| item.needs_download) {
                forgot |= vod.forget(item.slot);
            }
            if forgot {
                vod.save(task_dir)?;
            }
            let mut forgot = false;
            for item in audio_prepared.iter().filter(|item| item.needs_download) {
                forgot |= audio_vod.forget(item.slot);
            }
            if forgot {
                audio_vod.save(&audio_dir)?;
            }
        }
        let durable_slots: HashMap<PathBuf, (bool, usize, String)> = prepared
            .iter()
            .map(|item| (item.path.clone(), (false, item.slot, item.identity.clone())))
            .chain(
                audio_prepared
                    .iter()
                    .map(|item| (item.path.clone(), (true, item.slot, item.identity.clone()))),
            )
            .collect();
        let checkpoints = Mutex::new((&mut vod, &mut audio_vod));
        let record_durable = |paths: &[PathBuf]| -> Result<(), String> {
            let mut guard = checkpoints
                .lock()
                .map_err(|_| "HLS checkpoint lock poisoned".to_string())?;
            let (video, audio) = &mut *guard;
            let (mut video_changed, mut audio_changed) = (false, false);
            for path in paths {
                if let Some((is_audio, slot, identity)) = durable_slots.get(path) {
                    if *is_audio {
                        audio.remember(*slot, identity, file_len(path));
                        audio_changed = true;
                    } else {
                        video.remember(*slot, identity, file_len(path));
                        video_changed = true;
                    }
                }
            }
            if video_changed {
                video.save(task_dir)?;
            }
            if audio_changed {
                audio.save(&audio_dir)?;
            }
            Ok(())
        };
        let preview_update = Mutex::new(((0usize, 0usize), std::time::Instant::now()));
        let (completed, stop_error) = download_prepared_segments(
            &prepared,
            &audio_prepared,
            headers,
            proxy,
            control,
            options.concurrency.max(1),
            options.progress.as_deref().map(|path| {
                (
                    path,
                    timeline
                        .entries
                        .iter()
                        .map(|entry| file_len(&entry.path))
                        .sum::<u64>(),
                    timeline.entries.len() as u64,
                    timeline.entries.len() as u64
                        + prepared.len() as u64
                        + audio_prepared.len() as u64,
                    live,
                )
            }),
            &|completed| {
                // 点播前缀可以持续追加；直播/LL-HLS 会替换 part，暂不暴露为 EVENT 预览。
                if !previewable {
                    return;
                }
                let video_ready = prepared
                    .iter()
                    .take_while(|item| !item.needs_download || completed.contains(&item.path))
                    .count();
                let audio_ready = audio_prepared
                    .iter()
                    .take_while(|item| !item.needs_download || completed.contains(&item.path))
                    .count();
                let ready = (video_ready, audio_ready);
                if let Ok(mut last) = preview_update.lock() {
                    if ready == last.0
                        || (last.0 .0 > 0
                            && (!has_external_audio || last.0 .1 > 0)
                            && ready != (prepared.len(), audio_prepared.len())
                            && last.1.elapsed() < std::time::Duration::from_millis(500))
                    {
                        return;
                    }
                    *last = (ready, std::time::Instant::now());
                }
                if has_external_audio {
                    if audio_ready == 0 {
                        return;
                    }
                    let entries = buffered_entries(&[], &audio_prepared, completed, true);
                    if let Err(error) =
                        write_timeline_playlist(&audio_dir, audio_target_duration, &entries, false)
                    {
                        eprintln!("HLS audio preview playlist update failed: {error}");
                        return;
                    }
                }
                if let Err(error) = write_buffered_preview(
                    task_dir,
                    &timeline.entries,
                    &prepared,
                    completed,
                    playlist.target_duration,
                    has_external_audio,
                ) {
                    eprintln!("HLS preview playlist update failed: {error}");
                }
            },
            (!live).then_some(&record_durable as DurableSegments<'_>),
        );
        if let Some(error) = stop_error {
            // A stopping run must still vouch for the segments that landed:
            // `remember`/`save` used to sit after this call, so a pause threw
            // away the record of every completed segment and the resumed run
            // downloaded them again.
            if live {
                for item in prepared
                    .iter()
                    .filter(|item| completed.contains(&item.path))
                {
                    let discontinuity = item.segment.discontinuity
                        || std::mem::take(&mut timeline.pending_epoch_discontinuity);
                    timeline.insert(TimelineEntry {
                        identity: item.identity.clone(),
                        uri: stable_media_url(&item.segment.uri),
                        media_sequence: item.timeline_sequence,
                        part_index: item.segment.part_index,
                        path: item.path.clone(),
                        duration: item.segment.duration.max(0.0),
                        discontinuity,
                        init_map_path: item.init_map_path.clone(),
                    });
                }
                timeline.save(&state_path)?;
                write_timeline_playlist(
                    task_dir,
                    playlist.target_duration,
                    &timeline.entries,
                    false,
                )?;
                let action = live_ctl.effective_action();
                if action == "cancel" {
                    return Err("canceled".into());
                }
                if action == "pause" || error == "paused" {
                    return Err("paused".into());
                }
                if error == "finish" || action == "finish" {
                    break;
                }
            } else {
                for item in prepared
                    .iter()
                    .filter(|item| completed.contains(&item.path))
                {
                    vod.remember(item.slot, &item.identity, file_len(&item.path));
                }
                let _ = vod.save(task_dir);
                if has_external_audio {
                    for item in audio_prepared
                        .iter()
                        .filter(|item| completed.contains(&item.path))
                    {
                        audio_vod.remember(item.slot, &item.identity, file_len(&item.path));
                    }
                    let _ = audio_vod.save(&audio_dir);
                }
                if live_ctl.effective_action() == "cancel" {
                    return Err("canceled".into());
                }
            }
            return Err(error);
        }
        for prepared in prepared {
            if !live {
                vod.remember(prepared.slot, &prepared.identity, file_len(&prepared.path));
            }
            let duration = prepared.segment.duration.max(0.0);
            let discontinuity = prepared.segment.discontinuity
                || std::mem::take(&mut timeline.pending_epoch_discontinuity);
            timeline.insert(TimelineEntry {
                identity: prepared.identity,
                uri: stable_media_url(&prepared.segment.uri),
                media_sequence: prepared.timeline_sequence,
                part_index: prepared.segment.part_index,
                path: prepared.path,
                duration,
                discontinuity,
                init_map_path: prepared.init_map_path,
            });
            recorded_duration = timeline.entries.iter().map(|entry| entry.duration).sum();
            if live {
                timeline.save(&state_path)?;
                write_timeline_playlist(
                    task_dir,
                    playlist.target_duration,
                    &timeline.entries,
                    false,
                )?;
            }
        }
        if !live {
            vod.save(task_dir)?;
        }
        if let Some(progress) = &options.progress {
            let downloaded: u64 = timeline
                .entries
                .iter()
                .map(|entry| file_len(&entry.path))
                .sum();
            let completed = (timeline.entries.len() + audio_prepared.len()) as u64;
            crate::http_engine::write_media_progress(
                progress,
                downloaded
                    + audio_prepared
                        .iter()
                        .map(|item| file_len(&item.path))
                        .sum::<u64>(),
                completed,
                completed,
                live,
            );
        }
        if live_limit.is_some_and(|limit| recorded_duration >= limit) {
            break;
        }
        if playlist.end_list || !live {
            break;
        }
        let previous_tail = playlist_tail(&playlist);
        let reloaded = if playlist.can_block_reload {
            reload_playlist(&current, headers, proxy, &playlist, control)
        } else {
            let wait = if playlist.part_target > 0.0 {
                playlist.part_target
            } else {
                (playlist.target_duration / 2.0).max(0.2)
            };
            wait_with_control(control, wait.min(6.0))
                .and_then(|()| load_playlist(&current, headers, proxy))
        };
        playlist = match reloaded {
            Ok(next) => next,
            Err(_) if live_ctl.effective_action() == "cancel" => return Err("canceled".into()),
            Err(stop) if stop == "finish" || live_ctl.effective_action() == "finish" => break,
            Err(stop) if stop == "paused" || live_ctl.effective_action() == "pause" => {
                timeline.save(&state_path)?;
                write_timeline_playlist(
                    task_dir,
                    playlist.target_duration,
                    &timeline.entries,
                    false,
                )?;
                return Err("paused".into());
            }
            Err(error) => return Err(error),
        };
        apply_ad_policy(&mut playlist, options.skip_ads);
        match live_ctl.effective_action().as_str() {
            "cancel" => return Err("canceled".into()),
            "pause" => {
                timeline.save(&state_path)?;
                write_timeline_playlist(
                    task_dir,
                    playlist.target_duration,
                    &timeline.entries,
                    false,
                )?;
                return Err("paused".into());
            }
            "finish" => break,
            _ => {}
        }
        if playlist.end_list {
            continue;
        }
        timeline.observe_playlist(&playlist);
        if playlist.can_block_reload
            && pending_live_segments(&playlist, &timeline).is_empty()
            && tail_position(playlist_tail(&playlist)) <= tail_position(previous_tail)
        {
            let poll = if playlist.target_duration.is_finite() {
                (playlist.target_duration / 2.0).clamp(0.2, 2.0)
            } else {
                0.5
            };
            match wait_with_control(control, poll) {
                Ok(()) => {}
                Err(_) if live_ctl.effective_action() == "cancel" => return Err("canceled".into()),
                Err(stop) if stop == "finish" || live_ctl.effective_action() == "finish" => break,
                Err(stop) if stop == "paused" || live_ctl.effective_action() == "pause" => {
                    timeline.save(&state_path)?;
                    write_timeline_playlist(
                        task_dir,
                        playlist.target_duration,
                        &timeline.entries,
                        false,
                    )?;
                    return Err("paused".into());
                }
                Err(error) => return Err(error),
            }
        }
    }
    match live_ctl.effective_action().as_str() {
        "cancel" => return Err("canceled".into()),
        "pause" => {
            if live {
                timeline.save(&state_path)?;
                write_timeline_playlist(
                    task_dir,
                    playlist.target_duration,
                    &timeline.entries,
                    false,
                )?;
            }
            return Err("paused".into());
        }
        _ => {}
    }
    if live_ctl
        .finishing
        .is_some_and(|finishing| finishing.load(Ordering::SeqCst))
    {
        let _ = crate::task_control::set(control, "run")
            .or_else(|_| fs::write(control, "run").map_err(|error| error.to_string()));
    }
    let merge_control = live_ctl.merge_control();
    if let Some(recorder) = live_audio.as_mut() {
        audio_merged = recorder.finish()?;
    }
    if timeline.entries.is_empty() {
        if live {
            return Err("未录制到任何直播分片".into());
        }
        return Err("no segments to concatenate".into());
    }
    if playlist.end_list || !live {
        vod.save(task_dir)?;
    }
    write_timeline_playlist(task_dir, playlist.target_duration, &timeline.entries, true)?;
    if has_external_audio && !live {
        for item in &audio_prepared {
            audio_vod.remember(item.slot, &item.identity, file_len(&item.path));
        }
        audio_vod.save(&audio_dir)?;
        let entries = buffered_entries(&[], &audio_prepared, &HashSet::new(), false);
        write_timeline_playlist(&audio_dir, audio_target_duration, &entries, true)?;
        if let Some(progress) = &options.progress {
            crate::http_engine::write_media_merging(progress);
        }
        let output = audio_dir.join("merged.mp4");
        super::merge::merge_with_ffmpeg_opts(&audio_dir, &output, merge_control, false)?;
        audio_merged = Some(output);
    }
    let mut subtitles = Vec::new();
    if live && options.download_subtitles {
        subtitles = live_subs.publish(task_dir);
    } else {
        let mut labels = BTreeSet::new();
        for track in subtitle_tracks {
            if let Some(uri) = &track.uri {
                let sub_dir = task_dir.join("subs");
                fs::create_dir_all(&sub_dir).map_err(|error| error.to_string())?;
                let base = safe_label(&track.language, "und");
                let mut label = base.clone();
                let mut suffix = 2;
                while !labels.insert(label.to_ascii_lowercase()) {
                    label = format!("{base}-{suffix}");
                    suffix += 1;
                }
                let path = sub_dir.join(format!("{label}.vtt"));
                match download_vod_subtitle(uri, headers, proxy, &path, merge_control) {
                    Ok(()) => subtitles.push(path),
                    Err(error) if error == "paused" || error == "canceled" => return Err(error),
                    Err(error) => eprintln!("HLS subtitle download failed: {error}"),
                }
            }
        }
    }
    if let Some(progress) = options.progress.as_ref() {
        crate::http_engine::write_media_merging(progress);
    }
    let output = task_dir.join("merged.mp4");
    let files: Vec<PathBuf> = timeline
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let can_concat = timeline.entries.iter().all(|entry| {
        entry.init_map_path.is_none()
            && !entry.discontinuity
            && extension(entry.path.to_string_lossy().as_ref()).eq_ignore_ascii_case("ts")
    });
    let needs_final_mux = audio_merged.is_some() || !subtitles.is_empty();
    if can_concat && needs_final_mux {
        let staged = task_dir.join(REMUX_STAGING_TS);
        concat_files(&files, &staged)?;
        let muxed = task_dir.join("muxed.mp4");
        let mux_result = mux_av(
            &staged,
            audio_merged.as_deref(),
            &subtitles,
            &muxed,
            merge_control,
        );
        let _ = fs::remove_file(&staged);
        mux_result?;
        if let Some(audio) = &audio_merged {
            let _ = fs::remove_file(audio);
        }
        return Ok(muxed);
    }
    if can_concat && options.remux_ts_to_mp4 && super::merge::locate_ffmpeg().is_some() {
        // 可选：先拼接到 merged.ts，再无损重封装为 MP4；FFmpeg 失败时回落为原拼接结果。
        let staged = task_dir.join(REMUX_STAGING_TS);
        concat_files(&files, &staged)?;
        match super::merge::remux_ts_to_mp4_opts(&staged, &output, merge_control, !needs_final_mux)
        {
            Ok(()) => {
                let _ = fs::remove_file(&staged);
            }
            Err(error) if error == "paused" || error == "canceled" => {
                let _ = fs::remove_file(&staged);
                return Err(error);
            }
            Err(error) => {
                eprintln!("HLS TS→MP4 remux failed, keeping concatenated TS: {error}");
                if let Err(replace_error) = crate::atomic_replace::replace_file(&staged, &output) {
                    let _ = fs::remove_file(&staged);
                    return Err(replace_error.to_string());
                }
            }
        }
    } else if can_concat {
        concat_files(&files, &output)?;
    } else {
        super::merge::merge_with_ffmpeg_opts(task_dir, &output, merge_control, !needs_final_mux)?;
    }
    if needs_final_mux {
        let muxed = task_dir.join("muxed.mp4");
        let mux_result = mux_av(
            &output,
            audio_merged.as_deref(),
            &subtitles,
            &muxed,
            merge_control,
        );
        let _ = fs::remove_file(&output);
        mux_result?;
        if let Some(audio) = &audio_merged {
            let _ = fs::remove_file(audio);
        }
        return Ok(muxed);
    }
    Ok(output)
}

impl LiveTimeline {
    fn load(task_dir: &Path) -> Self {
        let path = task_dir.join("live_state.json");
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Self::default();
        };
        if value.get("version").and_then(|item| item.as_u64()) != Some(2) {
            return Self::default();
        }
        let sequence_offset = value
            .get("sequence_offset")
            .and_then(|item| item.as_u64())
            .unwrap_or(0);
        let pending_epoch_discontinuity = value
            .get("pending_epoch_discontinuity")
            .and_then(|item| item.as_bool())
            .unwrap_or(false);
        let mut entries = Vec::new();
        let Some(items) = value.get("segments").and_then(|item| item.as_array()) else {
            return Self::default();
        };
        for item in items {
            let identity = item
                .get("identity")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let uri = item
                .get("uri")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let Some(media_sequence) = item.get("media_sequence").and_then(|value| value.as_u64())
            else {
                continue;
            };
            let Some(path) = item
                .get("path")
                .and_then(|value| value.as_str())
                .and_then(|leaf| state_file(task_dir, leaf))
            else {
                continue;
            };
            if identity.is_empty() || file_len(&path) == 0 {
                continue;
            }
            let init_map_path = item
                .get("init_map_path")
                .and_then(|value| value.as_str())
                .and_then(|leaf| state_file(task_dir, leaf))
                .filter(|path| file_len(path) > 0);
            entries.push(TimelineEntry {
                identity,
                uri,
                media_sequence,
                part_index: item.get("part_index").and_then(|value| value.as_u64()),
                path,
                duration: item
                    .get("duration")
                    .and_then(|value| value.as_f64())
                    .unwrap_or(0.0)
                    .max(0.0),
                discontinuity: item
                    .get("discontinuity")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
                init_map_path,
            });
        }
        sort_timeline(&mut entries);
        let next_slot = entries
            .iter()
            .filter_map(|entry| {
                entry
                    .path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .max()
            .map(|slot| slot.saturating_add(1))
            .unwrap_or(0);
        Self {
            entries,
            next_slot,
            sequence_offset,
            pending_epoch_discontinuity,
        }
    }

    fn effective_sequence(&self, raw_sequence: u64) -> u64 {
        self.sequence_offset.saturating_add(raw_sequence)
    }

    fn observe_playlist(&mut self, playlist: &Playlist) {
        let non_gap: Vec<&Segment> = playlist
            .segments
            .iter()
            .filter(|segment| !segment.gap)
            .collect();
        if self.entries.is_empty() || non_gap.is_empty() {
            return;
        }
        let offset = self.sequence_offset;
        let any_matches = non_gap.iter().any(|segment| {
            let cand_seq = self.effective_sequence(segment.media_sequence);
            let stable = stable_media_url(&segment.uri);
            let cand_id = live_segment_identity(segment, offset);
            self.entries.iter().any(|entry| {
                entry.media_sequence == cand_seq
                    && entry.part_index == segment.part_index
                    && (entry.identity == cand_id || (!entry.uri.is_empty() && entry.uri == stable))
            })
        });
        if any_matches {
            return;
        }
        let max_timeline_seq = self
            .entries
            .iter()
            .map(|entry| entry.media_sequence)
            .max()
            .unwrap_or(0);
        let min_cand = non_gap
            .iter()
            .map(|segment| self.effective_sequence(segment.media_sequence))
            .min()
            .unwrap_or(0);
        let conflicting_slot = non_gap.iter().any(|segment| {
            let cand_seq = self.effective_sequence(segment.media_sequence);
            self.entries.iter().any(|entry| {
                entry.media_sequence == cand_seq && entry.part_index == segment.part_index
            })
        });
        if min_cand < max_timeline_seq || conflicting_slot {
            self.sequence_offset = max_timeline_seq.saturating_add(1);
            self.pending_epoch_discontinuity = true;
        }
    }

    fn insert(&mut self, mut entry: TimelineEntry) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|existing| existing.identity == entry.identity)
        {
            existing.discontinuity |= entry.discontinuity;
            if existing.uri.is_empty() && !entry.uri.is_empty() {
                existing.uri = entry.uri;
            }
            return;
        }
        let mut removed = Vec::new();
        self.entries.retain(|existing| {
            let replace = if entry.part_index.is_none() {
                existing.media_sequence == entry.media_sequence
            } else {
                existing.media_sequence == entry.media_sequence
                    && existing.part_index == entry.part_index
            };
            if replace {
                entry.discontinuity |= existing.discontinuity;
                if existing.path != entry.path {
                    removed.push(existing.path.clone());
                }
            }
            !replace
        });
        self.entries.push(entry);
        sort_timeline(&mut self.entries);
        for path in removed {
            let _ = fs::remove_file(path);
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let segments: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "identity": entry.identity,
                    "uri": entry.uri,
                    "media_sequence": entry.media_sequence,
                    "part_index": entry.part_index,
                    "path": playlist_leaf(&entry.path),
                    "duration": entry.duration,
                    "discontinuity": entry.discontinuity,
                    "init_map_path": entry.init_map_path.as_deref().map(playlist_leaf),
                })
            })
            .collect();
        write_json_atomic(
            path,
            &serde_json::json!({
                "version": 2,
                "sequence_offset": self.sequence_offset,
                "pending_epoch_discontinuity": self.pending_epoch_discontinuity,
                "segments": segments,
            })
            .to_string(),
        )
    }
}

fn write_buffered_preview(
    task_dir: &Path,
    previous: &[TimelineEntry],
    prepared: &[PreparedSegment],
    completed: &HashSet<PathBuf>,
    target_duration: f64,
    external_audio: bool,
) -> Result<(), String> {
    let entries = buffered_entries(previous, prepared, completed, true);
    if entries.is_empty() {
        return Ok(());
    }
    write_timeline_playlist(task_dir, target_duration, &entries, false)?;
    if external_audio && !task_dir.join("audio/local.m3u8").is_file() {
        return Ok(());
    }
    let master = if external_audio {
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Audio\",DEFAULT=YES,AUTOSELECT=YES,URI=\"audio/preview-audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1,AUDIO=\"audio\"\npreview-video.m3u8\n"
    } else {
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-STREAM-INF:BANDWIDTH=1\npreview-video.m3u8\n"
    };
    write_text_atomic(&task_dir.join("preview.m3u8"), master)
}

fn buffered_entries(
    previous: &[TimelineEntry],
    prepared: &[PreparedSegment],
    completed: &HashSet<PathBuf>,
    contiguous_only: bool,
) -> Vec<TimelineEntry> {
    let mut entries = previous.to_vec();
    // 并行下载会乱序完成；只发布无缺口的前缀，不能让播放器跳过尚未落盘的片段。
    for item in prepared.iter().take_while(|item| {
        !contiguous_only || !item.needs_download || completed.contains(&item.path)
    }) {
        // 点播的 previous 为空；prepared 已去重，不再逐片扫描刚追加的前缀。
        let mut discontinuity = item.segment.discontinuity;
        if !previous.is_empty() {
            entries.retain(|entry| {
                let replace = entry.identity == item.identity
                    || (entry.media_sequence == item.timeline_sequence
                        && (item.segment.part_index.is_none()
                            || entry.part_index == item.segment.part_index));
                if replace {
                    discontinuity |= entry.discontinuity;
                }
                !replace
            });
        }
        entries.push(TimelineEntry {
            identity: item.identity.clone(),
            uri: stable_media_url(&item.segment.uri),
            media_sequence: item.timeline_sequence,
            part_index: item.segment.part_index,
            path: item.path.clone(),
            duration: item.segment.duration.max(0.0),
            discontinuity,
            init_map_path: item.init_map_path.clone(),
        });
    }
    sort_timeline(&mut entries);
    entries
}

fn write_timeline_playlist(
    task_dir: &Path,
    target_duration: f64,
    entries: &[TimelineEntry],
    complete: bool,
) -> Result<(), String> {
    write_text_atomic(
        &task_dir.join("local.m3u8"),
        &timeline_playlist(target_duration, entries, complete),
    )
}

fn timeline_playlist(target_duration: f64, entries: &[TimelineEntry], complete: bool) -> String {
    let mut text = String::from("#EXTM3U\n#EXT-X-VERSION:9\n");
    text.push_str(if complete {
        "#EXT-X-PLAYLIST-TYPE:VOD\n"
    } else {
        "#EXT-X-PLAYLIST-TYPE:EVENT\n"
    });
    let media_sequence = entries
        .first()
        .map(|entry| entry.media_sequence)
        .unwrap_or(0);
    text.push_str(&format!(
        "#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:{media_sequence}\n",
        target_duration.max(1.0).ceil() as u64
    ));
    let mut current_map = None;
    for entry in entries {
        let map = entry
            .init_map_path
            .as_deref()
            .map(playlist_leaf)
            .filter(|leaf| !leaf.is_empty());
        if map != current_map {
            if let Some(ref map) = map {
                text.push_str(&format!("#EXT-X-MAP:URI=\"segments/{map}\"\n"));
            }
            current_map = map.clone();
        }
        if entry.discontinuity {
            text.push_str("#EXT-X-DISCONTINUITY\n");
        }
        text.push_str(&format!(
            "#EXTINF:{:.3},\nsegments/{}\n",
            entry.duration.max(0.0),
            playlist_leaf(&entry.path)
        ));
    }
    if complete {
        text.push_str("#EXT-X-ENDLIST\n");
    }
    text
}

fn check_live_control(control: &Path) -> Result<(), String> {
    match read_control(control).as_str() {
        "cancel" => Err("canceled".into()),
        "pause" => Err("paused".into()),
        "finish" => Err("finish".into()),
        _ => Ok(()),
    }
}

fn reload_playlist(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    previous: &Playlist,
    control: &Path,
) -> Result<Playlist, String> {
    check_live_control(control)?;
    let blocking_url = blocking_reload_url(url, previous);
    let mut throttle = crate::net_policy::current_throttle_context().unwrap_or_default();
    if throttle.control.as_os_str().is_empty() && !control.as_os_str().is_empty() {
        throttle.control = control.to_path_buf();
    }
    crate::net_policy::with_throttle_context(Some(throttle), || {
        match load_playlist(&blocking_url, headers, proxy) {
            Ok(playlist) => {
                check_live_control(control)?;
                if !playlist.segments.is_empty() || playlist.end_list {
                    return Ok(playlist);
                }
            }
            Err(error) if error == "paused" || error == "canceled" => {
                if read_control(control) == "cancel" {
                    return Err("canceled".into());
                }
                return Err(error);
            }
            Err(_) => {
                check_live_control(control)?;
            }
        }
        check_live_control(control)?;
        let playlist = load_playlist(url, headers, proxy)?;
        check_live_control(control)?;
        Ok(playlist)
    })
}

fn playlist_tail(playlist: &Playlist) -> Option<(u64, Option<u64>)> {
    playlist.unfiltered_tail.or_else(|| {
        playlist
            .segments
            .last()
            .map(|segment| (segment.media_sequence, segment.part_index))
    })
}

fn tail_position(tail: Option<(u64, Option<u64>)>) -> Option<(u64, u64)> {
    tail.map(|(seq, part)| (seq, part.unwrap_or(u64::MAX)))
}

fn blocking_reload_url(url: &str, previous: &Playlist) -> String {
    let (msn, part) = match playlist_tail(previous) {
        Some((seq, Some(part))) => (seq, Some(part.saturating_add(1))),
        Some((seq, None)) => (seq.saturating_add(1), None),
        None => (
            previous
                .media_sequence
                .saturating_add(previous.skipped_segments),
            None,
        ),
    };
    let mut query = vec![("_HLS_msn", msn.to_string())];
    if let Some(part) = part {
        query.push(("_HLS_part", part.to_string()));
    }
    if previous.skipped_segments > 0 {
        query.push(("_HLS_skip", "YES".to_string()));
    }
    append_query(url, &query)
}

fn append_query(url: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return url.to_string();
    }
    let (without_fragment, fragment) = url.split_once('#').unwrap_or((url, ""));
    let (base, existing) = without_fragment
        .split_once('?')
        .unwrap_or((without_fragment, ""));
    let names: BTreeSet<&str> = params.iter().map(|(key, _)| *key).collect();
    let mut query = existing
        .split('&')
        .filter(|part| {
            let name = part.split_once('=').map(|(name, _)| name).unwrap_or(part);
            !names.contains(name)
        })
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    query.extend(params.iter().map(|(key, value)| format!("{key}={value}")));
    let suffix = query.join("&");
    let separator = if suffix.is_empty() { "" } else { "?" };
    if fragment.is_empty() {
        format!("{base}{separator}{suffix}")
    } else {
        format!("{base}{separator}{suffix}#{fragment}")
    }
}

fn wait_with_control(control: &Path, seconds: f64) -> Result<(), String> {
    let total = seconds.max(0.0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(total);
    while std::time::Instant::now() < deadline {
        check_live_control(control)?;
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        std::thread::sleep(remaining.min(std::time::Duration::from_millis(100)));
    }
    check_live_control(control)
}

fn write_json_atomic(path: &Path, text: &str) -> Result<(), String> {
    write_text_atomic(path, text)
}

pub(super) fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "HLS 状态路径缺少父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "HLS 状态路径无效".to_string())?;
    let temporary = parent.join(format!(".{name}.tmp"));
    let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
    use std::io::Write;
    file.write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())?;
    drop(file);
    replace_file_atomic(&temporary, path).map_err(|error| error.to_string())
}

pub(super) fn replace_file_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    crate::atomic_replace::replace_file(from, to)
}

fn sort_timeline(entries: &mut [TimelineEntry]) {
    entries.sort_by_key(|entry| (entry.media_sequence, entry.part_index.unwrap_or(u64::MAX)));
}

fn state_file(task_dir: &Path, leaf: &str) -> Option<PathBuf> {
    if playlist_leaf(Path::new(leaf)) != leaf {
        return None;
    }
    Some(task_dir.join("segments").join(leaf))
}

fn pending_segments(playlist: &Playlist, timeline: &[TimelineEntry]) -> Vec<Segment> {
    pending_segments_at(playlist, timeline, 0)
}

fn pending_live_segments(playlist: &Playlist, timeline: &LiveTimeline) -> Vec<Segment> {
    pending_segments_at(playlist, &timeline.entries, timeline.sequence_offset)
}

fn pending_segments_at(
    playlist: &Playlist,
    timeline: &[TimelineEntry],
    sequence_offset: u64,
) -> Vec<Segment> {
    let full_in_playlist: BTreeSet<u64> = playlist
        .segments
        .iter()
        .filter(|segment| segment.part_index.is_none())
        .map(|segment| sequence_offset.saturating_add(segment.media_sequence))
        .collect();
    let full_downloaded: BTreeSet<u64> = timeline
        .iter()
        .filter(|entry| entry.part_index.is_none())
        .map(|entry| entry.media_sequence)
        .collect();
    let identities: BTreeSet<&str> = timeline
        .iter()
        .map(|entry| entry.identity.as_str())
        .collect();
    let mut positions = BTreeSet::new();
    playlist
        .segments
        .iter()
        .filter(|segment| !segment.gap)
        .filter(|segment| {
            let eff_seq = sequence_offset.saturating_add(segment.media_sequence);
            !segment.is_part
                || !full_in_playlist.contains(&eff_seq) && !full_downloaded.contains(&eff_seq)
        })
        .filter(|segment| {
            !identities.contains(live_segment_identity(segment, sequence_offset).as_str())
        })
        .filter(|segment| {
            positions.insert((
                sequence_offset.saturating_add(segment.media_sequence),
                segment.part_index,
            ))
        })
        .cloned()
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn prepare_segments(
    segments: Vec<Segment>,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    live: bool,
    sequence_offset: u64,
    vod: &VodCheckpoint,
    next_slot: &mut usize,
    key_cache: &mut HashMap<String, Vec<u8>>,
    map_cache: &mut HashMap<String, PathBuf>,
) -> Result<Vec<PreparedSegment>, String> {
    let seg_dir = task_dir.join("segments");
    let mut prepared = Vec::new();
    for segment in segments {
        match read_control(control).as_str() {
            "cancel" => return Err("canceled".into()),
            "pause" => return Err("paused".into()),
            "finish" if live => return Err("finish".into()),
            _ => {}
        }
        let slot = *next_slot;
        *next_slot = next_slot.saturating_add(1);
        let timeline_sequence = if live {
            sequence_offset.saturating_add(segment.media_sequence)
        } else {
            segment.media_sequence
        };
        let identity = if live {
            live_segment_identity(&segment, sequence_offset)
        } else {
            segment_identity(&segment)
        };
        let name = format!("{:06}.{}", slot, extension(&segment.uri));
        let output = seg_dir.join(name);
        let (path, needs_download) = if live {
            (output, true)
        } else {
            match resume_segment_path(&seg_dir, slot, &segment.uri) {
                Some(existing) if vod.can_reuse(slot, &identity, file_len(&existing)) => {
                    (existing, false)
                }
                // An on-disk segment that carries no matching checkpoint record
                // is never trusted.  A run killed between `fs::write(output, ..)`
                // and `vod.save(...)` leaves a complete-looking file behind, and
                // a playlist change reuses slot numbers, so reusing it anywhere
                // silent-splices stale or truncated bytes into the output.
                Some(existing) => {
                    let _ = fs::remove_file(existing);
                    (output, true)
                }
                None => (output, true),
            }
        };
        let init_map_path = match &segment.init_map {
            Some(init_map) => Some(ensure_init_map(
                init_map, headers, proxy, &seg_dir, control, key_cache, map_cache,
            )?),
            None => None,
        };
        let key_bytes = if needs_download {
            match &segment.key {
                Some(key) => Some(load_key(key, headers, proxy, control, key_cache)?),
                None => None,
            }
        } else {
            None
        };
        prepared.push(PreparedSegment {
            segment,
            identity,
            timeline_sequence,
            slot,
            path,
            init_map_path,
            key_bytes,
            needs_download,
        });
    }
    Ok(prepared)
}

/// VOD segment group commit: landed segments are fsynced in batches of at most
/// this many files, bytes, or age, then handed to the checkpoint callback.
const SEGMENT_COMMIT_MAX_FILES: usize = 32;
const SEGMENT_COMMIT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const SEGMENT_COMMIT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Called with segment files that were just made durable; it records them in
/// the resume checkpoint.
type DurableSegments<'a> = &'a (dyn Fn(&[PathBuf]) -> Result<(), String> + Sync);

#[derive(Default)]
pub(super) struct SegmentBatch<T = PathBuf> {
    files: Vec<T>,
    bytes: u64,
    oldest: Option<std::time::Instant>,
}

impl<T> SegmentBatch<T> {
    /// Adds a landed segment; returns the batch to commit when one is due.
    pub(super) fn push(&mut self, path: T, bytes: u64) -> Option<Vec<T>> {
        self.files.push(path);
        self.bytes = self.bytes.saturating_add(bytes);
        let oldest = *self.oldest.get_or_insert_with(std::time::Instant::now);
        let due = self.files.len() >= SEGMENT_COMMIT_MAX_FILES
            || self.bytes >= SEGMENT_COMMIT_MAX_BYTES
            || oldest.elapsed() >= SEGMENT_COMMIT_INTERVAL;
        due.then(|| self.take())
    }

    pub(super) fn remaining(&self) -> Option<std::time::Duration> {
        self.oldest
            .map(|oldest| SEGMENT_COMMIT_INTERVAL.saturating_sub(oldest.elapsed()))
    }

    pub(super) fn take(&mut self) -> Vec<T> {
        self.bytes = 0;
        self.oldest = None;
        std::mem::take(&mut self.files)
    }
}

/// Flushes already-published segment files to stable storage. On Unix the
/// containing directories are synced too, so the renames survive power loss.
pub(super) fn sync_segment_files(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|file| file.sync_data())
            .map_err(|error| format!("同步媒体分片失败 {}: {error}", path.display()))?;
    }
    #[cfg(unix)]
    {
        let parents: BTreeSet<&Path> = paths.iter().filter_map(|path| path.parent()).collect();
        for parent in parents {
            fs::File::open(parent)
                .and_then(|dir| dir.sync_all())
                .map_err(|error| format!("同步媒体分片目录失败 {}: {error}", parent.display()))?;
        }
    }
    Ok(())
}

/// Commits one batch: fsync, then the checkpoint callback. A batch that could
/// not be made durable is dropped from `completed`, so nothing vouches for it
/// and a resumed run downloads it again.
fn commit_segment_batch(
    batch: &[PathBuf],
    durable: DurableSegments<'_>,
    completed: &Mutex<HashSet<PathBuf>>,
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    if let Err(error) = sync_segment_files(batch) {
        if let Ok(mut completed) = completed.lock() {
            for path in batch {
                completed.remove(path);
            }
        }
        return Err(error);
    }
    // A failed checkpoint write is not fatal: the stop/finish path writes the
    // checkpoint again for every durable segment.
    if let Err(error) = durable(batch) {
        eprintln!("HLS segment checkpoint update failed: {error}");
    }
    Ok(())
}

/// Returns the segment files that actually landed, plus the first error.
///
/// The caller persists the VOD checkpoint for the landed segments even when the
/// run is stopping, so a segment that completed before a pause is not
/// re-downloaded on resume -- which is what the reused-segment contract needs.
///
/// With `durable` (VOD), segments are not fsynced one by one: they are synced
/// in groups ([`SEGMENT_COMMIT_MAX_FILES`] / [`SEGMENT_COMMIT_MAX_BYTES`] /
/// [`SEGMENT_COMMIT_INTERVAL`]) and each group is passed to `durable` for the
/// checkpoint. Before returning, every remaining landed segment is synced, so
/// every path in the returned set is durable. Without it (live), each segment
/// is synced before it is published, as before.
#[allow(clippy::too_many_arguments)]
fn download_prepared_segments(
    prepared: &[PreparedSegment],
    audio: &[PreparedSegment],
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    concurrency: usize,
    progress: Option<(&Path, u64, u64, u64, bool)>,
    on_completed: &(dyn Fn(&HashSet<PathBuf>) + Sync),
    durable: Option<DurableSegments<'_>>,
) -> (HashSet<PathBuf>, Option<String>) {
    let video_durations: Vec<_> = prepared.iter().map(|item| item.segment.duration).collect();
    let audio_durations: Vec<_> = audio.iter().map(|item| item.segment.duration).collect();
    let queue: VecDeque<SegmentWork> =
        super::preview::segment_order(&video_durations, &audio_durations)
            .into_iter()
            .map(|(is_audio, index)| {
                if is_audio {
                    &audio[index]
                } else {
                    &prepared[index]
                }
            })
            .filter(|item| item.needs_download)
            .map(|item| SegmentWork {
                segment: item.segment.clone(),
                path: item.path.clone(),
                key_bytes: item.key_bytes.clone(),
            })
            .collect();
    let reused = prepared
        .iter()
        .chain(audio)
        .filter(|item| !item.needs_download)
        .count() as u64;
    on_completed(&HashSet::new());
    if let Some((path, bytes, previous, total, live)) = progress {
        crate::http_engine::write_media_progress(
            path,
            bytes
                + prepared
                    .iter()
                    .chain(audio)
                    .filter(|item| !item.needs_download)
                    .map(|item| file_len(&item.path))
                    .sum::<u64>(),
            previous + reused,
            total,
            live,
        );
    }
    if queue.is_empty() {
        match read_control(control).as_str() {
            "cancel" => return (HashSet::new(), Some("canceled".into())),
            "pause" => return (HashSet::new(), Some("paused".into())),
            "finish" if durable.is_none() => return (HashSet::new(), Some("finish".into())),
            _ => return (HashSet::new(), None),
        }
    }
    let worker_count = concurrency.min(queue.len()).max(1);
    let queue = Arc::new(Mutex::new(queue));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let completed: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
    let batch = Mutex::new(SegmentBatch::default());
    let network_scope = media_network_scope();
    let replay = crate::credentials::scoped_replay_json();
    let throttle = crate::net_policy::current_throttle_context();
    // VOD 一次提交全部分片；不能等整批结束后才报告下载字节。
    let downloaded = Mutex::new((
        progress.map_or(0, |(_, bytes, _, _, _)| bytes)
            + prepared
                .iter()
                .chain(audio)
                .filter(|item| !item.needs_download)
                .map(|item| file_len(&item.path))
                .sum::<u64>(),
        reused,
    ));
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let errors = Arc::clone(&errors);
            let completed = Arc::clone(&completed);
            let replay = replay.clone();
            let network_scope = network_scope.clone();
            let throttle = throttle.clone();
            let downloaded = &downloaded;
            let batch = &batch;
            scope.spawn(move || {
                let _network = set_media_network_scope(network_scope);
                crate::net_policy::with_throttle_context(throttle, || {
                    crate::with_replay_json(&replay, || loop {
                        match read_control(control).as_str() {
                            "cancel" => {
                                if let Ok(mut queue) = queue.lock() {
                                    queue.clear();
                                }
                                if let Ok(mut errors) = errors.lock() {
                                    errors.push("canceled".into());
                                }
                                break;
                            }
                            "pause" => {
                                if let Ok(mut queue) = queue.lock() {
                                    queue.clear();
                                }
                                if let Ok(mut errors) = errors.lock() {
                                    errors.push("paused".into());
                                }
                                break;
                            }
                            "finish" if durable.is_none() => {
                                if let Ok(mut queue) = queue.lock() {
                                    queue.clear();
                                }
                                if let Ok(mut errors) = errors.lock() {
                                    errors.push("finish".into());
                                }
                                break;
                            }
                            _ => {}
                        }
                        let work = queue.lock().ok().and_then(|mut queue| {
                            if let Some(priority) =
                                control.parent().and_then(super::preview::priority_file)
                            {
                                if let Some(index) =
                                    queue.iter().position(|work| work.path == priority)
                                {
                                    queue.rotate_left(index);
                                }
                            }
                            queue.pop_front()
                        });
                        let Some(work) = work else {
                            break;
                        };
                        match download_segment(
                            &work.segment,
                            headers,
                            proxy,
                            &work.path,
                            control,
                            work.key_bytes.as_ref(),
                            work.segment.key.as_ref(),
                            work.segment.media_sequence,
                            durable.is_none(),
                        ) {
                            Ok(_) => {
                                if let Some(root) = control.parent() {
                                    if let Err(error) = super::preview::mark_ready(root, &work.path)
                                    {
                                        if let Ok(mut queue) = queue.lock() {
                                            queue.clear();
                                        }
                                        if let Ok(mut errors) = errors.lock() {
                                            errors.push(error);
                                        }
                                        break;
                                    }
                                }
                                if let Ok(mut completed) = completed.lock() {
                                    completed.insert(work.path.clone());
                                    on_completed(&completed);
                                }
                                if let Some(durable) = durable {
                                    let due = batch.lock().ok().and_then(|mut batch| {
                                        batch.push(work.path.clone(), file_len(&work.path))
                                    });
                                    if let Some(due) = due {
                                        if let Err(error) =
                                            commit_segment_batch(&due, durable, &completed)
                                        {
                                            if let Ok(mut queue) = queue.lock() {
                                                queue.clear();
                                            }
                                            if let Ok(mut errors) = errors.lock() {
                                                errors.push(error);
                                            }
                                            break;
                                        }
                                    }
                                }
                                if let Some((path, _, previous, total, live)) = progress {
                                    if let Ok(mut bytes) = downloaded.lock() {
                                        bytes.0 += file_len(&work.path);
                                        bytes.1 += 1;
                                        crate::http_engine::write_media_progress(
                                            path,
                                            bytes.0,
                                            previous + bytes.1,
                                            total,
                                            live,
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                // 已完成的分片仍正常记入 checkpoint；失败后不再派发新分片。
                                if let Ok(mut queue) = queue.lock() {
                                    queue.clear();
                                }
                                if let Ok(mut errors) = errors.lock() {
                                    errors.push(error);
                                }
                                break;
                            }
                        }
                    })
                });
            });
        }
    });
    // Pause, cancel and failure keep completed segments: sync what is still
    // pending so every returned path is durable before the caller vouches for it.
    if let Some(durable) = durable {
        let remaining = batch
            .lock()
            .map(|mut batch| batch.take())
            .unwrap_or_default();
        if let Err(error) = commit_segment_batch(&remaining, durable, &completed) {
            if let Ok(mut errors) = errors.lock() {
                errors.push(error);
            }
        }
    }
    let completed = completed
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let errors = errors.lock().map(|guard| guard.clone()).unwrap_or_default();
    if read_control(control) == "cancel" || errors.iter().any(|err| err == "canceled") {
        return (completed, Some("canceled".into()));
    }
    let mut error = errors
        .iter()
        .find(|err| *err != "paused" && *err != "finish")
        .or_else(|| errors.iter().find(|err| *err == "paused"))
        .or_else(|| errors.first())
        .cloned();
    if error.is_none() {
        match read_control(control).as_str() {
            "pause" => error = Some("paused".into()),
            "finish" if durable.is_none() => error = Some("finish".into()),
            _ => {}
        }
    }
    if error.is_none() {
        if let Some(missing) = prepared
            .iter()
            .chain(audio)
            .find(|item| file_len(&item.path) == 0)
        {
            error = Some(format!("HLS 分片下载结果为空: {}", missing.segment.uri));
        }
    }
    (completed, error)
}

fn is_retryable_key_status(status: u16) -> bool {
    retryable_http_status(status) || (500..=599).contains(&status)
}

fn is_retryable_key_error(error: &EngineError) -> bool {
    if matches!(
        error,
        EngineError::Pause | EngineError::Cancel | EngineError::RangeUnsupported(_)
    ) {
        return false;
    }
    let message = error.to_string();
    if message.contains("exceeds")
        || message.contains("长度")
        || message.contains("private network")
        || message.contains("blocked")
    {
        return false;
    }
    !extract_http_status(&message).is_some_and(|status| !is_retryable_key_status(status))
}

fn load_key(
    key: &MediaKey,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    cache: &mut HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let cache_key = key.uri.clone();
    if let Some(bytes) = cache.get(&cache_key) {
        return Ok(bytes.clone());
    }
    let mut attempt = 1u32;
    loop {
        if !control.as_os_str().is_empty() {
            check_live_control(control)?;
        }
        match fetch_hls_bytes_limited(&key.uri, headers, proxy, 16) {
            Ok((status, bytes)) if status == 200 || status == 206 => {
                if bytes.len() != 16 {
                    return Err(format!(
                        "AES-128 key 长度必须为 16 字节，实际为 {}",
                        bytes.len()
                    ));
                }
                cache.insert(cache_key, bytes.clone());
                return Ok(bytes);
            }
            Ok((status, _)) => {
                let message = format!("HLS key HTTP {status}");
                if !is_retryable_key_status(status) || attempt >= MAX_SEGMENT_ATTEMPTS {
                    return Err(message);
                }
                if !control.as_os_str().is_empty() {
                    wait_with_control(
                        control,
                        SEGMENT_RETRY_DELAY_MS as f64 * f64::from(attempt) / 1000.0,
                    )?;
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(
                        SEGMENT_RETRY_DELAY_MS * u64::from(attempt),
                    ));
                }
                attempt += 1;
            }
            Err(error) => {
                let message = error.to_string();
                if !is_retryable_key_error(&error) || attempt >= MAX_SEGMENT_ATTEMPTS {
                    return Err(message);
                }
                if !control.as_os_str().is_empty() {
                    wait_with_control(
                        control,
                        SEGMENT_RETRY_DELAY_MS as f64 * f64::from(attempt) / 1000.0,
                    )?;
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(
                        SEGMENT_RETRY_DELAY_MS * u64::from(attempt),
                    ));
                }
                attempt += 1;
            }
        }
    }
}

fn ensure_init_map(
    init_map: &InitMap,
    headers: &HashMap<String, String>,
    proxy: &str,
    seg_dir: &Path,
    control: &Path,
    key_cache: &mut HashMap<String, Vec<u8>>,
    map_cache: &mut HashMap<String, PathBuf>,
) -> Result<PathBuf, String> {
    let identity = init_map_identity(init_map);
    if let Some(path) = map_cache.get(&identity).filter(|path| file_len(path) > 0) {
        return Ok(path.clone());
    }
    let suffix = &identity[..16.min(identity.len())];
    let path = seg_dir.join(format!("init-{suffix}.{}", extension(&init_map.uri)));
    // 磁盘上的旧文件没有完成凭据，可能是上次写盘/解密失败留下的残片。
    // 仅 map_cache 中本次运行成功落盘的条目可以复用。
    let encryption = if let Some(key) = &init_map.key {
        let key_bytes = load_key(key, headers, proxy, control, key_cache)?;
        let iv = key.iv.ok_or("AES-128 HLS MAP 缺少显式 IV")?;
        Some((key_bytes, iv))
    } else {
        None
    };
    save_segment(
        &init_map.uri,
        headers,
        proxy,
        &path,
        control,
        init_map.byterange,
        encryption.as_ref().map(|(key, iv)| (key.as_slice(), *iv)),
        true,
    )
    .map_err(|error| error.to_string())?;
    map_cache.insert(identity, path.clone());
    Ok(path)
}

fn init_map_identity(init_map: &InitMap) -> String {
    let payload = serde_json::json!({
        "url": stable_media_url(&init_map.uri),
        "range": init_map.byterange,
        "key": init_map.key.as_ref().map(|key| stable_media_url(&key.uri)),
        "iv": init_map
            .key
            .as_ref()
            .and_then(|key| key.iv)
            .map(|iv| bytes_hex(&iv)),
    });
    crate::crypto_lite::sha256_hex(payload.to_string().as_bytes())
}

fn segment_identity(segment: &Segment) -> String {
    let base = vod_segment_identity(
        segment,
        segment.media_sequence,
        segment.key.as_ref(),
        segment
            .init_map
            .as_ref()
            .map(|init_map| init_map.uri.as_str()),
    );
    let map = segment
        .init_map
        .as_ref()
        .map(init_map_identity)
        .unwrap_or_default();
    crate::crypto_lite::sha256_hex(
        serde_json::json!({ "segment": base, "init_map": map })
            .to_string()
            .as_bytes(),
    )
}

fn live_segment_identity(segment: &Segment, sequence_offset: u64) -> String {
    let base = segment_identity(segment);
    if sequence_offset == 0 {
        return base;
    }
    crate::crypto_lite::sha256_hex(
        serde_json::json!({ "epoch_offset": sequence_offset, "segment": base })
            .to_string()
            .as_bytes(),
    )
}

struct LiveAudioRecorder {
    control: PathBuf,
    parent_control: PathBuf,
    finishing: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<Result<PathBuf, String>>>,
}

impl LiveAudioRecorder {
    #[cfg(test)]
    fn start(uri: &str, headers: &HashMap<String, String>, proxy: &str, task_dir: &Path) -> Self {
        let parent_control = crate::net_policy::current_throttle_context()
            .map(|ctx| ctx.control)
            .unwrap_or_default();
        Self::start_with_parent(uri, headers, proxy, task_dir, &parent_control)
    }

    fn start_with_parent(
        uri: &str,
        headers: &HashMap<String, String>,
        proxy: &str,
        task_dir: &Path,
        parent_control: &Path,
    ) -> Self {
        let control = task_dir.join("audio.control");
        let _ = crate::task_control::set(&control, "run")
            .or_else(|_| fs::write(&control, "run").map_err(|error| error.to_string()));
        let finishing = Arc::new(AtomicBool::new(false));
        let uri = uri.to_string();
        let headers = headers.clone();
        let proxy = proxy.to_string();
        let audio_dir = task_dir.join("audio");
        let thread_control = control.clone();
        let thread_parent = parent_control.to_path_buf();
        let thread_finishing = Arc::clone(&finishing);
        let network_scope = media_network_scope();
        let replay = crate::credentials::scoped_replay_json();
        let mut throttle = crate::net_policy::current_throttle_context().unwrap_or_default();
        // 保留主任务的限速/配额，但子请求须读取音轨自己的取消信号，否则 Drop 会等到网络超时。
        throttle.control = thread_control.clone();
        let handle = std::thread::spawn(move || {
            let _reg = crate::task_control::register(&thread_control);
            let _network = set_media_network_scope(network_scope);
            crate::net_policy::with_throttle_context(Some(throttle), || {
                crate::with_replay_json(&replay, || {
                    let _ = set_media_network_scope(Some(entry_network_scope(&uri, &proxy)));
                    super::with_media_headers(&uri, &headers, || {
                        download_hls_scoped_with_control(
                            &uri,
                            &headers,
                            &proxy,
                            &audio_dir,
                            LiveControl {
                                control: &thread_control,
                                parent_control: (!thread_parent.as_os_str().is_empty())
                                    .then_some(thread_parent.as_path()),
                                finishing: Some(&thread_finishing),
                            },
                            HlsDownloadOptions {
                                live: true,
                                download_subtitles: false,
                                ..HlsDownloadOptions::default()
                            },
                        )
                    })
                })
            })
        });
        Self {
            control,
            parent_control: parent_control.to_path_buf(),
            finishing,
            handle: Some(handle),
        }
    }

    fn check_parent_abort(&self) -> Option<(&'static str, &'static str)> {
        if self.parent_control.as_os_str().is_empty() {
            return None;
        }
        match read_control(&self.parent_control).as_str() {
            "cancel" => Some(("cancel", "canceled")),
            "pause" => Some(("pause", "paused")),
            _ => None,
        }
    }

    fn propagate_parent_abort(
        &self,
        handle: std::thread::JoinHandle<Result<PathBuf, String>>,
        control_word: &str,
    ) {
        self.finishing.store(false, Ordering::SeqCst);
        let _ = crate::task_control::set(&self.control, control_word)
            .or_else(|_| fs::write(&self.control, control_word).map_err(|error| error.to_string()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        while !handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }

    fn finish(&mut self) -> Result<Option<PathBuf>, String> {
        let Some(handle) = self.handle.take() else {
            return Ok(None);
        };
        if let Some((control_word, err_word)) = self.check_parent_abort() {
            self.propagate_parent_abort(handle, control_word);
            return Err(err_word.into());
        }
        self.finishing.store(true, Ordering::SeqCst);
        let _ = crate::task_control::set(&self.control, "cancel")
            .or_else(|_| fs::write(&self.control, "cancel").map_err(|error| error.to_string()));
        while !handle.is_finished() {
            if let Some((control_word, err_word)) = self.check_parent_abort() {
                self.propagate_parent_abort(handle, control_word);
                return Err(err_word.into());
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        if let Some((control_word, err_word)) = self.check_parent_abort() {
            self.finishing.store(false, Ordering::SeqCst);
            let _ = crate::task_control::set(&self.control, control_word).or_else(|_| {
                fs::write(&self.control, control_word).map_err(|error| error.to_string())
            });
            let _ = handle.join();
            return Err(err_word.into());
        }
        match handle
            .join()
            .map_err(|_| "HLS live audio worker panicked".to_string())?
        {
            Ok(path) => Ok(Some(path)),
            Err(error) if error == "未录制到任何直播分片" || error == "finish" => {
                let _ = fs::remove_dir_all(self.control.with_file_name("audio"));
                let _ = fs::remove_file(&self.control);
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for LiveAudioRecorder {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.finishing.store(false, Ordering::SeqCst);
            let stop_word = if !self.parent_control.as_os_str().is_empty()
                && read_control(&self.parent_control) == "pause"
            {
                "pause"
            } else {
                "cancel"
            };
            let _ = crate::task_control::set(&self.control, stop_word).or_else(|_| {
                fs::write(&self.control, stop_word).map_err(|error| error.to_string())
            });
            let _ = handle.join();
        }
    }
}

#[derive(Default)]
struct LiveSubSession {
    tracks: Vec<LiveSubTrack>,
}

struct LiveSubTrack {
    key: String,
    language: String,
    name: String,
    uri: String,
    seen: BTreeSet<String>,
    count: usize,
}

impl LiveSubSession {
    fn load(task_dir: &Path, tracks: &[Rendition]) -> Self {
        let mut session = Self::default();
        let saved = fs::read_to_string(task_dir.join("live_subtitles.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        for track in tracks {
            let Some(uri) = &track.uri else {
                continue;
            };
            let key = live_subtitle_key(track);
            let mut item = LiveSubTrack {
                key: key.clone(),
                language: track.language.clone(),
                name: track.name.clone(),
                uri: uri.clone(),
                seen: BTreeSet::new(),
                count: 0,
            };
            if let Some(saved_tracks) = saved
                .as_ref()
                .and_then(|value| value.get("tracks"))
                .and_then(|value| value.as_array())
            {
                if let Some(found) = saved_tracks.iter().find(|entry| {
                    entry.get("uri").and_then(|value| value.as_str()) == Some(uri.as_str())
                        || entry.get("key").and_then(|value| value.as_str()) == Some(key.as_str())
                }) {
                    item.count = found
                        .get("count")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as usize;
                    if let Some(seen) = found.get("seen").and_then(|value| value.as_array()) {
                        item.seen = seen
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_string))
                            .collect();
                    }
                }
            }
            session.tracks.push(item);
        }
        session
    }

    fn capture(
        &mut self,
        headers: &HashMap<String, String>,
        proxy: &str,
        task_dir: &Path,
        control: &Path,
    ) {
        for track in &mut self.tracks {
            let _ = capture_live_subtitle_track(track, headers, proxy, task_dir, control);
        }
        self.save(task_dir);
    }

    fn save(&self, task_dir: &Path) {
        let tracks: Vec<serde_json::Value> = self
            .tracks
            .iter()
            .map(|track| {
                serde_json::json!({
                    "key": track.key,
                    "language": track.language,
                    "name": track.name,
                    "uri": track.uri,
                    "count": track.count,
                    "seen": track.seen.iter().cloned().collect::<Vec<_>>(),
                })
            })
            .collect();
        let _ = write_json_atomic(
            &task_dir.join("live_subtitles.json"),
            &serde_json::json!({ "tracks": tracks }).to_string(),
        );
    }

    fn publish(&self, task_dir: &Path) -> Vec<PathBuf> {
        let sub_dir = task_dir.join("subs");
        let _ = fs::create_dir_all(&sub_dir);
        let mut published = Vec::new();
        let mut used = BTreeSet::new();
        for track in &self.tracks {
            let cache = task_dir.join("live-subtitles").join(&track.key);
            let mut texts = Vec::new();
            for index in 0..track.count {
                let path = cache.join(format!("{index:08}.vtt"));
                if let Ok(text) = fs::read_to_string(path) {
                    texts.push(text);
                }
            }
            let merged = merge_webvtt_segments(&texts);
            if !has_cues(&merged) {
                continue;
            }
            let mut label = if !track.language.is_empty() {
                safe_label(&track.language, "und")
            } else if !track.name.is_empty() {
                safe_label(&track.name, "und")
            } else {
                safe_label(&track.key, "und")
            };
            let base = label.clone();
            let mut suffix = 2;
            while !used.insert(label.to_ascii_lowercase()) {
                label = format!("{base}-{suffix}");
                suffix += 1;
            }
            let vtt = sub_dir.join(format!("{label}.vtt"));
            if fs::write(&vtt, &merged).is_ok() {
                let _ = fs::write(vtt.with_extension("srt"), webvtt_to_srt(&merged));
                published.push(vtt);
            }
        }
        published
    }
}

fn live_subtitle_key(track: &Rendition) -> String {
    let raw = if !track.language.is_empty() {
        track.language.as_str()
    } else if !track.name.is_empty() {
        track.name.as_str()
    } else {
        track.uri.as_deref().unwrap_or("sub")
    };
    let base = safe_label(raw, "sub");
    let stable = track
        .uri
        .as_deref()
        .map(stable_media_url)
        .unwrap_or_default();
    if stable.is_empty() {
        return base;
    }
    let digest = crate::crypto_lite::sha256_hex(stable.as_bytes());
    format!("{base}-{}", &digest[..8])
}

fn safe_label(raw: &str, fallback: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches(|ch| ch == '_' || ch == '.' || ch == '-');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn capture_live_subtitle_track(
    track: &mut LiveSubTrack,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
) -> Result<(), String> {
    let (status, body, final_url) =
        fetch_manifest_bytes(&track.uri, headers, proxy, true, super::MAX_MANIFEST_BYTES)
            .map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Ok(());
    }
    let cache = task_dir.join("live-subtitles").join(&track.key);
    fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    let text = std::str::from_utf8(&body).map_err(|_| "HLS 文本不是 UTF-8")?;
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("WEBVTT") {
        let identity = crate::crypto_lite::sha256_hex(&body);
        if !track.seen.contains(&identity) {
            let path = cache.join(format!("{:08}.vtt", track.count));
            write_text_atomic(&path, text)?;
            track.seen.insert(identity);
            track.count += 1;
        }
        return Ok(());
    }
    let playlist = match parse_playlist(text, &final_url) {
        Ok(parsed) if !parsed.is_master => parsed,
        _ => return Ok(()),
    };
    let mut keys = HashMap::new();
    for segment in playlist.segments {
        if segment.gap {
            continue;
        }
        let identity = segment_identity(&segment);
        if !track.seen.insert(identity.clone()) {
            continue;
        }
        let key = match segment
            .key
            .as_ref()
            .map(|key| load_key(key, headers, proxy, control, &mut keys))
            .transpose()
        {
            Ok(key) => key,
            Err(_) => {
                track.seen.remove(&identity);
                continue;
            }
        };
        let path = cache.join(format!("{:08}.vtt", track.count));
        if download_segment(
            &segment,
            headers,
            proxy,
            &path,
            control,
            key.as_ref(),
            segment.key.as_ref(),
            segment.media_sequence,
            true,
        )
        .is_ok()
            && file_len(&path) > 0
        {
            track.count += 1;
        } else {
            track.seen.remove(&identity);
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

// 字幕清单必须按 WebVTT 合并，不能递归进入需要音视频流的 MP4 合流路径。
fn download_vod_subtitle(
    uri: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    output: &Path,
    control: &Path,
) -> Result<(), String> {
    match read_control(control).as_str() {
        "cancel" => return Err("canceled".into()),
        "pause" => return Err("paused".into()),
        _ => {}
    }
    let (status, body, final_url) =
        fetch_manifest_bytes(uri, headers, proxy, true, super::MAX_MANIFEST_BYTES)
            .map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Err(format!("HLS subtitle HTTP {status}"));
    }
    let text = std::str::from_utf8(&body).map_err(|_| "HLS 文本不是 UTF-8")?;
    let merged = if text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("WEBVTT")
    {
        text.to_owned()
    } else {
        let playlist = parse_playlist(text, &final_url)?;
        if playlist.is_master {
            return Err("HLS subtitle rendition must reference a media playlist".into());
        }
        let cache = output.with_extension("segments");
        fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
        let mut keys = HashMap::new();
        let mut texts = Vec::new();
        for (index, segment) in playlist.segments.iter().enumerate().filter(|(_, s)| !s.gap) {
            let key = segment
                .key
                .as_ref()
                .map(|key| load_key(key, headers, proxy, control, &mut keys))
                .transpose()?;
            let path = cache.join(format!("{index:08}.vtt"));
            download_segment(
                segment,
                headers,
                proxy,
                &path,
                control,
                key.as_ref(),
                segment.key.as_ref(),
                segment.media_sequence,
                true,
            )?;
            texts.push(fs::read_to_string(path).map_err(|error| error.to_string())?);
        }
        merge_webvtt_segments(&texts)
    };
    if !has_cues(&merged) {
        return Err("HLS subtitle contains no WebVTT cues".into());
    }
    match read_control(control).as_str() {
        "cancel" => return Err("canceled".into()),
        "pause" => return Err("paused".into()),
        _ => {}
    }
    fs::write(output, &merged).map_err(|error| error.to_string())?;
    fs::write(output.with_extension("srt"), webvtt_to_srt(&merged))
        .map_err(|error| error.to_string())
}

fn load_playlist(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
) -> Result<Playlist, String> {
    let (status, body, final_url) =
        fetch_manifest_bytes(url, headers, proxy, true, super::MAX_MANIFEST_BYTES)
            .map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Err(format!("HLS playlist HTTP {status}"));
    }
    let text = std::str::from_utf8(&body).map_err(|_| "HLS 文本不是 UTF-8")?;
    parse_playlist(text, &final_url)
}

// 只重试可恢复的网络失败；解密和写盘失败不应重新请求相同资源。
const MAX_SEGMENT_ATTEMPTS: u32 = 3;
const SEGMENT_RETRY_DELAY_MS: u64 = 250;

pub(super) fn is_retryable_segment_error(error: &MediaTransferError) -> bool {
    match error {
        MediaTransferError::Output(_) => false,
        MediaTransferError::Transfer(error) => {
            if matches!(
                error,
                EngineError::Pause | EngineError::Cancel | EngineError::RangeUnsupported(_)
            ) {
                return false;
            }
            let message = error.to_string();
            !extract_http_status(&message).is_some_and(|status| !retryable_http_status(status))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn download_segment(
    segment: &Segment,
    headers: &HashMap<String, String>,
    proxy: &str,
    path: &Path,
    control: &Path,
    key_bytes: Option<&Vec<u8>>,
    media_key: Option<&MediaKey>,
    sequence: u64,
    sync: bool,
) -> Result<PathBuf, String> {
    let encryption = key_bytes
        .zip(media_key)
        .filter(|(_, key)| key.method.eq_ignore_ascii_case("AES-128"))
        .map(|(key, media_key)| {
            (
                key.as_slice(),
                media_key.iv.unwrap_or_else(|| sequence_iv(sequence)),
            )
        });
    let mut attempt = 1;
    loop {
        match save_segment(
            &segment.uri,
            headers,
            proxy,
            path,
            control,
            segment.byterange,
            encryption,
            sync,
        ) {
            Ok(path) => return Ok(path),
            Err(error) => {
                let message = error.to_string();
                if !is_retryable_segment_error(&error) {
                    return Err(message);
                }
                if attempt == MAX_SEGMENT_ATTEMPTS {
                    return Err(format!("{message}（已重试 {} 次）", attempt - 1));
                }
                wait_with_control(
                    control,
                    SEGMENT_RETRY_DELAY_MS as f64 * attempt as f64 / 1000.0,
                )?;
                attempt += 1;
            }
        }
    }
}

fn file_len(path: &Path) -> u64 {
    path.metadata().map(|meta| meta.len()).unwrap_or(0)
}

struct VodCheckpoint {
    records: HashMap<String, (String, u64)>,
}

impl VodCheckpoint {
    fn load(task_dir: &Path) -> Self {
        let Ok(text) = fs::read_to_string(task_dir.join("vod_segments.json")) else {
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
        }
        let mut records = HashMap::new();
        if let Some(map) = value.get("segments").and_then(|item| item.as_object()) {
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

    // Test-only helper: it distinguishes "no record at all" from "a record that
    // belongs to a different segment", which `can_reuse` deliberately reports the
    // same way.
    #[cfg(test)]
    fn has_slot(&self, slot: usize) -> bool {
        self.records.contains_key(&slot.to_string())
    }

    fn can_reuse(&self, slot: usize, identity: &str, size: u64) -> bool {
        if size == 0 {
            return false;
        }
        match self.records.get(&slot.to_string()) {
            Some((saved, saved_size)) => saved == identity && *saved_size == size,
            // No record means nothing vouches for this file, which is not the
            // same as "it is fine".
            None => false,
        }
    }

    /// Drops the record of a slot that is about to be downloaded again.
    fn forget(&mut self, slot: usize) -> bool {
        self.records.remove(&slot.to_string()).is_some()
    }

    fn remember(&mut self, slot: usize, identity: &str, size: u64) {
        if identity.is_empty() || size == 0 {
            return;
        }
        self.records
            .insert(slot.to_string(), (identity.to_string(), size));
    }

    fn save(&self, task_dir: &Path) -> Result<(), String> {
        let mut segments = serde_json::Map::new();
        for (slot, (identity, size)) in &self.records {
            segments.insert(
                slot.clone(),
                serde_json::json!({ "identity": identity, "size": size }),
            );
        }
        let payload = serde_json::json!({
            "version": 1,
            "segments": segments,
        });
        write_text_atomic(&task_dir.join("vod_segments.json"), &payload.to_string())
    }
}

pub(crate) fn vod_segment_identity(
    segment: &Segment,
    sequence: u64,
    key: Option<&MediaKey>,
    map_uri: Option<&str>,
) -> String {
    let range = match segment.byterange {
        Some((offset, length)) => serde_json::json!([offset, length]),
        None => serde_json::Value::Null,
    };
    let payload = serde_json::json!({
        "discontinuity": segment.discontinuity,
        "duration": (segment.duration * 1_000_000.0).round() / 1_000_000.0,
        "init": stable_media_url(map_uri.unwrap_or("")),
        "key": stable_media_url(key.map(|item| item.uri.as_str()).unwrap_or("")),
        "key_method": key.map(|item| item.method.as_str()),
        "iv": key.map(|item| bytes_hex(&item.iv.unwrap_or_else(|| sequence_iv(sequence)))),
        "part_index": segment.part_index,
        "range": range,
        "sequence": sequence,
        "url": stable_media_url(&segment.uri),
    });
    crate::crypto_lite::sha256_hex(payload.to_string().as_bytes())
}

fn bytes_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

pub(crate) fn stable_media_url(uri: &str) -> String {
    let uri = uri.split('#').next().unwrap_or(uri);
    let (left, query) = uri.split_once('?').unwrap_or((uri, ""));
    let origin = crate::http_engine::request_origin(uri);
    let path = if let Some(idx) = left.find("://").filter(|_| !origin.is_empty()) {
        let rest = &left[idx + 3..];
        rest.find('/')
            .map(|at| rest[at..].trim_end_matches('/'))
            .unwrap_or("")
    } else {
        left.trim_end_matches('/')
    };
    let path = if path.is_empty() { "/" } else { path };
    let stable_path = format!("{origin}{path}");
    let mut names = Vec::new();
    let mut pairs = Vec::new();
    for part in query.split('&') {
        if part.is_empty() {
            continue;
        }
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        names.push(name.to_ascii_lowercase());
        pairs.push((name.to_string(), value.to_string()));
    }
    let short_sig = names.iter().any(|name| name == "s") && names.iter().any(|name| name == "e");
    pairs.retain(|(name, _)| !volatile_query_name(name, short_sig));
    pairs.sort_by(|left, right| {
        left.0
            .to_ascii_lowercase()
            .cmp(&right.0.to_ascii_lowercase())
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });
    let query = pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    if query.is_empty() {
        stable_path
    } else {
        format!("{stable_path}?{query}")
    }
}

fn volatile_query_name(name: &str, short_sig: bool) -> bool {
    let name = name.to_ascii_lowercase();
    if name.starts_with("x-amz-") {
        return true;
    }
    if short_sig && matches!(name.as_str(), "s" | "e" | "_t") {
        return true;
    }
    matches!(
        name.as_str(),
        "token"
            | "auth"
            | "authorization"
            | "signature"
            | "sig"
            | "expires"
            | "expire"
            | "expiry"
            | "policy"
            | "key-pair-id"
            | "hdnea"
            | "hmac"
            | "jwt"
            | "session"
            | "sessionid"
            | "access_key"
            | "access-key"
            | "_hls_msn"
            | "_hls_part"
            | "_hls_skip"
    )
}

/// Downloads one segment into a temporary file and publishes it at `output`.
///
/// With `sync` the payload is flushed to stable storage before the rename
/// (init maps, subtitles, live segments). Without it the data is only handed
/// to the OS; the caller must make the file durable through
/// [`sync_segment_files`] before any checkpoint vouches for it (VOD group
/// commit).
#[allow(clippy::too_many_arguments)]
pub(super) fn save_segment(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    output: &Path,
    control: &Path,
    byterange: Option<(u64, u64)>,
    encryption: Option<(&[u8], [u8; 16])>,
    sync: bool,
) -> Result<PathBuf, MediaTransferError> {
    match read_control(control).as_str() {
        "cancel" => return Err(EngineError::Cancel.into()),
        "pause" => return Err(EngineError::Pause.into()),
        _ => {}
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = output
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "HLS 分片路径无效"))?;
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(".part");
    let temporary = output.with_file_name(temporary_name);
    let result = (|| {
        let job = Job {
            url: url.to_string(),
            headers: headers.clone(),
            // HLS owns completion and filenames; no per-response HTTP sidecar.
            output: PathBuf::new(),
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
            progress: PathBuf::new(),
            method: "GET".into(),
            body_path: PathBuf::new(),
            mirrors: Vec::new(),
            replay_json: crate::credentials::scoped_replay_json(),
            curl_body_limit: byterange.map(|(_, length)| length),
            range_stop: None,
            installer_download: false,
            origin_boundary: String::new(),
        };
        let mut writer = BufWriter::with_capacity(1024 * 1024, fs::File::create(&temporary)?);
        if let Some((key, iv)) = encryption {
            let mut cipher = crate::crypto_lite::Aes128CbcWriter::new(&mut writer, key, &iv)?;
            copy_hls_body(&job, byterange, &mut cipher)?;
            cipher.finish()?;
        } else {
            copy_hls_body(&job, byterange, &mut writer)?;
        }
        writer.flush()?;
        if writer.get_ref().metadata()?.len() == 0 {
            return Err(
                std::io::Error::new(std::io::ErrorKind::InvalidData, "HLS 媒体段为空").into(),
            );
        }
        if sync {
            writer.get_ref().sync_data()?;
        }
        drop(writer);
        match read_control(control).as_str() {
            "pause" => return Err(EngineError::Pause.into()),
            "cancel" => return Err(EngineError::Cancel.into()),
            _ => {}
        }
        replace_file_atomic(&temporary, output)?;
        Ok(output.to_path_buf())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
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

fn sequence_iv(sequence: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&sequence.to_be_bytes());
    iv
}

fn parse_iv(value: &str) -> Result<[u8; 16], String> {
    let mut hex = value
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_string();
    if hex.is_empty() || hex.len() > 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("无效 AES-128 IV: {value}"));
    }
    if !hex.len().is_multiple_of(2) {
        hex.insert(0, '0');
    }
    let padded = format!("{hex:0>32}");
    let mut iv = [0u8; 16];
    for (index, chunk) in padded.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|error| error.to_string())?;
        iv[index] = u8::from_str_radix(text, 16).map_err(|error| error.to_string())?;
    }
    Ok(iv)
}

fn apply_ad_policy(playlist: &mut Playlist, enabled: bool) {
    if !enabled {
        return;
    }
    if playlist.segments.iter().any(|segment| segment.is_ad) {
        playlist.unfiltered_tail = playlist
            .segments
            .last()
            .map(|segment| (segment.media_sequence, segment.part_index));
    }
    let mut kept = Vec::new();
    let mut pending_gap = false;
    let mut pending_part_gap = false;
    for mut segment in playlist.segments.drain(..) {
        if segment.is_ad {
            pending_gap = true;
            pending_part_gap = true;
            continue;
        }
        if segment.is_part {
            if pending_part_gap {
                segment.discontinuity = true;
                pending_part_gap = false;
            }
        } else if pending_gap {
            segment.discontinuity = true;
            pending_gap = false;
            pending_part_gap = false;
        }
        kept.push(segment);
    }
    playlist.segments = kept;
}

fn url_is_ad(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    [
        "ads",
        "advert",
        "advertisement",
        "preroll",
        "midroll",
        "postroll",
        "promo",
    ]
    .iter()
    .any(|token| {
        path.split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|part| part == *token)
    })
}

fn daterange_is_ad(attrs: &str) -> bool {
    let blob = attrs.to_ascii_lowercase();
    [
        "scte35",
        "splice",
        "preroll",
        "midroll",
        "postroll",
        "com.apple.hls.interstitial",
    ]
    .iter()
    .any(|marker| blob.contains(marker))
        || blob.contains("class=\"ad")
        || blob.contains("class=ad")
}

fn parse_byterange(value: &str) -> Result<(Option<u64>, u64), String> {
    let mut parts = value.split('@');
    let length = parts
        .next()
        .and_then(|item| item.trim().parse::<u64>().ok())
        .filter(|length| *length > 0)
        .ok_or_else(|| format!("无效 BYTERANGE: {value}"))?;
    let offset = match parts.next() {
        Some(item) => Some(
            item.trim()
                .parse::<u64>()
                .map_err(|_| format!("无效 BYTERANGE: {value}"))?,
        ),
        None => None,
    };
    if parts.next().is_some() {
        return Err(format!("无效 BYTERANGE: {value}"));
    }
    Ok((offset, length))
}

fn resolve_byterange(
    range: (Option<u64>, u64),
    uri: &str,
    previous_ends: &mut HashMap<String, u64>,
) -> Result<(u64, u64), String> {
    let (offset, length) = range;
    let offset = offset
        .or_else(|| previous_ends.get(uri).copied())
        .ok_or_else(|| "BYTERANGE 缺少起始偏移".to_string())?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| "BYTERANGE 超出范围".to_string())?;
    // An implicit offset follows the immediately preceding range of this kind.
    previous_ends.clear();
    previous_ends.insert(uri.to_string(), end);
    Ok((offset, length))
}

fn tag_value<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)
}

fn attr_str(attrs: &str, key: &str) -> Option<String> {
    let mut start = 0;
    let mut quoted = false;
    for (index, ch) in attrs.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if ch == ',' && !quoted {
            if let Some(value) = attr_part_value(&attrs[start..index], key) {
                return Some(value);
            }
            start = index + ch.len_utf8();
        }
    }
    if let Some(value) = attr_part_value(&attrs[start..], key) {
        return Some(value);
    }
    None
}

fn attr_part_value(part: &str, key: &str) -> Option<String> {
    let (name, value) = part.split_once('=')?;
    if name.trim() != key {
        return None;
    }
    Some(value.trim().trim_matches('"').to_string())
}

fn attr_u64(attrs: &str, key: &str) -> Option<u64> {
    attr_str(attrs, key)?.parse().ok()
}

fn charge_segment(
    remaining: &mut usize,
    uri: &str,
    key: Option<&MediaKey>,
    map: Option<&InitMap>,
) -> Result<(), String> {
    let key_size = |key: &MediaKey| key.uri.len() + key.method.len();
    let bytes = std::mem::size_of::<Segment>()
        + uri.len()
        + key.map_or(0, key_size)
        + map.map_or(0, |map| {
            map.uri.len() + map.key.as_ref().map_or(0, key_size)
        });
    *remaining = remaining
        .checked_sub(bytes)
        .ok_or("HLS 清单展开超过长度上限")?;
    Ok(())
}

fn parse_hls_duration(value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|duration| duration.is_finite() && *duration >= 0.0)
        .ok_or_else(|| "无效 HLS 时长".into())
}

fn parse_resolution(value: &str) -> (u32, u32) {
    let Some((width, height)) = value.split_once('x') else {
        return (0, 0);
    };
    (width.parse().unwrap_or(0), height.parse().unwrap_or(0))
}

pub(crate) fn resume_segment_path(
    seg_dir: &Path,
    index: usize,
    uri: &str,
) -> Option<std::path::PathBuf> {
    let path = seg_dir.join(format!("{:06}.{}", index, extension(uri)));
    (path.is_file() && path.metadata().map(|meta| meta.len() > 0).unwrap_or(false)).then_some(path)
}

fn extension(uri: &str) -> &str {
    Path::new(uri.split(['?', '#']).next().unwrap_or(uri))
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| {
            !ext.is_empty() && ext.len() <= 12 && ext.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        .unwrap_or("bin")
}

fn read_control(path: &Path) -> String {
    // 运行任务读进程内令牌，未注册路径仍读控制文件（行为不变）。
    crate::task_control::read_word(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn redirected_playlist_resolves_children_without_replaying_entry_credentials() {
        use crate::http_engine::serve_redirect_test_responses;
        let manifest = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"aes.key\"\n#EXTINF:1,\nvideo.ts\n#EXT-X-ENDLIST\n";
        let (cdn, cdn_server) = serve_redirect_test_responses(vec![
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{manifest}",
                manifest.len()
            ),
            "HTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\n0123456789abcdef".into(),
        ]);
        let (entry, entry_server) = serve_redirect_test_responses(vec![format!(
            "HTTP/1.1 302 Found\r\nLocation: {cdn}/vod/rendition.m3u8\r\nContent-Length: 0\r\n\r\n"
        )]);
        let url = format!("{entry}/entry.m3u8?token=entry-only");
        let headers = HashMap::from([("Authorization".into(), "Bearer entry-only".into())]);
        crate::media::with_media_headers(&url, &headers, || {
            let playlist = load_playlist(&url, &headers, "").unwrap();
            let segment = &playlist.segments[0];
            assert_eq!(segment.uri, format!("{cdn}/vod/video.ts"));
            let key = segment.key.as_ref().unwrap();
            assert_eq!(key.uri, format!("{cdn}/vod/aes.key"));
            assert_eq!(
                load_key(key, &headers, "", Path::new(""), &mut HashMap::new()).unwrap(),
                b"0123456789abcdef"
            );
        });
        assert!(entry_server.join().unwrap()[0].contains("Bearer entry-only"));
        let requests = cdn_server.join().unwrap();
        assert!(requests[0].starts_with("GET /vod/rendition.m3u8 "));
        assert!(requests[1].starts_with("GET /vod/aes.key "));
        assert!(requests.iter().all(|head| !head.contains("entry-only")));
    }

    #[test]
    fn public_playlist_rejects_private_children() {
        for line in [
            "//127.0.0.1/video.ts",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"http://169.254.169.254/key\"",
            "#EXT-X-MAP:URI=\"http://[::1]/init\"",
            "#EXT-X-PART:DURATION=1,URI=\"http://[::ffff:10.0.0.1]/part\"",
            "#EXT-X-MEDIA:TYPE=AUDIO,URI=\"http://localhost/audio.m3u8\"",
        ] {
            assert!(
                parse_playlist(&format!("#EXTM3U\n{line}"), "https://cdn.example/live.m3u8")
                    .unwrap_err()
                    .contains("private network"),
                "{line}"
            );
        }
    }

    #[test]
    fn segment_budget_accounts_for_inherited_map_keys() {
        let key = MediaKey {
            method: "AES-128".into(),
            uri: "k".repeat(512),
            iv: Some([0; 16]),
        };
        let map = InitMap {
            uri: "init.mp4".into(),
            key: Some(key.clone()),
            byterange: None,
        };
        let mut budget = std::mem::size_of::<Segment>() + 1_024;
        assert!(charge_segment(&mut budget, "s", None, Some(&map)).is_ok());
        assert!(charge_segment(&mut budget, "s", Some(&key), Some(&map)).is_err());
    }

    #[test]
    fn rejects_invalid_hls_durations_and_ambiguous_maps() {
        let base = "https://cdn.test/live.m3u8";
        for tags in [
            "#EXTINF:NaN,",
            "#EXT-X-TARGETDURATION:inf",
            "#EXT-X-PART:DURATION=-1,URI=\"p.m4s\"",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXT-X-MAP:URI=\"init.mp4\"",
            "#EXT-X-MAP:URI=\"m\",BYTERANGE=\"4@0\"\n#EXT-X-MAP:URI=\"m\"\n#EXT-X-MAP:URI=\"m\",BYTERANGE=\"4\"",
        ] {
            assert!(super::parse_playlist(&format!("#EXTM3U\n{tags}\ns.ts"), base).is_err());
        }
    }

    use super::*;
    use crate::http_engine::fetch_hls_bytes;
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Condvar;
    use std::time::{Duration, Instant};

    #[cfg(windows)]
    #[test]
    fn media_checkpoints_replace_in_deep_unicode_directories() {
        let root = std::env::temp_dir().join(format!("hls-long-media-{}", std::process::id()));
        let mut directory = root.clone();
        for _ in 0..6 {
            directory.push(format!("deep-{}", "测".repeat(35)));
        }
        let path = directory.join("checkpoint.json");
        write_text_atomic(&path, "first").unwrap();
        write_text_atomic(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "second");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_aes_key_reports_http_status_instead_of_body_limit() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let key = MediaKey {
            method: "AES-128".into(),
            uri: format!("http://{}/key", listener.local_addr().unwrap()),
            iv: None,
        };
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let _ = reader.get_mut().write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 23\r\nConnection: close\r\n\r\n<html>missing key</html>");
        });
        let error = load_key(
            &key,
            &HashMap::new(),
            "",
            Path::new(""),
            &mut HashMap::new(),
        )
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error, "HLS key HTTP 404");
    }

    #[test]
    fn playlist_requires_header_before_segment_or_document_content() {
        for text in [
            "<html>#EXTM3U\n#EXTINF:2,\nsegment.ts\n</html>",
            "#EXTM3UX\n#EXTINF:2,\nsegment.ts",
            "#EXTM3U extra\n#EXTINF:2,\nsegment.ts",
            "# error page containing #EXTM3U\nsegment.ts",
            "segment.ts\n#EXTM3U",
        ] {
            assert!(
                parse_playlist(text, "https://cdn.test/live/").is_err(),
                "{text}"
            );
        }
        let playlist = parse_playlist(
            "\u{feff} \r\n  #EXTM3U\r\n#EXTINF:2,\nsegment.ts\n#EXT-X-ENDLIST",
            "https://cdn.test/live/",
        )
        .unwrap();
        assert_eq!(playlist.segments.len(), 1);
        assert_eq!(playlist.segments[0].uri, "https://cdn.test/live/segment.ts");
        assert!(playlist.end_list);
    }

    #[test]
    fn init_map_failure_never_becomes_a_reusable_cache_entry() {
        let root = std::env::temp_dir().join(format!("hls-init-cache-{}", std::process::id()));
        let segments = root.join("segments");
        fs::create_dir_all(&segments).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = BufReader::new(&mut stream);
                let mut line = String::new();
                loop {
                    line.clear();
                    assert!(request.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                drop(request);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\nnot-valid-ciphertext").unwrap();
            }
        });
        let mut map = InitMap {
            uri: format!("http://{address}/init.mp4"),
            byterange: None,
            key: Some(MediaKey {
                method: "AES-128".into(),
                uri: "key".into(),
                iv: Some([0; 16]),
            }),
        };
        let key = map.key.as_ref().unwrap();
        let mut keys = HashMap::from([(key.uri.clone(), vec![0; 16])]);
        let mut maps = HashMap::new();
        let identity = init_map_identity(&map);
        let path = segments.join(format!("init-{}.mp4", &identity[..16]));
        fs::write(&path, b"stale partial file").unwrap();
        for _ in 0..2 {
            assert!(ensure_init_map(
                &map,
                &HashMap::new(),
                "",
                &segments,
                &control,
                &mut keys,
                &mut maps
            )
            .is_err());
            assert!(maps.is_empty());
        }
        map.key = None;
        let identity = init_map_identity(&map);
        let path = segments.join(format!("init-{}.mp4", &identity[..16]));
        fs::write(&path, b"stale partial file").unwrap();
        let fresh = ensure_init_map(
            &map,
            &HashMap::new(),
            "",
            &segments,
            &control,
            &mut keys,
            &mut maps,
        )
        .unwrap();
        assert_eq!(fs::read(&fresh).unwrap(), b"not-valid-ciphertext");
        fs::write(&control, "pause").unwrap();
        assert_eq!(
            ensure_init_map(
                &map,
                &HashMap::new(),
                "",
                &segments,
                &control,
                &mut keys,
                &mut maps
            )
            .unwrap(),
            fresh
        );
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vod_identity_tracks_effective_encryption_iv_and_method() {
        let playlist = parse_playlist(
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:1,\nseg.ts\n#EXT-X-ENDLIST\n",
            "https://cdn.test/list.m3u8",
        )
        .unwrap();
        let segment = &playlist.segments[0];
        let mut key = segment.key.clone().unwrap();
        let implicit = vod_segment_identity(segment, 7, Some(&key), None);
        key.iv = Some(sequence_iv(7));
        assert_eq!(implicit, vod_segment_identity(segment, 7, Some(&key), None));
        key.iv = Some(sequence_iv(8));
        assert_ne!(implicit, vod_segment_identity(segment, 7, Some(&key), None));
        key.iv = Some(sequence_iv(7));
        key.method = "NONE".into();
        assert_ne!(implicit, vod_segment_identity(segment, 7, Some(&key), None));
    }

    #[test]
    fn segment_streams_before_eof_and_preserves_old_file_when_stopped() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        use std::sync::mpsc;
        for ranged in [false, true] {
            for action in ["pause", "cancel"] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let address = listener.local_addr().unwrap();
                let root = std::env::temp_dir().join(format!("hls-stream-stop-{}", address.port()));
                fs::create_dir_all(&root).unwrap();
                let output = root.join("segment.ts");
                let staged = root.join(".segment.ts.part");
                let control = root.join("control");
                fs::write(&control, "run").unwrap();
                fs::write(&output, b"previous completed segment").unwrap();
                let (ready_tx, ready_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let server = std::thread::spawn(move || {
                    let (stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                            break;
                        }
                    }
                    let headers = if ranged {
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-4194303/4194304\r\nContent-Length: 4194304\r\n\r\n"
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Length: 4194304\r\n\r\n"
                    };
                    reader.get_mut().write_all(headers.as_bytes()).unwrap();
                    reader
                        .get_mut()
                        .write_all(&vec![7; 2 * 1024 * 1024])
                        .unwrap();
                    ready_tx.send(()).unwrap();
                    let _ = release_rx.recv_timeout(Duration::from_secs(10));
                });
                let mut segment = parse_playlist(
                    "#EXTM3U\n#EXTINF:1,\nsegment.ts\n",
                    &format!("http://{address}/index.m3u8"),
                )
                .unwrap()
                .segments
                .remove(0);
                segment.byterange = ranged.then_some((0, 4 * 1024 * 1024));
                let worker_output = output.clone();
                let worker_control = control.clone();
                let (result_tx, result_rx) = mpsc::channel();
                let worker = std::thread::spawn(move || {
                    result_tx
                        .send(download_segment(
                            &segment,
                            &HashMap::new(),
                            "",
                            &worker_output,
                            &worker_control,
                            None,
                            None,
                            0,
                            true,
                        ))
                        .unwrap();
                });
                let ready = ready_rx.recv_timeout(Duration::from_secs(5));
                let deadline = Instant::now() + Duration::from_secs(3);
                while file_len(&staged) == 0 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let streamed = file_len(&staged) > 0;
                fs::write(&control, action).unwrap();
                let stopped = result_rx.recv_timeout(Duration::from_secs(2));
                let _ = release_tx.send(());
                server.join().unwrap();
                worker.join().unwrap();
                let saved = fs::read(&output).unwrap();
                let leftover = staged.exists();
                let sidecar = fs::read_dir(&root).unwrap().flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .ends_with("response.json")
                });
                fs::remove_dir_all(root).unwrap();
                ready.unwrap();
                assert!(streamed, "data must reach provisional output before EOF");
                assert_eq!(
                    stopped.unwrap().unwrap_err(),
                    if action == "pause" {
                        "paused"
                    } else {
                        "canceled"
                    }
                );
                assert_eq!(saved, b"previous completed segment");
                assert!(!leftover);
                assert!(!sidecar);
            }
        }
    }

    #[test]
    #[ignore = "Explicit five-run local transport benchmark; not a media decode test"]
    fn hls_segment_transfer_benchmark() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let root = std::env::temp_dir().join(format!("hls-segment-bench-{}", address.port()));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let block: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
        let sent = block.clone();
        let total = 32 * 1024 * 1024;
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            let stream = reader.get_mut();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            for _ in 0..total / sent.len() {
                stream.write_all(&sent).unwrap();
            }
        });
        let segment = parse_playlist(
            "#EXTM3U\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n",
            &format!("http://{address}/index.m3u8"),
        )
        .unwrap()
        .segments
        .remove(0);
        let output = root.join("segment.ts");
        let start = std::time::Instant::now();
        let result = download_segment(
            &segment,
            &HashMap::new(),
            "",
            &output,
            &control,
            None,
            None,
            0,
            true,
        );
        let elapsed = start.elapsed().as_secs_f64();
        server.join().unwrap();
        let checked = (|| -> Result<(), String> {
            result?;
            let mut file = fs::File::open(&output).map_err(|e| e.to_string())?;
            assert_eq!(file.metadata().unwrap().len(), total as u64);
            let mut received = vec![0; block.len()];
            for _ in 0..total / block.len() {
                file.read_exact(&mut received).map_err(|e| e.to_string())?;
                assert_eq!(received, block);
            }
            Ok(())
        })();
        fs::remove_dir_all(root).unwrap();
        checked.unwrap();
        eprintln!(
            "HLS_SEGMENT_BENCH bytes={total} seconds={elapsed:.6} mib_per_sec={:.3}",
            32.0 / elapsed
        );
    }

    #[test]
    fn segment_retries_only_recoverable_requests() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let root = std::env::temp_dir().join(format!(
            "hls-segment-retry-{}-{}",
            std::process::id(),
            address.port()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let requests = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
        let seen = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                if server_stop.load(Ordering::SeqCst) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let target = line.split_whitespace().nth(1).unwrap().to_string();
                let mut range = false;
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    range |= line.to_ascii_lowercase().starts_with("range:");
                }
                let hit = {
                    let mut counts = seen.lock().unwrap();
                    let count = counts.entry(target.clone()).or_default();
                    *count += 1;
                    *count
                };
                let route = target.trim_start_matches('/').trim_start_matches("range-");
                let status = match route {
                    "transient" if hit < 3 => 503,
                    "exhausted" => 503,
                    "no-range" => 200,
                    _ => route
                        .parse::<u16>()
                        .unwrap_or(if range { 206 } else { 200 }),
                };
                let body = b"ABCDEF";
                let truncated = route == "truncated" && hit == 1;
                let range_header = if status == 206 {
                    "Content-Range: bytes 0-5/6\r\n"
                } else {
                    ""
                };
                let header = format!("HTTP/1.1 {status} Test\r\nContent-Length: 6\r\n{range_header}Connection: close\r\n\r\n");
                let _ = reader.get_mut().write_all(header.as_bytes());
                let _ = reader
                    .get_mut()
                    .write_all(if truncated { &body[..2] } else { body });
            }
        });
        // 先完成所有场景并停止服务器，再断言；旧逻辑失败时也不会遗留测试线程。
        let mut outcomes = Vec::new();
        for range in [false, true] {
            for route in [
                "401",
                "403",
                "404",
                "410",
                "416",
                "transient",
                "exhausted",
                "truncated",
                "no-range",
                "write",
                "cipher",
            ] {
                if !range && route == "no-range" {
                    continue;
                }
                let target = format!("{}{}", if range { "range-" } else { "" }, route);
                let playlist = parse_playlist(
                    &format!("#EXTM3U\n#EXTINF:1,\n{target}\n#EXT-X-ENDLIST\n"),
                    &format!("http://{address}/index.m3u8"),
                )
                .unwrap();
                let mut segment = playlist.segments[0].clone();
                segment.byterange = range.then_some((0, 6));
                let output = if route == "write" {
                    let parent = root.join(format!("blocked-{range}"));
                    fs::write(&parent, b"file, not a directory").unwrap();
                    parent.join("segment.ts")
                } else {
                    root.join(format!("{target}.ts"))
                };
                let key = MediaKey {
                    method: "AES-128".into(),
                    uri: String::new(),
                    iv: None,
                };
                let key_bytes = vec![0; 16];
                let encrypted = route == "cipher";
                let result = download_segment(
                    &segment,
                    &HashMap::new(),
                    "hls-downloader://direct-proxy",
                    &output,
                    &control,
                    encrypted.then_some(&key_bytes),
                    encrypted.then_some(&key),
                    0,
                    true,
                );
                let count = requests
                    .lock()
                    .unwrap()
                    .get(&format!("/{target}"))
                    .copied()
                    .unwrap_or(0);
                let saved = fs::read(&output).ok();
                outcomes.push((target, result, count, saved));
            }
        }
        stop.store(true, Ordering::SeqCst);
        drop(TcpStream::connect(address).unwrap());
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
        for (target, result, count, saved) in outcomes {
            let route = target.trim_start_matches("range-");
            let expected = match route {
                "transient" | "exhausted" => 3,
                "truncated" => 2,
                "write" => 0,
                _ => 1,
            };
            eprintln!("{target}: requests={count}, result={result:?}");
            if route == "403" {
                // 通用传输在 403 后可尝试一次已有的浏览器身份传输；不能再重跑整个分片。
                assert!((1..=2).contains(&count), "{target}: {result:?}");
            } else {
                assert_eq!(count, expected, "{target}: {result:?}");
            }
            if matches!(route, "transient" | "truncated") {
                assert!(result.is_ok(), "{target}: {result:?}");
                assert_eq!(saved.as_deref(), Some(b"ABCDEF".as_slice()), "{target}");
            } else {
                assert!(result.is_err(), "{target}: {result:?}");
                assert!(saved.is_none(), "{target}: invalid segment was published");
                if let Ok(status) = route.parse::<u16>() {
                    assert!(
                        result.unwrap_err().contains(&format!("HTTP {status}")),
                        "{target}: actual response status lost"
                    );
                }
            }
        }
    }

    #[test]
    fn optional_ts_remux_falls_back_to_the_concatenated_stream() {
        let root = std::env::temp_dir().join(format!(
            "hls-remux-fallback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let origin = super::super::harness::serve_dynamic(Arc::new(move |path, _| {
            Some(match path {
                "/index.m3u8" => b"#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\ns0.ts\n#EXTINF:4,\ns1.ts\n#EXT-X-ENDLIST\n".to_vec(),
                "/s0.ts" => b"first-".to_vec(),
                _ => b"second".to_vec(),
            })
        }));
        for remux in [false, true] {
            let merged = download_hls_with(
                &format!("{}/index.m3u8", origin.base),
                &HashMap::new(),
                "",
                &root,
                &control,
                HlsDownloadOptions {
                    concurrency: 1,
                    download_subtitles: false,
                    remux_ts_to_mp4: remux,
                    ..HlsDownloadOptions::default()
                },
            )
            .unwrap();
            // Invalid TS cannot be remuxed: the setting must never lose the download.
            assert_eq!(fs::read(&merged).unwrap(), b"first-second", "remux={remux}");
            assert!(!root.join(REMUX_STAGING_TS).exists(), "remux={remux}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn separate_vod_audio_opens_preview_before_track_completion_and_resumes_both_tracks() {
        let root = std::env::temp_dir().join(format!(
            "hls-interleaved-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&requests);
        let server_root = root.clone();
        let phase = Arc::new(AtomicUsize::new(0));
        let server_phase = Arc::clone(&phase);
        let preview_seen = Arc::new(AtomicBool::new(false));
        let server_preview_seen = Arc::clone(&preview_seen);
        let origin = super::super::harness::serve_dynamic(Arc::new(move |path, _| {
            seen.lock().unwrap().push(path.to_string());
            let phase = server_phase.load(Ordering::SeqCst);
            if (phase == 0 && path == "/a1.ts") || (phase == 1 && path == "/v1.ts") {
                if phase == 0 {
                    server_preview_seen
                        .store(server_root.join("preview.m3u8").is_file(), Ordering::SeqCst);
                }
                fs::write(server_root.join("control"), "pause").unwrap();
            }
            Some(match path {
                "/master.m3u8" => b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Audio\",DEFAULT=YES,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"a\"\nvideo.m3u8\n".to_vec(),
                "/video.m3u8" => b"#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nv0.ts\n#EXTINF:4,\nv1.ts\n#EXT-X-ENDLIST\n".to_vec(),
                "/audio.m3u8" => b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\na0.ts\n#EXTINF:1,\na1.ts\n#EXTINF:1,\na2.ts\n#EXTINF:1,\na3.ts\n#EXTINF:1,\na4.ts\n#EXT-X-ENDLIST\n".to_vec(),
                _ => b"fragment bytes".to_vec(),
            })
        }));
        let run = || {
            download_hls_with(
                &format!("{}/master.m3u8", origin.base),
                &HashMap::new(),
                "",
                &root,
                &control,
                HlsDownloadOptions {
                    concurrency: 1,
                    download_subtitles: false,
                    ..HlsDownloadOptions::default()
                },
            )
        };
        assert_eq!(run().unwrap_err(), "paused");
        assert!(preview_seen.load(Ordering::SeqCst));
        let video = root.join("segments/000000.ts");
        let audio = root.join("audio/segments/000000.ts");
        assert_eq!(fs::read(&video).unwrap(), b"fragment bytes");
        assert_eq!(fs::read(&audio).unwrap(), b"fragment bytes");
        assert!(super::super::preview::is_ready(
            &root,
            "audio/segments/000000.ts"
        ));
        assert!(!root.join("audio/segments/000004.ts").exists());
        let playlist = fs::read_to_string(root.join("audio/preview-audio.m3u8")).unwrap();
        assert!(playlist.contains("000004.ts") && playlist.contains("#EXT-X-ENDLIST"));
        phase.store(1, Ordering::SeqCst);
        fs::write(&control, "run").unwrap();
        assert_eq!(run().unwrap_err(), "paused");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.iter().filter(|path| *path == "/v0.ts").count(), 1);
        assert_eq!(requests.iter().filter(|path| *path == "/a0.ts").count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn buffered_preview_waits_for_contiguous_segments_and_external_audio() {
        let root = std::env::temp_dir().join(format!(
            "hls-preview-prefix-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("segments")).unwrap();
        let playlist = parse_playlist("#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\na.ts\n#EXTINF:5,\nb.ts\n#EXTINF:2,\nc.ts\n#EXT-X-ENDLIST\n", "https://cdn.test/v.m3u8").unwrap();
        let prepared: Vec<_> = playlist
            .segments
            .into_iter()
            .enumerate()
            .map(|(slot, segment)| {
                let path = root.join(format!("segments/{slot:06}.ts"));
                fs::write(&path, b"complete fragment").unwrap();
                let timeline_sequence = segment.media_sequence;
                PreparedSegment {
                    segment,
                    identity: slot.to_string(),
                    timeline_sequence,
                    slot,
                    path,
                    init_map_path: None,
                    key_bytes: None,
                    needs_download: true,
                }
            })
            .collect();
        let mut landed = HashSet::from([prepared[1].path.clone()]);
        write_buffered_preview(&root, &[], &prepared, &landed, 6.0, false).unwrap();
        assert!(!root.join("preview.m3u8").exists());
        landed.insert(prepared[0].path.clone());
        write_buffered_preview(&root, &[], &prepared, &landed, 6.0, true).unwrap();
        assert!(!root.join("preview.m3u8").exists());
        fs::create_dir_all(root.join("audio")).unwrap();
        fs::write(root.join("audio/local.m3u8"), "#EXTM3U\n#EXT-X-ENDLIST\n").unwrap();
        write_buffered_preview(&root, &[], &prepared, &landed, 6.0, true).unwrap();
        let video = fs::read_to_string(root.join("local.m3u8")).unwrap();
        assert!(video.contains("000000.ts") && video.contains("000001.ts"));
        assert!(!video.contains("000002.ts") && !video.contains("#EXT-X-ENDLIST"));
        assert!(fs::read_to_string(root.join("preview.m3u8"))
            .unwrap()
            .contains("audio/preview-audio.m3u8"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_external_audio_failure_fails_vod_and_live_hls_downloads() {
        for live in [false, true] {
            let mut files = HashMap::new();
            files.insert(
                "/master.m3u8".into(),
                b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",DEFAULT=YES,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=1280x720,AUDIO=\"aud\"\nvideo.m3u8\n"
                    .to_vec(),
            );
            files.insert(
                "/video.m3u8".into(),
                b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nvideo.ts\n#EXT-X-ENDLIST\n"
                    .to_vec(),
            );
            files.insert("/video.ts".into(), b"VIDEO".to_vec());
            let origin = super::super::harness::serve_files(files);
            let dir = std::env::temp_dir().join(format!(
                "hls-selected-audio-failure-{}-{}-{live}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let control = dir.join("control");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&control, "run").unwrap();

            let error = download_hls_with(
                &format!("{}/master.m3u8", origin.base),
                &HashMap::new(),
                "",
                &dir,
                &control,
                HlsDownloadOptions {
                    live,
                    ..HlsDownloadOptions::default()
                },
            )
            .unwrap_err();
            assert!(error.contains("not an HLS playlist"), "{error}");
            assert!(!dir.join("merged.mp4").exists());
            assert!(!dir.join("muxed.mp4").exists());
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn dropping_live_audio_interrupts_its_request_without_canceling_the_parent() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let root =
            std::env::temp_dir().join(format!("hls-live-audio-stop-{}-{port}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let parent_control = root.join("control");
        fs::write(&parent_control, "run").unwrap();
        let (requested_tx, requested_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            requested_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let _ = reader.get_mut().write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        });
        let (drop_tx, drop_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let worker_root = root.clone();
        let context = crate::net_policy::ThrottleContext {
            control: parent_control.clone(),
            ..Default::default()
        };
        let worker = std::thread::spawn(move || {
            let recorder = crate::net_policy::with_throttle_context(Some(context), || {
                LiveAudioRecorder::start(
                    &format!("http://127.0.0.1:{port}/audio.m3u8"),
                    &HashMap::new(),
                    "hls-downloader://direct-proxy",
                    &worker_root,
                )
            });
            drop_rx.recv().unwrap();
            drop(recorder);
            done_tx.send(()).unwrap();
        });
        requested_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        drop_tx.send(()).unwrap();
        // 服务端保持阻塞，不能通过关闭连接使停止检查偶然通过。
        let stopped = done_rx.recv_timeout(Duration::from_secs(3)).is_ok();
        release_tx.send(()).unwrap();
        server.join().unwrap();
        worker.join().unwrap();
        let parent_state = fs::read_to_string(&parent_control).unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(
            stopped,
            "audio worker waited for the server instead of its own cancel signal"
        );
        assert_eq!(
            parent_state, "run",
            "audio cleanup must not cancel the parent task"
        );
    }

    #[test]
    fn live_audio_recorder_finish_reports_thread_panic() {
        let dir = std::env::temp_dir().join(format!(
            "hls-live-audio-panic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut recorder = LiveAudioRecorder {
            control: dir.join("audio.control"),
            parent_control: PathBuf::new(),
            finishing: Arc::new(AtomicBool::new(false)),
            handle: Some(std::thread::spawn(|| -> Result<PathBuf, String> {
                panic!("intentional live audio worker panic")
            })),
        };

        let error = recorder.finish().unwrap_err();
        assert!(error.contains("panicked"), "{error}");
        assert!(recorder.finish().unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn master_prefers_resolution_over_audio_only() {
        let text = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",DEFAULT=YES,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=1280x720,CODECS=\"avc1.42e01e\",AUDIO=\"aud\"\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1920x1080,CODECS=\"avc1.640028\",AUDIO=\"aud\"\nhigh.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=128000,CODECS=\"mp4a.40.2\"\naudio.m3u8\n";
        let playlist = parse_playlist(text, "https://cdn.test/master.m3u8").unwrap();
        assert!(playlist.is_master);
        assert_eq!(
            select_variant_for(&playlist, 800_000, 0, "").unwrap().uri,
            "https://cdn.test/low.m3u8"
        );
        assert_eq!(
            select_variant_for(&playlist, 0, 720, "").unwrap().uri,
            "https://cdn.test/low.m3u8"
        );
        assert_eq!(variant_choices(&playlist)[0].height, 1080);
        assert_eq!(audio_choices(&playlist).len(), 1);
        assert_eq!(audio_choices(&playlist)[0].kind, "audio");
        assert_eq!(
            select_audio_track(
                &playlist,
                select_variant_for(&playlist, 0, 0, "").unwrap(),
                "English"
            )
            .unwrap()
            .name,
            "English"
        );
    }

    #[test]
    fn exact_variant_survives_rotated_query_and_relative_paths() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080\na.m3u8?token=new\n#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080\nb.m3u8?token=new\n";
        let playlist = parse_playlist(text, "https://cdn.test/media/master.m3u8").unwrap();
        let second = &playlist.variants[1];
        for id in [
            &second.uri,
            "https://CDN.test:443/media/b.m3u8?token=old#video",
            "./b.m3u8?token=old",
            "/media/b.m3u8",
        ] {
            assert_eq!(
                select_variant_for(&playlist, 5_000_000, 1080, id),
                Some(second)
            );
        }
        assert_eq!(
            select_variant_for(
                &playlist,
                5_000_000,
                1080,
                "https://other.test/media/b.m3u8"
            ),
            Some(&playlist.variants[0])
        );
    }

    #[test]
    fn ambiguous_rotated_variant_uses_quality_or_falls_back() {
        let playlist = parse_playlist(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=9000\nother.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nshared.m3u8?id=a\n#EXT-X-STREAM-INF:BANDWIDTH=2000\nshared.m3u8?id=b\n",
            "https://cdn.test/master.m3u8",
        ).unwrap();
        let rotated = "shared.m3u8?token=old";
        assert_eq!(
            select_variant_for(&playlist, 0, 0, rotated),
            Some(&playlist.variants[0])
        );
        assert_eq!(
            select_variant_for(&playlist, 8500, 0, rotated),
            Some(&playlist.variants[2])
        );
        assert_eq!(
            select_variant_for(&playlist, 1000, 0, "shared.m3u8?id=b"),
            Some(&playlist.variants[2])
        );
    }

    #[test]
    fn non_http_segment_uris_are_dropped() {
        let text = "#EXTM3U\n#EXTINF:1,\nfile:///C:/Windows/win.ini\n#EXTINF:1,\nseg.ts\n#EXTINF:1,\ndata:text/plain,x\n";
        let playlist = parse_playlist(text, "https://cdn.test/a.m3u8").unwrap();
        assert_eq!(playlist.segments.len(), 1);
        assert_eq!(playlist.segments[0].uri, "https://cdn.test/seg.ts");
        assert_eq!(
            super::super::resolve_http_uri("https://cdn.test/a.m3u8", "file:///C:/secret"),
            ""
        );
        assert_eq!(
            super::super::resolve_http_uri("https://cdn.test/a.m3u8", "javascript:alert(1)"),
            ""
        );
        assert_eq!(
            super::super::resolve_http_uri(
                "https://cdn.test/a.m3u8",
                "\u{feff}javascript:alert(1)"
            ),
            ""
        );
        assert_eq!(
            super::super::resolve_http_uri("https://cdn.test/a.m3u8", "https://cdn.test/x.ts\0y"),
            ""
        );
        assert_eq!(
            super::super::resolve_http_uri("https://cdn.test/a.m3u8", "//cdn2.test/x.ts"),
            "https://cdn2.test/x.ts"
        );
    }

    #[test]
    fn signed_access_query_is_inherited_only_by_same_origin_children() {
        assert_eq!(
            super::super::resolve_http_uri(
                "https://cdn.test/master.m3u8?token=secret&pkey=key&_HLS_msn=9",
                "variant.m3u8?quality=hd",
            ),
            "https://cdn.test/variant.m3u8?quality=hd&token=secret&pkey=key"
        );
        assert_eq!(
            super::super::resolve_http_uri(
                "https://cdn.test/master.m3u8?token=secret",
                "https://other.test/segment.ts",
            ),
            "https://other.test/segment.ts"
        );
    }

    #[test]
    fn byterange_offsets_continue_per_resource_and_map_ranges_are_preserved() {
        let playlist = parse_playlist(
            "#EXTM3U\n#EXT-X-MAP:URI=\"media.mp4\",BYTERANGE=\"100@0\"\n#EXTINF:1,\n#EXT-X-BYTERANGE:4@100\nmedia.mp4\n#EXTINF:1,\n#EXT-X-BYTERANGE:3\nmedia.mp4\n",
            "https://cdn.test/index.m3u8",
        )
        .unwrap();
        assert_eq!(playlist.map_byterange, Some((0, 100)));
        assert_eq!(playlist.segments[0].byterange, Some((100, 4)));
        assert_eq!(playlist.segments[1].byterange, Some((104, 3)));
        assert!(parse_playlist(
            "#EXTM3U\n#EXTINF:1,\n#EXT-X-BYTERANGE:3\nmedia.mp4\n",
            "https://cdn.test/index.m3u8",
        )
        .unwrap_err()
        .contains("缺少起始偏移"));
        for middle in [
            "#EXTINF:1,\nother.ts\n",
            "#EXTINF:1,\n#EXT-X-BYTERANGE:2@0\nother.mp4\n",
            "#EXTINF:1,\nmedia.mp4\n",
        ] {
            let text = format!("#EXTM3U\n#EXTINF:1,\n#EXT-X-BYTERANGE:4@100\nmedia.mp4\n{middle}#EXTINF:1,\n#EXT-X-BYTERANGE:3\nmedia.mp4\n");
            assert!(parse_playlist(&text, "https://cdn.test/index.m3u8").is_err());
        }
    }

    #[test]
    fn master_collects_subtitle_renditions() {
        let text = "#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"English\",LANGUAGE=\"en\",DEFAULT=YES,URI=\"en.vtt\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=1280x720,SUBTITLES=\"subs\"\nindex.m3u8\n";
        let playlist = parse_playlist(text, "https://cdn.test/master.m3u8").unwrap();
        assert_eq!(
            select_subtitles(&playlist)[0].uri.as_deref(),
            Some("https://cdn.test/en.vtt")
        );
    }

    #[test]
    fn subtitle_language_cannot_escape_subs_dir() {
        assert_eq!(safe_label("../Startup/pwn", "und"), "Startup_pwn");
        assert_eq!(safe_label("..\\..\\Windows", "und"), "Windows");
        assert_eq!(safe_label("en-US", "und"), "en-US");
        assert_eq!(safe_label("...", "und"), "und");
    }

    #[test]
    fn keyformat_com_apple_is_rejected() {
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,KEYFORMAT=\"com.apple.streamingkeydelivery\",URI=\"skd://x\"\n#EXTINF:1,\na.ts\n";
        let error = parse_playlist(text, "https://cdn.test/a.m3u8").unwrap_err();
        assert!(
            error.contains("SAMPLE-AES") || error.contains("KEYFORMAT") || error.contains("DRM")
        );
    }

    #[test]
    fn media_playlist_keeps_map_parts_and_endlist() {
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXT-X-PART:DURATION=0.333,URI=\"part0.m4s\"\n#EXTINF:4,\nseg.ts\n#EXT-X-ENDLIST\n";
        let playlist = parse_playlist(text, "https://cdn.test/index.m3u8").unwrap();
        assert!(playlist.end_list);
        assert_eq!(
            playlist.map_uri.as_deref(),
            Some("https://cdn.test/init.mp4")
        );
        assert_eq!(playlist.segments.len(), 2);
        assert!(playlist.segments[0].is_part);
    }

    #[test]
    fn sample_aes_is_rejected() {
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key.bin\"\n#EXTINF:1,\na.ts\n";
        let error = parse_playlist(text, "https://cdn.test/a.m3u8").unwrap_err();
        assert!(error.contains("SAMPLE-AES"));
    }

    #[test]
    fn vod_playlist_concatenates_ts_segments() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let count = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..count]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let body: &[u8] = if path.ends_with(".m3u8") {
                    b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\na.ts\n#EXTINF:1,\nb.ts\n#EXT-X-ENDLIST\n"
                } else if path.ends_with("a.ts") {
                    b"AAA"
                } else {
                    b"BBB"
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        let dir = std::env::temp_dir().join(format!("hls-vod-{}", std::process::id()));
        let control = dir.join("control");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&control, "run").unwrap();
        let merged = download_hls(
            &format!("http://127.0.0.1:{port}/index.m3u8"),
            &HashMap::new(),
            "",
            &dir,
            &control,
            false,
        )
        .unwrap();
        assert_eq!(std::fs::read(&merged).unwrap(), b"AAABBB");
        let playlist = std::fs::read_to_string(dir.join("local.m3u8")).unwrap();
        assert!(playlist.contains("#EXT-X-ENDLIST"));
        assert!(playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn authenticated_vod_pause_resume_reuses_completed_segments() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let blocked_segment_seen = Arc::new(AtomicBool::new(false));
        let release_blocked_segment = Arc::new((Mutex::new(false), Condvar::new()));
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));

        let server_stop = Arc::clone(&stop);
        let server_seen = Arc::clone(&blocked_segment_seen);
        let server_release = Arc::clone(&release_blocked_segment);
        let server_requests = Arc::clone(&requests);
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                // Windows 接受的连接可能继承非阻塞模式，不能把尚未到达的请求当作断线。
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let end_of_headers = line == "\r\n" || line == "\n";
                            request.push_str(&line);
                            if end_of_headers {
                                break;
                            }
                        }
                    }
                }
                // HTTP 客户端可能先建立连接再放弃；空连接不是一次未授权请求。
                if request.is_empty() {
                    continue;
                }
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let path = target.split('?').next().unwrap_or(target);
                let authorized = request.lines().any(|line| {
                    line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("authorization")
                            && value.trim() == "Bearer hls-v7-test"
                    })
                });
                if let Ok(mut log) = server_requests.lock() {
                    log.push(format!("{path}|{authorized}"));
                }
                let (status, body): (u16, &[u8]) = if !authorized {
                    (401, b"unauthorized")
                } else if path == "/vod.m3u8" {
                    (
                        200,
                        b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nvod/a.ts\n#EXTINF:1,\nvod/b.ts\n#EXT-X-ENDLIST\n",
                    )
                } else if path == "/vod/a.ts" {
                    (200, b"AAA")
                } else if path == "/vod/b.ts" {
                    if !server_seen.swap(true, Ordering::SeqCst) {
                        let (lock, wake) = &*server_release;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = wake.wait(released).unwrap();
                        }
                    }
                    (200, b"BBB")
                } else {
                    (404, b"not found")
                };
                let header = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = reader.get_mut().write_all(header.as_bytes());
                let _ = reader.get_mut().write_all(body);
            }
        });

        let dir = std::env::temp_dir().join(format!(
            "hls-auth-vod-resume-{}-{}",
            std::process::id(),
            port
        ));
        let control = dir.join("control");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&control, "run").unwrap();
        let url = format!("http://127.0.0.1:{port}/vod.m3u8");
        let empty_headers = HashMap::new();
        let (status, _) = fetch_hls_bytes(&url, &empty_headers, "").unwrap();
        assert_eq!(status, 401, "the fixture must reject missing credentials");

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer hls-v7-test".into());
        let first_headers = headers.clone();
        let first_dir = dir.clone();
        let first_control = control.clone();
        let first_url = url.clone();
        let first = thread::spawn(move || {
            download_hls_with(
                &first_url,
                &first_headers,
                "",
                &first_dir,
                &first_control,
                HlsDownloadOptions {
                    concurrency: 1,
                    ..HlsDownloadOptions::default()
                },
            )
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        while !blocked_segment_seen.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "server did not receive the second segment"
            );
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(&control, "pause").unwrap();
        let (lock, wake) = &*release_blocked_segment;
        *lock.lock().unwrap() = true;
        wake.notify_all();
        assert_eq!(first.join().unwrap().unwrap_err(), "paused");

        std::fs::write(&control, "run").unwrap();
        let merged = download_hls_with(
            &url,
            &headers,
            "",
            &dir,
            &control,
            HlsDownloadOptions {
                concurrency: 1,
                ..HlsDownloadOptions::default()
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(&merged).unwrap(), b"AAABBB");
        let log = requests.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|entry| entry.as_str() == "/vod/a.ts|true")
                .count(),
            1,
            "resume must reuse the completed first segment"
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry.as_str() == "/vod/b.ts|true")
                .count(),
            2
        );
        assert_eq!(
            log.iter().filter(|entry| entry.ends_with("|false")).count(),
            1,
            "only the explicit unauthenticated probe may be rejected: {log:?}"
        );
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn authenticated_master_auto_detects_live_and_restores_atomic_timeline() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let playlist_requests = Arc::new(AtomicUsize::new(0));
        let blocked_segment_seen = Arc::new(AtomicBool::new(false));
        let release_blocked_segment = Arc::new((Mutex::new(false), Condvar::new()));
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));

        let server_stop = Arc::clone(&stop);
        let server_playlists = Arc::clone(&playlist_requests);
        let server_seen = Arc::clone(&blocked_segment_seen);
        let server_release = Arc::clone(&release_blocked_segment);
        let server_requests = Arc::clone(&requests);
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let end_of_headers = line == "\r\n" || line == "\n";
                            request.push_str(&line);
                            if end_of_headers {
                                break;
                            }
                        }
                    }
                }
                // HTTP 客户端可能先建立连接再放弃；空连接不是一次未授权请求。
                if request.is_empty() {
                    continue;
                }
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let path = target.split('?').next().unwrap_or(target);
                let authorized = request.lines().any(|line| {
                    line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("authorization")
                            && value.trim() == "Bearer hls-v7-test"
                    })
                });
                if let Ok(mut log) = server_requests.lock() {
                    log.push(format!("{path}|{authorized}"));
                }
                let (status, body): (u16, &[u8]) = if !authorized {
                    (401, b"unauthorized")
                } else if path == "/live.m3u8" {
                    (
                        200,
                        b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100000\nlive-video.m3u8\n",
                    )
                } else if path == "/live-video.m3u8" {
                    let number = server_playlists.fetch_add(1, Ordering::SeqCst);
                    if number == 0 {
                        (
                            200,
                        b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1,\nlive/0.ts\n#EXTINF:1,\nlive/1.ts\n",
                        )
                    } else {
                        (
                            200,
                            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:1,\nlive/1.ts\n#EXT-X-ENDLIST\n",
                        )
                    }
                } else if path == "/live/0.ts" {
                    (200, b"LIVE0")
                } else if path == "/live/1.ts" {
                    if !server_seen.swap(true, Ordering::SeqCst) {
                        let (lock, wake) = &*server_release;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = wake.wait(released).unwrap();
                        }
                    }
                    (200, b"LIVE1")
                } else {
                    (404, b"not found")
                };
                let header = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = reader.get_mut().write_all(header.as_bytes());
                let _ = reader.get_mut().write_all(body);
            }
        });

        let dir = std::env::temp_dir().join(format!(
            "hls-auth-live-resume-{}-{}",
            std::process::id(),
            port
        ));
        let control = dir.join("control");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&control, "run").unwrap();
        let url = format!("http://127.0.0.1:{port}/live.m3u8");
        let empty_headers = HashMap::new();
        let (status, _) = fetch_hls_bytes(&url, &empty_headers, "").unwrap();
        assert_eq!(status, 401, "the fixture must reject missing credentials");

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer hls-v7-test".into());
        let first_headers = headers.clone();
        let first_dir = dir.clone();
        let first_control = control.clone();
        let first_url = url.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let first = thread::spawn(move || {
            let _ = started_tx.send(());
            let result = download_hls_with(
                &first_url,
                &first_headers,
                "",
                &first_dir,
                &first_control,
                HlsDownloadOptions {
                    concurrency: 1,
                    download_subtitles: false,
                    ..HlsDownloadOptions::default()
                },
            );
            let _ = done_tx.send(result);
        });
        started_rx
            .recv_timeout(Duration::from_secs(15))
            .expect("live download worker did not start");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !blocked_segment_seen.load(Ordering::SeqCst) {
            match done_rx.try_recv() {
                Ok(result) => panic!("live download exited before the second segment: {result:?}"),
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("live download result channel disconnected before the second segment")
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            assert!(
                Instant::now() < deadline,
                "server did not receive the second live segment after the first completed"
            );
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(&control, "pause").unwrap();
        let (lock, wake) = &*release_blocked_segment;
        *lock.lock().unwrap() = true;
        wake.notify_all();
        let first_result = done_rx
            .recv_timeout(Duration::from_secs(15))
            .expect("paused live download did not finish");
        assert_eq!(first_result.unwrap_err(), "paused");
        first.join().unwrap();

        let checkpoint: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("live_state.json")).unwrap())
                .unwrap();
        assert_eq!(checkpoint["version"].as_u64(), Some(2));
        assert_eq!(checkpoint["segments"].as_array().map(Vec::len), Some(1));

        std::fs::write(&control, "run").unwrap();
        let merged = download_hls_with(
            &url,
            &headers,
            "",
            &dir,
            &control,
            HlsDownloadOptions {
                concurrency: 1,
                download_subtitles: false,
                ..HlsDownloadOptions::default()
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(&merged).unwrap(), b"LIVE0LIVE1");
        let log = requests.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|entry| entry.as_str() == "/live/0.ts|true")
                .count(),
            1,
            "restored live timeline must not refetch segment zero"
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry.as_str() == "/live/1.ts|true")
                .count(),
            2
        );
        assert_eq!(
            log.iter().filter(|entry| entry.ends_with("|false")).count(),
            1,
            "only the explicit unauthenticated probe may be rejected: {log:?}"
        );
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn segment_group_commit_is_bounded_by_count_bytes_and_age() {
        let mut batch = SegmentBatch::default();
        for index in 0..SEGMENT_COMMIT_MAX_FILES - 1 {
            assert!(batch
                .push(PathBuf::from(format!("{index}.ts")), 1)
                .is_none());
        }
        let due = batch.push(PathBuf::from("last.ts"), 1).unwrap();
        assert_eq!(due.len(), SEGMENT_COMMIT_MAX_FILES);
        assert!(batch.files.is_empty() && batch.oldest.is_none());
        let due = batch
            .push(PathBuf::from("big.ts"), SEGMENT_COMMIT_MAX_BYTES)
            .unwrap();
        assert_eq!(due, vec![PathBuf::from("big.ts")]);
        assert!(batch.push(PathBuf::from("a.ts"), 1).is_none());
        batch.oldest = Some(
            std::time::Instant::now()
                .checked_sub(SEGMENT_COMMIT_INTERVAL)
                .unwrap(),
        );
        assert_eq!(batch.push(PathBuf::from("b.ts"), 1).unwrap().len(), 2);
    }

    #[test]
    fn segment_group_commit_vouches_only_for_synced_batches() {
        let root = std::env::temp_dir().join(format!(
            "hls-group-commit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let first = root.join("000000.ts");
        let second = root.join("000001.ts");
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"two").unwrap();
        let recorded = Mutex::new(Vec::new());
        let record = |paths: &[PathBuf]| -> Result<(), String> {
            recorded.lock().unwrap().extend_from_slice(paths);
            Ok(())
        };
        let completed = Mutex::new(HashSet::from([first.clone(), second.clone()]));
        commit_segment_batch(std::slice::from_ref(&first), &record, &completed).unwrap();
        assert_eq!(*recorded.lock().unwrap(), vec![first.clone()]);
        // A batch that cannot be synced is never recorded and leaves `completed`,
        // so a resumed run downloads it again instead of trusting it.
        let missing = root.join("000002.ts");
        completed.lock().unwrap().insert(missing.clone());
        let batch = vec![second.clone(), missing.clone()];
        assert!(commit_segment_batch(&batch, &record, &completed).is_err());
        assert_eq!(*recorded.lock().unwrap(), vec![first.clone()]);
        let completed = completed.into_inner().unwrap();
        assert!(completed.contains(&first));
        assert!(!completed.contains(&second) && !completed.contains(&missing));
        // An empty batch is a no-op.
        commit_segment_batch(&[], &record, &Mutex::new(HashSet::new())).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn redownloaded_slot_loses_its_old_checkpoint_record() {
        let root = std::env::temp_dir().join(format!(
            "hls-forget-slot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut vod = VodCheckpoint::load(&root);
        vod.remember(3, "identity", 10);
        assert!(vod.can_reuse(3, "identity", 10));
        assert!(vod.forget(3));
        assert!(!vod.forget(3));
        assert!(!vod.can_reuse(3, "identity", 10));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vod_identity_skips_matching_and_rejects_mismatch() {
        let dir = std::env::temp_dir().join(format!("hls-vod-id-{}", std::process::id()));
        let seg_dir = dir.join("segments");
        std::fs::create_dir_all(&seg_dir).unwrap();
        std::fs::write(seg_dir.join("000000.ts"), b"AAA").unwrap();
        let segment = Segment {
            uri: "https://cdn.test/a.ts?token=old".into(),
            media_sequence: 0,
            part_index: None,
            key: None,
            init_map: None,
            gap: false,
            duration: 1.0,
            discontinuity: false,
            byterange: None,
            is_part: false,
            is_ad: false,
        };
        let identity = vod_segment_identity(&segment, 0, None, None);
        let legacy = crate::crypto_lite::sha256_hex(
            serde_json::json!({
                "discontinuity": false, "duration": 1.0, "init": "/", "key": "/",
                "key_method": null, "iv": null, "part_index": null, "range": null,
                "sequence": 0, "url": "/a.ts"
            })
            .to_string()
            .as_bytes(),
        );
        std::fs::write(
            dir.join("vod_segments.json"),
            serde_json::json!({
                "version": 1,
                "segments": {
                    "0": {"identity": identity, "size": 3},
                    "1": {"identity": legacy, "size": 3}
                }
            })
            .to_string(),
        )
        .unwrap();
        let vod = VodCheckpoint::load(&dir);
        let path = resume_segment_path(&seg_dir, 0, &segment.uri).unwrap();
        assert!(vod.can_reuse(0, &identity, file_len(&path)));
        assert!(!vod.can_reuse(1, &identity, 3));
        for uri in [
            "https://cdn.test/a.ts?token=new",
            "HTTPS://CDN.TEST:443/a.ts?token=new",
        ] {
            let mut refreshed = segment.clone();
            refreshed.uri = uri.into();
            let refreshed = vod_segment_identity(&refreshed, 0, None, None);
            assert!(vod.can_reuse(0, &refreshed, 3), "{uri}");
        }
        for uri in [
            "https://other.test/a.ts?token=new",
            "http://cdn.test/a.ts?token=new",
            "https://cdn.test:8443/a.ts?token=new",
        ] {
            let mut refreshed = segment.clone();
            refreshed.uri = uri.into();
            let refreshed = vod_segment_identity(&refreshed, 0, None, None);
            assert!(!vod.can_reuse(0, &refreshed, 3), "{uri}");
            assert!(!vod.can_reuse(1, &refreshed, 3), "{uri}");
        }
        let other = vod_segment_identity(
            &Segment {
                uri: "https://cdn.test/b.ts".into(),
                media_sequence: 0,
                part_index: None,
                key: None,
                init_map: None,
                gap: false,
                duration: 1.0,
                discontinuity: false,
                byterange: None,
                is_part: false,
                is_ad: false,
            },
            0,
            None,
            None,
        );
        assert!(!vod.can_reuse(0, &other, 3));
        assert_eq!(
            stable_media_url("https://cdn.test/a.ts?token=new&keep=1"),
            "https://cdn.test/a.ts?keep=1"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stable_media_url_normalizes_origins_without_changing_query_policy() {
        for (url, expected) in [
            (
                "HTTPS://CDN.TEST:443/a.ts?token=old&keep=1#part",
                "https://cdn.test/a.ts?keep=1",
            ),
            ("HTTP://CDN.TEST:80", "http://cdn.test/"),
            (
                "https://cdn.test:8443/a.ts?sig=old&b=2&a=1",
                "https://cdn.test:8443/a.ts?a=1&b=2",
            ),
            (
                "https://[2001:DB8::1]:443/a.ts?expires=1",
                "https://[2001:db8::1]/a.ts",
            ),
            ("relative.ts?token=old&keep=1", "relative.ts?keep=1"),
            ("", "/"),
        ] {
            assert_eq!(stable_media_url(url), expected, "{url}");
        }
    }

    #[test]
    fn vod_resume_never_reuses_a_segment_with_no_checkpoint_record() {
        // A run killed between `fs::write(output, ..)` and `vod.save(...)`
        // leaves a complete-looking file behind with nothing vouching for it,
        // and slot numbers are reused across playlists.  Reusing such a file
        // silent-splices stale or truncated bytes into the output, so the
        // checkpoint must fail closed.
        let dir = std::env::temp_dir().join(format!("hls-vod-norecord-{}", std::process::id()));
        let seg_dir = dir.join("segments");
        std::fs::create_dir_all(&seg_dir).unwrap();
        std::fs::write(seg_dir.join("000000.ts"), b"AAA").unwrap();
        let segment = Segment {
            uri: "https://cdn.test/a.ts?token=old".into(),
            media_sequence: 0,
            part_index: None,
            key: None,
            init_map: None,
            gap: false,
            duration: 1.0,
            discontinuity: false,
            byterange: None,
            is_part: false,
            is_ad: false,
        };
        let identity = vod_segment_identity(&segment, 0, None, None);

        // No vod_segments.json at all: the file on disk proves nothing.
        let vod = VodCheckpoint::load(&dir);
        let path = resume_segment_path(&seg_dir, 0, &segment.uri).unwrap();
        assert!(!vod.can_reuse(0, &identity, file_len(&path)));
        assert!(!vod.has_slot(0));
        // prepare_segments therefore has to drop it and re-download.
        assert!(resume_segment_path(&seg_dir, 0, &segment.uri).is_some());

        // A record that does not match is still refused.
        std::fs::write(
            dir.join("vod_segments.json"),
            serde_json::json!({
                "version": 1,
                "segments": {"0": {"identity": "some-other-segment", "size": 3}}
            })
            .to_string(),
        )
        .unwrap();
        let vod = VodCheckpoint::load(&dir);
        assert!(vod.has_slot(0));
        assert!(!vod.can_reuse(0, &identity, file_len(&path)));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cue_out_and_ad_path_are_marked_and_filtered() {
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nmedia0.ts\n#EXT-X-CUE-OUT:30\n#EXTINF:4,\nads/preroll.ts\n#EXTINF:4,\nad_1.ts\n#EXT-X-CUE-IN\n#EXTINF:4,\nmedia1.ts\n#EXT-X-ENDLIST\n";
        let mut playlist = parse_playlist(text, "https://cdn.test/index.m3u8").unwrap();
        assert!(playlist.segments[1].is_ad);
        assert!(playlist.segments[2].is_ad);
        assert!(!playlist.segments[0].is_ad);
        assert!(!playlist.segments[3].is_ad);
        apply_ad_policy(&mut playlist, true);
        assert_eq!(playlist.segments.len(), 2);
        assert!(playlist.segments[1].discontinuity);
        assert_eq!(playlist.segments[1].uri, "https://cdn.test/media1.ts");
    }

    #[test]
    fn live_subtitles_merge_sidecar_cues() {
        let dir = std::env::temp_dir().join(format!("hls-live-sub-{}", std::process::id()));
        let cache = dir.join("live-subtitles").join("en");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("00000000.vtt"),
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\none\n",
        )
        .unwrap();
        std::fs::write(
            cache.join("00000001.vtt"),
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\ntwo\n",
        )
        .unwrap();
        let session = LiveSubSession {
            tracks: vec![LiveSubTrack {
                key: "en".into(),
                language: "en".into(),
                name: "English".into(),
                uri: "https://cdn.test/en.m3u8".into(),
                seen: BTreeSet::new(),
                count: 2,
            }],
        };
        let published = session.publish(&dir);
        assert_eq!(published.len(), 1);
        let text = std::fs::read_to_string(&published[0]).unwrap();
        assert!(has_cues(&text));
        assert!(text.contains("one"));
        assert!(text.contains("two"));
        assert!(dir.join("subs").join("en.srt").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_subtitle_webvtt_url_is_captured_once() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let body = b"WEBVTT\n\n00:00:00.000 --> 00:00:01.000\ncue\n";
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        let dir = std::env::temp_dir().join(format!("hls-live-vtt-{}", std::process::id()));
        let control = dir.join("control");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&control, "run").unwrap();
        let mut session = LiveSubSession {
            tracks: vec![LiveSubTrack {
                key: "en".into(),
                language: "en".into(),
                name: "English".into(),
                uri: format!("http://127.0.0.1:{port}/en.vtt"),
                seen: BTreeSet::new(),
                count: 0,
            }],
        };
        session.capture(&HashMap::new(), "", &dir, &control);
        session.capture(&HashMap::new(), "", &dir, &control);
        assert_eq!(session.tracks[0].count, 1);
        let published = session.publish(&dir);
        assert_eq!(published.len(), 1);
        assert!(std::fs::read_to_string(&published[0])
            .unwrap()
            .contains("cue"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ll_hls_tracks_sequence_parts_keys_maps_and_gaps() {
        let playlist = parse_playlist(
            "#EXTM3U\n\
             #EXT-X-MEDIA-SEQUENCE:40\n\
             #EXT-X-SKIP:SKIPPED-SEGMENTS=2\n\
             #EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,HOLD-BACK=3\n\
             #EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"4@0\"\n\
             #EXT-X-KEY:METHOD=AES-128,URI=\"key-a.bin\",IV=0x1\n\
             #EXT-X-PART:DURATION=0.5,URI=\"media.mp4\",BYTERANGE=\"3@4\"\n\
             #EXT-X-PART:DURATION=0.5,URI=\"media.mp4\",BYTERANGE=\"3\"\n\
             #EXT-X-KEY:METHOD=NONE\n\
             #EXTINF:1,\n\
             media.mp4\n\
             #EXT-X-GAP\n\
             #EXTINF:1,\n\
             gap.mp4\n",
            "https://cdn.test/index.m3u8",
        )
        .unwrap();
        assert!(playlist.can_block_reload);
        assert_eq!(playlist.skipped_segments, 2);
        assert_eq!(playlist.map_byterange, Some((0, 4)));
        assert_eq!(playlist.segments.len(), 4);
        assert_eq!(playlist.segments[0].media_sequence, 42);
        assert_eq!(playlist.segments[0].part_index, Some(0));
        assert_eq!(playlist.segments[0].byterange, Some((4, 3)));
        assert!(playlist.segments[0].key.is_some());
        assert_eq!(playlist.segments[1].part_index, Some(1));
        assert_eq!(playlist.segments[1].byterange, Some((7, 3)));
        assert_eq!(playlist.segments[2].media_sequence, 42);
        assert!(playlist.segments[2].part_index.is_none());
        assert!(playlist.segments[2].key.is_none());
        assert_eq!(playlist.segments[3].media_sequence, 43);
        assert!(playlist.segments[3].gap);

        let pending = pending_segments(&playlist, &[]);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].uri, "https://cdn.test/media.mp4");
        assert!(pending[0].part_index.is_none());
    }

    #[test]
    fn segment_identity_includes_sequence_part_range_and_stable_url() {
        let mut first = Segment {
            uri: "https://cdn.test/media.mp4?token=old&keep=1".into(),
            media_sequence: 7,
            part_index: Some(0),
            key: None,
            init_map: None,
            gap: false,
            duration: 0.5,
            discontinuity: false,
            byterange: Some((4, 3)),
            is_part: true,
            is_ad: false,
        };
        let second = Segment {
            part_index: Some(1),
            ..first.clone()
        };
        assert_ne!(segment_identity(&first), segment_identity(&second));
        first.media_sequence += 1;
        assert_ne!(segment_identity(&first), segment_identity(&second));
        first.media_sequence = second.media_sequence;
        first.byterange = Some((7, 3));
        assert_ne!(segment_identity(&first), segment_identity(&second));
        first.byterange = second.byterange;
        first.part_index = second.part_index;
        first.uri = second.uri.replace("old", "new");
        assert_eq!(segment_identity(&first), segment_identity(&second));
    }

    #[test]
    fn live_state_v2_is_atomic_and_invalid_state_is_discarded() {
        let dir = std::env::temp_dir().join(format!("hls-live-state-{}", std::process::id()));
        let seg_dir = dir.join("segments");
        std::fs::create_dir_all(&seg_dir).unwrap();
        let segment_path = seg_dir.join("000000.ts");
        std::fs::write(&segment_path, b"segment").unwrap();
        let state_path = dir.join("live_state.json");
        let timeline = LiveTimeline {
            entries: vec![TimelineEntry {
                identity: "identity".into(),
                uri: String::new(),
                media_sequence: 9,
                part_index: Some(0),
                path: segment_path,
                duration: 0.5,
                discontinuity: false,
                init_map_path: None,
            }],
            next_slot: 1,
            ..Default::default()
        };
        timeline.save(&state_path).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
        assert_eq!(
            value.get("version").and_then(|value| value.as_u64()),
            Some(2)
        );
        assert_eq!(LiveTimeline::load(&dir).entries.len(), 1);

        std::fs::write(&state_path, b"{\"version\":2,\"segments\":").unwrap();
        assert!(LiveTimeline::load(&dir).entries.is_empty());
        assert!(!dir.join(".live_state.json.tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn blocking_reload_requests_next_media_position() {
        let mut previous = parse_playlist(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:10\n#EXT-X-PART-INF:PART-TARGET=0.5\n#EXT-X-PART:DURATION=0.5,URI=\"10.0.m4s\"\n#EXT-X-PART:DURATION=0.5,URI=\"10.1.m4s\"\n",
            "https://cdn.test/live.m3u8?token=keep",
        )
        .unwrap();
        assert_eq!(
            blocking_reload_url(
                "https://cdn.test/live.m3u8?token=keep&_HLS_msn=old",
                &previous
            ),
            "https://cdn.test/live.m3u8?token=keep&_HLS_msn=10&_HLS_part=2"
        );
        previous.segments[1].part_index = None;
        assert_eq!(
            blocking_reload_url("https://cdn.test/live.m3u8?token=keep", &previous),
            "https://cdn.test/live.m3u8?token=keep&_HLS_msn=11"
        );
    }

    #[test]
    fn extension_sanitizes_invalid_or_ads_suffixes_to_bin() {
        assert_eq!(extension("https://cdn.test/seg-1.ts?sig=1"), "ts");
        assert_eq!(extension("https://cdn.test/seg-1.m4s#frag"), "m4s");
        assert_eq!(extension("https://cdn.test/img.jpg:large"), "bin");
        assert_eq!(extension("https://cdn.test/seg.ts;v=1"), "bin");
        assert_eq!(extension("https://cdn.test/seg.mp4%20"), "bin");
        assert_eq!(
            extension("https://cdn.test/seg.verylongextensionname"),
            "bin"
        );
    }

    #[test]
    fn live_subtitle_key_disambiguates_same_language_tracks_and_capture_skips_gap_with_byterange() {
        let track_a = Rendition {
            kind: "SUBTITLES".into(),
            group_id: "subs".into(),
            name: "English CC".into(),
            language: "en".into(),
            uri: Some("https://cdn.test/subs/en-cc.m3u8?token=a".into()),
            default: true,
        };
        let track_b = Rendition {
            kind: "SUBTITLES".into(),
            group_id: "subs".into(),
            name: "English Forced".into(),
            language: "en".into(),
            uri: Some("https://cdn.test/subs/en-forced.m3u8?token=b".into()),
            default: false,
        };
        assert_ne!(live_subtitle_key(&track_a), live_subtitle_key(&track_b));
        // Rotating auth query must keep the same stable key.
        let track_a_rotated = Rendition {
            uri: Some("https://cdn.test/subs/en-cc.m3u8?token=fresh".into()),
            ..track_a.clone()
        };
        assert_eq!(
            live_subtitle_key(&track_a),
            live_subtitle_key(&track_a_rotated)
        );

        let payload = concat!(
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nFirst\n\n",
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nSecond\n"
        );
        let split = payload.find("WEBVTT\n\n00:00:01").unwrap();
        let playlist_body = format!(
            "#EXTM3U\n#EXT-X-VERSION:4\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n\
             #EXT-X-GAP\n#EXTINF:1.0,\ngap.vtt\n\
             #EXTINF:1.0,\n#EXT-X-BYTERANGE:{split}@0\nbundle.vtt\n\
             #EXTINF:1.0,\n#EXT-X-BYTERANGE:{}@{split}\nbundle.vtt\n",
            payload.len() - split
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let playlist_bytes = playlist_body.into_bytes();
        let bundle_bytes = payload.as_bytes().to_vec();
        let handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            for _ in 0..3 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/");
                if path == "/sub.m3u8" {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        playlist_bytes.len()
                    );
                    let _ = stream.write_all(&playlist_bytes);
                } else if path == "/bundle.vtt" {
                    let range = req.lines().find_map(|line| {
                        let (k, v) = line.split_once(':')?;
                        k.trim()
                            .eq_ignore_ascii_case("range")
                            .then(|| v.trim().strip_prefix("bytes="))
                            .flatten()
                    });
                    if let Some((start, end)) = range.and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                    }) {
                        let slice = &bundle_bytes[start..=end.min(bundle_bytes.len() - 1)];
                        let _ = write!(
                            stream,
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            start + slice.len() - 1,
                            bundle_bytes.len(),
                            slice.len()
                        );
                        let _ = stream.write_all(slice);
                    } else {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            bundle_bytes.len()
                        );
                        let _ = stream.write_all(&bundle_bytes);
                    }
                }
            }
        });
        let base = format!("http://{addr}");
        let root = std::env::temp_dir().join(format!("hls-live-sub-gap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let mut session = LiveSubSession::load(
            &root,
            &[Rendition {
                kind: "SUBTITLES".into(),
                group_id: "s".into(),
                name: "English".into(),
                language: "en".into(),
                uri: Some(format!("{base}/sub.m3u8")),
                default: true,
            }],
        );
        session.capture(&HashMap::new(), "", &root, &control);
        let _ = handle.join();
        assert_eq!(
            session.tracks[0].count, 2,
            "must capture both byteranges and skip gap"
        );
        let published = session.publish(&root);
        assert_eq!(published.len(), 1);
        let merged = fs::read_to_string(&published[0]).unwrap();
        assert!(
            merged.contains("First") && merged.contains("Second"),
            "{merged}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hls_download_options_default_enables_ts_to_mp4_remux() {
        assert!(HlsDownloadOptions::default().remux_ts_to_mp4);
    }

    #[test]
    fn select_audio_track_preserves_embedded_default_audio_over_external_commentary() {
        let master = concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:4\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"Original\",LANGUAGE=\"en\",DEFAULT=YES,AUTOSELECT=YES\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"Commentary\",LANGUAGE=\"en\",DEFAULT=NO,AUTOSELECT=NO,URI=\"commentary.m3u8\"\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=4500000,RESOLUTION=1920x1080,AUDIO=\"aud\"\n",
            "video.m3u8\n",
        );
        let playlist = parse_playlist(master, "https://cdn.example.test/master.m3u8").unwrap();
        let variant = select_variant_for(&playlist, 0, 0, "").unwrap();

        let default_track = select_audio_track(&playlist, variant, "").unwrap();
        assert_eq!(default_track.name, "Original");
        assert!(
            default_track.uri.is_none(),
            "embedded DEFAULT=YES audio must keep uri=None so muxing retains 0:a:0"
        );

        let commentary_track = select_audio_track(&playlist, variant, "Commentary").unwrap();
        assert_eq!(commentary_track.name, "Commentary");
        assert_eq!(
            commentary_track.uri.as_deref(),
            Some("https://cdn.example.test/commentary.m3u8")
        );

        let explicit_original = select_audio_track(&playlist, variant, "Original").unwrap();
        assert_eq!(explicit_original.name, "Original");
        assert!(explicit_original.uri.is_none());

        let choices = audio_choices(&playlist);
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().any(|c| c.name == "Original"));
        assert!(choices.iter().any(|c| c.name == "Commentary"));

        let reordered = concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:4\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"Commentary\",LANGUAGE=\"en\",DEFAULT=NO,AUTOSELECT=NO,URI=\"commentary.m3u8\"\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"Original\",LANGUAGE=\"en\",DEFAULT=YES,AUTOSELECT=YES\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=4500000,RESOLUTION=1920x1080,AUDIO=\"aud\"\n",
            "video.m3u8\n",
        );
        let reordered_pl =
            parse_playlist(reordered, "https://cdn.example.test/master.m3u8").unwrap();
        let reordered_var = select_variant_for(&reordered_pl, 0, 0, "").unwrap();
        let lang_match = select_audio_track(&reordered_pl, reordered_var, "en").unwrap();
        assert_eq!(
            lang_match.name, "Original",
            "DEFAULT=YES must win language ties even when listed after DEFAULT=NO"
        );

        let embedded_only = concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"Main\",LANGUAGE=\"en\",DEFAULT=YES\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720,AUDIO=\"aud\"\n",
            "video.m3u8\n",
        );
        let embedded_only_pl =
            parse_playlist(embedded_only, "https://cdn.example.test/master.m3u8").unwrap();
        assert!(audio_choices(&embedded_only_pl).is_empty());
    }

    #[test]
    fn ll_hls_parts_do_not_consume_parent_discontinuity_daterange_gap_or_byterange() {
        let m3u8 = concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA-SEQUENCE:20\n",
            "#EXT-X-PART-INF:PART-TARGET=0.5\n",
            "#EXT-X-DISCONTINUITY\n",
            "#EXT-X-DATERANGE:ID=\"ad-break\",SCTE35-OUT=0xFC3011\n",
            "#EXT-X-GAP\n",
            "#EXT-X-BYTERANGE:120@40\n",
            "#EXT-X-PART:DURATION=0.5,URI=\"p0.m4s\"\n",
            "#EXT-X-PART:DURATION=0.5,URI=\"p1.m4s\"\n",
            "#EXTINF:1.0,\n",
            "seg20.m4s\n",
            "#EXTINF:1.0,\n",
            "seg21.m4s\n",
        );
        let parsed = parse_playlist(m3u8, "https://cdn.test/live.m3u8").unwrap();
        assert_eq!(parsed.segments.len(), 4);
        let p0 = &parsed.segments[0];
        let p1 = &parsed.segments[1];
        let seg20 = &parsed.segments[2];
        let seg21 = &parsed.segments[3];

        assert!(p0.is_part && p0.discontinuity && p0.is_ad && p0.gap && p0.byterange.is_none());
        assert!(p1.is_part && !p1.discontinuity && p1.is_ad && p1.gap && p1.byterange.is_none());
        assert!(!seg20.is_part);
        assert!(
            seg20.discontinuity,
            "parent segment must retain discontinuity"
        );
        assert!(
            seg20.is_ad,
            "parent segment must retain pending daterange ad"
        );
        assert!(seg20.gap, "parent segment must retain pending gap");
        assert_eq!(seg20.byterange, Some((40, 120)));

        assert!(!seg21.discontinuity && !seg21.is_ad && !seg21.gap && seg21.byterange.is_none());
    }

    #[test]
    fn ll_hls_blocking_reload_uses_unfiltered_ad_tail_and_propagates_controls() {
        let mut with_tail_ads = parse_playlist(
            concat!(
                "#EXTM3U\n",
                "#EXT-X-MEDIA-SEQUENCE:10\n",
                "#EXT-X-PART-INF:PART-TARGET=0.5\n",
                "#EXTINF:1.0,\n",
                "main10.m4s\n",
                "#EXT-X-CUE-OUT:10\n",
                "#EXT-X-PART:DURATION=0.5,URI=\"ad11.0.m4s\"\n",
                "#EXT-X-PART:DURATION=0.5,URI=\"ad11.1.m4s\"\n",
            ),
            "https://cdn.test/live.m3u8",
        )
        .unwrap();
        apply_ad_policy(&mut with_tail_ads, true);
        assert_eq!(with_tail_ads.segments.len(), 1);
        assert_eq!(
            blocking_reload_url("https://cdn.test/live.m3u8", &with_tail_ads),
            "https://cdn.test/live.m3u8?_HLS_msn=11&_HLS_part=2"
        );

        let root = std::env::temp_dir().join(format!(
            "hls-reload-control-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        for (word, expected) in [
            ("pause", "paused"),
            ("cancel", "canceled"),
            ("finish", "finish"),
        ] {
            fs::write(&control, word).unwrap();
            assert_eq!(
                reload_playlist(
                    "http://127.0.0.1:1/live.m3u8",
                    &HashMap::new(),
                    "",
                    &with_tail_ads,
                    &control
                )
                .unwrap_err(),
                expected
            );
            assert_eq!(wait_with_control(&control, 5.0).unwrap_err(), expected);
        }

        // Server ignoring _HLS_msn and returning unchanged tail must sleep bounded poll interval instead of tight-spinning.
        let spin_dir = root.join("spin");
        fs::create_dir_all(&spin_dir).unwrap();
        let spin_control = spin_dir.join("control");
        fs::write(&spin_control, "run").unwrap();
        let finish_ctrl = spin_control.clone();
        let reload_hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&reload_hits);
        let spin_origin = super::super::harness::serve_dynamic(Arc::new(
            move |path, _| match path {
                "/live.m3u8" => {
                    let n = server_hits.fetch_add(1, Ordering::SeqCst) + 1;
                    if n >= 3 {
                        let _ = fs::write(&finish_ctrl, "finish");
                    }
                    Some(
                    b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:1.0,\ns5.ts\n"
                        .to_vec(),
                )
                }
                "/s5.ts" => Some(b"SEG5".to_vec()),
                _ => None,
            },
        ));
        let started = Instant::now();
        let out = download_hls_with(
            &format!("{}/live.m3u8", spin_origin.base),
            &HashMap::new(),
            "",
            &spin_dir,
            &spin_control,
            HlsDownloadOptions {
                live: true,
                download_subtitles: false,
                remux_ts_to_mp4: false,
                ..HlsDownloadOptions::default()
            },
        )
        .unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(350),
            "unchanged LL-HLS reload tail must sleep bounded poll interval, elapsed={:?}",
            started.elapsed()
        );
        assert_eq!(fs::read(&out).unwrap(), b"SEG5");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_hls_finish_publishes_recorded_segments_and_handles_empty_or_silent_audio() {
        let root = std::env::temp_dir().join(format!(
            "hls-live-finish-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");

        // 0 segments recorded when finish is already set -> clean Chinese error.
        let empty_dir = root.join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        fs::write(&control, "finish").unwrap();
        let empty_origin = super::super::harness::serve_files(HashMap::from([(
            "/live.m3u8".into(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\ns0.ts\n".to_vec(),
        )]));
        let err = download_hls_with(
            &format!("{}/live.m3u8", empty_origin.base),
            &HashMap::new(),
            "",
            &empty_dir,
            &control,
            HlsDownloadOptions {
                live: true,
                remux_ts_to_mp4: false,
                ..HlsDownloadOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(err, "未录制到任何直播分片");

        // Live video + silent/unready external audio finishes cleanly when finish is requested.
        let rec_dir = root.join("recorded");
        fs::create_dir_all(&rec_dir).unwrap();
        let rec_control = rec_dir.join("control");
        fs::write(&rec_control, "run").unwrap();
        let signal_control = rec_control.clone();
        let origin = super::super::harness::serve_dynamic(Arc::new(move |path, hit| {
            match path {
            "/master.m3u8" => Some(
                b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Audio\",DEFAULT=YES,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"a\"\nvideo.m3u8\n".to_vec(),
            ),
            "/video.m3u8" => {
                if hit >= 2 {
                    let _ = fs::write(&signal_control, "finish");
                }
                Some(b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2,\nv0.ts\n".to_vec())
            }
            "/audio.m3u8" => {
                // Valid live audio playlist whose segments are still GAP / not yet available.
                Some(b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-GAP\n#EXTINF:2,\na0.ts\n".to_vec())
            }
            "/v0.ts" => Some(b"LIVE-VIDEO-BYTES".to_vec()),
            _ => None,
        }
        }));
        let out = download_hls_with(
            &format!("{}/master.m3u8", origin.base),
            &HashMap::new(),
            "",
            &rec_dir,
            &rec_control,
            HlsDownloadOptions {
                live: true,
                download_subtitles: false,
                remux_ts_to_mp4: false,
                ..HlsDownloadOptions::default()
            },
        )
        .unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"LIVE-VIDEO-BYTES");
        assert!(!rec_dir.join("audio").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn vod_cancel_overrides_paused_worker_error() {
        let root = std::env::temp_dir().join(format!(
            "hls-vod-cancel-prio-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("segments")).unwrap();
        let control = root.join("control");
        fs::write(&control, "pause").unwrap();
        let control_for_server = control.clone();
        let origin = super::super::harness::serve_dynamic(Arc::new(move |_, _| {
            let _ = fs::write(&control_for_server, "cancel");
            Some(b"DATA".to_vec())
        }));
        let seg = Segment {
            uri: format!("{}/s0.ts", origin.base),
            media_sequence: 0,
            part_index: None,
            key: None,
            init_map: None,
            gap: false,
            duration: 2.0,
            discontinuity: false,
            byterange: None,
            is_part: false,
            is_ad: false,
        };
        let prepared = vec![PreparedSegment {
            identity: segment_identity(&seg),
            timeline_sequence: 0,
            segment: seg,
            slot: 0,
            path: root.join("segments/000000.ts"),
            init_map_path: None,
            key_bytes: None,
            needs_download: true,
        }];
        fs::write(&control, "cancel").unwrap();
        let (_, err) = download_prepared_segments(
            &prepared,
            &[],
            &HashMap::new(),
            "",
            &control,
            1,
            None,
            &|_| {},
            None,
        );
        assert_eq!(err.as_deref(), Some("canceled"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn aes_key_caches_by_uri_retries_transient_only_and_skips_completed_vod_segments() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&hits);
        let server = std::thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                server_hits.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    stream
                        .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbusy")
                        .unwrap();
                } else {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\nConnection: close\r\n\r\n0123456789abcdef")
                        .unwrap();
                }
            }
        });

        let root = std::env::temp_dir().join(format!(
            "hls-key-cache-resume-{}-{}",
            std::process::id(),
            addr.port()
        ));
        fs::create_dir_all(root.join("segments")).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();

        let key_iv1 = MediaKey {
            method: "AES-128".into(),
            uri: format!("http://{addr}/aes.key"),
            iv: Some([1; 16]),
        };
        let key_iv2 = MediaKey {
            method: "AES-128".into(),
            uri: format!("http://{addr}/aes.key"),
            iv: Some([2; 16]),
        };
        let mut cache = HashMap::new();
        let bytes1 = load_key(&key_iv1, &HashMap::new(), "", &control, &mut cache).unwrap();
        let bytes2 = load_key(&key_iv2, &HashMap::new(), "", &control, &mut cache).unwrap();
        server.join().unwrap();
        assert_eq!(bytes1, b"0123456789abcdef");
        assert_eq!(bytes2, b"0123456789abcdef");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            2,
            "second IV must hit URI cache"
        );

        // Resumed VOD segment already on disk must not fetch an expired/dead key server.
        let dead_key_seg = Segment {
            uri: "https://cdn.test/seg0.ts".into(),
            media_sequence: 0,
            part_index: None,
            key: Some(MediaKey {
                method: "AES-128".into(),
                uri: "http://127.0.0.1:1/expired.key".into(),
                iv: Some([0; 16]),
            }),
            init_map: None,
            gap: false,
            duration: 2.0,
            discontinuity: false,
            byterange: None,
            is_part: false,
            is_ad: false,
        };
        let existing = root.join("segments/000000.ts");
        fs::write(&existing, b"DECRYPTED").unwrap();
        let mut vod = VodCheckpoint::load(&root);
        vod.remember(0, &segment_identity(&dead_key_seg), 9);
        let prepared = prepare_segments(
            vec![dead_key_seg],
            &HashMap::new(),
            "",
            &root,
            &control,
            false,
            0,
            &vod,
            &mut 0,
            &mut HashMap::new(),
            &mut HashMap::new(),
        )
        .unwrap();
        assert_eq!(prepared.len(), 1);
        assert!(!prepared[0].needs_download);
        assert!(prepared[0].key_bytes.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hls_key_and_segment_retry_backoff_propagate_finish_control() {
        let root = std::env::temp_dir().join(format!(
            "hls-finish-backoff-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();

        let key_control = control.clone();
        let key_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let key_addr = key_listener.local_addr().unwrap();
        let key_server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = key_listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let _ = std::io::Write::write_all(
                    &mut stream,
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                let _ = fs::write(&key_control, "finish");
            }
        });
        let media_key = MediaKey {
            method: "AES-128".into(),
            uri: format!("http://127.0.0.1:{}/live.key", key_addr.port()),
            iv: Some([0; 16]),
        };
        let key_err = load_key(
            &media_key,
            &HashMap::new(),
            "",
            &control,
            &mut HashMap::new(),
        )
        .unwrap_err();
        key_server.join().unwrap();
        assert_eq!(key_err, "finish");

        fs::write(&control, "run").unwrap();
        let seg_control = control.clone();
        let seg_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let seg_addr = seg_listener.local_addr().unwrap();
        let seg_server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = seg_listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let _ = std::io::Write::write_all(
                    &mut stream,
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                let _ = fs::write(&seg_control, "finish");
            }
        });
        let seg = Segment {
            uri: format!("http://127.0.0.1:{}/seg0.ts", seg_addr.port()),
            media_sequence: 0,
            part_index: None,
            key: None,
            init_map: None,
            gap: false,
            duration: 2.0,
            discontinuity: false,
            byterange: None,
            is_part: false,
            is_ad: false,
        };
        let seg_err = download_segment(
            &seg,
            &HashMap::new(),
            "",
            &root.join("seg0.ts"),
            &control,
            None,
            None,
            0,
            true,
        )
        .unwrap_err();
        seg_server.join().unwrap();
        assert_eq!(seg_err, "finish");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stopped_live_hls_writes_endlist_to_final_playlist_while_keeping_pause_open() {
        let root = std::env::temp_dir().join(format!(
            "hls-g3-1-endlist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let ctrl = control.clone();
        let mode = Arc::new(AtomicUsize::new(0));
        let server_mode = Arc::clone(&mode);
        let origin = super::super::harness::serve_dynamic(Arc::new(move |path, _| match path {
            "/live.m3u8" => {
                let step = server_mode.fetch_add(1, Ordering::SeqCst);
                if step == 1 {
                    let _ = fs::write(&ctrl, "pause");
                } else if step >= 3 {
                    let _ = fs::write(&ctrl, "finish");
                }
                Some(
                    b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\ns0.ts\n"
                        .to_vec(),
                )
            }
            "/s0.ts" => Some(b"LIVE-SEG-0".to_vec()),
            _ => None,
        }));

        let paused = download_hls_with(
            &format!("{}/live.m3u8", origin.base),
            &HashMap::new(),
            "",
            &root,
            &control,
            HlsDownloadOptions {
                live: true,
                download_subtitles: false,
                remux_ts_to_mp4: false,
                ..HlsDownloadOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(paused, "paused");
        let paused_playlist = fs::read_to_string(root.join("local.m3u8")).unwrap();
        assert!(paused_playlist.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(!paused_playlist.contains("#EXT-X-ENDLIST"));

        fs::write(&control, "run").unwrap();
        let out = download_hls_with(
            &format!("{}/live.m3u8", origin.base),
            &HashMap::new(),
            "",
            &root,
            &control,
            HlsDownloadOptions {
                live: true,
                download_subtitles: false,
                remux_ts_to_mp4: false,
                ..HlsDownloadOptions::default()
            },
        )
        .unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"LIVE-SEG-0");
        let final_playlist = fs::read_to_string(root.join("local.m3u8")).unwrap();
        assert!(final_playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(final_playlist.contains("#EXT-X-ENDLIST"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_audio_recorder_finish_aborts_blocking_ll_hls_reload_and_honors_parent_control() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;

        // 1. Graceful finish wakes an in-flight 30s LL-HLS blocking reload immediately and finalizes recorded audio.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut reader = BufReader::new(&mut stream);
                let mut req_line = String::new();
                if reader.read_line(&mut req_line).unwrap_or(0) == 0 {
                    continue;
                }
                let target = req_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                drop(reader);
                if target.contains("_HLS_msn=") {
                    let _ = blocked_tx.send(());
                    // Hold LL-HLS blocking reload open until test finishes!
                    let _ = release_rx.recv_timeout(Duration::from_secs(30));
                    break;
                } else if target.starts_with("/audio.m3u8") {
                    let body = b"#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\na0.ts\n";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                } else if target == "/a0.ts" {
                    let body = b"AUDIO-SEG-0";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                }
            }
        });

        let root = std::env::temp_dir().join(format!(
            "hls-g3-2-audio-finish-{}-{}",
            std::process::id(),
            addr.port()
        ));
        fs::create_dir_all(&root).unwrap();
        let parent_control = root.join("control");
        fs::write(&parent_control, "run").unwrap();
        let mut recorder = LiveAudioRecorder::start_with_parent(
            &format!("http://{addr}/audio.m3u8"),
            &HashMap::new(),
            "hls-downloader://direct-proxy",
            &root,
            &parent_control,
        );
        blocked_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("audio worker must enter LL-HLS blocking reload");
        let started = Instant::now();
        let merged = recorder
            .finish()
            .unwrap()
            .expect("recorded audio must publish");
        let elapsed = started.elapsed();
        let _ = release_tx.send(());
        server.join().unwrap();
        assert!(
            elapsed < Duration::from_secs(3),
            "LiveAudioRecorder::finish stalled on LL-HLS blocking reload: {elapsed:?}"
        );
        assert!(merged.is_file());
        let audio_playlist = fs::read_to_string(root.join("audio/local.m3u8")).unwrap();
        assert!(audio_playlist.contains("#EXT-X-ENDLIST"));

        // 2. While finish() waits on a slow worker, parent "pause" and "cancel" return promptly and clear finishing.
        for (parent_word, expected_err) in [("pause", "paused"), ("cancel", "canceled")] {
            let sub = root.join(parent_word);
            fs::create_dir_all(&sub).unwrap();
            let p_ctrl = sub.join("control");
            fs::write(&p_ctrl, "run").unwrap();
            let (stop_worker_tx, stop_worker_rx) = mpsc::channel();
            let finishing = Arc::new(AtomicBool::new(false));
            let mut slow_recorder = LiveAudioRecorder {
                control: sub.join("audio.control"),
                parent_control: p_ctrl.clone(),
                finishing: Arc::clone(&finishing),
                handle: Some(std::thread::spawn(move || {
                    let _ = stop_worker_rx.recv_timeout(Duration::from_secs(10));
                    Err("canceled".into())
                })),
            };
            let trigger_ctrl = p_ctrl.clone();
            let signaler = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(60));
                fs::write(&trigger_ctrl, parent_word).unwrap();
            });
            let wait_started = Instant::now();
            let err = slow_recorder.finish().unwrap_err();
            let wait_elapsed = wait_started.elapsed();
            let _ = stop_worker_tx.send(());
            signaler.join().unwrap();
            assert_eq!(err, expected_err);
            assert!(!finishing.load(Ordering::SeqCst));
            assert!(
                wait_elapsed < Duration::from_secs(2),
                "finish() ignored parent {parent_word}: {wait_elapsed:?}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ll_hls_ad_policy_and_timeline_insert_preserve_discontinuity_across_parts() {
        // 1. apply_ad_policy preserves pending_gap for parent full segment when EXT-X-PART entries precede it.
        let text = concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA-SEQUENCE:10\n",
            "#EXT-X-PART-INF:PART-TARGET=0.5\n",
            "#EXTINF:1.0,\n",
            "main10.m4s\n",
            "#EXT-X-CUE-OUT:2\n",
            "#EXTINF:1.0,\n",
            "ad11.m4s\n",
            "#EXT-X-CUE-IN\n",
            "#EXT-X-PART:DURATION=0.5,URI=\"main12.0.m4s\"\n",
            "#EXT-X-PART:DURATION=0.5,URI=\"main12.1.m4s\"\n",
            "#EXTINF:1.0,\n",
            "main12.m4s\n",
            "#EXTINF:1.0,\n",
            "main13.m4s\n",
        );
        let mut playlist = parse_playlist(text, "https://cdn.test/live.m3u8").unwrap();
        apply_ad_policy(&mut playlist, true);
        assert_eq!(playlist.segments.len(), 5);
        assert!(playlist.segments[1].is_part && playlist.segments[1].discontinuity);
        assert!(playlist.segments[2].is_part && !playlist.segments[2].discontinuity);
        assert!(
            !playlist.segments[3].is_part && playlist.segments[3].discontinuity,
            "parent full segment after ad-filtered parts must retain discontinuity"
        );
        assert!(!playlist.segments[4].discontinuity);

        let pending = pending_segments(&playlist, &[]);
        assert_eq!(pending.len(), 3);
        assert!(
            pending[1].discontinuity,
            "selected full segment main12.m4s must carry discontinuity"
        );

        // 2. LiveTimeline::insert preserves existing.discontinuity when full segment replaces parts.
        let root = std::env::temp_dir().join(format!(
            "hls-g3-3-discont-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("segments")).unwrap();
        let part0 = root.join("segments/000000.ts");
        let part1 = root.join("segments/000001.ts");
        let full = root.join("segments/000002.ts");
        fs::write(&part0, b"P0").unwrap();
        fs::write(&part1, b"P1").unwrap();
        fs::write(&full, b"FULL").unwrap();
        let mut timeline = LiveTimeline::default();
        timeline.insert(TimelineEntry {
            identity: "id-p0".into(),
            uri: "https://cdn.test/12.0.ts".into(),
            media_sequence: 12,
            part_index: Some(0),
            path: part0.clone(),
            duration: 0.5,
            discontinuity: true,
            init_map_path: None,
        });
        timeline.insert(TimelineEntry {
            identity: "id-p1".into(),
            uri: "https://cdn.test/12.1.ts".into(),
            media_sequence: 12,
            part_index: Some(1),
            path: part1.clone(),
            duration: 0.5,
            discontinuity: false,
            init_map_path: None,
        });
        timeline.insert(TimelineEntry {
            identity: "id-full".into(),
            uri: "https://cdn.test/12.ts".into(),
            media_sequence: 12,
            part_index: None,
            path: full,
            duration: 1.0,
            discontinuity: false,
            init_map_path: None,
        });
        assert_eq!(timeline.entries.len(), 1);
        assert!(
            timeline.entries[0].discontinuity,
            "full segment replacing discontinuous part must inherit discontinuity"
        );
        assert!(!part0.exists() && !part1.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_hls_sequence_reset_appends_monotonically_without_overwriting_and_persists_epoch() {
        let root = std::env::temp_dir().join(format!(
            "hls-g3-4-seq-reset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("segments")).unwrap();
        let p_before = parse_playlist(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:2.0,\nepoch1/s100.ts\n#EXTINF:2.0,\nepoch1/s101.ts\n",
            "https://cdn.test/live.m3u8",
        )
        .unwrap();
        let mut timeline = LiveTimeline::default();
        timeline.observe_playlist(&p_before);
        assert_eq!(timeline.sequence_offset, 0);
        for (slot, seg) in p_before.segments.iter().enumerate() {
            let path = root.join(format!("segments/{slot:06}.ts"));
            fs::write(&path, format!("E1-{slot}")).unwrap();
            timeline.insert(TimelineEntry {
                identity: live_segment_identity(seg, timeline.sequence_offset),
                uri: stable_media_url(&seg.uri),
                media_sequence: timeline.effective_sequence(seg.media_sequence),
                part_index: seg.part_index,
                path,
                duration: seg.duration,
                discontinuity: seg.discontinuity,
                init_map_path: None,
            });
        }
        timeline.next_slot = 2;

        // Encoder reconnects and resets #EXT-X-MEDIA-SEQUENCE back to 0 with new segment URIs.
        let p_reset = parse_playlist(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\nepoch2/s0.ts\n#EXTINF:2.0,\nepoch2/s1.ts\n",
            "https://cdn.test/live.m3u8",
        )
        .unwrap();
        timeline.observe_playlist(&p_reset);
        assert_eq!(timeline.sequence_offset, 102);
        assert!(timeline.pending_epoch_discontinuity);

        let pending = pending_live_segments(&p_reset, &timeline);
        assert_eq!(pending.len(), 2);
        for (idx, seg) in pending.iter().enumerate() {
            let slot = timeline.next_slot;
            timeline.next_slot += 1;
            let path = root.join(format!("segments/{slot:06}.ts"));
            fs::write(&path, format!("E2-{idx}")).unwrap();
            let discont =
                seg.discontinuity || std::mem::take(&mut timeline.pending_epoch_discontinuity);
            timeline.insert(TimelineEntry {
                identity: live_segment_identity(seg, timeline.sequence_offset),
                uri: stable_media_url(&seg.uri),
                media_sequence: timeline.effective_sequence(seg.media_sequence),
                part_index: seg.part_index,
                path,
                duration: seg.duration,
                discontinuity: discont,
                init_map_path: None,
            });
        }

        // Persist and reload live_state.json to verify resume keeps epoch offset and order.
        let state_path = root.join("live_state.json");
        timeline.save(&state_path).unwrap();
        let reloaded = LiveTimeline::load(&root);
        assert_eq!(reloaded.sequence_offset, 102);
        assert_eq!(reloaded.entries.len(), 4);
        let seqs: Vec<u64> = reloaded.entries.iter().map(|e| e.media_sequence).collect();
        assert_eq!(seqs, vec![100, 101, 102, 103]);
        assert!(reloaded.entries[2].discontinuity);
        assert!(!reloaded.entries[3].discontinuity);
        let payloads: Vec<String> = reloaded
            .entries
            .iter()
            .map(|e| fs::read_to_string(&e.path).unwrap())
            .collect();
        assert_eq!(payloads, vec!["E1-0", "E1-1", "E2-0", "E2-1"]);
        let _ = fs::remove_dir_all(root);
    }
}
