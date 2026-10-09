"""`ferb setup` / `doctor` / `uninstall` flows with a scripted fake daemon, fake installer and scripted answers."""

import io
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

    def client(self) -> "FakeClient":
        return FakeClient(self)


class FakeClient:
    def __init__(self, d: FakeDaemon) -> None:
        self.d = d

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.d.calls.append((method, params))
        if method == "ping":
            return {"pid": self.d.pid}
        if method == "doctor.check":
            st = [] if params and params.get("selftest") is False else [
                {"name": "ax_read", "ok": True, "detail": "ok"},
                {"name": "event_tap", "ok": self.d.tap_ok, "detail": "listen-only tap; no input posted"}]
            return {"permissions": dict(self.d.perms), "selftest": st, "executable": "/x/python3.12",
                    "code_hash": "h", "stopped": False, "pid": self.d.pid}
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
    return {"inst": inst, "runner": runner, "daemon": daemon, "panes": panes, "home": home}


def make_env(world: dict[str, Any], answers: list[str], daemon_up: bool = True) -> tuple[S.Env, io.StringIO]:
    out = io.StringIO()
    d: FakeDaemon = world["daemon"]
    answers = list(answers)

    def ask(prompt: str) -> str:
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
                running_apps=lambda: {"com.google.Chrome", "com.brave.Browser", "com.apple.Notes"})
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
    assert "permission full_disk_access (optional)" in text
    assert "✓ browser chrome/FERB: FERB's own profile: paired, not open right now" in text   # not a warning
    world["daemon"].perms.update(accessibility="granted", screen_recording="granted")
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
    assert "✓ protection:" in out.getvalue() and "stay out of the never-read stores" in out.getvalue()
    Config(world["home"]).set("protection", "never_read", "~/.ssh")
    env, out = make_env(world, [])
    assert S.doctor(env) == 1
    assert "✗ protection: protection.never_read in config.toml must be a list of paths" in out.getvalue()
