import asyncio
import ipaddress

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
