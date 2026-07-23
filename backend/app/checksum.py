import hashlib
import asyncio
from datetime import datetime
from pathlib import Path

from .models import Task, TaskStatus


SUPPORTED_ALGORITHMS = {"md5", "sha1", "sha256"}


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
