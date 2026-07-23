from collections import deque
from collections.abc import Callable
import logging
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


def present_browser_handoff(handoff_id: str) -> dict:
    """Present one browser handoff without serializing it behind other dialogs.

    Returns a presentation report so callers can distinguish:
    - desktop: presenter is live and a show call was scheduled
    - desktop-pending: desktop session is starting and the offer was queued
    - ui-fallback: no desktop shell; browser manager UI must show the offer
    """
    handoff_id = str(handoff_id).strip()
    if not handoff_id:
        return {"ok": False, "presented": False, "queued": False, "mode": "none"}

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
