"""P28 existing installs discover locally, then wait for explicit owner review."""
import asyncio
from pathlib import Path
from types import SimpleNamespace

from ferb.clock.clock import FakeClock
from ferb.config import Config
from ferb.daemon.questions import Approvals
from ferb.device.persona import persona_proposal
from ferb.daemon.persona import PersonaReview


def harness(tmp_path, collector):
    config = Config(tmp_path / 'home')
    clock = FakeClock()
    approvals = Approvals()
    saved, events = [], []
    review = PersonaReview(config, approvals, clock, collector,
        remember=lambda text: saved.append(text),
        publish=lambda topic, data: events.append((topic, data)),
        record=lambda kind, data: events.append((kind, data)))
    return review, config, approvals, saved, events, clock


def test_pending_scan_is_reused_across_reconnect_expiry_and_restart(tmp_path):
    async def scenario():
        calls = []
        def collect():
            calls.append(True)
            return persona_proposal([{'app': 'Code', 'bundle_id': 'code', 'uses': 3733,
                                     'last_used': '2026-10-10', 'source': 'spotlight'}], '2026-10-11')
        review, config, approvals, saved, events, clock = harness(tmp_path, collect)
        assert review.start()['state'] == 'scanning'
        assert review.start()['state'] == 'scanning'
        await review.task
        pending = review.status()
        ident = pending['approval']['approval_id']
        assert len(calls) == 1 and not saved and not config.get('setup', 'persona_reviewed')
        assert '3733' in pending['approval']['evidence'][1]
        assert review.start()['approval']['approval_id'] == ident
        clock.advance(601)
        assert review.status()['approval']['approval_id'] == ident      # never expires, never asked again (L3)
        again, _, approvals_again, saved_again, events_again, _ = harness(
            tmp_path, lambda: (_ for _ in ()).throw(AssertionError('rescan')))
        restarted = again.start()                     # a restart asks about the stored scan once
        assert restarted['state'] == 'pending'
        assert [ap.id for ap in approvals_again.pending()] == [restarted['approval']['approval_id']]
        assert again.start()['approval'] == again.status()['approval'] == restarted['approval']
        assert [topic for topic, _ in events_again] == ['approval.requested']
        assert not saved_again
    asyncio.run(scenario())


def test_confirm_edit_and_skip_are_exact_owner_actions(tmp_path):
    async def scenario():
        for answer, expected in [('yes', 'Apps you use: Code (7 recorded uses)'),
                                 ('I code and make music', 'I code and make music'), ('skip', None)]:
            root = tmp_path / answer.replace(' ', '-')
            review, config, approvals, saved, events, _ = harness(root, lambda: persona_proposal(
                [{'app': 'Code', 'uses': 7, 'last_used': 'unknown'}], '2026-10-11'))
            review.start(); await review.task
            ap = approvals.resolve(review.status()['approval']['approval_id'], True, text=answer)
            review.resolve(ap)
            assert saved == ([] if expected is None else [expected])
            assert config.get('setup', 'persona_reviewed') is True
            assert review.start()['state'] == 'reviewed'
    asyncio.run(scenario())


def test_decline_does_not_store_profile_and_scan_errors_are_retryable(tmp_path):
    async def scenario():
        attempts = []
        def collect():
            attempts.append(True)
            if len(attempts) == 1: raise OSError('Spotlight unavailable')
            return persona_proposal([], '2026-10-11')
        review, config, approvals, saved, events, _ = harness(tmp_path, collect)
        review.start(); await review.task
        assert review.status()['state'] == 'failed'
        assert not config.get('setup', 'persona_reviewed')
        review.start(); await review.task
        ap = approvals.resolve(review.status()['approval']['approval_id'], False)
        review.resolve(ap)
        assert not saved and config.get('setup', 'persona_reviewed')
    asyncio.run(scenario())


def test_owner_focus_is_fallback_not_agent_focus_or_false_usage_count():
    from ferb.device.persona import with_activity
    rows = [{'bundle_id': 'code', 'app': 'Code', 'uses': None, 'last_used': 'unknown'},
            {'bundle_id': 'blender', 'app': 'Blender', 'uses': 0, 'last_used': 'unknown'}]
    evidence = with_activity(rows, [{'bundle_id': 'code', 'app': 'Code', 'focus_events': 12,
                                    'last_used': '2026-10-11'}])
    result = persona_proposal(evidence, '2026-10-11')
    assert 'Code (12 recorded focus events)' in result['summary']
    assert '12 recorded uses' not in result['summary'] and 'Blender' not in result['summary']
    assert len(result['evidence']) == 3  # timestamp plus complete two-app evidence


def test_chat_attach_exposes_one_review_and_confirmation_enters_identity(daemon):
    from ferb.cli.client import Client
    captured = []
    def collect(home, device, activity, excluded):
        captured.append(activity)
        return persona_proposal([{'bundle_id': 'code', 'app': 'Code', 'uses': 7}], '2026-10-11')
    daemon.daemon.persona_scan = collect
    client = Client(daemon.sock)
    client.call('chat.attach')
    async def wait_for_scan():
        await daemon.daemon.persona.task
    daemon.run(wait_for_scan())
    pending = client.call('chat.status', {'peek': True})['approvals']
    assert len(pending) == 1 and pending[0]['capability'] == 'setup.persona'
    assert pending[0]['evidence'] and pending[0]['task_id'] is None
    assert not client.call('memory.list')['items']
    client.call('chat.attach')
    assert len(captured) == 1
    client.call('chat.answer', {'approval_id': pending[0]['approval_id'], 'text': 'I code and make music'})
    assert client.call('setup.persona', {'status': True})['state'] == 'reviewed'
    async def identity():
        return daemon.daemon._learning_identity()
    assert 'I code and make music' in daemon.run(identity())
    assert not client.call('chat.status', {'peek': True})['approvals']
    client.close()


def test_service_redacts_scan_errors_and_owner_edits_before_persistence(tmp_path):
    async def scenario():
        secret = 'PRIVATE_SENTINEL'
        def collect():
            raise RuntimeError('scan failed ' + secret)
        review, config, approvals, saved, events, _ = harness(tmp_path, collect)
        review.redact = lambda value: value.replace(secret, '[redacted]') if isinstance(value, str) else value
        review.start(); await review.task
        assert secret not in str(review.status()) and secret not in str(events)
        review.collect = lambda: persona_proposal([{'app': 'Code', 'uses': 1}], '2026-10-11')
        review.start(); await review.task
        ap = approvals.resolve(review.status()['approval']['approval_id'], True, text='My profile ' + secret)
        review.resolve(ap)
        assert saved == ['My profile [redacted]']
        assert secret not in config.path.read_text()
    asyncio.run(scenario())


def test_option_label_answer_confirms_profile_instead_of_storing_button_text(tmp_path):
    async def scenario():
        review, config, approvals, saved, _, _ = harness(tmp_path, lambda: persona_proposal(
            [{'app': 'Code', 'uses': 7}], '2026-10-11'))
        review.start(); await review.task
        ap = approvals.resolve(review.status()['approval']['approval_id'], True, text='Save app-use profile')
        review.resolve(ap)
        assert saved == ['Apps you use: Code (7 recorded uses)']
    asyncio.run(scenario())


def test_batched_spotlight_maps_by_file_identity_not_response_order(tmp_path):
    import plistlib
    from ferb.device.persona import app_usage
    first, second, missing = [tmp_path / name for name in ('Code.app', 'Brave.app', 'Missing.app')]
    first.mkdir(); second.mkdir(); missing.mkdir()
    alias = tmp_path / 'Data-Code.app'; alias.symlink_to(first)
    apps = [SimpleNamespace(path=str(path), bundle_id=path.stem, name=path.stem)
            for path in (first, second, missing)]
    calls = []
    def run(args, **kwargs):
        calls.append(args)
        return SimpleNamespace(stdout=plistlib.dumps([
            {'kMDItemPath': str(second), 'kMDItemUseCount': 10628},
            {'kMDItemPath': str(alias), 'kMDItemUseCount': 5137}]), returncode=0)
    rows = {row['app']: row for row in app_usage(apps, run)}
    assert len(calls) == 1 and calls[0][-3:] == [str(first), str(second), str(missing)]
    assert rows['Code']['uses'] == 5137 and rows['Brave']['uses'] == 10628
    assert rows['Missing']['uses'] is None


def test_shutdown_cancels_and_reaps_owned_blocked_metadata_process(tmp_path):
    import sys
    from ferb.device.persona import app_usage_async
    async def scenario():
        spawned = asyncio.Event()
        processes = []
        async def spawn(*args, **kwargs):
            assert args[0] == '/usr/bin/mdls'
            process = await asyncio.create_subprocess_exec(sys.executable, '-c', 'import time; time.sleep(3600)', **kwargs)
            processes.append(process); spawned.set()
            return process
        async def collect():
            return await app_usage_async([SimpleNamespace(path='/controlled/Code.app')], spawn=spawn)
        review, config, _, saved, events, _ = harness(tmp_path, collect)
        review.start()
        await spawned.wait()
        await asyncio.wait_for(review.close(), 2)
        assert processes[0].returncode is not None and processes[0].returncode < 0
        assert review.status() == {'state': 'closed'}
        assert review.start(refresh=True) == {'state': 'closed'}
        assert not saved and not config.get('setup', 'persona_reviewed') and not events
    asyncio.run(scenario())


def test_cancellation_while_spawn_returns_still_reaps_owned_child(tmp_path):
    import sys
    from ferb.device.persona import app_usage_async
    async def scenario():
        spawned, release = asyncio.Event(), asyncio.Event()
        processes = []
        async def spawn(*args, **kwargs):
            process = await asyncio.create_subprocess_exec(sys.executable, '-c', 'import time; time.sleep(3600)', **kwargs)
            processes.append(process); spawned.set()
            await release.wait()
            return process
        task = asyncio.create_task(app_usage_async([SimpleNamespace(path='/controlled/Code.app')], spawn=spawn))
        await spawned.wait(); task.cancel(); release.set()
        try:
            await task
        except asyncio.CancelledError:
            pass
        assert processes[0].returncode is not None and processes[0].returncode < 0
    asyncio.run(scenario())


def test_shutdown_rejects_discovery_before_service_exists(daemon):
    from ferb.cli.client import Client
    calls = []
    daemon.daemon.persona_scan = lambda *args: calls.append(args)
    daemon.daemon._shutting_down = True
    client = Client(daemon.sock)
    assert client.call('setup.persona', {'refresh': True}) == {'state': 'closed'}
    client.call('chat.attach')
    assert not calls and daemon.daemon.persona is None
    client.close()
