from __future__ import annotations

import asyncio
import contextlib
import os
import shlex
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Callable

from .config import settings
from .models import Task, TaskStatus


SCAN_TIMEOUT_SECONDS = 180.0
MAX_COMMAND_LENGTH = 2048


@dataclass(frozen=True)
class AvScanResult:
    state: str
    engine: str
    detail: str
    exit_code: int = 0

    def public(self) -> dict:
        return {
            "state": self.state,
            "engine": self.engine,
            "detail": self.detail[:300],
            "exit_code": int(self.exit_code),
        }


def skipped(detail: str, engine: str = "none") -> AvScanResult:
    return AvScanResult(state="skipped", engine=engine, detail=detail)


def discover_defender_command() -> list[str]:
    candidates: list[Path] = []
    for root in (os.environ.get("ProgramFiles", ""), os.environ.get("ProgramFiles(x86)", "")):
        if root:
            candidates.append(Path(root) / "Windows Defender" / "MpCmdRun.exe")
    platform = Path(os.environ.get("ProgramData", r"C:\\ProgramData")) / "Microsoft" / "Windows Defender" / "Platform"
    if platform.is_dir():
        try:
            children = sorted(platform.iterdir(), key=lambda item: item.name, reverse=True)
        except OSError:
            children = []
        for item in children:
            candidates.append(item / "MpCmdRun.exe")
    for path in candidates:
        try:
            if path.is_file():
                return [str(path), "-Scan", "-ScanType", "3", "-DisableRemediation", "-File"]
        except OSError:
            continue
    return []



def _unwrap_token(value: str) -> str:
    token = str(value or "")
    if len(token) >= 2 and token[0] == token[-1] and token[0] in {chr(34), chr(39)}:
        return token[1:-1]
    return token

def build_custom_command(template: str, file_path: str) -> list[str]:
    raw = str(template or "").strip()
    if not raw:
        return []
    if len(raw) > MAX_COMMAND_LENGTH or "{file}" not in raw:
        raise ValueError("自定义扫描命令必须包含 {file}")
    if any(ord(character) < 32 for character in raw):
        raise ValueError("自定义扫描命令含有无效字符")
    parts = shlex.split(raw, posix=os.name != "nt")
    if not parts:
        raise ValueError("自定义扫描命令为空")
    return [part.replace("{file}", file_path).strip('"') for part in parts]


def interpret_scan_exit(engine: str, exit_code: int, output: str) -> AvScanResult:
    text = str(output or "").strip().replace("\r", " ").replace("\n", " ")
    if engine == "defender":
        if exit_code == 0:
            return AvScanResult("clean", engine, "Windows Defender 未发现威胁", 0)
        if exit_code == 2:
            return AvScanResult("threat", engine, text or "Windows Defender 报告发现威胁", 2)
        return AvScanResult("error", engine, text or f"Windows Defender exit {exit_code}", exit_code)
    if exit_code == 0:
        return AvScanResult("clean", engine, "扫描器未发现威胁", 0)
    if exit_code == 1:
        return AvScanResult("threat", engine, text or "扫描器报告发现威胁", 1)
    return AvScanResult("error", engine, text or f"scanner exit {exit_code}", exit_code)


def resolve_scan_command(file_path: str, *, command_template: str = "", defender_factory=discover_defender_command) -> tuple[str, list[str]]:
    custom = str(command_template or "").strip()
    if custom:
        return "custom", build_custom_command(custom, file_path)
    defender = list(defender_factory() or [])
    if defender:
        return "defender", defender + [file_path]
    return "none", []


async def run_scan_command(argv: list[str], *, timeout: float = SCAN_TIMEOUT_SECONDS) -> tuple[int, str]:
    process = await asyncio.create_subprocess_exec(
        *argv,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    try:
        stdout, _stderr = await asyncio.wait_for(process.communicate(), timeout=timeout)
    except asyncio.TimeoutError:
        process.kill()
        with contextlib.suppress(Exception):
            await process.communicate()
        raise TimeoutError(f"scan timed out after {int(timeout)}s")
    output = (stdout or b"").decode("utf-8", errors="replace")
    return int(process.returncode or 0), output


def scan_target_path(task: Task) -> Path | None:
    raw = str(task.output_path or "").strip()
    if not raw:
        return None
    path = Path(raw)
    try:
        if path.is_file() and path.stat().st_size > 0:
            return path
    except OSError:
        return None
    return None


async def apply_post_download_scan(
    task: Task,
    *,
    on_progress: Callable | None = None,
    on_log: Callable | None = None,
    runner=run_scan_command,
    defender_factory=discover_defender_command,
) -> AvScanResult:
    if not bool(getattr(settings, "av_scan_enabled", False)):
        result = skipped("病毒扫描未开启")
        task.engine_state["av_scan"] = result.public()
        return result
    if task.status is not TaskStatus.DONE:
        result = skipped("任务尚未完成")
        task.engine_state["av_scan"] = result.public()
        return result
    path = scan_target_path(task)
    if path is None:
        result = skipped("完成结果不是单个本地文件")
        task.engine_state["av_scan"] = result.public()
        return result
    try:
        engine, argv = resolve_scan_command(
            str(path),
            command_template=str(getattr(settings, "av_scan_command", "") or ""),
            defender_factory=defender_factory,
        )
    except ValueError as exc:
        result = AvScanResult("error", "custom", str(exc), 0)
        task.engine_state["av_scan"] = result.public()
        if on_log:
            on_log(task.id, f"[scanning] {result.detail}")
        return result
    if not argv:
        result = skipped("未配置可用的病毒扫描器")
        task.engine_state["av_scan"] = result.public()
        if on_log:
            on_log(task.id, "[scanning] 未配置可用的病毒扫描器; download kept")
        return result
    task.status = TaskStatus.CHECKING
    task.stage = "scanning"
    task.last_log = "下载完成，正在扫描文件"
    task.engine_state["av_scan"] = {"state": "running", "engine": engine, "detail": "", "exit_code": 0}
    if on_progress:
        on_progress(task)
    if on_log:
        on_log(task.id, f"[scanning] using {engine}")
    try:
        exit_code, output = await runner(argv)
        result = interpret_scan_exit(engine, exit_code, output)
    except TimeoutError as exc:
        result = AvScanResult("error", engine, str(exc), 0)
    except Exception as exc:
        result = AvScanResult("error", engine, f"{type(exc).__name__}: {exc}", 0)
    task.engine_state["av_scan"] = result.public()
    if result.state == "threat" and bool(getattr(settings, "av_scan_fail_on_threat", True)):
        task.status = TaskStatus.FAILED
        task.error_code = "AV_THREAT"
        task.error_stage = "scanning"
        task.error_message = result.detail or "病毒扫描报告发现威胁"
        task.error_hint = "文件仍保留在磁盘上；若不信任请手动删除"
        task.finished_at = datetime.now().isoformat()
        task.last_log = task.error_message
        task.stage = "failed"
    else:
        task.status = TaskStatus.DONE
        task.stage = "done"
        if result.state == "clean":
            task.last_log = f"完成并已扫描: {path.name}"
        elif result.state == "error":
            task.last_log = f"已完成；扫描未完成: {result.detail}"
        else:
            task.last_log = f"完成: {path.name}"
    if on_progress:
        on_progress(task)
    if on_log:
        on_log(task.id, f"[scanning] {result.state}: {result.detail}")
    return result
