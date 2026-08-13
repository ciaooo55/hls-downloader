from backend.app.metalink import parse_metalink, read_metalink_files
from backend.app.link_file import EXPLORER_LINK_SUFFIXES, read_link_urls
from backend.app.url_recognition import _is_direct_file_response, _metalink_result


META4 = """<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="demo.bin">
    <size>4</size>
    <hash type="sha-256">9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08</hash>
    <url location="de" priority="1">https://cdn.example.test/demo.bin</url>
    <url priority="2">https://mirror.example.test/demo.bin</url>
    <url priority="3">ftp://ftp.example.test/demo.bin</url>
    <url>javascript:alert(1)</url>
  </file>
</metalink>
"""

META3 = """<?xml version="1.0" encoding="UTF-8"?>
<metalink version="3.0" xmlns="http://www.metalinker.org/">
  <files>
    <file name="pkg.zip">
      <size>8</size>
      <verification>
        <hash type="md5">098f6bcd4621d373cade4e832627b4f6</hash>
      </verification>
      <resources>
        <url type="http" preference="90">http://old.example.test/pkg.zip</url>
        <url type="http" preference="100">https://best.example.test/pkg.zip</url>
      </resources>
    </file>
    <file name="notes.txt">
      <resources>
        <url type="ftp">ftp://ftp.example.test/notes.txt</url>
      </resources>
    </file>
  </files>
</metalink>
"""


def test_parse_metalink4_picks_http_primary_and_mirrors():
    files = parse_metalink(META4)
    assert len(files) == 1
    item = files[0]
    assert item.name == "demo.bin"
    assert item.url == "https://cdn.example.test/demo.bin"
    assert item.mirrors == ["https://mirror.example.test/demo.bin"]
    assert item.checksum.startswith("sha256:")
    assert item.size == 4


def test_parse_metalink3_orders_by_preference_and_keeps_ftp_file():
    files = parse_metalink(META3)
    assert [item.name for item in files] == ["pkg.zip", "notes.txt"]
    assert files[0].url == "https://best.example.test/pkg.zip"
    assert files[0].mirrors == ["http://old.example.test/pkg.zip"]
    assert files[0].checksum.startswith("md5:")
    assert files[1].url == "ftp://ftp.example.test/notes.txt"
    assert files[1].mirrors == []


def test_rejects_empty_or_local_only_metalink():
    import pytest
    from backend.app.link_file import LinkFileError
    with pytest.raises(LinkFileError):
        parse_metalink("<metalink><file name=\"x\"><url>file:///tmp/a.bin</url></file></metalink>")


def test_read_link_urls_and_suffixes_include_metalink(tmp_path):
    path = tmp_path / "demo.meta4"
    path.write_text(META4, encoding="utf-8")
    assert ".meta4" in EXPLORER_LINK_SUFFIXES
    assert read_link_urls(path) == ["https://cdn.example.test/demo.bin"]
    assert read_metalink_files(path)[0].mirrors == ["https://mirror.example.test/demo.bin"]


def test_recognize_metalink_payload_exposes_checksum_and_mirrors():
    result = _metalink_result("https://example.test/demo.meta4", META4)
    assert result is not None
    assert result.kind == "file"
    assert result.candidates[0].url == "https://cdn.example.test/demo.bin"
    assert result.candidates[0].checksum.startswith("sha256:")
    assert result.candidates[0].mirrors == ["https://mirror.example.test/demo.bin"]
    assert _is_direct_file_response("application/metalink4+xml", "attachment", "https://example.test/demo.meta4") is False


def test_watch_folder_ignores_metalink(tmp_path):
    from backend.app.torrent_watch import TorrentWatchState, collect_new_torrents
    state = TorrentWatchState()
    (tmp_path / "demo.meta4").write_text(META4, encoding="utf-8")
    assert collect_new_torrents(str(tmp_path), state) == []
