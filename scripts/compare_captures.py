# -*- coding: utf-8 -*-
"""Validate a fresh full-fixture capture set, and diff it against an older baseline.

Three jobs, and the failure mode they share:

1. **Validate the new captures** -- size, fixture origin, black-pixel fraction, bytes.
   A capture where the window sat partly off-screen is still a legal PNG of the right
   size and looks normal at a glance; only the black fraction catches it.

2. **Diff against the baseline** and CLASSIFY the difference instead of just reporting
   "N pixels differ". A raw pixel count is useless: the whole point of this run is that
   a broad change to a shared layout composable (Main.kt) must not move any fixture
   except the ones we deliberately changed.

   Known-benign difference: the baseline was captured with the window at (48,48), so a
   820px-tall window ran 4px past the 864px screen and Robot painted the bottom 4 rows
   pure black. The new captures lock the fixture origin to (0,0), so those rows now
   contain real content. That difference is EXPECTED and is an improvement; any
   difference outside the bottom rows is a real regression until explained.

3. **Refuse to report a pass it cannot justify.** This is the important one. An earlier
   version of this script printed "validated 25, invalid 0" and exited 0 while:
     * a stray PNG from a failed run (a comma-joined argument list became one bogus
       fixture name) sat in the directory. It had no row in capture-report.json, so the
       origin check was SILENTLY SKIPPED -- and the file still got "OK".
     * a missing baseline file was SILENTLY SKIPPED, so pointing --baseline at the wrong
       directory (or one holding 2 of 24 files) produced
       "compared 2 against baseline; unexplained differences: 0" and exit 0.
   Both are confident false passes. Now:
     * every PNG must have a report row (else the origin is unverifiable -> hard fail);
     * every report row must have a PNG (else a capture is missing -> hard fail);
     * every capture must be compared against the baseline (else -> hard fail, unless
       --allow-partial-baseline is passed, and even then the coverage is printed loudly).

   A validator that can say "I could not check this" is worth more than one that
   reports a number it did not earn.

Usage:
  python compare_captures.py <new_dir> [--baseline <old_dir>] [--width 1400] [--height 820]
                                    [--allow-partial-baseline]
"""
import argparse
import json
import os
import sys

from PIL import Image, ImageChops

BLACK_FRACTION_LIMIT = 0.03


def black_fraction(im):
    grey = im.convert('L')
    return grey.histogram()[0] / float(im.size[0] * im.size[1])


def load_report(new_dir):
    """-> (dict key->row, list of problems). Missing/empty report is a hard failure:
    without it the fixture origin cannot be checked at all."""
    path = os.path.join(new_dir, 'capture-report.json')
    if not os.path.exists(path):
        return {}, ['capture-report.json not found -- fixture origin CANNOT be verified']
    try:
        data = json.load(open(path, encoding='utf-8'))
    except ValueError as exc:
        return {}, ['capture-report.json is not valid JSON: %s' % exc]
    rows = data if isinstance(data, list) else data.get('results', [])
    if not rows:
        return {}, ['capture-report.json holds 0 rows -- fixture origin CANNOT be verified']
    out = {}
    for r in rows:
        key = '%s-%s-%dx%d' % (r.get('fixture'), r.get('theme'), r.get('width'), r.get('height'))
        out[key] = r
    return out, []


def validate_one(path, want_w, want_h, report_row):
    """-> (problems, black_frac). report_row=None is itself a problem, not a skip."""
    problems = []
    size = os.path.getsize(path)
    if size < 4096:
        problems.append('implausibly small (%d bytes)' % size)
    with Image.open(path) as im:
        w, h = im.size
        if (w, h) != (want_w, want_h):
            problems.append('size %dx%d != requested %dx%d' % (w, h, want_w, want_h))
        frac = black_fraction(im.convert('RGB'))
        if frac >= BLACK_FRACTION_LIMIT:
            problems.append('black fraction %.2f%% >= %.1f%%' % (frac * 100, BLACK_FRACTION_LIMIT * 100))
    if report_row is None:
        problems.append('NO REPORT ROW -- fixture origin unverifiable, treat as bogus capture')
    else:
        detail = report_row.get('detail') or '{}'
        try:
            coords = json.loads(detail)
        except ValueError:
            coords = {}
        if not coords:
            problems.append('report row has unparseable detail -- fixture origin unverifiable')
        elif (coords.get('x'), coords.get('y')) != (0, 0):
            problems.append('window origin (%s,%s) != (0,0)' % (coords.get('x'), coords.get('y')))
    return problems, frac


def diff_against(new_path, old_path):
    """-> (differing_pixels, bbox, rows_touched) or a string describing why it cannot be compared."""
    if not os.path.exists(old_path):
        return 'NO BASELINE FILE'
    with Image.open(new_path) as a, Image.open(old_path) as b:
        a = a.convert('RGB')
        b = b.convert('RGB')
        if a.size != b.size:
            return 'SIZE MISMATCH %s vs %s' % (a.size, b.size)
        delta = ImageChops.difference(a, b)
        bbox = delta.getbbox()
        if bbox is None:
            return (0, None, [])
        px = delta.load()
        w, h = delta.size
        count = 0
        rows = set()
        for y in range(h):
            for x in range(w):
                if px[x, y] != (0, 0, 0):
                    count += 1
                    rows.add(y)
        return (count, bbox, sorted(rows))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('new_dir')
    ap.add_argument('--baseline', default='')
    ap.add_argument('--width', type=int, default=1400)
    ap.add_argument('--height', type=int, default=820)
    ap.add_argument('--allow-partial-baseline', action='store_true',
                    help='downgrade "capture not compared against baseline" from failure to warning')
    args = ap.parse_args()

    report, report_problems = load_report(args.new_dir)
    pngs = sorted(f for f in os.listdir(args.new_dir) if f.endswith('.png'))
    png_keys = {f[:-4] for f in pngs}

    print('new captures: %d  (dir %s)' % (len(pngs), args.new_dir))
    for p in report_problems:
        print('!! %s' % p)
    print()

    # --- set reconciliation: this is what catches the stray-comma file -----------------
    orphans = sorted(png_keys - set(report))
    missing = sorted(set(report) - png_keys)
    reconcile_fail = 0
    if orphans:
        reconcile_fail = len(orphans)
        print('!! %d PNG(s) have NO row in capture-report.json -- bogus/leftover captures:' % len(orphans))
        for k in orphans:
            print('     %s' % k)
    if missing:
        reconcile_fail += len(missing)
        print('!! %d capture(s) were requested but produced no PNG:' % len(missing))
        for k in missing:
            print('     %s' % k)
    if orphans or missing:
        print()

    invalid = 0
    print('%-40s %-9s %-8s %s' % ('capture', 'black%', 'bytes', 'verdict'))
    print('-' * 100)
    for name in pngs:
        path = os.path.join(args.new_dir, name)
        key = name[:-4]
        problems, frac = validate_one(path, args.width, args.height, report.get(key))
        if problems:
            invalid += 1
        print('%-40s %7.2f%%  %7d  %s' % (
            name, frac * 100, os.path.getsize(path),
            'OK' if not problems else '; '.join(problems)))
    print('-' * 100)
    print('validated %d, invalid %d' % (len(pngs), invalid))
    print()

    if not args.baseline:
        return 0 if (invalid == 0 and reconcile_fail == 0 and not report_problems) else 1

    print('=== diff vs baseline %s ===' % args.baseline)
    print()
    print('%-40s %-12s %-24s %s' % ('capture', 'differing px', 'bbox', 'verdict'))
    print('-' * 110)
    unexplained = 0
    compared = 0
    not_compared = []
    for name in pngs:
        old = os.path.join(args.baseline, name)
        res = diff_against(os.path.join(args.new_dir, name), old)
        if isinstance(res, str):
            not_compared.append((name, res))
            print('%-40s %-12s %-24s %s' % (name, '-', '-', res))
            continue
        compared += 1
        count, bbox, rows = res
        if count == 0:
            print('%-40s %-12s %-24s %s' % (name, 0, '-', 'IDENTICAL'))
            continue
        bottom_only = all(y >= args.height - 8 for y in rows)
        verdict = ('expected: fixture-origin fix (bottom rows were black, now content)'
                   if bottom_only else 'UNEXPLAINED -- inspect')
        if not bottom_only:
            unexplained += 1
        print('%-40s %-12d %-24s %s' % (name, count, str(bbox), verdict))
    print('-' * 110)

    coverage_ok = not not_compared or args.allow_partial_baseline
    print('compared %d / %d against baseline; unexplained differences: %d'
          % (compared, len(pngs), unexplained))
    if not_compared:
        print('!! %d capture(s) NOT compared -- "0 unexplained differences" over a partial '
              'baseline proves nothing:' % len(not_compared))
        for name, why in not_compared:
            print('     %s: %s' % (name, why))
        if args.allow_partial_baseline:
            print('   (--allow-partial-baseline: downgraded to warning)')
    if reconcile_fail:
        print('!! %d capture-set reconciliation failure(s) -- see above' % reconcile_fail)
    if report_problems:
        print('!! capture-report.json unusable -- fixture origin was never verified')

    ok = (invalid == 0 and unexplained == 0 and reconcile_fail == 0
          and not report_problems and coverage_ok)
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
