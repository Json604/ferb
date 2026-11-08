"""scripts/question_run.py (GATE-551): what the task-run tool reports per task, and its own limits. Fakes stand in for
FERB's socket; nothing reaches the owner's FERB."""
import importlib.util
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
