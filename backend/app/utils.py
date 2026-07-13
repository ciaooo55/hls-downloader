import re
from urllib.parse import urljoin, urlparse
from pathlib import PurePosixPath

def sanitize_filename(name: str) -> str:
    name = re.sub(r'[<>:"/\\|?*]', '_', name)
    name = re.sub(r'\s+', ' ', name).strip()
    name = name.strip('. ')
    if not name:
        name = "download"
    return name[:200]

def resolve_url(base: str, ref: str) -> str:
    return urljoin(base, ref)

def get_domain(url: str) -> str:
    parsed = urlparse(url)
    return parsed.hostname or ""

def safe_path(base_dir: str, filename: str) -> str:
    from pathlib import Path
    p = Path(base_dir) / sanitize_filename(filename)
    return str(p.resolve())

def humanize_bytes(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if abs(n) < 1024:
            return f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} PB"

def humanize_duration(seconds: float) -> str:
    if seconds < 0 or seconds > 360000:
        return "--:--:--"
    h = int(seconds // 3600)
    m = int((seconds % 3600) // 60)
    s = int(seconds % 60)
    return f"{h:02d}:{m:02d}:{s:02d}"
