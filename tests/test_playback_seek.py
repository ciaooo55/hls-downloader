import asyncio
import types

from backend.app.config import settings
from backend.app.downloader.hls import HLSDownloader
from backend.app.downloader.task_manager import TaskManager
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


def test_speculative_player_requests_do_not_replace_explicit_seek():
    class FakeDownloader:
        def __init__(self):
            self.requests = []

        def request_seek(self, segment_index):
            self.requests.append(segment_index)

    async def run():
        manager = TaskManager()
        task = Task(
            id="seek-priority",
            url="https://example.test/video.m3u8",
            status=TaskStatus.DOWNLOADING_SEGMENTS,
            playback_seek_index=4,
        )
        downloader = FakeDownloader()
        manager.tasks[task.id] = task
        manager._downloaders[task.id] = downloader

        await manager.request_playback_seek(task.id, 7, force=False)
        assert task.playback_seek_index == 4
        assert downloader.requests == []

        await manager.request_playback_seek(task.id, 2, force=True)
        assert task.playback_seek_index == 2
        assert downloader.requests == [2]

    asyncio.run(run())
