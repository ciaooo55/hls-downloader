"""Framing and bounded pipe reads shared by Native Host acceptance tools."""
from __future__ import annotations

import json
import struct
import threading


def frame(message: dict[str, object]) -> bytes:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return struct.pack("<I", len(payload)) + payload


def read_exact(stream: object, size: int, timeout: float) -> bytes:
    result: list[bytes] = []
    failure: list[BaseException] = []

    def read() -> None:
        try:
            result.append(stream.read(size))  # type: ignore[attr-defined]
        except BaseException as error:  # Reader errors are reported on the test thread.
            failure.append(error)

    worker = threading.Thread(target=read, daemon=True)
    worker.start()
    worker.join(timeout)
    if worker.is_alive():
        raise TimeoutError("Native Host did not return a framed response in time")
    if failure:
        raise failure[0]
    return result[0]
