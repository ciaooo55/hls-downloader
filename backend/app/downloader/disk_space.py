from __future__ import annotations

import errno
import os
import shutil
from pathlib import Path


MIN_FREE_RESERVE = 64 * 1024 * 1024


def _existing_ancestor(path: Path) -> Path:
    current = Path(path).expanduser().resolve()
    while not current.exists() and current.parent != current:
        current = current.parent
    return current


def _same_filesystem(left: Path, right: Path) -> bool:
    left_root = _existing_ancestor(left)
    right_root = _existing_ancestor(right)
    try:
        return os.stat(left_root).st_dev == os.stat(right_root).st_dev
    except OSError:
        left_drive = os.path.splitdrive(str(left_root))[0].casefold()
        right_drive = os.path.splitdrive(str(right_root))[0].casefold()
        return bool(left_drive and left_drive == right_drive)


def ensure_free_space(path: Path, required_bytes: int, *, operation: str) -> None:
    root = _existing_ancestor(path)
    required = max(0, int(required_bytes))
    free = int(shutil.disk_usage(root).free)
    if free < required:
        missing = required - free
        raise OSError(
            errno.ENOSPC,
            f"{operation}磁盘空间不足，还需要约 {missing / 1048576:.1f} MiB",
            str(root),
        )


def ensure_download_capacity(
    temp_path: Path,
    output_path: Path,
    expected_size: int,
    *,
    current_size: int = 0,
) -> None:
    """Check temporary and final volumes before starting a known-size file."""
    remaining = max(0, int(expected_size) - max(0, int(current_size)))
    ensure_free_space(
        temp_path,
        remaining + MIN_FREE_RESERVE,
        operation="下载临时盘",
    )
    if not _same_filesystem(temp_path, output_path):
        ensure_free_space(
            output_path,
            max(0, int(expected_size)) + MIN_FREE_RESERVE,
            operation="最终保存盘",
        )


def estimate_paths_size(paths) -> int:
    total = 0
    seen: set[Path] = set()
    for value in paths:
        path = Path(value)
        if path in seen:
            continue
        seen.add(path)
        try:
            if path.is_file():
                total += max(0, int(path.stat().st_size))
        except OSError:
            continue
    return total


def preallocate_payload(path: Path, size: int) -> None:
    """Create one Range payload with the final logical size.

    HTTP workers seek and write into this file. They never concatenate part
    files. On NTFS the file is marked sparse so a 4 GiB download does not
    physically zero-fill 4 GiB at start (IDM-style). Other volumes fall back
    to a normal truncate.
    """
    size = max(0, int(size))
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("wb") as stream:
        if os.name == "nt":
            _mark_sparse_windows(stream.fileno())
        stream.truncate(size)


def _mark_sparse_windows(fd: int) -> None:
    try:
        import ctypes
        import msvcrt
        from ctypes import wintypes
    except ImportError:
        return
    try:
        handle = msvcrt.get_osfhandle(fd)
        returned = wintypes.DWORD(0)
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        FSCTL_SET_SPARSE = 0x000900C4
        kernel32.DeviceIoControl.argtypes = [
            wintypes.HANDLE,
            wintypes.DWORD,
            wintypes.LPVOID,
            wintypes.DWORD,
            wintypes.LPVOID,
            wintypes.DWORD,
            ctypes.POINTER(wintypes.DWORD),
            wintypes.LPVOID,
        ]
        kernel32.DeviceIoControl.restype = wintypes.BOOL
        kernel32.DeviceIoControl(
            handle,
            FSCTL_SET_SPARSE,
            None,
            0,
            None,
            0,
            ctypes.byref(returned),
            None,
        )
    except OSError:
        return
