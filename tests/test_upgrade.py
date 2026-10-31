"""Upgrading from any older FERB (notes/storage.md decision 5; step 12a): a store migrated by an older build, from
before checksums were recorded, with or without the deleted 0040, reaches the current schema with its data kept; the
journal's reply ref lists are stripped and its append-only trigger is back; the tombstone tables are merged; a database
newer than this build is still refused. A first start on an older store and an older config.toml upgrades both.
Fixtures are synthetic: the migration files as older builds applied them (unchanged since: the runner refuses edits)
and the deleted 0040 from git history (tests/fixtures/migrations)."""
import sqlite3
from pathlib import Path
from typing import Any

import pytest

from ferb.store import db as dbmod
from ferb.store.db import Database, MigrationError, _split_sql

FIXTURES = Path(__file__).parent / "fixtures" / "migrations"
AT = "2026-10-20T10:00:00+05:30"
VERSIONS = {int(f.name[:4]) for f in dbmod.MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql")}
NEWEST = max(VERSIONS)


def _older_build(path: Path, upto: int, with_0040: bool = False, checksums: bool = False) -> None:
    """A store as a build that knew migrations up to `upto` left it: applied by the runner before step 1 (a
    `schema_version` without checksums) unless `checksums`, with the deleted 0040 when `with_0040`, and a file from
    before incremental auto-vacuum."""
    files = {int(f.name[:4]): f for f in dbmod.MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql") if int(f.name[:4]) <= upto}
    if with_0040:
        files[40] = FIXTURES / "0040_identity_held.sql"
    c = sqlite3.connect(path)
    c.execute("PRAGMA auto_vacuum = NONE")
    c.execute("CREATE TABLE schema_version (version INTEGER NOT NULL" + (", sha256 TEXT)" if checksums else ")"))
    for version in sorted(files):
        for statement in _split_sql(files[version].read_text()):
            c.execute(statement)
        c.execute("INSERT INTO schema_version (version) VALUES (?)", (version,))
    c.commit()
    c.close()


def _tables(c: Any) -> set[str]:
    return {r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}


def _rows(path: Path) -> None:
    """What older builds wrote, in the tables each had: tasks, chat (a reply with its ref list), a memory fact, an
    episode, tombstones and rows of tables later builds dropped."""
    c = sqlite3.connect(path)
    have = _tables(c)
    c.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
              "updated_at) VALUES ('t1', 't1', 'r1', 'tea', 'owner', 'worker', '[]', 'done', ?, ?)", (AT, AT))
    c.execute("INSERT INTO journal (id, at, kind, data_json) VALUES ('j1', ?, 'chat.message', "
              "'{\"text\": \"chai \\u0915\\u092e\"}')", (AT,))
    c.execute("INSERT INTO journal (id, at, kind, data_json) VALUES ('j2', ?, 'chat.reply', "
              "'{\"text\": \"done\", \"source_refs\": [\"trail:a\", \"trail:b\", \"entry:j1\"]}')", (AT,))
    c.execute("INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, "
              "valid_from, created_at) VALUES ('m1', 'preference', 'owner', 'tea', 'green tea', 'owner_stated', "
              "'active', 1.0, ?, ?, ?)", (AT, AT, AT))
    if "learning_forgotten" in have:
        c.execute("INSERT INTO learning_forgotten VALUES ('key-abc', ?)", (AT,))
    if "conversation_forgotten_sources" in have:
        c.execute("INSERT INTO conversation_forgotten_sources VALUES ('trail:gone')")
    if "notification_rules" in have:
        c.execute("INSERT INTO notification_rules (id, app, owner_quote, created_at) VALUES ('n1', 'x', 'mute', ?)",
                  (AT,))
    c.commit()
    c.close()


def _fresh_tables(tmp_path: Path) -> set[str]:
    fresh = Database(tmp_path / "fresh.db")
    fresh.migrate()
    names = _tables(fresh.conn)
    fresh.close()
    return names


@pytest.mark.parametrize("upto,with_0040", [(18, False), (30, False), (40, True), (44, False), (48, False),
                                            (48, True), (52, False), (55, False)])
def test_a_store_from_an_older_build_reaches_the_current_schema_with_its_data(tmp_path: Path, upto: int,
                                                                              with_0040: bool) -> None:
    path = tmp_path / "ferb.db"
    _older_build(path, upto, with_0040, checksums=upto > 48)
    _rows(path)
    db = Database(path)
    try:
        applied = db.migrate()
        assert applied == sorted(v for v in VERSIONS if v > upto)
        assert db.schema_version() == NEWEST and db.migrate() == []
        assert _tables(db.conn) == _fresh_tables(tmp_path)               # the same tables as a new install
        c = db.conn
        assert c.execute("SELECT COUNT(*) FROM schema_version WHERE sha256 IS NULL AND version != 40").fetchone()[0] == 0
        assert c.execute("SELECT goal FROM tasks WHERE id = 't1'").fetchone()[0] == "tea"
        assert c.execute("SELECT content FROM memory_items WHERE id = 'm1'").fetchone()[0] == "green tea"
        import json
        reply = json.loads(c.execute("SELECT data_json FROM journal WHERE id = 'j2'").fetchone()[0])
        assert reply == {"text": "done"}                                 # the ref list is gone, the reply kept
        assert json.loads(c.execute("SELECT data_json FROM journal WHERE id = 'j1'").fetchone()[0])["text"] == "chai कम"
        with pytest.raises(sqlite3.IntegrityError, match="append-only"):
            c.execute("UPDATE journal SET kind = 'x' WHERE id = 'j1'")  # the trigger is back
        tombs = {r[0] for r in c.execute("SELECT ref FROM forgotten_refs")}
        assert ("key-abc" in tombs) == (upto >= 30) and ("trail:gone" in tombs) == (upto >= 33)
        for gone in ("learning_forgotten", "conversation_forgotten_sources"):     # merged: their rows kept above
            assert gone not in _tables(c)
        # tables no code reads stay, with their rows, until the owner's OK'd clean-up drops them (decision 4)
        assert {"owner_questions", "notification_rules", "learning_receipts", "learning_jobs"} <= _tables(c)
        assert c.execute("SELECT COUNT(*) FROM notification_rules").fetchone()[0] == (1 if upto >= 32 else 0)
    finally:
        db.close()


def test_a_database_newer_than_this_build_is_still_refused(tmp_path: Path) -> None:
    path = tmp_path / "ferb.db"
    db = Database(path)
    db.migrate()
    db.conn.execute("INSERT INTO schema_version (version, sha256) VALUES (?, 'x')", (NEWEST + 1,))
    with pytest.raises(MigrationError, match="newer than this FERB"):
        db.migrate()
    db.close()


OLD_CONFIG = """\
[device]
id = "mac-test"

[setup]
permissions_version = 27
persona_reviewed = true

[setup.persona_pending]
scanned_at = "2026-10-20T10:00:00+05:30"
summary = "Uses a browser and an editor."

[gates]
owner_profiles = ["Default"]

[policy]
trusted_accounts = ["someone@example.com"]

[browser]
download_max_mb = 100
task_max_tabs = 20

[retention]
trace_days = 30
download_days = 3
"""


def test_the_first_start_on_an_older_store_and_config_upgrades_both(home: Path) -> None:
    """A schema-48 store (the owner's build before this rebuild) with the 0040 column and an older config.toml: the
    daemon starts, applies every migration, upgrades the config once (journaled), adds the new retention default, keeps
    the keys it no longer reads, moves the persona proposal out of config.toml when it is read, and works with the
    tables no code reads still there; the clean-up's preview lists them with their rows."""
    from ferb.config import Config
    from ferb.device.persona import PersonaState
    from ferb.store import cleanup
    from tests.support.schedules import own_daemon as _own_daemon
    home.mkdir(parents=True)
    home.chmod(0o700)
    _older_build(home / "ferb.db", 48, with_0040=True)
    _rows(home / "ferb.db")
    (home / "config.toml").write_text(OLD_CONFIG)
    (home / "config.toml").chmod(0o600)
    d = _own_daemon(home)
    try:
        async def read() -> dict[str, Any]:
            dd = d.daemon
            return {"schema": dd.db.schema_version(), "upgraded": [e.data for e in
                                                                   dd._journal().of_kind("config.upgraded")],
                    "memory": [e.kind for e in dd._journal().entries_of("memory.cleanup", "memory.profile_seeded")],
                    "pending": PersonaState(dd.config).pending()}
        got = d.run(read())
    finally:
        d.stop()
    assert got["schema"] == NEWEST
    [upgraded] = got["upgraded"]
    assert {c["key"] for c in upgraded["changes"]} >= {"browser.download_max_mb", "browser.task_max_tabs",
                                                        "retention.download_days"}
    assert got["memory"] == ["memory.cleanup", "memory.profile_seeded"]
    assert got["pending"]["summary"] == "Uses a browser and an editor."  # moved to persona.json when read
    config = Config(home)
    assert config.get("retention", "operational_days") == 90 and config.get("retention", "download_days") is None
    assert config.get("browser", "download_max_mb") == 512 and config.get("setup", "persona_pending") is None
    assert config.get("gates", "owner_profiles") == ["Default"]          # a key no longer read is left alone
    assert (home / "persona.json").exists()
    shown = cleanup.preview(home / "ferb.db")
    assert shown["compact"] is True and shown["ok"]
    drops = {s["table"]: s["rows"] for s in shown["steps"] if s.get("drop")}
    assert drops == {"owner_questions": 0, "notification_rules": 1, "learning_receipts": 0, "learning_jobs": 0}


def test_the_memory_upgrade_on_an_older_store_runs_once(home: Path) -> None:
    """Tier 1 seeded once from the facts an older store holds; a second start changes nothing (the embeddings are
    filled in the background once the model is there: tests/test_memory_search.py)."""
    from tests.support.schedules import own_daemon as _own_daemon
    home.mkdir(parents=True)
    _older_build(home / "ferb.db", 44)
    _rows(home / "ferb.db")
    for ids in ("id", "again"):
        d = _own_daemon(home, ids=ids)
        try:
            async def read() -> tuple[int, list[str]]:
                dd = d.daemon
                return len(dd._journal().of_kind("memory.profile_seeded")), dd.profile.ids()
            seeded, profile = d.run(read())
        finally:
            d.stop()
        assert seeded == 1 and profile


def test_jev_stays_off_after_an_upgrade_even_with_a_key(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """notes/daemon.md decision 9: an older config that had Jev questions enabled, with a Jev key in the Keychain (faked:
    the Keychain is never touched), starts with Jev off: no decider, no Keychain read, no request to Jev, also after a
    reload; the switch is the only thing that turns it on."""
    from ferb.daemon.areas.jev import JevGlue
    from tests.support.schedules import own_daemon as _own_daemon
    asked: list[str] = []

    async def key(self: Any) -> str:
        asked.append("keychain")
        return "jev-test-key"
    monkeypatch.setattr(JevGlue, "_jev_key", key)
    home.mkdir(parents=True)
    home.chmod(0o700)
    _older_build(home / "ferb.db", 48, with_0040=True)
    (home / "config.toml").write_text(OLD_CONFIG + '\n[jev]\nenabled = ["action.destructive", "trail.attention"]\n')
    (home / "config.toml").chmod(0o600)
    d = _own_daemon(home)
    try:
        async def state() -> tuple[Any, bool, dict[str, Any]]:
            reloaded = await d.daemon._jev_reload(None, {"ping": True})        # type: ignore[arg-type]
            return d.daemon.jev, d.daemon._jev_on(), reloaded
        jev, on, reloaded = d.run(state())
    finally:
        d.stop()
    assert jev is None and on is False and reloaded == {"configured": False} and asked == []
