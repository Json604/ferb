"""Stable content first, changing content last, so prompt caching can reuse the prefix (P20)."""
from ferb.core.context import ContextBlock, assemble_context
from tests.conftest import RunningDaemon, chat, install, sup


def _recent(daemon: RunningDaemon) -> str:
    async def go():
        return daemon.daemon.supervisor.recent(conversation="s1")
    return daemon.run(go())[0]


def test_budget_report_comes_last() -> None:
    out = assemble_context([ContextBlock("stable block", "a", protected=True),
                            ContextBlock("changing block", "b", protected=True)])
    assert out.startswith("stable block")
    assert out.rsplit("\n\n", 1)[1].startswith("Context budget: ")


def test_supervisor_puts_device_facts_then_history_first_and_the_owner_message_last(daemon) -> None:
    script = install(daemon, [sup("reply", "first"), sup("reply", "second")])
    chat(daemon, "an earlier message")
    chat(daemon, "the new message")
    ctx = [c for _, k, c, _ in script.contexts if k == "supervisor"][-1]
    body = ctx.rsplit("\n\nContext budget: ", 1)[0]
    assert body.startswith("This Mac: ")                       # installed apps: the same on every turn
    assert body.index("This Mac: ") < body.index("Conversation so far") < body.index("Owner's new message")
    assert body.rstrip().endswith("Owner's new message:\nthe new message")


def test_history_only_grows_at_the_end_as_the_conversation_continues(daemon: RunningDaemon) -> None:
    install(daemon, [sup("reply", f"answer {i}") for i in range(3)])
    chat(daemon, "first message")
    before = _recent(daemon)
    chat(daemon, "second message")
    after = _recent(daemon)
    assert after.startswith(before)                            # earlier lines unchanged, so the cached prefix holds
    assert "[c1] " in before and "first message" in before.split("[c2] ")[0]   # c1 is the oldest entry
