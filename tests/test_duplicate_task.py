from backend.app.duplicate_task import duplicate_task_entry, suggest_duplicate_action
from backend.app.models import Task, TaskStatus, TaskType


def test_suggests_resume_retry_start_open_and_focus():
    assert suggest_duplicate_action("paused", ["resume", "delete"]) == "resume"
    assert suggest_duplicate_action("failed", ["retry"]) == "retry"
    assert suggest_duplicate_action("queued", ["start"]) == "start"
    assert suggest_duplicate_action("done", ["open"], output_path="D:/a.bin") == "open"
    assert suggest_duplicate_action("done", ["retry"], output_missing=True) == "retry"
    assert suggest_duplicate_action("downloading", ["pause"]) == "focus"
    assert suggest_duplicate_action("queued", ["delete"]) == "none"


def test_duplicate_entry_uses_live_task_fields():
    task = Task(
        id="t1",
        url="https://cdn.example.test/a.bin",
        task_type=TaskType.HTTP,
        status=TaskStatus.PAUSED,
        filename="a.bin",
        output_path="D:/a.bin",
    )
    entry = duplicate_task_entry(task, ["resume", "delete"])
    assert entry["id"] == "t1"
    assert entry["suggested_action"] == "resume"
    assert entry["filename"] == "a.bin"
    assert "resume" in entry["available_actions"]
