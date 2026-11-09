"""Exercise the real native UI against a bounded, isolated IPC fault fixture.

This checks recovery/activation, not production downloads. No user profile,
network service, or production Core is opened by the fixture.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import socketserver
import struct
import subprocess
import tempfile
import threading
import time


class Fixture:
    def __init__(self, directory: Path, media: bool = False, visual: bool = False, empty: bool = False):
        self.directory = directory
        self.lock = threading.Lock()
        self.snapshots = 0
        self.waits: list[int] = []
        self.mutations: list[dict] = []
        self.recovered = threading.Event()
        self.errors: list[str] = []
        self.media = media
        self.media_sequence = 10001
        self.media_layout = 0
        self.media_paused = False
        self.media_position = 12.0
        self.media_active = media
        self.embeds: list[dict] = []
        self.settings = dict(type="settings", takeover_enabled=True,
                             takeover_minimum_bytes=0, legal_accepted=True,
                             speed_limit_kib=0, progress_window_enabled=False,
                             complete_popup_enabled=False, download_dir=str(directory))
        self.tasks = [dict(task_id=f"fixture-{index:04}", resource_kind="file",
                           status="paused", stage="paused", title="",
                           filename=f"原生测试视频 {index:04}.mp4", url=f"https://fixture.invalid/{index}.mp4",
                           downloaded_bytes=0, total_bytes=134217728,
                           speed_bytes_per_sec=0, eta_seconds=None, active_workers=0,
                           completed_ranges=0, total_ranges=16, playback_ready=False,
                           is_live=False, available_actions=["resume", "cancel", "delete"],
                           queue_index=index) for index in range(1000)]
        if visual:
            self.settings["queue_profiles"] = [
                dict(id="default", name="默认队列", priority=0),
                dict(id="video", name="视频收藏", priority=1),
            ]
            for task, (filename, status, actions, done) in zip(self.tasks, [
                ("旅行纪录片.mp4", "downloading", ["pause", "cancel", "delete"], 0.42),
                ("软件安装包.exe", "queued", ["start", "cancel", "delete"], 0),
                ("资料归档.zip", "paused", ["resume", "cancel", "delete"], 0.67),
                ("项目说明.pdf", "completed", ["open", "open_folder", "delete"], 1),
                ("课程录屏.mp4", "failed", ["retry", "delete"], 0.23),
                ("音乐现场.m3u8", "downloading", ["pause", "cancel", "delete"], 0.76),
            ]):
                task.update(filename=filename, status=status, stage=status,
                            available_actions=actions, downloaded_bytes=int(task["total_bytes"] * done),
                            speed_bytes_per_sec=3145728 if status == "downloading" else 0,
                            eta_seconds=42 if status == "downloading" else None,
                            queue_id="video" if filename.endswith((".mp4", ".m3u8")) else "default")
            self.tasks[5]["resource_kind"] = "hls"
        if empty:
            self.tasks.clear()

    def player_event(self) -> dict:
        self.media_sequence += 1
        return dict(sequence=self.media_sequence, event=dict(
            kind="player_session", active=self.media_active, title="原生媒体控制夹具（无视频解码）",
            session_generation=42, layout_generation=self.media_layout, task_id="fixture-0000",
            status="PAUSED" if self.media_paused else "PLAYING", paused=self.media_paused,
            position_seconds=self.media_position, duration_seconds=180.0,
            position_available=True, volume=60.0, muted=False, buffering=False,
            tracks=[dict(id="1", kind="audio", title="中文音轨", language="zh", selected=True),
                    dict(id="2", kind="sub", title="中文字幕", language="zh", selected=True)]))

    def request(self, request: dict) -> dict:
        kind, request_id = request["type"], request.get("request_id", 0)
        if kind == "hello":
            return dict(type="hello", protocol=request["protocol"], version=request["version"], pid=os.getpid())
        if kind == "capabilities":
            # Exercise the legacy snapshot path too; paging has its own
            # deterministic client regressions in crates/client.
            return dict(type="capabilities", request_id=request_id, product_version="7.0.3",
                        protocol_version=1, commands=["player_embed_generation", "player_control_generation"] if self.media else [], settings=[], max_frame_bytes=4194304)
        if kind == "snapshot":
            with self.lock:
                self.snapshots += 1
                sequence = [100, 5000, 10000, 20][min(self.snapshots - 1, 3)]
            return dict(type="snapshot", request_id=request_id, tasks=self.tasks,
                        latest_sequence=sequence, browser_connected=False)
        if kind == "load_handoffs":
            return dict(type="handoffs", request_id=request_id, items=[])
        if kind in ("load_settings", "store_settings"):
            if kind == "store_settings":
                with self.lock:
                    values = request["values"]
                    self.mutations.append(dict(kind=kind, values=values))
                    for key, value in values.items():
                        self.settings[{"download_speed_limit_kib": "speed_limit_kib",
                                       "queue_max_active": "queue_max"}.get(key, key)] = value
            return dict(self.settings, request_id=request_id)
        if kind == "command":
            with self.lock:
                self.mutations.append(request["command"])
                command = request["command"]
                if command.get("kind") == "set_setting":
                    key = {"download_speed_limit_kib": "speed_limit_kib", "queue_max_active": "queue_max"}.get(command["key"], command["key"])
                    self.settings[key] = command["value"]
                if self.media and command.get("kind") == "player_control":
                    action = command["action"]
                    if action == "pause": self.media_paused = True
                    if action == "resume": self.media_paused = False
                    if action == "stop": self.media_active = False
                    if action.startswith("seek_to:"): self.media_position = float(action[8:])
                    return dict(type="events", request_id=request_id, events=[self.player_event()])
            return dict(type="events", request_id=request_id, events=[])
        if kind == "player_embed" and self.media:
            with self.lock:
                embed = request["embed"]
                if embed["session_generation"] != 42 or embed["layout_generation"] <= self.media_layout:
                    self.errors.append("outdated media session/layout was submitted")
                    return dict(type="error", request_id=request_id, code="stale_embed", message="stale embed")
                self.media_layout = embed["layout_generation"]
                self.embeds.append(embed)
                return dict(type="events", request_id=request_id, events=[self.player_event()])
        if kind == "player_control" and self.media:
            with self.lock:
                if request["session_generation"] != 42:
                    self.errors.append("control targeted an outdated playback session")
                    return dict(type="error", request_id=request_id, code="stale_control", message="stale control")
                action = request["action"]
                self.mutations.append(dict(kind="player_control", action=action))
                if action == "pause": self.media_paused = True
                if action == "resume": self.media_paused = False
                if action == "stop": self.media_active = False
                if action.startswith("seek_to:"): self.media_position = float(action[8:])
                return dict(type="events", request_id=request_id, events=[self.player_event()])
        if kind == "wait_events":
            after = request["after_sequence"]
            with self.lock:
                self.waits.append(after)
                step = len(self.waits)
            expected = [5000, 10000, 20]
            if step <= 3 and after != expected[step - 1]:
                self.errors.append(f"poll {step}: expected {expected[step-1]}, received {after}")
                self.recovered.set()
            if step < 3:
                return dict(type="error", request_id=request_id, code="resync_required",
                            message="fixture: event ring covered / Core restarted")
            self.recovered.set()
            time.sleep(0.1)
            return dict(type="events", request_id=request_id, events=[])
        return dict(type="error", request_id=request_id, code="fixture_unsupported", message=kind)


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        # GUI holds may legitimately leave the command connection idle. The
        # explicit resync responses above provide the intended fault injection.
        self.connection.settimeout(900)
        try:
            while header := self.rfile.read(4):
                if len(header) != 4:
                    return
                length, = struct.unpack("<I", header)
                if length > 4194304:
                    raise ValueError("oversized fixture request")
                request = json.loads(self.rfile.read(length))
                response = self.server.fixture.request(request)
                payload = json.dumps(response, ensure_ascii=False).encode("utf-8")
                self.wfile.write(struct.pack("<I", len(payload)) + payload)
                self.wfile.flush()
        except (ConnectionError, OSError):
            pass


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ui", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--hold-seconds", type=int, default=0, help="Keep the isolated UI open for interaction checks")
    parser.add_argument("--media-fixture", action="store_true", help="Expose a simulated playback session; does not decode video")
    parser.add_argument("--lifecycle", action="store_true", help="Wait for a manual close, verify release, then reactivate the same UI process")
    parser.add_argument("--visual-fixture", action="store_true", help="Include mixed statuses, file categories and named queues for visual checks")
    parser.add_argument("--empty", action="store_true", help="Check the empty-workbench artwork with no tasks")
    parser.add_argument("--compact", action="store_true", help="Start at 1024 logical pixels using isolated saved window geometry")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    report_path = args.report.resolve()
    if not report_path.is_relative_to(root):
        parser.error("report must stay inside the project workspace")
    temporary_root = root / ".tool-cache/test-tmp"
    temporary_root.mkdir(parents=True, exist_ok=True)
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report = dict(scope="native-ui IPC fault fixture; no production download assertion",
                  ui_sha256=hashlib.sha256(args.ui.read_bytes()).hexdigest(), passed=False)
    try:
        with tempfile.TemporaryDirectory(prefix="hls-native-workbench-", dir=temporary_root) as name:
            directory = Path(name)
            if args.compact:
                (directory / "ui").mkdir()
                (directory / "ui/window-geometry.properties").write_text("width=1024\nheight=650\n", encoding="utf-8")
            fixture = Fixture(directory, args.media_fixture, args.visual_fixture, args.empty)
            with Server(("127.0.0.1", 0), Handler) as server:
                server.fixture = fixture
                threading.Thread(target=server.serve_forever, daemon=True).start()
                environment = dict(os.environ, HLS_V7_DATA_DIR=str(directory),
                                   HLS_V7_PIPE=rf"\\.\pipe\HLSDownloader.native-fixture.{os.getpid()}",
                                   HLS_V7_CORE_TCP="1", HLS_V7_CORE_BIND=f"127.0.0.1:{server.server_address[1]}",
                                   HLS_V7_PRESENTER_TRACE_FILE=str(directory / "presenter.trace"))
                with (report_path.with_suffix(".stderr.log")).open("w", encoding="utf-8") as log:
                    process = subprocess.Popen([str(args.ui.resolve()), "--workbench"], env=environment,
                                               stdout=log, stderr=log)
                    try:
                        if not fixture.recovered.wait(35):
                            raise AssertionError(f"UI recovery timed out (exit={process.poll()})")
                        if fixture.errors:
                            raise AssertionError("; ".join(fixture.errors))
                        duplicate = subprocess.run([str(args.ui.resolve()), "--workbench"], env=environment,
                                                   stdout=log, stderr=log, timeout=15)
                        if duplicate.returncode:
                            raise AssertionError(f"activation failed: {duplicate.returncode}")
                        if process.poll() is not None:
                            raise AssertionError("primary UI exited during activation")
                        report.update(passed=True, recovery_cursors=fixture.waits[:3],
                                      activation_forwarded=True, tasks=len(fixture.tasks), ui_pid=process.pid,
                                      visual_fixture=args.visual_fixture, compact_fixture=args.compact)
                        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
                        print(json.dumps(report, ensure_ascii=False), flush=True)
                        if args.lifecycle:
                            trace = directory / "presenter.trace"
                            deadline = time.monotonic() + min(max(args.hold_seconds, 30), 600)
                            while time.monotonic() < deadline and process.poll() is None:
                                if trace.exists() and "native workbench released" in trace.read_text(encoding="utf-8"):
                                    break
                                time.sleep(.2)
                            else:
                                raise AssertionError("workbench was not released after closing")
                            duplicate = subprocess.run([str(args.ui.resolve()), "--workbench"], env=environment,
                                                       stdout=log, stderr=log, timeout=15)
                            assert duplicate.returncode == 0 and process.poll() is None
                            deadline = time.monotonic() + 15
                            while time.monotonic() < deadline:
                                if trace.read_text(encoding="utf-8").count("native workbench created") >= 2:
                                    break
                                time.sleep(.1)
                            else:
                                raise AssertionError("activation did not rebuild the released workbench")
                            report["lifecycle"] = dict(controller_released=True, same_process_reactivated=True,
                                                       controller_recreated=True)
                            report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
                            print(json.dumps(report["lifecycle"]), flush=True)
                            time.sleep(10)
                        elif args.hold_seconds:
                            try:
                                process.wait(timeout=min(max(args.hold_seconds, 0), 600))
                            except subprocess.TimeoutExpired:
                                pass
                    finally:
                        if process.poll() is None:
                            process.terminate()
                            process.wait(timeout=10)
                        server.shutdown()
                        report.update(snapshots=fixture.snapshots, mutations=fixture.mutations,
                                      errors=fixture.errors, media_embeds=fixture.embeds,
                                      media_scope="simulated Core state, no libmpv assertion" if args.media_fixture else None,
                                      ui_stopped=process.poll() is not None)
                        if fixture.errors: report["passed"] = False
            report["workspace_cleanup"] = {"complete": True}
    except Exception as error:
        report.update(passed=False, error=str(error))
        raise
    finally:
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
