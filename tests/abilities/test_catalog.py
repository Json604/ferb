"""Packaged abilities are discoverable before learning; discovery grants no execution authority."""
import asyncio
import json
import subprocess
import sys
from pathlib import Path

import pytest
from pydantic import ValidationError

from ferb.abilities.browser_verification.definition import DEFINITION
from ferb.abilities.catalog import AbilityCatalog
from ferb.abilities.contracts import ProcedureResource
from ferb.abilities.tools import AbilitiesRead
from ferb.cli.client import Client
from ferb.core import origin
from ferb.core.errors import Refused
from ferb.core.types import ToolResult
from ferb.daemon.gateway import result_origins
from ferb.registry import build
from tests.conftest import chat, install, sup, task, tool


def test_fresh_daemon_exposes_metadata_to_planner_and_worker_and_reads_on_demand(daemon):
    assert not (daemon.daemon.home / 'skills').exists()
    results = {}

    def worker(token):
        results['list'] = tool(daemon.sock, token, 'abilities.list')
        client = Client(daemon.sock, role='mcp_shim', token=token)
        try:
            results['read'] = client.call('tools.call', {'name': 'abilities.read',
                                                       'arguments': {'name': 'browser_verification'}})
        finally:
            client.close()
        return json.dumps({'status': 'done', 'summary': 'Read the installed procedure.',
                           'artifacts': [], 'evidence': []})

    script = install(daemon, [sup('task', 'Read the installed browser verification procedure.',
                                task('Read the installed browser verification procedure.'))], {'worker': worker})
    chat(daemon, 'Read the installed browser verification procedure.')
    definition = results['list']['content']['abilities'][0]
    assert definition['id'] == 'browser_verification'
    assert definition['source_kind'] == 'builtin' and definition['origin'] == 'ferb'
    assert definition['availability']['available'] is False
    body = results['read']['content']['body']
    assert 'Saved expertise, fresh perception' in body
    assert 'Origin: ferb' in results['read']['footer']
    for role in ('supervisor', 'worker'):
        request = next(request for _, key, request in script.requests if key == role)
        context = request.system + request.context
        assert DEFINITION.description in context
        assert body not in context
    assert 'abilities.run' not in daemon.daemon.registry.capabilities


def test_registry_search_discovers_read_only_ability_tools():
    registry = build()
    search = registry.capabilities['actions.search']
    result = asyncio.run(search.run(search.Input(query='browser verification'), None))
    assert {'abilities.list', 'abilities.read'} <= {item['name'] for item in result.content['actions']}
    for name in ('abilities.list', 'abilities.read'):
        cap = registry.capabilities[name]
        assert cap.observation and cap.origin == origin.FERB
        assert cap.describe(cap.Input(**({'name': 'browser_verification'} if name.endswith('read') else {}))).kind == 'read'


def test_missing_dependencies_and_unimplemented_workflow_are_explicit():
    catalog = AbilityCatalog(capabilities=lambda: set())
    metadata = catalog.read('browser_verification')
    assert metadata['version'] == DEFINITION.version
    assert metadata['supported_interactions'] == []
    assert metadata['availability']['missing_capabilities'] == list(DEFINITION.required_capabilities)
    assert 'not installed' in metadata['availability']['reason']
    installed = AbilityCatalog(capabilities=lambda: set(DEFINITION.required_capabilities)).catalog()[0]
    assert installed['availability']['missing_capabilities'] == []
    assert installed['availability']['available'] is False
    assert 'workflow' in installed['availability']['reason']


def test_duplicate_builtin_ids_fail_registration():
    with pytest.raises(ValueError, match='registered twice'):
        AbilityCatalog((DEFINITION, DEFINITION))


@pytest.mark.parametrize('name', ['', '.', '..', '../browser_verification', '/tmp/SKILL.md',
                                 'browser_verification/SKILL.md', 'ferb.abilities.browser_verification',
                                 'browser_verification\\SKILL.md', 'browser_verification\x00'])
def test_names_cannot_be_paths_or_python_modules(name):
    with pytest.raises(Refused):
        AbilityCatalog().read(name)


def test_read_arguments_cannot_supply_files_or_import_paths():
    with pytest.raises(ValidationError):
        AbilitiesRead.Input(name='browser_verification', file='/tmp/evil.py')


def test_resource_cannot_be_a_parent_path():
    with pytest.raises(ValueError):
        ProcedureResource(DEFINITION.procedure_resource.package, '../SKILL.md')


@pytest.mark.parametrize('link_directory', [False, True])
def test_packaged_resource_symlinks_cannot_read_outside_package(tmp_path, monkeypatch, link_directory):
    from ferb.abilities import catalog as module
    outside = tmp_path / 'outside'
    outside.mkdir()
    (outside / 'SKILL.md').write_text('outside content')
    package = tmp_path / 'package'
    if link_directory:
        package.symlink_to(outside, target_is_directory=True)
    else:
        package.mkdir()
        (package / 'SKILL.md').symlink_to(outside / 'SKILL.md')
    monkeypatch.setattr(module.resources, 'files', lambda package_module: package)
    with pytest.raises(Refused):
        AbilityCatalog().read('browser_verification')


def test_missing_packaged_procedure_reports_unavailable(tmp_path, monkeypatch):
    from ferb.abilities import catalog as module
    monkeypatch.setattr(module.resources, 'files', lambda package_module: tmp_path)
    metadata = AbilityCatalog().catalog()[0]
    assert metadata['availability']['available'] is False
    assert 'procedure' in metadata['availability']['reason']
    with pytest.raises(Refused):
        AbilityCatalog().read('browser_verification')


def test_builtin_trust_cannot_override_observed_or_inferred_sources():
    cap = build().capabilities['abilities.read']
    returned = ToolResult({'observation': 'The check is still visible.', 'decision': 'Inspect again.'},
                          read_sources=(('web', 'https://kartikey.xyz'), ('origin', origin.INFERRED)))
    assert result_origins(returned.read_sources, cap.origin, 'read') == {
        origin.external('web', 'kartikey.xyz'), origin.INFERRED}
    assert result_origins((), cap.origin, 'read') == {origin.FERB}


def test_built_wheel_reads_same_procedure_without_source_checkout(tmp_path):
    root = Path(__file__).resolve().parents[2]
    subprocess.run(['uv', 'build', '--wheel', '--out-dir', str(tmp_path / 'dist')],
                   cwd=root, check=True, capture_output=True, text=True)
    wheel = next((tmp_path / 'dist').glob('*.whl'))
    code = '''import json, sys
sys.path.insert(0, sys.argv[1])
import ferb
from ferb.abilities.catalog import AbilityCatalog
assert ferb.__file__.startswith(sys.argv[1]), ferb.__file__
catalog = AbilityCatalog()
print(json.dumps([catalog.read(item["id"]) for item in catalog.catalog()]))
'''
    result = subprocess.run([sys.executable, '-I', '-c', code, str(wheel)], cwd=tmp_path,
                            check=True, capture_output=True, text=True)
    catalog = AbilityCatalog()
    assert json.loads(result.stdout) == [catalog.read(item['id']) for item in catalog.catalog()]


@pytest.mark.parametrize('name', [
    'task_planning', 'memory_intake', 'scheduling', 'continuous_work', 'delegated_conversations',
    'background_interfaces', 'sealed_login', 'media_capture', 'delegated_processes', 'artifact_work',
    'source_research',
])
def test_guidance_is_readable_and_hashed_without_claiming_a_runner(name):
    import hashlib
    catalog = AbilityCatalog()
    metadata = next(item for item in catalog.catalog() if item['id'] == name)
    assert metadata['readable'] and not metadata['availability']['available']
    assert 'body' not in metadata and 'sha256' not in metadata
    read = catalog.read(name)
    assert read['sha256'] == hashlib.sha256(read['body'].encode('utf-8')).hexdigest()
    assert read['version'] == '1' and read['origin'] == origin.FERB
    assert read['supported_interactions'] == []


def test_every_ability_including_guidance_refuses_symlink_resources(tmp_path, monkeypatch):
    from ferb.abilities import catalog as module
    catalog = AbilityCatalog()
    names = [item['id'] for item in catalog.catalog()]
    outside = tmp_path / 'outside.md'
    outside.write_text('untrusted file')
    package = tmp_path / 'package'
    package.mkdir()
    for name in names:
        filename = 'SKILL.md' if name == 'browser_verification' else name + '.md'
        (package / filename).symlink_to(outside)
    monkeypatch.setattr(module.resources, 'files', lambda _: package)
    for name in names:
        with pytest.raises(Refused):
            catalog.read(name)


def test_role_system_discovers_guidance_without_loading_irrelevant_bodies(daemon):
    supervisor = daemon.daemon.supervisor
    catalog = AbilityCatalog()
    async def system_for(role):
        return supervisor._system(role)
    for role in ('supervisor.md', 'worker.md', 'verifier.md'):
        system = daemon.run(system_for(role))
        assert 'memory_intake' in system and 'source_research' in system
        assert 'readability does not imply an execution runner' in system
        for name in ('sealed_login', 'media_capture', 'artifact_work'):
            assert catalog.read(name)['body'] not in system
    for role in ('dispatch.md', 'question.md', 'resources.md'):
        system = daemon.run(system_for(role))
        assert 'Installed FERB guidance index' not in system
        assert 'Shared interaction contract' in system


def test_structural_guidance_records_the_exact_version_hash_and_origin(daemon):
    from types import SimpleNamespace
    tree = SimpleNamespace(id='structural-guidance', context_sources={})
    supervisor = daemon.daemon.supervisor
    async def recorded_guidance():
        block = supervisor._guidance_block(tree, 'artifact_work')
        record = daemon.daemon.db.conn.execute(
            "SELECT output_json,origin FROM spans WHERE task_id=? AND kind='context.source'", (tree.id,)).fetchone()
        return block, dict(record)
    block, record = daemon.run(recorded_guidance())
    expected = AbilityCatalog().read('artifact_work')
    assert block.protected and block.origin == origin.FERB
    assert block.source_refs == ('ability:artifact_work',)
    assert json.loads(block.payload) == expected
    assert expected['sha256'] in block.text
    assert record['origin'] == origin.FERB
    assert json.loads(json.loads(record['output_json'])['text']) == expected
