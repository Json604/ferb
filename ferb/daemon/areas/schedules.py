"""Own durable timers' delivery, reminders and schedules (creating, changing and running them). Must not widen a
schedule's recorded scope."""

import asyncio
import json
from datetime import datetime, timedelta
from typing import Any

from ferb.act.fs.files import normalize_path
from ferb.core import origin as origin_
from ferb.core.errors import FerbError
from ferb.daemon.server import Session
from ferb.orchestrator.worker import reason


SCHEDULE_RETRY = "retry:"            # a schedule timer's message for a failed run's one retry


SCHEDULE_RETRY_S = 600.0             # 10 minutes after the failure (owner 2026-10-27)


class Schedules:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _build_timers(self) -> None:
        """Start the timer loop and the held-banner flush (the timers and schedules were built with the stores), after
        every active schedule has its next run booked (`_reconcile_schedules`)."""
        self._reconcile_schedules()
        self._timer_task = asyncio.get_running_loop().create_task(self.timers.run())
        self._held_task = asyncio.get_running_loop().create_task(self._supervise("held_banners", self._flush_held))

    def _reconcile_schedules(self) -> None:
        """At start-up, every active schedule has a pending timer for its next run. A crash between marking a schedule's
        timer handled and booking its next run (`_run_schedule` books it before the run starts, so that run never
        started either) left one without: its due run is booked again now (at its recorded time, so it runs as
        overdue), or the next one after now."""
        from ferb.orchestrator.schedules import ScheduleError
        assert self.db is not None
        for s in self.schedules.all(("active",)):
            timer = self.timers.get(s.timer_id) if s.timer_id else None
            handled = timer is not None and self.db.conn.execute(
                "SELECT 1 FROM timer_deliveries WHERE timer_id = ?", (timer.id,)).fetchone() is not None
            if timer is not None and timer.state in ("pending", "claimed") and not handled:
                continue
            try:
                due = s.next_run or s.next_after(self.clock.now())
            except ScheduleError:
                due = None
            if due is None:
                self.schedules.set_timer(s.id, None, None)
                self.schedules.set_state(s.id, "done")
                if s.ends_after(self.clock.now()):
                    self._journal().append("schedule.ended", {"schedule_id": s.id, "until": s.until.isoformat()
                                                              if s.until else None})
                continue
            booked = self.timers.add("schedule", due, s.id)
            self.schedules.set_timer(s.id, booked.id, due)
            self._journal().append("schedule.reconciled", {"schedule_id": s.id, "at": due.isoformat()})

    def _timer_failed(self, timer: Any, error: Exception) -> None:
        """A timer's consumer raised (the timer stays claimed and is handed over again), or a timer check failed. Not
        while shutting down: the consumer then refuses on purpose, so the timer fires after the restart."""
        if self._shutting_down:
            return
        self._journal().append("timer.failed", {"timer_id": timer.id if timer is not None else None,
                                                "kind": timer.kind if timer is not None else None,
                                                "error": self._redact_text(f"{type(error).__name__}: {error}")})

    async def _timer_fired(self, t: Any) -> None:
        """The consumer. Sleeps wake their worker; reminders reach the owner once (notification, chat, logs,
        journal), even if a crash made the timer fire again."""
        from ferb.clock.timers import mark_handled
        if self._shutting_down:          # left claimed, not acknowledged: it fires after the restart (L34 Phase G)
            raise FerbError("FERB is shutting down; this timer fires after the restart")
        if t.kind == "sleep":
            ev = self._sleepers.get(t.id)
            if ev is not None:
                ev.set()
            return
        if t.kind == "schedule":
            from ferb.clock.timers import mark_handled as handled
            assert self.db is not None
            if handled(self.db, t.id, self.clock.now()):                 # exactly once, even after a crash
                retry = t.message.startswith(SCHEDULE_RETRY)            # a failed run's one retry (no new kind)
                sid = t.message.removeprefix(SCHEDULE_RETRY)
                run = asyncio.create_task(self._run_schedule(sid, t.overdue_s or 0, retry=retry))
                self._runs.add(run)
                run.add_done_callback(self._runs.discard)
            return
        assert self.db is not None
        c = self.db.conn
        c.execute("BEGIN")
        try:
            if not mark_handled(self.db, t.id, self.clock.now()):
                c.execute("COMMIT")
                return                                               # already delivered before a crash
            self._journal().append("timer.delivered", {"timer_id": t.id, "kind": t.kind, "message": t.message,
                                                       "overdue_s": round(t.overdue_s or 0)})
            c.execute("COMMIT")
        except Exception:
            c.execute("ROLLBACK")
            raise
        late = f" (due {round((t.overdue_s or 0) / 60)} min ago)" if (t.overdue_s or 0) >= 60 else ""
        self._publish("timer.fired", {"timer_id": t.id, "kind": t.kind, "message": t.message,
                                      "overdue_by": round(t.overdue_s or 0)})
        try:
            await self._notify("FERB reminder" if t.kind == "reminder" else "FERB timer", f"{t.message}{late}",
                               urgent=True, category="reminder", target={"kind": "reminder", "timer_id": str(t.id)},
                               ident=f"reminder:{t.id}")                       # a reminder is for now
        except Exception:
            pass                                                     # chat, logs and the journal still have it

    async def _sleep_until(self, worker_id: str, task_id: str, at: datetime) -> str:
        """time.sleep_until: a durable sleep timer; waiting on it is the owner's kind of wait (not a stall)."""
        assert self.timers is not None
        t = self.timers.add("sleep", at, "", task_id=task_id)
        ev = self._sleepers[t.id] = asyncio.Event()
        if self.supervisor is not None:
            self.supervisor.activity.waiting(worker_id, True)
        try:
            await ev.wait()
            return str(t.id)
        finally:
            self._sleepers.pop(t.id, None)
            if self.supervisor is not None:
                self.supervisor.activity.waiting(worker_id, False)
            self.timers.cancel(t.id)

    def _schedule_create(self, spec: Any, own_words: str, request_id: str, origins: set[str],
                         request_origins: set[str] | None = None) -> tuple[str, dict[str, Any] | None]:
        """From chat: the owner's words (quote-checked), the targets recorded now, the first run in the future.
        `origins`: what the turn that wrote the scheduled request was given; its runs inherit them (S3, MP2).
        `request_origins`: what that request itself carried, its runs' request origins for the sink rule (S4, L4).
        Returns the owner's line and, for a schedule set, its event card from the stored row (step 8e)."""
        from ferb.orchestrator.schedules import WATCH_SAID, Rule, ScheduleError, until_said
        from ferb.policy.targets import Targets
        from ferb.memory.items import quoted
        if not quoted(own_words, spec.quote):
            self._journal().append("schedule.refused", {"why": "quote not in the owner's words"})
            return "I couldn't set that up — could you ask me again in your own words?", None
        ends = getattr(spec, "until", None)
        if ends is not None and not quoted(own_words, ends.quote):     # decision 22: the one quote check, as for both
            self._journal().append("schedule.refused", {"why": "end date words not in the owner's words"})
            return ("I couldn't set that up: the end date words I quoted from you aren't in what you typed — could you "
                    "say when it should stop?"), None
        try:
            from ferb.clock.when import end_at, resolve
            now = self.clock.now()
            first = resolve(spec.when, now)                                     # code computes the date, not the model
            until = end_at(ends.when(), now) if ends is not None else None      # and the end (decision 22)
            rule = Rule.build(spec.repeat, first, spec.weekday, spec.every_minutes)
            t = spec.targets
            recorded = Targets.of(
                t.apps, t.sites, [normalize_path(p, str(self.home / "workspace")) for p in t.paths],
                t.recipients)
            # D30: the standing scope stated now, with the owner here, is the only one the unattended runs get; only a
            # scope whose destinations the owner typed (now, or when stating a remembered item) is recorded (S3, MP2)
            scopes, unnamed = self._schedule_scopes(getattr(spec, "standing_sends", []), own_words, request_id)
            s = self.schedules.add(spec.request, own_words,
                                   recorded.to_json() | ({"standing_sends": scopes} if scopes else {}), rule, first,
                                   spec.watch, until)
        except (ScheduleError, ValueError) as e:
            return f"I couldn't schedule that: {e}.", None
        t = self.timers.add("schedule", first, s.id)
        self.schedules.set_timer(s.id, t.id, first)
        # the statement the owner is shown: each run's grant (`schedule:<id>`) links back to these recorded scopes
        self._journal().append("schedule.created", {"schedule_id": s.id, "request": s.request, "rule": rule.describe(),
                                                    "request_id": request_id, "standing_sends": scopes,
                                                    "not_recorded": unnamed, "watch": s.watch,
                                                    "until": s.until.isoformat() if s.until else None}
                               | ({"request_origins": sorted(request_origins)} if request_origins else {}),
                               origin=origin_.join(origins))
        # The owner's line in plain words (L11); the schedule's id stays in the journal row above
        at = f"{first.astimezone():%a %d %b %H:%M}"
        when = f"on {at}" if rule.repeat == "none" else f"{rule.describe()}, first on {at}"
        said = ", ".join(part for part in (f"🗓 Scheduled: {s.request} — {when}", until_said(s.until) if s.until else "",
                                           WATCH_SAID[s.watch]) if part)
        from ferb.orchestrator import cards
        card = cards.event("schedule", s.id, s.request, first.isoformat(), said,
                           repeat=rule.describe() if rule.repeat != "none" else None, watch=WATCH_SAID[s.watch] or None,
                           until=s.until.isoformat() if s.until else None)
        from ferb.orchestrator.plans import SendScope
        from ferb.policy import confirm
        if scopes:
            said += "\n" + confirm.from_task_lines("Each run will send these without asking each time:\n" + "\n".join(
                "- " + SendScope.model_validate(scope).line() for scope in scopes))
        if unnamed:
            said += "\n" + confirm.from_task_lines(
                "These weren't recorded, because their destination isn't in words you typed (now, or when you told me "
                "to remember it). In each run they get FERB's usual checks, and a send that asks can't be answered "
                "with nobody there, so it's reported not done:\n" + "\n".join("- " + line for line in unnamed))
        return said, card

    def _schedule_scopes(self, stated: list[Any], own_words: str,
                         request_id: str) -> tuple[list[dict[str, Any]], list[str]]:
        """The stated scopes a schedule records (redacted), and the lines of those it doesn't: a scope is recorded only
        when its site, account and recipients appear in what the owner typed, in this turn or behind a remembered item
        (`_typed_behind_memory`); never in a remembered item's wording (D30, S3, MP2)."""
        from ferb.policy.standing import owner_named
        owner_text = "\n".join([own_words, *self._typed_behind_memory(request_id)])
        recorded, unnamed = [], []
        for scope in stated:
            if owner_named(scope, owner_text):
                recorded.append(self.redact(scope.model_dump()))
            else:
                unnamed.append(self.redact(scope.line()))
        return recorded, unnamed

    def _typed_behind_memory(self, request_id: str) -> list[str]:
        """What the owner typed behind each active owner-stated memory item: the text of the `ferb memory add` / `edit`
        entry it rests on, or the owner's own words (pasted parts left out) of the chat turn it was captured in. Never
        the item's content (memory review I1 keeps chat captures to text the owner typed; this stays the narrower rule).
        The current request's turn is left out: an item captured in it adds nothing beyond its typed words (MP2). The setup
        persona is left out too: FERB built it from app names and the owner only approved it, so it never names a
        send destination (controller ruling, Task 7 x Task 18)."""
        from ferb.device.persona import PERSONA_KEY
        from ferb.memory.items import slug
        from ferb.sanitize.untrusted import recorded_own_words
        if self.memory is None:
            return []
        persona = slug(PERSONA_KEY)                      # as memory stores the key
        ids = sorted({src.removeprefix("journal:") for item in self.memory.items()
                      if item.origin == "owner_stated" and item.key != persona
                      for src in item.source_ids if src.startswith("journal:")})
        if not ids:
            return []
        assert self.db is not None
        typed = []
        for kind, data_json in self.db.conn.execute(
                f"SELECT kind, data_json FROM journal WHERE id IN ({','.join('?' * len(ids))}) ORDER BY seq", ids):
            data = json.loads(data_json)
            if kind in ("memory.added", "memory.edited"):
                typed.append(str(data.get("content", "")))
            elif kind == "chat.message" and data.get("request_id") != request_id:
                parts = data.get("untrusted_parts")      # the pasted parts it held, plus one for attached paths
                own = recorded_own_words(str(data.get("text", "")),
                                         None if parts is None else int(parts) - bool(data.get("input_paths")))
                typed += [own] if own else []
        return typed

    def _reminder_cancel_from_chat(self, rc: Any, own_words: str) -> str:
        from ferb.memory.items import quoted
        if not quoted(own_words, rc.quote):
            return "I didn't change any reminder — could you say which one in your own words?"
        t = self.timers.get(rc.id) if self.timers is not None else None
        if t is None or t.kind not in ("reminder", "timer") or not self.timers.cancel(rc.id):
            return "I couldn't find that reminder among the pending ones."
        self._journal().append("timer.cancelled", {"timer_id": rc.id, "by": "owner"})     # the id stays here (L11)
        return f"⏰ Cancelled the reminder “{t.message}” for {t.fire_at.astimezone():%a %d %b %H:%M}."

    def _schedule_change_from_chat(self, ch: Any, own_words: str) -> str:
        from ferb.memory.items import quoted
        if not quoted(own_words, ch.quote):
            return "I didn't change any schedule — could you say which one in your own words?"
        ends = getattr(ch, "until", None)
        if (ch.action == "set_end") != (ends is not None):  # argument validity: an end goes with set_end, and only there
            self._journal().append("schedule.refused", {"schedule_id": ch.id, "why": "set_end needs an end, and only "
                                                                                     "set_end takes one"})
            return "I didn't change that schedule — could you say again what should change?"
        if ch.action == "set_end":
            if not quoted(own_words, ends.quote):
                return ("I didn't change that schedule: the end date words I quoted from you aren't in what you typed "
                        "— could you say when it should stop?")
            return self._schedule_set_end(ch.id, ends)
        if not self._changeable(ch.id, ch.action):          # the owner's line never shows the id (L11)
            return "There's no active schedule like that to change."
        return self._schedule_change(ch.id, ch.action)

    def _schedule_set_end(self, sid: str, ends: Any) -> str:
        """Decision 22: an active or paused schedule gets the end the owner's words give it (code works out the time);
        a booked run after it is cancelled and the schedule ends now."""
        from ferb.clock.when import end_at
        from ferb.orchestrator.schedules import until_said
        s = self.schedules.get(sid)
        if s is None or s.state not in ("active", "paused"):
            return "There's no active schedule like that to change."
        try:
            until = end_at(ends.when(), self.clock.now())
        except ValueError as e:                                              # WhenError is a ValueError
            return f"I didn't change that schedule: {e}."
        self.schedules.set_until(sid, until)
        self._journal().append("schedule.set_end", {"schedule_id": sid, "until": until.isoformat()})
        if s.state == "active" and s.next_run is not None and s.next_run > until:
            if s.timer_id:
                self.timers.cancel(s.timer_id)
            self.schedules.set_timer(sid, None, None)
            self.schedules.set_state(sid, "done")
            self._journal().append("schedule.ended", {"schedule_id": sid, "until": until.isoformat()})
            self._withdraw_schedule_scope(sid, "ended")
            return self._ended_line(s.request, until)
        return f"🗓 {s.request} now runs {until_said(until)}."

    @staticmethod
    def _ended_line(request: str, until: datetime) -> str:
        """The owner's line when a schedule's end leaves it no more runs (decision 22)."""
        from ferb.orchestrator.schedules import until_said
        return f"🗓 Schedule ended: {request} (it ran {until_said(until)})"

    def _last_run_holds_scope(self, sid: str) -> bool:
        """Whether a one-time schedule's last run is still under way: from `_run_schedule` marking it done (its
        planner turn included, before the run's tree is granted the scope) until the run ends, or while a run holds
        the schedule's recorded scope (`schedule:<id>`, D30; a plan resumed after a restart) (final review M9)."""
        from ferb.policy.standing import SCHEDULE_GRANT, StandingPermissions
        assert self.db is not None
        if sid in self._last_runs:
            return not self._last_run_withdrawn(sid)
        standing = self.supervisor.standing if self.supervisor is not None else StandingPermissions(self.db)
        return standing.held(SCHEDULE_GRANT + sid)

    def _changeable(self, sid: str, action: str) -> bool:
        """An active or paused schedule; or a done one whose last run is under way, for a cancel or pause, which
        withdraws that run's scope (M9)."""
        s = self.schedules.get(sid)
        if s is None or s.state == "cancelled":
            return False
        return s.state != "done" or (action in ("cancel", "pause") and self._last_run_holds_scope(sid))

    def _listed_schedules(self) -> list[Any]:
        """The schedules the owner (`ferb schedules`) and the planner (its facts) can change: active and paused ones,
        and a done one whose last run is under way (M9)."""
        return self.schedules.all() + [s for s in self.schedules.all(("done",)) if self._last_run_holds_scope(s.id)]

    def _schedule_change(self, sid: str, action: str) -> str:
        s = self.schedules.get(sid)
        if s is None or not self._changeable(sid, action):
            return f"There's no active schedule {sid}."
        if s.state == "done":            # its last run is under way: the cancel or pause withdraws that run's scope
            if action == "cancel":
                self.schedules.set_state(sid, "cancelled")
            self._journal().append(f"schedule.{action}", {"schedule_id": sid})   # before the withdrawal: a grant
            self._withdraw_schedule_scope(sid, action)                           # checks it from now on
            covered = ("Its run already under way keeps going, but nothing it sends is covered any more: each send "
                       "gets FERB's usual checks.")
            if action == "pause":            # no future run to pause: it stays done, never "paused" for good
                return f"{s.request} has no future runs, so there's nothing left to pause. {covered}"
            return f"Cancelled: {s.request}. {covered}"
        if action == "cancel":
            self.schedules.set_state(sid, "cancelled")
            if s.timer_id:
                self.timers.cancel(s.timer_id)
            self._withdraw_schedule_scope(sid, action)
        elif action == "pause":
            self.schedules.set_state(sid, "paused")
            if s.timer_id:
                self.timers.cancel(s.timer_id)
            self._withdraw_schedule_scope(sid, action)
        elif action == "resume":
            self.schedules.set_state(sid, "active")
            nxt = s.next_after(max(self.clock.now(), s.first_run - timedelta(seconds=1))) \
                if s.rule.repeat != "none" else (s.next_run if s.next_run and s.next_run > self.clock.now() else None)
            if nxt is None:
                self.schedules.set_state(sid, "done")
                return f"{s.request} has no future runs left."
            t = self.timers.add("schedule", nxt, sid)
            self.schedules.set_timer(sid, t.id, nxt)
        self._journal().append(f"schedule.{action}", {"schedule_id": sid})
        return {"cancel": "Cancelled", "pause": "Paused", "resume": "Resumed"}[action] + f": {s.request}"

    def _withdraw_schedule_scope(self, sid: str, action: str) -> None:
        """D30: a cancelled or paused schedule's recorded scope stops covering its runs at once, a run in flight or one
        suspended at shutdown included (each holds it as `schedule:<id>`)."""
        from ferb.policy.standing import SCHEDULE_GRANT, StandingPermissions
        assert self.db is not None
        standing = self.supervisor.standing if self.supervisor is not None else StandingPermissions(self.db)
        tasks = standing.revoke_approval(SCHEDULE_GRANT + sid)
        if tasks:
            self._journal().append("standing.revoked", {"approval_id": SCHEDULE_GRANT + sid, "task_ids": tasks,
                                                        "reason": f"schedule {action}"})

    def _schedule_live(self, sid: str) -> bool:
        """Whether a schedule's runs may still hold its recorded scope: it exists and wasn't cancelled or paused. A
        done schedule counts (`_run_schedule` marks it done just before running its last occurrence, D30) unless the
        owner paused it during that run (a pause of a schedule with no future runs leaves it done, journaled) or it is
        a one-shot watch that ended (`schedule.met`): that withdraws its scope as a cancel does (fix wave 6c, M1)."""
        s = self.schedules.get(sid)
        return s is not None and (s.state == "active" or s.state == "done" and not self._last_run_withdrawn(sid))

    def _last_run_withdrawn(self, sid: str) -> bool:
        """A `schedule.pause` or `schedule.met` journaled after the schedule's latest run started."""
        assert self.db is not None
        row = self.db.conn.execute(
            "SELECT kind FROM journal WHERE kind IN ('schedule.run', 'schedule.pause', 'schedule.met') "
            "AND json_extract(data_json, '$.schedule_id') = ? ORDER BY seq DESC LIMIT 1", (sid,)).fetchone()
        return row is not None and row[0] in ("schedule.pause", "schedule.met")

    async def _run_schedule(self, sid: str, overdue_s: float, retry: bool = False) -> None:
        """Schedule the next occurrence and let the orchestrator assess this run from current timing/failure facts. A
        failed run retries once, SCHEDULE_RETRY_S later (owner 2026-10-27), unless the next regular run comes first;
        the retry (`retry`) leaves the regular occurrences alone and is never retried itself. A run never overlaps the
        schedule's previous one still under way (a resumed one included), and none starts while FERB is stopped: each
        is skipped, journaled; a one-time schedule's skipped run is told to the owner."""
        s = self.schedules.get(sid)
        if s is None or s.state != "active":
            return
        now = self.clock.now()
        nxt = None if retry else s.next_after(now)
        if retry:
            pass                                         # the regular next run stays as it is
        elif nxt is not None:
            t = self.timers.add("schedule", nxt, sid)
            self.schedules.set_timer(sid, t.id, nxt)
        else:
            self.schedules.set_timer(sid, None, None)
            self.schedules.set_state(sid, "done")
            if s.ends_after(now):                        # decision 22: its end, not its rule, makes this the last run
                self._journal().append("schedule.ended", {"schedule_id": sid, "until": s.until.isoformat()
                                                          if s.until else None})
        if self.supervisor is None:
            return
        busy = sid in self._schedule_runs or self.supervisor.schedule_running(sid)
        if busy or self.state.stopped:
            why = "the previous run is still under way" if busy else "FERB was stopped"
            self._journal().append("schedule.skipped", {"schedule_id": sid, "why": why, "retry": retry})
            if nxt is None and not retry:
                ended = f"\n\n{self._ended_line(s.request, s.until)}" if s.until and s.ends_after(now) else ""
                await self._task_result({"task_id": f"{sid}:{now:%Y%m%d%H%M}", "status": "cancelled", "goal": s.request,
                                         "answer": f"“{s.request}” was due, but {why}, so it didn't run.{ended}"},
                                        shown=False)
            return
        if nxt is None and not retry:
            self._last_runs.add(sid)                 # changeable (cancel, pause) from now until its run ends (M9)
        self._schedule_runs.add(sid)
        try:
            final = await self.supervisor.run_scheduled(s, late=overdue_s > 60, overdue_s=overdue_s, retry=retry)
        except Exception as e:                              # a run's failure is reported with its reason, never lost
            self._journal().append("schedule.failed", {"schedule_id": sid, "why": f"{type(e).__name__}: {e}"})
            final = {"task_id": f"{sid}:{now:%Y%m%d%H%M}", "status": "failed",
                     "answer": f"“{s.request}” didn't work this time; I'll try again at the next run. Why: "
                               f"{reason(e, self.redact)}"}
        finally:
            self._last_runs.discard(sid)
            self._schedule_runs.discard(sid)
        if final.get("suspended"):
            return                          # FERB shut down during the run: it resumes, and reports, after the restart
        await self._schedule_finished(s, final, overdue_s, retry, now)

    async def _resumed_outcome(self, event: dict[str, Any]) -> None:
        """A task resumed after a restart ended: a scheduled run's result goes through its schedule's bookkeeping (its
        failures, its retry, its watch) as a run that never stopped would; any other is told as a result."""
        held = event.get("schedule")
        s = self.schedules.get(str(held.get("id"))) if isinstance(held, dict) and self.schedules is not None else None
        if s is None or not isinstance(held, dict):
            await self._task_result(event, shown=False)
            return
        await self._schedule_finished(s, event, float(held.get("overdue_s") or 0), bool(held.get("retry")),
                                      self.clock.now())

    async def _schedule_finished(self, s: Any, final: dict[str, Any], overdue_s: float, retry: bool,
                                 now: datetime) -> None:
        """A scheduled run's result: counted (only a verified run counts as success), its one retry queued when it
        failed, its watch's crossing told once, then the result told."""
        from ferb.orchestrator.schedules import FINISHED, watch_met
        sid = s.id
        # Only a verified run counts as success. A done-but-unverified run (V4) doesn't clear earlier failures, and
        # isn't counted as one either: the next run's consecutive_failures stays a count of real failures.
        status = final.get("status")
        self.schedules.ran(sid, True if status == "done" else None if status == "done_unverified" else False)
        if status == "failed" and not retry and s.watch == "none":   # a watch already checks again on its own cadence
            regular = self.schedules.get(sid)
            due = now + timedelta(seconds=SCHEDULE_RETRY_S)
            if regular is not None and regular.state == "active" and (regular.next_run is None or regular.next_run > due):
                self.timers.add("schedule", due, SCHEDULE_RETRY + sid)
                self._journal().append("schedule.retry_queued", {"schedule_id": sid, "at": due.isoformat()})
        answer = str(final.get("answer") or final.get("summary") or "")
        # A watch (fix wave 6c): a met condition (its check judged the reported condition) is told like a reminder,
        # once per crossing: not when the latest earlier finished check that reported it found it met, an unconfirmed
        # one included (the owner was told then; stored with the schedule, re-review N1). A confirmed met run of a
        # one-shot watch ends it, as the owner's stored words for it say, and says so. A finished check with nothing
        # new to tell is recorded but reaches no client (review I3), and so does a check of a one-shot watch that
        # another check ended meanwhile (M1).
        told_before = s.last_reported_met is True            # as this run's planner was shown it
        due = now - timedelta(seconds=overdue_s)             # when this run was due (its timer's time)
        reported = final.get("watch_met")                    # recorded only by a watch's check, which judged it
        if s.watch != "none" and status in FINISHED and reported is not None:
            self.schedules.reported(sid, bool(reported))
        met = s.watch != "none" and watch_met(status, reported)
        superseded = s.watch == "until_met" and self._watch_ended(sid)
        alert = met and not told_before and not superseded
        result = {**final, "goal": s.request}
        ended = (met and not superseded and s.watch == "until_met" and status == "done"
                 and self._end_watch(sid, str(final.get("task_id"))))
        if ended:                        # decided before `quiet`: the stop line always reaches the owner
            line = f"🗓 Stopped checking: {s.request}"
            answer = f"{answer}\n\n{line}"
            self._record_stop_line(str(final.get("task_id")), line)
        # decision 22: the last run its end left it ends it, said once (never by a retry, which books nothing)
        expired = not ended and not retry and self._schedule_expired(sid)
        if expired:                      # like the stop line: always reaches the owner
            current = self.schedules.get(sid)
            line = self._ended_line(s.request, current.until if current is not None and current.until else now)
            answer = f"{answer}\n\n{line}"
            self._record_stop_line(str(final.get("task_id")), line)
        quiet = s.watch != "none" and not ended and not expired and (
            superseded or status in FINISHED and reported is not None and not alert)
        if overdue_s > 60:              # code's last line, after the answer's other code lines (fixed order)
            answer = f"{answer}\n(ran late)"
        cards = [self._run_card(sid, due)] + [c for c in final.get("cards") or [] if isinstance(c, dict)]
        await self._task_result(result | {"answer": answer, "cards": cards}, shown=quiet, alert=alert,
                                publish=not quiet)

    def _run_card(self, sid: str, due: datetime) -> dict[str, Any]:
        """A scheduled run's reminder card (step 8e), from the schedule's row as it stands after the run: its recorded
        request, when this run was due, its rule, its next run and its end (decision 22: the notch counts down to the
        end when there is one, else to the next run). An end only in the request's words, never recorded, gets no
        countdown: nothing here reads the words."""
        s = self.schedules.get(sid)
        assert s is not None
        active = s.state == "active"
        return {"type": "reminder", "schedule_id": sid, "title": s.request, "due": due.isoformat(),
                **({"repeat": s.rule.describe()} if s.rule.repeat != "none" else {}),
                **({"next_run": s.next_run.isoformat()} if s.next_run is not None and active else {}),
                **({"until": s.until.isoformat()} if s.until is not None and active else {})}

    def _schedule_expired(self, sid: str) -> bool:
        """Its end left it no more runs (`schedule.ended`) and nothing restarted it since (decision 22)."""
        s = self.schedules.get(sid)
        return s is not None and s.state == "done" and bool(self._journal().of_kind("schedule.ended", "schedule_id",
                                                                                   sid))

    def _watch_ended(self, sid: str) -> bool:
        """A one-shot watch already ended by one of its checks (`schedule.met`)."""
        return bool(self._journal().of_kind("schedule.met", "schedule_id", sid))

    def _record_stop_line(self, task_id: str, line: str) -> None:
        """The stop line in the ending task's recorded outcome as well, so a client restoring the answer from the
        task's record shows it too (review M3)."""
        assert self.db is not None
        row = self.db.conn.execute("SELECT status, outcome_json FROM tasks WHERE id = ?", (task_id,)).fetchone()
        if row is None:
            return
        outcome, line = json.loads(row["outcome_json"] or "{}"), self._redact_text(line)
        outcome["answer"] = f"{outcome.get('answer', '')}\n\n{line}"
        self.tasks.set_status(task_id, row["status"], outcome)

    def _end_watch(self, sid: str, task_id: str) -> bool:
        """A one-shot watch's confirmed run found its condition met: it ends (done, its next check cleared, journaled
        `schedule.met`, its recorded scope withdrawn as a cancel withdraws it). The stop is the owner's own words stored
        for that schedule; nothing a run reads can create or widen one. A watch the owner paused or cancelled meanwhile
        is left as they set it."""
        s = self.schedules.get(sid)
        if s is None or s.state != "active":
            return False
        if s.timer_id:
            self.timers.cancel(s.timer_id)
        self.schedules.set_timer(sid, None, None)
        self.schedules.set_state(sid, "done")
        self._journal().append("schedule.met", {"schedule_id": sid, "task_id": task_id})   # before the withdrawal:
        self._withdraw_schedule_scope(sid, "met")             # a grant checks it from now on, as for a cancel (M1)
        return True

    async def _schedules_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"schedules": [{"id": s.id, "request": s.request, "rule": s.rule.describe(), "state": s.state,
                               "next_run": s.next_run.isoformat() if s.next_run else None, "failures": s.failures,
                               "until": s.until.isoformat() if s.until else None,
                               "watch": s.watch, "last_result": self._schedule_last_result(s.id)}
                              for s in self._listed_schedules()]}

    def _schedule_last_result(self, sid: str) -> dict[str, Any] | None:
        """A schedule's latest finished run as the owner was told it (L34 Phase E): the result of the root task a
        `schedule.run` request made; None before any run finished."""
        assert self.db is not None
        row = self.db.conn.execute(
            "SELECT r.at, r.status, r.answer FROM task_results r JOIN tasks t ON t.id = r.task_id "
            "WHERE t.parent_id IS NULL AND t.request_id IN (SELECT json_extract(data_json, '$.request_id') FROM journal "
            "WHERE kind = 'schedule.run' AND json_extract(data_json, '$.schedule_id') = ?) ORDER BY r.at DESC LIMIT 1",
            (sid,)).fetchone()
        return {"at": row["at"], "status": row["status"], "answer": self._redact_text(row["answer"])} if row else None

    async def _schedules_change(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"text": self._schedule_change(str(params["id"]), str(params["action"]))}

    async def _reminders_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.timers is not None
        return {"pending": [{"id": t.id, "kind": t.kind, "at": t.fire_at.isoformat(), "message": t.message}
                            for t in self.timers.pending()]}

    async def _reminders_cancel(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.timers is not None
        t = self.timers.get(str(params["id"]))
        if t is None or t.kind == "sleep" or not self.timers.cancel(t.id):
            raise FerbError(f"no pending reminder {params['id']}")
        return {"cancelled": t.id}

    async def _reminders_unseen(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """For `ferb chat`: reminders delivered but not shown in chat yet (then marked shown)."""
        assert self.db is not None
        rows = self.db.conn.execute("SELECT id, message, overdue_s, delivered_at FROM timers WHERE state = 'delivered' "
                                    "AND kind != 'sleep' AND chat_shown = 0 ORDER BY fire_at").fetchall()
        for r in rows:
            self.db.conn.execute("UPDATE timers SET chat_shown = 1 WHERE id = ?", (r["id"],))
        return {"reminders": [{"id": r["id"], "message": self._redact_text(r["message"]),
                               "overdue_s": r["overdue_s"]} for r in rows]}
