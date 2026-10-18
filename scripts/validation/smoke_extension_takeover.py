"""Real Chromium smoke for ordinary-download takeover and popup controls.

The smoke uses an isolated browser profile and a temporary Native Messaging
host. It temporarily replaces only the current user's host registration for
the selected Chromium family, restores the exact previous value in ``finally``,
and never opens or modifies a real browser profile.
"""

from __future__ import annotations

import argparse
import contextlib
from email import policy
from email.parser import BytesParser
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
import zipfile
from typing import Any, Callable
from urllib.request import urlopen
import winreg

from selenium import webdriver
from selenium.webdriver.chrome.options import Options as ChromeOptions
from selenium.webdriver.chrome.service import Service as ChromeService
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService
import websocket


HOST_NAME = "com.ciaooo55.hls_downloader"
EXTENSION_ID = "bbdfldcjnikaemnimalegbopgaknjhla"
FILE_SIZE = 8 * 1024 * 1024
FILE_CHUNK = bytes(range(256)) * 128
SMALL_SIZE = 64 * 1024
QUICK_SIZE = 1024
STREAM_SIZE = 2 * 1024 * 1024
IMAGE_PAYLOAD = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
    "0000000b49444154789c636000020000050001a5f645400000000049454e44ae426082"
)
JSON_PAYLOAD = b'{"export":true,"items":[1,2,3]}\n'
LOCAL_DOWNLOAD_PAYLOAD = b'HLS Downloader local browser file canary: 20261011'


def _fixture_bytes(size: int, offset: int = 0) -> bytes:
    return bytes((offset + index) % 256 for index in range(size))


class _DownloadHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        if self.path != '/post.bin' or body != b'token=takeover-post-canary':
            self.send_error(403)
            return
        if self.headers.get('Content-Type', '').split(';', 1)[0] != 'application/x-www-form-urlencoded':
            self.send_error(415)
            return
        self.send_response(200)
        self.send_header('Content-Type', 'application/octet-stream')
        self.send_header('Content-Disposition', 'attachment; filename="post.bin"')
        self.send_header('Content-Length', str(SMALL_SIZE))
        self.send_header('Connection', 'close')
        self.end_headers()
        self.close_connection = True
        with contextlib.suppress(BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            self.wfile.write(_fixture_bytes(SMALL_SIZE))

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if self.path.split("?", 1)[0] == "/page":
            links = "".join(
                f'<a id="{name}" href="/{path}"{ " download" if download else ""}>{name} 下载</a><br>'
                for name, path, download in (
                    ("disabled", "disabled.bin", True),
                    ("excluded", "excluded.bin", True),
                    ("accept", "accept.bin", True),
                    ("fallback", "fallback.bin", True),
                    ("reject", "reject.bin", True),
                    ("disconnect", "disconnect.bin", True),
                    ("small", "small.bin", True),
                    ("stream", "stream.bin", True),
                    ("ordinary", "ordinary.bin", False),
                    ("quick", "quick.bin", True),
                    ("image", "image.png", True),
                    ("json", "export.json", True),
                    ("generic", "generic", True),
                )
            )
            payload = (
                "<!doctype html><html lang=zh-CN><head><meta charset=utf-8>"
                "<title>takeover smoke</title></head><body>"
                f"{links}<button id='generated_blob'>生成 Blob 下载</button>"
                "<button id='generated_blob_alt'>Alt 生成 Blob 下载</button>"
                "<button id='generated_data'>生成 data URL 下载</button>"
                "<button id='generated_data_excluded'>生成排除站点 data URL 下载</button>"
                "<button id='generated_blob_reject'>生成 Blob 拒绝下载</button>"
                "<form method='post' action='/post.bin'>"
                "<input type='hidden' name='token' value='takeover-post-canary'>"
                "<button id='post' type='submit'>表单 POST 下载</button></form>"
                "<form method='post' enctype='multipart/form-data' action='/multipart-accept.bin'>"
                "<input type='hidden' name='token' value='takeover-multipart-canary'>"
                "<button id='multipart_accept' type='submit'>multipart 接受下载</button></form>"
                "<form method='post' enctype='multipart/form-data' action='/multipart-reject.bin'>"
                "<input type='hidden' name='token' value='takeover-multipart-canary'>"
                "<button id='multipart_reject' type='submit'>multipart 拒绝下载</button></form>"
                "<script>"
                "const canary='HLS Downloader local browser file canary: 20261011';"
                "function download(url,name){const a=document.createElement('a');a.href=url;"
                "a.download=name;document.body.appendChild(a);a.click();a.remove();}"
                "document.getElementById('generated_blob').addEventListener('click',()=>"
                "download(URL.createObjectURL(new Blob([canary],{type:'application/octet-stream'})),"
                "'generated-blob.bin'));"
                "document.getElementById('generated_blob_alt').addEventListener('click',()=>"
                "download(URL.createObjectURL(new Blob([canary],{type:'application/octet-stream'})),"
                "'generated-blob-alt.bin'));"
                "document.getElementById('generated_data').addEventListener('click',()=>"
                "download('data:application/octet-stream;base64,'+btoa(canary),'generated-data.bin'));"
                "document.getElementById('generated_data_excluded').addEventListener('click',()=>"
                "download('data:application/octet-stream;base64,'+btoa(canary),'generated-data-excluded.bin'));"
                "document.getElementById('generated_blob_reject').addEventListener('click',()=>"
                "download(URL.createObjectURL(new Blob([canary],{type:'application/octet-stream'})),"
                "'generated-blob-rejected.bin'));"
                "</script></body></html>"
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        name = self.path.split("?", 1)[0].removeprefix("/")
        fixture = {
            "disabled.bin": (FILE_SIZE, "application/octet-stream", True, True, "disabled.bin"),
            "excluded.bin": (FILE_SIZE, "application/octet-stream", True, True, "excluded.bin"),
            "accept.bin": (FILE_SIZE, "application/octet-stream", True, True, "accept.bin"),
            "fallback.bin": (FILE_SIZE, "application/octet-stream", True, True, "fallback.bin"),
            "reject.bin": (FILE_SIZE, "application/octet-stream", True, True, "reject.bin"),
            "disconnect.bin": (FILE_SIZE, "application/octet-stream", True, True, "disconnect.bin"),
            "small.bin": (SMALL_SIZE, "application/octet-stream", True, True, "small.bin"),
            "stream.bin": (STREAM_SIZE, "application/octet-stream", True, False, "stream.bin"),
            "ordinary.bin": (SMALL_SIZE, "application/octet-stream", False, True, "ordinary.bin"),
            "quick.bin": (QUICK_SIZE, "application/octet-stream", True, True, "quick.bin"),
            "image.png": (len(IMAGE_PAYLOAD), "image/png", True, True, "image.png"),
            "export.json": (len(JSON_PAYLOAD), "application/json", False, True, "export.json"),
            "generic": (SMALL_SIZE, "application/octet-stream", False, True, "generic"),
        }.get(name)
        if fixture is None:
            self.send_error(404)
            return
        size, mime_type, attachment, content_length, filename = fixture
        payload = IMAGE_PAYLOAD if name == "image.png" else JSON_PAYLOAD if name == "export.json" else None
        start = 0
        end = size - 1
        range_value = self.headers.get("Range", "")
        if content_length and range_value.startswith("bytes="):
            with contextlib.suppress(ValueError):
                start_text, end_text = range_value[6:].split("-", 1)
                start = max(0, int(start_text))
                if end_text:
                    end = min(end, int(end_text))
        if start >= size or end < start:
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{size}")
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            return
        self.send_response(206 if content_length and range_value.startswith("bytes=") else 200)
        self.send_header("Content-Type", mime_type)
        if content_length:
            self.send_header("Accept-Ranges", "bytes")
        if attachment:
            self.send_header("Content-Disposition", f'attachment; filename="{filename}"')
        if content_length:
            self.send_header("Content-Length", str(end - start + 1))
        if content_length and range_value.startswith("bytes="):
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        remaining = end - start + 1
        offset = start
        try:
            while remaining > 0:
                chunk_size = min(remaining, 16 * 1024 if not content_length else len(FILE_CHUNK))
                chunk = (
                    payload[offset : offset + chunk_size]
                    if payload is not None
                    else _fixture_bytes(chunk_size, offset)
                )
                self.wfile.write(chunk)
                self.wfile.flush()
                remaining -= len(chunk)
                offset += len(chunk)
                if not content_length:
                    time.sleep(0.01)
                elif size >= FILE_SIZE:
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
      filename, _ := resource["filename"].(string)
      event["url"] = url
      event["filename"] = filename
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
    matches = [item for item in _download_items(driver, inspector) if suffix in str(item.get("url", ""))]
    return max(matches, key=lambda item:item['id'], default=None)


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
        ):
            output = Path(str(item.get("filename", "")))
            expected = hashlib.sha256(FILE_CHUNK * (FILE_SIZE // len(FILE_CHUNK))).hexdigest()
            if not output.is_file() or output.stat().st_size != FILE_SIZE or hashlib.sha256(output.read_bytes()).hexdigest() != expected:
                raise AssertionError(f"浏览器完成的 {suffix} 内容 SHA-256 不符：{output}")
            item['verified_bytes']=output.stat().st_size
            item['verified_sha256']=expected
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
    headed: bool = False,
):
    if browser_family == 'firefox':
        options = FirefoxOptions()
        options.binary_location = str(binary)
        options.page_load_strategy = 'eager'
        options.set_preference('browser.shell.checkDefaultBrowser', False)
        options.set_preference('browser.aboutwelcome.enabled', False)
        options.set_preference('browser.newtabpage.activity-stream.asrouter.providers.onboarding', '{"id":"onboarding","type":"local","localProvider":"OnboardingMessageProvider","enabled":false}')
        options.set_preference('browser.newtabpage.activity-stream.asrouter.providers.messaging-experiments', '{"id":"messaging-experiments","type":"remote-experiments","enabled":false}')
        if not headed:
            options.add_argument('-headless')
        options.set_preference('browser.download.folderList', 2)
        options.set_preference('browser.download.dir', str(downloads))
        options.set_preference('browser.download.useDownloadDir', True)
        options.set_preference('browser.helperApps.neverAsk.saveToDisk', 'application/octet-stream')
        service = FirefoxService(executable_path=str(driver_path),
            service_args=['--profile-root', str(profile.parent), '--allow-system-access'],
            log_output=str(profile.parent / 'geckodriver.log'))
        driver = webdriver.Firefox(service=service, options=options)
        addon = profile.parent / 'extension.xpi'
        with zipfile.ZipFile(addon, 'w', zipfile.ZIP_DEFLATED) as archive:
            for path in extension.rglob('*'):
                if path.is_file():archive.write(path, path.relative_to(extension).as_posix())
        driver.install_addon(str(addon), temporary=True)
        return driver
    common = [
        f"--user-data-dir={profile}",
        "--disable-features=DisableLoadExtensionCommandLineSwitch",
        f"--disable-extensions-except={extension}",
        f"--load-extension={extension}",
        "--remote-allow-origins=*",
        "--no-first-run",
        "--disable-default-apps",
        "--window-size=1280,800",
    ]
    if not headed:
        common.append('--headless=new')
    preferences = {
        "download.default_directory": str(downloads),
        "download.prompt_for_download": False,
        "download.directory_upgrade": True,
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
    temp_root = Path(__file__).resolve().parents[2] / ".tool-cache" / "test-tmp"
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
    parser.add_argument("--browser", choices=("chrome", "edge", "firefox"), default="edge")
    parser.add_argument("--browser-binary", type=Path, required=True)
    parser.add_argument("--driver", type=Path)
    parser.add_argument("--go", default=shutil.which("go") or "go")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--real-runtime", type=Path, help="Use the installed Host/Core and real desktop confirmation")
    parser.add_argument('--headed', action='store_true')
    parser.add_argument('--real-scenario', choices=('all', 'post', 'rejection', 'fallback'), default='all')
    args = parser.parse_args()
    if args.browser == 'firefox' and not args.real_runtime:
        parser.error('Firefox takeover validation requires --real-runtime')
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
    result = {'passed': False, 'browser': args.browser, 'headed': args.headed, 'real_native_host': True, 'checks': [], 'runtime': str(runtime)}
    result['components']={name:hashlib.sha256((runtime/'app/resources'/name).read_bytes()).hexdigest() for name in ['HLSDownloaderEngine.exe','HLSDownloaderNativeHost.exe','HLSDownloaderPresenter.exe']}
    result['background_sha256']=hashlib.sha256((args.extension/'background.js').read_bytes()).hexdigest()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    created = set()
    driver = None
    server = None
    inspector = None
    def core(message):
        p = Pipe()
        try: return p.request(message)
        finally: p.close()
    def tasks(): return core({'type':'snapshot','request_id':1})['tasks']
    initial_handoffs = {json.loads(encoded)['id'] for encoded in core({'type':'load_handoffs','request_id':7})['items']}
    def single_resolved_handoff(name, path=None, filename=None):
        url = f'http://127.0.0.1:{server.server_port}/{path or name + ".bin"}'
        handoffs = [json.loads(encoded) for encoded in core({'type':'load_handoffs','request_id':7})['items']]
        if filename:
            matches = [handoff for handoff in handoffs
                if handoff['id'] not in initial_handoffs and handoff.get('offer',{}).get('filename') == filename]
        else:
            matches = [handoff for handoff in handoffs
                if handoff.get('offer',{}).get('url') == url]
        assert len(matches) == 1 and matches[0]['status'] != 'pending', f'Duplicate or unresolved takeover: {matches}'
        record('one-resolved-handoff-'+name, {'id':matches[0]['id'],'status':matches[0]['status']})
    initial = {t['task_id'] for t in tasks()}
    settings = core({'type':'load_settings','request_id':2})
    def record(name, actual=True):
        result['checks'].append({'name':name,'actual':actual})
        args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8')
        print(json.dumps({'check':name}),flush=True)
    # 真实浏览器配置放本地临时目录，隔离沙盒映射盘对 Native Messaging 启动的影响。
    temp_root = Path(tempfile.gettempdir())
    with tempfile.TemporaryDirectory(prefix='real-takeover-',dir=temp_root) as temporary:
        stage = Path(temporary)
        try:
            downloads=stage/'downloads'; downloads.mkdir()
            class RealDownloadHandler(_DownloadHandler):
                fallback_requests=0
                response_headers={}
                multipart_requests={}
                multipart_lock=threading.Lock()
                urlencoded_post_requests=0
                def send_header(self, keyword, value):
                    if hasattr(self, '_fixture_response_headers'):
                        self._fixture_response_headers.append((keyword, value))
                    super().send_header(keyword,value)
                def do_POST(self):
                    path=self.path.split('?',1)[0]
                    if path == '/post.bin':
                        with RealDownloadHandler.multipart_lock:
                            RealDownloadHandler.urlencoded_post_requests += 1
                            result['urlencoded_post_requests'] = RealDownloadHandler.urlencoded_post_requests
                    if path not in {'/multipart-accept.bin','/multipart-reject.bin'}:
                        return super().do_POST()
                    with RealDownloadHandler.multipart_lock:
                        RealDownloadHandler.multipart_requests[path]=RealDownloadHandler.multipart_requests.get(path,0)+1
                        result.setdefault('multipart_request_methods', []).append({'path':path,'method':'POST'})
                    body=self.rfile.read(int(self.headers.get('Content-Length','0')))
                    content_type=self.headers.get('Content-Type','')
                    message=None
                    tokens=[]
                    try:
                        message=BytesParser(policy=policy.default).parsebytes(
                            f'Content-Type: {content_type}\r\nMIME-Version: 1.0\r\n\r\n'.encode('ascii')+body)
                        tokens=[part.get_content() for part in message.iter_parts()
                            if part.get_content_disposition()=='form-data'
                            and part.get_param('name',header='content-disposition')=='token'] if message.is_multipart() else []
                    except (UnicodeEncodeError, ValueError):
                        pass
                    if message is None or not message.is_multipart() or tokens != ['takeover-multipart-canary']:
                        self.send_error(403)
                        return
                    self.send_response(200)
                    self.send_header('Content-Type','application/octet-stream')
                    self.send_header('Content-Disposition',f'attachment; filename="{path.removeprefix("/")}"')
                    self.send_header('Content-Length',str(SMALL_SIZE))
                    self.send_header('Connection','close')
                    self.end_headers()
                    self.close_connection=True
                    with contextlib.suppress(BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                        self.wfile.write(_fixture_bytes(SMALL_SIZE))
                def do_GET(self):
                    if self.path.split('?',1)[0] in {'/multipart-accept.bin','/multipart-reject.bin'}:
                        with RealDownloadHandler.multipart_lock:
                            result.setdefault('multipart_request_methods', []).append({'path':self.path.split('?',1)[0],'method':'GET'})
                    if self.path.split('?',1)[0]=='/fallback.bin':
                        RealDownloadHandler.fallback_requests+=1
                        result.setdefault('fallback_requests',[]).append({'number':RealDownloadHandler.fallback_requests,'headers':dict(self.headers)})
                        # Core 分段下载显式要求 identity；Firefox 可重放原网页的 Fetch Metadata。
                        # 用传输编码区分调用方，确保真正制造桌面失败而非误把 Core 当浏览器。
                        if self.headers.get('Accept-Encoding') == 'identity':
                            self.send_error(403);return
                    fixture_path=self.path.split('?',1)[0].removeprefix('/')
                    if fixture_path in {'small.bin','stream.bin','ordinary.bin','quick.bin','image.png','export.json','generic'}:
                        self._fixture_response_headers=[]
                        RealDownloadHandler.response_headers.setdefault(fixture_path,[]).append(self._fixture_response_headers)
                    super().do_GET()
            server=ThreadingHTTPServer(('127.0.0.1',0),RealDownloadHandler)
            threading.Thread(target=server.serve_forever,daemon=True).start()
            driver=_create_driver(args.browser,args.browser_binary.resolve(),args.extension.resolve(),stage/'profile',downloads,args.driver.resolve() if args.driver else None,headed=args.headed)
            driver.set_script_timeout(25)
            driver.set_page_load_timeout(40)
            page_url=f'http://127.0.0.1:{server.server_port}/page'
            with urlopen(page_url, timeout=5) as response:
                result['page_fixture_probe'] = {'url': page_url, 'status': response.status,
                    'bytes': len(response.read())}
            args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8')
            driver.get(page_url); page=driver.current_window_handle
            popup_url = f'chrome-extension://{EXTENSION_ID}/popup.html'
            if args.browser == 'firefox':
                driver.set_context('chrome')
                uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
                addon_id = json.loads((args.extension/'manifest.json').read_text(encoding='utf-8'))['browser_specific_settings']['gecko']['id']
                driver.set_context('content')
                popup_url = f'moz-extension://{uuids[addon_id]}/popup.html'
            driver.switch_to.new_window('tab'); inspector=driver.current_window_handle
            driver.get(popup_url)
            def message(value):
                driver.switch_to.window(inspector)
                return _extension_call(driver,'chrome.runtime.sendMessage(arguments[0]).then(done,e=>done({ok:false,error:String(e)}))',value)
            ping = message({'type':'ping'})
            assert ping.get('ok'), f'Real Host unavailable: {ping}'
            result['default_takeover_settings'] = {
                'enabled': ping.get('takeover_enabled'),
                'minimum_bytes': ping.get('takeover_minimum_bytes'),
            }
            assert ping.get('takeover_enabled') is True, f'Default takeover is not enabled: {ping}'
            assert ping.get('takeover_minimum_bytes') == 0, f'Default minimumBytes must be 0: {ping}'
            def enabled(value):
                r=message({'type':'set-takeover-settings','enabled':value,'minimumBytes':1024*1024})
                assert r.get('ok'), r

            def accepted_case(name, path=None, size=None, expected_bytes=None, no_content_length=False, filename=None):
                output_name=filename or Path(path or name + '.bin').name
                before={t['task_id'] for t in tasks()}
                _click_download(driver,page,name)
                wait(lambda:presenter_window(runtime=runtime),f'{name} confirmation missing',20)
                click_download_confirmation(evidence_path=args.report.with_name(f'{name}-click.json'), runtime=runtime)
                def completed():
                    found=[t for t in tasks() if t['task_id'] not in before
                        and (f'/{path or name + ".bin"}' in t['url'] if path else
                            t.get('filename') == output_name or Path(t.get('output_path','')).name == output_name)]
                    created.update(t['task_id'] for t in found)
                    if len(found)>1:
                        raise AssertionError(f'{name} created duplicate desktop tasks: {found}')
                    if found and found[-1]['status']=='failed':
                        raise AssertionError(found[-1])
                    return found[-1] if found and found[-1]['status']=='completed' else None
                task=wait(completed,f'{name} desktop task did not complete',60)
                file_path=Path(task['output_path'])
                payload=expected_bytes if expected_bytes is not None else _fixture_bytes(size or SMALL_SIZE)
                expected=hashlib.sha256(payload).hexdigest()
                assert file_path.is_file() and file_path.stat().st_size==len(payload), (name,file_path)
                assert hashlib.sha256(file_path.read_bytes()).hexdigest()==expected, (name,file_path)
                wait(lambda:not any(Path(str(item.get('filename',''))).name == output_name
                    for item in _download_items(driver,inspector)),
                    f'{name} browser copy remained after accepted takeover',20)
                wait(lambda:not list(downloads.glob(f'{output_name}*')),
                    f'{name} duplicate browser output',5)
                single_resolved_handoff(name,path,output_name if filename else None)
                if no_content_length:
                    header_sets=RealDownloadHandler.response_headers.get(path or name+'.bin',[])
                    assert header_sets and all(not any(header.lower()=='content-length' for header,_ in headers)
                        for headers in header_sets), \
                        f'{name} fixture unexpectedly sent Content-Length'
                record(f'{name}-accepted-desktop-task-single-file-sha256-no-browser-copy', {
                    'task_id':task['task_id'],'size':len(payload),'sha256':expected,
                    'browser_download_absent':True,'no_content_length':no_content_length,
                })
                return task
            def multipart_case(accepted):
                name='multipart-accept' if accepted else 'multipart-reject'
                path=f'/{name}.bin'
                element_id='multipart_accept' if accepted else 'multipart_reject'
                output_name=f'{name}.bin'
                before={task['task_id'] for task in tasks()}
                _click_download(driver,page,element_id)
                wait(lambda:presenter_window(runtime=runtime),f'{name} confirmation missing',20)
                click_download_confirmation(accept=accepted,
                    evidence_path=args.report.with_name(f'{name}-click.json'), runtime=runtime)
                payload=_fixture_bytes(SMALL_SIZE)
                expected=hashlib.sha256(payload).hexdigest()
                if accepted:
                    def completed():
                        found=[task for task in tasks() if task['task_id'] not in before
                            and (task.get('filename')==output_name
                                or Path(task.get('output_path','')).name==output_name)]
                        created.update(task['task_id'] for task in found)
                        if len(found)>1:
                            raise AssertionError(f'{name} created duplicate desktop tasks: {found}')
                        if found and found[-1]['status']=='failed':
                            raise AssertionError(found[-1])
                        return found[-1] if found and found[-1]['status']=='completed' else None
                    task=wait(completed,f'{name} desktop task did not complete',60)
                    output=Path(task['output_path'])
                    assert output.is_file() and output.stat().st_size==len(payload), (name,output)
                    assert hashlib.sha256(output.read_bytes()).hexdigest()==expected, (name,output)
                    wait(lambda:not any(Path(str(item.get('filename',''))).name==output_name
                        for item in _download_items(driver,inspector)),
                        f'{name} browser copy remained after accepted takeover',20)
                    assert not list(downloads.glob(f'{output_name}*')), f'{name} duplicate browser output'
                    assert len({task['task_id'] for task in tasks()}-before)==1, \
                        f'{name} did not create exactly one desktop task'
                    status='accepted'
                else:
                    def browser_completed():
                        matches=[item for item in _download_items(driver,inspector)
                            if Path(str(item.get('filename',''))).name==output_name]
                        if len(matches)>1:
                            raise AssertionError(f'{name} created duplicate browser downloads: {matches}')
                        if not matches or matches[0].get('state')!='complete':
                            return None
                        output=Path(str(matches[0].get('filename','')))
                        return matches[0] if output.is_file() else None
                    browser_item=wait(browser_completed,f'{name} browser download did not complete',30)
                    output=Path(browser_item['filename'])
                    assert browser_item.get('bytesReceived')==len(payload), browser_item
                    assert output.stat().st_size==len(payload), output
                    assert hashlib.sha256(output.read_bytes()).hexdigest()==expected, output
                    assert before=={task['task_id'] for task in tasks()}, \
                        f'{name} rejection created a desktop task'
                    status='rejected'
                assert RealDownloadHandler.multipart_requests.get(path,0)==1, \
                    f'{name} POST was replayed: {RealDownloadHandler.multipart_requests}'
                handoffs=[json.loads(encoded) for encoded in core({'type':'load_handoffs','request_id':7})['items']]
                matches=[handoff for handoff in handoffs
                    if handoff['id'] not in initial_handoffs
                    and handoff.get('offer',{}).get('filename')==output_name]
                assert len(matches)==1 and matches[0]['status']==status, \
                    f'{name} expected one {status} handoff: {matches}'
                record(f'{name}-single-post-{"desktop" if accepted else "browser"}-sha256',{
                    'handoff_id':matches[0]['id'],'status':status,'size':len(payload),
                    'task_id':task['task_id'] if accepted else None,
                    'sha256':expected,'post_requests':1,
                    'browser_download_absent':accepted,
                    'desktop_tasks_unchanged':not accepted,
                })
            if args.real_scenario == 'post':
                post_task=accepted_case('post',size=SMALL_SIZE,filename='post.bin')
                assert RealDownloadHandler.urlencoded_post_requests == 1, 'URL-encoded POST was replayed'
                record('urlencoded-post-not-replayed', {'task_id':post_task['task_id'],'post_requests':1})
                multipart_case(accepted=True)
                multipart_case(accepted=False)
                result['passed'] = True
                return result
            if args.real_scenario == 'all':
                accepted_case('small',size=SMALL_SIZE)
                accepted_case('stream',size=STREAM_SIZE,no_content_length=True)
                accepted_case('ordinary',size=SMALL_SIZE)
                accepted_case('quick',size=QUICK_SIZE)
                accepted_case('image','image.png',expected_bytes=IMAGE_PAYLOAD)
                accepted_case('json','export.json',expected_bytes=JSON_PAYLOAD)
                accepted_case('generic','generic',size=SMALL_SIZE)
                post_task=accepted_case('post',size=SMALL_SIZE,filename='post.bin')
                assert RealDownloadHandler.urlencoded_post_requests == 1, 'URL-encoded POST was replayed'
                record('urlencoded-post-not-replayed', {'task_id':post_task['task_id'],'post_requests':1})
                multipart_case(accepted=True)
                multipart_case(accepted=False)
                accepted_case('generated_blob',expected_bytes=LOCAL_DOWNLOAD_PAYLOAD,filename='generated-blob.bin')
                accepted_case('generated_data',expected_bytes=LOCAL_DOWNLOAD_PAYLOAD,filename='generated-data.bin')
                rejected_blob_filename='generated-blob-rejected.bin'
                tasks_before_rejection={task['task_id'] for task in tasks()}
                _click_download(driver,page,'generated_blob_reject')
                wait(lambda:presenter_window(runtime=runtime),'Generated Blob rejection confirmation missing',20)
                click_download_confirmation(accept=False,
                    evidence_path=args.report.with_name('generated-blob-reject-click.json'), runtime=runtime)
                def rejected_blob_completed():
                    matches=[item for item in _download_items(driver,inspector)
                        if Path(str(item.get('filename',''))).name==rejected_blob_filename]
                    if len(matches)>1:
                        raise AssertionError(f'Duplicate browser downloads for rejected Blob: {matches}')
                    if not matches or matches[0].get('state')!='complete':
                        return None
                    output=Path(str(matches[0].get('filename','')))
                    if not output.is_file():
                        return None
                    return matches[0]
                rejected_blob=wait(rejected_blob_completed,
                    'Rejected Blob did not complete in the browser',30)
                rejected_blob_path=Path(rejected_blob['filename'])
                rejected_blob_sha256=hashlib.sha256(LOCAL_DOWNLOAD_PAYLOAD).hexdigest()
                assert rejected_blob.get('bytesReceived')==len(LOCAL_DOWNLOAD_PAYLOAD), rejected_blob
                assert rejected_blob_path.stat().st_size==len(LOCAL_DOWNLOAD_PAYLOAD), rejected_blob_path
                assert hashlib.sha256(rejected_blob_path.read_bytes()).hexdigest()==rejected_blob_sha256
                browser_blob_files=list(downloads.glob(f'{rejected_blob_filename}*'))
                assert browser_blob_files==[rejected_blob_path], browser_blob_files
                assert tasks_before_rejection=={task['task_id'] for task in tasks()}, \
                    'Rejected local Blob created a desktop task'
                rejected_blob_handoffs=[handoff for encoded in core({'type':'load_handoffs','request_id':7})['items']
                    if (handoff:=json.loads(encoded))['id'] not in initial_handoffs
                    and handoff.get('offer',{}).get('filename')==rejected_blob_filename]
                assert len(rejected_blob_handoffs)==1 and rejected_blob_handoffs[0]['status']=='rejected', \
                    f'Expected one rejected local Blob handoff: {rejected_blob_handoffs}'
                record('generated-blob-rejection-keeps-browser-file-sha256', {
                    'handoff_id':rejected_blob_handoffs[0]['id'],
                    'browser_filename':rejected_blob_filename,
                    'size':len(LOCAL_DOWNLOAD_PAYLOAD),
                    'sha256':rejected_blob_sha256,
                    'desktop_tasks_unchanged':True,
                })
                alt_blob_filename='generated-blob-alt.bin'
                alt_blob_tasks_before={task['task_id'] for task in tasks()}
                alt_blob_handoffs_before={handoff['id'] for encoded in core({'type':'load_handoffs','request_id':7})['items']
                    if (handoff:=json.loads(encoded))}
                driver.switch_to.window(page)
                ActionChains(driver).key_down(Keys.ALT).click(
                    driver.find_element(By.ID,'generated_blob_alt')
                ).key_up(Keys.ALT).perform()
                def alt_blob_completed():
                    matches=[item for item in _download_items(driver,inspector)
                        if Path(str(item.get('filename',''))).name==alt_blob_filename]
                    if len(matches)>1:
                        raise AssertionError(f'Duplicate browser downloads for Alt Blob: {matches}')
                    if not matches or matches[0].get('state')!='complete':
                        return None
                    output=Path(str(matches[0].get('filename','')))
                    if not output.is_file():
                        return None
                    return matches[0]
                alt_blob=wait(alt_blob_completed,'Alt Blob did not complete in the browser',30)
                alt_blob_path=Path(alt_blob['filename'])
                local_payload_sha256=hashlib.sha256(LOCAL_DOWNLOAD_PAYLOAD).hexdigest()
                assert alt_blob.get('bytesReceived')==len(LOCAL_DOWNLOAD_PAYLOAD), alt_blob
                assert alt_blob_path.stat().st_size==len(LOCAL_DOWNLOAD_PAYLOAD), alt_blob_path
                assert hashlib.sha256(alt_blob_path.read_bytes()).hexdigest()==local_payload_sha256
                assert alt_blob_tasks_before=={task['task_id'] for task in tasks()}, \
                    'Alt-triggered local Blob created a desktop task'
                alt_blob_handoffs_after=[json.loads(encoded) for encoded in core({'type':'load_handoffs','request_id':7})['items']]
                assert alt_blob_handoffs_before=={handoff['id'] for handoff in alt_blob_handoffs_after}, \
                    'Alt-triggered local Blob created a handoff'
                assert not any(handoff['id'] not in initial_handoffs and handoff.get('offer',{}).get('filename')==alt_blob_filename
                    for handoff in alt_blob_handoffs_after), 'Alt-triggered local Blob was offered for handoff'
                record('generated-blob-alt-bypasses-takeover-browser-file-sha256', {
                    'browser_filename':alt_blob_filename,
                    'size':len(LOCAL_DOWNLOAD_PAYLOAD),
                    'sha256':local_payload_sha256,
                    'desktop_tasks_unchanged':True,
                    'handoffs_unchanged':True,
                })
                excluded_data_filename='generated-data-excluded.bin'
                excluded_data_tasks_before={task['task_id'] for task in tasks()}
                excluded_data_handoffs_before={handoff['id'] for encoded in core({'type':'load_handoffs','request_id':7})['items']
                    if (handoff:=json.loads(encoded))}
                _extension_call(driver,"chrome.storage.local.set({excludedHosts:['127.0.0.1']}).then(()=>done(true))")
                try:
                    driver.switch_to.window(page)
                    _click_download(driver,page,'generated_data_excluded')
                    def excluded_data_completed():
                        matches=[item for item in _download_items(driver,inspector)
                            if Path(str(item.get('filename',''))).name==excluded_data_filename]
                        if len(matches)>1:
                            raise AssertionError(f'Duplicate browser downloads for excluded data URL: {matches}')
                        if not matches or matches[0].get('state')!='complete':
                            return None
                        output=Path(str(matches[0].get('filename','')))
                        if not output.is_file():
                            return None
                        return matches[0]
                    excluded_data=wait(excluded_data_completed,
                        'Excluded data URL did not complete in the browser',30)
                    excluded_data_path=Path(excluded_data['filename'])
                    assert excluded_data.get('bytesReceived')==len(LOCAL_DOWNLOAD_PAYLOAD), excluded_data
                    assert excluded_data_path.stat().st_size==len(LOCAL_DOWNLOAD_PAYLOAD), excluded_data_path
                    assert hashlib.sha256(excluded_data_path.read_bytes()).hexdigest()==local_payload_sha256
                    assert excluded_data_tasks_before=={task['task_id'] for task in tasks()}, \
                        'Excluded local data URL created a desktop task'
                    excluded_data_handoffs_after=[json.loads(encoded) for encoded in core({'type':'load_handoffs','request_id':7})['items']]
                    assert excluded_data_handoffs_before=={handoff['id'] for handoff in excluded_data_handoffs_after}, \
                        'Excluded local data URL created a handoff'
                    assert not any(handoff['id'] not in initial_handoffs and handoff.get('offer',{}).get('filename')==excluded_data_filename
                        for handoff in excluded_data_handoffs_after), 'Excluded local data URL was offered for handoff'
                    record('generated-data-excluded-bypasses-takeover-browser-file-sha256', {
                        'browser_filename':excluded_data_filename,
                        'size':len(LOCAL_DOWNLOAD_PAYLOAD),
                        'sha256':local_payload_sha256,
                        'desktop_tasks_unchanged':True,
                        'handoffs_unchanged':True,
                    })
                finally:
                    _extension_call(driver,"chrome.storage.local.set({excludedHosts:[]}).then(()=>done(true))")
            if args.real_scenario != 'all':
                enabled(True)
                name = 'reject' if args.real_scenario == 'rejection' else 'fallback'
                _click_download(driver,page,name)
                wait(lambda:presenter_window(runtime=runtime),'Targeted confirmation missing',20)
                click_download_confirmation(accept=name=='fallback', evidence_path=args.report.with_name(name+'-click.json'), runtime=runtime)
                if name == 'fallback':
                    def targeted_failure():
                        found=[t for t in tasks() if t['task_id'] not in initial and name+'.bin' in t['url']]
                        created.update(t['task_id'] for t in found)
                        return found[-1] if found and found[-1]['status']=='failed' else None
                    record('targeted-desktop-http-failure', wait(targeted_failure,'Targeted failure task missing',20))
                record('targeted-browser-recovery-sha256',_assert_browser_completed(driver,inspector,name+'.bin'))
                single_resolved_handoff(name)
                result['passed']=True
                return result
            enabled(False)
            _click_download(driver,page,'disabled')
            record('takeover-disabled-browser-complete-sha256',_assert_browser_completed(driver,inspector,'disabled.bin'))
            assert initial | created == {t['task_id'] for t in tasks()}
            enabled(True)
            _extension_call(driver,"chrome.storage.local.set({excludedHosts:['127.0.0.1']}).then(()=>done(true))")
            _click_download(driver,page,'excluded')
            record('site-excluded-browser-complete-sha256',_assert_browser_completed(driver,inspector,'excluded.bin'))
            assert initial | created == {t['task_id'] for t in tasks()}
            _extension_call(driver,"chrome.storage.local.set({excludedHosts:[]}).then(()=>done(true))")
            before={t['task_id'] for t in tasks()}
            _click_download(driver,page,'accept')
            wait(lambda:presenter_window(runtime=runtime),'Takeover confirmation missing',20)
            paused=_item_for(driver,inspector,'accept.bin')
            if args.browser == 'firefox':
                assert paused is None or paused['state'] == 'interrupted', paused
            else:
                assert paused and paused['paused'], paused
            assert before=={t['task_id'] for t in tasks()}, 'Task began before confirmation'
            record('real-takeover-blocks-before-desktop-confirmation' if args.browser=='firefox' else 'real-takeover-pauses-before-desktop-confirmation',paused)
            click_download_confirmation(evidence_path=args.report.with_name('accept-click.json'), runtime=runtime)
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
            single_resolved_handoff('accept')
            _click_download(driver,page,'reject')
            wait(lambda:presenter_window(runtime=runtime),'Reject confirmation missing',20)
            result['reject_before_confirmation'] = _download_items(driver, inspector)
            args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf8')
            click_download_confirmation(accept=False, evidence_path=args.report.with_name('reject-click.json'), runtime=runtime)
            record('real-desktop-rejection-browser-resumes-sha256',_assert_browser_completed(driver,inspector,'reject.bin'))
            single_resolved_handoff('reject')
            assert len(created)==len([t for t in tasks() if t['task_id'] not in initial]), 'Rejected takeover created another task'
            _click_download(driver,page,'fallback')
            wait(lambda:presenter_window(runtime=runtime),'Fallback confirmation missing',20)
            result['fallback_before_confirmation'] = _download_items(driver, inspector)
            args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf8')
            click_download_confirmation(evidence_path=args.report.with_name('fallback-click.json'), runtime=runtime)
            def failed():
                found=[t for t in tasks() if t['task_id'] not in initial and 'fallback.bin' in t['url']]
                created.update(t['task_id'] for t in found)
                return found[-1] if found and found[-1]['status']=='failed' else None
            failure=wait(failed,'Accepted desktop HTTP failure was not reported',60)
            assert failure.get('error_message'),failure
            record('real-accepted-desktop-failure-browser-fallback-sha256',_assert_browser_completed(driver,inspector,'fallback.bin'))
            single_resolved_handoff('fallback')
            _click_download(driver,page,'disconnect')
            wait(lambda:presenter_window(runtime=runtime),'Disconnect confirmation missing',20)
            result['disconnect_before_host_stop'] = _download_items(driver, inspector)
            owned=[]
            for process in psutil.process_iter(['name','exe']):
                if process.info['name']!='HLSDownloaderNativeHost.exe' or not process.info['exe']:continue
                if Path(process.info['exe']).resolve()!=runtime/'app/resources/HLSDownloaderNativeHost.exe':continue
                with contextlib.suppress(psutil.NoSuchProcess,psutil.AccessDenied):
                    if any(parent.pid==driver.service.process.pid for parent in process.parents()):owned.append(process)
            assert owned,'Could not identify this isolated browser Native Host'
            stopped=[]
            for process in owned:
                try:
                    process.terminate();process.wait(5)
                    stopped.append(process.pid)
                except psutil.NoSuchProcess:
                    # 同一浏览器可能有多个旧通信端口；枚举后自行退出不等于产品失败。
                    pass
            assert stopped, 'No live browser Native Host was interrupted'
            result['interrupted_host_pids']=stopped
            result['disconnect_after_host_stop'] = _download_items(driver, inspector)
            click_download_confirmation(accept=False, runtime=runtime)
            result['disconnect_after_reject'] = _download_items(driver, inspector)
            args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8')
            record('actual-native-host-disconnect-pending-confirmation-browser-recovery-sha256',_assert_browser_completed(driver,inspector,'disconnect.bin'))
            assert message({'type':'ping'}).get('ok'),'Native Host did not reconnect'
            result['passed']=True
        except Exception as error:
            result['error']=str(error);result['traceback']=traceback.format_exc()
            try:
                result['failure_tasks'] = tasks()
                result['failure_browser_files'] = [{'name':file.name,'size':file.stat().st_size}
                    for file in downloads.iterdir() if file.is_file()]
                result['failure_handoffs'] = [json.loads(item) for item in core({'type':'load_handoffs','request_id':7})['items']
                    if server and json.loads(item).get('offer',{}).get('url','').startswith(f'http://127.0.0.1:{server.server_port}/')]
                if driver and inspector:
                    driver.switch_to.window(inspector)
                    result['failure_browser_downloads'] = _download_items(driver, inspector)
                    for item in result['failure_browser_downloads']:
                        output = Path(str(item.get('filename', '')))
                        if item.get('state') == 'complete' and output.is_file():
                            item['verified_bytes'] = output.stat().st_size
                            item['verified_sha256'] = hashlib.sha256(output.read_bytes()).hexdigest()
                    result['failure_extension_storage'] = _extension_call(driver,
                        'Promise.all([chrome.storage.local.get(null),chrome.storage.session.get(null)]).then(done)')
                    if args.browser == 'firefox':
                        driver.set_context('chrome')
                        result['failure_console'] = driver.execute_script("""
                            const {ConsoleAPIStorage} = ChromeUtils.importESModule('resource://gre/modules/ConsoleAPIStorage.sys.mjs');
                            return (ConsoleAPIStorage?.getEvents?.() || []).filter(event => String(event.filename).startsWith('moz-extension:'))
                                .slice(-80).map(event => ({level:event.level,filename:event.filename,
                                    arguments:event.arguments.map(value => String(value))}));
                        """)
                        driver.set_context('content')
                elif driver:
                    result['failure_browser_url'] = driver.current_url
                    result['failure_browser_title'] = driver.title
                    driver.save_screenshot(str(args.report.with_name('failure-navigation.png')))
                handle = presenter_window(runtime=runtime)
                if handle:
                    from smoke_v7_presenter_visual import capture
                    capture(handle, args.report.with_name('failure-confirmation.png'))
            except Exception as diagnostic_error:
                result['diagnostic_error'] = str(diagnostic_error)
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
