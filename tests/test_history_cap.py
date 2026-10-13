"""The planner's conversation fills the model's window newest-first (L28); an entry is never clipped: it is shown
complete or counted in the omission note (chat.history keeps it). Resumable attempts keep their newest within a cap."""
from ferb.core.context import ContextBlock, assemble, count_tokens
from tests.conftest import RunningDaemon


def _entries(n: int, words: int = 120) -> list[ContextBlock]:
    return [ContextBlock(f"message number {i} " + "filler " * words, f"chat.history entry_id=e{i}", cache=True,
                         history=True) for i in range(n)]


def test_long_history_keeps_the_newest_entries_that_fit() -> None:
    blocks = [ContextBlock("Layer 1", "facts", protected=True, cache=True), *_entries(40),
              ContextBlock("the situation", "now", protected=True), ContextBlock("the new message", "m", protected=True)]
    out = assemble(blocks, budget_tokens=3000)
    text = "\n\n".join(out.parts)
    assert "message number 39 " in text and "message number 0 " not in text
    assert out.omitted_history and f"[{out.omitted_history} older entries not shown here; fetch full text: chat.history]" in text
    assert text.index("message number 38 ") < text.index("message number 39 ") < text.index("the situation")
    shown = [b for b, seen in zip(blocks, out.shown) if seen and b.history]
    assert shown == blocks[1:41][-len(shown):]                  # newest-first, never with a gap
    assert count_tokens("\n\n".join(out.parts[:-1])) <= 3000
    assert out.cacheable == 2 + len(shown)                      # Layer 1, the note, the shown entries


def test_an_entry_larger_than_the_room_left_is_never_clipped() -> None:
    big = "pasted document line. " * 3000
    blocks = [ContextBlock("a small first message", "chat.history entry_id=a", history=True),
              ContextBlock(big, "chat.history entry_id=b", history=True),
              ContextBlock("the new message", "m", protected=True)]
    text = "\n\n".join(assemble(blocks, budget_tokens=500).parts)
    assert big.strip() not in text and "a small first message" not in text     # no gap: older ones stay out too
    assert "2 older entries not shown" in text


def test_attempts_text_keeps_newest_within_cap(daemon: RunningDaemon) -> None:
    rows = [{"task_id": f"t{i}", "prior_task": None, "status": "failed", "goal": f"goal {i} " + "x " * 80}
            for i in range(50)]                        # candidates() returns newest first
    text = daemon.daemon.supervisor._attempts_text(rows, max_tokens=800)
    assert "task_id=t0 " in text and "task_id=t49 " not in text
    assert count_tokens(text) <= 800 + 100 and "older resumable attempts not shown" in text
