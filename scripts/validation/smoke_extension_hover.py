"""Real browser regression for player-specific, pointer-triggered media controls."""
from __future__ import annotations

import argparse
import functools
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import tempfile
import threading
import time
import zipfile

from selenium import webdriver
from selenium.webdriver import ActionChains
from selenium.webdriver.edge.options import Options
from selenium.webdriver.edge.service import Service
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService

from smoke_extension_media import QuietStaticHandler, _make_media, _overlay_state, _current_frame_overlay_state


def wait(condition, description, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = condition()
        if value:
            return value
        time.sleep(.1)
    raise AssertionError(description)


def run(args):
    if os.environ.get("HLS_VM_ACCEPTANCE") != "1":
        raise RuntimeError("Run this test inside the acceptance sandbox")
    report = {"passed": False, "browser": args.browser, "cases": []}
    with tempfile.TemporaryDirectory(prefix="media-hover-") as temporary:
        root = Path(temporary)
        site = root / "site"
        site.mkdir()
        _make_media(site, str(args.ffmpeg))
        server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(QuietStaticHandler, directory=str(site)))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        options = Options()
        options.add_argument("--headless=new")
        options.add_argument("--disable-features=DisableLoadExtensionCommandLineSwitch")
        options.add_argument(f"--load-extension={args.extension.resolve()}")
        options.add_argument(f"--disable-extensions-except={args.extension.resolve()}")
        options.add_argument(f"--user-data-dir={root / 'profile'}")
        options.add_argument("--autoplay-policy=no-user-gesture-required")
        options.add_argument("--no-first-run")
        options.add_argument("--window-size=1280,1000")
        driver = None
        try:
            if args.browser == "firefox":
                firefox = FirefoxOptions()
                firefox.binary_location = str(args.browser_binary)
                firefox.add_argument("-headless")
                firefox.set_preference("media.autoplay.default", 0)
                firefox.set_preference("media.autoplay.blocking_policy", 0)
                firefox.set_preference("browser.shell.checkDefaultBrowser", False)
                firefox.set_preference("browser.aboutwelcome.enabled", False)
                driver = webdriver.Firefox(service=FirefoxService(str(args.driver)), options=firefox)
                addon = root / "extension.xpi"
                with zipfile.ZipFile(addon, "w", zipfile.ZIP_DEFLATED) as archive:
                    for path in args.extension.rglob("*"):
                        if path.is_file():
                            archive.write(path, path.relative_to(args.extension).as_posix())
                driver.install_addon(str(addon), temporary=True)
            else:
                driver = webdriver.Edge(service=Service(str(args.driver)), options=options)
            for mode in ("direct", "dual-source", "shadow", "dynamic-shadow", "mse", "hls-mse", "ll-hls-mse", "dash-mse", "multi-mse", "dash-unknown-mse", "iframe", "cross-iframe", "srcdoc"):
                driver.switch_to.default_content()
                driver.get(f"http://127.0.0.1:{server.server_port}/index.html?mode={mode}")
                if mode in ("iframe", "cross-iframe", "srcdoc"):
                    frame = wait(lambda: next(iter(driver.find_elements("tag name", "iframe")), None), "Frame missing")
                    driver.switch_to.frame(frame)
                    if mode == "srcdoc":
                        driver.execute_script("document.body.prepend(Object.assign(document.createElement('h1'),{textContent:'srcdoc fixture'}))")
                state_now = lambda: _current_frame_overlay_state(driver)
                expected = 2 if mode == "multi-mse" else 1
                wait(lambda: sum(not v["paused"] and v["currentTime"] > 0 for v in state_now()["videos"]) == expected,
                     f"{mode}: media did not actually play")
                # Move outside the players before asserting the default state.
                ActionChains(driver).move_to_element(driver.find_element("tag name", "h1")).perform()
                time.sleep(.7)
                assert not state_now()["labels"], f"{mode}: unsolicited overlay"
                videos = driver.execute_script("""
                    const videos=[];
                    function visit(root){root.querySelectorAll('video').forEach(v=>videos.push(v));
                      root.querySelectorAll('*').forEach(e=>{if(e.shadowRoot)visit(e.shadowRoot)});}
                    visit(document);return videos;
                """)
                identities = []
                for video in videos:
                    ActionChains(driver).move_to_element(video).perform()
                    if mode == "dash-unknown-mse":
                        time.sleep(.7)
                        assert not state_now()["labels"], "Unmatched source must stay absent"
                        continue
                    state = wait(lambda: (s if (s := state_now())["labels"] == ["下载视频"] else None),
                                 f"{mode}: hovered player did not get exactly one actionable control")
                    identities.extend(state["resourceIds"])
                    if mode == "direct":
                        button = driver.execute_script("return document.querySelector('hls-downloader-media-panel').shadowRoot.querySelector('.video-download')")
                        ActionChains(driver).move_to_element(button).perform()
                        time.sleep(.7)
                        assert state_now()["labels"] == ["下载视频"], "Control vanished while crossing the player gap"
                        driver.execute_script("arguments[0].focus()", button)
                        ActionChains(driver).move_to_element(driver.find_element("tag name", "h1")).perform()
                        time.sleep(.7)
                        assert state_now()["labels"] == ["下载视频"], "Keyboard-focused control vanished"
                        driver.execute_script("document.querySelector('hls-downloader-media-panel').shadowRoot.activeElement?.blur()")
                    ActionChains(driver).move_to_element(driver.find_element("tag name", "h1")).perform()
                    wait(lambda: not state_now()["labels"], f"{mode}: overlay stayed after leaving player")
                if expected == 2:
                    assert len(set(identities)) == 2, "Players shared the wrong download resource"
                report["cases"].append({"mode": mode, "players": expected, "passed": True})
            report["passed"] = True
        except Exception as error:
            report["error"] = str(error)
            if driver:
                report["state"] = _overlay_state(driver)
        finally:
            if driver:
                driver.quit()
            server.shutdown()
            server.server_close()
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    for name in ("extension", "driver", "ffmpeg", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--browser", choices=("edge", "firefox"), default="edge")
    parser.add_argument("--browser-binary", type=Path)
    raise SystemExit(run(parser.parse_args()))
