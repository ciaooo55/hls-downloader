#!/usr/bin/env python3
"""Inspect and operate the v7 Compose workbench through Java Access Bridge."""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import time
from ctypes import wintypes
from pathlib import Path


MAX_STRING_SIZE = 1024
SHORT_STRING_SIZE = 256
MAX_ACTION_INFO = 256
MAX_ACTIONS_TO_DO = 32
PM_REMOVE = 0x0001

JOBJECT64 = ctypes.c_int64


class AccessibleContextInfo(ctypes.Structure):
    _fields_ = [
        ("name", ctypes.c_wchar * MAX_STRING_SIZE),
        ("description", ctypes.c_wchar * MAX_STRING_SIZE),
        ("role", ctypes.c_wchar * SHORT_STRING_SIZE),
        ("role_en_US", ctypes.c_wchar * SHORT_STRING_SIZE),
        ("states", ctypes.c_wchar * SHORT_STRING_SIZE),
        ("states_en_US", ctypes.c_wchar * SHORT_STRING_SIZE),
        ("index_in_parent", ctypes.c_int32),
        ("children_count", ctypes.c_int32),
        ("x", ctypes.c_int32),
        ("y", ctypes.c_int32),
        ("width", ctypes.c_int32),
        ("height", ctypes.c_int32),
        ("accessible_component", wintypes.BOOL),
        ("accessible_action", wintypes.BOOL),
        ("accessible_selection", wintypes.BOOL),
        ("accessible_text", wintypes.BOOL),
        ("accessible_interfaces", wintypes.BOOL),
    ]


class AccessibleActionInfo(ctypes.Structure):
    _fields_ = [("name", ctypes.c_wchar * SHORT_STRING_SIZE)]


class AccessibleActions(ctypes.Structure):
    _fields_ = [
        ("actions_count", ctypes.c_int32),
        ("action_info", AccessibleActionInfo * MAX_ACTION_INFO),
    ]


class AccessibleActionsToDo(ctypes.Structure):
    _fields_ = [
        ("actions_count", ctypes.c_int32),
        ("actions", AccessibleActionInfo * MAX_ACTIONS_TO_DO),
    ]


class JabClient:
    def __init__(self, dll_path: Path) -> None:
        if os.name != "nt":
            raise RuntimeError("Java Access Bridge validation requires Windows")
        if not dll_path.is_file():
            raise RuntimeError(f"Windows Access Bridge DLL is missing: {dll_path}")
        self.dll_path = dll_path.resolve()
        self.dll = ctypes.WinDLL(str(self.dll_path))
        self.user32 = ctypes.WinDLL("user32", use_last_error=True)
        self._bind()

    def _bind(self) -> None:
        self.dll.Windows_run.argtypes = []
        self.dll.Windows_run.restype = None
        self.dll.isJavaWindow.argtypes = [wintypes.HWND]
        self.dll.isJavaWindow.restype = wintypes.BOOL
        self.dll.getAccessibleContextFromHWND.argtypes = [
            wintypes.HWND,
            ctypes.POINTER(ctypes.c_long),
            ctypes.POINTER(JOBJECT64),
        ]
        self.dll.getAccessibleContextFromHWND.restype = wintypes.BOOL
        self.dll.getAccessibleContextInfo.argtypes = [
            ctypes.c_long,
            JOBJECT64,
            ctypes.POINTER(AccessibleContextInfo),
        ]
        self.dll.getAccessibleContextInfo.restype = wintypes.BOOL
        self.dll.getAccessibleChildFromContext.argtypes = [ctypes.c_long, JOBJECT64, ctypes.c_int32]
        self.dll.getAccessibleChildFromContext.restype = JOBJECT64
        self.dll.getAccessibleActions.argtypes = [
            ctypes.c_long,
            JOBJECT64,
            ctypes.POINTER(AccessibleActions),
        ]
        self.dll.getAccessibleActions.restype = wintypes.BOOL
        self.dll.doAccessibleActions.argtypes = [
            ctypes.c_long,
            JOBJECT64,
            AccessibleActionsToDo,
            ctypes.POINTER(ctypes.c_int32),
        ]
        self.dll.doAccessibleActions.restype = wintypes.BOOL
        self.dll.releaseJavaObject.argtypes = [ctypes.c_long, JOBJECT64]
        self.dll.releaseJavaObject.restype = None

    def start(self) -> None:
        self.dll.Windows_run()
        self.pump_messages()

    def pump_messages(self) -> None:
        message = wintypes.MSG()
        while self.user32.PeekMessageW(ctypes.byref(message), None, 0, 0, PM_REMOVE):
            self.user32.TranslateMessage(ctypes.byref(message))
            self.user32.DispatchMessageW(ctypes.byref(message))

    def find_window(self, title_fragment: str, timeout: float) -> tuple[int, str]:
        deadline = time.monotonic() + timeout
        last_titles: list[str] = []
        while time.monotonic() < deadline:
            self.pump_messages()
            matches: list[tuple[int, str]] = []
            titles: list[str] = []
            callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

            @callback_type
            def callback(hwnd: int, _lparam: int) -> bool:
                length = self.user32.GetWindowTextLengthW(hwnd)
                if length <= 0 or not self.user32.IsWindowVisible(hwnd):
                    return True
                buffer = ctypes.create_unicode_buffer(length + 1)
                self.user32.GetWindowTextW(hwnd, buffer, len(buffer))
                title = buffer.value
                titles.append(title)
                if title_fragment.casefold() in title.casefold() and self.dll.isJavaWindow(hwnd):
                    matches.append((int(hwnd), title))
                return True

            self.user32.EnumWindows(callback, 0)
            if matches:
                return matches[0]
            last_titles = titles
            time.sleep(0.1)
        candidates = [title for title in last_titles if title_fragment.casefold() in title.casefold()]
        raise RuntimeError(f"Java window not found for {title_fragment!r}; title matches={candidates}")

    def context_from_window(self, hwnd: int) -> tuple[int, int]:
        vm_id = ctypes.c_long()
        context = JOBJECT64()
        if not self.dll.getAccessibleContextFromHWND(hwnd, ctypes.byref(vm_id), ctypes.byref(context)):
            raise RuntimeError("getAccessibleContextFromHWND failed")
        if vm_id.value == 0 or context.value == 0:
            raise RuntimeError("Java Access Bridge returned an empty root context")
        return vm_id.value, context.value

    def actions(self, vm_id: int, context: int) -> list[str]:
        actions = AccessibleActions()
        if not self.dll.getAccessibleActions(vm_id, context, ctypes.byref(actions)):
            return []
        count = max(0, min(actions.actions_count, MAX_ACTION_INFO))
        return [actions.action_info[index].name for index in range(count)]

    def walk(self, vm_id: int, root: int, max_nodes: int) -> list[dict]:
        nodes: list[dict] = []

        def visit(context: int, depth: int, owned: bool, path: list[int]) -> None:
            try:
                if len(nodes) >= max_nodes:
                    raise RuntimeError(f"accessibility tree exceeds --max-nodes={max_nodes}")
                info = AccessibleContextInfo()
                if not self.dll.getAccessibleContextInfo(vm_id, context, ctypes.byref(info)):
                    raise RuntimeError(f"getAccessibleContextInfo failed at depth {depth}")
                node = {
                    "depth": depth,
                    "name": info.name,
                    "description": info.description,
                    "role": info.role_en_US or info.role,
                    "states": info.states_en_US or info.states,
                    "bounds": [info.x, info.y, info.width, info.height],
                    "children": info.children_count,
                    "actions": self.actions(vm_id, context) if info.accessible_action else [],
                    "path": path,
                }
                nodes.append(node)
                child_count = max(0, min(info.children_count, max_nodes - len(nodes)))
                for index in range(child_count):
                    child = int(self.dll.getAccessibleChildFromContext(vm_id, context, index))
                    if child:
                        visit(child, depth + 1, True, path + [index])
            finally:
                if owned:
                    self.dll.releaseJavaObject(vm_id, context)

        visit(root, 0, False, [])
        return nodes

    def resolve_path(self, vm_id: int, root: int, path: list[int]) -> tuple[int, list[int]]:
        context = root
        owned: list[int] = []
        for index in path:
            context = int(self.dll.getAccessibleChildFromContext(vm_id, context, index))
            if not context:
                for item in reversed(owned):
                    self.dll.releaseJavaObject(vm_id, item)
                raise RuntimeError(f"accessible path no longer exists: {path}")
            owned.append(context)
        return context, owned

    def invoke(self, vm_id: int, context: int, action_name: str | None) -> str:
        available = self.actions(vm_id, context)
        if not available:
            raise RuntimeError("target control exposes no accessible action")
        selected = action_name or available[0]
        match = next((value for value in available if value.casefold() == selected.casefold()), None)
        if match is None:
            raise RuntimeError(f"accessible action {selected!r} is unavailable; actions={available}")
        actions = AccessibleActionsToDo()
        actions.actions_count = 1
        actions.actions[0].name = match
        failure = ctypes.c_int32(-1)
        if not self.dll.doAccessibleActions(vm_id, context, actions, ctypes.byref(failure)):
            raise RuntimeError(f"accessible action failed at index {failure.value}")
        return match


def find_named_node(nodes: list[dict], query: str, actionable: bool = False) -> dict | None:
    folded = query.casefold()
    candidates = [node for node in nodes if not actionable or node["actions"]]
    exact = next((node for node in candidates if node["name"].casefold() == folded), None)
    return exact or next((node for node in candidates if folded in node["name"].casefold()), None)


def public_node(node: dict) -> dict:
    return node


def main() -> int:
    parser = argparse.ArgumentParser()
    java_home = os.environ.get("JAVA_HOME")
    if not java_home:
        repo_local = Path(__file__).resolve().parents[1] / ".tool-cache" / "build-cache" / "jdk-21"
        legacy = Path(r"E:\HLSDownloaderBuildCache\jdk-21")
        java_home = str(repo_local if (repo_local / "bin" / "java.exe").exists() else legacy)
    parser.add_argument("--dll", type=Path, default=Path(java_home) / "bin" / "WindowsAccessBridge-64.dll")
    parser.add_argument("--title", default="HLS Downloader")
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--max-nodes", type=int, default=5000)
    parser.add_argument("--min-nodes", type=int, default=20)
    parser.add_argument("--require-name", action="append", default=[])
    parser.add_argument("--forbid-name", action="append", default=[])
    parser.add_argument("--invoke-name")
    parser.add_argument("--invoke-action")
    parser.add_argument("--require-after-name", action="append", default=[])
    parser.add_argument("--forbid-after-name", action="append", default=[])
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.max_nodes < 1 or args.min_nodes < 1 or args.min_nodes > args.max_nodes:
        raise SystemExit("invalid node bounds")

    client = JabClient(args.dll)
    client.start()
    hwnd, title = client.find_window(args.title, args.timeout)
    vm_id, root = client.context_from_window(hwnd)
    try:
        tree_deadline = time.monotonic() + args.timeout
        while True:
            nodes = client.walk(vm_id, root, args.max_nodes)
            missing = [name for name in args.require_name if find_named_node(nodes, name) is None]
            forbidden = [name for name in args.forbid_name if find_named_node(nodes, name) is not None]
            if len(nodes) >= args.min_nodes and not missing and not forbidden:
                break
            if time.monotonic() >= tree_deadline:
                raise RuntimeError(
                    f"accessibility tree did not become ready: nodes={len(nodes)}, "
                    f"expected={args.min_nodes}, missing={missing}, forbidden={forbidden}"
                )
            client.dll.releaseJavaObject(vm_id, root)
            client.pump_messages()
            time.sleep(0.1)
            vm_id, root = client.context_from_window(hwnd)

        invocation = None
        after_nodes: list[dict] = []
        if args.invoke_name:
            target = find_named_node(nodes, args.invoke_name, actionable=True)
            if target is None:
                raise RuntimeError(f"actionable accessible target is missing: {args.invoke_name!r}")
            target_context, owned_contexts = client.resolve_path(vm_id, root, target["path"])
            try:
                invocation = {
                    "target": target["name"],
                    "action": client.invoke(vm_id, target_context, args.invoke_action),
                }
            finally:
                for context in reversed(owned_contexts):
                    client.dll.releaseJavaObject(vm_id, context)
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                client.pump_messages()
                time.sleep(0.05)
            client.dll.releaseJavaObject(vm_id, root)
            vm_id, root = client.context_from_window(hwnd)
            after_nodes = client.walk(vm_id, root, args.max_nodes)
            missing_after = [name for name in args.require_after_name if find_named_node(after_nodes, name) is None]
            if missing_after:
                raise RuntimeError(f"post-action accessible names are missing: {missing_after}")
            forbidden_after = [name for name in args.forbid_after_name if find_named_node(after_nodes, name) is not None]
            if forbidden_after:
                raise RuntimeError(f"post-action forbidden accessible names remain: {forbidden_after}")

        actionable = sum(bool(node["actions"]) for node in nodes)
        report = {
            "schema": 1,
            "passed": True,
            "bridge_dll": str(client.dll_path),
            "window": {"hwnd": hwnd, "title": title, "vm_id": vm_id},
            "summary": {
                "nodes": len(nodes),
                "named_nodes": sum(bool(node["name"]) for node in nodes),
                "actionable_nodes": actionable,
                "roles": sorted({node["role"] for node in nodes if node["role"]}),
            },
            "required_names": args.require_name,
            "forbidden_names": args.forbid_name,
            "invocation": invocation,
            "required_after_names": args.require_after_name,
            "forbidden_after_names": args.forbid_after_name,
            "nodes": [public_node(node) for node in nodes],
            "after_nodes": [public_node(node) for node in after_nodes],
        }
        serialized = json.dumps(report, ensure_ascii=False, indent=2)
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(serialized + "\n", encoding="utf-8")
        print(json.dumps({key: report[key] for key in ("passed", "window", "summary", "invocation")}, ensure_ascii=False, separators=(",", ":")))
        return 0
    finally:
        client.dll.releaseJavaObject(vm_id, root)


if __name__ == "__main__":
    raise SystemExit(main())
