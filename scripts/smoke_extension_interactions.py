"""Actual Firefox public-page and extension button interactions, with UI metrics."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import tempfile
import time
import traceback
import zipfile

from selenium import webdriver
from selenium.webdriver import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.support.ui import Select
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service
from selenium.common.exceptions import StaleElementReferenceException

from smoke_extension_media import _find_firefox
from smoke_v7_portable_app import Pipe, wait
from smoke_v7_browser_download_confirmation import click_download_confirmation


PAGE = "https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html"
MEDIA = "https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm"
EXPECTED = "c6f8a348953395598a9a73b9bab1676436410797bce9f398f4be1531d6e76dda"


def run(args):
    args.report.parent.mkdir(parents=True, exist_ok=True)
    evidence = args.report.parent
    result = {"passed": False, "page": PAGE, "checks": [], "buttons": []}
    driver = None
    original_tasks = set()
    created = set()
    original_settings = None

    def core(message):
        p = Pipe()
        try:
            return p.request(message)
        finally:
            p.close()

    def record(name, actual=True):
        result["checks"].append({"name": name, "actual": actual})
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
        print(json.dumps({"check": name}, ensure_ascii=True), flush=True)

    def message(value):
        return driver.execute_async_script(
            "const [m,done]=arguments;browser.runtime.sendMessage(m).then(done,e=>done({ok:false,error:String(e)}))", value)

    def popup_button(text):
        return wait(lambda: next((b for b in driver.find_elements(By.TAG_NAME, "button")
                                 if b.is_displayed() and text in b.text), None), "Missing popup button: " + text, 12)

    def styles(element):
        return driver.execute_script("""
            const e=arguments[0],r=e.getBoundingClientRect(),s=getComputedStyle(e);
            return {text:e.textContent.trim(),title:e.title,label:e.getAttribute('aria-label'),
                    width:r.width,height:r.height,x:r.x,y:r.y,disabled:e.disabled,
                    background:s.backgroundColor,color:s.color,transform:s.transform,
                    border:s.borderColor,shadow:s.boxShadow,outline:s.outlineColor};
        """, element)

    def measure(element, context):
        if not element.is_displayed():
            return
        driver.execute_script("arguments[0].scrollIntoView({block:'center',inline:'nearest'})", element)
        time.sleep(.15)
        ActionChains(driver).move_by_offset(0, 0).perform()
        driver.execute_script("document.activeElement?.blur()")
        neutral = ActionChains(driver)
        neutral.w3c_actions.pointer_action.move_to_location(2, 2)
        neutral.perform()
        driver.get_screenshot_as_png()
        diagnostic = driver.execute_script("return {hover:arguments[0].matches(':hover'),animations:arguments[0].getAnimations().map(a=>({state:a.playState,time:a.currentTime,type:a.constructor.name})),title:arguments[0].title}",element)
        if diagnostic["hover"]:
            print(json.dumps({"neutral_pointer":diagnostic}),flush=True)
        time.sleep(.25)
        rest = styles(element)
        ActionChains(driver).move_to_element(element).perform()
        driver.get_screenshot_as_png()
        time.sleep(.25)
        hover = styles(element)
        ActionChains(driver).click_and_hold(element).perform()
        time.sleep(.18)
        press = styles(element)
        neutral = ActionChains(driver)
        neutral.w3c_actions.pointer_action.move_to_location(2, 2)
        neutral.release().perform()
        fields = ("background", "color", "transform", "border", "shadow", "outline")
        item = {"context": context, "rest": rest, "hover": hover, "press": press,
                "hover_changes": [f for f in fields if rest[f] != hover[f]],
                "press_changes": [f for f in fields if hover[f] != press[f]]}
        result["buttons"].append(item)
        if not rest["disabled"]:
            assert rest["width"] >= 24 and rest["height"] >= 24, (context, rest)
            assert item["hover_changes"], ("missing hover feedback", context, rest)
            assert item["press_changes"], ("missing press feedback", context, rest)

    def measure_stable(locator, context):
        # 播放器初始化和资源扫描完成时允许一次 DOM 换代，重新定位后测同一按钮。
        for attempt in range(3):
            try:
                return measure(locator(), context)
            except StaleElementReferenceException:
                neutral = ActionChains(driver)
                neutral.w3c_actions.pointer_action.move_to_location(2, 2)
                neutral.release().perform()
                if attempt == 2:
                    raise

    with tempfile.TemporaryDirectory(prefix="all-buttons-firefox-", dir=args.runtime.resolve().parents[1]/"test-tmp") as temporary:
        stage = Path(temporary)
        try:
            original_settings = core({"type": "load_settings", "request_id": 1})
            original_tasks = {t["task_id"] for t in core({"type":"snapshot","request_id":2})["tasks"]}
            addon = stage / "extension.xpi"
            with zipfile.ZipFile(addon, "w", zipfile.ZIP_DEFLATED) as archive:
                for f in sorted(args.extension.resolve().rglob("*")):
                    if f.is_file(): archive.write(f, f.relative_to(args.extension.resolve()).as_posix())
            options = Options()
            options.binary_location = str(_find_firefox())
            options.add_argument("-headless")
            options.set_preference("media.autoplay.default", 0)
            service = Service(executable_path=str(args.driver),
                              service_args=["--profile-root", str(stage), "--allow-system-access"],
                              log_output=str(evidence / "geckodriver.log"))
            driver = webdriver.Firefox(options=options, service=service)
            driver.set_window_size(1280, 900)
            driver.set_page_load_timeout(40)
            driver.set_script_timeout(20)
            eid = driver.install_addon(str(addon), temporary=True)
            driver.set_context("chrome")
            uuid = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))[eid]
            driver.set_context("content")
            driver.get(PAGE)
            player = driver.current_window_handle
            def find_video():
                driver.switch_to.default_content()
                if driver.find_elements(By.TAG_NAME, "video"):
                    return True
                for frame in driver.find_elements(By.TAG_NAME, "iframe"):
                    driver.switch_to.frame(frame)
                    if driver.find_elements(By.TAG_NAME, "video"):
                        return True
                    driver.switch_to.default_content()
                return False
            wait(find_video, "Public page contains no video player", 20)
            driver.execute_script("document.querySelector('video').muted=true;document.querySelector('video').play()")
            wait(lambda: driver.execute_script("return Boolean(document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.video-download:not(.identifying)'))"), "Public video not identified", 40)

            def overlay(selector):
                return driver.execute_script("return document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector(arguments[0])||null", selector)

            if not args.popup_only:
                measure_stable(lambda: overlay(".video-download"), "public video download")
                measure_stable(lambda: overlay(".video-more"), "public video more")
                ActionChains(driver).move_to_element(overlay(".video-download")).perform()
                time.sleep(.3)
                hover = styles(overlay(".video-hover"))
                viewport = driver.execute_script("return innerWidth")
                assert hover["width"] > 0 and hover["x"] >= 0 and hover["x"]+hover["width"] <=viewport
                driver.save_screenshot(str(evidence/"public-video-hover.png"))
                record("real webpage hover card visible and inside viewport", hover)
                overlay(".video-download").click()
                click_download_confirmation()
                def downloaded():
                    task = next((t for t in core({"type":"snapshot","request_id":3})["tasks"]
                                 if t["task_id"] not in original_tasks and t["url"] == MEDIA), None)
                    if task:
                        created.add(task["task_id"])
                        return task if task["status"] in ("completed", "failed") else None
                task = wait(downloaded, "Actual public webpage download failed", 60)
                assert task["status"] == "completed", task
                file = Path(task["output_path"])
                assert file.stat().st_size == 554058 and hashlib.sha256(file.read_bytes()).hexdigest() == EXPECTED
                record("public overlay click -> actual Native Host/Core -> completed and SHA256", {"bytes":file.stat().st_size,"sha256":EXPECTED})

                overlay(".video-more").click()
                wait(lambda: overlay(".wrap") and overlay(".wrap").is_displayed(), "More button did not expand resource panel", 10)
                overlay(".pin").click()
                assert "已固定" in overlay(".pin").text
                for selector in (".pin", ".close"):
                    measure(overlay(selector), "expanded overlay " + selector)
                overlay(".pin").click()
                overlay(".close").click()
                record("overlay more, pin/unpin and collapse buttons")
                driver.save_screenshot(str(evidence/"public-video-collapsed.png"))

                # 真实拖动不能触发下载，靠近窗口边缘时悬浮卡仍应完整可见。
                count_before_drag = len(core({"type":"snapshot","request_id":31})["tasks"])
                for edge in ("top-left", "bottom-right"):
                    width, height = driver.execute_script("return [innerWidth,innerHeight]")
                    target_x, target_y = (15, 15) if edge == "top-left" else (width-15, height-15)
                    chain = ActionChains(driver).click_and_hold(overlay(".video-download"))
                    chain.w3c_actions.pointer_action.move_to_location(target_x, target_y)
                    chain.release().perform()
                    time.sleep(.6)
                    group = styles(overlay(".video-action-group"))
                    assert group["x"] >= 0 and group["y"] >= 0
                    assert group["x"]+group["width"] <= width and group["y"]+group["height"] <= height, group
                    ActionChains(driver).move_to_element(overlay(".video-download")).perform()
                    time.sleep(.35)
                    card = styles(overlay(".video-hover"))
                    assert card["x"] >= 0 and card["y"] >= 0 and card["x"]+card["width"] <= width and card["y"]+card["height"] <= height, card
                    driver.save_screenshot(str(evidence/("drag-"+edge+".png")))
                    record("real drag and hover card at " + edge, {"button":group,"card":card})
                assert len(core({"type":"snapshot","request_id":32})["tasks"]) == count_before_drag
                driver.set_window_size(720, 650)
                time.sleep(.7)
                ActionChains(driver).move_to_element(overlay(".video-download")).perform()
                time.sleep(.35)
                width, height = driver.execute_script("return [innerWidth,innerHeight]")
                group = styles(overlay(".video-action-group"))
                card = styles(overlay(".video-hover"))
                assert group["x"]+group["width"] <= width and card["x"]+card["width"] <= width and card["y"]+card["height"] <= height
                record("resize repositions dragged controls and hover card without unintended download")
                driver.set_window_size(1280, 900)
                for zoom in (1.0, 1.5, 2.0):
                    driver.set_context('chrome')
                    driver.execute_script('ZoomManager.zoom=arguments[0]', zoom)
                    driver.set_context('content')
                    time.sleep(.5)
                    ActionChains(driver).move_to_element(overlay('.video-download')).perform()
                    time.sleep(.35)
                    width,height=driver.execute_script('return [innerWidth,innerHeight]')
                    group=styles(overlay('.video-action-group'))
                    card=styles(overlay('.video-hover'))
                    for rect in (group,card):
                        assert rect['x']>=0 and rect['y']>=0 and rect['x']+rect['width']<=width+1 and rect['y']+rect['height']<=height+1,rect
                    overlay('.video-more').click()
                    wait(lambda:overlay('.wrap').is_displayed(),'Zoomed more button did not open',10)
                    overlay('.close').click()
                    record(f'actual Firefox page zoom {round(zoom*100)}% hover bounds and clickable controls')
                driver.set_context('chrome')
                driver.execute_script('ZoomManager.zoom=1')
                driver.set_context('content')

            driver.switch_to.new_window("tab")
            popup = driver.current_window_handle
            popup_url = f"moz-extension://{uuid}/popup.html"
            driver.get(popup_url)
            wait(lambda: driver.execute_script("return document.documentElement.dataset.popupReady==='ready'"), "Popup not ready", 25)
            assert message({"type":"ping"}).get("ok") is True
            record("popup actually connected to installed Native Host/Core")
            for b in driver.find_elements(By.TAG_NAME,"button"):
                if b.is_displayed() and not args.actions_only: measure(b,"popup light")
            driver.save_screenshot(str(evidence/"popup-light.png"))

            theme = driver.find_element(By.CSS_SELECTOR,".header-actions button:first-child")
            for _ in range(3):
                old = theme.get_attribute("title")
                ActionChains(driver).click(theme).perform()
                wait(lambda: theme.get_attribute("title") != old, "Theme did not change", 5)
            record("popup theme button cycles through automatic/light/dark")
            # 使用实际主题按钮，直到进入深色态，再测交互样式。
            for _ in range(3):
                if driver.execute_script("return document.documentElement.dataset.hlsdTheme") == "dark": break
                ActionChains(driver).click(theme).perform()
                time.sleep(.2)
            assert driver.execute_script("return document.documentElement.dataset.hlsdTheme") == "dark"
            for b in driver.find_elements(By.TAG_NAME,"button"):
                if b.is_displayed() and not args.actions_only: measure(b,"popup dark")

            for label in ("自动接管", "本站 Cookie", "本站提示"):
                b = popup_button(label)
                old = b.get_attribute("aria-pressed")
                b.click()
                wait(lambda: popup_button(label).get_attribute("aria-pressed") != old, "Popup toggle did not change: " + label, 10)
                popup_button(label).click()
                wait(lambda: popup_button(label).get_attribute("aria-pressed") == old, "Popup toggle did not restore: " + label, 10)
                record("popup toggle and restore: " + label)
            popup_button("重新识别").click()
            wait(lambda: driver.execute_script("return !document.querySelector('.scan-button').disabled"), "Rescan did not finish", 20)
            wait(lambda: driver.find_elements(By.CSS_SELECTOR,"article .copy-link"), "Rescan lost public video", 20)
            copy = driver.find_element(By.CSS_SELECTOR,"article .copy-link")
            driver.execute_script("arguments[0].scrollIntoView({block:'center'})", copy)
            copy.click()
            wait(lambda: "已复制" in driver.find_element(By.CSS_SELECTOR,"article .copy-link").text, "Copy link did not confirm success", 5)
            record("popup rescan and copy-link buttons")

            before_popup_download = {t["task_id"] for t in core({"type":"snapshot","request_id":41})["tasks"]}
            card = next(a for a in driver.find_elements(By.TAG_NAME,"article") if "flower.webm" in a.text)
            download_button = card.find_element(By.CSS_SELECTOR,'button[title="发送到下载器"]')
            driver.execute_script("arguments[0].scrollIntoView({block:'center'})",download_button)
            ActionChains(driver).click(download_button).perform()
            click_download_confirmation()
            def popup_downloaded():
                task=next((t for t in core({"type":"snapshot","request_id":42})["tasks"] if t["task_id"] not in before_popup_download and t["url"]==MEDIA),None)
                if task:
                    created.add(task["task_id"])
                    return task if task["status"] in ("completed","failed") else None
            task=wait(popup_downloaded,"Popup download did not complete",60)
            assert task["status"]=="completed" and hashlib.sha256(Path(task["output_path"]).read_bytes()).hexdigest()==EXPECTED,task
            record("popup download button -> actual public video -> completed and SHA256")

            # 字幕来源、目标、本页/本站开关是本地 UI 行为；真实云模型未配置。
            for label, value in (("来源语言", "en"), ("目标语言", "ja"), ("本站字幕翻译", "off"), ("本页字幕翻译", "on")):
                select = driver.find_element(By.CSS_SELECTOR,f'select[aria-label="{label}"]')
                original = select.get_attribute("value")
                Select(select).select_by_value(value)
                wait(lambda: driver.find_element(By.CSS_SELECTOR,f'select[aria-label="{label}"]').get_attribute("value")==value,
                     "Subtitle choice did not apply: "+label, 10)
                Select(driver.find_element(By.CSS_SELECTOR,f'select[aria-label="{label}"]')).select_by_value(original)
                record("subtitle popup select and restore: "+label)
            driver.find_element(By.CSS_SELECTOR, ".subtitle-panel summary").click()
            for b in driver.find_elements(By.CSS_SELECTOR, ".subtitle-provider button"):
                if not args.actions_only: measure(b, "subtitle provider")
            save_button = popup_button("保存服务设置")
            save_button.click()
            assert not driver.execute_script("return document.querySelector('.subtitle-provider').checkValidity()")
            record("empty subtitle credentials: browser validation prevents invalid save")
            popup_button("打开").click()
            record("popup open desktop button")
            driver.save_screenshot(str(evidence/"popup-final.png"))
            close_button=driver.find_element(By.CSS_SELECTOR,'button[title="关闭"]')
            ActionChains(driver).click(close_button).perform()
            wait(lambda: popup not in driver.window_handles,"Popup close button did not close window",10)
            driver.switch_to.window(player)
            record("popup close button closes its window and keeps webpage open")
            result["passed"] = True
        except Exception as error:
            result["error"] = str(error)
            result["traceback"] = traceback.format_exc()
            if driver:
                driver.save_screenshot(str(evidence/"failure.png"))
            print(json.dumps({"error":str(error)},ensure_ascii=True),flush=True)
        finally:
            if driver: driver.quit()
            for task_id in created:
                core({"type":"command","request_id":7,"command":{"kind":"task_action","task_id":task_id,"action":"delete_files"}})
            if original_settings is not None:
                core({"type":"store_settings","request_id":8,"values":{
                    "browser_takeover_enabled":original_settings["takeover_enabled"],
                    "browser_takeover_minimum_bytes":original_settings["takeover_minimum_bytes"]}})
            args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding="utf8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--popup-only", action="store_true")
    p.add_argument("--actions-only", action="store_true")
    p.add_argument("--runtime",type=Path,required=True)
    p.add_argument("--extension",type=Path,required=True)
    p.add_argument("--driver",type=Path,required=True)
    p.add_argument("--report",type=Path,required=True)
    raise SystemExit(run(p.parse_args()))
