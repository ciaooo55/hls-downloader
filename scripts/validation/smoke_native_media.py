"""Isolated real Core, download, libmpv and native workbench regression.

The optional hold lets Computer Use exercise the actual UI. Window inspection
is read-only; this script never injects input or accesses production data.
"""
from __future__ import annotations

import argparse
import base64
import ctypes
from ctypes import wintypes
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import traceback

import psutil

from v7_core_ipc import connect, free_port, send_frame, wait_task
from smoke_v7_resource_boundaries import wait


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def video_windows(pid: int) -> list[dict]:
    """Observe playback HWND parentage, without controlling native windows."""
    user = ctypes.windll.user32
    callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.GetParent.argtypes = [wintypes.HWND]
    user.GetParent.restype = wintypes.HWND
    user.EnumWindows.argtypes = [callback, wintypes.LPARAM]
    user.EnumChildWindows.argtypes = [wintypes.HWND, callback, wintypes.LPARAM]
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user.IsWindowVisible.argtypes = [wintypes.HWND]
    found: dict[int, dict] = {}

    @callback
    def child(hwnd, _):
        owner = wintypes.DWORD()
        user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid:
            rect = wintypes.RECT()
            user.GetWindowRect(hwnd, ctypes.byref(rect))
            found[int(hwnd)] = dict(hwnd=int(hwnd), parent=int(user.GetParent(hwnd) or 0),
                                    visible=bool(user.IsWindowVisible(hwnd)),
                                    rect=[rect.left, rect.top, rect.right, rect.bottom])
        return True

    @callback
    def top(hwnd, _):
        child(hwnd, 0)
        user.EnumChildWindows(hwnd, child, 0)
        return True

    user.EnumWindows(top, 0)
    return list(found.values())


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("engine", "ui", "ffmpeg", "libmpv", "report"):
        parser.add_argument(f"--{option}", type=Path, required=True)
    parser.add_argument("--hold-seconds", type=int, default=0)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    report_path = args.report.resolve()
    if not report_path.is_relative_to(root):
        parser.error("report must stay in the project workspace")
    for option in ("engine", "ui", "ffmpeg", "libmpv"):
        if not getattr(args, option).is_file():
            parser.error(f"{option} is not a file")
    report_path.parent.mkdir(parents=True, exist_ok=True)
    temporary_root = root / ".tool-cache/test-tmp"
    temporary_root.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, scope="real local authenticated HTTP download + libmpv + native workbench",
                  binaries={key: sha256(getattr(args, key)) for key in ("engine", "ui", "ffmpeg", "libmpv")},
                  ui_observations=[], credential_scope="generated local fixture headers, no browser handoff assertion")
    report['audio_output_devices'] = ctypes.windll.winmm.waveOutGetNumDevs()
    report['audio_scope'] = 'Real libmpv audio decoding and track selection; no assertion of audible speaker output on the hosted machine'

    def save():
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")

    try:
        with tempfile.TemporaryDirectory(prefix="hls-native-media-", dir=temporary_root) as name:
            directory = Path(name)
            media = directory / "source.mp4"
            captions = directory / "captions.srt"
            captions.write_text("1\n00:00:00,000 --> 00:04:59,000\nNative media validation\n", encoding="utf-8")
            with report_path.with_suffix(".ffmpeg.log").open("w", encoding="utf-8") as log:
                subprocess.run([str(args.ffmpeg.resolve()), "-nostdin", "-y", "-f", "lavfi", "-i",
                                "testsrc2=size=320x180:rate=10:duration=300", "-f", "lavfi", "-i",
                                "anullsrc=r=48000:cl=mono:d=300", "-i", str(captions),
                                "-map", "0:v", "-map", "1:a", "-map", "1:a", "-map", "2:0",
                                "-c:v", "libx264", "-preset", "ultrafast", "-crf", "30",
                                "-c:a", "aac", "-b:a", "32k", "-c:s", "mov_text",
                                "-metadata:s:a:0", "language=zho", "-metadata:s:a:1", "language=eng",
                                "-movflags", "+faststart", str(media)], stdout=log, stderr=log, check=True, timeout=90)
            payload = media.read_bytes()
            http_requests = []
            required = {"Authorization": "Bearer native-fixture", "Cookie": "fixture=local",
                        "Referer": "https://native-fixture.invalid/page", "Origin": "https://native-fixture.invalid",
                        "User-Agent": "HLS-Native-Fixture/1"}

            class Handler(http.server.BaseHTTPRequestHandler):
                def log_message(self, *_):
                    pass

                def do_HEAD(self):
                    self.respond(False)

                def do_GET(self):
                    self.respond(True)

                def respond(self, body):
                    valid = self.path == "/media.mp4?signature=kept" and all(self.headers.get(k) == v for k, v in required.items())
                    http_requests.append(dict(method=self.command, authenticated=valid, range=self.headers.get("Range")))
                    if not valid:
                        self.send_error(403)
                        return
                    start, end = 0, len(payload)-1
                    if requested := self.headers.get("Range"):
                        first, last = requested.removeprefix("bytes=").split("-", 1)
                        start, end = int(first), int(last) if last else end
                        end = min(end, len(payload)-1)
                        self.send_response(206)
                        self.send_header("Content-Range", f"bytes {start}-{end}/{len(payload)}")
                    else:
                        self.send_response(200)
                    self.send_header("Content-Length", str(end-start+1))
                    self.send_header("Content-Type", "video/mp4")
                    self.send_header("ETag", '"native-media-fixture-v1"')
                    self.end_headers()
                    if body:
                        self.wfile.write(payload[start:end+1])

            class Server(http.server.ThreadingHTTPServer):
                def __exit__(self, *errors):
                    self.shutdown()
                    return super().__exit__(*errors)

            with Server(("127.0.0.1", 0), Handler) as http_server:
                threading.Thread(target=http_server.serve_forever, daemon=True).start()
                port = free_port()
                environment = dict(os.environ, HLS_V7_DATA_DIR=str(directory / "data"),
                                   HLS_V7_DOWNLOAD_DIR=str(directory / "downloads"),
                                   HLS_V7_PIPE=rf"\\.\pipe\HLSDownloader.native-media.{os.getpid()}",
                                   HLS_V7_CORE_TCP="1", HLS_V7_CORE_BIND=f"127.0.0.1:{port}",
                                   HLS_V7_LIBMPV=str(args.libmpv.resolve()), HLS_FFMPEG=str(args.ffmpeg.resolve()))
                environment.pop("HLS_V7_PLAYER_NULL", None)
                with report_path.with_suffix(".core.log").open("w", encoding="utf-8") as core_log, \
                     report_path.with_suffix(".ui.log").open("w", encoding="utf-8") as ui_log:
                    engine = subprocess.Popen([str(args.engine.resolve())], env=environment, stdout=core_log, stderr=core_log)
                    ui, stream, owned_children = None, None, []
                    request_id = 1

                    def request(kind, **fields):
                        nonlocal request_id
                        request_id += 1
                        response = send_frame(stream, dict(type=kind, request_id=request_id, **fields))
                        if response.get("type") == "error" or any(e.get("event", {}).get("kind") == "error" for e in response.get("events", [])):
                            raise AssertionError(response)
                        return response

                    def command(kind, **fields):
                        return request("command", command=dict(kind=kind, **fields))

                    def status():
                        response = command("player_control", action="status")
                        return next(e["event"] for e in response["events"] if e["event"]["kind"] == "player_session")

                    def control(action):
                        return request("player_control", session_generation=generation, action=action)

                    def wait_status(predicate):
                        def matches():
                            value = status()
                            return value if predicate(value) else None
                        return wait(matches, 5)

                    try:
                        stream = connect(port)
                        request("store_settings", values=dict(legal_terms_accepted=True,
                                download_dir=str(directory / "downloads"), download_complete_popup_enabled=False,
                                download_progress_window_enabled=False))
                        response = command("create_task", spec=dict(
                            url=f"http://127.0.0.1:{http_server.server_address[1]}/media.mp4?signature=kept",
                            resource_kind="file", filename="native-media.mp4", title="原生真实播放回归",
                            download_dir=str(directory / "downloads"), headers=required, start_paused=True, concurrency=2))
                        task_id = next(e["event"]["snapshot"]["task_id"] for e in response["events"] if e["event"]["kind"] == "task_created")
                        command("task_action", task_id=task_id, action="resume")
                        completed = wait_task(stream, task_id, lambda t: t["status"] in ("completed", "failed"), 100)
                        assert completed["status"] == "completed", completed
                        output = Path(completed["output_path"])
                        assert sha256(output) == sha256(media), "download hash mismatch"
                        assert all(r["authenticated"] for r in http_requests), http_requests
                        report.update(download=dict(sha256=sha256(output), bytes=output.stat().st_size,
                                                    http_requests=list(http_requests)), task_id=task_id)
                        # Exercise queue persistence through the real Core before GUI editing.
                        settings = request("load_settings")
                        profiles = settings["queue_profiles"]
                        profiles.append(dict(profiles[0], id="native-fixture", name="原生验证队列"))
                        request("store_settings", values=dict(queue_profiles=profiles))
                        assert request("load_settings")["queue_profiles"] == profiles
                        report["queue_core_roundtrip"] = True
                        # These operations stay local. No cloud session is started.
                        configured = request("subtitle", request=dict(action="configure", provider="qwencloud", api_key="native-fixture-key",
                            preferences=dict(enabled=False, source="auto", target="zh", offset=1.25, showSource=True, prefetch=False)))
                        assert configured["result"]["has_key"] and "native-fixture-key" not in json.dumps(configured)
                        reread = request("subtitle", request=dict(action="configuration"))["result"]
                        assert reread["preferences"]["offset"] == 1.25
                        cleared = request("subtitle", request=dict(action="configure", provider="qwencloud", clear_key=True))
                        assert not cleared["result"]["has_key"]
                        request("store_settings", values=dict(site_rules=json.dumps([dict(host="media.example.test", enabled=True)])))
                        request("set_site_rule_credential", host="media.example.test", cookie="fixture-secret-cookie", request_headers={"Authorization":"Bearer fixture-secret-auth"})
                        settings = request("load_settings")
                        assert "fixture-secret" not in json.dumps(settings)
                        assert "media.example.test" in settings["site_rules"]
                        request("set_site_rule_credential", host="media.example.test", clear=True)
                        request("set_default_cookie", cookie="fixture-default-cookie")
                        settings = request("load_settings")
                        assert settings["default_cookie_configured"] and "fixture-default-cookie" not in json.dumps(settings)
                        request("set_default_cookie", cookie="")
                        assert not request("load_settings")["default_cookie_configured"]
                        report["local_service_settings"] = dict(subtitle_key_save_read_clear=True, preferences_roundtrip=True,
                                                                 site_credential_save_clear=True, default_cookie_save_clear=True, plaintext_not_returned=True,
                                                                 cloud_session_started=False)
                        idle_cast = command("control_cast", action="status")
                        assert not any(e["event"]["kind"] == "error" for e in idle_cast["events"]), idle_cast
                        assert any(e["event"]["kind"] == "cast_session" and not e["event"]["active"]
                                   and not e["event"]["media_url"] for e in idle_cast["events"]), idle_cast
                        report["idle_cast_status"] = dict(inactive=True, no_error=True)
                        command("play_task", task_id=task_id)
                        deadline = time.monotonic()+30
                        while time.monotonic() < deadline:
                            current = status()
                            if current["active"] and current["position_available"] and current["duration_seconds"] > 290:
                                break
                            time.sleep(.1)
                        else:
                            raise AssertionError(f"libmpv did not load fixture: {current}")
                        generation = current["session_generation"]
                        assert current["audio_tracks"] >= 2 and current["subtitle_tracks"] >= 1, current
                        control("pause")
                        before = wait_status(lambda value: value['paused'])
                        time.sleep(.35)
                        after = status()
                        report['pause_observation'] = {'before': before, 'after': after}
                        assert after["paused"] and abs(after["position_seconds"]-before["position_seconds"]) < .15, report['pause_observation']
                        control("seek_to:30")
                        control("speed:1.5")
                        control("volume:40")
                        audio = [t for t in after["tracks"] if t["kind"] == "audio"]
                        subtitles = [t for t in after["tracks"] if t["kind"] == "sub"]
                        control(f"audio:{audio[-1]['id']}")
                        control(f"subtitle:{subtitles[0]['id']}")
                        current = wait_status(lambda value: abs(value['position_seconds']-30) < 2
                            and abs(value['speed']-1.5) < .01 and abs(value['volume']-40) < 1
                            and any(t['id'] == audio[-1]['id'] and t['selected']
                                    for t in value['tracks'] if t['kind'] == 'audio'))
                        report['track_selection_observation'] = current
                        assert abs(current["position_seconds"]-30) < 2 and abs(current["speed"]-1.5) < .01, current
                        assert abs(current["volume"]-40) < 1, current
                        assert any(t["id"] == audio[-1]["id"] and t["selected"] for t in current["tracks"] if t["kind"] == "audio")
                        thumbnail = request("player_thumbnail", task_id=task_id, seconds=30)
                        assert base64.b64decode(thumbnail["jpeg_base64"]).startswith(b"\xff\xd8")
                        report["real_player_controls"] = dict(pause_stable=True, seek=True, speed=True, volume=True,
                                                              audio_track=True, thumbnail_bytes=len(base64.b64decode(thumbnail["jpeg_base64"])))
                        ui = subprocess.Popen([str(args.ui.resolve()), "--workbench"], env=environment, stdout=ui_log, stderr=ui_log)
                        report.update(ui_pid=ui.pid, engine_pid=engine.pid, session_generation=generation)
                        deadline = time.monotonic()+25
                        while time.monotonic() < deadline:
                            owned_children = psutil.Process(engine.pid).children(recursive=True)
                            players = [p for p in owned_children if "--player-process" in p.cmdline()]
                            if players:
                                windows = video_windows(players[0].pid)
                                if any(w["parent"] and w["visible"] for w in windows) and not any(w["visible"] and not w["parent"] for w in windows):
                                    break
                            time.sleep(.1)
                        else:
                            raise AssertionError("real player did not embed into native workbench")
                        player = players[0]
                        report["player_pid"] = player.pid
                        report["initial_embedded_windows"] = windows
                        embedded = status()
                        assert embedded["session_generation"] == generation and embedded["paused"], embedded
                        assert abs(embedded["position_seconds"]-current["position_seconds"]) < .2, embedded
                        report["late_embed_preserves_session"] = True
                        control("resume")
                        save()
                        print(json.dumps({key: report[key] for key in ("ui_pid", "engine_pid", "player_pid", "session_generation")}), flush=True)
                        deadline = time.monotonic()+min(max(args.hold_seconds, 0), 600)
                        previous = None
                        while time.monotonic() < deadline and ui.poll() is None:
                            current = status()
                            windows = video_windows(player.pid) if player.is_running() else []
                            observation = {key: current[key] for key in ("active", "session_generation", "layout_generation", "paused", "speed", "volume")}
                            observation["windows"] = windows
                            if observation != previous:
                                report["ui_observations"].append(dict(observation, position_seconds=current["position_seconds"]))
                                previous = observation
                                save()
                            time.sleep(.3)
                        assert len(http_requests) == len(report["download"]["http_requests"]), "playback unexpectedly redownloaded source"
                        report["playback_extra_source_requests"] = 0
                        settings_after_ui = request("load_settings")
                        report["queue_after_ui"] = settings_after_ui["queue_profiles"]
                        report["appearance_after_ui"] = {key: settings_after_ui[key] for key in ("task_column_layout", "toolbar_actions", "dark_mode")}
                        report["passed"] = True
                    finally:
                        owned_children = psutil.Process(engine.pid).children(recursive=True) if engine.poll() is None else owned_children
                        if ui and ui.poll() is None:
                            ui.terminate()
                            ui.wait(timeout=10)
                        if stream:
                            try:
                                command("shutdown")
                            except Exception as error:
                                report["shutdown_reply"] = str(error)
                            stream.close()
                        try:
                            engine.wait(timeout=15)
                        except subprocess.TimeoutExpired:
                            engine.terminate()
                            engine.wait(timeout=10)
                        for child in owned_children:
                            try:
                                if child.is_running():
                                    child.terminate()
                                    child.wait(timeout=10)
                            except psutil.NoSuchProcess:
                                pass
                        report["process_cleanup"] = dict(engine_stopped=engine.poll() is not None,
                                                         ui_stopped=ui is None or ui.poll() is not None)
        report["workspace_cleanup"] = dict(complete=True)
    except Exception:
        report.update(passed=False, error=traceback.format_exc())
        raise
    finally:
        save()


if __name__ == "__main__":
    main()
