from __future__ import annotations

import asyncio
import atexit
import ipaddress
import itertools
import mimetypes
import re
import secrets
import socket
import subprocess
import threading
import time
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import quote, unquote, urlencode, urlparse, urlsplit


TVBOX_PORTS = (9978, 9979, 9977, 9976)
_TVBOX_MARKER = re.compile(r"tvbox|影视|vod|player|/action|push", re.IGNORECASE)
_RANGE = re.compile(r"bytes=(\d*)-(\d*)$", re.IGNORECASE)
LOCAL_MEDIA_MAX_SECONDS = 2 * 60 * 60
LOCAL_MEDIA_IDLE_SECONDS = 2 * 60


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


@dataclass
class LocalMediaShare:
    token: str
    path: Path
    expires_at: float
    last_access_at: float
    active_streams: int = 0


class LocalMediaServer:
    """Serve only explicitly selected local files through short-lived URLs.

    The main FastAPI service deliberately stays bound to loopback.  TVBox needs
    a LAN-reachable URL for a local file, so this isolated server exposes a
    random-token URL for one selected file rather than exposing the download
    API or an entire folder to the network.
    """

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._shares: dict[str, LocalMediaShare] = {}
        self._server: ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None
        self._reaper_stop = threading.Event()
        self._reaper: threading.Thread | None = None

    def _purge_locked(self) -> None:
        now = time.monotonic()
        expired = [token for token, share in self._shares.items() if (
            share.expires_at <= now
            or not share.path.is_file()
            or (share.active_streams == 0 and now - share.last_access_at >= LOCAL_MEDIA_IDLE_SECONDS)
        )]
        for token in expired:
            self._shares.pop(token, None)

    def _begin_stream(self, token: str) -> LocalMediaShare | None:
        with self._lock:
            self._purge_locked()
            share = self._shares.get(token)
            if share is not None:
                share.active_streams += 1
                share.last_access_at = time.monotonic()
            return share

    def _finish_stream(self, token: str) -> None:
        with self._lock:
            share = self._shares.get(token)
            if share is not None:
                share.active_streams = max(0, share.active_streams - 1)
                share.last_access_at = time.monotonic()

    @staticmethod
    def _read_range(value: str, size: int) -> tuple[int, int] | None:
        match = _RANGE.fullmatch(value.strip())
        if not match or size <= 0:
            return None
        start_raw, end_raw = match.groups()
        if not start_raw and not end_raw:
            return None
        if start_raw:
            start = int(start_raw)
            end = int(end_raw) if end_raw else size - 1
        else:
            suffix = int(end_raw)
            if suffix <= 0:
                return None
            start = max(0, size - suffix)
            end = size - 1
        if start >= size or end < start:
            return None
        return start, min(end, size - 1)

    def _handler(self):
        owner = self

        class Handler(BaseHTTPRequestHandler):
            server_version = "HLSDownloaderMedia/1.0"

            def log_message(self, _format: str, *_args) -> None:
                return

            def _serve(self, body: bool) -> None:
                parts = [unquote(item) for item in urlsplit(self.path).path.split("/") if item]
                if len(parts) != 3 or parts[0] != "media":
                    self.send_error(404)
                    return
                token = parts[1]
                share = owner._begin_stream(token)
                if share is None:
                    self.send_error(404)
                    return
                try:
                    try:
                        size = share.path.stat().st_size
                    except OSError:
                        self.send_error(404)
                        return
                    requested_range = self.headers.get("Range", "")
                    byte_range = owner._read_range(requested_range, size) if requested_range else None
                    if requested_range and byte_range is None:
                        self.send_response(416)
                        self.send_header("Content-Range", f"bytes */{size}")
                        self.end_headers()
                        return
                    start, end = byte_range or (0, max(0, size - 1))
                    length = 0 if size == 0 else end - start + 1
                    self.send_response(206 if byte_range else 200)
                    mime_type, _ = mimetypes.guess_type(share.path.name)
                    self.send_header("Content-Type", mime_type or "application/octet-stream")
                    self.send_header("Content-Length", str(length))
                    self.send_header("Accept-Ranges", "bytes")
                    self.send_header("Cache-Control", "no-store")
                    self.send_header("Content-Disposition", f"inline; filename*=UTF-8''{quote(share.path.name)}")
                    if byte_range:
                        self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
                    self.end_headers()
                    if not body or not length:
                        return
                    with share.path.open("rb") as source:
                        source.seek(start)
                        remaining = length
                        while remaining:
                            chunk = source.read(min(256 * 1024, remaining))
                            if not chunk:
                                break
                            self.wfile.write(chunk)
                            remaining -= len(chunk)
                except (BrokenPipeError, ConnectionResetError, OSError):
                    return
                finally:
                    owner._finish_stream(token)

            def do_GET(self) -> None:  # noqa: N802
                self._serve(True)

            def do_HEAD(self) -> None:  # noqa: N802
                self._serve(False)

        return Handler

    def _ensure_server_locked(self) -> int:
        if self._server is None:
            self._server = ThreadingHTTPServer(("0.0.0.0", 0), self._handler())
            self._server.daemon_threads = True
            self._thread = threading.Thread(target=self._server.serve_forever, name="tvbox-media", daemon=True)
            self._thread.start()
            self._reaper_stop.clear()
            self._reaper = threading.Thread(target=self._run_reaper, name="tvbox-media-reaper", daemon=True)
            self._reaper.start()
        return int(self._server.server_address[1])

    def _run_reaper(self) -> None:
        while not self._reaper_stop.wait(20):
            with self._lock:
                self._purge_locked()

    def share(self, file_path: str, endpoint: str) -> dict:
        path = Path(str(file_path or "")).expanduser()
        try:
            path = path.resolve(strict=True)
        except OSError as exc:
            raise ValueError("找不到要推送的本机文件") from exc
        if not path.is_file():
            raise ValueError("只能推送单个本机文件，不能推送文件夹")
        address = local_address_for_tvbox(endpoint)
        if not address:
            raise RuntimeError("无法确定电视所在局域网的电脑地址，请确认电脑和电视在同一局域网")
        with self._lock:
            self._purge_locked()
            port = self._ensure_server_locked()
            token = secrets.token_urlsafe(32)
            now = time.monotonic()
            self._shares[token] = LocalMediaShare(token, path, now + LOCAL_MEDIA_MAX_SECONDS, now)
        return {
            "id": token,
            "url": f"http://{address}:{port}/media/{token}/{quote(path.name)}",
            "filename": path.name,
            "size": path.stat().st_size,
            "expires_in_seconds": LOCAL_MEDIA_MAX_SECONDS,
            "idle_cleanup_seconds": LOCAL_MEDIA_IDLE_SECONDS,
        }

    def revoke(self, token: str) -> bool:
        with self._lock:
            return self._shares.pop(str(token or ""), None) is not None

    def status(self, token: str) -> dict:
        with self._lock:
            self._purge_locked()
            share = self._shares.get(str(token or ""))
            if share is None:
                return {"active": False}
            return {
                "active": True,
                "filename": share.path.name,
                "active_streams": share.active_streams,
                "expires_in_seconds": max(0, int(share.expires_at - time.monotonic())),
            }

    def shutdown(self) -> None:
        with self._lock:
            server = self._server
            self._server = None
            self._shares.clear()
            self._reaper_stop.set()
        if server is not None:
            server.shutdown()
            server.server_close()


def local_address_for_tvbox(endpoint: str) -> str:
    parsed = urlparse(normalize_tvbox_endpoint(endpoint))
    host = parsed.hostname
    if not host:
        return ""
    try:
        remote = socket.gethostbyname(host)
        if not ipaddress.ip_address(remote).is_private:
            return ""
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
            probe.connect((remote, parsed.port or 80))
            address = probe.getsockname()[0]
        return address if ipaddress.ip_address(address).is_private else ""
    except (OSError, ValueError):
        return ""


local_media_server = LocalMediaServer()
atexit.register(local_media_server.shutdown)


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
