from backend.app import request_context
from backend.app.models import Task


def test_request_context_replays_authentication_but_filters_transport_headers(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_referer", "")
    monkeypatch.setattr(request_context.settings, "default_origin", "")
    monkeypatch.setattr(request_context.settings, "default_cookie", "")
    task = Task(
        id="context",
        url="https://cdn.example.test/file.bin",
        request_headers={
            "Authorization": "Bearer signed-token",
            "Sec-CH-UA": '"Chromium";v="140"',
            "X-Playback-Token": "opaque",
            "Host": "wrong.test",
            "Content-Length": "999",
            "Range": "bytes=0-1",
            "Accept-Encoding": "gzip, br",
            "Cookie": "captured=wrong",
        },
        cookie="session=explicit",
    )

    headers = request_context.build_task_headers(task)

    assert headers["authorization"] == "Bearer signed-token"
    assert headers["sec-ch-ua"] == '"Chromium";v="140"'
    assert headers["x-playback-token"] == "opaque"
    assert headers["Cookie"] == "session=explicit"
    lowered = {name.lower() for name in headers}
    assert "host" not in lowered
    assert "content-length" not in lowered
    assert "range" not in lowered
    assert "accept-encoding" not in lowered


def test_manual_download_has_no_unrelated_referer_or_origin(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_referer", "")
    monkeypatch.setattr(request_context.settings, "default_origin", "")
    monkeypatch.setattr(request_context.settings, "default_cookie", "")
    task = Task(id="manual", url="https://example.test/archive.zip")

    headers = request_context.build_task_headers(task)

    lowered = {name.lower() for name in headers}
    assert "referer" not in lowered
    assert "origin" not in lowered
    assert "cookie" not in lowered


def test_explicit_task_fields_override_captured_equivalents(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_referer", "")
    monkeypatch.setattr(request_context.settings, "default_origin", "")
    task = Task(
        id="override",
        url="https://example.test/file",
        referer="https://page.example.test/watch",
        origin="https://page.example.test",
        user_agent="Desktop UA",
        request_headers={
            "referer": "https://stale.test/",
            "origin": "https://stale.test",
            "user-agent": "Browser UA",
        },
    )

    headers = request_context.build_task_headers(task)

    assert headers["Referer"] == "https://page.example.test/watch"
    assert headers["Origin"] == "https://page.example.test"
    assert headers["User-Agent"] == "Desktop UA"
    assert "referer" not in headers
    assert "origin" not in headers
    assert "user-agent" not in headers


def test_browser_task_never_inherits_unrelated_global_identity(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_referer", "https://global.test/page")
    monkeypatch.setattr(request_context.settings, "default_origin", "https://global.test")
    monkeypatch.setattr(request_context.settings, "default_cookie", "global=secret")
    task = Task(
        id="browser-context",
        url="https://cdn.example.test/file.bin",
        request_headers={"x-playback-token": "opaque"},
        engine_state={"inherit_default_headers": False},
    )

    headers = request_context.build_task_headers(task)

    lowered = {name.lower() for name in headers}
    assert headers["x-playback-token"] == "opaque"
    assert "referer" not in lowered
    assert "origin" not in lowered
    assert "cookie" not in lowered


def test_hls_subresources_use_exact_origin_context_without_leaking_credentials(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_referer", "")
    monkeypatch.setattr(request_context.settings, "default_origin", "")
    monkeypatch.setattr(request_context.settings, "default_cookie", "")
    task = Task(
        id="origin-scopes",
        url="https://manifest.example.test/master.m3u8",
        source_page_url="https://page.example.test/watch",
        referer="https://page.example.test/watch",
        origin="https://page.example.test",
        cookie="manifest_session=one",
        request_headers={"authorization": "Bearer manifest"},
        request_contexts={
            "https://cdn.example.test": {
                "request_headers": {
                    "Authorization": "Bearer cdn",
                    "X-Playback-Token": "segment-token",
                },
                "referer": "https://page.example.test/watch",
                "origin": "https://page.example.test",
                "user_agent": "CDN Browser UA",
                "cookie": "cdn_session=two",
            }
        },
    )

    manifest = request_context.build_task_headers(task, request_url=task.url)
    cdn = request_context.build_task_headers(
        task, request_url="https://cdn.example.test/segments/1.ts"
    )
    unrelated = request_context.build_task_headers(
        task, request_url="https://other.example.test/segments/1.ts"
    )

    assert manifest["authorization"] == "Bearer manifest"
    assert manifest["Cookie"] == "manifest_session=one"
    assert cdn["authorization"] == "Bearer cdn"
    assert cdn["x-playback-token"] == "segment-token"
    assert cdn["Cookie"] == "cdn_session=two"
    assert cdn["User-Agent"] == "CDN Browser UA"
    assert "authorization" not in {name.lower() for name in unrelated}
    assert "cookie" not in {name.lower() for name in unrelated}
    assert unrelated["Referer"] == "https://page.example.test/watch"
    assert unrelated["Origin"] == "https://page.example.test"


def test_request_context_sanitizer_normalizes_origins_and_rejects_injection():
    contexts = request_context.sanitize_request_contexts({
        "https://CDN.example.test:443/path": {
            "request_headers": {"X-Token": "ok", "Host": "wrong.test"},
            "cookie": "session=ok",
            "referer": "https://page.test/watch\r\nX-Bad: injected",
        },
        "file:///tmp/video": {"cookie": "should-not-survive"},
    })

    assert set(contexts) == {"https://cdn.example.test"}
    assert contexts["https://cdn.example.test"]["request_headers"] == {"x-token": "ok"}
    assert contexts["https://cdn.example.test"]["cookie"] == "session=ok"
    assert contexts["https://cdn.example.test"]["referer"] == ""


def test_explicit_base_headers_survive_same_origin_but_not_cross_origin_credentials(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_referer", "")
    monkeypatch.setattr(request_context.settings, "default_origin", "")
    monkeypatch.setattr(request_context.settings, "default_cookie", "")
    task = Task(
        id="base-headers",
        url="https://manifest.example.test/master.m3u8",
        user_agent="Task Firefox UA",
        cookie="task=secret",
        request_headers={"authorization": "Bearer task"},
    )
    supplied = {
        "User-Agent": "Mozilla/5.0 Chrome/140.0 Safari/537.36",
        "Authorization": "Bearer supplied",
        "Cookie": "supplied=secret",
    }

    same_origin = request_context.build_task_headers(
        task,
        request_url="https://manifest.example.test/segment.ts",
        base_headers=supplied,
    )
    unrelated = request_context.build_task_headers(
        task,
        request_url="https://unrelated.example.test/segment.ts",
        base_headers=supplied,
    )

    assert same_origin["User-Agent"] == supplied["User-Agent"]
    assert same_origin["authorization"] == "Bearer supplied"
    assert same_origin["Cookie"] == "supplied=secret"
    assert unrelated["User-Agent"] == supplied["User-Agent"]
    assert "authorization" not in {name.lower() for name in unrelated}
    assert "cookie" not in {name.lower() for name in unrelated}


def test_exact_origin_context_overrides_supplied_credentials(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_cookie", "")
    task = Task(
        id="scoped-base",
        url="https://manifest.example.test/master.m3u8",
        request_contexts={
            "https://cdn.example.test": {
                "request_headers": {"authorization": "Bearer cdn"},
                "user_agent": "CDN Browser UA",
                "cookie": "cdn=secret",
            }
        },
    )

    headers = request_context.build_task_headers(
        task,
        request_url="https://cdn.example.test/segment.ts",
        base_headers={
            "User-Agent": "Manifest UA",
            "Authorization": "Bearer manifest",
            "Cookie": "manifest=secret",
        },
    )

    assert headers["User-Agent"] == "CDN Browser UA"
    assert headers["authorization"] == "Bearer cdn"
    assert headers["Cookie"] == "cdn=secret"
