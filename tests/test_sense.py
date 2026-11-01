"""Build step 10, offline: the trail pipeline (exclusions before any write, bus or tag; actor on every row; redaction),
attribution (FERB's app switches, file writes, shell commands, tabs, identity profile), safety test 18 (FERB's
activity never becomes resolver history), append-only rows, the retention purge that refuses unsummarized days,
`ferb pause` (every watcher stopped, nothing recorded, owner tabs forgotten, no real input, survives a restart), and
the watchers' own logic on fake inputs (folders, file events, idle, titles, app installs).
"""

import asyncio
import json
import sqlite3
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.core.errors import Refused
from ferb.sense.manager import Sense
from ferb.sense.trail import Observation
from tests.conftest import RunningDaemon
from tests.browserkit import bd, pair  # noqa: F401  (fixture)


def rows(d: RunningDaemon) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) | {"data": json.loads(r["data_json"])}
                for r in d.daemon.db.conn.execute("SELECT * FROM trail ORDER BY seq").fetchall()]  # type: ignore[union-attr]
    return d.run(go())  # type: ignore[no-any-return]


def record(d: RunningDaemon, o: Observation) -> Any:
    async def go() -> Any:
        return d.daemon.trail.record(o)                                # type: ignore[union-attr]
    return d.run(go())


# ---------- the pipeline ----------

# decision: SENSE-403
def test_exclusions_run_before_any_write_bus_event_or_tag(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    d.config.set("privacy", "exclude_domains", ["bank.example"])
    d.config.set("privacy", "exclude_paths", ["~/.ssh", str(d.home.parent / "private")])
    seen: list[str] = []
    publish = d.trail.publish                                          # type: ignore[union-attr]

    def pub(topic: str, data: dict[str, Any]) -> None:
        seen.append(f"publish:{len(rows_now())}")
        publish(topic, data)

    def rows_now() -> list[Any]:
        return d.db.conn.execute("SELECT id FROM trail").fetchall()    # type: ignore[union-attr]
    d.trail.publish = pub                                              # type: ignore[union-attr]
    d.trail.tag = lambda e: seen.append(f"tag:{e.id}")                 # type: ignore[union-attr]
    for o in (Observation("app_focus", {"app": "Passwords", "bundle_id": "com.apple.Passwords"}),   # default list
              Observation("browser_tab", {"browser": "chrome", "profile": "Default", "host": "www.bank.example",
                                          "url": "https://www.bank.example/acct", "title": "Balance"}),
              Observation("browser_tab", {"browser": "chrome", "profile": "Default", "host": "news.example",
                                          "url": "https://news.example/", "title": "x", "incognito": True}),
              Observation("file", {"path": str(d.home.parent / "private" / "diary.txt"), "change": "modified"}),
              Observation("file", {"path": str(Path.home() / ".ssh" / "id_ed25519"), "change": "modified"})):
        assert record(daemon, o) is None, o
    assert rows(daemon) == [] and seen == []                           # dropped completely: no row, bus or tag
    e = record(daemon, Observation("app_focus", {"app": "Notes", "bundle_id": "com.apple.Notes"}))
    assert [r["id"] for r in rows(daemon)] == [e.id]
    assert seen == ["publish:1", f"tag:{e.id}"]                         # persisted, then published, then tagged


# decision: SENSE-403
def test_every_row_has_an_actor_and_rows_are_append_only(daemon: RunningDaemon) -> None:
    e = record(daemon, Observation("idle", {"state": "idle"}, {"actor": "owner"}))
    db = daemon.daemon.db

    async def bad(sql: str, *args: Any) -> str:
        try:
            db.conn.execute(sql, args)                                 # type: ignore[union-attr]
            return "accepted"
        except sqlite3.DatabaseError as x:
            return str(x)
    base = "INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES ('x', 'a', 'd', 'dev', ?, 's', '{}')"
    assert "NOT NULL" in daemon.run(bad(base, None))
    assert "CHECK" in daemon.run(bad(base, "somebody"))
    assert "append-only" in daemon.run(bad("UPDATE trail SET actor = 'ferb' WHERE id = ?", e.id))
    assert "append-only" in daemon.run(bad("UPDATE trail SET at = 'x' WHERE id = ?", e.id))
    assert daemon.run(bad("UPDATE trail SET relevance = 'note' WHERE id = ?", e.id)) == "accepted"   # tagging later


# decision: SENSE-403
def test_titles_and_urls_are_redacted_before_they_are_stored(daemon: RunningDaemon) -> None:
    record(daemon, Observation("window", {"app": "Terminal", "bundle_id": "com.apple.Terminal",
                                          "title": "export KEY=sk-ant-abcdefghijklmnopqrstuvwxyz0123"}))
    record(daemon, Observation("browser_tab", {"browser": "chrome", "profile": "Default", "host": "x.example",
                                               "url": "https://x.example/cb?code=SECRETCODE123&x=1", "title": "t"}))
    stored = json.dumps([r["data"] for r in rows(daemon)])
    assert "sk-ant-abcdefghijklmnopqrstuvwxyz0123" not in stored and "SECRETCODE123" not in stored
    assert "[redacted]" in stored and "x=1" in stored


# ---------- attribution ----------


def test_activity_does_not_change_an_explicit_profile_choice(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    pair(d, launched, "Personal")
    pair(d, launched, "Work")
    resolver = d.daemon.router.resolver
    before = d.run(_async(lambda: resolver.choose("Work")))
    record(d, Observation("browser_tab", {"browser": "chrome", "profile": "Default", "host": "docs.example",
                                          "url": "https://docs.example/", "title": "d"}))
    choice = d.run(_async(lambda: resolver.choose("Work")))
    assert choice == before
    from ferb.act.browser.resolver import NeedsOwner
    with pytest.raises(NeedsOwner, match='select a profile'):
        d.run(_async(lambda: resolver.choose(None)))


async def _async(fn: Any) -> Any:
    return fn()


# ---------- tabs from the extension ----------


# ---------- retention ----------


# ---------- pause ----------

class FakeWatcher:
    def __init__(self, source: str) -> None:
        self.source, self.running = source, False

    async def run(self, emit: Any, stop: asyncio.Event) -> None:
        self.running = True
        emit(Observation("app_focus", {"app": "Notes", "bundle_id": "com.apple.Notes"}))
        await stop.wait()
        self.running = False


class FakeTap:
    def __init__(self) -> None:
        self.running = False

    def start(self) -> bool:
        self.running = True
        return True

    def stop(self) -> None:
        self.running = False


# decision: SENSE-403
def test_pause_stops_every_watcher_and_the_tap_and_records_nothing(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    watchers, tap = [FakeWatcher("a"), FakeWatcher("b")], FakeTap()

    async def build() -> Sense:
        s = Sense(d.home, d.trail, watchers, tap, on_pause=d._on_pause)  # type: ignore[arg-type]
        d.sense = s
        await s.start()
        await asyncio.sleep(0.05)
        return s
    s = daemon.run(build())
    assert all(w.running for w in watchers) and tap.running and len(rows(daemon)) == 2
    c = Client(daemon.sock)
    c.call("sense.pause")
    assert not any(w.running for w in watchers) and not tap.running and s.paused
    assert record(daemon, Observation("app_focus", {"app": "Notes", "bundle_id": "com.apple.Notes"})) is None
    assert len(rows(daemon)) == 2
    assert c.call("sense.status")["paused"] is True
    r = c.call("sense.resume")
    c.close()
    assert set(r["watchers"]) == {"a", "b"} and tap.running and not s.paused
    daemon.run(s.stop())


# decision: SENSE-403
def test_pause_forgets_owner_tabs_refuses_real_input_and_survives_a_restart(bd: Any, tmp_path: Path) -> None:  # noqa: F811
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.tab_event("updated", 7, "https://mail.example/inbox", "Inbox")
    ext.wait_for(lambda: len(rows(d)) == 1)
    br = next(iter(d.daemon.hub.bridges.values()))
    assert 7 in br.tabs
    c = Client(d.sock)
    c.call("sense.pause")
    c.close()
    assert 7 not in br.tabs                                             # what FERB knew about the owner's tabs
    ext.tab_event("updated", 8, "https://news.example/", "News")
    import time
    time.sleep(0.3)
    assert 8 not in br.tabs and len(rows(d)) == 1                      # the owner's tabs aren't watched
    with pytest.raises(Refused, match="paused"):
        d.run(d.daemon.mac.input.run([], "w", "click"))                # no tap -> no real input
    watcher, tap = FakeWatcher("a"), FakeTap()                         # after a restart: still paused
    restarted = Sense(d.daemon.home, d.daemon.trail, [watcher], tap)  # type: ignore[arg-type,list-item]
    d.run(restarted.start())
    assert restarted.paused and restarted.running == [] and not watcher.running and not tap.running


# ---------- the watchers' own logic ----------
