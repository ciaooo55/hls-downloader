"""Deterministic framing regressions; no product process or TCP listener."""

from __future__ import annotations

from collections import deque
import json
import struct
import unittest
from unittest.mock import patch

import v7_core_ipc as ipc


class FragmentedSocket:
    def __init__(self, payload: bytes) -> None:
        self.received = deque(payload)
        self.sent = bytearray()

    def recv(self, _size: int) -> bytes:
        return bytes([self.received.popleft()]) if self.received else b""

    def sendall(self, data: bytes) -> None:
        self.sent.extend(data)


class CoreIpcTest(unittest.TestCase):
    def test_fragmented_response_and_unicode_request(self) -> None:
        body = json.dumps({"type": "snapshot", "tasks": []}).encode()
        stream = FragmentedSocket(struct.pack("<I", len(body)) + body)
        request = {"filename": "中文.bin"}
        self.assertEqual(ipc.send_frame(stream, request), {"type": "snapshot", "tasks": []})
        length = struct.unpack("<I", stream.sent[:4])[0]
        self.assertEqual(length, len(stream.sent) - 4)
        self.assertEqual(json.loads(stream.sent[4:]), request)

    def test_rejects_invalid_frame_lengths_without_reading_body(self) -> None:
        for size in (0, ipc.MAX_FRAME_BYTES + 1, 0xFFFFFFFF):
            with self.subTest(size=size):
                with self.assertRaisesRegex(ValueError, "length"):
                    ipc.send_frame(FragmentedSocket(struct.pack("<I", size)), {})

    def test_rejects_truncated_frames(self) -> None:
        for response in (b"\x01\x00", struct.pack("<I", 2) + b"{"):
            with self.subTest(response=response):
                with self.assertRaisesRegex(RuntimeError, "closed"):
                    ipc.send_frame(FragmentedSocket(response), {})

    def test_core_errors_remain_available_to_acceptance_callers(self) -> None:
        expected = {"type": "error", "code": "expected_failure"}
        body = json.dumps(expected).encode()
        self.assertEqual(ipc.send_frame(FragmentedSocket(struct.pack("<I", len(body)) + body), {}), expected)

    def test_rejects_non_object_and_oversized_request(self) -> None:
        stream = FragmentedSocket(struct.pack("<I", 2) + b"[]")
        with self.assertRaisesRegex(RuntimeError, "not an object"):
            ipc.send_frame(stream, {})
        stream = FragmentedSocket(b"")
        with self.assertRaisesRegex(ValueError, "request"):
            ipc.send_frame(stream, {"data": "x" * ipc.MAX_FRAME_BYTES})
        self.assertEqual(stream.sent, b"")

    def test_failed_handshake_closes_connection(self) -> None:
        from unittest.mock import Mock

        stream = Mock()
        with patch.object(ipc.socket, "create_connection", return_value=stream), patch.object(
            ipc, "send_frame", side_effect=ValueError("invalid frame")
        ):
            with self.assertRaises(ValueError):
                ipc.connect(1234)
        stream.close.assert_called_once()

    def test_task_queries_select_id_and_fail_on_missing_task(self) -> None:
        task = {"task_id": "wanted", "status": "paused"}
        with patch.object(ipc, "send_frame", return_value={"tasks": [{"task_id": "other"}, task]}):
            self.assertEqual(ipc.wait_task(None, "wanted", lambda value: value["status"] == "paused", 12), task)
            with self.assertRaisesRegex(RuntimeError, "absent"):
                ipc.snapshot(None, "missing", 13)


if __name__ == "__main__":
    unittest.main()
