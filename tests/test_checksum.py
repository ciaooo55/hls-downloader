import hashlib
import asyncio

import pytest

from backend.app.checksum import apply_http_content_checksum, normalize_checksum, parse_http_content_checksum, prefer_http_content_checksum, verify_checksum, verify_task_checksum
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


def test_http_headers_supply_checksum_without_overriding_user_value():
    payload = b"download payload"
    md5 = __import__("hashlib").md5(payload).digest()
    sha = __import__("hashlib").sha256(payload).digest()
    import base64
    headers = {
        "Content-MD5": base64.b64encode(md5).decode("ascii"),
        "Digest": "SHA-256=" + base64.b64encode(sha).decode("ascii"),
    }
    assert parse_http_content_checksum(headers).startswith("sha256:")
    assert parse_http_content_checksum({}) == ""
    assert parse_http_content_checksum({"etag": '"abc"'}) == ""
    assert prefer_http_content_checksum("md5:" + "a" * 32, "sha256:" + "b" * 64).startswith("sha256:")

    task = Task(id="hdr", url="https://example.test/file")
    assert apply_http_content_checksum(task, headers).startswith("sha256:")
    assert task.engine_state["checksum_from"] == "http_header"
    kept = Task(id="user", url="https://example.test/file", expected_checksum="md5:" + "0" * 32)
    assert apply_http_content_checksum(kept, headers) == ""
    assert kept.expected_checksum == "md5:" + "0" * 32


def test_goog_hash_and_hex_content_md5_are_accepted():
    import base64
    import hashlib
    digest = hashlib.md5(b"payload").digest()
    assert parse_http_content_checksum({"x-goog-hash": "crc32c=ignore,md5=" + base64.b64encode(digest).decode("ascii")}) == "md5:" + digest.hex()
    assert parse_http_content_checksum({"content-md5": digest.hex()}) == "md5:" + digest.hex()
    assert parse_http_content_checksum({"content-md5": "%%%not-a-digest%%%"}) == ""

