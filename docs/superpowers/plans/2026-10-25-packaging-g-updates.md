# Packaging Phase G: Automatic Updates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FERB.app updates itself with Sparkle 2 from the public GitHub release feed, verified with FERB's own EdDSA
key and certificate, installing only when no task is running, then restarting the background service so a loop resumes
(L31); a Settings switch (default on) turns automatic installs off, and then FERB offers "Update available — Install".

**Architecture:** Sparkle (embedded `Sparkle.framework`, its standard updater with gentle reminders) finds, downloads
and verifies an update and then hands its install to `UpdateCoordinator`. The coordinator asks the daemon
`update.prepare`: the daemon answers what a restart would cut (running trees, except loops resting between rounds and
plan waits, and running FERB processes), or, when nothing, stops new work, quits FERB Voice and shuts down at once with
a clean exit that launchd doesn't restart. Only then is Sparkle told to install, so nothing runs from inside FERB.app
while Sparkle replaces it. Sparkle relaunches the new app, whose launch sees the new build and updates the background
service quietly (plan C's `ferb setup --service --update`); the daemon's start resumes suspended plans, loops included;
the notch says "Updated to <version>". FERB's shell can't touch FERB.app's preferences (where Sparkle keeps its
switches), and the feed URL comes only from the build.

**Tech Stack:** Sparkle 2 (Swift Package, exact pin), Swift 6 tools in Swift 5 mode, AppKit/SwiftUI; Python 3.12,
pytest; zsh build scripts; macOS `sandbox-exec`.

**Spec:** `docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md` (decision L34, approved by the owner on
2026-10-25: "all 7 parts in full, juat design them bare min. focus on correct working"), "Phase G — Automatic updates"
and "What a new user experiences" §6; owner 2026-10-24: "cant this be that, a new update came so update it??". Plans A-F
(`docs/superpowers/plans/2026-10-25-packaging-*.md`) are built before this one.

## How the install works (read before any task)

1. **Where FERB runs from.** The LaunchAgent `com.ferb.daemon` runs `FERB.app/Contents/Helpers/FERB.app/Contents/
   MacOS/FERB -m ferb.daemon`, which embeds `FERB.app/Contents/Resources/python` (`ferb/device/install.py:41-49,
   167-176`). `~/.ferb/venv` is only for a checkout install. The secret helper is copied to `~/.ferb/bin` and pinned,
   `~/.ferb/node` links into the app, and FERB Voice (`Contents/Helpers/FERB Voice.app`, `com.ferb.app`) runs from the
   bundle. So the daemon and FERB Voice execute code inside the bundle Sparkle replaces; Python imports modules lazily,
   so a daemon still running after the swap would load the new version's files into the old process.
2. **Never mid-task, nothing running from the bundle.** Sparkle's install waits for `UpdateCoordinator`, which calls
   `update.prepare`. The daemon refuses while a restart would cut work. When it accepts, in one step with no `await`
   before it, it sets `_shutting_down` (chat, timers and new trees stop), holds new work in the supervisor
   (`hold_new_work`), journals `update.prepared`, quits FERB Voice and sets `_stop`, so `Daemon.run()` shuts down
   cleanly (plans suspended, loops' state saved) and exits 0. launchd's `KeepAlive {SuccessfulExit: false}`
   (`install.py:266`) leaves a clean exit stopped. The app waits until `ping` fails (the socket is closed), then lets
   Sparkle install.
3. **Restart.** Sparkle relaunches the new FERB.app. Plan C's `AppSetupController.startAtLaunch()` sees
   `FERB_APP_BUILD` in the LaunchAgent differ from the new build (`AppSetupAction.update` → `.quietUpdate`) and runs
   `ferb setup --service --update` without a window: it verifies the bundle, copies and pins the new helpers (plan B
   makes that accept the release certificate), rewrites the LaunchAgent and loads it, and the daemon starts on the new
   code. The window opens only if that fails or a required grant is missing. `Daemon.start()` runs
   `supervisor.resume_plans()`: a loop resumes without a chat line (L31), its round re-run once if it was cut. When
   setup settles, plan F's `onSettled` fires and the notch posts "Updated to <version>" once.
4. **If the update fails after FERB stopped,** Sparkle's `didAbortWithError` reaches the coordinator, which restarts the
   service with plan C's `DaemonService.restart()` (kickstart, else bootstrap). If FERB.app itself never comes back,
   the background service starts at the next login (`RunAtLoad`), and FERB.app's next launch repairs it (`.update`
   when the daemon isn't running).
5. **Quit while an update waits.** With automatic installs, Sparkle installs a downloaded update when FERB.app quits.
   `applicationShouldTerminate` therefore asks the same question first: idle → FERB stops, the quit goes on; a task
   running → the owner chooses **Keep FERB Open** (the update installs when the task ends) or **Quit and Install**
   (`update.prepare {even_if_busy: true}`: the daemon stops now, as `ferb setup --update` always did).

## Global Constraints

- Read `CLAUDE.md` first. Never read `vibed/`. Work on `main`; commit messages cite `L34`.
- Free tools only: no Developer ID, no notarization. Sparkle verifies every update with FERB's EdDSA key and checks it
  carries the same certificate as the running app (plan B's "FERB Release Signing").
- The EdDSA private key is generated once by the owner (`scripts/sparkle-keys.sh`), kept in their login Keychain with
  an offline backup, and never enters the repository. Only the public key is committed
  (`scripts/sparkle-public-key.txt`).
- Only the public release list is requested: the feed is
  `https://github.com/Json604/ferb/releases/latest/download/appcast-<arch>.xml`; `SUEnableSystemProfiling` is false;
  the feed URL comes only from the build (`feedURLString(for:)`), never from a preference.
- Updates install only when no task is running (resting loops and plan waits excepted: they resume after the restart,
  L31); the service restarts afterwards. The only exception is the owner's own **Quit and Install** answer.
- Automatic installs are on by default; Settings → General → "Install updates automatically" turns them off; then the
  menu and Settings show "Update available — Install" and Sparkle's window appears only after the owner's click (rule
  17: a background app never takes focus by itself).
- Rule 10: FERB's shell and AppleScript can't write FERB.app's preference domains (where Sparkle keeps its switches),
  through `cfprefsd` or the plist file (Task 2).
- Sparkle's version is pinned exactly; a bump is its own reviewed change.
- No new behaviour limits. Two mechanism timings, named properties: `retryInterval` (60 s between asks while a task
  runs) and `exitWait` (up to 60 s for the daemon's socket to close).
- Each task: failing test first; Python `nice -n 10 uv run pytest <files> -q`; Swift
  `swift test --package-path ui/mac/FERBNotch --filter <Class>`. Before the last commit: `nice -n 10 uv run pytest -q`,
  `uv run mypy ferb/core`, `git diff --check`, `swift test --package-path ui/mac/FERBNotch`.
- Commit only the files the task names (other agents have uncommitted work); check `git diff --cached --stat`; end each
  message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Interfaces with other phases (assumptions; confirm against plans A/B/C/D)

Written against the plans as committed on 2026-10-25; the controller confirms them before execution.

- **Plan A (two architectures)** — confirm against plan A Task 5: `scripts/build-app.sh [--arch arm64|x86_64]` sets
  `$ARCH` and builds the notch with `SWIFT=(swift build --package-path "$NOTCH" -c release --arch "$ARCH" --scratch-path
  "$WORK/swift")`; `ferb_trim_app APP ARCH` thins every Mach-O file in `Contents` to `$ARCH` (Sparkle.framework's
  included, before signing) and keeps the framework; `scripts/build-dmg.sh` names the images `FERB-AppleSilicon.dmg`
  and `FERB-Intel.dmg`. Each architecture's app checks its own feed, `appcast-$ARCH.xml`.
- **Plan B (stable signing)** — confirm against plan B: (B1) releases use `SIGN_IDENTITY=release` ("FERB Release
  Signing", SHA-1 in `scripts/release-cert.sha1`); FERB.app's designated requirement is `identifier "com.ferb.FERB" and
  certificate leaf = H"<sha1>"`, which Sparkle checks each update against. (B2) `scripts/signing.sh` keeps `ferb_sign PATH
  [opts]` (adding the pinned requirement to a bundle with a `CFBundleIdentifier` in release mode), `ferb_sign_app APP
  WORK` and `ferb_entitlements`; plan B's notes say Phase G extends `ferb_sign_app` for Sparkle and gives the app
  `disable-library-validation`. (B3) `Installer.sealed_requirement` accepts FERB.app's own certificate, so the quiet
  service update works on a release-signed app.
- **Plan C (native setup)** — confirm against plan C Task 8: `AppSetupController` runs a quiet service update
  (`ferb setup --service --update`) when a new build launches and shows its window only on failure or a missing grant;
  `DaemonService` (`restart() async -> Bool`) is in `Sources/FERBNotch/SetupServices.swift`.
- **Plan D (menu bar)** — confirm against plan D Tasks 4 and 8: `MenuAction` and `StatusMenu.entries(state:paused:
  stopped:appName:canSetUp:hasWindow:)` are in `Sources/NotchCore/MenuBarState.swift`; `PanelCoordinator` rebuilds
  the menu in `menuNeedsUpdate` (`rebuildMenu`) and dispatches in `perform(_:)`. Task 5 adds `updateOffer` and
  `.installUpdate`.
- **Plan E (consumed):** `GeneralSettingsPage`, `SettingsView` (`SettingsPages.swift`), `FERBAppActions`
  (`FERBWindow.swift`), and `ScriptedRPC` with `replies`, `failures`, `queue(_:_:)`, `waits`, `params(of:)`, `methods()`.
- **Plan F (consumed):** `AppSetupController.onSettled: (() -> Void)?` and its wiring in `Application.swift`
  (`loginItems.applyDefaultAfterSetup()`).

## Review Focus

- A downloaded update waits and the owner quits FERB.app (or logs out) while a task runs: Sparkle would install on
  quit with the daemon mid-task. The quit must ask first, and Keep FERB Open must leave everything running. Pinned in
  Task 4 (`testQuittingWithATaskRunningAsksAndKeepFERBOpenInstallsNothing`).
- A chat message, a schedule's timer or a loop's probe trip arriving in the instant between `update.prepare` saying
  ready and the daemon's shutdown: nothing may start. Pinned in Task 1 (`test_an_idle_ferb_stops_for_the_update`, chat
  refused; `test_nothing_new_starts_once_the_update_holds`;
  `test_a_timer_that_fires_during_shutdown_is_kept_for_after_the_restart`).
- A long-lived FERB process (a dev server a task left running) when an update arrives: the restart would kill it, so it
  holds the update and the status names it. Pinned in Task 1 (`test_a_ferb_process_still_running_holds_the_update`).
- Sparkle refuses the update (bad EdDSA signature, other certificate) after FERB stopped for it: FERB must start again
  at once and say why. Pinned in Task 4 (`testAnUpdateThatFailedAfterFERBStoppedStartsFERBAgain`).
- A FERB shell command (from an injected page) runs `defaults write com.ferb.FERB SUAutomaticallyUpdate -bool NO` or
  writes the plist to point the feed elsewhere: refused by the sandbox, and the feed URL ignores preferences anyway.
  Pinned in Task 2 (`test_the_shell_cannot_write_ferb_apps_preferences`) and Task 4 (`testTheFeedComesOnlyFromTheBuild`).

---

### Task 1: The daemon's side: `update.prepare`, what holds an update, nothing new during the hold

**Files:**
- Modify: `ferb/orchestrator/supervisor.py` (`__init__` after `self._resting` at `:525`; new methods after
  `running_trees` at `:1012-1016`; `run_tree` at `:2117`; the loop round's `play` at `:2834`)
- Modify: `ferb/daemon/main.py` (`__init__` after `self._shutting_down = False` at `:410`; registration with plan E's
  lines; `run()` at `:630-638`; `_timer_fired` at `:2780-2785`; new section "updates" above `# ---------- methods`)
- Modify: `ferb/daemon/auth.py` (`METHODS[Role.CLI]`)
- Create: `tests/test_update_prepare.py`

**Interfaces:**
- Produces: RPC `update.prepare {"version"?: str, "even_if_busy"?: bool}` → `{"ready": bool, "running": [{"task_id",
  "goal"}]}`; when ready the daemon stops (journal `update.prepared {version, interrupted}`).
- Produces (Python): `Supervisor.update_blockers() -> list[dict[str, str]]`, `Supervisor.hold_new_work() -> None`,
  `Daemon._stop: asyncio.Event`, `Daemon._update_blockers()`.

- [ ] **Step 1: Write the failing tests** (create `tests/test_update_prepare.py`)

```python
"""L34 Phase G: FERB.app installs an update only when nothing runs that a restart would cut. `update.prepare` says what
it waits for, or holds new work, quits FERB Voice and stops the daemon at once. A loop resting between rounds and a plan
waiting for its time don't hold it (they resume after the restart, L31); the owner's Quit and Install stops even with a
task running."""

import asyncio
from datetime import timedelta
from types import SimpleNamespace
from typing import Any, Callable, Iterator

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core.errors import FerbError
from ferb.daemon import auth
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import RunningDaemon

LOOP = Plan(steps=[Step(id="keep", kind="loop", goal="Keep the tab playing")])


def _call(daemon: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    c = Client(daemon.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _on_loop(daemon: RunningDaemon, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


@pytest.fixture
def quits(daemon: RunningDaemon) -> list[int]:
    """FERB Voice is never really quit from a test (the owner's own may be running)."""
    calls: list[int] = []
    daemon.daemon._quit_voice_app = lambda: calls.append(1)            # type: ignore[method-assign]
    return calls


@pytest.fixture
def trees(daemon: RunningDaemon) -> Iterator[Callable[..., None]]:
    """Running trees as run_tree registers them (and their plans' step states); removed before the daemon stops."""
    sv = daemon.daemon.supervisor
    assert sv is not None
    seeded: list[str] = []

    def seed(tree_id: str, goal: str, plan: Plan | None = None, running: tuple[str, ...] = (),
             waiting: tuple[str, ...] = (), resting: tuple[str, ...] = ()) -> None:
        def go() -> None:
            sv._tree_of[tree_id] = SimpleNamespace(id=tree_id, spec=SimpleNamespace(goal=goal), watch=False, stop=None)
            if plan is not None:
                sv.plans.create(tree_id, plan, {"spec": {}, "request_id": "r-" + tree_id, "words": goal,
                                                "payload": goal}, None)
                sv._plan_runs[tree_id] = SimpleNamespace(wake=lambda: None)
                for step in running:
                    sv.plans.begin(tree_id, step)
                for step in waiting:
                    sv.plans.finish(tree_id, step, {"status": "waiting"})
            sv._resting.update(f"{tree_id}.loop.{step}" for step in resting)
        _on_loop(daemon, go)
        seeded.append(tree_id)
    yield seed

    def clear() -> None:
        for tree_id in seeded:
            sv._tree_of.pop(tree_id, None)
            sv._plan_runs.pop(tree_id, None)
            sv._resting.difference_update({r for r in set(sv._resting) if r.startswith(tree_id + ".")})
    _on_loop(daemon, clear)


def _prepared(daemon: RunningDaemon) -> list[Any]:
    return _on_loop(daemon, lambda: daemon.daemon.journal.of_kind("update.prepared"))   # type: ignore[union-attr]


def test_an_idle_ferb_stops_for_the_update(daemon: RunningDaemon, quits: list[int]) -> None:
    assert _call(daemon, "update.prepare", {"version": "0.28.0"}) == {"ready": True, "running": []}
    assert daemon.daemon._stop.is_set() and daemon.daemon._shutting_down
    assert quits == [1]
    assert _prepared(daemon)[-1].data == {"version": "0.28.0", "interrupted": []}
    c = Client(daemon.sock, timeout=10)
    try:
        with pytest.raises(RpcError, match="shutting down"):
            list(c.stream("chat.send", {"session_id": "s1", "text": "rename my invoices"}))
    finally:
        c.close()


def test_a_running_task_holds_the_update_and_nothing_stops(daemon: RunningDaemon, quits: list[int],
                                                           trees: Callable[..., None]) -> None:
    trees("tree-a", "Rename the invoices")
    assert _call(daemon, "update.prepare", {"version": "0.28.0"}) == {
        "ready": False, "running": [{"task_id": "tree-a", "goal": "Rename the invoices"}]}
    assert not daemon.daemon._stop.is_set() and not daemon.daemon._shutting_down and quits == []
    assert _prepared(daemon) == []


def test_a_loop_resting_between_rounds_does_not_hold_the_update(daemon: RunningDaemon, quits: list[int],
                                                                trees: Callable[..., None]) -> None:
    trees("tree-l", "Keep the tab playing", LOOP, running=("keep",), resting=("keep",))
    assert _call(daemon, "update.prepare", {})["ready"] is True


def test_a_loop_in_its_round_holds_the_update(daemon: RunningDaemon, quits: list[int],
                                              trees: Callable[..., None]) -> None:
    trees("tree-r", "Keep the tab playing", LOOP, running=("keep",))
    reply = _call(daemon, "update.prepare", {})
    assert reply["ready"] is False and reply["running"][0]["task_id"] == "tree-r"


def test_a_plan_waiting_for_its_time_does_not_hold_the_update(daemon: RunningDaemon, quits: list[int],
                                                              trees: Callable[..., None]) -> None:
    plan = Plan(steps=[Step(id="later", kind="wait", goal="Wait until 9", until="2026-10-26T09:00:00+00:00"),
                       Step(id="post", kind="do", goal="Post the update", depends_on=["later"])])
    trees("tree-w", "Post at 9", plan, waiting=("later",))
    assert _call(daemon, "update.prepare", {})["ready"] is True


def test_a_plan_doing_a_step_beside_its_resting_loop_holds_the_update(daemon: RunningDaemon, quits: list[int],
                                                                      trees: Callable[..., None]) -> None:
    plan = Plan(steps=[Step(id="keep", kind="loop", goal="Keep it playing"), Step(id="sum", kind="do", goal="Summarize")])
    trees("tree-d", "Two things", plan, running=("keep", "sum"), resting=("keep",))
    assert _call(daemon, "update.prepare", {})["ready"] is False


def test_a_tree_whose_plan_has_not_started_holds_the_update(daemon: RunningDaemon, quits: list[int],
                                                            trees: Callable[..., None]) -> None:
    trees("tree-p", "Plan something")                     # its planner turn is still running: no plan run yet
    assert _call(daemon, "update.prepare", {})["ready"] is False


def test_a_ferb_process_still_running_holds_the_update(daemon: RunningDaemon, quits: list[int]) -> None:
    proc = SimpleNamespace(running=True, owner_task="tree-old.worker", command="npm run dev")
    daemon.daemon.shells.list = lambda: [proc]                         # type: ignore[union-attr,method-assign]
    reply = _call(daemon, "update.prepare", {})
    assert reply == {"ready": False, "running": [{"task_id": "tree-old.worker", "goal": "process: npm run dev"}]}


def test_the_owners_quit_and_install_stops_even_with_a_task_running(daemon: RunningDaemon, quits: list[int],
                                                                    trees: Callable[..., None]) -> None:
    trees("tree-a", "Rename the invoices")
    reply = _call(daemon, "update.prepare", {"version": "0.28.0", "even_if_busy": True})
    assert reply["ready"] is True and reply["running"][0]["task_id"] == "tree-a"
    assert daemon.daemon._stop.is_set()
    assert _prepared(daemon)[-1].data["interrupted"] == ["tree-a"]


def test_nothing_new_starts_once_the_update_holds(daemon: RunningDaemon, quits: list[int]) -> None:
    from ferb.orchestrator.schemas import TaskSpec
    from tests.conftest import task
    _call(daemon, "update.prepare", {})
    sv = daemon.daemon.supervisor
    assert sv is not None

    async def start() -> None:
        gen = sv.run_tree(TaskSpec.model_validate(task("Rename the invoices")), "x", "req-held")
        try:
            await asyncio.wait_for(gen.__anext__(), 0.3)
        finally:
            await gen.aclose()
    with pytest.raises(asyncio.TimeoutError):
        daemon.run(start())


def test_a_timer_that_fires_during_shutdown_is_kept_for_after_the_restart(daemon: RunningDaemon) -> None:
    """Before, a timer due while FERB shut down was acknowledged unhandled: a reminder was lost."""
    d = daemon.daemon
    assert d.timers is not None
    d._shutting_down = True
    timer = _on_loop(daemon, lambda: d.timers.add("reminder", d.clock.now() - timedelta(seconds=1), "Call the bank"))
    with pytest.raises(FerbError, match="fires after the restart"):
        daemon.run(d.timers.tick())
    assert _on_loop(daemon, lambda: d.timers.get(timer.id).state) == "claimed"
    d._shutting_down = False
    daemon.run(d.timers.tick())
    assert _on_loop(daemon, lambda: d.timers.get(timer.id).state) == "delivered"


def test_only_the_owners_connection_may_stop_ferb_for_an_update() -> None:
    assert auth.allowed(auth.Role.CLI, "update.prepare")
    for role in (auth.Role.MCP_SHIM, auth.Role.VOICE, auth.Role.NATIVE_HOST):
        assert not auth.allowed(role, "update.prepare")
```

- [ ] **Step 2: Run them to see them fail**

Run: `nice -n 10 uv run pytest tests/test_update_prepare.py -q`
Expected: FAIL (`update.prepare not allowed for cli`; `Daemon` has no `_stop`).

- [ ] **Step 3: The supervisor**

In `Supervisor.__init__`, after `self._resting: set[str] = set() ...` (`:525`), add:

```python
        # L34 Phase G: open until an update's hold; a new tree and a loop's next round wait on it, and the shutdown
        # that follows the hold cancels them (a loop's round then runs after the restart, L31)
        self._work_open = asyncio.Event()
        self._work_open.set()
```

After `running_trees` (`:1016`), add:

```python
    def hold_new_work(self) -> None:
        """L34 Phase G: FERB stops for an update now. Nothing new starts: run_tree and a loop's round wait."""
        self._work_open.clear()

    def update_blockers(self) -> list[dict[str, str]]:
        """L34 Phase G: the running trees an update must wait for (watch checks included). A tree is left out only when
        each step it has in progress is a plan wait or a loop resting between rounds: a clean shutdown suspends its plan
        and it resumes after the restart as it was (L31). A tree whose plan hasn't started runs a planner turn."""
        return [{"task_id": tree_id, "goal": goal} for tree_id, goal in self.running_trees()
                if not self._restart_safe(tree_id)]

    def _restart_safe(self, tree_id: str) -> bool:
        if tree_id not in self._plan_runs:
            return False
        kinds = {step.id: step.kind for step in self.plans.load(tree_id)['plan'].steps}
        for key, state in self.plans.steps(tree_id).items():
            if key.startswith(EARLIER) or state['status'] not in ('running', 'waiting'):
                continue
            step = key.split(':', 1)[0]                      # a map batch's row is "<step>:<n>"
            if kinds.get(step) == 'wait' and state['status'] == 'waiting':
                continue
            if kinds.get(step) == 'loop' and f'{tree_id}.loop.{step}' in self._resting:
                continue
            return False
        return True
```

(`EARLIER` is already imported from `ferb.orchestrator.plan_store`, `supervisor.py:50`.)

In `run_tree`, make the first statement after the docstring:

```python
        await self._work_open.wait()                     # L34 Phase G: an update's hold (the shutdown ends the wait)
```

In the loop round's `async def play(cause: dict[str, Any]) -> dict[str, Any]:` (`:2834`), make the first statement:

```python
            await self._work_open.wait()                 # L34 Phase G: no round starts during an update's hold
```

- [ ] **Step 4: The daemon**

`ferb/daemon/auth.py`: in `METHODS[Role.CLI]`, add `"update.prepare"`.

`ferb/daemon/main.py`:
- After `self._shutting_down = False` (`:410`): `        self._stop = asyncio.Event()   # run() waits on it: SIGTERM, SIGINT or update.prepare (L34 Phase G)`
- Replace `run()` (`:630-638`) with:

```python
    async def run(self) -> None:
        """Run until SIGTERM / SIGINT, or until an update stops FERB (`update.prepare`), then shut down cleanly. The exit
        is clean, so launchd doesn't start the daemon again (KeepAlive SuccessfulExit false)."""
        await self.start()
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(sig, self._stop.set)
        await self._stop.wait()
        await self.shutdown()
```

- In `_timer_fired`, replace

```python
        if self._shutting_down:
            return
```

  with

```python
        if self._shutting_down:          # left claimed, not acknowledged: it fires after the restart (L34 Phase G)
            raise FerbError("FERB is shutting down; this timer fires after the restart")
```

  (`Timers.run` keeps a timer claimed when its consumer raises, and `claim_due` takes claimed timers again; a sleep
  left claimed is cancelled at start-up, as before.)
- After plan E's registration lines add `        self.server.unary['update.prepare'] = self._update_prepare`.
- Above `    # ---------- methods ----------`, add:

```python
    # ---------- updates (L34 Phase G) ----------

    def _update_blockers(self) -> list[dict[str, str]]:
        """What an update must wait for: the supervisor's running trees (update_blockers), and each FERB process still
        running whose tree isn't listed already (a long-lived server outlives its task; a restart would kill it)."""
        running = self.supervisor.update_blockers() if self.supervisor is not None else []
        listed = {r["task_id"] for r in running}
        if self.shells is not None:
            for p in self.shells.list():
                if p.running and p.owner_task.split(".", 1)[0] not in listed:
                    running.append({"task_id": p.owner_task,
                                    "goal": "process: " + self._redact_text(p.command)[:120]})
        return running

    async def _update_prepare(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """FERB.app is about to install an update. Ready only when nothing runs that a restart would cut, unless the
        owner chose Quit and Install (`even_if_busy`). When ready, in one step: nothing new starts, FERB Voice quits (it
        runs from the app being replaced) and the daemon shuts down at once. The new FERB.app's quiet service update
        starts it on the new code, and suspended plans, loops included, resume (L31)."""
        running = self._update_blockers()
        if running and params.get("even_if_busy") is not True:
            return {"ready": False, "running": running}
        self._shutting_down = True                   # before any await: chat, timers and new trees stop here
        if self.supervisor is not None:
            self.supervisor.hold_new_work()
        self._journal().append("update.prepared", {"version": str(params.get("version") or "")[:40],
                                                    "interrupted": [r["task_id"] for r in running]})
        with contextlib.suppress(Exception):
            self._quit_voice_app()
        self._stop.set()
        return {"ready": True, "running": running}
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `nice -n 10 uv run pytest tests/test_update_prepare.py tests/test_loops.py tests/test_auth.py -q`
Expected: PASS. If an existing test expected a timer due during shutdown to be acknowledged unhandled, it pinned the
lost-reminder bug: change it to expect the timer kept claimed.

- [ ] **Step 6: Commit**

```bash
git add ferb/orchestrator/supervisor.py ferb/daemon/main.py ferb/daemon/auth.py tests/test_update_prepare.py
git diff --cached --stat
git commit -m "feat(L34): update.prepare: FERB stops for an update only when no task runs; resting loops and plan waits resume after

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: FERB's shell can't change FERB.app's preferences (rule 10)

**Files:**
- Modify: `ferb/act/shell/sandbox.py` (constants after `APPS_LOCKDOWN` at `:103`; `Sandbox.__init__` at `:128-133`;
  `_profile` after the FERB.app rule at `:178-179`; module docstring)
- Create: `tests/test_app_preferences_sandbox.py`

**Interfaces:**
- Produces: `sandbox.APP_PREFERENCE_DOMAINS = ("com.ferb.FERB", "com.ferb.app", "com.ferb.daemon",
  "dev.ferb.notch")`; `Sandbox(..., app_domains=APP_PREFERENCE_DOMAINS)` (attribute `app_domains`). Both the shell's
  profile and mac.applescript's carry `(deny user-preference-write (preference-domain "<d>"))` and
  `(deny file-write* (literal "~/Library/Preferences/<d>.plist"))` for each domain, after every lift.

- [ ] **Step 1: Write the failing tests** (create `tests/test_app_preferences_sandbox.py`)

```python
"""L34 Phase G, rule 10: FERB's shell can't change FERB.app's own preferences (Sparkle's update switches live there),
neither through cfprefsd (`defaults write`) nor by writing the plist file. Measured with the real sandbox-exec on a
throwaway preference domain; the owner's real FERB domains are never written."""

import shutil
import subprocess
import uuid
from pathlib import Path

import pytest

from ferb.act.shell.sandbox import APP_PREFERENCE_DOMAINS
from tests.conftest import RunningDaemon
from tests.support.sandbox_probes import run_under

pytestmark = pytest.mark.skipif(not shutil.which("sandbox-exec"), reason="sandbox-exec is not available here")


def test_ferb_apps_preference_domains_are_listed() -> None:
    assert APP_PREFERENCE_DOMAINS == ("com.ferb.FERB", "com.ferb.app", "com.ferb.daemon", "dev.ferb.notch")


def test_both_profiles_deny_them(daemon: RunningDaemon) -> None:
    sb = daemon.daemon.shells.sandbox                                    # type: ignore[union-attr]
    for text in (sb.profile(), sb.profile(apps=True)):
        assert '(deny user-preference-write (preference-domain "com.ferb.FERB"))' in text
        assert 'Library/Preferences/com.ferb.FERB.plist"))' in text


def test_the_shell_cannot_write_ferb_apps_preferences(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    domain = f"com.ferb.sandboxtest.{uuid.uuid4().hex[:8]}"
    sb = daemon.daemon.shells.sandbox                                    # type: ignore[union-attr]
    monkeypatch.setattr(sb, "app_domains", (domain,))
    plist = Path.home() / "Library" / "Preferences" / f"{domain}.plist"
    try:
        assert run_under(sb, ["/usr/bin/true"]).returncode == 0          # the profile compiles
        run_under(sb, ["/usr/bin/defaults", "write", domain, "SUAutomaticallyUpdate", "-bool", "NO"])
        read = subprocess.run(["/usr/bin/defaults", "read", domain, "SUAutomaticallyUpdate"],
                              capture_output=True, text=True)
        assert read.returncode != 0, "the sandboxed write reached cfprefsd"
        made = run_under(sb, ["/usr/bin/plutil", "-create", "xml1", str(plist)])
        assert made.returncode != 0 and not plist.exists()
        other = run_under(sb, ["/usr/bin/defaults", "read", "-g", "AppleLocale"])
        assert "Operation not permitted" not in other.stderr             # other preferences stay readable
    finally:
        subprocess.run(["/usr/bin/defaults", "delete", domain], capture_output=True)
        plist.unlink(missing_ok=True)
```

- [ ] **Step 2: Run them to see them fail**

Run: `nice -n 10 uv run pytest tests/test_app_preferences_sandbox.py -q`
Expected: FAIL: `ImportError: cannot import name 'APP_PREFERENCE_DOMAINS'`.

- [ ] **Step 3: Implement** (`ferb/act/shell/sandbox.py`)

After `APPS_LOCKDOWN = (...)` add:

```python
# FERB.app's own preferences (rule 10, L34 Phase G): Sparkle keeps FERB's update switches there, and the app its own.
# The shell and AppleScript never write these domains, through cfprefsd or the plist files.
APP_PREFERENCE_DOMAINS = ("com.ferb.FERB", "com.ferb.app", "com.ferb.daemon", "dev.ferb.notch")
```

In `Sandbox.__init__`, add the keyword argument `app_domains: tuple[str, ...] = APP_PREFERENCE_DOMAINS` after
`listeners`, and set `self.app_domains = app_domains`.

In `_profile`, right after the FERB.app line
(`lines += [f"(deny file-write* (subpath {_q(f)}))" for app in p.app_entries() for f in _forms(app)]`), add:

```python
        # FERB.app's preferences (rule 10, L34 Phase G): after every lift, never lifted
        lines += [f"(deny user-preference-write (preference-domain {_q(d)}))" for d in self.app_domains]
        lines += [f"(deny file-write* (literal {_q(f)}))" for d in self.app_domains
                  for f in _forms(p.home / "Library" / "Preferences" / f"{d}.plist")]
```

Add one bullet to the module docstring after the Trash bullet:

```
- FERB.app's preference domains (APP_PREFERENCE_DOMAINS: Sparkle's update switches and the app's own) can't be written,
  through cfprefsd (`user-preference-write`) or their plist files (rule 10, L34 Phase G; measured by
  tests/test_app_preferences_sandbox.py on a throwaway domain).
```

- [ ] **Step 4: Run the tests to see them pass — this is the measurement**

Run: `nice -n 10 uv run pytest tests/test_app_preferences_sandbox.py tests/test_shell_sandbox.py -q`
Expected: PASS. If the profile doesn't compile with `user-preference-write`, or the `defaults write` still reaches
`cfprefsd`, stop: report the measured output to the owner and don't ship a weaker rule (rule 10).

- [ ] **Step 5: Commit**

```bash
git add ferb/act/shell/sandbox.py tests/test_app_preferences_sandbox.py
git diff --cached --stat
git commit -m "feat(L34, rule 10): FERB's shell can't write FERB.app's preferences, where Sparkle keeps its update switches

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Sparkle in the build: the package, the bundle, signing, the update key, notices

**Files:**
- Modify: `ui/mac/FERBNotch/Package.swift`
- Create: `ui/mac/FERBNotch/Package.resolved` (written by SwiftPM)
- Modify: `ui/mac/FERBNotch/build.sh` (the development build)
- Modify: `scripts/build-app.sh` (after the notch UI block plan A rewrote)
- Modify: `scripts/signing.sh` (`ferb_entitlements`; new `ferb_sign_sparkle`; `ferb_sign_app`)
- Create: `scripts/sparkle-keys.sh`
- Modify: `.gitignore`, `THIRD_PARTY_NOTICES.md`, `ui/mac/FERBNotch/ThirdPartyNotices.txt`
- Create: `tests/support/signing_stubs.py`
- Create: `tests/test_update_packaging.py`

**Interfaces:**
- Produces: the `FERBNotch` target depends on the `Sparkle` product; FERB.app carries
  `Contents/Frameworks/Sparkle.framework`, rpath `@executable_path/../Frameworks`, and (release builds) the Info.plist
  keys `SUFeedURL`, `SUPublicEDKey`, `SUEnableAutomaticChecks`, `SUAutomaticallyUpdate`, `SUScheduledCheckInterval`,
  `SUEnableSystemProfiling`; `FERB_UPDATES=off` builds without them. `ferb_sign_sparkle APP` in `signing.sh`.

- [ ] **Step 1: Pick the exact Sparkle version**

Run: `gh api repos/sparkle-project/Sparkle/releases --jq '.[] | select(.prerelease == false) | .tag_name + " " + .published_at' | head -8`
Use the newest `2.x.y` published at least 14 days before today, and no older than `2.6.4`. Write it in place of
`2.6.4` in Step 4. (The APIs this plan uses — `SPUStandardUpdaterController`, Task 4's delegate methods and gentle
reminders — exist since Sparkle 2.2.)

- [ ] **Step 2: Write the failing tests**

Create `tests/support/signing_stubs.py`:

```python
"""Stub codesign and security for signing-step tests (L34 Phase G), the shapes tests/test_packaging.py uses: every
codesign call is logged to $STUB_LOG, the entitlements a call gives are copied beside it, and `-dv` reports ad-hoc."""

import shutil
from pathlib import Path

STUB_CODESIGN = r"""#!/bin/zsh
print -r -- "codesign|${(j:|:)@}" >> "$STUB_LOG"
if [[ " $* " == *" -dv "* ]]; then print -u2 "Signature=adhoc"; exit 0; fi
for (( i = 1; i < $#; i++ )); do
  if [[ "${@[i]}" == --entitlements ]]; then cp "${@[i+1]}" "$STUB_DIR/ent${${@[-1]}//\//_}"; fi
done
exit 0
"""
STUB_SECURITY = r"""#!/bin/zsh
print -r -- "${STUB_IDENTITIES:-}"
print "     0 valid identities found"
"""


def stub_env(tmp_path: Path) -> dict[str, str]:
    stubs = tmp_path / "stubs"
    stubs.mkdir()
    for name, body in (("codesign", STUB_CODESIGN), ("security", STUB_SECURITY)):
        (stubs / name).write_text(body)
        (stubs / name).chmod(0o755)
    return {"PATH": f"{stubs}:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(tmp_path / "home"),
            "STUB_LOG": str(tmp_path / "calls.log"), "STUB_DIR": str(tmp_path)}


def signed_calls(tmp_path: Path) -> list[str]:
    """The codesign calls that wrote a signature, in order."""
    log = tmp_path / "calls.log"
    calls = log.read_text().splitlines() if log.exists() else []
    return [c for c in calls if c.startswith("codesign") and "|--sign|" in c and "--verify" not in c]


def entitlements(tmp_path: Path, signed: Path) -> str:
    return (tmp_path / ("ent" + str(signed).replace("/", "_"))).read_text()


def macho(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy("/usr/bin/true", path)
```

Create `tests/test_update_packaging.py`:

```python
"""L34 Phase G: Sparkle ships inside FERB.app, signed inside out as Sparkle documents for apps signed outside Xcode;
release builds name FERB's public feed and key and send no system profile; the development build never updates; the
private update key never goes in the repository; Sparkle's license ships."""

import json
import os
import plistlib
import re
import subprocess
from pathlib import Path

import pytest

from tests.support.signing_stubs import entitlements, macho, signed_calls, stub_env

ROOT = Path(__file__).resolve().parent.parent
BUILD_APP = ROOT / "scripts" / "build-app.sh"
NOTCH = ROOT / "ui" / "mac" / "FERBNotch"


def _fake_app(root: Path) -> Path:
    app = root / "FERB.app"
    c = app / "Contents"
    macho(c / "MacOS" / "FERBNotch")
    b = c / "Frameworks" / "Sparkle.framework" / "Versions" / "B"
    for rel in ("Sparkle", "Autoupdate", "Updater.app/Contents/MacOS/Updater",
                "XPCServices/Installer.xpc/Contents/MacOS/Installer",
                "XPCServices/Downloader.xpc/Contents/MacOS/Downloader"):
        macho(b / rel)
    (c / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "com.ferb.FERB",
                                                    "CFBundleExecutable": "FERBNotch"}))
    return app


def test_sparkle_is_signed_inside_out_before_the_app(tmp_path: Path) -> None:
    app = _fake_app(tmp_path / "out")
    done = subprocess.run([str(BUILD_APP), "--sign-only", str(app)], capture_output=True, text=True,
                          env=stub_env(tmp_path))
    assert done.returncode == 0, done.stderr + done.stdout
    calls = signed_calls(tmp_path)
    paths = [c.split("|")[-1] for c in calls]
    fw = app / "Contents" / "Frameworks" / "Sparkle.framework"
    b = fw / "Versions" / "B"
    order = [str(b / "XPCServices" / "Installer.xpc"), str(b / "XPCServices" / "Downloader.xpc"), str(b / "Autoupdate"),
             str(b / "Updater.app"), str(fw)]
    assert [p for p in paths if p in order] == order
    assert paths[-1] == str(app)
    assert "|--preserve-metadata=entitlements|" in next(c for c in calls if c.endswith("Downloader.xpc"))
    inner = ("/Versions/B/Sparkle", "/MacOS/Updater", "/MacOS/Installer", "/MacOS/Downloader")
    assert not any(p.endswith(inner) for p in paths)              # signed with their bundles, never one by one
    assert all("|--options|runtime|" in c for c in calls)
    # self-signed code has no Team ID, so library validation would refuse Sparkle.framework (as the daemon's modules)
    assert "com.apple.security.cs.disable-library-validation" in entitlements(tmp_path, app)


def test_release_builds_name_the_public_feed_and_key_and_send_no_profile() -> None:
    text = BUILD_APP.read_text()
    assert "https://github.com/Json604/ferb/releases/latest/download/appcast-$ARCH.xml" in text
    for key in ("SUPublicEDKey", "SUEnableAutomaticChecks bool true", "SUAutomaticallyUpdate bool true",
                "SUScheduledCheckInterval integer 86400", "SUEnableSystemProfiling bool false"):
        assert key in text, key
    assert "-add_rpath @executable_path/../Frameworks" in text
    assert "SUFeedURL" not in (NOTCH / "Info.plist").read_text()          # the development build never updates


def test_sparkle_is_pinned_exactly() -> None:
    text = (NOTCH / "Package.swift").read_text()
    assert re.search(r'\.package\(url: "https://github\.com/sparkle-project/Sparkle", exact: "2\.\d+\.\d+"\)', text)
    assert '.product(name: "Sparkle", package: "Sparkle")' in text
    pins = json.loads((NOTCH / "Package.resolved").read_text())["pins"]
    assert [p["identity"] for p in pins] == ["sparkle"]


def test_the_private_update_key_stays_outside_the_checkout() -> None:
    done = subprocess.run([str(ROOT / "scripts" / "sparkle-keys.sh"), "--export", str(ROOT / "key-backup")],
                          capture_output=True, text=True)
    assert done.returncode == 1 and "outside this checkout" in done.stderr
    assert not (ROOT / "key-backup").exists()
    assert "sparkle_private" in (ROOT / ".gitignore").read_text()


def test_sparkles_license_ships() -> None:
    for notices in (ROOT / "THIRD_PARTY_NOTICES.md", NOTCH / "ThirdPartyNotices.txt"):
        assert "Sparkle" in notices.read_text(), notices


@pytest.mark.skipif(not os.environ.get("FERB_APP_BUNDLE"), reason="set FERB_APP_BUNDLE=/path/FERB.app to check a build")
def test_a_built_app_embeds_sparkle_and_its_feed() -> None:
    app = Path(os.environ["FERB_APP_BUNDLE"])
    info = plistlib.loads((app / "Contents" / "Info.plist").read_bytes())
    assert info["SUFeedURL"].startswith("https://github.com/Json604/ferb/releases/latest/download/appcast-")
    assert info["SUPublicEDKey"] and info["SUEnableSystemProfiling"] is False
    assert (app / "Contents" / "Frameworks" / "Sparkle.framework").is_dir()
    loads = subprocess.run(["otool", "-l", str(app / "Contents" / "MacOS" / "FERBNotch")], capture_output=True,
                           text=True).stdout
    assert "@executable_path/../Frameworks" in loads
    assert subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)]).returncode == 0
```

- [ ] **Step 3: Run them to see them fail**

Run: `nice -n 10 uv run pytest tests/test_update_packaging.py -q`
Expected: FAIL (no Sparkle signing, no feed keys, no pin, no key script).

- [ ] **Step 4: The package** (`ui/mac/FERBNotch/Package.swift`, whole file)

```swift
// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "FERBNotch",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "FERBNotch", targets: ["FERBNotch"])],
    dependencies: [
        // Automatic updates (L34 Phase G). Pinned exactly; a bump is its own reviewed change.
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.6.4"),
    ],
    targets: [
        .target(name: "NotchCore"),
        .target(name: "AudioCapture"),
        .executableTarget(name: "FERBNotch", dependencies: ["NotchCore", "AudioCapture",
                                                            .product(name: "Sparkle", package: "Sparkle")]),
        .target(name: "AudioCaptureTestSupport", path: "Tests/AudioCaptureTestSupport"),
        .testTarget(name: "NotchCoreTests", dependencies: ["NotchCore", "FERBNotch", "AudioCapture", "AudioCaptureTestSupport"])
    ],
    swiftLanguageModes: [.v5]
)
```

If plans A-F changed other lines of this file, keep their changes and add only the `dependencies` entry and the
`Sparkle` product. Then run `swift package --package-path ui/mac/FERBNotch resolve` (writes `Package.resolved`) and
`swift build --package-path ui/mac/FERBNotch` (Expected: builds; `.build/debug/Sparkle.framework` exists).

- [ ] **Step 5: The development build carries the framework** (`ui/mac/FERBNotch/build.sh`)

After `cp "$BINARY_DIR/FERBNotch" "$APP_DIR/Contents/MacOS/FERBNotch"` add:

```zsh
# Sparkle.framework (L34 Phase G): the binary links it; a development build has no feed, so it never checks
mkdir -p "$APP_DIR/Contents/Frameworks"
rm -rf "$APP_DIR/Contents/Frameworks/Sparkle.framework"
ditto "$BINARY_DIR/Sparkle.framework" "$APP_DIR/Contents/Frameworks/Sparkle.framework"
install_name_tool -add_rpath @executable_path/../Frameworks "$APP_DIR/Contents/MacOS/FERBNotch"
```

- [ ] **Step 6: The release bundle** (`scripts/build-app.sh`)

After the notch block's `cp "$ROOT/THIRD_PARTY_NOTICES.md" "$CONTENTS/Resources/THIRD_PARTY_NOTICES.md"` add (before
plan A's trim step, so the framework is thinned to `$ARCH` and checked with the rest):

```zsh
print "==> Sparkle (automatic updates, L34 Phase G)"
NOTCH_BIN=$("${SWIFT[@]}" --show-bin-path)           # plan A's swift build for $ARCH
[[ -d "$NOTCH_BIN/Sparkle.framework" ]] || die "swift build left no Sparkle.framework in $NOTCH_BIN"
mkdir -p "$CONTENTS/Frameworks"
ditto "$NOTCH_BIN/Sparkle.framework" "$CONTENTS/Frameworks/Sparkle.framework"
install_name_tool -add_rpath @executable_path/../Frameworks "$CONTENTS/MacOS/FERBNotch"
if [[ "${FERB_UPDATES:-on}" == off ]]; then
  print "    FERB_UPDATES=off: this build never checks for updates"
else
  KEY_FILE="$ROOT/scripts/sparkle-public-key.txt"
  [[ -s "$KEY_FILE" ]] || die "no $KEY_FILE: the owner runs scripts/sparkle-keys.sh once (or build with FERB_UPDATES=off)"
  # One feed per architecture; only GitHub's public release list is asked; no system profile is sent (L34)
  plist "Add :SUFeedURL string https://github.com/Json604/ferb/releases/latest/download/appcast-$ARCH.xml" "$CONTENTS/Info.plist"
  plist "Add :SUPublicEDKey string $(<"$KEY_FILE")" "$CONTENTS/Info.plist"
  plist "Add :SUEnableAutomaticChecks bool true" "$CONTENTS/Info.plist"
  plist "Add :SUAutomaticallyUpdate bool true" "$CONTENTS/Info.plist"
  plist "Add :SUScheduledCheckInterval integer 86400" "$CONTENTS/Info.plist"
  plist "Add :SUEnableSystemProfiling bool false" "$CONTENTS/Info.plist"
fi
```

- [ ] **Step 7: Signing** (`scripts/signing.sh`, as plan B left it)

In `ferb_entitlements`, change the library-validation condition to include the app, with its comment:

```zsh
  # Ad-hoc or self-signed code (no Developer ID) has no Team ID, so library validation would refuse every extension
  # module the interpreter loads, and the app's Sparkle.framework (L34 Phase G). One Developer ID team signs all of
  # them, so validation stays on in that build.
  if [[ $1 == (daemon|python|app) ]] && ! ferb_developer_id; then keys+=(com.apple.security.cs.disable-library-validation); fi
```

After `ferb_vendor_signed`, add:

```zsh
# ferb_sign_sparkle APP: Sparkle.framework (L34 Phase G), signed inside out as Sparkle documents for apps signed outside
# Xcode: its XPC services (the Downloader keeps its entitlements), Autoupdate, Updater.app, then the framework.
ferb_sign_sparkle() {
  local fw="$1/Contents/Frameworks/Sparkle.framework"
  [[ -d "$fw" ]] || return 0
  local b="$fw/Versions/B"
  if [[ -d "$b/XPCServices/Installer.xpc" ]]; then ferb_sign "$b/XPCServices/Installer.xpc"; fi
  if [[ -d "$b/XPCServices/Downloader.xpc" ]]; then
    ferb_sign "$b/XPCServices/Downloader.xpc" --preserve-metadata=entitlements
  fi
  ferb_sign "$b/Autoupdate"
  ferb_sign "$b/Updater.app"
  ferb_sign "$fw"
}
```

In `ferb_sign_app`, change the `find` so the framework is left to `ferb_sign_sparkle`:

```zsh
  files=("${(@f)$(find "$app/Contents" \( -path "$app/Contents/Frameworks" -o -name '*.app' \) -prune -o -type f \( -name '*.so' -o -name '*.dylib' -o -perm -u+x \) -print \
                  | awk -F/ '{ print NF "\t" $0 }' | sort -rn | cut -f2-)}")
```

and add `  ferb_sign_sparkle "$app"` on the line before `  local nested id`.

- [ ] **Step 8: The update key script** (create `scripts/sparkle-keys.sh`, `chmod 755`)

```zsh
#!/bin/zsh -f
# FERB's update key (L34 Phase G), made once by the owner. Sparkle's generate_keys keeps the private EdDSA key in the
# login Keychain and prints the public key, which this writes to scripts/sparkle-public-key.txt for build-app.sh (a
# public key: commit that file). The private key never enters this repository.
#
#   scripts/sparkle-keys.sh                 make the key once (or read the existing one), write the public key file
#   scripts/sparkle-keys.sh --export FILE   back the private key up to FILE, outside this checkout; keep it offline
set -euo pipefail
ROOT="${0:A:h:h}"
NOTCH="$ROOT/ui/mac/FERBNotch"
die() { print -u2 "sparkle-keys.sh: $*"; exit 1 }

OUT=""
if [[ "${1:-}" == --export ]]; then
  OUT="${${2:?usage: sparkle-keys.sh --export FILE}:A}"
  [[ "$OUT" != "$ROOT" && "$OUT" != "$ROOT"/* ]] || die "keep the private key outside this checkout ($ROOT)"
fi
swift package --package-path "$NOTCH" resolve >/dev/null
TOOL=$(find "$NOTCH/.build/artifacts" -path '*/Sparkle/bin/generate_keys' -type f 2>/dev/null | head -1)
[[ -n "$TOOL" && -x "$TOOL" ]] || die "Sparkle's generate_keys isn't under $NOTCH/.build/artifacts"
if [[ -n "$OUT" ]]; then
  "$TOOL" -x "$OUT"
  chmod 600 "$OUT"
  print "Private key written to $OUT. Keep it offline: it signs every FERB update."
  exit 0
fi
PUBLIC=$("$TOOL" -p 2>/dev/null || true)
if [[ -z "$PUBLIC" ]]; then
  "$TOOL" >/dev/null
  PUBLIC=$("$TOOL" -p)
fi
print -r -- "$PUBLIC" > "$ROOT/scripts/sparkle-public-key.txt"
print "Public key: $PUBLIC (in scripts/sparkle-public-key.txt; commit it). Back up the private key with --export."
```

`.gitignore`, under "# Secrets and local config", add:

```
# FERB's update signing key (L34 Phase G) belongs in the owner's Keychain and an offline backup, never here
sparkle_private*
*.sparkle-private*
```

- [ ] **Step 9: Notices**

Append to `THIRD_PARTY_NOTICES.md` and to `ui/mac/FERBNotch/ThirdPartyNotices.txt` (the copy FERB.app ships):

```
Sparkle — https://github.com/sparkle-project/Sparkle (version from ui/mac/FERBNotch/Package.swift)
Embedded in FERB.app as Sparkle.framework for automatic updates (L34 Phase G).
```

followed by the full text of `ui/mac/FERBNotch/.build/checkouts/Sparkle/LICENSE`, copied verbatim (it covers Sparkle
and the code it includes).

- [ ] **Step 10: Run the tests to see them pass**

Run: `nice -n 10 uv run pytest tests/test_update_packaging.py tests/test_packaging.py -q` and
`swift test --package-path ui/mac/FERBNotch`.
Expected: PASS (the existing signing tests still pass: the framework prune only skips a folder their fake app lacks).

- [ ] **Step 11: Commit**

```bash
git add ui/mac/FERBNotch/Package.swift ui/mac/FERBNotch/Package.resolved ui/mac/FERBNotch/build.sh scripts/build-app.sh scripts/signing.sh scripts/sparkle-keys.sh .gitignore THIRD_PARTY_NOTICES.md ui/mac/FERBNotch/ThirdPartyNotices.txt tests/support/signing_stubs.py tests/test_update_packaging.py
git diff --cached --stat
git commit -m "feat(L34): Sparkle in FERB.app: pinned package, embedded and signed inside out, the public feed and key, the owner's key script

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The updater: `UpdatePlan`, `UpdateCoordinator`, `SparkleUpdater`

**Files:**
- Create: `ui/mac/FERBNotch/Sources/NotchCore/UpdatePlan.swift`
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/UpdateCoordinator.swift`
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/SparkleUpdater.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/UpdatePlanTests.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/UpdateCoordinatorTests.swift`

**Interfaces:**
- Consumes: Task 1's `update.prepare`; Task 3's Sparkle product; plan E's `ScriptedRPC`; plan C's `DaemonService`.
- Produces (NotchCore): `UpdatePlan.feedPrefix`, `feedURL(_ info:) -> String?`, `UpdatePlan.Prepared` (`.ready`,
  `.waiting([String])`), `prepared(_:)`, `waitingLine(version:work:)`, `updatedNote(previousBuild:build:version:)`.
- Produces (app): `@MainActor protocol UpdaterDriving` (`automaticallyDownloadsUpdates`, `checkForUpdates()`);
  `@MainActor final class UpdateCoordinator: ObservableObject` (`status: Status` with `line`, `automatic`,
  `installArmed`, `prepared`, `attach(_:)`, `found(version:)`, `notFound()`, `checkNow()`,
  `readyToInstall(version:install:) -> Task<Void, Never>`, `aborted(_:)`, `beforeQuit(_:)`, `retryInterval`,
  `exitWait`, `restartDaemon`, `confirmQuit`); `@MainActor final class SparkleUpdater` (`init?(coordinator:info:)`,
  `start()`).

- [ ] **Step 1: Write the failing tests**

Create `Tests/NotchCoreTests/UpdatePlanTests.swift`:

```swift
import XCTest
@testable import NotchCore

/// L34 Phase G: a release build names FERB's public feed and key; the daemon's answer to `update.prepare` reads as
/// ready or as the work it waits for; the notch says "Updated to …" once per new build.
final class UpdatePlanTests: XCTestCase {
    private let release: [String: Any] = [
        "SUFeedURL": "https://github.com/Json604/ferb/releases/latest/download/appcast-arm64.xml",
        "SUPublicEDKey": "pfIShU4dEXqPd5ObYNfDBiQWcXozk7estwzTnF9BamQ="]

    func testOnlyAReleaseBuildWithFERBsFeedAndKeyChecks() {
        XCTAssertEqual(UpdatePlan.feedURL(release), release["SUFeedURL"] as? String)
        XCTAssertNil(UpdatePlan.feedURL([:]))                                         // a development build
        XCTAssertNil(UpdatePlan.feedURL(["SUFeedURL": release["SUFeedURL"]!]))         // no key: Sparkle would refuse
        XCTAssertNil(UpdatePlan.feedURL(["SUFeedURL": "https://example.org/appcast-arm64.xml",
                                         "SUPublicEDKey": "k"]))
    }

    func testTheDaemonsAnswerReadsAsReadyOrTheWorkItWaitsFor() {
        XCTAssertEqual(UpdatePlan.prepared(["ready": true, "running": [[String: Any]]()]), .ready)
        XCTAssertEqual(UpdatePlan.prepared(["ready": false, "running": [["task_id": "t-1", "goal": "Rename"],
                                                                        ["task_id": "t-2", "goal": ""]]]),
                       .waiting(["Rename", "t-2"]))
        XCTAssertEqual(UpdatePlan.prepared(["ready": false]), .waiting(["a task"]))
        XCTAssertEqual(UpdatePlan.waitingLine(version: "0.28.0", work: ["Rename", "process: npm run dev"]),
                       "FERB 0.28.0 installs when this finishes: Rename; process: npm run dev")
    }

    func testUpdatedToOncePerNewBuild() {
        XCTAssertEqual(UpdatePlan.updatedNote(previousBuild: "202610250900", build: "202611010900", version: "0.28.0"),
                       "Updated to 0.28.0")
        XCTAssertNil(UpdatePlan.updatedNote(previousBuild: "202611010900", build: "202611010900", version: "0.28.0"))
        XCTAssertNil(UpdatePlan.updatedNote(previousBuild: nil, build: "202611010900", version: "0.28.0"))
    }
}
```

Create `Tests/NotchCoreTests/UpdateCoordinatorTests.swift`:

```swift
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
private final class FakeUpdater: UpdaterDriving {
    var automaticallyDownloadsUpdates = true
    var checks = 0
    func checkForUpdates() { checks += 1 }
}

/// L34 Phase G: an update installs only when FERB's daemon says nothing runs that a restart would cut, and FERB has
/// stopped; a quit while a task runs asks first; a failed install starts FERB again.
@MainActor
final class UpdateCoordinatorTests: XCTestCase {
    private func make(_ rpc: ScriptedRPC) -> (UpdateCoordinator, FakeUpdater) {
        let driver = FakeUpdater()
        let updates = UpdateCoordinator(rpc: rpc, driver: driver)
        updates.retryInterval = .milliseconds(10)
        updates.exitWait = .milliseconds(200)
        updates.restartDaemon = {}
        return (updates, driver)
    }

    private func busy(_ goal: String = "Rename the invoices") -> [String: Any] {
        ["ready": false, "running": [["task_id": "t-1", "goal": goal]]]
    }

    private func waitForAsk(_ rpc: ScriptedRPC) async throws {
        for _ in 0..<200 where rpc.params(of: "update.prepare").isEmpty { try await Task.sleep(for: .milliseconds(5)) }
    }

    func testAnIdleFERBStopsThenSparkleInstalls() async {
        let rpc = ScriptedRPC()
        rpc.replies["update.prepare"] = ["ready": true, "running": [[String: Any]]()]
        rpc.failures["ping"] = DaemonRPCError.unavailable                  // the daemon has exited
        let (updates, _) = make(rpc)
        var installed = 0
        await updates.readyToInstall(version: "0.28.0", install: { installed += 1 }).value
        XCTAssertEqual(installed, 1)
        XCTAssertEqual(updates.status, .installing("0.28.0"))
        XCTAssertEqual(rpc.params(of: "update.prepare").first?["version"] as? String, "0.28.0")
        XCTAssertNil(rpc.params(of: "update.prepare").first?["even_if_busy"])
    }

    func testARunningTaskHoldsTheInstallUntilItEnds() async {
        let rpc = ScriptedRPC()
        rpc.queue("update.prepare", [busy(), busy()])
        rpc.replies["update.prepare"] = ["ready": true, "running": [[String: Any]]()]
        rpc.failures["ping"] = DaemonRPCError.unavailable
        let (updates, _) = make(rpc)
        var installed = 0
        await updates.readyToInstall(version: "0.28.0", install: { installed += 1 }).value
        XCTAssertEqual(rpc.params(of: "update.prepare").count, 3)
        XCTAssertEqual(installed, 1)
    }

    func testWhileWaitingTheStatusNamesTheWork() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["update.prepare"] = busy()
        let (updates, _) = make(rpc)
        updates.retryInterval = .seconds(30)
        let job = updates.readyToInstall(version: "0.28.0", install: { XCTFail("installed while a task ran") })
        try await waitForAsk(rpc)
        try await Task.sleep(for: .milliseconds(20))
        XCTAssertEqual(updates.status, .waiting(version: "0.28.0", work: ["Rename the invoices"]))
        XCTAssertEqual(updates.status.line, "FERB 0.28.0 installs when this finishes: Rename the invoices")
        job.cancel()
    }

    func testNoDaemonMeansNoTaskCanBeRunning() async {
        let rpc = ScriptedRPC()
        rpc.failures["update.prepare"] = DaemonRPCError.unavailable
        let (updates, _) = make(rpc)
        var installed = 0
        await updates.readyToInstall(version: "0.28.0", install: { installed += 1 }).value
        XCTAssertEqual(installed, 1)
    }

    func testAnUnclearAnswerIsTreatedAsBusy() async throws {
        let rpc = ScriptedRPC()
        rpc.failures["update.prepare"] = DaemonRPCError.internalFault
        let (updates, _) = make(rpc)
        updates.retryInterval = .seconds(30)
        let job = updates.readyToInstall(version: "0.28.0", install: { XCTFail("installed without a clear answer") })
        try await waitForAsk(rpc)
        try await Task.sleep(for: .milliseconds(20))
        guard case .waiting = updates.status else { return XCTFail("expected waiting, got \(updates.status)") }
        job.cancel()
    }

    func testAnUpdateThatFailedAfterFERBStoppedStartsFERBAgain() async {
        let rpc = ScriptedRPC()
        rpc.replies["update.prepare"] = ["ready": true, "running": [[String: Any]]()]
        rpc.failures["ping"] = DaemonRPCError.unavailable
        let (updates, _) = make(rpc)
        var restarts = 0
        updates.restartDaemon = { restarts += 1 }
        await updates.readyToInstall(version: "0.28.0", install: {}).value
        updates.aborted("The update is improperly signed.")
        XCTAssertEqual(restarts, 1)
        XCTAssertEqual(updates.status, .failed("The update is improperly signed."))
        updates.aborted("No update")                                      // nothing was stopped this time
        XCTAssertEqual(restarts, 1)
    }

    func testQuittingWithATaskRunningAsksAndKeepFERBOpenInstallsNothing() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["update.prepare"] = busy()
        let (updates, _) = make(rpc)
        updates.retryInterval = .seconds(30)
        var asked: [String] = []
        updates.confirmQuit = { _, work in asked = work; return false }
        let job = updates.readyToInstall(version: "0.28.0", install: { XCTFail("installed while a task ran") })
        try await waitForAsk(rpc)
        let allowed = await withCheckedContinuation { c in updates.beforeQuit { c.resume(returning: $0) } }
        XCTAssertFalse(allowed)
        XCTAssertEqual(asked, ["Rename the invoices"])
        XCTAssertFalse(rpc.params(of: "update.prepare").contains { $0["even_if_busy"] as? Bool == true })
        job.cancel()
    }

    func testQuitAndInstallStopsFERBEvenWithATaskRunning() async throws {
        let rpc = ScriptedRPC()
        rpc.queue("update.prepare", [busy(), busy()])
        rpc.replies["update.prepare"] = ["ready": true, "running": [["task_id": "t-1", "goal": "Rename the invoices"]]]
        rpc.failures["ping"] = DaemonRPCError.unavailable
        let (updates, _) = make(rpc)
        updates.retryInterval = .seconds(30)
        updates.confirmQuit = { _, _ in true }
        let job = updates.readyToInstall(version: "0.28.0", install: {})
        try await waitForAsk(rpc)
        let allowed = await withCheckedContinuation { c in updates.beforeQuit { c.resume(returning: $0) } }
        XCTAssertTrue(allowed)
        XCTAssertEqual(rpc.params(of: "update.prepare").last?["even_if_busy"] as? Bool, true)
        XCTAssertTrue(updates.prepared)
        job.cancel()
    }

    func testQuittingWithoutAnUpdateWaitingNeverAsksFERB() async {
        let rpc = ScriptedRPC()
        let (updates, _) = make(rpc)
        let allowed = await withCheckedContinuation { c in updates.beforeQuit { c.resume(returning: $0) } }
        XCTAssertTrue(allowed)
        XCTAssertTrue(rpc.calls.isEmpty)
    }

    func testTheAutomaticSwitchDrivesSparkleAndAnOfferWaitsForAClick() {
        let (updates, driver) = make(ScriptedRPC())
        updates.automatic = false
        XCTAssertFalse(driver.automaticallyDownloadsUpdates)
        updates.found(version: "0.28.0")
        XCTAssertEqual(updates.status, .available("0.28.0"))
        XCTAssertEqual(updates.status.line, "Update available: FERB 0.28.0")
        updates.checkNow()
        XCTAssertEqual(driver.checks, 1)
        updates.automatic = true
        updates.found(version: "0.28.1")
        XCTAssertEqual(updates.status, .quiet)                            // Sparkle downloads it silently
        updates.notFound()
        XCTAssertEqual(updates.status, .upToDate)
    }

    func testTheFeedComesOnlyFromTheBuild() {
        let updates = UpdateCoordinator(rpc: ScriptedRPC())
        XCTAssertNil(SparkleUpdater(coordinator: updates, info: [:]))     // no feed in the build: no updater at all
        XCTAssertNil(SparkleUpdater(coordinator: updates, info: ["SUFeedURL": "https://example.org/appcast-arm64.xml",
                                                                  "SUPublicEDKey": "k"]))
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `swift test --package-path ui/mac/FERBNotch --filter "UpdatePlanTests|UpdateCoordinatorTests"`
Expected: FAIL to build (`UpdatePlan`, `UpdateCoordinator` missing).

- [ ] **Step 3: `Sources/NotchCore/UpdatePlan.swift`**

```swift
import Foundation

/// FERB's automatic updates (L34 Phase G): what the app reads from its own Info.plist and the daemon. Sparkle checks
/// the public feed and verifies FERB's EdDSA key and certificate; the daemon says whether a restart would cut work.
public enum UpdatePlan {
    public static let feedPrefix = "https://github.com/Json604/ferb/releases/latest/download/appcast-"

    /// The feed a release build checks: FERB's own, with FERB's public key beside it; nil for any other build.
    public static func feedURL(_ info: [String: Any]) -> String? {
        guard let feed = info["SUFeedURL"] as? String, feed.hasPrefix(feedPrefix), feed.hasSuffix(".xml"),
              let key = info["SUPublicEDKey"] as? String, !key.isEmpty else { return nil }
        return feed
    }

    public enum Prepared: Equatable {
        case ready
        case waiting([String])
    }

    /// `update.prepare`'s reply: FERB stopped for the update, or the work it waits for, as the owner reads it.
    public static func prepared(_ reply: [String: Any]) -> Prepared {
        if reply["ready"] as? Bool == true { return .ready }
        let work = (reply["running"] as? [[String: Any]] ?? []).map { row -> String in
            if let goal = row["goal"] as? String, !goal.isEmpty { return goal }
            return row["task_id"] as? String ?? "a task"
        }
        return .waiting(work.isEmpty ? ["a task"] : work)
    }

    public static func waitingLine(version: String, work: [String]) -> String {
        "FERB \(version) installs when this finishes: " + work.joined(separator: "; ")
    }

    /// The line the notch shows once after a launch on a new build; nil on a first launch or the same build.
    public static func updatedNote(previousBuild: String?, build: String, version: String) -> String? {
        guard let previousBuild, !previousBuild.isEmpty, previousBuild != build else { return nil }
        return "Updated to \(version)"
    }
}
```

- [ ] **Step 4: `Sources/FERBNotch/UpdateCoordinator.swift`**

```swift
import AppKit
import NotchCore

/// What drives Sparkle (SparkleUpdater), so the coordinator is testable without it.
@MainActor
protocol UpdaterDriving: AnyObject {
    var automaticallyDownloadsUpdates: Bool { get set }
    /// Sparkle's own window: the owner checks now, or installs an update FERB offered.
    func checkForUpdates()
}

/// FERB's automatic updates (L34 Phase G). Sparkle finds, downloads and verifies an update; this decides when it may
/// install: only once FERB's daemon says nothing runs that a restart would cut (`update.prepare`), which also stops the
/// daemon, so nothing runs from FERB.app while Sparkle replaces it. The new app's quiet service update starts it again.
@MainActor
final class UpdateCoordinator: ObservableObject {
    enum Status: Equatable {
        case quiet
        case upToDate
        case available(String)
        case waiting(version: String, work: [String])
        case installing(String)
        case failed(String)

        var line: String? {
            switch self {
            case .quiet: return nil
            case .upToDate: return "FERB is up to date."
            case .available(let version): return "Update available: FERB \(version)"
            case .waiting(let version, let work): return UpdatePlan.waitingLine(version: version, work: work)
            case .installing(let version): return "Installing FERB \(version)…"
            case .failed(let message): return "The update didn't install: \(message)"
            }
        }
    }

    @Published private(set) var status: Status = .quiet
    /// Settings → General → Install updates automatically (Sparkle keeps it; default on from the build's Info.plist).
    @Published var automatic: Bool { didSet { driver?.automaticallyDownloadsUpdates = automatic } }

    let rpc: DaemonRPCTransport
    weak var driver: UpdaterDriving?
    /// Starts FERB's background service again when an update it stopped for didn't install (plan C's DaemonService).
    var restartDaemon: @MainActor () -> Void = { Task { _ = await DaemonService().restart() } }
    /// Between asks while a task runs, and how long the daemon gets to close its socket.
    var retryInterval: Duration = .seconds(60)
    var exitWait: Duration = .seconds(60)
    /// The owner's answer when they quit while a task runs and an update waits: true is Quit and Install.
    var confirmQuit: @MainActor (_ version: String, _ work: [String]) -> Bool = { version, work in
        UpdateCoordinator.askBeforeQuitting(version: version, work: work)
    }

    private(set) var pendingVersion: String?
    private var pendingInstall: (() -> Void)?
    /// FERB's daemon stopped for this update (or wasn't running).
    private(set) var prepared = false
    private var installTask: Task<Void, Never>?

    init(rpc: DaemonRPCTransport, driver: UpdaterDriving? = nil) {
        self.rpc = rpc
        self.driver = driver
        automatic = driver?.automaticallyDownloadsUpdates ?? true
    }

    func attach(_ driver: UpdaterDriving) {
        self.driver = driver
        automatic = driver.automaticallyDownloadsUpdates
    }

    /// Sparkle downloaded an update and will install it when FERB.app quits.
    var installArmed: Bool { pendingInstall != nil }

    func found(version: String) {
        guard pendingInstall == nil, !prepared else { return }
        status = automatic ? .quiet : .available(version)
    }

    func notFound() {
        guard pendingInstall == nil, !prepared else { return }
        status = .upToDate
    }

    /// Settings or the menu: Install for an offered update, or Check for Updates…: Sparkle's own window.
    func checkNow() { driver?.checkForUpdates() }

    /// Sparkle is ready to install `version`; `install` replaces FERB.app and relaunches it.
    @discardableResult
    func readyToInstall(version: String, install: @escaping () -> Void) -> Task<Void, Never> {
        pendingVersion = version
        pendingInstall = install
        installTask?.cancel()
        let job = Task { [weak self] in await self?.installWhenIdle() }
        installTask = job
        return job
    }

    private func installWhenIdle() async {
        while let install = pendingInstall, !Task.isCancelled {
            let version = pendingVersion ?? ""
            switch await prepare(evenIfBusy: false) {
            case .ready:
                status = .installing(version)
                pendingInstall = nil
                install()
                return
            case .waiting(let work):
                status = .waiting(version: version, work: work)
                try? await Task.sleep(for: retryInterval)
            }
        }
    }

    /// Asks FERB's daemon to stop for the update; `.ready` means it stopped (or wasn't running).
    func prepare(evenIfBusy: Bool) async -> UpdatePlan.Prepared {
        var params: [String: Any] = ["version": pendingVersion ?? ""]
        if evenIfBusy { params["even_if_busy"] = true }
        do {
            let outcome = UpdatePlan.prepared(try await rpc.call("update.prepare", params: params))
            if outcome == .ready {
                prepared = true
                await waitForDaemonExit()
            }
            return outcome
        } catch DaemonRPCError.unavailable {
            prepared = true                              // FERB isn't running: no task can be
            return .ready
        } catch {
            return .waiting(["FERB couldn't say whether a task is running (\(error.localizedDescription))"])
        }
    }

    /// Until FERB's socket is gone (its shutdown ends by closing it), at most `exitWait`.
    private func waitForDaemonExit() async {
        let clock = ContinuousClock()
        let end = clock.now.advanced(by: exitWait)
        while clock.now < end {
            do { _ = try await rpc.call("ping", params: [:]) } catch { return }
            try? await Task.sleep(for: .milliseconds(500))
        }
    }

    /// Sparkle gave up: a failed download, a refused signature or certificate, a failed install.
    func aborted(_ message: String) {
        let stoppedFERB = prepared
        if stoppedFERB { restartDaemon() }               // stopped for nothing: start it again now
        if stoppedFERB || pendingInstall != nil { status = .failed(message) }
        prepared = false
        pendingInstall = nil
        installTask?.cancel()
    }

    /// The owner quits FERB.app while Sparkle waits to install on quit: only with nothing running, unless they choose
    /// Quit and Install. `reply(true)` lets the quit go on.
    func beforeQuit(_ reply: @escaping (Bool) -> Void) {
        guard installArmed, !prepared else { reply(true); return }
        Task {
            switch await prepare(evenIfBusy: false) {
            case .ready:
                reply(true)
            case .waiting(let work):
                if confirmQuit(pendingVersion ?? "", work) {
                    _ = await prepare(evenIfBusy: true)
                    reply(true)
                } else {
                    reply(false)
                }
            }
        }
    }

    static func askBeforeQuitting(version: String, work: [String]) -> Bool {
        let alert = NSAlert()
        alert.messageText = "FERB \(version) is ready to install"
        alert.informativeText = "FERB is still working on: " + work.joined(separator: "; ") +
            ". Quitting now stops it and installs the update; you can ask FERB to pick it up again after."
        alert.addButton(withTitle: "Keep FERB Open")
        alert.addButton(withTitle: "Quit and Install")
        NSApp.activate(ignoringOtherApps: true)
        return alert.runModal() == .alertSecondButtonReturn
    }
}
```

- [ ] **Step 5: `Sources/FERBNotch/SparkleUpdater.swift`**

```swift
import AppKit
import Sparkle
import NotchCore

/// Sparkle 2 for FERB.app (L34 Phase G): its standard updater over FERB's public release feed, about once a day
/// (`SUScheduledCheckInterval`). With automatic updates on, Sparkle downloads and verifies an update silently and hands
/// its install to UpdateCoordinator, which waits until FERB is idle. With them off, FERB shows "Update available —
/// Install" instead of Sparkle's window (gentle reminders: a background app never takes focus by itself, rule 17).
@MainActor
final class SparkleUpdater: NSObject, UpdaterDriving, SPUUpdaterDelegate, SPUStandardUserDriverDelegate {
    private let feed: String
    private weak var coordinator: UpdateCoordinator?
    private var controller: SPUStandardUpdaterController?

    /// nil for a build without FERB's release feed and key (a development build, or FERB_UPDATES=off).
    init?(coordinator: UpdateCoordinator, info: [String: Any] = Bundle.main.infoDictionary ?? [:]) {
        guard let feed = UpdatePlan.feedURL(info) else { return nil }
        self.feed = feed
        self.coordinator = coordinator
        super.init()
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: self,
                                                  userDriverDelegate: self)
    }

    func start() {
        guard let controller else { return }
        controller.updater.automaticallyChecksForUpdates = true    // checks stay on; the switch decides installing
        controller.startUpdater()
    }

    var automaticallyDownloadsUpdates: Bool {
        get { controller?.updater.automaticallyDownloadsUpdates ?? true }
        set { controller?.updater.automaticallyDownloadsUpdates = newValue }
    }

    func checkForUpdates() { controller?.checkForUpdates(nil) }

    // MARK: SPUUpdaterDelegate (Sparkle calls these on the main thread)

    /// FERB's own feed only: Sparkle would read a `SUFeedURL` preference before Info.plist; this answer comes first.
    nonisolated func feedURLString(for updater: SPUUpdater) -> String? { feed }

    nonisolated func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        let version = item.displayVersionString
        MainActor.assumeIsolated { coordinator?.found(version: version) }
    }

    nonisolated func updaterDidNotFindUpdate(_ updater: SPUUpdater) {
        MainActor.assumeIsolated { coordinator?.notFound() }
    }

    /// An automatic update is downloaded and verified: FERB installs it when idle (UpdateCoordinator).
    nonisolated func updater(_ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem,
                             immediateInstallationBlock immediateInstallHandler: @escaping () -> Void) -> Bool {
        let version = item.displayVersionString
        MainActor.assumeIsolated { coordinator?.readyToInstall(version: version, install: immediateInstallHandler) }
        return true
    }

    /// The owner chose Install in Sparkle's window: the same wait for an idle FERB before the relaunch.
    nonisolated func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem,
                             untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        let version = item.displayVersionString
        MainActor.assumeIsolated { coordinator?.readyToInstall(version: version, install: installHandler) }
        return true
    }

    nonisolated func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
        let message = error.localizedDescription
        MainActor.assumeIsolated { coordinator?.aborted(message) }
    }

    // MARK: SPUStandardUserDriverDelegate

    nonisolated var supportsGentleScheduledUpdateReminders: Bool { true }

    nonisolated func standardUserDriverShouldHandleShowingScheduledUpdate(_ update: SUAppcastItem,
                                                                           andInImmediateFocus immediateFocus: Bool) -> Bool {
        false                                            // FERB shows "Update available — Install" itself
    }

    nonisolated func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool,
                                                               forUpdate update: SUAppcastItem,
                                                               state: SPUUserUpdateState) {
        guard !handleShowingUpdate else { return }
        let version = update.displayVersionString
        MainActor.assumeIsolated { coordinator?.found(version: version) }
    }
}
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `swift test --package-path ui/mac/FERBNotch --filter "UpdatePlanTests|UpdateCoordinatorTests"`
Expected: PASS. If the compiler reports a delegate method's Swift name differs in the pinned Sparkle, take the name
from `ui/mac/FERBNotch/.build/artifacts/sparkle/Sparkle/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework/
Headers/SPUUpdaterDelegate.h` (its `NS_SWIFT_NAME` or the default import) and keep the behaviour.

- [ ] **Step 7: Commit**

```bash
git add ui/mac/FERBNotch/Sources/NotchCore/UpdatePlan.swift ui/mac/FERBNotch/Sources/FERBNotch/UpdateCoordinator.swift ui/mac/FERBNotch/Sources/FERBNotch/SparkleUpdater.swift ui/mac/FERBNotch/Tests/NotchCoreTests/UpdatePlanTests.swift ui/mac/FERBNotch/Tests/NotchCoreTests/UpdateCoordinatorTests.swift
git diff --cached --stat
git commit -m "feat(L34): the updater: Sparkle hands each install to FERB, which installs only once its daemon stopped idle

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Wiring: start the updater, guard the quit, Settings → Updates, the menu offer, "Updated to …"

**Files:**
- Modify: `ui/mac/FERBNotch/Sources/NotchCore/MenuBarState.swift` (plan D's: `MenuAction`, `StatusMenu.entries`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/PanelCoordinator.swift` (plan D's: properties, `rebuildMenu`,
  `perform`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/Application.swift`
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/FERBWindow.swift` (`FERBAppActions`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/SettingsPages.swift` (`GeneralSettingsPage`, `SettingsView`)
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/UpdateNotes.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/UpdateWiringTests.swift`

**Interfaces:**
- Consumes: Task 4; plan D's `MenuAction`, `StatusMenu`, `PanelCoordinator.perform`/`rebuildMenu`; plan F's
  `onSettled`; plan E's settings types.
- Produces: `MenuAction.installUpdate`; `StatusMenu.entries(..., hasWindow:, updateOffer: String? = nil)`;
  `PanelCoordinator.updateOffer`, `onInstallUpdate`; `NotchModel.noteUpdate(info:)`, `NotchModel.lastBuildKey`;
  `UpdatesSection`; `FERBAppActions.updates`.

- [ ] **Step 1: Write the failing tests** (create `Tests/NotchCoreTests/UpdateWiringTests.swift`)

```swift
import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 Phase G: after a new build's setup settles, the notch says "Updated to <version>" once; the menu offers an
/// update only while one waits for a click; Settings → General shows the switch and what the updater is doing.
@MainActor
final class UpdateWiringTests: XCTestCase {
    private func model() -> NotchModel {
        NotchModel(rpc: ScriptedRPC(), preferences: UserDefaults(suiteName: "ferb-update-\(UUID().uuidString)")!)
    }

    func testTheNotchSaysUpdatedToOncePerNewBuild() {
        let model = model()
        model.preferences.set("202610250900", forKey: NotchModel.lastBuildKey)
        let info: [String: Any] = ["CFBundleVersion": "202611010900", "CFBundleShortVersionString": "0.28.0"]
        model.noteUpdate(info: info)
        XCTAssertEqual(model.conversation.notices.map(\.text), ["Updated to 0.28.0"])
        XCTAssertEqual(model.conversation.notices.first?.tag, "FERB")
        model.noteUpdate(info: info)                                       // the same build again
        XCTAssertEqual(model.conversation.notices.count, 1)
        XCTAssertEqual(model.preferences.string(forKey: NotchModel.lastBuildKey), "202611010900")
    }

    func testAFirstLaunchSaysNothing() {
        let model = model()
        model.noteUpdate(info: ["CFBundleVersion": "202611010900", "CFBundleShortVersionString": "0.28.0"])
        XCTAssertTrue(model.conversation.notices.isEmpty)
        XCTAssertEqual(model.preferences.string(forKey: NotchModel.lastBuildKey), "202611010900")
    }

    func testTheMenuOffersAnUpdateOnlyWhileOneWaits() {
        func titles(_ offer: String?) -> [String] {
            StatusMenu.entries(state: .ready, paused: false, stopped: false, appName: "FERB", canSetUp: false,
                               hasWindow: true, updateOffer: offer).compactMap {
                if case .item(let title, _, _) = $0 { return title }
                return nil
            }
        }
        XCTAssertFalse(titles(nil).contains { $0.hasPrefix("Update available") })
        XCTAssertTrue(titles("0.28.0").contains("Update available (FERB 0.28.0) — Install"))
        XCTAssertTrue(StatusMenu.entries(state: .ready, paused: false, stopped: false, appName: "FERB", canSetUp: false,
                                         hasWindow: true, updateOffer: "0.28.0")
            .contains(.item("Update available (FERB 0.28.0) — Install", .installUpdate, key: "")))
    }

    func testTheUpdatesSectionLaysOut() {
        let updates = UpdateCoordinator(rpc: ScriptedRPC())
        updates.found(version: "0.28.0")
        let page = GeneralSettingsPage(feedback: SoundFeedback(), cursor: CursorController(makePanels: false),
                                       updates: updates)
        let hosting = NSHostingView(rootView: page.frame(width: 700, height: 500))
        hosting.frame = NSRect(x: 0, y: 0, width: 700, height: 500)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `swift test --package-path ui/mac/FERBNotch --filter UpdateWiringTests`
Expected: FAIL to build (`noteUpdate`, `updateOffer`, `UpdatesSection` missing).

- [ ] **Step 3: The menu offer** (plan D's files)

`MenuBarState.swift`: add `installUpdate` to `MenuAction`
(`case openFERB, openWindow, pause, resume, liftStop, restart, settings, setUp, quit, installUpdate`). In
`StatusMenu.entries`, add the parameter `updateOffer: String? = nil` after `hasWindow: Bool`, and right after the line
that appends `.item("Settings…", .settings, key: ",")` add:

```swift
        if let updateOffer {                  // automatic updates off: an update waits for the owner's click (L34 G)
            entries.append(.item("Update available (FERB \(updateOffer)) — Install", .installUpdate, key: ""))
        }
```

`PanelCoordinator.swift`: next to `var onOpenWindow`, add:

```swift
    var updateOffer: String?                     // L34 Phase G: an update waiting for the owner's click
    var onInstallUpdate: (() -> Void)?
```

In `rebuildMenu`, pass `updateOffer: updateOffer` to `StatusMenu.entries(…, hasWindow: hasWindow, updateOffer:
updateOffer)`. In `perform(_:)`, add `        case .installUpdate: onInstallUpdate?()`. (The menu is rebuilt each time it
opens, `menuNeedsUpdate`, so setting `updateOffer` is enough.)

- [ ] **Step 4: "Updated to …" and the Updates section** (create `Sources/FERBNotch/UpdateNotes.swift`)

```swift
import SwiftUI
import NotchCore

extension NotchModel {
    static let lastBuildKey = "FERBLastLaunchedBuild"

    /// L34 Phase G: once per new build, after its setup settled, the notch says "Updated to <version>".
    func noteUpdate(info: [String: Any] = Bundle.main.infoDictionary ?? [:]) {
        let build = info["CFBundleVersion"] as? String ?? ""
        let version = info["CFBundleShortVersionString"] as? String ?? ""
        let previous = preferences.string(forKey: Self.lastBuildKey)
        if !build.isEmpty { preferences.set(build, forKey: Self.lastBuildKey) }
        guard let note = UpdatePlan.updatedNote(previousBuild: previous, build: build, version: version) else { return }
        if conversation.addNotice(id: "update-" + build, text: note, tag: "FERB") {
            saveConversation()
            onBackgroundChange?(true)
        }
    }
}

/// Settings → General → Updates.
struct UpdatesSection: View {
    @ObservedObject var updates: UpdateCoordinator

    var body: some View {
        Toggle("Install updates automatically", isOn: $updates.automatic)
        if let line = updates.status.line { Text(line).textSelection(.enabled) }
        if case .available = updates.status {
            Button("Install") { updates.checkNow() }
        } else {
            Button("Check for Updates…") { updates.checkNow() }
        }
        Text("FERB asks only GitHub's public release list; nothing about you is sent. An update installs when no task " +
             "is running, then FERB's background service restarts.")
            .font(.caption).foregroundStyle(.secondary)
    }
}
```

`FERBWindow.swift`, in `FERBAppActions` after `var loginItems: LoginItemController?`, add
`    var updates: UpdateCoordinator?`.

`SettingsPages.swift`: in `GeneralSettingsPage` add `    var updates: UpdateCoordinator? = nil` after `loginItems`, and
after the "Startup" section:

```swift
            if let updates {
                Section("Updates") { UpdatesSection(updates: updates) }
            }
```

In `SettingsView.body`, the General page becomes
`GeneralSettingsPage(feedback: actions.feedback, cursor: actions.cursor, loginItems: actions.loginItems, updates: actions.updates)`.

- [ ] **Step 5: Start it, guard the quit, say "Updated to …"** (`Application.swift`)

Add `import Combine` at the top, and properties:

```swift
    private var updates: UpdateCoordinator?
    private var sparkle: SparkleUpdater?
    private var updateOffer: AnyCancellable?
```

In plan E's actions block (after plan F's login-item lines), add:

```swift
            let updates = UpdateCoordinator(rpc: coordinator.model.rpc)
            if appSetup.isPackaged, let sparkle = SparkleUpdater(coordinator: updates) {
                updates.attach(sparkle)
                sparkle.start()
                self.sparkle = sparkle
                actions.updates = updates
            }
            self.updates = updates
            coordinator.onInstallUpdate = { [updates] in updates.checkNow() }
            updateOffer = updates.$status.sink { [weak coordinator] status in
                if case .available(let version) = status { coordinator?.updateOffer = version }
                else { coordinator?.updateOffer = nil }
            }
```

Replace plan F's `appSetup.onSettled = { [loginItems] in loginItems.applyDefaultAfterSetup() }` with:

```swift
            appSetup.onSettled = { [loginItems, weak coordinator] in
                loginItems.applyDefaultAfterSetup()
                coordinator?.model.noteUpdate()               // "Updated to <version>", once per new build
            }
```

Add to `AppDelegate`:

```swift
    /// An update Sparkle will install on quit installs only with nothing running, or after the owner's Quit and
    /// Install (UpdateCoordinator.beforeQuit).
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let updates, updates.installArmed, !updates.prepared else { return .terminateNow }
        updates.beforeQuit { allow in NSApp.reply(toApplicationShouldTerminate: allow) }
        return .terminateLater
    }
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `swift test --package-path ui/mac/FERBNotch --filter UpdateWiringTests`, then
`swift test --package-path ui/mac/FERBNotch` (plan D's `MenuBarStateTests` still pass: `updateOffer` defaults to nil).
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ui/mac/FERBNotch/Sources/NotchCore/MenuBarState.swift ui/mac/FERBNotch/Sources/FERBNotch/PanelCoordinator.swift ui/mac/FERBNotch/Sources/FERBNotch/Application.swift ui/mac/FERBNotch/Sources/FERBNotch/FERBWindow.swift ui/mac/FERBNotch/Sources/FERBNotch/SettingsPages.swift ui/mac/FERBNotch/Sources/FERBNotch/UpdateNotes.swift ui/mac/FERBNotch/Tests/NotchCoreTests/UpdateWiringTests.swift
git diff --cached --stat
git commit -m "feat(L34): updates wired in: Settings switch, menu offer, a guarded quit and 'Updated to …'

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Releasing: the signed appcast per architecture

**Files:**
- Create: `scripts/make_appcast.py`
- Create: `scripts/release-appcast.sh` (`chmod 755`)
- Create: `tests/test_appcast.py`

**Interfaces:**
- Produces: `scripts/release-appcast.sh TAG DMG ARCH` → `build/appcast-<arch>.xml`, one item: `sparkle:version` = the
  app's `CFBundleVersion`, `sparkle:shortVersionString`, `sparkle:minimumSystemVersion`, an enclosure at
  `https://github.com/Json604/ferb/releases/download/<TAG>/<DMG name>` with Sparkle's EdDSA signature and length.

- [ ] **Step 1: Write the failing tests** (create `tests/test_appcast.py`)

```python
"""L34 Phase G: each release publishes one appcast per architecture with one item: the build Sparkle compares, the
version people read, the minimum macOS, and the release's DMG with FERB's EdDSA signature. Inputs are checked."""

import importlib.util
import subprocess
import sys
import xml.etree.ElementTree as ET
from datetime import datetime, timezone
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "make_appcast.py"
spec = importlib.util.spec_from_file_location("make_appcast", SCRIPT)
assert spec is not None and spec.loader is not None
make_appcast = importlib.util.module_from_spec(spec)
spec.loader.exec_module(make_appcast)

SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
SIGNED = 'sparkle:edSignature="QUJDRA==" length="734003200"'
WHEN = datetime(2026, 11, 1, 9, tzinfo=timezone.utc)


def test_one_item_pointing_at_the_releases_dmg() -> None:
    xml = make_appcast.appcast("0.28.0", "202611010900", "v0.28.0", "FERB-AppleSilicon.dmg", SIGNED, "14.2", WHEN)
    channel = ET.fromstring(xml).find("channel")
    assert channel is not None and len(channel.findall("item")) == 1
    item = channel.find("item")
    assert item is not None
    assert item.findtext(SPARKLE + "version") == "202611010900"
    assert item.findtext(SPARKLE + "shortVersionString") == "0.28.0"
    assert item.findtext(SPARKLE + "minimumSystemVersion") == "14.2"
    enclosure = item.find("enclosure")
    assert enclosure is not None
    assert enclosure.get("url") == "https://github.com/Json604/ferb/releases/download/v0.28.0/FERB-AppleSilicon.dmg"
    assert enclosure.get("length") == "734003200" and enclosure.get(SPARKLE + "edSignature") == "QUJDRA=="


@pytest.mark.parametrize("args", [
    ("0.28", "202611010900", "v0.28.0", "FERB-AppleSilicon.dmg", SIGNED, "14.2"),
    ("0.28.0", "2026", "v0.28.0", "FERB-AppleSilicon.dmg", SIGNED, "14.2"),
    ("0.28.0", "202611010900", "0.28.0", "FERB-AppleSilicon.dmg", SIGNED, "14.2"),
    ("0.28.0", "202611010900", "v0.28.0", "../FERB.dmg", SIGNED, "14.2"),
    ("0.28.0", "202611010900", "v0.28.0", "FERB-Intel.dmg", "not a signature", "14.2"),
    ("0.28.0", "202611010900", "v0.28.0", "FERB-Intel.dmg", SIGNED, "fourteen"),
])
def test_bad_inputs_are_refused(args: tuple[str, ...]) -> None:
    with pytest.raises(ValueError):
        make_appcast.appcast(*args, WHEN)


def test_the_command_writes_the_file_and_refuses_bad_input(tmp_path: Path) -> None:
    out = tmp_path / "appcast-x86_64.xml"
    base = [sys.executable, "-I", str(SCRIPT), "--version", "0.28.0", "--build", "202611010900", "--tag", "v0.28.0",
            "--dmg", "FERB-Intel.dmg", "--min-macos", "14.2", "--out", str(out)]
    assert subprocess.run(base + ["--signature", SIGNED]).returncode == 0 and out.exists()
    bad = subprocess.run(base + ["--signature", "nope"], capture_output=True, text=True)
    assert bad.returncode == 1 and "signature" in bad.stderr


def test_release_appcast_refuses_an_unknown_architecture(tmp_path: Path) -> None:
    done = subprocess.run([str(ROOT / "scripts" / "release-appcast.sh"), "v0.28.0", str(tmp_path / "x.dmg"), "ppc"],
                          capture_output=True, text=True)
    assert done.returncode == 1 and "arm64 or x86_64" in done.stderr


def test_the_feed_names_agree_across_the_build_the_release_and_the_app() -> None:
    assert "appcast-$ARCH.xml" in (ROOT / "scripts" / "build-app.sh").read_text()
    assert "appcast-$ARCH.xml" in (ROOT / "scripts" / "release-appcast.sh").read_text()
    swift = (ROOT / "ui" / "mac" / "FERBNotch" / "Sources" / "NotchCore" / "UpdatePlan.swift").read_text()
    assert 'feedPrefix = "https://github.com/Json604/ferb/releases/latest/download/appcast-"' in swift
```

- [ ] **Step 2: Run them to see them fail**

Run: `nice -n 10 uv run pytest tests/test_appcast.py -q`
Expected: FAIL at collection (no `scripts/make_appcast.py`).

- [ ] **Step 3: Create `scripts/make_appcast.py`**

```python
"""Write FERB's one-item Sparkle appcast for one architecture (L34 Phase G). Sparkle reads the newest item only, so the
feed holds just the release being published. Its archive is that release's DMG on GitHub, signed with FERB's EdDSA
key (sign_update's output is passed in). Release tooling, stdlib only; never part of FERB.app.

    python3 -I scripts/make_appcast.py --version 0.28.0 --build 202611010900 --tag v0.28.0 \
        --dmg FERB-AppleSilicon.dmg --signature 'sparkle:edSignature="…" length="…"' --min-macos 14.2 \
        --out build/appcast-arm64.xml"""

import argparse
import re
import sys
from datetime import datetime, timezone
from email.utils import format_datetime
from pathlib import Path
from xml.sax.saxutils import quoteattr

REPO = "https://github.com/Json604/ferb"
SIGNED = re.compile(r'sparkle:edSignature="([A-Za-z0-9+/]+=*)"\s+length="(\d+)"')
CHECKS = (("version", re.compile(r"\d+\.\d+\.\d+")), ("build", re.compile(r"\d{12}")),
          ("tag", re.compile(r"v\d+\.\d+\.\d+")), ("DMG name", re.compile(r"FERB-(AppleSilicon|Intel)\.dmg")),
          ("minimum macOS", re.compile(r"\d+(\.\d+){0,2}")))


def appcast(version: str, build: str, tag: str, dmg: str, signature: str, min_macos: str, now: datetime) -> str:
    for (what, pattern), value in zip(CHECKS, (version, build, tag, dmg, min_macos)):
        if not pattern.fullmatch(value):
            raise ValueError(f"{what} {value!r} isn't valid")
    signed = SIGNED.search(signature)
    if signed is None:
        raise ValueError('the signature must be sign_update\'s output: sparkle:edSignature="…" length="…"')
    ed, length = signed.groups()
    url = f"{REPO}/releases/download/{tag}/{dmg}"
    return ('<?xml version="1.0" encoding="utf-8"?>\n'
            '<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">\n'
            '  <channel>\n'
            '    <title>FERB</title>\n'
            '    <item>\n'
            f'      <title>FERB {version}</title>\n'
            f'      <pubDate>{format_datetime(now)}</pubDate>\n'
            f'      <sparkle:version>{build}</sparkle:version>\n'
            f'      <sparkle:shortVersionString>{version}</sparkle:shortVersionString>\n'
            f'      <sparkle:minimumSystemVersion>{min_macos}</sparkle:minimumSystemVersion>\n'
            f'      <link>{REPO}/releases/tag/{tag}</link>\n'
            f'      <enclosure url={quoteattr(url)} length="{length}" type="application/octet-stream" '
            f'sparkle:edSignature="{ed}"/>\n'
            '    </item>\n'
            '  </channel>\n'
            '</rss>\n')


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="FERB's Sparkle appcast for one architecture")
    for name in ("version", "build", "tag", "dmg", "signature", "min-macos", "out"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args(argv)
    try:
        text = appcast(args.version, args.build, args.tag, args.dmg, args.signature, args.min_macos,
                       datetime.now(timezone.utc))
    except ValueError as e:
        print(f"make_appcast.py: {e}", file=sys.stderr)
        return 1
    Path(args.out).write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
```

- [ ] **Step 4: Create `scripts/release-appcast.sh`** (`chmod 755`)

```zsh
#!/bin/zsh -f
# The Sparkle appcast for one architecture's release DMG (L34 Phase G): reads the version and build from the app in the
# DMG, checks that app asks for this architecture's feed, signs the DMG with FERB's EdDSA key (Sparkle's sign_update;
# the private key is in the owner's login Keychain) and writes build/appcast-<arch>.xml. Publishing is the owner's step
# (README, "Releasing").
#
#   scripts/release-appcast.sh TAG DMG ARCH     e.g. v0.28.0 build/FERB-AppleSilicon.dmg arm64
set -euo pipefail
ROOT="${0:A:h:h}"
NOTCH="$ROOT/ui/mac/FERBNotch"
die() { print -u2 "release-appcast.sh: $*"; exit 1 }

TAG=${1:?usage: release-appcast.sh TAG DMG ARCH}
DMG=${${2:?usage: release-appcast.sh TAG DMG ARCH}:A}
ARCH=${3:?usage: release-appcast.sh TAG DMG ARCH}
[[ "$ARCH" == (arm64|x86_64) ]] || die "ARCH is arm64 or x86_64"
[[ -f "$DMG" ]] || die "no DMG at $DMG"
SIGN=$(find "$NOTCH/.build/artifacts" -path '*/Sparkle/bin/sign_update' -type f 2>/dev/null | head -1)
[[ -n "$SIGN" ]] || die "Sparkle's sign_update isn't here yet: swift package --package-path ui/mac/FERBNotch resolve"

MNT=$(mktemp -d)
trap 'hdiutil detach -quiet "$MNT" 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT
hdiutil attach -nobrowse -readonly -quiet -mountpoint "$MNT" "$DMG"
INFO="$MNT/FERB.app/Contents/Info.plist"
VERSION=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$INFO")
BUILD=$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$INFO")
MINOS=$(/usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" "$INFO")
FEED=$(/usr/libexec/PlistBuddy -c "Print :SUFeedURL" "$INFO" 2>/dev/null || true)
hdiutil detach -quiet "$MNT"
[[ "$FEED" == *"/appcast-$ARCH.xml" ]] || die "the app in ${DMG:t} checks '${FEED:-no feed}', not appcast-$ARCH.xml"

SIGNATURE=$("$SIGN" "$DMG")
mkdir -p "$ROOT/build"
/usr/bin/python3 -I "$ROOT/scripts/make_appcast.py" --version "$VERSION" --build "$BUILD" --tag "$TAG" \
  --dmg "${DMG:t}" --signature "$SIGNATURE" --min-macos "$MINOS" --out "$ROOT/build/appcast-$ARCH.xml"
print "Wrote build/appcast-$ARCH.xml for FERB $VERSION ($BUILD, $ARCH)."
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `nice -n 10 uv run pytest tests/test_appcast.py -q`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add scripts/make_appcast.py scripts/release-appcast.sh tests/test_appcast.py
git diff --cached --stat
git commit -m "feat(L34): one signed Sparkle appcast per architecture for each GitHub release

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Documentation, registry, the owner's live check, the full suite

**Files:**
- Modify: `README.md`, `ARCHITECTURE.md`, `CHANGELOG.md`, `docs/RPC.md`, `docs/0.27_DECISIONS.toml`,
  `docs/OWNER_ACCEPTANCE.md`

- [ ] **Step 1: README.** In "FERB.app (disk image)", after plan C's sentence about quiet updates on a new build, add:

```markdown
FERB updates itself. About once a day it asks GitHub's public release list for a newer version (nothing about you is
sent), downloads it, checks it with FERB's own update key and certificate, and installs it when no task is running;
the background service restarts, a loop you asked for carries on, and the notch says "Updated to <version>". If you
quit FERB while a task runs and an update waits, FERB asks first (**Keep FERB Open** or **Quit and Install**).
Settings → General → **Install updates automatically** turns this off; FERB then shows "Update available — Install"
in its menu and in Settings.
```

Add a section "Releasing (owner)" under "Development and documentation":

```markdown
### Releasing (owner)

Once: `scripts/sparkle-keys.sh` makes FERB's update key (the private half stays in your login Keychain) and writes
`scripts/sparkle-public-key.txt` (commit it); `scripts/sparkle-keys.sh --export ~/Documents/FERB-keys/update-key`
backs the private key up outside the checkout. Keep that backup offline: without it no later release can update
existing installs.

Each release: build and sign both images with your release certificate (`SIGN_IDENTITY=release scripts/build-dmg.sh`),
then `scripts/release-appcast.sh v<version> build/FERB-AppleSilicon.dmg arm64` and
`scripts/release-appcast.sh v<version> build/FERB-Intel.dmg x86_64`, and publish all four files as one release:
`gh release create v<version> build/FERB-AppleSilicon.dmg build/FERB-Intel.dmg build/appcast-arm64.xml build/appcast-x86_64.xml`.
Every installed FERB finds it through `releases/latest`. A build with `FERB_UPDATES=off` never checks.
```

- [ ] **Step 2: ARCHITECTURE.md**, after plan F's section:

```markdown
### Automatic updates (L34 Phase G)

- **Find and verify:** Sparkle 2 (`Contents/Frameworks/Sparkle.framework`, pinned in `ui/mac/FERBNotch/Package.swift`)
  checks `SUFeedURL` (`releases/latest/download/appcast-<arch>.xml`, one item, `scripts/make_appcast.py`) about daily;
  `SparkleUpdater.feedURLString(for:)` answers the build's feed, so no preference can redirect it; the archive is the
  release DMG, verified with `SUPublicEDKey` and against the running app's designated requirement (plan B's
  certificate). No system profile is sent.
- **When:** `UpdateCoordinator` holds Sparkle's install (`willInstallUpdateOnQuit` / `shouldPostponeRelaunchForUpdate`)
  until `update.prepare` answers ready: `Supervisor.update_blockers()` (running trees, except loops resting between
  rounds and plan waits) plus running FERB processes. Ready means, in one step: `_shutting_down`, `hold_new_work()`,
  `update.prepared` journaled, FERB Voice quit, `_stop` set; `Daemon.run()` shuts down and exits 0 (launchd doesn't
  restart a clean exit). Once `ping` fails, Sparkle replaces FERB.app and relaunches it.
- **Restart:** the new app's launch sees a new build and runs plan C's quiet `ferb setup --service --update`, which
  re-pins the helpers and loads the LaunchAgent; `resume_plans()` resumes suspended plans and loops (L31); when setup
  settles (`onSettled`), `NotchModel.noteUpdate` posts "Updated to <version>" once. A failed install after the stop
  runs `DaemonService.restart()`.
- **Quit:** `applicationShouldTerminate` asks `update.prepare` when an install is armed; with a task running the owner
  chooses Keep FERB Open or Quit and Install (`even_if_busy`).
- **Rule 10:** the shell sandbox denies `user-preference-write` for `APP_PREFERENCE_DOMAINS` and writes to their plist
  files. A timer due while FERB shuts down stays claimed and fires after the restart.
```

  In §16.10, add `update.prepared` with one line. In `docs/RPC.md` (plan C's L34 section), add the `update.prepare` row
  with Task 1's params and reply.

- [ ] **Step 3: CHANGELOG.md**, under the L34 entry:

```markdown
Phase G, automatic updates (L34): Sparkle 2 in FERB.app checks GitHub's public release feed about daily, verifies each
update with FERB's EdDSA key and certificate, and installs it only when no task is running (loops resting between
rounds and plan waits resume after the restart); the background service restarts quietly and the notch says "Updated
to <version>". Settings → General → Install updates automatically (default on); off shows "Update available —
Install". A quit while a task runs and an update waits asks first. New `update.prepare` (owner-only); the sandbox keeps
FERB's shell out of FERB.app's preferences; a timer due during shutdown now fires after the restart instead of being
lost; `scripts/sparkle-keys.sh` and `scripts/release-appcast.sh` make the key and each release's appcasts.
```

- [ ] **Step 4: Registry.** Append to `[L34] tests`: `"tests/test_update_prepare.py"`,
  `"tests/test_app_preferences_sandbox.py"`, `"tests/test_update_packaging.py"`, `"tests/test_appcast.py"`,
  `"ui/mac/FERBNotch/Tests/NotchCoreTests/UpdatePlanTests.swift"`,
  `"ui/mac/FERBNotch/Tests/NotchCoreTests/UpdateCoordinatorTests.swift"`,
  `"ui/mac/FERBNotch/Tests/NotchCoreTests/UpdateWiringTests.swift"`; append to its `note`: ` Phase G rulings (not the
  owner's): resting loops and plan waits let an update through, nothing else that runs does; running FERB processes
  hold it; a quit with an armed update and a running task asks Keep FERB Open / Quit and Install; one-item appcast per
  architecture.`

- [ ] **Step 5: Owner checks.** Append to `docs/OWNER_ACCEPTANCE.md`:

```markdown
## Automatic updates (L34 Phase G) — pending live check (2026-10-25)

Needs plan B's certificate and `scripts/sparkle-keys.sh` done once. Publishing a test release reaches GitHub: run it
only with the owner's go-ahead. On a fresh second macOS user, the owner watching:

1. Install build N (`v0.28.0`) from its DMG; finish setup. Start a loop ("keep this YouTube tab playing") and a long
   task.
2. Publish build N+1 (`v0.28.1`: both DMGs and both appcasts, `gh release create`). Settings → General → Check for
   Updates… **Expected:** while the long task runs, Settings says "FERB 0.28.1 installs when this finishes: …"; nothing
   restarts. When it ends, within a minute FERB.app quits and reopens by itself, the notch says "Updated to 0.28.1",
   `ferb doctor` shows the new version, and the loop keeps the tab playing with no new chat line. `ferb timeline`
   shows `update.prepared` and `daemon.started`.
3. macOS permissions still granted (System Settings → Privacy & Security shows FERB on; `ferb doctor`), no Open Anyway
   prompt for the update.
4. Turn Install updates automatically off; publish N+2. **Expected:** the menu shows "Update available (FERB …) —
   Install" and no Sparkle window appears by itself; clicking it shows Sparkle's window; Install waits for idle again.
5. Quit FERB from the menu while a task runs and an update waits. **Expected:** the Keep FERB Open / Quit and Install
   question; Keep FERB Open leaves the task running.
6. `log stream --predicate 'process == "FERBNotch"'` during a check shows only the GitHub feed request (no other host).
```

- [ ] **Step 6: Full verification**

Run: `nice -n 10 uv run pytest -q`; `uv run mypy ferb/core`; `swift test --package-path ui/mac/FERBNotch`;
`git diff --check`. Expected: all pass, no `git diff --check` output.

- [ ] **Step 7: Commit**

```bash
git add README.md ARCHITECTURE.md CHANGELOG.md docs/RPC.md docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git diff --cached --stat
git commit -m "docs(L34): automatic updates: README (updating, releasing), ARCHITECTURE, RPC, CHANGELOG, registry, owner check

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
