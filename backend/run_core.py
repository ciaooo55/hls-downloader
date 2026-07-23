from __future__ import annotations

import threading

import uvicorn

try:
    from .app.config import settings
    from .app.main import app
    from .app.native_desktop import register_core_shutdown
except ImportError:
    from app.config import settings
    from app.main import app
    from app.native_desktop import register_core_shutdown


def main() -> int:
    config = uvicorn.Config(
        app,
        host=settings.host,
        port=settings.port,
        log_level="warning",
        access_log=False,
        use_colors=False,
    )
    server = uvicorn.Server(config)

    def stop() -> None:
        def request() -> None:
            server.should_exit = True

        threading.Timer(0.1, request).start()

    register_core_shutdown(stop)
    try:
        server.run()
    finally:
        register_core_shutdown(None)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
