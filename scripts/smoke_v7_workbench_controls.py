"""Exercise the installed workbench through Windows accessibility and real keyboard input."""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
from pathlib import Path
import threading
import time
import uuid

from PIL import ImageGrab

from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, RangeHandler, Server
from smoke_v7_accessibility import JabClient
from smoke_v7_portable_app import Pipe


def run(args):
    args.report.parent.mkdir(parents=True, exist_ok=True)
    client = JabClient(args.dll.resolve())
    client.start()
    hwnd, _ = client.find_window("HLS Downloader", 30)
    pipe = Pipe()
    server = None
    created = set()
    result = {"passed": False, "checks": []}
    initial = {t["task_id"] for t in pipe.request({"type": "snapshot", "request_id": 1})["tasks"]}
    token = uuid.uuid4().hex

    def tree():
        client.pump_messages()
        vm, root = client.context_from_window(hwnd)
        try:
            return client.walk(vm, root, 5000)
        finally:
            client.dll.releaseJavaObject(vm, root)

    def wait(predicate, description, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(.1)
        raise AssertionError(description)

    def require(name):
        return wait(lambda: next((n for n in tree() if name in n["name"]), None), f"missing UI: {name}")

    def click(name):
        vm, root = client.context_from_window(hwnd)
        owned = []
        try:
            node = next(n for n in client.walk(vm, root, 5000) if n["name"] == name and n["actions"])
            context, owned = client.resolve_path(vm, root, node["path"])
            client.invoke(vm, context, None)
        finally:
            for context in reversed(owned):
                client.dll.releaseJavaObject(vm, context)
            client.dll.releaseJavaObject(vm, root)

    def record(name):
        result["checks"].append(name)
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
        print(name, flush=True)

    try:
        if any(n["name"] == "保存设置" for n in tree()):
            click("取消")
            require("下载任务列表")
        click("设置")
        for tab, content in [("通用", "文件与运行"), ("下载", "默认并发"),
                             ("计划", "限速与队列计划"), ("网络", "代理与站点"),
                             ("媒体", "媒体处理与直播"), ("BT", "BT 与种子"),
                             ("安全", "发布与扫描"), ("浏览器", "浏览器下载接管"),
                             ("外观", "外观与可访问性")]:
            click(tab)
            require(content)
            record("settings: " + tab)
        click("取消")
        require("下载任务列表")
        for button, content, close in [("新建", "新建下载", "取消"),
                                      ("批量添加", "链接列表", "取消"),
                                      ("页面抓取", "抓取本页链接", "取消"),
                                      ("管理队列", "保存队列", "取消"),
                                      ("插件", "浏览器插件", "关闭")]:
            click(button)
            require(content)
            ImageGrab.grab().save(args.report.parent / ("controls-" + button + ".png"))
            click(close)
            require("下载任务列表")
            record("open and close: " + button)

        server = Server(("127.0.0.1", 0), RangeHandler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        url = f"http://127.0.0.1:{server.server_port}/ui-{token}.bin"
        click("新建")
        require("等待输入")
        # 新建对话框将键盘焦点放在 URL 输入框；只向当前工作台发送真实键盘事件。
        user = ctypes.windll.user32
        user.SetForegroundWindow(wintypes.HWND(hwnd))
        if user.GetForegroundWindow() != hwnd:
            raise RuntimeError("workbench did not take keyboard focus")

        class Key(ctypes.Structure):
            _fields_ = [("vk", wintypes.WORD), ("scan", wintypes.WORD), ("flags", wintypes.DWORD),
                        ("time", wintypes.DWORD), ("extra", ctypes.c_size_t)]

        class Mouse(ctypes.Structure):
            _fields_ = [("x", wintypes.LONG), ("y", wintypes.LONG), ("data", wintypes.DWORD),
                        ("flags", wintypes.DWORD), ("time", wintypes.DWORD), ("extra", ctypes.c_size_t)]

        class Union(ctypes.Union):
            _fields_ = [("key", Key), ("mouse", Mouse)]

        class Input(ctypes.Structure):
            _fields_ = [("kind", wintypes.DWORD), ("payload", Union)]

        events = []
        for char in url:
            for flags in (4, 6):
                event = Input(kind=1)
                event.payload.key = Key(0, ord(char), flags, 0, 0)
                events.append(event)
        array = (Input * len(events))(*events)
        if user.SendInput(len(events), array, ctypes.sizeof(Input)) != len(events):
            raise ctypes.WinError()
        require("类型：")
        click("创建下载")

        def task():
            snapshot = pipe.request({"type": "snapshot", "request_id": 2})
            match = next((t for t in snapshot["tasks"] if t["task_id"] not in initial and token in json.dumps(t)), None)
            if match:
                created.add(match["task_id"])
            return match

        downloaded = wait(lambda: (t if (t := task()) and t["status"] == "completed" else None),
                          "GUI-created download did not complete", 90)
        output = Path(downloaded["output_path"])
        expected = hashlib.sha256(BLOCK * (PAYLOAD_SIZE // len(BLOCK))).hexdigest()
        if output.stat().st_size != PAYLOAD_SIZE or hashlib.sha256(output.read_bytes()).hexdigest() != expected:
            raise AssertionError("GUI download integrity mismatch")
        require(output.name)
        ImageGrab.grab().save(args.report.parent / "controls-download-complete.png")
        record("real keyboard URL -> create button -> Core -> 32MiB SHA256 -> displayed completion")
        result["passed"] = True
    except Exception as error:
        result["error"] = str(error)
        ImageGrab.grab().save(args.report.parent / "controls-failure.png")
        print(str(error), flush=True)
    finally:
        for task_id in created:
            pipe.request({"type": "command", "request_id": 3,
                          "command": {"kind": "task_action", "task_id": task_id, "action": "delete_files"}})
        pipe.close()
        if server:
            server.shutdown()
            server.server_close()
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--dll", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
