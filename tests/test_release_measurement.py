import json
from pathlib import Path

from ferb.eval.comparison import compare_baselines


def test_baseline_comparison_keeps_denominators_and_missing_values():
    before = {'requests': 10, 'done': 4, 'verified': 3, 'owner_pass': 2, 'owner_verdicts': 3,
              'avg_task_s': 30, 'cache_pct': 50, 'tool_calls': 100}
    after = {'requests': 20, 'done': 12, 'verified': 10, 'owner_pass': 5, 'owner_verdicts': 6,
             'avg_task_s': 20, 'cache_pct': 70, 'tool_calls': 250}
    report = compare_baselines(before, after)
    assert report['metrics']['done_pct'] == {'before': 40.0, 'after': 60.0, 'delta': 20.0}
    assert report['metrics']['avg_task_s']['delta'] == -10
    assert report['metrics']['owner_questions']['delta'] is None
    assert report['denominators']['after']['owner_verdicts'] == 6
    assert report['acceptance'] == 'not inferred from aggregate metrics'


def test_compare_cli_uses_saved_snapshots_without_daemon(tmp_path, monkeypatch, capsys):
    from ferb.cli import main
    a, b = tmp_path / 'a.json', tmp_path / 'b.json'
    a.write_text(json.dumps({'requests': 0, 'done': 0})); b.write_text(json.dumps({'requests': 2, 'done': 1}))
    monkeypatch.setattr('ferb.cli.Client', lambda *a: (_ for _ in ()).throw(AssertionError('no daemon')))
    assert main(['baseline', '--compare', str(a), str(b)]) == 0
    report = json.loads(capsys.readouterr().out)
    assert report['metrics']['done_pct']['before'] is None
    assert report['metrics']['done_pct']['after'] == 50


def test_release_catalogue_covers_every_target_and_security_scenario_without_verdicts():
    from ferb.eval.release import checklist
    data = checklist()
    assert {item['target'] for item in data['check']} == set(range(1, 12))
    assert data['security_full'] == list(range(1, 39))
    assert data['security_p0'] == list(range(1, 8))
    assert data['runs_per_security_case'] == 5 and data['benign_twins_required']
    assert data['runtime'] == 'codex' and data['test_send_allowlist_required']
    assert all(item['surface'] and item['prompt'] and item['expect'] and 'verdict' not in item for item in data['check'])
