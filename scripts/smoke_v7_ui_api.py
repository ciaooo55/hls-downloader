#!/usr/bin/env python3
"""Exercise the opt-in v7 Compose UI test API without Windows UI Automation."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import time
import urllib.error
import urllib.request
from pathlib import Path

from PIL import Image


def request(base: str, token: str, path: str, method: str = "GET", payload: dict | None = None) -> bytes:
    data = None if payload is None else json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(
        f"{base}{path}",
        data=data,
        method=method,
        headers={"X-HLS-Test-Token": token, "Content-Type": "application/json; charset=utf-8"},
    )
    with urllib.request.urlopen(req, timeout=10) as response:
        return response.read()


def wait_ready(base: str, token: str, timeout: float) -> dict:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            return json.loads(request(base, token, "/health"))
        except Exception as error:  # The app may still be compiling or opening its window.
            last_error = error
            time.sleep(0.2)
    raise RuntimeError(f"UI test API did not become ready: {last_error}")


def inspect_png(data: bytes, path: Path) -> dict:
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise RuntimeError("screenshot response is not a PNG")
    image = Image.open(io.BytesIO(data)).convert("RGB")
    colors = image.getcolors(maxcolors=image.width * image.height)
    extrema = image.getextrema()
    if colors is None or len(colors) < 16 or all(low == high for low, high in extrema):
        raise RuntimeError("screenshot is blank or visually degenerate")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return {
        "path": str(path.resolve()),
        "width": image.width,
        "height": image.height,
        "color_count": len(colors),
        "sha256": hashlib.sha256(data).hexdigest().upper(),
    }


def capture_png(base: str, token: str, path: Path, attempts: int = 5, endpoint: str = "/screenshot") -> dict:
    last_error: Exception | None = None
    for attempt in range(1, attempts + 1):
        try:
            result = inspect_png(request(base, token, endpoint), path)
            result["capture_attempts"] = attempt
            return result
        except RuntimeError as error:
            last_error = error
            if attempt < attempts:
                time.sleep(0.2)
    if endpoint == "/screenshot":
        try:
            result = inspect_png(request(base, token, "/screenshot?mode=paint"), path)
            result["capture_attempts"] = attempts + 1
            result["capture_mode"] = "window_paint_fallback"
            return result
        except RuntimeError as error:
            last_error = error
    raise RuntimeError(f"screenshot remained invalid after {attempts} attempts: {last_error}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--token", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--action", action="append", default=[], help="JSON action object; may be repeated")
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--expected-width", type=int, default=1024)
    parser.add_argument("--expected-height", type=int, default=600)
    parser.add_argument("--screen-output", type=Path)
    args = parser.parse_args()
    if len(args.token) < 16:
        raise SystemExit("--token must contain at least 16 characters")

    base = f"http://127.0.0.1:{args.port}"
    health = wait_ready(base, args.token, args.timeout)
    window = json.loads(request(base, args.token, "/window"))
    if window["width"] != args.expected_width or window["height"] != args.expected_height:
        raise RuntimeError(f"unexpected window size: {window['width']}x{window['height']}")
    if window.get("iconCount", 0) < 1 or window.get("iconWidth", 0) < 16 or window.get("iconHeight", 0) < 16:
        raise RuntimeError(f"product window icon is not installed: {window}")

    unauthorized_status = None
    try:
        request(base, "invalid-test-token", "/window")
    except urllib.error.HTTPError as error:
        unauthorized_status = error.code
    if unauthorized_status != 401:
        raise RuntimeError(f"unauthorized request returned {unauthorized_status}, expected 401")

    screenshots = [capture_png(base, args.token, args.output_dir / "00-baseline.png")]
    actions = [json.loads(value) for value in args.action]
    for index, action in enumerate(actions, start=1):
        json.loads(request(base, args.token, "/action", method="POST", payload=action))
        time.sleep(0.15)
        screenshots.append(capture_png(base, args.token, args.output_dir / f"{index:02d}-after.png"))

    screen = capture_png(base, args.token, args.screen_output, endpoint="/screen") if args.screen_output else None

    report = {
        "schema": 1,
        "passed": True,
        "health": health,
        "window": window,
        "unauthorized_status": unauthorized_status,
        "actions": actions,
        "screenshots": screenshots,
        "screen": screen,
    }
    report_path = args.output_dir / "report.json"
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
