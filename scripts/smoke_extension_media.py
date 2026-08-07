"""Real Edge smoke for content-script media ownership.

The test uses an isolated temporary browser profile and local HTTP server. It
does not require the desktop app and never modifies the user's browser profile.
Build the Chromium extension before running it.
"""

from __future__ import annotations

import argparse
import contextlib
import functools
import http.server
import json
import shutil
import subprocess
import tempfile
import threading
import time
from pathlib import Path
import zipfile

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService


EDGE_CANDIDATES = (
    Path(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
    Path(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"),
)
FIREFOX_CANDIDATES = (
    Path(r"E:\Firefox\firefox.exe"),
    Path(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
    Path(r"C:\Program Files (x86)\Mozilla Firefox\firefox.exe"),
)


class QuietStaticHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, _format: str, *_args) -> None:
        pass


PAGE = """<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><link rel="icon" href="data:,"><title>Extension media smoke</title>
<style>body{margin:20px;background:#111;color:#eee;font:16px sans-serif}video{display:block;width:640px;height:360px;background:#000}iframe{display:block;width:720px;height:440px;border:1px solid #555}</style>
</head><body><h1 id="mode"></h1><div id="mount"></div><script>
const mode = new URL(location.href).searchParams.get('mode') || 'direct';
document.querySelector('#mode').textContent = mode;
const mount = document.querySelector('#mount');
function video() { const value=document.createElement('video'); value.controls=true; value.autoplay=true; value.muted=true; return value; }
function append(buffer, bytes) {
  return new Promise((resolve,reject) => { buffer.addEventListener('updateend',resolve,{once:true}); buffer.addEventListener('error',reject,{once:true}); buffer.appendBuffer(bytes); });
}
async function appendUrl(buffer, url) { await append(buffer, new Uint8Array(await (await fetch(url)).arrayBuffer())); }
function mseVideo(channel) {
  const value=video(); mount.append(value); const source=new MediaSource(); value.src=URL.createObjectURL(source);
  source.addEventListener('sourceopen', async () => {
    const buffer=source.addSourceBuffer('video/mp4; codecs="avc1.64001e, mp4a.40.2"');
    const reader=(await fetch('/stream.mp4?channel='+encodeURIComponent(channel))).body.getReader();
    while (true) {
      const result=await reader.read(); if (result.done) break;
      await append(buffer, result.value);
    }
    if (source.readyState === 'open') source.endOfStream();
    await value.play();
  }, {once:true});
}
function mseSlicedVideo() {
  const value=video(); mount.append(value); const source=new MediaSource(); value.src=URL.createObjectURL(source);
  source.addEventListener('sourceopen', async () => {
    const buffer=source.addSourceBuffer('video/mp4; codecs="avc1.64001e, mp4a.40.2"');
    const response=await fetch('/stream.mp4?channel=sliced');
    const copied=new Uint8Array(await response.arrayBuffer()).slice();
    await append(buffer,copied);
    if (source.readyState === 'open') source.endOfStream(); await value.play();
  }, {once:true});
}
function hlsVideo() {
  const value=video(); mount.append(value); const source=new MediaSource(); value.src=URL.createObjectURL(source);
  source.addEventListener('sourceopen', async () => {
    const buffer=source.addSourceBuffer('video/mp4; codecs="avc1.64001e"');
    const response=await fetch('/hls/index.m3u8?token=browser-smoke'); const text=await response.text();
    const map=text.match(/#EXT-X-MAP:URI="([^"]+)"/i)?.[1];
    const segments=text.split(/\\r?\\n/).map(line=>line.trim()).filter(line=>line && !line.startsWith('#'));
    if (map) await appendUrl(buffer, new URL(map,response.url));
    for (const segment of segments) await appendUrl(buffer,new URL(segment,response.url));
    if (source.readyState === 'open') source.endOfStream(); await value.play();
  }, {once:true});
}
function llHlsVideo() {
  const value=video(); mount.append(value); const source=new MediaSource(); value.src=URL.createObjectURL(source);
  source.addEventListener('sourceopen', async () => {
    const buffer=source.addSourceBuffer('video/mp4; codecs="avc1.64001e"');
    const response=await fetch('/llhls/index.m3u8?token=browser-smoke'); const text=await response.text();
    const map=text.match(/#EXT-X-MAP:URI="([^"]+)"/i)?.[1];
    const parts=[...text.matchAll(/#EXT-X-PART:[^\\r\\n]*?URI="([^"]+)"/gi)].map(match=>match[1]);
    if (map) await appendUrl(buffer,new URL(map,response.url));
    for (const part of parts) await appendUrl(buffer,new URL(part,response.url));
    if (source.readyState === 'open') source.endOfStream(); await value.play();
  }, {once:true});
}
function dashVideo() {
  const value=video(); mount.append(value); const source=new MediaSource(); value.src=URL.createObjectURL(source);
  source.addEventListener('sourceopen', async () => {
    const buffer=source.addSourceBuffer('video/mp4; codecs="avc1.64001e"');
    const response=await fetch('/dash/manifest.mpd?token=browser-smoke'); const text=await response.text();
    const xml=new DOMParser().parseFromString(text,'application/xml');
    const representation=[...xml.querySelectorAll('Representation')].find(item => (item.getAttribute('mimeType')||item.parentElement?.getAttribute('mimeType')||'').startsWith('video/'));
    const template=representation?.querySelector('SegmentTemplate') || representation?.parentElement?.querySelector('SegmentTemplate');
    const id=representation?.getAttribute('id') || '0'; const start=Number(template?.getAttribute('startNumber')||1);
    const resolve=(pattern,number) => pattern.replace(/\\$RepresentationID\\$/g,id).replace(/\\$Number(?:%0(\\d+)d)?\\$/g,(_all,width)=>String(number).padStart(Number(width||0),'0'));
    await appendUrl(buffer,new URL(resolve(template.getAttribute('initialization'),start),response.url));
    for(let number=start;number<start+4;number++) await appendUrl(buffer,new URL(resolve(template.getAttribute('media'),number),response.url));
    if (source.readyState === 'open') source.endOfStream(); await value.play();
  }, {once:true});
}
if (mode === 'shadow') {
  const host=document.createElement('section'); mount.append(host);
  const root=host.attachShadow({mode:'open'}); const value=video(); value.src='/stream.mp4?player=shadow'; root.append(value);
} else if (mode === 'dynamic-shadow') {
  const host=document.createElement('section'); mount.append(host);
  setTimeout(() => { const root=host.attachShadow({mode:'open'}); const value=video(); value.src='/stream.mp4?player=dynamic-shadow'; root.append(value); }, 1200);
} else if (mode === 'mse') {
  mseVideo('single');
} else if (mode === 'mse-sliced') {
  mseSlicedVideo();
} else if (mode === 'multi-mse') {
  mseVideo('one'); mseVideo('two');
} else if (mode === 'hls-mse') {
  hlsVideo();
} else if (mode === 'll-hls-mse') {
  llHlsVideo();
} else if (mode === 'dash-mse') {
  dashVideo();
} else if (mode === 'iframe' || mode === 'cross-iframe') {
  const frame=document.createElement('iframe');
  const host=mode === 'cross-iframe' ? 'localhost' : location.hostname;
  frame.src=location.protocol+'//'+host+':'+location.port+'/index.html?mode=direct&nested=1';
  mount.append(frame);
} else if (mode === 'ad-direct') {
  const value=video(); value.src='/stream.mp4?ad=preroll'; mount.append(value);
  setTimeout(() => { value.src='/stream.mp4?player=main'; void value.play().catch(()=>{}); }, 1200);
} else if (mode === 'spa') {
  const value=video(); value.src='/stream.mp4?player=spa-one'; mount.append(value);
  setTimeout(() => {
    history.pushState({},'',location.pathname+'?mode=spa&route=two');
    value.src='/stream.mp4?player=spa-two';
    void value.play().catch(()=>{});
  }, 1200);
} else {
  const value=video(); value.src='/stream.mp4?player=direct'; mount.append(value);
}
</script></body></html>"""


def _find_edge() -> Path:
    for path in EDGE_CANDIDATES:
        if path.is_file():
            return path
    raise RuntimeError("Microsoft Edge 未安装在标准路径")


def _find_firefox() -> Path:
    for path in FIREFOX_CANDIDATES:
        if path.is_file():
            return path
    raise RuntimeError("Firefox 未安装在已知路径")


def _make_media(root: Path, ffmpeg: str) -> None:
    command = [
        ffmpeg,
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=640x360:rate=30",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:sample_rate=48000",
        "-t",
        "4",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-g",
        "30",
        "-c:a",
        "aac",
        "-movflags",
        "+frag_keyframe+empty_moov+default_base_moof",
        str(root / "stream.mp4"),
    ]
    subprocess.run(command, check=True, timeout=60)
    hls = root / "hls"
    hls.mkdir()
    subprocess.run([
        ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", str(root / "stream.mp4"),
        "-map", "0:v:0", "-c", "copy", "-hls_time", "1", "-hls_segment_type", "fmp4",
        "-hls_playlist_type", "vod", "-hls_fmp4_init_filename", "init.mp4",
        "-hls_segment_filename", str(hls / "segment-%03d.m4s"), str(hls / "index.m3u8"),
    ], check=True, timeout=60, cwd=hls)
    dash = root / "dash"
    dash.mkdir()
    subprocess.run([
        ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", str(root / "stream.mp4"),
        "-map", "0:v:0", "-c", "copy", "-f", "dash", "-seg_duration", "1",
        "-use_template", "1", "-use_timeline", "0",
        "-init_seg_name", "init-$RepresentationID$.m4s",
        "-media_seg_name", "chunk-$RepresentationID$-$Number%05d$.m4s",
        str(dash / "manifest.mpd"),
    ], check=True, timeout=60, cwd=dash)
    (root / "index.html").write_text(PAGE, encoding="utf-8")
    llhls = root / "llhls"
    llhls.mkdir()
    parts = sorted(hls.glob("segment-*.m4s"))
    if not parts:
        raise RuntimeError("FFmpeg 没有生成 HLS 媒体分片")
    llhls_lines = [
        "#EXTM3U",
        "#EXT-X-VERSION:9",
        "#EXT-X-TARGETDURATION:1",
        "#EXT-X-PART-INF:PART-TARGET=1.0",
        "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=2.0",
        "#EXT-X-MEDIA-SEQUENCE:1",
        '#EXT-X-MAP:URI="../hls/init.mp4"',
        *[f'#EXT-X-PART:DURATION=1.0,URI="../hls/{part.name}"' for part in parts],
        f'#EXT-X-PRELOAD-HINT:TYPE=PART,URI="../hls/{parts[-1].name}"',
    ]
    (llhls / "index.m3u8").write_text("\n".join(llhls_lines) + "\n", encoding="utf-8")


def _current_frame_overlay_state(driver) -> dict:
    return driver.execute_script(
        """
        const labels=[]; const roots=[]; const seen=new Set();
        const visit=(root) => {
          if (!root || seen.has(root)) return; seen.add(root); roots.push(root);
          root.querySelectorAll('*').forEach(element => { if (element.shadowRoot) visit(element.shadowRoot); });
        };
        visit(document);
        const resourceIds=[]; const moreButtons=[]; const panelActions=[];
        roots.forEach(root => root.querySelectorAll('button.video-download').forEach(button => { labels.push(button.innerText.trim()); resourceIds.push(button.dataset.resourceId || ''); }));
        roots.forEach(root => root.querySelectorAll('button.video-more').forEach(button => moreButtons.push(button.getAttribute('aria-label') || button.title || '')));
        roots.forEach(root => root.querySelectorAll('.item-actions button').forEach(button => panelActions.push(button.innerText.trim())));
        const videos=[];
        roots.forEach(root => root.querySelectorAll('video').forEach(video => videos.push({paused:video.paused,currentTime:video.currentTime,src:video.currentSrc})));
        return {marker:document.documentElement.getAttribute('data-hls-downloader-extension'),labels,resourceIds,moreButtons,panelActions,videos};
        """
    )


def _overlay_state(driver) -> dict:
    """Collect extension UI from every frame, including cross-origin frames."""
    frames: list[dict] = []
    driver.switch_to.default_content()

    def visit(path: list[int]) -> None:
        state = _current_frame_overlay_state(driver)
        state["framePath"] = list(path)
        frames.append(state)
        children = driver.find_elements(By.TAG_NAME, "iframe")
        for index, child in enumerate(children):
            try:
                driver.switch_to.frame(child)
                visit([*path, index])
            finally:
                driver.switch_to.parent_frame()

    visit([])
    driver.switch_to.default_content()
    return {
        "marker": frames[0].get("marker") if frames else None,
        "frameMarkers": [item.get("marker") for item in frames],
        "labels": [label for item in frames for label in item.get("labels", [])],
        "resourceIds": [value for item in frames for value in item.get("resourceIds", [])],
        "moreButtons": [value for item in frames for value in item.get("moreButtons", [])],
        "panelActions": [value for item in frames for value in item.get("panelActions", [])],
        "videos": [video for item in frames for video in item.get("videos", [])],
        "frameStates": frames,
    }


def _resource_id(url: str) -> str:
    """Mirror the extension's two 64-bit FNV passes for an ASCII smoke URL."""
    def fnv64(value: str, offset: int) -> int:
        result = offset
        for character in value:
            result ^= ord(character)
            result = (result * 1_099_511_628_211) & ((1 << 64) - 1)
        return result

    forward = fnv64(url, 14_695_981_039_346_656_037)
    reverse = fnv64(url[::-1], 7_809_847_782_465_536_322)
    return f"{forward:016x}{reverse:016x}"


def _open_first_media_actions(driver) -> bool:
    return bool(driver.execute_script(
        """
        const roots=[]; const seen=new Set();
        const visit=(root) => {
          if (!root || seen.has(root)) return; seen.add(root); roots.push(root);
          root.querySelectorAll('*').forEach(element => { if (element.shadowRoot) visit(element.shadowRoot); });
        };
        visit(document);
        const button=roots.map(root => root.querySelector('button.video-more')).find(Boolean);
        if (!button) return false;
        button.click();
        return true;
        """
    ))


def _browser_errors(driver) -> list[str]:
    try:
        return [
            str(item.get("message") or "")
            for item in driver.get_log("browser")
            if str(item.get("level") or "").upper() in {"SEVERE", "WARNING"}
        ]
    except Exception:
        return []


def run(
    extension: Path,
    *,
    browser_name: str,
    addon: Path | None,
    headed: bool,
    ffmpeg: str,
    driver_path: Path | None = None,
    binary_path: Path | None = None,
) -> list[dict]:
    if browser_name == "edge" and not (extension / "manifest.json").is_file():
        raise RuntimeError(f"Chromium 扩展未构建: {extension}")
    if browser_name == "firefox" and addon is not None and not addon.is_file():
        raise RuntimeError(f"Firefox 临时扩展包不存在: {addon}")
    if browser_name == "firefox" and addon is None and not (extension / "manifest.json").is_file():
        raise RuntimeError(f"Firefox 扩展未构建: {extension}")
    with tempfile.TemporaryDirectory(prefix="hls-extension-smoke-") as temporary:
        root = Path(temporary)
        media_root = root / "site"
        profile = root / "profile"
        media_root.mkdir()
        _make_media(media_root, ffmpeg)
        resolved_addon = addon
        if browser_name == "firefox" and resolved_addon is None:
            resolved_addon = root / "hls-downloader-smoke.xpi"
            with zipfile.ZipFile(resolved_addon, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for path in sorted(extension.rglob("*")):
                    if path.is_file():
                        archive.write(path, path.relative_to(extension).as_posix())
        handler = functools.partial(QuietStaticHandler, directory=str(media_root))
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        results: list[dict] = []
        driver = None
        try:
            if browser_name == "edge":
                options = EdgeOptions()
                options.binary_location = str(binary_path or _find_edge())
                options.add_argument(f"--user-data-dir={profile}")
                options.add_argument(f"--disable-extensions-except={extension}")
                options.add_argument(f"--load-extension={extension}")
                options.add_argument("--autoplay-policy=no-user-gesture-required")
                options.add_argument("--no-first-run")
                options.add_argument("--disable-default-apps")
                options.add_argument("--window-size=1280,800")
                options.set_capability("goog:loggingPrefs", {"browser": "ALL"})
                if not headed:
                    options.add_argument("--headless=new")
                service = EdgeService(executable_path=str(driver_path)) if driver_path else EdgeService()
                driver = webdriver.Edge(service=service, options=options)
            else:
                options = FirefoxOptions()
                options.binary_location = str(binary_path or _find_firefox())
                options.set_preference("media.autoplay.default", 0)
                options.set_preference("media.autoplay.blocking_policy", 0)
                options.set_preference("browser.shell.checkDefaultBrowser", False)
                if not headed:
                    options.add_argument("-headless")
                service = FirefoxService(executable_path=str(driver_path)) if driver_path else FirefoxService()
                driver = webdriver.Firefox(service=service, options=options)
                driver.install_addon(str(resolved_addon), temporary=True)
            for mode in (
                "direct", "shadow", "dynamic-shadow", "iframe", "cross-iframe",
                "ad-direct", "spa", "mse", "mse-sliced", "multi-mse", "hls-mse", "ll-hls-mse", "dash-mse",
            ):
                driver.get(f"http://127.0.0.1:{server.server_port}/index.html?mode={mode}")
                deadline = time.monotonic() + 15
                state: dict = {}
                while time.monotonic() < deadline:
                    state = _overlay_state(driver)
                    expected = 2 if mode == "multi-mse" else 1
                    playing = sum(float(item.get("currentTime") or 0) > 0 for item in state.get("videos", [])) == expected
                    actionable = state.get("labels", []).count("下载视频") == expected
                    expected_player = "main" if mode == "ad-direct" else "spa-two" if mode == "spa" else ""
                    main_ready = not expected_player or any(
                        f"player={expected_player}" in str(item.get("src") or "")
                        for item in state.get("videos", [])
                    )
                    expected_frames = 2 if mode in {"iframe", "cross-iframe"} else 1
                    frames_ready = state.get("frameMarkers", []).count("1") == expected_frames
                    if state.get("marker") == "1" and frames_ready and playing and actionable and main_ready:
                        break
                    time.sleep(0.2)
                state["mode"] = mode
                state["browserErrors"] = _browser_errors(driver)
                results.append(state)
                if state.get("marker") != "1":
                    raise AssertionError(f"{mode}: content script 未就绪: {state}")
                expected_frames = 2 if mode in {"iframe", "cross-iframe"} else 1
                if state.get("frameMarkers", []).count("1") != expected_frames:
                    raise AssertionError(f"{mode}: 子 frame content script 未就绪: {state}")
                if state.get("browserErrors"):
                    raise AssertionError(f"{mode}: 页面或扩展产生浏览器错误: {state}")
                expected = 2 if mode == "multi-mse" else 1
                if sum(float(item.get("currentTime") or 0) > 0 for item in state.get("videos", [])) != expected:
                    raise AssertionError(f"{mode}: 视频没有开始播放: {state}")
                if "下载视频" not in state.get("labels", []):
                    raise AssertionError(f"{mode}: 当前播放器没有得到唯一一键资源: {state}")
                if mode == "direct":
                    if not state.get("moreButtons"):
                        raise AssertionError(f"{mode}: 单资源播放器没有显示投屏/推送更多操作入口: {state}")
                    if not _open_first_media_actions(driver):
                        raise AssertionError(f"{mode}: 无法打开单资源播放器的投屏/推送操作面板: {state}")
                    action_deadline = time.monotonic() + 5
                    while time.monotonic() < action_deadline:
                        state = _overlay_state(driver)
                        if {"投屏链接", "推送链接"}.issubset(set(state.get("panelActions", []))):
                            break
                        time.sleep(0.1)
                    if not {"投屏链接", "推送链接"}.issubset(set(state.get("panelActions", []))):
                        raise AssertionError(f"{mode}: 投屏/推送链接操作没有出现在资源面板: {state}")
                if mode == "ad-direct":
                    main_url = next((item["src"] for item in state["videos"] if "player=main" in item.get("src", "")), "")
                    if state.get("resourceIds") != [_resource_id(main_url)]:
                        raise AssertionError(f"{mode}: 广告切主片后按钮仍绑定错误资源: {state}")
                if mode == "spa":
                    main_url = next((item["src"] for item in state["videos"] if "player=spa-two" in item.get("src", "")), "")
                    if state.get("resourceIds") != [_resource_id(main_url)]:
                        raise AssertionError(f"{mode}: pushState 后按钮仍绑定旧资源: {state}")
                if mode == "multi-mse":
                    resource_ids = [value for value in state.get("resourceIds", []) if value]
                    if len(resource_ids) != 2 or len(set(resource_ids)) != 2:
                        raise AssertionError(f"{mode}: 两个播放器没有绑定两个不同资源: {state}")
        finally:
            if driver is not None:
                with contextlib.suppress(Exception):
                    driver.quit()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
        return results


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", choices=("edge", "firefox"), default="edge")
    parser.add_argument("--extension", type=Path, default=Path("extension/.output/chrome-mv3"))
    parser.add_argument("--addon", type=Path, help="Firefox 使用的 WXT zip/xpi")
    parser.add_argument("--headed", action="store_true")
    parser.add_argument("--ffmpeg", default=shutil.which("ffmpeg") or "ffmpeg")
    parser.add_argument("--driver", type=Path, help="可选的匹配版 WebDriver；省略时由 Selenium Manager 查找")
    parser.add_argument("--browser-binary", type=Path, help="可选的浏览器可执行文件路径")
    arguments = parser.parse_args()
    print(json.dumps(run(
        arguments.extension.resolve(),
        browser_name=arguments.browser,
        addon=arguments.addon.resolve() if arguments.addon else None,
        headed=arguments.headed,
        ffmpeg=arguments.ffmpeg,
        driver_path=arguments.driver.resolve() if arguments.driver else None,
        binary_path=arguments.browser_binary.resolve() if arguments.browser_binary else None,
    ), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
