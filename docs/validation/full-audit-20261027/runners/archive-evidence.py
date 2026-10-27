"""Archive selected evidence with signed URL queries and credential strings removed."""
import hashlib
import json
from pathlib import Path
import re
from urllib.parse import urlsplit, urlunsplit

repo = Path(__file__).resolve().parents[4]
source = repo/'.tool-cache/full-audit-20261027/reports'
target = Path(__file__).resolve().parent.parent/'results'
target.mkdir(exist_ok=True)
selected = [
    'final-core-suite.stdout.txt','curl-regression.stdout.txt','winhttp-regression.stdout.txt',
    'final-real-merge.stdout.txt','before-post.stdout.txt','before-queue.stdout.txt',
    'final-task-import.json','http-boundaries.json','http-stall.json','download-playback.json',
    'firefox-ownership-final.json','edge-ownership-final.json','edge-ownership-final-v2.json',
    'ui-hot-threads.json','ui-thread-dump.txt','render-comparison-v2.json','render-comparison-fixed.json',
    'render-v2-SOFTWARE.log','render-fixed-SOFTWARE.log','ui-idle-fix-build.txt',
    'installed-audit.json','fresh-installed-audit.json','installed-create-pause.json',
    'installed-verify-paused.json','installed-verify-resume.json','comparison-clean.json',
    'hover-edge.json','hover-firefox.json',
]


def scrub_text(value):
    def clean_url(match):
        raw = match[0]
        try:
            url = urlsplit(raw)
            return urlunsplit((url.scheme,url.hostname or '',url.path,'[redacted-query]' if url.query else '', ''))
        except ValueError:
            return '[redacted-url]'
    value = re.sub(r'https?://[^\s"\'<>]+', clean_url, value)
    return re.sub(r'(?i)(bearer\s+)[a-z0-9._~-]+', r'\1[redacted]', value)


def scrub(value):
    if isinstance(value,dict):
        return {key: ('[redacted]' if isinstance(item,str) and re.search(
            r'(?i)token|secret|password|cookie|authorization|protected_blob|protected_context',key)
            else scrub(item)) for key,item in value.items()}
    if isinstance(value,list):
        return [scrub(item) for item in value]
    return scrub_text(value) if isinstance(value,str) else value


manifest = []
for name in selected:
    path = source/name
    if not path.is_file():
        continue
    raw = path.read_bytes()
    try:
        text = raw.decode('utf-8-sig')
    except UnicodeDecodeError:
        # Windows PowerShell 5.1的原生命令重定向可能沿用系统代码页。
        text = raw.decode('gb18030')
    content = (json.dumps(scrub(json.loads(text)),ensure_ascii=False,indent=2)+'\n'
               if name.endswith('.json') else scrub_text(text))
    (target/name).write_text(content,'utf8')
    manifest.append({'path':name,'original_sha256':hashlib.sha256(raw).hexdigest(),
                     'archived_sha256':hashlib.sha256((target/name).read_bytes()).hexdigest()})
(target/'EVIDENCE-MANIFEST.json').write_text(json.dumps(manifest,indent=2)+'\n','utf8')
print(f'Archived {len(manifest)} reports; queries and credential fields sanitized')
