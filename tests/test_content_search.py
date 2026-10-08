"""Raw activity and daily summaries are findable by content as well as by date (D9), Hindi words included; purged
or forgotten rows leave the index."""
from datetime import UTC, date, datetime
from typing import Any

from ferb.memory import summary
from ferb.memory.items import query_terms
from ferb.sense.tools import TrailDays
from tests.conftest import RunningDaemon, trail_row


def _query(daemon: RunningDaemon, text: str) -> dict:
    async def go():
        return daemon.daemon.trail.query(since="2000-01-01", text=text)
    return daemon.run(go())


def _days(daemon: RunningDaemon, text: str | None = None, since_days: int = 3650) -> list[dict]:
    async def go():
        return daemon.daemon.trail.days(text=text, since_days=since_days)
    return daemon.run(go())


def _append(daemon: RunningDaemon, kind: str, data: dict) -> None:
    async def go():
        daemon.daemon.journal.append(kind, data)
    daemon.run(go())


def _tool(daemon: RunningDaemon, **args: Any):
    async def go():
        tool = TrailDays(lambda: daemon.daemon.trail)
        return await tool.run(TrailDays.Input(**args), None)  # type: ignore[arg-type]
    return daemon.run(go())


def test_trail_query_matches_text(daemon: RunningDaemon) -> None:
    trail_row(daemon, "2026-10-07", data={"host": "github.com", "title": "devops-heros pipeline run"})
    trail_row(daemon, "2026-10-07", data={"host": "youtube.com", "title": "finance interns"}, hour=11)
    out = _query(daemon, "pipeline")
    assert [e["data"]["title"] for e in out["events"]] == ["devops-heros pipeline run"]
    assert _query(daemon, "pipeline finance")["count"] == 0                    # every word must match


def test_hindi_words_are_found_in_activity_and_summaries(daemon: RunningDaemon) -> None:
    trail_row(daemon, "2026-10-07", data={"title": "दिल्ली मौसम"})
    _append(daemon, summary.NARRATIVE_KIND, {"day": "2026-10-07", "text": "आज दिल्ली में काम किया"})
    assert _query(daemon, "दिल्ली")["count"] == 1
    found = _days(daemon, "दिल्ली")
    assert [(d["day"], d["kind"]) for d in found] == [("2026-10-07", "narrative")]


def test_json_keys_do_not_match(daemon: RunningDaemon) -> None:
    trail_row(daemon, "2026-10-07", data={"title": "something"})
    assert _query(daemon, "title")["count"] == 0


def test_days_by_date_newest_first(daemon: RunningDaemon) -> None:
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 8, tzinfo=UTC))
    _append(daemon, summary.SUMMARY_KIND, {"day": "2026-10-01", "lines": []})
    _append(daemon, summary.SUMMARY_KIND, {"day": "2026-10-06", "lines": []})
    assert [d["day"] for d in _days(daemon, since_days=3)] == ["2026-10-06"]
    assert [d["day"] for d in _days(daemon, since_days=30)] == ["2026-10-06", "2026-10-01"]


def test_purged_rows_leave_the_index(daemon: RunningDaemon) -> None:
    trail_row(daemon, "2026-09-01", data={"title": "zebrafish notes"})
    _append(daemon, summary.NARRATIVE_KIND, {"day": "2026-09-01", "text": "zebrafish all day"})

    async def forget():
        daemon.daemon.db.conn.execute("DELETE FROM trail WHERE day = '2026-09-01'")
        daemon.daemon.db.conn.execute("DELETE FROM journal WHERE kind = ?", (summary.NARRATIVE_KIND,))
    daemon.run(forget())
    assert _query(daemon, "zebrafish")["count"] == 0 and _days(daemon, "zebrafish") == []


def test_trail_days_labels_what_it_returns(daemon: RunningDaemon) -> None:
    """One rule (summary.digest_origins), the same as the stored origin; labels cover only the rows returned."""
    assert _tool(daemon).read_sources == () and TrailDays.origin == "ferb"          # nothing read: FERB's own
    _append(daemon, summary.SUMMARY_KIND, {"day": "2026-10-06", "windows": ["Safari: x"], "sites": {}})
    assert _tool(daemon).read_sources == (("origin", "external:web:trail"),)
    _append(daemon, summary.NARRATIVE_KIND, {"day": "2026-10-06", "text": "worked"})
    assert set(_tool(daemon).read_sources) == {("origin", "external:web:trail"), ("origin", "inferred")}
    _append(daemon, summary.SUMMARY_KIND, {"day": "2026-10-09", "windows": [], "sites": {}, "apps_min": {"Notes": 5}})
    assert _tool(daemon, since_days=3650, limit=1).read_sources == (("origin", "private"),)
    stored = daemon.run(_rows(daemon, "SELECT origin FROM journal WHERE kind = ? ORDER BY seq", summary.SUMMARY_KIND))
    assert [r[0] for r in stored] == ["external:web:trail", "private"]


def _rows(daemon: RunningDaemon, sql: str, *args: Any):
    async def go():
        return daemon.daemon.db.conn.execute(sql, args).fetchall()
    return go()


def _digest_of(daemon: RunningDaemon, day: str) -> dict:
    async def go():
        return summary.build(daemon.daemon.db, day).digest
    return daemon.run(go())


def test_summary_index_holds_content_only(daemon: RunningDaemon) -> None:
    """Folder paths are digest KEYS and are found; fixed wording (made_by, duration) matches nothing."""
    trail_row(daemon, "2026-10-07", source="file", data={"path": "/Users/k/Documents/thesis/ch1.md", "change": "modified"})
    digest = _digest_of(daemon, "2026-10-07")
    assert "/Users/k/Documents/thesis" in digest["folders"]
    _append(daemon, summary.SUMMARY_KIND, digest | {"made_by": summary.SUMMARY_MADE_BY})
    assert [d["day"] for d in _days(daemon, "thesis")] == ["2026-10-07"]
    assert _days(daemon, "code") == [] and _days(daemon, "focus") == [] and _days(daemon, "intervals") == []
    assert _days(daemon, "2026") == [] and _days(daemon, "made_by") == []


def test_invalid_json_row_does_not_break_insert(daemon: RunningDaemon) -> None:
    async def go():
        c = daemon.daemon.db.conn
        c.execute("INSERT INTO trail (id, at, day, device_id, actor, source, data_json) "
                  "VALUES ('bad', '2026-10-07T10:00:00+00:00', '2026-10-07', 'dev', 'owner', 'file', 'not json')")
        c.execute("INSERT INTO journal (id, at, kind, data_json) VALUES ('badj', '2026-10-07T10:00:00+00:00', ?, '{oops')",
                  (summary.SUMMARY_KIND,))
    daemon.run(go())
    assert _query(daemon, "json")["count"] == 0


def test_update_path_reindexes(daemon: RunningDaemon) -> None:
    rid = trail_row(daemon, "2026-10-07", data={"title": "oldword page"})

    async def upd():
        daemon.daemon.db.conn.execute("UPDATE trail SET data_json = ? WHERE id = ?", ('{"title": "newword page"}', rid))
    daemon.run(upd())
    assert _query(daemon, "oldword")["count"] == 0 and _query(daemon, "newword")["count"] == 1


def test_query_terms_lowercase_like_the_tokenizer(daemon: RunningDaemon) -> None:
    trail_row(daemon, "2026-10-07", data={"title": "Straße Cafe"})
    assert query_terms("STRASSE") == ["strasse"] and query_terms("Straße") == ["straße"]
    assert _query(daemon, "Straße")["count"] == 1


def _fts_bytes(daemon: RunningDaemon) -> bytes:
    async def go():
        c = daemon.daemon.db.conn
        return b"".join(bytes(r[0]) for t in ("trail_fts_data", "summary_fts_data")
                        for r in c.execute(f"SELECT block FROM {t}"))
    return daemon.run(go())


def test_forgotten_words_leave_the_fts_storage(daemon: RunningDaemon) -> None:
    word = "quokkaphrase"
    for day in ("2026-08-01", "2026-08-02"):
        trail_row(daemon, day, data={"title": f"{word} notes"})
    _append(daemon, summary.NARRATIVE_KIND, {"day": "2026-08-01", "text": f"{word} all day"})
    assert word.encode() in _fts_bytes(daemon)

    async def go():
        daemon.daemon.trail.purge(30, date(2026, 10, 8))          # refuses unsummarized days: summarize one first
    _append(daemon, summary.SUMMARY_KIND, {"day": "2026-08-01"})
    daemon.run(go())
    assert _query(daemon, word)["count"] == 1                     # the unsummarized day's row is still there

    async def forget():
        c = daemon.daemon.db.conn
        c.execute("DELETE FROM trail WHERE day = '2026-08-02'")
        summary.rebuild(daemon.daemon.db, daemon.daemon.journal, "2026-08-02")
        c.execute("DELETE FROM journal WHERE kind = ?", (summary.NARRATIVE_KIND,))
    daemon.run(forget())
    assert _query(daemon, word)["count"] == 0 and word.encode() not in _fts_bytes(daemon)
