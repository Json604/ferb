"""L28, D32: every learning model call (consolidation, distillation) is one `model.learning` span: its role, runtime,
model and input size (never the input), its outcome and the redacted error, so `ferb stats` and `ferb trace` show a
failing learner. Live 2026-10-11 to 10-20: 226 learning jobs failed and no trace showed a learning call at all."""
import io
import json
from typing import Any

from ferb.core.types import ModelLimits, ModelRequest, RunHandle, RuntimeEvent
from ferb.cli.client import Client
from ferb.runtimes.codex_stream import ProtocolError
from tests.conftest import FakeDistillRuntime, RunningDaemon, trail_row

CODEX_REFUSAL = ("Codex could not complete turn/start: Input exceeds the maximum length of 1048576 characters. "
                 "(code -32602; input_too_large, 1100000 characters sent)")
REQUEST = "open the tea shop page and read the opening hours"


class RefusedAtStart(FakeDistillRuntime):
    """Codex's app-server refusing turn/start, as the Codex runtime now raises it (Codex's own message)."""

    async def start(self, task: Any, request: ModelRequest, token: str) -> RunHandle:
        self.contexts.append(request.context)
        raise ProtocolError(CODEX_REFUSAL)


class Answers(FakeDistillRuntime):
    async def events(self, run: RunHandle) -> Any:
        yield RuntimeEvent("usage", {"input_tokens": 10, "output_tokens": 2})
        yield RuntimeEvent("final", {"text": json.dumps(self.answer)})


def _card(d: Any, ident: str) -> Any:
    return d.learning.put("episode", ident, {"task_id": ident, "verified": True, "request": REQUEST, "apps": {},
                                             "slots": {}, "status": "done",
                                             "actions": [{"id": ident + "-a", "tool": "browser.open",
                                                          "args": {"n": ident}}]},
                          [], origin_label="ferb", made_by="code:evidence-card/2", state="active")


def _learning_spans(daemon: RunningDaemon) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [{"input": json.loads(r[0]), "output": json.loads(r[1] or "null"), "error": r[2], "task_id": r[3]}
                for r in daemon.daemon.db.conn.execute(
                    "SELECT input_json, output_json, error, task_id FROM spans WHERE kind = 'model.learning' "
                    "ORDER BY rowid")]
    return daemon.run(go())


def test_a_failed_consolidation_call_is_a_span_with_codexs_error_and_size_never_its_input(daemon) -> None:
    d = daemon.daemon
    daemon.run(_put(d))
    rt = RefusedAtStart({})
    d.consolidator.make_runtime = lambda *a, **k: rt
    daemon.run(d.consolidator.catch_up(force=True), timeout=30)
    spans = _learning_spans(daemon)
    assert len(spans) == 1
    span = spans[0]
    assert span["input"]["role"] == "consolidate" and span["input"]["runtime"] == "fake"
    assert span["input"]["call"].startswith("sleep-") and span["task_id"] is None
    assert span["output"]["outcome"] == "failed" and span["output"]["input_chars"] == len(rt.contexts[0])
    assert span["output"]["input_tokens_estimate"] > 0
    assert "Input exceeds the maximum length of 1048576 characters." in span["error"]
    assert REQUEST not in json.dumps(span)                              # sizes only, never the input
    error = daemon.run(_job_error(d))
    assert "Input exceeds the maximum length of 1048576 characters." in error


async def _put(d: Any) -> None:
    _card(d, "e1")


async def _job_error(d: Any) -> str:
    return str(d.db.conn.execute("SELECT error FROM learning_jobs").fetchone()[0])


def test_an_answered_consolidation_call_records_its_outcome(daemon) -> None:
    d = daemon.daemon
    daemon.run(_put(d))
    d.consolidator.make_runtime = lambda *a, **k: Answers({"procedures": [], "lessons": [], "identity": [],
                                                           "episodes": []})
    daemon.run(d.consolidator.catch_up(force=True), timeout=30)
    [span] = _learning_spans(daemon)
    assert span["output"]["outcome"] == "answered" and span["error"] is None


class RetriesThenAnswers(Answers):
    async def events(self, run: RunHandle) -> Any:
        yield RuntimeEvent("retrying", {"message": "Codex is retrying after an error: stream disconnected"})
        async for event in super().events(run):
            yield event


def test_a_retried_error_in_a_learning_call_is_recorded_and_the_call_answers(daemon) -> None:
    d = daemon.daemon
    trail_row(daemon, "2025-12-30")
    d.distiller.make_runtime = lambda *a, **k: RetriesThenAnswers({"narrative": "", "items": []})
    assert daemon.run(d.distiller.run_day("2025-12-30"), timeout=30)["status"] == "distilled"
    [span] = _learning_spans(daemon)
    assert span["output"]["outcome"] == "answered"

    async def retries() -> list[Any]:
        return [tuple(r) for r in d.db.conn.execute(
            "SELECT s.error, p.kind FROM spans s JOIN spans p ON p.id = s.parent_id WHERE s.kind = 'model.retry'")]
    assert daemon.run(retries()) == [("Codex is retrying after an error: stream disconnected", "model.learning")]
    from ferb.cli.admin import render_trace
    c = Client(daemon.sock)
    try:
        lines = render_trace(c.call("trace.get", {"task_id": "distill-2025-12-30"}))
    finally:
        c.close()
    assert any("stream disconnected" in line for line in lines)


def test_a_failed_distillation_call_is_a_span_and_its_day_keeps_codexs_error(daemon) -> None:
    d = daemon.daemon
    trail_row(daemon, "2025-12-30")
    d.distiller.make_runtime = lambda *a, **k: RefusedAtStart({})
    result = daemon.run(d.distiller.run_day("2025-12-30"), timeout=30)
    assert result["status"] == "failed"
    [span] = _learning_spans(daemon)
    assert span["input"]["role"] == "distill" and span["input"]["call"] == "distill-2025-12-30"
    assert span["output"]["outcome"] == "failed" and "Input exceeds the maximum length" in span["error"]


def test_a_call_refused_over_the_cap_is_recorded_with_its_size(daemon, monkeypatch) -> None:
    d = daemon.daemon
    trail_row(daemon, "2025-12-30")
    rt = FakeDistillRuntime({"narrative": "", "items": []})
    monkeypatch.setattr(FakeDistillRuntime, "limits", ModelLimits("m", 1_000_000, 100_000, "test double",
                                                                  max_input_chars=100))
    d.distiller.make_runtime = lambda *a, **k: rt
    daemon.run(d.distiller.run_day("2025-12-30"), timeout=30)
    [span] = _learning_spans(daemon)
    assert span["output"]["outcome"] == "failed" and span["output"]["input_chars"] > 100
    assert span["output"]["max_input_chars"] == 100 and "InputTooLarge" in span["error"] and not rt.contexts


def test_stats_and_trace_show_failing_learning_calls(daemon) -> None:
    from ferb.cli.admin import render_trace, stats
    d = daemon.daemon
    trail_row(daemon, "2025-12-30")
    d.distiller.make_runtime = lambda *a, **k: RefusedAtStart({})
    daemon.run(d.distiller.run_day("2025-12-30"), timeout=30)
    out = io.StringIO()
    assert stats(d.home, out, days=36500) == 0
    text = out.getvalue()
    assert "Learning model calls: 1, failed: 1" in text and "Input exceeds the maximum length" in text
    assert "ferb trace distill-2025-12-30" in text
    from ferb.cli.admin import trace
    c = Client(daemon.sock)
    story = io.StringIO()
    try:
        lines = render_trace(c.call("trace.get", {"task_id": "distill-2025-12-30"}))
        trace(c, "distill-2025-12-30", story)                         # `ferb trace distill-2025-12-30`
    finally:
        c.close()
    assert any("model.learning" in line for line in lines)
    assert any("Input exceeds the maximum length" in line for line in lines)
    assert "model.learning" in story.getvalue() and "Input exceeds the maximum length" in story.getvalue()
