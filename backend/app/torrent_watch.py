"""Optional watch folder for newly dropped .torrent files."""

from __future__ import annotations

from pathlib import Path

MAX_WATCH_TORRENT_BYTES = 16 * 1024 * 1024
MAX_IMPORTS_PER_SCAN = 20


def torrent_fingerprint(path: Path) -> str:
    stat = path.stat()
    return f"{path.resolve()}|{stat.st_size}|{getattr(stat, "st_mtime_ns", int(stat.st_mtime * 1_000_000_000))}"


class TorrentWatchState:
    def __init__(self) -> None:
        self.seen: set[str] = set()
        self.primed_dir = ""

    def disable(self) -> None:
        self.seen.clear()
        self.primed_dir = ""


watch_state = TorrentWatchState()


def collect_new_torrents(directory: str, state: TorrentWatchState | None = None) -> list[Path]:
    """Return torrents that appeared after the folder was first primed.

    The first successful scan of a directory only records fingerprints. That
    avoids importing a folder full of old seeds when the user enables watching.
    """
    current = state or watch_state
    raw = str(directory or "").strip()
    if not raw:
        current.disable()
        return []
    root = Path(raw).expanduser()
    try:
        root = root.resolve()
    except OSError:
        return []
    if not root.is_dir():
        return []
    key = str(root)
    if current.primed_dir != key:
        current.seen.clear()
        current.primed_dir = key
        prime = True
    else:
        prime = False
    discovered: list[tuple[str, Path]] = []
    try:
        children = list(root.iterdir())
    except OSError:
        return []
    for item in children:
        try:
            if not item.is_file() or item.suffix.lower() not in {".torrent", ".url", ".magnet"}:
                continue
            mark = torrent_fingerprint(item)
        except OSError:
            continue
        discovered.append((mark, item))
    if prime:
        current.seen.update(mark for mark, _path in discovered)
        return []
    fresh: list[Path] = []
    for mark, item in discovered:
        if mark in current.seen:
            continue
        current.seen.add(mark)
        fresh.append(item)
        if len(fresh) >= MAX_IMPORTS_PER_SCAN:
            break
    return fresh


def read_watch_torrent(path: Path) -> bytes:
    if path.suffix.lower() != ".torrent" or not path.is_file():
        raise ValueError("不是有效的 .torrent 文件")
    size = path.stat().st_size
    if size <= 0 or size > MAX_WATCH_TORRENT_BYTES:
        raise ValueError("种子文件为空或超过 16 MiB")
    data = path.read_bytes()
    if not data or len(data) > MAX_WATCH_TORRENT_BYTES:
        raise ValueError("种子文件为空或超过 16 MiB")
    return data
