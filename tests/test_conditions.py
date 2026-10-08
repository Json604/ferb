"""Conditions describe an observable outcome; ledger_done is gone (V6, V7); an unconfirmed one says how to check (V4)."""
import asyncio
from types import SimpleNamespace

from ferb.core.types import Identity, ToolContext
from ferb.orchestrator.schemas import CHECK_KINDS, VERIFIER_SCHEMA, Check, Condition, VerifierOutcome
from ferb.orchestrator.supervisor import Outcome, _report
from ferb.orchestrator.verify import UNCHECKED, Checks, ConditionResult


def test_ledger_done_is_not_a_check_kind() -> None:
    assert "ledger_done" not in CHECK_KINDS
    assert "ledger_done" not in str(Check.model_json_schema())


def test_an_old_ledger_done_condition_is_treated_as_an_observation_not_proof(tmp_path) -> None:
    checks = Checks(ledger=None, urls_of=lambda t: [])    # type: ignore[arg-type]
    # A condition that slips through with kind ledger_done (from an older plan) is never code-checked true.
    cond = Condition.model_construct(text="the message was sent", layer="outcome", check=SimpleNamespace(
        kind="ledger_done", path=None, text=None, url_contains=None, site=None, expected=None, tool=None))
    ctx = ToolContext("t1", "verifier", "r", Identity.OWNER, frozenset(), "dev", None, str(tmp_path), (), ())
    assert checks.code_checkable(cond, "t1") is False
    r = asyncio.run(checks.run(0, cond, "t1", ctx))
    assert r.met is not True


def test_the_verifier_may_say_how_the_owner_can_check_a_condition() -> None:
    assert "how" in VERIFIER_SCHEMA["properties"]["results"]["items"]["properties"]
    out = VerifierOutcome.model_validate({"results": [
        {"index": 0, "met": None, "evidence": "can't see Sent", "how": "Open Gmail and look in Sent for it."}]})
    assert out.results[0].how == "Open Gmail and look in Sent for it."


def test_an_unconfirmed_condition_shows_how_to_check_it() -> None:
    tree = SimpleNamespace(stop_reason="", verification=[1], owner_question=None)
    results = [ConditionResult(0, "the email was sent", None, "verifier", "Sent wasn't readable",
                               how="Open Gmail and look in Sent."),
               ConditionResult(1, "the file exists", False, "verifier", "missing", how="ignored when false"),
               ConditionResult(2, "the chat has it", None, UNCHECKED, "x", how="ignored when unchecked")]
    text = _report(tree, results, Outcome("t", "done", "s", [], []), "Done, but not confirmed:", "")  # type: ignore[arg-type]
    assert "- the email was sent — unverified: Sent wasn't readable\n  To check: Open Gmail and look in Sent." in text
    assert text.count("To check:") == 1
