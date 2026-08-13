from backend.app.models import TaskType
from backend.app.site_profiles import resolve_site_profile


def test_empty_and_disabled_profiles_are_noop(monkeypatch):
    monkeypatch.setattr("backend.app.site_profiles.settings", type("S", (), {"site_profiles": []})())
    assert resolve_site_profile("https://cdn.example.test/a.bin") == {}

    monkeypatch.setattr("backend.app.site_profiles.settings", type("S", (), {"site_profiles": [{
        "host": "*.example.test",
        "enabled": False,
        "cookie": "sid=1",
        "download_dir": "D:\\site",
        "concurrency": 8,
    }]})())
    assert resolve_site_profile("https://cdn.example.test/a.bin") == {}


def test_first_matching_host_returns_cookie_and_directory(monkeypatch):
    monkeypatch.setattr("backend.app.site_profiles.settings", type("S", (), {"site_profiles": [
        {"host": "other.test", "cookie": "skip=1"},
        {
            "host": "*.example.test",
            "cookie": "sid=abc",
            "download_dir": "D:\\Videos\\Example",
            "referer": "https://example.test/",
            "concurrency": 4,
            "speed_limit_kib": 256,
        },
        {"host": "*", "cookie": "too-broad=1"},
    ]})())
    profile = resolve_site_profile("https://cdn.example.test/file.bin")
    assert profile["host"] == "*.example.test"
    assert profile["cookie"] == "sid=abc"
    assert profile["download_dir"] == "D:\\Videos\\Example"
    assert profile["referer"] == "https://example.test/"
    assert profile["concurrency"] == 4
    assert profile["speed_limit_kib"] == 256


def test_non_http_or_invalid_url_has_no_profile(monkeypatch):
    monkeypatch.setattr("backend.app.site_profiles.settings", type("S", (), {"site_profiles": [
        {"host": "*", "cookie": "x=1"},
    ]})())
    assert resolve_site_profile("not a url") == {}
    assert resolve_site_profile("") == {}
    assert TaskType.HTTP.value == "http"

def test_profile_from_task_keeps_cookie_and_directory():
    class Fake:
        url = "https://CDN.Example.test/a.bin?token=1"
        user_agent = "UA"
        referer = "https://example.test/watch"
        origin = "https://example.test"
        cookie = "sid=secret"
        request_headers = {"X-Token": "abc"}
        concurrency = 6
        speed_limit_kib = 128
        engine_state = {"output_dir": "D:/Videos/Example"}

    from backend.app.site_profiles import site_profile_from_task
    profile = site_profile_from_task(Fake())
    assert profile["host"] == "cdn.example.test"
    assert profile["cookie"] == "sid=secret"
    assert profile["download_dir"] == "D:/Videos/Example"
    assert profile["concurrency"] == 6
    assert profile["referer"] == "https://example.test/watch"


def test_upsert_replaces_same_host_and_prepends_new_host():
    from backend.app.site_profiles import upsert_site_profile
    first, action = upsert_site_profile([], {"host": "a.test", "cookie": "1"})
    assert action == "created"
    assert first[0]["host"] == "a.test"
    updated, action = upsert_site_profile(first, {"host": "A.test", "cookie": "2"})
    assert action == "updated"
    assert updated == [{"host": "a.test", "cookie": "2"}]
    two, action = upsert_site_profile(updated, {"host": "b.test", "cookie": "3"})
    assert action == "created"
    assert [item["host"] for item in two] == ["b.test", "a.test"]


def test_magnet_task_has_no_site_host():
    from backend.app.site_profiles import site_host_from_url, site_profile_from_task
    assert site_host_from_url("magnet:?xt=urn:btih:abc") == ""
    class Fake:
        url = "magnet:?xt=urn:btih:abc"
        engine_state = {}
    try:
        site_profile_from_task(Fake())
    except ValueError:
        return
    raise AssertionError("expected ValueError")


def test_site_proxy_is_opt_in_and_manual_keeps_url(monkeypatch):
    monkeypatch.setattr("backend.app.site_profiles.settings", type("S", (), {"site_profiles": [
        {"host": "*.example.test", "cookie": "sid=1", "proxy_mode": "direct"},
    ]})())
    profile = resolve_site_profile("https://cdn.example.test/a.bin")
    assert profile["proxy_mode"] == "direct"
    assert profile["proxy_url"] == ""

    monkeypatch.setattr("backend.app.site_profiles.settings", type("S", (), {"site_profiles": [
        {"host": "cdn.example.test", "proxy_mode": "manual", "proxy_url": "socks5://127.0.0.1:1080"},
    ]})())
    profile = resolve_site_profile("https://cdn.example.test/a.bin")
    assert profile["proxy_mode"] == "manual"
    assert profile["proxy_url"] == "socks5://127.0.0.1:1080"

    monkeypatch.setattr("backend.app.site_profiles.settings", type("S", (), {"site_profiles": [
        {"host": "cdn.example.test", "proxy_mode": "inherit", "proxy_url": "socks5://127.0.0.1:1080"},
    ]})())
    profile = resolve_site_profile("https://cdn.example.test/a.bin")
    assert profile["proxy_mode"] == ""
    assert profile["proxy_url"] == ""


def test_save_from_task_does_not_invent_proxy():
    class Fake:
        url = "https://cdn.example.test/a.bin"
        user_agent = ""
        referer = ""
        origin = ""
        cookie = ""
        request_headers = {}
        concurrency = 0
        speed_limit_kib = 0
        engine_state = {}

    from backend.app.site_profiles import site_profile_from_task
    profile = site_profile_from_task(Fake())
    assert profile["proxy_mode"] == ""
    assert profile["proxy_url"] == ""
