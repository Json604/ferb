"""Own explicit installation, permission setup and update workflows. Must not run app interactions as task recovery.

`ferb setup [--update]`, `ferb doctor`, `ferb uninstall [--purge]`.

Interactive but injectable (installer, daemon connection, prompts, System Settings opener, browser folders) so the
flows are testable without touching the real Mac. Permissions are always checked by the daemon, never by this process.
Setup never opens, quits or kills a browser and never writes a browser's Preferences: per browser it writes only
FERB's own files (the native host, the Web Store install request) and reads each profile's state (L25)."""

import signal
import sqlite3
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
from ferb.core.errors import FerbError, NotRunning
from ferb.device import permissions
from ferb.device.install import (MARKER, NATIVE_HOST_MANIFEST, InstallError, Installer, app_bundle, code_hash,
                                 designated_requirement, repo_root, trusted_requirement, voice_app)
from ferb.device.profile import ProfileStore, diff, summarize
from ferb.device.scan import scan
from ferb.device.persona import app_usage, setup_persona
from ferb.identity import seed

Connect = Callable[[], Client]
Ask = Callable[[str], str]
REQUIRED_PERMISSIONS = ("accessibility", "screen_recording", "full_disk_access")
SETUP_PERMISSIONS = (*REQUIRED_PERMISSIONS, 'input_monitoring')
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


def installer(home: Path, support: Path, every_browser: bool = False, **kwargs: Any) -> Installer:
    """The installer with FERB's native host going to each installed browser (every known one when removing it),
    accepting both the pinned unpacked id and the Web Store id. Run from FERB.app, it installs from that app."""
    kwargs.setdefault("bundle", app_bundle())
    browsers = list(profiles.BROWSERS) if every_browser else profiles.installed(support)
    try:
        ids = autoinstall.accepted_extension_ids(Config(home))
    except InsecureConfig:                      # doctor reports it; setup and the daemon refuse such a config anyway
        ids = (FERB_EXTENSION_ID,)
    return Installer(home, native_hosts_dirs=autoinstall.native_host_dirs(support, browsers), extension_ids=ids,
                     **kwargs)


def extension_setup(env: Env) -> None:
    """Every installed Chromium-family browser: the Web Store install request once the store id is set, and each
    profile's state (installed and enabled from its Preferences, read only; paired from the daemon when it answers).
    The native host is registered by the installer."""
    if env.browser_support is None:
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
    so far, so work done meanwhile (the store clean-out) can end early."""
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
    """Install, then say where FERB now runs when that changed (FERB.app and a checkout share one LaunchAgent). A store
    that still needs its one-time clean-out (asked read-only, while the old daemon runs) gets it between stopping the
    old daemon and starting the new one (`_clean_store`)."""
    inst = env.inst
    before = inst.active_program()
    state = cleanout_state(inst.home / "ferb.db")
    with _finish_install() as stops:
        inst.install(update_only=update,
                     while_stopped=(lambda: _clean_store(env, lambda: bool(stops), state)) if state else None)
    now = inst.active_program()
    if before and now and before != now:
        env.say(f"     The background service now runs from {now} (it ran from {before}).")
    _app_installed(env)


def setup(env: Env, update: bool = False) -> int:
    inst = env.inst
    if update:
        env.say(f"Updating FERB from {inst.bundle} and restarting the daemon…" if inst.bundle else
                "Updating FERB in its venv and restarting the daemon…")
        env.say(env.tokenizer(inst.home))          # before the restart: the new daemon counts tokens with it
        _install(env, update=True)
        client = wait_for_daemon(env)
        config = Config(inst.home)
        health = client.call("doctor.check", {"selftest": False})
        grants = health.get("permissions", {})
        missing = [f"{name}={grants.get(name, 'unknown')}" for name in SETUP_PERMISSIONS
                   if grants.get(name) != "granted"]
        if missing:
            again = "Run `ferb setup`" + (" (or Set Up FERB… in FERB's menu bar)" if inst.bundle else "")
            env.say(f"Permission setup deferred: {', '.join(missing)}. {again} when you want its permission dialogs.")
        if not missing:
            config.set('setup', 'permissions_version', max(27, config.get('setup', 'permissions_version', 0)))
            env.say("All macOS permissions are granted (checked by the running daemon).")
        _review_persona(env, client)
        client.close()
        if codex_needed(inst.home):           # C2: an update never asks; it says how
            env.say(f"FERB's own Codex isn't signed in, so FERB can't answer yet: {CODEX_LOGIN_HOW}.")
        if inst.bundle is not None or (inst.home / "FERB.app").exists():
            _voice_helper(env)        # L26: its new modes; L27: the new app's own helper; no permission dialog either way
        extension_setup(env)
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

    if inst.bundle:
        env.say(f"1/4  Setting up FERB's background service from {inst.bundle}…")
    else:
        env.say("1/4  Installing FERB's dedicated Python, its venv and the LaunchAgent…")
    env.say("     " + env.tokenizer(inst.home))
    _install(env, update=False)
    client = wait_for_daemon(env)
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

    env.say("3/4  Permissions (checked by the daemon itself)")
    for perm in SETUP_PERMISSIONS:
        if perm == "full_disk_access":
            containment = client.call("doctor.check", {"selftest": False}).get("containment", {})
            if not (containment.get("profile_ok") and containment.get("shell") and containment.get("proxy")
                    and containment.get("proxy_alive")):
                env.say("Full Disk Access setup deferred: containment must report a working shell profile and proxy first.")
                continue
            env.say("Full Disk Access is part of FERB's full-access setup; containment is active before this grant.")
        client = _grant(env, client, perm, binary, required=perm in REQUIRED_PERMISSIONS)
    _automation(env, client, profile)
    hearing_setup(env, client)
    clipboard_setup(env, client)
    config.set('setup', 'permissions_version', 27)

    env.say("4/4  Self-test through the daemon")
    ok = _print_selftest(env, client.call("doctor.check"))
    _review_persona(env, client)
    client.close()
    ok &= _offer_codex_login(env)

    env.say("     FERB uses the installed action registry without per-action permission prompts.")
    env.say("     Connect the accounts/profiles you want available; macOS permissions and logins still apply.")
    env.say("Setup complete." if ok else "Setup finished with problems — `ferb doctor` shows what to fix.")
    return 0 if ok else 1


# ---------- the store's one-time clean-out (plan 2026-10-23 Task 1; L28, C1) ----------

CLEANED = "store.cards_cleaned"      # journal: the clean-out of card rows ran to the end (counts only)
COMPACTED = "store.compacted"        # journal: the compaction after it ran (sizes only)


def cleanout_state(path: Path) -> str | None:
    """What this store's one-time clean-out still has to do: "clean" while it holds cards the composer before memory
    review C1 wrote (`COPYING_CARD_MAKER`) and no `store.cards_cleaned` entry, "compact" when the rows were removed
    but the file wasn't compacted yet (`store.compacted`), else None. Read-only, so it can ask while a daemon runs; a
    store it can't read (none yet, or from before F8) has nothing to do."""
    from ferb.memory.learning import COPYING_CARD_MAKER
    if not path.exists():
        return None
    try:
        c = sqlite3.connect(path.absolute().as_uri() + "?mode=ro", uri=True)
    except sqlite3.Error:
        return None
    try:
        old, cleaned, compacted = c.execute(
            "SELECT EXISTS (SELECT 1 FROM learning_nodes WHERE kind = 'episode' AND made_by = ?), EXISTS (SELECT 1 FROM "
            "journal WHERE kind = ?), EXISTS (SELECT 1 FROM journal WHERE kind = ?)",
            (COPYING_CARD_MAKER, CLEANED, COMPACTED)).fetchone()
    except sqlite3.Error:
        return None
    finally:
        c.close()
    return None if not old else "clean" if not cleaned else "compact" if not compacted else None


def _daemon_answers(env: Env) -> bool:
    try:
        c = env.connect()
        c.call("ping")
        c.close()
        return True
    except (NotRunning, FerbError, OSError):
        return False


def _clean_store(env: Env, stop: Callable[[], bool], state: str) -> None:
    """The store's one-time clean-out, run by the installer while no daemon runs. With `state` "clean", every version of
    each traced task's card keeps only the source rows the current card rule gives it (`Learning.prune_card_sources`),
    and `store.cards_cleaned` records the counts. Then, and alone with "compact", the file is compacted
    (`Database.compact`, when the disk has room for a full copy) and `store.compacted` records the sizes. Never while
    launchd still has the agent, while the process it stopped hasn't exited (`Installer.exiting`: the installer's wait
    ended first), or while a daemon answers (`ferb daemon run` in a terminal). A stop (`stop`: Ctrl-C, FERB.app
    quitting) ends it before the next card, and an error is reported; the update goes on either way, and the next
    `ferb setup --update` does what is left."""
    from ferb.clock.clock import SystemClock
    from ferb.core.ids import RandomIds
    from ferb.memory.journal import Journal
    from ferb.memory.learning import Learning
    from ferb.sanitize.redact import Redactor
    from ferb.store.db import Database, store_size
    inst = env.inst
    loaded = inst.loaded()
    exiting = None if loaded else inst.exiting()
    running = ("launchd hasn't stopped FERB's daemon yet" if loaded else
               f"FERB's daemon (process {exiting}) hasn't exited since launchd stopped it" if exiting is not None else
               "a FERB daemon outside launchd answers (`ferb daemon run` in a terminal?)" if _daemon_answers(env) else "")
    if running:
        env.say(f"     Database clean-out not run: {running}. Stop it, then run `ferb setup --update` again.")
        return
    path = inst.home / "ferb.db"
    env.say("     Cleaning FERB's database with the daemon stopped (once; it may take a few minutes)…" if state == "clean"
            else "     Compacting FERB's database with the daemon stopped (its unused card rows were removed earlier)…")
    said = [0]                                       # the tenths said so far

    def progress(done: int, total: int) -> None:
        if done * 10 // total > said[0]:
            said[0] = done * 10 // total
            env.say(f"     …{done:,} of {total:,} card versions done")

    def rebuilding() -> None:
        env.say("     Stopping: making a database index again over the rows left first (it can take a minute)…")
    db = None
    try:
        before = store_size(path)
        db = Database(path)
        journal = Journal(db, SystemClock(), RandomIds(), Redactor())
        done = ""                                    # what this run removed, as setup says it
        if state == "clean":
            counts = Learning(db, SystemClock(), lambda *args, **kwargs: None, lambda text: text).prune_card_sources(
                stop=stop, progress=progress, rebuilding=rebuilding)
            if counts["stopped"]:
                env.say(f"     Database clean-out stopped before it finished ({counts['rows']:,} unused rows removed so "
                        "far); the next `ferb setup --update` finishes it.")
                return
            journal.append(CLEANED, {k: counts[k] for k in ("tasks", "nodes", "cleaned", "rows", "untraced")})
            done = f"Cleaned FERB's database: {counts['rows']:,} unused rows removed" + (
                f" ({counts['untraced']} card versions whose trace aged out were left as they are)"
                if counts["untraced"] else "")
        if stop():                                   # VACUUM can't be stopped halfway: it waits for the next update
            env.say("     " + (done or "Compacting FERB's database stopped") + "; the next `ferb setup --update` "
                    "compacts the file.")
            return
        compaction = db.compact()
        if not compaction["compacted"]:
            env.say("     " + (done + ". Not" if done else "FERB's database is still not") + " compacted "
                    f"({shown_bytes(before)}): that needs {shown_bytes(compaction['needed'])} free and the disk has "
                    f"{shown_bytes(compaction['free'])}; FERB reuses the freed space inside the file, and the next "
                    "`ferb setup --update` compacts it.")
            return
        after = store_size(path)
        journal.append(COMPACTED, {"bytes_before": before, "bytes_after": after})
        env.say("     " + (f"{done}; " if done else "Compacted FERB's database: ") +
                f"{shown_bytes(before)} → {shown_bytes(after)}.")
    except Exception as e:
        env.say(f"     Database clean-out stopped ({type(e).__name__}: {e}); FERB works without it, and the next "
                "`ferb setup --update` tries again.")
    finally:
        if db is not None:
            db.close()


def _app_installed(env: Env) -> None:
    """After installing from FERB.app: say where `ferb` is. Hearing and push-to-talk are pinned to this app's own
    voice helper by `_voice_helper` (an update replaces it, and an ad-hoc signature pins the exact code)."""
    inst = env.inst
    if inst.bundle is None:
        return
    status = inst.cli_status()
    if status:
        env.say(f"     {status}")


def persona_setup(env: Env) -> int:
    """Review local app-use evidence without installing, restarting or opening permissions."""
    client = env.connect()
    try:
        return _review_persona(env, client, refresh=True)
    finally:
        client.close()


def _review_persona(env: Env, client: Any, *, refresh: bool = False) -> int:
    from ferb.cli.chat import printable
    result = client.call('setup.persona', {'refresh': refresh})
    if result['state'] == 'scanning':
        env.say('Getting to know this Mac from local app-use metadata…')
    while result['state'] == 'scanning':
        env.sleep(0.1)
        result = client.call('setup.persona', {'status': True})
    if result['state'] == 'failed':
        env.say('App-use scan failed: ' + printable(result.get('error', 'unknown error')))
        env.say('Retry with `ferb setup --persona`. Your profile was not confirmed.')
        return 1
    if result['state'] != 'pending':
        return 0
    approval = result['approval']
    env.say(printable(approval['reason']))
    if approval.get('proposal'):
        env.say(printable(approval['proposal']))
    while True:
        try:
            answer = env.ask('Does that describe you? yes, edit in your own words, evidence, or skip: ').strip()
        except EOFError:
            env.say('Profile review is still pending; open FERB or run `ferb setup --persona`.')
            return 0
        if answer.lower() != 'evidence':
            break
        for line in approval.get('evidence', []):
            env.say('  ' + printable(line))
    if not answer:
        env.say('Profile review is still pending; no memory was saved.')
    elif answer.lower() in {'skip', 'no'}:
        client.call('chat.approve', {'approval_id': approval['approval_id'], 'granted': False})
    else:
        client.call('chat.answer', {'approval_id': approval['approval_id'], 'text': answer})
        env.say('Saved your confirmed profile.')
    return 0


def _status(client: Client, perm: str) -> str:
    return str(client.call("doctor.check", {"selftest": False})["permissions"][perm])


def _grant(env: Env, client: Client, perm: str, binary: Path | None, required: bool) -> Client:
    label = perm.replace("_", " ").title()
    while _status(client, perm) != "granted":
        if env.inst.bundle:
            env.say(f"     {label}: not granted. In the System Settings pane that just opened, switch on FERB ({binary});")
            env.say("     if it isn't listed, click +, press ⌘⇧G, paste that path, click Open and switch it on.")
        else:
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
    if ok:          # L28: Codex's own model catalog, so FERB knows each model's window before the first run
        from ferb.runtimes.codex_exec import bundled_catalog
        if bundled_catalog(env.inst.home) is None:
            env.say("Codex didn't print its model catalog; run `ferb setup --update`.")
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
    fix = (f"run `ferb setup` to let macOS ask once, then set FERB to Allow in {PASTE_PANE}" if state == "default"
           else f"set FERB to Allow in {PASTE_PANE}")
    return Check("clipboard", "warn", f"FERB doesn't record what you copy: {why}", fix)


def clipboard_setup(env: Env, client: Any) -> None:
    """I3: on macOS 15.4+, an app reading the clipboard makes macOS ask unless it's set to Allow; an app that never
    asked isn't even listed in that pane. With the owner's yes, the daemon reads it once now, so macOS asks here (an
    alert the owner expects) instead of never, and the owner can then set FERB to Allow."""
    state = client.call("doctor.check", {"selftest": False}).get("clipboard")
    if state not in ("default", "ask"):
        return
    env.say("     Clipboard: FERB records text you copy (kept on this Mac for 7 days) only if macOS lets it read the "
            "clipboard without asking.")
    if state == "default":
        if env.ask("     Let macOS ask now whether FERB may read the clipboard? You'll see its alert once [y/N] "
                   ).strip().lower() not in (
                "y", "yes"):
            env.say("     Clipboard capture stays off; `ferb doctor` says how to turn it on.")
            return
        state = client.call("clipboard.request").get("access", state)
    if state in ("allow", "unrestricted"):
        env.say("     Clipboard: allowed ✓")
        return
    env.say(f"     To let FERB record what you copy without asking each time, set FERB to Allow in {PASTE_PANE}.")


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


def database_check(home: Path) -> Check | None:
    """The store's size on disk, and what its one-time clean-out still has to do (plan 2026-10-23 Task 1)."""
    from ferb.store.db import store_size
    path = home / "ferb.db"
    if not path.exists():
        return None
    size = shown_bytes(store_size(path))
    state = cleanout_state(path)
    if state == "clean":
        return Check("database", "warn", f"{size}; a one-time clean-out of unused card rows is pending",
                     "run `ferb setup --update` (it stops FERB for a few minutes)")
    if state == "compact":
        return Check("database", "warn", f"{size}; its unused card rows were removed, compacting the file is left (it "
                     "needs free disk of at least the database's size)", "free disk space, then run `ferb setup --update`")
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
            optional = ""
            add(Check(f"permission {perm}{optional}", level, status,
                      f"grant it to {dedicated} (System Settings → Privacy & Security)" if level != "ok" else ""))
        for r in report["selftest"]:
            add(Check(f"self-test {r['name']}", "ok" if r["ok"] else "fail", r["detail"]))
        add(containment_check(report.get("containment")))
        if (local_reports := local_reports_check(report.get("containment"))) is not None:
            add(local_reports)
        add(snapshot_check(report.get("containment")))
        add_hearing_check(add, report.get("hearing"))
        if (clipboard := clipboard_check(report.get("clipboard"))) is not None:
            add(clipboard)
        voice = report.get("voice", {})
        if voice.get("push_to_talk"):
            perms = voice.get("permissions", {})
            ok = voice.get("connected") and perms.get("microphone") == "granted" and perms.get("speech") == "granted"
            add(Check("push-to-talk", "ok" if ok else "warn",
                      "FERB.app connected, microphone + speech allowed" if ok else
                      ("FERB.app not connected" if not voice.get("connected") else
                       f"microphone: {perms.get('microphone')}, speech: {perms.get('speech')}"),
                      "" if ok else "run `ferb setup --voice`"))
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
    store = autoinstall.store_extension_id(Config(env.inst.home))
    if autoinstall.valid_id(store):             # setup asked Chrome to install the Web Store copy (L25)
        env.ask(f"     Turn FERB on in the “{name}” profile when Chrome offers it (or add it from "
                f"https://chromewebstore.google.com/detail/{store}), then press Enter ")
    else:
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
    for key in secret_keys:
        if env.ask(f"Also delete FERB's Keychain item '{key}'? [y/N] ").strip().lower() == "y":
            r = subprocess.run(["security", "delete-generic-password", "-s", "ferb", "-a", key], capture_output=True)
            env.say(f"Keychain '{key}': {'deleted' if r.returncode == 0 else 'not found'}")
    if binary:
        what = f"FERB ({inst.grant_target()})" if inst.bundle else str(binary)
        env.say(f"macOS can't remove permissions for you: in System Settings → Privacy & Security, remove "
                f"{what} from Accessibility, Screen Recording (and Input Monitoring / Automation if granted).")
    return 0


CERTIFICATE = "FERB Local Code Signing"


def _build_app(env: Env, run: Callable[..., Any]) -> str | None:
    """From a checkout: build FERB.app into ~/.ferb and sign it with the local certificate (so its permissions survive
    rebuilds). Returns its signature requirement, or None (said why)."""
    # not `-v`: that lists only certificates macOS trusts, and a self-made one never is (it still signs fine)
    found = run(["security", "find-identity", "-p", "codesigning"], capture_output=True, text=True)
    if CERTIFICATE not in (found.stdout or ""):
        env.say(f"The signing certificate \"{CERTIFICATE}\" isn't in your login keychain. Create it in Keychain Access →")
        env.say("Certificate Assistant → Create a Certificate (name it exactly that, type Code Signing), then run this again.")
        return None
    app = env.inst.home / "FERB.app"
    env.say("Building FERB.app (on-device speech recognition and push-to-talk's microphone; no windows, no Dock icon)…")
    built = run([str(repo_root() / "macapp" / "FERB" / "build.sh"), str(app)], capture_output=True, text=True)
    if built.returncode != 0:
        env.say("Build failed:\n" + ((built.stderr or built.stdout or "").strip()[-1500:]))
        return None
    shown = run(["/usr/bin/codesign", "-d", "-r-", str(app)], capture_output=True, text=True)
    req = designated_requirement((shown.stdout or "") + (shown.stderr or ""))
    if not trusted_requirement(req, pinned_ok=False):
        env.say("FERB.app isn't signed with the local certificate, so FERB won't use it.")
        return None
    return req


def _shipped_app(env: Env) -> str | None:
    """From the packaged FERB.app: its own shipped, signed voice helper, used as is (nothing is built and no local
    certificate is needed). Returns its signature requirement, or None (said why)."""
    bundle = env.inst.bundle
    assert bundle is not None
    app = voice_app(env.inst.home, bundle)
    if not (app / "Contents" / "MacOS" / "FERB").is_file():
        env.say(f"{bundle} has no voice helper; download FERB again.")
        return None
    try:                                      # the very helper FERB.app's verified signature seals
        env.inst.check_bundle()
        return env.inst.sealed_requirement(app, "Helpers/FERB Voice.app")
    except InstallError as e:
        env.say(f"FERB won't use its voice helper: {e}")
        return None


def _voice_helper(env: Env, run: Callable[..., Any] | None = None) -> str | None:
    """FERB's voice/speech app (hearing, L26; push-to-talk): built from a checkout, or the packaged FERB.app's shipped
    helper (L27). Records its signature for the daemon's checks: hearing always, push-to-talk when it is set up (the
    same app). Returns the requirement, or None (said why)."""
    req = _shipped_app(env) if env.inst.bundle is not None else _build_app(env, run or env.run)
    if req is None:
        return None
    config = Config(env.inst.home)
    config.set("hearing", "requirement", req)
    if config.get("voice", "requirement") not in (None, req):   # push-to-talk checks the same app's signature
        config.set("voice", "requirement", req)
    return req


def _voice_name(env: Env) -> str:
    """The name macOS shows for the voice/speech app in permission prompts."""
    return "FERB Voice" if env.inst.bundle is not None else "FERB"


def hearing_setup(env: Env, client: Any) -> None:
    """L26: FERB hears app sound during tasks. Set up FERB's voice/speech app (built from a checkout, shipped in the
    packaged FERB.app) and ask once for Speech Recognition only (never the microphone: that is `ferb setup --voice`).
    Speech is recognized on this Mac only; without it FERB can't turn sound into text, but setup still finishes."""
    env.say("     Hearing: FERB turns an app's sound into text during a task, on this Mac only.")
    if _voice_helper(env) is None:
        env.say("     Hearing isn't set up; `ferb doctor` will remind you.")
        return
    app = voice_app(env.inst.home, env.inst.bundle)
    name = _voice_name(env)
    env.say(f"     macOS will ask once for Speech Recognition for {name} — allow it.")
    env.run(["/usr/bin/open", "-n", "-W", str(app), "--args", "request-speech"])
    hearing = client.call("doctor.check", {"selftest": False, "hearing": True}).get("hearing", {})
    speech = hearing.get("speech")
    if speech == "granted":
        line = "on this Mac" if hearing.get("on_device") else \
            f"but on-device recognition isn't available for {hearing.get('locale')}, so FERB won't transcribe"
        env.say(f"     Speech Recognition: granted ✓ ({line})")
    else:
        env.say(f"     Speech Recognition: {speech or 'not checked'}. Allow {name} in System Settings → Privacy & "
                "Security → Speech Recognition, then run `ferb setup` again. Until then FERB can't turn sound into text.")


def voice_setup(env: Env, run: Callable[..., Any] = subprocess.run) -> int:
    """Push-to-talk: build FERB.app into ~/.ferb, sign it with the local certificate (so its permissions
    survive rebuilds), record its signature for the daemon's check, ask for Microphone and Speech Recognition once,
    turn push-to-talk on. The daemon is not restarted. From the packaged FERB.app, its own signed voice helper is
    used as shipped: nothing is built and no local certificate is needed."""
    req = _voice_helper(env, run)
    if req is None:
        env.say("Push-to-talk stays off.")
        return 1
    app = voice_app(env.inst.home, env.inst.bundle)
    config = Config(env.inst.home)
    config.set("voice", "requirement", req)
    env.say(f"macOS will ask once for Microphone and Speech Recognition for {_voice_name(env)} — allow both.")
    run(["/usr/bin/open", "-n", "-W", str(app), "--args", "request"])
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
