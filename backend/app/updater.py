import hashlib
import asyncio
import json
import subprocess
import threading
import time
import urllib.error
import urllib.request
import uuid
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


class UpdateCheckError(UpdateError):
    """A safe, actionable error that can be shown by update clients.

    urllib's messages frequently contain transport implementation details.  They
    are useful in a debug log, but not in the update dialog (and, in practice,
    make a temporary GitHub outage look like an application failure).
    """

    def __init__(
        self,
        message: str,
        *,
        code: str = "UPDATE_CHECK_FAILED",
        retry_after_seconds: int | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.retry_after_seconds = retry_after_seconds

    def to_dict(self) -> dict[str, int | str]:
        detail: dict[str, int | str] = {"message": str(self), "code": self.code}
        if self.retry_after_seconds is not None:
            detail["retry_after_seconds"] = self.retry_after_seconds
        return detail


def _rate_limit_error(error: urllib.error.HTTPError) -> UpdateCheckError | None:
    """Translate GitHub's anonymous API limit to a short user-facing message."""
    if error.code != 403:
        return None
    body = b""
    try:
        body = error.read(4096).lower()
    except OSError:
        pass
    headers = error.headers or {}
    remaining = str(headers.get("X-RateLimit-Remaining", ""))
    if remaining != "0" and b"rate limit" not in body:
        return None

    retry_after_seconds: int | None = None
    try:
        reset = int(str(headers.get("X-RateLimit-Reset", "")))
        retry_after_seconds = max(0, reset - int(time.time()))
    except (TypeError, ValueError):
        pass
    message = "GitHub 暂时限制了匿名更新检查，请稍后重试。"
    if retry_after_seconds:
        minutes = max(1, (retry_after_seconds + 59) // 60)
        message = f"{message}预计约 {minutes} 分钟后可重试。"
    return UpdateCheckError(
        message,
        code="GITHUB_RATE_LIMITED",
        retry_after_seconds=retry_after_seconds,
    )


def _network_error(error: BaseException) -> UpdateCheckError:
    if isinstance(error, urllib.error.HTTPError):
        limited = _rate_limit_error(error)
        if limited:
            return limited
        if 500 <= error.code <= 599:
            return UpdateCheckError(
                "GitHub 更新服务暂时不可用，请稍后重试，或到 Release 页面手动下载。",
                code="GITHUB_UNAVAILABLE",
            )
    return UpdateCheckError(
        "网络连接不稳定，暂时无法检查更新。请稍后重试，或到 Release 页面手动下载。",
        code="NETWORK_ERROR",
    )


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
    except urllib.error.HTTPError as exc:
        raise _network_error(exc) from exc
    except (OSError, ValueError, urllib.error.URLError) as exc:
        raise _network_error(exc) from exc


def _release_redirect_update(*, opener=urllib.request.urlopen) -> UpdateInfo:
    """Find the latest tag without consuming GitHub's REST API allowance.

    GitHub redirects the stable ``/releases/latest`` page to the concrete tag.
    This path deliberately only enables opening the Release page: unlike the
    API response it has no trusted asset digest, so it must never be used for
    unattended installer downloads.
    """
    request = urllib.request.Request(
        LATEST_RELEASE_PAGE,
        headers={
            "Accept": "text/html,application/xhtml+xml",
            "User-Agent": f"HLS-Downloader/{APP_VERSION}",
        },
    )
    try:
        with opener(request, timeout=12) as response:
            destination = str(response.geturl())
    except (OSError, ValueError, urllib.error.URLError) as exc:
        raise _network_error(exc) from exc

    parsed = urlparse(destination)
    prefix = "/ciaooo55/hls-downloader/releases/tag/"
    if parsed.scheme != "https" or parsed.hostname != "github.com" or not parsed.path.startswith(prefix):
        raise UpdateCheckError("GitHub 更新页面返回了无法识别的版本信息。", code="UPDATE_CHECK_FAILED")
    latest = parsed.path.removeprefix(prefix).strip().lstrip("v")
    if not latest:
        raise UpdateCheckError("GitHub 更新页面没有提供版本号。", code="UPDATE_CHECK_FAILED")
    return UpdateInfo(
        current_version=APP_VERSION,
        latest_version=latest,
        available=is_newer_version(latest, APP_VERSION),
        # There is no signed/digested asset descriptor on the page redirect.
        # Keep the safe managed-install path reserved for API-verified assets.
        can_auto_install=False,
        release_url=destination,
        download_url="",
        size=0,
        digest="",
        notes="",
        download_directory=str(get_update_directory()),
    )


def check_for_update(*, opener=urllib.request.urlopen) -> UpdateInfo:
    try:
        payload = _request_json(LATEST_RELEASE_API, opener=opener)
    except UpdateError as api_error:
        if isinstance(api_error, UpdateCheckError) and api_error.code == "GITHUB_RATE_LIMITED":
            # Match the resilient pattern used by mature desktop updaters:
            # retry through a non-API stable endpoint.  It prevents a shared
            # NAT's 60-request REST allowance from masquerading as a failed
            # update check, while preserving checksum requirements for install.
            try:
                return _release_redirect_update(opener=opener)
            except UpdateCheckError:
                # Preserve the actionable rate-limit state if the independent
                # fallback is also unavailable (for example, an offline PC).
                raise api_error
        raise api_error

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
    # Production updates use the same range probing, concurrent chunks,
    # retries, throttling and checksum path as ordinary HTTP tasks. Keep the
    # injected opener path below for deterministic transport unit tests.
    if opener is urllib.request.urlopen:
        from .config import settings
        from .downloader.http_file import HTTPDownloader
        from .models import Task, TaskStatus, TaskType

        task = Task(
            id=f"update-{uuid.uuid4().hex}",
            url=info.download_url,
            task_type=TaskType.HTTP,
            title=f"HLS Downloader v{info.latest_version} 更新",
            filename=f"HLSDownloader-Update-{info.latest_version}.exe",
            concurrency=max(1, int(settings.default_concurrency)),
            expected_checksum=f"sha256:{info.digest}",
            engine_state={
                "output_dir": str(root),
                "temp_dir": str(RUNTIME_PATHS.data_root / "updates"),
            },
        )
        task.cancel_event = asyncio.Event()
        task.pause_event = asyncio.Event()
        asyncio.run(HTTPDownloader(task).run())
        if task.status is not TaskStatus.DONE or not task.output_path:
            raise UpdateError(task.error_message or "安装包下载失败，请检查网络后重试。")
        destination = Path(task.output_path)
        if info.size and destination.stat().st_size != info.size:
            destination.unlink(missing_ok=True)
            raise UpdateError(f"安装包大小不匹配：期望 {info.size}，实际 {destination.stat().st_size if destination.exists() else 0}")
        with destination.open("rb") as handle:
            if handle.read(2) != b"MZ":
                destination.unlink(missing_ok=True)
                raise UpdateError("下载结果不是有效的 Windows 安装程序")
        return destination

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
        raise UpdateError("安装包下载失败，请检查网络后重试。") from exc
    finally:
        temporary.unlink(missing_ok=True)


async def queue_update_download(info: UpdateInfo, manager) -> object:
    """Create a real, persisted HTTP task for the installer download.

    Updates must have the same pause/resume, queue and progress semantics as
    user downloads. The task identity is stable for a version+digest pair so a
    restart can resume the partial installer instead of starting over.
    """
    from .models import TaskStatus, TaskType

    if not info.available:
        raise UpdateError("当前已经是最新版本")
    if not info.can_auto_install:
        raise UpdateError("自动安装仅适用于安装版")
    identity = f"{info.latest_version}:{info.digest.lower()}"
    for task in manager.tasks.values():
        if task.engine_state.get("update_identity") != identity:
            continue
        if task.status is TaskStatus.DONE and task.output_path and Path(task.output_path).is_file():
            return task
        if task.status is TaskStatus.PAUSED:
            await manager.resume_task(task.id)
        elif task.status is TaskStatus.QUEUED and not task.task_handle:
            await manager.start_task(task.id)
        return task

    task = await manager.create_task(
        url=info.download_url,
        task_type=TaskType.HTTP,
        title=f"HLS Downloader v{info.latest_version} 更新",
        filename=f"HLSDownloader-Update-{info.latest_version}.exe",
        checksum=f"sha256:{info.digest}",
        output_dir=str(get_update_directory()),
        auto_start=False,
        inherit_default_headers=False,
    )
    task.engine_state.update({
        "is_update": True,
        "update_identity": identity,
        "update_version": info.latest_version,
        "update_expected_size": info.size,
        "temp_dir": str(RUNTIME_PATHS.data_root / "updates"),
    })
    task.last_log = f"正在下载 v{info.latest_version} 更新安装包"
    await manager._save_db(task)
    await manager.start_task(task.id)
    return task


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
    CACHE_TTL_SECONDS = 15 * 60
    INSTALL_CACHE_TTL_SECONDS = 24 * 60 * 60

    def __init__(self) -> None:
        self._cache: tuple[float, UpdateInfo] | None = None
        self._install_lock = threading.Lock()
        self._install_started = False

    def check(self, *, force: bool = False) -> UpdateInfo:
        if not force and self._cache and time.monotonic() - self._cache[0] < self.CACHE_TTL_SECONDS:
            return self._cache[1]
        info = check_for_update()
        self._cache = (time.monotonic(), info)
        return info

    def _cached_installable_update(self) -> UpdateInfo | None:
        """Return only an in-memory result previously validated by `check`.

        The install flow must not turn a successful update discovery into a
        failure merely because a second GitHub API request is rate limited.
        This cache is process-local and contains the digest obtained from the
        trusted release API or checksum fallback; it is never populated from a
        client request.
        """
        if not self._cache:
            return None
        checked_at, info = self._cache
        if time.monotonic() - checked_at > self.INSTALL_CACHE_TTL_SECONDS:
            return None
        if not info.available or not info.can_auto_install:
            return None
        if len(info.digest) != 64 or any(char not in "0123456789abcdef" for char in info.digest.lower()):
            return None
        return info

    def prepare_managed_download(self) -> UpdateInfo:
        """Get a verified update descriptor without creating a transient task."""
        if self._install_started:
            raise UpdateError("更新安装程序已经启动")
        return self._cached_installable_update() or self.check(force=True)

    def download_and_launch(self, *, process_starter=subprocess.Popen) -> UpdateInfo:
        if not self._install_lock.acquire(blocking=False):
            raise UpdateError("更新程序正在下载，请勿重复操作")
        try:
            if self._install_started:
                raise UpdateError("更新安装程序已经启动")
            info = self._cached_installable_update() or self.check(force=True)
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
