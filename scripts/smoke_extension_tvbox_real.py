#!/usr/bin/env python3
"""Drive the production browser -> Native Host -> Compose -> TVBox path.

This smoke intentionally does not issue Core share_media/cast commands. Core TCP is
used only to discover and preselect the expected receiver. The actual media-push
request originates from the production extension and the final confirmation is
invoked through the packaged Compose accessibility surface.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import hashlib
import http.server
import json
import socket
import subprocess
import tempfile
import threading
import time
import zipfile
from pathlib import Path
from urllib.parse import urlparse

from selenium import webdriver
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService

from smoke_extension_media import (
    QuietStaticHandler,
    _find_edge,
    _find_firefox,
    _make_media,
    _open_first_media_actions,
    _overlay_state,
)
from smoke_v7_tvbox_real import local_lan_ip, send_frame, wait_core


class ReceiverAwareHandler(QuietStaticHandler):
    expected_host = ""
    requests: list[dict[str, object]] = []
    receiver_fetched = threading.Event()

    def do_GET(self) -> None:  # noqa: N802
        started = time.perf_counter()
        super().do_GET()
        record = {
            "client": self.client_address[0],
            "method": "GET",
            "path": self.path,
            "range": self.headers.get("Range", ""),
            "elapsed_ms": round((time.perf_counter() - started) * 1000, 2),
        }
        type(self).requests.append(record)
        if self.client_address[0] == type(self).expected_host and self.path.split("?", 1)[0] == "/stream.mp4":
            type(self).receiver_fetched.set()


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def discover_and_preselect(core_port: int, expected_host: str) -> dict[str, object]:
    stream = wait_core(core_port, timeout=30.0)
    try:
        discovery = send_frame(
            stream,
            {
                "type": "command",
                "request_id": 101,
                "command": {"kind": "discover_cast_devices", "mode": "tvbox"},
            },
        )
        devices: list[dict[str, object]] = []
        for envelope in discovery.get("events", []):
            if not isinstance(envelope, dict):
                continue
            event = envelope.get("event")
            if isinstance(event, dict) and event.get("kind") == "cast_devices":
                raw = event.get("devices", [])
                if isinstance(raw, list):
                    devices = [item for item in raw if isinstance(item, dict)]
        candidates = [
            item
            for item in devices
            if urlparse(str(item.get("location", ""))).hostname == expected_host
        ]
        if len(candidates) != 1:
            raise RuntimeError(
                f"Expected exactly one TVBox receiver at {expected_host}; discovered={devices}"
            )
        device = candidates[0]
        settings = send_frame(
            stream,
            {
                "type": "store_settings",
                "request_id": 102,
                "values": {"preferred_cast_device_id": str(device.get("id", ""))},
            },
        )
        if settings.get("type") != "settings":
            raise RuntimeError(f"Core did not persist preferred TVBox receiver: {settings}")
        return device
    finally:
        stream.close()


def click_tvbox_action(driver) -> bool:
    return bool(
        driver.execute_script(
            """
            const roots=[]; const seen=new Set();
            const visit=(root) => {
              if (!root || seen.has(root)) return; seen.add(root); roots.push(root);
              root.querySelectorAll('*').forEach(element => { if (element.shadowRoot) visit(element.shadowRoot); });
            };
            visit(document);
            const button=roots.flatMap(root => [...root.querySelectorAll('.item-actions button')])
              .find(value => value.innerText.trim() === 'TVBox');
            if (!button) return false;
            button.click();
            return true;
            """
        )
    )


def tvbox_ui_state(driver) -> dict[str, object]:
    return driver.execute_script(
        """
        const roots=[]; const seen=new Set();
        const visit=(root) => {
          if (!root || seen.has(root)) return; seen.add(root); roots.push(root);
          root.querySelectorAll('*').forEach(element => { if (element.shadowRoot) visit(element.shadowRoot); });
        };
        visit(document);
        const buttons=roots.flatMap(root => [...root.querySelectorAll('.item-actions button')]).map(value => value.innerText.trim());
        const results=roots.flatMap(root => [...root.querySelectorAll('.result')]).map(value => value.innerText.trim()).filter(Boolean);
        return {buttons,results};
        """
    )


def unpack_firefox_addon(extension: Path, root: Path) -> Path:
    addon = root / "hls-downloader-tvbox-smoke.xpi"
    with zipfile.ZipFile(addon, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(extension.rglob("*")):
            if path.is_file():
                archive.write(path, path.relative_to(extension).as_posix())
    return addon


def launch_browser(
    *,
    browser: str,
    extension: Path,
    profile: Path,
    browser_binary: Path,
    driver_path: Path | None,
    addon: Path | None,
):
    if browser == "edge":
        options = EdgeOptions()
        options.binary_location = str(browser_binary)
        options.add_argument(f"--user-data-dir={profile}")
        options.add_argument("--disable-features=DisableLoadExtensionCommandLineSwitch")
        options.add_argument(f"--disable-extensions-except={extension}")
        options.add_argument(f"--load-extension={extension}")
        options.add_argument("--autoplay-policy=no-user-gesture-required")
        options.add_argument("--no-first-run")
        options.add_argument("--disable-default-apps")
        options.add_argument("--window-size=1280,800")
        service = EdgeService(executable_path=str(driver_path)) if driver_path else EdgeService()
        return webdriver.Edge(service=service, options=options)

    options = FirefoxOptions()
    options.binary_location = str(browser_binary)
    options.set_preference("media.autoplay.default", 0)
    options.set_preference("media.autoplay.blocking_policy", 0)
    options.set_preference("browser.shell.checkDefaultBrowser", False)
    service = FirefoxService(executable_path=str(driver_path)) if driver_path else FirefoxService()
    driver = webdriver.Firefox(service=service, options=options)
    if addon is None:
        raise RuntimeError("Firefox TVBox smoke requires an addon archive")
    driver.install_addon(str(addon), temporary=True)
    return driver


def run_browser(
    *,
    browser: str,
    extension: Path,
    core_port: int,
    expected_host: str,
    ffmpeg: str,
    access_bridge_python: Path,
    access_bridge_dll: Path,
    browser_binary: Path | None,
    driver_path: Path | None,
    output: Path,
) -> dict[str, object]:
    started = time.perf_counter()
    expected_device = discover_and_preselect(core_port, expected_host)
    lan_ip = local_lan_ip(expected_host)
    resolved_browser_binary = (browser_binary or (_find_edge() if browser == "edge" else _find_firefox())).resolve()
    if not resolved_browser_binary.is_file():
        raise FileNotFoundError(f"{browser}: browser executable is missing: {resolved_browser_binary}")
    with tempfile.TemporaryDirectory(prefix=f"hls-{browser}-tvbox-real-") as temporary:
        root = Path(temporary)
        media_root = root / "site"
        profile = root / "profile"
        media_root.mkdir()
        _make_media(media_root, ffmpeg)
        stream_path = media_root / "stream.mp4"
        ReceiverAwareHandler.expected_host = expected_host
        ReceiverAwareHandler.requests = []
        ReceiverAwareHandler.receiver_fetched = threading.Event()
        handler = functools.partial(ReceiverAwareHandler, directory=str(media_root))
        server = http.server.ThreadingHTTPServer(("0.0.0.0", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        addon = unpack_firefox_addon(extension, root) if browser == "firefox" else None
        driver = None
        access_report = root / "access-bridge.json"
        try:
            driver = launch_browser(
                browser=browser,
                extension=extension,
                profile=profile,
                browser_binary=resolved_browser_binary,
                driver_path=driver_path,
                addon=addon,
            )
            capabilities = driver.capabilities or {}
            service_path = Path(str(getattr(driver.service, "path", ""))) if getattr(driver, "service", None) else None
            browser_identity = {
                "executable": str(resolved_browser_binary),
                "executable_sha256": file_sha256(resolved_browser_binary),
                "version": str(capabilities.get("browserVersion", "")),
                "driver_executable": str(service_path) if service_path else "",
                "driver_sha256": file_sha256(service_path) if service_path and service_path.is_file() else "",
                "driver_version": str(
                    capabilities.get("moz:geckodriverVersion", "")
                    or (capabilities.get("msedge", {}) or {}).get("msedgedriverVersion", "")
                ),
            }
            if not browser_identity["version"]:
                raise RuntimeError(f"{browser}: Selenium did not report browserVersion")
            origin = f"http://{lan_ip}:{server.server_port}"
            driver.get(f"{origin}/index.html?mode=direct")
            deadline = time.monotonic() + 30.0
            state: dict[str, object] = {}
            while time.monotonic() < deadline:
                state = _overlay_state(driver)
                if state.get("marker") == "1" and "下载视频" in state.get("labels", []):
                    break
                time.sleep(0.2)
            if state.get("marker") != "1" or "下载视频" not in state.get("labels", []):
                raise RuntimeError(f"{browser}: production content overlay did not become ready: {state}")
            if not _open_first_media_actions(driver):
                raise RuntimeError(f"{browser}: media action panel did not open")
            action_deadline = time.monotonic() + 5.0
            while time.monotonic() < action_deadline:
                state = _overlay_state(driver)
                if "TVBox" in state.get("panelActions", []):
                    break
                time.sleep(0.1)
            if "TVBox" not in state.get("panelActions", []):
                raise RuntimeError(f"{browser}: TVBox production action is missing: {state}")
            if not click_tvbox_action(driver):
                raise RuntimeError(f"{browser}: TVBox production action could not be clicked")

            access = subprocess.run(
                [
                    str(access_bridge_python),
                    str(Path(__file__).resolve().parent / "smoke_v7_accessibility.py"),
                    "--dll",
                    str(access_bridge_dll),
                    "--title",
                    "HLS Downloader",
                    "--timeout",
                    "60",
                    "--min-nodes",
                    "10",
                    "--require-name",
                    "确认推送",
                    "--invoke-name",
                    "确认推送",
                    "--output",
                    str(access_report),
                ],
                text=True,
                capture_output=True,
                timeout=90,
            )
            if access.returncode != 0:
                raise RuntimeError(
                    f"{browser}: Compose accessibility confirmation failed: stdout={access.stdout} stderr={access.stderr}"
                )

            ui_deadline = time.monotonic() + 135.0
            ui_state: dict[str, object] = {}
            while time.monotonic() < ui_deadline:
                ui_state = tvbox_ui_state(driver)
                if "已发送" in ui_state.get("buttons", []):
                    break
                if any("失败" in str(value) or "取消" in str(value) for value in ui_state.get("results", [])):
                    raise RuntimeError(f"{browser}: browser reported failed TVBox resolution: {ui_state}")
                time.sleep(0.5)
            if "已发送" not in ui_state.get("buttons", []):
                raise RuntimeError(f"{browser}: browser did not observe completed TVBox request: {ui_state}")
            if not ReceiverAwareHandler.receiver_fetched.wait(30.0):
                raise RuntimeError(
                    f"{browser}: selected TVBox {expected_host} did not fetch the media; requests={ReceiverAwareHandler.requests}"
                )
            matching = [item for item in ReceiverAwareHandler.requests if item.get("client") == expected_host]
            if not matching:
                raise RuntimeError(
                    f"{browser}: media was fetched, but not by selected receiver {expected_host}: {ReceiverAwareHandler.requests}"
                )
            report = {
                "schema": 1,
                "passed": True,
                "browser": browser,
                "browser_identity": browser_identity,
                "expected_receiver_host": expected_host,
                "selected_device": expected_device,
                "fixture_url": f"{origin}/stream.mp4?player=direct",
                "fixture_sha256": file_sha256(stream_path),
                "browser_ui": ui_state,
                "receiver_requests": matching,
                "all_requests": ReceiverAwareHandler.requests,
                "access_bridge": json.loads(access_report.read_text(encoding="utf-8")),
                "elapsed_ms": round((time.perf_counter() - started) * 1000, 2),
            }
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            print(json.dumps(report, ensure_ascii=False, separators=(",", ":")))
            return report
        finally:
            if driver is not None:
                with contextlib.suppress(Exception):
                    driver.quit()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", choices=("edge", "firefox"), required=True)
    parser.add_argument("--extension", required=True, type=Path)
    parser.add_argument("--core-port", required=True, type=int)
    parser.add_argument("--expected-host", required=True)
    parser.add_argument("--ffmpeg", required=True)
    parser.add_argument("--access-bridge-python", required=True, type=Path)
    parser.add_argument("--access-bridge-dll", required=True, type=Path)
    parser.add_argument("--browser-binary", type=Path)
    parser.add_argument("--driver", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not args.extension.is_dir():
        raise FileNotFoundError(f"candidate extension directory is missing: {args.extension}")
    if not args.access_bridge_dll.is_file():
        raise FileNotFoundError(f"packaged Windows Access Bridge DLL is missing: {args.access_bridge_dll}")
    run_browser(
        browser=args.browser,
        extension=args.extension.resolve(),
        core_port=args.core_port,
        expected_host=args.expected_host,
        ffmpeg=args.ffmpeg,
        access_bridge_python=args.access_bridge_python.resolve(),
        access_bridge_dll=args.access_bridge_dll.resolve(),
        browser_binary=args.browser_binary.resolve() if args.browser_binary else None,
        driver_path=args.driver.resolve() if args.driver else None,
        output=args.output.resolve(),
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
