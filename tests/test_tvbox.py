import asyncio
import ipaddress
from urllib.request import Request, urlopen

import pytest

from backend.app import tvbox


def test_normalize_tvbox_endpoint_strips_action_and_slashes():
    assert tvbox.normalize_tvbox_endpoint("http://192.168.1.20:9979/action/") == "http://192.168.1.20:9979"
    assert tvbox.tvbox_action_url("http://192.168.1.20:9979/") == "http://192.168.1.20:9979/action"


def test_normalize_tvbox_endpoint_rejects_invalid_values():
    with pytest.raises(ValueError):
        tvbox.normalize_tvbox_endpoint("")
    with pytest.raises(ValueError):
        tvbox.normalize_tvbox_endpoint("ftp://192.168.1.20:9979")


def test_scan_tvboxes_deduplicates_probe_results(monkeypatch):
    monkeypatch.setattr(tvbox, "_local_networks", lambda: [ipaddress.ip_network("192.168.1.0/30")])

    async def fake_probe(host, port, timeout):
        if host == "192.168.1.1" and port == 9979:
            return tvbox.TvboxDevice("http://192.168.1.1:9979", host, port, "TVBox", True)
        return None

    monkeypatch.setattr(tvbox, "_probe", fake_probe)
    result = asyncio.run(tvbox.scan_tvboxes(max_hosts=10))
    assert result == [{
        "endpoint": "http://192.168.1.1:9979",
        "host": "192.168.1.1",
        "port": 9979,
        "label": "TVBox",
        "matched": True,
    }]


def test_tvbox_push_form_preserves_signed_url():
    form = tvbox.tvbox_push_form("https://cdn.example/video.m3u8?token=a+b&expires=10")
    assert "do=push" in form
    assert "url=https%3A%2F%2Fcdn.example%2Fvideo.m3u8%3Ftoken%3Da%2Bb%26expires%3D10" in form


def test_local_media_share_only_exposes_selected_file_with_range_support(tmp_path, monkeypatch):
    source = tmp_path / "演示视频.mkv"
    source.write_bytes(b"0123456789")
    monkeypatch.setattr(tvbox, "local_address_for_tvbox", lambda _endpoint: "127.0.0.1")
    monkeypatch.setattr(tvbox, "private_ipv4_addresses", lambda _host: frozenset({"127.0.0.1"}))
    server = tvbox.LocalMediaServer()
    try:
        share = server.share(str(source), "http://192.168.1.20:9979")
        request = Request(share["url"], headers={"Range": "bytes=2-5"})
        with urlopen(request, timeout=3) as response:
            assert response.status == 206
            assert response.headers["Content-Range"] == "bytes 2-5/10"
            assert response.read() == b"2345"
        assert "/media/" in share["url"]
        assert str(source) not in share["url"]
    finally:
        server.shutdown()


def test_local_media_stream_share_waits_for_the_requested_range(monkeypatch):
    monkeypatch.setattr(tvbox, "local_address_for_tvbox", lambda _endpoint: "127.0.0.1")
    monkeypatch.setattr(tvbox, "private_ipv4_addresses", lambda _host: frozenset({"127.0.0.1"}))
    requested = []
    server = tvbox.LocalMediaServer()
    try:
        share = server.share_stream(
            filename="growing.mp4",
            size=10,
            endpoint="http://192.168.1.20:9979",
            read_range=lambda start, end: requested.append((start, end)) or b"0123456789"[start:end + 1],
            mime_type="video/mp4",
        )
        request = Request(share["url"], headers={"Range": "bytes=4-7"})
        with urlopen(request, timeout=3) as response:
            assert response.status == 206
            assert response.headers["Content-Range"] == "bytes 4-7/10"
            assert response.read() == b"4567"
        assert requested == [(4, 7)]
    finally:
        server.shutdown()


def test_local_media_playlist_share_serves_playlist_and_local_segments(monkeypatch):
    monkeypatch.setattr(tvbox, "local_address_for_tvbox", lambda _endpoint: "127.0.0.1")
    monkeypatch.setattr(tvbox, "private_ipv4_addresses", lambda _host: frozenset({"127.0.0.1"}))
    server = tvbox.LocalMediaServer()
    try:
        def read_asset(kind, name):
            if kind == "segments" and name == "000000.seg":
                return b"segment-bytes", "video/mp4"
            if kind == "maps" and name == "init-000000.init":
                return b"init-bytes", "video/mp4"
            raise ValueError("unexpected asset")

        share = server.share_playlist(
            filename="直播.m3u8",
            endpoint="http://192.168.1.20:9979",
            playlist=lambda: '#EXTM3U\n#EXT-X-MAP:URI="maps/init-000000.init?session=x&token=y"\n#EXTINF:1,\nsegments/000000.seg?session=x&token=y\n',
            read_asset=read_asset,
        )
        with urlopen(share["url"], timeout=3) as response:
            assert response.status == 200
            assert response.headers["Content-Type"].startswith("application/vnd.apple.mpegurl")
            playlist = response.read().decode()
        with urlopen(Request(share["url"], method="HEAD"), timeout=3) as response:
            assert int(response.headers["Content-Length"]) == len(playlist.encode())
        assert "segments/000000.seg" in playlist
        assert "maps/init-000000.init" in playlist
        segment_url = share["url"].replace("index.m3u8", "segments/000000.seg")
        with urlopen(segment_url, timeout=3) as response:
            assert response.status == 200
            assert response.read() == b"segment-bytes"
        map_url = share["url"].replace("index.m3u8", "maps/init-000000.init")
        with urlopen(Request(map_url, headers={"Range": "bytes=1-4"}), timeout=3) as response:
            assert response.status == 206
            assert response.headers["Content-Range"] == "bytes 1-4/10"
            assert response.read() == b"nit-"
    finally:
        server.shutdown()


def test_local_media_share_rejects_missing_files(monkeypatch, tmp_path):
    monkeypatch.setattr(tvbox, "local_address_for_tvbox", lambda _endpoint: "127.0.0.1")
    with pytest.raises(ValueError, match="找不到"):
        tvbox.LocalMediaServer().share(str(tmp_path / "missing.mp4"), "http://192.168.1.20:9979")


def test_local_media_share_is_removed_after_idle_timeout(tmp_path, monkeypatch):
    source = tmp_path / "movie.mp4"
    source.write_bytes(b"media")
    monkeypatch.setattr(tvbox, "local_address_for_tvbox", lambda _endpoint: "127.0.0.1")
    monkeypatch.setattr(tvbox, "private_ipv4_addresses", lambda _host: frozenset({"127.0.0.1"}))
    server = tvbox.LocalMediaServer()
    try:
        share = server.share(str(source), "http://192.168.1.20:9979")
        with server._lock:
            server._shares[share["id"]].last_access_at -= tvbox.LOCAL_MEDIA_IDLE_SECONDS + 1
            server._purge_locked()
            assert share["id"] not in server._shares
    finally:
        server.shutdown()


def test_local_media_share_can_be_revoked_immediately(tmp_path, monkeypatch):
    source = tmp_path / "movie.mp4"
    source.write_bytes(b"media")
    monkeypatch.setattr(tvbox, "local_address_for_tvbox", lambda _endpoint: "127.0.0.1")
    monkeypatch.setattr(tvbox, "private_ipv4_addresses", lambda _host: frozenset({"127.0.0.1"}))
    server = tvbox.LocalMediaServer()
    try:
        share = server.share(str(source), "http://192.168.1.20:9979")
        assert server.status(share["id"])["active"] is True
        assert server.revoke(share["id"]) is True
        assert server._begin_stream(share["id"]) is None
        assert server.status(share["id"]) == {"active": False}
    finally:
        server.shutdown()


def test_local_media_share_rejects_a_client_other_than_the_selected_tv(tmp_path, monkeypatch):
    source = tmp_path / "movie.mp4"
    source.write_bytes(b"media")
    monkeypatch.setattr(tvbox, "local_address_for_tvbox", lambda _endpoint: "192.168.1.2")
    monkeypatch.setattr(tvbox, "private_ipv4_addresses", lambda _host: frozenset({"192.168.1.20"}))
    server = tvbox.LocalMediaServer()
    try:
        share = server.share(str(source), "http://192.168.1.20:9979")
        assert server._begin_stream(share["id"], "192.168.1.20") is not None
        server._finish_stream(share["id"])
        assert server._begin_stream(share["id"], "192.168.1.21") is None
    finally:
        server.shutdown()


def test_local_address_rejects_loopback_device(monkeypatch):
    monkeypatch.setattr(tvbox, "private_ipv4_addresses", lambda _host: frozenset())
    assert tvbox.local_address_for_tvbox("http://127.0.0.1:9979") == ""
