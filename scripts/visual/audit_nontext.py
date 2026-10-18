# -*- coding: utf-8 -*-
"""WCAG 1.4.11 non-text contrast: control boundaries, focus rings, status dots.

Threshold is 3.0:1, not 4.5:1 -- these carry state/identity, they are not text.

Important nuance baked into the verdict: 1.4.11 only requires 3:1 for visual
information *required* to identify a component. Where the component is already
identifiable by its own text or a high-contrast icon, a low-contrast border is
supplementary emphasis and does NOT constitute a failure. Every border row below
carries an `identifiable_by` note recording what else identifies it.
"""
import sys

import audit_ext_fills as E

THRESHOLD = 3.0

# (label, kind, foreground expr, background expr, identifiable_by)
# `identifiable_by` is None for rows where the graphic IS the only indicator.
CASES = [
    # --- focus rings: the outline is drawn OUTSIDE the element (offset 1-2px),
    # so it sits on the container background, not on the element's own fill.
    ('hlsd-button 焦点环', 'focus', 'var(--primary)', 'var(--surface)', None),
    ('hlsd-button 焦点环', 'focus', 'var(--primary)', 'var(--bg)', None),
    ('hlsd-button 焦点环', 'focus', 'var(--primary)', 'var(--surface-3)', None),
    ('restore-site 焦点环', 'focus', 'var(--primary)', 'var(--surface)', None),
    ('注入面板 焦点环', 'focus', 'var(--primary)', 'var(--surface)', None),

    # --- control boundaries that now use the dedicated --control-border token
    #     (its whole job is to clear 3:1, so these must pass)
    ('hlsd-select 边框/自身', 'border', 'var(--control-border)', 'var(--surface-2)', None),
    ('hlsd-select 边框/相邻', 'border', 'var(--control-border)', 'var(--surface)', None),
    ('hlsd-select 边框/浅底', 'border', 'var(--control-border)', 'var(--surface-3)', None),
    ('quality-trigger 边框/自身', 'border', 'var(--control-border)', 'var(--surface-2)', None),
    ('quality-trigger 边框/相邻', 'border', 'var(--control-border)', 'var(--surface)', None),
    ('empty-retry 边框/自身', 'border', 'var(--control-border)', 'var(--surface)', None),
    ('empty-retry 边框/相邻', 'border', 'var(--control-border)', 'var(--bg)', None),
    ('注入面板 quality-select 边框', 'border', 'var(--control-border)', 'var(--bg)', None),

    # --- remaining separators: deliberately soft, component is identifiable anyway
    ('empty 虚线框/相邻', 'border', 'var(--border)', 'var(--bg)', '空态图标 + 说明文字（容器框为装饰）'),
    ('quality-menu 边框/自身', 'border', 'var(--border)', 'var(--surface)', '菜单项文字 + 浮层阴影'),
    ('quality-menu 边框/相邻', 'border', 'var(--border)', 'var(--bg)', '菜单项文字 + 浮层阴影'),
    ('video-hover 边框/自身', 'border', 'var(--overlay-border)', 'var(--surface)', '浮层内文字'),
    ('video-more 边框/自身', 'border', 'var(--overlay-border)', 'var(--surface)', '「···」字形'),
    ('video-more 边框/相邻', 'border', 'var(--overlay-border)', 'var(--surface-2)', '「···」字形'),
    ('video-download.identifying 边框', 'border', 'var(--overlay-border)',
     'color-mix(in srgb,var(--surface) 88%,var(--primary))', '「正在识别」文字'),
    ('update-notice 边框/自身', 'border',
     'color-mix(in srgb,var(--primary) 42%,var(--border))',
     'color-mix(in srgb,var(--primary) 10%,var(--surface))', '提示文字 + 按钮'),
    ('update-notice 边框/相邻', 'border',
     'color-mix(in srgb,var(--primary) 42%,var(--border))', 'var(--bg)', '提示文字 + 按钮'),
    ('send-error 边框/自身', 'border',
     'color-mix(in srgb,var(--red) 40%,var(--border))',
     'color-mix(in srgb,var(--red) 10%,var(--surface))', '错误文字'),
    ('item 分隔线（参考）', 'border', 'var(--border)', 'var(--surface)', '纯分隔线，不承担识别功能'),

    # --- state indicators: the dot IS the indicator, no text alternative in the dot
    ('status 离线圆点', 'indicator', 'var(--red)', 'var(--surface)', None),
    ('status 在线圆点', 'indicator', 'var(--green)', 'var(--surface)', None),

    # --- icons that carry meaning
    ('empty-retry 图标', 'icon', 'var(--primary)', 'var(--surface)', None),
    ('quality-menu 选中勾', 'icon', 'var(--primary)', 'var(--surface)', None),
    ('section-title 计数', 'icon', 'var(--primary)', 'var(--bg)', None),
]


def rows():
    """[(label, kind, theme, ratio, fg_hex, bg_hex, passes, identifiable_by)]"""
    tokens = E.parse_tokens()
    out = []
    for label, kind, fg_x, bg_x, alt in CASES:
        for theme in ('dark', 'light'):
            toks = tokens[theme]
            bg = E.parse_color(bg_x, toks)
            fg = E.parse_color(fg_x, toks)
            if bg is None or fg is None:
                continue
            if bg[3] < 1.0:
                bg = E.over(bg, E.parse_color(toks['bg'], toks))
            ratio = E.contrast(E.over(fg, bg), bg)
            out.append((label, kind, theme, ratio,
                        '#%02x%02x%02x' % tuple(int(round(fg[i])) for i in range(3)),
                        '#%02x%02x%02x' % tuple(int(round(bg[i])) for i in range(3)),
                        ratio >= THRESHOLD, alt))
    return out


def main():
    data = rows()
    print('%-30s %-10s %-6s %-9s %-9s %s' % ('case', 'kind', 'theme', 'fg', 'bg', 'ratio'))
    print('-' * 88)
    for label, kind, theme, ratio, fg, bg, ok, alt in data:
        print('%-30s %-10s %-6s %-9s %-9s %.2f%s' % (
            label, kind, theme, fg, bg, ratio, '' if ok else '   <<< BELOW 3.0'))
    print('-' * 88)
    hard = [r for r in data if not r[6] and not r[7]]
    soft = [r for r in data if not r[6] and r[7]]
    print('rows: %d   hard failures (no other identifier): %d   '
          'border-only misses (component still identifiable): %d'
          % (len(data), len(hard), len(soft)))
    for r in hard:
        print('  HARD:', r[0], r[2], '%.2f' % r[3])

    # 空输入不是"通过"：解析器在某次重构后一条都匹配不到时，
    # hard 会是空的，于是"硬失败 0"这个结论空洞成立。必须显式拒绝。
    if not data:
        print('!! 一条规则都没解析出来 —— 审计器失效，不是"零失败"')
        return 1
    return 1 if hard else 0


if __name__ == '__main__':
    sys.exit(main())
