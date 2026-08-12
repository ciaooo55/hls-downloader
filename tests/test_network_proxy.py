from backend.app.config import settings
import asyncio

import pytest

from backend.app.network_proxy import (
    HostNotAllowedError,
    PrivateDestinationError,
    PolicyAsyncClient,
    _normalize_system_proxy,
    _proxy_route,
    curl_proxy,
    ensure_url_allowed,
    ensure_public_destination,
    host_matches_patterns,
    httpx_proxy_options,
    reset_proxy_runtime_state,
)
import httpx


def test_manual_proxy_and_bypass(monkeypatch):
    monkeypatch.setattr(settings, "proxy_mode", "manual")
    monkeypatch.setattr(settings, "proxy_url", "socks5://127.0.0.1:1080")
    monkeypatch.setattr(settings, "proxy_bypass", ["localhost", "*.lan"])

    assert httpx_proxy_options("https://cdn.test/file") == {
        "proxy": "socks5://127.0.0.1:1080",
        "trust_env": False,
    }
    assert curl_proxy("https://cdn.test/file") == "socks5://127.0.0.1:1080"
    assert httpx_proxy_options("http://media.lan/file") == {"trust_env": False}
    assert curl_proxy("http://media.lan/file") == ""


def test_system_proxy_uses_environment(monkeypatch):
    monkeypatch.setattr(settings, "proxy_mode", "system")
    monkeypatch.setattr(settings, "proxy_bypass", [])
    reset_proxy_runtime_state()
    monkeypatch.setattr(
        "backend.app.network_proxy.urllib.request.getproxies",
        lambda: {},
    )
    assert httpx_proxy_options("https://cdn.test/file") == {"trust_env": True}
    assert curl_proxy("https://cdn.test/file") is None
    assert _proxy_route("https://cdn.test/file") == ("system", "")


def test_system_proxy_reads_os_settings_for_httpx_and_curl(monkeypatch):
    monkeypatch.setattr(settings, "proxy_mode", "system")
    monkeypatch.setattr(settings, "proxy_bypass", [])
    reset_proxy_runtime_state()
    monkeypatch.setattr(
        "backend.app.network_proxy.urllib.request.getproxies",
        lambda: {"http": "127.0.0.1:7890", "https": "127.0.0.1:7890"},
    )
    monkeypatch.setattr(
        "backend.app.network_proxy.urllib.request.proxy_bypass",
        lambda _host: False,
    )
    assert httpx_proxy_options("https://github.com/file") == {
        "proxy": "http://127.0.0.1:7890",
        "trust_env": False,
    }
    assert curl_proxy("https://github.com/file") == "http://127.0.0.1:7890"
    assert _proxy_route("https://github.com/file") == ("proxy", "http://127.0.0.1:7890")


def test_system_proxy_honors_os_bypass_and_app_bypass(monkeypatch):
    monkeypatch.setattr(settings, "proxy_mode", "system")
    monkeypatch.setattr(settings, "proxy_bypass", ["*.lan"])
    reset_proxy_runtime_state()
    monkeypatch.setattr(
        "backend.app.network_proxy.urllib.request.getproxies",
        lambda: {"https": "http://127.0.0.1:7890"},
    )
    monkeypatch.setattr(
        "backend.app.network_proxy.urllib.request.proxy_bypass",
        lambda host: host == "intranet.test",
    )
    assert httpx_proxy_options("https://intranet.test/file") == {"trust_env": True}
    assert curl_proxy("https://intranet.test/file") is None
    assert httpx_proxy_options("http://media.lan/file") == {"trust_env": True}
    assert curl_proxy("https://github.com/file") == "http://127.0.0.1:7890"


def test_windows_socks_entry_is_normalized_to_socks5():
    assert _normalize_system_proxy("127.0.0.1:1080") == "http://127.0.0.1:1080"
    assert _normalize_system_proxy("socks://127.0.0.1:1080") == "socks5://127.0.0.1:1080"
    assert _normalize_system_proxy("socks4://127.0.0.1:1080") == "socks5://127.0.0.1:1080"
    assert _normalize_system_proxy("socks5://127.0.0.1:1080") == "socks5://127.0.0.1:1080"


def test_proxy_bypass_supports_ipv6_cidr_suffix_and_port(monkeypatch):
    monkeypatch.setattr(settings, "proxy_mode", "manual")
    monkeypatch.setattr(settings, "proxy_url", "http://127.0.0.1:8080")
    monkeypatch.setattr(settings, "proxy_bypass", ["10.0.0.0/8", "2001:db8::/32", ".example.test", "media.test:8443"])

    for url in (
        "http://10.2.3.4/file",
        "http://[2001:db8::1234]/file",
        "https://cdn.example.test/file",
        "https://media.test:8443/file",
    ):
        assert httpx_proxy_options(url) == {"trust_env": False}
    assert httpx_proxy_options("https://media.test/file")["proxy"] == "http://127.0.0.1:8080"


def test_allowed_host_patterns_cover_wildcards_and_redirect_destinations(monkeypatch):
    monkeypatch.setattr(settings, "allowed_hosts", ["media.example.test", "*.cdn.example.test", "10.0.0.0/8"])

    ensure_url_allowed("https://media.example.test/master.m3u8")
    ensure_url_allowed("https://edge.cdn.example.test/segment.ts")
    ensure_url_allowed("http://10.5.6.7/file")
    with pytest.raises(HostNotAllowedError):
        ensure_url_allowed("https://redirect.attacker.test/file")

    options = httpx_proxy_options("https://media.example.test/master.m3u8")
    hook = options["event_hooks"]["request"][0]
    request = type("Request", (), {"url": "https://redirect.attacker.test/file"})()
    with pytest.raises(HostNotAllowedError):
        asyncio.run(hook(request))


def test_host_pattern_does_not_confuse_ipv6_colons_with_ports(monkeypatch):
    monkeypatch.setattr(settings, "proxy_mode", "system")
    reset_proxy_runtime_state()
    monkeypatch.setattr(
        "backend.app.network_proxy.urllib.request.getproxies",
        lambda: {},
    )
    assert host_matches_patterns("http://[::1]:8765/", ["::1"])
    assert host_matches_patterns("http://[::1]:8765/", ["[::1]:8765"])
    assert not host_matches_patterns("http://[::1]:9000/", ["[::1]:8765"])
    assert curl_proxy("https://cdn.test/file") is None


def test_browser_destination_policy_blocks_loopback_and_private_dns(monkeypatch):
    monkeypatch.setattr(settings, "allowed_hosts", [])
    reset_proxy_runtime_state()

    async def run():
        with pytest.raises(PrivateDestinationError):
            await ensure_public_destination("http://127.0.0.1:8765/api/settings")

        monkeypatch.setattr(
            "backend.app.network_proxy.socket.getaddrinfo",
            lambda *_args, **_kwargs: [
                (2, 1, 6, "", ("192.168.1.20", 443)),
            ],
        )
        with pytest.raises(PrivateDestinationError):
            await ensure_public_destination("https://rebinding.example/file")

    asyncio.run(run())


def test_browser_destination_policy_allows_dns_proxy_fake_ip_but_not_literal(monkeypatch):
    monkeypatch.setattr(settings, "allowed_hosts", [])
    reset_proxy_runtime_state()
    monkeypatch.setattr(
        "backend.app.network_proxy.socket.getaddrinfo",
        lambda *_args, **_kwargs: [
            (2, 1, 6, "", ("198.18.2.67", 443)),
        ],
    )

    async def run():
        # A domain resolved by a local proxy/TUN's Fake-IP DNS is safe to pass
        # through that routing layer. The same reserved address typed directly
        # remains blocked as a browser-originated destination.
        await ensure_public_destination("https://media.example.test/master.m3u8")
        with pytest.raises(PrivateDestinationError):
            await ensure_public_destination("https://198.18.2.67/master.m3u8")

    asyncio.run(run())


def test_browser_destination_policy_rejects_fake_ip_mixed_with_private_dns(monkeypatch):
    monkeypatch.setattr(settings, "allowed_hosts", [])
    reset_proxy_runtime_state()
    monkeypatch.setattr(
        "backend.app.network_proxy.socket.getaddrinfo",
        lambda *_args, **_kwargs: [
            (2, 1, 6, "", ("198.18.2.67", 443)),
            (2, 1, 6, "", ("192.168.1.20", 443)),
        ],
    )

    async def run():
        with pytest.raises(PrivateDestinationError):
            await ensure_public_destination("https://rebinding.example/master.m3u8")

    asyncio.run(run())


def test_browser_destination_policy_caches_successful_public_dns(monkeypatch):
    monkeypatch.setattr(settings, "allowed_hosts", [])
    reset_proxy_runtime_state()
    lookups = {"count": 0}

    def fake_getaddrinfo(*_args, **_kwargs):
        lookups["count"] += 1
        return [(2, 1, 6, "", ("8.8.8.8", 443))]

    monkeypatch.setattr("backend.app.network_proxy.socket.getaddrinfo", fake_getaddrinfo)

    async def run():
        await ensure_public_destination("https://cdn.example.test/a.ts")
        await ensure_public_destination("https://cdn.example.test/b.ts")

    asyncio.run(run())
    assert lookups["count"] == 1


def test_manual_destination_policy_still_allows_lan_addresses(monkeypatch):
    monkeypatch.setattr(settings, "allowed_hosts", [])
    ensure_url_allowed("http://192.168.1.20/file")


def test_policy_client_recalculates_proxy_route_after_redirect(monkeypatch):
    monkeypatch.setattr(settings, "proxy_mode", "manual")
    monkeypatch.setattr(settings, "proxy_url", "http://127.0.0.1:8080")
    monkeypatch.setattr(settings, "proxy_bypass", ["*.lan"])
    monkeypatch.setattr(settings, "allowed_hosts", [])
    seen_routes = []

    async def handler(request: httpx.Request) -> httpx.Response:
        if request.url.host == "origin.test":
            return httpx.Response(302, headers={"Location": "http://media.lan/file"}, request=request)
        return httpx.Response(200, content=b"ok", request=request)

    async def run():
        raw = httpx.AsyncClient(transport=httpx.MockTransport(handler))
        client = PolicyAsyncClient(follow_redirects=True)

        def select(url: str):
            seen_routes.append(_proxy_route(url))
            ensure_url_allowed(url)
            return raw

        monkeypatch.setattr(client, "_client_for", select)
        try:
            response = await client.get("https://origin.test/file")
            assert response.status_code == 200
            assert str(response.url) == "http://media.lan/file"
        finally:
            await raw.aclose()

    asyncio.run(run())
    assert ("proxy", "http://127.0.0.1:8080") in seen_routes
    assert ("direct", "") in seen_routes


def test_policy_client_rejects_disallowed_redirect_before_second_request(monkeypatch):
    monkeypatch.setattr(settings, "proxy_mode", "direct")
    monkeypatch.setattr(settings, "allowed_hosts", ["origin.test"])
    requested = []

    async def handler(request: httpx.Request) -> httpx.Response:
        requested.append(str(request.url))
        return httpx.Response(
            302,
            headers={"Location": "https://attacker.test/file"},
            request=request,
        )

    async def run():
        async with PolicyAsyncClient(
            follow_redirects=True,
            transport=httpx.MockTransport(handler),
        ) as client:
            with pytest.raises(HostNotAllowedError):
                await client.get("https://origin.test/file")

    asyncio.run(run())
    assert requested == ["https://origin.test/file"]


def test_network_budget_limits_each_host_and_shares_rate_limit_backoff(monkeypatch):
    from backend.app import network_proxy

    monkeypatch.setattr(network_proxy, "GLOBAL_CONNECTION_LIMIT", 3)
    monkeypatch.setattr(network_proxy, "PER_HOST_CONNECTION_LIMIT", 1)
    monkeypatch.setattr(settings, "allowed_hosts", [])
    budget = network_proxy.NetworkBudget()
    active = 0
    maximum = 0

    async def worker():
        nonlocal active, maximum
        async with budget.slot("https://cdn.example.test/file"):
            active += 1
            maximum = max(maximum, active)
            await asyncio.sleep(0.01)
            active -= 1

    async def run():
        await asyncio.gather(*(worker() for _ in range(5)))
        budget.record_response(
            "https://cdn.example.test/file",
            429,
            {"retry-after": "2"},
        )
        state = budget._state()
        host = budget._host("https://cdn.example.test/file")
        assert state.retry_until[host] > network_proxy.time.monotonic() + 1

    asyncio.run(run())
    assert maximum == 1
