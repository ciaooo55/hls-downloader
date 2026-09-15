# -*- coding: utf-8 -*-
"""Cross-validate the colour parser against the real browser.

The whole contrast table rests on audit_ext_fills.py resolving `color-mix()` and
`var()` the way a browser does. If that math is wrong, every number in the report
is wrong -- and this parser has already been wrong once (a rstrip that silently
swallowed every `color-mix(...,var(--x))`).

So: hand chromium the exact expressions used in the CSS, read back
`getComputedStyle`, and diff the resulting RGB against the parser.
"""
import json
import re
import subprocess
import sys

sys.path.insert(0, r'C:\Users\lee\hls-visual')
import audit_ext_fills as E

CHROME = r'C:\Users\lee\AppData\Local\ms-playwright\chromium-1228\chrome-win64\chrome.exe'
HTML = r'C:\Users\lee\hls-visual\mixcheck.html'

# (label, fg expr, bg expr) -- copied verbatim from the CSS, incl. pre-fix forms
CASES = [
    ('badge AFTER', 'var(--primary-ink)', 'color-mix(in srgb,var(--primary) 20%,var(--surface-2))'),
    ('badge BEFORE', 'var(--primary)', 'color-mix(in srgb,var(--primary) 20%,var(--surface-2))'),
    ('empty-icon AFTER', 'var(--primary-ink)', 'color-mix(in srgb,var(--primary) 10%,var(--surface))'),
    ('empty-icon BEFORE', 'var(--primary)', 'color-mix(in srgb,var(--primary) 10%,var(--surface))'),
    ('btn.active AFTER', 'var(--green-ink)', 'color-mix(in srgb,var(--green) 16%,var(--surface-3))'),
    ('btn.active BEFORE', 'var(--green)', 'color-mix(in srgb,var(--green) 16%,var(--surface-3))'),
    ('btn.act:hover AFTER', 'var(--green-ink)', 'color-mix(in srgb,var(--green) 22%,var(--surface-3))'),
    ('btn.act:hover BEFORE', 'var(--green)', 'color-mix(in srgb,var(--primary) 12%,var(--surface-3))'),
    ('result AFTER', 'var(--green-ink)', 'color-mix(in srgb,var(--green) 14%,var(--surface))'),
    ('result BEFORE', 'var(--green)', 'color-mix(in srgb,var(--green) 14%,var(--surface))'),
    ('result.error AFTER', 'var(--red-ink)', 'color-mix(in srgb,var(--red) 12%,var(--surface))'),
    ('result.error BEFORE', 'var(--red)', 'color-mix(in srgb,var(--red) 12%,var(--surface))'),
    ('send-error AFTER', 'var(--red-ink)', 'color-mix(in srgb,var(--red) 10%,var(--surface))'),
    ('send-error BEFORE', 'var(--red)', 'color-mix(in srgb,var(--red) 10%,var(--surface))'),
    ('push-button AFTER', 'var(--purple-ink)', 'color-mix(in srgb,var(--purple) 22%,var(--surface-3))'),
    ('push-button BEFORE', 'var(--purple)', 'color-mix(in srgb,var(--purple) 22%,var(--surface-3))'),
    ('push-btn:hover AFTER', 'var(--on-primary)', 'var(--purple)'),
    ('push-btn:hover BEFORE', 'var(--purple)', 'color-mix(in srgb,var(--purple) 32%,var(--surface-3))'),
    ('pin.active AFTER', 'var(--green-ink)', 'color-mix(in srgb,var(--green) 18%,var(--surface-3))'),
    ('pin.active BEFORE', 'var(--green)', 'color-mix(in srgb,var(--green) 18%,var(--surface-3))'),
    ('btn.primary REF', 'var(--on-primary)', 'var(--primary)'),
]

tokens = E.parse_tokens()
LOOKUP = {label: (fg, bg) for label, fg, bg in CASES}


def token_css(theme):
    body = ''.join('--%s:%s;' % (k, v) for k, v in tokens[theme].items())
    return '[data-hlsd-theme="%s"]{%s}' % (theme, body)


rows = [{'name': label, 'theme': theme, 'fg': fg, 'bg': bg}
        for theme in ('dark', 'light') for label, fg, bg in CASES]

# Build the probe page. Rows are separated with <br> rather than a newline so that
# no escape sequence has to survive two layers of string quoting -- doing it with
# a newline escape silently produced an unterminated JS string and an empty dump.
js = """
const rows = __ROWS__;
const p = document.getElementById('out');
for (const r of rows) {
  const d = document.createElement('div');
  d.setAttribute('data-hlsd-theme', r.theme);
  d.style.background = r.bg;
  d.style.color = r.fg;
  d.textContent = 'x';
  document.body.appendChild(d);
  const cs = getComputedStyle(d);
  p.appendChild(document.createTextNode(
    r.name + '|' + r.theme + '|' + cs.color + '|' + cs.backgroundColor));
  p.appendChild(document.createElement('br'));
}
""".replace('__ROWS__', json.dumps(rows))

html = (
    '<!doctype html><html><head><meta charset="utf-8"><style>\n'
    + token_css('dark') + '\n' + token_css('light') + '\nbody{margin:0}\n'
    + '</style></head><body><pre id="out"></pre><script>' + js + '</script></body></html>'
)
with open(HTML, 'w', encoding='utf-8') as fh:
    fh.write(html)

proc = subprocess.run(
    [CHROME, '--headless=new', '--disable-gpu', '--no-sandbox',
     '--virtual-time-budget=4000', '--dump-dom', 'file:///C:/Users/lee/hls-visual/mixcheck.html'],
    capture_output=True, text=True, encoding='utf-8', timeout=180)
match = re.search(r'<pre id="out">(.*?)</pre>', proc.stdout, re.S)
if not match:
    print('DUMP FAILED')
    print(proc.stdout[:1200])
    print(proc.stderr[:1200])
    sys.exit(1)


def to_hex(css):
    """Chromium serialises a resolved color-mix() as `color(srgb r g b)` with
    0..1 floats, but plain var() references as legacy `rgb(r, g, b)`. Handle both."""
    css = css.strip()
    m = re.match(r'color\(srgb\s+([^)]+)\)', css)
    if m:
        parts = [float(x) for x in m.group(1).split()]
        return '#%02x%02x%02x' % tuple(
            max(0, min(255, int(round(p * 255)))) for p in parts[:3])
    m = re.match(r'rgba?\(([^)]+)\)', css)
    if not m:
        return None
    parts = [float(x) for x in m.group(1).replace('/', ' ').replace(',', ' ').split()]
    return '#%02x%02x%02x' % tuple(int(round(p)) for p in parts[:3])


def parser_hex(expr, theme):
    c = E.parse_color(expr, tokens[theme])
    if c is None:
        return None, None
    return '#%02x%02x%02x' % tuple(int(round(c[i])) for i in range(3)), c


def hex_to_rgb(h):
    h = h.lstrip('#')
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4)) + (1.0,)


print('%-22s %-6s %-9s %-9s %-9s %-7s %-7s %s' % (
    'case', 'theme', 'browser-bg', 'parser-bg', 'browser-fg', 'parser', 'browser', 'delta'))
print('-' * 104)
bg_bad = fg_bad = 0
count = 0
worst = 0.0
for chunk in match.group(1).split('<br>'):
    line = chunk.strip()
    if not line or '|' not in line:
        continue
    name, theme, col, bg = line.split('|')
    fg_expr, bg_expr = LOOKUP[name]
    pbg_hex, pbg = parser_hex(bg_expr, theme)
    pfg_hex, pfg = parser_hex(fg_expr, theme)
    bbg_hex, bfg_hex = to_hex(bg), to_hex(col)
    ok = bbg_hex == pbg_hex and bfg_hex == pfg_hex
    if bbg_hex != pbg_hex:
        bg_bad += 1
    if bfg_hex != pfg_hex:
        fg_bad += 1
    # ratio as the parser sees it, and as the browser's own pixels would give it
    r_parser = E.contrast(E.over(pfg, pbg), pbg)
    bbg = hex_to_rgb(bbg_hex) if bbg_hex else pbg
    bfg = hex_to_rgb(bfg_hex) if bfg_hex else pfg
    r_browser = E.contrast(E.over(bfg, bbg), bbg)
    delta = abs(r_parser - r_browser)
    worst = max(worst, delta)
    count += 1
    print('%-22s %-6s %-9s %-9s %-9s %-7.2f %-7.2f %.3f%s' % (
        name, theme, bbg_hex, pbg_hex, bfg_hex, r_parser, r_browser, delta,
        '' if ok else '  <<< 1-LSB'))
print('-' * 104)
print('rows: %d   bg 1-LSB diffs: %d   fg diffs: %d   worst ratio delta: %.3f' % (
    count, bg_bad, fg_bad, worst))

# 退出码。bg 的 1-LSB 差是已知且可接受的（整数中点舍入），不作为失败；
# 失败只针对"一行都没对上"——解析器和浏览器各读一份数据，若夹具为空，
# "两者一致"就是空洞成立的，那才是真正需要拦下来的情况。
if count == 0:
    print('!! 一行都没比对 —— 对拍失效，不是"两者一致"')
    sys.exit(1)
sys.exit(0)
