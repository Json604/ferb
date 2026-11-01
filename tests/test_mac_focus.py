"""Step 12e, the Mac background/focus fixes (HAND-211-11, owner 2026-10-31; Rulings (step 12e, Mac
focus)). Fakes only: no real app is launched, activated or pressed, and no real input event is posted."""
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from ferb.act.mac.apps import RunningApp
from tests.browserkit import call, task
from tests.conftest import FakeCap, RunningDaemon


def _front(name: str, bundle: str, pid: int) -> dict[str, Any]:
    return {"unavailable": {}, "frontmost_app": {"pid": pid, "bundle_id": bundle, "name": name},
            "key_window": {"pid": pid, "ax_identity": pid}, "fullscreen": False, "active_space_epoch": 0}


# ---------- HAND-212, fix 3: an app that brought itself to the front is told to the worker, never put back ----------

def test_a_self_activation_during_a_background_call_reaches_the_worker_and_nothing_is_put_back(
        daemon: RunningDaemon) -> None:
    """Live 2026-10-31 (Slack uninstall): a background menu press made Finder come to the front by itself; the span
    recorded it but the worker never heard. Now its result says so, and FERB restores nothing and asks nothing."""
    d = daemon.daemon
    press = FakeCap("probe.press", "click", {"probe"})
    press.on_screen = True                                   # type: ignore[attr-defined]  # it can change the screen
    d.registry.capabilities[press.name] = press
    reads = [_front("Terminal", "com.apple.Terminal", 9040)] * 2 + [_front("Finder", "com.apple.finder", 96185)]
    seen: list[dict[str, Any]] = []

    def read() -> dict[str, Any]:
        seen.append(reads[min(len(seen), len(reads) - 1)])
        return dict(seen[-1])
    d.disturbance.read = read
    g = task(daemon, "x", {"probe"})
    daemon.run(d.disturbance.start("x"))                     # the task's baseline (Terminal in front)
    out = call(daemon, g, "probe.press")
    assert not out["is_error"], out
    told = out["content"]["screen_changed_during_call"]
    assert told["front_app"] == {"before": "Terminal", "after": "Finder"} and told["put_back"] is False
    assert len(seen) == 3                                    # baseline, before, after: no read to put anything back
    assert d.state.approvals.pending() == []                 # and no question
    quiet = call(daemon, g, "probe.press")                   # nothing changed this time: nothing said
    assert "screen_changed_during_call" not in quiet["content"]


# ---------- fixes 4 and 5: mac.open opens a file or address in the background; a windowless app is reopened ----------

class _Opening:
    """The parts of the apps module mac.open uses, faked: what was opened, and whether anything was activated."""

    def __init__(self, apps: Any) -> None:
        self.apps, self.opened, self.reopened, self.windows = apps, [], [], {11: True}

    def open_in(self, bundle_id: str, target: str) -> str | None:
        self.opened.append((bundle_id, target))
        return None

    def reopen(self, pid: int) -> str | None:
        self.reopened.append(pid)
        self.windows[pid] = True
        return None

    def launch(self, bundle_id: str) -> bool:
        return True

    def is_application(self, path: str) -> bool:
        return path.endswith(".app")

    def runs_when_opened(self, path: str) -> bool:
        """Launch Services' type facts, faked: a program, a script, an installer package, a profile, a shortcut."""
        return path.endswith((".command", ".jar", ".py", ".pkg", ".mobileconfig", ".shortcut"))


def _opener(monkeypatch: Any, windows: bool = True) -> tuple[Any, Any, Any]:
    from ferb.act.mac.tools import MacOpen
    from tests.mackit import mac
    d, ax, apps, posted = mac(monkeypatch)
    fake = _Opening(apps)
    fake.windows[11] = windows
    for name in ("open_in", "reopen", "launch", "is_application", "runs_when_opened"):
        setattr(apps, name, getattr(fake, name))
    apps.default_app_for = lambda path: "com.apple.Terminal" if path.endswith(".command") else "com.apple.TextEdit"
    apps.declares_shell = lambda bundle: False
    ax.ax_windows = lambda pid: ["w"] if fake.windows.get(pid) else []
    monkeypatch.setattr(d, "ensure_window", _quick(d))      # no 10 s wait for a window in a test
    return d, MacOpen(d), fake


def test_mac_open_opens_a_file_in_an_app_without_activating_anything(monkeypatch: Any, tmp_path: Any) -> None:
    import asyncio
    from types import SimpleNamespace
    d, tool, fake = _opener(monkeypatch)
    note = tmp_path / "hello.txt"
    note.write_text("hi")
    out = asyncio.run(tool.run(tool.Input(bundle_id="com.apple.Notes", path=str(note)),
                               SimpleNamespace(task_id="t1", terminal=None)))
    assert not out.is_error and out.content["path"] == str(note) and out.content["window"] is True
    assert fake.opened == [("com.apple.Notes", str(note))] and d.apps.activated == []
    assert tool.file_paths(tool.Input(bundle_id="com.apple.Notes", path="~/x.txt")) == [
        (str(Path.home() / "x.txt"), "read")]                # the protection lists see the file the app gets
    for bad in ({"url": "file:///etc/passwd"}, {"url": "javascript:alert(1)"}):
        with pytest.raises(ValidationError):
            tool.Input(bundle_id="com.apple.Safari", **bad)
    assert tool.precheck(tool.Input(bundle_id="com.apple.Notes", path="notes/x.txt"))   # relative: refused
    app = tmp_path / "Thing.app"                             # a program written to disk (the shell can't open apps)
    app.mkdir()
    out = asyncio.run(tool.run(tool.Input(bundle_id="com.apple.finder", path=str(app)),
                               SimpleNamespace(task_id="t1", terminal=None)))
    assert out.is_error and fake.opened == [("com.apple.Notes", str(note))]


def test_a_file_that_would_run_in_a_terminal_needs_the_owners_yes_to_its_contents(monkeypatch: Any,
                                                                                 tmp_path: Any) -> None:
    """A script opened in Terminal, or a .command opened through Finder, runs: the terminal question (D31) shows the
    file's exact contents, and without that Yes nothing opens; the same file in TextEdit only shows it."""
    import asyncio
    from types import SimpleNamespace
    from ferb.core.types import TerminalGrant
    d, tool, fake = _opener(monkeypatch)
    d.apps.apps.append(RunningApp("com.apple.Terminal", "Terminal", 21, False))
    d.apps.apps.append(RunningApp("com.apple.finder", "Finder", 22, False))
    script = tmp_path / "clean.command"
    script.write_text("rm -rf ~/Downloads/x\n")
    for app in ("com.apple.Terminal", "com.apple.finder"):
        inp = asyncio.run(tool.terminal_input(tool.Input(bundle_id=app, path=str(script)), "t1"))
        assert inp is not None and [f[0] for f in inp.files] == [str(script)] and "rm -rf" in inp.files[0][2]
        out = asyncio.run(tool.run(tool.Input(bundle_id=app, path=str(script)),
                                   SimpleNamespace(task_id="t1", terminal=None)))
        assert out.is_error and fake.opened == []
    yes = TerminalGrant(asyncio.run(tool.terminal_input(tool.Input(bundle_id="com.apple.Terminal", path=str(script)),
                                                        "t1")), "c1")
    assert not asyncio.run(tool.run(tool.Input(bundle_id="com.apple.Terminal", path=str(script)),
                                    SimpleNamespace(task_id="t1", terminal=yes))).is_error
    script.write_text("rm -rf ~\n")                          # changed after the Yes: the Yes doesn't cover it
    assert asyncio.run(tool.run(tool.Input(bundle_id="com.apple.Terminal", path=str(script)),
                                SimpleNamespace(task_id="t1", terminal=yes))).is_error
    note = tmp_path / "notes.txt"
    note.write_text("groceries")
    assert asyncio.run(tool.terminal_input(tool.Input(bundle_id="com.apple.Notes", path=str(note)), "t1")) is None


@pytest.mark.parametrize("name,app", [("tool.jar", "com.apple.JavaLauncher"), ("run.py", "org.python.PythonLauncher"),
                                      ("setup.pkg", "com.apple.installer"), ("wifi.mobileconfig", "com.apple.finder"),
                                      ("go.shortcut", "com.apple.shortcuts")])
def test_a_file_whose_type_runs_when_opened_asks_in_any_app(monkeypatch: Any, tmp_path: Any, name: str,
                                                            app: str) -> None:
    """Past bug (12e review): only a terminal app, or Finder handing the file to one, counted as running it, so a .jar
    the shell wrote opened in Java Launcher (which runs it) unasked. Now the file's type decides, whatever the app:
    the terminal question shows its contents, nothing opens without that Yes, and the Yes covers only those bytes."""
    import asyncio
    from types import SimpleNamespace
    from ferb.core.types import TerminalGrant
    d, tool, fake = _opener(monkeypatch)
    f = tmp_path / name
    f.write_bytes(b"PK\x03\x04\xff payload")
    args = tool.Input(bundle_id=app, path=str(f))
    inp = asyncio.run(tool.terminal_input(args, "t1"))
    assert inp is not None and inp.surface == "a file that can run when opened"
    assert inp.files[0][0] == str(f) and inp.files[0][2] == "(a binary file of 13 bytes)"
    assert asyncio.run(tool.run(args, SimpleNamespace(task_id="t1", terminal=None))).is_error and fake.opened == []
    yes = TerminalGrant(inp, "c1")
    f.write_bytes(b"changed")                                # not the bytes the owner saw
    assert asyncio.run(tool.run(args, SimpleNamespace(task_id="t1", terminal=yes))).is_error and fake.opened == []
    f.write_bytes(b"PK\x03\x04\xff payload")
    assert not asyncio.run(tool.run(args, SimpleNamespace(task_id="t1", terminal=yes))).is_error
    assert fake.opened == [(app, str(f))]


def test_a_running_app_with_no_window_is_asked_for_one_in_the_background(monkeypatch: Any) -> None:
    import asyncio
    from types import SimpleNamespace
    d, tool, fake = _opener(monkeypatch, windows=False)
    out = asyncio.run(tool.run(tool.Input(bundle_id="com.apple.Notes"), SimpleNamespace(task_id="t1", terminal=None)))
    assert out.content["window"] is False and "reopen=true" in out.content["next"] and fake.reopened == []
    out = asyncio.run(tool.run(tool.Input(bundle_id="com.apple.Notes", reopen=True),
                               SimpleNamespace(task_id="t1", terminal=None)))
    assert out.content["reopened"] is True and out.content["window"] is True and fake.reopened == [11]
    assert d.apps.activated == []                            # nothing was brought forward
    again = asyncio.run(tool.run(tool.Input(bundle_id="com.apple.Notes", reopen=True),
                                 SimpleNamespace(task_id="t1", terminal=None)))
    assert "reopened" not in again.content and fake.reopened == [11]   # it has a window: nothing sent


def _quick(d: Any) -> Any:
    real = d.ensure_window

    async def ensure(bundle_id: str, wait_s: float = 10.0) -> bool:
        return bool(await real(bundle_id, wait_s=0))
    return ensure


def test_mac_open_of_a_protected_file_asks_through_the_gateway(daemon: RunningDaemon, monkeypatch: Any) -> None:
    """The file goes through the same protection lists as the file tools (rule 10): a never-read folder asks the owner
    first, and on a No nothing opens."""
    from tests.conftest import Answers
    d, tool, fake = _opener(monkeypatch)
    daemon.daemon.registry.capabilities[tool.name] = tool
    g = task(daemon, "x", {"mac:read"})
    answers = Answers(daemon, granted=False)
    try:
        out = call(daemon, g, "mac.open", bundle_id="com.apple.Notes", path="~/.ssh/id_ed25519")
    finally:
        answers.stop()
    assert out["is_error"] and fake.opened == [] and len(answers.asked) == 1


# ---------- HAND-213: a sound change is never counted against FERB when the owner did it ----------

def test_the_owners_own_mute_or_input_coincident_change_records_no_sound_fact(daemon: RunningDaemon) -> None:
    """Only changes FERB could have caused: the owner muting a tab themselves (the browser's mutedInfo.reason "user",
    protocol 21) and a change while the owner was using the keyboard or mouse record nothing, even with a FERB call
    in flight; the same change with neither is still a fact."""
    from types import SimpleNamespace
    d = daemon.daemon
    sentinel = d.disturbance
    records: list[tuple[str, str, dict[str, Any]]] = []
    sentinel.record = lambda task, kind, data: records.append((task, kind, data))
    last_input: list[float | None] = [None]
    sentinel.owner_input = lambda: {"monitored": True, "recent": False, "last_event_wall": last_input[0]}

    async def acting() -> None:
        from ferb.daemon.disturbance import _Observation
        sentinel.active["x"] = _Observation()
        sentinel.call_started("x.worker", "span-1", "browser.use_tab")
    daemon.run(acting())
    browser = SimpleNamespace(name="chrome")

    def sound(**kw: Any) -> None:
        e = SimpleNamespace(ferb=False, tab_id=41, audible=True, muted=False, changed=["audible"], muted_reason=None)
        for k, v in kw.items():
            setattr(e, k, v)
        daemon.call_soon(d._tab_sound, browser, e)
        daemon.run(_settle())

    sound(muted=True, changed=["muted"], muted_reason="user")          # the owner muted it
    sound(muted=False, changed=["muted"], muted_reason="user")         # and unmuted it
    now = d.clock.now().timestamp()
    last_input[0] = now - 1.0                                           # the owner pressed space a second ago
    sound(audible=False)
    assert records == []
    last_input[0] = now - 60.0                                          # no input of theirs near: FERB's call was
    sound(audible=False)
    sound(muted=True, changed=["muted"], muted_reason="extension")     # an extension muted it, not the owner
    changes = [r[2]["changes"]["tab_sound"] for r in records if r[1] == "disturbance.change"]
    assert [sorted(k for k in c if k != "tab") for c in changes] == [["audible"], ["muted"]]


async def _settle() -> None:
    import asyncio
    await asyncio.sleep(0)


def test_a_runnable_file_opened_in_its_declared_editor_doesnt_ask(monkeypatch: Any, tmp_path: Any) -> None:
    """Review 2026-11-01 (side effect of the rule above): a script opened in an editor asked. Now the app that receives
    the file (the one named, or Finder's default app for it) declaring itself the file's Editor lifts the question;
    a launcher (Viewer), a terminal (Shell) or an unreadable app still asks, and a profile always does."""
    import asyncio
    from types import SimpleNamespace
    d, tool, fake = _opener(monkeypatch)
    d.apps.apps.append(RunningApp("com.apple.finder", "Finder", 22, False))
    editors = {("com.microsoft.VSCode", ".py"), ("com.apple.TextEdit", ".py"), ("com.apple.TextEdit", ".mobileconfig")}
    d.apps.edits = lambda bundle, path: any(bundle == b and path.endswith(e) for b, e in editors) and \
        not path.endswith(".mobileconfig")                   # apps.edits never lifts a profile
    script = tmp_path / "run.py"
    script.write_text("import os\n")
    profile = tmp_path / "wifi.mobileconfig"
    profile.write_text("<plist/>")

    def asks(app: str, path: Any) -> bool:
        return asyncio.run(tool.terminal_input(tool.Input(bundle_id=app, path=str(path)), "t1")) is not None
    assert not asks("com.microsoft.VSCode", script) and not asks("com.apple.finder", script)   # Finder: TextEdit
    assert asks("org.python.PythonLauncher", script) and asks("com.apple.TextEdit", profile)
    assert not asyncio.run(tool.run(tool.Input(bundle_id="com.microsoft.VSCode", path=str(script)),
                                    SimpleNamespace(task_id="t1", terminal=None))).is_error
    assert fake.opened == [("com.microsoft.VSCode", str(script))]


def test_an_apps_editor_role_is_read_from_its_document_types(tmp_path: Any) -> None:
    from ferb.act.mac import apps

    def conforms(kind: str, to: str) -> bool:
        return to in {"public.python-script": {"public.python-script", "public.script", "public.plain-text"},
                      "com.apple.rtfd": {"com.apple.rtfd"}}.get(kind, {kind})
    vscode = {"CFBundleDocumentTypes": [{"CFBundleTypeRole": "Editor", "CFBundleTypeExtensions": ["py", "sh"]}]}
    textedit = {"CFBundleDocumentTypes": [{"CFBundleTypeRole": "Editor",
                                           "LSItemContentTypes": ["public.plain-text", "com.apple.rtfd"]}]}
    launcher = {"CFBundleDocumentTypes": [{"CFBundleTypeRole": "Viewer", "LSItemContentTypes": ["public.script"]}]}
    terminal = {"CFBundleDocumentTypes": [{"CFBundleTypeRole": "Shell", "LSItemContentTypes": ["public.script"]}]}
    untyped = {"CFBundleDocumentTypes": [{"LSItemContentTypes": ["public.script"]}]}
    assert apps.editor_role(vscode, "public.python-script", "py", conforms)
    assert apps.editor_role(textedit, "public.python-script", "py", conforms)      # by conformance
    assert apps.editor_role(textedit, "com.apple.rtfd", "rtfd", conforms)
    for info in (launcher, terminal, untyped, {}, None):
        assert not apps.editor_role(info, "public.python-script", "py", conforms)
    # the real lookup, on scratch files: a declared Editor lifts a script, never a profile; an unknown app never does
    script, profile = tmp_path / "x.py", tmp_path / "p.mobileconfig"
    script.write_text("print(1)\n")
    profile.write_text("<plist/>")
    apps._document_types["test.ferb.editor"] = ({"CFBundleTypeRole": "Editor", "CFBundleTypeExtensions": ["py"],
                                                 "LSItemContentTypes": ["com.apple.mobileconfig", "public.data"]},)
    try:
        assert apps.edits("test.ferb.editor", str(script)) and not apps.edits("test.ferb.editor", str(profile))
        assert not apps.edits("test.ferb.no-such-app", str(script))
    finally:
        apps._document_types.pop("test.ferb.editor", None)
        apps._document_types.pop("test.ferb.no-such-app", None)
