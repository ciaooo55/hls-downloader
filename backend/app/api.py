import asyncio
import json
import re
import threading
from pathlib import Path
from fastapi import APIRouter, HTTPException, Header, Request, UploadFile, File, Form
from fastapi.responses import FileResponse, PlainTextResponse, StreamingResponse, Response
from .schemas import (
    HealthResponse,
    SettingsUpdate,
    TaskBatchCreate,
    TaskCreate,
    TaskResponse,
    UrlRecognitionRequest,
    UserscriptPing,
    PlaybackSeekRequest,
    TorrentFileSelection,
    BrowserHandoffAccept,
)
from .config import apply_settings_update, settings, save_settings
from .downloader.task_manager import (
    TaskConflictError,
    TaskNotFoundError,
    manager,
    task_output_is_file,
)
from .downloader.playback import (
    PlaybackError,
    PlaybackNotReadyError,
    PlaybackSessionError,
    playback_service,
)
from .utils import get_domain
from .userscript_monitor import userscript_monitor
from .desktop_runtime import activate_window, present_browser_handoff, request_shutdown
from .url_recognition import RecognitionError, recognize_url
from .updater import UpdateError, update_service
from .models import TaskStatus, TaskType
from .browser_handoff import browser_handoffs

router = APIRouter(prefix="/api")

def _check_token(x_token: str = Header(default="")):
    if x_token != settings.token:
        raise HTTPException(status_code=401, detail="Invalid token")


def _check_playback_token(x_token: str = "", token: str = ""):
    """Allow native HLS clients to carry the local token in the media URL."""
    _check_token(x_token or token)

def _check_host(url: str):
    if url.lower().startswith("magnet:") or url.lower().startswith("torrent-file:"):
        return
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


@router.post("/browser/handoffs")
async def create_browser_handoff(request: Request, x_token: str = Header(default="")):
    _check_token(x_token)
    payload = await request.json()
    url = str(payload.get("url", ""))
    if not url.startswith(("http://", "https://", "magnet:")):
        raise HTTPException(status_code=422, detail="浏览器资源地址无效")
    _check_host(url)
    item = browser_handoffs.create(payload)
    presentation = present_browser_handoff(item.id)
    mode = str(presentation.get("mode") or "none")
    if mode == "desktop-pending":
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
    body = item.public()
    body["presentation_mode"] = mode
    body["presentation_ok"] = bool(presentation.get("ok"))
    body["presentation_queued"] = bool(presentation.get("queued"))
    return body


@router.post("/browser/ping")
async def browser_extension_ping(request: Request, x_token: str = Header(default="")):
    _check_token(x_token)
    payload = await request.json()
    browser_handoffs.record_ping(str(payload.get("version", "")))
    return {"ok": True}


@router.get("/browser/status")
async def browser_extension_status(x_token: str = Header(default="")):
    _check_token(x_token)
    return browser_handoffs.status()


@router.get("/browser/handoffs")
async def list_browser_handoffs(x_token: str = Header(default="")):
    _check_token(x_token)
    return browser_handoffs.pending()


@router.get("/browser/handoffs/{handoff_id}")
async def get_browser_handoff(handoff_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    item = browser_handoffs.get(handoff_id)
    if not item:
        raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
    return item.public()


@router.post("/browser/handoffs/{handoff_id}/accept")
async def accept_browser_handoff(handoff_id: str, body: BrowserHandoffAccept | None = None, x_token: str = Header(default="")):
    _check_token(x_token)
    body = body or BrowserHandoffAccept()
    item = browser_handoffs.claim(handoff_id)
    if not item:
        existing = browser_handoffs.get(handoff_id)
        if not existing:
            raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
        raise HTTPException(status_code=409, detail=f"接管请求当前状态为 {existing.status}")
    output_dir = Path(body.download_dir or settings.browser_category_dirs.get(body.category) or settings.download_dir).expanduser().resolve()
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
    _check_token(x_token)
    deadline = asyncio.get_running_loop().time() + browser_handoffs.ttl + 2
    while True:
        item = browser_handoffs.get(handoff_id)
        if not item:
            raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
        if item.status != "pending" or asyncio.get_running_loop().time() >= deadline:
            return item.public()
        await asyncio.sleep(0.25)


async def _create_browser_task(item, output_dir: str = ""):
    task = await manager.create_task(
        url=item.url,
        task_type=TaskType.AUTO,
        source_page_url=item.source_page_url,
        mime_type=item.mime_type,
        referer=item.referer,
        origin=item.origin,
        user_agent=item.user_agent,
        cookie=item.cookie,
        filename=item.filename,
        output_dir=output_dir,
        auto_start=True,
    )
    return task


@router.post("/browser/downloads")
async def create_browser_download(request: Request, x_token: str = Header(default="")):
    _check_token(x_token)
    payload = await request.json()
    url = str(payload.get("url", ""))
    if not url.startswith(("http://", "https://", "magnet:")):
        raise HTTPException(status_code=422, detail="浏览器资源地址无效")
    _check_host(url)
    item = browser_handoffs.create(payload)
    task = await _create_browser_task(item)
    item.status = "accepted"
    item.task_id = task.id
    activate_window()
    return _to_resp(task)


@router.post("/browser/handoffs/{handoff_id}/reject")
async def reject_browser_handoff(handoff_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    item = browser_handoffs.reject(handoff_id)
    if not item:
        raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
    return item.public()


@router.post("/browser/handoffs/{handoff_id}/cancel")
async def cancel_browser_handoff(handoff_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    item = browser_handoffs.cancel(handoff_id)
    if not item:
        raise HTTPException(status_code=404, detail="接管请求不存在或已过期")
    return item.public()


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
    return settings.model_dump()

@router.post("/tasks", response_model=TaskResponse)
async def create_task(body: TaskCreate, x_token: str = Header(default="")):
    _check_token(x_token)
    _check_host(body.url)
    task = await manager.create_task(
        url=body.url, task_type=body.task_type,
        source_page_url=body.source_page_url, mime_type=body.mime_type,
        referer=body.referer, origin=body.origin,
        user_agent=body.user_agent, cookie=body.cookie,
        title=body.title, filename=body.filename,
        concurrency=body.concurrency,
        output_dir=body.download_dir,
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
            url=t.url, task_type=t.task_type,
            source_page_url=t.source_page_url, mime_type=t.mime_type,
            referer=t.referer, origin=t.origin,
            user_agent=t.user_agent, cookie=t.cookie,
            title=t.title, filename=t.filename,
            concurrency=t.concurrency,
            output_dir=t.download_dir,
            auto_start=True,
        )
        results.append(_to_resp(task))
    return results


@router.post("/tasks/torrent-file", response_model=TaskResponse)
async def create_torrent_file_task(
    file: UploadFile = File(...),
    title: str = Form(default=""),
    x_token: str = Header(default=""),
):
    _check_token(x_token)
    name = file.filename or "download.torrent"
    if not name.lower().endswith(".torrent"):
        raise HTTPException(status_code=400, detail="只接受 .torrent 文件")
    content = await file.read(16 * 1024 * 1024 + 1)
    if not content or len(content) > 16 * 1024 * 1024:
        raise HTTPException(status_code=400, detail="种子文件为空或超过 16 MiB")
    task = await manager.create_task(
        url=f"torrent-file:{name}",
        task_type=TaskType.TORRENT,
        title=title or Path(name).stem,
        filename=Path(name).stem,
        auto_start=False,
    )
    task_dir = Path(settings.download_dir) / ".tasks" / task.id
    task_dir.mkdir(parents=True, exist_ok=True)
    source = task_dir / "uploaded.torrent"
    source.write_bytes(content)
    task.engine_state["torrent_path"] = str(source)
    await manager._save_db(task)
    await manager.start_task(task.id)
    return _to_resp(task)


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
    _check_playback_token(x_token, token)
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


def _playback_task(task_id: str):
    task = manager.tasks.get(task_id)
    if task is None:
        raise HTTPException(status_code=404, detail="Task not found")
    return task


def _raise_playback_error(exc: PlaybackError):
    if isinstance(exc, PlaybackSessionError):
        raise HTTPException(status_code=410, detail=str(exc)) from exc
    if isinstance(exc, PlaybackNotReadyError):
        raise HTTPException(status_code=425, detail=str(exc)) from exc
    raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/tasks/{task_id}/playback")
async def open_task_playback(task_id: str, x_token: str = Header(default="")):
    _check_token(x_token)
    task = _playback_task(task_id)
    if task.task_type is not TaskType.HLS:
        try:
            _, size = manager.get_stream_info(task.id)
        except TaskConflictError as exc:
            raise HTTPException(status_code=425, detail=str(exc)) from exc
        session_id = playback_service.open_session(task.id)
        return {
            "session_id": session_id,
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
    return {"session_id": session_id, **snapshot.to_dict()}


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
        if task.task_type is not TaskType.HLS:
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
    _check_playback_token(x_token, token)
    task = _playback_task(task_id)
    try:
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
    _check_playback_token(x_token, token)
    _playback_task(task_id)
    try:
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
    _check_playback_token(x_token, token)
    _playback_task(task_id)
    try:
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
    _check_token(x_token or token)
    task = _playback_task(task_id)
    try:
        playback_service.touch(task_id, session)
    except PlaybackError as exc:
        _raise_playback_error(exc)
    if task.task_type is not TaskType.HLS:
        try:
            path, total = manager.get_stream_info(task_id)
        except TaskConflictError as exc:
            raise HTTPException(status_code=425, detail=str(exc)) from exc
        range_header = request.headers.get("range", "")
        match = re.match(r"^bytes=(\d*)-(\d*)$", range_header, re.IGNORECASE)
        if not match:
            start, end = 0, min(total - 1, 2 * 1024 * 1024 - 1)
        else:
            start_text, end_text = match.groups()
            if not start_text and not end_text:
                raise HTTPException(status_code=416, detail="无效的 Range")
            if start_text:
                start = int(start_text)
                end = int(end_text) if end_text else min(total - 1, start + 4 * 1024 * 1024 - 1)
            else:
                length = min(int(end_text), 4 * 1024 * 1024)
                start, end = max(0, total - length), total - 1
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

@router.post("/launch-file")
async def launch_file(body: dict, x_token: str = Header(default="")):
    _check_token(x_token)
    path = body.get("path", "")
    if not path:
        raise HTTPException(status_code=400, detail="path required")
    target = Path(path)
    if not target.exists() or not target.is_file():
        raise HTTPException(status_code=404, detail="file not found")
    import os
    if not hasattr(os, "startfile"):
        raise HTTPException(status_code=501, detail="当前系统不支持直接打开文件")
    await asyncio.to_thread(os.startfile, str(target))
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
        id=task.id, task_type=task.task_type.value,
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
        playback_ready=manager._playback_ready(task),
        error_message=task.error_message,
        error_code=task.error_code,
        error_stage=task.error_stage,
        error_url=task.error_url,
        error_hint=task.error_hint,
        http_status=task.http_status,
        error_attempt=task.error_attempt,
        output_path=task.output_path,
        output_is_file=task_output_is_file(task),
        created_at=task.created_at or "",
        updated_at=task.updated_at or "",
        started_at=task.started_at or "",
        finished_at=task.finished_at or "",
        available_actions=manager.get_available_actions(task),
        queue_position=manager.get_queue_position(task),
    )


