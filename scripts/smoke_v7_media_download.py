"""Verify real installed Core HLS/DASH downloads by decoding every output video frame."""
from __future__ import annotations

import argparse
import functools
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
import uuid
import xml.etree.ElementTree as ET
from urllib.parse import urlsplit

from smoke_v7_portable_app import Pipe


class Handler(SimpleHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if urlsplit(self.path).path == '/live/index.m3u8':
            body = self.server.live_playlist
            if self.server.live_finished.is_set():
                body += '\n#EXT-X-ENDLIST\n'
            encoded = body.encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/vnd.apple.mpegurl')
            self.send_header('Content-Length', str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
            return
        if urlsplit(self.path).path in getattr(self.server, 'blocked_files', set()):
            self.send_error(403, 'Test audio unavailable')
            return
        super().do_GET()


def run(args):
    runtime = args.runtime.resolve()
    ffmpeg = runtime / "app/resources/ffmpeg.exe"
    ffprobe = runtime / "app/resources/ffprobe.exe"
    args.report.parent.mkdir(parents=True, exist_ok=True)
    result = {"passed": False, "cases": [], "engine_sha256": hashlib.sha256(
        (runtime / 'app/resources/HLSDownloaderEngine.exe').read_bytes()).hexdigest()}
    created = set()
    server = None

    def request(message):
        # 解码期间可能超过 Core 的空闲连接期限，每次请求独立建立连接。
        pipe = Pipe()
        try:
            return pipe.request(message)
        finally:
            pipe.close()

    def generate(*params, cwd=None):
        subprocess.run([str(ffmpeg), "-hide_banner", "-loglevel", "error", "-y", *map(str, params)],
                       cwd=cwd, check=True, timeout=60, capture_output=True)

    def hashes(path):
        output = subprocess.check_output([str(ffmpeg), "-v", "error", "-i", str(path),
                                          "-map", "0:v:0", "-f", "framemd5", "-"], timeout=60).decode()
        return [line.rsplit(",", 1)[-1].strip() for line in output.splitlines() if line and not line.startswith("#")]

    with tempfile.TemporaryDirectory(prefix="core-media-", dir=runtime.parents[1] / "test-tmp") as temporary:
        stage = Path(temporary)
        try:
            source = stage / "source.mp4"
            generate("-f", "lavfi", "-i", "testsrc=size=320x180:rate=15", "-f", "lavfi", "-i",
                     "sine=frequency=440:sample_rate=48000", "-t", "4", "-c:v", "libx264", "-pix_fmt",
                     "yuv420p", "-g", "15", "-c:a", "aac", source)
            expected = hashes(source)
            for kind in ("hls", "encrypted", "dash"):
                folder = stage / kind
                folder.mkdir()
                if kind == "dash":
                    generate("-i", source, "-c", "copy", "-f", "dash", "-seg_duration", "1",
                             "-use_template", "1", "-use_timeline", "1", folder / "index.mpd", cwd=folder)
                else:
                    extra = ["-hls_segment_type", "fmp4"]
                    if kind == "encrypted":
                        key = folder / "key.bin"
                        key.write_bytes(bytes(range(16)))
                        info = folder / "key-info.txt"
                        info.write_text("key.bin\n" + str(key) + "\n", encoding="utf8")
                        extra = ["-hls_key_info_file", info]
                    generate("-i", source, "-c", "copy", "-hls_time", "1", "-hls_playlist_type", "vod",
                             *extra, folder / "index.m3u8", cwd=folder)
            server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(stage)))
            if args.live:
                shutil.copytree(stage / 'hls', stage / 'live')
                server.live_playlist = (stage / 'live/index.m3u8').read_text().replace('#EXT-X-PLAYLIST-TYPE:VOD', '').replace('#EXT-X-ENDLIST', '')
                server.live_finished = threading.Event()
            threading.Thread(target=server.serve_forever, daemon=True).start()
            cases = [("HLS fMP4 with audio", "hls", "hls/index.m3u8"),
                                         ("HLS AES-128 with audio", "hls", "encrypted/index.m3u8"),
                                         ("DASH separate video and audio", "dash", "dash/index.mpd")]
            if args.live:
                cases = [("HLS live finite window, pause/resume and full decode", "live", "live/index.m3u8")]
            for case, kind, manifest in cases:
                if args.audio_failure and kind != 'dash':
                    continue
                if args.audio_failure:
                    xml = ET.parse(stage / manifest)
                    audio = next(r for r in xml.iter() if r.tag.endswith('Representation')
                                 and r.attrib.get('mimeType', '').startswith('audio/'))
                    server.blocked_files = {'/dash/' + p.name for p in (stage / 'dash').iterdir()
                                            if 'stream' + audio.attrib['id'] + '.' in p.name
                                            or 'stream' + audio.attrib['id'] + '-' in p.name}
                    assert server.blocked_files, 'Audio failure fixture has no blocked segments'
                    if args.audio_failure == 'segment':
                        server.blocked_files = {min(p for p in server.blocked_files if 'chunk-' in p)}
                    result['blocked_files'] = sorted(server.blocked_files)
                url = f"http://127.0.0.1:{server.server_port}/{manifest}"
                response = request({"type": "command", "request_id": 1, "command": {
                    "kind": "create_task", "spec": {"url": url, "resource_kind": kind,
                    "filename": "media-validation-" + uuid.uuid4().hex + ".mp4", "allow_duplicate": True}}})
                events = response.get("events", [])
                task_id = next(e["event"]["snapshot"]["task_id"] for e in events if e["event"].get("kind") == "task_created")
                created.add(task_id)
                request({"type": "command", "request_id": 2, "command": {
                    "kind": "task_action", "task_id": task_id, "action": "start"}})
                if args.live:
                    from smoke_v7_portable_app import wait
                    def current():
                        return next(t for t in request({'type': 'snapshot', 'request_id': 3})['tasks'] if t['task_id'] == task_id)
                    active = wait(lambda: (t if (t := current())['status'] == 'recording' and t['downloaded_bytes'] > 0 else None),
                                  'Live recording did not report transferred bytes', 30)
                    request({'type': 'command', 'request_id': 2, 'command': {'kind': 'task_action', 'task_id': task_id, 'action': 'pause'}})
                    paused = wait(lambda: (t if (t := current())['status'] == 'paused' and t['active_workers'] == 0 else None),
                                  'Live recording did not pause', 15)
                    request({'type': 'command', 'request_id': 2, 'command': {'kind': 'task_action', 'task_id': task_id, 'action': 'resume'}})
                    wait(lambda: current()['status'] == 'recording', 'Live recording did not resume', 15)
                    server.live_finished.set()
                    result['live_checkpoint'] = {'recording_bytes': active['downloaded_bytes'], 'paused_bytes': paused['downloaded_bytes']}
                deadline = time.monotonic() + 90
                while time.monotonic() < deadline:
                    task = next(t for t in request({"type": "snapshot", "request_id": 3})["tasks"] if t["task_id"] == task_id)
                    if task["status"] in {"completed", "failed"}:
                        break
                    time.sleep(.1)
                if args.audio_failure:
                    result['unavailable_audio_task'] = task
                    assert task['status'] == 'failed', 'Missing audio silently published as a successful download'
                    assert not Path(task['output_path']).is_file(), 'Incomplete download was published'
                    server.blocked_files.clear()
                    request({'type': 'command', 'request_id': 2, 'command': {
                        'kind': 'task_action', 'task_id': task_id, 'action': 'retry'}})
                    deadline = time.monotonic() + 90
                    while time.monotonic() < deadline:
                        task = next(t for t in request({'type': 'snapshot', 'request_id': 3})['tasks'] if t['task_id'] == task_id)
                        if task['status'] in {'completed', 'failed'}:
                            break
                        time.sleep(.1)
                if task["status"] != "completed":
                    result["failed_task"] = task
                    raise AssertionError(f"{case}: {task['status']}")
                output = Path(task["output_path"])
                metadata = json.loads(subprocess.check_output([str(ffprobe), "-v", "error", "-show_streams",
                                                               "-show_format", "-of", "json", str(output)], timeout=30))
                actual = hashes(output)
                types = {s["codec_type"] for s in metadata["streams"]}
                if actual != expected or not {"video", "audio"}.issubset(types):
                    raise AssertionError(f"{case}: frame integrity or audio stream mismatch")
                result["cases"].append({"name": case, "passed": True, "video_frames": len(actual),
                                        "audio": True, "duration": metadata["format"].get("duration"),
                                        "bytes": output.stat().st_size})
                print(case + " passed", flush=True)
                request({"type": "command", "request_id": 4, "command": {
                    "kind": "task_action", "task_id": task_id, "action": "delete_files"}})
                created.discard(task_id)
            result["passed"] = True
        except Exception as error:
            result["error"] = str(error)
            print(str(error), flush=True)
        finally:
            for task_id in created:
                request({"type": "command", "request_id": 5, "command": {
                    "kind": "task_action", "task_id": task_id, "action": "delete_files"}})
            if server:
                server.shutdown()
                server.server_close()
            args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument('--audio-failure', nargs='?', const='all', choices=['all', 'segment'])
    parser.add_argument('--live', action='store_true')
    raise SystemExit(run(parser.parse_args()))
