from io import StringIO


def test_owner_scroll_check_keeps_cleanup_last(tmp_path):
    from ferb.device.watch import run
    out = StringIO()
    assert run(tmp_path, out, parts=('mac',), only='M14', dry_run=True) == 0
    text = out.getvalue()
    assert text.index('M2 ') < text.index('M14 ') < text.index('M13 ')
    assert 'window-targeted' in text


def test_watch_never_claims_an_existing_document_as_disposable(monkeypatch):
    from types import SimpleNamespace
    import pytest
    from ferb.device import watch
    fake = SimpleNamespace(app_window=lambda pid: 'existing', cg_windows=lambda pid: [{'id': 1}],
                           press_menu=lambda *a: False)
    monkeypatch.setattr(watch, '_ax', lambda: fake)
    monkeypatch.setattr(watch, '_attr', lambda el, key: 'Existing document' if key == 'AXTitle' else None)
    monkeypatch.setattr(watch.time, 'sleep', lambda s: None)
    test = watch.MacTest()
    test.pid = 42
    with pytest.raises(RuntimeError, match='new document'):
        test.new_document()
    assert not test.created and test.win is None


def test_interrupted_owner_verification_remains_pending(monkeypatch):
    from types import SimpleNamespace
    import pytest
    from ferb.device import watch
    monkeypatch.setattr(watch.time, 'sleep', lambda s: None)
    monkeypatch.setattr(watch, '_mouse', lambda: (0, 0))
    monkeypatch.setattr(watch, '_front', lambda: 'owner')
    def interrupted(prompt):
        raise KeyboardInterrupt
    detector = SimpleNamespace(since=lambda t: {'notifications': [], 'frontmost_changes': []})
    session = watch.Session(StringIO(), interrupted, detector, 'owner')
    with pytest.raises(KeyboardInterrupt):
        session.do(watch.Step('M14', 'one wheel tick', lambda: {'landed': True}))
    assert session.results[0]['owner_saw'] == 'pending'
    assert session.results[0]['owner_action_worked'] == 'u'
