from backend.app.naming import is_generic_media_name, suggest_manifest_name


def test_generic_manifest_names_prefer_page_title():
    assert is_generic_media_name("video.m3u8")
    assert suggest_manifest_name(
        "https://cdn.example/video.m3u8?token=secret",
        filename="video.m3u8",
        title="第十二集：重新出发",
        source_page_url="https://site.example/watch/episode-12",
    ) == "第十二集：重新出发"


def test_manifest_name_prefers_response_and_playlist_metadata():
    assert suggest_manifest_name(
        "https://cdn.example/master.m3u8",
        filename="master.m3u8",
        title="网页标题",
        manifest_title="片内标题",
        response_filename="服务器片名.mp4",
    ) == "服务器片名.mp4"
    assert suggest_manifest_name(
        "https://cdn.example/master.m3u8",
        filename="master.m3u8",
        title="网页标题",
        manifest_title="片内标题",
    ) == "片内标题"


def test_manifest_name_uses_meaningful_url_name_before_generic_fallback():
    assert suggest_manifest_name("https://cdn.example/series/episode-07.m3u8") == "episode-07"
    assert suggest_manifest_name("https://cdn.example/video.m3u8", fallback="abc12345") == "video"
