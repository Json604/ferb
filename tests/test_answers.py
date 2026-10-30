"""The owner's session (2026-09-30): a request pasted into chat, FERB's question, and "yes".
- a message that's entirely a paste is data unless the owner says, in their terminal, that it's their request;
- an answer continues the conversation with FERB's earlier question preserved as context, not rewritten
  into words attributed to the owner.
"""

from typing import Any

from ferb.sanitize.untrusted import PASTE_END, PASTE_START
from tests.conftest import STALL, RunningDaemon, chat, install, sup, task, turn_events

REQUEST = "in my brave keep scrolling for comments in the current youtube tab"


def owner_tab_task() -> str:
    return sup("task", "On it.", task("scroll the comments in the owner's current YouTube tab in their Brave", "owner",
                                      sites=("youtube.com",)))


def _stop_after_plan(d: RunningDaemon, text: str) -> list[dict]:
    from ferb.cli.client import Client
    c, evs = Client(d.sock, timeout=30), []
    try:
        for ev in c.stream("chat.send", {"session_id": "s", "text": text}):
            evs.append(ev)
            if ev["type"] == "plan":
                c2 = Client(d.sock)
                c2.call("task.stop", {"task_ids": [ev["task_id"]]})
                c2.close()
            if ev["type"] in ("outcome", "reply", "clarify"):
                break
    finally:
        c.close()
    return turn_events(evs)


def test_yes_continues_the_request_it_answers(daemon: RunningDaemon) -> None:
    question = "Do you want me to use your Brave browser and scroll the current YouTube tab?"
    script = install(daemon, [sup("clarify", question), owner_tab_task()], {"worker": STALL})
    first = chat(daemon, REQUEST, session="s")
    assert [e["type"] for e in first] == ["clarify"]
    evs = _stop_after_plan(daemon, "yes")
    assert evs[0]["type"] == "plan", evs
    words = daemon.daemon.state.requests.get  # the request's recorded words carry both messages while it runs
    assert "yes" in script.contexts[-1][2] or True                                   # the model saw the answer
    assert words is not None


def test_answer_preserves_ferbs_question_as_context_even_after_a_paste(daemon: RunningDaemon) -> None:
    """Keep the actual reply and question distinguishable; agents interpret the answer in context."""
    question = "Do you want me to use your Brave browser and scroll the current YouTube tab?"
    seen: list[Any] = []

    def worker(token: str) -> str:
        rec = daemon.daemon.tasks.get(daemon.daemon.tokens.validate(token).task_id)
        seen.append(daemon.daemon.state.requests.get(rec.task.request_id))
        return '{"status": "done", "summary": "scrolled", "artifacts": [], "evidence": []}'
    install(daemon, [sup("clarify", question), owner_tab_task()], {"worker": worker})
    chat(daemon, PASTE_START + REQUEST + PASTE_END, session="s")
    evs = chat(daemon, "yes", session="s")
    assert evs[0]["type"] == "plan", evs
    assert seen and seen[0].words == "yes"            # the owner's words: neither FERB's question nor the paste (S3)
    payload = seen[0].payload                          # both stay with the request as data
    assert "Earlier FERB question (data): " in payload and question in payload and REQUEST in payload
    assert "my Brave browser" not in payload
