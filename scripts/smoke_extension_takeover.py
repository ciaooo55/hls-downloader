"""Real Chromium smoke for ordinary-download takeover and popup controls.

The smoke uses an isolated browser profile and a temporary Native Messaging
host. It temporarily replaces only the current user's host registration for
the selected Chromium family, restores the exact previous value in ``finally``,
and never opens or modifies a real browser profile.
"""

from __future__ import annotations

import argparse
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
from typing import Any, Callable
from urllib.request import urlopen
import winreg

from selenium import webdriver
from selenium.webdriver.chrome.options import Options as ChromeOptions
from selenium.webdriver.chrome.service import Service as ChromeService
from selenium.webdriver.common.by import By
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
import websocket


HOST_NAME = "com.ciaooo55.hls_downloader"
EXTENSION_ID = "bbdfldcjnikaemnimalegbopgaknjhla"
FILE_SIZE = 8 * 1024 * 1024
FILE_CHUNK = bytes(range(256)) * 128


class _DownloadHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if self.path.split("?", 1)[0] == "/page":
            links = "".join(
                f'<a id="{name}" href="/{name}.bin" download>{name} 下载</a><br>'
                for name in ("disabled", "excluded", "accept", "reject", "disconnect")
            )
            payload = (
                "<!doctype html><html lang=zh-CN><head><meta charset=utf-8>"
                "<title>takeover smoke</title></head><body>"
                f"{links}</body></html>"
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        name = self.path.split("?", 1)[0].removeprefix("/")
        if name not in {
            "disabled.bin", "excluded.bin", "accept.bin", "reject.bin", "disconnect.bin"
        }:
            self.send_error(404)
            return
        start = 0
        end = FILE_SIZE - 1
        range_value = self.headers.get("Range", "")
        if range_value.startswith("bytes="):
            with contextlib.suppress(ValueError):
                start_text, end_text = range_value[6:].split("-", 1)
                start = max(0, int(start_text))
                if end_text:
                    end = min(end, int(end_text))
        if start >= FILE_SIZE or end < start:
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{FILE_SIZE}")
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            return
        self.send_response(206 if range_value.startswith("bytes=") else 200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Disposition", f'attachment; filename="{name}"')
        self.send_header("Content-Length", str(end - start + 1))
        if range_value.startswith("bytes="):
            self.send_header("Content-Range", f"bytes {start}-{end}/{FILE_SIZE}")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        remaining = end - start + 1
        try:
            while remaining > 0:
                chunk = FILE_CHUNK[: min(remaining, len(FILE_CHUNK))]
                self.wfile.write(chunk)
                self.wfile.flush()
                remaining -= len(chunk)
                time.sleep(0.008)
        except (BrokenPipeError, ConnectionResetError):
            pass


def _build_fake_host(root: Path, go: str) -> tuple[Path, Path]:
    log_path = root / "native-host.jsonl"
    source = root / "native-host.go"
    source.write_text(
        f'''package main

import (
  "encoding/binary"
  "encoding/json"
  "io"
  "os"
  "strings"
  "time"
)

var logPath = {json.dumps(str(log_path))}

func logEvent(value map[string]interface{{}}) {{
  value["at"] = time.Now().UnixMilli()
  data, _ := json.Marshal(value)
  file, err := os.OpenFile(logPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
  if err == nil {{
    _, _ = file.Write(append(data, '\\n'))
    _ = file.Close()
  }}
}}

func writeMessage(value map[string]interface{{}}) {{
  data, _ := json.Marshal(value)
  _ = binary.Write(os.Stdout, binary.LittleEndian, uint32(len(data)))
  _, _ = os.Stdout.Write(data)
}}

func main() {{
  statusChecks := map[string]int{{}}
  takeoverEnabled := true
  for {{
    var length uint32
    if err := binary.Read(os.Stdin, binary.LittleEndian, &length); err != nil {{ return }}
    if length == 0 || length > 1024*1024 {{ return }}
    payload := make([]byte, length)
    if _, err := io.ReadFull(os.Stdin, payload); err != nil {{ return }}
    message := map[string]interface{{}}{{}}
    if json.Unmarshal(payload, &message) != nil {{ return }}
    op, _ := message["op"].(string)
    requestID, _ := message["__request_id"].(string)
    response := map[string]interface{{}}{{"ok": true, "__request_id": requestID}}
    event := map[string]interface{{}}{{"op": op}}
    switch op {{
    case "ping":
      response["takeover_enabled"] = takeoverEnabled
      response["takeover_minimum_bytes"] = 0
    case "set_takeover_settings":
      enabled, ok := message["enabled"].(bool)
      if !ok {{ enabled = true }}
      takeoverEnabled = enabled
      response["takeover_enabled"] = enabled
      response["takeover_minimum_bytes"] = message["minimum_bytes"]
      event["enabled"] = enabled
    case "offer":
      resource, _ := message["resource"].(map[string]interface{{}})
      url, _ := resource["url"].(string)
      event["url"] = url
      if strings.Contains(url, "disconnect.bin") {{
        event["disconnect"] = true
        logEvent(event)
        os.Exit(23)
      }}
      decision := "accepted"
      if strings.Contains(url, "reject.bin") {{ decision = "rejected" }}
      id := decision + ":" + url
      response["handoff"] = map[string]interface{{}}{{
        "id": id, "status": "pending", "presentation": "presented",
        "presentation_mode": "desktop", "presentation_ok": true,
      }}
    case "handoff_status":
      id, _ := message["handoff_id"].(string)
      statusChecks[id]++
      status := "pending"
      threshold := 3
      if strings.HasPrefix(id, "rejected:") {{ threshold = 2 }}
      if statusChecks[id] >= threshold {{
        if strings.HasPrefix(id, "rejected:") {{ status = "rejected" }} else {{ status = "accepted" }}
      }}
      response["handoff"] = map[string]interface{{}}{{"id": id, "status": status}}
      event["handoff_id"] = id
      event["status"] = status
    }}
    logEvent(event)
    writeMessage(response)
  }}
}}
''',
        encoding="utf-8",
    )
    executable = root / "HLSDownloaderNativeHostSmoke.exe"
    subprocess.run(
        [go, "build", "-trimpath", "-o", str(executable), str(source)],
        check=True,
        cwd=root,
        timeout=90,
    )
    return executable, log_path


@contextlib.contextmanager
def _registered_host(manifest: Path, browser_family: str):
    vendor = "Google\\Chrome" if browser_family == "chrome" else "Microsoft\\Edge"
    key_path = f"Software\\{vendor}\\NativeMessagingHosts\\{HOST_NAME}"
    existed = True
    old_value: tuple[Any, int] | None = None
    try:
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key_path) as key:
                with contextlib.suppress(FileNotFoundError):
                    old_value = winreg.QueryValueEx(key, "")
        except FileNotFoundError:
            existed = False
        with winreg.CreateKey(winreg.HKEY_CURRENT_USER, key_path) as key:
            winreg.SetValueEx(key, "", 0, winreg.REG_SZ, str(manifest))
        yield
    finally:
        if old_value is not None:
            with winreg.CreateKey(winreg.HKEY_CURRENT_USER, key_path) as key:
                winreg.SetValueEx(key, "", 0, old_value[1], old_value[0])
        elif existed:
            with contextlib.suppress(FileNotFoundError):
                with winreg.OpenKey(
                    winreg.HKEY_CURRENT_USER, key_path, 0, winreg.KEY_SET_VALUE
                ) as key:
                    winreg.DeleteValue(key, "")
        else:
            with contextlib.suppress(FileNotFoundError, OSError):
                winreg.DeleteKey(winreg.HKEY_CURRENT_USER, key_path)


def _wait_until(
    predicate: Callable[[], Any], description: str, timeout: float = 20
) -> Any:
    deadline = time.monotonic() + timeout
    last: Any = None
    while time.monotonic() < deadline:
        last = predicate()
        if last:
            return last
        time.sleep(0.1)
    raise AssertionError(f"等待超时：{description}；最后状态={last!r}")


def _host_events(path: Path) -> list[dict[str, Any]]:
    if not path.is_file():
        return []
    events = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        with contextlib.suppress(json.JSONDecodeError):
            events.append(json.loads(line))
    return events


def _extension_call(driver, script: str, *arguments: object) -> Any:
    return driver.execute_async_script(
        f"const done=arguments[arguments.length-1]; {script}", *arguments
    )


def _debug_targets(driver) -> list[dict[str, Any]]:
    address = str(driver.capabilities.get("ms:edgeOptions", {}).get("debuggerAddress")
                  or driver.capabilities.get("goog:chromeOptions", {}).get("debuggerAddress")
                  or "")
    if not address:
        return []
    with urlopen(f"http://{address}/json/list", timeout=2) as response:
        value = json.loads(response.read())
    return value if isinstance(value, list) else []


def _cdp_evaluate(websocket_url: str, expression: str, await_promise: bool = False) -> Any:
    connection = websocket.create_connection(websocket_url, timeout=5, suppress_origin=True)
    try:
        connection.send(json.dumps({
            "id": 1,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expression,
                "awaitPromise": await_promise,
                "returnByValue": True,
            },
        }))
        while True:
            message = json.loads(connection.recv())
            if message.get("id") != 1:
                continue
            result = message.get("result", {}).get("result", {})
            if result.get("subtype") == "error" or "exceptionDetails" in message.get("result", {}):
                raise RuntimeError(f"CDP evaluation failed: {message}")
            return result.get("value")
    finally:
        connection.close()


def _download_items(driver, inspector: str) -> list[dict[str, Any]]:
    driver.switch_to.window(inspector)
    return _extension_call(
        driver,
        "chrome.downloads.search({}, items => done(items.map(item => ({"
        "id:item.id,url:item.url,finalUrl:item.finalUrl,state:item.state,paused:item.paused,"
        "bytesReceived:item.bytesReceived,totalBytes:item.totalBytes,filename:item.filename,error:item.error,"
        "canResume:item.canResume"
        "}))));",
    )


def _item_for(driver, inspector: str, suffix: str) -> dict[str, Any] | None:
    return next(
        (item for item in _download_items(driver, inspector) if suffix in str(item.get("url", ""))),
        None,
    )


def _click_download(driver, page: str, element_id: str) -> None:
    driver.switch_to.window(page)
    driver.find_element(By.ID, element_id).click()


def _assert_browser_completed(driver, inspector: str, suffix: str) -> dict[str, Any]:
    deadline = time.monotonic() + 30
    item = None
    while time.monotonic() < deadline:
        item = _item_for(driver, inspector, suffix)
        if (
            item
            and item.get("state") == "complete"
            and int(item.get("bytesReceived") or 0) == FILE_SIZE
        ):
            return item
        time.sleep(0.1)
    raise AssertionError(f"浏览器没有恢复并完成 {suffix}：{item!r}")


def _create_driver(
    browser_family: str,
    binary: Path,
    extension: Path,
    profile: Path,
    downloads: Path,
    driver_path: Path | None,
):
    common = [
        f"--user-data-dir={profile}",
        f"--disable-extensions-except={extension}",
        f"--load-extension={extension}",
        "--headless=new",
        "--remote-allow-origins=*",
        "--no-first-run",
        "--disable-default-apps",
        "--window-size=1280,800",
    ]
    preferences = {
        "download.default_directory": str(downloads),
        "download.prompt_for_download": False,
        "download.directory_upgrade": True,
        "safebrowsing.enabled": False,
    }
    if browser_family == "chrome":
        options = ChromeOptions()
        options.binary_location = str(binary)
        for argument in common:
            options.add_argument(argument)
        options.add_experimental_option("prefs", preferences)
        service = ChromeService(executable_path=str(driver_path)) if driver_path else ChromeService()
        return webdriver.Chrome(service=service, options=options)
    options = EdgeOptions()
    options.binary_location = str(binary)
    for argument in common:
        options.add_argument(argument)
    options.add_experimental_option("prefs", preferences)
    service = EdgeService(executable_path=str(driver_path)) if driver_path else EdgeService()
    return webdriver.Edge(service=service, options=options)


def run(
    extension: Path,
    browser_binary: Path,
    browser_family: str,
    go: str,
    driver_path: Path | None = None,
) -> dict[str, Any]:
    if not (extension / "manifest.json").is_file():
        raise RuntimeError(f"Chromium 扩展未构建：{extension}")
    with tempfile.TemporaryDirectory(prefix="hls-takeover-smoke-") as temporary:
        root = Path(temporary)
        host, host_log = _build_fake_host(root, go)
        manifest = root / "native-host.json"
        manifest.write_text(json.dumps({
            "name": HOST_NAME,
            "description": "HLS Downloader takeover smoke host",
            "path": str(host),
            "type": "stdio",
            "allowed_origins": [f"chrome-extension://{EXTENSION_ID}/"],
        }), encoding="utf-8")
        downloads = root / "downloads"
        downloads.mkdir()
        server = ThreadingHTTPServer(("127.0.0.1", 0), _DownloadHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        driver = None
        try:
            with _registered_host(manifest, browser_family):
                driver = _create_driver(
                    browser_family,
                    browser_binary,
                    extension,
                    root / "profile",
                    downloads,
                    driver_path,
                )
                page_url = f"http://127.0.0.1:{server.server_port}/page"
                driver.get(page_url)
                page = driver.current_window_handle
                _wait_until(
                    lambda: driver.execute_script(
                        "return document.documentElement.getAttribute('data-hls-downloader-extension')"
                    ) == "1",
                    "生产 content script 就绪",
                )
                driver.switch_to.new_window("tab")
                inspector = driver.current_window_handle
                popup_url = f"chrome-extension://{EXTENSION_ID}/popup.html?inspector=1"
                driver.get(popup_url)
                _wait_until(
                    lambda: driver.execute_script("return document.querySelector('main') !== null"),
                    "插件 popup 就绪",
                )
                _extension_call(
                    driver,
                    "chrome.downloads.search({}, items => Promise.all(items.map(item => "
                    "chrome.downloads.erase({id:item.id}))).then(() => done(true)));",
                )

                inspector_target = _wait_until(
                    lambda: next(
                        (
                            target for target in _debug_targets(driver)
                            if target.get("url") == popup_url
                        ),
                        None,
                    ),
                    "找到插件检查页调试目标",
                )

                # Open a real popup document in an inactive tab. Its active-tab
                # query therefore resolves the loopback page, exactly like the
                # browser action popup rather than an extension page opened as a tab.
                driver.switch_to.window(page)
                site_popup_tab = _cdp_evaluate(
                    str(inspector_target["webSocketDebuggerUrl"]),
                    "new Promise(resolve => chrome.tabs.create({"
                    "url: chrome.runtime.getURL('popup.html?site=1'), active:false"
                    "}, tab => resolve(tab.id)))",
                    True,
                )
                site_popup_target = _wait_until(
                    lambda: next(
                        (
                            target for target in _debug_targets(driver)
                            if "popup.html?site=1" in str(target.get("url", ""))
                        ),
                        None,
                    ),
                    "浏览器创建非活动 popup 调试目标",
                )
                _wait_until(
                    lambda: _cdp_evaluate(
                        str(site_popup_target["webSocketDebuggerUrl"]),
                        "[...document.querySelectorAll('button')].some(button => "
                        "button.textContent.includes('排除本站') && !button.disabled)",
                    ),
                    "排除本站按钮绑定顶层页面",
                )
                _cdp_evaluate(
                    str(site_popup_target["webSocketDebuggerUrl"]),
                    "[...document.querySelectorAll('button')].find(button => "
                    "button.textContent.includes('排除本站')).click()"
                )
                _wait_until(
                    lambda: _cdp_evaluate(
                        str(inspector_target["webSocketDebuggerUrl"]),
                        "new Promise(resolve => chrome.storage.local.get('excludedHosts', value => "
                        "resolve((value.excludedHosts || []).includes('127.0.0.1'))))",
                        True,
                    ),
                    "排除本站写入设置",
                )

                offers_before = len([event for event in _host_events(host_log) if event.get("op") == "offer"])
                _click_download(driver, page, "excluded")
                _assert_browser_completed(driver, inspector, "excluded.bin")
                offers_after = len([event for event in _host_events(host_log) if event.get("op") == "offer"])
                if offers_after != offers_before:
                    raise AssertionError("排除本站后仍把文件发送给桌面端")

                # Unexclude through the same real popup control.
                _cdp_evaluate(
                    str(site_popup_target["webSocketDebuggerUrl"]),
                    "[...document.querySelectorAll('button')].find(button => "
                    "button.textContent.includes('本站已排除')).click()"
                )
                _wait_until(
                    lambda: not _cdp_evaluate(
                        str(inspector_target["webSocketDebuggerUrl"]),
                        "new Promise(resolve => chrome.storage.local.get('excludedHosts', value => "
                        "resolve((value.excludedHosts || []).includes('127.0.0.1'))))",
                        True,
                    ),
                    "取消排除本站",
                )

                # Exercise the actual popup's auto-takeover control. The first
                # click disables locally before Native Messaging can respond.
                driver.switch_to.window(inspector)
                def auto_button():
                    return driver.find_element(
                        By.XPATH, "//button[starts-with(normalize-space(.), '自动接管')]"
                    )
                _wait_until(lambda: not auto_button().get_attribute("disabled"), "自动接管按钮就绪")
                auto_button().click()
                try:
                    _wait_until(lambda: "关" in auto_button().text, "自动接管立即关闭", 5)
                except AssertionError as error:
                    diagnostic = _extension_call(
                        driver,
                        "chrome.storage.local.get(null, value => done({value, buttons:"
                        "[...document.querySelectorAll('button')].map(button => ({text:button.innerText,disabled:button.disabled})),"
                        "error:document.querySelector('.send-error')?.innerText||''}));",
                    )
                    raise AssertionError(f"{error}; popup={diagnostic}") from error
                _click_download(driver, page, "disabled")
                _assert_browser_completed(driver, inspector, "disabled.bin")
                if any(
                    event.get("op") == "offer" and "disabled.bin" in str(event.get("url", ""))
                    for event in _host_events(host_log)
                ):
                    raise AssertionError("自动接管关闭后仍发送下载")
                driver.switch_to.window(inspector)
                auto_button().click()
                _wait_until(lambda: "开" in auto_button().text, "自动接管重新开启")

                _click_download(driver, page, "accept")
                paused = _wait_until(
                    lambda: (
                        item
                        if (item := _item_for(driver, inspector, "accept.bin"))
                        and item.get("state") == "in_progress"
                        and item.get("paused") is True
                        else None
                    ),
                    "桌面确认前浏览器项目已暂停",
                )
                _wait_until(
                    lambda: _item_for(driver, inspector, "accept.bin") is None,
                    "桌面接受后清除浏览器副本",
                    20,
                )

                _click_download(driver, page, "reject")
                rejected = _assert_browser_completed(driver, inspector, "reject.bin")
                _wait_until(
                    lambda: any(
                        event.get("op") == "handoff_status"
                        and event.get("status") == "rejected"
                        and "reject.bin" in str(event.get("handoff_id", ""))
                        for event in _host_events(host_log)
                    ),
                    "桌面拒绝状态已返回",
                )

                _click_download(driver, page, "disconnect")
                disconnected = _assert_browser_completed(driver, inspector, "disconnect.bin")
                disconnect_events = [
                    event for event in _host_events(host_log)
                    if event.get("op") == "offer" and event.get("disconnect") is True
                ]
                if len(disconnect_events) < 2:
                    raise AssertionError(f"Native Host 断线重试不足：{disconnect_events}")

                return {
                    "sitePopupTab": site_popup_tab,
                    "acceptPausedBytes": paused.get("bytesReceived"),
                    "rejectCompletedBytes": rejected.get("bytesReceived"),
                    "disconnectCompletedBytes": disconnected.get("bytesReceived"),
                    "nativeEvents": len(_host_events(host_log)),
                }
        finally:
            if driver is not None:
                with contextlib.suppress(Exception):
                    driver.quit()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--extension", type=Path, default=Path("extension/.output/chrome-mv3"))
    parser.add_argument("--browser", choices=("chrome", "edge"), default="edge")
    parser.add_argument("--browser-binary", type=Path, required=True)
    parser.add_argument("--driver", type=Path)
    parser.add_argument("--go", default=shutil.which("go") or "go")
    args = parser.parse_args()
    result = run(
        args.extension.resolve(),
        args.browser_binary.resolve(),
        args.browser,
        args.go,
        args.driver.resolve() if args.driver else None,
    )
    print(json.dumps(result, ensure_ascii=False, indent=2))
    print("Production extension takeover smoke passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
