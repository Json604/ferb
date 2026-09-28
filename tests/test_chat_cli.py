"""`ferb chat`: bracketed pastes spanning lines are read as one message; events render as readable lines."""

import io

from ferb.cli.chat import read_message, render
from ferb.sanitize.untrusted import PASTE_END, PASTE_START


def test_a_multi_line_paste_is_one_message() -> None:
    inp = io.StringIO(f"summarize {PASTE_START}line one\nline two\nline three{PASTE_END} please\nnext\n")
    assert read_message(inp) == f"summarize {PASTE_START}line one\nline two\nline three{PASTE_END} please"
    assert read_message(inp) == "next"
    assert read_message(inp) is None


def test_render() -> None:
    assert render({"type": "reply", "text": "hi"}) == "ferb> hi"
    out = render({"type": "outcome", "status": "done", "summary": "wrote it", "artifacts": ["a.md"]})
    assert "done — wrote it" in out and "a.md" in out and "verification" in out
    assert "blocked (intent)" in render({"type": "event", "topic": "policy.blocked",
                                         "data": {"step": "intent", "reason": "x"}})
    assert "ferb undo hold-1" in render({"type": "event", "topic": "action.held",
                                         "data": {"kind": "send", "seconds": 10, "hold_id": "hold-1"}})
