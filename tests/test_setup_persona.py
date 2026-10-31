"""Decision 16 (owner 2026-10-31): FERB works out who the owner is from their Mac and shows it once, as "here's what I
saw", with no yes/no (decision 15). Code gathers the facts (installed apps, their use, browsers and profiles), a model
interprets them, and they are stored as observed memory, never as the owner's words; a refresh re-interprets only when
the facts changed. The old setup question's Save / Edit / Skip flow is gone."""
import asyncio
import json
from datetime import timedelta
from types import SimpleNamespace

from ferb.config import Config
from ferb.daemon.persona import PersonaReview
from ferb.device.persona import PersonaState, facts_fingerprint, mac_facts, with_activity
from tests.conftest import FakeDistillRuntime

FACTS = mac_facts([{'bundle_id': 'com.microsoft.VSCode', 'app': 'Code', 'uses': 5162, 'last_used': '2026-10-30'},
                   {'bundle_id': 'com.brave.Browser', 'app': 'Brave Browser', 'uses': 10652,
                    'last_used': '2026-10-31'},
                   {'bundle_id': 'org.blender', 'app': 'Blender', 'uses': 0, 'last_used': 'unknown'}],
                  '2026-10-31T10:00:00+05:30', ['brave', 'google chrome'],
                  [{'browser': 'Brave', 'name': 'Personal', 'email': 'owner@example.com', 'last_used': True}])
ANSWER = {"summary": "You seem to spend most of your time coding in VS Code, and Brave is your main browser.",
          "facts": [{"kind": "fact", "key": "main_activity", "content": "The owner mostly codes on this Mac."},
                    {"kind": "env", "key": "main_browser",
                     "content": "The owner's main browser is Brave, profile Personal (owner@example.com)."}]}


def notices(daemon):
    async def read():
        return daemon.daemon.journal.entries_of('chat.notice')
    return daemon.run(read())


def harness(tmp_path, collector):
    config = Config(tmp_path / 'home')
    built, events = [], []

    async def build(facts):
        built.append(facts)
        return {'summary': 'You code a lot.', 'shown': "Here's what I saw on your Mac: You code a lot.",
                'facts': [{'id': 'm1', 'text': 'The owner codes.'}]}
    review = PersonaReview(config, collector, build=build, record=lambda kind, data: events.append((kind, data)))
    return review, config, built, events


def test_gathered_facts_keep_the_most_used_apps_and_owner_focus_only_as_a_fallback():
    rows = [{'bundle_id': 'code', 'app': 'Code', 'uses': None, 'last_used': 'unknown'},
            {'bundle_id': 'blender', 'app': 'Blender', 'uses': 0, 'last_used': 'unknown'}]
    evidence = with_activity(rows, [{'bundle_id': 'code', 'app': 'Code', 'focus_events': 12,
                                    'last_used': '2026-10-11'}])
    facts = mac_facts(evidence, '2026-10-11', [], [])
    assert facts['apps'] == [{'app': 'Code', 'bundle_id': 'code', 'focus_events': 12, 'last_used': '2026-10-11'}]
    assert (facts['installed_apps'], facts['apps_with_use']) == (2, 1)
    assert [a['app'] for a in FACTS['apps']] == ['Brave Browser', 'Code']          # most used first, unused left out


def test_a_refresh_is_worth_interpreting_only_when_the_apps_browsers_or_profiles_changed():
    busier = json.loads(json.dumps(FACTS))
    busier['apps'][0]['uses'] += 400                                            # counts move every day
    assert facts_fingerprint(busier) == facts_fingerprint(FACTS)
    other = json.loads(json.dumps(FACTS))
    other['profiles'][0]['last_used'] = False
    assert facts_fingerprint(other) != facts_fingerprint(FACTS)


def test_what_ferb_saw_is_built_once_shown_once_in_chat_and_stored_as_observed(daemon):
    from ferb.cli.client import Client
    calls = []

    def collect(home, device, activity, excluded):
        calls.append(activity)
        return FACTS
    daemon.daemon.persona_scan = collect
    rt = FakeDistillRuntime(ANSWER)
    daemon.daemon._make_runtime = lambda *a, **k: rt
    client = Client(daemon.sock)
    try:
        client.call('chat.attach')

        async def wait_for_scan():
            await daemon.daemon.persona.task
        daemon.run(wait_for_scan())
        status = client.call('setup.persona', {'status': True})
        assert status['state'] == 'seen' and status['summary'].startswith("Here's what I saw on your Mac: ")
        assert 'Brave' in status['summary'] and len(status['facts']) == 2
        assert not client.call('chat.status', {'peek': True})['approvals']            # no yes/no (decision 15)
        items = client.call('memory.list')['items']
        assert sorted(i['key'] for i in items) == ['main_activity', 'main_browser']
        assert {i['origin'] for i in items} == {'observed'} and not any(i['in_profile'] for i in items)
        shown = [e for e in notices(daemon) if "Here's what I saw" in e.data['text']]
        assert len(shown) == 1
        client.call('chat.attach')                                                   # never built or shown twice
        assert len(calls) == 1
        assert len([e for e in notices(daemon) if "Here's what I saw" in e.data['text']]) == 1
        assert rt.contexts and 'Brave Browser' in str(rt.contexts[0])
    finally:
        client.close()


def test_the_setup_step_shows_it_so_chat_does_not(daemon):
    from ferb.cli.client import Client
    daemon.daemon.persona_scan = lambda *args: FACTS
    daemon.daemon._make_runtime = lambda *a, **k: FakeDistillRuntime(ANSWER)
    client = Client(daemon.sock)
    try:
        assert client.call('setup.persona', {})['state'] == 'scanning'

        async def wait_for_scan():
            await daemon.daemon.persona.task
        daemon.run(wait_for_scan())
        assert client.call('setup.persona', {'status': True})['state'] == 'seen'
        assert not [e for e in notices(daemon) if "Here's what I saw" in e.data['text']]
    finally:
        client.close()


def test_mac_facts_join_the_core_profile_only_once_they_hold_up(daemon):
    daemon.daemon.persona_scan = lambda *args: FACTS
    daemon.daemon._make_runtime = lambda *a, **k: FakeDistillRuntime(ANSWER)

    async def go():
        daemon.daemon._prepare_persona(shown_by='setup')
        await daemon.daemon.persona.task
        items = daemon.daemon.memory.items('active')
        fresh = [daemon.daemon.profile.holds_up(it) for it in items]
        daemon.daemon.clock.advance(timedelta(days=8).total_seconds())
        later = [daemon.daemon.profile.holds_up(it) for it in daemon.daemon.memory.items('active')]
        return fresh, later
    fresh, later = daemon.run(go())
    assert fresh == [False, False] and later == [True, True]


def test_a_refresh_replaces_what_changed_quietly(daemon):
    other = json.loads(json.dumps(FACTS))
    other['browsers'] = ['arc']
    facts = [FACTS]
    daemon.daemon.persona_scan = lambda *args: facts[-1]
    answers = [ANSWER, {"summary": "You moved to Arc.", "facts": [
        {"kind": "env", "key": "main_browser", "content": "The owner's main browser is Arc."}]}]
    daemon.daemon._make_runtime = lambda *a, **k: FakeDistillRuntime(answers.pop(0))

    async def go():
        daemon.daemon._prepare_persona(shown_by='setup')
        await daemon.daemon.persona.task
        unchanged = await daemon.daemon.persona.refresh()
        facts.append(other)
        changed = await daemon.daemon.persona.refresh()
        return unchanged, changed, [(i.key, i.content) for i in daemon.daemon.memory.items('active')]
    unchanged, changed, items = daemon.run(go())
    assert (unchanged, changed) == (False, True)
    assert items == [('main_browser', "The owner's main browser is Arc.")]   # main_activity no longer given: ended


def test_scan_errors_are_redacted_and_can_be_retried(tmp_path):
    async def scenario():
        secret = 'PRIVATE_SENTINEL'

        def collect():
            raise RuntimeError('scan failed ' + secret)
        review, config, built, events = harness(tmp_path, collect)
        review.redact = lambda value: value.replace(secret, '[redacted]') if isinstance(value, str) else value
        review.start()
        await review.task
        assert review.status()['state'] == 'failed' and secret not in str(review.status()) + str(events)
        review.collect = lambda: FACTS
        review.start()
        await review.task
        assert review.status()['state'] == 'seen' and len(built) == 1
        assert review.start()['state'] == 'seen' and len(built) == 1               # once
        persisted = (config.home / 'persona.json').read_text()
        assert secret not in persisted and PersonaState(config).seen()
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
        review, config, built, events = harness(tmp_path, collect)
        review.start()
        await spawned.wait()
        await asyncio.wait_for(review.close(), 2)
        assert processes[0].returncode is not None and processes[0].returncode < 0
        assert review.status() == {'state': 'closed'}
        assert review.start(refresh=True) == {'state': 'closed'}
        assert not built and not PersonaState(config).seen() and not events
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
