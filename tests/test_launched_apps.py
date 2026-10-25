"""L39: apps FERB launches are task resources (live case 39e35d57: Pages, launched by mac.open, was still running
after the task; every task.cleanup released nothing). An app that wasn't running and that FERB launched is quit when
its task's resources are released, unless the owner used it or it may hold unsaved changes: then it stays and the
answer names it. Launches go to the background."""

import json
from typing import Any

import pytest

from ferb.act.mac import apps as appsm
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.launched import (KEPT_OWNER, KEPT_UNKNOWN, KEPT_UNSAVED, Launched, LaunchedApps, brought_forward,
                                   left_running)
from ferb.core.types import Identity
from tests.conftest import RunningDaemon
from tests.mackit import _fake_mac, gateway_call as call, grant_task as task

PAGES = Launched("com.apple.iWork.Pages", "Pages", 41, "2026-10-25T07:34:05+05:30")


def _apps(running: list[RunningApp], used: bool = False, edited: bool | None = False) -> tuple[LaunchedApps, list[int]]:
    quits: list[int] = []

    def quit(pid: int, bundle: str) -> bool:
        quits.append(pid)
        return True
    return LaunchedApps(lambda: running, lambda task, app: used, lambda pid: edited, quit), quits


def _run(coro: Any) -> Any:
    import asyncio
    return asyncio.run(coro)


def test_an_app_ferb_launched_is_quit_when_its_task_ends() -> None:
    launched, quits = _apps([RunningApp(PAGES.bundle_id, "Pages", 41, False)])
    launched.add("t.step.two", PAGES)
    [record] = _run(launched.release("t.step.two"))
    assert quits == [41] and record == {"resource": "app", "app": "Pages", "bundle_id": PAGES.bundle_id,
                                        "released": True, "closed": True}
    assert _run(launched.release("t.step.two")) == []              # released once


@pytest.mark.parametrize("running, used, edited, why", [
    ([RunningApp(PAGES.bundle_id, "Pages", 41, True)], False, False, KEPT_OWNER),      # the owner is in it now
    ([RunningApp(PAGES.bundle_id, "Pages", 41, False)], True, False, KEPT_OWNER),      # brought forward by the owner
    ([RunningApp(PAGES.bundle_id, "Pages", 41, False)], False, True, KEPT_UNSAVED),
    ([RunningApp(PAGES.bundle_id, "Pages", 41, False)], False, None, KEPT_UNKNOWN),
])
def test_an_app_the_owner_used_or_that_may_hold_unsaved_work_stays_and_is_named(
        running: list[RunningApp], used: bool, edited: bool | None, why: str) -> None:
    launched, quits = _apps(running, used, edited)
    launched.add("t", PAGES)
    records = _run(launched.release("t"))
    assert quits == [] and records[0]["left_running"] == why and records[0]["closed"] is False
    assert left_running(records) == [f"Pages is still open: FERB opened it for this task and left it running because "
                                     f"{why}."]


def test_an_app_that_quit_or_was_relaunched_by_the_owner_is_left_alone() -> None:
    launched, quits = _apps([RunningApp(PAGES.bundle_id, "Pages", 99, False)])      # another process now
    launched.add("t", PAGES)
    [record] = _run(launched.release("t"))
    assert quits == [] and record["note"] == "it had already quit" and not left_running([record])


def test_coming_forward_with_no_ferb_on_screen_call_near_is_the_owners(daemon: RunningDaemon) -> None:
    async def go() -> tuple[bool, bool]:
        spans = daemon.daemon.spans
        for actions in ([{"tool": "mac.click", "on_screen": True}], []):
            s = spans.start("disturbance.change", task_id="tree", input={"operation": "observe desktop"})
            spans.finish(s, output={"changes": {"frontmost_app": {"before": {"pid": 1}, "after": {"pid": 41}}},
                                    "actions": actions})
            if actions:
                near = brought_forward(daemon.daemon.db, "tree", 41, "2000-01-01")
        return near, brought_forward(daemon.daemon.db, "tree", 41, "2000-01-01")
    near, alone = daemon.run(go())
    assert near is False                       # the live case: Word came forward during FERB's click in Pages
    assert alone is True


def test_mac_open_records_the_app_it_launched_and_its_task_end_quits_it(daemon: RunningDaemon,
                                                                       monkeypatch: pytest.MonkeyPatch) -> None:
    _fake_mac(daemon, monkeypatch)
    mac = daemon.daemon.mac
    assert mac is not None
    fake = mac.apps

    def launch(bid: str) -> bool:
        fake.apps = fake.apps + [RunningApp(bid, "Pages", 41, False)]
        return True
    fake.launch = launch
    quits: list[tuple[int, str]] = []
    launched = mac.launched                                                  # type: ignore[attr-defined]
    launched.unsaved = lambda pid: False
    launched.quit = lambda pid, bundle: quits.append((pid, bundle)) or True
    g = task(daemon, "t-open", Identity.OWNER, {"mac:read"})
    r = call(daemon, g, "mac.open", bundle_id="com.apple.iWork.Pages")
    assert not r["is_error"] and r["content"]["launched"] is True, r
    assert call(daemon, g, "mac.open", bundle_id="com.apple.Notes")["content"]["launched"] is False   # already running
    resources = daemon.run(daemon.daemon._task_end("t-open"))
    assert quits == [(41, "com.apple.iWork.Pages")]
    assert {"resource": "app", "app": "Pages", "bundle_id": "com.apple.iWork.Pages", "released": True,
            "closed": True} in resources


def test_a_launch_goes_to_the_background(monkeypatch: pytest.MonkeyPatch) -> None:
    ran: list[list[str]] = []
    monkeypatch.setattr(appsm, "candidates", lambda bid: [])

    class Workspace:
        @staticmethod
        def sharedWorkspace() -> "Workspace":
            return Workspace()

        def URLForApplicationWithBundleIdentifier_(self, bid: str) -> str:
            return "/Applications/Pages.app"
    import AppKit
    monkeypatch.setattr(AppKit, "NSWorkspace", Workspace)
    import subprocess
    monkeypatch.setattr(subprocess, "run", lambda cmd, **kw: ran.append(cmd) or subprocess.CompletedProcess(cmd, 0))
    assert appsm.launch("com.apple.iWork.Pages")
    assert ran == [["open", "-g", "-j", "-b", "com.apple.iWork.Pages"]]     # -g: never brought forward


def test_released_apps_reach_the_answer_lines() -> None:
    records = [{"resource": "app", "app": "Microsoft Word", "left_running": KEPT_UNSAVED},
               {"resource": "tab", "closed": True}, {"resource": "app", "app": "Pages", "closed": True}]
    assert left_running(records) == ["Microsoft Word is still open: FERB opened it for this task and left it running "
                                     "because it has unsaved changes."]
    assert json.dumps(records)                                              # plain data for the trace


def test_an_app_left_running_is_named_in_the_tasks_answer(daemon: RunningDaemon,
                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    from tests.conftest import chat, cond, done, install, outcome, sup, task as plan, tool
    from tests.support.checker_calls import verdict
    _fake_mac(daemon, monkeypatch)
    mac = daemon.daemon.mac
    assert mac is not None
    fake = mac.apps

    def launch(bid: str) -> bool:
        fake.apps = fake.apps + [RunningApp(bid, "Pages", 41, False)]
        return True
    fake.launch = launch
    mac.launched.unsaved = lambda pid: True                                 # type: ignore[attr-defined]
    mac.launched.quit = lambda pid, bundle: pytest.fail("an app with unsaved changes is never quit")  # type: ignore

    def worker(token: str) -> str:
        assert not tool(daemon.sock, token, "mac.open", bundle_id="com.apple.iWork.Pages")["is_error"]
        return done("Opened the assignment in Pages.")
    install(daemon, [sup("task", "On it.", plan("open the assignment in Pages", conds=(cond("It is open"),)))],
            {"worker": worker, "worker.verify": verdict(True)})
    out = outcome(chat(daemon, "open the assignment in Pages"))
    assert ("Pages is still open: FERB opened it for this task and left it running because it has unsaved changes."
            in out["answer"])
