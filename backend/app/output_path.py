from __future__ import annotations

from pathlib import Path
from typing import Literal

ExistingFilePolicy = Literal['rename', 'overwrite', 'skip']


class ExistingFileError(RuntimeError):
    """Raised when skip policy refuses to replace a populated output file."""


def normalize_existing_file_policy(value: object) -> ExistingFilePolicy:
    raw = str(value or 'rename').strip().lower()
    if raw in {'rename', 'overwrite', 'skip'}:
        return raw  # type: ignore[return-value]
    return 'rename'


def _policy() -> ExistingFilePolicy:
    try:
        from .config import settings
        return normalize_existing_file_policy(getattr(settings, 'existing_file_policy', 'rename'))
    except Exception:
        return 'rename'


def reserve_output_path(path: Path, policy: object | None = None) -> Path:
    """Claim a final output file. Default policy is IDM-style auto-rename."""
    dest = Path(path)
    dest.parent.mkdir(parents=True, exist_ok=True)
    chosen = normalize_existing_file_policy(policy) if policy is not None else _policy()
    if dest.exists():
        size = dest.stat().st_size
        if chosen == 'skip' and size > 0:
            raise ExistingFileError(f'target already exists: {dest.name}')
        if chosen == 'overwrite' or (chosen == 'skip' and size == 0):
            dest.unlink()
            dest.open('xb').close()
            return dest
    for index in range(10000):
        candidate = dest if index == 0 else dest.with_name(f'{dest.stem}_{index}{dest.suffix}')
        try:
            candidate.open('xb').close()
            return candidate
        except FileExistsError:
            continue
    raise RuntimeError(f'cannot allocate unique output name: {dest.name}')


def choose_output_path(path: Path, policy: object | None = None) -> Path:
    """Pick a publish path without creating it. Used by BitTorrent."""
    dest = Path(path)
    dest.parent.mkdir(parents=True, exist_ok=True)
    chosen = normalize_existing_file_policy(policy) if policy is not None else _policy()
    if not dest.exists():
        return dest
    if chosen == 'skip':
        raise ExistingFileError(f'target already exists: {dest.name}')
    if chosen == 'overwrite' and dest.is_file():
        dest.unlink()
        return dest
    for index in range(1, 10000):
        candidate = dest.with_name(f'{dest.stem}_{index}{dest.suffix}')
        if not candidate.exists():
            return candidate
    raise RuntimeError(f'cannot allocate unique output name: {dest.name}')

