"""Per-app learned capability profiles (rework phase 3 / 5): which background mechanisms landed or failed in each app,
counted, updated by every real action's outcome and seedable from `ferb.device.probe`. Nothing here is written by
hand per app: the driver asks the profile, and the profile only knows what it saw.

Kept in memory and, when a directory is given, as JSON under <ferb home>/device/apps/<bundle>.json.
"""

from __future__ import annotations

import json
import re
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# mechanism names the driver records; the probe's result keys that seed them
PROBE_KEYS = {"ax_press_ok": "ax_press", "ax_set_value_ok": "ax_value", "ax_selected_text_ok": "ax_selected_text",
              "ax_focus_ok": "ax_focus", "ax_post_key_ok": "ax_keyboard", "pid_keys_ok": "pid_keys",
              "pid_keycode_ok": "pid_keycodes", "pid_mouse_click_ok": "pid_mouse", "pid_scroll_ok": "pid_scroll",
              "menu_press_ok": "menu_press", "app_hit_test_ok": "hit_test"}


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

    def prefers(self, bundle: str, mechanism: str) -> bool:
        """The profile says this mechanism works in this app (its last outcome was a success)."""
        return self.status(bundle, mechanism) == "landed"

    def untried(self, bundle: str, mechanism: str) -> bool:
        return self.status(bundle, mechanism) is None

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

    def seed(self, bundle: str, probe_result: dict[str, Any]) -> list[str]:
        """Seed from a probe's JSON profile: each known key with a True/False result counts as one landed / failed
        outcome (None = not applicable: untouched). Returns the mechanisms written."""
        p = self.get(bundle)
        written = []
        for key, mech in PROBE_KEYS.items():
            v = probe_result.get(key)
            if v is None:
                continue
            m = p.mech(mech)
            if v:
                m.landed, m.streak, m.last = m.landed + 1, 0, "landed"
            else:
                m.failed, m.streak, m.last = m.failed + 1, m.streak + 1, "failed"
            written.append(mech)
        p.updated, p.seeded_from = _now(), "probe"
        self._save(p)
        return written

    def as_dict(self, bundle: str) -> dict[str, Any]:
        p = self.get(bundle)
        return {"bundle": p.bundle, "updated": p.updated, "seeded_from": p.seeded_from,
                "mechanisms": {k: asdict(v) for k, v in p.mechanisms.items()}}

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
        except (OSError, ValueError, TypeError):
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
        except OSError:
            pass                                     # the in-memory profile still serves this daemon's life


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")
