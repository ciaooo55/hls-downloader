import aiosqlite
import asyncio
import weakref
from pathlib import Path
from .paths import RUNTIME_PATHS

DB_PATH = RUNTIME_PATHS.database_path

SCHEMA = """CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    task_type TEXT DEFAULT 'hls',
    source_page_url TEXT DEFAULT '',
    mime_type TEXT DEFAULT '',
    title TEXT DEFAULT '',
    url TEXT NOT NULL,
    referer TEXT DEFAULT '',
    origin TEXT DEFAULT '',
    user_agent TEXT DEFAULT '',
    cookie TEXT DEFAULT '',
    request_headers TEXT DEFAULT '',
    request_contexts TEXT DEFAULT '',
    request_method TEXT DEFAULT 'GET',
    request_body TEXT DEFAULT '',
    filename TEXT DEFAULT '',
    concurrency INTEGER DEFAULT 4,
    status TEXT DEFAULT 'queued',
    stage TEXT DEFAULT '',
    last_log TEXT DEFAULT '',
    total_segments INTEGER DEFAULT 0,
    completed_segments INTEGER DEFAULT 0,
    failed_segments INTEGER DEFAULT 0,
    downloaded_bytes INTEGER DEFAULT 0,
    total_bytes INTEGER DEFAULT 0,
    speed_bytes_per_sec REAL DEFAULT 0,
    eta_seconds REAL DEFAULT 0,
    post_percent REAL DEFAULT 0,
    playable_segments INTEGER DEFAULT 0,
    playable_duration REAL DEFAULT 0,
    media_duration REAL DEFAULT 0,
    progress_percent REAL DEFAULT 0,
    uploaded_bytes INTEGER DEFAULT 0,
    upload_speed_bytes_per_sec REAL DEFAULT 0,
    peer_count INTEGER DEFAULT 0,
    seed_count INTEGER DEFAULT 0,
    engine_state TEXT DEFAULT '{}',
    error_message TEXT DEFAULT '',
    error_code TEXT DEFAULT '',
    error_stage TEXT DEFAULT '',
    error_url TEXT DEFAULT '',
    error_hint TEXT DEFAULT '',
    http_status INTEGER DEFAULT 0,
    error_attempt INTEGER DEFAULT 0,
    expected_checksum TEXT DEFAULT '',
    checksum_algorithm TEXT DEFAULT '',
    checksum_actual TEXT DEFAULT '',
    checksum_verified INTEGER,
    output_path TEXT DEFAULT '',
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    started_at TEXT DEFAULT '',
    finished_at TEXT DEFAULT ''
)"""

# Migration: add columns if they don't exist
MIGRATIONS = [
    "ALTER TABLE tasks ADD COLUMN task_type TEXT DEFAULT 'hls'",
    "ALTER TABLE tasks ADD COLUMN source_page_url TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN mime_type TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN started_at TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN finished_at TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN post_percent REAL DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN playable_segments INTEGER DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN playable_duration REAL DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN media_duration REAL DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN progress_percent REAL DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN uploaded_bytes INTEGER DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN upload_speed_bytes_per_sec REAL DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN peer_count INTEGER DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN seed_count INTEGER DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN engine_state TEXT DEFAULT '{}'",
    "ALTER TABLE tasks ADD COLUMN error_code TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN error_stage TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN error_url TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN error_hint TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN http_status INTEGER DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN error_attempt INTEGER DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN expected_checksum TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN checksum_algorithm TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN checksum_actual TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN checksum_verified INTEGER",
    "ALTER TABLE tasks ADD COLUMN request_headers TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN request_contexts TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN request_method TEXT DEFAULT 'GET'",
    "ALTER TABLE tasks ADD COLUMN request_body TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN speed_limit_kib INTEGER DEFAULT 0",
    "ALTER TABLE tasks ADD COLUMN selected_video TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN selected_audio TEXT DEFAULT ''",
]
SCHEMA_VERSION = 1

_connection: aiosqlite.Connection | None = None
_connection_loop: asyncio.AbstractEventLoop | None = None
_connection_path: Path | None = None
_operation_lock: asyncio.Lock | None = None
_ephemeral_locks: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()

async def _migrate(db):
    """Apply the legacy-column baseline once, then record explicit versions."""
    await db.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations ("
        "version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT (datetime('now')))"
    )
    cursor = await db.execute("SELECT COALESCE(MAX(version), 0) FROM schema_migrations")
    current = int((await cursor.fetchone())[0] or 0)
    if current >= SCHEMA_VERSION:
        return
    cursor = await db.execute("PRAGMA table_info(tasks)")
    cols = {row[1] for row in await cursor.fetchall()}
    for sql in MIGRATIONS:
        col_name = sql.split("ADD COLUMN")[1].strip().split()[0]
        if col_name not in cols:
            await db.execute(sql)
            cols.add(col_name)
    await db.execute(
        "CREATE INDEX IF NOT EXISTS idx_tasks_status_created ON tasks(status, created_at DESC)"
    )
    await db.execute(
        "CREATE INDEX IF NOT EXISTS idx_tasks_updated ON tasks(updated_at DESC)"
    )
    await db.execute(
        "INSERT INTO schema_migrations(version) VALUES (?)",
        (SCHEMA_VERSION,),
    )
    await db.commit()


async def _prepare(db: aiosqlite.Connection) -> None:
    db.row_factory = aiosqlite.Row
    await db.execute("PRAGMA busy_timeout=10000")
    await db.execute("PRAGMA journal_mode=WAL")
    await db.execute("PRAGMA synchronous=NORMAL")
    await db.execute(SCHEMA)
    await _migrate(db)


async def initialize_database() -> None:
    """Open and migrate the application database once for this Core lifetime."""
    global _connection, _connection_loop, _connection_path, _operation_lock
    loop = asyncio.get_running_loop()
    path = Path(DB_PATH).resolve()
    if _connection is not None and _connection_loop is loop and _connection_path == path:
        return
    if _connection is not None:
        await _connection.close()
    path.parent.mkdir(parents=True, exist_ok=True)
    connection = await aiosqlite.connect(str(path), timeout=30)
    try:
        await _prepare(connection)
    except Exception:
        await connection.close()
        raise
    _connection = connection
    _connection_loop = loop
    _connection_path = path
    _operation_lock = asyncio.Lock()


async def close_database() -> None:
    global _connection, _connection_loop, _connection_path, _operation_lock
    connection = _connection
    _connection = None
    _connection_loop = None
    _connection_path = None
    _operation_lock = None
    if connection is not None:
        await connection.close()


def _active_connection() -> tuple[aiosqlite.Connection, asyncio.Lock] | None:
    loop = asyncio.get_running_loop()
    path = Path(DB_PATH).resolve()
    if (
        _connection is not None
        and _operation_lock is not None
        and _connection_loop is loop
        and _connection_path == path
    ):
        return _connection, _operation_lock
    return None


def _ephemeral_lock() -> asyncio.Lock:
    loop = asyncio.get_running_loop()
    lock = _ephemeral_locks.get(loop)
    if lock is None:
        lock = asyncio.Lock()
        _ephemeral_locks[loop] = lock
    return lock


async def _execute(db: aiosqlite.Connection, sql, params=()):
    try:
        cursor = await db.execute(sql, params)
        await db.commit()
        return await cursor.fetchall() if cursor.description else []
    except Exception:
        await db.rollback()
        raise

async def run_db(sql, params=()):
    active = _active_connection()
    if active is not None:
        db, lock = active
        async with lock:
            return await _execute(db, sql, params)
    async with _ephemeral_lock():
        db = await aiosqlite.connect(str(DB_PATH), timeout=30)
        try:
            await _prepare(db)
            return await _execute(db, sql, params)
        finally:
            await db.close()

async def run_db_many(sql, params_list):
    active = _active_connection()
    if active is not None:
        db, lock = active
        async with lock:
            try:
                await db.executemany(sql, params_list)
                await db.commit()
            except Exception:
                await db.rollback()
                raise
        return
    async with _ephemeral_lock():
        db = await aiosqlite.connect(str(DB_PATH), timeout=30)
        try:
            await _prepare(db)
            await db.executemany(sql, params_list)
            await db.commit()
        except Exception:
            await db.rollback()
            raise
        finally:
            await db.close()
