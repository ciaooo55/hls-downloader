import json
import threading
import time
import urllib.error
import urllib.request
import webbrowser
from pathlib import Path

import uvicorn

try:
    from .app.config import PROJECT_ROOT, settings
    from .app.desktop_runtime import register_activation
    from .app.main import app
    from .app.userscript_service import USERSCRIPT_FILENAME, export_userscript, render_userscript
except ImportError:
    from app.config import PROJECT_ROOT, settings
    from app.desktop_runtime import register_activation
    from app.main import app
    from app.userscript_service import USERSCRIPT_FILENAME, export_userscript, render_userscript


def public_base_url() -> str:
    host = settings.host
    if host in {"0.0.0.0", "::"}:
        host = "127.0.0.1"
    return f"http://{host}:{settings.port}"


class DesktopBridge:
    def __init__(
        self,
        window=None,
        folder_dialog_type=None,
        source_path: Path | None = None,
        url_opener=None,
    ) -> None:
        self._window = window
        self._folder_dialog_type = folder_dialog_type
        self._source_path = source_path
        self._url_opener = url_opener or webbrowser.open

    def _set_window(self, window) -> None:
        self._window = window

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
        self._thread = threading.Thread(target=self._server.run, name="hls-api")

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


def activate_existing_instance() -> bool:
    try:
        request = urllib.request.Request(
            f"{public_base_url()}/api/app/activate",
            data=b"{}",
            headers={"Content-Type": "application/json", "X-Token": settings.token},
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=2) as response:
            body = json.loads(response.read().decode("utf-8"))
            return response.status == 200 and body.get("ok") is True
    except (OSError, ValueError, urllib.error.URLError):
        return False


class DesktopController:
    def __init__(self, window, server, shutdown_timeout: float = 20) -> None:
        self.window = window
        self.server = server
        self.shutdown_timeout = shutdown_timeout
        self._allow_close = False
        self._shutdown_started = False
        self._shutdown_done = threading.Event()
        self._state_lock = threading.Lock()

    def on_closing(self) -> bool:
        with self._state_lock:
            if self._allow_close:
                return True
            if self._shutdown_started:
                return False

        confirmed = self.window.create_confirmation_dialog(
            "退出下载器",
            "将停止当前任务并退出，确定继续吗？",
        )
        if not confirmed:
            return False

        with self._state_lock:
            if self._shutdown_started:
                return False
            self._shutdown_started = True
        threading.Thread(target=self._shutdown_then_destroy, daemon=True).start()
        return False

    def wait_for_shutdown(self, timeout: float | None = None) -> bool:
        return self._shutdown_done.wait(timeout)

    def activate(self) -> None:
        self.window.restore()
        self.window.show()

    def _shutdown_then_destroy(self) -> None:
        try:
            self.server.stop()
            self.server.join(timeout=self.shutdown_timeout)
            with self._state_lock:
                self._allow_close = True
            self.window.destroy()
        finally:
            self._shutdown_done.set()


def _show_startup_error(message: str) -> None:
    try:
        import ctypes

        ctypes.windll.user32.MessageBoxW(0, message, "HLS Downloader 启动失败", 0x10)
    except Exception:
        print(message, flush=True)


def main() -> int:
    if activate_existing_instance():
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

    try:
        import webview
    except ImportError:
        server.stop()
        server.join(timeout=5)
        _show_startup_error("桌面组件未安装，请重新安装 HLS Downloader。")
        return 1

    bridge = DesktopBridge(folder_dialog_type=webview.FileDialog.FOLDER)
    window = webview.create_window(
        "HLS Downloader",
        f"{public_base_url()}/ui",
        js_api=bridge,
        width=1180,
        height=760,
        min_size=(900, 600),
        background_color="#17191d",
        text_select=True,
    )
    bridge._set_window(window)
    controller = DesktopController(window, server)
    register_activation(controller.activate)
    window.events.closing += controller.on_closing

    try:
        storage_path = Path(PROJECT_ROOT) / ".webview"
        webview.start(
            gui="edgechromium",
            private_mode=False,
            storage_path=str(storage_path),
        )
    finally:
        register_activation(None)
        if not controller.wait_for_shutdown(timeout=0):
            server.stop()
            server.join(timeout=20)
    return 0
