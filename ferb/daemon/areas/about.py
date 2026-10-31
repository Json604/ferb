"""Own the daemon's side of FERB's self-description (decision 6: `about.get`, `about.guide`, the always-on summary, the
`about.read` tool's document) and of FERB.app's Settings → Data saved secrets (decision 8: `secrets.saved`,
`secrets.delete`). Must not change a setting, show a secret's value, or let anything but FERB.app delete one."""

import asyncio
import sqlite3
from datetime import timedelta
from typing import Any

from ferb.core.errors import FerbError, Refused
from ferb.daemon.server import Session


class About:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _models(self) -> dict[str, str]:
        """Role -> "runtime model", from config (the runtime table's defaults when unset)."""
        from ferb.runtimes.catalog import ROLES, label, model_for
        out = {}
        for role in ROLES:
            name = self._runtime_for(role)
            out["planner" if role == "orchestrator" else role] = f"{label(name)} {model_for(self.config, name)}".strip()
        return out

    def _about_summary(self) -> str:
        """The always-on line in every role's instructions (`_facts`): stable, no live numbers."""
        import ferb
        from ferb.daemon import about
        return about.summary(ferb.__version__, self._models())

    def _guide(self) -> dict[str, Any]:
        return dict(getattr(self, "_app_guide", None) or {"howtos": [], "shortcuts": [], "places": {}})

    async def _about_document(self) -> dict[str, Any]:
        """The whole self-description, redacted (FERB.app's About page and the `about.read` tool read this one)."""
        import ferb
        from ferb.daemon import settings
        from ferb.device import permissions
        from ferb.store import retention
        guide = self._guide()
        places = guide["places"]
        grants = await asyncio.to_thread(self.permission_check)
        rows = permissions.rows(grants, self._setup_containment())
        values = settings.read(self.config)
        address = str(self.config.get("mailbox", "address", "") or "")
        prof = self._identity_profile()
        owner_profile = str(self.config.get("browsing", "owner_profile", "") or "")
        doc: dict[str, Any] = {
            "version": {"ferb": ferb.__version__},
            "models": self._models(),
            "account": ({"email": address, "browser": prof[0] if prof else "", "profile": prof[1] if prof else "",
                         "paired": self._identity_paired()} if address else
                        {"email": "", "note": "FERB has no Google account of its own; features that need its own inbox "
                                              "(one-time codes for its own sign-ups, sending from its address) aren't "
                                              "set up" + (f"; it browses as the owner in {owner_profile}"
                                                          if owner_profile else "")}),
            "permissions": [{"id": r["id"], "status": r["status"]} for r in rows],
            "settings": [{"key": k, "value": v, "where": places.get(k, "FERB.app → Settings")}
                         for k, v in values.items()],
            "records": retention.describe(self.config.get),
            "health": await self._about_health(),
            "cost": self._about_cost(),
            "app": {"howtos": guide["howtos"], "shortcuts": guide["shortcuts"],
                    "where_settings_live": places or "FERB.app hasn't sent its guide since FERB started"},
        }
        return dict(self.redact(doc))

    async def _about_health(self) -> dict[str, Any]:
        """The database's size and whether its one-time clean-up waits for the owner's OK; FERB's CPU and memory over
        the last day from `resource_samples` (L30)."""
        from ferb.store import cleanup
        from ferb.store.db import store_size
        out: dict[str, Any] = {}
        if self.db is not None:
            path = self.db.path
            out["database_bytes"] = await asyncio.to_thread(store_size, path)
            out["cleanup_waiting_for_ok"] = await asyncio.to_thread(cleanup.pending, path)
            since = (self.clock.now() - timedelta(days=1)).isoformat()
            try:
                row = self.db.conn.execute(
                    "SELECT SUM(window_s), SUM(cpu_s), MAX(peak_footprint) FROM resource_samples WHERE at >= ?",
                    (since,)).fetchone()
                last = self.db.conn.execute("SELECT footprint FROM resource_samples ORDER BY id DESC LIMIT 1").fetchone()
            except sqlite3.Error:
                row, last = None, None
            if row is not None and row[0]:
                out["cpu_share_last_day"] = round(float(row[1] or 0) / float(row[0]), 4)
                out["memory_peak_last_day_bytes"] = row[2]
            if last is not None and last[0] is not None:
                out["memory_now_bytes"] = last[0]
        return out

    def _about_cost(self) -> dict[str, Any]:
        """The priced model calls in traces (API-equivalent dollars; a subscription is billed by its plan) over the
        last 7 and 30 days; traces older than their window are gone, so 30 days is the most it can say."""
        if self.db is None:
            return {}
        now = self.clock.now()
        out: dict[str, Any] = {"basis": "API-equivalent price of the model calls FERB traced"}
        for days in (7, 30):
            row = self.db.conn.execute("SELECT SUM(cost_usd) FROM spans WHERE cost_usd IS NOT NULL AND started_at >= ?",
                                       ((now - timedelta(days=days)).isoformat(),)).fetchone()
            out[f"last_{days}_days_usd"] = round(float(row[0] or 0), 2)
        return out

    async def _about_get(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings → About in FERB.app: the self-description (read-only)."""
        return await self._about_document()

    async def _about_guide(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """FERB.app hands over its how-tos, shortcuts and where each setting lives (the list kept with its screens),
        once per connection (auth.APP_ONLY: only FERB.app's words become FERB's guide). Kept in memory."""
        from ferb.daemon.about import clean_guide
        self._app_guide = clean_guide(params)
        return {"howtos": len(self._app_guide["howtos"]), "shortcuts": len(self._app_guide["shortcuts"]),
                "places": len(self._app_guide["places"])}

    # ---------- saved secrets and cards (decision 8) ----------

    def _owner_secrets(self) -> Any:
        from ferb.identity.owner_secrets import OwnerSecrets
        return OwnerSecrets(self.home)

    async def _secrets_saved(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings → Data: every saved secret and card detail by place and name (auth.APP_ONLY). Never a value."""
        try:
            return {"entries": await asyncio.to_thread(lambda: self._owner_secrets().inventory())}
        except Refused as e:
            raise FerbError(str(e)) from None

    async def _secrets_delete(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner's Delete on one saved entry, after FERB.app asked them once (auth.APP_ONLY; permanent): its
        Keychain item and its name go. Journaled by place and name only."""
        place, name = str(params.get("place") or ""), str(params.get("name") or "")
        if not place or not name:
            raise FerbError("say which saved secret: its place and its name")
        try:
            out = await asyncio.to_thread(lambda: self._owner_secrets().delete(place, name))
        except Refused as e:
            raise FerbError(str(e)) from None
        self._journal().append("secret.deleted", {"place": place, "name": name}, origin="owner")
        return dict(out)
