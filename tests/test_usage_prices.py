# tests/test_usage_prices.py
"""Every runtime reports usage the same way; cost comes from a dated price table, never guessed (D32)."""
import asyncio
import json
import sys
from pathlib import Path

import pytest

from ferb.core.types import RunHandle
from ferb.runtimes.codex_exec import CodexRuntime
from ferb.runtimes.usage import cost_usd, from_anthropic, from_codex, from_gemini, from_openai


def test_openai_usage_counts_cached_inside_input() -> None:
    u = from_openai("gpt-6.1-sol", {"input_tokens": 5100, "input_tokens_details": {"cached_tokens": 3000,
                    "cache_write_tokens": 2000}, "output_tokens": 50, "output_tokens_details": {"reasoning_tokens": 20}},
                    billing="api")
    assert (u.input_tokens, u.cached_input_tokens, u.cache_write_tokens, u.output_tokens, u.reasoning_tokens) == \
           (5100, 3000, 2000, 50, 20)
    # 100 fresh × $2 + 3000 cached × $0.10 + 2000 written × $2.50 + 50 out × $10, per 1M tokens
    assert u.cost_usd == pytest.approx((100 * 2 + 3000 * 0.10 + 2000 * 2.50 + 50 * 10) / 1e6)
    assert u.cost_basis == "price_table" and u.billing == "api"


def test_anthropic_input_adds_cache_reads_and_writes() -> None:
    u = from_anthropic("claude-sonnet-5-5", {"input_tokens": 100, "cache_creation_input_tokens": 2000,
                       "cache_read_input_tokens": 3000, "output_tokens": 50}, billing="api")
    assert (u.input_tokens, u.cached_input_tokens, u.cache_write_tokens) == (5100, 3000, 2000)
    assert u.cost_usd == pytest.approx((100 * 2 + 3000 * 0.20 + 2000 * 2.50 + 50 * 10) / 1e6)


def test_claude_sdk_cost_comes_from_the_sdk() -> None:
    u = from_anthropic("claude", {"input_tokens": 10, "output_tokens": 5}, billing="subscription", provider_cost=0.0123)
    assert u.cost_usd == 0.0123 and u.cost_basis == "provider" and u.billing == "subscription"


def test_gemini_output_includes_thoughts_and_input_includes_tool_prompt() -> None:
    u = from_gemini("gemini-3.8-flash", {"promptTokenCount": 5000, "cachedContentTokenCount": 3000,
                    "candidatesTokenCount": 30, "thoughtsTokenCount": 20, "toolUsePromptTokenCount": 100}, billing="api")
    assert (u.input_tokens, u.cached_input_tokens, u.output_tokens, u.reasoning_tokens) == (5100, 3000, 50, 20)


def test_codex_usage_is_an_api_equivalent_estimate() -> None:
    u = from_codex("gpt-5.5", {"input_tokens": 14840, "cached_input_tokens": 6656, "cache_write_input_tokens": 0,
                               "output_tokens": 60, "reasoning_output_tokens": 9})
    assert u.billing == "subscription" and u.cost_basis == "price_table" and u.cost_usd is not None


def test_unknown_model_has_no_cost() -> None:
    assert cost_usd("some-new-model", 1000, 0, 0, 10) is None
    assert from_openai("some-new-model", {"input_tokens": 1}, billing="api").cost_basis == "unknown"


def test_prices_change_on_their_date_and_long_prompts_cost_more() -> None:
    assert cost_usd("gemini-3.8-flash", 1_000_000, 0, 0, 0, day="2026-12-31") == pytest.approx(0.75)
    assert cost_usd("gemini-3.8-flash", 1_000_000, 0, 0, 0, day="2027-01-01") == pytest.approx(1.50)
    assert cost_usd("gpt-6-sol", 300_000, 0, 0, 0) == pytest.approx(300_000 * 2 * 2.0 / 1e6)   # above 272K: 2x input


@pytest.mark.anyio
async def test_codex_adapter_yields_normalized_usage(tmp_path: Path) -> None:
    line = json.dumps({"type": "turn.completed", "usage": {"input_tokens": 100, "cached_input_tokens": 40,
                                                           "output_tokens": 7, "reasoning_output_tokens": 2}})
    proc = await asyncio.create_subprocess_exec(sys.executable, "-c", f"print({line!r}, flush=True)",
                                                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    rt = CodexRuntime(Path("/unused"), "worker", tmp_path, tmp_path, timeout_s=None)
    run = RunHandle("t", rt.name, "t", {"proc": proc, "violation": None, "seen": set()})
    usage = [e.data async for e in rt.events(run) if e.kind == "usage"][0]
    assert usage["input_tokens"] == 100 and usage["cached_input_tokens"] == 40 and usage["billing"] == "subscription"
