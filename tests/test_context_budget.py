"""Budget accounting must preserve complete protected sources and make omissions recoverable."""

from ferb.core.context import ContextBlock, assemble_context, count_tokens


def test_whole_optional_blocks_are_omitted_with_fetch_reference():
    required = "OWNER REQUEST " * 10
    optional = "earlier observation " * 1000
    result = assemble_context([ContextBlock(required, "request", protected=True),
                               ContextBlock(optional, "task.evidence span_id=source-1")], budget_tokens=300)
    assert required in result
    assert optional not in result
    assert "omitted" in result.lower() and "task.evidence span_id=source-1" in result
    assert count_tokens(result) <= 300


def test_protected_text_is_never_clipped_even_above_budget():
    required = "完整的用户请求。" * 1000
    result = assemble_context([ContextBlock(required, "request", protected=True)], budget_tokens=10)
    assert required in result
    assert "over_budget" in result and "true" in result


def test_unicode_and_oversized_token_like_text_count_without_special_token_errors():
    assert count_tokens("<|endoftext|> Hello 世界 🦊") > 0


def test_token_estimation_processes_all_text_in_bounded_work_chunks(monkeypatch):
    import ferb.core.context as module
    seen = []
    class Encoding:
        def encode_ordinary(self, text):
            seen.append(text)
            return list(text)
    monkeypatch.setattr(module, '_encoding', lambda: Encoding())
    source = 'unbroken' * 10000
    assert module.count_tokens(source) == len(source)
    assert ''.join(seen) == source
    assert max(map(len, seen)) <= 8192
