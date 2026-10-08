"""Raw activity and daily summaries are findable by content as well as by date (D9), Hindi words included; purged
or forgotten rows leave the index."""
from datetime import UTC, datetime
from typing import Any

from ferb.memory import summary
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
    """Summaries hold window titles and hosts (outside text); narratives are model-written from them."""
    assert _tool(daemon).read_sources == () and TrailDays.origin == "ferb"          # nothing read: FERB's own
    _append(daemon, summary.SUMMARY_KIND, {"day": "2026-10-06", "windows": ["Safari: x"], "sites": {}})
    assert _tool(daemon).read_sources == (("web", "trail"),)
    _append(daemon, summary.NARRATIVE_KIND, {"day": "2026-10-06", "text": "worked"})
    assert set(_tool(daemon).read_sources) == {("web", "trail"), ("origin", "inferred")}
    _append(daemon, summary.SUMMARY_KIND, {"day": "2026-10-09", "windows": [], "sites": {}, "apps_min": {"Notes": 5}})
    assert _tool(daemon, since_days=3650, limit=1).read_sources == (("app", "trail"),)   # only what it returned
