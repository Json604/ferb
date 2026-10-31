"""Step 12d end-to-end runs (tests decision 1): a chat turn through the real daemon, gateway and scripted model, for the
confirmations the target design names: a send to someone new asks and a No sends nothing; an action whose outcome is
unknown is never repeated without the owner; one payment asks once. (The other runs: test_execution_runs, test_chat_turns,
test_merged_checker, test_answers_in_steps, test_typed_cards; a file write checked: test_careful_checks; a forget removes
the item: test_forget_in_chat.)"""
import json
from typing import Any

from pydantic import BaseModel

from ferb.core.types import ActionFacts, ActionSpec, Target, ToolResult
from ferb.policy.rules import Kind
from tests.conftest import Answers, FakeCap, RunningDaemon, chat, done, install, outcome, sup, task, tool


def _owner_questions(d: RunningDaemon) -> list[dict[str, Any]]:
    """FERB's own questions to the owner, as recorded (the owner.ask spans)."""
    async def go() -> list[dict[str, Any]]:
        rows = d.daemon.db.conn.execute("SELECT input_json FROM spans WHERE kind = 'tool.call' AND "
                                        "json_extract(input_json, '$.tool') = 'owner.ask' ORDER BY rowid").fetchall()
        return [json.loads(r["input_json"]) for r in rows]
    return d.run(go())


def _worker(d: RunningDaemon, calls: list[tuple[str, dict[str, Any]]], seen: list[dict[str, Any]]) -> Any:
    def act(token: str) -> str:
        for name, args in calls:
            seen.append(tool(d.sock, token, name, **args))
        return done("Did it.")
    return act


def test_a_send_to_someone_new_asks_and_a_no_sends_nothing(daemon: RunningDaemon) -> None:
    send = FakeCap("probe.send", Kind.SEND, {"probe"})
    daemon.daemon.registry.capabilities[send.name] = send
    seen: list[dict[str, Any]] = []
    install(daemon, [sup("task", "Sending.", task("send the notes to the new contact"))],
            {"worker": _worker(daemon, [("probe.send", {"to": "newperson@elsewhere.example", "text": "the notes"})],
                               seen)})
    owner = Answers(daemon, granted=False, choice=2)                  # No
    try:
        outcome(chat(daemon, "send the notes to the new contact I met"))
    finally:
        owner.stop()
    assert owner.asked and "newperson@elsewhere.example" in _owner_questions(daemon)[-1]["args"]["question"]
    assert send.calls == 0 and seen and seen[0]["is_error"]


def test_an_action_whose_outcome_is_unknown_is_never_repeated_without_the_owner(daemon: RunningDaemon) -> None:
    """The first post loses its acknowledgement (outcome unknown); the worker's identical second post is refused with
    the fact, and nothing ran twice. Its question then waits for the owner; a No repeats nothing."""
    post = FakeCap("probe.post", Kind.POST, {"probe"}, boom=True)
    daemon.daemon.registry.capabilities[post.name] = post
    seen: list[dict[str, Any]] = []
    install(daemon, [sup("task", "Posting.", task("post the update", sites=("kartikey.xyz",)))],
            {"worker": _worker(daemon, [("probe.post", {"site": "kartikey.xyz", "text": "the update"}),
                                        ("probe.post", {"site": "kartikey.xyz", "text": "the update"})], seen)})
    owner = Answers(daemon, granted=True, choice=2)                   # Yes to posting; No to repeating it
    try:
        outcome(chat(daemon, "post the update on kartikey.xyz"))
    finally:
        owner.stop()
    assert post.calls == 1                                            # never ran a second time
    assert len(seen) == 2 and seen[1]["is_error"] and "unknown" in seen[1]["content"]["error"].lower()


class _Checkout(FakeCap):
    """A browser click in a task's tab, reporting the page's amount and merchant and the tab it is in as facts (as
    the browser tools do), never the worker's number."""
    generic_input = True

    class Input(BaseModel):
        amount: str | None = None
        card: bool = False

    def __init__(self) -> None:
        super().__init__("probe.checkout", Kind.CLICK, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.CLICK, Target(site="shop.example"), content=f"click:{args.card}:{self.calls}",
                          facts=ActionFacts(page_url="https://shop.example/checkout", page_amount=args.amount,
                                            page_merchant="Example Shop", tab_chain="t1",
                                            submits_form=True if args.card else None,
                                            field_kinds=("card_number", "card_csc") if args.card else ()))

    async def run(self, args: Any, ctx: Any) -> ToolResult:
        self.calls += 1
        return ToolResult({"ok": True})


def test_one_payment_asks_once_with_the_pages_amount(daemon: RunningDaemon) -> None:
    """Pay, the card choice and the card form of one checkout: one question, naming the amount read on the page."""
    shop = _Checkout()
    daemon.daemon.registry.capabilities[shop.name] = shop
    seen: list[dict[str, Any]] = []
    steps = [("probe.checkout", {"amount": "Pay $12.40", "effect": "pay"}),
             ("probe.checkout", {"amount": "$12.40", "effect": "pay"}),
             ("probe.checkout", {"card": True, "effect": "pay"})]
    install(daemon, [sup("task", "Ordering.", task("order the book on shop.example", sites=("shop.example",)))],
            {"worker": _worker(daemon, steps, seen)})
    owner = Answers(daemon, granted=True, choice=1)                   # Yes
    try:
        outcome(chat(daemon, "order the book on shop.example"))
    finally:
        owner.stop()
    assert [r["is_error"] for r in seen] == [False, False, False] and shop.calls == 3
    asked = _owner_questions(daemon)
    assert len(asked) == 1 and "$12.40" in asked[0]["args"]["question"]
