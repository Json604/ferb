# tests/test_protection.py
"""The protection lists (SQ3 revised, SQ5, D27): where FERB's shell may never read or write, what code always adds
(FERB's own state), and which rules the escape hatch may lift (D31). FERB's fs tools never touch FERB's own state."""
import asyncio
from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel

from ferb.act.browser.tools import OWNER_BROWSER, BrowserAct, BrowserRecord
from ferb.act.fs.files import FsEdit, FsList, FsRead, FsTrash, FsWrite
from ferb.act.mac.tools import MacRecord
from ferb.act.recording import RecordingInspect, RecordingStop
from ferb.clock.clock import FakeClock
from ferb.config import Config
from ferb.core.ids import SequentialIds
from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.daemon.main import Daemon
from ferb.policy import protection
from ferb.policy.protection import (DEFAULT_NEVER_READ, DEFAULT_NEVER_WRITE, DEFAULT_TERMINAL_APPS, EXTRA_NEVER_READ,
                                    BadProtection, Protection)
from ferb.policy.rules import Kind
from ferb.trace.compose import walk
from tests.conftest import RunningDaemon, gateway_setup, gateway_tool_call


def _tree(d: RunningDaemon, task_id: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:                  # the database belongs to the daemon's thread
        return d.daemon.spans.tree_json(task_id)
    return d.run(go())


def _ledger_rows(d: RunningDaemon) -> int:
    async def go() -> int:
        return int(d.daemon.db.conn.execute("SELECT COUNT(*) FROM action_ledger").fetchone()[0])
    return d.run(go())


def _prot(tmp_path: Path) -> Protection:
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace" / "uploads").mkdir(parents=True)
    (ferb / "run").mkdir()
    return Protection(home, ferb, [str(home / "Library" / "Keychains"), str(home / ".netrc")],
                      [str(home / ".zshrc"), str(home / "Library" / "LaunchAgents")], ["com.apple.Terminal"])


def test_ferbs_own_state_is_always_protected_and_never_liftable(tmp_path: Path) -> None:
    p = _prot(tmp_path)
    ferb = tmp_path / "home" / ".ferb"
    for path in (ferb / "ferb.db", ferb / "run" / "ferb.sock", ferb / "run" / "token-x", ferb / "recovery" / "a",
                 ferb / "config.toml", ferb / "workspace" / "uploads" / "t" / "x" / "cv.pdf"):
        assert p.read_denied(path) == "ferb_state" and p.write_denied(path) == "ferb_state", path
    assert p.read_denied(ferb / "workspace" / "notes.txt") is None
    assert p.write_denied(ferb / "workspace" / "downloads" / "a.zip") is None
    assert not p.liftable("ferb_state") and not p.liftable("network") and p.liftable("admin")


def test_configured_lists_give_rule_ids_and_reads_imply_writes(tmp_path: Path) -> None:
    p = _prot(tmp_path)
    home = tmp_path / "home"
    assert p.read_denied(home / "Library" / "Keychains" / "login.keychain-db") == "read:~/Library/Keychains"
    assert p.write_denied(home / "Library" / "Keychains" / "x") == "read:~/Library/Keychains"   # unreadable: unwritable
    assert p.read_denied(home / ".netrc") == "read:~/.netrc" and p.read_denied(home / ".netrc2") is None
    assert p.write_denied(home / ".zshrc") == "write:~/.zshrc" and p.read_denied(home / ".zshrc") is None
    assert p.write_denied(home / "Library" / "LaunchAgents" / "x.plist") == "write:~/Library/LaunchAgents"
    assert p.liftable("read:~/Library/Keychains") and p.liftable("write:~/.zshrc")
    ids = {r["id"] for r in p.rules()}
    assert {"ferb_state", "network", "admin", "read:~/.netrc", "write:~/.zshrc"} <= ids


def test_the_default_lists_are_the_plans(tmp_path: Path) -> None:
    for entry in ("~/Library/Keychains", "~/.ssh", "~/.aws", "~/.config/gh", "~/.netrc", "~/.npmrc", "~/.docker",
                  "~/Library/Messages", "~/Library/Mail", "~/Library/Safari",
                  "~/Library/Group Containers/group.com.apple.usernoted",
                  "~/Library/Application Support/Google/Chrome", "~/Library/Application Support/BraveSoftware"):
        assert entry in DEFAULT_NEVER_READ, entry
    for entry in ("~/.ssh", "~/Library/Keychains", "~/Library/LaunchAgents", "~/.zshrc", "~/.zprofile",
                  "~/.bash_profile", "/System", "/Library", "/Applications"):
        assert entry in DEFAULT_NEVER_WRITE, entry


def test_the_extra_never_read_entries_sit_in_one_place() -> None:
    """OPEN QUESTION 2, built as recommended: the same class of credential stores, kept in one tuple the owner's answer
    can remove."""
    extra = ("~/.gnupg", "~/.kube", "~/.config/gcloud", "~/.azure", "~/.git-credentials", "~/.pypirc",
             "~/.cargo/credentials.toml")
    assert EXTRA_NEVER_READ == extra
    assert DEFAULT_NEVER_READ[-len(extra):] == extra and len(set(DEFAULT_NEVER_READ)) == len(DEFAULT_NEVER_READ)


def test_a_store_reached_through_a_link_is_still_the_store(tmp_path: Path) -> None:
    """Entries and checked paths both resolve as the OS does (files.real_path): links first, then `..`. A rule id
    names the entry as the owner wrote it, with the home as ~; a relative entry is the home's."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "run").mkdir(parents=True)
    real_ssh = home / "dotfiles" / "ssh"
    real_ssh.mkdir(parents=True)
    (home / ".ssh").symlink_to(real_ssh)
    (home / "state").symlink_to(ferb / "run")
    p = Protection(home, ferb, ["~/.ssh", "Library/Cookies"], [], [])
    assert p.read_denied(real_ssh / "id_ed25519") == "read:~/.ssh"
    assert p.read_denied(home / ".ssh" / "id_ed25519") == "read:~/.ssh"
    assert p.read_denied(home / "dotfiles" / "x" / ".." / "ssh" / "config") == "read:~/.ssh"
    assert p.read_denied(home / "Library" / "Cookies" / "Cookies.binarycookies") == "read:~/Library/Cookies"
    assert p.read_denied(home / "state" / "token-x") == "ferb_state"
    assert p.read_denied(home / "dotfiles" / "zshrc") is None


def test_a_path_spelled_in_another_case_is_the_same_path(tmp_path: Path) -> None:
    """Mac volumes ignore case by default: ~/.FERB/run is ~/.ferb/run and ~/.SSH is ~/.ssh, so the lists compare paths
    the way the volume does (protection._key says what that means on a case-sensitive volume)."""
    p = _prot(tmp_path)
    home = tmp_path / "home"
    assert p.read_denied(home / ".FERB" / "Run" / "token-x") == "ferb_state"
    assert p.read_denied(home / ".Ferb" / "WORKSPACE" / "Uploads" / "cv.pdf") == "ferb_state"
    assert p.read_denied(home / ".ferb" / "Workspace" / "notes.txt") is None
    assert p.read_denied(home / "library" / "KEYCHAINS" / "login.keychain-db") == "read:~/Library/Keychains"
    assert p.write_denied(home / ".ZSHRC") == "write:~/.zshrc"
    assert p.fs_denied(home / ".FERB" / "ferb.db", Kind.READ) == "ferb_state"


def test_an_entry_that_becomes_a_link_after_start_still_matches_as_typed(tmp_path: Path) -> None:
    """A path is checked as typed and as resolved now, against each entry as written and as resolved at start."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (home / ".ssh").mkdir(parents=True)
    (home / "elsewhere").mkdir()
    p = Protection(home, ferb, ["~/.ssh"], [], [])
    (home / ".ssh").rmdir()
    (home / ".ssh").symlink_to(home / "elsewhere")                    # after start: ~/.ssh now points elsewhere
    assert p.read_denied(home / ".ssh" / "id_ed25519") == "read:~/.ssh"
    assert p.fs_denied(home / ".ssh" / "id_ed25519", "read") == "read:~/.ssh"


def test_a_link_planted_as_the_workspace_never_opens_ferbs_state(tmp_path: Path) -> None:
    """The workspace exception is the literal folder in FERB's home: with ~/.ferb/workspace -> ~/.ferb/run, the run
    tokens stay FERB's state by either spelling, and the daemon's loader refuses that workspace outright."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "run").mkdir(parents=True)
    (ferb / "run" / "token-x").write_text("t")
    (ferb / "workspace").symlink_to(ferb / "run")
    p = Protection(home, ferb, [], [], [])
    assert p.workspace == ferb.resolve() / "workspace" and p.uploads == p.workspace / "uploads"
    assert p.read_denied(ferb / "run" / "token-x") == "ferb_state"
    assert p.read_denied(ferb / "workspace" / "token-x") == "ferb_state"
    assert p.fs_denied(ferb / "workspace" / "token-x", "read") == "ferb_state"
    problem = protection.workspace_problem(ferb)
    assert problem is not None and "link" in problem and "won't start" in problem
    with pytest.raises(BadProtection, match="link"):
        protection.load(lambda section, key, default: default, home, ferb)


def test_the_daemon_refuses_to_start_with_a_linked_workspace(home: Path) -> None:
    """Before migrations and browsers: no database is opened, and the reason is in plain words."""
    (home / "run").mkdir(parents=True)
    (home / "workspace").symlink_to(home / "run")

    async def start() -> None:
        d = Daemon(home=home, watchers=False, distill=False, retention=False, notifications=False, snapshots=False,
                   watch_owner_input=False)
        await d.start()
    with pytest.raises(BadProtection, match="workspace"):
        asyncio.run(start())
    assert not (home / "ferb.db").exists()


def test_the_daemon_refuses_to_start_with_a_malformed_protection_list(home: Path) -> None:
    Config(home).set("protection", "never_read", "~/.ssh")

    async def start() -> None:
        d = Daemon(home=home, watchers=False, distill=False, retention=False, notifications=False, snapshots=False,
                   watch_owner_input=False)
        await d.start()
    with pytest.raises(BadProtection, match="never_read"):
        asyncio.run(start())
    assert not (home / "ferb.db").exists()


def test_entries_for_the_profile_builder(tmp_path: Path) -> None:
    """Resolved (path, is_dir) pairs: a file is a file; a folder, or an entry that doesn't exist yet, is a folder, so
    whatever is created there later stays covered."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ferb.mkdir(parents=True)
    (home / ".aws").mkdir()
    (home / ".netrc").write_text("")
    p = Protection(home, ferb, ["~/.aws", "~/.netrc", "~/.kube"], ["~/.zshrc"], [])
    assert p.read_entries() == [((home / ".aws").resolve(), True), ((home / ".netrc").resolve(), False),
                                (home.resolve() / ".kube", True)]
    assert p.write_entries() == [(home.resolve() / ".zshrc", True)]


def test_fs_tools_are_kept_out_of_the_never_read_stores_unless_the_owner_opens_them(tmp_path: Path) -> None:
    """OPEN QUESTION 6, built as recommended: protection.fs_reads_never_read = false keeps FERB's fs tools out of the
    never-read stores (reads and writes alike) until F5b's redaction is measured; true is SQ3 as written. Never-write
    entries don't bind the fs tools (SQ5: changes there go through them, with S1 copies). Nothing opens FERB's own state."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace" / "uploads").mkdir(parents=True)
    closed = Protection(home, ferb, ["~/.ssh"], ["~/.zshrc"], [])
    opened = Protection(home, ferb, ["~/.ssh"], ["~/.zshrc"], [], fs_reads_never_read=True)
    for kind in (Kind.READ, Kind.WRITE_FILE, Kind.TRASH):
        assert closed.fs_denied(home / ".ssh" / "id_ed25519", kind) == "read:~/.ssh", kind
        assert opened.fs_denied(home / ".ssh" / "id_ed25519", kind) is None, kind
        assert closed.fs_denied(home / ".zshrc", kind) is None and closed.fs_denied(home / "notes.txt", kind) is None
        assert opened.fs_denied(ferb / "ferb.db", kind) == "ferb_state", kind


def test_removing_a_folder_that_holds_a_never_read_store_is_refused(tmp_path: Path) -> None:
    """Trashing ~/.config would hand over ~/.config/gh in the Trash: while the switch is false a removal of a folder
    holding a never-read store is refused with that store's rule. Reading the folder itself (a listing) is fine."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    entries = ["~/.config/gh", "~/Library/Application Support/Google/Chrome"]
    closed = Protection(home, ferb, entries, [], [])
    opened = Protection(home, ferb, entries, [], [], fs_reads_never_read=True)
    for access in ("remove", Kind.TRASH, Kind.DELETE_PERMANENT):
        assert closed.fs_denied(home / ".config", access) == "read:~/.config/gh", access
        assert (closed.fs_denied(home / "Library" / "Application Support", access)
                == "read:~/Library/Application Support/Google/Chrome"), access
        assert opened.fs_denied(home / ".config", access) is None, access
    assert closed.fs_denied(home / ".config", "read") is None and closed.fs_denied(home / ".config", "write") is None
    assert closed.fs_denied(home / ".config" / "other", "remove") is None


def test_guarded_parents_are_every_folder_above_a_protected_path(tmp_path: Path) -> None:
    """For the shell profile: moving any folder above an entry or above FERB's upload staging would carry protected
    content to a path no rule names, so each such folder is listed (the root is not)."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ferb.mkdir(parents=True)
    p = Protection(home, ferb, ["~/.config/gh"], ["~/Library/LaunchAgents"], [])
    parents = p.guarded_parents()
    real_home = home.resolve()
    for folder in (real_home / ".config", real_home / "Library", real_home, real_home / ".ferb",
                   real_home / ".ferb" / "workspace", real_home.parent):
        assert folder in parents, folder
    assert real_home / ".config" / "gh" not in parents and p.uploads not in parents and Path("/") not in parents
    assert parents == sorted(set(parents))


def test_trashing_a_folder_that_holds_ferbs_state_is_refused(tmp_path: Path) -> None:
    """Moving a folder moves FERB's state inside it: the home, ~/.ferb and the workspace (it holds upload staging)."""
    p = _prot(tmp_path)
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    for folder in (tmp_path, home, ferb, ferb / "workspace"):
        assert p.fs_denied(folder, Kind.TRASH) == "ferb_state", folder
        assert p.fs_denied(folder, Kind.DELETE_PERMANENT) == "ferb_state", folder
    assert p.fs_denied(ferb / "workspace" / "downloads", Kind.TRASH) is None
    assert p.fs_denied(home, Kind.READ) is None and p.fs_denied(ferb / "workspace", Kind.READ) is None


def test_a_malformed_list_or_switch_is_refused_not_guessed(tmp_path: Path) -> None:
    """A protection list written as one string would otherwise be read letter by letter (a "~" entry closes the
    whole home); a switch written as text could read either way."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    with pytest.raises(ValueError, match="never_read"):
        Protection(home, ferb, "~/.ssh", [], [])                 # type: ignore[arg-type]
    with pytest.raises(ValueError, match="never_write"):
        Protection(home, ferb, [], ["~/.zshrc", 3], [])          # type: ignore[list-item]
    with pytest.raises(ValueError, match="terminal_apps"):
        Protection(home, ferb, [], [], "com.apple.Terminal")     # type: ignore[arg-type]
    with pytest.raises(ValueError, match="fs_reads_never_read"):
        Protection(home, ferb, [], [], [], fs_reads_never_read="false")   # type: ignore[arg-type]
    with pytest.raises(BadProtection, match="never_read"):                # a TOML inline table: never its keys
        Protection(home, ferb, {"~/.ssh": True}, [], [])        # type: ignore[arg-type]
    with pytest.raises(BadProtection, match="never_write"):
        Protection(home, ferb, [], [""], [])


def test_fs_tools_never_read_or_write_ferbs_own_state(daemon: RunningDaemon) -> None:
    grant, _ = gateway_setup(daemon, {"fs:read", "fs:write"}, None, [])
    home = daemon.daemon.home
    (home / "run").mkdir(exist_ok=True)
    (home / "run" / "token-abc").write_text("secret-token")
    r = gateway_tool_call(daemon, grant, "fs.read", path=str(home / "run" / "token-abc"))
    assert r["is_error"] and "FERB's own" in r["content"]["error"] and "secret-token" not in str(r)
    r = gateway_tool_call(daemon, grant, "fs.write", path=str(home / "config.toml"), content="x", overwrite=True)
    assert r["is_error"] and "FERB's own" in r["content"]["error"]
    assert not (home / "config.toml").exists() or (home / "config.toml").read_text() != "x"
    r = gateway_tool_call(daemon, grant, "fs.write", path="notes.txt", content="hello")   # the workspace: fine
    assert not r["is_error"]
    r = gateway_tool_call(daemon, grant, "fs.trash", path=str(home / "ferb.db"))
    assert r["is_error"] and "FERB's own" in r["content"]["error"]


def test_fs_tools_cant_reach_ferbs_state_by_another_spelling(daemon: RunningDaemon) -> None:
    """The Mac's volume ignores case: FERB/RUN/token is FERB's run token, and stays closed."""
    grant, _ = gateway_setup(daemon, {"fs:read"}, None, [])
    home = daemon.daemon.home
    (home / "run").mkdir(exist_ok=True)
    (home / "run" / "token-abc").write_text("secret-token")
    r = gateway_tool_call(daemon, grant, "fs.read", path=str(home.parent / home.name.upper() / "RUN" / "token-abc"))
    assert r["is_error"] and "FERB's own" in r["content"]["error"] and "secret-token" not in str(r)


def test_a_refused_fs_call_leaves_only_a_refused_span(daemon: RunningDaemon) -> None:
    """Refused before anything runs: no tool.call span, no ledger row, a tool.refused span saying why; an upload copy
    reached through a link in the workspace is still FERB's state."""
    grant, _ = gateway_setup(daemon, {"fs:read", "fs:write"}, None, [])
    home = daemon.daemon.home
    staged = home / "workspace" / "uploads" / "t1" / "cv.pdf"
    staged.parent.mkdir(parents=True)
    staged.write_text("staged copy")
    (home / "workspace" / "cv-link.pdf").symlink_to(staged)
    r = gateway_tool_call(daemon, grant, "fs.read", path="cv-link.pdf")
    assert r["is_error"] and "FERB's own" in r["content"]["error"] and "staged copy" not in str(r)
    r = gateway_tool_call(daemon, grant, "fs.edit", path=str(home / "config.toml"), old_text="a", new_text="b")
    assert r["is_error"] and "FERB's own" in r["content"]["error"]
    tree = _tree(daemon, grant.task_id)
    refused = [n for n in walk(tree) if n["kind"] == "tool.refused"]
    assert [(n["input"]["tool"], n["error"]) for n in refused] == [("fs.read", "FERB's own state"),
                                                                   ("fs.edit", "FERB's own state")]
    assert not [n for n in walk(tree) if n["kind"] == "tool.call"]
    assert _ledger_rows(daemon) == 0


class _Declares:
    """A tool outside fs.* that declares the files it names: the declaration, not the name, brings the check."""
    name, description, scopes = "probe.files", "reads a file", {"probe"}
    identities, risk, reversible = {Identity.OWNER, Identity.FERB}, RiskLevel.LOW, True

    class Input(BaseModel):
        where: str

    def __init__(self) -> None:
        self.runs = 0

    def file_paths(self, args: Any) -> list[tuple[str, str]]:
        return [(args.where, "read")]

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.READ, Target(site="example.com"))

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        self.runs += 1
        return ToolResult({"text": Path(args.where).read_text()})


def test_any_tool_that_declares_its_files_is_checked(daemon: RunningDaemon) -> None:
    tool = _Declares()
    grant, _ = gateway_setup(daemon, {"probe"}, None, [tool])           # type: ignore[list-item]
    home = daemon.daemon.home
    (home / "run").mkdir(exist_ok=True)
    (home / "run" / "token-abc").write_text("secret-token")
    r = gateway_tool_call(daemon, grant, "probe.files", where=str(home / "run" / "token-abc"))
    assert r["is_error"] and "FERB's own" in r["content"]["error"] and tool.runs == 0
    (home / "workspace" / "notes.txt").write_text("fine")
    r = gateway_tool_call(daemon, grant, "probe.files", where=str(home / "workspace" / "notes.txt"))   # fine
    assert not r["is_error"] and tool.runs == 1


def test_every_tool_that_names_files_declares_them(daemon: RunningDaemon) -> None:
    """The check follows declarations, so a file tool that forgot one would go unchecked: every registered tool with
    a files scope declares, and so do the upload and recording tools wherever they are registered."""
    caps = daemon.daemon.registry.capabilities
    undeclared = [n for n, c in caps.items() if set(c.scopes) & {"fs:read", "fs:write"} and not hasattr(c, "file_paths")]
    assert undeclared == []
    for cls in (BrowserAct, BrowserRecord, MacRecord, RecordingInspect, RecordingStop, FsList, FsRead, FsWrite, FsEdit,
                FsTrash):
        assert callable(getattr(cls, "file_paths", None)), cls


class _NoBrowser:
    """A browser driver the refused calls must never reach."""
    def __init__(self) -> None:
        self.used: list[str] = []

    def tab_url(self, tab_id: str) -> str | None:
        return None

    def element_info(self, tab_id: str, element_id: str) -> None:
        return None

    def owned_by(self, tab_id: str, owner: str, borrow: bool = True) -> Any:
        self.used.append("owned_by")
        raise AssertionError("a refused call reached the browser")


class _NoRecordings:
    def __init__(self) -> None:
        self.records: dict[str, Any] = {}
        self.started: list[Path] = []

    async def start(self, *args: Any) -> dict[str, Any]:
        self.started.append(args[2])
        raise AssertionError("a refused recording started")


def test_uploads_and_recordings_are_checked_like_fs_tools(daemon: RunningDaemon, tmp_path: Path) -> None:
    """An upload never reads FERB's own files (a run token, ferb.db) or, while the switch is false, a never-read store;
    a recording is never inspected from a store nor written into FERB's home outside its workspace. All refused before
    the browser, the staging folder or the recorder is touched."""
    home, fake = daemon.daemon.home, tmp_path / "fakehome"
    (home / "run").mkdir(exist_ok=True)
    (home / "run" / "token-x").write_text("secret-token")
    (fake / ".ssh").mkdir(parents=True)
    (fake / ".ssh" / "id").write_text("PRIVATE-KEY-BYTES")
    (fake / ".ssh" / "old.mp4").write_text("video")
    driver, recordings = _NoBrowser(), _NoRecordings()
    act, record = BrowserAct(driver, OWNER_BROWSER), BrowserRecord(driver, OWNER_BROWSER, recordings)
    grant, _ = gateway_setup(daemon, {"browser:act", "browser:read", "fs:read", "fs:write"}, None,
                             [act, record])                               # type: ignore[list-item]
    daemon.daemon.gateway.roots.protection = Protection(fake, home, ["~/.ssh"], [], [])
    for files in ([str(home / "run" / "token-x")], [str(home / "ferb.db")], [str(fake / ".ssh" / "id")]):
        r = gateway_tool_call(daemon, grant, "browser.act", tab="t1", element="e1", action="upload", files=files)
        assert r["is_error"] and ("FERB's own" in r["content"]["error"] or "~/.ssh" in r["content"]["error"]), files
        assert "secret-token" not in str(r) and "PRIVATE-KEY-BYTES" not in str(r)
    r = gateway_tool_call(daemon, grant, "recording.inspect", path=str(fake / ".ssh" / "old.mp4"))
    assert r["is_error"] and "~/.ssh" in r["content"]["error"]
    r = gateway_tool_call(daemon, grant, "browser.record", tab="t1", path=str(home / "run" / "task.mp4"))
    assert r["is_error"] and "FERB's own" in r["content"]["error"]
    assert driver.used == [] and recordings.started == [] and not (home / "workspace" / "uploads").exists()
    assert not (home / "run" / "task.mp4").exists()


def test_recording_stop_declares_the_video_it_publishes() -> None:
    from ferb.act.recording import RecordingManager
    manager = RecordingManager()
    stop = RecordingStop(manager)
    assert stop.file_paths(stop.Input(id="rec-x")) == []
    manager.records["rec-x"] = type("R", (), {"path": Path("/somewhere/task.mp4")})()   # type: ignore[assignment]
    assert stop.file_paths(stop.Input(id="rec-x")) == [("/somewhere/task.mp4", "write")]


def _ctx(ws: Path, prot: Protection) -> ToolContext:
    def closed(path: Path, access: str) -> str | None:
        rule = prot.fs_denied(path, access)
        return f"closed by {rule}" if rule else None
    return ToolContext("t", "w", "r", Identity.OWNER, frozenset({"fs:read", "fs:write"}), "dev", None, str(ws), (), (),
                       path_closed=closed)


def test_fs_tools_check_what_they_really_opened(tmp_path: Path) -> None:
    """The gateway checks a path before the call; between that check and the open a folder can become a link. Each fs
    tool checks again the file macOS really opened (F_GETPATH): links planted here into FERB's run folder are refused
    and nothing is read, written, edited, listed or trashed through them."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws, run = ferb / "workspace", ferb / "run"
    ws.mkdir(parents=True)
    run.mkdir()
    (run / "token-x").write_text("secret-token")
    prot = Protection(home, ferb, [], [], [])
    ctx = _ctx(ws, prot)
    (ws / "tok").symlink_to(run / "token-x")
    (ws / "dir").symlink_to(run)

    def go(tool: Any, **args: Any) -> ToolResult:
        return asyncio.run(tool.run(tool.Input(**args), ctx))
    for result in (go(FsRead(), path="tok"), go(FsList(), path="dir"), go(FsWrite(), path="dir/new.txt", content="x"),
                   go(FsEdit(), path="tok", old_text="secret", new_text="public"), go(FsTrash(), path="tok")):
        assert result.is_error and "closed by ferb_state" in result.content["error"], result.content
        assert "secret-token" not in str(result.content)
    assert (run / "token-x").read_text() == "secret-token" and not (run / "new.txt").exists()
    assert sorted(p.name for p in run.iterdir()) == ["token-x"]
    assert not go(FsWrite(), path="notes.txt", content="fine").is_error and (ws / "notes.txt").read_text() == "fine"
    assert go(FsRead(), path="notes.txt").content["text"] == "fine"


def test_fs_tools_and_a_never_read_store_follow_the_owners_switch(daemon: RunningDaemon, tmp_path: Path) -> None:
    """The refusal says why in plain words and names the sealed secret path; with the switch true the same read runs."""
    fake = tmp_path / "fakehome"
    (fake / ".aws").mkdir(parents=True)
    (fake / ".aws" / "config").write_text("region = eu-west-1\n")
    roots = daemon.daemon.gateway.roots
    grant, _ = gateway_setup(daemon, {"fs:read", "fs:write"}, None, [])
    roots.protection = Protection(fake, daemon.daemon.home, ["~/.aws"], [], [])
    r = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".aws" / "config"))
    error = r["content"]["error"]
    assert r["is_error"] and "eu-west-1" not in str(r)
    assert "~/.aws" in error and "passwords" in error and "sealed secret" in error and "escape hatch" in error
    r = gateway_tool_call(daemon, grant, "fs.write", path=str(fake / ".aws" / "new"), content="x")
    assert r["is_error"] and not (fake / ".aws" / "new").exists()
    roots.protection = Protection(fake, daemon.daemon.home, ["~/.aws"], [], [], fs_reads_never_read=True)
    r = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".aws" / "config"))
    assert not r["is_error"] and "eu-west-1" in r["content"]["text"]


def test_config_gets_the_protection_defaults_once(daemon: RunningDaemon) -> None:
    cfg = daemon.daemon.config
    assert cfg.get("protection", "never_read") == list(DEFAULT_NEVER_READ)
    assert cfg.get("protection", "never_write") == list(DEFAULT_NEVER_WRITE)
    assert cfg.get("protection", "terminal_apps") == list(DEFAULT_TERMINAL_APPS)
    assert cfg.get("protection", "fs_reads_never_read") is True
    cfg.set("protection", "never_read", ["~/mine"])
    cfg.set("protection", "fs_reads_never_read", True)
    cfg.ensure_privacy_defaults()
    assert cfg.get("protection", "never_read") == ["~/mine"]                     # the owner's edit stays
    assert cfg.get("protection", "fs_reads_never_read") is True


def test_the_daemon_builds_protection_from_the_owners_config(home: Path) -> None:
    """The daemon reads [protection] once at start and hands the same object to its fs tools (and, later, its shell)."""
    cfg = Config(home)
    cfg.set("protection", "never_read", ["~/.aws"])
    cfg.set("protection", "fs_reads_never_read", True)
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    try:
        prot = d.daemon.protection
        assert prot is d.daemon.gateway.roots.protection
        assert [e.id for e in prot.reads] == ["read:~/.aws"] and prot.fs_reads_never_read is True
        assert [e.id for e in prot.writes] == [f"write:{e}" for e in DEFAULT_NEVER_WRITE]
        assert prot.ferb_home == home.resolve()
    finally:
        d.stop()


def test_an_upload_is_checked_as_typed_too(daemon: RunningDaemon, tmp_path: Path) -> None:
    """Once ~/.ssh becomes a link after start, ~/.ssh/id still names the store: an upload of it is refused, as an
    fs.read of it is."""
    home, fake = daemon.daemon.home, tmp_path / "fakehome"
    (fake / ".ssh").mkdir(parents=True)
    (fake / "elsewhere").mkdir()
    (fake / "elsewhere" / "id").write_text("PRIVATE-KEY-BYTES")
    driver = _NoBrowser()
    grant, _ = gateway_setup(daemon, {"browser:act", "fs:read"}, None, [BrowserAct(driver, OWNER_BROWSER)])  # type: ignore[list-item]
    daemon.daemon.gateway.roots.protection = Protection(fake, home, ["~/.ssh"], [], [])
    (fake / ".ssh").rmdir()
    (fake / ".ssh").symlink_to(fake / "elsewhere")
    r = gateway_tool_call(daemon, grant, "browser.act", tab="t1", element="e1", action="upload",
                          files=[str(fake / ".ssh" / "id")])
    assert r["is_error"] and "~/.ssh" in r["content"]["error"] and driver.used == []
    r = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".ssh" / "id"))
    assert r["is_error"] and "~/.ssh" in r["content"]["error"] and "PRIVATE-KEY-BYTES" not in str(r)


class _OneTab:
    """A browser driver that records what an upload is handed."""
    def __init__(self) -> None:
        self.actions: list[Any] = []

    def tab_url(self, tab_id: str) -> str | None:
        return "https://example.com/form"

    def element_info(self, tab_id: str, element_id: str) -> None:
        return None

    def owned_by(self, tab_id: str, owner: str, borrow: bool = True) -> Any:
        return type("Tab", (), {"tab_id": tab_id})()

    def element_ref(self, tab: Any, element_id: str) -> str:
        return element_id

    async def act(self, el: Any, action: Any, intent_id: str) -> Any:
        from ferb.core.types import ActionResult
        self.actions.append(action)
        return ActionResult(True, "1 file(s) set", {})


def test_a_relative_upload_path_is_the_workspaces_in_the_check_and_the_read(daemon: RunningDaemon) -> None:
    """`cv.pdf` means ~/.ferb/workspace/cv.pdf, as for every other file tool: the check reads it so (`../run/token-x`
    is FERB's run folder, refused) and the driver is handed that absolute path, with the after-open check."""
    home = daemon.daemon.home
    (home / "run").mkdir(exist_ok=True)
    (home / "run" / "token-x").write_text("secret-token")
    (home / "workspace" / "cv.pdf").write_bytes(b"%PDF")
    driver = _OneTab()
    grant, _ = gateway_setup(daemon, {"browser:act"}, None, [BrowserAct(driver, OWNER_BROWSER)])  # type: ignore[list-item]
    r = gateway_tool_call(daemon, grant, "browser.act", tab="t1", element="e1", action="upload",
                          files=["../run/token-x"])
    assert r["is_error"] and "FERB's own" in r["content"]["error"] and driver.actions == []
    r = gateway_tool_call(daemon, grant, "browser.act", tab="t1", element="e1", action="upload", files=["cv.pdf"])
    assert not r["is_error"], r
    [action] = driver.actions
    assert action.files == (str(home / "workspace" / "cv.pdf"),) and action.files_closed is not None


class _Undeclared:
    """A tool with file access that doesn't say which files it names."""
    name, description, scopes = "probe.undeclared", "writes somewhere", {"fs:write"}
    identities, risk, reversible = {Identity.OWNER, Identity.FERB}, RiskLevel.LOW, True

    class Input(BaseModel):
        where: str

    def __init__(self) -> None:
        self.runs = 0

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.READ, Target(site="example.com"))

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        self.runs += 1
        return ToolResult({"ok": True})


def test_a_files_tool_that_declares_nothing_is_refused(daemon: RunningDaemon) -> None:
    tool = _Undeclared()
    grant, _ = gateway_setup(daemon, {"fs:write"}, None, [tool])        # type: ignore[list-item]
    r = gateway_tool_call(daemon, grant, "probe.undeclared", where="x.txt")
    assert r["is_error"] and "doesn't declare the files" in r["content"]["error"] and tool.runs == 0
    refused = [n for n in walk(_tree(daemon, grant.task_id)) if n["kind"] == "tool.refused"]
    assert [(n["input"]["tool"], n["error"]) for n in refused] == [("probe.undeclared", "no file_paths declared")]


def test_a_write_that_cant_tell_its_folder_fails_cleanly_and_can_be_retried(daemon: RunningDaemon,
                                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    """When FERB can't tell which folder it opened, nothing is written and the call is a plain failure, not an unknown
    outcome: the same write runs on the next try (P19 doesn't hold it)."""
    import ferb.act.fs.files as files
    grant, _ = gateway_setup(daemon, {"fs:read", "fs:write"}, None, [])
    target = daemon.daemon.home / "workspace" / "notes.txt"
    target.write_text("before")
    real = files.opened_path

    def blind(fd: int) -> Path:
        raise OSError(5, "Input/output error")
    monkeypatch.setattr(files, "opened_path", blind)
    r = gateway_tool_call(daemon, grant, "fs.write", path="notes.txt", content="after", overwrite=True)
    assert r["is_error"] and "couldn't tell which folder" in r["content"]["error"] and target.read_text() == "before"
    monkeypatch.setattr(files, "opened_path", real)
    r = gateway_tool_call(daemon, grant, "fs.write", path="notes.txt", content="after", overwrite=True)
    assert not r["is_error"] and target.read_text() == "after"

    async def states() -> list[str]:
        return [row[0] for row in daemon.daemon.db.conn.execute(
            "SELECT state FROM action_ledger WHERE capability = 'fs.write' ORDER BY rowid")]
    assert daemon.run(states()) == ["failed", "done"]


def test_fs_write_never_makes_a_folder_inside_ferbs_state(tmp_path: Path) -> None:
    """mkdir -p through a folder swapped for a link after the gateway's check would make folders in FERB's run folder:
    each missing folder is made inside a folder FERB opened and checked, so nothing is made there."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws, run = ferb / "workspace", ferb / "run"
    ws.mkdir(parents=True)
    run.mkdir()
    (ws / "a").symlink_to(run)
    tool = FsWrite()
    result = asyncio.run(tool.run(tool.Input(path="a/b/c/notes.txt", content="x"), _ctx(ws, Protection(home, ferb, [], [], []))))
    assert result.is_error and "closed by ferb_state" in result.content["error"]
    assert list(run.iterdir()) == []
    result = asyncio.run(tool.run(tool.Input(path="d/e/notes.txt", content="x"), _ctx(ws, Protection(home, ferb, [], [], []))))
    assert not result.is_error and (ws / "d" / "e" / "notes.txt").read_text() == "x"
