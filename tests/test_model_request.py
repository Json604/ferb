"""One request shape for every runtime: instructions, ordered context parts, and the cacheable prefix (D32, P20)."""
from ferb.core.context import ContextBlock, assemble_context, assemble_parts
from ferb.core.types import ModelRequest
from tests.conftest import chat, install, sup


def test_parts_keep_order_and_count_the_cacheable_prefix() -> None:
    parts, cacheable = assemble_parts([ContextBlock("facts", "a", protected=True, cache=True),
                                       ContextBlock("history", "b", protected=True, cache=True),
                                       ContextBlock("memory", "c"),
                                       ContextBlock("message", "d", protected=True)])
    assert parts[:4] == ["facts", "history", "memory", "message"]
    assert parts[-1].startswith("Context budget: ")
    assert cacheable == 2


def test_an_omitted_cache_block_ends_the_cacheable_prefix() -> None:
    parts, cacheable = assemble_parts([ContextBlock("long " * 400, "a", cache=True),
                                       ContextBlock("history", "b", protected=True, cache=True)], budget_tokens=60)
    assert parts[0].startswith("[Context omitted") and cacheable == 0


def test_assemble_context_is_the_joined_parts() -> None:
    blocks = [ContextBlock("one", "a", protected=True), ContextBlock("two", "b")]
    assert assemble_context(blocks) == "\n\n".join(assemble_parts(blocks)[0])
    assert assemble_context([]) == "\n\n".join(assemble_parts([])[0]) and assemble_context([]).startswith("\n\n")


def test_model_request_context_joins_parts() -> None:
    assert ModelRequest("sys", ("a", "b"), 1).context == "a\n\nb"


def test_supervisor_request_marks_device_facts_and_history_cacheable(daemon) -> None:
    script = install(daemon, [sup("reply", "first"), sup("reply", "second")])
    chat(daemon, "an earlier message")
    chat(daemon, "the new message")
    request = [r for _, key, r in script.requests if key == "supervisor"][-1]
    assert request.parts[0].startswith("This Mac: ") and request.parts[1].startswith("Conversation so far")
    assert request.cacheable == 2
    assert request.system                                 # FERB's instructions travel separately from the context
