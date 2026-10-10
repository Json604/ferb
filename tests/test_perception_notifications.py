import plistlib
import sqlite3
from pathlib import Path

import pytest

from ferb.sanitize.redact import Redactor
from ferb.sense.watchers.notifications import NotificationDatabase


def test_notification_database_is_read_only_and_reports_redacted_conversation(tmp_path: Path) -> None:
    path = tmp_path / "db"
    with sqlite3.connect(path) as connection:
        connection.executescript("CREATE TABLE app(app_id INTEGER, identifier TEXT); CREATE TABLE record(uuid BLOB, app_id INTEGER, data BLOB);")
        connection.execute("INSERT INTO app VALUES(1, 'com.whatsapp.WhatsApp')")
        payload = {"req": {"titl": "Message yourself", "body": "sk-ant-abcdefghijklmnopqrstuvwx", "thread": "self"}}
        connection.execute("INSERT INTO record VALUES(?,?,?)", (b"abc", 1, plistlib.dumps(payload)))
    reader = NotificationDatabase(path, Redactor())
    rows = reader.read()
    assert rows[0].data["conversation"] == "self" and rows[0].data["bundle_id"] == "com.whatsapp.WhatsApp"
    assert "sk-ant-" not in str(rows)
    assert path.exists() and not (tmp_path / "db-journal").exists()


def test_notification_missing_or_changed_schema_is_failure_not_empty_success(tmp_path: Path) -> None:
    with pytest.raises((OSError, sqlite3.Error)):
        NotificationDatabase(tmp_path / "missing", Redactor()).read()
    path = tmp_path / "db"
    with sqlite3.connect(path) as connection:
        connection.execute("CREATE TABLE unsupported(x)")
    with pytest.raises(RuntimeError, match="schema"):
        NotificationDatabase(path, Redactor()).read()
