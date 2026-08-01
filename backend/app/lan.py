from __future__ import annotations

import ipaddress
import socket
from urllib.parse import urljoin, urlparse


_REDIRECT_STATUSES = {301, 302, 303, 307, 308}


def private_ipv4_addresses(host: str) -> frozenset[str]:
    """Resolve a LAN host without accepting loopback or special-use targets."""

    value = str(host or "").strip().rstrip(".")
    if not value:
        return frozenset()
    addresses: set[str] = set()
    try:
        records = socket.getaddrinfo(value, None, socket.AF_INET, socket.SOCK_STREAM)
    except OSError:
        return frozenset()
    for record in records:
        try:
            address = ipaddress.IPv4Address(record[4][0])
        except (IndexError, ValueError):
            continue
        if (
            address.is_private
            and not address.is_loopback
            and not address.is_link_local
            and not address.is_multicast
            and not address.is_unspecified
            and not address.is_reserved
        ):
            addresses.add(str(address))
    return frozenset(addresses)


def validate_lan_url(value: str, *, expected_addresses: frozenset[str] | None = None) -> tuple[str, frozenset[str]]:
    raw = str(value or "").strip()
    if any(ord(character) < 32 or ord(character) == 127 for character in raw):
        raise ValueError("局域网设备地址包含无效控制字符")
    parsed = urlparse(raw)
    try:
        port = parsed.port
    except ValueError as exc:
        raise ValueError("局域网设备地址的端口无效") from exc
    if (
        parsed.scheme.lower() not in {"http", "https"}
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.fragment
    ):
        raise ValueError("局域网设备地址必须是有效且不含账号或片段的 HTTP(S) 地址")
    if port == 0:
        raise ValueError("局域网设备地址的端口无效")
    addresses = private_ipv4_addresses(parsed.hostname)
    if not addresses:
        raise ValueError("设备地址不是可访问的 IPv4 局域网地址")
    if expected_addresses is not None and addresses.isdisjoint(expected_addresses):
        raise ValueError("设备响应跳转到了另一台主机，已拒绝访问")
    return raw, addresses


async def request_lan(
    client,
    method: str,
    url: str,
    *,
    expected_addresses: frozenset[str] | None = None,
    max_redirects: int = 3,
    max_response_bytes: int = 2 * 1024 * 1024,
    **kwargs,
):
    """Issue a bounded request while validating every redirect as the same LAN device."""

    import httpx

    current, initial_addresses = validate_lan_url(url, expected_addresses=expected_addresses)
    device_addresses = expected_addresses or initial_addresses
    request_method = method.upper()
    request_kwargs = dict(kwargs)
    for redirect_count in range(max_redirects + 1):
        request = client.build_request(request_method, current, **request_kwargs)
        response = await client.send(request, stream=True, follow_redirects=False)
        try:
            if response.status_code in _REDIRECT_STATUSES and response.headers.get("location"):
                if redirect_count >= max_redirects:
                    raise RuntimeError("局域网设备重定向次数过多")
                target = urljoin(current, response.headers["location"])
                current, _ = validate_lan_url(target, expected_addresses=device_addresses)
                if response.status_code == 303 or (
                    response.status_code in {301, 302} and request_method == "POST"
                ):
                    request_method = "GET"
                    request_kwargs.pop("content", None)
                    request_kwargs.pop("data", None)
                    request_kwargs.pop("json", None)
                continue

            declared = response.headers.get("content-length", "").strip()
            if declared:
                try:
                    if int(declared) > max_response_bytes:
                        raise RuntimeError("局域网设备响应过大")
                except ValueError:
                    pass
            content = bytearray()
            async for chunk in response.aiter_bytes():
                if len(content) + len(chunk) > max_response_bytes:
                    raise RuntimeError("局域网设备响应过大")
                content.extend(chunk)
            return httpx.Response(
                status_code=response.status_code,
                headers=response.headers,
                content=bytes(content),
                request=request,
                extensions=response.extensions,
            )
        finally:
            await response.aclose()
    raise RuntimeError("局域网设备请求失败")
