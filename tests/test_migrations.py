"""The migration runner refuses what it used to skip silently: a new file numbered at or below the newest applied one,
two files with one number, an applied file edited afterwards, and a database newer than the code."""

from pathlib import Path

import pytest

from ferb.store.db import Database, MigrationError


def _write(folder: Path, name: str, sql: str) -> None:
    folder.mkdir(exist_ok=True)
    (folder / name).write_text(sql)


# decision: STORE-414
def test_applies_in_order_and_records_each_file(tmp_path: Path) -> None:
    m = tmp_path / "m"
    _write(m, "0001_a.sql", "CREATE TABLE a (x INTEGER);")
    _write(m, "0002_b.sql", "CREATE TABLE b (x INTEGER);")
    d = Database(tmp_path / "ferb.db")
    assert d.migrate(m) == [1, 2]
    assert d.migrate(m) == []
    assert d.schema_version() == 2


# decision: STORE-414
def test_a_new_file_numbered_below_the_newest_applied_is_refused_not_skipped(tmp_path: Path) -> None:
    m = tmp_path / "m"
    _write(m, "0001_a.sql", "CREATE TABLE a (x INTEGER);")
    _write(m, "0003_c.sql", "CREATE TABLE c (x INTEGER);")
    d = Database(tmp_path / "ferb.db")
    d.migrate(m)
    _write(m, "0002_b.sql", "CREATE TABLE b (x INTEGER);")
    with pytest.raises(MigrationError, match="0002"):
        d.migrate(m)
    assert d.conn.execute("SELECT name FROM sqlite_master WHERE name = 'b'").fetchone() is None


# decision: STORE-414
def test_two_files_with_one_number_are_refused(tmp_path: Path) -> None:
    m = tmp_path / "m"
    _write(m, "0001_a.sql", "CREATE TABLE a (x INTEGER);")
    _write(m, "0001_b.sql", "CREATE TABLE b (x INTEGER);")
    with pytest.raises(MigrationError, match="0001"):
        Database(tmp_path / "ferb.db").migrate(m)


# decision: STORE-414
def test_an_applied_file_edited_afterwards_is_refused(tmp_path: Path) -> None:
    m = tmp_path / "m"
    _write(m, "0001_a.sql", "CREATE TABLE a (x INTEGER);")
    d = Database(tmp_path / "ferb.db")
    d.migrate(m)
    _write(m, "0001_a.sql", "CREATE TABLE a (x INTEGER, y TEXT);")
    with pytest.raises(MigrationError, match="0001"):
        d.migrate(m)


# decision: STORE-414
def test_a_database_newer_than_the_code_is_refused(tmp_path: Path) -> None:
    m = tmp_path / "m"
    _write(m, "0001_a.sql", "CREATE TABLE a (x INTEGER);")
    _write(m, "0002_b.sql", "CREATE TABLE b (x INTEGER);")
    d = Database(tmp_path / "ferb.db")
    d.migrate(m)
    (m / "0002_b.sql").unlink()
    with pytest.raises(MigrationError, match="newer"):
        d.migrate(m)


# decision: STORE-414, TEST-505
def test_rows_from_the_old_runner_are_accepted_and_get_their_checksum(tmp_path: Path) -> None:
    """An install migrated before checksums existed: its schema_version has versions only. The runner accepts it,
    records each present file's checksum, and from then on an edit is caught. A version whose file no longer exists
    (0040 was added and removed on 2026-10-19/20) is fine as long as it isn't the newest."""
    m = tmp_path / "m"
    _write(m, "0001_a.sql", "CREATE TABLE a (x INTEGER);")
    _write(m, "0003_c.sql", "CREATE TABLE c (x INTEGER);")
    d = Database(tmp_path / "ferb.db")
    d.conn.execute("CREATE TABLE schema_version (version INTEGER NOT NULL)")
    d.conn.execute("CREATE TABLE a (x INTEGER)")
    d.conn.execute("CREATE TABLE c (x INTEGER)")
    d.conn.executemany("INSERT INTO schema_version (version) VALUES (?)", [(1,), (2,), (3,)])
    assert d.migrate(m) == []
    _write(m, "0003_c.sql", "CREATE TABLE c (x INTEGER, y TEXT);")
    with pytest.raises(MigrationError, match="0003"):
        d.migrate(m)


# decision: STORE-414
def test_a_failing_migration_leaves_nothing_behind(tmp_path: Path) -> None:
    m = tmp_path / "m"
    _write(m, "0001_a.sql", "CREATE TABLE a (x INTEGER);\nTHIS IS NOT SQL;")
    d = Database(tmp_path / "ferb.db")
    with pytest.raises(Exception):
        d.migrate(m)
    assert d.schema_version() == 0
    assert d.conn.execute("SELECT name FROM sqlite_master WHERE name = 'a'").fetchone() is None
