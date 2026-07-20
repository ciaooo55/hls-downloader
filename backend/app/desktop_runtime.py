from collections.abc import Callable
import threading


_activation_callback: Callable[[], None] | None = None
_shutdown_callback: Callable[[], bool | None] | None = None
_activation_lock = threading.Lock()
_activation_running = False


def register_activation(callback: Callable[[], None] | None) -> None:
    global _activation_callback
    _activation_callback = callback


def activate_window() -> bool:
    global _activation_running
    callback = _activation_callback
    if callback is None:
        return False
    with _activation_lock:
        if _activation_running:
            return True
        _activation_running = True

    def run() -> None:
        global _activation_running
        try:
            callback()
        finally:
            with _activation_lock:
                _activation_running = False

    threading.Thread(target=run, name="desktop-activate", daemon=True).start()
    return True


def register_shutdown(callback: Callable[[], bool | None] | None) -> None:
    global _shutdown_callback
    _shutdown_callback = callback


def request_shutdown() -> bool:
    callback = _shutdown_callback
    if callback is None:
        return False
    return callback() is not False
