"""Inspect new app bundles and track subsequent TCC/launch-job facts. Never execute an installed app."""
import asyncio
import json
from pathlib import Path
from typing import Any, Callable

from ferb.sense.software import app_security_facts, associated_jobs, change_stamp, privacy_facts, login_items
from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit


class SoftwareWatcher:
    source = "software"

    def __init__(self, installed: Callable[[], list[tuple[str, str]]], redact: Callable[[Any], Any]) -> None:
        self.installed, self.redact = installed, redact

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        previous: dict[str, str] = {}
        stamp: tuple[object, ...] | None = None
        while not stop.is_set():
            # The grants and launch jobs of apps already seen are read again only when TCC or a launch-job folder
            # changed (stat calls each tick); a newly installed app is read at once
            now = await asyncio.to_thread(change_stamp)
            changed, stamp = now != stamp, now
            for bundle, path in self.installed():
                if stop.is_set():
                    break
                if path in previous and not changed:
                    continue
                if path not in previous:
                    facts = await asyncio.to_thread(lambda: self.redact(app_security_facts(Path(path), bundle)))
                    current = {key: facts[key] for key in ("privacy", "launch_jobs", "login_items")}
                    emit(Observation(self.source, facts, {"actor": "unknown"}))
                else:
                    current = await asyncio.to_thread(lambda: self.redact({"privacy": privacy_facts(bundle, Path(path)),
                            "launch_jobs": associated_jobs(Path(path)), "login_items": login_items(Path(path), bundle)}))
                fingerprint = json.dumps(current, sort_keys=True)
                if path in previous and previous[path] != fingerprint:
                    emit(Observation(self.source, self.redact({"bundle_id": bundle, "path": path,
                        "change": "permissions_or_persistence", **current}), {"actor": "unknown"}))
                previous[path] = fingerprint
            try:
                await asyncio.wait_for(stop.wait(), 1.0)
            except asyncio.TimeoutError:
                pass
