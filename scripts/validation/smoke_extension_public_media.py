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

from smoke_extension_media import _find_edge, _find_firefox, _overlay_state, _resource_id, _fit_browser_window


HLS = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8"
DASH = "https://dash.akamaized.net/akamai/bbb_30fps/bbb_30fps.mpd"
CASES = [
    ("hls-js", "https://hlsjs.video-dev.org/demo/?" + urlencode({"src": HLS}), HLS),
    ("dash-js", "https://reference.dashif.org/dash.js/latest/samples/getting-started/manual-load-single-video.html", DASH),
    ("mdn-video", "https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html", "https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm"),
]


def _hook_replay_diagnostics(driver, expected: str) -> dict:
    """Replay MAIN-world resource events and summarize matching network evidence."""
    replay = driver.execute_async_script(
        r"""
        const [expected, done] = arguments;
        const events = [];
        const safeUrl = value => {
          try {
            const url = new URL(value);
            url.search = [...url.searchParams.keys()].map(key => `${encodeURIComponent(key)}=<redacted>`).join('&');
            url.hash = '';
            return url.href;
          } catch { return String(value || '').slice(0, 500); }
        };
        const onResource = event => {
          let detail = {};
          try { detail = typeof event.detail === 'string' ? JSON.parse(event.detail) : {}; } catch {}
          events.push({
            url: safeUrl(detail.url), mimeType: detail.mimeType || '',
            inspected: detail.inspection?.inspected === true,
            manifestType: detail.inspection?.manifestType || '',
            variantCount: Array.isArray(detail.inspection?.variants) ? detail.inspection.variants.length : 0,
          });
        };
        window.addEventListener('__hls_downloader_resource__', onResource);
        window.dispatchEvent(new Event('__hls_downloader_replay__'));
        setTimeout(() => {
          window.removeEventListener('__hls_downloader_resource__', onResource);
          const expectedUrl = new URL(expected);
          const network = performance.getEntriesByType('resource')
            .filter(entry => {
              try {
                const url = new URL(entry.name);
                return /\.m3u8?(?:$|[?#])/i.test(url.href)
                  || (url.origin === expectedUrl.origin && url.pathname === expectedUrl.pathname);
              } catch { return false; }
            })
            .map(entry => ({url: safeUrl(entry.name), initiatorType: entry.initiatorType, durationMs: Math.round(entry.duration)}));
          done({events, network, marker: document.documentElement.getAttribute('data-hls-downloader-extension')});
        }, 1200);
        """,
        expected,
    )
    driver.switch_to.default_content()
    replay["overlay_after_replay"] = _overlay_state(driver)
    return replay


def _browser_console_errors(driver) -> list[dict]:
    try:
        entries = driver.get_log("browser")
    except Exception as error:
        return [{"logging_unavailable": str(error)}]
    return [
        {key: entry.get(key) for key in ("level", "message", "source", "timestamp")}
        for entry in entries
        if entry.get("level") in {"SEVERE", "ERROR"}
    ][:100]


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
    temp_root = Path(__file__).resolve().parents[2] / ".tool-cache" / "test-tmp"
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
                for value in [f"--user-data-dir={root / 'profile'}", *([] if args.headed else ['--headless=new']),
                              "--disable-features=DisableLoadExtensionCommandLineSwitch",
                              f"--disable-extensions-except={extension}", f"--load-extension={extension}",
                              "--autoplay-policy=no-user-gesture-required", "--no-first-run", "--window-size=1280,900"]:
                    options.add_argument(value)
                options.set_capability("goog:loggingPrefs", {"browser": "ALL"})
                service = EdgeService(executable_path=str(args.driver))
                driver = webdriver.Edge(service=service, options=options)
                driver.execute_cdp_cmd("Page.addScriptToEvaluateOnNewDocument", {"source": """
                    (() => {
                      const errors = window.__hlsQaRuntimeErrors = [];
                      const add = value => { errors.push(value); if (errors.length > 100) errors.shift(); };
                      window.addEventListener('error', event => add({type:'error',message:event.message,filename:event.filename,line:event.lineno,column:event.colno}), true);
                      window.addEventListener('unhandledrejection', event => add({type:'unhandledrejection',reason:String(event.reason?.stack||event.reason||'').slice(0,1000)}));
                    })();
                """})
            else:
                addon = root / "extension.xpi"
                with zipfile.ZipFile(addon, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                    for p in sorted(extension.rglob("*")):
                        if p.is_file(): archive.write(p, p.relative_to(extension).as_posix())
                options = FirefoxOptions()
                options.set_preference('browser.shell.checkDefaultBrowser', False)
                options.set_preference('browser.aboutwelcome.enabled', False)
                options.set_preference('browser.newtabpage.activity-stream.asrouter.providers.onboarding', '{"id":"onboarding","type":"local","localProvider":"OnboardingMessageProvider","enabled":false}')
                options.set_preference('browser.newtabpage.activity-stream.asrouter.providers.messaging-experiments', '{"id":"messaging-experiments","type":"remote-experiments","enabled":false}')
                options.binary_location = str(_find_firefox())
                if not args.headed: options.add_argument("-headless")
                options.set_preference("media.autoplay.default", 0)
                options.set_preference("media.autoplay.blocking_policy", 0)
                service = FirefoxService(executable_path=str(args.driver),
                                         service_args=["--profile-root", str(root)])
                driver = webdriver.Firefox(service=service, options=options)
                driver.install_addon(str(addon), temporary=True)
            _fit_browser_window(driver)
            driver.set_page_load_timeout(30)
            for name, url, expected in CASES:
                if args.case and name not in args.case: continue
                record = {"name": name, "page": url, "expected_media": expected, "passed": False}
                try:
                    try: driver.get(url)
                    except TimeoutException: record["page_load_timed_out"] = True
                    clicked_play = False
                    deadline = time.monotonic() + 45
                    while time.monotonic() < deadline:
                        driver.switch_to.default_content()
                        # DASH 手动加载样例由页面控制器启动；video.play() 不会替它加载清单。
                        if name == 'dash-js' and not clicked_play:
                            for button in driver.find_elements(By.CSS_SELECTOR, 'button[title="Play/Pause"]'):
                                if button.is_displayed() and button.is_enabled():
                                    button.click()
                                    clicked_play = True
                                    break
                        play_frames(driver)
                        # 小屏幕沙盒中 HLS demo 的播放器位于首屏下方；入口只随可见播放器显示。
                        driver.execute_script("document.querySelector('video')?.scrollIntoView({block:'center',inline:'nearest'})")
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
                        if args.browser == "edge":
                            record["hook_diagnostics"] = _hook_replay_diagnostics(driver, expected)
                            record["browser_console_errors"] = _browser_console_errors(driver)
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
    parser.add_argument("--headed", action="store_true")
    parser.add_argument("--case", choices=tuple(c[0] for c in CASES), action="append")
    raise SystemExit(0 if run(parser.parse_args())["passed"] else 1)
