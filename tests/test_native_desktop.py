import threading
import time

from backend.app.native_desktop import NativeDesktopSession


def test_native_desktop_session_queues_commands_in_order():
    session = NativeDesktopSession()
    assert session.activate() is False
    session.start()
    assert session.activate() is True
    session.handoff("offer-1")
    result = session.poll(0, 0)
    assert [item["kind"] for item in result["commands"]] == ["activate", "handoff"]
    assert result["commands"][1]["handoff_id"] == "offer-1"


def test_native_desktop_poll_wakes_for_new_command():
    session = NativeDesktopSession()
    session.start()
    result = {}

    def poll():
        result.update(session.poll(0, 2))

    worker = threading.Thread(target=poll)
    worker.start()
    time.sleep(0.03)
    session.shutdown()
    worker.join(1)
    assert not worker.is_alive()
    assert result["commands"][0]["kind"] == "shutdown"


def test_native_desktop_stop_releases_long_poll():
    session = NativeDesktopSession()
    session.start()
    result = {}
    worker = threading.Thread(target=lambda: result.update(session.poll(0, 2)))
    worker.start()
    time.sleep(0.03)
    session.stop()
    worker.join(1)
    assert result["active"] is False
