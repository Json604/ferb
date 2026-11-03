"""L36 §3: one ranked search over content; step 9b: facts searched in memory_items directly, each role within its
limits (MEM-5)."""
from pathlib import Path
from typing import Any

import pytest

from ferb.memory.learning import digest, search_text
from ferb.memory.search import ATTENTION, WORKER, MemorySearch
from ferb.orchestrator.context_layers import target_words
from tests.support.learning_fixtures import new_store
from tests.support.memory_corpus import USERNAME, fact, goal_like_episodes, owner_like_store
from tests.support.memory_items import owner_fact, run, understood
from tests.support.permit_audit import MSG_1217, MSG_1222, PERMIT_GOAL, PERMIT_TARGETS, RESEARCH_OFFICIAL

# The Learning.search tests below rank nodes against the pre-9b budgets they were measured with (60 / 20 nodes); the
# roles' limits now are search.ATTENTION / search.WORKER (MEM-27)
NODE_PLANNER_LIMIT, NODE_WORKER_LIMIT = 60, 20
PLANNER_QUERIES = ["log in to bookslots.com",
                   "Check bookslots.com in Chrome Home_Personal for document check and interview availability",
                   "bookslots permit document check interview booking fields Home_Personal owner-account"]


@pytest.fixture
def store(tmp_path: Path) -> Any:
    db, learning = new_store(tmp_path / "memory.db")
    owner_like_store(learning)
    yield learning
    db.close()


@pytest.fixture
def busy_store(tmp_path: Path) -> Any:
    """The owner-like store plus long past tasks that share a long goal's ordinary words (`goal_like_episodes`)."""
    db, learning = new_store(tmp_path / "memory.db")
    owner_like_store(learning)
    goal_like_episodes(learning)
    yield learning
    db.close()


def ranked(store: Any, query: str, node_id: str, limit: int, **kw: Any) -> bool:
    return node_id in [n["id"] for n in store.search(query, limit=limit, **kw)]


# decision: MEM-416
def test_the_owners_permit_messages_recall_the_username_through_the_task_they_continue(store: Any) -> None:
    """The audit's reproduction as a fixture: the fact sat at rank 167-201 of 207 unranked lines. The owner's 12:17 and
    12:22 messages share no word with it; the running permit task they continue names the site (Ruling 12a), so the
    planner's query (message, then that task's goal and targets) recalls it within the 60."""
    username = fact(store, "bookslots_username", USERNAME)
    continued = PERMIT_GOAL + "\n" + target_words(PERMIT_TARGETS)
    for message in (MSG_1217, MSG_1222):
        assert ranked(store, message + "\n" + continued, username["id"], NODE_PLANNER_LIMIT), message[:20]
        assert not ranked(store, message, username["id"], NODE_PLANNER_LIMIT), message[:20]   # alone: no shared word
    for query in PLANNER_QUERIES:
        assert ranked(store, query, username["id"], NODE_PLANNER_LIMIT), query


# decision: MEM-416
def test_the_research_official_step_recalls_the_username_within_its_20(store: Any) -> None:
    """The step that typed the owner's email (12:34): its query is the request, the parent goal, its step goal and the
    task's targets (Ruling 12)."""
    username = fact(store, "bookslots_username", USERNAME)
    query = "\n".join((MSG_1217, PERMIT_GOAL, RESEARCH_OFFICIAL, target_words(PERMIT_TARGETS)))
    assert ranked(store, query, username["id"], NODE_WORKER_LIMIT)


# decision: MEM-416
def test_long_past_tasks_bury_the_username_unless_the_tasks_targets_are_ranked_alone(busy_store: Any) -> None:
    """Fix round (measured on a copy of the owner's store, 2026-10-24): long past tasks share the permit goal's ordinary
    words, so the research step's query ranked the username 94th and the 12:17 planner query 91st. The task's target
    words ranked alone (`focus`) and fused with the main ranking bring it inside the 20 and the 60."""
    username = fact(busy_store, "bookslots_username", USERNAME)
    targets = target_words(PERMIT_TARGETS)
    step = f"{MSG_1217}\n{PERMIT_GOAL}\n{RESEARCH_OFFICIAL}\n{targets}"
    planner = f"{MSG_1217}\n{PERMIT_GOAL}\n{targets}"
    assert not ranked(busy_store, step, username["id"], NODE_WORKER_LIMIT)
    assert not ranked(busy_store, planner, username["id"], NODE_PLANNER_LIMIT)
    assert ranked(busy_store, step, username["id"], NODE_WORKER_LIMIT, focus=targets)
    assert ranked(busy_store, planner, username["id"], NODE_PLANNER_LIMIT, focus=targets)


# decision: MEM-6
def test_common_words_find_nothing_with_an_embedding_model_too(store: Any) -> None:
    """m5: similarity alone would rank every node; a query whose words all carry no weight finds nothing."""
    from tests.support.fake_embedder import FakeEmbedder
    model = FakeEmbedder()
    MemorySearch(store.db, store.clock).embed_pending(model)
    store.embedder = model
    assert store.search("is the for and owner") == []
    assert store.search("contest leaderboard") != []


# decision: MEM-6
def test_embeddings_embed_what_search_matches_and_never_a_rollup(store: Any) -> None:
    """Step 9b: the background embedder embeds each node's search text (what keyword search matches too), once; a node
    kind search no longer serves (an old rollup) is never embedded."""
    from tests.support.fake_embedder import FakeEmbedder
    rollup = store.put("summary", "day:2026-10-01", {"period": "day", "text": "task001", "children": [], "count": 0},
                       [], made_by="code:rollup/1", state="active")
    model = FakeEmbedder()
    MemorySearch(store.db, store.clock).embed_pending(model)
    hashes = dict(store.db.conn.execute("SELECT doc, text_hash FROM recall_vectors").fetchall())
    episode = next(n for n in store.items("episode"))
    assert hashes[episode["id"]] == embeddings_hash(search_text("episode", episode["key"], episode["body"]))
    assert rollup["id"] not in hashes
    calls = model.calls
    MemorySearch(store.db, store.clock).embed_pending(model)
    assert model.calls == calls                                # nothing new: nothing encoded again


def embeddings_hash(text: str) -> str:
    from ferb.memory.embeddings import text_hash
    return text_hash(text)


# decision: MEM-5, MEM-27
def test_a_role_gets_at_most_its_limit_best_first(daemon: Any) -> None:
    """MEM-5 (step 9b): retrieved memory is within the role's limits, best first, never protected; what is left out
    is one counted line."""
    for i in range(80):
        understood(daemon, f"tomato_{i}", f"tomato tip {i}: water them in the morning", subject="project:garden")
    for i in range(100):
        understood(daemon, f"bicycle_{i}", f"bicycle note {i}: oil the chain", subject="project:garage")
    found = daemon.run(daemon.daemon._learning_candidates("how should I grow tomato plants"))
    *items, more = found
    assert len(items) <= ATTENTION.memory_items and all("tomato" in c["text"] for c in items)
    assert sum(len(c["text"]) for c in items) <= ATTENTION.memory_tokens * 4
    assert "more matching memories left out" in more["text"]
    worker = daemon.run(daemon.daemon._learning_candidates("how should I grow tomato plants", WORKER))
    assert len(worker) - 1 <= WORKER.memory_items


# decision: MEM-5
def test_a_profile_entry_never_takes_a_slot_of_the_ranked_limit(daemon: Any) -> None:
    """Final review M1, MEM-15: the owner's preference "reply to permit emails in a formal tone" is in every role's
    core profile, so the ranked lists leave it out; the memory tools still find it."""
    d = daemon.daemon
    for i in range(4):
        owner_fact(daemon, f"chain_{i}", f"bicycle chain note {i}")
    tone = owner_fact(daemon, "permit_email_tone", "reply to permit emails in a formal tone", kind="preference")
    other = understood(daemon, "permit_email_signature", "permit email replies are signed Sam",
                       subject="project:permit")
    assert "reply to permit emails in a formal tone" in run(daemon, d._core_profile_text)     # Tier 1, every role
    worker = [c["text"] for c in daemon.run(d._learning_candidates("draft the permit email reply", WORKER))]
    assert any("signed Sam" in t for t in worker) and not any("formal tone" in t for t in worker)
    found = daemon.run(d._memory_item_search("draft the permit email reply"))
    assert tone.id in found and other.id in found                         # memory.read still finds both


# decision: MEM-6
def test_memory_read_ranks_a_several_word_query(daemon: Any) -> None:
    """Live 2026-10-24: 14 of 20 memory.read queries came back empty; 9 were phrases like "ORBIT shapes Contest rank
    score", which a literal substring match can never find."""
    from ferb.orchestrator.evidence import MemoryRead
    for i in range(4):
        owner_fact(daemon, f"chain_{i}", f"bicycle chain note {i}")
    orbit = owner_fact(daemon, "orbit", "I track my ORBIT shapes competition rank on Contest")
    reader = MemoryRead(lambda: daemon.daemon.memory, search=daemon.daemon._memory_item_search)
    out = daemon.run(reader.run(MemoryRead.Input(query="ORBIT shapes Contest rank score"), None))
    assert [i["id"] for i in out.content["items"]][:1] == [orbit.id]


# decision: MEM-34
def test_memory_read_without_a_query_shows_an_ended_fact_as_ended(daemon: Any) -> None:
    """Final review M2 and the step-9a fix list: after "book anywhere" replaced "Jaipur, notify first" a worker paging
    memory.read saw both as active. The no-query listing now shows current items only (a dated one until its end);
    an ended one is read by its id, with its real status."""
    from datetime import timedelta

    from ferb.orchestrator.evidence import MemoryRead
    d = daemon.daemon
    jaipur = owner_fact(daemon, "booking", "book in Jaipur, notify me first")
    anywhere = owner_fact(daemon, "booking", "book anywhere")
    gone = owner_fact(daemon, "dentist", "my dentist is Dr. Rao")
    run(daemon, lambda: d.memory.retire(gone.id, "the owner said so"))
    dated = owner_fact(daemon, "trip", "I'm in Pune this week")
    end = run(daemon, lambda: d.clock.now()) + timedelta(days=3)
    run(daemon, lambda: d.memory.expire_at(dated.id, end))
    reader = MemoryRead(lambda: d.memory, search=d._memory_item_search)
    listed = {i["id"]: (i["status"], i["valid_to"] is not None)
              for i in daemon.run(reader.run(MemoryRead.Input(), None)).content["items"]}
    assert listed == {anywhere.id: ("active", False), dated.id: ("active", True)}   # until its end date it holds
    by_id = daemon.run(reader.run(MemoryRead.Input(item_id=jaipur.id), None))
    assert not by_id.is_error and "notify me first" in str(by_id.content)


# decision: MEM-34
def test_memory_read_query_leaves_out_a_retired_item(daemon: Any) -> None:
    from ferb.orchestrator.evidence import MemoryRead
    for i in range(4):
        owner_fact(daemon, f"chain_{i}", f"bicycle chain note {i}")
    gone = owner_fact(daemon, "dentist", "my dentist is Dr. Rao")
    reader = MemoryRead(lambda: daemon.daemon.memory, search=daemon.daemon._memory_item_search)
    assert [i["id"] for i in daemon.run(reader.run(MemoryRead.Input(query="dentist"), None)).content["items"]] == [gone.id]
    run(daemon, lambda: daemon.daemon.memory.retract(gone.id, "outdated"))
    assert daemon.run(reader.run(MemoryRead.Input(query="dentist"), None)).content["items"] == []
