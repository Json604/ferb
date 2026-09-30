"""Build step 16: environment-aware resolution (section 9.3) and calls / messages with the undo countdown (10.3).
- the route order: the owner's correction > most used over 14 days (ties: most recent) > installed app > paired web;
- FERB's own activity never counts; an unpaired profile is never a route; nothing workable -> unknown (ask);
- services are found in the catalogue, by an installed app's name, or by a visited site's name;
- the planner gets the environment summary before the owner's message and the live part at the end; the route is
  said in plain words when the task starts;
- a call the owner asked for is held with the undo countdown; one they didn't ask for is blocked.
"""

import json
from datetime import datetime, timedelta, timezone
from typing import Any

from ferb.act.mac.tools import call_or_send
from ferb.policy import services as svc
from ferb.policy.intent import RequestIntent
from ferb.policy.rules import Kind, Target
from ferb.policy.services import Environment
from tests.conftest import RunningDaemon
from tests.test_policy import act, engine
from tests.test_supervisor import chat, install, plan, sub, sup

NOW = datetime(2026, 9, 30, 12, 0, tzinfo=timezone.utc)
WA_APP = ("net.whatsapp.WhatsApp", "WhatsApp")
BRAVE_WORK = ("brave", "Default", "Work")


def env(**kw: Any) -> Environment:
    base: dict[str, Any] = {"apps": [WA_APP, ("com.apple.Notes", "Notes")], "paired": [BRAVE_WORK]}
    return Environment(**(base | kw))


def wa() -> svc.Service:
    return next(s for s in svc.CATALOGUE if s.name == "WhatsApp")


# ---------- the route order ----------

def test_an_installed_app_wins_over_the_website_when_there_is_no_usage() -> None:
    r = svc.route(wa(), env())
    assert r is not None and (r.kind, r.target) == ("app", "net.whatsapp.WhatsApp")
    assert r.say() == "Using the WhatsApp app"


def test_how_the_owner_actually_uses_it_beats_the_installed_app() -> None:
    e = env(usage_apps={"net.whatsapp.WhatsApp": (2, NOW)},
            usage_web={("web.whatsapp.com", "brave", "Default"): (9, NOW - timedelta(days=1))})
    r = svc.route(wa(), e)
    assert r is not None and (r.kind, r.target, r.profile) == ("web", "web.whatsapp.com", "Default")
    assert "Brave (Work) profile" in r.say()


def test_a_tie_goes_to_the_most_recent() -> None:
    e = env(usage_apps={"net.whatsapp.WhatsApp": (5, NOW - timedelta(days=3))},
            usage_web={("web.whatsapp.com", "brave", "Default"): (5, NOW)})
    assert svc.route(wa(), e).kind == "web"                                        # type: ignore[union-attr]
    e.usage_apps["net.whatsapp.WhatsApp"] = (5, NOW + timedelta(minutes=1))
    assert svc.route(wa(), e).kind == "app"                                        # type: ignore[union-attr]


def test_the_owners_correction_always_wins() -> None:
    e = env(usage_apps={"net.whatsapp.WhatsApp": (50, NOW)},
            preferences={"route:whatsapp": "use the web version for WhatsApp"})
    r = svc.route(wa(), e)
    assert r is not None and r.kind == "web" and r.reason == "you asked for the web version"
    e.preferences = {"route:whatsapp": "use the app"}
    e.usage_web = {("web.whatsapp.com", "brave", "Default"): (50, NOW)}
    assert svc.route(wa(), e).kind == "app"                                        # type: ignore[union-attr]


def test_the_website_in_a_paired_profile_when_there_is_no_app() -> None:
    r = svc.route(wa(), env(apps=[]))
    assert r is not None and (r.kind, r.browser, r.profile_name) == ("web", "brave", "Work")


def test_an_unpaired_profile_is_never_a_route_and_nothing_workable_means_ask() -> None:
    e = env(apps=[], paired=[], usage_web={("web.whatsapp.com", "chrome", "Profile 2"): (30, NOW)})
    assert svc.route(wa(), e) is None
    e.paired = [BRAVE_WORK]                                                        # used in Chrome, only Brave paired
    r = svc.route(wa(), e)
    assert r is not None and (r.browser, r.profile) == ("brave", "Default")


def test_ferbs_own_activity_never_counts_as_usage(daemon: RunningDaemon) -> None:
    """FERB used WhatsApp Web 20 times; the owner used the app once. The owner's habit wins."""
    d = daemon.daemon
    d._installed_apps = lambda: [WA_APP]                                           # type: ignore[method-assign]
    now = d.clock.now()
    for i, actor in enumerate(["ferb"] * 20 + ["owner"]):
        daemon.run(_trail(daemon, f"t{i}", actor, now))
    got = daemon.run(_environment(daemon, "call mom on whatsapp"))
    assert [(r.kind, r.target, r.reason) for r in got["routes"]] == [("app", "net.whatsapp.WhatsApp", "how you use it")]


async def _environment(d: RunningDaemon, words: str) -> Any:
    return d.daemon._environment(words)


async def _trail(d: RunningDaemon, eid: str, actor: str, now: datetime) -> None:
    src, data = ("app_focus", {"bundle_id": "net.whatsapp.WhatsApp"}) if actor == "owner" else \
        ("browser_tab", {"host": "web.whatsapp.com", "browser": "brave", "profile": "Default"})
    d.daemon.db.conn.execute(                                                      # type: ignore[union-attr]
        "INSERT INTO trail (id, at, day, device_id, source, actor, data_json) VALUES (?, ?, ?, 'dev', ?, ?, ?)",
        (eid, now.isoformat(), now.date().isoformat(), src, actor, json.dumps(data)))


# ---------- finding what the owner named ----------

def test_services_come_from_the_catalogue_installed_apps_and_visited_sites() -> None:
    e = env(apps=[WA_APP, ("com.culturedcode.ThingsMac", "Things")],
            usage_web={("app.linear.app", "brave", "Default"): (3, NOW)})
    names = [s.name for s in svc.services_named("video call Rahul on whatsapp, add it to things, then linear", e)]
    assert names == ["WhatsApp", "Things", "Linear"]
    assert svc.services_named("what's the weather", e) == []


# ---------- what the planner sees ----------

def test_the_planner_gets_the_environment_and_the_route_is_said_when_starting(daemon: RunningDaemon) -> None:
    route = svc.Route("WhatsApp", "app", "net.whatsapp.WhatsApp", reason="installed")
    daemon.daemon.supervisor.environment = lambda words: {                         # type: ignore[union-attr]
        "routes": [route] if "whatsapp" in words.lower() else [],
        "stable": "This Mac: macOS 26.0. Browser profiles FERB can use: Brave “Work”.",
        "live": "Running now: WhatsApp."}
    script = install(daemon, [sup("plan", "Calling Rahul.", plan([sub("a")], paths=("x.md",)))], {"a": "__stall__"})
    evs = []
    from ferb.cli.client import Client
    c = Client(daemon.sock, timeout=30)
    try:
        for ev in c.stream("chat.send", {"session_id": "s", "text": "video call Rahul on WhatsApp and note it in x.md"}):
            evs.append(ev)
            if ev["type"] == "plan":                                               # then Ctrl-C
                c2 = Client(daemon.sock)
                c2.call("task.stop", {"task_ids": [ev["task_id"]]})
                c2.close()
            if ev["type"] == "outcome":
                break
    finally:
        c.close()
    ctx = script.contexts[0][2]
    stable, mine, live = (ctx.index("This Mac: macOS"), ctx.index("Owner's new message"), ctx.index("Running now"))
    assert stable < mine < live                                                    # the changing part goes last
    assert "WhatsApp → the app (net.whatsapp.WhatsApp)" in ctx
    assert evs[0]["type"] == "plan", (evs, [c[2][-400:] for c in script.contexts[1:]])
    assert evs[0]["text"].startswith("Using the WhatsApp app. ")


# ---------- calls and messages ----------

def test_call_and_send_buttons_are_recognised() -> None:
    assert call_or_send("Video call") == Kind.CALL and call_or_send("Start a call") == Kind.CALL
    assert call_or_send("FaceTime Audio") == Kind.CALL
    assert call_or_send("Send") == Kind.SEND and call_or_send("Send message") == Kind.SEND
    assert call_or_send("Search") is None and call_or_send("Recall settings") is None


def test_a_call_the_owner_asked_for_is_held_for_undo_and_one_they_didnt_is_blocked() -> None:
    e, clock = engine()
    e.intents.record(RequestIntent.from_plan("r1", {"call"}, recipients=frozenset({"rahul"})))
    call = act(Kind.CALL, {"mac:act"}, target=Target(recipients=("rahul",)))
    v = e.evaluate(call)
    assert v.decision == "hold"                                                    # "Calling Rahul… undo"
    assert e.undo(v.hold_id)
    clock.advance(11)
    assert e.release_due() == []                                                   # undone: never happens
    e.intents.record(RequestIntent.from_plan("r2", {"edit_file"}))
    assert e.evaluate(act(Kind.CALL, {"mac:act"}, request_id="r2")).decision == "block"
