"""Own per-app observations of attempted mechanisms, including storage diagnostics.
Profiles are evidence for agents; they never choose a method or authorize a fallback.
"""

from __future__ import annotations

import json
import re
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

@dataclass
class Mechanism:
    landed: int = 0
    failed: int = 0
    streak: int = 0              # failures in a row since the last success
    attempted_at: str | None = None
    last: str | None = None      # "landed" / "failed"


@dataclass
class AppProfile:
    bundle: str
    mechanisms: dict[str, Mechanism] = field(default_factory=dict)
    updated: str = ""
    seeded_from: str | None = None       # "probe" when the probe wrote it

    def mech(self, name: str) -> Mechanism:
        return self.mechanisms.setdefault(name, Mechanism())


class Profiles:
    def __init__(self, directory: Path | None = None) -> None:
        self.dir = directory
        self._mem: dict[str, AppProfile] = {}
        self.diagnostics: dict[str, list[dict[str, str]]] = {}

    # ---- reading

    def get(self, bundle: str) -> AppProfile:
        p = self._mem.get(bundle)
        if p is None:
            p = self._load(bundle) or AppProfile(bundle)
            self._mem[bundle] = p
        return p

    def status(self, bundle: str, mechanism: str) -> str | None:
        """"landed" / "failed" (the last outcome) or None when the app was never tried with it."""
        m = self.get(bundle).mechanisms.get(mechanism)
        return m.last if m else None

    # ---- writing

    def record(self, bundle: str, mechanism: str, landed: bool) -> None:
        p = self.get(bundle)
        m = p.mech(mechanism)
        m.attempted_at = _now()
        if landed:
            m.landed, m.streak, m.last = m.landed + 1, 0, "landed"
        else:
            m.failed, m.streak, m.last = m.failed + 1, m.streak + 1, "failed"
        p.updated = _now()
        self._save(p)

    def as_dict(self, bundle: str) -> dict[str, Any]:
        p = self.get(bundle)
        return {"bundle": p.bundle, "updated": p.updated, "seeded_from": p.seeded_from,
                "mechanisms": {k: asdict(v) for k, v in p.mechanisms.items()},
                "diagnostics": self.diagnostics.get(bundle, [])}

    # ---- files

    def _path(self, bundle: str) -> Path | None:
        if self.dir is None:
            return None
        safe = re.sub(r"[^A-Za-z0-9._-]", "_", bundle)
        return self.dir / f"{safe}.json"

    def _load(self, bundle: str) -> AppProfile | None:
        path = self._path(bundle)
        if path is None or not path.exists():
            return None
        try:
            raw = json.loads(path.read_text())
            mechs = {k: Mechanism(**{f: int(v) if f in {"landed", "failed", "streak"} else v for f, v in d.items() if f in Mechanism.__dataclass_fields__})
                     for k, d in (raw.get("mechanisms") or {}).items()}
            return AppProfile(bundle, mechs, str(raw.get("updated") or ""), raw.get("seeded_from"))
        except (OSError, ValueError, TypeError) as exc:
            self.diagnostics.setdefault(bundle, []).append({"operation": "load", "path": str(path), "error": str(exc)})
            return None

    def _save(self, p: AppProfile) -> None:
        path = self._path(p.bundle)
        if path is None:
            return
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(".json.tmp")
            tmp.write_text(json.dumps(self.as_dict(p.bundle), indent=1))
            tmp.replace(path)
        except OSError as exc:
            self.diagnostics.setdefault(p.bundle, []).append({"operation": "save", "path": str(path), "error": str(exc)})


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")
