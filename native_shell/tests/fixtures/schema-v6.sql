-- Frozen schema from v7.0.3-r22 native_shell/src/store.rs.
CREATE TABLE IF NOT EXISTS tasks (
task_id TEXT PRIMARY KEY,
snapshot_json TEXT NOT NULL,
created_at_ms INTEGER NOT NULL,
updated_at_ms INTEGER NOT NULL,
event_sequence INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS task_specs (
task_id TEXT PRIMARY KEY,
spec_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS task_ranges (
task_id TEXT NOT NULL,
start_byte INTEGER NOT NULL,
end_byte INTEGER NOT NULL,
validator TEXT NOT NULL DEFAULT '',
PRIMARY KEY(task_id, start_byte, end_byte),
FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS handoffs (
handoff_id TEXT PRIMARY KEY,
public_json TEXT NOT NULL,
status TEXT NOT NULL,
task_id TEXT,
created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS media_tracks (
task_id TEXT NOT NULL,
track_id TEXT NOT NULL,
track_json TEXT NOT NULL,
PRIMARY KEY(task_id, track_id),
FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS event_checkpoints (
checkpoint_id INTEGER PRIMARY KEY CHECK(checkpoint_id = 1),
sequence INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS logs (
id INTEGER PRIMARY KEY AUTOINCREMENT,
task_id TEXT,
level TEXT NOT NULL,
code TEXT NOT NULL,
message TEXT NOT NULL,
created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
key TEXT PRIMARY KEY,
value_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS credentials (
credential_ref TEXT PRIMARY KEY,
protected_blob TEXT NOT NULL,
kind TEXT NOT NULL,
created_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_updated ON tasks(updated_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_task_ranges_task ON task_ranges(task_id);
CREATE INDEX IF NOT EXISTS idx_logs_task ON logs(task_id, id);
PRAGMA user_version = 6;
