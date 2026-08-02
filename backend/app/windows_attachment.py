from __future__ import annotations

import os
from ipaddress import ip_address
from pathlib import Path
from urllib.parse import urlsplit

from .downloader.errors import redact_url


def is_public_download_url(value: str) -> bool:
    try:
        parsed = urlsplit(str(value or ""))
        host = (parsed.hostname or "").rstrip(".").lower()
    except ValueError:
        return False
    if parsed.scheme not in {"http", "https"} or not host:
        return False
    if host == "localhost" or host.endswith((".localhost", ".local")):
        return False
    try:
        address = ip_address(host)
    except ValueError:
        return True
    return not (
        address.is_private
        or address.is_loopback
        or address.is_link_local
        or address.is_multicast
        or address.is_reserved
        or address.is_unspecified
    )


def _write_zone_identifier(path: Path, source_url: str, referrer_url: str = "") -> None:
    lines = ["[ZoneTransfer]", "ZoneId=3"]
    source = redact_url(source_url)
    referrer = redact_url(referrer_url)
    if referrer:
        lines.append(f"ReferrerUrl={referrer}")
    if source:
        lines.append(f"HostUrl={source}")
    with open(f"{path}:Zone.Identifier", "w", encoding="utf-8", newline="\r\n") as stream:
        stream.write("\n".join(lines) + "\n")


def mark_download_from_internet(path_value: str, source_url: str, referrer_url: str = "") -> int:
    """Apply Windows Mark-of-the-Web without ever persisting signed query data."""

    if os.name != "nt" or not is_public_download_url(source_url):
        return 0
    root = Path(path_value)
    if not root.exists():
        return 0
    files = [root] if root.is_file() else [path for path in root.rglob("*") if path.is_file()]
    marked = 0
    for path in files:
        try:
            _write_zone_identifier(path, source_url, referrer_url)
            marked += 1
        except OSError:
            continue
    return marked
