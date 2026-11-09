"""APP-777 (owner, 2026-11-07, refining APP-1): where FERB browses while it has no browser profile of its own.
A task the planner gives to FERB works as the owner in the profile they picked; not picked yet, FERB asks once (the
task waits), and the answer is kept; None goes without a browser and says what that costs; a run nobody attends goes
on without one and says why; a schedule asks when it is made; a picked profile that's gone is asked about again."""

import json
import threading
from datetime import timedelta
from typing import Any

import pytest

from ferb.act.browser.profiles import BrowserProfile
from ferb.orchestrator import browsing
from tests.conftest import RunningDaemon, answer_question, chat, install, outcome, sup, task
from tests.support.schedules import fire, run, schedule_reply

WORK = BrowserProfile("chrome", "Profile 1", "Work", "me@work.example")
HOME = BrowserProfile("brave", "Default", "Home", "")
WORK_LABEL = "Google Chrome “Work” (me@work.example)"


def _profiles(d: RunningDaemon, *found: BrowserProfile) -> None:
    d.daemon._browsing_profiles = lambda: list(found)      # type: ignore[method-assign]


def _worker(seen: dict[str, Any], d: RunningDaemon) -> Any:
    def act(token: str) -> str:
        seen["identity"] = getattr(d.daemon.tokens.validate(token).identity, "value", None)
        return json.dumps({"status": "done", "summary": "Found it.", "answer": "Found it.", "artifacts": [],
                           "evidence": []})
    return act


def _chat_while(d: RunningDaemon, text: str, answer: Any) -> list[dict[str, Any]]:
    """A chat turn whose task waits on FERB's question, answered from another thread as the owner would."""
    got: dict[str, Any] = {}
    owner = threading.Thread(target=lambda: got.setdefault("ap", answer()))
    owner.start()
    events = chat(d, text)
    owner.join(20)
    return events


def _identity_row(d: RunningDaemon) -> str:
    return str(run(d, lambda: d.daemon.db.conn.execute(
        "SELECT identity FROM tasks WHERE id LIKE '%.worker' ORDER BY created_at DESC LIMIT 1").fetchone()[0]))


# decision: APP-777
# decision: HAND-426
def test_a_task_ferb_runs_as_itself_works_as_the_owner_in_their_picked_profile(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _profiles(daemon, WORK, HOME)
    d.config.set("browsing", "owner_profile", "chrome/Profile 1")
    seen: dict[str, Any] = {}
    install(daemon, [sup("task", "Looking it up.", task("Find the answer on the web"))], {"worker": _worker(seen, daemon)})
    out = outcome(chat(daemon, "find the answer on the web"))
    assert seen["identity"] == "owner" and _identity_row(daemon) == "owner"        # the owner's browser tools
    assert not [ap for ap in d.state.approvals.pending() if ap.capability == browsing.BROWSING_ASK]
    assert browsing.CHOSEN.format(label=WORK_LABEL) not in json.dumps(out, ensure_ascii=False)       # said only when just answered


# decision: APP-777
# decision: HAND-426
def test_with_no_pick_ferb_asks_once_and_the_task_waits_for_the_answer(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _profiles(daemon, WORK, HOME)
    seen: dict[str, Any] = {}
    install(daemon, [sup("task", "Looking it up.", task("Find the answer on the web")),
                     sup("task", "Again.", task("Find another answer on the web"))], {"worker": _worker(seen, daemon)})
    asked: dict[str, Any] = {}

    def owner() -> Any:
        ap = answer_question(daemon, browsing.BROWSING_ASK, choice=WORK_LABEL)
        asked["question"], asked["options"] = ap.reason, list(ap.options)
        return ap
    out = outcome(_chat_while(daemon, "find the answer on the web", owner))
    assert asked["options"] == [WORK_LABEL, "Brave “Home”", browsing.NONE]       # each profile with its account
    assert asked["question"] == browsing.QUESTION
    assert d.config.get("browsing", "owner_profile") == "chrome/Profile 1"
    assert d.config.get("browsing", browsing.ASKED) is True
    assert seen["identity"] == "owner" and browsing.CHOSEN.format(label=WORK_LABEL) in json.dumps(out, ensure_ascii=False)
    seen.clear()
    outcome(chat(daemon, "find another answer on the web"))                       # never asked again
    assert seen["identity"] == "owner"


# decision: APP-777
# decision: HAND-426
def test_none_goes_without_a_browser_says_what_it_costs_once_and_never_asks_again(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _profiles(daemon, WORK)
    seen: dict[str, Any] = {}
    install(daemon, [sup("task", "Looking it up.", task("Find the answer on the web")),
                     sup("task", "Again.", task("Find another answer on the web"))], {"worker": _worker(seen, daemon)})
    out = outcome(_chat_while(daemon, "find the answer on the web",
                              lambda: answer_question(daemon, browsing.BROWSING_ASK, choice=browsing.NONE)))
    assert seen["identity"] == "ferb" and browsing.NONE_CHOSEN in json.dumps(out, ensure_ascii=False)
    assert d.config.get("browsing", "owner_profile") == "" and d.config.get("browsing", browsing.ASKED) is True
    later = outcome(chat(daemon, "find another answer on the web"))
    assert browsing.NONE_CHOSEN not in json.dumps(later, ensure_ascii=False)
    assert run(daemon, lambda: d._browsing(True, "r")) == {"declined": True}
    assert browsing.NONE_FACT in run(daemon, lambda: d._availability().text)                        # the planner can say why when asked


# decision: APP-778
# decision: HAND-426
def test_a_run_nobody_attends_goes_without_a_browser_and_says_why(daemon: RunningDaemon, unattended: None) -> None:
    d = daemon.daemon
    _profiles(daemon, WORK)
    seen: dict[str, Any] = {}
    install(daemon, [sup("task", "Looking it up.", task("Find the answer on the web"))], {"worker": _worker(seen, daemon)})
    out = outcome(chat(daemon, "find the answer on the web"))
    assert seen["identity"] == "ferb" and browsing.NO_BROWSER_RUN in json.dumps(out, ensure_ascii=False)
    assert not d.state.approvals.pending() and d.config.get("browsing", browsing.ASKED) is None


# decision: APP-778
# decision: HAND-426
def test_a_schedule_that_browses_as_ferb_asks_when_it_is_made(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _profiles(daemon, WORK)
    words = "every day check the news"
    reply = json.loads(schedule_reply(daemon, "check the news", words, d.clock.now() + timedelta(minutes=1),
                                      repeat="daily", paths=()))
    reply["schedule"] |= {"until": None, "when_active": False, "folders": [], "identity": "ferb",
                          "worker_type": "default", "profile": "", "account": ""}
    install(daemon, [json.dumps(reply)])
    chat(daemon, words)
    [ap] = [ap for ap in d.state.approvals.pending() if ap.capability == browsing.BROWSING_ASK]
    assert ap.task_id in ("", None) and list(ap.options) == [WORK_LABEL, browsing.NONE]
    d.state.approvals.resolve(ap.id, True, 1)
    assert run(daemon, lambda: d._question_answered(ap)) == browsing.CHOSEN.format(label=WORK_LABEL)
    assert d.config.get("browsing", "owner_profile") == "chrome/Profile 1"
    assert run(daemon, lambda: d._question_answered(ap)) == ""                                         # applied once, whoever saw it first


# decision: APP-777
# decision: APP-778
# decision: HAND-426
def test_a_picked_profile_that_is_gone_is_asked_about_again_and_one_question_is_open(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _profiles(daemon, HOME)
    d.config.set("browsing", "owner_profile", "chrome/Profile 1")
    d.config.set("browsing", browsing.ASKED, True)
    first = run(daemon, lambda: d._browsing(True, "r1"))
    assert run(daemon, lambda: d._browsing(True, "r2")) == first                                       # the open question, not a second
    ap = d.state.approvals.get(first["ask"])
    assert browsing.GONE.format(label="Google Chrome “Profile 1”") in ap.reason
    assert run(daemon, lambda: d._browsing(False, "r3"))["note"].startswith("This run had no browser: the profile")
    d.config.set("identity", "profile", "Profile 7")                              # FERB's own profile: as before
    assert run(daemon, lambda: d._browsing(True, "r4")) == {}


# decision: APP-777
# decision: HAND-426
@pytest.mark.parametrize("typed", ["1", "work"])
def test_only_an_options_own_words_answer_the_browsing_question(daemon: RunningDaemon, typed: str) -> None:
    from ferb.cli.client import Client, RpcError
    d = daemon.daemon
    _profiles(daemon, WORK)
    asked = run(daemon, lambda: d._browsing(True, "r"))["ask"]
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError):
            c.call("chat.answer", {"approval_id": asked, "text": typed})
    finally:
        c.close()
    assert d.state.approvals.get(asked).state == "pending"


# decision: APP-777
# decision: HAND-426
def test_a_picked_profile_not_paired_yet_is_paired_rather_than_refused() -> None:
    from ferb.act.browser.resolver import NeedsOwner, Resolver
    picked = Resolver(lambda: [WORK], lambda: [], lambda: None, owner_default=lambda: ("chrome", "Profile 1"))
    with pytest.raises(NeedsOwner, match="browser_pair profile=chrome/Profile 1"):
        picked.choose(None)


# decision: APP-777
# decision: HAND-426
def test_the_notch_buttons_answer_it_and_the_pick_is_stored(daemon: RunningDaemon) -> None:
    from ferb.cli.client import Client
    d = daemon.daemon
    _profiles(daemon, WORK)
    asked = run(daemon, lambda: d._browsing(True, "r"))["ask"]
    c = Client(daemon.sock)
    try:
        c.call("chat.approve", {"approval_id": asked, "granted": True, "choice": 1})
    finally:
        c.close()
    assert d.config.get("browsing", "owner_profile") == "chrome/Profile 1"
    assert run(daemon, lambda: d._browsing(True, "r2")) == {"as_owner": True, "label": WORK_LABEL}


# decision: APP-778
def test_a_schedule_says_its_runs_browse_in_the_owners_pick(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _profiles(daemon, WORK)
    d.config.set("browsing", "owner_profile", "chrome/Profile 1")
    words = "every day check the news"
    reply = json.loads(schedule_reply(daemon, "check the news", words, d.clock.now() + timedelta(minutes=1),
                                      repeat="daily", paths=()))
    reply["schedule"] |= {"until": None, "when_active": False, "folders": [], "identity": "ferb",
                          "worker_type": "default", "profile": "", "account": ""}
    install(daemon, [json.dumps(reply)])
    told = "\n".join(str(e.get("text")) for e in chat(daemon, words))
    assert "as you, in your browser profile “Work”" in told and "FERB's own browser" not in told
    assert not d.state.approvals.pending()
