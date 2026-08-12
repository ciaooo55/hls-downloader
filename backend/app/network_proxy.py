from __future__ import annotations

import asyncio
import socket
import time
import urllib.request
import weakref
from collections import OrderedDict
from contextlib import asynccontextmanager
from email.utils import parsedate_to_datetime
from fnmatch import fnmatch
from ipaddress import IPv6Address, ip_address, ip_network
from typing import Any
from urllib.parse import urlsplit

import httpx

from .config import settings


class HostNotAllowedError(ValueError):
    pass


class PrivateDestinationError(HostNotAllowedError):
    pass


GLOBAL_CONNECTION_LIMIT = 128
PER_HOST_CONNECTION_LIMIT = 24
MAX_BUDGET_HOSTS = 512
# Clash and similar transparent proxy/TUN implementations intentionally hand
# out addresses from the RFC 2544 benchmarking block and recover the original
# hostname in their local routing layer. It is not public, so a literal URL
# must still be blocked; it is only tolerated for DNS names after the normal
# hostname and redirect policy have already run.
SYNTHETIC_PROXY_NETWORKS = (ip_network("198.18.0.0/15"),)


class _LoopBudget:
    def __init__(self) -> None:
        self.global_slots = asyncio.Semaphore(GLOBAL_CONNECTION_LIMIT)
        self.host_slots: OrderedDict[str, asyncio.Semaphore] = OrderedDict()
        self.host_active: dict[str, int] = {}
        self.retry_until: dict[str, float] = {}
        self.failures: dict[str, int] = {}


class NetworkBudget:
    """Process-wide HTTP connection budget and shared per-host backoff."""

    def __init__(self) -> None:
        self._states: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()

    def _state(self) -> _LoopBudget:
        loop = asyncio.get_running_loop()
        state = self._states.get(loop)
        if state is None:
            state = _LoopBudget()
            self._states[loop] = state
        return state

    @staticmethod
    def _host(url: str) -> str:
        host, port = _url_host_port(url)
        return f"{host}:{port}" if host and port else host or "<invalid>"

    def _host_semaphore(self, state: _LoopBudget, host: str) -> asyncio.Semaphore:
        semaphore = state.host_slots.get(host)
        if semaphore is None:
            semaphore = asyncio.Semaphore(PER_HOST_CONNECTION_LIMIT)
            state.host_slots[host] = semaphore
        else:
            state.host_slots.move_to_end(host)
        if len(state.host_slots) > MAX_BUDGET_HOSTS:
            for stale_host, _stale in list(state.host_slots.items()):
                if stale_host != host and state.host_active.get(stale_host, 0) == 0:
                    state.host_slots.pop(stale_host, None)
                    state.host_active.pop(stale_host, None)
                    state.retry_until.pop(stale_host, None)
                    state.failures.pop(stale_host, None)
                    if len(state.host_slots) <= MAX_BUDGET_HOSTS:
                        break
        return semaphore

    async def wait_for_host(self, url: str) -> None:
        state = self._state()
        host = self._host(url)
        remaining = state.retry_until.get(host, 0.0) - time.monotonic()
        if remaining > 0:
            await asyncio.sleep(remaining)

    @asynccontextmanager
    async def slot(self, url: str):
        ensure_url_allowed(url)
        state = self._state()
        host = self._host(url)
        await self.wait_for_host(url)
        host_slots = self._host_semaphore(state, host)
        await host_slots.acquire()
        state.host_active[host] = state.host_active.get(host, 0) + 1
        try:
            await state.global_slots.acquire()
            try:
                yield
            finally:
                state.global_slots.release()
        finally:
            state.host_active[host] = max(0, state.host_active.get(host, 1) - 1)
            host_slots.release()

    def record_response(self, url: str, status_code: int, headers: Any = None) -> None:
        state = self._state()
        host = self._host(url)
        if status_code not in {429, 503}:
            if 200 <= status_code < 400:
                state.failures[host] = max(0, state.failures.get(host, 0) - 1)
                if not state.failures[host]:
                    state.retry_until.pop(host, None)
            return
        failures = min(6, state.failures.get(host, 0) + 1)
        state.failures[host] = failures
        retry_after = str((headers or {}).get("retry-after", "") or "").strip()
        delay = min(60.0, float(2 ** (failures - 1)))
        if retry_after:
            try:
                delay = max(delay, min(120.0, float(retry_after)))
            except ValueError:
                try:
                    delay = max(
                        delay,
                        min(120.0, parsedate_to_datetime(retry_after).timestamp() - time.time()),
                    )
                except (TypeError, ValueError, OverflowError):
                    pass
        state.retry_until[host] = max(
            state.retry_until.get(host, 0.0),
            time.monotonic() + max(0.0, delay),
        )


network_budget = NetworkBudget()


class _BudgetedAsyncStream(httpx.AsyncByteStream):
    def __init__(self, stream: httpx.AsyncByteStream, slot_context) -> None:
        self._stream = stream
        self._slot_context = slot_context
        self._released = False

    async def __aiter__(self):
        async for chunk in self._stream:
            yield chunk

    async def aclose(self) -> None:
        try:
            await self._stream.aclose()
        finally:
            if not self._released:
                self._released = True
                await self._slot_context.__aexit__(None, None, None)


def _url_host_port(url: str) -> tuple[str, int | None]:
    try:
        parsed = urlsplit(str(url or ""))
        return (parsed.hostname or "").rstrip(".").lower(), parsed.port
    except (TypeError, ValueError):
        return "", None


def _pattern_host_port(pattern: str) -> tuple[str, int | None]:
    value = str(pattern or "").strip().lower()
    if value.startswith("["):
        closing = value.find("]")
        if closing > 0:
            host = value[1:closing]
            suffix = value[closing + 1:]
            if not suffix:
                return host, None
            if suffix.startswith(":") and suffix[1:].isdigit():
                return host, int(suffix[1:])
            return value, None
    # A single colon can be a domain/IPv4 port delimiter. Raw IPv6 literals
    # contain multiple colons and are kept intact.
    if value.count(":") == 1:
        host, port = value.rsplit(":", 1)
        if port.isdigit():
            return host.rstrip("."), int(port)
    return value.rstrip("."), None


def host_matches_patterns(url: str, patterns) -> bool:
    host, port = _url_host_port(url)
    if not host:
        return False
    try:
        address = ip_address(host)
    except ValueError:
        address = None
    for raw_pattern in patterns or ():
        pattern, required_port = _pattern_host_port(raw_pattern)
        if not pattern or required_port is not None and port != required_port:
            continue
        if pattern == "*":
            return True
        if address is not None:
            try:
                if address in ip_network(pattern, strict=False):
                    return True
            except ValueError:
                pass
        suffix = pattern.removeprefix("*.").removeprefix(".")
        if pattern.startswith(("*.", ".")) and (host == suffix or host.endswith(f".{suffix}")):
            return True
        if fnmatch(host, pattern):
            return True
    return False


def url_is_allowed(url: str) -> bool:
    rules = getattr(settings, "allowed_hosts", None) or []
    return not rules or host_matches_patterns(url, rules)


def ensure_url_allowed(url: str) -> None:
    if url_is_allowed(url):
        return
    host, _ = _url_host_port(url)
    raise HostNotAllowedError(f"Host {host or '<invalid>'} not in allowed_hosts")


def _is_public_address(value: str) -> bool:
    try:
        address = ip_address(value.split("%", 1)[0])
    except ValueError:
        return False
    if isinstance(address, IPv6Address) and address.ipv4_mapped is not None:
        address = address.ipv4_mapped
    return bool(address.is_global)


def _is_synthetic_proxy_address(value: str) -> bool:
    """Return whether a DNS answer is a recognized local proxy fake IP."""
    try:
        address = ip_address(value.split("%", 1)[0])
    except ValueError:
        return False
    if isinstance(address, IPv6Address) and address.ipv4_mapped is not None:
        address = address.ipv4_mapped
    return any(address in network for network in SYNTHETIC_PROXY_NETWORKS)


# A multi-connection HTTP task and a segmented HLS/DASH stream repeat the
# public-destination check for every request and redirect hop.  Caching only
# *successful* validations for a short window removes a per-range DNS lookup
# from the hot path while keeping the DNS-rebinding window narrow; failures
# are never cached so a recovering resolver retries immediately.
_PUBLIC_HOST_TTL_SECONDS = 60.0
_PUBLIC_HOST_CACHE_LIMIT = 256
_public_host_cache: OrderedDict[tuple[str, int], float] = OrderedDict()


async def ensure_public_destination(url: str) -> None:
    """Reject browser-originated requests that resolve outside public IP space.

    Validation is repeated immediately before every request and redirect hop.
    Manual desktop tasks deliberately do not use this restriction so users can
    still download from a NAS or another explicitly entered LAN address.
    """
    ensure_url_allowed(url)
    host, port = _url_host_port(url)
    if not host:
        raise PrivateDestinationError("下载地址缺少有效主机名")
    try:
        literal = ip_address(host.split("%", 1)[0])
    except ValueError:
        literal = None
    if literal is not None:
        if not _is_public_address(host):
            raise PrivateDestinationError(f"浏览器来源任务禁止访问非公网地址 {host}")
        return
    cache_key = (host, int(port or 443))
    now = time.monotonic()
    cached_until = _public_host_cache.get(cache_key, 0.0)
    if cached_until > now:
        _public_host_cache.move_to_end(cache_key)
        return
    try:
        records = await asyncio.to_thread(
            socket.getaddrinfo,
            host,
            port or 443,
            socket.AF_UNSPEC,
            socket.SOCK_STREAM,
        )
    except socket.gaierror as exc:
        raise PrivateDestinationError(f"无法解析浏览器来源任务的主机 {host}") from exc
    addresses = {str(record[4][0]).split("%", 1)[0] for record in records if record[4]}
    # Never allow private, loopback or link-local results. The one exception
    # is a proxy/TUN fake IP supplied for a *hostname*; literal fake-IP URLs
    # were rejected above so this cannot turn the reserved block into a manual
    # browser-originated destination.
    if not addresses or any(
        not (_is_public_address(address) or _is_synthetic_proxy_address(address))
        for address in addresses
    ):
        raise PrivateDestinationError(f"浏览器来源任务禁止访问解析到非公网地址的主机 {host}")
    _public_host_cache[cache_key] = now + _PUBLIC_HOST_TTL_SECONDS
    _public_host_cache.move_to_end(cache_key)
    while len(_public_host_cache) > _PUBLIC_HOST_CACHE_LIMIT:
        _public_host_cache.popitem(last=False)


async def _validate_httpx_request(request) -> None:
    ensure_url_allowed(str(request.url))


def _bypassed(url: str) -> bool:
    return host_matches_patterns(url, settings.proxy_bypass)


# ``urllib.request.getproxies()`` prefers *_PROXY environment variables and
# falls back to the OS configuration (WinINET registry on Windows, System
# Configuration on macOS).  IDM and browsers read the same Windows settings,
# so resolving them explicitly keeps "system" mode consistent across httpx
# and curl-cffi instead of leaving curl with environment variables only.
# A short TTL keeps toggling a local proxy client (Clash/v2rayN) effective
# for new requests without re-reading the registry on every byte range.
_SYSTEM_PROXY_TTL_SECONDS = 15.0
_system_proxy_state: tuple[float, dict[str, str]] = (0.0, {})


def _normalize_system_proxy(value: str) -> str:
    text = str(value or "").strip()
    if not text:
        return ""
    if "://" not in text:
        return f"http://{text}"
    lowered = text.lower()
    # CPython labels a bare Windows SOCKS entry socks4; every contemporary
    # local proxy (Clash, v2rayN, sing-box, SSR) speaks SOCKS5 with remote
    # DNS, and httpx/socksio only implements SOCKS5.  Normalizing here keeps
    # httpx and curl-cffi behavior identical.
    if lowered.startswith(("socks://", "socks4://", "socks4a://")):
        # httpx/httpcore speak SOCKS5; curl-cffi accepts the same URL.
        return "socks5://" + text.split("://", 1)[1]
    return text


def reset_proxy_runtime_state() -> None:
    """Drop cached OS proxy and public-host DNS results (tests / settings)."""
    global _system_proxy_state
    _system_proxy_state = (0.0, {})
    _public_host_cache.clear()


def system_proxies(*, now: float | None = None) -> dict[str, str]:
    """Return scheme -> proxy URL from environment or OS settings (cached)."""
    global _system_proxy_state
    current = time.monotonic() if now is None else float(now)
    expires, cached = _system_proxy_state
    if current < expires:
        return cached
    try:
        raw = urllib.request.getproxies()
    except Exception:
        raw = {}
    proxies = {
        str(scheme).lower(): _normalize_system_proxy(value)
        for scheme, value in raw.items()
        if str(scheme).lower() != "no" and str(value or "").strip()
    }
    _system_proxy_state = (current + _SYSTEM_PROXY_TTL_SECONDS, proxies)
    return proxies


def system_proxy_for(url: str) -> str:
    """Resolve the effective system proxy for one URL, honoring bypass rules."""
    proxies = system_proxies()
    if not proxies:
        return ""
    try:
        parsed = urlsplit(str(url or ""))
        scheme = (parsed.scheme or "http").lower()
        host = parsed.hostname or ""
    except (TypeError, ValueError):
        return ""
    if host:
        try:
            # Covers NO_PROXY and the Windows ProxyOverride/<local> list.
            if urllib.request.proxy_bypass(host):
                return ""
        except Exception:
            pass
    return proxies.get(scheme, "") or proxies.get("all", "")


def httpx_proxy_options(url: str) -> dict:
    """Build explicit httpx proxy policy for a task URL."""
    mode = str(getattr(settings, "proxy_mode", "system") or "system").lower()
    options = {"event_hooks": {"request": [_validate_httpx_request]}} if settings.allowed_hosts else {}
    if mode == "system":
        proxy = "" if _bypassed(url) else system_proxy_for(url)
        if proxy:
            return {"proxy": proxy, "trust_env": False, **options}
        return {"trust_env": True, **options}
    if mode == "manual" and settings.proxy_url and not _bypassed(url):
        return {"proxy": settings.proxy_url, "trust_env": False, **options}
    return {"trust_env": False, **options}


def _proxy_route(url: str) -> tuple[str, str]:
    """Return a stable transport route, recalculated for every request URL."""
    mode = str(getattr(settings, "proxy_mode", "system") or "system").lower()
    if mode == "system":
        # Resolve the concrete proxy (environment first, then the Windows /
        # macOS system configuration) so redirects and long-lived tasks react
        # when the user toggles a local proxy client mid-session.
        proxy = "" if _bypassed(url) else system_proxy_for(url)
        if proxy:
            return "proxy", proxy
        return "system", ""
    if mode == "manual" and settings.proxy_url and not _bypassed(url):
        return "proxy", str(settings.proxy_url)
    return "direct", ""


class PolicyAsyncClient:
    """Small httpx facade with per-request proxy and per-hop redirect policy.

    A single ``httpx.AsyncClient(proxy=...)`` fixes routing to the URL used at
    construction time. Media manifests commonly redirect or reference another
    CDN host, so the bypass/allowed-host decision must be repeated for every
    request and redirect destination.
    """

    def __init__(
        self,
        *,
        follow_redirects: bool = True,
        max_redirects: int = 10,
        deny_private_networks: bool = False,
        **kwargs: Any,
    ) -> None:
        self.follow_redirects = bool(follow_redirects)
        self.max_redirects = max(0, int(max_redirects))
        self.deny_private_networks = bool(deny_private_networks)
        self._kwargs = dict(kwargs)
        self._kwargs.pop("proxy", None)
        self._kwargs.pop("trust_env", None)
        self._kwargs.pop("follow_redirects", None)
        self._kwargs.pop("event_hooks", None)
        self._clients: dict[tuple[str, str], httpx.AsyncClient] = {}

    def _client_for(self, url: str) -> httpx.AsyncClient:
        ensure_url_allowed(url)
        route = _proxy_route(url)
        existing = self._clients.get(route)
        if existing is not None:
            return existing
        kind, proxy = route
        options = dict(self._kwargs)
        options["follow_redirects"] = False
        options["trust_env"] = kind == "system"
        if kind == "proxy":
            options["proxy"] = proxy
        client = httpx.AsyncClient(**options)
        self._clients[route] = client
        return client

    def build_request(self, method: str, url: str, **kwargs: Any) -> httpx.Request:
        return self._client_for(str(url)).build_request(method, url, **kwargs)

    async def send(
        self,
        request: httpx.Request,
        *,
        stream: bool = False,
        follow_redirects: bool | None = None,
    ) -> httpx.Response:
        should_redirect = self.follow_redirects if follow_redirects is None else bool(follow_redirects)
        history: list[httpx.Response] = []
        current = request
        for _hop in range(self.max_redirects + 1):
            url = str(current.url)
            if self.deny_private_networks:
                await ensure_public_destination(url)
            client = self._client_for(url)
            slot_context = network_budget.slot(url)
            await slot_context.__aenter__()
            try:
                response = await client.send(current, stream=stream, follow_redirects=False)
            except BaseException:
                await slot_context.__aexit__(None, None, None)
                raise
            network_budget.record_response(url, response.status_code, response.headers)
            if stream:
                response.stream = _BudgetedAsyncStream(response.stream, slot_context)
            else:
                await slot_context.__aexit__(None, None, None)
            next_request = response.next_request if should_redirect else None
            if next_request is None:
                response.history = history
                return response
            history.append(response)
            ensure_url_allowed(str(next_request.url))
            await response.aclose()
            current = next_request
        for response in history:
            await response.aclose()
        raise httpx.TooManyRedirects(
            f"Exceeded maximum allowed redirects ({self.max_redirects})",
            request=current,
        )

    async def request(self, method: str, url: str, **kwargs: Any) -> httpx.Response:
        follow_redirects = kwargs.pop("follow_redirects", None)
        request = self.build_request(method, url, **kwargs)
        return await self.send(request, follow_redirects=follow_redirects)

    async def get(self, url: str, **kwargs: Any) -> httpx.Response:
        return await self.request("GET", url, **kwargs)

    async def head(self, url: str, **kwargs: Any) -> httpx.Response:
        return await self.request("HEAD", url, **kwargs)

    async def post(self, url: str, **kwargs: Any) -> httpx.Response:
        return await self.request("POST", url, **kwargs)

    @asynccontextmanager
    async def stream(self, method: str, url: str, **kwargs: Any):
        follow_redirects = kwargs.pop("follow_redirects", None)
        request = self.build_request(method, url, **kwargs)
        response = await self.send(request, stream=True, follow_redirects=follow_redirects)
        try:
            yield response
        finally:
            await response.aclose()

    async def aclose(self) -> None:
        clients = list(self._clients.values())
        self._clients.clear()
        for client in clients:
            await client.aclose()

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args):
        await self.aclose()


def policy_httpx_client(**kwargs: Any) -> PolicyAsyncClient:
    return PolicyAsyncClient(**kwargs)


def curl_proxy(url: str) -> str | None:
    """Return curl-cffi's proxy argument; an empty string disables env proxy."""
    mode = str(getattr(settings, "proxy_mode", "system") or "system").lower()
    if mode == "system":
        # curl only reads environment variables on its own; hand it the same
        # explicitly resolved system proxy that the httpx clients use so HLS
        # and browser-profile transfers follow WinINET settings too.
        proxy = "" if _bypassed(url) else system_proxy_for(url)
        return proxy or None
    if mode == "manual" and settings.proxy_url and not _bypassed(url):
        return settings.proxy_url
    return ""
