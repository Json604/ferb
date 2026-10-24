"""Explicit owner-requested secrets. Values stay in the login Keychain; metadata binds each to its exact place.

One login record per place (L36 §5): an HTTPS origin, or `app:<bundle id>` for a native app. A record holds the
username (plain text: owner decision 2026-10-24, "Yes, plain"; set only from the owner's own words, `set_username`, and
cleared when the memory it came from is forgotten), the sites the owner named for a task its entries were saved in
(`sites`: hosts, taken by code; I14) and named sealed entries (the password, each security question's answer
under that question's text), each a Keychain item under service `ferb.owner-secrets`. Saving a name that exists for
that place replaces its value, so no duplicates. The place is always code-derived (D12); the worker chooses only the
entry's name.

A record serves its own place and the sites it lists (I14): the visa site's sign-in form is a single sign-on page on
another origin (atlasauth.b2clogin.com) while the owner names usvisascheduling.com, so the form's record lists the
task's sites the owner typed (the gateway picks them, `Gateway._login_sites`), a listing for the site finds it, and a
username the owner gave for the site names the form's account too. A site matches a host exactly, a leading "www."
aside (`same_site`): never a subdomain, since on shared hosting (b2clogin.com, github.io) each tenant has its own. The
records of one site with and without "www." serve each other too. Entries still fill only at their record's own place
(`reveal`).

`owner-secret-handles.json` (version 2): {"version": 2, "records": {place: {"username", "username_source", "sites":
[host], "entries": {name: Keychain account}}}, "unusable": [{"name", "place", "why"}]}. A new entry's Keychain account
is "<place> · <name>". A version-1 file (handle -> place) is read as version 2 and never rewritten by a read: each
handle becomes an entry of its place under its own name (its Keychain account unchanged, so its value is never entered
again), and one bound to a whole web browser is listed unusable. A file that can't be read, or isn't in either form, is
never overwritten: every call refuses and says why.

The signed Security.framework helper keeps values out of argv, environment and metadata; private pipes carry values.
"""
import json
import os
import threading
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterable, Iterator

from ferb.core.errors import Refused

SERVICE = 'ferb.owner-secrets'
VERSION = 2
WHOLE_BROWSER = 'it was saved for a whole web browser, which FERB no longer allows; ask the owner again'
_write_lock = threading.RLock()


def _keychain(handle: str, value: str | None = None, home: Path | None = None) -> str | None:
    from ferb.identity.helper import request
    out = request({'operation': 'store' if value is not None else 'lookup', 'service': SERVICE,
                   'account': handle, **({'value': value} if value is not None else {})}, home)
    return out.get('value')


def refuse_whole_browser(target: str) -> None:
    """A secret is bound to the exact place it is filled: a web page's HTTPS origin, or a native app. A web browser
    as an app (`app:com.google.Chrome`) is every site it shows, so no handle is bound to one (owner 2026-10-24: "at
    places only ive approved")."""
    from ferb.act.browser.profiles import is_web_browser
    if target.startswith('app:') and is_web_browser(target[4:]):
        raise Refused('a secret is never bound to a whole web browser; a web page\'s secret is bound to its HTTPS '
                      'address through the browser connection')


def login_place(target: str) -> str:
    """The place a login record is kept for: `app:<bundle id>` as given, else the address's HTTPS origin
    (https://host[:port], the form a browser fill's origin takes); "" for anything else."""
    from ferb.identity.secrets import normalize_origin
    target = target.strip()
    if target.startswith('app:'):
        return target if len(target) > 4 else ''
    try:
        origin = normalize_origin(target)
    except ValueError:                                  # a port no address can have
        return ''
    return origin if origin.startswith('https://') and origin != 'https://' else ''


def site_host(place_or_site: str) -> str:
    """A web place's or a task site's host as a browser reads it (IDNA, lowercase, no trailing dot); "" for an app."""
    from ferb.policy import urls
    text = place_or_site.strip()
    if not text or text.startswith('app:'):
        return ''
    parsed = urls.parse(text if '://' in text else 'https://' + text)
    return parsed.host.rstrip('.') if parsed is not None and parsed.host else ''


def same_site(a: str, b: str) -> bool:
    """Two hosts are one site when equal, a leading "www." aside; never a subdomain or a sibling (fix round 1: with
    subdomains, a record listing b2clogin.com would serve evil.b2clogin.com, another tenant's page). An empty host (an
    app's place has none) is no site, nor is "www." alone."""
    a, b = a.removeprefix('www.'), b.removeprefix('www.')
    return bool(a) and bool(b) and a == b


def typed_site(host: str, typed: Any) -> bool:
    """Whether the owner typed `host`'s site: the host, with or without a leading "www.", is a whole token or a URL's
    host of their words (`typed`: `provenance.prepare` of them; rule 8's provenance, as a sink's destination)."""
    bare = host.removeprefix('www.')
    return bool(bare) and bool({bare, 'www.' + bare} & (typed.tokens | typed.hosts))


def _empty() -> dict[str, Any]:
    return {'username': None, 'username_source': None, 'sites': [], 'entries': {}}


def _migrated(old: dict[str, str]) -> dict[str, Any]:
    data: dict[str, Any] = {'version': VERSION, 'records': {}, 'unusable': []}
    for handle, place in old.items():
        try:
            refuse_whole_browser(place)
        except Refused:
            data['unusable'].append({'name': handle, 'place': place, 'why': WHOLE_BROWSER})
            continue
        data['records'].setdefault(place, _empty())['entries'][handle] = handle
    return data


def _strings(value: Any) -> bool:
    return isinstance(value, dict) and all(isinstance(k, str) and isinstance(v, str) for k, v in value.items())


def _version_2(data: dict[str, Any]) -> dict[str, Any] | None:
    """A version-2 file's data with every record's fields present, or None when its shape or a field's type is not
    version 2's (so the file is refused, never overwritten)."""
    records, unusable = data.get('records'), data.get('unusable', [])
    if not isinstance(records, dict) or not isinstance(unusable, list):
        return None
    shaped: dict[str, Any] = {}
    for place, record in records.items():
        if not isinstance(record, dict) or not _strings(record.get('entries')):
            return None
        shaped[place] = _empty() | record
        sites, username, source = shaped[place]['sites'], shaped[place]['username'], shaped[place]['username_source']
        if (not isinstance(sites, list) or not all(isinstance(site, str) for site in sites)
                or not isinstance(username, (str, type(None))) or not isinstance(source, (str, type(None)))):
            return None
    if not all(isinstance(u, dict) and _strings(u) and {'name', 'place', 'why'} <= u.keys() for u in unusable):
        return None
    return {'version': VERSION, 'records': shaped, 'unusable': unusable}


def _linked(records: dict[str, Any], place: str) -> list[str]:
    """The other records that serve `place` (I14): one at the same site, a leading "www." aside (the planner writes a
    username's site either way, L36 §5), one listing its host's site (a sign-on form's record saved during a task on
    that site), or one at the host of a site its own record lists (the site that task named)."""
    host, own_sites = site_host(place), records.get(place, {}).get('sites', [])
    found = []
    for other, record in records.items():
        other_host = site_host(other)
        if other != place and (same_site(host, other_host) or any(same_site(s, host) for s in record.get('sites', []))
                               or any(same_site(s, other_host) for s in own_sites)):
            found.append(other)
    return sorted(found)


def _accounts(data: dict[str, Any]) -> set[str]:
    """Every Keychain account a saved secret uses: each record's entries, and each unusable one's (its old handle)."""
    return ({a for record in data['records'].values() for a in record['entries'].values()}
            | {u['name'] for u in data['unusable']})


def _username(records: dict[str, Any], place: str) -> str | None:
    """A place's username: its own record's, else the one its linked records agree on (never one of two that
    differ, so a question never names the wrong account)."""
    own = records.get(place, {}).get('username')
    if own:
        return str(own)
    linked = {str(records[p]['username']) for p in _linked(records, place) if records[p].get('username')}
    return linked.pop() if len(linked) == 1 else None


class OwnerSecrets:
    def __init__(self, home: Path, keychain: Any = _keychain) -> None:
        self.path = home / 'owner-secret-handles.json'
        self.keychain = (lambda handle, value=None: _keychain(handle, value, home)) if keychain is _keychain else keychain

    def _load(self) -> dict[str, Any]:
        if not self.path.exists():
            return {'version': VERSION, 'records': {}, 'unusable': []}
        try:
            data = json.loads(self.path.read_text())
        except (OSError, ValueError) as e:
            raise Refused(f"FERB's saved secret names ({self.path.name}) can't be read ({type(e).__name__}); FERB "
                          "leaves the file as it is") from None
        shaped = _version_2(data) if isinstance(data, dict) and data.get('version') == VERSION else None
        if shaped is not None:
            return shaped
        if _strings(data):                              # version 1: handle -> place (version 2's number is no string)
            return _migrated(data)
        raise Refused(f"FERB's saved secret names ({self.path.name}) are in a form this FERB doesn't know; FERB "
                      "leaves the file as it is")

    @contextmanager
    def _edit(self) -> Iterator[dict[str, Any]]:
        """Every change goes through here: under one lock (instances are short-lived per prompt and daemon worker
        threads share the file), on the file as read, written back only when something changed, so a change that
        changes nothing never rewrites a version-1 file."""
        with _write_lock:
            data = self._load()
            before = json.dumps(data, sort_keys=True)
            yield data
            if json.dumps(data, sort_keys=True) != before:
                self._save(data)

    def records(self) -> dict[str, dict[str, Any]]:
        return dict(self._load()['records'])

    def unusable(self) -> list[dict[str, str]]:
        return list(self._load()['unusable'])

    def username(self, target: str) -> str | None:
        return _username(self._load()['records'], target)

    def listing(self, target: str) -> dict[str, Any]:
        """What `secrets.list` shows for one place: its username and its entries' names, never a value or a Keychain
        account (L36 §5); every entry saved for a whole browser, unusable; and, under `linked`, each other record that
        serves this place, by the place where its entries fill (I14)."""
        data = self._load()
        records = data['records']
        out: dict[str, Any] = {'place': target, 'username': _username(records, target),
                               'entries': sorted(records.get(target, _empty())['entries']),
                               'unusable': [{'name': u['name'], 'why': u['why']} for u in data['unusable']]}
        linked = [{'place': p, 'username': records[p]['username'], 'entries': sorted(records[p]['entries'])}
                  for p in _linked(records, target)]
        return out | ({'linked': linked} if linked else {})

    def reveal(self, handle: str, target: str) -> str:
        refuse_whole_browser(target)                    # a handle saved that way before the rule is never filled
        account = self._load()['records'].get(target, _empty())['entries'].get(handle)
        if account is None:
            raise Refused('the secret handle is not bound to this exact destination')
        value = self.keychain(account)
        if value is None:
            raise Refused('the saved secret is unavailable')
        return value

    def remember(self, handle: str, target: str, value: str, sites: Iterable[str] = ()) -> None:
        """Save `value` under the name `handle` for the place `target`, replacing that name's value; `sites`: the
        sites the record lists from then on (I14; the gateway's `_login_sites`). A new entry's Keychain account is
        never one another entry already uses (a version-1 handle can be any text)."""
        refuse_whole_browser(target)
        own = site_host(target)
        hosts = {h for h in map(site_host, sites) if h and not same_site(h, own)}
        with self._edit() as data:
            record = data['records'].setdefault(target, _empty())
            account = record['entries'].get(handle)
            if account is None:
                account = f'{target} · {handle}'
                if account in _accounts(data):
                    raise Refused('that name would reuse the Keychain item of another saved secret; save it under '
                                  'another name')
            self.keychain(account, value)
            record['entries'][handle] = account
            record['sites'] = sorted(set(record['sites']) | hosts)

    def set_username(self, target: str, username: str, source: str) -> None:
        """The record's username, from an owner-stated memory item (`source`: "memory:<id>"), kept as the owner spelled
        it; `target` must be a login place."""
        if login_place(target) != target:
            raise Refused('a login record is kept for an https:// origin or app:<bundle id>')
        refuse_whole_browser(target)
        with self._edit() as data:
            record = data['records'].setdefault(target, _empty())
            record['username'], record['username_source'] = username, source

    def clear_usernames(self, sources: set[str]) -> list[str]:
        """Clear every username that came from one of `sources` (a forgotten or retracted memory item); the places,
        sorted."""
        with self._edit() as data:
            cleared = sorted(p for p, record in data['records'].items() if record.get('username_source') in sources)
            for place in cleared:
                data['records'][place]['username'] = data['records'][place]['username_source'] = None
        return cleared

    def _save(self, data: dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix('.tmp')
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, 'w') as out:
            os.fchmod(out.fileno(), 0o600)
            json.dump(data, out)
        os.replace(tmp, self.path)
