"""从截图像素里量出侧栏宽度，验证 1120dp 折叠断点真的生效。

为什么不用"看截图"下结论：侧栏 190dp 和 56dp 在缩略图上都"像那么回事"，
而断点是 1120dp 这种差 10dp 就翻转的边界，必须量数字。

检测原理：侧栏右边缘画的是 1dp 的 `border`（浅色 #D8E0EA / 深色 #383D43），
它与左侧的 `rail`（#FFFFFF / #1C1F23）和右侧的 `canvas`（#EEF2F6 / #151719）都不相等。
在 x ∈ [30, 300] 里找与该边框色匹配的像素，它的下标 + 1 就是侧栏宽度。

**两个必须处理的干扰，都是实测踩出来的：**

1. **弹窗打开时颜色被遮罩压暗。** 本项目实测遮罩是"乘 ~0.657"：
   浅色 `#d8e0ea` → `#8e939a`、`rail` `#ffffff` → `#a8a8a8`；
   深色 `#383d43` → `#24282c`、`rail` `#1c1f23` → `#121417`。
   所以不能只按本色匹配，要按一组候选遮罩系数去试。

2. **弹窗自己的 1dp 边框也是 `border` 本色，而且没被压暗**，
   于是"最接近目标色"的像素是弹窗左边缘（1400dp 视口下恒在 (1400−880)/2 = 260）。
   本项目实测：不加区分地扫 24 张全夹具截图，15 张报出"异常"值（261 或 31），**全是假阳性**。

   区分办法：**侧栏边框的左边是 `rail`，弹窗边框的左边是遮罩下的画布**。
   用候选自身推出的遮罩系数把左邻像素归一化，再看它是否等于 `rail` 色。
   实测：侧栏边框左邻归一化后距离 rail 本色 = 1；弹窗边框左邻 = 44。阈值取 20 分得很干净。

测不出来时**必须报"不确定"**，不能给一个自信的错数——一个敢说"我测不了"的检测器，
比一个报错数的检测器有用。
"""
import sys
from collections import Counter
from pathlib import Path

from PIL import Image

BORDER = {
    "light": (0xD8, 0xE0, 0xEA),
    "dark": (0x38, 0x3D, 0x43),
}
RAIL = {
    "light": (0xFF, 0xFF, 0xFF),
    "dark": (0x1C, 0x1F, 0x23),
}
# 候选遮罩系数。1.0 = 无遮罩；实测弹窗遮罩约 0.657，多给几档容错。
SCALES = (1.0, 0.86, 0.76, 0.66, 0.56, 0.46)
BORDER_TOL = 26      # 归一化后与 border 本色的允许偏差
RAIL_TOL = 20        # 归一化后左邻与 rail 本色的允许偏差
COLUMN_UNIFORMITY = 0.85   # 一条竖线要在 ≥85% 的采样行上保持同一颜色
EXPECTED_EXPANDED = 190
EXPECTED_COLLAPSED = 56
COLLAPSE_THRESHOLD = 1120


def dist(a, b):
    return abs(a[0] - b[0]) + abs(a[1] - b[1]) + abs(a[2] - b[2])


def _scaled(color, scale):
    return tuple(min(255, max(0, c * scale)) for c in color)


def detect_width(path, theme):
    """-> (width|None, size, hits, rows_sampled)

    width 为 None 表示**测不出来**。调用方必须把 None 当作"未验证"，不能当作"通过"。

    按**列**分析而不是按行扫描，因为还要判"这是一条竖线"：
    真正的 1dp 边框在整列上颜色恒定（跨全部采样行），而图标抗锯齿边缘不是
    —— 实测深色弹窗夹具里，图标边缘的某列恰好落在"压暗后的边框色"容差内，
    按行扫描会把它当成 x=33 处的边框（假阳性 -157dp）。
    """
    im = Image.open(path).convert("RGB")
    px = im.load()
    w, h = im.size
    border = BORDER[theme]
    rail = RAIL[theme]
    ys = list(range(int(h * 0.20), int(h * 0.90), 7))
    rows = len(ys)

    def column_matches(x):
        """这一列是不是一条贯穿全高的、颜色恒定的、且左边是 rail 的边框？"""
        values = [px[x, y] for y in ys]
        # 列内一致性：取该列众数，要求它覆盖绝大多数采样行
        mode, count = Counter(values).most_common(1)[0]
        if count < rows * COLUMN_UNIFORMITY:
            return False
        lefts = [px[max(0, x - 4), y] for y in ys]
        left_mode = Counter(lefts).most_common(1)[0][0]
        for scale in SCALES:
            b = _scaled(border, scale)
            r = _scaled(rail, scale)
            # 关键：必须**更接近 border 而不是 rail**。
            # 只用绝对容差会出事——遮罩系数越小，scaled(border) 越往黑里塌，
            # 最后塌到 rail 本色附近，于是整片纯背景都被判成"边框"
            # （实测深色弹窗夹具因此在 x=34 报出 35dp）。
            if dist(mode, b) > BORDER_TOL or dist(mode, b) >= dist(mode, r):
                continue
            # 左边必须是 rail：这条排除弹窗左边缘（它的左边是遮罩下的画布）
            if dist(left_mode, r) <= RAIL_TOL and dist(left_mode, r) < dist(left_mode, b):
                return True
        return False

    for x in range(32, min(300, w)):
        if column_matches(x):
            return x + 1, im.size, rows, rows
    return None, im.size, 0, rows


def main():
    root = Path(sys.argv[1] if len(sys.argv) > 1 else r"A:\Ubuntu\测试\hls-downloader\artifacts\v7-productization\compose-visual-responsive")
    pattern = sys.argv[2] if len(sys.argv) > 2 else "tasks_1000-*.png"
    files = sorted(root.glob(pattern))
    if not files:
        print(f"没有找到截图：{root} / {pattern}")
        return 1

    failures = 0
    undecided = 0
    print(f"{'文件':<44}{'图尺寸':<12}{'量到侧栏':<10}{'期望':<8}{'判定'}")
    print("-" * 96)
    for path in files:
        theme = "dark" if "-dark-" in path.name else "light"
        try:
            win_w = int(path.stem.split("-")[-1].split("x")[0])
        except ValueError:
            win_w = None
        width, size, hits, rows = detect_width(path, theme)
        expected = EXPECTED_COLLAPSED if (win_w and win_w < COLLAPSE_THRESHOLD) else EXPECTED_EXPANDED
        if width is None:
            verdict = "不确定：找不到贯穿全高的侧栏边框"
            undecided += 1
        elif abs(width - expected) <= 1:
            verdict = "OK"
        else:
            verdict = f"不符（差 {width - expected:+d}dp）"
            failures += 1
        print(f"{path.name:<44}{str(size):<12}{str(width):<10}{expected:<8}{verdict}  (采样 {rows} 行)")

    print("-" * 96)
    print(f"共 {len(files)} 张：不符 {failures} 张，不确定 {undecided} 张")
    if undecided:
        print("注：'不确定'不是通过。")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
