"""Replay a current browser HLS request in an isolated sandbox Core.

Request URLs and credentials remain in guest memory/local temporary files. The
shared report contains only HTTP status, byte counts and local decode results.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
from urllib.parse import urlsplit
import uuid

from selenium import webdriver
from selenium.webdriver.edge.options import Options
from selenium.webdriver.edge.service import Service

from smoke_v7_portable_app import Pipe
from smoke_v7_presenter import native_message


def record(engine, host, ffmpeg, resource):
    result = {"passed": False, "samples": []}
    with tempfile.TemporaryDirectory(prefix="live-replay-") as temporary:
        local = Path(temporary)
        downloads = local / "downloads"
        downloads.mkdir()
        pipe = rf"\\.\pipe\HLSDownloader.live-{uuid.uuid4().hex}"
        environment = {**os.environ, "HLS_V7_DATA_DIR": str(local / "profile"),
                       "HLS_V7_PIPE": pipe, "HLS_FFMPEG": str(ffmpeg)}
        processes = []
        def request(message):
            client = Pipe(pipe)
            try:
                return client.request(message)
            finally:
                client.close()
        with (local / "runtime.log").open("wb") as log:
            try:
                processes.append(subprocess.Popen([str(engine)], env=environment, stdout=log, stderr=log))
                deadline = time.monotonic() + 30
                while True:
                    try:
                        response = request({"type": "hello", "protocol": "hls-downloader-v7-core", "version": 1})
                        assert response["type"] == "hello"
                        break
                    except OSError:
                        if time.monotonic() > deadline:
                            raise TimeoutError("Core not ready")
                        time.sleep(.05)
                configured = request({"type": "store_settings", "request_id": 1, "values": {
                    "download_dir": str(downloads), "legal_terms_accepted": True,
                    "live_record_max_minutes": 1, "download_subtitles": False}})
                assert configured.get("type") != "error", "Could not configure isolated Core"
                native = subprocess.Popen([str(host)], env=environment, stdin=subprocess.PIPE,
                                          stdout=subprocess.PIPE, stderr=log)
                processes.append(native)
                offered = native_message(native, {"op": "download", "resource": resource})
                assert offered.get("ok"), "Native download request rejected"
                task_id = offered["task"]["task_id"]
                deadline = time.monotonic() + 120
                while True:
                    task = next(t for t in request({"type": "snapshot", "request_id": 2})["tasks"] if t["task_id"] == task_id)
                    sample = {key: task.get(key) for key in ("status", "is_live", "downloaded_bytes", "completed_ranges")}
                    if not result["samples"] or result["samples"][-1] != sample:
                        result["samples"].append(sample)
                    if task["status"] in ("completed", "failed", "canceled"):
                        break
                    if time.monotonic() > deadline:
                        raise TimeoutError("Live recording did not reach its one-minute limit")
                    time.sleep(.5)
                if task["status"] != "completed":
                    result["task_error"] = re.sub(r'https?://\S+', '[redacted URL]', task.get("error_message", ""))
                    raise AssertionError("Live task did not complete")
                output = Path(task["output_path"])
                result["output_bytes"] = output.stat().st_size
                decoded = subprocess.run([str(ffmpeg), "-v", "error", "-i", str(output),
                    "-map", "0:v:0", "-frames:v", "1", "-f", "null", "-"],
                    stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)
                result["video_decodes"] = decoded.returncode == 0
                result["live_detected"] = any(s["is_live"] and s["status"] == "recording" for s in result["samples"])
                result["task_cache_removed"] = not (downloads / ".hls-tasks" / task_id).exists()
                result["passed"] = result["video_decodes"] and result["live_detected"] and result["task_cache_removed"]
            except Exception as error:
                # Exceptions from browser/native APIs may contain signed URLs.
                result["error_type"] = type(error).__name__
            finally:
                for process in reversed(processes):
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=15)
    return result


def run(args):
    assert os.environ.get("HLS_VM_ACCEPTANCE") == "1", "Sandbox only"
    report = {"passed": False, "browser_manifest_statuses": []}
    with tempfile.TemporaryDirectory(prefix="live-browser-") as temporary:
        options = Options()
        options.add_argument("--no-first-run")
        options.add_argument("--autoplay-policy=no-user-gesture-required")
        options.add_argument("--user-data-dir=" + temporary)
        options.set_capability("ms:loggingPrefs", {"performance": "ALL"})
        browser = webdriver.Edge(service=Service(str(args.driver)), options=options)
        browser.set_page_load_timeout(35)
        requests, extra, successful = {}, {}, []
        try:
            browser.get(args.page)
            deadline = time.monotonic() + 40
            while time.monotonic() < deadline and not successful:
                for entry in browser.get_log("performance"):
                    event = json.loads(entry["message"])["message"]
                    params = event.get("params", {})
                    identity = params.get("requestId")
                    if event["method"] == "Network.requestWillBeSent":
                        value = params["request"]
                        if ".m3u8" in urlsplit(value["url"]).path:
                            requests[identity] = value
                    elif event["method"] == "Network.requestWillBeSentExtraInfo":
                        extra[identity] = params.get("headers", {})
                    elif event["method"] == "Network.responseReceived" and identity in requests:
                        response = params["response"]
                        report["browser_manifest_statuses"].append(response["status"])
                        if response["status"] == 200 and "_HLS_" not in requests[identity]["url"]:
                            successful.append(identity)
                time.sleep(.25)
            assert successful, "Browser did not load an HLS playlist; no verification/login is automated"
            identity = successful[0]
            request = requests[identity]
            headers = {k.lower(): str(v) for k, v in {**request["headers"], **extra.get(identity, {})}.items()
                       if not k.startswith(":") and k.lower() not in ("host", "connection", "content-length", "accept-encoding")}
            cookies = browser.execute_cdp_cmd("Network.getCookies", {"urls": [request["url"]]})["cookies"]
            resource = {"url": request["url"], "resource_kind": "hls", "filename": "live-fixture.mp4",
                        "source_page_url": args.page, "request_headers": headers,
                        "referer": headers.get("referer", args.page), "origin": headers.get("origin", ""),
                        "user_agent": browser.execute_script("return navigator.userAgent"),
                        "cookie": "; ".join(f'{c["name"]}={c["value"]}' for c in cookies)}
            report["candidate"] = record(args.engine, args.host, args.ffmpeg, resource)
            report["passed"] = report["candidate"]["passed"]
        except Exception as error:
            report["error_type"] = type(error).__name__
        finally:
            browser.quit()
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--page", required=True)
    for name in ("driver", "engine", "host", "ffmpeg", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    raise SystemExit(run(parser.parse_args()))
