"""scripts/question_run.py (GATE-551): what the task-run tool reports per task, and its own limits. Fakes stand in for
FERB's socket; nothing reaches the owner's FERB."""
import importlib.util
import json
import sqlite3
import subprocess
import threading
from pathlib import Path
from typing import Any, NamedTuple

import pytest

SPEC = importlib.util.spec_from_file_location("question_run", Path(__file__).parent.parent / "scripts/question_run.py")
assert SPEC is not None and SPEC.loader is not None
qr = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(qr)


class _Trace:
    def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        assert method == "trace.get"
        span = {"started_at": "2026-11-08T10:00:00+00:00", "ended_at": "2026-11-08T10:00:04+00:00", "children": []}
        return {"spans": [
            span | {"id": "1", "kind": "tool.call", "input": {"tool": "browser.act", "kind": "click",
                                                              "target": {"site": "shop.example"}}},
            span | {"id": "2", "kind": "tool.call", "input": {"tool": "browser.read", "kind": "read"}},
            span | {"id": "3", "kind": "model.usage", "output": {
                "model": "claude-opus-5-5", "typed": "strict", "input_tokens": 100, "cached_input_tokens": 80,
                "cache_write_1h_tokens": 20, "output_tokens": 5, "cost_usd": 0.001, "timing": {"total_ms": 2400}}}],
            "episode": {"task_id": "t1", "status": "done", "answer": "Added it to the cart."}}


# decision: GATE-551
def test_each_task_reports_its_times_tokens_answer_and_what_it_may_have_left() -> None:
    turn = {"text": "add the kettle to my cart", "request_id": "r1", "task_ids": ["t1"], "replies": [], "outcomes": [],
            "started_at": "2026-11-08T10:00:00+00:00", "ended_at": "2026-11-08T10:00:09+00:00", "seconds": 9.0}
    got = qr.collect(_Trace(), turn)
    assert got["step_times"] == [{"tool": "browser.act", "kind": "click", "seconds": 4.0, "error": False},
                                 {"tool": "browser.read", "kind": "read", "seconds": 4.0, "error": False}]
    assert got["model_call_times"] == [{"model": "claude-opus-5-5", "seconds": 2.4}]
    assert got["usage"]["cache_write_1h_tokens"] == 20 and got["cost_usd"] == 0.001
    assert got["final_answer"] == "Added it to the cart."
    assert got["possible_leftovers"] == [{"tool": "browser.act", "kind": "click", "target": {"site": "shop.example"},
                                          "effect": None}]
    assert got["started_at"] and got["ended_at"]


# decision: GATE-551
def test_a_task_past_its_limit_is_stopped_through_ferbs_stop(monkeypatch: pytest.MonkeyPatch) -> None:
    stopped = threading.Event()
    calls: list[tuple[str, Any]] = []

    class Fake:
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            pass

        def stream(self, method: str, params: dict[str, Any]) -> Any:
            assert params["test_run"] == {"run": "r", "connection": "google"}
            yield {"type": "plan", "request_id": "req", "task_id": "tree1.worker"}
            stopped.wait(5)                                # a task that never ends by itself

        def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
            calls.append((method, params))
            stopped.set()
            return {"stopped": params["task_ids"]}

        def close(self) -> None:
            pass
    monkeypatch.setattr(qr, "Client", Fake)
    turn = qr.send("s", "find it", {"run": "r", "connection": "google"}, 0.2)
    assert calls == [("task.stop", {"task_ids": ["tree1"]})]
    assert turn["stopped_by_runner"] == f"stopped by the runner after {0.2 / 60:g} min"
    assert turn["task_ids"] == ["tree1.worker"] and turn["seconds"] < 5


# decision: RUN-442
def test_the_runs_model_is_set_through_the_checked_path_and_a_refusal_stops_the_run() -> None:
    seen: list[tuple[str, Any]] = []

    class Fake:
        def set_timeout(self, s: float) -> None:
            pass

        def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
            seen.append((method, params))
            return {"ok": params["model"] != "gpt-nope", "detail": "not in the models your key can use"}
    qr.set_model(Fake(), "openai", "gpt-5.6-luna")
    assert seen == [("runtimes.model", {"runtime": "openai", "model": "gpt-5.6-luna"})]
    with pytest.raises(SystemExit, match="not in the models"):
        qr.set_model(Fake(), "openai", "gpt-nope")


# decision: RUN-442
def test_the_live_runs_models_have_documented_prices_and_limits() -> None:
    from ferb.runtimes import anthropic_api, openai_api
    from ferb.runtimes.prices import PRICES
    luna = PRICES["gpt-5.6-luna"][0]
    assert (luna.input, luna.cached, luna.cache_write, luna.output) == (0.20, 0.02, 0.25, 1.20)
    assert openai_api.LIMITS["gpt-5.6-luna"] == (1_050_000, 128_000)
    sol = PRICES["gpt-6.1-sol"][0]                       # checked 2026-11-08 on OpenAI's pricing and model pages
    assert (sol.input, sol.cached, sol.cache_write, sol.output) == (2.00, 0.10, 2.50, 10.00)
    assert openai_api.LIMITS["gpt-6.1-sol"] == (1_050_000, 128_000)
    # Anthropic's page, checked 2026-11-08: hits are $0.10 on Sonnet 5.5, $0.20 on Opus 5.5, $0.25 on Fable 5.1
    assert [PRICES[m][0].cached for m in ("claude-sonnet-5-5", "claude-opus-5-5", "claude-fable-5-1")] == [0.10, 0.20, 0.25]
    assert [PRICES[m][0].cache_write_1h for m in ("claude-sonnet-5-5", "claude-opus-5-5")] == [4.00, 8.00]
    assert "claude-sonnet-5-5" in anthropic_api.LIMITS


# decision: GATE-552
def test_a_run_sends_the_whole_file_into_one_chat_and_the_next_run_gets_a_fresh_one(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner 2026-11-08: "i thogh it ws one bye one privider. a oriver rund hte full test and then other provide in
    fresh session". Every task of a run, in file order, goes into one chat; each turn is recorded on its own and the
    report is written after every turn."""
    sent: list[tuple[str, str]] = []
    writes: list[int] = []

    class Fake:
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            pass

        def stream(self, method: str, params: dict[str, Any]) -> Any:
            sent.append((params["session_id"], params["text"]))
            yield {"type": "reply", "request_id": "r" + str(len(sent)), "text": "Done: " + params["text"]}

        def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
            return {"test.mode": {"questions": "record"}, "runtimes.status": {"roles": {"worker": "codex"}},
                    "runtimes.models": {"model": "gpt-6.1-sol", "roles": {}},
                    "trace.get": {"spans": []}}.get(method, {})

        def set_timeout(self, s: float) -> None:
            pass

        def close(self) -> None:
            pass
    monkeypatch.setattr(qr, "Client", Fake)
    real_write = Path.write_text

    def counted(self: Path, text: str, *args: Any, **kwargs: Any) -> int:
        writes.append(1)
        return real_write(self, text, *args, **kwargs)
    monkeypatch.setattr(Path, "write_text", counted)
    monkeypatch.setattr(qr, "mac_usage", lambda: (_Usage(_START[1]), ""))    # RUN-444: no real process is read
    monkeypatch.setattr(qr, "ps_text", lambda: _START[0])
    monkeypatch.setattr(qr, "ferb_home", lambda: tmp_path)
    tasks = tmp_path / "tasks.txt"
    real_write(tasks, "find the weather\nlist my files || and sort them\n")
    out = tmp_path / "run.json"
    monkeypatch.setattr("sys.argv", ["question_run.py", str(tasks), "--runtime", "codex", "--out", str(out),
                                     "--repeat", "2"])
    assert qr.main() == 0
    sessions = [s for s, _ in sent]
    assert [t for _, t in sent] == ["find the weather", "list my files", "and sort them"] * 2
    assert len(set(sessions[:3])) == 1 and len(set(sessions[3:])) == 1 and sessions[0] != sessions[3]
    report = json.loads(out.read_text())
    assert report["chat"] == "one per run" and len(report["tasks"]) == 4
    assert [len(t["turns"]) for t in report["tasks"]] == [1, 2, 1, 2]
    assert report["tasks"][1]["final_answer"] == "Done: and sort them" and report["tasks"][1]["started_at"]
    assert len(writes) >= 6                                                  # after every turn, and at the end
    turns = [t for task in report["tasks"] for t in task["turns"]]
    assert all("daemon" in t["resources"]["groups"] for t in turns)
    assert all(t["resources"]["background"] == {"problem": f"no database at {tmp_path / 'ferb.db'}"} for t in turns)
    assert all(task["resources"]["ferb_cpu_s"] == 0 for task in report["tasks"])
    assert report["resources_note"]["browsers"] == qr.BROWSER_BASIS


class _P(NamedTuple):
    start: int
    cpu_s: float
    children_cpu_s: float
    footprint: int


class _Usage:
    """Stands in for libproc: each process's CPU counters and footprint, swapped between looks."""

    def __init__(self, procs: dict[int, _P]) -> None:
        self.procs = procs

    def process(self, pid: int) -> _P | None:
        return self.procs.get(pid)

    def path(self, pid: int) -> str | None:
        return None


MB = 2**20
_PS = {
    100: "    1 300000 /Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB -m ferb.daemon",
    101: "  100  51200 /Applications/FERB.app/Contents/Resources/codex/codex app-server",
    102: "  101   2048 /bin/zsh -c ls ~/Downloads",
    200: "    1 204800 /Applications/FERB.app/Contents/MacOS/FERBNotch",
    300: "    1 512000 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    301: "  300 102400 /Applications/Google Chrome.app/Contents/Frameworks/Helpers/Google Chrome Helper (Renderer).app/"
         "Contents/MacOS/Google Chrome Helper (Renderer) --type=renderer",
    302: "  300   9216 /Applications/FERB.app/Contents/Resources/python/bin/python3.12 -I -B -m ferb.native_host.main "
         "chrome-extension://abcdef/",
    303: "  300  20480 /Applications/Google Chrome.app/Contents/Frameworks/Helpers/Google Chrome Helper (Renderer).app/"
         "Contents/MacOS/Google Chrome Helper (Renderer) --type=renderer",
    400: "    1 307200 /Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    401: "    1   5120 /Applications/Brave Browser.app/Contents/Frameworks/Helpers/chrome_crashpad_handler",
    500: "    1   1024 /Applications/Some Editor.app/Contents/MacOS/Editor",
}


def _ps(*pids: int) -> str:
    return "\n".join(f"{pid:>5} {_PS[pid]}" for pid in pids) + "\n"


_START = (_ps(100, 101, 200, 300, 301, 302, 400, 401, 500), {
    100: _P(1, 50.0, 1.0, 300 * MB), 101: _P(2, 10.0, 2.0, 50 * MB), 200: _P(3, 20.0, 0.0, 200 * MB),
    300: _P(4, 100.0, 40.0, 400 * MB), 301: _P(5, 5.0, 0.0, 100 * MB), 302: _P(6, 0.1, 0.0, 9 * MB),
    400: _P(7, 900.0, 3000.0, 300 * MB), 401: _P(8, 1.0, 0.0, 5 * MB), 500: _P(9, 77.0, 0.0, MB)})


# decision: RUN-444
def test_ps_output_sorts_processes_into_ferbs_groups_and_each_browser() -> None:
    got = qr.groups(qr.ps_table(_ps(100, 101, 102, 200, 300, 301, 302, 400, 401, 500) + "garbage line\n"))
    assert got == {100: "daemon", 101: "daemon_children", 102: "daemon_children", 200: "app",
                   300: "browser:Google Chrome", 301: "browser:Google Chrome", 302: "native_host",
                   400: "browser:Brave Browser", 401: "browser:Brave Browser"}      # the editor is in no group


# decision: RUN-443, RUN-444
def test_a_turns_cpu_counts_short_lived_children_and_marks_browsers_as_an_upper_bound() -> None:
    """Owner 2026-11-08: "track teh cpu usage acrros tasks and if its acutally ferb's task using that". A shell command
    seen once mid-turn and an ffmpeg that started and ended between looks are both in daemon_children, through their
    parents' ended-children counters; a renderer started in the turn is counted in its browser."""
    state = {"ps": _START[0]}
    usage = _Usage(dict(_START[1]))
    clock = iter([0.0, 10.0])
    meter = qr.Meter(usage, ps=lambda: state["ps"], every=3600, clock=lambda: next(clock))
    meter.start()
    state["ps"] = _ps(100, 101, 102, 200, 300, 301, 302, 400, 401, 500)        # a look mid-turn: zsh running
    usage.procs |= {102: _P(10, 0.5, 0.0, 2 * MB), 101: _P(2, 11.0, 2.0, 80 * MB)}
    meter.look()
    state["ps"] = _ps(100, 101, 200, 300, 301, 302, 303, 400, 401, 500)        # the end: zsh gone, a new renderer
    usage.procs = dict(_START[1]) | {
        100: _P(1, 51.5, 4.0, 310 * MB),        # ffmpeg (3 s, never seen) ended under the daemon
        101: _P(2, 13.0, 2.8, 60 * MB),         # zsh's whole 0.8 s now in codex's ended-children counter
        300: _P(4, 101.0, 40.0, 400 * MB), 303: _P(11, 1.0, 0.0, 20 * MB),
        400: _P(7, 900.2, 3000.0, 300 * MB), 500: _P(9, 99.0, 0.0, MB)}
    got = meter.stop()
    g = got["groups"]
    assert (got["wall_s"], g["daemon"]["cpu_s"], g["daemon_children"]["cpu_s"]) == (10.0, 1.5, 6.8)
    assert g["daemon_children"]["started_in_turn"] == 1
    assert g["daemon_children"]["busiest"] == [["codex", 3.8], ["zsh", 0.5]]
    assert g["daemon_children"]["peak_footprint_mb"] == 82.0 and g["daemon_children"]["end_footprint_mb"] == 60.0
    assert (g["app"]["cpu_s"], g["native_host"]["cpu_s"]) == (0.0, 0.0)
    assert (got["ferb_cpu_s"], got["ferb_cpu_pct"]) == (8.3, 83.0)
    chrome = g["browser:Google Chrome"]
    assert (chrome["cpu_s"], chrome["started_in_turn"], chrome["basis"]) == (2.0, 1, qr.BROWSER_BASIS)
    assert (g["browser:Brave Browser"]["cpu_s"], got["browser_cpu_s"]) == (0.2, 2.2)
    assert g["daemon"]["basis"] == qr.FERB_BASIS and "problems" not in got


# decision: RUN-444
def test_a_daemon_restart_or_a_failed_ps_is_said_not_measured_as_zero() -> None:
    state: dict[str, Any] = {"ps": _START[0]}
    usage = _Usage(dict(_START[1]))
    meter = qr.Meter(usage, ps=lambda: state["ps"], every=3600, clock=iter([0.0, 5.0]).__next__)
    meter.start()
    usage.procs = dict(_START[1]) | {100: _P(99, 0.4, 0.0, 100 * MB)}          # a new daemon process
    assert "daemon restarted" in meter.stop()["problems"][0]

    def broken() -> str:
        raise subprocess.SubprocessError("ps timed out")
    failed = qr.Meter(usage, ps=broken, every=3600, clock=iter([0.0, 5.0]).__next__)
    failed.start()
    got = failed.stop()
    assert "groups" not in got and got["problems"] == ["ps: ps timed out"]


# decision: RUN-444
def test_background_lists_other_task_trees_and_overlapping_resource_samples(tmp_path: Path) -> None:
    db = tmp_path / "ferb.db"
    conn = sqlite3.connect(db)
    conn.executescript(
        "CREATE TABLE tasks (tree_id TEXT, role TEXT, status TEXT, created_at TEXT, updated_at TEXT);"
        "CREATE TABLE resource_samples (id INTEGER PRIMARY KEY, at TEXT, window_s REAL, cpu_s REAL, child_cpu_s REAL,"
        " app_cpu_s REAL, work_json TEXT);")
    conn.executemany("INSERT INTO tasks VALUES (?, ?, ?, ?, ?)", [
        ("mine", "worker", "done", "2026-11-08T10:00:01+00:00", "2026-11-08T10:00:30+00:00"),
        ("sched", "worker", "running", "2026-11-08T09:00:00+00:00", "2026-11-08T09:00:00+00:00"),
        ("old", "worker", "done", "2026-11-08T08:00:00+00:00", "2026-11-08T08:10:00+00:00"),
        ("later", "worker", "done", "2026-11-08T11:00:00+00:00", "2026-11-08T11:05:00+00:00")])
    work = json.dumps({"tasks_running": 2, "tool_calls": 9, "models": {"worker": 1}, "perception": {"wakes": 1},
                       "top": [["codex", 4.0]], "events": {"window": 3}})
    conn.executemany("INSERT INTO resource_samples (at, window_s, cpu_s, child_cpu_s, app_cpu_s, work_json) "
                     "VALUES (?, 300, 12.5, 4.0, 3.0, ?)", [("2026-11-08T09:59:00+00:00", work),
                                                            ("2026-11-08T10:04:00+00:00", work),
                                                            ("2026-11-08T10:09:00+00:00", work)])
    conn.commit()
    conn.close()
    got = qr.background(db, "2026-11-08T10:00:00+00:00", "2026-11-08T10:02:00+00:00", {"mine"})
    assert [t["tree_id"] for t in got["other_tasks"]] == ["sched"]
    assert [s["at"] for s in got["resource_samples"]] == ["2026-11-08T10:04:00+00:00"]
    assert got["resource_samples"][0]["tasks_running"] == 2 and "events" not in got["resource_samples"][0]


# decision: RUN-444
def test_compare_puts_cpu_per_task_and_totals_per_connection() -> None:
    spec = importlib.util.spec_from_file_location("question_compare",
                                                  Path(__file__).parent.parent / "scripts/question_compare.py")
    assert spec is not None and spec.loader is not None
    qc = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(qc)
    res = {"wall_s": 20.0, "ferb_cpu_s": 5.0, "ferb_cpu_pct": 25.0, "browser_cpu_s": 7.5, "other_tasks": 1,
           "cpu_s": {"daemon": 1.0, "daemon_children": 3.5, "app": 0.5},
           "peak_footprint_mb": {"daemon": 300.0, "daemon_children": 80.0, "browser:Google Chrome": 900.0}}
    task = {"id": "1", "run": 1, "seconds": 20.0, "steps": 3, "questions": 0, "cost_usd": 0.01, "resources": res,
            "turns": [{"text": "find the weather"}]}
    text = qc.table([{"runtime": "codex", "tasks": [task, task | {"id": "2"}]},
                     {"runtime": "openai", "tasks": [task | {"resources": None}]}])
    header = next(line for line in text.splitlines() if line.startswith("| connection | run | outcome"))
    first = text.splitlines()[text.splitlines().index(header) + 2].split(" | ")
    cols = header.strip("| ").split(" | ")
    assert [first[cols.index(c)] for c in ("FERB CPU s", "children s", "browsers s (upper bound)", "peak FERB MB",
                                           "other tasks")] == ["5.0", "3.5", "7.5", "380", "1"]
    totals = text.split("## Totals")[1]
    assert "| codex | 1 | 2 | 40.0 | 6 | 0.0200 | 10.0 | 25.0 | 2.0 | 7.0 | 1.0 | 15.0 |" in totals
    assert "| openai | 1 | 1 | 20.0 | 3 | 0.0100 |  |  |  |  |  |  |" in totals


# decision: GATE-553
def test_a_clarifying_question_is_declined_in_its_own_turn_and_never_answered_by_the_next_task(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """Live 2026-11-08 14:34: FERB asked "Which YouTube account should I change?"; the old runner sent the next task,
    which FERB took as the answer. Now the question is recorded, the runner answers it with the decline line in the
    same chat and waits for the turn to end."""
    sent: list[str] = []

    class Fake:
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            pass

        def stream(self, method: str, params: dict[str, Any]) -> Any:
            sent.append(params["text"])
            if len(sent) == 1:
                yield {"type": "clarify", "request_id": "r1", "text": "Which YouTube account should I change?"}
            else:
                yield {"type": "plan", "request_id": "r1", "task_id": "t1"}
                yield {"type": "outcome", "task_id": "t1", "status": "done", "summary": "Turned them off."}

        def close(self) -> None:
            pass
    monkeypatch.setattr(qr, "Client", Fake)
    turn = qr.send("s", "turn off all the youtube emails", {"run": "r", "connection": "openai"}, 30)
    assert sent == ["turn off all the youtube emails", qr.DECLINE]
    assert turn["clarify"][0]["question"] == "Which YouTube account should I change?"
    assert turn["outcomes"][0]["status"] == "done" and turn["task_ids"] == ["t1"]
    got = qr.collect(type("T", (), {"call": lambda self, m, p: {"spans": []}})(), turn)
    assert [q["kind"] for q in got["questions"]] == ["clarify"]


# decision: GATE-574
def test_a_question_still_open_after_the_declines_sends_the_next_task_to_a_fresh_chat(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Live 2026-11-10 (Codex): FERB asked the Amazon question again after every decline; the runner then sent the hotel
    and table tasks into the same chat, where FERB read them with its open question. The question left open is recorded
    as unanswered, and the next task starts in a chat of its own."""
    sent: list[tuple[str, str]] = []

    class Fake:
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            pass

        def stream(self, method: str, params: dict[str, Any]) -> Any:
            sent.append((params["session_id"], params["text"]))
            if sent[0][1] == "order me cables" and len(sent) <= qr.MAX_CLARIFY + 1:
                yield {"type": "clarify", "request_id": f"r{len(sent)}", "text": "Which Amazon account?"}
            else:
                yield {"type": "reply", "request_id": f"r{len(sent)}", "text": "Done."}

        def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
            return {"test.mode": {"questions": "record"}, "runtimes.status": {"roles": {}},
                    "runtimes.models": {"model": "m", "roles": {}}, "trace.get": {"spans": []}}.get(method, {})

        def set_timeout(self, s: float) -> None:
            pass

        def close(self) -> None:
            pass
    monkeypatch.setattr(qr, "Client", Fake)
    monkeypatch.setattr(qr, "mac_usage", lambda: (_Usage(_START[1]), ""))
    monkeypatch.setattr(qr, "ps_text", lambda: _START[0])
    monkeypatch.setattr(qr, "ferb_home", lambda: tmp_path)
    tasks = tmp_path / "tasks.txt"
    tasks.write_text("order me cables\nbook a hotel\nbook a table\n")
    out = tmp_path / "run.json"
    monkeypatch.setattr("sys.argv", ["question_run.py", str(tasks), "--runtime", "codex", "--out", str(out)])
    assert qr.main() == 0
    texts = [t for _, t in sent]
    assert texts == ["order me cables", *[qr.DECLINE] * qr.MAX_CLARIFY, "book a hotel", "book a table"]
    cables, hotel, table = (s for s, t in sent if t in ("order me cables", "book a hotel", "book a table"))
    assert hotel != cables and table == hotel                   # a fresh chat after the open question only
    report = json.loads(out.read_text())
    asked = report["tasks"][0]["turns"][0]["clarify"]
    assert [q["answer"] for q in asked] == [qr.DECLINE] * qr.MAX_CLARIFY + [qr.UNANSWERED]


# decision: GATE-574
def test_test_mode_goes_off_on_a_new_connection_when_the_runs_own_one_broke(
        capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[dict[str, Any]] = []

    class Broken:
        def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
            raise OSError("the socket closed")

    class Fresh:
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            pass

        def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
            calls.append(dict(params or {}))
            return {"enabled": False}

        def close(self) -> None:
            pass
    monkeypatch.setattr(qr, "Client", Fresh)
    qr.test_mode_off(Broken())
    assert calls == [{"off": True}] and "test mode: off" in capsys.readouterr().err


# decision: GATE-553
def test_the_compare_table_marks_tasks_the_old_runner_tangled_and_totals_without_them() -> None:
    spec = importlib.util.spec_from_file_location("question_compare",
                                                  Path(__file__).parent.parent / "scripts/question_compare.py")
    assert spec is not None and spec.loader is not None
    qc = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(qc)

    def task(ident: str, turns: list[dict[str, Any]]) -> dict[str, Any]:
        return {"id": ident, "session": "s", "turns": turns, "seconds": 10, "questions": 0, "cost_usd": 1.0}
    asked = {"text": "turn off the youtube emails", "request_id": "r1", "task_ids": [], "replies": [], "outcomes": []}
    done = {"text": "what are people saying", "request_id": "r2", "task_ids": ["t2"], "replies": [],
            "outcomes": [{"status": "failed"}]}
    old = {"runtime": "openai", "run": "20261108-131046", "tasks": [task("F2", [asked]), task("G1", [done]),
                                                                     task("H1", [done])]}
    assert qc.tangled(old, set()) == {0, 1}
    assert qc.tangled(old, {"r1"}) == {0, 1}                        # the journal says the same
    fixed = {"runtime": "anthropic", "run": "r2", "tasks": [task("F2", [asked | {"clarify": [{"question": "q"}]}]),
                                                             task("G1", [done | {"clarify": []}])]}
    assert qc.tangled(fixed, set()) == set()
    text = qc.table([old, fixed])
    assert "tangled by the old runner" in text and "ran with the runner before the clarify fix" in text
    totals = text.split("## Totals")[1]
    assert "| openai | 1 | 3 | 30.0 |" in totals and "| openai | 1 without tangled | 1 | 10.0 |" in totals
    assert "| anthropic | 1 | 2 | 20.0 |" in totals and "anthropic | 1 without" not in totals


# decision: GATE-554
@pytest.mark.parametrize("ending", ["end", "ctrl_c", "error", "sigterm"])
def test_every_way_out_of_a_run_turns_test_mode_off_and_says_so(
        ending: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
    """2026-11-09: a run cancelled on 11-08 left test mode on, and the next morning FERB declined the owner's own
    question in a real scheduled run. Whatever ends the run, test mode goes off and the terminal says so."""
    modes: list[dict[str, Any]] = []

    class Fake:
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            pass

        def stream(self, method: str, params: dict[str, Any]) -> Any:
            yield {"type": "reply", "request_id": "r1", "text": "ok"}

        def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
            if method == "runtimes.use" and ending == "error" and modes:
                raise RuntimeError("the socket closed")             # an error the runner doesn't handle
            if method == "trace.get" and ending in ("ctrl_c", "sigterm"):    # mid-run, in the runner's own thread
                if ending == "ctrl_c":
                    raise KeyboardInterrupt
                qr._exit_on_signal(15, None)                    # what the SIGTERM handler does
            if method == "test.mode":
                modes.append(dict(params or {}))
                if (params or {}).get("off"):
                    return {"enabled": False, "questions": "ask"}
            return {"test.mode": {"questions": "record"}, "runtimes.status": {"roles": {}},
                    "runtimes.models": {"model": "m", "roles": {}}, "trace.get": {"spans": []}}.get(method, {})

        def set_timeout(self, s: float) -> None:
            pass

        def close(self) -> None:
            pass
    monkeypatch.setattr(qr, "Client", Fake)
    monkeypatch.setattr(qr, "mac_usage", lambda: (_Usage(_START[1]), ""))
    monkeypatch.setattr(qr, "ps_text", lambda: _START[0])
    monkeypatch.setattr(qr, "ferb_home", lambda: tmp_path)
    tasks = tmp_path / "tasks.txt"
    tasks.write_text("find the weather\n")
    monkeypatch.setattr("sys.argv", ["question_run.py", str(tasks), "--runtime", "codex", "--out",
                                     str(tmp_path / "run.json"), "--enable", "me@example.com"])
    if ending == "end":
        assert qr.main() == 0
    else:
        with pytest.raises((KeyboardInterrupt, RuntimeError, SystemExit)):
            qr.main()
    assert modes[0] == {"allow_send": ["me@example.com"], "questions": "record"}
    assert modes[-1] == {"off": True}
    assert "test mode: off" in capsys.readouterr().err
