# tests/test_protection.py
"""The protection lists (SQ3 revised, SQ5, D27): where FERB's shell may never read or write, what code always adds
(FERB's own state), and which rules the escape hatch may lift (D31). FERB's fs tools never touch FERB's own state."""
from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel

from ferb.clock.clock import FakeClock
from ferb.config import Config
from ferb.core.ids import SequentialIds
from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy.protection import (DEFAULT_NEVER_READ, DEFAULT_NEVER_WRITE, DEFAULT_TERMINAL_APPS, EXTRA_NEVER_READ,
                                    Protection)
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
    the way the volume does (on a case-sensitive volume this only closes more)."""
    p = _prot(tmp_path)
    home = tmp_path / "home"
    assert p.read_denied(home / ".FERB" / "Run" / "token-x") == "ferb_state"
    assert p.read_denied(home / ".Ferb" / "WORKSPACE" / "Uploads" / "cv.pdf") == "ferb_state"
    assert p.read_denied(home / ".ferb" / "Workspace" / "notes.txt") is None
    assert p.read_denied(home / "library" / "KEYCHAINS" / "login.keychain-db") == "read:~/Library/Keychains"
    assert p.write_denied(home / ".ZSHRC") == "write:~/.zshrc"
    assert p.fs_denied(home / ".FERB" / "ferb.db", Kind.READ) == "ferb_state"


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


class _Hookless:
    """An fs tool that names its file but gives FERB no way to resolve it (no real_path hook, no path argument)."""
    name, description, scopes = "fs.hookless", "hookless read", {"fs:read"}
    identities, risk, reversible = {Identity.OWNER, Identity.FERB}, RiskLevel.LOW, True

    class Input(BaseModel):
        where: str

    def __init__(self) -> None:
        self.runs = 0

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.READ, Target(path=args.where))

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        self.runs += 1
        return ToolResult({"text": Path(args.where).read_text()})


def test_an_fs_tool_without_a_path_hook_is_checked_on_the_path_it_names(daemon: RunningDaemon) -> None:
    tool = _Hookless()
    grant, _ = gateway_setup(daemon, {"fs:read"}, None, [tool])         # type: ignore[list-item]
    home = daemon.daemon.home
    (home / "run").mkdir(exist_ok=True)
    (home / "run" / "token-abc").write_text("secret-token")
    r = gateway_tool_call(daemon, grant, "fs.hookless", where=str(home / "run" / "token-abc"))
    assert r["is_error"] and "FERB's own" in r["content"]["error"] and tool.runs == 0
    (home / "workspace" / "notes.txt").write_text("fine")
    r = gateway_tool_call(daemon, grant, "fs.hookless", where=str(home / "workspace" / "notes.txt"))
    assert not r["is_error"] and tool.runs == 1


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
    assert cfg.get("protection", "fs_reads_never_read") is False
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
