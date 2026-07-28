import hashlib

from backend.app import config


def test_known_public_tokens_are_stored_as_digests_only():
    assert config._is_leaked_token("55555")
    assert not config._is_leaked_token("an-installation-local-token")
    assert all(len(value) == 64 for value in config._LEAKED_TOKEN_HASHES)
    assert hashlib.sha256(b"55555").hexdigest() in config._LEAKED_TOKEN_HASHES


def test_default_config_version_includes_legacy_token_rotation():
    assert config.Settings().config_version >= 16
