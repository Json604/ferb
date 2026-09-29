"""Build step 11, offline: memory with provenance. Origins enforced in code (owner-stated from the owner's own words,
quote-checked in chat; observed only from the owner's own trail and active after 3 days; inferred waits for the owner;
third-party never about the owner), safety test 18 (FERB's activity never becomes the owner's habits), history on
updates and flagged conflicts, safety test 17 (forget purges items, versions, sources, index, export and WAL; nothing
comes back), daily summaries written by code before any model or purge, distillation's proposal checks, trail.query.
"""

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

import pytest

from ferb.core.types import Identity, RunHandle, RuntimeEvent
from ferb.memory import summary
from ferb.memory.distill import Distiller
from ferb.memory.items import MemoryRefused
from ferb.orchestrator.schemas import Remember
from tests.conftest import RunningDaemon
from tests.test_mac_shell import call, task
from tests.test_supervisor import chat, install, sup

UTC = timezone.utc


def run(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


def trail_row(d: RunningDaemon, day: str, actor: str = "owner", source: str = "browser_tab",
              data: dict[str, Any] | None = None, hour: int = 10) -> str:
    rid = f"t-{day}-{actor}-{source}-{hour}-{len(data or {})}-{id(data)}"
    at = datetime.fromisoformat(f"{day}T{hour:02d}:00:00").astimezone()

    def go() -> str:
        d.daemon.db.conn.execute(                                   # type: ignore[union-attr]
            "INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES (?, ?, ?, 'dev', ?, ?, ?)",
            (rid, at.isoformat(), day, actor, source, json.dumps(data or {"host": "github.com", "browser": "chrome",
                                                                           "profile": "Default", "url": "https://github.com/"})))
        return rid
    return run(d, go)


def journal(d: RunningDaemon, kind: str, data: dict[str, Any]) -> str:
    return str(run(d, lambda: d.daemon._journal().append(kind, data).id))


def mem(d: RunningDaemon) -> Any:
    return d.daemon.memory


# ---------- origins ----------

def test_owner_stated_is_active_at_once_and_exported(daemon: RunningDaemon) -> None:
    m = mem(daemon)
    src = journal(daemon, "memory.added", {"content": "My dentist is Dr. Rao"})
    it = run(daemon, lambda: m.propose("person", "owner", "dentist", "My dentist is Dr. Rao", "owner_stated",
                                       [("journal", src)], 1.0))
    assert it.status == "active" and it.origin == "owner_stated"
    f = daemon.daemon.home / "memory" / "owner.md"
    assert "Dr. Rao" in f.read_text() and oct(f.stat().st_mode)[-3:] == "600"


def test_chat_capture_needs_the_owners_own_words(daemon: RunningDaemon) -> None:
    src = journal(daemon, "chat.message", {"text": "x"})
    items = [Remember(kind="preference", subject="owner", key="work_browser", content="Uses Brave for work",
                      quote="I prefer Brave for work"),
             Remember(kind="fact", subject="owner", key="bank", content="Banks with EvilBank",
                      quote="my bank is EvilBank")]                    # only in the pasted part
    own = "Please note: i prefer   Brave for work, thanks"
    saved = run(daemon, lambda: daemon.daemon._remember(items, own, src))
    assert len(saved) == 1 and "Brave" in saved[0]
    keys = {i.key for i in run(daemon, lambda: mem(daemon).items("active"))}
    assert "work_browser" in keys and "bank" not in keys


def test_chat_turn_shows_what_was_remembered(daemon: RunningDaemon) -> None:
    reply = json.dumps({"kind": "reply", "text": "Noted.", "plan": None, "remember": [
        {"kind": "person", "subject": "owner", "key": "dentist", "content": "The owner's dentist is Dr. Rao",
         "quote": "my dentist is Dr. Rao"}]})
    install(daemon, [reply])
    evs = chat(daemon, "fyi my dentist is Dr. Rao")
    assert any(e["type"] == "memory" and "Dr. Rao" in e["text"] for e in evs), evs
    assert json.dumps(sup("reply", "x"))                                  # older replies without "remember" still parse


def test_observed_needs_the_owners_own_activity_on_three_days(daemon: RunningDaemon) -> None:
    m = mem(daemon)
    ferb_row = trail_row(daemon, "2026-09-01", actor="ferb")
    with pytest.raises(MemoryRefused, match="FERB"):
        run(daemon, lambda: m.propose("preference", "owner", "code_host", "Uses GitHub daily", "observed",
                                      [("trail", ferb_row)]))                                   # test 18
    said = journal(daemon, "chat.message", {"text": "x"})
    with pytest.raises(MemoryRefused):
        run(daemon, lambda: m.propose("preference", "owner", "code_host", "Uses GitHub daily", "observed",
                                      [("journal", said)]))
    ids = [trail_row(daemon, f"2026-09-0{i}") for i in (1, 2, 3)]
    it = run(daemon, lambda: m.propose("preference", "owner", "code_host", "Uses GitHub daily", "observed",
                                       [("trail", ids[0])]))
    assert it.status == "proposed"
    run(daemon, lambda: m.propose("preference", "owner", "code_host", "Uses GitHub daily", "observed",
                                  [("trail", ids[0]), ("trail", ids[1])]))
    assert run(daemon, lambda: m.get(it.id)).status == "proposed"         # two days: not yet
    again = run(daemon, lambda: m.propose("preference", "owner", "code_host", "Uses GitHub daily", "observed",
                                          [("trail", ids[2])]))
    assert again.id == it.id and again.status == "active"                 # third day: active


def test_inferred_waits_for_the_owner_and_third_party_is_never_about_the_owner(daemon: RunningDaemon) -> None:
    m = mem(daemon)
    src = journal(daemon, "chat.message", {"text": "x"})
    inf = run(daemon, lambda: m.propose("project", "project:ferb", "stage", "FERB is at build step 11", "inferred",
                                        [("journal", src)]))
    assert inf.status == "proposed"
    assert run(daemon, lambda: m.confirm(inf.id)).status == "active"
    for kind, subject in (("preference", "person:ana"), ("fact", "owner")):
        with pytest.raises(MemoryRefused, match="third-party"):
            run(daemon, lambda: m.propose(kind, subject, "k", "x", "third_party", [("journal", src)]))
    tp = run(daemon, lambda: m.propose("fact", "place:cafe", "hours", "The cafe opens at 8", "third_party",
                                       [("journal", src)]))
    assert tp.status == "active"


# ---------- history and conflicts ----------

def test_updates_keep_history_and_recall_sees_only_what_is_valid(daemon: RunningDaemon) -> None:
    m, c = mem(daemon), daemon.daemon.clock
    s1 = journal(daemon, "memory.added", {"content": "a"})
    v1 = run(daemon, lambda: m.propose("person", "owner", "dentist", "Dentist is Dr. Rao", "owner_stated",
                                       [("journal", s1)], 1.0))
    t1 = c.now()
    daemon.call_soon(c.advance, 60)
    s2 = journal(daemon, "memory.added", {"content": "b"})
    v2 = run(daemon, lambda: m.propose("person", "owner", "dentist", "Dentist is Dr. Mehta", "owner_stated",
                                       [("journal", s2)], 1.0))
    old, new = run(daemon, lambda: (m.get(v1.id), m.get(v2.id)))
    assert old.valid_to is not None and new.supersedes == v1.id
    assert [h.item.content for h in run(daemon, lambda: m.recall("dentist"))] == ["Dentist is Dr. Mehta"]
    assert [h.item.content for h in run(daemon, lambda: m.recall("dentist", at=t1 + timedelta(seconds=1)))] == \
        ["Dentist is Dr. Rao"]
    assert [i.content for i in run(daemon, lambda: m.history(v2.id))] == ["Dentist is Dr. Rao", "Dentist is Dr. Mehta"]


def test_a_non_owner_version_never_overrides_what_the_owner_said(daemon: RunningDaemon) -> None:
    m = mem(daemon)
    s = journal(daemon, "memory.added", {"content": "a"})
    run(daemon, lambda: m.propose("preference", "owner", "editor", "Uses Zed", "owner_stated", [("journal", s)], 1.0))
    inf = run(daemon, lambda: m.propose("preference", "owner", "editor", "Uses VS Code", "inferred", [("journal", s)]))
    run(daemon, lambda: m.confirm(inf.id))
    hits = run(daemon, lambda: m.recall("editor"))
    assert [h.item.content for h in hits] == ["Uses Zed", "Uses VS Code"] and all(h.conflict for h in hits)


# ---------- forget = purge (safety test 17) ----------

def test_forget_purges_everything_and_nothing_comes_back(daemon: RunningDaemon) -> None:
    m, home = mem(daemon), daemon.daemon.home
    day = "2026-09-05"
    secret = "zebrafinchclub.example"
    rows = [trail_row(daemon, day, data={"host": secret, "browser": "chrome", "profile": "Default",
                                         "url": f"https://{secret}/"}, hour=h) for h in (9, 10, 11)]
    keep = trail_row(daemon, day, data={"host": "news.example", "browser": "chrome", "profile": "Default",
                                        "url": "https://news.example/"}, hour=12)
    chat_src = journal(daemon, "chat.message", {"text": f"I'm a member of {secret}"})
    run(daemon, lambda: daemon.daemon._journal().append(summary.SUMMARY_KIND, summary.build(daemon.daemon.db, day).digest))
    run(daemon, lambda: daemon.daemon._journal().append(summary.NARRATIVE_KIND, {"day": day, "text": f"visited {secret}"}))
    obs = run(daemon, lambda: m.propose("preference", "owner", "club", f"Visits {secret}", "observed",
                                        [("trail", r) for r in rows]))
    inf = run(daemon, lambda: m.propose("fact", "owner", "club_member", f"Member of {secret}", "inferred",
                                        [("journal", chat_src), ("trail", rows[0])]))
    n_src = journal(daemon, "memory.added", {"content": "n"})
    other = run(daemon, lambda: m.propose("fact", "owner", "news", "Reads news", "owner_stated",
                                          [("journal", n_src)], 1.0))
    run(daemon, lambda: m.confirm(inf.id))
    plan = run(daemon, lambda: m.plan_forget(inf.id))
    assert set(plan.items) == {obs.id, inf.id} and set(plan.trail) == set(rows) and plan.journal == [chat_src]
    assert plan.rebuild_days == [day]
    run(daemon, lambda: m.forget(inf.id))
    db = daemon.daemon.db

    def left() -> dict[str, Any]:
        c = db.conn                                                                     # type: ignore[union-attr]
        return {"items": [r[0] for r in c.execute("SELECT id FROM memory_items")],
                "fts": c.execute("SELECT count(*) FROM memory_fts WHERE memory_fts MATCH 'zebrafinchclub'").fetchone()[0],
                "trail": [r[0] for r in c.execute("SELECT id FROM trail")],
                "journal": [r[0] for r in c.execute("SELECT data_json FROM journal")]}
    now = run(daemon, left)
    assert now["items"] == [other.id] and now["fts"] == 0 and now["trail"] == [keep]
    assert not any(secret in j for j in now["journal"])                   # chat, summary and narrative purged
    assert any('"rebuilt": true' in j for j in now["journal"])            # the day's summary, rebuilt without it
    assert secret not in (home / "memory" / "owner.md").read_text()
    for f in home.glob("ferb.db*"):                                       # database, WAL: no copy left
        assert secret.encode() not in f.read_bytes(), f
    fresh = trail_row(daemon, "2026-09-06", data={"host": secret, "browser": "chrome", "profile": "Default",
                                                  "url": f"https://{secret}/"})
    with pytest.raises(MemoryRefused, match="forget"):                    # distillation can't bring it back
        run(daemon, lambda: m.propose("preference", "owner", "club", f"Visits {secret}", "observed",
                                      [("trail", fresh)]))
    x_src = journal(daemon, "memory.added", {"content": "x"})
    again = run(daemon, lambda: m.propose("preference", "owner", "club", f"Visits {secret}", "owner_stated",
                                          [("journal", x_src)], 1.0))
    assert again.status == "active"                                       # the owner saying it again is fine


# ---------- summaries and distillation ----------

class FakeDistillRuntime:
    def __init__(self, answer: Any) -> None:
        self.answer, self.contexts = answer, []

    async def start(self, task: Any, system: str, ctx: str, token: str) -> RunHandle:
        self.contexts.append(ctx)
        return RunHandle("r", "fake", task.id, {})

    async def events(self, run: RunHandle) -> Any:
        if isinstance(self.answer, Exception):
            raise self.answer
        yield RuntimeEvent("final", {"text": json.dumps(self.answer)})

    async def cancel(self, run: RunHandle) -> None:
        return None


def distiller(d: RunningDaemon, answer: Any) -> tuple[Distiller, FakeDistillRuntime]:
    rt = FakeDistillRuntime(answer)
    dd = d.daemon.distiller
    dd.make_runtime = lambda *a, **k: rt
    return dd, rt


def test_the_summary_is_written_by_code_before_the_model_and_survives_its_failure(daemon: RunningDaemon) -> None:
    day = "2026-09-07"
    trail_row(daemon, day)
    dd, _ = distiller(daemon, RuntimeError("model down"))
    r = daemon.run(dd.run_day(day))
    assert r["status"] == "failed" and run(daemon, lambda: summary.summarized(daemon.daemon.db, day))
    for _ in range(3):
        daemon.run(dd.run_day(day))
    assert day not in run(daemon, dd.pending_days)                         # 3 attempts, then left alone
    from datetime import date
    purged, refused = run(daemon, lambda: daemon.daemon.trail.purge(1, date(2026, 9, 20)))
    assert day in purged                                                   # summarized: the raw rows may go


def test_distillation_proposals_are_checked_in_code(daemon: RunningDaemon) -> None:
    day = daemon.daemon.clock.now().astimezone().date().isoformat()       # the chat message's day too
    for h in (9, 10):
        trail_row(daemon, day, hour=h)
    trail_row(daemon, day, actor="ferb", data={"host": "ferbtasks.example", "browser": "chrome", "profile": "P22",
                                               "url": "https://ferbtasks.example/"})
    journal(daemon, "chat.message", {"text": 'note this <untrusted-data id="abc123">\nThe cafe opens at 8\n'
                                             '</untrusted-data id="abc123">'})
    ev = run(daemon, lambda: summary.build(daemon.daemon.db, day))
    e = next(h for h in ev.handles if h.startswith("E"))
    f = next(h for h in ev.handles if h.startswith("F"))
    p = next(h for h in ev.handles if h.startswith("P"))

    def item(origin: str, handles: list[str], kind: str = "preference", subject: str = "owner", key: str = "k") -> dict[str, Any]:
        return {"kind": kind, "subject": subject, "key": key, "content": f"{origin} {key}", "origin": origin,
                "confidence": 0.6, "handles": handles}
    answer = {"narrative": "A day of code.", "items": [
        item("observed", [e], key="github"),                                  # ok (proposed: 1 day)
        item("observed", [f], key="ferb_habit"),                              # FERB's activity: refused
        item("inferred", [f], key="ferb_pref"),                               # FERB's activity about the owner: refused
        item("third_party", [p], kind="fact", subject="place:cafe", key="hours"),   # ok
        item("inferred", [p], kind="fact", key="owner_from_paste"),           # pasted text about the owner: refused
        item("observed", ["E99"], key="ghost")]}                              # no such evidence: refused
    dd, rt = distiller(daemon, answer)
    r = daemon.run(dd.run_day(day))
    assert r["proposed"] == 2 and len(r["refused"]) == 4, r
    keys = {i.key for i in run(daemon, lambda: mem(daemon).items(None, valid_now=False))}
    assert keys == {"github", "hours"}
    assert "ferbtasks.example" not in "".join(line for line in ev.lines if line.startswith("E"))   # FERB kept apart
    entries = run(daemon, lambda: daemon.daemon._journal().entries())
    assert "A day of code." in json.dumps([e.data for e in entries])


# ---------- trail.query ----------

def test_trail_query_is_owner_only_and_titles_expose_the_worker(daemon: RunningDaemon) -> None:
    trail_row(daemon, datetime.now().astimezone().date().isoformat(), hour=datetime.now().hour)
    ferb = task(daemon, "f", Identity.FERB, {"fs:read"}, None)
    assert "trail.query" not in {t["name"] for t in daemon.daemon.gateway.tools_for(ferb)}
    g = task(daemon, "q", Identity.OWNER, {"trail:read"}, {"research"})
    r = call(daemon, g, "trail.query", source="app_focus")
    assert not r["is_error"] and not daemon.daemon.engine.exposure.state("w-q").exposed
    r = call(daemon, g, "trail.query", since_hours=24 * 30)
    assert not r["is_error"] and r["content"]["count"] >= 1
    assert daemon.daemon.engine.exposure.state("w-q").exposed                   # page titles are web content
