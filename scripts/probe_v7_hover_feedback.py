"""悬停反馈探针：证明"鼠标移上去真的有视觉变化，且变化落在被悬停的那一项上"。

为什么需要它：`/action {"type":"move",x,y}` 会移动真实指针（`Robot.mouseMove`），
所以悬停态是可以夹具化的。但**只截一张图看一眼是不够的**——"没出现反馈"和
"反馈画在别处"在缩略图上是同一个现象。判据必须落在坐标上：

  1. 悬停前后的差异包围盒 **非空**；
  2. 该包围盒 **与被悬停项的矩形相交**。

两个对照组不可省，否则"全部通过"不构成证据：

  * **基线稳定性**：连拍两张基线必须**逐像素相同**。夹具里若有动画未收敛
    （进度条、无限旋转的 spinner），差异会被误读成"悬停反馈"，整轮判定失效。
  * **惰性点**：侧栏自己的 9dp 内边距区域（x=4，任何子项之外）必须**零差异**。
    它证明这套测量真的能说出"这里没变化"，而不是无脑报一个 bbox。

任一对照组不成立 → 整轮 INVALID 并以退出码 1 结束（不是警告）。

用法：
  python probe_v7_hover_feedback.py --app <HLSDownloader.exe> --fixture tasks_1000 \
      --theme light --width 1400 --height 820 --out <目录>
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

from PIL import Image, ImageChops

TOKEN = "hls-visual-capture-20260914"
PRODUCT_PROCESS_NAMES = (
    "HLSDownloader.exe",
    "HLSDownloaderEngine.exe",
    "HLSDownloaderPresenter.exe",
    "HLSDownloaderNativeHost.exe",
)

# 选中底：浅色 #E7F0FF / 深色 #263A51。用来在像素里定位"当前选中的那一项"，
# 从而推出相邻项的位置——不靠手算布局偏移，布局一变探针不会静默指向别处。
SELECTED_SURFACE = {"light": (0xE7, 0xF0, 0xFF), "dark": (0x26, 0x3A, 0x51)}

NAV_ROW_HEIGHT = 36  # 展开态侧栏行高
RAIL_ITEM_HEIGHT = 40  # 折叠态图标栏项高
RAIL_ITEM_GAP = 2  # 折叠态项间距（Arrangement.spacedBy(2.dp)）


def request(base: str, path: str, method: str = "GET", payload=None):
    data = None
    headers = {"X-HLS-Test-Token": TOKEN}
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(f"{base}{path}", data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read()


def wait_health(base: str, seconds: int) -> bool:
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            request(base, "/health")
            return True
        except Exception:
            time.sleep(0.3)
    return False


def capture(base: str, target: Path) -> None:
    target.write_bytes(request(base, "/screenshot"))


def move_pointer(base: str, x: int, y: int) -> None:
    request(base, "/action", "POST", {"type": "move", "x": x, "y": y})


def matches(pixel, target, tol=6) -> bool:
    return all(abs(pixel[i] - target[i]) <= tol for i in range(3))


def selected_band(im: Image.Image, target, x_from: int, x_to: int):
    """在 [x_from, x_to) 里找最长的一段 y，其多数像素等于选中底色。

    返回 (top, bottom) 或 None。判据是**结构性**的（"这一整段都是选中底色"），
    不是"最接近某个颜色"——后者在遮罩/缩放下会塌到别的基准色附近。
    """
    px = im.load()
    xs = list(range(x_from, x_to, 3))
    need = max(1, int(len(xs) * 0.5))
    bands, current = [], None
    for y in range(im.size[1]):
        hits = sum(1 for x in xs if matches(px[x, y], target))
        if hits >= need:
            current = [y, y] if current is None else [current[0], y]
        elif current is not None:
            bands.append(tuple(current))
            current = None
    if current is not None:
        bands.append(tuple(current))
    if not bands:
        return None
    return max(bands, key=lambda b: b[1] - b[0])


def diff_bbox(before: Path, after: Path):
    a = Image.open(before).convert("RGB")
    b = Image.open(after).convert("RGB")
    if a.size != b.size:
        raise SystemExit(f"尺寸不一致：{a.size} vs {b.size}")
    return ImageChops.difference(a, b).getbbox()


def intersects(box, rect) -> bool:
    x0, y0, x1, y1 = box
    rx0, ry0, rx1, ry1 = rect
    return not (x1 <= rx0 or rx1 <= x0 or y1 <= ry0 or ry1 <= y0)


def kill_product_processes() -> None:
    for name in PRODUCT_PROCESS_NAMES:
        subprocess.run(
            ["taskkill", "/IM", name, "/F"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", required=True)
    parser.add_argument("--fixture", default="tasks_1000")
    parser.add_argument("--theme", default="light", choices=("light", "dark"))
    parser.add_argument("--width", type=int, default=1400)
    parser.add_argument("--height", type=int, default=820)
    parser.add_argument("--port", type=int, default=19739)
    parser.add_argument("--timeout-seconds", type=int, default=120)
    parser.add_argument("--settle-ms", type=int, default=2500)
    parser.add_argument("--out", required=True)
    parser.add_argument("--report", default="")
    args = parser.parse_args()

    app = Path(args.app).resolve()
    if not app.is_file():
        raise SystemExit(f"应用不存在：{app}")
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    report_path = Path(args.report).resolve() if args.report else out / "hover-probe-report.json"

    base = f"http://127.0.0.1:{args.port}"
    env = dict(os.environ)
    env.update(
        {
            "HLS_UI_TEST_API": "1",
            "HLS_UI_TEST_TOKEN": TOKEN,
            "HLS_UI_TEST_PORT": str(args.port),
            "HLS_UI_AUDIT_SURFACE": args.fixture,
            "HLS_UI_AUDIT_THEME": args.theme,
            "HLS_UI_AUDIT_WIDTH": str(args.width),
            "HLS_UI_AUDIT_HEIGHT": str(args.height),
            "HLS_V6_SKIP_MIGRATE": "1",
            "HLS_V7_DATA_DIR": str(out / "profile"),
        }
    )

    kill_product_processes()
    stdout_log = (out / "stdout.log").open("wb")
    stderr_log = (out / "stderr.log").open("wb")
    proc = subprocess.Popen(
        [str(app)],
        cwd=str(app.parent),
        env=env,
        stdout=stdout_log,
        stderr=stderr_log,
    )

    findings: list[dict] = []
    invalid: list[str] = []
    failures: list[str] = []
    try:
        if not wait_health(base, args.timeout_seconds):
            raise SystemExit("测试 API 未在超时内就绪；见 " + str(out / "stderr.log"))
        time.sleep(args.settle_ms / 1000.0)
        request(base, "/action", "POST", {"type": "activate"})
        time.sleep(0.5)

        window = json.loads(request(base, "/window"))
        width, height = window["width"], window["height"]

        target_color = SELECTED_SURFACE[args.theme]
        # 侧栏内边距是 9dp，x=4 一定落在任何子项之外（惰性点）；展开态侧栏宽 190dp。
        rail_mode = width < 1120
        if rail_mode:
            probe_x, band_x_from, band_x_to = 28, 10, 50
            item_height, item_gap = RAIL_ITEM_HEIGHT, RAIL_ITEM_GAP
        else:
            probe_x, band_x_from, band_x_to = 95, 12, 170
            item_height, item_gap = NAV_ROW_HEIGHT, 0
        inert_x = 4

        # 把指针先停到惰性点，保证基线本身没有悬停态。
        move_pointer(base, inert_x, height // 2)
        time.sleep(0.4)
        baseline_a = out / "baseline-a.png"
        capture(base, baseline_a)
        time.sleep(0.4)
        baseline_b = out / "baseline-b.png"
        capture(base, baseline_b)

        stability = diff_bbox(baseline_a, baseline_b)
        if stability is not None:
            invalid.append(
                f"基线不稳定：连拍两张在 {stability} 处有差异，夹具里还有动画没收敛，"
                "悬停差异无法与它区分。整轮判定无效。"
            )
            findings.append({"name": "baseline-stability", "verdict": "INVALID", "bbox": stability})
        else:
            findings.append({"name": "baseline-stability", "verdict": "PASS", "bbox": None})

        im = Image.open(baseline_a).convert("RGB")
        band = selected_band(im, target_color, band_x_from, band_x_to)
        if band is None:
            invalid.append(
                f"在 x∈[{band_x_from},{band_x_to}) 里找不到选中底色 {target_color} 的连续段，"
                "无法定位被悬停项；探针的定位判据在这个夹具上失效。"
            )
            band = None
        else:
            findings.append(
                {
                    "name": "locate-active-item",
                    "verdict": "PASS",
                    "band": band,
                    "item_height": item_height,
                }
            )

        targets: list[tuple[str, int, int, tuple[int, int, int, int]]] = []
        if band is not None:
            top, bottom = band
            for index, offset in enumerate((item_gap + item_height, 2 * (item_gap + item_height)), start=1):
                y0 = bottom + 1 + (offset - item_height)
                y1 = bottom + 1 + offset
                if y1 >= height - 40:
                    continue
                center = (y0 + y1) // 2
                targets.append((f"nav-item+{index}", probe_x, center, (0, y0, 190 if not rail_mode else 56, y1)))

        # 惰性点必须零差异，否则说明这套测量分不出"没变化"。
        move_pointer(base, inert_x, height // 2)
        time.sleep(0.5)
        inert_png = out / "inert.png"
        capture(base, inert_png)
        inert_box = diff_bbox(baseline_a, inert_png)
        if inert_box is not None:
            invalid.append(
                f"惰性点 (x={inert_x}) 与基线有差异 {inert_box}：该处不该有任何反馈。"
                "测量工具分不出'没变化'，整轮判定无效。"
            )
            findings.append({"name": "inert-control", "verdict": "INVALID", "bbox": inert_box})
        else:
            findings.append({"name": "inert-control", "verdict": "PASS", "bbox": None})

        for name, x, y, rect in targets:
            move_pointer(base, x, y)
            # 900ms：越过 tooltip 自身 450ms 的显示延迟，让淡入也走完。
            time.sleep(0.9)
            hover_png = out / f"{name}.png"
            capture(base, hover_png)
            box = diff_bbox(baseline_a, hover_png)
            verdict = "FAIL"
            if box is None:
                failures.append(f"{name}: 悬停 (x={x},y={y}) 前后零差异，反馈没生效")
            elif not intersects(box, rect):
                failures.append(
                    f"{name}: 差异包围盒 {box} 与目标矩形 {rect} 不相交，反馈画在了别处"
                )
            else:
                verdict = "PASS"
            findings.append(
                {"name": name, "x": x, "y": y, "rect": list(rect), "bbox": box, "verdict": verdict}
            )
            move_pointer(base, inert_x, height // 2)
            time.sleep(0.4)
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=10)
        except Exception:
            proc.kill()
        stdout_log.close()
        stderr_log.close()
        kill_product_processes()

    payload = {
        "schema": 1,
        "app": str(app),
        "fixture": args.fixture,
        "theme": args.theme,
        "window": {"width": args.width, "height": args.height},
        "invalid": invalid,
        "failures": failures,
        "findings": findings,
    }
    report_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")

    for item in findings:
        print(f"  {item['name']:<18} {item['verdict']:<8} bbox={item.get('bbox')}")
    print("-" * 78)
    if invalid:
        for line in invalid:
            print(f"INVALID: {line}")
        print("整轮判定无效，不要把它读成通过。")
        return 1
    if failures:
        for line in failures:
            print(f"FAIL: {line}")
        return 1
    print(f"悬停反馈探针通过：{len(findings)} 项，报告 {report_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
