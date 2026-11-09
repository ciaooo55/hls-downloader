"""Isolated real Core and native dialogs, with local resources for GUI review.

This harness never injects UI input. Computer Use performs the GUI actions;
the Core snapshots below check their persisted results independently.
"""
from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import urllib.parse
import zipfile

from v7_core_ipc import connect, free_port, send_frame


def clipboard_files():
    """Observe CF_HDROP after an explicit fixture copy; never log other clipboard data."""
    import ctypes
    user = ctypes.WinDLL("user32", use_last_error=True)
    shell = ctypes.WinDLL("shell32", use_last_error=True)
    user.OpenClipboard.argtypes = [ctypes.c_void_p]
    user.GetClipboardData.argtypes = [ctypes.c_uint]
    user.GetClipboardData.restype = ctypes.c_void_p
    shell.DragQueryFileW.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_wchar_p, ctypes.c_uint]
    if not user.OpenClipboard(None):
        raise AssertionError("fixture could not read the file clipboard")
    try:
        handle = user.GetClipboardData(15)
        if not handle:
            return []
        count = shell.DragQueryFileW(handle, 0xffffffff, None, 0)
        assert count <= 4096, "unexpected file clipboard size"
        paths = []
        for index in range(count):
            length = shell.DragQueryFileW(handle, index, None, 0)
            assert 0 < length <= 32767, "invalid clipboard path length"
            path = ctypes.create_unicode_buffer(length+1)
            assert shell.DragQueryFileW(handle, index, path, length+1) == length
            paths.append(path.value)
        return paths
    finally:
        user.CloseClipboard()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("engine", "ui", "report"):
        parser.add_argument(f"--{option}", required=True, type=Path)
    parser.add_argument("--hold-seconds", type=int, default=600)
    parser.add_argument("--require-ui-save", action="store_true")
    parser.add_argument("--file-transfers", action="store_true", help="Prepare real completed files and import lists for GUI clipboard/drag checks")
    parser.add_argument("--verify-caption", action="store_true", help="Check real Windows caption API results across Core theme changes; does not inject UI input")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    report_path = args.report.resolve()
    if not report_path.is_relative_to(root):
        parser.error("report must remain in the workspace")
    for path in (args.engine, args.ui):
        if not path.is_file():
            parser.error(f"missing executable: {path}")
    report_path.parent.mkdir(parents=True, exist_ok=True)
    finish = report_path.with_suffix(".finish")
    if finish.exists():
        parser.error("finish marker already exists; use a distinct report path")
    caption_trace = report_path.with_suffix(".caption.log")
    if args.verify_caption:
        if os.name != "nt":
            parser.error("caption verification requires Windows")
        if caption_trace.exists():
            parser.error("caption trace already exists; use a distinct report path")
    temporary = root / ".tool-cache/test-tmp"
    temporary.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, scope="real Core + native dialogs + loopback resources",
                  binaries={key: hashlib.sha256(getattr(args, key).read_bytes()).hexdigest() for key in ("engine", "ui")},
                  http_requests=[], settings_observations=[], gui_review={})
    if args.verify_caption and not args.hold_seconds and not args.require_ui_save:
        report.update(scope="real Core + native Windows caption API", gui_review=dict(visual_verified=False, ui_input=False))

    def save():
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")

    directory = None
    try:
        with tempfile.TemporaryDirectory(prefix="hls-native-tools-", dir=temporary) as name:
            directory = Path(name)
            archive = directory / "bundle.zip"
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
                output.writestr("docs/readme.txt", "Native tools fixture\n")
                output.writestr("media/notes.txt", "Archive metadata fixture\n")
            payload = bytes(range(256)) * 4096
            files = {
                "/index.html": (b'<html><body><a href="/bundle.zip">bundle.zip</a><a href="/sample.bin">sample.bin</a></body></html>', "text/html; charset=utf-8"),
                "/bundle.zip": (archive.read_bytes(), "application/zip"),
                "/sample.bin": (payload, "application/octet-stream"),
            }

            class Source(http.server.BaseHTTPRequestHandler):
                def log_message(self, *_):
                    pass

                def do_HEAD(self):
                    self.reply(False)

                def do_GET(self):
                    self.reply(True)

                def reply(self, body):
                    path = urllib.parse.urlsplit(self.path).path
                    item = files.get(path)
                    if item is None:
                        self.send_error(404)
                        return
                    data, content_type = item
                    start, end = 0, len(data)-1
                    range_header = self.headers.get("Range", "")
                    if range_header:
                        try:
                            assert range_header.startswith("bytes=") and "," not in range_header
                            first, last = range_header[6:].split("-")
                            start = int(first) if first else max(0, len(data)-int(last))
                            end = min(int(last), len(data)-1) if first and last else len(data)-1
                            assert 0 <= start <= end
                        except (ValueError, AssertionError):
                            self.send_error(416)
                            return
                    report["http_requests"].append(dict(method=self.command, path=path, range=range_header))
                    self.send_response(206 if range_header else 200)
                    self.send_header("Content-Type", content_type)
                    self.send_header("Content-Length", str(end-start+1))
                    self.send_header("Accept-Ranges", "bytes")
                    if range_header:
                        self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
                    self.end_headers()
                    if body:
                        self.wfile.write(data[start:end+1])

            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Source)
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            engine, ui, stream = None, None, None
            port = free_port()
            environment = dict(os.environ, HLS_V7_DATA_DIR=str(directory / "data"),
                               HLS_V7_DOWNLOAD_DIR=str(directory / "downloads"),
                               HLS_V7_PIPE=rf"\\.\pipe\HLSDownloader.native-tools.{os.getpid()}",
                               HLS_V7_CORE_TCP="1", HLS_V7_CORE_BIND=f"127.0.0.1:{port}")
            if args.verify_caption:
                environment["HLS_V7_PRESENTER_TRACE_FILE"] = str(caption_trace)
            counter = 0

            def request(kind, **fields):
                nonlocal counter
                counter += 1
                response = send_frame(stream, dict(type=kind, request_id=counter, **fields))
                assert response.get("type") != "error", response
                assert not any(e.get("event", {}).get("kind") == "error" for e in response.get("events", [])), response
                return response

            try:
                with report_path.with_suffix(".core.log").open("w", encoding="utf-8") as core_log, \
                     report_path.with_suffix(".ui.log").open("w", encoding="utf-8") as ui_log:
                    engine = subprocess.Popen([str(args.engine.resolve())], env=environment, stdout=core_log, stderr=core_log)
                    stream = connect(port)
                    request("store_settings", values=dict(legal_terms_accepted=True, dark_mode=False,
                            download_dir=str(directory / "downloads"), download_progress_window_enabled=False,
                            download_complete_popup_enabled=False, site_rules=json.dumps([dict(host="media.example.test", enabled=True)])))
                    request("set_site_rule_credential", host="media.example.test", cookie="native-tools-synthetic-cookie")
                    base = f"http://127.0.0.1:{server.server_address[1]}"
                    request("command", command=dict(kind="create_task", spec=dict(url=base+"/sample.bin",
                            resource_kind="file", filename="界面验收文件.bin", start_paused=True)))
                    if args.file_transfers:
                        for name, source in (("复制验收一.bin", "/sample.bin"), ("复制验收二.zip", "/bundle.zip")):
                            request("command", command=dict(kind="create_task", spec=dict(url=base+source,
                                    resource_kind="file", filename=name, allow_duplicate=True)))
                        deadline = time.monotonic()+30
                        while time.monotonic() < deadline:
                            completed = [task for task in request("snapshot")["tasks"] if task["status"] == "completed"]
                            if len(completed) == 2:
                                break
                            time.sleep(.1)
                        else:
                            raise AssertionError("file-transfer fixture downloads did not complete")
                        request("command", command=dict(kind="task_action", task_id=completed[0]["task_id"], action="copy_file"))
                        assert clipboard_files() == [completed[0]["output_path"]], "Core background file copy mismatch"
                        report["core_file_copy"] = dict(background_thread_owner=True, file_clipboard_roundtrip=True)
                        incoming = directory / "导入文件"
                        incoming.mkdir()
                        for index in (1, 3):
                            (incoming / f"{index}-任务列表.json").write_text(json.dumps(dict(schema="hls-downloader.tasks.v1", tasks=[
                                dict(url=base+f"/sample.bin?import={index}", filename=f"粘贴导入{index}.bin", start_paused=True)
                            ]), ensure_ascii=False), encoding="utf-8")
                        (incoming / "2-无效列表.json").write_text("invalid JSON", encoding="utf-8")
                        report["file_transfer_fixture"] = dict(incoming=str(incoming),
                            completed=[task["output_path"] for task in completed])
                    ui = subprocess.Popen([str(args.ui.resolve()), "--workbench"], env=environment, stdout=ui_log, stderr=ui_log)
                    report.update(ui_pid=ui.pid, engine_pid=engine.pid, port=port,
                                  fixtures=dict(page=base+"/index.html", archive=base+"/bundle.zip", local_archive=str(archive), download=base+"/sample.bin"),
                                  finish_file=str(finish))
                    save()
                    print(json.dumps({key: report[key] for key in ("ui_pid", "engine_pid", "fixtures", "finish_file")}, ensure_ascii=False), flush=True)
                    if args.verify_caption:
                        report["caption_api_checks"] = []
                        offset = 0
                        for dark in (False, True, False):
                            if report["caption_api_checks"]:
                                offset = len(caption_trace.read_text(encoding="utf-8"))
                                request("store_settings", values=dict(dark_mode=dark))
                            colors = {20: int(dark), 35: 0x2a2622 if dark else 0xfefdfc, 36: 0xf6f5f4 if dark else 0x2a170f}
                            expected = [f"native caption dark={str(dark).lower()} attribute={attribute} value={value:#010x} hresult=0x00000000" for attribute, value in colors.items()]
                            timeout = time.monotonic()+15
                            while time.monotonic() < timeout:
                                assert ui.poll() is None, "native UI exited during caption check"
                                trace = caption_trace.read_text(encoding="utf-8")[offset:] if caption_trace.exists() else ""
                                if all(line in trace for line in expected):
                                    break
                                time.sleep(.05)
                            else:
                                raise AssertionError(f"Windows did not accept caption colors for dark={dark}")
                            report["caption_api_checks"].append(dict(dark=dark, attributes=colors, hresult=0))
                            save()
                    deadline = time.monotonic()+min(max(args.hold_seconds, 0), 1800)
                    previous = None
                    while time.monotonic() < deadline and not finish.exists():
                        assert ui.poll() is None, "native UI exited during review"
                        settings = request("load_settings")
                        observation = {key: settings[key] for key in ("dark_mode", "task_column_layout", "toolbar_actions", "site_rules", "queue_profiles")}
                        assert "native-tools-synthetic-cookie" not in json.dumps(settings), "credential leaked in settings"
                        if observation != previous:
                            report["settings_observations"].append(observation)
                            previous = observation
                            save()
                        time.sleep(.5)
                    settings = request("load_settings")
                    report["final_settings"] = {key: settings[key] for key in ("dark_mode", "task_column_layout", "toolbar_actions", "site_rules", "queue_profiles")}
                    report["final_tasks"] = request("snapshot").get("tasks", [])
                    if args.file_transfers:
                        imported = [task for task in report["final_tasks"] if task["filename"] in ("粘贴导入1.bin", "粘贴导入3.bin")]
                        assert len(imported) == 2, "GUI file paste did not import both valid lists"
                        report["file_transfer_import"] = dict(valid_lists_imported=2, invalid_list_created_no_task=True)
                    report["downloaded_files"] = []
                    for task in report["final_tasks"]:
                        source = urllib.parse.urlsplit(task["url"])
                        if task["status"] != "completed" or source.netloc != urllib.parse.urlsplit(base).netloc:
                            continue
                        output = Path(task["output_path"])
                        actual = hashlib.sha256(output.read_bytes()).hexdigest()
                        expected = hashlib.sha256(files[source.path][0]).hexdigest()
                        assert actual == expected, "GUI-created download hash mismatch"
                        report["downloaded_files"].append(dict(task_id=task["task_id"], sha256=actual, expected=expected))
                    report["crawl_projects"] = request("crawl", request=dict(action="list")).get("projects", [])
                    rules = json.loads(settings["site_rules"])
                    if args.require_ui_save:
                        rule = next(rule for rule in rules if rule["host"] == "media.example.test")
                        report["ui_persistence_checks"] = dict(
                            site_rule_speed=rule.get("speed_limit_kib", 0) == 512, vault_reference=bool(rule.get("credential_ref")),
                            queue=any(p["id"] != "default" and p["name"] for p in settings["queue_profiles"]))
                        assert all(report["ui_persistence_checks"].values()), report["ui_persistence_checks"]
                    report["passed"] = True
            finally:
                if ui and ui.poll() is None:
                    ui.terminate()
                    ui.wait(timeout=10)
                if stream:
                    try:
                        request("command", command=dict(kind="shutdown"))
                    except Exception as error:
                        report["shutdown_reply"] = str(error)
                    stream.close()
                if engine:
                    try:
                        engine.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        engine.terminate()
                        engine.wait(timeout=10)
                server.shutdown()
                server.server_close()
                worker.join(timeout=5)
                if finish.exists():
                    finish.unlink()
                report["process_cleanup"] = dict(engine_stopped=engine is None or engine.poll() is not None,
                                                  ui_stopped=ui is None or ui.poll() is not None,
                                                  http_stopped=not worker.is_alive())
        report["workspace_cleanup"] = dict(complete=True)
    except Exception as error:
        report.update(passed=False, error=str(error))
        raise
    finally:
        if directory is not None:
            report["workspace_cleanup"] = dict(complete=not directory.exists())
        save()


if __name__ == "__main__":
    main()
