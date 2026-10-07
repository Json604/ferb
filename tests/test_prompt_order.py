"""Stable content first, changing content last, so prompt caching can reuse the prefix (P20)."""
from ferb.core.context import ContextBlock, assemble_context
from tests.conftest import chat, install, sup


def test_budget_report_comes_last() -> None:
    out = assemble_context([ContextBlock("stable block", "a", protected=True),
                            ContextBlock("changing block", "b", protected=True)])
    assert out.startswith("stable block")
    assert out.rsplit("\n\n", 1)[1].startswith("Context budget: ")


def test_supervisor_puts_history_first_and_the_owner_message_last(daemon) -> None:
    script = install(daemon, [sup("reply", "first"), sup("reply", "second")])
    chat(daemon, "an earlier message")
    chat(daemon, "the new message")
    ctx = [c for _, k, c, _ in script.contexts if k == "supervisor"][-1]
    body = ctx.rsplit("\n\nContext budget: ", 1)[0]
    assert body.startswith("Conversation so far")
    assert body.rstrip().endswith("Owner's new message:\nthe new message")
