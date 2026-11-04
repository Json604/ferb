"""Step 9b (MEM-5, 8, 17, 18 and the fix list): one fused search by meaning and keywords over
facts, past tasks with their how-notes, playbooks and old chat, each role within fixed limits; chat.history by meaning
and date from the database; no fact copies in learning_nodes; playbooks and the repeat measure; bounded card sources;
stale facts split by source; "Now always on"; and the cache guarantee. Nothing downloads: the embedding model is
`FakeEmbedder`."""
import io
import json
from datetime import timedelta
from typing import Any

from ferb.memory.learning import how_note, search_text, shown_text
from ferb.memory.profile import CoreProfile
from ferb.orchestrator import episodes
from tests.conftest import FakeDistillRuntime, chat, done, install, sup, task, tool
from tests.support.fake_embedder import FakeEmbedder
from tests.support.memory_items import journal, owner_fact, run


def put_card(d: Any, task_id: str, goal: str, *, how: str = "", verified: bool = True,
             actions: list[dict[str, Any]] | None = None, **extra: Any) -> dict[str, Any]:
    body = {"task_id": task_id, "intent": goal, "request": goal.lower(), "status": "done", "verified": verified,
            "narrative": "Done.", "how": how, "ended_at": "2026-10-20T10:00:00+00:00", "actions": actions or [],
            "apps": {}, "slots": {}} | extra
    return dict(run(d, lambda: d.daemon.learning.put("episode", task_id, body, [], origin_label="ferb",
                                                     made_by="code:evidence-card/4", state="active")))


def said(d: Any, text: str, session: str, request: str, reply: str = "") -> str:
    jid = journal(d, "chat.message", {"session_id": session, "request_id": request, "text": text,
                                      "untrusted_parts": 0})
    if reply:
        journal(d, "chat.reply", {"session_id": session, "request_id": request, "text": reply})
    return jid


def understood(d: Any, key: str, content: str, subject: str) -> Any:
    """FERB's understanding of a fact (not yet holding up): found by search, never in the always-on profile (the owner's
    own words would join it at once, MEM-15)."""
    src = journal(d, "chat.message", {"text": content})
    return run(d, lambda: d.daemon.memory.propose("fact", subject, key, content, "inferred", [("journal", src)]))


def planner_context(script: Any) -> str:
    return [ctx for _, key, ctx, _ in script.contexts if key == "supervisor"][-1]


# ---------- the fused search (MEM-6) ----------

# decision: MEM-6, ORCH-515, MEM-26
def test_one_search_finds_facts_past_tasks_and_old_chat_by_meaning(daemon: Any) -> None:
    """None of these share a word with the query; the local model's nearest find them (keywords alone don't)."""
    d = daemon.daemon
    understood(daemon, "music", "I am learning piano", subject="hobby:music")
    put_card(daemon, "t-mail", "Send outreach emails to recruiters", how="Used Gmail drafts, one per recruiter")
    said(daemon, "which keyboard should I buy for sonatas", "s-old", "r-old", reply="A 61-key keyboard is fine.")
    d.learning.embedder = FakeEmbedder()
    out = daemon.run(d._embed_now())
    assert out is not None and out["embedded"] >= 4                      # the fact, the card, the message, the reply
    found = [c["text"] for c in daemon.run(d._learning_candidates("melody chords for a newbie"))]
    assert any("I am learning piano" in t for t in found)
    mail = [c["text"] for c in daemon.run(d._learning_candidates("my mailing routine"))]
    assert any("Send outreach emails to recruiters" in t for t in mail)
    old = chat_found(daemon, "melody practice")                          # chat.history's search
    assert "which keyboard should I buy for sonatas" in old and "A 61-key keyboard is fine." in old
    d.learning.embedder = None                                           # keywords alone: none of them
    assert not [c for c in daemon.run(d._learning_candidates("melody chords for a newbie")) if "piano" in c["text"]]
    assert chat_found(daemon, "melody practice") == []


def chat_found(daemon: Any, words: str) -> list[str]:
    """The conversation entries the fused search finds for `words` (what chat.history's query ranks), their texts."""
    d = daemon.daemon

    async def go() -> list[str]:
        vector, _ = await d._query_vector(words)
        return [str(hit.data["text"]) for hit in d._memory_search().chat(words, vector=vector,
                                                                          kinds=("chat.message", "chat.reply"))]
    return daemon.run(go())                  # type: ignore[no-any-return]


# decision: ORCH-515
def test_the_planner_is_pushed_no_old_exchanges_and_finds_them_with_chat_history(daemon: Any) -> None:
    """MEM-16's push of matching old exchanges from other chats is gone (ORCH-515); the planner finds older entries
    with chat.history, which searches its chat by meaning and keywords."""
    said(daemon, "what sonatas suit a beginner", "s1", "r-old", reply="Try Clementi's Sonatina in C.")
    said(daemon, "what sonatas suit a beginner", "s-other", "r-other", reply="Try Kuhlau's Sonatina.")
    found: list[dict[str, Any]] = []

    def look(token: str) -> str:
        found.append(tool(daemon.sock, token, "chat.history", query="easy sonatas for a beginner"))
        return sup("reply", "Earlier I suggested Clementi's Sonatina in C.")
    script = install(daemon, [look])
    chat(daemon, "which sonatas did you suggest before?")
    ctx = planner_context(script)
    assert "Kuhlau" not in ctx and "earlier conversation matching" not in ctx     # another chat's: not pushed
    shown = json.dumps(found[0]["content"])
    assert not found[0]["is_error"] and "what sonatas suit a beginner" in shown and "r-other" not in shown


# decision: ORCH-515
def test_chat_history_finds_by_meaning_and_date_and_pages_from_the_database(daemon: Any) -> None:
    d = daemon.daemon
    d.learning.embedder = FakeEmbedder()
    start = run(daemon, d.clock.now)
    for n in range(6):
        said(daemon, f"note number {n} about lunch", "hist", f"r{n}", reply=f"Noted {n}.")
        daemon.call_soon(d.clock.advance, timedelta(days=1).total_seconds())
    said(daemon, "which keyboard should I buy", "hist", "r-kb")
    said(daemon, "a keyboard for the other chat", "elsewhere", "r-x")
    daemon.run(d._embed_now())                                           # the background embedder's pass
    assert not hasattr(d.supervisor.continuity, "history")              # no whole-journal read any more
    got: dict[str, Any] = {}

    def worker(token: str) -> str:
        got["meaning"] = tool(daemon.sock, token, "chat.history", query="melody")["content"]
        got["page"] = tool(daemon.sock, token, "chat.history", limit=3)["content"]
        day = (start + timedelta(days=2)).astimezone().date().isoformat()
        got["day"] = tool(daemon.sock, token, "chat.history", since=day, until=day)["content"]
        return done("Read it.")
    install(daemon, [sup("task", "Looking.", task())], {"worker": worker})
    chat(daemon, "what did I ask about music gear?", session="hist")
    meaning = json.loads(got["meaning"]["text"]) if "text" in got["meaning"] else got["meaning"]
    previews = [r["preview"] for r in meaning["records"]]
    assert any("which keyboard should I buy" in p for p in previews)
    assert not any("other chat" in p for p in previews)                  # another conversation's entry never shows
    page = got["page"]
    assert len(page["records"]) == 3 and page["next_after"] == 3 and page["total_records"] > 3
    assert {r["preview"] for r in got["day"]["records"]} == {json.dumps(x, ensure_ascii=False) for x in (
        {"session_id": "hist", "request_id": "r2", "text": "note number 2 about lunch", "untrusted_parts": 0},
        {"session_id": "hist", "request_id": "r2", "text": "Noted 2."})}


# decision: MEM-34
def test_facts_are_searched_in_memory_items_with_no_copies_in_learning_nodes(daemon: Any) -> None:
    """Fix list: `sync_facts` copied every memory item into learning_nodes on every search; no copy is made now, and
    the copies an older build left close once (journaled, idempotent)."""
    from ferb.memory.upgrade import close_fact_nodes
    d = daemon.daemon
    item = understood(daemon, "dentist", "my dentist is Dr. Rao", subject="person:dr_rao")
    found: list[dict[str, Any]] = []

    def look(token: str) -> str:                          # the planner looks it up (ORCH-515)
        found.append(tool(daemon.sock, token, "memory.read", query="my dentist"))
        return sup("reply", "Dr. Rao.")
    install(daemon, [look])
    chat(daemon, "who is my dentist?")
    assert "my dentist is Dr. Rao" in json.dumps(found[0]["content"])
    assert daemon.run(d._memory_item_search("dentist")) == [item.id]
    count = run(daemon, lambda: d.db.conn.execute(
        "SELECT COUNT(*) FROM learning_nodes WHERE kind IN ('fact', 'identity')").fetchone()[0])
    assert count == 0
    assert run(daemon, lambda: d.journal.of_kind("memory.fact_nodes_closed"))           # the upgrade ran at start


# decision: MEM-34
def test_an_older_builds_fact_copies_close_once(tmp_path: Any) -> None:
    from ferb.memory.upgrade import close_fact_nodes
    from tests.support.learning_fixtures import new_store
    db, learning = new_store(tmp_path / "m.db")
    learning.put("fact", "owner/old", {"content": "an old copy"}, [], made_by="code:fact-index/1", state="active")
    told: list[Any] = []

    def record(kind: str, data: dict[str, Any]) -> None:
        told.append((kind, data))
        db.conn.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, 'now', ?, ?, 'ferb')",
                        (f"j{len(told)}", kind, json.dumps(data)))
    assert close_fact_nodes(db, "2026-10-31T00:00:00+00:00", record) == 1
    assert close_fact_nodes(db, "2026-10-31T00:00:00+00:00", record) is None and len(told) == 1
    assert db.conn.execute("SELECT valid_to FROM learning_nodes WHERE kind = 'fact'").fetchone()[0] is not None
    db.close()


# ---------- limits (MEM-5) ----------

# decision: MEM-5, ORCH-515
def test_the_planners_context_doesnt_grow_with_the_store(daemon: Any) -> None:
    """Coordinator 2026-10-31 (MEM-5): 10,000 facts, hundreds of past tasks and a long chat history give the planner
    the same bounded blocks as a small store."""
    d = daemon.daemon

    def planner_turn(text: str) -> str:
        script = install(daemon, [sup("reply", "Ok.")])
        chat(daemon, text, session="limits")
        return planner_context(script)
    owner_fact(daemon, "garden", "the tomato garden needs water at dawn", subject="project:garden")
    small = planner_turn("how is my tomato garden doing")

    def topic(n: int) -> str:                    # one in twenty is about the garden: its words still carry weight
        return "tomato garden" if n % 20 == 0 else f"errand{n % 400} chore{n % 37}"

    def fill() -> None:
        now = d.clock.now().isoformat()
        rows = [(f"bulk{n}", "fact", "project:home", f"tip_{n}", f"note {n}: the {topic(n)} needs care at dawn",
                 "owner_stated", "active", 1.0, now, now, now) for n in range(10_000)]
        d.db.conn.execute("BEGIN")
        d.db.conn.executemany("INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, "
                              "observed_at, valid_from, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)", rows)
        d.db.conn.execute("COMMIT")
        for n in range(300):
            d.learning.put("episode", f"bulk-task{n}", {"task_id": f"bulk-task{n}", "intent": f"Look after the {topic(n)} {n}",
                                                        "status": "done", "verified": True, "narrative": "Watered.",
                                                        "actions": [], "apps": {}, "slots": {}},
                           [], origin_label="ferb", made_by="code:evidence-card/4", state="active")
        for n in range(400):
            d._journal().append("chat.message", {"session_id": "old-chat", "request_id": f"o{n}",
                                                 "text": f"how is the {topic(n)} {n}", "untrusted_parts": 0})
            d._journal().append("chat.reply", {"session_id": "old-chat", "request_id": f"o{n}",
                                               "text": f"The {topic(n)} is fine {n}. " + "x" * 400})
            d.daily_activity.record({"id": f"a{n}", "source": "window", "what": "window",
                                     "data": {"title": f"{topic(n)} planner {n} " + "y" * 200}})
    run(daemon, fill)
    big = planner_turn("how is my tomato garden doing")
    # ORCH-515: nothing of memory, old chat or activity is pushed, so the store's size never reaches the prompt; the
    # turn's own conversation grew by one exchange
    assert "Layer 4 —" not in big and "Layer 3 —" not in big and "tomato garden needs water" not in big
    assert len(big) < len(small) + 1000


# ---------- how-notes, playbooks and the repeat measure ----------

# decision: MEM-1, MEM-31
def test_a_how_note_is_built_from_the_recorded_calls_the_worker_and_the_owner(daemon: Any) -> None:
    actions = [{"tool": "browser.navigate", "args": {"url": "https://www.bookslots.com/en-US/login?x=1"}},
               {"tool": "browser.snapshot", "args": {}}, {"tool": "browser.snapshot", "args": {}},
               {"tool": "browser.act", "args": {"element": "s1e2"}, "error": "stale ref"},
               {"tool": "browser.act", "args": {"element": "s2e4"}, "result": {"ok": True}}]
    body = {"how": "Hard refresh first, then the login form works.", "actions": actions, "verified": True,
            "owner_hints": ["hard refresh usually fixes it"], "intent": "Log in to the permit site", "task_id": "t1"}
    note = how_note(body)
    assert note.startswith("Hard refresh first")
    assert "“hard refresh usually fixes it”" in note
    assert "browser.navigate(www.bookslots.com/en-US/login) → browser.snapshot ×2 → browser.act" in note
    assert "(1 call failed)" in note and "x=1" not in note
    assert "Recorded steps" in search_text("episode", "t1", body)
    shown = shown_text({"kind": "episode", "key": "t1", "body": body | {"status": "done"}, "recorded_at": "2026-10-30"})
    assert "how it was done: Hard refresh first" in shown
    d = daemon.daemon
    jid = said(daemon, "hard refresh usually fixes it", "s1", "r9")
    journal(daemon, "task.steered", {"task_id": "t-permit", "session_id": "s1", "request_id": "r9",
                                     "source_ref": "entry:" + jid})
    hints, refs = run(daemon, lambda: d.learning._owner_hints("t-permit"))
    assert hints == ["hard refresh usually fixes it"] and refs == {"journal:" + jid}


def record_run(d: Any, task_id: str, goal: str, attempts: int, seconds: float, day: int) -> dict[str, Any]:
    steps = [{"at": "t", "role": "Worker", "seconds": 1, "text": "", "mark": "✓"}] * attempts
    run(d, lambda: episodes.write(d.daemon.db, {
        "task_id": task_id, "request_id": None, "request_text": goal, "goal": goal, "status": "done",
        "verified": True, "started_at": f"2026-10-{day:02d}T10:00:00+00:00", "ended_at": f"2026-10-{day:02d}T10:05:00+00:00",
        "seconds": seconds, "answer": "Booked.", "conditions": [], "steps": steps, "problems": [], "tools": {},
        "origins": ["ferb"], "usage": {}, "desktop": {}, "made_by": "code:episode/1", "built_at": "now"}))
    return put_card(d, task_id, goal, how="Open the OFC page, pick the first slot",
                    ended_at=f"2026-10-{day:02d}T10:05:00+00:00")


# decision: MEM-29
def test_verified_runs_of_one_kind_merge_into_a_playbook_and_the_repeat_measure_shows_the_change(daemon: Any) -> None:
    from ferb.cli.admin import stats
    from ferb.cli.client import Client
    from ferb.memory.playbooks import candidates, merge
    d = daemon.daemon
    d.learning.embedder = FakeEmbedder()
    record_run(daemon, "t-permit-1", "Book the permit appointment at the consulate", attempts=3, seconds=600, day=10)
    record_run(daemon, "t-permit-2", "Book a permit appointment for biometrics", attempts=1, seconds=120, day=20)
    record_run(daemon, "t-food", "Order dinner on orderly", attempts=1, seconds=60, day=21)
    answer = {"same_kind": True, "members": ["t-permit-1", "t-permit-2", "t-food"], "title": "Book a permit slot",
              "text": "Open the OFC page; pick the first slot; confirm."}
    rt = FakeDistillRuntime(answer)
    record = lambda kind, data: d._journal().append(kind, data)    # noqa: E731
    written = daemon.run(merge(d.learning, lambda *a, **k: rt, d.learning.embedder, record=record))
    daemon.run(d._embed_now())
    assert len(written) == 1 and len(rt.contexts) == 1
    assert "t-permit-1" in str(rt.contexts[0]) and "t-food" not in str(rt.contexts[0])   # code chose who is shown
    book = run(daemon, lambda: d.learning.get(written[0], False))
    assert book["kind"] == "playbook" and book["body"]["tasks"] == ["t-permit-1", "t-permit-2"]   # never t-food
    assert run(daemon, lambda: candidates(d.learning, d.learning.embedder)) == []             # merged: not asked again
    found = [c["text"] for c in daemon.run(d._learning_candidates("permit appointment booking"))]
    assert any("playbook “Book a permit slot”" in t for t in found)
    c = Client(daemon.sock)
    try:
        [measured] = c.call("memory.repeats", {})["playbooks"]
    finally:
        c.close()
    assert [r["attempts"] for r in measured["runs"]] == [3, 1]
    assert measured["change"] == {"attempts": -2, "seconds": -480.0}
    out = io.StringIO()
    stats(d.home, out)
    assert "Repeated work (playbooks): 1" in out.getvalue() and "first 3 attempt(s), 600 s" in out.getvalue()


# decision: MEM-29
def test_a_group_the_model_says_is_not_one_kind_is_not_asked_again(daemon: Any) -> None:
    from ferb.memory.playbooks import candidates, merge
    d = daemon.daemon
    d.learning.embedder = FakeEmbedder()
    record_run(daemon, "t-a", "Book the permit appointment at the consulate", attempts=1, seconds=60, day=10)
    record_run(daemon, "t-b", "Cancel a permit appointment", attempts=1, seconds=60, day=11)
    rt = FakeDistillRuntime({"same_kind": False, "members": [], "title": "", "text": ""})
    record = lambda kind, data: d._journal().append(kind, data)    # noqa: E731
    assert daemon.run(merge(d.learning, lambda *a, **k: rt, d.learning.embedder, record=record)) == []
    assert run(daemon, lambda: candidates(d.learning, d.learning.embedder)) == []


# decision: MEM-30
def test_a_card_keeps_one_source_per_kind_of_origin_not_every_row_a_tool_read(tmp_path: Any) -> None:
    """Fix list: 1.5M learning_sources rows, up to 58,250 per card, from refs inside tool results. The labels are
    computed from every ref; the card keeps its journal and memory refs and one ref per distinct set of origins."""
    from tests.support.learning_fixtures import new_store
    db, learning = new_store(tmp_path / "m.db")
    for n in range(500):
        db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES "
                        "(?,?,?,?,?,?,?,?)", (f"r{n}", "2026-10-21T00:00:00+00:00", "2026-10-21", "dev", "owner", None,
                                              "app_focus" if n % 2 else "browser_tab", "{}"))
    jid = db.conn.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES ('j1', '2026-10-21', "
                          "'chat.message', '{}', 'owner') RETURNING id").fetchone()[0]
    refs = {f"trail:r{n}" for n in range(500)} | {"journal:" + jid}
    labels, damaged, expired, forgotten, kept = learning._card_labels(refs)
    assert "journal:j1" in kept and len(kept) == 3                       # the journal ref and one per origin set
    assert labels >= learning.labels(["trail:r0", "trail:r1"]) and not damaged and not forgotten
    db.close()


# ---------- MEM-14 and MEM-15 ----------

def notices(d: Any) -> list[dict[str, Any]]:
    return [e.data for e in run(d, lambda: d.daemon._journal().of_kind("chat.notice"))]


# decision: MEM-14, MEM-20, MEM-32
def test_stale_facts_are_split_by_source(daemon: Any) -> None:
    """MEM-14: FERB's understanding that looks over is retired and told once; the owner's own words are only set
    aside (not used, kept), told once, and come back when the owner says them again or restores them."""
    from ferb.cli.client import Client
    d = daemon.daemon
    trip = owner_fact(daemon, "trip", "I'm in Goa this week")
    src = journal(daemon, "chat.message", {"text": "x"})
    guess = run(daemon, lambda: d.memory.propose("fact", "owner", "exam", "The owner has an exam this week",
                                                 "inferred", [("journal", src)]))
    daemon.call_soon(d.clock.advance, timedelta(days=8).total_seconds())
    rt = FakeDistillRuntime({"sections": [], "ended": [{"id": trip.id, "why": "the week is over"},
                                                       {"id": guess.id, "why": "the week is over"}]})
    d._make_runtime = lambda *a, **k: rt
    daemon.run(d._profile_pass(force=True))
    kept, gone = run(daemon, lambda: d.memory.get(trip.id)), run(daemon, lambda: d.memory.get(guess.id))
    assert kept.set_aside and kept.valid_to is None and gone.valid_to is not None and not gone.set_aside
    texts = [n["text"] for n in notices(daemon)]
    assert any("I'm in Goa this week" in t and "stop using" in t and "kept" in t for t in texts)
    assert any("The owner has an exam" in t and "look out of date" in t for t in texts)
    assert daemon.run(d._memory_item_search("Goa week")) == []                 # not used while set aside
    again = journal(daemon, "memory.added", {"content": "I'm in Goa this week"})
    run(daemon, lambda: d.memory.save("fact", "owner", "trip", "I'm in Goa this week", "owner_stated",
                                      [("journal", again)], 1.0))
    assert not run(daemon, lambda: d.memory.get(trip.id)).set_aside             # saying it again: it holds
    run(daemon, lambda: d.memory.looks_over(trip.id, "test"))
    c = Client(daemon.sock)
    try:
        assert c.call("memory.restore", {"id": trip.id})["restored"] is True
    finally:
        c.close()
    assert daemon.run(d._memory_item_search("Goa week")) == [trip.id]


# decision: MEM-423, MEM-424, MEM-425
def test_understanding_that_holds_up_joins_with_one_quiet_now_always_on_card(daemon: Any) -> None:
    """MEM-15: FERB's understanding joins the core profile once it holds up (here, seen on a second day), with one
    "Now always on" note card (the 8e note card, marked always_on), never a question; the owner's words join at
    once (any kind, about the owner)."""
    d = daemon.daemon
    mine = owner_fact(daemon, "city", "I live in Pune")                   # a fact, not a preference: joins at once
    assert mine.id in run(daemon, d.profile.ids)
    first = journal(daemon, "chat.message", {"text": "beginner piano"})
    guess = run(daemon, lambda: d.memory.propose("fact", "owner", "piano", "The owner is a piano beginner",
                                                 "inferred", [("journal", first)]))
    assert guess.id not in run(daemon, d.profile.ids) and notices(daemon) == []
    daemon.call_soon(d.clock.advance, timedelta(days=1, hours=2).total_seconds())
    second = journal(daemon, "chat.message", {"text": "still a piano beginner"})
    run(daemon, lambda: d.memory.propose("fact", "owner", "piano", "The owner is a piano beginner", "inferred",
                                         [("journal", second)]))
    daemon.run(d._settle_forgets())
    assert guess.id in run(daemon, d.profile.ids)
    [told] = [n for n in notices(daemon) if "Now always on" in n["text"]]
    assert told["text"] == "Now always on: “The owner is a piano beginner” (FERB's understanding)"
    [card] = told["cards"]
    assert card["type"] == "note" and card["item_id"] == guess.id and card["always_on"] is True
    assert run(daemon, lambda: d.state.approvals.pending()) == []


# decision: MEM-7, MEM-423
def test_the_profile_and_so_the_cached_prefix_change_only_when_a_profile_entry_changes(daemon: Any) -> None:
    """MEM-15's guarantee: the planner's instructions (the cached prefix) are identical across turns unless a
    profile entry changed; saving a fact outside the profile changes nothing; nothing but the weekly pass rewrites it on
    a schedule (the hourly jobs leave it as it is)."""
    d = daemon.daemon
    d.learning.embedder = FakeEmbedder()
    owner_fact(daemon, "style", "keep replies short", kind="preference")
    script = install(daemon, [sup("reply", "Ok.")] * 4)

    def system() -> str:
        chat(daemon, "hello", session="cache")
        return str([req.system for _, key, req in script.requests if key == "supervisor"][-1])
    first = system()
    src = journal(daemon, "chat.message", {"text": "x"})
    run(daemon, lambda: d.memory.propose("fact", "owner", "guess", "The owner may like jazz", "inferred",
                                         [("journal", src)]))
    understood(daemon, "shop", "the shop is on MG Road", subject="place:shop")   # not holding up yet: not in it
    daemon.run(d._refresh_cards())
    daemon.run(d._merge_playbooks())
    daemon.run(d._embed_now())
    assert system() == first
    owner_fact(daemon, "tone", "never use emojis", kind="preference")       # a profile entry: the prefix changes
    changed = system()
    assert changed != first and "never use emojis" in changed
    assert system() == changed


# decision: ORCH-325
def test_a_browser_connecting_or_disconnecting_leaves_the_cached_prefix_alone(daemon: Any) -> None:
    """ORCH-325: what's usable (`{capabilities}` in the instructions) holds only stable facts, the
    paired profiles; whether one is connected right now is a live fact after the conversation."""
    from types import SimpleNamespace
    from ferb.core.types import Identity
    d = daemon.daemon
    binding = SimpleNamespace(identity=Identity.OWNER, browser="chrome", folder="Profile 7", install_id="i7")
    bridge = SimpleNamespace(conn=None)

    def wire() -> None:
        d.pairings = SimpleNamespace(all=lambda: [binding], for_profile=lambda browser, folder: binding
                                     if (browser, folder) == ("chrome", "Profile 7") else None)
        d.hub = SimpleNamespace(bridges={("chrome", "Profile 7"): bridge}, unpaired={}, refused={},
                                problem=lambda binding: None)
        d._fresh_profiles = lambda: [SimpleNamespace(browser="chrome", folder="Profile 7", display_name="Work")]
        d.supervisor.availability = d._availability
    run(daemon, wire)
    script = install(daemon, [sup("reply", "Ok.")] * 3)

    def turn() -> tuple[str, str]:
        chat(daemon, "hello", session="browsers")
        return (str([req.system for _, key, req in script.requests if key == "supervisor"][-1]),
                planner_context(script))
    idle_system, idle_context = turn()
    assert "Work" in idle_system and "“Work”: paired, connected" not in idle_context
    bridge.conn = object()                                               # the profile connects
    live_system, live_context = turn()
    assert live_system == idle_system and "“Work”: paired, connected" in live_context
    bridge.conn = None                                                   # and disconnects
    assert turn()[0] == idle_system


# decision: MEM-17, MEM-20
def test_a_burst_of_owner_facts_stays_within_budget_with_at_most_one_early_pass_a_day(daemon: Any) -> None:
    """Coordinator 2026-10-31: owner facts that don't fit make the pass due early, at most once a day; the profile
    stays within its budget, nothing is lost from the fact store, and retrieval finds what was left out."""
    d = daemon.daemon
    profile = CoreProfile(d.memory, d.clock, record=lambda kind, data: d._journal().append(kind, data),
                          budget_tokens=300)
    rt = FakeDistillRuntime({"sections": [], "ended": []})
    run(daemon, lambda: profile.apply({"sections": [], "ended": []}))        # a pass ran today
    d.memory.on_saved = profile.saved
    for n in range(40):
        owner_fact(daemon, f"rule_{n}", f"house rule number {n}: shoes off at the door, always")
    text, left_out = run(daemon, profile.render)
    assert len(text) <= 300 * 4 and left_out == 0
    assert len(run(daemon, lambda: d.memory.items("active"))) == 40         # nothing lost
    assert not run(daemon, profile.due)                                      # a pass already ran today
    daemon.call_soon(d.clock.advance, timedelta(hours=25).total_seconds())
    assert run(daemon, profile.due)                                          # due early, once the day has passed
    daemon.run(profile.run_pass(lambda *a, **k: rt))
    owner_fact(daemon, "rule_late", "house rule late: lights off at night")
    assert not run(daemon, profile.due)                                      # at most one early pass a day
    outside = [i.id for i in run(daemon, lambda: d.memory.items("active")) if i.id not in run(daemon, profile.ids)]
    assert outside and daemon.run(d._memory_item_search("house rule number 39"))[:1] == \
        [next(i.id for i in run(daemon, lambda: d.memory.items("active")) if i.key == "rule_39")]
