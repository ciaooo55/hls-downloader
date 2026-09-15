# -*- coding: utf-8 -*-
"""Run every positive AND negative control for the verification scripts.

The point of this file
----------------------
Seven verification scripts in this project were reporting a confident pass they had not
earned: empty input made the criterion vacuously true ("no row is untrustworthy" over
zero rows), or the verdict was only printed and never reached the exit code. A script
that cannot fail looks exactly like a script that works -- right up until it matters.

So the controls live in one place and run in one command. Every check below asserts a
SPECIFIC exit code, and a wrong code is a control failure, not a warning.

Usage
-----
  C:\\Users\\lee\\.conda\\envs\\test\\python.exe scripts\\run-validator-controls.py

The interpreter matters: ``run_script`` propagates ``sys.executable`` to every child,
and ``compare-layout.py`` imports cv2/PIL/numpy. Run this file with an interpreter that
has them (the conda env above), otherwise the compare-layout control "fails" for a
reason that has nothing to do with the comparator.
"""
import contextlib
import importlib.util
import io
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable
REPORT = r'A:\Ubuntu\测试\hls-downloader\outputs\v7-ui-audit-2026-09-14.html'
CAPTURES = r'A:\Ubuntu\测试\hls-downloader\artifacts\v7-productization\compose-visual-all'
NEG = r'A:\Ubuntu\测试\hls-downloader\artifacts\v7-productization\layout-regression\negative-controls'

results = []


def record(label, expected, actual, extra=''):
    ok = (expected == actual)
    results.append(ok)
    print('  %-58s exit=%-4s expect=%-4s [%s] %s'
          % (label, actual, expected, 'OK' if ok else 'CONTROL FAILED', extra))


def run_script(name, *args):
    proc = subprocess.run([PY, os.path.join(HERE, name)] + list(args),
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
    try:
        with contextlib.redirect_stdout(buf):
            rc = mod.main()
    except SystemExit as exc:
        rc = exc.code
    return rc


class _BrowserStub:
    """Stands in for subprocess.run when driving headless Chromium: an empty dump."""

    stdout = '<!doctype html><html><body><pre id="out"></pre></body></html>'
    stderr = ''
    returncode = 0


def mixcheck_control():
    """mixcheck.py is a module-level script, so it has to be run, not imported."""
    import runpy
    real_run = subprocess.run
    subprocess.run = lambda *a, **k: _BrowserStub()
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
    print('=== subprocess-level controls ===')
    rc, tail = run_script('validate_report.py', REPORT)
    record('validate_report: real report', 0, rc, tail[:40])
    rc, tail = run_script('validate_report.py', os.path.join(NEG, 'empty-report.html'))
    record('validate_report: no tables, no images', 1, rc, tail[:40])

    rc, tail = run_script('validate_captures.py', CAPTURES)
    record('validate_captures: real 24-capture set', 0, rc, tail[:40])
    rc, tail = run_script('validate_captures.py', os.path.join(NEG, 'empty-report'))
    record('validate_captures: empty results', 1, rc, tail[:40])
    rc, tail = run_script('validate_captures.py', os.path.join(NEG, 'count-mismatch'))
    record('validate_captures: claims 24, holds 1', 1, rc, tail[:40])
    rc, tail = run_script('validate_captures.py', os.path.join(NEG, 'dialog-missing'))
    record('validate_captures: dialog never rendered (anchor)', 1, rc, tail[:40])

    rc, tail = run_script('compare-layout.py', '--selftest', CAPTURES)
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
    rc, tail = run_script('mixcheck.py')
    record('mixcheck.py: real 42-row cross-check', 0, rc, tail[:40])

    print()
    passed = sum(1 for r in results if r)
    print('controls: %d/%d passed' % (passed, len(results)))
    if passed != len(results):
        print('A failed control means the tool cannot be trusted, not that the product is bad.')
    return 0 if passed == len(results) else 1


if __name__ == '__main__':
    sys.exit(main())
