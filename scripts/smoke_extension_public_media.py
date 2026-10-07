"""Identify media on public player pages with an isolated production extension.

These pages exercise upstream players and network/CDN behavior. They supplement
the controlled regressions; they do not prove coverage of every website.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import tempfile
import time
from urllib.parse import urlencode
import zipfile

from selenium import webdriver
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService
from selenium.common.exceptions import TimeoutException
from selenium.webdriver.common.by import By

from smoke_extension_media import _find_edge, _find_firefox, _overlay_state, _resource_id


HLS = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8"
DASH = "https://dash.akamaized.net/akamai/bbb_30fps/bbb_30fps.mpd"
CASES = [
    ("hls-js", "https://hlsjs.video-dev.org/demo/?" + urlencode({"src": HLS}), HLS),
    ("dash-js", "https://reference.dashif.org/dash.js/latest/samples/getting-started/manual-load-single-video.html", DASH),
    ("mdn-video", "https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html", "https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm"),
]


def play_frames(driver):
    driver.execute_script("document.querySelectorAll('video').forEach(v=>{v.muted=true;v.play().catch(()=>{});})")
    for frame in driver.find_elements(By.TAG_NAME, "iframe"):
        try:
            driver.switch_to.frame(frame)
            play_frames(driver)
        finally:
            driver.switch_to.parent_frame()


def run(args):
    extension = args.extension.resolve()
    if not (extension / "manifest.json").is_file():
        raise ValueError("Build the extension before testing public players")
    temp_root = Path(__file__).resolve().parents[1] / ".tool-cache" / "test-tmp"
    temp_root.mkdir(parents=True, exist_ok=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    result = {"browser": args.browser, "passed": False, "cases": [], "extension_sha256": {
        str(p.relative_to(extension)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted(extension.rglob("*")) if p.is_file()
    }}
    with tempfile.TemporaryDirectory(prefix="public-media-", dir=temp_root) as temporary:
        root = Path(temporary)
        driver = None
        try:
            if args.browser == "edge":
                options = EdgeOptions()
                options.binary_location = str(_find_edge())
                for value in [f"--user-data-dir={root / 'profile'}", "--headless=new",
                              "--disable-features=DisableLoadExtensionCommandLineSwitch",
                              f"--disable-extensions-except={extension}", f"--load-extension={extension}",
                              "--autoplay-policy=no-user-gesture-required", "--no-first-run", "--window-size=1280,900"]:
                    options.add_argument(value)
                service = EdgeService(executable_path=str(args.driver))
                driver = webdriver.Edge(service=service, options=options)
            else:
                addon = root / "extension.xpi"
                with zipfile.ZipFile(addon, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                    for p in sorted(extension.rglob("*")):
                        if p.is_file(): archive.write(p, p.relative_to(extension).as_posix())
                options = FirefoxOptions()
                options.binary_location = str(_find_firefox())
                options.add_argument("-headless")
                options.set_preference("media.autoplay.default", 0)
                options.set_preference("media.autoplay.blocking_policy", 0)
                service = FirefoxService(executable_path=str(args.driver),
                                         service_args=["--profile-root", str(root)])
                driver = webdriver.Firefox(service=service, options=options)
                driver.install_addon(str(addon), temporary=True)
                driver.set_window_size(1280, 900)
            driver.set_page_load_timeout(30)
            for name, url, expected in CASES:
                if args.case and name not in args.case: continue
                record = {"name": name, "page": url, "expected_media": expected, "passed": False}
                try:
                    try: driver.get(url)
                    except TimeoutException: record["page_load_timed_out"] = True
                    deadline = time.monotonic() + 45
                    while time.monotonic() < deadline:
                        driver.switch_to.default_content()
                        play_frames(driver)
                        state = _overlay_state(driver)
                        record["state"] = state
                        # The button must point at the requested manifest/media, not merely any detected URL.
                        if _resource_id(expected) in state["resourceIds"] and any(v["currentTime"] > 0 for v in state["videos"]):
                            record["passed"] = True
                            break
                        time.sleep(.5)
                    driver.save_screenshot(str(args.report.with_name(f"{args.report.stem}-{name}.png")))
                    if not record["passed"]:
                        record["page_text"] = driver.execute_script("return document.body.innerText.slice(0,6000)")
                except Exception as error:
                    record["error"] = str(error)
                result["cases"].append(record)
                print(json.dumps({"case": name, "passed": record["passed"]}), flush=True)
                args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
            result["passed"] = bool(result["cases"]) and all(c["passed"] for c in result["cases"])
            return result
        finally:
            if driver: driver.quit()
            args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--extension", type=Path, required=True)
    parser.add_argument("--driver", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--browser", choices=("edge", "firefox"), default="edge")
    parser.add_argument("--case", choices=tuple(c[0] for c in CASES), action="append")
    raise SystemExit(0 if run(parser.parse_args())["passed"] else 1)
