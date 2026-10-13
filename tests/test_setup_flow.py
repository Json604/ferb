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
                    "containment": {"sandbox_exec": True, "profile_ok": True, "reason": "ok", "shell": True,
                                    "shell_reason": "ok", "proxy": True, "proxy_alive": True, "snapshots": False}}
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
