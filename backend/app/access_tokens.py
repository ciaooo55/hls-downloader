from __future__ import annotations

import hashlib
import hmac
import time

from .config import settings


FILE_ACCESS_TTL_SECONDS = 15 * 60
DESKTOP_ACCESS_TTL_SECONDS = 10 * 60
BROWSER_ACCESS_TTL_SECONDS = 24 * 60 * 60


def _issue_scoped_token(scope: str, ttl: int, now: int | None = None) -> str:
    issued_at = int(time.time() if now is None else now)
    expires = issued_at + ttl
    message = f"scope:{scope}:{expires}".encode("utf-8")
    signature = hmac.new(settings.token.encode("utf-8"), message, hashlib.sha256).hexdigest()
    return f"{scope}.{expires}.{signature}"


def _verify_scoped_token(scope: str, token: str, ttl: int, now: int | None = None) -> bool:
    try:
        token_scope, expires_text, signature = str(token or "").split(".", 2)
        expires = int(expires_text)
    except (TypeError, ValueError):
        return False
    current = int(time.time() if now is None else now)
    if token_scope != scope or expires < current or expires > current + ttl + 60:
        return False
    message = f"scope:{scope}:{expires}".encode("utf-8")
    expected = hmac.new(settings.token.encode("utf-8"), message, hashlib.sha256).hexdigest()
    return hmac.compare_digest(signature, expected)


def issue_desktop_access_token(now: int | None = None) -> str:
    return _issue_scoped_token("desktop", DESKTOP_ACCESS_TTL_SECONDS, now)


def verify_desktop_access_token(token: str, now: int | None = None) -> bool:
    return _verify_scoped_token("desktop", token, DESKTOP_ACCESS_TTL_SECONDS, now)


def issue_browser_access_token(now: int | None = None) -> str:
    return _issue_scoped_token("browser", BROWSER_ACCESS_TTL_SECONDS, now)


def verify_browser_access_token(token: str, now: int | None = None) -> bool:
    return _verify_scoped_token("browser", token, BROWSER_ACCESS_TTL_SECONDS, now)


def issue_file_access_token(task_id: str, now: int | None = None) -> str:
    issued_at = int(time.time() if now is None else now)
    expires = issued_at + FILE_ACCESS_TTL_SECONDS
    message = f"file:{task_id}:{expires}".encode("utf-8")
    signature = hmac.new(settings.token.encode("utf-8"), message, hashlib.sha256).hexdigest()
    return f"{expires}.{signature}"


def verify_file_access_token(task_id: str, token: str, now: int | None = None) -> bool:
    try:
        expires_text, signature = str(token or "").split(".", 1)
        expires = int(expires_text)
    except (TypeError, ValueError):
        return False
    current = int(time.time() if now is None else now)
    if expires < current or expires > current + FILE_ACCESS_TTL_SECONDS + 60:
        return False
    message = f"file:{task_id}:{expires}".encode("utf-8")
    expected = hmac.new(settings.token.encode("utf-8"), message, hashlib.sha256).hexdigest()
    return hmac.compare_digest(signature, expected)
