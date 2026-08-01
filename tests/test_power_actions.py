import asyncio

from fastapi import FastAPI
from fastapi.testclient import TestClient
from pydantic import ValidationError
import pytest

from backend.app import api as api_module
from backend.app.config import settings
from backend.app.downloader import task_manager as manager_module
from backend.app.downloader.task_manager import TaskManager
from backend.app.models import Task, TaskStatus
from backend.app.power_actions import PowerActionService
from backend.app.schemas import TaskCreate


def test_power_action_service_publishes_and_can_be_canceled():
    async def run():
        service = PowerActionService(delay_seconds=30)
        events = []
        action_id = service.schedule(
            task_id="task",
            task_title="download",
            action="sleep",
            publish=events.append,
            executor=lambda _action: pytest.fail("canceled action must not execute"),
        )
        await asyncio.sleep(0)
        assert events[0]["type"] == "power_action_pending"
        assert events[0]["delay_seconds"] == 30
        assert service.pending(action_id)["action"] == "sleep"
        assert service.cancel(action_id) is True
        await asyncio.sleep(0)
        assert service.pending(action_id) is None

    asyncio.run(run())


def test_power_action_service_executes_after_countdown(monkeypatch):
    original_sleep = asyncio.sleep

    async def instant_sleep(_seconds):
        return None

    monkeypatch.setattr("backend.app.power_actions.asyncio.sleep", instant_sleep)

    async def run():
        service = PowerActionService(delay_seconds=5)
        events = []
        executed = []
        service.schedule(
            task_id="task",
            task_title="download",
            action="hibernate",
            publish=events.append,
            executor=executed.append,
        )
        await original_sleep(0)
        await original_sleep(0)
        assert executed == ["hibernate"]
        assert events[-1]["type"] == "power_action_executed"

    asyncio.run(run())


def test_completed_task_schedules_configured_power_action(monkeypatch):
    async def run():
        manager = TaskManager()
        task = Task(id="done", url="https://example.test/file", status=TaskStatus.DONE)
        task.engine_state["completion_action"] = "shutdown"
        manager.tasks[task.id] = task
        scheduled = []

        def schedule(**kwargs):
            scheduled.append(kwargs)
            return "action"

        async def save(_task):
            return None

        monkeypatch.setattr(manager_module.power_action_service, "schedule", schedule)
        monkeypatch.setattr(manager, "_save_db", save)
        manager._on_task_finished(task)
        await asyncio.sleep(0)

        assert scheduled[0]["task_id"] == task.id
        assert scheduled[0]["action"] == "shutdown"
        assert task.engine_state["completion_action_handled"] is True

    asyncio.run(run())


def test_completion_action_request_is_strictly_validated():
    with pytest.raises(ValidationError):
        TaskCreate(url="https://example.test/file", completion_action="reboot")


def test_power_action_api_requires_token_and_supports_cancel(monkeypatch):
    monkeypatch.setattr(api_module.power_action_service, "cancel", lambda action_id: action_id == "known")
    app = FastAPI()
    app.include_router(api_module.router)
    with TestClient(app) as client:
        unauthorized = client.post("/api/power-actions/known/cancel")
        canceled = client.post(
            "/api/power-actions/known/cancel", headers={"X-Token": settings.token}
        )
        missing = client.post(
            "/api/power-actions/missing/cancel", headers={"X-Token": settings.token}
        )
    assert unauthorized.status_code == 401
    assert canceled.status_code == 200
    assert missing.status_code == 404
