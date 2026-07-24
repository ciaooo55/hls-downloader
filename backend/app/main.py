import uvicorn
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, FileResponse
from contextlib import asynccontextmanager

from .config import PROJECT_ROOT, settings
from .api import router
from .downloader.task_manager import manager
from .downloader.throttle import download_throttle
from .updater import cleanup_update_cache

@asynccontextmanager
async def lifespan(app: FastAPI):
    cleanup_update_cache()
    download_throttle.configure(getattr(settings, "download_speed_limit_kib", 0) or 0)
    await manager.load_from_db()
    manager.start_maintenance()
    try:
        await manager.cleanup_orphan_temp_dirs()
    except Exception:
        pass
    try:
        yield
    finally:
        await manager.shutdown()

app = FastAPI(title="HLS Downloader", lifespan=lifespan)
app.add_middleware(
    CORSMiddleware,
    allow_origin_regex=r"^(tauri://localhost|https?://tauri\.localhost|https?://(localhost|127\.0\.0\.1)(:\d+)?)$",
    allow_credentials=False,
    allow_methods=["*"],
    allow_headers=["*"],
)
app.include_router(router)

UI_DIST = PROJECT_ROOT / "frontend" / "dist"
UI_RESPONSE_HEADERS = {
    "Cache-Control": "no-store, no-cache, must-revalidate, max-age=0",
    "Pragma": "no-cache",
    "Expires": "0",
}


@app.get("/help")
async def serve_help():
    return HTMLResponse(
        f"""<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>HLS Downloader 使用教程</title>
  <style>
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; color: #17202a; background: #f3f5f7; font: 16px/1.65 system-ui, sans-serif; }}
    main {{ width: min(760px, calc(100% - 32px)); margin: 48px auto; }}
    h1 {{ margin: 0 0 8px; font-size: 30px; letter-spacing: 0; }}
    h2 {{ margin: 28px 0 10px; font-size: 19px; letter-spacing: 0; }}
    p {{ margin: 8px 0; }}
    .status {{ border-left: 4px solid #16845b; background: #fff; padding: 16px 18px; }}
    .status strong {{ display: block; font-size: 18px; }}
    .actions {{ display: flex; flex-wrap: wrap; gap: 10px; margin: 18px 0; }}
    a {{ color: #075ca8; }}
    .button {{ display: inline-block; padding: 9px 14px; border-radius: 6px; color: #fff; background: #1267a8; text-decoration: none; }}
    ol {{ padding-left: 24px; }}
    code {{ padding: 2px 5px; background: #e7ebef; border-radius: 4px; }}
    .note {{ color: #59636e; font-size: 14px; }}
  </style>
</head>
<body>
  <main>
    <h1>HLS Downloader 使用教程</h1>
    <p>下载器已经启动。请安装 Chrome/Edge 或 Firefox 浏览器插件，再打开需要下载的网页。</p>
    <div class="actions">
      <a class="button" href="/ui">打开下载管理器</a>
    </div>
    <div class="status">
      <strong>仅使用正式浏览器插件</strong>
      <span>安装包内含 Chromium 扩展目录；Firefox 插件包请从 GitHub Release 下载。</span>
    </div>
    <h2>使用步骤</h2>
    <ol>
      <li>在桌面端“浏览器集成”中打开 Chromium 扩展目录并加载插件，或安装经过 Mozilla 签名的 Firefox 版本。</li>
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
    idx = UI_DIST / "index.html"
    if idx.exists():
        return FileResponse(idx, headers=UI_RESPONSE_HEADERS)
    return HTMLResponse("<h2>Frontend not built</h2><p>Run: cd frontend && npm run build</p>", status_code=404)

@app.get("/ui/{full_path:path}")
async def serve_ui_files(full_path: str):
    file = UI_DIST / full_path
    if file.exists() and file.is_file():
        return FileResponse(file, headers=UI_RESPONSE_HEADERS)
    # SPA fallback: return index.html for unknown routes
    idx = UI_DIST / "index.html"
    if idx.exists():
        return FileResponse(idx, headers=UI_RESPONSE_HEADERS)
    return HTMLResponse("Not found", status_code=404)

@app.get("/")
async def root():
    return {"message": "HLS Downloader", "ui": "/ui", "docs": "/docs"}

if __name__ == "__main__":
    uvicorn.run("app.main:app", host=settings.host, port=settings.port)
