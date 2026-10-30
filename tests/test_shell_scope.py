"""Hands decision 4 and the shell fix list (step 5a), with the real sandbox-exec on scratch folders in pytest's tmp_path:
the folder scope (no changes in the owner's visible folders unless the task names them; hidden folders, ~/Library,
temp and the workspace stay writable), the window-server deny, signals only within the command, the lift-order and
FERB-service rules, sends into lifted commands, process-group cleanup after the command's own process ended, pruning,
partial sends, `shell.kill`'s kind, durable host records and the shell-log retention. Nothing touches the owner's own
folders: each Protection is built over a fake home inside tmp_path."""
import asyncio
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.act.shell.manager import ProcHandle, ShellManager
from ferb.act.shell.pty_proc import PtyProc, group_members
from ferb.act.shell.sandbox import SANDBOX_EXEC, SCOPE_REFUSED, WINDOW_SERVER, Sandbox
from ferb.act.shell.tools import ShellKill
from ferb.core.errors import Refused
from ferb.core.types import ActionFacts, ActionSpec, Identity, ReceivedBy
from ferb.policy.protection import Protection
from ferb.policy.rules import REVERSIBLE, Kind
from tests.conftest import RunningDaemon
from tests.mackit import gateway_call as call, grant_task as task
from tests.support.sandbox_probes import lookups

pytestmark = pytest.mark.skipif(not shutil.which("sandbox-exec"), reason="sandbox-exec is not available here")
SHELL = {"shell", "fs:read"}


def _home(tmp_path: Path) -> tuple[Path, Path]:
    home = tmp_path / "home"
    for d in ("Documents/repo", "Projects", "Library/Caches", "Library/Mobile Documents/com~apple~CloudDocs",
              ".cache", ".ferb/workspace"):
        (home / d).mkdir(parents=True)
    (home / "Documents" / "report.txt").write_text("the owner's report")
    return home, home / ".ferb"


def _run(sb: Sandbox, command: str, folders: tuple[str, ...] = (), lift: str | None = None) -> subprocess.CompletedProcess[str]:
    argv = sb.argv(command, lift, folders)
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=30)
    finally:
        sb.cleanup(sb.profile_path(argv))


def _ok(sb: Sandbox, command: str, folders: tuple[str, ...] = (), lift: str | None = None) -> bool:
    return _run(sb, command, folders, lift).returncode == 0


# ---------- the folder scope (hands decision 4, A) ----------

def test_a_command_cant_change_the_owners_visible_folders_but_programs_keep_working(tmp_path: Path) -> None:
    home, ferb = _home(tmp_path)
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "profiles", listeners=lambda: set())
    docs = home / "Documents"
    for refused in (f"echo x > '{docs}/new.txt'", f": > '{docs}/report.txt'", f"echo more >> '{docs}/report.txt'",
                    f"rm '{docs}/report.txt'", f"mv '{docs}/report.txt' '{docs}/renamed.txt'",
                    f"mv '{docs}/report.txt' '{docs}/.hidden-copy'", f"mkdir '{docs}/sub'", f"rm -r '{docs}'",
                    f"echo x > '{home}/top.txt'",
                    f"echo x > '{home}/Library/Mobile Documents/com~apple~CloudDocs/a.txt'",
                    f"ln '{docs}/report.txt' '{home}/.cache/link'"):          # no new name for a visible file either
        r = _run(sb, refused)
        assert r.returncode != 0 and "Operation not permitted" in r.stderr, refused
    assert (docs / "report.txt").read_text() == "the owner's report" and sorted(p.name for p in docs.iterdir()) == \
        ["repo", "report.txt"]
    # git works in any repo (its .git is hidden), tools write hidden folders, ~/Library, temp and the workspace
    assert _ok(sb, f"cd '{docs}/repo' && git init -q . && git config user.email t@x && echo x > .git/probe")
    assert _ok(sb, f"echo c > '{home}/.cache/c' && echo l > '{home}/Library/Caches/l' && echo w > '{ferb}/workspace/w'")
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    assert _ok(sb, f"echo t > '{scratch}/t' && python3 -c 'print(1)'")


def test_the_tasks_folders_are_writable_and_only_those(tmp_path: Path) -> None:
    home, ferb = _home(tmp_path)
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "profiles", listeners=lambda: set())
    named = (str(home / "Projects"),)
    assert _ok(sb, f"echo x > '{home}/Projects/a.txt' && mkdir '{home}/Projects/sub' && rm '{home}/Projects/a.txt'",
               named)
    assert not _ok(sb, f"echo x > '{home}/Documents/a.txt'", named)
    with pytest.raises(ValueError, match="absolute"):
        sb.profile(folders=("Projects",))
    profile = sb.profile()
    assert '(deny file-write* (subpath "/Volumes"))' in profile                 # other disks: by the profile's text
    assert profile.index('(allow file-write* (regex #"/[.][^/]"))') < profile.index("(deny file-read* file-write*")


def test_the_scope_never_reopens_a_protected_place(tmp_path: Path) -> None:
    """The scope's allows come first: FERB's own state, the lists and the Trash still win after them."""
    home, ferb = _home(tmp_path)
    (home / ".ssh").mkdir()
    sb = Sandbox(Protection(home, ferb, [], ["~/.ssh"], []), tmp_path / "profiles", listeners=lambda: set())
    for refused in (f"echo x > '{ferb}/config.toml'", f"echo x > '{home}/.ssh/authorized_keys'",
                    f"echo x > '{home}/.Trash/x'"):
        assert not _ok(sb, refused, (str(home),)), refused                    # even with the whole home named


def test_a_refused_write_tells_the_worker_why_and_what_to_do(daemon: RunningDaemon, tmp_path: Path) -> None:
    home, ferb = _home(tmp_path)
    daemon.daemon.protection = Protection(home, daemon.daemon.home, [], [], [])
    daemon.daemon.shells.sandbox.protection = daemon.daemon.protection      # type: ignore[union-attr]
    g = task(daemon, "scope", Identity.OWNER, SHELL)
    r = call(daemon, g, "shell.run", command=f"echo x > '{home}/Documents/a.txt'")
    assert r["is_error"] and r["content"]["sandbox_note"] == SCOPE_REFUSED
    assert "file tools" in SCOPE_REFUSED and "which folder" in SCOPE_REFUSED
    assert "visible folders" in daemon.daemon.registry.capabilities["shell.run"].description
    # the task's folders reach the command's profile from the gateway (step 8 sets them from the planner)
    daemon.daemon.gateway.task_folders["scope"] = (str(home / "Documents"),)
    r = call(daemon, g, "shell.run", command=f"echo x > '{home}/Documents/a.txt'")
    assert not r["is_error"] and (home / "Documents" / "a.txt").read_text() == "x\n"


# ---------- the window server, signals ----------

def test_commands_cant_reach_the_window_server_and_still_run_git_and_python(tmp_path: Path) -> None:
    home, ferb = _home(tmp_path)
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "profiles", listeners=lambda: set())
    found = lookups(sb, list(WINDOW_SERVER))
    assert found and all(rc != 0 for rc in found.values()), found
    assert _ok(sb, f"cd '{ferb}/workspace' && git init -q r && cd r && git status --short && python3 -c 'print(2)'")
    assert "windowserver" not in sb.profile(apps=True)                       # measured for the shell only


def test_a_command_signals_only_its_own_processes(tmp_path: Path) -> None:
    home, ferb = _home(tmp_path)
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "profiles", listeners=lambda: set())
    outside = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], start_new_session=True)
    try:
        r = _run(sb, f"kill {outside.pid}; echo outside=$?; sleep 30 & k=$!; kill $k; echo own=$?")
        assert "outside=1" in r.stdout and "own=0" in r.stdout and "Operation not permitted" in r.stderr
        assert outside.poll() is None                                          # still alive
    finally:
        outside.kill()
        outside.wait()


# ---------- lifts (item 4) ----------

def test_a_write_lift_never_reopens_a_never_read_store_inside_it(tmp_path: Path) -> None:
    """Lifting write:/Library must not reopen /Library/Keychains: the store's write deny comes after the lift."""
    home, ferb = _home(tmp_path)
    (home / "Lib" / "Keychains").mkdir(parents=True)
    sb = Sandbox(Protection(home, ferb, ["~/Lib/Keychains"], ["~/Lib"], []), tmp_path / "profiles",
                 listeners=lambda: set())
    assert _ok(sb, f"echo x > '{home}/Lib/other'", lift="write:~/Lib")
    assert not _ok(sb, f"echo x > '{home}/Lib/Keychains/login'", lift="write:~/Lib")
    profile = sb.profile("write:~/Lib")
    assert profile.rindex(f'(deny file-write* (subpath "{home}/Lib/Keychains"))') > \
        profile.index(f'(allow file-write* file-read-metadata (subpath "{home}/Lib"))')


def test_ferbs_own_service_definition_is_never_lifted(tmp_path: Path) -> None:
    home, ferb = _home(tmp_path)
    agents = home / "Library" / "LaunchAgents"
    agents.mkdir(parents=True)
    (agents / "com.ferb.daemon.plist").write_text("<plist/>")
    sb = Sandbox(Protection(home, ferb, [], ["~/Library/LaunchAgents"], []), tmp_path / "profiles",
                 listeners=lambda: set())
    lift = "write:~/Library/LaunchAgents"
    assert _ok(sb, f"echo x > '{agents}/com.owner.thing.plist'", lift=lift)
    for refused in (f"echo x > '{agents}/com.ferb.daemon.plist'", f"rm '{agents}/com.ferb.daemon.plist'",
                    f"echo x > '{agents}/com.ferb.FERB.login.plist'", f"mv '{agents}' '{home}/Library/away'"):
        assert not _ok(sb, refused, lift=lift), refused
    assert (agents / "com.ferb.daemon.plist").read_text() == "<plist/>"
    for rule in ("ferb_service", "ferb_app"):
        with pytest.raises(ValueError, match="never lifted"):
            sb.profile(rule)


def test_input_into_a_lifted_command_is_refused(tmp_path: Path) -> None:
    """Item 4 (chosen: refuse): the owner's Yes covered that exact command, not what is typed into it later."""
    from ferb.act.shell.manager import ProcInfo

    class Pty:
        exit_code = None
        def line_limit(self) -> None: return None
        def write(self, data: bytes) -> int: raise AssertionError("nothing may be typed")
    manager = ShellManager(tmp_path, lambda text: text)
    manager._procs["p-1"] = (Pty(), ProcInfo("p-1", "t", "bash", str(tmp_path), 0, True, None, False,  # type: ignore[assignment]
                                             lifted="read:~/.ssh"))
    with pytest.raises(Refused, match="lift"):
        asyncio.run(manager.send(ProcHandle("p-1", "t"), "cat ~/.ssh/id_ed25519\n"))


def test_a_read_escape_yes_covers_the_task_and_the_file_tools(daemon: RunningDaemon, tmp_path: Path) -> None:
    """Hands decision 1: "Let FERB read ~/.aws for this task?" once; later shell read escapes of that folder and file
    tool reads of it in the same task run without asking. Writes still ask per command."""
    from tests.conftest import Answers, gateway_request, gateway_setup, gateway_tool_call
    from types import SimpleNamespace
    fake = tmp_path / "fake"
    (fake / ".aws").mkdir(parents=True)
    (fake / ".aws" / "config").write_text("region = eu-west-1\n")
    daemon.daemon.gateway.roots.protection = Protection(fake, daemon.daemon.home, ["~/.aws"], [], [])
    grant, _ = gateway_setup(daemon, {"shell", "fs:read"}, gateway_request("check my aws region"), [])
    rec = daemon.daemon.tasks.get(grant.task_id)
    yes = Answers(daemon, choice=1)
    try:
        first = daemon.run(daemon.daemon.gateway._escape(grant, rec, SimpleNamespace(
            escape="read:~/.aws", command="cat config", cwd=str(tmp_path)), "look at the region"))
        second = daemon.run(daemon.daemon.gateway._escape(grant, rec, SimpleNamespace(
            escape="read:~/.aws", command="ls -la", cwd=str(tmp_path)), "list it"))
        read = gateway_tool_call(daemon, grant, "fs.read", path=str(fake / ".aws" / "config"))
    finally:
        yes.stop()
    assert len(yes.asked) == 1 and "for this task" in yes.asked[0].summary
    assert first.command == "cat config" and second.command == "ls -la"
    assert first.confirmation_id == second.confirmation_id
    assert not read["is_error"] and "eu-west-1" in read["content"]["text"]


# ---------- shell fixes (item 6) ----------

def test_the_whole_process_group_dies_at_task_end_even_after_the_command_exited(tmp_path: Path) -> None:
    async def go() -> tuple[int, list[int], bool]:
        manager = ShellManager(tmp_path / "logs", lambda text: text)
        h = await manager.start("nohup sleep 30 >/dev/null 2>&1 & sleep 0.5; echo started", tmp_path, "task-a")
        p = manager._procs[h.id][0]
        assert await manager.wait(h, 5) == 0                                  # its own process ended at once
        assert p.proc is not None
        pgid = p.proc.pid
        alive = group_members(pgid)
        n = await manager.end_task("task-a")
        return n, alive, bool(group_members(pgid)) or h.id in manager._procs
    n, alive, left = asyncio.run(go())
    assert alive and n == 0 and not left                                      # the child died; the entry went


def test_a_group_whose_id_could_be_reused_is_never_signalled_without_a_check(tmp_path: Path) -> None:
    async def go() -> bool:
        p = PtyProc(["/bin/sh", "-c", "nohup sleep 30 >/dev/null 2>&1 & sleep 0.5; exit 0"], tmp_path, {"PATH": "/usr/bin:/bin"},
                    tmp_path / "l.log", str)
        await p.start()
        await p.wait(5)
        assert p.proc is not None
        await p.kill(lambda members: False)                                    # members not confirmed: nothing sent
        still = bool(group_members(p.proc.pid))
        await p.kill(lambda members: True)
        return still and not group_members(p.proc.pid)
    assert asyncio.run(go())


def test_the_daemon_kills_a_sandboxed_commands_leftover_children(daemon: RunningDaemon) -> None:
    shells = daemon.daemon.shells
    assert shells is not None
    h = daemon.run(shells.start("nohup sleep 30 >/dev/null 2>&1 & sleep 0.5; exit 0", daemon.daemon.home / "workspace",
                                "t-left"))
    p = shells._procs[h.id][0]
    assert daemon.run(shells.wait(h, 5)) == 0 and p.proc is not None
    pgid = p.proc.pid
    assert group_members(pgid)                                                 # the child lives on in the group
    daemon.run(shells.end_task("t-left"))
    deadline = time.monotonic() + 5
    while group_members(pgid):
        assert time.monotonic() < deadline, "the contained child survived its task"
        time.sleep(0.05)
    assert h.id not in shells._procs


def test_a_line_too_long_for_a_line_reading_command_is_refused_not_dropped(tmp_path: Path) -> None:
    async def go() -> tuple[str, tuple[int, int], str]:
        manager = ShellManager(tmp_path / "logs", lambda text: text)
        h = await manager.start("IFS= read -r line; printf 'got %s\\n' \"${#line}\"", tmp_path, "t")
        await asyncio.sleep(0.2)
        try:
            await manager.send(h, "a" * 3001 + "\n")
            refused = ""
        except Refused as e:
            refused = str(e)
        sent = await manager.send(h, "b" * 1023 + "\n")                       # the longest line it takes
        await manager.wait(h, 5)
        text, _, _ = await manager.read(h)
        return refused, sent, text
    refused, sent, text = asyncio.run(go())
    assert "1023 bytes" in refused and "Nothing was sent" in refused
    assert sent == (1024, 1024) and "got 1023" in text


def test_a_send_that_doesnt_all_go_in_is_reported(tmp_path: Path) -> None:
    async def go() -> tuple[int, int]:
        manager = ShellManager(tmp_path / "logs", lambda text: text)
        h = await manager.start("exit 0", tmp_path, "t")
        await manager.wait(h, 5)
        return await manager.send(h, "too late\n", wait_s=0.1)
    assert asyncio.run(go()) == (0, 9)


def test_shell_send_says_when_input_didnt_go_in(daemon: RunningDaemon) -> None:
    g = task(daemon, "send", Identity.OWNER, SHELL)
    pid = call(daemon, g, "shell.run", command="true")["content"]["id"]
    r = call(daemon, g, "shell.send", id=pid, text="hello\n")
    assert r["is_error"] and r["content"]["sent"] is False and r["content"]["bytes_sent"] == 0


def test_shell_kill_reports_its_true_kind() -> None:
    spec = ShellKill.__new__(ShellKill).describe(ShellKill.Input(id="p-1"))
    assert spec.kind == Kind.STOP_PROCESS and Kind.STOP_PROCESS not in REVERSIBLE


def test_finished_processes_are_pruned_once_their_task_ends(daemon: RunningDaemon) -> None:
    g = task(daemon, "prune", Identity.OWNER, SHELL)
    pids = [call(daemon, g, "shell.run", command=f"echo {n}")["content"]["id"] for n in range(3)]
    shells = daemon.daemon.shells
    assert shells is not None and all(p in shells._procs for p in pids)
    daemon.run(daemon.daemon._task_end("prune"))
    assert not any(p in shells._procs for p in pids)


def test_each_host_is_journaled_as_the_command_first_reaches_it() -> None:
    """Durable host records: the proxy tells `on_destination` of each process's first connection to a host as it
    opens (the daemon journals it), not only in the final record at forget/close."""
    from ferb.act.shell.proxy import EgressProxy
    seen: list[tuple[str, str, int]] = []
    proxy = EgressProxy(lambda proc, host, port: (True, ""), on_destination=lambda *a: seen.append(a),
                        excluded=lambda host: host == "hidden.example")
    try:
        proxy.credentials("p-1")
        assert proxy._opened("p-1", "example.com", 443) and proxy._opened("p-1", "example.com", 443)
        assert proxy._opened("p-1", "hidden.example", 443)
        assert seen == [("p-1", "example.com", 443), ("p-1", "excluded", 443)]
        assert proxy.pending("p-1")
    finally:
        proxy.close()


def test_the_daemon_journals_a_shell_destination_when_it_happens(daemon: RunningDaemon) -> None:
    daemon.daemon._shell_destination("p-x", "example.com", 443)

    async def rows() -> list[Any]:
        await asyncio.sleep(0.05)
        return daemon.daemon.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind = 'shell.destination'").fetchall()
    found = daemon.run(rows())
    assert found and "example.com" in found[-1][0]


def test_shell_logs_go_after_the_evidence_window(tmp_path: Path) -> None:
    logs = tmp_path / "logs"
    logs.mkdir()
    old, new = logs / "p-old.log", logs / "p-new.log"
    old.write_text("old output")
    new.write_text("new output")
    week = time.time() - 8 * 86400
    os.utime(old, (week, week))
    manager = ShellManager(logs, lambda text: text)
    assert manager.purge_logs(time.time() - 7 * 86400) == 1
    assert not old.exists() and new.exists()


def test_the_retention_pass_purges_shell_logs(daemon: RunningDaemon) -> None:
    shells = daemon.daemon.shells
    assert shells is not None
    shells.log_dir.mkdir(parents=True, exist_ok=True)
    stale = shells.log_dir / "p-stale.log"
    stale.write_text("x")
    when = (daemon.daemon.clock.now().timestamp()) - 8 * 86400
    os.utime(stale, (when, when))

    async def purge() -> dict[str, int]:
        return daemon.daemon._purge_records()
    assert daemon.run(purge())["shell_logs"] == 1 and not stale.exists()


# ---------- the facts shape (item 1) ----------

def test_action_facts_are_optional_typed_and_recorded_without_empty_fields() -> None:
    assert ActionSpec(Kind.CLICK).facts is None
    facts = ActionFacts(submits_form=True, form_action_host="pay.example.com", form_method="POST",
                        field_kinds=("card_number", "card_csc"), page_amount="₹137.39",
                        received_by=ReceivedBy(bundle_id="com.google.Chrome", role="button"), secure_field=False)
    assert facts.recorded() == {"submits_form": True, "form_action_host": "pay.example.com", "form_method": "POST",
                                "field_kinds": ["card_number", "card_csc"], "page_amount": "₹137.39",
                                "received_by": {"bundle_id": "com.google.Chrome", "role": "button"},
                                "secure_field": False}
    assert ActionFacts().recorded() == {}


def test_the_gateway_records_a_tools_facts_untouched(daemon: RunningDaemon, tmp_path: Path) -> None:
    from tests.conftest import gateway_setup, gateway_tool_call
    f = daemon.daemon.home / "workspace" / "facts.txt"
    grant, _ = gateway_setup(daemon, {"fs:write"}, None, [])
    assert not gateway_tool_call(daemon, grant, "fs.write", path=str(f), content="x")["is_error"]

    async def recorded() -> Any:
        import json
        row = daemon.daemon.db.conn.execute("SELECT input_json FROM spans WHERE kind = 'tool.call' AND "
                                            "json_extract(input_json, '$.tool') = 'fs.write'").fetchone()
        return json.loads(row[0])
    assert daemon.run(recorded())["facts"] == {"writes": [str(f)]}
