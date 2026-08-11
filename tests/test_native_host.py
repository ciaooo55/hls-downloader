from backend import native_host


def test_wait_presenter_returns_as_soon_as_session_can_queue(monkeypatch):
    calls = []

    def request(method, path):
        calls.append((method, path))
        return {"ready": False, "session": True}

    monkeypatch.setattr(native_host, "_request", request)
    monkeypatch.setattr(
        native_host.time,
        "sleep",
        lambda _seconds: (_ for _ in ()).throw(AssertionError("unexpected wait")),
    )

    native_host._wait_presenter(18.0)

    assert calls == [("GET", "/browser/presenter")]


def test_cold_offer_uses_one_presenter_deadline_instead_of_two_waits(monkeypatch):
    health_calls = 0
    presenter_timeouts = []
    starts = []

    def request(method, path):
        nonlocal health_calls
        assert (method, path) == ("GET", "/health")
        health_calls += 1
        raise OSError("core is not running")

    monkeypatch.setattr(native_host, "_request", request)
    monkeypatch.setattr(native_host, "_start_app", lambda: starts.append(True))
    monkeypatch.setattr(native_host, "_wait_presenter", presenter_timeouts.append)

    native_host._ensure_app(require_presenter=True)

    assert health_calls == 1
    assert starts == [True]
    assert presenter_timeouts == [18.0]
