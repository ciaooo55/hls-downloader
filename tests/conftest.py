import pytest

from backend.app import legal
from backend.app.native_shell import reset_native_shell


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


@pytest.fixture(autouse=True)
def reset_resident_native_shell():
    """Native-shell boot is sticky process state; never leak it across tests."""
    reset_native_shell()
    yield
    reset_native_shell()
