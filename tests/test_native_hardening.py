"""Regression evidence for explicit native mechanisms, stable targets and uncapped file delivery."""

import asyncio
from pathlib import Path
from types import SimpleNamespace

import pytest

from ferb.act.fs.files import FsList, FsRead, FsWrite, owner_path
from ferb.act.mac import accessibility as axm
from ferb.act.mac import applescript, screenshot
from ferb.act.shell.tools import ShellSend, ShellStart
from ferb.core.types import ComputerAction, Identity, ToolContext
from tests.mackit import Ax, background_mac, ids


def context(folder):
    return ToolContext('t', 'w', 'r', Identity.OWNER, frozenset({'fs:read', 'fs:write'}),
                       'dev', None, str(folder), (), ())


def test_missing_literal_path_never_substitutes_a_finder_colon_name(tmp_path):
    (tmp_path / 'a:b').mkdir()
    assert owner_path(str(tmp_path / 'a/b'), str(tmp_path)) == tmp_path / 'a/b'
    assert owner_path('Desktop/test.txt', str(tmp_path)) == tmp_path / 'Desktop/test.txt'


def test_file_listing_pages_every_candidate_with_identity_evidence(tmp_path):
    for i in range(503):
        (tmp_path / f'file-{i:04}.txt').write_text(str(i))
    tool = FsList()
    async def read_all():
        entries, offset = [], 0
        while True:
            result = await tool.run(tool.Input(path=str(tmp_path), offset=offset), context(tmp_path))
            entries.extend(result.content['entries'])
            offset = result.content['next_offset']
            if offset is None:
                break
        return entries
    entries = asyncio.run(read_all())
    assert len(entries) == 503
    assert all(e.get('path') and e.get('modified_at') and e.get('source') == 'local' for e in entries)


def test_utf8_file_pages_reconstruct_exact_text(tmp_path):
    text = 'abc💡défg ' * 30
    (tmp_path / 'text.txt').write_text(text)
    tool = FsRead()
    async def read_all():
        parts, offset = [], 0
        while True:
            result = await tool.run(tool.Input(path='text.txt', offset=offset, max_bytes=5), context(tmp_path))
            parts.append(result.content['text'])
            offset = result.content['next_offset']
            if offset is None:
                break
        return ''.join(parts)
    assert asyncio.run(read_all()) == text


def test_artifact_arguments_have_no_old_content_caps(tmp_path):
    content = 'x' * (5 * 1024 * 1024 + 1)
    tool = FsWrite()
    result = asyncio.run(tool.run(tool.Input(path='large.txt', content=content), context(tmp_path)))
    assert not result.is_error and (tmp_path / 'large.txt').read_text() == content
    assert ShellStart.Input(command='x' * 4001).command == 'x' * 4001
    assert ShellSend.Input(id='p', text='y' * 4001).text == 'y' * 4001
    assert 'z' * 4001 in applescript.wrap('example.app', 'z' * 4001)


def test_atomic_write_never_overwrites_an_unrelated_predictable_temp(tmp_path):
    marker = tmp_path / '.notes.txt.ferb-tmp'
    marker.write_text('unrelated file')
    tool = FsWrite()
    asyncio.run(tool.run(tool.Input(path='notes.txt', content='new notes'), context(tmp_path)))
    assert marker.read_text() == 'unrelated file'


def test_failed_press_does_not_choose_ancestor_or_send_return(monkeypatch):
    ax = Ax(press_ok=False)
    d, apps, posted = background_mac(monkeypatch, ax)
    element = ids(d)['Send']
    result = asyncio.run(d.act('WhatsApp', ComputerAction('press', element)))
    assert not result.ok
    assert 'press ancestor' not in ax.calls and posted == [] and apps.activated == []


def test_unobserved_typing_is_not_replayed_or_changed_to_whole_value(monkeypatch):
    ax = Ax(takes_background=False)
    writes = []
    ax.set_value = lambda element, text: writes.append(text) or True
    d, apps, posted = background_mac(monkeypatch, ax)
    element = ids(d)['Message']
    result = asyncio.run(d.act('WhatsApp', ComputerAction('type', element, 'once')))
    assert 'observ' in result.detail.lower() or 'confirm' in result.detail.lower()
    assert len(posted) == 1 and not writes
    assert 'ax keys once' not in ax.calls and apps.activated == []


def test_popup_prefix_is_not_an_identity(monkeypatch):
    ax = Ax()
    ax.popup_menu = lambda el, pid: 'menu'
    ax.menu_items = lambda menu: [('a', 'Account A'), ('b', 'Account B')]
    ax.cancel = lambda el: True
    selected = []
    ax.press = lambda el: selected.append(el) or True
    d, _, _ = background_mac(monkeypatch, ax)
    element = ids(d)['Message']
    result = asyncio.run(d.act('WhatsApp', ComputerAction('select', element, 'Account')))
    assert not result.ok and 'a' not in selected and 'b' not in selected
    assert result.facts['candidates'] == [{'label':'Account A'}, {'label':'Account B'}]


def test_window_title_ambiguity_never_chooses_first(monkeypatch):
    monkeypatch.setattr(axm, 'ax_windows', lambda pid: ['a', 'b'])
    monkeypatch.setattr(axm, 'window_title', lambda win: 'Same title')
    assert axm.window_named(123, 'Same title') is None
    assert axm.window_named(123, 'Same') is None


def test_exact_window_capture_does_not_fall_back_to_other_window(monkeypatch):
    monkeypatch.setattr(screenshot, '_info_windows', lambda everywhere=False: [{
        'kCGWindowNumber': 77, 'kCGWindowOwnerPID': 12, 'kCGWindowLayer': 0,
        'kCGWindowBounds': {'X': 0, 'Y': 0, 'Width': 800, 'Height': 600}, 'kCGWindowName': 'Other'}])
    assert screenshot.front_window(12, (100, 100, 800, 600)) is None


def test_doctor_never_posts_a_global_mouse_event(monkeypatch):
    from ferb.device import selftest
    posted = []
    monkeypatch.setattr(selftest.Quartz, 'CGEventPost', lambda *args: posted.append(args))
    monkeypatch.setattr(selftest, '_Tap', lambda: SimpleNamespace(tap=True, marked=1, close=lambda: None))
    monkeypatch.setattr(selftest, 'ax_read', lambda: selftest.Result('ax_read', True, 'read'))
    selftest.run_all()
    assert posted == []


def test_voice_cannot_answer_the_first_of_two_pending_questions():
    from ferb.voice.controller import Voice
    answers, said = [], []
    voice = Voice(lambda text: None,
                  lambda: [SimpleNamespace(id='q1', kind='owner', question='Which file?'),
                           SimpleNamespace(id='q2', kind='owner', question='Which recipient?')],
                  lambda *args: answers.append(args), SimpleNamespace(say=said.append),
                  SimpleNamespace(append=lambda *args: None), lambda: None)
    result = asyncio.run(voice.transcript('the second one'))
    assert not answers and result['handled'] == 'question_selection_required'
    assert [q['id'] for q in result['questions']] == ['q1', 'q2']


def test_explicit_foreground_has_no_restore_and_stops_on_owner_input(monkeypatch):
    from ferb.act.mac.input import Yielded, Step
    ax = Ax()
    d, apps, posted = background_mac(monkeypatch, ax)
    async def work():
        await d.act('WhatsApp', ComputerAction('key', text='tab', mechanism='foreground', allow_foreground=True))
        await asyncio.sleep(0)
    asyncio.run(work())
    assert apps.activated == ['net.whatsapp.WhatsApp']
    assert not hasattr(d, 'restore_owner') and not hasattr(d, '_restore_at')
    d.input.owner_active = lambda: True
    with pytest.raises(Yielded):
        asyncio.run(d.input.run([Step('key', lambda: posted.append('extra'))], 'w', 'key'))
    assert 'extra' not in posted


def test_native_ui_words_never_decide_action_meaning():
    from ferb.act.mac.tools import MacAct, MacMenu, MacClick
    from ferb.policy.rules import Kind
    from types import SimpleNamespace
    driver = SimpleNamespace(node=lambda *args: SimpleNamespace(label='Delete permanently', role='button'))
    act = MacAct(driver)
    assert act._kind(act.Input(app='Some app', action='press', element='s1e1')) == Kind.MAC_INPUT
    assert act._kind(act.Input(app='Some app', action='key', text='return')) == Kind.MAC_INPUT
    menu = MacMenu(driver)
    menu._bundle = lambda args: 'some.app'
    assert menu.describe(menu.Input(app='Some app', path='File > Send')).kind == Kind.MAC_INPUT
    click = MacClick(driver)
    click._bundle = lambda args: 'some.app'
    spec = click.describe(click.Input(app='Some app', shot='s1', x=1, y=2, purpose='send'))
    assert spec.kind == Kind.MAC_INPUT and spec.declared == Kind.SEND     # the purpose is declared beside the kind (W2)


def test_large_native_snapshot_has_retrievable_pages(monkeypatch):
    from tests.test_mac_snapshot import El, fake_ax
    from ferb.act.mac import accessibility as ax
    fake_ax(monkeypatch)
    monkeypatch.setattr(ax, 'MAX_ELEMENTS', 3)
    win = El('AXWindow', children=[El('AXStaticText', {'AXValue': f'text{i}'}) for i in range(7)])
    first = ax.AxTree()
    ax._walk(win, first, [], 0, None, [0])
    assert first.next_offset == 3
    second = ax.AxTree(offset=first.next_offset)
    ax._walk(win, second, [], 0, None, [0])
    assert second.nodes[0].value == 'text2' and second.next_offset == 6


def test_profile_load_errors_are_facts(tmp_path):
    from ferb.act.mac.profile import Profiles
    (tmp_path / 'some.app.json').write_text('{broken')
    out = Profiles(tmp_path).as_dict('some.app')
    assert out['diagnostics'][0]['operation'] == 'load'


def test_native_element_refs_are_bound_to_observed_process(monkeypatch):
    from ferb.core.errors import StaleRef
    from ferb.act.mac.apps import RunningApp
    from tests.mackit import mac
    d, ax, apps, posted = mac(monkeypatch)
    _, out = asyncio.run(d.snapshot('Notes'))
    ref = next(el['id'] for el in out['elements'] if el['label'] == 'Save')
    apps.apps = [RunningApp(a.bundle_id, a.name, a.pid + 100 if a.name == 'Notes' else a.pid, a.active) for a in apps.apps]
    with pytest.raises(StaleRef, match='process'):
        asyncio.run(d.act('Notes', ComputerAction('press', ref)))
    assert posted == [] and 'ax.press' not in ax.calls


def test_ax_graph_cycles_never_duplicate_candidates(monkeypatch):
    from tests.test_mac_snapshot import El, fake_ax
    from ferb.act.mac import accessibility as ax
    fake_ax(monkeypatch)
    win = El('AXWindow')
    button = El('AXButton', {'AXTitle':'7'}, acts=['AXPress'])
    win.children.extend([win, button, button])
    tree = ax.AxTree()
    ax._walk(win, tree, [], 0, None, [0])
    assert len([n for n in tree.nodes if n.label == '7']) == 1
    assert not tree.truncated_by and tree.repeated_nodes == 2


def test_multiple_builtin_microphones_never_select_first():
    from ferb.voice.mic import InputDevice, choose
    devices=[InputDevice('a','A','bltn'),InputDevice('b','B','bltn')]
    assert choose(devices) is None


def test_applescript_does_not_raise_a_permission_dialog(monkeypatch,tmp_path):
    from ferb.act.mac.tools import MacAppleScript
    from ferb.device import permissions
    from tests.mackit import mac
    d,*_=mac(monkeypatch)
    monkeypatch.setattr(permissions,'automation',lambda *args,**kwargs: permissions.Status.NOT_DETERMINED)
    async def run(*args): pytest.fail('would trigger Automation prompt')
    monkeypatch.setattr(applescript,'run',run)
    tool=MacAppleScript(d)
    result=asyncio.run(tool.run(tool.Input(app='Notes',script='get name'),context(tmp_path)))
    assert result.is_error and result.content['code']=='needs_permission'


def test_deep_subtree_continuation_survives_three_snapshot_generations(monkeypatch):
    from tests.mackit import mac
    from ferb.act.mac.accessibility import AxTree
    d,ax,*_=mac(monkeypatch)
    root=ax.tree.nodes[0]
    ax.read_tree=lambda pid,browser=False,window=None,offset=0: AxTree([root],next_offset=offset+1 if offset<2 else None)
    async def read_pages():
        _,initial=await d.snapshot('Notes')
        _,page=await d.snapshot('Notes',subtree=initial['elements'][0]['id'])
        offsets=[page['offset']]
        while page['next_offset'] is not None:
            _,page=await d.snapshot('Notes',**page['continuation'])
            offsets.append(page['offset'])
        return offsets
    assert asyncio.run(read_pages())==[0,1,2]


@pytest.mark.parametrize('kind',['set_value','type','select'])
def test_native_text_actions_require_explicit_text(monkeypatch,kind):
    from ferb.core.errors import Refused
    ax=Ax(box='Keep this value')
    d,apps,posted=background_mac(monkeypatch,ax)
    ref=ids(d)['Message']
    ax.calls.clear()
    with pytest.raises(Refused,match='requires explicit text'):
        asyncio.run(d.act('WhatsApp',ComputerAction(kind,ref)))
    assert ax.box=='Keep this value' and ax.calls==[] and posted==[] and apps.activated==[]


def test_explicit_empty_value_remains_a_deliberate_clear(monkeypatch):
    ax=Ax(box='Clear this value')
    ax.set_value=lambda el,text: setattr(ax,'box',text) or True
    d,apps,posted=background_mac(monkeypatch,ax)
    ref=ids(d)['Message']
    result=asyncio.run(d.act('WhatsApp',ComputerAction('set_value',ref,'')))
    assert result.ok and ax.box=='' and not posted and not apps.activated
