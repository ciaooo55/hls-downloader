import pytest

from backend.app.downloader.parser import UnsupportedPlaylistError, parse_m3u8


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


def test_parse_rejects_live_and_sample_aes_playlists():
    live = """#EXTM3U
#EXTINF:4,
one.ts
"""
    with pytest.raises(UnsupportedPlaylistError, match="点播"):
        parse_m3u8("https://example.test/live.m3u8", live)

    sample_aes = """#EXTM3U
#EXT-X-KEY:METHOD=SAMPLE-AES,URI="key.bin"
#EXTINF:4,
one.ts
#EXT-X-ENDLIST
"""
    with pytest.raises(UnsupportedPlaylistError, match="SAMPLE-AES"):
        parse_m3u8("https://example.test/vod.m3u8", sample_aes)


def test_parse_rejects_invalid_implicit_byte_range():
    content = """#EXTM3U
#EXTINF:4,
#EXT-X-BYTERANGE:500
media.mp4
#EXT-X-ENDLIST
"""
    with pytest.raises(ValueError, match="BYTERANGE"):
        parse_m3u8("https://example.test/vod.m3u8", content)
