# tests/test_protection.py
"""The protection lists (SQ3 revised, SQ5, D27): where FERB's shell may never read or write, what code always adds
(FERB's own state), and which rules the escape hatch may lift (D31). FERB's fs tools never touch FERB's own state."""
import asyncio
import os
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
from tests.conftest import Answers, RunningDaemon, gateway_setup, gateway_tool_call
from tests.support.trash import use_trash


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
        d = Daemon(home=home, watchers=False, distill=False, retention=False, resources=False, notifications=False,
                   snapshots=False, watch_owner_input=False, preload_secrets=False)
        await d.start()
    with pytest.raises(BadProtection, match="workspace"):
        asyncio.run(start())
    assert not (home / "ferb.db").exists()


def test_the_daemon_refuses_to_start_with_a_malformed_protection_list(home: Path) -> None:
    Config(home).set("protection", "never_read", "~/.ssh")

    async def start() -> None:
        d = Daemon(home=home, watchers=False, distill=False, retention=False, resources=False, notifications=False,
                   snapshots=False, watch_owner_input=False, preload_secrets=False)
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


def test_fs_tools_follow_the_never_read_and_never_write_lists(tmp_path: Path) -> None:
    """Gateway decision 2 (2026-10-30): FERB's file tools follow both lists like the shell: a never-read store is closed
    to their reads, writes and removals, a never-write place to their writes and removals (the gateway asks the owner
    to lift either). FERB's own state is never opened, and the old fs_reads_never_read switch is gone."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace" / "uploads").mkdir(parents=True)
    p = Protection(home, ferb, ["~/.ssh"], ["~/.zshrc", "~/.ssh"], [])
    for kind in (Kind.READ, Kind.WRITE_FILE, Kind.TRASH):
        assert p.fs_denied(home / ".ssh" / "id_ed25519", kind) == "read:~/.ssh", kind
        assert p.fs_denied(home / "notes.txt", kind) is None, kind
        assert p.fs_denied(ferb / "ferb.db", kind) == "ferb_state", kind
    assert p.fs_denied(home / ".zshrc", Kind.READ) is None                      # readable, not changeable
    assert p.fs_denied(home / ".zshrc", "write") == "write:~/.zshrc"
    assert p.fs_denied(home / ".zshrc", Kind.TRASH) == "write:~/.zshrc"
    assert p.fs_rules(home / ".ssh" / "authorized_keys", "write") == ["read:~/.ssh", "write:~/.ssh"]
    assert p.fs_rules(home / ".ssh" / "authorized_keys", "read") == ["read:~/.ssh"]
    assert p.fs_rules(home / ".zshrc", "metadata") == [] and p.fs_rules(home / ".ssh" / "x", "metadata") == []
    assert not hasattr(p, "fs_reads_never_read")


def test_ferbs_own_service_definition_is_never_changed_or_lifted(tmp_path: Path) -> None:
    """Rule 10: FERB's launchd definitions (~/Library/LaunchAgents/com.ferb.daemon.plist and the app's login fallback)
    are FERB's own: file tools may read them, never change or remove them (nor the folder holding them), whatever
    the lists say, and no lift reopens them. The labels match the installer's."""
    from ferb.device.install import APP_LOGIN_LABEL, LABEL
    from ferb.policy.protection import FERB_SERVICE, SERVICE_LABELS
    assert SERVICE_LABELS == (LABEL, APP_LOGIN_LABEL)
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    agents = home / "Library" / "LaunchAgents"
    agents.mkdir(parents=True)
    (agents / "com.ferb.daemon.plist").write_text("<plist/>")
    p = Protection(home, ferb, [], [], [])
    for label in SERVICE_LABELS:
        plist = agents / f"{label}.plist"
        assert p.fs_denied(plist, "read") is None
        assert p.fs_rules(plist, "write") == [FERB_SERVICE] and p.fs_rules(plist, Kind.TRASH) == [FERB_SERVICE]
    assert p.fs_denied(agents, Kind.TRASH) == FERB_SERVICE and p.fs_denied(agents / "other.plist", "write") is None
    assert not p.liftable(FERB_SERVICE)
    assert any(r["id"] == FERB_SERVICE and r["liftable"] is False for r in p.rules())


def test_the_service_definition_inside_ferb_app_is_never_changed_or_lifted(tmp_path: Path) -> None:
    """Rule 10, notes/daemon.md decision 2: the background service's definition FERB.app registers with macOS
    (Contents/Library/LaunchAgents/com.ferb.daemon.plist) is refused to FERB's file tools as FERB's own service, not
    only as part of the app; reading it stays open, no lift reopens it, and the shell profile denies writing it."""
    from ferb.act.shell.sandbox import Sandbox
    from ferb.device.install import BUNDLE_SERVICE
    from ferb.policy.protection import BUNDLE_SERVICE as PROTECTED, FERB_SERVICE
    assert PROTECTED == BUNDLE_SERVICE
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    app = tmp_path / "Applications" / "FERB.app"
    definition = app / BUNDLE_SERVICE
    definition.parent.mkdir(parents=True)
    definition.write_text("<plist/>")
    p = Protection(home, ferb, [], ["~/Library/LaunchAgents"], [], app_bundle=app)
    assert p.fs_denied(definition, "read") is None
    for access in ("write", Kind.TRASH, Kind.DELETE_PERMANENT):
        assert p.fs_rules(definition, access) == [FERB_SERVICE], access
    assert p.fs_denied(definition.parent, Kind.TRASH) in (FERB_SERVICE, "ferb_app")   # nor the folder holding it
    assert not p.liftable(FERB_SERVICE)
    assert any(r["id"] == FERB_SERVICE and str(definition) in r["path"] for r in p.rules())
    profile = Sandbox(p, tmp_path / "profiles", listeners=lambda: set()).profile()
    assert f'(deny file-write* (literal "{definition.resolve()}"))' in profile


def test_a_daemon_not_run_from_ferb_app_closes_the_installed_ferb_app(tmp_path: Path,
                                                                     monkeypatch: pytest.MonkeyPatch) -> None:
    """Past bug (12e review): a daemon run from ~/.ferb/venv had no FERB.app to protect, so its tools and shell could
    rewrite the installed FERB.app (whose signature decides which process is FERB.app) and its service definition.
    Now the installed one is closed as the bundled one is; another app with that name is not FERB's."""
    import plistlib
    from ferb import paths
    from ferb.device.install import BUNDLE_SERVICE
    from ferb.policy.protection import FERB_APP, FERB_SERVICE
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    app, other = tmp_path / "Applications" / "FERB.app", tmp_path / "Other" / "FERB.app"
    for bundle, ident in ((app, "com.ferb.FERB"), (other, "com.example.other")):
        (bundle / BUNDLE_SERVICE).parent.mkdir(parents=True)
        (bundle / "Contents" / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": ident}))
    assert paths.installed_app((tmp_path / "none" / "FERB.app", other, app)) == app
    assert paths.installed_app((other,)) is None
    monkeypatch.setattr(paths, "app_bundle", lambda prefix=None: None)          # run from ~/.ferb/venv
    monkeypatch.setattr(paths, "installed_app", lambda candidates=None: app)
    p = protection.load(lambda section, key, default: default, home, ferb)
    assert p.fs_rules(app / "Contents" / "MacOS" / "FERB", "write") == [FERB_APP]
    assert p.fs_rules(app / BUNDLE_SERVICE, "write") == [FERB_SERVICE]
    assert p.fs_denied(app / "Contents" / "Info.plist", "read") is None


def test_removing_a_folder_that_holds_a_never_read_store_is_refused(tmp_path: Path) -> None:
    """Trashing ~/.config would hand over ~/.config/gh in the Trash: a removal of a folder holding a never-read store is
    closed with that store's rule. Reading the folder itself (a listing) is fine."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    entries = ["~/.config/gh", "~/Library/Application Support/Google/Chrome"]
    closed = Protection(home, ferb, entries, [], [])
    for access in ("remove", Kind.TRASH, Kind.DELETE_PERMANENT):
        assert closed.fs_denied(home / ".config", access) == "read:~/.config/gh", access
        assert (closed.fs_denied(home / "Library" / "Application Support", access)
                == "read:~/Library/Application Support/Google/Chrome"), access
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

    async def start(self, *args: Any, **kwargs: Any) -> dict[str, Any]:
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
    no = Answers(daemon, choice=2)                                      # the owner says no to reading ~/.ssh
    for files in ([str(home / "run" / "token-x")], [str(home / "ferb.db")], [str(fake / ".ssh" / "id")]):
        r = gateway_tool_call(daemon, grant, "browser.act", tab="t1", element="e1", action="upload", files=files)
        assert r["is_error"] and ("FERB's own" in r["content"]["error"] or "~/.ssh" in r["content"]["error"]), files
        assert "secret-token" not in str(r) and "PRIVATE-KEY-BYTES" not in str(r)
    r = gateway_tool_call(daemon, grant, "recording.inspect", path=str(fake / ".ssh" / "old.mp4"))
    assert r["is_error"] and "~/.ssh" in r["content"]["error"]           # the owner's No to ~/.ssh stays final
    r = gateway_tool_call(daemon, grant, "browser.record", tab="t1", path=str(home / "run" / "task.mp4"))
    assert r["is_error"] and "FERB's own" in r["content"]["error"]
    no.stop()
    assert driver.used == [] and recordings.started == [] and not (home / "workspace" / "uploads").exists()
    assert not (home / "run" / "task.mp4").exists() and len(no.asked) == 1


def test_recording_stop_declares_the_video_it_publishes() -> None:
    from ferb.act.recording import RecordingManager
    manager = RecordingManager()
    stop = RecordingStop(manager)
    assert stop.file_paths(stop.Input(id="rec-x")) == []
    manager.records["rec-x"] = type("R", (), {"path": Path("/somewhere/task.mp4")})()   # type: ignore[assignment]
    assert stop.file_paths(stop.Input(id="rec-x")) == [("/somewhere/task.mp4", "write")]


def _ctx(ws: Path, prot: Protection, lifted: bool = False) -> ToolContext:
    """`lifted`: the owner lifted the lists' rules for this call (only FERB's own never-lifted rules close)."""
    from ferb.policy.protection import NEVER_LIFTED

    def closed(path: Path, access: str) -> str | None:
        rule = prot.fs_denied(path, access)
        return f"closed by {rule}" if rule and (not lifted or rule in NEVER_LIFTED) else None
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


def test_a_never_read_store_asks_once_per_task_for_reads_and_per_call_for_changes(daemon: RunningDaemon,
                                                                                 tmp_path: Path) -> None:
    """Gateway decision 2, hands decision 1: a file tool's read of a protected folder asks the owner once for the task
    (every later read of that folder runs); a change asks every time; a No refuses with nothing read or written."""
    fake = tmp_path / "fakehome"
    (fake / ".aws").mkdir(parents=True)
    (fake / ".aws" / "config").write_text("region = eu-west-1\n")
    (fake / ".aws" / "credentials").write_text("[default]\n")
    roots = daemon.daemon.gateway.roots
    grant, _ = gateway_setup(daemon, {"fs:read", "fs:write"}, None, [])
    roots.protection = Protection(fake, daemon.daemon.home, ["~/.aws"], [], [])
    no = Answers(daemon, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".aws" / "config"))
    finally:
        no.stop()
    assert r["is_error"] and "eu-west-1" not in str(r) and len(no.asked) == 1
    assert "read ~/.aws for this task" in no.asked[0].summary
    r = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".aws" / "credentials"))   # the No is final
    assert r["is_error"] and "already said no" in r["content"]["error"]


def test_a_yes_to_reading_a_protected_folder_covers_the_task_and_changes_ask_each_time(daemon: RunningDaemon,
                                                                                     tmp_path: Path) -> None:
    fake = tmp_path / "fakehome"
    (fake / ".aws").mkdir(parents=True)
    (fake / ".aws" / "config").write_text("region = eu-west-1\n")
    (fake / ".aws" / "credentials").write_text("[default]\n")
    grant, _ = gateway_setup(daemon, {"fs:read", "fs:write"}, None, [])
    daemon.daemon.gateway.roots.protection = Protection(fake, daemon.daemon.home, ["~/.aws"], [], [])
    yes = Answers(daemon, choice=1)
    try:
        first = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".aws" / "config"))
        again = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".aws" / "credentials"))
        listed = gateway_tool_call(daemon, grant, "fs.list", path=str(fake / ".aws"))
        asked_reads = len(yes.asked)
        w1 = gateway_tool_call(daemon, grant, "fs.write", path=str(fake / ".aws" / "new"), content="x")
        w2 = gateway_tool_call(daemon, grant, "fs.write", path=str(fake / ".aws" / "new2"), content="y")
    finally:
        yes.stop()
    assert not first["is_error"] and "eu-west-1" in first["content"]["text"]
    assert not again["is_error"] and not listed["is_error"] and asked_reads == 1
    assert not w1["is_error"] and not w2["is_error"] and len(yes.asked) == 3              # each change asked
    assert "change ~/.aws once" in yes.asked[1].summary
    assert (fake / ".aws" / "new").read_text() == "x" and (fake / ".aws" / "new2").read_text() == "y"


def test_a_never_write_place_asks_before_a_file_tool_changes_it(daemon: RunningDaemon, tmp_path: Path) -> None:
    """~/.zshrc (never-write) reads freely; a write asks first and a No leaves it as it was (no S1 copy needed)."""
    fake = tmp_path / "fakehome"
    fake.mkdir()
    (fake / ".zshrc").write_text("export A=1\n")
    daemon.daemon.gateway.roots.protection = Protection(fake, daemon.daemon.home, [], ["~/.zshrc"], [])
    grant, _ = gateway_setup(daemon, {"fs:read", "fs:write"}, None, [])
    assert not gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".zshrc"))["is_error"]
    no = Answers(daemon, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "fs.edit", path=str(fake / ".zshrc"), old_text="A=1", new_text="A=2")
    finally:
        no.stop()
    assert r["is_error"] and len(no.asked) == 1 and (fake / ".zshrc").read_text() == "export A=1\n"


def test_config_gets_the_protection_defaults_once(daemon: RunningDaemon) -> None:
    cfg = daemon.daemon.config
    assert cfg.get("protection", "never_read") == list(DEFAULT_NEVER_READ)
    assert cfg.get("protection", "never_write") == list(DEFAULT_NEVER_WRITE)
    assert cfg.get("protection", "terminal_apps") == list(DEFAULT_TERMINAL_APPS)
    assert cfg.get("protection", "fs_reads_never_read") is None                  # the switch is gone (decision 2)
    cfg.set("protection", "never_read", ["~/mine"])
    cfg.set("protection", "fs_reads_never_read", True)                          # an old config keeps it, harmlessly
    cfg.ensure_privacy_defaults()
    assert cfg.get("protection", "never_read") == ["~/mine"]                     # the owner's edit stays
    assert cfg.get("protection", "fs_reads_never_read") is True


def test_the_daemon_builds_protection_from_the_owners_config(home: Path) -> None:
    """The daemon reads [protection] once at start and hands the same object to its fs tools (and, later, its shell)."""
    cfg = Config(home)
    cfg.set("protection", "never_read", ["~/.aws"])
    cfg.set("protection", "fs_reads_never_read", True)                          # an old config's switch: ignored
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    try:
        prot = d.daemon.protection
        assert prot is d.daemon.gateway.roots.protection
        assert [e.id for e in prot.reads] == ["read:~/.aws"]
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
    no = Answers(daemon, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "browser.act", tab="t1", element="e1", action="upload",
                              files=[str(fake / ".ssh" / "id")])
        assert r["is_error"] and "~/.ssh" in r["content"]["error"] and driver.used == []
        r = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".ssh" / "id"))
        assert r["is_error"] and "~/.ssh" in r["content"]["error"] and "PRIVATE-KEY-BYTES" not in str(r)
    finally:
        no.stop()
    assert len(no.asked) == 1                                          # asked as the store, by its typed spelling


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


# ---------- FERB.app (L27): the daemon's own code is never FERB's tools' to change (rule 10) ----------

def _app(root: Path) -> Path:
    app = root / "FERB.app"
    site = app / "Contents" / "Resources" / "python" / "lib" / "python3.12" / "site-packages"
    (site / "ferb" / "policy").mkdir(parents=True)
    (site / "ferb" / "policy" / "protection.py").write_text("# FERB's protection rules\n")
    (app / "Contents" / "Helpers").mkdir(parents=True)
    (app / "Contents" / "Helpers" / "ferb-secrets").write_bytes(b"helper")
    return app


def test_ferb_app_is_closed_to_writes_and_removes_and_never_liftable(tmp_path: Path) -> None:
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    for app in (_app(tmp_path / "Applications"), _app(home / "Applications")):      # /Applications or ~/Applications
        p = Protection(home, ferb, [], ["/Applications"], [], app_bundle=app)
        site = app / "Contents" / "Resources" / "python" / "lib" / "python3.12" / "site-packages"
        for target in (site / "sitecustomize.py", site / "planted.pth", site / "ferb" / "policy" / "protection.py",
                       app / "Contents" / "Helpers" / "ferb-secrets", app):
            assert p.fs_denied(target, "write") == "ferb_app", target
            assert p.fs_denied(target, "remove") == "ferb_app", target
            assert p.write_denied(target) == "ferb_app", target
        assert p.fs_denied(app.parent, Kind.TRASH) == "ferb_app"              # a folder that holds it, taken away
        assert p.fs_denied(site / "ferb" / "policy" / "protection.py", "read") is None   # reading its code is fine
        assert p.fs_denied(app.parent / "Other.app", "write") is None
        link = tmp_path / f"link-{app.parent.parent.name}"
        link.symlink_to(app)
        assert p.fs_denied(link / "Contents" / "Helpers" / "ferb-secrets", "write") == "ferb_app"   # through a link
        assert p.fs_denied(Path(str(app).upper()) / "Contents" / "x", "write") == "ferb_app"     # another spelling
        assert not p.liftable("ferb_app") and p.liftable("write:/Applications")
        assert {"id": "ferb_app", "kind": "write", "path": str(app), "liftable": False} in p.rules()
    assert Protection(home, ferb, [], [], []).fs_denied(_app(tmp_path / "x"), "write") is None   # from a checkout


def test_protection_load_closes_the_app_ferb_runs_from(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.paths
    home, ferb_home = tmp_path / "home", tmp_path / "home" / ".ferb"
    ferb_home.mkdir(parents=True)
    app = _app(tmp_path / "Applications")
    monkeypatch.setattr(ferb.paths, "app_bundle", lambda prefix=None: app)
    p = protection.load(lambda section, key, default: default, home, ferb_home)
    assert p.fs_denied(app / "Contents" / "Helpers" / "ferb-secrets", "write") == "ferb_app"


def test_fs_write_into_ferb_app_is_refused(daemon: RunningDaemon, tmp_path: Path) -> None:
    app = _app(tmp_path / "Applications")
    prot = Protection(daemon.daemon.protection.home, daemon.daemon.home, [], [], [], app_bundle=app)
    daemon.daemon.protection = daemon.daemon.gateway.roots.protection = prot
    grant, _ = gateway_setup(daemon, {"fs:read", "fs:write"}, None, [])
    site = app / "Contents" / "Resources" / "python" / "lib" / "python3.12" / "site-packages"
    r = gateway_tool_call(daemon, grant, "fs.write", path=str(site / "sitecustomize.py"), content="import os")
    assert r["is_error"] and "FERB's own code" in r["content"]["error"] and not (site / "sitecustomize.py").exists()
    r = gateway_tool_call(daemon, grant, "fs.trash", path=str(app))
    assert r["is_error"] and "FERB's own code" in r["content"]["error"] and app.exists()
    assert "passwords" not in r["content"]["error"]                          # not the never-read store message


# ---------- other spellings the kernel accepts for the same file (L27 fix round 2) ----------

def _aliases(path: Path) -> list[Path]:
    """The same existing file or folder through the Data volume's own mount and through volfs (by device and inode)."""
    st = os.stat(path)
    return [Path("/System/Volumes/Data" + os.path.realpath(path)), Path(f"/.vol/{st.st_dev}/{st.st_ino}")]


def test_volume_and_inode_aliases_of_protected_paths_are_still_protected(tmp_path: Path) -> None:
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    (ferb / "config.toml").write_text("x")
    gh = home / ".config" / "gh"
    gh.mkdir(parents=True)
    app = _app(tmp_path / "Applications")
    p = Protection(home, ferb, ["~/.config/gh"], [], [], app_bundle=app)
    cases = [(ferb, Kind.TRASH, "ferb_state"), (ferb / "config.toml", "write", "ferb_state"),
             (ferb / "config.toml", "read", "ferb_state"), (app, Kind.TRASH, "ferb_app"),
             (app.parent, Kind.TRASH, "ferb_app"), (app / "Contents" / "Helpers" / "ferb-secrets", "write", "ferb_app"),
             (gh, "read", "read:~/.config/gh"), (home / ".config", Kind.TRASH, "read:~/.config/gh")]
    for target, access, rule in cases:
        for alias in _aliases(target):
            assert alias.exists(), alias
            assert p.fs_denied(alias, access) == rule, (alias, access)
    contents = os.stat(app / "Contents")                     # a file inside, or a new one, through its folder's inode
    inside = Path(f"/.vol/{contents.st_dev}/{contents.st_ino}")
    assert p.fs_denied(inside / "Helpers" / "ferb-secrets", "write") == "ferb_app"
    assert p.fs_denied(inside / "Resources" / "planted.pth", "write") == "ferb_app"
    ws = os.stat(ferb / "workspace")                         # the workspace stays open under every spelling
    assert p.fs_denied(Path(f"/.vol/{ws.st_dev}/{ws.st_ino}/notes.txt"), "write") is None
    assert p.fs_denied(Path("/System/Volumes/Data" + os.path.realpath(ferb / "workspace")) / "n.txt", "write") is None
    assert p.write_denied(_aliases(ferb / "config.toml")[1]) == "ferb_state"     # the shell's check too


def test_a_volfs_path_ferb_cant_map_is_refused(tmp_path: Path) -> None:
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    p = Protection(home, ferb, [], [], [])
    assert p.fs_denied(Path("/.vol/1/2/whatever"), "write") == "ferb_state"   # no such inode: can't tell, so closed
    assert p.fs_denied(Path("/.vol/1/2/whatever"), "read") == "ferb_state"


def test_fs_trash_checks_the_folder_it_really_opened(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """FsTrash checks the item's folder as macOS opened it (F_GETPATH), so the check sees the kernel's own spelling,
    never an alias, before anything moves."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws = ferb / "workspace"
    ws.mkdir(parents=True)
    (ferb / "config.toml").write_text("x")
    seen: list[Path] = []

    def closed(path: Path, access: str) -> str | None:
        seen.append(path)                                      # a check that only knows FERB's own spelling
        return "closed" if path in (Path(os.path.realpath(ferb)), Path(os.path.realpath(ferb)) / "config.toml") else None
    trash = tmp_path / ".Trash"
    trash.mkdir()
    use_trash(monkeypatch, trash)
    ctx = ToolContext("t", "w", "r", Identity.OWNER, frozenset({"fs:write"}), "dev", None, str(ws), (), (),
                      path_closed=closed)
    folder = os.stat(ferb)
    data = "/System/Volumes/Data"
    for alias in (Path(f"/.vol/{folder.st_dev}/{folder.st_ino}/config.toml"),       # the file, by its folder's inode
                  Path(data + os.path.realpath(ferb / "config.toml")), Path(data + os.path.realpath(ferb))):
        r = asyncio.run(FsTrash().run(FsTrash.Input(path=str(alias)), ctx))
        assert r.is_error and "closed" in r.content["error"], (alias, r.content)
    for alias in (_aliases(ferb / "config.toml")[1], _aliases(ferb)[1]):         # an item by its own inode: its folder
        r = asyncio.run(FsTrash().run(FsTrash.Input(path=str(alias)), ctx))     # can't be opened, so nothing moves
        assert r.is_error, (alias, r.content)
    assert list(trash.iterdir()) == [] and (ferb / "config.toml").exists()
    assert any(p == Path(os.path.realpath(ferb)) / "config.toml" for p in seen)


def test_a_path_with_a_nul_byte_is_checked_without_raising(tmp_path: Path) -> None:
    """The kernel can't open such a path: Protection still answers (the tool then refuses the path cleanly)."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    p = Protection(home, ferb, [], [], [])
    assert p.fs_denied(Path("notes\x00.txt"), "write") is None
    assert p.fs_denied(ferb / "config\x00.toml", "write") == "ferb_state"


def _trash_bin(tmp_path: Path) -> Path:
    trash = tmp_path / ".Trash"                           # named as a Trash: FsTrash moves into nothing else
    trash.mkdir()
    return trash


def test_fs_trash_moves_only_what_it_checked(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """FsTrash checks the item in its folder as macOS opened the folder, and moves exactly that: if something else
    stood at the name when it moved (swapped after the check), FERB leaves it in the Trash, never moves it out, and
    says so with both paths."""
    import socket
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws = ferb / "workspace"
    ws.mkdir(parents=True)
    trash = _trash_bin(tmp_path)
    victim = ws / "notes.txt"
    swap = {"on": False}

    def attacker() -> None:                               # swaps the item after FERB checked it
        if swap["on"]:
            victim.unlink()
            victim.write_text("something else")
    use_trash(monkeypatch, trash, before_move=attacker)
    ctx = _ctx(ws, Protection(home, ferb, [], [], []))

    def go(path: Path) -> ToolResult:
        return asyncio.run(FsTrash().run(FsTrash.Input(path=str(path)), ctx))
    victim.write_text("mine")
    where = os.path.realpath(victim)
    swap["on"] = True
    r = go(victim)
    landed = os.path.realpath(trash / "notes.txt")
    assert r.is_error and "changed" in r.content["error"] and landed in r.content["error"], r.content
    assert r.content == {"error": r.content["error"], "path": where, "trashed_to": landed, "verified": False}
    assert (trash / "notes.txt").read_text() == "something else" and not victim.exists()   # left in the Trash
    swap["on"] = False
    locked = ws / "locked.txt"
    locked.write_text("x")
    locked.chmod(0o000)                                    # FERB never opens the item itself to check it
    sock_path = ws / "s.sock"
    server = socket.socket(socket.AF_UNIX)
    monkeypatch.chdir(ws)
    server.bind("s.sock")                                  # relative: the full path is longer than AF_UNIX allows
    try:
        for item in (locked, sock_path):
            before = os.lstat(item)
            r = go(item)
            assert not r.is_error, (item, r.content)
            assert (trash / item.name).exists() or (trash / item.name).is_socket()
            assert r.content["trashed_to"] == os.path.realpath(trash / item.name)
            assert (r.content["device"], r.content["inode"]) == (before.st_dev, before.st_ino)
    finally:
        server.close()
        (trash / "locked.txt").chmod(0o600)


@pytest.mark.parametrize("protected, name", [("Library/LaunchAgents", "evil.plist"), ("", ".zshenv")])
def test_fs_trash_never_puts_back_through_a_folder_swapped_for_a_link(tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
                                                                       protected: str, name: str) -> None:
    """Reviewer's round-3 reproduction: the item is swapped after the check, and the folder becomes a link to a
    never-write folder. A put-back by path renamed the attacker's plist into ~/Library/LaunchAgents (or wrote
    ~/.zshenv). FERB never moves anything out of the Trash after a mismatch: it stays there (rule 10)."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws = ferb / "workspace"
    ws.mkdir(parents=True)
    target = home / protected
    target.mkdir(parents=True, exist_ok=True)
    trash = _trash_bin(tmp_path)
    proj = ws / "proj"
    proj.mkdir()
    item = proj / name
    item.write_text("checked item")
    before = sorted(p.name for p in target.iterdir())

    def swap_item() -> None:                              # after the check: what moves isn't what was checked
        item.unlink()
        item.write_text("attacker payload")

    def swap_folder() -> None:                            # the folder becomes a link
        proj.rename(ws / "proj-moved")
        proj.symlink_to(target)
    use_trash(monkeypatch, trash, before_move=swap_item, after_move=swap_folder)
    ctx = _ctx(ws, Protection(home, ferb, [], DEFAULT_NEVER_WRITE, []))
    r = asyncio.run(FsTrash().run(FsTrash.Input(path=str(item)), ctx))
    assert sorted(p.name for p in target.iterdir()) == before, "nothing lands in the protected folder"
    assert r.is_error and "changed" in r.content["error"] and r.content["verified"] is False, r.content
    assert (trash / name).read_text() == "attacker payload" and list((ws / "proj-moved").iterdir()) == []


@pytest.mark.parametrize("victim_at", ["Library/LaunchAgents/com.owner.agent.plist", ".zshenv"])
def test_fs_trash_never_moves_a_trash_side_swap_out_of_the_trash(tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
                                                                  victim_at: str) -> None:
    """Reviewer's round-4 reproduction (trash_side_swap.py): right after the owner's file lands in the Trash, what is
    there is swapped for an attacker's file. The fd put-back planted "ATTACKER PAYLOAD" at the owner's file's place
    (LaunchAgents, ~/.zshenv) and said "nothing was moved". FERB never moves anything out of the Trash after a
    mismatch: the payload stays in the Trash, reported with both paths for the owner. (The swap itself needs write
    access to the Trash, which FERB's shell no longer has: test_shell_sandbox.)"""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws = ferb / "workspace"
    ws.mkdir(parents=True)
    victim = home / victim_at
    victim.parent.mkdir(parents=True, exist_ok=True)
    victim.write_text("owner's genuine file")
    evil = ws / "evil"
    evil.write_text("ATTACKER PAYLOAD")
    trash = _trash_bin(tmp_path)

    def swap_in_the_trash() -> None:
        os.rename(trash / victim.name, ws / "stolen")
        os.rename(evil, trash / victim.name)
    use_trash(monkeypatch, trash, after_move=swap_in_the_trash)
    where = os.path.realpath(victim)
    r = asyncio.run(FsTrash().run(FsTrash.Input(path=str(victim)),       # the owner lifted never-write for it
                                  _ctx(ws, Protection(home, ferb, [], DEFAULT_NEVER_WRITE, []), lifted=True)))
    assert not os.path.lexists(victim), "nothing is planted where the owner's file was"
    assert (trash / victim.name).read_text() == "ATTACKER PAYLOAD"
    landed = os.path.realpath(trash / victim.name)
    assert r.is_error and r.content["path"] == where and r.content["trashed_to"] == landed, r.content
    assert r.content["verified"] is False and "nothing was moved" not in r.content["error"]
    assert landed in r.content["error"] and "changed" in r.content["error"]


def test_fs_trash_moves_from_the_folder_it_checked_when_the_path_is_swapped(tmp_path: Path,
                                                                             monkeypatch: pytest.MonkeyPatch) -> None:
    """Reviewer's round-3 reproduction: the folder becomes a link to FERB's home while the item moves, then is swapped
    back. A move by path trashed ~/.ferb/config.toml and the put-back renamed it into the attacker's folder. FERB moves
    the item out of the folder it checked and holds open, so FERB's own file never moves (rule 10)."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws = ferb / "workspace"
    ws.mkdir(parents=True)
    (ferb / "config.toml").write_text("owner settings")
    out = home / "Desktop" / "proj"
    out.mkdir(parents=True)
    (out / "config.toml").write_text("decoy")
    decoy = os.stat(out / "config.toml")
    trash = _trash_bin(tmp_path)

    def swap_in() -> None:
        out.rename(out.parent / "proj-moved")
        out.symlink_to(ferb)

    def swap_back() -> None:
        out.unlink()
        (out.parent / "proj-moved").rename(out)
        (out / "config.toml").unlink(missing_ok=True)
    use_trash(monkeypatch, trash, before_move=swap_in, after_move=swap_back)
    ctx = _ctx(ws, Protection(home, ferb, [], [], []))
    where = Path(os.path.realpath(out)) / "config.toml"
    r = asyncio.run(FsTrash().run(FsTrash.Input(path=str(out / "config.toml")), ctx))
    assert (ferb / "config.toml").read_text() == "owner settings"
    assert (trash / "config.toml").read_text() == "decoy" and not (out / "config.toml").exists()
    assert not r.is_error and r.content == {"path": str(where), "trashed_to": os.path.realpath(trash / "config.toml"),
                                            "device": decoy.st_dev, "inode": decoy.st_ino}


def test_fs_trash_reports_where_the_item_went_even_when_it_cant_look_at_it(tmp_path: Path,
                                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    """Where the item went is known before anything that can fail after the move: emptied from the Trash at once, it
    is still reported (and recorded for the owner), never lost to an exception."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws = ferb / "workspace"
    ws.mkdir(parents=True)
    trash = _trash_bin(tmp_path)
    item = ws / "notes.txt"
    item.write_text("mine")
    use_trash(monkeypatch, trash, after_move=lambda: (trash / "notes.txt").unlink())
    r = asyncio.run(FsTrash().run(FsTrash.Input(path=str(item)), _ctx(ws, Protection(home, ferb, [], [], []))))
    assert r.is_error and r.content["trashed_to"] == os.path.realpath(trash / "notes.txt"), r.content
    assert r.content["path"] == os.path.realpath(item) and "confirm" in r.content["error"]
    assert r.content["verified"] is False


def test_fs_trash_never_replaces_what_is_already_in_the_trash(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws = ferb / "workspace"
    ws.mkdir(parents=True)
    trash = _trash_bin(tmp_path)
    (trash / "notes.txt").write_text("trashed last week")
    item = ws / "notes.txt"
    item.write_text("mine")
    use_trash(monkeypatch, trash)
    r = asyncio.run(FsTrash().run(FsTrash.Input(path=str(item)), _ctx(ws, Protection(home, ferb, [], [], []))))
    assert not r.is_error and (trash / "notes.txt").read_text() == "trashed last week"
    landed = Path(r.content["trashed_to"])
    assert landed.parent == Path(os.path.realpath(trash)) and landed.name != "notes.txt"
    assert landed.read_text() == "mine" and landed.suffix == ".txt" and not item.exists()


def test_fs_trash_refuses_a_trash_folder_that_isnt_one(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """The Trash folder is checked as opened too, each with its own refusal: a link standing in for it, a folder that
    isn't a Trash (named .Trash/.Trashes or inside one, the rule restore uses), a Trash inside FERB's own state
    (reached through a link above it), and a folder on another disk (a move there would be a copy)."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    ws = ferb / "workspace"
    ws.mkdir(parents=True)
    agents = home / "Library" / "LaunchAgents"
    agents.mkdir(parents=True)
    (ferb / ".Trash").mkdir()
    item = home / "Desktop" / "notes.txt"
    item.parent.mkdir()
    item.write_text("mine")
    ctx = _ctx(ws, Protection(home, ferb, [], DEFAULT_NEVER_WRITE, []))
    link = tmp_path / ".Trash"
    link.symlink_to(agents)
    (tmp_path / "up").symlink_to(home)
    (tmp_path / "Bin").mkdir()
    cases = ((link, "couldn't open the Trash at", "(Not a directory)"),           # O_NOFOLLOW|O_DIRECTORY: ENOTDIR
             (tmp_path / "Bin", "isn't a Trash folder", str(Path(os.path.realpath(tmp_path)) / "Bin")),
             (tmp_path / "up" / ".ferb" / ".Trash", "closed by ferb_state", ""),
             (Path("/dev"), "on a different disk from its Trash", "/dev"))
    for bin_, refusal, detail in cases:
        use_trash(monkeypatch, bin_)
        r = asyncio.run(FsTrash().run(FsTrash.Input(path=str(item)), ctx))
        assert r.is_error and refusal in r.content["error"] and detail in r.content["error"], (bin_, r.content)
        assert "trashed_to" not in r.content and item.read_text() == "mine"
        assert list(agents.iterdir()) == [] and list((ferb / ".Trash").iterdir()) == []


def test_the_trash_is_never_made_through_a_link(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """macOS is asked to make a missing Trash only after FERB checked that no link stands at it or above it; an
    existing Trash is never made again."""
    import ferb.act.fs.files as files
    made: list[Path] = []
    real = tmp_path / "real"
    real.mkdir()
    (tmp_path / "linked").symlink_to(real)

    def ask(answer: Path | None) -> None:
        def url(near: Path, create: bool) -> Path | None:
            if create:
                made.append(near)
                assert answer is not None
                answer.mkdir()
            return answer
        monkeypatch.setattr(files, "_trash_url", url)
    for bad in (tmp_path / "linked" / ".Trash", tmp_path / "dangling"):
        if bad.name == "dangling":
            bad.symlink_to(tmp_path / "nowhere")
        ask(bad)
        with pytest.raises(files.TrashRefused, match="link"):
            files.trash_folder(tmp_path)
    assert made == [] and not (real / ".Trash").exists() and not (tmp_path / "nowhere").exists()
    ask(tmp_path / "real" / ".Trash")                     # missing, no link anywhere: made
    assert files.trash_folder(tmp_path) == tmp_path / "real" / ".Trash" and made == [tmp_path]
    made.clear()
    assert files.trash_folder(tmp_path) == tmp_path / "real" / ".Trash" and made == []   # there: never made again
    home = tmp_path / "home"                              # macOS names none yet: FERB knows where it would be
    home.mkdir()
    monkeypatch.setattr(files, "_owner_home", lambda: home)

    def unnamed(near: Path, create: bool) -> Path | None:
        if not create:
            return None
        made.append(near)
        (home / ".Trash").mkdir()
        return home / ".Trash"
    monkeypatch.setattr(files, "_trash_url", unnamed)
    (home / ".Trash").symlink_to(tmp_path / "nowhere")
    with pytest.raises(files.TrashRefused, match="link"):
        files.trash_folder(tmp_path)
    assert made == []
    (home / ".Trash").unlink()
    assert files.trash_folder(tmp_path) == home / ".Trash" and made == [tmp_path]


def test_the_kernel_lookup_opens_only_folders_and_regular_files(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """A model can name any path: a device node, a FIFO or a socket is never opened to learn its name (its folder is),
    and a file FERB can't open still gets the kernel's spelling through its folder."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    p = Protection(home, ferb, [], [], [])
    fifo = ferb / "workspace" / "pipe"
    os.mkfifo(fifo)
    locked = ferb / "secret"
    locked.write_text("x")
    locked.chmod(0o000)
    opened: list[str] = []
    real_open = os.open

    def spy(path: Any, *args: Any, **kwargs: Any) -> int:
        opened.append(os.fsdecode(path))
        return real_open(path, *args, **kwargs)
    monkeypatch.setattr(protection.os, "open", spy)
    try:
        assert p.fs_denied(Path("/dev/null"), "write") is None and "/dev/null" not in opened
        assert p.fs_denied(fifo, "write") is None and str(fifo) not in opened
        st = os.stat(ferb)
        assert p.fs_denied(Path(f"/.vol/{st.st_dev}/{st.st_ino}/secret"), "write") == "ferb_state"   # through its folder
    finally:
        locked.chmod(0o600)


def test_the_kernel_lookup_never_follows_a_file_swapped_for_a_link_to_a_device(tmp_path: Path,
                                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    """A regular file swapped for a link to a device between the lookup's stat and its open is never opened through
    the link (O_NOFOLLOW): the device is never opened, and the file is named through its folder."""
    import stat as stat_
    ws = tmp_path / "ws"
    ws.mkdir()
    f = ws / "notes.txt"
    f.write_text("x")
    real_open = os.open
    devices: list[str] = []

    def swap_then_open(path: Any, flags: int, *args: Any, **kwargs: Any) -> int:
        if os.fsdecode(path) == str(f) and not f.is_symlink():
            f.unlink()
            f.symlink_to("/dev/null")                       # the swap, just before the lookup opens the file
        fd = real_open(path, flags, *args, **kwargs)
        if not stat_.S_ISDIR(os.fstat(fd).st_mode) and not stat_.S_ISREG(os.fstat(fd).st_mode):
            devices.append(os.fsdecode(path))
        return fd
    monkeypatch.setattr(protection.os, "open", swap_then_open)
    named = protection._kernel(f)
    monkeypatch.undo()
    assert devices == [] and f.is_symlink()
    assert named == Path(os.path.realpath(ws)) / "notes.txt"


def test_building_the_lists_never_opens_other_apps_folders(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Opening another app's data folder can raise macOS's "access data from other apps" prompt: the never-read and
    never-write entries are resolved without opening them; only FERB's own home and FERB.app are opened."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    group = home / "Library" / "Group Containers" / "group.com.apple.usernoted"
    group.mkdir(parents=True)
    opened: list[str] = []
    real_open = os.open

    def spy(path: Any, *args: Any, **kwargs: Any) -> int:
        opened.append(os.fsdecode(path))
        return real_open(path, *args, **kwargs)
    monkeypatch.setattr(protection.os, "open", spy)
    app = _app(tmp_path / "Applications")
    p = Protection(home, ferb, ["~/Library/Group Containers/group.com.apple.usernoted"], ["~/Library"], [],
                   app_bundle=app)
    assert not [o for o in opened if "Library" in o], opened
    alias = Path("/System/Volumes/Data" + os.path.realpath(group)) / "db"
    assert p.fs_denied(alias, "read") == "read:~/Library/Group Containers/group.com.apple.usernoted"
