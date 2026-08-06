from fastapi.testclient import TestClient
import pytest

from backend.app import api, legal
from backend.app.config import Settings
from backend.app.main import app


def _blank_settings() -> Settings:
    return Settings(
        legal_terms_accepted_version="",
        legal_terms_accepted_digest="",
        legal_terms_accepted_at="",
    )


def test_legal_digest_binds_terms_and_privacy_documents():
    terms = legal.read_terms_document()
    privacy = legal.read_privacy_document()
    digest = legal.terms_digest(terms, privacy)

    assert len(terms) > 1000
    assert len(privacy) > 1000
    assert len(digest) == 64
    assert digest != legal.terms_digest(terms, privacy + "\nchanged")


def test_acceptance_requires_one_explicit_agreement():
    current = _blank_settings()
    persisted = []
    digest = legal.terms_digest()

    with pytest.raises(ValueError, match="不同意"):
        legal.record_legal_acceptance(
            version=legal.TERMS_VERSION,
            digest=digest,
            accepted=False,
            current=current,
            persist=persisted.append,
        )

    status = legal.record_legal_acceptance(
        version=legal.TERMS_VERSION,
        digest=digest,
        accepted=True,
        current=current,
        persist=persisted.append,
    )

    assert status["accepted"] is True
    assert current.legal_terms_accepted_version == legal.TERMS_VERSION
    assert current.legal_terms_accepted_digest == digest
    assert current.legal_terms_accepted_at.endswith("Z")
    assert persisted == [current]


def test_changed_document_invalidates_existing_acceptance():
    current = _blank_settings()
    current.legal_terms_accepted_version = legal.TERMS_VERSION
    current.legal_terms_accepted_digest = legal.terms_digest()
    current.legal_terms_accepted_at = "2026-08-06T00:00:00Z"

    assert legal.legal_acceptance_current(current)
    assert not legal.legal_acceptance_current(
        current,
        legal.read_terms_document() + "\nchanged",
        legal.read_privacy_document(),
    )


def test_api_blocks_transfer_until_local_acceptance(monkeypatch):
    current = legal.settings
    current.legal_terms_accepted_version = ""
    current.legal_terms_accepted_digest = ""
    current.legal_terms_accepted_at = ""
    auth = {"X-Token": api.settings.token}

    def accept_without_writing_config(**kwargs):
        return legal.record_legal_acceptance(
            **kwargs,
            current=current,
            persist=lambda _settings: None,
        )

    monkeypatch.setattr(api, "record_legal_acceptance", accept_without_writing_config)
    with TestClient(app) as client:
        status = client.get("/api/legal/status", headers=auth)
        blocked = client.post(
            "/api/tasks",
            headers=auth,
            json={"url": "https://example.test/file.bin", "task_type": "auto"},
        )
        terms = client.get("/api/legal/terms", headers=auth).json()
        accepted = client.post(
            "/api/legal/accept",
            headers=auth,
            json={
                "version": terms["required_version"],
                "document_digest": terms["document_digest"],
                "accepted": True,
            },
        )

    assert status.status_code == 200
    assert status.json()["accepted"] is False
    assert blocked.status_code == 428
    assert blocked.json()["detail"]["code"] == "LEGAL_TERMS_REQUIRED"
    assert accepted.status_code == 200
    assert accepted.json()["accepted"] is True
    api._require_legal_acceptance()
