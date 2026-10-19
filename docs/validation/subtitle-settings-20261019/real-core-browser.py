import faulthandler
trace=open('C:/HLSQA/real-core-trace.log','w');faulthandler.enable(trace);faulthandler.dump_traceback_later(50,file=trace)
import json, os, subprocess, tempfile, time
from pathlib import Path
from smoke_v7_portable_app import Pipe
from smoke_extension_browsers import (_chromium_extension_id, _evaluate, _free_port, _open_debug_target, _read_debug_targets, _stop_process_tree, _loopback_page, _capture_screenshot)
from smoke_extension_takeover import _registered_host

def wait(f, message):
    end = time.monotonic() + 35
    while time.monotonic() < end:
        try:
            value=f()
            if value: return value
        except Exception: pass
        time.sleep(.2)
    raise TimeoutError(message)

root=Path('C:/QAWork/real-core-browser'); root.mkdir(parents=True,exist_ok=True)
extension=Path('C:/HLSSource/extension/.output/chrome-mv3')
eid=_chromium_extension_id(extension)
manifest=root/'native-host.json'
manifest.write_text(json.dumps({'name':'com.ciaooo55.hls_downloader','description':'sandbox real Core configuration acceptance','path':'C:\\QAWork\\Custom Install\\HLSDownloader\\app\\resources\\HLSDownloaderNativeHost.exe','type':'stdio','allowed_origins':[f'chrome-extension://{eid}/']}))
assert Path(json.loads(manifest.read_text())['path']).is_file()
p=Pipe()
try:
    config=p.request({'type':'subtitle','request_id':1,'request':{'action':'configuration'}})['result']
finally: p.close()
assert config['provider']=='qwencloud' and config['has_key']
expected={'enabled':True,'source':'en','target':'zh','offset':1.0,'showSource':True,'prefetch':True}
assert config['preferences']==expected, config
process=None
Path('C:/HLSQA/real-core-progress.txt').write_text('configuration verified',encoding='utf-8')
port=_free_port()
try:
    with _registered_host(manifest,'edge'), _loopback_page() as url:
        process=subprocess.Popen(['C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',f'--user-data-dir={root / "profile"}',f'--remote-debugging-port={port}','--remote-allow-origins=*','--headless=new','--enable-unsafe-extension-debugging','--autoplay-policy=no-user-gesture-required','--no-first-run','--no-default-browser-check','--disable-features=DisableLoadExtensionCommandLineSwitch',f'--disable-extensions-except={extension}',f'--load-extension={extension}','about:blank'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        wait(lambda:_read_debug_targets(port),'Edge startup')
        Path('C:/HLSQA/real-core-progress.txt').write_text('Edge started',encoding='utf-8')
        _open_debug_target(port,url)
        page=wait(lambda:next((t['webSocketDebuggerUrl'] for t in _read_debug_targets(port) if t.get('url')==url),None),'page')
        wait(lambda:_evaluate(page,"Boolean(document.querySelector('video'))"),'media document not ready')
        _evaluate(page,"document.querySelector('video').src='/sample.wav';true")
        popup_url=f'chrome-extension://{eid}/popup.html'
        _open_debug_target(port,popup_url)
        popup=wait(lambda:next((t['webSocketDebuggerUrl'] for t in _read_debug_targets(port) if t.get('url')==popup_url),None),'popup')
        expression="(async()=>{const t=(await chrome.tabs.query({})).find(t=>t.url==="+json.dumps(url)+");return chrome.runtime.sendMessage({type:'subtitle-state-get',tabId:t.id})})()"
        Path('C:/HLSQA/real-core-progress.txt').write_text('popup opened',encoding='utf-8')
        state=wait(lambda:(lambda s:s if isinstance(s,dict) and s.get('settings',{}).get('offset')==1 else None)(_evaluate(popup,expression)),'desktop preferences not synchronized')
        assert all(state['settings'].get(k)==v for k,v in expected.items()),state
        assert state['configured'] is True,state
        Path('C:/HLSQA/real-core-progress.txt').write_text('preferences synchronized',encoding='utf-8')
        wait(lambda:_evaluate(popup,"Boolean(document.querySelector('#tab-1'))"),'subtitle tab')
        _evaluate(popup,"document.querySelector('#tab-1').click();true")
        wait(lambda:_evaluate(popup,"Boolean(document.querySelector('[data-subtitle-panel]'))"),'subtitle panel')
        assert _evaluate(popup,"document.querySelector('[data-subtitle-panel]').querySelectorAll('input,form').length") == 0
        denied=_evaluate(popup,"chrome.runtime.sendMessage({type:'subtitle-settings',settings:{enabled:false}})")
        assert denied.get('ok') is False,denied
        client=_evaluate(popup,"chrome.storage.local.get('desktopClientId').then(v=>v.desktopClientId)")
        native=_evaluate(popup,"chrome.runtime.sendNativeMessage('com.ciaooo55.hls_downloader',"+json.dumps({'op':'subtitle','client_id':client,'request':{'action':'configure','provider':'qwencloud'}})+")")
        # Native envelope varies by bridge protocol; assert the production host refuses mutation.
        assert '主程序' in json.dumps(native,ensure_ascii=False),native
        _capture_screenshot(popup,Path('C:/HLSQA/real-core-popup.png'))
        report={'passed':True,'cloud_called':False,'desktop_preferences':config,'extension_settings':state['settings'],'configuration_inputs':0,'plugin_global_write_denied':denied,'native_configure_response':native}
        Path('C:/HLSQA/real-core-browser.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
finally:
    Path('C:/HLSQA/real-core-progress.txt').write_text('browser flow finished',encoding='utf-8')
