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
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowW.restype = wintypes.HWND
user32.GetDpiForWindow.argtypes = [wintypes.HWND]
user32.GetDpiForWindow.restype = wintypes.UINT
user32.MonitorFromWindow.argtypes = [wintypes.HWND, wintypes.DWORD]
user32.MonitorFromWindow.restype = wintypes.HANDLE


class MonitorInfo(ctypes.Structure):
    _fields_ = [
        ("cbSize", wintypes.DWORD),
        ("rcMonitor", wintypes.RECT),
        ("rcWork", wintypes.RECT),
        ("dwFlags", wintypes.DWORD),
    ]


user32.GetMonitorInfoW.argtypes = [wintypes.HANDLE, ctypes.POINTER(MonitorInfo)]
user32.GetMonitorInfoW.restype = wintypes.BOOL
user32.SetForegroundWindow.argtypes = [wintypes.HWND]
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.SetWindowPos.argtypes = [
    wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
    ctypes.c_int, ctypes.c_int, wintypes.UINT,
]


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
    # fallback for renderers that reject PW_RENDERFULLCONTENT or report success
    # while returning an all-black GPU surface.
    image = print_window(hwnd, width, height)
    sampled_colors = image.getcolors(maxcolors=64) if image is not None else None
    if image is None or (sampled_colors is not None and len(sampled_colors) <= 1):
        # GPU-backed Slint windows can be absent from PrintWindow and can also
        # sit behind the test runner when the desktop app steals focus. Raise
        # this exact HWND immediately before the physical-pixel fallback.
        user32.SetWindowPos(hwnd, wintypes.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0040)
        user32.SetForegroundWindow(hwnd)
        time.sleep(0.08)
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


def simulate_confirm_height(hwnd: int, logical_height: int) -> dict[str, object]:
    dpi = user32.GetDpiForWindow(hwnd)
    if not dpi:
        raise RuntimeError("无法读取窗口 DPI，不能验证逻辑高度")
    scale = dpi / 96.0
    info = MonitorInfo(cbSize=ctypes.sizeof(MonitorInfo))
    monitor = user32.MonitorFromWindow(hwnd, 2)  # MONITOR_DEFAULTTONEAREST
    if not monitor or not user32.GetMonitorInfoW(monitor, ctypes.byref(info)):
        raise ctypes.WinError()
    work = info.rcWork
    width = min(round(620 * scale), work.right - work.left)
    height = round(logical_height * scale)
    if width < round(360 * scale) or height > work.bottom - work.top:
        raise RuntimeError("当前工作区装不下请求的模拟尺寸；不能当作通过")
    x = work.left + (work.right - work.left - width) // 2
    y = work.top + (work.bottom - work.top - height) // 2
    if not user32.SetWindowPos(hwnd, None, x, y, width, height, 0x0004 | 0x0010):
        raise ctypes.WinError()  # SWP_NOZORDER | SWP_NOACTIVATE
    return {
        "mode": "SetWindowPos 逻辑高度模拟，非真实 DPI 切换或 rcWork 缩小",
        "logical_height": logical_height,
        "actual_window_dpi": dpi,
        "scale": scale,
        "requested_physical_size": [width, height],
        "work_area_physical": [work.left, work.top, work.right, work.bottom],
    }


def confirm_footer_geometry(image: Image.Image, scale: float, dark: bool) -> dict[str, object]:
    # 只在底栏内找填充色的独立连通块；正文的分类选中态不能冒充确认按钮。
    colors = [(35, 39, 43), (94, 162, 243)] if dark else [(245, 247, 250), (37, 99, 235)]
    width, height = image.size
    top = max(0, height - round(60 * scale))
    pixels = image.load()
    buttons = []
    for primary, color in enumerate(colors):
        pending = {
            (x, y) for y in range(top, height) for x in range(width)
            if max(abs(pixels[x, y][channel] - color[channel]) for channel in range(3)) <= 2
        }
        while pending:
            seed = pending.pop()
            stack = [seed]
            left = right = seed[0]
            upper = lower = seed[1]
            area = 0
            while stack:
                x, y = stack.pop()
                area += 1
                left, right = min(left, x), max(right, x)
                upper, lower = min(upper, y), max(lower, y)
                for point in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
                    if point in pending:
                        pending.remove(point)
                        stack.append(point)
            if right - left + 1 >= 60 * scale and lower - upper + 1 >= 20 * scale:
                buttons.append({
                    "primary": bool(primary),
                    "bbox": [left, upper, right + 1, lower + 1],
                    "fill_ratio": area / ((right - left + 1) * (lower - upper + 1)),
                })
    buttons.sort(key=lambda button: button["bbox"][0])
    tolerance = 3 * scale
    passed = len(buttons) == 3 and [button["primary"] for button in buttons] == [False, False, True]
    if passed:
        passed = (
            abs(buttons[0]["bbox"][0] - 18 * scale) <= tolerance
            and abs(buttons[-1]["bbox"][2] - (width - 18 * scale)) <= tolerance
            and all(
                abs(button["bbox"][1] - (height - 52 * scale)) <= tolerance
                and abs(button["bbox"][3] - (height - 18 * scale)) <= tolerance
                and button["fill_ratio"] >= 0.6
                for button in buttons
            )
            and all(buttons[i + 1]["bbox"][0] - buttons[i]["bbox"][2] >= 6 * scale for i in (0, 1))
        )
    return {"buttons": buttons, "expected_button_count": 3, "passed": passed}


def verify_confirm_geometry(hwnd: int, report: dict[str, object], simulation: dict[str, object], dark: bool) -> dict[str, object]:
    rect = wintypes.RECT()
    if not user32.GetWindowRect(hwnd, ctypes.byref(rect)):
        raise ctypes.WinError()
    bounds = [rect.left, rect.top, rect.right, rect.bottom]
    work = simulation["work_area_physical"]
    width, height = rect.right - rect.left, rect.bottom - rect.top
    path = Path(report["path"])
    with Image.open(path) as source:
        image = source.convert("RGB")
    footer = confirm_footer_geometry(image, simulation["scale"], dark)
    crop = path.with_name(path.stem + "-buttons.png")
    image.crop((0, max(0, height - round(60 * simulation["scale"])), width, height)).save(crop)
    black_ratio = sum(1 for pixel in image.get_flattened_data() if pixel == (0, 0, 0)) / (width * height)
    geometry_passed = (
        [width, height] == simulation["requested_physical_size"]
        and image.size == (width, height)
        and work[0] <= rect.left < rect.right <= work[2]
        and work[1] <= rect.top < rect.bottom <= work[3]
        and black_ratio < 0.03
        and footer["passed"]
    )
    return {
        **simulation,
        "window_bounds_physical": bounds,
        "button_crop": str(crop.resolve()),
        "black_ratio": round(black_ratio, 4),
        "footer": footer,
        "passed": bool(geometry_passed),
    }


def render_fixture(presenter: Path, output: Path, fixture: str, dark: bool, logical_height: int | None = None) -> dict[str, object]:
    title = {"confirm": "确认下载", "confirm-error": "确认下载", "progress": "下载进度", "complete": "下载完成"}[fixture]
    # 生产辅助函数按标题找 HWND；已有同名窗口时停止，而不是触碰或结束别人的实例。
    if user32.FindWindowW(None, title):
        raise RuntimeError(f"已有同名窗口 {title!r}，请先自行关闭再运行夹具")
    output.mkdir(parents=True, exist_ok=True)
    suffix = f"-logical-{logical_height}" if logical_height is not None else ""
    ready = output / f".{fixture}-{'dark' if dark else 'light'}{suffix}.ready"
    ready.unlink(missing_ok=True)
    environment = os.environ.copy()
    environment["HLS_V7_PRESENTER_READY_FILE"] = str(ready)
    # The software renderer is deterministic under PrintWindow. Production can
    # still use the default GPU backend; this visual fixture validates the same
    # Slint layout and tokens without capturing the window behind it.
    environment["SLINT_BACKEND"] = "winit-software"
    if logical_height is not None:
        # 模拟尺寸只用当前窗口的真实 DPI，排除继承的 Slint 人工缩放覆盖。
        environment.pop("SLINT_SCALE_FACTOR", None)
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
        simulation = simulate_confirm_height(hwnd, logical_height) if logical_height is not None else None
        # 等待尺寸事件和随后的渲染；后面仍须核对实际尺寸，等待本身不是成功证据。
        time.sleep(0.08)
        theme = "dark" if dark else "light"
        report = capture(hwnd, output / f"presenter-{fixture}-{theme}{suffix}.png")
        if simulation is not None:
            geometry = verify_confirm_geometry(hwnd, report, simulation, dark)
            report["geometry"] = geometry
            report["passed"] = bool(report["passed"] and geometry["passed"])
        report.update({
            "fixture": fixture,
            "theme": theme,
            "visible_ms": round((time.perf_counter() - started) * 1000, 2),
        })
        return report
    finally:
        # 仅清理本次 Popen 拥有的进程，不按产品名全局结束进程。
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
        ready.unlink(missing_ok=True)


def verify_close_control(presenter: Path, output: Path, fixture: str) -> dict[str, object]:
    title = {"confirm": "确认下载", "progress": "下载进度", "complete": "下载完成"}[fixture]
    if user32.FindWindowW(None, title):
        raise RuntimeError(f"已有同名窗口 {title!r}，请先自行关闭再运行夹具")
    ready = output / f".{fixture}-close.ready"
    ready.unlink(missing_ok=True)
    environment = os.environ.copy()
    environment["HLS_V7_PRESENTER_READY_FILE"] = str(ready)
    environment["SLINT_BACKEND"] = "winit-software"
    process = subprocess.Popen(
        [str(presenter), "--visual-fixture", fixture],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    started = time.perf_counter()
    try:
        hwnd = wait_window(process.pid, title)
        deadline = time.perf_counter() + 5
        while not ready.exists() and time.perf_counter() < deadline:
            if process.poll() is not None:
                stderr = process.stderr.read().decode("utf-8", errors="replace") if process.stderr else ""
                raise RuntimeError(f"close fixture exited before interaction ({process.returncode}): {stderr}")
            time.sleep(0.01)
        if not ready.exists():
            raise TimeoutError("close fixture did not write its ready marker")

        rect = wintypes.RECT()
        if not user32.GetWindowRect(hwnd, ctypes.byref(rect)):
            raise ctypes.WinError()
        width = rect.right - rect.left
        # Exercise the real Slint TouchArea through the HWND message queue. The
        # close button is a stable 28 px control inset 26 px from the top/right.
        x, y = width - 40, 38
        lparam = (y << 16) | (x & 0xFFFF)
        user32.PostMessageW(hwnd, 0x0200, 0, lparam)  # WM_MOUSEMOVE
        user32.PostMessageW(hwnd, 0x0201, 0x0001, lparam)  # WM_LBUTTONDOWN
        user32.PostMessageW(hwnd, 0x0202, 0, lparam)  # WM_LBUTTONUP
        try:
            exit_status = process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            exit_status = None
        passed = exit_status == 0
        return {
            "fixture": fixture,
            "action": "close",
            "output": "window_closed" if passed else "window_remained_open",
            "exit_status": exit_status,
            "elapsed_ms": round((time.perf_counter() - started) * 1000, 2),
            "passed": passed,
        }
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
        ready.unlink(missing_ok=True)


def check_footer_detector_controls() -> None:
    from PIL import ImageDraw

    # 检测器的正负对照不启动窗口；黑图、缺按钮或按钮下移都必须拒绝。
    for scale in (1.0, 1.25, 1.5, 2.0):
        for dark in (False, True):
            background = (28, 31, 35) if dark else (252, 253, 254)
            fills = [(35, 39, 43), (94, 162, 243)] if dark else [(245, 247, 250), (37, 99, 235)]
            for logical_height in (480, 516, 568, 584):
                size = (round(620 * scale), round(logical_height * scale))
                for count, shift, expected in ((3, 0, True), (2, 0, False), (3, 12, False)):
                    image = Image.new("RGB", size, background)
                    draw = ImageDraw.Draw(image)
                    for index, (left, right) in enumerate(((18, 98), (386, 514), (522, 602))):
                        if index >= count:
                            break
                        box = (
                            round(left * scale), round((logical_height - 52 + shift) * scale),
                            round(right * scale) - 1, round((logical_height - 18 + shift) * scale) - 1,
                        )
                        draw.rounded_rectangle(box, radius=round(7 * scale), fill=fills[index == 2])
                    if confirm_footer_geometry(image, scale, dark)["passed"] != expected:
                        raise AssertionError(f"按钮检测器对照失败：{scale=}, {dark=}, {logical_height=}, {count=}, {shift=}")
            if confirm_footer_geometry(Image.new("RGB", (620, 480)), scale, dark)["passed"]:
                raise AssertionError("按钮检测器错误接受了黑图")


def main() -> int:
    parser = argparse.ArgumentParser(description="Render and reject blank v7 presenter popups")
    parser.add_argument("--presenter", required=True, type=Path)
    parser.add_argument("--output", type=Path, default=Path("artifacts/v7-productization/presenter-visual"))
    parser.add_argument(
        "--confirm-logical-heights", nargs="+", type=int, choices=(320, 480, 516, 568, 584), default=[],
        help="可选：用 SetWindowPos 模拟确认窗逻辑高度（含重复/错误提示）；不改变真实 DPI 或 rcWork",
    )
    args = parser.parse_args()
    if args.confirm_logical_heights:
        check_footer_detector_controls()
    presenter = args.presenter.resolve()
    if not presenter.is_file():
        raise SystemExit(f"presenter not found: {presenter}")
    reports = [
        render_fixture(presenter, args.output, fixture, dark)
        for dark in (False, True)
        for fixture in ("confirm", "progress", "complete")
    ]
    close_controls = [
        verify_close_control(presenter, args.output, fixture)
        for fixture in ("confirm", "progress", "complete")
    ]
    confirm_heights = [
        render_fixture(presenter, args.output, fixture, dark, height)
        for height in args.confirm_logical_heights
        for dark in (False, True)
        for fixture in ("confirm", "confirm-error")
    ]
    result = {
        "fixtures": reports,
        "close_controls": close_controls,
        "confirm_logical_heights": confirm_heights,
        "passed": all(bool(item["passed"]) for item in reports + close_controls + confirm_heights),
    }
    args.output.mkdir(parents=True, exist_ok=True)
    report_path = args.output / "report.json"
    report_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
