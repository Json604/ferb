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
from ferb.device.install import MARKER, NATIVE_HOST_MANIFEST, InstallError, Installer, code_hash, repo_root
from ferb.device.profile import ProfileStore, diff, summarize
from ferb.device.scan import scan
from ferb.identity import seed

Connect = Callable[[], Client]
Ask = Callable[[str], str]
REQUIRED_PERMISSIONS = ("accessibility", "screen_recording")
BRAVE = "com.brave.Browser"
IDENTITY_CHECK_S = 90.0


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

    env.say("1/4  Installing FERB's dedicated Python, its venv and the LaunchAgent…")
    inst.install()
    client = wait_for_daemon(env)
    binary = inst.granted_binary()
    env.say(f"     Daemon running. Permissions must be granted to: {binary}")

    env.say("2/4  Scanning this Mac's apps (Info.plist only)…")
    config = Config(inst.home)
    profile = scan(device_id(config))
    store = ProfileStore(inst.home)
    store.save(profile, "setup scan")
    s = summarize(profile)
    env.say(f"     {s['apps']} apps, {s['scriptable']} scriptable; browsers: {', '.join(s['browsers']) or 'none'}")

    env.say("3/4  Permissions (checked by the daemon itself)")
    for perm in REQUIRED_PERMISSIONS:
        client = _grant(env, client, perm, binary, required=True)
    client = _input_monitoring_if_needed(env, client, binary)      # macOS needs it when the tap can't be made

    env.say("4/4  Self-test through the daemon")
    ok = _print_selftest(env, client.call("doctor.check"))
    client.close()

    # Progressive setup (section 8): nothing else is asked now. Optional permissions and per-app Automation are asked
    # the first time a task needs them; FERB's own identity (`ferb setup --identity`) and Jev (`ferb setup --jev`)
    # when first wanted; facts such as the owner's email when a request first needs them.
    env.say("     Everything else is asked the first time a task needs it.")
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


def _input_monitoring_if_needed(env: Env, client: Client, binary: Path | None) -> Client:
    tap = next((r for r in client.call("doctor.check")["selftest"] if r["name"] == "event_tap"), None)
    if tap and not tap["ok"]:
        env.say("     The owner-input watcher couldn't create its event tap: Input Monitoring is needed.")
        client = _grant(env, client, "input_monitoring", binary, required=False)
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
        rs = report.get("runtimes", {})
        for name in ("codex", "claude"):
            if name in rs:
                ok = not str(rs[name]).startswith("problem")
                used = name in (rs.get("orchestrator"), rs.get("worker"))
                add(Check(f"runtime {name}", "ok" if ok else ("fail" if used else "warn"), str(rs[name]),
                          "run `ferb setup --update`" if not ok else ""))
        if rs:
            add(Check("roles", "ok", f"supervisor {rs.get('orchestrator')}, worker {rs.get('worker')}"))
        for name, why in rs.get("unregistered", {}).items():
            add(Check(f"runtime {name} unregistered", "fail", why, "a run showed a tool outside the allowlist; "
                                                                   "restart the daemon after investigating"))
        try:
            c = env.connect()
            if hasattr(c, "set_timeout"):
                c.set_timeout(IDENTITY_CHECK_S)                 # it may wake FERB's browser profile first
            ident = c.call("identity.check")
            c.close()
        except (NotRunning, FerbError) as e:
            ident = None
            add(Check("identity session", "warn", f"couldn't check: {e}", "run `ferb doctor` again"))
        if ident:
            if not ident["profile"]:
                add(Check("identity profile", "warn", "not set up", "run `ferb setup --identity`"))
            else:
                add(Check("identity profile", "ok" if ident["profile_ok"] else "fail",
                          f"{ident.get('profile_name')} ({ident['profile']}) holds only {ident['address']}"
                          if ident["profile_ok"] else str(ident["error"]),
                          "FERB's Chrome profile must hold only FERB's account; fix it in Chrome, or run "
                          "`ferb setup --identity`"))
                if ident["profile_ok"] and ident.get("paired") and ident.get("error"):
                    add(Check("identity session", "warn", f"couldn't check: {ident['error']}",
                              "open FERB's Chrome profile (FERB reconnects within 30 s) and run doctor again"))
                elif ident["profile_ok"]:
                    add(Check("identity session", "ok" if ident.get("session") else
                              ("fail" if ident.get("paired") else "warn"),
                              f"signed in as {ident['address']} (through the extension)" if ident.get("session") else
                              ("not signed in" if ident.get("paired") else "not paired"),
                              "sign FERB's account into its Chrome profile" if ident.get("paired") else
                              "run `ferb setup --identity`"))
            for s in ident["secrets"]:
                add(Check(f"secret {s['key']}", "ok" if s["in_keychain"] else "warn",
                          "in Keychain" if s["in_keychain"] else "missing from Keychain",
                          f"in your own terminal: security add-generic-password -s ferb -a {s['key']} -w"))
        for hosts_dir in inst.native_hosts_dirs:
            m = hosts_dir / NATIVE_HOST_MANIFEST
            add(Check(f"native host ({hosts_dir.parent.name})", "ok" if m.exists() and inst.native_host_launcher.exists()
                      else "fail", str(m) if m.exists() else "missing", "run `ferb setup --update`"))
        try:
            c = env.connect()
            profiles = c.call("browsers.list")["profiles"]
            c.close()
        except (NotRunning, FerbError):
            profiles = []
        for p in (p for p in profiles if p["paired"]):
            name = f"browser {p['browser']}/{p['name']}"
            if p["refused"]:
                add(Check(name, "fail", p["refused"], "reload the FERB extension in that profile"))
            elif not p["connected"] and p.get("identity") == "ferb":   # FERB's own: opened only when a task needs it
                add(Check(name, "ok", "FERB's own profile: paired, not open right now (FERB opens it when a task "
                                      "needs it)"))
            else:
                add(Check(name, "ok" if p["connected"] else "warn",
                          f"connected, extension {p['version']}" if p["connected"] else "paired, not connected",
                          "open that profile; FERB reconnects within 30 s"))
        if report["stopped"]:
            add(Check("emergency stop", "warn", "engaged", "`ferb resume-tasks` lifts it"))
        try:
            c = env.connect()
            sense = c.call("sense.status")
            c.close()
        except (NotRunning, FerbError):
            sense = None
        if sense is not None:
            if sense["paused"]:
                add(Check("watchers", "warn", "paused: nothing is recorded, no real mouse / keyboard input",
                          "`ferb resume`"))
            else:
                add(Check("watchers", "ok", ", ".join(sense["watchers"]) or "none running"))
            try:
                c = env.connect()
                jev = c.call("jev.stats")
                c.close()
            except (NotRunning, FerbError):
                jev = None
            if jev is not None and jev.get("configured"):
                if jev["recent_total"] and jev["recent_answered"] == 0:
                    add(Check("jev", "warn", f"not answering ({jev['last_error']}); FERB uses its own decisions",
                              "check the key: `ferb setup --jev`"))
                else:
                    on = [s["question"] for s in jev["stats"] if s["enabled"]]
                    add(Check("jev", "ok", f"shadow mode{'; enabled: ' + ', '.join(on) if on else ''}"))
            for source, why in sense.get("failed", {}).items():
                add(Check(f"watcher {source}", "warn", f"stopped: {why}", "restart FERB (`ferb setup --update`)"))
            for problem in sense["folder_problems"]:
                add(Check("watch folder", "warn", problem, "fix [sense] watch_folders in ~/.ferb/config.toml"))
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


# ---------- FERB's identity browser (section 8 step 3) ----------

def identity_setup(env: Env, name: str | None = None, chrome_data: Path = seed.CHROME_DATA) -> int:
    """Find the Chrome profile holding exactly FERB's account, confirm it with the owner, record it. Nothing is
    copied and Chrome is never quit; the extension is installed and paired there in build step 6."""
    config = Config(env.inst.home)
    address = str(config.get("mailbox", "address", "") or "")
    if not address:
        address = env.ask("     FERB's own Google address (the account in its Chrome profile): ").strip().lower()
        if "@" not in address:
            env.say("     That isn't an email address; nothing changed.")
            return 1
        config.set("mailbox", "provider", "gmail")
        config.set("mailbox", "address", address)
    try:
        p = seed.choose(seed.profiles(chrome_data), address, name)
    except seed.SeedError as e:
        env.say(f"     {e}")
        return 1
    env.say(f"     Chrome profile “{p.name}” (folder {p.folder}); Chrome accounts: {', '.join(p.accounts)}")
    if env.ask("     Use it as FERB's identity profile? [y/N] ").strip().lower() not in ("y", "yes"):
        env.say("     Not recorded.")
        return 1
    config.set("identity", "browser", "chrome")
    config.set("identity", "profile", p.folder)
    return 0 if _identity_pair(env, p.name) and _identity_status(env) else 1


def _identity_pair(env: Env, name: str) -> bool:
    """Pair the extension in FERB's profile as identity FERB (section 9.6). Re-pairing asks the owner first."""
    env.ask(f"     Load the FERB extension in the “{name}” profile (chrome://extensions → Developer mode → Load "
            f"unpacked → {repo_root() / 'extensions' / 'chromium'}), then press Enter ")
    confirm = False
    for _ in range(2):
        try:
            c = env.connect()
            r = c.call("identity.pair", {"confirm": confirm})
            c.close()
        except (NotRunning, FerbError) as e:
            if "needs your confirmation" in str(e) and not confirm:
                confirm = env.ask("     FERB's profile is already paired. Pair it again? [y/N] ").strip().lower() in ("y", "yes")
                if confirm:
                    continue
                env.say("     Kept the existing pairing.")
                return True
            env.say(f"     Couldn't pair: {e}")
            return False
        if r["paired"]:
            env.say(f"     Paired “{name}” as FERB's identity.")
            return True
        env.say(f"     Not paired: {r.get('hint', 'no answer from the extension')}")
        return False
    return False


def _identity_status(env: Env) -> bool:
    try:
        c = env.connect()
        if hasattr(c, "set_timeout"):
            c.set_timeout(IDENTITY_CHECK_S)
        r = c.call("identity.check")
        c.close()
    except (NotRunning, FerbError) as e:
        env.say(f"     Couldn't check: {e}")
        return False
    if not r["profile_ok"]:
        env.say(f"     FERB's identity profile isn't usable: {r['error']}")
        return False
    if not r.get("paired"):
        env.say(f"     Profile “{r.get('profile_name')}” holds only {r['address']}, but isn't paired yet: "
                "`ferb setup --identity`.")
        return False
    if r.get("session"):
        env.say(f"     Paired; signed in as {r['address']}.")
        return True
    env.say(f"     Paired, but not signed in as {r['address']}{' (' + r['error'] + ')' if r.get('error') else ''}: "
            "sign FERB's account back into its Chrome profile.")
    return False


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


def jev_setup(env: Env) -> int:
    """The Jev key goes straight into the Keychain (`security` asks for it; it's never an argument, never in chat or
    config), then the daemon picks it up and makes one test call."""
    import subprocess
    from ferb.decide.decider import KEY_ACCOUNT
    from ferb.identity.secrets import SERVICE
    env.say("Paste your Jev API key when `security` asks (twice; nothing is shown):")
    r = subprocess.run(["security", "add-generic-password", "-U", "-s", SERVICE, "-a", KEY_ACCOUNT, "-w"])
    if r.returncode != 0:
        env.say("The key wasn't stored.")
        return 1
    try:
        c = env.connect()
        out = c.call("jev.reload", {"ping": True})
        c.close()
    except (NotRunning, FerbError) as e:
        env.say(f"Stored. The daemon couldn't check it yet: {e}")
        return 1
    if not out.get("configured"):
        env.say("Stored, but the daemon can't read it from the Keychain.")
        return 1
    env.say("Jev answered: shadow mode is on for every question." if out.get("ping") == "ok"
            else f"Stored, but Jev didn't answer the test call: {out.get('ping')}")
    return 0 if out.get("ping") == "ok" else 1
