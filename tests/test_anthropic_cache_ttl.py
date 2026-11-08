"""Anthropic API caching (RUN-436, owner 2026-11-08: "Mine: history at 1h too (Recommended)"): FERB's instructions and
the stable context, conversation history included, at the 1-hour TTL; the automatic tail at 5 minutes; each call's
cache writes by TTL and reads in its usage, 1-hour writes priced at 2x input."""
from typing import Any

from ferb.core.types import ModelRequest
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.usage import cost_usd, from_anthropic

HOUR = {"type": "ephemeral", "ttl": "1h"}


def _sent(request: ModelRequest, more: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    p = AnthropicProvider("claude-opus-5-5")
    state = p.begin(request, [], None, "low", "k")
    state["body"]["messages"] += more or []
    return p.http(state)[2]


# decision: RUN-436, RUN-437
def test_instructions_and_history_cache_for_an_hour_and_the_tail_for_five_minutes() -> None:
    body = _sent(ModelRequest("FERB instructions", ("facts", "history", "message"), 2))
    assert body["system"][0]["cache_control"] == HOUR
    content = body["messages"][0]["content"]
    assert content[1]["cache_control"] == HOUR and "cache_control" not in content[2]
    assert body["cache_control"] == {"type": "ephemeral"}                    # the automatic tail: 5 minutes, last
    # the 1h marker on the request's last block: no automatic 5m one beside it (the API refuses the mix there)
    last = _sent(ModelRequest("FERB instructions", ("facts", "history"), 2))
    assert last["messages"][0]["content"][-1]["cache_control"] == HOUR and "cache_control" not in last
    # once the loop adds turns after it, the automatic tail comes back
    later = _sent(ModelRequest("FERB instructions", ("facts", "history"), 2),
                  [{"role": "assistant", "content": [{"type": "text", "text": "x"}]},
                   {"role": "user", "content": [{"type": "text", "text": "y"}]}])
    assert later["cache_control"] == {"type": "ephemeral"}


# decision: RUN-437
def test_usage_splits_cache_writes_by_ttl_and_prices_hour_writes_at_twice_input() -> None:
    u = from_anthropic("claude-opus-5-5", {"input_tokens": 100, "cache_read_input_tokens": 3000,
                                            "cache_creation_input_tokens": 2000, "output_tokens": 50,
                                            "cache_creation": {"ephemeral_5m_input_tokens": 500,
                                                               "ephemeral_1h_input_tokens": 1500}}, billing="api")
    assert (u.cached_input_tokens, u.cache_write_tokens, u.cache_write_5m_tokens, u.cache_write_1h_tokens) == \
        (3000, 2000, 500, 1500)
    # Opus 5.5: $4 input, $0.20 read, $5 5m write, $8 1h write (2x input), $20 output
    expected = (100 * 4 + 3000 * 0.20 + 500 * 5 + 1500 * 8 + 50 * 20) / 1e6
    assert abs((u.cost_usd or 0) - expected) < 1e-12
    assert cost_usd("claude-opus-5-5", 2100, 0, 2000, 0, cache_write_1h=2000) == (100 * 4 + 2000 * 8) / 1e6
    # no split reported: every write counts as 5 minutes
    old = from_anthropic("claude-opus-5-5", {"input_tokens": 1, "cache_creation_input_tokens": 10}, billing="api")
    assert (old.cache_write_5m_tokens, old.cache_write_1h_tokens) == (10, 0)
