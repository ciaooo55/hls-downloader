import pytest

from backend.app import legal


@pytest.fixture(autouse=True)
def accept_current_legal_terms_for_existing_tests():
    """Keep legacy endpoint tests focused; legal-gate tests opt out explicitly."""
    current = legal.settings
    previous = (
        current.legal_terms_accepted_version,
        current.legal_terms_accepted_digest,
        current.legal_terms_accepted_at,
    )
    current.legal_terms_accepted_version = legal.TERMS_VERSION
    current.legal_terms_accepted_digest = legal.terms_digest()
    current.legal_terms_accepted_at = "2026-08-06T00:00:00Z"
    try:
        yield
    finally:
        (
            current.legal_terms_accepted_version,
            current.legal_terms_accepted_digest,
            current.legal_terms_accepted_at,
        ) = previous
