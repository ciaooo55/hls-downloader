"""Merge segmented WebVTT subtitle tracks into single sidecar files.

HLS masters commonly expose subtitles as an EXT-X-MEDIA rendition whose
playlist lists many short WebVTT segments.  Saving them requires stitching
the segments back into one document: applying each segment's
X-TIMESTAMP-MAP offset, dropping the cues that overlapping segments repeat
at their boundaries, and emitting both .vtt and .srt so any player works.
"""

import re

_TIMESTAMP_RE = re.compile(
    r"(?:(?P<hours>\d+):)?(?P<minutes>[0-5]?\d):(?P<seconds>[0-5]?\d)\.(?P<millis>\d{3})"
)
_CUE_TIMING_RE = re.compile(
    r"^\s*(?:(?:\d+:)?[0-5]?\d:[0-5]?\d\.\d{3})\s*-->\s*(?:(?:\d+:)?[0-5]?\d:[0-5]?\d\.\d{3})"
)
_TIMESTAMP_MAP_RE = re.compile(r"X-TIMESTAMP-MAP=(?P<body>[^\r\n]+)", re.IGNORECASE)
# SRT has no styling vocabulary beyond b/i/u; WebVTT voice, class, ruby and
# language spans must be stripped or players render them as literal text.
_VTT_ONLY_TAG_RE = re.compile(r"</?(?:v|c|lang|ruby|rt)(?:[.\s][^>]*)?>", re.IGNORECASE)
MPEGTS_CLOCK = 90000.0


def _parse_timestamp(value: str) -> float | None:
    match = _TIMESTAMP_RE.fullmatch(value.strip())
    if not match:
        return None
    hours = int(match.group("hours") or 0)
    return (
        hours * 3600.0
        + int(match.group("minutes")) * 60.0
        + int(match.group("seconds"))
        + int(match.group("millis")) / 1000.0
    )


def _format_timestamp(value: float, decimal: str = ".") -> str:
    total_ms = max(0, round(value * 1000))
    hours, remainder = divmod(total_ms, 3_600_000)
    minutes, remainder = divmod(remainder, 60_000)
    seconds, millis = divmod(remainder, 1000)
    return f"{hours:02d}:{minutes:02d}:{seconds:02d}{decimal}{millis:03d}"


def _timestamp_offset(text: str) -> float:
    """Return the X-TIMESTAMP-MAP offset in seconds (0 when absent)."""
    match = _TIMESTAMP_MAP_RE.search(text)
    if not match:
        return 0.0
    local = 0.0
    mpegts = 0.0
    for part in match.group("body").split(","):
        key, _, raw = part.partition(":")
        key = key.strip().upper()
        if key == "LOCAL":
            parsed = _parse_timestamp(raw)
            if parsed is not None:
                local = parsed
        elif key == "MPEGTS":
            try:
                mpegts = float(raw.strip())
            except ValueError:
                mpegts = 0.0
    return mpegts / MPEGTS_CLOCK - local


def _parse_cues(text: str) -> list[tuple[float, float, str, str]]:
    """Extract (start, end, settings, payload) cues from one WebVTT document."""
    cues: list[tuple[float, float, str, str]] = []
    blocks = re.split(r"\r?\n\r?\n+", text.replace("﻿", ""))
    for block in blocks:
        lines = [line for line in block.split("\n") if line.strip()]
        if not lines:
            continue
        first = lines[0].strip().upper()
        if first.startswith(("WEBVTT", "NOTE", "STYLE", "REGION", "X-TIMESTAMP-MAP")):
            # Header blocks may still hold a timing line further down when a
            # segment omits the blank line after WEBVTT; scan for it below.
            lines = [line for line in lines if not line.strip().upper().startswith(
                ("WEBVTT", "NOTE", "STYLE", "REGION", "X-TIMESTAMP-MAP")
            )]
            if not lines:
                continue
        timing_at = next(
            (position for position, line in enumerate(lines) if _CUE_TIMING_RE.match(line)),
            None,
        )
        if timing_at is None:
            continue
        timing_line = lines[timing_at]
        start_raw, _, rest = timing_line.partition("-->")
        end_parts = rest.strip().split(None, 1)
        start = _parse_timestamp(start_raw)
        end = _parse_timestamp(end_parts[0]) if end_parts else None
        if start is None or end is None:
            continue
        settings = end_parts[1].strip() if len(end_parts) > 1 else ""
        payload = "\n".join(lines[timing_at + 1:]).strip()
        if not payload:
            continue
        cues.append((start, end, settings, payload))
    return cues


def merge_webvtt_segments(texts: list[str]) -> str:
    """Stitch segmented WebVTT documents into a single ordered document."""
    merged: list[tuple[float, float, str, str]] = []
    seen: set[tuple[int, int, str]] = set()
    for text in texts:
        offset = _timestamp_offset(text)
        for start, end, settings, payload in _parse_cues(text):
            shifted_start = max(0.0, start + offset)
            shifted_end = max(shifted_start, end + offset)
            # Consecutive segments repeat the cues that straddle their
            # boundary; keep the first occurrence only.
            key = (round(shifted_start * 1000), round(shifted_end * 1000), payload)
            if key in seen:
                continue
            seen.add(key)
            merged.append((shifted_start, shifted_end, settings, payload))
    merged.sort(key=lambda cue: (cue[0], cue[1]))
    lines = ["WEBVTT", ""]
    for start, end, settings, payload in merged:
        timing = f"{_format_timestamp(start)} --> {_format_timestamp(end)}"
        if settings:
            timing += f" {settings}"
        lines.append(timing)
        lines.append(payload)
        lines.append("")
    return "\n".join(lines)


def webvtt_to_srt(vtt_text: str) -> str:
    """Convert a merged WebVTT document to SubRip for maximum compatibility."""
    lines: list[str] = []
    for number, (start, end, _settings, payload) in enumerate(_parse_cues(vtt_text), 1):
        lines.append(str(number))
        lines.append(
            f"{_format_timestamp(start, ',')} --> {_format_timestamp(end, ',')}"
        )
        lines.append(_VTT_ONLY_TAG_RE.sub("", payload))
        lines.append("")
    return "\n".join(lines)


def has_cues(vtt_text: str) -> bool:
    return bool(_parse_cues(vtt_text))
