# -*- coding: utf-8 -*-
"""Layout regression between a pre-change baseline and the new captures.

Why a pixel diff is not evidence here
-------------------------------------
This change set rewrote 42 hard-coded status colours, 367 typography / radius / shadow
literals and added four ``--*-ink`` tokens. A pixel diff therefore lights up the whole
frame even when the layout is untouched, and it cannot tell

    "the task title got darker"        (deliberate, reviewed)
from
    "the sidebar fell off"             (a regression)

Three colour-invariant metrics were tried and REJECTED because each failed its control:

  * difference connected components -- antialiased recolour edges chain into one sparse
    web spanning the whole dialog, so "largest component" measures edge connectivity,
    not structural damage.
  * Canny edge IoU -- Canny thresholds on gradient magnitude, which is contrast, so a
    recolour moves the edges themselves. Same fixture scored 43% while two *different*
    fixtures scored 54%: no separation.
  * ink-mask projections -- needs a background estimate; the dialog scrim becomes the
    modal colour, so the mask flips wholesale and same-fixture correlation went to -0.99.

What actually works
-------------------
**Compare the coordinates of the panel border lines.** A border is a run of
near-constant colour in a column (or row) that differs from BOTH neighbours; that is a
structural fact about the layout and it survives a recolour as long as any contrast
remains. A layout break adds, removes or moves such lines.

Plus the **bright-panel bounding box** (the dialog sits on a dim scrim, so it is the
bright region), which pins the panel's width and height directly.

Controls are mandatory, and ``--selftest`` runs them:
  1. identical images must match;
  2. two fixtures with different panel structure (``settings`` vs ``tasks_1000``) must
     NOT match;
  3. an image shifted by 3px must NOT match itself -- otherwise the tolerance is so
     loose that a real move would be absorbed.

Usage
-----
  python compare-layout.py <baseline_dir> <new_dir> [--ignore-bottom 8]
  python compare-layout.py --selftest <dir>
"""
import argparse
import os
import sys

import numpy as np
import cv2
from PIL import Image

LINE_TOL = 2          # px: a line counts as the same line within this distance
MIN_UNIFORM = 0.55    # a real border is constant over most of its length
CONTRAST = 8          # min channel delta vs both neighbours
PANEL_LUM = 230       # "bright panel" threshold for the dialog bbox


def load(path, ignore_bottom):
    arr = np.asarray(Image.open(path).convert('RGB'), dtype=np.int32)
    return arr[:-ignore_bottom] if ignore_bottom else arr


def _merge(positions, gap=2):
    groups = []
    for p in positions:
        if groups and p - groups[-1][-1] <= gap:
            groups[-1].append(p)
        else:
            groups.append([p])
    return [int(round(sum(g) / len(g))) for g in groups]


def border_lines(arr, axis):
    """Coordinates of long, uniform, contrasted lines along the given axis."""
    h, w, _ = arr.shape
    n = w if axis == 'v' else h
    found = []
    for i in range(1, n - 1):
        if axis == 'v':
            cur, n1, n2 = arr[:, i, :], arr[:, i - 1, :], arr[:, i + 1, :]
        else:
            cur, n1, n2 = arr[i, :, :], arr[i - 1, :, :], arr[i + 1, :, :]
        key = (cur[:, 0] // 10) * 1000000 + (cur[:, 1] // 10) * 1000 + (cur[:, 2] // 10)
        _, counts = np.unique(key, return_counts=True)
        if counts.max() / len(cur) < MIN_UNIFORM:
            continue
        d1 = np.abs(cur - n1).max(axis=1)
        d2 = np.abs(cur - n2).max(axis=1)
        if ((d1 > CONTRAST) & (d2 > CONTRAST)).mean() < 0.5:
            continue
        found.append(i)
    return _merge(found)


def panel_bbox(arr, thr=PANEL_LUM):
    """Advisory only: bounding box of the largest bright blob (usually the dialog).

    Unreliable by nature -- in a dark theme the dialog is not the bright region, and in
    a fixture with no dialog the whole window is. Use it as a hint, never as a verdict.
    """
    lum = (arr[:, :, 0] * 299 + arr[:, :, 1] * 587 + arr[:, :, 2] * 114) // 1000
    m = (lum > thr).astype(np.uint8)
    m = cv2.morphologyEx(m, cv2.MORPH_CLOSE, np.ones((9, 9), np.uint8))
    n, _, stats, _ = cv2.connectedComponentsWithStats(m, connectivity=8)
    if n <= 1:
        return None
    i = int(np.argmax(stats[1:, cv2.CC_STAT_AREA])) + 1
    s = stats[i]
    return (int(s[0]), int(s[1]), int(s[0] + s[2] - 1), int(s[1] + s[3] - 1))


def only_in(a, b, tol=LINE_TOL):
    return [x for x in a if not any(abs(x - y) <= tol for y in b)]


def compare(base_path, new_path, ignore_bottom):
    a = load(base_path, ignore_bottom)
    b = load(new_path, ignore_bottom)
    if a.shape != b.shape:
        return {'error': 'shape %s vs %s' % (a.shape, b.shape)}
    out = {}
    for axis in ('v', 'h'):
        la, lb = border_lines(a, axis), border_lines(b, axis)
        out['%s_only_base' % axis] = only_in(la, lb)
        out['%s_only_new' % axis] = only_in(lb, la)
        out['%s_count' % axis] = (len(la), len(lb))
    out['panel_base'] = panel_bbox(a)
    out['panel_new'] = panel_bbox(b)
    return out


def describe(r):
    """Verdict from the line sets ONLY.

    ``panel_bbox`` is deliberately NOT part of the verdict. An earlier version used it
    as a pass/fail criterion and it reported "harvest dialog grew 148px" -- which was
    the metric's fault, not the product's: the threshold caught extra bright rows and
    the real growth was 8px. A metric that has been caught lying once does not get to
    decide pass/fail; it is printed as an advisory column instead.
    """
    if 'error' in r:
        return 'FAIL', r['error']
    problems = []
    if r['v_only_base'] or r['v_only_new']:
        problems.append('vertical lines: -%s +%s' % (r['v_only_base'][:5], r['v_only_new'][:5]))
    if r['h_only_base'] or r['h_only_new']:
        problems.append('horizontal lines: -%s +%s' % (r['h_only_base'][:5], r['h_only_new'][:5]))
    return ('DIFF', '; '.join(problems)) if problems else ('SAME', 'border structure unchanged')


def panel_note(r):
    pb, pn = r.get('panel_base'), r.get('panel_new')
    if pb is None and pn is None:
        return '(no panel)'
    if pb is None or pn is None:
        return '?'
    dw, dh = (pn[2] - pn[0]) - (pb[2] - pb[0]), (pn[3] - pn[1]) - (pb[3] - pb[1])
    return '%dx%d -> %dx%d (%+dx%+d, advisory)' % (
        pb[2] - pb[0] + 1, pb[3] - pb[1] + 1, pn[2] - pn[0] + 1, pn[3] - pn[1] + 1, dw, dh)


def selftest(d):
    """Controls. A comparator that cannot fail is not a comparator."""
    ok = True
    print('=== controls ===')

    def check(label, path_a, path_b, expect_same):
        nonlocal ok
        r = compare(path_a, path_b, 0)
        v, why = describe(r)
        got_same = (v == 'SAME')
        good = (got_same == expect_same)
        ok &= good
        print('  %-42s -> %-5s  %s   [%s]'
              % (label, v, why[:58], 'OK' if good else 'CONTROL FAILED'))

    p = os.path.join(d, 'tasks_1000-dark-1400x820.png')
    q = os.path.join(d, 'settings-dark-1400x820.png')
    check('identical images', p, p, True)
    check('different panel structure (tasks_1000 vs settings)', p, q, False)

    # 3px shift: the tolerance must not be loose enough to swallow a real move.
    shifted = os.path.join(d, '_selftest_shift3.png')
    arr = np.asarray(Image.open(p).convert('RGB'), dtype=np.uint8)
    shifted_arr = np.roll(arr, 3, axis=1)
    Image.fromarray(shifted_arr).save(shifted)
    try:
        check('same image shifted 3px', p, shifted, False)
    finally:
        if os.path.exists(shifted):
            os.remove(shifted)

    print('controls: %s' % ('PASS' if ok else 'FAILED'))
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('baseline_dir', nargs='?')
    ap.add_argument('new_dir', nargs='?')
    ap.add_argument('--ignore-bottom', type=int, default=8)
    ap.add_argument('--selftest', metavar='DIR')
    args = ap.parse_args()

    if args.selftest:
        return selftest(args.selftest)
    if not (args.baseline_dir and args.new_dir):
        ap.error('need <baseline_dir> <new_dir>, or --selftest <dir>')

    names = sorted(f for f in os.listdir(args.baseline_dir) if f.endswith('.png'))
    if not names:
        print('no PNGs in %s' % args.baseline_dir)
        return 1

    print('%-40s %-5s %-46s %s' % ('capture', 'v', 'verdict', 'panel (advisory)'))
    print('-' * 132)
    diffs, missing = 0, 0
    for name in names:
        new_path = os.path.join(args.new_dir, name)
        if not os.path.exists(new_path):
            print('%-40s %-14s %s' % (name, '-', 'NOT IN NEW SET'))
            missing += 1
            continue
        r = compare(os.path.join(args.baseline_dir, name), new_path, args.ignore_bottom)
        v, why = describe(r)
        if v == 'DIFF':
            diffs += 1
        print('%-40s %-5s %-46s %s' % (name, v, why, panel_note(r)))
    print('-' * 132)
    print('captures %d | DIFF %d | missing %d' % (len(names), diffs, missing))
    print('DIFF is not automatically a regression -- every entry needs an explanation,')
    print('and the panel-size delta usually gives it away.')
    return 1 if missing else 0


if __name__ == '__main__':
    sys.exit(main())
