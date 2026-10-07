"""Use the packaged Firefox extension, registered Host and default portable Core."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import threading
from urllib.request import urlopen
import winreg
import zipfile

from selenium import webdriver
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service

from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, Server
from smoke_extension_media import _find_firefox
from smoke_v7_local_app import Handler
from smoke_v7_portable_app import Pipe, wait


def run(args):
    root = args.runtime.resolve()
    extension = root / "extensions/Firefox"
    manifest = json.loads((extension / "manifest.json").read_text(encoding="utf-8"))
    eid = manifest["browser_specific_settings"]["gecko"]["id"]
    report = {"passed": False, "runtime": str(root), "steps": []}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    environment = {k: v for k, v in os.environ.items() if not k.startswith("HLS_")}
    server = None
    driver = None
    pipe = None
    created_tasks = []

    def check(name, passed, actual):
        report["steps"].append({"name": name, "passed": bool(passed), "actual": actual})
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({"step": name, "passed": bool(passed)}), flush=True)
        if not passed:
            raise AssertionError(f"{name}: {actual}")

    with tempfile.TemporaryDirectory(prefix="portable-browser-", dir=root.parents[1] / "test-tmp") as temporary:
        stage = Path(temporary)
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Mozilla\NativeMessagingHosts\com.ciaooo55.hls_downloader") as key:
                host_path = Path(winreg.QueryValueEx(key, "")[0])
            registration = json.loads(host_path.read_text(encoding="utf-8-sig"))
            check("real-native-host-registration", Path(registration["path"]) == root / "app/resources/HLSDownloaderNativeHost.exe" and eid in registration["allowed_extensions"], registration)
            addon = stage / "extension.xpi"
            with zipfile.ZipFile(addon, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for p in sorted(extension.rglob("*")):
                    if p.is_file():
                        archive.write(p, p.relative_to(extension).as_posix())
            options = Options()
            options.binary_location = str(_find_firefox())
            options.add_argument("-headless")
            service = Service(executable_path=str(args.driver), env=environment,
                              service_args=["--profile-root", str(stage), "--allow-system-access"],
                              log_output=str(args.report.with_suffix(".gecko.log")))
            driver = webdriver.Firefox(service=service, options=options)
            driver.set_page_load_timeout(30)
            driver.set_script_timeout(20)
            check("packaged-extension-loads", driver.install_addon(str(addon), temporary=True) == eid, manifest["version"])
            driver.set_context("chrome")
            uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
            driver.set_context("content")
            server = Server(("127.0.0.1", 0), Handler)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            page_url = f"http://127.0.0.1:{server.server_port}/page"
            driver.get(page_url)
            wait(lambda: driver.execute_script("return document.body.dataset.ready==='1'"), "Page response not captured", 30)
            driver.switch_to.new_window("tab")
            driver.get(f"moz-extension://{uuids[eid]}/popup.html")

            def message(value):
                return driver.execute_async_script("const [message,done]=arguments;browser.runtime.sendMessage(message).then(done,e=>done({ok:false,error:String(e)}))", value)

            tabs = driver.execute_async_script("const done=arguments[0];browser.tabs.query({}).then(done)")
            tab_id = next(t["id"] for t in tabs if t.get("url") == page_url)
            ping = message({"type": "ping"})
            check("browser-host-core-default-ping", ping.get("ok") is True, ping)
            subtitle = message({"type": "subtitle-state-get"})
            check("subtitle-auto-to-chinese-default", subtitle.get("settings", {}).get("source") == "auto" and subtitle.get("settings", {}).get("target") == "zh", subtitle)
            provider = message({"type": "subtitle-provider", "request": {"action": "configuration"}})
            check("browser-subtitle-provider-configuration", provider.get("ok") is True and provider.get("has_key") is False, provider)
            url = f"http://127.0.0.1:{server.server_port}/file.bin"

            def resource():
                values = message({"type": "list", "tabId": tab_id, "pageUrl": page_url})
                return next((r for r in values if r.get("url") == url), None) if isinstance(values, list) else None

            detected = wait(resource, "Plugin did not recognize the actual HTTP response", 30)
            check("precise-response-recognition", detected.get("kind") == "file" and detected.get("size") == PAYLOAD_SIZE, detected)
            pipe = Pipe()
            pipe.request({"type": "hello", "protocol": "hls-downloader-v7-core", "version": 1})

            def download(value, expected_bytes, expected_hash, name):
                requested = message({"type": "download-now", "resource": {**value, "tabId": tab_id, "pageUrl": page_url}})
                check(name+"-plugin-request", requested.get("ok") is True, requested)

                def task():
                    tasks = pipe.request({"type": "snapshot", "request_id": 10})["tasks"]
                    return next((t for t in tasks if t.get("url") == value["url"]), None)

                current = wait(task, "Plugin task absent from Core", 20)
                task_id = current["task_id"]
                created_tasks.append(task_id)
                completed = wait(lambda: (t if (t := task())["status"] in ("completed", "failed") else None), "Plugin download did not complete", 90)
                check(name+"-completed", completed["status"] == "completed", completed)
                output = Path(completed["output_path"])
                actual = hashlib.sha256(output.read_bytes()).hexdigest()
                check(name+"-integrity", output.stat().st_size == expected_bytes and actual == expected_hash and output.is_relative_to(root), {"bytes": output.stat().st_size, "sha256": actual, "path": str(output)})
                deleted = pipe.request({"type": "command", "request_id": 11, "command": {"kind": "task_action", "task_id": task_id, "action": "delete_files"}})
                check(name+"-cleaned", deleted.get("type") != "error" and not output.exists(), {"task_id": task_id})
                created_tasks.remove(task_id)

            download({**detected, "filename": "portable-browser.bin"}, PAYLOAD_SIZE,
                     hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest(), "local-32mib")
            public_url = "https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm"
            with urlopen(public_url, timeout=30) as response:
                expected = response.read(8*1024*1024+1)
            if not expected or len(expected) > 8*1024*1024:
                raise ValueError("Unexpected public media fixture")
            driver.switch_to.new_window("tab")
            driver.get("https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html")
            driver.execute_script("document.querySelectorAll('video').forEach(v=>{v.muted=true;v.play().catch(()=>{});})")
            driver.switch_to.window(driver.window_handles[1])
            tabs = driver.execute_async_script("const done=arguments[0];browser.tabs.query({}).then(done)")
            tab_id = next(t["id"] for t in tabs if t.get("url", "").startswith("https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html"))
            page_url = "https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html"
            url = public_url
            public_resource = wait(resource, "Public player media URL not recognized", 45)
            check("precise-public-media-recognition", public_resource.get("url") == public_url, public_resource)
            download({**public_resource, "filename": "portable-public-flower.webm"}, len(expected), hashlib.sha256(expected).hexdigest(), "public-https-media")
            driver.save_screenshot(str(args.report.with_suffix(".png")))
            check("user-profile-left-empty", pipe.request({"type": "snapshot", "request_id": 12})["tasks"] == [], {})
            report["passed"] = True
        except Exception as error:
            report["error"] = str(error)
            print(str(error), flush=True)
        finally:
            if driver:
                driver.quit()
            if pipe:
                for task_id in created_tasks:
                    pipe.request({"type": "command", "request_id": 13, "command": {"kind": "task_action", "task_id": task_id, "action": "delete_files"}})
                pipe.close()
            if server:
                server.shutdown()
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--driver", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
