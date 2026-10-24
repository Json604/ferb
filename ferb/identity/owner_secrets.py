"""Explicit owner-requested secret handles. Values stay in the login Keychain; metadata binds the exact target.

The signed Security.framework helper keeps values out of argv, environment and metadata; private pipes carry values.
"""
import json
import os
import threading
from pathlib import Path
from ferb.core.errors import Refused

SERVICE = 'ferb.owner-secrets'
_write_lock = threading.RLock()


def _keychain(handle: str, value: str | None = None, home: Path | None = None) -> str | None:
    from ferb.identity.helper import request
    out = request({'operation': 'store' if value is not None else 'lookup', 'service': SERVICE,
                   'account': handle, **({'value': value} if value is not None else {})}, home)
    return out.get('value')


def _not_a_whole_browser(target: str) -> None:
    """A secret is bound to the exact place it is filled: a web page's HTTPS origin, or a native app. A web browser
    as an app (`app:com.google.Chrome`) is every site it shows, so no handle is bound to one (owner 2026-10-24: "at
    places only ive approved")."""
    from ferb.act.browser.profiles import is_web_browser
    if target.startswith('app:') and is_web_browser(target[4:]):
        raise Refused('a secret is never bound to a whole web browser; a web page\'s secret is bound to its HTTPS '
                      'address through the browser connection')


class OwnerSecrets:
    def __init__(self, home: Path, keychain=_keychain) -> None:
        self.path = home / 'owner-secret-handles.json'
        self.keychain = (lambda handle, value=None: _keychain(handle, value, home)) if keychain is _keychain else keychain

    def handles(self) -> dict[str, str]:
        return json.loads(self.path.read_text()) if self.path.exists() else {}

    def reveal(self, handle: str, target: str) -> str:
        _not_a_whole_browser(target)                    # a handle saved that way before the rule is never filled
        if self.handles().get(handle) != target:
            raise Refused('the secret handle is not bound to this exact destination')
        value = self.keychain(handle)
        if value is None:
            raise Refused('the saved secret is unavailable')
        return value

    def remember(self, handle: str, target: str, value: str) -> None:
        _not_a_whole_browser(target)
        # Instances are short-lived per prompt; serialize their metadata updates across daemon worker threads.
        with _write_lock:
            self._remember(handle, target, value)

    def _remember(self, handle: str, target: str, value: str) -> None:
        entries = self.handles()
        if handle in entries and entries[handle] != target:
            raise Refused('that secret handle belongs to another destination')
        self.keychain(handle, value)
        entries[handle] = target
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix('.tmp')
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, 'w') as out:
            os.fchmod(out.fileno(), 0o600)
            json.dump(entries, out)
        os.replace(tmp, self.path)
