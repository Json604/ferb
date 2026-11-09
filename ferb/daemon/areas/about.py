"""Own the daemon's side of FERB's self-description (DMN-6: `about.get`, `about.guide`, the always-on summary, the
`about.read` tool's document), of FERB.app's Settings → Data saved secrets (APP-2: `secrets.saved`,
`secrets.delete`) and of "What FERB saw" (step 12e: `seen.list`, `seen.image`, the stored redacted screenshots). Must
not change a setting, show a secret's value, let anything but FERB.app delete one or see a screenshot, take a capture
anew, or show an image of an app or site on the owner's exclusion list."""

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
        from ferb.runtimes.catalog import ROLES, label, role_model
        out = {}
        for role in ROLES:
            name = self._runtime_for(role)
            out["planner" if role == "orchestrator" else role] = f"{label(name)} {role_model(self.config, name, role)}".strip()
        return out

    def _about_summary(self) -> str:
        """The always-on line in every role's instructions (`_facts`): stable, no live numbers."""
        import ferb
        from ferb.daemon import about
        return about.summary(ferb.__version__, self._models())

    def _guide(self) -> dict[str, Any]:
        return dict(getattr(self, "_app_guide", None) or {"howtos": [], "shortcuts": [], "places": {}, "app_settings": []})

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
        from ferb.act.browser.profiles import named, shown
        prof, owner = self._identity_profile(), self._owner_browsing_profile()
        # HAND-424: profiles by name with the account each is signed in to, read fresh (folders without Full Disk Access)
        found = await asyncio.to_thread(self._profiles) if prof or owner else []
        mine = named(found, *prof) if prof else None
        theirs = named(found, *owner) if owner else None
        doc: dict[str, Any] = {
            "version": {"ferb": ferb.__version__},
            "models": self._models(),
            "account": ({"email": address, "browser": prof[0] if prof else "", "profile": prof[1] if prof else "",
                         "profile_name": mine.display_name if mine else "", "paired": self._identity_paired()}
                        if address else
                        {"email": "", "note": "FERB has no Google account of its own; features that need its own inbox "
                                              "(one-time codes for its own sign-ups, sending from its address) aren't "
                                              "set up" + (f"; it browses as the owner in {shown(theirs)}"
                                                          if theirs else "")}
                        | ({"owner_profile": {"browser": theirs.browser, "folder": theirs.folder,
                                              "name": theirs.display_name, "email": theirs.email}} if theirs else {})),
            "permissions": [{"id": r["id"], "status": r["status"]} for r in rows],
            "settings": [{"key": k, "value": v, "where": places.get(k, "FERB.app → Settings")}
                         for k, v in values.items()],
            "records": retention.describe(self.config.get),
            "health": await self._about_health(),
            "cost": self._about_cost(),
            "app": {"howtos": guide["howtos"], "shortcuts": guide["shortcuts"],
                    "where_settings_live": places or "FERB.app hasn't sent its guide since FERB started",
                    # FERB.app's own settings as they are now (APP-728: re-sent by the app whenever one changes)
                    "settings": guide.get("app_settings") or []},
            # ORCH-578: the standing send scopes, known destinations and the ask-free window in force; the saved
            # logins by place and name only (never a value or a username)
            "permissions_in_force": self._permissions_in_force() if self.db is not None else {},
            "saved_logins": await asyncio.to_thread(self._saved_logins),
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
        """FERB.app hands over its how-tos, shortcuts, where each setting lives and its own settings with their current
        values (declared with its screens), on each connection and again when one of its settings changes (APP-728;
        auth.APP_ONLY: only FERB.app's words become FERB's guide). Kept in memory, each send replacing the last."""
        from ferb.daemon.about import clean_guide
        self._app_guide = clean_guide(params)
        return {"howtos": len(self._app_guide["howtos"]), "shortcuts": len(self._app_guide["shortcuts"]),
                "places": len(self._app_guide["places"]), "app_settings": len(self._app_guide["app_settings"])}

    # ---------- saved secrets and cards (APP-2) ----------

    def _owner_secrets(self) -> Any:
        from ferb.identity.owner_secrets import OwnerSecrets
        return OwnerSecrets(self.home)

    async def _secrets_saved(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings → Data: every saved secret and card detail by place and name (auth.APP_ONLY). Never a value."""
        try:
            return {"entries": await asyncio.to_thread(lambda: self._owner_secrets().inventory())}
        except Refused as e:
            raise FerbError(str(e)) from None

    # ---------- What FERB saw (step 12e): the stored redacted screenshots, FERB.app only ----------

    def _seen_rows(self, where: str, args: tuple[Any, ...], limit: int) -> tuple[list[dict[str, Any]], int]:
        """Stored images (never a video, never a data-only row) of the evidence window, newest first, that the owner's
        exclusion list allows today: (rows, how many in all)."""
        from ferb.store import retention
        cutoff = (self.clock.now() - timedelta(days=retention.days(
            self.config.get("retention", "evidence_days", 7)))).isoformat()
        rows = self.db.conn.execute(
            "SELECT * FROM evidence WHERE path IS NOT NULL AND mime LIKE 'image/%' AND captured_at >= ? "
            f"AND ({where}) ORDER BY captured_at DESC, rowid DESC", (cutoff, *args)).fetchall()
        shown = [r for r in (_seen_item(row) for row in rows) if not self._seen_excluded(r)]
        return shown[:limit], len(shown)

    def _seen_excluded(self, item: dict[str, Any]) -> bool:
        """An image of an app or site the owner excludes (now: the list may have changed since it was kept)."""
        from ferb.sanitize.redact import EXCLUDED
        ex = self._exclusions()
        url = item.get("url") or ""
        return (ex.app(item.get("bundle_id"), item.get("app")) or ex.url(url) or url == EXCLUDED
                or "excluded" in str(item.get("origin") or ""))

    async def _seen_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`seen.list {request_id? | task_id?, limit?}` (FERB.app only, auth.APP_ONLY): the redacted screenshots FERB
        kept, newest first, exactly as the model got them: of one message (`request_id`: the owner's front window when
        it was sent, and every image its tasks captured), of one task tree (`task_id`), or all of the evidence window
        (retention.evidence_days, 7). Facts only (time, kind, app, site, title); the bytes come from `seen.image`."""
        if self.db is None:
            raise FerbError("daemon not started")
        limit = max(1, min(int(params.get("limit") or 300), 2000))
        request, task = str(params.get("request_id") or ""), str(params.get("task_id") or "")
        if request:
            where = ("task_id = ? OR task_id IN (SELECT id FROM tasks WHERE tree_id IN "
                     "(SELECT tree_id FROM tasks WHERE request_id = ?) OR request_id = ?)")
            args: tuple[Any, ...] = (request, request, request)
        elif task:
            tree = self.tasks.tree_of(task) or task.split(".", 1)[0]
            where = ("task_id = ? OR (task_id >= ? || '.' AND task_id < ? || '/') OR "
                     "task_id IN (SELECT id FROM tasks WHERE tree_id = ?)")
            args = (tree, tree, tree, tree)
        else:
            where, args = "1", ()
        items, total = self._seen_rows(where, args, limit)
        return {"captures": items, "total": total,
                "days": int(self.config.get("retention", "evidence_days", 7) or 7)}

    async def _seen_image(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`seen.image {id}` (FERB.app only, auth.APP_ONLY): one stored redacted screenshot's bytes (base64), as the
        model got it. Only a stored image of the evidence window; never a capture anew; never an excluded app's."""
        import base64
        if self.db is None or self.evidence is None:
            raise FerbError("daemon not started")
        eid = str(params.get("id") or "")
        items, _ = self._seen_rows("id = ?", (eid,), 1)
        if not items:
            raise FerbError("that screenshot isn't kept any more, or it shows something you exclude")
        row = self.db.conn.execute("SELECT path, mime FROM evidence WHERE id = ?", (eid,)).fetchone()
        path = self.evidence.stored_file(row["path"]) if row is not None else None   # only inside the store
        try:
            image = await asyncio.to_thread(path.read_bytes) if path is not None else None
        except OSError:
            image = None
        if image is None:
            raise FerbError("that screenshot isn't kept any more")
        return {**items[0], "mime": row["mime"] or "image/jpeg", "image": base64.b64encode(image).decode("ascii")}

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


def _seen_item(row: Any) -> dict[str, Any]:
    """One stored screenshot as "What FERB saw" lists it: when, which capture, and the app or site (redacted facts)."""
    import json
    from urllib.parse import urlsplit
    try:
        data = json.loads(row["data_json"]) if row["data_json"] else {}
    except ValueError:
        data = {}
    data = data if isinstance(data, dict) else {}
    url = str(data.get("url") or "")
    site = ""
    if url.startswith(("http://", "https://")):
        try:
            site = urlsplit(url).hostname or ""
        except ValueError:
            site = ""
    title = data.get("tab_title") or data.get("title") or data.get("window") or ""
    return {"id": row["id"], "at": row["captured_at"], "kind": row["kind"], "task_id": row["task_id"],
            "app": str(data.get("app") or ""), "bundle_id": str(data.get("bundle_id") or ""), "site": site, "url": url,
            "title": str(title) if isinstance(title, str) else "", "tool": str(data.get("tool") or ""),
            "origin": row["origin"]}
