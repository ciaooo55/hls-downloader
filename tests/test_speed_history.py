from types import SimpleNamespace

from backend.app.models import Task, TaskProgress, TaskStatus
from backend.app.speed_history import (
    record_speed_sample,
    record_speed_samples,
    speed_history_payload,
    speed_peak_payload,
)


def _task(status=TaskStatus.DOWNLOADING, speed=1024):
    task = Task(id="t1", url="https://example.test/a.bin")
    task.status = status
    task.progress = TaskProgress(speed_bytes_per_sec=speed)
    return task


def test_transfer_samples_are_capped_and_throttled():
    task = _task(speed=2048)
    assert record_speed_sample(task, now=10.0) is True
    assert record_speed_sample(task, now=10.4) is False
    assert record_speed_sample(task, now=11.0) is True
    assert speed_history_payload(task) == [2048, 2048]
    assert speed_peak_payload(task) == 2048


def test_history_keeps_only_the_latest_window():
    task = _task(speed=100)
    for index in range(200):
        task.progress.speed_bytes_per_sec = index
        record_speed_sample(task, now=float(index))
    history = speed_history_payload(task)
    assert len(history) == 180
    assert history[0] == 20
    assert history[-1] == 199
    assert speed_peak_payload(task) == 199


def test_idle_status_records_a_single_trailing_zero():
    task = _task(speed=4096)
    record_speed_sample(task, now=1.0)
    task.status = TaskStatus.PAUSED
    task.progress.speed_bytes_per_sec = 0
    assert record_speed_sample(task, now=2.0) is True
    assert record_speed_sample(task, now=3.0) is False
    assert speed_history_payload(task) == [4096, 0]
    assert speed_peak_payload(task) == 4096


def test_queued_task_does_not_invent_history():
    task = _task(status=TaskStatus.QUEUED, speed=0)
    assert record_speed_sample(task, now=1.0) is False
    assert speed_history_payload(task) == []


def test_record_speed_samples_covers_a_batch():
    running = _task(speed=512)
    queued = _task(status=TaskStatus.QUEUED, speed=0)
    queued.id = "t2"
    assert record_speed_samples([running, queued], now=5.0) == 1
    assert speed_history_payload(running) == [512]
    assert speed_history_payload(queued) == []


def test_invalid_speed_is_stored_as_zero():
    task = _task()
    task.progress.speed_bytes_per_sec = "bad"  # type: ignore[assignment]
    assert record_speed_sample(task, now=1.0) is True
    assert speed_history_payload(task) == [0]
