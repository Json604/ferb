"""A model's tool call with no arguments, or with arguments that aren't JSON, never ends the turn (live 2026-11-08
08:16: "anthropic: streamed tool arguments were not JSON" on a planner lookup with no arguments)."""
from typing import Any

from ferb.runtimes.anthropic_api import AnthropicProvider, AnthropicStream
from ferb.runtimes.openai_api import OpenAIProvider


def _stream(deltas: list[str] | None) -> dict[str, Any]:
    """A Messages stream as the API sends it: a tool_use block whose input starts {}, then its argument fragments
    (none at all when `deltas` is None)."""
    s = AnthropicStream()
    s.feed({"type": "message_start", "message": {"id": "m", "role": "assistant", "usage": {"input_tokens": 9}}})
    s.feed({"type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "t1", "name": "memory_read", "input": {}}})
    for part in deltas or []:
        s.feed({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": part}})
    s.feed({"type": "content_block_stop", "index": 0})
    s.feed({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 8}})
    s.feed({"type": "message_stop"})
    return s.finish()


# decision: RUN-430
def test_an_argument_less_or_malformed_tool_call_never_ends_the_turn() -> None:
    provider = AnthropicProvider("claude-sonnet-5-5")
    for deltas in ([""], None, [" ", ""]):                                   # empty fragments, or none at all
        state: dict[str, Any] = {"body": {"messages": []}}
        [call] = provider.parse(state, _stream(deltas)).calls
        assert call.arguments == {} and call.arguments_error == ""
    state = {"body": {"messages": []}}
    [bad] = provider.parse(state, _stream(['{"query": ', '"unfinished'])).calls
    assert bad.arguments == {} and bad.arguments_error == "not valid JSON"     # told back to the model: one repair
    assert state["body"]["messages"][-1]["content"][0]["input"] == {}          # replayed as an object the API takes
    [ok] = provider.parse({"body": {"messages": []}}, _stream(['{"query": ', '"x"}'])).calls
    assert ok.arguments == {"query": "x"}


# decision: RUN-430
def test_openai_tool_calls_with_empty_arguments_are_argument_less() -> None:
    provider = OpenAIProvider("gpt-6.1-sol")
    for raw in ("", "  ", None):
        state: dict[str, Any] = {"body": {"input": []}}
        data = {"output": [{"type": "function_call", "call_id": "c1", "name": "memory_read", "arguments": raw}],
                "status": "completed", "usage": {}}
        [call] = provider.parse(state, data).calls
        assert call.arguments == {} and call.arguments_error == ""
