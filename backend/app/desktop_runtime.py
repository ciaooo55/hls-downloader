from collections import deque
from collections.abc import Callable
import logging
import os
import threading


_activation_callback: Callable[[], None] | None = None
_shutdown_callback: Callable[[], bool | None] | None = None
_handoff_callback: Callable[[str], None] | None = None
_activation_lock = threading.Lock()
_activation_running = False
_activation_generation = 0
_handoff_lock = threading.Lock()
_pending_handoffs: deque[str] = deque()
_pending_handoff_ids: set[str] = set()
_pending_native_offers: deque[tuple[str, dict | None]] = deque()
_pending_native_ids: set[str] = set()
_desktop_session_active = False
logger = logging.getLogger(__name__)


def register_activation(callback: Callable[[], None] | None) -> None:
    global _activation_callback, _activation_generation, _activation_running
    with _activation_lock:
        _activation_callback = callback
        # A replaced desktop presenter must not remain suppressed by an older
        # callback that is still unwinding on another thread.
        _activation_generation += 1
        _activation_running = False


def activate_window() -> bool:
    global _activation_running
    with _activation_lock:
        callback = _activation_callback
        generation = _activation_generation
        if callback is None:
            return False
        if _activation_running:
            return True
        _activation_running = True

    def run() -> None:
        global _activation_running
        try:
            callback()
        finally:
            with _activation_lock:
                if generation == _activation_generation:
                    _activation_running = False

    threading.Thread(target=run, name="desktop-activate", daemon=True).start()
    return True


def _mark_presentation_failed(handoff_id: str, error: str) -> None:
    try:
        from .browser_handoff import browser_handoffs

        browser_handoffs.mark_presentation(handoff_id, "failed", error)
    except Exception:
        logger.exception("failed to mark browser handoff presentation failure %s", handoff_id)


def _run_handoff_callback(callback: Callable[[str], None], handoff_id: str) -> None:
    try:
        callback(handoff_id)
    except Exception:
        # The caller cannot surface UI-thread failures to the browser process.
        # Leaving the handoff pending still lets it expire safely.
        logger.exception("failed to present browser handoff %s", handoff_id)
        _mark_presentation_failed(handoff_id, "desktop presenter raised")


def set_desktop_handoff_session(active: bool) -> None:
    """Mark whether the desktop shell is alive and can own handoff windows."""
    global _desktop_session_active, _handoff_callback
    with _handoff_lock:
        _desktop_session_active = bool(active)
        if not active:
            _handoff_callback = None
            _pending_handoffs.clear()
            _pending_handoff_ids.clear()


def register_browser_handoff(callback: Callable[[str], None] | None) -> None:
    """Register the desktop presenter and flush handoffs received during startup."""
    global _handoff_callback, _desktop_session_active
    with _handoff_lock:
        _handoff_callback = callback
        if callback is not None:
            _desktop_session_active = True
            pending = list(_pending_handoffs)
            _pending_handoffs.clear()
            _pending_handoff_ids.clear()
        else:
            pending = []

    for handoff_id in pending:
        threading.Thread(
            target=_run_handoff_callback,
            args=(callback, handoff_id),
            name=f"desktop-handoff-{handoff_id[:8]}",
            daemon=True,
        ).start()


def native_shell_expected() -> bool:
    """True when a click must wait for HLSNativeShell instead of a WebView fallback."""
    try:
        from .native_shell import is_native_shell_ready, native_shell_was_closed
    except Exception:
        return False
    if is_native_shell_ready():
        return True
    if native_shell_was_closed():
        # Tray exit cleared resident HWNDs. Leftover HLS_STARTED_BY_NATIVE_SHELL
        # must not keep offers in native-shell-pending unless a respawn is queued.
        return has_pending_native_handoffs()
    flag = os.environ.get("HLS_STARTED_BY_NATIVE_SHELL", "").strip().lower()
    if flag in {"1", "true", "yes", "on"}:
        return True
    native_flag = os.environ.get("HLS_NATIVE_SHELL", "").strip().lower()
    if native_flag in {"1", "true", "yes", "on"}:
        return True
    return False


def has_pending_native_handoffs() -> bool:
    with _handoff_lock:
        return bool(_pending_native_ids)


def clear_pending_native_handoffs() -> None:
    with _handoff_lock:
        _pending_native_offers.clear()
        _pending_native_ids.clear()


def _queue_native_handoff(handoff_id: str, snapshot: dict | None) -> None:
    with _handoff_lock:
        if handoff_id in _pending_native_ids:
            return
        _pending_native_offers.append((handoff_id, dict(snapshot) if isinstance(snapshot, dict) else None))
        _pending_native_ids.add(handoff_id)


def _unqueue_native_handoff(handoff_id: str) -> None:
    with _handoff_lock:
        _pending_native_ids.discard(handoff_id)
        remaining = [item for item in _pending_native_offers if item[0] != handoff_id]
        _pending_native_offers.clear()
        _pending_native_offers.extend(remaining)


def flush_pending_native_handoffs() -> None:
    """Paint queued confirmations after the supervisor POSTs boot."""
    with _handoff_lock:
        pending = list(_pending_native_offers)
        _pending_native_offers.clear()
        _pending_native_ids.clear()
    for handoff_id, snapshot in pending:
        if _present_via_native_shell(handoff_id, snapshot) is None:
            logger.warning("native shell still not ready for queued handoff %s", handoff_id)


def _ensure_native_shell_process(*, force: bool = False):
    try:
        from .config import settings
        from .native_shell import is_native_shell_ready, maybe_spawn_native_shell_process

        if is_native_shell_ready():
            return None
        return maybe_spawn_native_shell_process(
            core_url=f"http://127.0.0.1:{int(settings.port)}/api",
            token=str(settings.token or ""),
            force=force,
        )
    except Exception:
        logger.exception("failed to start native shell for a browser confirmation")
        return None


def _present_via_native_shell(handoff_id: str, snapshot: dict | None) -> dict | None:
    """Prefer the resident supervisor when its confirmation window is already warm."""
    try:
        from .native_shell import is_native_shell_ready, native_shell_supervisor, paint_snapshot
    except Exception:
        return None
    if not is_native_shell_ready():
        return None
    source = dict(snapshot) if isinstance(snapshot, dict) else {}
    if not source.get("id") or source.get("id") != handoff_id:
        source["id"] = handoff_id
    if len(source) <= 1:
        try:
            from .browser_handoff import browser_handoffs

            item = browser_handoffs.get(handoff_id)
            if item is not None:
                source = item.public()
                source["id"] = handoff_id
        except Exception:
            logger.exception("failed to load browser handoff snapshot %s", handoff_id)
    try:
        event = native_shell_supervisor().offer(source)
    except (RuntimeError, ValueError):
        return None
    except Exception:
        logger.exception("native shell offer failed for %s", handoff_id)
        return None
    return {
        "ok": True,
        "presented": True,
        "queued": False,
        "mode": "native-shell",
        "presentable": True,
        "snapshot": event.get("snapshot") or paint_snapshot(source),
    }


def present_browser_handoff(handoff_id: str, snapshot: dict | None = None) -> dict:
    """Present one browser handoff without serializing it behind other dialogs.

    Returns a presentation report so callers can distinguish:
    - native-shell: resident supervisor painted a pre-created confirmation window
    - native-shell-pending: supervisor is expected and the offer waits for boot
    - desktop: presenter is live and a show call was scheduled
    - desktop-pending: desktop session is starting and the offer was queued
    - ui-fallback: no desktop shell; browser manager UI must show the offer
    """
    handoff_id = str(handoff_id).strip()
    if not handoff_id:
        return {"ok": False, "presented": False, "queued": False, "mode": "none"}

    native = _present_via_native_shell(handoff_id, snapshot)
    if native is not None:
        return native

    if native_shell_expected():
        _queue_native_handoff(handoff_id, snapshot)
        _ensure_native_shell_process()
        return {
            "ok": True,
            "presented": False,
            "queued": True,
            "mode": "native-shell-pending",
            "presentable": False,
        }

    try:
        from .native_shell import native_shell_was_closed

        closed = native_shell_was_closed()
    except Exception:
        closed = False
    if closed:
        _queue_native_handoff(handoff_id, snapshot)
        spawned = _ensure_native_shell_process(force=True)
        if spawned is not None:
            return {
                "ok": True,
                "presented": False,
                "queued": True,
                "mode": "native-shell-pending",
                "presentable": False,
            }
        _unqueue_native_handoff(handoff_id)

    with _handoff_lock:
        callback = _handoff_callback
        desktop_session = _desktop_session_active
        if callback is None:
            if desktop_session:
                if handoff_id not in _pending_handoff_ids:
                    _pending_handoffs.append(handoff_id)
                    _pending_handoff_ids.add(handoff_id)
                return {
                    "ok": True,
                    "presented": False,
                    "queued": True,
                    "mode": "desktop-pending",
                }
            return {
                "ok": True,
                "presented": False,
                "queued": False,
                "mode": "ui-fallback",
            }

    threading.Thread(
        target=_run_handoff_callback,
        args=(callback, handoff_id),
        name=f"desktop-handoff-{handoff_id[:8]}",
        daemon=True,
    ).start()
    return {"ok": True, "presented": False, "queued": False, "mode": "desktop"}


def has_browser_handoff_presenter() -> bool:
    with _handoff_lock:
        return _handoff_callback is not None


def is_desktop_handoff_session() -> bool:
    with _handoff_lock:
        return _desktop_session_active


def register_shutdown(callback: Callable[[], bool | None] | None) -> None:
    global _shutdown_callback
    _shutdown_callback = callback


def request_shutdown() -> bool:
    callback = _shutdown_callback
    if callback is None:
        return False
    return callback() is not False
