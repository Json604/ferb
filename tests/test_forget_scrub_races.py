"""Fix wave 6a: a forget's scrub never loses a span's output or misses a span finishing during it.

The daemon runs a forget's echo scrub in a worker thread on a connection of its own (`echoes.finish_at`,
`finish_node_at`) while the event loop keeps writing spans (`SpanStore.finish`, `update_input`) on its own; the notice
purge (`attention.expire_derived`) runs on the loop's connection while a scrub may be running. Each scrub rewriter read
a span, worked out its new text in Python, and wrote both columns back by id, so:
1. lost output: a span finishing between the read and the write had its new output overwritten with the stale one;
2. missed text: a span finishing after the scrub read it (or listed its candidates) kept the forgotten text.
Each test drives one interleaving deterministically (`tests/support/interleave.py`): the live write lands right after
a chosen statement of the scrub, on the other connection."""
import json
from pathlib import Path
from typing import Any

from ferb.memory import echoes
from ferb.memory.echoes import REMOVED
from ferb.orchestrator.attention import AttentionNotices, expire_derived
from tests.support.interleave import Interleaved, TwoConnections, lists, reads

ITEM = "my dentist is Dr. Quillfeather"


def _forgotten() -> echoes.Forgotten:
    """A memory item task t1 was given (its exposures cited it)."""
    return echoes.Forgotten(contents=[ITEM], refs={"memory:m1"}, through={"t1": {"memory:m1"}})


def _forgotten_message() -> echoes.Forgotten:
    """A forgotten chat message (request r1): FERB's whole answer to it goes."""
    return echoes.Forgotten(said=["remember that my dentist is Dr. Quillfeather"], requests={"r1"})


# ---------- a memory forget (`echoes.finish`): the given task's spans ----------

def test_a_span_finishing_between_the_task_scrubs_read_and_write_keeps_its_output(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    span = two.spans.start("tool.call", task_id="t1",
                           input={"tool": "mail.send", "body": "Booked with Dr. Quillfeather"})
    Interleaved(two.scrub, reads(span.id), lambda: two.spans.finish(span, output={"text": "Sent 1 email"}))
    echoes.finish(two.scrub, _forgotten())
    inp, out, ended = two.held(span.id)
    assert out == {"text": "Sent 1 email"} and ended is not None        # its output stays
    assert inp == {"tool": "mail.send", "body": REMOVED}                 # and the item's line still goes
    two.close()


def test_a_task_span_finishing_after_the_scrub_read_it_loses_the_items_lines(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    span = two.spans.start("tool.call", task_id="t1", input={"tool": "calendar.search", "query": "appointments"})
    Interleaved(two.scrub, reads(span.id),
                lambda: two.spans.finish(span, output={"text": "Booked Quillfeather for Monday\nAll done"}))
    echoes.finish(two.scrub, _forgotten())
    assert two.held(span.id)[1] == {"text": f"{REMOVED}\nAll done"}       # a rare word of the item: the line goes
    two.close()


def test_the_planner_answer_to_a_forgotten_message_is_never_written_from_a_stale_read(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    turn = two.spans.start("model.turn", input={"role": "supervisor", "request_id": "r1"})
    two.spans.finish(turn, output={"text": "Noted.", "remember": []})

    def another_writer() -> None:                       # e.g. the loop's notice purge rewrote this turn meanwhile
        two.loop.conn.execute("UPDATE spans SET output_json = ? WHERE id = ?",
                              (json.dumps({"text": "Noted.", "remember": [], "notice": "expired"}), turn.id))
    Interleaved(two.scrub, reads("r1"), another_writer)
    echoes.finish(two.scrub, _forgotten_message())
    assert two.held(turn.id)[1] == {"text": REMOVED, "remember": [], "notice": "expired"}
    two.close()


def test_a_planner_turn_answering_a_forgotten_message_that_finishes_meanwhile_loses_its_answer(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    turn = two.spans.start("model.turn", input={"role": "supervisor", "request_id": "r1"})
    Interleaved(two.scrub, reads("r1"),
                lambda: two.spans.finish(turn, output={"text": "Okay, your dentist is Dr. Quillfeather."}))
    echoes.finish(two.scrub, _forgotten_message())
    assert two.held(turn.id)[1] == {"text": REMOVED}
    two.close()


# ---------- every span: the item's text as whole words (`echoes.scrub_told`) ----------

def test_a_span_finishing_between_the_told_scrubs_read_and_write_keeps_its_output(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    span = two.spans.start("tool.call", task_id="t2", input={"tool": "web.search", "query": ITEM})
    Interleaved(two.scrub, reads(span.id), lambda: two.spans.finish(span, output={"text": "3 results"}))
    echoes.finish(two.scrub, _forgotten())
    inp, out, _ = two.held(span.id)
    assert out == {"text": "3 results"} and inp == {"tool": "web.search", "query": REMOVED}
    two.close()


def test_a_span_finishing_with_the_item_while_the_told_scrub_runs_loses_it(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    holder = two.spans.start("tool.call", task_id="t2", input={"tool": "notes.read", "name": ITEM})
    two.spans.finish(holder, output={"text": "read"})
    running = two.spans.start("tool.call", task_id="t3", input={"tool": "notes.search", "query": "dentist"})
    Interleaved(two.scrub, reads(holder.id),
                lambda: two.spans.finish(running, output={"text": f"Found: {ITEM}"}))
    echoes.finish(two.scrub, _forgotten())
    assert two.held(running.id)[1] == {"text": f"Found: {REMOVED}"}
    two.close()


# ---------- a learned-item forget (`echoes.finish_node`): the spans that showed it ----------

NODE = "node-lesson-77"
LESSON = "always cc Zorblat on invoices"


def _node_echo() -> echoes.NodeEcho:
    return echoes.NodeEcho(naming=["the lesson about invoice copies"], versions={NODE: [LESSON]})


def test_a_span_finishing_between_the_node_scrubs_read_and_write_keeps_its_output(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    span = two.spans.start("model.turn", task_id="t4", input={"source_refs": [NODE], "context": f"Lesson: {LESSON}"})
    Interleaved(two.scrub, reads(span.id), lambda: two.spans.finish(span, output={"text": "Drafted."}))
    echoes.finish_node(two.scrub, _node_echo())
    inp, out, _ = two.held(span.id)
    assert out == {"text": "Drafted."} and inp["context"] == f"Lesson: {REMOVED}"
    two.close()


def test_a_span_that_showed_the_node_finishing_after_the_scrub_read_it_loses_its_text(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    span = two.spans.start("model.turn", task_id="t4", input={"source_refs": [NODE], "context": "a lesson applies"})
    Interleaved(two.scrub, reads(span.id), lambda: two.spans.finish(span, output={"text": f"Remember: {LESSON}"}))
    echoes.finish_node(two.scrub, _node_echo())
    assert two.held(span.id)[1] == {"text": f"Remember: {REMOVED}"}
    two.close()


# ---------- the notice purge (`attention.expire_derived`) ----------

NOTICE = "You copied the unicorn invoice"


def _notice(two: TwoConnections) -> Any:
    return AttentionNotices(two.loop, two.clock, lambda value: value).add(
        "batch:1", ["trail:copied"], NOTICE, [])


def _told(text: str) -> str:
    """A told notice as a later model context holds it: a JSON "text" field inside a recorded string."""
    return json.dumps({"text": text})


def _purge(two: TwoConnections) -> None:
    expire_derived(two.scrub, {"trail:copied"}, "expired", two.clock.now().isoformat())


def test_a_span_finishing_between_the_notice_scrubs_read_and_write_keeps_its_output(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    _notice(two)
    span = two.spans.start("model.turn", input={"role": "worker", "context": _told(NOTICE)})
    Interleaved(two.scrub, lists("expiring_text e"), lambda: two.spans.finish(span, output={"text": "ok"}))
    _purge(two)
    inp, out, _ = two.held(span.id)
    assert out == {"text": "ok"} and NOTICE not in inp["context"]
    two.close()


def test_a_span_finishing_with_a_told_notice_while_the_notice_scrub_runs_loses_it(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    _notice(two)
    holder = two.spans.start("model.turn", input={"role": "worker", "context": _told(NOTICE)})
    two.spans.finish(holder, output={"text": "ok"})
    running = two.spans.start("model.turn", input={"role": "worker", "context": "nothing told"})
    Interleaved(two.scrub, lists("expiring_text e"),
                lambda: two.spans.finish(running, output={"history": _told(NOTICE)}))
    _purge(two)
    assert NOTICE not in json.dumps(two.held(running.id)[1])
    two.close()


def _notice_entry(two: TwoConnections) -> str:
    """The notice's chat entry, as the planner's conversation exposure cites it."""
    from ferb.core.ids import SequentialIds
    from ferb.memory.journal import Journal
    made = _notice(two)
    data = {k: AttentionNotices.view(made)[k] for k in ("id", "text", "quotes")}
    return "entry:" + Journal(two.loop, two.clock, SequentialIds("j"), lambda value: value).append(
        "chat.notice", data, origin=made["origin"]).id


def test_an_exposure_payload_is_never_written_from_a_stale_read(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    entry = _notice_entry(two)
    shown = two.spans.start("context.exposure", input={"source_refs": [entry], "reference": "chat.history"})
    two.spans.finish(shown, output={"text": "display lines naming Zorblat", "payload": f"earlier\n\n{NOTICE}"})

    def another_writer() -> None:                       # e.g. a forget's scrub took a word out of it meanwhile
        two.loop.conn.execute("UPDATE spans SET output_json = ? WHERE id = ?", (json.dumps(
            {"text": f"display lines naming {REMOVED}", "payload": f"earlier\n\n{NOTICE}"}), shown.id))
    Interleaved(two.scrub, lists("expiring_entries e"), another_writer)
    _purge(two)
    assert two.held(shown.id)[1] == {"text": f"display lines naming {REMOVED}", "payload": "earlier\n\nexpired"}
    two.close()


def test_an_exposure_finishing_with_a_notice_payload_while_the_purge_runs_loses_it(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    entry = _notice_entry(two)
    shown = two.spans.start("context.exposure", input={"source_refs": [entry], "reference": "chat.history"})
    Interleaved(two.scrub, lists("expiring_entries e"),
                lambda: two.spans.finish(shown, output={"text": "display lines", "payload": f"earlier\n\n{NOTICE}"}))
    _purge(two)
    assert two.held(shown.id)[1] == {"text": "display lines", "payload": "earlier\n\nexpired"}
    two.close()


CONSIDERATIONS = "FROM spans WHERE kind='attention.consider'"


def test_a_consideration_finishing_between_the_purges_read_and_write_keeps_its_answer_cleared(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    _notice(two)
    check = two.spans.start("attention.consider", input={"source_refs": ["trail:copied"], "context": "rows"})
    Interleaved(two.scrub, lists(CONSIDERATIONS),
                lambda: two.spans.finish(check, output={"text": NOTICE, "quotes": [], "kind": "notice"}))
    _purge(two)
    inp, out, ended = two.held(check.id)
    assert ended is not None and out == {"text": "expired", "quotes": [], "kind": "notice"}
    assert inp["context"] != "rows"
    two.close()


def test_a_consideration_finishing_after_the_purge_listed_it_loses_its_answer(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    _notice(two)
    check = two.spans.start("attention.consider", input={"source_refs": ["trail:copied"]})
    Interleaved(two.scrub, lists(CONSIDERATIONS), lambda: two.spans.finish(check, output={"text": NOTICE}))
    _purge(two)
    assert two.held(check.id)[1] == {"text": "expired", "quotes": []}
    two.close()


# ---------- the other rows a scrub rewrites, and the loop's own read-and-write ----------

def test_a_task_outcome_written_between_the_scrubs_read_and_write_is_kept_and_scrubbed(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    now = two.clock.now().isoformat()
    two.loop.conn.execute(
        "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
        "outcome_json, created_at, updated_at) VALUES ('t1', 't1', NULL, 'r9', 'book the dentist', 'worker', "
        "'worker', '[]', 'running', ?, ?, ?)", (json.dumps({"answer": "Booking with Dr. Quillfeather"}), now, now))

    def task_finished() -> None:
        two.loop.conn.execute("UPDATE tasks SET status = 'done', outcome_json = ? WHERE id = 't1'", (json.dumps(
            {"answer": "Booked with Dr. Quillfeather", "verified": True}),))
    Interleaved(two.scrub, lists("SELECT id, goal, outcome_json FROM tasks"), task_finished)
    echoes.finish(two.scrub, _forgotten())
    outcome = two.loop.conn.execute("SELECT outcome_json FROM tasks WHERE id = 't1'").fetchone()[0]
    assert json.loads(outcome) == {"answer": REMOVED, "verified": True}
    two.close()


def test_a_notice_cleared_between_the_node_scrubs_read_and_write_stays_cleared(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    AttentionNotices(two.loop, two.clock, lambda value: value).add("batch:2", ["trail:t"], f"Tip: {LESSON}", [])

    def purged() -> None:                               # the loop's notice purge cleared it meanwhile
        two.loop.conn.execute("UPDATE attention_notices SET text = 'Source evidence was forgotten.'")
    Interleaved(two.scrub, lists("SELECT id, text FROM attention_notices"), purged)
    echoes.finish_node(two.scrub, echoes.NodeEcho(naming=["the lesson about invoice copies"],
                                                  versions={NODE: [LESSON]}, statements={NODE: LESSON}))
    assert [r[0] for r in two.loop.conn.execute("SELECT text FROM attention_notices")] == [
        "Source evidence was forgotten."]
    two.close()


def test_a_journal_row_deleted_between_the_scrubs_read_and_rewrite_is_never_written_back(tmp_path: Path) -> None:
    from ferb.core.ids import SequentialIds
    from ferb.memory.journal import Journal
    two = TwoConnections(tmp_path)
    entry = Journal(two.loop, two.clock, SequentialIds("j"), lambda value: value).append(
        "chat.notice", {"id": "batch:3", "text": "About the lesson about invoice copies", "quotes": []},
        origin="external:observation")

    def purged() -> None:                               # the loop's notice purge deletes a notice's chat entry
        two.loop.conn.execute("DELETE FROM journal WHERE id = ?", (entry.id,))
    Interleaved(two.scrub, lists("SELECT seq, id, at, kind, data_json, origin FROM journal"), purged)
    echoes.finish_node(two.scrub, _node_echo())
    assert two.loop.conn.execute("SELECT COUNT(*) FROM journal WHERE id = ?", (entry.id,)).fetchone()[0] == 0
    two.close()


def test_measured_input_added_while_a_scrub_rewrites_the_span_never_brings_the_text_back(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    span = two.spans.start("model.turn", task_id="t1", input={"role": "worker", "goal": ITEM})

    def scrub_wrote() -> None:                          # the scrub's own connection, between the loop's read and write
        two.scrub.conn.execute("UPDATE spans SET input_json = ? WHERE id = ?",
                               (json.dumps({"role": "worker", "goal": REMOVED}), span.id))
    Interleaved(two.loop, reads(span.id), scrub_wrote)
    two.spans.update_input(span.id, {"context_budget_tokens": 1000})
    assert two.held(span.id)[0] == {"role": "worker", "goal": REMOVED, "context_budget_tokens": 1000}
    two.close()


# ---------- racing nothing, a scrub behaves exactly as before: one pass, each changed span written once ----------

def _span_writes(seen: Interleaved) -> list[str]:
    return sorted(params[2] for sql, params in seen.statements if sql.lstrip().startswith("UPDATE spans"))


def test_a_memory_forget_racing_nothing_runs_one_pass(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    given = two.spans.start("tool.call", task_id="t1", input={"body": "Booked with Dr. Quillfeather\nthanks"})
    two.spans.finish(given, output={"text": "Booked Quillfeather\nok"})
    other = two.spans.start("tool.call", task_id="t2", input={"query": ITEM})
    two.spans.finish(other, output={"text": "none"})
    clean = two.spans.start("tool.call", task_id="t3", input={"query": "weather"})
    two.spans.finish(clean, output={"text": "sunny"})
    running = two.spans.start("tool.call", task_id="t3", input={"query": "weather tomorrow"})
    seen = Interleaved(two.scrub, lambda sql, params: False)
    echoes.finish(two.scrub, _forgotten())
    assert two.held(given.id)[:2] == ({"body": f"{REMOVED}\nthanks"}, {"text": f"{REMOVED}\nok"})
    assert two.held(other.id)[:2] == ({"query": REMOVED}, {"text": "none"})
    assert two.held(clean.id)[:2] == ({"query": "weather"}, {"text": "sunny"})
    assert two.held(running.id)[:2] == ({"query": "weather tomorrow"}, None)
    assert _span_writes(seen) == sorted([given.id, other.id])
    assert sum("told_needles n" in sql for sql, _ in seen.statements) == 1
    two.close()


def test_a_node_forget_racing_nothing_runs_one_pass(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    shown = two.spans.start("model.turn", task_id="t4", input={"source_refs": [NODE], "context": f"Lesson: {LESSON}"})
    two.spans.finish(shown, output={"text": "Drafted."})
    two.spans.start("model.turn", task_id="t5", input={"context": "unrelated"})
    seen = Interleaved(two.scrub, lambda sql, params: False)
    echoes.finish_node(two.scrub, _node_echo())
    assert two.held(shown.id)[:2] == ({"source_refs": [NODE], "context": f"Lesson: {REMOVED}"}, {"text": "Drafted."})
    assert _span_writes(seen) == [shown.id]
    assert sum("told_needles n" in sql for sql, _ in seen.statements) == 1
    two.close()


def test_a_notice_purge_racing_nothing_runs_one_pass(tmp_path: Path) -> None:
    two = TwoConnections(tmp_path)
    _notice(two)
    told = two.spans.start("model.turn", input={"role": "worker", "context": _told(NOTICE)})
    two.spans.finish(told, output={"text": "ok"})
    two.spans.start("attention.consider", input={"source_refs": ["trail:other"], "context": "rows"})
    seen = Interleaved(two.scrub, lambda sql, params: False)
    _purge(two)
    assert two.held(told.id)[:2] == ({"role": "worker", "context": _told("expired")}, {"text": "ok"})
    assert _span_writes(seen) == [told.id]
    assert sum("expiring_text e" in sql for sql, _ in seen.statements) == 1
    two.close()
