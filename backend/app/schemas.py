from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator
from datetime import datetime, timezone
from typing import Literal, Optional
from urllib.parse import urlparse

from .version import APP_VERSION
from .models import TaskType
from .checksum import normalize_checksum
from .downloader.mirrors import normalize_mirror_urls
from .tvbox import normalize_tvbox_endpoint
from .dlna import normalize_cast_device
from .credentials import SECRET_MASK


class BrowserHandoffCreate(BaseModel):
    """Bounded browser-to-desktop request accepted over HTTP/Native Messaging."""

    model_config = ConfigDict(extra="ignore")

    url: str = Field(max_length=8192)
    filename: str = Field(default="", max_length=255)
    title: str = Field(default="", max_length=512)
    mime_type: str = Field(default="", max_length=255)
    source_page_url: str = Field(default="", max_length=8192)
    resource_kind: str = Field(default="file", max_length=32)
    referer: str = Field(default="", max_length=4096)
    origin: str = Field(default="", max_length=1024)
    cookie: str = Field(default="", max_length=16 * 1024)
    user_agent: str = Field(default="", max_length=2048)
    request_headers: dict[str, str] = Field(default_factory=dict, max_length=64)
    request_contexts: dict[str, dict] = Field(default_factory=dict, max_length=12)
    request_method: str = Field(default="GET", max_length=16)
    request_body: str = Field(default="", max_length=175000)
    size: int = Field(default=0, ge=0, le=2**63 - 1)
    extension_version: str = Field(default="", max_length=64)
    extension_client_id: str = Field(default="", max_length=128)
    extension_browser: str = Field(default="", max_length=32)
    client_request_id: str = Field(default="", max_length=160)

    @field_validator("url")
    @classmethod
    def validate_browser_url(cls, value: str) -> str:
        value = str(value or "").strip()
        parsed = urlparse(value)
        if parsed.scheme == "magnet" and parsed.query:
            return value
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("url 必须是有效的 HTTP(S) 或 magnet 地址")
        return value


class BrowserPing(BaseModel):
    model_config = ConfigDict(extra="ignore")

    version: str = Field(default="", max_length=64)
    client_id: str = Field(default="", max_length=128)
    browser: str = Field(default="", max_length=32)


class BrowserTakeoverSettings(BaseModel):
    model_config = ConfigDict(extra="forbid")

    enabled: Optional[bool] = None
    minimum_bytes: Optional[int] = Field(default=None, ge=0, le=2**63 - 1)


class BrowserMediaPushCreate(BaseModel):
    model_config = ConfigDict(extra="ignore")

    kind: Literal["cast", "tvbox"]
    resource: BrowserHandoffCreate


class BrowserMediaPushComplete(BaseModel):
    model_config = ConfigDict(extra="ignore")

    status: Literal["done", "failed", "canceled"]
    message: str = Field(default="", max_length=300)

class TaskCreate(BaseModel):
    url: str = Field(max_length=8192)
    task_type: TaskType = TaskType.AUTO
    source_page_url: str = Field(default="", max_length=8192)
    mime_type: str = Field(default="", max_length=255)
    referer: str = Field(default="", max_length=4096)
    origin: str = Field(default="", max_length=1024)
    user_agent: str = Field(default="", max_length=2048)
    cookie: str = Field(default="", max_length=16 * 1024)
    request_headers: dict[str, str] = Field(default_factory=dict, max_length=64)
    request_contexts: dict[str, dict] = Field(default_factory=dict, max_length=12)
    request_method: str = Field(default="GET", max_length=16)
    request_body: str = Field(default="", max_length=175000)
    title: str = Field(default="", max_length=512)
    filename: str = Field(default="", max_length=255)
    download_dir: str = Field(default="", max_length=32767)
    concurrency: int = Field(default=0, ge=0, le=64)
    checksum: str = Field(default="", max_length=80)
    allow_duplicate: bool = False
    selected_video: str = Field(default="", max_length=2048)
    selected_audio: str = Field(default="", max_length=256)
    scheduled_start_at: Optional[datetime] = None
    scheduled_stop_at: Optional[datetime] = None
    completion_action: Literal["none", "shutdown", "sleep", "hibernate"] = "none"
    mirrors: list[str] = Field(default_factory=list, max_length=16)

    @field_validator("scheduled_start_at", "scheduled_stop_at")
    @classmethod
    def normalize_schedule_to_utc(cls, value: Optional[datetime]) -> Optional[datetime]:
        if value is None:
            return None
        # HTML datetime-local has no offset. Interpret it in the user's current
        # Windows time zone at creation, then persist an unambiguous UTC value.
        if value.tzinfo is None:
            value = value.astimezone()
        return value.astimezone(timezone.utc)

    @field_validator("url")
    @classmethod
    def validate_url(cls, value: str) -> str:
        parsed = urlparse(value)
        if parsed.scheme == "magnet" and parsed.query:
            return value
        if parsed.scheme in {"ftp", "ftps", "sftp"} and parsed.hostname:
            return value
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("url 必须是有效的 HTTP(S)、FTP(S)、SFTP 或 magnet 地址")
        return value

    @field_validator("checksum")
    @classmethod
    def validate_checksum(cls, value: str) -> str:
        raw = str(value or "").strip()
        if not raw:
            return ""
        algorithm, digest = normalize_checksum(raw)
        return f"{algorithm}:{digest}"

    @model_validator(mode="after")
    def validate_schedule_window(self):
        if (
            self.scheduled_start_at is not None
            and self.scheduled_stop_at is not None
            and self.scheduled_stop_at.timestamp() <= self.scheduled_start_at.timestamp()
        ):
            raise ValueError("计划停止时间必须晚于计划开始时间")
        lowered = str(self.url or "").lower()
        if lowered.startswith("magnet:") and self.mirrors:
            raise ValueError("magnet 任务不支持备用下载地址")
        if lowered.startswith(("ftp://", "ftps://", "sftp://")) and self.mirrors:
            raise ValueError("FTP/SFTP 任务不支持备用下载地址")
        self.mirrors = normalize_mirror_urls(self.url, self.mirrors)
        return self

class TaskBatchCreate(BaseModel):
    tasks: list[TaskCreate] = Field(min_length=1, max_length=100)


class TaskRequestUpdate(BaseModel):
    """Replace an expiring download request without discarding task data."""

    url: str = Field(max_length=8192)
    source_page_url: Optional[str] = Field(default=None, max_length=8192)
    mime_type: Optional[str] = Field(default=None, max_length=255)
    referer: Optional[str] = Field(default=None, max_length=4096)
    origin: Optional[str] = Field(default=None, max_length=1024)
    user_agent: Optional[str] = Field(default=None, max_length=2048)
    cookie: Optional[str] = Field(default=None, max_length=16 * 1024)
    request_headers: Optional[dict[str, str]] = Field(default=None, max_length=64)
    request_contexts: Optional[dict[str, dict]] = Field(default=None, max_length=12)
    request_method: Optional[str] = Field(default=None, max_length=16)
    request_body: Optional[str] = Field(default=None, max_length=175000)
    auto_resume: bool = True

    @field_validator("url")
    @classmethod
    def validate_url(cls, value: str) -> str:
        value = str(value or "").strip()
        parsed = urlparse(value)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("url 必须是有效的 HTTP(S) 地址")
        return value

class TaskResponse(BaseModel):
    id: str
    task_type: str = "hls"
    request_method: str = "GET"
    source_page_url: str = ""
    mime_type: str = ""
    title: str
    url: str
    referer: str
    origin: str
    user_agent: str
    cookie: str
    filename: str
    download_dir: str = ""
    concurrency: int
    status: str
    stage: str
    last_log: str
    total_segments: int
    completed_segments: int
    failed_segments: int
    downloaded_bytes: int
    total_bytes: int
    speed_bytes_per_sec: float
    eta_seconds: float
    active_workers: int = 0
    max_workers: int = 0
    reconnect_count: int = 0
    connection_status: str = "idle"
    last_worker_error: str = ""
    post_percent: float = 0.0
    active_slots: int = 0
    active_segment_indexes: list[int] = Field(default_factory=list)
    playable_segments: int = 0
    playable_duration: float = 0.0
    media_duration: float = 0.0
    progress_percent: float = 0.0
    uploaded_bytes: int = 0
    upload_speed_bytes_per_sec: float = 0.0
    peer_count: int = 0
    seed_count: int = 0
    playback_ready: bool = False
    is_live: bool = False
    speed_limit_kib: int = 0
    error_message: str
    error_code: str = ""
    error_stage: str = ""
    error_url: str = ""
    error_hint: str = ""
    http_status: int = 0
    error_attempt: int = 0
    output_path: str
    expected_checksum: str = ""
    checksum_algorithm: str = ""
    checksum_actual: str = ""
    checksum_verified: Optional[bool] = None
    output_is_file: bool = False
    output_missing: bool = False
    file_access_token: str = ""
    created_at: str
    updated_at: str
    started_at: str = ""
    finished_at: str = ""
    available_actions: list[str] = Field(default_factory=list)
    queue_position: int = 0
    scheduled_start_at: str = ""
    scheduled_stop_at: str = ""
    completion_action: str = "none"
    mirrors: list[str] = Field(default_factory=list)
    mirror_status: list[dict] = Field(default_factory=list)
    av_scan: dict = Field(default_factory=dict)
    speed_history: list[int] = Field(default_factory=list)
    speed_peak_bytes_per_sec: float = 0.0
    connection_parts: list[dict] = Field(default_factory=list)

class TaskSpeedLimit(BaseModel):
    limit_kib: int = Field(ge=0, le=1048576)


class SiteProfile(BaseModel):
    host: str = Field(min_length=1, max_length=255)
    enabled: bool = True
    user_agent: str = Field(default="", max_length=2048)
    referer: str = Field(default="", max_length=4096)
    origin: str = Field(default="", max_length=1024)
    request_headers: dict[str, str] = Field(default_factory=dict, max_length=64)
    cookie: str = Field(default="", max_length=16 * 1024)
    download_dir: str = Field(default="", max_length=32767)
    concurrency: int = Field(default=0, ge=0, le=64)
    speed_limit_kib: int = Field(default=0, ge=0, le=1048576)
    proxy_mode: str = Field(default="", pattern=r"^(|direct|system|manual)$")
    proxy_url: str = Field(default="", max_length=2048)

    @field_validator("proxy_url")
    @classmethod
    def validate_site_proxy_url(cls, value: str) -> str:
        return SettingsUpdate.validate_proxy_url(value)


class SettingsUpdate(BaseModel):
    download_dir: Optional[str] = Field(default=None, min_length=1, max_length=32767)
    temp_dir: Optional[str] = Field(default=None, min_length=1, max_length=32767)
    default_concurrency: Optional[int] = Field(default=None, ge=1, le=64)
    default_user_agent: Optional[str] = Field(default=None, max_length=2048)
    default_referer: Optional[str] = Field(default=None, max_length=4096)
    default_origin: Optional[str] = Field(default=None, max_length=1024)
    default_cookie: Optional[str] = Field(default=None, max_length=16 * 1024)
    ffmpeg_path: Optional[str] = Field(default=None, min_length=1, max_length=32767)
    allowed_hosts: Optional[list[str]] = Field(default=None, max_length=100)
    keep_temp_files: Optional[bool] = None
    max_concurrent_tasks: Optional[int] = Field(default=None, ge=1, le=16)
    http_chunk_size_mb: Optional[int] = Field(default=None, ge=1, le=64)
    download_speed_limit_kib: Optional[int] = Field(default=None, ge=0, le=1048576)
    speed_schedule_enabled: Optional[bool] = None
    speed_schedule_start: Optional[str] = Field(default=None, pattern=r"^([01]\d|2[0-3]):[0-5]\d$")
    speed_schedule_end: Optional[str] = Field(default=None, pattern=r"^([01]\d|2[0-3]):[0-5]\d$")
    speed_schedule_limit_kib: Optional[int] = Field(default=None, ge=0, le=1048576)
    bt_upload_limit_kib: Optional[int] = Field(default=None, ge=0, le=1048576)
    bt_max_connections: Optional[int] = Field(default=None, ge=10, le=1000)
    bt_enable_dht: Optional[bool] = None
    watch_torrents: Optional[bool] = None
    watch_dir: Optional[str] = Field(default=None, max_length=32767)
    browser_takeover_enabled: Optional[bool] = None
    browser_takeover_min_mb: Optional[int] = Field(default=None, ge=0, le=10240)
    browser_category_dirs: Optional[dict[str, str]] = None
    auto_category_dirs: Optional[bool] = None
    queue_auto_start_enabled: Optional[bool] = None
    queue_auto_start_time: Optional[str] = Field(default=None, pattern=r"^([01]\d|2[0-3]):[0-5]\d$")
    queue_auto_stop_enabled: Optional[bool] = None
    queue_auto_stop_time: Optional[str] = Field(default=None, pattern=r"^([01]\d|2[0-3]):[0-5]\d$")
    queue_active_days: Optional[list[int]] = Field(default=None, min_length=1, max_length=7)
    live_record_max_minutes: Optional[int] = Field(default=None, ge=0, le=2880)
    download_subtitles: Optional[bool] = None
    skip_ad_segments: Optional[bool] = None
    clipboard_watch: Optional[bool] = None
    completion_sound_enabled: Optional[bool] = None
    resume_interrupted_on_startup: Optional[bool] = None
    auto_retry_failed_max: Optional[int] = Field(default=None, ge=0, le=10)
    av_scan_enabled: Optional[bool] = None
    av_scan_command: Optional[str] = Field(default=None, max_length=2048)
    av_scan_fail_on_threat: Optional[bool] = None
    existing_file_policy: Optional[str] = Field(default=None, pattern="^(rename|overwrite|skip)$")

    @field_validator("av_scan_command")
    @classmethod
    def validate_av_scan_command(cls, value: Optional[str]) -> Optional[str]:
        if value is None:
            return None
        text = str(value).strip()
        if not text:
            return ""
        if "{file}" not in text or any(ord(character) < 32 for character in text):
            raise ValueError("av_scan_command must include {file}")
        return text
    tvbox_endpoint: Optional[str] = Field(default=None, max_length=512)
    cast_device: Optional[dict[str, str]] = None
    site_profiles: Optional[list[SiteProfile]] = Field(default=None, max_length=100)
    proxy_mode: Optional[str] = Field(default=None, pattern="^(system|direct|manual)$")
    proxy_url: Optional[str] = Field(default=None, max_length=2048)
    proxy_bypass: Optional[list[str]] = Field(default=None, max_length=100)

    @field_validator("proxy_url")
    @classmethod
    def validate_proxy_url(cls, value: Optional[str]) -> str:
        value = str(value or "").strip()
        if not value:
            return ""
        if value == SECRET_MASK:
            return value
        parsed = urlparse(value)
        try:
            port = parsed.port
        except ValueError as exc:
            raise ValueError("proxy_url 端口无效") from exc
        if (
            parsed.scheme.lower() not in {"http", "https", "socks5", "socks5h"}
            or not parsed.hostname
            or parsed.query
            or parsed.fragment
            or port == 0
            or any(ord(character) < 32 or ord(character) == 127 for character in value)
        ):
            raise ValueError("proxy_url 必须是 HTTP(S) 或 SOCKS5 代理地址")
        return value

    @field_validator("queue_active_days")
    @classmethod
    def validate_queue_active_days(cls, value: Optional[list[int]]) -> Optional[list[int]]:
        if value is None:
            return None
        normalized = sorted(set(value))
        if any(day < 0 or day > 6 for day in normalized):
            raise ValueError("星期编号必须在 0 到 6 之间")
        return normalized

    @field_validator("allowed_hosts")
    @classmethod
    def validate_allowed_hosts(cls, value: Optional[list[str]]) -> list[str] | None:
        if value is None:
            return None
        result = []
        for item in value:
            pattern = str(item or "").strip().lower()
            if (
                not pattern
                or len(pattern) > 255
                or any(character in pattern for character in "\r\n/\\@")
            ):
                raise ValueError("Host 规则无效")
            if pattern not in result:
                result.append(pattern)
        return result

    @field_validator("proxy_bypass")
    @classmethod
    def validate_proxy_bypass(cls, value: Optional[list[str]]) -> list[str] | None:
        if value is None:
            return None
        result = []
        for item in value:
            pattern = str(item or "").strip().lower()
            if (
                not pattern
                or len(pattern) > 255
                or any(ord(character) < 32 or ord(character) == 127 for character in pattern)
                or any(character in pattern for character in "\\@")
                or "://" in pattern
            ):
                raise ValueError("代理绕过规则无效")
            if "/" in pattern:
                from ipaddress import ip_network

                try:
                    ip_network(pattern, strict=False)
                except ValueError as exc:
                    raise ValueError("代理绕过 CIDR 规则无效") from exc
            if pattern not in result:
                result.append(pattern)
        return result

    @field_validator("tvbox_endpoint")
    @classmethod
    def validate_tvbox_endpoint(cls, value: Optional[str]) -> Optional[str]:
        value = str(value or "").strip()
        if not value:
            return ""
        try:
            return normalize_tvbox_endpoint(value)
        except ValueError as exc:
            raise ValueError(f"tvbox_endpoint：{exc}") from exc

    @field_validator("cast_device")
    @classmethod
    def validate_cast_device(cls, value: Optional[dict[str, str]]) -> dict[str, str]:
        try:
            return normalize_cast_device(value)
        except ValueError as exc:
            raise ValueError(f"cast_device：{exc}") from exc


class LegalAcceptanceRequest(BaseModel):
    version: str = Field(min_length=1, max_length=64)
    document_digest: str = Field(min_length=64, max_length=64, pattern=r"^[0-9a-f]{64}$")
    accepted: bool = False


class BrowserHandoffAccept(BaseModel):
    filename: str = Field(default="", max_length=255)
    download_dir: str = Field(default="", max_length=2048)
    category: str = Field(default="other", pattern="^(media|program|archive|other)$")
    remember: bool = True
    # Empty means "use the browser-captured source-page context".  Values are
    # optional overrides entered explicitly in the confirmation window.
    cookie: str = Field(default="", max_length=16 * 1024)
    request_headers: dict[str, str] = Field(default_factory=dict)


class BrowserHandoffCancel(BaseModel):
    suppress_site_kind: bool = False


class TvboxPush(BaseModel):
    url: str = Field(min_length=1, max_length=8192)
    endpoint: str = Field(default="", max_length=512)

    @field_validator("url")
    @classmethod
    def validate_url(cls, value: str) -> str:
        value = str(value or "").strip()
        parsed = urlparse(value)
        if parsed.scheme.lower() not in {"http", "https"} or not parsed.hostname:
            raise ValueError("待推送的视频地址必须是有效的 HTTP(S) 地址")
        return value


class TvboxLocalPush(BaseModel):
    path: str = Field(min_length=1, max_length=32767)
    endpoint: str = Field(default="", max_length=512)

    @field_validator("path")
    @classmethod
    def validate_path(cls, value: str) -> str:
        value = str(value or "").strip()
        if not value:
            raise ValueError("请先选择要推送的本机文件")
        return value


class CastLocalPush(BaseModel):
    path: str = Field(min_length=1, max_length=32767)
    device: dict[str, str] | None = None

    @field_validator("path")
    @classmethod
    def validate_path(cls, value: str) -> str:
        value = str(value or "").strip()
        if not value:
            raise ValueError("请先选择要投屏的本机文件")
        return value


class TvboxTaskPush(BaseModel):
    task_id: str = Field(min_length=1, max_length=64, pattern=r"^[A-Za-z0-9_-]+$")
    endpoint: str = Field(default="", max_length=512)


class CastTaskPush(BaseModel):
    task_id: str = Field(min_length=1, max_length=64, pattern=r"^[A-Za-z0-9_-]+$")
    device: dict[str, str] | None = None


class CastUrlPush(BaseModel):
    url: str = Field(min_length=1, max_length=8192)
    filename: str = Field(default="", max_length=255)
    device: dict[str, str] | None = None

    @field_validator("url")
    @classmethod
    def validate_url(cls, value: str) -> str:
        value = str(value or "").strip()
        parsed = urlparse(value)
        if parsed.scheme.lower() not in {"http", "https"} or not parsed.hostname:
            raise ValueError("待投屏地址必须是有效的 HTTP(S) 地址")
        return value


class CastControl(BaseModel):
    action: str = Field(pattern="^(play|pause|seek|seek_to|status|stop)$")
    seconds: int = Field(default=0, ge=-86400, le=86400)
    device: dict[str, str] | None = None

class HealthResponse(BaseModel):
    status: str = "ok"
    version: str = APP_VERSION
    app_id: str = "com.ciaooo55.hls-downloader"
    protocol_version: int = 3
    authenticated: bool = False


class PlaybackSeekRequest(BaseModel):
    time: float = Field(ge=0, le=86400)


class TorrentFileSelection(BaseModel):
    indexes: list[int] = Field(min_length=1, max_length=10000)


class TorrentPathImport(BaseModel):
    path: str = Field(min_length=1, max_length=32767)


class LinkPathImport(BaseModel):
    path: str = Field(min_length=1, max_length=32767)
    auto_start: bool = True


class FileSystemAction(BaseModel):
    task_id: str = Field(default="", max_length=64, pattern=r"^[A-Za-z0-9_-]*$")
    path: str = Field(default="", max_length=32767)
    confirm_executable: bool = False

    @model_validator(mode="after")
    def require_target(self):
        if not self.task_id and not self.path:
            raise ValueError("task_id or path required")
        return self






class PageHarvestProbeRequest(BaseModel):
    urls: list[str] = Field(min_length=1, max_length=100)
    referer: str = Field(default="", max_length=4096)
    origin: str = Field(default="", max_length=1024)
    user_agent: str = Field(default="", max_length=2048)
    cookie: str = Field(default="", max_length=16 * 1024)
    request_headers: dict[str, str] = Field(default_factory=dict)

    @field_validator("urls")
    @classmethod
    def validate_probe_urls(cls, value: list[str]) -> list[str]:
        cleaned: list[str] = []
        seen: set[str] = set()
        for item in value or []:
            url = str(item or "").strip()
            lowered = url.lower()
            if not url or len(url) > 8192:
                continue
            if lowered.startswith(("javascript:", "data:", "blob:", "file:")):
                continue
            key = lowered
            if key in seen:
                continue
            seen.add(key)
            cleaned.append(url)
        if not cleaned:
            raise ValueError("没有可探测的链接")
        return cleaned


class PageHarvestRequest(BaseModel):
    url: str = Field(max_length=8192)
    referer: str = Field(default="", max_length=4096)
    origin: str = Field(default="", max_length=1024)
    user_agent: str = Field(default="", max_length=2048)
    cookie: str = Field(default="", max_length=16 * 1024)
    request_headers: dict[str, str] = Field(default_factory=dict)
    extensions: list[str] = Field(default_factory=list, max_length=64)

    @field_validator("url")
    @classmethod
    def validate_harvest_url(cls, value: str) -> str:
        parsed = urlparse(value)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("页面抓取只支持 HTTP(S) 网页地址")
        return value

    @field_validator("extensions")
    @classmethod
    def validate_extensions(cls, value: list[str]) -> list[str]:
        cleaned: list[str] = []
        for item in value or []:
            ext = str(item or "").strip().lower().lstrip(".")
            if ext and len(ext) <= 8 and ext.isalnum() and ext not in cleaned:
                cleaned.append(ext)
        return cleaned


class UrlRecognitionRequest(BaseModel):
    url: str = Field(max_length=8192)
    referer: str = Field(default="", max_length=4096)
    origin: str = Field(default="", max_length=1024)
    user_agent: str = Field(default="", max_length=2048)
    cookie: str = Field(default="", max_length=16 * 1024)
    request_headers: dict[str, str] = Field(default_factory=dict)

    @field_validator("url")
    @classmethod
    def validate_url(cls, value: str) -> str:
        parsed = urlparse(value)
        if parsed.scheme in {"ftp", "ftps", "sftp"} and parsed.hostname:
            return value
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("url 必须是有效的 HTTP(S)、FTP(S) 或 SFTP 地址")
        return value
