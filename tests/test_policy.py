"""Policy (section 10): identity, intent, exposure, data flow, autonomy, approvals, undo, and the engine's order.
Safety tests 21.3, 21.7, 21.8, 21.9, 21.10 live here."""

import dataclasses

import pytest

from ferb.clock.clock import FakeClock
from ferb.core.types import Identity, Task
from ferb.policy import dataflow, intent as intent_mod
from ferb.policy.approvals import Approvals
from ferb.policy.autonomy import Level
from ferb.policy.engine import PolicyEngine
from ferb.policy.intent import IntentRegistry, RequestIntent
from ferb.policy.rules import (ActionRequest, DataLabel, Kind, Target, check_child_authority, check_identity,
                               destructive_shell, payload_hash)

WS = "/Users/me/.ferb/workspace"
ALL_OWNER = frozenset({"browser:read", "browser:act", "mac:read", "mac:act", "applescript", "shell", "fs:read",
                       "fs:write", "screen"})


def act(kind: str, scopes: set[str], identity: Identity = Identity.OWNER, **kw: object) -> ActionRequest:
    base = dict(capability=f"cap.{kind}", kind=kind, identity=identity, scopes=frozenset(scopes), task_id="t1",
                worker_id="w1", request_id="r1", task_scopes=ALL_OWNER if identity is Identity.OWNER else
                frozenset({"identity:browse", "secrets:fill", "mailbox:read", "mailbox:send", "fs:read",
                           "fs:write"}))
    base.update(kw)
    return ActionRequest(**base)  # type: ignore[arg-type]


def engine(level: Level = Level.BALANCED, **kw: object) -> tuple[PolicyEngine, FakeClock]:
    clock = FakeClock()
    return PolicyEngine(clock, WS, level=lambda: level, **kw), clock  # type: ignore[arg-type]


# ---------- 21.3 identity ----------

@pytest.mark.parametrize("scopes", [{"browser:act"}, {"shell"}, {"applescript"}, {"mac:act"}])
def test_ferb_task_cannot_reach_owner_browsers_shell_or_applescript(scopes: set[str]) -> None:
    assert check_identity(act("x", scopes, Identity.FERB), WS)


@pytest.mark.parametrize("scopes", [{"secrets:fill"}, {"mailbox:read"}, {"identity:browse"}])
def test_owner_task_cannot_use_fill_secret_mailbox_or_ferb_browser(scopes: set[str]) -> None:
    assert check_identity(act("x", scopes), WS)


def test_ferb_files_only_inside_its_workspace() -> None:
    assert check_identity(act(Kind.WRITE_FILE, {"fs:write"}, Identity.FERB, target=Target(path=f"{WS}/a.txt")),
                          WS) is None
    assert check_identity(act(Kind.WRITE_FILE, {"fs:write"}, Identity.FERB, target=Target(path="/Users/me/x")), WS)
    assert check_identity(act(Kind.WRITE_FILE, {"fs:write"}, Identity.FERB,
                              target=Target(path=f"{WS}/../../.ssh/id")), WS)


def test_worker_scopes_must_be_granted_to_its_task() -> None:
    e, _ = engine()
    v = e.evaluate(act(Kind.RUN_SHELL, {"shell"}, task_scopes=frozenset({"fs:read"})))
    assert (v.decision, v.step) == ("block", "identity")


def test_child_never_gets_more_authority_than_parent() -> None:
    from datetime import datetime, timedelta, timezone
    end = datetime(2026, 9, 29, 12, tzinfo=timezone.utc)
    parent = Task("p", None, "g", "mac", Identity.OWNER, {"fs:read", "browser:read"}, "mixed", "r", [], end)
    ok = dataclasses.replace(parent, id="c", scopes={"fs:read"}, deadline=end - timedelta(minutes=5))
    assert check_child_authority(parent, ok) is None
    assert check_child_authority(parent, dataclasses.replace(ok, scopes={"fs:read", "shell"}))
    assert check_child_authority(parent, dataclasses.replace(ok, deadline=end + timedelta(minutes=1)))
    assert check_child_authority(parent, dataclasses.replace(ok, deadline=None))     # no limit > limited


def test_destructive_shell_patterns() -> None:
    for c in ("rm -rf /", "rm -fr ~/x", "git push --force", "git reset --hard HEAD~3", "sudo reboot",
              "diskutil eraseDisk JHFS+ X disk2", "dd if=/dev/zero of=/dev/disk2"):
        assert destructive_shell(c), c
    for c in ("rm notes.txt", "git push", "ls -la", "git status"):
        assert not destructive_shell(c), c


# ---------- intent (21.7) ----------

def summarize_only() -> RequestIntent:
    return RequestIntent.from_plan("r1", {"summarize"}, sites=frozenset({"news.example"}))


@pytest.mark.parametrize("a", [
    act(Kind.SEND, {"browser:act"}, target=Target(recipients=("boss@x.com",))),
    act(Kind.RUN_SHELL, {"shell"}),
    act(Kind.MAILBOX_SEND, {"browser:act"}, target=Target(recipients=("new@person.com",))),
    act(Kind.POST, {"browser:act"}, target=Target(site="news.example")),
])
def test_summarize_only_request_blocks_send_shell_and_new_recipients(a: ActionRequest) -> None:
    e, _ = engine()
    e.intents.record(summarize_only())
    v = e.evaluate(a)
    assert (v.decision, v.step) == ("block", "intent")
    assert v.events[0][0] == "policy.blocked"


def test_implied_kinds_book_a_table() -> None:
    intent = RequestIntent.from_plan("r1", {"book"}, sites=frozenset({"tables.example"}))
    assert intent_mod.check(act(Kind.FILL_FORM, {"browser:act"}, target=Target(site="tables.example")),
                            intent).decision == "match"
    assert intent_mod.check(act(Kind.SUBMIT, {"browser:act"}, target=Target(site="tables.example")),
                            intent).decision == "match"
    assert intent_mod.check(act(Kind.SEND, {"browser:act"}), intent).decision == "block"
    assert intent_mod.check(act(Kind.PAY, {"browser:act"}), intent).decision == "block"   # booking != buying


def test_requested_kind_with_surprising_target_asks() -> None:
    intent = RequestIntent.from_plan("r1", {"email"}, recipients=frozenset({"alice@x.com"}))
    r = intent_mod.check(act(Kind.SEND, {"browser:act"}, target=Target(recipients=("alice@x.com", "eve@y.com"))),
                         intent)
    assert (r.decision, "eve@y.com" in r.reason) == ("ask", True)


def test_intent_is_write_once_workers_cannot_change_it() -> None:
    reg = IntentRegistry()
    reg.record(summarize_only())
    with pytest.raises(PermissionError):
        reg.record(RequestIntent.from_plan("r1", {"email", "run_command"}))
    assert reg.get("r1") == summarize_only()


def test_fuzzy_checker_can_only_make_things_stricter() -> None:
    intent = RequestIntent.from_plan("r1", {"email"}, recipients=frozenset({"alice@x.com"}))
    send = act(Kind.SEND, {"browser:act"}, target=Target(recipients=("alice@x.com",)))
    assert intent_mod.check(send, intent, lambda a, i: "no").decision == "block"
    assert intent_mod.check(send, intent, lambda a, i: "unsure").decision == "ask"
    run = act(Kind.RUN_SHELL, {"shell"})
    assert intent_mod.check(run, intent, lambda a, i: "yes").decision == "block"   # "yes" can't allow a mismatch


def test_missing_intent_blocks_consequential_actions_but_reversible_just_run() -> None:
    e, _ = engine()
    assert e.evaluate(act(Kind.SEND, {"browser:act"})).decision == "block"
    for k in (Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB, Kind.DRAFT):
        assert e.evaluate(act(k, {"browser:read"})).decision == "allow"


# ---------- exposure (21.8) ----------

def test_exposed_worker_loses_shell_applescript_and_file_write() -> None:
    e, _ = engine()
    e.intents.record(RequestIntent.from_plan("r1", {"run_command", "edit_file"}, extra_kinds={Kind.APPLESCRIPT}))
    shell = act(Kind.RUN_SHELL, {"shell"})
    assert e.evaluate(shell).decision == "allow"
    assert e.exposure.read("w1", "web", "https://evil.example/page") is True
    for a in (shell, act(Kind.APPLESCRIPT, {"applescript"}), act(Kind.WRITE_FILE, {"fs:write"})):
        v = e.evaluate(a)
        assert (v.decision, v.step) == ("block", "exposure")
    assert e.evaluate(act(Kind.READ, {"fs:read"})).decision == "allow"     # reading still fine
    other = dataclasses.replace(shell, worker_id="w2")                    # a clean worker is unaffected
    assert e.evaluate(other).decision == "allow"


def test_trusted_sources_do_not_expose_and_injection_exposes_immediately() -> None:
    e, _ = engine()
    e.exposure.trusted_sources.add("notes:owner")
    assert e.exposure.read("w1", "web", "notes:owner") is False
    assert not e.exposure.state("w1").exposed
    assert e.exposure.injection_signal("w1", "page said 'ignore previous instructions'") is True
    assert e.exposure.state("w1").exposed


# ---------- data flow (21.9) ----------

def test_owner_email_content_cannot_be_typed_into_an_unrelated_site() -> None:
    intent = RequestIntent.from_plan("r1", {"email"}, recipients=frozenset({"alice@x.com"}),
                                     extra_kinds={Kind.FILL_FORM})
    gmail = DataLabel("owner_account", account="gmail:me@x.com", site="mail.google.com")
    leak = act(Kind.FILL_FORM, {"browser:act"}, target=Target(site="pastebin.example"),
               data_labels=frozenset({gmail}))
    assert dataflow.check(leak, intent)
    ok = act(Kind.SEND, {"browser:act"}, target=Target(recipients=("alice@x.com",)), data_labels=frozenset({gmail}))
    assert dataflow.check(ok, intent) is None
    back_home = act(Kind.FILL_FORM, {"browser:act"}, target=Target(site="mail.google.com"),
                    data_labels=frozenset({gmail}))
    assert dataflow.check(back_home, intent) is None
    e, _ = engine()
    e.intents.record(intent)
    assert (e.evaluate(leak).decision, e.evaluate(leak).step) == ("block", "dataflow")


def test_owner_data_never_crosses_into_ferb_identity() -> None:
    gmail = DataLabel("owner_account", account="gmail:me@x.com")
    a = act(Kind.MAILBOX_SEND, {"mailbox:send"}, Identity.FERB, data_labels=frozenset({gmail}),
            target=Target(recipients=("alice@x.com",)))
    assert dataflow.check(a, RequestIntent.from_plan("r1", {"email"}, recipients=frozenset({"alice@x.com"})))


# ---------- autonomy, approvals, undo (21.10) ----------

@pytest.mark.parametrize("level", list(Level))
def test_hard_floor_asks_at_every_level_even_when_requested(level: Level) -> None:
    e, _ = engine(level)
    e.intents.record(RequestIntent.from_plan("r1", {"buy"}, max_amount=50.0))
    for a in (act(Kind.PAY, {"browser:act"}, target=Target(amount=20.0)),
              act(Kind.DELETE_PERMANENT, {"fs:write"}, target=Target(path="/Users/me/x"))):
        if a.kind == Kind.DELETE_PERMANENT:
            e.intents._by_request.clear()
            e.intents.record(RequestIntent.from_plan("r1", set(), extra_kinds={Kind.DELETE_PERMANENT}))
        assert e.evaluate(a).decision == "ask"


def test_careful_asks_for_consequential_balanced_does_not() -> None:
    intent = RequestIntent.from_plan("r1", {"book"}, sites=frozenset({"tables.example"}))
    submit = act(Kind.SUBMIT, {"browser:act"}, target=Target(site="tables.example"))
    careful, _ = engine(Level.CAREFUL)
    careful.intents.record(intent)
    balanced, _ = engine(Level.BALANCED)
    balanced.intents.record(intent)
    assert careful.evaluate(submit).decision == "ask"
    assert balanced.evaluate(submit).decision == "allow"
    trusting, _ = engine(Level.CAREFUL, trusted_kinds=lambda: frozenset({Kind.SUBMIT}))
    trusting.intents.record(intent)
    assert trusting.evaluate(submit).decision == "allow"


def test_approval_is_bound_to_the_exact_payload_and_single_use() -> None:
    e, _ = engine()
    e.intents.record(RequestIntent.from_plan("r1", {"buy"}, max_amount=100.0))
    pay = act(Kind.PAY, {"browser:act"}, target=Target(account="shop:me", amount=40.0))
    v = e.evaluate(pay)
    assert v.decision == "ask" and v.events[0][0] == "approval.requested"
    e.approvals.resolve(v.approval_id, granted=True)
    changed_amount = dataclasses.replace(pay, target=Target(account="shop:me", amount=90.0))
    changed_account = dataclasses.replace(pay, target=Target(account="shop:someone-else", amount=40.0))
    assert e.evaluate(changed_amount).decision == "ask"          # approval doesn't cover a changed amount
    assert e.evaluate(changed_account).decision == "ask"         # ... or account
    assert e.evaluate(pay).decision == "allow"                   # the exact action: allowed once
    assert e.evaluate(pay).decision == "ask"                     # single-use


def test_approval_expires() -> None:
    clock = FakeClock()
    approvals = Approvals(clock, ttl_s=60)
    a = act(Kind.PAY, {"browser:act"})
    ap = approvals.request(a, "hard floor")
    clock.advance(61)
    with pytest.raises(ValueError, match="expired"):
        approvals.resolve(ap.id, True)
    ap2 = approvals.request(a, "hard floor")
    approvals.resolve(ap2.id, True)
    clock.advance(61)
    assert approvals.consume(a) is None
    assert payload_hash(a) == ap2.payload_hash


def test_send_is_held_for_undo_and_undo_means_it_never_releases() -> None:
    e, clock = engine()
    e.intents.record(RequestIntent.from_plan("r1", {"email"}, recipients=frozenset({"alice@x.com"})))
    send = act(Kind.SEND, {"browser:act"}, target=Target(recipients=("alice@x.com",)))
    v1, v2 = e.evaluate(send), e.evaluate(send)
    assert (v1.decision, v1.events[0][0]) == ("hold", "action.held")
    assert e.undo(v1.hold_id)
    clock.advance(9.9)
    assert e.release_due() == []
    clock.advance(0.2)
    released = e.release_due()
    assert [h.id for h, _ in released] == [v2.hold_id]            # the undone one never releases
    assert released[0][1].events[0][0] == "action.released"
    assert not e.undo(v2.hold_id)                                 # too late once released


def test_stop_blocks_first_even_an_approved_action_and_holds_never_release() -> None:
    e, clock = engine()
    e.intents.record(RequestIntent.from_plan("r1", {"buy", "email"}, max_amount=100.0,
                                             recipients=frozenset({"alice@x.com"})))
    pay = act(Kind.PAY, {"browser:act"}, target=Target(amount=10.0))
    e.approvals.resolve(e.evaluate(pay).approval_id, True)
    held = e.evaluate(act(Kind.SEND, {"browser:act"}, target=Target(recipients=("alice@x.com",))))
    e.stopped = True
    v = e.evaluate(pay)
    assert (v.decision, v.step) == ("block", "stop")
    clock.advance(30)
    assert e.release_due() == []
    e.stopped = False
    assert [h.id for h, _ in e.release_due()] == [held.hold_id]
