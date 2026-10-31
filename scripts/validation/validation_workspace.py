"""Workspace location for disposable validation files."""

from pathlib import Path


def test_tmp() -> Path:
    path = Path(__file__).resolve().parents[2] / '.tool-cache' / 'test-tmp'
    path.mkdir(parents=True, exist_ok=True)
    return path
