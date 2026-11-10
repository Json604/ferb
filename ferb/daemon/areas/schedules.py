"""Own durable timers' delivery, reminders and schedules (creating, changing and running them). Must not widen a
schedule's recorded scope."""

import asyncio
import json
import os
from datetime import datetime, timedelta
from typing import Any

from ferb.act.fs.files import normalize_path
from ferb.core import origin as origin_
from ferb.core.errors import FerbError
from ferb.daemon.server import Session
from ferb.orchestrator.answer_marks import headline
from ferb.orchestrator.schedules import WATCH_SAID, until_said
from ferb.orchestrator.worker import reason


SCHEDULE_RETRY = "retry:"            # a schedule timer's message for a failed run's one retry


SCHEDULE_RETRY_S = 600.0             # 10 minutes after the failure (owner 2026-10-27)
LAST_LINE = 200                      # ORCH-592: a schedule's last answer's first line in the planner's list, at most


class Schedules:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _build_timers(self) -> None:
        """Start the timer loop and the held-banner flush (the timers and schedules were built with the stores), after
        every active schedule has its next run booked (`_reconcile_schedules`). ORCH-317 (clarified 2026-10-31):
        timers held for the owner follow the Mac's presence signal (wake, unlock, login: `MacPresence`, never input
        idleness); its return wakes the timers and brings the day's paused-task mentions (ORCH-312)."""
        from datetime import datetime as _dt
        mac = self.presence
        self.timers.owner_away = mac.away
        # when the current stretch of use began; None when FERB can't tell (not watching): unknown counts as present
        self.timers.present_since = lambda: (None if not mac.running else _dt.fromtimestamp(
            mac.present_since if mac.present_since is not None and not mac.away() else 0).astimezone())
        loop = asyncio.get_running_loop()
        mac.on_return = lambda signal: loop.call_soon_threadsafe(self._owner_back, signal)
        self._reconcile_schedules()
        self._timer_task = asyncio.get_running_loop().create_task(self.timers.run())
        self._held_task = asyncio.get_running_loop().create_task(self._supervise("held_banners", self._flush_held))

    def _record_start(self) -> bool:
        """Journal `daemon.started` and say whether this start is in a new login session (SENSE-201, SENSE-407). The
        session's digest is kept under `mac_login`, compared as stored with the previous start's: the old key,
        `session`, names a cookie to the redactor, so it was always "[redacted]" and every start counted as a login
        (live 2026-11-01..03: 18 false returns firing held reminders). A previous start without the key (an older
        build) can't be compared: no return; no start before at all: a return."""
        login = self._login_session()
        previous = self.journal.recent_values("daemon.started", "mac_login", 1)
        entry = self.journal.append("daemon.started", {"pid": os.getpid()} | ({"mac_login": login} if login else {}))
        stored = entry.data.get("mac_login")
        return stored is not None and previous != [None] and previous != [stored]

    @staticmethod
    def _login_session() -> str | None:
        """A digest of this login session's unique id (CoreGraphics; a restart or a new login gives a new one), kept
        in `daemon.started`; None when it can't be read. Not a secret: it only tells one login session from another."""
        import hashlib
        from ferb.sense.watchers.presence import session
        ident = session().get("id")
        return hashlib.sha256(str(ident).encode()).hexdigest()[:16] if ident else None

    def _mac_started(self, new_session: bool) -> None:
        """ORCH-317: the daemon starting in a new login session (after a restart or a login) is the Mac coming back
        into use, a return, unless the Mac is out of use right now (its unlock is the return then). A start in the same
        session (an update, a crash) is no return: the current stretch began at the last return recorded."""
        mac = self.presence
        if not mac.running:
            return
        if new_session:
            if not mac.away():
                mac.returned("login")
            return
        row = self.db.conn.execute("SELECT at FROM journal WHERE kind = 'mac.back_in_use' ORDER BY seq DESC "
                                   "LIMIT 1").fetchone() if self.db is not None else None
        if row is not None and mac.present_since is None:
            mac.present_since = datetime.fromisoformat(row[0]).timestamp()

    def _reconcile_schedules(self) -> None:
        """At start-up, every active schedule has a pending timer for its next run. A crash between marking a schedule's
        timer handled and booking its next run (`_run_schedule` books it before the run starts, so that run never
        started either) left one without: its due run is booked again now (at its recorded time, so it runs as
        overdue), or the next one after now."""
        from ferb.orchestrator.schedules import ScheduleError
        assert self.db is not None
        for s in self.schedules.all(("active",)):
            timer = self.timers.get(s.timer_id) if s.timer_id else None
            handled = timer is not None and self.timers.delivered(timer.id)
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
            booked = self._book(s, due)
            self.schedules.set_timer(s.id, booked.id, due)
            self._journal().append("schedule.reconciled", {"schedule_id": s.id, "at": due.isoformat()})

    def _book(self, s: Any, due: datetime, message: str = "", return_after: datetime | None = None) -> Any:
        """A timer for one of a schedule's runs (`message`: its id, or a retry's), held for the owner when the schedule
        waits for them (ORCH-317): the one place every run of a schedule is booked."""
        return self.timers.add("schedule", due, message or s.id, when_active=s.rule.when_active,
                               return_after=return_after)

    def _owner_back(self, signal: str = "") -> None:
        """The Mac came back into use (ORCH-317: woken, unlocked, logged in or restarted; `signal` says which):
        what was held for the owner fires now, and paused tasks get their mention of the day (ORCH-312)."""
        if self._shutting_down:
            return
        self._journal().append("mac.back_in_use", {"signal": signal})
        if self.timers is not None:
            self.timers.wake()
        if self.supervisor is not None and not self._shutting_down:
            self._spawn(self.supervisor.mention_paused(), "paused-task mention")

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
        named = self.timers.get(t.id) if self.timers is not None else None      # the claimed row has no name
        name = named.label if named is not None else t.label
        self._publish("timer.fired", {"timer_id": t.id, "kind": t.kind, "message": t.message, "name": name,
                                      "overdue_by": round(t.overdue_s or 0)})
        try:
            # T-1055: the banner's heading is its short name, and its card says timer or reminder (its kind)
            await self._notify(name, f"{t.message}{late}", urgent=True, category="reminder",
                               target={"kind": "reminder", "timer_id": str(t.id)}, ident=f"reminder:{t.id}",
                               card={"type": "reminder", "title": name, "kind": t.kind, "timer_id": str(t.id),
                                     "due": t.fire_at.isoformat(),         # a reminder is for now
                                     # a timer's start: its done alert shows its whole ring and repeats its duration
                                     **({"started": named.created_at.isoformat()}
                                        if t.kind == "timer" and named is not None and named.created_at else {}),
                                     # a Repeat's: the first timer of its chain, whose one box it is (APP-769)
                                     **({"repeat_of": first} if (first := self._first_of(t.id)) else {})})
        except Exception:
            pass                                                     # chat, logs and the journal still have it

    def _first_of(self, tid: str) -> str | None:
        """The first timer of a Repeat chain this timer belongs to (timer_repeats, DMN-548), or None for a timer no
        Repeat started: the app draws one box per chain (APP-769)."""
        if self.timers is None:
            return None
        first, seen = None, {tid}
        while (earlier := self.timers.repeat_of(first or tid)) is not None and earlier not in seen:
            first = earlier
            seen.add(earlier)
        return first

    async def _sleep_until(self, worker_id: str, task_id: str, at: datetime, span_id: str | None = None) -> str:
        """time.sleep_until: a durable sleep timer; waiting on it is the owner's kind of wait (not a stall). ORCH-598:
        while it sleeps, the task's status says until when and the worker's reason (its call's `why`, as the call's
        span recorded it, redacted)."""
        assert self.timers is not None
        t = self.timers.add("sleep", at, "", task_id=task_id)
        ev = self._sleepers[t.id] = asyncio.Event()
        why = ""
        if span_id and self.db is not None:
            row = self.db.conn.execute("SELECT json_extract(input_json, '$.why') FROM spans WHERE id = ?",
                                       (span_id,)).fetchone()
            why = str(row[0] or "") if row is not None else ""
        if self.supervisor is not None:
            self.supervisor.activity.waiting(worker_id, True)
            self.supervisor.task_sleeping(task_id, at, why)
        try:
            await ev.wait()
            return str(t.id)
        finally:
            self._sleepers.pop(t.id, None)
            if self.supervisor is not None:
                self.supervisor.activity.waiting(worker_id, False)
                self.supervisor.task_sleeping(task_id, None, "")
            self.timers.cancel(t.id)

    def _schedule_create(self, spec: Any, own_words: str, request_id: str, origins: set[str],
                         request_origins: set[str] | None = None) -> tuple[str, dict[str, Any] | None]:
        """From chat: the owner's words (quote-checked), the targets recorded now, the first run in the future.
        `origins`: what the turn that wrote the scheduled request was given; its runs inherit them (S3, MP2).
        `request_origins`: what that request itself carried, its runs' request origins for the sink rule (S4, L4).
        Returns the owner's line and, for a schedule set, its event card from the stored row (step 8e)."""
        from ferb.orchestrator.schedules import RUN_SCOPE, TIMING, WATCH_SAID, ScheduleError, scope_said, until_said
        from ferb.policy.targets import Targets
        from ferb.memory.items import quoted
        from ferb.policy import blocked as blocked_
        # `own_words`: the owner's words this turn and the earlier message of theirs the supervisor found the quote in
        # (ORCH-520); a refusal here is the quote check's, said as a safety check (ORCH-519)
        if not quoted(own_words, spec.quote):
            self._journal().append("schedule.refused", {"why": "quote not in the owner's words"})
            return blocked_.blocked(blocked_.REMINDERS, blocked_.NOT_IN_WORDS, f"“{spec.request}” isn't scheduled",
                                    "type what you want scheduled and I'll set it up"), None
        ends = getattr(spec, "until", None)
        if ends is not None and not quoted(own_words, ends.quote):     # ORCH-320: the one quote check, as for both
            self._journal().append("schedule.refused", {"why": "end date words not in the owner's words"})
            return blocked_.blocked(blocked_.END_DATE, "the end date words I quoted from you aren't in what you wrote",
                                    f"“{spec.request}” isn't scheduled",
                                    "say when it should stop and I'll set it up"), None
        try:
            now = self.clock.now()
            plan = self._schedule_plan(spec, now)
            rule, first, return_after, until, timing = plan.rule, plan.first, plan.return_after, plan.until, plan.timing
            t = spec.targets
            recorded = Targets.of(
                t.apps, t.sites, [normalize_path(p, str(self.home / "workspace")) for p in t.paths],
                t.recipients)
            # D30: the standing scope stated now, with the owner here, is the only one the unattended runs get; only a
            # scope whose destinations the owner typed (now, or when stating a remembered item) is recorded (S3, MP2)
            scopes, unnamed = self._schedule_scopes(getattr(spec, "standing_sends", []), own_words, request_id)
            # ORCH-313: what every run uses, as this turn resolved it (folders normalized as the task's are)
            run_scope = spec.run_scope() if hasattr(spec, "run_scope") else None
            if run_scope is not None:
                run_scope["folders"] = list(dict.fromkeys(normalize_path(f, str(self.home / "workspace"))
                                                          for f in run_scope["folders"] if f.strip()))
            s = self.schedules.add(spec.request, own_words,
                                   recorded.to_json() | ({"standing_sends": scopes} if scopes else {})
                                   | ({RUN_SCOPE: self.redact(run_scope)} if run_scope is not None else {})
                                   | ({TIMING: timing.to_json()} if timing is not None else {}), rule,
                                   first, spec.watch, until, name=str(getattr(spec, "name", "") or ""))
        except (ScheduleError, ValueError) as e:
            return f"I couldn't schedule that: {e}.", None
        first = plan.booked                 # T-1066: its first run, its timing's earlier times included
        t = self._book(s, first, return_after=return_after)
        self.schedules.set_timer(s.id, t.id, first)
        # the statement the owner is shown: each run's grant (`schedule:<id>`) links back to these recorded scopes
        self._journal().append("schedule.created", {"schedule_id": s.id, "request": s.request, "rule": s.when_said(),
                                                    "request_id": request_id, "standing_sends": scopes,
                                                    "not_recorded": unnamed, "watch": s.watch,
                                                    "until": s.until.isoformat() if s.until else None,
                                                    RUN_SCOPE: s.run_scope}
                               | ({"request_origins": sorted(request_origins)} if request_origins else {}),
                               origin=origin_.join(origins))
        # The owner's line in plain words (L11); the schedule's id stays in the journal row above
        local = first.astimezone()
        at = f"{local:%a %d %b}" if rule.when_active and (local.hour, local.minute) == (0, 0) else \
            f"{local:%a %d %b %H:%M}"                    # ORCH-317: due from a day's start, held for the owner
        if rule.repeat != "none" or s.timing is not None:
            when = f"{s.when_said()}, first on {at}"
        elif not rule.when_active:
            when = f"on {at}"
        elif return_after is not None and return_after <= now + timedelta(seconds=1):
            when = "the next time you're back at your Mac"
        else:
            when = f"on {at}, when you're at your Mac"
        said = ", ".join(part for part in (f"🗓 Scheduled: {s.label} — {when}", until_said(s.until) if s.until else "",
                                           WATCH_SAID[s.watch]) if part)
        from ferb.orchestrator import cards
        card = cards.event("schedule", s.id, s.label, first.isoformat(), said,
                           repeat=s.when_said() if rule.repeat != "none" or s.timing else None,
                           watch=WATCH_SAID[s.watch] or None,
                           until=s.until.isoformat() if s.until else None)
        from ferb.orchestrator.plans import SendScope
        from ferb.policy import confirm
        if s.run_scope is not None:          # ORCH-313: stated now, so the owner can correct it
            shown = dict(s.run_scope)
            if shown.get("identity") == "ferb" and self._identity_profile() is None:
                # APP-777: with no profile of its own, its runs browse as the owner in their pick: said as that
                chosen = self._owner_browsing_profile()
                if chosen is not None and self._browsing(False, request_id).get("as_owner"):
                    from ferb.act.browser.profiles import named
                    shown |= {"identity": "owner", "profile": named(self._browsing_profiles(), *chosen).display_name}
            said += "\n" + confirm.from_task_lines(scope_said(shown))
        if scopes:
            said += "\n" + confirm.from_task_lines("Each run will send these without asking each time:\n" + "\n".join(
                "- " + SendScope.model_validate(scope).line() for scope in scopes))
        if unnamed:
            said += "\n" + confirm.from_task_lines(
                "These weren't recorded, because their destination isn't in words you typed (now, or when you told me "
                "to remember it). In each run they get FERB's usual checks, and a send that asks can't be answered "
                "with nobody there, so it's reported not done:\n" + "\n".join("- " + line for line in unnamed))
        if plan.missing:                    # ORCH-555: its words name times it doesn't run at: kept, said plainly
            said += "\n" + self._other_times_line(plan)
        if (s.run_scope or {}).get("identity") == "ferb":
            # APP-777: its runs can't ask, so FERB asks now, while the owner is here, which of their profiles it uses
            self._browsing(True, request_id)
        return said, card

    def _schedule_plan(self, spec: Any, now: datetime) -> Any:
        """A new schedule worked out by code (T-1066: its first run, end and several daily times), its random
        times seeded afresh."""
        from ferb.orchestrator.schedules import plan_new
        return plan_new(spec, now, self.ids.new_id())

    @staticmethod
    def _other_times_line(plan: Any) -> str:
        """ORCH-555 (owner: "Keep it, say it plainly"): a schedule whose words still name a time it never runs at, after
        the planner's one repair, is kept; code's line says when it runs and that those times in its instructions
        aren't run times."""
        from ferb.orchestrator.schedules import said_list
        return (f"It runs {plan.schedule.when_said()}; {said_list(plan.missing)} in its instructions "
                f"{'is' if len(plan.missing) == 1 else 'are'} not a time it runs.")

    def _schedule_problems(self, entry: Any) -> list[str]:
        """What code finds wrong with a new or changed schedule before the planner's reply stands (T-1066), told back
        for repair: timing code can't work out, or a clock time its request names that it would never run at."""
        from ferb.orchestrator.schedules import ScheduleError, plan_edit
        now = self.clock.now()
        new = hasattr(entry, "when")
        if not new and getattr(entry, "action", "") != "edit":
            return []
        what = f"schedule {entry.request[:60]!r}" if new else f"schedule_changes {entry.id!r}"
        try:
            if new:
                plan = self._schedule_plan(entry, now)
            else:
                old = self.schedules.get(str(entry.id))
                if old is None or old.state not in ("active", "paused"):
                    return [f"{what}: there's no active or paused schedule with that id to edit."]
                if not str(entry.request).strip() and not entry.timing.repeat:
                    return [f"{what}: an edit changes its request, its timing or both."]
                plan = plan_edit(old, str(entry.request), entry.timing, now, "check")
        except (ScheduleError, ValueError) as e:
            return [f"{what}: {e}."]
        if plan.missing:
            return [f"{what}: its request names {', '.join(plan.missing)}, but its saved timing would run "
                    f"{plan.schedule.when_said()}. Every clock time a schedule's request names should be a time it runs "
                    f"at (when, times, random) or its end: set the timing to every time the owner asked for. A time that "
                    f"isn't a run time stays, and FERB says it isn't one."]
        return []

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
        for entry in self._journal().by_ids(sorted(ids)):
            kind, data = entry.kind, entry.data
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
        from ferb.policy import blocked as blocked_
        if not quoted(own_words, rc.quote):
            return blocked_.blocked(blocked_.CHANGES, blocked_.NOT_IN_WORDS, "no reminder was cancelled",
                                    "say which reminder to cancel and I'll cancel it")
        t = self.timers.get(rc.id) if self.timers is not None else None
        if t is None or t.kind not in ("reminder", "timer") or not self.timers.cancel(rc.id):
            return "I couldn't find that reminder among the pending ones."
        self._journal().append("timer.cancelled", {"timer_id": rc.id, "by": "owner"})     # the id stays here (L11)
        what = "timer" if t.kind == "timer" else "reminder"
        return f"⏰ Cancelled the {what} “{t.label}” for {t.fire_at.astimezone():%a %d %b %H:%M}."

    def _schedule_change_from_chat(self, ch: Any, own_words: str) -> str:
        from ferb.memory.items import quoted
        from ferb.policy import blocked as blocked_
        if not quoted(own_words, ch.quote):
            return blocked_.blocked(blocked_.CHANGES, blocked_.NOT_IN_WORDS, "no schedule was changed",
                                    "say which schedule should change and how")
        ends = getattr(ch, "until", None)
        renamed = " ".join(str(getattr(ch, "name", "") or "").split())
        if renamed and ch.action != "cancel" and self.schedules.get(ch.id) is not None:
            # T-1055: a change keeps the schedule's name unless the planner wrote a new one in it
            self.schedules.set_name(ch.id, renamed)
            self._journal().append("schedule.renamed", {"schedule_id": ch.id, "name": renamed})
        if (ch.action == "set_end") != (ends is not None):  # argument validity: an end goes with set_end, and only there
            self._journal().append("schedule.refused", {"schedule_id": ch.id, "why": "set_end needs an end, and only "
                                                                                     "set_end takes one"})
            return "I didn't change that schedule — could you say again what should change?"
        if ch.action == "set_end":
            if not quoted(own_words, ends.quote):
                return blocked_.blocked(blocked_.END_DATE, "the end date words I quoted from you aren't in what you "
                                        "wrote", "that schedule's end didn't change", "say when it should stop")
            return self._schedule_set_end(ch.id, ends)
        if not self._changeable(ch.id, ch.action):          # the owner's line never shows the id (L11)
            return "There's no active schedule like that to change."
        return self._schedule_change(ch.id, ch.action)

    def _schedule_edit_from_chat(self, ch: Any, own_words: str, request_id: str, origins: set[str],
                                 request_origins: set[str] | None = None) -> tuple[str, dict[str, Any] | None]:
        """T-1066: an active or paused schedule changed in place, on the owner's quoted words: its instructions, its
        timing or both, with the same id, runs and history. Code works out the new timing, checks its words against
        it as for a new schedule, books its next run and says what changed (old → new) in a line and a card. What every
        run uses (ORCH-313) and its standing scope stay as recorded; `origins` and `request_origins` join the ones its
        runs already carry (the request was rewritten from this turn)."""
        from ferb.memory.items import quoted
        from ferb.orchestrator import cards
        from ferb.orchestrator.schedules import TIMING, WATCH_SAID, ScheduleError, plan_edit, until_said
        from ferb.policy import blocked as blocked_
        if not quoted(own_words, ch.quote):
            return blocked_.blocked(blocked_.CHANGES, blocked_.NOT_IN_WORDS, "no schedule was changed",
                                    "say which schedule should change and how"), None
        old = self.schedules.get(str(ch.id))
        if old is None or old.state not in ("active", "paused"):
            return "There's no active schedule like that to change.", None
        if not str(ch.request).strip() and not ch.timing.repeat:
            return "I didn't change that schedule — could you say again what should change?", None
        now = self.clock.now()
        try:
            plan = plan_edit(old, str(ch.request), ch.timing, now, self.ids.new_id())
        except (ScheduleError, ValueError) as e:
            return f"I didn't change “{old.label}”: {e}.", None
        retimed = bool(ch.timing.repeat)
        targets = None                          # a new timing replaces the old one in its recorded JSON
        if retimed:
            targets = {k: v for k, v in old.targets.items() if k != TIMING} | (
                {TIMING: plan.timing.to_json()} if plan.timing is not None else {})
        self.schedules.edit(old.id, plan.schedule.request if plan.schedule.request != old.request else None,
                            plan.rule if retimed else None, plan.first if retimed else None, targets, own_words)
        renamed = " ".join(str(getattr(ch, "name", "") or "").split())
        if renamed:
            self.schedules.set_name(old.id, renamed)
        if retimed and old.state == "active":
            if old.timer_id:
                self.timers.cancel(old.timer_id)
            booked = self._book(plan.schedule, plan.booked)
            self.schedules.set_timer(old.id, booked.id, plan.booked)
        s = self.schedules.get(old.id)
        assert s is not None
        before, after = old.when_said(), s.when_said()
        self._journal().append("schedule.edited", {"schedule_id": s.id, "request_id": request_id,
                                                   "before": {"request": old.request, "rule": before},
                                                   "after": {"request": s.request, "rule": after}}
                               | ({"request_origins": sorted(request_origins)} if request_origins else {}),
                               origin=origin_.join(origins))
        nxt = f"; next run {s.next_run.astimezone():%a %d %b %H:%M}" if s.state == "active" and s.next_run else \
            ("; it stays paused" if s.state == "paused" else "")
        timing = f"now {after} (was {before})" if after != before else f"still {after}"
        line = ", ".join(part for part in (f"🗓 Changed: {s.label} — {timing}{nxt}",
                                           until_said(s.until) if s.until else "", WATCH_SAID[s.watch]) if part)
        said = line + (f"\nIts instructions now: {s.request}" if s.request != old.request else "") + (
            "\n" + self._other_times_line(plan) if plan.missing else "")   # ORCH-555: kept, said plainly
        card = cards.event("schedule", s.id, s.label, s.next_run.isoformat() if s.next_run else "", line,
                           repeat=after, watch=WATCH_SAID[s.watch] or None,
                           until=s.until.isoformat() if s.until else None) | {
            "changed": True, **({"was_repeat": before} if before != after else {}),
            **({"was_request": old.request, "request": s.request} if s.request != old.request else {})}
        return said, card

    def _schedule_set_end(self, sid: str, ends: Any) -> str:
        """ORCH-320: an active or paused schedule gets the end the owner's words give it (code works out the time);
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
            return self._ended_line(s.label, until)
        return f"🗓 {s.label} now runs {until_said(until)}."

    @staticmethod
    def _ended_line(name: str, until: datetime) -> str:
        """The owner's line when a schedule's end leaves it no more runs (ORCH-320), by its heading (T-1055)."""
        from ferb.orchestrator.schedules import until_said
        return f"🗓 Schedule ended: {name} (it ran {until_said(until)})"

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

    def _schedule_change(self, sid: str, action: str, with_task: str = "") -> str:
        """`with_task`: a pause or resume that follows the owner's pause or resume of the schedule's run (ORCH-312)."""
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
                return f"{s.label} has no future runs, so there's nothing left to pause. {covered}"
            return f"Cancelled: {s.label}. {covered}"
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
            now = self.clock.now()
            nxt = s.next_after(max(now, s.first_run - timedelta(seconds=1))) if s.rule.repeat != "none" else \
                (s.next_run if s.next_run and s.next_run > now else s.next_after(now))    # once: a later time today
            if nxt is None:
                self.schedules.set_state(sid, "done")
                return f"{s.label} has no future runs left."
            t = self._book(s, nxt)
            self.schedules.set_timer(sid, t.id, nxt)
        self._journal().append(f"schedule.{action}", {"schedule_id": sid} | ({"with_task": with_task} if with_task
                                                                            else {}))
        return {"cancel": "Cancelled", "pause": "Paused", "resume": "Resumed"}[action] + f": {s.label}"

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
        kind = self._journal().latest_kind(("schedule.run", "schedule.pause", "schedule.met"), "schedule_id", sid)
        return kind in ("schedule.pause", "schedule.met")

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
            t = self._book(s, nxt)
            self.schedules.set_timer(sid, t.id, nxt)
        else:
            self.schedules.set_timer(sid, None, None)
            self.schedules.set_state(sid, "done")
            if s.ends_after(now):                        # ORCH-320: its end, not its rule, makes this the last run
                self._journal().append("schedule.ended", {"schedule_id": sid, "until": s.until.isoformat()
                                                          if s.until else None})
        if self.supervisor is None:
            return
        busy = sid in self._schedule_runs or self.supervisor.schedule_running(sid)
        if busy or self.state.stopped:
            why = "the previous run is still under way" if busy else "FERB was stopped"
            self._journal().append("schedule.skipped", {"schedule_id": sid, "why": why, "retry": retry})
            if nxt is None and not retry:
                ended = f"\n\n{self._ended_line(s.label, s.until)}" if s.until and s.ends_after(now) else ""
                await self._task_result({"task_id": f"{sid}:{now:%Y%m%d%H%M}", "status": "cancelled", "goal": s.request,
                                         "name": s.label,
                                         "answer": f"“{s.label}” was due, but {why}, so it didn't run.{ended}"},
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
                     "answer": f"“{s.label}” didn't work this time; I'll try again at the next run. Why: "
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
        failed, its watch's crossing told once, then the result told. ORCH-590 (owner: "Post every time"): every run
        posts its result in the owner's chat, one that found nothing included; only a watch's met crossing alerts."""
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
                self._book(s, due, SCHEDULE_RETRY + sid)
                self._journal().append("schedule.retry_queued", {"schedule_id": sid, "at": due.isoformat()})
        answer = str(final.get("answer") or final.get("summary") or "")
        # A watch (fix wave 6c): a met condition (its check judged the reported condition) is told like a reminder,
        # once per crossing: not when the latest earlier finished check that reported it found it met, an unconfirmed
        # one included (the owner was told then; stored with the schedule, re-review N1). A confirmed met run of a
        # one-shot watch ends it, as the owner's stored words for it say, and says so. A check of a one-shot watch
        # that another check ended meanwhile (M1) alerts nothing. Every check posts its result like any run (ORCH-590).
        told_before = s.last_reported_met is True            # as this run's planner was shown it
        due = now - timedelta(seconds=overdue_s)             # when this run was due (its timer's time)
        reported = final.get("watch_met")                    # recorded only by a watch's check, which judged it
        if s.watch != "none" and status in FINISHED and reported is not None:
            self.schedules.reported(sid, bool(reported))
        met = s.watch != "none" and watch_met(status, reported)
        superseded = s.watch == "until_met" and self._watch_ended(sid)
        alert = met and not told_before and not superseded
        result = {**final, "goal": s.request, "name": s.label}
        ended = (met and not superseded and s.watch == "until_met" and status == "done"
                 and self._end_watch(sid, str(final.get("task_id"))))
        if ended:
            line = f"🗓 Stopped checking: {s.label}"
            answer = f"{answer}\n\n{line}"
            self._record_stop_line(str(final.get("task_id")), line)
        # ORCH-320: the last run its end left it ends it, said once (never by a retry, which books nothing)
        expired = not ended and not retry and self._schedule_expired(sid)
        if expired:
            current = self.schedules.get(sid)
            line = self._ended_line(s.label, current.until if current is not None and current.until else now)
            answer = f"{answer}\n\n{line}"
            self._record_stop_line(str(final.get("task_id")), line)
        if overdue_s > 60:              # code's last line, after the answer's other code lines (fixed order)
            answer = f"{answer}\n(ran late)"
        cards = [self._run_card(sid, due)] + [c for c in final.get("cards") or [] if isinstance(c, dict)]
        await self._task_result(result | {"answer": answer, "cards": cards}, shown=False, alert=alert)
        self._posted(sid, s.label, final, answer)   # ORCH-588: the owner's chat shows it, so the planner's history does

    def _posted(self, sid: str, name: str, final: dict[str, Any], answer: str) -> None:
        """ORCH-588: a scheduled run's result FERB posted in the owner's chat, journaled as posted (schedule.posted)
        with its task answer's origin and stored owner values (GATE-519), so the planner's conversation holds it."""
        task_id = str(final.get("task_id") or "")
        rows = self._journal().of_kind("task.answer", "task_id", task_id) if task_id else []
        made, stored = (rows[-1].origin, rows[-1].data.get("provenance")) if rows else ("inferred", None)
        self._journal().append("schedule.posted", {"schedule_id": sid, "task_id": task_id, "name": name,
                                                   "status": str(final.get("status") or ""),
                                                   "text": self._redact_text(answer)}
                               | ({"provenance": stored} if isinstance(stored, dict) else {}), origin=made)

    def _run_card(self, sid: str, due: datetime) -> dict[str, Any]:
        """A scheduled run's reminder card (step 8e), from the schedule's row as it stands after the run: its recorded
        request, when this run was due, its rule, its next run and its end (ORCH-320: the notch counts down to the
        end when there is one, else to the next run). An end only in the request's words, never recorded, gets no
        countdown: nothing here reads the words."""
        s = self.schedules.get(sid)
        assert s is not None
        active = s.state == "active"
        return {"type": "reminder", "schedule_id": sid, "title": s.label, "kind": "schedule", "due": due.isoformat(),
                **({"repeat": s.when_said()} if s.rule.repeat != "none" or s.timing else {}),
                **({"next_run": s.next_run.isoformat()} if s.next_run is not None and active else {}),
                **({"until": s.until.isoformat()} if s.until is not None and active else {})}

    def _schedule_expired(self, sid: str) -> bool:
        """Its end left it no more runs (`schedule.ended`) and nothing restarted it since (ORCH-320)."""
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
        status = self.tasks.column(task_id, "status")
        if status is None:
            return
        outcome, line = json.loads(self.tasks.column(task_id, "outcome_json") or "{}"), self._redact_text(line)
        outcome["answer"] = f"{outcome.get('answer', '')}\n\n{line}"
        self.tasks.set_status(task_id, status, outcome)

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
        return {"schedules": [{"id": s.id, "request": s.request, "name": s.label, "rule": s.when_said(),
                               "state": s.state,
                               "next_run": s.next_run.isoformat() if s.next_run else None, "failures": s.failures,
                               "until": s.until.isoformat() if s.until else None,
                               "watch": s.watch, "last_result": self._schedule_last_result(s.id)}
                              for s in self._listed_schedules()]}

    def _schedule_rows(self) -> list[dict[str, Any]]:
        """The owner's current schedules as FERB's records hold them (T-1066, owner: "where is honesty in ferb stating
        how many schedules are there and not from jsut its memory"): each one's name, the timing code renders from its
        saved rule, its end, its watch, its state and its next run. The planner's list and the schedules card both read
        this, so what FERB says about schedules comes from here, never from the conversation."""
        rows = []
        for s in self._listed_schedules():
            rows.append({"id": s.id, "name": s.label, "repeat": s.when_said(),
                         "state": "done; its last run is under way" if s.state == "done" else s.state,
                         "next_run": s.next_run.isoformat() if s.next_run is not None and s.state == "active" else None,
                         "until": s.until.isoformat() if s.until else None, "watch": WATCH_SAID[s.watch] or None,
                         "request": s.request, "failures": s.failures})
        return rows

    def _schedule_last_runs(self) -> dict[str, dict[str, Any]]:
        """ORCH-592: each listed schedule's latest finished run as the owner was told it (the fields the app's list
        reads, `_schedule_last_result`): when, its status, its answer's first line, and that answer's recorded origin
        (its task.answer row; model-written, so `inferred` when none is recorded)."""
        if self.task_results is None:
            return {}
        out: dict[str, dict[str, Any]] = {}
        for s in self._listed_schedules():
            row = self.task_results.latest_of_schedule(s.id)
            if row is None:
                continue
            answers = self._journal().of_kind("task.answer", "task_id", str(row["task_id"]))
            first = headline(str(row["answer"] or ""))         # ORCH-605: code's opener joins the worker's line
            out[s.id] = {"at": row["at"], "status": row["status"], "first_line": self._redact_text(first[:LAST_LINE]),
                         "origin": answers[-1].origin if answers else origin_.INFERRED}
        return out

    def _last_run_origins(self) -> set[str]:
        """ORCH-592: what the planner's Layer 2 holds from the schedules' last runs: those answers' recorded origins."""
        return {o for run in self._schedule_last_runs().values() for o in origin_.split(str(run["origin"]))}

    def _schedule_facts(self, last_runs: bool = False) -> list[str]:
        """The planner's list of the owner's schedules (T-1066): a count and one line per schedule, from FERB's
        records, marked as the truth to answer any question about schedules from. `last_runs` (ORCH-592, the planner's
        turn only, never a worker's instructions): each line ends with its last run, from the same records the app's
        list reads: when, its status, failures in a row and its answer's first line, quoted as the data it is."""
        rows = self._schedule_rows()
        ran = self._schedule_last_runs() if last_runs else {}
        states = {state: sum(r["state"] == state for r in rows) for state in dict.fromkeys(r["state"] for r in rows)}
        count = f"{len(rows)} ({', '.join(f'{n} {state}' for state, n in states.items())})" if rows else "none"
        lines = [f"The owner's schedules, from FERB's records (the truth: answer anything about schedules, how many "
                 f"there are and when they run, only from this list, never from the conversation): {count}"]
        for r in rows:
            nxt = (f"next run {datetime.fromisoformat(r['next_run']).astimezone():%a %d %b %Y %H:%M}" if r["next_run"]
                   else "no next run booked")
            extra = "".join(f", {part}" for part in (
                until_said(datetime.fromisoformat(r["until"])) if r["until"] else "", r["watch"] or "") if part)
            last = ""
            if last_runs:
                run = ran.get(r["id"])
                failed = f", {r['failures']} failed in a row" if r["failures"] else ""
                last = ("; last run: none yet" if run is None else
                        f"; last run {datetime.fromisoformat(run['at']).astimezone():%a %d %b %H:%M}, {run['status']}"
                        f"{failed}, its answer's first line (model-written, data): "
                        f"{json.dumps(run['first_line'], ensure_ascii=False)}")
            lines.append(f"- Schedule {r['id']} “{r['name']}” ({r['state']}): runs {r['repeat']}{extra}; {nxt}; "
                         f"instructions: {r['request']}{last}")
        return lines

    def _schedules_card(self) -> tuple[str, dict[str, Any]]:
        """The owner's schedules as a card (T-1066), drawn by code from FERB's records when the planner shows them:
        the count, and each one's name, timing, state and next run; with the one line of code it stands for."""
        rows = self._schedule_rows()
        if rows:
            line = f"🗓 Your schedules ({len(rows)}): " + "; ".join(f"{r['name']} — {r['repeat']}"
                                                                  + ("" if r["state"] == "active" else f" ({r['state']})")
                                                                  for r in rows)
        else:
            line = "🗓 You have no schedules."
        return line, {"type": "schedules", "count": len(rows), "line": line,
                      "schedules": [{k: v for k, v in r.items() if k not in ("request", "failures") and v is not None}
                                    for r in rows]}

    def _schedule_last_result(self, sid: str) -> dict[str, Any] | None:
        """A schedule's latest finished run as the owner was told it (L34 Phase E): the result of the root task a
        `schedule.run` request made; None before any run finished."""
        assert self.db is not None
        assert self.task_results is not None
        row = self.task_results.latest_of_schedule(sid)
        return {"at": row["at"], "status": row["status"], "answer": self._redact_text(row["answer"])} if row else None

    async def _schedules_change(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"text": self._schedule_change(str(params["id"]), str(params["action"]))}

    async def _reminders_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.timers is not None
        return {"pending": [{"id": t.id, "kind": t.kind, "at": t.fire_at.isoformat(), "message": t.message,
                             "name": t.label,
                             # a timer's start (APP-762): the closed notch's live activity and its ring
                             **({"started": t.created_at.isoformat()} if t.kind == "timer" and t.created_at else {})}
                            for t in self.timers.pending()]}

    def _timer_changed(self, t: Any, state: str) -> None:
        """DMN-548: a timer was set, cancelled (a card's Cancel, chat, Settings, a schedule's change) or fired; one event
        from the store's one write path (`Timers.on_change`), so the closed notch, its cards and every client show the
        state FERB's row has. A repeat names the timer it repeats."""
        of = self.timers.repeat_of(t.id) if state == "running" and t.kind == "timer" else None
        self._publish("timer.changed", {"timer_id": t.id, "kind": t.kind, "state": state, "name": t.label,
                                        "at": t.fire_at.isoformat(), "message": t.message}
                      | ({"started": t.created_at.isoformat()} if t.kind == "timer" and t.created_at else {})
                      | ({"repeat_of": of} if of else {}))

    def _timer_row(self, t: Any) -> dict[str, Any]:
        """A timer as `timers.repeat` returns it: the pending row's fields, its shown state and the timer it repeats."""
        from ferb.clock.timers import SHOWN_STATE
        assert self.timers is not None
        of = self.timers.repeat_of(t.id)
        return {"timer": t.id, "id": t.id, "kind": t.kind, "at": t.fire_at.isoformat(), "name": t.label,
                "started": t.created_at.isoformat() if t.created_at else None, "message": t.message,
                "state": SHOWN_STATE.get(t.state, t.state)} | ({"repeat_of": of} if of else {})

    async def _timers_repeat(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The done alert's Repeat (APP-762, owner 2026-11-06): the same timer again, for the same duration, from now,
        with its name and message. The owner's own tap on a timer FERB already set: no model, no new words. A fired
        timer repeats at most once (DMN-548): a second Repeat (the notch's alert, then the chat's card) returns the timer
        the first one started; a timer still running or cancelled is refused."""
        assert self.timers is not None
        t = self.timers.get(str(params.get("id") or ""))
        if t is None or t.kind != "timer" or t.created_at is None or t.fire_at <= t.created_at:
            raise FerbError(f"no timer {params.get('id')} to repeat")
        made = self.timers.repeated_by(t.id)
        again = self.timers.get(made) if made else None
        if again is not None:
            return self._timer_row(again)
        if t.state != "delivered":
            raise FerbError("that timer was cancelled, so there's nothing to repeat" if t.state == "cancelled"
                            else "that timer is still running")
        again = self.timers.add("timer", self.clock.now() + (t.fire_at - t.created_at), t.message, name=t.name,
                                repeat_of=t.id)
        self._journal().append("timer.repeated", {"timer_id": again.id, "of": t.id, "by": "owner"})
        return self._timer_row(again)

    async def _timers_states(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """DMN-548: what FERB's rows say of the timers and reminders the app shows (`ids`): running, cancelled or fired,
        and the timer a fired one's Repeat started. On connect and when a card first shows, the app takes these, not
        its memory. An id FERB has no such row for (a sleep, a schedule's run, one gone with its records) is left out."""
        from ferb.clock.timers import SHOWN_STATE
        assert self.timers is not None
        ids = params.get("ids")
        if not isinstance(ids, list) or not all(isinstance(i, str) for i in ids):
            raise FerbError("ids must be a list of timer ids")
        out: dict[str, Any] = {}
        for tid in dict.fromkeys(ids):
            t = self.timers.get(tid)
            if t is None or t.kind not in ("reminder", "timer"):
                continue
            made = self.timers.repeated_by(t.id)
            out[t.id] = {"kind": t.kind, "state": SHOWN_STATE.get(t.state, t.state)} | (
                {"repeated_by": made} if made else {})
        return {"timers": out}

    async def _reminders_cancel(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.timers is not None
        t = self.timers.get(str(params["id"]))
        if t is None or t.kind == "sleep" or not self.timers.cancel(t.id):
            raise FerbError(f"no pending reminder {params['id']}")
        return {"cancelled": t.id}
