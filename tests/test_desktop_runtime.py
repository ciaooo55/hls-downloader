import sys
import threading
from pathlib import Path

from fastapi.testclient import TestClient

from backend.app.desktop_runtime import (
    activate_window,
    present_browser_handoff,
    register_activation,
    register_browser_handoff,
    register_shutdown,
    request_shutdown,
    set_desktop_handoff_session,
)
from backend.app import main as main_module
from backend.app.main import app
import backend.desktop as desktop_module
from backend.desktop import (
    BrowserHandoffWindowManager,
    DesktopBridge,
    DesktopController,
    StartupExitController,
    SingleInstanceLock,
    UvicornServerThread,
    _activate_windows_window,
    desktop_ui_url,
    public_base_url,
)


class FakeWindow:
    def __init__(self, confirm_result: bool = True) -> None:
        self.confirm_result = confirm_result
        self.calls: list[str] = []
        self.selected_folders: tuple[str, ...] | None = None

    def create_confirmation_dialog(self, title: str, message: str) -> bool:
        self.calls.append(f"confirm:{title}:{message}")
        return self.confirm_result

    def restore(self) -> None:
        self.calls.append("restore")

    def show(self) -> None:
        self.calls.append("show")

    def load_url(self, url: str) -> None:
        self.calls.append(f"load-url:{url}")

    @property
    def on_top(self) -> bool:
        return False

    @on_top.setter
    def on_top(self, value: bool) -> None:
        self.calls.append(f"on-top:{value}")

    def hide(self) -> None:
        self.calls.append("hide")

    def destroy(self) -> None:
        self.calls.append("destroy")

    def create_file_dialog(self, dialog_type):
        self.calls.append(f"file-dialog:{dialog_type}")
        return self.selected_folders


class FakeServer:
    def __init__(self) -> None:
        self.calls: list[str] = []

    def stop(self) -> None:
        self.calls.append("stop")

    def join(self, timeout: float | None = None) -> None:
        self.calls.append(f"join:{timeout}")


class FakeTray:
    def __init__(self) -> None:
        self.calls: list[str] = []

    def stop(self) -> None:
        self.calls.append("stop")


class FakeKernel32:
    def __init__(self, last_error: int = 0) -> None:
        self.last_error = last_error
        self.closed: list[int] = []

    def CreateMutexW(self, *_args):
        return 42

    def GetLastError(self):
        return self.last_error

    def CloseHandle(self, handle):
        self.closed.append(handle)


class FakeUser32:
    def __init__(self, hwnd=42) -> None:
        self.hwnd = hwnd
        self.calls = []

    def FindWindowW(self, class_name, title):
        self.calls.append(("find", class_name, title))
        return self.hwnd

    def ShowWindow(self, hwnd, mode):
        self.calls.append(("show", hwnd, mode))

    def SetWindowPos(self, hwnd, insert_after, x, y, width, height, flags):
        self.calls.append(("position", hwnd, insert_after, flags))

    def SetForegroundWindow(self, hwnd):
        self.calls.append(("foreground", hwnd))

    def BringWindowToTop(self, hwnd):
        self.calls.append(("bring-to-top", hwnd))


def test_window_close_hides_to_tray_and_keeps_server_running():
    window = FakeWindow()
    server = FakeServer()
    controller = DesktopController(window, server)

    assert controller.on_closing() is False

    assert server.calls == []
    assert window.calls == ["hide"]
    assert "destroy" not in window.calls


def test_single_instance_lock_holds_and_releases_windows_mutex():
    kernel = FakeKernel32()
    lock = SingleInstanceLock(kernel32=kernel)

    assert lock.acquire() is True
    assert kernel.closed == []
    lock.release()

    assert kernel.closed == [42]


def test_single_instance_lock_rejects_second_process():
    kernel = FakeKernel32(last_error=SingleInstanceLock.ERROR_ALREADY_EXISTS)
    lock = SingleInstanceLock(kernel32=kernel)

    assert lock.acquire() is False
    assert kernel.closed == [42]


def test_exit_stops_server_and_tray_before_destroying_window():
    window = FakeWindow()
    server = FakeServer()
    tray = FakeTray()
    controller = DesktopController(window, server, tray=tray, shutdown_timeout=12)

    assert controller.request_exit() is True
    assert controller.wait_for_shutdown(timeout=2)

    assert server.calls == ["stop", "join:12"]
    assert tray.calls == ["stop"]
    assert window.calls[-1] == "destroy"
    assert controller.on_closing() is True


def test_exit_stops_tray_before_waiting_for_server_shutdown():
    release = threading.Event()
    join_started = threading.Event()
    tray = FakeTray()

    class BlockingServer(FakeServer):
        def join(self, timeout: float | None = None) -> None:
            self.calls.append(f"join:{timeout}")
            join_started.set()
            release.wait(timeout=2)

    controller = DesktopController(FakeWindow(), BlockingServer(), tray=tray)

    try:
        assert controller.request_exit() is True
        assert join_started.wait(timeout=1)
        assert tray.calls == ["stop"]
    finally:
        release.set()
        assert controller.wait_for_shutdown(timeout=2)


def test_exit_continues_when_tray_or_window_hide_fails():
    class BrokenTray(FakeTray):
        def stop(self) -> None:
            super().stop()
            raise RuntimeError("tray failed")

    class BrokenWindow(FakeWindow):
        def hide(self) -> None:
            super().hide()
            raise RuntimeError("window failed")

    window = BrokenWindow()
    server = FakeServer()
    tray = BrokenTray()
    controller = DesktopController(window, server, tray=tray)

    assert controller.request_exit() is True
    assert controller.wait_for_shutdown(timeout=2)
    assert tray.calls == ["stop"]
    assert server.calls == ["stop", "join:20"]
    assert window.calls == ["hide", "destroy"]


def test_repeated_exit_does_not_start_duplicate_shutdown():
    window = FakeWindow()
    server = FakeServer()
    release = threading.Event()

    class BlockingServer(FakeServer):
        def join(self, timeout: float | None = None) -> None:
            self.calls.append(f"join:{timeout}")
            release.wait(timeout=1)

    server = BlockingServer()
    controller = DesktopController(window, server)

    assert controller.request_exit() is True
    assert controller.request_exit() is False
    release.set()
    assert controller.wait_for_shutdown(timeout=2)

    assert server.calls.count("stop") == 1


def test_exit_watchdog_forces_process_termination_if_window_loop_stalls():
    forced = threading.Event()
    exit_codes: list[int] = []
    controller = DesktopController(
        FakeWindow(),
        FakeServer(),
        force_exit=lambda code: (exit_codes.append(code), forced.set()),
        force_exit_delay=0.01,
    )

    assert controller.request_exit() is True
    assert controller.wait_for_shutdown(timeout=2)
    assert forced.wait(timeout=2)
    assert exit_codes == [0]


def test_exit_watchdog_runs_even_when_window_destroy_blocks():
    forced = threading.Event()
    release_destroy = threading.Event()

    class BlockingWindow(FakeWindow):
        def destroy(self) -> None:
            self.calls.append("destroy")
            release_destroy.wait(timeout=2)

    controller = DesktopController(
        BlockingWindow(),
        FakeServer(),
        force_exit=lambda _code: forced.set(),
        force_exit_delay=0.01,
    )

    try:
        assert controller.request_exit() is True
        assert forced.wait(timeout=1)
        assert controller.wait_for_shutdown(timeout=0.01) is False
    finally:
        release_destroy.set()
        assert controller.wait_for_shutdown(timeout=2)


def test_startup_exit_stops_server_and_forces_stalled_process_exit():
    forced = threading.Event()
    exit_codes: list[int] = []
    server = FakeServer()
    controller = StartupExitController(
        server,
        force_exit=lambda code: (exit_codes.append(code), forced.set()),
        force_exit_delay=0.01,
    )

    assert controller.request_exit() is True
    assert controller.request_exit() is False
    assert server.calls == ["stop"]
    assert forced.wait(timeout=1)
    assert exit_codes == [0]


def test_startup_exit_is_disabled_after_desktop_controller_takes_over():
    server = FakeServer()
    controller = StartupExitController(server)

    controller.disarm()

    assert controller.request_exit() is False
    assert server.calls == []


def test_activation_restores_and_shows_registered_window():
    window = FakeWindow()
    controller = DesktopController(
        window,
        FakeServer(),
        native_window_activator=lambda _window: False,
    )
    register_activation(controller.activate)

    assert activate_window() is True
    for _ in range(100):
        if len(window.calls) == 4:
            break
        threading.Event().wait(0.01)
    assert window.calls == ["restore", "show", "on-top:True", "on-top:False"]


def test_windows_activation_uses_native_window_api_without_webview_calls():
    user32 = FakeUser32()
    timers = []

    class FakeTimer:
        def __init__(self, delay, callback):
            self.delay = delay
            self.callback = callback
            self.daemon = False
            self.canceled = False
            timers.append(self)

        def start(self):
            pass

        def cancel(self):
            self.canceled = True

    assert _activate_windows_window(user32=user32, timer_factory=FakeTimer) is True
    assert user32.calls == [
        ("find", None, "HLS Downloader"),
        ("show", 42, 9),
        ("position", 42, -1, 0x0001 | 0x0002 | 0x0040),
        ("bring-to-top", 42),
        ("foreground", 42),
    ]
    assert timers[0].delay == 2.5
    assert timers[0].daemon is True
    timers[0].callback()
    assert user32.calls[-1] == ("position", 42, -2, 0x0001 | 0x0002 | 0x0040)


def test_activation_returns_without_waiting_for_blocked_window():
    started = threading.Event()
    release = threading.Event()

    def blocked_activation():
        started.set()
        release.wait(timeout=2)

    register_activation(blocked_activation)
    try:
        before = threading.Event()
        before.set()
        assert activate_window() is True
        assert started.wait(timeout=1.0)
        assert activate_window() is True
    finally:
        release.set()
        register_activation(None)


def test_activation_reports_false_when_no_window_is_registered():
    register_activation(None)
    assert activate_window() is False


def test_registered_shutdown_requests_controller_exit():
    calls: list[str] = []
    register_shutdown(lambda: calls.append("exit"))

    assert request_shutdown() is True
    assert calls == ["exit"]

    register_shutdown(None)
    assert request_shutdown() is False


def test_registered_shutdown_returns_callback_result():
    register_shutdown(lambda: False)

    assert request_shutdown() is False

    register_shutdown(None)


def test_activation_api_requires_token_and_calls_registered_window():
    calls: list[str] = []
    register_activation(lambda: calls.append("activate"))

    with TestClient(app) as client:
        unauthorized = client.post("/api/app/activate")
        activated = client.post("/api/app/activate", headers={"X-Token": "55555"})

    assert unauthorized.status_code == 401
    assert activated.status_code == 200
    assert activated.json() == {"ok": True}
    assert calls == ["activate"]


def test_shutdown_api_requires_token_and_calls_registered_shutdown():
    calls: list[str] = []
    register_shutdown(lambda: calls.append("shutdown"))

    with TestClient(app) as client:
        unauthorized = client.post("/api/app/shutdown")
        stopped = client.post("/api/app/shutdown", headers={"X-Token": "55555"})

    assert unauthorized.status_code == 401
    assert stopped.status_code == 200
    assert stopped.json() == {"ok": True}
    assert calls == ["shutdown"]


def test_public_base_url_uses_loopback_for_wildcard_host(monkeypatch):
    from backend import desktop as desktop_module

    monkeypatch.setattr(desktop_module.settings, "host", "0.0.0.0")
    monkeypatch.setattr(desktop_module.settings, "port", 9876)

    assert public_base_url() == "http://127.0.0.1:9876"


def test_desktop_ui_url_is_versioned_to_bypass_webview_cache(monkeypatch):
    monkeypatch.setattr(desktop_module, "APP_VERSION", "9.8.7")

    assert desktop_ui_url() == f"{public_base_url()}/ui?version=9.8.7"


def test_ui_files_disable_persistent_webview_cache(monkeypatch, tmp_path: Path):
    dist = tmp_path / "dist"
    dist.mkdir()
    (dist / "index.html").write_text("<h1>fresh UI</h1>", encoding="utf-8")
    (dist / "app.js").write_text("console.log('fresh')", encoding="utf-8")
    monkeypatch.setattr(main_module, "UI_DIST", dist)

    with TestClient(app) as client:
        index = client.get("/ui?version=9.8.7")
        asset = client.get("/ui/app.js")

    assert index.status_code == 200
    assert asset.status_code == 200
    assert "no-store" in index.headers["cache-control"]
    assert "no-store" in asset.headers["cache-control"]


def test_desktop_server_configures_without_console_streams(monkeypatch):
    monkeypatch.setattr(sys, "stdout", None)
    monkeypatch.setattr(sys, "stderr", None)

    server = UvicornServerThread()

    assert server.is_alive() is False


def test_desktop_bridge_choose_folder(tmp_path):
    window = FakeWindow()
    window.selected_folders = (str(tmp_path),)
    bridge = DesktopBridge(window, folder_dialog_type="folder")

    result = bridge.choose_folder(str(tmp_path))

    assert result == {"ok": True, "path": str(tmp_path)}
    assert "file-dialog:folder" in window.calls

    window.selected_folders = None
    canceled = bridge.choose_folder()
    assert canceled == {"ok": False, "canceled": True}


def test_desktop_bridge_opens_chrome_and_bundled_extension_folder(tmp_path):
    extension = tmp_path / "browser-extension" / "chrome"
    extension.mkdir(parents=True)
    (extension / "manifest.json").write_text("{}", encoding="utf-8")
    browser = tmp_path / "chrome.exe"
    browser.write_bytes(b"chrome")
    started: list[list[str]] = []
    bridge = DesktopBridge(
        browser_extension_path=extension,
        browser_executable=browser,
        process_starter=lambda command: started.append(command),
    )

    result = bridge.open_browser_extension_installer()

    assert result == {"ok": True, "path": str(extension), "browser_opened": True}
    assert started == [
        [str(browser), "chrome://extensions"],
        ["explorer.exe", str(extension)],
    ]


def test_desktop_bridge_reports_missing_bundled_extension(tmp_path):
    bridge = DesktopBridge(browser_extension_path=tmp_path / "missing")

    assert bridge.open_browser_extension_installer() == {
        "ok": False,
        "error": "安装包中缺少浏览器扩展，请重新安装最新版",
    }


def test_desktop_bridge_keeps_native_objects_private_from_js_discovery():
    bridge = DesktopBridge(window=FakeWindow(), folder_dialog_type="folder")

    assert vars(bridge)
    assert all(name.startswith("_") for name in vars(bridge))


def test_desktop_bridge_reports_installed_mode_and_starts_uninstaller(tmp_path):
    uninstaller = tmp_path / "Uninstall.exe"
    uninstaller.write_bytes(b"installer")
    started: list[list[str]] = []
    exits: list[str] = []
    window = FakeWindow(confirm_result=True)
    bridge = DesktopBridge(
        window=window,
        uninstaller_path=uninstaller,
        process_starter=lambda command: started.append(command),
        exit_request=lambda: exits.append("exit"),
    )

    assert bridge.get_desktop_info() == {"ok": True, "installed": True, "mode": "installed"}
    assert bridge.begin_uninstall() == {"ok": True}
    assert started == [[str(uninstaller)]]
    assert exits == ["exit"]
    assert any(call.startswith("confirm:卸载 HLS Downloader") for call in window.calls)


def test_desktop_bridge_does_not_offer_uninstall_without_uninstaller(tmp_path):
    bridge = DesktopBridge(window=FakeWindow(), uninstaller_path=tmp_path / "missing.exe")

    assert bridge.get_desktop_info()["installed"] is False
    assert bridge.begin_uninstall() == {"ok": False, "error": "当前版本无需卸载"}


class FakeEventHook:
    def __init__(self) -> None:
        self.handlers = []

    def __iadd__(self, handler):
        self.handlers.append(handler)
        return self

    def emit(self):
        for handler in list(self.handlers):
            handler()


class FakeChildEvents:
    def __init__(self) -> None:
        self.closing = FakeEventHook()
        self.closed = FakeEventHook()


class FakeChildWindow(FakeWindow):
    def __init__(self) -> None:
        super().__init__()
        self.events = FakeChildEvents()
        self._on_top = False

    @property
    def on_top(self) -> bool:
        return self._on_top

    @on_top.setter
    def on_top(self, value: bool) -> None:
        self._on_top = value
        self.calls.append(f"on-top:{value}")


def test_browser_handoffs_queue_during_cold_start_and_flush_independently():
    shown: list[str] = []
    ready = threading.Event()
    register_browser_handoff(None)
    set_desktop_handoff_session(False)
    try:
        # Pure backend mode falls back to manager/web UI instead of silently queueing.
        fallback = present_browser_handoff("web-only")
        assert fallback["mode"] == "ui-fallback"
        assert fallback["queued"] is False

        set_desktop_handoff_session(True)
        queued_one = present_browser_handoff("queued-one")
        queued_two = present_browser_handoff("queued-two")
        assert queued_one["ok"] is True and queued_one["queued"] is True
        assert queued_two["mode"] == "desktop-pending"

        def show(handoff_id: str) -> None:
            shown.append(handoff_id)
            if {"queued-one", "queued-two"}.issubset(shown):
                ready.set()

        register_browser_handoff(show)
        assert ready.wait(timeout=1)
        assert "queued-one" in shown
        assert "queued-two" in shown
    finally:
        register_browser_handoff(None)
        set_desktop_handoff_session(False)


def test_browser_handoff_window_manager_creates_one_window_per_request():
    created = []
    canceled: list[str] = []
    presented: list[str] = []

    class MasterWindow:
        x = 100
        y = 60

    def create_window(title, url, **kwargs):
        window = FakeChildWindow()
        created.append((title, url, kwargs, window))
        return window

    manager = BrowserHandoffWindowManager(
        MasterWindow(),
        create_window,
        "folder",
        url_builder=lambda handoff_id: f"http://local/ui?handoff={handoff_id}",
        cancel_handoff=canceled.append,
        mark_presented=presented.append,
    )

    manager.show("one")
    manager.show("two")
    manager.show("one")

    assert len(created) == 2
    assert created[0][1].endswith("handoff=one")
    assert created[1][1].endswith("handoff=two")
    assert created[0][2]["on_top"] is True
    assert created[0][2]["x"] != created[1][2]["x"]
    assert created[0][3].calls[:2] == ["restore", "show"]
    assert presented == ["one", "two", "one"]

    created[0][3].events.closing.emit()
    for _ in range(50):
        if canceled:
            break
        threading.Event().wait(0.01)
    assert canceled == ["one"]

    manager.mark_resolved("two")
    created[1][3].events.closing.emit()
    for _ in range(20):
        if "two" in canceled:
            break
        threading.Event().wait(0.01)
    assert canceled == ["one"]


def test_background_controller_defers_manager_ui_until_opened():
    window = FakeWindow()
    controller = DesktopController(
        window,
        FakeServer(),
        native_window_activator=lambda _window: False,
        deferred_ui=True,
    )

    controller.activate()

    assert window.calls[0].startswith("load-url:http://")
    assert window.calls[1:5] == ["restore", "show", "on-top:True", "on-top:False"]


def test_presenter_status_endpoint_reflects_desktop_session():
    from backend.app.desktop_runtime import (
        has_browser_handoff_presenter,
        is_desktop_handoff_session,
        register_browser_handoff,
        set_desktop_handoff_session,
    )

    register_browser_handoff(None)
    set_desktop_handoff_session(False)
    client = TestClient(app)
    try:
        idle = client.get("/api/browser/presenter", headers={"X-Token": "55555"})
        assert idle.status_code == 200
        assert idle.json()["ready"] is False
        assert idle.json()["session"] is False
        assert idle.json()["mode"] == "ui-fallback"

        set_desktop_handoff_session(True)
        pending = client.get("/api/browser/presenter", headers={"X-Token": "55555"})
        body = pending.json()
        assert body["session"] is True
        assert body["ready"] is False
        assert body["mode"] == "desktop-pending"

        register_browser_handoff(lambda _handoff_id: None)
        ready = client.get("/api/browser/presenter", headers={"X-Token": "55555"})
        ready_body = ready.json()
        assert ready_body["ready"] is True
        assert ready_body["mode"] == "desktop"
        assert has_browser_handoff_presenter() is True
        assert is_desktop_handoff_session() is True
    finally:
        register_browser_handoff(None)
        set_desktop_handoff_session(False)
