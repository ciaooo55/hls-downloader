from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import unquote, urlparse
from xml.etree import ElementTree

from .checksum import normalize_checksum
from .downloader.mirrors import normalize_mirror_urls
from .link_file import LinkFileError, MAX_LINK_FILE_BYTES, MAX_LINK_URLS, decode_link_bytes
from .utils import sanitize_filename

METALINK_SUFFIXES = {".metalink", ".meta4"}
MAX_METALINK_FILES = MAX_LINK_URLS
MAX_URLS_PER_FILE = 16
_HASH_ALIASES = {
    "sha-256": "sha256",
    "sha256": "sha256",
    "sha-1": "sha1",
    "sha1": "sha1",
    "md5": "md5",
}


@dataclass(frozen=True)
class MetalinkFile:
    name: str
    url: str
    mirrors: list[str] = field(default_factory=list)
    checksum: str = ""
    size: int = 0


def _local(tag: str) -> str:
    if "}" in tag:
        return tag.rsplit("}", 1)[-1].lower()
    return tag.lower()


def _text(node: ElementTree.Element | None) -> str:
    if node is None or node.text is None:
        return ""
    return str(node.text).strip()


def _safe_http_ftp_magnet(raw: str) -> str:
    value = unquote(str(raw or "").strip())
    if not value or len(value) > 8192:
        return ""
    parsed = urlparse(value)
    scheme = (parsed.scheme or "").lower()
    if scheme == "magnet" and parsed.query:
        return value
    if scheme in {"http", "https", "ftp", "ftps", "sftp"} and parsed.hostname:
        lowered = value.lower()
        if lowered.startswith(("javascript:", "data:", "blob:", "file:")):
            return ""
        return value
    return ""


def _checksum_from_hashes(items: list[tuple[str, str]]) -> str:
    preferred = {"sha256": 0, "sha1": 1, "md5": 2}
    ranked: list[tuple[int, str]] = []
    for raw_type, digest in items:
        algorithm = _HASH_ALIASES.get(str(raw_type or "").strip().lower())
        if not algorithm or not digest:
            continue
        try:
            normalized_algo, normalized_digest = normalize_checksum(f"{algorithm}:{digest}")
        except ValueError:
            continue
        ranked.append((preferred[normalized_algo], f"{normalized_algo}:{normalized_digest}"))
    ranked.sort()
    return ranked[0][1] if ranked else ""


def _pick_urls(entries: list[tuple[int, str]]) -> tuple[str, list[str]]:
    ordered: list[str] = []
    seen: set[str] = set()
    for _, url in sorted(entries, key=lambda item: item[0]):
        key = url.lower()
        if key in seen:
            continue
        seen.add(key)
        ordered.append(url)
        if len(ordered) >= MAX_URLS_PER_FILE:
            break
    if not ordered:
        return "", []
    http = [item for item in ordered if item.lower().startswith(("http://", "https://"))]
    ftp = [item for item in ordered if item.lower().startswith(("ftp://", "ftps://"))]
    sftp = [item for item in ordered if item.lower().startswith("sftp://")]
    magnets = [item for item in ordered if item.lower().startswith("magnet:")]
    if http:
        primary = http[0]
        mirrors = normalize_mirror_urls(primary, http[1:])
        return primary, mirrors
    if ftp:
        return ftp[0], []
    if sftp:
        return sftp[0], []
    return magnets[0], []


def _parse_metalink4_file(node: ElementTree.Element) -> MetalinkFile | None:
    name = sanitize_filename(node.attrib.get("name") or Path(_text(next((child for child in node if _local(child.tag) == "name"), None))).name or "download")
    size_text = _text(next((child for child in node if _local(child.tag) == "size"), None))
    try:
        size = max(0, int(size_text)) if size_text else 0
    except ValueError:
        size = 0
    hashes = [
        (child.attrib.get("type") or "", _text(child))
        for child in node
        if _local(child.tag) == "hash"
    ]
    urls: list[tuple[int, str]] = []
    for child in node:
        if _local(child.tag) != "url":
            continue
        url = _safe_http_ftp_magnet(_text(child))
        if not url:
            continue
        try:
            priority = int(child.attrib.get("priority") or 100)
        except ValueError:
            priority = 100
        urls.append((priority, url))
    primary, mirrors = _pick_urls(urls)
    if not primary:
        return None
    return MetalinkFile(name=name, url=primary, mirrors=mirrors, checksum=_checksum_from_hashes(hashes), size=size)


def _parse_metalink3_file(node: ElementTree.Element) -> MetalinkFile | None:
    name = sanitize_filename(node.attrib.get("name") or _text(next((child for child in node if _local(child.tag) == "name"), None)) or "download")
    size_text = _text(next((child for child in node if _local(child.tag) == "size"), None))
    try:
        size = max(0, int(size_text)) if size_text else 0
    except ValueError:
        size = 0
    hashes: list[tuple[str, str]] = []
    for child in node.iter():
        if _local(child.tag) == "hash":
            hashes.append((child.attrib.get("type") or "", _text(child)))
    urls: list[tuple[int, str]] = []
    for child in node.iter():
        if _local(child.tag) != "url":
            continue
        url = _safe_http_ftp_magnet(_text(child))
        if not url:
            continue
        try:
            preference = int(child.attrib.get("preference") or 0)
        except ValueError:
            preference = 0
        urls.append((-preference, url))
    primary, mirrors = _pick_urls(urls)
    if not primary:
        return None
    return MetalinkFile(name=name, url=primary, mirrors=mirrors, checksum=_checksum_from_hashes(hashes), size=size)


def looks_like_metalink(text: str) -> bool:
    head = str(text or "")[:4000].lower()
    return "<metalink" in head and ("<file" in head or "<url" in head)


def parse_metalink(text: str) -> list[MetalinkFile]:
    body = str(text or "").strip()
    if not body:
        raise LinkFileError("metalink 文件是空的")
    if not looks_like_metalink(body):
        raise LinkFileError("不是有效的 metalink 文件")
    try:
        root = ElementTree.fromstring(body)
    except ElementTree.ParseError as exc:
        raise LinkFileError("metalink XML 无效") from exc
    files: list[MetalinkFile] = []
    root_name = _local(root.tag)
    nodes: list[ElementTree.Element]
    if root_name == "file":
        nodes = [root]
    else:
        nodes = [child for child in root.iter() if _local(child.tag) == "file"]
    metalink3 = "metalinker.org" in (root.tag or "") or any(
        "preference" in child.attrib for child in root.iter() if _local(child.tag) == "url"
    )
    for node in nodes:
        parsed = _parse_metalink3_file(node) if metalink3 else _parse_metalink4_file(node)
        if parsed is None:
            continue
        files.append(parsed)
        if len(files) >= MAX_METALINK_FILES:
            break
    if not files:
        raise LinkFileError("metalink 里没有可下载的远程地址")
    return files


def read_metalink_files(path: Path) -> list[MetalinkFile]:
    source = Path(path)
    if source.suffix.lower() not in METALINK_SUFFIXES:
        raise LinkFileError("不是 metalink 文件")
    if not source.is_file():
        raise LinkFileError("not a file")
    size = source.stat().st_size
    if size <= 0 or size > MAX_LINK_FILE_BYTES:
        raise LinkFileError("link file is empty or too large")
    data = source.read_bytes()
    if not data or len(data) > MAX_LINK_FILE_BYTES:
        raise LinkFileError("link file is empty or too large")
    return parse_metalink(decode_link_bytes(data))
