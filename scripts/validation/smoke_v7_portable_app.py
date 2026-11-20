"""Shared Windows named-pipe and runtime-process helpers for acceptance tests."""
from __future__ import annotations

import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import struct
import time

import psutil


class Pipe:
    def __init__(self, pipe_name=None):
        pipe_name = pipe_name or os.environ.get('HLS_V7_PIPE') or r"\\.\pipe\HLSDownloader.v7"
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                           wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        self.kernel.CreateFileW.restype = wintypes.HANDLE
        for name in ("ReadFile", "WriteFile"):
            getattr(self.kernel, name).argtypes = [wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
                                                   ctypes.POINTER(wintypes.DWORD), wintypes.LPVOID]
        self.kernel.PeekNamedPipe.argtypes = [wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
                                             wintypes.LPVOID, ctypes.POINTER(wintypes.DWORD), wintypes.LPVOID]
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel.WaitNamedPipeW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD]
        deadline = time.monotonic() + 2
        while True:
            self.handle = self.kernel.CreateFileW(pipe_name, 0xc0000000, 0, None, 3, 0, None)
            if self.handle != ctypes.c_void_p(-1).value:
                break
            error = ctypes.get_last_error()
            if error != 231 or time.monotonic() >= deadline:
                raise ctypes.WinError(error)
            remaining_ms = max(1, int((deadline - time.monotonic()) * 1000))
            if not self.kernel.WaitNamedPipeW(pipe_name, remaining_ms):
                raise ctypes.WinError(ctypes.get_last_error())

    def read(self, size):
        data = bytearray()
        deadline = time.monotonic() + 10
        while len(data) < size:
            available = wintypes.DWORD()
            if not self.kernel.PeekNamedPipe(self.handle, None, 0, None, ctypes.byref(available), None):
                raise ctypes.WinError(ctypes.get_last_error())
            if not available.value:
                if time.monotonic() >= deadline:
                    raise TimeoutError("Core named-pipe response exceeded 10 seconds")
                time.sleep(.01)
                continue
            buffer = ctypes.create_string_buffer(min(size-len(data), available.value))
            read = wintypes.DWORD()
            if not self.kernel.ReadFile(self.handle, buffer, len(buffer), ctypes.byref(read), None):
                raise ctypes.WinError(ctypes.get_last_error())
            if not read.value:
                raise EOFError("Core closed its pipe")
            data.extend(buffer.raw[:read.value])
        return bytes(data)

    def request(self, message):
        payload = json.dumps(message, separators=(",", ":")).encode()
        framed = struct.pack("<I", len(payload)) + payload
        sent = wintypes.DWORD()
        buffer = ctypes.create_string_buffer(framed)
        if not self.kernel.WriteFile(self.handle, buffer, len(framed), ctypes.byref(sent), None):
            raise ctypes.WinError(ctypes.get_last_error())
        if sent.value != len(framed):
            raise RuntimeError("Core pipe write was truncated")
        size = struct.unpack("<I", self.read(4))[0]
        if not 0 < size <= 4*1024*1024:
            raise ValueError("Core response frame is invalid")
        return json.loads(self.read(size))

    def close(self):
        self.kernel.CloseHandle(self.handle)


def runtime_processes(root):
    result = []
    for process in psutil.process_iter(["pid", "name", "exe"]):
        path = process.info.get("exe")
        if path and Path(path).is_relative_to(root) and process.info["name"].startswith("HLSDownloader"):
            result.append(process.info)
    return result


def wait(predicate, description, timeout=60):
    deadline = time.monotonic()+timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.1)
    raise TimeoutError(description)
