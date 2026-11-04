"""Tier 1, the core profile (kb area memory, MEM-5, MEM-7, MEM-12): always on in every role's prompt,
within a hard budget, rendered by code from the facts (the owner's words word for word, each with its label and id),
updated by code the moment a fact in it changes, and compacted by a weekly model pass that chooses from facts that
hold up and names facts whose time has passed.

Live 2026-10-31: `appointment_preferences` still said "notify me before booking" after the owner said to just book;
the later instruction must replace the earlier one, and FERB's line must say so."""
import json
from datetime import timedelta
from typing import Any

from ferb.memory.profile import PROFILE_TOKENS, CoreProfile, entry_line
from tests.conftest import FakeDistillRuntime, chat, done, install, sup, task
from tests.support.memory_items import journal, owner_fact, run

NOTIFY = "i want jaipur as both the centers. Any date range, just book. BUT notify me before booking"
JUST_BOOK = "no need to notify me, just book the first slot you find"


def profile_text(d: Any) -> str:
    """Tier 1 as every prompt has it: the settled block and the entries newly always on (MEM-429)."""
    return str(run(d, d.daemon._core_profile_text)) + str(run(d, d.daemon._core_profile_pending))


# decision: MEM-7, MEM-21
def test_a_later_instruction_replaces_the_earlier_one_and_the_line_names_it(daemon: Any) -> None:
    """The booking case: the planner sees the earlier fact (its id in retrieved memory) and names it replaced; code
    ends it, the core profile follows at once, and FERB's line names what was replaced."""
    first = owner_fact(daemon, "appointment_preferences", NOTIFY, kind="project", subject="project:us-work-permit")
    run(daemon, lambda: daemon.daemon.profile.add(first.id, "Current projects"))
    assert "notify me before booking" in profile_text(daemon)
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay, I'll just book.", "remember": [
        {"kind": "preference", "subject": "project:us-work-permit", "key": "permit_booking", "content": JUST_BOOK,
         "quote": JUST_BOOK, "replaces": [first.id]}]})])
    events = chat(daemon, JUST_BOOK)
    reply = [e["text"] for e in events if e.get("type") == "reply"][-1]
    assert "replaces" in reply and "notify me before booking" in reply
    assert run(daemon, lambda: daemon.daemon.memory.get(first.id)).valid_to is not None
    text = profile_text(daemon)
    assert JUST_BOOK in text and "notify me before booking" not in text           # Tier 1 followed, by code
    [now] = [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.content == JUST_BOOK]
    assert f"[{now.id}]" in text and "(your words" in text


# decision: MEM-21
def test_the_same_key_replaces_and_is_said_too(daemon: Any) -> None:
    """Fix list: same-key replace was silent; now the line names it whatever path saved it."""
    owner_fact(daemon, "appointment_preferences", NOTIFY, kind="project", subject="project:us-work-permit")
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "remember": [
        {"kind": "project", "subject": "Project:US-WORK permit", "key": "appointment preferences",
         "content": JUST_BOOK, "quote": JUST_BOOK}]})])
    events = chat(daemon, JUST_BOOK)
    assert any("replaces" in e["text"] and "notify me" in e["text"] for e in events if e.get("type") == "reply")
    [now] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (now.subject, now.content) == ("project:us_work_permit", JUST_BOOK)   # subject normalized like the key


# decision: MEM-5, MEM-429
def test_the_profile_is_in_every_roles_instructions(daemon: Any) -> None:
    """Always on in the planner's and the workers' prompts: a newly joined entry right after each one's cached part
    from the turn it joins, then, once settled, in the stable position in the instructions (MEM-429)."""
    owner_fact(daemon, "reply_style", "keep replies short and plain", kind="preference")
    script = install(daemon, [sup("task", "On it.", task("Say hello"))], {"worker": done("Said hello.")})
    chat(daemon, "say hello")
    for key in ("supervisor", "worker"):
        [req] = [r for _, k, r in script.requests if k == key]
        assert "keep replies short and plain" not in req.system
        assert "keep replies short and plain" in "\n".join(req.parts[req.cacheable:])
    run(daemon, lambda: daemon.daemon.profile.settle("test"))
    script = install(daemon, [sup("task", "On it.", task("Say hello again"))], {"worker": done("Said hello.")})
    chat(daemon, "say hello again")
    systems = {key: req.system for _, key, req in script.requests}
    assert "keep replies short and plain" in systems["supervisor"]
    assert "keep replies short and plain" in systems["worker"]
    assert "Tier 1 — core profile" in systems["worker"]
    assert "skills.read" not in systems["supervisor"] and "skills.read" in systems["worker"]   # lookup tools: workers


# decision: MEM-428, MEM-429
def test_a_join_leaves_the_cached_prefix_alone_until_a_fold_settles_it_and_a_forget_is_at_once(daemon: Any) -> None:
    """Owner 2026-11-04 ("lets add a newly added always on at the end"): a fact joining Tier 1 leaves the planner's
    cached prefix byte for byte as it was and shows after it, before the new message; a chat's running-summary fold
    (which changes the prefix anyway) settles it into the block; forgetting a newly joined fact takes it out at once
    without settling anything."""
    d = daemon.daemon
    sv = d.supervisor
    owner_fact(daemon, "style", "keep replies short", kind="preference")
    run(daemon, lambda: d.profile.settle("test"))
    script = install(daemon, [sup("reply", "Ok.")] * 4, {"summary": json.dumps({"summary": "EARLIER"})})

    def planner() -> Any:
        chat(daemon, "hello", session="cache")
        return [req for _, key, req in script.requests if key == "supervisor"][-1]
    first = planner()
    owner_fact(daemon, "tone", "never use emojis", kind="preference")
    joined = planner()
    assert joined.system == first.system                                     # the instructions: byte for byte
    assert joined.parts[:first.cacheable] == first.parts[:first.cacheable]
    live = "\n".join(joined.parts[joined.cacheable:])
    assert "never use emojis" in live and "(your words" in live
    assert live.index("never use emojis") < live.index("owner's complete new message")

    async def fold() -> None:
        entries = sv._timeline("", "cache", sv._summary("cache"))[0]
        await sv._fold("cache", None, entries[:2])
    daemon.run(fold())
    folded = planner()
    assert folded.system != first.system and "never use emojis" in folded.system
    assert "never use emojis" not in "\n".join(folded.parts)
    late = owner_fact(daemon, "music", "no music while I work", kind="preference")
    assert "no music while I work" in "\n".join(planner().parts)
    run(daemon, lambda: d.memory.forget(late.id))
    daemon.run(d._settle_forgets())
    after = planner()
    assert "no music while I work" not in after.system + "\n".join(after.parts)
    assert after.system == folded.system                                     # nothing settled for it


# decision: MEM-5, MEM-17
def test_the_profile_keeps_its_hard_budget(daemon: Any) -> None:
    d = daemon.daemon
    profile = CoreProfile(d.memory, d.clock, budget_tokens=200)

    async def fill() -> tuple[str, int, dict[str, Any]]:
        for i in range(40):
            src = d._journal().append("memory.added", {"content": f"rule {i}"})
            it = d.memory.propose("preference", "owner", f"rule_{i}", f"Always do thing number {i} the careful way",
                                  "owner_stated", [("journal", src.id)], 1.0)
            profile.add(it.id, "Preferences")
        profile.settle("test")                       # into the block, whose render keeps the budget too
        text, left_out = profile.render()
        return text, left_out, profile.view()
    text, left_out, view = daemon.run(fill())
    assert len(text) <= 200 * 4 and left_out > 0 and "left out to stay within its budget" in text
    assert view["used_tokens"] > view["budget_tokens"] and view["left_out"] == left_out
    assert run(daemon, profile.due)                                  # over budget: the pass is due sooner
    assert PROFILE_TOKENS == 2500


# decision: MEM-423, MEM-424
def test_a_preference_the_owner_states_joins_at_once_and_understanding_waits_to_hold_up(daemon: Any) -> None:
    d = daemon.daemon
    mine = owner_fact(daemon, "summaries", "short summary lists, no jargon", kind="preference")
    src = journal(daemon, "chat.message", {"text": "I'm a beginner"})
    guess = run(daemon, lambda: d.memory.propose("fact", "owner", "piano", "The owner is a piano beginner",
                                                 "inferred", [("journal", src)]))
    assert mine.id in run(daemon, d.profile.ids) and guess.id not in run(daemon, d.profile.ids)
    assert not run(daemon, lambda: d.profile.holds_up(guess))
    daemon.call_soon(d.clock.advance, timedelta(days=8).total_seconds())
    assert run(daemon, lambda: d.profile.holds_up(d.memory.get(guess.id)))     # stood a weekly pass uncorrected


# decision: PRM-319, MEM-35, MEM-425
def test_only_the_owners_words_about_themselves_join_at_once_the_pass_places_the_rest(daemon: Any) -> None:
    """MEM-35 (T-5; live 2026-11-03: an owner fact about something else went always-on at once): the owner's words
    about the owner join at once; what they say about a project or a person waits for the weekly pass, which may place
    it; a fact the owner placed by search (PRM-319's M7/M8, journaled by the seed) never joins; FERB's understanding
    about someone else doesn't join at once."""
    from ferb.memory.profile import SEARCH_ONLY_KIND
    d = daemon.daemon
    mine = owner_fact(daemon, "city", "I live in Pune")
    project = owner_fact(daemon, "thesis_deadline", "my thesis is due on Dec 12", subject="project:thesis")
    person = owner_fact(daemon, "sam_role", "Sam is my landlord", kind="person", subject="person:sam")
    run(daemon, lambda: d._journal().append(SEARCH_ONLY_KIND, {"keys": [["project:acme", "acme_groups"]]}))
    placed = owner_fact(daemon, "acme_groups", "check all group names containing ABC", subject="project:acme")
    src = journal(daemon, "chat.message", {"text": "Sam fixed the sink"})
    guess = run(daemon, lambda: d.memory.propose("person", "person:sam", "sam_handy", "Sam is handy", "inferred",
                                                 [("journal", src)]))
    ids = run(daemon, d.profile.ids)
    assert mine.id in ids
    assert project.id not in ids and person.id not in ids and placed.id not in ids and guess.id not in ids
    assert run(daemon, lambda: d.profile.holds_up(project))           # eligible: the pass may place it
    d._make_runtime = lambda *a, **k: FakeDistillRuntime({"sections": [
        {"title": "Projects", "items": [project.id, placed.id]}, {"title": "About you", "items": [mine.id]}],
        "ended": []})
    result = daemon.run(d._profile_pass(force=True))
    ids = run(daemon, d.profile.ids)
    assert project.id in ids and mine.id in ids and placed.id not in ids and result["refused"] == [placed.id]


# decision: MEM-7
def test_forget_and_retire_take_an_entry_out_at_once(daemon: Any) -> None:
    d = daemon.daemon
    a = owner_fact(daemon, "a", "always use dark mode", kind="preference")
    b = owner_fact(daemon, "b", "never call before 10am", kind="preference")
    run(daemon, lambda: d.memory.retire(a.id, "the owner said so"))
    run(daemon, lambda: d.memory.forget(b.id))
    assert run(daemon, d.profile.ids) == set()
    assert "dark mode" not in profile_text(daemon) and "10am" not in profile_text(daemon)


# decision: MEM-7, MEM-14, MEM-20
def test_the_weekly_pass_groups_what_holds_up_and_retires_what_is_over(daemon: Any) -> None:
    """MEM-7: a model chooses and groups; code checks every id (one that doesn't hold up is refused), renders
    from the facts, retires what the pass found over and tells the owner once."""
    d = daemon.daemon
    tone = owner_fact(daemon, "tone", "write formally to professors", kind="preference")
    exam = owner_fact(daemon, "exam", "the DevOps capstone deadline is on the 16th", kind="project",
                      subject="project:devops")
    src = journal(daemon, "chat.message", {"text": "x"})
    fresh = run(daemon, lambda: d.memory.propose("fact", "owner", "guess", "FERB thinks the owner likes jazz",
                                                 "inferred", [("journal", src)]))
    answer = {"sections": [{"title": "How you like things done", "items": [tone.id, fresh.id]},
                           {"title": "Current projects", "items": [exam.id]}],
              "ended": [{"id": exam.id, "why": "the 16th has passed"}]}
    rt = FakeDistillRuntime(answer)
    d._make_runtime = lambda *a, **k: rt
    out = daemon.run(d._profile_pass(force=True))
    assert out["refused"] == [fresh.id] and out["ended"] == [exam.id]
    view = run(daemon, d.profile.view)
    assert [s["title"] for s in view["sections"]] == ["How you like things done"]
    assert [i["id"] for i in view["sections"][0]["items"]] == [tone.id]
    assert run(daemon, lambda: d.memory.get(exam.id)).set_aside      # MEM-14: the owner's words set aside
    told = [e.data["text"] for e in run(daemon, lambda: d._journal().of_kind("chat.notice"))]
    assert len(told) == 1 and "DevOps capstone" in told[0]
    assert tone.id in str(rt.contexts[0]) and "write formally to professors" in str(rt.contexts[0])
    assert not run(daemon, d.profile.due)                                     # a week until the next


# decision: MEM-12
def test_the_memory_page_reads_and_edits_the_profile(daemon: Any) -> None:
    from ferb.cli.client import Client
    item = owner_fact(daemon, "city", "I live in Pune")
    c = Client(daemon.sock)
    try:
        view = c.call("memory.profile", {"op": "add", "id": item.id, "section": "Who you are"})
        assert "## Who you are" in view["markdown"] and "I live in Pune" in view["markdown"]
        assert [i["label"] for s in view["sections"] for i in s["items"]] == ["your words"]
        listed = c.call("memory.list", {})["items"]
        assert [(i["label"], i["in_profile"]) for i in listed] == [("your words", True)]
        view = c.call("memory.profile", {"op": "remove", "id": item.id})
        assert view["sections"] == [] and c.call("memory.list", {})["items"][0]["in_profile"] is False
    finally:
        c.close()


# decision: MEM-10
def test_an_entry_shows_the_owners_words_their_label_and_its_id() -> None:
    from datetime import datetime, timezone

    from ferb.core.types import MemoryItem
    now = datetime(2026, 10, 31, tzinfo=timezone.utc)
    item = MemoryItem("m1", "preference", "owner", "rating", "ok. score this 1 star - not ordering this again", [],
                      now, now, None, None, 1.0, "owner_stated", "active", about="Rate the Simplee order on Orderly")
    line = entry_line(item)
    assert "ok. score this 1 star - not ordering this again" in line           # word for word (MEM-10)
    assert "your words" in line and "Rate the Simplee order on Orderly" in line and "m1" in line
