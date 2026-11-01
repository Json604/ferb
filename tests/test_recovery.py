"""FERB's file changes are recoverable for 7 days (S1): a clone before an overwrite/edit, the original path kept for a
trash, restore (never over a change the owner made since), fallback when clonefile is unavailable, the retention purge,
`ferb restore`, and one local APFS snapshot per task tree before shell or delete work (best effort).

Offline: the Trash is a folder beside the test home (tests.support.trash.use_trash), and no real tmutil snapshot is ever
taken (the test daemon has no snapshot hook; local_snapshot's own tests replace subprocess.run)."""

import asyncio
import io
import json
import os
import stat
import subprocess
import threading
from datetime import timedelta
from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel

from ferb.act.fs import files, recovery
from ferb.cli import admin
from ferb.cli.client import Client, RpcError
from ferb.core.types import ActionSpec, Target, ToolContext, ToolResult
from ferb.policy.rules import Kind
from tests.conftest import FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.confirm import FerbAnswers
from tests.support.trash import use_trash


# ---------- helpers ----------

def _ws(daemon: RunningDaemon) -> Path:
    ws = daemon.daemon.home / "workspace"
    ws.mkdir(parents=True, exist_ok=True)
    return ws


def _root(daemon: RunningDaemon) -> Path:
    return daemon.daemon.home / "recovery"


def _write(daemon: RunningDaemon, grant: Any, path: Path, text: str, overwrite: bool = True) -> dict[str, Any]:
    return gateway_tool_call(daemon, grant, "fs.write", path=str(path), content=text, overwrite=overwrite,
                             effect="edit")


def _fs_grant(daemon: RunningDaemon) -> Any:
    """A grant with the real fs tools' scopes (the daemon registers them), so the real tool paths run."""
    scopes = set().union(*(c.scopes for c in daemon.daemon.gateway.caps.values()
                           if c.name in ("fs.write", "fs.edit", "fs.trash")))
    grant, _ = gateway_setup(daemon, scopes, gateway_request("save my notes"), [])
    return grant


def _recovery_rows(daemon: RunningDaemon) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) for r in daemon.daemon.db.conn.execute("SELECT * FROM recovery ORDER BY at, rowid")]
    return daemon.run(go())


def _restore(daemon: RunningDaemon, record_id: str) -> Path:
    async def go() -> Path:
        return recovery.restore(record_id, daemon.daemon.db, _root(daemon))
    return daemon.run(go())


def _sql(daemon: RunningDaemon, sql: str, params: tuple[Any, ...] = ()) -> None:
    async def go() -> None:
        daemon.daemon.db.conn.execute(sql, params)
    daemon.run(go())


def _copies(daemon: RunningDaemon) -> list[Path]:
    root = _root(daemon)
    return sorted(p for p in root.rglob("*") if p.is_file()) if root.exists() else []


def _spans(daemon: RunningDaemon, name: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) for r in daemon.daemon.db.conn.execute(
            "SELECT * FROM spans WHERE kind = ? ORDER BY rowid", (name,))]
    return daemon.run(go())


@pytest.fixture
def trash(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> Path:
    """A Trash folder beside the test's FERB home (never inside it: FsTrash puts nothing into FERB's own state, rule
    10): offline tests never touch the owner's real Trash."""
    bin_ = daemon.daemon.home.parent / ".Trash"
    bin_.mkdir(parents=True, exist_ok=True)
    use_trash(monkeypatch, bin_)
    return bin_


# ---------- clone before a change ----------

def test_an_overwrite_keeps_a_recoverable_copy(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "notes.txt"
    f.write_text("the original")
    grant = _fs_grant(daemon)
    out = _write(daemon, grant, f, "the replacement")
    assert not out["is_error"] and f.read_text() == "the replacement"
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "clone" and rec["original"] == str(f.resolve()) and rec["reason"] == "overwrite"
    assert rec["task_id"] == grant.task_id
    assert out["content"]["recoverable"] == rec["id"]                     # the worker and the trace see it
    span = _spans(daemon, "tool.call")[-1]
    assert json.loads(span["output_json"])["recoverable"] == rec["id"]
    restored = _restore(daemon, rec["id"])
    assert restored.read_text() == "the original" and restored.resolve() == f.resolve()


def test_an_edit_keeps_a_recoverable_copy(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "todo.txt"
    f.write_text("buy milk")
    grant = _fs_grant(daemon)
    out = gateway_tool_call(daemon, grant, "fs.edit", path=str(f), old_text="milk", new_text="bread")
    assert not out["is_error"] and f.read_text() == "buy bread"
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "clone" and rec["reason"] == "edit"
    assert _restore(daemon, rec["id"]).read_text() == "buy milk"


def test_a_brand_new_file_keeps_no_copy(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "fresh.txt"
    grant = _fs_grant(daemon)
    out = _write(daemon, grant, f, "new")
    assert not out["is_error"] and f.read_text() == "new"
    assert _recovery_rows(daemon) == [] and _copies(daemon) == []      # nothing was overwritten
    assert "recoverable" not in out["content"]


def test_a_call_that_does_not_change_the_file_keeps_no_copy(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "keep.txt"
    f.write_text("one one")
    grant = _fs_grant(daemon)
    refused = _write(daemon, grant, f, "other", overwrite=False)          # no overwrite: refused, never copied
    assert refused["is_error"] and f.read_text() == "one one"
    no_match = gateway_tool_call(daemon, grant, "fs.edit", path=str(f), old_text="one", new_text="two")
    assert no_match["is_error"] and f.read_text() == "one one"            # two matches: the edit refused
    assert _recovery_rows(daemon) == [] and _copies(daemon) == []          # its copy and row were dropped


def test_recovery_falls_back_when_clone_is_unavailable(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.act.fs.recovery as rec_mod

    def no_clone(src: Path, dst: Path) -> None:
        raise OSError(18, "cross-device")                                   # EXDEV: another volume
    monkeypatch.setattr(rec_mod, "_clone", no_clone)
    f = _ws(daemon) / "x.txt"
    f.write_text("before")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "after")
    rec = _recovery_rows(daemon)[-1]
    assert _restore(daemon, rec["id"]).read_text() == "before"             # the copy fallback preserved it


def test_no_copy_means_no_change(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """Never lose the original: when neither a clone nor a byte copy can be made, the file is not changed."""
    import ferb.act.fs.recovery as rec_mod

    def fail(src: Path, dst: Path) -> None:
        raise OSError(28, "No space left on device")
    monkeypatch.setattr(rec_mod, "_clone", fail)
    monkeypatch.setattr(rec_mod, "_byte_copy", fail)
    f = _ws(daemon) / "precious.txt"
    f.write_text("only copy")
    grant = _fs_grant(daemon)
    out = _write(daemon, grant, f, "replacement")
    assert out["is_error"] and "recovery copy" in out["content"]["error"]
    assert f.read_text() == "only copy" and _recovery_rows(daemon) == [] and _copies(daemon) == []

    async def ledger() -> list[str]:
        return [r["state"] for r in daemon.daemon.db.conn.execute(
            "SELECT state FROM action_ledger WHERE capability = 'fs.write'")]
    assert daemon.run(ledger()) == ["failed"]                              # nothing ran: a retry isn't blocked


def test_an_unknown_outcome_keeps_the_copy(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """A write that raised something unexpected may or may not have changed the file: the copy stays."""
    f = _ws(daemon) / "half.txt"
    f.write_text("before")

    def boom(p: Path, text: str, closed: Any = None) -> None:
        raise RuntimeError("the writer broke")
    monkeypatch.setattr(files, "atomic_text", boom)
    grant = _fs_grant(daemon)
    out = _write(daemon, grant, f, "after")
    assert out["is_error"] and "outcome unknown" in out["content"]["error"]
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "clone" and rec["written_sha256"] is None
    assert len(_copies(daemon)) == 1


def test_an_os_error_is_a_plain_failure_and_drops_the_unneeded_copy(daemon: RunningDaemon,
                                                                    monkeypatch: pytest.MonkeyPatch) -> None:
    """Hands fix list: an OS refusal before the file changed is the tool's plain error (nothing was written), not an
    outcome the ledger must treat as unknown; the file is still what the copy holds, so the copy goes."""
    f = _ws(daemon) / "half.txt"
    f.write_text("before")

    def boom(p: Path, text: str, closed: Any = None) -> None:
        raise OSError(28, "No space left on device")
    monkeypatch.setattr(files, "atomic_text", boom)
    out = _write(daemon, _fs_grant(daemon), f, "after")
    error = out["content"]["error"]
    assert out["is_error"] and "outcome unknown" not in error and "No space left" in error and "nothing" in error
    assert f.read_text() == "before" and _copies(daemon) == []


def test_copies_live_inside_the_recovery_root_owner_only(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "odd name.weird ext!"
    f.write_text("a")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "b")
    root = _root(daemon).resolve()
    [copy] = _copies(daemon)
    assert copy.resolve().is_relative_to(root) and copy.read_text() == "a"
    assert (root.stat().st_mode & 0o077) == 0                              # ~/.ferb/recovery: owner only
    rec = _recovery_rows(daemon)[-1]
    assert not Path(rec["saved_to"]).is_absolute() and (root / rec["saved_to"]).resolve() == copy.resolve()


# ---------- restore never overwrites the owner's later change ----------

def test_restore_leaves_a_file_the_owner_changed_since_and_puts_the_copy_beside_it(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "notes.txt"
    f.write_text("the original")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "FERB's version")
    f.write_text("the owner's newer words")                               # changed after FERB's write
    rec = _recovery_rows(daemon)[-1]
    restored = _restore(daemon, rec["id"])
    assert f.read_text() == "the owner's newer words"                     # never overwritten
    assert restored.name == "notes (before FERB).txt" and restored.read_text() == "the original"
    again = _restore(daemon, rec["id"])                                   # a second restore never replaces the first
    assert again.name == "notes (before FERB 2).txt" and restored.read_text() == "the original"


def test_restore_puts_back_a_file_that_was_deleted_since(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "gone.txt"
    f.write_text("the original")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "FERB's version")
    f.unlink()
    rec = _recovery_rows(daemon)[-1]
    assert _restore(daemon, rec["id"]).resolve() == f.resolve() and f.read_text() == "the original"


def test_restore_accepts_a_unique_prefix_and_refuses_unknown_ids(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "p.txt"
    f.write_text("one")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "two")
    rid = _recovery_rows(daemon)[-1]["id"]
    assert _restore(daemon, rid[:-1]).read_text() == "one"
    with pytest.raises(recovery.NotRestorable):
        _restore(daemon, "nope")


def test_restore_and_purge_never_reach_outside_the_recovery_root(daemon: RunningDaemon) -> None:
    """A record whose copy path points outside ~/.ferb/recovery is never read from or deleted."""
    outside = daemon.daemon.home / "outside.txt"
    outside.write_text("not FERB's copy")
    f = _ws(daemon) / "t.txt"
    f.write_text("x")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "y")
    rid = _recovery_rows(daemon)[-1]["id"]
    for bad in (str(outside), "../outside.txt"):
        _sql(daemon, "UPDATE recovery SET saved_to = ? WHERE id = ?", (bad, rid))
        with pytest.raises(recovery.NotRestorable):
            _restore(daemon, rid)
    assert f.read_text() == "y"
    _sql(daemon, "UPDATE recovery SET at = ?", ((daemon.daemon.clock.now() - timedelta(days=30)).isoformat(),))

    async def purge() -> int:
        return recovery.purge_before(daemon.daemon.clock.now().isoformat(), daemon.daemon.db, _root(daemon))
    assert daemon.run(purge()) == 1 and outside.read_text() == "not FERB's copy"


# ---------- the Trash ----------

def test_a_trash_keeps_the_original_path_durably(daemon: RunningDaemon, trash: Path) -> None:
    f = _ws(daemon) / "old.txt"
    f.write_text("bye")
    grant = _fs_grant(daemon)
    out = gateway_tool_call(daemon, grant, "fs.trash", path=str(f), effect="delete")
    assert not out["is_error"] and not f.exists()
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "trash" and rec["original"] == str(f.resolve())
    assert rec["saved_to"] == str(trash.resolve() / "old.txt") and rec["saved_dev"] and rec["saved_ino"]
    assert rec["reason"] == "trash" and out["content"]["recoverable"] == rec["id"]
    restored = _restore(daemon, rec["id"])
    assert restored.resolve() == f.resolve() and f.read_text() == "bye" and not (trash / "old.txt").exists()
    # back where it was: its trash record goes; the backup copy S1 keeps beside it stays its retention days (L39)
    assert [r["kind"] for r in _recovery_rows(daemon)] == ["clone"]


def test_a_trash_keeps_a_backup_copy_too(daemon: RunningDaemon, trash: Path) -> None:
    """L39, S1 ("Trash plus a backup copy"): live case 39e35d57 left 5 trashed files with a trash record and no copy.
    A copy (an APFS clone) is kept before the move, so emptying the Trash doesn't lose the file."""
    f = _ws(daemon) / "Student_Name_ID_HCI_Assignment1.docx"
    f.write_bytes(b"the original assignment")
    out = gateway_tool_call(daemon, _fs_grant(daemon), "fs.trash", path=str(f))
    assert not out["is_error"] and not f.exists()
    rows = {r["kind"]: r for r in _recovery_rows(daemon)}
    assert set(rows) == {"clone", "trash"}
    copy = rows["clone"]
    assert copy["original"] == str(f.resolve()) and copy["reason"] == "trash copy"
    assert out["content"]["recoverable"] == rows["trash"]["id"] and out["content"]["backup"] == copy["id"]
    (trash / f.name).unlink()                                              # the owner empties the Trash
    assert _restore(daemon, copy["id"]).read_bytes() == b"the original assignment"


def test_a_trash_that_moved_nothing_keeps_no_copy(daemon: RunningDaemon, trash: Path,
                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    f = _ws(daemon) / "kept.txt"
    f.write_text("stays")
    monkeypatch.setattr(files, "trash_item", lambda *a, **k: files.Trashed(None, error="it is in use"))
    out = gateway_tool_call(daemon, _fs_grant(daemon), "fs.trash", path=str(f))
    assert out["is_error"] and f.read_text() == "stays"
    assert _recovery_rows(daemon) == [] and _copies(daemon) == []


def test_a_trash_restore_never_replaces_what_is_there_now(daemon: RunningDaemon, trash: Path) -> None:
    f = _ws(daemon) / "report.txt"
    f.write_text("old report")
    grant = _fs_grant(daemon)
    gateway_tool_call(daemon, grant, "fs.trash", path=str(f))
    f.write_text("a new report the owner wrote")
    rid = _recovery_rows(daemon)[-1]["id"]
    restored = _restore(daemon, rid)
    assert f.read_text() == "a new report the owner wrote"
    assert restored.name == "report (restored).txt" and restored.read_text() == "old report"


def test_a_trash_restore_says_when_the_item_left_the_trash(daemon: RunningDaemon, trash: Path) -> None:
    f = _ws(daemon) / "emptied.txt"
    f.write_text("z")
    grant = _fs_grant(daemon)
    gateway_tool_call(daemon, grant, "fs.trash", path=str(f))
    (trash / "emptied.txt").unlink()                                       # the owner emptied the Trash
    with pytest.raises(recovery.NotRestorable, match="no longer in the Trash"):
        _restore(daemon, _recovery_rows(daemon)[-1]["id"])


# ---------- retention ----------

def test_recovery_purges_after_seven_days(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "y.txt"
    f.write_text("a")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "b")
    assert len(_copies(daemon)) == 1
    _sql(daemon, "UPDATE recovery SET at = ?", ((daemon.daemon.clock.now() - timedelta(days=8)).isoformat(),))

    async def purge() -> int:
        return recovery.purge_before((daemon.daemon.clock.now() - timedelta(days=7)).isoformat(),
                                     daemon.daemon.db, _root(daemon))
    assert daemon.run(purge()) == 1 and _recovery_rows(daemon) == [] and _copies(daemon) == []


def test_the_retention_pass_purges_recovery_past_recovery_days_and_journals_it(daemon: RunningDaemon) -> None:
    assert daemon.daemon.config.get("retention", "recovery_days") == 7         # the owner sees it in config.toml
    f = _ws(daemon) / "r.txt"
    f.write_text("a")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "b")
    _write(daemon, grant, f, "c")                                          # a second copy, recent: kept
    first = _recovery_rows(daemon)[0]["id"]
    _sql(daemon, "UPDATE recovery SET at = ? WHERE id = ?",
         ((daemon.daemon.clock.now() - timedelta(days=8)).isoformat(), first))
    result: dict[str, int] = {}
    daemon.call_soon(lambda: result.update(daemon.daemon._purge_records()))
    assert result["recovery"] == 1 and [r["id"] for r in _recovery_rows(daemon)] != [first]
    assert len(_recovery_rows(daemon)) == 1 and len(_copies(daemon)) == 1

    async def row() -> dict[str, Any]:
        return json.loads(daemon.daemon.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind = 'retention.purged'").fetchone()["data_json"])
    got = daemon.run(row())
    assert got["recovery"] == 1 and got["recovery_days"] == 7


# ---------- ferb restore ----------

# decision: DMN-33
def test_ferb_restore_lists_and_restores_in_the_owners_words(daemon: RunningDaemon, trash: Path) -> None:
    f = _ws(daemon) / "notes.txt"
    f.write_text("the original")
    g = _ws(daemon) / "old.txt"
    g.write_text("bye")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "FERB's version")
    gateway_tool_call(daemon, grant, "fs.trash", path=str(g))
    rows = _recovery_rows(daemon)
    clone_id = next(r["id"] for r in rows if r["reason"] == "overwrite")
    trash_id = next(r["id"] for r in rows if r["kind"] == "trash")
    c = Client(daemon.sock)
    try:
        out = io.StringIO()
        assert admin.restore(c, None, out) == 0
        listing = out.getvalue()
        assert "overwrote" in listing and "moved to Trash" in listing and clone_id in listing
        assert "Trash copy" in listing                                    # the copy kept beside the trash (L39)
        assert "notes.txt" in listing and "ferb restore" in listing
        out = io.StringIO()
        assert admin.restore(c, clone_id, out) == 0
        assert out.getvalue().startswith("Restored ") and "to what it was before FERB changed it." in out.getvalue()
        assert f.read_text() == "the original"
        out = io.StringIO()
        assert admin.restore(c, trash_id, out) == 0
        assert "back from the Trash" in out.getvalue() and g.read_text() == "bye"
        with pytest.raises(RpcError):
            c.call("recovery.restore", {"id": "nope"})
    finally:
        c.close()

    async def journaled() -> int:
        return int(daemon.daemon.db.conn.execute(
            "SELECT count(*) FROM journal WHERE kind = 'recovery.restored'").fetchone()[0])
    assert daemon.run(journaled()) == 2


def test_ferb_restore_says_so_when_nothing_was_changed(daemon: RunningDaemon) -> None:
    c = Client(daemon.sock)
    try:
        out = io.StringIO()
        assert admin.restore(c, None, out) == 0
        assert "hasn't changed or deleted any files" in out.getvalue()
    finally:
        c.close()


def test_short_paths_use_the_owners_home() -> None:
    assert recovery.short(Path.home() / "Documents" / "a.txt") == "~/Documents/a.txt"
    assert recovery.short(Path("/tmp/x")) == "/tmp/x"


# ---------- snapshots before bulk or destructive work ----------

class _Snapshots:
    """A fake `tmutil localsnapshot`: each call a new name (or a failure), in call order."""

    def __init__(self, fail: str = "", gate: threading.Event | None = None) -> None:
        self.fail, self.gate, self.order, self.n = fail, gate, [], 0

    def __call__(self) -> str | None:
        if self.gate is not None:
            self.gate.wait(5)
        self.order.append("snapshot")
        if self.fail == "boom":
            raise RuntimeError("tmutil went away")
        if self.fail == "none":
            return None
        self.n += 1
        return f"com.apple.TimeMachine.2026-10-09-00000{self.n}.local"


class _Ordered(FakeCap):
    def __init__(self, name: str, kind: str, order: list[str]) -> None:
        super().__init__(name, kind, {"probe"})
        self.order = order

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        self.order.append(self.name)
        return await super().run(args, ctx)


def _snaps(daemon: RunningDaemon, fake: _Snapshots) -> recovery.Snapshots:
    service = daemon.daemon.snapshots
    assert service is not None and daemon.daemon.gateway.snapshots is service
    service.take = fake
    return service


def _snapshot_rows(daemon: RunningDaemon) -> list[dict[str, Any]]:
    return [r for r in _recovery_rows(daemon) if r["kind"] == "snapshot"]


def test_the_test_daemon_never_takes_a_real_snapshot(daemon: RunningDaemon) -> None:
    assert daemon.daemon.snapshots is not None and daemon.daemon.snapshots.take is None


def test_an_agent_asks_for_a_snapshot_and_it_is_recorded(daemon: RunningDaemon) -> None:
    fake = _Snapshots()
    _snaps(daemon, fake)
    grant, _ = gateway_setup(daemon, set(), gateway_request("clear out the old builds"), [])
    out = gateway_tool_call(daemon, grant, "recovery.snapshot", before="delete every build folder")
    assert not out["is_error"] and out["content"]["snapshot"] == "com.apple.TimeMachine.2026-10-09-000001.local"
    assert "macOS Recovery or Time Machine" in out["content"]["note"]
    [row] = _snapshot_rows(daemon)
    assert row["original"] == "/" and row["saved_to"] == out["content"]["snapshot"]
    assert row["reason"] == "delete every build folder" and row["task_id"] == grant.task_id
    assert out["content"]["record"] == row["id"]

    async def ledger() -> int:
        return int(daemon.daemon.db.conn.execute("SELECT count(*) FROM action_ledger").fetchone()[0])
    assert daemon.run(ledger()) == 0                                        # reversible: no ledger row


# decision: HAND-206
def test_the_last_snapshot_is_reused_only_while_no_command_ran_since(daemon: RunningDaemon) -> None:
    """HAND-206: a fresh snapshot before each shell command, skipped only when no command ran since the
    last one (replacing "at most one per 15 minutes"); an agent's own request follows the same rule."""
    fake = _Snapshots()
    _snaps(daemon, fake)
    shell = _Ordered("shell.fake", Kind.RUN_SHELL, fake.order)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("tidy"), [shell])
    first = gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk rename")["content"]
    again = gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk delete")["content"]
    assert again["reused"] is True and again["snapshot"] == first["snapshot"] and fake.n == 1   # nothing ran since
    assert "no command ran since" in again["note"]
    a = gateway_tool_call(daemon, grant, "shell.fake", text="make")          # nothing ran since the first: reused
    b = gateway_tool_call(daemon, grant, "shell.fake", text="make install")  # a command ran: a fresh one
    c = gateway_tool_call(daemon, grant, "shell.fake", text="make clean")
    assert a["content"]["recovery_snapshot"]["reused"] is True and fake.n == 3
    assert not b["content"]["recovery_snapshot"]["reused"] and not c["content"]["recovery_snapshot"]["reused"]
    assert fake.order == ["snapshot", "shell.fake", "snapshot", "shell.fake", "snapshot", "shell.fake"]
    daemon.call_soon(daemon.daemon.clock.advance, 16 * 60)                  # time alone never decides
    later = gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk delete")["content"]
    assert fake.n == 4 and "reused" not in later
    assert len(_snapshot_rows(daemon)) == 4


# decision: HAND-206
def test_a_shell_call_takes_a_snapshot_first_and_trash_and_writes_take_none(daemon: RunningDaemon, trash: Path) -> None:
    """F5a (carried item 6): code can't tell a shell delete from a build (rule 6), so a shell command gets a snapshot
    first; a trash and a write are recoverable by themselves (S1) and take none."""
    fake = _Snapshots()
    _snaps(daemon, fake)
    shell = _Ordered("shell.fake", Kind.RUN_SHELL, fake.order)
    grant, _ = gateway_setup(daemon, {"probe", "fs:write"}, gateway_request("tidy"), [shell])
    out = gateway_tool_call(daemon, grant, "shell.fake", text="rm -r build")
    f = _ws(daemon) / "t.txt"
    f.write_text("x")
    gateway_tool_call(daemon, grant, "fs.trash", path=str(f))
    _write(daemon, grant, _ws(daemon) / "w.txt", "x")
    assert fake.order == ["snapshot", "shell.fake"] and len(_snapshot_rows(daemon)) == 1
    assert out["content"]["recovery_snapshot"]["record"] == _snapshot_rows(daemon)[0]["id"]
    [span] = _spans(daemon, "recovery.snapshot")
    assert span["error"] is None


def test_a_permanent_delete_takes_a_snapshot_first(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:delete_permanent")   # FERB's own question(s) on the way (F5a floor)
    fake = _Snapshots()
    _snaps(daemon, fake)
    gone = _Ordered("fs.fake_delete", Kind.DELETE_PERMANENT, fake.order)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("delete it for good"), [gone])
    assert not gateway_tool_call(daemon, grant, "fs.fake_delete", text="a")["is_error"]
    assert fake.order == ["snapshot", "fs.fake_delete"]
    [span] = _spans(daemon, "recovery.snapshot")
    assert span["error"] is None and json.loads(span["output_json"])["snapshot"].startswith("com.apple.TimeMachine.")
    assert len(_snapshot_rows(daemon)) == 1


# decision: HAND-206, HAND-216
def test_a_failed_snapshot_never_refuses_a_call_says_so_and_is_tried_again(ferb_yes: FerbAnswers,
                                                                          daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:delete_permanent")   # FERB's own question(s) on the way (F5a floor)
    for fail in ("boom", "none"):
        fake = _Snapshots(fail=fail)
        _snaps(daemon, fake)
        gone = _Ordered(f"fs.fake_delete_{fail}", Kind.DELETE_PERMANENT, fake.order)
        grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("delete"), [gone])
        for text in ("a", "b"):          # both run anyway; right after a failure the next call doesn't wait on tmutil
            out = gateway_tool_call(daemon, grant, gone.name, text=text)
            assert not out["is_error"] and "no snapshot was taken" in out["content"]["snapshot_error"]
        assert fake.order == ["snapshot", gone.name, gone.name]
        daemon.daemon.clock.advance(61)                                  # the pause after a failure has passed
        out = gateway_tool_call(daemon, grant, gone.name, text="c")
        assert not out["is_error"] and "no snapshot was taken" in out["content"]["snapshot_error"]
        assert fake.order == ["snapshot", gone.name, gone.name, "snapshot", gone.name]
        asked = gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk")
        assert asked["is_error"] and "no snapshot was taken" in asked["content"]["error"]
        daemon.daemon.clock.advance(61)
    spans = _spans(daemon, "recovery.snapshot")
    assert spans and all(s["error"] for s in spans) and _snapshot_rows(daemon) == []


def test_a_cancelled_wait_still_records_the_snapshot_for_the_next_request(daemon: RunningDaemon) -> None:
    gate = threading.Event()
    fake = _Snapshots(gate=gate)
    service = _snaps(daemon, fake)

    async def go() -> tuple[bool, dict[str, Any]]:
        waiting = asyncio.ensure_future(service.request("task-1", "bulk delete"))
        await asyncio.sleep(0.05)
        waiting.cancel()
        try:
            await waiting
        except asyncio.CancelledError:
            pass
        gate.set()
        for _ in range(100):                                             # the snapshot goes on without its asker
            if service._running is not None and service._running.done():
                break
            await asyncio.sleep(0.02)
        return waiting.cancelled(), await service.request("task-2", "bulk delete")
    cancelled, nxt = daemon.run(go())
    assert cancelled and fake.n == 1 and nxt["reused"] is True and len(_snapshot_rows(daemon)) == 1


def test_ferb_restore_lists_a_snapshot_with_how_to_open_it(daemon: RunningDaemon) -> None:
    _snaps(daemon, _Snapshots())
    grant, _ = gateway_setup(daemon, set(), gateway_request("tidy"), [])
    gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk delete")
    rid = _snapshot_rows(daemon)[0]["id"]
    c = Client(daemon.sock)
    try:
        out = io.StringIO()
        admin.restore(c, None, out)
        assert "disk snapshot" in out.getvalue() and "macOS Recovery or Time Machine" in out.getvalue()
        with pytest.raises(RpcError, match="admin rights"):
            c.call("recovery.restore", {"id": rid})
    finally:
        c.close()


def test_old_snapshots_are_deleted_then_their_records_go(daemon: RunningDaemon) -> None:
    _snaps(daemon, _Snapshots())
    grant, _ = gateway_setup(daemon, set(), gateway_request("tidy"), [])
    gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk delete")
    _sql(daemon, "INSERT INTO recovery (id, task_id, kind, original, saved_to, reason, at) "
                 "VALUES ('odd', 't', 'snapshot', '/', 'tmutil said: done', 'x', ?)",
         (daemon.daemon.clock.now().isoformat(),))
    _sql(daemon, "UPDATE recovery SET at = ? WHERE kind = 'snapshot'",
         ((daemon.daemon.clock.now() - timedelta(days=8)).isoformat(),))
    cutoff = (daemon.daemon.clock.now() - timedelta(days=7)).isoformat()
    deleted: list[str] = []
    gone: list[tuple[str, bool]] = []

    async def purge(ok: bool) -> int:
        async def delete(name: str) -> bool:
            deleted.append(name)
            return ok
        return await recovery.purge_snapshots(cutoff, daemon.daemon.db, delete, lambda n, d: gone.append((n, d)))
    assert daemon.run(purge(False)) == 1                                  # the odd name: record only, never deleted
    assert gone == [("tmutil said: done", False)] and len(_snapshot_rows(daemon)) == 1
    assert daemon.run(purge(True)) == 1 and _snapshot_rows(daemon) == []
    assert deleted == ["com.apple.TimeMachine.2026-10-09-000001.local"] * 2 and gone[-1][1] is True


def test_local_snapshot_asks_tmutil_without_a_prompt(monkeypatch: pytest.MonkeyPatch) -> None:
    seen: dict[str, Any] = {}

    def fake_run(cmd: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        seen["cmd"], seen["kw"] = cmd, kw
        return subprocess.CompletedProcess(cmd, 0, "NOTE: local snapshots are considered purgeable and may be "
                                                   "removed at any time by deleted(8).\n"
                                                   "Created local snapshot with date: 2026-10-09-001938\n", "")
    monkeypatch.setattr(recovery.subprocess, "run", fake_run)
    assert recovery.local_snapshot() == "com.apple.TimeMachine.2026-10-09-001938.local"
    assert seen["cmd"] == ["/usr/bin/tmutil", "localsnapshot"]
    assert seen["kw"]["stdin"] is subprocess.DEVNULL and seen["kw"]["timeout"]   # never waits on a prompt


@pytest.mark.parametrize("outcome", ["refused", "missing", "timeout"])
def test_local_snapshot_is_none_when_tmutil_cannot(monkeypatch: pytest.MonkeyPatch, outcome: str) -> None:
    def fake_run(cmd: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        if outcome == "missing":
            raise FileNotFoundError(cmd[0])
        if outcome == "timeout":
            raise subprocess.TimeoutExpired(cmd, kw["timeout"])
        return subprocess.CompletedProcess(cmd, 77, "", "Operation not permitted\n")
    monkeypatch.setattr(recovery.subprocess, "run", fake_run)
    assert recovery.local_snapshot() is None


def test_a_snapshot_tmutil_reports_in_other_words_keeps_its_raw_name(monkeypatch: pytest.MonkeyPatch) -> None:
    for out, name in (("Created local snapshot with date: 2026-10-09T00:19:38\n", "2026-10-09T00:19:38"),
                      ("NOTE: purgeable\nsnapshot made\n", "snapshot made")):
        monkeypatch.setattr(recovery.subprocess, "run",
                            lambda cmd, **kw: subprocess.CompletedProcess(cmd, 0, out, ""))
        assert recovery.local_snapshot() == name


def test_delete_local_snapshot_deletes_only_ferbs_named_snapshot_without_a_prompt(
        monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[tuple[str, ...]] = []
    listed = {"out": "Snapshots for disk /:\ncom.apple.TimeMachine.2026-10-09-001938.local\n"}

    async def fake(*args: str, timeout_s: float) -> tuple[int, str]:
        calls.append(args)
        assert timeout_s > 0
        return 0, listed["out"] if args[0] == "listlocalsnapshots" else ""
    monkeypatch.setattr(recovery, "_tmutil", fake)
    name = "com.apple.TimeMachine.2026-10-09-001938.local"
    assert asyncio.run(recovery.delete_local_snapshot(name))
    assert calls[-1] == ("deletelocalsnapshots", "2026-10-09-001938")
    calls.clear()
    listed["out"] = "Snapshots for disk /:\n"                             # macOS thinned it already
    assert asyncio.run(recovery.delete_local_snapshot(name)) and calls == [("listlocalsnapshots", "/")]
    calls.clear()
    assert not asyncio.run(recovery.delete_local_snapshot("2026-10-09-001938; rm -rf ~")) and calls == []


def test_tmutil_runs_without_a_terminal_and_never_holds_up_its_caller(tmp_path: Path) -> None:
    """The real helper with a stand-in program: stdin closed, own session; a run past its timeout is left alone."""
    async def go() -> tuple[Any, Any]:
        import ferb.act.fs.recovery as rec_mod
        real = rec_mod.asyncio.create_subprocess_exec
        seen: dict[str, Any] = {}

        async def spy(program: str, *args: str, **kw: Any) -> Any:
            seen.update(kw)
            seen["proc"] = await real("/bin/sleep", "5", **kw)
            return seen["proc"]
        rec_mod.asyncio.create_subprocess_exec = spy                     # type: ignore[assignment]
        try:
            started = asyncio.get_running_loop().time()
            out = await rec_mod._tmutil("deletelocalsnapshots", "x", timeout_s=0.2)
            took = asyncio.get_running_loop().time() - started
            assert seen["proc"].returncode is None                       # left running, not waited for
            seen["proc"].kill()                                          # the test closes what it started
            await seen["proc"].wait()
            return (out, took), seen
        finally:
            rec_mod.asyncio.create_subprocess_exec = real                # type: ignore[assignment]
    (out, took), seen = asyncio.run(go())
    assert out is None and took < 2
    assert seen["stdin"] is subprocess.DEVNULL and seen["start_new_session"] is True


def test_an_overwrite_through_a_symlinked_folder_and_dotdot_copies_the_real_file(daemon: RunningDaemon) -> None:
    """`link/../notes.txt` is, for the OS, the notes.txt beside the link's target; textually it names a decoy."""
    ws = _ws(daemon)
    (ws / "elsewhere" / "inner").mkdir(parents=True)
    (ws / "link").symlink_to(ws / "elsewhere" / "inner")
    real, decoy = ws / "elsewhere" / "notes.txt", ws / "notes.txt"
    real.write_text("the real original")
    decoy.write_text("a decoy FERB never touches")
    grant = _fs_grant(daemon)
    out = _write(daemon, grant, ws / "link" / ".." / "notes.txt", "FERB's version")
    assert not out["is_error"] and real.read_text() == "FERB's version"
    assert decoy.read_text() == "a decoy FERB never touches"
    [rec] = _recovery_rows(daemon)
    assert rec["original"] == str(real.resolve()) and out["content"]["recoverable"] == rec["id"]
    assert _restore(daemon, rec["id"]).resolve() == real.resolve() and real.read_text() == "the real original"
    assert decoy.read_text() == "a decoy FERB never touches"


def test_copies_and_hashes_run_off_the_daemon_loop(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.act.fs.recovery as rec_mod
    seen: list[str] = []
    real_copy, real_hash = rec_mod._make_copy, rec_mod._sha256

    def copy(src: Path, dst: Path) -> None:
        seen.append(threading.current_thread().name)
        real_copy(src, dst)

    def digest(path: Path) -> str:
        seen.append(threading.current_thread().name)
        return real_hash(path)
    monkeypatch.setattr(rec_mod, "_make_copy", copy)
    monkeypatch.setattr(rec_mod, "_sha256", digest)
    f = _ws(daemon) / "big.txt"
    f.write_text("a")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "b")
    assert seen and daemon.thread.name not in seen


class _WritesThenErrs:
    """A write-file tool that changes the file and still reports an error."""
    name, description, scopes, identities = "fs.half", "half", {"fs:write"}, files.FsWrite.identities
    risk, reversible = files.FsWrite.risk, False
    Input = files.FsRead.Input

    def file_paths(self, args: Any) -> list[tuple[str, str]]:              # rule 10: a files tool declares them
        return [(args.path, "write")]

    def describe(self, args: Any) -> Any:
        return files.FsWrite().describe(files.FsWrite.Input(path=args.path, content=""))

    def real_path(self, args: Any, workspace: str) -> Path:
        return files.real_path(args.path, workspace)

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        Path(args.path).write_text("changed anyway")
        return ToolResult({"error": "partly failed"}, is_error=True)


def test_an_error_that_changed_the_file_keeps_the_copy(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "half.txt"
    f.write_text("before")
    half: Any = _WritesThenErrs()
    grant, _ = gateway_setup(daemon, {"fs:write"}, gateway_request("save"), [half])
    out = gateway_tool_call(daemon, grant, "fs.half", path=str(f))
    assert out["is_error"] and f.read_text() == "changed anyway"
    [rec] = _recovery_rows(daemon)
    assert out["content"]["recoverable"] == rec["id"] and rec["written_sha256"] is None
    restored = _restore(daemon, rec["id"])                                # unknown what FERB left: beside, not over
    assert restored.name == "half (before FERB).txt" and restored.read_text() == "before"


# ---------- the Trash: the very item, while it is there ----------

def test_a_trash_restore_never_takes_out_a_different_item_with_the_same_name(daemon: RunningDaemon,
                                                                             trash: Path) -> None:
    f = _ws(daemon) / "same.txt"
    f.write_text("FERB trashed this")
    grant = _fs_grant(daemon)
    gateway_tool_call(daemon, grant, "fs.trash", path=str(f))
    (trash / "same.txt").unlink()
    (trash / "same.txt").write_text("the owner's other file, trashed later")
    with pytest.raises(recovery.NotRestorable, match="isn't the"):
        _restore(daemon, _recovery_rows(daemon)[-1]["id"])
    assert (trash / "same.txt").read_text() == "the owner's other file, trashed later" and not f.exists()


def test_a_trash_that_moved_something_else_is_left_in_the_trash_for_the_owner(daemon: RunningDaemon, trash: Path,
                                                                             monkeypatch: pytest.MonkeyPatch) -> None:
    """What landed in the Trash isn't what FERB checked (swapped in the Trash right after the move): FERB never moves
    it out. The call fails, the record says where it is and that it needs the owner, and `ferb restore` won't put it
    at the original path by itself (S1, rule 10)."""
    f = _ws(daemon) / "swapped.txt"
    f.write_text("checked")

    def swap_in_the_trash() -> None:
        (trash / "swapped.txt").rename(_ws(daemon) / "stolen.txt")
        (trash / "swapped.txt").write_text("ATTACKER PAYLOAD")
    use_trash(monkeypatch, trash, after_move=swap_in_the_trash)
    out = gateway_tool_call(daemon, _fs_grant(daemon), "fs.trash", path=str(f))
    assert out["is_error"] and (trash / "swapped.txt").read_text() == "ATTACKER PAYLOAD" and not f.exists()
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "trash" and rec["original"] == str(f.resolve()) and out["content"]["recoverable"] == rec["id"]
    assert rec["saved_to"] == str(trash.resolve() / "swapped.txt") and rec["reason"] == recovery.TRASH_UNVERIFIED
    assert rec["saved_dev"] is None and rec["saved_ino"] is None
    with pytest.raises(recovery.NotRestorable, match="won't put it back"):
        _restore(daemon, rec["id"])
    assert not f.exists() and (trash / "swapped.txt").read_text() == "ATTACKER PAYLOAD"
    async def listing() -> dict[str, Any]:
        return recovery.recent(daemon.daemon.db, _root(daemon))[0][0]
    listed = daemon.run(listing())
    assert listed["needs_owner"] is True and listed["available"] is False
    lines = io.StringIO()
    admin.restore(_listing_client(listed), None, lines)
    assert "won't put it back" in lines.getvalue()


def _listing_client(row: dict[str, Any]) -> Any:
    class Listing:
        def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
            assert method == "recovery.list"
            return {"records": [row], "days": 7, "total": 1}
    return Listing()


def test_a_trash_record_keeps_what_ferb_verified_never_what_is_there_later(daemon: RunningDaemon, trash: Path,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    """The record stores the device and inode fs.trash verified through the open Trash, not a later look by path: an
    item swapped in after the move is never restored in the owner's file's place."""
    f = _ws(daemon) / "real.txt"
    f.write_text("owner's file")
    cap = daemon.daemon.gateway.caps["fs.trash"]
    run = cap.run

    async def then_swap(args: Any, ctx: Any) -> Any:
        result = await run(args, ctx)                                  # verified; the gateway hasn't recorded yet
        (trash / "real.txt").rename(_ws(daemon) / "stolen.txt")
        (trash / "real.txt").write_text("ATTACKER PAYLOAD")
        return result
    monkeypatch.setattr(cap, "run", then_swap)
    before = os.stat(f)
    out = gateway_tool_call(daemon, _fs_grant(daemon), "fs.trash", path=str(f))
    assert not out["is_error"]
    rec = _recovery_rows(daemon)[-1]
    assert (rec["saved_dev"], rec["saved_ino"]) == (before.st_dev, before.st_ino)
    with pytest.raises(recovery.NotRestorable, match="isn't the"):
        _restore(daemon, rec["id"])
    assert not f.exists() and (trash / "real.txt").read_text() == "ATTACKER PAYLOAD"


def test_a_restore_never_goes_through_a_folder_swapped_for_a_link(daemon: RunningDaemon, trash: Path) -> None:
    """Restore moves between the open Trash and the open destination folder, and the folder must be the one the item
    came from: a link standing where that folder was (into FERB's own state here) gets nothing."""
    proj = _ws(daemon) / "proj"
    proj.mkdir()
    f = proj / "config.toml"
    f.write_text("the owner's project config")
    gateway_tool_call(daemon, _fs_grant(daemon), "fs.trash", path=str(f))
    rid = _recovery_rows(daemon)[-1]["id"]
    proj.rename(_ws(daemon) / "proj-moved")
    proj.symlink_to(daemon.daemon.home)                                 # FERB's home: config.toml is FERB's own
    (daemon.daemon.home / "config.toml").unlink(missing_ok=True)
    with pytest.raises(recovery.NotRestorable, match="isn't where it was"):
        _restore(daemon, rid)
    assert not (daemon.daemon.home / "config.toml").exists() and (trash / "config.toml").exists()


def test_a_restore_makes_no_folder_through_a_link(daemon: RunningDaemon, trash: Path) -> None:
    """The item's folder is gone and a link stands where a folder above it was: restore refuses before making any
    folder, so nothing is created where the link points (measured before the fix: empty folders inside the target)."""
    proj = _ws(daemon) / "proj"
    (proj / "sub").mkdir(parents=True)
    f = proj / "sub" / "notes.txt"
    f.write_text("mine")
    gateway_tool_call(daemon, _fs_grant(daemon), "fs.trash", path=str(f))
    rid = _recovery_rows(daemon)[-1]["id"]
    agents = daemon.daemon.home.parent / "LaunchAgents"                 # a stand-in for a folder the shell can't write
    agents.mkdir()
    proj.rename(_ws(daemon) / "proj-moved")
    proj.symlink_to(agents)
    with pytest.raises(recovery.NotRestorable, match="isn't where it was"):
        _restore(daemon, rid)
    assert list(agents.iterdir()) == [] and (trash / "notes.txt").read_text() == "mine"


def test_a_restore_checks_what_came_out_of_the_trash(daemon: RunningDaemon, trash: Path,
                                                     monkeypatch: pytest.MonkeyPatch) -> None:
    f = _ws(daemon) / "after.txt"
    f.write_text("mine")
    gateway_tool_call(daemon, _fs_grant(daemon), "fs.trash", path=str(f))
    rid = _recovery_rows(daemon)[-1]["id"]
    real = files.move_excl

    def then_swap(src_dir: int, src: str, dst_dir: int, dst: str) -> None:
        real(src_dir, src, dst_dir, dst)
        f.unlink()                                                       # replaced right after it came out
        f.write_text("swapped")
    monkeypatch.setattr(files, "move_excl", then_swap)
    with pytest.raises(recovery.NotRestorable, match="isn't it"):
        _restore(daemon, rid)


def test_a_trash_record_outside_a_real_trash_folder_is_refused(daemon: RunningDaemon, trash: Path) -> None:
    f = _ws(daemon) / "d.txt"
    f.write_text("z")
    grant = _fs_grant(daemon)
    gateway_tool_call(daemon, grant, "fs.trash", path=str(f))
    rid = _recovery_rows(daemon)[-1]["id"]
    victim = _ws(daemon) / "victim.txt"
    victim.write_text("not in any Trash")
    for bad in (str(trash / ".." / "workspace" / "victim.txt"), str(victim), "relative/.Trash/x"):
        _sql(daemon, "UPDATE recovery SET saved_to = ? WHERE id = ?", (bad, rid))
        with pytest.raises(recovery.NotRestorable):
            _restore(daemon, rid)
    assert victim.read_text() == "not in any Trash"


def test_a_trash_record_stays_while_its_item_is_in_the_trash_up_to_ninety_days(daemon: RunningDaemon,
                                                                               trash: Path) -> None:
    for name in ("kept.txt", "emptied.txt", "ancient.txt"):
        f = _ws(daemon) / name
        f.write_text(name)
        gateway_tool_call(daemon, _fs_grant(daemon), "fs.trash", path=str(f))
    now = daemon.daemon.clock.now()
    _sql(daemon, "UPDATE recovery SET at = ?", ((now - timedelta(days=30)).isoformat(),))
    _sql(daemon, "UPDATE recovery SET at = ? WHERE original LIKE '%ancient.txt'",
         ((now - timedelta(days=91)).isoformat(),))
    (trash / "emptied.txt").unlink()

    async def purge() -> int:
        return recovery.purge_before((now - timedelta(days=7)).isoformat(), daemon.daemon.db, _root(daemon),
                                     trash_backstop_iso=(now - timedelta(days=90)).isoformat())
    # each trash's copy goes after its retention days (L39); the trash records as before: 3 copies and 2 records
    assert daemon.run(purge()) == 5
    assert [Path(r["original"]).name for r in _recovery_rows(daemon)] == ["kept.txt"]
    assert (trash / "ancient.txt").exists()                               # the item itself stays in the Trash


# ---------- restore and purge housekeeping ----------

def test_restore_waits_for_a_ferb_write_on_the_same_file(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "busy.txt"
    f.write_text("before")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "after")
    rid = _recovery_rows(daemon)[-1]["id"]

    async def go() -> tuple[bool, bool]:
        async with daemon.daemon.gateway.locks.hold(["file:" + str(f.resolve())], "a FERB write"):
            job = asyncio.ensure_future(daemon.daemon._recovery_restore(None, {"id": rid}))   # type: ignore[arg-type]
            await asyncio.sleep(0.1)
            waited = not job.done()
        await job
        return waited, job.done()
    assert daemon.run(go()) == (True, True) and f.read_text() == "before"


def test_a_copy_of_a_locked_file_is_not_locked(tmp_path: Path) -> None:
    src, dst = tmp_path / "locked.txt", tmp_path / "copy.txt"
    src.write_text("x")
    os.chflags(src, stat.UF_IMMUTABLE)
    try:
        recovery._make_copy(src, dst)
        assert not os.lstat(dst).st_flags & stat.UF_IMMUTABLE and dst.read_text() == "x"
        dst.unlink()                                                     # the purge can always delete it
    finally:
        os.chflags(src, 0)


def test_the_purge_sweeps_copies_that_never_got_a_record(daemon: RunningDaemon) -> None:
    root = _root(daemon)
    now = daemon.daemon.clock.now()
    old_day = (now - timedelta(days=9)).astimezone().date().isoformat()
    today = now.astimezone().date().isoformat()
    for day in (old_day, today):
        (root / day).mkdir(parents=True, exist_ok=True)
        (root / day / "orphan.txt").write_text("no record")

    async def purge() -> int:
        return recovery.purge_before((now - timedelta(days=7)).isoformat(), daemon.daemon.db, root)
    daemon.run(purge())
    assert not (root / old_day).exists() and (root / today / "orphan.txt").exists()   # a fresh one may be in flight


def test_ferb_restore_says_how_many_it_does_not_show(daemon: RunningDaemon) -> None:
    at = daemon.daemon.clock.now().isoformat()
    for i in range(105):
        _sql(daemon, "INSERT INTO recovery (id, task_id, kind, original, saved_to, reason, at) "
                     "VALUES (?, 't', 'clone', ?, ?, 'edit', ?)", (f"r{i:03d}", f"/x/{i}.txt", f"d/{i}.txt", at))
    c = Client(daemon.sock)
    try:
        out = io.StringIO()
        admin.restore(c, None, out)
    finally:
        c.close()
    text = out.getvalue()
    assert "Showing the newest 100 of 105." in text and text.count("(its copy is gone)") == 100


# ---------- fix round 2 ----------

class _PlainWrite:
    """A write-file tool with no real_path hook: the gateway finds its file from `target_args` (or can't)."""
    description, scopes, identities = "plain write", {"fs:write"}, files.FsWrite.identities
    risk, reversible = files.FsWrite.risk, False

    class Input(BaseModel):
        where: str
        text: str = "new"

    def __init__(self, name: str, target_args: tuple[str, ...] | None) -> None:
        self.name, self.runs = name, 0
        if target_args is not None:
            self.target_args = target_args

    def file_paths(self, args: Any) -> list[tuple[str, str]]:              # rule 10: a files tool declares them
        return [(args.where, "write")]

    def describe(self, args: Any) -> Any:
        return ActionSpec(Kind.WRITE_FILE, Target(path=args.where), content=args.text)

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        self.runs += 1
        Path(args.where).write_text(args.text)                          # the OS: symlinks first, then `..`
        return ToolResult({"path": args.where})


def test_a_write_tool_without_the_hook_is_copied_from_its_path_argument_symlinks_first(daemon: RunningDaemon) -> None:
    ws = _ws(daemon)
    (ws / "elsewhere" / "inner").mkdir(parents=True)
    (ws / "link").symlink_to(ws / "elsewhere" / "inner")
    real, decoy = ws / "elsewhere" / "notes.txt", ws / "notes.txt"
    real.write_text("the real original")
    decoy.write_text("decoy")
    tool: Any = _PlainWrite("fs.plain", ("where",))
    grant, _ = gateway_setup(daemon, {"fs:write"}, gateway_request("save"), [tool])
    out = gateway_tool_call(daemon, grant, "fs.plain", where=str(ws / "link" / ".." / "notes.txt"), text="new")
    assert not out["is_error"] and real.read_text() == "new" and decoy.read_text() == "decoy"
    [rec] = _recovery_rows(daemon)
    assert rec["original"] == str(real.resolve())
    assert _restore(daemon, rec["id"]).resolve() == real.resolve() and real.read_text() == "the real original"


def test_a_write_tool_whose_file_cannot_be_told_is_refused(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "unknown.txt"
    f.write_text("keep me")
    tool: Any = _PlainWrite("fs.blind", None)
    grant, _ = gateway_setup(daemon, {"fs:write"}, gateway_request("save"), [tool])
    out = gateway_tool_call(daemon, grant, "fs.blind", where=str(f), text="replaced")
    assert out["is_error"] and "can't tell which file this tool writes" in out["content"]["error"]
    assert tool.runs == 0 and f.read_text() == "keep me" and _recovery_rows(daemon) == []

    async def ledger() -> list[str]:
        return [r["state"] for r in daemon.daemon.db.conn.execute(
            "SELECT state FROM action_ledger WHERE capability = 'fs.blind'")]
    assert daemon.run(ledger()) == ["failed"]


def test_a_shutdown_during_a_slow_snapshot_delete_keeps_the_retention_record(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "r.txt"
    f.write_text("a")
    _write(daemon, _fs_grant(daemon), f, "b")
    _snaps(daemon, _Snapshots())
    grant, _ = gateway_setup(daemon, set(), gateway_request("tidy"), [])
    gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk delete")
    _sql(daemon, "UPDATE recovery SET at = ?", ((daemon.daemon.clock.now() - timedelta(days=8)).isoformat(),))
    started = threading.Event()

    async def slow_delete(name: str) -> bool:
        started.set()
        await asyncio.sleep(3600)                                        # a tmutil delete that takes forever
        return True

    async def go() -> tuple[bool, float]:
        daemon.daemon.snapshot_deleter = slow_delete
        job = asyncio.ensure_future(daemon.daemon._purge_pass())
        for _ in range(100):
            if started.is_set():
                break
            await asyncio.sleep(0.01)
        t0 = asyncio.get_running_loop().time()
        job.cancel()
        try:
            await job
        except asyncio.CancelledError:
            pass
        return job.cancelled(), asyncio.get_running_loop().time() - t0
    cancelled, took = daemon.run(go())
    assert cancelled and took < 1

    async def journal() -> list[dict[str, Any]]:
        return [json.loads(r["data_json"]) for r in daemon.daemon.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind = 'retention.purged'")]
    [row] = daemon.run(journal())
    assert row["recovery"] == 1                                          # journaled before the snapshot step
    assert len(_snapshot_rows(daemon)) == 1                              # not deleted: its record stays


def test_the_agents_before_words_are_stored_redacted(daemon: RunningDaemon) -> None:
    _snaps(daemon, _Snapshots())
    grant, _ = gateway_setup(daemon, set(), gateway_request("tidy"), [])
    secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"
    assert not gateway_tool_call(daemon, grant, "recovery.snapshot", before=f"bulk delete with {secret}")["is_error"]
    [row] = _snapshot_rows(daemon)
    assert secret not in row["reason"] and row["reason"].startswith("bulk delete with")


def test_a_snapshot_with_a_raw_name_is_recorded_listed_and_rate_limited(daemon: RunningDaemon) -> None:
    calls: list[str] = []

    def raw() -> str:
        calls.append("x")
        return "snapshot made"
    service = _snaps(daemon, _Snapshots())
    service.take = raw
    grant, _ = gateway_setup(daemon, set(), gateway_request("tidy"), [])
    first = gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk")["content"]
    again = gateway_tool_call(daemon, grant, "recovery.snapshot", before="bulk")["content"]
    assert first["snapshot"] == "snapshot made" and again["reused"] is True and calls == ["x"]
    c = Client(daemon.sock)
    try:
        out = io.StringIO()
        admin.restore(c, None, out)
        assert "snapshot made" in out.getvalue()
    finally:
        c.close()


# ---------- fix round 3 ----------

class _FakeRecordings:
    def __init__(self) -> None:
        self.stopped: list[str] = []
        self.records: dict[str, Any] = {}                 # RecordingStop.file_paths looks up the reserved video here

    async def stop(self, rid: str, owner: str, reason: str | None = None) -> dict[str, Any]:
        self.stopped.append(rid)
        return {"id": rid, "path": "/tmp/task.mp4", "capture_complete": True}


def test_recording_stop_runs_it_only_creates_its_own_new_file(daemon: RunningDaemon) -> None:
    from ferb.act.recording import RecordingStop
    manager = _FakeRecordings()
    stop: Any = RecordingStop(manager)                                   # type: ignore[arg-type]
    assert stop.describe(stop.Input(id="r1")).kind == Kind.WRITE_FILE and stop.creates_new_only is True
    grant, _ = gateway_setup(daemon, {"fs:write"}, gateway_request("record it"), [stop])
    out = gateway_tool_call(daemon, grant, "recording.stop", id="r1")
    assert not out["is_error"] and manager.stopped == ["r1"] and _recovery_rows(daemon) == []


def test_only_a_declared_new_file_writer_without_a_target_runs(daemon: RunningDaemon) -> None:
    class _NewOnly(_PlainWrite):
        creates_new_only = True                                          # declared: it never touches existing files

        def describe(self, args: Any) -> Any:
            return ActionSpec(Kind.WRITE_FILE)
    f = _ws(daemon) / "fresh.out"
    declared: Any = _NewOnly("fs.new_only", None)
    blind: Any = _PlainWrite("fs.no_target", None)
    blind.describe = lambda args: ActionSpec(Kind.WRITE_FILE)
    grant, _ = gateway_setup(daemon, {"fs:write"}, gateway_request("make it"), [declared, blind])
    assert not gateway_tool_call(daemon, grant, "fs.new_only", where=str(f), text="new")["is_error"]
    refused = gateway_tool_call(daemon, grant, "fs.no_target", where=str(f), text="over")
    assert declared.runs == 1 and blind.runs == 0 and f.read_text() == "new"
    assert refused["is_error"] and "can't tell which file this tool writes" in refused["content"]["error"]


def test_snapshot_words_are_redacted_before_they_are_cut(daemon: RunningDaemon) -> None:
    secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"
    service = _snaps(daemon, _Snapshots())
    service.take = lambda: f"snapshot made {secret}"

    async def go() -> dict[str, Any]:
        return await service.request("task-1", "y" * 185 + " " + secret)   # the cut at 200 falls inside it
    out = daemon.run(go())
    [row] = _snapshot_rows(daemon)
    assert "sk-ant" not in row["reason"] and "sk-ant" not in row["saved_to"] and "sk-ant" not in out["snapshot"]
