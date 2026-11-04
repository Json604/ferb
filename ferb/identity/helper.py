"""Call the installed, locally signed secret helper. Raw values travel only on private subprocess pipes.

A failure says why as far as FERB can tell (HAND-410), never with the request, the value or the Keychain account: the
Keychain's OSStatus and Security's text for it when the helper reports them; with a helper that reports none (the
pinned one, REL-404), a failed lookup is followed by the helper's own `present` check (no value read), which tells an
item the Keychain didn't show from one it has but wouldn't hand over."""
import base64
import json
import os
from pathlib import Path
import subprocess
from typing import Any, Callable

from ferb.core.errors import Refused

# HAND-410: told each helper failure's facts (operation, service, status / message / found when known), for the
# daemon's journal; called from whichever thread ran the helper. Never the value, the account or the request.
on_failure: Callable[[dict[str, Any]], None] = lambda facts: None


class HelperFailed(Refused):
    """The helper ran and the operation failed; `facts` is what is known about why (never a value or account)."""

    def __init__(self, message: str, facts: dict[str, Any]) -> None:
        super().__init__(message)
        self.facts = facts


def _helper(home: Path | None) -> Path:
    root = home or Path(os.environ.get('FERB_HOME', str(Path.home()/'.ferb')))
    binary, requirement = root/'bin'/'ferb-secrets', root/'bin'/'ferb-secrets.requirement'
    if not binary.is_file() or not requirement.is_file():
        raise Refused('the signed secret helper is unavailable; owner setup is required')
    checked = subprocess.run(['/usr/bin/codesign', '--verify', '--strict', '-R=' + requirement.read_text().strip(), str(binary)],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if checked.returncode:
        raise Refused('the secret helper signature does not match the installed identity')
    return binary


def _run(binary: Path, body: dict[str, Any]) -> tuple[int, Any]:
    result = subprocess.run([str(binary)], input=json.dumps(body).encode(), stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, close_fds=True)
    try:
        return result.returncode, json.loads(result.stdout)
    except (ValueError, UnicodeError):
        return result.returncode, None


def _status(out: Any) -> tuple[int | None, str]:
    """The OSStatus and Security's words for it, when the helper's error reply carries them."""
    if not isinstance(out, dict) or isinstance(out.get('status'), bool) or not isinstance(out.get('status'), int):
        return None, ''
    message = out.get('message')
    return int(out['status']), ' '.join(str(message).split())[:200] if isinstance(message, str) else ''


def _failure(binary: Path, body: dict[str, Any], out: Any) -> HelperFailed:
    operation = str(body.get('operation') or '')
    facts: dict[str, Any] = {'operation': operation, 'service': str(body.get('service') or '')}
    status, message = _status(out)
    if status is not None:
        facts['status'] = status
        if message:
            facts['message'] = message
        why = f"the Keychain refused (status {status}{': ' + message if message else ''})"
    elif operation == 'lookup' and body.get('service') and body.get('account'):
        code, seen = _run(binary, {'operation': 'present', 'service': body['service'], 'account': body['account']})
        found = seen.get('present') if code == 0 and isinstance(seen, dict) else None
        if found is True:
            facts['found'] = True
            why = ("the Keychain has this item but didn't hand it to FERB (a Keychain prompt was refused or couldn't "
                   "be shown); the helper doesn't report the Keychain's status")
        elif found is False:
            facts['found'] = False
            why = "the Keychain didn't show this item to FERB (it isn't saved there, or the keychain is locked)"
        else:
            why = 'the secret operation failed and the helper gave no reason'
    else:
        why = 'the secret operation failed and the helper gave no reason'
    try:
        on_failure(facts)
    except Exception:
        pass                                            # a recording never changes the answer
    return HelperFailed(why, facts)


def request(body: dict[str, Any], home: Path | None = None) -> dict[str, Any]:
    binary = _helper(home)
    code, out = _run(binary, body)
    if code or (isinstance(out, dict) and out.get('error')):
        raise _failure(binary, body, out)
    if not isinstance(out, dict):
        raise Refused('the secret helper returned an invalid result')
    return out


def seal(home: Path, key: bytes, value: str, command: str, origin: str) -> dict[str, str]:
    out = request({'operation': 'seal', 'key': base64.b64encode(key).decode(), 'value': value,
                   'command': command, 'origin': origin}, home)
    return {'nonce': out['nonce'], 'ct': out['ct']}
