"""Run only in Windows Sandbox; validates the native installer shutdown boundary."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import traceback


def main():
    assert os.environ.get('USERNAME') == 'WDAGUtilityAccount', 'Windows Sandbox only'
    parser = argparse.ArgumentParser()
    parser.add_argument('--helper', type=Path, required=True)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--tests', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    args.report.mkdir(parents=True, exist_ok=True)
    work = Path(r'C:\QAPreflight')
    work.mkdir(exist_ok=True)
    root, sibling = work/'installed', work/'installed-other'
    os.environ['HLS_V7_PIPE'] = r'\\.\pipe\HLSInstallerBoundaryFixture'
    result = dict(finished=False, passed=False, steps=[], helper_sha256=hashlib.sha256(args.helper.read_bytes()).hexdigest())
    children = []

    def save():
        (args.report/'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')

    def run(command, name, expected=0):
        result['phase'] = name
        save()
        started = time.monotonic()
        proc = subprocess.run(list(map(str, command)), stdin=subprocess.DEVNULL, capture_output=True, timeout=45, creationflags=subprocess.CREATE_NO_WINDOW)
        (args.report/(name+'.stdout.txt')).write_bytes(proc.stdout)
        (args.report/(name+'.stderr.txt')).write_bytes(proc.stderr)
        result['steps'].append(dict(name=name, exit_code=proc.returncode, expected=expected, seconds=time.monotonic()-started))
        save()
        assert proc.returncode == expected, (name, proc.returncode)

    def spawn(path, *arguments):
        child = subprocess.Popen([str(path), *map(str, arguments)], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)
        children.append(child)
        time.sleep(.3)
        assert child.poll() is None, str(path)
        return child

    try:
        run([args.tests], 'unit-tests')
        for directory in (root, sibling):
            (directory/'app/resources').mkdir(parents=True)
            for relative in ('HLSDownloader.exe', 'app/resources/HLSDownloaderEngine.exe', 'app/resources/HLSDownloaderNativeHost-7.0.3.exe'):
                shutil.copyfile(args.fixture, directory/relative)
        other = spawn(sibling/'HLSDownloader.exe')
        target = spawn(root/'HLSDownloader.exe')
        host = spawn(root/'app/resources/HLSDownloaderNativeHost-7.0.3.exe')
        run([args.helper, root], 'installation-boundary')
        target.wait(timeout=5)
        host.wait(timeout=5)
        assert other.poll() is None, 'helper stopped a different installation'
        run([args.helper, root], 'idle')
        lock = spawn(args.fixture, root/'HLSDownloader.exe')
        run([args.helper, root], 'exclusive-file-lock', 1)
        assert (root/'HLSDownloader.exe').exists() and lock.poll() is None
        lock.terminate()
        lock.wait(timeout=5)
        core = spawn(root/'app/resources/HLSDownloaderEngine.exe', '--pipe-server', os.environ['HLS_V7_PIPE'])
        sentinel = root/'recovery-sentinel.bin'
        sentinel.write_bytes(b'preserve checkpoint and output')
        before = sentinel.read_bytes()
        run([args.helper, root], 'failed-checkpoint', 1)
        assert core.poll() is None, 'helper force-stopped Core after checkpoint failure'
        assert sentinel.read_bytes() == before
        core.terminate()
        core.wait(timeout=5)
        core = spawn(root/'app/resources/HLSDownloaderEngine.exe')
        other_core = spawn(sibling/'app/resources/HLSDownloaderEngine.exe', '--pipe-server', os.environ['HLS_V7_PIPE'])
        run([args.helper, root], 'other-installation-pipe-owner', 1)
        assert core.poll() is None and other_core.poll() is None
        assert sentinel.read_bytes() == before
        core.terminate()
        core.wait(timeout=5)
        other_core.terminate()
        other_core.wait(timeout=5)
        run([args.helper, root], 'retry-after-lock-and-core-release')
        run([args.helper, work], 'invalid-installation', 1)
        assert other.poll() is None
        result['passed'] = True
    except Exception as error:
        result.update(error=repr(error), traceback=traceback.format_exc())
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
        logs = args.report/'diagnostics'
        logs.mkdir(exist_ok=True)
        for folder in (Path(os.environ['TEMP']), Path(r'C:\Windows\Temp')):
            for path in folder.glob('HLSInstaller-*.log'):
                shutil.copyfile(path, logs/(folder.name+'-'+path.name))
        result['finished'] = True
        save()
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
