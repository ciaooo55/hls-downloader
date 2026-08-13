from pathlib import Path

import pytest

from backend.app.output_path import ExistingFileError, choose_output_path, reserve_output_path


def test_rename_policy_keeps_existing_file(tmp_path: Path):
    first = tmp_path / 'movie.mp4'
    first.write_bytes(b'old')
    reserved = reserve_output_path(first, 'rename')
    assert reserved.name == 'movie_1.mp4'
    assert first.read_bytes() == b'old'
    assert reserved.exists()


def test_overwrite_policy_replaces_existing_file(tmp_path: Path):
    first = tmp_path / 'movie.mp4'
    first.write_bytes(b'old')
    reserved = reserve_output_path(first, 'overwrite')
    assert reserved == first
    assert reserved.stat().st_size == 0


def test_skip_policy_refuses_populated_file(tmp_path: Path):
    first = tmp_path / 'movie.mp4'
    first.write_bytes(b'old')
    with pytest.raises(ExistingFileError):
        reserve_output_path(first, 'skip')
    assert first.read_bytes() == b'old'


def test_skip_policy_reclaims_empty_placeholder(tmp_path: Path):
    first = tmp_path / 'movie.mp4'
    first.write_bytes(b'')
    reserved = reserve_output_path(first, 'skip')
    assert reserved == first


def test_choose_output_path_matches_policy(tmp_path: Path):
    first = tmp_path / 'pack.zip'
    first.write_bytes(b'x')
    assert choose_output_path(first, 'rename').name == 'pack_1.zip'
    with pytest.raises(ExistingFileError):
        choose_output_path(first, 'skip')

