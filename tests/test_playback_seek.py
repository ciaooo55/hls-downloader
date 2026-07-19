import asyncio
import types

from backend.app.config import settings
from backend.app.downloader.hls import HLSDownloader
from backend.app.models import Task, TaskStatus


def test_playback_seek_prioritizes_target_and_forward_segments(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    task = Task(
        id="priority",
        url="https://example.test/video.m3u8",
        status=TaskStatus.DOWNLOADING_SEGMENTS,
        concurrency=1,
    )
    task.cancel_event = asyncio.Event()
    task.pause_event = asyncio.Event()
    task.progress.total_segments = 8
    downloader = HLSDownloader(task)
    order = []

    async def fake_download(self, client, segment, headers):
        order.append(segment["index"])
        if segment["index"] == 0:
            self.request_seek(6)
        await asyncio.sleep(0)
        return True

    downloader._download_one_segment = types.MethodType(fake_download, downloader)
    segments = [
        {"index": index, "url": f"https://example.test/{index}.ts"}
        for index in range(8)
    ]

    completed = asyncio.run(
        downloader._download_segments(
            object(),
            segments,
            {},
            concurrency=1,
        )
    )

    assert completed is True
    assert order[:3] == [0, 6, 7]
    assert sorted(order) == list(range(8))
