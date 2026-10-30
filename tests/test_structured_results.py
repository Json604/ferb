"""Whatever a provider promises, FERB checks each typed answer against the schema it asked for (D32)."""
import json

import pytest

from ferb.core.structured import check_final
from ferb.memory.distill import DISTILL_SCHEMA
from ferb.orchestrator import schemas
from tests.conftest import install, sup
from tests.support.strict_schema import strict_problems

SCHEMA = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": False}


def test_a_matching_answer_passes_unchanged() -> None:
    assert check_final('{"ok": true}', SCHEMA) == '{"ok": true}'


@pytest.mark.parametrize("text,why", [("not json", "not JSON"), ('{"ok": "yes"}', "at ok"), ('{}', "top level"),
                                      ('{"ok": true, "extra": 1}', "top level")])
def test_a_wrong_answer_names_the_problem(text: str, why: str) -> None:
    with pytest.raises(ValueError, match=why):
        check_final(text, SCHEMA)


def test_no_schema_means_free_text() -> None:
    assert check_final("plain words", None) == "plain words"


def test_every_ferb_output_schema_is_strict_compatible() -> None:
    """OpenAI strict mode and Anthropic output_config need additionalProperties false and every property required."""
    found = {n: strict_problems(getattr(schemas, n)) for n in dir(schemas) if n.endswith("_SCHEMA")}
    found["DISTILL_SCHEMA"] = strict_problems(DISTILL_SCHEMA)
    assert not {n: p for n, p in found.items() if p}


def test_a_worker_answer_that_breaks_the_schema_is_not_used(daemon) -> None:
    from tests.conftest import FakeRuntime, chat, outcome, task
    script = install(daemon, [sup("task", "Checking.", task("check something"))],
                     {"worker": json.dumps({"status": "done"})})      # WORKER_SCHEMA also requires reason, answer, artifacts…

    def make(role, *, mode, tool_names, schema):
        rt = FakeRuntime(script, mode, tool_names)
        if mode == "worker":
            rt.output_schema = schema                         # the test double carries the schema a real runtime would
        return rt
    daemon.daemon.supervisor.make_runtime = make
    out = outcome(chat(daemon, "check something"))
    assert out["status"] != "done"
    errors = daemon.run(_worker_errors(daemon))
    assert any("doesn't match the schema" in (e or "") for e in errors)


async def _worker_errors(daemon):
    return [r["error"] for r in daemon.daemon.db.conn.execute("SELECT error FROM spans WHERE kind = 'worker'").fetchall()]
