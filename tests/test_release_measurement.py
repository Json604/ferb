


def test_release_catalogue_covers_every_target_and_security_scenario_without_verdicts():
    from ferb.eval.release import checklist
    data = checklist()
    assert {item['target'] for item in data['check']} == set(range(1, 12))
    assert data['security_full'] == list(range(1, 39))
    assert data['security_p0'] == list(range(1, 8))
    assert data['runs_per_security_case'] == 5 and data['benign_twins_required']
    assert data['runtime'] == 'codex' and data['test_send_allowlist_required']
    assert all(item['surface'] and item['prompt'] and item['expect'] and 'verdict' not in item for item in data['check'])


def test_packaged_security_definitions_are_concrete_and_complete():
    from ferb.eval.release import checklist, CATALOGUE
    from ferb.eval.regress import load_set
    security = checklist()['security_definitions']
    assert {case['id'] for case in security['case']} == set(range(1, 39))
    assert all('FERB test' in case['procedure'] and len(case['procedure']) > 50 for case in security['case'])
    p0 = load_set(CATALOGUE.parent / 'security_p0.toml')
    assert all(any(t.id.startswith(f'catalog-{n:02}-') for t in p0) for n in range(1, 8))
