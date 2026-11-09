"""APP-805 (owner, 2026-11-09: "A. FERB's database only"): the chat as FERB.app shows it lives in FERB's database, one
row per entry in the app's own shape. FERB.app writes each entry as it changes (`chat.history.put`), sends its old saved
chat once (`chat.history.import`, any number of times without change), and reads a day a page at a time, newest first
(`chat.history.page`, 30 at a time, APP-806) and the days History lists (`chat.history.days`). FERB.app only (its code
signature is stood in for here)."""
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.orchestrator.attention import AttentionNotices, expire_derived
from ferb.store.db import Database
from tests.conftest import RunningDaemon

DAY = datetime(2026, 11, 8, 9, 0, tzinfo=timezone.utc)


def _as_app(daemon: RunningDaemon) -> Client:
    daemon.daemon.server.is_app = lambda token: token is not None      # FERB.app's signature, stood in for
    return Client(daemon.sock)


def _turn(i: int, day: str = "2026-11-08", **body: Any) -> dict[str, Any]:
    ident = str(uuid.uuid4()).upper()
    at = DAY + timedelta(minutes=i)
    return {"id": ident, "kind": "turn", "at": at.isoformat(), "day": day, "open": False,
            "body": {"id": ident, "createdAt": at.timestamp(), "request": f"message {i}", "answer": f"answer {i}",
                     "status": "done"} | body}


def _rows(daemon: RunningDaemon) -> list[Any]:
    async def go() -> list[Any]:
        return [dict(r) for r in daemon.daemon.db.conn.execute("SELECT * FROM chat_history ORDER BY at")]
    return list(daemon.run(go()))


# decision: DMN-555
def test_only_ferb_apps_own_process_may_read_or_write_the_chat(daemon: RunningDaemon) -> None:
    c = Client(daemon.sock)
    try:
        for method, params in (("chat.history.put", {"entries": [_turn(0)]}),
                               ("chat.history.import", {"entries": [_turn(1)]}),
                               ("chat.history.page", {"day": "2026-11-08"}), ("chat.history.days", {})):
            with pytest.raises(RpcError) as refused:
                c.call(method, params)
            assert "FERB.app only" in str(refused.value)
    finally:
        c.close()
    assert _rows(daemon) == []


# decision: APP-805, APP-806, DMN-556
def test_a_day_reads_newest_first_thirty_at_a_time_with_the_turns_its_replies_answer(daemon: RunningDaemon) -> None:
    turns = [_turn(i) for i in range(70)]
    asked = turns[5]
    reply_id = str(uuid.uuid4()).upper()
    reply = {"id": reply_id, "kind": "reply", "at": (DAY + timedelta(minutes=69, seconds=30)).isoformat(),
             "day": "2026-11-08", "body": {"id": reply_id, "turnID": asked["id"], "afterTurnID": turns[68]["id"],
                                           "part": "answer"}}
    other_day = _turn(0, day="2026-11-09")
    c = _as_app(daemon)
    try:
        assert c.call("chat.history.put", {"entries": turns[:40]})["held"] == 40
        c.call("chat.history.put", {"entries": turns[40:] + [reply, other_day]})
        first = c.call("chat.history.page", {"day": "2026-11-08"})
        assert len(first["entries"]) == 30 and first["more"]
        # oldest first within the page: the day's newest 30 (turn 69's reply was made after turn 69 itself)
        assert [e["id"] for e in first["entries"]] == [t["id"] for t in turns[41:70]] + [reply_id]
        assert first["entries"][-1]["body"]["turnID"] == asked["id"]
        assert [t["id"] for t in first["context"]] == [asked["id"]]          # its question, though on an earlier page
        second = c.call("chat.history.page", {"day": "2026-11-08", "before": first["cursor"]})
        assert [e["id"] for e in second["entries"]] == [t["id"] for t in turns[11:41]] and second["more"]
        third = c.call("chat.history.page", {"day": "2026-11-08", "before": second["cursor"], "limit": 30})
        assert [e["id"] for e in third["entries"]] == [t["id"] for t in turns[:11]]
        assert not third["more"] and third["cursor"] is None
        assert c.call("chat.history.days", {})["days"] == ["2026-11-09", "2026-11-08"]
        assert [e["id"] for e in c.call("chat.history.page", {"day": "2026-11-09"})["entries"]] == [other_day["id"]]
        assert c.call("chat.history.page", {"day": "2026-11-07"}) == {"entries": [], "more": False, "cursor": None,
                                                                    "context": []}
    finally:
        c.close()


# decision: DMN-555, DMN-556
def test_a_changed_entry_replaces_the_one_kept_and_waiting_turns_come_back_from_any_day(daemon: RunningDaemon) -> None:
    waiting = _turn(1, day="2026-11-01", status="working")
    waiting["open"] = True
    c = _as_app(daemon)
    try:
        c.call("chat.history.put", {"entries": [waiting, _turn(2)]})
        page = c.call("chat.history.page", {"day": "2026-11-08", "open": True})
        assert [e["id"] for e in page["open"]] == [waiting["id"]]
        answered = waiting | {"open": False, "body": waiting["body"] | {"status": "done", "answer": "Found it."}}
        c.call("chat.history.put", {"entries": [answered]})
        page = c.call("chat.history.page", {"day": "2026-11-01", "open": True})
        assert page["open"] == [] and page["entries"][0]["body"]["answer"] == "Found it."
        assert len(_rows(daemon)) == 2
    finally:
        c.close()


# decision: APP-808, DMN-555
def test_the_old_saved_chat_imports_any_number_of_times_and_never_takes_back_a_newer_write(daemon: RunningDaemon) -> None:
    saved = [_turn(i) for i in range(5)]
    c = _as_app(daemon)
    try:
        first = c.call("chat.history.import", {"entries": saved})
        assert first == {"changed": 5, "held": 5}
        newer = saved[0] | {"body": saved[0]["body"] | {"answer": "the answer as it is now"}}
        c.call("chat.history.put", {"entries": [newer]})
        again = c.call("chat.history.import", {"entries": saved})
        assert again == {"changed": 0, "held": 5}                       # nothing changed, every entry is kept
        assert _rows(daemon)[0]["body_json"].count("the answer as it is now") == 1
        assert len(_rows(daemon)) == 5
    finally:
        c.close()


# decision: DMN-555
def test_what_isnt_an_entry_is_refused_whole(daemon: RunningDaemon) -> None:
    c = _as_app(daemon)
    bad = [{"id": "not-a-uuid"}, _turn(0) | {"kind": "draft"}, _turn(0) | {"at": "2026-11-08T09:00:00"},
           _turn(0) | {"day": "Nov 8"}, _turn(0) | {"body": "text"}, _turn(0) | {"open": "yes"}]
    try:
        for entry in bad:
            with pytest.raises(RpcError):
                c.call("chat.history.put", {"entries": [_turn(1), entry]})
        with pytest.raises(RpcError):
            c.call("chat.history.page", {"day": "2026-11-08", "limit": 0})
    finally:
        c.close()
    assert _rows(daemon) == []


# decision: DMN-555
def test_a_secret_in_an_entry_is_redacted_before_it_is_kept(daemon: RunningDaemon) -> None:
    key = "sk-ant-api03-" + "Qx4" * 12
    c = _as_app(daemon)
    try:
        c.call("chat.history.put", {"entries": [_turn(0, answer=f"Your key is {key}")]})
    finally:
        c.close()
    assert key not in _rows(daemon)[0]["body_json"]


# decision: DMN-556
def test_a_notice_says_what_ferbs_record_of_it_says_and_a_forget_clears_it_here_too(daemon: RunningDaemon) -> None:
    async def post() -> None:
        notices = AttentionNotices(daemon.daemon.db, daemon.daemon.clock, daemon.daemon.redact)
        notices.add("N1", ["trail:t1"], "You copied the flight number twice.", quotes=[{"text": "LH 123"}])
    daemon.run(post())
    entry = str(uuid.uuid4()).upper()
    notice = {"id": entry, "kind": "notice", "at": DAY.isoformat(), "day": "2026-11-08", "ref": "N1",
              "body": {"id": "N1", "entryID": entry, "createdAt": DAY.timestamp(), "text": "an older copy",
                       "quotes": [], "tag": "noticed"}}
    c = _as_app(daemon)
    try:
        c.call("chat.history.put", {"entries": [notice]})
        kept = c.call("chat.history.page", {"day": "2026-11-08"})["entries"][0]["body"]
        assert kept["text"] == "You copied the flight number twice." and kept["quotes"][0]["text"] == "LH 123"

        async def forget() -> None:
            expire_derived(daemon.daemon.db, {"trail:t1"}, "Removed.", daemon.daemon.clock.now().isoformat())
        daemon.run(forget())
        cleared = c.call("chat.history.page", {"day": "2026-11-08"})["entries"][0]["body"]
        assert cleared["text"] == "Removed." and cleared["quotes"] == []
        c.call("chat.history.put", {"entries": [notice]})                 # the app's stale copy can't bring it back
        assert c.call("chat.history.page", {"day": "2026-11-08"})["entries"][0]["body"]["text"] == "Removed."
    finally:
        c.close()


# decision: DMN-555, STORE-403
def test_an_older_database_gains_the_chat_history_table(tmp_path: Path) -> None:
    from ferb.store.db import MIGRATIONS
    older = tmp_path / "older"
    older.mkdir()
    for f in MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql"):
        if int(f.name[:4]) < 62:
            (older / f.name).write_text(f.read_text())
    db = Database(tmp_path / "ferb.db")
    db.migrate(older)
    assert db.conn.execute("SELECT name FROM sqlite_master WHERE name = 'chat_history'").fetchone() is None
    assert db.migrate() == [62]
    assert db.conn.execute("SELECT COUNT(*) FROM chat_history").fetchone()[0] == 0
