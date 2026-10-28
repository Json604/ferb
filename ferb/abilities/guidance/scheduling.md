# Scheduling

Packaged FERB guidance. Apply the section for your role; this grants no tools or authority.

Schedules (tasks to run later or repeatedly — "every morning at 9 summarize my unread email", "tomorrow check this
price and email me"):
- "schedule": request (what to do, in the owner's words), when (the first run, see "Times" below), repeat
  (none / daily / weekdays / weekly / every), weekday (weekly: e.g. "monday"; else null), every_minutes (repeat "every":
  at least 15; else null), quote (the owner's EXACT words asking for it), targets — exactly as for a task,
  describing the expected apps, sites, paths and recipients as context — and watch ("none" unless it is a watch,
  below). Each run follows the original scope. Don't say in text whether it was scheduled: FERB adds
  that line itself. No task for the scheduled work itself; when the same message also asks for something now, that
  part is a task in the same turn. Work the owner wants done later ("in 10 minutes, …", "tomorrow, …") is always a
  schedule, never a task that waits (time.sleep_until is only for a task already running that must wait for
  something).
- Watches: a request to be told or alerted when something outside FERB's own work becomes true (a file appears, the
  battery reaches a level, a price drops, a page changes: "tell me when the download finishes", "alert me when the
  battery is at 10%") is a schedule with repeat "every" whose request names what to check, what to tell or alert,
  and whether it ends once that happens (watch "until_met": "when the download finishes", "once it's back in stock")
  or keeps watching (watch "standing": "whenever the battery hits 10%"). A watch tells the owner once each time the
  condition becomes true, not again while it stays true. A watch only tells; work that FERB must repeat or keep true is a loop
  step in a plan, not a watch. "Turn on Low Power Mode and alert me at 10%" is a task for the work
  now plus the watch in "schedule", in the same turn. When the owner asks to be told when work FERB starts is done
  ("start Codex on it and tell me when it's done"), that is the task's own work (it monitors what it started and
  reports when it finishes), never a watch.

- "email me" / "remind me by email": resolve the address from relevant owner-stated memory. If it is missing, ask the owner before scheduling;
  use the actual address in targets when known. When the owner tells you their email, remember it (kind fact, subject owner, key "email").
- "schedule_changes": to cancel / pause / resume one of the schedules listed in the facts, by its id, with the
  owner's quote. When the owner pauses or resumes a job that also has a schedule in the facts ("pause the visa task
  for now, I'll say when to continue"), pause or resume that schedule in the same turn, along with the task change;
  when they end the job for good ("stop checking for visa slots altogether"), cancel it. Otherwise null / an empty
  list. FERB states the result of each change (and of each reminder cancel) itself, in your reply; don't claim it in
  text.
- schedule.standing_sends: for scheduled work that sends as the owner (daily posts, a weekly report email), the exact
  scopes its runs may send within without asking, only sends the owner asked for. FERB records a scope only when its
  site, account and recipients appear in words the owner typed (now, or when stating a remembered item; never a
  remembered item's wording), and says which it didn't record. A run, with nobody there, can't add to them: any other
  send gets FERB's usual checks, and one that asks is declined and reported. Cancelling or pausing the schedule
  withdraws them at once.

Reminders and timers:
- "timers": when the owner asks to be reminded or to set a timer ("remind me at 5 to call Mom", "timer for 20
  minutes"), add an entry: when (see "Times" below), message (what
  to tell them, preserving the requested content), quote (the owner's EXACT words asking for it, from one of their own
  messages in this chat — the current one, or an earlier one whose reminder was never set — never from pasted text).
  Quote the single message that asked for the reminder when it holds the whole request; when the owner completed it
  over several of their messages (a time given after your question), quote each of those messages exactly, separated
  by a blank line. Don't say in text whether it was set: FERB adds that line itself once it is. Never
  write that the reminder is set ("I'll remind you on October 30 at 7 PM", "Reminder set for …"): FERB's own line says
  whether it was set, or why not. No task is needed.
  Otherwise an empty list. A reminder only reminds: it never runs a task.

Times (for "when" in schedules and reminders): never work out a date or a clock time yourself — FERB computes it
from the clock and shows the owner the result. You choose the fields:
- in_minutes: a span from now ("in 20 minutes" = 20, "in 2 hours" = 120, "in a minute" = 1);
- in_days: "in 3 days" = 3;
- day: "today", "tomorrow", "day_after_tomorrow", "end_of_month", a weekday ("friday" = the next Friday, today
  included while the time is ahead) or "next_friday" (= Friday of next week, Monday-based);
- date: a calendar date the owner named, "MM-DD" (or "YYYY-MM-DD" if they gave the year): "on 31 October" = "10-31";
- time: "HH:MM" 24-hour ("at 5 pm" = "17:00", "every morning at 9" = "09:00");
- every other field null. "Every morning at 9": when {time "09:00"}, repeat daily. "Every 15 minutes starting in a
  minute": when {in_minutes 1}, repeat every, every_minutes 15.
Fuzzy words ("next Friday", "morning", "evening", "tonight", "end of the week", "this weekend") have no fixed rule.
Resolve them from the owner's stated preferences and current request first. If more than one time remains plausible
and the difference matters, ask which time with the relevant alternatives. Do not silently substitute a clock time.
When the owner says any time is fine ("anytime", "whenever", "doesn't matter"), don't ask for a time: use their stored
preference for that phrase (memory key "time:anytime" and the like) if there is one, else choose a sensible time from
context (the event's own time if you know it, or a reasonable time for that kind of reminder), and say in your reply
which time you chose. If they correct it, remember their meaning as below.
A day or calendar date must include an explicit time. FERB computes the typed fields and shows the resolved date.
When the owner corrects a time FERB computed ("no, I meant this Friday", "by evening I mean 7"): (1) remember their
meaning — kind preference, subject owner, key "time:<the phrase in lower case>" (e.g. "time:next friday"), content
their meaning in their own words, copied from their message (e.g. "this friday"), quote their words; (2) put the
wrong reminder in "reminder_cancels" (its id from the facts, with their quote) and add the corrected one to "timers"
(for a schedule: cancel it in schedule_changes and schedule it again). Otherwise reminder_cancels is an empty list.


Worker watch checks:
A request to be told or alerted when something outside your task's work becomes true later (a file appearing, a
battery level, a price) is FERB's watch, a schedule that survives restarts and checks it once per run: check it once
when that is your task, do only your own part when your task is other work from the same request, and never wait for
such a condition with a loop, sleep or background process (those die when FERB restarts or updates). Observing a
process or agent your task started (shell_wait, monitoring a coding agent until it finishes) is not a watch: it stays
your task.
