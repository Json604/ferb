"""F7 wiring: native sources feed Trail; durable Jev decisions remain observations, never owner authority."""
import json
from datetime import timedelta
from typing import Any

from ferb.sense.batches import EventBatcher
from ferb.sense.capture import mark_clipboard_write
from ferb.sense.notifications import NotificationPolicy
from ferb.sense.trail import Observation


def _full_disk_access() -> bool:
    from ferb.device.permissions import Status, full_disk_access
    try:
        return full_disk_access() == Status.GRANTED
    except Exception:
        return False


class Perception:
    source = "jev_batches"

    def __init__(self, daemon: Any) -> None:
        self.daemon = daemon
        # macOS banners FERB posted that Notification Center hasn't shown back yet: {title, body, poster, at} (L10).
        # Each one matches one record; they live with this process, so none is kept as another copy of FERB's words.
        self.banners: list[dict[str, str]] = []
        self.policy = NotificationPolicy(daemon.db, daemon.clock, daemon.ids, daemon.redact)
        self.batches = EventBatcher(daemon.db, daemon.clock, daemon.ids, lambda: daemon._jev_active(),
                                    self.policy, daemon._publish,
                                    oldest_seconds=lambda: float(daemon.config.get("sense", "batch_age_s", 60.0)))

    def notify(self, event: Any) -> None:
        self.batches.notify(event)

    async def run(self, emit: Any, stop: Any) -> None:
        await self.batches.run(stop)

    def native_watchers(self) -> list[Any]:
        from ferb.sense.watchers.accessibility import AccessibilityWatcher
        from ferb.sense.watchers.clipboard import ClipboardWatcher
        from ferb.sense.watchers.notifications import DATABASE, NotificationDatabase, NotificationWatcher
        from ferb.sense.watchers.software import SoftwareWatcher
        from ferb.sense.watchers.text_input import TextInputWatcher
        from ferb.sense.watchers.workspace import WorkspaceWatcher
        d = self.daemon
        excluded = lambda bundle, name: d._exclusions().app(bundle, name)
        return [WorkspaceWatcher(d.redact, excluded), AccessibilityWatcher(d.redact, excluded),
                ClipboardWatcher(d.redact, excluded, report=self.clipboard_access),
                TextInputWatcher(d.owner_input, d.redact, excluded,
                                 lambda: d.protection.terminal_apps if d.protection is not None else ()),
                NotificationWatcher(NotificationDatabase(DATABASE, d.redact, excluded), self.seen, self.notification,
                                    floor=self.notification_floor, allowed=_full_disk_access),
                SoftwareWatcher(self.installed, d.redact)]

    def clipboard_access(self, state: str) -> None:
        """I3: how macOS treats FERB reading the clipboard changed (or was first seen): journaled once per change."""
        self.daemon._journal().append("clipboard.access", {"access": state})

    def seen(self, external_id: str) -> bool:
        return self.daemon.db.conn.execute("SELECT 1 FROM perception_seen WHERE source='notification' AND external_id=?",
                                           (external_id,)).fetchone() is not None

    def notification_floor(self, observations: list[Observation]) -> list[Observation]:
        """I1: the first Notification Center read after perception's install marks every record already there as seen
        (never recorded, so never sent to Jev); every later read passes through."""
        db = self.daemon.db
        row = db.conn.execute("SELECT seq FROM perception_floor WHERE source='notification'").fetchone()
        if row is None or row["seq"] is not None:
            return observations
        for observation in observations:
            db.conn.execute("INSERT OR IGNORE INTO perception_seen VALUES('notification',?,NULL)",
                            (str(observation.data["external_id"]),))
        db.conn.execute("UPDATE perception_floor SET seq=?,at=? WHERE source='notification'",
                        (len(observations), self.daemon.clock.now().isoformat()))
        return []

    def banner_posted(self, shown: dict[str, str], poster: str) -> dict[str, str]:
        """FERB is posting a macOS banner with this (redacted) title and body, under the app id `poster`."""
        banner = {"title": shown["title"], "body": shown["body"], "poster": poster,
                  "at": self.daemon.clock.now().isoformat()}
        self.banners.append(banner)
        return banner

    def banner_failed(self, banner: dict[str, str]) -> None:
        """That banner was never posted."""
        self.banners = [b for b in self.banners if b is not banner]

    def _own_banner(self, observation: Observation) -> Observation:
        """A record showing a banner FERB posted (same poster app, title and body) is FERB's (P4; any row FERB.app or its development build posted is too, L34 D), as a fact: recorded
        with actor FERB, without the body (FERB's words stay with the notice or answer they came from), and never sent
        to Jev as an outside arrival. Any other record is returned unchanged. Bundle ids compare in any case (I4)."""
        data = observation.data
        from ferb.device.install import APP_BUNDLE_ID, DEV_APP_BUNDLE_ID
        if str(data.get("bundle_id", "")).lower() in (APP_BUNDLE_ID.lower(), DEV_APP_BUNDLE_ID.lower()):
            # FERB.app posts only FERB's own banners (L34 D): every row of it is FERB's, by its poster alone, and its
            # words stay with the answer or question they came from
            kept = {k: v for k, v in data.items() if k != "body"} | {"ferb_banner": {"poster": str(data["bundle_id"])}}
            return Observation(observation.source, kept, {"actor": "ferb"})
        for banner in self.banners:
            if (banner["poster"].lower(), banner["title"], banner["body"]) == (
                    str(data.get("bundle_id", "")).lower(), data.get("title"), data.get("body")):
                self.banners.remove(banner)
                kept = {k: v for k, v in data.items() if k != "body"} | {"ferb_banner": {"posted_at": banner["at"]}}
                return Observation(observation.source, kept, {"actor": "ferb"})
        return observation

    def notification(self, observation: Observation) -> None:
        d = self.daemon
        if d.sense is not None and d.sense.paused:
            return
        external_id = str(observation.data["external_id"])
        if self.seen(external_id):
            return
        # Record first. If the daemon dies before receipt, the durable trail row itself prevents a duplicate.
        previous = d.db.conn.execute("SELECT id FROM trail WHERE source='notification' AND json_extract(data_json,'$.external_id')=?",
                                     (external_id,)).fetchone()
        event = d.trail.record(self._own_banner(observation)) if previous is None else None
        event_id = previous["id"] if previous else event.id if event is not None else None
        if event_id is not None:
            d.db.conn.execute("INSERT OR IGNORE INTO perception_seen VALUES('notification',?,?)", (external_id, event_id))

    def installed(self) -> list[tuple[str, str]]:
        rows = self.daemon.db.conn.execute("SELECT data_json FROM trail WHERE source='app_install' ORDER BY seq")
        apps = {}
        for row in rows:
            data = json.loads(row["data_json"])
            if data.get("change") == "removed":
                apps = {bundle: path for bundle, path in apps.items() if path != data.get("path")}
            elif data.get("change") == "installed" and data.get("bundle_id") and data.get("path"):
                apps[str(data["bundle_id"])] = str(data["path"])
        return list(apps.items())

    def bind_clipboard(self) -> None:
        cap = self.daemon.registry.capabilities.get("mac.clipboard")
        if cap is not None:
            cap.on_write = mark_clipboard_write

    def purge_text(self) -> int:
        """D14/P8: typed/clipboard text expires at seven days even without a daily summary. Keep source IDs. Notice
        text derived from any expired row (this pass's, or one expired before notices were purged too) expires with
        it, wherever it was copied (I3); that reconciliation is idempotent. Returns the rows this pass expired."""
        from ferb.orchestrator.attention import expire_derived
        d = self.daemon
        cutoff = (d.clock.now() - timedelta(days=7)).isoformat()
        rows = d.db.conn.execute("SELECT id,data_json FROM trail WHERE source IN ('typed_text','clipboard') AND at < ? AND json_extract(data_json,'$.content_expired') IS NULL", (cutoff,)).fetchall()
        for row in rows:
            data = json.loads(row["data_json"])
            kept = {k: data[k] for k in ("bundle_id", "app", "pid", "window_id", "field_id", "role", "change_count") if k in data}
            kept["content_expired"] = True
            d.db.conn.execute("UPDATE trail SET data_json=? WHERE id=?", (json.dumps(kept), row["id"]))
            d.daily_activity.redact_source(row["id"], kept)
        expired = {"trail:" + r["id"] for r in d.db.conn.execute(
            "SELECT id FROM trail WHERE source IN ('typed_text','clipboard') "
            "AND json_extract(data_json,'$.content_expired') IS NOT NULL")}
        if expired:
            expire_derived(d.db, expired, "This notice came from typed or copied text, which expires after 7 days.",
                           d.clock.now().isoformat())
        return len(rows)

    def status(self) -> dict[str, Any]:
        d = self.daemon
        states = {r["state"]: r["n"] for r in d.db.conn.execute("SELECT state,COUNT(*) n FROM perception_batches GROUP BY state")}
        return {"batches": states, "unsent_events": self.batches.pending_count(),
                "batch_age_s": self.batches.oldest_seconds(),
                "notification_triage": self.policy.measurements(),
                "now_playing": {"status": "pending_owner_probe", "detail": "MediaRemote access after macOS 15.4 is unverified; no fabricated playback events."}}
