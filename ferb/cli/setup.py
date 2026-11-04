"""Own explicit installation, permission setup and update workflows. Must not run app interactions as task recovery.

`ferb setup [--update]` (a checkout), `ferb setup --service [--update]` (FERB.app's setup window), `ferb doctor`,
`ferb uninstall [--purge]`.

Interactive but injectable (installer, daemon connection, prompts, System Settings opener, browser folders) so the
flows are testable without touching the real Mac. Permissions are always checked by the daemon, never by this process.
Setup never opens, quits or kills a browser and never writes a browser's Preferences. Run from FERB.app it never looks
at a browser's folder at all: FERB.app has no Full Disk Access, so FERB's background service connects the browsers
once it has it (DMN-507). A checkout's setup (run from the developer's terminal) writes, per browser, only FERB's own
files (the native host, the Web Store install request) and reads each profile's state (L25)."""

import json
import os
import shutil
import signal
import subprocess
import threading
import time
import uuid
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterator, TextIO

import ferb
from ferb.act.browser import autoinstall, profiles
from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.cli import tokenizer
from ferb.cli.admin import shown_bytes
from ferb.cli.client import Client
from ferb.config import Config, InsecureConfig
from ferb.core.errors import FerbError, NotRunning, Unsupported
from ferb.device import permissions
from ferb.device.install import (BUNDLE_CLI, MARKER, NATIVE_HOST_MANIFEST, SWITCHED_OFF_HOW, SWITCHED_OFF_STATUS,
                                 InstallError, Installer, ServiceSwitchedOff, app_bundle, code_hash,
                                 designated_requirement, repo_root, trusted_requirement)
from ferb.device.profile import ProfileStore, diff, summarize
from ferb.device.scan import scan
from ferb.device.persona import app_usage

Connect = Callable[[], Client]
Ask = Callable[[str], str]
SETUP_PERMISSIONS = permissions.DAEMON_PERMISSIONS


@dataclass
class Env:
    inst: Installer
    connect: Connect
    ask: Ask
    out: TextIO
    open_pane: Callable[[str], None] = permissions.open_pane
    sleep: Callable[[float], None] = time.sleep
    running_apps: Callable[[], set[str]] = lambda: _running_bundle_ids()
    persona_usage: Callable[[list[Any]], list[dict[str, Any]]] = app_usage
    browser_support: Path | None = None      # ~/Library/Application Support (the CLI's); None: no browser step
    run: Callable[..., Any] = subprocess.run  # FERB.app's build, signature and permission request (tests replace it)
    tokenizer: Callable[[Path], str] = tokenizer.ensure   # FERB's own pinned tokenizer copy (fix wave 3, 8a)

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


def installer(home: Path, support: Path, every_browser: bool = False, browser_folders: bool | None = None,
              **kwargs: Any) -> Installer:
    """The installer, accepting both the pinned unpacked id and the Web Store id. Run from FERB.app, it installs from
    that app and lists no browser folder: nothing here looks at another app's data before FERB's background service
    has Full Disk Access, which then writes the native host itself (DMN-507). A checkout's (or `browser_folders`, a
    developer's explicit command) sends FERB's native host to each installed browser; removing it (`every_browser`)
    goes to every known one."""
    kwargs.setdefault("bundle", app_bundle())
    if kwargs["bundle"] is None:                  # a checkout fetches the embedding model at install (FERB.app ships it)
        from ferb.memory.embeddings import download
        kwargs.setdefault("fetch_model", download)
    if browser_folders is None:
        browser_folders = every_browser or kwargs["bundle"] is None
    browsers = (list(profiles.BROWSERS) if every_browser else profiles.installed(support)) if browser_folders else []
    try:
        ids = autoinstall.accepted_extension_ids(Config(home))
    except InsecureConfig:                      # doctor reports it; setup and the daemon refuse such a config anyway
        ids = (FERB_EXTENSION_ID,)
    return Installer(home, native_hosts_dirs=autoinstall.native_host_dirs(support, browsers), extension_ids=ids,
                     **kwargs)


def extension_setup(env: Env) -> None:
    """Every installed Chromium-family browser: the Web Store install request once the store id is set, and each
    profile's state (installed and enabled from its Preferences, read only; paired from the daemon when it answers).
    The native host is registered by the installer. From FERB.app nothing here runs: FERB's background service connects
    the browsers once it has Full Disk Access (DMN-507)."""
    if env.browser_support is None:
        return
    if env.inst.bundle is not None:
        env.say("Browsers: FERB connects them once it has Full Disk Access (the setup tour's Pair browsers step).")
        return
    from ferb.device.extensions import extension_path
    try:
        c = env.connect()
        listed = c.call("browsers.list")["profiles"]
        c.close()
        paired: set[tuple[str, str]] | None = {(p["browser"], p["folder"]) for p in listed if p["paired"]}
    except (NotRunning, FerbError, OSError):      # no daemon to ask: pairing is reported as not checked
        paired = None
    config = Config(env.inst.home)
    try:
        store, ids = autoinstall.store_extension_id(config), autoinstall.accepted_extension_ids(config)
    except InsecureConfig as e:
        env.say(f"Browser extension: not set up, config.toml can't be trusted ({e}); `ferb doctor` shows the fix.")
        return
    # requests FERB wrote for another id (changed, cleared, mistyped) go before the current one is written
    withdrawn = autoinstall.withdraw(env.inst.home, store if autoinstall.valid_id(store) else "")
    states = autoinstall.prepare(env.browser_support, store, ids, paired, home=env.inst.home)
    for line in autoinstall.describe(states, store, extension_path(env.inst.home), withdrawn):
        env.say(line)


def device_id(config: Config) -> str:
    existing = config.get("device", "id")
    if existing:
        return str(existing)
    new = uuid.uuid4().hex
    config.set("device", "id", new)
    return new


# ---------- setup ----------

@contextmanager
def _finish_install() -> Iterator[list[int]]:
    """A stop (SIGTERM from FERB.app quitting, or Ctrl-C) while the background service is being (re)installed waits
    until it is loaded again, then stops setup: the daemon is never left unloaded halfway (M5). Yields the stops caught
    so far, so work done meanwhile (the store's clean-up) can end early."""
    if threading.current_thread() is not threading.main_thread():
        yield []
        return
    caught: list[int] = []
    previous = {sig: signal.signal(sig, lambda signum, frame: caught.append(signum))
                for sig in (signal.SIGTERM, signal.SIGINT)}
    try:
        yield caught
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
    if caught:
        raise KeyboardInterrupt


def _install(env: Env, update: bool) -> None:
    """Install, then say where FERB now runs when that changed (FERB.app and a checkout share one service label). The
    store's one-time clean-up is not part of an install: it waits for the owner's OK (`cleanup`)."""
    inst = env.inst
    before = inst.active_program()
    with _finish_install():
        inst.install(update_only=update)
    now = inst.active_program()
    if before and now and before != now:
        env.say(f"     The background service now runs from {now} (it ran from {before}).")
    for note in inst.notes:
        env.say(f"     {note}")
    _app_installed(env)


def _started(env: Env) -> Client:
    """The daemon on what was just installed, once it answers (the LaunchAgent was loaded or restarted by the
    install, DMN-513)."""
    return wait_for_daemon(env)


def _foreign_daemon(env: Env) -> str | None:
    """A FERB daemon answering outside launchd (`ferb daemon run` in a terminal) holds the socket, so setup can't start
    the installed one: says so, else None."""
    try:
        c = env.connect()
        pid = c.call("ping")["pid"]
        c.close()
    except NotRunning:
        return None
    if pid != env.inst.agent_pid():
        return (f"A FERB daemon (pid {pid}) is already running outside launchd — probably `ferb daemon run` "
                "in a terminal. Stop it (Ctrl-C) and run `ferb setup` again.")
    return None


def service_setup(env: Env, update: bool = False) -> int:
    """FERB.app's setup window (L34 C): install (or, with `update`, update) the background service from this app and
    wait until it answers, with no questions. Permissions, the AI service, browsers and the profile are the window's own
    steps, through the daemon's setup RPCs. An InstallError (a disk-image or translocated copy, a damaged app) reaches
    the caller with its fix. Switched off in Login Items, it says where to switch it on and exits SWITCHED_OFF_STATUS
    (FERB.app guides that one System Settings visit)."""
    inst = env.inst
    if not update and (refusal := _foreign_daemon(env)):
        env.say(refusal)
        return 1
    env.say("Updating FERB's background service…" if update else "Setting up FERB's background service…")
    env.say(env.tokenizer(inst.home))             # before the daemon starts: it counts tokens with it
    try:
        _install(env, update=update)
    except ServiceSwitchedOff as e:
        env.say(str(e))
        return SWITCHED_OFF_STATUS
    if not update:                                # the app list behind the Permissions step's Automation rows
        ProfileStore(inst.home).save(scan(device_id(Config(inst.home))), "setup scan")
    if inst.bundle is not None or (inst.home / "FERB.app").exists():
        _voice_helper(env)                        # pins FERB.app for hearing; never a prompt
    # DMN-513: the LaunchAgent is loaded by the install, so the command waits for the daemon itself (no launch
    # constraint from another build can stop it, as SMAppService's did)
    _started(env).close()
    extension_setup(env)
    if not update:
        _filevault_line(env)                      # DMN-502
    env.say("FERB's background service is running.")
    return 0


def setup(env: Env, update: bool = False) -> int:
    """`ferb setup [--update]`: a checkout's install (FERB.app runs `--service`); run from FERB.app's `ferb` link it
    sets up from that app."""
    inst = env.inst
    if update:
        env.say(f"Updating FERB from {inst.bundle} and restarting the daemon…" if inst.bundle else
                "Updating FERB in a new venv and restarting the daemon…")
        env.say(env.tokenizer(inst.home))          # before the restart: the new daemon counts tokens with it
        _install(env, update=True)
        if inst.bundle is not None or (inst.home / "FERB.app").exists():
            _voice_helper(env)        # L26: pins FERB.app for hearing; no permission dialog
        client = _started(env)
        _permissions_state(env, client)
        _review_persona(env, client)
        client.close()
        if codex_needed(inst.home):           # C2: an update never asks; it says how
            env.say(f"FERB's own Codex isn't signed in, so FERB can't answer yet: {CODEX_LOGIN_HOW}.")
        extension_setup(env)
        env.say("Done.")
        return 0

    if refusal := _foreign_daemon(env):           # a foreground dev daemon would hold the socket
        env.say(refusal)
        return 1

    if inst.bundle:
        env.say(f"1/4  Setting up FERB's background service from {inst.bundle}…")
    else:
        env.say("1/4  Installing FERB's dedicated Python, its venv and the LaunchAgent…")
    env.say("     " + env.tokenizer(inst.home))
    _install(env, update=False)
    client = _started(env)
    binary = inst.grant_target()
    env.say(f"     Daemon running. Permissions go to FERB ({binary}); macOS lists it as FERB." if inst.bundle else
            f"     Daemon running. Permissions must be granted to: {binary}")
    extension_setup(env)

    env.say("2/4  Scanning this Mac's apps (Info.plist only)…")
    config = Config(inst.home)
    profile = scan(device_id(config))
    store = ProfileStore(inst.home)
    store.save(profile, "setup scan")
    s = summarize(profile)
    env.say(f"     {s['apps']} apps, {s['scriptable']} scriptable; browsers: {', '.join(s['browsers']) or 'none'}")
    _filevault_line(env)                          # DMN-502

    env.say("3/4  Permissions (checked by the daemon itself)")
    if _voice_helper(env) is None:            # hearing's app (L26): built or pinned, never a permission dialog
        env.say("     Hearing isn't set up; `ferb doctor` will remind you.")
    _permissions_state(env, client)

    env.say("4/4  Self-test through the daemon")
    ok = _print_selftest(env, client.call("doctor.check"))
    _review_persona(env, client)
    client.close()
    ok &= _offer_codex_login(env)

    env.say("     FERB uses the installed action registry without per-action permission prompts.")
    env.say("     Connect the accounts/profiles you want available; macOS permissions and logins still apply.")
    env.say("Setup complete." if ok else "Setup finished with problems — `ferb doctor` shows what to fix.")
    return 0 if ok else 1


def _permissions_state(env: Env, client: Any) -> None:
    """The macOS permissions as the running daemon reports them, and the clipboard included. FERB's
    setup window asks for them (the CLI has no permission walkthrough). Setup is never recorded as done here
    (`permissions_version`): every row decides it, FERB.app's own Notifications and Microphone included, and only the
    window's Done (`setup.finish`) can read those."""
    report = client.call("doctor.check", {"selftest": False, "hearing": True})
    missing = permissions.missing(permissions.report_rows(report))
    if missing:
        env.say(f"     Not granted yet: {', '.join(f'{row}={status}' for row, status in missing)}. Grant them in "
                "FERB's setup window (Set Up FERB… in FERB's menu bar); `ferb doctor` lists them.")
        return
    env.say("     All of the daemon's macOS permissions are granted (checked by the running daemon); FERB's setup "
            "window checks FERB.app's own and records setup as done.")


# ---------- the store's one-time clean-up (STORE-402; step 12a) ----------

def _daemon_answers(env: Env) -> bool:
    try:
        c = env.connect()
        c.call("ping")
        c.close()
        return True
    except (NotRunning, FerbError, OSError):
        return False


def cleanup_preview(env: Env) -> int:
    """`ferb setup --cleanup`: what the one-time clean-up would remove, as one JSON object for FERB.app to show the
    owner (tables, row counts, what each held, whether the file still needs compacting, its size and the disk's free
    space, and `ok`, the digest the owner's OK carries). Read-only: a running daemon is left alone."""
    from ferb.store import cleanup
    path = env.inst.home / "ferb.db"
    if not path.exists():
        env.say(json.dumps({"steps": [], "rows": 0, "compact": False, "ok": None}))
        return 0
    env.say(json.dumps(cleanup.preview(path)))
    return 0


def cleanup_apply(env: Env, ok: str) -> int:
    """`ferb setup --cleanup --ok DIGEST`: the clean-up the owner agreed to, with the daemon stopped, then the daemon
    started again whatever happens. Refused, with nothing removed, when the plan read with the daemon stopped isn't
    the one the owner saw (exit 2, the new plan printed), and while a daemon still runs (exit 1). A stop (FERB.app
    quitting, Ctrl-C) ends it between batches; what was removed stays removed and the next run finishes. The last line
    is the result as JSON."""
    inst = env.inst
    if _daemon_answers(env) and not inst.loaded():
        env.say("The clean-up didn't run: a FERB daemon outside launchd answers (`ferb daemon run` in a terminal?). "
                "Stop it, then try again.")
        return 1
    result: dict[str, Any] = {}

    def work(stop: Callable[[], bool]) -> None:
        result.update(_run_cleanup(env, ok, stop))
    with _finish_install() as stops:               # a stop waits until the daemon is loaded again, then ends setup
        if inst.loaded() or inst.labelled_plist():
            try:
                inst.while_stopped(lambda: work(lambda: bool(stops)))
            except InstallError as e:                # the stopped process hasn't exited: nothing ran
                env.say(f"The clean-up didn't run: {e}. Try again in a moment.")
                result["not_run"] = str(e)
        else:                                        # no service installed or registered: nothing to stop
            work(lambda: bool(stops))
        env.say(json.dumps(result))
    return 2 if result.get("refused") else 1 if result.get("error") or result.get("not_run") else 0


def _run_cleanup(env: Env, ok: str, stop: Callable[[], bool]) -> dict[str, Any]:
    """The clean-up on the stopped daemon's store: never while launchd still has the agent, while the process it
    stopped hasn't exited (`Installer.exiting`), or while a daemon answers. An error is reported, never raised: the
    daemon is started again either way."""
    from ferb.clock.clock import SystemClock
    from ferb.core.ids import RandomIds
    from ferb.memory.journal import Journal
    from ferb.sanitize.redact import Redactor
    from ferb.store import cleanup
    from ferb.store.db import Database
    inst = env.inst
    loaded = inst.loaded()
    exiting = None if loaded else inst.exiting()
    running = ("launchd hasn't stopped FERB's daemon yet" if loaded else
               f"FERB's daemon (process {exiting}) hasn't exited since launchd stopped it" if exiting is not None else
               "a FERB daemon still answers" if _daemon_answers(env) else "")
    if running:
        env.say(f"The clean-up didn't run: {running}. Try again in a moment.")
        return {"not_run": running}
    db = None
    try:
        db = Database(inst.home / "ferb.db")
        db.migrate()
        journal = Journal(db, SystemClock(), RandomIds(), Redactor())
        out = cleanup.apply(db, journal, ok, stop=stop, say=env.say)
    except Exception as e:
        env.say(f"The clean-up stopped ({type(e).__name__}: {e}); what it removed stays removed, and running it "
                "again finishes it.")
        return {"error": f"{type(e).__name__}: {e}"}
    finally:
        if db is not None:
            db.close()
    if out.get("refused"):
        env.say("The clean-up didn't run: what it would remove changed since you saw it. Look at it again.")
    elif out["stopped"]:
        env.say(f"The clean-up stopped before it finished ({sum(out['removed'].values()):,} rows removed so far); "
                "running it again finishes it.")
    elif out.get("compacted") is False and "needed" in out:
        env.say(f"Removed {sum(out['removed'].values()):,} rows. Not compacted: that needs "
                f"{shown_bytes(out['needed'])} free and the disk has {shown_bytes(out['free'])}; FERB reuses the freed "
                "space inside the file, and running the clean-up again compacts it.")
    else:
        env.say(f"Removed {sum(out['removed'].values()):,} rows" + (
            f"; FERB's database: {shown_bytes(out['bytes_before'])} → {shown_bytes(out['bytes_after'])}."
            if out.get("compacted") else "."))
    return out


def _app_installed(env: Env) -> None:
    """After installing from FERB.app: say where `ferb` is. Hearing is pinned to this app's own signature by
    `_voice_helper` (an update replaces it, and an ad-hoc signature pins the exact code)."""
    inst = env.inst
    if inst.bundle is None:
        return
    status = inst.cli_status()
    if status:
        env.say(f"     {status}")


def _review_persona(env: Env, client: Any, *, refresh: bool = False) -> int:
    """What FERB saw on this Mac (MEM-13): started, waited for, and shown once; nothing to answer (MEM-12)."""
    from ferb.sanitize.terminal import printable
    result = client.call('setup.persona', {'refresh': refresh})
    if result['state'] == 'scanning':
        env.say('Getting to know this Mac from local app-use metadata…')
    while result['state'] == 'scanning':
        env.sleep(0.1)
        result = client.call('setup.persona', {'status': True})
    if result['state'] == 'failed':
        env.say('App-use scan failed: ' + printable(result.get('error', 'unknown error')))
        env.say("FERB's setup window can look again (Set Up FERB… in FERB's menu bar).")
        return 1
    if result['state'] == 'seen' and result.get('summary'):
        env.say(printable(result['summary']))
    return 0


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


CODEX_LOGIN_HOW = "choose Sign in to Codex… in FERB's menu bar, or run `ferb setup --codex-login`"


def context_window_check(role: str, lim: dict[str, Any]) -> Check:
    """`ferb doctor`'s line for one role's documented limits (L28: the daemon status `limits` entry), or why not."""
    if "problem" in lim:
        return Check(f"context window {role}", "fail", f"{lim.get('runtime')} {lim.get('model')}: {lim['problem']}",
                     "set a documented model in config.toml [models]")
    cap = f", at most {lim['max_input_chars']} characters in one input" if lim.get("max_input_chars") else ""
    return Check(f"context window {role}", "ok", f"{lim.get('runtime')} {lim.get('model')}: "
                 f"{lim['context_window']} tokens, {lim['max_output']} for the reply{cap}")


def codex_needed(home: Path) -> bool:
    """FERB's runtimes use Codex ([runtimes] orchestrator/worker/checker, Codex by default) and FERB's own Codex home
    has no sign-in. The daemon never falls back to the owner's personal ~/.codex (owner decision, 2026-10-01)."""
    from ferb.runtimes.codex_exec import codex_home
    config = Config(home)
    worker = str(config.get("runtimes", "worker", "codex"))
    roles = {str(config.get("runtimes", "orchestrator", "codex")), worker,
             str(config.get("runtimes", "checker", worker))}
    return "codex" in roles and codex_home(home) is None


def _offer_codex_login(env: Env) -> bool:
    """C2: setup (the packaged app's first-run console included) offers FERB's own Codex sign-in when it's missing;
    without it FERB can't answer anything. `codex login` opens the browser itself and reads no terminal input."""
    if not codex_needed(env.inst.home):
        return True
    answer = env.ask("     FERB answers through Codex, which isn't signed in for FERB. "
                     "Sign in to ChatGPT for FERB now? [Y/n] ").strip().lower()
    if answer in ("", "y", "yes") and codex_login(env, run=env.run) == 0:
        return True
    env.say(f"     FERB can't answer until its Codex is signed in: {CODEX_LOGIN_HOW}.")
    return False


def claude_logout(env: Env, run: Callable[..., Any] = subprocess.run) -> int:
    """Sign out FERB's own Claude: Claude Code's `auth logout` with FERB's Claude home only (the owner's personal Claude
    Code login is never touched), then the sign-in marker goes once Claude Code no longer reports a login there, so
    FERB stops offering Claude instead of failing every call."""
    from ferb.runtimes import claude_login as cl
    if cl.claude_home(env.inst.home) is None:
        env.say("FERB's Claude isn't signed in; nothing to sign out of.")
        return 0
    try:
        cli = cl.cli_binary()
    except Exception as e:
        env.say(f"FERB's pinned Claude Code isn't installed: {e}")
        return 1
    run(cl.logout_argv(cli), env=cl.env(env.inst.home))
    status = run(cl.status_argv(cli), env=cl.env(env.inst.home), capture_output=True, text=True)
    if cl.signed_in(status.stdout or ""):
        env.say("Claude didn't sign out; nothing changed.")
        return 1
    cl.unmark(env.inst.home)
    env.say("Signed out: FERB can't use Claude until you sign in again.")
    return 0


def codex_logout(env: Env, run: Callable[..., Any] = subprocess.run) -> int:
    """Settings → AI → Sign out of ChatGPT (L34 Phase E): `codex logout` in FERB's own Codex home only; the owner's
    personal Codex login is never touched."""
    import os
    from ferb.runtimes.codex_exec import codex_binary
    home = env.inst.home / "codex-home"
    if not (home / "auth.json").exists():
        env.say("FERB's Codex isn't signed in; nothing to sign out of.")
        return 0
    try:
        binary = codex_binary(env.inst.home)
    except Exception as e:
        env.say(f"FERB's pinned Codex isn't installed: {e}")
        return 1
    rc = run([str(binary), "logout"], env={**os.environ, "CODEX_HOME": str(home)}).returncode
    gone = not (home / "auth.json").exists()
    env.say("Signed out: FERB can't answer through Codex until you sign in again." if rc == 0 and gone else
            "Codex didn't sign out; nothing changed.")
    return 0 if rc == 0 and gone else 1


def cli_setup(env: Env) -> int:
    """Settings → Advanced → Install command-line tool (L34 Phase E): `ferb` on PATH as a link to this FERB.app's
    launcher (Installer.link_cli leaves anything else called `ferb` alone). Setup never edits shell profiles."""
    inst = env.inst
    if inst.bundle is None:
        env.say("The command-line tool is linked from FERB.app; a checkout runs `uv run ferb`.")
        return 1
    inst.link_cli()
    link = inst.cli_link
    ok = link is not None and link.is_symlink() and link.resolve() == (inst.bundle / BUNDLE_CLI).resolve()
    env.say(inst.cli_status() or "")
    if ok:
        env.say('To use it in a terminal, add  export PATH="$HOME/.local/bin:$PATH"  to ~/.zprofile.')
    return 0 if ok else 1


def codex_login(env: Env, run: Callable[..., Any] = subprocess.run) -> int:
    """Sign in FERB's own Codex home (owner decision, 2026-10-01): the pinned Codex, with CODEX_HOME set to
    ~/.ferb/codex-home, so FERB's runs never load the owner's personal Codex settings or skills."""
    import os
    from ferb.runtimes.codex_exec import codex_binary
    home = env.inst.home / "codex-home"
    home.mkdir(parents=True, exist_ok=True)
    os.chmod(home, 0o700)
    try:
        try:
            binary = codex_binary(env.inst.home)
        except Unsupported:                       # no role used Codex until now: installed when the owner signs in
            env.inst.ensure_codex()
            binary = codex_binary(env.inst.home)
    except (Unsupported, InstallError) as e:
        env.say(f"FERB's pinned Codex couldn't be installed: {e}")
        return 1
    env.say("Signing in FERB's own Codex (a browser window opens)…")
    rc = run([str(binary), "login"], env={**os.environ, "CODEX_HOME": str(home)}).returncode
    ok = (home / "auth.json").exists()
    if ok:          # L28: Codex's own model catalog, so FERB knows each model's window before the first run
        from ferb.runtimes.codex_exec import bundled_catalog
        if bundled_catalog(env.inst.home) is None:
            env.say("Codex didn't print its model catalog; FERB reads it again when its background service restarts.")
    env.say("Done: FERB now uses its own Codex home." if rc == 0 and ok else "Not signed in; nothing changed.")
    return 0 if ok else 1


PASTE_PANE = "System Settings → Privacy & Security → Paste from Other Apps"


def clipboard_check(state: str | None) -> Check | None:
    """I3: whether macOS lets FERB's daemon read the clipboard without asking. FERB never reads it otherwise (no
    surprise alert), so clipboard capture is off until the owner sets FERB to Allow."""
    if state is None:
        return None
    if state in ("allow", "unrestricted"):
        return Check("clipboard", "ok", "FERB may read what you copy (kept on this Mac for 7 days)")
    why = {"default": "macOS hasn't asked you yet", "ask": "macOS would ask you every time",
           "deny": "you set FERB to Deny"}.get(state, f"macOS reports {state}")
    fix = (f"let macOS ask once from FERB's setup window, then set FERB to Allow in {PASTE_PANE}" if state == "default"
           else f"set FERB to Allow in {PASTE_PANE}")
    return Check("clipboard", "warn", f"FERB doesn't record what you copy: {why}", fix)


def _first_line(run: Callable[..., Any], argv: list[str]) -> str | None:
    """The first output line of a status command that needs no root (csrutil, fdesetup); None when it can't run."""
    try:
        done = run(argv, capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    lines = (done.stdout or "").strip().splitlines()
    return lines[0].strip() if done.returncode == 0 and lines else None


def sip_check(run: Callable[..., Any]) -> Check:
    """DMN-506: System Integrity Protection off (or partly off, a custom configuration) is a problem, with what it
    exposes. `csrutil status` needs no root."""
    line = _first_line(run, ["/usr/bin/csrutil", "status"])
    if line is not None and line.lower().endswith("status: enabled."):
        return Check("system integrity protection", "ok", "on")
    what = "couldn't be read" if line is None else "is off" if "disabled" in line.lower() else "is partly off"
    return Check("system integrity protection", "warn",
                 f"System Integrity Protection {what}: any process can then read a running command's proxy credential "
                 "from its environment, and no sandbox rule stops it",
                 "turn it back on from macOS Recovery (`csrutil enable`)")


def filevault_check(run: Callable[..., Any]) -> Check:
    """DMN-502: FileVault encrypts the disk FERB's database, traces and screenshots live on; FERB adds no encryption of
    its own. `fdesetup status` needs no root."""
    line = _first_line(run, ["/usr/bin/fdesetup", "status"])
    if line is not None and line.startswith("FileVault is On"):
        return Check("filevault", "ok", "on")
    what = "couldn't be read" if line is None else "is off"
    return Check("filevault", "warn", f"FileVault {what}: what FERB keeps in ~/.ferb (its database, traces and "
                 "screenshots) sits unencrypted on this Mac's disk", "turn it on in System Settings → Privacy & "
                 "Security → FileVault")


def _filevault_line(env: Env) -> None:
    """Setup's FileVault check (DMN-502): one line when it isn't on; nothing otherwise."""
    check = filevault_check(env.run)
    if check.status != "ok":
        env.say(f"     {check.detail}; {check.hint}.")


def protection_check(home: Path) -> Check:
    """[protection] and FERB's workspace as the daemon reads them at start (rule 10): when this fails, FERB doesn't
    start, and this says why."""
    from ferb.policy import protection
    try:
        p = protection.load(Config(home).get, Path.home(), home)
    except FerbError as e:
        return Check("protection", "fail", str(e), "fix [protection] in ~/.ferb/config.toml (or the workspace), then "
                     "`ferb setup --update`")
    return Check("protection", "ok", f"{len(p.reads)} never-read and {len(p.writes)} never-write entries; FERB's "
                 "shell and file tools follow both (a task asks you before it opens one)")


def database_check(home: Path) -> Check | None:
    """The store's size on disk, and whether its one-time clean-up waits for the owner's OK (STORE-402)."""
    from ferb.store import cleanup
    from ferb.store.db import store_size
    path = home / "ferb.db"
    if not path.exists():
        return None
    size = shown_bytes(store_size(path))
    if cleanup.pending(path):
        return Check("database", "warn", f"{size}; a one-time clean-up of data FERB no longer uses waits for your OK",
                     "FERB's app shows what it removes (`ferb setup --cleanup` lists it)")
    return Check("database", "ok", size)


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


def local_reports_check(c: dict[str, Any] | None) -> Check | None:
    """The kernel's reports of direct connections to owner-pinned local ports (L24), when there are pinned ports: a
    stopped stream leaves those connections unrecorded (null) until the daemon restarts."""
    status = (c or {}).get("local_reports")
    if status is None:
        return None
    if status.get("running"):
        return Check("local connections", "ok", "direct connections to pinned local ports are recorded from the "
                     "kernel's reports")
    why = status.get("stopped") or "not started"
    return Check("local connections", "warn", f"the kernel's reports stopped ({why}): direct connections to pinned "
                 "local ports are unrecorded (null) until FERB restarts",
                 "restart the daemon (`ferb setup --update`); reading the system log may need an admin account")


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


def add_hearing_check(add: Callable[[Check], None], hearing: dict[str, Any] | None) -> None:
    """L26: FERB.app's Speech Recognition, as the daemon asked FERB.app itself (no prompt)."""
    if not hearing:
        return
    fix = "run `ferb setup`"
    if not hearing.get("installed"):
        add(Check("hearing", "warn", "FERB.app isn't installed, so FERB can't turn sound into text", fix))
    elif "signed" not in hearing:                    # a quick check: FERB.app wasn't asked
        return
    elif not hearing.get("signed"):
        add(Check("hearing", "warn", "FERB.app's signature isn't the one recorded at setup", fix))
    elif hearing.get("error"):
        add(Check("hearing", "warn", f"FERB.app didn't answer: {hearing['error']}", fix))
    elif hearing.get("speech") != "granted":
        add(Check("hearing", "warn", f"FERB.app's Speech Recognition: {hearing.get('speech')}",
                  fix + " and allow Speech Recognition for FERB"))
    elif not hearing.get("on_device"):
        add(Check("hearing", "warn", f"on-device speech recognition isn't available for {hearing.get('locale')}; FERB "
                  "never uses a cloud recognizer", "on-device support follows the Mac's Dictation languages (System Settings → "
                  "Keyboard → Dictation)"))
    else:
        add(Check("hearing", "ok", f"FERB.app may use on-device Speech Recognition ({hearing.get('locale')})"))


def service_check(inst: Installer) -> Check:
    """The background service's LaunchAgent (DMN-513), exactly as setup writes it from this FERB.app or checkout, and
    for FERB.app the build setup last installed."""
    fix = "open FERB.app: it sets its background service up again" if inst.bundle else "run `ferb setup --update`"
    if not inst.plist_path.exists():
        return Check("LaunchAgent", "fail", f"{inst.plist_path} is missing", fix)
    if not inst.definition_current():
        return Check("LaunchAgent", "fail", f"{inst.plist_path} differs from what setup writes", fix)
    built = str(inst.bundle_info().get("CFBundleVersion", ""))
    if inst.bundle and inst.recorded().get("build") != built:
        return Check("LaunchAgent", "warn", f"FERB.app build {built} isn't the one set up", fix)
    return Check("LaunchAgent", "ok", str(inst.plist_path))


def doctor(env: Env) -> int:
    inst = env.inst
    checks: list[Check] = []
    add = checks.append
    add(Check("home", "ok" if (inst.home / MARKER).exists() else "fail", str(inst.home), "run `ferb setup`"))
    own = (inst.home / "codex-home" / "auth.json").exists()
    if own:
        add(Check("codex home", "ok", "FERB's own (~/.ferb/codex-home)"))
    elif codex_needed(inst.home):          # C2: the daemon refuses to run Codex without it
        add(Check("codex home", "fail", "FERB's own Codex isn't signed in, so FERB can't answer (it never uses "
                  "your personal Codex)", CODEX_LOGIN_HOW))
    else:
        add(Check("codex home", "ok", "not signed in; FERB's runtimes don't use Codex"))
    try:
        Config(inst.home).load()
        add(Check("config permissions", "ok", "owner-only"))
    except InsecureConfig as e:
        add(Check("config permissions", "fail", str(e), "chmod 600 ~/.ferb/config.toml; chmod 700 ~/.ferb"))
    add(protection_check(inst.home))
    add(sip_check(env.run))                       # DMN-506
    add(filevault_check(env.run))                 # DMN-502
    if (database := database_check(inst.home)) is not None:
        add(database)
    if inst.bundle:
        add(Check("app", "ok", str(inst.bundle)))
        add(Check("interpreter", "ok" if inst.daemon_python.is_file() else "fail", str(inst.daemon_python),
                  "download FERB again"))
    else:
        add(Check("interpreter", "ok" if inst.interpreter() else "fail", str(inst.interpreter() or "missing"),
                  "run `ferb setup`"))
        add(Check("venv", "ok" if inst.venv_python.exists() else "fail", str(inst.venv), "run `ferb setup`"))
    add(service_check(inst))
    off = inst.disabled()
    add(Check("background service", "ok" if inst.loaded() and not off else "fail",
              f"{inst.label} is switched off in Login Items" if off else inst.label,
              SWITCHED_OFF_HOW if off else "open FERB.app" if inst.bundle else "run `ferb setup`"))
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
        # only a checkout's own installed daemon is compared with the checkout (FERB.app's code is the app's build)
        if (repo_root() / "pyproject.toml").exists() and not inst.bundle and report["code_hash"] != here:
            add(Check("installed code", "warn", "differs from this checkout", "run `ferb setup --update`"))
        ready = permissions.fda_ready(report.get("containment") or {})
        for perm, status in report["permissions"].items():     # every permission is needed (none optional)
            waits = perm == "full_disk_access" and status != "granted" and not ready
            add(Check(f"permission {perm}", "ok" if status == "granted" else "fail",
                      f"{status} ({permissions.FDA_WAITS})" if waits else status,
                      "" if status == "granted" else "fix containment first, then grant it" if waits else
                      f"grant it to {dedicated} (System Settings → Privacy & Security)"))
        for r in report["selftest"]:
            add(Check(f"self-test {r['name']}", "ok" if r["ok"] else "fail", r["detail"]))
        add(containment_check(report.get("containment")))
        if (local_reports := local_reports_check(report.get("containment"))) is not None:
            add(local_reports)
        add(snapshot_check(report.get("containment")))
        add_hearing_check(add, report.get("hearing"))
        if (clipboard := clipboard_check(report.get("clipboard"))) is not None:
            add(clipboard)
        if "tokenizer" in report:                  # how the daemon counts tokens (its budgets depend on it)
            exact = str(report["tokenizer"]).startswith("o200k")
            add(Check("tokenizer", "ok" if exact else "warn",
                      "o200k_base, pinned copy verified" if exact else
                      "missing: token counts (the planner's budget) use a byte ceiling",
                      "" if exact else "run `ferb setup --update`"))
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
        for role, lim in rs.get("limits", {}).items():          # L28: the model's documented window, or why not
            add(context_window_check(role, lim))
        if rs:
            add(Check("roles", "ok", f"supervisor {rs.get('orchestrator')}, worker {rs.get('worker')}"))
        try:
            c = env.connect()
            ident = c.call("identity.check", {"session": False})
            c.close()
        except (NotRunning, FerbError) as e:
            ident = None
            add(Check("identity session", "warn", f"couldn't check: {e}", "run `ferb doctor` again"))
        if ident:
            if not ident["profile"]:
                add(Check("identity profile", "warn", "not set up",
                          "FERB's own account is set up in FERB.app → Settings → Browsers → FERB's own account"))
            else:
                add(Check("identity profile", "ok" if ident["profile_ok"] else "fail",
                          f"{ident.get('profile_name')} ({ident['profile']}) holds only {ident['address']}"
                          if ident["profile_ok"] else str(ident["error"]),
                          "FERB's Chrome profile must hold only FERB's account; fix it in Chrome"))
                if ident["profile_ok"]:
                    paired = bool(ident.get("paired"))
                    add(Check("identity session", "warn",
                              "session not checked (observational doctor; no browser opened)" if paired else
                              "not paired; session not checked",
                              "" if paired else "pair it from FERB.app"))
            for s in ident["secrets"]:
                add(Check(f"secret {s['key']}", "ok" if s["in_keychain"] else "warn",
                          "in Keychain" if s["in_keychain"] else "missing from Keychain",
                          f"in your own terminal: security add-generic-password -s ferb -a {s['key']} -w"))
        # FERB.app's installer lists no browser folder (its service writes them, DMN-507); doctor, a developer's
        # command run in their terminal, looks at each installed browser's
        hosts_dirs = inst.native_hosts_dirs or (autoinstall.native_host_dirs(
            env.browser_support, autoinstall.profiles.installed(env.browser_support)) if env.browser_support is not None
            else ())
        for hosts_dir in hosts_dirs:
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
            add(Check("emergency stop", "warn", "engaged", "FERB.app's menu lifts it"))
        for day in (report.get("learning") or {}).get("undistilled", []):      # MEM-421
            add(Check(f"learning {day['day']}", "warn",
                      f"not learned from yet ({day['attempts']} tries): {day['error']}",
                      "FERB tries that day again every hour; sign in to its AI service if it says signed out"))
        for card in (report.get("learning") or {}).get("stuck_cards", []):     # MEM-422
            add(Check(f"episode card {card['task_id']}", "warn", f"couldn't be built twice: {card['error']}",
                      "FERB tries it again when that task changes (a new verdict or a rebuilt episode)"))
        try:
            c = env.connect()
            sense = c.call("sense.status")
            c.close()
        except (NotRunning, FerbError):
            sense = None
        if sense is not None:
            if sense["paused"]:
                add(Check("watchers", "warn", "paused: nothing is recorded, no real mouse / keyboard input",
                          "FERB.app's menu resumes them"))
            else:
                add(Check("watchers", "ok", ", ".join(sense["watchers"]) or "none running"))
                if sense.get("owner_input_why"):          # SENSE-406
                    add(Check("owner input", "warn", f"{sense['owner_input_why']}: FERB can't tell whether you're "
                              "using the Mac, so it makes no typing or clicking in front of you",
                              "turn FERB on in Settings → Privacy & Security → Input Monitoring; FERB starts "
                              "watching by itself within a minute"))
            try:
                c = env.connect()
                jev = c.call("jev.stats")
                c.close()
            except (NotRunning, FerbError):
                jev = None
            if jev is not None and jev.get("configured"):
                if jev["recent_total"] and jev["recent_answered"] == 0:
                    add(Check("jev", "warn", f"not answering ({jev['last_error']}); FERB uses its own decisions",
                              "check the Jev key in the Keychain"))
                else:
                    on = [s["question"] for s in jev["stats"] if s["enabled"]]
                    add(Check("jev", "ok", f"shadow mode{'; enabled: ' + ', '.join(on) if on else ''}"))
            elif jev is not None:
                from ferb.decide.decider import JEV_KEYS_URL
                add(Check("jev", "ok", "off: activity notices are paused (SENSE-2); a key comes "
                                       f"from {JEV_KEYS_URL}"))
            for source, why in sense.get("failed", {}).items():
                add(Check(f"watcher {source}", "warn", f"failing, trying again: {why}",
                          "FERB retries it by itself; if it keeps failing, check its permission in Settings"))
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


# ---------- uninstall ----------

def uninstall(env: Env, purge: bool, confirmed: bool = False) -> int:
    """Remove FERB's background service, browser connection and `ferb` link; `purge` (after the owner types
    'delete', or `confirmed`: they said yes to FERB.app's one question, APP-2) also deletes FERB's home and
    every FERB Keychain item (API keys, the Jev key, FERB's own secrets, the owner's saved secrets and cards, FERB's
    Claude sign-in). FERB.app first unregisters any SMAppService registration a 2026-11 build left (DMN-513); this
    boots the LaunchAgent out and deletes it. FERB's own shell can't run this to any effect: its
    sandbox denies FERB's home, the Keychain service and launchd (ferb/act/shell/sandbox.py)."""
    inst = env.inst
    if purge:
        if not (inst.home / MARKER).exists():
            env.say(f"Refusing to purge {inst.home}: it has no {MARKER} marker.")
            return 1
        if not confirmed and env.ask(f"This deletes {inst.home} entirely (database, journal, config, FERB's Chrome profile) and every "
                   "FERB item in your Keychain (API keys, saved secrets and cards, FERB's sign-ins). "
                   "Type 'delete' to confirm: ").strip() != "delete":
            env.say("Not purged.")
            purge = False
    binary = inst.granted_binary()
    store = ""
    if env.browser_support is not None:
        try:
            store = autoinstall.store_extension_id(Config(inst.home))
        except InsecureConfig:                  # the record in FERB's home still lists every request FERB wrote
            store = ""
    requests = autoinstall.remove_requests(inst.home, env.browser_support, store) \
        if env.browser_support is not None else []
    for path in requests:                       # reported before anything else can fail
        env.say(f"removed {path}")
    try:
        removed = inst.uninstall(purge=purge)
    except InstallError as e:
        env.say(str(e))
        return 1
    for r in removed:
        env.say(f"removed {r}")
    if not removed and not requests:
        env.say("Nothing to remove.")
    if binary:
        what = f"FERB ({inst.grant_target()})" if inst.bundle else str(binary)
        env.say(f"macOS can't remove permissions for you: in System Settings → Privacy & Security, remove "
                f"{what} from Accessibility, Screen Recording (and Input Monitoring / Automation if granted).")
    return 0


CERTIFICATE = "FERB Local Code Signing"


def _build_app(env: Env, run: Callable[..., Any]) -> str | None:
    """From a checkout: build the notch app (ui/mac/FERBNotch/build.sh), copy it to ~/.ferb/FERB.app and sign it with the
    local certificate (so its permissions survive rebuilds). Returns its signature requirement, or None (said why)."""
    # not `-v`: that lists only certificates macOS trusts, and a self-made one never is (it still signs fine)
    found = run(["security", "find-identity", "-p", "codesigning"], capture_output=True, text=True)
    if CERTIFICATE not in (found.stdout or ""):
        env.say(f"The signing certificate \"{CERTIFICATE}\" isn't in your login keychain. Create it in Keychain Access →")
        env.say("Certificate Assistant → Create a Certificate (name it exactly that, type Code Signing), then run this again.")
        return None
    app = env.inst.home / "FERB.app"
    env.say("Building FERB.app (the notch app, which also turns an app's sound into text for hearing)…")
    notch = repo_root() / "ui" / "mac" / "FERBNotch"
    built = run([str(notch / "build.sh"), "release"], capture_output=True, text=True)
    if built.returncode != 0:
        env.say("Build failed:\n" + ((built.stderr or built.stdout or "").strip()[-1500:]))
        return None
    staged = app.with_name(".FERB.app.new")     # the app in use stays until the copy is built and signed
    shutil.rmtree(staged, ignore_errors=True)
    copied = run(["/usr/bin/ditto", str(notch / "build" / "FERB Notch.app"), str(staged)], capture_output=True, text=True)
    if copied.returncode != 0:
        env.say("Copying the build failed:\n" + ((copied.stderr or copied.stdout or "").strip()[-1500:]))
        shutil.rmtree(staged, ignore_errors=True)
        return None
    # build.sh signs ad hoc and not hardened; a hardened build would need `--options runtime` and the entitlements here
    signed = run(["/usr/bin/codesign", "--force", "--deep", "--sign", CERTIFICATE, str(staged)],
                 capture_output=True, text=True)
    if signed.returncode != 0:
        env.say("Signing failed:\n" + ((signed.stderr or signed.stdout or "").strip()[-1500:]))
        shutil.rmtree(staged, ignore_errors=True)
        return None
    shutil.rmtree(app, ignore_errors=True)
    os.replace(staged, app)
    shown = run(["/usr/bin/codesign", "-d", "-r-", str(app)], capture_output=True, text=True)
    req = designated_requirement((shown.stdout or "") + (shown.stderr or ""))
    if not trusted_requirement(req, pinned_ok=False):
        env.say("FERB.app isn't signed with the local certificate, so FERB won't use it.")
        return None
    return req


def _shipped_app(env: Env) -> str | None:
    """From the packaged FERB.app: hearing runs the app itself, whose verified signature is pinned as it is (nothing is
    built and no local certificate is needed). Returns its signature requirement, or None (said why)."""
    try:                                      # check_bundle verifies the app's own signature first
        env.inst.check_bundle()
        return env.inst.own_requirement()
    except InstallError as e:
        env.say(f"FERB won't use FERB.app for hearing: {e}")
        return None


def _voice_helper(env: Env, run: Callable[..., Any] | None = None) -> str | None:
    """The app hearing runs (L26): built from a checkout, or the packaged FERB.app itself. Records its signature for the
    daemon's checks (an older pin, which named the old helper's bundle id com.ferb.app, is replaced here, and `[voice] requirement` from
    an older push-to-talk setup follows it). Returns the requirement, or None (said why)."""
    req = _shipped_app(env) if env.inst.bundle is not None else _build_app(env, run or env.run)
    if req is None:
        return None
    config = Config(env.inst.home)
    config.set("hearing", "requirement", req)
    if config.get("voice", "requirement") not in (None, req):   # push-to-talk checks the same app's signature
        config.set("voice", "requirement", req)
    return req
