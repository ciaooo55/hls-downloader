"""Exercise the existing installed cold/concurrent/resident startup gate without UI input."""
import json
import os
from pathlib import Path
import traceback

import psutil
from smoke_v7_installed_footprint import verify_native_host_startup

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Windows Sandbox only'
resources=Path(r'C:\QA Cold Startup\app\resources')
report=Path(r'C:\QAReports\cold-start.json')
result={'passed':False,'gate_ms':1500,'desktop_input_used':False}


def save():
    report.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf8')


try:
    verify_native_host_startup(resources,result,save)
except Exception:
    result['error']=traceback.format_exc()
finally:
    for process in psutil.process_iter(['exe']):
        if process.info['exe'] and Path(process.info['exe']).is_relative_to(resources.parent.parent):
            process.kill()
            process.wait(timeout=10)
    save()
raise SystemExit(0 if result['passed'] else 1)
