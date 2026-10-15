"""Click an original public download link and verify real Host/Core takeover."""
from __future__ import annotations

import argparse
import hashlib
import json
import ssl
from pathlib import Path
import tempfile
from urllib.parse import urlparse
from urllib.request import urlopen

import certifi

from selenium.webdriver.common.by import By

from smoke_extension_takeover import _download_items, _extension_call
from smoke_qwencloud_public_subtitles import _open_driver
from smoke_v7_browser_download_confirmation import click_download_confirmation, presenter_window
from smoke_v7_portable_app import Pipe, wait


def run(args):
    report = {"passed": False, "browser": args.browser,
              "page": "https://www.7-zip.org/download.html", "checks": []}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    report["components"] = {name: hashlib.sha256((args.runtime / "app/resources" / name).read_bytes()).hexdigest()
                            for name in ("HLSDownloaderEngine.exe", "HLSDownloaderNativeHost.exe", "HLSDownloaderPresenter.exe")}
    report["background_sha256"] = hashlib.sha256((args.extension / "background.js").read_bytes()).hexdigest()
    created = set()
    driver = None

    def core(request):
        pipe = Pipe()
        try:
            return pipe.request(request)
        finally:
            pipe.close()

    def tasks():
        return core({"type": "snapshot", "request_id": 1})["tasks"]

    def record(name, actual=True):
        report["checks"].append({"name": name, "actual": actual})
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")

    initial = None
    temporary = tempfile.TemporaryDirectory(prefix="public-takeover-")
    try:
        initial = {task["task_id"] for task in tasks()}
        driver, popup_url, extension_id = _open_driver(args, args.extension, Path(temporary.name) / "profile")
        driver.set_script_timeout(25)
        driver.set_page_load_timeout(45)
        driver.get(report["page"])
        page = driver.current_window_handle
        link = wait(lambda: next(iter(driver.find_elements(By.CSS_SELECTOR, "a[href$='-src.tar.xz']")), None),
                    "Official source download link missing", 20)
        url = link.get_attribute("href")
        if urlparse(url).hostname not in {"github.com", "www.7-zip.org", "7-zip.org"}:
            raise ValueError("Unexpected official download host")
        filename = Path(urlparse(url).path).name
        with urlopen(url, timeout=40, context=ssl.create_default_context(cafile=certifi.where())) as source:
            expected = source.read(32 * 1024 * 1024 + 1)
        if not expected.startswith(b"\xfd7zXZ\x00") or len(expected) > 32 * 1024 * 1024:
            raise ValueError("Unexpected public source archive size")
        expected_hash = hashlib.sha256(expected).hexdigest()
        record("independent-public-source", {"url": url, "bytes": len(expected), "sha256": expected_hash})
        driver.switch_to.new_window("tab")
        inspector = driver.current_window_handle
        driver.get(popup_url)
        ping = _extension_call(driver, "chrome.runtime.sendMessage({type:'ping'}).then(done,e=>done({error:String(e)}))")
        if not ping.get("ok") or ping.get("takeover_enabled") is not True or ping.get("takeover_minimum_bytes") != 0:
            raise RuntimeError(f"Default real Host takeover unavailable: {ping}")
        report["extension_id"] = extension_id
        record("default-takeover-enabled-for-all-sizes")
        driver.switch_to.window(page)
        link.click()
        wait(lambda: presenter_window(runtime=args.runtime), "Public browser download did not reach desktop confirmation", 25)
        if {task["task_id"] for task in tasks()} != initial:
            raise AssertionError("Core task started before actual confirmation")
        record("original-website-link-reaches-confirmation-before-task")
        click_download_confirmation(runtime=args.runtime, evidence_path=args.report.with_name("accept.json"))

        def completed():
            found = [task for task in tasks() if task["task_id"] not in initial and task.get("filename") == filename]
            created.update(task["task_id"] for task in found)
            if len(found) > 1:
                raise AssertionError("Public download created duplicate Core tasks")
            return found[0] if found and found[0]["status"] in {"completed", "failed"} else None

        task = wait(completed, "Public source archive did not finish through Core", 100)
        if task["status"] != "completed":
            raise AssertionError(task.get("error_message") or task)
        output = Path(task["output_path"])
        if output.stat().st_size != len(expected) or hashlib.sha256(output.read_bytes()).hexdigest() != expected_hash:
            raise AssertionError("Core public download differs from independent source")
        record("one-Core-file-completed-with-source-SHA256", {"bytes": len(expected), "sha256": expected_hash})

        def no_browser_copy():
            items = [item for item in _download_items(driver, inspector)
                     if filename in str(item.get("url", "")) or Path(item.get("filename", "")).name == filename]
            return {"items": items} if not any(item.get("exists") for item in items) else None

        record("browser-keeps-no-second-file", wait(no_browser_copy, "Browser kept a duplicate downloaded file", 20))
        driver.switch_to.window(page)
        driver.save_screenshot(str(args.report.with_suffix(".png")))
        report["passed"] = True
    except Exception as error:
        report["error"] = str(error)
        if driver is not None:
            try:
                driver.save_screenshot(str(args.report.with_suffix(".failure.png")))
            except Exception:
                pass
    finally:
        if driver is not None:
            try:
                driver.quit()
            except Exception as error:
                report["browser_cleanup_error"] = str(error)
                report["passed"] = False
        try:
            temporary.cleanup()
        except Exception as error:
            report["profile_cleanup_error"] = str(error)
            report["passed"] = False
        try:
            for task_id in created:
                core({"type": "command", "request_id": 2, "command": {
                    "kind": "task_action", "task_id": task_id, "action": "delete_files"}})
            if initial is not None:
                report["original_tasks_preserved"] = {task["task_id"] for task in tasks()} == initial
                if not report["original_tasks_preserved"]:
                    report["passed"] = False
        except Exception as error:
            report["task_cleanup_error"] = str(error)
            report["passed"] = False
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser", choices=("edge", "firefox"), required=True)
    parser.add_argument("--extension", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--driver", required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--headed", action="store_true")
    result = run(parser.parse_args())
    print(json.dumps(result, ensure_ascii=False, indent=2))
    raise SystemExit(0 if result["passed"] else 1)
