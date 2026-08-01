from __future__ import annotations

import asyncio
import html
import ipaddress
import mimetypes
import selectors
import socket
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
    try:
        import pychromecast
        from zeroconf import InterfaceChoice, Zeroconf

        addresses = _private_lan_addresses()
        zeroconf = Zeroconf(interfaces=addresses or InterfaceChoice.Default)
        chromecasts, browser = pychromecast.get_chromecasts(
            timeout=timeout, tries=1, zeroconf_instance=zeroconf,
        )
    except Exception:
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
        browser.stop_discovery()
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


def _didl_metadata(media_url: str, filename: str) -> str:
    mime_type, _ = mimetypes.guess_type(filename)
    protocol = f"http-get:*:{mime_type or 'application/octet-stream'}:*"
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


def _with_chromecast(device: dict, operation):
    import pychromecast

    selected = normalize_cast_device(device)
    if not private_ipv4_addresses(selected["host"]):
        raise ValueError("Chromecast 设备不是可访问的 IPv4 局域网设备")
    chromecasts, browser = pychromecast.get_chromecasts(
        known_hosts=[selected["host"]], timeout=8, tries=1,
    )
    try:
        chromecast = next((item for item in chromecasts if str(item.uuid) == selected["id"]), None)
        if chromecast is None:
            raise RuntimeError("找不到已选择的 Chromecast 设备，请重新扫描")
        chromecast.wait(timeout=8)
        return operation(chromecast, selected)
    finally:
        for chromecast in chromecasts:
            chromecast.disconnect()
        browser.stop_discovery()


def _cast_chromecast_media(device: dict, media_url: str, filename: str) -> None:
    mime_type, _ = mimetypes.guess_type(filename)

    def play(chromecast, _selected):
        controller = chromecast.media_controller
        controller.play_media(media_url, mime_type or "application/octet-stream", title=filename, stream_type="BUFFERED")
        controller.block_until_active(10)

    _with_chromecast(device, play)


def _control_chromecast(device: dict, action: str, seconds: int) -> None:
    def control(chromecast, _selected):
        controller = chromecast.media_controller
        if action == "play":
            controller.play()
        elif action == "pause":
            controller.pause()
        elif action == "seek":
            controller.update_status()
            controller.seek(max(0, float(controller.status.current_time or 0) + seconds))
        else:
            raise ValueError("不支持的投屏控制操作")

    _with_chromecast(device, control)


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
    parts = value.strip().split(":")
    if len(parts) != 3:
        raise ValueError("投屏设备没有返回当前播放进度")
    hours, minutes, seconds = (int(part) for part in parts)
    if hours < 0 or not 0 <= minutes < 60 or not 0 <= seconds < 60:
        raise ValueError("投屏设备返回了无效的播放进度")
    return hours * 3600 + minutes * 60 + seconds


async def _current_position(device: dict) -> int:
    body = await _av_transport_action(device, "GetPositionInfo", {"InstanceID": "0"})
    try:
        root = ET.fromstring(body)
        for element in root.iter():
            if element.tag.rsplit("}", 1)[-1] == "RelTime" and element.text:
                return _parse_duration(element.text)
    except ET.ParseError as exc:
        raise RuntimeError("投屏设备没有返回可用的播放进度") from exc
    raise RuntimeError("投屏设备不支持读取当前播放进度，无法快进")


async def cast_control(device: dict, action: str, seconds: int = 0) -> dict[str, str | bool]:
    selected = normalize_cast_device(device)
    if selected["protocol"] == "chromecast":
        try:
            await asyncio.to_thread(_control_chromecast, selected, action, seconds)
        except Exception as exc:
            raise RuntimeError(f"Chromecast 控制失败：{exc}") from exc
        return {"ok": True, "label": selected["label"]}
    if action == "play":
        await _av_transport_action(selected, "Play", {"InstanceID": "0", "Speed": "1"})
    elif action == "pause":
        await _av_transport_action(selected, "Pause", {"InstanceID": "0"})
    elif action == "seek":
        position = await _current_position(selected)
        await _av_transport_action(selected, "Seek", {
            "InstanceID": "0",
            "Unit": "REL_TIME",
            "Target": _format_duration(position + seconds),
        })
    else:
        raise ValueError("不支持的投屏控制操作")
    return {"ok": True, "label": selected["label"]}
