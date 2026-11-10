"""Explicit owner-requested secrets. Values stay in the login Keychain; metadata binds each to its exact place.

One login record per place (L36 §5): an HTTPS origin, or `app:<bundle id>` for a native app. A record holds the
username (plain text: owner decision 2026-10-24, "Yes, plain"; set only from the owner's own words, `set_username`;
a newer version of the memory it came from gives it its spelling or clears it, and a forget, retract or retire of that
memory clears it: `replace_usernames`; read only while that memory holds, past its end date none: `_held`), the
sites the owner named for a task its entries were saved in
(`sites`: hosts, taken by code; I14) and named sealed entries (the password, each security question's answer
under that question's text), each a Keychain item under service `ferb.owner-secrets`. Saving a name that exists for
that place replaces its value, so no duplicates. The place is always code-derived (D12); the worker chooses only the
entry's name.

A record serves its own place and the sites it lists (I14): an appointment site's sign-in form is a single sign-on page
on another origin (login.provider.example) while the owner names appointments.example, so the form's record lists the
task's site the owner typed, when they typed exactly one (the gateway picks it, `HiddenInput._question_sites`), a listing
for the site finds it, and a username the owner gave for the site names the form's account too. Before any entry is
saved there, a question or a listing during a task reads the task's one owner-typed site as a link the same way,
writing nothing. A username read through a link is shown with the place it came from (`account`). A site matches a
host exactly, a leading "www." aside (`same_site`): never a subdomain, since on shared hosting (b2clogin.com,
github.io) each tenant has its own. The records of one site with and without "www." serve each other too. Entries
still fill only at their record's own place (`reveal`).

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
from typing import Any, Container, Iterable, Iterator

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


def _keychain_delete(account: str, home: Path | None = None) -> bool:
    """Delete one saved secret's Keychain item through the signed helper (which made it); never reads its value.
    False when there was none."""
    from ferb.identity.helper import request
    out = request({'operation': 'delete', 'service': SERVICE, 'account': account}, home)
    return bool(out.get('deleted'))


def refuse_whole_browser(target: str) -> None:
    """A secret is bound to the exact place it is filled: a web page's HTTPS origin, or a native app. A web browser
    as an app (`app:com.google.Chrome`) is every site it shows, so no handle is bound to one (owner 2026-10-24: "at
    places only ive approved")."""
    from ferb.act.mac.browsers import is_browser
    if target.startswith('app:') and is_browser(target[4:]):
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


# GATE-206: a card's details saved from a card field (autocomplete cc-number / cc-csc / cc-exp, the field
# kinds the extension reports) keep that kind, and fill only a field of the same kind; a card field takes only a saved
# entry of its kind (or the owner's one-time hidden input). Models see the name and the kind, never the value.
CARD_KINDS = frozenset({'card_number', 'card_csc', 'card_expiry'})


def _empty() -> dict[str, Any]:
    return {'username': None, 'username_source': None, 'sites': [], 'entries': {}, 'kinds': {}}


def card_mismatch(saved: str | None, field: str | None) -> str | None:
    """Why a saved entry of kind `saved` can't fill a field of kind `field` (either a card kind), else None."""
    if (saved in CARD_KINDS or field in CARD_KINDS) and saved != field:
        what = {'card_number': "a card number", 'card_csc': "a card's security code",
                'card_expiry': "a card's expiry date"}
        return (f"that saved secret is {what.get(saved or '', 'not a card detail')} and this field takes "
                f"{what.get(field or '', 'no card detail')}; a card detail fills only its own kind of field")
    return None


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
        if not isinstance(record, dict) or not _strings(record.get('entries')) or not _strings(record.get('kinds', {})):
            return None
        shaped[place] = _empty() | record
        sites, username, source = shaped[place]['sites'], shaped[place]['username'], shaped[place]['username_source']
        if (not isinstance(sites, list) or not all(isinstance(site, str) for site in sites)
                or not isinstance(username, (str, type(None))) or not isinstance(source, (str, type(None)))):
            return None
    if not all(isinstance(u, dict) and _strings(u) and {'name', 'place', 'why'} <= u.keys() for u in unusable):
        return None
    return {'version': VERSION, 'records': shaped, 'unusable': unusable}


def _linked(records: dict[str, Any], place: str, sites: Iterable[str] = ()) -> list[str]:
    """The other records that serve `place` (I14): one at the same site, a leading "www." aside (the planner writes a
    username's site either way, L36 §5), one listing its host's site (a sign-on form's record saved during a task on
    that site), or one at the host of a site its own record lists (the site that task named). `sites`: the current
    task's one owner-typed site (`HiddenInput._question_sites`), read as if the place's record listed it, so the first
    sign-in on a form already finds the site's record; never written (coordinator ruling on concern 1)."""
    host, own_sites = site_host(place), [*records.get(place, {}).get('sites', []), *sites]
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


def _held(record: dict[str, Any], holding: Container[str] | None) -> str | None:
    """A record's username, unless the memory it came from no longer holds: `holding` is the sources that do (the
    caller's memory decides: past its end date, retired, retracted or forgotten is none; L36 §6, review fix round 1),
    None when the caller has no memory to ask."""
    name = record.get('username')
    return str(name) if name and (holding is None or record.get('username_source') in holding) else None


def _account(records: dict[str, Any], place: str, sites: Iterable[str] = (),
             holding: Container[str] | None = None) -> tuple[str | None, str | None]:
    """A place's username and where it came from: its own record's (None), else the one its linked records agree on,
    with the place of the first record holding it (review I-2: the owner sees whose account a question names); never
    one of two that differ, so a question never names the wrong account. Only a username whose memory still holds
    (`_held`)."""
    own = _held(records.get(place, {}), holding)
    if own:
        return own, None
    holders = {p: name for p in _linked(records, place, sites) if (name := _held(records[p], holding))}
    names = set(holders.values())
    return (names.pop(), min(holders)) if len(names) == 1 else (None, None)


class OwnerSecrets:
    def __init__(self, home: Path, keychain: Any = _keychain, delete: Any = _keychain_delete) -> None:
        self.path = home / 'owner-secret-handles.json'
        self.keychain = (lambda handle, value=None: _keychain(handle, value, home)) if keychain is _keychain else keychain
        self.keychain_delete = (lambda account: _keychain_delete(account, home)) if delete is _keychain_delete else delete

    def inventory(self) -> list[dict[str, Any]]:
        """Every saved entry by name, for FERB.app's Settings → Data (APP-2): its place, its name (the
        handle a model sees), its kind (a card detail's, else "secret") and whether it is unusable. Never a value, a
        Keychain account or a username."""
        data = self._load()
        out = [{'place': place, 'name': name, 'kind': record['kinds'].get(name, 'secret'), 'unusable': False}
               for place, record in sorted(data['records'].items()) for name in sorted(record['entries'])]
        out += [{'place': u['place'], 'name': u['name'], 'kind': 'secret', 'unusable': True} for u in data['unusable']]
        return out

    def delete(self, place: str, name: str) -> dict[str, Any]:
        """The owner's Delete on one saved entry (APP-2): its Keychain item goes first (through the helper,
        never read), then its name; a record left with no entries, username or sites goes too. Refused for a name not
        saved there. Returns what was deleted (names only)."""
        with self._edit() as data:
            record = data['records'].get(place)
            unusable = next((u for u in data['unusable'] if u['place'] == place and u['name'] == name), None)
            if record is not None and name in record['entries']:
                account = record['entries'][name]
            elif unusable is not None:
                account = unusable['name']
            else:
                raise Refused('nothing is saved under that name for that place')
            uses = [a for r in data['records'].values() for a in r['entries'].values()] + \
                [u['name'] for u in data['unusable']]
            if uses.count(account) == 1:                        # an item another entry also names stays for that one
                self.keychain_delete(account)
            if record is not None and name in record['entries']:
                del record['entries'][name]
                record['kinds'].pop(name, None)
                if not record['entries'] and not record.get('username') and not record.get('sites'):
                    del data['records'][place]
            else:
                data['unusable'].remove(unusable)
        return {'place': place, 'name': name, 'deleted': True}

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

    def username(self, target: str, sites: Iterable[str] = ()) -> str | None:
        """`sites`: the current task's owner-typed site, read as a link (`_linked`)."""
        return _account(self._load()['records'], target, sites)[0]

    def account(self, target: str, sites: Iterable[str] = (),
                holding: Container[str] | None = None) -> tuple[str | None, str | None]:
        """The username for `target` and, when it comes through a link, the place whose record holds it; `holding`:
        the memory sources a username may still come from (`_held`)."""
        return _account(self._load()['records'], target, sites, holding)

    def listing(self, target: str, sites: Iterable[str] = (), holding: Container[str] | None = None) -> dict[str, Any]:
        """What `secrets.list` shows for one place: its username and its entries' names, never a value or a Keychain
        account (L36 §5); every entry saved for a whole browser, unusable; and, under `linked`, each other record that
        serves this place, by the place where its entries fill (I14). `sites`: the current task's owner-typed site,
        read as a link (`_linked`). A username read through a link comes with `username_site`, the host of the record
        holding it (review I-2). `holding`: only a username whose memory still holds is listed (`_held`)."""
        data = self._load()
        records = data['records']
        username, source = _account(records, target, sites, holding)
        out: dict[str, Any] = {'place': target, 'username': username,
                               **({'username_site': site_host(source)} if source else {}),
                               'entries': sorted(records.get(target, _empty())['entries']),
                               **({'card_details': dict(kinds)} if (kinds := records.get(target, _empty())['kinds'])
                                  else {}),
                               'unusable': [{'name': u['name'], 'why': u['why']} for u in data['unusable']]}
        linked = [{'place': p, 'username': _held(records[p], holding), 'entries': sorted(records[p]['entries'])}
                  for p in _linked(records, target, sites)]
        return out | ({'linked': linked} if linked else {})

    def reveal(self, handle: str, target: str, kind: str | None = None) -> str:
        """The value saved as `handle` for `target`, for a field of kind `kind` (a card detail fills only its own kind
        of field, GATE-206)."""
        refuse_whole_browser(target)                    # a handle saved that way before the rule is never filled
        record = self._load()['records'].get(target, _empty())
        account = record['entries'].get(handle)
        if account is None:
            raise Refused('the secret handle is not bound to this exact destination')
        if (why := card_mismatch(record['kinds'].get(handle), kind)) is not None:
            raise Refused(why)
        value = self.keychain(account)
        if value is None:
            raise Refused('the saved secret is unavailable')
        return value

    def remember(self, handle: str, target: str, value: str, sites: Iterable[str] = (), kind: str | None = None) -> None:
        """Save `value` under the name `handle` for the place `target`, replacing that name's value; `sites`: the
        sites the record lists from then on (I14; the gateway's `_question_sites`). A new entry's Keychain account is
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
            if kind in CARD_KINDS:                       # GATE-206: a card detail keeps its field's kind
                record['kinds'][handle] = kind
            else:
                record['kinds'].pop(handle, None)
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

    def replace_usernames(self, sources: set[str], username: str | None = None, source: str | None = None) -> list[str]:
        """Every username that came from one of `sources` (memory items that stopped holding): `username` from `source`
        instead (a newer version of the same fact in the owner's spelling, L36 review m-8), or cleared when `username`
        is None; the places, sorted."""
        with self._edit() as data:
            changed = sorted(p for p, record in data['records'].items() if record.get('username_source') in sources)
            for place in changed:
                record = data['records'][place]
                record['username'], record['username_source'] = (username, source) if username else (None, None)
        return changed

    def clear_usernames(self, sources: set[str]) -> list[str]:
        """Clear every username that came from one of `sources` (a forgotten, retracted, retired or superseded memory
        item); the places, sorted."""
        return self.replace_usernames(sources)

    def _save(self, data: dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix('.tmp')
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, 'w') as out:
            os.fchmod(out.fileno(), 0o600)
            json.dump(data, out)
        os.replace(tmp, self.path)
