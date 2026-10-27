"""Read-only inventory of the actual public commands and UI entry identifiers."""
import hashlib
import json
from pathlib import Path
import re
import subprocess

root = Path(__file__).resolve().parents[4]
contract = root / 'native_shell/src/contract.rs'
text = contract.read_text('utf8')
body = text.split('pub enum CoreCommand {', 1)[1].split('\n}\n', 1)[0]
commands = re.findall(r'^    ([A-Z]\w*)(?:\s*\{|,)', body, re.M)
sources = [root / name for name in (
    'native_shell/src/core_server.rs', 'native_shell/src/download_worker.rs',
    'native_shell/src/core_runtime.rs', 'desktop_ui/src/main/kotlin/com/hlsdownloader/desktop/Protocol.kt')]
references = {name: [] for name in commands}
for source in sources:
    for number, line in enumerate(source.read_text('utf8').splitlines(), 1):
        for name in commands:
            if 'CoreCommand::' + name in line:
                references[name].append(f'{source.relative_to(root).as_posix()}:{number}')
controls = []
for source in sorted((root / 'desktop_ui/src/main/kotlin/com/hlsdownloader/desktop').glob('*.kt')):
    for number, line in enumerate(source.read_text('utf8').splitlines(), 1):
        for identifier in re.findall(r'reportControlBounds\("([^"\n]+)"\)', line):
            controls.append({'id_expression': identifier, 'source':f'{source.relative_to(root).as_posix()}:{number}'})
settings = (root / 'desktop_ui/src/main/kotlin/com/hlsdownloader/desktop/SettingsV7.kt').read_text('utf8')
report = {
    'base_commit': subprocess.check_output(['git','rev-parse','HEAD'], cwd=root, text=True).strip(),
    'scope':'Static entrypoint inventory; references are navigation aids, not proof of behavior or test coverage',
    'core_commands':[{'name':name,'implementation_references':references[name]} for name in commands],
    'desktop_control_identifiers':controls,
    'settings_tabs':re.findall(r'SettingsTab\("([^"\n]+)"', settings),
    'source_sha256':{p.relative_to(root).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in [contract,*sources]},
}
destination = root / 'docs/validation/full-audit-20261027/inventory.json'
destination.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n','utf8')
print(json.dumps({'commands':len(commands),'desktop_controls':len(controls),'settings_tabs':report['settings_tabs']},ensure_ascii=False))
