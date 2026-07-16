import uvicorn
from fastapi import FastAPI
from fastapi.responses import HTMLResponse, FileResponse, PlainTextResponse
from contextlib import asynccontextmanager
from html import escape
from pathlib import Path

from .config import PROJECT_ROOT, settings
from .api import router
from .downloader.task_manager import manager
from .userscript_monitor import userscript_monitor
from .userscript_service import render_userscript
from .updater import cleanup_update_cache

@asynccontextmanager
async def lifespan(app: FastAPI):
    cleanup_update_cache()
    await manager.load_from_db()
    try:
        await manager.cleanup_orphan_temp_dirs()
    except Exception:
        pass
    try:
        yield
    finally:
        await manager.shutdown()

app = FastAPI(title="HLS Downloader", lifespan=lifespan)
app.include_router(router)

UI_DIST = PROJECT_ROOT / "frontend" / "dist"
USERSCRIPT_FILE = PROJECT_ROOT / "userscript" / "m3u8-sniffer.user.js"


@app.get("/userscript/m3u8-sniffer.user.js")
async def serve_userscript():
    if not USERSCRIPT_FILE.exists():
        return HTMLResponse("Bundled userscript not found", status_code=404)
    host = settings.host if settings.host not in {"0.0.0.0", "::"} else "127.0.0.1"
    source = USERSCRIPT_FILE.read_text(encoding="utf-8")
    rendered = render_userscript(
        source,
        host=host,
        port=settings.port,
        token=settings.token,
    )
    return PlainTextResponse(
        rendered,
        media_type="application/javascript",
        headers={"Cache-Control": "no-store"},
    )


@app.get("/help")
async def serve_help():
    status = userscript_monitor.snapshot()
    if status.detected:
        state_class = "detected"
        state_title = "已检测到油猴脚本运行"
        version = escape(status.version or "未知")
        page_origin = escape(status.page_origin or "未知页面")
        state_detail = f"版本 {version}，来源 {page_origin}"
    elif status.seen_before:
        state_class = "waiting"
        state_title = "脚本此前运行过，目前未收到报到"
        state_detail = "请打开一个 HTTPS 视频页面，等待几秒后刷新此页。"
    else:
        state_class = "waiting"
        state_title = "本次启动尚未检测到油猴脚本"
        state_detail = "安装脚本后打开一个 HTTPS 视频页面，此页会自动更新。"

    return HTMLResponse(
        f"""<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="refresh" content="5">
  <title>HLS Downloader 使用教程</title>
  <style>
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; color: #17202a; background: #f3f5f7; font: 16px/1.65 system-ui, sans-serif; }}
    main {{ width: min(760px, calc(100% - 32px)); margin: 48px auto; }}
    h1 {{ margin: 0 0 8px; font-size: 30px; letter-spacing: 0; }}
    h2 {{ margin: 28px 0 10px; font-size: 19px; letter-spacing: 0; }}
    p {{ margin: 8px 0; }}
    .status {{ border-left: 4px solid #d49a24; background: #fff; padding: 16px 18px; }}
    .status.detected {{ border-color: #16845b; }}
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
    <p>下载器已经启动。先安装油猴脚本，再打开视频网站。</p>
    <div class="actions">
      <a class="button" href="/userscript/m3u8-sniffer.user.js">安装油猴脚本</a>
      <a class="button" href="/ui">打开下载管理器</a>
    </div>
    <div class="status {state_class}">
      <strong>{state_title}</strong>
      <span>{state_detail}</span>
    </div>
    <h2>使用步骤</h2>
    <ol>
      <li>浏览器先安装 Tampermonkey（油猴）扩展。</li>
      <li>点击上面的“安装油猴脚本”，在油猴页面确认安装。</li>
      <li>打开 HTTPS 视频页面并播放，页面右上角会出现嗅探结果。</li>
      <li>点击下载后，可回到下载管理器查看分片和合并进度。</li>
    </ol>
    <p class="note">受浏览器安全限制，程序不能读取油猴的安装列表；这里显示的是脚本最近是否向本地下载器报到。</p>
  </main>
</body>
</html>"""
    )

@app.get("/ui")
async def serve_ui_root():
    idx = UI_DIST / "index.html"
    if idx.exists():
        return FileResponse(idx)
    return HTMLResponse("<h2>Frontend not built</h2><p>Run: cd frontend && npm run build</p>", status_code=404)

@app.get("/ui/{full_path:path}")
async def serve_ui_files(full_path: str):
    file = UI_DIST / full_path
    if file.exists() and file.is_file():
        return FileResponse(file)
    # SPA fallback: return index.html for unknown routes
    idx = UI_DIST / "index.html"
    if idx.exists():
        return FileResponse(idx)
    return HTMLResponse("Not found", status_code=404)

@app.get("/")
async def root():
    return {"message": "HLS Downloader", "ui": "/ui", "docs": "/docs"}

if __name__ == "__main__":
    uvicorn.run("app.main:app", host=settings.host, port=settings.port)
