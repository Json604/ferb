"""L9 (launch-week fixes, 2026-10-13), offline from documented shapes.

- A perception notice is a FERB message in the owner's chat: journaled `chat.notice {id, text, quotes}`, published on
  `attention.notice`, listed in `chat.status.notices` until a client acknowledges it, and told from the moment it is
  posted (no ack needed). macOS banners (notices and task results) only when no FERB client is connected.
- Jev's `trail.attention` is live (Q4): its decisions are recorded `mode='live'` with their effect (C3).
- Perception starts at its install: older trail rows and Notification Center records never reach Jev (I1).
- Events in an app while a FERB action runs there are FERB's (P4).
- Notification Center records are read with their real 4-letter keys; mutes match bundle ids in any case (I4).
- The D14 seven-day purge also clears notice text derived from typed/clipboard rows (I3).
"""
import asyncio
import json
import plistlib
import sqlite3
import threading
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Callable

from ferb.act.mac.tools import MacAct
from ferb.cli.client import Client
from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.core.types import ActionResult, Identity, ToolContext
from ferb.sanitize.redact import Redactor
from ferb.sense.batches import EventBatcher
from ferb.sense.daily import DailyActivity
from ferb.sense.exclusions import Exclusions
from ferb.sense.notifications import NotificationPolicy
from ferb.sense.perception import Perception
from ferb.sense.trail import Attribution, Observation, Trail
from ferb.sense.watchers.accessibility import accessibility_observation
from ferb.sense.watchers.notifications import NotificationDatabase
from ferb.store.db import Database
from tests.conftest import RunningDaemon, install

UPGRADE = "You upgraded Claude to Max 20x. It renews on Nov 13."
QUOTE = {"text": "Max 20x", "origin": "external:observation"}


def _brave(title: str) -> Observation:
    """The Claude-upgrade moment as Brave's Accessibility rows recorded it (traces.md §5)."""
    return accessibility_observation("com.brave.Browser", "Brave Browser", 501, "AXTitleChanged",
                                     {"window_id": "7", "role": "AXWindow", "title": title}, Redactor())


def _record(d: RunningDaemon, o: Observation) -> Any:
    async def go() -> Any:
        return d.daemon.trail.record(o)                      # type: ignore[union-attr]
    return d.run(go())


def _notes(d: RunningDaemon) -> list[tuple[str, str]]:
    notes: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        notes.append((title, text))
    d.daemon.notify = note
    return notes


def _consider(d: RunningDaemon, batch: str, *rows: Any) -> None:
    d.run(d.daemon._consider_attention({"batch_id": batch, "source_refs": ["trail:" + r.id for r in rows]}))


def _tell(d: RunningDaemon, text: str, told: list[Any]) -> None:
    """The consideration's model step, replaced: it records what it was told FERB already said."""
    async def attention(refs: list[str], rows: list[dict[str, Any]], already_told: list[dict[str, Any]]) -> dict[str, Any]:
        told.append(already_told)
        return {"tell": True, "text": text, "source_refs": refs[:1], "quotes": [QUOTE]}
    d.daemon.supervisor.attention = attention                # type: ignore[union-attr, method-assign]


def _until(check: Callable[[], Any], what: str) -> Any:
    for _ in range(200):
        value = check()
        if value:
            return value
        time.sleep(0.02)
    raise AssertionError("timed out waiting for " + what)


def _journal(d: RunningDaemon, kind: str) -> list[Any]:
    async def go() -> list[Any]:
        return [e for e in d.daemon._journal().entries() if e.kind == kind]
    return d.run(go())  # type: ignore[no-any-return]


def _call(d: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> Any:
    c = Client(d.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


class Subscriber:
    """An authenticated FERB client holding a bus subscription: unfiltered, as the notch UI and `ferb chat` do, or
    filtered, as `ferb logs --filter` and `ferb voice` do."""

    def __init__(self, d: RunningDaemon, filters: list[str] | None = None) -> None:
        self.d, self.client, self.events = d, Client(d.sock), []          # type: list[dict[str, Any]]
        self.filters = filters
        before = len(d.daemon._log_subscriptions)
        self.thread = threading.Thread(target=self._read, daemon=True)
        self.thread.start()
        _until(lambda: len(d.daemon._log_subscriptions) > before, "the subscription to open")

    def _read(self) -> None:
        try:
            for event in self.client.stream("logs.subscribe", {"filters": self.filters} if self.filters else {}):
                self.events.append(event)
        except Exception:
            return

    def topic(self, topic: str) -> list[dict[str, Any]]:
        return [e["data"] for e in self.events if e["topic"] == topic]

    def close(self) -> None:
        before = len(self.d.daemon._log_subscriptions)
        self.client.interrupt()
        self.client.close()
        self.thread.join(5)
        _until(lambda: len(self.d.daemon._log_subscriptions) < before, "the subscription to close")


# ---------- delivery ----------

def test_a_notice_reaches_the_chat_journal_and_status_with_no_banner_while_a_client_is_connected(
        daemon: RunningDaemon) -> None:
    notes, told = _notes(daemon), []
    _tell(daemon, UPGRADE, told)
    client = Subscriber(daemon)
    try:
        row = _record(daemon, _brave("Claude - Upgrade to Max"))
        _consider(daemon, "b1", row)
        notice = _until(lambda: client.topic("attention.notice"), "attention.notice")[0]
        assert {k: notice[k] for k in ("text", "quotes")} == {"text": UPGRADE, "quotes": [QUOTE]}
        assert notice["id"] and notice["at"] and "request_id" not in notice
        journaled = _journal(daemon, "chat.notice")
        assert [e.data for e in journaled] == [{"id": notice["id"], "text": UPGRADE, "quotes": [QUOTE]}]
        assert journaled[0].origin == "external:observation,inferred"       # MP2: inferred from outside data
        assert _call(daemon, "chat.status", {"peek": True})["notices"] == [
            {"id": notice["id"], "text": UPGRADE, "quotes": [QUOTE], "at": notice["at"]}]
        daemon.run(daemon.daemon._task_result({"task_id": "t-connected", "status": "done", "answer": "Done."},
                                              shown=False))
        assert notes == []                                                  # the client shows both, not osascript
        assert _call(daemon, "attention.ack", {"ids": [notice["id"]]}) == {"ok": True}
        assert _call(daemon, "chat.status", {"peek": True})["notices"] == []
    finally:
        client.close()


def test_a_subscription_that_never_shows_notices_does_not_stop_banners(daemon: RunningDaemon) -> None:
    """`ferb logs --filter timers` or `ferb voice` is connected but never shows a notice or a task result."""
    notes = _notes(daemon)
    _tell(daemon, UPGRADE, [])
    timers, voice = Subscriber(daemon, ["timers"]), Subscriber(daemon, ["voice"])
    try:
        _consider(daemon, "b-filtered", _record(daemon, _brave("Claude - Upgrade to Max")))
        daemon.run(daemon.daemon._task_result({"task_id": "t-filtered", "status": "done", "answer": "Done."},
                                              shown=False))
        assert notes == [("FERB noticed", UPGRADE), ("FERB finished", "Done.")]
    finally:
        timers.close()
        voice.close()


def test_quote_origins_come_from_the_considered_rows_never_from_the_model(daemon: RunningDaemon) -> None:
    _notes(daemon)

    async def attention(refs: list[str], rows: list[dict[str, Any]], already_told: list[dict[str, Any]]) -> dict[str, Any]:
        return {"tell": True, "text": UPGRADE, "source_refs": refs,
                "quotes": [{"text": "Max 20x", "origin": "owner"}, {"text": "renews Nov 13"}]}
    daemon.daemon.supervisor.attention = attention          # type: ignore[union-attr, method-assign]
    _consider(daemon, "b-quotes", _record(daemon, _brave("Claude - Upgrade to Max")))
    notice = _call(daemon, "chat.status", {"peek": True})["notices"][0]
    assert notice["quotes"] == [{"text": "Max 20x", "origin": "external:observation"},
                                {"text": "renews Nov 13", "origin": "external:observation"}]
    assert _journal(daemon, "chat.notice")[0].data["quotes"] == notice["quotes"]


def test_the_daemon_wires_ferbs_app_actions_into_the_trail(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    assert d.mac is not None and d.mac.acting == d.attribution.acting

    async def during() -> Any:
        with d.mac.acting("com.apple.Notes", "task-wired"):                   # type: ignore[union-attr]
            return d.trail.record(accessibility_observation(                    # type: ignore[union-attr]
                "com.apple.Notes", "Notes", 77, "AXValueChanged", {"window_id": "3", "title": "milk"}, Redactor()))
    row = daemon.run(during())
    assert (row.actor, row.task_id) == ("ferb", "task-wired")


def test_with_no_client_connected_the_banner_is_sent(daemon: RunningDaemon) -> None:
    notes = _notes(daemon)
    _tell(daemon, UPGRADE, [])
    _consider(daemon, "b2", _record(daemon, _brave("Claude - Upgrade to Max")))
    daemon.run(daemon.daemon._task_result({"task_id": "t-alone", "status": "done", "answer": "Done."}, shown=False))
    assert notes == [("FERB noticed", UPGRADE), ("FERB finished", "Done.")]
    # The notice is in the owner's chat as well, waiting for the next client that opens it.
    assert [e.data["text"] for e in _journal(daemon, "chat.notice")] == [UPGRADE]
    assert [(n["text"], n["quotes"]) for n in _call(daemon, "chat.status", {"peek": True})["notices"]] == [
        (UPGRADE, [QUOTE])]


def test_the_second_consideration_sees_the_first_notice_as_told_without_any_ack(daemon: RunningDaemon) -> None:
    _notes(daemon)
    told: list[Any] = []
    _tell(daemon, UPGRADE, told)
    _consider(daemon, "b3", _record(daemon, _brave("Claude - Upgrade to Max")))
    _consider(daemon, "b4", _record(daemon, _brave("Processing payment...")))
    assert told[0] == []
    assert [n["text"] for n in told[1]] == [UPGRADE]                         # posted to chat = told; nobody acked


# ---------- audit ----------

def test_trail_attention_decisions_are_recorded_live_with_their_effect(daemon: RunningDaemon) -> None:
    d = daemon.daemon

    async def jev(url: str, headers: dict[str, str], body: dict[str, Any], timeout: float) -> tuple[int, Any]:
        return 200, {"answers": {n: {"type": "noul", "noul": 0.9} for n in body["questions"]}}
    d.jev_post = jev
    daemon.run(d._build_jev())
    _notes(daemon)
    _tell(daemon, UPGRADE, [])
    _record(daemon, _brave("Claude - Upgrade to Max"))
    daemon.run(d.perception.batches.flush("test"))

    def decision() -> Any:
        async def go() -> Any:
            return d.db.conn.execute("SELECT mode, effect, ref FROM decisions WHERE question='trail.attention'").fetchall()
        rows = daemon.run(go())
        return rows if rows and rows[-1]["effect"] and "told" in rows[-1]["effect"] else None
    rows = _until(decision, "the decision's effect")
    assert [r["mode"] for r in rows] == ["live"]
    notice = _journal(daemon, "chat.notice")[0].data
    assert notice["id"] in rows[0]["effect"]
    stats = next(s for s in daemon.run(_async(d.jev.stats)) if s["question"] == "trail.attention")
    assert stats["mode"] == "live" and stats["acted"] == 1


def test_a_live_question_has_no_switch(daemon: RunningDaemon) -> None:
    from ferb.cli.client import RpcError
    import pytest
    for on in (True, False):
        with pytest.raises(RpcError, match="live"):
            _call(daemon, "jev.enable", {"question": "trail.attention", "on": on})


def test_a_consideration_that_ends_early_records_why_on_its_decision(daemon: RunningDaemon) -> None:
    d = daemon.daemon

    async def jev(url: str, headers: dict[str, str], body: dict[str, Any], timeout: float) -> tuple[int, Any]:
        return 200, {"answers": {}}
    d.jev_post = jev
    daemon.run(d._build_jev())
    arrival = _record(daemon, Observation("notification", {"external_id": "n-early", "bundle_id": "net.whatsapp.whatsapp",
                                                           "title": "Family", "body": "hi"}, {"actor": "unknown"}))

    async def decided() -> None:
        d.db.conn.execute("INSERT INTO decisions (id, at, question, ref, baseline, jev, mode, effect) VALUES "
                          "('d-early', 'x', 'trail.attention', 'batch:b-early', 'no', 'yes', 'live', "
                          "'attention consideration started')")
    daemon.run(decided())
    _consider(daemon, "b-early", arrival)
    effect = daemon.run(_async(lambda: d.db.conn.execute(
        "SELECT effect FROM decisions WHERE id='d-early'").fetchone()["effect"]))
    assert effect != "attention consideration started" and effect.startswith("not considered")


def test_the_consideration_is_published_before_its_decision_says_it_started(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("o")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *args: None, lambda: "dev", lambda: False)
    trail.record(Observation("window", {"bundle_id": "com.microsoft.VSCode", "title": "FERB"}))
    order: list[str] = []

    class Jev:
        async def answers(self, state: Any, asks: Any) -> dict[str, str]:
            return {asks[0].key: "yes"}

        def mark_effect(self, key: str, ref: str | None, effect: str) -> None:
            order.append("effect")
    batcher = EventBatcher(db, clock, ids, lambda: Jev(), NotificationPolicy(db, clock, ids, Redactor()),
                           lambda topic, data: order.append(topic))
    asyncio.run(batcher.flush("age"))
    assert order == ["perception.batch", "perception.attention", "effect"]
    db.close()


def _async(fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return go()


# ---------- install floor ----------

def test_rows_recorded_before_the_perception_install_are_never_batched(tmp_path: Path, monkeypatch: Any) -> None:
    import ferb.store.db as store
    real = store.MIGRATIONS
    install_migration = next(f for f in sorted(real.glob("[0-9][0-9][0-9][0-9]_*.sql"))
                             if "perception_floor" in f.read_text())
    before = tmp_path / "migrations"
    before.mkdir()
    for f in real.glob("[0-9][0-9][0-9][0-9]_*.sql"):
        if f.name < install_migration.name:
            (before / f.name).write_text(f.read_text())
    monkeypatch.setattr(store, "MIGRATIONS", before)
    db = Database(tmp_path / "ferb.db")
    db.migrate()                                                         # the version installed before
    clock, ids = FakeClock(), SequentialIds("i")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *args: None, lambda: "dev", lambda: False)
    old = trail.record(Observation("clipboard", {"bundle_id": "com.apple.Notes", "text": "copied last week"}))
    monkeypatch.setattr(store, "MIGRATIONS", real)
    db.migrate()                                                         # perception's install/migration
    new = trail.record(Observation("window", {"bundle_id": "com.microsoft.VSCode", "title": "FERB"}))
    states: list[Any] = []

    class Jev:
        async def answers(self, state: Any, asks: Any) -> dict[str, str]:
            states.append(state)
            return {}
    batcher = EventBatcher(db, clock, ids, lambda: Jev(), NotificationPolicy(db, clock, ids, Redactor()),
                           lambda *args: None)
    assert [e["id"] for e in batcher.pending()] == [new.id] and batcher.pending_count() == 1
    asyncio.run(batcher.flush("age"))
    assert [e["id"] for state in states for e in state["events"]] == [new.id]
    assert old.id not in json.dumps(states)
    db.close()


# ---------- actor tagging ----------

def test_an_event_in_an_app_while_a_ferb_action_runs_there_is_ferbs(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    attribution = Attribution()
    trail = Trail(db, FakeClock(), SequentialIds("a"), Redactor(), Exclusions, lambda *args: None, lambda: "dev",
                  lambda: False, attribution)
    notes_app = "com.apple.Notes"

    def ax(title: str) -> Observation:
        return accessibility_observation(notes_app, "Notes", 77, "AXValueChanged",
                                         {"window_id": "3", "role": "AXTextArea", "title": title}, Redactor())
    during: dict[str, Any] = {}

    async def act(app: str, action: Any, holder: str = "", task: str | None = None) -> ActionResult:
        during["input"] = trail.record(ax("milk"))                             # FERB's typing, seen by AX
        during["arrival"] = trail.record(Observation("notification", {"external_id": "n1", "bundle_id": notes_app,
                                                                      "title": "Shared note"}, {"actor": "unknown"}))
        return ActionResult(True, "typed")
    driver = SimpleNamespace(apps=SimpleNamespace(find=lambda name: SimpleNamespace(bundle_id=notes_app, name="Notes")),
                             act=act, acting=attribution.acting)
    tool = MacAct(driver)                                                # type: ignore[arg-type]
    ctx = ToolContext("task-1", "w1", "r1", Identity.OWNER, frozenset({"mac:act"}), "dev", None, "/tmp", (), ())
    result = asyncio.run(tool.run(tool.Input(app="Notes", action="type", text="milk"), ctx))
    assert not result.is_error
    later = trail.record(ax("eggs"))
    actors = {r["id"]: (r["actor"], r["task_id"]) for r in db.conn.execute("SELECT id, actor, task_id FROM trail")}
    assert actors[during["input"].id] == ("ferb", "task-1")
    assert actors[during["arrival"].id] == ("unknown", None)              # a notification arrives from outside
    assert actors[later.id] == ("unknown", None)                          # the action ended: no timer
    db.close()


def test_the_owners_hidden_input_wait_is_not_ferbs_only_the_fill_is(tmp_path: Path) -> None:
    from ferb.act.mac.tools import MacSecret
    attribution, app = Attribution(), "com.example.Bank"
    ax = accessibility_observation(app, "Bank", 88, "AXValueChanged", {"window_id": "1", "title": "Code"}, Redactor())
    seen: dict[str, Any] = {}
    node = SimpleNamespace(ref="field")

    async def ask_secret(ctx: Any, where: str, handle: Any = None, remember: Any = None) -> str:
        seen["waiting"] = attribution.decide(ax)                            # the owner typing into the app meanwhile
        return "424242"

    def set_value(ref: Any, value: str) -> bool:
        seen["filling"] = attribution.decide(ax)
        return True
    driver = SimpleNamespace(apps=SimpleNamespace(find=lambda name: SimpleNamespace(bundle_id=app, name="Bank")),
                             app=lambda name: SimpleNamespace(bundle_id=app, pid=88), node=lambda bundle, element: node,
                             ask_secret=ask_secret, ax=SimpleNamespace(set_value=set_value),
                             release_secret=lambda value: None, acting=attribution.acting)
    tool = MacSecret(driver)                                             # type: ignore[arg-type]
    ctx = ToolContext("task-2", "w1", "r1", Identity.OWNER, frozenset({"mac:act"}), "dev", None, "/tmp", (), ())
    assert not asyncio.run(tool.run(tool.Input(app="Bank", element="s1e1"), ctx)).is_error
    assert seen == {"waiting": ("unknown", None), "filling": ("ferb", "task-2")}


# ---------- Notification Center ----------

def test_a_lowercase_whatsapp_row_is_muted_by_a_rule_saved_with_capitals(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("m")
    policy = NotificationPolicy(db, clock, ids, Redactor())
    policy.add("net.whatsapp.WhatsApp", None, None, "mute WhatsApp", "please mute WhatsApp")
    assert policy.muted("net.whatsapp.whatsapp", "Family")                # Notification Center stores lowercase
    batcher = EventBatcher(db, clock, ids, lambda: None, policy, lambda *args: None)
    row = {"id": "n1", "source": "notification", "data": {"bundle_id": "net.whatsapp.whatsapp", "conversation": "Family"}}
    assert batcher._request([row], "b") == ({"muted": True}, [])
    db.close()


def _nc(path: Path, records: list[tuple[str, dict[str, Any]]]) -> None:
    """The usernoted db2 shape: `app` and `record` tables; each record's data is a binary plist whose request
    dictionary uses Notification Center's 4-letter keys (titl, subt, body, thre, iden, cate)."""
    with sqlite3.connect(path) as c:
        c.execute("CREATE TABLE IF NOT EXISTS app (app_id INTEGER PRIMARY KEY, identifier VARCHAR, badge INTEGER NULL)")
        c.execute("CREATE TABLE IF NOT EXISTS record (rec_id INTEGER PRIMARY KEY, app_id INTEGER, uuid BLOB, data BLOB, "
                  "request_date REAL, request_last_date REAL, delivered_date REAL, presented Bool, style INTEGER, "
                  "snooze_fire_date REAL)")
        for bundle, req in records:
            row = c.execute("SELECT app_id FROM app WHERE identifier=?", (bundle,)).fetchone()
            app_id = row[0] if row else c.execute("INSERT INTO app (identifier) VALUES (?)", (bundle,)).lastrowid
            uid = (bundle + req.get("iden", "")).encode()
            data = plistlib.dumps({"app": bundle, "date": 782000000.0, "req": req, "uuid": uid}, fmt=plistlib.FMT_BINARY)
            c.execute("INSERT INTO record (app_id, uuid, data, delivered_date, presented, style) VALUES (?,?,?,?,1,1)",
                      (app_id, uid, data, 782000000.0))


def test_a_real_shape_notification_center_record_yields_its_thread_id_and_is_read_once(tmp_path: Path) -> None:
    path = tmp_path / "db"
    group = {"titl": "Family", "subt": "Mum", "body": "Dinner at 8?", "iden": "A1",
             "thre": "120363020000000000@g.us", "cate": "net.whatsapp.message"}
    _nc(path, [("net.whatsapp.whatsapp", group)])
    reader = NotificationDatabase(path, Redactor())
    first = reader.read()
    assert [(o.data["conversation"], o.data["conversation_source"]) for o in first] == [
        ("120363020000000000@g.us", "thread")]
    assert first[0].data["bundle_id"] == "net.whatsapp.whatsapp" and first[0].data["title"] == "Family"
    assert reader.read() == []                                          # incremental: nothing new, nothing re-read
    _nc(path, [("com.apple.iCal", {"titl": "Standup", "body": "in 5 minutes", "iden": "B2"})])
    second = reader.read()
    assert [(o.data["conversation"], o.data["conversation_source"]) for o in second] == [("Standup", "title")]


def test_notification_center_records_present_at_install_are_marked_seen_not_recorded(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids, redact = FakeClock(), SequentialIds("p"), Redactor()
    activity = DailyActivity(tmp_path / "activity.jsonl", clock.now)
    trail = Trail(db, clock, ids, redact, Exclusions, lambda *args: None, lambda: "dev", lambda: False)
    daemon = SimpleNamespace(db=db, clock=clock, ids=ids, redact=redact, daily_activity=activity, trail=trail,
                             sense=None, jev=None, _publish=lambda *args: None,
                             config=SimpleNamespace(get=lambda *args: args[-1]))
    perception = Perception(daemon)
    old = Observation("notification", {"external_id": "before-install", "bundle_id": "net.whatsapp.whatsapp",
                                       "body": "from last month"}, {"actor": "unknown"})
    assert perception.notification_floor([old]) == []                    # the first read after install
    assert perception.seen("before-install")
    new = Observation("notification", {"external_id": "after-install", "bundle_id": "net.whatsapp.whatsapp",
                                       "body": "now"}, {"actor": "unknown"})
    assert perception.notification_floor([new]) == [new]                 # later reads pass through
    assert db.conn.execute("SELECT COUNT(*) FROM trail").fetchone()[0] == 0
    db.close()


# ---------- D14 purge ----------

def test_the_seven_day_purge_clears_notice_text_derived_from_a_clipboard_row(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _notes(daemon)
    focus = _record(daemon, accessibility_observation("com.apple.Notes", "Notes", 77, "AXFocusedUIElementChanged",
                                                       {"window_id": "3", "role": "AXTextArea", "title": "Notes"},
                                                       Redactor()))
    copied = _record(daemon, Observation("clipboard", {"bundle_id": "com.apple.Notes", "text": "uniquecopiednote",
                                                       "change_count": 3}))
    later = _record(daemon, _brave("Claude - Settings"))
    # The model cites only the focus row, but it saw the copied text too: the notice is derived from both. A later,
    # unrelated consideration is told the notice (already_told), so its recorded context holds the text as well.
    install(daemon, [json.dumps({"tell": True, "text": "You copied uniquecopiednote.", "source_refs": ["trail:" + focus.id],
                                 "quotes": [{"text": "uniquecopiednote", "origin": "external:observation"}]}),
                     json.dumps({"tell": False, "text": "", "source_refs": ["trail:" + later.id]})])
    _consider(daemon, "b9", focus, copied)
    _consider(daemon, "b10", later)
    assert "uniquecopiednote" in json.dumps(_call(daemon, "chat.status", {"peek": True})["notices"])

    def copies() -> str:
        async def go() -> str:
            c = d.db.conn
            return json.dumps([[dict(r) for r in c.execute("SELECT * FROM attention_notices")],
                               [dict(r) for r in c.execute("SELECT * FROM journal WHERE kind='chat.notice'")],
                               [dict(r) for r in c.execute("SELECT input_json, output_json FROM spans "
                                                           "WHERE kind='attention.consider'")],
                               [dict(r) for r in c.execute("SELECT * FROM notification_told")]])
        return daemon.run(go())  # type: ignore[no-any-return]
    spans = json.loads(copies())[2]
    assert len(spans) == 2 and all("uniquecopiednote" in span["input_json"] for span in spans)
    daemon.call_soon(d.clock.advance, 7 * 86400 + 1)
    assert daemon.run(_async(d.perception.purge_text)) == 1
    assert "uniquecopiednote" not in copies()
    assert _call(daemon, "chat.status", {"peek": True})["notices"] == []
    assert len(json.loads(copies())[2]) == 2                             # audit rows stay; only the text goes


def test_notices_from_rows_that_expired_before_this_purge_reached_them_are_cleared(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _notes(daemon)
    copied = _record(daemon, Observation("clipboard", {"bundle_id": "com.apple.Notes", "text": "olduniquecopy",
                                                       "change_count": 5}))
    install(daemon, [json.dumps({"tell": True, "text": "You copied olduniquecopy.", "source_refs": ["trail:" + copied.id]})])
    _consider(daemon, "b11", copied)

    async def expired_by_the_old_purge() -> None:                        # the row's text went; the notice's stayed
        d.db.conn.execute("UPDATE trail SET data_json=? WHERE id=?",
                          (json.dumps({"bundle_id": "com.apple.Notes", "change_count": 5, "content_expired": True}),
                           copied.id))
    daemon.run(expired_by_the_old_purge())
    assert daemon.run(_async(d.perception.purge_text)) == 0               # no newly expired rows
    assert "olduniquecopy" not in daemon.run(_async(lambda: json.dumps(
        [dict(r) for r in d.db.conn.execute("SELECT * FROM attention_notices")] +
        [dict(r) for r in d.db.conn.execute("SELECT * FROM journal WHERE kind='chat.notice'")])))
