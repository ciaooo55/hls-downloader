import asyncio
from pathlib import Path
from types import SimpleNamespace

import pytest

from backend.app.downloader import postprocess_slot


def _task(task_id: str):
    return SimpleNamespace(
        id=task_id,
        status="merging",
        stage="merging",
        last_log="",
    )


def test_shared_volume_work_is_serialized(tmp_path):
    async def scenario():
        first = await postprocess_slot.acquire_postprocess_lease((tmp_path / "one",))
        acquired_second = asyncio.Event()

        async def take_second():
            lease = await postprocess_slot.acquire_postprocess_lease((tmp_path / "two",))
            acquired_second.set()
            lease.release()

        waiter = asyncio.create_task(take_second())
        await asyncio.sleep(0)
        assert not acquired_second.is_set()
        first.release()
        await asyncio.wait_for(waiter, timeout=1)
        assert acquired_second.is_set()

    asyncio.run(scenario())


def test_waiting_task_reports_stage_and_message(tmp_path):
    async def scenario():
        first = await postprocess_slot.acquire_postprocess_lease((tmp_path / "one",))
        task = _task("second")
        updates = []
        logs = []
        waiter = asyncio.create_task(
            postprocess_slot.acquire_postprocess_lease(
                (tmp_path / "two",),
                task=task,
                waiting_stage="remuxing",
                waiting_message="等待同盘合并",
                on_progress=lambda current: updates.append(current.last_log),
                on_log=lambda task_id, message: logs.append((task_id, message)),
            )
        )
        await asyncio.sleep(0)
        assert task.status.value == "remuxing"
        assert task.stage == "remuxing"
        assert updates == ["等待同盘合并"]
        assert logs == [("second", "[merge] 等待同盘合并")]
        first.release()
        second = await asyncio.wait_for(waiter, timeout=1)
        second.release()

    asyncio.run(scenario())


def test_cancelled_waiter_does_not_keep_volume_locked(tmp_path):
    async def scenario():
        first = await postprocess_slot.acquire_postprocess_lease((tmp_path / "one",))
        waiter = asyncio.create_task(
            postprocess_slot.acquire_postprocess_lease((tmp_path / "two",))
        )
        await asyncio.sleep(0)
        waiter.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiter
        first.release()
        third = await asyncio.wait_for(
            postprocess_slot.acquire_postprocess_lease((tmp_path / "three",)),
            timeout=1,
        )
        third.release()

    asyncio.run(scenario())


def test_windows_volume_key_is_case_insensitive(monkeypatch):
    monkeypatch.setattr(postprocess_slot.os, "name", "nt")
    assert postprocess_slot.volume_key(Path("D:/one")) == postprocess_slot.volume_key(
        Path("d:/two")
    )
