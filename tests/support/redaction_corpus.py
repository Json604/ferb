"""Ordinary learning bodies for redaction tests (L29 re-reviews R1, Q1): episode cards and model-written procedures
whose text reads like a labelled secret to a prose redactor but holds none. Redaction must store them, never refuse
them, and a reading added for real secrets must leave them as they were."""
from typing import Any, Callable, Iterator


def ordinary_card() -> dict[str, Any]:
    """An episode card shaped as `prepare_episode` writes one (a WhatsApp reply)."""
    return {'task_id': '4b1f9c2e-7a3d-4e8b-9c1a-2f3e4d5c6b7a', 'request': 'reply to Alex in the FERB test group',
            'intent': 'reply on WhatsApp', 'slots': {}, 'apps': {'net.whatsapp.WhatsApp': '24.20.79'},
            'app_versions_captured': True, 'app_version_scope': 'identified apps and browser inventory',
            'surfaces': {'gui.click': 2, 'gui.read': 1},
            'actions': [{'id': '9d8c7b6a-5f4e-4d3c-2b1a-0f9e8d7c6b5a', 'tool': 'gui.click',
                         'args': {'app': 'net.whatsapp.WhatsApp', 'text': 'FERB test group'}, 'kind': 'act',
                         'outside_action': False, 'arg_sources': {'text': ['owner']}, 'error': None,
                         'result': {'ok': True}},
                        {'id': '1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d', 'tool': 'gui.click',
                         'args': {'app': 'net.whatsapp.WhatsApp', 'text': 'Reply'}, 'kind': 'act',
                         'outside_action': False, 'arg_sources': {'text': ['ferb']}, 'error': None,
                         'result': {'ok': True}}],
            'status': 'done', 'verified': True, 'conditions': [],
            'feedback': [{'verdict': 'pass', 'note': 'good', 'at': '2026-10-21T10:15:00+05:30'}],
            'narrative': 'Replied to Alex in FERB test group.', 'problems': [],
            'started_at': '2026-10-21T10:12:03+05:30', 'ended_at': '2026-10-21T10:12:41+05:30',
            'usage': {'input_tokens': 18234, 'output_tokens': 912}}


# Ordinary UI and request text that reads like a labelled secret in compact JSON (re-review 3's corpus)
ORDINARY = ['Pin message', 'Pin', 'Unpin', 'Pin chat', 'Pinned the message.', 'Token usage', 'Show password',
            'Forgot password?', 'Change password', 'Reset your password here', 'Session expired', 'Enter OTP',
            'Resend OTP', 'API keys', 'Copy API key', 'Secret Santa', 'Security key', 'Passkey', 'Enter PIN',
            'UPI PIN', 'Wrong password', 'Type your password here', "I couldn't find the password field.",
            'The session ended before the message was pinned.', 'Rotated the API key for staging.',
            'Opened the token settings page.', 'Saved the secret santa list to Notes.',
            'Turned on 2FA with an authenticator app.', 'The login page asked for a code; I stopped.', 'Pin the tab',
            'pin the message from Alex', 'change my netflix password', "what's my token usage",
            'open the secret santa list', 'reset the router password', 'show my API keys page']
FIELDS: dict[str, Callable[[dict[str, Any], str], None]] = {
    'request': lambda b, t: b.__setitem__('request', t),
    'intent': lambda b, t: b.__setitem__('intent', t),
    'click text': lambda b, t: b['actions'][1]['args'].__setitem__('text', t),
    'narrative': lambda b, t: b.__setitem__('narrative', t),
    'problems': lambda b, t: b.__setitem__('problems', [t]),
    'feedback note': lambda b, t: b['feedback'][0].__setitem__('note', t)}


def ordinary_cards() -> Iterator[tuple[str, dict[str, Any]]]:
    """The 216 cards: each ordinary text in each free-text field."""
    for text in ORDINARY:
        for field, place in FIELDS.items():
            body = ordinary_card()
            place(body, text)
            yield f'{field}: {text}', body


_PAGES = [
    'Sign in\nEmail or phone\nForgot email?\nNext', 'Welcome back! Enter your password\nShow password\nForgot password?',
    'Check your phone\nGoogle sent a notification to your Pixel. Tap "Yes" to continue.',
    'Pinned messages (2)\nAlex: dinner at 8?\nYou: sure', 'Your order #405-1234567-8901234 has shipped. Arriving Thursday.',
    'API keys\nName  Created  Last used\nstaging  Oct 1, 2026  2 days ago\nCreate new secret key',
    'Usage\nTokens this month: 1,284,551\nCost: $12.40', 'Token usage by model\ngpt-x  812,331\nother  472,220',
    'Two-step verification is ON since Oct 3, 2026', 'Security\nPassword  Last changed Mar 2, 2025\n2-Step Verification  On',
    'Session expired. Please sign in again.', 'Enter the 6-digit code from your authenticator app',
    'Secret Santa 2026 — draw on Dec 1', 'Your PIN was changed on 12 Oct 2026', 'Order code: AB-1234',
    'Invoice INV-2026-0042 — due 2026-11-01 — ₹4,500', 'The page showed "Password:" and a "Next" button.',
    'Clicked "Forgot password?" then "Send code".', 'It said "Enter the code we sent" and showed a field.',
    'Search results for "pin message whatsapp"', 'Settings > Privacy > Passkeys (1)',
    'Last login: 2026-10-20 22:14 from 10.0.0.12']
_CLICKS = ['Pin message', 'Pin chat', 'Unpin', 'Token usage', 'Show password', 'Forgot password?', 'Next', 'Send code',
           'API keys', 'Create new secret key', 'Security', 'Passkeys', 'Copy', 'Reply', 'Star', 'Resend OTP',
           'Enter PIN', 'Use another account', 'Sign out of all sessions']
_REQUESTS = ['pin the newest message in the FERB test group', "what's my API token usage this month",
             'reset my netflix password', 'check if 2FA is on for my google account', 'reply to Alex on WhatsApp',
             'find the invoice INV-2026-0042 in Gmail', 'open the secret santa sheet', 'show my API keys page',
             'log into kartikey.xyz admin', 'my UPI PIN changed, update the note']
_NARRATIVES = ['Pinned the newest message.', 'Read the usage page: 1,284,551 tokens this month.',
               'Opened the security page; the password was last changed Mar 2, 2025.',
               'The page asked for a code; I stopped and asked you.', 'Clicked "Pin message" in the menu.',
               'Opened API keys; there is one key named staging, last used 2 days ago.',
               'Searched Gmail for "INV-2026-0042" and opened the invoice.',
               'The session had expired, so I signed in again.']


def _act(i: int, tool: str, args: dict[str, Any], result: dict[str, Any] | None = None) -> dict[str, Any]:
    return {'id': f'0f9e8d7c-6b5a-4c3d-2e1f-0a9b8c7d6e{i:02d}', 'tool': tool, 'args': args, 'kind': 'act',
            'outside_action': False, 'arg_sources': {}, 'error': None, 'result': result or {'ok': True}}


def realistic_bodies() -> Iterator[tuple[str, dict[str, Any]]]:
    """Re-review 4's 443 bodies: 418 browser, WhatsApp and shell cards (22 page texts × 19 clicks) and 25 model-written
    procedures whose steps click such text or type a slot ("Password: {password}")."""
    for i, page in enumerate(_PAGES):
        for j, click in enumerate(_CLICKS):
            request = _REQUESTS[(i + j) % len(_REQUESTS)]
            actions = [_act(1, 'browser.navigate', {'url': 'https://myaccount.google.com/security?hl=en&authuser=0'}),
                       _act(2, 'browser.read', {'selector': 'main'},
                            {'ok': True, 'text': page, 'url': 'https://myaccount.google.com/security'}),
                       _act(3, 'browser.click', {'text': click, 'tab': 1832}),
                       _act(4, 'gui.click', {'app': 'net.whatsapp.WhatsApp', 'text': click}),
                       _act(5, 'shell.run', {'command': 'grep -n token ~/proj/config.yaml | head -5'},
                            {'ok': True, 'stdout': '12: token_limit: 4096\n', 'exit': 0})]
            yield f'page{i}/click{j}', {
                'task_id': '7c2e1a90-3b4d-4f5e-8a6b-1c2d3e4f5a6b', 'request': request, 'intent': request,
                'slots': {}, 'apps': {'com.google.Chrome': '130.0.6723.70', 'net.whatsapp.WhatsApp': '24.20.79'},
                'app_versions_captured': True, 'app_version_scope': 'identified apps and browser inventory',
                'surfaces': {'browser.navigate': 1, 'browser.click': 2, 'browser.read': 1}, 'actions': actions,
                'status': 'done', 'verified': True, 'conditions': [], 'feedback': [],
                'narrative': _NARRATIVES[(i * 3 + j) % len(_NARRATIVES)], 'problems': [page] if j % 4 == 0 else [],
                'started_at': '2026-10-21T10:12:03+05:30', 'ended_at': '2026-10-21T10:12:41+05:30',
                'usage': {'input_tokens': 18234, 'output_tokens': 912}}
    for click in _CLICKS + ['Password: {password}', 'Enter PIN {pin}', 'Enter the code {code}', 'PIN {pin}', '{otp}',
                            'Use code {code}']:
        yield f'procedure {click!r}', {
            'intent': 'do a thing', 'slots': {'chat': 'string', 'code': 'string', 'pin': 'string',
                                              'password': 'string', 'otp': 'string'},
            'preconditions': {'apps': {'net.whatsapp.WhatsApp': '24.20.79'}},
            'steps': [{'tool': 'gui.click', 'args': {'text': '{chat}'}, 'expect': 'chat open', 'optional': False,
                       'evidence': ['0f9e8d7c-6b5a-4c3d-2e1f-0a9b8c7d6e01']},
                      {'tool': 'gui.click', 'args': {'text': click}, 'expect': 'done', 'optional': False,
                       'evidence': ['0f9e8d7c-6b5a-4c3d-2e1f-0a9b8c7d6e02']}],
            'ui_hints': f'Right-click the message, then {click}.',
            'examples': [{'episode': 'm-' + 'a1' * 32, 'slots': {'chat': 'FERB test group'}}]}
