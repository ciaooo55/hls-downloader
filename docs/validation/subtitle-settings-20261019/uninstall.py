import json,subprocess,winreg
from pathlib import Path
import psutil
from smoke_extension_browsers import _stop_process_tree
root=Path('C:/QAWork/Custom Install/HLSDownloader')
_stop_process_tree(None,Path('C:/QAWork/real-core-browser/profile'))
for p in psutil.process_iter(['exe']):
    try:
        if p.info['exe'] and Path(p.info['exe']).is_relative_to(root): p.kill()
    except (psutil.NoSuchProcess,psutil.AccessDenied): pass
code=json.loads(Path('C:/HLSQA/package.json').read_text(encoding='utf-8-sig'))['product_code']
r=subprocess.run(['msiexec.exe','/x',code,'/qn','/norestart','/l*v','C:/HLSQA/uninstall.log'],timeout=120)
assert r.returncode==0,r.returncode
for hive in (winreg.HKEY_CURRENT_USER,winreg.HKEY_LOCAL_MACHINE):
    try:
        with winreg.OpenKey(hive,'Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\'+code): pass
    except FileNotFoundError: continue
    raise AssertionError('MSI uninstall registration remains')
assert not (root/'HLSDownloader.exe').exists()
Path('C:/HLSQA/uninstall-result.json').write_text(json.dumps({'passed':True,'exit_code':r.returncode,'msi_registration_removed':True,'launcher_removed':True,'host_install_changed':False},indent=2),encoding='utf-8')