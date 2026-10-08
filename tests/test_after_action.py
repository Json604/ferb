"""After every acting tool, the result says what changed (V2); a procedure's expect line is compared with it."""
import asyncio

from ferb.act import aftercheck
from ferb.core.types import ToolResult
from tests.conftest import GatewayCap, RunningDaemon, gateway_request as request, gateway_setup as setup, gateway_tool_call as call


def test_after_block_is_uniform() -> None:
    facts = {"method": "ax", "delivered": True, "observed_change": True, "outcome_verified": False}
    assert aftercheck.after_of(facts) == {"read": "ax", "changed": True, "value": None, "note": ""}


def test_value_read_back() -> None:
    facts = {"method": "ax_value", "accepted": True, "value_matches": True, "value": "done"}
    assert aftercheck.after_of(facts) == {"read": "value", "changed": True, "value": "done", "note": ""}


def test_no_read_back() -> None:
    facts = {"method": "ax", "accepted": True, "outcome_verified": False}
    assert aftercheck.after_of(facts) == {"read": "none", "changed": None, "value": None, "note": ""}


def test_browser_upload_bytes_match_is_a_read_back() -> None:
    assert aftercheck.after_of({"bytes_match": True})["changed"] is True
    assert aftercheck.after_of({"bytes_match": False})["changed"] is False
    assert aftercheck.after_of({"bytes_match": None})["read"] == "none"
    assert aftercheck.after_of({"ok": True, "detail": "clicked"}) == \
        {"read": "none", "changed": None, "value": None, "note": ""}


def test_expect_comparison() -> None:
    assert aftercheck.compare({"value_contains": "Order placed"},
                              {"read": "ax", "changed": True, "value": "Order placed #123", "note": ""})["met"] is True
    assert aftercheck.compare({"value_contains": "Order placed"},
                              {"read": "ax", "changed": True, "value": "Add to cart", "note": ""})["met"] is False
    assert aftercheck.compare({"changed": True}, {"read": "none", "changed": None, "value": None, "note": ""}
                              )["met"] is None


class _MacCap(GatewayCap):
    async def run(self, args, ctx):  # type: ignore[no-untyped-def]
        return ToolResult({"ok": True, "facts": {"method": "ax", "observed_change": True}})


class _BrowserCap(GatewayCap):
    async def run(self, args, ctx):  # type: ignore[no-untyped-def]
        return ToolResult({"ok": True, "detail": "clicked"})


def test_gateway_attaches_after_for_consequential_kinds_only(daemon: RunningDaemon) -> None:
    mac = _MacCap("fake.mac", "send", {"browser:act"})
    web = _BrowserCap("fake.web", "send", {"browser:act"})
    look = GatewayCap("fake.look", "read", {"browser:read"})
    grant, _ = setup(daemon, {"browser:act", "browser:read"}, request("send it to alice@x.com"), [mac, web, look])
    r = call(daemon, grant, "fake.mac")
    assert r["content"]["after"] == {"read": "ax", "changed": True, "value": None, "note": ""}
    r = call(daemon, grant, "fake.web", text="again")
    assert r["content"]["after"]["read"] == "none" and r["content"]["after"]["changed"] is None
    assert "after" not in call(daemon, grant, "fake.look", to="")["content"]


def test_after_condition_kinds_record_a_comparison_fact() -> None:
    from ferb.core.types import Identity, ToolContext
    from ferb.orchestrator.schemas import Check, Condition
    from ferb.orchestrator.verify import Checks
    log = aftercheck.AfterLog()
    checks = Checks(None, lambda _: [], afters=log)   # type: ignore[arg-type]
    cond = Condition(text="order placed", check=Check(kind="after_value", path=None, text="Order placed",
                                                       url_contains=None, site=None, expected=None))
    assert not checks.code_checkable(cond, "t1")      # nothing read after any action yet
    log.note("t1", {"read": "ax", "changed": True, "value": "Add to cart", "note": ""})
    assert checks.code_checkable(cond, "t1")
    ctx = ToolContext("t1", "verifier", "r", Identity.OWNER, frozenset(), "dev", None, "/tmp", (), ())
    r = asyncio.run(checks.run(0, cond, "t1", ctx))
    assert r.met is False and r.path == "code"
    chg = Condition(text="changed", check=Check(kind="after_change", path=None, text=None, url_contains=None,
                                                 site=None, expected="true"))
    assert asyncio.run(checks.run(1, chg, "t1", ctx)).met is True
