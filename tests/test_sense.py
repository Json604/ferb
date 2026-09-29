"""Build step 10, offline: the trail pipeline (exclusions before any write, bus or tag; actor on every row; redaction),
attribution (FERB's app switches, file writes, shell commands, tabs, identity profile), safety test 18 (FERB's
activity never becomes resolver history), append-only rows, the retention purge that refuses unsummarized days,
`ferb pause` (every watcher stopped, nothing recorded, owner tabs forgotten, no real input, survives a restart), and
the watchers' own logic on fake inputs (folders, file events, idle, titles, app installs).
"""

import asyncio
import json
import plistlib
import sqlite3
from datetime import date, timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.config import Config
from ferb.core.errors import Refused
from ferb.sense.exclusions import Exclusions
from ferb.sense.manager import Sense
from ferb.sense.trail import SUMMARY_KIND, Attribution, Observation
from ferb.sense.watchers.app_installs import AppInstallsWatcher
from ferb.sense.watchers.base import CREATED, MODIFIED, REMOVED, Focused
from ferb.sense.watchers.files import FilesWatcher, valid_folders
from ferb.sense.watchers.idle import IdleWatcher
from ferb.sense.watchers.windows import WindowTitleWatcher
from tests.conftest import RunningDaemon
from tests.test_ext_bridge import bd, pair  # noqa: F401  (fixture)


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


def test_titles_and_urls_are_redacted_before_they_are_stored(daemon: RunningDaemon) -> None:
    record(daemon, Observation("window", {"app": "Terminal", "bundle_id": "com.apple.Terminal",
                                          "title": "export KEY=sk-ant-abcdefghijklmnopqrstuvwxyz0123"}))
    record(daemon, Observation("browser_tab", {"browser": "chrome", "profile": "Default", "host": "x.example",
                                               "url": "https://x.example/cb?code=SECRETCODE123&x=1", "title": "t"}))
    stored = json.dumps([r["data"] for r in rows(daemon)])
    assert "sk-ant-abcdefghijklmnopqrstuvwxyz0123" not in stored and "SECRETCODE123" not in stored
    assert "[redacted]" in stored and "x=1" in stored


# ---------- attribution ----------

def test_attribution_of_app_switches_files_shell_and_tabs() -> None:
    clock = [0.0]
    a = Attribution(now=lambda: clock[0])
    focus = Observation("app_focus", {"bundle_id": "com.apple.Notes"})
    assert a.decide(focus) == ("owner", None)
    a.ferb_activated("com.apple.Notes", "task-1")
    assert a.decide(focus) == ("ferb", "task-1")                       # FERB switched to it
    clock[0] = 10.0
    assert a.decide(focus) == ("owner", None)                          # later switches are the owner's
    a.ferb_touched("/Users/x/notes/todo.md", "task-2")
    assert a.decide(Observation("file", {"path": "/Users/x/notes/todo.md"})) == ("ferb", "task-2")
    a.shell_cwds = lambda: [("/Users/x/proj", "task-3")]
    assert a.decide(Observation("file", {"path": "/Users/x/proj/build/out.o"})) == ("unknown", None)
    assert a.decide(Observation("file", {"path": "/Users/x/other.txt"})) == ("owner", None)
    tab = Observation("browser_tab", {"host": "a.example"})
    assert a.decide(Observation("browser_tab", {}, {"ferb_identity": True})) == ("ferb", None)
    assert a.decide(Observation("browser_tab", {}, {"tab_task": "task-4"})) == ("ferb", "task-4")
    assert a.decide(tab) == ("owner", None)
    assert a.decide(Observation("idle", {"state": "idle"}, {"actor": "owner"})) == ("owner", None)


def test_ferbs_activity_never_becomes_resolver_history(daemon: RunningDaemon) -> None:
    """Safety test 18: only the owner's own visits count for resolver rule 2."""
    record(daemon, Observation("browser_tab", {"browser": "chrome", "profile": "Profile 1", "host": "docs.example",
                                               "url": "https://docs.example/", "title": "d"}))
    record(daemon, Observation("browser_tab", {"browser": "brave", "profile": "Default", "host": "docs.example",
                                               "url": "https://docs.example/x", "title": "d"}, {"tab_task": "t-9"}))
    record(daemon, Observation("browser_tab", {"browser": "chrome", "profile": "Profile 22", "host": "docs.example",
                                               "url": "https://docs.example/y", "title": "d"}, {"ferb_identity": True}))
    trail = daemon.daemon.trail
    assert trail is not None
    assert daemon.run(_async(lambda: trail.owner_site_profile("docs.example"))) == ("chrome", "Profile 1")
    assert daemon.run(_async(lambda: trail.owner_site_profile("other.example"))) is None


def test_resolver_rule_2_uses_the_owners_last_profile_for_a_site(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    pair(d, launched, "Personal")
    pair(d, launched, "Work")
    resolver = d.daemon.router.resolver
    before = d.run(_async(lambda: resolver.choose("docs.example")))
    assert "last used" not in before.reason                                # nothing in the trail yet
    record(d, Observation("browser_tab", {"browser": "chrome", "profile": "Default", "host": "docs.example",
                                          "url": "https://docs.example/", "title": "d"}))
    c = d.run(_async(lambda: resolver.choose("docs.example")))
    assert (c.browser, c.folder) == ("chrome", "Default") and "last used" in c.reason


async def _async(fn: Any) -> Any:
    return fn()


# ---------- tabs from the extension ----------

def test_tab_events_become_trail_rows_with_the_pairing_profile(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.tab_event("updated", 7, "https://mail.example/inbox", "Inbox")
    ext.tab_event("updated", 8, "https://priv.example/", "Private", incognito=True)
    ext.tab_event("created", 9, "https://new.example/")                         # only loads and switches count
    ext.wait_for(lambda: len(rows(d)) >= 1)
    import time
    time.sleep(0.3)
    r = rows(d)
    assert [x["data"]["host"] for x in r] == ["mail.example"]
    assert r[0]["actor"] == "owner" and r[0]["data"]["browser"] == "chrome" and r[0]["data"]["profile"] == "Profile 1"


# ---------- retention ----------

def test_purge_refuses_days_without_a_permanent_summary(daemon: RunningDaemon) -> None:
    d = daemon.daemon

    async def seed() -> None:
        for day in ("2026-07-01", "2026-07-02", "2026-09-28"):
            d.db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, source, data_json) "   # type: ignore[union-attr]
                              "VALUES (?, ?, ?, 'dev', 'owner', 'idle', '{}')", (f"e-{day}", day + "T10:00:00+00:00", day))
    daemon.run(seed())
    trail = d.trail
    assert trail is not None
    today = date(2026, 9, 29)
    purged, refused = daemon.run(_async(lambda: trail.purge(30, today)))
    assert purged == [] and refused == ["2026-07-01", "2026-07-02"]      # nothing summarized: nothing deleted
    daemon.run(_async(lambda: d.journal.append(SUMMARY_KIND, {"day": "2026-07-01", "summary": "..."})))  # type: ignore[union-attr]
    purged, refused = daemon.run(_async(lambda: trail.purge(30, today)))
    assert purged == ["2026-07-01"] and refused == ["2026-07-02"]
    assert sorted(r["day"] for r in rows(daemon)) == ["2026-07-02", "2026-09-28"]   # recent days always kept
    assert today - timedelta(days=30) > date(2026, 7, 2)


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

def test_only_valid_owner_folders_are_watched(tmp_path: Path) -> None:
    home = tmp_path / "home"
    (home / "notes").mkdir(parents=True)
    (home / "secret").mkdir()
    (home / "a.txt").write_text("x")
    ex = Exclusions.build(paths=[str(home / "secret")])
    ok, problems = valid_folders([str(home / "notes"), str(home / "secret"), str(home / "missing"),
                                  str(home / "a.txt"), str(tmp_path)], ex, home)
    assert ok == [(home / "notes").resolve()]
    assert len(problems) == 4 and any("excluded" in p for p in problems) and any("home" in p for p in problems)


def test_file_events_are_metadata_coalesced_and_noise_free(tmp_path: Path) -> None:
    t = [0.0]
    w = FilesWatcher(lambda: [], now=lambda: t[0])
    f = tmp_path / "todo.md"
    f.write_text("secret contents")
    o = w.observe(str(f), CREATED)
    assert o is not None and o.data == {"path": str(f), "change": "created", "is_dir": False}
    assert "secret contents" not in json.dumps(o.data)
    t[0] = 2.0
    assert w.observe(str(f), MODIFIED).data["change"] == "modified"    # type: ignore[union-attr]
    assert w.observe(str(f), MODIFIED) is None                         # coalesced within a second
    t[0] = 4.0
    assert w.observe(str(f), CREATED | MODIFIED).data["change"] == "modified"   # type: ignore[union-attr]  # cumulative flags
    f.unlink()
    assert w.observe(str(f), REMOVED).data["change"] == "removed"      # type: ignore[union-attr]
    assert w.observe(str(tmp_path / ".DS_Store"), CREATED) is None
    assert w.observe(str(tmp_path / ".git" / "index"), MODIFIED) is None


def test_idle_records_transitions_only_and_nothing_without_the_tap() -> None:
    now, last, tap, out = [1000.0], [None], [True], []
    w = IdleWatcher(lambda: last[0], lambda: tap[0], idle_after_s=120, now=lambda: now[0])
    w.check(out.append)
    last[0] = 990.0                                                     # type: ignore[assignment]
    w.check(out.append)
    w.check(out.append)
    now[0] = 1200.0
    w.check(out.append)
    assert [o.data["state"] for o in out] == ["idle", "active", "idle"]
    assert all(o.hints["actor"] == "owner" for o in out)
    tap[0] = False
    w.state = None
    w.check(out.append)
    assert len(out) == 3                                                # can't tell: nothing


def test_window_titles_skip_browsers_and_follow_the_switch() -> None:
    on, reads, out = [True], [(Focused(1, "com.brave.Browser", "Brave"), "Bank - Brave"),
                              (Focused(2, "com.apple.Notes", "Notes"), "Groceries")], []
    w = WindowTitleWatcher(lambda: on[0], interval_s=0, read=lambda: reads.pop(0) if reads else None)

    async def go() -> None:
        stop = asyncio.Event()
        loop = asyncio.get_running_loop()
        loop.call_later(0.1, stop.set)
        await w.run(out.append, stop)
    asyncio.run(go())
    assert [o.data["title"] for o in out] == ["Groceries"]
    on[0], reads[:] = False, [(Focused(2, "com.apple.Notes", "Notes"), "Other")]
    out.clear()
    asyncio.run(go())
    assert out == []


def test_an_installed_app_updates_the_device_profile_and_bus_first(tmp_path: Path) -> None:
    apps = tmp_path / "Applications"
    app = apps / "Thing.app" / "Contents"
    app.mkdir(parents=True)
    (app / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "com.example.thing",
                                                     "CFBundleName": "Thing"}))
    changes, emitted = [], []
    w = AppInstallsWatcher(lambda c, info, p: changes.append((c, info.bundle_id if info else None)), folders=[apps])
    assert w.observe(str(apps / "Thing.app" / "Contents" / "Info.plist"), CREATED) is None   # inside a bundle
    o = w.observe(str(apps / "Thing.app"), CREATED)
    assert o is not None
    w.deliver(emitted.append, o)
    assert changes == [("installed", "com.example.thing")] and emitted[0].data["bundle_id"] == "com.example.thing"
    assert o.hints["actor"] == "unknown"


def test_defaults_are_written_once_and_owner_values_stay(tmp_path: Path) -> None:
    c = Config(tmp_path / "h")
    c.ensure_privacy_defaults()
    assert c.get("sense", "window_titles") is True and c.get("sense", "watch_folders") == []
    assert c.get("sense", "trail_retention_days") == 30 and "~/.ssh" in c.get("privacy", "exclude_paths")
    c.set("sense", "window_titles", False)
    c.set("privacy", "exclude_paths", [])
    c.ensure_privacy_defaults()
    assert c.get("sense", "window_titles") is False and c.get("privacy", "exclude_paths") == []


def test_a_polling_watcher_survives_a_bad_read_but_not_a_broken_source() -> None:
    from ferb.sense.watchers.base import MAX_FAILED_READS, poll
    calls = [0]

    def flaky() -> None:
        calls[0] += 1
        if calls[0] in (2, 3):
            raise RuntimeError("an app answered badly")

    async def go(step: Any) -> None:
        stop = asyncio.Event()
        asyncio.get_running_loop().call_later(0.2, stop.set)
        await poll(0.0, stop, step)
    asyncio.run(go(flaky))
    assert calls[0] > 3                                                  # kept going after the bad reads

    def broken() -> None:
        raise RuntimeError("gone")
    with pytest.raises(RuntimeError):
        asyncio.run(go(broken))
    assert MAX_FAILED_READS == 20
