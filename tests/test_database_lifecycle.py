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

            assert id(database._connection) == connection_id
            assert calls == 1
            assert rows[0]["title"] == "two"
            assert str(journal[0][0]).lower() == "wal"
        finally:
            await database.close_database()

        assert database._connection is None

    asyncio.run(run())
