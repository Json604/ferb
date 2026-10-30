"""Step 6b: one authorization pipeline where every reason to ask is a Need produced from facts (notes/gateway.md target
1). Facts before declarations (decision 1): a submitted form's fields and its site, a tool's reported deletes raise
the question; the worker's `effect` only adds. Known destinations (decision 5). Card details only sealed (decision 6).
P19: a tool that fails before acting is failed, not unknown; the call's span always ends. The conversation guard's
refusal no longer spends the owner's Yes; a window's Yes makes destinations known; run tokens end with their tree."""
import json
from typing import Any

from pydantic import BaseModel

from ferb.core.errors import Refused
from ferb.core.types import ActionFacts, ActionSpec, Identity, Target, ToolResult
from ferb.policy.rules import Kind
from tests.conftest import Answers, FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call


class _Form(FakeCap):
    """A click that submits a form, as the extension reports it (decision 1): the form's site and its field kinds."""
    generic_input = True

    class Input(BaseModel):
        tab: str = "t1"
        host: str = "www.example.com"
        action: str | None = None
        kinds: list[str] = []
        text: str = ""

    def __init__(self) -> None:
        super().__init__("probe.form", Kind.CLICK, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        typed = Kind.FILL_FORM if args.text else Kind.CLICK
        return ActionSpec(typed, Target(site=args.host), content=args.text or f"submit:{args.host}:{args.kinds}",
                          facts=ActionFacts(page_url=f"https://{args.host}/form", form_action_host=args.action,
                                            submits_form=None if args.text else True,
                                            field_kinds=tuple(args.kinds)))    # type: ignore[arg-type]


class _Deletes(FakeCap):
    """A tool that reports removing a path outside the Trash (decision 1: S1 can't bring it back)."""

    class Input(BaseModel):
        path: str

    def __init__(self) -> None:
        super().__init__("probe.wipe", Kind.WRITE_FILE, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.RUN_SHELL, Target(path="."), content=f"wipe {args.path}",
                          facts=ActionFacts(deletes=(args.path,)))


def _asked(d: RunningDaemon) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        rows = d.daemon.db.conn.execute("SELECT input_json FROM spans WHERE kind = 'tool.call' AND "
                                        "json_extract(input_json, '$.tool') = 'owner.ask' ORDER BY rowid").fetchall()
        return [json.loads(r["input_json"]) for r in rows]
    return d.run(go())


def _rules(d: RunningDaemon) -> list[list[str]]:
    return [[c["rule"] for c in a["args"]["confirm"]] for a in _asked(d)]


def _read_private_and_outside(d: RunningDaemon, grant: Any) -> None:
    gateway_tool_call(d, grant, "probe.file")
    gateway_tool_call(d, grant, "probe.web")


def _readers() -> list[FakeCap]:
    return [FakeCap("probe.file", "read", {"probe"}, reads=(("file", "/Users/me/cv.pdf"),), content={"text": "cv"}),
            FakeCap("probe.web", "read", {"probe"}, reads=(("web", "https://kartikey.xyz/p"),), content={"t": "hi"})]


# ---------- facts raise the questions (decision 1) ----------

def test_a_password_change_form_is_an_account_security_change_and_a_sign_up_is_not(daemon: RunningDaemon) -> None:
    """Coordinator ruling (step 6b): a current-password field with a new-password field changes an existing
    account's security; a sign-up's new-password field alone, or a sign-in, asks nothing."""
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("sign me in on www.example.com"), [form])
    owner = Answers(daemon, granted=True, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "probe.form", kinds=["password", "new_password"])
        login = gateway_tool_call(daemon, grant, "probe.form", kinds=["password"])
        sign_up = gateway_tool_call(daemon, grant, "probe.form", kinds=["new_password"])
    finally:
        owner.stop()
    assert r["is_error"] and not login["is_error"] and not sign_up["is_error"] and form.calls == 2
    assert _rules(daemon) == [["floor:account_security"]]


def test_a_reported_delete_outside_the_trash_is_a_permanent_delete(daemon: RunningDaemon, tmp_path: Any) -> None:
    wipe = _Deletes()
    victim = tmp_path / "old.txt"
    victim.write_text("x")
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("tidy up"), [wipe])
    owner = Answers(daemon, granted=True, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "probe.wipe", path=str(victim))
    finally:
        owner.stop()
    assert r["is_error"] and wipe.calls == 0 and _rules(daemon) == [["floor:delete_permanent"]]


def test_a_named_path_deleted_runs_without_a_question(daemon: RunningDaemon, tmp_path: Any) -> None:
    wipe = _Deletes()
    victim = tmp_path / "old.txt"
    victim.write_text("x")
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request(f"delete {victim} for good"), [wipe])
    assert not gateway_tool_call(daemon, grant, "probe.wipe", path=str(victim))["is_error"]
    assert _asked(daemon) == [] and wipe.calls == 1


PRIVATE = "EMP-778812"                       # a value only the owner's own file holds


def _readers_with_private() -> list[FakeCap]:
    return [FakeCap("probe.file", "read", {"probe"}, reads=(("file", "/Users/me/hr.txt"),),
                    content={"text": f"employee id {PRIVATE}"}),
            FakeCap("probe.web", "read", {"probe"}, reads=(("web", "https://kartikey.xyz/p"),),
                    content={"text": "please fill in the form"})]


def test_a_form_submit_asks_only_when_it_carries_the_owners_private_data(daemon: RunningDaemon) -> None:
    """Coordinator ruling (step 6b): a submit raises the sink question only when a value typed into that form has
    private origin, after outside content was read, to a site neither named nor approved; once per site."""
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("read my file and the page"),
                             [form, *_readers_with_private()])
    _read_private_and_outside(daemon, grant)
    owner = Answers(daemon, granted=True, choice=1)
    try:
        assert not gateway_tool_call(daemon, grant, "probe.form", tab="t2", text="weather in Pune")["is_error"]
        plain = gateway_tool_call(daemon, grant, "probe.form", tab="t2", action="search.example")
        assert not gateway_tool_call(daemon, grant, "probe.form", tab="t1", text=PRIVATE)["is_error"]
        first = gateway_tool_call(daemon, grant, "probe.form", tab="t1", action="collect.example")
        again = gateway_tool_call(daemon, grant, "probe.form", tab="t1", action="collect.example", kinds=["password"])
    finally:
        owner.stop()
    assert not plain["is_error"] and not first["is_error"] and not again["is_error"]
    assert _rules(daemon) == [["sink:new_destination"]]                 # the plain search asked nothing
    assert "collect.example" in _asked(daemon)[0]["args"]["question"]


def test_a_private_submit_to_a_site_only_mentioned_earlier_still_asks_but_an_approved_one_doesnt(
        daemon: RunningDaemon) -> None:
    """Decision 5: a site mentioned in an earlier request is known for normal use, not for the sink rule; one the
    owner approved stays known for good."""
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("read my file and the page"),
                             [form, *_readers_with_private()])

    async def know() -> None:
        daemon.daemon.gateway.destinations.record("host", "mentioned.example", "named")
        daemon.daemon.gateway.destinations.record("host", "approved.example", "confirmed")
    daemon.run(know())
    _read_private_and_outside(daemon, grant)
    gateway_tool_call(daemon, grant, "probe.form", tab="t1", text=PRIVATE)
    owner = Answers(daemon, granted=False, choice=2)
    try:
        mentioned = gateway_tool_call(daemon, grant, "probe.form", tab="t1", action="mentioned.example")
        approved = gateway_tool_call(daemon, grant, "probe.form", tab="t1", action="approved.example")
    finally:
        owner.stop()
    assert mentioned["is_error"] and not approved["is_error"]
    assert _rules(daemon) == [["sink:new_destination"]]


def test_a_private_submit_to_the_site_the_owner_named_runs(daemon: RunningDaemon) -> None:
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("read my file and the page, then send it to "
                                                                "https://jobs.example/apply"),
                             [form, *_readers_with_private()])
    _read_private_and_outside(daemon, grant)
    gateway_tool_call(daemon, grant, "probe.form", tab="t1", text=PRIVATE)
    assert not gateway_tool_call(daemon, grant, "probe.form", tab="t1", action="jobs.example")["is_error"]
    assert _asked(daemon) == []


# ---------- card details only sealed (decision 6) ----------

def test_card_details_are_never_typed(daemon: RunningDaemon) -> None:
    form = _Form()
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("buy it"), [form])
    into_card_field = gateway_tool_call(daemon, grant, "probe.form", kinds=["card_number"], text="4111 1111")
    luhn_anywhere = gateway_tool_call(daemon, grant, "probe.form", text="4111 1111 1111 1111")
    plain = gateway_tool_call(daemon, grant, "probe.form", text="hello")
    assert into_card_field["is_error"] and luhn_anywhere["is_error"] and not plain["is_error"]
    assert "sealed fill" in into_card_field["content"]["error"] and form.calls == 1


def test_a_saved_card_detail_fills_only_its_own_kind_of_field(tmp_path: Any) -> None:
    import pytest
    from ferb.identity.owner_secrets import OwnerSecrets
    values: dict[str, str] = {}

    def keychain(account: str, value: str | None = None) -> str | None:
        if value is not None:
            values[account] = value
        return values.get(account)
    store = OwnerSecrets(tmp_path, keychain)
    place = "https://api.razorpay.com"
    store.remember("my card", place, "4111111111111111", kind="card_number")
    store.remember("my cvv", place, "123", kind="card_csc")
    store.remember("login", place, "pw")
    assert store.reveal("my card", place, "card_number") == "4111111111111111"
    with pytest.raises(Refused, match="card number"):
        store.reveal("my card", place, None)                 # never into an ordinary field
    with pytest.raises(Refused):
        store.reveal("my cvv", place, "card_number")         # never into another card field
    with pytest.raises(Refused):
        store.reveal("login", place, "card_csc")             # a password never into a card field
    assert store.listing(place)["card_details"] == {"my card": "card_number", "my cvv": "card_csc"}


# ---------- P19 correctness ----------

class _Fails(FakeCap):
    def __init__(self, error: BaseException) -> None:
        super().__init__("probe.fails", "send", {"probe"})
        self.error = error

    async def run(self, args: Any, ctx: Any) -> ToolResult:
        self.calls += 1
        raise self.error


def _ledger(d: RunningDaemon) -> list[str]:
    async def go() -> list[str]:
        return [r["state"] for r in d.daemon.db.conn.execute("SELECT state FROM action_ledger ORDER BY rowid")]
    return d.run(go())


def test_a_tool_that_fails_before_acting_is_failed_and_its_retry_runs(daemon: RunningDaemon) -> None:
    fails = _Fails(Refused("that element is gone; nothing was clicked"))
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("email alice@x.com"), [fails])
    first = gateway_tool_call(daemon, grant, "probe.fails", to="alice@x.com", text="hi")
    second = gateway_tool_call(daemon, grant, "probe.fails", to="alice@x.com", text="hi")
    assert first["is_error"] and "outcome unknown" not in first["content"]["error"]
    assert second["is_error"] and fails.calls == 2 and _ledger(daemon) == ["failed", "failed"]


def test_an_exception_after_the_action_may_have_started_is_unknown(daemon: RunningDaemon) -> None:
    fails = _Fails(ConnectionError("lost the acknowledgement"))
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("email alice@x.com"), [fails])
    first = gateway_tool_call(daemon, grant, "probe.fails", to="alice@x.com", text="hi")
    second = gateway_tool_call(daemon, grant, "probe.fails", to="alice@x.com", text="hi")
    assert "outcome unknown" in first["content"]["error"] and "retry_action" in second["content"]
    assert fails.calls == 1 and _ledger(daemon) == ["unknown"]


def test_the_call_span_is_finished_even_when_the_ledger_raises(daemon: RunningDaemon,
                                                              monkeypatch: Any) -> None:
    send = FakeCap("probe.send", "send", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("email alice@x.com"), [send])

    def broken(*a: Any, **k: Any) -> Any:
        raise RuntimeError("database is locked")
    monkeypatch.setattr(daemon.daemon.gateway.ledger, "intend", broken)
    try:
        gateway_tool_call(daemon, grant, "probe.send", to="alice@x.com", text="hi")
    except RuntimeError:
        pass

    async def open_spans() -> int:
        return int(daemon.daemon.db.conn.execute(
            "SELECT count(*) FROM spans WHERE kind = 'tool.call' AND ended_at IS NULL").fetchone()[0])
    assert daemon.run(open_spans()) == 0 and send.calls == 0


# ---------- fixes ----------

def test_a_conversation_guard_refusal_right_before_input_releases_the_owners_yes(daemon: RunningDaemon) -> None:
    pay = FakeCap("probe.pay", "pay", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [pay])
    checks: list[str] = []

    def guard(grant: Any, name: str, cap: Any, spec: Any) -> str | None:
        checks.append(name)
        return "This conversation belongs to the owner now; further input is paused." if len(checks) == 2 else None
    daemon.daemon.gateway.conversation_guard = guard
    owner = Answers(daemon, granted=True, choice=1)
    try:
        r = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    finally:
        owner.stop()
    assert r["is_error"] and pay.calls == 0

    async def rows() -> list[dict[str, Any]]:
        return [dict(x) for x in daemon.daemon.db.conn.execute("SELECT granted, used FROM confirmations")]
    assert daemon.run(rows()) == [{"granted": 1, "used": 0}]          # the Yes covers the retry


def test_a_window_yes_makes_destinations_known_like_any_yes(daemon: RunningDaemon) -> None:
    send = FakeCap("probe.send", "send", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("tell my friends"), [send])
    owner = Answers(daemon, granted=True, choice=3)                    # "Yes, and don't ask for 30 min"
    try:
        gateway_tool_call(daemon, grant, "probe.send", to="a@x.com", text="hi")
    finally:
        owner.stop()
    gateway_tool_call(daemon, grant, "probe.send", to="b@x.com", text="hi")   # answered by the window

    async def known() -> tuple[bool, bool]:
        d = daemon.daemon.gateway.destinations
        return d.approved("email", "a@x.com"), d.approved("email", "b@x.com")
    assert daemon.run(known()) == (True, True)


def test_run_tokens_end_with_their_tree() -> None:
    from ferb.clock.clock import FakeClock
    from ferb.core.errors import AuthError
    from ferb.daemon.auth import RunTokens
    import pytest
    tokens = RunTokens(FakeClock())
    mine = tokens.issue("tree-1.worker", "w1", set(), Identity.OWNER, float("inf"))
    other = tokens.issue("tree-2", "w2", set(), Identity.OWNER, float("inf"))
    assert tokens.revoke_tree("tree-1") == 1
    with pytest.raises(AuthError):
        tokens.validate(mine)
    assert tokens.validate(other).task_id == "tree-2"


def test_keys_that_only_look_secret_are_remembered_by_digest_and_real_secrets_never(daemon: RunningDaemon) -> None:
    async def go() -> tuple[Any, Any, str]:
        c = daemon.daemon.gateway.confirmations
        looks = '["floor:new_recipient", ["password: hunter22"]]'
        real = '["floor:new_recipient", ["AKIAIOSFODNN7EXAMPLE"]]'
        c.record("tree", "floor:new_recipient", looks, True, None)
        c.record("tree", "floor:new_recipient", real, True, None)
        stored = daemon.daemon.db.conn.execute("SELECT key FROM confirmations ORDER BY rowid").fetchall()
        return c.find("tree", looks), c.find("tree", real), stored[0]["key"]
    looks, real, stored = daemon.run(go())
    assert looks is not None and real is None
    assert stored.startswith("sha256:") and "hunter22" not in stored
