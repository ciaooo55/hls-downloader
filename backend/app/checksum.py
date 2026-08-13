import asyncio
import base64
import binascii
import hashlib
from datetime import datetime
from pathlib import Path

from .models import Task, TaskStatus


SUPPORTED_ALGORITHMS = {"md5", "sha1", "sha256"}
_DIGEST_SIZES = {"md5": 16, "sha1": 20, "sha256": 32}
_HEADER_ALGORITHMS = (
    ("sha256", ("sha-256", "sha256")),
    ("sha1", ("sha-1", "sha1", "sha")),
    ("md5", ("md5",)),
)


def _hex_or_b64_digest(value: str, size: int) -> str:
    text = str(value or "").strip().strip('"').strip("'")
    if not text:
        return ""
    hexed = text.lower().replace(":", "").replace(" ", "")
    if len(hexed) == size * 2 and all(char in "0123456789abcdef" for char in hexed):
        return hexed
    try:
        pad = "=" * ((4 - len(text) % 4) % 4)
        raw = base64.b64decode(text + pad, validate=False)
        if len(raw) != size:
            raw = base64.urlsafe_b64decode(text + pad)
        if len(raw) == size:
            return raw.hex()
    except (ValueError, binascii.Error):
        return ""
    return ""


def _pairs_from_http_header(value: str) -> list[tuple[str, str]]:
    pairs: list[tuple[str, str]] = []
    for part in str(value or "").split(","):
        name, separator, raw = part.partition("=")
        if not separator:
            continue
        pairs.append((name.strip().lower(), raw.strip()))
    return pairs


def parse_http_content_checksum(headers) -> str:
    """Return algo:hex when a response advertises a whole-file digest.

    Missing or unusable headers return empty so ordinary downloads stay unchanged.
    """
    if headers is None:
        return ""
    lowered = {}
    try:
        items = headers.items()
    except Exception:
        items = []
    for name, value in items:
        lowered[str(name or "").lower()] = value

    def getter(name, default=""):
        if lowered:
            return lowered.get(str(name).lower(), default)
        if hasattr(headers, "get"):
            return headers.get(name, default)
        return default

    found: dict[str, str] = {}

    content_md5 = _hex_or_b64_digest(str(getter("content-md5", "") or ""), 16)
    if content_md5:
        found["md5"] = content_md5

    for name, raw in _pairs_from_http_header(str(getter("digest", "") or "")):
        for algorithm, aliases in _HEADER_ALGORITHMS:
            if name in aliases:
                digest = _hex_or_b64_digest(raw, _DIGEST_SIZES[algorithm])
                if digest:
                    found[algorithm] = digest
                break

    for name, raw in _pairs_from_http_header(str(getter("x-goog-hash", "") or "")):
        if name == "md5":
            digest = _hex_or_b64_digest(raw, 16)
            if digest:
                found["md5"] = digest

    for algorithm, size in (("md5", 16), ("sha1", 20), ("sha256", 32)):
        digest = _hex_or_b64_digest(str(getter(f"x-checksum-{algorithm}", "") or ""), size)
        if digest:
            found[algorithm] = digest

    for algorithm in ("sha256", "sha1", "md5"):
        if algorithm in found:
            return f"{algorithm}:{found[algorithm]}"
    return ""


def prefer_http_content_checksum(*values: str) -> str:
    rank = {"sha256": 3, "sha1": 2, "md5": 1}
    best = ""
    best_rank = 0
    for value in values:
        algorithm = str(value or "").split(":", 1)[0]
        score = rank.get(algorithm, 0)
        if score > best_rank:
            best = str(value)
            best_rank = score
    return best


def apply_http_content_checksum(task: Task, headers=None, checksum: str = "") -> str:
    """Adopt a server digest only when the task does not already have one."""
    if str(getattr(task, "expected_checksum", "") or "").strip():
        return ""
    checksum = str(checksum or parse_http_content_checksum(headers) or "").strip()
    if not checksum:
        return ""
    task.expected_checksum = checksum
    task.checksum_algorithm = checksum.split(":", 1)[0]
    engine = getattr(task, "engine_state", None)
    if isinstance(engine, dict):
        engine["checksum_from"] = "http_header"
    return checksum


def normalize_checksum(value: str) -> tuple[str, str]:
    raw = str(value or "").strip().lower().replace(" ", "")
    algorithm, separator, digest = raw.partition(":")
    if not separator:
        lengths = {32: "md5", 40: "sha1", 64: "sha256"}
        digest = raw
        algorithm = lengths.get(len(digest), "")
    if algorithm not in SUPPORTED_ALGORITHMS or not digest or any(char not in "0123456789abcdef" for char in digest):
        raise ValueError("校验和必须是 MD5、SHA-1 或 SHA-256 十六进制值")
    expected_length = {"md5": 32, "sha1": 40, "sha256": 64}[algorithm]
    if len(digest) != expected_length:
        raise ValueError(f"{algorithm.upper()} 校验和长度不正确")
    return algorithm, digest


def calculate_checksum(path: Path, algorithm: str) -> str:
    if algorithm not in SUPPORTED_ALGORITHMS:
        raise ValueError("不支持的校验和算法")
    digest = hashlib.new(algorithm)
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_checksum(path: Path, expected: str) -> tuple[bool, str, str]:
    algorithm, expected_digest = normalize_checksum(expected)
    actual = calculate_checksum(path, algorithm)
    return actual == expected_digest, algorithm, actual


async def verify_task_checksum(
    task: Task,
    path: Path,
    *,
    on_progress=None,
    on_log=None,
) -> bool:
    """Verify the final regular file before its downloader may mark it done.

    A mismatch intentionally keeps the completed output for manual inspection;
    only the task state changes to FAILED. Directory-shaped torrent payloads do
    not have a standard single-file digest and therefore fail explicitly rather
    than hashing an arbitrary member.
    """
    if not task.expected_checksum:
        return True

    publish = on_progress or (lambda _task: None)
    log = on_log or (lambda _task_id, _message: None)
    task.status = TaskStatus.CHECKING
    task.stage = "verifying_checksum"
    task.last_log = "正在校验最终文件"
    task.progress.connection_status = "checking"
    log(task.id, "[verifying_checksum] 正在校验最终文件")
    publish(task)

    if not path.is_file():
        task.checksum_verified = False
        task.error_code = "CHECKSUM_UNSUPPORTED_OUTPUT"
        task.error_stage = "verifying_checksum"
        task.error_url = ""
        task.error_hint = "校验和只能验证单个最终文件；多文件 BT 任务请在任务详情中逐个核对。"
        task.error_message = "校验和无法用于多文件输出"
    else:
        try:
            ok, algorithm, actual = await asyncio.to_thread(
                verify_checksum, path, task.expected_checksum
            )
            task.checksum_algorithm = algorithm
            task.checksum_actual = actual
            task.checksum_verified = ok
            if ok:
                task.progress.connection_status = "idle"
                task.last_log = f"{algorithm.upper()} 校验通过"
                log(task.id, f"[verifying_checksum] {algorithm.upper()} 校验通过")
                publish(task)
                return True
            task.error_code = "CHECKSUM_MISMATCH"
            task.error_stage = "verifying_checksum"
            task.error_url = ""
            task.error_hint = "文件已保留。请核对发布方提供的校验和，或删除任务文件后重新下载。"
            task.error_message = f"{algorithm.upper()} 校验不匹配：期望 {task.expected_checksum}，实际 {actual}"
        except (OSError, ValueError) as exc:
            task.checksum_verified = False
            task.error_code = "CHECKSUM_VERIFY_FAILED"
            task.error_stage = "verifying_checksum"
            task.error_url = ""
            task.error_hint = "请检查最终文件是否仍存在、可读取，然后重试任务。"
            task.error_message = f"无法校验最终文件：{exc}"

    task.status = TaskStatus.FAILED
    task.stage = "checksum_failed"
    task.last_log = task.error_message
    task.finished_at = datetime.now().isoformat()
    task.progress.connection_status = "error"
    log(task.id, f"[checksum_failed] {task.error_message}")
    publish(task)
    return False
