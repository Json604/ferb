"""L36 §3: one ranked search over content, with the owner's budgets ("Planner 60, worker 20")."""
import threading
from datetime import timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.memory import embeddings
from ferb.memory.learning import (PLANNER_MEMORY_ITEMS, RECENCY_WEIGHT, SEARCH_TEXT_VERSION, WORKER_MEMORY_ITEMS,
                                  digest, encoded,
                                  reindex_search, search_text, shown_text, slug_words)
from ferb.orchestrator.context_layers import target_words
from ferb.orchestrator.schemas import TargetsSpec
from tests.conftest import chat, install, sup, task
from tests.support.learning_fixtures import TickingClock, new_store
from tests.support.live_steer import StalledWorker
from tests.support.memory_corpus import USERNAME, fact, owner_like_store
from tests.support.memory_items import owner_fact, run

# The audit's real texts (scratchpad memaudit/repro.py and the live task 0a5fbe31, 2026-10-24); only the owner's email
# address is replaced (Ruling 25). The owner's 12:17 message created the task; 12:22 corrected it.
MSG_1217 = ("hmm idk if thats s agood strategy for this, cause slots can pen anytime. search online extensilvely for "
            "this and then propose what is a good strategy")
MSG_1222 = ("also, wherever its available, book it. dont just wait for chennai. first ofc then counselar. search what "
            "info you’d require at booking the slot stpe. if nothting is req from me then just booook it. dont wait for "
            "me. book and then tell me")
VISA_GOAL = ("Update the ongoing US B1/B2 appointment strategy work for your `owner@example.com` account in Chrome "
             "`New_Personal`: optimize for actually securing an appointment without paying an agent. Do not restrict "
             "the search to Chennai—look for the earliest available valid OFC appointment first and corresponding "
             "consular interview across any Indian locations the official system permits, with any date acceptable. "
             "Research exactly what information, documents, selections, or decisions the official booking flow "
             "requires before the final booking step. Use your securely stored login and security answers where "
             "applicable. If no new information or decision is required from you when a valid appointment pair "
             "appears, book it immediately without waiting for you, then report the exact OFC and consular locations, "
             "dates, and confirmation details. If the booking flow requires a genuinely missing personal choice or "
             "information, ask only for that. Also determine and implement a monitoring frequency that materially "
             "improves your chance of getting a slot while respecting official limits and avoiding account blocks, "
             "bots, paid agents, or unsafe extensions.")
VISA_TARGETS = TargetsSpec(apps=["Chrome New_Personal"], sites=["usvisascheduling.com"])
RESEARCH_OFFICIAL = ("Collect current official guidance on India B1/B2 appointment availability, scheduling rules, "
                     "rescheduling limits, account access, and automation or abuse restrictions from official US "
                     "government and scheduling sources.")
PLANNER_QUERIES = ["log in to usvisascheduling.com",
                   "Check usvisascheduling.com in Chrome New_Personal for Chennai OFC and interview availability",
                   "usvisascheduling B1 B2 India OFC consular booking fields New_Personal owner-account"]


@pytest.fixture
def store(tmp_path: Path) -> Any:
    db, learning = new_store(tmp_path / "memory.db")
    owner_like_store(learning)
    yield learning
    db.close()


def ranked(store: Any, query: str, node_id: str, limit: int) -> bool:
    return node_id in [n["id"] for n in store.search(query, limit=limit)]


def test_target_and_slug_words_are_the_items_and_tasks_own_words() -> None:
    assert target_words(TargetsSpec(sites=["https://www.usvisascheduling.com/en-US/login", "usvisascheduling.com"],
                                    apps=["Chrome New_Personal"], recipients=["Asha"],
                                    paths=["/Users/owner/Desktop/proofOfWork/CV/Resume_2026.pdf"])) == \
        "www usvisascheduling com usvisascheduling com Chrome New_Personal Asha Resume_2026.pdf"
    assert slug_words("usvisascheduling_username") == "usvisascheduling username"
    assert slug_words("project:us-b1b2-visa") == "us b1b2 visa"


def test_the_owners_visa_messages_recall_the_username_through_the_task_they_continue(store: Any) -> None:
    """The audit's reproduction as a fixture: the fact sat at rank 167-201 of 207 unranked lines. The owner's 12:17 and
    12:22 messages share no word with it; the running visa task they continue names the site (Ruling 12a), so the
    planner's query (message, then that task's goal and targets) recalls it within the 60."""
    username = fact(store, "usvisascheduling_username", USERNAME)
    continued = VISA_GOAL + "\n" + target_words(VISA_TARGETS)
    for message in (MSG_1217, MSG_1222):
        assert ranked(store, message + "\n" + continued, username["id"], PLANNER_MEMORY_ITEMS), message[:20]
        assert not ranked(store, message, username["id"], PLANNER_MEMORY_ITEMS), message[:20]   # alone: no shared word
    for query in PLANNER_QUERIES:
        assert ranked(store, query, username["id"], PLANNER_MEMORY_ITEMS), query


def test_the_research_official_step_recalls_the_username_within_its_20(store: Any) -> None:
    """The step that typed the owner's email (12:34): its query is the request, the parent goal, its step goal and the
    task's targets (Ruling 12)."""
    username = fact(store, "usvisascheduling_username", USERNAME)
    query = "\n".join((MSG_1217, VISA_GOAL, RESEARCH_OFFICIAL, target_words(VISA_TARGETS)))
    assert ranked(store, query, username["id"], WORKER_MEMORY_ITEMS)


def test_the_planner_recalls_for_the_work_its_message_continues(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """A planner turn's memory query adds the goals and targets of the tasks its task layers show: the owner's 12:17
    message, sent while the visa task runs in the same chat, brings the username into the planner's memory. The fillers
    hold every word the message shares with the username's fact ("is"), so only the task's words can recall it; the same
    message in a chat with no task (the negative control) leaves it out."""
    for i in range(6):
        owner_fact(daemon, f"hose_{i}", f"the garden hose {i} is a good thing for this and then what")
    owner_fact(daemon, "usvisascheduling_username", USERNAME)
    worker = StalledWorker(monkeypatch)
    script = install(daemon, [sup("task", "Looking.", task(VISA_GOAL, apps=("Chrome New_Personal",),
                                                           sites=("usvisascheduling.com",)))])
    first: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: first.extend(chat(daemon, "look for visa slots", session="visa")))
    thread.start()
    try:
        assert worker.started.wait(5)
        script.sup.append(sup("reply", "Here is a strategy."))
        chat(daemon, MSG_1217, session="visa", timeout=5)
        planner = [ctx for _, key, ctx, _ in script.contexts if key == "supervisor"][-1]
        assert USERNAME in planner
        script.sup.append(sup("reply", "Here is a strategy."))
        chat(daemon, MSG_1217, session="elsewhere", timeout=5)       # another chat: its planner sees no visa task
        planner = [ctx for _, key, ctx, _ in script.contexts if key == "supervisor"][-1]
        assert MSG_1217 in planner and USERNAME not in planner
    finally:
        worker.release.set()
        thread.join(5)


def test_words_most_items_share_find_nothing_instead_of_the_newest_items(store: Any) -> None:
    """Review Focus: never the newest 60 or 20 items for a query of common words."""
    assert store.search("is the for and owner") == []
    assert store.search("") != []                              # no words at all: a listing, newest first


class _Embedder:
    """A stand-in embedding model: every text gets the same direction, so similarity ranks every node."""
    name = "test-model"

    def encode(self, text: str, *, query: bool = False) -> list[int]:
        return [1, 2, 3]


def test_common_words_find_nothing_with_an_embedding_model_too(store: Any) -> None:
    """m5: similarity alone would rank every node; a query whose words all carry no weight finds nothing."""
    embeddings.index(store, _Embedder())
    store.embedder = _Embedder()
    assert store.search("is the for and owner") == []
    assert store.search("kaggle leaderboard") != []


def test_embeddings_embed_what_search_matches_and_never_a_rollup(store: Any) -> None:
    rollup = store.put("summary", "day:2026-10-01", {"period": "day", "text": "task001", "children": [], "count": 0},
                       [], made_by="code:rollup/1", state="active")
    store.db.conn.execute("INSERT INTO learning_vectors VALUES (?,?,?,?,?)",
                          (rollup["id"], _Embedder.name, b"\x01\x02\x03", "2026-10", "an older text"))
    embeddings.index(store, _Embedder())
    hashes = dict(store.db.conn.execute("SELECT node_id, text_hash FROM learning_vectors").fetchall())
    episode = next(n for n in store.items("episode"))
    assert hashes[episode["id"]] == digest(search_text("episode", episode["key"], episode["body"]))
    assert rollup["id"] not in hashes                          # nothing to match: its older vector goes too


def test_a_budgeted_search_reads_only_the_bodies_it_returns(store: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """On the owner's store a planner query matched 698 nodes; reading every body before the budget applied took 1.4 s
    a turn (review fix 5). Nodes are ranked and filtered by their columns first, then read best first until the budget
    is full, in the same order as reading them all."""
    everything = store.search("owner is kept in the gmail inbox and the kaggle leaderboard")
    read: list[str] = []
    get = type(store).get
    monkeypatch.setattr(type(store), "get", lambda self, ident, sources=True: (read.append(ident),
                                                                               get(self, ident, sources))[1])
    top = store.search("owner is kept in the gmail inbox and the kaggle leaderboard", limit=5)
    assert [n["id"] for n in top] == [n["id"] for n in everything[:5]] and len(everything) > 20
    assert len(read) == 5
    assert "sources" in top[0] and "sources" not in store.search("gmail inbox", limit=1, sources=False)[0]


def test_json_field_names_and_scaffolding_are_not_searched(store: Any) -> None:
    fact(store, "dentist", "my dentist is Dr. Rao")
    assert store.search("unconfirmed_hint observed_at confidence") == []
    assert search_text("fact", "owner/dentist", {"content": "my dentist is Dr. Rao", "subject": "owner",
                                                 "key": "dentist", "observed_at": "x"}) == \
        "my dentist is Dr. Rao owner dentist"


def test_an_episode_shows_its_goal_outcome_and_date_and_one_without_a_goal_is_left_out(store: Any) -> None:
    episode = next(n for n in store.search("kaggle leaderboard") if n["kind"] == "episode")
    line = shown_text(episode)
    assert line.startswith("past task task") and "Check the kaggle leaderboard for the owner" in line
    assert "(2026-10-" in line and ("— done, verified" in line or line.endswith("— done"))
    store.put("episode", "nogoal", {"task_id": "nogoal", "intent": "", "narrative": "zebra crossing found",
                                    "status": "done"}, [], origin_label="ferb", made_by="code:evidence-card/4",
              state="active")
    assert [n for n in store.search("zebra crossing") if n["key"] == "nogoal"] == []


def test_the_owners_words_rank_before_a_confirmed_guess_at_equal_relevance(tmp_path: Path) -> None:
    db, learning = new_store(tmp_path / "m.db")
    for i in range(4):
        fact(learning, f"other_{i}", f"bicycle chain note {i}")
    mine = fact(learning, "editor", "the editor is Zed")
    guess = fact(learning, "editor", "the editor is Zed", owner=False)     # can't supersede the owner's version
    assert [n["id"] for n in learning.search("which editor")][:2] == [mine["id"], guess["id"]]
    db.close()


def test_a_guess_recorded_after_the_owners_identical_fact_still_ranks_after_it(tmp_path: Path) -> None:
    """Recency never cancels owner-first: the owner's words don't age, so an owner-stated node takes the full recency
    share, and a guess written a second later scores no higher at equal relevance (review fix 1)."""
    db, learning = new_store(tmp_path / "m.db", clock=TickingClock())
    for i in range(4):
        fact(learning, f"other_{i}", f"bicycle chain note {i}")
    mine = fact(learning, "editor", "the editor is Zed")
    guess = fact(learning, "editor", "the editor is Zed", owner=False)
    assert guess["recorded_at"] > mine["recorded_at"]
    assert [n["id"] for n in learning.search("which editor")][:2] == [mine["id"], guess["id"]]
    db.close()


def test_recency_orders_equal_matches_newest_first_and_never_passes_a_match_over_0_05_better(tmp_path: Path) -> None:
    """At equal bm25 the newer node ranks first. Recency adds at most RECENCY_WEIGHT (0.05), so it never passes a
    match more than that much better; a match one rank worse is worth much more (1 against 1/2)."""
    clock = TickingClock()
    db, learning = new_store(tmp_path / "m.db", clock=clock)
    better = fact(learning, "chain", "the bicycle chain was oiled", owner=False)
    clock.at += timedelta(days=40)
    older = fact(learning, "note_a", "a kettle descaling note", owner=False)
    clock.at += timedelta(days=40)
    newer = fact(learning, "note_b", "a kettle descaling note", owner=False)
    weaker = fact(learning, "bell", "the bicycle bell rings", owner=False)
    clock.at += timedelta(days=1)
    assert [n["id"] for n in learning.search("kettle descaling")] == [newer["id"], older["id"]]
    found = learning.search("bicycle chain")
    assert [n["id"] for n in found] == [better["id"], weaker["id"]]
    assert 1 < found[0]["score"] <= 1 + RECENCY_WEIGHT and 0.5 < found[1]["score"] <= 0.5 + RECENCY_WEIGHT
    db.close()


def _indexed_by_json(db: Any, node: dict[str, Any]) -> None:
    """`node` indexed as the search before L36 indexed it: its key and its whole JSON body."""
    db.conn.execute("UPDATE learning_fts SET text = ? WHERE id = ?", (node["key"] + " " + encoded(node["body"]),
                                                                       node["id"]))


def _fts_text(db: Any, node: dict[str, Any]) -> str:
    return str(db.conn.execute("SELECT text FROM learning_fts WHERE id = ?", (node["id"],)).fetchone()[0])


def test_a_store_indexed_by_json_is_reindexed_once_by_content(tmp_path: Path) -> None:
    db, learning = new_store(tmp_path / "m.db")
    node = fact(learning, "dentist", "my dentist is Dr. Rao")
    _indexed_by_json(db, node)
    db.conn.execute("DELETE FROM learning_search_version")
    assert reindex_search(db) is True
    assert _fts_text(db, node) == "my dentist is Dr. Rao owner dentist"
    assert db.conn.execute("SELECT version FROM learning_search_version").fetchone()[0] == SEARCH_TEXT_VERSION
    assert reindex_search(db) is False
    assert db.conn.execute("SELECT COUNT(*) FROM learning_fts").fetchone()[0] == 1
    db.close()


def test_a_stopped_reindex_resumes_where_it_stopped(tmp_path: Path) -> None:
    """The owner's store is re-indexed in short batches, each its own transaction (never one long write lock): a
    re-index stopped between batches (a shutdown) keeps what it did, and the next start finishes the rest."""
    db, learning = new_store(tmp_path / "m.db")
    first, second = fact(learning, "dentist", "my dentist is Dr. Rao"), fact(learning, "editor", "the editor is Zed")
    for node in (first, second):
        _indexed_by_json(db, node)
    db.conn.execute("DELETE FROM learning_search_version")
    content = {first["id"]: "my dentist is Dr. Rao owner dentist", second["id"]: "the editor is Zed owner editor"}
    batches: list[int] = []
    assert reindex_search(db, batch_bytes=1, stop=lambda: batches.append(1) or len(batches) > 1) is True
    assert sorted(_fts_text(db, n) == content[n["id"]] for n in (first, second)) == [False, True]   # one batch done
    assert reindex_search(db, batch_bytes=1) is True
    assert all(_fts_text(db, n) == content[n["id"]] for n in (first, second))
    assert reindex_search(db) is False
    db.close()


def test_a_reindex_resumes_after_a_compaction_renumbers_the_nodes(tmp_path: Path) -> None:
    """The re-index resumes after the last node id it did, never an implicit rowid: a compaction (VACUUM) may
    renumber those, and a forget between runs leaves gaps for it to close (review fix 2)."""
    db, learning = new_store(tmp_path / "m.db")
    nodes = [fact(learning, f"note_{i}", f"kettle note number {i}") for i in range(6)]
    for node in nodes:
        _indexed_by_json(db, node)
    db.conn.execute("DELETE FROM learning_search_version")
    batches: list[int] = []
    assert reindex_search(db, batch_bytes=1, stop=lambda: batches.append(1) or len(batches) > 3) is True
    done = [n for n in nodes if not _fts_text(db, n).startswith("owner/")]
    assert len(done) == 3
    learning.forget([n["id"] for n in done])            # the re-indexed ones go, then the store is compacted
    db.compact()
    # VACUUM may renumber the rowids of a table without an INTEGER PRIMARY KEY (SQLite's documented right; this
    # build happens to keep them), so the test renumbers them as it may: the remaining nodes now hold the old ones'
    db.conn.execute("UPDATE learning_nodes SET rowid = rowid - 3")
    assert reindex_search(db, batch_bytes=1) is True
    assert all(_fts_text(db, n) == f"kettle note number {n['key'][-1]} owner note {n['key'][-1]}"
               for n in nodes if n not in done)
    assert reindex_search(db) is False
    db.close()


def test_an_empty_store_records_the_version_without_reindexing(tmp_path: Path) -> None:
    db, _ = new_store(tmp_path / "m.db")
    assert reindex_search(db) is False
    assert db.conn.execute("SELECT version FROM learning_search_version").fetchone()[0] == SEARCH_TEXT_VERSION
    db.close()


def test_the_daemon_journals_a_reindex_only_when_nodes_were_reindexed(daemon: Any) -> None:
    """m3: a fresh store (nothing to re-index) journals nothing; a store indexed by JSON journals the one re-index."""
    d = daemon.daemon
    daemon.run(d._search_ready())
    assert run(daemon, lambda: d.journal.of_kind("memory.search_reindexed")) == []
    owner_fact(daemon, "dentist", "my dentist is Dr. Rao")
    node = run(daemon, lambda: (d.learning.sync_facts(d.memory), d.learning.items("fact")[0])[1])
    run(daemon, lambda: (_indexed_by_json(d.db, node), d.db.conn.execute("DELETE FROM learning_search_version")))
    daemon.run(d._reindex_search())
    assert [e.data for e in run(daemon, lambda: d.journal.of_kind("memory.search_reindexed"))] == \
        [{"version": SEARCH_TEXT_VERSION, "finished": True}]
    assert run(daemon, lambda: _fts_text(d.db, node)) == "my dentist is Dr. Rao owner dentist"


def test_the_planner_gets_at_most_its_60_items(daemon: Any) -> None:
    for i in range(80):
        owner_fact(daemon, f"tomato_{i}", f"tomato tip {i}: water them in the morning")
    for i in range(100):
        owner_fact(daemon, f"bicycle_{i}", f"bicycle note {i}: oil the chain")
    found = daemon.run(daemon.daemon._learning_candidates("how should I grow tomato plants"))
    assert len(found) == PLANNER_MEMORY_ITEMS and all("tomato" in c["text"] for c in found)


def test_an_empty_query_lists_newest_first_with_no_recency_or_owner_boost(tmp_path: Path) -> None:
    clock = TickingClock()
    db, learning = new_store(tmp_path / "m.db", clock=clock)
    old_owner = fact(learning, "a", "old owner note")
    clock.at += timedelta(days=90)
    guess = fact(learning, "b", "newer guess", owner=False)
    clock.at += timedelta(days=1)
    newest = fact(learning, "c", "newest guess", owner=False)
    assert [n["id"] for n in learning.search("")] == [newest["id"], guess["id"], old_owner["id"]]
    assert [n["id"] for n in learning.search("", kind="")] == [newest["id"], guess["id"], old_owner["id"]]
    assert all(n["score"] <= 1 for n in learning.search(""))
    db.close()


def test_memory_read_ranks_a_several_word_query(daemon: Any) -> None:
    """Live 2026-10-24: 14 of 20 memory.read queries came back empty; 9 were phrases like "RSNA knee Kaggle rank
    score", which a literal substring match can never find."""
    from ferb.orchestrator.evidence import MemoryRead
    for i in range(4):
        owner_fact(daemon, f"chain_{i}", f"bicycle chain note {i}")
    rsna = owner_fact(daemon, "rsna", "I track my RSNA knee competition rank on Kaggle")
    reader = MemoryRead(lambda: daemon.daemon.memory, search=daemon.daemon._memory_item_search)
    out = daemon.run(reader.run(MemoryRead.Input(query="RSNA knee Kaggle rank score"), None))
    assert [i["id"] for i in out.content["items"]][:1] == [rsna.id]


def test_memory_read_without_its_search_refuses_a_query_rather_than_match_substrings(daemon: Any) -> None:
    from ferb.orchestrator.evidence import MemoryRead
    out = daemon.run(MemoryRead(lambda: daemon.daemon.memory).run(MemoryRead.Input(query="anything"), None))
    assert out.is_error and "search" in out.content["error"]


def test_memory_learned_returns_content_with_ids(daemon: Any) -> None:
    import json
    from ferb.memory.tools import LearningRead
    for i in range(4):
        owner_fact(daemon, f"chain_{i}", f"bicycle chain note {i}")
    owner_fact(daemon, "dentist", "my dentist is Dr. Rao")
    reader = LearningRead(lambda: daemon.daemon.learning, daemon.daemon._search)
    out = daemon.run(reader.run(LearningRead.Input(query="dentist"), None))
    [c] = [c for c in json.loads(out.content["text"])["candidates"] if c["kind"] == "fact"]
    assert c["text"] == "owner/dentist — my dentist is Dr. Rao" and c["id"].startswith("m-")


def test_the_memory_tools_are_registered_on_the_waiting_search(daemon: Any) -> None:
    """memory.learned must never search a half-re-indexed store: the daemon registers it on `_search`."""
    tool = daemon.daemon.registry.capabilities["memory.learned"]
    assert tool.search == daemon.daemon._search


def test_memory_read_refuses_a_query_with_no_words_and_finds_nothing_for_common_words(daemon: Any) -> None:
    from ferb.orchestrator.evidence import MemoryRead
    for i in range(4):
        owner_fact(daemon, f"note_{i}", f"the note {i} is for the owner")
    reader = MemoryRead(lambda: daemon.daemon.memory, search=daemon.daemon._memory_item_search)
    for query in ("???", "  ", "...!"):
        out = daemon.run(reader.run(MemoryRead.Input(query=query), None))
        assert out.is_error and "needs words" in out.content["error"]
    out = daemon.run(reader.run(MemoryRead.Input(query="is the for and owner"), None))
    assert not out.is_error and out.content["items"] == []


def test_memory_read_query_leaves_out_a_retired_item(daemon: Any) -> None:
    from ferb.orchestrator.evidence import MemoryRead
    for i in range(4):
        owner_fact(daemon, f"chain_{i}", f"bicycle chain note {i}")
    gone = owner_fact(daemon, "dentist", "my dentist is Dr. Rao")
    reader = MemoryRead(lambda: daemon.daemon.memory, search=daemon.daemon._memory_item_search)
    assert [i["id"] for i in daemon.run(reader.run(MemoryRead.Input(query="dentist"), None)).content["items"]] == [gone.id]
    run(daemon, lambda: daemon.daemon.memory.retract(gone.id, "outdated"))
    assert daemon.run(reader.run(MemoryRead.Input(query="dentist"), None)).content["items"] == []
