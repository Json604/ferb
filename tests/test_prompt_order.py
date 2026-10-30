"""Stable content first, changing content last, so prompt caching can reuse the prefix (P20)."""
from ferb.core.context import ContextBlock, assemble_context
from tests.conftest import RunningDaemon, chat, install, sup
from tests.support.history import conversation_text


def _recent(daemon: RunningDaemon) -> str:
    async def go():
        return conversation_text(daemon.daemon.supervisor, "s1")
    return daemon.run(go())


def test_budget_report_comes_last() -> None:
    out = assemble_context([ContextBlock("stable block", "a", protected=True),
                            ContextBlock("changing block", "b", protected=True)], budget_tokens=1000)
    assert out.startswith("stable block")
    assert out.rsplit("\n\n", 1)[1].startswith("Context budget: ")


def test_supervisor_orders_stable_layers_first_and_the_new_message_last(daemon) -> None:
    """L28: system, Layer 1 and the append-only conversation first (cacheable), then the blocks that change every turn:
    the situation, activity, memory, task state and questions, and the owner's new message last."""
    script = install(daemon, [sup("reply", "first"), sup("reply", "second")])
    chat(daemon, "an earlier message")
    chat(daemon, "the new message")
    ctx = [c for _, k, c, _ in script.contexts if k == "supervisor"][-1]
    body = ctx.rsplit("\n\nContext budget: ", 1)[0]
    assert body.startswith("Layer 1 — stable device and capabilities:\nThis Mac: ")
    assert body.index("Layer 1 —") < body.index("Layer 5 — conversation:") < body.index("an earlier message")
    assert body.index("an earlier message") < body.index("Layer 2 —") < body.index("Layer 3 —")
    assert body.index("Layer 3 —") < body.index("Layer 6 — open questions") < body.index("Layer 5 — owner's complete")
    assert body.endswith("Layer 5 — owner's complete new message:\nthe new message")


def test_history_only_grows_at_the_end_as_the_conversation_continues(daemon: RunningDaemon) -> None:
    install(daemon, [sup("reply", f"answer {i}") for i in range(3)])
    chat(daemon, "first message")
    before = _recent(daemon)
    chat(daemon, "second message")
    after = _recent(daemon)
    assert after.startswith(before)                            # earlier lines unchanged, so the cached prefix holds
    assert "[c1] " in before and "first message" in before.split("[c2] ")[0]   # c1 is the oldest entry
