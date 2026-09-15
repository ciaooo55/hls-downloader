# -*- coding: utf-8 -*-
"""把多批采集目录合并成一份完整的 24 夹具采集集。

为什么需要它：单条命令有约 120 秒硬上限，而 24 个夹具 × 每个约 15~20 秒的
应用重启，一轮完整采集必然要拆成 3~4 批。但 `compare_captures.py` 只接受
**一个** new_dir，且它按 `capture-report.json` 做集合对账 —— 传半个目录进去，
"未解释差异 0" 是在一个残缺集合上算出来的，什么也证明不了。

合并规则里最关键的一条：**按报告行复制 PNG，而不是按目录里的 *.png 列表复制**。
基线目录 `compose-visual-all/` 里就躺着一个反面教材 ——
`tasks_1000,settings,...,cast-light,dark-1400x820.png`，是某次参数被逗号拼成
单个字符串后采出来的垃圾文件，它没有报告行。如果按目录列表复制，这种孤儿文件
会悄悄传进新集合；而 `compare_captures.py` 对"PNG 没有报告行"是硬失败的，
于是它会把一轮本来干净的采集判成失败。反过来，报告里有行却没有 PNG，
那是采集中断，同样必须硬失败 —— 两个方向都要查。

退出码：
  0  合并成功，且输出集合的报告行数与 PNG 数逐一对齐
  1  任何一条前置条件不成立（缺报告 / 空报告 / 键冲突 / app_path 不一致 /
     报告行指向的 PNG 不存在 / 输出目录残留旧 PNG）
"""
import argparse
import json
import os
import shutil
import sys
from pathlib import Path

REPORT = "capture-report.json"


def key_of(row):
    return "%s-%s-%dx%d" % (row.get("fixture"), row.get("theme"), row.get("width"), row.get("height"))


def load_one(directory):
    """-> (rows, app_path, problems)。缺报告或空报告一律是硬失败，不是跳过。"""
    problems = []
    path = directory / REPORT
    if not path.exists():
        return [], None, ["%s 里没有 %s —— 这一批的夹具原点无法校验" % (directory, REPORT)]
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except ValueError as exc:
        return [], None, ["%s 不是合法 JSON：%s" % (path, exc)]
    rows = data if isinstance(data, list) else data.get("results", [])
    if not rows:
        return [], data.get("app_path"), ["%s 里有 0 条 results —— 这一批什么都没采到" % path]
    declared_total = data.get("total")
    if isinstance(declared_total, int) and declared_total != len(rows):
        problems.append("%s 自称 total=%d，实际 %d 条 —— 采集可能中途中断"
                        % (path, declared_total, len(rows)))
    declared_captured = data.get("captured")
    n_captured = sum(1 for r in rows if r.get("status") == "captured")
    if isinstance(declared_captured, int) and declared_captured != n_captured:
        problems.append("%s 自称 captured=%d，实际 %d 条" % (path, declared_captured, n_captured))
    return rows, data.get("app_path"), problems


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+", help="要合并的采集目录（按顺序）")
    ap.add_argument("--out", required=True, help="输出目录")
    args = ap.parse_args()

    dirs = [Path(d) for d in args.dirs]
    out = Path(args.out)

    problems = []
    rows_all = []
    app_paths = set()
    seen = {}

    for d in dirs:
        rows, app_path, probs = load_one(d)
        problems += probs
        if app_path:
            app_paths.add(app_path)
        for row in rows:
            k = key_of(row)
            if k in seen:
                # 同一个夹具被采了两次：静默去重会掩盖"某一批的参数写错了"，
                # 所以这里直接判失败，让人去看是哪两批撞了。
                problems.append("夹具键冲突：%s 同时出现在 %s 和 %s" % (k, seen[k], d))
                continue
            seen[k] = d
            # 报告里的 png 是**采集当时**写下的绝对路径。照抄它去读文件有一个很隐蔽的坑：
            # 目录一旦被移动或改名（把一批产物挪个地方再合并是很自然的操作），这行就仍然
            # 指向旧位置 —— 合并出来的"新集合"里混着别处的旧图，而且一切校验都通过。
            # 所以这里只认"与本报告同一个目录"的那份文件，路径对不上就硬失败。
            recorded = Path(row.get("png") or "")
            src = d / recorded.name
            if os.path.normcase(str(recorded.parent.resolve())) != os.path.normcase(str(d.resolve())):
                problems.append(
                    "%s 的报告行记录的 PNG 不在本目录内：%s\n"
                    "     （报告是采集时写的，目录移动/改名后这行会指向旧位置；"
                    "请重新采集，或手工修好报告再合并）" % (d, recorded))
                continue
            if not src.exists():
                problems.append("%s 的报告行指向的 PNG 不存在：%s" % (d, src))
                continue
            rows_all.append((row, src))

    if len(app_paths) > 1:
        problems.append("各批的 app_path 不一致，说明不是同一个构建产物的截图：%s"
                        % sorted(app_paths))

    if problems:
        print("合并前的检查未通过，拒绝产出集合：")
        for p in problems:
            print("  !! %s" % p)
        return 1

    if not rows_all:
        print("没有任何一行可合并 —— 空集合是空洞通过，按失败处理")
        return 1

    out.mkdir(parents=True, exist_ok=True)
    # 输出目录必须是干净的：残留的旧 PNG 会让 compare_captures.py 报"没有报告行"，
    # 于是把一次干净的采集判成失败。宁可直接拒绝，也不要产出一个自相矛盾的集合。
    stale = sorted(p.name for p in out.glob("*.png"))
    if stale:
        print("输出目录里已有 %d 个 PNG，先清空再合并（避免新旧混在一起）：" % len(stale))
        for s in stale:
            print("  %s" % s)
        return 1

    merged = []
    for row, src in rows_all:
        dst = out / ("%s.png" % key_of(row))
        shutil.copy2(src, dst)
        row = dict(row)
        row["png"] = str(dst)
        merged.append(row)

    merged.sort(key=key_of)
    report = {
        "schema": 1,
        "app_path": sorted(app_paths)[0],
        "captured": sum(1 for r in merged if r.get("status") == "captured"),
        "total": len(merged),
        "results": merged,
    }
    (out / REPORT).write_text(json.dumps(report, ensure_ascii=False), encoding="utf-8")

    pngs = sorted(p.stem for p in out.glob("*.png"))
    keys = sorted(key_of(r) for r in merged)
    if pngs != keys:
        print("合并后自检失败：PNG 列表与报告行列表不一致")
        print("  只在 PNG 里：%s" % sorted(set(pngs) - set(keys)))
        print("  只在报告里：%s" % sorted(set(keys) - set(pngs)))
        return 1

    print("合并 %d 批 -> %s" % (len(dirs), out))
    print("  夹具 %d 个（captured %d）" % (len(merged), report["captured"]))
    print("  app_path %s" % report["app_path"])
    for k in keys:
        print("    %s" % k)
    return 0


if __name__ == "__main__":
    sys.exit(main())
