"""A learning runtime double that answers each call by the schema it was made for (intent labels vs consolidation)."""
import json
from typing import Any, Callable

from ferb.core.types import ModelLimits, ModelRequest, RunHandle, RuntimeEvent


class SchemaAnswers:
    """`make` is the consolidator's `make_runtime`. `answers` maps a schema object (by identity) to a function of the
    request context that returns the JSON answer; `limits` lets a test shrink the window. `calls` records each call as
    (id of its schema, its context)."""
    name = "fake"

    def __init__(self, answers: dict[int, Callable[[str], Any]],
                 limits: ModelLimits = ModelLimits("fake-model", 1_000_000, 100_000, "test double")) -> None:
        self.answers, self.limits = answers, limits
        self.calls: list[tuple[int, str]] = []

    def make(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> "SchemaAnswers._Run":
        return SchemaAnswers._Run(self, schema)

    class _Run:
        def __init__(self, owner: "SchemaAnswers", schema: dict[str, Any]) -> None:
            self.owner, self.schema, self.name, self.limits = owner, schema, owner.name, owner.limits
            self.context = ""

        async def start(self, task: Any, request: ModelRequest, token: str) -> RunHandle:
            self.context = request.context
            self.owner.calls.append((id(self.schema), self.context))
            return RunHandle("r", "fake", task.id, {})

        async def events(self, run: RunHandle) -> Any:
            yield RuntimeEvent("final", {"text": json.dumps(self.owner.answers[id(self.schema)](self.context))})

        async def cancel(self, run: RunHandle) -> None:
            return None
