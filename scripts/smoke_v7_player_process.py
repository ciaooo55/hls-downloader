from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from pathlib import Path


def exchange(process: subprocess.Popen[str], payload: dict) -> dict:
    assert process.stdin is not None and process.stdout is not None
    process.stdin.write(json.dumps(payload) + "\n")
    process.stdin.flush()
    line = process.stdout.readline()
    if not line:
        raise RuntimeError("player process closed stdout")
    return json.loads(line)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", required=True)
    parser.add_argument("--libmpv", default="")
    parser.add_argument("--media", default="")
    args = parser.parse_args()
    env = dict(os.environ)
    if args.libmpv:
        env.pop("HLS_V7_PLAYER_NULL", None)
        env["HLS_V6_LIBMPV"] = str(Path(args.libmpv).resolve())
    else:
        env["HLS_V7_PLAYER_NULL"] = "1"
    process = subprocess.Popen(
        [str(Path(args.engine).resolve()), "--player-process"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    try:
        metadata = exchange(process, {"op": "metadata"})
        if not metadata.get("ok") or metadata["value"]["position_available"]:
            raise RuntimeError(f"invalid player metadata response: {metadata}")
        command = exchange(process, {"op": "mpv", "command": "set pause yes"})
        if not command.get("ok"):
            raise RuntimeError(f"invalid player command response: {command}")
        real_metadata = None
        if args.media:
            media = Path(args.media).resolve()
            if not media.is_file():
                raise RuntimeError(f"player media fixture does not exist: {media}")
            mpv_path = media.as_posix().replace('"', '\\"')
            loaded = exchange(process, {"op": "mpv", "command": f'loadfile "{mpv_path}" replace'})
            if not loaded.get("ok"):
                raise RuntimeError(f"libmpv loadfile failed: {loaded}")
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                real_metadata = exchange(process, {"op": "metadata"})
                value = real_metadata.get("value") or {}
                if value.get("position_available") and value.get("audio_tracks", 0) >= 1 and value.get("subtitle_tracks", 0) >= 1:
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError(f"libmpv did not expose real duration/audio/subtitle metadata: {real_metadata}")
            for player_command in ("set aid 1", "set sid 1", "seek 50 absolute-percent", "set pause no", "set pause yes"):
                response = exchange(process, {"op": "mpv", "command": player_command})
                if not response.get("ok"):
                    raise RuntimeError(f"libmpv command failed ({player_command}): {response}")
            stopped = exchange(process, {"op": "stop"})
            if not stopped.get("ok"):
                raise RuntimeError(f"libmpv stop failed: {stopped}")
        process.kill()
        process.wait(timeout=3)
        suffix = ',"libmpv_load":true' if args.libmpv else ''
        real = ',"real_media":true,"audio_track":true,"subtitle_track":true,"seek":true' if args.media else ''
        print('{"player_process":"passed","metadata":true,"command":true,"forced_exit":true%s%s}' % (suffix, real))
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=3)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
