//! Native DASH downloader. Unsupported multi-period codec switches fail closed.

use crate::http_engine::{fetch_bytes, run_job, Job};
use crate::media::merge::merge_with_ffmpeg;
use crate::media::subtitles::{has_cues, merge_webvtt_segments, webvtt_to_srt};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct DashManifest {
    pub dynamic: bool,
    pub period_count: usize,
    pub representations: Vec<Representation>,
}

#[derive(Debug, Clone, PartialEq)]
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
    pub media: Vec<String>,
}

pub fn parse_mpd(xml: &str, base: &str) -> Result<DashManifest, String> {
    if !xml.contains("<MPD") && !xml.contains("<mpd") {
        return Err("not a DASH manifest".into());
    }
    let dynamic = xml.contains("type=\"dynamic\"") || xml.contains("type='dynamic'");
    let period_count = xml
        .matches("<Period")
        .count()
        .max(xml.matches("<period").count());
    let mut representations = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Representation") {
        let absolute = xml.len() - rest.len() + start;
        let after = &rest[start..];
        let end = after
            .find("</Representation>")
            .or_else(|| after.find("/>"))
            .ok_or_else(|| "truncated Representation".to_string())?;
        let block = &after[..end];
        let id = attr(block, "id").unwrap_or_else(|| "0".into());
        let bandwidth = attr(block, "bandwidth")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let height = attr(block, "height")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let mime = {
            let mut mime = attr(block, "mimeType").unwrap_or_default();
            if mime.is_empty() {
                mime = enclosing_adaptation_set(xml, absolute)
                    .and_then(|set| attr(&set, "mimeType"))
                    .unwrap_or_default();
            }
            mime
        };
        let (lang, codecs, content_type, label) = set_meta(block, xml, absolute);
        let base_url = tag_text(block, "BaseURL")
            .map(|value| resolve(base, &value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| base.to_string());
        let template_block = segment_template_block(block).unwrap_or(block);
        let init = attr(template_block, "initialization")
            .or_else(|| attr(block, "initialization"))
            .map(|value| resolve(&base_url, &apply_template(&value, &id, 0, 0, bandwidth)));
        let media_template = attr(template_block, "media");
        let mut media = Vec::new();
        if let Some(template) = media_template {
            media.extend(expand_timeline(
                template_block,
                &template,
                &base_url,
                &id,
                bandwidth,
            ));
        }
        for line in block.split("<SegmentURL") {
            if let Some(source) = attr(line, "media") {
                media.push(resolve(&base_url, &source));
            }
        }
        if media.is_empty() {
            if let Some(set) = enclosing_adaptation_set(xml, absolute) {
                let set_template = segment_template_block(&set).unwrap_or(set.as_str());
                let init_from_set = attr(set_template, "initialization")
                    .map(|value| resolve(&base_url, &apply_template(&value, &id, 0, 0, bandwidth)));
                if let Some(template) = attr(set_template, "media") {
                    media.extend(expand_timeline(
                        set_template,
                        &template,
                        &base_url,
                        &id,
                        bandwidth,
                    ));
                }
                representations.push(Representation {
                    id,
                    bandwidth,
                    height,
                    mime,
                    lang,
                    codecs,
                    content_type,
                    label,
                    base_url,
                    init: init.or(init_from_set).filter(|url| !url.is_empty()),
                    media: media.into_iter().filter(|url| !url.is_empty()).collect(),
                });
                rest = &after[end.saturating_add(1)..];
                continue;
            }
        }
        representations.push(Representation {
            id,
            bandwidth,
            height,
            mime,
            lang,
            codecs,
            content_type,
            label,
            base_url,
            init: init.filter(|url| !url.is_empty()),
            media: media.into_iter().filter(|url| !url.is_empty()).collect(),
        });
        rest = &after[end.saturating_add(1)..];
    }
    if representations.is_empty() {
        return Err("DASH manifest has no representations".into());
    }
    Ok(DashManifest {
        dynamic,
        period_count,
        representations,
    })
}

pub fn representation_choices(manifest: &DashManifest) -> Vec<crate::StreamVariant> {
    let mut choices: Vec<_> = manifest
        .representations
        .iter()
        .filter(|item| is_video(item))
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
        .filter(|item| is_audio(item))
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

fn select_video<'a>(
    representations: &'a [Representation],
    preferred_bandwidth: u64,
) -> Option<&'a Representation> {
    let videos: Vec<_> = representations
        .iter()
        .filter(|item| is_video(item))
        .collect();
    if preferred_bandwidth > 0 {
        videos
            .into_iter()
            .min_by_key(|item| item.bandwidth.abs_diff(preferred_bandwidth))
    } else {
        videos.into_iter().max_by_key(|item| item.bandwidth)
    }
}

pub fn download_dash(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
) -> Result<PathBuf, String> {
    download_dash_selected(url, headers, proxy, task_dir, control, 0, true, "")
}

pub fn download_dash_selected(
    url: &str,
    headers: &HashMap<String, String>,
    proxy: &str,
    task_dir: &Path,
    control: &Path,
    preferred_bandwidth: u64,
    download_subtitles: bool,
    preferred_audio: &str,
) -> Result<PathBuf, String> {
    let (status, body) = fetch_bytes(url, headers, proxy).map_err(|error| error.to_string())?;
    if status != 200 && status != 206 {
        return Err(format!("DASH manifest HTTP {status}"));
    }
    let xml = String::from_utf8_lossy(&body);
    fs::create_dir_all(task_dir).map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut xml = xml.into_owned();
    let mut vod = DashCheckpoint::load(task_dir);
    let mut recorded_audio: Option<Representation> = None;
    let mut recorded_subs: Vec<Representation> = Vec::new();
    loop {
        let manifest = parse_mpd(&xml, url)?;
        if multi_period_codec_change(&xml, manifest.period_count) {
            return Err("unsupported multi-period DASH with codec changes".into());
        }
        let video = select_video(&manifest.representations, preferred_bandwidth).cloned();
        let audio = select_audio(&manifest.representations, preferred_audio).cloned();
        if video.is_none() && audio.is_none() {
            return Err("DASH has no audio or video representation".into());
        }
        let checkpoint = !manifest.dynamic;
        if let Some(video) = video.as_ref() {
            if let Some(init) = &video.init {
                if seen.insert(init.clone()) {
                    files.push(resume_or_fetch(
                        init,
                        headers,
                        proxy,
                        &task_dir.join("init.mp4"),
                        control,
                        task_dir,
                        "init",
                        "init",
                        &video.id,
                        &video.mime,
                        checkpoint,
                        &mut vod,
                    )?);
                    report_dash_progress(task_dir, &files, manifest.dynamic);
                }
            }
            for media in &video.media {
                if !seen.insert(media.clone()) {
                    continue;
                }
                if read_control(control) != "run" {
                    break;
                }
                let index = files.len();
                let dest = task_dir.join(format!("seg-{index:04}.m4s"));
                files.push(resume_or_fetch(
                    media,
                    headers,
                    proxy,
                    &dest,
                    control,
                    task_dir,
                    "video",
                    &index.to_string(),
                    &video.id,
                    &video.mime,
                    checkpoint,
                    &mut vod,
                )?);
                report_dash_progress(task_dir, &files, manifest.dynamic);
            }
        } else if let Some(audio) = audio.as_ref() {
            if let Some(init) = &audio.init {
                if seen.insert(init.clone()) {
                    files.push(resume_or_fetch(
                        init,
                        headers,
                        proxy,
                        &task_dir.join("init.mp4"),
                        control,
                        task_dir,
                        "init",
                        "audio-init",
                        &audio.id,
                        &audio.mime,
                        checkpoint,
                        &mut vod,
                    )?);
                    report_dash_progress(task_dir, &files, manifest.dynamic);
                }
            }
            for media in &audio.media {
                if !seen.insert(media.clone()) {
                    continue;
                }
                if read_control(control) != "run" {
                    break;
                }
                let index = files.len();
                let dest = task_dir.join(format!("seg-{index:04}.m4s"));
                files.push(resume_or_fetch(
                    media,
                    headers,
                    proxy,
                    &dest,
                    control,
                    task_dir,
                    "audio",
                    &index.to_string(),
                    &audio.id,
                    &audio.mime,
                    checkpoint,
                    &mut vod,
                )?);
                report_dash_progress(task_dir, &files, manifest.dynamic);
            }
        }
        if video.is_some() {
            if let Some(audio) = audio.as_ref() {
                merge_representation(&mut recorded_audio, audio);
            }
        }
        if download_subtitles {
            for track in select_subtitles(&manifest.representations) {
                merge_sub_track(&mut recorded_subs, &track);
            }
        }
        if manifest.dynamic {
            if let Some(audio) = recorded_audio.as_ref() {
                let _ =
                    pull_dash_audio_segments(audio, task_dir, headers, proxy, control, &mut vod);
            }
            if download_subtitles {
                save_dash_subtitles(task_dir, &recorded_subs, headers, proxy, control, &mut vod);
            }
        }
        if !manifest.dynamic {
            if files.is_empty() {
                return Err("DASH representation produced no segments".into());
            }
            write_dash_playlist(task_dir, &files)?;
            return finish_dash(
                task_dir,
                files,
                recorded_audio,
                recorded_subs,
                headers,
                proxy,
                control,
                &mut vod,
            );
        }
        if read_control(control) != "run" {
            break;
        }
        let wait = minimum_update_period(&xml).unwrap_or(3.0).clamp(0.4, 8.0);
        std::thread::sleep(std::time::Duration::from_secs_f64(wait));
        if read_control(control) != "run" {
            break;
        }
        let (status, body) = fetch_bytes(url, headers, proxy).map_err(|error| error.to_string())?;
        if status != 200 && status != 206 {
            break;
        }
        xml = String::from_utf8_lossy(&body).into_owned();
    }
    if files.is_empty() {
        return Err("DASH representation produced no segments".into());
    }
    write_dash_playlist(task_dir, &files)?;
    finish_dash(
        task_dir,
        files,
        recorded_audio,
        recorded_subs,
        headers,
        proxy,
        control,
        &mut vod,
    )
}

fn finish_dash(
    task_dir: &Path,
    files: Vec<PathBuf>,
    audio: Option<Representation>,
    subtitles: Vec<Representation>,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    vod: &mut DashCheckpoint,
) -> Result<PathBuf, String> {
    let mut audio_merged = None;
    if let Some(audio) = audio {
        if let Some(audio_out) = fetch_dash_audio(&audio, task_dir, headers, proxy, control, vod)? {
            audio_merged = Some(audio_out);
        }
    }
    let output = task_dir.join("merged.mp4");
    merge_with_ffmpeg(task_dir, &output)
        .or_else(|_| crate::media::merge::concat_files(&files, &output))?;
    let published = if let Some(audio) = audio_merged {
        let muxed = task_dir.join("muxed.mp4");
        crate::media::merge::mux_av(&output, Some(&audio), &[], &muxed)?;
        muxed
    } else {
        output
    };
    save_dash_subtitles(task_dir, &subtitles, headers, proxy, control, vod);
    Ok(published)
}

fn merge_representation(dst: &mut Option<Representation>, src: &Representation) {
    match dst {
        None => *dst = Some(src.clone()),
        Some(existing) => {
            if existing.init.is_none() {
                existing.init = src.init.clone();
            }
            for media in &src.media {
                if !existing.media.iter().any(|item| item == media) {
                    existing.media.push(media.clone());
                }
            }
        }
    }
}

fn merge_sub_track(tracks: &mut Vec<Representation>, incoming: &Representation) {
    if let Some(existing) = tracks.iter_mut().find(|item| {
        item.id == incoming.id && item.lang == incoming.lang && item.mime == incoming.mime
    }) {
        if existing.init.is_none() {
            existing.init = incoming.init.clone();
        }
        for media in &incoming.media {
            if !existing.media.iter().any(|item| item == media) {
                existing.media.push(media.clone());
            }
        }
        return;
    }
    tracks.push(incoming.clone());
}

fn pull_dash_audio_segments(
    audio: &Representation,
    task_dir: &Path,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    vod: &mut DashCheckpoint,
) -> Result<Vec<PathBuf>, String> {
    let audio_dir = task_dir.join("audio");
    fs::create_dir_all(&audio_dir).map_err(|error| error.to_string())?;
    let mut audio_files = Vec::new();
    if let Some(init) = &audio.init {
        if let Ok(path) = resume_or_fetch(
            init,
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
        ) {
            audio_files.push(path);
        }
    }
    for (index, media) in audio.media.iter().enumerate() {
        if read_control(control) != "run" {
            break;
        }
        if let Ok(path) = resume_or_fetch(
            media,
            headers,
            proxy,
            &audio_dir.join(format!("seg-{index:04}.m4s")),
            control,
            task_dir,
            "audio",
            &index.to_string(),
            &audio.id,
            &audio.mime,
            true,
            vod,
        ) {
            audio_files.push(path);
        }
    }
    Ok(audio_files)
}

fn fetch_dash_audio(
    audio: &Representation,
    task_dir: &Path,
    headers: &HashMap<String, String>,
    proxy: &str,
    control: &Path,
    vod: &mut DashCheckpoint,
) -> Result<Option<PathBuf>, String> {
    let audio_files = pull_dash_audio_segments(audio, task_dir, headers, proxy, control, vod)?;
    if audio_files.is_empty() {
        return Ok(None);
    }
    let audio_dir = task_dir.join("audio");
    write_dash_playlist(&audio_dir, &audio_files)?;
    let audio_out = audio_dir.join("merged.m4a");
    merge_with_ffmpeg(&audio_dir, &audio_out)
        .or_else(|_| crate::media::merge::concat_files(&audio_files, &audio_out))?;
    Ok(Some(audio_out))
}

fn download_one(
    url: &str,
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
    };
    run_job(&job).map_err(|error| error.to_string())?;
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
    mime: &str,
) -> String {
    let payload = serde_json::json!({
        "kind": kind,
        "mime": mime,
        "representation": representation,
        "slot": slot,
        "url": super::hls::stable_media_url(url),
    });
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

    fn has(&self, slot: &str) -> bool {
        self.records.contains_key(slot)
    }

    fn can_reuse(&self, slot: &str, identity: &str, size: u64) -> bool {
        if size == 0 {
            return false;
        }
        match self.records.get(slot) {
            Some((saved, saved_size)) => saved == identity && *saved_size == size,
            None => true,
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
        fs::write(task_dir.join("dash_vod_segments.json"), payload.to_string())
            .map_err(|error| error.to_string())
    }
}

fn resume_or_fetch(
    url: &str,
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
    let identity = dash_file_identity(kind, slot, representation, url, mime);
    let size = dest.metadata().map(|meta| meta.len()).unwrap_or(0);
    if dest.is_file() && vod.can_reuse(&rel, &identity, size) {
        if checkpoint {
            vod.remember(&rel, &identity, size);
            vod.save(task_dir)?;
        }
        return Ok(dest.to_path_buf());
    }
    if dest.is_file() && vod.has(&rel) {
        let _ = fs::remove_file(dest);
    }
    download_one(url, headers, proxy, dest, control)?;
    if checkpoint {
        vod.remember(
            &rel,
            &identity,
            dest.metadata().map(|meta| meta.len()).unwrap_or(0),
        );
        vod.save(task_dir)?;
    }
    Ok(dest.to_path_buf())
}

fn attr(block: &str, key: &str) -> Option<String> {
    let pattern = format!("{key}=\"");
    let start = block.find(&pattern)?;
    let rest = &block[start + pattern.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn tag_text(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(block[start..end].trim().to_string())
}

fn resolve(base: &str, reference: &str) -> String {
    super::resolve_http_uri(base, reference)
}

fn minimum_update_period(xml: &str) -> Option<f64> {
    attr(xml, "minimumUpdatePeriod").and_then(|value| parse_duration_seconds(&value))
}

fn parse_duration_seconds(value: &str) -> Option<f64> {
    let text = value.trim();
    if let Some(rest) = text.strip_prefix("PT") {
        let mut total = 0.0;
        let mut number = String::new();
        for ch in rest.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                number.push(ch);
                continue;
            }
            let parsed = number.parse::<f64>().ok()?;
            number.clear();
            total += match ch {
                'H' => parsed * 3600.0,
                'M' => parsed * 60.0,
                'S' => parsed,
                _ => 0.0,
            };
        }
        return (total > 0.0).then_some(total);
    }
    text.parse().ok()
}

fn read_control(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|_| "run".into())
        .trim()
        .to_ascii_lowercase()
}

fn report_dash_progress(task_dir: &Path, files: &[PathBuf], live: bool) {
    let downloaded: u64 = files
        .iter()
        .map(|path| fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
        .sum();
    crate::http_engine::write_progress(
        &task_dir.join("progress.json"),
        downloaded,
        0,
        0.0,
        if live { "recording" } else { "downloading" },
    );
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
        text.push_str(&format!("#EXTINF:4.000,\n{}\n", playlist_leaf(file)));
    }
    text.push_str("#EXT-X-ENDLIST\n");
    fs::write(task_dir.join("local.m3u8"), text).map_err(|error| error.to_string())
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

fn enclosing_adaptation_set(xml: &str, at: usize) -> Option<String> {
    let prefix = xml.get(..at.min(xml.len()))?;
    let start = prefix.rfind("<AdaptationSet")?;
    let from = &xml[start..];
    let end = from
        .find("</AdaptationSet>")
        .unwrap_or(from.len().min(16_384));
    Some(from[..end].to_string())
}

fn set_meta(block: &str, xml: &str, absolute: usize) -> (String, String, String, String) {
    let set = enclosing_adaptation_set(xml, absolute);
    let from_set = |key: &str| set.as_ref().and_then(|value| attr(value, key));
    (
        attr(block, "lang")
            .or_else(|| from_set("lang"))
            .unwrap_or_default(),
        attr(block, "codecs")
            .or_else(|| from_set("codecs"))
            .unwrap_or_default(),
        attr(block, "contentType")
            .or_else(|| from_set("contentType"))
            .unwrap_or_default(),
        attr(block, "label")
            .or_else(|| from_set("label"))
            .unwrap_or_default(),
    )
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

fn segment_template_block(block: &str) -> Option<&str> {
    let start = block.find("<SegmentTemplate")?;
    let rest = &block[start..];
    let end = rest
        .find("</SegmentTemplate>")
        .map(|index| index + "</SegmentTemplate>".len())
        .or_else(|| rest.find("/>").map(|index| index + 2))?;
    Some(&rest[..end])
}

fn expand_timeline(
    block: &str,
    template: &str,
    base_url: &str,
    id: &str,
    bandwidth: u64,
) -> Vec<String> {
    let start_number = attr(block, "startNumber")
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let mut media = Vec::new();
    let mut number = start_number;
    let mut clock = 0u64;
    let mut rest = block;
    while let Some(index) = rest.find("<S") {
        let after = &rest[index + 2..];
        if !(after.starts_with(' ')
            || after.starts_with('\n')
            || after.starts_with('\t')
            || after.starts_with('>'))
        {
            rest = after;
            continue;
        }
        let end = after.find('>').unwrap_or(0);
        let tag = &after[..end];
        let duration = attr_unquoted_or_quoted(tag, "d")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let time = attr_unquoted_or_quoted(tag, "t")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(clock);
        let repeat = attr_unquoted_or_quoted(tag, "r")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0)
            .max(0) as u64;
        clock = time;
        for _ in 0..=repeat {
            media.push(resolve(
                base_url,
                &apply_template(template, id, number, clock, bandwidth),
            ));
            number += 1;
            clock = clock.saturating_add(duration);
        }
        rest = &after[end.saturating_add(1)..];
    }
    media
}

fn apply_template(template: &str, id: &str, number: u64, time: u64, bandwidth: u64) -> String {
    let mut out = template
        .replace("$RepresentationID$", id)
        .replace("$Bandwidth$", &bandwidth.to_string())
        .replace("$Time$", &time.to_string());
    while let Some(start) = out.find("$Number") {
        let after = &out[start + 7..];
        if after.starts_with('$') {
            out.replace_range(start..start + 8, &number.to_string());
        } else if let Some(rest) = after.strip_prefix('%') {
            if let Some(spec_end) = rest.find('$') {
                let width = rest[..spec_end]
                    .trim_end_matches('d')
                    .parse::<usize>()
                    .unwrap_or(0);
                let formatted = format!("{number:0width$}");
                out.replace_range(start..start + 8 + spec_end + 1, &formatted);
            } else {
                break;
            }
        } else {
            break;
        }
    }
    out
}

fn attr_unquoted_or_quoted(block: &str, key: &str) -> Option<String> {
    attr(block, key).or_else(|| {
        let pattern = format!("{key}=");
        let start = block.find(&pattern)?;
        let rest = &block[start + pattern.len()..];
        let end = rest
            .find(|ch: char| ch == ' ' || ch == '/' || ch == '>')
            .unwrap_or(rest.len());
        Some(rest[..end].trim_matches('"').to_string())
    })
}

fn multi_period_codec_change(xml: &str, period_count: usize) -> bool {
    if period_count <= 1 {
        return false;
    }
    let mut codecs = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("codecs=\"") {
        let after = &rest[start + 8..];
        if let Some(end) = after.find('"') {
            let value = &after[..end];
            if !value.is_empty() && !codecs.iter().any(|item| item == value) {
                codecs.push(value.to_string());
            }
            rest = &after[end..];
        } else {
            break;
        }
    }
    codecs.len() > 1
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn fails_closed_on_multi_period_codec_change() {
        let xml = r#"<MPD type="static"><Period><AdaptationSet><Representation id="v" bandwidth="1" codecs="avc1" mimeType="video/mp4"><SegmentURL media="a.m4s"/></Representation></AdaptationSet></Period><Period><AdaptationSet><Representation id="v2" bandwidth="1" codecs="hvc1" mimeType="video/mp4"><SegmentURL media="b.m4s"/></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/manifest.mpd").unwrap();
        assert_eq!(parsed.period_count, 2);
        assert!(multi_period_codec_change(xml, parsed.period_count));
    }

    #[test]
    fn formats_padded_number_templates() {
        assert_eq!(
            apply_template("seg_$Number%05d$.m4s", "v", 12, 0, 1),
            "seg_00012.m4s"
        );
        assert_eq!(
            apply_template("$RepresentationID$_$Number$.m4s", "v1", 3, 0, 1),
            "v1_3.m4s"
        );
    }

    #[test]
    fn dynamic_manifest_and_update_period() {
        let xml = r#"<MPD type="dynamic" minimumUpdatePeriod="PT2.5S"><Period><AdaptationSet><Representation id="v" bandwidth="1" mimeType="video/mp4"><SegmentURL media="1.m4s"/></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_mpd(xml, "https://cdn.test/live.mpd").unwrap();
        assert!(parsed.dynamic);
        assert_eq!(minimum_update_period(xml), Some(2.5));
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
            "video/mp4",
        );
        std::fs::write(
            dir.join("dash_vod_segments.json"),
            serde_json::json!({
                "version": 1,
                "files": {"seg-0001.m4s": {"identity": identity, "size": 3}}
            })
            .to_string(),
        )
        .unwrap();
        let vod = DashCheckpoint::load(&dir);
        assert!(vod.can_reuse("seg-0001.m4s", &identity, 3));
        let other = dash_file_identity("video", "1", "v", "https://cdn.test/2.m4s", "video/mp4");
        assert!(!vod.can_reuse("seg-0001.m4s", &other, 3));
        assert_eq!(
            dash_file_identity(
                "video",
                "1",
                "v",
                "https://cdn.test/1.m4s?token=old",
                "video/mp4"
            ),
            dash_file_identity(
                "video",
                "1",
                "v",
                "https://cdn.test/1.m4s?token=new",
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
<AdaptationSet mimeType="audio/mp4"><Representation id="a" bandwidth="128000" codecs="mp4a.40.2"><SegmentURL media="a.m4s"/></Representation></AdaptationSet>
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
        assert_eq!(select_video(&parsed.representations, 0).unwrap().id, "v");
        assert_eq!(select_audio(&parsed.representations, "").unwrap().id, "a");
        assert_eq!(representation_choices(&parsed).len(), 1);
    }

    #[test]
    fn live_audio_and_sub_urls_accumulate_across_mpd_refreshes() {
        let mut audio = None;
        merge_representation(
            &mut audio,
            &Representation {
                id: "a".into(),
                bandwidth: 128000,
                height: 0,
                mime: "audio/mp4".into(),
                lang: "en".into(),
                codecs: "mp4a.40.2".into(),
                content_type: "audio".into(),
                label: String::new(),
                base_url: String::new(),
                init: Some("https://cdn.test/a-init.mp4".into()),
                media: vec!["https://cdn.test/a0.m4s".into()],
            },
        );
        merge_representation(
            &mut audio,
            &Representation {
                id: "a".into(),
                bandwidth: 128000,
                height: 0,
                mime: "audio/mp4".into(),
                lang: "en".into(),
                codecs: "mp4a.40.2".into(),
                content_type: "audio".into(),
                label: String::new(),
                base_url: String::new(),
                init: Some("https://cdn.test/a-init.mp4".into()),
                media: vec![
                    "https://cdn.test/a1.m4s".into(),
                    "https://cdn.test/a0.m4s".into(),
                ],
            },
        );
        let audio = audio.unwrap();
        assert_eq!(audio.init.as_deref(), Some("https://cdn.test/a-init.mp4"));
        assert_eq!(
            audio.media,
            vec![
                "https://cdn.test/a0.m4s".to_string(),
                "https://cdn.test/a1.m4s".to_string()
            ]
        );
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
                media: vec!["https://cdn.test/en-0.vtt".into()],
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
                media: vec!["https://cdn.test/en-1.vtt".into()],
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
        assert!(select_video(&parsed.representations, 0).is_none());
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
