# -*- coding: utf-8 -*-
"""Audit the extension's *filled* controls, including modifiers that only
override `background` and inherit `color` from a base class.

The first auditor (audit_ext_contrast.py) skipped any rule lacking both a
`color` and a `background` in the same declaration block, which silently
dropped every modifier fill (.download.push-tv, .download.cast, the :hover
states, ...). This one resolves the effective foreground by cascading a small
explicit inheritance map, and can apply a proposed token override so the
before/after can be compared in one run.

Usage:
  python audit_ext_fills.py                 # report current values
  python audit_ext_fills.py --on-primary "#151719"   # report with an override
"""
import argparse
import os
import re
import sys

EXT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "extension")
THEME = os.path.join(EXT, "lib", "theme.ts")


# ---------------------------------------------------------------- token parsing
def parse_tokens():
    src = open(THEME, encoding="utf-8").read()
    out = {}
    for theme in ("dark", "light"):
        m = re.search(r'\[data-hlsd-theme="%s"\]\{(.*?)\}' % theme, src, re.S)
        toks = {}
        for name, value in re.findall(r"--([\w-]+):\s*([^;]+);", m.group(1)):
            toks[name] = value.strip()
        out[theme] = toks
    return out


HEX = re.compile(r"^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$")
RGBA = re.compile(r"^rgba?\(([^)]+)\)$")


def split_stops(inner):
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
        out.append((m.group(1).strip(), float(m.group(2))) if m else (p, None))
    return out


def parse_color(text, toks, depth=0):
    if depth > 6:
        return None
    text = (text or "").strip()
    m = re.match(r"^var\(--([\w-]+)\)$", text)
    if m:
        name = m.group(1)
        return parse_color(toks[name], toks, depth + 1) if name in toks else None
    m = HEX.match(text)
    if m:
        h = m.group(1)
        if len(h) == 3:
            h = "".join(c * 2 for c in h)
        if len(h) == 8:
            return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16), int(h[6:8], 16) / 255)
        return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16), 1.0)
    m = RGBA.match(text)
    if m:
        parts = [p.strip() for p in m.group(1).split(",")]
        r, g, b = (float(p) for p in parts[:3])
        return (r, g, b, float(parts[3]) if len(parts) > 3 else 1.0)
    if text.startswith("color-mix("):
        # Strip exactly ONE trailing paren: rstrip(")") would also eat the
        # inner paren of a trailing var(--x), making every
        # `color-mix(...,var(--surface))` silently unparseable.
        inner = text[len("color-mix("):]
        if inner.endswith(")"):
            inner = inner[:-1]
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
        scale = min(total, 100.0) / 100.0
        out = [(c1[i] * p1 + c2[i] * p2) / total for i in range(3)]
        a = (c1[3] * p1 + c2[3] * p2) / total
        return (out[0], out[1], out[2], a * scale)
    return None


def over(fg, bg):
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


# ---------------------------------------------------------------- stylesheet scan
SOURCES = [
    ("popup/style.css", os.path.join(EXT, "entrypoints", "popup", "style.css")),
    ("lib/theme.ts", THEME),
    ("content.ts", os.path.join(EXT, "entrypoints", "content.ts")),
    ("takeover.content.ts", os.path.join(EXT, "entrypoints", "takeover.content.ts")),
    ("hooks.content.ts", os.path.join(EXT, "entrypoints", "hooks.content.ts")),
]

DECL = re.compile(r"(?<![\w-])(color|background|background-color)\s*:\s*([^;}]+)")
FONT = re.compile(r"font(?:-size)?\s*:\s*([^;}]*)")


def load_rules():
    """selector -> {declarations} for every rule, keeping the first definition."""
    rules = {}
    for fname, path in SOURCES:
        if not os.path.exists(path):
            continue
        text = open(path, encoding="utf-8").read()
        for m in re.finditer(r"([^{}\n]{1,300})\{([^{}]*)\}", text):
            sel, body = m.group(1).strip(), m.group(2)
            if ":" not in body:
                continue
            decls = {}
            for prop, val in DECL.findall(body):
                key = "background" if prop.startswith("background") else prop
                decls.setdefault(key, val.strip())
            f = FONT.search(body)
            if f:
                decls["_font"] = f.group(1)
            if decls:
                rules.setdefault(sel, (fname, decls))
    return rules


# Modifier selectors that override only `background`; colour is inherited from
# the listed base selectors. Chains run LOW specificity -> HIGH, and the target
# selector is applied last, so a later base must not clobber an earlier one.
FILLS = [
    (".hlsd-button.primary", [".hlsd-button"], "主要按钮"),
    (".hlsd-button.primary:hover:not(:disabled)", [".hlsd-button", ".hlsd-button.primary"], "主要按钮 hover"),
    (".download", [], "面板下载"),
    (".download:hover", [".download"], "面板下载 hover"),
    (".download.push-tv", [".download"], "推送 TVBox"),
    (".download.push-tv:hover", [".download", ".download.push-tv"], "推送 TVBox hover"),
    (".download.cast", [".download"], "投屏"),
    (".download.cast:hover", [".download", ".download.cast"], "投屏 hover"),
    (".hover-action.primary", [".hover-action"], "悬浮卡主操作"),
    (".hover-action.primary:hover", [".hover-action", ".hover-action.primary"], "悬浮卡主操作 hover"),
    (".video-download", [], "视频悬浮按钮"),
    (".video-download:hover", [".video-download"], "视频悬浮按钮 hover"),
    (".video-download b", [], "视频计数徽标"),
    (".update-notice button", [], "更新提示按钮"),
    (".push-button:hover:not(:disabled)", [".hlsd-button", ".push-button"], "推送按钮 hover"),
]

# Tinted (non-solid) pairs: text colour on a low-alpha tint of the same hue.
# (selector, base_chain low->high, label, bg_override)
#
# The chain matters twice over: a modifier like `.push-button` declares only a
# colour and inherits its box from `.hlsd-button`, and a `:hover` rule often
# declares only `background`, so the *label* colour has to be pulled from the
# base state. Listing the base state alone is how the first version of this list
# silently skipped .pin.active, .scan-button:hover and .hlsd-button.active:hover.
#
# `bg_override` is for elements that declare no background of their own because
# they sit on a container that paints it (.status.online sits on header's
# --surface). Without it the pair is skipped in silence, which reads as "passing".
TINTS = [
    (".hlsd-badge", [], "徽标", None),
    (".hlsd-button.active", [".hlsd-button"], "激活态按钮", None),
    (".hlsd-button.active:hover:not(:disabled)",
     [".hlsd-button", ".hlsd-button.active", ".hlsd-button:hover:not(:disabled)"], "激活态按钮 hover", None),
    (".empty-icon", [], "空态图标", None),
    (".scan-button:hover:not(:disabled)", [".scan-button"], "扫描按钮 hover", None),
    (".result", [], "结果条", None),
    (".result.error", [".result"], "错误结果条", None),
    (".send-error", [], "发送错误", None),
    (".push-button", [".hlsd-button"], "推送按钮(淡)", None),
    (".pin.active", [".pin"], "面板固定态", None),
    (".status.online", [], "在线状态", "var(--surface)"),
    (".status", [], "离线状态", "var(--surface)"),
    (".video-download.identifying", [], "识别中", None),
]


def effective(sel, base_chain, rules):
    """Merge base_chain (low→high) then sel, later wins."""
    out = {}
    for s in list(base_chain) + [sel]:
        if s in rules:
            out.update(rules[s][1])
    return out, (rules.get(sel) or rules.get(base_chain[-1] if base_chain else sel, ("?", {})))[0]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--on-primary", default=None, help="override --on-primary for both themes")
    ap.add_argument("--on-primary-dark", default=None)
    ap.add_argument("--on-primary-light", default=None)
    ap.add_argument("--only", default=None, help="substring filter on the label")
    args = ap.parse_args()

    tokens = parse_tokens()
    rules = load_rules()
    if args.on_primary:
        tokens["dark"]["on-primary"] = args.on_primary
        tokens["light"]["on-primary"] = args.on_primary
    if args.on_primary_dark:
        tokens["dark"]["on-primary"] = args.on_primary_dark
    if args.on_primary_light:
        tokens["light"]["on-primary"] = args.on_primary_light

    print(f"{'ratio':>7}  {'theme':5}  {'size':>5}  {'kind':9}  label / selector")
    print("-" * 108)
    rows = []

    for sel, chain, label in FILLS:
        if args.only and args.only not in label and args.only not in sel:
            continue
        decls, fname = effective(sel, chain, rules)
        bg_txt, fg_txt = decls.get("background"), decls.get("color")
        if not bg_txt or not fg_txt:
            print(f"{'?':>7}  {'-':5}  {'-':>5}  {'MISSING':9}  {label} :: {sel}  (bg={bg_txt} color={fg_txt})")
            continue
        for theme in ("dark", "light"):
            toks = tokens[theme]
            bg, fg = parse_color(bg_txt, toks), parse_color(fg_txt, toks)
            if bg is None or fg is None:
                continue
            if bg[3] < 1.0:
                bg = over(bg, parse_color(toks["bg"], toks))
            fg_solid = over(fg, bg)
            size = None
            fm = re.search(r"([\d.]+)px", decls.get("_font", ""))
            if fm:
                size = float(fm.group(1))
            rows.append((contrast(fg_solid, bg), theme, size, "fill", label, sel, fg_txt, bg_txt, fname))

    for sel, chain, label, bg_override in TINTS:
        if args.only and args.only not in label and args.only not in sel:
            continue
        if sel not in rules:
            print(f"{'?':>7}  {'-':5}  {'-':>5}  {'MISSING':9}  {label} :: {sel}  (no such selector)")
            continue
        decls, fname = effective(sel, chain, rules)
        bg_txt, fg_txt = decls.get("background"), decls.get("color")
        if bg_override:
            if not fg_txt:
                print(f"{'?':>7}  {'-':5}  {'-':>5}  {'MISSING':9}  {label} :: {sel}  (no colour)")
                continue
            bg_txt = bg_override
        if not bg_txt or not fg_txt:
            print(f"{'?':>7}  {'-':5}  {'-':>5}  {'MISSING':9}  {label} :: {sel}  (bg={bg_txt} color={fg_txt})")
            continue
        for theme in ("dark", "light"):
            toks = tokens[theme]
            bg, fg = parse_color(bg_txt, toks), parse_color(fg_txt, toks)
            if bg is None or fg is None:
                continue
            if bg[3] < 1.0:
                bg = over(bg, parse_color(toks["bg"], toks))
            rows.append((contrast(over(fg, bg), bg), theme, None, "tint", label, sel, fg_txt, bg_txt, fname))

    rows.sort(key=lambda r: (r[1], r[0]))
    under3 = under45 = 0
    for r, theme, size, kind, label, sel, fg_txt, bg_txt, fname in rows:
        flag = ""
        if r < 3.0:
            flag = "  <<< BELOW 3.0"
            under3 += 1
        elif r < 4.5:
            flag = "  <<< BELOW 4.5"
            under45 += 1
        print(f"{r:7.2f}  {theme:5}  {str(size):>5}  {kind:9}  {label} :: {sel}{flag}")
        if flag:
            print(f"{'':7}  {'':5}  {'':>5}  {'':9}    fg={fg_txt}  bg={bg_txt}  [{fname}]")

    # 退出码：低于 3.0 就是硬失败（1.4.11 的下限）。空输入不是通过——
    # 解析器一条都匹配不到时，"低于 3.0 的有 0 条"是空洞成立的。
    print("-" * 100)
    print(f"rows: {len(rows)}   below 3.0: {under3}   below 4.5: {under45}")
    if not rows:
        print("!! 一条规则都没解析出来 —— 审计器失效，不是\"零失败\"")
        return 1
    return 1 if under3 else 0


if __name__ == "__main__":
    sys.exit(main())
