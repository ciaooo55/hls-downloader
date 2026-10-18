"""Upgrade the local installation and verify its package and user state."""
import argparse
from contextlib import closing
import ctypes
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import shutil
import subprocess
import uuid
import winreg

import psutil
from smoke_v7_portable_app import Pipe, runtime_processes, wait

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--evidence', type=Path, required=True)
parser.add_argument('--verify-backup', type=Path, help='Verify an already completed installation against this backup; do not run MSI')
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
evidence = args.evidence.resolve()
assert evidence.is_relative_to(root / 'artifacts')
runtime = root / '.tool-cache/runtime/HLSDownloader'
resources = runtime / 'app/resources'
installer = json.loads((evidence / 'installer.json').read_text(encoding='utf-8-sig'))
package = Path(installer['path'])
assert package.is_relative_to(evidence) and package.is_file()
report = {'passed': False, 'product_code': installer['product_code'], 'steps': [],
          'installer_sha256': installer['sha256'], 'attempted_msiexec': False}
before = settings = None
attempt = uuid.uuid4().hex
backup = args.verify_backup.resolve() if args.verify_backup else root / '.tool-cache/backups/local-upgrade' / attempt
assert backup.is_relative_to(root / '.tool-cache/backups')
transaction_result = evidence / ('installer-transaction-' + attempt + '.json')
previous_report = evidence / 'installed-verification.json'
if previous_report.is_file():
    shutil.copy2(previous_report, evidence / ('installed-verification-' + attempt + '-previous.json'))
report['backup'] = str(backup)
if not args.verify_backup:
    report['transaction_result'] = str(transaction_result)
persistent = ['data', 'downloads', 'config.json', 'portable']
report['restored_missing_user_paths'] = []
restart_needed = False
transaction_finished = True


def core(request):
    pipe = Pipe()
    try:
        return pipe.request(request)
    finally:
        pipe.close()


def ready():
    try:
        return core({'type': 'snapshot', 'request_id': 4})
    except (OSError, RuntimeError):
        return None

def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def save():
    target = evidence / 'installed-verification.json'
    temporary = target.with_suffix('.json.tmp')
    temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    temporary.replace(target)

def restore_missing_user_state():
    for name in persistent:
        saved = backup / name
        files = saved.rglob('*') if saved.is_dir() else [saved]
        for path in files:
            if not path.is_file():
                continue
            target = runtime / path.relative_to(backup)
            if not target.exists():
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, target)
                report['restored_missing_user_paths'].append(str(path.relative_to(backup)))

env = {k: v for k, v in os.environ.items() if not k.startswith(('HLS_UI_', 'HLS_V7_PRESENTER_TRACE'))}
try:
    assert sha(package) == installer['sha256']
    if args.verify_backup:
        assert not runtime_processes(runtime), 'Stop the local application before recovering the saved state'
        state = json.loads((backup / 'user-state.json').read_text(encoding='utf8'))
        before, settings, files = state['tasks'], state['settings'], state['files']
        expected = json.loads((evidence / 'BUILD-PROVENANCE.json').read_text(encoding='utf8'))
        restart_needed = True
        report['status'] = 'verifying_completed_installation'
        report['verification_only'] = True
        save()
        restore_missing_user_state()
    else:
        if not runtime_processes(runtime):
            subprocess.Popen([str(resources / 'HLSDownloaderEngine.exe')], cwd=runtime, env=env,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             creationflags=subprocess.CREATE_NO_WINDOW)
        before = wait(ready, 'Core unavailable before upgrade', 60)['tasks']
        settings = core({'type': 'load_settings', 'request_id': 2})
        assert not any(t['status'] in ('downloading', 'recording', 'merging', 'checking') for t in before)
        assert not any(json.loads(h)['status'] == 'pending' for h in core({'type': 'load_handoffs', 'request_id': 3})['items'])
        files = {t['output_path']: sha(Path(t['output_path'])) for t in before
                 if t['status'] == 'completed' and Path(t['output_path']).is_file()}
        backup.mkdir(parents=True, exist_ok=True)
        # sqlite3 的 with 只提交事务，不关闭连接；升级前必须释放数据库文件句柄。
        with closing(sqlite3.connect(f'file:{(runtime / "data/data.db").as_posix()}?mode=ro', uri=True)) as source:
            with closing(sqlite3.connect(backup / 'user-data.db')) as destination:
                source.backup(destination)
        (backup / 'user-state.json').write_text(json.dumps({'tasks': before, 'settings': settings, 'files': files}, ensure_ascii=False), encoding='utf8')
        expected = json.loads((evidence / 'BUILD-PROVENANCE.json').read_text(encoding='utf8'))
        stopped = []
        for item in runtime_processes(runtime):
            try:
                p = psutil.Process(item['pid'])
                p.terminate()
                restart_needed = True
                stopped.append(p)
            except psutil.NoSuchProcess:
                pass
        _, alive = psutil.wait_procs(stopped, timeout=10)
        assert not alive
        for name in persistent:
            path = runtime / name
            if path.is_dir():
                shutil.copytree(path, backup / name, dirs_exist_ok=True)
            elif path.is_file():
                shutil.copy2(path, backup / name)
        report['stopped_only_runtime_processes'] = [p.pid for p in stopped]
        save()
        arguments = ['/i', str(package), '/qn', '/norestart', 'INSTALLDIR=' + str(runtime),
                     'MSIRESTARTMANAGERCONTROL=Disable',
                     '/l*v', str(evidence / ('installer-install-' + attempt + '.log'))]
        report['installer_log'] = arguments[-1]
        report['status'] = 'installing' if ctypes.windll.shell32.IsUserAnAdmin() else 'waiting_for_uac'
        save()
        install_env = dict(env, HLS_ACCEPTANCE_INSTALL_ARGS=subprocess.list2cmdline(arguments),
                           HLS_ACCEPTANCE_INSTALL_RESULT=str(transaction_result))
        process = subprocess.Popen(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command',
            "$ErrorActionPreference='Stop'; $p=$null; $code=1; $detail=$null; try {"
            "$options=@{FilePath='msiexec.exe'; ArgumentList=$env:HLS_ACCEPTANCE_INSTALL_ARGS; "
            "WindowStyle='Hidden'; PassThru=$true; Wait=$true}; "
            "if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent())"
            ".IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {$options.Verb='RunAs'}; "
            "$p=Start-Process @options; $code=$p.ExitCode"
            "} catch { $detail=$_.Exception.Message; $failure=$_.Exception; while ($null -ne $failure) {"
            "if ($failure.NativeErrorCode -eq 1223 -or $failure.HResult -eq -2147023673) {$code=1223}; "
            "$failure=$failure.InnerException } }; "
            "@{exit_code=$code; attempted_msiexec=($null -ne $p); error=$detail} | ConvertTo-Json | "
            "Set-Content -LiteralPath $env:HLS_ACCEPTANCE_INSTALL_RESULT -Encoding UTF8; exit $code"],
            env=install_env, creationflags=subprocess.CREATE_NEW_PROCESS_GROUP)
        transaction_finished = False
        while True:
            try:
                observer_code = process.wait()
                break
            except KeyboardInterrupt:
                # 对话中断不能触发恢复；必须等待安装事务的最终结果。
                report['observer_interrupted_while_installer_running'] = True
                save()
        report['observer_exit_code'] = observer_code
        if not transaction_result.is_file():
            raise RuntimeError('Installer observer ended without a transaction result; keep Core stopped')
        transaction = json.loads(transaction_result.read_text(encoding='utf-8-sig'))
        transaction_finished = True
        code = transaction['exit_code']
        report['status'] = 'installer_finished'
        report['installer_exit_code'] = None if code == 1223 else code
        report['attempted_msiexec'] = transaction['attempted_msiexec']
        if code == 1223:
            report['status'] = 'blocked'
            report['error'] = 'Windows UAC confirmation was canceled; MSI did not run'
            save()
            raise RuntimeError(report['error'])
        save()
        assert code in (0, 3010), transaction
        restore_missing_user_state()
    query = ctypes.WinDLL('msi').MsiQueryProductStateW
    query.argtypes = [ctypes.c_wchar_p]
    query.restype = ctypes.c_int
    report['installed_product_state'] = query(installer['product_code'])
    assert report['installed_product_state'] == 5
    actual = {name: sha(resources / name) for name in expected['components']}
    assert actual == expected['components']
    assert {name: sha(runtime / 'app' / name) for name in expected['desktop_jars']} == expected['desktop_jars']
    for family, hashes in expected['extension_outputs'].items():
        assert {str(p.relative_to(runtime / 'extensions' / family)): sha(p)
                for p in (runtime / 'extensions' / family).rglob('*') if p.is_file()} == hashes
    assert {name: sha(runtime / name) for name in expected['extension_archives']} == expected['extension_archives']
    assert sha(resources / 'BUILD-PROVENANCE.json') == sha(evidence / 'BUILD-PROVENANCE.json')
    report['installed_component_hashes_match'] = True
    for browser, key in [('Firefox', 'Mozilla'), ('Edge', 'Microsoft/Edge'), ('Chrome', 'Google/Chrome')]:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, ('Software/' + key + '/NativeMessagingHosts/com.ciaooo55.hls_downloader').replace('/', '\\')) as handle:
            path = Path(winreg.QueryValueEx(handle, None)[0])
        manifest = json.loads(path.read_text(encoding='utf8'))
        assert path.is_relative_to(resources) and Path(manifest['path']) == resources / 'HLSDownloaderNativeHost.exe'
        report['steps'].append({'browser': browser, 'native_registration': str(path)})
    assert all(sha(Path(path)) == digest for path, digest in files.items())
    report['user_files_byte_preserved'] = True
    report['package_verified'] = True
except Exception as error:
    report['error'] = repr(error)
finally:
    if restart_needed and transaction_finished:
        try:
            # 旧包仍会递归卸载，恢复本次备份后才能打开数据库。
            restore_missing_user_state()
            subprocess.Popen([str(resources / 'HLSDownloaderEngine.exe')], cwd=runtime, env=env,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             creationflags=subprocess.CREATE_NO_WINDOW)
            after = wait(ready, 'Core did not restart after installer', 60)['tasks']
            assert before == after
            assert settings == core({'type': 'load_settings', 'request_id': 2})
            assert all(sha(Path(path)) == digest for path, digest in files.items())
            report['user_tasks_and_settings_preserved'] = True
            report['passed'] = report.get('package_verified', False) and 'error' not in report
            if report['passed']:
                report['status'] = 'passed'
            subprocess.Popen([str(runtime / 'HLSDownloader.exe')], cwd=runtime, env=env,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception as error:
            report['restore_error'] = repr(error)
            report['passed'] = False
    elif not transaction_finished:
        report['status'] = 'transaction_result_missing'
        report['core_kept_stopped'] = True
    save()
print(json.dumps(report, ensure_ascii=True))
raise SystemExit(0 if report['passed'] else 1)
