import asyncio

import httpx
import pytest

from backend.app import lan


def test_validate_lan_url_rejects_loopback_and_public_hosts(monkeypatch):
    monkeypatch.setattr(
        lan,
        "private_ipv4_addresses",
        lambda host: frozenset({"192.168.1.20"}) if host == "tv.lan" else frozenset(),
    )
    assert lan.validate_lan_url("http://tv.lan:9979/action")[1] == frozenset({"192.168.1.20"})
    with pytest.raises(ValueError):
        lan.validate_lan_url("http://127.0.0.1/private")
    with pytest.raises(ValueError):
        lan.validate_lan_url("https://example.com/device")


def test_lan_request_rejects_redirect_to_another_host(monkeypatch):
    monkeypatch.setattr(
        lan,
        "private_ipv4_addresses",
        lambda host: {
            "tv.lan": frozenset({"192.168.1.20"}),
            "other.lan": frozenset({"192.168.1.21"}),
        }.get(host, frozenset()),
    )

    async def run():
        transport = httpx.MockTransport(
            lambda request: httpx.Response(302, headers={"Location": "http://other.lan/private"})
        )
        async with httpx.AsyncClient(transport=transport) as client:
            with pytest.raises(ValueError, match="另一台主机"):
                await lan.request_lan(client, "GET", "http://tv.lan/device")

    asyncio.run(run())


def test_lan_request_bounds_response_body(monkeypatch):
    monkeypatch.setattr(lan, "private_ipv4_addresses", lambda _host: frozenset({"192.168.1.20"}))

    async def run():
        transport = httpx.MockTransport(lambda request: httpx.Response(200, content=b"12345"))
        async with httpx.AsyncClient(transport=transport) as client:
            with pytest.raises(RuntimeError, match="响应过大"):
                await lan.request_lan(client, "GET", "http://tv.lan/device", max_response_bytes=4)

    asyncio.run(run())
