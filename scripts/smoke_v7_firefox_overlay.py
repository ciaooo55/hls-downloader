"""Verify a real Firefox overlay click and visible Native Messaging rejection."""
from __future__ import annotations

import argparse
import functools
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import uuid
import zipfile

from selenium import webdriver
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service

from smoke_extension_media import _find_firefox
from smoke_v7_portable_app import Pipe, wait


class Handler(SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass


def run(args):
    runtime = args.runtime.resolve()
    extension = args.extension.resolve()
    manifest = json.loads((extension / "manifest.json").read_text(encoding="utf-8"))
    report = {"passed": False, "steps": []}
    driver = server = pipe = None
    task_ids = set()
    args.report.parent.mkdir(parents=True, exist_ok=True)

    def check(name, actual):
        report["steps"].append({"name": name, "actual": actual})
        print(json.dumps({"passed": name}, ensure_ascii=False), flush=True)

    with tempfile.TemporaryDirectory(prefix="firefox-overlay-", dir=runtime.parents[1] / "test-tmp") as temporary:
        stage = Path(temporary)
        try:
            media_name = f"overlay-regression-{uuid.uuid4().hex}.mp4"
            media = stage / media_name
            subprocess.run([str(runtime / "app/resources/ffmpeg.exe"), "-hide_banner", "-loglevel", "error",
                            "-f", "lavfi", "-i", "testsrc=size=640x360:rate=15", "-t", "4",
                            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(media)],
                           check=True, timeout=60)
            (stage / "index.html").write_text(
                f'<title>Firefox overlay regression</title><video style="width:640px;height:360px" '
                f'src="/{media_name}" controls muted autoplay loop></video>', encoding="utf-8")
            server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(stage)))
            threading.Thread(target=server.serve_forever, daemon=True).start()
            url = f"http://127.0.0.1:{server.server_port}/index.html"
            resource_url = f"http://127.0.0.1:{server.server_port}/{media_name}"
            pipe = Pipe()
            original_tasks = {t["task_id"] for t in pipe.request({"type": "snapshot", "request_id": 1})["tasks"]}
            options = Options()
            options.binary_location = str(_find_firefox())
            options.set_preference("media.autoplay.default", 0)
            service = Service(executable_path=str(args.driver),
                              service_args=["--profile-root", str(stage)],
                              log_output=str(args.report.with_suffix(".gecko.log")))
            driver = webdriver.Firefox(options=options, service=service)
            driver.set_page_load_timeout(30)
            driver.set_window_size(1100, 750)

            def addon_file(name, extension_id=None):
                path = stage / name
                with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
                    for source in sorted(extension.rglob("*")):
                        if not source.is_file():
                            continue
                        relative = source.relative_to(extension).as_posix()
                        if relative == "manifest.json" and extension_id:
                            altered = json.loads(source.read_text(encoding="utf-8"))
                            altered["browser_specific_settings"]["gecko"]["id"] = extension_id
                            archive.writestr(relative, json.dumps(altered))
                        else:
                            archive.write(source, relative)
                return path

            def open_player():
                driver.get(url)
                wait(lambda: driver.execute_script("return document.querySelector('video').currentTime>0"),
                     "Fixture video did not play", 30)
                return wait(lambda: driver.execute_script(
                    "return document.querySelector('hls-downloader-media-panel')?.shadowRoot"
                    "?.querySelector('.video-download:not(.identifying)') || null"),
                    "Playback download button did not appear", 30)

            good_id = driver.install_addon(str(addon_file("valid.xpi")), temporary=True)
            assert good_id == manifest["browser_specific_settings"]["gecko"]["id"]
            button = open_player()
            button.click()

            def download_task():
                current = pipe.request({"type": "snapshot", "request_id": 2})["tasks"]
                for task in current:
                    if task["task_id"] not in original_tasks and task.get("url") == resource_url:
                        task_ids.add(task["task_id"])
                        return task if task["status"] in ("completed", "failed") else None
                return None

            task = wait(download_task, "Overlay click did not complete a download", 45)
            assert task["status"] == "completed", task.get("log_tail")
            assert hashlib.sha256(Path(task["output_path"]).read_bytes()).digest() == hashlib.sha256(media.read_bytes()).digest()
            check("content-click-host-core-file-sha256", {"status": task["status"], "bytes": media.stat().st_size})
            driver.save_screenshot(str(args.report.with_name("overlay-download-success.png")))
            pipe.request({"type": "command", "request_id": 4,
                          "command": {"kind": "task_action", "task_id": task["task_id"], "action": "delete_files"}})
            task_ids.discard(task["task_id"])
            driver.uninstall_addon(good_id)
            driver.install_addon(str(stage / "valid.xpi"), temporary=True)
            button = open_player()
            button.click()
            task = wait(download_task, "Cached playback click was rejected", 45)
            assert task["status"] == "completed", task.get("log_tail")
            assert hashlib.sha256(Path(task["output_path"]).read_bytes()).digest() == hashlib.sha256(media.read_bytes()).digest()
            check("reinstalled-extension-cached-video-download", {"status": task["status"], "bytes": media.stat().st_size})
            driver.uninstall_addon(good_id)
            driver.install_addon(str(addon_file("rejected.xpi", "hls-overlay-rejection-test@local")), temporary=True)
            button = open_player()
            button.click()
            error = wait(lambda: driver.execute_script(
                "return document.querySelector('hls-downloader-media-panel')?.shadowRoot"
                "?.querySelector('.video-hover .send-error')?.textContent || ''"),
                "Native Messaging rejection was hidden from the hover card", 30)
            assert error.strip()
            check("unregistered-extension-connection-failure-visible-in-hover-card", error)
            driver.save_screenshot(str(args.report.with_name("overlay-visible-error.png")))
            report["passed"] = True
        except Exception as error:
            report["error"] = str(error)
            if driver:
                driver.save_screenshot(str(args.report.with_name("overlay-failure.png")))
            print(json.dumps({"error": str(error)}, ensure_ascii=False), flush=True)
        finally:
            if driver:
                driver.quit()
            if pipe:
                for task_id in task_ids:
                    pipe.request({"type": "command", "request_id": 3,
                                  "command": {"kind": "task_action", "task_id": task_id, "action": "delete_files"}})
                pipe.close()
            if server:
                server.shutdown()
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--extension", type=Path, required=True)
    parser.add_argument("--driver", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
