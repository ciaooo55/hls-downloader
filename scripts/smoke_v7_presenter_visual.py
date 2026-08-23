from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import time

user32 = ctypes.windll.user32
try:
    # GetWindowRect and ImageGrab must speak the same physical-pixel coordinate
    # system at 125%/150% DPI, otherwise the screenshot can sample the window
    # underneath the popup and falsely pass or fail the blank-frame gate.
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except (AttributeError, OSError):
    user32.SetProcessDPIAware()

# Import Pillow only after the process has selected physical-pixel DPI
# awareness; ImageGrab can otherwise cache virtualized screen coordinates.
from PIL import Image, ImageGrab, ImageStat  # noqa: E402


user32.EnumWindows.argtypes = [ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM), wintypes.LPARAM]
user32.EnumWindows.restype = wintypes.BOOL
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]


class BitmapInfoHeader(ctypes.Structure):
    _fields_ = [
        ("biSize", wintypes.DWORD),
        ("biWidth", wintypes.LONG),
        ("biHeight", wintypes.LONG),
        ("biPlanes", wintypes.WORD),
        ("biBitCount", wintypes.WORD),
        ("biCompression", wintypes.DWORD),
        ("biSizeImage", wintypes.DWORD),
        ("biXPelsPerMeter", wintypes.LONG),
        ("biYPelsPerMeter", wintypes.LONG),
        ("biClrUsed", wintypes.DWORD),
        ("biClrImportant", wintypes.DWORD),
    ]


class BitmapInfo(ctypes.Structure):
    _fields_ = [("bmiHeader", BitmapInfoHeader), ("bmiColors", wintypes.DWORD * 3)]


gdi32 = ctypes.windll.gdi32
user32.GetWindowDC.argtypes = [wintypes.HWND]
user32.GetWindowDC.restype = wintypes.HDC
user32.PrintWindow.argtypes = [wintypes.HWND, wintypes.HDC, wintypes.UINT]
user32.ReleaseDC.argtypes = [wintypes.HWND, wintypes.HDC]
gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.CreateCompatibleDC.restype = wintypes.HDC
gdi32.CreateCompatibleBitmap.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
gdi32.CreateCompatibleBitmap.restype = wintypes.HANDLE
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HANDLE]
gdi32.SelectObject.restype = wintypes.HANDLE
gdi32.GetDIBits.argtypes = [
    wintypes.HDC, wintypes.HANDLE, wintypes.UINT, wintypes.UINT,
    ctypes.c_void_p, ctypes.POINTER(BitmapInfo), wintypes.UINT,
]
gdi32.DeleteObject.argtypes = [wintypes.HANDLE]
gdi32.DeleteDC.argtypes = [wintypes.HDC]


def print_window(hwnd: int, width: int, height: int) -> Image.Image | None:
    window_dc = user32.GetWindowDC(hwnd)
    memory_dc = gdi32.CreateCompatibleDC(window_dc)
    bitmap = gdi32.CreateCompatibleBitmap(window_dc, width, height)
    previous = gdi32.SelectObject(memory_dc, bitmap)
    try:
        if not user32.PrintWindow(hwnd, memory_dc, 2):
            return None
        header = BitmapInfoHeader(
            biSize=ctypes.sizeof(BitmapInfoHeader),
            biWidth=width,
            biHeight=-height,
            biPlanes=1,
            biBitCount=32,
            biCompression=0,
            biSizeImage=width * height * 4,
        )
        info = BitmapInfo(bmiHeader=header)
        buffer = ctypes.create_string_buffer(width * height * 4)
        rows = gdi32.GetDIBits(memory_dc, bitmap, 0, height, buffer, ctypes.byref(info), 0)
        if rows != height:
            return None
        return Image.frombuffer("RGB", (width, height), buffer, "raw", "BGRX", 0, 1).copy()
    finally:
        gdi32.SelectObject(memory_dc, previous)
        gdi32.DeleteObject(bitmap)
        gdi32.DeleteDC(memory_dc)
        user32.ReleaseDC(hwnd, window_dc)


def find_window(pid: int, title: str) -> int | None:
    found: list[int] = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def visit(hwnd: int, _lparam: int) -> bool:
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value != pid or not user32.IsWindowVisible(hwnd):
            return True
        length = user32.GetWindowTextLengthW(hwnd)
        buffer = ctypes.create_unicode_buffer(length + 1)
        user32.GetWindowTextW(hwnd, buffer, length + 1)
        if buffer.value == title:
            found.append(hwnd)
            return False
        return True

    user32.EnumWindows(visit, 0)
    return found[0] if found else None


def wait_window(pid: int, title: str, timeout: float = 5.0) -> int:
    deadline = time.perf_counter() + timeout
    while time.perf_counter() < deadline:
        hwnd = find_window(pid, title)
        if hwnd:
            return hwnd
        time.sleep(0.01)
    raise TimeoutError(f"visual fixture window {title!r} did not appear")


def capture(hwnd: int, destination: Path) -> dict[str, object]:
    rect = wintypes.RECT()
    if not user32.GetWindowRect(hwnd, ctypes.byref(rect)):
        raise ctypes.WinError()
    width = rect.right - rect.left
    height = rect.bottom - rect.top
    if width < 300 or height < 140:
        raise RuntimeError(f"popup geometry is invalid: {width}x{height}")
    # PrintWindow reads the popup's own composited surface and cannot mistake
    # an overlapping app for the popup.  ImageGrab remains a compatibility
    # fallback for older Windows renderers that reject PW_RENDERFULLCONTENT.
    image = print_window(hwnd, width, height)
    if image is None:
        image = ImageGrab.grab(
            bbox=(rect.left, rect.top, rect.right, rect.bottom),
            include_layered_windows=True,
            all_screens=True,
        ).convert("RGB")
    destination.parent.mkdir(parents=True, exist_ok=True)
    image.save(destination)

    stat = ImageStat.Stat(image)
    luminance_stddev = sum(stat.stddev) / len(stat.stddev)
    pixels = list(image.get_flattened_data())
    corner_samples = [
        image.getpixel((2, 2)),
        image.getpixel((max(0, width - 3), 2)),
        image.getpixel((2, max(0, height - 3))),
        image.getpixel((max(0, width - 3), max(0, height - 3))),
    ]
    background = tuple(sorted(sample[channel] for sample in corner_samples)[len(corner_samples) // 2] for channel in range(3))
    foreground = sum(
        1 for pixel in pixels
        if max(abs(pixel[channel] - background[channel]) for channel in range(3)) >= 18
    )
    near_white = sum(1 for red, green, blue in pixels if red >= 248 and green >= 248 and blue >= 248)
    accent = sum(1 for red, green, blue in pixels if blue >= red + 28 and blue >= green + 8 and blue >= 120)
    unique_colors = len(set(pixels))
    foreground_ratio = foreground / max(1, len(pixels))
    white_ratio = near_white / max(1, len(pixels))
    passed = (
        unique_colors >= 32
        and luminance_stddev >= 5.0
        and foreground_ratio >= 0.015
        and white_ratio < 0.985
        and accent >= 8
    )
    return {
        "path": str(destination.resolve()),
        "width": width,
        "height": height,
        "unique_colors": unique_colors,
        "luminance_stddev": round(luminance_stddev, 3),
        "foreground_ratio": round(foreground_ratio, 4),
        "white_ratio": round(white_ratio, 4),
        "accent_pixels": accent,
        "passed": passed,
    }


def render_fixture(presenter: Path, output: Path, fixture: str, dark: bool) -> dict[str, object]:
    title = {"confirm": "确认下载", "progress": "下载进度", "complete": "下载完成"}[fixture]
    output.mkdir(parents=True, exist_ok=True)
    ready = output / f".{fixture}-{'dark' if dark else 'light'}.ready"
    ready.unlink(missing_ok=True)
    environment = os.environ.copy()
    environment["HLS_V7_PRESENTER_READY_FILE"] = str(ready)
    command = [str(presenter), "--visual-fixture", fixture]
    if dark:
        command.append("--dark")
    started = time.perf_counter()
    process = subprocess.Popen(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        hwnd = wait_window(process.pid, title)
        deadline = time.perf_counter() + 5
        while not ready.exists() and time.perf_counter() < deadline:
            if process.poll() is not None:
                stderr = process.stderr.read().decode("utf-8", errors="replace") if process.stderr else ""
                raise RuntimeError(f"visual fixture exited early ({process.returncode}): {stderr}")
            time.sleep(0.01)
        if not ready.exists():
            raise TimeoutError("visual fixture did not write its ready marker")
        # Allow one compositor frame after Slint reports the component visible.
        time.sleep(0.08)
        theme = "dark" if dark else "light"
        report = capture(hwnd, output / f"presenter-{fixture}-{theme}.png")
        report.update({
            "fixture": fixture,
            "theme": theme,
            "visible_ms": round((time.perf_counter() - started) * 1000, 2),
        })
        return report
    finally:
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=2)
        ready.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Render and reject blank v7 presenter popups")
    parser.add_argument("--presenter", required=True, type=Path)
    parser.add_argument("--output", type=Path, default=Path("artifacts/v7-productization/presenter-visual"))
    args = parser.parse_args()
    presenter = args.presenter.resolve()
    if not presenter.is_file():
        raise SystemExit(f"presenter not found: {presenter}")
    reports = [
        render_fixture(presenter, args.output, fixture, dark)
        for dark in (False, True)
        for fixture in ("confirm", "progress", "complete")
    ]
    result = {"fixtures": reports, "passed": all(bool(item["passed"]) for item in reports)}
    args.output.mkdir(parents=True, exist_ok=True)
    report_path = args.output / "report.json"
    report_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
