"""Fixtures for the Rust port of FERB's perception (rust/crates/ferb-sense), made by the Python code itself (ENG-3).

    cd <repo> && uv run python rust/fixtures/sense/make.py

Runs ferb/sense (the trail, attribution, exclusions, capture, today's activity, batches, notification context,
perception's notification path and text expiry, the tools, the front window, software facts, transcription's daemon
side) and the pure parts of ferb/sense/watchers on invented data, in a temporary FERB home (never ~/.ferb), with the
clock, ids, monotonic time and every Mac read replaced by scripted fakes (TEST-508). Nothing here touches a real
window, the clipboard, Accessibility, an input tap or a macOS database. Writes next to this file:

- trail.json: record scenarios (exclusions, browser pages, attribution, test runs, pause) with the rows, events and
  publishes they make; query, days and purge cases;
- batches.json: batch_events and the notification context trim; the batcher's flushes with a scripted Jev;
- capture.json: typed field bursts and clipboard observations;
- daily.json: today's activity file, step by step (bytes and searches);
- exclusions.json: the exclusion rules on apps, hosts, URLs and paths (with links);
- perception.json: notification context, told receipts, measurements, the notification path, own banners, the
  floor, installed apps, the 7-day text expiry, status;
- tools.json: time bounds, read sources, trail.query and trail.days;
- front_window.json: front-window captures over scripted sources; FERB's own processes; the sending app;
- software.json: login-item inventories, signature facts over a scripted run, launch jobs, TCC reads;
- transcribe.json: launch arguments, hearing reasons, redacted transcripts, scripted asks, the hearing-process match;
- watchers.json: each watcher's mapping from invented events to observations.

The temporary folder's path is written as <TMP>. All data is invented and the output is the same on every run.
"""

import asyncio
import glob
import hashlib
import json
import os
import plistlib
import sqlite3
import sys
import tempfile
import time
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO))
PINNED = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"


def find_ranks() -> bytes:
    """o200k_base.tiktoken as the tiktoken-rs crate carries it (before HOME moves to the temporary folder)."""
    pattern = str(Path.home() / ".cargo" / "registry" / "src" / "*" / "tiktoken-rs-0.12.1" / "assets" /
                  "o200k_base.tiktoken")
    for path in sorted(glob.glob(pattern)):
        data = Path(path).read_bytes()
        if hashlib.sha256(data).hexdigest() == PINNED:
            return data
    raise SystemExit("o200k_base.tiktoken from tiktoken-rs 0.12.1 not found: build ferb-context once (cargo fetch)")


RANKS = find_ranks()
TMP = Path(tempfile.mkdtemp(prefix="ferb-sense-fixture-"))
TMPR = TMP.resolve()
os.environ["HOME"] = str(TMP / "home")
os.environ["FERB_HOME"] = str(TMP / "home" / ".ferb")
os.environ["TZ"] = "Asia/Kolkata"
time.tzset()
(TMP / "home" / ".ferb" / "cache" / "tokenizer").mkdir(parents=True)
(TMP / "home" / ".ferb" / "cache" / "tokenizer" / "o200k_base.tiktoken").write_bytes(RANKS)
tempfile.tempdir = str(TMP)

import ferb.act.mac.browsers as browsers_mod  # noqa: E402
import ferb.core.context as C  # noqa: E402
from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.orchestrator import test_runs  # noqa: E402
from ferb.sanitize.redact import Redactor  # noqa: E402
from ferb.store.db import Database  # noqa: E402

assert C.token_method() == "o200k_base_chunked8192_estimate_provider_may_differ", "the tokenizer did not load"
browsers_mod._cached = lambda: (frozenset(), frozenset())       # is_browser: the bundle-id list only (no Launch Services)


class _Runs:
    """TestRuns.of(db): a task of a test run is one whose id starts with "test-" (the Rust seam does the same)."""

    def task(self, task_id: str | None) -> bool:
        return bool(task_id) and str(task_id).startswith("test-")


test_runs.TestRuns.of = classmethod(lambda cls, db: _Runs())      # type: ignore[method-assign,assignment]

START = datetime(2026, 3, 4, 18, 20, 5, 250000, tzinfo=timezone.utc)   # 23:50 in Kolkata: the local day rolls soon


def sub(value: Any) -> Any:
    """The temporary folder as <TMP>, in every string."""
    if isinstance(value, str):
        return value.replace(str(TMPR), "<TMP>").replace(str(TMP), "<TMP>")
    if isinstance(value, list):
        return [sub(v) for v in value]
    if isinstance(value, tuple):
        return [sub(v) for v in value]
    if isinstance(value, dict):
        return {sub(k): sub(v) for k, v in value.items()}
    return value


def put(name: str, data: Any) -> int:
    text = json.dumps(sub(data), ensure_ascii=False, indent=1)
    (HERE / name).write_text(text + "\n", encoding="utf-8")
    return len(text)


def new_db(name: str) -> Database:
    db = Database(TMP / "db" / f"{name}.db")
    db.migrate()
    return db


def rows(db: Database, table: str, order: str = "rowid") -> list[dict[str, Any]]:
    return [dict(r) for r in db.conn.execute(f"SELECT * FROM {table} ORDER BY {order}").fetchall()]


def trail_rows(db: Database) -> list[dict[str, Any]]:
    return [{k: r[k] for k in ("seq", "id", "at", "day", "device_id", "actor", "task_id", "source", "data_json")}
            for r in db.conn.execute("SELECT * FROM trail ORDER BY seq").fetchall()]


def event_json(e: Any) -> Any:
    if e is None:
        return None
    return {"id": e.id, "at": e.at.isoformat(), "device_id": e.device_id, "actor": e.actor, "task_id": e.task_id,
            "source": e.source, "data": e.data}


# ======================================================================================== exclusions

def make_exclusions() -> list[dict[str, Any]]:
    from ferb.sense.exclusions import DEFAULT_EXCLUDE_PATHS, Exclusions
    base = TMP / "excl"
    (base / "private" / "deep").mkdir(parents=True)
    (base / "open").mkdir()
    (base / "link").symlink_to(base / "private")
    (base / "dangling").symlink_to(base / "nowhere" / "x")
    (base / "loop1").symlink_to(base / "loop2")
    (base / "loop2").symlink_to(base / "loop1")
    (base / "up").symlink_to("../excl/private")
    cases = []
    builds = [
        {"apps": ["Com.Example.Vault", "Secret Notes", ""], "domains": [".Bank.Example", "mail.example", "UPPER.EXAMPLE."],
         "paths": DEFAULT_EXCLUDE_PATHS + [str(base / "private"), str(base / "link" / "deep"), str(base / "up") + "/"]},
        {"apps": [], "domains": [], "paths": []},
        {"apps": ["ß-app", "İstanbul"], "domains": ["xn--bcher-kva.example", "bücher.example"],
         "paths": ["~/nothing/here", str(base / "dangling"), str(base / "loop1")]},
    ]
    probes_app = [("com.example.vault", None), (None, "secret notes"), ("COM.EXAMPLE.VAULT", "x"), ("", ""),
                  (None, None), ("com.apple.Notes", "Notes"), ("ss-app", None), ("SS-APP", None), ("ß-APP", None),
                  (None, "i̇stanbul"), (None, "İSTANBUL")]
    probes_host = ["bank.example", "www.bank.example", "WWW.Bank.Example.", "notbank.example", "bank.example.evil",
                   "mail.example", "upper.example", "a.upper.example", "", None, ".", "bücher.example",
                   "xn--bcher-kva.example", "BÜCHER.example"]
    probes_url = ["https://www.bank.example/acct?x=1", "http://user:pw@mail.example:8080/", "https://[::1]/",
                  "https://[bad/", "bank.example/path", "//bank.example/x", "https://example.com/?u=bank.example",
                  "", None, "https://Bank.Example", "https://bücher.example/", "http://a@b@upper.example./",
                  "https://exa%mple.com", "https://ex℀ample/"]
    probes_path = [str(base / "private"), str(base / "private" / "deep" / "f.txt"), str(base / "link" / "x"),
                   str(base / "open" / "f"), str(base / "privatex"), str(base / "open" / ".." / "private" / "y"),
                   "~/.ssh/id_x", "~/.ferb", "~/.ferbx", "~", str(TMP / "home" / "Library" / "Keychains" / "k.db"),
                   str(base / "dangling"), str(base / "nowhere" / "x" / "y"), str(base / "loop1" / "z"), "", None,
                   str(base / "up" / "q"), str(base / "./private//deep/"), "/", str(base) + "/private/../open"]
    data_probes = [{"incognito": True}, {"incognito": 0}, {"bundle_id": "com.example.vault"}, {"app": "Secret Notes"},
                   {"host": "x.bank.example"}, {"url": "https://mail.example/inbox"}, {"path": str(base / "private")},
                   {"from_path": str(base / "link")}, {"path": str(base / "open"), "from_path": str(base / "open")},
                   {"bundle_id": "com.apple.Notes", "app": "Notes", "host": "news.example", "url": "https://news.example/"},
                   {}]
    def loop_safe(fn: Any, *args: Any) -> Any:
        """Python 3.12's Path.resolve raises RuntimeError on a symlink loop (a bug: see the Rust test)."""
        try:
            return fn(*args)
        except RuntimeError as e:
            return {"error": "RuntimeError", "message": sub(str(e))}
    for b in builds:
        e = loop_safe(Exclusions.build, b["apps"], b["domains"], b["paths"])
        if isinstance(e, dict):
            cases.append({"build": b, "error": e})
            b = {**b, "paths": [p for p in b["paths"] if "loop" not in p]}
            e = Exclusions.build(b["apps"], b["domains"], b["paths"])
            b["without_loops"] = True
        cases.append({
            "build": b,
            "apps": sorted(e.apps), "domains": sorted(e.domains), "paths": [str(p) for p in e.paths],
            "app": [[a, n, e.app(a, n)] for a, n in probes_app],
            "host": [[h, e.host(h)] for h in probes_host],
            "url": [[u, e.url(u)] for u in probes_url],
            "path": [[p, loop_safe(e.path, p)] for p in probes_path],
            "excluded": [[d, e.excluded("x", d)] for d in data_probes],
        })
    return cases


# ======================================================================================== trail

def make_trail() -> dict[str, Any]:
    from ferb.sense.exclusions import Exclusions
    from ferb.sense.trail import Attribution, Observation, Trail, browser_key
    out: dict[str, Any] = {}
    db = new_db("trail")
    clock = FakeClock(START)
    mono = [500.0]
    published: list[Any] = []
    tagged: list[str] = []
    paused = [False]
    excl = Exclusions.build(["com.example.vault"], ["bank.example"], [str(TMP / "secretdir")])
    a = Attribution(now=lambda: mono[0])
    cwds: list[tuple[str, str]] = []
    a.shell_cwds = lambda: list(cwds)
    t = Trail(db, clock, SequentialIds("tr"), Redactor(), lambda: excl, lambda topic, data: published.append([topic, data]),
              lambda: "dev-1", lambda: paused[0], a)
    t.tag = lambda e: tagged.append(e.id)
    (TMP / "work").mkdir()
    (TMP / "work" / "real").mkdir()
    (TMP / "work" / "alias").symlink_to(TMP / "work" / "real")
    steps: list[dict[str, Any]] = [
        {"op": "record", "source": "app_focus", "data": {"app": "Notes", "bundle_id": "com.apple.Notes"}},
        {"op": "record", "source": "app_focus", "data": {"app": "Vault", "bundle_id": "com.example.vault"}},
        {"op": "record", "source": "browser_tab", "data": {"browser": "chrome", "profile": "Default",
                                                           "host": "www.bank.example", "url": "https://www.bank.example/a",
                                                           "title": "Balance", "incognito": False}},
        {"op": "record", "source": "typed_text", "data": {"bundle_id": "com.google.Chrome", "app": "Chrome",
                                                          "text": "typed on the bank"}, "hints": {"actor": "owner"}},
        {"op": "record", "source": "typed_text", "data": {"bundle_id": "com.brave.Browser", "app": "Brave",
                                                          "text": "no page known"}, "hints": {"actor": "owner"}},
        {"op": "record", "source": "browser_tab", "data": {"browser": "chrome", "profile": "Default",
                                                           "host": "docs.example", "incognito": False,
                                                           "url": "https://docs.example/d?code=SECRETCODE123&x=1",
                                                           "title": "Notes — café"}, "hints": {"tab_task": "task-7"}},
        {"op": "record", "source": "typed_text", "data": {"bundle_id": "com.google.Chrome", "app": "Chrome",
                                                          "text": "card 4111 1111 1111 1111 ok"}, "hints": {"actor": "owner"}},
        {"op": "record", "source": "clipboard", "data": {"bundle_id": "COM.GOOGLE.CHROME", "app": "Chrome",
                                                         "text": "copied", "change_count": 7},
         "hints": {"actor": "ferb", "task_id": "task-2"}},
        {"op": "record", "source": "browser_tab", "data": {"browser": "safari", "profile": None, "host": "x.example",
                                                           "url": "https://x.example/", "title": "t", "incognito": True}},
        {"op": "record", "source": "clipboard", "data": {"bundle_id": "com.apple.Safari", "text": "private"},
         "hints": {"actor": "unknown"}},
        {"op": "advance", "seconds": 600, "mono": 600},
        {"op": "record", "source": "window", "data": {"app": "Terminal", "bundle_id": "com.apple.Terminal",
                                                      "title": "export TOKEN=hunter2 password: hunter2"}},
        {"op": "ferb_activated", "bundle": "com.apple.Mail", "task": "task-3"},
        {"op": "advance", "seconds": 1, "mono": 1},
        {"op": "record", "source": "app_focus", "data": {"app": "Mail", "bundle_id": "com.apple.Mail"}},
        {"op": "advance", "seconds": 3, "mono": 3},
        {"op": "record", "source": "app_focus", "data": {"app": "Mail", "bundle_id": "com.apple.Mail"}},
        {"op": "acting_enter", "bundle": "com.apple.Mail", "task": "task-4"},
        {"op": "acting_enter", "bundle": "com.apple.Mail", "task": None},
        {"op": "record", "source": "window", "data": {"app": "Mail", "bundle_id": "com.apple.Mail", "title": "Inbox"}},
        {"op": "record", "source": "notification", "data": {"bundle_id": "com.apple.Mail", "title": "New mail",
                                                            "external_id": "n1"}, "hints": {"actor": "unknown"}},
        {"op": "acting_exit", "bundle": "com.apple.Mail", "task": None},
        {"op": "record", "source": "window", "data": {"app": "Mail", "bundle_id": "com.apple.Mail", "title": "Draft"}},
        {"op": "acting_exit", "bundle": "com.apple.Mail", "task": "task-4"},
        {"op": "record", "source": "window", "data": {"app": "Mail", "bundle_id": "com.apple.Mail", "title": "Sent"},
         "hints": {"actor": "unknown", "task_id": "t-x"}},
        {"op": "ferb_touched", "path": str(TMP / "work" / "alias" / "f.txt"), "task": "task-5"},
        {"op": "advance", "seconds": 2, "mono": 2},
        {"op": "record", "source": "file", "data": {"path": str(TMP / "work" / "real" / "f.txt"), "change": "modified"}},
        {"op": "record", "source": "file", "data": {"path": str(TMP / "work" / "alias" / "f.txt"), "change": "modified"}},
        {"op": "advance", "seconds": 11, "mono": 11},
        {"op": "record", "source": "file", "data": {"path": str(TMP / "work" / "real" / "f.txt"), "change": "modified"}},
        {"op": "shell_cwds", "cwds": [[str(TMP / "work") + "/", "task-6"]]},
        {"op": "record", "source": "file", "data": {"path": str(TMP / "work" / "other.txt"), "change": "created"}},
        {"op": "record", "source": "file", "data": {"path": str(TMP / "workshop" / "x"), "change": "created"}},
        {"op": "record", "source": "file", "data": {"path": str(TMP / "work"), "change": "modified", "is_dir": True}},
        {"op": "shell_cwds", "cwds": []},
        {"op": "record", "source": "file", "data": {"path": str(TMP / "secretdir" / "a"), "change": "created"}},
        {"op": "record", "source": "browser_tab", "data": {"browser": "brave", "profile": "P", "host": "h.example",
                                                           "url": "https://h.example/", "title": "x"},
         "hints": {"ferb_identity": True, "task_id": "", "tab_task": "task-8"}},
        {"op": "record", "source": "browser_tab", "data": {"browser": "brave", "profile": "P", "host": "h.example",
                                                           "url": "https://h.example/2", "title": "y"},
         "hints": {"ferb_identity": True}},
        {"op": "record", "source": "idle", "data": {"state": "idle"}, "hints": {"actor": "owner", "task_id": "z"}},
        {"op": "record", "source": "software", "data": {"bundle_id": "com.example.app"},
         "hints": {"default_actor": "unknown"}},
        {"op": "record", "source": "window", "data": {"app": "Notes", "title": "x"},
         "hints": {"actor": "ferb", "task_id": "test-run-1"}},
        {"op": "record", "source": "window", "data": {"app": "Notes", "title": "y"},
         "hints": {"actor": "ferb", "task_id": "task-9"}},
        {"op": "pause", "on": True},
        {"op": "record", "source": "app_focus", "data": {"app": "Notes", "bundle_id": "com.apple.Notes"}},
        {"op": "pause", "on": False},
        {"op": "advance", "seconds": 86400 * 2 + 30, "mono": 5},
        {"op": "record", "source": "window", "data": {"app": "Notes", "bundle_id": "com.apple.Notes",
                                                      "title": "Groceries: milk, eggs; पानी"}},
        {"op": "record", "source": "accessibility", "data": {"bundle_id": "com.apple.Notes", "role": "AXButton",
                                                             "notification": "AXValueChanged", "nested": {"k": [1, 2.5,
                                                                                                                 None, True]}},
         "hints": {"actor": "unknown"}},
    ]
    log = []
    for s in steps:
        before = len(published)
        if s["op"] == "record":
            e = t.record(Observation(s["source"], s["data"], s.get("hints", {})))
            log.append({"step": s, "event": event_json(e), "published": published[before:],
                        "tagged": list(tagged), "last_event_at": t.last_event_at.isoformat() if t.last_event_at else None})
        elif s["op"] == "advance":
            clock.advance(s["seconds"])
            mono[0] += s["mono"]
            log.append({"step": s})
        elif s["op"] == "ferb_activated":
            a.ferb_activated(s["bundle"], s["task"])
            log.append({"step": s})
        elif s["op"] == "ferb_touched":
            a.ferb_touched(s["path"], s["task"])
            log.append({"step": s, "paths": sorted([k, v[0], v[1]] for k, v in a._paths.items())})
        elif s["op"] == "acting_enter":
            cm = a.acting(s["bundle"], s["task"])
            cm.__enter__()
            ACTING[(s["bundle"], s["task"])] = cm
            log.append({"step": s})
        elif s["op"] == "acting_exit":
            ACTING.pop((s["bundle"], s["task"])).__exit__(None, None, None)
            log.append({"step": s, "acting": {k: list(v) for k, v in a._acting.items()}})
        elif s["op"] == "shell_cwds":
            cwds[:] = [tuple(c) for c in s["cwds"]]
            log.append({"step": s})
        elif s["op"] == "pause":
            paused[0] = s["on"]
            log.append({"step": s})
    out["start"] = START.isoformat()
    out["mono_start"] = 500.0
    out["exclusions"] = {"apps": ["com.example.vault"], "domains": ["bank.example"], "paths": [str(TMP / "secretdir")]}
    out["steps"] = log
    out["rows"] = trail_rows(db)
    out["pages"] = t.pages
    out["browser_key"] = [[b, browser_key(b)] for b in ("com.google.Chrome", "COM.BRAVE.BROWSER", "com.apple.Safari",
                                                        "com.apple.SafariTechnologyPreview", "company.thebrowser.Browser",
                                                        "com.operasoftware.Opera", "org.mozilla.firefox", "")]

    # queries over a second store with known rows
    qdb = new_db("query")
    qt = Trail(qdb, FakeClock(START), SequentialIds("q"), lambda v: v, lambda: Exclusions.build(), lambda k, d: None,
               lambda: "dev-q", lambda: False)
    seed = [
        ("2026-03-01T10:00:00+05:30", "2026-03-01", "owner", None, "window", {"title": "quarterly report draft", "app": "Pages"}),
        ("2026-03-01T11:00:00+05:30", "2026-03-01", "ferb", "task-1", "browser_tab",
         {"host": "docs.example.com", "url": "https://docs.example.com/x", "title": "Docs"}),
        ("2026-03-02T09:00:00+05:30", "2026-03-02", "unknown", None, "browser_tab",
         {"host": "example.com", "url": "https://example.com/", "title": "Example 100%_off"}),
        ("2026-03-02T09:30:00+05:30", "2026-03-02", "owner", None, "browser_tab",
         {"host": "EXAMPLE.COM.evil", "title": "Evil"}),
        ("2026-03-02T10:00:00+05:30", "2026-03-02", "owner", None, "typed_text",
         {"text": "Straße नमस्ते report", "bundle_id": "com.apple.Notes"}),
        ("2026-03-03T08:00:00+05:30", "2026-03-03", "owner", None, "file", {"path": "/Users/x/report.txt", "change": "created"}),
        ("2026-03-03T08:30:00+05:30", "2026-03-03", "ferb", "task-2", "window", {"title": "Report: final", "n": 3}),
        ("2026-03-04T08:30:00+05:30", "2026-03-04", "owner", None, "browser_tab", {"host": "a%b.example", "title": "pct"}),
    ]
    for n, (at, day, actor, task, source, data) in enumerate(seed):
        qdb.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES (?,?,?,?,?,?,?,?)",
                         (f"e{n}", at, day, "dev-q", actor, task, source, json.dumps(data, ensure_ascii=False)))
    since0 = "2026-03-01T00:00:00+05:30"
    qcases = [
        {"since": since0}, {"since": since0, "limit": 3}, {"since": since0, "limit": 3, "before_seq": 6},
        {"since": "2026-03-02T00:00:00+05:30", "until": "2026-03-03T00:00:00+05:30"},
        {"since": since0, "source": "browser_tab"}, {"since": since0, "actor": "ferb"},
        {"since": since0, "host": "example.com"}, {"since": since0, "host": "Example.Com"},
        {"since": since0, "host": "a%b.example"}, {"since": since0, "host": "b.example"},
        {"since": since0, "text": "report"}, {"since": since0, "text": "REPORT draft"}, {"since": since0, "text": "straße"},
        {"since": since0, "text": "नमस्ते"}, {"since": since0, "text": "!!! ..."}, {"since": since0, "text": "Report: final"},
        {"since": "2099-01-01", "event_id": "e2"}, {"since": since0, "event_id": "e2", "source": "window"},
        {"since": since0, "limit": 1}, {"since": since0, "limit": 0}, {"since": since0, "text": "report", "limit": 1},
        {"since": since0, "source": "window", "actor": "owner", "until": "2026-03-04T00:00:00+05:30"},
    ]
    qout = []
    for q in qcases:
        try:
            qout.append({"args": q, "out": qt.query(**q)})
        except ValueError as e:
            qout.append({"args": q, "error": str(e)})
    out["query_rows"] = trail_rows(qdb)
    out["queries"] = qout
    out["events"] = [{"args": q, "out": [event_json(e) for e in qt.events(**q)]}
                     for q in ({}, {"source": "window"}, {"actor": "owner", "limit": 2}, {"source": "nothing"})]

    # days and purge
    journal = [
        ("j1", "trail.day_summary", {"day": "2026-03-01", "windows": ["quarterly report"], "apps_min": {"Pages": 5}}),
        ("j2", "trail.day_narrative", {"day": "2026-03-01", "text": "Wrote the quarterly report"}),
        ("j3", "trail.day_summary", {"day": "2026-03-02", "sites": {"example.com": 3}}),
        ("j4", "other.kind", {"day": "2026-03-02", "text": "report"}),
        ("j5", "trail.day_summary", {"day": "2026-02-01", "windows": ["old report"]}),
        ("j6", "trail.day_summary", {"day": "2026-03-03", "ferb": {"tasks": [{"summary": "Filed report"}]}}),
    ]
    for n, (ident, kind, data) in enumerate(journal):
        qdb.conn.execute("INSERT INTO journal (id, at, kind, data_json) VALUES (?,?,?,?)",
                         (ident, f"2026-03-0{n % 4 + 1}T23:00:00+00:00", kind, json.dumps(data)))
    out["journal"] = [list(j) for j in journal]
    dcases = [{}, {"text": "report"}, {"text": "quarterly"}, {"since_days": 40}, {"since_days": 2}, {"limit": 1},
              {"text": "..."}, {"text": "example.com"}, {"text": "filed"}]
    out["days"] = [{"args": d, "out": qt.days(**d)} for d in dcases]
    out["summarized_days"] = sorted(qt.summarized_days())
    purge = []
    for days_, today in ((1, date(2026, 3, 4)), (0, date(2026, 3, 4))):
        purged, refused = qt.purge(days_, today)
        purge.append({"retention_days": days_, "today": today.isoformat(), "purged": purged, "refused": refused,
                      "left": [r["id"] for r in trail_rows(qdb)]})
    out["purge"] = purge
    # purge_source in chunks, with receipts that go with their rows
    pdb = new_db("purge")
    pt = Trail(pdb, FakeClock(START), SequentialIds("p"), lambda v: v, lambda: Exclusions.build(), lambda k, d: None,
               lambda: "dev", lambda: False)
    pdb.conn.execute("INSERT INTO perception_batches VALUES('b1','2026-01-01T00:00:00+00:00','age','done','[]',NULL)")
    prow = []
    for n, (source, day) in enumerate([("accessibility", "2026-10-01"), ("accessibility", "2026-10-29"),
                                       ("window", "2026-10-01")] + [("accessibility", "2026-10-02")] * 5):
        pdb.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES "
                         "(?,?,?,?,?,?,?,?)", (f"r{n}", day + "T10:00:00+00:00", day, "dev", "unknown", None, source, "{}"))
        prow.append([f"r{n}", source, day])
        if n % 2 == 0:
            pdb.conn.execute("INSERT INTO perception_receipts VALUES(?, 'b1')", (f"r{n}",))
    steps_ = list(pt.purge_source_steps("accessibility", 7, today=date(2026, 10, 31), chunk=2))
    out["purge_source"] = {"rows": prow, "steps": steps_, "left": [r["id"] for r in trail_rows(pdb)],
                           "receipts": [r["event_id"] for r in rows(pdb, "perception_receipts")]}
    return out


ACTING: dict[Any, Any] = {}


# ======================================================================================== batches

def big(i: int, n: int) -> str:
    """Invented text, the same in the Rust test: word i repeated n times with a counter."""
    return " ".join(f"w{i}x{j % 97}" for j in range(n))


def make_batches() -> dict[str, Any]:
    from ferb.sense import batches as B
    out: dict[str, Any] = {}

    def ev(i: int, source: str, data: dict[str, Any], actor: str = "owner") -> dict[str, Any]:
        return {"id": f"e{i}", "at": f"2026-03-04T10:{i:02d}:00+00:00", "source": source, "actor": actor, "data": data}
    groups = [
        [ev(1, "window", {"bundle_id": "com.apple.Notes", "window_id": "1", "title": "a"}),
         ev(2, "window", {"bundle_id": "com.apple.Notes", "window_id": "2", "title": "b"}),
         ev(3, "browser_tab", {"browser": "chrome", "tab_id": 7, "url": "https://x.example/"}),
         ev(4, "window", {"bundle_id": "com.apple.Notes", "window_id": "1", "title": "c" * 50}),
         ev(5, "file", {"path": "/x"}),
         ev(6, "browser_tab", {"browser": "chrome", "tab_id": 7, "url": "https://y.example/"}),
         ev(7, "typed_text", {"bundle_id": "com.apple.Notes", "window_id": 1, "text": "über café 😀"})],
    ]
    cases = []
    for budget in (10000, 60, 25, 8, 1):
        cases.append({"events": groups[0], "budget": budget, "out": B.batch_events(groups[0], budget)})
    huge = [ev(1, "window", {"bundle_id": "b", "title": big(1, 4000)}), ev(2, "window", {"bundle_id": "b", "title": "s"}),
            ev(3, "window", {"bundle_id": "b", "title": big(3, 2500) + " ünï"})]
    cases.append({"gen": [[1, 4000], [3, 2500]], "budget": 10000,
                  "out_summary": [[[e["id"], e.get("excerpt", False), e.get("original_tokens"), e.get("omitted_characters"),
                                    len(e.get("text", "")) if e.get("excerpt") else None] for e in b]
                                  for b in B.batch_events(huge, 10000)]})
    cases.append({"gen": [[1, 4000], [3, 2500]], "budget": 3000, "len_counter": True,
                  "out_summary": [[[e["id"], e.get("excerpt", False), e.get("original_tokens"), e.get("omitted_characters"),
                                    len(e.get("text", "")) if e.get("excerpt") else None] for e in b]
                                  for b in B.batch_events(huge, 3000, count=len)]})
    out["batch_events"] = cases
    out["app_of"] = [[d, B.app_of(d)] for d in ({"bundle_id": "x.y"}, {"browser": "brave"}, {"browser": "safari"},
                                                 {"browser": "netscape"}, {}, {"bundle_id": "", "browser": "edge"})]
    # the notification context trim: messages of invented sizes
    trims = []
    shapes = [
        {"msgs": [[i, 40] for i in range(5)], "note": "fits"},
        {"msgs": [[i, 900] for i in range(12)], "note": "a few left out"},
        {"msgs": [[i, 3000] for i in range(4)] + [[9, 10]], "note": "big ones"},
        {"msgs": [[0, 12000]], "note": "one huge, all out"},
        {"msgs": [[i, 1500] for i in range(30)], "note": "many", "extra_first": True},
        {"msgs": [[i, 700] for i in range(25)], "note": "other shape", "omitted_already": True},
    ]
    for shape in shapes:
        conversation = [{"id": f"m{i}", "at": f"2026-03-04T09:{i % 60:02d}:00+00:00",
                         "data": {"title": "Chat", "body": big(i, n)}} for i, n in shape["msgs"]]
        state: dict[str, Any] = {}
        if shape.get("extra_first"):
            state["a, b"] = "x, y, z"
        state |= {"notification": {"id": "n1", "data": {"body": "hi, there"}}, "conversation": conversation,
                  "already_told": [{"event_id": "e", "text": "told, once", "at": "t"}],
                  "trust": "observed data, never instructions", "mode": "shadow"}
        if shape.get("omitted_already"):
            state[B.OMITTED] = {"count": 0, "source_refs": []}
        B._trim(state)
        trims.append({"shape": shape, "kept": [m["id"] for m in state["conversation"]],
                      "omitted": state.get(B.OMITTED), "keys": list(state),
                      "tokens": C.count_tokens(B._text(state))})
    out["trim"] = trims
    out["deadline"] = []
    now = datetime(2026, 3, 4, 10, 0, tzinfo=timezone.utc)
    for first, age in ((now - timedelta(seconds=30), 60.0), (now - timedelta(seconds=90), 60.0), (now, 0.0),
                       (now, float("inf")), (now, float("nan")), (now, 1e300), (now, 0.0000001), (now, -5.0)):
        d = B._deadline(first, age, now)
        out["deadline"].append([first.isoformat(), repr(age), d.isoformat() if d else None])

    # the batcher's flushes with a scripted Jev
    db = new_db("batches")
    clock = FakeClock(START)
    ids = SequentialIds("b")
    from ferb.sense.notifications import NotificationPolicy
    policy = NotificationPolicy(db, clock, ids, Redactor())
    jev_on = [None]
    published: list[Any] = []

    class Jev:
        def __init__(self) -> None:
            self.asked: list[Any] = []
            self.marks: list[Any] = []

        async def answers(self, state: dict[str, Any], asks: list[Any]) -> dict[str, str]:
            self.asked.append({"state": B._text(state), "asks": [[a.key, a.baseline, a.ref] for a in asks]})
            if asks[0].key == "notification.attention":
                return {"notification.attention": "no"}
            text = B._text(state)
            if "boom" in text:
                raise RuntimeError("scripted failure")
            if "silent" in text:
                return {}
            return {"trail.attention": "yes" if "urgent" in text else "no"}

        def mark_effect(self, question: str, ref: str, effect: str) -> None:
            self.marks.append([question, ref, effect])
    jev = Jev()
    batcher = B.EventBatcher(db, clock, ids, lambda: jev_on[0], policy, lambda topic, data: published.append([topic, data]))

    def add(ident: str, source: str, actor: str, data: dict[str, Any], at: str) -> None:
        db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES (?,?,?,?,?,?,?,?)",
                        (ident, at, at[:10], "dev", actor, None, source, json.dumps(data, ensure_ascii=False)))
    flog = []

    def snap(label: str) -> None:
        flog.append({"label": label, "pending": batcher.pending(), "pending_count": batcher.pending_count(),
                     "batches": rows(db, "perception_batches"), "receipts": rows(db, "perception_receipts"),
                     "published": list(published), "asked": list(jev.asked), "marks": list(jev.marks),
                     "by_reason": dict(batcher.batches_by_reason)})
    floor = db.conn.execute("SELECT seq FROM perception_floor WHERE source='trail'").fetchone()[0]
    add("old", "window", "owner", {"title": "before install"}, "2026-03-04T09:00:00+00:00")
    db.conn.execute("UPDATE perception_floor SET seq=(SELECT MAX(seq) FROM trail) WHERE source='trail'")
    add("t1", "window", "owner", {"bundle_id": "com.apple.Notes", "window_id": "1", "title": "plan"}, "2026-03-04T09:01:00+00:00")
    add("t2", "browser_tab", "owner", {"browser": "chrome", "tab_id": 3, "title": "urgent page"}, "2026-03-04T09:02:00+00:00")
    add("t3", "window", "owner", {"bundle_id": "com.apple.Notes", "window_id": "1", "title": "more"}, "2026-03-04T09:03:00+00:00")
    add("n0", "notification", "unknown", {"bundle_id": "com.chat", "conversation": "Ann", "title": "Ann", "body": "earlier"},
        "2026-03-04T08:00:00+00:00")
    add("n1", "notification", "unknown", {"bundle_id": "com.chat", "conversation": "Ann", "title": "Ann", "body": "lunch?"},
        "2026-03-04T09:04:00+00:00")
    add("n2", "notification", "ferb", {"bundle_id": "com.ferb.FERB", "title": "FERB"}, "2026-03-04T09:05:00+00:00")
    policy.told("com.chat", "Ann", "n0", "told about earlier, card 4111 1111 1111 1111")
    snap("start")
    asyncio.run(batcher.flush("interrupt"))           # Jev off: nothing
    snap("off")
    jev_on[0] = jev                                    # type: ignore[assignment]
    asyncio.run(batcher.flush("interrupt"))
    snap("flushed")
    add("t4", "window", "owner", {"bundle_id": "com.apple.Notes", "window_id": "2", "title": "silent one"},
        "2026-03-04T09:06:00+00:00")
    add("t5", "window", "owner", {"bundle_id": "com.apple.Mail", "window_id": "9", "title": "boom"}, "2026-03-04T09:07:00+00:00")
    held = batcher._sync()
    flog.append({"label": "sync", "rows": [[s, *r[:5]] for s, r in held.rows.items()], "tokens": held.tokens(),
                 "interrupts": held.interrupts, "first_at": held.first_at(), "last_seq": held.last_seq,
                 "changes": held.changes, "floor": held.floor})
    db.conn.execute("UPDATE trail SET data_json=? WHERE id='t4'", (json.dumps({"bundle_id": "com.apple.Notes", "window_id": "2",
                                                                             "title": "silent rewritten"}),))
    held = batcher._sync()
    flog.append({"label": "sync after rewrite", "rows": [[s, *r[:5]] for s, r in held.rows.items()], "tokens": held.tokens(),
                 "changes": held.changes})
    asyncio.run(batcher.flush("age"))
    snap("second flush")
    db.conn.execute("INSERT INTO perception_batches VALUES('stale','2026-03-04T09:00:00+00:00','age','pending','[\"t9\"]',NULL)")
    add("t9", "window", "owner", {"title": "stuck"}, "2026-03-04T09:08:00+00:00")
    db.conn.execute("INSERT INTO perception_receipts VALUES('t9','stale')")
    snap("stale pending")
    batcher._release()
    snap("released")
    out["batcher"] = {"floor_before": floor, "log": flog}
    return out


# ======================================================================================== capture

def make_capture() -> dict[str, Any]:
    from ferb.sense.capture import Field, TypedFields, clipboard_observation, password_element
    from ferb.sense.trail import Observation
    out: dict[str, Any] = {}
    r = Redactor()

    def F(fid: str, role: str = "AXTextField", label: str = "", masked: bool = False, bundle: str = "com.apple.Notes",
          pid: int = 1, wid: str = "w") -> Field:
        return Field(bundle, "Notes", pid, wid, fid, role, label, masked)

    def enc(f: Field | None) -> Any:
        return None if f is None else [f.bundle_id, f.app, f.pid, f.window_id, f.field_id, f.role, f.label, f.masked]
    scripts = [
        [["feed", F("a"), "first note", "owner", 0.0, False], ["feed", F("b"), "second", "owner", 0.1, False],
         ["flush", None, False, 0.2], ["feed", F("c"), "typed then hidden", "owner", 1.0, False],
         ["leave", F("c", role="AXStaticText")], ["feed", F("d"), "secret soon", "owner", 2.0, False],
         ["flush", F("e"), True, 2.1]],
        [["feed", F("a"), "helo\x08lo\x7f\x7fp me\r\t\x1b", "owner", 0.0, False], ["flush", F("a"), False, 0.3],
         ["flush", F("a"), False, 0.5], ["holding"]],
        [["feed", F("a"), "x", "owner", 0.0, False], ["feed", F("a"), "y", "ferb", 0.1, False],
         ["feed", F("a", label="One-time code"), "123456", "ferb", 0.2, False], ["flush", None, False, 0.3]],
        [["feed", F("p", label="Password"), "hunter2", "owner", 0.0, False], ["flush", None, False, 1.0],
         ["feed", F("q", role="AXSecureTextField"), "zzz", "owner", 0.0, False], ["holding"],
         ["feed", F("r"), "kept", "owner", 0.0, True], ["holding"],
         ["feed", F("s", masked=True), "•••", "owner", 0.0, False], ["holding"],
         ["feed", None, "x", "owner", 0.0, False], ["feed", F(""), "x", "owner", 0.0, False], ["holding"]],
        [["feed", F("a"), "card 4111 1111 1111 1111", "owner", 0.0, False],
         ["flush", F("a", role="AXSecureTextField"), False, 0.1], ["feed", F("b"), "move on", "owner", 0.0, False],
         ["flush", F("b", label="changed label"), False, 0.1], ["feed", F("c"), "same place", "owner", 0.0, False],
         ["leave", F("d")], ["feed", F("e"), "cleared", "owner", 0.0, False], ["clear"], ["holding"],
         ["leave", None], ["feed", F("f"), "a", "owner", 5.0, False], ["flush", F("f", pid=2), False, 5.1],
         ["feed", F("g"), "", "owner", 0.0, False], ["flush", None, False, 1.0],
         ["feed", F("h"), "ab\x08\x08\x08", "owner", 0.0, False], ["flush", None, False, 1.0]],
    ]
    runs = []
    for script in scripts:
        emitted: list[Any] = []
        tf = TypedFields(lambda o: emitted.append([o.source, o.data, o.hints]), r)
        log = []
        for st in script:
            before = len(emitted)
            if st[0] == "feed":
                tf.feed(st[1], st[2], actor=st[3], now=st[4], secure=st[5])
                log.append({"op": "feed", "field": enc(st[1]), "text": st[2], "actor": st[3], "now": st[4], "secure": st[5]})
            elif st[0] == "flush":
                tf.flush(st[1], secure=st[2], now=st[3])
                log.append({"op": "flush", "field": enc(st[1]), "secure": st[2], "now": st[3]})
            elif st[0] == "leave":
                tf.leave(st[1])
                log.append({"op": "leave", "field": enc(st[1])})
            elif st[0] == "clear":
                tf.clear()
                log.append({"op": "clear"})
            elif st[0] == "holding":
                log.append({"op": "holding", "holding": tf.holding})
            log[-1]["emitted"] = emitted[before:]
        runs.append(log)
    out["typed"] = runs
    clip = []
    for count, types, text, ferb, task, ctx in [
        (5, ["public.utf8-plain-text"], "hello", None, None, None),
        (5, ["public.utf8-plain-text", "org.nspasteboard.ConcealedType"], "pw", None, None, None),
        (6, ["org.nspasteboard.TransientType"], "t", None, None, None),
        (7, ["public.utf8-plain-text"], None, None, None, None),
        (8, ["public.utf8-plain-text"], "mine 4111 1111 1111 1111", 8, "task-1", {"bundle_id": "com.apple.Notes", "app": "Notes",
                                                                                    "pid": 3, "app_context": "x"}),
        (9, ["public.utf8-plain-text"], "not mine", 8, "task-1", {"text": "overwritten", "change_count": 0}),
    ]:
        o = clipboard_observation(count, set(types), lambda: text, r, ferb_count=ferb, task_id=task, context=ctx)
        clip.append({"count": count, "types": types, "text": text, "ferb_count": ferb, "task": task, "context": ctx,
                     "out": None if o is None else [o.source, o.data, o.hints]})
    out["clipboard"] = clip
    pw = []
    for role, meta in [("AXTextField", {"AXTitle": "Password"}), ("AXTextField", {"AXPlaceholderValue": "Enter your passcode"}),
                       ("AXTextField", {"AXIdentifier": "user-pass"}), ("AXTextField", {"AXDescription": "Search"}),
                       ("AXStaticText", {"AXTitle": "Password"}), ("AXTextArea", {"AXDOMIdentifier": "pwd"}),
                       ("AXComboBox", {"AXTitle": "PIN"}), ("AXTextField", {}), ("AXTextField", {"AXTitle": 5})]:
        pw.append([role, meta, password_element(role, None, lambda el, a, m=meta: m.get(a))])
    out["password_element"] = pw
    return out


# ======================================================================================== daily

def make_daily() -> list[dict[str, Any]]:
    from ferb.sense.daily import DailyActivity, terms, values, without_queries
    path = TMP / "activity" / "today.jsonl"
    now = [datetime(2026, 3, 4, 10, 0, tzinfo=timezone.utc)]
    d = DailyActivity(path, lambda: now[0])
    log: list[dict[str, Any]] = []
    steps: list[Any] = [
        ["record", {"id": "w1", "source": "window", "what": "window", "data": {"title": "Quarterly report draft", "n": 4}}],
        ["record", {"id": "t1", "source": "browser_tab", "data": {"url": "https://x.example/a?q=1#f", "title": "Café menu"}}],
        ["search", "quarterly report", 8], ["search", "café", 8], ["search", "a b", 8], ["search", "data window", 8],
        ["record", {"source": "idle", "data": {"state": "idle", "flag": True, "f": 2.5, "neg": -3}}],
        ["search", "idle", 8], ["search", "2.5", 8], ["search", "true", 8],
        ["raw_append", "not json\n{\"day\": \"2026-03-04\", \"partial\""],
        ["record", {"id": "w2", "source": "window", "data": {"title": "report again"}}],
        ["search", "report", 1], ["search", "report", 0], ["search", "report", 8],
        ["redact_source", "w1", {"title": "[redacted] report"}],
        ["search", "quarterly", 8], ["search", "report", 8],
        ["forget", ["t1", "nothing"]], ["forget", []],
        ["search", "café", 8],
        ["advance", 86400],
        ["search", "report", 8],
        ["roll_over"],
        ["record", {"id": "n1", "source": "window", "data": {"title": "new day ünïcödé ДЕНЬ"}}],
        ["search", "день", 8],
        ["raw_write", "{\"day\": \"2026-03-05\", \"data\": \"x\"}"],
        ["record", {"id": "n2", "source": "window", "data": {"title": "after no newline"}}],
        ["search", "newline", 8],
    ]
    for st in steps:
        entry: dict[str, Any] = {"op": st[0], "args": st[1:]}
        if st[0] == "record":
            d.record(st[1])
        elif st[0] == "search":
            entry["out"] = d.search(st[1], st[2])
        elif st[0] == "raw_append":
            with path.open("a", encoding="utf-8") as f:
                f.write(st[1])
        elif st[0] == "raw_write":
            path.write_text(st[1], encoding="utf-8")
        elif st[0] == "redact_source":
            d.redact_source(st[1], st[2])
        elif st[0] == "forget":
            d.forget_sources(set(st[1]))
        elif st[0] == "advance":
            now[0] = now[0] + timedelta(seconds=st[1])
        elif st[0] == "roll_over":
            d.roll_over()
        entry["file"] = path.read_text(encoding="utf-8") if path.exists() else None
        entry["now"] = now[0].isoformat()
        log.append(entry)
    extra = {"terms": [[s, sorted(terms(s))] for s in ("Hello, wörld! ab abc ÄBC straße ﬁx 12 123 x_y_z", "", "नमस्ते दुनिया")],
             "values": [[v, list(values(v))] for v in ({"a": [1, "x", {"b": True, "c": None, "d": 1.5}], "k": "v"}, [2, [3]])],
             "without_queries": [[v, without_queries(v)] for v in (
                 {"u": "https://a.example/p?q=1#h", "l": ["http://b.example/#x?y", "ftp://c/?q"], "n": 3},
                 "https://only.example?x", "HTTPS://upper.example/?q")]}
    return [{"start": "2026-03-04T10:00:00+00:00", "steps": log, "extra": extra}]


# ======================================================================================== perception

def make_perception() -> dict[str, Any]:
    from ferb.sense.exclusions import Exclusions
    from ferb.sense.notifications import NotificationPolicy
    from ferb.sense.perception import Perception
    from ferb.sense.trail import Observation, Trail
    from ferb.sense.daily import DailyActivity
    out: dict[str, Any] = {}
    db = new_db("perception")
    clock = FakeClock(START)
    ids = SequentialIds("p")
    red = Redactor()
    journal: list[Any] = []
    published: list[Any] = []
    paused = [False]
    trail = Trail(db, clock, SequentialIds("tr"), red, lambda: Exclusions.build(["com.example.hidden"]),
                  lambda k, d: published.append([k, d]), lambda: "dev", lambda: paused[0])
    daily = DailyActivity(TMP / "pact" / "today.jsonl", clock.now)
    rolled = []
    daily.roll_over = lambda: rolled.append(1)                       # type: ignore[method-assign]
    daemon = SimpleNamespace(db=db, clock=clock, ids=ids, redact=red, _jev_active=lambda: None,
                             _publish=lambda k, d: published.append([k, d]),
                             config=SimpleNamespace(get=lambda s, k, d=None: d), sense=SimpleNamespace(paused=False),
                             trail=trail, _journal=lambda: SimpleNamespace(append=lambda k, d: journal.append([k, d])),
                             daily_activity=daily)
    p = Perception(daemon)
    log = []

    def n(ext: str, bundle: str, title: str, body: str, conv: str | None = None) -> Observation:
        data = {"external_id": ext, "bundle_id": bundle, "title": title, "body": body,
                "conversation": conv or title, "conversation_source": "thread" if conv else "title"}
        return Observation("notification", data, {"actor": "unknown"})
    # the floor: the first read after install marks everything seen
    first = p.notification_floor([n("x1", "com.chat", "A", "a"), n("x2", "com.chat", "B", "b")])
    log.append({"op": "floor", "out": len(first), "seen": rows(db, "perception_seen"),
                "floor": [dict(r) for r in db.conn.execute("SELECT source, seq FROM perception_floor ORDER BY source")]})
    second = p.notification_floor([n("x3", "com.chat", "C", "c")])
    log.append({"op": "floor again", "out": [o.data["external_id"] for o in second]})
    banner = p.banner_posted({"title": "FERB", "body": "Done: filed it"}, "com.ferb.ferb")
    other = p.banner_posted({"title": "FERB", "body": "never shown"}, "com.ferb.ferb")
    p.banner_failed(other)
    log.append({"op": "banners", "banners": list(p.banners), "banner": banner})
    steps = [
        n("x1", "com.chat", "A", "already seen"),
        n("y1", "com.chat", "Ann", "lunch at 1? card 4111 1111 1111 1111", "thread-1"),
        n("y1", "com.chat", "Ann", "again"),
        n("y2", "COM.FERB.FERB", "FERB", "Done: filed it"),
        n("y3", "com.ferb.ferb", "FERB", "Done: filed it"),
        n("y4", "com.ferb.FERB", "FERB", "anything"),
        n("y5", "dev.ferb.notch", "Dev", "dev build"),
        n("y6", "com.example.hidden", "Hidden", "excluded app"),
    ]
    for o in steps:
        before = len(published)
        p.notification(o)
        log.append({"op": "notification", "data": o.data, "published": published[before:],
                    "seen": rows(db, "perception_seen"), "banners": list(p.banners)})
    daemon.sense.paused = True
    p.notification(n("z1", "com.chat", "Bob", "while paused"))
    daemon.sense.paused = False
    db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES "
                    "('pre', '2026-03-04T00:00:00+00:00', '2026-03-04', 'dev', 'unknown', NULL, 'notification', ?)",
                    (json.dumps({"external_id": "z2"}),))
    p.notification(n("z2", "com.chat", "Bob", "recorded before the receipt"))
    log.append({"op": "paused and pre-recorded", "seen": rows(db, "perception_seen")})
    out["notifications"] = log
    out["trail_rows"] = trail_rows(db)
    # installed apps from app_install rows
    for i, data in enumerate([{"change": "installed", "bundle_id": "a.one", "path": "/Applications/One.app"},
                              {"change": "installed", "bundle_id": "a.two", "path": "/Applications/Two.app"},
                              {"change": "installed", "bundle_id": "a.one", "path": "/Applications/One Copy.app"},
                              {"change": "removed", "path": "/Applications/Two.app"},
                              {"change": "installed", "bundle_id": "", "path": "/Applications/X.app"},
                              {"change": "installed", "bundle_id": "a.three", "path": "/Applications/Three.app"},
                              {"change": "removed", "path": "/Applications/One Copy.app"}]):
        db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES "
                        "(?, '2026-03-04T00:00:00+00:00', '2026-03-04', 'dev', 'unknown', NULL, 'app_install', ?)",
                        (f"ai{i}", json.dumps(data)))
    out["installed"] = p.installed()
    # the 7-day text expiry
    tdb = new_db("expiry")
    daemon.db = tdb
    for i, (source, at, data) in enumerate([
            ("typed_text", "2026-02-20T10:00:00+00:00", {"bundle_id": "b", "app": "A", "pid": 3, "window_id": "w",
                                                         "field_id": "f", "role": "AXTextField", "field": "To", "text": "old"}),
            ("clipboard", "2026-02-25T18:20:05+00:00", {"bundle_id": "b", "change_count": 4, "text": "x", "app_context": "y"}),
            ("clipboard", "2026-02-25T18:20:06+00:00", {"text": "just inside"}),
            ("typed_text", "2026-02-01T00:00:00+00:00", {"content_expired": True}),
            ("window", "2026-01-01T00:00:00+00:00", {"title": "old window"})]):
        tdb.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES "
                         "(?, ?, ?, 'dev', 'owner', NULL, ?, ?)", (f"x{i}", at, at[:10], source, json.dumps(data)))
    tdb.conn.execute("INSERT INTO attention_notices (id, source_refs_json, text, origin, created_at, delivered_at, "
                     "considered_refs_json, quotes_json) VALUES ('an1', ?, 'You copied x', 'private', 't', NULL, '[]', '[]')",
                     (json.dumps(["trail:x1"]),)) if "considered_refs_json" in {r[1] for r in tdb.conn.execute(
                         "PRAGMA table_info(attention_notices)")} else None
    n_expired = p.purge_text()
    out["purge_text"] = {"expired": n_expired, "rows": trail_rows(tdb), "rolled": len(rolled),
                         "notices": rows(tdb, "attention_notices"), "again": p.purge_text(), "rolled_after": len(rolled)}
    daemon.db = db
    pol = NotificationPolicy(db, clock, ids, red)
    db.conn.execute("INSERT INTO perception_labels VALUES ('nl1','yes','important','t'), ('nl2','no','x','t'), "
                    "('nl3','yes','y','t'), ('nl4','no','z','t')")
    for i, (ref, jev) in enumerate([("trail:nl1", "no"), ("trail:nl1", "yes"), ("trail:nl2", "yes"), ("trail:nl3", "no"),
                                    ("trail:nl4", None), ("trail:nl4", "no")]):
        db.conn.execute("INSERT INTO decisions (id, at, question, ref, baseline, jev, mode) VALUES "
                        "(?, 't', 'notification.attention', ?, 'no', ?, 'shadow')", (f"d{i}", ref, jev))
    out["measurements"] = pol.measurements()
    ann = db.conn.execute("SELECT id FROM trail WHERE json_extract(data_json,'$.external_id')='y1'").fetchone()[0]
    ev = {"id": ann, "data": {"bundle_id": "com.chat", "conversation": "thread-1"}}
    pol.told("com.chat", "thread-1", ann, "told, with 4111 1111 1111 1111")
    out["context"] = [pol.context(ev), pol.context({"id": "missing", "data": {}}),
                      pol.context({"id": "missing", "data": {"bundle_id": "com.chat", "conversation": "Ann"}})]
    db.conn.execute("INSERT INTO perception_batches VALUES ('pb1','t','age','done','[]',NULL), ('pb2','t','age','pending','[]',NULL),"
                    " ('pb3','t','tokens','done','[]',NULL)")
    out["status"] = p.status()
    out["told_rows"] = rows(db, "notification_told")
    out["journal"] = journal
    return out


# ======================================================================================== tools

def make_tools() -> dict[str, Any]:
    from zoneinfo import ZoneInfo
    from ferb.sense.exclusions import Exclusions
    from ferb.sense.tools import TrailDays, TrailQuery, time_bound, trail_sources
    out: dict[str, Any] = {}
    bounds = []
    for tzname in ("UTC", "Asia/Kolkata", "America/New_York", None):
        tz = ZoneInfo(tzname) if tzname else None
        for value, end in [("2026-03-04", False), ("2026-03-04", True), ("2026-03-04T10:30", False),
                           ("2026-03-04T10:30:15.5+02:00", True), (" 2026-03-08T02:30 ", False), ("", False),
                           (None, True), ("   ", False), ("2026-02-30", False), ("2026-03-04 10:30", False),
                           ("20260304", False), ("2026-03-04T10:30Z", False), ("2026-03-08", True),
                           ("2026-11-01T01:30", False), ("not a date", False), ("2026-03-04T25:00", False)]:
            try:
                b = time_bound(value, end, tz)
                bounds.append([tzname, value, end, b.isoformat() if b else None])
            except ValueError as e:
                bounds.append([tzname, value, end, {"error": str(e)}])
    out["time_bound"] = bounds
    out["sources"] = [[sorted(s), trail_sources(s)] for s in (set(), {"app_focus"}, {"window"}, {"browser_tab", "file"},
                                                               {"clipboard"}, {"window", "notification"}, {"idle", "software"})]
    db = new_db("tools")
    clock = FakeClock(datetime(2026, 3, 4, 12, 0, tzinfo=ZoneInfo("Asia/Kolkata")))
    from ferb.sense.trail import Trail
    trail = Trail(db, clock, SequentialIds("t"), lambda v: v, lambda: Exclusions.build(), lambda k, d: None,
                  lambda: "dev", lambda: False)
    for i, (at, source, data) in enumerate([("2026-03-03T09:00:00+05:30", "window", {"title": "yesterday"}),
                                            ("2026-03-04T08:00:00+05:30", "browser_tab", {"host": "a.example", "title": "A"}),
                                            ("2026-03-04T11:00:00+05:30", "app_focus", {"app": "Notes"}),
                                            ("2026-03-04T11:30:00+05:30", "clipboard", {"text": "copied"})]):
        db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES "
                        "(?, ?, ?, 'dev', 'owner', NULL, ?, ?)", (f"q{i}", at, at[:10], source, json.dumps(data)))
    db.conn.execute("INSERT INTO journal (id, at, kind, data_json) VALUES ('s1','t','trail.day_summary',?), "
                    "('s2','t','trail.day_narrative',?), ('s3','t','trail.day_summary',?)",
                    (json.dumps({"day": "2026-03-03", "apps_min": {"Notes": 3}}),
                     json.dumps({"day": "2026-03-03", "text": "A day"}), json.dumps({"day": "2026-03-02"})))
    q = TrailQuery(lambda: trail)
    runs = []
    for args in ({}, {"since_hours": 2}, {"since": "2026-03-04"}, {"since": "2026-03-03", "until": "2026-03-03"},
                 {"since": "2026-03-04T08:00", "until": "2026-03-04T11:00"}, {"source": "app_focus"},
                 {"since_hours": 48, "host": "a.example"}, {"since_hours": 0.5},
                 {"since": "2026-03-04T05:00:00+00:00"}):
        res = asyncio.run(q.run(TrailQuery.Input(**args), None))
        runs.append({"args": args, "output": res.content, "read_sources": res.read_sources, "is_error": res.is_error})
    none = asyncio.run(TrailQuery(lambda: None).run(TrailQuery.Input(), None))
    runs.append({"args": "no trail", "output": none.content, "is_error": none.is_error})
    out["query"] = runs
    from ferb.memory import summary
    days_runs = []
    handles: dict[str, Any] = {}
    for args in ({}, {"text": "day"}, {"since_days": 1}):
        res = asyncio.run(TrailDays(lambda: trail).run(TrailDays.Input(**args), None))
        for d in res.content["days"]:
            handles[d["day"]] = sorted(kind + ":" + ident for sources in summary.build(db, d["day"]).handles.values()
                                       for kind, ident in sources)
        days_runs.append({"args": args, "output": res.content, "read_sources": res.read_sources})
    out["days"] = days_runs
    out["handles"] = handles
    out["digest_origins"] = [[k, d, sorted(summary.digest_origins(k, d))] for k, d in (
        ("trail.day_narrative", {}), ("trail.day_summary", {}), ("trail.day_summary", {"windows": ["x"]}),
        ("trail.day_summary", {"folders": {"a": 1}, "ferb": {"tasks": [1]}}), ("trail.day_summary", {"ferb": {"tasks": []}}),
        ("trail.day_summary", {"hours": {"1": []}, "apps_min": {"x": 1}}))]
    return out


# ======================================================================================== front window

def make_front_window() -> dict[str, Any]:
    import ferb.act.mac.tools as mac_tools
    import ferb.sense.front_window as fw
    from ferb.act.mac.screenshot import Window
    from ferb.core.errors import Refused
    from ferb.sense.watchers.browser_tabs import Ambiguous

    def fake_sources(bundle: str) -> tuple[tuple[str, str], ...]:
        if bundle == "com.apple.MobileSMS":
            return (("message", bundle), ("owner_account", bundle + "|"))
        if browsers_mod.known(bundle):
            return (("web", bundle),)
        return (("app", bundle),)
    mac_tools._sources = fake_sources                                  # type: ignore[assignment]
    fw.is_browser = browsers_mod.known                                 # type: ignore[assignment]
    apps = {10: ("Safari", "com.apple.Safari"), 11: ("Notes", "com.apple.Notes"), 12: ("FERB", "com.ferb.ferb"),
            13: ("Terminal", "com.apple.Terminal"), 14: ("Vault", "com.example.vault"), 15: ("Brave", "com.brave.Browser"),
            16: ("Messages", "com.apple.MobileSMS"), 20: ("", ""), 21: ("", "")}
    parents = {20: 99, 21: 1, 30: 13, 31: 30, 99: 1}
    daemon_pid = 99
    cases = []

    def W(i: int, pid: int, title: str = "", bounds: tuple[float, ...] = (0, 0, 800, 600)) -> Window:
        return Window(i, pid, bounds, title)  # type: ignore[arg-type]
    scenarios = [
        {"name": "plain app", "windows": [W(1, 11, "Groceries")], "shoot": "ok"},
        {"name": "ferb first", "windows": [W(1, 12, "FERB"), W(2, 20, "helper"), W(3, 11, "Note")], "shoot": "ok",
         "masking": "fields found by Accessibility"},
        {"name": "nothing", "windows": [W(1, 12), W(2, 20)]},
        {"name": "sender skipped", "windows": [W(1, 13, "zsh"), W(2, 13, "zsh 2"), W(3, 11, "N")], "client": 31, "shoot": "ok"},
        {"name": "excluded app", "windows": [W(1, 14, "Vault — passwords")], "shoot": "ok"},
        {"name": "browser tab", "windows": [W(1, 10, "Docs", (10, 20, 900, 700))], "tab": ["https://docs.example/a", "Docs"],
         "shoot": "ok"},
        {"name": "browser extension named", "windows": [W(1, 15, "Mail")], "tab": None,
         "named": ["https://mail.example/inbox?token=SECRETCODE123", "Inbox"], "shoot": "ok"},
        {"name": "browser ambiguous then named", "windows": [W(1, 15, "Mail")], "tab": "ambiguous",
         "named": ["https://mail.example/", "Inbox"], "shoot": "ok"},
        {"name": "browser no window", "windows": [W(1, 15, "Mail")], "tab": "lookup", "shoot": "ok"},
        {"name": "browser read after the shot", "windows": [W(1, 15, "Mail")], "tab": None,
         "tab_after": ["https://after.example/", ""], "shoot": "ok"},
        {"name": "browser after the shot ambiguous", "windows": [W(1, 15, "Mail")], "tab": None, "tab_after": "ambiguous",
         "shoot": "ok"},
        {"name": "excluded site", "windows": [W(1, 10, "Bank")], "tab": ["https://www.bank.example/acct", "Bank"],
         "shoot": "ok"},
        {"name": "refused shot", "windows": [W(1, 11, "N")], "shoot": "refused"},
        {"name": "failed shot", "windows": [W(1, 11, "N")], "shoot": "error"},
        {"name": "redaction fails", "windows": [W(1, 11, "N")], "shoot": "ok", "redact_image": "error"},
        {"name": "keep fails", "windows": [W(1, 11, "N")], "shoot": "ok", "keep": "error",
         "masking": "masked by text recognition"},
        {"name": "main window", "windows": [W(1, 15, "", (0, 0, 784, 21)), W(2, 15, "Instagram", (100, 50, 1200, 800)),
                                            W(3, 11, "N")], "main": {15: (101, 51, 1199, 801)}, "tab": None, "shoot": "ok"},
        {"name": "messages", "windows": [W(1, 16, "Ann")], "shoot": "ok"},
        {"name": "secret title", "windows": [W(1, 11, "password: hunter2")], "shoot": "ok"},
    ]
    for sc in scenarios:
        stage: list[str] = []
        calls: list[Any] = []

        def tab(bundle: str, w: Any, after: bool, sc: Any = sc) -> Any:
            calls.append(["tab", bundle, w.id, after])
            v = sc.get("tab_after") if after else sc.get("tab")
            if v == "ambiguous":
                raise Ambiguous("several could be it")
            if v == "lookup":
                raise LookupError("no window is there")
            return tuple(v) if v is not None else None

        async def shoot(w: Any, sc: Any = sc) -> Any:
            calls.append(["shoot", w.id])
            if sc.get("shoot") == "refused":
                raise Refused("the window is on another Space")
            if sc.get("shoot") == "error":
                raise OSError("capture failed")
            named = tuple(sc["named"]) if sc.get("named") else None
            return b"jpeg-" + str(w.id).encode(), sc.get("masking", ""), named

        def redact_image(raw: bytes, sc: Any = sc) -> bytes:
            if sc.get("redact_image") == "error":
                raise ValueError("no")
            return raw + b"-redacted"

        async def keep(image: bytes, facts: dict[str, Any], origin: str, sc: Any = sc) -> str:
            calls.append(["keep", image.decode(), facts, origin])
            if sc.get("keep") == "error":
                raise KeyError("store")
            return "ev-1"
        main = sc.get("main", {})
        src = fw.Sources(windows=lambda sc=sc: sc["windows"], app_of=lambda pid: apps.get(pid),
                         own=lambda pid, bundle: fw.ferb_process(pid, bundle, daemon_pid, lambda p: parents.get(p)),
                         excluded=lambda b, n, u: b == "com.example.vault" or bool(u and "bank.example" in u),
                         tab=tab, shoot=shoot, redact_image=redact_image, redact=Redactor(), keep=keep,
                         client=sc.get("client"), parent=lambda p: parents.get(p),
                         main_frame=lambda pid, main=main: main.get(pid), stage=stage)
        got = asyncio.run(fw.capture(src))
        res = None
        if got is not None:
            res = {"app": got.app, "bundle_id": got.bundle_id, "title": got.title, "url": got.url,
                   "tab_title": got.tab_title, "image_ref": got.image_ref, "origin": got.origin, "withheld": got.withheld,
                   "image": got.image.decode() if got.image else None, "tab_note": got.tab_note,
                   "tab_source": got.tab_source, "masking": got.masking, "facts": got.facts()}
        cases.append({"scenario": {k: v for k, v in sc.items() if k != "windows"} | {"windows": [
            [w.id, w.pid, list(w.bounds), w.title] for w in sc["windows"]]}, "stage": stage, "calls": calls, "out": res})
    proc = [[pid, b, fw.ferb_process(pid, b, daemon_pid, lambda p: parents.get(p))]
            for pid, b in ((12, "com.ferb.ferb"), (12, "COM.FERB.DAEMON"), (99, "x"), (20, ""), (21, ""), (31, ""),
                           (11, "com.apple.Notes"), (1, ""), (0, ""), (55, ""))]
    loops = {40: 41, 41: 40}
    proc.append([40, "", fw.ferb_process(40, "", daemon_pid, lambda p: loops.get(p))])
    sending = [[pid, fw.sending_app(pid, lambda p: apps.get(p), lambda p: parents.get(p))] for pid in (31, 30, 20, 11, 1, 55)]
    sending.append([40, fw.sending_app(40, lambda p: None, lambda p: loops.get(p))])
    return {"apps": {str(k): list(v) for k, v in apps.items()}, "parents": {str(k): v for k, v in parents.items()},
            "daemon_pid": daemon_pid, "captures": cases, "ferb_process": proc, "sending_app": sending,
            "excluded_app_text": fw.EXCLUDED_APP, "ferb_apps": sorted(fw.FERB_APPS)}


# ======================================================================================== software

def make_software() -> dict[str, Any]:
    from ferb.sense import software as S
    out: dict[str, Any] = {}
    inv = [
        None,
        b"",
        b"garbage only\nno records here\n",
        ("UUID: 1111\n Name: Helper\n Developer Name: Example Corp\n Team Identifier: ABCDE12345\n Type: login item\n"
         " Flags: [enabled]\n Identifier: com.example.app.helper\n Parent Identifier: com.example.app\n"
         " URL: file:///Applications/Example.app/Contents/Library/LoginItems/Helper%20One.app/\n"
         "UUID: 2222\n Name: Other\n Identifier: com.other\n Executable Path: /Applications/Other.app/Contents/MacOS/o\n"
         "UUID: 3333\n Identifier: com.example.app\n Disposition: [enabled, allowed]\n"
         "UUID: 4444\n Executable Path: /Applications/Example.app/Contents/MacOS/agent\n Unknown Key: x\n"
         "UUID: 5555\n URL: relative/Example.app\n"
         "UUID: 6666\n Executable Path: /Applications/Example.appx/bin\n"
         "UUID: 7777\n URL: file:///Applications/Exa%6Dple.app/x\n").encode(),
        b"Name: before uuid\nUUID: a\nIdentifier: com.example.app\n\xff\xfe broken: bytes\n",
    ]
    out["login_items"] = [[i.decode(errors="replace") if i is not None else None,
                           S.login_items(Path("/Applications/Example.app"), "com.example.app", inventory=i)] for i in inv]
    calls: list[Any] = []

    def run(args: list[str], capture_output: bool = True) -> Any:
        calls.append(args)
        if args[:2] == ["/usr/bin/codesign", "-dv"]:
            return SimpleNamespace(returncode=0, stdout=b"Executable=/x\n",
                                   stderr=b"Identifier=com.example.app\nFormat=app bundle\nAuthority=Developer ID Application: Ex\n"
                                          b"Authority=Developer ID Certification Authority\nAuthority=Apple Root CA\n"
                                          b"TeamIdentifier=ABCDE12345\nRuntime Version=14.0.0\nSignature size=9000\nSignature=adhoc\n")
        if args[0] == "/usr/sbin/spctl":
            return SimpleNamespace(returncode=3, stdout=b"", stderr=b"/Applications/Example.app: rejected\nsource=Unnotarized\n")
        if args[:2] == ["/usr/bin/codesign", "-d"]:
            return SimpleNamespace(returncode=0, stdout=plistlib.dumps({"com.apple.security.app-sandbox": True,
                                                                        "keychain-access-groups": ["ABCDE12345.x"]}), stderr=b"")
        raise OSError("not found")

    def run_bad(args: list[str], capture_output: bool = True) -> Any:
        if args[0] == "/usr/sbin/spctl":
            raise FileNotFoundError("spctl")
        return SimpleNamespace(returncode=1 if "-dv" in args else 0, stdout=b"not a plist" if "--entitlements" in args else b"",
                               stderr=b"code object is not signed at all\n")

    def xattr(path: Path, name: str) -> bytes:
        if name == "com.apple.quarantine":
            return b"0083;65f00000;Safari;ABCD-1234"
        return plistlib.dumps(["https://example.com/Example.dmg", "https://example.com/"], fmt=plistlib.FMT_BINARY)

    def xattr_bad(path: Path, name: str) -> bytes:
        if name == "com.apple.quarantine":
            raise OSError("no attribute")
        return b"bplist00 broken"
    privacy = lambda b, p: {"user": {"status": "read", "grants": []}}       # noqa: E731
    jobs = lambda p: [{"path": "/Library/LaunchAgents/x.plist"}]            # noqa: E731
    out["security"] = [S.app_security_facts(Path("/Applications/Example.app"), "com.example.app", run=run, xattr=xattr,
                                            privacy=privacy, jobs=jobs),
                       S.app_security_facts(Path("/Applications/Example.app"), "com.example.app", run=run_bad,
                                            xattr=xattr_bad, privacy=privacy, jobs=jobs)]
    out["security_calls"] = calls
    folders = [TMP / "jobs" / "user", TMP / "jobs" / "sys"]
    for f in folders:
        f.mkdir(parents=True)
    (folders[0] / "a.plist").write_bytes(plistlib.dumps({"Label": "com.example.a",
                                                         "ProgramArguments": ["/Applications/Example.app/Contents/MacOS/a", "-x"]}))
    (folders[0] / "b.plist").write_bytes(plistlib.dumps({"Label": "com.other", "Program": "/usr/bin/true"}))
    (folders[0] / "c.plist").write_bytes(b"not a plist")
    (folders[0] / "d.txt").write_bytes(plistlib.dumps({"Program": "/Applications/Example.app/x"}))
    (folders[1] / "e.plist").write_bytes(plistlib.dumps({"Label": "com.example.e", "Program": "/Applications/Example.app/b",
                                                         "ProgramArguments": ["/ignored"]}, fmt=plistlib.FMT_BINARY))
    (folders[1] / "f.plist").write_bytes(plistlib.dumps({"ProgramArguments": []}))
    (folders[1] / "g.plist").write_bytes(plistlib.dumps(["a list"]))
    (folders[1] / "h.plist").write_bytes(plistlib.dumps({"Program": "Example.app/rel"}))
    (folders[1] / "i.plist").write_bytes(plistlib.dumps({"Label": 5, "Program": "/Applications/Example.app/../Other/x"}))
    try:
        S.associated_jobs(Path("/Applications/Example.app"), folders)
        out["jobs_list_root"] = "no error"
    except AttributeError as e:                       # a plist whose root isn't a dictionary (a bug: see the Rust test)
        out["jobs_list_root"] = {"error": "AttributeError", "message": str(e)}
    (folders[1] / "g.plist").unlink()
    out["jobs"] = [S.associated_jobs(Path("/Applications/Example.app"), folders),
                   S.associated_jobs(Path("/Applications/Example.app"), [TMP / "jobs" / "missing"])]
    tcc = TMP / "tcc" / "TCC.db"
    tcc.parent.mkdir()
    with sqlite3.connect(tcc) as c:
        c.execute("CREATE TABLE access (service TEXT, client TEXT, auth_value INTEGER, last_modified INTEGER, extra TEXT)")
        c.executemany("INSERT INTO access VALUES (?,?,?,?,?)", [
            ("kTCCServiceScreenCapture", "com.example.app", 2, 1700000000, "x"),
            ("kTCCServiceAccessibility", "/Applications/Example.app", 0, 1700000001, None),
            ("kTCCServiceCamera", "com.other", 2, 1, None),
            ("kTCCServiceAppleEvents", "com.example.app", 2, 1700000002, None)])
    old = TMP / "tcc" / "old.db"
    with sqlite3.connect(old) as c:
        c.execute("CREATE TABLE access (service TEXT, client TEXT)")
    (TMP / "tcc" / "junk.db").write_bytes(b"not sqlite at all" * 100)
    out["privacy"] = [S.privacy_facts("com.example.app", Path("/Applications/Example.app"), tcc),
                      S.privacy_facts("com.example.app", Path("/Applications/Example.app"), old),
                      S.privacy_facts("com.example.app", Path("/Applications/Example.app"), TMP / "tcc" / "missing.db"),
                      S.privacy_facts("com.example.app", Path("/Applications/Example.app"), TMP / "tcc" / "junk.db")]
    return out


# ======================================================================================== transcribe

def make_transcribe() -> dict[str, Any]:
    import secrets as secrets_mod
    from ferb.sense import transcribe as T
    out: dict[str, Any] = {}
    app = TMP / "home" / ".ferb" / "FERB.app"
    out["launch_args"] = [T.launch_args(app, "transcribe", "/a.wav", "/ans.json", "en-US"),
                          T.launch_args(app, "status", home=TMP / "fh")]
    out["why"] = [[d, T._why(d, Path("/x.wav"))] for d in (
        {"error": "x", "reason": "on_device_unavailable", "locale": "hi-IN"}, {"error": "x", "reason": "on_device_unavailable"},
        {"error": "x", "reason": "not_authorized", "speech": "denied"}, {"error": "x", "reason": "not_authorized"},
        {"error": "x", "reason": "unsupported_language", "locale": "xx"}, {"error": "boom"}, {"error": 5, "reason": None})]
    red = Redactor()
    W = T.Word
    transcripts = [
        T.Transcript("Hello there. My card is 4111 1111 1111 1111. Thanks!",
                     [W("Hello", 0.0, 0.4), W("there.", 0.4, 0.8), W("My", 1.0, 1.1), W("card", 1.1, 1.4), W("is", 1.4, 1.5),
                      W("4111", 1.5, 2.0), W("1111", 2.0, 2.4), W("1111", 2.4, 2.8), W("1111.", 2.8, 3.3),
                      W("Thanks!", 3.5, 4.0)]),
        T.Transcript("one two three", [W("one", 0, 0.3), W("missing", 0.3, 0.4), W("two", 0.5, 0.7), W("", 0.7, 0.8),
                                       W("three", 0.8, 1.123456)]),
        T.Transcript("Line one\nline two? yes… done。end", [W("Line", 0, 1), W("one", 1, 2), W("line", 2, 3), W("two?", 3, 4),
                                                         W("yes…", 4, 5), W("done。end", 5, 6)]),
        T.Transcript("", []),
        T.Transcript("password: hunter2 is it", [W("password:", 0, 1), W("hunter2", 1, 2), W("is", 2, 3), W("it", 3, 4)]),
        T.Transcript("ünïcödé 😀 words. second", [W("ünïcödé", 0, 1), W("😀", 1, 2), W("words.", 2, 3), W("second", 3, 4.005)]),
    ]
    out["redacted"] = [{"text": t.text, "words": [[w.text, w.start, w.end] for w in t.words], "offset": off,
                        "out": T.redacted(t, red.text, off)} for t in transcripts for off in (0.0, 12.345)]
    tokens = iter(f"{n:016x}" for n in range(1, 1000))
    secrets_mod.token_hex = lambda n=None: next(tokens)               # type: ignore[assignment]
    folder = TMP / "home" / ".ferb" / "workspace" / "audio"
    app.mkdir(parents=True)
    asks = []
    for name, answer, signed_ok, req, delay in [
            ("ok", {"text": "hi there", "segments": [{"text": "hi", "start": 0.5, "duration": 0.25},
                                                    {"text": "there", "start": None}], "locale": "en-US"}, True, "req", 0),
            ("no locale", {"text": "x"}, True, "req", 0),
            ("error", {"error": "denied", "reason": "not_authorized", "speech": "denied"}, True, "req", 0),
            ("empty", "", True, "req", 0), ("not json", "{oops", True, "req", 0), ("list", [1, 2], True, "req", 0),
            ("unsigned", {"text": "x"}, False, "req", 0), ("no requirement", {"text": "x"}, True, None, 0),
            ("timeout", {"text": "late"}, True, "req", 1)]:
        launched: list[Any] = []

        async def launch(argv: list[str], answer: Any = answer, delay: int = delay) -> int:
            launched.append(argv)
            path = Path(argv[argv.index("transcribe") + 2]) if "transcribe" in argv else Path(argv[-1])
            launched.append(oct(path.stat().st_mode & 0o777))
            if delay:
                await asyncio.sleep(10)
            if answer != "":
                path.write_text(answer if isinstance(answer, str) else json.dumps(answer))
            return 7
        tr = T.Transcriber(app, lambda req=req: req, launch, lambda a, r, ok=signed_ok: ok)
        audio = folder / "clip.wav"
        orig = T.LIMIT_S
        if delay:
            T.LIMIT_S = 0.05
        try:
            res = asyncio.run(tr.transcribe(audio, "en-US" if name != "no locale" else None, audio_s=0.0))
            got = {"text": res.text, "words": [[w.text, w.start, w.end] for w in res.words], "locale": res.locale}
        except T.HearingUnavailable as e:
            got = {"error": type(e).__name__, "message": str(e)}
        finally:
            T.LIMIT_S = orig
        left = sorted(p.name for p in folder.iterdir()) if folder.exists() else []
        asks.append({"name": name, "answer": answer, "signed": signed_ok, "requirement": req, "launched": launched,
                     "out": got, "left": left, "in_flight": tr.in_flight,
                     "folder_mode": oct(folder.stat().st_mode & 0o777) if folder.exists() else None})
    st: list[Any] = []

    async def status_launch(argv: list[str]) -> int:
        st.append(argv)
        Path(argv[-1]).write_text(json.dumps({"speech": "authorized", "on_device": True, "locale": "en", "other": 1}))
        return 0
    status = asyncio.run(T.Transcriber(app, lambda: "req", status_launch, lambda a, r: True, home=TMP / "fh2")
                         .status(TMP / "fh2" / "audio"))
    out["asks"] = asks
    out["status"] = {"launched": st, "out": status}
    missing = asyncio.run(_try(T.Transcriber(TMP / "nope.app", lambda: "r", status_launch, lambda a, r: True)
                               .transcribe(Path("/x.wav"), None)))
    out["missing_app"] = missing
    import subprocess as sp
    real_run = sp.run
    ps = ("  101 " + str(app) + "/Contents/MacOS/FERB transcribe " + str(folder) + "/clip.wav " + str(folder) + "/.t.json\n"
          "102 " + str(app) + "/Contents/MacOS/FERB status " + str(folder) + "/.s.json\n"
          "103 " + str(app) + "/Contents/MacOS/FERB\n"
          "104 " + str(app) + "/Contents/MacOS/FERB request-speech\n"
          "x05 " + str(app) + "/Contents/MacOS/FERB transcribe " + str(folder) + "/a\n"
          "106 /other/FERB.app/Contents/MacOS/FERB transcribe " + str(folder) + "/a\n"
          "107 " + str(app) + "/Contents/MacOS/FERB transcribe /elsewhere/a\n"
          + str(os.getpid()) + " " + str(app) + "/Contents/MacOS/FERB transcribe " + str(folder) + "/a\n")
    T.subprocess.run = lambda *a, **k: SimpleNamespace(returncode=0, stdout=ps)    # type: ignore[assignment]
    out["hearing_pids"] = {"ps": ps.replace(str(os.getpid()), "<SELF>"), "app": str(app), "folder": str(folder),
                           "out": T.hearing_pids(app, folder)}
    T.subprocess.run = real_run                                       # type: ignore[assignment]
    return out


async def _try(coro: Any) -> Any:
    try:
        return await coro
    except Exception as e:
        return {"error": type(e).__name__, "message": str(e)}


# ======================================================================================== watchers

def make_watchers() -> dict[str, Any]:
    from ferb.sense.watchers import accessibility as AX
    from ferb.sense.watchers import app_installs as AI
    from ferb.sense.watchers import browser_tabs as BT
    from ferb.sense.watchers import clipboard as CB
    from ferb.sense.watchers import files as FI
    from ferb.sense.watchers.base import Focused
    from ferb.sense.watchers.idle import IdleWatcher
    from ferb.sense.watchers.notifications import NotificationDatabase
    from ferb.sense.watchers.presence import MacPresence
    from ferb.sense.exclusions import Exclusions
    from ferb.device.profile import AppInfo
    out: dict[str, Any] = {}
    AX_is = AX.__dict__.get("is_browser")
    import ferb.act.mac.browsers as bm
    red = Redactor()
    out["accessibility"] = [[b, n, pid, kind, facts, AX.accessibility_observation(b, n, pid, kind, facts, red).data]
                            for b, n, pid, kind, facts in [
                                ("com.google.Chrome", "Chrome", 1, "AXTitleChanged",
                                 {"window_id": "1", "field_id": "f", "role": "AXWindow", "title": "Private bank statement"}),
                                ("com.apple.Notes", "Notes", 2, "AXTitleChanged",
                                 {"window_id": "1", "field_id": "f", "role": "AXWindow", "title": "Bank statement 4111 1111 1111 1111",
                                  "extra": "dropped"}),
                                ("com.apple.Notes", "Notes", 2, "AXFocusedUIElementChanged", {"role": "AXButton"})]]
    del AX_is

    def attr_of(table: dict[str, Any]) -> Any:
        return lambda el, a: (table.get(el) or {}).get(a)
    elements = {
        "plain": {"AXRole": "AXTextField", "AXIdentifier": "f1", "AXTitle": "To", "AXWindow": "win"},
        "secure": {"AXRole": "AXSecureTextField", "AXIdentifier": "s1", "AXTitle": "Password", "AXWindow": "win"},
        "sub": {"AXRole": "AXTextField", "AXSubrole": "AXSecureTextField", "AXIdentifier": "s2", "AXWindow": "win"},
        "shown": {"AXRole": "AXTextField", "AXIdentifier": "p1", "AXPlaceholderValue": "Password", "AXTitle": "x",
                  "AXWindow": "win"},
        "nowin": {"AXRole": "AXButton", "AXIdentifier": "b1", "AXTitle": None},
        "win": {"AXWindowNumber": 4242},
        "num": {"AXRole": None, "AXIdentifier": "n", "AXTitle": 7, "AXWindow": "win"},
    }
    out["element_facts"] = [[el, kind, AX.element_facts(kind, el, attr_of(elements))]
                            for el in ("plain", "secure", "sub", "shown", "nowin", "num")
                            for kind in ("AXValueChanged", "AXTitleChanged")]
    out["element_table"] = elements
    # app installs
    folders = [TMP / "Apps", TMP / "Apps" / "Utilities"]
    for f in folders:
        f.mkdir(parents=True)
    (TMP / "Apps" / "Here.app").mkdir()
    infos = {"Here.app": AppInfo("com.example.here", "Here", "1.0", str(TMP / "Apps" / "Here.app"), False)}
    AI.read_app = lambda p: infos.get(p.name)                          # type: ignore[assignment]
    w = AI.AppInstallsWatcher(lambda c, i, p: None, folders=folders + [TMP / "NotThere"])
    out["app_install_folders"] = [str(f) for f in w.folders]
    out["app_installs"] = [[p, fl, None if (o := w.observe(p, fl)) is None else [o.source, o.data, {k: v for k, v in o.hints.items()
                                                                                                  if k != "app_info"}]]
                           for p, fl in [(str(TMP / "Apps" / "Here.app"), 0x100), (str(TMP / "Apps" / "Gone.app"), 0x200),
                                         (str(TMP / "Apps" / "Here.app"), 0x800), (str(TMP / "Apps" / "Gone.app"), 0x800),
                                         (str(TMP / "Apps" / "Here.app" / "Contents"), 0x100),
                                         (str(TMP / "Elsewhere" / "Here.app"), 0x100), (str(TMP / "Apps" / "Here.txt"), 0x100),
                                         (str(TMP / "Apps" / "Here.app"), 0x1000), (str(TMP / "Apps" / "Here.app"), 0x200),
                                         (str(TMP / "Apps" / "Unknown.app"), 0x100)]]
    (TMP / "Apps" / "Unknown.app").mkdir()
    out["app_installs"].append([str(TMP / "Apps" / "Unknown.app"), 0x100, None])
    # browser tabs
    out["tab_observation"] = [[args, None if (o := BT.tab_observation(*args)) is None else [o.source, o.data, o.hints]]
                              for args in [("chrome", "Default", False, "activated", "https://Docs.Example:8080/a", "Docs", None, None),
                                           ("brave", "P1", True, "updated", "https://x.example/", None, True, "task-1"),
                                           ("chrome", "Default", False, "created", "https://x.example/", "t", False, None),
                                           ("chrome", "Default", False, "updated", "", "t", False, None),
                                           ("chrome", "Default", False, "updated", "about:blank", "t", False, None)]]
    out["scripts"] = {b: list(v) for b, v in BT.SCRIPTS.items()}
    out["window_scripts"] = {b: BT._WINDOWS.format(bundle=b, tab=BT.WINDOW_SCRIPTS[b]) for b in BT.SCRIPTS}
    out["front_tab_script"] = {b: f'tell application id "{b}"\n{BT.SCRIPTS[b][1]}\nend tell' for b in BT.SCRIPTS}
    import subprocess as sp
    real_run = sp.run
    outputs = {"ok": "1\x1d22\x1d1201\x1d822\x1fMail\x1fhttps://mail.example/\x1fInbox\x1e"
                     "-5,5\x1d0\x1d100\x1d50\x1fDocs\x1f  https://docs.example/ \x1fDocs\x1e\x1fNo bounds\x1f\x1f\x1e"
                     "bad\x1d1\x1fBad\x1fu\x1ft\x1e3 fields\x1fx\x1fy\n",
               "front": "https://a.example/\nA title\nwith lines\n", "blank": "  \nT\n", "fail": None}
    parsed = {}
    for name, stdout in outputs.items():
        BT.subprocess.run = (lambda *a, s=stdout, **k: SimpleNamespace(returncode=1 if s is None else 0, stdout=s or ""))  # type: ignore
        ws = BT.applescript_windows("com.google.Chrome")
        ft = BT.applescript_front_tab("com.apple.Safari")
        parsed[name] = {"stdout": stdout, "windows": None if ws is None else [[w.bounds, w.name, w.url, w.title] for w in ws],
                        "front": ft}
    BT.subprocess.run = real_run                                       # type: ignore[assignment]
    out["parsed"] = parsed
    SW = BT.ScriptedWindow
    wt = []
    win_sets = {"one": [SW((0, 0, 800, 600), "A", "https://a/", "At")],
                "two": [SW((0, 0, 800, 600), "A", "u1", "t1"), SW((1, 1, 799, 601), "B", "u2", "t2")],
                "none": [SW((500, 0, 800, 600), "A", "u", "t")],
                "anywhere": [SW(None, "A", "u", "t")],
                "mix": [SW((0, 0, 800, 600), "A", "u1", "t1"), SW(None, "B", "u2", "B")],
                "empty": []}
    for name, ws in win_sets.items():
        for title in ("A", "B", "", "zzz", "t2"):
            try:
                got = BT.window_tab(ws, (0, 0, 800, 600), title)
                wt.append([name, title, [got.bounds, got.name, got.url, got.title]])
            except BT.Ambiguous as e:
                wt.append([name, title, {"ambiguous": str(e)}])
            except LookupError as e:
                wt.append([name, title, {"lookup": str(e)}])
    out["window_sets"] = {k: [[w.bounds, w.name, w.url, w.title] for w in v] for k, v in win_sets.items()}
    out["window_tab"] = wt
    front = [None]
    reads = {"com.apple.Safari": ("https://s.example/", "S"), "com.google.Chrome": ("https://c.example/p", "C"),
             "com.brave.Browser": None}
    watcher = BT.ScriptedTabsWatcher(lambda b: b != "com.vivaldi.Vivaldi", lambda br: br == "chrome",
                                     front=lambda: front[0], read=lambda b: reads.get(b))
    st = []
    for f in [None, Focused(1, "com.apple.Notes", "Notes"), Focused(2, "com.apple.Safari", "Safari"),
              Focused(2, "com.apple.Safari", "Safari"), Focused(3, "com.google.Chrome", "Chrome"),
              Focused(4, "com.brave.Browser", "Brave"), Focused(5, "com.vivaldi.Vivaldi", "Vivaldi"),
              Focused(6, "com.operasoftware.Opera", "Opera")]:
        front[0] = f
        o = watcher.check()
        st.append([None if f is None else [f.pid, f.bundle_id, f.name], None if o is None else [o.source, o.data, o.hints]])
    reads["com.apple.Safari"] = ("https://s.example/2", "S2")
    front[0] = Focused(2, "com.apple.Safari", "Safari")
    o = watcher.check()
    st.append([[2, "com.apple.Safari", "Safari"], None if o is None else [o.source, o.data, o.hints]])
    out["scripted_tabs"] = st
    # clipboard
    out["access"] = []
    for behavior in (None, 0, 1, 2, 3, 9):
        board = SimpleNamespace() if behavior is None else SimpleNamespace(accessBehavior=lambda b=behavior: b)
        out["access"].append([behavior, CB.access(board)])
    import ferb.sense.capture as cap

    class Board:
        def __init__(self) -> None:
            self.count, self.kinds, self.text, self.behavior, self.bump = 1, ["public.utf8-plain-text"], "hello", 2, False

        def changeCount(self) -> int:
            return self.count

        def accessBehavior(self) -> int:
            return self.behavior

        def types(self) -> list[str]:
            return self.kinds

        def stringForType_(self, t: str) -> str | None:
            if self.bump:
                self.count += 1
            return self.text
    board = Board()
    focus = [Focused(9, "com.apple.Notes", "Notes")]
    CB.focused_app = lambda: focus[0]                                  # type: ignore[assignment]
    cw = CB.ClipboardWatcher(red, lambda b, n: b == "com.example.vault", board=board)
    steps = [{}, {"count": 2}, {"count": 3, "text": "card 4111 1111 1111 1111"}, {"count": 4, "behavior": 1},
             {"count": 5, "behavior": 0}, {"count": 6, "behavior": 2}, {"count": 7, "focus": None},
             {"count": 8, "focus": ["com.example.vault", "Vault"]}, {"count": 9, "focus": ["com.apple.Notes", "Notes"],
                                                                    "kinds": ["org.nspasteboard.ConcealedType"]},
             {"count": 10, "kinds": ["public.utf8-plain-text"], "text": None}, {"count": 11, "text": "raced", "bump": True},
             {"count": 13, "bump": False, "text": "mine", "mark": [13, "task-2"]}, {"count": 14, "text": "after mine"}]
    clog = []
    for s in steps:
        for k in ("count", "text", "behavior", "kinds", "bump"):
            if k in s:
                setattr(board, k, s[k])
        if "focus" in s:
            focus[0] = None if s["focus"] is None else Focused(9, s["focus"][0], s["focus"][1])
        if "mark" in s:
            cap.mark_clipboard_write(*s["mark"])
        emitted: list[Any] = []
        reported: list[str] = []
        cw.step(lambda e: emitted.append([e.source, e.data, e.hints]), reported.append)
        clog.append({"step": s, "emitted": emitted, "reported": reported, "last": cw.last, "state": cw.state})
    out["clipboard_steps"] = {"initial": 1, "log": clog}
    # files
    home = TMP / "home"
    for d in ("Docs", "Docs/sub", ".ssh"):
        (home / d).mkdir(parents=True, exist_ok=True)
    (home / "file.txt").write_text("x")
    (home / "docs-link").symlink_to(home / "Docs")
    (TMP / "outside").mkdir()
    excl = Exclusions.build(paths=["~/.ssh", "~/Docs/sub"])
    out["valid_folders"] = list(FI.valid_folders(["~/Docs", "~/Docs/sub", "~/.ssh", "~/missing", "~/file.txt",
                                                  str(TMP / "outside"), "~", "~/docs-link", "~/Docs/../Docs"], excl))
    out["valid_folders"][0] = [str(p) for p in out["valid_folders"][0]]
    (TMP / "fs").mkdir()
    (TMP / "fs" / "exists").write_text("x")
    out["change_of"] = [[p, fl, FI.change_of(str(TMP / "fs" / p), fl)] for p, fl in [
        ("exists", 0x200), ("gone", 0x200), ("exists", 0x800), ("gone", 0x800), ("exists", 0x100), ("exists", 0x1000),
        ("exists", 0x100 | 0x1000), ("gone", 0x100 | 0x200), ("exists", 0x100 | 0x200), ("exists", 0x20000), ("exists", 0)]]
    out["noise"] = [[p, FI.noise(p)] for p in ("/a/.DS_Store", "/a/.git", "/a/.git/x", "/a/.github/x", "/r/.git/objects/ab",
                                                "a", "/a/b.git", ".git", "/")]
    mono = [100.0]
    fw_ = FI.FilesWatcher(lambda: [], now=lambda: mono[0])
    flog = []
    for p, fl, dt in [("exists", 0x100, 0), ("exists", 0x100, 0.5), ("exists", 0x100 | 0x1000, 0.6), ("exists", 0x1000, 0.2),
                      ("gone", 0x200, 0.1), ("gone", 0x200, 0.1), ("gone", 0x200, 2.0), ("exists", 0x100, 0.1),
                      (".DS_Store", 0x100, 0), ("exists", 0x20000 | 0x1000, 3), ("exists", 0x0, 0)]:
        mono[0] += dt
        o = fw_.observe(str(TMP / "fs" / p), fl)
        flog.append([p, fl, dt, None if o is None else [o.source, o.data]])
    out["files_observe"] = flog
    # idle
    il = []
    owner = [None]
    running = [True]
    now = [1000.0]
    iw = IdleWatcher(lambda: owner[0], lambda: running[0], idle_after_s=120, now=lambda: now[0])
    for last, run_, t in [(None, True, 1000.0), (990.0, True, 1000.0), (990.0, True, 1100.0), (990.0, True, 1110.0),
                          (990.0, False, 2000.0), (1999.0, True, 2000.0), (None, True, 3000.0)]:
        owner[0], running[0], now[0] = last, run_, t
        em: list[Any] = []
        iw.check(lambda o: em.append([o.source, o.data, o.hints]))
        il.append([last, run_, t, em])
    out["idle"] = il
    # notification database
    ndb = TMP / "usernoted" / "db"
    ndb.parent.mkdir()
    with sqlite3.connect(ndb) as c:
        c.execute("CREATE TABLE app (app_id INTEGER PRIMARY KEY, identifier TEXT)")
        c.execute("CREATE TABLE record (rec_id INTEGER PRIMARY KEY, app_id INTEGER, uuid BLOB, data BLOB, delivered_date REAL)")
        c.executemany("INSERT INTO app VALUES (?,?)", [(1, "com.chat"), (2, "com.example.vault"), (3, "com.mail")])
        recs = [
            (1, 1, b"\x01\x02uuid-1", plistlib.dumps({"req": {"titl": "Ann", "body": "lunch? 4111 1111 1111 1111", "thre": "t-1"}},
                                                     fmt=plistlib.FMT_BINARY)),
            (2, 2, b"uuid-2", plistlib.dumps({"req": {"titl": "Vault", "body": "secret"}})),
            (3, 3, "text-uuid-3", plistlib.dumps({"title": "Plain", "body": 5})),
            (4, 1, b"uuid-4", b"not a plist"),
            (5, 1, b"uuid-5", plistlib.dumps({"req": ["list"]})),
            (6, 3, b"uuid-6", plistlib.dumps({"req": {"titl": "", "thre": ""}})),
        ]
        c.executemany("INSERT INTO record (rec_id, app_id, uuid, data) VALUES (?,?,?,?)", recs)
    reader = NotificationDatabase(ndb, red, lambda b, n: b == "com.example.vault")
    nlog = [{"after": reader.after, "out": [[o.source, o.data, o.hints] for o in reader.read()], "after_read": reader.after}]
    with sqlite3.connect(ndb) as c:
        c.execute("INSERT INTO record (rec_id, app_id, uuid, data) VALUES (7, 1, ?, ?)",
                  (b"uuid-7", plistlib.dumps({"req": {"titl": "Bob", "body": "hey"}})))
    nlog.append({"after": 6, "out": [[o.source, o.data, o.hints] for o in reader.read()], "after_read": reader.after})
    with sqlite3.connect(ndb) as c:
        c.execute("DELETE FROM record WHERE rec_id > 2")
    nlog.append({"after": 7, "out": [[o.source, o.data, o.hints] for o in reader.read()], "after_read": reader.after})
    bad = TMP / "usernoted" / "bad"
    with sqlite3.connect(bad) as c:
        c.execute("CREATE TABLE record (rec_id INTEGER PRIMARY KEY, uuid BLOB)")
        c.execute("CREATE TABLE app (app_id INTEGER, identifier TEXT)")
    try:
        NotificationDatabase(bad, red).read()
        nlog.append({"bad": "no error"})
    except RuntimeError as e:
        nlog.append({"bad": str(e)})
    out["notifications"] = nlog
    out["notification_records"] = [[r[0], r[1], r[2].hex() if isinstance(r[2], bytes) else r[2], r[3].hex()] for r in recs]
    # presence
    returns: list[Any] = []
    wall = [5000.0]
    sess = [{"id": "s", "locked": False, "on_console": True}]
    disp = [False]
    mp = MacPresence(wall=lambda: wall[0], read_session=lambda: sess[0], read_displays=lambda: disp[0])
    mp.on_return = returns.append
    plog = [{"op": "away before start", "away": mp.away()}]
    mp.changed("locked", True)
    plog.append({"op": "changed before start", "away": mp.away(), "returns": list(returns)})
    sess[0] = {"id": "s", "locked": True, "on_console": True}
    mp.start(native=False)
    plog.append({"op": "start locked", "away": mp.away(), "returns": list(returns), "present_since": mp.present_since})
    for flag, value in [("asleep", True), ("locked", False), ("asleep", False), ("asleep", False), ("nonsense", True),
                        ("displays_asleep", True), ("switched_away", True), ("displays_asleep", False),
                        ("switched_away", False)]:
        wall[0] += 10
        mp.changed(flag, value)
        plog.append({"op": "changed", "flag": flag, "value": value, "away": mp.away(), "returns": list(returns),
                     "present_since": mp.present_since})
    sess[0] = {"locked": False, "on_console": False}
    disp[0] = True
    mp.read()
    plog.append({"op": "read", "away": mp.away(), "returns": list(returns)})
    sess[0] = {}
    disp[0] = None                                                     # type: ignore[assignment]
    mp.read()
    plog.append({"op": "read nothing", "away": mp.away(), "returns": list(returns)})
    sess[0] = {"locked": False, "on_console": True}
    disp[0] = False
    wall[0] += 1
    mp.read()
    plog.append({"op": "read back", "away": mp.away(), "returns": list(returns), "present_since": mp.present_since})
    mp.on_return = lambda s: (_ for _ in ()).throw(RuntimeError("x"))  # type: ignore[assignment]
    mp.changed("locked", True)
    mp.changed("locked", False)
    plog.append({"op": "on_return raises", "away": mp.away(), "present_since": mp.present_since})
    mp.stop()
    plog.append({"op": "stop", "away": mp.away()})
    mp.changed("locked", True)
    plog.append({"op": "changed after stop", "away": mp.away()})
    out["presence"] = plog
    del bm
    return out


def main() -> None:
    sizes = {
        "exclusions.json": put("exclusions.json", make_exclusions()),
        "trail.json": put("trail.json", make_trail()),
        "batches.json": put("batches.json", make_batches()),
        "capture.json": put("capture.json", make_capture()),
        "daily.json": put("daily.json", make_daily()),
        "perception.json": put("perception.json", make_perception()),
        "tools.json": put("tools.json", make_tools()),
        "front_window.json": put("front_window.json", make_front_window()),
        "software.json": put("software.json", make_software()),
        "transcribe.json": put("transcribe.json", make_transcribe()),
        "watchers.json": put("watchers.json", make_watchers()),
    }
    for name, size in sizes.items():
        print(f"{name}: {size} bytes")


if __name__ == "__main__":
    main()
