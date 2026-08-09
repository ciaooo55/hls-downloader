import asyncio
import sys
from types import SimpleNamespace

import pytest

from backend.app import dlna


def test_private_lan_addresses_excludes_link_local_and_virtual_adapters(monkeypatch):
    adapters = [
        SimpleNamespace(name="Wi-Fi", nice_name="WLAN", ips=[
            SimpleNamespace(ip="169.254.1.9"),
            SimpleNamespace(ip="192.168.2.14"),
        ]),
        SimpleNamespace(name="VPN adapter", nice_name="Mihomo", ips=[SimpleNamespace(ip="10.0.0.2")]),
        SimpleNamespace(name="Ethernet", nice_name="Ethernet", ips=[SimpleNamespace(ip="172.20.0.8")]),
    ]
    monkeypatch.setitem(sys.modules, "ifaddr", SimpleNamespace(get_adapters=lambda: adapters))

    assert dlna._private_lan_addresses() == ["192.168.2.14", "172.20.0.8"]


def test_normalize_cast_device_keeps_a_discovered_renderer():
    device = dlna.normalize_cast_device({
        "location": "http://192.168.1.25:8200/description.xml",
        "control_url": "http://192.168.1.25:8200/upnp/control/AVTransport1",
        "service_type": "urn:schemas-upnp-org:service:AVTransport:1",
        "label": "Living Room TV",
        "host": "ignored-by-normalizer",
    })

    assert device["host"] == "192.168.1.25"
    assert device["label"] == "Living Room TV"
    assert device["protocol"] == "dlna"
    assert device["id"] == "dlna:http://192.168.1.25:8200/upnp/control/AVTransport1"


def test_normalize_cast_device_accepts_a_discovered_chromecast():
    device = dlna.normalize_cast_device({
        "id": "0a5b5c58-3524-4e69-b245-6e0f9cf39024",
        "protocol": "chromecast",
        "location": "http://192.168.1.30",
        "label": "客厅电视",
        "host": "192.168.1.30",
    })

    assert device["protocol"] == "chromecast"
    assert device["control_url"] == ""


def test_normalize_cast_device_rejects_an_unrelated_control_host():
    try:
        dlna.normalize_cast_device({
            "location": "http://192.168.1.25/description.xml",
            "control_url": "http://192.168.1.30/control",
            "service_type": "urn:schemas-upnp-org:service:AVTransport:1",
        })
    except ValueError as exc:
        assert "无效" in str(exc)
    else:
        raise AssertionError("Expected invalid cast device to be rejected")


def test_ssdp_location_rejects_public_and_loopback_targets(monkeypatch):
    def resolve(host):
        return frozenset({"192.168.1.20"}) if host == "tv.lan" else frozenset()

    monkeypatch.setattr("backend.app.lan.private_ipv4_addresses", resolve)
    assert dlna._parse_ssdp_location(b"HTTP/1.1 200 OK\r\nLOCATION: http://tv.lan/desc.xml\r\n\r\n")
    assert dlna._parse_ssdp_location(b"HTTP/1.1 200 OK\r\nLOCATION: http://127.0.0.1/private\r\n\r\n") == ""
    assert dlna._parse_ssdp_location(b"HTTP/1.1 200 OK\r\nLOCATION: https://example.com/desc.xml\r\n\r\n") == ""


def test_scan_cast_devices_describes_unique_ssdp_locations(monkeypatch):
    monkeypatch.setattr(dlna, "_search_ssdp", lambda _timeout: ["http://192.168.1.20/a.xml", "http://192.168.1.20/b.xml"])

    async def fake_describe(location, _timeout):
        return dlna.CastDevice(location, "http://192.168.1.20/control", "urn:schemas-upnp-org:service:AVTransport:1", "电视", "192.168.1.20")

    monkeypatch.setattr(dlna, "_describe", fake_describe)
    monkeypatch.setattr(dlna, "_scan_chromecasts", lambda _timeout: [])
    devices = asyncio.run(dlna.scan_cast_devices())

    assert devices == [{
        "id": "dlna:http://192.168.1.20/control",
        "protocol": "dlna",
        "location": "http://192.168.1.20/a.xml",
        "control_url": "http://192.168.1.20/control",
        "service_type": "urn:schemas-upnp-org:service:AVTransport:1",
        "label": "电视",
        "host": "192.168.1.20",
    }]


def test_scan_cast_devices_includes_chromecast(monkeypatch):
    monkeypatch.setattr(dlna, "_search_ssdp", lambda _timeout: [])
    monkeypatch.setattr(dlna, "_scan_chromecasts", lambda _timeout: [
        dlna.CastDevice("http://192.168.1.30", "", "", "客厅电视", "192.168.1.30", "chromecast", "cast-uuid"),
    ])

    devices = asyncio.run(dlna.scan_cast_devices())

    assert devices[0]["protocol"] == "chromecast"
    assert devices[0]["id"] == "cast-uuid"


def test_failed_chromecast_discovery_closes_zeroconf(monkeypatch):
    closed = []

    class FakeZeroconf:
        def __init__(self, **_kwargs):
            pass

        def close(self):
            closed.append(True)

    monkeypatch.setattr(dlna, "_private_lan_addresses", lambda: ["192.168.1.2"])
    monkeypatch.setitem(
        sys.modules,
        "zeroconf",
        SimpleNamespace(InterfaceChoice=SimpleNamespace(Default="default"), Zeroconf=FakeZeroconf),
    )
    monkeypatch.setitem(
        sys.modules,
        "pychromecast",
        SimpleNamespace(get_chromecasts=lambda **_kwargs: (_ for _ in ()).throw(RuntimeError("scan failed"))),
    )

    assert dlna._scan_chromecasts(1.0) == []
    assert closed == [True]


def test_ssdp_scan_uses_a_response_window_that_cannot_expire_before_mx():
    assert dlna.SSDP_RESPONSE_DELAY_SECONDS == 1
    assert dlna.MEDIA_RENDERER_TARGET_V2 in dlna.SSDP_TARGETS
    assert "urn:schemas-upnp-org:service:AVTransport:2" in dlna.SSDP_TARGETS


def test_cast_control_seeks_forward_from_the_reported_position(monkeypatch):
    calls = []

    async def fake_action(_device, action, arguments, timeout=8.0):
        calls.append((action, arguments))
        if action == "GetPositionInfo":
            return "<Envelope><RelTime>00:02:15</RelTime></Envelope>"
        return ""

    monkeypatch.setattr(dlna, "_av_transport_action", fake_action)
    device = {
        "location": "http://192.168.1.25/description.xml",
        "control_url": "http://192.168.1.25/control",
        "service_type": "urn:schemas-upnp-org:service:AVTransport:1",
    }

    asyncio.run(dlna.cast_control(device, "seek", 10))

    assert calls == [
        ("GetPositionInfo", {"InstanceID": "0"}),
        ("Seek", {"InstanceID": "0", "Unit": "REL_TIME", "Target": "00:02:25"}),
    ]


def test_didl_metadata_is_escaped_once_by_the_soap_envelope():
    metadata = dlna._didl_metadata("http://192.168.1.2/media/file.mp4", "测试.mp4")
    body = dlna._soap_body("SetAVTransportURI", "urn:schemas-upnp-org:service:AVTransport:1", {"CurrentURIMetaData": metadata})

    assert "&lt;DIDL-Lite" in body
    assert "&amp;lt;DIDL-Lite" not in body


def test_invalid_dlna_duration_is_rejected():
    with pytest.raises(ValueError):
        dlna._parse_duration("00:99:00")
    with pytest.raises(ValueError):
        dlna._parse_duration("00:00:61")
