import socket
import threading
import time

import pytest

from backend.app.native_shell import (
    NativeShellSupervisor,
    NativeShellIpcServer,
    boot_native_shell,
    decode_frame,
    dispatch_ipc,
    encode_frame,
    is_native_shell_ready,
    native_shell_supervisor,
    paint_snapshot,
    read_frame,
    reset_native_shell,
    write_frame,
)


def test_resident_boot_keeps_main_window_closed_and_overlays_warm():
    shell = NativeShellSupervisor()
    status = shell.boot_resident()
    assert status["resident"] is True
    assert status["main_open"] is False
    assert status["windows"] == {"handoff": True, "progress": True, "complete": True}
    assert status["hot_path"] == "pipe-to-precreated-window"
    shell.hide_main()
    assert shell.status()["resident"] is True
    assert shell.status()["main_open"] is False


def test_offer_before_resident_boot_fails_fast():
    shell = NativeShellSupervisor()
    with pytest.raises(RuntimeError, match="尚未就绪"):
        shell.offer({"id": "h1", "filename": "setup.exe", "url": "https://cdn.test/setup.exe"})


def test_offer_paints_from_snapshot_without_another_fetch():
    shell = NativeShellSupervisor()
    shell.boot_resident()
    event = shell.offer({
        "id": "h1",
        "filename": "setup.exe",
        "url": "https://cdn.test/setup.exe",
        "size": 4096,
        "cookie": "must-not-be-required-for-first-paint",
        "resource_kind": "file",
    })
    assert event["presentable"] is True
    assert event["kind"] == "handoff"
    assert event["snapshot"]["filename"] == "setup.exe"
    assert event["snapshot"]["url"] == "https://cdn.test/setup.exe"
    assert event["snapshot"]["size"] == 4096
    assert "cookie" not in event["snapshot"]
    frame = encode_frame(event)
    restored = decode_frame(frame)
    assert restored["snapshot"]["filename"] == "setup.exe"


def test_click_path_wakes_immediately_instead_of_long_polling():
    shell = NativeShellSupervisor()
    shell.boot_resident()
    result = {}

    def wait():
        result.update(shell.wait_event(0, 2))

    worker = threading.Thread(target=wait)
    worker.start()
    time.sleep(0.02)
    started = time.monotonic()
    shell.offer({"id": "h2", "filename": "a.mp4", "url": "https://cdn.test/a.mp4"})
    worker.join(1)
    elapsed_ms = (time.monotonic() - started) * 1000
    assert not worker.is_alive()
    assert result["events"][0]["kind"] == "handoff"
    assert result["events"][0]["presentable"] is True
    assert elapsed_ms < 200


def test_progress_and_complete_use_the_same_warm_windows():
    shell = NativeShellSupervisor()
    shell.boot_resident()
    progress = shell.progress([{"id": "t1", "filename": "a.bin", "percent": 40}])
    complete = shell.complete({"id": "t1", "filename": "a.bin"})
    assert progress["presentable"] is True
    assert complete["presentable"] is True
    shell.open_main()
    assert shell.status()["main_open"] is True
    shell.hide_main()
    assert shell.status()["resident"] is True
    shell.shutdown()
    assert shell.status()["resident"] is False


def test_paint_snapshot_drops_unknown_size_and_keeps_name():
    assert paint_snapshot({"id": "x", "filename": "doc.pdf", "size": "nope"}) == {
        "id": "x",
        "url": "",
        "filename": "doc.pdf",
        "title": "",
        "mime_type": "",
        "size": 0,
        "resource_kind": "file",
        "status": "pending",
    }


def test_global_supervisor_boot_and_reset():
    reset_native_shell()
    assert is_native_shell_ready() is False
    status = boot_native_shell()
    assert status["resident"] is True
    assert is_native_shell_ready() is True
    assert native_shell_supervisor().windows["handoff"] is True
    reset_native_shell()
    assert is_native_shell_ready() is False


def test_dispatch_ipc_offer_and_hide_main_keep_resident():
    shell = NativeShellSupervisor()
    hello = dispatch_ipc(shell, {"op": "hello"})
    assert hello["protocol"] == "hls-downloader-native-shell"
    boot = dispatch_ipc(shell, {"op": "boot"})
    assert boot["windows"]["complete"] is True
    event = dispatch_ipc(shell, {
        "op": "offer",
        "handoff": {"id": "h3", "filename": "a.exe", "url": "https://cdn.test/a.exe", "cookie": "secret"},
    })
    assert event["presentable"] is True
    assert "cookie" not in event["snapshot"]
    opened = dispatch_ipc(shell, {"op": "open_main"})
    assert opened["main_open"] is True
    hidden = dispatch_ipc(shell, {"op": "hide_main"})
    assert hidden["main_open"] is False
    assert hidden["resident"] is True
    assert shell.is_ready() is True


def test_tcp_ipc_roundtrip_paints_without_http():
    shell = NativeShellSupervisor()
    server = NativeShellIpcServer(shell)
    endpoint = server.start()
    try:
        client = socket.create_connection((endpoint["host"], endpoint["port"]), timeout=2)
        try:
            write_frame(client, {"op": "boot"})
            boot = read_frame(client)
            assert boot["resident"] is True
            write_frame(client, {
                "op": "offer",
                "handoff": {"id": "pipe-1", "filename": "setup.exe", "url": "https://cdn.test/setup.exe", "size": 8},
            })
            event = read_frame(client)
            assert event["kind"] == "handoff"
            assert event["snapshot"]["filename"] == "setup.exe"
            assert event["presentable"] is True
        finally:
            client.close()
    finally:
        server.stop()


def test_locate_packaged_supervisor_and_skip_spawn_in_pytest(tmp_path):
    from backend.app.native_shell import locate_native_shell_executable, maybe_spawn_native_shell_process

    exe = tmp_path / "HLSNativeShell.exe"
    exe.write_bytes(b"MZ")
    assert locate_native_shell_executable(tmp_path) == exe
    assert locate_native_shell_executable(tmp_path / "missing") is None
    assert maybe_spawn_native_shell_process(
        core_url="http://127.0.0.1:8765/api",
        token="x" * 40,
        project_root=tmp_path,
    ) is None


def test_windows_spawn_uses_detached_flags_not_create_no_window(monkeypatch, tmp_path):
    from backend.app import native_shell as ns

    captured = {}

    class FakeProcess:
        pass

    def fake_popen(*args, **kwargs):
        captured["args"] = args
        captured["kwargs"] = kwargs
        return FakeProcess()

    exe = tmp_path / "HLSNativeShell.exe"
    exe.write_bytes(b"MZ")
    monkeypatch.setattr(ns, "running_on_windows", lambda: True)
    monkeypatch.delenv("PYTEST_CURRENT_TEST", raising=False)
    monkeypatch.setattr(ns.subprocess, "Popen", fake_popen)

    result = ns.maybe_spawn_native_shell_process(
        core_url="http://127.0.0.1:8765/api",
        token="x" * 40,
        project_root=tmp_path,
    )

    assert result == exe
    flags = captured["kwargs"]["creationflags"]
    assert flags & 0x00000008
    assert flags & 0x00000200
    assert flags & 0x08000000 == 0
    assert captured["kwargs"]["close_fds"] is False
    assert captured["kwargs"]["stdin"] is ns.subprocess.DEVNULL

