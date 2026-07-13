from collections.abc import Callable


_activation_callback: Callable[[], None] | None = None


def register_activation(callback: Callable[[], None] | None) -> None:
    global _activation_callback
    _activation_callback = callback


def activate_window() -> bool:
    callback = _activation_callback
    if callback is None:
        return False
    callback()
    return True
