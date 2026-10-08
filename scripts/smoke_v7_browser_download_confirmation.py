"""Public Firefox media -> actual desktop confirmation -> verified file."""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
from pathlib import Path
import tempfile
import time
import uuid
import zipfile
import subprocess

import psutil

from selenium import webdriver
from selenium.webdriver import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service

from smoke_extension_media import _find_firefox
from smoke_v7_portable_app import Pipe, wait, runtime_processes
from smoke_v7_presenter_visual import capture, find_window, user32

PAGE = 'https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html'
MEDIA = 'https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm'
SHA256 = 'c6f8a348953395598a9a73b9bab1676436410797bce9f398f4be1531d6e76dda'


def presenter_window(title='确认下载', runtime=None):
    runtime = runtime or Path(__file__).resolve().parents[1] / '.tool-cache/runtime/HLSDownloader'
    for process in runtime_processes(runtime.resolve()):
        if process['name'] == 'HLSDownloaderPresenter.exe':
            handle = find_window(process['pid'], title)
            if handle:
                return handle
    return None


def click_download_confirmation(accept=True):
    handle = wait(presenter_window, 'Desktop confirmation did not appear', 10)
    user32.SetWindowPos(handle, wintypes.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0040)
    user32.SetForegroundWindow(handle)
    time.sleep(.1)
    rect = wintypes.RECT()
    user32.GetWindowRect(handle, ctypes.byref(rect))
    scale = user32.GetDpiForWindow(handle) / 96
    # 窗口底部固定的“确认下载”按钮；真实鼠标输入，保留生产回调。
    user32.SetCursorPos(rect.right - round((54 if accept else 134) * scale), rect.bottom - round(35 * scale))
    user32.mouse_event(2, 0, 0, 0, 0)
    user32.mouse_event(4, 0, 0, 0, 0)
    user32.SetWindowPos(handle, wintypes.HWND(-2), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010)


def run(args):
    args.report.parent.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'page': PAGE, 'checks': []}
    driver = None
    created = set()
    initial = set()
    unique_url = MEDIA + '?hls-confirmation=' + uuid.uuid4().hex

    def core(request):
        pipe = Pipe()
        try:
            return pipe.request(request)
        finally:
            pipe.close()

    def command(value):
        return core({'type': 'command', 'request_id': 1, 'command': value})

    def tasks():
        current = core({'type': 'snapshot', 'request_id': 1})['tasks']
        for task in current:
            if task['task_id'] not in initial and task['url'] == unique_url:
                created.add(task['task_id'])
        return current

    def record(name, actual=True):
        report['checks'].append({'name': name, 'actual': actual})
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
        print(json.dumps({'check': name}, ensure_ascii=True), flush=True)

    def popup_window(title='确认下载'):
        return presenter_window(title, args.runtime)

    def reject_confirmation():
        click_download_confirmation(accept=False)

    def media_button():
        return driver.execute_script("return document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.video-download:not(.identifying)') || null")

    def click_media():
        button = wait(lambda: (b if (b := media_button()) and b.is_enabled() else None), 'Media action unavailable', 15)
        started = time.perf_counter()
        ActionChains(driver).move_to_element(button).click().perform()
        handle = wait(popup_window, 'Web click did not display desktop confirmation', 10)
        latency = (time.perf_counter() - started) * 1000
        capture(handle, args.report.with_name(f'confirmation-{len(report["checks"])}.png'))
        assert not any(t['task_id'] not in initial and t['url'] == unique_url for t in tasks()), 'Task started before confirmation'
        record('public-media-click-shows-confirmation-before-task', {'latency_ms': round(latency, 2)})

    with tempfile.TemporaryDirectory(prefix='browser-confirmation-', dir=args.runtime.resolve().parents[1] / 'test-tmp') as temporary:
        stage = Path(temporary)
        try:
            initial = {t['task_id'] for t in tasks()}
            if args.cold_ui:
                stopped = []
                for process in runtime_processes(args.runtime.resolve()):
                    if process['name'] in ('HLSDownloader.exe', 'HLSDownloaderPresenter.exe'):
                        try:
                            p = psutil.Process(process['pid'])
                            p.terminate()
                            stopped.append(p)
                        except psutil.NoSuchProcess:
                            pass
                _, alive = psutil.wait_procs(stopped, timeout=10)
                assert not alive, 'Workbench or Presenter did not stop'
                record('workbench-and-presenter-closed-before-browser-click')
            addon = stage / 'extension.xpi'
            with zipfile.ZipFile(addon, 'w', zipfile.ZIP_DEFLATED) as archive:
                for source in args.extension.resolve().rglob('*'):
                    if source.is_file():
                        archive.write(source, source.relative_to(args.extension.resolve()).as_posix())
            options = Options()
            options.binary_location = str(_find_firefox())
            options.page_load_strategy = 'eager'
            if not args.headed:
                options.add_argument('-headless')
            options.set_preference('media.autoplay.default', 0)
            service = Service(str(args.driver), service_args=['--profile-root', str(stage), '--allow-system-access'], log_output=str(args.report.with_suffix('.gecko.log')))
            driver = webdriver.Firefox(options=options, service=service)
            driver.set_window_size(1280, 900)
            driver.set_page_load_timeout(40)
            driver.install_addon(str(addon), temporary=True)
            driver.get(PAGE)
            def find_video():
                driver.switch_to.default_content()
                if driver.find_elements(By.TAG_NAME, 'video'):
                    return True
                for frame in driver.find_elements(By.TAG_NAME, 'iframe'):
                    driver.switch_to.frame(frame)
                    if driver.find_elements(By.TAG_NAME, 'video'):
                        return True
                    driver.switch_to.default_content()
                return False
            wait(find_video, 'Public page has no video', 15)
            driver.execute_script('const v=document.querySelector("video");v.src=arguments[0];v.muted=true;v.loop=true;v.play();', unique_url)
            wait(lambda: driver.execute_script('return document.querySelector("video").currentTime > 0'), 'Public video did not play', 20)
            click_media()
            if args.cold_ui:
                assert not any(p['name'] == 'HLSDownloader.exe' for p in runtime_processes(args.runtime.resolve()))
                record('browser-wakes-confirmation-without-starting-workbench')
            reject_confirmation()
            wait(lambda: not popup_window(), 'Cancel did not close confirmation', 5)
            wait(lambda: media_button().is_enabled(), 'Cancel left webpage download disabled', 8)
            assert not created
            record('desktop-cancel-leaves-no-task-and-allows-retry')
            click_media()
            click_download_confirmation()

            def completed():
                task = next((t for t in tasks() if t['url'] == unique_url and t['task_id'] not in initial), None)
                if task and task['status'] == 'failed':
                    raise AssertionError(task.get('error_message'))
                return task if task and task['status'] == 'completed' else None

            task = wait(completed, 'Confirmed public media did not finish downloading', 45)
            assert hashlib.sha256(Path(task['output_path']).read_bytes()).hexdigest() == SHA256
            record('actual-confirm-button-downloads-public-file-with-correct-sha256', {'bytes': task['downloaded_bytes'], 'sha256': SHA256})
            record('completed-notification-visible', bool(wait(lambda: popup_window('下载完成'), 'Completion popup missing', 10)))
            # 工具栏弹窗与网页按钮共用同一桌面确认链路，重复的已完成任务仍应可见。
            driver.set_context('chrome')
            addon_id = json.loads((args.extension / 'manifest.json').read_text(encoding='utf8'))['browser_specific_settings']['gecko']['id']
            extension_uuid = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))[addon_id]
            driver.set_context('content')
            driver.switch_to.new_window('tab')
            driver.get(f'moz-extension://{extension_uuid}/popup.html')
            button = wait(lambda: next((b for b in driver.find_elements(By.TAG_NAME, 'button') if b.is_displayed() and b.text.strip() == '下载'), None), 'Popup media action missing', 15)
            ActionChains(driver).move_to_element(button).click().perform()
            handle = wait(popup_window, 'Popup download did not display confirmation', 10)
            capture(handle, args.report.with_name('popup-confirmation.png'))
            reject_confirmation()
            wait(lambda: not popup_window(), 'Popup cancel did not close confirmation', 5)
            record('toolbar-popup-also-opens-desktop-confirmation')
            report['passed'] = True
        except Exception as error:
            report['error'] = str(error)
            report['runtime_processes_at_failure'] = runtime_processes(args.runtime.resolve())
            report['handoffs_at_failure'] = [json.loads(item) for item in core({'type': 'load_handoffs', 'request_id': 1})['items'] if json.loads(item).get('offer', {}).get('url') == unique_url]
            if driver:
                driver.save_screenshot(str(args.report.with_name('failure.png')))
            raise
        finally:
            if driver:
                driver.quit()
            for encoded in core({'type': 'load_handoffs', 'request_id': 1})['items']:
                handoff = json.loads(encoded)
                if handoff.get('status') == 'pending' and handoff.get('offer', {}).get('url') == unique_url:
                    command({'kind': 'reject_handoff', 'handoff_id': handoff['id'], 'suppress_site_kind': False})
            # 只删除本次唯一地址创建的任务，绝不按编号范围删除用户任务。
            for task in tasks():
                if task['task_id'] in created:
                    command({'kind': 'task_action', 'task_id': task['task_id'], 'action': 'delete_files'})
            report['cleanup_preserved_original_task_ids'] = initial <= {t['task_id'] for t in tasks()}
            if args.cold_ui:
                subprocess.Popen([str(args.runtime.resolve() / 'HLSDownloader.exe')], cwd=args.runtime.resolve(), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                report['workbench_restart_requested'] = True
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--extension', type=Path, required=True)
    parser.add_argument('--driver', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--cold-ui', action='store_true')
    parser.add_argument('--headed', action='store_true')
    print(json.dumps(run(parser.parse_args()), ensure_ascii=False, indent=2))
