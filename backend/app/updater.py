import hashlib
import json
import subprocess
import threading
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass
from pathlib import Path
from urllib.parse import urlparse

from .paths import RUNTIME_PATHS
from .version import APP_VERSION


LATEST_RELEASE_API = "https://api.github.com/repos/ciaooo55/hls-downloader/releases/latest"
LATEST_RELEASE_PAGE = "https://github.com/ciaooo55/hls-downloader/releases/latest"
RELEASE_DOWNLOAD_PREFIX = "/ciaooo55/hls-downloader/releases/download/"
SETUP_ASSET_NAME = "HLSDownloader-Windows-x64-Setup.exe"
MAX_INSTALLER_BYTES = 400 * 1024 * 1024


class UpdateError(RuntimeError):
    pass


@dataclass(frozen=True)
class UpdateInfo:
    current_version: str
    latest_version: str
    available: bool
    can_auto_install: bool
    release_url: str
    download_url: str
    size: int
    digest: str
    notes: str
    download_directory: str = ""

    def to_dict(self) -> dict:
        return asdict(self)


def _version_parts(value: str) -> tuple[int, ...]:
    clean = value.strip().lower().lstrip("v").split("-", 1)[0]
    try:
        parts = tuple(int(part) for part in clean.split("."))
    except ValueError as exc:
        raise UpdateError(f"无法识别版本号：{value}") from exc
    if not parts:
        raise UpdateError(f"无法识别版本号：{value}")
    return parts


def is_newer_version(candidate: str, current: str) -> bool:
    left = _version_parts(candidate)
    right = _version_parts(current)
    width = max(len(left), len(right))
    return left + (0,) * (width - len(left)) > right + (0,) * (width - len(right))


def get_update_directory() -> Path:
    # Import lazily so updater helpers remain usable while configuration starts up.
    from .config import settings

    return Path(settings.download_dir).expanduser().resolve()


def _request_json(url: str, *, opener=urllib.request.urlopen) -> dict:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": f"HLS-Downloader/{APP_VERSION}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with opener(request, timeout=12) as response:
            return json.load(response)
    except (OSError, ValueError, urllib.error.URLError) as exc:
        raise UpdateError(f"无法连接 GitHub 检查更新：{exc}") from exc


def check_for_update(*, opener=urllib.request.urlopen) -> UpdateInfo:
    try:
        payload = _request_json(LATEST_RELEASE_API, opener=opener)
    except UpdateError as api_error:
        try:
            return _check_from_release_files(opener=opener)
        except UpdateError as fallback_error:
            raise UpdateError(f"{api_error}；备用检查也失败：{fallback_error}") from fallback_error

    latest = str(payload.get("tag_name", "")).strip().lstrip("v")
    if not latest:
        raise UpdateError("GitHub 最新版本信息中缺少版本号")

    asset = next(
        (item for item in payload.get("assets", []) if item.get("name") == SETUP_ASSET_NAME),
        None,
    )
    if not asset:
        raise UpdateError("最新版本没有 Windows 安装包")

    digest = str(asset.get("digest", "")).lower()
    if not digest.startswith("sha256:") or len(digest.removeprefix("sha256:")) != 64:
        raise UpdateError("最新安装包缺少有效的 SHA-256 校验值")

    size = int(asset.get("size", 0) or 0)
    if size <= 0 or size > MAX_INSTALLER_BYTES:
        raise UpdateError("最新安装包的文件大小无效")

    download_url = str(asset.get("browser_download_url", ""))
    parsed = urlparse(download_url)
    if (
        parsed.scheme != "https"
        or parsed.hostname != "github.com"
        or not parsed.path.startswith(RELEASE_DOWNLOAD_PREFIX)
    ):
        raise UpdateError("最新安装包的下载地址不可信")

    return UpdateInfo(
        current_version=APP_VERSION,
        latest_version=latest,
        available=is_newer_version(latest, APP_VERSION),
        can_auto_install=RUNTIME_PATHS.mode == "installed",
        release_url=str(payload.get("html_url", "")),
        download_url=download_url,
        size=size,
        digest=digest.removeprefix("sha256:"),
        notes=str(payload.get("body", ""))[:4000],
        download_directory=str(get_update_directory()),
    )


def _check_from_release_files(*, opener=urllib.request.urlopen) -> UpdateInfo:
    page_request = urllib.request.Request(
        LATEST_RELEASE_PAGE,
        headers={"User-Agent": f"HLS-Downloader/{APP_VERSION}"},
    )
    try:
        with opener(page_request, timeout=15) as response:
            final_url = response.geturl()
    except (OSError, urllib.error.URLError) as exc:
        raise UpdateError(f"无法读取 GitHub 最新 Release：{exc}") from exc

    parsed_release = urlparse(final_url)
    tag_prefix = "/ciaooo55/hls-downloader/releases/tag/"
    if parsed_release.hostname != "github.com" or not parsed_release.path.startswith(tag_prefix):
        raise UpdateError("GitHub 最新 Release 跳转地址无效")
    tag = parsed_release.path.removeprefix(tag_prefix).split("/", 1)[0]
    latest = tag.lstrip("v")
    _version_parts(latest)

    base = f"https://github.com/ciaooo55/hls-downloader/releases/download/{tag}"
    checksums_request = urllib.request.Request(
        f"{base}/SHA256SUMS.txt",
        headers={"User-Agent": f"HLS-Downloader/{APP_VERSION}"},
    )
    try:
        with opener(checksums_request, timeout=15) as response:
            checksums = response.read(64 * 1024 + 1)
    except (OSError, urllib.error.URLError) as exc:
        raise UpdateError(f"无法读取最新版本校验文件：{exc}") from exc
    if len(checksums) > 64 * 1024:
        raise UpdateError("最新版本校验文件过大")

    digest = ""
    for line in checksums.decode("ascii", errors="ignore").splitlines():
        parts = line.strip().split()
        if len(parts) == 2 and parts[1] == SETUP_ASSET_NAME:
            digest = parts[0].lower()
            break
    if len(digest) != 64 or any(char not in "0123456789abcdef" for char in digest):
        raise UpdateError("校验文件中没有 Windows 安装包的 SHA-256")

    return UpdateInfo(
        current_version=APP_VERSION,
        latest_version=latest,
        available=is_newer_version(latest, APP_VERSION),
        can_auto_install=RUNTIME_PATHS.mode == "installed",
        release_url=final_url,
        download_url=f"{base}/{SETUP_ASSET_NAME}",
        size=0,
        digest=digest,
        notes="",
        download_directory=str(get_update_directory()),
    )


def download_installer(
    info: UpdateInfo,
    *,
    opener=urllib.request.urlopen,
    destination_root: Path | None = None,
) -> Path:
    if not info.available:
        raise UpdateError("当前已经是最新版本")
    if not info.can_auto_install:
        raise UpdateError("自动安装仅适用于安装版")

    root = destination_root or get_update_directory()
    root.mkdir(parents=True, exist_ok=True)
    destination = root / f"HLSDownloader-Update-{info.latest_version}.exe"
    temporary = destination.with_suffix(".exe.part")
    temporary.unlink(missing_ok=True)
    digest = hashlib.sha256()
    written = 0
    request = urllib.request.Request(
        info.download_url,
        headers={"User-Agent": f"HLS-Downloader/{APP_VERSION}"},
    )

    try:
        with opener(request, timeout=60) as response, temporary.open("wb") as output:
            while chunk := response.read(1024 * 1024):
                written += len(chunk)
                if written > MAX_INSTALLER_BYTES:
                    raise UpdateError("安装包超过允许的最大大小")
                digest.update(chunk)
                output.write(chunk)
        if info.size and written != info.size:
            raise UpdateError(f"安装包大小不匹配：期望 {info.size}，实际 {written}")
        if digest.hexdigest().lower() != info.digest.lower():
            raise UpdateError("安装包 SHA-256 校验失败")
        with temporary.open("rb") as handle:
            if handle.read(2) != b"MZ":
                raise UpdateError("下载结果不是有效的 Windows 安装程序")
        temporary.replace(destination)
        return destination
    except (OSError, urllib.error.URLError) as exc:
        raise UpdateError(f"下载安装包失败：{exc}") from exc
    finally:
        temporary.unlink(missing_ok=True)


def cleanup_update_cache() -> None:
    roots = {get_update_directory(), RUNTIME_PATHS.data_root / "updates"}
    for root in roots:
        if not root.is_dir():
            continue
        for item in root.glob("HLSDownloader-Update-*.exe*"):
            if not item.is_file():
                continue
            try:
                item.unlink()
            except OSError:
                pass


class UpdateService:
    def __init__(self) -> None:
        self._cache: tuple[float, UpdateInfo] | None = None
        self._install_lock = threading.Lock()
        self._install_started = False

    def check(self, *, force: bool = False) -> UpdateInfo:
        if not force and self._cache and time.monotonic() - self._cache[0] < 900:
            return self._cache[1]
        info = check_for_update()
        self._cache = (time.monotonic(), info)
        return info

    def download_and_launch(self, *, process_starter=subprocess.Popen) -> UpdateInfo:
        if not self._install_lock.acquire(blocking=False):
            raise UpdateError("更新程序正在下载，请勿重复操作")
        try:
            if self._install_started:
                raise UpdateError("更新安装程序已经启动")
            info = self.check(force=True)
            installer = download_installer(info)
            try:
                process_starter([str(installer), "/DELETESELF=1"])
            except OSError as exc:
                raise UpdateError(f"无法启动更新安装程序：{exc}") from exc
            self._install_started = True
            return info
        finally:
            self._install_lock.release()


update_service = UpdateService()
