"""FERB's file changes are recoverable for 7 days (S1): a clone before an overwrite/edit, the original path kept for a
trash, restore (never over a change the owner made since), fallback when clonefile is unavailable, the retention purge,
`ferb restore`, and one local APFS snapshot per task tree before shell or delete work (best effort).

Offline: the Trash is a folder in the test home (files.move_to_trash is replaced), and no real tmutil snapshot is ever
taken (the test daemon has no snapshot hook; local_snapshot's own tests replace subprocess.run)."""

import io
import json
import os
import subprocess
from datetime import timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.act.fs import files, recovery
from ferb.cli import admin
from ferb.cli.client import Client, RpcError
from ferb.core.types import ToolContext, ToolResult
from ferb.policy.rules import Kind
from tests.conftest import FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call


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
    """A Trash folder inside the test home: offline tests never touch the owner's real Trash."""
    bin_ = daemon.daemon.home / ".Trash"
    bin_.mkdir(parents=True, exist_ok=True)

    def move(p: Path) -> str | None:
        where = bin_ / p.name
        os.rename(p, where)
        return str(where)
    monkeypatch.setattr(files, "move_to_trash", move)
    return bin_


# ---------- clone before a change ----------

def test_an_overwrite_keeps_a_recoverable_copy(daemon: RunningDaemon) -> None:
    f = _ws(daemon) / "notes.txt"
    f.write_text("the original")
    grant = _fs_grant(daemon)
    out = _write(daemon, grant, f, "the replacement")
    assert not out["is_error"] and f.read_text() == "the replacement"
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "clone" and rec["original"] == str(f) and rec["reason"] == "overwrite"
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
    """A write that raised may or may not have changed the file: the copy stays."""
    f = _ws(daemon) / "half.txt"
    f.write_text("before")

    def boom(p: Path, text: str) -> None:
        raise OSError(5, "I/O error")
    monkeypatch.setattr(files, "atomic_text", boom)
    grant = _fs_grant(daemon)
    out = _write(daemon, grant, f, "after")
    assert out["is_error"] and "outcome unknown" in out["content"]["error"]
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "clone" and rec["written_sha256"] is None
    assert len(_copies(daemon)) == 1


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
    assert rec["kind"] == "trash" and rec["original"] == str(f) and rec["saved_to"] == str(trash / "old.txt")
    assert rec["reason"] == "trash" and out["content"]["recoverable"] == rec["id"]
    restored = _restore(daemon, rec["id"])
    assert restored.resolve() == f.resolve() and f.read_text() == "bye" and not (trash / "old.txt").exists()
    assert _recovery_rows(daemon) == []                                   # back where it was: nothing left to recover


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

def test_ferb_restore_lists_and_restores_in_the_owners_words(daemon: RunningDaemon, trash: Path) -> None:
    f = _ws(daemon) / "notes.txt"
    f.write_text("the original")
    g = _ws(daemon) / "old.txt"
    g.write_text("bye")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "FERB's version")
    gateway_tool_call(daemon, grant, "fs.trash", path=str(g))
    clone_id, trash_id = (r["id"] for r in _recovery_rows(daemon))
    c = Client(daemon.sock)
    try:
        out = io.StringIO()
        assert admin.restore(c, None, out) == 0
        listing = out.getvalue()
        assert "overwrote" in listing and "moved to Trash" in listing and clone_id in listing
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


# ---------- a snapshot before shell or delete work ----------

class _Snapshots:
    def __init__(self, name: str | None = "com.apple.TimeMachine.2026-10-09-001938.local",
                 boom: bool = False) -> None:
        self.name, self.boom, self.order = name, boom, []

    def __call__(self) -> str | None:
        self.order.append("snapshot")
        if self.boom:
            raise RuntimeError("tmutil went away")
        return self.name


class _Shell(FakeCap):
    def __init__(self, order: list[str]) -> None:
        super().__init__("shell.fake", Kind.RUN_SHELL, {"shell"})
        self.order = order

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        self.order.append("shell")
        return await super().run(args, ctx)


def test_the_test_daemon_never_takes_a_real_snapshot(daemon: RunningDaemon) -> None:
    assert daemon.daemon.gateway.snapshot is None


def test_one_snapshot_per_task_tree_before_its_first_shell_command(daemon: RunningDaemon) -> None:
    snaps = _Snapshots()
    daemon.daemon.gateway.snapshot = snaps
    shell = _Shell(snaps.order)
    grant, _ = gateway_setup(daemon, {"shell"}, gateway_request("tidy my folder"), [shell])
    assert not gateway_tool_call(daemon, grant, "shell.fake", text="ls")["is_error"]
    assert not gateway_tool_call(daemon, grant, "shell.fake", text="rm -r build")["is_error"]
    assert snaps.order == ["snapshot", "shell", "shell"]                    # once, and before the first command
    [span] = _spans(daemon, "recovery.snapshot")
    assert json.loads(span["output_json"]) == {"snapshot": snaps.name} and span["error"] is None


def test_a_failed_snapshot_is_recorded_and_the_command_still_runs(daemon: RunningDaemon) -> None:
    for snaps in (_Snapshots(boom=True), _Snapshots(name=None)):
        daemon.daemon.gateway.snapshot = snaps
        daemon.daemon.gateway.forget_tasks({"task-1"})
        shell = _Shell(snaps.order)
        grant, _ = gateway_setup(daemon, {"shell"}, gateway_request("tidy"), [shell])
        assert not gateway_tool_call(daemon, grant, "shell.fake", text=f"echo {id(snaps)}")["is_error"]
        assert snaps.order == ["snapshot", "shell"]
    spans = _spans(daemon, "recovery.snapshot")
    assert len(spans) == 2 and all(s["error"] for s in spans)


def test_no_snapshot_for_a_plain_file_write(daemon: RunningDaemon) -> None:
    snaps = _Snapshots()
    daemon.daemon.gateway.snapshot = snaps
    grant = _fs_grant(daemon)
    _write(daemon, grant, _ws(daemon) / "w.txt", "x")                     # a per-file copy covers a write
    assert snaps.order == []


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


@pytest.mark.parametrize("outcome", ["refused", "missing", "timeout", "garbled"])
def test_local_snapshot_is_none_when_tmutil_cannot(monkeypatch: pytest.MonkeyPatch, outcome: str) -> None:
    def fake_run(cmd: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        if outcome == "missing":
            raise FileNotFoundError(cmd[0])
        if outcome == "timeout":
            raise subprocess.TimeoutExpired(cmd, kw["timeout"])
        if outcome == "refused":
            return subprocess.CompletedProcess(cmd, 77, "", "Operation not permitted\n")
        return subprocess.CompletedProcess(cmd, 0, "something else\n", "")
    monkeypatch.setattr(recovery.subprocess, "run", fake_run)
    assert recovery.local_snapshot() is None
