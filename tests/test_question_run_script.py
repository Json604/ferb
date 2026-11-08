"""scripts/question_run.py (GATE-551): what the task-run tool reports per task, and its own limits. Fakes stand in for
FERB's socket; nothing reaches the owner's FERB."""
import importlib.util
import json
import threading
from pathlib import Path
from typing import Any

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
