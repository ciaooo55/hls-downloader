from urllib.parse import urljoin

import m3u8


DRM_METHODS = {"sample-aes", "sample-aes-ctr"}


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


def parse_m3u8(url: str, content: str) -> dict:
    playlist = m3u8.loads(content, uri=url)

    if playlist.is_variant:
        best = None
        best_bw = -1
        for candidate in playlist.playlists:
            info = candidate.stream_info
            bandwidth = getattr(info, "average_bandwidth", None) or info.bandwidth or 0
            if bandwidth > best_bw:
                best_bw = bandwidth
                best = candidate
        if best is None or not best.uri:
            raise ValueError("主清单中没有可用视频变体")
        info = best.stream_info
        return {
            "type": "variant",
            "url": _resolve_url(url, best.uri),
            "base_url": _resolve_url(url, best.uri),
            "external_audio": bool(getattr(info, "audio", None)),
            "external_subtitles": bool(getattr(info, "subtitles", None)),
        }

    if not playlist.is_endlist:
        raise UnsupportedPlaylistError("当前仅支持点播 HLS，不支持直播清单")
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

    return {
        "type": "media",
        "url": url,
        "segments": segments,
        "total_duration": sum(segment["duration"] for segment in segments),
        "is_fmp4": any(segment["init_map"] is not None for segment in segments),
        "external_subtitles": bool(playlist.media),
    }
