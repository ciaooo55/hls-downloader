import asyncio
import contextlib
import json
import os
import re
import secrets
import shutil
import subprocess
import threading
import time
import uuid
from pathlib import Path
from urllib.parse import quote, urlsplit

import httpx
from fastapi import APIRouter, HTTPException, Header, Request, UploadFile, File, Form
from fastapi.responses import FileResponse, PlainTextResponse, StreamingResponse, Response
from pydantic import BaseModel, ValidationError
from .schemas import (
    BrowserHandoffCreate,
    BrowserMediaPushComplete,
    BrowserMediaPushCreate,
    BrowserPing,
    BrowserTakeoverSettings,
    HealthResponse,
    LegalAcceptanceRequest,
    SettingsUpdate,
    TaskBatchCreate,
    TaskCreate,
    TaskResponse,
    TaskRequestUpdate,
    TaskSpeedLimit,
    UrlRecognitionRequest,
    PageHarvestRequest,
    PageHarvestProbeRequest,
    PlaybackSeekRequest,
    TorrentFileSelection,
    TorrentPathImport,
    LinkPathImport,
    FileSystemAction,
    BrowserHandoffAccept,
    BrowserHandoffCancel,
    CastLocalPush,
    CastTaskPush,
    CastUrlPush,
    CastControl,
    TvboxLocalPush,
    TvboxTaskPush,
    TvboxPush,
)
from .config import PROJECT_ROOT, apply_settings_update, settings, save_settings
from .download_category import resolve_category_output_dir
from .credentials import SECRET_MASK, mask_site_profiles, restore_masked_site_profiles
from .downloader.task_manager import (
    TaskConflictError,
    TaskNotFoundError,
    manager,
    parse_queue_direction,
    task_output_is_file,
    task_output_missing,
)
from .downloader.engine import task_work_dir
from .downloader.playback import (
    PlaybackAuthorizationError,
    PlaybackError,
    PlaybackNotReadyError,
    PlaybackSessionError,
    playback_service,
)
from .desktop_runtime import (
    activate_window,
    has_browser_handoff_presenter,
    has_pending_native_handoffs,
    is_desktop_handoff_session,
    native_shell_expected,
    present_browser_handoff,
    request_shutdown,
)
from .desktop_runtime import register_activation, register_browser_handoff, register_shutdown, set_desktop_handoff_session
from .native_desktop import native_desktop_session, request_core_shutdown
from .native_shell import (
    boot_native_shell,
    is_native_shell_ready,
    maybe_spawn_desktop_ui_process,
    native_shell_status,
    native_shell_supervisor,
    shutdown_native_shell,
    start_native_shell_ipc,
    stop_native_shell_ipc,
)
from .url_recognition import RecognitionError, recognize_url
from .page_harvest import HarvestError, harvest_page, probe_harvest_links
from .updater import (
    UpdateCheckError,
    UpdateError,
    extract_portable_update,
    queue_update_download,
    update_service,
    validate_portable_archive,
)
from .paths import RUNTIME_PATHS
from .version import APP_VERSION
from .legal import (
    TERMS_VERSION,
    legal_acceptance_current,
    legal_status,
    record_legal_acceptance,
    terms_payload,
)
from .power_actions import power_action_service
from .models import TaskStatus, TaskType
from .browser_handoff import browser_handoffs
from .access_tokens import (
    issue_browser_access_token,
    issue_desktop_access_token,
    issue_file_access_token,
    verify_browser_access_token,
    verify_desktop_access_token,
    verify_file_access_token,
)
from .downloader.throttle import download_throttle, effective_download_speed_limit_kib
from .speed_history import speed_history_payload, speed_peak_payload
from .connection_parts import connection_parts_payload
from .duplicate_task import duplicate_task_entry
from .site_profiles import normalize_site_proxy, site_profile_from_task, upsert_site_profile
from .network_proxy import ensure_url_allowed, policy_httpx_client
from .request_context import request_origin, sanitize_request_contexts, sanitize_request_headers
from .tvbox import local_media_server, push_tvbox, scan_tvboxes
from .dlna import cast_control, cast_media, normalize_cast_device, scan_cast_devices

router = APIRouter(prefix="/api")
_browser_media_pushes: dict[str, dict] = {}
_browser_media_push_lock = threading.Lock()
MAX_BROWSER_MEDIA_PUSHES = 64
_update_launch_tasks: set[str] = set()
MAX_BROWSER_JSON_BODY_BYTES = 256 * 1024
MAX_TORRENT_MULTIPART_BODY_BYTES = 17 * 1024 * 1024
_HANDOFF_WAIT_TERMINAL = frozenset({"accepted", "rejected", "canceled", "expired", "failed"})

def _check_token(x_token: str = Header(default="")):
    if not (
        secrets.compare_digest(x_token, settings.token)
        or verify_desktop_access_token(x_token)
    ):
        raise HTTPException(status_code=401, detail="Invalid token")


def _check_control_token(x_token: str) -> None:
    if not secrets.compare_digest(x_token, settings.token):
        raise HTTPException(status_code=401, detail="Invalid control token")


def _check_browser_token(x_token: str) -> None:
    if verify_browser_access_token(x_token):
        return
    _check_token(x_token)


def _check_playback_token(x_token: str = "", token: str = ""):
    """Allow native HLS clients to carry the local token in the media URL."""
    _check_token(x_token or token)


async def _read_browser_json(request: Request, model_type: type[BaseModel]) -> BaseModel:
    """Parse a bounded browser message without buffering an unbounded body."""
    content_encoding = request.headers.get("content-encoding", "").strip().lower()
    if content_encoding and content_encoding != "identity":
        raise HTTPException(status_code=415, detail="浏览器请求不支持压缩请求体")
    content_length = request.headers.get("content-length", "").strip()
    if content_length:
        try:
            if int(content_length) < 0:
                raise ValueError
            if int(content_length) > MAX_BROWSER_JSON_BODY_BYTES:
                raise HTTPException(status_code=413, detail="浏览器请求体过大")
        except ValueError as exc:
            raise HTTPException(status_code=400, detail="Content-Length 无效") from exc
    body = bytearray()
    async for chunk in request.stream():
        body.extend(chunk)
        if len(body) > MAX_BROWSER_JSON_BODY_BYTES:
            raise HTTPException(status_code=413, detail="浏览器请求体过大")
    try:
        return model_type.model_validate_json(bytes(body))
    except ValidationError as exc:
        raise HTTPException(
            status_code=422,
            detail=exc.errors(include_url=False, include_input=False),
        ) from exc


def _browser_payload(value: BrowserHandoffCreate) -> dict:
    payload = value.model_dump()
    # Apply the same canonical bounds before the handoff service keeps the
    # request in memory. TaskManager repeats this at its persistence boundary.
    payload["request_headers"] = sanitize_request_headers(payload.get("request_headers"))
    payload["request_contexts"] = sanitize_request_contexts(payload.get("request_contexts"))
    return payload


def _public_settings() -> dict:
    """Return user-configurable settings without the internal IPC credential."""
    from .config import settings as current
    body = current.model_dump(exclude={
        "token", "host", "port",
        "legal_terms_accepted_version",
        "legal_terms_accepted_digest",
        "legal_terms_accepted_at",
    })
    body["default_cookie_configured"] = bool(current.default_cookie)
    # A default Cookie is an authentication credential, not a display value.
    # Manual task dialogs therefore receive an empty value and must not replay
    # a mask as though it were a real Cookie.
    body["default_cookie"] = ""
    body["proxy_url_configured"] = bool(current.proxy_url)
    body["proxy_url"] = SECRET_MASK if current.proxy_url else ""
    body["site_profiles"] = mask_site_profiles(current.site_profiles)
    body["effective_download_speed_limit_kib"] = effective_download_speed_limit_kib()
    return body


def _require_legal_acceptance() -> None:
    if legal_acceptance_current():
        return
    raise HTTPException(
        status_code=428,
        detail={
            "code": "LEGAL_TERMS_REQUIRED",
            "message": "首次使用前必须在桌面主窗口阅读并同意用户协议与免责声明",
            "required_version": TERMS_VERSION,
        },
    )


@router.get("/legal/status")
async def get_legal_status(x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        return legal_status()
    except RuntimeError as exc:
        raise HTTPException(status_code=503, detail=str(exc)) from exc


@router.get("/legal/terms")
async def get_legal_terms(x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        return terms_payload()
    except RuntimeError as exc:
        raise HTTPException(status_code=503, detail=str(exc)) from exc


@router.post("/legal/accept")
async def accept_legal_terms(body: LegalAcceptanceRequest, x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        return record_legal_acceptance(
            version=body.version,
            digest=body.document_digest,
            accepted=body.accepted,
        )
    except (RuntimeError, ValueError) as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc

def _check_host(url: str):
    if url.lower().startswith("magnet:") or url.lower().startswith("torrent-file:"):
        return
    try:
        ensure_url_allowed(url)
    except ValueError as exc:
        raise HTTPException(status_code=403, detail=str(exc)) from exc


async def _manager_action(awaitable):
    try:
        await awaitable
    except TaskNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except TaskConflictError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc

@router.get("/health", response_model=HealthResponse)
async def health(x_token: str = Header(default="")):
    if x_token:
        if verify_browser_access_token(x_token):
            pass
        else:
            _check_token(x_token)
    return HealthResponse(authenticated=bool(x_token))


@router.post("/desktop/credential")
async def create_desktop_credential(x_token: str = Header(default="")):
    _check_control_token(x_token)
    return {"credential": issue_desktop_access_token()}


def _is_local_ui_origin(value: str) -> bool:
    """Allow credential bootstrap only to the UI served by this Core.

    The standalone web UI has no Tauri command channel from which it could
    obtain the installation credential.  Requiring an exact loopback Origin
    keeps the bootstrap useful for ``/ui`` and the local Vite dev server while
    preventing an arbitrary website from reading a newly issued API token.
    """
    try:
        parsed = urlsplit(str(value or "").strip())
        if parsed.scheme not in {"http", "https"}:
            return False
        host = (parsed.hostname or "").lower().rstrip(".")
        if host not in {"127.0.0.1", "localhost"}:
            return False
        port = parsed.port or (443 if parsed.scheme == "https" else 80)
        return port in {int(settings.port), 1420}
    except (TypeError, ValueError):
        return False


@router.post("/ui/credential")
async def create_ui_credential(request: Request):
    """Bootstrap the standalone local UI without exposing the master token."""
    origin = request.headers.get("origin", "")
    if not _is_local_ui_origin(origin):
        raise HTTPException(status_code=403, detail="仅允许从本机下载器界面获取凭据")
    return {
        "credential": issue_desktop_access_token(),
        "port": int(settings.port),
    }


@router.post("/browser/credential")
async def create_browser_credential(x_token: str = Header(default="")):
    _check_control_token(x_token)
    return {"credential": issue_browser_access_token()}



def _normalize_resource_url(url: str) -> str:
    value = str(url or '').strip()
    if not value:
        return ''
    try:
        from urllib.parse import urlsplit, urlunsplit
        parts = urlsplit(value)
        path = parts.path.rstrip('/') or '/'
        return urlunsplit((parts.scheme.lower(), parts.netloc.lower(), path, parts.query, ''))
    except Exception:
        return value.rstrip('/')


def _duplicate_task_payload(url: str) -> list[dict]:
    matches = manager.find_tasks_by_url(url, limit=5)
    return [
        duplicate_task_entry(
            task,
            manager.get_available_actions(task),
            output_missing=task_output_missing(task),
        )
        for task in matches
    ]


def _require_allow_duplicate(url: str, allow_duplicate: bool) -> None:
    """IDM-style guard: refuse same-URL create unless the client opts in."""
    if allow_duplicate:
        return
    duplicates = _duplicate_task_payload(url)
    if not duplicates:
        return
    top = duplicates[0]
    name = top.get('filename') or '已有任务'
    status = top.get('status') or ''
    raise HTTPException(
        status_code=409,
        detail={
            'code': 'DUPLICATE_URL',
            'message': f'下载列表中已有相同链接（{name} · {status}）。若仍要下载，请确认后重试。',
            'duplicates': duplicates,
        },
    )


def _handoff_public(item) -> dict:
    body = item.public()
    duplicates = _duplicate_task_payload(item.url)
    body['duplicate'] = bool(duplicates)
    body['duplicates'] = duplicates
    if duplicates:
        top = duplicates[0]
        status = top.get('status') or ''
        name = top.get('filename') or '已有任务'
        body['duplicate_message'] = (
            f'下载列表中已有相同链接（{name} · {status}）。仍可继续下载，也可取消。'
        )
    else:
        body['duplicate_message'] = ''
    return body


def _handoff_detail(item) -> dict:
    body = item.detail()
    duplicates = _duplicate_task_payload(item.url)
    body["duplicate"] = bool(duplicates)
    body["duplicates"] = duplicates
    if duplicates:
        top = duplicates[0]
        body["duplicate_message"] = (
            f'下载列表中已有相同链接（{top.get("filename") or "已有任务"} · {top.get("status") or ""}）。仍可继续下载，也可取消。'
        )
    else:
        body["duplicate_message"] = ""
    # Chromium keeps its original DownloadItem paused until the desktop task
    # has proved that it can replay the request. Expose only non-sensitive
    # progress metadata so the extension can safely fall back to the browser
    # for one-use signed URLs instead of discarding the only working transfer.
    task = manager.tasks.get(str(item.task_id or "")) if item.task_id else None
    if task is not None:
        body["task_status"] = task.status.value
        body["task_stage"] = task.stage
        body["task_downloaded_bytes"] = max(0, int(task.progress.downloaded_bytes or 0))
        body["task_total_bytes"] = max(0, int(task.progress.total_bytes or 0))
        body["task_error_code"] = str(task.error_code or "")
    return body

@router.post("/browser/handoffs")
async def create_browser_handoff(request: Request, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    _require_legal_acceptance()
    model = await _read_browser_json(request, BrowserHandoffCreate)
    payload = _browser_payload(model)
    url = payload["url"]
    _check_host(url)
    item = browser_handoffs.create(payload)
    presentation = present_browser_handoff(item.id, snapshot=item.public())
    mode = str(presentation.get("mode") or "none")
    if mode == "native-shell":
        # Pre-created confirmation window already has the offer snapshot.
        browser_handoffs.mark_presentation(item.id, "presented")
    elif mode in {"desktop-pending", "native-shell-pending"}:
        browser_handoffs.mark_presentation(item.id, "queued")
    elif mode == "desktop":
        # Presenter thread will upgrade this to presented; do not overwrite later.
        browser_handoffs.mark_presentation(item.id, "queued")
    elif mode == "ui-fallback":
        # Manager UI / browser tab will show the offer; treat that as presented.
        browser_handoffs.mark_presentation(item.id, "presented")
    else:
        browser_handoffs.mark_presentation(item.id, "failed", "no presenter")
    item = browser_handoffs.get(item.id) or item
    body = _handoff_public(item)
    body["presentation_mode"] = mode
    body["presentation_ok"] = bool(presentation.get("ok"))
    body["presentation_queued"] = bool(presentation.get("queued"))
    body["presentable"] = bool(presentation.get("presentable"))
    if presentation.get("snapshot"):
        body["snapshot"] = presentation["snapshot"]
    return body


@router.post("/browser/ping")
async def browser_extension_ping(request: Request, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    model = await _read_browser_json(request, BrowserPing)
    browser_handoffs.record_ping(
        model.version,
        model.client_id,
        model.browser,
    )
    # Return update metadata on the same heartbeat. Store-installed extensions
    # update through the browser; unpacked/self-hosted builds can at least show
    # an accurate release link instead of silently staying on an old protocol.
    return {
        "ok": True,
        "core_version": APP_VERSION,
        "takeover_enabled": bool(settings.browser_takeover_enabled),
        "takeover_minimum_bytes": max(0, int(settings.browser_takeover_min_mb or 0)) * 1024 * 1024,
        **browser_handoffs.status(),
    }


@router.post("/browser/takeover-settings")
async def update_browser_takeover_settings(
    body: BrowserTakeoverSettings,
    x_token: str = Header(default=""),
):
    _check_browser_token(x_token)
    if body.enabled is not None:
        settings.browser_takeover_enabled = body.enabled
    if body.minimum_bytes is not None:
        settings.browser_takeover_min_mb = int(body.minimum_bytes) // (1024 * 1024)
    save_settings(settings)
    return {
        "ok": True,
        "takeover_enabled": bool(settings.browser_takeover_enabled),
        "takeover_minimum_bytes": max(0, int(settings.browser_takeover_min_mb or 0)) * 1024 * 1024,
    }


@router.post("/browser/activate")
async def activate_browser_desktop(x_token: str = Header(default="")):
    _check_browser_token(x_token)
    return {"ok": activate_window()}


@router.get("/browser/presenter")
async def browser_presenter_status(x_token: str = Header(default="")):
    """Desktop shell readiness for cold-start handoffs."""
    _check_browser_token(x_token)
    shell_ready = is_native_shell_ready()
    expected = native_shell_expected() or has_pending_native_handoffs()
    ready = has_browser_handoff_presenter() or shell_ready
    session = is_desktop_handoff_session() or shell_ready or expected
    if shell_ready:
        mode = "native-shell"
    elif expected:
        mode = "native-shell-pending"
    elif has_browser_handoff_presenter():
        mode = "desktop"
    elif is_desktop_handoff_session():
        mode = "desktop-pending"
    else:
        mode = "ui-fallback"
    return {
        "ok": True,
        "ready": ready,
        "session": session,
        "mode": mode,
    }


@router.get("/browser/status")
async def browser_extension_status(x_token: str = Header(default="")):
    _check_browser_token(x_token)
    return browser_handoffs.status()


@router.get("/browser/handoffs")
async def list_browser_handoffs(x_token: str = Header(default="")):
    _check_browser_token(x_token)
    return [
        _handoff_public(browser_handoffs.get(item["id"]))
        for item in browser_handoffs.pending()
        if browser_handoffs.get(item["id"]) is not None
    ]


@router.get("/browser/handoffs/{handoff_id}")
async def get_browser_handoff(handoff_id: str, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    item = browser_handoffs.get(handoff_id)
    if not item:
        raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
    return _handoff_detail(item)


@router.post("/browser/handoffs/{handoff_id}/accept")
async def accept_browser_handoff(handoff_id: str, body: BrowserHandoffAccept | None = None, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    _require_legal_acceptance()
    body = body or BrowserHandoffAccept()
    item = browser_handoffs.claim(handoff_id)
    if not item:
        existing = browser_handoffs.get(handoff_id)
        if not existing:
            raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
        raise HTTPException(status_code=409, detail=f"接管请求当前状态为 {existing.status}")
    resolved = resolve_category_output_dir(
        filename=item.filename,
        url=item.url,
        mime_type=item.mime_type,
        category=body.category,
        explicit_dir=body.download_dir,
    )
    output_dir = Path(resolved or settings.download_dir).expanduser().resolve()
    try:
        output_dir.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        browser_handoffs.fail_accept(handoff_id)
        raise HTTPException(status_code=400, detail=f"无法使用保存目录: {exc}") from exc
    if not output_dir.is_dir():
        browser_handoffs.fail_accept(handoff_id)
        raise HTTPException(status_code=400, detail="保存位置不是文件夹")
    if body.filename.strip():
        item.filename = body.filename.strip()
    # Keep the browser-captured source-page context by default.  Only an
    # explicit value from the expanded handoff form overrides it; this avoids
    # falling back to global defaults and is essential for authenticated sites.
    if body.cookie.strip():
        item.cookie = body.cookie.strip()
    if body.request_headers:
        item.request_headers = sanitize_request_headers(body.request_headers)
        manual_headers = item.request_headers
    else:
        manual_headers = {}
    if body.cookie.strip() or body.request_headers:
        # Manual values are intentionally scoped to the actual download
        # origin. build_task_headers otherwise strips top-level cookies and
        # Authorization across origins to prevent credential leakage, which
        # would make an explicit 403 workaround appear to do nothing.
        origin = request_origin(item.url)
        if origin:
            context = dict(item.request_contexts.get(origin) or {})
            if body.cookie.strip():
                context["cookie"] = body.cookie.strip()
            if body.request_headers:
                context["request_headers"] = item.request_headers
                for header_name, context_key in (
                    ("referer", "referer"),
                    ("origin", "origin"),
                    ("user-agent", "user_agent"),
                ):
                    if manual_headers.get(header_name):
                        context[context_key] = manual_headers[header_name]
            context.setdefault("referer", item.referer)
            context.setdefault("origin", item.origin)
            context.setdefault("user_agent", item.user_agent)
            item.request_contexts[origin] = context
    try:
        task = await _create_browser_task(item, output_dir=str(output_dir))
    except Exception:
        browser_handoffs.fail_accept(handoff_id)
        raise
    if body.remember:
        settings.browser_category_dirs[body.category] = str(output_dir)
        save_settings(settings)
    accepted = browser_handoffs.complete_accept(handoff_id, task.id)
    return (accepted or item).public()


@router.get("/browser/handoffs/{handoff_id}/wait")
async def wait_browser_handoff(handoff_id: str, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    deadline = asyncio.get_running_loop().time() + browser_handoffs.ttl + 2
    while True:
        item = browser_handoffs.get(handoff_id)
        if not item:
            raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
        if item.status in _HANDOFF_WAIT_TERMINAL or asyncio.get_running_loop().time() >= deadline:
            return item.public()
        await asyncio.sleep(0.25)


def _browser_offer_task_type(item) -> TaskType:
    """Trust the overlay's HLS/DASH kind; URL-only AUTO misses .m3u / mpegurl variants."""
    kind = str(getattr(item, "resource_kind", "") or "").lower()
    if kind == "hls":
        return TaskType.HLS
    if kind == "dash":
        return TaskType.DASH
    if kind == "magnet":
        return TaskType.TORRENT
    return TaskType.AUTO


async def _create_browser_task(item, output_dir: str = ""):
    expired = manager.find_expired_request_task(item.url, item.source_page_url)
    if expired is not None:
        return await manager.refresh_task_request(
            expired.id,
            url=item.url,
            source_page_url=item.source_page_url,
            mime_type=item.mime_type,
            referer=item.referer,
            origin=item.origin,
            user_agent=item.user_agent,
            cookie=item.cookie,
            request_headers=item.request_headers,
            request_contexts=item.request_contexts,
            request_method=item.request_method,
            request_body=item.request_body,
            auto_resume=True,
            browser_originated=True,
        )
    task = await manager.create_task(
        url=item.url,
        task_type=_browser_offer_task_type(item),
        source_page_url=item.source_page_url,
        mime_type=item.mime_type,
        referer=item.referer,
        origin=item.origin,
        user_agent=item.user_agent,
        cookie=item.cookie,
        request_headers=item.request_headers,
        request_contexts=item.request_contexts,
        request_method=item.request_method,
        request_body=item.request_body,
        title=item.title,
        filename=item.filename,
        output_dir=output_dir,
        auto_start=True,
        inherit_default_headers=False,
        browser_originated=True,
    )
    return task


@router.post("/browser/downloads")
async def create_browser_download(request: Request, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    _require_legal_acceptance()
    model = await _read_browser_json(request, BrowserHandoffCreate)
    payload = _browser_payload(model)
    url = payload["url"]
    _check_host(url)
    item = browser_handoffs.create(payload)
    claimed = browser_handoffs.claim(item.id)
    if claimed is None:
        raise HTTPException(status_code=409, detail="接管请求无法确认")
    try:
        task = await _create_browser_task(claimed)
    except Exception:
        browser_handoffs.fail_accept(claimed.id)
        raise
    browser_handoffs.complete_accept(claimed.id, task.id)
    activate_window()
    return _to_resp(task)


@router.post("/browser/handoffs/{handoff_id}/reject")
async def reject_browser_handoff(handoff_id: str, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    item = browser_handoffs.reject(handoff_id)
    if not item:
        raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
    return item.public()


@router.post("/browser/handoffs/{handoff_id}/cancel")
async def cancel_browser_handoff(
    handoff_id: str,
    body: BrowserHandoffCancel | None = None,
    x_token: str = Header(default=""),
):
    _check_browser_token(x_token)
    item = browser_handoffs.cancel(
        handoff_id,
        suppress_site_kind=bool(body and body.suppress_site_kind),
    )
    if not item:
        raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
    return item.public()


@router.post("/app/activate")
async def activate_desktop_app(x_token: str = Header(default="")):
    _check_token(x_token)
    return {"ok": activate_window()}


@router.post("/app/shutdown")
async def shutdown_desktop_app(
    resume_tasks: bool = False,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    marked = await manager.prepare_for_update_restart() if resume_tasks else 0
    return {"ok": request_shutdown(), "resume_tasks": marked}


@router.post("/desktop/session/start")
async def start_native_desktop_session(x_token: str = Header(default="")):
    _check_token(x_token)
    status = native_desktop_session.start()
    set_desktop_handoff_session(True)
    register_activation(native_desktop_session.activate)
    register_shutdown(native_desktop_session.shutdown)
    register_browser_handoff(native_desktop_session.handoff)
    return status


@router.post("/desktop/session/stop")
async def stop_native_desktop_session(x_token: str = Header(default="")):
    _check_token(x_token)
    register_activation(None)
    register_browser_handoff(None)
    register_shutdown(None)
    set_desktop_handoff_session(False)
    return native_desktop_session.stop()


@router.get("/desktop/session/commands")
async def poll_native_desktop_commands(
    after: int = 0,
    timeout: float = 20.0,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    return await asyncio.to_thread(native_desktop_session.poll, after, timeout)


@router.post("/desktop/native-shell/boot")
async def boot_resident_native_shell(x_token: str = Header(default="")):
    """Mark the supervisor resident: tray + warm confirm/progress/complete windows."""
    _check_token(x_token)
    return boot_native_shell()


@router.post("/desktop/native-shell/shutdown")
async def shutdown_resident_native_shell(x_token: str = Header(default="")):
    _check_token(x_token)
    return shutdown_native_shell()


@router.get("/desktop/native-shell/status")
async def get_native_shell_status(x_token: str = Header(default="")):
    _check_token(x_token)
    return native_shell_status()


@router.get("/desktop/native-shell/events")
async def poll_native_shell_events(
    after: int = 0,
    timeout: float = 1.0,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    return await asyncio.to_thread(native_shell_supervisor().wait_event, after, timeout)


@router.post("/desktop/native-shell/main/open")
async def open_native_shell_main(x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        return native_shell_supervisor().open_main()
    except RuntimeError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc


@router.post("/desktop/native-shell/main/hide")
async def hide_native_shell_main(x_token: str = Header(default="")):
    _check_token(x_token)
    return native_shell_supervisor().hide_main()


@router.post("/desktop/native-shell/settings")
async def open_native_shell_settings(x_token: str = Header(default="")):
    """Show settings / new-task / device picker. Idle native-shell has no WebView."""
    _check_token(x_token)
    spawned = maybe_spawn_desktop_ui_process(project_root=PROJECT_ROOT)
    activated = activate_window()
    return {"ok": bool(activated or spawned)}


@router.post("/desktop/native-shell/ipc/start")
async def start_resident_native_shell_ipc(x_token: str = Header(default="")):
    _check_token(x_token)
    return start_native_shell_ipc()


@router.post("/desktop/native-shell/ipc/stop")
async def stop_resident_native_shell_ipc(x_token: str = Header(default="")):
    _check_token(x_token)
    stop_native_shell_ipc()
    return {"ok": True}


@router.post("/desktop/native-shell/progress")
async def present_native_shell_progress(request: Request, x_token: str = Header(default="")):
    _check_token(x_token)
    body = await request.json()
    tasks = body.get("tasks") if isinstance(body, dict) else None
    if not isinstance(tasks, list):
        raise HTTPException(status_code=400, detail="progress tasks missing")
    try:
        return native_shell_supervisor().progress(tasks)
    except RuntimeError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc


@router.post("/desktop/native-shell/complete")
async def present_native_shell_complete(request: Request, x_token: str = Header(default="")):
    _check_token(x_token)
    body = await request.json()
    item = body.get("item") if isinstance(body, dict) else None
    if not isinstance(item, dict):
        raise HTTPException(status_code=400, detail="complete item missing")
    try:
        return native_shell_supervisor().complete(item)
    except RuntimeError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc


@router.post("/desktop/handoffs/{handoff_id}/presented")
async def mark_native_handoff_presented(handoff_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    item = browser_handoffs.mark_presentation(handoff_id, "presented")
    if item is None:
        raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
    return item.public()


@router.post("/desktop/core/shutdown")
async def shutdown_native_core(x_token: str = Header(default="")):
    _check_control_token(x_token)
    # The native desktop supervisor can stop/restart Core while a live HLS
    # task is mid-poll. Persist a resumable marker first; otherwise the normal
    # asyncio cancellation is indistinguishable from a crash and the next
    # start only reports ``core_interrupted``. HLS then preserves its segment
    # checkpoint instead of entering the intentional pause-and-merge path.
    marked = await manager.prepare_for_update_restart()
    return {"ok": request_core_shutdown(), "resume_tasks": marked}


@router.get("/update/check")
async def check_update(force: bool = False, x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        info = await asyncio.to_thread(update_service.check, force=force)
    except UpdateCheckError as exc:
        raise HTTPException(status_code=503, detail=exc.to_dict()) from exc
    except UpdateError as exc:
        raise HTTPException(status_code=503, detail=str(exc)) from exc
    return info.to_dict()


@router.post("/update/install")
async def install_update(x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        info = await asyncio.to_thread(update_service.prepare_managed_download)
        task = await queue_update_download(info, manager)
    except UpdateError as exc:
        status = 409 if any(
            marker in str(exc) for marker in ("重复", "正在下载", "已经启动")
        ) else 400
        raise HTTPException(status_code=status, detail=str(exc)) from exc
    if task.status is TaskStatus.DONE and task.output_path:
        await _launch_managed_update(task.id)
    elif task.id not in _update_launch_tasks:
        _update_launch_tasks.add(task.id)
        asyncio.create_task(_wait_and_launch_managed_update(task.id))
    return {"ok": True, "version": info.latest_version, "task_id": task.id}


@router.post("/power-actions/{action_id}/cancel")
async def cancel_power_action(action_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    if not power_action_service.cancel(action_id):
        raise HTTPException(status_code=404, detail="电源动作不存在或已结束")
    manager.publish_event({
        "type": "power_action_canceled",
        "power_action_id": action_id,
    })
    return {"ok": True}


@router.get("/power-actions")
async def list_power_actions(x_token: str = Header(default="")):
    _check_token(x_token)
    return power_action_service.all_pending()


@router.post("/power-actions/{action_id}/confirm")
async def confirm_power_action(action_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        confirmed = await asyncio.to_thread(power_action_service.confirm, action_id)
    except (OSError, ValueError) as exc:
        raise HTTPException(status_code=500, detail=f"无法执行电源动作：{exc}") from exc
    if not confirmed:
        raise HTTPException(status_code=404, detail="电源动作不存在或已结束")
    return {"ok": True}


async def _wait_and_launch_managed_update(task_id: str) -> None:
    try:
        task = manager.tasks.get(task_id)
        if task and task.task_handle:
            with contextlib.suppress(asyncio.CancelledError):
                await task.task_handle
        if task and task.status is TaskStatus.DONE and task.output_path:
            await _launch_managed_update(task_id)
    finally:
        _update_launch_tasks.discard(task_id)


async def _launch_managed_update(task_id: str) -> None:
    task = manager.tasks.get(task_id)
    if not task or task.status is not TaskStatus.DONE or not task.output_path:
        return
    package = Path(task.output_path)
    asset_kind = str(task.engine_state.get("update_asset_kind") or "installer")

    def validate_package() -> None:
        expected_size = int(task.engine_state.get("update_expected_size") or 0)
        if not package.is_file() or package.stat().st_size <= 0:
            raise OSError("更新包文件不存在")
        if expected_size and package.stat().st_size != expected_size:
            raise OSError(
                f"更新包大小不匹配：期望 {expected_size}，实际 {package.stat().st_size}"
            )
        expected = str(task.expected_checksum or "").strip().lower()
        expected_digest = expected.removeprefix("sha256:") if expected.startswith("sha256:") else ""
        if len(expected_digest) != 64 or any(
            character not in "0123456789abcdef" for character in expected_digest
        ):
            raise OSError("更新包缺少可信的 SHA-256 校验值")
        import hashlib

        digest = hashlib.sha256()
        with package.open("rb") as source:
            while chunk := source.read(1024 * 1024):
                digest.update(chunk)
        if digest.hexdigest() != expected_digest:
            raise OSError("更新包 SHA-256 校验失败")
        if asset_kind == "portable":
            validate_portable_archive(package)
        else:
            with package.open("rb") as handle:
                if handle.read(2) != b"MZ":
                    raise OSError("不是有效的 Windows 安装程序")

    portable_stage: Path | None = None
    try:
        await asyncio.to_thread(validate_package)
        if asset_kind == "portable":
            if RUNTIME_PATHS.mode != "portable":
                raise OSError("当前不是便携版，拒绝应用便携更新包")
            portable_stage = await asyncio.to_thread(
                extract_portable_update,
                package,
                str(task.engine_state.get("update_version") or "update"),
            )
        # Finish and persist active download state before the installer begins
        # trying to close/replace this process. Launching NSIS first created a
        # real lock race: its shutdown helper waited on a Core that was still
        # doing update preparation, making a healthy upgrade appear hung.
        await manager.prepare_for_update_restart()
        if asset_kind == "portable":
            if portable_stage is None:
                raise OSError("便携更新包尚未完成解压")
            upgrade_script = portable_stage / "scripts" / "upgrade-portable.ps1"
            subprocess.Popen([
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(upgrade_script),
                "-TargetDir",
                str(RUNTIME_PATHS.project_root),
                "-StartAfterUpgrade",
                "-DeleteSourceAfterUpgrade",
            ])
        else:
            subprocess.Popen([str(package), "/DELETESELF=1"])
    except (OSError, UpdateError) as exc:
        package.unlink(missing_ok=True)
        if portable_stage is not None:
            await asyncio.to_thread(shutil.rmtree, portable_stage, True)
        task.status = TaskStatus.FAILED
        task.error_code = (
            "UPDATE_PORTABLE_INVALID"
            if asset_kind == "portable"
            else "UPDATE_INSTALLER_INVALID"
        )
        task.error_stage = "verifying"
        task.error_message = f"更新包验证失败：{exc}"
        task.last_log = f"更新包已下载，但无法自动启动：{exc}"
        await manager.save_task(task)
        return
    update_service._install_started = True
    timer = threading.Timer(0.1, request_shutdown)
    timer.daemon = True
    timer.start()


@router.post("/recognize/harvest/probe")
async def harvest_probe_links(body: PageHarvestProbeRequest, x_token: str = Header(default="")):
    _check_token(x_token)
    headers = sanitize_request_headers(body.request_headers)
    headers["user-agent"] = body.user_agent or settings.default_user_agent
    if body.referer:
        headers["referer"] = body.referer
    if body.origin:
        headers["origin"] = body.origin
    cookie = body.cookie or settings.default_cookie
    if cookie:
        headers["cookie"] = cookie
    probes = await probe_harvest_links(body.urls, headers=headers)
    return {"probes": probes}


@router.post("/recognize/harvest")
async def harvest_input_page(body: PageHarvestRequest, x_token: str = Header(default="")):
    _check_token(x_token)
    _check_host(body.url)
    headers = sanitize_request_headers(body.request_headers)
    headers["user-agent"] = body.user_agent or settings.default_user_agent
    if body.referer:
        headers["referer"] = body.referer
    if body.origin:
        headers["origin"] = body.origin
    cookie = body.cookie or settings.default_cookie
    if cookie:
        headers["cookie"] = cookie
    try:
        return await harvest_page(body.url, headers=headers, extensions=body.extensions)
    except HarvestError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/recognize")
async def recognize_input_url(body: UrlRecognitionRequest, x_token: str = Header(default="")):
    _check_token(x_token)
    _check_host(body.url)
    headers = sanitize_request_headers(body.request_headers)
    headers["user-agent"] = body.user_agent or settings.default_user_agent
    if body.referer:
        headers["referer"] = body.referer
    if body.origin:
        headers["origin"] = body.origin
    cookie = body.cookie or settings.default_cookie
    if cookie:
        headers["cookie"] = cookie
    try:
        return await recognize_url(body.url, headers=headers)
    except RecognitionError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/manifest/tracks")
async def manifest_tracks(body: UrlRecognitionRequest, x_token: str = Header(default="")):
    """List user-selectable renditions of an HLS master or DASH MPD.

    Best-effort: any fetch/parse problem returns empty lists so the client
    simply falls back to automatic selection.
    """
    _check_token(x_token)
    _check_host(body.url)
    headers = sanitize_request_headers(body.request_headers)
    headers["user-agent"] = body.user_agent or settings.default_user_agent
    if body.referer:
        headers["referer"] = body.referer
    if body.origin:
        headers["origin"] = body.origin
    if body.cookie:
        headers["cookie"] = body.cookie
    empty = {"format": "", "video": [], "audio": []}
    try:
        async with policy_httpx_client(
            follow_redirects=True,
            timeout=httpx.Timeout(10),
        ) as client:
            async with client.stream("GET", body.url, headers=headers) as response:
                response.raise_for_status()
                payload = bytearray()
                async for chunk in response.aiter_bytes():
                    payload.extend(chunk)
                    if len(payload) > 4 * 1024 * 1024:
                        return empty
                encoding = response.encoding or "utf-8"
                text = bytes(payload).decode(encoding, errors="replace")
                final_url = str(response.url or body.url)
                ensure_url_allowed(final_url)
    except Exception:
        return empty
    stripped = text.lstrip("﻿ \t\r\n")
    try:
        if stripped.startswith("#EXTM3U"):
            from .downloader.parser import list_hls_audio_tracks, list_hls_video_tracks

            return {
                "format": "hls",
                "video": list_hls_video_tracks(final_url, text),
                "audio": list_hls_audio_tracks(final_url, text),
            }
        if "<MPD" in text[:4096]:
            from .downloader.mpd import NativeDashUnsupported, parse_mpd

            try:
                parsed = parse_mpd(final_url, text)
            except NativeDashUnsupported:
                return {"format": "dash", "video": [], "audio": []}
            audio_by_choice: dict[str, dict] = {}
            for option in parsed.get("audio_options") or []:
                choice = option["lang"] or option["id"]
                current = audio_by_choice.get(choice)
                if current is None or option["bandwidth"] > current["bandwidth"]:
                    audio_by_choice[choice] = {**option, "id": choice}
            return {
                "format": "dash",
                "video": sorted(
                    parsed.get("video_options") or [],
                    key=lambda item: (item["height"], item["bandwidth"]),
                    reverse=True,
                ),
                "audio": list(audio_by_choice.values()),
            }
    except Exception:
        return empty
    return empty


@router.get("/test")
async def test_connection(x_token: str = Header(default="")):
    import shutil
    _check_token(x_token)
    results: dict[str, object] = {"health": True}
    results["browser_bridge"] = "native-messaging"
    ffmpeg_found = shutil.which(settings.ffmpeg_path) is not None
    if not ffmpeg_found:
        ffmpeg_found = Path(settings.ffmpeg_path).exists()
    results["ffmpeg"] = ffmpeg_found
    results["ffmpeg_path"] = settings.ffmpeg_path
    results["download_dir"] = settings.download_dir
    results["temp_dir"] = settings.temp_dir
    results["concurrency"] = settings.default_concurrency
    results["max_tasks"] = settings.max_concurrent_tasks
    return results

@router.get("/settings")
async def get_settings(x_token: str = Header(default="")):
    _check_token(x_token)
    return _public_settings()

@router.post("/settings")
async def update_settings(body: SettingsUpdate, x_token: str = Header(default="")):
    _check_token(x_token)
    data = body.model_dump(exclude_none=True)
    if data.get("default_cookie") == SECRET_MASK:
        data.pop("default_cookie", None)
    if data.get("proxy_url") == SECRET_MASK:
        data.pop("proxy_url", None)
    if "site_profiles" in data:
        data["site_profiles"] = restore_masked_site_profiles(
            data["site_profiles"], settings.site_profiles
        )
    apply_settings_update(settings, data)
    download_throttle.configure(effective_download_speed_limit_kib())
    return _public_settings()


@router.get("/tvbox/scan")
async def scan_tvbox_devices(x_token: str = Header(default="")):
    _check_token(x_token)
    return {"devices": await scan_tvboxes()}


@router.get("/cast/scan")
async def scan_cast_devices_endpoint(x_token: str = Header(default="")):
    _check_token(x_token)
    return {"devices": await scan_cast_devices()}


@router.post("/tvbox/push")
async def push_tvbox_url(body: TvboxPush, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    endpoint = body.endpoint or settings.tvbox_endpoint
    if not endpoint:
        raise HTTPException(status_code=409, detail="请先在设置中选择电视推送设备")
    try:
        return await push_tvbox(endpoint, body.url)
    except Exception as exc:
        raise HTTPException(status_code=502, detail=f"电视推送失败：{exc}") from exc


@router.post("/tvbox/push-local")
async def push_local_tvbox_file(body: TvboxLocalPush, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    endpoint = body.endpoint or settings.tvbox_endpoint
    if not endpoint:
        raise HTTPException(status_code=409, detail="请先在设置中选择电视推送设备")
    share: dict | None = None
    try:
        share = local_media_server.share(body.path, endpoint)
        result = await push_tvbox(endpoint, share["url"])
        return {**result, "share": share}
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    except Exception as exc:
        if share:
            local_media_server.revoke(share.get("id", ""))
        raise HTTPException(status_code=502, detail=f"本机文件推送失败：{exc}") from exc


@router.post("/cast/push-local")
async def cast_local_file(body: CastLocalPush, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    device = body.device or settings.cast_device
    if not device:
        raise HTTPException(status_code=409, detail="请先在设置中扫描并选择投屏设备")
    share: dict | None = None
    try:
        selected = normalize_cast_device(device)
        share = local_media_server.share(body.path, selected["location"])
        result = await cast_media(selected, share["url"], share["filename"])
        return {**result, "share": share}
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    except Exception as exc:
        if share:
            local_media_server.revoke(share.get("id", ""))
        raise HTTPException(status_code=502, detail=f"投屏失败：{exc}") from exc


def _task_stream_share(task_id: str, endpoint: str) -> dict:
    """Create a LAN URL that waits for verified ranges of an active HTTP task."""
    task = manager.tasks.get(task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="任务不存在")
    if task.task_type not in {TaskType.HTTP, TaskType.TORRENT, TaskType.FTP}:
        raise HTTPException(
            status_code=409,
            detail="当前下载是分片直播，尚未生成连续本地文件；请先录制完成再投屏",
        )
    if not manager.playback_ready(task):
        raise HTTPException(status_code=425, detail="下载尚未获得可播放的文件范围")
    try:
        _, size = manager.get_stream_info(task.id)
    except TaskConflictError as exc:
        raise HTTPException(status_code=425, detail=str(exc)) from exc
    loop = asyncio.get_running_loop()

    def read_range(start: int, end: int) -> bytes:
        future = asyncio.run_coroutine_threadsafe(
            manager.wait_for_stream_range(task.id, start, end, timeout=45.0),
            loop,
        )
        try:
            path, _total = future.result(timeout=52.0)
            with path.open("rb") as stream:
                stream.seek(start)
                return stream.read(end - start + 1)
        except Exception as exc:
            raise RuntimeError("目标下载范围尚未准备好") from exc

    filename = task.filename or task.title or task.id
    return local_media_server.share_stream(
        filename=filename,
        size=size,
        endpoint=endpoint,
        read_range=read_range,
        mime_type=task.mime_type,
    )


def _rewrite_local_playback_playlist(content: str, share_id: str) -> str:
    """Rewrite loopback playback URIs to one token-scoped LAN share."""
    token = str(share_id or "").strip()
    if not token:
        raise RuntimeError("本地媒体共享尚未初始化")

    def rewrite_uri(raw_uri: str) -> str:
        parsed = urlsplit(raw_uri.strip().strip('"'))
        path = parsed.path.lstrip("/")
        parts = path.split("/")
        if len(parts) != 2 or parts[0] not in {"segments", "maps"}:
            raise RuntimeError("本地播放清单包含未识别的资源 URI")
        name = Path(parts[1]).name
        if name != parts[1] or (parts[0] == "segments" and not re.fullmatch(r"\d{6}\.seg", name)):
            raise RuntimeError("本地播放清单包含无效的分片路径")
        if parts[0] == "maps" and not name.endswith(".init"):
            raise RuntimeError("本地播放清单包含无效的初始化片段")
        return f"/media/{token}/{parts[0]}/{quote(name)}"

    lines: list[str] = []
    for raw_line in str(content or "").splitlines():
        line = raw_line.strip()
        if not line:
            lines.append(raw_line)
            continue
        # EXT-X-MAP stores the URI inside a tag rather than on its own line.
        # Strip the playback session/access token from both URI forms.
        if line.startswith("#EXT-X-MAP:"):
            match = re.search(r'URI="([^"]+)"', line)
            if not match:
                raise RuntimeError("本地播放清单的初始化片段缺少 URI")
            replacement = rewrite_uri(match.group(1))
            lines.append(line[:match.start(1)] + replacement + line[match.end(1):])
            continue
        if line.startswith("#"):
            lines.append(raw_line)
            continue
        lines.append(rewrite_uri(line))
    return "\n".join(lines) + "\n"


def _task_segment_share(task_id: str, endpoint: str) -> dict:
    """Expose an HLS/DASH task's verified local playback session to a TV.

    The LAN server receives a rewritten local playlist and local segment/map
    callbacks.  It never forwards the original CDN URL, cookies, or signed
    query string to a television.
    """
    task = manager.tasks.get(task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="任务不存在")
    if not _uses_segment_playback(task):
        raise HTTPException(status_code=409, detail="该任务尚未生成可投屏的本地分片播放清单")
    try:
        session_id, _snapshot = playback_service.open_ready_session(
            task.id,
            task.status.value,
            task.output_path,
        )
        access_token = playback_service.access_token(task.id, session_id)
    except PlaybackError as exc:
        _raise_playback_error(exc)
    loop = asyncio.get_running_loop()
    share_ref: dict[str, str] = {"id": ""}

    def playlist() -> str:
        playback_service.authorize(task.id, session_id, access_token)
        content = playback_service.playlist(
            task.id,
            task.status.value,
            session_id,
            access_token=access_token,
        )
        return _rewrite_local_playback_playlist(content, share_ref["id"])

    def read_asset(kind: str, name: str) -> tuple[bytes, str]:
        playback_service.authorize(task.id, session_id, access_token)
        if kind == "maps":
            path = playback_service.map_path(task.id, name, session_id)
            mime = "video/mp4"
        elif kind == "segments":
            match = re.fullmatch(r"(\d{6})\.seg", name)
            if not match:
                raise PlaybackError("无效的本地分片")
            future = asyncio.run_coroutine_threadsafe(
                playback_service.wait_for_segment(
                    task.id,
                    int(match.group(1)),
                    session_id,
                    sparse=False,
                    timeout=45.0,
                ),
                loop,
            )
            path, is_fmp4 = future.result(timeout=52.0)
            mime = "video/mp4" if is_fmp4 else "video/mp2t"
        else:
            raise PlaybackError("无效的本地播放资源")
        if not path.is_file():
            raise PlaybackNotReadyError("本地播放资源尚未准备好")
        size = path.stat().st_size
        if size <= 0 or size > 64 * 1024 * 1024:
            raise PlaybackError("本地播放分片大小异常")
        return path.read_bytes(), mime

    share = local_media_server.share_playlist(
        filename=f"{task.filename or task.title or task.id}.m3u8",
        endpoint=endpoint,
        playlist=playlist,
        read_asset=read_asset,
    )
    share_ref["id"] = str(share.get("id") or "")
    return share


def _task_share(task_id: str, endpoint: str) -> dict:
    task = manager.tasks.get(task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="任务不存在")
    if task.status is TaskStatus.DONE and task.output_path and task_output_is_file(task):
        return local_media_server.share(task.output_path, endpoint)
    if task.task_type in {TaskType.HLS, TaskType.DASH}:
        return _task_segment_share(task_id, endpoint)
    return _task_stream_share(task_id, endpoint)


@router.post("/tvbox/push-task")
async def push_tvbox_task(body: TvboxTaskPush, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    endpoint = body.endpoint or settings.tvbox_endpoint
    if not endpoint:
        raise HTTPException(status_code=409, detail="请先在设置中选择电视推送设备")
    share: dict | None = None
    try:
        share = _task_share(body.task_id, endpoint)
        result = await push_tvbox(endpoint, share["url"])
        return {**result, "share": share}
    except HTTPException:
        raise
    except Exception as exc:
        if share:
            local_media_server.revoke(share.get("id", ""))
        raise HTTPException(status_code=502, detail=f"下载中任务 TVBox 推送失败：{exc}") from exc


@router.post("/cast/push-task")
async def cast_task(body: CastTaskPush, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    if not body.device:
        raise HTTPException(status_code=409, detail="请先选择投屏设备")
    share: dict | None = None
    try:
        device = normalize_cast_device(body.device)
        share = _task_share(body.task_id, device["location"])
        result = await cast_media(device, share["url"], share["filename"])
        return {**result, "share": share}
    except HTTPException:
        raise
    except Exception as exc:
        if share:
            local_media_server.revoke(share.get("id", ""))
        raise HTTPException(status_code=502, detail=f"下载中任务投屏失败：{exc}") from exc


@router.post("/cast/push")
async def cast_url(body: CastUrlPush, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    device = body.device or settings.cast_device
    if not device:
        raise HTTPException(status_code=409, detail="请选择投屏设备")
    try:
        return await cast_media(normalize_cast_device(device), body.url, body.filename or "video")
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    except Exception as exc:
        raise HTTPException(status_code=502, detail=f"投屏失败：{exc}") from exc


@router.post("/browser/media-push")
async def create_browser_media_push(request: Request, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    _require_legal_acceptance()
    model = await _read_browser_json(request, BrowserMediaPushCreate)
    kind = model.kind
    resource = _browser_payload(model.resource)
    url = str(resource.get("url", ""))
    if kind not in {"cast", "tvbox"} or not url.startswith(("http://", "https://")):
        raise HTTPException(status_code=422, detail="浏览器投送请求无效")
    request_id = uuid.uuid4().hex
    with _browser_media_push_lock:
        now = time.monotonic()
        for stale_id, item in list(_browser_media_pushes.items()):
            if now - float(item.get("created_at", now)) > 180:
                _browser_media_pushes.pop(stale_id, None)
        while len(_browser_media_pushes) >= MAX_BROWSER_MEDIA_PUSHES:
            oldest_id = min(
                _browser_media_pushes,
                key=lambda key: float(_browser_media_pushes[key].get("created_at", now)),
            )
            _browser_media_pushes.pop(oldest_id, None)
        _browser_media_pushes[request_id] = {"id": request_id, "kind": kind, "resource": resource, "created_at": now, "status": "pending", "message": "等待在桌面端选择设备"}
    native_desktop_session.queue("media_push", request_id)
    if not native_desktop_session.status().get("active"):
        spawned = maybe_spawn_desktop_ui_process(project_root=PROJECT_ROOT)
        if spawned is None and not os.environ.get("PYTEST_CURRENT_TEST"):
            with _browser_media_push_lock:
                pending_item = _browser_media_pushes.get(request_id)
                if pending_item is not None and pending_item.get("status") == "pending":
                    pending_item["status"] = "failed"
                    pending_item["message"] = "未能打开桌面设置窗口，请先打开下载器再投屏"
    activate_window()
    return {"ok": True, "id": request_id}


@router.get("/browser/media-push/{request_id}")
async def get_browser_media_push(request_id: str, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    with _browser_media_push_lock:
        item = _browser_media_pushes.get(request_id)
    if not item:
        raise HTTPException(status_code=404, detail="投送请求不存在或已过期")
    return {key: value for key, value in item.items() if key != "created_at"}


@router.get("/browser/media-push/{request_id}/status")
async def get_browser_media_push_status(request_id: str, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    with _browser_media_push_lock:
        item = _browser_media_pushes.get(request_id)
    if not item:
        raise HTTPException(status_code=404, detail="投送请求不存在或已过期")
    return {key: value for key, value in item.items() if key in {"id", "status", "message"}}


@router.post("/browser/media-push/{request_id}/complete")
async def complete_browser_media_push(request_id: str, request: Request, x_token: str = Header(default="")):
    _check_browser_token(x_token)
    model = await _read_browser_json(request, BrowserMediaPushComplete)
    status = model.status
    message = model.message.strip()
    with _browser_media_push_lock:
        item = _browser_media_pushes.get(request_id)
        if not item:
            raise HTTPException(status_code=404, detail="投送请求不存在或已过期")
        item["status"] = status
        item["message"] = message or ("已完成" if status == "done" else "投送失败" if status == "failed" else "已取消")
    return {"ok": True}


@router.post("/cast/control")
async def control_cast(body: CastControl, x_token: str = Header(default="")):
    _check_token(x_token)
    device = body.device or settings.cast_device
    if not device:
        raise HTTPException(status_code=409, detail="请先在设置中扫描并选择投屏设备")
    try:
        return await cast_control(device, body.action, body.seconds)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    except Exception as exc:
        raise HTTPException(status_code=502, detail=f"投屏控制失败：{exc}") from exc


@router.post("/tvbox/shares/{share_id}/stop")
async def stop_local_tvbox_share(share_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    local_media_server.revoke(share_id)
    return {"ok": True}


@router.get("/tvbox/shares/{share_id}")
async def get_local_tvbox_share(share_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    return local_media_server.status(share_id)

@router.post("/tasks", response_model=TaskResponse)
async def create_task(body: TaskCreate, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    _check_host(body.url)
    for mirror in body.mirrors:
        _check_host(mirror)
    _require_allow_duplicate(body.url, body.allow_duplicate)
    task = await manager.create_task(
        url=body.url, task_type=body.task_type,
        source_page_url=body.source_page_url, mime_type=body.mime_type,
        referer=body.referer, origin=body.origin,
        user_agent=body.user_agent, cookie=body.cookie,
        request_headers=body.request_headers,
        request_contexts=body.request_contexts,
        request_method=body.request_method,
        request_body=body.request_body,
        title=body.title, filename=body.filename,
        concurrency=body.concurrency,
        checksum=body.checksum,
        output_dir=body.download_dir,
        auto_start=True,
        selected_video=body.selected_video,
        selected_audio=body.selected_audio,
        scheduled_start_at=body.scheduled_start_at.isoformat() if body.scheduled_start_at else "",
        scheduled_stop_at=body.scheduled_stop_at.isoformat() if body.scheduled_stop_at else "",
        completion_action=body.completion_action,
        mirrors=body.mirrors,
    )
    return _to_resp(task)

@router.post("/tasks/batch")
async def create_batch(body: TaskBatchCreate, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    for task in body.tasks:
        _check_host(task.url)
        for mirror in task.mirrors:
            _check_host(mirror)
        _require_allow_duplicate(task.url, task.allow_duplicate)
    results = []
    for t in body.tasks:
        task = await manager.create_task(
            url=t.url, task_type=t.task_type,
            source_page_url=t.source_page_url, mime_type=t.mime_type,
            referer=t.referer, origin=t.origin,
            user_agent=t.user_agent, cookie=t.cookie,
            request_headers=t.request_headers,
            request_contexts=t.request_contexts,
            request_method=t.request_method,
            request_body=t.request_body,
            title=t.title, filename=t.filename,
            concurrency=t.concurrency,
            checksum=t.checksum,
            output_dir=t.download_dir,
            auto_start=True,
            scheduled_start_at=t.scheduled_start_at.isoformat() if t.scheduled_start_at else "",
            scheduled_stop_at=t.scheduled_stop_at.isoformat() if t.scheduled_stop_at else "",
            completion_action=t.completion_action,
            mirrors=t.mirrors,
        )
        results.append(_to_resp(task))
    return results


@router.post("/tasks/torrent-file", response_model=TaskResponse)
async def create_torrent_file_task(
    request: Request,
    file: UploadFile = File(...),
    title: str = Form(default=""),
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    _require_legal_acceptance()
    content_length = request.headers.get("content-length", "").strip()
    if content_length:
        try:
            if int(content_length) > MAX_TORRENT_MULTIPART_BODY_BYTES:
                raise HTTPException(status_code=413, detail="种子上传请求过大")
        except ValueError as exc:
            raise HTTPException(status_code=400, detail="Content-Length 无效") from exc
    if len(title) > 512:
        raise HTTPException(status_code=422, detail="种子任务标题过长")
    name = (file.filename or "download.torrent").strip()[:255]
    if not name.lower().endswith(".torrent"):
        raise HTTPException(status_code=400, detail="只接受 .torrent 文件")
    content = await file.read(16 * 1024 * 1024 + 1)
    if not content or len(content) > 16 * 1024 * 1024:
        raise HTTPException(status_code=400, detail="种子文件为空或超过 16 MiB")
    try:
        from .downloader.torrent import TorrentDownloader
        metadata = TorrentDownloader.inspect_torrent_bytes(content)
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    except RuntimeError as exc:
        raise HTTPException(status_code=503, detail="BT 下载组件不可用，请修复安装后重试") from exc
    task = await manager.create_task(
        url=f"torrent-file:{name}",
        task_type=TaskType.TORRENT,
        title=title or Path(name).stem,
        filename=Path(name).stem,
        auto_start=False,
    )
    task_dir = task_work_dir(task)
    task_dir.mkdir(parents=True, exist_ok=True)
    source = task_dir / "uploaded.torrent"
    source.write_bytes(content)
    files = metadata["files"]
    task.engine_state.update({
        "torrent_path": str(source),
        "files": files,
        "selected_files": [entry["index"] for entry in files],
    })
    task.title = title or metadata["name"] or task.title
    task.filename = task.title or task.filename
    task.progress.total_bytes = sum(int(entry["size"]) for entry in files)
    task.progress.total_segments = int(metadata["piece_count"])
    task.status = TaskStatus.AWAITING_SELECTION
    task.stage = "awaiting_selection"
    task.last_log = "请选择要下载的 BT 文件，然后点击开始下载"
    await manager.save_task(task)
    return _to_resp(task)


@router.post("/tasks/torrent-path", response_model=TaskResponse)
async def create_torrent_path_task(body: TorrentPathImport, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    source = Path(body.path).expanduser()
    if source.suffix.lower() != ".torrent" or not source.is_file():
        raise HTTPException(status_code=400, detail="请选择有效的 .torrent 文件")
    try:
        if source.stat().st_size > 16 * 1024 * 1024:
            raise ValueError("种子文件超过 16 MiB 限制")
        with source.open("rb") as handle:
            content = handle.read(16 * 1024 * 1024 + 1)
        if len(content) > 16 * 1024 * 1024:
            raise ValueError("种子文件超过 16 MiB 限制")
        from .downloader.torrent import TorrentDownloader
        metadata = TorrentDownloader.inspect_torrent_bytes(content)
    except (OSError, ValueError) as exc:
        raise HTTPException(status_code=400, detail="种子文件无法读取或已损坏") from exc
    task = await manager.create_task(url=f"torrent-file:{source.name}", task_type=TaskType.TORRENT, title=source.stem, filename=source.stem, auto_start=False)
    task_dir = task_work_dir(task)
    task_dir.mkdir(parents=True, exist_ok=True)
    saved = task_dir / "uploaded.torrent"
    saved.write_bytes(content)
    files = metadata["files"]
    task.engine_state.update({
        "torrent_path": str(saved),
        "files": files,
        "selected_files": [entry["index"] for entry in files],
    })
    task.title = metadata["name"] or task.title
    task.filename = task.title or task.filename
    task.progress.total_bytes = sum(int(entry["size"]) for entry in files)
    task.progress.total_segments = int(metadata["piece_count"])
    task.status = TaskStatus.AWAITING_SELECTION
    task.stage = "awaiting_selection"
    task.last_log = "请选择要下载的 BT 文件，然后点击开始下载"
    await manager.save_task(task)
    return _to_resp(task)



@router.post("/tasks/link-path", response_model=TaskResponse)
async def create_link_path_task(body: LinkPathImport, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    source = Path(body.path).expanduser()
    try:
        from .link_file import LinkFileError, read_link_urls
        from .metalink import METALINK_SUFFIXES, read_metalink_files
        if source.suffix.lower() in METALINK_SUFFIXES:
            jobs = [
                {"url": item.url, "title": item.name, "filename": item.name, "checksum": item.checksum, "mirrors": item.mirrors}
                for item in read_metalink_files(source)
            ]
        else:
            urls = read_link_urls(source)
            jobs = [{"url": url, "title": "", "filename": "", "checksum": "", "mirrors": []} for url in urls]
    except (OSError, LinkFileError) as exc:
        raise HTTPException(status_code=400, detail=str(exc) or "link file invalid") from exc
    if not jobs:
        raise HTTPException(status_code=400, detail="link file invalid")
    # A single Explorer shortcut keeps its previous auto-start behavior.
    # Multi-link playlists/pages are queued so a saved HTML file cannot
    # suddenly start dozens of downloads.
    auto_start = bool(body.auto_start) and len(jobs) == 1
    created = None
    last_conflict = None
    for index, job in enumerate(jobs):
        url = job["url"]
        _check_host(url)
        try:
            _require_allow_duplicate(url, False)
        except HTTPException as exc:
            last_conflict = exc
            continue
        title = job["title"] or (source.stem if len(jobs) == 1 else f"{source.stem}-{index + 1}")
        task = await manager.create_task(
            url=url,
            title=title,
            filename=job["filename"] or title,
            checksum=job["checksum"],
            mirrors=job["mirrors"],
            auto_start=auto_start,
        )
        created = created or task
    if created is None:
        if last_conflict is not None:
            raise last_conflict
        raise HTTPException(status_code=400, detail="link file invalid")
    return _to_resp(created)


@router.get("/tasks/{task_id}/files")
async def get_task_files(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    task = manager.tasks.get(task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="Task not found")
    if task.task_type is not TaskType.TORRENT:
        raise HTTPException(status_code=409, detail="该任务不是 BT 任务")
    return {
        "files": task.engine_state.get("files", []),
        "selected": task.engine_state.get("selected_files", []),
    }


@router.put("/tasks/{task_id}/files")
async def select_task_files(
    task_id: str,
    body: TorrentFileSelection,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    await _manager_action(manager.select_torrent_files(task_id, body.indexes))
    return {"ok": True}

@router.get("/tasks", response_model=list[TaskResponse])
async def list_tasks(x_token: str = Header(default="")):
    _check_token(x_token)
    return [_to_resp(t) for t in manager.tasks.values()]

@router.delete("/tasks/completed")
async def clear_completed_tasks(x_token: str = Header(default="")):
    _check_token(x_token)
    task_ids = [task.id for task in manager.tasks.values() if task.status.value == "done"]
    for task_id in task_ids:
        await _manager_action(manager.delete_task(task_id))
    return {"ok": True, "count": len(task_ids)}

@router.get("/tasks/{task_id}", response_model=TaskResponse)
async def get_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    task = manager.tasks.get(task_id)
    if not task:
        raise HTTPException(status_code=404, detail="Task not found")
    return _to_resp(task)


@router.post("/tasks/{task_id}/site-profile")
async def save_task_site_profile(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    task = manager.tasks.get(task_id)
    if not task:
        raise HTTPException(status_code=404, detail="Task not found")
    try:
        profile = site_profile_from_task(task)
        host = str(profile.get("host") or "")
        for item in settings.site_profiles or []:
            if not isinstance(item, dict):
                continue
            existing = str(item.get("host") or "").strip().lower().rstrip(".")
            if existing == host:
                mode, proxy_url = normalize_site_proxy(item)
                profile["proxy_mode"] = mode
                profile["proxy_url"] = proxy_url
                break
    except ValueError:
        raise HTTPException(status_code=409, detail="task has no hostname")
    next_profiles, action = upsert_site_profile(list(settings.site_profiles or []), profile)
    apply_settings_update(settings, {"site_profiles": next_profiles})
    public = mask_site_profiles([profile])[0]
    return {"ok": True, "action": action, "host": profile["host"], "profile": public}


@router.post("/tasks/{task_id}/start")
async def start_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    await _manager_action(manager.start_task(task_id))
    return {"ok": True}

@router.post("/tasks/{task_id}/pause")
async def pause_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    await _manager_action(manager.pause_task(task_id))
    return {"ok": True}

@router.post("/tasks/{task_id}/resume")
async def resume_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    await _manager_action(manager.resume_task(task_id))
    return {"ok": True}

@router.post("/tasks/{task_id}/cancel")
async def cancel_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    await _manager_action(manager.cancel_task(task_id))
    return {"ok": True}

@router.post("/tasks/{task_id}/retry")
async def retry_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    _require_legal_acceptance()
    await _manager_action(manager.retry_task(task_id))
    return {"ok": True}


@router.patch("/tasks/{task_id}/request", response_model=TaskResponse)
async def refresh_task_request(
    task_id: str,
    body: TaskRequestUpdate,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    _require_legal_acceptance()
    _check_host(body.url)
    values = body.model_dump(exclude_unset=True)
    values.pop("url", None)
    try:
        task = await manager.refresh_task_request(task_id, url=body.url, **values)
    except (TaskNotFoundError, TaskConflictError) as exc:
        status = 404 if isinstance(exc, TaskNotFoundError) else 409
        raise HTTPException(status_code=status, detail=str(exc)) from exc
    return _to_resp(task)

@router.post("/tasks/{task_id}/speed-limit")
async def set_task_speed_limit(
    task_id: str, body: TaskSpeedLimit, x_token: str = Header(default="")
):
    _check_token(x_token)
    await _manager_action(manager.set_task_speed_limit(task_id, body.limit_kib))
    return {"ok": True}


@router.post("/tasks/{task_id}/queue/{direction}", response_model=TaskResponse)
async def reorder_task_queue(task_id: str, direction: str, x_token: str = Header(default="")):
    """Move a queued task by relative direction or an absolute before/after/index target."""
    _check_token(x_token)
    try:
        kind, payload = parse_queue_direction(direction)
    except TaskConflictError as exc:
        raise HTTPException(
            status_code=422,
            detail="direction must be up, down, top, bottom, before:<id>, after:<id>, or index:<n>",
        ) from exc
    if kind in {"before", "after"}:
        direction = f"{kind}:{payload}"
    elif kind == "index":
        direction = f"index:{payload}"
    else:
        direction = kind
    try:
        task = await manager.reorder_queue(task_id, direction)
    except TaskNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except TaskConflictError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    return _to_resp(task)

@router.delete("/tasks/{task_id}")
async def delete_task(task_id: str, delete_files: bool = False, x_token: str = Header(default="")):
    _check_token(x_token)
    await _manager_action(manager.delete_task(task_id, delete_files=delete_files))
    return {"ok": True}


@router.get("/tasks/{task_id}/file")
async def download_task_file(
    task_id: str,
    token: str = "",
    x_token: str = Header(default=""),
):
    if x_token:
        _check_token(x_token)
    elif not verify_file_access_token(task_id, token):
        raise HTTPException(status_code=401, detail="Invalid file access token")
    task = manager.tasks.get(task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="Task not found")
    if task.status is not TaskStatus.DONE or not task.output_path:
        raise HTTPException(status_code=409, detail="任务尚未下载完成")
    path = Path(task.output_path)
    if not path.exists() or not path.is_file():
        raise HTTPException(status_code=404, detail="下载文件不存在或该任务包含多个文件")
    return FileResponse(path, filename=path.name, headers={"Cache-Control": "private, no-store"})

@router.get("/tasks/{task_id}/log")
async def get_task_log(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    task = manager.tasks.get(task_id)
    if not task:
        raise HTTPException(status_code=404, detail="Task not found")
    log_file = task_work_dir(task) / "download.log"
    if log_file.exists():
        return {"log": log_file.read_text(encoding="utf-8", errors="replace")}
    return {
        "log": (
            f"stage: {task.stage}\n"
            f"error_code: {task.error_code}\n"
            f"last_log: {task.last_log}\n"
            f"error: {task.error_message}\n"
            f"hint: {task.error_hint}"
        )
    }


def _playback_task(task_id: str):
    task = manager.tasks.get(task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="Task not found")
    return task


def _raise_playback_error(exc: PlaybackError):
    if isinstance(exc, PlaybackAuthorizationError):
        raise HTTPException(status_code=401, detail=str(exc)) from exc
    if isinstance(exc, PlaybackSessionError):
        raise HTTPException(status_code=410, detail=str(exc)) from exc
    if isinstance(exc, PlaybackNotReadyError):
        raise HTTPException(status_code=425, detail=str(exc)) from exc
    raise HTTPException(status_code=400, detail=str(exc)) from exc


def _uses_segment_playback(task) -> bool:
    """DASH native downloads share the same local HLS preview layout."""
    if task.task_type is TaskType.HLS:
        return True
    return (
        task.task_type is TaskType.DASH
        and (task_work_dir(task) / "playback-plan.json").is_file()
    )


@router.post("/tasks/{task_id}/playback")
async def open_task_playback(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    task = _playback_task(task_id)
    if not _uses_segment_playback(task):
        try:
            _, size = manager.get_stream_info(task.id)
        except TaskConflictError as exc:
            raise HTTPException(status_code=425, detail=str(exc)) from exc
        session_id = playback_service.open_session(task.id)
        return {
            "session_id": session_id,
            "playback_token": playback_service.access_token(task.id, session_id),
            "ready": True,
            "mode": "file",
            "available_segments": task.progress.completed_segments,
            "total_segments": task.progress.total_segments,
            "available_duration": task.progress.media_duration,
            "total_duration": task.progress.media_duration,
            "complete": task.status is TaskStatus.DONE,
            "total_bytes": size,
        }
    try:
        session_id, snapshot = playback_service.open_ready_session(
            task.id,
            task.status.value,
            task.output_path,
        )
    except PlaybackError as exc:
        _raise_playback_error(exc)
    return {
        "session_id": session_id,
        "playback_token": playback_service.access_token(task.id, session_id),
        **snapshot.to_dict(),
    }


@router.get("/tasks/{task_id}/playback/status")
async def task_playback_status(
    task_id: str,
    session: str,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    task = _playback_task(task_id)
    try:
        playback_service.touch(task.id, session)
        if not _uses_segment_playback(task):
            _, size = manager.get_stream_info(task.id)
            return {
                "ready": True,
                "mode": "file",
                "available_segments": task.progress.completed_segments,
                "total_segments": task.progress.total_segments,
                "available_duration": task.progress.media_duration,
                "total_duration": task.progress.media_duration,
                "complete": task.status is TaskStatus.DONE,
                "total_bytes": size,
            }
        return playback_service.snapshot(
            task.id,
            task.status.value,
            task.output_path,
        ).to_dict()
    except PlaybackError as exc:
        _raise_playback_error(exc)


@router.post("/tasks/{task_id}/playback/heartbeat")
async def heartbeat_task_playback(
    task_id: str,
    session: str,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    _playback_task(task_id)
    try:
        playback_service.touch(task_id, session)
    except PlaybackError as exc:
        _raise_playback_error(exc)
    return {"ok": True}


@router.post("/tasks/{task_id}/playback/seek")
async def seek_task_playback(
    task_id: str,
    request: PlaybackSeekRequest,
    session: str,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    _playback_task(task_id)
    try:
        target = playback_service.request_seek(task_id, session, request.time)
        await manager.request_playback_seek(task_id, target["index"])
        return target
    except PlaybackError as exc:
        _raise_playback_error(exc)
    except TaskNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@router.delete("/tasks/{task_id}/playback")
async def close_task_playback(
    task_id: str,
    session: str,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    try:
        closed = await manager.release_playback(task_id, session)
    except TaskNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {"ok": closed}


@router.get("/tasks/{task_id}/playback/index.m3u8")
async def task_playback_playlist(
    task_id: str,
    session: str,
    token: str = "",
    full: bool = False,
    x_token: str = Header(default=""),
):
    task = _playback_task(task_id)
    try:
        playback_service.authorize(task.id, session, token)
        content = playback_service.playlist(
            task.id,
            task.status.value,
            session,
            access_token=token,
            full=full,
        )
    except PlaybackError as exc:
        _raise_playback_error(exc)
    return PlainTextResponse(
        content,
        media_type="application/vnd.apple.mpegurl",
        headers={"Cache-Control": "no-store"},
    )


@router.get("/tasks/{task_id}/playback/segments/{index}.seg")
async def task_playback_segment(
    task_id: str,
    index: int,
    session: str,
    token: str = "",
    full: bool = False,
    x_token: str = Header(default=""),
):
    _playback_task(task_id)
    try:
        playback_service.authorize(task_id, session, token)
        if full:
            await manager.request_playback_seek(task_id, index, force=False)
            path, is_fmp4 = await playback_service.wait_for_segment(
                task_id,
                index,
                session,
                sparse=True,
            )
        else:
            path, is_fmp4 = playback_service.segment_path(task_id, index, session)
    except PlaybackError as exc:
        _raise_playback_error(exc)
    return FileResponse(
        path,
        media_type="video/mp4" if is_fmp4 else "video/mp2t",
        headers={"Cache-Control": "private, max-age=3600"},
    )


@router.get("/tasks/{task_id}/playback/maps/{map_name}")
async def task_playback_map(
    task_id: str,
    map_name: str,
    session: str,
    token: str = "",
    x_token: str = Header(default=""),
):
    _playback_task(task_id)
    try:
        playback_service.authorize(task_id, session, token)
        path = playback_service.map_path(task_id, map_name, session)
    except PlaybackError as exc:
        _raise_playback_error(exc)
    return FileResponse(
        path,
        media_type="video/mp4",
        headers={"Cache-Control": "private, max-age=3600"},
    )


@router.get("/tasks/{task_id}/playback/media")
async def task_playback_media(
    task_id: str,
    request: Request,
    session: str,
    token: str = "",
    x_token: str = Header(default=""),
):
    task = _playback_task(task_id)
    try:
        playback_service.authorize(task_id, session, token)
    except PlaybackError as exc:
        _raise_playback_error(exc)
    if task.task_type is not TaskType.HLS:
        try:
            path, total = manager.get_stream_info(task_id)
        except TaskConflictError as exc:
            raise HTTPException(status_code=425, detail=str(exc)) from exc
        range_header = request.headers.get("range", "")
        match = re.match(r"^bytes=(\d*)-(\d*)$", range_header, re.IGNORECASE)
        if range_header and not match:
            raise HTTPException(
                status_code=416,
                detail="无效的 Range",
                headers={"Content-Range": f"bytes */{total}"},
            )
        if not range_header:
            start, end = 0, min(total - 1, 2 * 1024 * 1024 - 1)
        else:
            assert match is not None
            start_text, end_text = match.groups()
            if not start_text and not end_text:
                raise HTTPException(status_code=416, detail="无效的 Range")
            if len(start_text) > 20 or len(end_text) > 20:
                raise HTTPException(status_code=416, detail="无效的 Range")
            try:
                if start_text:
                    start = int(start_text)
                    end = int(end_text) if end_text else min(total - 1, start + 4 * 1024 * 1024 - 1)
                else:
                    length = min(int(end_text), 4 * 1024 * 1024)
                    start, end = max(0, total - length), total - 1
            except ValueError as exc:
                raise HTTPException(status_code=416, detail="无效的 Range") from exc
            end = min(end, total - 1, start + 4 * 1024 * 1024 - 1)
        if start < 0 or start >= total or end < start:
            raise HTTPException(
                status_code=416,
                detail="请求范围超出文件长度",
                headers={"Content-Range": f"bytes */{total}"},
            )
        try:
            path, total = await manager.wait_for_stream_range(task_id, start, end)
        except (TaskConflictError, FileNotFoundError, TimeoutError) as exc:
            raise HTTPException(status_code=425, detail=str(exc)) from exc

        def read_range() -> bytes:
            with path.open("rb") as media:
                media.seek(start)
                return media.read(end - start + 1)

        content = await asyncio.to_thread(read_range)
        if len(content) != end - start + 1:
            raise HTTPException(status_code=425, detail="目标字节范围尚未完整写入")
        return Response(
            content=content,
            status_code=206,
            media_type=task.mime_type or "application/octet-stream",
            headers={
                "Accept-Ranges": "bytes",
                "Content-Range": f"bytes {start}-{end}/{total}",
                "Content-Length": str(len(content)),
                "Cache-Control": "private, no-store",
            },
        )

    if task.status.value != "done" or not task.output_path:
        raise HTTPException(status_code=409, detail="最终媒体文件尚未准备好")
    path = Path(task.output_path)
    if not path.exists() or not path.is_file() or path.stat().st_size <= 0:
        raise HTTPException(status_code=404, detail="媒体文件不存在")
    return FileResponse(
        path,
        media_type="video/mp4",
        content_disposition_type="inline",
        headers={"Cache-Control": "private, max-age=3600"},
    )

@router.get("/events")
async def events(x_token: str = Header(default="")):
    _check_token(x_token)
    q = manager.subscribe()

    async def stream():
        try:
            while True:
                try:
                    event = await asyncio.wait_for(q.get(), timeout=15)
                    yield f"data: {json.dumps(event, ensure_ascii=False)}\n\n"
                except asyncio.TimeoutError:
                    # Send keepalive comment to prevent connection drop
                    yield ": keepalive\n\n"
        except (asyncio.CancelledError, GeneratorExit):
            pass
        finally:
            manager.unsubscribe(q)

    return StreamingResponse(stream(), media_type="text/event-stream")

def _task_output_path(task_id: str) -> Path:
    task = manager.tasks.get(task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="Task not found")
    if not task.output_path:
        raise HTTPException(status_code=409, detail="任务尚无输出文件")
    return Path(task.output_path).resolve()


def _path_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _allowed_explorer_path(value: str) -> Path:
    path = Path(value).resolve()
    roots = {
        Path(settings.download_dir).resolve(),
        Path(settings.temp_dir).resolve(),
        RUNTIME_PATHS.data_root.resolve(),
    }
    for task in manager.tasks.values():
        if task.output_path:
            output = Path(task.output_path).resolve()
            roots.add(output if output.is_dir() else output.parent)
    if any(_path_within(path, root) for root in roots):
        return path
    ffmpeg = str(settings.ffmpeg_path or "").strip()
    if ffmpeg and path == Path(ffmpeg).resolve():
        return path
    raise HTTPException(status_code=403, detail="只允许打开任务输出或已配置的数据目录")


@router.post("/open-explorer")
async def open_explorer(body: FileSystemAction, x_token: str = Header(default="")):
    _check_token(x_token)
    import subprocess
    p = _task_output_path(body.task_id) if body.task_id else _allowed_explorer_path(body.path)
    if not p.exists():
        parent = p.parent
        if parent.exists():
            subprocess.Popen(["explorer", str(parent)])
            return {"ok": True, "missing": True}
        raise HTTPException(status_code=404, detail="文件已删除，且所在目录也不存在")
    if p.is_file():
        subprocess.Popen(["explorer", "/select,", str(p)])
    else:
        subprocess.Popen(["explorer", str(p)])
    return {"ok": True}

@router.post("/launch-file")
async def launch_file(body: FileSystemAction, x_token: str = Header(default="")):
    _check_token(x_token)
    if not body.task_id or body.path:
        raise HTTPException(status_code=400, detail="launch-file requires task_id")
    task = manager.tasks.get(body.task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="Task not found")
    if task.status is not TaskStatus.DONE:
        raise HTTPException(status_code=409, detail="任务尚未完成")
    target = _task_output_path(body.task_id)
    if not target.exists() or not target.is_file():
        raise HTTPException(status_code=404, detail="最终文件已删除或不可访问，可重新下载")
    if target.suffix.lower() in {".bat", ".cmd", ".com", ".exe", ".js", ".msi", ".ps1", ".scr", ".vbs"} and not body.confirm_executable:
        raise HTTPException(status_code=409, detail="启动可执行文件前需要明确确认")
    import os
    if not hasattr(os, "startfile"):
        raise HTTPException(status_code=501, detail="当前系统不支持直接打开文件")
    await asyncio.to_thread(os.startfile, str(target))
    return {"ok": True}

@router.get("/browse-dir")
async def browse_dir(
    path: str = "",
    offset: int = 0,
    limit: int = 200,
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    if not path:
        # Tauri uses the native folder dialog. The web fallback starts from
        # explicitly configured application roots instead of exposing every
        # same-user drive through the local HTTP API.
        roots = {
            Path(settings.download_dir).resolve(),
            Path(settings.temp_dir).resolve(),
            RUNTIME_PATHS.data_root.resolve(),
        }
        items = [
            {"name": str(root), "path": str(root), "is_dir": True}
            for root in sorted(roots, key=lambda value: str(value).casefold())
            if root.exists() and root.is_dir()
        ]
        return {"current": "", "items": items, "parent": "", "total": len(items)}

    p = _allowed_explorer_path(path)
    if not p.exists() or not p.is_dir():
        raise HTTPException(status_code=404, detail="directory not found")

    safe_offset = max(0, int(offset))
    safe_limit = min(250, max(1, int(limit)))

    def list_directory():
        entries = []
        for child in p.iterdir():
            try:
                is_dir = child.is_dir()
                entries.append({
                    "name": child.name,
                    "path": str(child),
                    "is_dir": is_dir,
                })
            except (PermissionError, OSError):
                continue
        entries.sort(key=lambda item: (not item["is_dir"], item["name"].casefold()))
        return entries

    try:
        entries = await asyncio.wait_for(asyncio.to_thread(list_directory), timeout=5)
    except TimeoutError as exc:
        raise HTTPException(status_code=504, detail="读取目录超时") from exc
    except PermissionError as exc:
        raise HTTPException(status_code=403, detail="没有权限读取该目录") from exc

    parent = ""
    if p.parent != p:
        try:
            parent = str(_allowed_explorer_path(str(p.parent)))
        except HTTPException:
            parent = ""
    return {
        "current": str(p),
        "items": entries[safe_offset:safe_offset + safe_limit],
        "parent": parent,
        "total": len(entries),
        "offset": safe_offset,
        "limit": safe_limit,
    }

def _to_resp(task) -> TaskResponse:
    return TaskResponse(
        id=task.id, task_type=task.task_type.value,
        request_method=task.request_method,
        source_page_url=task.source_page_url, mime_type=task.mime_type,
        title=task.title, url=task.url,
        referer=task.referer, origin=task.origin,
        user_agent=task.user_agent, cookie="",
        filename=task.filename, download_dir=str(task.engine_state.get("output_dir") or settings.download_dir), concurrency=task.concurrency,
        status=task.status.value, stage=task.stage, last_log=task.last_log,
        total_segments=task.progress.total_segments,
        completed_segments=task.progress.completed_segments,
        failed_segments=task.progress.failed_segments,
        downloaded_bytes=task.progress.downloaded_bytes,
        total_bytes=task.progress.total_bytes,
        speed_bytes_per_sec=task.progress.speed_bytes_per_sec,
        eta_seconds=task.progress.eta_seconds,
        active_workers=task.progress.active_workers,
        max_workers=task.progress.max_workers,
        reconnect_count=task.progress.reconnect_count,
        connection_status=task.progress.connection_status,
        last_worker_error=task.progress.last_worker_error,
        post_percent=task.progress.post_percent,
        active_slots=task.progress.active_slots,
        active_segment_indexes=task.progress.active_segment_indexes,
        playable_segments=task.progress.playable_segments,
        playable_duration=task.progress.playable_duration,
        media_duration=task.progress.media_duration,
        progress_percent=(
            task.progress.progress_percent
            or (
                task.progress.completed_segments * 100 / task.progress.total_segments
                if task.progress.total_segments
                else 0.0
            )
        ),
        uploaded_bytes=task.progress.uploaded_bytes,
        upload_speed_bytes_per_sec=task.progress.upload_speed_bytes_per_sec,
        peer_count=task.progress.peer_count,
        seed_count=task.progress.seed_count,
        playback_ready=manager.playback_ready(task),
        speed_limit_kib=task.speed_limit_kib,
        speed_history=speed_history_payload(task),
        speed_peak_bytes_per_sec=speed_peak_payload(task),
        connection_parts=connection_parts_payload(task),
        av_scan=dict(task.engine_state.get("av_scan") or {}),
        is_live=bool(task.engine_state.get("live")),
        error_message=task.error_message,
        error_code=task.error_code,
        error_stage=task.error_stage,
        error_url=task.error_url,
        error_hint=task.error_hint,
        http_status=task.http_status,
        error_attempt=task.error_attempt,
        output_path=task.output_path,
        expected_checksum=task.expected_checksum,
        checksum_algorithm=task.checksum_algorithm,
        checksum_actual=task.checksum_actual,
        checksum_verified=task.checksum_verified,
        output_is_file=task_output_is_file(task),
        output_missing=task_output_missing(task),
        file_access_token=(
            issue_file_access_token(task.id)
            if task.status is TaskStatus.DONE and task_output_is_file(task) and not task_output_missing(task)
            else ""
        ),
        created_at=task.created_at or "",
        updated_at=task.updated_at or "",
        started_at=task.started_at or "",
        finished_at=task.finished_at or "",
        scheduled_start_at=str(task.engine_state.get("scheduled_start_at") or ""),
        scheduled_stop_at=str(task.engine_state.get("scheduled_stop_at") or ""),
        completion_action=str(task.engine_state.get("completion_action") or "none"),
        mirrors=list(task.engine_state.get("mirrors") or []),
        mirror_status=list(task.engine_state.get("mirror_status") or []),
        available_actions=manager.get_available_actions(task),
        queue_position=manager.get_queue_position(task),
    )

