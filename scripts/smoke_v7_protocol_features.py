"""Run the installed Core against real local FTP/SFTP/HTTP servers and task files."""
from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
from ftplib import FTP, FTP_TLS
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import logging
import os
from pathlib import Path
import socket
import ssl
import shutil
import subprocess
import tempfile
import threading
import time
import traceback
import uuid
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener

import paramiko
from pyftpdlib.authorizers import DummyAuthorizer
from pyftpdlib.handlers import FTPHandler, ThrottledDTPHandler
from pyftpdlib.servers import FTPServer

from msi_checkpoint_fixture import connect, send, snapshot, wait_task
from smoke_v7_transfer_performance import free_port


def run(args):
    args.report.parent.mkdir(parents=True, exist_ok=True)
    result = {"passed": False, "engine_sha256": hashlib.sha256(args.engine.read_bytes()).hexdigest(), "checks": []}
    cache = Path.cwd()/".tool-cache/test-tmp"
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="protocol-features-", dir=cache) as tmp:
        root = Path(tmp)
        engine = root/'HLSDownloaderEngine.exe'
        shutil.copy2(args.engine, engine)
        runtime_dll = args.engine.parent / 'vcruntime140.dll'
        if runtime_dll.is_file():
            shutil.copy2(runtime_dll, root / runtime_dll.name)
        vm_acceptance = os.environ.get("HLS_VM_ACCEPTANCE") == "1"
        firewall_rule = None
        firewall_add_attempted = False
        if vm_acceptance and not args.no_firewall_rule:
            firewall_rule = {
                "name": f"HLS-v7-protocol-features-{root.name}",
                "program": str(engine.resolve()),
                "add_returncode": None,
                "delete_returncode": None,
            }
            result["firewall_rule"] = firewall_rule
        payload = bytes(range(256))*32768
        expected = hashlib.sha256(payload).hexdigest()
        media = root/"payload.bin"
        media.write_bytes(payload)
        ftp_commands, sftp_reads, http_requests = [], [], []
        broken_enabled = threading.Event()
        broken_enabled.set()
        timeout_release = threading.Event()
        logging.getLogger("pyftpdlib").setLevel(logging.WARNING)
        logging.getLogger("paramiko").setLevel(logging.CRITICAL)

        class Transfer(ThrottledDTPHandler):
            write_limit = 2*1024*1024

        class Ftp(FTPHandler):
            dtp_handler = Transfer

            def pre_process_command(self, line, cmd, arg):
                if cmd != "PASS":
                    ftp_commands.append([cmd, arg])
                return super().pre_process_command(line, cmd, arg)

        authorizer = DummyAuthorizer()
        authorizer.add_user("qa", "test-password", str(root), perm="elr")
        Ftp.authorizer = authorizer
        ftp = FTPServer(("127.0.0.1", 0), Ftp)
        threading.Thread(target=ftp.serve_forever, kwargs={"timeout": .1}, daemon=True).start()
        ftp_port = ftp.socket.getsockname()[1]

        class Ssh(paramiko.ServerInterface):
            def check_auth_password(self, username, password):
                return paramiko.AUTH_SUCCESSFUL if (username, password) == ("qa", "test-password") else paramiko.AUTH_FAILED

            def get_allowed_auths(self, username):
                return "password"

            def check_channel_request(self, kind, channel_id):
                return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

        class Handle(paramiko.SFTPHandle):
            def read(self, offset, length):
                sftp_reads.append([offset, length])
                time.sleep(.01)
                return super().read(offset, length)

        class Files(paramiko.SFTPServerInterface):
            def stat(self, path):
                if path != "/payload.bin":
                    return paramiko.SFTP_NO_SUCH_FILE
                return paramiko.SFTPAttributes.from_stat(media.stat())

            lstat = stat

            def open(self, path, flags, attr):
                if path != "/payload.bin":
                    return paramiko.SFTP_NO_SUCH_FILE
                if flags & (os.O_WRONLY | os.O_RDWR):
                    return paramiko.SFTP_PERMISSION_DENIED
                handle = Handle(flags)
                handle.readfile = media.open("rb")
                return handle

        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(.2)
        sftp_port = listener.getsockname()[1]
        key = paramiko.RSAKey.generate(2048)
        stop = threading.Event()
        transports = []

        def ssh_serve():
            while not stop.is_set():
                try:
                    peer, _ = listener.accept()
                except socket.timeout:
                    continue
                except OSError:
                    break
                transport = paramiko.Transport(peer)
                transports.append(transport)
                transport.add_server_key(key)
                transport.set_subsystem_handler("sftp", paramiko.SFTPServer, Files)
                threading.Thread(target=lambda t=transport: t.start_server(server=Ssh()), daemon=True).start()

        threading.Thread(target=ssh_serve, daemon=True).start()

        class Http(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_HEAD(self):
                self.respond(False)

            def do_GET(self):
                self.respond(True)

            def do_POST(self):
                self.respond(True)

            def respond(self, write):
                body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
                http_requests.append({"path": self.path, "method": self.command, "body": body.decode(),
                                      "cookie": self.headers.get("Cookie"), "authorization": self.headers.get("Authorization")})
                if self.path == '/timeout':
                    timeout_release.wait(90)
                    return
                if self.path == "/post" and (self.command != "POST" or body != b"id=42" or
                        self.headers.get("Cookie") != "sid=qa-secret" or self.headers.get("Authorization") != "Bearer qa-token"):
                    self.send_error(403)
                    return
                if self.path == "/expired":
                    self.send_error(403)
                    return
                if self.path == "/missing":
                    self.send_error(404)
                    return
                if self.path == "/login.mp4":
                    page = b'<!doctype html><html><body>Login required</body></html>'
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/html')
                    self.send_header('Content-Length', str(len(page)))
                    self.end_headers()
                    if write:
                        self.wfile.write(page)
                    return
                start, end = 0, len(payload)-1
                ranged = self.headers.get("Range", "")
                if ranged:
                    a, b = ranged.removeprefix("bytes=").split("-")
                    start, end = int(a), int(b) if b else end
                self.send_response(206 if ranged else 200)
                self.send_header("Content-Length", str(end-start+1))
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Accept-Ranges", "bytes")
                if ranged:
                    self.send_header("Content-Range", f"bytes {start}-{end}/{len(payload)}")
                self.end_headers()
                if write:
                    try:
                        if self.path == '/broken' and broken_enabled.is_set():
                            if start == 0:
                                self.wfile.write(payload[:min(32768, end+1)])
                            self.wfile.flush()
                            self.connection.shutdown(socket.SHUT_RDWR)
                            return
                        self.wfile.write(payload[start:end+1])
                    except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                        pass

        http = ThreadingHTTPServer(("127.0.0.1", 0), Http)
        threading.Thread(target=http.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{http.server_port}"
        port = free_port()
        env = {k: v for k, v in os.environ.items() if not k.startswith("HLS_")}
        env.update({"HLS_V7_DATA_DIR": str(root/"data"), "HLS_V7_DOWNLOAD_DIR": str(root/"downloads"),
                    "HLS_V7_CORE_TCP": "1", "HLS_V7_CORE_BIND": f"127.0.0.1:{port}",
                    "HLS_V7_PIPE": rf"\\.\pipe\HLSDownloader.protocol-{uuid.uuid4().hex}"})
        process = None
        try:
            if firewall_rule is not None:
                firewall_add_attempted = True
                added = subprocess.run(
                    ["netsh", "advfirewall", "firewall", "add", "rule",
                     f"name={firewall_rule['name']}", "dir=in", "action=allow",
                     f"program={firewall_rule['program']}", "enable=yes", "profile=any"],
                    capture_output=True, text=True, check=False,
                    creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
                )
                firewall_rule["add_returncode"] = added.returncode
                if added.returncode != 0:
                    raise RuntimeError(f"Temporary Engine firewall rule add failed: {added.returncode}")
            with args.report.with_suffix(".core.log").open("wb") as log:
                process = subprocess.Popen([str(engine)], env=env, stdout=log, stderr=log,
                                           creationflags=subprocess.CREATE_NO_WINDOW)
            with connect(port) as ipc:
                settings = send(ipc, {"type": "store_settings", "request_id": 1, "values": {
                    "legal_terms_accepted": True, "legal_terms_version": "2026-08-06-cn-1", "av_scan_enabled": False,
                    "download_progress_window_enabled": False, "download_complete_popup_enabled": False}})
                assert settings["type"] != "error", settings

                def command(kind, **values):
                    response = send(ipc, {"type": "command", "request_id": 2, "command": {"kind": kind, **values}})
                    assert response.get("type") != "error", response
                    return [item["event"] for item in response.get("events", [])]

                def create(url, **spec):
                    return next(e["snapshot"] for e in command("create_task", spec={
                        "url": url, "filename": uuid.uuid4().hex+".bin", "resource_kind": "file", "allow_duplicate": True,
                        "concurrency": 1, **spec}) if e["kind"] == "task_created")

                def action(task, name):
                    command("task_action", task_id=task["task_id"], action=name)

                def terminal(task):
                    return wait_task(ipc, task["task_id"], lambda t: t["status"] in ("completed", "failed"), 3)

                def intact(task):
                    assert task["status"] == "completed", task
                    output = Path(task["output_path"])
                    assert output.stat().st_size == len(payload) and hashlib.sha256(output.read_bytes()).hexdigest() == expected

                def resumed(url, kind, offsets):
                    task = create(url, resource_kind=kind)
                    action(task, "start")
                    wait_task(ipc, task["task_id"], lambda t: t["downloaded_bytes"] >= 262144 or t["status"] == "failed", 4)
                    action(task, "pause")
                    paused = wait_task(ipc, task["task_id"], lambda t: t["status"] == "paused" and t["active_workers"] == 0, 5)
                    assert 0 < paused["downloaded_bytes"] < len(payload), paused
                    boundary = len(offsets)
                    action(task, "resume")
                    done = terminal(task)
                    intact(done)
                    assert (any(cmd == "REST" and int(arg) > 0 for cmd, arg in offsets[boundary:]) if kind == "ftp"
                            else any(offset > 0 for offset, _ in offsets[boundary:]))
                    return {"paused_bytes": paused["downloaded_bytes"], "bytes": len(payload), "sha256": expected}

                def check(name, operation):
                    if args.only and args.only.lower() not in name.lower():
                        return
                    entry = {"name": name, "passed": False}
                    try:
                        entry["actual"] = operation()
                        entry["passed"] = True
                    except Exception as error:
                        entry["error"] = str(error)
                        entry["traceback"] = traceback.format_exc()
                    result["checks"].append(entry)
                    args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
                    print(json.dumps({"check": name, "passed": entry["passed"], "error": entry.get("error")}), flush=True)

                check("real FTP password login, pause, REST resume and SHA256", lambda: resumed(
                    f"ftp://qa:test-password@127.0.0.1:{ftp_port}/payload.bin", "ftp", ftp_commands))
                check("real SFTP password login, pinned host key, pause, offset resume and SHA256", lambda: resumed(
                    f"sftp://qa:test-password@127.0.0.1:{sftp_port}/payload.bin", "sftp", sftp_reads))

                def curl():
                    task = next(e["snapshot"] for e in command("import_curl", command=
                        f'curl -X POST -H "Authorization: Bearer qa-token" -b "sid=qa-secret" --data-raw "id=42" {base}/post',
                        options={"url": "", "resource_kind": "file", "filename": "curl.bin"}) if e["kind"] == "task_created")
                    action(task, "start")
                    done = terminal(task)
                    intact(done)
                    assert "qa-secret" not in json.dumps(done) and "qa-token" not in json.dumps(done)
                    return {"bytes": len(payload), "sha256": expected, "secrets_absent_from_snapshot": True}
                check("cURL import -> actual authenticated POST with Cookie and exact body", curl)

                def checksum():
                    good = create(base+"/payload", checksum="sha256:"+expected)
                    action(good, "start")
                    good = terminal(good)
                    intact(good)
                    assert good["checksum_verified"] is True, good
                    bad = create(base+"/payload", checksum="sha256:"+"0"*64)
                    action(bad, "start")
                    bad = terminal(bad)
                    assert bad["status"] == "failed" and bad.get("error_message"), bad
                    assert not (root/"downloads"/bad["filename"]).exists()
                    return {"correct_verified": True, "incorrect_failed": bad["error_message"]}
                check("checksum match completes; mismatch fails without publishing corrupt file", checksum)

                def roundtrip():
                    task = create(base+"/payload?export=1", title="导出往返", speed_limit_kib=512, scheduled_start_at="2999-01-01T00:00:00Z",
                                  checksum="sha256:"+expected)
                    event = next(e for e in command("export_tasks", task_ids=[task["task_id"]], format="json") if e["kind"] == "task_export")
                    document = json.loads(event["data"])
                    assert document["schema"] == "hls-downloader.tasks.v1" and len(document["tasks"]) == 1
                    source = root/"tasks.json"
                    source.write_text(event["data"], encoding="utf8")
                    action(task, "delete")
                    imported_events = command("import_paths", paths=[str(source)])
                    imported = next((e["snapshot"] for e in imported_events if e["kind"] == "task_created"), None)
                    assert imported is not None, imported_events
                    for field in ("url", "title", "filename", "speed_limit_kib", "scheduled_start_at", "expected_checksum"):
                        assert imported[field] == task[field], (field, imported[field], task[field])
                    assert imported["downloaded_bytes"] == 0 and imported["status"] == "queued", imported
                    action(imported, "start")
                    time.sleep(.5)
                    queued = snapshot(ipc, imported["task_id"], 7)
                    assert queued["status"] == "queued" and queued["downloaded_bytes"] == 0, queued
                    return {"configuration_preserved": True, "runtime_state_not_imported": True, "future_schedule_prevents_transfer": True}
                check("export JSON -> delete record -> import, preserving configuration and future schedule", roundtrip)

                def scheduled_transfer():
                    token = uuid.uuid4().hex
                    path = f"/scheduled?case={token}"
                    due = datetime.now(timezone.utc).replace(microsecond=0) + timedelta(seconds=6)
                    scheduled_at = due.isoformat(timespec="seconds").replace("+00:00", "Z")
                    task = create(base + path, scheduled_start_at=scheduled_at)
                    action(task, "start")

                    remaining = (due - datetime.now(timezone.utc)).total_seconds()
                    assert remaining > 1, {"scheduled_start_at": scheduled_at, "remaining_seconds": remaining}
                    before_due = time.monotonic() + remaining - .25
                    while time.monotonic() < before_due:
                        current = snapshot(ipc, task["task_id"], 7)
                        requests = [item for item in http_requests if item["path"] == path]
                        assert current["status"] == "queued" and current["downloaded_bytes"] == 0, current
                        assert not requests, requests
                        time.sleep(.1)

                    released = terminal(task)
                    intact(released)
                    requests = [item for item in http_requests if item["path"] == path]
                    assert any(item["method"] == "GET" for item in requests), requests
                    return {"scheduled_start_at": scheduled_at, "pre_due_status": "queued",
                            "pre_due_downloaded_bytes": 0, "pre_due_http_requests": 0,
                            "post_due_status": released["status"], "sha256": expected}

                check("scheduling and rate: near-term schedule waits then downloads with SHA256", scheduled_transfer)

                def speed_limited_transfer():
                    token = uuid.uuid4().hex
                    path = f"/rate-limited?case={token}"
                    limit_kib = 64
                    burst_tolerance = 512 * 1024
                    task = create(base + path, speed_limit_kib=limit_kib)
                    action(task, "start")
                    started = wait_task(ipc, task["task_id"],
                                        lambda current: current["downloaded_bytes"] > 0
                                        or current["status"] in ("completed", "failed"), 8)
                    assert started["status"] not in ("completed", "failed"), started
                    baseline = started["downloaded_bytes"]
                    started_at = time.monotonic()
                    # Core 在分块提交时更新 downloaded_bytes；64 KiB/s 下下一块可耗时约 28 秒。
                    observed = wait_task(ipc, task["task_id"],
                                         lambda current: current["downloaded_bytes"] > baseline
                                         or current["status"] in ("completed", "failed"), 7)
                    elapsed = time.monotonic() - started_at
                    delta = observed["downloaded_bytes"] - baseline
                    allowed = limit_kib * 1024 * elapsed + burst_tolerance
                    assert observed["status"] not in ("completed", "failed"), observed
                    assert delta > 0, {"baseline": baseline, "observed": observed}
                    assert delta <= allowed, {"limit_kib": limit_kib, "elapsed_seconds": elapsed,
                                               "downloaded_delta": delta, "allowed_bytes": allowed}
                    assert any(item["path"] == path and item["method"] == "GET" for item in http_requests), http_requests

                    action(task, "pause")
                    paused = wait_task(ipc, task["task_id"],
                                       lambda current: current["status"] == "paused"
                                       and current["active_workers"] == 0, 7)
                    assert 0 < paused["downloaded_bytes"] < len(payload), paused
                    action(task, "speed:0")
                    action(task, "resume")
                    done = terminal(task)
                    intact(done)
                    return {"limit_kib": limit_kib, "observation_seconds": elapsed,
                            "observed_downloaded_delta": delta, "allowed_delta_bytes": int(allowed),
                            "paused_bytes": paused["downloaded_bytes"], "sha256": expected}

                check("scheduling and rate: measured per-task KiB limit then unlimited completion", speed_limited_transfer)

                def refresh():
                    task = create(base+"/expired")
                    action(task, "start")
                    failed = terminal(task)
                    assert failed["status"] == "failed" and "403" in failed["error_message"], failed
                    command("refresh_task_request", task_id=task["task_id"], url=base+"/payload", auto_resume=True)
                    done = terminal(task)
                    intact(done)
                    return {"same_task_id": done["task_id"], "sha256": expected}
                check("expired signed URL -> visible failure -> refresh URL and resume same task", refresh)

                def http_failure(status, endpoint):
                    task = create(base + endpoint)
                    action(task, 'start')
                    failed = terminal(task)
                    assert failed['status'] == 'failed' and failed['error_code'] == f'HTTP_{status}', failed
                    assert failed.get('error_message') and failed.get('error_hint'), failed
                    assert not Path(failed['output_path']).is_file(), failed
                    command('refresh_task_request', task_id=task['task_id'], url=base+'/payload', auto_resume=True)
                    recovered = terminal(task)
                    intact(recovered)
                    assert recovered['task_id'] == failed['task_id'] and not recovered.get('error_code'), recovered
                    return {'error_code': failed['error_code'], 'hint': failed['error_hint'], 'recovered_sha256': expected}

                check('exceptions HTTP 403 visible classified failure and same-task recovery', lambda: http_failure(403, '/expired'))
                check('exceptions HTTP 404 visible classified failure and same-task recovery', lambda: http_failure(404, '/missing'))

                def login_media():
                    task = create(base+'/login.mp4', filename=uuid.uuid4().hex+'.mp4', mime_type='video/mp4')
                    action(task, 'start')
                    failed = terminal(task)
                    assert failed['status'] == 'failed' and 'HTML' in failed['error_message'], failed
                    assert not Path(failed['output_path']).is_file(), failed
                    action(failed, 'delete_files')
                    return {'error': failed['error_message'], 'media_file_not_published': True}

                check('exceptions HTTP 200 login HTML is rejected as media without false success', login_media)

                def invalid_directory():
                    blocker = root/'not-a-directory'
                    blocker.write_bytes(b'preserve this file')
                    response = send(ipc, {'type':'command','request_id':2,'command':{'kind':'create_task','spec':{
                        'url':base+'/payload', 'filename':uuid.uuid4().hex+'.bin', 'resource_kind':'file',
                        'download_dir':str(blocker/'child'), 'concurrency':1}}})
                    if response.get('type') == 'error':
                        error = response
                    else:
                        task = next(e['event']['snapshot'] for e in response['events'] if e['event']['kind']=='task_created')
                        action(task, 'start')
                        error = terminal(task)
                        assert error['status'] == 'failed' and error.get('error_message'), error
                    assert blocker.read_bytes() == b'preserve this file'
                    good = create(base+'/payload')
                    action(good, 'start')
                    intact(terminal(good))
                    return {'failure':error, 'blocking_file_preserved':True, 'valid_directory_recovers':True}

                check('exceptions invalid save directory fails clearly and valid directory recovers', invalid_directory)

                def broken_transfer():
                    task = create(base+'/broken')
                    action(task, 'start')
                    failed = terminal(task)
                    assert failed['status'] == 'failed' and failed.get('error_message') and failed.get('error_hint'), failed
                    assert not Path(failed['output_path']).is_file(), failed
                    broken_enabled.clear()
                    action(failed, 'retry')
                    recovered = terminal(task)
                    intact(recovered)
                    return {'failure': failed['error_message'], 'recovered_sha256': expected, 'same_task_id': task['task_id']}

                check('exceptions interrupted response fails without corrupt output then retry completes same task', broken_transfer)

                def refused_connection():
                    with socket.socket() as reserved:
                        reserved.bind(('127.0.0.1', 0))
                        task = create(f'http://127.0.0.1:{reserved.getsockname()[1]}/payload')
                        action(task, 'start')
                        failed = terminal(task)
                    assert failed['status'] == 'failed' and failed.get('error_message') and failed.get('error_hint'), failed
                    command('refresh_task_request', task_id=task['task_id'], url=base+'/payload', auto_resume=True)
                    intact(terminal(task))
                    return {'failure': failed['error_message'], 'recovered_sha256': expected}

                check('exceptions offline endpoint refusal is visible and recovered after link refresh', refused_connection)

                def timed_out_response():
                    task = create(base+'/timeout')
                    action(task, 'start')
                    deadline = time.monotonic()+100
                    while time.monotonic() < deadline:
                        failed = snapshot(ipc, task['task_id'], 3)
                        if failed['status'] in ('failed', 'completed'):
                            break
                        time.sleep(.1)
                    assert failed['status'] == 'failed' and failed.get('error_message') and failed.get('error_hint'), failed
                    assert not Path(failed['output_path']).is_file(), failed
                    command('refresh_task_request', task_id=task['task_id'], url=base+'/payload', auto_resume=True)
                    intact(terminal(task))
                    return {'failure': failed['error_message'], 'hint': failed['error_hint'], 'recovered_sha256': expected}

                check('exceptions real default HTTP response timeout and same-task recovery', timed_out_response)

                def lan_share():
                    event = next(e for e in command("share_media", path=str(media), url="", title="LAN验收", device_id="")
                                 if e["kind"] == "cast_session")
                    location = event["media_url"]
                    opener = build_opener(ProxyHandler({}))
                    with opener.open(Request(location, headers={"Range": "bytes=1024-4095"}), timeout=10) as response:
                        assert response.status == 206 and response.read() == payload[1024:4096]
                    command("control_cast", action="stop")
                    try:
                        with opener.open(location, timeout=10) as response:
                            raise AssertionError(f"Stopped share still returns {response.status}")
                    except HTTPError as error:
                        assert error.code == 404, error
                    return {"lan_url": location, "range_bytes_match": True, "stop_revokes_url": True}
                check("LAN share actual published address serves correct Range; stop revokes address", lan_share)
                if args.public_ftps or args.local_ftps:
                    def local_ftps():
                        if os.environ.get("HLS_VM_ACCEPTANCE") != "1":
                            raise RuntimeError("local FTPS fixture is restricted to the isolated acceptance guest")
                        from cryptography import x509
                        from cryptography.hazmat.primitives import hashes, serialization
                        from cryptography.hazmat.primitives.asymmetric import rsa
                        from cryptography.x509.oid import NameOID
                        import ipaddress
                        from pyftpdlib.handlers import TLS_FTPHandler

                        ftps_payload = (bytes(range(256))*313)[:80000]
                        ftps_file = root/"ftps-payload.bin"
                        ftps_file.write_bytes(ftps_payload)
                        now = datetime.now(timezone.utc).replace(tzinfo=None)
                        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
                        name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
                        certificate = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
                            .public_key(key.public_key()).serial_number(x509.random_serial_number())
                            .not_valid_before(now-timedelta(minutes=1)).not_valid_after(now+timedelta(days=1))
                            .add_extension(x509.SubjectAlternativeName([
                                x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))
                            ]), critical=False).sign(key, hashes.SHA256()))
                        cert_file, key_file = root/"ftps-cert.pem", root/"ftps-key.pem"
                        cert_der = root/"ftps-cert.cer"
                        cert_file.write_bytes(certificate.public_bytes(serialization.Encoding.PEM))
                        key_file.write_bytes(key.private_bytes(serialization.Encoding.PEM,
                            serialization.PrivateFormat.TraditionalOpenSSL, serialization.NoEncryption()))
                        cert_der.write_bytes(certificate.public_bytes(serialization.Encoding.DER))
                        thumbprint = certificate.fingerprint(hashes.SHA1()).hex().upper()
                        certificate_cleanup_needed = True
                        servers = []
                        server_threads = []
                        commands = []
                        from pyftpdlib.ioloop import IOLoop

                        def run_certutil(arguments):
                            process = subprocess.Popen(["certutil.exe", *arguments], stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True,
                                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
                            try:
                                stdout, stderr = process.communicate(timeout=30)
                                timed_out = False
                            except subprocess.TimeoutExpired:
                                process.terminate()
                                try:
                                    stdout, stderr = process.communicate(timeout=5)
                                except subprocess.TimeoutExpired:
                                    process.kill()
                                    stdout, stderr = process.communicate()
                                timed_out = True
                            return {"exit_code": process.returncode, "stdout": stdout,
                                "stderr": stderr, "timed_out": timed_out}

                        class Ftps(TLS_FTPHandler):
                            def pre_process_command(self, line, cmd, arg):
                                if cmd != "PASS":
                                    commands.append([cmd, arg])
                                return super().pre_process_command(line, cmd, arg)

                        class ImplicitFtps(Ftps):
                            def __init__(self, conn, server, *args, **kwargs):
                                super().__init__(conn, server, *args, **kwargs)
                                if self.connected:
                                    # The first loop callback sends the 220 banner after this constructor returns.
                                    self.secure_connection(self.ssl_context)

                        def make_server(handler, port):
                            handler.authorizer = authorizer
                            handler.certfile = str(cert_file)
                            handler.keyfile = str(key_file)
                            handler.tls_control_required = True
                            handler.tls_data_required = True
                            handler.ssl_context = None
                            server = FTPServer(("127.0.0.1", port), handler, ioloop=IOLoop())
                            thread = threading.Thread(target=server.serve_forever,
                                kwargs={"timeout": .1}, daemon=True)
                            thread.start()
                            servers.append(server)
                            server_threads.append(thread)
                            return server.socket.getsockname()[1]

                        try:
                            added = run_certutil(["-f", "-addstore", "Root", str(cert_der)])
                            assert not added["timed_out"] and added["exit_code"] == 0, added
                            explicit_port = make_server(Ftps, 0)
                            implicit_port = make_server(ImplicitFtps, 990)
                            checkpoints = []
                            expected = hashlib.sha256(ftps_payload).hexdigest()
                            for mode, port in (("explicit", explicit_port), ("implicit", implicit_port)):
                                task = create(f"ftps://qa:test-password@localhost:{port}/ftps-payload.bin",
                                              resource_kind="ftp", speed_limit_kib=16)
                                action(task, "start")
                                started = wait_task(ipc, task["task_id"],
                                    lambda t: t["downloaded_bytes"] > 0 or t["status"] == "failed", 10)
                                assert started["status"] != "failed", started
                                action(task, "pause")
                                paused = wait_task(ipc, task["task_id"],
                                    lambda t: t["status"] == "paused" and t["active_workers"] == 0, 11)
                                checkpoint = paused["downloaded_bytes"]
                                assert 0 < checkpoint < len(ftps_payload), paused
                                action(task, "speed:0")
                                boundary = len(commands)
                                action(task, "resume")
                                done = terminal(task)
                                assert done["status"] == "completed", done
                                assert Path(done["output_path"]).read_bytes() == ftps_payload
                                assert any(cmd == "REST" and int(arg) > 0 for cmd, arg in commands[boundary:]), commands[boundary:]
                                checkpoints.append({"mode": mode, "paused_bytes": checkpoint})
                            return {"modes": ["explicit", "implicit"], "bytes": len(ftps_payload),
                                "sha256": expected, "resume_checkpoints": checkpoints,
                                "server_certificate_subject": "localhost"}
                        finally:
                            cleanup_errors = []
                            for server in servers:
                                try:
                                    server.close_all()
                                except Exception as error:
                                    cleanup_errors.append(str(error))
                            for thread in server_threads:
                                thread.join(timeout=3)
                                if thread.is_alive():
                                    cleanup_errors.append("FTPS server IOLoop thread did not stop")
                            if certificate_cleanup_needed:
                                try:
                                    deleted = run_certutil(["-f", "-delstore", "Root", thumbprint])
                                    if deleted["timed_out"] or deleted["exit_code"] != 0:
                                        cleanup_errors.append(f"Temporary certificate removal failed: {deleted}")
                                except OSError as error:
                                    cleanup_errors.append(f"Could not start certutil for certificate cleanup: {error}")
                            if cleanup_errors:
                                raise RuntimeError("; ".join(cleanup_errors))

                    def public_ftps():
                        import certifi
                        blocks = []
                        class SessionFTP_TLS(FTP_TLS):
                            def ntransfercmd(self, cmd, rest=None):
                                conn, size = FTP.ntransfercmd(self, cmd, rest)
                                if self._prot_p:
                                    try:
                                        # 公网服务要求数据通道复用控制通道会话，ftplib 默认未传 session。
                                        conn = self.context.wrap_socket(conn, server_hostname=self.host,
                                                                        session=self.sock.session)
                                    except Exception:
                                        conn.close()
                                        raise
                                    assert conn.session_reused, "Reference FTPS data TLS session was not resumed"
                                return conn, size
                        reference = SessionFTP_TLS(timeout=12,
                            context=ssl.create_default_context(cafile=certifi.where()))
                        try:
                            reference.connect("test.rebex.net", 21)
                        except (EOFError, TimeoutError, ConnectionError, socket.gaierror) as error:
                            reference.close()
                            raise RuntimeError(f"external unavailable: {error}") from error
                        try:
                            reference.login("demo", "password")
                            reference.prot_p()
                            reference.retrbinary("RETR readme.txt", blocks.append)
                            resume_blocks = []
                            reference.retrbinary("RETR /pub/example/WinFormClient.png", resume_blocks.append)
                        finally:
                            reference.close()
                        reference_bytes = b"".join(blocks)
                        resume_bytes = b"".join(resume_blocks)
                        assert reference_bytes
                        assert len(resume_bytes) == 80000
                        task = create("ftps://demo:password@test.rebex.net:21/readme.txt", resource_kind="ftp")
                        action(task, "start")
                        done = terminal(task)
                        assert done["status"] == "completed", done
                        assert Path(done["output_path"]).read_bytes() == reference_bytes
                        task = create("ftps://demo:password@test.rebex.net:21/pub/example/WinFormClient.png",
                                      resource_kind="ftp", speed_limit_kib=16)
                        action(task, "start")
                        started = wait_task(ipc, task["task_id"],
                                            lambda t: t["downloaded_bytes"] > 0 or t["status"] == "failed", 10)
                        assert started["status"] != "failed", started
                        action(task, "pause")
                        paused = wait_task(ipc, task["task_id"],
                                           lambda t: t["status"] == "paused" and t["active_workers"] == 0, 11)
                        checkpoint = paused["downloaded_bytes"]
                        assert 0 < checkpoint < len(resume_bytes), paused
                        action(task, "speed:0")
                        action(task, "resume")
                        done = terminal(task)
                        assert done["status"] == "completed", done
                        assert Path(done["output_path"]).read_bytes() == resume_bytes
                        checkpoints = [{"mode": "explicit", "paused_bytes": checkpoint}]
                        return {"bytes": len(reference_bytes), "sha256": hashlib.sha256(reference_bytes).hexdigest(),
                                "resume_file_bytes": len(resume_bytes),
                                "resume_sha256": hashlib.sha256(resume_bytes).hexdigest(), "resume_checkpoints": checkpoints,
                                "server_documentation": "https://test.rebex.net/",
                                "reference_protocol": "explicit FTPS (public test file)", "core_protocols": ["explicit"]}
                    if args.local_ftps:
                        check("local FTPS explicit/implicit TLS, REST resume and SHA256", local_ftps)
                    if args.public_ftps:
                        check("public FTPS Rebex endpoint actual TLS and independent file SHA256", public_ftps)
                result["passed"] = all(c["passed"] for c in result["checks"])
                result["sftp_host_key_persisted"] = (root/"data/known_hosts").is_file()
        except Exception as error:
            result["error"] = str(error)
            result["traceback"] = traceback.format_exc()
            if firewall_rule is not None and firewall_add_attempted and firewall_rule["add_returncode"] is None:
                firewall_rule["add_error"] = str(error)
        finally:
            timeout_release.set()
            if process and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
                    result["passed"] = False
                    result["isolated_process_stop_error"] = "Engine did not exit after terminate; killed it"
            if firewall_rule is not None and firewall_add_attempted:
                try:
                    deleted = subprocess.run(
                        ["netsh", "advfirewall", "firewall", "delete", "rule",
                         f"name={firewall_rule['name']}", f"program={firewall_rule['program']}"],
                        capture_output=True, text=True, check=False,
                        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
                    )
                    firewall_rule["delete_returncode"] = deleted.returncode
                    if deleted.returncode != 0:
                        raise RuntimeError(f"netsh returned {deleted.returncode}")
                except Exception as error:
                    result["passed"] = False
                    firewall_rule["delete_error"] = str(error)
            http.shutdown()
            http.server_close()
            ftp.close_all()
            stop.set()
            listener.close()
            for transport in transports:
                transport.close()
            result["isolated_process_stopped"] = process is None or process.poll() is not None
            args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--only", default="", help="Run checks whose name contains this text")
    parser.add_argument("--public-ftps", action="store_true")
    parser.add_argument("--local-ftps", action="store_true")
    parser.add_argument("--no-firewall-rule", action="store_true", help="Leave firewall settings unchanged for loopback-only acceptance")
    raise SystemExit(run(parser.parse_args()))
