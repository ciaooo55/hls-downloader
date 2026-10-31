# -*- coding: utf-8 -*-
"""Check visual validators against current captures and deliberately invalid inputs.

Usage: python scripts/visual/run-validator-controls.py --captures CAPTURE_DIRECTORY
Requires the existing cv2, PIL and numpy dependencies and a Chromium browser.
"""
import argparse
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable
ROOT = os.path.dirname(os.path.dirname(HERE))

results = []


def record(label, expected, actual, extra=''):
    ok = (expected == actual)
    results.append(ok)
    print('  %-58s exit=%-4s expect=%-4s [%s] %s'
          % (label, actual, expected, 'OK' if ok else 'CONTROL FAILED', extra))


def run_script(name, *args):
    proc = subprocess.run([PY, '-X', 'utf8', os.path.join(HERE, name)] + list(args),
                          capture_output=True, text=True, encoding='utf-8',
                          cwd=HERE, timeout=600)
    tail = [l for l in proc.stdout.splitlines() if l.strip()]
    return proc.returncode, (tail[-1] if tail else '')


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, path))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def call_main(mod):
    buf = io.StringIO()
    previous_argv = sys.argv
    sys.argv = [mod.__file__]
    try:
        with contextlib.redirect_stdout(buf):
            rc = mod.main()
    except SystemExit as exc:
        rc = exc.code
    finally:
        sys.argv = previous_argv
    return rc


class _BrowserStub:
    """Stands in for subprocess.run when driving headless Chromium: an empty dump."""

    stdout = '<!doctype html><html><body><pre id="out"></pre></body></html>'
    stderr = ''
    returncode = 0


def mixcheck_control(mismatch=False):
    """Check missing output and a complete but incorrect browser response."""
    import runpy
    real_run = subprocess.run
    def browser_stub(*args, **kwargs):
        response = _BrowserStub()
        if mismatch:
            import json
            from pathlib import Path
            from urllib.parse import unquote, urlparse
            html = Path(unquote(urlparse(args[0][-1]).path).lstrip('/')).read_text(encoding='utf-8')
            rows = json.loads(html.split('const rows = ', 1)[1].split(';', 1)[0])
            response.stdout = '<pre id="out">' + '<br>'.join(
                f"{row['name']}|{row['theme']}|rgb(0,0,0)|rgb(0,0,0)"
                for row in rows) + '</pre>'
        return response
    subprocess.run = browser_stub
    buf = io.StringIO()
    code = None
    try:
        with contextlib.redirect_stdout(buf):
            runpy.run_path(os.path.join(HERE, 'mixcheck.py'), run_name='__main__')
    except SystemExit as exc:
        code = exc.code
    finally:
        subprocess.run = real_run
    tail = [l for l in buf.getvalue().splitlines() if l.strip()]
    return code, (tail[-1] if tail else '')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--captures', required=True, type=Path,
                        help='Current capture set containing tasks_1000 and settings in dark theme at 1400x820')
    args = parser.parse_args()
    captures = args.captures.resolve()
    data = json.loads((captures / 'capture-report.json').read_text(encoding='utf-8-sig'))
    print('=== subprocess-level controls ===')
    rc, tail = run_script('validate_captures.py', str(captures))
    record('validate_captures: current capture set', 0, rc, tail[:40])
    if rc != 0:
        return 1
    # Derive corrupt reports from validated current inputs, without historical fixtures.
    work = Path(ROOT) / 'artifacts' / 'v7-productization' / 'layout-regression'
    work.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='validator-controls-', dir=work) as temporary:
        directory = Path(temporary)
        item = next(row for row in data['results'] if row['fixture'] == 'tasks_1000')
        cases = [
            ('empty results', {'results': [], 'total': 0, 'captured': 0}),
            ('count mismatch', {'results': [item], 'total': 2, 'captured': 1}),
            ('dialog missing', {'results': [dict(item, fixture='settings')], 'total': 1, 'captured': 1}),
        ]
        for label, report in cases:
            (directory / 'capture-report.json').write_text(json.dumps(report), encoding='utf-8')
            rc, tail = run_script('validate_captures.py', str(directory))
            record('validate_captures: ' + label, 1, rc, tail[:40])
    rc, tail = run_script('compare-layout.py', '--selftest', str(captures))
    record('compare_layout: its own 3 controls', 0, rc, tail[:40])

    print()
    print('=== in-process controls (data source stubbed to empty) ===')
    for path, attr, stub in [('audit_nontext.py', 'rows', lambda: []),
                             ('audit_desktop_nontext.py', 'rows', lambda: [])]:
        mod = load_module('_ctl_' + path.replace('.py', ''), path)
        record('%s: real' % path, 0, call_main(mod))
        setattr(mod, attr, stub)
        record('%s: %s() -> []' % (path, attr), 1, call_main(mod))

    mod = load_module('_ctl_aef', 'audit_ext_fills.py')
    record('audit_ext_fills.py: real', 0, call_main(mod))
    mod.load_rules = lambda: {}
    record('audit_ext_fills.py: load_rules() -> {}', 1, call_main(mod))

    mod = load_module('_ctl_aec', 'audit_ext_contrast.py')
    record('audit_ext_contrast.py: real', 0, call_main(mod))
    mod.css_sources = lambda: []
    record('audit_ext_contrast.py: css_sources() -> []', 1, call_main(mod))

    print()
    print('=== mixcheck: browser stub returns an empty dump ===')
    rc, tail = mixcheck_control()
    record('mixcheck.py: browser returned nothing', 1, rc, tail[:40])
    rc, tail = mixcheck_control(mismatch=True)
    record('mixcheck.py: all browser colours are wrong', 1, rc, tail[:40])
    rc, tail = run_script('mixcheck.py')
    record('mixcheck.py: real 42-row cross-check', 0, rc, tail[:40])

    print()
    print('=== hover probe cleanup preserves existing processes ===')
    from unittest.mock import patch
    probe = load_module('_ctl_hover', 'probe_v7_hover_feedback.py')
    with patch.object(probe, 'product_processes', return_value={10: 'HLSDownloaderEngine.exe', 20: 'HLSDownloader.exe'}):
        with patch.object(probe.subprocess, 'run') as run:
            probe.kill_product_processes({10})
            record('hover cleanup: only newly started PID is stopped',
                   [['taskkill', '/PID', '20', '/F']],
                   [call.args[0] for call in run.call_args_list])

    print()
    passed = sum(1 for r in results if r)
    print('controls: %d/%d passed' % (passed, len(results)))
    if passed != len(results):
        print('A failed control means the tool cannot be trusted, not that the product is bad.')
    return 0 if passed == len(results) else 1


if __name__ == '__main__':
    sys.exit(main())
