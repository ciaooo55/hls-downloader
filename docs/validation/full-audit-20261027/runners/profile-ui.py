import json
import os
from pathlib import Path
import subprocess
import time
import psutil

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1'
processes = [p for p in psutil.process_iter(['exe']) if p.info['exe']
             and Path(p.info['exe']).is_relative_to(r'C:\QAWork\runtime')
             and p.name().lower() == 'hlsdownloader.exe']
target = max(processes, key=lambda p: sum(p.cpu_times()[:2]))
before = {t.id:t.user_time+t.system_time for t in target.threads()}
time.sleep(5)
after = {t.id:t.user_time+t.system_time for t in target.threads()}
hot = sorted([{'thread_id':tid, 'nid':hex(tid), 'cpu_seconds':cpu-before.get(tid,cpu)}
              for tid,cpu in after.items()], key=lambda item:item['cpu_seconds'], reverse=True)
root = Path(r'C:\QAReports')
(root/'ui-hot-threads.json').write_text(json.dumps({'pid':target.pid, 'sample_seconds':5, 'threads':hot},indent=2),'utf8')
with (root/'ui-thread-dump.txt').open('wb') as output:
    subprocess.run([r'C:\QAInput\profile-jdk\bin\jcmd.exe',str(target.pid),'Thread.print','-l'],
                   stdout=output,stderr=subprocess.STDOUT,timeout=30)
