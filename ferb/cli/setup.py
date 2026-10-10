"""Own explicit installation, permission setup and update workflows. Must not run app interactions as task recovery.

`ferb setup [--update]`, `ferb doctor`, `ferb uninstall [--purge]`.

Interactive but injectable (installer, daemon connection, prompts, System Settings opener) so the flows are
testable without touching the real Mac. Permissions are always checked by the daemon, never by this process.
Setup never quits or kills a browser and never touches Brave."""

import subprocess
import time
import uuid
from dataclasses import dataclass
from datetime import datetime
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
SETUP_PERMISSIONS = (*REQUIRED_PERMISSIONS, 'input_monitoring', 'full_disk_access')
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
        client = wait_for_daemon(env)
        config = Config(inst.home)
        health = client.call("doctor.check", {"selftest": False})
        missing = [f"{name}={status}" for name, status in health.get("permissions", {}).items()
                   if status != "granted"]
        if config.get('setup', 'permissions_version', 0) < 22 or missing:
            facts = ", ".join(missing) or "permission setup has not been recorded"
            env.say(f"Permission setup deferred: {facts}. Run `ferb setup` when you want its permission dialogs.")
        client.close()
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
    for perm in SETUP_PERMISSIONS:
        client = _grant(env, client, perm, binary, required=perm in REQUIRED_PERMISSIONS)
    _automation(env, client, profile)
    config.set('setup', 'permissions_version', 22)

    env.say("4/4  Self-test through the daemon")
    ok = _print_selftest(env, client.call("doctor.check"))
    client.close()

    env.say("     FERB uses the installed action registry without per-action permission prompts.")
    env.say("     Connect the accounts/profiles you want available; macOS permissions and logins still apply.")
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


def _automation(env: Env, client: Client, profile: Any) -> None:
    running = env.running_apps()
    apps = [a for a in (profile.apps if profile else []) if a.scriptable and a.bundle_id in running]
    if not apps:
        env.say("     No scriptable apps are running. Rerun setup to grant Automation for additional apps.")
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


def codex_login(env: Env, run: Callable[..., Any] = subprocess.run) -> int:
    """Sign in FERB's own Codex home (owner decision, 2026-10-01): the pinned Codex, with CODEX_HOME set to
    ~/.ferb/codex-home, so FERB's runs never load the owner's personal Codex settings or skills."""
    import os
    from ferb.runtimes.codex_exec import codex_binary
    home = env.inst.home / "codex-home"
    home.mkdir(parents=True, exist_ok=True)
    os.chmod(home, 0o700)
    try:
        binary = codex_binary(env.inst.home)
    except Exception as e:
        env.say(f"FERB's pinned Codex isn't installed: {e}")
        return 1
    env.say("Signing in FERB's own Codex (a browser window opens)…")
    rc = run([str(binary), "login"], env={**os.environ, "CODEX_HOME": str(home)}).returncode
    ok = (home / "auth.json").exists()
    env.say("Done: FERB now uses its own Codex home." if rc == 0 and ok else "Not signed in; nothing changed.")
    return 0 if ok else 1


def protection_check(home: Path) -> Check:
    """[protection] and FERB's workspace as the daemon reads them at start (rule 10): when this fails, FERB doesn't
    start, and this says why."""
    from ferb.policy import protection
    try:
        p = protection.load(Config(home).get, Path.home(), home)
    except FerbError as e:
        return Check("protection", "fail", str(e), "fix [protection] in ~/.ferb/config.toml (or the workspace), then "
                     "`ferb setup --update`")
    files = "may read the never-read stores" if p.fs_reads_never_read else "stay out of the never-read stores"
    return Check("protection", "ok", f"{len(p.reads)} never-read and {len(p.writes)} never-write entries; FERB's "
                 f"tools that name files {files}")


def containment_check(c: dict[str, Any] | None) -> Check:
    """FERB's shell sandbox as the running daemon sees it (S2, Q7): without it the shell and AppleScript tools are
    left out, never run uncontained."""
    if not c:
        return Check("containment", "fail", "the daemon didn't report it", "run `ferb setup --update`")
    if c.get("profile_ok") and c.get("shell"):
        detail = "FERB's shell and AppleScript run inside its sandbox (sandbox-exec)"
        if c.get("proxy") and not c.get("proxy_alive", True):
            return Check("containment", "fail", detail + "; FERB's egress proxy didn't answer, so the shell has no "
                         f"network (last error: {c.get('proxy_last_error') or 'none'})",
                         "restart the daemon (`ferb setup --update`)")
        if c.get("proxy_restarted"):
            detail += ("; FERB's egress proxy had stopped and was restarted "
                       f"(last error: {c.get('proxy_last_error') or 'none'})")
        return Check("containment", "ok", detail)
    why = c.get("reason") if not c.get("profile_ok") else c.get("shell_reason")
    return Check("containment", "fail", f"FERB's shell is unavailable until sandbox-exec works: {why}",
                 "restart the daemon after fixing it (`ferb setup --update`)")


def snapshot_check(c: dict[str, Any] | None) -> Check:
    """S1 snapshots before shell work and permanent deletes (OPEN QUESTION 3: when one can't be taken the command
    still runs, and this says so)."""
    if not c:
        return Check("snapshots", "warn", "the daemon didn't report them", "run `ferb setup --update`")
    failure = c.get("snapshot_failure") or {}
    if c.get("snapshots") is False or failure.get("off"):
        return Check("snapshots", "warn", "snapshots are turned off: shell commands and permanent deletes run without "
                     "a recovery snapshot", "")
    if failure:
        when = ""
        try:
            when = " at " + datetime.fromisoformat(str(failure["at"])).astimezone().strftime("%Y-%m-%d %H:%M")
        except (KeyError, ValueError):
            pass
        return Check("snapshots", "warn", f"the last snapshot attempt failed{when} ({failure.get('why')}); shell "
                     "commands since then ran without a recovery snapshot", "check that Time Machine local snapshots "
                     "work (`tmutil localsnapshot`)")
    return Check("snapshots", "ok", "a local snapshot comes before shell work and permanent deletes (one per 15 "
                 "minutes, reused)")


def doctor(env: Env) -> int:
    inst = env.inst
    checks: list[Check] = []
    add = checks.append
    add(Check("home", "ok" if (inst.home / MARKER).exists() else "fail", str(inst.home), "run `ferb setup`"))
    own = (inst.home / "codex-home" / "auth.json").exists()
    add(Check("codex home", "ok" if own else "warn",
              "FERB's own (~/.ferb/codex-home)" if own else "using your personal Codex setup",
              "`ferb setup --codex-login` signs in FERB's own Codex"))
    try:
        Config(inst.home).load()
        add(Check("config permissions", "ok", "owner-only"))
    except InsecureConfig as e:
        add(Check("config permissions", "fail", str(e), "chmod 600 ~/.ferb/config.toml; chmod 700 ~/.ferb"))
    add(protection_check(inst.home))
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
        add(containment_check(report.get("containment")))
        add(snapshot_check(report.get("containment")))
        voice = report.get("voice", {})
        if voice.get("push_to_talk"):
            perms = voice.get("permissions", {})
            ok = voice.get("connected") and perms.get("microphone") == "granted" and perms.get("speech") == "granted"
            add(Check("push-to-talk", "ok" if ok else "warn",
                      "FERB.app connected, microphone + speech allowed" if ok else
                      ("FERB.app not connected" if not voice.get("connected") else
                       f"microphone: {perms.get('microphone')}, speech: {perms.get('speech')}"),
                      "" if ok else "run `ferb setup --voice`"))
        rs = report.get("runtimes", {})
        for name in ("codex", "claude"):
            if name in rs:
                ok = not str(rs[name]).startswith("problem")
                used = name in (rs.get("orchestrator"), rs.get("worker"))
                add(Check(f"runtime {name}", "ok" if ok else ("fail" if used else "warn"), str(rs[name]),
                          "run `ferb setup --update`" if not ok else ""))
        from ferb.runtimes.catalog import CATALOG, ROLES
        cfg = Config(inst.home)
        for role in ROLES:
            default = cfg.get("runtimes", "worker", "codex") if role == "checker" else "codex"
            rname = str(cfg.get("runtimes", role, default))
            usable, fix = CATALOG[rname].available(inst.home) if rname in CATALOG else (False, "check config.toml [runtimes]")
            add(Check(f"runtime {role}", "ok", rname) if usable else Check(f"runtime {role}", "warn", f"{rname} not usable", fix))
        if rs:
            add(Check("roles", "ok", f"supervisor {rs.get('orchestrator')}, worker {rs.get('worker')}"))
        for name, why in rs.get("unregistered", {}).items():
            add(Check(f"runtime {name} unregistered", "fail", why, "a run showed a tool outside the allowlist; "
                                                                   "restart the daemon after investigating"))
        try:
            c = env.connect()
            ident = c.call("identity.check", {"session": False})
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
                if ident["profile_ok"]:
                    paired = bool(ident.get("paired"))
                    add(Check("identity session", "warn",
                              "session not checked (observational doctor; no browser opened)" if paired else
                              "not paired; session not checked",
                              "" if paired else "run `ferb setup --identity`"))
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


# ---------- FERB's identity browser ----------

def identity_setup(env: Env, name: str | None = None, chrome_data: Path = seed.CHROME_DATA) -> int:
    """Find the Chrome profile holding exactly FERB's account, confirm it with the owner, record it. Nothing is
    copied and Chrome is never quit; extension pairing is a separate setup action."""
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
    """Pair the extension in FERB's profile as identity FERB. Re-pairing asks the owner first."""
    from ferb.device.extensions import extension_path
    env.ask(f"     Load the FERB extension in the “{name}” profile (chrome://extensions → Developer mode → Load "
            f"unpacked → {extension_path(env.inst.home)}), then press Enter ")
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


CERTIFICATE = "FERB Local Code Signing"


def voice_setup(env: Env, run: Callable[..., Any] = subprocess.run) -> int:
    """Push-to-talk: build FERB.app into ~/.ferb, sign it with the local certificate (so its permissions
    survive rebuilds), record its signature for the daemon's check, ask for Microphone and Speech Recognition once,
    turn push-to-talk on. The daemon is not restarted."""
    # not `-v`: that lists only certificates macOS trusts, and a self-made one never is (it still signs fine)
    found = run(["security", "find-identity", "-p", "codesigning"], capture_output=True, text=True)
    if CERTIFICATE not in (found.stdout or ""):
        env.say(f"The signing certificate \"{CERTIFICATE}\" isn't in your login keychain. Create it in Keychain Access →")
        env.say("Certificate Assistant → Create a Certificate (name it exactly that, type Code Signing), then run this again.")
        return 1
    app = env.inst.home / "FERB.app"
    env.say("Building FERB.app (the microphone part of push-to-talk; no windows, no Dock icon)…")
    built = run([str(repo_root() / "macapp" / "FERB" / "build.sh"), str(app)], capture_output=True, text=True)
    if built.returncode != 0:
        env.say("Build failed:\n" + ((built.stderr or built.stdout or "").strip()[-1500:]))
        return 1
    shown = run(["codesign", "-d", "-r-", str(app)], capture_output=True, text=True)
    req = next((ln.split("=>", 1)[1].strip() for ln in ((shown.stdout or "") + (shown.stderr or "")).splitlines()
                if ln.startswith("designated =>")), "")
    if "certificate leaf" not in req:
        env.say("FERB.app isn't signed with the local certificate; push-to-talk stays off.")
        return 1
    config = Config(env.inst.home)
    config.set("voice", "requirement", req)
    env.say("macOS will ask once for Microphone and Speech Recognition for FERB — allow both.")
    run(["open", "-n", "-W", str(app), "--args", "request"])
    config.set("voice", "push_to_talk", True)
    if config.get("voice", "mic") is None:
        config.set("voice", "mic", "auto")
    try:
        c = env.connect()
        c.call("voice.reload")
        report: dict[str, Any] = {}
        for _ in range(20):                                   # FERB.app loads its speech model, then connects
            report = c.call("doctor.check", {"selftest": False}).get("voice", {})
            if report.get("connected"):
                break
            env.sleep(1)
        c.close()
    except (NotRunning, FerbError) as e:
        env.say(f"Push-to-talk is on, but the daemon couldn't start it yet: {e}")
        return 1
    perms = report.get("permissions", {})
    if not report.get("connected"):
        env.say("Push-to-talk is on, but FERB.app didn't connect. `ferb doctor` shows more.")
        return 1
    if perms.get("microphone") != "granted" or perms.get("speech") != "granted":
        env.say(f"FERB.app is connected but lacks permission (microphone: {perms.get('microphone')}, speech: "
                f"{perms.get('speech')}). Allow FERB in System Settings → Privacy & Security, then run this again.")
        return 1
    env.say("Push-to-talk is ready: hold Right Option, speak, let go. A tap or a shortcut (Right Option + a key) is ignored.")
    env.say("It uses the built-in mic whenever your default input is a Bluetooth headset (it never opens a headset's mic).")
    return 0


def key_setup(env: Env, provider: str) -> int:
    """The key goes straight into the Keychain (`security` asks for it; never an argument, never in chat or config),
    then the daemon makes one tiny typed call with it."""
    import subprocess
    from ferb.identity.secrets import SERVICE
    from ferb.runtimes.keys import account
    env.say(f"Paste your {provider} API key when `security` asks (twice; nothing is shown):")
    if subprocess.run(["security", "add-generic-password", "-U", "-s", SERVICE, "-a", account(provider), "-w"]).returncode:
        env.say("The key wasn't stored.")
        return 1
    try:
        c = env.connect()
        out = c.call("runtimes.check", {"runtime": provider})
        c.close()
    except (NotRunning, FerbError) as e:
        env.say(f"Stored. The daemon couldn't check it yet: {e}")
        return 1
    env.say(f"{provider} answered; use it with `ferb runtimes use ROLE {provider}`." if out.get("ok")
            else f"Stored, but the check failed: {out.get('detail')}")
    return 0 if out.get("ok") else 1


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
