"""Conditions describe an observable outcome; ledger_done is gone (V6, V7); an unconfirmed one says how to check (V4)."""
import asyncio
from types import SimpleNamespace

from ferb.core.types import Identity, ToolContext
from ferb.orchestrator.schemas import CHECK_KINDS, CHECKER_SCHEMA, Check, CheckerOutcome, Condition
from ferb.orchestrator.verify import Checks


def test_ledger_done_is_not_a_check_kind() -> None:
    assert "ledger_done" not in CHECK_KINDS
    assert "ledger_done" not in str(Check.model_json_schema())


def test_an_old_ledger_done_condition_is_treated_as_an_observation_not_proof(tmp_path) -> None:
    checks = Checks(urls_of=lambda t: [])
    # A condition that slips through with kind ledger_done (from an older plan) is never code-checked true.
    cond = Condition.model_construct(text="the message was sent", layer="outcome", check=SimpleNamespace(
        kind="ledger_done", path=None, text=None, url_contains=None, site=None, expected=None, tool=None))
    ctx = ToolContext("t1", "verifier", "r", Identity.OWNER, frozenset(), "dev", None, str(tmp_path), (), ())
    assert checks.code_checkable(cond, "t1") is False
    r = asyncio.run(checks.run(0, cond, "t1", ctx))
    assert r.met is not True


def test_the_verifier_may_say_how_the_owner_can_check_a_condition() -> None:
    assert "how" in CHECKER_SCHEMA["properties"]["results"]["items"]["properties"]
    out = CheckerOutcome.model_validate({"results": [
        {"index": 0, "met": None, "evidence": "can't see Sent", "how": "Open Gmail and look in Sent for it."}],
        "next": "accept_unconfirmed"})
    assert out.results[0].how == "Open Gmail and look in Sent for it."


def _old_spec_json() -> str:
    from ferb.orchestrator.schemas import TaskSpec
    spec = TaskSpec.model_validate({
        "goal": "like it", "identity": "owner", "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
        "success_conditions": [{"text": "liked", "check": {"kind": "none", "path": None, "text": None,
                                "url_contains": None, "site": None, "expected": "Spotify like"}}]})
    d = spec.model_dump()
    d["success_conditions"][0]["check"]["kind"] = "ledger_done"      # a spec saved before V6
    import json
    return json.dumps(d)


def test_a_saved_spec_with_ledger_done_still_loads_as_an_unchecked_condition() -> None:
    from ferb.orchestrator.schemas import TaskSpec
    spec = TaskSpec.model_validate_json(_old_spec_json())
    check = spec.success_conditions[0].check
    assert check.kind == "none" and check.expected == "Spotify like"
    assert "ledger_done" not in CHECK_KINDS


def test_continuity_load_of_a_task_saved_with_ledger_done_does_not_raise(tmp_path) -> None:
    from ferb.orchestrator.continuity import Continuity
    from ferb.store.db import Database
    db = Database(tmp_path / "t.db")
    db.migrate()
    cont = Continuity(db, lambda x: x, lambda t: set())
    conversation = cont.attach(new=True)
    db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, "
                    "outcome_json, created_at, updated_at) VALUES ('t1','t1','r1','g','owner','root','[]','done',"
                    "'{\"summary\": \"ok\"}','x','x')")
    db.conn.execute("INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words, prior_task, "
                    "request_payload) VALUES ('t1', ?, ?, 'like it', NULL, '')", (conversation, _old_spec_json()))
    loaded = cont.load("t1", conversation)
    assert loaded is not None and loaded[0].success_conditions[0].check.kind == "none"


def test_the_checks_hold_no_ledger() -> None:
    """Ledger rows never decide a condition (V6): the literal checks don't even take one."""
    import inspect
    assert "ledger" not in inspect.signature(Checks).parameters
