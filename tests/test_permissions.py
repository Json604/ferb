"""Permission checks (ported from spike 6): known states only, and nothing ever prompts."""

from ferb.device import permissions as P
import asyncio
import threading

import pytest

KNOWN = {s.value for s in P.Status}


def test_all_checks_return_known_states() -> None:
    result = P.check_all()
    assert set(result) == {"accessibility", "screen_recording", "input_monitoring", "full_disk_access"}
    assert set(result.values()) <= KNOWN


def test_automation_without_asking_for_an_app_that_isnt_running() -> None:
    assert P.automation("com.ferb.no-such-app", ask=False) is P.Status.TARGET_NOT_RUNNING


def test_every_permission_has_a_settings_pane() -> None:
    assert set(P.PANES) >= {"accessibility", "screen_recording", "input_monitoring", "full_disk_access", "automation"}


def test_cancelled_permission_wait_does_not_block_loop_exit_or_duplicate_native_check(monkeypatch):
    entered, release = threading.Event(), threading.Event()
    calls = []

    def blocked(bundle_id, ask=False):
        calls.append((bundle_id, ask, threading.current_thread().daemon))
        entered.set()
        release.wait()
        return P.Status.GRANTED

    monkeypatch.setattr(P, 'automation', blocked)

    async def cancel_wait():
        waiter = asyncio.create_task(P.automation_async('com.spotify.client'))
        while not entered.is_set():
            await asyncio.sleep(0)
        waiter.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiter

    try:
        asyncio.run(cancel_wait())  # must return while the native check is still blocked
        pending = P.automation_check('com.spotify.client')
        assert pending is P.automation_check('com.spotify.client')
        assert not pending.done() and not pending.cancelled()
        assert calls == [('com.spotify.client', False, True)]
        release.set()
        assert pending.result(timeout=2) is P.Status.GRANTED
    finally:
        release.set()


def test_explicit_permission_request_does_not_share_a_passive_check(monkeypatch):
    release = threading.Event()
    def blocked(bundle_id, ask=False):
        release.wait()
        return P.Status.GRANTED if ask else P.Status.NOT_DETERMINED
    monkeypatch.setattr(P, 'automation', blocked)
    try:
        passive = P.automation_check('com.ferb.permission-test')
        requested = P.automation_check('com.ferb.permission-test', ask=True)
        assert passive is not requested
        release.set()
        assert passive.result(timeout=2) is P.Status.NOT_DETERMINED
        assert requested.result(timeout=2) is P.Status.GRANTED
    finally:
        release.set()


def test_cancelled_check_consumes_late_error_and_new_check_can_retry(monkeypatch):
    entered, release = threading.Event(), threading.Event()
    def failed(bundle_id, ask=False):
        entered.set()
        release.wait()
        raise OSError('native check failed')
    monkeypatch.setattr(P, 'automation', failed)
    async def go():
        errors = []
        asyncio.get_running_loop().set_exception_handler(lambda loop, ctx: errors.append(ctx))
        waiter = asyncio.create_task(P.automation_async('com.ferb.error-test'))
        while not entered.is_set():
            await asyncio.sleep(0)
        pending = P.automation_check('com.ferb.error-test')
        waiter.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiter
        release.set()
        while not pending.done():
            await asyncio.sleep(0)
        with pytest.raises(OSError, match='native check failed'):
            pending.result()
        # Let the cancelled waiter's late completion callbacks run.
        for _ in range(4):
            await asyncio.sleep(0)
        assert not errors
        with pytest.raises(OSError, match='native check failed'):
            await P.automation_async('com.ferb.error-test')
    try:
        asyncio.run(go())
    finally:
        release.set()


def test_passive_watcher_skips_pending_permission_without_blocking_or_caching_denial(daemon, monkeypatch):
    from concurrent.futures import Future
    pending = Future()
    calls = []
    def check(bundle_id, ask=False):
        calls.append((bundle_id, ask))
        return pending
    monkeypatch.setattr(P, 'automation_check', check)
    d = daemon.daemon
    assert d._automation_granted('com.apple.Safari') is False
    assert d._automation_granted('com.apple.Safari') is False
    assert calls == [('com.apple.Safari', False)]
    assert 'com.apple.Safari' not in d._automation_cache
    pending.set_result(P.Status.GRANTED)
    assert d._automation_granted('com.apple.Safari') is True
    assert d._automation_granted('com.apple.Safari') is True
    assert calls == [('com.apple.Safari', False)]
