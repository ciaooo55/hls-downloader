from __future__ import annotations

import argparse
import json
import struct
import subprocess


def exchange(executable: str, message: dict) -> dict:
    payload = json.dumps(message).encode("utf-8")
    process = subprocess.Popen(
        [executable],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout, stderr = process.communicate(struct.pack("<I", len(payload)) + payload, timeout=15)
    if process.returncode != 0:
        raise RuntimeError(stderr.decode("utf-8", errors="replace"))
    if len(stdout) < 4:
        raise RuntimeError("Native host did not return a framed response")
    length = struct.unpack("<I", stdout[:4])[0]
    response = stdout[4 : 4 + length]
    if len(response) != length:
        raise RuntimeError("Native host returned a truncated response")
    return json.loads(response.decode("utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True)
    args = parser.parse_args()
    response = exchange(args.exe, {"op": "ping", "version": "package-smoke"})
    if response.get("ok") is not True or not response.get("version"):
        raise RuntimeError(f"Native host ping failed: {response}")
    print(f"Native host connected to desktop v{response['version']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
