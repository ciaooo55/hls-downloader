# -*- coding: utf-8 -*-
"""标定 /action 的 move 到底把指针放到了哪里。

背景：给夹具采集加了"采图前把指针停到惰性点 (4,4)"之后，tasks_1000 的截图里
冒出一行带悬停底色（surface2 #F5F7FA）的表格行 —— 而 (4,4) 是窗口左上角，
空间上不可能悬停到 y≈320 的表格行。所以在动手改采集脚本之前，先把
"move 的坐标语义"实测清楚，而不是继续猜。

做法：同一个应用进程内，依次把指针移到若干个已知点，每移一次等悬停动画收敛后
截一张图；用相邻两张的差异 bbox 反推"悬停落在屏幕的哪一块"。

用法：
  python calibrate_move_action.py --app <HLSDownloader.exe> --fixture tasks_1000 --theme light
退出码 0 = 标定完成并打印了每步的差异；1 = 应用没起来 / 一次都没截到。
"""
import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request

import numpy as np
from PIL import Image

TOKEN = "hls-visual-capture-20260914"


def post(base, headers, path, payload):
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(base + path, data=data, headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=15) as r:
        return r.read()


def get_bytes(base, headers, path):
    req = urllib.request.Request(base + path, headers=headers)
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.read()


def wait_health(base, headers, seconds=120):
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(urllib.request.Request(base + "/health", headers=headers), timeout=3) as r:
                if r.status == 200:
                    return True
        except Exception:
            pass
        time.sleep(0.3)
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--app", required=True)
    ap.add_argument("--fixture", default="tasks_1000")
    ap.add_argument("--theme", default="light")
    ap.add_argument("--width", type=int, default=1400)
    ap.add_argument("--height", type=int, default=820)
    ap.add_argument("--port", type=int, default=19740)
    ap.add_argument("--out", default=r"C:\hls-visual\calib")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    env = dict(os.environ)
    env.update({
        "HLS_UI_TEST_API": "1",
        "HLS_UI_TEST_TOKEN": TOKEN,
        "HLS_UI_TEST_PORT": str(args.port),
        "HLS_UI_AUDIT_SURFACE": args.fixture,
        "HLS_UI_AUDIT_THEME": args.theme,
        "HLS_UI_AUDIT_WIDTH": str(args.width),
        "HLS_UI_AUDIT_HEIGHT": str(args.height),
        "HLS_V6_SKIP_MIGRATE": "1",
        "HLS_V7_DATA_DIR": os.path.join(args.out, "profile"),
    })
    base = "http://127.0.0.1:%d" % args.port
    headers = {"X-HLS-Test-Token": TOKEN, "Content-Type": "application/json"}

    proc = subprocess.Popen([args.app], cwd=os.path.dirname(args.app), env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        if not wait_health(base, headers):
            print("应用没在时限内就绪")
            return 1
        time.sleep(3.0)

        # 归位点必须满足"同一坐标重复到达 -> 逐像素可复现"。
        # 所以这里刻意把两个候选点各去两次，夹着一次明显会改变悬停的移动：
        # 若某个点的两次结果不一致，它就不是惰性点，不能用来消除悬停污染。
        points = [(4, 4), (4, 600), (300, 340), (4, 600), (4, 4)]
        shots = []
        for i, (x, y) in enumerate(points):
            post(base, headers, "/action", {"type": "move", "x": x, "y": y})
            time.sleep(0.9)  # 悬停动画最长 140ms，900ms 足够收敛
            png = os.path.join(args.out, "p%d_%d_%d.png" % (i, x, y))
            with open(png, "wb") as fh:
                fh.write(get_bytes(base, headers, "/screenshot"))
            # 活性检查是必须的：没有它，"同一坐标两次结果相同"可能只是**两张纯黑图**。
            # 实测踩过——应用在第一次移动后停止渲染，后四张全是纯黑，脚本照样打印
            # "(4,600): 两次逐像素相同 -> 可复现"，把一个死掉的应用判成可复现。
            # 判据不是"两张一样"，而是"两张一样 **且** 都是真实界面"。
            with Image.open(png) as im:
                arr = np.asarray(im.convert("RGB"))
            frac_black = float((arr.sum(axis=2) == 0).mean())
            flat = len(np.unique(arr.reshape(-1, 3), axis=0)) <= 4
            if frac_black > 0.03 or flat:
                print("shot %d 不是真实界面（纯黑占比 %.1f%%，色数 %d）—— 应用可能已经停止渲染，"
                      "本次标定作废" % (i, frac_black * 100, len(np.unique(arr.reshape(-1, 3), axis=0))))
                return 1
            shots.append((png, (x, y)))
            print("shot %d: move(%d,%d) -> %s" % (i, x, y, os.path.basename(png)))

        # 同一坐标重复到达时的可复现性
        print()
        print("== 重复到达同一坐标的可复现性 ==")
        seen = {}
        for png, pt in shots:
            if pt in seen:
                a = np.asarray(Image.open(seen[pt]).convert("RGB")).astype(np.int16)
                b = np.asarray(Image.open(png).convert("RGB")).astype(np.int16)
                m = np.any(a != b, axis=2)
                if m.any():
                    ys, xs = np.where(m)
                    print("  %s: 两次不一致 %d px  bbox=(%d,%d,%d,%d)  -> 不是惰性点"
                          % (pt, int(m.sum()), int(xs.min()), int(ys.min()),
                             int(xs.max() + 1), int(ys.max() + 1)))
                else:
                    print("  %s: 两次逐像素相同 -> 可复现" % (pt,))
            else:
                seen[pt] = png
        print()

        # 相邻两张的差异 bbox：若把指针从 A 移到 B 只改变了悬停态，
        # 差异应该由"A 处掉色 + B 处上色"两块组成。
        for i in range(len(shots) - 1):
            a = np.asarray(Image.open(shots[i][0]).convert("RGB")).astype(np.int16)
            b = np.asarray(Image.open(shots[i + 1][0]).convert("RGB")).astype(np.int16)
            m = np.any(a != b, axis=2)
            if not m.any():
                print("  %s -> %s : 无差异" % (shots[i][1], shots[i + 1][1]))
                continue
            ys, xs = np.where(m)
            print("  %s -> %s : %d px  bbox=(%d,%d,%d,%d)"
                  % (shots[i][1], shots[i + 1][1], int(m.sum()),
                     int(xs.min()), int(ys.min()), int(xs.max() + 1), int(ys.max() + 1)))
            # 分两簇：按 y 把差异行聚类，能看出"掉了哪一行、上了哪一行"
            rows = sorted(set(ys.tolist()))
            groups = []
            start = prev = rows[0]
            for y in rows[1:]:
                if y == prev + 1:
                    prev = y
                    continue
                groups.append((start, prev))
                start = prev = y
            groups.append((start, prev))
            for y0, y1 in groups:
                sub = m[y0:y1 + 1]
                gy, gx = np.where(sub)
                print("      行带 y %d..%d  x %d..%d  %d px"
                      % (y0, y1, int(gx.min()), int(gx.max()), int(sub.sum())))
        return 0
    finally:
        try:
            proc.kill()
        except Exception:
            pass


if __name__ == "__main__":
    sys.exit(main())
