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
    'render-trace-jdk-SOFTWARE.log','render-trace-SOFTWARE.log',
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
files = [(source/name, Path(name)) for name in selected]
build_source = repo/'.tool-cache/full-audit-20261027'
files += [(build_source/name, Path('build-validation')/name) for name in (
    'desktop-tests.txt', 'desktop-tests-limited.txt', 'desktop-jar.txt',
    'extension-tests.txt', 'extension-tests-limited.txt', 'extension-typecheck.txt',
    'extension-chrome-build.txt', 'extension-firefox-build.txt',
    'final-release-build.txt', 'repack.txt', 'rollback-contract.txt',
)]
final_source = repo/'.tool-cache/full-audit-20261027/final-reports'
files += [(final_source/name, Path('final-no-gpu')/name) for name in (
    'installed-audit.json', 'fresh-installed-audit.json',
    'installed-create-pause.json', 'installed-verify-paused.json',
    'installed-verify-resume.json', 'fresh-installed-audit.runtime.log',
    'fresh-installed-audit.png', 'fresh-installed-audit.paint.png',
)]
render_source = repo/'.tool-cache/full-audit-20261027/render-reports'
files += [(render_source/name, Path('render-followup')/name) for name in (
    'render-installed-audit.json', 'render-installed-audit.runtime.log',
    'render-installed-audit.png', 'render-installed-audit.paint.png',
)]
history_source = repo/'.tool-cache/handoff-history-20261027'
files += [(path, Path('handoff-history')/path.relative_to(history_source))
          for path in sorted((history_source/'reports').glob('*'))
          if path.suffix in ('.json', '.txt')]
files += [(history_source/name, Path('handoff-history')/name) for name in (
    'build-tests.stderr.txt', 'build-tests-fixed.stderr.txt', 'core-build.txt',
    'ui-build.txt', 'ui-build-fixed.txt', 'presenter-build.txt', 'repack.txt',
    'rollback-contract.txt',
)]
recovery_source = repo/'.tool-cache/core-recovery-20261028'
files += [(path, Path('core-recovery')/path.relative_to(recovery_source))
          for path in sorted((recovery_source/'reports').glob('*'))
          if path.suffix in ('.json', '.txt')]
files += [(recovery_source/name, Path('core-recovery')/name) for name in (
    'build-tests.stderr.txt', 'engine-build.txt', 'repack.txt', 'rollback-contract.txt',
    'paged-build-tests.stderr.txt', 'paged-core-build.txt', 'paged-presenter-build.txt',
    'paged-ui-build.txt', 'paged-ui-build-fixed.txt',
    'input/installer.json', 'input/components.json', 'input/BUILD-PROVENANCE.json',
)]
workbench_source = repo/'.tool-cache/workbench-interaction-20261028'
files += [(path, Path('workbench-interaction')/path.relative_to(workbench_source))
          for path in sorted((workbench_source/'reports').glob('*'))
          if path.suffix in ('.json', '.txt', '.png')]
files += [(workbench_source/name, Path('workbench-interaction')/name) for name in (
    'ui-build-tests.txt', 'ui-build-tests-retry.txt', 'ui-focus-build.txt',
    'rollback-contract.txt', 'rollback-contract-focus.txt',
    'parity-contract.txt',
    'input/installer.json', 'input/components.json', 'input/BUILD-PROVENANCE.json',
)]
for path, relative in files:
    if not path.is_file():
        continue
    raw = path.read_bytes()
    if relative.suffix == '.log':
        relative = relative.with_suffix('.txt')
    destination = target/relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    if relative.suffix == '.png':
        destination.write_bytes(raw)
    else:
        try:
            text = raw.decode('utf-8-sig')
        except UnicodeDecodeError:
            # Windows PowerShell 5.1的原生命令重定向可能沿用系统代码页。
            text = raw.decode('gb18030')
        text = text.replace('\r\n', '\n').replace('\r', '')
        text = '\n'.join(line.rstrip() for line in text.split('\n'))
        content = (json.dumps(scrub(json.loads(text)),ensure_ascii=False,indent=2)+'\n'
                   if relative.suffix == '.json' else scrub_text(text))
        content = content.rstrip('\n')+'\n' if content.strip() else ''
        # Match the repository's LF checkout contract so archived hashes also
        # verify after cloning on another machine. Original hashes stay intact.
        destination.write_text(content,encoding='utf8',newline='\n')
    manifest.append({'path':relative.as_posix(),'original_sha256':hashlib.sha256(raw).hexdigest(),
                     'archived_sha256':hashlib.sha256(destination.read_bytes()).hexdigest()})
(target/'EVIDENCE-MANIFEST.json').write_text(json.dumps(manifest,indent=2)+'\n',encoding='utf8',newline='\n')
print(f'Archived {len(manifest)} reports; queries and credential fields sanitized')
