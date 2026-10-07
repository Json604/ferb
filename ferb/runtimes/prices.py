"""Owns FERB's dated table of provider prices (USD per 1M tokens). Must not guess: an unlisted model has no price.

Checked 2026-10-07 against the official pages:
- OpenAI: https://developers.openai.com/api/docs/pricing and /api/docs/models/<id>; prompts above 272K tokens cost 2x input
  (cached and written too) and 1.5x output for the whole request; GPT-5.6+ cache writes cost 1.25x input.
- Anthropic: https://platform.claude.com/docs/en/about-claude/pricing (5-minute cache writes 1.25x input).
- Google: https://ai.google.dev/gemini-api/docs/pricing (gemini-3.8-flash doubles on 2027-01-01; 3.1-pro-preview above
  200K tokens: 2x input, 1.5x output).
Re-check against a billing statement before relying on totals; update by adding a dated entry, never editing an old one."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Price:
    input: float                     # uncached input
    cached: float                    # read from the provider's cache
    cache_write: float               # written to the cache (equal to input where writes aren't billed extra)
    output: float                    # generated tokens, reasoning included
    long_above: int | None = None    # prompt size above which the long-context factors apply to the whole request
    long_input: float = 1.0          # multiplies input, cached and cache_write
    long_output: float = 1.0
    since: str = "2000-01-01"        # ISO date this price starts


PRICES: dict[str, list[Price]] = {
    "gpt-6-astra": [Price(10.00, 1.00, 12.50, 50.00, 272_000, 2.0, 1.5)],
    "gpt-6.1-sol": [Price(2.00, 0.10, 2.50, 10.00, 272_000, 2.0, 1.5)],
    "gpt-6-sol": [Price(2.00, 0.20, 2.50, 10.00, 272_000, 2.0, 1.5)],
    "gpt-6-luna": [Price(0.10, 0.01, 0.125, 0.50)],
    "gpt-5.5": [Price(5.00, 0.50, 5.00, 30.00, 272_000, 2.0, 1.5)],
    "claude-fable-5-1": [Price(10.00, 0.25, 12.50, 50.00)],
    "claude-opus-5-5": [Price(4.00, 0.20, 5.00, 20.00)],
    "claude-sonnet-5-5": [Price(2.00, 0.20, 2.50, 10.00)],
    "claude-haiku-4-5": [Price(1.00, 0.10, 1.25, 5.00)],
    "claude-haiku-4-5-20251001": [Price(1.00, 0.10, 1.25, 5.00)],
    "gemini-3.8-flash": [Price(0.75, 0.075, 0.75, 3.75), Price(1.50, 0.15, 1.50, 7.50, since="2027-01-01")],
    "gemini-3.1-pro-preview": [Price(2.00, 0.20, 2.00, 12.00, 200_000, 2.0, 1.5)],
}
