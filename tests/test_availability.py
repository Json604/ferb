"""What the planner may use right now (owner decision, 2026-09-30): the capability list it sees is built in code from
what is usable now — paired and connected browser profiles, FERB's own browser, the Mac permissions — with one short
"Unavailable: X (reason)" line only when something is missing. Plan validation uses the same scopes (the backstop),
and a plan that needed something missing ends in a plain "can't do that right now", naming the fix.
"""

from types import SimpleNamespace
from typing import Any

from ferb.core.types import Identity
from ferb.orchestrator.availability import build
from tests.conftest import RunningDaemon
from tests.test_supervisor import chat, install, plan, sub, sup

REGISTERED = {"fs:read", "fs:write", "browser:read", "browser:act", "mac:read", "mac:act", "applescript", "screen",
              "shell", "time", "trail:read"}
WORK = SimpleNamespace(browser="brave", folder="Default", identity=Identity.OWNER)


def test_only_usable_groups_are_listed_and_missing_ones_get_one_line() -> None:
    a = build(REGISTERED, {}, "Brave “Work”")
    assert a.scopes == REGISTERED and "connected now: Brave “Work”" in a.text and "Unavailable" not in a.text
    a = build(REGISTERED, {"owner_browsers": "Brave “Work” paired but not open — open it"})
    assert not a.scopes & {"browser:read", "browser:act"} and "The owner's browsers (" not in a.text
    assert a.unavailable_line() == "Unavailable: the owner's browsers (Brave “Work” paired but not open — open it)."
    assert a.text.count("Unavailable:") == 1
    assert "FERB's own browser" not in build({"fs:read"}, {}).text          # not part of this FERB: not mentioned
    mac = build({"mac:read", "mac:act"}, {}).text                           # found in a real run: never more than present
    assert "through Accessibility (mac:act, mac:read)" in mac and "applescript" not in mac.lower()


def rig(d: RunningDaemon, paired: list[Any], connected: set[tuple[str, str]],
        perms: dict[str, bool] | None = None, identity: tuple[str, str] | None = None) -> Any:
    dm = d.daemon
    dm.pairings = SimpleNamespace(all=lambda: paired, for_profile=lambda b, f: None)          # type: ignore[assignment]
    dm.hub.bridges = {(b.browser, b.folder): SimpleNamespace(conn=object() if (b.browser, b.folder) in connected
                                                             else None) for b in paired}      # type: ignore[union-attr]
    dm._profiles = lambda: [SimpleNamespace(browser="brave", folder="Default", display_name="Work")]  # type: ignore
    dm._permissions = lambda: perms or {"accessibility": True, "screen": True}              # type: ignore
    dm._identity_profile = lambda: identity                                                   # type: ignore
    return dm


async def _avail(dm: Any) -> Any:
    return dm._availability()


def test_the_owners_browsers_only_when_a_paired_profile_is_connected(daemon: RunningDaemon) -> None:
    dm = rig(daemon, [], set())
    a = daemon.run(_avail(dm))
    assert "browser:act" not in a.scopes and "none of the owner's browser profiles is paired" in a.unavailable_line()
    rig(daemon, [WORK], set())
    a = daemon.run(_avail(dm))
    assert "Brave “Work” paired but not open" in a.unavailable_line() and "browser:act" not in a.scopes
    rig(daemon, [WORK], {("brave", "Default")})
    a = daemon.run(_avail(dm))
    assert {"browser:read", "browser:act"} <= a.scopes and "connected now: Brave “Work”" in a.text
    assert a.missing == []


def test_ferbs_own_browser_and_the_mac_permissions(daemon: RunningDaemon) -> None:
    dm = rig(daemon, [WORK], {("brave", "Default")}, perms={"accessibility": False, "screen": False},
             identity=("chrome", "Profile 22"))
    a = daemon.run(_avail(dm))
    line = a.unavailable_line()
    assert "FERB's own browser (not paired — run `ferb setup --identity`)" in line
    assert "the owner's Mac apps (FERB doesn't have the Accessibility permission — run `ferb doctor`)" in line
    assert "Screen Recording" in line and not a.scopes & {"mac:act", "mac:read", "applescript", "screen"}
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
    assert "What workers can do right now:" in system and "{capabilities}" not in system
    assert "Unavailable: the owner's browsers (Brave “Work” paired but not open" in system
    assert "The owner's browsers (\"owner\" identity), connected now" not in system
    assert script.contexts                                                   # the turn ran


async def _system(dm: Any) -> str:
    return str(dm.supervisor._system("supervisor.md"))


def test_a_plan_needing_something_missing_gets_a_plain_answer(daemon: RunningDaemon) -> None:
    dm = rig(daemon, [WORK], set())
    dm.supervisor.availability = dm._availability
    p = plan([sub("a", identity="owner", scopes=("browser:act",))], verbs=("fill_form",), sites=("example.com",))
    install(daemon, [sup("plan", "On it.", p)] * 3)                         # the model keeps planning it
    evs = chat(daemon, "fill the form on example.com")
    text = [e["text"] for e in evs if e["type"] == "reply"]
    assert text == ["I can't do that right now. Unavailable: the owner's browsers (Brave “Work” paired but not open "
                    "— open it and FERB reconnects within 30 s)."], evs
