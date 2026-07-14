import aiosqlite
import asyncio
from .paths import RUNTIME_PATHS

DB_PATH = RUNTIME_PATHS.database_path

SCHEMA = """CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    title TEXT DEFAULT '',
    url TEXT NOT NULL,
    referer TEXT DEFAULT '',
    origin TEXT DEFAULT '',
    user_agent TEXT DEFAULT '',
    cookie TEXT DEFAULT '',
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
    error_message TEXT DEFAULT '',
    output_path TEXT DEFAULT '',
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    started_at TEXT DEFAULT '',
    finished_at TEXT DEFAULT ''
)"""

# Migration: add columns if they don't exist
MIGRATIONS = [
    "ALTER TABLE tasks ADD COLUMN started_at TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN finished_at TEXT DEFAULT ''",
    "ALTER TABLE tasks ADD COLUMN post_percent REAL DEFAULT 0",
]

_lock = asyncio.Lock()

async def _migrate(db):
    """Add missing columns to existing tables."""
    try:
        cursor = await db.execute("PRAGMA table_info(tasks)")
        cols = [row[1] for row in await cursor.fetchall()]
        for sql in MIGRATIONS:
            col_name = sql.split("ADD COLUMN")[1].strip().split()[0]
            if col_name not in cols:
                try:
                    await db.execute(sql)
                except Exception:
                    pass
        await db.commit()
    except Exception:
        pass

async def run_db(sql, params=()):
    async with _lock:
        db = await aiosqlite.connect(str(DB_PATH), timeout=30)
        db.row_factory = aiosqlite.Row
        try:
            await db.execute("PRAGMA busy_timeout=10000")
            await db.execute(SCHEMA)
            await _migrate(db)
            cursor = await db.execute(sql, params)
            await db.commit()
            rows = await cursor.fetchall() if cursor.description else []
            return rows
        finally:
            await db.close()

async def run_db_many(sql, params_list):
    async with _lock:
        db = await aiosqlite.connect(str(DB_PATH), timeout=30)
        db.row_factory = aiosqlite.Row
        try:
            await db.execute("PRAGMA busy_timeout=10000")
            await db.execute(SCHEMA)
            await _migrate(db)
            for params in params_list:
                await db.execute(sql, params)
            await db.commit()
        finally:
            await db.close()
