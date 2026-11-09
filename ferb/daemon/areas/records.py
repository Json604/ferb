"""Own "FERB's records" of its own state for the planner and the owner (ORCH-563, generalizing ORCH-554's schedules;
owner 2026-11-08: "ok, what about other things ferb shdol tell and not just see in its plan or memroy??"): for each kind
the planner can show (schedules, reminders, memory, permissions, logins, about) the rows FERB keeps, a card drawn from
them and the one line of code it stands for. The planner's live facts list the reminders the same way the schedules are
listed. Must not read a model's words to build a row, show a secret's value, or change anything."""

import asyncio
import json
from datetime import datetime
from typing import Any

# A row's text in a card is cut here (a heading, never the whole stored text); the card's count is always the full one
CARD_TEXT = 200


def _cut(text: Any) -> str:
    words = " ".join(str(text or "").split())
    return words if len(words) <= CARD_TEXT else words[:CARD_TEXT - 1].rstrip() + "…"


def _when(at: datetime) -> str:
    return f"{at.astimezone():%a %d %b %H:%M}"


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
        """The planner's list of them (ORCH-563, as `_schedule_facts`): a count by kind, then one line each."""
        rows = self._reminder_rows()
        kinds = {kind: sum(r["kind"] == kind for r in rows) for kind in ("reminder", "timer")}
        count = (f"{len(rows)} (" + ", ".join(f"{n} {kind}" + ("s" if n != 1 else "") for kind, n in kinds.items() if n)
                 + ")") if rows else "none"
        lines = [f"The owner's pending reminders and timers, from FERB's records (the truth: answer anything about "
                 f"them only from this list, never from the conversation): {count}"]
        lines += [f"- {r['kind'].capitalize()} {r['id']} “{r['name']}”: {r['message']} — "
                  f"{datetime.fromisoformat(r['at']).astimezone():%a %d %b %Y %H:%M}" for r in rows]
        return lines

    def _reminders_card(self) -> tuple[str, dict[str, Any]]:
        rows = self._reminder_rows()
        line = (f"⏰ Your reminders and timers ({len(rows)}): " + "; ".join(
            f"{r['name']} — {_when(datetime.fromisoformat(r['at']))}" for r in rows)) if rows else \
            "⏰ You have no reminders or timers set."
        return line, {"type": "reminders", "count": len(rows), "line": line,
                      "reminders": [{k: v for k, v in r.items() if k != "message"} for r in rows]}

    # ---------- memory ----------

    def _memory_card(self) -> tuple[str, dict[str, Any]]:
        """What FERB's memory holds now (what memory.read lists without a query) and the forgets on record (their
        contents are never kept: only when and how many)."""
        items = self.memory.current_items() if self.memory is not None else []
        forgot = self._forgotten()
        gone = sum(f["items"] for f in forgot)
        line = (f"🧠 FERB remembers {len(items)} thing{'s' if len(items) != 1 else ''}"
                + (f"; {gone} forgotten (their contents aren't kept)" if gone else "") + ".")
        return line, self.redact({"type": "memory", "count": len(items), "line": line, "items": [
            {"id": it.id, "kind": it.kind, "subject": it.subject, "key": it.key, "text": _cut(it.content),
             "origin": it.origin} for it in items], "forgotten": forgot})

    def _forgotten(self) -> list[dict[str, Any]]:
        """The forgets on record (ORCH-567, memory.read forgotten reads the same): when, and how many items went."""
        from ferb.memory.items import forgets_on_record
        return forgets_on_record(self.db) if self.db is not None else []

    # ---------- permissions, logins, about (ORCH-565) ----------

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

    def _permissions_card(self) -> tuple[str, dict[str, Any]]:
        held = self._permissions_in_force()
        scopes, known = held["standing_sends"], held["known_destinations"]
        until = held["routine_questions_off_until"]
        line = (f"🔐 Standing permissions: {len(scopes)} send scope{'s' if len(scopes) != 1 else ''} in force; "
                f"{len(known)} known destination{'s' if len(known) != 1 else ''} "
                f"({sum(k['approved'] for k in known)} approved); routine questions "
                + (f"off until {_when(datetime.fromisoformat(until))}." if until else "on."))
        return line, self.redact({"type": "permissions", "count": len(scopes), "line": line} | held)

    async def _logins_card(self) -> tuple[str, dict[str, Any]]:
        saved = await asyncio.to_thread(self._saved_logins)
        owner = [e for e in saved["owner_saved"] if "name" in e]
        line = (f"🔑 Saved logins ({len(owner)}): " + "; ".join(f"{e['place']} — {e['name']}" for e in owner)
                if owner else "🔑 No logins are saved in FERB.")
        return line, self.redact({"type": "logins", "count": len(owner), "line": line} | saved)

    async def _about_card(self) -> tuple[str, dict[str, Any]]:
        doc = await self._about_document()
        rows = doc.get("permissions") or []
        granted = sum(r.get("status") == "granted" for r in rows)
        models = doc.get("models") or {}
        line = (f"ℹ️ FERB {doc.get('version', {}).get('ferb', '')}; "
                + "; ".join(f"{role} {model}" for role, model in models.items())
                + f"; {granted} of {len(rows)} permissions granted.")
        return line, {"type": "about", "line": line, "version": doc.get("version"), "models": models,
                      "permissions": rows}

    async def _records_card(self, kind: str) -> tuple[str, dict[str, Any]] | None:
        """ORCH-563: the card of one kind of FERB's state the planner shows (`show`), from FERB's records after this
        turn's own changes; None for a kind this FERB can't read (a store not running)."""
        if kind == "schedules":
            return self._schedules_card()
        if kind == "reminders":
            return self._reminders_card()
        if kind == "memory" and self.memory is not None:
            return self._memory_card()
        if kind == "permissions" and self.db is not None:
            return self._permissions_card()
        if kind == "logins":
            return await self._logins_card()
        if kind == "about":
            return await self._about_card()
        return None
