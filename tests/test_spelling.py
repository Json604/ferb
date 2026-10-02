"""Misspelled app names in the owner's own words (owner decision 2026-10-01): one close match is used and said; several
are asked about; a plan that still names something the owner didn't gets the real reason, not a vague apology."""

import json

from ferb.orchestrator.supervisor import SORRY_PLAN
from ferb.policy.spelling import distance, fix
from tests.conftest import RunningDaemon
from tests.test_supervisor import chat, install, plan, sub, sup

NAMES = ["Slack", "Slides", "WhatsApp", "Notes", "Notion", "Gmail", "Mail", "Spotify", "Zoom", "Zoho"]


def test_one_close_name_is_used_and_said() -> None:
    assert distance("salck", "slack") == 1                            # a swap of neighbours is one edit
    f = fix("check if ive got any salck messages", NAMES)
    assert f.text == "check if ive got any Slack messages" and f.note() == "Using Slack, from “salck”."
    assert fix("open whatsap", NAMES).text == "open WhatsApp"


def test_real_words_and_exact_names_are_left_alone() -> None:
    for text in ("check my email inbox", "take notes", "message the team", "slack off", "open Slack"):
        f = fix(text, NAMES)
        assert f.text == text and not f.fixes and not f.ambiguous, text


def test_a_word_close_to_several_names_is_asked_about() -> None:
    f = fix("join the zooh call", NAMES)                              # one letter from Zoom and from Zoho
    assert f.text == "join the zooh call" and not f.fixes
    assert f.question() == "Did you mean Zoho or Zoom (for “zooh”)?"


def _slack_plan() -> dict:
    p = plan([sub("check", role="reader", identity="owner", scopes=("mac:read",), fields=(("unread", "string"),))],
             verbs=("summarize",))
    p["intent"]["apps"] = ["Slack"]
    return p


def test_a_typo_doesnt_stop_the_plan(daemon: RunningDaemon) -> None:
    daemon.daemon.supervisor.apps = lambda: [("com.tinyspeck.slackmacgap", "Slack")]   # type: ignore[union-attr]
    worker = json.dumps({"status": "done", "summary": "read", "fields": {"unread": "none"}, "artifacts": [],
                         "evidence": []})
    install(daemon, [sup("plan", "Checking Slack.", _slack_plan())], {"check": worker})
    evs = chat(daemon, "check if ive got any salck messages")
    first = next(e for e in evs if e["type"] == "plan")
    assert first["text"].startswith("Using Slack, from “salck”.")


def test_an_ambiguous_typo_asks_and_the_answer_carries_on(daemon: RunningDaemon) -> None:
    daemon.daemon.supervisor.apps = lambda: [("us.zoom.xos", "Zoom"), ("com.zoho.mail", "Zoho")]  # type: ignore[union-attr]
    worker = json.dumps({"status": "done", "summary": "read", "fields": {"unread": "none"}, "artifacts": [],
                         "evidence": []})
    zoom = _slack_plan()
    zoom["intent"]["apps"] = ["Zoom"]
    script = install(daemon, [sup("plan", "Checking Zoom.", zoom)], {"check": worker})
    asked = chat(daemon, "check the zooh chat")
    assert [e["type"] for e in asked] == ["clarify"] and "Zoho or Zoom" in asked[0]["text"]
    assert script.contexts == []                                       # nothing planned yet
    evs = chat(daemon, "Zoom")                                         # the answer: no second question
    assert any(e["type"] == "plan" for e in evs)


def test_a_name_the_owner_never_said_is_asked_about_plainly(daemon: RunningDaemon) -> None:
    daemon.daemon.supervisor.apps = lambda: [("com.tinyspeck.slackmacgap", "Slack")]   # type: ignore[union-attr]
    install(daemon, [sup("plan", "x", _slack_plan())] * 3)              # keeps naming Slack after each repair
    evs = chat(daemon, "check my work messages")
    assert evs[-1]["type"] == "clarify" and "“Slack”" in evs[-1]["text"] and "pasted" not in evs[-1]["text"]
    assert "which app, site or file" in evs[-1]["text"]


def test_other_plan_problems_say_the_real_reason(daemon: RunningDaemon) -> None:
    bad = plan([sub("a", deps=("b",)), sub("b", deps=("a",))], paths=("notes.txt",))   # steps in a circle
    install(daemon, [sup("plan", "x", bad)] * 3)
    evs = chat(daemon, "tidy notes.txt")
    assert evs[-1]["type"] == "reply" and evs[-1]["text"] == SORRY_PLAN               # the planner's mistake, said so
    assert "circle" not in evs[-1]["text"]                                              # without its internals


def test_word_forms_are_english_too() -> None:
    """The Mac's dictionary has "look" but not "looks": word forms must not become app names."""
    names = NAMES + ["Books", "Music", "Maps"]
    for text in ("see what the reminder looks like", "in two minutes", "it stopped", "maps of the hooks", "musics"):
        assert fix(text, names).text == text, text
    assert fix("open bookz", names).text == "open Books"
