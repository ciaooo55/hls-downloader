"""Exercise the current installed workbench and measure real pointer feedback."""
from __future__ import annotations

import argparse
import ctypes
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import threading
import traceback
import time
import uuid
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import psutil
from PIL import Image, ImageChops

from smoke_v7_accessibility import AccessibleContextInfo, JabClient
from smoke_v7_portable_app import Pipe, runtime_processes, wait
from smoke_v7_transfer_performance import free_port


def run(args):
    if args.extras_stage == "destructive":
        assert os.environ.get("HLS_VM_ACCEPTANCE") == "1", "Destructive acceptance is Sandbox-only"
    runtime, evidence = args.runtime.resolve(), args.report.resolve().parent
    evidence.mkdir(parents=True, exist_ok=True)
    report = {"passed": False, "runtime": str(runtime), "checks": [], "controls": {}, "feedback": [],
              "test_script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    report["components"] = {str(p.relative_to(runtime)): hashlib.sha256(p.read_bytes()).hexdigest()
                            for p in [runtime/"app/HLSDownloaderDesktop-7.0.3.jar",
                                      runtime/"app/resources/HLSDownloaderEngine.exe"]}
    normal_env = {k: v for k, v in os.environ.items() if not k.startswith("HLS_UI_")}
    port, token = free_port(), uuid.uuid4().hex
    client = JabClient(runtime / "runtime/bin/windowsaccessbridge-64.dll")
    client.start()
    hwnd = None
    startup_modal_failure = False
    fixture = None
    torrent_peer = None
    drop_helper = None
    created_tasks = set()
    settings_to_restore = {}

    def core(message):
        p = Pipe()
        try:
            return p.request(message)
        finally:
            p.close()

    def save():
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf8")

    def record(name, actual=True):
        report["checks"].append({"name": name, "actual": actual})
        save()
        print(json.dumps({"check": name}, ensure_ascii=True), flush=True)

    def stop_ui():
        running = []
        for info in runtime_processes(runtime):
            if info["name"] == "HLSDownloader.exe":
                try:
                    p = psutil.Process(info["pid"])
                    p.terminate()
                    running.append(p)
                except psutil.NoSuchProcess:
                    pass
        _, alive = psutil.wait_procs(running, timeout=10)
        if alive:
            raise RuntimeError("Installed workbench did not stop")

    def ui(path, payload=None, raw=False, timeout=15):
        data = json.dumps(payload).encode() if payload is not None else None
        request = Request(f"http://127.0.0.1:{port}/{path}", data=data,
                          headers={"X-HLS-Test-Token": token, "Content-Type": "application/json"})
        try:
            with urlopen(request, timeout=timeout) as response:
                body = response.read()
        except HTTPError as error:
            detail = error.read().decode("utf8", "replace").strip()
            raise RuntimeError(f"UI {path} HTTP {error.code}: {detail}") from error
        return body if raw else json.loads(body)

    def action(kind, timeout=15, **values):
        ui("action", {"type": kind, **values}, timeout=timeout)

    def tree(stop_when=None):
        client.pump_messages()
        vm, root = client.context_from_window(hwnd)
        try:
            return client.walk(vm, root, 5000, stop_when=stop_when)
        finally:
            client.dll.releaseJavaObject(vm, root)

    node_paths = {}

    def node(name, actionable=False, index=0):
        key = (name, actionable, index)
        def find():
            path = node_paths.get(key)
            if path is not None:
                client.pump_messages()
                vm, root = client.context_from_window(hwnd)
                owned = []
                try:
                    context, owned = client.resolve_path(vm, root, path)
                    info = AccessibleContextInfo()
                    if client.dll.getAccessibleContextInfo(vm, context, ctypes.byref(info)):
                        actions = client.actions(vm, context) if info.accessible_action else []
                        if (info.name == name if actionable else name in info.name) and (not actionable or actions):
                            return {"name": info.name, "role": info.role_en_US or info.role,
                                    "states": info.states_en_US or info.states,
                                    "bounds": [info.x, info.y, info.width, info.height], "actions": actions, "path": path}
                except RuntimeError:
                    # 切换页签和弹窗会重建树；只缓存路径，每次都读取当前控件状态。
                    pass
                finally:
                    for context in reversed(owned):
                        client.dll.releaseJavaObject(vm, context)
                    client.dll.releaseJavaObject(vm, root)
                node_paths.pop(key, None)
            matches = []
            def found(n):
                if (n["name"] == name if actionable else name in n["name"]) and (not actionable or n["actions"]):
                    matches.append(n)
                return index >= 0 and len(matches) > index
            tree(stop_when=found)
            enough = len(matches) > index if index >= 0 else len(matches) >= -index
            if enough:
                node_paths[key] = matches[index]["path"]
                return matches[index]
            return None
        return wait(find, "Missing installed UI control: " + name, 12)

    def click(name, index=0):
        if name in {"批量添加", "页面抓取", "浏览器插件"}:
            click("工具")
        target = node(name, True, index)
        if current_context.startswith("main"):
            categories = ("媒体", "程序", "压缩包", "其他")
            statuses = ("全部", "进行中", "排队中", "已暂停", "已完成", "失败")
            section = ("categories", 3) if target["name"].startswith(categories) else (
                ("statuses", -3) if target["name"].startswith(statuses) else None)
            if section:
                direction = section[1]
                window = ui("window")
                scroll_x = round(window["deviceWidth"] * .075)
                scroll_y = window["deviceHeight"] // 2
                for _ in range(12):
                    x, y, width, height = target["bounds"]
                    local_y = y - window["deviceY"]
                    if (width > 0 and height > 0 and "showing" in target["states"] and
                            0 <= local_y and local_y + height <= window["deviceHeight"] - 40):
                        break
                    action("scroll", x=scroll_x, y=scroll_y, delta=direction)
                    time.sleep(.2)
                    target = node(name, True, index)
        if current_context.startswith("settings-") and (
                target["role"] == "page tab" or
                target["role"] in ("check box", "toggle button") or
                target["role"] == "push button" and name not in ("关闭", "取消", "保存设置")):
            target_path = target["path"]
            scroll_pane = next(
                n for n in tree()
                if n["role"] == "scroll pane" and "showing" in n["states"] and
                target_path[:len(n["path"])] == n["path"])
            pane_x, pane_y, pane_width, pane_height = scroll_pane["bounds"]
            window = ui("window")
            scroll_x = pane_x + pane_width // 2 - window["deviceX"]
            scroll_y = pane_y + pane_height // 2 - window["deviceY"]
            for _ in range(12):
                x, y, width, height = target["bounds"]
                if (width > 0 and height > 0 and "showing" in target["states"] and y >= pane_y and
                        y + height <= pane_y + pane_height):
                    break
                action("scroll", x=scroll_x, y=scroll_y,
                       delta=-3 if y < pane_y else 3)
                time.sleep(.2)
                target = node(name, True, index)
            else:
                raise AssertionError("Settings control remains clipped: " + name)
            assert "showing" in target["states"], "Settings control is not visible: " + name
        x, y, width, height = target["bounds"]
        assert width > 0 and height > 0, "Cannot click a clipped control: " + name
        window = ui("window")
        started=time.perf_counter()
        action("click", x=x+width//2-window["deviceX"], y=y+height//2-window["deviceY"])
        time.sleep(.2)
        report["controls"].setdefault(f'{current_context}/{name}', {})["clicked"] = True
        save()
        return started

    def observe(context):
        nonlocal current_context
        current_context = context
        controls = []
        for n in tree():
            if n["actions"] and "showing" in n["states"]:
                entry = {"role": n["role"], "bounds": n["bounds"], "enabled": "enabled" in n["states"]}
                key = context + "/" + n["name"]
                previous = report["controls"].get(key, {})
                report["controls"][key] = {**previous, **entry}
                controls.append(n["name"])
        (evidence / (context.replace("/", "-") + ".png")).write_bytes(ui("screenshot", raw=True))
        save()
        return controls

    def focus_text_field():
        target = next(n for n in tree() if n["role"] == "text" and "editable" in n["states"] and "showing" in n["states"])
        x,y,w,h = target["bounds"]
        window = ui("window")
        action("click",x=x+w//2-window["deviceX"],y=y+h//2-window["deviceY"])

    def feedback(control, label, selected=False):
        controls = ui("state")["controlBounds"]
        bounds = controls.get(control)
        if not bounds:
            raise AssertionError("Control has no current bounds: " + control)
        if control.startswith("settings."):
            footer = controls.get("dialog.primary.保存设置")
            if footer:
                for _ in range(8):
                    if bounds[3] < footer[1] - 30:
                        break
                    action("scroll", x=(bounds[0]+bounds[2])//2, y=footer[1]-120, delta=3)
                    time.sleep(.2)
                    bounds = ui("state")["controlBounds"][control]
                assert bounds[3] < footer[1] - 30, "Settings control remains clipped: " + control
        left, top, right, bottom = bounds
        x, y = (left + right) // 2, (top + bottom) // 2
        assert right > left and bottom > top
        action("move", x=1, y=1)
        time.sleep(.18)
        baseline = Image.open(io.BytesIO(ui("screenshot", raw=True))).convert("RGB")
        action("move", x=x, y=y)
        time.sleep(.22)
        hover = Image.open(io.BytesIO(ui("screenshot", raw=True))).convert("RGB")
        action("press", x=x, y=y)
        time.sleep(.18)
        pressed = Image.open(io.BytesIO(ui("screenshot", raw=True))).convert("RGB")
        # 移出后抬起，测按下反馈而不触发可能删除任务或保存设置的动作。
        action("move", x=1, y=1)
        action("release")
        window = ui("window")
        scale_x = baseline.width / window["deviceWidth"]
        scale_y = baseline.height / window["deviceHeight"]
        region = tuple(round(value * (scale_x if i % 2 == 0 else scale_y))
                       for i, value in enumerate(bounds))
        def pixels(a, b):
            diff = ImageChops.difference(a.crop(region), b.crop(region))
            return sum(any(channel > 2 for channel in pixel) for pixel in diff.get_flattened_data())
        result = {"control": control, "label": label, "bounds": bounds,
                  "selected_segment": selected,
                  "hover_changed_pixels": pixels(baseline, hover),
                  "press_changed_pixels": pixels(hover, pressed)}
        filename = f"feedback-{len(report['feedback']):03d}"
        crop_width, crop_height = region[2]-region[0], region[3]-region[1]
        strip = Image.new("RGB", (crop_width, crop_height*3))
        for i, frame in enumerate((baseline, hover, pressed)):
            strip.paste(frame.crop(region), (0, crop_height*i))
        strip.save(evidence / (filename + ".png"))
        result["image"] = filename + ".png"
        report["feedback"].append(result)
        save()
        if (not selected and not result["hover_changed_pixels"]) or not result["press_changed_pixels"]:
            raise AssertionError("Missing pointer feedback: " + json.dumps(result))

    def probe_context(context, prefixes):
        observe(context)
        for name in list(ui("state")["controlBounds"]):
            if any(name.startswith(prefix) for prefix in prefixes):
                if "primary" in name:
                    # 空表单主动作应禁用，禁用态不应伪造悬停、按压反馈。
                    continue
                selected = name in ("newtask.tab." + context.removeprefix("newtask-"), "newtask.method.GET")
                if name.startswith("settings.day."):
                    selected = name.rsplit(".", 1)[1] in original_settings["queue_active_days"].split(",")
                if name.startswith("settings.choice."):
                    field = {"settings-下载": "existing_file_policy", "settings-计划": "completion_power_action", "settings-网络": "proxy_mode"}.get(context)
                    selected = bool(field) and name == "settings.choice." + original_settings[field]
                feedback(name, context, selected=selected)
        record("pointer feedback: " + context)

    current_context = "main"
    original_settings = None
    try:
        p = Pipe()
        original_settings = p.request({"type": "load_settings", "request_id": 1})
        initial_browser_connected = p.request({"type":"snapshot", "request_id":2}).get("browser_connected", False)
        p.close()
        (evidence / "settings-before.json").write_text(json.dumps(original_settings), encoding="utf8")
        stop_ui()
        env = {**normal_env, "HLS_UI_TEST_API": "1", "HLS_UI_TEST_PORT": str(port), "HLS_UI_TEST_TOKEN": token}
        with (evidence / "installed-ui.log").open("wb") as log:
            subprocess.Popen([str(runtime / "HLSDownloader.exe")], cwd=runtime, env=env, stdout=log, stderr=log)
        def ready():
            try:
                return "已连接" in ui("state").get("engineText", "")
            except OSError:
                return False
        wait(ready, "Installed UI did not connect", 90)
        hwnd, _ = client.find_window("HLS Downloader", 15)
        startup_modal_prefixes = (
            "dialog.", "newtask.", "batch.", "harvest.", "settings.", "device.",
            "queue.", "extension.", "torrent.", "duplicate.", "update.",
        )
        startup_state = ui("state")
        modal_deadline = time.monotonic() + 8
        while not any(
            name.startswith(startup_modal_prefixes)
            for name in startup_state["controlBounds"]
        ) and time.monotonic() < modal_deadline:
            time.sleep(.25)
            startup_state = ui("state")
        startup_modal_controls = {
            name: bounds for name, bounds in startup_state["controlBounds"].items()
            if name.startswith(startup_modal_prefixes)
        }
        if startup_modal_controls:
            startup_tree = []
            try:
                startup_tree = tree()
            except Exception as error:
                startup_tree_error = str(error)
            else:
                startup_tree_error = None
            update_dialog = (
                set(startup_modal_controls) == {
                    "dialog.secondary.稍后",
                    "dialog.secondary.查看发布页",
                    "dialog.primary.下载更新",
                }
            )
            if update_dialog:
                bounds = startup_modal_controls["dialog.secondary.稍后"]
                action("click", x=(bounds[0] + bounds[2]) // 2, y=(bounds[1] + bounds[3]) // 2)
                wait(
                    lambda: not any(
                        name.startswith(startup_modal_prefixes)
                        for name in ui("state")["controlBounds"]
                    ),
                    "Expected update notice did not close after clicking 稍后",
                    10,
                )
                report["dismissed_startup_update"] = {
                    "title": "发现新版本",
                    "button": "稍后",
                    "download_requested": False,
                }
                save()
                record("dismiss expected startup update notice without downloading")
            else:
                from PIL import ImageGrab
                before_path = evidence / "startup-before.png"
                ImageGrab.grab(all_screens=True).save(before_path)
                diagnostic = {
                    "title_candidates": [
                        item["name"] for item in startup_tree
                        if item["name"] and item["role"] == "label" and
                        any(marker in item["name"] for marker in (
                            "选择", "新建", "设置", "导入", "导出", "批量", "抓取", "确认", "设备", "队列",
                        ))
                    ],
                    "control_bounds": startup_modal_controls,
                    "screenshot": before_path.name,
                    "accessibility_tree": startup_tree,
                }
                if startup_tree_error:
                    diagnostic["accessibility_error"] = startup_tree_error
                report["unexpected_startup_modal"] = diagnostic
                save()
                startup_modal_failure = True
                raise AssertionError(
                    "Unexpected modal after workbench startup: " +
                    ", ".join(diagnostic["title_candidates"] or startup_modal_controls)
                )
        node("新建", True)
        # 前一组隔离浏览器已经退出时，旧租约可在 UI 启动期间到期；只验证仍在线的客户端。
        if initial_browser_connected and core({"type":"snapshot", "request_id":2}).get("browser_connected", False):
            node("浏览器插件 · 已连接")
            record("workbench restart immediately restores current browser connection from snapshot")
        report["window"] = ui("window")
        report["start_at"] = args.start_at
        if args.start_at == "hover":
            from smoke_v7_presenter_visual import user32
            from PIL import ImageGrab, ImageStat
            settings_to_restore["dark_mode"] = original_settings["dark_mode"]
            transitions = []
            for dark in (False, True):
                wanted = "浅色模式" if dark else "深色模式"
                if not any(n["name"] == wanted for n in tree()):
                    click("深色模式" if dark else "浅色模式")
                observe("hover-dark" if dark else "hover-light")
                window = ui("window")
                controls = ui("state")["controlBounds"]
                names = [name for name in controls if name.startswith("taskrow.")]
                names += [name for name in ("sidebar.status.进行中", "toolbar.设置", "toolbar.刷新") if name in controls]
                for name in names:
                    left, top, right, bottom = controls[name]
                    # 取内部空白底色，排除文字、描边和圆角。
                    region = (window["deviceX"] + left + (right-left)//3, window["deviceY"] + top + 5,
                              window["deviceX"] + right - (right-left)//3, window["deviceY"] + top + 8)
                    x, y = window["deviceX"] + (left+right)//2, window["deviceY"] + (top+bottom)//2
                    away = (window["deviceX"] + 1, window["deviceY"] + 1)
                    user32.SetCursorPos(*away)
                    time.sleep(.3)
                    frames = [(0.0, ImageGrab.grab(bbox=region).convert("RGB"))]
                    started = time.perf_counter()
                    user32.SetCursorPos(x, y)
                    while time.perf_counter() - started < .32:
                        frames.append((time.perf_counter()-started, ImageGrab.grab(bbox=region).convert("RGB")))
                        time.sleep(.008)
                    time.sleep(.1)
                    frames.append((time.perf_counter()-started, ImageGrab.grab(bbox=region).convert("RGB")))
                    user32.SetCursorPos(*away)
                    started = time.perf_counter()
                    while time.perf_counter() - started < .32:
                        frames.append((time.perf_counter()-started+.5, ImageGrab.grab(bbox=region).convert("RGB")))
                        time.sleep(.008)
                    colors = [ImageStat.Stat(frame).mean for _, frame in frames]
                    endpoints = [colors[0], colors[next(i for i, (t, _) in enumerate(frames) if t >= .4)]]
                    low = [min(c[channel] for c in endpoints) for channel in range(3)]
                    high = [max(c[channel] for c in endpoints) for channel in range(3)]
                    overshoot = max(max(low[channel]-color[channel], color[channel]-high[channel], 0)
                                    for color in colors for channel in range(3))
                    result = {"control": name, "dark": dark, "samples": [
                        {"time": t, "rgb": color} for (t, _), color in zip(frames, colors)],
                        "max_channel_overshoot": overshoot}
                    strip = Image.new("RGB", (frames[0][1].width, frames[0][1].height*len(frames)))
                    for i, (_, frame) in enumerate(frames):
                        strip.paste(frame, (0, i*frame.height))
                    result["image"] = f"{args.report.stem}-hover-timeline-{len(transitions):02d}.png"
                    strip.save(evidence / result["image"])
                    transitions.append(result)
                    report["hover_transitions"] = transitions
                    save()
            assert transitions and all(t["max_channel_overshoot"] <= 3 for t in transitions), "Hover color briefly overshoots its resting/hover endpoints"
            record("real-pointer-hover-enter-and-leave-without-color-flash", len(transitions))
        if args.start_at in ("main", "feedback"):
            observe("main-light")
            for control in list(ui("state")["controlBounds"]):
                if control.startswith("toolbar."):
                    feedback(control, "main-light")
            record("all toolbar controls: light-theme bounds, hover, press")

            for name in ("进行中", "排队中", "已暂停", "已完成", "失败", "全部", "媒体", "程序", "压缩包", "其他", "全部"):
                target = next(n for n in tree() if n["actions"] and (n["name"] == name or n["name"].startswith(name + ",")))
                click(target["name"])
                state = ui("state")
                record("sidebar click: " + name, {k: state[k] for k in ("activeFilter", "activeCategory", "activeQueueId")})
            click("刷新")
            record("refresh keeps workbench connected", ui("state")["engineText"])
            click("深色模式")
            observe("main-dark")
            for control in list(ui("state")["controlBounds"]):
                if control.startswith("toolbar."):
                    feedback(control, "main-dark")
            click("浅色模式")
            record("all toolbar controls: dark-theme feedback, theme restored")

        if args.start_at in ("main", "newtask"):
            click("新建")
            node("新建下载")
            for tab in ("基本", "连接", "请求", "计划"):
                click(tab)
                probe_context("newtask-" + tab, ("dialog.secondary.", "newtask.tab.", "newtask.method."))
                if tab == "请求":
                    for method in ("POST", "HEAD", "GET"):
                        click(method)
                        record("request method click: " + method)
                if tab == "计划":
                    for option in ("睡眠", "休眠", "关机", "无"):
                        click(option)
                        record("completion draft option: " + option)
            click("取消")
            record("new-task cancel removes dialog")

            for button, expected in (("批量添加", "链接列表"), ("页面抓取", "抓取本页链接"), ("浏览器插件", "浏览器插件")):
                click(button)
                node(expected)
                probe_context("dialog-" + button, ("dialog.secondary.",))
                click("取消" if button != "浏览器插件" else "完成", -1)
                record("dialog open / cancel: " + button)

            click("管理队列")
            node("保存队列")
            observe("queue-manager")
            click("新建队列")
            node("删除队列", True)
            for label in ("上移队列", "下移队列", "删除队列"):
                n = node(label, True)
                if "enabled" in n["states"]:
                    click(label)
                    record("queue-manager draft action: " + label)
            cancel_bounds = wait(
                lambda: ui("state")["controlBounds"].get("dialog.secondary.取消"),
                "Queue manager cancel bounds are missing", 5)
            action("click", x=(cancel_bounds[0]+cancel_bounds[2])//2,
                   y=(cancel_bounds[1]+cancel_bounds[3])//2)
            wait(lambda: not any(n["name"] == "保存队列" and "showing" in n["states"] for n in tree()),
                 "Queue manager did not close after cancel", 5)

        if args.start_at in ("main", "settings"):
            click("设置")
            node("保存设置")
            started_settings = args.settings_start_tab == "通用"
            for tab, heading in (("通用", "文件与运行"), ("下载", "默认并发"), ("计划", "限速与队列计划"),
                                 ("网络", "代理与站点"), ("媒体", "媒体处理与直播"), ("BT", "BT 与种子"),
                                 ("安全", "发布与扫描"), ("浏览器", "浏览器下载接管"), ("外观", "外观与可访问性")):
                started_settings = started_settings or tab == args.settings_start_tab
                if not started_settings:
                    continue
                current_context = "settings-" + tab
                click(tab)
                node(heading)
                probe_context(current_context, ("dialog.secondary.", "settings.choice.", "settings.day."))
                toggles = [n["name"] for n in tree() if n["actions"] and n["role"] in ("check box", "toggle button")
                           and "enabled" in n["states"] and "showing" in n["states"]]
                for label in dict.fromkeys(toggles):
                    before = node(label, True)["states"]
                    click(label)
                    after = wait(lambda: (states if ("checked" in before) != ("checked" in (states := node(label, True)["states"])) else None),
                                 "Settings switch did not change: " + label, 5)
                    click(label)
                    wait(lambda: ("checked" in node(label, True)["states"]) == ("checked" in before),
                         "Settings switch did not restore: " + label, 5)
                    record("settings switch toggles and restores: " + tab + "/" + label)
                for label in [n["name"] for n in tree() if n["actions"] and n["role"] == "push button"
                              and "showing" in n["states"] and n["name"] in
                              ("不限速", "继承", "关闭", "手动", "系统", "DIRECT", "自动", "无", "全部", "顺序", "随机")]:
                    if label != "关闭":
                        click(label)
                        record("settings choice: " + tab + "/" + label)
                if tab == "网络":
                    click("添加规则")
                    observe("settings-site-rule")
                    # 新规则为空时保存按钮应禁用，取消应丢弃草稿。
                    assert "enabled" not in node("保存设置", True)["states"]
                    record("empty site rule disables save")
            click("取消")
            record("settings cancel preserves existing configuration")
        if args.start_at == "functional":
            from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, RangeHandler, Server
            token_url = uuid.uuid4().hex
            resume_gate = threading.Event()
            resume_gate_reached = threading.Event()
            class Handler(RangeHandler):
                def do_GET(self):
                    if self.path == f"/controls-{token_url}.bin?resume=1":
                        start, end, status = 0, PAYLOAD_SIZE - 1, 200
                        value = self.headers.get("Range", "")
                        if value.startswith("bytes="):
                            first, _, last = value[6:].partition("-")
                            start = int(first or 0)
                            end = min(int(last) if last else PAYLOAD_SIZE - 1, PAYLOAD_SIZE - 1)
                            status = 206
                        self._headers(status, start, end)
                        remaining, offset, sent = end - start + 1, start, 0
                        while remaining:
                            size = min(remaining, len(BLOCK))
                            pos = offset % len(BLOCK)
                            chunk = (BLOCK[pos:] + BLOCK[:pos])[:size]
                            try:
                                self.wfile.write(chunk)
                                self.wfile.flush()
                            except (BrokenPipeError, ConnectionResetError):
                                return
                            remaining -= size
                            offset += size
                            sent += size
                            if sent >= 512 * 1024 and not resume_gate.is_set():
                                resume_gate_reached.set()
                                resume_gate.wait()
                            time.sleep(0.01)
                        return
                    if self.path == "/page-" + token_url:
                        body = (f'<a href="/controls-{token_url}.zip?harvest=1">zip</a>'
                                f'<a href="/controls-{token_url}.bin?harvest=2">file</a>').encode()
                        self.send_response(200)
                        self.send_header("Content-Type", "text/html")
                        self.send_header("Content-Length", str(len(body)))
                        self.end_headers()
                        self.wfile.write(body)
                    else:
                        super().do_GET()
            fixture = Server(("127.0.0.1", 0), Handler)
            threading.Thread(target=fixture.serve_forever, daemon=True).start()
            url = f"http://127.0.0.1:{fixture.server_port}/controls-{token_url}.bin"
            initial = {t["task_id"] for t in core({"type": "snapshot", "request_id": 2})["tasks"]}

            def tasks():
                all_tasks = core({"type": "snapshot", "request_id": 3})["tasks"]
                ours = [t for t in all_tasks if t["task_id"] not in initial and token_url in t["url"]]
                created_tasks.update(t["task_id"] for t in ours)
                return ours

            def pointer(control):
                state = ui("state")
                bounds = state["controlBounds"][control]
                left, top, right, bottom = bounds
                point = [(left+right)//2, (top+bottom)//2]
                action("click", x=point[0], y=point[1])
                return {"state": state, "bounds": bounds, "point": point}

            def enter(text, replace=False):
                if replace:
                    action("key", key="A", modifiers=["CTRL"])
                for index, line in enumerate(text.split("\n")):
                    if index:
                        action("key", key="ENTER")
                    if line:
                        action("type", text=line)

            if args.functional_stage == "all":
                click("新建")
                node("等待输入")
                assert "enabled" not in node("创建下载", True)["states"]
                pointer("newtask.url")
                enter("javascript:alert(1)")
                node("链接格式无效或 cURL 命令不完整")
                assert "enabled" not in node("创建下载", True)["states"]
                action("key", key="ENTER")
                pointer("dialog.primary.创建下载")
                node("链接格式无效或 cURL 命令不完整")
                assert len(core({"type": "snapshot", "request_id": 31})["tasks"]) == len(initial), "Disabled submit created a task"
                record("invalid URL blocks both Enter and clicking the disabled submit button")
                pointer("newtask.url")
                enter(url, replace=True)
                node("类型：file")
                click("分析资源")
                # 分析期间原新建窗口仍有可用的提交按钮；等结果窗口再点，避免点击旧布局。
                wait(lambda: "newtask.url" not in ui("state")["controlBounds"], "Probe did not replace new task dialog", 90)
                node("选择清晰度、音轨或直接使用自动选择")
                wait(lambda: "enabled" in node("创建下载", True)["states"], "URL analysis did not finish", 30)
                observe("newtask-valid-analysis")
                click("创建下载")
                downloaded = wait(lambda: next((t for t in tasks() if t["status"] == "completed"), None),
                                  "GUI-created download did not complete", 90)
                output = Path(downloaded["output_path"])
                expected = hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest()
                assert output.stat().st_size == PAYLOAD_SIZE and hashlib.sha256(output.read_bytes()).hexdigest() == expected
                record("new-task invalid input, disabled submit, analyze, actual 32MiB download and SHA256")

                row = "taskrow." + downloaded["task_id"]
                wait(lambda: row in ui("state")["controlBounds"], "New task not rendered", 15)
                wait(lambda: any(downloaded['filename'] in n['name'] and '已完成' in n['name'] for n in tree()),
                     "Workbench did not render completed download", 30)
                pointer(row)
                wait(lambda: downloaded["task_id"] in ui("state")["selectedTaskIds"], "Row click did not select task", 10)
                record("real task row click selects downloaded task")
                left, top, right, bottom = ui("state")["controlBounds"][row]
                action("right_click", x=(left+right)//2, y=(top+bottom)//2)
                menu = wait(lambda: ui("state")["contextMenuActions"], "Task menu did not open", 10)
                record("real task right-click exposes legal actions", menu)
                selected_before_escape = ui("state")["selectedTaskIds"]
                action("key", key="ESCAPE")
                wait(lambda: not ui("state")["contextMenuActions"], "Escape left task menu open", 10)
                assert ui("state")["selectedTaskIds"] == selected_before_escape, "Menu Escape cleared selection"
                record("Escape dismisses task menu without clearing selection")
                left, top, right, bottom = ui("state")["controlBounds"][row]
                action("right_click", x=(left+right)//2, y=(top+bottom)//2)
                wait(lambda: ui("state")["contextMenuActions"], "Task menu did not reopen", 10)
                click("详情与日志")
                node("任务详情")
                for tab in ("概览", "连接", "速度", "日志"):
                    click(tab)
                    observe("task-details-" + tab)
                    record("task details tab: " + tab)
                click("关闭", -1)
                wait(lambda: not ui("state")["contextMenuActions"], "Task details left context menu open", 10)

            else:
                response = core({"type":"command","request_id":32,"command":{"kind":"create_task","spec":{
                    "url":url,"resource_kind":"file","filename":"controls-"+token_url+".bin","concurrency":4}}})
                downloaded = next(e["event"]["snapshot"] for e in response["events"] if e["event"]["kind"]=="task_created")
                created_tasks.add(downloaded["task_id"])
                core({"type":"command","request_id":33,"command":{"kind":"task_action","task_id":downloaded["task_id"],"action":"start"}})
                downloaded = wait(lambda: next((t for t in tasks() if t["task_id"]==downloaded["task_id"] and t["status"]=="completed"),None),"Prepare UI fixture",90)
                row = "taskrow."+downloaded["task_id"]
                wait(lambda: row in ui("state")["controlBounds"],"Fixture row not rendered",15)
                expected = hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest()
                record("prepared completed download fixture for remaining UI actions")

            if args.functional_stage in ("all", "resume"):
                # 暂停与继续通过当前任务工具栏真实点击，任务来自同一安装程序的下载引擎。
                response = core({"type": "command", "request_id": 4, "command": {"kind": "create_task", "spec": {
                    "url": url+"?resume=1", "resource_kind": "file", "filename": "resume-"+token_url+".bin", "concurrency": 1,
                    "speed_limit_kib": 1024}}})
                queued = next(e["event"]["snapshot"] for e in response["events"] if e["event"]["kind"] == "task_created")
                created_tasks.add(queued["task_id"])
                wait(lambda: "taskrow."+queued["task_id"] in ui("state")["controlBounds"], "Queued task not rendered", 15)
                # 等行的语义和布局同时就绪；新建后的首帧位置不能当作稳定点击坐标。
                queued_row = node(queued["filename"])
                x, y, width, height = queued_row["bounds"]
                window = ui("window")
                action("click", x=x+width//3-window["deviceX"], y=y+height//2-window["deviceY"])
                wait(lambda: queued["task_id"] in ui("state")["selectedTaskIds"], "Queued row was not selected", 10)
                pointer("toolbar.全部开始")
                wait(lambda: any(t["task_id"] == queued["task_id"] and t["downloaded_bytes"] > 262144 for t in tasks()),
                     "Start button did not begin transfer", 20)
                wait(resume_gate_reached.is_set, "Resume fixture did not reach its pause gate", 15)
                running_ui = wait(
                    lambda: (state if "toolbar.暂停" in (state := ui("state"))["controlBounds"] else None),
                    "UI did not expose pause for the selected running task", 15)
                pause_before = next(t for t in tasks() if t["task_id"] == queued["task_id"])
                assert 0 < pause_before["downloaded_bytes"] < PAYLOAD_SIZE, pause_before
                pause_click = pointer("toolbar.全部暂停")
                report["pause_diagnostic"] = {
                    "ui_running_task": running_ui,
                    "core_before_click": pause_before,
                    "ui_at_click": pause_click["state"],
                    "click": {"control": "toolbar.全部暂停", "bounds": pause_click["bounds"], "point": pause_click["point"]},
                    "core_after_click": next(t for t in tasks() if t["task_id"] == queued["task_id"]),
                    "ui_after_click": ui("state"),
                }
                save()
                wait(lambda: any(t["task_id"] == queued["task_id"] and t["status"] == "paused" and t["active_workers"] == 0 for t in tasks()),
                     "Pause button did not settle", 20)
                report["pause_diagnostic"]["core_after_settle"] = next(t for t in tasks() if t["task_id"] == queued["task_id"])
                report["pause_diagnostic"]["ui_after_settle"] = ui("state")
                save()
                resume_gate.set()
                assert next(t for t in tasks() if t["task_id"] == downloaded["task_id"])["status"] == "completed", "All-start restarted a completed download"
                if queued["task_id"] not in ui("state")["selectedTaskIds"]:
                    pointer("taskrow."+queued["task_id"])
                wait(lambda: "toolbar.开始" in ui("state")["controlBounds"], "Selected paused task has no start toolbar", 10)
                pointer("toolbar.开始")
                resumed = wait(lambda: next((t for t in tasks() if t["task_id"] == queued["task_id"] and t["status"] == "completed"), None),
                               "Resume button did not complete", 90)
                assert hashlib.sha256(Path(resumed["output_path"]).read_bytes()).hexdigest() == expected
                record("all-start, all-pause, selected-task resume, checkpoint and SHA256")

            if args.functional_stage != "queue":
                click("批量添加")
                node("链接列表")
                focus_text_field()
                enter(url+"?batch=1\ninvalid input\n"+url+"?batch=2")
                node("第 2 行链接无效，请修正后创建")
                assert "enabled" not in node("添加 3 条链接", True)["states"]
                assert not any("batch=" in t["url"] for t in tasks())
                enter(url+"?batch=1\n"+url+"?batch=2", replace=True)
                node("共 2 条链接")
                click("添加 2 条链接")
                wait(lambda: len([t for t in tasks() if "batch=" in t["url"] and t["status"] == "completed"]) == 2,
                     "Batch button did not complete both valid downloads", 90)
                record("batch blocks invalid input, then downloads both corrected links")

                click("页面抓取")
                node("网页地址")
                focus_text_field()
                enter(f"http://127.0.0.1:{fixture.server_port}/page-{token_url}")
                click("抓取本页链接")
                node("网页抓取结果")
                click("取消当前分类")
                assert "enabled" not in node("创建 0 个下载", True)["states"]
                click("全选当前分类")
                click("读取文件大小")
                node("≥ 1 MB", True)
                wait(lambda: "enabled" in node("≥ 1 MB", True)["states"], "Harvest size probe did not finish", 30)
                for label in ("≥ 1 MB", "≥ 10 MB", "全部大小"):
                    click(label)
                    record("harvest size filter: " + label)
                category = next(n["name"] for n in tree() if n["actions"] and n["name"].startswith("压缩包 "))
                click(category)
                click("创建 1 个下载")
                wait(lambda: any("harvest=1" in t["url"] and t["status"] == "completed" for t in tasks()),
                     "Selected harvest resource did not download", 90)
                assert not any("harvest=2" in t["url"] for t in tasks())
                record("page harvest, size probing, category selection, select/deselect and selected download")

            settings_to_restore["queue_profiles"] = original_settings["queue_profiles"]
            click("管理队列")
            click("新建队列")
            click("保存队列")
            old_ids = {p["id"] for p in original_settings["queue_profiles"]}
            new_queue = wait(lambda: next((p for p in core({"type":"load_settings","request_id":8})["queue_profiles"]
                                          if p["id"] not in old_ids), None), "Queue save did not reach Core", 15)
            record("queue manager creates and saves real queue", new_queue["name"])
            # 队列调度只支持排队或暂停的任务，已完成任务的移动按钮应禁用。
            pending_result = core({"type": "command", "request_id": 9, "command": {
                "kind": "create_task", "spec": {"url": url + "?move-queue=1", "resource_kind": "file",
                    "scheduled_start_at": "2999-01-01T00:00:00Z"}}})
            pending_id = next(item["event"]["snapshot"]["task_id"] for item in pending_result["events"]
                              if item["event"]["kind"] == "task_created")
            created_tasks.add(pending_id)
            wait(lambda: "taskrow." + pending_id in ui("state")["controlBounds"], "Queue target row missing", 10)
            pointer("taskrow." + pending_id)
            wait(lambda: pending_id in ui("state")["selectedTaskIds"], "Queue target not selected", 10)
            pointer("toolbar.移动队列")
            node("移动到队列")
            click("选择队列 " + new_queue["name"])
            click("移动")
            wait(lambda: any(t["task_id"] == pending_id and t["queue_id"] == new_queue["id"] for t in tasks()),
                 "Queue assignment did not reach Core", 15)
            record("selected task moves into saved queue")

            # 通知中心和关于由 extras 阶段在无选择状态下验收；选中任务时此菜单是任务操作。
            record("test task count", len(created_tasks))
        if args.start_at == "extras":
            from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
            token_url = uuid.uuid4().hex
            payload = bytes(range(256))*128
            media_path = evidence / "player-hud-fixture.mkv"
            subtitle_path = evidence / "player-hud-fixture.srt"
            subtitle_path2 = evidence / "player-hud-fixture-track-2.srt"
            ffmpeg = runtime / "app/resources/ffmpeg.exe"
            if not ffmpeg.is_file():
                raise FileNotFoundError("Bundled ffmpeg is missing: " + str(ffmpeg))
            subtitle_path.write_text(
                "1\n00:00:00,500 --> 00:00:03,500\nPlayer HUD subtitle fixture\n\n"
                "2\n00:00:04,000 --> 00:00:07,500\nSubtitle track remains available\n",
                encoding="ascii")
            verify_token = "HUDTRACK7K"
            ffmpeg_args = [
                str(ffmpeg), "-v", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x180:rate=15",
                "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
                "-f", "lavfi", "-i", "sine=frequency=660:sample_rate=48000",
                "-f", "srt", "-i", str(subtitle_path),
            ]
            subtitle_path2.write_text(
                f"1\n00:00:00,200 --> 00:00:07,500\n{verify_token}\n",
                encoding="ascii")
            ffmpeg_args.extend(["-f", "srt", "-i", str(subtitle_path2)])
            ffmpeg_args.extend([
                "-t", "8", "-map", "0:v:0", "-map", "1:a:0", "-map", "2:a:0", "-map", "3:0",
            ])
            ffmpeg_args.extend(["-map", "4:0"])
            ffmpeg_args.extend([
                "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "64k",
                "-c:s", "srt", "-metadata:s:a:0", "title=Audio 1", "-metadata:s:a:1", "title=Audio 2",
                str(media_path),
            ])
            subprocess.run(ffmpeg_args, check=True, timeout=60, capture_output=True)
            class ExtraHandler(BaseHTTPRequestHandler):
                def log_message(self, *_): pass
                def do_GET(self):
                    body = media_path.read_bytes() if self.path.endswith(".mkv") else payload
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.send_header("Content-Type", "application/octet-stream")
                    self.end_headers()
                    self.wfile.write(body)
            fixture = ThreadingHTTPServer(("127.0.0.1",0), ExtraHandler)
            threading.Thread(target=fixture.serve_forever,daemon=True).start()
            ours = []
            def snapshot(): return core({"type":"snapshot","request_id":31})["tasks"]
            def pointer(control, modifiers=None):
                bounds = wait(lambda: ui("state")["controlBounds"].get(control), "Missing control " + control,10)
                action("click",x=(bounds[0]+bounds[2])//2,y=(bounds[1]+bounds[3])//2,modifiers=modifiers or [])
            def enter(text):
                action("key",key="A",modifiers=["CTRL"])
                action("type",text=text)
            def visible_action(name):
                return next((n for n in tree() if n["name"] == name and n["actions"] and
                             "showing" in n["states"]), None)
            if args.extras_stage == "all":
                for label in ("通知中心", "关于 HLS Downloader"):
                    if not visible_action(label):
                        pointer("toolbar.更多操作")
                        wait(lambda: visible_action(label), "More menu item did not appear: " + label, 5)
                    click(label)
                    node("关于" if label == "关于 HLS Downloader" else label)
                    observe("more-"+label)
                    click("关闭",-1)
                    title = "关于" if label == "关于 HLS Downloader" else label
                    wait(lambda: not any(n["name"] == title and n["role"] == "label" and
                                         "showing" in n["states"] for n in tree()),
                         "More-menu dialog did not close: " + label, 5)
                    record("main more-menu opens and closes: "+label)
            for ext in ("bin","zip","mkv"):
                response=core({"type":"command","request_id":32,"command":{"kind":"create_task","spec":{
                    "url":f"http://127.0.0.1:{fixture.server_port}/extras-{token_url}.{ext}","filename":f"extras-{token_url}.{ext}","resource_kind":"file"}}})
                task=next(e["event"]["snapshot"] for e in response["events"] if e["event"]["kind"]=="task_created")
                created_tasks.add(task["task_id"])
                core({"type":"command","request_id":33,"command":{"kind":"task_action","task_id":task["task_id"],"action":"start"}})
                task=wait(lambda: next((t for t in snapshot() if t["task_id"]==task["task_id"] and t["status"]=="completed"),None),"Prepare extras fixture",30)
                ours.append(task)
            record("prepared three downloaded files for remaining interactions")
            settings_to_restore["task_sort"] = original_settings["task_sort"]
            click("搜索任务")
            enter(token_url)
            wait(lambda: all("taskrow."+t["task_id"] in ui("state")["controlBounds"] for t in ours),"Search omitted matching tasks",10)
            enter("no-match-"+token_url)
            wait(lambda: not any(k.startswith("taskrow.") for k in ui("state")["controlBounds"]),"Search did not filter rows",10)
            enter(token_url)
            record("search matches filename and empty result then restores test-only view")
            if args.extras_stage == "all":
                for field,label in (("name","名称"),("size","大小"),("status","状态")):
                    before=core({"type":"load_settings","request_id":34})["task_sort"]
                    target=node(label,True)
                    x,y,w,h=target["bounds"]
                    window=ui("window")
                    header_point={"x":x+w//2-window["deviceX"],"y":y+h//2-window["deviceY"]}
                    action("click",**header_point)
                    changed=wait(lambda: (v if (v:=core({"type":"load_settings","request_id":35})["task_sort"])!=before else None),"Sort did not persist",10)
                    assert changed.startswith(field+":"),(field,changed)
                    action("click",**header_point)
                    wait(lambda: core({"type":"load_settings","request_id":36})["task_sort"]!=changed,"Sort direction did not toggle",10)
                record("name, size, status sort directions persist through actual table header clicks")
            pointer("taskrow."+ours[0]["task_id"])
            wait(lambda: ours[0]["task_id"] in ui("state")["selectedTaskIds"],"Row selection not settled",10)
            # 真正的 Windows Ctrl+A，排除测试 API 的 AWT 事件合成差异。
            user=ctypes.windll.user32
            for vk,flag in ((0x11,0),(0x41,0),(0x41,2),(0x11,2)):
                user.keybd_event(vk,0,flag,0)
                time.sleep(.05)
            wait(lambda: set(ui("state")["selectedTaskIds"])=={t["task_id"] for t in ours},"Select-all included hidden tasks",10)
            pointer("toolbar.删除")
            node("此操作将影响 3 个任务")
            click("取消",-1)
            assert all(any(t["task_id"]==o["task_id"] for t in snapshot()) for o in ours)
            record("select-all respects search and destructive confirmation cancel preserves tasks")
            action("key",key="ESCAPE")
            pointer("taskrow."+ours[2]["task_id"])
            wait(lambda: ui("state")["selectedTaskIds"]==[ours[2]["task_id"]],"Media row selection failed",10)
            pointer("toolbar.播放")
            def player_status():
                response = core({"type": "command", "request_id": 36, "command": {"kind": "player_control", "action": "status"}})
                return next((e["event"] for e in response.get("events", []) if e["event"].get("kind") == "player_session"), {})
            wait(lambda: player_status().get("position_available"), "Local video did not open", 60)
            wait(lambda: "video.surface" in ui("state")["controlBounds"], "Local video did not open in the workbench overlay", 15)
            assert not any(key.startswith("player.") for key in ui("state")["controlBounds"]), "Local playback opened a detached control HUD"
            pointer("video.关闭播放器，下载继续")
            wait(lambda: not player_status().get("active") and "video.surface" not in ui("state")["controlBounds"], "Local video close did not release the overlay", 15)
            record("local video opens in the workbench overlay and closes using its own close button")
            settings_to_restore["completion_sound_enabled"] = original_settings["completion_sound_enabled"]
            pointer("toolbar.设置")
            click("完成提示音")
            click("保存设置")
            wait(lambda: core({"type":"load_settings","request_id":37})["completion_sound_enabled"]!=original_settings["completion_sound_enabled"],"Settings save did not persist",15)
            record("settings save persists changed option; original restored in cleanup")
            pointer("taskrow."+ours[0]["task_id"])
            wait(lambda: ui("state")["selectedTaskIds"]==[ours[0]["task_id"]],"Delete fixture selection failed",10)
            pointer("toolbar.删除")
            click("确认删除")
            wait(lambda: all(t["task_id"]!=ours[0]["task_id"] for t in snapshot()),"Delete confirmation did not remove record",15)
            assert Path(ours[0]["output_path"]).exists()
            Path(ours[0]["output_path"]).unlink()
            created_tasks.discard(ours[0]["task_id"])
            record("confirmed delete removes task and preserves completed file")
            if args.extras_stage == "destructive":
                victim = ours[1]
                victim_file = Path(victim["output_path"])
                pointer("taskrow." + victim["task_id"])
                wait(lambda: ui("state")["selectedTaskIds"] == [victim["task_id"]], "Delete-file target not selected", 10)

                def open_file_delete():
                    left, top, right, bottom = ui("state")["controlBounds"]["taskrow." + victim["task_id"]]
                    action("right_click", x=(left + right)//2, y=(top + bottom)//2)
                    click("删除任务和文件")
                    node("任务记录、已下载文件和过程文件都会删除。")

                open_file_delete()
                click("取消", -1)
                assert victim_file.is_file() and any(t["task_id"] == victim["task_id"] for t in snapshot())
                open_file_delete()
                click("确认删除")
                wait(lambda: all(t["task_id"] != victim["task_id"] for t in snapshot()), "Delete-files left its task", 15)
                wait(lambda: not victim_file.exists(), "Delete-files left the completed file", 15)
                created_tasks.discard(victim["task_id"])
                record("delete task and file: cancel preserves both; confirm removes both")

                pending_response = core({"type":"command", "request_id":40, "command":{"kind":"create_task", "spec":{
                    "url":f"http://127.0.0.1:{fixture.server_port}/queued-{token_url}.bin", "resource_kind":"file", "filename":"pending-"+token_url+".bin",
                    "scheduled_start_at":"2999-01-01T00:00:00Z"}}})
                pending = next(e["event"]["snapshot"] for e in pending_response["events"] if e["event"]["kind"] == "task_created")
                created_tasks.add(pending["task_id"])
                assert any(t["task_id"] == pending["task_id"] and t["status"] == "queued" for t in snapshot())
                completed = [t for t in snapshot() if t["status"] == "completed"]
                completed_ids = {t["task_id"] for t in completed}
                preserved = {str(p):hashlib.sha256(p.read_bytes()).hexdigest() for t in completed
                             if (p := Path(t["output_path"])).is_file()}
                assert completed_ids and preserved
                pointer("content_header.clear_completed")
                node("只删除任务记录，已完成文件将保留。")
                click("取消", -1)
                assert completed_ids.issubset({t["task_id"] for t in snapshot()})
                pointer("content_header.clear_completed")
                click("确认删除")
                wait(lambda: not completed_ids.intersection(t["task_id"] for t in snapshot()), "Clear-completed left completed tasks", 20)
                assert any(t["task_id"] == pending["task_id"] and t["status"] == "queued" for t in snapshot())
                assert all(Path(p).is_file() and hashlib.sha256(Path(p).read_bytes()).hexdigest() == digest
                           for p, digest in preserved.items())
                created_tasks.difference_update(completed_ids)
                record("clear completed: cancel preserves records; confirm removes completed records, preserves queued task and file SHA256")
            focus_text_field()
            enter("")
        if args.start_at == "torrent":
            import re
            import socketserver
            import struct
            from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
            torrent_token=uuid.uuid4().hex
            parts=[bytes([i])*65536 for i in (11,22,33)]
            payload=b"".join(parts)
            requested_pieces=[]
            metadata_requests=[]
            metadata_connections=[]
            tracker_requests=[]
            metadata_gate=threading.Event()
            loading_responses=[]
            info_bytes=b""
            def bencode(value):
                if isinstance(value,int): return b"i"+str(value).encode()+b"e"
                if isinstance(value,str): value=value.encode()
                if isinstance(value,bytes): return str(len(value)).encode()+b":"+value
                if isinstance(value,list): return b"l"+b"".join(bencode(v) for v in value)+b"e"
                return b"d"+b"".join(bencode(k)+bencode(value[k]) for k in sorted(value))+b"e"
            class Peer(socketserver.BaseRequestHandler):
                def handle(self):
                    self.request.settimeout(3)
                    def read(size):
                        data=b""
                        while len(data)<size:
                            chunk=self.request.recv(size-len(data))
                            if not chunk: raise EOFError()
                            data+=chunk
                        return data
                    try:
                        handshake=read(68)
                        metadata_connections.append(time.perf_counter())
                        self.request.sendall(handshake+b"\x00\x00\x00\x02\x05\xe0"+b"\x00\x00\x00\x01\x01")
                        while True:
                            size=struct.unpack(">I",read(4))[0]
                            message=read(size)
                            if message and message[0]==20:
                                if message[1]==0:
                                    body=b"\x14\x00"+bencode({"m":{"ut_metadata":3},"metadata_size":len(info_bytes)})
                                elif message[1]==3:
                                    index=int(re.search(rb"5:piecei(\d+)e",message[2:]).group(1))
                                    time.sleep(.25)
                                    metadata_requests.append(index)
                                    body=b"\x14\x01"+bencode({"msg_type":1,"piece":index,"total_size":len(info_bytes)})+info_bytes[index*16384:(index+1)*16384]
                                else: continue
                                self.request.sendall(struct.pack(">I",len(body))+body)
                                continue
                            if not message or message[0]!=6: continue
                            index,offset,length=struct.unpack(">III",message[1:13])
                            requested_pieces.append(index)
                            body=b"\x07"+struct.pack(">II",index,offset)+payload[index*65536+offset:index*65536+offset+length]
                            self.request.sendall(struct.pack(">I",len(body))+body)
                    except (EOFError,OSError): pass
            class PeerServer(socketserver.ThreadingTCPServer):
                daemon_threads=True
            torrent_peer=PeerServer(("127.0.0.1",0),Peer)
            threading.Thread(target=torrent_peer.serve_forever,daemon=True).start()
            torrent_bytes=b""
            class TorrentHttp(BaseHTTPRequestHandler):
                def log_message(self,*_): pass
                def do_GET(self):
                    if self.path=="/loading.torrent":
                        if not metadata_gate.wait(30): return
                        loading_responses.append(True)
                    if not self.path.endswith(".torrent"):
                        tracker_requests.append(self.path)
                    body=b"!invalid torrent" if self.path.endswith("invalid.torrent") else torrent_bytes if self.path.endswith(".torrent") else bencode({
                        "interval":30,"peers":bytes([127,0,0,1])+struct.pack(">H",torrent_peer.server_address[1])})
                    self.send_response(200)
                    self.send_header("Content-Length",str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
            fixture=ThreadingHTTPServer(("127.0.0.1",0),TorrentHttp)
            base=f"http://127.0.0.1:{fixture.server_port}"
            info={
                "name":"ui-torrent-"+torrent_token,"piece length":65536,
                "pieces":b"".join(hashlib.sha1(part).digest() for part in parts),
                "files":[{"length":len(part),"path":[f"file-{i}.bin"]} for i,part in enumerate(parts)]}
            info_bytes=bencode(info)
            torrent_bytes=bencode({"announce":base+"/announce","info":info})
            threading.Thread(target=fixture.serve_forever,daemon=True).start()
            source=base+"/ui-"+torrent_token+".torrent"
            if args.torrent_source=="magnet":
                source=f"magnet:?xt=urn:btih:{hashlib.sha1(info_bytes).hexdigest()}&dn=ui-magnet-{torrent_token}&x.pe=127.0.0.1:{torrent_peer.server_address[1]}&tr={base}/announce"
            def enter_torrent(url):
                click("新建")
                node("等待输入")
                bounds=ui("state")["controlBounds"]["newtask.url"]
                action("click",x=(bounds[0]+bounds[2])//2,y=(bounds[1]+bounds[3])//2)
                action("type",text=url)
            if args.torrent_source=="magnet" and args.torrent_checks=="all":
                loading_source=base+"/loading.torrent"
                enter_torrent(loading_source)
                click("创建下载")
                node("正在解析种子")
                observe("torrent-metadata-loading")
                assert not loading_responses,"Loading assertion raced completed metadata"
                click("关闭")
                metadata_gate.set()
                wait(lambda: loading_responses,"Closed probe did not complete",10)
                time.sleep(1)
                assert not any("选择种子文件" in n["name"] for n in tree()),"Late result reopened closed probe"
                assert not any(t["url"]==loading_source for t in core({"type":"snapshot","request_id":64})["tasks"]),"Closing probe created a task"
                record("torrent shows metadata loading; close remains responsive; late result does not reopen selection or create task")
                enter_torrent(base+"/invalid.torrent")
                click("创建下载")
                wait(lambda:any("invalid bencode" in item[1] for item in ui("state").get("noticeHistory",[])),"Parse error did not reach notification history",10)
                (evidence/"torrent-invalid-error.png").write_bytes(ui("screenshot",raw=True))
                wait(lambda:not any("正在解析种子" in n["name"] for n in tree()),"Failed probe did not leave loading",10)
                record("invalid torrent preserves parse error in notification history and leaves loading state")
            enter_torrent(source)
            node("类型：torrent")
            metadata_started=click("创建下载")
            node("选择种子文件")
            metadata_ms=(time.perf_counter()-metadata_started)*1000
            observe("torrent-file-selection")
            click("选择全部种子文件")
            assert "enabled" not in node("创建已选任务",True)["states"]
            click("选择 file-0.bin")
            click("选择 file-2.bin")
            node("已选 2 / 3")
            click("创建已选任务")
            def torrent_task():
                task=next((t for t in core({"type":"snapshot","request_id":65})["tasks"] if t["url"]==source),None)
                if task: created_tasks.add(task["task_id"])
                return task
            done=wait(lambda:(t if (t:=torrent_task()) and t["status"] in ("completed","failed") else None),"UI torrent did not finish",60)
            assert done["status"]=="completed",done
            output=Path(done["output_path"])
            assert output.is_dir(),done
            for index in (0,2):
                assert (output/f"file-{index}.bin").read_bytes()==parts[index]
            assert not (output/"file-1.bin").exists()
            assert set(requested_pieces)=={0,2},requested_pieces
            if args.torrent_source=="magnet":
                assert metadata_requests and not tracker_requests,{"metadata":metadata_requests,"tracker":tracker_requests}
                peer_ms=(metadata_connections[0]-metadata_started)*1000
                assert peer_ms<8000,peer_ms
            record("real new-task "+args.torrent_source+" probe, deselect-all disabled submit, select two files, peer transfer and selected-only publication",
                   {"ui_action_and_accessibility_observation_ms":metadata_ms,"first_peer_ms":(metadata_connections[0]-metadata_started)*1000,
                    "metadata_requests":metadata_requests,"tracker_requests":len(tracker_requests),
                    "requested_pieces":sorted(set(requested_pieces)),"selected_sha256":[hashlib.sha256(parts[i]).hexdigest() for i in (0,2)]})
        if args.start_at == "drop":
            from ctypes import wintypes
            from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
            from smoke_v7_presenter_visual import user32, find_window
            from PIL import ImageGrab
            drop_token=uuid.uuid4().hex
            payload=bytes(range(256))*512
            class DropHttp(BaseHTTPRequestHandler):
                def log_message(self,*_): pass
                def do_GET(self):
                    self.send_response(200)
                    self.send_header("Content-Length",str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
            fixture=ThreadingHTTPServer(("127.0.0.1",0),DropHttp)
            threading.Thread(target=fixture.serve_forever,daemon=True).start()
            source=f"http://127.0.0.1:{fixture.server_port}/drop-{drop_token}.bin"
            dropped=evidence/"真实拖入链接.txt"
            dropped.write_text(source+"\n",encoding="utf8")
            helper=evidence/"DragFixture.java"
            helper.write_text('''import javax.swing.*;
import java.awt.datatransfer.*;
import java.awt.event.*;
import java.io.File;
import java.util.List;
public class DragFixture {
  public static void main(String[] args) {
    SwingUtilities.invokeLater(() -> {
      JFrame frame=new JFrame("HLS native file drag fixture");
      JLabel file=new JLabel(new File(args[0]).getName(),SwingConstants.CENTER);
      file.setTransferHandler(new TransferHandler() {
        public int getSourceActions(JComponent c) { return COPY; }
        protected Transferable createTransferable(JComponent c) {
          return new Transferable() {
            public DataFlavor[] getTransferDataFlavors() { return new DataFlavor[]{DataFlavor.javaFileListFlavor}; }
            public boolean isDataFlavorSupported(DataFlavor f) { return DataFlavor.javaFileListFlavor.equals(f); }
            public Object getTransferData(DataFlavor f) throws UnsupportedFlavorException {
              if (!isDataFlavorSupported(f)) throw new UnsupportedFlavorException(f);
              return List.of(new File(args[0]));
            }
          };
        }
      });
      file.addMouseMotionListener(new MouseMotionAdapter() {
        public void mouseDragged(MouseEvent e) { System.out.println("drag source started"); file.getTransferHandler().exportAsDrag(file,e,TransferHandler.COPY); }
      });
      frame.setDefaultCloseOperation(JFrame.DISPOSE_ON_CLOSE);
      frame.add(file); frame.setBounds(15,150,330,170); frame.setVisible(true);
    });
  }
}''',encoding="utf8")
            temporary=evidence/"java-tmp"
            temporary.mkdir(exist_ok=True)
            original_window=ui("window")
            user32.SetWindowPos.argtypes=[wintypes.HWND,wintypes.HWND,ctypes.c_int,ctypes.c_int,
                                          ctypes.c_int,ctypes.c_int,wintypes.UINT]
            user32.SetWindowPos.restype=wintypes.BOOL
            user32.SystemParametersInfoW.argtypes=[wintypes.UINT,wintypes.UINT,
                                                   ctypes.POINTER(wintypes.RECT),wintypes.UINT]
            user32.SystemParametersInfoW.restype=wintypes.BOOL
            work_area=wintypes.RECT()
            assert user32.SystemParametersInfoW(0x0030,0,ctypes.byref(work_area),0), \
                "Could not read desktop work area"
            margin=16
            work_width,work_height=work_area.right-work_area.left,work_area.bottom-work_area.top
            fitted_width=min(1250,work_width-2*margin)
            fitted_height=min(960,work_height-2*margin)
            assert fitted_width>0 and fitted_height>0,"Desktop work area is too small for native drag"
            fitted_x=max(work_area.left+margin,min(500,work_area.right-fitted_width-margin))
            fitted_y=max(work_area.top+margin,min(0,work_area.bottom-fitted_height-margin))
            assert user32.SetWindowPos(wintypes.HWND(hwnd),None,fitted_x,fitted_y,
                                      fitted_width,fitted_height,0x0014), \
                "Could not fit workbench into the visible desktop work area"
            time.sleep(.3)
            try:
                with (evidence/"drag-fixture.log").open("wb") as log:
                    java_exe=os.environ.get("HLS_JAVA_EXE", str(Path(__file__).resolve().parents[2]/".tool-cache/build-cache/jdk-21/bin/java.exe"))
                    drop_helper=subprocess.Popen([java_exe,
                        "-Djava.io.tmpdir="+str(temporary),str(helper),str(dropped)],stdout=log,stderr=log,
                        creationflags=subprocess.CREATE_NO_WINDOW)
                try:
                    source_window=wait(lambda:find_window(drop_helper.pid,"HLS native file drag fixture"),"Native drag source did not open",90)
                except TimeoutError:
                    diagnostic={"pid":drop_helper.pid,"exit_code":drop_helper.poll(),"exe":None,"windows":[]}
                    try:
                        diagnostic["exe"]=psutil.Process(drop_helper.pid).exe()
                    except psutil.Error as error:
                        diagnostic["process_error"]=str(error)
                    user32.GetClassNameW.argtypes=[wintypes.HWND,wintypes.LPWSTR,ctypes.c_int]
                    user32.GetClassNameW.restype=ctypes.c_int
                    user32.EnumWindows.argtypes=[ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM),wintypes.LPARAM]
                    @ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
                    def inspect_drag_window(handle,_):
                        owner=wintypes.DWORD()
                        user32.GetWindowThreadProcessId(handle,ctypes.byref(owner))
                        if owner.value!=drop_helper.pid:
                            return True
                        title=ctypes.create_unicode_buffer(user32.GetWindowTextLengthW(handle)+1)
                        class_name=ctypes.create_unicode_buffer(256)
                        rect=wintypes.RECT()
                        user32.GetWindowTextW(handle,title,len(title))
                        user32.GetClassNameW(handle,class_name,len(class_name))
                        user32.GetWindowRect(handle,ctypes.byref(rect))
                        diagnostic["windows"].append({"hwnd":int(handle),"visible":bool(user32.IsWindowVisible(handle)),
                            "title":title.value,"class":class_name.value,
                            "rect":[rect.left,rect.top,rect.right,rect.bottom]})
                        return True
                    try:
                        user32.EnumWindows(inspect_drag_window,0)
                    except OSError as error:
                        diagnostic["window_enumeration_error"]=str(error)
                    report["drop_helper_diagnostic"]=diagnostic
                    raise
                user32.SetForegroundWindow(source_window)
                rect=wintypes.RECT()
                user32.GetWindowRect(source_window,ctypes.byref(rect))
                sx,sy=(rect.left+rect.right)//2,(rect.top+rect.bottom)//2
                target=ui("window")
                workbench_rect=[target["deviceX"],target["deviceY"],
                                target["deviceX"]+target["deviceWidth"],
                                target["deviceY"]+target["deviceHeight"]]
                assert (work_area.left<=workbench_rect[0]<workbench_rect[2]<=work_area.right and
                        work_area.top<=workbench_rect[1]<workbench_rect[3]<=work_area.bottom), \
                    "Fitted workbench is not fully inside the visible desktop work area: " + str(workbench_rect)
                report["native_drag_layout"]={
                    "work_area":[work_area.left,work_area.top,work_area.right,work_area.bottom],
                    "workbench_rect":workbench_rect,
                }
                save()
                tx,ty=target["deviceX"]+target["deviceWidth"]//2,target["deviceY"]+target["deviceHeight"]//2
                user32.SetCursorPos(sx,sy)
                time.sleep(.3)
                user32.mouse_event(2,0,0,0,0)
                time.sleep(.2)
                try:
                    for step in range(1,31):
                        user32.SetCursorPos(round(sx+(tx-sx)*step/30),round(sy+(ty-sy)*step/30))
                        time.sleep(.05)
                    time.sleep(.5)
                    ImageGrab.grab().save(evidence/"native-drag-hover.png")
                    user32.GetAncestor.argtypes=[wintypes.HWND,wintypes.UINT]
                    user32.GetAncestor.restype=wintypes.HWND
                    user32.WindowFromPoint.argtypes=[wintypes.POINT]
                    user32.WindowFromPoint.restype=wintypes.HWND
                    cursor=wintypes.POINT()
                    user32.GetCursorPos(ctypes.byref(cursor))
                    hit=user32.WindowFromPoint(cursor)
                    hit_root=user32.GetAncestor(hit,2) if hit else 0
                    target_visible=(work_area.left<=cursor.x<work_area.right and
                                    work_area.top<=cursor.y<work_area.bottom and
                                    int(hit_root or 0)==int(hwnd))
                    report["native_drag_release_target"]={
                        "point":[cursor.x,cursor.y],
                        "work_area":[work_area.left,work_area.top,work_area.right,work_area.bottom],
                        "hit_window":int(hit or 0),"hit_root_window":int(hit_root or 0),
                        "workbench_window":int(hwnd),"visible_workbench_hit":target_visible,
                    }
                    save()
                    if not target_visible:
                        user32.SetCursorPos(sx,sy)
                        raise AssertionError(
                            "Native drag release point is outside the visible workbench: " +
                            json.dumps(report["native_drag_release_target"])
                        )
                finally:
                    user32.mouse_event(4,0,0,0,0)
                def dropped_task():
                    task=next((t for t in core({"type":"snapshot","request_id":75})["tasks"] if t["url"]==source),None)
                    if task: created_tasks.add(task["task_id"])
                    return task if task and task["status"] in ("completed","failed") else None
                done=wait(dropped_task,"Native file drag did not import and finish",30)
                assert done["status"]=="completed",done
                assert Path(done["output_path"]).read_bytes()==payload
                observe("native-drag-imported")
                record("real cross-process native file drag imports UTF8 URL list and downloads exact bytes",
                       {"bytes":len(payload),"sha256":hashlib.sha256(payload).hexdigest()})
            finally:
                user32.SetWindowPos(hwnd,None,original_window["deviceX"],original_window["deviceY"],original_window["deviceWidth"],original_window["deviceHeight"],0x0014)
        if args.start_at in ("dialogs", "files"):
            from ctypes import wintypes
            from PIL import ImageGrab
            user=ctypes.windll.user32
            user.SetForegroundWindow.argtypes=[wintypes.HWND]
            user.GetWindowTextW.argtypes=[wintypes.HWND,wintypes.LPWSTR,ctypes.c_int]
            user.GetWindowTextLengthW.argtypes=[wintypes.HWND]
            user.IsWindowVisible.argtypes=[wintypes.HWND]
            user.GetWindowThreadProcessId.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.DWORD)]
            user.EnumWindows.argtypes=[ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM),wintypes.LPARAM]
            callback_type=ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
            def use_chooser(title, control_label, selected_path=None):
                target=node(control_label,True)
                x,y,w,h=target["bounds"]
                window=ui("window")
                click_point={"x":x+w//2-window["deviceX"],"y":y+h//2-window["deviceY"]}
                runtime_pid=wintypes.DWORD()
                user.GetWindowThreadProcessId(wintypes.HWND(hwnd),ctypes.byref(runtime_pid))
                def find():
                    found=[]
                    @callback_type
                    def visit(handle,_):
                        text=ctypes.create_unicode_buffer(user.GetWindowTextLengthW(handle)+1)
                        user.GetWindowTextW(handle,text,len(text))
                        candidate_pid=wintypes.DWORD()
                        user.GetWindowThreadProcessId(handle,ctypes.byref(candidate_pid))
                        if candidate_pid.value==runtime_pid.value and text.value and "HLS Downloader" not in text.value and user.IsWindowVisible(handle): found.append(handle)
                        return True
                    user.EnumWindows(visit,0)
                    return found[0] if found else None
                click_errors=[]
                click_thread=None
                if selected_path is None:
                    def click_cancel_flow():
                        try:
                            action("click",timeout=60,**click_point)
                        except Exception as error:
                            click_errors.append(error)
                    click_thread=threading.Thread(target=click_cancel_flow,daemon=True)
                    click_thread.start()
                else:
                    action("click",**click_point)
                try:
                    chooser=wait(find,"System chooser not visible: "+title,20)
                    ImageGrab.grab().save(evidence/("chooser-"+title[:12]+".png"))
                    vm,root=client.context_from_window(chooser)
                    owned=[]
                    try:
                        controls=client.walk(vm,root,1000)
                        (evidence/("chooser-"+title[:12]+".json")).write_text(json.dumps(controls,ensure_ascii=False,indent=2),encoding="utf8")
                        if selected_path is not None:
                            field=next(n for n in controls if n["role"]=="text" and "editable" in n["states"] and "showing" in n["states"])
                            x,y,w,h=field["bounds"]
                            action("click",x=x+w//2-window["deviceX"],y=y+h//2-window["deviceY"])
                            action("key",key="A",modifiers=["CTRL"])
                            action("type",text=str(selected_path))
                            controls=client.walk(vm,root,1000)
                        labels=("保存","打开") if selected_path is not None else ("取消",)
                        button=next(n for n in controls if any(n["name"].startswith(label) for label in labels) and n["role"]=="push button")
                        context,owned=client.resolve_path(vm,root,button["path"])
                        client.invoke(vm,context,None)
                    finally:
                        for context in reversed(owned):client.dll.releaseJavaObject(vm,context)
                        client.dll.releaseJavaObject(vm,root)
                    wait(lambda: not user.IsWindowVisible(wintypes.HWND(chooser)),"System chooser button failed",10)
                finally:
                    if click_thread:
                        click_thread.join(timeout=60)
                        if click_thread.is_alive():
                            raise TimeoutError("Chooser click action did not return after closing: "+title)
                        if click_errors:
                            raise click_errors[0]
                record("real system chooser "+("selects file: " if selected_path is not None else "opens and cancels: ")+title)
            if args.start_at == "files":
                from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
                file_token=uuid.uuid4().hex
                payload=bytes(range(256))*128
                class FileHandler(BaseHTTPRequestHandler):
                    def log_message(self,*_): pass
                    def do_GET(self):
                        self.send_response(200)
                        self.send_header("Content-Length",str(len(payload)))
                        self.end_headers()
                        self.wfile.write(payload)
                fixture=ThreadingHTTPServer(("127.0.0.1",0),FileHandler)
                threading.Thread(target=fixture.serve_forever,daemon=True).start()
                file_url=f"http://127.0.0.1:{fixture.server_port}/files-{file_token}.bin"
                response=core({"type":"command","request_id":50,"command":{"kind":"create_task","spec":{
                    "url":file_url,"filename":f"files-{file_token}.bin","resource_kind":"file"}}})
                source=next(e["event"]["snapshot"] for e in response["events"] if e["event"]["kind"]=="task_created")
                created_tasks.add(source["task_id"])
                def file_tasks():
                    matches=[t for t in core({"type":"snapshot","request_id":51})["tasks"] if t["url"]==file_url]
                    created_tasks.update(t["task_id"] for t in matches)
                    return matches
                source=wait(lambda:next((t for t in file_tasks() if t["status"]=="completed"),None),"File fixture did not complete",30)
                target=node(source["filename"])
                x,y,w,h=target["bounds"]
                window=ui("window")
                action("click",x=x+w//3-window["deviceX"],y=y+h//2-window["deviceY"])
                wait(lambda:ui("state")["selectedTaskIds"]==[source["task_id"]],"Export source not selected",10)
                bounds=ui("state")["controlBounds"]["toolbar.更多操作"]
                action("click",x=(bounds[0]+bounds[2])//2,y=(bounds[1]+bounds[3])//2)
                exported=evidence/"tasks-roundtrip.json"
                use_chooser("导出任务列表","导出任务列表",exported)
                wait(exported.is_file,"Export did not write file",15)
                document=json.loads(exported.read_text(encoding="utf8"))
                assert len(document["tasks"])==1 and document["tasks"][0]["url"]==file_url,document
                click("删除")
                click("确认删除")
                wait(lambda:not file_tasks(),"Delete did not remove exported record",15)
                assert Path(source["output_path"]).read_bytes()==payload
                Path(source["output_path"]).unlink()
                created_tasks.discard(source["task_id"])
                # Core 删除可先于 Compose 关闭确认窗完成，等模态控件真正移除后再点主窗口。
                wait(lambda: not any(key.startswith("dialog.") for key in ui("state")["controlBounds"]),
                     "Delete confirmation did not leave the main window", 10)
                import_menu_before = ui("state")
                bounds=ui("state")["controlBounds"]["toolbar.更多操作"]
                action("click",x=(bounds[0]+bounds[2])//2,y=(bounds[1]+bounds[3])//2)
                import_menu_after = ui("state")
                (evidence/"import-menu-click.json").write_text(json.dumps({
                    "before": import_menu_before, "after": import_menu_after,
                    "bounds": bounds, "window": ui("window")}, ensure_ascii=False, indent=2), encoding="utf8")
                (evidence/"import-menu-click.png").write_bytes(ui("screenshot", raw=True))
                use_chooser("导入任务 JSON、种子、Metalink 或 URL 列表","导入任务或种子",exported)
                imported=wait(lambda:next((t for t in file_tasks() if t["status"]=="completed"),None),"Imported task did not download",30)
                assert Path(imported["output_path"]).read_bytes()==payload
                record("actual export chooser saves selected task; delete preserves file; import chooser recreates task and downloads matching bytes")
                record("imported output SHA256",hashlib.sha256(payload).hexdigest())
            else:
                click("新建")
                node("新建下载")
                use_chooser("选择下载保存目录","选择目录")
                click("取消",-1)
            for label,title in (("导入任务或种子","导入任务 JSON、种子、Metalink 或 URL 列表"),("导出任务列表","导出任务列表")):
                if args.start_at=="files": break
                bounds=ui("state")["controlBounds"]["toolbar.更多操作"]
                action("click",x=(bounds[0]+bounds[2])//2,y=(bounds[1]+bounds[3])//2)
                use_chooser(title,label)
            original_window=ui("window")
            click("最大化")
            node("还原",True)
            observe("window-maximized")
            click("还原")
            node("最大化",True)
            click("最小化")
            user.IsIconic.argtypes=[wintypes.HWND]
            wait(lambda:user.IsIconic(hwnd),"Minimize button failed",10)
            user.ShowWindow(wintypes.HWND(hwnd),9)
            user.SetForegroundWindow(hwnd)
            record("maximize, restore, minimize and restore visible workbench")
            user.SetWindowPos.argtypes=[wintypes.HWND,wintypes.HWND,ctypes.c_int,ctypes.c_int,ctypes.c_int,ctypes.c_int,wintypes.UINT]
            try:
                user.SetWindowPos(hwnd,None,original_window["deviceX"],original_window["deviceY"],1120,750,0x0014)
                time.sleep(.5)
                observe("window-narrow")
                for label in ("新建","粘贴链接","工具","设置"):
                    target=node(label,True)
                    assert "showing" in target["states"] and target["bounds"][2]>0,target
                click("新建")
                node("新建下载")
                action("key",key="ESCAPE")
                wait(lambda:"newtask.url" not in ui("state")["controlBounds"],"Escape did not cancel new task",10)
                record("narrow window preserves essential buttons and Escape cancels modal")
            finally:
                user.SetWindowPos(hwnd,None,original_window["deviceX"],original_window["deviceY"],original_window["deviceWidth"],original_window["deviceHeight"],0x0014)
        observe("main-final")
        report["passed"] = True
    except Exception as error:
        report["error"] = str(error)
        report["traceback"] = traceback.format_exc()
        try:
            report["failure_tasks"] = core({"type": "snapshot", "request_id": 88})["tasks"]
        except Exception:
            pass
        try:
            (evidence / "failure.png").write_bytes(ui("screenshot", raw=True))
            report["failure_state"] = ui("state")
            report["failure_tree"] = tree()
        except Exception:
            pass
        print(json.dumps({"error": str(error)}, ensure_ascii=True), flush=True)
    finally:
        try:
            action("release")
            if not startup_modal_failure:
                action("key", key="ESCAPE")
        except Exception:
            pass
        try:
            if args.start_at == "extras":
                core({"type":"command","request_id":89,"command":{"kind":"player_control","action":"stop"}})
            for task_id in created_tasks:
                core({"type": "command", "request_id": 90, "command": {
                    "kind": "task_action", "task_id": task_id, "action": "delete_files"}})
            if settings_to_restore:
                core({"type":"store_settings","request_id":91,"values":settings_to_restore})
            if args.start_at == "functional":
                resume_gate.set()
            if fixture:
                fixture.shutdown()
                fixture.server_close()
            if torrent_peer:
                torrent_peer.shutdown()
                torrent_peer.server_close()
            if drop_helper:
                drop_helper.terminate()
                drop_helper.wait(timeout=10)
        except Exception as error:
            report["cleanup_error"] = str(error)
            report["passed"] = False
        try:
            stop_ui()
            with (evidence / "normal-ui.log").open("wb") as log:
                subprocess.Popen([str(runtime / "HLSDownloader.exe")], cwd=runtime, env=normal_env, stdout=log, stderr=log)
            report["normal_runtime_restored"] = True
        except Exception as error:
            report["restore_error"] = str(error)
            report["passed"] = False
        save()
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--extras-stage", choices=("all", "selection", "destructive"), default="all")
    parser.add_argument("--functional-stage", choices=("all", "resume", "batch", "queue"), default="all")
    parser.add_argument("--settings-start-tab", default="通用")
    parser.add_argument("--torrent-source", choices=("torrent", "magnet"), default="torrent")
    parser.add_argument("--torrent-checks", choices=("all", "download"), default="all")
    parser.add_argument("--start-at", choices=("main", "newtask", "settings", "functional", "extras", "dialogs", "files", "drop", "torrent", "feedback", "hover"), default="main")
    raise SystemExit(run(parser.parse_args()))
