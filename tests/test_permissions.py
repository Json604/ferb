"""Permission checks (ported from spike 6): known states only, and nothing ever prompts."""

from ferb.device import permissions as P

KNOWN = {s.value for s in P.Status}


def test_all_checks_return_known_states() -> None:
    result = P.check_all()
    assert set(result) == {"accessibility", "screen_recording", "input_monitoring", "full_disk_access"}
    assert set(result.values()) <= KNOWN


def test_automation_without_asking_for_an_app_that_isnt_running() -> None:
    assert P.automation("com.ferb.no-such-app", ask=False) is P.Status.TARGET_NOT_RUNNING


def test_every_permission_has_a_settings_pane() -> None:
    assert set(P.PANES) >= {"accessibility", "screen_recording", "input_monitoring", "full_disk_access", "automation"}
