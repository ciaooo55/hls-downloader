import base64

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
    assert headers["x-playback-token"] == "opaque"
    assert headers["Cookie"] == "session=explicit"
    lowered = {name.lower() for name in headers}
    assert "host" not in lowered
    assert "content-length" not in lowered
    assert "range" not in lowered
    assert "accept-encoding" not in lowered
    assert "sec-ch-ua" not in lowered
    assert "user-agent" not in lowered


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
    assert "user-agent" not in {name.lower() for name in cdn}
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

    assert same_origin["authorization"] == "Bearer supplied"
    assert same_origin["Cookie"] == "supplied=secret"
    assert "user-agent" not in {name.lower() for name in same_origin}
    assert "user-agent" not in {name.lower() for name in unrelated}
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

    assert headers["authorization"] == "Bearer cdn"
    assert headers["Cookie"] == "cdn=secret"
    assert "user-agent" not in {name.lower() for name in headers}


def test_exact_origin_header_overrides_are_used_for_manual_403_workarounds(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_cookie", "")
    task = Task(
        id="scoped-header-override",
        url="https://manifest.example.test/master.m3u8",
        request_contexts={
            "https://cdn.example.test": {
                "referer": "https://stale.example.test/watch",
                "origin": "https://stale.example.test",
                "user_agent": "Stale UA",
                "request_headers": {
                    "Referer": "https://manual.example.test/watch",
                    "Origin": "https://manual.example.test",
                    "User-Agent": "Manual UA",
                },
                "cookie": "cdn=secret",
            }
        },
    )

    headers = request_context.build_task_headers(
        task, request_url="https://cdn.example.test/segment.ts"
    )

    assert headers["Referer"] == "https://manual.example.test/watch"
    assert headers["Origin"] == "https://manual.example.test"
    assert "user-agent" not in {name.lower() for name in headers}


def test_exact_browser_request_identity_overrides_top_page_fallback(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_cookie", "")
    task = Task(
        id="browser-page-authority",
        url="https://media-cdn.example/720p/video.m3u8",
        source_page_url="https://video-page.example/watch/240#player",
        referer="https://media-cdn.example/",
        origin="https://media-cdn.example",
        request_contexts={
            "https://media-cdn.example": {
                "request_headers": {
                    "Referer": "https://media-cdn.example/video.m3u8",
                    "Origin": "https://media-cdn.example",
                },
                "referer": "https://media-cdn.example/video.m3u8",
                "origin": "https://media-cdn.example",
                "cookie": "session=valid",
            }
        },
    )

    headers = request_context.build_task_headers(
        task, request_url="https://media-cdn.example/720p/segment-1.ts"
    )

    assert headers["Referer"] == "https://media-cdn.example/video.m3u8"
    assert headers["Origin"] == "https://media-cdn.example"
    assert headers["Cookie"] == "session=valid"


def test_exact_browser_context_preserves_omitted_origin(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_origin", "")
    task = Task(
        id="browser-omitted-origin",
        url="https://cdn.example.test/video.mp4",
        source_page_url="https://page.example.test/watch",
        request_contexts={
            "https://cdn.example.test": {
                "request_headers": {"Referer": "https://embed.example.test/player"},
                "referer": "https://embed.example.test/player",
                "origin": "",
                "cookie": "",
            }
        },
    )

    headers = request_context.build_task_headers(task, request_url=task.url)

    assert headers["Referer"] == "https://embed.example.test/player"
    assert "origin" not in {name.lower() for name in headers}


def test_generic_http_client_keeps_browser_user_agent_without_client_hints(monkeypatch):
    monkeypatch.setattr(request_context.settings, "default_user_agent", "Fallback UA")
    task = Task(
        id="plain-http-identity",
        url="https://cdn.example.test/file.mp4",
        user_agent="Captured Browser UA",
        request_headers={
            "Accept": "video/mp4,*/*",
            "Sec-CH-UA": '"Chromium";v="140"',
            "Priority": "u=1",
        },
    )

    headers = request_context.build_task_headers(
        task, browser_profile_managed=False
    )

    assert headers["User-Agent"] == "Captured Browser UA"
    assert headers["accept"] == "video/mp4,*/*"
    assert "sec-ch-ua" not in {name.lower() for name in headers}
    assert "priority" not in {name.lower() for name in headers}


def test_request_replay_allows_only_bounded_json_or_form_post_bodies():
    payload = base64.b64encode(b'{"export":"episode-12"}').decode("ascii")
    method, body = request_context.sanitize_request_replay(
        "post", payload, {"Content-Type": "application/json; charset=utf-8"}
    )

    assert method == "POST"
    assert body == payload
    assert request_context.replay_request_body(method, body, {"content-type": "application/json"}) == b'{"export":"episode-12"}'

    assert request_context.sanitize_request_replay(
        "POST", payload, {"Content-Type": "multipart/form-data; boundary=test"}
    ) == ("GET", "")
    assert request_context.sanitize_request_replay(
        "DELETE", payload, {"Content-Type": "application/json"}
    ) == ("GET", "")
    assert request_context.sanitize_request_replay(
        "POST", "not-base64", {"Content-Type": "application/json"}
    ) == ("GET", "")
