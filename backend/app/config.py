import json
from pathlib import Path
from pydantic_settings import BaseSettings
from .paths import RUNTIME_PATHS

PROJECT_ROOT = RUNTIME_PATHS.project_root
CONFIG_PATH = RUNTIME_PATHS.config_path

class Settings(BaseSettings):
    host: str = "127.0.0.1"
    port: int = 8765
    token: str = "55555"
    download_dir: str = "downloads"
    default_concurrency: int = 4
    default_user_agent: str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0"
    default_referer: str = ""
    default_origin: str = ""
    default_cookie: str = ""
    ffmpeg_path: str = "bin\\ffmpeg.exe"
    allowed_hosts: list[str] = []
    keep_temp_files: bool = False
    max_concurrent_tasks: int = 2

    model_config = {"env_prefix": "HLS_"}

def _resolve_path(v: str, base: Path = PROJECT_ROOT) -> str:
    if not v:
        return v
    p = Path(v)
    if p.is_absolute():
        return str(p)
    return str((base / p).resolve())


def _serialize_path(v: str) -> str:
    if not v:
        return v
    path = Path(v)
    if not path.is_absolute():
        return str(path)
    try:
        return str(path.resolve().relative_to(PROJECT_ROOT.resolve()))
    except ValueError:
        return str(path)

def load_settings() -> Settings:
    if CONFIG_PATH.exists():
        data = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
        s = Settings(**data)
    else:
        s = Settings(download_dir=str(RUNTIME_PATHS.default_download_dir))
        save_settings(s)
    s.download_dir = _resolve_path(s.download_dir, PROJECT_ROOT)
    Path(s.download_dir).mkdir(parents=True, exist_ok=True)
    s.ffmpeg_path = _resolve_path(s.ffmpeg_path)
    return s

def save_settings(s: Settings) -> None:
    CONFIG_PATH.parent.mkdir(parents=True, exist_ok=True)
    data = s.model_dump()
    data["download_dir"] = _serialize_path(data["download_dir"])
    data["ffmpeg_path"] = _serialize_path(data["ffmpeg_path"])
    CONFIG_PATH.write_text(
        json.dumps(data, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )


def apply_settings_update(s: Settings, data: dict) -> None:
    for key, value in data.items():
        if hasattr(s, key):
            setattr(s, key, value)
    s.download_dir = _resolve_path(s.download_dir)
    s.ffmpeg_path = _resolve_path(s.ffmpeg_path, PROJECT_ROOT)
    Path(s.download_dir).mkdir(parents=True, exist_ok=True)

settings = load_settings()
