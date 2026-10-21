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


def test_markers_that_cant_all_fit_fold_the_lowest_priority_run_into_one_note():
    """Fix wave 6b round 1 (I1): optional blocks of one run (`fold`) whose omission markers can't all fit beside the
    protected text fold, lowest priority first, into one note in the place of the first, naming the first and last
    and the run's route; blocks without `fold` keep their markers as before."""
    from ferb.core.context import assemble
    blocks = [ContextBlock("OWNER REQUEST", "request", protected=True),
              ContextBlock("unrelated optional " * 50, "memory.read item_id=m1"),
              *[ContextBlock("call body " * 20, f"call-{i}", priority=1 + i, fold="the calls' route") for i in range(200)]]
    out = assemble(blocks, budget_tokens=10 ** 9, max_chars=3_000)
    text = "\n\n".join(out.parts)
    assert out.chars <= 3_000 and "OWNER REQUEST" in text
    assert "[Context omitted; fetch full text: memory.read item_id=m1]" in text
    [note] = [p for p in out.parts if p.startswith("[") and "more not shown" in p]
    count = int(note[1:].split(" ", 1)[0])
    assert note == (f"[{count} more not shown or listed one by one; fetch full text: the calls' route — from call-0 to "
                    f"call-{count - 1}]")
    listed = [p for p in out.parts if p.startswith("[Context omitted; fetch full text: call-")]
    assert listed == [f"[Context omitted; fetch full text: call-{i}]" for i in range(count, 200)]   # the newer ones
    assert out.parts.index(note) == 2                                       # where the first folded block stood
    assert not any(out.shown[2:])


def test_blocks_that_fit_are_never_folded():
    from ferb.core.context import assemble
    blocks = [ContextBlock("OWNER REQUEST", "request", protected=True),
              *[ContextBlock(f"call {i}", f"call-{i}", priority=1 + i, fold="route") for i in range(5)]]
    out = assemble(blocks, budget_tokens=10 ** 9)
    assert all(out.shown) and "more not shown" not in "\n\n".join(out.parts)
