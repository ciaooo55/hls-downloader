"""Deterministic authenticated HLS fixture for candidate Engine evidence."""

import argparse
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit


TOKEN = "Bearer hls-v7-candidate"


class FixtureState:
    def __init__(self, mode: str, run_root: Path):
        self.mode = mode
        self.run_root = run_root
        self.log_path = run_root / "requests.jsonl"
        self.port_path = run_root / "http-port.txt"
        self.stop_path = run_root / "server.stop"
        self.release_path = run_root / "first-segment.release"
        self.first_seen_path = run_root / "first-segment.seen"
        self.log_lock = threading.Lock()
        self.playlist_count = 0

    def log(self, path: str, authorized: bool, status: int) -> None:
        row = {"path": path, "authorized": authorized, "status": status}
        with self.log_lock:
            with self.log_path.open("a", encoding="utf-8", newline="\n") as stream:
                stream.write(json.dumps(row, separators=(",", ":")) + "\n")

    def wait_for_release(self) -> None:
        self.first_seen_path.write_text("seen\n", encoding="ascii", newline="\n")
        deadline = time.monotonic() + 30.0
        while not self.release_path.exists() and time.monotonic() < deadline:
            time.sleep(0.01)


def make_handler(state: FixtureState):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, _format, *_args):
            return

        def do_GET(self):  # noqa: N802 - BaseHTTPRequestHandler API
            path = urlsplit(self.path).path
            authorized = self.headers.get("Authorization", "") == TOKEN
            if not authorized:
                status, body = 401, b"unauthorized"
            elif state.mode == "vod":
                status, body = self.vod_response(path)
            else:
                status, body = self.live_response(path)
            state.log(path, authorized, status)
            self.send_response(status)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)

        def vod_response(self, path):
            if path == "/vod.m3u8":
                return 200, (
                    b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n"
                    b"#EXTINF:1,\nvod/a.ts\n#EXTINF:1,\nvod/b.ts\n"
                    b"#EXT-X-ENDLIST\n"
                )
            if path == "/vod/a.ts":
                if not state.first_seen_path.exists():
                    state.wait_for_release()
                return 200, b"CANDIDATE-VOD-0"
            if path == "/vod/b.ts":
                return 200, b"CANDIDATE-VOD-1"
            return 404, b"not found"

        def live_response(self, path):
            if path == "/live.m3u8":
                with state.log_lock:
                    current = state.playlist_count
                    state.playlist_count += 1
                if current == 0:
                    return 200, (
                        b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n"
                        b"#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1,\nlive/0.ts\n"
                    )
                return 200, (
                    b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n"
                    b"#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:1,\n"
                    b"live/1.ts\n#EXT-X-ENDLIST\n"
                )
            if path == "/live/0.ts":
                if not state.first_seen_path.exists():
                    state.wait_for_release()
                return 200, b"CANDIDATE-LIVE-0"
            if path == "/live/1.ts":
                return 200, b"CANDIDATE-LIVE-1"
            return 404, b"not found"

    return Handler


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("vod", "live"), required=True)
    parser.add_argument("--run-root", required=True)
    args = parser.parse_args()
    run_root = Path(args.run_root)
    run_root.mkdir(parents=True, exist_ok=True)
    state = FixtureState(args.mode, run_root)
    server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(state))
    state.port_path.write_text(str(server.server_address[1]) + "\n", encoding="ascii", newline="\n")
    server.timeout = 0.2
    try:
        while not state.stop_path.exists():
            server.handle_request()
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
