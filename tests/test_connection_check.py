"""Settings → AI's Check (T-1069, owner 2026-11-08): the call shapes FERB really makes, each a line with pass or fail,
the provider's words redacted, the model, the typed form, tokens, cost, and the total. Fake runtimes stand in for the
providers; the check tools go through the real gateway."""
import json
from typing import Any, AsyncIterator

import anyio

from ferb.core.types import RunHandle, RuntimeEvent
from tests.conftest import RunningDaemon, tool
from tests.test_schema_shape import sample

SECRET = "sk-test-CHECKSECRET0123456789"


class _Provider:
    """One connection's fake adapter: a valid answer for each call's schema, the check tools called through the
    gateway for the worker call, and the planner's call refused when `refuse_planner`."""

    def __init__(self, daemon: RunningDaemon, role: str, schema: dict[str, Any], tools: list[str],
                 refuse_planner: bool, call_tools: bool) -> None:
        self.daemon, self.role, self.schema, self.tools = daemon, role, schema, tools
        self.refuse_planner, self.call_tools = refuse_planner, call_tools

    async def start(self, task: Any, request: Any, run_token: str) -> RunHandle:
        return RunHandle("run-" + task.id, "fake", task.id, {"token": run_token, "images": len(request.images)})

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        if self.role == "orchestrator" and self.refuse_planner and "properties" in self.schema \
                and "kind" in self.schema["properties"]:
            yield RuntimeEvent("error", {"message": f"anthropic: HTTP 400: The compiled grammar is too large ({SECRET})"})
            return
        if self.call_tools and self.tools and run.state["token"]:
            token = run.state["token"]
            await anyio.to_thread.run_sync(lambda: tool(self.daemon.sock, token, "check.ping"))
            await anyio.to_thread.run_sync(lambda: tool(self.daemon.sock, token, "check.echo", text="hello"))
        yield RuntimeEvent("usage", {"model": "fake-model-1", "typed": "strict", "input_tokens": 40,
                                     "output_tokens": 6, "cost_usd": 0.0005})
        yield RuntimeEvent("final", {"text": json.dumps(sample(self.schema, False))})

    async def cancel(self, run: RunHandle) -> None:
        return None


def _check(daemon: RunningDaemon, monkeypatch: Any, *, refuse_planner: bool = False,
           call_tools: bool = True) -> dict[str, Any]:
    d = daemon.daemon
    d._add_secret_value(SECRET)

    def build(name: str, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any],
              effort: Any = None, model: str = "") -> _Provider:
        return _Provider(daemon, role, schema, tool_names, refuse_planner, call_tools)
    monkeypatch.setattr(d, "_build_runtime", build)
    return daemon.run(d._runtimes_check_calls(None, {"runtime": "anthropic"}))   # type: ignore[no-any-return]


# decision: RUN-434, RUN-435
def test_every_call_shape_passes_with_its_facts_and_the_total(daemon: RunningDaemon, monkeypatch: Any) -> None:
    report = _check(daemon, monkeypatch)
    names = [line["name"] for line in report["lines"]]
    assert names == ["Planner call (its real schema, lookups offered)", "Tool call with no arguments",
                     "Tool call with arguments", "Worker call (tools, its typed report)", "Checker call (its schema)",
                     "A picture in the request"]
    assert report["ok"] and all(line["ok"] for line in report["lines"])
    planner = report["lines"][0]
    assert planner["model"] == "fake-model-1" and planner["typed"] == "strict" and planner["input_tokens"] == 40
    assert abs(report["total_cost_usd"] - 0.0005 * 4) < 1e-9                  # four model calls
    assert "check.ping" not in daemon.daemon.registry.capabilities            # the check tools are gone after it


# decision: RUN-434
def test_a_refused_call_says_the_providers_words_redacted_and_tools_not_called(daemon: RunningDaemon,
                                                                              monkeypatch: Any) -> None:
    report = _check(daemon, monkeypatch, refuse_planner=True, call_tools=False)
    planner, no_args, with_args = report["lines"][:3]
    assert not report["ok"] and not planner["ok"]
    assert "The compiled grammar is too large" in planner["detail"] and SECRET not in json.dumps(report)
    assert not no_args["ok"] and "didn't call check.ping" in no_args["detail"]
    assert not with_args["ok"]
    assert report["lines"][3]["ok"]                                           # the worker's own answer still fits
