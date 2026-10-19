"""Exercise QwenCloud subtitles on an actual public HTML media player."""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
import time
from pathlib import Path
from typing import Any
from urllib.parse import urlparse
from urllib.request import urlopen

import websocket

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from smoke_extension_browsers import _chromium_extension_id, _zip_firefox_extension
from smoke_extension_media import _find_edge, _find_firefox
from smoke_extension_takeover import _extension_call
from smoke_v7_portable_app import Pipe

DEFAULT_URL = "https://commons.wikimedia.org/wiki/File:En-us-Wikipedia.ogg"
EXPECTED_SOURCE = "wikipedia"
EXPECTED_TRANSLATION = "维基百科"


def _wait(predicate, description: str, timeout: float = 75.0):
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except Exception as error:
            last_error = error
        time.sleep(0.25)
    if last_error:
        raise TimeoutError(f"{description}: {last_error}") from last_error
    raise TimeoutError(description)


def _message(driver, message: dict[str, Any]) -> Any:
    return _extension_call(driver, "chrome.tabs.query({currentWindow:true}).then(t=>{const tab=t.find(x=>x.active&&/^https?:\\/\\//i.test(x.url||''))||[...t].reverse().find(x=>/^https?:\\/\\//i.test(x.url||''));if(!tab)throw Error('web tab missing');return chrome.runtime.sendMessage({...arguments[0],tabId:tab.id})}).then(done,e=>done({ok:false,error:String(e)}))", message)


def _require_ok(response: Any, operation: str) -> Any:
    if not isinstance(response, dict) or response.get("ok") is not True:
        reason = response.get("error", "invalid extension response") if isinstance(response, dict) else "invalid extension response"
        raise RuntimeError(f"{operation} failed: {reason}")
    return response


def _set_settings(driver, settings: dict[str, Any]) -> None:
    pipe = Pipe()
    try:
        response = pipe.request({"type": "subtitle", "request_id": 1, "request": {
            "action": "configure", "provider": "qwencloud", "preferences": {"enabled": True, "source": "auto",
                "target": "zh", "offset": 0.0, "showSource": True, "prefetch": False, **settings},
        }})
        result = response.get("result", {}) if isinstance(response, dict) else {}
        if not isinstance(result, dict) or result.get("preferences", {}).get("enabled") is not True:
            raise RuntimeError(f"Core subtitle preferences failed: {response}")
    finally:
        pipe.close()
    _wait(lambda: all(_extension_state(driver).get("settings", {}).get(key) == value
                     for key, value in settings.items()), "desktop preferences did not reach extension")


def _extension_state(driver) -> dict[str, Any]:
    return _require_ok(_message(driver, {"type": "subtitle-state-get"}), "subtitle state")


def _core_poll_popup(driver, popup_handle: str, page_handle: str, session_id: str, after_sequence: int = 0) -> dict[str, Any]:
    current = driver.current_window_handle
    try:
        driver.switch_to.window(popup_handle)
        response = driver.execute_async_script("""
          const done=arguments[arguments.length-1], sessionId=arguments[0], afterSequence=arguments[1];
          chrome.storage.local.get('desktopClientId').then(values=>{
            const clientId=String(values.desktopClientId||'').trim();
            if(!clientId) throw Error('extension browser client id is unavailable');
            chrome.runtime.sendNativeMessage('com.ciaooo55.hls_downloader',
              {op:'subtitle',client_id:clientId,request:{action:'poll',session_id:sessionId,after_sequence:afterSequence}},
              result=>{
                const error=chrome.runtime.lastError;
                done(error?{ok:false,error:error.message}:(result?.result||result));
              });
          }).catch(error=>done({ok:false,error:String(error)}));
        """, session_id, after_sequence)
        return _require_ok(response, "Core subtitle poll")
    finally:
        if current in driver.window_handles:
            driver.switch_to.window(current)


def _open_driver(args, extension: Path, profile: Path):
    from selenium import webdriver

    browser = args.browser
    if browser == "edge":
        from selenium.webdriver.edge.options import Options
        from selenium.webdriver.edge.service import Service

        runtime = Path(args.runtime) if args.runtime else None
        binary = runtime if runtime and runtime.is_file() else (runtime / "msedge.exe" if runtime else None)
        if not binary or not binary.is_file():
            binary = _find_edge()
        options = Options()
        if binary and binary.is_file():
            options.binary_location = str(binary)
        options.page_load_strategy = "eager"
        options.add_argument(f"--user-data-dir={profile}")
        options.add_argument("--no-first-run")
        options.add_argument("--no-default-browser-check")
        options.add_argument("--autoplay-policy=no-user-gesture-required")
        options.add_argument("--disable-features=DisableLoadExtensionCommandLineSwitch")
        if not args.headed:
            options.add_argument("--headless=new")
        options.add_argument(f"--disable-extensions-except={extension}")
        options.add_argument(f"--load-extension={extension}")
        options.add_argument("--enable-unsafe-extension-debugging")
        driver = webdriver.Edge(service=Service(executable_path=args.driver) if args.driver else Service(), options=options)
        extension_id = _chromium_extension_id(extension)
        return driver, f"chrome-extension://{extension_id}/popup.html", extension_id

    from selenium.webdriver.firefox.options import Options
    from selenium.webdriver.firefox.service import Service

    options = Options()
    runtime = Path(args.runtime) if args.runtime else None
    binary = runtime if runtime and runtime.is_file() else (runtime / "firefox.exe" if runtime else None)
    if binary and binary.is_file():
        options.binary_location = str(binary)
    elif runtime:
        options.binary_location = str(_find_firefox())
    options.page_load_strategy = "eager"
    options.set_preference("media.autoplay.default", 0)
    options.set_preference("media.autoplay.blocking_policy", 0)
    options.set_preference("browser.shell.checkDefaultBrowser", False)
    options.set_preference("browser.aboutwelcome.enabled", False)
    options.set_preference("browser.newtabpage.activity-stream.asrouter.providers.onboarding",
                           '{"id":"onboarding","type":"local","localProvider":"OnboardingMessageProvider","enabled":false}')
    options.set_preference("browser.newtabpage.activity-stream.asrouter.providers.messaging-experiments",
                           '{"id":"messaging-experiments","type":"remote-experiments","enabled":false}')
    if not args.headed:
        options.add_argument("-headless")
    service = Service(executable_path=args.driver, service_args=["--allow-system-access"]) if args.driver else Service(service_args=["--allow-system-access"])
    driver = webdriver.Firefox(service=service, options=options)
    addon = profile / "hls-downloader.xpi"
    profile.mkdir(parents=True, exist_ok=True)
    _zip_firefox_extension(extension, addon)
    driver.set_script_timeout(10)
    addon_id = driver.install_addon(str(addon), temporary=True)
    driver.set_context("chrome")
    uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
    driver.set_context("content")
    extension_uuid = uuids.get(addon_id)
    if not extension_uuid:
        driver.quit()
        raise RuntimeError("Firefox did not assign a temporary extension UUID")
    return driver, f"moz-extension://{extension_uuid}/popup.html?site=1", addon_id


def _trigger_chromium_action(driver, extension_id: str, page_url: str) -> None:
    capabilities = driver.capabilities
    address = (capabilities.get("goog:chromeOptions", {}).get("debuggerAddress")
               or capabilities.get("ms:edgeOptions", {}).get("debuggerAddress"))
    if not address:
        raise RuntimeError("Chromium WebDriver did not expose its DevTools address")
    with urlopen(f"http://{address}/json/version", timeout=3) as response:
        browser_socket = json.load(response)["webSocketDebuggerUrl"]
    connection = websocket.create_connection(browser_socket, timeout=5, suppress_origin=True)
    try:
        connection.send(json.dumps({"id": 1, "method": "Target.getTargets",
                                    "params": {"filter": [{"type": "tab", "exclude": False}]}}))
        while True:
            response = json.loads(connection.recv())
            if response.get("id") == 1:
                target = next((item for item in response["result"]["targetInfos"]
                               if item.get("url") == page_url), None)
                if target is None:
                    raise RuntimeError("active Original file tab was missing from DevTools targets")
                target_id = target["targetId"]
                break
        connection.send(json.dumps({"id": 2, "method": "Extensions.triggerAction",
                                    "params": {"id": extension_id, "targetId": target_id}}))
        while True:
            response = json.loads(connection.recv())
            if response.get("id") == 2:
                if "error" in response:
                    raise RuntimeError(f"extension toolbar invocation failed: {response['error']}")
                return
    finally:
        connection.close()


def _media_inventory(driver) -> list[dict[str, Any]]:
    return driver.execute_script("""
      return [...document.querySelectorAll('audio,video')].map((m,i)=>({index:i,tag:m.tagName,
        src:m.currentSrc||m.src||m.querySelector('source[src]')?.src||'',preload:m.preload,
        readyState:m.readyState,networkState:m.networkState,duration:Number.isFinite(m.duration)?m.duration:null,
        paused:m.paused,currentTime:m.currentTime,ended:m.ended,disabled:m.disabled===true,
        hasAudio:m.audioTracks?m.audioTracks.length>0:null}));
    """)


def _caption_state(driver) -> dict[str, Any]:
    return driver.execute_script("""
      const hosts=[...document.querySelectorAll('[data-hls-subtitle]')];
      const visible=element=>{
        if(!element) return false;
        const rect=element.getBoundingClientRect();
        if(rect.width<=0||rect.height<=0||rect.right<=0||rect.bottom<=0||rect.left>=innerWidth||rect.top>=innerHeight) return false;
        for(let node=element;node&&node instanceof Element;node=node.parentElement){
          const style=getComputedStyle(node);
          if(style.display==='none'||style.visibility==='hidden'||style.visibility==='collapse'||Number(style.opacity)<=0) return false;
        }
        return true;
      };
      return hosts.map(host=>{
        const root=host.shadowRoot,source=root?.querySelector('.source'),translation=root?.querySelector('.translation'),status=root?.querySelector('.status');
        const hostVisible=visible(host);
        return {source:hostVisible&&visible(source)?source.textContent||'':'',
          translation:hostVisible&&visible(translation)?translation.textContent||'':'',
          status:hostVisible&&visible(status)?status.textContent||'':'',visible:hostVisible};
      });
    """)


def _page_diagnostics(driver, args) -> dict[str, Any]:
    details = driver.execute_script("""
      return {title:document.title,url:location.href,
        media:[...document.querySelectorAll('audio,video')].map((m,i)=>({index:i,tag:m.tagName,
          src:m.currentSrc||m.src||m.querySelector('source[src]')?.src||'',preload:m.preload,
          rect:(r=>({x:r.x,y:r.y,width:r.width,height:r.height}))(m.getBoundingClientRect()),
          readyState:m.readyState,networkState:m.networkState,duration:Number.isFinite(m.duration)?m.duration:null,
          paused:m.paused,currentTime:m.currentTime,error:m.error?{code:m.error.code,message:m.error.message}:null,
          disabled:m.disabled===true,outerHTML:m.outerHTML.slice(0,1500),
          parentHTML:m.parentElement?.outerHTML.slice(0,3000)||''})),
        originalFileLinks:[...document.querySelectorAll("a[href^='https://upload.wikimedia.org/']")].map(a=>({
          text:a.textContent.trim(),title:a.title,href:a.href,
          visible:(r=>r.width>0&&r.height>0)(a.getBoundingClientRect())})),
        playerControls:[...document.querySelectorAll('.mw-tmh-play')].map(button=>{
          const rect=button.getBoundingClientRect(), icon=button.querySelector('.mw-tmh-play-icon'), iconRect=icon?.getBoundingClientRect();
          return {text:button.textContent,title:button.title,href:button.href,
            rect:{x:rect.x,y:rect.y,width:rect.width,height:rect.height},
            iconRect:iconRect?{x:iconRect.x,y:iconRect.y,width:iconRect.width,height:iconRect.height}:null,
            visible:rect.width>0&&rect.height>0,outerHTML:button.outerHTML};
        }),
        overlay:[...document.querySelectorAll('[data-hls-subtitle]')].map(host=>{
          const measure=element=>{if(!element)return null;const r=element.getBoundingClientRect(),s=getComputedStyle(element);
            return {rect:{x:r.x,y:r.y,width:r.width,height:r.height},display:s.display,visibility:s.visibility,opacity:s.opacity};};
          const root=host.shadowRoot,hostStyle=getComputedStyle(host);
          return {text:root?.textContent?.slice(0,1000)||'',
            host:{...measure(host),display:hostStyle.display,visibility:hostStyle.visibility,opacity:hostStyle.opacity},
            source:measure(root?.querySelector('.source')),translation:measure(root?.querySelector('.translation'))};
        })};
    """)
    screenshot = Path(args.report).with_suffix(".failure.png") if args.report else Path.cwd() / "qwencloud-public-subtitles.failure.png"
    screenshot.parent.mkdir(parents=True, exist_ok=True)
    if driver.save_screenshot(str(screenshot)):
        details["screenshot"] = str(screenshot.resolve())
    return details


def _open_original_file_link(driver) -> tuple[str, str]:
    """Follow Commons' real Original file link into Wikimedia's media document."""
    from selenium.webdriver.common.by import By
    from selenium.webdriver.support.ui import WebDriverWait

    source_url = driver.current_url
    source_handle = driver.current_window_handle
    links = WebDriverWait(driver, 20).until(lambda current: current.find_elements(
        By.CSS_SELECTOR, "a[href^='https://upload.wikimedia.org/']"))
    link = next((item for item in links if item.is_displayed() and item.text.strip() == "Original file"), None)
    if link is None:
        anchors = driver.execute_script("""
          return [...document.querySelectorAll("a[href^='https://upload.wikimedia.org/']")]
            .map(a=>({text:a.textContent.trim(),title:a.title,href:a.href}));
        """)
        raise RuntimeError(f"Commons Original file link to upload.wikimedia.org was not available: {anchors}")
    original_url = link.get_attribute("href")
    link.click()
    WebDriverWait(driver, 20).until(lambda current: len(current.window_handles) > 1 or current.current_url != source_url)
    destination = next((handle for handle in driver.window_handles if handle != source_handle), None)
    if destination:
        driver.switch_to.window(destination)
    if urlparse(driver.current_url).hostname != "upload.wikimedia.org":
        raise RuntimeError(f"Original file link did not open Wikimedia media: {driver.current_url}")
    return source_url, driver.current_url


def _activate_original_player(driver, native_media_document: bool = False) -> list[dict[str, Any]]:
    from selenium.webdriver.common.by import By

    buttons = driver.find_elements(By.CSS_SELECTOR, ".mw-tmh-play")
    playable_button = next((button for button in buttons if button.is_displayed() and button.is_enabled()), None)
    if playable_button is not None:
        # Click the visible play icon; the full-width anchor's center can fall on
        # the timeline rather than the actual control on Commons.
        target = playable_button.find_elements(By.CSS_SELECTOR, ".mw-tmh-play-icon")
        (target[0] if target and target[0].is_displayed() else playable_button).click()
    elif native_media_document:
        # Firefox's native media document owns the source channel internally;
        # calling load() on its source-less video element resets that channel.
        pass
    else:
        count = driver.execute_script("""
          const media=[...document.querySelectorAll('audio,video')].filter(m=>!m.disabled);
          for(const m of media) if(m.currentSrc||m.src||m.querySelector('source[src]')) m.load();
          return media.length;
        """)
        if not count:
            raise RuntimeError("Commons original player control .mw-tmh-play was not available")

    media = _wait(lambda: [m for m in _media_inventory(driver)
                           if m["src"] and not m["disabled"] and m["readyState"] >= 1],
                  "original Commons player did not activate media metadata", timeout=30)
    driver.execute_script("""
      for(const m of document.querySelectorAll('audio,video')) {
        if(!m.disabled && (m.currentSrc||m.src)) { m.pause(); m.currentTime=0; }
      }
    """)
    return media


def _configure_core(api_key: str = "") -> dict[str, Any]:
    pipe = Pipe()
    try:
        request = {"action": "configure", "provider": "qwencloud", "api_key": api_key} if api_key else {"action": "configuration"}
        response = pipe.request({"type": "subtitle", "request_id": 1, "request": request})
    finally:
        pipe.close()
    result = response.get("result", response) if isinstance(response, dict) else {}
    if not isinstance(result, dict) or result.get("provider") != "qwencloud" or result.get("has_key") is not True:
        raise RuntimeError("Core did not confirm QwenCloud credential configuration")
    return {"provider": result["provider"], "has_key": result["has_key"]}


def _redact(value: Any, api_key: str) -> Any:
    if isinstance(value, str):
        return value.replace(api_key, "[redacted]") if api_key else value
    if isinstance(value, list):
        return [_redact(item, api_key) for item in value]
    if isinstance(value, dict):
        return {str(key): _redact(item, api_key) for key, item in value.items() if str(key).lower() not in {"api_key", "key", "authorization"}}
    return value


def run(args, api_key: str) -> dict[str, Any]:
    """Run the public-site acceptance flow; the key is used only in memory."""
    use_configured_key = bool(getattr(args, "use_configured_key", False))
    if not api_key and not use_configured_key:
        raise ValueError("API key is required")
    extension = Path(args.extension).resolve()
    if not (extension / "manifest.json").is_file():
        raise ValueError("--extension must point to a built extension directory")
    parsed = urlparse(args.url)
    original_file_link = bool(getattr(args, "original_file_link", False))
    allowed_hosts = {"commons.wikimedia.org", "www.commons.wikimedia.org"}
    if parsed.scheme != "https" or parsed.hostname not in allowed_hosts:
        raise ValueError("The public subtitle smoke URL must be an HTTPS Wikimedia Commons page")

    result: dict[str, Any] = {
        "passed": False,
        "browser": args.browser,
        "url": args.url,
        "provider": "qwencloud",
        "model": "qwen3.5-livetranslate-flash-realtime",
        "runtime": str(Path(args.runtime).resolve()) if args.runtime else None,
        "checks": [],
    }
    driver = None
    page_handle = None
    temp_context = None
    stop_requested_on_end = False
    live_snapshot: dict[str, Any] = {"poll_samples": 0, "saw_core_final": False, "saw_expected_overlay": False}
    try:
        result["core_configuration"] = _configure_core(api_key)
        result["checks"].append("Core confirmed encrypted QwenCloud configuration")
        temp_context = tempfile.TemporaryDirectory(prefix="qwencloud-public-subtitles-")
        temp = Path(temp_context.__enter__())
        result["profile_root"] = str(temp.resolve())
        profile = temp / "profile"
        try:
            driver, popup_url, extension_id = _open_driver(args, extension, profile)
            driver.set_page_load_timeout(45)
            driver.get(args.url)
            commons_handle = driver.current_window_handle
            original_link = None
            if original_file_link:
                commons_url, media_url = _open_original_file_link(driver)
                original_link = {"commons_url": commons_url, "media_url": media_url}
                result["original_file_link"] = original_link
            page_handle = driver.current_window_handle
            media = _activate_original_player(driver, native_media_document=original_file_link)
            expected_media = "wikipedia" in args.url.lower() or "wikipedia" in driver.title.lower()
            if original_link:
                expected_media = "wikipedia" in original_link["media_url"].lower()
            if not expected_media:
                raise RuntimeError(f"public page did not expose the expected Wikipedia media: {media}")
            page_media_id = None
            if original_file_link:
                page_media_id = driver.execute_script("""
                  const m=[...document.querySelectorAll('audio,video')].find(v=>v.currentSrc||v.src);
                  if(!m) return null;
                  const r=m.getBoundingClientRect();
                  const host=[...document.querySelectorAll('[data-hls-subtitle]')].find(h=>{
                    const b=h.getBoundingClientRect();
                    return Math.abs(b.left-r.left)<2&&Math.abs(b.top-r.top)<2&&Math.abs(b.width-r.width)<2;
                  });
                  return host?.dataset.hlsSubtitle||null;
                """)
                if page_media_id:
                    result["page_media_id"] = page_media_id
            result["page"] = {"title": driver.title, "media": media}
            result["checks"].append("Wikimedia page exposed a real playable media element")
            if original_file_link:
                from selenium.webdriver.common.keys import Keys
                from selenium.webdriver.common.action_chains import ActionChains

                ActionChains(driver).key_down(Keys.CONTROL).key_down(Keys.SHIFT).send_keys("y").key_up(Keys.SHIFT).key_up(Keys.CONTROL).perform()
                result["checks"].append("Ctrl+Shift+Y was sent to the production browser command on the Wikimedia media tab")
                if args.browser == "edge":
                    _trigger_chromium_action(driver, extension_id, driver.current_url)
                    result["checks"].append("Chromium toolbar action invoked on the active Wikimedia media tab")

            driver.switch_to.new_window("tab")
            popup_handle = driver.current_window_handle
            driver.get(popup_url)
            _wait(lambda: driver.execute_script("return Boolean(document.querySelector('[data-subtitle-panel]'))"),
                  "production extension popup did not load")
            _wait(lambda: bool(_extension_state(driver).get("players")),
                  "extension did not inventory the page's real media player")
            state = _extension_state(driver)
            players = state.get("players") or []
            if not any("media-" in str(player.get("mediaId", "")) for player in players):
                raise RuntimeError(f"extension player inventory is invalid: {players}")
            selected_player = next((player for player in players if player.get("mediaId") == page_media_id), None)
            if selected_player is None and original_file_link:
                selected_player = max((player for player in players if int(player.get("frameId", -1)) == 0),
                                      key=lambda player: float(player.get("area", 0)), default=None)
                if selected_player is None:
                    raise RuntimeError(f"active Original file player was not found in the top frame: {players}")
            if selected_player is None:
                selected_player = players[0]
            result["extension_id"] = extension_id
            result["players"] = players
            result["selected_player"] = selected_player
            result["checks"].append("production popup runtime message found the webpage media player")

            driver.switch_to.window(page_handle)
            driver.execute_script("""
              const m=[...document.querySelectorAll('audio,video')].find(v=>v.currentSrc||v.src);
              if(!m) throw Error('no page media element');
              m.pause(); if(m.currentTime>.05) m.currentTime=0; m.muted=false; m.volume=1;
            """)
            driver.switch_to.window(popup_handle)
            _set_settings(driver, {"enabled": True, "source": "en", "target": "zh", "showSource": True, "prefetch": False})
            start_response = _require_ok(_message(driver, {"type": "subtitle-start", "frameId": selected_player["frameId"], "mediaId": selected_player["mediaId"]}), "start live subtitles")
            result["start_response"] = start_response
            driver.switch_to.window(page_handle)
            _wait(lambda: bool(driver.execute_script("return document.querySelector('[data-hls-subtitle]')?.shadowRoot")),
                  "production subtitle overlay did not attach to the webpage player")
            driver.execute_script("""
              const m=[...document.querySelectorAll('audio,video')].find(v=>v.currentSrc||v.src);
              if(!m) throw Error('no page media element');
              m.muted=false; m.volume=1;
            """)
            try:
                playback = driver.execute_async_script("""
                  const done=arguments[arguments.length-1];
                  const m=[...document.querySelectorAll('audio,video')].find(v=>v.currentSrc||v.src);
                  if(!m) return done({ok:false,error:'no active media element'});
                  Promise.resolve(m.play()).then(()=>done({ok:true,paused:m.paused,currentTime:m.currentTime}),
                    error=>done({ok:false,error:String(error),paused:m.paused,currentTime:m.currentTime}));
                """)
            except Exception as error:
                playback = {"ok": False, "error": str(error)}
            if not playback.get("ok"):
                from selenium.webdriver.common.by import By

                visible_controls = [button for button in driver.find_elements(By.CSS_SELECTOR, ".mw-tmh-play")
                                    if button.is_displayed() and button.is_enabled()]
                if visible_controls:
                    icons = visible_controls[0].find_elements(By.CSS_SELECTOR, ".mw-tmh-play-icon")
                    (icons[0] if icons and icons[0].is_displayed() else visible_controls[0]).click()
            result["playback_attempt"] = playback
            _wait(lambda: any(not m["paused"] and m["currentTime"] > 0 for m in _media_inventory(driver)),
                  "the original webpage media did not begin playback after capture started", timeout=20)

            def final_caption():
                nonlocal stop_requested_on_end
                inventory = _media_inventory(driver)
                visible = _caption_state(driver)
                source = " ".join(item["source"] for item in visible).strip()
                translation = " ".join(item["translation"] for item in visible).strip()
                state = _extension_state_popup(driver, popup_handle, page_handle)
                session = state.get("session") or {}
                live_snapshot.update(media=inventory, captions=visible,
                                     session={key: session.get(key) for key in ("id", "status", "capture", "epoch", "error", "mediaId")})
                if not session.get("id"):
                    return None
                try:
                    core = _core_poll_popup(driver, popup_handle, page_handle, session["id"])
                except Exception as error:
                    live_snapshot["core_poll_error"] = str(error)
                    return None
                cues = core.get("events") or []
                live_snapshot["poll_samples"] += 1
                live_snapshot["core"] = {key: core.get(key) for key in ("status", "epoch", "latest_sequence")}
                live_snapshot["core_events"] = [
                    {key: cue.get(key) for key in ("role", "text", "final", "start", "end", "epoch", "sequence")}
                    for cue in cues if cue.get("type") == "cue"
                ]
                source_final = " ".join(str(cue.get("text", "")) for cue in cues
                                         if cue.get("role") == "source" and cue.get("final") is True)
                translation_final = " ".join(str(cue.get("text", "")) for cue in cues
                                              if cue.get("role") == "translation" and cue.get("final") is True)
                has_core_final = EXPECTED_SOURCE in source_final.lower() and EXPECTED_TRANSLATION in translation_final
                has_visible_caption = EXPECTED_SOURCE in source.lower() and EXPECTED_TRANSLATION in translation
                live_snapshot["saw_core_final"] = live_snapshot["saw_core_final"] or has_core_final
                live_snapshot["saw_expected_overlay"] = live_snapshot["saw_expected_overlay"] or has_visible_caption
                if has_visible_caption:
                    live_snapshot["expected_overlay"] = {"captions": visible, "source": source, "translation": translation}
                    if not result.get("visible_caption_screenshot"):
                        screenshot = Path(args.report).with_suffix(".visible-caption.png") if args.report else Path.cwd() / "qwencloud-public-subtitles.visible-caption.png"
                        screenshot.parent.mkdir(parents=True, exist_ok=True)
                        if not driver.save_screenshot(str(screenshot)):
                            raise RuntimeError("could not save the visible subtitle overlay screenshot")
                        result["visible_caption_screenshot"] = str(screenshot.resolve())
                if live_snapshot["saw_expected_overlay"] and live_snapshot["saw_core_final"]:
                    return {"media": inventory, **live_snapshot["expected_overlay"],
                            "core_final_cues": [cue for cue in cues if cue.get("type") == "cue" and cue.get("final") is True],
                            "session_status": session.get("status")}
                if (not stop_requested_on_end and session.get("status") == "running"
                        and inventory and all(item["ended"] for item in inventory)):
                    stop_requested_on_end = True
                    current = driver.current_window_handle
                    driver.switch_to.window(popup_handle)
                    try:
                        stop = _require_ok(_message(driver, {"type": "subtitle-stop"}), "stop subtitles after media ended")
                        live_snapshot["stop_after_media_end"] = stop
                        result["checks"].append("production subtitle-stop action was sent after the short media ended to flush final cues")
                    finally:
                        driver.switch_to.window(current)
                return None

            caption = _wait(final_caption, "cloud final did not show Wikipedia source and Chinese translation", timeout=40)
            live_state = _extension_state_popup(driver, popup_handle, page_handle)
            session = live_state.get("session") or {}
            if session.get("status") not in {"running", "stopped"} or session.get("capture") not in {"element", "tab"}:
                raise RuntimeError(f"cloud subtitles did not come from webpage audio capture: {session}")
            expected_capture = "tab" if args.browser == "edge" and original_file_link else "element"
            if session.get("capture") != expected_capture:
                raise RuntimeError(f"expected {expected_capture} capture for {args.browser}, got {session}")
            result["caption"] = caption
            result["live_session"] = {"status": session.get("status"), "capture": session.get("capture"),
                                      "epoch": session.get("epoch"), "media_id": session.get("mediaId")}
            result["checks"].append("real webpage audio produced Core final English and Chinese subtitle cues")
            result["checks"].append("extension reports capture from the selected webpage player")
            screenshot = Path(args.report).with_suffix(".png") if args.report else Path.cwd() / "qwencloud-public-subtitles.png"
            screenshot.parent.mkdir(parents=True, exist_ok=True)
            driver.save_screenshot(str(screenshot))
            result["screenshot"] = str(screenshot.resolve())

            if session.get("status") == "running":
                before_seek = _extension_state_popup(driver, popup_handle, page_handle)
                driver.switch_to.window(page_handle)
                driver.execute_script("""
                  const m=[...document.querySelectorAll('audio,video')].find(v=>v.currentSrc||v.src);
                  if(!m||!Number.isFinite(m.duration)||m.duration<0.2) throw Error('media cannot be seeked');
                  m.currentTime=Math.min(0.05,Math.max(0,m.duration-0.1));
                """)
                epoch_before = int(before_seek.get("session", {}).get("epoch", 0))
                after_seek = _wait(lambda: (s if (s := _extension_state_popup(driver, popup_handle, page_handle)).get("session")
                                             and int(s["session"].get("epoch", 0)) > epoch_before else None),
                                   "seek did not advance the live subtitle session epoch", timeout=25)
                epoch_after = int(after_seek["session"].get("epoch", 0))
                if after_seek["session"].get("status") in {"stopped", "failed"} or epoch_after <= epoch_before:
                    raise RuntimeError(f"seek boundary did not advance the live session epoch: before={epoch_before}, after={epoch_after}")
                result["seek"] = {"epoch_before": epoch_before, "epoch_after": epoch_after}
                result["checks"].append("seek reset advanced the live subtitle session epoch")

                driver.switch_to.window(popup_handle)
                stop = _require_ok(_message(driver, {"type": "subtitle-stop"}), "stop subtitles")
                stopped = _wait(lambda: (s if (s := _extension_state(driver)).get("session")
                                          and s["session"].get("status") == "stopped" else None),
                                "subtitle session did not stop", timeout=15)
                result["stop"] = stop
                result["stopped_session"] = stopped["session"]
            else:
                if stop_requested_on_end:
                    result["checks"].append("short media ended naturally; production stop flushed Core final cues; long video seek/reset was not exercised")
                else:
                    result["checks"].append("short media ended naturally after Core final cues; long video seek/reset and manual stop were not exercised")
            result["passed"] = True
        except Exception:
            raise
    except Exception as error:
        result["error"] = str(error)
        if live_snapshot.get("poll_samples"):
            result["live_snapshot"] = live_snapshot
        if driver is not None:
            try:
                if page_handle in driver.window_handles:
                    driver.switch_to.window(page_handle)
                result["page_diagnostics"] = _page_diagnostics(driver, args)
            except Exception as diagnostic_error:
                result["page_diagnostics_error"] = str(diagnostic_error)
    finally:
        if driver is not None:
            try:
                driver.quit()
            except Exception as error:
                result["driver_cleanup_error"] = str(error)
        if temp_context is not None:
            try:
                temp_context.__exit__(None, None, None)
            except Exception as error:
                result["profile_cleanup_error"] = str(error)

    return _redact(result, api_key)


def _extension_state_popup(driver, popup_handle: str, page_handle: str) -> dict[str, Any]:
    current = driver.current_window_handle
    try:
        driver.switch_to.window(popup_handle)
        state = _extension_state(driver)
        driver.switch_to.window(page_handle)
        return state
    finally:
        if current in driver.window_handles and current not in {popup_handle, page_handle}:
            driver.switch_to.window(current)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser", choices=("edge", "firefox"), default="edge")
    parser.add_argument("--extension", type=Path, required=True)
    parser.add_argument("--driver", help="EdgeDriver or geckodriver path")
    parser.add_argument("--runtime", help="Extracted browser runtime directory")
    parser.add_argument("--url", default=DEFAULT_URL)
    parser.add_argument("--original-file-link", action="store_true", help="Click Commons' real Original file link before capture")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--headed", action="store_true")
    parser.add_argument("--use-configured-key", action="store_true", help="Use QwenCloud credentials already configured in Core")
    args = parser.parse_args()
    api_key = "" if args.use_configured_key else sys.stdin.readline().rstrip("\r\n")
    if not api_key and not args.use_configured_key:
        parser.error("API key must be provided on stdin")
    report = run(args, api_key)
    encoded = json.dumps(report, ensure_ascii=False, indent=2)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)
    return 0 if report.get("passed") else 1


if __name__ == "__main__":
    raise SystemExit(main())
