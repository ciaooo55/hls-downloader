"""Real Chromium smoke for ordinary-download takeover and popup controls.

The smoke uses an isolated browser profile and a temporary Native Messaging
host. It temporarily replaces only the current user's host registration for
the selected Chromium family, restores the exact previous value in ``finally``,
and never opens or modifies a real browser profile.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
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
                for name in ("disabled", "excluded", "accept", "fallback", "reject", "disconnect")
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
            "disabled.bin", "excluded.bin", "accept.bin", "fallback.bin", "reject.bin", "disconnect.bin"
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
        offset = start % len(FILE_CHUNK)
        try:
            while remaining > 0:
                chunk = FILE_CHUNK[offset : offset + min(remaining, len(FILE_CHUNK) - offset)]
                self.wfile.write(chunk)
                self.wfile.flush()
                remaining -= len(chunk)
                offset = 0
                time.sleep(0.008)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
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
      handoff := map[string]interface{{}}{{"id": id, "status": status}}
      if status == "accepted" {{
        if strings.Contains(id, "fallback.bin") {{
          handoff["task_status"] = "failed"
          handoff["task_stage"] = "failed"
          handoff["task_error_code"] = "HTTP_404"
          handoff["task_downloaded_bytes"] = 0
        }} else {{
          handoff["task_status"] = "downloading"
          handoff["task_stage"] = "downloading"
          handoff["task_downloaded_bytes"] = 4096
        }}
      }}
      response["handoff"] = handoff
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
    vendor = {"chrome": "Google\\Chrome", "edge": "Microsoft\\Edge", "firefox": "Mozilla"}[browser_family]
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
        "canResume:item.canResume,danger:item.danger,mime:item.mime,exists:item.exists"
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
            output = Path(str(item.get("filename", "")))
            expected = hashlib.sha256(FILE_CHUNK * (FILE_SIZE // len(FILE_CHUNK))).hexdigest()
            if not output.is_file() or hashlib.sha256(output.read_bytes()).hexdigest() != expected:
                raise AssertionError(f"浏览器完成的 {suffix} 内容 SHA-256 不符：{output}")
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
        "--disable-features=DisableLoadExtensionCommandLineSwitch",
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
    report: Path | None = None,
) -> dict[str, Any]:
    if not (extension / "manifest.json").is_file():
        raise RuntimeError(f"Chromium 扩展未构建：{extension}")
    temp_root = Path(__file__).resolve().parents[1] / ".tool-cache" / "test-tmp"
    temp_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="hls-takeover-smoke-", dir=temp_root) as temporary:
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
                idle_media_ui = driver.execute_script(
                    """
                    const roots=[]; const seen=new Set();
                    const visit=(root) => {
                      if (!root || seen.has(root)) return; seen.add(root); roots.push(root);
                      root.querySelectorAll('*').forEach(element => { if (element.shadowRoot) visit(element.shadowRoot); });
                    };
                    visit(document);
                    return roots.some(root => root.querySelector('.video-buttons, .wrap .panel'));
                    """
                )
                if idle_media_ui:
                    raise RuntimeError("无媒体页面不应初始化完整媒体悬浮窗")
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
                        "button.textContent.includes('本站提示：已显示') && !button.disabled)",
                    ),
                    "排除本站按钮绑定顶层页面",
                )
                # 先等到按钮真的可点再点：popup 的控件文案是在读到 storage 之后才写入的，
                # 上一轮直接点会撞上"按钮还没渲染好"。失败时把真实文案带进错误信息。
                click_deadline = time.monotonic() + 20
                button_texts: list[str] = []
                while True:
                    button_texts = _cdp_evaluate(
                        str(site_popup_target["webSocketDebuggerUrl"]),
                        "[...document.querySelectorAll('button')].map(button => button.textContent)",
                    )
                    if any("本站提示：已显示" in text for text in button_texts):
                        break
                    if time.monotonic() >= click_deadline:
                        raise AssertionError(f"排除本站按钮未出现：{button_texts}")
                    time.sleep(0.1)
                _cdp_evaluate(
                    str(site_popup_target["webSocketDebuggerUrl"]),
                    "[...document.querySelectorAll('button')].find(button => "
                    "button.textContent.includes('本站提示：已显示')).click()"
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
                excluded = _assert_browser_completed(driver, inspector, "excluded.bin")
                offers_after = len([event for event in _host_events(host_log) if event.get("op") == "offer"])
                if offers_after != offers_before:
                    raise AssertionError("排除本站后仍把文件发送给桌面端")

                # Unexclude through the same real popup control.
                # 恢复显示同样要等按钮文案翻转过来再点，否则点的还是排除前那个状态。
                restore_deadline = time.monotonic() + 20
                restore_texts: list[str] = []
                while True:
                    restore_texts = _cdp_evaluate(
                        str(site_popup_target["webSocketDebuggerUrl"]),
                        "[...document.querySelectorAll('button')].map(button => button.textContent)",
                    )
                    if any("本站提示：已隐藏" in text for text in restore_texts):
                        break
                    if time.monotonic() >= restore_deadline:
                        raise AssertionError(f"取消排除本站按钮未出现：{restore_texts}")
                    time.sleep(0.1)
                _cdp_evaluate(
                    str(site_popup_target["webSocketDebuggerUrl"]),
                    "[...document.querySelectorAll('button')].find(button => "
                    "button.textContent.includes('本站提示：已隐藏')).click()"
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
                disabled = _assert_browser_completed(driver, inspector, "disabled.bin")
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

                _click_download(driver, page, "fallback")
                fallback = _assert_browser_completed(driver, inspector, "fallback.bin")
                _wait_until(
                    lambda: any(
                        event.get("op") == "handoff_status"
                        and event.get("status") == "accepted"
                        and "fallback.bin" in str(event.get("handoff_id", ""))
                        for event in _host_events(host_log)
                    ),
                    "桌面任务失败后恢复浏览器原下载",
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

                result = {
                    "passed": True,
                    "sitePopupTab": site_popup_tab,
                    "excludedCompletedBytes": excluded.get("bytesReceived"),
                    "disabledCompletedBytes": disabled.get("bytesReceived"),
                    "acceptPausedBytes": paused.get("bytesReceived"),
                    "fallbackCompletedBytes": fallback.get("bytesReceived"),
                    "rejectCompletedBytes": rejected.get("bytesReceived"),
                    "disconnectCompletedBytes": disconnected.get("bytesReceived"),
                    "nativeEvents": len(_host_events(host_log)),
                    "browserFileSha256Verified": ["excluded.bin", "disabled.bin", "fallback.bin", "reject.bin", "disconnect.bin"],
                }
                if report:
                    report.parent.mkdir(parents=True, exist_ok=True)
                    report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
                return result
        except Exception as error:
            if report:
                diagnostic: dict[str, Any] = {"error": str(error), "nativeEvents": _host_events(host_log)}
                if driver is not None:
                    with contextlib.suppress(Exception):
                        diagnostic["downloads"] = _download_items(driver, inspector)
                        diagnostic["extension"] = _extension_call(
                            driver, "chrome.storage.local.get(null, value => done({value, buttons:"
                            "[...document.querySelectorAll('button')].map(button => ({text:button.innerText,disabled:button.disabled}))}));",
                        )
                    with contextlib.suppress(Exception):
                        diagnostic["targets"] = _debug_targets(driver)
                    with contextlib.suppress(Exception):
                        driver.switch_to.window(page)
                        diagnostic["page"] = driver.execute_script(
                            "return {url:location.href,html:document.body.innerHTML,ready:document.readyState}"
                        )
                report.parent.mkdir(parents=True, exist_ok=True)
                report.write_text(json.dumps(diagnostic, ensure_ascii=False, indent=2), encoding="utf-8")
            raise
        finally:
            if driver is not None:
                with contextlib.suppress(Exception):
                    driver.quit()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--extension", type=Path, required=True, help="已打包候选/正式产物解压后的扩展目录")
    parser.add_argument("--browser", choices=("chrome", "edge"), default="edge")
    parser.add_argument("--browser-binary", type=Path, required=True)
    parser.add_argument("--driver", type=Path)
    parser.add_argument("--go", default=shutil.which("go") or "go")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--real-runtime", type=Path, help="Use the installed Host/Core and real desktop confirmation")
    args = parser.parse_args()
    if args.real_runtime:
        result = run_real(args)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0 if result['passed'] else 1
    result = run(
        args.extension.resolve(),
        args.browser_binary.resolve(),
        args.browser,
        args.go,
        args.driver.resolve() if args.driver else None,
        args.report.resolve() if args.report else None,
    )
    print(json.dumps(result, ensure_ascii=False, indent=2))
    print("Production extension takeover smoke passed.")
    return 0


def run_real(args) -> dict[str, Any]:
    from smoke_v7_portable_app import Pipe, wait
    from smoke_v7_browser_download_confirmation import click_download_confirmation, presenter_window
    import traceback
    import psutil
    runtime = args.real_runtime.resolve()
    result = {'passed': False, 'real_native_host': True, 'checks': [], 'runtime': str(runtime)}
    result['components']={name:hashlib.sha256((runtime/'app/resources'/name).read_bytes()).hexdigest() for name in ['HLSDownloaderEngine.exe','HLSDownloaderNativeHost.exe','HLSDownloaderPresenter.exe']}
    result['background_sha256']=hashlib.sha256((args.extension/'background.js').read_bytes()).hexdigest()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    created = set()
    driver = None
    server = None
    def core(message):
        p = Pipe()
        try: return p.request(message)
        finally: p.close()
    def tasks(): return core({'type':'snapshot','request_id':1})['tasks']
    initial = {t['task_id'] for t in tasks()}
    settings = core({'type':'load_settings','request_id':2})
    def record(name, actual=True):
        result['checks'].append({'name':name,'actual':actual})
        args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8')
        print(json.dumps({'check':name}),flush=True)
    temp_root = Path(__file__).resolve().parents[1]/'.tool-cache/test-tmp'
    with tempfile.TemporaryDirectory(prefix='real-takeover-',dir=temp_root) as temporary:
        stage = Path(temporary)
        try:
            downloads=stage/'downloads'; downloads.mkdir()
            class RealDownloadHandler(_DownloadHandler):
                fallback_requests=0
                def do_GET(self):
                    if self.path.split('?',1)[0]=='/fallback.bin':
                        RealDownloadHandler.fallback_requests+=1
                        result.setdefault('fallback_requests',[]).append({'number':RealDownloadHandler.fallback_requests,'headers':dict(self.headers)})
                        # 浏览器保留 Fetch Metadata；Core 的分段及重试请求不带这些头。
                        # 按调用方制造错误，不能把后续 Core 分段误当浏览器续传并阻塞。
                        if not self.headers.get('Sec-Fetch-Mode'):
                            self.send_error(403);return
                    super().do_GET()
            server=ThreadingHTTPServer(('127.0.0.1',0),RealDownloadHandler)
            threading.Thread(target=server.serve_forever,daemon=True).start()
            driver=_create_driver(args.browser,args.browser_binary.resolve(),args.extension.resolve(),stage/'profile',downloads,args.driver.resolve())
            driver.set_script_timeout(25)
            page_url=f'http://127.0.0.1:{server.server_port}/page'
            driver.get(page_url); page=driver.current_window_handle
            driver.switch_to.new_window('tab'); inspector=driver.current_window_handle
            driver.get(f'chrome-extension://{EXTENSION_ID}/popup.html')
            def message(value):
                driver.switch_to.window(inspector)
                return _extension_call(driver,'chrome.runtime.sendMessage(arguments[0]).then(done,e=>done({ok:false,error:String(e)}))',value)
            assert message({'type':'ping'}).get('ok'), 'Real Host unavailable'
            def enabled(value):
                r=message({'type':'set-takeover-settings','enabled':value,'minimumBytes':1024*1024})
                assert r.get('ok'), r
            enabled(False)
            _click_download(driver,page,'disabled')
            record('takeover-disabled-browser-complete-sha256',_assert_browser_completed(driver,inspector,'disabled.bin'))
            assert initial=={t['task_id'] for t in tasks()}
            enabled(True)
            _extension_call(driver,"chrome.storage.local.set({excludedHosts:['127.0.0.1']}).then(()=>done(true))")
            _click_download(driver,page,'excluded')
            record('site-excluded-browser-complete-sha256',_assert_browser_completed(driver,inspector,'excluded.bin'))
            assert initial=={t['task_id'] for t in tasks()}
            _extension_call(driver,"chrome.storage.local.set({excludedHosts:[]}).then(()=>done(true))")
            before={t['task_id'] for t in tasks()}
            _click_download(driver,page,'accept')
            wait(lambda:presenter_window(runtime=runtime),'Takeover confirmation missing',20)
            paused=_item_for(driver,inspector,'accept.bin')
            assert paused and paused['paused'], paused
            assert before=={t['task_id'] for t in tasks()}, 'Task began before confirmation'
            record('real-takeover-pauses-before-desktop-confirmation',paused)
            click_download_confirmation()
            def completed():
                found=[t for t in tasks() if t['task_id'] not in before and 'accept.bin' in t['url']]
                created.update(t['task_id'] for t in found)
                if found and found[-1]['status']=='failed': raise AssertionError(found[-1])
                return found[-1] if found and found[-1]['status']=='completed' else None
            task=wait(completed,'Real takeover task did not complete',60)
            expected=hashlib.sha256(FILE_CHUNK*(FILE_SIZE//len(FILE_CHUNK))).hexdigest()
            path=Path(task['output_path'])
            assert path.stat().st_size==FILE_SIZE and hashlib.sha256(path.read_bytes()).hexdigest()==expected
            wait(lambda:_item_for(driver,inspector,'accept.bin') is None,'Browser copy remained after accepted takeover',20)
            assert not list(downloads.glob('accept*')), 'Duplicate browser output'
            record('real-takeover-accepted-single-complete-file-sha256',expected)
            _click_download(driver,page,'reject')
            wait(lambda:presenter_window(runtime=runtime),'Reject confirmation missing',20)
            click_download_confirmation(accept=False)
            record('real-desktop-rejection-browser-resumes-sha256',_assert_browser_completed(driver,inspector,'reject.bin'))
            assert len(created)==len([t for t in tasks() if t['task_id'] not in initial]), 'Rejected takeover created another task'
            _click_download(driver,page,'fallback')
            wait(lambda:presenter_window(runtime=runtime),'Fallback confirmation missing',20)
            click_download_confirmation()
            def failed():
                found=[t for t in tasks() if t['task_id'] not in initial and 'fallback.bin' in t['url']]
                created.update(t['task_id'] for t in found)
                return found[-1] if found and found[-1]['status']=='failed' else None
            failure=wait(failed,'Accepted desktop HTTP failure was not reported',60)
            assert failure.get('error_message'),failure
            record('real-accepted-desktop-failure-browser-fallback-sha256',_assert_browser_completed(driver,inspector,'fallback.bin'))
            _click_download(driver,page,'disconnect')
            wait(lambda:presenter_window(runtime=runtime),'Disconnect confirmation missing',20)
            owned=[]
            for process in psutil.process_iter(['name','exe']):
                if process.info['name']!='HLSDownloaderNativeHost.exe' or not process.info['exe']:continue
                if Path(process.info['exe']).resolve()!=runtime/'app/resources/HLSDownloaderNativeHost.exe':continue
                with contextlib.suppress(psutil.NoSuchProcess,psutil.AccessDenied):
                    if any(parent.pid==driver.service.process.pid for parent in process.parents()):owned.append(process)
            assert owned,'Could not identify this isolated browser Native Host'
            for process in owned:process.terminate();process.wait(5)
            click_download_confirmation(accept=False)
            record('actual-native-host-disconnect-pending-confirmation-browser-recovery-sha256',_assert_browser_completed(driver,inspector,'disconnect.bin'))
            assert message({'type':'ping'}).get('ok'),'Native Host did not reconnect'
            result['passed']=True
        except Exception as error:
            result['error']=str(error);result['traceback']=traceback.format_exc()
            args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8')
        finally:
            if driver: driver.quit()
            try:
                if server:
                    marker=f'http://127.0.0.1:{server.server_port}/'
                    created.update(t['task_id'] for t in tasks() if t['url'].startswith(marker))
                    for encoded in core({'type':'load_handoffs','request_id':3})['items']:
                        handoff=json.loads(encoded)
                        if handoff.get('status')=='pending' and handoff.get('offer',{}).get('url','').startswith(marker):
                            core({'type':'command','request_id':4,'command':{'kind':'reject_handoff','handoff_id':handoff['id'],'suppress_site_kind':False}})
                for task_id in created:
                    core({'type':'command','request_id':5,'command':{'kind':'task_action','task_id':task_id,'action':'delete_files'}})
                core({'type':'store_settings','request_id':6,'values':{'browser_takeover_enabled':settings['takeover_enabled'],'browser_takeover_minimum_bytes':settings['takeover_minimum_bytes']}})
                result['user_tasks_preserved']=initial<={t['task_id'] for t in tasks()}
            except Exception as error:
                result['cleanup_error']=str(error)
                result['passed']=False
            if server: server.shutdown();server.server_close()
            args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8')
    return result


if __name__ == "__main__":
    raise SystemExit(main())
