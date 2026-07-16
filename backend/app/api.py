import asyncio
import json
import threading
from pathlib import Path
from fastapi import APIRouter, HTTPException, Header, Request
from fastapi.responses import StreamingResponse
from .schemas import (
    HealthResponse,
    SettingsUpdate,
    TaskBatchCreate,
    TaskCreate,
    TaskResponse,
    UrlRecognitionRequest,
    UserscriptPing,
)
from .config import apply_settings_update, settings, save_settings
from .downloader.task_manager import (
    TaskConflictError,
    TaskNotFoundError,
    manager,
)
from .utils import get_domain
from .userscript_monitor import userscript_monitor
from .desktop_runtime import activate_window, request_shutdown
from .url_recognition import RecognitionError, recognize_url
from .updater import UpdateError, update_service

router = APIRouter(prefix="/api")

def _check_token(x_token: str = Header(default="")):
    if x_token != settings.token:
        raise HTTPException(status_code=401, detail="Invalid token")

def _check_host(url: str):
    if settings.allowed_hosts:
        domain = get_domain(url)
        if domain not in settings.allowed_hosts:
            raise HTTPException(status_code=403, detail=f"Host {domain} not in allowed_hosts")


async def _manager_action(awaitable):
    try:
        await awaitable
    except TaskNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except TaskConflictError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc

@router.get("/health", response_model=HealthResponse)
async def health():
    return HealthResponse()


@router.post("/app/activate")
async def activate_desktop_app(x_token: str = Header(default="")):
    _check_token(x_token)
    return {"ok": activate_window()}


@router.post("/app/shutdown")
async def shutdown_desktop_app(x_token: str = Header(default="")):
    _check_token(x_token)
    return {"ok": request_shutdown()}


@router.get("/update/check")
async def check_update(force: bool = False, x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        info = await asyncio.to_thread(update_service.check, force=force)
    except UpdateError as exc:
        raise HTTPException(status_code=503, detail=str(exc)) from exc
    return info.to_dict()


@router.post("/update/install")
async def install_update(x_token: str = Header(default="")):
    _check_token(x_token)
    try:
        info = await asyncio.to_thread(update_service.download_and_launch)
    except UpdateError as exc:
        status = 409 if any(
            marker in str(exc) for marker in ("重复", "正在下载", "已经启动")
        ) else 400
        raise HTTPException(status_code=status, detail=str(exc)) from exc
    timer = threading.Timer(0.75, request_shutdown)
    timer.daemon = True
    timer.start()
    return {"ok": True, "version": info.latest_version}


@router.post("/recognize")
async def recognize_input_url(body: UrlRecognitionRequest, x_token: str = Header(default="")):
    _check_token(x_token)
    _check_host(body.url)
    headers = {
        "User-Agent": body.user_agent or settings.default_user_agent,
    }
    if body.referer:
        headers["Referer"] = body.referer
    if body.origin:
        headers["Origin"] = body.origin
    if body.cookie:
        headers["Cookie"] = body.cookie
    try:
        return await recognize_url(body.url, headers=headers)
    except RecognitionError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/userscript/ping")
async def userscript_ping(body: UserscriptPing, x_token: str = Header(default="")):
    _check_token(x_token)
    userscript_monitor.record(version=body.version, page_url=body.page_url)
    return {"ok": True}


@router.get("/userscript/status")
async def userscript_status(x_token: str = Header(default="")):
    _check_token(x_token)
    return userscript_monitor.snapshot()

@router.get("/test")
async def test_connection(x_token: str = Header(default="")):
    import shutil
    results = {"health": True}
    results["token_valid"] = (x_token == settings.token)
    ffmpeg_found = shutil.which(settings.ffmpeg_path) is not None
    if not ffmpeg_found:
        ffmpeg_found = Path(settings.ffmpeg_path).exists()
    results["ffmpeg"] = ffmpeg_found
    results["ffmpeg_path"] = settings.ffmpeg_path
    results["download_dir"] = settings.download_dir
    results["concurrency"] = settings.default_concurrency
    results["max_tasks"] = settings.max_concurrent_tasks
    return results

@router.get("/settings")
async def get_settings(x_token: str = Header(default="")):
    _check_token(x_token)
    return settings.model_dump()

@router.post("/settings")
async def update_settings(body: SettingsUpdate, x_token: str = Header(default="")):
    _check_token(x_token)
    data = body.model_dump(exclude_none=True)
    apply_settings_update(settings, data)
    save_settings(settings)
    return {"ok": True}

@router.post("/tasks", response_model=TaskResponse)
async def create_task(body: TaskCreate, x_token: str = Header(default="")):
    _check_token(x_token)
    _check_host(body.url)
    task = await manager.create_task(
        url=body.url, referer=body.referer, origin=body.origin,
        user_agent=body.user_agent, cookie=body.cookie,
        title=body.title, filename=body.filename,
        concurrency=body.concurrency,
        auto_start=True,
    )
    return _to_resp(task)

@router.post("/tasks/batch")
async def create_batch(body: TaskBatchCreate, x_token: str = Header(default="")):
    _check_token(x_token)
    for task in body.tasks:
        _check_host(task.url)
    results = []
    for t in body.tasks:
        task = await manager.create_task(
            url=t.url, referer=t.referer, origin=t.origin,
            user_agent=t.user_agent, cookie=t.cookie,
            title=t.title, filename=t.filename,
            concurrency=t.concurrency,
            auto_start=True,
        )
        results.append(_to_resp(task))
    return results

@router.get("/tasks", response_model=list[TaskResponse])
async def list_tasks(x_token: str = Header(default="")):
    _check_token(x_token)
    return [_to_resp(t) for t in manager.tasks.values()]

@router.get("/tasks/{task_id}", response_model=TaskResponse)
async def get_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    task = manager.tasks.get(task_id)
    if not task:
        raise HTTPException(status_code=404, detail="Task not found")
    return _to_resp(task)

@router.post("/tasks/{task_id}/start")
async def start_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
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
    await _manager_action(manager.retry_task(task_id))
    return {"ok": True}

@router.delete("/tasks/{task_id}")
async def delete_task(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    await _manager_action(manager.delete_task(task_id))
    return {"ok": True}

@router.get("/tasks/{task_id}/log")
async def get_task_log(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    task = manager.tasks.get(task_id)
    if not task:
        raise HTTPException(status_code=404, detail="Task not found")
    log_file = Path(settings.download_dir) / ".tasks" / task_id / "download.log"
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

@router.get("/events")
async def events(request: Request):
    tok = request.query_params.get("x_token") or request.query_params.get("token") or request.headers.get("x-token", "")
    if tok != settings.token:
        raise HTTPException(status_code=401, detail="Invalid token")
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

@router.get("/sniffed")
async def get_sniffed(x_token: str = Header(default="")):
    _check_token(x_token)
    return manager._sniffed

@router.post("/sniffed")
async def add_sniffed(body: dict, x_token: str = Header(default="")):
    _check_token(x_token)
    manager._sniffed.append(body)
    return {"ok": True}

@router.post("/open-explorer")
async def open_explorer(body: dict, x_token: str = Header(default="")):
    _check_token(x_token)
    path = body.get("path", "")
    if not path:
        raise HTTPException(status_code=400, detail="path required")
    import subprocess
    p = Path(path)
    if not p.exists():
        raise HTTPException(status_code=404, detail="path not found")
    if p.is_file():
        subprocess.Popen(["explorer", "/select,", str(p)])
    else:
        subprocess.Popen(["explorer", str(p)])
    return {"ok": True}

@router.get("/browse-dir")
async def browse_dir(path: str = "", x_token: str = Header(default="")):
    _check_token(x_token)
    if not path:
        # Default to drives on Windows
        import string
        drives = []
        for letter in string.ascii_uppercase:
            drive = f"{letter}:\\"
            if Path(drive).exists():
                drives.append({"name": drive, "path": drive, "is_dir": True})
        return {"current": "", "items": drives, "parent": ""}

    p = Path(path)
    if not p.exists() or not p.is_dir():
        raise HTTPException(status_code=404, detail="directory not found")

    items = []
    try:
        for child in sorted(p.iterdir(), key=lambda x: (not x.is_dir(), x.name.lower())):
            try:
                items.append({
                    "name": child.name,
                    "path": str(child),
                    "is_dir": child.is_dir(),
                })
            except (PermissionError, OSError):
                pass
    except PermissionError:
        pass

    parent = str(p.parent) if str(p.parent) != str(p) else ""
    return {"current": str(p), "items": items, "parent": parent}

def _to_resp(task) -> TaskResponse:
    return TaskResponse(
        id=task.id, title=task.title, url=task.url,
        referer=task.referer, origin=task.origin,
        user_agent=task.user_agent, cookie=task.cookie,
        filename=task.filename, concurrency=task.concurrency,
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
        error_message=task.error_message,
        error_code=task.error_code,
        error_stage=task.error_stage,
        error_url=task.error_url,
        error_hint=task.error_hint,
        http_status=task.http_status,
        error_attempt=task.error_attempt,
        output_path=task.output_path,
        created_at=task.created_at or "",
        updated_at=task.updated_at or "",
        started_at=task.started_at or "",
        finished_at=task.finished_at or "",
    )


