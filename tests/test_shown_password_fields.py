"""A password typed with the site's "Show password" ticked sits in a plain text field (type=text, AXTextField), yet the
field's own metadata still names it a password (Google keeps autocomplete="current-password", name="Passwd" and the
label "Enter your password"). Its value is a secret at every capture boundary (rule 14, D7): DOM snapshots, typed
text, AX value changes and the extension's element list and screenshot masks. A checkbox or button ("Show password")
is no text field: its state stays readable."""
import json
import re
import subprocess
from pathlib import Path
from typing import Any

import pytest

from ferb.core.types import Identity
from ferb.policy.rules import ActionRequest
from ferb.sanitize.redact import REDACTED, Redactor
from tests.chromelab import _node
from tests.browserkit import bd  # noqa: F401
from tests.support.typed_text import Screen, capture, key_event, texts

GOOGLE = {'type': 'text', 'autocomplete': 'current-password', 'name': 'Passwd', 'label': 'Enter your password',
          'value': 'Hunter2!x'}


# A. the redaction boundary --------------------------------------------------------------------------------------------

def test_the_google_shown_password_value_is_redacted_in_a_dom_snapshot() -> None:
    r = Redactor()
    assert r(GOOGLE)['value'] == REDACTED
    assert 'Hunter2!x' not in json.dumps(r({'elements': [{'role': 'textbox', 'label': 'Email', 'value': 'a@b.c'},
                                                         GOOGLE]}))


@pytest.mark.parametrize('meta', [{'autocomplete': 'current-password'}, {'autocomplete': 'new-password'},
                                  {'name': 'Passwd'}, {'id': 'login_pwd'}, {'label': 'Enter your password'},
                                  {'placeholder': 'Password'}, {'title': 'New PASSWORD'}])
def test_each_piece_of_password_metadata_marks_the_field(meta) -> None:
    assert Redactor()({'type': 'text', **meta, 'value': 'Hunter2!x'})['value'] == REDACTED
    assert Redactor().field('Hunter2!x', role='AXTextField', metadata=tuple(meta.values())) == REDACTED


def test_typed_text_under_a_password_label_is_redacted_at_the_field_boundary() -> None:
    assert Redactor().field('Hunter2!x', role='AXTextField', label='Enter your password') == REDACTED


def test_a_show_password_checkbox_and_ordinary_fields_stay_readable() -> None:
    r = Redactor()
    assert r({'type': 'checkbox', 'label': 'Show password', 'value': 'checked'})['value'] == 'checked'
    assert r({'role': 'checkbox', 'label': 'Show password', 'value': 'checked'})['value'] == 'checked'
    assert r({'type': 'email', 'name': 'identifier', 'label': 'Email or phone', 'value': 'owner@kartikey.xyz'}) == \
        {'type': 'email', 'name': 'identifier', 'label': 'Email or phone', 'value': 'owner@kartikey.xyz'}
    assert r.field('hello', role='AXTextField', label='Search') == 'hello'
    assert r.field('checked', role='AXCheckBox', label='Show password') == 'checked'
    assert r({'role': 'menuitem', 'label': 'Passwords', 'text': 'Passwords'})['text'] == 'Passwords'
    assert r({'role': 'AXPopUpButton', 'title': 'Password manager', 'value': '1Password'})['value'] == '1Password'


def test_the_password_rule_covers_plain_text_fields_and_a_kindless_record_s_value_only() -> None:
    r = Redactor()
    policy = {'name': 'passwords-policy.md', 'content': 'Rotate every 90 days.'}
    assert r(policy) == policy
    for kind in ('email', 'tel', 'url'):
        reset = {'type': kind, 'label': 'Where should I send the link to reset your password?', 'value': 'x@y.z'}
        assert r(reset) == reset
    assert r({'name': 'Passwd', 'value': 'Hunter2!x'})['value'] == REDACTED          # a kindless record's value
    assert r({'type': 'text', 'label': 'Password', 'text': 'Hunter2!x'})['text'] == REDACTED


def test_an_answer_to_a_question_about_passwords_is_the_owners_words(daemon) -> None:
    """A question is no password field: its words never hide the owner's answer."""
    action = ActionRequest('owner.ask', 'owner_question', Identity.OWNER, frozenset(), 't', 'w', 'r', frozenset())
    ap = daemon.daemon.state.approvals.question(action, 'Which password manager do you use?', [])
    daemon.run(daemon.daemon._chat_answer(None, {'approval_id': ap.id, 'text': 'Bitwarden'}))
    assert ap.answer == 'Bitwarden'


# B. typed text and AX value changes -------------------------------------------------------------------------------------

@pytest.mark.parametrize('attrs', [{'AXTitle': 'Enter your password'}, {'AXDescription': 'Enter your password'},
                                   {'AXPlaceholderValue': 'Password'}, {'AXIdentifier': 'Passwd'},
                                   {'AXDOMIdentifier': 'login_pwd'}])
def test_typing_into_a_shown_password_field_records_nothing(monkeypatch, attrs) -> None:
    assert texts(capture(monkeypatch, Screen(bundle_id='com.google.Chrome', label=''), [key_event('s')])) == ['s']
    shown = Screen(bundle_id='com.google.Chrome', label='', **attrs)
    assert capture(monkeypatch, shown, [key_event('Hunter2!x')]) == []


class Element:
    """An AX element's attributes, read through the same `_attr` shape the watcher uses."""

    def __init__(self, **attrs: Any) -> None:
        self.attrs = {'AXIdentifier': 'f1'} | attrs

    @staticmethod
    def attr(element: Any, name: str) -> Any:
        return element.attrs.get(name) if isinstance(element, Element) else None


def test_value_changes_of_password_and_secure_fields_are_never_recorded() -> None:
    from ferb.sense.watchers.accessibility import element_facts
    shown = Element(AXRole='AXTextField', AXDescription='Enter your password', AXValue='Hunter2!x')
    secure = Element(AXRole='AXTextField', AXSubrole='AXSecureTextField', AXTitle='Password')
    search = Element(AXRole='AXTextField', AXTitle='Search')
    checkbox = Element(AXRole='AXCheckBox', AXTitle='Show password')
    assert element_facts('AXValueChanged', shown, Element.attr) is None
    assert element_facts('AXValueChanged', secure, Element.attr) is None
    assert element_facts('AXValueChanged', search, Element.attr)['title'] == 'Search'
    assert element_facts('AXValueChanged', checkbox, Element.attr)['title'] == 'Show password'
    focused = element_facts('AXFocusedUIElementChanged', shown, Element.attr)
    assert focused is not None and 'title' not in focused and 'Hunter2!x' not in json.dumps(focused)


# C. the extension -----------------------------------------------------------------------------------------------------

def extension(after: str) -> Any:
    """content.js's own secret classification and element description, run in node over plain input objects."""
    source = Path('extensions/chromium/content.js').read_text()
    secret_name = re.search(r'  const SECRET_NAME = .*\n', source).group()
    group = source[source.index('  function codeGroup('):source.index('\n\n  function formKind')]
    describe = source[source.index('  function describe('):source.index('  // Visible text nearest')]
    fill = source[source.index('  function fill('):source.index('\n\n  window.__ferb =')]
    script = secret_name + r'''
class HTMLInputElement {}
class HTMLTextAreaElement {}
let sealedSplitDocument = false, target = null;
const location = {origin: 'https://accounts.example'}, element = () => ({el: target}), setValue = (n, v) => n.value = v;
const sealedFields = new WeakSet(), visible = () => true, maskedEditable = () => false, label = el => el.labelText;
const implicitRole = () => 'textbox', hitTest = () => false, scrollerOf = () => null, round = n => n;
const formKind = () => null, maskUrl = u => u, innerWidth = 1000, innerHeight = 1000, containers = new Map();
const input = (attrs, labelText = '') => Object.assign(new HTMLInputElement(), {tagName: 'INPUT', type: 'text', name: '', id: '',
  autocomplete: '', value: '', maxLength: -1, inputMode: '', pattern: '', disabled: false, readOnly: false,
  parentElement: null, labelText, getAttribute: () => null, isConnected: true,
  getBoundingClientRect: () => ({x: 0, y: 0, width: 10, height: 10, left: 0, top: 0, right: 10, bottom: 10})}, attrs);
''' + group + '\n' + describe + fill + '\n' + after
    ran = subprocess.run([_node(), '-e', script], capture_output=True, text=True, check=True)
    return json.loads(ran.stdout)


def test_the_extension_masks_the_google_shown_password_field() -> None:
    shown = extension(r'''
const google = input({autocomplete: 'current-password', name: 'Passwd', value: 'Hunter2!x'}, 'Enter your password');
const one = (attrs, text) => { const e = describe(input(Object.assign({value: 'Hunter2!x'}, attrs), text), 1);
                               return {secret: e.secret, value: e.value}; };
console.log(JSON.stringify({google: describe(google, 1), each: [one({autocomplete: 'current-password'}),
  one({autocomplete: 'new-password'}), one({name: 'Passwd'}), one({id: 'login_pwd'}), one({}, 'Enter your password')],
  checkbox: one({type: 'checkbox', checked: true}, 'Show password'), email: one({type: 'email', name: 'identifier'},
  'Email or phone'), reset: one({type: 'email'}, 'Email to reset your password')}));
''')
    assert shown['google']['secret'] is True and shown['google']['value'] == 'filled'
    assert 'Hunter2!x' not in json.dumps(shown['google'])
    assert shown['each'] == [{'secret': True, 'value': 'filled'}] * 5
    assert shown['checkbox'] == {'secret': False, 'value': 'checked'}
    assert shown['email'] == {'secret': False, 'value': 'Hunter2!x'}
    assert shown['reset'] == {'secret': False, 'value': 'Hunter2!x'}


def test_a_login_code_fill_refuses_a_password_but_not_a_one_time_password_field() -> None:
    """"passw" also matches "One-time password": a field marked as a code field stays one; a real password doesn't."""
    filled = extension(r'''
const code = (attrs, text) => { target = input(attrs, text); return fill(1, 1, location.origin, '482913', true).detail; };
console.log(JSON.stringify({otp: code({}, 'One-time password'), otpName: code({name: 'one_time_password'}),
  autocomplete: code({autocomplete: 'one-time-code', name: 'otp_pwd'}),
  shown: code({autocomplete: 'current-password', name: 'Passwd'}, 'Enter your password'),
  labelled: code({}, 'Password'), hidden: code({type: 'password'}, 'One-time password')}));
''')
    assert filled == {'otp': 'filled', 'otpName': 'filled', 'autocomplete': 'filled', 'shown': 'code_field_changed',
                      'labelled': 'code_field_changed', 'hidden': 'code_field_changed'}


def test_a_login_code_is_never_asked_for_a_shown_password_field(bd, monkeypatch) -> None:
    """The extension now marks a shown password secret; a login-code fill still refuses it before asking the owner."""
    import tests.fake_extension as fake
    from tests.browserkit import call
    from tests.chromelab import SHOP, _ready
    monkeypatch.setattr(fake, 'page', lambda url: {'title': 'Sign in', 'text': 'Welcome', 'elements': [
        {'role': 'textbox', 'label': 'Enter your password', 'secret': True, 'input_type': 'text', 'value': 'filled'}]})
    d, launched = bd
    ext, grant = _ready(d, launched)
    tab = call(d, grant, 'browser.open', url=f'{SHOP}/signin')['content']['tab']
    snap = call(d, grant, 'browser.snapshot', tab=tab)['content']
    out = call(d, grant, 'browser.login_code', tab=tab, element=snap['elements'][0]['id'], effect='edit',
               why='finish requested login')
    assert out['is_error'] and 'not a password' in json.dumps(out)
    assert not any(a.kind == 'owner_login_code' for a in d.daemon.state.approvals.pending())


def test_a_login_code_is_asked_for_a_one_time_password_field(bd, monkeypatch) -> None:
    """A "One-time password" field is a code field: browser.login_code asks the owner and fills it."""
    import asyncio
    import tests.fake_extension as fake
    from ferb.identity.seal import seal
    from tests.browserkit import call
    from tests.chromelab import SHOP, _ready
    monkeypatch.setattr(fake, 'page', lambda url: {'title': 'Sign in', 'text': 'Enter the code', 'elements': [
        {'role': 'textbox', 'label': 'One-time password', 'secret': True, 'input_type': 'text', 'value': ''}]})
    d, launched = bd
    # Real sealing, as tests/test_owner_login.py does, without the installed signing helper.
    monkeypatch.setattr(d.daemon.router, 'seal_value', lambda key, value, command, origin:
                        seal(key, value, command, origin))
    ext, grant = _ready(d, launched)
    tab = call(d, grant, 'browser.open', url=f'{SHOP}/code')['content']['tab']
    snap = call(d, grant, 'browser.snapshot', tab=tab)['content']
    result = asyncio.run_coroutine_threadsafe(d.daemon.gateway.call(grant, 'browser.login_code',
        {'tab': tab, 'element': snap['elements'][0]['id'], 'effect': 'edit', 'why': 'finish requested login'}), d.loop)
    ext.wait_for(lambda: result.done() or any(a.kind == 'owner_login_code' for a in d.daemon.state.approvals.pending()))
    assert not result.done(), result.result()
    ap = next(a for a in d.daemon.state.approvals.pending() if a.kind == 'owner_login_code')
    d.run(d.daemon._chat_answer(None, {'approval_id': ap.id, 'text': '482913'}))
    out = result.result(10)
    assert not out['is_error'], out
    assert ext.fills and '482913' not in json.dumps(out)
