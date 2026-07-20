from __future__ import annotations

import argparse
import json
import struct
import subprocess


def _frame(message: dict) -> bytes:
    payload = json.dumps(message).encode("utf-8")
    return struct.pack("<I", len(payload)) + payload


def exchange(executable: str, messages: list[dict]) -> list[dict]:
    process = subprocess.Popen(
        [executable],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout, stderr = process.communicate(b"".join(_frame(message) for message in messages), timeout=20)
    if process.returncode != 0:
        raise RuntimeError(stderr.decode("utf-8", errors="replace"))
    responses = []
    offset = 0
    while offset < len(stdout):
        if len(stdout) - offset < 4:
            raise RuntimeError("Native host returned a truncated frame header")
        length = struct.unpack("<I", stdout[offset : offset + 4])[0]
        offset += 4
        response = stdout[offset : offset + length]
        if len(response) != length:
            raise RuntimeError("Native host returned a truncated response")
        responses.append(json.loads(response.decode("utf-8")))
        offset += length
    if len(responses) != len(messages):
        raise RuntimeError(f"Native host returned {len(responses)} responses for {len(messages)} requests")
    return responses


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True)
    args = parser.parse_args()
    responses = exchange(args.exe, [
        {"op": "ping", "version": "package-smoke"},
        {"op": "ping", "version": "package-smoke"},
    ])
    if any(response.get("ok") is not True or not response.get("version") for response in responses):
        raise RuntimeError(f"Native host persistent ping failed: {responses}")
    print(f"Native host reused one process for {len(responses)} messages; desktop v{responses[-1]['version']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
