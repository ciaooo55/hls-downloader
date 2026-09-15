# -*- coding: utf-8 -*-
"""把一次改前/改后的像素差异**解释**成可判断的证据，而不是只报一个数字。

`compare_captures.py` 回答"哪些夹具变了、变了多少像素"；它回答不了"变的是什么"。
而本项目对差异的态度是：**每一处差异都必须被解释，否则视为回归**。像素计数本身
解释不了任何东西 —— 33,652 个差异像素既可能是"某个列表显示了不同的内容"，
也可能是"某个开关的滑块换了颜色"，两者的处置完全不同。

所以这个脚本给出两样东西：

1. **颜色迁移表**（old RGB → new RGB，按像素数排序）。判读方法：
   * 少数几条、且是纯色到纯色 —— 样式/状态变化（例如滑块从白换成近黑）；
   * 长尾、大量相近色 —— 文字/内容不同（抗锯齿会把每个字边都变成一条）；
   * 出现"文字色 → 背景色"这类对 —— 某个文字消失了。

2. **行带分布**（把差异行聚成连续区间，各给一个 bbox）。判读方法：
   * 窄而高的竖条、x 固定 —— 某一列控件（开关/滚动条/指示条）；
   * 正好一行高（如 36px）的横条、x 从侧栏内边距开始 —— 一个列表行；
   * 覆盖大片 —— 内容整体不同，或整页换了一屏。

用 numpy 向量化：纯 Python 逐像素在 1400x820 上每张要数秒，12 张就顶到命令时限了。

用法：
  python explain_capture_diff.py <new_dir> <baseline_dir> [capture ...]
不给 capture 名字就解释全部有差异的夹具。退出码：0 = 至少解释了一个夹具；
1 = 一个都没能解释（比如目录/基线不对，或根本没有差异）—— 空输入不当作通过。
"""
import os
import sys
from collections import Counter

import numpy as np
from PIL import Image

MAX_TRANSITIONS = 8


def load_rgb(path):
    with Image.open(path) as im:
        return np.asarray(im.convert("RGB"), dtype=np.uint8)


def differing_pairs(new_dir, base_dir, name):
    """-> dict / None(确实无差异) / {'error': ...}

    「文件不存在」与「两张图一样」必须分开报。混成一句 "无差异（或文件缺失）"
    踩过一次：夹具名少写了 `-1400x820` 后缀，脚本报"无差异"，看起来像"这个夹具
    没被改动"—— 而实际上它一张图都没读。这正是本项目反复栽的静默跳过。
    """
    a_path = os.path.join(new_dir, name + ".png")
    b_path = os.path.join(base_dir, name + ".png")
    missing = [p for p in (a_path, b_path) if not os.path.exists(p)]
    if missing:
        return {"error": "文件不存在：%s" % "; ".join(missing)}
    a, b = load_rgb(a_path), load_rgb(b_path)
    if a.shape != b.shape:
        return {"error": "尺寸不同 %s vs %s" % (a.shape, b.shape)}
    mask = np.any(a != b, axis=2)
    if not mask.any():
        return None

    old = b[mask]
    new = a[mask]
    # 必须先把每个像素的 R/G/B 折成一个 24 位整数，再把"旧像素+新像素"打包成 48 位。
    # 踩过一次：直接对 (N,3) 数组做 << / | 是**按通道**打包的，np.unique 会把
    # 三个通道当成三个独立取值 —— #445566->#AABBCC 会被拆成 #440000->#AA0000、
    # #550000->#BB0000、#660000->#CC0000 三条，像素数还翻三倍。颜色迁移表就废了。
    # 用 uint64 是必须的：两个 24 位字段共 48 位，uint32 会静默截断并把不同的
    # 颜色对折叠成同一条。
    def pack24(rgb):
        rgb = rgb.astype(np.uint64)
        return (rgb[..., 0] << 16) | (rgb[..., 1] << 8) | rgb[..., 2]

    old_px = pack24(old)
    new_px = pack24(new)
    packed = (old_px << 24) | new_px
    uniq, counts = np.unique(packed, return_counts=True)
    trans = Counter()
    for v, n in zip(uniq.tolist(), counts.tolist()):
        # 解码顺序必须与 pack24 严格镜像：R 在高位（bit16）、B 在低位（bit0）。
        # 写反过一次，输出变成 #665544（= 把 B 当 R 读），颜色迁移表会把人指向
        # 完全错误的颜色，比不输出更糟。
        o = ((v >> 40) & 0xFF, (v >> 32) & 0xFF, (v >> 24) & 0xFF)
        t = ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
        trans[(o, t)] += n

    rows = np.where(mask.any(axis=1))[0]
    xmin = np.where(mask, np.arange(mask.shape[1])[None, :], mask.shape[1]).min(axis=1)
    xmax = np.where(mask, np.arange(mask.shape[1])[None, :], -1).max(axis=1)
    row_extent = {int(y): (int(xmin[y]), int(xmax[y])) for y in rows.tolist()}
    return {"w": a.shape[1], "h": a.shape[0], "trans": trans, "row_extent": row_extent}


def bands(rows):
    """把差异行聚成连续区间 -> [(y0, y1)]（闭区间）。"""
    out = []
    start = prev = None
    for y in rows:
        if start is None:
            start = prev = y
            continue
        if y == prev + 1:
            prev = y
            continue
        out.append((start, prev))
        start = prev = y
    if start is not None:
        out.append((start, prev))
    return out


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 1
    new_dir, base_dir = sys.argv[1], sys.argv[2]
    wanted = sys.argv[3:]
    if not wanted:
        wanted = sorted(f[:-4] for f in os.listdir(new_dir) if f.endswith(".png"))

    explained = 0
    problems = 0
    for name in wanted:
        res = differing_pairs(new_dir, base_dir, name)
        if res is not None and "error" in res:
            problems += 1
            print("!! %s == %s" % (name, res["error"]))
            continue
        if res is None:
            print("== %s == 无差异（两张图逐像素相同）" % name)
            continue
        explained += 1
        total = sum(res["trans"].values())
        print("== %s == 差异 %d px" % (name, total))
        print("   颜色迁移（旧 -> 新，取前 %d 条）：" % MAX_TRANSITIONS)
        for (old, new), n in res["trans"].most_common(MAX_TRANSITIONS):
            print("     #%02X%02X%02X -> #%02X%02X%02X   %8d px  (%.1f%%)"
                  % (old[0], old[1], old[2], new[0], new[1], new[2], n, 100.0 * n / total))
        if len(res["trans"]) > MAX_TRANSITIONS:
            print("     …… 共 %d 种不同的颜色对" % len(res["trans"]))
        extent = res["row_extent"]
        print("   行带分布（共 %d 行有差异）：" % len(extent))
        for y0, y1 in bands(sorted(extent)):
            xmin = min(extent[y][0] for y in range(y0, y1 + 1))
            xmax = max(extent[y][1] for y in range(y0, y1 + 1))
            print("     y %4d..%-4d  高 %3d   bbox=(%d, %d, %d, %d)"
                  % (y0, y1, y1 - y0 + 1, xmin, y0, xmax + 1, y1 + 1))
        print()

    if explained == 0:
        print("没有任何夹具被解释（全部无差异 / 路径不对）—— 不当作通过")
        return 1
    if problems:
        print("!! 有 %d 个夹具连图都没读到，上面的结论不覆盖它们" % problems)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
