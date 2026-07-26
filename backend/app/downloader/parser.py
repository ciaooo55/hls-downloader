import re
from urllib.parse import urljoin

import m3u8


DRM_METHODS = {"sample-aes", "sample-aes-ctr"}


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
    return urljoin(base, ref)


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
    return any(key and key.method and key.method.lower() in DRM_METHODS for key in keys)


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


def parse_m3u8(url: str, content: str) -> dict:
    playlist = m3u8.loads(content, uri=url)
    playlist_title = _playlist_title(content)

    if playlist.is_variant:
        best = None
        best_rank = (-1, -1, -1, -1)
        for candidate in playlist.playlists:
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
        if best is None or not best.uri:
            raise ValueError("主清单中没有可用视频变体")
        info = best.stream_info
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
            "external_audio": bool(getattr(info, "audio", None)),
            "external_subtitles": bool(getattr(info, "subtitles", None))
            or bool(subtitle_tracks),
            "subtitle_tracks": subtitle_tracks,
            "title": playlist_title,
        }

    if is_drm_protected(playlist):
        raise UnsupportedPlaylistError("不支持 SAMPLE-AES / DRM 加密")

    media_ranges: dict[str, int] = {}
    map_ranges: dict[str, int] = {}
    map_cache: dict[int, dict] = {}
    segments = []

    for index, segment in enumerate(playlist.segments):
        if not segment.uri:
            raise ValueError(f"分片 {index} 缺少 URI")
        segment_url = _resolve_url(url, segment.uri)
        media_sequence = int(playlist.media_sequence or 0) + index

        init_map = None
        init_section = getattr(segment, "init_section", None)
        if init_section and init_section.uri:
            cache_key = id(init_section)
            init_map = map_cache.get(cache_key)
            if init_map is None:
                init_url = _resolve_url(url, init_section.uri)
                init_map = {
                    "uri": init_url,
                    "byte_range": _parse_byte_range(
                        getattr(init_section, "byterange", None),
                        init_url,
                        map_ranges,
                    ),
                }
                map_cache[cache_key] = init_map

        segments.append(
            {
                "url": segment_url,
                "duration": float(segment.duration or 0),
                "index": index,
                "media_sequence": media_sequence,
                "byte_range": _parse_byte_range(segment.byterange, segment_url, media_ranges),
                "key": _key_info(url, segment.key, media_sequence),
                "init_map": init_map,
                "discontinuity": bool(segment.discontinuity),
            }
        )

    target_duration = float(playlist.target_duration or 0)
    if target_duration <= 0:
        target_duration = max(
            (segment["duration"] for segment in segments),
            default=6.0,
        ) or 6.0

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
        "media_sequence": int(playlist.media_sequence or 0),
    }
