"""A worker/checker whose runtime shows FERB's tools were never loaded is re-run once with that fact."""
import json

from ferb.core.types import RuntimeEvent
from tests.conftest import chat, done, install, outcome, sup, task, tool


def _report(summary="ok"):
    return json.dumps({"status": "done", "summary": summary, "artifacts": [], "evidence": []})


def test_rerun_when_tools_never_loaded(daemon) -> None:
    def second(token):
        tool(daemon.sock, token, "actions.search", query="files")      # a real FERB tool call
        return _report("did it with tools")
    script = install(daemon, [sup("task", "Working", task())], {
        "worker": [RuntimeEvent("final", {"text": _report("blind"), "tools_seen": ["apply_patch"]}), second]})
    chat(daemon, "do the thing")
    worker_contexts = [c for _, k, c, _ in script.contexts if k == "worker"]
    assert len(worker_contexts) == 2
    assert "never loaded FERB's tools" in worker_contexts[1]
    async def span_kinds():
        return [r[0] for r in daemon.daemon.db.conn.execute("SELECT kind FROM spans")]
    kinds = daemon.run(span_kinds())
    assert "runtime.tools_not_loaded" in kinds


def test_no_rerun_when_ferb_tools_were_seen(daemon) -> None:
    script = install(daemon, [sup("task", "Working", task())], {
        "worker": [RuntimeEvent("final", {"text": _report(), "tools_seen": ["tool_search", "mcp__ferb.fs_read"]})]})
    chat(daemon, "do the thing")
    assert len([1 for _, k, _, _ in script.contexts if k == "worker"]) == 1


def test_no_rerun_when_runtime_reports_nothing(daemon) -> None:
    script = install(daemon, [sup("task", "Working", task())], {"worker": done("ok")})
    chat(daemon, "do the thing")
    assert len([1 for _, k, _, _ in script.contexts if k == "worker"]) == 1
