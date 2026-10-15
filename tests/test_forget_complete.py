"""Memory review I2 (fix wave 3): forget is complete, and the chat can forget.

Forgetting an item also removes what FERB's own records say of it: the code line "Remembered: …", FERB's whole answer
to the forgotten message, and each line of FERB's replies given the item that states it, wherever it was recorded
(journal rows, the planner's stored turns and exposures). A forget asked in chat waits for a plain Yes or No to a
question that names what goes, like `ferb memory forget`. Ported from the review's probes P2 and P7."""
import json
import re
from typing import Any

from tests.conftest import RunningDaemon, chat, install


def _on_loop(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


def _reply(text: str, **more: Any) -> str:
    return json.dumps({"kind": "reply", "text": text, "display_text": text, **more})


def _remember(d: RunningDaemon, said: str, content: str, key: str) -> Any:
    install(d, [_reply("Okay.", remember=[{"kind": "person", "subject": "owner", "key": key, "content": content,
                                           "quote": content}])])
    chat(d, said)
    return _on_loop(d, lambda: next(i for i in d.daemon.memory.items("active") if i.key == key))


def _holding(d: RunningDaemon, word: str) -> tuple[list[str], list[str]]:
    db = d.daemon.db
    return _on_loop(d, lambda: (
        [r[0] for r in db.conn.execute("SELECT kind FROM journal WHERE data_json LIKE ?", (f"%{word}%",))],
        [r[0] for r in db.conn.execute("SELECT kind FROM spans WHERE input_json LIKE ? OR output_json LIKE ?",
                                       (f"%{word}%", f"%{word}%"))]))


def test_forget_removes_ferbs_remembered_line_and_the_planner_no_longer_reads_it(daemon: RunningDaemon) -> None:
    """P2 and P7: after a forget, FERB's "Remembered: …" reply and the planner's stored turns still held the item, and
    the next planner turn read it from the chat."""
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    install(daemon, [_reply("Noted.")])
    chat(daemon, "what else should I know?")
    _on_loop(daemon, lambda: daemon.daemon.memory.forget(item.id))
    assert _holding(daemon, "Quillfeather") == ([], [])
    script = install(daemon, [_reply("Hi.")])
    chat(daemon, "who is my dentist?")
    context = "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")
    assert "Quillfeather" not in context


def test_forget_removes_a_reply_line_that_stated_the_item_in_other_words(daemon: RunningDaemon) -> None:
    """FERB answered from the item in its own words. That line goes; the rest of the chat stays."""
    item = _remember(daemon, "remember that my test lab colour is teal-otter", "my test lab colour is teal-otter",
                     "test_lab_colour")
    install(daemon, [_reply("Your test lab colour is teal-otter.\nAnything else?")])
    chat(daemon, "what is my test lab colour?")
    _on_loop(daemon, lambda: daemon.daemon.memory.forget(item.id))
    journal, spans = _holding(daemon, "otter")
    assert journal == [] and spans == []
    rows = _on_loop(daemon, lambda: [(e.kind, e.data.get("text")) for e in daemon.daemon.journal.entries_of(
        "chat.message", "chat.reply")])
    assert ("chat.message", "what is my test lab colour?") in rows                # the owner's question stays
    assert any(kind == "chat.reply" and "Anything else?" in str(text) for kind, text in rows)
    script = install(daemon, [_reply("I don't know.")])
    chat(daemon, "what is my test lab colour?")
    assert not re.search("teal|otter", "\n".join(str(c) for _, k, c, _ in script.contexts if k == "supervisor"))


def test_the_forget_confirmation_names_the_items_that_go(daemon: RunningDaemon) -> None:
    """Review M4: the confirmation showed counts only, while items built on the same sources went too."""
    from ferb.cli.client import Client
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    c = Client(daemon.sock)
    try:
        planned = c.call("memory.forget", {"id": item.id})
    finally:
        c.close()
    assert planned["confirmed"] is False
    assert any("“my dentist is Dr. Quillfeather”" in line for line in planned["will_delete"]), planned


def _forget_question(d: RunningDaemon) -> Any:
    return _on_loop(d, lambda: next((q for q in d.daemon.state.approvals.pending()
                                     if q.capability == "memory.forget"), None))


def test_chat_can_forget_after_a_yes_to_a_question_naming_what_goes(daemon: RunningDaemon) -> None:
    """Review live test 4: "forget my test lab colour" in the notch said it forgot, but the item stayed."""
    item = _remember(daemon, "remember that my test lab colour is violet-heron", "my test lab colour is violet-heron",
                     "test_lab_colour")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget my test lab colour"}])])
    asked = chat(daemon, "forget my test lab colour")
    question = _forget_question(daemon)
    assert question is not None and "violet-heron" in question.reason and question.options == ("Yes", "No")
    assert any("violet-heron" in str(e.get("text")) for e in asked)
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)) is not None      # nothing gone before the Yes
    install(daemon, [json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": question.id,
                                                                                    "answer": "Yes"}})])
    told = "\n".join(str(e.get("text")) for e in chat(daemon, "Yes"))
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)) is None
    assert "Forgot" in told
    assert _holding(daemon, "heron")[0] == []


def test_a_no_keeps_the_item(daemon: RunningDaemon) -> None:
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget my dentist"}])])
    chat(daemon, "forget my dentist")
    question = _forget_question(daemon)
    install(daemon, [json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": question.id,
                                                                                    "answer": "No"}})])
    chat(daemon, "No")
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)) is not None


def test_a_chat_forget_needs_the_owners_own_words(daemon: RunningDaemon) -> None:
    """Only the owner's words carry authority (MP2): a forget the planner wrote without them asks nothing."""
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget everything"}])])
    chat(daemon, "summarize the page I'm looking at")
    assert _forget_question(daemon) is None
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)) is not None
