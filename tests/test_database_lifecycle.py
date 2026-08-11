import asyncio


def test_core_lifecycle_reuses_one_wal_connection_and_migrates_once(tmp_path, monkeypatch):
    from backend.app import database

    async def run():
        await database.close_database()
        monkeypatch.setattr(database, "DB_PATH", tmp_path / "tasks.db")
        calls = 0
        original_migrate = database._migrate

        async def counted_migrate(connection):
            nonlocal calls
            calls += 1
            await original_migrate(connection)

        monkeypatch.setattr(database, "_migrate", counted_migrate)
        try:
            await database.initialize_database()
            connection_id = id(database._connection)
            await database.run_db(
                "INSERT INTO tasks (id,url,title) VALUES (?,?,?)",
                ("persistent", "https://example.test/file", "one"),
            )
            await database.run_db(
                "UPDATE tasks SET title=? WHERE id=?",
                ("two", "persistent"),
            )
            rows = await database.run_db("SELECT title FROM tasks WHERE id=?", ("persistent",))
            journal = await database.run_db("PRAGMA journal_mode")
            migrations = await database.run_db(
                "SELECT version FROM schema_migrations ORDER BY version"
            )

            assert id(database._connection) == connection_id
            assert calls == 1
            assert rows[0]["title"] == "two"
            assert str(journal[0][0]).lower() == "wal"
            assert [row["version"] for row in migrations] == [database.SCHEMA_VERSION]
        finally:
            await database.close_database()

        assert database._connection is None

    asyncio.run(run())


def test_large_select_can_be_streamed_in_bounded_batches(tmp_path, monkeypatch):
    from backend.app import database

    async def run():
        await database.close_database()
        monkeypatch.setattr(database, "DB_PATH", tmp_path / "streamed.db")
        try:
            await database.initialize_database()
            await database.run_db_many(
                "INSERT INTO tasks (id,url,title) VALUES (?,?,?)",
                [(f"task-{index}", "https://example.test/file", str(index)) for index in range(1200)],
            )
            ids = [
                row["id"]
                async for row in database.iter_db_rows(
                    "SELECT id FROM tasks ORDER BY id",
                    batch_size=127,
                )
            ]
            assert len(ids) == 1200
            assert ids[0] == "task-0"
            assert ids[-1] == "task-999"
        finally:
            await database.close_database()

    asyncio.run(run())


def test_corrupt_database_is_restored_from_last_clean_backup(tmp_path, monkeypatch):
    from backend.app import database

    async def run():
        await database.close_database()
        db_path = tmp_path / "recover.db"
        monkeypatch.setattr(database, "DB_PATH", db_path)
        try:
            await database.initialize_database()
            await database.run_db(
                "INSERT INTO tasks (id,url,title) VALUES (?,?,?)",
                ("recoverable", "https://example.test/file", "kept"),
            )
            await database.close_database()
            backup, _, _ = database._backup_paths(db_path)
            assert backup.is_file()

            db_path.write_bytes(b"not a sqlite database")
            await database.initialize_database()
            rows = await database.run_db(
                "SELECT title FROM tasks WHERE id=?",
                ("recoverable",),
            )
            assert rows[0]["title"] == "kept"
        finally:
            await database.close_database()

    asyncio.run(run())
