"""The final checker is read-only, enforced by code, not its prompt (V3, W4). A click, send, write or shell is refused
even if the checker is told to do it; it returns unknown instead."""

from pathlib import Path
from typing import Any

from ferb.daemon.auth import RunGrant
from ferb.daemon.gateway import observes
from ferb.registry import (Registry, add_context_readers, add_evidence, add_identity_browser, add_mac,
                           add_owner_browsers, add_recording, add_shell, add_time, add_trail, build)
from tests.conftest import FakeCap, RunningDaemon, gateway_setup, gateway_tool_call

# Every installed capability a checker may use (W4: snapshots, reads, screenshots, scrolling, opening a URL in FERB's
# own browser). Anything else, including a capability added later and left unmarked, is unavailable to a checker.
OBSERVATION = {
    "actions.search", "chat.history", "memory.read", "task.evidence", "trail.days", "trail.query",
    "fs.list", "fs.read", "recording.inspect", "shell.list", "shell.read", "shell.wait",
    "mac.apps", "mac.screenshot", "mac.snapshot",
    "browser.find", "browser.navigate", "browser.point", "browser.profiles", "browser.read", "browser.screenshot",
    "browser.scroll", "browser.snapshot", "browser.tabs", "browser.wait",
    "ferb_browser.find", "ferb_browser.navigate", "ferb_browser.open", "ferb_browser.otp_candidates",
    "ferb_browser.point", "ferb_browser.read", "ferb_browser.screenshot", "ferb_browser.scroll",
    "ferb_browser.snapshot", "ferb_browser.wait",
}


def _every_capability() -> Registry:
    """The full installed registry, built with stand-in drivers (construction only stores them)."""
    r = build()
    add_recording(r, object())
    add_identity_browser(r, object(), recordings=object())  # type: ignore[arg-type]
    add_owner_browsers(r, object(), recordings=object())    # type: ignore[arg-type]
    add_mac(r, object(), lambda: None, recordings=object())  # type: ignore[arg-type]
    add_shell(r, object())                                  # type: ignore[arg-type]
    add_trail(r, lambda: None)
    add_evidence(r, object(), lambda task_id: None)
    add_context_readers(r, object(), lambda task_id: None, lambda: None)
    add_time(r, lambda: None, lambda: None, lambda: None, lambda task_id: None)
    return r


def _verifier_grant(daemon: RunningDaemon, *caps: Any) -> RunGrant:
    grant, _ = gateway_setup(daemon, {"probe"}, None, list(caps))
    return daemon.run(_observe(daemon, grant))      # type: ignore[no-any-return]


async def _observe(daemon: RunningDaemon, grant: RunGrant) -> RunGrant:
    # Re-issue the same grant as observe-only, the way the verifier's token is issued.
    tokens = daemon.daemon.tokens
    return tokens.validate(tokens.issue(grant.task_id, grant.worker_id, set(grant.scopes), grant.identity, ttl_s=600,
                                        observe_only=True))


def test_observation_tools_are_offered_acting_tools_are_not(daemon: RunningDaemon) -> None:
    read = FakeCap("probe.read", "read", {"probe"})
    read.observation = True                                 # type: ignore[attr-defined]
    send = FakeCap("probe.send", "send", {"probe"})
    grant = _verifier_grant(daemon, read, send)
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert "probe.read" in names and "probe.send" not in names


def test_the_checker_cannot_act_even_when_asked(daemon: RunningDaemon) -> None:
    send = FakeCap("probe.send", "send", {"probe"})        # observation defaults False
    grant = _verifier_grant(daemon, send)
    r = gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    assert r["is_error"] and "read-only" in r["content"]["error"].lower()
    assert "unknown" in r["content"]["error"]               # what the checker reports instead
    assert send.calls == 0                                  # the capability never ran


def test_an_observation_tool_runs_for_the_checker(daemon: RunningDaemon) -> None:
    read = FakeCap("probe.read", "read", {"probe"}, content={"seen": "the sent message"})
    read.observation = True                                 # type: ignore[attr-defined]
    grant = _verifier_grant(daemon, read)
    r = gateway_tool_call(daemon, grant, "probe.read")
    assert not r["is_error"] and r["content"]["seen"] == "the sent message" and read.calls == 1


def test_a_normal_worker_still_acts(daemon: RunningDaemon) -> None:
    send = FakeCap("probe.send", "send", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, None, [send])   # not observe-only
    assert not grant.observe_only
    r = gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    assert not r["is_error"] and send.calls == 1


def test_only_reading_capabilities_are_marked_observation() -> None:
    """The checker's whole tool set, pinned: a change to it is a conscious change to this list. Opening an app
    (mac.open), a tab in the owner's profile (browser.open) or pressing a menu item (mac.menu) is a side effect."""
    caps = _every_capability().capabilities
    marked = {name for name, cap in caps.items() if observes(cap)}
    assert marked == OBSERVATION
    assert not {"mac.open", "browser.open", "mac.menu", "browser.dialog", "owner.ask"} & marked
    assert all(getattr(caps[name], "reversible", True) for name in marked)   # none carries an effect annotation


def test_the_checker_prompt_matches_the_gateway() -> None:
    """The prompt no longer offers acting (typing in a search field, opening an app); it says what to do instead."""
    md = Path("ferb/orchestrator/prompts/verifier.md").read_text()
    assert "observation tools only" in md and "`met=null`" in md and "the step that would gather" in md
    assert "mac_open" not in md and "You may open sources" not in md
