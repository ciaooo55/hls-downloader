import json
import secrets
import hashlib
from pathlib import Path
from pydantic import Field
from pydantic_settings import BaseSettings
from .paths import RUNTIME_PATHS

PROJECT_ROOT = RUNTIME_PATHS.project_root
CONFIG_PATH = RUNTIME_PATHS.config_path


def _new_internal_token() -> str:
    """Create an installation-local credential that is never user managed."""
    return secrets.token_urlsafe(32)


# SHA-256 digests of tokens that reached public commits while config.json was
# tracked by git. Keeping digests prevents re-publishing the credentials while
# still allowing an installation to rotate a compromised legacy value on sight.
_LEAKED_TOKEN_HASHES = frozenset({
    "c507a68f3093e885765257ed3f176c757aaf62bb4cbc2ef94b2e7da3406d9676",
    "b04cb8d73a825328d40038f6e8e9b02fc36303f6452298ffe548aa87f76d3a8d",
    "9671d1a6c492898aa8fae3c034f8144cfe518a1aaed6b535252526e7b6399200",
})


def _is_leaked_token(value: object) -> bool:
    if not isinstance(value, str):
        return False
    return hashlib.sha256(value.encode("utf-8")).hexdigest() in _LEAKED_TOKEN_HASHES


class Settings(BaseSettings):
    config_version: int = 16
    host: str = "127.0.0.1"
    port: int = 8765
    token: str = Field(default_factory=_new_internal_token, min_length=32)
    download_dir: str = "downloads"
    temp_dir: str = "."
    default_concurrency: int = 12
    default_user_agent: str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0"
    default_referer: str = ""
    default_origin: str = ""
    default_cookie: str = ""
    ffmpeg_path: str = "bin\\ffmpeg.exe"
    allowed_hosts: list[str] = []
    keep_temp_files: bool = False
    max_concurrent_tasks: int = 3
    http_chunk_size_mb: int = 8
    download_speed_limit_kib: int = 0
    bt_upload_limit_kib: int = 1024
    bt_max_connections: int = 200
    bt_enable_dht: bool = True
    browser_takeover_enabled: bool = True
    browser_takeover_min_mb: int = 0
    browser_category_dirs: dict[str, str] = Field(default_factory=dict)
    queue_auto_start_enabled: bool = False
    queue_auto_start_time: str = "00:00"
    live_record_max_minutes: int = 0
    download_subtitles: bool = True
    clipboard_watch: bool = True
    tvbox_endpoint: str = ""
    cast_device: dict[str, str] = Field(default_factory=dict)

    # Ignore fields written by a newer release so downgrade/upgrade helpers can
    # still start far enough to close the running application cleanly.
    model_config = {"env_prefix": "HLS_", "extra": "ignore"}

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
        migrated = False
        version = int(data.get("config_version", 1) or 1)
        if version < 2:
            # Request defaults must be site-neutral. A global Referer/Origin
            # breaks other providers and can leak an unrelated page identity.
            data.setdefault("default_referer", "")
            data.setdefault("default_origin", "")
            data["config_version"] = 2
            version = 2
            migrated = True
        if version < 3:
            if int(data.get("default_concurrency", 4) or 4) == 4:
                data["default_concurrency"] = 8
            if int(data.get("max_concurrent_tasks", 2) or 2) == 2:
                data["max_concurrent_tasks"] = 3
            data["config_version"] = 3
            migrated = True
        if version < 4:
            data["config_version"] = 4
            version = 4
            migrated = True
        if version < 5:
            data["browser_category_dirs"] = {}
            data["config_version"] = 5
            version = 5
            migrated = True
        if version < 6:
            if int(data.get("default_concurrency", 8) or 8) == 8:
                data["default_concurrency"] = 12
            data["config_version"] = 6
            version = 6
            migrated = True
        if version < 7:
            data["temp_dir"] = str(RUNTIME_PATHS.default_temp_dir)
            data["config_version"] = 7
            version = 7
            migrated = True
        if version < 8:
            # Old builds applied one site's Referer/Origin to every manual URL,
            # which causes unrelated CDNs to reject otherwise valid downloads.
            if data.get("default_referer") == "https://missav.ai/":
                data["default_referer"] = ""
            if data.get("default_origin") == "https://missav.ai":
                data["default_origin"] = ""
            data["config_version"] = 8
            migrated = True
        if version < 9:
            data["queue_auto_start_enabled"] = False
            data["queue_auto_start_time"] = "00:00"
            data["config_version"] = 9
            version = 9
            migrated = True
        if version < 10:
            data.setdefault("download_speed_limit_kib", 0)
            data["config_version"] = 10
            migrated = True
            version = 10
        if version < 11:
            if not isinstance(data.get("tvbox_endpoint"), str):
                data["tvbox_endpoint"] = ""
            data["config_version"] = 11
            migrated = True
            version = 11
        if version < 12:
            # v1.6.4 exposed a 1 MB takeover default without a settings control.
            # Move only that legacy default to IDM-style capture of all explicit
            # browser downloads; non-default values remain the user's choice.
            if int(data.get("browser_takeover_min_mb", 1) or 0) == 1:
                data["browser_takeover_min_mb"] = 0
            data["config_version"] = 12
            migrated = True
            version = 12
        if version < 13:
            if not isinstance(data.get("cast_device"), dict):
                data["cast_device"] = {}
            data["config_version"] = 13
            migrated = True
            version = 13
        if version < 14:
            # The legacy fixed value was exposed in Settings and shared by all
            # installations. Replace it with an implementation detail used only
            # by the desktop shell and Native Messaging host.
            if not isinstance(data.get("token"), str) or len(data.get("token", "")) < 32 or _is_leaked_token(data.get("token")):
                data["token"] = _new_internal_token()
            # Browser integration is Native Messaging only. The privileged
            # control API is an internal desktop transport, never a LAN API.
            data["host"] = "127.0.0.1"
            data["config_version"] = 14
            migrated = True
            version = 14
        if version < 15:
            # config.json used to be tracked by git, so any token that ever
            # reached a public commit must be rotated on sight — it is a
            # shared credential from that moment on.
            if _is_leaked_token(data.get("token")):
                data["token"] = _new_internal_token()
            data["config_version"] = 15
            migrated = True
            version = 15
        if version < 16:
            # Rotate a second historic public token even for installations
            # already migrated to v15.  The deny-list contains digests only.
            if _is_leaked_token(data.get("token")):
                data["token"] = _new_internal_token()
            data["config_version"] = 16
            migrated = True
            version = 16
        if not isinstance(data.get("tvbox_endpoint"), str):
            data["tvbox_endpoint"] = ""
            migrated = True
        s = Settings(**data)
        # Keep a manually edited/legacy TVBox address from poisoning startup;
        # invalid values are cleared and can be selected again in Settings.
        if s.tvbox_endpoint:
            try:
                from .tvbox import normalize_tvbox_endpoint
                canonical_endpoint = normalize_tvbox_endpoint(s.tvbox_endpoint)
            except ValueError:
                canonical_endpoint = ""
            if canonical_endpoint != s.tvbox_endpoint:
                s.tvbox_endpoint = canonical_endpoint
                migrated = True
        if not isinstance(s.cast_device, dict):
            s.cast_device = {}
            migrated = True
        elif s.cast_device:
            try:
                from .dlna import normalize_cast_device
                canonical_cast_device = normalize_cast_device(s.cast_device)
            except ValueError:
                canonical_cast_device = {}
            if canonical_cast_device != s.cast_device:
                s.cast_device = canonical_cast_device
                migrated = True
        if migrated:
            save_settings(s)
    else:
        s = Settings(
            download_dir=str(RUNTIME_PATHS.default_download_dir),
            temp_dir=str(RUNTIME_PATHS.default_temp_dir),
        )
        save_settings(s)
    s.download_dir = _resolve_path(s.download_dir, PROJECT_ROOT)
    s.temp_dir = _resolve_path(s.temp_dir, PROJECT_ROOT)
    Path(s.download_dir).mkdir(parents=True, exist_ok=True)
    Path(s.temp_dir).mkdir(parents=True, exist_ok=True)
    s.ffmpeg_path = _resolve_path(s.ffmpeg_path)
    return s

def save_settings(s: Settings) -> None:
    CONFIG_PATH.parent.mkdir(parents=True, exist_ok=True)
    data = s.model_dump()
    data["download_dir"] = _serialize_path(data["download_dir"])
    data["temp_dir"] = _serialize_path(data["temp_dir"])
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
    s.temp_dir = _resolve_path(s.temp_dir)
    s.ffmpeg_path = _resolve_path(s.ffmpeg_path, PROJECT_ROOT)
    Path(s.download_dir).mkdir(parents=True, exist_ok=True)
    Path(s.temp_dir).mkdir(parents=True, exist_ok=True)

settings = load_settings()
