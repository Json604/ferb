"""FERB's shell runs under a macOS sandbox built from the protection lists (S2, SQ3 revised, SQ5, D27). It cannot
read the sensitive stores or FERB's own state (run tokens, the database, recovery copies, config, upload staging),
cannot write FERB's own state or the never-write places, and cannot reach the daemon socket. Offline: the real
sandbox-exec on throwaway paths inside the test home; never the owner's stores. The only off-Mac address used is
192.0.2.1 (RFC 5737, never routed), which the sandbox refuses before a packet leaves."""
import asyncio
import os
import shutil
import socket
import subprocess
import sys
import threading
from pathlib import Path

import pytest

from ferb.act.shell.manager import ShellManager
from ferb.act.shell.sandbox import APP_SERVICES, CERT_BUNDLE, NETWORK_SERVICES, SANDBOX_EXEC, SECURITY_SERVER, TRUST_SERVICES, Sandbox
from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.core.types import Identity
from ferb.policy.protection import Protection
from tests.conftest import Answers, RunningDaemon
from tests.mackit import gateway_call as call, grant_task as task
from tests.support.sandbox_probes import SECRET, SERVICE, lookups, run_under, throwaway_keychain

pytestmark = pytest.mark.skipif(not shutil.which("sandbox-exec"), reason="sandbox-exec is not available here")
SHELL = {"shell", "fs:read"}


def _state(d: RunningDaemon) -> Path:
    home = d.daemon.home
    (home / "run").mkdir(exist_ok=True)
    (home / "run" / "token-abc").write_text("secret-token")
    (home / "recovery").mkdir(exist_ok=True)
    (home / "recovery" / "copy.txt").write_text("old")
    (home / "workspace" / "uploads" / "t" / "r").mkdir(parents=True, exist_ok=True)
    (home / "workspace" / "uploads" / "t" / "r" / "cv.pdf").write_text("staged")
    (home / "workspace" / "ok.txt").write_text("fine")
    return home


def _fake_owner(d: RunningDaemon, tmp_path: Path, never_read: list[str], never_write: list[str]) -> Path:
    """The daemon's Protection rebuilt over a fake owner home inside tmp_path, so nothing touches the real one."""
    fake = tmp_path / "home"
    fake.mkdir(exist_ok=True)
    d.daemon.protection = Protection(fake, d.daemon.home, never_read, never_write, [])
    d.daemon.shells.sandbox.protection = d.daemon.protection          # type: ignore[union-attr]
    return fake


def test_the_shell_cannot_reach_ferbs_state_tokens_or_socket(daemon: RunningDaemon) -> None:
    home = _state(daemon)
    g = task(daemon, "s", Identity.OWNER, SHELL)
    for path in ("run/token-abc", "ferb.db", "recovery/copy.txt", "config.toml", "workspace/uploads/t/r/cv.pdf"):
        r = call(daemon, g, "shell.run", command=f"cat {home / path}")
        assert r["is_error"] and "secret-token" not in str(r) and "staged" not in str(r), path
    for path in ("ferb.db", "config.toml", "run/x", "recovery/new", "workspace/uploads/t/r/cv.pdf"):
        r = call(daemon, g, "shell.run", command=f"echo pwned > {home / path}; cat {home / path}")
        assert r["is_error"] and "pwned" not in str(r), path
    assert (home / "workspace" / "uploads" / "t" / "r" / "cv.pdf").read_text() == "staged"
    r = call(daemon, g, "shell.run", command=f"cat {home / 'workspace' / 'ok.txt'}; echo more >> {home / 'workspace' / 'ok.txt'}")
    assert not r["is_error"] and "fine" in r["content"]["output"]
    sock = daemon.sock
    r = call(daemon, g, "shell.run", command=f"python3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.connect(\"{sock}\")'")
    assert r["is_error"] and "Operation not permitted" in r["content"]["output"]
    r = call(daemon, g, "shell.run", command=f"nc -U {sock} < /dev/null")
    assert r["is_error"]


def test_another_spelling_of_ferbs_home_is_the_same_place(daemon: RunningDaemon) -> None:
    """APFS ignores case: the sandbox matched every case spelling of a protected path (measured), so must the test."""
    home = _state(daemon)
    upper = home.parent / home.name.upper()
    if not (upper / "run" / "token-abc").exists():
        pytest.skip("case-sensitive volume")
    g = task(daemon, "c", Identity.OWNER, SHELL)
    r = call(daemon, g, "shell.run", command=f"cat {upper}/RUN/TOKEN-ABC; cat {upper}/run/token-abc")
    assert r["is_error"] and "secret-token" not in str(r)


def test_the_shell_cannot_read_the_sensitive_stores_or_write_the_protected_places(daemon: RunningDaemon, tmp_path: Path) -> None:
    fake = _fake_owner(daemon, tmp_path, ["~/Library/Keychains"], ["~/.ssh", "~/.zshrc"])
    (fake / "Library" / "Keychains").mkdir(parents=True)
    (fake / "Library" / "Keychains" / "login.keychain-db").write_text("keys")
    (fake / ".ssh").mkdir()
    (fake / ".zshrc").write_text("export X=1")
    g = task(daemon, "p", Identity.OWNER, SHELL)
    r = call(daemon, g, "shell.run", command=f"cat {fake}/Library/Keychains/login.keychain-db")
    assert r["is_error"] and "keys" not in str(r)
    r = call(daemon, g, "shell.run", command=f"ls {fake}/Library/Keychains")
    assert r["is_error"]
    for write in (f"echo y > {fake}/.ssh/id_test", f"echo z >> {fake}/.zshrc"):
        assert call(daemon, g, "shell.run", command=write)["is_error"], write
    assert (fake / ".zshrc").read_text() == "export X=1" and not (fake / ".ssh" / "id_test").exists()
    assert not call(daemon, g, "shell.run", command=f"cat {fake}/.zshrc")["is_error"]      # readable, not writable


def test_renaming_a_folder_above_a_protected_path_or_the_workspace_is_refused(daemon: RunningDaemon, tmp_path: Path) -> None:
    """Moving ~/.config away would carry ~/.config/gh to a path no rule names; moving the workspace away and planting
    a link there would point the workspace exception at FERB's home (Task 1 review). Files can still be made inside."""
    fake = _fake_owner(daemon, tmp_path, ["~/.config/gh"], [])
    (fake / ".config" / "gh").mkdir(parents=True)
    (fake / ".config" / "gh" / "hosts.yml").write_text("gh-token")
    ws = daemon.daemon.home / "workspace"
    g = task(daemon, "m", Identity.OWNER, SHELL)
    r = call(daemon, g, "shell.run", command=f"mv {fake}/.config {fake}/moved; cat {fake}/moved/gh/hosts.yml")
    assert r["is_error"] and "gh-token" not in str(r) and (fake / ".config" / "gh" / "hosts.yml").exists()
    r = call(daemon, g, "shell.run", command=f"mv {fake} {tmp_path}/away")
    assert r["is_error"] and fake.is_dir()
    r = call(daemon, g, "shell.run", command=f"mv {ws} {tmp_path}/ws-away")         # a link could then take its place
    assert r["is_error"] and ws.is_dir() and not ws.is_symlink() and not (tmp_path / "ws-away").exists()
    r = call(daemon, g, "shell.run", command=f"echo ok > {fake}/.config/other.txt; mkdir {ws}/sub && echo in > {ws}/sub/f")
    assert not r["is_error"] and (fake / ".config" / "other.txt").read_text() == "ok\n"


def test_no_new_name_can_be_made_for_a_protected_file(daemon: RunningDaemon, tmp_path: Path) -> None:
    """A hardlink or clone of a denied file would be a new path no rule names: Seatbelt refuses making one."""
    home = _state(daemon)
    fake = _fake_owner(daemon, tmp_path, ["~/.aws"], ["~/.zshrc"])
    (fake / ".aws").mkdir()
    (fake / ".aws" / "credentials").write_text("aws-key")
    (fake / ".zshrc").write_text("export X=1")
    out = tmp_path / "out"
    out.mkdir()
    g = task(daemon, "h", Identity.OWNER, SHELL)
    for src in (fake / ".aws" / "credentials", fake / ".zshrc", home / "run" / "token-abc"):
        r = call(daemon, g, "shell.run", command=f"ln {src} {out}/h")
        assert r["is_error"] and not (out / "h").exists(), src
    for src in (fake / ".aws" / "credentials", home / "run" / "token-abc"):         # unreadable: no clone either
        r = call(daemon, g, "shell.run", command=f"cp -c {src} {out}/c; cat {out}/c")
        assert r["is_error"] and not (out / "c").exists() and "aws-key" not in str(r) and "secret-token" not in str(r)


@pytest.mark.real
def test_the_keychain_service_is_out_of_reach_without_a_prompt(daemon: RunningDaemon, tmp_path: Path) -> None:
    # A throwaway keychain file with a throwaway item. `security create-keychain` adds it to the user's search list;
    # the helper restores that list even if the test fails. `security` must fail at the service at once, never show a
    # prompt and never print the secret.
    with throwaway_keychain(tmp_path) as kc:
        g = task(daemon, "k", Identity.OWNER, SHELL)
        r = call(daemon, g, "shell.run", command=f"security find-generic-password -s {SERVICE} -w {kc} 2>&1; echo rc=$?")
        assert "rc=0" not in r["content"]["output"] and SECRET not in str(r)


def test_direct_network_is_denied_inside_the_sandbox(daemon: RunningDaemon) -> None:
    """Refused at the socket ("Operation not permitted", curl's 7), not timed out: nothing left the Mac."""
    g = task(daemon, "n", Identity.OWNER, SHELL)
    for cmd in ("python3 -c 'import socket; socket.create_connection((\"192.0.2.1\", 443), 2)'",
                "python3 -c 'import socket; s=socket.socket(socket.AF_INET, socket.SOCK_DGRAM); "
                "s.sendto(b\"x\", (\"192.0.2.1\", 53))'",
                "nc -v -z -w 2 192.0.2.1 443"):
        r = call(daemon, g, "shell.run", command=cmd + " 2>&1; echo rc=$?")
        assert "rc=0" not in r["content"]["output"] and "Operation not permitted" in r["content"]["output"], cmd
    # Direct, ignoring the proxy variables (through FERB's proxy is test_egress_proxy's)
    r = call(daemon, g, "shell.run", command="curl -sS --noproxy '*' --max-time 3 http://192.0.2.1/ 2>&1; echo rc=$?")
    assert "rc=7" in r["content"]["output"]
    r = call(daemon, g, "shell.run", command="dig +short +time=1 +tries=1 @192.0.2.1 example.invalid; echo rc=$?")
    assert "rc=0" not in r["content"]["output"]
    srv = socket.socket(); srv.bind(("127.0.0.1", 0)); srv.listen(1)
    port = srv.getsockname()[1]
    threading.Thread(target=lambda: srv.accept(), daemon=True).start()
    r = call(daemon, g, "shell.run", command=f"python3 -c 'import socket; socket.create_connection((\"127.0.0.1\", {port}), 2)' 2>&1; echo rc=$?")
    assert "rc=0" not in r["content"]["output"] and "Operation not permitted" in r["content"]["output"]   # not FERB's
    r = call(daemon, g, "shell.run", command="python3 -c 'import socket; s=socket.socket(); s.bind((\"127.0.0.1\", 0)); "
                                             "s.listen(1); socket.create_connection(s.getsockname(), 2); print(\"reached\")'")
    assert not r["is_error"] and "reached" in r["content"]["output"]                   # a dev server of its own: yes


def test_the_shell_names_apples_certificate_bundle(daemon: RunningDaemon) -> None:
    """The Keychain service is denied, so tools that list root certificates through it find none; they read the
    bundle instead (Codex CLI's TLS failed without it, worked with it: Task 2 report)."""
    if not os.path.isfile(CERT_BUNDLE):
        pytest.skip("no certificate bundle on this Mac")
    g = task(daemon, "e", Identity.OWNER, SHELL)
    r = call(daemon, g, "shell.run", command="echo cert=$SSL_CERT_FILE; grep -c 'BEGIN CERTIFICATE' \"$SSL_CERT_FILE\"")
    lines = r["content"]["output"].split()
    assert f"cert={CERT_BUNDLE}" in lines and int(lines[-1]) > 50


def test_shell_rules_lists_the_boundaries_and_what_is_liftable(daemon: RunningDaemon) -> None:
    g = task(daemon, "r", Identity.OWNER, SHELL)
    r = call(daemon, g, "shell.rules")
    ids = {x["id"]: x for x in r["content"]["rules"]}
    assert ids["ferb_state"]["liftable"] is False and ids["network"]["liftable"] is False and ids["admin"]["liftable"]
    assert r["footer"].endswith("Origin: ferb")
    assert "danger-full-access" in r["content"]["own_sandbox"]
    assert "can't open apps" in r["content"]["apps"]
    assert "escape" not in r["content"]                                       # Task 6 adds it
    # Derived from proxy_port(): the proxy runs (F5a Task 3), so the line names it and what it records
    port = daemon.daemon.proxy.port                                           # type: ignore[union-attr]
    assert r["content"]["network"].startswith("any remote host, without asking, only through FERB's proxy")
    assert f"only through FERB's proxy on 127.0.0.1:{port}, which records" in r["content"]["network"]
    daemon.daemon.sandbox.proxy_port = lambda: None                          # type: ignore[union-attr]
    r = call(daemon, g, "shell.rules")
    assert r["content"]["network"].startswith("no network beyond this Mac; FERB's proxy is not running")


def test_shell_tools_say_a_programs_own_sandbox_cant_start_inside(daemon: RunningDaemon) -> None:
    """A fact for the agents (E5), never a flag FERB adds to another agent's command."""
    caps = daemon.daemon.registry.capabilities
    for name in ("shell.run", "shell.start"):
        assert "can't start its own macOS sandbox" in caps[name].description, name
    g = task(daemon, "f", Identity.OWNER, SHELL)
    r = call(daemon, g, "shell.run", command="sandbox-exec -p '(version 1)(allow default)' /bin/echo nested 2>&1")
    assert r["is_error"] and "Operation not permitted" in r["content"]["output"]


def test_every_shell_command_is_sandboxed_and_the_record_says_so(daemon: RunningDaemon) -> None:
    g = task(daemon, "m", Identity.OWNER, SHELL)
    pid = call(daemon, g, "shell.run", command="echo hi")["content"]["id"]
    info = next(p for p in daemon.daemon.shells.list() if p.id == pid)                 # type: ignore[union-attr]
    assert info.sandboxed is True and info.lifted is None and info.command == "echo hi"


def test_a_profile_file_lives_until_its_process_ends(daemon: RunningDaemon) -> None:
    """sandbox-exec reads the profile after it starts, so removing it at start failed every run (measured)."""
    shells, sb = daemon.daemon.shells, daemon.daemon.sandbox
    assert shells is not None and sb is not None
    h = daemon.run(shells.start("sleep 30", daemon.daemon.home / "workspace", "t-life"))
    try:
        assert len(list(sb.profiles_dir.glob("*.sb"))) == 1
    finally:
        daemon.run(shells.kill(h))
    daemon.run(shells.wait(h, 5))
    assert not list(sb.profiles_dir.glob("*.sb"))


def test_a_lifted_rule_leaves_every_other_rule_and_never_lifts_ferbs_own(tmp_path: Path) -> None:
    home, ferb = tmp_path / "h", tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    sb = Sandbox(Protection(home, ferb, ["~/.aws", "~/.ssh"], ["~/.ssh"], []), tmp_path / "p")
    lifted = sb.profile("read:~/.aws")
    assert f'(deny file-read* (subpath "{home}/.aws"))' not in lifted
    assert f'(deny file-write* (subpath "{home}/.aws"))' in lifted
    assert f'(deny file-read* (subpath "{home}/.ssh"))' in lifted
    assert f'(deny file-write* (subpath "{home}/.ssh"))' in sb.profile("read:~/.ssh")    # a lifted read may not write
    for rule in ("ferb_state", "network", "admin", "read:~/nothing"):
        with pytest.raises(ValueError):
            sb.profile(rule)


def test_a_link_where_the_workspace_was_opens_nothing(tmp_path: Path) -> None:
    """The workspace allow names the literal folder only: an allow never widens to where a link points."""
    home, ferb = tmp_path / "h", tmp_path / "h" / ".ferb"
    ferb.mkdir(parents=True)
    (ferb / "secret").write_text("ferb-state")
    (ferb / "workspace").symlink_to(ferb)
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "p")
    argv = sb.argv(f"cat {ferb}/workspace/secret")
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=10)
    finally:
        sb.cleanup(sb.profile_path(argv))
    assert r.returncode != 0 and "ferb-state" not in r.stdout


def test_applescript_runs_under_the_same_sandbox(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """An explicitly approved trustd lift leaves osascript's file containment intact; no app is addressed.
    mac.applescript independently hands every script to the default sandbox and removes its profile afterwards.
    """
    from ferb.act.mac import applescript
    home, ferb = tmp_path / "h", tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    (ferb / "token").write_text("secret-token")
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "p")
    # This Mac needs trustd to load Standard Additions. Without the exact-command lift, osascript fails at
    # parsing `do shell script`; that is not evidence that the protected file read reached its sandbox deny.
    argv = sb.wrap(["/usr/bin/osascript", "-e", f'do shell script "cat {ferb}/token"'], "trustd")
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=20)
    finally:
        sb.cleanup(sb.profile_path(argv))
    assert r.returncode != 0 and "secret-token" not in r.stdout and "Operation not permitted" in r.stderr
    seen: list[list[str]] = []
    captured: dict[str, object] = {}
    real_exec = asyncio.create_subprocess_exec

    async def fake_exec(*a: str, **k: object) -> object:
        seen.append(list(a))
        assert Path(a[2]).exists()                                   # the profile is there while it runs
        captured["apps"] = "(deny lsopen)" not in Path(a[2]).read_text()   # the apps profile keeps lsopen
        captured["cwd"] = k.get("cwd")
        captured["env"] = k.get("env")
        return await real_exec(sys.executable, "-c", "print('ok')", stdout=-1, stderr=-1)
    monkeypatch.setattr(applescript.asyncio, "create_subprocess_exec", fake_exec)
    ok, out = asyncio.run(applescript.run("com.apple.TextEdit", "get name", sandbox=sb, cwd=str(ferb / "workspace")))
    assert ok and seen[0][:2] == [SANDBOX_EXEC, "-f"] and seen[0][3] == "/usr/bin/osascript"
    assert captured["apps"] is True                                           # applescript.run passes apps=True
    assert captured["cwd"] == str(ferb / "workspace")
    assert captured["env"] is not None and "FERB_RUN_TOKEN" not in captured["env"]   # the shell's cleaned env
    assert not list((tmp_path / "p").glob("*.sb"))


def test_the_apps_profile_differs_only_by_the_app_rules_and_keeps_every_other_deny(tmp_path: Path) -> None:
    """mac.applescript's profile keeps Apple Events and app launches; the shell's denies them. Nothing else differs."""
    home, ferb = tmp_path / "h", tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "p", listeners=lambda: {9999})   # fixed: comparable
    shell_only = set(sb.profile().splitlines()) - set(sb.profile(apps=True).splitlines())
    assert shell_only == {"(deny lsopen)", *(f'(deny mach-lookup (global-name "{n}"))' for n in APP_SERVICES)}
    assert not set(sb.profile(apps=True).splitlines()) - set(sb.profile().splitlines())   # apps adds nothing
    assert "(deny job-creation)" in sb.profile(apps=True)                     # still no background jobs


def test_osascript_can_use_the_approved_exact_command_trustd_escape(daemon: RunningDaemon) -> None:
    """The shell interface can load Standard Additions with one approval, without addressing an app."""
    grant = task(daemon, "osascript-escape", Identity.OWNER, SHELL)
    command = '/usr/bin/osascript -e \'do shell script "printf ferb-contained-check"\''
    answers = Answers(daemon, granted=True, choice=1)
    try:
        result = call(daemon, grant, "shell.run", command=command, escape="trustd")
    finally:
        answers.stop()
    assert not result["is_error"] and "ferb-contained-check" in result["content"]["output"]
    assert len(answers.asked) == 1
    info = next(p for p in daemon.daemon.shells.list() if p.id == result["content"]["id"])
    assert info.command == command and info.lifted == "trustd" and info.sandboxed


@pytest.mark.real
def test_the_apps_profile_still_blocks_files_network_and_the_keychain(daemon: RunningDaemon, tmp_path: Path) -> None:
    """The apps profile keeps Apple Events, but every file, network and Keychain deny still holds (fix round 2)."""
    home = tmp_path / "home"
    (home / ".ssh").mkdir(parents=True)
    (home / ".zshrc").write_text("export X=1")
    ferb = daemon.daemon.home
    (ferb / "run").mkdir(exist_ok=True)
    (ferb / "run" / "token-x").write_text("secret-token")
    (ferb / "workspace" / "uploads" / "t").mkdir(parents=True, exist_ok=True)
    (ferb / "workspace" / "uploads" / "t" / "cv.pdf").write_text("staged")
    sb = Sandbox(Protection(home, ferb, ["~/.ssh"], ["~/.zshrc"], []), tmp_path / "p", socket=daemon.sock)
    def out(cmd: str) -> subprocess.CompletedProcess[str]:
        return run_under(sb, ["/bin/sh", "-c", cmd], apps=True)
    assert out(f"cat {ferb}/run/token-x").returncode != 0
    assert "secret-token" not in out(f"cat {ferb}/run/token-x").stdout
    assert out(f"cat {ferb}/workspace/uploads/t/cv.pdf").returncode != 0 and "staged" not in out(f"cat {ferb}/workspace/uploads/t/cv.pdf").stdout
    assert out(f"cat {home}/.ssh/config 2>&1").returncode != 0
    assert out(f"echo z >> {home}/.zshrc").returncode != 0 and (home / ".zshrc").read_text() == "export X=1"
    assert out("curl -sS --max-time 3 http://192.0.2.1/ >/dev/null 2>&1; echo $?").stdout.strip() == "7"
    with throwaway_keychain(tmp_path) as kc:
        r = out(f"security find-generic-password -s {SERVICE} -w {kc} 2>&1")
        assert r.returncode != 0 and SECRET not in r.stdout
    assert not out(f"cat {ferb}/workspace/ok.txt 2>/dev/null; echo ws-ok").returncode   # the workspace stays usable


def test_each_denied_mach_service_fails_to_look_up(tmp_path: Path) -> None:
    """A behavioural pin for every mach deny: bootstrap_look_up fails under the shell profile (1100 = not permitted),
    and the app services succeed under the apps profile. No Automation grant and no running app are needed."""
    home, ferb = tmp_path / "h", tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "p")
    appleevents = "com.apple.coreservices.appleevents"
    assert appleevents in APP_SERVICES                           # a raw send needs this service; the shell denies it
    denied = [SECURITY_SERVER, "com.apple.coreservices.launchservicesd", "com.apple.lsd.mapdb", appleevents]
    shell = lookups(sb, [*denied, *NETWORK_SERVICES, *TRUST_SERVICES])
    assert all(shell[name] != 0 for name in [*denied, *NETWORK_SERVICES, *TRUST_SERVICES]), shell
    apps = lookups(sb, [*denied, *NETWORK_SERVICES, *TRUST_SERVICES], apps=True)
    assert apps[SECURITY_SERVER] != 0                            # the Keychain service stays denied
    assert all(apps[name] == 0 for name in denied if name != SECURITY_SERVER)   # app services back for AppleScript
    # No network around FERB's proxy for either profile: background URL sessions (nsurlsessiond reached a server the
    # sandbox denies: F5a Task 3) and mDNSResponder's resolver service (DNS for any name)
    assert "com.apple.nsurlsessiond" in NETWORK_SERVICES and "com.apple.dnssd.service" in NETWORK_SERVICES
    assert all(apps[name] != 0 for name in NETWORK_SERVICES), apps
    assert all(apps[name] != 0 for name in TRUST_SERVICES), apps


def test_without_a_working_sandbox_the_shell_and_applescript_are_left_out(home: Path,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    """Q7: never uncontained. The tools are not registered, the journal says why, and `ferb doctor` shows it."""
    from ferb.cli.setup import containment_check
    monkeypatch.setattr(Sandbox, "available", lambda self: (False, f"{SANDBOX_EXEC} is missing"))
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    try:
        names = set(d.daemon.registry.capabilities)
        assert not {n for n in names if n.startswith("shell.")} and "mac.applescript" not in names
        assert "mac.act" in names
        async def why() -> list[str]:
            return [e.data["why"] for e in d.daemon.journal.of_kind("shell.unavailable")]   # type: ignore[union-attr]
        assert d.run(why()) == [f"{SANDBOX_EXEC} is missing"]
    finally:
        d.stop()
    bad = containment_check({"profile_ok": False, "reason": f"{SANDBOX_EXEC} is missing", "shell": False})
    assert bad.status == "fail" and "FERB's shell is unavailable until sandbox-exec works" in bad.detail
    assert containment_check({"profile_ok": True, "shell": True}).status == "ok"
    assert containment_check(None).status == "fail"


def test_the_daemon_reports_containment(daemon: RunningDaemon) -> None:
    assert daemon.daemon.containment == (True, "ok")
    assert {"shell.run", "shell.rules", "mac.applescript"} <= set(daemon.daemon.registry.capabilities)
    # The daemon socket is named too, not only covered by the network rule: a later network rule can't reopen it
    assert f'(remote unix-socket (path-literal "{daemon.sock}"))' in daemon.daemon.sandbox.profile()   # type: ignore[union-attr]


def test_dev_tools_work_in_the_workspace(daemon: RunningDaemon) -> None:
    """FERB's home may be looked at (metadata), so tools that walk up from the workspace work: git, a POSIX cd,
    python/node realpath, venv, and ls -l at the root (fix round 1). FERB's own files stay unreadable."""
    ws = daemon.daemon.home / "workspace"
    (ws / "proj").mkdir(exist_ok=True)
    g = task(daemon, "d", Identity.OWNER, SHELL)
    checks = {
        "git": "cd proj && git init -q . && git status --short; echo rc=$?",
        "cd+pwd": f"cd {ws} && pwd; echo rc=$?",
        "getcwd": "cd proj && python3 -c 'import os; print(os.getcwd())'; echo rc=$?",
        "venv": "cd proj && rm -rf v && python3 -m venv v && test -x v/bin/python3; echo rc=$?",
        "ls -l root": f"ls -l {ws} >/dev/null; echo rc=$?",
        "find": f"cd {ws} && find . -maxdepth 1 >/dev/null; echo rc=$?",
    }
    for name, cmd in checks.items():
        r = call(daemon, g, "shell.run", command=cmd)
        assert "rc=0" in r["content"]["output"] and not r["is_error"], f"{name}: {r['content']['output'][-300:]}"
    if __import__("shutil").which("node"):
        r = call(daemon, g, "shell.run", command="cd proj && node -e 'console.log(require(\"fs\").realpathSync(\".\"))'; echo rc=$?")
        assert "rc=0" in r["content"]["output"], r["content"]["output"][-300:]
    # Looking is allowed; reading FERB's own files is not
    (daemon.daemon.home / "run").mkdir(exist_ok=True)
    (daemon.daemon.home / "run" / "tok").write_text("secret-token")
    r = call(daemon, g, "shell.run", command=f"cat {daemon.daemon.home / 'run' / 'tok'}")
    assert r["is_error"] and "secret-token" not in str(r)


def test_the_shell_cannot_open_apps_urls_or_start_background_jobs(daemon: RunningDaemon) -> None:
    """job-creation is denied, and `open` of a temp file is refused (lsopen); the mach pins are a separate test. No
    app is launched, so none can be left running (fix round 1/2)."""
    g = task(daemon, "a", Identity.OWNER, SHELL)
    (daemon.daemon.home / "workspace" / "x.txt").write_text("x")
    label = f"com.ferb.test.sandbox.{__import__('os').getpid()}"
    try:
        r = call(daemon, g, "shell.run", command=f"launchctl submit -l {label} -- /usr/bin/true; echo rc=$?")
        assert "rc=0" not in r["content"]["output"]
        # `open` of a plain file is refused by lsopen; -g -j would keep any launch in the background, but none happens
        r = call(daemon, g, "shell.run", command=f"open -g -j {daemon.daemon.home / 'workspace' / 'x.txt'}; echo rc=$?")
        assert "rc=0" not in r["content"]["output"]
    finally:
        subprocess.run(["launchctl", "remove", label], capture_output=True)
    assert label not in subprocess.run(["launchctl", "list"], capture_output=True, text=True).stdout
    caps = daemon.daemon.registry.capabilities
    assert "can't open apps" in caps["shell.run"].description


def test_the_metadata_allow_is_a_literal_not_a_subpath(daemon: RunningDaemon) -> None:
    """`cd ~/.ferb` works (its own attributes are allowed), but stat of a file inside it fails: a literal→subpath slip
    would leak every inner file's metadata (fix round 2)."""
    ferb = daemon.daemon.home
    (ferb / "run").mkdir(exist_ok=True)
    (ferb / "run" / "token-x").write_text("secret-token")
    g = task(daemon, "s", Identity.OWNER, SHELL)
    r = call(daemon, g, "shell.run", command=f"cd {ferb} && pwd; echo rc=$?")
    assert "rc=0" in r["content"]["output"]
    r = call(daemon, g, "shell.run", command=f"stat {ferb}/run/token-x 2>&1; echo rc=$?")
    assert "rc=0" not in r["content"]["output"]
    r = call(daemon, g, "shell.run", command=f"cd {ferb}/run 2>&1; echo rc=$?")   # a folder inside stays closed
    assert "rc=0" not in r["content"]["output"]


def test_literal_parent_denies_only_cover_existing_folders(tmp_path: Path) -> None:
    """A first `cargo`/`rustup` install must be able to create ~/.cargo; the entry's own subpath deny still blocks
    creating the entry itself (fix round 1)."""
    home, ferb = tmp_path / "h", tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    sb = Sandbox(Protection(home, ferb, ["~/.cargo/credentials.toml"], [], []), tmp_path / "p")
    prof = sb.profile()
    assert f'(deny file-write* (literal "{home}/.cargo"))' not in prof        # ~/.cargo doesn't exist yet
    assert f'(deny file-write* (literal "{home}"))' in prof                   # the home does
    argv = sb.argv(f"mkdir -p {home}/.cargo/bin && echo ok > {home}/.cargo/bin/x && "
                   f"cat {home}/.cargo/credentials.toml 2>&1")
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=10)
    finally:
        sb.cleanup(sb.profile_path(argv))
    assert (home / ".cargo" / "bin" / "x").read_text() == "ok\n"              # the install folder is created
    (home / ".cargo" / "credentials.toml").write_text("token")
    assert run_under(sb, ["cat", str(home / ".cargo" / "credentials.toml")]).returncode != 0   # the entry stays denied


def test_a_missing_guarded_parent_cant_be_planted_as_a_link_or_renamed(tmp_path: Path) -> None:
    """A missing guarded parent (~/.cargo, above the entry) may be created as a real folder, but a long-lived process
    must not create it as a symlink, nor rename/remove it once made (fix round 2). The entry itself stays denied."""
    home, ferb = tmp_path / "h", tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    other = home / "other"
    other.mkdir(parents=True)
    sb = Sandbox(Protection(home, ferb, ["~/.cargo/credentials.toml"], [], []), tmp_path / "p")
    cargo = home / ".cargo"
    # It can't be created as a symlink (planting FERB-visible content behind the guarded name)
    assert run_under(sb, ["ln", "-s", str(other), str(cargo)]).returncode != 0 and not cargo.exists()
    # It can be created as a real folder, with children
    assert run_under(sb, ["/bin/sh", "-c", f"mkdir -p {cargo}/bin && echo ok > {cargo}/bin/x"]).returncode == 0
    assert (cargo / "bin" / "x").read_text() == "ok\n"
    # Once made it can't be removed or renamed
    assert run_under(sb, ["rmdir", str(cargo / "bin")]).returncode != 0 and (cargo / "bin").is_dir()
    assert run_under(sb, ["mv", str(cargo), str(home / "cargo-moved")]).returncode != 0 and cargo.is_dir()
    # And the entry itself is still unreadable
    (cargo / "credentials.toml").write_text("token")
    assert run_under(sb, ["cat", str(cargo / "credentials.toml")]).returncode != 0


def test_ferb_app_stays_closed_to_the_shell_even_when_its_folder_is_lifted(tmp_path: Path) -> None:
    """FERB.app holds the daemon's code: a lifted never-write entry for its folder (an approved escape) still leaves
    the app closed, wherever it is, and the app rule itself is never lifted."""
    home, ferb = tmp_path / "h", tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    apps = home / "Applications"
    app = apps / "FERB.app"
    (app / "Contents" / "Resources").mkdir(parents=True)
    sb = Sandbox(Protection(home, ferb, [], ["~/Applications"], [], app_bundle=app), tmp_path / "p")
    lifted = sb.profile("write:~/Applications")
    real = app.resolve()
    assert lifted.rindex(f'(deny file-write* (subpath "{real}"))') > lifted.index("(allow file-write* file-read-metadata")
    assert f'(deny file-write* (literal "{apps.resolve()}"))' in lifted       # its folder can't be moved away
    with pytest.raises(ValueError):
        sb.profile("ferb_app")
    planted = app / "Contents" / "Resources" / "planted.pth"
    argv = sb.wrap(["/bin/sh", "-c", f"echo import os > '{planted}'; mv '{app}' '{apps}/Moved.app'"],
                   "write:~/Applications")
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=10)
    finally:
        sb.cleanup(sb.profile_path(argv))
    assert not planted.exists() and app.is_dir() and "Operation not permitted" in r.stderr
    other = apps / "Other.txt"                                               # the lift still works for the rest
    argv = sb.wrap(["/bin/sh", "-c", f"echo ok > '{other}'"], "write:~/Applications")
    try:
        subprocess.run(argv, capture_output=True, text=True, timeout=10)
    finally:
        sb.cleanup(sb.profile_path(argv))
    assert other.read_text() == "ok\n"
