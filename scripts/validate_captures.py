"""校验一轮视觉采集的产物是否可信，而不是"文件存在就算通过"。

检查四件事，任何一条不过就判失败：
1. PNG 尺寸 == 请求的 width x height（窗口被平台缩放或裁剪时这条会挂）；
2. 报告里的窗口坐标 == (0,0)（夹具模式已锁死位置；非 (0,0) 说明跑的是旧产物或位置没生效）；
3. 纯黑像素占比 < 3%（窗口有一部分在屏幕外时，Robot 会把那块截成纯黑）；
4. PNG 字节数 >= 4096（防止截到空白帧）。

外加一条**前置**检查（0. 报告本身完整）：`results` 非空、且与报告自称的 `total` / `captured` 一致。
没有这条时，一份**什么都没采到**的报告会算出 total=0 / failures=0 并退出 0；
采集中途中断只写回 8 张的报告同样会通过。判据"没有任何一张不可信"在零张时是空洞成立的。

为什么需要第 3 条：窗口落到屏幕外时截出来的图仍然是一张合法 PNG，
尺寸也对，肉眼扫一眼也像"正常截图"，只有纯黑占比能把它抓出来。
实测踩过一次——1110dp 的夹具落到 (805,245)，右侧 379dp 出屏，字节数从 151KB 掉到 96KB。

**第 5 条（本轮补）：夹具是否真的到达了预期画面。** 上面四条全部"看图是否健康"，
没有一条看"图里是不是它该拍的画面"。实测踩过一个假通过口子：`settings_download-light`
**整幅没有画出设置对话框**（空表底色从 32.5% 涨到 82.9%），但尺寸/原点/纯黑占比/字节数全过。
判据是"锚点色占比"：每个夹具必须出现它该有的大块平坦色（对话框表面 / 侧栏底 / 选中行）。
阈值 = 改前基线实测占比的约 40~50%，不是拍脑袋——实测值写在 `ANCHORS` 的注释里。
文字色不能当锚点（抗锯齿会让精确值趋近 0）。
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

BLACK_FRACTION_LIMIT = 0.03

# 主题调色板（取自 Main.kt 的 WorkbenchPalette）
PALETTE = {
    "light": {"rail": (255, 255, 255), "dialog": (252, 253, 254), "selected": (231, 240, 255)},
    "dark": {"rail": (28, 31, 35), "dialog": (34, 38, 42), "selected": (38, 58, 81)},
}

# 每个夹具"到达了预期画面"的锚点色及最低占比。
# 阈值 = 改前基线（compose-visual-all）实测占比的约 40~50%，实测值：
#   new_task 0.199/0.201  batch 0.126/0.127  harvest 0.129/0.130  queues 0.293/0.299
#   extension 0.090/0.092 cast dialog 0.048 + rail 0.759  settings 0.408/0.409
#   settings_download 0.303/0.306  settings_network 0.313/0.314  settings_devices 0.374/0.376
#   settings_appearance 0.459/0.460  tasks_1000 rail 0.752（无对话框）
ANCHORS = {
    "tasks_1000": [("rail", 0.30)],
    "new_task": [("dialog", 0.08)],
    "batch": [("dialog", 0.05)],
    "harvest": [("dialog", 0.05)],
    "queues": [("dialog", 0.10)],
    "extension": [("dialog", 0.035)],
    "cast": [("dialog", 0.02), ("rail", 0.30)],
    "settings": [("dialog", 0.15)],
    "settings_download": [("dialog", 0.10)],
    "settings_network": [("dialog", 0.12)],
    "settings_devices": [("dialog", 0.15)],
    "settings_appearance": [("dialog", 0.18)],
}

# 所有夹具都必须满足的通用锚点：侧栏或对话框底色合计。它能抓住"整幅空帧 / 拍到了别的画面 / 主题错了"。
# 实测基线里最小的是 extension（dialog 0.090，rail 0）⇒ 阈值取 0.04（约 44%）。
# 不把 selected 当通用锚点：batch/harvest/new_task/extension 四个夹具的选中行占比是 0.000
#（对话框把活跃行盖住了），拿它做通用判据会把这 8 张全误判——这是对照实验查出来的。
UNIVERSAL_RAIL_DIALOG = 0.04


def _colour_fraction(arr, rgb):
    return float(((arr == list(rgb)).all(axis=1)).mean())


def surface_reasons(fixture, theme, im):
    """返回 list[str]：该夹具的锚点色占比不达阈值的原因。"""
    palette = PALETTE.get(theme or "")
    if palette is None:
        return [f"未知主题 {theme!r} —— 无法判定锚点色"]
    arr = np.asarray(im.convert("RGB")).reshape(-1, 3)
    reasons = []

    fractions = {k: _colour_fraction(arr, v) for k, v in palette.items()}
    if fractions["rail"] + fractions["dialog"] < UNIVERSAL_RAIL_DIALOG:
        reasons.append(
            "侧栏/对话框底色合计仅 %.1f%% < %.0f%%（整幅可能是空帧或拍到了别的画面）"
            % (100.0 * (fractions["rail"] + fractions["dialog"]), 100.0 * UNIVERSAL_RAIL_DIALOG))

    anchors = ANCHORS.get(fixture or "")
    if anchors is None:
        # 不在表里的夹具没有"对话框必须在场"的判据，只能走通用锚点。
        # 明确说出来，别让人误以为它覆盖了"该夹具的专属画面"。
        print(f"  注：夹具 {fixture!r} 没有专属锚点，只做了通用校验（若它会打开对话框，请补一行 ANCHORS）")
    else:
        for token, minimum in anchors:
            if fractions[token] < minimum:
                reasons.append(
                    f"锚点 {token} 占比 {fractions[token]:.1%} < {minimum:.1%}"
                    f" —— 该夹具应打开的{'对话框' if token == 'dialog' else '侧栏'}没出现")
    return reasons


def black_fraction(im):
    # 转灰度再数 0 值，比逐像素比 RGB 三元组快一个量级
    hist = im.convert("L").histogram()
    return hist[0] / float(im.size[0] * im.size[1])


def main():
    root = Path(sys.argv[1] if len(sys.argv) > 1 else r"A:\Ubuntu\测试\hls-downloader\artifacts\v7-productization\compose-visual-responsive")
    report = root / "capture-report.json"
    if not report.exists():
        print(f"找不到采集报告：{report}")
        return 1

    data = json.loads(report.read_text(encoding="utf-8"))

    # 先证明"这份报告本身是完整的"，再看每一张。否则空报告会算出
    # total=0 / failures=0 并退出 0 —— 一份"什么都没采到"的报告被当成通过。
    # 这是本项目第四次栽在同一件事上（静默污染证据），所以这里一律硬失败。
    results = data.get("results")
    if not isinstance(results, list) or not results:
        print("报告里没有 results（或为空）—— 无法判定任何一张，按失败处理")
        return 1
    declared_total = data.get("total")
    if isinstance(declared_total, int) and declared_total != len(results):
        print(f"报告自称 total={declared_total}，实际只有 {len(results)} 条结果 —— 采集可能中途中断")
        return 1
    declared_captured = data.get("captured")
    n_captured = sum(1 for i in results if i.get("status") == "captured")
    if isinstance(declared_captured, int) and declared_captured != n_captured:
        print(f"报告自称 captured={declared_captured}，实际 {n_captured} 条 —— 报告与产物不一致")
        return 1

    failures = 0
    print(f"{'夹具':<34}{'尺寸':<12}{'坐标':<12}{'黑占比':<10}{'判定'}")
    print("-" * 90)
    for item in results:
        name = f"{item['fixture']}-{item['theme']}-{item['width']}x{item['height']}"
        reasons = []
        if item.get("status") != "captured":
            reasons.append(f"采集未成功：{item.get('detail', '')[:60]}")
        png = Path(item.get("png") or "")
        if not png.exists():
            reasons.append("PNG 不存在")
            print(f"{name:<34}{'-':<12}{'-':<12}{'-':<10}{'；'.join(reasons)}")
            failures += 1
            continue

        try:
            window = json.loads(item.get("detail") or "{}")
        except json.JSONDecodeError:
            window = {}
        pos = (window.get("x"), window.get("y"))
        if pos != (0, 0):
            reasons.append(f"窗口坐标 {pos} ≠ (0,0)")

        im = Image.open(png)
        expect = (item["width"], item["height"])
        if im.size != expect:
            reasons.append(f"图尺寸 {im.size} ≠ {expect}")
        frac = black_fraction(im)
        if frac >= BLACK_FRACTION_LIMIT:
            reasons.append(f"纯黑占比 {frac:.1%} ≥ {BLACK_FRACTION_LIMIT:.0%}（疑似出屏）")
        if png.stat().st_size < 4096:
            reasons.append(f"字节数 {png.stat().st_size} < 4096")

        reasons += surface_reasons(item.get("fixture"), item.get("theme"), im)

        verdict = "OK" if not reasons else "；".join(reasons)
        if reasons:
            failures += 1
        print(f"{name:<34}{str(im.size):<12}{str(pos):<12}{frac:<10.2%}{verdict}")

    total = len(results)
    print("-" * 90)
    print(f"共 {total} 张，不可信 {failures} 张")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
