"""FERB's records of its own state for the planner (ORCH-577, ORCH-578; owner 2026-11-08: "ok, what
about other things ferb shdol tell and not just see in its plan or memroy??"): the planner's live facts list the pending
reminders and timers the way the schedules are listed, and FERB's self-description (about.read) carries the permissions
in force and the saved logins by name. No card is drawn from them (owner 2026-11-10, APP-823). Must not read a model's
words to build a row, show a secret's value, or change anything."""

import json
from datetime import datetime
from typing import Any


class Records:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    # ---------- reminders and timers ----------

    def _reminder_rows(self) -> list[dict[str, Any]]:
        """The owner's pending reminders and timers, as FERB's timer rows hold them, soonest first."""
        if self.timers is None:
            return []
        rows = sorted(self.timers.pending(("reminder", "timer")), key=lambda t: t.fire_at)
        return [{"id": t.id, "kind": t.kind, "name": t.label, "message": t.message, "at": t.fire_at.isoformat()}
                for t in rows]

    def _reminder_facts(self) -> list[str]:
        """The planner's list of them (ORCH-577, as `_schedule_facts`): a count by kind, then one line each."""
        rows = self._reminder_rows()
        kinds = {kind: sum(r["kind"] == kind for r in rows) for kind in ("reminder", "timer")}
        count = (f"{len(rows)} (" + ", ".join(f"{n} {kind}" + ("s" if n != 1 else "") for kind, n in kinds.items() if n)
                 + ")") if rows else "none"
        lines = [f"The owner's pending reminders and timers, from FERB's records (the truth: answer anything about "
                 f"them only from this list, never from the conversation): {count}"]
        lines += [f"- {r['kind'].capitalize()} {r['id']} “{r['name']}”: {r['message']} — "
                  f"{datetime.fromisoformat(r['at']).astimezone():%a %d %b %Y %H:%M}" for r in rows]
        return lines

    # ---------- permissions, logins, about (ORCH-578) ----------

    def _permissions_in_force(self) -> dict[str, Any]:
        """What lets FERB act without asking, from its records: the standing send scopes in force (a task's or a
        schedule's), the destinations the owner named or approved, and the "don't ask for 30 minutes" window's end."""
        db = self.db
        assert db is not None
        scopes = []
        for row in db.conn.execute("SELECT task_id, scope FROM standing_permissions WHERE active = 1 ORDER BY rowid"):
            scope = json.loads(row[1])
            scopes.append({"held_by": str(row[0]), "capability": scope.get("capability"),
                           "identity": scope.get("identity"), "account": scope.get("account"),
                           "site": scope.get("site"), "recipients": scope.get("recipients") or [],
                           "any_text": scope.get("content") is None})
        known = [{"kind": r[0], "value": "(kept only as a fingerprint)" if str(r[1]).startswith("sha256:") else r[1],
                  "how": r[2], "approved": r[2] in ("answered", "confirmed")}
                 for r in db.conn.execute("SELECT kind, value, how FROM known_destinations ORDER BY at")]
        until = getattr(self.gateway, "allow_until", None) if self.gateway is not None else None
        window = until.isoformat() if until is not None and self.clock.now() < until else None
        return {"standing_sends": scopes, "known_destinations": known, "routine_questions_off_until": window}

    def _saved_logins(self) -> dict[str, Any]:
        """The logins FERB can fill, by name and site only (never a value or a username): the owner's saved entries and
        FERB's own secrets."""
        try:
            owner = self._owner_secrets().inventory()
        except Exception as e:                        # the handles file can't be read: say so, never guess
            owner = [{"problem": f"the saved logins couldn't be read ({type(e).__name__})"}]
        mine = [{"name": s.ref.key, "account": s.account_hint, "sites": list(s.allowed_origins)}
                for s in self.secrets.specs()]
        return {"owner_saved": owner, "ferb_own": mine}
