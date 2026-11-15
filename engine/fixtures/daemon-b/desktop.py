"""Fixtures for the Rust port of ferb/daemon/areas/desktop.py (ENG-3): the desktop observer's records, a task's
released resources at its end, a pause and a resume, a finished tree, whether the owner used an app FERB launched,
FERB's shell processes, the native page reads and window capture, Automation and the front tab, an app's change in the
device profile, permissions and apps. The real mixin methods run over a fake daemon (`Fake`) whose every other part
answers from the case's `state` and logs what it is handed.

    cd <repo> && uv run python rust/fixtures/daemon-b/desktop.py

Writes desktop.json beside this file: `cases`, each `{name, method, args, state, out, log, journal, published,
files}`: `log` is every call the fakes received, in order; `journal` the rows appended (redacted as the journal
stores them); `published` the events; `files` what the device profile store holds after the case (written
`/HOME/...`). Exceptions are recorded as `{"raised": "<Type>: <text>"}`. The monotonic clock is a counter the case
moves (`mono` steps). Invented data only; the same bytes on every run.
"""

import asyncio
import json
import os
import shutil
import sqlite3
import sys
import tempfile
import time
from concurrent.futures import Future
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
os.environ["TZ"] = "UTC"
time.tzset()
sys.path.insert(0, str(REPO))

import ferb.act.mac.apps as appsm  # noqa: E402
import ferb.act.mac.screenshot as screenshot  # noqa: E402
import ferb.daemon.areas.desktop as desktop  # noqa: E402
import ferb.device.permissions as permissions  # noqa: E402
import ferb.sense.watchers.browser_tabs as browser_tabs  # noqa: E402
from ferb.config import Config  # noqa: E402
from ferb.core.errors import NoTarget, Refused, Withheld  # noqa: E402
from ferb.device.profile import AppInfo, DeviceProfile, ProfileStore  # noqa: E402
from ferb.sanitize.redact import Redactor  # noqa: E402

ROOT = Path(tempfile.gettempdir()) / "fxb-desktop"
ERRORS = {"RuntimeError": RuntimeError, "ValueError": ValueError, "KeyError": KeyError, "OSError": OSError,
          "NoTarget": NoTarget, "Withheld": Withheld, "Refused": Refused, "TimeoutError": TimeoutError}


def js(v: Any) -> Any:
    return json.loads(json.dumps(v, default=list))


def boom(spec: str, **attrs: Any) -> BaseException:
    kind, _, text = spec.partition(": ")
    e = ERRORS[kind](text)
    for k, v in attrs.items():
        setattr(e, k, v)
    return e


class World:
    def __init__(self, st: dict[str, Any]) -> None:
        self.st = st
        self.log: list[Any] = []
        self.mono = 1000.0


class Journal:
    def __init__(self, w: World, redact: Any) -> None:
        self.w, self.redact, self.rows = w, redact, []

    def append(self, kind: str, data: dict[str, Any]) -> None:
        self.rows.append([kind, js(self.redact(data))])

    def latest_kind(self, kinds: Any, key: str, value: str) -> str | None:
        self.w.log.append(["journal.latest_kind", list(kinds), key, value])
        return self.w.st.get("latest_kind", {}).get(value)  # type: ignore[no-any-return]

    def of_kind(self, kind: str, key: str, value: str) -> list[Any]:
        self.w.log.append(["journal.of_kind", kind, key, value])
        return [SimpleNamespace(data=d) for d in self.w.st.get("of_kind", {}).get(value, [])]


class Spans:
    def __init__(self, w: World) -> None:
        self.w, self.n = w, 0

    def start(self, kind: str, task_id: str, parent_id: Any, input: Any) -> Any:
        self.n += 1
        self.w.log.append(["spans.start", kind, task_id, parent_id, input])
        return SimpleNamespace(id=f"span-{self.n}")

    def finish(self, span: Any, output: Any) -> None:
        self.w.log.append(["spans.finish", span.id, js(output)])


def ender(w: World, name: str, table: dict[str, Any]) -> Any:
    async def end_task(task_id: str) -> list[Any]:
        w.log.append([f"{name}.end_task", task_id])
        return [dict(x) for x in table.get(task_id, [])]
    return end_task


class Driver:
    def __init__(self, w: World, name: str, st: dict[str, Any]) -> None:
        self.w, self.name, self.st = w, name, st

    async def release(self, task_id: str) -> Any:
        self.w.log.append([f"{self.name}.release", task_id])
        if "raise" in self.st:
            raise boom(self.st["raise"], facts={"tabs": self.st["tabs"]} if "tabs" in self.st else {})
        return None if self.st.get("none") else [dict(x) for x in self.st.get("release", [])]


class Shells:
    def __init__(self, w: World, st: dict[str, Any]) -> None:
        self.w, self.st = w, st

    async def end_task(self, task_id: str) -> None:
        self.w.log.append(["shells.end_task", task_id])
        if "end_raise" in self.st:
            raise boom(self.st["end_raise"])

    async def stop_running(self, mine: Any) -> int:
        owners = self.st.get("running_owners", [])
        self.w.log.append(["shells.stop_running", [o for o in owners if mine(o)]])
        stop = self.st.get("stop", 0)
        if isinstance(stop, str):
            raise boom(stop)
        return int(stop)

    def list(self) -> list[Any]:
        return [SimpleNamespace(**p) for p in self.st.get("procs", [])]


class Mac:
    def __init__(self, w: World, st: dict[str, Any]) -> None:
        self.w, self.st = w, st
        self.launched = None
        if "launched" in st:
            ls = st["launched"]

            async def release(task_id: str) -> list[Any]:
                w.log.append(["launched.release", task_id])
                if "raise" in ls:
                    raise boom(ls["raise"])
                return [dict(x) for x in ls.get("release", [])]
            self.launched = SimpleNamespace(release=release)

    async def page_secrets(self, pid: int, bounds: Any, title: str) -> Any:
        self.w.log.append(["mac.page_secrets", pid, list(bounds), title])
        if "page_secrets_raise" in self.st:
            raise boom(self.st["page_secrets_raise"])
        return self.st.get("page_secrets")

    async def place_on_web_area(self, rect: Any, viewport: Any, pid: int, bounds: Any, title: str) -> Any:
        self.w.log.append(["mac.place_on_web_area", list(rect), list(viewport), pid, list(bounds), title])
        p = self.st.get("place")
        return tuple(p) if p else None

    def snapshot_images(self) -> bool:
        return bool(self.st.get("snapshot_images", True))

    async def window_image(self, target: str, window: str) -> Any:
        self.w.log.append(["mac.window_image", target, window])
        if "image_raise" in self.st:
            raise boom(self.st["image_raise"])
        return None, SimpleNamespace(masking=self.st.get("masking", "")), self.st.get("jpeg", "jpeg").encode()


class Fake(desktop.Desktop):
    def __init__(self, w: World, home: Path) -> None:
        st = w.st
        self.w = w
        self.home = home
        self.config = Config(home)
        self.redact = Redactor()
        self.jr = Journal(w, self.redact)
        self.journal = self.jr
        self.published: list[Any] = []
        tasks = st.get("tasks", {})
        self.tasks = None if st.get("no_tasks") else SimpleNamespace(
            get=lambda tid: None if tid not in tasks else SimpleNamespace(
                span_id=tasks[tid].get("span_id"), tree_id=tasks[tid].get("tree_id")))
        self.spans = Spans(w) if st.get("spans", True) else None
        hearing, recordings = st.get("hearing", {}), st.get("recordings", {})
        self.hearing = SimpleNamespace(end_task=ender(w, "hearing", hearing.get("end", {})),
                                       listening={str(i): SimpleNamespace(owner=o)
                                                  for i, o in enumerate(hearing.get("owners", []))})
        self.recordings = SimpleNamespace(end_task=ender(w, "recordings", recordings.get("end", {})),
                                          records={str(i): SimpleNamespace(owner=o)
                                                   for i, o in enumerate(recordings.get("owners", []))})
        self.mac = Mac(w, st["mac"]) if st.get("mac") is not None else None
        self.identity_browser = Driver(w, "identity_browser", st["identity_browser"]) \
            if st.get("identity_browser") is not None else None
        self.router = Driver(w, "router", st["router"]) if st.get("router") is not None else None
        if st.get("downloads"):
            self.downloads = SimpleNamespace(forget_task=lambda t: w.log.append(["downloads.forget_task", t]))
        self.shells = Shells(w, st["shells"]) if st.get("shells") is not None else None
        sched = st.get("schedules")
        self.schedules = None if sched is None else SimpleNamespace(
            get=lambda sid: SimpleNamespace(id=sid, state=sched[sid]) if sid in sched else None)
        self.tokens = SimpleNamespace(revoke_tree=lambda t: w.log.append(["tokens.revoke_tree", t]))

        async def finish(tree: str) -> None:
            w.log.append(["disturbance.finish", tree])
            if "disturbance_raise" in st:
                raise boom(st["disturbance_raise"])
        self.disturbance = SimpleNamespace(finish=finish)
        changes = st.get("changes")
        self.changes = None if changes is None else SimpleNamespace(
            forget=lambda t: (w.log.append(["changes.forget", t]), list(changes.get("stuck", [])))[1])
        self.uploads = SimpleNamespace(forget_tree="forget_tree") if st.get("uploads") else None
        self.db = None
        if "spans_rows" in st:
            conn = sqlite3.connect(":memory:")
            conn.execute("CREATE TABLE spans (task_id TEXT, kind TEXT, started_at TEXT, output_json TEXT)")
            conn.executemany("INSERT INTO spans VALUES (?, ?, ?, ?)", [tuple(r) for r in st["spans_rows"]])
            self.db = SimpleNamespace(conn=conn)
        self._automation_cache: dict[str, Any] = {}
        self._automation_checks: dict[str, Any] = {}
        self._tab_granted: set[str] = set()

    def _publish(self, topic: str, data: dict[str, Any]) -> None:
        self.published.append([topic, js(self.redact(data))])

    def _journal(self) -> Any:
        return self.jr

    def _schedule_change(self, sid: str, action: str, with_task: str) -> None:
        self.w.log.append(["schedule_change", sid, action, with_task])

    async def _clean_uploads(self, forget: Any, tree: str, about: dict[str, Any]) -> None:
        self.w.log.append(["clean_uploads", forget, tree, about])

    def _redact_text(self, text: str) -> str:
        return self.redact(text)  # type: ignore[no-any-return]

    def _spawn(self, coro: Any, name: str) -> None:
        self.w.log.append(["spawn", name])
        asyncio.run(coro)


def run(case: dict[str, Any]) -> dict[str, Any]:
    if ROOT.exists():
        shutil.rmtree(ROOT)
    home = ROOT / "home"
    home.mkdir(parents=True, mode=0o700)
    (home / "config.toml").write_text(case.get("config", ""))
    (home / "config.toml").chmod(0o600)
    for rel, text in case.get("files", {}).items():
        p = home / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    st = case.get("state", {})
    w = World(st)
    d = Fake(w, home)

    # the Mac's natives, from the state
    def normal_windows() -> list[Any]:
        w.log.append(["screenshot.normal_windows"])
        if "windows_raise" in st:
            raise boom(st["windows_raise"])
        return [screenshot.Window(x["id"], x["pid"], tuple(x["bounds"]), x["title"]) for x in st.get("windows", [])]
    screenshot.normal_windows = normal_windows

    def window(wid: int) -> Any:
        w.log.append(["screenshot.window", wid])
        x = st.get("window_now")
        return None if x is None else screenshot.Window(x["id"], x["pid"], tuple(x["bounds"]), x["title"])
    screenshot.window = window

    def running() -> list[Any]:
        if st.get("running") == "raise":
            raise RuntimeError("no workspace")
        return [SimpleNamespace(bundle_id=a[0], name=a[1], pid=a[2]) for a in st.get("running", [])]
    appsm.running = running

    perms = st.get("permissions", {})

    def perm(name: str) -> Any:
        def read() -> Any:
            v = perms.get(name, "granted")
            if v == "raise":
                raise RuntimeError("tcc")
            return permissions.Status(v)
        return read
    permissions.accessibility = perm("accessibility")
    permissions.screen_recording = perm("screen")

    probes = {k: list(v) for k, v in st.get("probes", {}).items()}

    def automation_check(bundle_id: str, ask: bool = False) -> Future[Any]:
        w.log.append(["automation_check", bundle_id, ask])
        f: Future[Any] = Future()
        answer = probes[bundle_id].pop(0) if probes.get(bundle_id) else "pending"
        if answer != "pending":
            f.set_result(permissions.Status(answer))
        w.pending = (bundle_id, f)            # type: ignore[attr-defined]
        return f
    permissions.automation_check = automation_check

    def applescript_windows(bundle_id: str) -> Any:
        w.log.append(["applescript_windows", bundle_id])
        v = st.get("scripted", {}).get(bundle_id)
        if v == "raise":
            raise OSError("osascript failed")
        return None if v is None else [browser_tabs.ScriptedWindow(
            tuple(x["bounds"]) if x["bounds"] else None, x["name"], x["url"], x["title"]) for x in v]
    browser_tabs.applescript_windows = applescript_windows
    clocked = any(s[0] in ("_automation_granted", "_front_tab") for s in case["steps"])
    if clocked:
        time.monotonic = lambda: w.mono      # type: ignore[assignment]
    try:
        outs = steps(case, w, d)
    finally:
        time.monotonic = MONOTONIC           # type: ignore[assignment]
    files = {}
    dev = home / "device"
    if dev.exists():
        for p in sorted(dev.iterdir()):
            files[f"device/{p.name}"] = p.read_text()
    return {"name": case["name"], "config": case.get("config", ""), "start_files": case.get("files", {}),
            "state": st, "steps": case["steps"], "out": outs, "log": w.log, "journal": d.jr.rows,
            "published": d.published, "files": files,
            "after": {"automation_cache": sorted(d._automation_cache), "automation_checks":
                      sorted(d._automation_checks), "tab_granted": sorted(d._tab_granted)}}


MONOTONIC = time.monotonic


def steps(case: dict[str, Any], w: World, d: Fake) -> list[Any]:

    outs: list[Any] = []
    for step in case["steps"]:
        method, args = step[0], step[1:]
        if method == "mono":
            w.mono += args[0]
            outs.append(None)
            continue
        if method == "resolve":                   # the pending probe answers now
            bundle_id, answer = args
            d._automation_checks[bundle_id].set_result(permissions.Status(answer))
            outs.append(None)
            continue
        call_args = list(args)
        if method == "_owner_used_app":
            call_args[1] = SimpleNamespace(pid=args[1]["pid"], at=args[1]["at"])
        if method == "_front_tab":
            x = args[1]
            call_args[1] = screenshot.Window(x["id"], x["pid"], tuple(x["bounds"]), x["title"])
        if method in ("_native_page_secrets", "_native_window_capture"):
            call_args[1] = tuple(args[1])
        if method == "_native_place":
            call_args = [tuple(args[0]), tuple(args[1]), args[2], tuple(args[3]), args[4]]
        if method == "_app_changed":
            info = args[1]
            call_args[1] = None if info is None else AppInfo(**info)
        try:
            out = getattr(d, method)(*call_args)
            if asyncio.iscoroutine(out):
                out = asyncio.run(out)
            if isinstance(out, tuple) and method == "_native_window_capture":
                out = [None if out[0] is None else out[0].decode(), out[1]]
            elif method == "_device_profile":
                out = None if out is None else json.loads(out.to_json())
            out = js(out)
        except Exception as e:
            out = {"raised": f"{type(e).__name__}: {e}"}
        outs.append(out)
    return outs


CASES: list[dict[str, Any]] = []


def case(name: str, steps: list[Any], state: dict[str, Any] | None = None, config: str = "",
         files: dict[str, str] | None = None) -> None:
    CASES.append({"name": name, "steps": steps, "state": state or {}, "config": config, "files": files or {}})


# ---- _record_disturbance
TASKS = {"t1": {"span_id": "sp-root", "tree_id": "t1"}, "t1.w1": {"span_id": None, "tree_id": "t1"},
         "t2.w": {"tree_id": None}}
case("disturbance_change", [["_record_disturbance", "t1", "disturbance.change",
                             {"changes": {"frontmost_app": {"before": {"pid": 1}, "after": {"pid": 2}}},
                              "note": "card 4111 1111 1111 1111"}]], {"tasks": TASKS})
case("disturbance_error_no_task", [["_record_disturbance", "t9", "disturbance.error", {"error": "RuntimeError"}]],
     {"tasks": TASKS})
case("disturbance_summary", [["_record_disturbance", "t1.w1", "disturbance.summary", {"samples": 3}]],
     {"tasks": TASKS})
case("disturbance_no_spans", [["_record_disturbance", "t1", "disturbance.change", {"x": 1}]], {"spans": False})

# ---- _task_end
case("task_end_all", [["_task_end", "t1.w1"]], {
    "hearing": {"end": {"t1.w1": [{"resource": "listen", "released": True}]}},
    "recordings": {"end": {"t1.w1": [{"resource": "recording", "released": True, "deleted": 1}]}},
    "mac": {"launched": {"release": [{"resource": "app", "released": True, "name": "TextEdit"}]}},
    "identity_browser": {"release": [{"resource": "ferb_browser", "released": True}]},
    "router": {"none": True}, "downloads": True, "shells": {}})
case("task_end_failures", [["_task_end", "t1"]], {
    "mac": {"launched": {"raise": "RuntimeError: quit refused"}},
    "identity_browser": {"raise": "TimeoutError: no answer", "tabs": [{"tab_id": 3, "url": "https://a.test/"}]},
    "router": {"raise": "ValueError: bad", "tabs": []}, "shells": {"end_raise": "OSError: kill failed"}})
case("task_end_router_error_no_facts", [["_task_end", "t1"]], {"router": {"raise": "KeyError: tab"}})
case("task_end_nothing", [["_task_end", "t1"]], {"mac": {}})

# ---- _task_paused / _task_resumed
case("paused_full", [["_task_paused", {"task_id": "t1", "schedule_id": "s-1"}]], {
    "shells": {"running_owners": ["t1.w1", "t10.w", "t1", None, "t2"], "stop": 2},
    "hearing": {"owners": ["t1.w2", "t1.w1", "t2.w", None, "t1.w1"],
                "end": {"t1.w1": [{"resource": "listen", "stopped": True}], "t1.w2": []}},
    "recordings": {"owners": ["t1"], "end": {"t1": [{"resource": "recording", "deleted": True}]}},
    "schedules": {"s-1": "active"}})
case("paused_shell_error_schedule_paused", [["_task_paused", {"task_id": "t1", "schedule_id": "s-1"}]], {
    "shells": {"stop": "RuntimeError: stuck"}, "schedules": {"s-1": "paused"}})
case("paused_nothing", [["_task_paused", {"task_id": "t1"}], ["_task_paused", {}],
                        ["_task_paused", {"task_id": "t2", "schedule_id": "gone"}]],
     {"shells": {"stop": 0}, "schedules": {}})
case("paused_no_stores", [["_task_paused", {"task_id": "t1", "schedule_id": "s-1"}]], {})
R = {"schedules": {"s-1": "paused", "s-2": "active"}, "latest_kind": {"s-1": "schedule.pause", "s-3": "schedule.pause"},
     "of_kind": {"s-1": [{"with_task": "t0"}, {"with_task": "t1"}], "s-3": [{"with_task": "t1"}]}}
case("resumed_yes", [["_task_resumed", {"task_id": "t1", "schedule_id": "s-1"}]], R)
case("resumed_other_task", [["_task_resumed", {"task_id": "t0", "schedule_id": "s-1"}]], R)
case("resumed_active", [["_task_resumed", {"task_id": "t1", "schedule_id": "s-2"}]], R)
case("resumed_gone", [["_task_resumed", {"task_id": "t1", "schedule_id": "s-3"}]], R)
case("resumed_owner_resumed", [["_task_resumed", {"task_id": "t1", "schedule_id": "s-1"}]],
     {**R, "latest_kind": {"s-1": "schedule.resume"}})
case("resumed_no_sid", [["_task_resumed", {"task_id": "t1"}], ["_task_resumed", {"schedule_id": ""}]], R)
case("resumed_no_store", [["_task_resumed", {"task_id": "t1", "schedule_id": "s-1"}]], {})

# ---- _tree_finished
case("tree_finished_all", [["_tree_finished", "t1"]], {"changes": {"stuck": ["/a", "/b"]}, "uploads": True})
case("tree_finished_clean", [["_tree_finished", "t1"]], {"changes": {"stuck": []}})
case("tree_finished_raises", [["_tree_finished", "t1"]], {"disturbance_raise": "RuntimeError: probe",
                                                          "changes": {"stuck": ["/x"]}, "uploads": True})
case("tree_finished_bare", [["_tree_finished", "t1"]], {})

# ---- _owner_used_app
SPANS = [
    ["t1", "disturbance.change", "2026-01-01T09:00:05+00:00",
     json.dumps({"changes": {"frontmost_app": {"after": {"pid": 501}}}, "actions": [{"on_screen": True}]})],
    ["t1", "disturbance.change", "2026-01-01T09:00:06+00:00",
     json.dumps({"changes": {"frontmost_app": {"after": {"pid": 502}}}, "actions": [{"on_screen": False}, "x"]})],
    ["t1", "disturbance.change", "2026-01-01T08:00:00+00:00",
     json.dumps({"changes": {"frontmost_app": {"after": {"pid": 503}}}})],
    ["t1", "disturbance.sample", "2026-01-01T09:00:07+00:00",
     json.dumps({"changes": {"frontmost_app": {"after": {"pid": 503}}}})],
    ["t2", "disturbance.change", "2026-01-01T09:00:07+00:00",
     json.dumps({"changes": {"frontmost_app": {"after": {"pid": 504}}}, "actions": None})],
    ["t2", "disturbance.change", "2026-01-01T09:00:08+00:00", None],
]
AT = "2026-01-01T09:00:00+00:00"
case("owner_used_app", [["_owner_used_app", "t1.w1", {"pid": p, "at": AT}] for p in (501, 502, 503)]
     + [["_owner_used_app", "t2.w", {"pid": 504, "at": AT}], ["_owner_used_app", "t9.x", {"pid": 504, "at": AT}],
        ["_owner_used_app", "t2", {"pid": 504, "at": "2026-01-01T10:00:00+00:00"}]],
     {"tasks": {"t1.w1": {"tree_id": "t1"}, "t2.w": {"tree_id": None}}, "spans_rows": SPANS})

# ---- shell processes
PROCS = [{"id": "p-1", "owner_task": "t1.w1", "command": "curl -H 'Authorization: Bearer invented-token-abcdef123456'",
          "long_lived": False, "running": True, "started": 1767258000.5},
         {"id": "p-2", "owner_task": "t1", "command": "sleep 5", "long_lived": True, "running": False,
          "started": 1767258001.0},
         {"id": "p-3", "owner_task": None, "command": "npm run dev", "long_lived": True, "running": True,
          "started": 12.0}]
case("running_processes", [["_running_processes"]], {"shells": {"procs": PROCS}})
case("running_processes_none", [["_running_processes"]], {})
case("shell_process", [["_shell_process", "p-1"], ["_shell_process", "p-3"], ["_shell_process", "nope"]],
     {"shells": {"procs": PROCS}})
case("shell_process_no_shells", [["_shell_process", "p-1"]], {})

# ---- native page reads and the window capture
B = [10.0, 20.0, 800.0, 600.0]
case("page_secrets", [["_native_page_secrets", 77, B, "Sign in"]],
     {"mac": {"page_secrets": [{"x": 1, "y": 2, "w": 3, "h": 4}]}})
case("page_secrets_fails", [["_native_page_secrets", 77, B, "Sign in"]],
     {"mac": {"page_secrets_raise": "OSError: denied"}})
case("page_secrets_no_mac", [["_native_page_secrets", 77, B, "Sign in"]], {})
RECT, VIEW = [1.0, 2.5, 3.0, 4.0], [0.0, 0.0, 800.0, 600.0]
case("place", [["_native_place", RECT, VIEW, 77, B, "Page"]], {"mac": {"place": [5.0, 6.25, 7.0, 8.0]}})
case("place_none", [["_native_place", RECT, VIEW, 77, B, "Page"]], {"mac": {}})
case("place_no_mac", [["_native_place", RECT, VIEW, 77, B, "Page"]], {})
W1 = {"id": 41, "pid": 77, "bounds": [11.0, 21.5, 799.0, 600.0], "title": "Invented  page "}
W2 = {"id": 42, "pid": 77, "bounds": [10.0, 20.0, 800.0, 600.0], "title": "Other"}
W3 = {"id": 43, "pid": 78, "bounds": B, "title": "Invented page"}
W4 = {"id": 44, "pid": 77, "bounds": [13.0, 20.0, 800.0, 600.0], "title": "Invented page"}
for name, st, title in [
    ("capture_ok", {"windows": [W1, W2, W3, W4], "window_now": W1, "masking": "masked from AX"}, "Invented page"),
    ("capture_no_title", {"windows": [W1, W2], "window_now": W1}, ""),
    ("capture_only_one_untitled", {"windows": [W2, W3], "window_now": W2, "masking": ""}, ""),
    ("capture_none_near", {"windows": [W3, W4]}, "Invented page"),
    ("capture_two", {"windows": [W1, dict(W1, id=45)]}, "Invented page"),
    ("capture_changed", {"windows": [W1], "window_now": W2}, "Invented page"),
    ("capture_gone_now", {"windows": [W1]}, "Invented page"),
    ("capture_no_target", {"windows": [W1], "image_raise": "NoTarget: gone"}, "Invented page"),
    ("capture_withheld", {"windows": [W1], "image_raise": "Withheld: pixels withheld: masks failed; pixels withheld"},
     "Invented page"),
    ("capture_refused", {"windows": [W1], "image_raise": "Refused: FERB doesn't look at that app"}, "Invented page"),
    ("capture_fails", {"windows_raise": "OSError: window list"}, "Invented page"),
    ("capture_off", {"snapshot_images": False}, "Invented page"),
]:
    mac = {k: st.pop(k) for k in ("masking", "image_raise", "snapshot_images") if k in st}
    case(name, [["_native_window_capture", 77, B, title]], {**st, "mac": mac})
case("capture_no_mac", [["_native_window_capture", 77, B, "x"]], {})

# ---- _automation_granted and _front_tab
case("automation", [["_automation_granted", "com.google.Chrome"], ["resolve", "com.google.Chrome", "granted"],
                    ["_automation_granted", "com.google.Chrome"], ["mono", 30], ["_automation_granted",
                                                                                 "com.google.Chrome"],
                    ["mono", 31], ["_automation_granted", "com.google.Chrome"],
                    ["_automation_granted", "com.apple.Safari"], ["_automation_granted", "com.apple.Safari"]],
     {"probes": {"com.google.Chrome": ["pending", "denied"], "com.apple.Safari": ["not_determined"]}})
SW = [{"bounds": [11.0, 21.0, 799.0, 600.0], "name": "Invented page", "url": "https://example.test/p",
       "title": "Invented page"},
      {"bounds": [500.0, 20.0, 800.0, 600.0], "name": "Else", "url": "https://else.test/", "title": "Else"}]
FW = {"id": 41, "pid": 77, "bounds": [10.0, 20.0, 800.0, 600.0], "title": "Invented page"}
case("front_tab_granted", [["_front_tab", "com.google.Chrome", FW], ["_front_tab", "com.google.Chrome", FW, True]],
     {"probes": {"com.google.Chrome": ["granted"]}, "scripted": {"com.google.Chrome": SW}})
case("front_tab_not_browser", [["_front_tab", "com.apple.TextEdit", FW]], {})
case("front_tab_pending_then_after_shot", [
    ["_front_tab", "com.google.Chrome", FW], ["_front_tab", "com.google.Chrome", FW, True]],
    {"probes": {"com.google.Chrome": ["pending"]}, "scripted": {"com.google.Chrome": SW}})
case("front_tab_kept_grant_while_pending", [
    ["_front_tab", "com.google.Chrome", FW], ["mono", 61], ["_front_tab", "com.google.Chrome", FW],
    ["_front_tab", "com.google.Chrome", FW, True], ["resolve", "com.google.Chrome", "denied"],
    ["_front_tab", "com.google.Chrome", FW, True]],
    {"probes": {"com.google.Chrome": ["granted", "pending"]}, "scripted": {"com.google.Chrome": SW}})
case("front_tab_script_fails", [["_front_tab", "com.google.Chrome", FW], ["_front_tab", "com.google.Chrome", FW]],
     {"probes": {"com.google.Chrome": ["granted"]}, "scripted": {"com.google.Chrome": "raise"}})
case("front_tab_lists_none", [["_front_tab", "com.google.Chrome", FW]],
     {"probes": {"com.google.Chrome": ["granted"]}})
case("front_tab_no_url", [["_front_tab", "com.google.Chrome", FW]],
     {"probes": {"com.google.Chrome": ["granted"]}, "scripted": {"com.google.Chrome": [dict(SW[0], url="")]}})
case("front_tab_ambiguous", [["_front_tab", "com.google.Chrome", FW]],
     {"probes": {"com.google.Chrome": ["granted"]},
      "scripted": {"com.google.Chrome": [SW[0], dict(SW[0], name="Two")]}})
case("front_tab_nowhere", [["_front_tab", "com.google.Chrome", FW]],
     {"probes": {"com.google.Chrome": ["granted"]}, "scripted": {"com.google.Chrome": SW[1:]}})

# ---- the device profile: _app_changed, _device_profile, _installed_apps
PROFILE = DeviceProfile("mac-1", "macOS", "26.1", "2026-01-01T09:00:00+00:00",
                        [AppInfo("com.apple.TextEdit", "TextEdit", "1.0", "/Applications/TextEdit.app", True),
                         AppInfo("com.example.old", "Old", "2", "/Applications/Old.app", False, "ok"),
                         AppInfo("com.apple.finder", "Finder", "14", "/System/Finder.app", True)], [])
PFILES = {"device/mac-1.json": PROFILE.to_json() + "\n"}
NEW = {"bundle_id": "com.example.new", "name": "Nouveau", "version": "3.1", "path": "/Applications/New.app",
       "scriptable": False}
case("app_added", [["_app_changed", "added", NEW, "/Applications/New.app"]], {}, '[device]\nid = "mac-1"\n', PFILES)
case("app_updated", [["_app_changed", "changed", dict(NEW, bundle_id="com.example.old", version="3"),
                      "/Applications/Old2.app"]], {}, '[device]\nid = "mac-1"\n', PFILES)
case("app_removed", [["_app_changed", "removed", None, "/Applications/Old.app"]], {}, '[device]\nid = "mac-1"\n',
     PFILES)
case("app_removed_unknown", [["_app_changed", "removed", None, "/Applications/Never.app"]], {},
     '[device]\nid = "mac-1"\n', PFILES)
case("app_no_profile", [["_app_changed", "added", NEW, "/Applications/New.app"]], {}, '[device]\nid = "mac-9"\n',
     PFILES)
case("app_no_device_id", [["_app_changed", "added", NEW, "/Applications/New.app"]], {}, "", PFILES)
case("device_profile", [["_device_profile"], ["_installed_apps"]], {}, '[device]\nid = "mac-1"\n', PFILES)
case("device_profile_none", [["_device_profile"], ["_installed_apps"]], {}, "", PFILES)
case("device_profile_missing", [["_device_profile"], ["_installed_apps"]], {}, '[device]\nid = "mac-2"\n', PFILES)

# ---- permissions and apps
case("permissions", [["_permissions"]], {"permissions": {"accessibility": "granted", "screen": "denied"}})
case("permissions_unknown", [["_permissions"]], {"permissions": {"accessibility": "unknown",
                                                                 "screen": "not_determined"}})
case("permissions_raise", [["_permissions"]], {"permissions": {"screen": "raise"}})
case("running_apps", [["_running_apps"]], {"running": [["com.apple.finder", "Finder", 4101],
                                                      ["com.example.x", "Ünï", 4102]]})
case("running_apps_raise", [["_running_apps"]], {"running": "raise"})


def main() -> None:
    out = []
    try:
        for c in CASES:
            out.append(run(c))
    finally:
        shutil.rmtree(ROOT, ignore_errors=True)
    text = json.dumps({"cases": out}, ensure_ascii=False, indent=1)
    (HERE / "desktop.json").write_text(text + "\n", encoding="utf-8")
    print(f"desktop.json: {len(out)} cases, {len(text)} bytes")


if __name__ == "__main__":
    main()
