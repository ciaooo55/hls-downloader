import time

from backend.app.browser_handoff import BrowserHandoffService
from backend.app.credentials import PREFIX, protect_secret, unprotect_secret


def test_browser_handoff_confirmation_and_expiry():
    service = BrowserHandoffService(ttl=0.01)
    item = service.create({"url": "https://cdn.test/file.zip", "cookie": "session=secret", "size": 42})
    assert item.status == "pending"
    assert "cookie" not in item.public()
    assert service.pending()[0]["id"] == item.id
    assert service.reject(item.id).status == "rejected"

    expired = service.create({"url": "https://cdn.test/old.zip"})
    expired.created_at = time.time() - 0.02
    assert service.get(expired.id).status == "expired"


def test_task_cookie_uses_dpapi_on_windows():
    protected = protect_secret("session=secret")
    assert unprotect_secret(protected) == "session=secret"
    if protected != "session=secret":
        assert protected.startswith(PREFIX)
