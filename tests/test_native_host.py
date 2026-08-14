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


def test_wait_presenter_returns_for_native_shell_without_tauri(monkeypatch):
    calls = []

    def request(method, path):
        calls.append((method, path))
        return {"ready": True, "session": True, "mode": "native-shell"}

    monkeypatch.setattr(native_host, "_request", request)
    monkeypatch.setattr(
        native_host.time,
        "sleep",
        lambda _seconds: (_ for _ in ()).throw(AssertionError("unexpected wait")),
    )

    native_host._wait_presenter(18.0)

    assert calls == [("GET", "/browser/presenter")]


def test_offer_posts_handoff_without_a_prior_browser_ping(monkeypatch):
    calls = []
    monkeypatch.setattr(native_host, "_ensure_app", lambda *_args, **_kwargs: None)

    def request(method, path, payload=None, timeout=4):
        calls.append((method, path, payload))
        if path == "/browser/handoffs":
            return {"id": "handoff-1", "status": "pending"}
        raise AssertionError(f"unexpected {method} {path}")

    monkeypatch.setattr(native_host, "_request", request)
    result = native_host.dispatch({"op": "offer", "resource": {"url": "https://cdn.test/a.mp4"}})

    assert result == {"ok": True, "handoff": {"id": "handoff-1", "status": "pending"}}
    assert calls == [("POST", "/browser/handoffs", {"url": "https://cdn.test/a.mp4"})]
