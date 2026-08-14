import sys
from pathlib import Path, PurePosixPath, PureWindowsPath
from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, FileResponse, JSONResponse, RedirectResponse
from contextlib import asynccontextmanager
import os

import uvicorn

from .config import PROJECT_ROOT, settings
from .api import router
from .downloader.task_manager import manager
from .database import close_database, initialize_database
from .downloader.throttle import download_throttle, effective_download_speed_limit_kib
from .updater import cleanup_update_cache
from .legal import legal_acceptance_current

@asynccontextmanager
async def lifespan(app: FastAPI):
    cleanup_update_cache()
    download_throttle.configure(effective_download_speed_limit_kib())
    await initialize_database()
    await manager.load_from_db(auto_start_allowed=legal_acceptance_current())
    manager.start_maintenance()
    try:
        await manager.cleanup_orphan_temp_dirs()
    except Exception:
        pass
    if os.environ.get("HLS_NATIVE_SHELL", "").strip().lower() in {"1", "true", "yes", "on"}:
        from .native_shell import boot_native_shell

        boot_native_shell()
    else:
        from .native_shell import maybe_spawn_native_shell_process

        maybe_spawn_native_shell_process(
            core_url=f"http://127.0.0.1:{settings.port}/api",
            token=settings.token,
            project_root=PROJECT_ROOT,
        )
    try:
        yield
    finally:
        from .native_shell import reset_native_shell

        reset_native_shell()
        await manager.shutdown()
        await close_database()

app = FastAPI(title="HLS Downloader", lifespan=lifespan)
MAX_JSON_BODY_BYTES = 4 * 1024 * 1024
MAX_TORRENT_MULTIPART_BODY_BYTES = 17 * 1024 * 1024


@app.middleware("http")
async def limit_json_request_body(request: Request, call_next):
    content_type = request.headers.get("content-type", "").split(";", 1)[0].strip().lower()
    if content_type == "application/json":
        content_length = request.headers.get("content-length", "").strip()
        if content_length:
            try:
                if int(content_length) > MAX_JSON_BODY_BYTES:
                    return JSONResponse(status_code=413, content={"detail": "JSON 请求体过大"})
            except ValueError:
                return JSONResponse(status_code=400, content={"detail": "Content-Length 无效"})
        body = await request.body()
        if len(body) > MAX_JSON_BODY_BYTES:
            return JSONResponse(status_code=413, content={"detail": "JSON 请求体过大"})
    # UploadFile parsing happens inside FastAPI after this middleware. A
    # chunked multipart request has no trustworthy Content-Length, so checking
    # only in the endpoint would allow the parser to consume an unbounded body
    # first. Buffer only this bounded endpoint's request before handing it to
    # FastAPI; this is at most 17 MiB and keeps the error a clear 413 even when
    # the client uses chunked transfer encoding.
    if request.url.path.rstrip("/") == "/api/tasks/torrent-file" and content_type == "multipart/form-data":
        received = bytearray()
        async for chunk in request.stream():
            received.extend(chunk)
            if len(received) > MAX_TORRENT_MULTIPART_BODY_BYTES:
                return JSONResponse(status_code=413, content={"detail": "种子上传请求过大"})
        request._body = bytes(received)
    return await call_next(request)
CHROMIUM_EXTENSION_ORIGIN = "chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla"
ALLOWED_CORS_ORIGINS = [
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    f"http://127.0.0.1:{settings.port}",
    f"http://localhost:{settings.port}",
    CHROMIUM_EXTENSION_ORIGIN,
]
if not getattr(sys, "frozen", False):
    ALLOWED_CORS_ORIGINS.extend([
        "http://127.0.0.1:1420",
        "http://localhost:1420",
    ])
app.add_middleware(
    CORSMiddleware,
    # WebExtensions with loopback host_permissions do not require CORS. Firefox
    # moz-extension origins contain a per-profile UUID rather than the signed
    # Gecko add-on ID, so accepting arbitrary UUIDs would accept every local
    # Firefox extension. Keep browser CORS limited to the stable release ID.
    allow_origins=ALLOWED_CORS_ORIGINS,
    allow_credentials=False,
    allow_methods=["GET", "POST", "PATCH", "DELETE", "OPTIONS"],
    allow_headers=["Content-Type", "X-Token"],
)
app.include_router(router)

UI_DIST = PROJECT_ROOT / "frontend" / "dist"
UI_RESPONSE_HEADERS = {
    "Cache-Control": "no-store, no-cache, must-revalidate, max-age=0",
    "Pragma": "no-cache",
    "Expires": "0",
    "Content-Security-Policy": "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:",
    "Permissions-Policy": "camera=(), microphone=(), geolocation=()",
    "Referrer-Policy": "no-referrer",
    "X-Content-Type-Options": "nosniff",
    "X-Frame-Options": "DENY",
}


def _resolve_ui_file(full_path: str) -> Path | None:
    """Resolve a URL path without permitting Windows or POSIX path escape."""
    if "\x00" in full_path:
        return None
    if not full_path:
        return UI_DIST.resolve()
    windows_path = PureWindowsPath(full_path)
    posix_path = PurePosixPath(full_path)
    if (
        windows_path.is_absolute()
        or windows_path.drive
        or windows_path.root
        or posix_path.is_absolute()
        or ".." in windows_path.parts
        or ".." in posix_path.parts
    ):
        return None
    root = UI_DIST.resolve()
    candidate = (root / Path(*posix_path.parts)).resolve()
    try:
        candidate.relative_to(root)
    except ValueError:
        return None
    return candidate


def _ui_index_file() -> Path | None:
    """Resolve the SPA entrypoint through the same containment check as assets."""
    return _resolve_ui_file("index.html")


@app.get("/help")
async def serve_help():
    return HTMLResponse(
        """<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>HLS Downloader 使用教程</title>
  <style>
    * { box-sizing: border-box; }
    body { margin: 0; color: #17202a; background: #f3f5f7; font: 16px/1.65 system-ui, sans-serif; }
    main { width: min(760px, calc(100% - 32px)); margin: 48px auto; }
    h1 { margin: 0 0 8px; font-size: 30px; letter-spacing: 0; }
    h2 { margin: 28px 0 10px; font-size: 19px; letter-spacing: 0; }
    p { margin: 8px 0; }
    .status { border-left: 4px solid #16845b; background: #fff; padding: 16px 18px; }
    .status strong { display: block; font-size: 18px; }
    .actions { display: flex; flex-wrap: wrap; gap: 10px; margin: 18px 0; }
    a { color: #075ca8; }
    .button { display: inline-block; padding: 9px 14px; border-radius: 6px; color: #fff; background: #1267a8; text-decoration: none; }
    ol { padding-left: 24px; }
    code { padding: 2px 5px; background: #e7ebef; border-radius: 4px; }
    .note { color: #59636e; font-size: 14px; }
  </style>
</head>
<body>
  <main>
    <h1>HLS Downloader 使用教程</h1>
    <p>下载器已经启动。请安装 Chrome/Edge 或 Firefox 浏览器插件，再打开需要下载的网页。</p>
    <div class="actions">
      <a class="button" href="/ui/">打开下载管理器</a>
    </div>
    <div class="status">
      <strong>仅使用正式浏览器插件</strong>
      <span>安装包内含 Chromium 扩展目录；Firefox 请从 <a href="https://addons.mozilla.org/zh-CN/firefox/addon/hls_downloader/" target="_blank" rel="noopener">Firefox Add-ons 官方页面</a> 安装。</span>
    </div>
    <h2>使用步骤</h2>
    <ol>
      <li>在桌面端“浏览器集成”中打开 Chromium 扩展目录并加载插件，或从 Firefox Add-ons 官方页面安装经过 Mozilla 签名的版本。</li>
      <li>打开网页并播放媒体，插件会在当前页面显示捕获结果。</li>
      <li>点击资源或真实下载链接，确认后交给桌面端下载。</li>
      <li>点击下载后，可回到下载管理器查看分片和合并进度。</li>
    </ol>
    <p class="note">插件只重放浏览器实际捕获且适合重放的请求身份。Cookie 需要按站点授权，未捕获来源的 Cookie/Authorization 不会跨域发送。</p>
  </main>
</body>
</html>"""
    )

@app.get("/ui")
async def serve_ui_root():
    idx = _ui_index_file()
    if idx is not None and idx.is_file():
        return RedirectResponse(url="/ui/", status_code=307)
    return HTMLResponse("<h2>Frontend not built</h2><p>Run: cd frontend && npm run build</p>", status_code=404)

@app.get("/ui/{full_path:path}")
async def serve_ui_files(full_path: str):
    file = _resolve_ui_file(full_path)
    if file is None:
        return HTMLResponse("Not found", status_code=404)
    if file.exists() and file.is_file():
        return FileResponse(file, headers=UI_RESPONSE_HEADERS)
    # SPA fallback: return index.html for unknown routes
    idx = _ui_index_file()
    if idx is not None and idx.is_file():
        return FileResponse(idx, headers=UI_RESPONSE_HEADERS)
    return HTMLResponse("Not found", status_code=404)

@app.get("/")
async def root():
    return {"message": "HLS Downloader", "ui": "/ui", "docs": "/docs"}

if __name__ == "__main__":
    uvicorn.run("app.main:app", host=settings.host, port=settings.port)
