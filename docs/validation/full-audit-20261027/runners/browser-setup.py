import json
import os
from pathlib import Path
import time
from smoke_v7_portable_app import Pipe

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1'
deadline = time.monotonic() + 20
while True:
    try:
        pipe = Pipe()
        break
    except OSError:
        if time.monotonic() > deadline:
            raise
        time.sleep(.1)
try:
    response = pipe.request({'type': 'store_settings', 'request_id': 1, 'values': {
        'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1',
        'download_dir': os.environ['HLS_V7_DOWNLOAD_DIR'],
        'download_complete_popup_enabled': False, 'download_progress_window_enabled': False,
        'av_scan_enabled': False, 'auto_retry_failed_max': 0,
    }})
    assert response.get('type') != 'error', response
    print(json.dumps(response))
finally:
    pipe.close()
