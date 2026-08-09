import re
from urllib.parse import urljoin

import m3u8

from ..utils import inherit_hls_access_query


DRM_METHODS = {"sample-aes", "sample-aes-ctr"}
_AUXILIARY_AUDIO = re.compile(
    r"(?:commentary|description|descriptive|visual[-_ ]?description|"
    r"audio[-_ ]?description|sign[-_ ]?language|emergency|alternate[-_ ]?audio)",
    re.IGNORECASE,
)
_AD_SEGMENT_URL = re.compile(
    r"(?:^|[/_.-])(?:ad|ads|advert|advertisement|preroll|midroll|postroll|promo)"
    r"(?:[/_.?&=-]|$)",
    re.IGNORECASE,
)
_AD_DATERANGE = re.compile(
    r"(?:^|[._:-])(?:ad|ads|advert|advertisement|scte35|splice|preroll|midroll|postroll)"
    r"(?:$|[._:-])",
    re.IGNORECASE,
)


def _playlist_title(content: str) -> str:
    for line in content.splitlines():
        line = line.strip()
        if line.upper().startswith(("#EXT-X-TITLE:", "#TITLE:")):
            return line.split(":", 1)[1].strip().strip('"')
        if not line.upper().startswith("#EXT-X-SESSION-DATA:"):
            continue
        data_id = re.search(r'DATA-ID="([^"]+)"', line, re.IGNORECASE)
        value = re.search(r'VALUE="([^"]+)"', line, re.IGNORECASE)
        if data_id and value and re.search(r"(?:title|name|filename)", data_id.group(1), re.IGNORECASE):
            return value.group(1).replace(r'\"', '"').strip()
    return ""


class UnsupportedPlaylistError(Exception):
    """The playlist is valid HLS but outside the downloader's supported scope."""


def _resolve_url(base: str, ref: str) -> str:
    if not ref:
        return ref
    return inherit_hls_access_query(base, urljoin(base, ref))


def _parse_byte_range(value: str | None, uri: str, previous_ends: dict[str, int]) -> dict | None:
    if not value:
        return None
    length_text, separator, offset_text = value.partition("@")
    try:
        length = int(length_text)
        if length <= 0:
            raise ValueError
        if separator:
            offset = int(offset_text)
        elif uri in previous_ends:
            offset = previous_ends[uri]
        else:
            raise ValueError(f"BYTERANGE 缺少起始偏移: {value}")
        if offset < 0:
            raise ValueError
    except (TypeError, ValueError) as exc:
        if isinstance(exc, ValueError) and "BYTERANGE" in str(exc):
            raise
        raise ValueError(f"无效 BYTERANGE: {value}") from exc
    previous_ends[uri] = offset + length
    return {"length": length, "offset": offset}


def _parse_iv(value: str | None, media_sequence: int) -> bytes:
    if not value:
        return media_sequence.to_bytes(16, "big")
    text = value[2:] if value.lower().startswith("0x") else value
    try:
        raw = bytes.fromhex(text)
    except ValueError as exc:
        raise ValueError(f"无效 AES-128 IV: {value}") from exc
    if len(raw) > 16:
        raise ValueError(f"无效 AES-128 IV 长度: {value}")
    return raw.rjust(16, b"\x00")


def _key_info(base_url: str, key, media_sequence: int) -> dict | None:
    if key is None or not key.method or key.method.lower() == "none":
        return None
    method = key.method.lower()
    keyformat = str(getattr(key, "keyformat", "") or "").strip().lower()
    if keyformat and keyformat != "identity":
        raise UnsupportedPlaylistError(
            f"不支持 KEYFORMAT={getattr(key, 'keyformat', keyformat)} / DRM 加密"
        )
    if method in DRM_METHODS:
        raise UnsupportedPlaylistError(f"不支持 {key.method} / DRM 加密")
    if method != "aes-128":
        raise UnsupportedPlaylistError(f"不支持的 HLS 加密方式: {key.method}")
    if not key.uri:
        raise ValueError("AES-128 密钥缺少 URI")
    return {
        "method": "AES-128",
        "uri": _resolve_url(base_url, key.uri),
        "iv": _parse_iv(key.iv, media_sequence),
    }


def is_drm_protected(playlist: m3u8.M3U8) -> bool:
    keys = list(playlist.session_keys or []) + list(playlist.keys or [])
    for key in keys:
        if not key or not getattr(key, "method", None) or key.method.lower() == "none":
            continue
        keyformat = str(getattr(key, "keyformat", "") or "").strip().lower()
        if keyformat and keyformat != "identity":
            return True
        if key.method.lower() in DRM_METHODS:
            return True
    return False


def _unsupported_keyformat(playlist: m3u8.M3U8) -> str:
    keys = list(playlist.session_keys or []) + list(playlist.keys or [])
    for key in keys:
        if not key or not getattr(key, "method", None) or key.method.lower() == "none":
            continue
        keyformat = str(getattr(key, "keyformat", "") or "").strip()
        if keyformat and keyformat.lower() != "identity":
            return keyformat
    return ""


def _variant_dimensions(info) -> tuple[int, int]:
    """Read m3u8's resolution value without depending on its concrete type."""
    resolution = getattr(info, "resolution", None)
    if isinstance(resolution, (tuple, list)) and len(resolution) >= 2:
        try:
            return max(0, int(resolution[0])), max(0, int(resolution[1]))
        except (TypeError, ValueError):
            return 0, 0
    if isinstance(resolution, str):
        match = re.fullmatch(r"\s*(\d+)x(\d+)\s*", resolution, re.IGNORECASE)
        if match:
            return int(match.group(1)), int(match.group(2))
    return 0, 0


def _is_audio_only_variant(info, width: int, height: int) -> bool:
    if width or height:
        return False
    codecs = str(getattr(info, "codecs", "") or "").lower()
    if not codecs:
        # Old/simple HLS masters often omit CODECS and RESOLUTION. Do not
        # discard them as audio based on missing metadata alone.
        return False
    return not any(marker in codecs for marker in (
        "avc", "hev", "hvc", "vp8", "vp9", "av01", "theora",
    ))


def list_hls_video_tracks(url: str, content: str) -> list[dict]:
    """Enumerate a master playlist's selectable video renditions.

    Returned ids are resolved media-playlist URLs — exactly what the
    download engine consumes as a selected rendition.
    """
    playlist = m3u8.loads(content, uri=url)
    if not playlist.is_variant:
        return []
    tracks: list[dict] = []
    seen: set[str] = set()
    for candidate in playlist.playlists:
        if not candidate.uri:
            continue
        info = candidate.stream_info
        width, height = _variant_dimensions(info)
        if _is_audio_only_variant(info, width, height):
            continue
        resolved = _resolve_url(url, candidate.uri)
        if resolved in seen:
            continue
        seen.add(resolved)
        tracks.append(
            {
                "id": resolved,
                "width": width,
                "height": height,
                "bandwidth": int(
                    getattr(info, "average_bandwidth", None) or info.bandwidth or 0
                ),
                "codecs": str(getattr(info, "codecs", "") or ""),
                "lang": "",
            }
        )
    tracks.sort(key=lambda item: (item["height"], item["bandwidth"]), reverse=True)
    return tracks


def list_hls_audio_tracks(url: str, content: str) -> list[dict]:
    """Enumerate external audio renditions in an HLS master playlist."""
    playlist = m3u8.loads(content, uri=url)
    if not playlist.is_variant:
        return []
    tracks: list[dict] = []
    seen: set[str] = set()
    for media in playlist.media or []:
        if str(getattr(media, "type", "") or "").upper() != "AUDIO":
            continue
        uri = str(getattr(media, "uri", "") or "")
        if not uri:
            continue
        resolved = _resolve_url(url, uri)
        if resolved in seen:
            continue
        seen.add(resolved)
        language = str(getattr(media, "language", "") or "")
        name = str(getattr(media, "name", "") or "")
        group_id = str(getattr(media, "group_id", "") or "")
        characteristics = str(getattr(media, "characteristics", "") or "")
        forced = str(getattr(media, "forced", "") or "").upper() == "YES"
        autoselect = str(getattr(media, "autoselect", "") or "").upper() == "YES"
        tracks.append({
            # selected_audio is intentionally short (language/name/id), while
            # the signed rendition URL can be several kilobytes long.
            "id": language or name or group_id or resolved,
            "url": resolved,
            "width": 0,
            "height": 0,
            "bandwidth": 0,
            "codecs": "",
            "lang": language,
            "name": name,
            "group_id": group_id,
            "default": str(getattr(media, "default", "") or "").upper() == "YES",
            "forced": forced,
            "autoselect": autoselect,
            "characteristics": characteristics,
            "auxiliary": bool(_AUXILIARY_AUDIO.search(f"{name} {characteristics}")),
        })
    tracks.sort(key=lambda item: (not item["default"], item["lang"], item["name"]))
    return tracks


def parse_m3u8(
    url: str,
    content: str,
    preferred_variant: str = "",
    preferred_audio: str = "",
) -> dict:
    """Parse a playlist; preferred_variant picks a specific rendition URL.

    Selecting through the master (instead of fetching the rendition
    directly) keeps EXT-X-MEDIA audio/subtitle detection intact, so a
    master with separate audio still routes to the muxing engine.
    """
    playlist = m3u8.loads(content, uri=url)
    playlist_title = _playlist_title(content)

    # Session keys belong to a master playlist too; reject unsupported
    # KEYFORMAT/DRM before returning a seemingly usable variant selection.
    if is_drm_protected(playlist):
        keyformat = _unsupported_keyformat(playlist)
        if keyformat:
            raise UnsupportedPlaylistError(f"不支持 KEYFORMAT={keyformat} / DRM 加密")
        raise UnsupportedPlaylistError("不支持 SAMPLE-AES / DRM 加密")

    if playlist.is_variant:
        best = None
        best_rank = (-1, -1, -1, -1)
        chosen = None
        for candidate in playlist.playlists:
            if preferred_variant and candidate.uri and _resolve_url(url, candidate.uri) == preferred_variant:
                chosen = candidate
            info = candidate.stream_info
            bandwidth = getattr(info, "average_bandwidth", None) or info.bandwidth or 0
            width, height = _variant_dimensions(info)
            # Highest advertised bitrate is not necessarily the clearest
            # stream: a 720p high-frame-rate rendition can exceed a 1080p
            # rendition, and audio-only variants can have a high bitrate too.
            # IDM-like one-click behavior should prefer an actual video and
            # the largest resolution before bitrate.
            rank = (
                0 if _is_audio_only_variant(info, width, height) else 1,
                height,
                width,
                int(bandwidth),
            )
            if rank > best_rank:
                best_rank = rank
                best = candidate
        # A stale selection (rendition gone after a manifest update) falls
        # back to the automatic best pick instead of failing the task.
        best = chosen or best
        if best is None or not best.uri:
            raise ValueError("主清单中没有可用视频变体")
        info = best.stream_info
        audio_group = str(getattr(info, "audio", "") or "")
        audio_tracks = [
            track for track in list_hls_audio_tracks(url, content)
            if audio_group and track.get("group_id") == audio_group
        ]
        chosen_audio = None
        if preferred_audio:
            preferred = preferred_audio.strip().lower()
            chosen_audio = next((
                track for track in audio_tracks
                if preferred in {
                    str(track.get("id", "")).lower(),
                    str(track.get("url", "")).lower(),
                    str(track.get("lang", "")).lower(),
                    str(track.get("name", "")).lower(),
                    str(track.get("group_id", "")).lower(),
                }
            ), None)
        if chosen_audio is None:
            # Prefer the main/default rendition.  Commentary, descriptive and
            # emergency tracks are still exposed for an explicit user choice,
            # but must not silently replace the program audio.
            automatic_tracks = [track for track in audio_tracks if not track.get("auxiliary")]
            candidates = automatic_tracks or audio_tracks
            chosen_audio = next((track for track in candidates if track.get("default")), None)
            if chosen_audio is None:
                chosen_audio = next((track for track in candidates if track.get("autoselect")), None)
            if chosen_audio is None and candidates:
                chosen_audio = candidates[0]
        subtitle_tracks = []
        for media in playlist.media or []:
            if str(getattr(media, "type", "") or "").upper() != "SUBTITLES":
                continue
            if not getattr(media, "uri", None):
                continue
            subtitle_tracks.append(
                {
                    "uri": _resolve_url(url, media.uri),
                    "language": str(getattr(media, "language", "") or ""),
                    "name": str(getattr(media, "name", "") or ""),
                    "default": str(getattr(media, "default", "") or "").upper() == "YES",
                    "forced": str(getattr(media, "forced", "") or "").upper() == "YES",
                }
            )
        return {
            "type": "variant",
            "url": _resolve_url(url, best.uri),
            "base_url": _resolve_url(url, best.uri),
            # A group without URI describes in-band audio.  Only a concrete
            # rendition needs the dual-track recorder/muxing path.
            "external_audio": chosen_audio is not None,
            "external_audio_url": str((chosen_audio or {}).get("url", "")),
            "audio_tracks": audio_tracks,
            "external_subtitles": bool(getattr(info, "subtitles", None))
            or bool(subtitle_tracks),
            "subtitle_tracks": subtitle_tracks,
            "title": playlist_title,
        }

    media_ranges: dict[str, int] = {}
    part_ranges: dict[str, int] = {}
    map_ranges: dict[str, int] = {}
    map_cache: dict[int, dict] = {}
    segments: list[dict] = []

    skipped_segments = 0
    skip = getattr(playlist, "skip", None)
    if skip is not None:
        try:
            skipped_segments = max(0, int(getattr(skip, "skipped_segments", 0) or 0))
        except (TypeError, ValueError):
            skipped_segments = 0
    first_sequence = int(playlist.media_sequence or 0) + skipped_segments

    playlist_map = next(
        (
            item for item in reversed(list(getattr(playlist, "segment_map", None) or []))
            if getattr(item, "uri", None)
        ),
        None,
    )
    playlist_key = next(
        (
            item for item in reversed(list(getattr(playlist, "keys", None) or []))
            if item is not None
        ),
        None,
    )
    ad_cue_active = False

    def segment_is_ad(segment, resolved_url: str) -> bool:
        nonlocal ad_cue_active
        cue_in = bool(getattr(segment, "cue_in", False))
        cue_out = bool(getattr(segment, "cue_out", False) or getattr(segment, "cue_out_start", False))
        dateranges = list(getattr(segment, "dateranges", None) or [])
        explicit_range = any(_daterange_is_ad(item) for item in dateranges)
        url_signal = bool(_AD_SEGMENT_URL.search(resolved_url))
        # EXT-X-CUE-IN terminates the range before the following media URI.
        if cue_in:
            ad_cue_active = False
        marked = cue_out or ad_cue_active or explicit_range or url_signal
        if cue_out:
            ad_cue_active = True
        return marked

    def init_map_info(init_section) -> dict | None:
        if init_section is None or not getattr(init_section, "uri", None):
            return None
        cache_key = id(init_section)
        cached = map_cache.get(cache_key)
        if cached is not None:
            return cached
        init_url = _resolve_url(url, init_section.uri)
        cached = {
            "uri": init_url,
            "byte_range": _parse_byte_range(
                getattr(init_section, "byterange", None),
                init_url,
                map_ranges,
            ),
        }
        map_cache[cache_key] = cached
        return cached

    for index, segment in enumerate(playlist.segments):
        media_sequence = first_sequence + index
        if not segment.uri:
            # LL-HLS puts the parts of the *currently-being-produced* media
            # segment at the tail of a playlist.  python-m3u8 represents that
            # tail as a Segment with uri=None and a non-empty parts list.
            # It is not a malformed media segment: on the next playlist poll
            # the origin normally publishes its complete URI.  Treating it as
            # fatal made a separate audio rendition fail at arbitrary poll
            # boundaries (for example "分片 4 缺少 URI").  Defer that incomplete
            # tail instead; completed segments remain lossless and recording
            # continues on the next live refresh.
            parts = list(getattr(segment, "parts", None) or [])
            if parts:
                # Some LL-HLS origins publish only EXT-X-PART objects while
                # the stream is live. Waiting for an eventual EXTINF URI made
                # those streams sit at 0/0 until the stall timeout. A part is
                # an independently addressable media fragment, so expose every
                # completed PART (but never PRELOAD-HINT) to the live recorder.
                # The recorder de-duplicates them by URL and atomically
                # replaces them if the later full parent segment appears.
                init_map = init_map_info(
                    getattr(segment, "init_section", None) or playlist_map
                )
                gap = False
                for part_index, part in enumerate(parts):
                    if not getattr(part, "uri", None):
                        continue
                    if str(getattr(part, "gap", "") or "").upper() == "YES":
                        gap = True
                        continue
                    part_url = _resolve_url(url, part.uri)
                    part_is_ad = segment_is_ad(segment, part_url)
                    segments.append(
                        {
                            "url": part_url,
                            "duration": float(getattr(part, "duration", 0) or 0),
                            "index": len(segments),
                            "media_sequence": media_sequence,
                            "part_index": part_index,
                            "is_partial": True,
                            "byte_range": _parse_byte_range(
                                getattr(part, "byterange", None),
                                part_url,
                                part_ranges,
                            ),
                            "key": _key_info(
                                url,
                                getattr(segment, "key", None) or playlist_key,
                                media_sequence,
                            ),
                            "init_map": init_map,
                            "discontinuity": bool(segment.discontinuity)
                            if part_index == 0
                            else gap,
                            "is_ad": part_is_ad,
                        }
                    )
                    gap = False
                continue
            # A few origins emit an empty EXTINF entry while rewriting a
            # sliding window. Although this is more commonly seen in live
            # LL-HLS, VOD mirrors can expose the same torn line during a
            # cache refresh. Preserve every valid segment and let the next
            # reload/resume fill the gap instead of discarding the whole
            # recording because one URI is temporarily absent.
            continue
        segment_url = _resolve_url(url, segment.uri)
        init_map = init_map_info(getattr(segment, "init_section", None))
        segment_is_ad_flag = segment_is_ad(segment, segment_url)

        segments.append(
            {
                "url": segment_url,
                "duration": float(segment.duration or 0),
                "index": len(segments),
                "media_sequence": media_sequence,
                "part_index": None,
                "is_partial": False,
                "byte_range": _parse_byte_range(segment.byterange, segment_url, media_ranges),
                "key": _key_info(url, segment.key, media_sequence),
                "init_map": init_map,
                "discontinuity": bool(segment.discontinuity),
                "is_ad": segment_is_ad_flag,
            }
        )

    if not segments:
        # A freshly started live encoder may legally publish the media
        # playlist metadata (and sometimes only a PRELOAD-HINT) before its
        # first complete PART/segment exists. Rejecting that snapshot makes a
        # valid LL-HLS recording fail at startup instead of polling for the
        # first media bytes. Keep fail-fast behavior for VOD/ENDLIST and for
        # documents that merely happen to start with EXTM3U.
        upper_content = content.upper()
        live_metadata_present = any(
            marker in upper_content
            for marker in (
                "#EXT-X-TARGETDURATION:",
                "#EXT-X-PART-INF:",
                "#EXT-X-SERVER-CONTROL:",
                "#EXT-X-PRELOAD-HINT:",
            )
        )
        if playlist.is_endlist or not live_metadata_present:
            raise ValueError("m3u8 中没有可用分片")

    target_duration = float(playlist.target_duration or 0)
    if target_duration <= 0:
        target_duration = max(
            (float(segment.get("duration") or 0) for segment in segments),
            default=6.0,
        ) or 6.0
    part_information = getattr(playlist, "part_inf", None)
    try:
        part_target_duration = max(
            0.0,
            float(getattr(part_information, "part_target", 0) or 0),
        )
    except (TypeError, ValueError):
        part_target_duration = 0.0
    server_control = getattr(playlist, "server_control", None)
    can_block_reload = (
        str(getattr(server_control, "can_block_reload", "") or "").upper() == "YES"
    )

    return {
        "type": "media",
        "url": url,
        "segments": segments,
        "total_duration": sum(segment["duration"] for segment in segments),
        "is_fmp4": any(segment["init_map"] is not None for segment in segments),
        "external_subtitles": bool(playlist.media),
        "title": playlist_title,
        # A media playlist without EXT-X-ENDLIST is a live/event stream that
        # keeps growing; downstream engines record it instead of assuming a
        # fixed segment list.
        "is_live": not playlist.is_endlist,
        "target_duration": target_duration,
        "part_target_duration": part_target_duration,
        "can_block_reload": can_block_reload,
        "media_sequence": first_sequence,
    }


def _daterange_is_ad(value) -> bool:
    """Recognize explicit server-side ad markers without guessing from titles."""
    for attr in ("id", "class", "scte35", "oatcls_scte35"):
        text = str(getattr(value, attr, "") or "")
        if text and _AD_DATERANGE.search(text):
            return True
    return False


def filter_ad_segments(parsed: dict, enabled: bool = True) -> dict:
    """Drop only explicit HLS ad-marked segments and preserve a timeline gap.

    URL substring filtering alone is too aggressive for ordinary assets named
    ``promotion`` or ``adventure``.  The parser marks SCTE-35/CUE-OUT ranges
    and obvious segment paths; this function is the single policy boundary
    used by both VOD and live recording.  The next retained segment receives a
    discontinuity marker so a local HLS mux cannot bridge two encodings.
    """
    if not enabled or parsed.get("type") != "media":
        return parsed
    segments = list(parsed.get("segments") or [])
    if not segments:
        return parsed
    kept: list[dict] = []
    pending_gap = False
    skipped = 0
    for raw in segments:
        segment = dict(raw)
        if bool(segment.get("is_ad")):
            skipped += 1
            pending_gap = True
            continue
        if pending_gap:
            segment["discontinuity"] = True
            pending_gap = False
        kept.append(segment)
    if not skipped:
        return parsed
    filtered = dict(parsed)
    filtered["segments"] = kept
    filtered["total_duration"] = sum(float(item.get("duration") or 0) for item in kept)
    filtered["ad_segments_skipped"] = skipped
    return filtered
