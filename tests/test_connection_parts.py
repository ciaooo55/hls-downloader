from backend.app.connection_parts import (
    CONNECTION_PARTS_LIMIT,
    build_connection_parts,
    connection_parts_payload,
    count_active_parts,
    normalize_connection_parts,
    set_connection_parts,
)
from backend.app.models import Task, TaskProgress, TaskType
from backend.app.schemas import TaskResponse


def test_empty_inputs_stay_empty():
    assert build_connection_parts(total=0, chunks=[(0, 9)]) == []
    assert build_connection_parts(total=10, chunks=[]) == []
    assert normalize_connection_parts(None) == []
    assert normalize_connection_parts("x") == []


def test_http_fake_ranges_publish_done_active_queued():
    parts = build_connection_parts(
        total=30,
        chunks=[(0, 9), (10, 19), (20, 29)],
        range_current={0: 10, 1: 10, 2: 20},
        completed={0},
        partials={(1, 12): 4},
        finished_intervals={1: {10: 12}},
    )
    assert parts == [
        {"start": 0, "end": 11, "done": 12, "state": "done"},
        {"start": 12, "end": 15, "done": 4, "state": "active"},
        {"start": 16, "end": 29, "done": 0, "state": "queued"},
    ]
    assert count_active_parts(parts) == 1


def test_non_http_and_no_range_payload_stay_empty():
    hls = Task(id="hls1", url="https://example.test/a.m3u8", task_type=TaskType.HLS)
    assert connection_parts_payload(hls) == []
    http = Task(id="http1", url="https://example.test/a.bin", task_type=TaskType.HTTP)
    http.progress = TaskProgress(total_bytes=100)
    assert connection_parts_payload(http) == []
    set_connection_parts(http, [])
    assert http.progress.connection_parts == []


def test_http_task_with_fake_ranges_exposes_payload():
    task = Task(id="http2", url="https://example.test/a.bin", task_type=TaskType.HTTP)
    task.progress = TaskProgress(total_bytes=20)
    set_connection_parts(
        task,
        build_connection_parts(
            total=20,
            chunks=[(0, 9), (10, 19)],
            range_current={0: 10, 1: 10},
            completed={0},
        ),
    )
    payload = connection_parts_payload(task)
    assert payload[0]["state"] == "done"
    assert payload[0]["start"] == 0
    assert payload[0]["end"] == 9
    assert payload[1]["state"] == "queued"
    assert payload[1]["start"] == 10


def test_normalize_clips_and_caps():
    parts = [{"start": -3, "end": "4", "done": "9", "state": "weird"}]
    assert normalize_connection_parts(parts, total=8) == [
        {"start": 0, "end": 4, "done": 5, "state": "done"}
    ]
    many = [{"start": i, "end": i, "done": 0, "state": "queued"} for i in range(200)]
    bounded = normalize_connection_parts(many, total=200)
    assert len(bounded) <= CONNECTION_PARTS_LIMIT
    assert bounded[0]["start"] == 0
    assert bounded[-1]["end"] == 199


def test_task_response_accepts_connection_parts():
    payload = TaskResponse(
        id="t",
        title="a",
        url="https://example.test/a.bin",
        referer="",
        origin="",
        user_agent="",
        cookie="",
        filename="a.bin",
        concurrency=4,
        status="downloading",
        stage="downloading",
        last_log="",
        total_segments=2,
        completed_segments=1,
        failed_segments=0,
        downloaded_bytes=10,
        total_bytes=20,
        speed_bytes_per_sec=0,
        eta_seconds=0,
        error_message="",
        output_path="",
        created_at="",
        updated_at="",
        connection_parts=[{"start": 0, "end": 9, "done": 10, "state": "done"}],
    )
    assert payload.connection_parts[0]["state"] == "done"
