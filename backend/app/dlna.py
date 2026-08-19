from __future__ import annotations

import asyncio
import contextlib
import html
import ipaddress
import mimetypes
import selectors
import socket
import threading
import time
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from email.parser import BytesHeaderParser
from urllib.parse import urljoin, urlparse

from .lan import private_ipv4_addresses, request_lan, validate_lan_url


SSDP_ADDRESS = ("239.255.255.250", 1900)
SSDP_RESPONSE_DELAY_SECONDS = 1
MEDIA_RENDERER_TARGET = "urn:schemas-upnp-org:device:MediaRenderer:1"
MEDIA_RENDERER_TARGET_V2 = "urn:schemas-upnp-org:device:MediaRenderer:2"
SSDP_TARGETS = (
    MEDIA_RENDERER_TARGET,
    MEDIA_RENDERER_TARGET_V2,
    "urn:schemas-upnp-org:service:AVTransport:1",
    "urn:schemas-upnp-org:service:AVTransport:2",
    "ssdp:all",
)
AV_TRANSPORT_PREFIX = "urn:schemas-upnp-org:service:AVTransport:"
SOAP_ENVELOPE = "http://schemas.xmlsoap.org/soap/envelope/"


@dataclass(frozen=True)
class CastDevice:
    location: str
    control_url: str
    service_type: str
    label: str
    host: str
    protocol: str = "dlna"
    device_id: str = ""

    def public(self) -> dict[str, str]:
        return {
            "id": self.device_id or f"{self.protocol}:{self.control_url or self.host}",
            "protocol": self.protocol,
            "location": self.location,
            "control_url": self.control_url,
            "service_type": self.service_type,
            "label": self.label,
            "host": self.host,
        }


def normalize_cast_device(value: dict | None) -> dict[str, str]:
    if not isinstance(value, dict) or not value:
        return {}
    protocol = str(value.get("protocol") or "dlna").strip().lower()
    location = str(value.get("location") or "").strip()
    control_url = str(value.get("control_url") or "").strip()
    service_type = str(value.get("service_type") or "").strip()
    label = str(value.get("label") or "").strip()
    if protocol == "chromecast":
        host = str(value.get("host") or "").strip()
        device_id = str(value.get("id") or "").strip()
        if not host or not device_id:
            raise ValueError("Chromecast 设备信息无效，请重新扫描并选择设备")
        return CastDevice(
            location or f"http://{host}", "", "", label or host, host, protocol, device_id,
        ).public()
    if protocol != "dlna":
        raise ValueError("不支持的投屏设备类型，请重新扫描并选择设备")
    parsed_location = urlparse(location)
    parsed_control = urlparse(control_url)
    if (
        parsed_location.scheme.lower() not in {"http", "https"}
        or parsed_control.scheme.lower() not in {"http", "https"}
        or not parsed_location.hostname
        or not parsed_control.hostname
        or parsed_location.hostname.lower() != parsed_control.hostname.lower()
        or not service_type.startswith(AV_TRANSPORT_PREFIX)
    ):
        raise ValueError("投屏设备信息无效，请重新扫描并选择设备")
    return CastDevice(
        location, control_url, service_type, label or parsed_location.hostname, parsed_location.hostname,
        protocol, str(value.get("id") or f"dlna:{control_url}"),
    ).public()


def _parse_ssdp_location(payload: bytes) -> str:
    lines = payload.replace(b"\r\n", b"\n").split(b"\n", 1)
    headers = BytesHeaderParser().parsebytes(lines[1] if len(lines) > 1 else payload)
    location = str(headers.get("location") or "").strip()
    parsed = urlparse(location)
    if parsed.scheme.lower() not in {"http", "https"} or not parsed.hostname:
        return ""
    try:
        validate_lan_url(location)
    except ValueError:
        return ""
    return location


def _search_ssdp(timeout: float) -> list[str]:
    locations: set[str] = set()
    clients: list[socket.socket] = []
    selector = selectors.DefaultSelector()
    try:
        # SSDP replies are unicast to the source address. Sending only through
        # the first adapter misses TVs whenever Wi-Fi, Ethernet and virtual
        # adapters coexist, so probe every eligible private IPv4 interface.
        local_addresses = _private_lan_addresses() or [""]
        for address in local_addresses:
            client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
            try:
                if address:
                    client.bind((address, 0))
                    client.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(address))
                client.setblocking(False)
                selector.register(client, selectors.EVENT_READ)
                clients.append(client)
            except OSError:
                # A stale adapter can remain visible while Windows has already
                # removed its address. Continue with the working interfaces.
                client.close()
        if not clients:
            return []
        for target in SSDP_TARGETS:
            request = (
                "M-SEARCH * HTTP/1.1\r\n"
                "HOST: 239.255.255.250:1900\r\n"
                'MAN: "ssdp:discover"\r\n'
                # ``MX: 3`` permits a renderer to delay its response for up
                # to three seconds, while the old default scan only listened
                # for 2.5 seconds.  That made an otherwise healthy renderer
                # appear and disappear between scans.  One second is enough
                # for a LAN response and keeps the picker responsive.
                f"MX: {SSDP_RESPONSE_DELAY_SECONDS}\r\n"
                "USER-AGENT: HLSDownloader/1.6 UPnP/1.1\r\n"
                f"ST: {target}\r\n\r\n"
            ).encode("ascii")
            for client in clients:
                try:
                    client.sendto(request, SSDP_ADDRESS)
                except OSError:
                    # One adapter can disappear during a scan without making
                    # results from every other adapter unusable.
                    continue
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            for key, _event in selector.select(remaining):
                while True:
                    try:
                        response, _address = key.fileobj.recvfrom(65535)
                    except BlockingIOError:
                        break
                    except OSError:
                        break
                    location = _parse_ssdp_location(response)
                    if location:
                        locations.add(location)
    finally:
        selector.close()
        for client in clients:
            client.close()
    return sorted(locations)


def _private_lan_addresses() -> list[str]:
    addresses: list[str] = []
    try:
        import ifaddr

        for adapter in ifaddr.get_adapters():
            name = (adapter.nice_name or adapter.name or "").casefold()
            if any(marker in name for marker in ("mihomo", "vpn", "tunnel", "loopback", "virtual")):
                continue
            for item in adapter.ips:
                address = item.ip if isinstance(item.ip, str) else ""
                try:
                    parsed = ipaddress.ip_address(address)
                    if parsed.version == 4 and parsed.is_private and not parsed.is_loopback and not parsed.is_link_local:
                        if address not in addresses:
                            addresses.append(address)
                except ValueError:
                    continue
    except Exception:
        pass
    return addresses


def _scan_chromecasts(timeout: float) -> list[CastDevice]:
    zeroconf = None
    browser = None
    try:
        import pychromecast
        from zeroconf import InterfaceChoice, Zeroconf

        addresses = _private_lan_addresses()
        zeroconf = Zeroconf(interfaces=addresses or InterfaceChoice.Default)
        chromecasts, browser = pychromecast.get_chromecasts(
            timeout=timeout, tries=1, zeroconf_instance=zeroconf,
        )
    except Exception:
        # Zeroconf owns a background socket/thread. If discovery raises after
        # it was constructed, returning without closing it leaks one worker on
        # every device-picker refresh.
        if browser is not None:
            with contextlib.suppress(Exception):
                browser.stop_discovery()
        if zeroconf is not None:
            with contextlib.suppress(Exception):
                zeroconf.close()
        return []
    try:
        devices = []
        for chromecast in chromecasts:
            host = str(chromecast.host or "").strip()
            device_id = str(chromecast.uuid or "").strip()
            if host and device_id:
                label = str(chromecast.device.friendly_name or chromecast.name or host)
                devices.append(CastDevice(f"http://{host}", "", "", label, host, "chromecast", device_id))
        return devices
    finally:
        if browser is not None:
            with contextlib.suppress(Exception):
                browser.stop_discovery()
        if zeroconf is not None:
            with contextlib.suppress(Exception):
                zeroconf.close()


async def _describe(location: str, timeout: float) -> CastDevice | None:
    import httpx

    try:
        _, device_addresses = validate_lan_url(location)
        async with httpx.AsyncClient(timeout=timeout, follow_redirects=False, trust_env=False) as client:
            response = await request_lan(
                client, "GET", location, expected_addresses=device_addresses,
            )
        response.raise_for_status()
        root = ET.fromstring(response.content)
        device_ns = "{urn:schemas-upnp-org:device-1-0}"
        friendly_name = (root.findtext(f".//{device_ns}friendlyName") or "").strip()[:160]
        url_base = (root.findtext(f".//{device_ns}URLBase") or location).strip()
        for service in root.findall(f".//{device_ns}service"):
            service_type = (service.findtext(f"{device_ns}serviceType") or "").strip()
            control_path = (service.findtext(f"{device_ns}controlURL") or "").strip()
            if service_type.startswith(AV_TRANSPORT_PREFIX) and control_path:
                control_url = urljoin(url_base, control_path)
                parsed = urlparse(location)
                validate_lan_url(control_url, expected_addresses=device_addresses)
                candidate = CastDevice(
                    location,
                    control_url,
                    service_type,
                    friendly_name or parsed.hostname or "DLNA 设备",
                    parsed.hostname or "",
                )
                normalize_cast_device(candidate.public())
                return candidate
    except (httpx.HTTPError, ET.ParseError, RuntimeError, ValueError):
        return None
    return None


async def scan_cast_devices(timeout: float = 4.0) -> list[dict[str, str]]:
    locations, chromecasts = await asyncio.gather(
        asyncio.to_thread(_search_ssdp, timeout),
        asyncio.to_thread(_scan_chromecasts, max(3.0, timeout)),
    )
    devices = await asyncio.gather(*(_describe(location, timeout) for location in locations))
    unique: dict[str, CastDevice] = {}
    for device in [*devices, *chromecasts]:
        if device:
            unique.setdefault(device.public()["id"], device)
    return [device.public() for device in sorted(unique.values(), key=lambda device: (device.label.casefold(), device.host))]


def _media_mime(filename: str, media_url: str = "") -> str:
    """Guess a renderer MIME. stdlib maps `.ts` to a Qt linguist type on some platforms."""
    name = str(filename or "").strip().lower()
    url = str(media_url or "").split("?", 1)[0].strip().lower()
    leaf = name or url.rsplit("/", 1)[-1]
    if leaf.endswith((".m3u8", ".m3u")) or "/index.m3u8" in url:
        return "application/vnd.apple.mpegurl"
    if leaf.endswith((".mpd",)):
        return "application/dash+xml"
    if leaf.endswith((".ts", ".m2ts", ".mts")):
        return "video/mp2t"
    if leaf.endswith((".mp4", ".m4v")):
        return "video/mp4"
    if leaf.endswith((".webm",)):
        return "video/webm"
    if leaf.endswith((".mkv",)):
        return "video/x-matroska"
    guessed, _ = mimetypes.guess_type(leaf or "video.bin")
    return guessed or "application/octet-stream"


def _chromecast_stream_type(filename: str, media_url: str = "") -> str:
    mime = _media_mime(filename, media_url)
    if "mpegurl" in mime or mime == "application/dash+xml":
        return "LIVE"
    return "BUFFERED"


def _didl_metadata(media_url: str, filename: str) -> str:
    mime_type = _media_mime(filename, media_url)
    protocol = f"http-get:*:{mime_type}:*"
    title = html.escape(filename, quote=False)
    url = html.escape(media_url, quote=False)
    return (
        '<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" '
        'xmlns:dc="http://purl.org/dc/elements/1.1/" '
        'xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">'
        f'<item id="0" parentID="0" restricted="1"><dc:title>{title}</dc:title>'
        f'<upnp:class>object.item.videoItem</upnp:class><res protocolInfo="{protocol}">{url}</res>'
        '</item></DIDL-Lite>'
    )


def _soap_body(action: str, service_type: str, arguments: dict[str, str]) -> str:
    values = "".join(f"<{key}>{html.escape(value, quote=False)}</{key}>" for key, value in arguments.items())
    return (
        f'<s:Envelope xmlns:s="{SOAP_ENVELOPE}" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">'
        f'<s:Body><u:{action} xmlns:u="{service_type}">{values}</u:{action}></s:Body></s:Envelope>'
    )


async def _av_transport_action(device: dict, action: str, arguments: dict[str, str], timeout: float = 8.0) -> str:
    import httpx

    selected = normalize_cast_device(device)
    _, device_addresses = validate_lan_url(selected["location"])
    validate_lan_url(selected["control_url"], expected_addresses=device_addresses)
    response = None
    try:
        async with httpx.AsyncClient(timeout=timeout, follow_redirects=False, trust_env=False) as client:
            response = await request_lan(
                client,
                "POST",
                selected["control_url"],
                expected_addresses=device_addresses,
                content=_soap_body(action, selected["service_type"], arguments),
                headers={
                    "Content-Type": 'text/xml; charset="utf-8"',
                    "SOAPACTION": f'"{selected["service_type"]}#{action}"',
                },
            )
        response.raise_for_status()
    except httpx.HTTPError as exc:
        detail = response.text[:240] if response is not None else str(exc)
        raise RuntimeError(f"投屏设备拒绝了播放请求：{detail}") from exc
    return response.text


class _ChromecastHandle:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.chromecast = None
        self.browser = None
        self.chromecasts: list = []


_chromecast_guard = threading.Lock()
_chromecast_handles: dict[str, _ChromecastHandle] = {}


def _chromecast_handle(device_id: str) -> _ChromecastHandle:
    with _chromecast_guard:
        handle = _chromecast_handles.get(device_id)
        if handle is None:
            handle = _ChromecastHandle()
            _chromecast_handles[device_id] = handle
        return handle


def _disconnect_chromecast_handle(handle: _ChromecastHandle) -> None:
    chromecasts = handle.chromecasts
    browser = handle.browser
    handle.chromecast = None
    handle.browser = None
    handle.chromecasts = []
    for item in chromecasts:
        with contextlib.suppress(Exception):
            item.disconnect()
    if browser is not None:
        with contextlib.suppress(Exception):
            browser.stop_discovery()


def close_chromecast_session(device: dict | None = None) -> None:
    if device is None:
        with _chromecast_guard:
            handles = list(_chromecast_handles.values())
            _chromecast_handles.clear()
        for handle in handles:
            with handle.lock:
                _disconnect_chromecast_handle(handle)
        return
    selected = normalize_cast_device(device)
    with _chromecast_guard:
        handle = _chromecast_handles.pop(selected["id"], None)
    if handle is None:
        return
    with handle.lock:
        _disconnect_chromecast_handle(handle)


def _with_chromecast(device: dict, operation):
    import pychromecast

    selected = normalize_cast_device(device)
    if not private_ipv4_addresses(selected["host"]):
        raise ValueError("Chromecast 设备不是可访问的 IPv4 局域网设备")
    handle = _chromecast_handle(selected["id"])
    with handle.lock:
        if handle.chromecast is None:
            chromecasts, browser = pychromecast.get_chromecasts(
                known_hosts=[selected["host"]], timeout=8, tries=1,
            )
            try:
                chromecast = next((item for item in chromecasts if str(item.uuid) == selected["id"]), None)
                if chromecast is None:
                    raise RuntimeError("找不到已选择的 Chromecast 设备，请重新扫描")
                chromecast.wait(timeout=8)
            except Exception:
                for item in chromecasts:
                    with contextlib.suppress(Exception):
                        item.disconnect()
                with contextlib.suppress(Exception):
                    browser.stop_discovery()
                raise
            handle.chromecast = chromecast
            handle.browser = browser
            handle.chromecasts = list(chromecasts)
        try:
            return operation(handle.chromecast, selected)
        except Exception:
            _disconnect_chromecast_handle(handle)
            raise


def _cast_chromecast_media(device: dict, media_url: str, filename: str) -> None:
    mime_type = _media_mime(filename, media_url)
    stream_type = _chromecast_stream_type(filename, media_url)

    def play(chromecast, _selected):
        controller = chromecast.media_controller
        controller.play_media(media_url, mime_type, title=filename, stream_type=stream_type)
        controller.block_until_active(10)

    _with_chromecast(device, play)


async def cast_media(device: dict, media_url: str, filename: str) -> dict[str, str | bool]:
    selected = normalize_cast_device(device)
    if selected["protocol"] == "chromecast":
        try:
            await asyncio.to_thread(_cast_chromecast_media, selected, media_url, filename)
        except Exception as exc:
            raise RuntimeError(f"Chromecast 投屏失败：{exc}") from exc
        return {"ok": True, "label": selected["label"]}
    await _av_transport_action(selected, "SetAVTransportURI", {
        "InstanceID": "0",
        "CurrentURI": media_url,
        "CurrentURIMetaData": _didl_metadata(media_url, filename),
    })
    await _av_transport_action(selected, "Play", {"InstanceID": "0", "Speed": "1"})
    return {"ok": True, "label": selected["label"]}


def _format_duration(seconds: int) -> str:
    seconds = max(0, int(seconds))
    hours, seconds = divmod(seconds, 3600)
    minutes, seconds = divmod(seconds, 60)
    return f"{hours:02d}:{minutes:02d}:{seconds:02d}"


def _parse_duration(value: str) -> int:
    text = str(value or "").strip()
    if not text or text.upper() in {"NOT_IMPLEMENTED", "NOT IMPLEMENTED"}:
        raise ValueError("投屏设备没有返回当前播放进度")
    if "." in text:
        text = text.split(".", 1)[0]
    parts = text.split(":")
    if len(parts) == 2:
        parts = ["0", *parts]
    if len(parts) != 3:
        raise ValueError("投屏设备没有返回当前播放进度")
    hours, minutes, seconds = (int(part) for part in parts)
    if hours < 0 or not 0 <= minutes < 60 or not 0 <= seconds < 60:
        raise ValueError("投屏设备返回了无效的播放进度")
    return hours * 3600 + minutes * 60 + seconds


def _xml_text(body: str, local_name: str) -> str:
    text = str(body or "").strip()
    if not text:
        return ""
    try:
        root = ET.fromstring(text)
    except ET.ParseError:
        return ""
    for element in root.iter():
        if element.tag.rsplit("}", 1)[-1] == local_name and element.text:
            return element.text
    return ""


def parse_position_info(body: str) -> tuple[int, int]:
    """Return (position, duration) seconds from a GetPositionInfo SOAP body."""
    rel = _xml_text(body, "RelTime")
    duration = _xml_text(body, "TrackDuration")
    position = 0
    total = 0
    try:
        if rel:
            position = _parse_duration(rel)
    except ValueError:
        position = 0
    try:
        if duration:
            total = _parse_duration(duration)
    except ValueError:
        total = 0
    return position, total


def rel_time_available(body: str) -> bool:
    rel = _xml_text(body, "RelTime")
    if not rel:
        return False
    try:
        _parse_duration(rel)
        return True
    except ValueError:
        return False


def parse_transport_state(body: str) -> str:
    return (_xml_text(body, "CurrentTransportState") or "").upper()


async def _current_position(device: dict) -> int:
    body = await _av_transport_action(device, "GetPositionInfo", {"InstanceID": "0"})
    try:
        rel = _xml_text(body, "RelTime")
        if not rel:
            raise ValueError("missing RelTime")
        return _parse_duration(rel)
    except (ET.ParseError, ValueError) as exc:
        raise RuntimeError("投屏设备没有返回可用的播放进度") from exc


async def _dlna_status(device: dict) -> dict[str, str | bool | int]:
    position_result, transport_result = await asyncio.gather(
        _av_transport_action(device, "GetPositionInfo", {"InstanceID": "0"}),
        _av_transport_action(device, "GetTransportInfo", {"InstanceID": "0"}),
        return_exceptions=True,
    )
    position, duration = 0, 0
    position_ok = not isinstance(position_result, BaseException)
    if position_ok:
        body = str(position_result or "")
        try:
            position, duration = parse_position_info(body)
            position_ok = rel_time_available(body)
            if not position_ok:
                position = 0
        except Exception:
            position_ok = False
            position, duration = 0, 0
    state = ""
    transport_ok = not isinstance(transport_result, BaseException)
    if transport_ok:
        state = parse_transport_state(str(transport_result or ""))
    return {
        "ok": position_ok or transport_ok,
        "position_ok": position_ok,
        "transport_ok": transport_ok,
        "label": device["label"],
        "playing": state in {"PLAYING", "TRANSITIONING"} if transport_ok else False,
        "paused": state == "PAUSED_PLAYBACK" if transport_ok else False,
        "position": position,
        "duration": duration,
        "state": state or "UNKNOWN",
    }


def _control_chromecast(device: dict, action: str, seconds: int) -> dict[str, str | bool | int]:
    def control(chromecast, selected):
        controller = chromecast.media_controller
        if action == "play":
            controller.play()
        elif action == "pause":
            controller.pause()
        elif action == "stop":
            controller.stop()
        elif action == "seek":
            controller.update_status()
            current = float(getattr(controller.status, "current_time", 0) or 0)
            controller.seek(max(0, current + seconds))
        elif action == "seek_to":
            controller.seek(max(0, float(seconds)))
        elif action != "status":
            raise ValueError("不支持的投屏控制操作")
        if action != "stop":
            controller.update_status()
        status = controller.status
        state = str(getattr(status, "player_state", "") or "").upper()
        if action == "stop":
            state = "STOPPED"
        return {
            "ok": True,
            "position_ok": True,
            "transport_ok": True,
            "label": selected["label"],
            "playing": state in {"PLAYING", "BUFFERING"},
            "paused": state == "PAUSED",
            "position": int(float(getattr(status, "current_time", 0) or 0)),
            "duration": int(float(getattr(status, "duration", 0) or 0)),
            "state": state or "UNKNOWN",
        }

    return _with_chromecast(device, control)


async def cast_control(device: dict, action: str, seconds: int = 0) -> dict[str, str | bool | int]:
    selected = normalize_cast_device(device)
    if selected["protocol"] == "chromecast":
        try:
            result = await asyncio.to_thread(_control_chromecast, selected, action, seconds)
        except Exception as exc:
            raise RuntimeError(f"Chromecast 控制失败：{exc}") from exc
        if action == "stop":
            await asyncio.to_thread(close_chromecast_session, selected)
        return result
    if action == "play":
        await _av_transport_action(selected, "Play", {"InstanceID": "0", "Speed": "1"})
    elif action == "pause":
        await _av_transport_action(selected, "Pause", {"InstanceID": "0"})
    elif action == "stop":
        await _av_transport_action(selected, "Stop", {"InstanceID": "0"})
    elif action in {"seek", "seek_to"}:
        position = seconds if action == "seek_to" else await _current_position(selected) + seconds
        await _av_transport_action(selected, "Seek", {
            "InstanceID": "0",
            "Unit": "REL_TIME",
            "Target": _format_duration(max(0, int(position))),
        })
    elif action != "status":
        raise ValueError("不支持的投屏控制操作")
    status = await _dlna_status(selected)
    status["label"] = selected["label"]
    return status
