"""Native MPEG-DASH manifest parsing for the built-in segment engine.

Covers the static VOD manifests that make up the bulk of real-world DASH:
SegmentTemplate with $Number$ or a SegmentTimeline, SegmentList, and
single-file (SegmentBase-style) representations.  Anything outside that
scope raises NativeDashUnsupported so the caller can fall back to the
bundled yt-dlp engine, while DRM raises UnsupportedPlaylistError because
no engine may bypass it.
"""

from __future__ import annotations

import math
import re
from urllib.parse import urljoin
from xml.etree import ElementTree

from ..utils import inherit_hls_access_query
from .parser import UnsupportedPlaylistError


class NativeDashUnsupported(Exception):
    """The manifest is valid but outside the native engine's scope."""


_DURATION_RE = re.compile(
    r"^P(?:(?P<days>\d+(?:\.\d+)?)D)?"
    r"(?:T(?:(?P<hours>\d+(?:\.\d+)?)H)?"
    r"(?:(?P<minutes>\d+(?:\.\d+)?)M)?"
    r"(?:(?P<seconds>\d+(?:\.\d+)?)S)?)?$"
)
_TEMPLATE_RE = re.compile(r"\$(RepresentationID|Number|Bandwidth|Time)(%0\d+d)?\$|\$\$")
_VIDEO_CODEC_RE = re.compile(r"^(avc|hev|hvc|vp0?8|vp0?9|av01)", re.IGNORECASE)


def _resolve_url(base: str, reference: str) -> str:
    return inherit_hls_access_query(base, urljoin(base, reference))


def parse_iso_duration(value: str | None) -> float:
    if not value:
        return 0.0
    match = _DURATION_RE.match(value.strip())
    if not match:
        return 0.0
    days = float(match.group("days") or 0)
    hours = float(match.group("hours") or 0)
    minutes = float(match.group("minutes") or 0)
    seconds = float(match.group("seconds") or 0)
    return days * 86400 + hours * 3600 + minutes * 60 + seconds


def expand_template(template: str, *, representation_id: str = "", number: int | None = None,
                    bandwidth: int | None = None, time: int | None = None) -> str:
    def substitute(match: re.Match) -> str:
        if match.group(0) == "$$":
            return "$"
        key = match.group(1)
        fmt = match.group(2)
        if key == "RepresentationID":
            return representation_id
        value = {"Number": number, "Bandwidth": bandwidth, "Time": time}[key]
        if value is None:
            raise NativeDashUnsupported(f"模板变量 ${key}$ 缺少对应数据")
        return (fmt % value) if fmt else str(value)

    return _TEMPLATE_RE.sub(substitute, template)


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def _children(node: ElementTree.Element, name: str) -> list[ElementTree.Element]:
    return [child for child in node if _local(child.tag) == name]


def _child(node: ElementTree.Element, name: str) -> ElementTree.Element | None:
    found = _children(node, name)
    return found[0] if found else None


def _resolve_base(url: str, *nodes: ElementTree.Element) -> str:
    base = url
    for node in nodes:
        element = _child(node, "BaseURL")
        if element is not None and (element.text or "").strip():
            base = _resolve_url(base, element.text.strip())
    return base


def _has_content_protection(*nodes: ElementTree.Element) -> bool:
    return any(_children(node, "ContentProtection") for node in nodes)


def _merged_segment_template(*nodes: ElementTree.Element) -> dict[str, str] | None:
    """Collect SegmentTemplate attributes with Representation-level override."""
    merged: dict[str, str] = {}
    timeline: ElementTree.Element | None = None
    found = False
    for node in nodes:
        template = _child(node, "SegmentTemplate")
        if template is None:
            continue
        found = True
        merged.update(template.attrib)
        inner = _child(template, "SegmentTimeline")
        if inner is not None:
            timeline = inner
    if not found:
        return None
    merged["_timeline"] = timeline  # type: ignore[assignment]
    return merged


def _timeline_entries(
    timeline: ElementTree.Element,
    timescale: float,
    period_duration: float,
    presentation_offset: int = 0,
) -> list[tuple[int, int]]:
    """Expand <S> rows into (start_time, duration) pairs in timescale units.

    Per ISO 23009-1, r="-1" repeats until the next row's @t, and only until
    the Period end when it is the last row.  The timeline origin is the
    presentationTimeOffset — live-to-VOD manifests keep epoch-scale start
    times, so Period end in media time is offset + duration, not duration.
    """
    entries: list[tuple[int, int]] = []
    rows = _children(timeline, "S")
    current = 0
    for index, row in enumerate(rows):
        duration = int(row.get("d") or 0)
        if duration <= 0:
            raise NativeDashUnsupported("SegmentTimeline 缺少有效分片时长")
        start = int(row.get("t")) if row.get("t") is not None else current
        repeat = int(row.get("r") or 0)
        if repeat < 0:
            next_row = rows[index + 1] if index + 1 < len(rows) else None
            if next_row is not None:
                if next_row.get("t") is None:
                    raise NativeDashUnsupported("开放式重复的下一行缺少起始时间")
                end_units = int(next_row.get("t"))
            else:
                if period_duration <= 0:
                    raise NativeDashUnsupported("开放式 SegmentTimeline 需要已知时段长度")
                end_units = presentation_offset + int(period_duration * timescale)
            remaining_units = end_units - start
            if remaining_units <= 0:
                raise NativeDashUnsupported("SegmentTimeline 起点超出时段范围")
            repeat = max(0, math.ceil(remaining_units / duration) - 1)
        for offset in range(repeat + 1):
            entries.append((start + offset * duration, duration))
        current = entries[-1][0] + duration
    return entries


def _template_segments(
    template: dict,
    base_url: str,
    representation_id: str,
    bandwidth: int,
    period_duration: float,
) -> tuple[str | None, list[dict]]:
    timescale = float(template.get("timescale") or 1)
    media = template.get("media")
    if not media:
        raise NativeDashUnsupported("SegmentTemplate 缺少 media 模板")
    init_url = None
    initialization = template.get("initialization")
    if initialization:
        init_url = _resolve_url(base_url, expand_template(
            initialization, representation_id=representation_id, bandwidth=bandwidth,
        ))
    segments: list[dict] = []
    timeline = template.get("_timeline")
    start_number = int(template.get("startNumber") or 1)
    presentation_offset = int(float(template.get("presentationTimeOffset") or 0))
    if timeline is not None:
        for offset, (start, duration) in enumerate(
            _timeline_entries(timeline, timescale, period_duration, presentation_offset)
        ):
            url = _resolve_url(base_url, expand_template(
                media,
                representation_id=representation_id,
                bandwidth=bandwidth,
                number=start_number + offset,
                time=start,
            ))
            segments.append({
                "url": url,
                "duration": duration / timescale,
                # Stable identity across live manifest refreshes: the media
                # timeline position in timescale units.
                "identity": start,
                # Wall-clock position on the media timeline: live tracks can
                # begin at different points, and the mux must offset them.
                "start": start / timescale,
            })
        return init_url, segments
    duration_units = float(template.get("duration") or 0)
    if duration_units <= 0:
        raise NativeDashUnsupported("SegmentTemplate 缺少 duration 或 SegmentTimeline")
    if period_duration <= 0:
        raise NativeDashUnsupported("无法确定时段长度，不能计算分片数量")
    segment_seconds = duration_units / timescale
    count = max(1, math.ceil(period_duration / segment_seconds))
    for index in range(count):
        remaining = period_duration - index * segment_seconds
        url = _resolve_url(base_url, expand_template(
            media,
            representation_id=representation_id,
            bandwidth=bandwidth,
            number=start_number + index,
            time=int(index * duration_units),
        ))
        segments.append({
            "url": url,
            "duration": min(segment_seconds, max(0.001, remaining)),
            "identity": start_number + index,
            "start": index * segment_seconds,
        })
    return init_url, segments


def _list_segments(node: ElementTree.Element, base_url: str) -> tuple[str | None, list[dict]]:
    segment_list = _child(node, "SegmentList")
    if segment_list is None:
        return None, []
    timescale = float(segment_list.get("timescale") or 1)
    duration_units = float(segment_list.get("duration") or 0)
    init_url = None
    initialization = _child(segment_list, "Initialization")
    if initialization is not None:
        if initialization.get("range"):
            raise NativeDashUnsupported("SegmentList 使用字节区间初始化")
        source = initialization.get("sourceURL")
        if source:
            init_url = _resolve_url(base_url, source)
    segments: list[dict] = []
    for segment_url in _children(segment_list, "SegmentURL"):
        if segment_url.get("mediaRange"):
            raise NativeDashUnsupported("SegmentList 使用字节区间分片")
        media = segment_url.get("media")
        if not media:
            raise NativeDashUnsupported("SegmentURL 缺少 media 地址")
        segments.append({
            "url": _resolve_url(base_url, media),
            "duration": duration_units / timescale if duration_units else 0.0,
        })
    return init_url, segments


def _is_video(adaptation: ElementTree.Element, representation: ElementTree.Element) -> bool:
    # DASH-IF thumbnail tracks (mimeType="image/jpeg" or contentType="image")
    # advertise the full tile-grid width/height, which can exceed the real
    # video's resolution — they must never enter best-video selection.
    content_type = (
        representation.get("contentType") or adaptation.get("contentType") or ""
    ).lower()
    if content_type == "image":
        return False
    mime = (representation.get("mimeType") or adaptation.get("mimeType") or "").lower()
    if mime.startswith("video/"):
        return True
    if mime.startswith(("audio/", "text/", "application/", "image/")):
        return False
    codecs = representation.get("codecs") or adaptation.get("codecs") or ""
    if _VIDEO_CODEC_RE.match(codecs):
        return True
    return bool(representation.get("width") or representation.get("height")
                or adaptation.get("width") or adaptation.get("height"))


def _is_audio(adaptation: ElementTree.Element, representation: ElementTree.Element) -> bool:
    mime = (representation.get("mimeType") or adaptation.get("mimeType") or "").lower()
    if mime.startswith("audio/"):
        return True
    if mime:
        return False
    codecs = (representation.get("codecs") or adaptation.get("codecs") or "").lower()
    return codecs.startswith(("mp4a", "opus", "vorbis", "ac-3", "ec-3", "flac"))


def _is_subtitle(adaptation: ElementTree.Element, representation: ElementTree.Element) -> bool:
    content_type = (
        representation.get("contentType") or adaptation.get("contentType") or ""
    ).lower()
    mime = (representation.get("mimeType") or adaptation.get("mimeType") or "").lower()
    codecs = (representation.get("codecs") or adaptation.get("codecs") or "").lower()
    return (
        content_type in {"text", "subtitle", "subtitles"}
        or mime.startswith("text/")
        or mime == "application/ttml+xml"
        or (mime == "application/mp4" and codecs.startswith(("stpp", "wvtt")))
        or codecs.startswith(("stpp", "wvtt"))
    )


def parse_mpd(
    url: str,
    content: str,
    preferred_video: str = "",
    preferred_audio: str = "",
) -> dict:
    """Parse a static MPD into downloadable best/selected tracks.

    preferred_video matches a Representation id; preferred_audio matches a
    Representation id or an AdaptationSet language. Unmatched preferences
    fall back to the automatic best pick so a stale selection can never
    break a retry.
    """
    try:
        root = ElementTree.fromstring(content)
    except ElementTree.ParseError as exc:
        raise NativeDashUnsupported(f"MPD 解析失败: {exc}") from exc
    if _local(root.tag) != "MPD":
        raise NativeDashUnsupported("不是 MPD 清单")
    is_dynamic = (root.get("type") or "static").lower() == "dynamic"
    periods = _children(root, "Period")
    if not periods:
        raise NativeDashUnsupported("MPD 中没有 Period")
    if len(periods) > 1:
        raise NativeDashUnsupported("多 Period MPD 暂不支持原生下载")
    period = periods[0]
    total_duration = parse_iso_duration(period.get("duration")) or parse_iso_duration(
        root.get("mediaPresentationDuration")
    )
    if is_dynamic and total_duration <= 0:
        # A live window has no fixed length; SegmentTimeline entries carry
        # their own durations, while duration-computed templates (which need
        # a known period length) fall through to NativeDashUnsupported below
        # and reach the fallback engine.
        total_duration = 0.0

    best_video: dict | None = None
    best_audio: dict | None = None
    forced_video: dict | None = None
    forced_audio: dict | None = None
    video_options: list[dict] = []
    audio_options: list[dict] = []
    subtitle_tracks: list[dict] = []
    for adaptation in _children(period, "AdaptationSet"):
        for representation in _children(adaptation, "Representation"):
            if _has_content_protection(adaptation, representation):
                raise UnsupportedPlaylistError("该 DASH 使用 DRM/ContentProtection 保护")
            is_video = _is_video(adaptation, representation)
            is_audio = not is_video and _is_audio(adaptation, representation)
            is_subtitle = not is_video and not is_audio and _is_subtitle(adaptation, representation)
            if not is_video and not is_audio and not is_subtitle:
                continue
            base_url = _resolve_base(url, root, period, adaptation, representation)
            representation_id = representation.get("id") or ""
            bandwidth = int(representation.get("bandwidth") or 0)
            template = _merged_segment_template(period, adaptation, representation)
            if template is not None:
                init_url, segments = _template_segments(
                    template, base_url, representation_id, bandwidth, total_duration,
                )
                single_file = False
            else:
                # SegmentList inherits like SegmentTemplate: the deepest
                # node carrying one wins.
                list_node = next(
                    (
                        node
                        for node in (representation, adaptation, period)
                        if _child(node, "SegmentList") is not None
                    ),
                    None,
                )
                init_url, segments = (
                    _list_segments(list_node, base_url)
                    if list_node is not None
                    else (None, [])
                )
                if not segments:
                    if list_node is not None:
                        raise NativeDashUnsupported("SegmentList 中没有分片")
                    # Only a BaseURL on the Representation itself denotes a
                    # complete single file; an ancestor BaseURL is just a
                    # directory prefix and must not be downloaded as media.
                    rep_base = _child(representation, "BaseURL")
                    if rep_base is None or not (rep_base.text or "").strip():
                        raise NativeDashUnsupported("Representation 缺少可用的分片信息")
                    init_url = None
                    segments = [{"url": base_url, "duration": total_duration}]
                    single_file = True
                else:
                    single_file = False
            if not segments:
                continue
            if is_dynamic and (
                single_file or template is None or template.get("_timeline") is None
            ):
                # A duration-based template enumerates a fixed future list the
                # recorder cannot follow; the fallback engine handles it.
                raise NativeDashUnsupported(
                    "直播 MPD 暂仅支持 SegmentTimeline 形式的原生录制"
                )
            candidate = {
                "id": representation_id,
                "mime": representation.get("mimeType") or adaptation.get("mimeType") or "",
                "codecs": representation.get("codecs") or adaptation.get("codecs") or "",
                "bandwidth": bandwidth,
                "width": int(representation.get("width") or 0),
                "height": int(representation.get("height") or 0),
                "lang": adaptation.get("lang") or "",
                "init_url": init_url,
                "single_file": single_file,
                "segments": segments,
            }
            option = {
                "id": candidate["id"],
                "height": candidate["height"],
                "width": candidate["width"],
                "bandwidth": candidate["bandwidth"],
                "codecs": candidate["codecs"],
                "lang": candidate["lang"],
            }
            if is_subtitle:
                candidate["name"] = adaptation.get("label") or representation_id
                subtitle_tracks.append(candidate)
            elif is_video:
                video_options.append(option)
                if preferred_video and candidate["id"] == preferred_video:
                    forced_video = candidate
                rank = (candidate["height"], candidate["width"], candidate["bandwidth"])
                current = (
                    (best_video["height"], best_video["width"], best_video["bandwidth"])
                    if best_video else (-1, -1, -1)
                )
                if rank > current:
                    best_video = candidate
            else:
                audio_options.append(option)
                if preferred_audio and (
                    candidate["id"] == preferred_audio
                    or (candidate["lang"] and candidate["lang"] == preferred_audio)
                ):
                    # Among several bitrates of the selected language, keep
                    # the best one.
                    if forced_audio is None or candidate["bandwidth"] > forced_audio["bandwidth"]:
                        forced_audio = candidate
                if best_audio is None or candidate["bandwidth"] > best_audio["bandwidth"]:
                    best_audio = candidate
    best_video = forced_video or best_video
    best_audio = forced_audio or best_audio
    if best_video is None and best_audio is None:
        raise NativeDashUnsupported("MPD 中没有可下载的音视频轨道")
    if total_duration <= 0:
        for track in (best_video, best_audio):
            if track:
                total_duration = max(
                    total_duration,
                    sum(segment["duration"] for segment in track["segments"]),
                )
    return {
        "type": "dynamic" if is_dynamic else "static",
        "duration": total_duration,
        "update_period": parse_iso_duration(root.get("minimumUpdatePeriod")),
        "video": best_video,
        "audio": best_audio,
        "video_options": video_options,
        "audio_options": audio_options,
        "subtitle_tracks": subtitle_tracks,
    }
