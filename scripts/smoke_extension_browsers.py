"""Load the production extension builds in real Chrome and Firefox.

This smoke test verifies more than TypeScript/Vitest: each browser installs the
actual WXT output, opens a private loopback page, and waits for the production
content script to mark the document.  It never touches a user's browser
profile, cookies, extensions, or native-host registration.
"""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from urllib.request import urlopen
import zipfile

import websocket
from selenium import webdriver
from selenium.common.exceptions import WebDriverException
from selenium.webdriver.firefox.options import Options as FirefoxOptions


MARKER = "data-hls-downloader-extension"
OVERLAY_EXPRESSION = """
(() => {
  const video = document.querySelector('video');
  if (!video) return false;
  if (!window.__hlsOverlaySmokeStarted) {
    window.__hlsOverlaySmokeStarted = true;
    // A page-world synthetic Event does not reliably cross Firefox's
    // extension-world boundary.  Exercise a real browser playback transition
    // instead: a canvas MediaStream needs no codec, network, or fixture file,
    // and emits the same trusted media lifecycle events as a site player.
    const canvas = document.createElement('canvas');
    canvas.width = 320;
    canvas.height = 180;
    const context = canvas.getContext('2d');
    context?.fillRect(0, 0, canvas.width, canvas.height);
    const stream = canvas.captureStream(5);
    window.__hlsOverlaySmokeStream = stream;
    video.muted = true;
    video.srcObject = stream;
    void video.play().catch(error => { window.__hlsOverlaySmokeError = String(error); });
  }
  return [...document.querySelectorAll('*')].some(element => {
    const button = element.shadowRoot?.querySelector('.video-download.identifying');
    return button?.textContent?.includes('正在识别') === true;
  });
})()
"""


class _PageHandler(BaseHTTPRequestHandler):
    def log_message(self, _format: str, *_args) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if self.path != "/page":
            self.send_error(404)
            return
        payload = (
            b"<!doctype html><html><head><meta charset=utf-8><title>Extension smoke</title></head>"
            b"<body><video controls width=320 height=180></video><a href=/sample.mp4>sample</a></body></html>"
        )
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


@contextmanager
def _loopback_page():
    server = ThreadingHTTPServer(("127.0.0.1", 0), _PageHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}/page"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)


def _wait_for_content_script(driver: webdriver.Remote, browser_name: str) -> None:
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            marker = driver.execute_script(
                "return document.documentElement.getAttribute(arguments[0])", MARKER
            )
            if marker == "1":
                return
        except WebDriverException:
            pass
        time.sleep(0.1)
    raise RuntimeError(f"{browser_name} loaded the page but the production content script did not run")


def _wait_for_identifying_overlay(driver: webdriver.Remote, browser_name: str) -> None:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            if driver.execute_script(f"return {OVERLAY_EXPRESSION}"):
                return
        except WebDriverException:
            pass
        time.sleep(0.1)
    diagnostics = driver.execute_script(
        """
        const video = document.querySelector('video');
        return {
          marker: document.documentElement.getAttribute(arguments[0]),
          paused: video?.paused,
          readyState: video?.readyState,
          playbackError: window.__hlsOverlaySmokeError || '',
          shadowHosts: [...document.querySelectorAll('*')].filter(element => element.shadowRoot).length,
          overlayLabels: [...document.querySelectorAll('*')].flatMap(element =>
            [...(element.shadowRoot?.querySelectorAll('.video-download') || [])].map(button => button.textContent)
          ),
        };
        """,
        MARKER,
    )
    raise RuntimeError(
        f"{browser_name} did not show the immediate identifying overlay: {diagnostics}"
    )


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def _find_chromium_binary(configured: str | None) -> Path:
    candidates = [
        configured,
        shutil.which("chrome"),
        shutil.which("chrome.exe"),
        shutil.which("msedge"),
        shutil.which("msedge.exe"),
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return Path(candidate).resolve()
    raise RuntimeError("Chrome/Edge binary not found; pass --chrome-binary")


def _read_debug_targets(port: int) -> list[dict]:
    with urlopen(f"http://127.0.0.1:{port}/json/list", timeout=1) as response:
        payload = json.loads(response.read())
    return payload if isinstance(payload, list) else []


def _evaluate(websocket_url: str, expression: str) -> object:
    connection = websocket.create_connection(websocket_url, timeout=2, suppress_origin=True)
    try:
        connection.send(json.dumps({
            "id": 1,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expression,
                "returnByValue": True,
            },
        }))
        while True:
            message = json.loads(connection.recv())
            if message.get("id") == 1:
                return message.get("result", {}).get("result", {}).get("value")
    finally:
        connection.close()


def _exercise_chrome(extension_dir: Path, page_url: str, binary: str | None, temp_root: Path) -> None:
    browser = _find_chromium_binary(binary)
    port = _free_port()
    profile = temp_root / "chromium-profile"
    command = [
        str(browser),
        "--headless=new",
        f"--remote-debugging-port={port}",
        "--remote-allow-origins=*",
        f"--user-data-dir={profile}",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-background-networking",
        f"--disable-extensions-except={extension_dir}",
        f"--load-extension={extension_dir}",
        page_url,
    ]
    process = subprocess.Popen(
        command,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    try:
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"Chromium exited before extension verification ({process.returncode})")
            try:
                targets = _read_debug_targets(port)
                page = next((item for item in targets if item.get("type") == "page" and item.get("url") == page_url), None)
                if page:
                    websocket_url = str(page["webSocketDebuggerUrl"])
                    marker = _evaluate(
                        websocket_url,
                        f"document.documentElement.getAttribute('{MARKER}')",
                    )
                    if marker == "1" and _evaluate(websocket_url, OVERLAY_EXPRESSION) is True:
                        return
            except (OSError, ValueError, KeyError, websocket.WebSocketException):
                pass
            time.sleep(0.15)
        raise RuntimeError("Chromium content script loaded but the immediate identifying overlay did not appear")
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def _zip_firefox_extension(extension_dir: Path, destination: Path) -> None:
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(extension_dir.rglob("*")):
            if path.is_file():
                archive.write(path, path.relative_to(extension_dir).as_posix())


def _exercise_firefox(extension_dir: Path, page_url: str, binary: str | None, temp_root: Path) -> None:
    addon = temp_root / "hls-downloader-smoke.xpi"
    _zip_firefox_extension(extension_dir, addon)
    options = FirefoxOptions()
    options.add_argument("-headless")
    options.set_preference("datareporting.policy.dataSubmissionEnabled", False)
    options.set_preference("browser.shell.checkDefaultBrowser", False)
    options.set_preference("browser.startup.homepage_override.mstone", "ignore")
    if binary:
        options.binary_location = binary
    with webdriver.Firefox(options=options) as driver:
        driver.install_addon(str(addon), temporary=True)
        driver.set_page_load_timeout(20)
        driver.get(page_url)
        _wait_for_content_script(driver, "Firefox")
        _wait_for_identifying_overlay(driver, "Firefox")


def _require_build(path: Path, browser_name: str) -> Path:
    resolved = path.resolve()
    if not (resolved / "manifest.json").is_file():
        raise RuntimeError(f"missing {browser_name} production extension build: {resolved}")
    return resolved


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--extension-output", type=Path, required=True)
    parser.add_argument("--browser", choices=("both", "chrome", "firefox"), default="both")
    parser.add_argument("--chrome-binary")
    parser.add_argument("--firefox-binary")
    args = parser.parse_args()

    output = args.extension_output.resolve()
    chrome = _require_build(output / "chrome-mv3", "Chrome")
    firefox = _require_build(output / "firefox-mv3", "Firefox")
    with tempfile.TemporaryDirectory(prefix="hls-downloader-extension-smoke-") as temp_dir:
        temp_root = Path(temp_dir)
        try:
            with _loopback_page() as page_url:
                if args.browser in {"both", "chrome"}:
                    print("Loading the production Chromium extension...", flush=True)
                    _exercise_chrome(chrome, page_url, args.chrome_binary, temp_root)
                if args.browser in {"both", "firefox"}:
                    print("Loading the production Firefox extension...", flush=True)
                    _exercise_firefox(firefox, page_url, args.firefox_binary, temp_root)
        finally:
            # Selenium normally removes profiles itself. This also handles a
            # driver crash without ever targeting a real browser profile.
            shutil.rmtree(temp_root, ignore_errors=True)

    print(f"Production extension browser smoke passed: {args.browser}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
