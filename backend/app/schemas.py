from pydantic import BaseModel, Field, field_validator
from typing import Optional
from urllib.parse import urlparse

from .version import APP_VERSION
from .models import TaskType

class TaskCreate(BaseModel):
    url: str
    task_type: TaskType = TaskType.AUTO
    source_page_url: str = ""
    mime_type: str = ""
    referer: str = ""
    origin: str = ""
    user_agent: str = ""
    cookie: str = ""
    title: str = ""
    filename: str = ""
    download_dir: str = ""
    concurrency: int = Field(default=0, ge=0, le=256)

    @field_validator("url")
    @classmethod
    def validate_url(cls, value: str) -> str:
        parsed = urlparse(value)
        if parsed.scheme == "magnet" and parsed.query:
            return value
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("url 必须是有效的 HTTP(S) 或 magnet 地址")
        return value

class TaskBatchCreate(BaseModel):
    tasks: list[TaskCreate] = Field(min_length=1, max_length=100)

class TaskResponse(BaseModel):
    id: str
    task_type: str = "hls"
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
    error_message: str
    error_code: str = ""
    error_stage: str = ""
    error_url: str = ""
    error_hint: str = ""
    http_status: int = 0
    error_attempt: int = 0
    output_path: str
    output_is_file: bool = False
    created_at: str
    updated_at: str
    started_at: str = ""
    finished_at: str = ""
    available_actions: list[str] = Field(default_factory=list)
    queue_position: int = 0

class SettingsUpdate(BaseModel):
    host: Optional[str] = None
    port: Optional[int] = Field(default=None, ge=1, le=65535)
    token: Optional[str] = Field(default=None, min_length=1, max_length=256)
    download_dir: Optional[str] = None
    default_concurrency: Optional[int] = Field(default=None, ge=1, le=256)
    default_user_agent: Optional[str] = None
    default_referer: Optional[str] = None
    default_origin: Optional[str] = None
    default_cookie: Optional[str] = None
    ffmpeg_path: Optional[str] = None
    allowed_hosts: Optional[list[str]] = None
    keep_temp_files: Optional[bool] = None
    max_concurrent_tasks: Optional[int] = Field(default=None, ge=1, le=16)
    http_chunk_size_mb: Optional[int] = Field(default=None, ge=1, le=64)
    bt_upload_limit_kib: Optional[int] = Field(default=None, ge=0, le=1048576)
    bt_max_connections: Optional[int] = Field(default=None, ge=10, le=1000)
    bt_enable_dht: Optional[bool] = None
    browser_takeover_enabled: Optional[bool] = None
    browser_takeover_min_mb: Optional[int] = Field(default=None, ge=0, le=10240)
    browser_category_dirs: Optional[dict[str, str]] = None


class BrowserHandoffAccept(BaseModel):
    filename: str = Field(default="", max_length=255)
    download_dir: str = Field(default="", max_length=2048)
    category: str = Field(default="other", pattern="^(media|program|archive|other)$")
    remember: bool = True

class HealthResponse(BaseModel):
    status: str = "ok"
    version: str = APP_VERSION


class UserscriptPing(BaseModel):
    version: str = Field(default="", max_length=64)
    page_url: str = Field(default="", max_length=2048)


class PlaybackSeekRequest(BaseModel):
    time: float = Field(ge=0, le=86400)


class TorrentFileSelection(BaseModel):
    indexes: list[int] = Field(min_length=1, max_length=10000)


class UrlRecognitionRequest(BaseModel):
    url: str
    referer: str = ""
    origin: str = ""
    user_agent: str = ""
    cookie: str = ""

    @field_validator("url")
    @classmethod
    def validate_url(cls, value: str) -> str:
        parsed = urlparse(value)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("url 必须是有效的 HTTP(S) 地址")
        return value
