import json
import logging
import os
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import webbrowser
from pathlib import Path

import uvicorn


logger = logging.getLogger(__name__)

try:
    from .app.config import PROJECT_ROOT, settings
    from .app.paths import RUNTIME_PATHS
    from .app.version import APP_VERSION
    from .app.desktop_runtime import register_activation, register_browser_handoff, register_shutdown, set_desktop_handoff_session
    from .app.main import app
    from .app.userscript_service import USERSCRIPT_FILENAME, export_userscript, render_userscript
except ImportError:
    from app.config import PROJECT_ROOT, settings
    from app.paths import RUNTIME_PATHS
    from app.version import APP_VERSION
    from app.desktop_runtime import register_activation, register_browser_handoff, register_shutdown, set_desktop_handoff_session
    from app.main import app
    from app.userscript_service import USERSCRIPT_FILENAME, export_userscript, render_userscript


def public_base_url() -> str:
    host = settings.host
    if host in {"0.0.0.0", "::"}:
        host = "127.0.0.1"
    return f"http://{host}:{settings.port}"


def desktop_ui_url() -> str:
    return f"{public_base_url()}/ui?version={APP_VERSION}"


def browser_handoff_ui_url(handoff_id: str) -> str:
    return f"{public_base_url()}/ui?handoff={handoff_id}&version={APP_VERSION}"


def desktop_host_url() -> str:
    return f"{public_base_url()}/ui?host=1&version={APP_VERSION}"


class DesktopBridge:
    def __init__(
        self,
        window=None,
        folder_dialog_type=None,
        source_path: Path | None = None,
        url_opener=None,
        uninstaller_path: Path | None = None,
        process_starter=None,
        exit_request=None,
        browser_extension_path: Path | None = None,
        browser_executable: Path | None = None,
    ) -> None:
        self._window = window
        self._folder_dialog_type = folder_dialog_type
        self._source_path = source_path
        self._url_opener = url_opener or webbrowser.open
        self._uninstaller_path = uninstaller_path or Path(PROJECT_ROOT) / "Uninstall.exe"
        self._process_starter = process_starter or subprocess.Popen
        self._exit_request = exit_request
        self._browser_extension_path = browser_extension_path or Path(PROJECT_ROOT) / "browser-extension" / "chrome"
        self._browser_executable = browser_executable

    def _set_window(self, window) -> None:
        self._window = window

    def _set_exit_request(self, callback) -> None:
        self._exit_request = callback

    def export_userscript(self) -> dict:
        if self._window is None:
            return {"ok": False, "error": "桌面窗口尚未就绪"}
        selected = self._window.create_file_dialog(self._folder_dialog_type)
        if not selected:
            return {"ok": False, "canceled": True}

        source_path = self._source_path or Path(PROJECT_ROOT) / "userscript" / USERSCRIPT_FILENAME
        source = source_path.read_text(encoding="utf-8")
        host = settings.host if settings.host not in {"0.0.0.0", "::"} else "127.0.0.1"
        content = render_userscript(
            source,
            host=host,
            port=settings.port,
            token=settings.token,
        )
        directory = Path(selected[0])
        target = directory / USERSCRIPT_FILENAME
        overwrite = False
        if target.exists():
            overwrite = self._window.create_confirmation_dialog(
                "覆盖油猴脚本",
                f"{target.name} 已存在，是否覆盖？",
            )
            if not overwrite:
                return {"ok": False, "canceled": True}
        exported = export_userscript(directory, content, overwrite=overwrite)
        return {"ok": True, "path": str(exported)}

    def open_userscript_installer(self) -> dict:
        self._url_opener(f"{public_base_url()}/userscript/m3u8-sniffer.user.js")
        return {"ok": True}

    def open_browser_extension_installer(self) -> dict:
        manifest = self._browser_extension_path / "manifest.json"
        if not manifest.is_file():
            return {"ok": False, "error": "安装包中缺少浏览器扩展，请重新安装最新版"}
        browser = self._browser_executable
        if browser is None:
            candidates = [
                Path(os.environ.get("PROGRAMFILES", "")) / "Google" / "Chrome" / "Application" / "chrome.exe",
                Path(os.environ.get("PROGRAMFILES(X86)", "")) / "Google" / "Chrome" / "Application" / "chrome.exe",
                Path(os.environ.get("LOCALAPPDATA", "")) / "Google" / "Chrome" / "Application" / "chrome.exe",
            ]
            browser = next((path for path in candidates if path.is_file()), None)
        try:
            if browser is not None:
                self._process_starter([str(browser), "chrome://extensions"])
            self._process_starter(["explorer.exe", str(self._browser_extension_path)])
        except OSError as exc:
            return {"ok": False, "error": f"无法打开扩展安装位置：{exc}"}
        return {"ok": True, "path": str(self._browser_extension_path), "browser_opened": browser is not None}

    def get_desktop_info(self) -> dict:
        installed = self._uninstaller_path.is_file()
        mode = "installed" if installed else RUNTIME_PATHS.mode
        return {"ok": True, "installed": installed, "mode": mode}

    def close_window(self) -> dict:
        window = self._window
        if window is None:
            return {"ok": False, "error": "桌面窗口尚未就绪"}

        def close() -> None:
            try:
                window.destroy()
            except Exception:
                logger.exception("failed to close desktop child window")

        timer = threading.Timer(0.05, close)
        timer.daemon = True
        timer.start()
        return {"ok": True}

    def begin_uninstall(self) -> dict:
        if not self._uninstaller_path.is_file():
            return {"ok": False, "error": "当前版本无需卸载"}
        if self._window is None:
            return {"ok": False, "error": "桌面窗口尚未就绪"}
        confirmed = self._window.create_confirmation_dialog(
            "卸载 HLS Downloader",
            "将关闭下载器并打开卸载程序，是否继续？",
        )
        if not confirmed:
            return {"ok": False, "canceled": True}
        try:
            self._process_starter([str(self._uninstaller_path)])
        except OSError as exc:
            return {"ok": False, "error": f"无法启动卸载程序：{exc}"}
        if self._exit_request is not None:
            self._exit_request()
        return {"ok": True}




def _mark_browser_handoff_presented(handoff_id: str) -> None:
    try:
        from .app.browser_handoff import browser_handoffs
    except ImportError:
        from app.browser_handoff import browser_handoffs
    browser_handoffs.mark_presentation(handoff_id, "presented")


def _cancel_browser_handoff(handoff_id: str) -> None:
    try:
        request = urllib.request.Request(
            f"{public_base_url()}/api/browser/handoffs/{handoff_id}/cancel",
            data=b"{}",
            headers={"Content-Type": "application/json", "X-Token": settings.token},
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=1):
            pass
    except (OSError, urllib.error.URLError):
        # Closing an already accepted/expired window is harmless.
        pass


class BrowserHandoffWindowManager:
    """Owns one independent native window for every browser download offer."""

    def __init__(
        self,
        master_window,
        create_window,
        folder_dialog_type,
        *,
        url_builder=browser_handoff_ui_url,
        cancel_handoff=_cancel_browser_handoff,
        mark_presented=None,
    ) -> None:
        self.master_window = master_window
        self.create_window = create_window
        self.folder_dialog_type = folder_dialog_type
        self.url_builder = url_builder
        self.cancel_handoff = cancel_handoff
        self.mark_presented = mark_presented
        self._windows: dict[str, object] = {}
        self._closing: set[str] = set()
        self._resolved: set[str] = set()
        self._lock = threading.RLock()
        self._create_lock = threading.Lock()
        self._sequence = 0

    def _position(self) -> tuple[int, int]:
        with self._lock:
            slot = self._sequence % 9
            self._sequence += 1
        try:
            base_x = int(getattr(self.master_window, "x", 120) or 120)
            base_y = int(getattr(self.master_window, "y", 80) or 80)
        except (TypeError, ValueError):
            base_x, base_y = 120, 80
        return max(20, base_x + 70 + slot * 28), max(20, base_y + 55 + slot * 24)

    @staticmethod
    def _raise(window, *, keep_on_top: bool = True) -> None:
        try:
            window.restore()
        except Exception:
            pass
        try:
            window.show()
        except Exception:
            pass
        try:
            # Newest confirm dialog should win focus over the browser and siblings.
            window.on_top = False
            window.on_top = True
            if not keep_on_top:
                window.on_top = False
        except Exception:
            logger.exception("failed to foreground browser handoff window")

    def _mark_presented(self, handoff_id: str) -> None:
        if self.mark_presented is None:
            return
        try:
            self.mark_presented(handoff_id)
        except Exception:
            logger.exception("failed to mark browser handoff presented %s", handoff_id)

    def mark_resolved(self, handoff_id: str) -> None:
        handoff_id = str(handoff_id).strip()
        if not handoff_id:
            return
        with self._lock:
            self._resolved.add(handoff_id)

    def show(self, handoff_id: str) -> None:
        handoff_id = str(handoff_id).strip()
        if not handoff_id:
            return
        with self._create_lock:
            with self._lock:
                existing = self._windows.get(handoff_id)
            if existing is not None:
                self._raise(existing)
                self._mark_presented(handoff_id)
                return

            x, y = self._position()
            bridge = DesktopBridge(folder_dialog_type=self.folder_dialog_type)
            title = f"下载文件信息 - HLS Downloader [{handoff_id[:6]}]"
            try:
                window = self.create_window(
                    title,
                    self.url_builder(handoff_id),
                    js_api=bridge,
                    width=500,
                    height=620,
                    x=x,
                    y=y,
                    min_size=(420, 520),
                    resizable=True,
                    on_top=True,
                    focus=True,
                    background_color="#17191d",
                    text_select=True,
                )
            except Exception:
                logger.exception("failed to create browser handoff window %s", handoff_id)
                try:
                    from backend.app.browser_handoff import browser_handoffs
                except ImportError:
                    from app.browser_handoff import browser_handoffs
                browser_handoffs.mark_presentation(handoff_id, "failed", "window create failed")
                raise
            if window is None:
                try:
                    from backend.app.browser_handoff import browser_handoffs
                except ImportError:
                    from app.browser_handoff import browser_handoffs
                browser_handoffs.mark_presentation(handoff_id, "failed", "window create cancelled")
                raise RuntimeError(f"browser handoff window was not created for {handoff_id}")
            bridge._set_window(window)

            def on_closing(*_args) -> None:
                with self._lock:
                    if handoff_id in self._closing:
                        return
                    self._closing.add(handoff_id)
                    resolved = handoff_id in self._resolved
                if resolved:
                    return
                threading.Thread(
                    target=self.cancel_handoff,
                    args=(handoff_id,),
                    name=f"cancel-handoff-{handoff_id[:8]}",
                    daemon=True,
                ).start()

            def on_closed(*_args) -> None:
                with self._lock:
                    self._windows.pop(handoff_id, None)
                    self._closing.discard(handoff_id)
                    self._resolved.discard(handoff_id)

            window.events.closing += on_closing
            window.events.closed += on_closed
            with self._lock:
                self._windows[handoff_id] = window
            self._raise(window)
            self._mark_presented(handoff_id)

    def close(self, handoff_id: str) -> None:
        handoff_id = str(handoff_id).strip()
        if not handoff_id:
            return
        self.mark_resolved(handoff_id)
        with self._lock:
            window = self._windows.get(handoff_id)
        if window is None:
            return
        try:
            window.destroy()
        except Exception:
            logger.exception("failed to close browser handoff window %s", handoff_id)

    def close_all(self) -> None:
        with self._lock:
            windows = list(self._windows.items())
            for handoff_id, _window in windows:
                self._resolved.add(handoff_id)
        for _handoff_id, window in windows:
            try:
                window.destroy()
            except Exception:
                logger.exception("failed to close browser handoff window")


class UvicornServerThread:
    def __init__(self) -> None:
        config = uvicorn.Config(
            app,
            host=settings.host,
            port=settings.port,
            log_level="warning",
            access_log=False,
            use_colors=False,
        )
        self._server = uvicorn.Server(config)
        self._thread = threading.Thread(target=self._server.run, name="hls-api", daemon=True)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._server.should_exit = True

    def join(self, timeout: float | None = None) -> None:
        self._thread.join(timeout)

    def is_alive(self) -> bool:
        return self._thread.is_alive()

    def wait_until_ready(self, timeout: float = 20) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not self.is_alive():
                return False
            try:
                with urllib.request.urlopen(f"{public_base_url()}/api/health", timeout=1) as response:
                    if response.status == 200:
                        return True
            except (OSError, urllib.error.URLError):
                time.sleep(0.1)
        return False


def _send_existing_instance_command(command: str) -> bool:
    try:
        request = urllib.request.Request(
            f"{public_base_url()}/api/app/{command}",
            data=b"{}",
            headers={"Content-Type": "application/json", "X-Token": settings.token},
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=2) as response:
            body = json.loads(response.read().decode("utf-8"))
            return response.status == 200 and body.get("ok") is True
    except (OSError, ValueError, urllib.error.URLError):
        return False


def activate_existing_instance() -> bool:
    return _send_existing_instance_command("activate")


def shutdown_existing_instance() -> bool:
    return _send_existing_instance_command("shutdown")


def _create_tray_image():
    from PIL import Image, ImageDraw

    icon_path = Path(PROJECT_ROOT) / "assets" / "app-icon.png"
    if icon_path.is_file():
        with Image.open(icon_path) as source:
            return source.convert("RGBA").resize((64, 64), Image.Resampling.LANCZOS)

    image = Image.new("RGBA", (64, 64), (23, 25, 29, 255))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((8, 8, 56, 56), radius=10, fill=(39, 43, 50, 255))
    draw.line((32, 16, 32, 39), fill=(70, 194, 126, 255), width=7)
    draw.polygon(((20, 34), (32, 47), (44, 34)), fill=(70, 194, 126, 255))
    draw.line((18, 50, 46, 50), fill=(235, 238, 242, 255), width=4)
    return image


_attention_timer_lock = threading.Lock()
_attention_timer = None


def _redraw_windows_window(user32=None) -> bool:
    if os.name != "nt" and user32 is None:
        return False
    if user32 is None:
        import ctypes

        user32 = ctypes.windll.user32
    redraw = getattr(user32, "RedrawWindow", None)
    if redraw is None:
        return False
    hwnd = user32.FindWindowW(None, "HLS Downloader")
    if not hwnd:
        return False
    redraw(hwnd, None, None, 0x0001 | 0x0100 | 0x0080)
    return True


def _activate_windows_window(
    _window=None,
    user32=None,
    *,
    release_delay: float = 2.5,
    timer_factory=threading.Timer,
) -> bool:
    if os.name != "nt" and user32 is None:
        return False
    if user32 is None:
        import ctypes

        user32 = ctypes.windll.user32
    hwnd = user32.FindWindowW(None, "HLS Downloader")
    if not hwnd:
        return False
    sw_restore = 9
    hwnd_topmost = -1
    hwnd_notopmost = -2
    flags = 0x0001 | 0x0002 | 0x0040
    user32.ShowWindow(hwnd, sw_restore)
    user32.SetWindowPos(hwnd, hwnd_topmost, 0, 0, 0, 0, flags)
    bring_to_top = getattr(user32, "BringWindowToTop", None)
    if bring_to_top is not None:
        bring_to_top(hwnd)
    user32.SetForegroundWindow(hwnd)
    # Force WebView2 and its child HWND to repaint after a tray hide/show cycle.
    _redraw_windows_window(user32)

    # Keep the restored manager briefly above other applications while Windows
    # transfers foreground ownership back from the browser.
    def release_topmost() -> None:
        try:
            user32.SetWindowPos(hwnd, hwnd_notopmost, 0, 0, 0, 0, flags)
        except Exception:
            logger.exception("failed to release temporary topmost window state")

    global _attention_timer
    timer = timer_factory(release_delay, release_topmost)
    try:
        timer.daemon = True
    except Exception:
        pass
    with _attention_timer_lock:
        if _attention_timer is not None:
            _attention_timer.cancel()
        _attention_timer = timer
        timer.start()
    return True


class DesktopTray:
    def __init__(self, on_open, on_exit) -> None:
        self._on_open = on_open
        self._on_exit = on_exit
        self._icon = None

    def start(self) -> None:
        import pystray

        menu = pystray.Menu(
            pystray.MenuItem(
                "打开 HLS Downloader",
                lambda _icon, _item: self._on_open(),
                default=True,
            ),
            pystray.MenuItem("退出", lambda _icon, _item: self._on_exit()),
        )
        self._icon = pystray.Icon(
            "HLSDownloader",
            _create_tray_image(),
            "HLS Downloader",
            menu,
        )
        self._icon.run_detached()

    def stop(self) -> None:
        icon = self._icon
        self._icon = None
        if icon is not None:
            icon.stop()


class DesktopController:
    def __init__(
        self,
        window,
        server,
        tray=None,
        shutdown_timeout: float = 20,
        force_exit=None,
        force_exit_delay: float = 3,
        native_window_activator=None,
        handoff_windows=None,
        deferred_ui: bool = False,
    ) -> None:
        self.window = window
        self.server = server
        self.tray = tray
        self.shutdown_timeout = shutdown_timeout
        self.force_exit = force_exit
        self.force_exit_delay = force_exit_delay
        self.native_window_activator = native_window_activator or _activate_windows_window
        self.handoff_windows = handoff_windows
        self._deferred_ui = deferred_ui
        self._allow_close = False
        self._shutdown_started = False
        self._shutdown_done = threading.Event()
        self._state_lock = threading.Lock()
        self._force_exit_timer = None

    def on_closing(self) -> bool:
        with self._state_lock:
            if self._allow_close:
                return True
            if self._shutdown_started:
                return False
        self.window.hide()
        return False

    def set_tray(self, tray) -> None:
        self.tray = tray

    def request_exit(self) -> bool:
        tray = None
        with self._state_lock:
            if self._shutdown_started:
                return False
            self._shutdown_started = True
            tray = self.tray
            if self.force_exit is not None:
                self._force_exit_timer = threading.Timer(
                    self.force_exit_delay,
                    self.force_exit,
                    args=(0,),
                )
                self._force_exit_timer.daemon = True
                self._force_exit_timer.start()
        if tray is not None:
            try:
                tray.stop()
            except Exception:
                logger.exception("failed to stop tray icon during shutdown")
        if self.handoff_windows is not None:
            try:
                self.handoff_windows.close_all()
            except Exception:
                logger.exception("failed to close browser handoff windows during shutdown")
        try:
            self.window.hide()
        except Exception:
            logger.exception("failed to hide desktop window during shutdown")
        threading.Thread(target=self._shutdown_then_destroy, daemon=True).start()
        return True

    def wait_for_shutdown(self, timeout: float | None = None) -> bool:
        return self._shutdown_done.wait(timeout)

    def _refresh_surface(self) -> None:
        try:
            _redraw_windows_window()
        except Exception:
            logger.exception("failed to redraw desktop window")
        try:
            self.window.run_js(
                "window.dispatchEvent(new Event('desktop-activated'));"
                "requestAnimationFrame(() => document.body && document.body.getBoundingClientRect());"
            )
        except Exception:
            # The DOM may still be starting during a cold launch.
            pass

    def on_restored(self, *_args) -> None:
        self._refresh_surface()

    def activate(self) -> None:
        with self._state_lock:
            load_manager_ui = self._deferred_ui
            self._deferred_ui = False
        if load_manager_ui:
            try:
                self.window.load_url(desktop_ui_url())
            except Exception:
                logger.exception("failed to load deferred desktop UI")

        # Keep pywebview's own visibility state in sync. Calling ShowWindow only
        # on the native HWND leaves EdgeChromium thinking the host is hidden and
        # is a common source of a black/stale surface after restoring from tray.
        try:
            self.window.restore()
            self.window.show()
        except Exception:
            logger.exception("failed to restore desktop window")

        native_activated = self.native_window_activator(self.window)
        if not native_activated and not (os.name == "nt" and self.native_window_activator is _activate_windows_window):
            try:
                self.window.on_top = True
                self.window.on_top = False
            except Exception:
                logger.exception("failed to bring desktop window to foreground")
        self._refresh_surface()

    def _shutdown_then_destroy(self) -> None:
        try:
            self.server.stop()
            self.server.join(timeout=self.shutdown_timeout)
            with self._state_lock:
                self._allow_close = True
            self.window.destroy()
        finally:
            self._shutdown_done.set()


class StartupExitController:
    """Accept exit requests while the desktop window is still being created."""

    def __init__(self, server, force_exit=None, force_exit_delay: float = 3) -> None:
        self.server = server
        self.force_exit = force_exit
        self.force_exit_delay = force_exit_delay
        self._active = True
        self._exit_requested = False
        self._state_lock = threading.Lock()

    def request_exit(self) -> bool:
        with self._state_lock:
            if not self._active or self._exit_requested:
                return False
            self._exit_requested = True
            if self.force_exit is not None:
                timer = threading.Timer(
                    self.force_exit_delay,
                    self.force_exit,
                    args=(0,),
                )
                timer.daemon = True
                timer.start()
        self.server.stop()
        return True

    def disarm(self) -> None:
        with self._state_lock:
            self._active = False


class SingleInstanceLock:
    ERROR_ALREADY_EXISTS = 183

    def __init__(self, name: str = "Local\\HLSDownloader.ciaooo55", kernel32=None) -> None:
        self.name = name
        self.kernel32 = kernel32
        self.handle = None

    def acquire(self) -> bool:
        if os.name != "nt" and self.kernel32 is None:
            return True
        if self.kernel32 is None:
            import ctypes

            self.kernel32 = ctypes.windll.kernel32
        handle = self.kernel32.CreateMutexW(None, False, self.name)
        if not handle:
            raise OSError("无法创建单实例锁")
        if self.kernel32.GetLastError() == self.ERROR_ALREADY_EXISTS:
            self.kernel32.CloseHandle(handle)
            return False
        self.handle = handle
        return True

    def release(self) -> None:
        if self.handle is not None:
            self.kernel32.CloseHandle(self.handle)
            self.handle = None


def _activate_existing_with_retry(timeout: float = 8) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if activate_existing_instance():
            return True
        time.sleep(0.2)
    return False


def _show_startup_error(message: str) -> None:
    try:
        import ctypes

        ctypes.windll.user32.MessageBoxW(0, message, "HLS Downloader 启动失败", 0x10)
    except Exception:
        print(message, flush=True)


def _run_desktop(*, start_hidden: bool = False) -> int:
    if not start_hidden and activate_existing_instance():
        return 0

    server = UvicornServerThread()
    server.start()
    if not server.wait_until_ready():
        server.stop()
        server.join(timeout=5)
        _show_startup_error(
            f"本地服务无法启动。请检查端口 {settings.port} 是否被其他程序占用。"
        )
        return 1

    startup_exit = StartupExitController(server, force_exit=os._exit)
    register_shutdown(startup_exit.request_exit)
    try:
        import webview
    except ImportError:
        register_shutdown(None)
        server.stop()
        server.join(timeout=5)
        _show_startup_error("桌面组件未安装，请重新安装 HLS Downloader。")
        return 1

    bridge = DesktopBridge(folder_dialog_type=webview.FileDialog.FOLDER)
    window = webview.create_window(
        "HLS Downloader",
        desktop_host_url() if start_hidden else desktop_ui_url(),
        js_api=bridge,
        width=1180,
        height=760,
        min_size=(900, 600),
        background_color="#17191d",
        text_select=True,
        hidden=start_hidden,
    )
    bridge._set_window(window)
    set_desktop_handoff_session(True)
    handoff_windows = BrowserHandoffWindowManager(
        window,
        webview.create_window,
        webview.FileDialog.FOLDER,
        mark_presented=_mark_browser_handoff_presented,
    )
    controller = DesktopController(
        window,
        server,
        force_exit=os._exit,
        handoff_windows=handoff_windows,
        deferred_ui=start_hidden,
    )
    tray = DesktopTray(controller.activate, controller.request_exit)
    controller.set_tray(tray)
    bridge._set_exit_request(controller.request_exit)
    register_activation(controller.activate)
    register_browser_handoff(handoff_windows.show)
    register_shutdown(controller.request_exit)
    startup_exit.disarm()
    window.events.closing += controller.on_closing
    window.events.restored += controller.on_restored

    try:
        storage_path = RUNTIME_PATHS.webview_path
        storage_path.mkdir(parents=True, exist_ok=True)
        webview.start(
            tray.start,
            gui="edgechromium",
            private_mode=False,
            storage_path=str(storage_path),
        )
    finally:
        register_activation(None)
        register_browser_handoff(None)
        set_desktop_handoff_session(False)
        register_shutdown(None)
        handoff_windows.close_all()
        if not controller.wait_for_shutdown(timeout=0):
            controller.request_exit()
            controller.wait_for_shutdown(timeout=20)
    return 0


def main() -> int:
    args = set(sys.argv[1:])
    if "--shutdown" in args:
        shutdown_existing_instance()
        return 0

    start_hidden = "--background" in args or "--native-host" in args
    instance_lock = SingleInstanceLock()
    if not instance_lock.acquire():
        if not start_hidden:
            _activate_existing_with_retry()
        return 0
    try:
        return _run_desktop(start_hidden=start_hidden)
    finally:
        instance_lock.release()
