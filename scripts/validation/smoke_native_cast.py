"""Real Core and native UI against a receiver on this machine only.

Uses a configured TVBox endpoint; multicast discovery and real-device behavior
are deliberately not asserted. The receiver fetches the Core proxy with Range.
"""
from __future__ import annotations

import argparse
import hashlib
import http.server
import ipaddress
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request

import psutil
from v7_core_ipc import connect, free_port, send_frame


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("engine", "ui", "report"):
        parser.add_argument(f"--{option}", required=True, type=Path)
    parser.add_argument("--receiver-host", required=True)
    parser.add_argument("--hold-seconds", type=int, default=0)
    parser.add_argument("--push-fixture", action="store_true", help="Restore a pending browser-style push for GUI device selection")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    if not args.report.resolve().is_relative_to(root):
        parser.error("report must remain in the workspace")
    host = str(ipaddress.IPv4Address(args.receiver_host))
    local_ips = {address.address for values in psutil.net_if_addrs().values()
                 for address in values if address.family == socket.AF_INET}
    if host not in local_ips or host.startswith("169.254."):
        parser.error("receiver-host must be a usable IPv4 address owned by this machine")
    for path in (args.engine, args.ui):
        if not path.is_file():
            parser.error(f"missing executable: {path}")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    temporary = root / ".tool-cache/test-tmp"
    temporary.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, scope="real Core + native UI + configured simulated TVBox on this machine",
                  multicast_discovery=False, real_device=False, receiver_host=host,
                  binaries={name: hashlib.sha256(getattr(args, name).read_bytes()).hexdigest()
                            for name in ("engine", "ui")}, pushes=[], receiver_errors=[])

    def save():
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")

    payload = bytes(range(256)) * 4096
    fetched = threading.Event()
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    class Receiver(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            if self.path != "/source.mp4":
                self.send_error(404)
                return
            header = self.headers.get("Range", "")
            start, end = 0, len(payload)-1
            if header:
                try:
                    assert header.startswith("bytes=") and "," not in header
                    first, last = header[6:].split("-")
                    start = int(first)
                    end = min(int(last) if last else len(payload)-1, len(payload)-1)
                    assert 0 <= start <= end
                except (ValueError, AssertionError):
                    self.send_error(416)
                    return
            self.send_response(206 if header else 200)
            self.send_header("Content-Type", "video/mp4")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(end-start+1))
            if header:
                self.send_header("Content-Range", f"bytes {start}-{end}/{len(payload)}")
            self.end_headers()
            self.wfile.write(payload[start:end+1])

        def do_POST(self):
            try:
                assert self.path == "/action", self.path
                fields = urllib.parse.parse_qs(self.rfile.read(int(self.headers.get("Content-Length", "0"))).decode())
                media_url = fields["url"][0]
                report["pushes"].append(dict(method="POST", path=self.path, url=media_url))
                # This is a local fixture mount, not a browser source URL.
                assert urllib.parse.urlsplit(media_url).hostname == host
                with opener.open(urllib.request.Request(media_url, headers={"Range": "bytes=4096-8191"}), timeout=10) as response:
                    assert response.status == 206
                    assert response.read() == payload[4096:8192]
                report["receiver_range_hash"] = hashlib.sha256(payload[4096:8192]).hexdigest()
                fetched.set()
                body = b'{"code":200,"msg":"success"}'
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            except Exception as error:
                report["receiver_errors"].append(str(error))
                self.send_error(500)

    server = http.server.ThreadingHTTPServer((host, 0), Receiver)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="hls-native-cast-", dir=temporary) as name:
            directory = Path(name)
            media = directory / "local-fixture.mp4"
            media.write_bytes(payload)
            port = free_port()
            environment = dict(os.environ, HLS_V7_DATA_DIR=str(directory / "data"),
                               HLS_V7_DOWNLOAD_DIR=str(directory / "downloads"),
                               HLS_V7_PIPE=rf"\\.\pipe\HLSDownloader.native-cast.{os.getpid()}",
                               HLS_V7_CORE_TCP="1", HLS_V7_CORE_BIND=f"127.0.0.1:{port}",
                               HLS_V7_CAST_NULL="1")
            with args.report.with_suffix(".core.log").open("w", encoding="utf-8") as core_log, \
                 args.report.with_suffix(".ui.log").open("w", encoding="utf-8") as ui_log:
                engine = subprocess.Popen([str(args.engine.resolve())], env=environment, stdout=core_log, stderr=core_log)
                ui, stream = None, None
                counter = 0

                def request(kind, allow_error=False, **fields):
                    nonlocal counter
                    counter += 1
                    response = send_frame(stream, dict(type=kind, request_id=counter, **fields))
                    if not allow_error:
                        assert response["type"] != "error", response
                        assert not any(e["event"]["kind"] == "error" for e in response.get("events", [])), response
                    return response

                def command(kind, **fields):
                    return request("command", command=dict(kind=kind, **fields))

                def cast_event(response):
                    return next(e["event"] for e in response["events"] if e["event"]["kind"] == "cast_session")

                try:
                    stream = connect(port)
                    endpoint = f"http://{host}:{server.server_address[1]}"
                    request("store_settings", values=dict(legal_terms_accepted=True, tvbox_endpoint=endpoint,
                            download_progress_window_enabled=False, download_complete_popup_enabled=False))
                    discovered = command("discover_cast_devices", mode="tvbox")
                    devices = next(e["event"]["devices"] for e in discovered["events"] if e["event"]["kind"] == "cast_devices")
                    assert any(d["id"] == "tvbox:configured" and d["control_url"] == endpoint for d in devices), devices
                    current = cast_event(command("share_media", path=str(media), url="", title="原生投屏本地夹具",
                                                 device_id="tvbox:configured", request_id=""))
                    assert fetched.wait(3), report
                    assert current["active"] and current["device_kind"] == "tvbox"
                    assert current["media_url"] == report["pushes"][-1]["url"], current
                    previous_url = current["media_url"]
                    previous = current["session_generation"]
                    current = cast_event(command("share_media", path=str(media), url="", title="第二次投屏会话",
                                                 device_id="tvbox:configured", request_id=""))
                    assert current["session_generation"] > previous
                    assert current["media_url"] == report["pushes"][-1]["url"], current
                    try:
                        opener.open(previous_url, timeout=5)
                        raise AssertionError("replaced media mount is still available")
                    except urllib.error.HTTPError as error:
                        assert error.code == 404, error
                    refused = request("cast_control", allow_error=True, session_generation=previous, action="stop")
                    assert refused["type"] == "error" and refused["code"] == "cast_control_failed", refused
                    status = cast_event(command("control_cast", action="status"))
                    assert status["active"] and status["session_generation"] == current["session_generation"]
                    for key in ("media_url", "title", "task_id"):
                        assert status[key] == current[key], (key, status, current)
                    report["core_roundtrip"] = dict(configured_device=True, receiver_range_fetch=True,
                                                    stale_stop_rejected=True, current_session_preserved=True,
                                                    source_restored=True, replaced_mount_revoked=True)
                    push_id = f"native-cast-push-{os.getpid()}" if args.push_fixture else None
                    if push_id:
                        command("request_media_push", request=dict(id=push_id, push_kind="tvbox",
                                url=f"{endpoint}/source.mp4", title="浏览器媒体推送恢复夹具", mime_type="video/mp4",
                                status="pending", message="", location="", created_at_ms=int(time.time()*1000)))
                    ui = subprocess.Popen([str(args.ui.resolve()), "--workbench"], env=environment, stdout=ui_log, stderr=ui_log)
                    report.update(ui_pid=ui.pid, engine_pid=engine.pid)
                    # Read-only API fixture for the GUI: its configured-device
                    # selector and source dialog are tested through Computer Use.
                    save()
                    print(json.dumps(dict(ui_pid=ui.pid, engine_pid=engine.pid)), flush=True)
                    deadline = time.monotonic() + min(max(args.hold_seconds, 0), 600)
                    while time.monotonic() < deadline and ui.poll() is None:
                        time.sleep(.3)
                    # A long manual GUI hold can outlive Core's idle TCP
                    # connection. Reconnect before querying committed state;
                    # never replay the preceding share/push mutation.
                    stream.close()
                    stream = connect(port)
                    if push_id:
                        rows = request("load_handoffs", handoff_id=push_id, pending_only=False)["items"]
                        push = next(json.loads(row) for row in rows if json.loads(row).get("id") == push_id)
                        report["restored_push"] = dict(id=push_id, status=push["status"], message=push["message"])
                        assert push["status"] in ("done", "canceled"), push
                    status = cast_event(command("control_cast", action="status"))
                    if status["active"]:
                        stopped = cast_event(request("cast_control", session_generation=status["session_generation"], action="stop"))
                        assert not stopped["active"]
                        assert not stopped["media_url"] and not stopped["task_id"] and not stopped["title"], stopped
                        try:
                            opener.open(status["media_url"], timeout=5)
                            raise AssertionError("stopped media mount is still available")
                        except urllib.error.HTTPError as error:
                            assert error.code == 404, error
                        report["stopped_mount_revoked"] = True
                    idle = cast_event(command("control_cast", action="status"))
                    assert not idle["active"] and not idle["media_url"] and not idle["supported_actions"], idle
                    report["idle_status_after_stop"] = True
                    report["session_stopped"] = True
                    report["passed"] = not report["receiver_errors"]
                finally:
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
                    report["process_cleanup"] = dict(engine_stopped=engine.poll() is not None,
                                                     ui_stopped=ui is None or ui.poll() is not None)
        report["workspace_cleanup"] = dict(complete=True)
    except Exception as error:
        report.update(passed=False, error=str(error))
        raise
    finally:
        server.shutdown()
        server.server_close()
        save()


if __name__ == "__main__":
    main()
