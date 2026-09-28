"""`ferb setup [--update]`, `ferb doctor`, `ferb uninstall [--purge]` (section 8, section 15).

Interactive but injectable (installer, daemon connection, prompts, System Settings opener) so the flows are
testable without touching the real Mac. Permissions are always checked by the daemon, never by this process.
Setup never quits or kills a browser and never touches Brave.
"""

import subprocess
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, TextIO

import ferb
from ferb.cli.client import Client
from ferb.config import Config, InsecureConfig
from ferb.core.errors import FerbError, NotRunning
from ferb.device import permissions
from ferb.device.install import MARKER, InstallError, Installer, code_hash, repo_root
from ferb.device.profile import ProfileStore, diff, summarize
from ferb.device.scan import scan

Connect = Callable[[], Client]
Ask = Callable[[str], str]
REQUIRED_PERMISSIONS = ("accessibility", "screen_recording")
BRAVE = "com.brave.Browser"


@dataclass
class Env:
    inst: Installer
    connect: Connect
    ask: Ask
    out: TextIO
    open_pane: Callable[[str], None] = permissions.open_pane
    sleep: Callable[[float], None] = time.sleep
    running_apps: Callable[[], set[str]] = lambda: _running_bundle_ids()

    def say(self, line: str = "") -> None:
        print(line, file=self.out, flush=True)


def _running_bundle_ids() -> set[str]:
    from AppKit import NSWorkspace
    return {str(a.bundleIdentifier()) for a in NSWorkspace.sharedWorkspace().runningApplications()
            if a.bundleIdentifier()}


def wait_for_daemon(env: Env, timeout: float = 30) -> Client:
    end = time.monotonic() + timeout
    while True:
        try:
            c = env.connect()
            c.call("ping")
            return c
        except (NotRunning, FerbError):
            if time.monotonic() > end:
                raise NotRunning("the daemon didn't come up; see ~/.ferb/logs/daemon.err.log") from None
            env.sleep(0.3)


def device_id(config: Config) -> str:
    existing = config.get("device", "id")
    if existing:
        return str(existing)
    new = uuid.uuid4().hex
    config.set("device", "id", new)
    return new


# ---------- setup ----------

def setup(env: Env, update: bool = False) -> int:
    inst = env.inst
    if update:
        env.say("Updating FERB in its venv and restarting the daemon…")
        inst.install(update_only=True)
        wait_for_daemon(env)
        env.say("Done.")
        return 0

    try:                                   # a foreground dev daemon would hold the socket
        c = env.connect()
        pid = c.call("ping")["pid"]
        c.close()
        if pid != inst.agent_pid():
            env.say(f"A FERB daemon (pid {pid}) is already running outside launchd — probably `ferb daemon run` "
                    "in a terminal. Stop it (Ctrl-C) and run `ferb setup` again.")
            return 1
    except NotRunning:
        pass

    env.say("1/6  Installing FERB's dedicated Python, its venv and the LaunchAgent…")
    inst.install()
    client = wait_for_daemon(env)
    binary = inst.granted_binary()
    env.say(f"     Daemon running. Permissions must be granted to: {binary}")

    env.say("2/6  Scanning this Mac's apps (Info.plist only)…")
    config = Config(inst.home)
    profile = scan(device_id(config))
    store = ProfileStore(inst.home)
    store.save(profile, "setup scan")
    s = summarize(profile)
    env.say(f"     {s['apps']} apps, {s['scriptable']} scriptable; browsers: {', '.join(s['browsers']) or 'none'}")

    env.say("3/6  Permissions (checked by the daemon itself)")
    for perm in REQUIRED_PERMISSIONS:
        client = _grant(env, client, perm, binary, required=True)

    env.say("4/6  Optional permissions")
    client = _optional(env, client, binary)

    env.say("5/6  Automation (Apple Events) for apps you choose")
    _automation(env, client, profile)

    env.say("6/6  Self-test through the daemon")
    ok = _print_selftest(env, client.call("doctor.check"))
    client.close()
    env.say("Setup complete." if ok else "Setup finished with problems — `ferb doctor` shows what to fix.")
    return 0 if ok else 1


def _status(client: Client, perm: str) -> str:
    return str(client.call("doctor.check", {"selftest": False})["permissions"][perm])


def _grant(env: Env, client: Client, perm: str, binary: Path | None, required: bool) -> Client:
    label = perm.replace("_", " ").title()
    while _status(client, perm) != "granted":
        env.say(f"     {label}: not granted. In the System Settings pane that just opened, click +, press ⌘⇧G,")
        env.say(f"     paste {binary}, click Open and switch it on.")
        env.open_pane(perm)
        answer = env.ask(f"     Press Enter when done{'' if required else ' (or type skip)'}, or 'skip' to continue "
                         "without it: ").strip().lower()
        if answer == "skip":
            env.say(f"     Skipped {label}. {'FERB cannot act on the Mac without it; ' if required else ''}"
                    "`ferb doctor` will keep reminding you.")
            return client
        if perm == "screen_recording":          # macOS applies a new Screen Recording grant only after a restart
            client.close()
            env.inst.restart()
            client = wait_for_daemon(env)
    env.say(f"     {label}: granted ✓")
    return client


def _optional(env: Env, client: Client, binary: Path | None) -> Client:
    tap = next((r for r in client.call("doctor.check")["selftest"] if r["name"] == "event_tap"), None)
    if tap and not tap["ok"]:
        env.say("     The owner-input watcher couldn't create its event tap: Input Monitoring is needed.")
        client = _grant(env, client, "input_monitoring", binary, required=False)
    if _status(client, "full_disk_access") != "granted":
        if env.ask("     Full Disk Access is optional (not needed in Block 1). Grant it now? [y/N] ").strip().lower() == "y":
            client = _grant(env, client, "full_disk_access", binary, required=False)
    return client


def _automation(env: Env, client: Client, profile: Any) -> None:
    running = env.running_apps()
    apps = [a for a in profile.apps if a.scriptable and a.bundle_id in running and a.bundle_id != BRAVE]
    if not apps:
        env.say("     No scriptable apps are running; each app is asked the first time FERB needs it.")
        return
    for i, a in enumerate(apps, 1):
        env.say(f"     {i:>2}. {a.name} ({a.bundle_id})")
    picked = env.ask("     Numbers to request now, comma-separated (Enter for none): ").strip()
    chosen = [apps[int(n) - 1].bundle_id for n in picked.replace(" ", "").split(",") if n.isdigit()
              and 1 <= int(n) <= len(apps)]
    if chosen:
        for bundle, status in client.call("automation.request", {"bundle_ids": chosen})["results"].items():
            env.say(f"     {bundle}: {status}")


def _print_selftest(env: Env, report: dict[str, Any]) -> bool:
    ok = True
    for r in report["selftest"]:
        env.say(f"     {'✓' if r['ok'] else '✗'} {r['name']}: {r['detail']}")
        ok &= bool(r["ok"])
    return ok


# ---------- doctor ----------

@dataclass
class Check:
    name: str
    status: str            # "ok" | "fail" | "warn"
    detail: str
    hint: str = ""


def doctor(env: Env) -> int:
    inst, checks = env.inst, []
    add = checks.append
    add(Check("home", "ok" if (inst.home / MARKER).exists() else "fail", str(inst.home), "run `ferb setup`"))
    try:
        Config(inst.home).load()
        add(Check("config permissions", "ok", "owner-only"))
    except InsecureConfig as e:
        add(Check("config permissions", "fail", str(e), "chmod 600 ~/.ferb/config.toml; chmod 700 ~/.ferb"))
    add(Check("interpreter", "ok" if inst.interpreter() else "fail", str(inst.interpreter() or "missing"),
              "run `ferb setup`"))
    add(Check("venv", "ok" if inst.venv_python.exists() else "fail", str(inst.venv), "run `ferb setup`"))
    import plistlib
    current = plistlib.loads(inst.plist_path.read_bytes()) if inst.plist_path.exists() else None
    add(Check("LaunchAgent", "ok" if current == inst.plist() else "fail",
              "installed" if current == inst.plist() else ("differs from expected" if current else "missing"),
              "run `ferb setup`"))
    add(Check("agent loaded", "ok" if inst.loaded() else "fail", inst.label, "run `ferb setup`"))
    try:
        c = env.connect()
        report = c.call("doctor.check")
        c.close()
    except (NotRunning, FerbError) as e:
        add(Check("daemon", "fail", str(e), "check ~/.ferb/logs/daemon.err.log, then `ferb setup`"))
        report = None
    if report:
        dedicated = inst.granted_binary()
        add(Check("daemon interpreter", "ok" if dedicated and Path(report["executable"]) == dedicated else "warn",
                  report["executable"],
                  "this is a dev daemon (`ferb daemon run`), not the installed one" if report["executable"] != str(
                      dedicated) else ""))
        here = code_hash(Path(ferb.__file__).parent)
        if (repo_root() / "pyproject.toml").exists() and report["code_hash"] != here:
            add(Check("installed code", "warn", "differs from this checkout", "run `ferb setup --update`"))
        for perm, status in report["permissions"].items():
            level = "fail" if perm in REQUIRED_PERMISSIONS and status != "granted" else (
                "ok" if status == "granted" else "warn")
            optional = " (optional)" if perm == "full_disk_access" else ""
            add(Check(f"permission {perm}{optional}", level, status,
                      f"grant it to {dedicated} (System Settings → Privacy & Security)" if level != "ok" else ""))
        for r in report["selftest"]:
            add(Check(f"self-test {r['name']}", "ok" if r["ok"] else "fail", r["detail"]))
        if report["stopped"]:
            add(Check("emergency stop", "warn", "engaged", "`ferb resume-tasks` lifts it"))
    did = Config(inst.home).get("device", "id") if (inst.home / "config.toml").exists() else None
    stored = ProfileStore(inst.home).load(str(did)) if did else None
    if stored is None:
        add(Check("device profile", "fail", "missing", "run `ferb setup`"))
    else:
        d = diff(stored, scan(stored.device_id))
        add(Check("device profile", "ok" if d.empty else "warn",
                  "up to date" if d.empty else f"{len(d.added)} added, {len(d.removed)} removed, {len(d.changed)} "
                                               "changed since last scan", "" if d.empty else "run `ferb setup`"))
    mark = {"ok": "✓", "warn": "!", "fail": "✗"}
    for ch in checks:
        env.say(f"{mark[ch.status]} {ch.name}: {ch.detail}" + (f"  → {ch.hint}" if ch.hint and ch.status != "ok" else ""))
    return 1 if any(ch.status == "fail" for ch in checks) else 0


# ---------- uninstall ----------

def uninstall(env: Env, purge: bool) -> int:
    inst = env.inst
    secret_keys: list[str] = []
    if purge:
        if not (inst.home / MARKER).exists():
            env.say(f"Refusing to purge {inst.home}: it has no {MARKER} marker.")
            return 1
        if env.ask(f"This deletes {inst.home} entirely (database, journal, config, FERB's Chrome profile). "
                   "Type 'delete' to confirm: ").strip() != "delete":
            env.say("Not purged.")
            purge = False
        else:
            try:
                secret_keys = list((Config(inst.home).load().get("secrets") or {}).keys())
            except InsecureConfig:
                secret_keys = []
    binary = inst.granted_binary()
    try:
        removed = inst.uninstall(purge=purge)
    except InstallError as e:
        env.say(str(e))
        return 1
    for r in removed:
        env.say(f"removed {r}")
    if not removed:
        env.say("Nothing to remove.")
    for key in secret_keys:
        if env.ask(f"Also delete FERB's Keychain item '{key}'? [y/N] ").strip().lower() == "y":
            r = subprocess.run(["security", "delete-generic-password", "-s", "ferb", "-a", key], capture_output=True)
            env.say(f"Keychain '{key}': {'deleted' if r.returncode == 0 else 'not found'}")
    if binary:
        env.say(f"macOS can't remove permissions for a plain binary: in System Settings → Privacy & Security, remove "
                f"{binary} from Accessibility, Screen Recording (and Input Monitoring / Automation if granted).")
    return 0
