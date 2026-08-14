import threading
import time

import pytest

from backend.app.native_shell import (
    NativeShellSupervisor,
    decode_frame,
    encode_frame,
    paint_snapshot,
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
