from __future__ import annotations

import asyncio
import ipaddress
import itertools
import re
import socket
import subprocess
from dataclasses import dataclass
from urllib.parse import urlencode, urlparse


TVBOX_PORTS = (9978, 9979, 9977, 9976)
_TVBOX_MARKER = re.compile(r"tvbox|影视|vod|player|/action|push", re.IGNORECASE)


@dataclass(frozen=True)
class TvboxDevice:
    endpoint: str
    host: str
    port: int
    label: str
    matched: bool = False

    def public(self) -> dict:
        return {
            "endpoint": self.endpoint,
            "host": self.host,
            "port": self.port,
            "label": self.label,
            "matched": self.matched,
        }


def normalize_tvbox_endpoint(value: str) -> str:
    raw = str(value or "").strip().rstrip("/")
    if not raw:
        raise ValueError("请先选择或填写电视推送地址")
    parsed = urlparse(raw)
    if parsed.scheme.lower() not in {"http", "https"} or not parsed.netloc:
        raise ValueError("电视推送地址必须是 http:// 或 https:// 地址")
    # An endpoint is the base URL of the TVBox service.  User-info, query
    # strings and fragments are not part of the service address and can cause
    # us to POST to an unexpected host/path, so reject them explicitly.
    try:
        hostname = parsed.hostname
        parsed.port  # force validation of malformed ports (e.g. :abc)
    except ValueError as exc:
        raise ValueError("电视推送地址的主机或端口无效") from exc
    if not hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("电视推送地址不能包含账号、查询参数或片段")
    path = re.sub(r"/action/?$", "", parsed.path or "", flags=re.IGNORECASE).rstrip("/")
    return parsed._replace(scheme=parsed.scheme.lower(), path=path, fragment="").geturl().rstrip("/")


def tvbox_action_url(endpoint: str) -> str:
    return f"{normalize_tvbox_endpoint(endpoint)}/action"


def tvbox_push_form(url: str) -> str:
    return urlencode({"do": "push", "url": str(url or "")})


def _local_networks() -> list[ipaddress.IPv4Network]:
    values: list[tuple[str, str]] = []
    try:
        output = subprocess.run(
            ["ipconfig"], capture_output=True, text=True, timeout=3,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        ).stdout
        current_ip = ""
        for line in output.splitlines():
            # Windows ipconfig is localized.  Match both English and Chinese
            # labels and tolerate the dotted alignment characters it prints.
            ip_match = re.search(
                r"(?:IPv4|IPv4\s+Address|IPv4\s+地址|IP\s+Address)[^0-9]*"
                r"([0-9]{1,3}(?:\.[0-9]{1,3}){3})",
                line,
                re.IGNORECASE,
            )
            if ip_match:
                current_ip = ip_match.group(1)
                continue
            mask_match = re.search(
                r"(?:Subnet Mask|子网掩码)[^0-9]*"
                r"([0-9]{1,3}(?:\.[0-9]{1,3}){3})",
                line,
                re.IGNORECASE,
            )
            if mask_match and current_ip:
                values.append((current_ip, mask_match.group(1)))
                current_ip = ""
    except (OSError, subprocess.SubprocessError):
        pass
    if not values:
        try:
            for item in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET):
                address = item[4][0]
                values.append((address, "255.255.255.0"))
        except OSError:
            pass
    networks: list[ipaddress.IPv4Network] = []
    for address, mask in values:
        try:
            ip = ipaddress.IPv4Address(address)
            network = ipaddress.IPv4Network(f"{address}/{mask}", strict=False)
            if ip.is_private and not ip.is_loopback and not ip.is_link_local and network not in networks:
                networks.append(network)
        except ValueError:
            continue
    return networks


def scan_hosts(max_hosts: int = 512) -> list[str]:
    hosts: list[str] = []
    for network in _local_networks():
        # Do not turn a /16 into a multi-minute scan. Most home LANs are /24.
        # ``network.hosts()`` is a generator.  Do not materialize a /16 or
        # larger private network just to take the first few addresses.
        remaining = max_hosts - len(hosts)
        candidates = itertools.islice(network.hosts(), max(0, remaining))
        for address in candidates:
            value = str(address)
            if value not in hosts:
                hosts.append(value)
            if len(hosts) >= max_hosts:
                return hosts
    return hosts


async def _probe(host: str, port: int, timeout: float) -> TvboxDevice | None:
    writer: asyncio.StreamWriter | None = None
    try:
        reader, writer = await asyncio.wait_for(asyncio.open_connection(host, port), timeout)
        host_header = f"[{host}]" if ":" in host and not host.startswith("[") else host
        request = f"GET / HTTP/1.1\r\nHost: {host_header}:{port}\r\nConnection: close\r\n\r\n".encode()
        writer.write(request)
        await writer.drain()
        data = await asyncio.wait_for(reader.read(8192), timeout)
        text = data.decode("utf-8", errors="ignore")
        header, _, body = text.partition("\r\n\r\n")
        matched = bool(_TVBOX_MARKER.search(text))
        status = int(re.search(r"HTTP/\d(?:\.\d)?\s+(\d+)", header).group(1)) if re.search(r"HTTP/\d(?:\.\d)?\s+(\d+)", header) else 0
        if not status or status >= 500:
            return None
        # Known TVBox ports are useful even when a fork returns a blank root page.
        if not matched and port not in TVBOX_PORTS:
            return None
        label = "TVBox / 影视盒子" if matched else "局域网设备"
        return TvboxDevice(f"http://{host}:{port}", host, port, label, matched)
    except (OSError, asyncio.TimeoutError, ValueError):
        return None
    finally:
        if writer is not None:
            writer.close()
            try:
                await writer.wait_closed()
            except OSError:
                pass


async def scan_tvboxes(max_hosts: int = 512, timeout: float = 0.45) -> list[dict]:
    hosts = scan_hosts(max_hosts=max_hosts)
    semaphore = asyncio.Semaphore(64)

    async def probe(host: str, port: int):
        async with semaphore:
            return await _probe(host, port, timeout)

    results = await asyncio.gather(*(probe(host, port) for host in hosts for port in TVBOX_PORTS))
    devices: dict[str, TvboxDevice] = {}
    for device in results:
        if device:
            devices[device.endpoint] = device
    return [device.public() for device in sorted(devices.values(), key=lambda value: (not value.matched, value.host, value.port))]


async def push_tvbox(endpoint: str, url: str, timeout: float = 8.0) -> dict:
    import httpx

    target_url = str(url or "").strip()
    parsed_url = urlparse(target_url)
    if parsed_url.scheme.lower() not in {"http", "https"} or not parsed_url.hostname:
        raise ValueError("待推送的视频地址必须是有效的 HTTP(S) 地址")
    target = tvbox_action_url(endpoint)
    form = tvbox_push_form(target_url)
    async with httpx.AsyncClient(timeout=timeout, follow_redirects=True) as client:
        response = await client.post(target, content=form, headers={"Content-Type": "application/x-www-form-urlencoded"})
        if response.status_code in {404, 405}:
            response = await client.get(f"{target}?{form}")
        response.raise_for_status()
        text = response.text.strip()
        if text:
            try:
                body = response.json()
            except ValueError:
                body = None
            if isinstance(body, dict):
                # TVBox forks use several response conventions.  Treat
                # explicit negative flags and non-success numeric codes as
                # failures, while accepting a normal 200/true response.
                code = body.get("code")
                failed_code = isinstance(code, (int, float)) and not isinstance(code, bool) and code >= 400
                if (
                    body.get("ok") is False
                    or body.get("success") is False
                    or body.get("error")
                    or failed_code
                ):
                    raise RuntimeError(str(body.get("error") or body.get("message") or body.get("msg") or "电视拒绝了推送"))
            if re.match(r"^(?:error|fail|failed)\b", text, re.IGNORECASE):
                raise RuntimeError(text)
    return {"ok": True, "endpoint": normalize_tvbox_endpoint(endpoint)}
