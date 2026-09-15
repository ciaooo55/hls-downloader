# -*- coding: utf-8 -*-
"""WCAG 1.4.11 non-text contrast for the Compose Desktop workbench.

Why a separate script from audit_nontext.py: that one resolves `var(--*)` from the
extension's CSS token file. The desktop palette lives in Kotlin (`Main.kt`,
`WorkbenchPalette`), so the two share the threshold and the verdict logic but not
the resolver.

The palette is parsed out of the Kotlin source rather than copied by hand. Hand-copied
values are exactly how a "0 failures" audit goes stale after someone edits a colour.

Verdict rule (same as audit_nontext.py): 1.4.11 only requires 3:1 for visual
information *required* to identify a component. Where the component is already
identified by its own text or a high-contrast icon, a low-contrast border/background
is supplementary emphasis and does NOT constitute a failure. Every row carries an
`identifiable_by` note; rows with no other identifier are hard requirements.
"""
import os
import re
import sys

MAIN_KT = os.path.join(
    r'A:\Ubuntu\测试\hls-downloader', 'desktop_ui', 'src', 'main', 'kotlin',
    'com', 'hlsdownloader', 'desktop', 'Main.kt')

TEXT_THRESHOLD = 4.5
GRAPHIC_THRESHOLD = 3.0


# --- colour maths ------------------------------------------------------------

def _lin(channel):
    c = channel / 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def luminance(rgb):
    r, g, b = rgb
    return 0.2126 * _lin(r) + 0.7152 * _lin(g) + 0.0722 * _lin(b)


def contrast(a, b):
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def hex_of(rgb):
    return '#%02x%02x%02x' % tuple(int(round(c)) for c in rgb)


# --- palette parsing ---------------------------------------------------------

NAMED = {'White': (255, 255, 255), 'Black': (0, 0, 0)}


def _parse_color_literal(text):
    """`Color(0xFF2563EB)` / `Color.White` / `Color(0xFF151719)` -> (r, g, b)."""
    m = re.match(r'Color\(0x([0-9A-Fa-f]{6,8})\)$', text)
    if m:
        digits = m.group(1)
        if len(digits) == 8:          # AARRGGBB -- the leading AA is FF here
            digits = digits[2:]
        return (int(digits[0:2], 16), int(digits[2:4], 16), int(digits[4:6], 16))
    m = re.match(r'Color\.(\w+)$', text)
    if m and m.group(1) in NAMED:
        return NAMED[m.group(1)]
    return None


def parse_palettes(path=MAIN_KT):
    """{'light': {...}, 'dark': {...}} straight out of the Kotlin source."""
    src = open(path, encoding='utf-8').read()
    out = {}
    for theme in ('light', 'dark'):
        m = re.search(r'private val %sPalette = WorkbenchPalette\((.*?)\n\)' % theme,
                      src, re.S)
        if not m:
            raise SystemExit('could not find the %s palette in %s' % (theme, path))
        body = m.group(1)
        tokens = {}
        for name, value in re.findall(r'(\w+)\s*=\s*(Color\(0x[0-9A-Fa-f]{6,8}\)|Color\.\w+)', body):
            rgb = _parse_color_literal(value)
            if rgb:
                tokens[name] = rgb
        out[theme] = tokens
    return out


# --- 派生色：悬停/按压态用的不是调色板里的字面值 -----------------------------
#
# 悬停底色是 `surface3.blendToward(ink, .08f)` 这类**算出来**的颜色，按压是
# `.blendToward(ink, .05f)`，输入框悬停边框是 `blue.copy(alpha = .75f)`。
# 手抄这些结果等于让审计在有人改了混合比例之后静默过期，所以这里照样解析表达式。
#
#   blend(<token>, <token|White|Black>, <amount>)   线性混合
#   alpha(<token>, <amount>)                        与所在行的背景色做 alpha 合成
#
# 表达式里的 token 名写错会抛 KeyError，调用方把它当成"跳过并告警"，
# 而不是悄悄算出一个别的颜色。

_BLEND = re.compile(r'blend\(\s*(\w+)\s*,\s*(\w+)\s*,\s*([0-9.]+)\s*\)$')
_ALPHA = re.compile(r'alpha\(\s*(\w+)\s*,\s*([0-9.]+)\s*\)$')


def resolve(expr, toks, over=None):
    expr = expr.strip()
    m = _BLEND.match(expr)
    if m:
        base = toks[m.group(1)]
        target = toks.get(m.group(2)) or NAMED[m.group(2)]
        amount = float(m.group(3))
        return tuple(base[i] + (target[i] - base[i]) * amount for i in range(3))
    m = _ALPHA.match(expr)
    if m:
        colour = toks[m.group(1)]
        amount = float(m.group(2))
        if over is None:
            raise KeyError('alpha(%s) 需要所在行的背景色，调用方没给' % expr)
        return tuple(colour[i] * amount + over[i] * (1 - amount) for i in range(3))
    if expr in toks:
        return toks[expr]
    if expr in NAMED:
        return NAMED[expr]
    raise KeyError(expr)


# --- cases -------------------------------------------------------------------

# (label, kind, fg token, bg token, threshold, identifiable_by)
# `identifiable_by` is None when the graphic is the only thing identifying it.
CASES = [
    # --- collapsed sidebar rail (SidebarRail / RailItem) --------------------
    # The rail icon is the ONLY identifier of a nav item when collapsed: the text
    # label is gone and the tooltip needs a hover. So these two are hard 3:1.
    ('折叠栏 图标（未选中）', 'icon', 'muted', 'rail', GRAPHIC_THRESHOLD, None),
    ('折叠栏 图标（选中）', 'icon', 'blue', 'selected', GRAPHIC_THRESHOLD, None),
    # Selection is carried by three cues (background + icon tint + 3dp left bar),
    # so no single one of them is "required".
    ('折叠栏 选中底色', 'indicator', 'selected', 'rail', GRAPHIC_THRESHOLD,
     '选中项图标色（blue/muted 反转）+ 左侧 3dp 色条'),
    ('折叠栏 选中色条', 'indicator', 'blue', 'rail', GRAPHIC_THRESHOLD,
     '选中项图标色 + 底色'),
    # Badge: the number itself is the identifier, at 6.44/8.01:1.
    ('折叠栏 角标文字（未选中）', 'text', 'muted', 'surface3', TEXT_THRESHOLD, None),
    ('折叠栏 角标文字（选中）', 'text', 'onBlue', 'blue', TEXT_THRESHOLD, None),
    # The 1.5dp stroke exists to separate the badge from the icon strokes behind it.
    ('折叠栏 角标描边（未选中）', 'border', 'rail', 'surface3', GRAPHIC_THRESHOLD,
     '角标文字（muted on surface3 = 6.44/8.01:1）'),
    ('折叠栏 角标描边（选中）', 'border', 'selected', 'blue', GRAPHIC_THRESHOLD,
     '角标文字（onBlue on blue = 5.17/6.79:1）'),
    # --- expanded sidebar, for comparison (the rail must not be worse) ------
    ('展开侧栏 未选中图标/文字', 'icon', 'muted', 'rail', GRAPHIC_THRESHOLD, None),
    ('展开侧栏 选中项图标', 'icon', 'blue', 'selected', GRAPHIC_THRESHOLD, None),
    ('展开侧栏 选中底色', 'indicator', 'selected', 'rail', GRAPHIC_THRESHOLD,
     '选中项图标色 + 文字加粗'),
    # --- rail chrome --------------------------------------------------------
    ('折叠栏 分组分隔线', 'border', 'border', 'rail', GRAPHIC_THRESHOLD,
     '纯分隔线，不承担识别功能'),
    ('折叠栏 底色 vs 画布', 'border', 'rail', 'canvas', GRAPHIC_THRESHOLD,
     '侧栏与内容区的分界由布局留白承担，非识别手段'),

    # --- 悬停 / 按压反馈（本轮新增） -----------------------------------------
    #
    # 悬停态没有常驻夹具（夹具是静态的），所以这一组只能解析计算。
    # 「悬停确实渲染出来了、且落在被悬停项上」由 probe_v7_hover_feedback.py 用实拍图证明。
    # 新增 UI 引入新配对后必须补审：报告里旧的"硬失败 0"是旧结论，不覆盖这些新配对。
    ('侧栏悬停 文字/图标', 'text', 'ink', 'surface3', TEXT_THRESHOLD, None),
    ('侧栏悬停 底色', 'indicator', 'surface3', 'rail', GRAPHIC_THRESHOLD,
     '悬停同时把图标/文字从 muted 提到 ink'),
    ('行悬停 文字（未选中）', 'text', 'muted', 'surface3', TEXT_THRESHOLD, None),
    ('菜单项 文字（悬停）', 'text', 'ink', 'surface2', TEXT_THRESHOLD, None),
    ('菜单项 文字（按下）', 'text', 'ink', 'surface3', TEXT_THRESHOLD, None),
    ('菜单项 悬停底色', 'indicator', 'surface2', 'dialog', GRAPHIC_THRESHOLD,
     '菜单项自带文字，底色只是状态提示'),
    ('表头 文字（悬停）', 'text', 'muted', 'surface3', TEXT_THRESHOLD, None),
    # 输入框悬停边框 = blue.copy(alpha = .75f) 压在自身 rail 底上。
    ('输入框 边框（悬停）', 'border', 'alpha(blue, 0.75)', 'rail', GRAPHIC_THRESHOLD,
     '输入框自带文字与占位符'),
    # 复选框：对勾是"已选中"的图形，没有别的识别手段 ⇒ 硬 3:1。
    ('复选框 对勾（选中）', 'icon', 'onBlue', 'blue', GRAPHIC_THRESHOLD, None),
    ('复选框 对勾（选中·悬停）', 'icon', 'onBlue', 'blend(blue, White, 0.08)', GRAPHIC_THRESHOLD, None),
    ('复选框 边框（未选中）', 'border', 'border', 'rail', GRAPHIC_THRESHOLD,
     '方框形状 + 旁边的文字标签'),
    ('复选框 边框（未选中·悬停）', 'border', 'border', 'surface3', GRAPHIC_THRESHOLD,
     '方框形状 + 旁边的文字标签'),
    # 开关：拇指亮度与轨道相反，四个组合都要过 3:1。
    ('开关 拇指（选中）', 'icon', 'onBlue', 'blue', GRAPHIC_THRESHOLD, None),
    ('开关 拇指（选中·悬停）', 'icon', 'onBlue', 'blend(blue, White, 0.08)', GRAPHIC_THRESHOLD, None),
    ('开关 拇指（未选中）', 'icon', 'faint', 'surface3', GRAPHIC_THRESHOLD, None),
    ('开关 拇指（未选中·悬停）', 'icon', 'faint', 'blend(surface3, ink, 0.08)', GRAPHIC_THRESHOLD, None),
    ('开关 轨道（选中）', 'indicator', 'blue', 'rail', GRAPHIC_THRESHOLD,
     '拇指位置（左/右）与拇指色共同表达状态'),
    # 强调色文字压"自己颜色的淡底"。这一组此前从未进过清单 ——
    # 折叠栏引入 muted on rail / blue on selected 之后报告里的"硬失败 0"是旧结论。
    ('强调色文字 压选中底', 'text', 'blue', 'selected', TEXT_THRESHOLD, None),
    ('强调色图标 压选中底', 'icon', 'blue', 'selected', GRAPHIC_THRESHOLD, None),
    ('强调色文字 压画布', 'text', 'blue', 'canvas', TEXT_THRESHOLD, None),
]


def rows():
    """[(label, kind, theme, ratio, fg_hex, bg_hex, passes, threshold, identifiable_by)]"""
    palettes = parse_palettes()
    out = []
    for label, kind, fg_expr, bg_expr, need, alt in CASES:
        for theme in ('light', 'dark'):
            toks = palettes[theme]
            try:
                bg = resolve(bg_expr, toks)
                fg = resolve(fg_expr, toks, over=bg)
            except KeyError as exc:
                print('  ! skipped %s / %s: 解析不了 %s' % (label, theme, exc))
                continue
            ratio = contrast(fg, bg)
            out.append((label, kind, theme, ratio, hex_of(fg), hex_of(bg),
                        ratio >= need, need, alt))
    return out


def main():
    data = rows()
    print('%-26s %-9s %-6s %-9s %-9s %-7s %s'
          % ('case', 'kind', 'theme', 'fg', 'bg', 'ratio', 'need'))
    print('-' * 92)
    for label, kind, theme, ratio, fg, bg, ok, need, alt in data:
        print('%-26s %-9s %-6s %-9s %-9s %5.2f %s'
              % (label, kind, theme, fg, bg, ratio, need))
    print('-' * 92)
    hard = [r for r in data if not r[6] and not r[8]]
    soft = [r for r in data if not r[6] and r[8]]
    print('rows: %d   hard failures (no other identifier): %d   '
          'supplementary-only misses: %d' % (len(data), len(hard), len(soft)))
    for r in hard:
        print('  HARD:', r[0], r[2], '%.2f (need %.1f)' % (r[3], r[7]))
    for r in soft:
        print('  soft:', r[0], r[2], '%.2f  <- %s' % (r[3], r[8]))

    # 同 audit_nontext：空输入不是通过。解析器失效（源文件改名、正则不匹配）
    # 时 hard 为空 ⇒ "硬失败 0" 空洞成立。
    if not data:
        print('!! 一条规则都没解析出来 —— 审计器失效，不是"零失败"')
        return 1
    return 0 if not hard else 1


if __name__ == '__main__':
    sys.exit(main())
