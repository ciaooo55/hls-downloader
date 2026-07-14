import json
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import webbrowser
from pathlib import Path

import uvicorn

try:
    from .app.config import PROJECT_ROOT, settings
    from .app.paths import RUNTIME_PATHS
    from .app.desktop_runtime import register_activation, register_shutdown
    from .app.main import app
    from .app.userscript_service import USERSCRIPT_FILENAME, export_userscript, render_userscript
except ImportError:
    from app.config import PROJECT_ROOT, settings
    from app.paths import RUNTIME_PATHS
    from app.desktop_runtime import register_activation, register_shutdown
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
        uninstaller_path: Path | None = None,
        process_starter=None,
        exit_request=None,
    ) -> None:
        self._window = window
        self._folder_dialog_type = folder_dialog_type
        self._source_path = source_path
        self._url_opener = url_opener or webbrowser.open
        self._uninstaller_path = uninstaller_path or Path(PROJECT_ROOT) / "Uninstall.exe"
        self._process_starter = process_starter or subprocess.Popen
        self._exit_request = exit_request

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

    def get_desktop_info(self) -> dict:
        installed = self._uninstaller_path.is_file()
        mode = "installed" if installed else RUNTIME_PATHS.mode
        return {"ok": True, "installed": installed, "mode": mode}

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

    image = Image.new("RGBA", (64, 64), (23, 25, 29, 255))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((8, 8, 56, 56), radius=10, fill=(39, 43, 50, 255))
    draw.line((32, 16, 32, 39), fill=(70, 194, 126, 255), width=7)
    draw.polygon(((20, 34), (32, 47), (44, 34)), fill=(70, 194, 126, 255))
    draw.line((18, 50, 46, 50), fill=(235, 238, 242, 255), width=4)
    return image


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
    def __init__(self, window, server, tray=None, shutdown_timeout: float = 20) -> None:
        self.window = window
        self.server = server
        self.tray = tray
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
        self.window.hide()
        return False

    def set_tray(self, tray) -> None:
        self.tray = tray

    def request_exit(self) -> bool:
        with self._state_lock:
            if self._shutdown_started:
                return False
            self._shutdown_started = True
        threading.Thread(target=self._shutdown_then_destroy, daemon=True).start()
        return True

    def wait_for_shutdown(self, timeout: float | None = None) -> bool:
        return self._shutdown_done.wait(timeout)

    def activate(self) -> None:
        self.window.restore()
        self.window.show()

    def _shutdown_then_destroy(self) -> None:
        try:
            self.server.stop()
            self.server.join(timeout=self.shutdown_timeout)
            if self.tray is not None:
                self.tray.stop()
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
    if "--shutdown" in sys.argv[1:]:
        shutdown_existing_instance()
        return 0

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
    tray = DesktopTray(controller.activate, controller.request_exit)
    controller.set_tray(tray)
    bridge._set_exit_request(controller.request_exit)
    register_activation(controller.activate)
    register_shutdown(controller.request_exit)
    window.events.closing += controller.on_closing

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
        register_shutdown(None)
        if not controller.wait_for_shutdown(timeout=0):
            controller.request_exit()
            controller.wait_for_shutdown(timeout=20)
    return 0
