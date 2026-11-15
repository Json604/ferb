"""Records group B's smaller areas from the real Python daemon (ENG-3): About ("What FERB saw" with stored
screenshots, the guide, FERB's own account), owner notices and banner switches, Jev's switches while Jev is off, and
the intro's suggestions from a seen scan. The base transcripts (rust/fixtures/daemon/rpc_transcripts.json) recorded
these only empty or refused.

Run from the repo root:  uv run python rust/fixtures/daemon-b/small.py     -> small.json (+ bodies-small.json)

Built on common.py (the daemon harness of rust/fixtures/daemon/make.py). Each scenario's `setup` lists what a Rust test
rebuilds: config lines, evidence rows and files, notices, the scan's status. All data is invented.
"""

import base64
import json
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

H = common.load("small")

ENTRY = ["ferb/daemon/areas/rpc.py:130 _register_rpc"]
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==")


def on_loop(sc: Any, fn: Any) -> Any:
    return H.on_loop(sc.host, fn)


def sc_about() -> dict[str, Any]:
    """About with FERB's own account set (no Full Disk Access: no profile is read), FERB.app's guide sent, and "What
    FERB saw" over invented evidence rows: one per kind of filter, one of an excluded app, one of an excluded site, one
    outside the evidence window, one without a file, one whose file escapes the store."""
    rows = [
        # id, task_id, kind, mime, path, data, origin, captured_at
        ["ev-1", "t1", "screenshot", "image/png", "2026-01-01/ev-1.png",
         {"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "title": "Invented notes", "tool": "mac.screenshot"},
         "ferb", "2026-01-01T08:00:00+00:00"],
        ["ev-2", "t1.s2", "snapshot", "image/jpeg", "2026-01-01/ev-2.jpg",
         {"url": "https://www.example.test/page?q=1", "tab_title": "Invented page", "tool": "browser.screenshot"},
         "external:web", "2026-01-01T08:30:00+00:00"],
        ["ev-3", "t2", "app_window", "image/png", "2026-01-01/ev-3.png",
         {"app": "1Password", "bundle_id": "com.1password.1password"}, "ferb", "2026-01-01T08:40:00+00:00"],
        ["ev-4", "t2", "snapshot", "image/png", "2026-01-01/ev-4.png",
         {"url": "https://excluded.example/x", "title": 7}, "external:web", "2026-01-01T08:41:00+00:00"],
        ["ev-5", "t3", "screenshot", "image/png", "2025-12-01/ev-5.png", {"app": "Finder"}, "ferb",
         "2025-12-01T08:00:00+00:00"],
        ["ev-6", "t3", "screenshot", "image/png", None, {"app": "Finder"}, "ferb", "2026-01-01T08:50:00+00:00"],
        ["ev-7", "t3", "recording", "video/mp4", "2026-01-01/ev-7.mp4", {"app": "Finder"}, "ferb",
         "2026-01-01T08:51:00+00:00"],
        ["ev-8", "t4", "screenshot", "image/png", "../outside.png", {"window": "Outside"}, "ferb",
         "2026-01-01T08:52:00+00:00"],
        ["ev-9", "t4", "screenshot", None, "2026-01-01/ev-9.png", "not json", "ferb", "2026-01-01T08:53:00+00:00"],
    ]
    files = {"2026-01-01/ev-1.png": PNG, "2026-01-01/ev-2.jpg": b"invented jpeg bytes",
             "2026-01-01/ev-3.png": PNG, "2026-01-01/ev-4.png": PNG, "2025-12-01/ev-5.png": PNG,
             "2026-01-01/ev-7.mp4": b"invented video", "../outside.png": PNG}
    tasks = [["t1", "t1", "req-1"], ["t1.s2", "t1", "req-1"], ["t2", "t2", "req-2"], ["t3", "t3", "req-3"]]

    def prepare(d: Any) -> None:
        d.config.set("mailbox", "address", "ferb.invented@example.test")
        d.config.set("identity", "browser", "chrome")
        d.config.set("identity", "profile", "Profile 3")
        d.config.set("privacy", "exclude_domains", ["excluded.example"])

    sc = common.BScenario("small_about", "About, the guide and What FERB saw over invented evidence rows",
                          ENTRY + ["ferb/daemon/areas/about.py", "ferb/daemon/about.py"],
                          before_start=[prepare],
                          setup={"evidence": rows, "files": {
                              k: base64.b64encode(v).decode() for k, v in files.items()}, "tasks": tasks})

    def seed() -> None:
        d = sc.d
        sc.setup["config"] = (d.home / "config.toml").read_text()
        root = d.home / "evidence"
        for rel, data in files.items():
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(data)
        for r in rows:
            d.db.conn.execute("INSERT INTO evidence (id, task_id, span_id, kind, mime, path, data_json, origin, "
                              "captured_at) VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?)",
                              (r[0], r[1], r[2], r[3], r[4], r[5] if isinstance(r[5], str) else json.dumps(r[5]),
                               r[6], r[7]))
        for tid, tree, req in tasks:
            d.db.conn.execute("INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, "
                              "scopes_json, status, created_at, updated_at) VALUES (?, ?, NULL, ?, 'invented', "
                              "'owner', 'mixed', '[]', 'done', ?, ?)",
                              (tid, tree, req, "2026-01-01T08:00:00+00:00", "2026-01-01T08:00:00+00:00"))
    on_loop(sc, seed)
    c = sc.conn("app")
    guide = {"howtos": [{"title": " Pair  a browser ", "steps": "Settings → Browsers → Pair"}, {"title": "x"},
                        "not a dict"],
             "shortcuts": [{"action": "Open FERB", "keys": "⌥Space"}, {"action": "", "keys": "x"}],
             "places": {"sense.window_titles": "Settings → Privacy", "": "nowhere", "retention.trace_days": ""},
             "app_settings": [{"key": "sounds", "name": "Sounds", "where": "Settings → General", "what": "Chimes",
                               "value": "On" * 70}, {"key": "secret", "name": "Hidden", "where": "Settings"},
                              {"key": "", "name": "No key", "where": "Settings"}]}
    sc.calls(c, ("about.guide", guide), ("about.get", {}),
             ("seen.list", {}), ("seen.list", {"limit": 2}), ("seen.list", {"request_id": "req-1"}),
             ("seen.list", {"task_id": "t1.s2"}), ("seen.list", {"task_id": "t3"}), ("seen.list", {"limit": 0}),
             ("seen.list", {"limit": "x"}),
             ("seen.image", {"id": "ev-1"}), ("seen.image", {"id": "ev-2"}), ("seen.image", {"id": "ev-3"}),
             ("seen.image", {"id": "ev-5"}), ("seen.image", {"id": "ev-6"}), ("seen.image", {"id": "ev-8"}),
             ("seen.image", {"id": "ev-9"}))
    sc.s.close(c)
    return sc.finish()


def sc_notices() -> dict[str, Any]:
    """Owner notices posted through _tell_owner (a loop's, a refused fill's), then listed and acknowledged; the
    banner switches."""
    sc = common.BScenario("small_notices", "Owner notices and banner categories", ENTRY + [
        "ferb/daemon/areas/notify.py", "ferb/daemon/banners.py", "ferb/orchestrator/attention.py"])

    async def post() -> None:
        d = sc.d
        await d._tell_owner("loop-s1-0001", ["task:t1"], "Paused the invented loop.", "ferb", category="loop",
                            task_id="t1")
        d.clock.advance(5)
        await d._tell_refused({"what": "password", "place": "example.test", "words": "Try again",
                               "span_id": "sp-1", "task_id": "t2"})
        d.clock.advance(5)
        await d._tell_owner("plain-1", [], "An invented note.", "ferb",
                            cards=[{"type": "reminder", "title": "Invented"}])
    sc.host.run(post())
    H.settle(sc.host)
    c = sc.conn("app")
    sc.calls(c, ("notices.list", {}), ("attention.ack", {"ids": ["plain-1"]}), ("notices.list", {}),
             ("attention.ack", {"id": "loop-s1-0001"}), ("attention.ack", {"ids": ["no-such-notice"]}),
             ("attention.ack", {"ids": "x"}), ("attention.ack", {"ids": [""]}), ("notices.list", {}),
             ("notifications.categories", {"set": {"loop": False, "alert": False}}),
             ("notifications.categories", {}), ("notifications.categories", {"set": {"nope": True, "zz": False}}),
             ("notifications.categories", {"set": {"loop": "x"}}), ("notifications.categories", {"set": {}}),
             ("notifications.categories", {"set": "x"}))
    sc.s.close(c)
    return sc.finish()


def sc_jev() -> dict[str, Any]:
    """Jev's switches while Jev is off (the default): nothing reads a key or calls Jev."""
    sc = common.BScenario("small_jev", "Jev's switches with Jev off", ENTRY + ["ferb/daemon/areas/jev.py"])
    c = sc.conn("app")
    sc.calls(c, ("jev.enable", {"question": "trail.relevance"}),
             ("jev.enable", {"question": "trail.attention", "on": False}),
             ("jev.enable", {"question": "action.destructive"}),
             ("jev.enable", {"question": "action.destructive", "on": False}),
             ("jev.enable", {"question": "memory.worth_keeping", "on": 0}),
             ("jev.enable", {}), ("jev.reload", {"ping": True}), ("jev.stats", {}))
    sc.s.close(c)
    return sc.finish()


SCAN = {"state": "seen", "summary": "An invented   Mac used for writing\nand sketches.",
        "facts": [{"text": "Uses  TextEdit daily"}, {"text": ""}, "not a dict", {"text": "Keeps notes in ~/notes"}]}
ANSWERS = [
    {"requests": [{"request": "Summarize   my notes folder", "app": "Finder"},
                  {"request": "summarize my notes  folder", "app": "TextEdit"},
                  {"request": "Find " + "the longest invented sketch file " * 12, "app": "Preview" * 15}]},
    {"requests": [{"request": "Compare three well-reviewed electric kettles on the web and tell me which is the "
                              "best value.", "app": ""}]},
    {"requests": []},
]


def sc_intro() -> dict[str, Any]:
    """intro.suggest with a seen scan: the model's requests deduplicated, cut and topped up (three calls)."""
    model = H.Model(planner=[json.dumps(a) for a in ANSWERS])

    def prepare(d: Any) -> None:
        from types import SimpleNamespace
        d._mac_profile_service = lambda: SimpleNamespace(status=lambda: json.loads(json.dumps(SCAN)))
        make = d._make_runtime

        class Named:
            """The harness's scripted runtime keeps its recorder in `model`; a span records the model's name."""
            model = "fake-model"

            def __init__(self, rt: Any) -> None:
                self._rt = rt

            def __getattr__(self, name: str) -> Any:
                return getattr(self._rt, name)
        d._make_runtime = lambda *a, **k: Named(make(*a, **k))

    sc = common.BScenario("small_intro", "intro.suggest from a seen scan", ENTRY + ["ferb/daemon/areas/intro.py"],
                          before_start=[prepare], setup={"scan": SCAN, "answers": ANSWERS}, model=model)
    c = sc.conn("app")
    sc.calls(c, ("intro.suggest", {}), ("intro.suggest", {}), ("intro.suggest", {}))
    sc.s.close(c)
    return sc.finish()


TOLD = [
    [{"task_id": "t1", "status": "done", "answer": "Done: the invented report is ready.", "goal": "Make a report",
      "name": "Invented report", "cards": [{"type": "file", "path": "/x"}, "not a card"],
      "at": "2026-01-01T08:59:00+00:00"}, False, False, True],
    [{"task_id": "t2", "status": "failed", "summary": "It failed.", "goal": "Fail"}, True, False, True],
    [{"task_id": "t3", "status": "cancelled", "answer": "Stopped.", "goal": "Stop"}, False, False, True],
    [{"task_id": "t4", "status": "done", "answer": "The watched price dropped.", "goal": "Watch", "name": "Price"},
     False, True, True],
    [{"task_id": "t5", "status": "done", "answer": "Nothing to tell.", "goal": "Check"}, False, False, False],
    [{"task_id": "t6", "status": "done", "answer": "Unnamed result.", "goal": "Plain", "request_id": "req-6"},
     False, False, True],
]
QUESTIONS = [{"task_id": "t1.s1", "approval_id": "ap-1", "summary": "Send the invented note?"},
             {"task_id": "t2", "approval_id": "ap-2", "reason": "Delete the invented file?"},
             {"capability": "setup.persona", "approval_id": "ap-3", "summary": "x"}]


def sc_tell() -> dict[str, Any]:
    """Results, questions and notices told while FERB.app holds a poster: what it receives, the rows kept, the
    events published."""
    import uuid
    counter = iter(range(1, 100))
    common.H.MP.setattr(uuid, "uuid4", lambda: uuid.UUID(int=next(counter)))
    sc = common.BScenario("small_tell", "Telling the owner with FERB.app as the poster", ENTRY + [
        "ferb/daemon/areas/notify.py:33 _task_result, :64 _approval_notice, :209 _tell_owner, :223 _post_notice",
        "ferb/daemon/banners.py"], setup={"told": TOLD, "questions": QUESTIONS})
    d = sc.d
    queue = on_loop(sc, lambda: d.posters.open())

    async def tell() -> None:
        for ev, shown, alert, publish in TOLD:
            await d._task_result(dict(ev), shown, alert=alert, publish=publish)
        for q in QUESTIONS:
            d._approval_notice(dict(q))
        await d._tell_owner("loop-s9-0009", ["task:t9"], "Backing off the invented loop.", "ferb", category="loop",
                            task_id="t9")
    sc.host.run(tell())
    H.settle(sc.host)
    items = on_loop(sc, lambda: [queue.get_nowait() for _ in range(queue.qsize())])
    rows = on_loop(sc, lambda: [list(r) for r in d.db.conn.execute(
        "SELECT r.task_id, r.at, r.status, r.goal, r.answer, r.shown, n.name, c.cards_json FROM task_results r "
        "LEFT JOIN task_result_names n USING (task_id) LEFT JOIN task_result_cards c USING (task_id) "
        "ORDER BY r.task_id")])
    out = sc.finish()
    out["delivered"] = items
    out["task_results"] = rows
    return out


if __name__ == "__main__":
    common.run("small", "small.json", [sc_about, sc_notices, sc_jev, sc_intro, sc_tell])
