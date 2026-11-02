"""Local facts about how the owner uses this Mac (MEM-13): installed apps (Info.plist only), their
use (Spotlight's use counts, the owner's own focus events), the browsers and their profiles. Code only gathers; a model
interprets them (ferb/memory/mac_profile.py). Installed software alone never establishes an owner's occupation."""
import asyncio
import json
import os
import plistlib
import subprocess
from pathlib import Path
from typing import Any, Callable

# The memory key the old setup step saved its app-use dump under (MEM-13 replaced it; the step 9a clean-up made it
# observed and keeps it out of the core profile); never words the owner typed (D30, MP2)
PERSONA_KEY = "setup-persona"
PERSONA_FILE = "persona.json"          # FERB's home: what FERB saw on this Mac and when (MEM-13)
# Ruling: the most-used apps FERB gathers and gives the model to interpret (the rest are counted)
TOP_APPS = 25
# Where these lived before they moved out of config.toml (a 10 KB proposal there was parsed on every config read)
OLD_KEYS = ("persona_pending", "persona_reviewed")


class PersonaState:
    """What FERB saw on this Mac (`seen`: the summary, the facts saved and when) and, from older builds, the P28
    proposal and whether the owner reviewed it, in FERB's home (`persona.json`, owner-only, written atomically). The first read on a home that still has them under config.toml's [setup] moves them here
    (written here first, then removed there in one atomic write)."""

    def __init__(self, config: Any) -> None:
        self.config = config
        self.path = Path(config.home) / PERSONA_FILE

    def _read(self) -> dict[str, Any]:
        if self.path.exists():
            from ferb.config import _check_owner_only
            _check_owner_only(self.path)
            value = json.loads(self.path.read_text())
            return value if isinstance(value, dict) else {}
        old = {"pending": self.config.get("setup", "persona_pending"),
               "reviewed": self.config.get("setup", "persona_reviewed")}
        state = {k: v for k, v in old.items() if v is not None}
        if state:
            self._write(state)
            self.config.remove("setup", *OLD_KEYS)
        return state

    def _write(self, state: dict[str, Any]) -> None:
        from ferb.config import write_private
        write_private(Path(self.config.home), self.path, json.dumps(state, ensure_ascii=False))

    def _change(self, key: str, value: Any) -> None:
        state = self._read()
        state[key] = value
        self._write(state)

    def pending(self) -> dict[str, Any]:
        value = self._read().get("pending")
        return dict(value) if isinstance(value, dict) else {}

    def set_pending(self, proposal: dict[str, Any]) -> None:
        self._change("pending", proposal)

    def reviewed(self) -> bool:
        return self._read().get("reviewed") is True

    def set_reviewed(self, reviewed: bool) -> None:
        self._change("reviewed", reviewed)

    def seen(self) -> dict[str, Any]:
        value = self._read().get("seen")
        return dict(value) if isinstance(value, dict) else {}

    def set_seen(self, seen: dict[str, Any]) -> None:
        self._change("seen", seen)


def app_usage(apps: list[Any], run: Callable[..., Any] = subprocess.run) -> list[dict[str, Any]]:
    """Synchronous caller compatibility; production discovery uses the cancellable async collector."""
    if not apps:
        return []
    try:
        result = run(_usage_command(apps), capture_output=True)
        return _usage_rows(apps, result.stdout)
    except OSError:
        return _usage_rows(apps, b'')


def _usage_command(apps: list[Any]) -> list[str]:
    return ['/usr/bin/mdls', '-plist', '-', '-name', 'kMDItemPath', '-name', 'kMDItemUseCount',
            '-name', 'kMDItemLastUsedDate', *[a.path for a in apps]]


def _path_identity(path: str) -> Any:
    try:
        st = os.stat(path)
        return st.st_dev, st.st_ino  # /Applications and /System/Volumes/Data/Applications are firmlink aliases
    except OSError:
        return os.path.abspath(path)


def _usage_rows(apps: list[Any], raw: bytes) -> list[dict[str, Any]]:
    try:
        data = plistlib.loads(raw)
    except (ValueError, plistlib.InvalidFileException):
        data = []
    records = data if isinstance(data, list) else [data] if isinstance(data, dict) else []
    by_path = {_path_identity(row['kMDItemPath']): row for row in records
               if isinstance(row, dict) and isinstance(row.get('kMDItemPath'), str)}
    rows = []
    for app in apps:
        record = by_path.get(_path_identity(app.path), {})
        count = record.get('kMDItemUseCount')
        rows.append({'bundle_id': app.bundle_id, 'app': app.name,
                     'uses': count if isinstance(count, int) and not isinstance(count, bool) and count >= 0 else None,
                     'last_used': str(record.get('kMDItemLastUsedDate') or 'unknown')})
    return sorted(rows, key=lambda r: -(r['uses'] or 0))


async def app_usage_async(apps: list[Any], spawn: Callable[..., Any] = asyncio.create_subprocess_exec) -> list[dict[str, Any]]:
    if not apps:
        return []
    process = None
    spawning = asyncio.create_task(spawn(*_usage_command(apps), stdout=asyncio.subprocess.PIPE,
                                       stderr=asyncio.subprocess.PIPE))
    try:
        process = await asyncio.shield(spawning)
        output, _ = await process.communicate()
        return _usage_rows(apps, output)
    except asyncio.CancelledError:
        # Creation itself may be cancelled after the OS child exists but before its handle is returned.
        if process is None:
            try:
                process = await spawning
            except OSError:
                pass
        if process is not None:
            if process.returncode is None:
                try:
                    process.kill()
                except ProcessLookupError:
                    pass
            await process.communicate()  # drain pipes and reap this exact owned process before shutdown returns
        raise
    except OSError:
        return _usage_rows(apps, b'')


def with_activity(rows: list[dict[str, Any]], activity: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Owner-attributed focus events are a fallback, never a Spotlight launch count."""
    focused = {r['bundle_id']: r for r in activity}
    result = []
    for row in rows:
        item = dict(row)
        if item.get('uses') is None and item.get('bundle_id') in focused:
            item.update(focus_events=focused[item['bundle_id']]['focus_events'], source='owner_activity',
                        last_used=focused[item['bundle_id']]['last_used'])
        else:
            item['source'] = 'spotlight'
        result.append(item)
    return result


def mac_facts(rows: list[dict[str, Any]], scanned_at: str, browsers: list[str],
              profiles: list[dict[str, Any]], profiles_read: bool = True) -> dict[str, Any]:
    """What code gathered, for the model to interpret: the most-used apps (use counts and last use), how many apps are
    installed and how many show use, the installed browsers and their profiles (name, signed-in account, the one used
    last). `profiles_read` False: the profiles weren't looked at yet (no Full Disk Access, DMN-507), not that there
    are none."""
    used = [r for r in rows if (r.get('uses') or r.get('focus_events') or 0) > 0]
    ranked = sorted(used, key=lambda r: -(r.get('uses') or r.get('focus_events') or 0))
    return {'scanned_at': scanned_at, 'installed_apps': len(rows), 'apps_with_use': len(used),
            'apps': [{k: r[k] for k in ('app', 'bundle_id', 'uses', 'focus_events', 'last_used') if r.get(k) is not None}
                     for r in ranked[:TOP_APPS]],
            'browsers': browsers, 'profiles': profiles, 'profiles_read': profiles_read}


def facts_fingerprint(facts: dict[str, Any]) -> str:
    """What makes a refresh worth interpreting again (MEM-13, "refreshed as usage changes"): the set of the most-used
    apps, the browsers and their profiles; never the counts alone, which change every day."""
    import hashlib
    key = {'apps': sorted(str(a.get('bundle_id') or a.get('app')) for a in facts.get('apps', [])[:10]),
           'browsers': sorted(facts.get('browsers', [])),
           'profiles': sorted(f"{p.get('browser')}|{p.get('name')}|{p.get('email')}|{p.get('last_used')}"
                              for p in facts.get('profiles', []))}
    return hashlib.sha256(json.dumps(key, sort_keys=True).encode()).hexdigest()[:16]


def browser_profiles() -> list[dict[str, Any]]:
    """Each Chromium-family browser's profiles from its Local State (read only; ferb/act/browser/profiles.py): name,
    signed-in account and whether it is the one used last. Only called once FERB has Full Disk Access (DMN-507)."""
    from ferb.act.browser import profiles as bp
    out = []
    for browser in bp.installed():
        last = bp.last_used(browser)
        for p in bp.discover(browser):
            out.append({'browser': bp.BROWSERS[browser].label, 'name': p.display_name, 'email': p.email,
                        'last_used': p.folder == last})
    return out


def _other_app_data() -> bool:
    from ferb.device.permissions import other_app_data
    return other_app_data()


async def discover_persona(home: Any, device_id: str, activity: list[dict[str, Any]],
                           excluded: frozenset[str],
                           profiles: Callable[[], list[dict[str, Any]]] = browser_profiles,
                           allowed: Callable[[], bool] = _other_app_data) -> dict[str, Any]:
    """Refresh the bundle inventory and read metadata only; never launch an app or call a model (the facts for
    `mac_profile.interpret`). The part needing no other app's data (the apps, their use, which browsers are installed)
    runs at once; the browsers' profiles only once FERB has Full Disk Access (`allowed`, DMN-507): before it they are
    left out and marked not read, and the daemon gathers them again once access is there."""
    from ferb.device.profile import ProfileStore
    from ferb.device.scan import scan
    profile = scan(device_id)
    ProfileStore(home).save(profile, 'mac profile scan')
    rows = await app_usage_async([a for a in profile.apps if a.bundle_id not in excluded])
    readable = await asyncio.to_thread(allowed)
    found: list[dict[str, Any]] = []
    if readable:
        try:
            found = profiles()
        except Exception:                                # a browser's unreadable Local State: no profiles, said
            found = []
    return mac_facts(with_activity(rows, activity), profile.scanned_at,
                     [b.name for b in profile.browsers if b.bundle_id not in excluded], found, profiles_read=readable)
