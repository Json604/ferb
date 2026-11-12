"""FERB reinstalled over what an older FERB left in its home (owner, 2026-11-01: "if files are broken it deletes and
starts new; if nothing's broken it proceeds normally"; kb area daemon). Program files and FERB's own state are
rewritten by setup; the owner's data that can't be read is moved aside with a timestamp, never deleted, journaled and
told once; a database from a newer FERB is refused unchanged. Temporary homes and fake bundles only."""

import hashlib
import socket
import sqlite3
from datetime import datetime
from pathlib import Path
from typing import Any

import pytest

from ferb.store import salvage
from ferb.store.db import Database, NewerStore, known_schema
from tests.support.app_bundle import PINNED, BundleRunner, make_ferb_app

NOW = datetime(2026, 11, 1, 2, 15)


def _home(home: Path) -> Path:
    home.mkdir(parents=True, exist_ok=True)
    home.chmod(0o700)
    return home


def _store(home: Path, note: str = "kept") -> Path:
    """A store at the current schema holding one recognisable row, closed (its WAL checkpointed)."""
    db = Database(home / "ferb.db")
    db.migrate()
    db.conn.execute("CREATE TABLE owner_note (text TEXT)")
    db.conn.execute("INSERT INTO owner_note VALUES (?)", (note,))
    db.close()
    return home / "ferb.db"


def _truncate(path: Path) -> None:
    """Cut the file short at a page boundary past the header, as a crash or a bad copy leaves it."""
    data = path.read_bytes()
    path.write_bytes(data[:len(data) // 2 // 4096 * 4096 or 4096])


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


# ---------- the owner's data ----------

# decision: STORE-1
@pytest.mark.parametrize("damage", ["garbage", "truncated"])
def test_a_damaged_database_is_moved_aside_with_its_wal_never_deleted(tmp_path: Path, damage: str) -> None:
    home = _home(tmp_path / ".ferb")
    path = _store(home)
    for _ in range(40):                                          # enough pages that cutting it in half loses some
        db = sqlite3.connect(path)
        db.execute("INSERT INTO owner_note VALUES (?)", ("x" * 3000,))
        db.commit()
        db.close()
    if damage == "garbage":
        path.write_bytes(b"this is not a database at all\n" * 200)
    else:
        _truncate(path)
    (home / "ferb.db-wal").write_bytes(b"")
    (home / "ferb.db-shm").write_bytes(b"\0" * 32768)
    before = _sha(path)
    moved = salvage.check_home(home, "build-1", lambda: NOW)
    assert [m["file"] for m in moved] == ["ferb.db"] and moved[0]["why"].startswith(("SQLite", "SQLite's"))
    aside = home / "ferb.db.broken-20261101-0215"
    assert moved[0]["moved_to"] == str(aside) and _sha(aside) == before          # the same bytes, kept
    assert (home / "ferb.db.broken-20261101-0215-wal").exists() and (home / "ferb.db.broken-20261101-0215-shm").exists()
    assert not path.exists() and not (home / "ferb.db-wal").exists() and not (home / "ferb.db-shm").exists()
    assert salvage.pending(home) == moved                                        # until the daemon tells the owner
    assert not (home / salvage.CHECKED).exists()                                 # no build passed a check


# decision: STORE-1
def test_a_second_set_aside_in_the_same_minute_never_overwrites_the_first(tmp_path: Path) -> None:
    home = _home(tmp_path / ".ferb")
    for n in (1, 2):
        (home / "ferb.db").write_bytes(b"garbage %d" % n * 500)
        salvage.check_store(home, now=lambda: NOW)
    assert (home / "ferb.db.broken-20261101-0215").read_bytes().startswith(b"garbage 1")
    assert (home / "ferb.db.broken-20261101-0215-2").read_bytes().startswith(b"garbage 2")
    assert len(salvage.pending(home)) == 2


# decision: STORE-4
def test_a_readable_database_is_kept_and_the_full_check_runs_once_per_build(tmp_path: Path,
                                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    home = _home(tmp_path / ".ferb")
    path = _store(home)
    ran: list[str] = []
    check = salvage._check
    monkeypatch.setattr(salvage, "_check", lambda conn, pragma: ran.append(pragma) or check(conn, pragma))
    for build in ("build-1", "build-1", "build-2"):
        assert salvage.check_home(home, build, lambda: NOW) == []
    assert ran == ["quick_check", "integrity_check", "quick_check", "quick_check", "integrity_check"]
    assert salvage._checked(home) == "build-2" and path.exists() and not list(home.glob("*.broken-*"))
    assert sqlite3.connect(path).execute("SELECT text FROM owner_note").fetchone() == ("kept",)


# decision: STORE-4
def test_a_locked_database_is_not_judged_damaged(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Only SQLite's damage codes move a file: a lock (setup's clean-up holding it) is raised as it is."""
    home = _home(tmp_path / ".ferb")
    path = _store(home)
    monkeypatch.setattr(salvage, "BUSY_TIMEOUT_MS", 50)
    holder = sqlite3.connect(path, isolation_level=None)
    holder.execute("PRAGMA journal_mode=DELETE")
    holder.execute("PRAGMA locking_mode=EXCLUSIVE")
    holder.execute("BEGIN EXCLUSIVE")
    try:
        with pytest.raises(sqlite3.OperationalError, match="locked"):
            salvage.check_store(home, now=lambda: NOW)
    finally:
        holder.execute("ROLLBACK")
        holder.close()
    assert path.exists() and salvage.pending(home) == []


# decision: STORE-3, DMN-10
def test_a_database_from_a_newer_ferb_is_refused_and_nothing_changes(tmp_path: Path) -> None:
    home = _home(tmp_path / ".ferb")
    path = _store(home)
    db = sqlite3.connect(path)
    db.execute("INSERT INTO schema_version (version, sha256) VALUES (?, 'x')", (known_schema() + 1,))
    db.commit()
    db.close()
    (home / "config.toml").write_text("[broken\n")                     # even a broken config stays: nothing changes
    before = (_sha(path), (home / "config.toml").read_text())
    with pytest.raises(NewerStore, match="newer FERB"):
        salvage.check_home(home, "build-1", lambda: NOW)
    with pytest.raises(NewerStore, match="Install the newer FERB"):
        salvage.refuse_newer(home)
    assert (_sha(path), (home / "config.toml").read_text()) == before
    assert not list(home.glob("*.broken-*")) and salvage.pending(home) == []


# decision: STORE-3
def test_setup_refuses_a_newer_database_before_it_installs_anything(tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
                                                                    capsys: pytest.CaptureFixture[str]) -> None:
    from ferb.cli import main
    home = _home(tmp_path / ".ferb")
    path = _store(home)
    db = sqlite3.connect(path)
    db.execute("INSERT INTO schema_version (version, sha256) VALUES (?, 'x')", (known_schema() + 1,))
    db.commit()
    db.close()
    monkeypatch.setenv("FERB_HOME", str(home))
    import ferb.cli.setup as setup_cmd
    monkeypatch.setattr(setup_cmd, "installer", lambda *a, **k: pytest.fail("setup went on to install"))
    assert main(["setup", "--service"]) == 1
    assert "newer FERB" in capsys.readouterr().err                     # FERB.app's setup window shows this line
    assert {p.name for p in home.iterdir()} <= {"ferb.db", "ferb.db-wal", "ferb.db-shm"}   # SQLite's own, empty


# decision: STORE-1, DMN-10
def test_an_unparseable_config_and_secret_names_are_set_aside_and_persona_is_rescanned(tmp_path: Path) -> None:
    home = _home(tmp_path / ".ferb")
    (home / "config.toml").write_text("[runtimes]\nworker = \"codex\n")
    (home / "config.toml").chmod(0o600)
    (home / "owner-secret-handles.json").write_text('{"version": 2, "records": {')
    (home / "persona.json").write_text("{not json")
    moved = salvage.check_files(home, lambda: NOW)
    assert {m["file"] for m in moved} == {"config.toml", "owner-secret-handles.json"}
    assert (home / "config.toml.broken-20261101-0215").read_text() == "[runtimes]\nworker = \"codex\n"
    assert (home / "owner-secret-handles.json.broken-20261101-0215").exists()
    assert not (home / "config.toml").exists() and not (home / "persona.json").exists()   # FERB's own: scanned again
    lines = salvage.describe(moved)
    assert any("config.toml" in line and "config.toml.broken-20261101-0215" in line for line in lines)
    # readable files of an older build are used as they are, a newer form of the secret names included
    (home / "config.toml").write_text('[device]\nid = "mac"\n')
    (home / "owner-secret-handles.json").write_text('{"version": 9}')
    assert salvage.check_files(home, lambda: NOW) == []


# decision: STORE-1, STORE-2, DMN-10
def test_the_daemon_sets_a_damaged_store_and_config_aside_starts_fresh_journals_and_tells_once(home: Path) -> None:
    from tests.support.schedules import own_daemon
    _home(home)
    (home / "ferb.db").write_bytes(b"SQLite format 3\0" + b"\xff" * 8000)   # a header, then garbage
    (home / "config.toml").write_text("[privacy\nexclude_apps = [")
    (home / "config.toml").chmod(0o600)
    d = own_daemon(home)
    try:
        async def read() -> dict[str, Any]:
            dd = d.daemon
            return {"journal": [e.data for e in dd._journal().of_kind("home.set_aside")],
                    "notices": dd.attention_notices.unseen()}
        got = d.run(read())
    finally:
        d.stop()
    assert {e["file"] for e in got["journal"]} == {"ferb.db", "config.toml"}
    texts = [n["text"] for n in got["notices"]]
    assert len(texts) == 2 and all("moved the file aside" in t and "nothing was deleted" in t for t in texts)
    assert any("its database" in t for t in texts) and any("config.toml" in t for t in texts)
    assert len(list(home.glob("ferb.db.broken-*"))) == 1 and len(list(home.glob("config.toml.broken-*"))) == 1
    assert salvage.pending(home) == [] and (home / "config.toml").exists()        # told; a fresh config written
    again = own_daemon(home, ids="again")                                         # a restart tells nothing new
    try:
        notices = again.run(_unseen(again))
        journal = again.run(_set_aside_rows(again))
    finally:
        again.stop()
    assert len(notices) == 2 and len(journal) == 2


async def _unseen(d: Any) -> list[dict[str, Any]]:
    return list(d.daemon.attention_notices.unseen())


async def _set_aside_rows(d: Any) -> list[Any]:
    return list(d.daemon._journal().of_kind("home.set_aside"))


# decision: STORE-3
def test_the_daemon_refuses_a_newer_store_unchanged_and_exits_cleanly(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from tests.conftest import RunningDaemon
    _home(home)
    path = _store(home)
    db = sqlite3.connect(path)
    db.execute("INSERT INTO schema_version (version, sha256) VALUES (?, 'x')", (known_schema() + 1,))
    db.commit()
    db.close()
    before = _sha(path)
    with pytest.raises(NewerStore):
        RunningDaemon(home, watch_owner_input=False)
    assert _sha(path) == before and not (home / "config.toml").exists()

    import ferb.daemon.main as daemon_main                                        # `main` exits 0: no launchd loop
    class Refusing:
        _redact_text = staticmethod(lambda text: text)

        async def run(self) -> None:
            raise NewerStore("FERB's database was last used by a newer FERB")
    monkeypatch.setattr(daemon_main, "Daemon", Refusing)
    monkeypatch.setattr(daemon_main, "service_streams", lambda home: None)
    monkeypatch.setattr(daemon_main, "redacting_hooks", lambda *a, **k: None)
    daemon_main.main()                                                            # returns: a clean exit


# decision: DMN-10
def test_leftover_run_files_of_an_older_daemon_are_cleared_at_start(home: Path) -> None:
    """A socket and sandbox profiles a crashed daemon left; FERB's own catalog dump cut short."""
    from tests.support.schedules import own_daemon
    _home(home)
    run = home / "run"
    (run / "sandbox").mkdir(parents=True)
    stale = socket.socket(socket.AF_UNIX)
    stale.bind(str(run / "ferb.sock"))
    stale.close()
    (run / "sandbox" / "0123456789abcdef.sb").write_text("(version 1)")
    d = own_daemon(home)
    try:
        assert d.run(_answers(d))
    finally:
        d.stop()
    assert not list((run / "sandbox").glob("*.sb"))


async def _answers(d: Any) -> bool:
    return d.daemon.server._server is not None


def test_a_cut_short_codex_catalog_dump_is_dumped_again(tmp_path: Path) -> None:
    from ferb.runtimes import codex_exec
    home = _home(tmp_path / ".ferb")
    binary = home / "node" / codex_exec._NATIVE_BIN
    binary.parent.mkdir(parents=True)
    binary.write_text("#!/bin/sh\necho '{\"models\": [{\"slug\": \"m\"}]}'\n")
    binary.chmod(0o755)
    dump = codex_exec._bundled_path(home)
    dump.parent.mkdir(parents=True)
    dump.write_text('{"models": [{"slu')
    assert codex_exec.bundled_catalog(home) == dump
    assert dump.read_text() == '{"models": [{"slug": "m"}]}'


# ---------- program files ----------

# decision: DMN-10
def test_reinstall_over_an_older_builds_leftovers_rewrites_every_program_file(tmp_path: Path) -> None:
    """Uninstalled (or deleted) without purge, then a new FERB.app opened: the helper missing or not the one the app
    seals, a stale record naming the older build and app, the `ferb` link and the Codex link into the deleted app, an
    older build's LaunchAgent: setup recreates or rewrites each; the owner's data stays."""
    import json
    import plistlib
    from ferb.device.install import Installer
    app = make_ferb_app(tmp_path / "Applications", build="202611010900")
    home = _home(tmp_path / ".ferb")
    (home / "config.toml").write_text('[device]\nid = "mac"\n')
    (home / "config.toml").chmod(0o600)
    _store(home)
    gone = tmp_path / "Downloads" / "FERB.app"                                   # the older build's app, deleted
    (home / "service.json").write_text(json.dumps({"build": "202610140201", "app": str(gone), "label": "com.ferb.test"}))
    (home / "bin").mkdir()
    (home / "bin" / "ferb-secrets").write_bytes(b"an older helper, another signature")
    (home / "bin" / "ferb-secrets.requirement").write_text('cdhash H"0000000000000000000000000000000000000000"\n')
    (tmp_path / "bin").mkdir()
    (tmp_path / "bin" / "ferb").symlink_to(gone / "Contents/Resources/bin/ferb")
    (home / "node").symlink_to(gone / "Contents/Resources/node")
    agents = tmp_path / "LaunchAgents"
    agents.mkdir()
    (agents / "com.ferb.test.plist").write_bytes(plistlib.dumps({"Label": "com.ferb.test",
                                                                 "ProgramArguments": [str(gone / "x")]}))
    from ferb.runtimes.codex_exec import CODEX_VERSION
    inst = Installer(home, label="com.ferb.test", run=BundleRunner(home, codex_version=f"codex-cli {CODEX_VERSION}"),
                     launch_agents=agents, bundle=app, cli_dir=tmp_path / "bin",
                     native_hosts_dirs=(tmp_path / "Chrome" / "NativeMessagingHosts",), extension_ids=("abc",))
    assert inst.installed()                                                      # an update, as FERB.app runs it
    inst.install(update_only=True)
    assert (home / "bin" / "ferb-secrets").read_bytes() == (app / "Contents/Helpers/ferb-secrets").read_bytes()
    assert (home / "bin" / "ferb-secrets.requirement").read_text().strip() == PINNED
    assert {k: v for k, v in inst.recorded().items() if k != "definition"} == {
        "build": "202611010900", "app": str(app), "label": "com.ferb.test", "signer": "adhoc",
        "service": "launch_agent"}
    assert (tmp_path / "bin" / "ferb").resolve() == (app / "Contents/Resources/bin/ferb").resolve()
    assert (home / "node").resolve() == (app / "Contents/Resources/node").resolve()
    assert inst.definition_current()                                             # this build's LaunchAgent
    assert json.loads((tmp_path / "Chrome" / "NativeMessagingHosts" / "com.ferb.native_host.json").read_text())["path"] \
        == str(home / "bin" / "ferb-native-host")
    assert str(app) in (home / "bin" / "ferb-native-host").read_text()
    assert (home / "config.toml").read_text() == '[device]\nid = "mac"\n'          # the owner's data untouched
    assert sqlite3.connect(home / "ferb.db").execute("SELECT text FROM owner_note").fetchone() == ("kept",)
    removed = inst.uninstall()                                                   # without purge: the record goes too
    assert not (home / "service.json").exists() and str(home / "service.json") in removed
    assert (home / "ferb.db").exists() and (home / "config.toml").exists()
