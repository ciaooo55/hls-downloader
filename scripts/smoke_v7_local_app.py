"""Install and exercise a complete application with an isolated MSI identity."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading
import tempfile
from urllib.request import Request, urlopen
import uuid
import zipfile

from selenium import webdriver
from selenium.webdriver.edge.options import Options
from selenium.webdriver.edge.service import Service

from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, RangeHandler, Server, connect, send, wait_task
from smoke_extension_browsers import _chromium_extension_id
from smoke_extension_media import _find_edge
from smoke_extension_takeover import _registered_host, _wait_until
from smoke_v7_transfer_performance import free_port
from smoke_v7_presenter import native_message, wait_window


class Handler(RangeHandler):
    def do_GET(self):
        if self.path == "/page":
            data = b"<!doctype html><title>Installed download test</title><script>fetch('/file.bin',{headers:{Range:'bytes=0-0'}}).then(r=>r.arrayBuffer()).then(()=>document.body.dataset.ready='1')</script><body>Installed download test</body>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            super().do_GET()


def product_state(code):
    import ctypes
    from ctypes import wintypes
    query = ctypes.windll.msi.MsiQueryProductStateW
    query.argtypes = [wintypes.LPCWSTR]
    query.restype = ctypes.c_int
    return query(code)


def run(args):
    package = json.loads(args.package.read_text(encoding="utf-8-sig"))
    install = Path(package["install_dir"]).resolve()
    repo = Path(__file__).resolve().parents[1]
    allowed = (repo / ".tool-cache" / "test-tmp" / "full-app-validation-20261007").resolve()
    if not install.is_relative_to(allowed) or package.get("release_ready") is not False:
        raise ValueError("Only the isolated local-validation package is allowed")
    if args.system_temp:
        package["requested_install_dir"] = str(install)
        install = Path(tempfile.gettempdir()) / ("HLSDownloader-local-app-" + package["product_code"].strip("{}")) / "installed"
        package["install_dir"] = str(install)
    if product_state(package["product_code"]) != -1:
        raise ValueError("The isolated test product is already registered")
    evidence = args.package.parent
    result = {"passed": False, "scope": package["scope"], "package": package, "steps": []}
    processes = []
    driver = None
    server = None
    output_path = evidence / "installed-app.json"

    def check(name, passed, actual):
        result["steps"].append({"name": name, "passed": bool(passed), "actual": actual})
        output_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        if not passed:
            raise AssertionError(f"{name}: {actual}")

    def msi(action, target, name, *extra):
        return subprocess.run([str(Path(os.environ["SystemRoot"]) / "System32" / "msiexec.exe"), action, target, "/qn", "/norestart", "/l*v", str(evidence / name), *extra], timeout=90).returncode

    try:
        code = msi("/i", package["msi"], "full-install.log", f"INSTALLDIR={install}")
        check("installation", code in (0, 3010), code)
        check("registered", product_state(package["product_code"]) == 5, product_state(package["product_code"]))
        resources = install / "app" / "resources"
        files = [install / "HLSDownloader.exe", *(resources / name for name in ["HLSDownloaderEngine.exe", "HLSDownloaderNativeHost.exe", "HLSDownloaderPresenter.exe", "HLSDownloaderUpdater.exe", "ffmpeg.exe", "ffprobe.exe"])]
        check("complete-runtime-installed", all(p.is_file() for p in files), [str(p) for p in files])
        result["installed_sha256"] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files}
        provenance = json.loads((resources / "BUILD-PROVENANCE.json").read_text(encoding="utf-8"))
        check("honest-local-provenance", provenance.get("package_tier") == "local-validation" and provenance.get("git_worktree_clean") is False and provenance.get("release_ready") is False, provenance)

        root = install.parent / ("runtime-" + package["product_code"].strip("{}"))
        root.mkdir(parents=True, exist_ok=True)
        core_port, ui_port = free_port(), free_port()
        pipe = rf"\\.\pipe\HLSLocalInstalled-{uuid.uuid4().hex}"
        token = uuid.uuid4().hex
        env = os.environ.copy()
        env.update(LOCALAPPDATA=str(root / "local-appdata"), HLS_V7_DATA_DIR=str(root / "data"), HLS_V7_DOWNLOAD_DIR=str(root / "downloads"), HLS_V7_PIPE=pipe,
                   HLS_V7_CORE_TCP="1", HLS_V7_CORE_BIND=f"127.0.0.1:{core_port}", HLS_ENGINE_PATH=str(resources / "HLSDownloaderEngine.exe"),
                   HLS_PRESENTER_PATH=str(resources / "HLSDownloaderPresenter.exe"), HLS_UI_TEST_API="1", HLS_UI_TEST_TOKEN=token,
                   HLS_UI_TEST_PORT=str(ui_port), HLS_UI_AUDIT_WIDTH="1280", HLS_UI_AUDIT_HEIGHT="760",
                   JAVA_TOOL_OPTIONS=f"-Dhls.engine.pipe={pipe}")
        with (evidence / "core.stderr.log").open("wb") as log:
            processes.append(subprocess.Popen([str(resources / "HLSDownloaderEngine.exe")], env=env, stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW))
        with connect(core_port) as ipc:
            setting = send(ipc, {"type": "store_setting", "request_id": 1, "key": "download_dir", "value": str(root / "downloads")})
            check("installed-core-ipc", setting.get("type") != "error", setting)
        with (evidence / "presenter.stderr.log").open("wb") as log:
            processes.append(subprocess.Popen([str(resources / "HLSDownloaderPresenter.exe")], env=env, stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW))
        with (evidence / "ui.stderr.log").open("wb") as log:
            processes.append(subprocess.Popen([str(install / "HLSDownloader.exe")], cwd=install, env=env, stdout=log, stderr=log))

        def ui(path, raw=False):
            with urlopen(Request(f"http://127.0.0.1:{ui_port}/{path}", headers={"X-HLS-Test-Token": token}), timeout=5) as response:
                data = response.read()
            return data if raw else json.loads(data)

        def ready():
            try:
                health, window, state = ui("health"), ui("window"), ui("state")
                return (health, window, state) if health.get("ok") and window.get("showing") and "已连接" in state.get("engineText", "") else None
            except OSError:
                return None

        health, window, state = _wait_until(ready, "installed Compose connection", timeout=90)
        check("installed-workbench", health.get("version") == "7.0.2" and window.get("width") == 1280 and window.get("height") == 760 and window.get("iconCount", 0) > 0, {"health": health, "window": window, "state": state})
        (evidence / "installed-workbench-empty.png").write_bytes(ui("screenshot", raw=True))

        extension = root / "extension"
        with zipfile.ZipFile(resources / "extensions" / "HLSDownloader-7.0.2-Chromium.zip") as archive:
            archive.extractall(extension)
        manifest = root / "installed-host.json"
        extension_id = _chromium_extension_id(extension)
        manifest.write_text(json.dumps({"name": "com.ciaooo55.hls_downloader", "description": "Isolated installed production Host", "path": str(resources / "HLSDownloaderNativeHost.exe"), "type": "stdio", "allowed_origins": [f"chrome-extension://{extension_id}/"]}), encoding="utf-8")
        server = Server(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        page_url = f"http://127.0.0.1:{server.server_port}/page"
        download_url = f"http://127.0.0.1:{server.server_port}/file.bin"
        # 浏览器及其 Native Host 继承同一个隔离 pipe；注册上下文结束时恢复原值。
        with _registered_host(manifest, "edge"):
            options = Options()
            options.binary_location = str(_find_edge())
            for option in [f"--user-data-dir={root / 'edge-profile'}", "--headless=new", "--no-first-run", "--disable-features=DisableLoadExtensionCommandLineSwitch", f"--disable-extensions-except={extension}", f"--load-extension={extension}"]:
                options.add_argument(option)
            driver = webdriver.Edge(service=Service(executable_path=str(args.driver), env=env), options=options)
            driver.set_script_timeout(30)
            driver.get(page_url)
            _wait_until(lambda: driver.execute_script("return document.body.dataset.ready==='1'"), "captured HTTP response")
            driver.execute_script("window.open('about:blank')")
            driver.switch_to.window(driver.window_handles[-1])
            driver.get(f"chrome-extension://{extension_id}/popup.html")
            response = driver.execute_async_script("const done=arguments[arguments.length-1];chrome.runtime.sendMessage({type:'ping'},done)")
            check("installed-extension-native-ping", response.get("ok") is True, response)
            settings = driver.execute_async_script("const done=arguments[arguments.length-1];chrome.runtime.sendMessage({type:'subtitle-state-get'},done)")
            check("installed-subtitle-default-languages", settings.get("settings", {}).get("source") == "auto" and settings.get("settings", {}).get("target") == "zh", settings)
            provider = driver.execute_async_script("const done=arguments[arguments.length-1];chrome.runtime.sendMessage({type:'subtitle-provider',request:{action:'configuration'}},done)")
            check("installed-subtitle-provider-ipc", provider.get("ok") is True and provider.get("has_key") is False, provider)
            response = driver.execute_async_script("const [url,page,size,done]=arguments;chrome.tabs.query({},tabs=>{const tab=tabs.find(t=>t.url===page);if(!tab){done({error:'source tab absent'});return;}chrome.runtime.sendMessage({type:'download-now',resource:{url,pageUrl:page,tabId:tab.id,kind:'file',filename:'installed-file.bin',title:'Installed plugin download',size,statusCode:206,confidence:100,evidence:['response_headers']}},done);})", download_url, page_url, PAYLOAD_SIZE)
            check("installed-extension-download-request", response.get("ok") is True, response)
            with connect(core_port) as ipc:
                tasks = send(ipc, {"type": "snapshot", "request_id": 2}).get("tasks", [])
                task = next((t for t in tasks if t.get("url") == download_url), None)
                if task is None and len(tasks) == 1:
                    task = tasks[0]
                check("plugin-task-in-installed-core", task is not None, tasks)
                task = wait_task(ipc, task["task_id"], lambda t: t["status"] in ("completed", "failed"), 3)
                check("installed-download-completed", task["status"] == "completed", task)
                expected = hashlib.sha256(BLOCK * (PAYLOAD_SIZE // len(BLOCK))).hexdigest()
                actual = hashlib.sha256(Path(task["output_path"]).read_bytes()).hexdigest()
                check("installed-download-integrity", actual == expected and Path(task["output_path"]).stat().st_size == PAYLOAD_SIZE, {"bytes": PAYLOAD_SIZE, "sha256": actual})
                bounds = ui("state")["controlBounds"]["sidebar.status.已完成"]
                action = json.dumps({"type": "click", "x": (bounds[0] + bounds[2]) // 2, "y": (bounds[1] + bounds[3]) // 2}).encode()
                with urlopen(Request(f"http://127.0.0.1:{ui_port}/action", data=action, headers={"X-HLS-Test-Token": token, "Content-Type": "application/json"}), timeout=5) as response:
                    response.read()
                def completed_in_ui():
                    state = ui("state")
                    return state if state.get("activeFilter") == "已完成" and f"taskrow.{task['task_id']}" in state.get("controlBounds", {}) else None
                final_state = _wait_until(completed_in_ui, "completed download rendered in Compose", timeout=30)
                check("installed-workbench-completion", True, final_state)
                (evidence / "installed-workbench-downloaded.png").write_bytes(ui("screenshot", raw=True))
                result["final_ui_state"] = final_state
            driver.quit()
            driver = None
        host = subprocess.Popen([str(resources / "HLSDownloaderNativeHost.exe")], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)
        processes.append(host)
        offered = native_message(host, {"op": "offer", "resource": {"url": download_url + "?confirmation=1", "resource_kind": "file", "filename": "confirmation.bin", "title": "Installed Presenter verification", "size": PAYLOAD_SIZE, "client_request_id": "local-installed-presenter-" + uuid.uuid4().hex}})
        check("installed-presenter-offer", offered.get("ok") is True and bool(offered.get("handoff", {}).get("id")), offered)
        visible_ms = wait_window(processes[1].pid, "确认下载", True, 30)
        check("installed-presenter-visible", True, {"pid": processes[1].pid, "visible_wait_ms": visible_ms})
        rejected = native_message(host, {"op": "reject_handoff", "handoff_id": offered["handoff"]["id"]})
        wait_window(processes[1].pid, "确认下载", False, 10)
        check("installed-presenter-reject", rejected.get("ok") is True, rejected)
        result["passed"] = True
    except Exception as error:
        result["error"] = str(error)
    finally:
        if driver:
            driver.quit()
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        if server:
            server.shutdown()
            server.server_close()
        if product_state(package["product_code"]) >= 1:
            try:
                code = msi("/x", package["product_code"], "full-uninstall.log")
                check("uninstall", code in (0, 3010) and product_state(package["product_code"]) == -1 and not (install / "HLSDownloader.exe").exists(), code)
            except Exception as error:
                result["passed"] = False
                result["cleanup_error"] = str(error)
        output_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--driver", type=Path, required=True)
    parser.add_argument("--system-temp", action="store_true", help="Use an isolated system-temp installation when the workspace volume is full")
    raise SystemExit(0 if run(parser.parse_args())["passed"] else 1)
