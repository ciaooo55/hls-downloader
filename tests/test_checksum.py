import hashlib
import asyncio

import pytest

from backend.app.checksum import normalize_checksum, verify_checksum, verify_task_checksum
from backend.app.models import Task, TaskStatus


def test_checksum_accepts_prefixed_and_unprefixed_sha256(tmp_path):
    path = tmp_path / "file.bin"
    path.write_bytes(b"download payload")
    digest = hashlib.sha256(b"download payload").hexdigest()
    assert normalize_checksum(digest) == ("sha256", digest)
    assert verify_checksum(path, f"sha256:{digest}") == (True, "sha256", digest)


def test_checksum_reports_mismatch_without_hiding_actual_digest(tmp_path):
    path = tmp_path / "file.bin"
    path.write_bytes(b"download payload")
    ok, algorithm, actual = verify_checksum(path, "sha1:" + "0" * 40)
    assert not ok and algorithm == "sha1"
    assert actual == hashlib.sha1(b"download payload").hexdigest()


def test_finalization_keeps_mismatched_file_and_marks_task_failed(tmp_path):
    path = tmp_path / "completed.bin"
    path.write_bytes(b"download payload")
    task = Task(id="checksum-task", url="https://example.test/file", expected_checksum="sha256:" + "0" * 64)
    events = []

    verified = asyncio.run(verify_task_checksum(task, path, on_progress=events.append))

    assert not verified
    assert path.exists()
    assert task.status is TaskStatus.FAILED
    assert task.error_code == "CHECKSUM_MISMATCH"
    assert task.checksum_verified is False
    assert task.checksum_actual == hashlib.sha256(b"download payload").hexdigest()
    assert events


def test_finalization_accepts_matching_file(tmp_path):
    path = tmp_path / "completed.bin"
    path.write_bytes(b"download payload")
    digest = hashlib.md5(b"download payload").hexdigest()
    task = Task(id="checksum-task", url="https://example.test/file", expected_checksum=f"md5:{digest}")

    assert asyncio.run(verify_task_checksum(task, path))
    assert task.checksum_verified is True
    assert task.checksum_actual == digest


@pytest.mark.parametrize("value", ["", "sha256:bad", "sha512:" + "0" * 128, "not-a-digest"])
def test_checksum_rejects_ambiguous_or_unsupported_values(value):
    with pytest.raises(ValueError):
        normalize_checksum(value)
