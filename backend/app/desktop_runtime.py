from collections.abc import Callable


_activation_callback: Callable[[], None] | None = None
_shutdown_callback: Callable[[], bool | None] | None = None


def register_activation(callback: Callable[[], None] | None) -> None:
    global _activation_callback
    _activation_callback = callback


def activate_window() -> bool:
    callback = _activation_callback
    if callback is None:
        return False
    callback()
    return True


def register_shutdown(callback: Callable[[], bool | None] | None) -> None:
    global _shutdown_callback
    _shutdown_callback = callback


def request_shutdown() -> bool:
    callback = _shutdown_callback
    if callback is None:
        return False
    return callback() is not False
