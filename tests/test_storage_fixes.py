"""Step 12a's confirmed storage bugs (notes/storage.md): a continued tree's episode reads its last run; episodes keep
the outside-content labels of what the planner was shown; non-ASCII evidence is found whether it was written before
(JSON escapes) or after (UTF-8); a forget's scrub finds a told text in both forms."""
from pathlib import Path

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.orchestrator import episodes
from ferb.store.db import Database
from ferb.store.spans import SpanStore


def _store(tmp_path: Path) -> tuple[Database, SpanStore, FakeClock]:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock = FakeClock()
    return db, SpanStore(db, clock, SequentialIds("s"), lambda value: value), clock


def _task_row(db: Database, root: str, at: str) -> None:
    db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
                    "updated_at) VALUES (?, ?, 'r1', 'tea', 'owner', 'worker', '[]', 'done', ?, ?)", (root, root, at, at))


def test_a_continued_trees_episode_has_its_last_runs_result_and_end(tmp_path: Path) -> None:
    db, spans, clock = _store(tmp_path)
    _task_row(db, "t1", clock.now().isoformat())
    first = spans.start("task", task_id="t1", input={"request": "order tea", "request_id": "r1"})
    spans.finish(first, output={"status": "failed", "verified": False})
    clock.advance(3600)                                  # continued an hour later: a second run of the same tree
    second = spans.start("task", task_id="t1", input={"request": "order tea, it's back in stock", "request_id": "r1"})
    clock.advance(60)
    spans.finish(second, output={"status": "done", "verified": True})
    ep = episodes.build(spans, "t1", clock.now().isoformat())
    assert ep["verified"] is True and ep["status"] == "done"
    assert ep["ended_at"] == clock.now().isoformat()                    # the last run's end, not the first's
    assert ep["request_text"]                                            # the request is still the first run's
    db.close()


def test_an_episode_keeps_the_origins_of_the_blocks_its_planner_was_shown(tmp_path: Path) -> None:
    db, spans, clock = _store(tmp_path)
    _task_row(db, "t1", clock.now().isoformat())
    root = spans.start("task", task_id="t1", input={"request": "summarize the page", "request_id": "r1"})
    spans.set_origin(root.id, "owner")
    shown = spans.start("context.exposure", task_id="t1", input={"source_refs": [], "reference": "Layer 3"})
    spans.finish(shown, output={"text": "a page"}, origin="external:web:x.com")
    spans.finish(root, output={"status": "done", "verified": True})
    ep = episodes.build(spans, "t1", clock.now().isoformat())
    assert "external:web:x.com" in ep["origins"]                         # outlives the spans with the episode
    db.close()


def test_non_ascii_evidence_is_found_as_written_now_and_as_older_rows_hold_it(tmp_path: Path) -> None:
    db, spans, clock = _store(tmp_path)
    new = spans.start("tool.call", task_id="t1", input={"tool": "browser.read", "args": {}})
    spans.finish(new, output={"text": "चाय का ऑर्डर"})
    assert "चाय" in db.conn.execute("SELECT output_json FROM spans WHERE id = ?", (new.id,)).fetchone()[0]
    db.conn.execute("INSERT INTO spans (id, task_id, kind, started_at, input_json, output_json) VALUES ('old', 't1', "
                    "'tool.call', ?, '{\"tool\": \"browser.read\"}', ?)",
                    (clock.now().isoformat(), '{"text": "\\u091a\\u093e\\u092f"}'))       # before step 12a: escaped
    found = {r["id"] for r in spans.tool_calls("t1", query="चाय")["records"]}
    assert found == {new.id, "old"}
    db.close()


def test_a_forget_scrub_finds_a_told_text_in_both_forms(tmp_path: Path) -> None:
    from ferb.orchestrator.attention import scrub_spans
    db, spans, clock = _store(tmp_path)
    told = '"text": "मिलते हैं"'
    new = spans.start("tool.call", task_id="t1", input={"tool": "chat.history", "args": {}})
    spans.finish(new, output={"context": f"{{{told}}}"})                # stored as UTF-8 now
    db.conn.execute("INSERT INTO spans (id, task_id, kind, started_at, input_json, output_json) VALUES ('old', 't1', "
                    "'tool.call', ?, '{}', ?)", (clock.now().isoformat(),
                                                 '{"context": "{\\"text\\": \\"\\u092e\\u093f\\u0932\\u0924\\u0947 '
                                                 '\\u0939\\u0948\\u0902\\"}"}'))
    scrub_spans(db, [(told, '"text": "[removed]"'), ('"text": "\\u092e\\u093f\\u0932\\u0924\\u0947 \\u0939\\u0948\\u0902"',
                                                     '"text": "[removed]"')], every_kind=True)
    for ident in (new.id, "old"):
        out = db.conn.execute("SELECT output_json FROM spans WHERE id = ?", (ident,)).fetchone()[0]
        assert "[removed]" in out and "मिलते" not in out and "\\u092e" not in out, (ident, out)
    db.close()
