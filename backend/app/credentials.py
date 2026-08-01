from __future__ import annotations

import base64
import binascii
import ctypes
import os
from ctypes import wintypes


PREFIX = "dpapi:"
SECRET_MASK = "••••••••"


class DATA_BLOB(ctypes.Structure):
    _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_byte))]


def _blob(data: bytes) -> tuple[DATA_BLOB, ctypes.Array]:
    buffer = ctypes.create_string_buffer(data)
    return DATA_BLOB(len(data), ctypes.cast(buffer, ctypes.POINTER(ctypes.c_byte))), buffer


def protect_secret(value: str) -> str:
    if not value or value.startswith(PREFIX) or os.name != "nt":
        return value
    source, source_buffer = _blob(value.encode("utf-8"))
    output = DATA_BLOB()
    if not ctypes.windll.crypt32.CryptProtectData(
        ctypes.byref(source), None, None, None, None, 0x01, ctypes.byref(output)
    ):
        raise ctypes.WinError()
    try:
        encrypted = ctypes.string_at(output.pbData, output.cbData)
        return PREFIX + base64.b64encode(encrypted).decode("ascii")
    finally:
        ctypes.windll.kernel32.LocalFree(output.pbData)
        del source_buffer


def unprotect_secret(value: str) -> str:
    if not value or not value.startswith(PREFIX) or os.name != "nt":
        return value
    encrypted = base64.b64decode(value[len(PREFIX):])
    source, source_buffer = _blob(encrypted)
    output = DATA_BLOB()
    if not ctypes.windll.crypt32.CryptUnprotectData(
        ctypes.byref(source), None, None, None, None, 0x01, ctypes.byref(output)
    ):
        raise ctypes.WinError()
    try:
        return ctypes.string_at(output.pbData, output.cbData).decode("utf-8")
    finally:
        ctypes.windll.kernel32.LocalFree(output.pbData)
        del source_buffer


def is_sensitive_header(name: object) -> bool:
    normalized = str(name or "").strip().lower().replace("_", "-")
    if normalized in {"authorization", "cookie", "proxy-authorization", "x-api-key", "x-token"}:
        return True
    return any(marker in normalized for marker in ("auth", "token", "secret", "api-key"))


def protect_site_profiles(values: object) -> list[dict]:
    result: list[dict] = []
    for raw in values if isinstance(values, list) else []:
        if not isinstance(raw, dict):
            continue
        profile = dict(raw)
        headers = profile.get("request_headers")
        if isinstance(headers, dict):
            profile["request_headers"] = {
                str(name): protect_secret(str(value)) if is_sensitive_header(name) else str(value)
                for name, value in headers.items()
            }
        if "cookie" in profile:
            profile["cookie"] = protect_secret(str(profile.get("cookie") or ""))
        result.append(profile)
    return result


def unprotect_site_profiles(values: object) -> list[dict]:
    result: list[dict] = []
    for raw in values if isinstance(values, list) else []:
        if not isinstance(raw, dict):
            continue
        profile = dict(raw)
        headers = profile.get("request_headers")
        if isinstance(headers, dict):
            decoded: dict[str, str] = {}
            for name, value in headers.items():
                try:
                    decoded[str(name)] = unprotect_secret(str(value))
                except (OSError, UnicodeError, ValueError, TypeError, binascii.Error):
                    decoded[str(name)] = ""
            profile["request_headers"] = decoded
        if "cookie" in profile:
            try:
                profile["cookie"] = unprotect_secret(str(profile.get("cookie") or ""))
            except (OSError, UnicodeError, ValueError, TypeError, binascii.Error):
                profile["cookie"] = ""
        result.append(profile)
    return result


def mask_site_profiles(values: object) -> list[dict]:
    result: list[dict] = []
    for raw in values if isinstance(values, list) else []:
        if not isinstance(raw, dict):
            continue
        profile = dict(raw)
        headers = profile.get("request_headers")
        if isinstance(headers, dict):
            profile["request_headers"] = {
                str(name): SECRET_MASK if value and is_sensitive_header(name) else str(value)
                for name, value in headers.items()
            }
        if profile.get("cookie"):
            profile["cookie"] = SECRET_MASK
        result.append(profile)
    return result


def restore_masked_site_profiles(values: object, previous: object) -> list[dict]:
    previous_profiles = previous if isinstance(previous, list) else []
    old_by_host = {
        str(item.get("host") or "").strip().lower(): item
        for item in previous_profiles if isinstance(item, dict)
    }
    result: list[dict] = []
    for raw in values if isinstance(values, list) else []:
        if not isinstance(raw, dict):
            continue
        profile = dict(raw)
        old = old_by_host.get(str(profile.get("host") or "").strip().lower(), {})
        old_headers = old.get("request_headers") if isinstance(old, dict) else {}
        headers = profile.get("request_headers")
        if isinstance(headers, dict):
            profile["request_headers"] = {
                str(name): (
                    str((old_headers or {}).get(name, ""))
                    if value == SECRET_MASK
                    else str(value)
                )
                for name, value in headers.items()
            }
        if profile.get("cookie") == SECRET_MASK:
            profile["cookie"] = str(old.get("cookie") or "")
        result.append(profile)
    return result
