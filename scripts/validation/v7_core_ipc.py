"""Bounded Core TCP helpers for validation tools, without fixture side effects."""

from __future__ import annotations

import json
import socket
import struct
import time


MAX_FRAME_BYTES = 4 * 1024 * 1024


def _read_exact(stream: socket.socket, size: int) -> bytes:
    result = bytearray()
    while len(result) < size:
        chunk = stream.recv(size - len(result))
        if not chunk:
            raise RuntimeError("Core IPC closed before the response completed")
        result.extend(chunk)
    return bytes(result)


def send_frame(stream: socket.socket, message: dict[str, object]) -> dict[str, object]:
    """Exchange one frame; leave Core error objects for the caller to inspect."""
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    if not 0 < len(payload) <= MAX_FRAME_BYTES:
        raise ValueError("Core IPC request exceeds the frame limit")
    stream.sendall(struct.pack("<I", len(payload)) + payload)
    size = struct.unpack("<I", _read_exact(stream, 4))[0]
    if not 0 < size <= MAX_FRAME_BYTES:
        raise ValueError("Core IPC response has an invalid frame length")
    response = json.loads(_read_exact(stream, size).decode("utf-8"))
    if not isinstance(response, dict):
        raise RuntimeError("Core IPC response was not an object")
    return response


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def connect(port: int) -> socket.socket:
    """Wait up to 15 seconds for a Core handshake, closing failed attempts."""
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        stream = None
        try:
            stream = socket.create_connection(("127.0.0.1", port), timeout=1)
            stream.settimeout(10)
            hello = send_frame(stream, {"type": "hello", "protocol": "hls-downloader-v7-core", "version": 1})
            if hello.get("type") == "hello":
                return stream
        except OSError:
            time.sleep(0.05)
        except BaseException:
            if stream is not None:
                stream.close()
            raise
        if stream is not None:
            stream.close()
    raise TimeoutError("Engine TCP IPC did not become ready")


def snapshot(stream: socket.socket, task_id: str, request_id: int) -> dict[str, object]:
    response = send_frame(stream, {"type": "snapshot", "request_id": request_id})
    task = next((item for item in response.get("tasks", []) if item.get("task_id") == task_id), None)
    if not task:
        raise RuntimeError(f"task is absent from snapshot: {response}")
    return task


def wait_task(stream: socket.socket, task_id: str, predicate, request_id: int) -> dict[str, object]:
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        task = snapshot(stream, task_id, request_id)
        request_id += 1
        if predicate(task):
            return task
        time.sleep(0.05)
    raise TimeoutError(f"task state deadline expired: {task}")
