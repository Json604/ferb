"""Chat history and resumable attempts are capped to their newest entries, never dropped whole."""
from ferb.core.context import count_tokens
from tests.conftest import RunningDaemon, chat, install, sup


def _recent(daemon: RunningDaemon, **kw):
    async def go():
        return daemon.daemon.supervisor.recent(conversation="s1", **kw)
    return daemon.run(go())


def test_long_history_keeps_newest_entries_within_cap(daemon: RunningDaemon) -> None:
    install(daemon, [sup("reply", f"answer {i}") for i in range(40)])
    for i in range(40):
        chat(daemon, f"message number {i} " + "filler " * 120)
    text, _ = _recent(daemon, max_tokens=3000)
    assert count_tokens(text) <= 3000 + 200          # cap plus the header/omission lines
    assert "message number 39" in text                # the newest turn is always there
    assert "message number 0 " not in text            # the oldest was left out
    assert "older entries not shown" in text and "chat.history" in text
    assert text.index("message number 38") < text.index("message number 39")   # oldest first


def test_newest_entry_larger_than_cap_is_kept_whole(daemon: RunningDaemon) -> None:
    big = "pasted document line. " * 3000
    install(daemon, [sup("reply", "ok"), sup("reply", big)])      # the newest journal entry is the big reply
    chat(daemon, "a small first message")
    chat(daemon, "now the long one")
    text, _ = _recent(daemon, max_tokens=500)
    assert big.strip() in text                        # never clipped
    assert "a small first message" not in text and "older entries not shown" in text


def test_attempts_text_keeps_newest_within_cap(daemon: RunningDaemon) -> None:
    rows = [{"task_id": f"t{i}", "prior_task": None, "status": "failed", "goal": f"goal {i} " + "x " * 80}
            for i in range(50)]                        # candidates() returns newest first
    text = daemon.daemon.supervisor._attempts_text(rows, max_tokens=800)
    assert "task_id=t0 " in text and "task_id=t49 " not in text
    assert count_tokens(text) <= 800 + 100 and "older resumable attempts not shown" in text
