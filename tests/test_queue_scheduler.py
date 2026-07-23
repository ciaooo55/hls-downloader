from datetime import datetime

from backend.app.downloader.task_manager import TaskManager
from backend.app import downloader as downloader_package


def test_queue_auto_start_time_is_fail_closed_and_time_aware(monkeypatch):
    manager = TaskManager()
    settings = __import__("backend.app.downloader.task_manager", fromlist=["settings"]).settings
    monkeypatch.setattr(settings, "queue_auto_start_enabled", True)
    monkeypatch.setattr(settings, "queue_auto_start_time", "21:30")
    assert not manager._queue_auto_start_due(datetime(2026, 7, 23, 21, 29))
    assert manager._queue_auto_start_due(datetime(2026, 7, 23, 21, 30))
    monkeypatch.setattr(settings, "queue_auto_start_time", "bad")
    assert not manager._queue_auto_start_due(datetime(2026, 7, 23, 22, 0))
