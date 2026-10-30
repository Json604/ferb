"""Local app-use evidence for a setup persona. Installed software never establishes an owner's occupation."""
import asyncio
import json
import os
import plistlib
import subprocess
from pathlib import Path
from typing import Any, Callable

# The memory key of the approved setup persona: FERB's own app-use summary, never words the owner typed (D30, MP2)
PERSONA_KEY = "setup-persona"
PERSONA_FILE = "persona.json"          # FERB's home: the scan's proposal and whether the owner reviewed it
# Where these lived before they moved out of config.toml (a 10 KB proposal there was parsed on every config read)
OLD_KEYS = ("persona_pending", "persona_reviewed")


class PersonaState:
    """The P28 proposal FERB scanned and whether the owner reviewed it, in FERB's home (`persona.json`, owner-only,
    written atomically). The first read on a home that still has them under config.toml's [setup] moves them here
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


def usage_summary(rows: list[dict[str, Any]]) -> str:
    used = [r for r in rows if (r.get('uses') or r.get('focus_events') or 0) > 0]
    return "Apps you use: " + ", ".join(
        f"{r['app']} ({r['focus_events']} recorded focus events)" if r.get('uses') is None else
        f"{r['app']} ({r['uses']} recorded uses)" for r in used
    ) if used else "App-use history is unavailable; tell me how you use this Mac."


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


def persona_proposal(rows: list[dict[str, Any]], scanned_at: str) -> dict[str, Any]:
    used = sum((r.get('uses') or r.get('focus_events') or 0) > 0 for r in rows)
    evidence = [f'Local scan: {scanned_at}']
    for row in rows:
        count = (f"{row['focus_events']} owner focus events" if row.get('focus_events') else
                 f"{row['uses']} Spotlight uses" if row.get('uses') is not None else 'use count unavailable')
        evidence.append(f"{row['app']}: {count}; last used {row.get('last_used', 'unknown')}.")
    reason = (f'I found recorded use for {used} of {len(rows)} apps on this Mac. '
              'Save this app-use profile, edit how you use this Mac below, or skip? '
              'App usage alone does not establish your job or interests.') if used else (
              'App-use history is unavailable. Tell me how you use this Mac, or skip for now.')
    return {'scanned_at': scanned_at, 'summary': usage_summary(rows), 'reason': reason,
            'used_apps': used, 'apps': len(rows), 'evidence': evidence}


async def discover_persona(home: Any, device_id: str, activity: list[dict[str, Any]],
                           excluded: frozenset[str]) -> dict[str, Any]:
    """Refresh bundle inventory and read metadata only; never launch an app or call a model."""
    from ferb.device.profile import ProfileStore
    from ferb.device.scan import scan
    profile = scan(device_id)
    ProfileStore(home).save(profile, 'setup persona scan')
    rows = await app_usage_async([a for a in profile.apps if a.bundle_id not in excluded])
    return persona_proposal(with_activity(rows, activity), profile.scanned_at)


def confirm_persona(proposal: str, answer: str) -> str | None:
    if not answer.strip() or answer.strip().lower() in {"skip", "no"}:
        return None
    return proposal if answer.strip().lower() in {"yes", "y"} else answer.strip()


def setup_persona(env: Any, config: Any, profile: Any, client: Any,
                  usage: Callable[[list[Any]], list[dict[str, Any]]] = app_usage) -> None:
    state = PersonaState(config)
    if state.reviewed():
        return
    from ferb.sanitize.redact import Redactor
    redact = Redactor()
    proposal = redact({"text": usage_summary(usage(profile.apps))})["text"]
    env.say("     " + proposal)
    answer = env.ask("     Does that describe you? yes, edit in your own words, or skip: ")
    confirmed = confirm_persona(proposal, answer)
    if confirmed is not None:
        confirmed = redact({"text": confirmed})["text"]
    if confirmed is not None:
        # Stored as an explicit owner preference through the same CLI memory path, not an inferred identity edit.
        client.call("memory.add", {"content": confirmed, "kind": "preference", "key": PERSONA_KEY})
    state.set_reviewed(True)
