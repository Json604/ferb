"""`ferb setup` / `doctor` / `uninstall` flows with a scripted fake daemon, fake installer and scripted answers."""

import io
import subprocess
from pathlib import Path
from typing import Any

import pytest

from ferb.cli import setup as S
from ferb.config import Config
from ferb.core.errors import NotRunning
from ferb.device.install import Installer
from ferb.runtimes.codex_exec import _NATIVE_BIN
from tests.conftest import FakeRunner, SteppedTime, make_app


class FakeDaemon:
    """Answers like the real daemon; permissions flip to granted after the owner 'grants' them."""

    def __init__(self, pid: int = 4242) -> None:
        self.pid = pid
        self.perms = {"accessibility": "denied", "screen_recording": "denied", "input_monitoring": "granted",
                      "full_disk_access": "denied"}
        self.pending_sr = False          # granted in Settings but not applied until restart
        self.tap_ok = True
        self.automation_requests: list[list[str]] = []
        self.requested: list[str] = []   # setup.permission_request (L34 C): macOS asked to list FERB in that pane
        self.calls: list[tuple[str, dict[str, Any] | None]] = []
        # FERB.app's Speech Recognition state as the daemon reports it (L26: FERB hears app sound during tasks)
        self.hearing = {"installed": True, "signed": True, "speech": "granted", "on_device": True, "locale": "en-US"}
        self.clipboard = "allow"         # macOS lets the daemon read the clipboard without asking (I3)
        self.clipboard_requests = 0
        self.tokenizer = "o200k_base_chunked8192_estimate_provider_may_differ"   # how the daemon counts tokens (8a)

    def client(self) -> "FakeClient":
        return FakeClient(self)


class FakeClient:
    def __init__(self, d: FakeDaemon) -> None:
        self.d = d

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.d.calls.append((method, params))
        if method == "ping":
            return {"pid": self.d.pid}
        if method == 'setup.persona':
            return {'state': 'reviewed'}
        if method == "doctor.check":
            st = [] if params and params.get("selftest") is False else [
                {"name": "ax_read", "ok": True, "detail": "ok"},
                {"name": "event_tap", "ok": self.d.tap_ok, "detail": "listen-only tap; no input posted"}]
            probe = (params or {}).get("hearing", not (params and params.get("selftest") is False))
            return {"permissions": dict(self.d.perms), "selftest": st, "executable": "/x/python3.12",
                    "code_hash": "h", "stopped": False, "pid": self.d.pid,
                    "hearing": dict(self.d.hearing) if probe else {"installed": self.d.hearing["installed"]},
                    "clipboard": self.d.clipboard, "tokenizer": self.d.tokenizer,
                    "containment": {"sandbox_exec": True, "profile_ok": True, "reason": "ok", "shell": True,
                                    "shell_reason": "ok", "proxy": True, "proxy_alive": True, "snapshots": False}}
        if method == "clipboard.request":            # macOS shows its alert once; the owner then sets Allow
            self.d.clipboard_requests += 1
            self.d.clipboard = "ask"
            return {"access": "ask"}
        if method == "jev.stats":
            return {"configured": False, "stats": []}
        if method == "sense.status":
            return {"paused": False, "watchers": ["app_focus"], "owner_input": True, "folder_problems": []}
        if method == "browsers.list":
            return {"profiles": [{"browser": "chrome", "name": "Work", "folder": "Profile 1", "email": "",
                                  "ferb_identity": False, "paired": True, "identity": "owner",
                                  "connected": True, "version": "0.6.0", "refused": None},
                                 {"browser": "chrome", "name": "FERB", "folder": "Profile 7", "email": "",
                                  "ferb_identity": True, "paired": True, "identity": "ferb",
                                  "connected": False, "version": None, "refused": None}], "extension_ids": []}
        if method == "identity.check":
            return {"address": "ferb@test.example", "profile": "Profile 7", "profile_ok": True,
                    "profile_name": "FERB", "error": None, "session": None if params and params.get("session") is False else True, "paired": True,
                    "secrets": [{"key": "ferb/email/password", "in_keychain": True}]}
        if method == "automation.request":
            self.d.automation_requests.append(params["bundle_ids"])
            return {"results": {b: "granted" for b in params["bundle_ids"]}}
        if method == "setup.permission_request":     # macOS shows its prompt; granting stays the owner's (in Settings)
            self.d.requested.append(params["permission"])
            return {"permission": params["permission"], "status": self.d.perms.get(params["permission"], "unknown")}
        raise AssertionError(method)

    def close(self) -> None:
        pass


class FakeRun:
    """The commands setup runs for FERB.app (build, signature, its one permission request); nothing really runs."""

    REQUIREMENT = 'identifier "com.ferb.app" and certificate leaf = H"abc"'

    def __init__(self) -> None:
        self.calls: list[list[str]] = []

    def __call__(self, argv: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        self.calls.append([str(a) for a in argv])
        if argv[:2] == ["security", "find-identity"]:
            return subprocess.CompletedProcess(argv, 0, '  1) ABC "FERB Local Code Signing"\n', "")
        if str(argv[0]).endswith("build.sh"):
            Path(argv[1]).mkdir(parents=True, exist_ok=True)
            return subprocess.CompletedProcess(argv, 0, "", "")
        if argv[:3] == ["/usr/bin/codesign", "-d", "-r-"]:
            return subprocess.CompletedProcess(argv, 0, "", f"designated => {self.REQUIREMENT}\n")
        if argv[0] == "/usr/bin/open":
            return subprocess.CompletedProcess(argv, 0, "", "")
        if str(argv[0]).endswith("/bin/codex") and argv[1:] == ["login"]:      # C2: the browser sign-in succeeds
            home = Path(kw["env"]["CODEX_HOME"])
            (home / "auth.json").write_text("{}")
            return subprocess.CompletedProcess(argv, 0, "", "")
        raise AssertionError(argv)

    def ran(self, *prefix: str) -> list[list[str]]:
        return [c for c in self.calls if c[:len(prefix)] == list(prefix)]


@pytest.fixture
def world(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> dict[str, Any]:
    apps = tmp_path / "Applications"
    make_app(apps, "Notes", "com.apple.Notes", sdef=True)
    make_app(apps, "Google Chrome", "com.google.Chrome", NSAppleScriptEnabled=True)
    make_app(apps, "Brave Browser", "com.brave.Browser", NSAppleScriptEnabled=True)
    monkeypatch.setattr(S, "scan", lambda did: __import__("ferb.device.scan", fromlist=["scan"]).scan(did, [apps]))
    home = tmp_path / ".ferb"
    Config(home).set("identity", "profile", "Profile 7")              # FERB's identity profile already chosen
    runner = FakeRunner(home)
    inst = Installer(home, label="com.ferb.test", run=runner, launch_agents=tmp_path / "LA")
    daemon = FakeDaemon()
    panes: list[str] = []
    (home / "codex-home").mkdir(parents=True)
    (home / "codex-home" / "auth.json").write_text("{}")            # FERB's own Codex is signed in (C2 tests undo it)
    return {"inst": inst, "runner": runner, "daemon": daemon, "panes": panes, "home": home, "run": FakeRun()}


def make_env(world: dict[str, Any], answers: list[str], daemon_up: bool = True) -> tuple[S.Env, io.StringIO]:
    out = io.StringIO()
    d: FakeDaemon = world["daemon"]
    answers = list(answers)

    def ask(prompt: str) -> str:
        if "Does that describe you?" in prompt:
            return "skip"
        a = answers.pop(0)
        if "Press Enter" in prompt and a == "":                      # the owner grants in System Settings
            for perm in ("accessibility", "full_disk_access", "input_monitoring"):
                if d.perms[perm] != "granted" and perm in world["panes"][-1:]:
                    d.perms[perm] = "granted"
            if world["panes"][-1:] == ["screen_recording"]:
                d.pending_sr = True
        return a

    def restart() -> None:
        if d.pending_sr:
            d.perms["screen_recording"] = "granted"
    world["inst"].restart = restart                                  # type: ignore[method-assign]

    def connect() -> FakeClient:
        if not daemon_up and not world["runner"].loaded:
            raise NotRunning("not running")
        return d.client()
    env = S.Env(world["inst"], connect, ask, out, open_pane=world["panes"].append, sleep=lambda s: None,  # type: ignore[arg-type]
                running_apps=lambda: {"com.google.Chrome", "com.brave.Browser", "com.apple.Notes"}, persona_usage=lambda apps: [],
                run=world["run"])
    return env, out


def test_refuses_while_a_foreground_dev_daemon_holds_the_socket(world: dict[str, Any]) -> None:
    world["daemon"].pid = 999                            # not launchd's pid (nothing loaded)
    env, out = make_env(world, [])
    assert S.setup(env) == 1
    assert "outside launchd" in out.getvalue()
    assert world["runner"].ran("uv", "python", "install") == 0


def test_plain_setup_asks_for_no_permission_and_never_records_setup_done(world: dict[str, Any]) -> None:
    """The CLI permission walkthrough is gone (notes/daemon.md decision 4: FERB's setup window asks); `ferb setup`
    opens no pane, requests nothing, and never records setup as done: every row decides that, FERB.app's own
    Notifications and Microphone included, which only the window's Done can read (12e review)."""
    env, out = make_env(world, [], daemon_up=False)          # any request for owner input would fail this test
    assert S.setup(env) == 0, out.getvalue()
    d = world["daemon"]
    assert world["panes"] == [] and d.requested == [] and d.automation_requests == [] and d.clipboard_requests == 0
    assert "Not granted yet" in out.getvalue() and "accessibility=denied" in out.getvalue()
    assert Config(world["home"]).get("setup", "permissions_version", 0) == 0
    assert (world["home"] / "device").is_dir() and "Setup complete." in out.getvalue()
    d.perms = {name: "granted" for name in S.SETUP_PERMISSIONS}
    env, out = make_env(world, [], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    assert "permissions are granted" in out.getvalue()
    assert Config(world["home"]).get("setup", "permissions_version", 0) == 0


def test_doctor_reports_and_fails_on_missing_required_permission(world: dict[str, Any]) -> None:
    world["inst"].install()
    env, out = make_env(world, [])
    rc = S.doctor(env)
    text = out.getvalue()
    assert rc == 1
    assert "✗ permission accessibility: denied" in text and "✓ background service" in text
    ok_lines = [l for l in text.splitlines() if l.startswith("✓")]
    assert ok_lines and not any("→" in l for l in ok_lines)            # hints only on problems
    assert "✗ permission full_disk_access: denied" in text
    assert "✓ browser chrome/FERB: FERB's own profile: paired, not open right now" in text   # not a warning
    assert "✓ containment: FERB's shell and AppleScript run inside its sandbox (sandbox-exec)" in text
    world["daemon"].perms.update(accessibility="granted", screen_recording="granted", full_disk_access="granted")
    S.device_id(__import__("ferb.config", fromlist=["Config"]).Config(world["home"]))
    from ferb.device.profile import ProfileStore
    from ferb.config import Config
    ProfileStore(world["home"]).save(S.scan(Config(world["home"]).get("device", "id")))
    out2 = io.StringIO()
    env.out = out2
    assert S.doctor(env) == 0, out2.getvalue()


def test_doctor_says_jev_is_off_and_where_a_key_comes_from(world: dict[str, Any]) -> None:
    """Jev is switched off (notes/memory.md decision 4, daemon decision 9): doctor says so as an ok line, never a
    failure, and names no removed command."""
    world["inst"].install()
    env, out = make_env(world, [])
    S.doctor(env)
    line = next(l for l in out.getvalue().splitlines() if " jev:" in l)
    assert line.startswith("✓") and "→" not in line
    assert "activity notices are paused" in line and "https://console.typesafe.ai/keys" in line
    assert "ferb setup --jev" not in out.getvalue()


def test_uninstall_purge_needs_typed_confirmation(world: dict[str, Any]) -> None:
    world["inst"].install()
    env, out = make_env(world, ["yes please"])
    assert S.uninstall(env, purge=True) == 0
    assert world["home"].exists() and "Not purged." in out.getvalue()
    assert not world["inst"].plist_path.exists()
    env, out = make_env(world, ["delete"])
    assert S.uninstall(env, purge=True) == 0
    assert not world["home"].exists()
    assert "remove" in out.getvalue() and "Accessibility" in out.getvalue()


def test_update_reports_missing_permissions_without_opening_settings_or_prompting(world):
    world['inst'].install()
    env, out = make_env(world, [])  # any request for owner input would fail this test
    assert S.setup(env, update=True) == 0, out.getvalue()
    assert world['panes'] == [] and world['daemon'].automation_requests == []
    assert Config(world['home']).get('setup', 'permissions_version', 0) == 0
    assert 'Not granted yet' in out.getvalue() and 'accessibility=denied' in out.getvalue()
    assert ('doctor.check', {'selftest': False, 'hearing': True}) in world['daemon'].calls


def test_update_recognizes_existing_grants_despite_an_older_setup_marker(world):
    world['inst'].install()
    config = Config(world['home'])
    config.set('setup', 'permissions_version', 22)
    world['daemon'].perms = {name: 'granted' for name in S.SETUP_PERMISSIONS}
    env, out = make_env(world, [])
    assert S.setup(env, update=True) == 0
    assert 'deferred' not in out.getvalue()
    assert "permissions are granted" in out.getvalue()
    assert Config(world['home']).get('setup', 'permissions_version') == 22       # only the window's Done records it
    assert world['panes'] == [] and world['daemon'].automation_requests == []


def test_update_does_not_treat_a_missing_permission_status_as_a_grant(world):
    world['inst'].install()
    world['daemon'].perms = {name: 'granted' for name in S.SETUP_PERMISSIONS if name != 'full_disk_access'}
    env, out = make_env(world, [])
    assert S.setup(env, update=True) == 0
    assert 'full_disk_access=unknown' in out.getvalue()
    assert Config(world['home']).get('setup', 'permissions_version', 0) == 0
    assert world['panes'] == []


def test_doctor_never_wakes_browser_or_calls_not_checked_signed_out(world):
    world['inst'].install()
    env, out = make_env(world, [])
    S.doctor(env)
    assert ('identity.check', {'session': False}) in world['daemon'].calls
    assert 'session not checked' in out.getvalue()
    assert 'not signed in' not in out.getvalue()
    assert not world['panes'] and not world['daemon'].automation_requests


def test_doctor_shows_the_protection_lists_and_why_a_malformed_one_stops_ferb(world: dict[str, Any]) -> None:
    """[protection] as the daemon reads it at start (rule 10): when it can't be enforced FERB doesn't start, and doctor
    says why without needing the daemon."""
    world["inst"].install()
    env, out = make_env(world, [])
    S.doctor(env)
    assert "✓ protection:" in out.getvalue() and "shell and file tools follow both" in out.getvalue()
    Config(world["home"]).set("protection", "fs_reads_never_read", False)    # an old config's switch: read harmlessly
    env, out = make_env(world, [])
    S.doctor(env)
    assert "✓ protection:" in out.getvalue() and "a task asks you before it opens one" in out.getvalue()
    Config(world["home"]).set("protection", "never_read", "~/.ssh")
    env, out = make_env(world, [])
    assert S.doctor(env) == 1
    assert "✗ protection: protection.never_read in config.toml must be a list of paths" in out.getvalue()


def test_update_shows_what_ferb_saw_without_asking_or_repeating_permission_flow(world):
    """Decision 16: setup shows "here's what I saw" once; nothing to answer (decision 15)."""
    world['inst'].install()
    env, out = make_env(world, [])
    calls = []

    class SeenClient(FakeClient):
        def call(self, method, params=None):
            calls.append((method, params))
            if method == 'setup.persona':
                return {'state': 'seen', 'summary': "Here's what I saw on your Mac: you code a lot in VS Code."}
            return super().call(method, params)
    client = SeenClient(world['daemon'])
    env.connect = lambda: client
    env.ask = lambda _: (_ for _ in ()).throw(AssertionError('setup asked about the profile'))
    assert S.setup(env, update=True) == 0
    assert "Here's what I saw on your Mac" in out.getvalue()
    assert not [m for m, _ in calls if m in ('chat.answer', 'chat.approve')]
    assert world['panes'] == [] and world['daemon'].automation_requests == []


# ---------- L26: FERB.app hears app sound during tasks (Speech Recognition only, unless --voice) ----------

def test_plain_setup_builds_ferb_app_for_hearing_without_a_permission_dialog(world: dict[str, Any]) -> None:
    env, out = make_env(world, [], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    run: FakeRun = world["run"]
    app = str(world["home"] / "FERB.app")
    assert [c[1] for c in run.calls if c[0].endswith("build.sh")] == [app]
    assert not run.ran("/usr/bin/open")                    # Speech Recognition is asked by FERB's setup window
    assert Config(world["home"]).get("hearing", "requirement") == FakeRun.REQUIREMENT


def test_doctor_reports_ferb_apps_speech_permission(world: dict[str, Any]) -> None:
    world["inst"].install()
    env, out = make_env(world, [])
    S.doctor(env)
    assert "✓ hearing: FERB.app may use on-device Speech Recognition (en-US)" in out.getvalue()
    world["daemon"].hearing.update(speech="not_determined")
    env, out = make_env(world, [])
    S.doctor(env)
    assert "! hearing: FERB.app's Speech Recognition: not_determined" in out.getvalue()
    assert "run `ferb setup`" in out.getvalue()
    world["daemon"].hearing.update(speech="granted", on_device=False, locale="en-IN")
    env, out = make_env(world, [])
    S.doctor(env)
    assert "! hearing: on-device speech recognition isn't available for en-IN" in out.getvalue()
    assert not world["run"].calls                                    # doctor never builds or asks


def test_update_rebuilds_an_installed_ferb_app_without_asking_for_anything(world: dict[str, Any]) -> None:
    world["inst"].install()
    (world["home"] / "FERB.app").mkdir()
    env, out = make_env(world, [])
    assert S.setup(env, update=True) == 0, out.getvalue()
    run: FakeRun = world["run"]
    assert [c for c in run.calls if c[0].endswith("build.sh")]       # the new transcribe mode is installed
    assert not run.ran("/usr/bin/open")                               # no permission dialog during an update
    assert Config(world["home"]).get("hearing", "requirement") == FakeRun.REQUIREMENT


def test_doctor_says_nothing_about_hearing_it_did_not_probe() -> None:
    lines: list[S.Check] = []
    S.add_hearing_check(lines.append, {"installed": True})          # a quick check: FERB.app wasn't asked
    assert lines == []
    S.add_hearing_check(lines.append, {"installed": False})
    assert lines[0].status == "warn" and "isn't installed" in lines[0].detail


def test_update_refreshes_the_voice_signature_when_ferb_apps_requirement_changes(world: dict[str, Any]) -> None:
    world["inst"].install()
    (world["home"] / "FERB.app").mkdir()
    Config(world["home"]).set("voice", "requirement", 'identifier "com.ferb.app" and certificate leaf = H"old"')
    env, out = make_env(world, [])
    assert S.setup(env, update=True) == 0, out.getvalue()
    assert Config(world["home"]).get("voice", "requirement") == FakeRun.REQUIREMENT
    assert world["run"].ran("/usr/bin/codesign", "-d", "-r-")


# ---------- packaged FERB.app (L27) ----------

from tests.support.app_bundle import PINNED, BundleRunner, make_ferb_app  # noqa: E402


@pytest.fixture
def app_world(world: dict[str, Any], tmp_path: Path) -> dict[str, Any]:
    """The same Mac, with FERB installed from a packaged FERB.app instead of a checkout; the app has registered its
    background service (macOS loaded it)."""
    app = make_ferb_app(tmp_path / "Applications")
    runner = BundleRunner(world["home"])
    runner.loaded = True
    world.update(app=app, runner=runner, inst=Installer(world["home"], label="com.ferb.test", run=runner,
                                                        launch_agents=tmp_path / "LA", bundle=app,
                                                        cli_dir=tmp_path / "bin"))
    return world


def test_setup_from_the_app_installs_from_the_bundle_and_says_where_ferb_is(app_world: dict[str, Any]) -> None:
    env, out = make_env(app_world, ["", "", "", ""], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    text = out.getvalue()
    assert f"from {app_world['app']}" in text and "venv" not in text and "python3.12" not in text
    assert f"Permissions go to FERB ({app_world['app']}/Contents/Helpers/FERB.app)" in text   # what they go to
    assert f"`ferb` command: {app_world['inst'].cli_dir / 'ferb'}" in text
    assert "PATH" not in text                              # the app is the product; the CLI is for development
    assert app_world["runner"].ran("uv") == 0


def test_setup_from_the_app_repins_push_to_talk_to_the_bundled_voice_helper(app_world: dict[str, Any]) -> None:
    config = Config(app_world["home"])
    config.set("voice", "requirement", 'identifier "com.ferb.app" and certificate leaf = H"old"')
    app_world["inst"].install()
    env, out = make_env(app_world, [])
    assert S.setup(env, update=True) == 0, out.getvalue()
    assert Config(app_world["home"]).get("voice", "requirement") == PINNED   # the app's own helper, not the old build
    assert "Set Up FERB… in FERB's menu bar" in out.getvalue()               # an ad-hoc update can lose permissions


def test_closing_setup_while_it_installs_never_leaves_the_daemon_unloaded(world: dict[str, Any]) -> None:
    """A stop (SIGTERM, Ctrl-C) that arrives while the background service is being reinstalled waits until it is
    loaded again, then setup stops."""
    import os
    import signal
    runner = world["runner"]
    world["inst"].install()
    real = runner.__call__

    def stop_midway(cmd: list[str]) -> Any:
        if cmd[:2] == ["launchctl", "bootout"]:
            os.kill(os.getpid(), signal.SIGTERM)           # the owner stops setup between unload and load
        return real(cmd)
    world["inst"].run = stop_midway
    env, out = make_env(world, [], daemon_up=False)
    with pytest.raises(KeyboardInterrupt):
        S.setup(env, update=True)
    assert runner.loaded and runner.ran("launchctl", "bootstrap") == 2       # loaded before setup stopped
    assert signal.getsignal(signal.SIGTERM) is signal.SIG_DFL


def test_a_checkout_update_says_it_took_over_from_the_app(app_world: dict[str, Any], tmp_path: Path) -> None:
    app_world["inst"].install()
    source = Installer(app_world["home"], label="com.ferb.test", run=app_world["runner"], launch_agents=tmp_path / "LA")
    app_world["runner"].requirement = 'identifier "com.ferb.secrets" and certificate leaf = H"local"'   # its own build
    from ferb.runtimes.codex_exec import CODEX_VERSION
    app_world["runner"].codex_version = f"codex-cli {CODEX_VERSION}\n"
    (source.venv / "bin").mkdir(parents=True)
    (source.venv / "bin" / "python").write_text("")
    app_world["inst"] = source
    env, out = make_env(app_world, [])
    assert S.setup(env, update=True) == 0, out.getvalue()
    assert f"now runs from {source.venv_python} (it ran from {app_world['app']}" in out.getvalue()


def test_doctor_from_the_app_checks_the_bundled_interpreter_not_a_venv(app_world: dict[str, Any]) -> None:
    app_world["inst"].install()
    env, out = make_env(app_world, [])
    S.doctor(env)
    text = out.getvalue()
    assert f"✓ app: {app_world['app']}" in text
    assert f"✓ interpreter: {app_world['app']}/Contents/Helpers/FERB.app/Contents/MacOS/FERB" in text
    assert "venv" not in text and "✓ service definition: inside FERB.app" in text and "✓ background service" in text


# ---------- packaged FERB.app × hearing (L26 + L27 integration) ----------

def test_plain_setup_from_the_app_hears_through_its_shipped_helper_without_building(app_world: dict[str, Any]) -> None:
    env, out = make_env(app_world, [], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    run: FakeRun = app_world["run"]
    assert not any(c[0] == "security" or c[0].endswith("build.sh") for c in run.calls)   # no certificate, no build
    assert not run.ran("/usr/bin/open")                    # no permission dialog from the CLI
    assert Config(app_world["home"]).get("hearing", "requirement") == PINNED   # the shipped helper's signature
    assert not (app_world["home"] / "FERB.app").exists()


def test_update_from_the_app_repins_hearing_to_the_new_helper_without_asking(app_world: dict[str, Any]) -> None:
    Config(app_world["home"]).set("hearing", "requirement", 'identifier "com.ferb.app" and certificate leaf = H"old"')
    app_world["inst"].install()
    env, out = make_env(app_world, [])
    assert S.setup(env, update=True) == 0, out.getvalue()
    run: FakeRun = app_world["run"]
    assert not run.calls                                              # nothing built, no permission dialog
    assert Config(app_world["home"]).get("hearing", "requirement") == PINNED
    assert Config(app_world["home"]).get("voice", "requirement") is None     # push-to-talk stays as it was (off)


def test_setup_from_the_app_refuses_a_voice_helper_its_signature_doesnt_seal(app_world: dict[str, Any]) -> None:
    runner = app_world["runner"]

    def swapped(cmd: list[str]) -> "subprocess.CompletedProcess[str]":
        if cmd[0].endswith("codesign") and "-dv" in cmd and cmd[-1].endswith("FERB Voice.app"):
            return subprocess.CompletedProcess(cmd, 0, "", f"Executable={cmd[-1]}\nCDHash={'0' * 40}\n"
                                                           "Signature=adhoc\nTeamIdentifier=not set\n")
        return runner(cmd)
    Config(app_world["home"]).set("hearing", "requirement", "old")
    app_world["inst"].install()
    app_world["inst"].run = swapped
    env, out = make_env(app_world, [])
    assert S.setup(env, update=True) == 0, out.getvalue()
    assert "FERB Voice.app isn't the one FERB.app's signature seals" in out.getvalue()
    assert Config(app_world["home"]).get("hearing", "requirement") == "old"   # never pinned to other code


# ---------- C2: FERB's own Codex sign-in ----------

def _signed_out(world: dict[str, Any]) -> Path:
    auth = world["home"] / "codex-home" / "auth.json"
    auth.unlink()
    binary = world["home"] / "node" / _NATIVE_BIN
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_text("")
    return auth


def test_setup_offers_the_codex_sign_in_when_ferbs_codex_home_has_none(world: dict[str, Any]) -> None:
    auth = _signed_out(world)
    asked: list[str] = []
    env, out = make_env(world, ["", "", "", "1,2,3", ""], daemon_up=False)
    plain = env.ask
    env.ask = lambda prompt: (asked.append(prompt), plain(prompt))[1]   # type: ignore[method-assign]
    assert S.setup(env) == 0, out.getvalue()
    assert any("Sign in to ChatGPT for FERB now? [Y/n]" in p for p in asked)
    assert world["run"].ran(str(auth.parent.parent / "node" / _NATIVE_BIN), "login")
    assert auth.exists() and "Setup complete." in out.getvalue()


def test_declining_the_codex_sign_in_says_how_to_do_it_later(world: dict[str, Any]) -> None:
    auth = _signed_out(world)
    env, out = make_env(world, ["n"], daemon_up=False)
    assert S.setup(env) == 1
    text = out.getvalue()
    assert not auth.exists() and not [c for c in world["run"].calls if c[1:] == ["login"]]
    assert "Sign in to Codex…" in text and "ferb setup --codex-login" in text


def test_setup_does_not_ask_when_ferbs_codex_is_signed_in_or_unused(world: dict[str, Any]) -> None:
    env, out = make_env(world, ["", "", "", "1,2,3"], daemon_up=False)     # one answer fewer: no sign-in question
    assert S.setup(env) == 0, out.getvalue()
    _signed_out(world)
    for role in ("orchestrator", "worker"):
        Config(world["home"]).set("runtimes", role, "claude")
    world["daemon"].perms = {name: "granted" for name in S.SETUP_PERMISSIONS}
    env, out = make_env(world, ["1,2,3"], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()


def test_update_never_asks_but_says_codex_isnt_signed_in(world: dict[str, Any]) -> None:
    _signed_out(world)
    world["inst"].install()
    env, out = make_env(world, [])                    # any request for owner input would fail this test
    assert S.setup(env, update=True) == 0
    assert "Sign in to Codex…" in out.getvalue() and "ferb setup --codex-login" in out.getvalue()


def test_doctor_fails_without_ferbs_own_codex_sign_in(world: dict[str, Any]) -> None:
    _signed_out(world)
    world["inst"].install()
    env, out = make_env(world, [])
    S.doctor(env)
    text = out.getvalue()
    assert "✗ codex home" in text and "ferb setup --codex-login" in text
    assert "using your personal Codex setup" not in text          # the daemon never falls back to ~/.codex


# ---------- I3: macOS pasteboard privacy ----------

def test_setup_never_requests_clipboard_access_without_a_yes(world: dict[str, Any]) -> None:
    world["daemon"].clipboard = "default"
    env, out = make_env(world, ["", "", "", "1,2,3", ""], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    assert world["daemon"].clipboard_requests == 0


def test_doctor_says_how_to_let_ferb_read_the_clipboard(world: dict[str, Any]) -> None:
    world["inst"].install()
    for state, line in (("default", "! clipboard"), ("ask", "! clipboard"), ("deny", "! clipboard"),
                        ("allow", "✓ clipboard")):
        world["daemon"].clipboard = state
        env, out = make_env(world, [])
        S.doctor(env)
        text = out.getvalue()
        assert line in text, (state, text)
        if state != "allow":
            assert "Paste from Other Apps" in text


# ---------- fix wave 3, 8a: FERB's own pinned tokenizer copy ----------

def test_doctor_says_when_token_counts_fall_back_to_a_byte_ceiling(world: dict[str, Any]) -> None:
    world["inst"].install()
    for method, line in (("o200k_base_chunked8192_estimate_provider_may_differ", "✓ tokenizer"),
                         ("utf8_byte_ceiling_tokenizer_unavailable", "! tokenizer")):
        world["daemon"].tokenizer = method
        env, out = make_env(world, [])
        S.doctor(env)
        assert line in out.getvalue(), out.getvalue()
    assert "ferb setup --update" in out.getvalue()


def test_setup_puts_ferbs_tokenizer_in_place_before_the_daemon_restarts(world: dict[str, Any]) -> None:
    world["inst"].install()
    env, out = make_env(world, [])
    said: list[Path] = []
    env.tokenizer = lambda home: (said.append(home), "Tokenizer: FERB's pinned copy is in place.")[1]
    assert S.setup(env, update=True) == 0
    assert said == [world["home"]] and "Tokenizer: FERB's pinned copy is in place." in out.getvalue()


# ---------- storage decision 4 (step 12a): the one-time clean-up, after the owner's OK, with the daemon stopped ----------

def _old_store(home: Path) -> dict[str, Any]:
    """A store an older FERB left: cards the composer before C1 wrote (three versions of one traced task's card, each
    citing every ref its planner was shown), Jev's decisions and perception receipts, the old embedding vectors, task
    memory refs and a rollup node; beside them what decision 4 protects (memory, the episode, chat, the task, a
    schedule, the traced task's spans)."""
    import sqlite3
    from ferb.core import origin
    from tests.support.cards import card_store, copied_cards, traced_task
    home.mkdir(parents=True, exist_ok=True)
    made = sqlite3.connect(home / "ferb.db")                              # a file from before incremental auto-vacuum
    made.executescript("PRAGMA auto_vacuum = NONE; CREATE TABLE made (x); DROP TABLE made;")
    made.close()
    db, _, spans, learning = card_store(home)
    kept, shown = traced_task(db, spans, learning, "c0ffee01")
    rule = learning.card_source_refs("c0ffee01")
    versions = copied_cards(learning, "c0ffee01", kept | shown)
    c = db.conn
    for i in range(7):
        c.execute("INSERT INTO decisions (id, at, question, baseline, mode) VALUES (?, '2026-10-01T00:00:00+00:00', "
                  "'q', 'b', 'shadow')", (f"d{i}",))
    c.execute("INSERT INTO perception_batches VALUES ('b1', '2026-10-01', 'size', 'done', '[]', NULL)")
    c.executemany("INSERT INTO perception_receipts VALUES (?, 'b1')", [(f"e{i}",) for i in range(5)])
    rollup = learning.put("summary", "2026-W40", {"text": "a week"}, ["journal:x"], made_by="code:rollup/1",
                          labels={origin.PRIVATE})["id"]
    c.execute("INSERT INTO learning_vectors VALUES (?, 'old-model', x'00', '2026-10', 'h')", (rollup,))
    c.executemany("INSERT INTO task_memory_refs VALUES ('c0ffee01', ?)", [(f"memory:m{i}",) for i in range(4)])
    c.executemany("INSERT INTO learning_jobs (source_ref, status, at) VALUES (?, 'done', '2026-10-01')",
                  [(f"intent:{i}",) for i in range(3)])                  # a table no code reads any more
    c.execute("INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, "
              "valid_from, created_at) VALUES ('mem1', 'preference', 'owner', 'tea', 'likes green tea', 'owner_stated', "
              "'active', 1.0, '2026-10-01', '2026-10-01', '2026-10-01')")
    c.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES ('j1', '2026-10-01T00:00:00+00:00', "
              "'chat.message', '{\"text\": \"hi\"}', 'owner')")
    at = "2026-10-01T00:00:00+00:00"                                     # what FERB keeps forever beside them
    c.execute("INSERT INTO task_result_cards VALUES ('c0ffee01', '[]')")
    c.execute("INSERT INTO timers (id, kind, fire_at, message, state, delivered_at, created_at) VALUES "
              "('tm1', 'reminder', ?, 'tea', 'delivered', ?, ?)", (at, at, at))
    c.execute("INSERT INTO memory_set_aside VALUES ('mem1', ?, 'not about the owner')", (at,))
    c.execute("INSERT INTO memory_import_runs (id, source, status, started_at, updated_at) VALUES "
              "('run1', 'chatgpt', 'done', ?, ?)", (at, at))
    c.execute("INSERT INTO memory_import_done VALUES ('chatgpt', 'part1', 'run1', ?)", (at,))
    c.execute("INSERT INTO memory_import_items VALUES ('mem1', 'chatgpt', 'run1', ?)", (at,))
    c.execute("INSERT INTO forgotten_refs VALUES ('trail:gone', ?)", (at,))
    c.execute("INSERT INTO memory_forgotten VALUES ('print1', ?)", (at,))
    db.close()
    return {"versions": versions, "rule": rule, "copied": kept | shown, "rollup": rollup}


def _protected(home: Path) -> dict[str, list[Any]]:
    """Every row of every table decision 4 protects, order-free (VACUUM may renumber rowids)."""
    import sqlite3
    from ferb.store.cleanup import PROTECTED, PROTECTED_ROWS
    c = sqlite3.connect(home / "ferb.db")
    where = PROTECTED_ROWS | {"journal": "kind NOT LIKE 'store.%'"}            # the journal: but the run's own records
    try:
        names = {r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}
        assert set(PROTECTED) <= names                                   # a renamed table can't slip out of the check
        rows = {t: sorted(map(repr, c.execute(f"SELECT * FROM {t}" + (f" WHERE {where[t]}" if t in where else ""))))
                for t in PROTECTED}
        assert rows["learning_nodes"] and rows["timers"] and rows["memory_import_items"]   # the fixture fills them
        return rows
    finally:
        c.close()


def _q(home: Path, sql: str, *params: Any) -> Any:
    import sqlite3
    c = sqlite3.connect(home / "ferb.db")
    try:
        return c.execute(sql, params).fetchone()[0]
    finally:
        c.close()


def _journal_kinds(home: Path) -> list[str]:
    import sqlite3
    c = sqlite3.connect(home / "ferb.db")
    try:
        return [r[0] for r in c.execute("SELECT kind FROM journal WHERE kind LIKE 'store.%' ORDER BY seq")]
    finally:
        c.close()


def _preview(world: dict[str, Any]) -> dict[str, Any]:
    import json
    env, out = make_env(world, [])
    assert S.cleanup_preview(env) == 0
    return json.loads(out.getvalue().strip().splitlines()[-1])


def _apply(world: dict[str, Any], ok: str, daemon_up: bool = False) -> tuple[int, str, dict[str, Any]]:
    import json
    env, out = make_env(world, [], daemon_up=daemon_up)
    code = S.cleanup_apply(env, ok)
    return code, out.getvalue(), json.loads(out.getvalue().strip().splitlines()[-1])


def test_an_update_never_cleans_the_database_on_its_own(world: dict[str, Any]) -> None:
    """Decision 4: the clean-up waits for the owner's OK; installing or updating FERB removes nothing."""
    world["inst"].install()
    old = _old_store(world["home"])
    before = _protected(world["home"]), _q(world["home"], "SELECT COUNT(*) FROM learning_sources")
    env, out = make_env(world, [], daemon_up=False)
    assert S.setup(env, update=True) == 0, out.getvalue()
    assert (_protected(world["home"]), _q(world["home"], "SELECT COUNT(*) FROM learning_sources")) == before
    assert _q(world["home"], "SELECT COUNT(*) FROM decisions") == 7 and _journal_kinds(world["home"]) == []
    assert old["versions"]


def test_the_preview_lists_what_the_cleanup_removes_and_changes_nothing(world: dict[str, Any]) -> None:
    world["inst"].install()
    old = _old_store(world["home"])
    before = _protected(world["home"]), _q(world["home"], "SELECT COUNT(*) FROM learning_sources")
    shown = _preview(world)
    rows = {(s["table"], s["rows"]) for s in shown["steps"]}
    card_rows = 3 * len(old["copied"] - old["rule"])
    assert {("decisions", 7), ("perception_receipts", 5), ("perception_batches", 1), ("learning_vectors", 1),
            ("task_memory_refs", 4), ("learning_nodes", 1), ("learning_sources", card_rows)} <= rows, rows
    assert shown["compact"] is True and shown["ok"] and shown["rows"] == sum(s["rows"] for s in shown["steps"])
    assert {s["table"]: s["rows"] for s in shown["steps"] if s.get("drop")} == {
        "owner_questions": 0, "notification_rules": 0, "learning_receipts": 0, "learning_jobs": 3}
    assert shown["bytes"] > 0 and shown["free"] > 0
    assert (_protected(world["home"]), _q(world["home"], "SELECT COUNT(*) FROM learning_sources")) == before
    env, out = make_env(world, [])
    S.doctor(env)
    assert "! database: " in out.getvalue() and "waits for your OK" in out.getvalue()


def test_the_cleanup_refuses_without_the_owners_ok_and_starts_the_daemon_again(world: dict[str, Any]) -> None:
    world["inst"].install()
    _old_store(world["home"])
    before = _q(world["home"], "SELECT COUNT(*) FROM learning_sources"), _q(world["home"], "SELECT COUNT(*) FROM decisions")
    code, text, result = _apply(world, "not-what-was-shown")
    assert code == 2 and result["refused"] is True and result["plan"]["ok"] == _preview(world)["ok"]
    assert "changed since you saw it" in text
    assert (_q(world["home"], "SELECT COUNT(*) FROM learning_sources"),
            _q(world["home"], "SELECT COUNT(*) FROM decisions")) == before
    assert _journal_kinds(world["home"]) == [] and world["runner"].loaded


def test_the_ok_holds_while_the_hourly_pass_trims_rows_and_not_when_a_row_only_the_cleanup_removes_changes(
        world: dict[str, Any]) -> None:
    """The owner's OK covers the steps and the rows only the clean-up changes; the running daemon's hourly pass ages
    out Jev's decisions and perception receipts between the preview and the OK, which must not refuse it."""
    import sqlite3
    world["inst"].install()
    _old_store(world["home"])
    ok = _preview(world)["ok"]

    def write(sql: str) -> None:
        c = sqlite3.connect(world["home"] / "ferb.db")
        c.execute(sql)
        c.commit()
        c.close()
    write("DELETE FROM decisions WHERE id IN ('d0', 'd1')")             # the hourly pass, meanwhile
    write("DELETE FROM perception_receipts WHERE rowid = (SELECT MIN(rowid) FROM perception_receipts)")
    assert _preview(world)["ok"] == ok
    write("INSERT INTO task_memory_refs VALUES ('c0ffee01', 'memory:m9')")   # not what the owner saw
    code, text, result = _apply(world, ok)
    assert code == 2 and result["refused"] is True
    write("DELETE FROM task_memory_refs WHERE ref = 'memory:m9'")
    code, text, result = _apply(world, ok)
    assert code == 0, text
    assert _q(world["home"], "SELECT COUNT(*) FROM decisions") == 0


def test_the_cleanup_runs_with_the_daemon_stopped_removes_only_what_was_shown_and_journals_it(
        world: dict[str, Any], monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.store import cleanup
    world["inst"].install()
    old = _old_store(world["home"])
    protected = _protected(world["home"])
    shown = _preview(world)
    runner = world["runner"]
    seen: list[tuple[bool, int]] = []
    real = cleanup.apply
    monkeypatch.setattr(cleanup, "apply", lambda *a, **k: (seen.append((runner.loaded, len(runner.cmds))),
                                                           real(*a, **k))[1])
    runner.cmds.clear()
    code, text, result = _apply(world, shown["ok"])
    assert code == 0, text
    [(loaded, at)] = seen
    assert not loaded                                                     # never while the daemon runs
    before = [c[:2] for c in runner.cmds[:at] if c[:2] in (["launchctl", "bootout"], ["launchctl", "bootstrap"])]
    assert before == [["launchctl", "bootout"]]                           # stopped right before it
    assert ["launchctl", "bootstrap"] in [c[:2] for c in runner.cmds[at:]] and runner.loaded   # started after it
    assert sum(result["removed"].values()) == shown["rows"] and result["compacted"] is True
    assert _protected(world["home"]) == protected                        # memory, episodes, chat, tasks, traces...
    from tests.support.cards import sources_of
    from ferb.store.db import Database
    db = Database(world["home"] / "ferb.db")
    try:
        assert all(sources_of(db, v) == old["rule"] for v in old["versions"])
        for table in ("decisions", "perception_receipts", "perception_batches", "learning_vectors", "task_memory_refs"):
            assert db.conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0] == 0, table
        assert db.conn.execute("SELECT COUNT(*) FROM learning_nodes WHERE id = ?", (old["rollup"],)).fetchone()[0] == 0
        assert db.incremental()                                           # later passes hand pages back
        tables = {r[0] for r in db.conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}
        assert not tables & {"owner_questions", "notification_rules", "learning_receipts", "learning_jobs"}
        [cleaned] = [r[0] for r in db.conn.execute("SELECT data_json FROM journal WHERE kind = 'store.cleaned'")]
    finally:
        db.close()
    import json
    assert json.loads(cleaned)["rows"] == shown["rows"]                   # counts only
    assert sorted(json.loads(cleaned)["dropped"]) == ["learning_jobs", "learning_receipts", "notification_rules",
                                                      "owner_questions"]
    assert _journal_kinds(world["home"]) == ["store.cleaned", "store.compacted"]
    again = _preview(world)
    assert again["steps"] == [] and again["compact"] is False
    env, out = make_env(world, [])
    S.doctor(env)
    assert "✓ database: " in out.getvalue()


def test_the_cleanup_waits_while_another_daemon_answers_or_the_stopped_one_hasnt_exited(
        world: dict[str, Any], monkeypatch: pytest.MonkeyPatch) -> None:
    import math
    import ferb.device.install as install_mod
    monkeypatch.setattr(install_mod, "time", SteppedTime())
    world["inst"].install()
    _old_store(world["home"])
    ok = _preview(world)["ok"]
    code, text, result = _apply(world, ok, daemon_up=True)               # answers even with launchd's agent gone
    assert code == 1 and "a FERB daemon still answers" in text and world["runner"].loaded
    world["runner"].tail = math.inf                                      # its process never exits
    code, text, result = _apply(world, ok)
    assert code == 1 and "hasn't exited since launchd stopped it" in text and world["runner"].loaded
    assert _q(world["home"], "SELECT COUNT(*) FROM decisions") == 7 and _journal_kinds(world["home"]) == []
    world["runner"].tail = 3                                             # a slow exit, within the wait
    code, text, result = _apply(world, ok)
    assert code == 0, text
    assert _q(world["home"], "SELECT COUNT(*) FROM decisions") == 0


def test_quitting_during_the_cleanup_stops_it_starts_the_daemon_and_the_next_run_finishes(
        world: dict[str, Any], monkeypatch: pytest.MonkeyPatch) -> None:
    import os
    import signal
    from ferb.memory.learning import Learning
    world["inst"].install()
    old = _old_store(world["home"])
    ok = _preview(world)["ok"]
    real = Learning.prune_card_sources

    def quit_first(self: Learning, stop: Any = lambda: False, **kwargs: Any) -> dict[str, Any]:
        os.kill(os.getpid(), signal.SIGTERM)                             # FERB.app quits while it runs
        return real(self, stop, **kwargs)
    monkeypatch.setattr(Learning, "prune_card_sources", quit_first)
    env, out = make_env(world, [], daemon_up=False)
    with pytest.raises(KeyboardInterrupt):
        S.cleanup_apply(env, ok)
    assert world["runner"].loaded and signal.getsignal(signal.SIGTERM) is signal.SIG_DFL
    assert "stopped before it finished" in out.getvalue()
    assert _q(world["home"], "SELECT COUNT(*) FROM decisions") == 7       # stopped before the next step
    assert _journal_kinds(world["home"]) == ["store.cleaned"]             # what it did, marked stopped
    monkeypatch.setattr(Learning, "prune_card_sources", real)
    code, text, result = _apply(world, _preview(world)["ok"])            # the owner agrees to what is left
    assert code == 0, text
    assert _q(world["home"], "SELECT COUNT(*) FROM decisions") == 0 and old["versions"]


def test_without_room_to_compact_the_rows_go_and_a_later_run_compacts(world: dict[str, Any],
                                                                     monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.store.db import Database
    world["inst"].install()
    _old_store(world["home"])
    real = Database.compact
    monkeypatch.setattr(Database, "compact", lambda self, free=None: real(self, free=lambda folder: 0))
    code, text, result = _apply(world, _preview(world)["ok"])
    assert code == 0 and result["compacted"] is False and "Not compacted" in text, text
    assert _q(world["home"], "SELECT COUNT(*) FROM decisions") == 0
    shown = _preview(world)
    assert shown["steps"] == [] and shown["compact"] is True               # only the compaction is left
    monkeypatch.setattr(Database, "compact", real)
    code, text, result = _apply(world, shown["ok"])
    assert code == 0 and result["compacted"] is True, text
    assert _journal_kinds(world["home"]) == ["store.cleaned", "store.cleaned", "store.compacted"]
    assert _preview(world)["compact"] is False


# ---------- FERB.app's setup window (L34 C): `ferb setup --service` ----------

def test_the_setup_window_installs_the_service_without_asking_anything(app_world: dict[str, Any]) -> None:
    env, out = make_env(app_world, [], daemon_up=False)        # no answers: any question would fail the test
    assert S.service_setup(env) == 0, out.getvalue()
    text = out.getvalue()
    assert "FERB's background service is running." in text
    assert app_world["runner"].loaded
    assert app_world["panes"] == []                             # permissions are the window's own step
    calls = [m for m, _ in app_world["daemon"].calls]
    assert "setup.permission_request" not in calls and "setup.persona" not in calls and "doctor.check" not in calls
    assert (app_world["home"] / "device").is_dir()             # the app scan behind Automation's list
    assert Config(app_world["home"]).get("hearing", "requirement") == PINNED   # FERB Voice pinned for hearing


def test_the_setup_window_updates_the_service_quietly(app_world: dict[str, Any]) -> None:
    app_world["inst"].install()                                 # installed from this app before
    env, out = make_env(app_world, [])
    assert S.service_setup(env, update=True) == 0, out.getvalue()
    assert "Updating FERB's background service" in out.getvalue()
    assert [m for m, _ in app_world["daemon"].calls if m != "ping"] == []


def test_the_setup_window_refuses_a_disk_image_copy_with_its_fix(app_world: dict[str, Any], tmp_path: Path) -> None:
    from ferb.device.install import InstallError
    app_world["inst"] = Installer(app_world["home"], label="com.ferb.test", run=app_world["runner"],
                                  launch_agents=tmp_path / "LA", bundle=app_world["app"], cli_dir=tmp_path / "bin",
                                  read_only=lambda path: True)
    env, out = make_env(app_world, [], daemon_up=False)
    app_world["runner"].loaded = False
    with pytest.raises(InstallError, match="drag FERB.app to Applications"):
        S.service_setup(env)
    assert not app_world["runner"].loaded and not app_world["inst"].record_path.exists()


def test_the_setup_window_refuses_while_a_foreground_dev_daemon_holds_the_socket(world: dict[str, Any]) -> None:
    world["daemon"].pid = 999
    env, out = make_env(world, [])
    assert S.service_setup(env) == 1
    assert "outside launchd" in out.getvalue()


def test_ferb_setup_service_runs_the_quiet_step(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    from ferb.cli import main as cli_main
    ran: list[bool] = []
    monkeypatch.setenv("FERB_HOME", str(tmp_path / "home"))
    monkeypatch.setattr(S, "service_setup", lambda env, update=False: ran.append(update) or 0)
    assert cli_main(["setup", "--service", "--update"]) == 0
    assert cli_main(["setup", "--service"]) == 0
    assert ran == [True, False]
