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
    and the run's route; blocks without `fold` keep their markers as before. Since L29 Task 5 fix round 1 the run's
    highest-priority blocks are shown whole when folding more of its lower ones makes room for them."""
    from ferb.core.context import assemble
    blocks = [ContextBlock("OWNER REQUEST", "request", protected=True),
              ContextBlock("unrelated optional " * 50, "memory.read item_id=m1"),
              *[ContextBlock("call body " * 20, f"call-{i}", priority=1 + i, fold="the calls' route")
                for i in range(200)]]
    out = assemble(blocks, budget_tokens=10 ** 9, max_chars=3_000)
    text = "\n\n".join(out.parts)
    assert out.chars <= 3_000 and "OWNER REQUEST" in text
    assert "[Context omitted; fetch full text: memory.read item_id=m1]" in text
    [note] = [p for p in out.parts if p.startswith("[") and "more not shown" in p]
    count = int(note[1:].split(" ", 1)[0])
    assert note == (f"[{count} more not shown or listed one by one; fetch full text: the calls' route — from call-0 "
                    f"to call-{count - 1}]")
    shown = [i for i in range(200) if out.shown[2 + i]]
    assert shown and shown == list(range(200 - len(shown), 200))            # the newest, whole
    listed = [p for p in out.parts if p.startswith("[Context omitted; fetch full text: call-")]
    assert listed == [f"[Context omitted; fetch full text: call-{i}]" for i in range(count, shown[0])]   # the newer
    assert out.parts.index(note) == 2                                       # where the first folded block stood


def test_blocks_that_fit_are_never_folded():
    from ferb.core.context import assemble
    blocks = [ContextBlock("OWNER REQUEST", "request", protected=True),
              *[ContextBlock(f"call {i}", f"call-{i}", priority=1 + i, fold="route") for i in range(5)]]
    out = assemble(blocks, budget_tokens=10 ** 9)
    assert all(out.shown) and "more not shown" not in "\n\n".join(out.parts)


# L29 Task 5 fix round 1: a run's markers also fold to make room for a higher-priority block ---------------------------

def _tokens(text: str) -> int:
    """A cheap stand-in tokenizer for the property trials: not the character count, so the two limits differ."""
    return (len(text.encode()) + 3) // 4


def _random_blocks(rng, *, runs: int) -> list:
    """Protected blocks, conversation entries and optional blocks of random sizes and priorities; `runs` fold routes
    (none: no block has `fold`)."""
    blocks = []
    for n in range(rng.randint(1, 30)):
        kind = rng.choice(("protected", "history", "optional", "optional", "optional"))
        text = rng.choice(("ab ", "数据 ", "word ")) * rng.randint(1, 120)
        route = f"route {rng.randint(1, runs)}" if runs and kind == "optional" and rng.random() < 0.6 else ""
        blocks.append(ContextBlock(text if rng.random() > 0.05 else "", f"ref-{n:03}", protected=kind == "protected",
                                   history=kind == "history", priority=rng.randint(0, 6), cache=rng.random() < 0.2,
                                   fold=route))
    return blocks


def test_without_a_run_assembly_is_byte_identical_to_before(monkeypatch):
    """A context without a fold group is assembled exactly as before runs could fold for a higher-priority block:
    random trials against the earlier `assemble` (tests/support/context_reference.py), result or refusal alike."""
    import random
    from ferb.core.context import assemble
    from ferb.core.errors import InputTooLarge
    from tests.support.context_reference import assemble_before_priority_folds
    monkeypatch.setattr("ferb.core.context.count_tokens", _tokens)
    rng = random.Random(29)
    for _ in range(600):
        blocks = _random_blocks(rng, runs=0)
        limits = dict(budget_tokens=rng.randint(50, 4_000), reserved_tokens=rng.randint(0, 200),
                      max_chars=rng.choice((None, rng.randint(200, 12_000))))
        outcomes = []
        for build in (assemble, assemble_before_priority_folds):
            try:
                outcomes.append(build(blocks, **limits))
            except InputTooLarge as e:
                outcomes.append(("refused", e.args))
        assert outcomes[0] == outcomes[1], (blocks, limits)


def test_a_block_is_never_left_out_while_lower_priority_run_markers_could_make_room(monkeypatch):
    """Property (L29 Task 5 fix round 1): an optional block left out with its marker listed could not have been shown
    whole by folding every lower-priority run marker still listed into its run's note; and a folded block leaves no
    lower-priority run marker listed (runs fold lowest first). Measured on the text sent, within what assemble charges
    beyond it (its report and notes at their longest, a note for a run whose blocks it later showed): a tolerance far
    below every block's size here, so a block that folding could have fitted is caught."""
    import random
    import re
    from ferb.core.context import _fold, _report, assemble
    from ferb.core.errors import InputTooLarge
    monkeypatch.setattr("ferb.core.context.count_tokens", _tokens)
    rng = random.Random(529)
    note_re = re.compile(r"^\[(\d+) more not shown or listed one by one; fetch full text: (.+) — from (\S+) "
                         r"to (\S+)\]$")
    checked = folds = 0
    for _ in range(800):
        blocks = [ContextBlock("REQUEST " * rng.randint(1, 60), "req", protected=True)]
        for n in range(rng.randint(2, 40)):
            route = f"route {rng.randint(1, 3)}" if rng.random() < 0.7 else ""
            blocks.append(ContextBlock("x" * rng.randint(300, 1_000), f"ref-{n:03}", priority=rng.randint(0, 50),
                                       fold=route))
        cap = rng.randint(800, 20_000)
        try:
            out = assemble(blocks, budget_tokens=10 ** 9, max_chars=cap)
        except InputTooLarge:
            continue
        marker = {i: f"[Context omitted; fetch full text: {b.reference}]" for i, b in enumerate(blocks)}
        listed = {i for i in range(1, len(blocks)) if marker[i] in out.parts}
        notes = {m.group(2): part for part in out.parts if (m := note_re.match(part))}
        folded = {i for i in range(1, len(blocks)) if not out.shown[i] and i not in listed}
        assert all(blocks[i].fold in notes for i in folded)
        folds += bool(notes)
        longest = {route: len(_fold(10 ** 9, "", "", route)) + 2 * len("ref-000") + 2 for route in ("route 1",
                                                                                                    "route 2",
                                                                                                    "route 3")}
        tolerance = (2 + len(_report(10 ** 9, 0, 10 ** 9, True, len(blocks))) - len(out.parts[-1]) +
                     sum(longest[route] - 2 - len(part) for route, part in notes.items()) + max(longest.values()))
        for x in listed:
            lower = [i for i in listed if blocks[i].fold and blocks[i].priority < blocks[x].priority]
            if not lower:
                continue
            savings = sum(len(marker[i]) + 2 for i in lower) - sum(
                longest[route] for route in {blocks[i].fold for i in lower} - set(notes))
            assert cap - out.chars + savings < len(blocks[x].text) - len(marker[x]) + tolerance, (blocks, cap, x)
            checked += 1
        for x in folded:
            assert not [i for i in listed if blocks[i].fold and blocks[i].priority < blocks[x].priority], (blocks, x)
    assert checked and folds


def test_a_higher_priority_block_stays_whole_while_a_lower_run_folds():
    """The review's case: the markers of a long run fill the window, and a higher-priority block (this task's own
    earlier attempt) and the run's newest blocks are shown whole, the run's older ones counted in its note."""
    from ferb.core.context import assemble
    blocks = [ContextBlock("OWNER REQUEST", "request", protected=True),
              ContextBlock("ATTEMPT " * 60, "task.evidence span_id=attempts", priority=500),
              *[ContextBlock(f"run {i} " * 40, f"memory.learned item_id=run-{i:03}", priority=3 + i, fold="runs")
                for i in range(200)]]
    out = assemble(blocks, budget_tokens=10 ** 9, max_chars=6_000)
    text = "\n\n".join(out.parts)
    assert out.chars <= 6_000 and out.shown[1] and "ATTEMPT " * 60 in text
    shown = [i for i in range(200) if out.shown[2 + i]]
    assert shown and shown == list(range(200 - len(shown), 200))           # the newest, whole
    [note] = [p for p in out.parts if "more not shown" in p]
    assert note.endswith("fetch full text: runs — from memory.learned item_id=run-000 to memory.learned item_id="
                         f"run-{199 - len(shown) - sum(p.startswith('[Context omitted') for p in out.parts):03}]")
