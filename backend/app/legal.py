from __future__ import annotations

import hashlib
from datetime import datetime, timezone
from pathlib import Path

from .config import Settings, save_settings, settings
from .paths import RUNTIME_PATHS


TERMS_VERSION = "2026-08-06-cn-1"
TERMS_PATH = RUNTIME_PATHS.project_root / "TERMS.md"
PRIVACY_PATH = RUNTIME_PATHS.project_root / "PRIVACY.md"


def read_terms_document(path: Path | None = None) -> str:
    document_path = path or TERMS_PATH
    try:
        content = document_path.read_text(encoding="utf-8").strip()
    except OSError as exc:
        raise RuntimeError("用户协议文件缺失，无法确认合规状态") from exc
    if len(content) < 1000:
        raise RuntimeError("用户协议文件不完整，无法确认合规状态")
    return content


def read_privacy_document(path: Path | None = None) -> str:
    document_path = path or PRIVACY_PATH
    try:
        content = document_path.read_text(encoding="utf-8").strip()
    except OSError as exc:
        raise RuntimeError("隐私政策文件缺失，无法确认合规状态") from exc
    if len(content) < 1000:
        raise RuntimeError("隐私政策文件不完整，无法确认合规状态")
    return content


def terms_digest(content: str | None = None, privacy_content: str | None = None) -> str:
    terms = content if content is not None else read_terms_document()
    privacy = privacy_content if privacy_content is not None else read_privacy_document()
    payload = f"{terms}\n\n---HLS-DOWNLOADER-LEGAL-SEPARATOR---\n\n{privacy}"
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def legal_acceptance_current(
    current: Settings | None = None,
    content: str | None = None,
    privacy_content: str | None = None,
) -> bool:
    active = current or settings
    try:
        digest = terms_digest(content, privacy_content)
    except RuntimeError:
        return False
    return bool(
        active.legal_terms_accepted_version == TERMS_VERSION
        and active.legal_terms_accepted_digest == digest
        and active.legal_terms_accepted_at
    )


def legal_status(
    current: Settings | None = None,
    content: str | None = None,
    privacy_content: str | None = None,
) -> dict:
    active = current or settings
    document = content if content is not None else read_terms_document()
    privacy = privacy_content if privacy_content is not None else read_privacy_document()
    digest = terms_digest(document, privacy)
    return {
        "accepted": legal_acceptance_current(active, document, privacy),
        "required_version": TERMS_VERSION,
        "document_digest": digest,
        "accepted_version": active.legal_terms_accepted_version,
        "accepted_at": active.legal_terms_accepted_at,
        "record_location": "local_config",
    }


def terms_payload(current: Settings | None = None) -> dict:
    content = read_terms_document()
    privacy_content = read_privacy_document()
    return {
        **legal_status(current, content, privacy_content),
        "title": "HLS Downloader 用户协议与免责声明（中国大陆版）",
        "content": content,
        "privacy_document": "PRIVACY.md",
        "privacy_content": privacy_content,
    }


def record_legal_acceptance(
    *,
    version: str,
    digest: str,
    accepted: bool,
    current: Settings | None = None,
    persist=save_settings,
) -> dict:
    active = current or settings
    document = read_terms_document()
    privacy = read_privacy_document()
    expected_digest = terms_digest(document, privacy)
    if version != TERMS_VERSION or digest != expected_digest:
        raise ValueError("用户协议已更新，请重新阅读后确认")
    if not accepted:
        raise ValueError("不同意用户协议与隐私政策，软件将退出")
    previous = (
        active.legal_terms_accepted_version,
        active.legal_terms_accepted_digest,
        active.legal_terms_accepted_at,
    )
    active.legal_terms_accepted_version = TERMS_VERSION
    active.legal_terms_accepted_digest = expected_digest
    active.legal_terms_accepted_at = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    try:
        persist(active)
    except Exception:
        (
            active.legal_terms_accepted_version,
            active.legal_terms_accepted_digest,
            active.legal_terms_accepted_at,
        ) = previous
        raise
    return legal_status(active, document, privacy)
