import pytest

from backend.app.downloader.parser import UnsupportedPlaylistError, filter_ad_segments, parse_m3u8


def test_parse_media_playlist_builds_encryption_ranges_and_maps():
    content = """#EXTM3U
#EXT-X-VERSION:7
#EXT-X-MEDIA-SEQUENCE:42
#EXT-X-KEY:METHOD=AES-128,URI="key.bin"
#EXT-X-MAP:URI="init.mp4",BYTERANGE="100@0"
#EXTINF:4,
#EXT-X-BYTERANGE:500@100
media.mp4
#EXT-X-DISCONTINUITY
#EXT-X-KEY:METHOD=AES-128,URI="key2.bin",IV=0x0000000000000000000000000000002b
#EXT-X-MAP:URI="init2.mp4"
#EXTINF:5,
#EXT-X-BYTERANGE:600
media.mp4
#EXT-X-ENDLIST
"""

    parsed = parse_m3u8("https://example.test/vod/index.m3u8", content)

    assert parsed["type"] == "media"
    assert parsed["total_duration"] == 9
    first, second = parsed["segments"]
    assert first["media_sequence"] == 42
    assert first["byte_range"] == {"length": 500, "offset": 100}
    assert second["byte_range"] == {"length": 600, "offset": 600}
    assert first["key"]["uri"] == "https://example.test/vod/key.bin"
    assert first["key"]["iv"] == (42).to_bytes(16, "big")
    assert second["key"]["uri"] == "https://example.test/vod/key2.bin"
    assert second["key"]["iv"] == (43).to_bytes(16, "big")
    assert first["init_map"]["uri"] == "https://example.test/vod/init.mp4"
    assert first["init_map"]["byte_range"] == {"length": 100, "offset": 0}
    assert second["init_map"]["uri"] == "https://example.test/vod/init2.mp4"
    assert second["discontinuity"] is True


def test_parse_inherits_playlist_token_to_relative_variant_key_map_and_segment():
    master_url = "https://edge.test/live/master.m3u8?token=a%2Fb%2Bc&_HLS_msn=5"
    master = """#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1000
video/index.m3u8
"""
    variant = parse_m3u8(master_url, master)
    assert variant["url"] == (
        "https://edge.test/live/video/index.m3u8?token=a%2Fb%2Bc"
    )

    media = """#EXTM3U
#EXT-X-KEY:METHOD=AES-128,URI="key.bin"
#EXT-X-MAP:URI="init.mp4"
#EXTINF:4,
part.m4s
#EXT-X-ENDLIST
"""
    parsed = parse_m3u8(variant["url"], media)
    segment = parsed["segments"][0]
    assert segment["url"].endswith("part.m4s?token=a%2Fb%2Bc")
    assert segment["key"]["uri"].endswith("key.bin?token=a%2Fb%2Bc")
    assert segment["init_map"]["uri"].endswith("init.mp4?token=a%2Fb%2Bc")


def test_parse_marks_live_playlists_and_rejects_sample_aes():
    live = """#EXTM3U
#EXT-X-TARGETDURATION:5
#EXT-X-MEDIA-SEQUENCE:7
#EXTINF:4,
one.ts
"""
    parsed = parse_m3u8("https://example.test/live.m3u8", live)
    assert parsed["is_live"] is True
    assert parsed["target_duration"] == 5
    assert parsed["segments"][0]["media_sequence"] == 7

    vod = """#EXTM3U
#EXTINF:4,
one.ts
#EXT-X-ENDLIST
"""
    assert parse_m3u8("https://example.test/vod.m3u8", vod)["is_live"] is False

    live_sample_aes = """#EXTM3U
#EXT-X-KEY:METHOD=SAMPLE-AES,URI="key.bin"
#EXTINF:4,
one.ts
"""
    with pytest.raises(UnsupportedPlaylistError, match="SAMPLE-AES"):
        parse_m3u8("https://example.test/live.m3u8", live_sample_aes)

    sample_aes = """#EXTM3U
#EXT-X-KEY:METHOD=SAMPLE-AES,URI="key.bin"
#EXTINF:4,
one.ts
#EXT-X-ENDLIST
"""
    with pytest.raises(UnsupportedPlaylistError, match="SAMPLE-AES"):
        parse_m3u8("https://example.test/vod.m3u8", sample_aes)


def test_parse_rejects_non_identity_keyformat_even_when_method_is_aes128():
    playlist = """#EXTM3U
#EXT-X-KEY:METHOD=AES-128,KEYFORMAT="com.apple.streamingkeydelivery",URI="key.bin"
#EXTINF:4,
one.ts
#EXT-X-ENDLIST
"""
    with pytest.raises(UnsupportedPlaylistError, match="KEYFORMAT"):
        parse_m3u8("https://example.test/vod.m3u8", playlist)


def test_parse_ll_hls_exposes_completed_parts_but_not_preload_hint():
    # At a live poll boundary, python-m3u8 exposes the current partial media
    # segment as uri=None plus EXT-X-PART entries. Some origins remain in this
    # state for the whole broadcast, so completed parts must be recordable.
    playlist = """#EXTM3U
#EXT-X-VERSION:9
#EXT-X-TARGETDURATION:2
#EXT-X-PART-INF:PART-TARGET=0.333
#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0
#EXT-X-MEDIA-SEQUENCE:40
#EXTINF:2.0,
complete-40.m4s
#EXT-X-PART:DURATION=0.333,URI="part-41-0.m4s"
#EXT-X-PART:DURATION=0.333,URI="part-41-1.m4s"
#EXT-X-PRELOAD-HINT:TYPE=PART,URI="part-41-2.m4s"
"""

    parsed = parse_m3u8("https://example.test/live.m3u8", playlist)

    assert parsed["is_live"] is True
    assert parsed["part_target_duration"] == 0.333
    assert parsed["can_block_reload"] is True
    assert [segment["url"] for segment in parsed["segments"]] == [
        "https://example.test/complete-40.m4s",
        "https://example.test/part-41-0.m4s",
        "https://example.test/part-41-1.m4s",
    ]
    assert [segment["media_sequence"] for segment in parsed["segments"]] == [40, 41, 41]
    assert [segment["is_partial"] for segment in parsed["segments"]] == [False, True, True]
    assert [segment["part_index"] for segment in parsed["segments"]] == [None, 0, 1]


def test_parse_live_defers_an_incomplete_trailing_extinf_without_uri():
    playlist = """#EXTM3U
#EXT-X-TARGETDURATION:2
#EXTINF:2.0,
complete.ts
#EXTINF:2.0,
"""
    parsed = parse_m3u8("https://example.test/live.m3u8", playlist)
    assert [segment["url"] for segment in parsed["segments"]] == [
        "https://example.test/complete.ts",
    ]


def test_parse_live_skips_an_incomplete_middle_extinf_without_uri():
    # Some LL-HLS origins briefly publish an empty EXTINF entry between two
    # already-completed entries while the sliding window is rewritten.
    playlist = """#EXTM3U
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:10
#EXTINF:2.0,
before.ts
#EXTINF:2.0,
#EXTINF:2.0,
after.ts
"""
    parsed = parse_m3u8("https://example.test/live.m3u8", playlist)
    assert [segment["url"] for segment in parsed["segments"]] == [
        "https://example.test/before.ts",
        "https://example.test/after.ts",
    ]


def test_parse_vod_keeps_valid_segments_when_one_uri_is_torn():
    playlist = """#EXTM3U
#EXT-X-TARGETDURATION:2
#EXTINF:2.0,
before.ts
#EXTINF:2.0,
#EXTINF:2.0,
after.ts
#EXT-X-ENDLIST
"""
    parsed = parse_m3u8("https://example.test/vod.m3u8", playlist)
    assert [segment["url"] for segment in parsed["segments"]] == [
        "https://example.test/before.ts",
        "https://example.test/after.ts",
    ]


def test_parse_ll_hls_part_inherits_map_and_delta_skip_sequence():
    playlist = """#EXTM3U
#EXT-X-VERSION:9
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:40
#EXT-X-SKIP:SKIPPED-SEGMENTS=3
#EXT-X-MAP:URI="init.mp4"
#EXT-X-PART:DURATION=0.5,URI="part-43-0.m4s",INDEPENDENT=YES
"""

    parsed = parse_m3u8("https://example.test/live.m3u8", playlist)

    assert parsed["media_sequence"] == 43
    assert parsed["segments"][0]["media_sequence"] == 43
    assert parsed["segments"][0]["init_map"]["uri"] == "https://example.test/init.mp4"


def test_parse_rejects_invalid_implicit_byte_range():
    content = """#EXTM3U
#EXTINF:4,
#EXT-X-BYTERANGE:500
media.mp4
#EXT-X-ENDLIST
"""
    with pytest.raises(ValueError, match="BYTERANGE"):
        parse_m3u8("https://example.test/vod.m3u8", content)


def test_parse_extracts_session_title_metadata():
    content = """#EXTM3U
#EXT-X-SESSION-DATA:DATA-ID="com.example.video-title",VALUE="真实片名"
#EXTINF:4,
one.ts
#EXT-X-ENDLIST
"""
    assert parse_m3u8("https://example.test/video.m3u8", content)["title"] == "真实片名"


def test_parse_marks_explicit_hls_ad_ranges_without_guessing_regular_names():
    content = """#EXTM3U
#EXT-X-TARGETDURATION:6
#EXT-X-CUE-OUT:12
#EXTINF:6,
ads/spot-1.ts
#EXTINF:6,
ads/spot-2.ts
#EXT-X-CUE-IN
#EXTINF:6,
adventure/main.ts
#EXT-X-ENDLIST
"""

    parsed = parse_m3u8("https://example.test/live.m3u8", content)
    assert [item["is_ad"] for item in parsed["segments"]] == [True, True, False]
    filtered = filter_ad_segments(parsed)
    assert [item["url"] for item in filtered["segments"]] == [
        "https://example.test/adventure/main.ts",
    ]
    assert filtered["segments"][0]["discontinuity"] is True
    assert filtered["total_duration"] == 6


def test_master_playlist_prefers_video_resolution_before_bitrate():
    content = """#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=12000000,CODECS="mp4a.40.2"
audio-only.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=9000000,RESOLUTION=1280x720,CODECS="avc1.64001f,mp4a.40.2"
720p.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=4200000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2"
1080p.m3u8
"""

    parsed = parse_m3u8("https://example.test/master.m3u8", content)

    assert parsed["type"] == "variant"
    assert parsed["url"] == "https://example.test/1080p.m3u8"


def test_master_playlist_marks_a_separate_audio_rendition_for_compatible_download():
    content = """#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio-main",NAME="国语",DEFAULT=YES,AUTOSELECT=YES,URI="audio/index.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=4200000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2",AUDIO="audio-main"
video/1080.m3u8
"""

    parsed = parse_m3u8("https://example.test/master.m3u8", content)

    assert parsed["type"] == "variant"
    assert parsed["external_audio"] is True
