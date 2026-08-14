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


def test_native_desktop_queues_media_push_until_session_starts():
    session = NativeDesktopSession()
    assert session.push("media_push", "req-1") is False
    assert session.activate() is False
    assert session.queue("media_push", "req-1") is True
    assert session.poll(0, 0)["commands"] == []
    session.start()
    result = session.poll(0, 0)
    assert result["commands"][0]["kind"] == "media_push"
    assert result["commands"][0]["handoff_id"] == "req-1"
    assert session.queue("media_push", "req-2") is True
    follow = session.poll(result["sequence"], 0)
    assert follow["commands"][0]["handoff_id"] == "req-2"


def test_native_desktop_start_does_not_replay_old_commands():
    session = NativeDesktopSession()
    session.start()
    assert session.activate() is True
    session.handoff("old-offer")
    session.stop()
    assert session.queue("media_push", "req-new") is True
    session.start()
    kinds = [item["kind"] for item in session.poll(0, 0)["commands"]]
    assert kinds == ["media_push"]
    assert session.poll(0, 0)["commands"][0]["handoff_id"] == "req-new"
