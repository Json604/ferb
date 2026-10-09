"""After every acting tool, the result says what changed (V2); a procedure's expect line is compared with it."""
import asyncio

import pytest

from ferb.act import aftercheck
from ferb.core.types import ToolResult
from tests.conftest import GatewayCap, RunningDaemon, gateway_request as request, gateway_setup as setup, gateway_tool_call as call


def test_after_block_is_uniform() -> None:
    facts = {"method": "ax", "delivered": True, "observed_change": True, "outcome_verified": False}
    assert aftercheck.after_of(facts) == {"read": "ax", "changed": True, "value": None, "note": ""}


def test_unconfirmed_change_is_unknown_never_false() -> None:
    after = aftercheck.after_of({"method": "ax", "delivered": True, "observed_change": False})
    assert after["changed"] is None
    assert aftercheck.compare({"changed": False}, after)["met"] is None


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


class _Scripted(GatewayCap):
    def __init__(self, name: str, kind: str, scopes: set[str], content: dict, error: bool = False) -> None:
        super().__init__(name, kind, scopes)
        self.content, self.error = content, error

    async def run(self, args, ctx):  # type: ignore[no-untyped-def]
        return ToolResult(self.content, is_error=self.error)


def _cond(kind: str, **kw):  # type: ignore[no-untyped-def]
    from ferb.orchestrator.schemas import Check, Condition
    base = dict(path=None, text=None, url_contains=None, site=None, expected=None, tool=None) | kw
    return Condition(text="c", check=Check(kind=kind, **base))


def _check(daemon: RunningDaemon, grant, cond):  # type: ignore[no-untyped-def]
    from ferb.core.types import Identity, ToolContext
    from ferb.orchestrator.verify import Checks
    checks = Checks(lambda _: [], daemon.daemon.spans)
    ctx = ToolContext(grant.task_id, "verifier", "r", Identity.OWNER, frozenset(), "dev", None, "/tmp", (), ())
    assert checks.code_checkable(cond, grant.task_id)
    return daemon.run(checks.run(0, cond, grant.task_id, ctx))


def test_condition_reads_the_recorded_span_of_the_named_tool(daemon: RunningDaemon) -> None:
    first = _Scripted("fake.first", "send", {"browser:act"}, {"ok": True, "facts": {"method": "ax", "observed_change": True}})
    later = _Scripted("fake.later", "send", {"browser:act"}, {"ok": True, "facts": {"method": "ax"}})
    grant, _ = setup(daemon, {"browser:act"}, request("send it to alice@x.com"), [first, later])
    call(daemon, grant, "fake.first")
    call(daemon, grant, "fake.later", text="other")
    assert _check(daemon, grant, _cond("after_change", expected="true", tool="fake.first")).met is True
    assert _check(daemon, grant, _cond("after_change", expected="true")).met is None   # latest acting: read nothing
    assert _check(daemon, grant, _cond("after_change", expected="false", tool="fake.later")).met is None


def test_failed_later_call_of_the_named_tool_is_what_the_check_sees(daemon: RunningDaemon) -> None:
    cap = _Scripted("fake.set", "fill_form", {"browser:act"},
                    {"ok": True, "facts": {"method": "ax_value", "value_matches": True, "accepted": True}})
    grant, _ = setup(daemon, {"browser:act"}, request("fill it for alice@x.com"), [cap])
    call(daemon, grant, "fake.set")
    assert _check(daemon, grant, _cond("after_change", expected="true", tool="fake.set")).met is True
    cap.content = {"ok": False, "facts": {"method": "ax_value", "value_matches": False, "accepted": True}}
    cap.error = True
    r = call(daemon, grant, "fake.set", text="again")
    assert r["is_error"] and r["content"]["after"]["changed"] is False       # errors carry their real facts too
    assert _check(daemon, grant, _cond("after_change", expected="true", tool="fake.set")).met is False


def test_a_value_edit_that_reported_neither_match_nor_acceptance_is_unknown_not_unchanged() -> None:
    # driver._edit's failed ax_value rewrite reports only the method: nothing was read back, so no "no change".
    after = aftercheck.after_of({"method": "ax_value", "outcome_verified": False, "replayed": False})
    assert after["changed"] is None and after["read"] == "none"
    assert aftercheck.compare({"changed": False}, after)["met"] is None
    assert aftercheck.after_of({"method": "ax_value", "value_matches": None})["changed"] is None
    assert aftercheck.after_of({"method": "ax_value", "accepted": False})["changed"] is False   # a definite answer


@pytest.mark.parametrize("content", [
    {"observed_change": True, "error": "browser error: the page went away"},     # browser.act: the span's error
    {"ok": False, "detail": "AX press refused", "facts": {"method": "ax", "observed_change": True}},   # mac.act
])
def test_an_errored_call_that_changed_the_screen_does_not_confirm_the_expected_change(
        daemon: RunningDaemon, content: dict) -> None:
    cap = _Scripted("fake.press", "send", {"browser:act"}, content, error=True)
    grant, _ = setup(daemon, {"browser:act"}, request("send it to alice@x.com"), [cap])
    r = call(daemon, grant, "fake.press")
    assert r["is_error"] and r["content"]["after"]["changed"] is True        # the fact stays as read
    got = _check(daemon, grant, _cond("after_change", expected="true", tool="fake.press"))
    assert got.met is None and "error" in got.evidence                       # something changed, not shown to be it
    assert _check(daemon, grant, _cond("after_change", expected="false", tool="fake.press")).met is False
