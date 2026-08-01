import json

from backend.app import utils


def test_atomic_write_text_flushes_and_replaces_existing_state(tmp_path, monkeypatch):
    destination = tmp_path / "live_state.json"
    destination.write_text('{"version":0}', encoding="utf-8")
    fsync_calls: list[int] = []
    monkeypatch.setattr(utils.os, "fsync", lambda descriptor: fsync_calls.append(descriptor))

    utils.atomic_write_text(destination, json.dumps({"version": 1, "segments": [1]}))

    assert fsync_calls
    assert json.loads(destination.read_text(encoding="utf-8")) == {
        "version": 1,
        "segments": [1],
    }
    assert not destination.with_name("live_state.json.tmp").exists()


def test_durable_replace_flushes_media_before_publishing(tmp_path, monkeypatch):
    temporary = tmp_path / "000001.seg.tmp"
    destination = tmp_path / "000001.seg"
    temporary.write_bytes(b"complete-media")
    fsync_calls: list[int] = []
    monkeypatch.setattr(utils.os, "fsync", lambda descriptor: fsync_calls.append(descriptor))

    utils.durable_replace(temporary, destination)

    assert fsync_calls
    assert destination.read_bytes() == b"complete-media"
    assert not temporary.exists()


def test_stable_request_key_drops_signatures_but_keeps_resource_parameters():
    assert utils.stable_request_key(
        "https://CDN.test/video.mp4?quality=1080&s=old&e=1&_t=2"
    ) == utils.stable_request_key(
        "https://cdn.test/video.mp4?e=9&_t=8&s=new&quality=1080"
    )
    assert utils.stable_request_key(
        "https://cdn.test/video.mp4?quality=1080&token=old"
    ) != utils.stable_request_key(
        "https://cdn.test/video.mp4?quality=720&token=new"
    )


def test_canonical_hls_url_removes_reload_cursor_but_keeps_session():
    polled = "https://edge.test/live.m3u8?_HLS_msn=99&_HLS_part=3&_HLS_skip=YES&session=current"
    canonical = "https://edge.test/live.m3u8?session=current"
    assert utils.canonical_hls_url(polled) == canonical
    assert utils.stable_request_key(polled) == utils.stable_request_key(canonical)


def test_canonical_hls_url_preserves_raw_signed_query_encoding():
    value = (
        "https://cdn.test/live.m3u8?token=a%2Fb%2Bc&Policy=x%2Fy"
        "&_HLS_msn=44&_HLS_part=2&empty="
    )
    assert utils.canonical_hls_url(value) == (
        "https://cdn.test/live.m3u8?token=a%2Fb%2Bc&Policy=x%2Fy&empty="
    )


def test_hls_access_query_inheritance_is_same_origin_and_auth_only():
    base = (
        "https://cdn.test/live/master.m3u8?quality=1080&token=a%2Fb%2Bc"
        "&_HLS_msn=4"
    )
    assert utils.inherit_hls_access_query(
        base, "https://cdn.test/live/video.m3u8"
    ) == "https://cdn.test/live/video.m3u8?token=a%2Fb%2Bc"
    assert utils.inherit_hls_access_query(
        base, "https://other.test/live/video.m3u8"
    ) == "https://other.test/live/video.m3u8"
    assert utils.inherit_hls_access_query(
        base, "https://cdn.test/live/video.m3u8?sig=child"
    ) == "https://cdn.test/live/video.m3u8?sig=child"
