# -*- coding: utf-8 -*-
"""Audit the extension's colour pairs by resolving its design tokens.

Extracts every rule that declares both a text colour and a background, resolves
var() / color-mix() against lib/theme.ts, and computes WCAG contrast.
"""
import os
import re
import sys

from audit_ext_fills import EXT, THEME, contrast, over, parse_color, parse_tokens


def css_sources():
    """All CSS text: popup stylesheet, base tokens, injected panel CSS."""
    srcs = []
    p = os.path.join(EXT, "entrypoints", "popup", "style.css")
    srcs.append(("popup/style.css", open(p, encoding="utf-8").read()))
    srcs.append(("lib/theme.ts", open(THEME, encoding="utf-8").read()))
    for name in ("content.ts", "takeover.content.ts", "hooks.content.ts"):
        p = os.path.join(EXT, "entrypoints", name)
        if os.path.exists(p):
            srcs.append((name, open(p, encoding="utf-8").read()))
    return srcs


def rules(text):
    """Yield (selector, declarations) for every rule with a { } body."""
    for m in re.finditer(r"([^{}\n]{1,300})\{([^{}]*)\}", text):
        sel, body = m.group(1).strip(), m.group(2)
        if ":" not in body:
            continue
        yield sel, body


DECL = re.compile(r"(?<![\w-])(color|background|background-color|border-color)\s*:\s*([^;}]+)")


def main():
    tokens = parse_tokens()
    rows = []
    seen = set()
    for fname, text in css_sources():
        for sel, body in rules(text):
            decls = {}
            for prop, val in DECL.findall(body):
                decls.setdefault(prop, val.strip())
            fg_txt = decls.get("color")
            bg_txt = decls.get("background-color") or decls.get("background")
            if not fg_txt or not bg_txt:
                continue
            key = (fname, sel, fg_txt, bg_txt)
            if key in seen:
                continue
            seen.add(key)
            for theme in ("dark", "light"):
                toks = tokens[theme]
                bg = parse_color(bg_txt, toks)
                fg = parse_color(fg_txt, toks)
                if bg is None or fg is None:
                    continue
                if bg[3] < 1.0:
                    # background itself is translucent: composite over the theme bg
                    base = parse_color(toks.get("bg", "#ffffff"), toks)
                    bg = over(bg, base)
                fg_solid = over(fg, bg)
                fs = re.search(r"font(?:-size)?:\s*(?:[^;]*?)([\d.]+)px", body)
                size = float(fs.group(1)) if fs else None
                weight = 700 if re.search(r"font-weight:\s*(700|bold)", body) else None
                rows.append((contrast(fg_solid, bg), theme, fname, sel,
                             fg_txt, bg_txt, size, weight))

    rows.sort()
    print(f"{'ratio':>7}  {'theme':5}  {'size':>5}  selector")
    print("-" * 100)
    under3 = under45 = 0
    for r, theme, fname, sel, fg_txt, bg_txt, size, weight in rows:
        flag = ""
        if r < 3.0:
            flag = "  <<< BELOW 3.0"
            under3 += 1
        elif r < 4.5:
            big = size is not None and (size >= 18.66 or (size >= 14 and weight == 700))
            flag = "  <<< below 4.5 (large text ok)" if big else "  <<< BELOW 4.5"
            under45 += 1
        print(f"{r:7.2f}  {theme:5}  {str(size):>5}  {fname} :: {sel}{flag}")

    # 与 audit_ext_fills 同一套退出码约定：低于 3.0 硬失败，空输入也是失败。
    print("-" * 100)
    print(f"rows: {len(rows)}   below 3.0: {under3}   below 4.5: {under45}")
    if not rows:
        print("!! 一条规则都没解析出来 —— 审计器失效，不是\"零失败\"")
        return 1
    return 1 if under3 else 0


if __name__ == "__main__":
    sys.exit(main())
