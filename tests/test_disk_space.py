import errno
import shutil

import pytest

from backend.app.downloader.disk_space import (
    MIN_FREE_RESERVE,
    ensure_download_capacity,
    ensure_free_space,
)


def test_free_space_preflight_raises_enospc(tmp_path, monkeypatch):
    monkeypatch.setattr(
        shutil,
        "disk_usage",
        lambda _path: shutil._ntuple_diskusage(total=100, used=90, free=10),
    )
    with pytest.raises(OSError) as captured:
        ensure_free_space(tmp_path, 20, operation="测试")
    assert captured.value.errno == errno.ENOSPC


def test_known_download_accounts_for_existing_partial_file(tmp_path, monkeypatch):
    part = tmp_path / "part.bin"
    part.write_bytes(b"x" * 80)
    output = tmp_path / "final.bin"
    required: list[int] = []

    monkeypatch.setattr(
        "backend.app.downloader.disk_space.ensure_free_space",
        lambda _path, value, **_kwargs: required.append(value),
    )
    ensure_download_capacity(part, output, 100, current_size=part.stat().st_size)

    assert required == [20 + MIN_FREE_RESERVE]
