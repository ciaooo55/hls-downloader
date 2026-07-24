from fastapi.testclient import TestClient

from backend.app.main import app
from backend.app.models import Task, TaskStatus, TaskType


def test_create_task_rejects_duplicate_url_without_opt_in(monkeypatch):
    from backend.app import api as api_module

    existing = Task(
        id="dup1",
        url="https://cdn.example.test/video.mp4",
        task_type=TaskType.HTTP,
        status=TaskStatus.QUEUED,
        filename="video.mp4",
    )
    monkeypatch.setattr(api_module.manager, "tasks", {existing.id: existing})

    client = TestClient(app)
    response = client.post(
        "/api/tasks",
        headers={"X-Token": "55555"},
        json={"url": "https://cdn.example.test/video.mp4"},
    )
    assert response.status_code == 409
    detail = response.json()["detail"]
    assert detail["code"] == "DUPLICATE_URL"
    assert detail["duplicates"][0]["id"] == "dup1"


def test_create_task_allows_duplicate_when_flag_set(monkeypatch):
    from backend.app import api as api_module

    existing = Task(
        id="dup2",
        url="https://cdn.example.test/file.bin",
        task_type=TaskType.HTTP,
        status=TaskStatus.DONE,
        filename="file.bin",
    )
    monkeypatch.setattr(api_module.manager, "tasks", {existing.id: existing})

    created = []

    async def create_task(**kwargs):
        task = Task(
            id="new1",
            url=kwargs["url"],
            task_type=TaskType.HTTP,
            status=TaskStatus.QUEUED,
            filename="file.bin",
        )
        created.append(task)
        return task

    monkeypatch.setattr(api_module.manager, "create_task", create_task)
    # find_tasks_by_url uses manager.tasks
    response = TestClient(app).post(
        "/api/tasks",
        headers={"X-Token": "55555"},
        json={"url": "https://cdn.example.test/file.bin", "allow_duplicate": True},
    )
    assert response.status_code == 200
    assert created and created[0].id == "new1"
