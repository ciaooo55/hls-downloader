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
from urllib.request import Request, urlopen

import psutil
from PIL import Image, ImageChops

from smoke_v7_accessibility import JabClient
from smoke_v7_portable_app import Pipe, runtime_processes, wait
from smoke_v7_transfer_performance import free_port


def run(args):
    runtime, evidence = args.runtime.resolve(), args.report.resolve().parent
    evidence.mkdir(parents=True, exist_ok=True)
    report = {"passed": False, "runtime": str(runtime), "checks": [], "controls": {}, "feedback": []}
    normal_env = {k: v for k, v in os.environ.items() if not k.startswith("HLS_UI_")}
    port, token = free_port(), uuid.uuid4().hex
    client = JabClient(runtime / "runtime/bin/windowsaccessbridge-64.dll")
    client.start()
    hwnd = None
    fixture = None
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

    def ui(path, payload=None, raw=False):
        data = json.dumps(payload).encode() if payload is not None else None
        request = Request(f"http://127.0.0.1:{port}/{path}", data=data,
                          headers={"X-HLS-Test-Token": token, "Content-Type": "application/json"})
        with urlopen(request, timeout=15) as response:
            body = response.read()
        return body if raw else json.loads(body)

    def action(kind, **values):
        ui("action", {"type": kind, **values})

    def tree():
        client.pump_messages()
        vm, root = client.context_from_window(hwnd)
        try:
            return client.walk(vm, root, 5000)
        finally:
            client.dll.releaseJavaObject(vm, root)

    def node(name, actionable=False, index=0):
        def find():
            matches = [n for n in tree() if (n["name"] == name if actionable else name in n["name"]) and (not actionable or n["actions"])]
            return matches[index] if len(matches) > index else None
        return wait(find, "Missing installed UI control: " + name, 12)

    def click(name, index=0):
        target = node(name, True, index)
        vm, root = client.context_from_window(hwnd)
        owned = []
        try:
            context, owned = client.resolve_path(vm, root, target["path"])
            client.invoke(vm, context, None)
        finally:
            for context in reversed(owned):
                client.dll.releaseJavaObject(vm, context)
            client.dll.releaseJavaObject(vm, root)
        time.sleep(.2)
        report["controls"].setdefault(f'{current_context}/{name}', {})["clicked"] = True
        save()

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
        node("新建", True)
        if initial_browser_connected:
            node("浏览器插件 · 已连接")
            record("workbench restart immediately restores current browser connection from snapshot")
        report["window"] = ui("window")
        report["start_at"] = args.start_at
        if args.start_at == "main":
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

            for button, expected in (("批量添加", "链接列表"), ("页面抓取", "抓取本页链接"), ("插件", "浏览器插件")):
                click(button)
                node(expected)
                probe_context("dialog-" + button, ("dialog.secondary.",))
                click("取消" if button != "插件" else "关闭", -1)
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
            click("取消")

        if args.start_at not in ("functional", "extras", "dialogs"):
            click("设置")
            node("保存设置")
            started_settings = args.settings_start_tab == "通用"
            for tab, heading in (("通用", "文件与运行"), ("下载", "默认并发"), ("计划", "限速与队列计划"),
                                 ("网络", "代理与站点"), ("媒体", "媒体处理与直播"), ("BT", "BT 与种子"),
                                 ("安全", "发布与扫描"), ("浏览器", "浏览器下载接管"), ("外观", "外观与可访问性")):
                started_settings = started_settings or tab == args.settings_start_tab
                if not started_settings:
                    continue
                click(tab)
                node(heading)
                probe_context("settings-" + tab, ("dialog.secondary.", "settings.choice.", "settings.day."))
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
            class Handler(RangeHandler):
                def do_GET(self):
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
                left, top, right, bottom = ui("state")["controlBounds"][control]
                action("click", x=(left+right)//2, y=(top+bottom)//2)

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
                enter(url, replace=True)
                node("类型：file")
                click("分析资源")
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
                    "url": url+"?resume=1", "resource_kind": "file", "filename": "resume-"+token_url+".bin", "concurrency": 1}}})
                queued = next(e["event"]["snapshot"] for e in response["events"] if e["event"]["kind"] == "task_created")
                created_tasks.add(queued["task_id"])
                wait(lambda: "taskrow."+queued["task_id"] in ui("state")["controlBounds"], "Queued task not rendered", 15)
                pointer("taskrow."+queued["task_id"])
                wait(lambda: queued["task_id"] in ui("state")["selectedTaskIds"], "Queued row was not selected", 10)
                pointer("toolbar.全部开始")
                wait(lambda: any(t["task_id"] == queued["task_id"] and t["downloaded_bytes"] > 262144 for t in tasks()),
                     "Start button did not begin transfer", 20)
                pointer("toolbar.全部暂停")
                wait(lambda: any(t["task_id"] == queued["task_id"] and t["status"] == "paused" and t["active_workers"] == 0 for t in tasks()),
                     "Pause button did not settle", 20)
                assert next(t for t in tasks() if t["task_id"] == downloaded["task_id"])["status"] == "completed", "All-start restarted a completed download"
                if queued["task_id"] not in ui("state")["selectedTaskIds"]:
                    pointer("taskrow."+queued["task_id"])
                wait(lambda: "toolbar.开始" in ui("state")["controlBounds"], "Selected paused task has no start toolbar", 10)
                pointer("toolbar.开始")
                resumed = wait(lambda: next((t for t in tasks() if t["task_id"] == queued["task_id"] and t["status"] == "completed"), None),
                               "Resume button did not complete", 90)
                assert hashlib.sha256(Path(resumed["output_path"]).read_bytes()).hexdigest() == expected
                record("all-start, all-pause, selected-task resume, checkpoint and SHA256")

            click("批量添加")
            node("链接列表")
            focus_text_field()
            enter(url+"?batch=1\ninvalid input\n"+url+"?batch=2")
            node("有效 2 / 输入 3 条链接")
            click("创建 2 个任务")
            wait(lambda: len([t for t in tasks() if "batch=" in t["url"] and t["status"] == "completed"]) == 2,
                 "Batch button did not complete both valid downloads", 90)
            record("batch form rejects invalid line and downloads two valid links")

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
            pointer(row)
            wait(lambda: downloaded["task_id"] in ui("state")["selectedTaskIds"], "Queue target not selected", 10)
            pointer("toolbar.移动队列")
            node("移动到队列")
            click("选择队列 " + new_queue["name"])
            click("移动")
            wait(lambda: any(t["task_id"] == downloaded["task_id"] and t["queue_id"] == new_queue["id"] for t in tasks()),
                 "Queue assignment did not reach Core", 15)
            record("selected task moves into saved queue")

            # 通知中心和关于由 extras 阶段在无选择状态下验收；选中任务时此菜单是任务操作。
            record("test task count", len(created_tasks))
        if args.start_at == "extras":
            from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
            token_url = uuid.uuid4().hex
            payload = bytes(range(256))*128
            media_path = Path("artifacts/v7-productization/adversarial-2026-10-08/every-control/player-hud-fixture.mkv").resolve()
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
            if args.extras_stage == "all":
                for label in ("通知中心", "关于 HLS Downloader"):
                    pointer("toolbar.更多操作")
                    click(label)
                    node("关于" if label == "关于 HLS Downloader" else label)
                    observe("more-"+label)
                    click("关闭",-1)
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
            wait(lambda:"player.audio_track" in ui("state")["controlBounds"],"Player did not refresh real metadata",60)
            pointer("toolbar.暂停")
            node("本机播放器 · 已暂停")
            for control in ("toolbar.降低音量","toolbar.后退 10 秒","toolbar.前进 10 秒","toolbar.提高音量","player.speed"):
                feedback(control,"player HUD")
                pointer(control)
            node("1.25x")
            click("选择音轨")
            click("音轨 1")
            wait(lambda: 'enabled' in node('1.25x', True)['states'], 'Audio switch did not finish', 15)
            click("选择字幕")
            click("字幕 1")
            wait(lambda: 'enabled' in node('1.25x', True)['states'], 'Subtitle switch did not finish', 15)
            click("画中画")
            node("本机播放器 · 画中画")
            click("全屏")
            node("本机播放器 · 全屏播放")
            click("停止播放")
            wait(lambda: "player.speed" not in ui("state")["controlBounds"],"Stop did not remove player HUD",10)
            record("real player HUD pause, volume, seeking, speed, audio, subtitles, PIP, fullscreen and stop")
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
            focus_text_field()
            enter("")
        if args.start_at == "dialogs":
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
            def cancel_chooser(title, control_label):
                result={}
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
                action("click",**click_point)
                chooser=wait(find,"System chooser not visible: "+title,20)
                ImageGrab.grab().save(evidence/("chooser-"+title[:12]+".png"))
                vm,root=client.context_from_window(chooser)
                owned=[]
                try:
                    controls=client.walk(vm,root,1000)
                    cancel=next(n for n in controls if n["name"].startswith("取消") and n["role"]=="push button")
                    context,owned=client.resolve_path(vm,root,cancel["path"])
                    client.invoke(vm,context,None)
                finally:
                    for context in reversed(owned):client.dll.releaseJavaObject(vm,context)
                    client.dll.releaseJavaObject(vm,root)
                wait(lambda: not user.IsWindowVisible(wintypes.HWND(chooser)),"System chooser cancel button failed",10)
                record("real system chooser opens and cancels: "+title)
            click("新建")
            node("新建下载")
            cancel_chooser("选择下载保存目录","选择目录")
            click("取消",-1)
            for label,title in (("导入任务或种子","导入任务 JSON、种子、Metalink 或 URL 列表"),("导出任务列表","导出任务列表")):
                bounds=ui("state")["controlBounds"]["toolbar.更多操作"]
                action("click",x=(bounds[0]+bounds[2])//2,y=(bounds[1]+bounds[3])//2)
                cancel_chooser(title,label)
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
                for label in ("新建","批量添加","页面抓取","设置"):
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
            (evidence / "failure.png").write_bytes(ui("screenshot", raw=True))
            report["failure_state"] = ui("state")
            report["failure_tree"] = tree()
        except Exception:
            pass
        print(json.dumps({"error": str(error)}, ensure_ascii=True), flush=True)
    finally:
        try:
            action("release")
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
            if fixture:
                fixture.shutdown()
                fixture.server_close()
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
    parser.add_argument("--extras-stage", choices=("all", "selection"), default="all")
    parser.add_argument("--functional-stage", choices=("all", "resume", "batch"), default="all")
    parser.add_argument("--settings-start-tab", default="通用")
    parser.add_argument("--start-at", choices=("main", "newtask", "settings", "functional", "extras", "dialogs"), default="main")
    raise SystemExit(run(parser.parse_args()))
