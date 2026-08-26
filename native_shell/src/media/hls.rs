//! HLS / LL-HLS downloader. Segments use the HTTP engine; mux is local.

use crate::http_engine::{fetch_hls_bytes, fetch_hls_bytes_range};
use crate::media::merge::{concat_files, merge_with_ffmpeg, mux_av};
use crate::media::subtitles::{has_cues, merge_webvtt_segments, webvtt_to_srt};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
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
    if !text.contains("#EXTM3U") {
        return Err("not an HLS playlist".into());
    }
    let mut playlist = Playlist {
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
    let mut pending_bandwidth = 0u64;
    let mut pending_codecs = String::new();
    let mut pending_resolution = (0u32, 0u32);
    let mut pending_audio_group = String::new();
    let mut pending_duration = 0.0;
    let mut pending_range: Option<(Option<u64>, u64)> = None;
    let mut media_range_ends = HashMap::new();
    let mut map_range_ends = HashMap::new();
    let mut current_key: Option<MediaKey> = None;
    let mut current_map: Option<InitMap> = None;
    let mut complete_segments = 0u64;
    let mut part_index = 0u64;
    let mut discontinuity = false;
    let mut pending_gap = false;
    let mut ad_cue_active = false;
    let mut pending_daterange_ad = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(value) = tag_value(line, "#EXT-X-TARGETDURATION:") {
            playlist.target_duration = value.parse().unwrap_or(6.0);
        } else if let Some(value) = tag_value(line, "#EXT-X-MEDIA-SEQUENCE:") {
            playlist.media_sequence = value.parse().unwrap_or(0);
        } else if let Some(attrs) = tag_value(line, "#EXT-X-SERVER-CONTROL:") {
            playlist.can_block_reload = attr_str(attrs, "CAN-BLOCK-RELOAD")
                .is_some_and(|value| value.eq_ignore_ascii_case("YES"));
        } else if let Some(attrs) = tag_value(line, "#EXT-X-SKIP:") {
            playlist.skipped_segments = attr_u64(attrs, "SKIPPED-SEGMENTS").unwrap_or(0);
        } else if let Some(attrs) = tag_value(line, "#EXT-X-PART-INF:") {
            playlist.part_target = attr_f64(attrs, "PART-TARGET").unwrap_or(0.0);
        } else if line == "#EXT-X-ENDLIST" {
            playlist.end_list = true;
        } else if line == "#EXT-X-DISCONTINUITY" {
            discontinuity = true;
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
                .map(|uri| resolve(base, &uri))
                .filter(|uri| !uri.is_empty());
            let byterange = match (uri.as_deref(), attr_str(attrs, "BYTERANGE")) {
                (Some(uri), Some(value)) => Some(resolve_byterange(
                    parse_byterange(&value)?,
                    uri,
                    &mut map_range_ends,
                )?),
                _ => None,
            };
            playlist.map_uri = uri.clone();
            playlist.map_byterange = byterange;
            current_map = uri.map(|uri| InitMap {
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
                    .map(|uri| resolve(base, &uri))
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
            pending_duration = value
                .split(',')
                .next()
                .and_then(|item| item.parse().ok())
                .unwrap_or(0.0);
        } else if let Some(value) = tag_value(line, "#EXT-X-BYTERANGE:") {
            pending_range = Some(parse_byterange(value)?);
        } else if let Some(attrs) = tag_value(line, "#EXT-X-PART:") {
            if let Some(uri) = attr_str(attrs, "URI") {
                let resolved = resolve(base, &uri);
                if resolved.is_empty() {
                    pending_daterange_ad = false;
                    discontinuity = false;
                    pending_gap = false;
                    pending_range = None;
                    continue;
                }
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
                    duration: attr_f64(attrs, "DURATION").unwrap_or(0.0),
                    discontinuity,
                    byterange: match attr_str(attrs, "BYTERANGE") {
                        Some(value) => Some(resolve_byterange(
                            parse_byterange(&value)?,
                            &resolved,
                            &mut media_range_ends,
                        )?),
                        None => None,
                    },
                    is_part: true,
                    is_ad: ad_cue_active || pending_daterange_ad || url_is_ad(&resolved),
                });
                part_index = part_index.saturating_add(1);
                pending_daterange_ad = false;
                discontinuity = false;
                pending_gap = false;
                pending_range = None;
            }
        } else if let Some(attrs) = tag_value(line, "#EXT-X-MEDIA:") {
            playlist.renditions.push(Rendition {
                kind: attr_str(attrs, "TYPE").unwrap_or_default(),
                group_id: attr_str(attrs, "GROUP-ID").unwrap_or_default(),
                name: attr_str(attrs, "NAME").unwrap_or_default(),
                language: attr_str(attrs, "LANGUAGE").unwrap_or_default(),
                uri: attr_str(attrs, "URI")
                    .map(|uri| resolve(base, &uri))
                    .filter(|uri| !uri.is_empty()),
                default: attr_str(attrs, "DEFAULT")
                    .is_some_and(|value| value.eq_ignore_ascii_case("YES")),
            });
        } else if !line.starts_with('#') {
            let uri = resolve(base, line);
            if uri.is_empty() {
                pending_bandwidth = 0;
                pending_codecs.clear();
                pending_resolution = (0, 0);
                pending_audio_group.clear();
                pending_daterange_ad = false;
                pending_duration = 0.0;
                pending_range = None;
                discontinuity = false;
                pending_gap = false;
                continue;
            }
            if playlist.is_master || pending_bandwidth > 0 {
                playlist.is_master = true;
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
                pending_gap = false;
                complete_segments = complete_segments.saturating_add(1);
                part_index = 0;
            }
        }
    }
    Ok(playlist)
}

pub fn select_variant(playlist: &Playlist) -> Option<&Variant> {
    select_variant_for(playlist, 0, 0)
}

pub fn select_variant_for(
    playlist: &Playlist,
    preferred_bandwidth: u64,
    preferred_height: u32,
) -> Option<&Variant> {
    if playlist.variants.is_empty() {
        return None;
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
    let mut choices: Vec<_> = playlist
        .renditions
        .iter()
        .filter(|item| item.kind.eq_ignore_ascii_case("AUDIO") && item.uri.is_some())
        .map(|item| crate::StreamVariant {
            label: if item.language.is_empty() {
                item.name.clone()
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
                && item.uri.is_some()
        })
        .collect();
    if !preferred.trim().is_empty() {
        if let Some(hit) = tracks.iter().copied().find(|item| {
            item.name.eq_ignore_ascii_case(preferred)
                || item.language.eq_ignore_ascii_case(preferred)
        }) {
            return Some(hit);
        }
    }
    tracks.sort_by_key(|item| (!item.default, item.language.clone(), item.name.clone()));
    tracks.into_iter().next()
}

pub fn select_default_audio<'a>(
    playlist: &'a Playlist,
    variant: &Variant,
) -> Option<&'a Rendition> {
    select_audio_track(playlist, variant, "")
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
    pub preferred_audio: String,
    pub skip_ads: bool,
    pub download_subtitles: bool,
    pub live_max_minutes: u64,
    pub progress: Option<PathBuf>,
}

impl Default for HlsDownloadOptions {
    fn default() -> Self {
        Self {
            live: false,
            concurrency: 8,
            preferred_bandwidth: 0,
            preferred_height: 0,
            preferred_audio: String::new(),
            skip_ads: true,
            download_subtitles: true,
            live_max_minutes: 0,
            progress: None,
        }
    }
}

#[derive(Debug, Clone)]
struct TimelineEntry {
    identity: String,
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
}

struct PreparedSegment {
    segment: Segment,
    identity: String,
    slot: usize,
    path: PathBuf,
    init_map_path: Option<PathBuf>,
    key_bytes: Option<Vec<u8>>,
    needs_download: bool,
}

struct SegmentWork {
    segment: Segment,
    path: PathBuf,
    key_bytes: Option<Vec<u8>>,
}

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

pub fn download_hls_selected(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    live: bool,
    preferred_bandwidth: u64,
    preferred_height: u32,
) -> Result<PathBuf, String> {
    download_hls_with(
        url,
        headers,
        proxy,
        task_dir,
        control,
        HlsDownloadOptions {
            live,
            preferred_bandwidth,
            preferred_height,
            ..HlsDownloadOptions::default()
        },
    )
}

pub fn download_hls_with(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    options: HlsDownloadOptions,
) -> Result<PathBuf, String> {
    let live = options.live;
    fs::create_dir_all(task_dir).map_err(|error| error.to_string())?;
    let mut current = url.to_string();
    let mut playlist = load_playlist(&current, headers, proxy)?;
    let mut audio_uri = None;
    let mut subtitle_tracks = Vec::new();
    if playlist.is_master {
        let variant = select_variant_for(
            &playlist,
            options.preferred_bandwidth,
            options.preferred_height,
        )
        .ok_or_else(|| "HLS master has no variants".to_string())?;
        audio_uri = select_audio_track(&playlist, variant, &options.preferred_audio)
            .and_then(|track| track.uri.clone());
        if options.download_subtitles {
            subtitle_tracks = select_subtitles(&playlist);
        }
        current = variant.uri.clone();
        playlist = load_playlist(&current, headers, proxy)?;
    }
    apply_ad_policy(&mut playlist, options.skip_ads);
    let seg_dir = task_dir.join("segments");
    fs::create_dir_all(&seg_dir).map_err(|error| error.to_string())?;
    let mut vod = VodCheckpoint::load(task_dir);
    let state_path = task_dir.join("live_state.json");
    let mut timeline = if live {
        LiveTimeline::load(task_dir)
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
    let mut live_audio = if live {
        audio_uri
            .as_ref()
            .map(|uri| LiveAudioRecorder::start(uri, headers, proxy, task_dir))
    } else {
        None
    };
    let mut live_subs = if live && options.download_subtitles {
        LiveSubSession::load(task_dir, &subtitle_tracks)
    } else {
        LiveSubSession::default()
    };
    loop {
        if read_control(control) == "cancel" {
            return Err("canceled".into());
        }
        if read_control(control) == "pause" {
            return Err("paused".into());
        }
        if live && options.download_subtitles {
            live_subs.capture(headers, proxy, task_dir, control);
        }
        let mut pending = pending_segments(&playlist, &timeline.entries);
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
        let prepared = prepare_segments(
            pending,
            headers,
            proxy,
            task_dir,
            control,
            live,
            &vod,
            &mut timeline.next_slot,
            &mut key_cache,
            &mut map_cache,
        )?;
        download_prepared_segments(
            &prepared,
            headers,
            proxy,
            control,
            options.concurrency.max(1),
        )?;
        for prepared in prepared {
            if !live {
                vod.remember(prepared.slot, &prepared.identity, file_len(&prepared.path));
            }
            let duration = prepared.segment.duration.max(0.0);
            timeline.insert(TimelineEntry {
                identity: prepared.identity,
                media_sequence: prepared.segment.media_sequence,
                part_index: prepared.segment.part_index,
                path: prepared.path,
                duration,
                discontinuity: prepared.segment.discontinuity,
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
            crate::http_engine::write_progress(
                progress,
                downloaded,
                0,
                0.0,
                if live { "recording" } else { "downloading" },
            );
        }
        if live_limit.is_some_and(|limit| recorded_duration >= limit) {
            break;
        }
        if playlist.end_list || !live {
            break;
        }
        if playlist.can_block_reload {
            playlist = reload_playlist(&current, headers, proxy, &playlist)?;
        } else {
            let wait = if playlist.part_target > 0.0 {
                playlist.part_target
            } else {
                (playlist.target_duration / 2.0).max(0.2)
            };
            wait_with_control(control, wait.min(6.0))?;
            playlist = load_playlist(&current, headers, proxy)?;
        }
        apply_ad_policy(&mut playlist, options.skip_ads);
        if playlist.end_list {
            continue;
        }
        if read_control(control) != "run" {
            break;
        }
    }
    if playlist.end_list || !live {
        vod.save(task_dir)?;
    }
    write_timeline_playlist(
        task_dir,
        playlist.target_duration,
        &timeline.entries,
        playlist.end_list || !live,
    )?;
    let mut audio_merged = None;
    if let Some(recorder) = live_audio.as_mut() {
        audio_merged = recorder.finish();
    } else if let Some(audio) = audio_uri {
        let audio_dir = task_dir.join("audio");
        audio_merged = download_hls(&audio, headers, proxy, &audio_dir, control, false).ok();
    }
    let mut subtitles = Vec::new();
    if live && options.download_subtitles {
        subtitles = live_subs.publish(task_dir);
    } else {
        for track in subtitle_tracks {
            if let Some(uri) = &track.uri {
                let sub_dir = task_dir.join("subs");
                fs::create_dir_all(&sub_dir).map_err(|error| error.to_string())?;
                let lang = safe_label(&track.language, "und");
                if uri.contains(".m3u8") {
                    if let Ok(merged) = download_hls(uri, headers, proxy, &sub_dir, control, false)
                    {
                        subtitles.push(merged);
                    }
                } else {
                    let path = sub_dir.join(format!("{lang}.vtt"));
                    if download_one(uri, headers, proxy, &path, control).is_ok() {
                        subtitles.push(path);
                    }
                }
            }
        }
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
            && extension(entry.path.to_string_lossy().as_ref()) == "ts"
    });
    if can_concat {
        concat_files(&files, &output)?;
    } else {
        merge_with_ffmpeg(task_dir, &output)?;
    }
    if audio_merged.is_some() || !subtitles.is_empty() {
        let muxed = task_dir.join("muxed.mp4");
        mux_av(&output, audio_merged.as_deref(), &subtitles, &muxed)?;
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
        Self { entries, next_slot }
    }

    fn insert(&mut self, entry: TimelineEntry) {
        if self
            .entries
            .iter()
            .any(|existing| existing.identity == entry.identity)
        {
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
                removed.push(existing.path.clone());
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
            &serde_json::json!({ "version": 2, "segments": segments }).to_string(),
        )
    }
}

fn write_timeline_playlist(
    task_dir: &Path,
    target_duration: f64,
    entries: &[TimelineEntry],
    complete: bool,
) -> Result<(), String> {
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
    write_text_atomic(&task_dir.join("local.m3u8"), &text)
}

fn reload_playlist(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    previous: &Playlist,
) -> Result<Playlist, String> {
    let blocking_url = blocking_reload_url(url, previous);
    match load_playlist(&blocking_url, headers, proxy) {
        Ok(playlist) if !playlist.segments.is_empty() || playlist.end_list => Ok(playlist),
        Ok(_) | Err(_) => load_playlist(url, headers, proxy),
    }
}

fn blocking_reload_url(url: &str, previous: &Playlist) -> String {
    let last = previous.segments.last();
    let (msn, part) = match last.and_then(|segment| segment.part_index) {
        Some(part) => (
            last.map(|segment| segment.media_sequence)
                .unwrap_or_else(|| {
                    previous
                        .media_sequence
                        .saturating_add(previous.skipped_segments)
                }),
            Some(part.saturating_add(1)),
        ),
        None => (
            last.map(|segment| segment.media_sequence.saturating_add(1))
                .unwrap_or_else(|| {
                    previous
                        .media_sequence
                        .saturating_add(previous.skipped_segments)
                }),
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
        match read_control(control).as_str() {
            "cancel" => return Err("canceled".into()),
            "pause" => return Err("paused".into()),
            _ => {}
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        std::thread::sleep(remaining.min(std::time::Duration::from_millis(100)));
    }
    Ok(())
}

fn write_json_atomic(path: &Path, text: &str) -> Result<(), String> {
    write_text_atomic(path, text)
}

fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
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

#[cfg(windows)]
fn replace_file_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
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
    let full_in_playlist: BTreeSet<u64> = playlist
        .segments
        .iter()
        .filter(|segment| segment.part_index.is_none())
        .map(|segment| segment.media_sequence)
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
            !segment.is_part
                || !full_in_playlist.contains(&segment.media_sequence)
                    && !full_downloaded.contains(&segment.media_sequence)
        })
        .filter(|segment| !identities.contains(segment_identity(segment).as_str()))
        .filter(|segment| positions.insert((segment.media_sequence, segment.part_index)))
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
    vod: &VodCheckpoint,
    next_slot: &mut usize,
    key_cache: &mut HashMap<String, Vec<u8>>,
    map_cache: &mut HashMap<String, PathBuf>,
) -> Result<Vec<PreparedSegment>, String> {
    let seg_dir = task_dir.join("segments");
    let mut prepared = Vec::new();
    for segment in segments {
        let slot = *next_slot;
        *next_slot = next_slot.saturating_add(1);
        let identity = segment_identity(&segment);
        let init_map_path = match &segment.init_map {
            Some(init_map) => Some(ensure_init_map(
                init_map,
                segment.media_sequence,
                headers,
                proxy,
                &seg_dir,
                control,
                key_cache,
                map_cache,
            )?),
            None => None,
        };
        let key_bytes = match &segment.key {
            Some(key) => Some(load_key(key, headers, proxy, key_cache)?),
            None => None,
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
                Some(existing) if vod.has_slot(slot) => {
                    let _ = fs::remove_file(existing);
                    (output, true)
                }
                Some(existing) => (existing, false),
                None => (output, true),
            }
        };
        prepared.push(PreparedSegment {
            segment,
            identity,
            slot,
            path,
            init_map_path,
            key_bytes,
            needs_download,
        });
    }
    Ok(prepared)
}

fn download_prepared_segments(
    prepared: &[PreparedSegment],
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    concurrency: usize,
) -> Result<(), String> {
    let queue: VecDeque<SegmentWork> = prepared
        .iter()
        .filter(|item| item.needs_download)
        .map(|item| SegmentWork {
            segment: item.segment.clone(),
            path: item.path.clone(),
            key_bytes: item.key_bytes.clone(),
        })
        .collect();
    if queue.is_empty() {
        return Ok(());
    }
    let worker_count = concurrency.min(queue.len()).max(1);
    let queue = Arc::new(Mutex::new(queue));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let replay = crate::credentials::scoped_replay_json();
    let throttle = crate::net_policy::current_throttle_context();
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let errors = Arc::clone(&errors);
            let replay = replay.clone();
            let throttle = throttle.clone();
            scope.spawn(move || {
                crate::net_policy::with_throttle_context(throttle, || {
                    crate::with_replay_json(&replay, || loop {
                        let work = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                        let Some(work) = work else {
                            break;
                        };
                        if let Err(error) = download_segment(
                            &work.segment,
                            headers,
                            proxy,
                            &work.path,
                            control,
                            work.key_bytes.as_ref(),
                            work.segment.key.as_ref(),
                            work.segment.media_sequence,
                        ) {
                            if let Ok(mut errors) = errors.lock() {
                                errors.push(error);
                            }
                            break;
                        }
                    })
                });
            });
        }
    });
    let errors = errors.lock().map_err(|_| "HLS worker mutex poisoned")?;
    if let Some(error) = errors.first() {
        return Err(error.clone());
    }
    if let Some(missing) = prepared.iter().find(|item| file_len(&item.path) == 0) {
        return Err(format!("HLS 分片下载结果为空: {}", missing.segment.uri));
    }
    Ok(())
}

fn load_key(
    key: &MediaKey,
    headers: &HashMap<String, String>,
    proxy: &str,
    cache: &mut HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let cache_key = format!("{}:{:?}", key.uri, key.iv);
    if let Some(bytes) = cache.get(&cache_key) {
        return Ok(bytes.clone());
    }
    let (status, bytes) =
        fetch_hls_bytes(&key.uri, headers, proxy).map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Err(format!("HLS key HTTP {status}"));
    }
    if bytes.len() != 16 {
        return Err(format!(
            "AES-128 key 长度必须为 16 字节，实际为 {}",
            bytes.len()
        ));
    }
    cache.insert(cache_key, bytes.clone());
    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
fn ensure_init_map(
    init_map: &InitMap,
    sequence: u64,
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
    if file_len(&path) == 0 {
        download_one_with_range(
            &init_map.uri,
            headers,
            proxy,
            &path,
            control,
            init_map.byterange,
        )?;
        if let Some(key) = &init_map.key {
            let key_bytes = load_key(key, headers, proxy, key_cache)?;
            let iv = key.iv.unwrap_or_else(|| sequence_iv(sequence));
            let encrypted = fs::read(&path).map_err(|error| error.to_string())?;
            let plain = decrypt_aes128(&key_bytes, &iv, &encrypted)?;
            fs::write(&path, plain).map_err(|error| error.to_string())?;
        }
    }
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

struct LiveAudioRecorder {
    control: PathBuf,
    handle: Option<std::thread::JoinHandle<Result<PathBuf, String>>>,
}

impl LiveAudioRecorder {
    fn start(uri: &str, headers: &HashMap<String, String>, proxy: &str, task_dir: &Path) -> Self {
        let control = task_dir.join("audio.control");
        let _ = fs::write(&control, "run");
        let uri = uri.to_string();
        let headers = headers.clone();
        let proxy = proxy.to_string();
        let audio_dir = task_dir.join("audio");
        let thread_control = control.clone();
        let replay = crate::credentials::scoped_replay_json();
        let throttle = crate::net_policy::current_throttle_context();
        let handle = std::thread::spawn(move || {
            crate::net_policy::with_throttle_context(throttle, || {
                crate::with_replay_json(&replay, || {
                    download_hls(&uri, &headers, &proxy, &audio_dir, &thread_control, true)
                })
            })
        });
        Self {
            control,
            handle: Some(handle),
        }
    }

    fn finish(&mut self) -> Option<PathBuf> {
        let _ = fs::write(&self.control, "finish");
        self.handle
            .take()
            .and_then(|handle| handle.join().ok())
            .and_then(Result::ok)
    }
}

impl Drop for LiveAudioRecorder {
    fn drop(&mut self) {
        if self.handle.is_some() {
            let _ = self.finish();
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
        let _ = fs::write(
            task_dir.join("live_subtitles.json"),
            serde_json::json!({ "tracks": tracks }).to_string(),
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
    safe_label(raw, "sub")
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
    let (status, body) =
        fetch_hls_bytes(&track.uri, headers, proxy).map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Ok(());
    }
    let cache = task_dir.join("live-subtitles").join(&track.key);
    fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&body);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("WEBVTT") {
        let identity = crate::crypto_lite::sha256_hex(&body);
        if track.seen.insert(identity) {
            let path = cache.join(format!("{:08}.vtt", track.count));
            fs::write(&path, &body).map_err(|error| error.to_string())?;
            track.count += 1;
        }
        return Ok(());
    }
    let playlist = match parse_playlist(&text, &track.uri) {
        Ok(parsed) if !parsed.is_master => parsed,
        _ => return Ok(()),
    };
    for segment in playlist.segments {
        if !track.seen.insert(segment.uri.clone()) {
            continue;
        }
        let path = cache.join(format!("{:08}.vtt", track.count));
        if download_one(&segment.uri, headers, proxy, &path, control).is_ok() && file_len(&path) > 0
        {
            track.count += 1;
        } else {
            track.seen.remove(&segment.uri);
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

fn load_playlist(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
) -> Result<Playlist, String> {
    let (status, body) = fetch_hls_bytes(url, headers, proxy).map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Err(format!("HLS playlist HTTP {status}"));
    }
    let text = String::from_utf8_lossy(&body);
    parse_playlist(&text, url)
}

fn download_segment(
    segment: &Segment,
    headers: &HashMap<String, String>,
    proxy: &str,
    path: &Path,
    control: &Path,
    key_bytes: Option<&Vec<u8>>,
    media_key: Option<&MediaKey>,
    sequence: u64,
) -> Result<PathBuf, String> {
    download_one_with_range(
        &segment.uri,
        headers,
        proxy,
        path,
        control,
        segment.byterange,
    )?;
    if let (Some(key), Some(media_key)) = (key_bytes, media_key) {
        if media_key.method.eq_ignore_ascii_case("AES-128") {
            let iv = media_key.iv.unwrap_or_else(|| sequence_iv(sequence));
            let encrypted = fs::read(path).map_err(|error| error.to_string())?;
            let plain = decrypt_aes128(key, &iv, &encrypted)?;
            fs::write(path, plain).map_err(|error| error.to_string())?;
        }
    }
    Ok(path.to_path_buf())
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

    fn has_slot(&self, slot: usize) -> bool {
        self.records.contains_key(&slot.to_string())
    }

    fn can_reuse(&self, slot: usize, identity: &str, size: u64) -> bool {
        if size == 0 {
            return false;
        }
        match self.records.get(&slot.to_string()) {
            Some((saved, saved_size)) => saved == identity && *saved_size == size,
            None => true,
        }
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
        fs::write(task_dir.join("vod_segments.json"), payload.to_string())
            .map_err(|error| error.to_string())
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
    let path = if let Some(idx) = left.find("://") {
        let rest = &left[idx + 3..];
        rest.find('/')
            .map(|at| rest[at..].trim_end_matches('/'))
            .unwrap_or("")
    } else {
        left.trim_end_matches('/')
    };
    let path = if path.is_empty() { "/" } else { path };
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
        path.to_string()
    } else {
        format!("{path}?{query}")
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

fn download_one(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    output: &Path,
    control: &Path,
) -> Result<PathBuf, String> {
    match read_control(control).as_str() {
        "cancel" => return Err("canceled".into()),
        "pause" => return Err("paused".into()),
        _ => {}
    }
    let (status, body) = fetch_hls_bytes(url, headers, proxy).map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Err(format!("HLS media HTTP {status}"));
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(output, body).map_err(|error| error.to_string())?;
    Ok(output.to_path_buf())
}

fn download_one_with_range(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    output: &Path,
    control: &Path,
    byterange: Option<(u64, u64)>,
) -> Result<PathBuf, String> {
    let Some((offset, length)) = byterange else {
        return download_one(url, headers, proxy, output, control);
    };
    match read_control(control).as_str() {
        "cancel" => return Err("canceled".into()),
        "pause" => return Err("paused".into()),
        _ => {}
    }
    let body = fetch_hls_bytes_range(url, headers, proxy, offset, length)
        .map_err(|error| error.to_string())?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(output, body).map_err(|error| error.to_string())?;
    Ok(output.to_path_buf())
}

fn write_local_playlist(
    task_dir: &Path,
    target_duration: f64,
    has_map: bool,
    files: &[PathBuf],
    durations: &[f64],
    discontinuities: &[bool],
    complete: bool,
) -> Result<(), String> {
    let mut text = String::from("#EXTM3U\n#EXT-X-VERSION:6\n");
    if complete {
        text.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    } else {
        text.push_str("#EXT-X-PLAYLIST-TYPE:EVENT\n");
    }
    text.push_str(&format!(
        "#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n",
        target_duration.max(1.0) as u64
    ));
    let media = if has_map {
        text.push_str("#EXT-X-MAP:URI=\"segments/init.mp4\"\n");
        files.get(1..).unwrap_or(&[])
    } else {
        files
    };
    for (index, file) in media.iter().enumerate() {
        if discontinuities.get(index).copied().unwrap_or(false) {
            text.push_str("#EXT-X-DISCONTINUITY\n");
        }
        let duration = durations.get(index).copied().unwrap_or(1.0);
        text.push_str(&format!(
            "#EXTINF:{duration:.3},\nsegments/{}\n",
            playlist_leaf(file)
        ));
    }
    if complete {
        text.push_str("#EXT-X-ENDLIST\n");
    }
    write_text_atomic(&task_dir.join("local.m3u8"), &text)
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

fn decrypt_aes128(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    crate::crypto_lite::decrypt_aes128_cbc_pkcs7(key, iv, data)
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
    if hex.len() % 2 != 0 {
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
    let mut kept = Vec::new();
    let mut pending_gap = false;
    for mut segment in playlist.segments.drain(..) {
        if segment.is_ad {
            pending_gap = true;
            continue;
        }
        if pending_gap {
            segment.discontinuity = true;
            pending_gap = false;
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

fn attr_f64(attrs: &str, key: &str) -> Option<f64> {
    attr_str(attrs, key)?.parse().ok()
}

fn resolve(base: &str, reference: &str) -> String {
    super::resolve_http_uri(base, reference)
}

fn parse_resolution(value: &str) -> (u32, u32) {
    let Some((width, height)) = value.split_once('x') else {
        return (0, 0);
    };
    (width.parse().unwrap_or(0), height.parse().unwrap_or(0))
}

fn load_seen(path: &Path) -> std::collections::BTreeSet<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return std::collections::BTreeSet::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return std::collections::BTreeSet::new();
    };
    if let Some(items) = value.get("seen").and_then(|item| item.as_array()) {
        return items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect();
    }
    value
        .get("segments")
        .and_then(|item| item.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("url")
                        .and_then(|url| url.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn resume_segment_path(
    seg_dir: &Path,
    index: usize,
    uri: &str,
) -> Option<std::path::PathBuf> {
    let candidates = [
        seg_dir.join(format!("{:06}.{}", index, extension(uri))),
        seg_dir.join(format!("{:06}.seg", index)),
        seg_dir.join(format!("seg-{index:04}.m4s")),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file() && path.metadata().map(|meta| meta.len() > 0).unwrap_or(false))
}

fn save_seen(path: &Path, seen: &std::collections::BTreeSet<String>) -> Result<(), String> {
    let value = serde_json::json!({ "seen": seen.iter().cloned().collect::<Vec<_>>() });
    fs::write(path, value.to_string()).map_err(|error| error.to_string())
}

fn extension(uri: &str) -> &str {
    Path::new(uri.split(['?', '#']).next().unwrap_or(uri))
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("bin")
}

fn read_control(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|_| "run".into())
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Condvar;
    use std::time::{Duration, Instant};

    #[test]
    fn master_prefers_resolution_over_audio_only() {
        let text = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",DEFAULT=YES,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=1280x720,CODECS=\"avc1.42e01e\",AUDIO=\"aud\"\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1920x1080,CODECS=\"avc1.640028\",AUDIO=\"aud\"\nhigh.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=128000,CODECS=\"mp4a.40.2\"\naudio.m3u8\n";
        let playlist = parse_playlist(text, "https://cdn.test/master.m3u8").unwrap();
        assert!(playlist.is_master);
        assert_eq!(
            select_variant(&playlist).unwrap().uri,
            "https://cdn.test/high.m3u8"
        );
        assert_eq!(
            select_variant_for(&playlist, 800_000, 0).unwrap().uri,
            "https://cdn.test/low.m3u8"
        );
        assert_eq!(
            select_variant_for(&playlist, 0, 720).unwrap().uri,
            "https://cdn.test/low.m3u8"
        );
        assert_eq!(variant_choices(&playlist)[0].height, 1080);
        assert_eq!(audio_choices(&playlist).len(), 1);
        assert_eq!(audio_choices(&playlist)[0].kind, "audio");
        assert_eq!(
            select_audio_track(&playlist, select_variant(&playlist).unwrap(), "English")
                .unwrap()
                .name,
            "English"
        );
        assert_eq!(
            select_default_audio(&playlist, select_variant(&playlist).unwrap())
                .unwrap()
                .uri
                .as_deref(),
            Some("https://cdn.test/audio.m3u8")
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
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let first_segment_seen = Arc::new(AtomicBool::new(false));
        let release_first_segment = Arc::new((Mutex::new(false), Condvar::new()));
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));

        let server_stop = Arc::clone(&stop);
        let server_seen = Arc::clone(&first_segment_seen);
        let server_release = Arc::clone(&release_first_segment);
        let server_requests = Arc::clone(&requests);
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let mut buf = [0u8; 4096];
                let count = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..count]);
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
                    if !server_seen.swap(true, Ordering::SeqCst) {
                        let (lock, wake) = &*server_release;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = wake.wait(released).unwrap();
                        }
                    }
                    (200, b"AAA")
                } else if path == "/vod/b.ts" {
                    (200, b"BBB")
                } else {
                    (404, b"not found")
                };
                let header = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
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
        let deadline = Instant::now() + Duration::from_secs(5);
        while !first_segment_seen.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "server did not receive the first segment"
            );
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(&control, "pause").unwrap();
        let (lock, wake) = &*release_first_segment;
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
            1
        );
        assert_eq!(
            log.iter().filter(|entry| entry.ends_with("|false")).count(),
            1,
            "only the explicit unauthenticated probe may be rejected"
        );
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn authenticated_live_pause_resume_restores_atomic_timeline() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let playlist_requests = Arc::new(AtomicUsize::new(0));
        let first_segment_seen = Arc::new(AtomicBool::new(false));
        let release_first_segment = Arc::new((Mutex::new(false), Condvar::new()));
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));

        let server_stop = Arc::clone(&stop);
        let server_playlists = Arc::clone(&playlist_requests);
        let server_seen = Arc::clone(&first_segment_seen);
        let server_release = Arc::clone(&release_first_segment);
        let server_requests = Arc::clone(&requests);
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let mut buf = [0u8; 4096];
                let count = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..count]);
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
                    let number = server_playlists.fetch_add(1, Ordering::SeqCst);
                    if number == 0 {
                        (
                            200,
                            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1,\nlive/0.ts\n",
                        )
                    } else {
                        (
                            200,
                            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:1,\nlive/1.ts\n#EXT-X-ENDLIST\n",
                        )
                    }
                } else if path == "/live/0.ts" {
                    if !server_seen.swap(true, Ordering::SeqCst) {
                        let (lock, wake) = &*server_release;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = wake.wait(released).unwrap();
                        }
                    }
                    (200, b"LIVE0")
                } else if path == "/live/1.ts" {
                    (200, b"LIVE1")
                } else {
                    (404, b"not found")
                };
                let header = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
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
        let first = thread::spawn(move || {
            download_hls_with(
                &first_url,
                &first_headers,
                "",
                &first_dir,
                &first_control,
                HlsDownloadOptions {
                    live: true,
                    concurrency: 1,
                    download_subtitles: false,
                    ..HlsDownloadOptions::default()
                },
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !first_segment_seen.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "server did not receive the first live segment"
            );
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(&control, "pause").unwrap();
        let (lock, wake) = &*release_first_segment;
        *lock.lock().unwrap() = true;
        wake.notify_all();
        assert_eq!(first.join().unwrap().unwrap_err(), "paused");

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
                live: true,
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
            1
        );
        assert_eq!(
            log.iter().filter(|entry| entry.ends_with("|false")).count(),
            1,
            "only the explicit unauthenticated probe may be rejected"
        );
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_local_playlist_is_event_until_complete() {
        let dir = std::env::temp_dir().join(format!("hls-event-{}", std::process::id()));
        let seg_dir = dir.join("segments");
        std::fs::create_dir_all(&seg_dir).unwrap();
        let file = seg_dir.join("000000.ts");
        std::fs::write(&file, b"seg").unwrap();
        write_local_playlist(&dir, 4.0, false, &[file.clone()], &[1.0], &[false], false).unwrap();
        let live = std::fs::read_to_string(dir.join("local.m3u8")).unwrap();
        assert!(live.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(!live.contains("#EXT-X-ENDLIST"));
        write_local_playlist(&dir, 4.0, false, &[file], &[1.0], &[false], true).unwrap();
        let vod = std::fs::read_to_string(dir.join("local.m3u8")).unwrap();
        assert!(vod.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(vod.contains("#EXT-X-ENDLIST"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resume_reuses_legacy_segment_files() {
        let dir = std::env::temp_dir().join(format!("hls-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("000000.seg"), b"seg").unwrap();
        let path = resume_segment_path(&dir, 0, "https://cdn.test/a.ts").unwrap();
        assert_eq!(path.file_name().unwrap(), "000000.seg");
        let _ = std::fs::remove_dir_all(dir);
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
        std::fs::write(
            dir.join("vod_segments.json"),
            serde_json::json!({
                "version": 1,
                "segments": {"0": {"identity": identity, "size": 3}}
            })
            .to_string(),
        )
        .unwrap();
        let vod = VodCheckpoint::load(&dir);
        let path = resume_segment_path(&seg_dir, 0, &segment.uri).unwrap();
        assert!(vod.can_reuse(0, &identity, file_len(&path)));
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
            "/a.ts?keep=1"
        );
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
                media_sequence: 9,
                part_index: Some(0),
                path: segment_path,
                duration: 0.5,
                discontinuity: false,
                init_map_path: None,
            }],
            next_slot: 1,
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
}
