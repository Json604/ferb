"""The live preview (kb area app): guards for what live use can't show. An abandoned or waiting request never holds
up the worker; no frame after the task ends or for a tab the task doesn't own; the preview freezes with a state when
FERB leaves the surface, pauses or asks; nothing is persisted; only FERB.app's own process may call it; a declaration
that fails never touches the call."""
import asyncio
import base64
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.locks import ResourceLocks
from ferb.act.point import PreviewTarget
from ferb.core.errors import Refused, Withheld
from ferb.daemon.call_hooks import CallHooks
from ferb.daemon.task_preview import TaskPreviews


async def _plain(image: bytes) -> bytes:
    return image


def _previews(locks: ResourceLocks | None = None, limit_s: float = 2.0) -> tuple[TaskPreviews, ResourceLocks]:
    held = locks or ResourceLocks()
    return TaskPreviews(_plain, held.try_hold, limit_s=limit_s), held


def _target(scope: str, capture: Any, label: str = "Notes") -> PreviewTarget:
    async def resolve() -> tuple[dict[str, Any], Any]:
        return {"scope": scope}, capture
    return PreviewTarget(scope, label, resolve)


# decision: DMN-403
def test_a_request_never_queues_behind_the_worker_and_an_abandoned_one_frees_the_surface() -> None:
    """The 10-30 slow calls (kb area app): requests queued on the worker's locks and ran ahead of its next call.
    Now a request while the worker acts answers at once with the last frame (busy); a request whose caller leaves
    cancels its capture, so the worker's next call takes the lock at once."""
    async def check() -> None:
        previews, locks = _previews()
        entered, release = asyncio.Event(), asyncio.Event()

        async def first() -> bytes:
            return b"first"
        previews.declare("t", "t.worker", _target("app:Notes", first), ("app:com.apple.Notes",))
        assert (await previews.frame("t"))["image"] == b"first"

        async with locks.hold(["app:com.apple.Notes"], "worker"):         # the worker is acting there
            answer = await asyncio.wait_for(previews.frame("t"), 0.5)       # answers at once, never waits
        assert answer["busy"] is True and answer["image"] == b"first" and answer["fresh"] is False

        async def stuck() -> bytes:
            entered.set()
            await release.wait()
            return b"late"
        previews.declare("t", "t.worker", _target("app:Notes", stuck), ("app:com.apple.Notes",))
        request = asyncio.create_task(previews.frame("t"))
        await entered.wait()
        request.cancel()                                                    # the app left
        with pytest.raises(asyncio.CancelledError):
            await request
        await asyncio.sleep(0)
        async with locks.hold(["app:com.apple.Notes"], "worker"):           # free again without waiting
            pass
        assert previews.counts.busy == 1 and previews.counts.frames == 1
    asyncio.run(asyncio.wait_for(check(), 5))


# decision: DMN-403, DMN-404
def test_a_slow_capture_ends_within_the_daemon_time_limit_with_the_last_frame() -> None:
    async def check() -> None:
        previews, _ = _previews(limit_s=0.2)

        async def ok() -> bytes:
            return b"kept"

        async def slow() -> bytes:
            await asyncio.sleep(5)
            return b"never"
        previews.declare("t", "t.worker", _target("app:Notes", ok))
        await previews.frame("t")
        previews.declare("t", "t.worker", _target("app:Notes", slow))
        answer = await asyncio.wait_for(previews.frame("t"), 1)
        assert answer["image"] == b"kept" and answer["problem"]["kind"] == "slow"
    asyncio.run(check())


# decision: DMN-405
def test_no_frame_after_the_task_ends_even_from_a_capture_already_running() -> None:
    async def check() -> None:
        previews, _ = _previews()
        entered, release = asyncio.Event(), asyncio.Event()

        async def capture() -> bytes:
            entered.set()
            await release.wait()
            return b"captured"
        previews.declare("t", "t.step.a", _target("tab:browser/7", capture))
        pending = asyncio.create_task(previews.frame("t"))
        await entered.wait()
        previews.remove({"t"})                                              # the task ended
        release.set()
        answer = await pending
        assert "image" not in answer and answer["state"] == "done"
        previews.status = lambda root, worker: ("done", "Done")
        assert "image" not in await previews.frame("t")
    asyncio.run(check())


# decision: DMN-405
def test_no_frame_for_a_tab_the_task_does_not_own() -> None:
    """The browser declaration resolves through the driver's ownership check with the calling task: a tab the task
    doesn't hold gives no frame, only why."""
    from ferb.act.browser.tools import BrowserSnapshot

    class Driver:
        def owned_by(self, tab: str, owner: str) -> str:
            raise Refused("that tab isn't this task's")

        def recording_source(self, tab: str, owner: str) -> Any:
            raise AssertionError("never captured")
    family = SimpleNamespace(prefix="browser", identity=SimpleNamespace(), read="browser.read")
    cap = object.__new__(BrowserSnapshot)
    cap.driver, cap.family = Driver(), family

    async def check() -> None:
        previews, _ = _previews()
        target = cap.preview_target(SimpleNamespace(tab="tab-9"), "t.step.a")
        assert target is not None and target.scope == "tab:browser/tab-9"
        previews.declare("t", "t.step.a", target)
        answer = await previews.frame("t")
        assert "image" not in answer and answer["problem"]["kind"] == "gone"
    asyncio.run(check())


# decision: DMN-404
def test_the_preview_freezes_with_a_state_when_ferb_leaves_the_surface_or_asks() -> None:
    """Decision 2: once FERB's call leaves the window (a call on no surface), or the task asks, waits or pauses, the
    preview shows FERB's last frame of it with what FERB is doing now, and captures nothing (no new messages stream)."""
    async def check() -> None:
        previews, _ = _previews()
        frames = iter([b"one", b"two", b"three"])

        async def capture() -> bytes:
            return next(frames)
        previews.declare("t", "t.worker", _target("app:WhatsApp", capture, "WhatsApp"))
        assert (await previews.frame("t"))["image"] == b"one"
        previews.declare("t", "t.worker", None)                             # a file or terminal call: left it
        answer = await previews.frame("t")
        assert answer["state"] == "working_elsewhere" and answer["image"] == b"one" and not answer["fresh"]
        previews.declare("t", "t.worker", _target("app:WhatsApp", capture, "WhatsApp"))
        previews.status = lambda root, worker: ("asking", "Asking you: Send it?")
        answer = await previews.frame("t")
        assert answer["state"] == "asking" and answer["line"] == "Asking you: Send it?" and answer["image"] == b"one"
        previews.status = lambda root, worker: None
        assert (await previews.frame("t"))["image"] == b"two"               # back at work there: live again
        assert previews.counts.frames == 2
    asyncio.run(check())


# decision: DMN-404
def test_parallel_steps_each_keep_their_surface_and_the_answer_names_the_step() -> None:
    async def check() -> None:
        previews, _ = _previews()

        async def a() -> bytes:
            return b"tab-a"

        async def b() -> bytes:
            return b"tab-b"
        previews.declare("t", "t.step.permit", _target("tab:browser/1", a))
        previews.declare("t", "t.step.term", _target("tab:browser/2", b))
        latest = await previews.frame("t")
        assert latest["image"] == b"tab-b" and latest["step"] == "term"
        chosen = await previews.frame("t", "t.step.permit")
        assert chosen["image"] == b"tab-a" and chosen["step"] == "permit"
    asyncio.run(check())


# decision: DMN-404, DMN-405
def test_a_withheld_frame_and_the_masking_note_are_said() -> None:
    async def check() -> None:
        previews, _ = _previews()

        async def masked() -> tuple[bytes, str]:
            return b"page", "masked by text recognition only"

        async def withheld() -> bytes:
            raise Withheld("its password field couldn't be painted over")
        previews.declare("t", "t.worker", _target("tab:browser/1", masked))
        assert (await previews.frame("t"))["masking"] == "masked by text recognition only"
        previews.declare("t", "t.worker", _target("tab:browser/1", withheld))
        answer = await previews.frame("t")
        assert answer["problem"]["kind"] == "withheld" and answer["image"] == b"page"
    asyncio.run(check())


def test_a_failing_declaration_never_touches_the_call() -> None:
    """A capability's declaration that raises is no surface; the hook never raises, so a successful call's result is
    never lost (kb area app: gateway.py:1446, 1457)."""
    class Boom:
        def preview_target(self, args: Any, owner: str, observed: Any = None) -> Any:
            raise RuntimeError("AX went away")
    gateway = SimpleNamespace(locks=ResourceLocks(), redact=lambda x: x)
    hooks = CallHooks(gateway)
    grant, rec = SimpleNamespace(task_id="t.worker"), SimpleNamespace(tree_id="t")
    hooks.preview(Boom(), SimpleNamespace(app="Notes"), grant, rec, [])
    hooks.preview(Boom(), SimpleNamespace(app="Notes"), grant, rec, [], {"windows": []})
    hooks.preview(object(), SimpleNamespace(), grant, rec, [])             # a tool with no surface at all
    assert asyncio.run(hooks.previews.frame("t"))["state"] == "working_elsewhere"


def test_a_snapshot_shows_the_window_it_reports_current_not_the_drivers_latest_tree(monkeypatch: Any) -> None:
    from tests.mackit import vision
    from ferb.act.mac.tools import MacSnapshot
    import ferb.sanitize.images
    d, ax, shots, *_ = vision(monkeypatch)
    asyncio.run(d.snapshot('Notes'))
    d._state['com.apple.Notes'].tree.window = 'old-window-from-concurrent-preview'
    ax.frame = lambda ref: (_ for _ in ()).throw(AssertionError('must use returned window id'))
    monkeypatch.setattr(ferb.sanitize.images, 'ImageRedactor', lambda redact: lambda image: image)
    cap = MacSnapshot(d)

    async def check() -> None:
        hooks = CallHooks(SimpleNamespace(locks=ResourceLocks(), redact=lambda x: x))
        grant, rec = SimpleNamespace(task_id='task.worker'), SimpleNamespace(tree_id='task')
        hooks.preview(cap, SimpleNamespace(app='Notes'), grant, rec, ['app:com.apple.Notes'])
        hooks.preview(cap, SimpleNamespace(app='Notes'), grant, rec, ['app:com.apple.Notes'],
                      {'windows': [{'id': 110, 'current': True}, {'id': 999, 'current': False}]})
        assert (await hooks.previews.frame('task'))['image'] == shots.jpeg
        assert shots.captured == [110]
    asyncio.run(check())


# decision: DMN-23, DMN-402, DMN-405
def test_only_ferb_apps_own_process_may_watch_previews_and_nothing_is_persisted(daemon: Any) -> None:
    """auth.APP_ONLY: any program running as the owner gets "unauthorized"; FERB.app's process (its code signature,
    app_peer.py; stood in for here) gets frames, and no pixel reaches the journal, the trace or a file."""
    from ferb.cli.client import Client, RpcError

    async def capture() -> bytes:
        return b"\xff\xd8masked-frame"
    previews = daemon.daemon.gateway.hooks.previews
    daemon.call_soon(lambda: previews.declare("t", "t.worker", _target("tab:browser/1", capture)))
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError) as refused:
            c.call("tasks.preview", {"task_id": "t"})
        assert "FERB.app only" in str(refused.value)
    finally:
        c.close()
    before = daemon.run(_counts(daemon))
    daemon.daemon.server.is_app = lambda token: token is not None          # FERB.app's signature, stood in for
    c = Client(daemon.sock)
    try:
        daemon.daemon.supervisor._tree_of["t"] = SimpleNamespace(id="t", worker=SimpleNamespace(request_id="r"))
        answer = c.call("tasks.preview", {"task_id": "t"})
    finally:
        c.close()
        daemon.daemon.supervisor._tree_of.pop("t", None)
    assert base64.b64decode(answer["image"]) == b"\xff\xd8masked-frame"
    assert daemon.run(_counts(daemon)) == before
    files = [p for p in daemon.daemon.home.rglob("*") if p.is_file() and b"masked-frame" in p.read_bytes()]
    assert not files


async def _counts(daemon: Any) -> tuple[int, int]:
    conn = daemon.daemon.db.conn
    return (conn.execute("SELECT COUNT(*) FROM journal").fetchone()[0],
            conn.execute("SELECT COUNT(*) FROM spans").fetchone()[0])


# decision: DMN-402
def test_the_app_peer_check_fails_closed_for_any_other_process() -> None:
    """The real check (Security framework, the peer's audit token): this test process is not FERB.app."""
    import ctypes
    import os
    import socket
    from pathlib import Path
    from ferb.daemon.app_peer import AppPeers, peer_token
    a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        token = peer_token(a)
        assert token is not None and len(token) == 32
        assert AppPeers()(token) is False and AppPeers()(None) is False
        # the positive control: against the signature of the code this test runs as, the same check passes
        libproc, path = ctypes.CDLL("/usr/lib/libproc.dylib"), ctypes.create_string_buffer(4096)
        assert libproc.proc_pidpath(os.getpid(), path, len(path)) > 0
        assert AppPeers(Path(path.value.decode()))(token) is True
        assert AppPeers(Path("/bin/ls"))(token) is False               # another program's signature: refused
    finally:
        a.close()
        b.close()


def test_mac_exact_element_preview_preserves_worker_references(monkeypatch):
    from tests.mackit import vision
    d, ax, shots, *_ = vision(monkeypatch)
    async def check():
        await d.snapshot('Notes')
        _, shot, _ = await d.screenshot('Notes')
        state = d._state['com.apple.Notes']
        element = f's{state.gen}e1'
        state.nodes[element] = SimpleNamespace(ref='observed-control')
        ax.element_window = lambda ref: 'exact-window'
        ax.frame = lambda ref: shots.bounds
        target, capture = d.preview_source('Notes', element=element)
        assert target['window_id'] == 110
        assert await capture() == shots.jpeg
        assert await capture() == shots.jpeg
        assert state.shot is shot
        assert element in state.nodes
    asyncio.run(check())


def test_mac_preview_without_exact_element_window_never_uses_front_window(monkeypatch):
    from tests.mackit import vision
    d, ax, shots, *_ = vision(monkeypatch)
    asyncio.run(d.snapshot('Notes'))
    state = d._state['com.apple.Notes']
    element = f's{state.gen}e1'
    state.nodes[element] = SimpleNamespace(ref='observed-control')
    ax.element_window = lambda ref: None
    shots.front_window = lambda *args: (_ for _ in ()).throw(AssertionError('no front fallback'))
    with pytest.raises(Refused):
        d.preview_source('Notes', element=element)


# decision: DMN-403
def test_the_app_leaving_mid_request_cancels_the_capture_through_the_socket(daemon: Any) -> None:
    """kb area app: an abandoned request (the app's socket closed) no longer runs on: the server ends the call and
    the capture with it, so nothing holds the surface for the worker's next call."""
    import json
    import threading
    import time
    from tests.conftest import raw, send
    started, cancelled = threading.Event(), threading.Event()

    async def stuck() -> bytes:
        started.set()
        try:
            await asyncio.sleep(30)
        except asyncio.CancelledError:
            cancelled.set()
            raise
        return b"never"
    previews = daemon.daemon.gateway.hooks.previews
    daemon.daemon.server.is_app = lambda token: True
    daemon.daemon.supervisor._tree_of["t"] = SimpleNamespace(id="t", worker=SimpleNamespace(request_id="r"))
    daemon.call_soon(lambda: previews.declare("t", "t.worker", _target("tab:browser/1", stuck), ("tab:browser/1",)))
    s, f = raw(daemon.sock)
    try:
        send(s, {"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "cli"}})
        assert json.loads(f.readline())["result"]["role"] == "cli"
        send(s, {"jsonrpc": "2.0", "id": 2, "method": "tasks.preview", "params": {"task_id": "t"}})
        assert started.wait(5)
    finally:
        f.close()
        s.close()                                                           # the app leaves
    assert cancelled.wait(5)
    end = time.monotonic() + 5
    while time.monotonic() < end and daemon.run(_locked(daemon)):
        time.sleep(0.02)
    assert not daemon.run(_locked(daemon))
    daemon.daemon.supervisor._tree_of.pop("t", None)


async def _locked(daemon: Any) -> bool:
    lock = daemon.daemon.gateway.locks._locks.get("tab:browser/1")
    return bool(lock is not None and lock.locked())


def _fake_app(root: Any, bundle_id: str = "com.ferb.FERB") -> Any:
    import plistlib
    app = root / "FERB.app"
    (app / "Contents" / "MacOS").mkdir(parents=True)
    (app / "Contents" / "_CodeSignature").mkdir()
    (app / "Contents" / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": bundle_id,
                                                                   "CFBundleExecutable": "FERB"}))
    (app / "Contents" / "MacOS" / "FERB").write_bytes(b"build 1")
    (app / "Contents" / "_CodeSignature" / "CodeResources").write_bytes(b"seal 1")
    return app


class _FakeSecurity:
    """The Security calls, stood in for: each FERB.app signature gets its own requirement; a token passes only the
    requirement it is signed for."""
    def __init__(self) -> None:
        self.derived: list[Any] = []

    def designated(self, bundle: Any) -> int:
        self.derived.append(bundle)
        return len(self.derived)

    def requirement(self, text: str) -> int:
        self.derived.append(text)
        return 100

    def satisfies(self, token: bytes, requirement: int) -> bool:
        return token == b"build-%d" % requirement + b"-" * (32 - len(b"build-%d" % requirement))


def _token(n: int) -> bytes:
    raw = b"build-%d" % n
    return raw + b"-" * (32 - len(raw))


# decision: DMN-402
def test_a_daemon_run_from_ferbs_venv_checks_the_installed_ferb_app_and_follows_its_updates(tmp_path: Any,
                                                                                             monkeypatch: Any) -> None:
    """The owner's install runs the daemon from ~/.ferb/venv: it compares the peer with the installed FERB.app's own
    signature, never with an identifier any ad-hoc-signed program can claim; an app update is followed without a
    daemon restart; with no FERB.app installed nobody passes."""
    import os
    from ferb.daemon.app_peer import AppPeers
    monkeypatch.setenv("FERB_HOME", str(tmp_path / ".ferb"))
    venv = tmp_path / ".ferb" / "venv"
    venv.mkdir(parents=True)
    security = _FakeSecurity()
    missing = AppPeers(prefix=venv, candidates=(tmp_path / "none" / "FERB.app",), security=lambda: security)
    assert missing.mode == "installed" and missing(_token(1)) is False and missing(_token(100)) is False
    app = _fake_app(tmp_path / "Applications")
    peers = AppPeers(prefix=venv, candidates=(tmp_path / "none" / "FERB.app", app), security=lambda: security)
    assert peers(_token(1)) is True and security.derived == [app]
    assert peers(_token(100)) is False                                # the identifier-only requirement never applies
    seal = app / "Contents" / "_CodeSignature" / "CodeResources"      # FERB.app updated: re-signed in place
    seal.write_bytes(b"seal 2")
    os.utime(seal, ns=(seal.stat().st_atime_ns, seal.stat().st_mtime_ns + 10_000_000))
    assert peers(_token(2)) is True and peers(_token(1)) is False      # the new build passes, the old one no more
    other = _fake_app(tmp_path / "Other", bundle_id="com.example.other")
    assert AppPeers(prefix=venv, candidates=(other,), security=lambda: security)(_token(1)) is False


# decision: DMN-402
def test_only_a_checkout_daemon_falls_back_to_the_notch_identifiers(tmp_path: Any, monkeypatch: Any) -> None:
    from ferb.daemon.app_peer import AppPeers
    monkeypatch.setenv("FERB_HOME", str(tmp_path / ".ferb"))
    repo = tmp_path / "FERB"
    (repo / ".venv").mkdir(parents=True)
    (repo / "pyproject.toml").write_text("[project]\n")
    assert AppPeers(prefix=repo / ".venv", candidates=(), security=_FakeSecurity).mode == "checkout"
    fake_checkout = tmp_path / ".ferb" / ".venv"                     # FERB's own home is never a checkout
    fake_checkout.mkdir(parents=True)
    (tmp_path / ".ferb" / "pyproject.toml").write_text("[project]\n")
    assert AppPeers(prefix=fake_checkout, candidates=(), security=_FakeSecurity).mode == "installed"
