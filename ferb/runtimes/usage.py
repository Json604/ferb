"""Owns one usage record for every runtime (D32): tokens in, cached, written, out, reasoning, and the cost basis.

Providers count differently: OpenAI and Gemini input includes cached tokens, Anthropic's input_tokens is only what follows
the last cache breakpoint; Gemini's candidates exclude thoughts. Each from_* turns one provider's numbers into the same
meaning. Subscription runtimes (Codex, Claude Agent SDK) report an API-equivalent estimate, labelled as such."""

from dataclasses import dataclass
from datetime import date
from typing import Any

from ferb.runtimes.prices import PRICES


@dataclass(frozen=True)
class Usage:
    model: str
    input_tokens: int                # every prompt token, cached or not
    cached_input_tokens: int         # read from the provider's cache
    cache_write_tokens: int          # written to the cache by this call
    output_tokens: int               # every generated token, reasoning included
    reasoning_tokens: int
    cost_usd: float | None           # None when the model has no listed price
    cost_basis: str                  # "provider" (the runtime reported it) | "price_table" | "unknown"
    billing: str                     # "api" (billed per token) | "subscription" (cost is the API-equivalent estimate)
    # RUN-436: Anthropic's cache writes by TTL (cache_write_tokens is their sum; a provider without the split: all 5m)
    cache_write_5m_tokens: int = 0
    cache_write_1h_tokens: int = 0


def cost_usd(model: str, input_tokens: int, cached: int, cache_write: int, output: int, day: str | None = None,
             cache_write_1h: int = 0) -> float | None:
    """`cache_write_1h`: the part of `cache_write` written with the 1-hour TTL, priced at the model's 1-hour write
    price (RUN-436); the rest at its 5-minute one."""
    entries = [p for p in PRICES.get(model, []) if p.since <= (day or date.today().isoformat())]
    if not entries:
        return None
    p = max(entries, key=lambda e: e.since)
    long = p.long_above is not None and input_tokens > p.long_above
    fi, fo = (p.long_input, p.long_output) if long else (1.0, 1.0)
    fresh = max(input_tokens - cached - cache_write, 0)
    hour = min(cache_write_1h, cache_write)
    writes = (cache_write - hour) * p.cache_write + hour * (p.cache_write_1h or p.cache_write)
    return (fresh * p.input * fi + cached * p.cached * fi + writes * fi + output * p.output * fo) / 1e6


def _make(model: str, inp: int, cached: int, write: int, out: int, reasoning: int, billing: str,
          provider_cost: float | None = None, write_1h: int = 0) -> Usage:
    split = {"cache_write_5m_tokens": write - min(write_1h, write), "cache_write_1h_tokens": min(write_1h, write)}
    if provider_cost is not None:
        return Usage(model, inp, cached, write, out, reasoning, float(provider_cost), "provider", billing, **split)
    cost = cost_usd(model, inp, cached, write, out, cache_write_1h=write_1h)
    return Usage(model, inp, cached, write, out, reasoning, cost, "price_table" if cost is not None else "unknown", billing,
                 **split)


def _int(d: dict[str, Any], key: str) -> int:
    return int(d.get(key) or 0)


def from_openai(model: str, usage: dict[str, Any], billing: str) -> Usage:
    details = usage.get("input_tokens_details") or {}
    out = usage.get("output_tokens_details") or {}
    return _make(model, _int(usage, "input_tokens"), _int(details, "cached_tokens"), _int(details, "cache_write_tokens"),
                 _int(usage, "output_tokens"), _int(out, "reasoning_tokens"), billing)


def from_codex(model: str, usage: dict[str, Any]) -> Usage:
    """Codex's turn.completed usage (OpenAI meaning: input includes cached, output includes reasoning)."""
    return _make(model, _int(usage, "input_tokens"), _int(usage, "cached_input_tokens"),
                 _int(usage, "cache_write_input_tokens"), _int(usage, "output_tokens"),
                 _int(usage, "reasoning_output_tokens"), "subscription")


def from_anthropic(model: str, usage: dict[str, Any], billing: str, provider_cost: float | None = None) -> Usage:
    read, write = _int(usage, "cache_read_input_tokens"), _int(usage, "cache_creation_input_tokens")
    by_ttl = usage.get("cache_creation") if isinstance(usage.get("cache_creation"), dict) else {}
    return _make(model, _int(usage, "input_tokens") + read + write, read, write, _int(usage, "output_tokens"), 0,
                 billing, provider_cost, _int(by_ttl, "ephemeral_1h_input_tokens"))


def from_gemini(model: str, meta: dict[str, Any], billing: str) -> Usage:
    thoughts = _int(meta, "thoughtsTokenCount")
    return _make(model, _int(meta, "promptTokenCount") + _int(meta, "toolUsePromptTokenCount"),
                 _int(meta, "cachedContentTokenCount"), 0, _int(meta, "candidatesTokenCount") + thoughts, thoughts, billing)
