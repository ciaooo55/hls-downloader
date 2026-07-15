from dataclasses import dataclass, field
from enum import Enum
from typing import Optional
import asyncio

class TaskStatus(str, Enum):
    QUEUED = "queued"
    DOWNLOADING = "downloading"
    DOWNLOADING_M3U8 = "downloading_m3u8"
    PARSING = "parsing"
    DOWNLOADING_SEGMENTS = "downloading_segments"
    MERGING = "merging"
    REMUXING = "remuxing"
    DONE = "done"
    FAILED = "failed"
    PAUSING = "pausing"
    PAUSED = "paused"
    CANCELED = "canceled"
    UNSUPPORTED = "unsupported"

@dataclass
class TaskProgress:
    total_segments: int = 0
    completed_segments: int = 0
    failed_segments: int = 0
    downloaded_bytes: int = 0
    total_bytes: int = 0
    speed_bytes_per_sec: float = 0.0
    eta_seconds: float = 0.0
    active_workers: int = 0
    max_workers: int = 0
    reconnect_count: int = 0
    connection_status: str = "idle"
    last_worker_error: str = ""
    post_percent: float = 0.0
    active_slots: int = 0
    active_segment_indexes: list[int] = field(default_factory=list)

@dataclass
class Task:
    id: str
    url: str
    referer: str = ""
    origin: str = ""
    user_agent: str = ""
    cookie: str = ""
    title: str = ""
    filename: str = ""
    concurrency: int = 0  # 0 = use server default from config
    status: TaskStatus = TaskStatus.QUEUED
    progress: TaskProgress = field(default_factory=TaskProgress)
    error_message: str = ""
    error_code: str = ""
    error_stage: str = ""
    error_url: str = ""
    error_hint: str = ""
    http_status: int = 0
    error_attempt: int = 0
    output_path: str = ""
    stage: str = ""
    last_log: str = ""
    created_at: str = ""
    started_at: str = ""
    finished_at: str = ""
    updated_at: str = ""

    cancel_event: Optional[asyncio.Event] = field(default=None, repr=False)
    pause_event: Optional[asyncio.Event] = field(default=None, repr=False)
    task_handle: Optional[asyncio.Task] = field(default=None, repr=False)
