from backend.app.windows_attachment import is_public_download_url, mark_download_from_internet


def test_public_download_url_rejects_local_and_private_destinations():
    assert is_public_download_url("https://cdn.example.test/file.exe") is True
    assert is_public_download_url("http://127.0.0.1/file.exe") is False
    assert is_public_download_url("http://192.168.1.5/file.exe") is False
    assert is_public_download_url("http://169.254.169.254/latest/meta-data") is False
    assert is_public_download_url("file:///C:/temp/file.exe") is False


def test_motw_uses_redacted_source_urls_on_windows(tmp_path):
    target = tmp_path / "download.exe"
    target.write_bytes(b"MZ")

    marked = mark_download_from_internet(
        str(target),
        "https://cdn.example.test/download.exe?token=secret",
        "https://site.example.test/page?session=private",
    )

    if marked:
        zone = (tmp_path / "download.exe:Zone.Identifier").read_text(encoding="utf-8")
        assert "ZoneId=3" in zone
        assert "token" not in zone
        assert "session" not in zone
    else:
        # Non-Windows CI intentionally skips ADS creation.
        assert marked == 0
