"""Build step 16a: never guess between contacts, enforced in code.
- a call / send to a person must name the contact (name as the owner said it, selected + detail as shown);
- the selected contact and its detail must have been on screen in this task;
- several entries matching the name, with nothing the owner said picking one -> "Which Rahul?" (numbered);
- the owner's words ("Rahul from work", or a memory they stated) resolve it without asking;
- the answer applies to that one action only: nothing is stored;
- the undo countdown names the exact contact with its detail.
"""

import asyncio
from typing import Any

from pydantic import BaseModel

from ferb.cli.chat import question, render
from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy import contacts
from ferb.policy.contacts import Contact
from ferb.policy.rules import Kind
from ferb.policy.targets import Request, Targets
from tests.conftest import Answers, RunningDaemon
from tests.test_gateway import setup

WA = "net.whatsapp.WhatsApp"
TWO_RAHULS = {"bundle_id": WA, "app": "WhatsApp", "elements": [
    {"id": "s1e1", "role": "AXTextField", "label": "Search", "value": "Rahul"},
    {"id": "s1e2", "role": "AXCell", "label": "Rahul Sharma", "value": "+91 98100 11111 · Work"},
    {"id": "s1e3", "role": "AXCell", "label": "Rahul Verma", "value": "+91 98200 22222 · Gym"},
    {"id": "s1e4", "role": "AXButton", "label": "Video call"}], "text": ""}
ONE_RAHUL = {"bundle_id": WA, "app": "WhatsApp", "elements": [
    {"id": "s2e1", "role": "AXStaticText", "label": "Rahul Verma", "value": "+91 98200 22222"},
    {"id": "s2e2", "role": "AXButton", "label": "Video call"}], "text": ""}
VERMA = {"name": "Rahul", "selected": "Rahul Verma", "detail": "+91 98200 22222"}
SHARMA = {"name": "Rahul", "selected": "Rahul Sharma", "detail": "+91 98100 11111"}


class Snap:
    name, description, identities = "mac.snapshot", "fake snapshot", {Identity.OWNER}
    scopes, risk, reversible = {"mac:read"}, RiskLevel.LOW, True

    class Input(BaseModel):
        app: str

    def __init__(self) -> None:
        self.screens: list[dict[str, Any]] = []

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ, Target(site=WA))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        return ToolResult(self.screens.pop(0))


class Call:
    name, description, identities = "mac.act", "fake call button", {Identity.OWNER}
    scopes, risk, reversible = {"mac:act"}, RiskLevel.MEDIUM, False

    class Input(BaseModel):
        app: str
        contact: Contact | None = None

    def __init__(self) -> None:
        self.calls = 0

    def _bundle(self, args: BaseModel) -> str:
        return WA

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.CALL, Target(site=WA))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.calls += 1
        return ToolResult({"ok": True})


def rig(d: RunningDaemon, *screens: dict[str, Any], words: str = "video call rahul on whatsapp") -> tuple[Any, Call]:
    snap, call = Snap(), Call()
    snap.screens = list(screens)
    grant, _ = setup(d, {"mac:read", "mac:act"},
                     Request("req-1", words, Targets.of(recipients=["Rahul"], apps=[WA])), [snap, call])  # type: ignore[list-item]
    d.daemon.gateway.owner_words = lambda rid, name: words
    for _ in screens:
        assert not d.run(d.daemon.gateway.call(grant, "mac.snapshot", {"app": "WhatsApp"}))["is_error"]
    return grant, call


def press_call(d: RunningDaemon, grant: Any, contact: dict[str, Any] | None) -> dict[str, Any]:
    async def go() -> dict[str, Any]:
        job = asyncio.create_task(d.daemon.gateway.call(grant, "mac.act", {"app": "WhatsApp", "contact": contact,
                                                                           "effect": "send", "why": "calling Rahul"}))
        for _ in range(100):                                            # past the undo window
            await asyncio.sleep(0.05)
            d.daemon.clock.advance(1)
            if job.done():
                break
        return await asyncio.wait_for(job, 5)
    return d.run(go(), timeout=20)


def listen(d: RunningDaemon) -> list[tuple[str, dict[str, Any]]]:
    got: list[tuple[str, dict[str, Any]]] = []
    real = d.daemon.gateway.publish

    def pub(topic: str, data: dict[str, Any]) -> None:
        got.append((topic, data))
        real(topic, data)
    d.daemon.gateway.publish = pub                                      # type: ignore[method-assign]
    return got


# ---------- the rule, in code ----------

def test_several_matches_and_nothing_to_pick_one_means_ask() -> None:
    shots = [contacts.entries(TWO_RAHULS)]
    r = contacts.check(Contact(**VERMA), shots, "video call rahul")
    assert r.decision == "ask" and r.why == "Which Rahul did you mean?"
    assert [o.split(" ")[:2] for o in r.options] == [["Rahul", "Sharma"], ["Rahul", "Verma"]]  # not the search box


def test_the_owners_words_pick_one() -> None:
    shots = [contacts.entries(TWO_RAHULS)]
    assert contacts.check(Contact(**SHARMA), shots, "video call rahul from work").decision == "ok"
    wrong = contacts.check(Contact(**VERMA), shots, "video call rahul from work")
    assert wrong.decision == "refuse" and "Rahul Sharma" in wrong.why
    stated = "call rahul\nperson:rahul Rahul always means Rahul Sharma (work)"          # an owner-stated memory
    assert contacts.check(Contact(**SHARMA), shots, stated).decision == "ok"


def test_a_contact_that_was_never_on_screen_is_refused() -> None:
    shots = [contacts.entries(ONE_RAHUL)]
    assert contacts.check(Contact(**VERMA), shots, "call rahul").decision == "ok"            # one match: no question
    made_up = contacts.check(Contact(name="Rahul", selected="Rahul Kapoor", detail="+91 1"), shots, "call rahul")
    assert made_up.decision == "refuse" and "wasn't on screen" in made_up.why
    bad_detail = contacts.check(Contact(name="Rahul", selected="Rahul Verma", detail="+91 99999"), shots, "call rahul")
    assert bad_detail.decision == "refuse" and "detail" in bad_detail.why
    assert contacts.check(Contact(**VERMA), [], "call rahul").decision == "refuse"             # no snapshot at all


# ---------- through the gateway ----------

def test_which_rahul_is_asked_and_the_answer_is_for_that_one_action(daemon: RunningDaemon) -> None:
    got = listen(daemon)
    grant, call = rig(daemon, TWO_RAHULS, ONE_RAHUL)
    pick = Answers(daemon, choice=2)                                   # the owner: "2" (Rahul Verma)
    try:
        r = press_call(daemon, grant, VERMA)
        assert not r["is_error"] and call.calls == 1, r
        asked = [ap for ap in pick.asked if ap.options]
        assert len(asked) == 1 and asked[0].reason == "Which Rahul did you mean?"
        held_ev = [d for t, d in got if t == "action.held"]
        assert held_ev and held_ev[0]["to"] == "Rahul Verma (+91 98200 22222)"
        assert render({"type": "event", "topic": "action.held", "data": held_ev[0]}) == \
            "   📞 Calling Rahul Verma (+91 98200 22222) in 10 s… (Ctrl-C to cancel)"
        again = press_call(daemon, grant, VERMA)                    # the next call: asked again (nothing stored)
        assert not again["is_error"] and len([ap for ap in pick.asked if ap.options]) == 2
    finally:
        pick.stop()
    mem = daemon.daemon.memory
    assert mem is None or not [it for it in daemon.run(_items(daemon)) if "verma" in it.content.lower()]


async def _items(d: RunningDaemon) -> list[Any]:
    return d.daemon.memory.items(None)                                 # type: ignore[union-attr]


def test_a_wrong_pick_is_refused_then_the_retry_uses_the_owners_answer(daemon: RunningDaemon) -> None:
    grant, call = rig(daemon, TWO_RAHULS)
    pick = Answers(daemon, choice=1)                                   # the owner meant Rahul Sharma
    try:
        r = press_call(daemon, grant, VERMA)
        assert r["is_error"] and "Rahul Sharma" in r["content"]["error"] and call.calls == 0
        ok = press_call(daemon, grant, SHARMA)                      # the worker reselects: no second question
        assert not ok["is_error"] and call.calls == 1
        assert len([ap for ap in pick.asked if ap.options]) == 1
    finally:
        pick.stop()


def test_cancelling_the_question_stops_the_call(daemon: RunningDaemon) -> None:
    grant, call = rig(daemon, TWO_RAHULS)
    nobody = Answers(daemon, choice=None)                              # Enter: cancel
    try:
        r = press_call(daemon, grant, VERMA)
        assert r["is_error"] and "declined" in r["content"]["error"] and call.calls == 0
    finally:
        nobody.stop()


def test_the_owners_words_resolve_it_without_asking(daemon: RunningDaemon) -> None:
    grant, call = rig(daemon, TWO_RAHULS, words="video call rahul from work on whatsapp")
    watch = Answers(daemon)
    try:
        assert not press_call(daemon, grant, SHARMA)["is_error"] and call.calls == 1
        assert watch.asked == []
    finally:
        watch.stop()


def test_a_call_must_say_who_and_someone_the_owner_never_named_is_asked_about(daemon: RunningDaemon) -> None:
    """A call names its contact. Calling someone the owner never named isn't refused outright: it's a send to a
    stranger, so FERB asks the owner (with the worker's reason) and a no stops it."""
    grant, call = rig(daemon, ONE_RAHUL)
    r = press_call(daemon, grant, None)
    assert r["is_error"] and "say who this is for" in r["content"]["error"]
    no = Answers(daemon, granted=False)
    try:
        r = press_call(daemon, grant, {**VERMA, "name": "Priya"})
    finally:
        no.stop()
    assert r["is_error"] and "declined" in r["content"]["error"] and call.calls == 0
    assert len(no.asked) == 1 and "calling Rahul" in no.asked[0].reason


def test_the_choice_question_reads_as_numbered_options() -> None:
    q = question({"kind": "contact_choice", "reason": "Which Rahul did you mean?",
                  "options": ["Rahul Sharma +91 98100 11111 · Work", "Rahul Verma +91 98200 22222 · Gym"]})
    assert "1) Rahul Sharma" in q and "2) Rahul Verma" in q and q.rstrip().endswith("number (Enter to cancel):")


# ---------- screenshot clicks in messaging / calling apps ----------

def _whatsapp(d: RunningDaemon, mp: Any, cells: list[tuple[str, str]], at_point: str | None = None) -> tuple[Any, Any]:
    from datetime import timedelta
    from ferb.act.mac.accessibility import AxNode, AxTree
    from ferb.act.mac.apps import RunningApp
    from ferb.core.types import Task
    from ferb.orchestrator.tree import TaskRecord
    from tests.test_mac_vision import _fake_mac
    shots, posted = _fake_mac(d, mp)
    shots.top = 150                                                              # WhatsApp's window (pid 15)
    mac = d.daemon.mac
    mac.apps.apps.append(RunningApp(WA, "WhatsApp", 15, False))                  # type: ignore[union-attr]
    mac.ax.tree = AxTree(nodes=[AxNode(None, "AXCell", n, v, (0, 0, 10, 10), False, True) for n, v in cells])  # type: ignore[union-attr]
    mac.ax.label_at = lambda x, y: at_point                                      # type: ignore[union-attr]
    d.daemon.gateway.owner_words = lambda rid, name: "video call rahul on whatsapp"
    t = Task("wa", None, "g", "dev", Identity.OWNER, {"screen", "mac:act"}, "worker", "req-wa", [],
             d.daemon.clock.now() + timedelta(minutes=10))

    async def go() -> Any:
        d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), d.daemon.spans.start("task", task_id="wa", input={}).id))
        d.daemon.engine.requests.record(Request("req-wa", "video call rahul on whatsapp",
                                                Targets.of(recipients=["Rahul"], apps=[WA])))
        return d.daemon.tokens.validate(d.daemon.tokens.issue("wa", "w-wa", t.scopes, Identity.OWNER, ttl_s=600))
    return d.run(go()), posted


def _click(d: RunningDaemon, grant: Any, **args: Any) -> dict[str, Any]:
    async def go() -> dict[str, Any]:
        shot = await d.daemon.gateway.call(grant, "mac.screenshot", {"app": "WhatsApp"})
        assert not shot["is_error"], shot
        job = asyncio.create_task(d.daemon.gateway.call(
            grant, "mac.click", {"app": "WhatsApp", "shot": shot["content"]["shot"], "x": 400, "y": 300,
                                 "why": "the call button", **args}))
        for _ in range(100):                                                     # past the undo window
            await asyncio.sleep(0.05)
            d.daemon.clock.advance(1)
            if job.done():
                break
        return await asyncio.wait_for(job, 5)
    return d.run(go(), timeout=20)


def test_an_undeclared_click_in_a_messaging_app_is_refused(daemon: RunningDaemon, monkeypatch: Any) -> None:
    grant, posted = _whatsapp(daemon, monkeypatch, [("Rahul Verma", "+91 98200 22222")])
    r = _click(daemon, grant)
    assert r["is_error"] and "messaging / calling app" in r["content"]["error"] and posted == []


def test_other_on_a_call_button_is_still_a_call(daemon: RunningDaemon, monkeypatch: Any) -> None:
    """The control under the point says "Video call": the floor makes it a call whatever the worker declared, and
    a call needs its contact."""
    grant, posted = _whatsapp(daemon, monkeypatch, [("Rahul Verma", "+91 98200 22222")], at_point="Video call")
    r = _click(daemon, grant, purpose="other")
    assert r["is_error"] and "say who this is for" in r["content"]["error"] and posted == []


def test_a_declared_call_click_goes_through_the_contact_check_and_countdown(daemon: RunningDaemon,
                                                                           monkeypatch: Any) -> None:
    got = listen(daemon)
    grant, posted = _whatsapp(daemon, monkeypatch, [("Rahul Verma", "+91 98200 22222")])
    r = _click(daemon, grant, purpose="call", contact=VERMA)
    assert not r["is_error"] and posted[-1].startswith("click"), r
    assert [d["to"] for t, d in got if t == "action.held"] == ["Rahul Verma (+91 98200 22222)"]
    made_up = _click(daemon, grant, purpose="call", contact={**VERMA, "selected": "Rahul Kapoor"})
    assert made_up["is_error"] and "wasn't on screen" in made_up["content"]["error"]
    assert _click(daemon, grant, purpose="call")["content"]["error"].startswith("say who this is for")


def test_two_matches_on_a_screenshot_ask_which_one(daemon: RunningDaemon, monkeypatch: Any) -> None:
    grant, posted = _whatsapp(daemon, monkeypatch, [("Rahul Sharma", "+91 98100 11111"),
                                                   ("Rahul Verma", "+91 98200 22222")])
    pick = Answers(daemon, choice=2)
    try:
        r = _click(daemon, grant, purpose="call", contact=VERMA)
        assert not r["is_error"] and [ap.reason for ap in pick.asked if ap.options] == ["Which Rahul did you mean?"]
    finally:
        pick.stop()


def test_other_apps_need_no_purpose(daemon: RunningDaemon, monkeypatch: Any) -> None:
    from tests.test_mac_shell import call, task
    from tests.test_mac_vision import _fake_mac
    _, posted = _fake_mac(daemon, monkeypatch)
    g = task(daemon, "n", Identity.OWNER, {"screen", "mac:act"}, {"use_app"})
    assert not call(daemon, g, "mac.screenshot", app="Notes")["is_error"]
    assert not call(daemon, g, "mac.click", app="Notes", shot="shot1", x=400, y=300)["is_error"] and posted


def test_a_web_pages_text_and_an_open_chat_dont_count_as_more_contacts() -> None:
    """Found in the real run: a page's text is one line holding every contact, and an open chat repeats the name."""
    page = {"elements": [{"role": "button", "label": "Rahul Sharma · +91 98100 11111 · Work"},
                         {"role": "button", "label": "Rahul Verma · +91 98200 22222 · Gym"},
                         {"role": "button", "label": "Video call"}],
            "text": "Chats Rahul Sharma · +91 98100 11111 · Work Rahul Verma · +91 98200 22222 · Gym Rahul Verma "
                    "+91 98200 22222 Video call"}
    two = contacts.matching("Rahul", contacts.entries(page))
    assert two == ["Rahul Sharma · +91 98100 11111 · Work", "Rahul Verma · +91 98200 22222 · Gym"]
    one = {"elements": [{"role": "button", "label": "Rahul Verma · +91 98200 22222 · Gym"},
                        {"role": "heading", "label": "Rahul Verma"}, {"role": "button", "label": "Video call"}],
           "text": "Chats Rahul Verma · +91 98200 22222 · Gym Priya Nair Rahul Verma +91 98200 22222"}
    assert contacts.matching("Rahul", contacts.entries(one)) == ["Rahul Verma · +91 98200 22222 · Gym"]
    assert contacts.check(Contact(**VERMA), [contacts.entries(one)], "video call rahul").decision == "ok"


# ---------- one message per contact per task (owner decision 2026-10-01) ----------

class Send:
    name, description, identities = "mac.act", "fake send button", {Identity.OWNER}
    scopes, risk, reversible = {"mac:act"}, RiskLevel.MEDIUM, False

    class Input(BaseModel):
        app: str
        text: str | None = None
        contact: Contact | None = None

    def __init__(self) -> None:
        self.sent: list[str | None] = []

    def _bundle(self, args: BaseModel) -> str:
        return WA

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.SEND, Target(site=WA), content=getattr(args, "text"))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.sent.append(getattr(args, "text"))
        return ToolResult({"ok": True})


def send(d: RunningDaemon, grant: Any, text: str) -> dict[str, Any]:
    async def go() -> dict[str, Any]:
        job = asyncio.create_task(d.daemon.gateway.call(grant, "mac.act", {"app": "WhatsApp", "text": text,
                                                                           "contact": VERMA, "effect": "send",
                                                                           "why": "messaging Rahul"}))
        for _ in range(100):                                            # past the undo window
            await asyncio.sleep(0.05)
            d.daemon.clock.advance(1)
            if job.done():
                break
        return await asyncio.wait_for(job, 5)
    return d.run(go(), timeout=20)


def test_a_second_message_to_the_same_contact_needs_the_owners_ok(daemon: RunningDaemon) -> None:
    snap, tool = Snap(), Send()
    snap.screens = [ONE_RAHUL]
    grant, _ = setup(daemon, {"mac:read", "mac:act"},
                     Request("req-1", "message rahul on whatsapp: running late",
                             Targets.of(recipients=["Rahul"], apps=[WA])), [snap, tool])  # type: ignore[list-item]
    daemon.daemon.gateway.owner_words = lambda rid, name: "message rahul on whatsapp: running late"
    assert not daemon.run(daemon.daemon.gateway.call(grant, "mac.snapshot", {"app": "WhatsApp"}))["is_error"]
    owner = Answers(daemon, granted=False)                             # the owner says no to a second one
    try:
        assert not send(daemon, grant, "running late")["is_error"] and tool.sent == ["running late"]
        assert [ap for ap in owner.asked if ap.kind == "send_again"] == []     # the first one isn't asked about
        again = send(daemon, grant, "running late!")
        assert again["is_error"] and "check it by reading the chat" in again["content"]["error"]
        assert tool.sent == ["running late"]                            # nothing sent twice
        asked = [ap for ap in owner.asked if ap.kind == "send_again"]
        assert asked and "running late" in asked[0].reason and "running late!" in asked[0].reason
        assert question({"kind": "send_again", "reason": asked[0].reason}).endswith("[y/N] ")
        owner.granted = True                                           # yes: the second one goes
        assert not send(daemon, grant, "running late!")["is_error"] and tool.sent == ["running late", "running late!"]
    finally:
        owner.stop()
