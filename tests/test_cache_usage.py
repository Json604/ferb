"""Each call's cache reads and writes as the provider reports them (RUN-438, owner 2026-11-08: "Keep 30 min, measure
(Recommended)"): OpenAI's cached and written tokens, Gemini's implicit hits, a write count the provider doesn't report
marked as unknown rather than zero, and GPT-5.6+ writes priced at 1.25x input. Recorded-style usage payloads."""
from ferb.runtimes.prices import PRICES
from ferb.runtimes.usage import from_gemini, from_openai


# decision: RUN-438
def test_openai_reads_and_writes_are_recorded_and_writes_priced_at_one_and_a_quarter() -> None:
    u = from_openai("gpt-5.6-sol", {"input_tokens": 12000, "output_tokens": 300,
                                    "input_tokens_details": {"cached_tokens": 8000, "cache_write_tokens": 3000},
                                    "output_tokens_details": {"reasoning_tokens": 120}}, billing="api")
    assert (u.input_tokens, u.cached_input_tokens, u.cache_write_tokens, u.cache_write_reported) == \
        (12000, 8000, 3000, True)
    assert (u.cache_write_5m_tokens, u.cache_write_1h_tokens) == (0, 0)            # Anthropic's TTL split only
    price = PRICES["gpt-5.6-sol"][0]
    assert price.cache_write == 1.25 * price.input
    assert abs((u.cost_usd or 0) - (1000 * 4.00 + 8000 * 0.40 + 3000 * 5.00 + 300 * 20.00) / 1e6) < 1e-12


# decision: RUN-438
def test_a_write_count_openai_doesnt_report_is_marked_unknown() -> None:
    u = from_openai("gpt-5.5", {"input_tokens": 5000, "output_tokens": 10,
                                "input_tokens_details": {"cached_tokens": 4096}}, billing="api")
    assert u.cached_input_tokens == 4096 and u.cache_write_tokens == 0 and u.cache_write_reported is False


# decision: RUN-438
def test_gemini_implicit_hits_are_the_reads_and_writes_are_not_reported() -> None:
    u = from_gemini("gemini-3.8-flash", {"promptTokenCount": 9000, "cachedContentTokenCount": 6000,
                                         "candidatesTokenCount": 200, "thoughtsTokenCount": 50}, billing="api")
    assert (u.input_tokens, u.cached_input_tokens, u.cache_write_tokens, u.cache_write_reported) == \
        (9000, 6000, 0, False)
    assert u.output_tokens == 250 and u.reasoning_tokens == 50
