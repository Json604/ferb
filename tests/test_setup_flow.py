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
from tests.conftest import FakeRunner, make_app


class FakeDaemon:
    """Answers like the real daemon; permissions flip to granted after the owner 'grants' them."""

    def __init__(self, pid: int = 4242) -> None:
        self.pid = pid
        self.perms = {"accessibility": "denied", "screen_recording": "denied", "input_monitoring": "granted",
                      "full_disk_access": "denied"}
        self.pending_sr = False          # granted in Settings but not applied until restart
        self.tap_ok = True
        self.automation_requests: list[list[str]] = []
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
                                 {"browser": "chrome", "name": "FERB", "folder": "Profile 22", "email": "",
                                  "ferb_identity": True, "paired": True, "identity": "ferb",
                                  "connected": False, "version": None, "refused": None}], "extension_ids": []}
        if method == "identity.check":
            return {"address": "ferb@test.example", "profile": "Profile 22", "profile_ok": True,
                    "profile_name": "FERB", "error": None, "session": None if params and params.get("session") is False else True, "paired": True,
                    "secrets": [{"key": "ferb/email/password", "in_keychain": True}]}
        if method == "automation.request":
            self.d.automation_requests.append(params["bundle_ids"])
            return {"results": {b: "granted" for b in params["bundle_ids"]}}
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
    Config(home).set("identity", "profile", "Profile 22")              # FERB's identity profile already chosen
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


def test_full_setup_collects_permissions_and_selected_automation_including_brave(world: dict[str, Any]) -> None:
    env, out = make_env(world, ["", "", "", "1,2,3"], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    d = world["daemon"]
    assert world["panes"][:2] == ["accessibility", "screen_recording"]
    assert d.perms["accessibility"] == d.perms["screen_recording"] == "granted"
    text = out.getvalue()
    assert set(d.automation_requests[0]) == {"com.brave.Browser", "com.google.Chrome", "com.apple.Notes"}                                    # progressive: asked on first use (8)
    assert "Optional" not in text and "identity profile" not in text and "4/4" in text
    assert "Setup complete." in text
    assert (world["home"] / "device").is_dir()


def test_skipping_a_required_permission_is_allowed_with_a_warning(world: dict[str, Any]) -> None:
    world["daemon"].perms["screen_recording"] = "granted"
    env, out = make_env(world, ["skip", "", ""], daemon_up=False)
    assert S.setup(env) == 0
    assert "Skipped Accessibility" in out.getvalue() and "cannot act on the Mac" in out.getvalue()


def test_input_monitoring_is_collected_at_setup(world: dict[str, Any]) -> None:
    d = world["daemon"]
    d.perms.update(accessibility="granted", screen_recording="granted", input_monitoring="denied")
    d.tap_ok = False
    env, out = make_env(world, ["skip", "", ""], daemon_up=False)
    S.setup(env)
    assert "input_monitoring" in world["panes"]


def test_doctor_reports_and_fails_on_missing_required_permission(world: dict[str, Any]) -> None:
    world["inst"].install()
    env, out = make_env(world, [])
    rc = S.doctor(env)
    text = out.getvalue()
    assert rc == 1
    assert "✗ permission accessibility: denied" in text and "✓ agent loaded" in text
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
    assert 'Permission setup deferred' in out.getvalue() and 'accessibility=denied' in out.getvalue()
    assert ('doctor.check', {'selftest': False}) in world['daemon'].calls


def test_update_recognizes_existing_grants_despite_an_older_setup_marker(world):
    world['inst'].install()
    config = Config(world['home'])
    config.set('setup', 'permissions_version', 22)
    world['daemon'].perms = {name: 'granted' for name in S.SETUP_PERMISSIONS}
    env, out = make_env(world, [])
    assert S.setup(env, update=True) == 0
    assert 'deferred' not in out.getvalue()
    assert 'All macOS permissions are granted' in out.getvalue()
    assert Config(world['home']).get('setup', 'permissions_version') == 27
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
    assert "✓ protection:" in out.getvalue() and "may read the never-read stores" in out.getvalue()
    Config(world["home"]).set("protection", "fs_reads_never_read", False)
    env, out = make_env(world, [])
    S.doctor(env)
    assert "✓ protection:" in out.getvalue() and "stay out of the never-read stores" in out.getvalue()
    Config(world["home"]).set("protection", "never_read", "~/.ssh")
    env, out = make_env(world, [])
    assert S.doctor(env) == 1
    assert "✗ protection: protection.never_read in config.toml must be a list of paths" in out.getvalue()


def test_update_reviews_missing_profile_once_without_repeating_permission_flow(world):
    world['inst'].install()
    env, out = make_env(world, [])
    calls, answers = [], iter(['evidence', 'I code and make music'])
    class ReviewClient(FakeClient):
        reviewed = False
        def call(self, method, params=None):
            calls.append((method, params))
            if method == 'setup.persona':
                return {'state': 'reviewed'} if self.reviewed else {'state': 'pending', 'approval': {
                    'approval_id': 'profile', 'reason': 'I found recorded use for 2 apps.',
                    'evidence': ['Code: 3733 Spotlight uses; last used 2026-10-10.']}}
            if method == 'chat.answer':
                self.reviewed = True
                return {'state': 'granted'}
            return super().call(method, params)
    client = ReviewClient(world['daemon'])
    env.connect = lambda: client
    env.ask = lambda _: next(answers)
    assert S.setup(env, update=True) == 0
    assert ('chat.answer', {'approval_id': 'profile', 'text': 'I code and make music'}) in calls
    assert '3733 Spotlight uses' in out.getvalue()
    assert world['panes'] == [] and world['daemon'].automation_requests == []
    assert S.setup(env, update=True) == 0  # already reviewed; no second question


def test_persona_only_review_does_not_install_restart_or_confirm_empty_input(world):
    env, out = make_env(world, [])
    calls = []
    class Client:
        def call(self, method, params):
            calls.append((method, params))
            return {'state': 'pending', 'approval': {'approval_id': 'profile', 'reason': 'Review local use.'}}
        def close(self): pass
    env.connect = Client
    env.ask = lambda _: ''
    assert S.persona_setup(env) == 0
    assert calls == [('setup.persona', {'refresh': True})]
    assert world['runner'].ran('uv', 'python', 'install') == 0
    assert 'still pending' in out.getvalue() and world['panes'] == []


# ---------- L26: FERB.app hears app sound during tasks (Speech Recognition only, unless --voice) ----------

def test_plain_setup_builds_ferb_app_and_asks_for_speech_recognition_only(world: dict[str, Any]) -> None:
    env, out = make_env(world, ["", "", "", "1,2,3"], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    run: FakeRun = world["run"]
    app = str(world["home"] / "FERB.app")
    assert [c[1] for c in run.calls if c[0].endswith("build.sh")] == [app]
    assert run.ran("/usr/bin/open", "-n", "-W", app, "--args", "request-speech")
    assert not any(c[-1] == "request" for c in run.ran("/usr/bin/open"))   # never the microphone without --voice
    assert Config(world["home"]).get("hearing", "requirement") == FakeRun.REQUIREMENT
    assert ("doctor.check", {"selftest": False, "hearing": True}) in world["daemon"].calls
    assert "Speech Recognition" in out.getvalue() and "on this Mac" in out.getvalue()


def test_setup_says_how_to_allow_speech_recognition_and_still_finishes(world: dict[str, Any]) -> None:
    world["daemon"].hearing.update(speech="denied")
    env, out = make_env(world, ["", "", "", "1,2,3"], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    assert "Privacy & Security" in out.getvalue() and "Speech Recognition" in out.getvalue()


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
    """The same Mac, with FERB installed from a packaged FERB.app instead of a checkout."""
    app = make_ferb_app(tmp_path / "Applications")
    runner = BundleRunner(world["home"])
    world.update(app=app, runner=runner, inst=Installer(world["home"], label="com.ferb.test", run=runner,
                                                        launch_agents=tmp_path / "LA", bundle=app,
                                                        cli_dir=tmp_path / "bin"))
    return world


def test_setup_from_the_app_installs_from_the_bundle_and_says_where_ferb_is(app_world: dict[str, Any]) -> None:
    env, out = make_env(app_world, ["", "", "", ""], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    text = out.getvalue()
    assert f"from {app_world['app']}" in text and "venv" not in text and "python3.12" not in text
    assert f"switch on FERB ({app_world['app']}/Contents/Helpers/FERB.app)" in text   # what permissions go to
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


def test_voice_setup_from_the_app_uses_its_signed_helper_without_building(app_world: dict[str, Any]) -> None:
    import subprocess
    ran: list[list[str]] = []

    def run(cmd: list[str], **kw: Any) -> "subprocess.CompletedProcess[str]":
        ran.append(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", f"# designated => {PINNED}\n" if "-r-" in cmd else "")

    class VoiceClient(FakeClient):
        def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
            if method == "voice.reload":
                return {"push_to_talk": True}
            if method == "doctor.check":
                return {"voice": {"connected": True, "permissions": {"microphone": "granted", "speech": "granted"}}}
            return super().call(method, params)
    env, out = make_env(app_world, [])
    env.connect = lambda: VoiceClient(app_world["daemon"])
    assert S.voice_setup(env, run=run) == 0, out.getvalue()
    helper = str(app_world["app"] / "Contents" / "Helpers" / "FERB Voice.app")
    assert not any(c[0] == "security" or c[0].endswith("build.sh") for c in ran)   # no local certificate, no build
    assert ["/usr/bin/open", "-n", "-W", helper, "--args", "request"] in ran     # integrated: L26 runs /usr/bin/open
    assert Config(app_world["home"]).get("voice", "requirement") == PINNED


def test_closing_setup_while_it_installs_never_leaves_the_daemon_unloaded(app_world: dict[str, Any]) -> None:
    """The app stops setup with SIGTERM when it quits: a stop that arrives while the background service is being
    (re)installed waits until it is loaded again, then setup stops."""
    import os
    import signal
    runner = app_world["runner"]
    real = runner.__call__

    def stop_midway(cmd: list[str]) -> Any:
        if cmd[:2] == ["launchctl", "bootout"]:
            os.kill(os.getpid(), signal.SIGTERM)           # the owner quits FERB between unload and load
        return real(cmd)
    app_world["inst"].run = stop_midway
    env, out = make_env(app_world, [], daemon_up=False)
    with pytest.raises(KeyboardInterrupt):
        S.setup(env)
    assert runner.loaded and runner.ran("launchctl", "bootstrap") == 1       # loaded before setup stopped
    assert signal.getsignal(signal.SIGTERM) is signal.SIG_DFL


def test_a_checkout_update_says_it_took_over_from_the_app(app_world: dict[str, Any], tmp_path: Path) -> None:
    app_world["inst"].install()
    source = Installer(app_world["home"], label="com.ferb.test", run=app_world["runner"], launch_agents=tmp_path / "LA")
    app_world["runner"].requirement = 'identifier "com.ferb.secrets" and certificate leaf = H"local"'   # its own build
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
    assert "venv" not in text and "✓ LaunchAgent: installed" in text


# ---------- packaged FERB.app × hearing (L26 + L27 integration) ----------

def test_plain_setup_from_the_app_hears_through_its_shipped_helper_without_building(app_world: dict[str, Any]) -> None:
    env, out = make_env(app_world, ["", "", "", ""], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    run: FakeRun = app_world["run"]
    helper = str(app_world["app"] / "Contents" / "Helpers" / "FERB Voice.app")
    assert not any(c[0] == "security" or c[0].endswith("build.sh") for c in run.calls)   # no certificate, no build
    assert run.ran("/usr/bin/open", "-n", "-W", helper, "--args", "request-speech")
    assert not any(c[-1] == "request" for c in run.ran("/usr/bin/open"))   # never the microphone without --voice
    assert Config(app_world["home"]).get("hearing", "requirement") == PINNED   # the shipped helper's signature
    assert not (app_world["home"] / "FERB.app").exists()
    assert "Speech Recognition for FERB Voice" in out.getvalue()


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
    binary = world["home"] / "node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex"
    binary.parent.mkdir(parents=True)
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
    assert world["run"].ran(str(auth.parent.parent / "node/node_modules/@openai/codex-darwin-arm64/vendor/"
                                "aarch64-apple-darwin/bin/codex"), "login")
    assert auth.exists() and "Setup complete." in out.getvalue()


def test_declining_the_codex_sign_in_says_how_to_do_it_later(world: dict[str, Any]) -> None:
    auth = _signed_out(world)
    env, out = make_env(world, ["", "", "", "1,2,3", "n"], daemon_up=False)
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

def test_setup_asks_before_letting_macos_ask_about_the_clipboard(world: dict[str, Any]) -> None:
    world["daemon"].clipboard = "default"
    asked: list[str] = []
    env, out = make_env(world, ["", "", "", "1,2,3", "y"], daemon_up=False)
    plain = env.ask
    env.ask = lambda prompt: (asked.append(prompt), plain(prompt))[1]   # type: ignore[method-assign]
    assert S.setup(env) == 0, out.getvalue()
    assert any("Paste from Other Apps" in p or "clipboard" in p.lower() for p in asked)
    assert world["daemon"].clipboard_requests == 1
    assert "Paste from Other Apps" in out.getvalue() and "Allow" in out.getvalue()


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
