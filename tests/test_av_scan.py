import asyncio

import pytest

from backend.app.av_scan import (
    apply_post_download_scan,
    build_custom_command,
    interpret_scan_exit,
    resolve_scan_command,
)
from backend.app.config import settings
from backend.app.models import Task, TaskStatus


def test_disabled_scan_is_noop(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "av_scan_enabled", False)
    output = tmp_path / "a.bin"
    output.write_bytes(b"hello")
    task = Task(id="av-off", url="https://cdn.example.test/a.bin", status=TaskStatus.DONE, output_path=str(output))
    called = []

    async def runner(argv):
        called.append(argv)
        return 0, "ok"

    result = asyncio.run(apply_post_download_scan(task, runner=runner, defender_factory=lambda: ["MpCmdRun.exe"]))
    assert result.state == "skipped"
    assert called == []
    assert task.status is TaskStatus.DONE
    assert task.engine_state["av_scan"]["state"] == "skipped"


def test_clean_defender_scan_keeps_done(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "av_scan_enabled", True)
    monkeypatch.setattr(settings, "av_scan_command", "")
    output = tmp_path / "clean.bin"
    output.write_bytes(b"payload")
    task = Task(id="av-clean", url="https://cdn.example.test/a.bin", status=TaskStatus.DONE, output_path=str(output))

    async def runner(argv):
        assert argv[-1] == str(output)
        assert argv[-2] == "-File"
        return 0, "no threats"

    result = asyncio.run(apply_post_download_scan(task, runner=runner, defender_factory=lambda: ["MpCmdRun.exe", "-Scan", "-ScanType", "3", "-DisableRemediation", "-File"]))
    assert result.state == "clean"
    assert task.status is TaskStatus.DONE
    assert task.engine_state["av_scan"]["state"] == "clean"


def test_threat_fails_task_but_keeps_file(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "av_scan_enabled", True)
    monkeypatch.setattr(settings, "av_scan_fail_on_threat", True)
    monkeypatch.setattr(settings, "av_scan_command", "")
    output = tmp_path / "bad.bin"
    output.write_bytes(b"virus")
    task = Task(id="av-bad", url="https://cdn.example.test/a.bin", status=TaskStatus.DONE, output_path=str(output))

    async def runner(_argv):
        return 2, "Threat found"

    result = asyncio.run(apply_post_download_scan(task, runner=runner, defender_factory=lambda: ["MpCmdRun.exe", "-Scan", "-ScanType", "3", "-DisableRemediation", "-File"]))
    assert result.state == "threat"
    assert task.status is TaskStatus.FAILED
    assert task.error_code == "AV_THREAT"
    assert output.is_file()


def test_missing_scanner_keeps_successful_download(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "av_scan_enabled", True)
    monkeypatch.setattr(settings, "av_scan_command", "")
    output = tmp_path / "ok.bin"
    output.write_bytes(b"ok")
    task = Task(id="av-miss", url="https://cdn.example.test/a.bin", status=TaskStatus.DONE, output_path=str(output))
    result = asyncio.run(apply_post_download_scan(task, defender_factory=lambda: []))
    assert result.state == "skipped"
    assert task.status is TaskStatus.DONE


def test_custom_command_requires_file_placeholder():
    with pytest.raises(ValueError):
        build_custom_command("clamscan.exe --no-summary", "C:/a.bin")
    argv = build_custom_command(r"C:\Tools\clamscan.exe --no-summary {file}", r"D:\out\a.bin")
    assert argv[0].endswith("clamscan.exe")
    assert argv[-1].endswith("a.bin")


def test_custom_command_threat_and_error(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "av_scan_enabled", True)
    monkeypatch.setattr(settings, "av_scan_command", "clamscan.exe {file}")
    output = tmp_path / "x.bin"
    output.write_bytes(b"x")
    task = Task(id="av-custom", url="https://cdn.example.test/a.bin", status=TaskStatus.DONE, output_path=str(output))

    async def infected(_argv):
        return 1, "FOUND"

    result = asyncio.run(apply_post_download_scan(task, runner=infected, defender_factory=lambda: ["unused"]))
    assert result.state == "threat"
    assert task.status is TaskStatus.FAILED

    task.status = TaskStatus.DONE
    task.error_code = ""

    async def crashed(_argv):
        return 3, "engine down"

    result = asyncio.run(apply_post_download_scan(task, runner=crashed, defender_factory=lambda: ["unused"]))
    assert result.state == "error"
    assert task.status is TaskStatus.DONE


def test_directory_output_is_skipped(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "av_scan_enabled", True)
    folder = tmp_path / "bundle"
    folder.mkdir()
    task = Task(id="av-dir", url="magnet:?xt=urn:btih:abc", status=TaskStatus.DONE, output_path=str(folder))
    result = asyncio.run(apply_post_download_scan(task, defender_factory=lambda: ["MpCmdRun.exe"]))
    assert result.state == "skipped"
    assert task.status is TaskStatus.DONE


def test_interpret_scan_exit_and_resolve_command():
    assert interpret_scan_exit("defender", 0, "").state == "clean"
    assert interpret_scan_exit("defender", 2, "bad").state == "threat"
    assert interpret_scan_exit("custom", 1, "bad").state == "threat"
    engine, argv = resolve_scan_command("C:/a.bin", command_template="", defender_factory=lambda: ["MpCmdRun.exe", "-File"])
    assert engine == "defender"
    assert argv[-1] == "C:/a.bin"