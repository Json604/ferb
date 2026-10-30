"""Installed capability discovery preserves tools while reporting connection facts."""

from types import SimpleNamespace
from typing import Any

from ferb.cli.client import Client
from ferb.core.types import Identity
from ferb.orchestrator.availability import build
from tests.conftest import RunningDaemon
from tests.conftest import STALL, chat, install, sup, task

REGISTERED = {"fs:read", "fs:write", "browser:read", "browser:act", "mac:read", "mac:act", "applescript", "screen",
              "shell", "time", "trail:read"}
WORK = SimpleNamespace(install_id="install-work", browser="brave", folder="Default", identity=Identity.OWNER)
# Live 2026-10-24 (task 0a5fbe31): "paired, extension transport disconnected" read as broken, and the worker drove the
# profile through Chrome's menus with Accessibility instead of the browser tools, which start it without a window.
IDLE = "Brave “Work” paired; not connected now — FERB connects once that profile is open in its browser"


def test_installed_groups_remain_listed_with_connection_observations() -> None:
    a = build(REGISTERED, {}, "Brave “Work”")
    assert a.scopes == REGISTERED and "connection observations: Brave “Work”" in a.text and "Unavailable" not in a.text
    a = build(REGISTERED, {"owner_browsers": "Brave “Work” paired but not open — open it"})
    assert {"browser:read", "browser:act"} <= a.scopes and "The owner's browsers," in a.text
    assert a.unavailable_line() == "Unavailable: the owner's browsers (Brave “Work” paired but not open — open it)."
    assert a.text.count("Connection/permission observations:") == 1
    assert "FERB's own browser" not in build({"fs:read"}, {}).text          # not part of this FERB: not mentioned
    mac = build({"mac:read", "mac:act"}, {}).text                           # found in a real run: never more than present
    assert "through Accessibility (mac:act, mac:read)" in mac and "applescript" not in mac.lower()


def rig(d: RunningDaemon, paired: list[Any], connected: set[tuple[str, str]],
        perms: dict[str, bool] | None = None, identity: tuple[str, str] | None = None) -> Any:
    dm = d.daemon
    if dm.router is not None:
        dm.router.wake = None  # tests below cover a setup without the automatic profile wake service
    dm.pairings = SimpleNamespace(all=lambda: paired, for_profile=lambda b, f: None)          # type: ignore[assignment]
    dm.hub.bridges = {(b.browser, b.folder): SimpleNamespace(conn=object() if (b.browser, b.folder) in connected
                                                             else None) for b in paired}      # type: ignore[union-attr]
    dm._profiles = lambda: [SimpleNamespace(browser="brave", folder="Default", display_name="Work", email="")]  # type: ignore
    dm._permissions = lambda: perms or {"accessibility": True, "screen": True}              # type: ignore
    dm._identity_profile = lambda: identity                                                   # type: ignore
    return dm


async def _avail(dm: Any) -> Any:
    return dm._availability()


def test_a_paired_profile_with_a_wake_service_can_be_opened(daemon: RunningDaemon) -> None:
    dm = rig(daemon, [WORK], set())
    original = dm.router
    dm.router = SimpleNamespace(wake=object(), shutdown=_noop)
    a = daemon.run(_avail(dm))
    assert "browser:act" in a.scopes
    assert "can wake" in a.text and "connection observations: Brave" not in a.text
    dm.router = original


async def _facts(dm: Any) -> str:
    return str(dm._live_facts())          # the owner's browsers as FERB can use them now: live (step 8c)


def test_the_planner_facts_say_a_paired_profile_not_running_starts_in_the_background(daemon: RunningDaemon) -> None:
    dm = rig(daemon, [WORK], set())
    original = dm.router
    dm.router = SimpleNamespace(wake=object(), shutdown=_noop)
    try:
        facts = daemon.run(_facts(dm))
        assert ("- The owner's brave profile “Work”: paired; not connected now (usually the profile just isn't "
                "running) — FERB's browser tools start it in the background and open its tab there themselves, "
                "nothing to ask") in facts
        assert "transport disconnected" not in facts
        dm.hub.refused["install-work"] = "extension 0.15.5 (protocol 16), FERB needs 0.15.6 (protocol 17): reload it"
        facts = daemon.run(_facts(dm))
        assert ("- The owner's brave profile “Work”: paired; its extension was refused: extension 0.15.5 "
                "(protocol 16), FERB needs 0.15.6 (protocol 17): reload it") in facts
        dm.hub.refused.clear()
        dm.router = SimpleNamespace(wake=None, shutdown=_noop)
        facts = daemon.run(_facts(dm))
        assert "- The owner's brave profile “Work”: paired; not connected now — FERB connects once that profile is " \
               "open in its browser" in facts
        rig(daemon, [WORK], {("brave", "Default")})
        assert "- The owner's brave profile “Work”: paired, connected" in daemon.run(_facts(dm))
    finally:
        dm.hub.refused.clear()
        dm.router = original


def test_owner_browser_connection_facts_do_not_withdraw_installed_tools(daemon: RunningDaemon) -> None:
    dm = rig(daemon, [], set())
    a = daemon.run(_avail(dm))
    assert "browser:act" in a.scopes and "none of the owner's browser profiles is paired" in a.unavailable_line()
    rig(daemon, [WORK], set())
    a = daemon.run(_avail(dm))
    assert IDLE in a.unavailable_line() and "browser:act" in a.scopes
    rig(daemon, [WORK], {("brave", "Default")})
    a = daemon.run(_avail(dm))
    assert {"browser:read", "browser:act"} <= a.scopes and "connection observations: Brave “Work”" in a.text
    assert a.missing == []


def test_ferbs_own_browser_and_the_mac_permissions(daemon: RunningDaemon) -> None:
    dm = rig(daemon, [WORK], {("brave", "Default")}, perms={"accessibility": False, "screen": False},
             identity=("chrome", "Profile 22"))
    a = daemon.run(_avail(dm))
    line = a.unavailable_line()
    assert "FERB's own browser (not paired — run `ferb setup --identity`)" in line
    assert "the owner's Mac apps (FERB doesn't have the Accessibility permission — run `ferb doctor`)" in line
    assert "Screen Recording" in line and {"mac:act", "mac:read", "applescript", "screen"} <= a.scopes
    dm.identity_browser = SimpleNamespace(shutdown=_noop)                   # set up, but its profile isn't open:
    try:                                                                     # usable — FERB opens it when needed
        a = daemon.run(_avail(dm))
        assert "FERB's own browser" not in a.unavailable_line()
    finally:
        dm.identity_browser = None


async def _noop() -> None:
    return None


def test_the_planner_sees_the_built_list(daemon: RunningDaemon) -> None:
    dm = rig(daemon, [WORK], set())
    dm.supervisor.availability = dm._availability
    script = install(daemon, [sup("reply", "Hi.")])
    chat(daemon, "hello")
    system = daemon.run(_system(dm))
    assert "Installed worker capabilities:" in system and "{capabilities}" not in system
    assert f"Connection/permission observations: the owner's browsers ({IDLE}" in system
    assert "The owner's browsers (\"owner\" identity), connected now" not in system
    assert script.contexts                                                   # the turn ran


async def _system(dm: Any) -> str:
    return str(dm.supervisor._system("supervisor.md"))


def test_worker_keeps_installed_tools_while_availability_reports_connections(daemon: RunningDaemon) -> None:
    """A worker receives installed tools despite a disconnected extension."""
    dm = rig(daemon, [WORK], set())
    dm.supervisor.availability = dm._availability
    install(daemon, [sup("task", "On it.", task("fill the form on example.com", "owner", sites=("example.com",)))],
            {"worker": STALL})
    c = Client(daemon.sock, timeout=30)
    try:
        for ev in c.stream("chat.send", {"session_id": "s", "text": "fill the form on example.com"}):
            if ev["type"] == "plan":
                t = daemon.daemon.tasks.get(ev["task_id"] + ".worker")
                assert t is not None and {"browser:read", "browser:act"} <= set(t.task.scopes)
                assert {"fs:read", "fs:write", "mac:read", "mac:act", "shell"} <= set(t.task.scopes)
                c2 = Client(daemon.sock)
                c2.call("task.stop", {"task_ids": [ev["task_id"]]})
                c2.close()
            if ev["type"] == "outcome":
                break
    finally:
        c.close()
