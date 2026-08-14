from backend import native_host


def test_frozen_install_root_accepts_native_shell_marker(tmp_path):
    install_root = tmp_path / "HLS Downloader"
    host = (
        install_root
        / "native-host"
        / "versions"
        / "HLSDownloaderNativeHost-5.0.14.exe"
    )
    host.parent.mkdir(parents=True)
    host.write_bytes(b"host")
    (install_root / "HLSNativeShell.exe").write_bytes(b"MZ")

    assert native_host._frozen_install_root(host) == install_root


def test_start_app_prefers_native_shell(tmp_path, monkeypatch):
    (tmp_path / "HLSNativeShell.exe").write_bytes(b"MZ")
    (tmp_path / "HLSDownloader.exe").write_bytes(b"MZ")
    monkeypatch.setattr(native_host, "ROOT", tmp_path)
    captured = {}

    def fake_popen(args, **kwargs):
        captured["args"] = list(args)
        captured["kwargs"] = kwargs

        class Process:
            pass

        return Process()

    monkeypatch.setattr(native_host.subprocess, "Popen", fake_popen)
    native_host._start_app()
    assert captured["args"][0].endswith("HLSNativeShell.exe")
    assert "--background" not in captured["args"]
    flags = captured["kwargs"]["creationflags"]
    assert flags & 0x00000008
    assert flags & 0x08000000 == 0


def test_start_app_falls_back_to_desktop_background(tmp_path, monkeypatch):
    (tmp_path / "HLSDownloader.exe").write_bytes(b"MZ")
    monkeypatch.setattr(native_host, "ROOT", tmp_path)
    captured = {}

    def fake_popen(args, **kwargs):
        captured["args"] = list(args)
        captured["kwargs"] = kwargs

        class Process:
            pass

        return Process()

    monkeypatch.setattr(native_host.subprocess, "Popen", fake_popen)
    native_host._start_app()
    assert captured["args"][0].endswith("HLSDownloader.exe")
    assert "--background" in captured["args"]
    assert captured["kwargs"]["creationflags"] == 0x08000000


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
