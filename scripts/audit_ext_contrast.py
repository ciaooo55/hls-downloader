# -*- coding: utf-8 -*-
"""Audit the extension's colour pairs by resolving its design tokens.

Extracts every rule that declares both a text colour and a background, resolves
var() / color-mix() against lib/theme.ts, and computes WCAG contrast.
"""
import os
import re
import sys

EXT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "extension")
THEME = os.path.join(EXT, "lib", "theme.ts")


def parse_tokens():
    src = open(THEME, encoding="utf-8").read()
    out = {}
    for theme in ("dark", "light"):
        m = re.search(r'\[data-hlsd-theme="%s"\]\{(.*?)\}' % theme, src, re.S)
        block = m.group(1)
        toks = {}
        for name, value in re.findall(r"--([\w-]+):\s*([^;]+);", block):
            toks[name] = value.strip()
        out[theme] = toks
    return out


HEX = re.compile(r"^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$")
RGBA = re.compile(r"^rgba?\(([^)]+)\)$")


def parse_color(text, toks, depth=0):
    """Return (r, g, b, a) or None."""
    if depth > 6:
        return None
    text = text.strip()
    m = re.match(r"^var\(--([\w-]+)\)$", text)
    if m:
        name = m.group(1)
        if name not in toks:
            return None
        return parse_color(toks[name], toks, depth + 1)
    m = HEX.match(text)
    if m:
        h = m.group(1)
        if len(h) == 3:
            h = "".join(c * 2 for c in h)
        if len(h) == 8:
            return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16),
                    int(h[6:8], 16) / 255)
        return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16), 1.0)
    m = RGBA.match(text)
    if m:
        parts = [p.strip() for p in m.group(1).split(",")]
        r, g, b = (float(p) for p in parts[:3])
        a = float(parts[3]) if len(parts) > 3 else 1.0
        return (r, g, b, a)
    if text.startswith("color-mix("):
        inner = text[len("color-mix("):].rstrip(")")
        # strip the colour space token
        inner = re.sub(r"^in\s+\w+\s*,", "", inner).strip()
        stops = split_stops(inner)
        if len(stops) != 2:
            return None
        parsed = []
        for stop, pct in stops:
            c = parse_color(stop, toks, depth + 1)
            if c is None:
                return None
            parsed.append((c, pct))
        (c1, p1), (c2, p2) = parsed
        if p1 is None and p2 is None:
            p1, p2 = 50.0, 50.0
        elif p1 is None:
            p1 = 100.0 - p2
        elif p2 is None:
            p2 = 100.0 - p1
        total = p1 + p2
        if total <= 0:
            return None
        # Premultiplied mix; alpha scales with the summed weight when < 100%.
        scale = min(total, 100.0) / 100.0
        out = []
        for i in range(3):
            v = (c1[i] * p1 + c2[i] * p2) / total
            out.append(v)
        a = (c1[3] * p1 + c2[3] * p2) / total
        return (out[0], out[1], out[2], a * scale)
    return None


def split_stops(inner):
    """Split 'A 40%,B' into [(A,40.0),(B,None)] respecting nested parens."""
    parts, depth, buf = [], 0, ""
    for ch in inner:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(buf)
            buf = ""
        else:
            buf += ch
    parts.append(buf)
    out = []
    for p in parts:
        p = p.strip()
        m = re.match(r"^(.*?)\s+([\d.]+)%$", p)
        if m:
            out.append((m.group(1).strip(), float(m.group(2))))
        else:
            out.append((p, None))
    return out


def over(fg, bg):
    """Composite fg (possibly translucent) over an opaque bg."""
    a = fg[3]
    return tuple(fg[i] * a + bg[i] * (1 - a) for i in range(3)) + (1.0,)


def lum(c):
    def ch(v):
        v /= 255.0
        return v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4
    return 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])


def contrast(fg, bg):
    lf, lb = lum(fg), lum(bg)
    return (max(lf, lb) + 0.05) / (min(lf, lb) + 0.05)


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
