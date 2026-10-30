# Orchestration: decisions and fix list

Area: `ferb/orchestrator/` (supervisor.py, plans, plan_store, plan_executor, loops, probes, schedules, tree, worker,
verify, evidence, ledger, steering, continuity, schemas, context_layers, conversations, reply_stream), the prompts
that drive it, and its wiring in `ferb/daemon/main.py`.
Basis: a read of every line of that code (2026-10-30), plus read-only counts from the live DB. File:line refs are
to commit c6621c3. S = supervisor.py, M = main.py.

## Owner decisions (2026-10-30)

1. **Notification rules are removed** (`notification_rules`; 0 uses; depends on paused perception).
2. **Delegated conversations and answering questions in chat stay** (not selected for removal).
3. **The checker and the dispatcher merge into one role**: it judges the conditions and chooses what's next.
4. **The planner gets recent conversation plus the running summary**, not history filling the model's window.
   (Owner, 2026-10-30, "Recent + summary": the last **20 exchanges** word for word (20 owner messages plus FERB's
   replies to them), plus the running summary for everything older; long task reports in them in their short form.
   Owner, 2026-10-31: summary = recent (~2 weeks) + still-open only (waiting tasks, open questions, promises); older
   settled material falls out of it (memory facts and episodes hold what lasts); folds about every 20-40 exchanges,
   in the background after the reply; the planner reads only the latest summary.)
5. **Map stays as an ability; its separate code goes.** A `do` step may carry an input list; independent steps
   already run in parallel; the "every input covered exactly once" check stays in plan validation.
6. **Standing sends stay.** They are the only way to send to recipients FERB found itself (not in the owner's words,
   not already known), and the only way an unattended run can do so. Recipients in the owner's words or approved once
   never ask anyway (confirm.py:529-534; known destinations).
7. **Check-in after 5 attempts** (owner changed 10 → 5, 2026-10-30). Counted per task or plan step: worker runs in one attempt loop. Not model calls
   or tool calls, not time; loop rounds don't count (each round starts at 0). After 5 failed attempts FERB pauses and asks "keep
   going?"; Yes allows 5 more; No stops with what it found. An unattended run can't ask, so it stops at 5 and
   reports. *(Definition confirmed by the owner.)*
8. **Pause / resume is added** (owner, 2026-10-30). Pause ends the current attempt (a Codex turn can't freeze) and
   marks the task `paused`, not cancelled: no task-end clean-up (main.py:972-992 closes task tabs, releases launched
   apps, ends shells and listening), no final answer, loop state kept. Resume continues in place with the owner's
   discussion as the update. Built on the shared "wait for the owner" mechanism, not as separate machinery.
   Triggered by the planner's reading of the owner's words (no keyword list): when the owner questions, doubts or
   wants to rethink a running task, the planner pauses it rather than letting it run on, since resume loses nothing;
   a plain correction still steers live. No pause button in the app; the model's judgment is trusted (owner).
9. **"Done" is about the outcome asked for** (owner, 2026-10-30). If every outcome condition is verified, the task is
   done; an unproven side detail is removed from the answer or marked inline (fix-only-the-answer, no redo), so FERB
   never states unseen things as fact. How the work was done (background, no disturbance, right profile) is judged
   from code facts (disturbance samples, tool records), not by the model. Done-unconfirmed only when an outcome
   itself can't be verified. Every answer: the answer first, then at most one short "Couldn't confirm: …" line; no
   "Why work stopped" on finished work, no restated goal. (Example: task 810e9973, the F1 question, became
   done-unconfirmed over one uncaptured timing graphic, with the answer buried under "Why work stopped".)

10. **A new message restarts the planner turn** (owner, 2026-10-30). The planner changes nothing until it returns, so a
    message sent while it is still thinking cancels that turn and starts again with both messages; nothing waits
    behind a slow turn. (Today a per-chat lock holds later messages until the planner's first plan/reply/clarify,
    S:1142-1156.)
11. **Planner time limit 90 s** (owner, 2026-10-30; today 900 s for every non-worker call, main.py:4574). The 180 s
    silence guard (worker.py:422-445) stays for other non-worker calls; workers keep no time limit.
12. **The planner stays tool-less** (see notes/memory.md decision 10).

13. **Pause details** (owner, 2026-10-30): pausing stops the task's running terminal commands and audio listening and
    keeps only its tabs and progress; FERB mentions a paused task once a day ("Visa check is still paused. Resume?");
    pausing a scheduled run pauses that schedule's next runs.
14. **Scheduled runs and reminders use what the original request recorded** (owner, 2026-10-30): folders, recipients
    and sites, profile/account, worker type and standing scope are fixed when the schedule or reminder is made (and
    stated then, so the owner can correct them); unattended runs never re-guess them.
15. **No failure check-in across runs** (owner, 2026-10-30): each failed run already reports in chat, and the visa
    schedule recovered by itself after 4 failures (13 of the next 15 runs succeeded).
16. **AI service down → one-tap switch** (owner, 2026-10-30): when the configured runtime reports a usage limit or a
    sign-in failure, FERB says so once and offers another signed-in runtime with one tap; it never switches by itself
    (billing). Today it only shows the raw error (worker.py:66).

17. **"When you're back at your Mac" trigger** (owner, 2026-10-30): reminders and schedules can fire when the owner
    becomes active at the Mac (idle → active / wake), instead of polling; a scheduled run that did nothing posts nothing
    (as watches already do). (Evidence: "remind me everyday whenever I'm on my mac" became a run every 15 minutes, 14
    runs and "nothing to report" messages on 10-30.)
18. **Restart rule kept, narrow** (decision 10 clarified, 2026-10-30): it applies only to messages sent before FERB's
    first reply to the previous one; FERB restarts once and reads both (related → folded in, unrelated → both answered);
    later messages wait their turn. Once FERB has replied, a running task is unaffected and new messages are normal
    turns. **Code-enforced**: at most one restart per turn (a counter, not a prompt), and the 90 s planner limit
    covers the turn including its restart, so a burst of messages can't stall FERB. Messages that queued while a turn
    was running are read together in the next turn (4 quick messages → 2 turns, not 4).

19. **Answers in steps** (owner approved the mock, 2026-10-30: https://claude.ai/artifact/LpxdUXbaXXTFCSRzWBknDj).
    A part is one of the owner's asks, not a plan step. The planner declares `parts` in the owner's order (id, quote
    checked by the shared quote check, label, and either `direct` text or one `answer_step`); code validates (each part
    one answer source; answer steps distinct; wait/ask steps can't answer; every success condition belongs to exactly
    one part; no summing-up step). Tool-free parts ("who are you", general knowledge) are answered in the first reply
    with the plan line and code receipts. Each remaining part posts **once**, when its answer step's checker verdict is
    accept or accept-unconfirmed, keyed by (task, part) and recorded durably (journal `chat.part` with the redacted
    text), never twice, also across restarts. **No final summarizing call**: the code-added final worker goes (for
    multi-part and one-part multi-step plans; the last step's checked answer is the answer); after the last part code
    adds only its fixed lines and a computed "N of M done". A part that needs the owner pauses only its row; a failed
    part shows "Couldn't do: reason"; a later change to a posted part shows "updated" in the same row with one line why.
    Parts post immediately. (Why: the reverted dc11e6ba sent a duplicate in every one of its 5 tasks — parts were plan
    steps, usually the last one, plus two summary layers.)
20. **The notch gets the mock's UI** (owner, 2026-10-30): the turn card with one row per part and its states (working
    spinner, done check, asks amber, couldn't-do red, updated tag, paused violet), the done/sites footer, questions with
    buttons inside a row, the plan-line bubble style, and the typing-dots bubble (not in the notch today). Not the
    mock's pink dashed explanation labels or "code" chips. Functional UI work done here; the owner polishes.

21. **The notch opens itself; banners only when it can't** (owner, 2026-10-31): when a part needs the owner (question
    or Yes) and when the whole task finishes, the notch opens itself without taking keyboard focus or switching apps
    (as answers already do, NotchModel.swift:808/826). Rows that fill quietly only update the card (no opening, no
    banner). A macOS banner is the fallback only when the notch can't show: a full-screen app/video covers the notch
    area, the display is asleep, or FERB.app is quit; tapping it opens the notch.

## What exists today (verified)

- Core shape: planner (no tools) → worker through the gateway → observe-only checker → code writes the status from
  verdicts. Sound; stays.
- The same concern is implemented separately per path:
  - `tasks.status` written in 17 places; stop → status mapped 3 times (S:3172-3174, S:3948-3957, S:2386).
  - "Needs the owner" has 4 meanings: single task ends and waits (S:3349); loop pauses (loops.py:344, 446); ask step
    waits or blocks when unattended (S:2489-2511); plan do step is relaunched forever and the owner is never asked
    (plan_executor.py:102 skips only failed/blocked/cancelled; plan_store.py:53 stores needs_user raw).
  - Retries in 6 places; "stopped" computed 4 ways; `task.finished` published per worker/checker/step run plus the tree.
  - `_send` (S:1284-1569): 8 exits, 4 reply recorders, 9 hand-placed `_env.pop`s; `_env` holds ~24 untyped keys.
  - 42 hooks set on the supervisor after construction (M:909-944, 1404, 1549-1571, 2850-2855); supervisor writes
    gateway attributes back (S:546, 549, 638).
  - 7 quote-check implementations (remember, until, timers, schedules, forget, conversation changes, notification rules).
  - ~110 special-case branches in code, ~25 special rules in supervisor.md.
- Never used live: step kinds map/wait/ask/converse (stored plans: do 77, reduce 20, loop 3); `reduce` has no code
  of its own; `answer_question` 0 of 360 turns; forget, reminder_cancels, conversation_changes,
  notification_rules, standing_sends (`standing_permissions`: 0 rows), procedures 0; `steer` can't be produced
  (prompt says steer, schema enum is stop/retry/continue, schemas.py:115).
- Planner prompt size (latest planner turn): ~342k chars, ~95k tokens. System prompt 55k chars; conversation 158k
  (summary 5.5k; the owner's 27 messages 12k; FERB's 26 replies ~140k, task reports ~5-8k chars each); retrieved
  memory 59k; today's activity 44k; the rest ~25k; the owner's new message 353 chars.
- Prompt caching: 125 of 398 planner calls since 10-20 had any cache hit; average cached 2.3k of 98k input tokens.
- Attempt loop: up to 4 model roles judge "done" (worker, checker, dispatcher, resource reviewer). The checker
  rejects 24-28% of worker "done" reports; 31% of rejections fail only the report-truthfulness condition and cost a
  full worker redo. Worker runs per task: 1×320, 2×73, 3×9, 4×8, 5×3, 6×1, 9×1. WORKER_SCHEMA requires all 11
  fields every run (schemas.py:30-33, 138-151); code treats failed/blocked/needs_user the same (S:3309-3312).

## Target design

1. **One execution path.** A single task is a one-step plan. Every step is a worker step with dependencies;
   `reduce` folds into `do`. Step kinds left: `do` (optional input list; optional repeat rule = loop), `wait`,
   `ask`, `converse`. A loop is a step with a repeat rule (probe trip / event / steer / backup timer; ends on a
   verified condition or stop) on the same path.
2. **One status function, one finish function, one stop check, one injected clock** for tree, step, loop round and
   conversation turn. "Needs the owner" means one thing everywhere: pause in place and ask; an unattended run
   declines at once.
3. **One chat-turn path.** A typed per-turn object replaces `_env`; one exit applies side entries, records the reply
   and yields; one quote check for every side entry; one "continue/correct" verb for running and ended tasks.
4. **Attempt loop.** The checker judges and chooses next: retry with instructions, fix only the answer, ask the
   owner, accept as unconfirmed, fail. The worker reports done / not done + reason + optional question. Per-role
   schemas. Done-unverified shows the worker's answer. Conditions confirmed in an earlier attempt carry across
   retries.
5. **Planner context.** Ordered most-stable first so the provider caches the prefix: instructions → stable facts →
   running summary → recent conversation (append-only; when over budget, the oldest chunk folds into the summary) →
   memory, activity, time, front window, task state → the new message. FERB's long task reports appear in history as
   their short form; the full text stays reachable through `chat.history`.
6. **Explicit wiring.** Supervisor dependencies passed to its constructor; no hooks set afterwards; the supervisor
   doesn't write gateway attributes.

## Depends on
- notes/daemon.md: composition function and typed ports (explicit wiring, target 6); events object (owns
  `notify_plan_event`); disturbance facts for decision 9.
- notes/gateway.md and notes/hands.md: code facts for decision 9 (tool records, focused app, disturbance).
- notes/runtimes.md: provider-neutral, per-role schemas.
- notes/tests.md: the end-to-end harness hard-codes `.dispatch`/`.verify` (conftest.py:373-432); rewrite it with
  this area, before old tests go.
- notes/hands.md: tab groups use pause and wait-for-owner.

## Rulings (step 8c, technical)
- Conversation bounds (constants in supervisor.py): the planner reads the summary and every entry after what it covers,
  append-only (target design 5: consecutive turns share the cached prefix; no entry is ever in neither). A fold runs
  once that tail holds FOLD_CHUNK_TOKENS=20k beyond the last 20 exchanges (counted within a RECENT_CONVERSATION_TOKENS=15k
  safety cap), or the whole tail passes 15k + 20k; it folds the entries beyond those exchanges, a call of at most 20k
  each (live 2026-10-28: one fold read 533k tokens), so the tail drops back to about the last 20 exchanges: the planner
  sees at least 20 exchanges and at most about one chunk more. The summary is asked to stay within SUMMARY_TOKENS=2k
  and is cut to it when shown; SUMMARY_DAYS=14 is the "~2 weeks" it keeps (the fold call is told today's date and to
  keep only those days and what's still open; summary.md got that one line); a report's short form is its first
  paragraph, 300 chars.
- An exchange starts at an owner message; messages read together are one exchange.
- Restart: the turn's lead message keeps the stream; later ones end with `{"type": "merged", "into": lead id}`. The
  front window used is the lead's. The 90 s deadline starts with the turn's intake, and the restart keeps it.
- A retry whose stop doesn't settle within 10 s replies "I'll start again as soon as it has finished", lets the chat go,
  and keeps waiting (no cap) before carrying the task on.
- Folders must each name one specific folder: "/", the home folder, /Volumes and iCloud Drive's root are sent back for
  repair (argument validity).
- A correction from another chat leaves the task in its own chat (its answer reports there).
- Ended tasks in the task block carry no report line (it is in the conversation or the summary); running ones do.
- Question kinds are structural (capability/approval kind, or the asker's tag); answers record state and the chosen
  option, never typed words.
- The legacy continuity upgrade (pre-0012 owner words, pre-task-context tasks) runs once at start, journaled; conflicting
  records are skipped and counted, never guessed between.
- Scheduled runs now get the environment (Layer 1, worker types) like chat turns.

## Rulings (step 8d, technical)
- A card opens only for two or more parts. One part is one ask: no card, no rows, no count; its `answer_step` only names
  which step answers. A task whose parts are all direct, or whose only part is direct, is told back (reply instead).
- A part's `conditions` are indexes into `success_conditions`. An answer step is checked against its part's conditions
  first, then its own step conditions; a one-ask plan's one last step (no step depends on it) gets all of them. Repair
  facts: a one-ask plan must end in exactly one step; every step must lead to some answer step (this is how a summing-up
  or combining step is refused, structurally); answer steps are distinct and not wait/ask; each condition in exactly one
  part; a direct part has none, an answer part at least one; quotes pass `items.quoted`. Without a plan the answer step
  is "worker" (the task's one step).
- `direct`: null = not direct; "" is allowed (code's receipt of this turn's reminder/memory/schedule answers it). Direct
  parts are recorded once as done `chat.part` rows when the plan line goes out (never sent as part events) and count as
  done.
- Records: one `chat.part` journal row per posted change (done, failed, updated, and direct), keyed by (task, part)
  across turns (the same part id and quote; a reused id with other words is another ask), with the redacted text, note,
  correction, why, verified, the turn that posted it and `card_request_id` (the turn whose card first showed it). `seq` is the UTC time to the microsecond as an ISO
  string, strictly increasing also across a restart (a millisecond integer was redacted by the journal's redactor as a
  long number and crashed the read-back). Events: on the task's stream and on the bus as `task.part` (CRITICAL); a
  resumed task's parts go only on the bus (not again inside `task.resumed_event`).
- A part posts its own answer once (`Card.settle`): a rerun of its answer step in the same run (a restart after it posted)
  never posts again, and a later failure never replaces an answer. A changed answer is one `updated` row: a failed part a
  correction carried through ("done after your correction"), or a part a follow-up turn changed ("changed after your
  follow-up"); the same answer posts nothing. A row
  changes after posting only through a later step's `part_updates` (worker report field of a multi-part task's plan
  steps; the step is shown the answers already posted, fresh each attempt; its checker sees the corrections as part of
  its report; posted once the check accepts, only for a done/updated row and a different text).
- A multi-part turn says its receipts (Remembered, 🗓 Scheduled) in the first reply and not again at the end; one ask
  keeps them at the end as before.
- Status of a multi-part task: done when every part is done (done_unverified when a row stands unconfirmed); failed when
  a part couldn't be done; a stop: cancelled/failed with a headline above the footer. The outcome's `answer` is composed
  by code from the posted rows (history, voice, task results, older clients); `card` carries the footer for the app.
- Upgrades: a spec saved before 8d has no parts. A saved plan that ends in several steps (made for the removed final
  worker) answers with those steps' saved answers joined; the task's success conditions stay unconfirmed (nothing checks
  them now). An answer step that finished before a restart answers from its saved output (its answer, note, correction,
  verification are kept in the step output).
- A loop or delegated conversation that is an answer step: the check at its end also judges the task's success
  conditions it answers (a loop's end check judges end_when and them; a conversation that hands over or finishes gets
  that check), as the removed final worker's check did, so it ends done when they are verified. They never decide a
  loop's end, only whether its answer is confirmed.
- Removed with the final worker: its step context and the `task.send_digest` journal row (only it read that).
- The final worker also carried a correction that arrived after a step ended unfinished (no worker left to hear it).
  Now the plan's unfinished steps run again under it (`PlanStore.retry_unfinished`), only when a step that ended
  unfinished never saw the latest correction (`steers_seen` in its output); a failed row that the rerun carries through
  updates to its answer ("done after your correction"). Past-bug guard: tests/test_steering_races.py.
- A running task's parts, like its plan steps, don't change while it runs: a correction without a plan of its own is not
  checked against parts and leaves them as they are (its new conditions reach the workers through the correction; a
  part's answer step is checked against that part's conditions).
- A continue of a multi-part task never shows a finished part again: the continued run's card carries only the parts not
  finished before it (and any it changes); finished ones stay as posted in the card that showed them; the done count
  covers all of the task's asks; the answer kept for the record holds only that card's rows.
- Each row and part event names `step_task_id` (the step's task id): a confirmation the gateway asks for a task id under
  it belongs to that row (the app ties `approval.requested` by `task_id` prefix); FERB's own questions in a step also
  carry `part` on `approval.requested`.

## Fix list
- [x] The plan line states resolved `folders` whenever there are any ("I'll work in …"), not only for big jobs or
      standing sends (supervisor.py ~2499, 2733), so a wrong folder guess can be corrected.
      (Step 8c: TASK_SCHEMA `folders`; `run_tree` adds "I'll work in ~/…" to the plan line and sets
      `gateway.task_folders` (the shell scope from step 5a); a correction updates them; the worker is told them.)
- [x] Answer format with the lines code appends: answer first, then at most one "Couldn't confirm" line, then the
      code lines in a fixed order ("Remembered:", "🗓 Scheduled:", "Sites contacted:", "(ran late)"), kept short.
      (Step 8b: `Supervisor._final` / `_with_code_lines`; after "Sites contacted:" come the saved-word, app-left-running
      and clean-up lines; "(ran late)" is now the last line, schedules.py.)
- [x] Record the kind of each question asked and its answer, so "few questions" can be measured.
      (Step 8c: `question.asked` / `question.answered` journal rows from every `approval.requested` /
      `approval.resolved` (daemon `_question_record`; kind from who asks: task, confirm, retry, check_in, closed_tab,
      loop, runtime_switch, memory, secret, conversation_scope), and for the planner's own clarify / FERB's "which?".)
- [x] Clarified: a fix-only-the-answer round does not count toward the 5-attempt check-in.
      (Step 8b: fix_answer applies the checker's edits and ends the attempt loop; only a retry is counted.)

### Execution
- [x] Single task as a one-step plan; remove the separate single-worker branch (S:3165).
      (Step 8a: `plans.single`; every tree runs `_execute_plan`; a single task's step is the tree's own worker, id
      `<tree>.worker`; every tree is stored in `task_plans`, so every tree resumes after a restart.)
- [x] Fold `reduce` into `do` (plans.py:108 is its only special rule).
- [x] Fold `map` into `do` with an input list; keep per-input coverage (plans.py problems; plan_executor.py:62-87;
      the duplicate map-level check plan_executor.py:76-81 goes).
      (Step 8a: a do step's `inputs` must each get one note (attempt loop); plan validation covers every inventoried
      input exactly once across do steps; old saved `map`/`reduce`/`loop` steps load as do steps.)
- [x] Check-in after 5 attempts per task/step (decision 7). (Step 8a: `ferb/orchestrator/attempts.py`, counted where
      the next-attempt choice retries; 8b keeps the counter at its retry point.)
- [x] Pause / resume (decision 8): a control action; paused status; resume = continue in place.
      (Step 8a: `TaskControl.action` pause, migration 0049 `paused`; `pause_tree`, `_hold_pause`. Step 8c: the prompt
      line for when to pause.)
- [x] Restart the planner turn on a new message in the same chat (decision 10).
      (Step 8c, decision 18: `_ChatLine`; at most MAX_RESTARTS=1 per turn, only before its first reply; later
      messages queue and are read together in the next turn; a message read with another ends its stream with
      `merged` and its chat.message names `read_with`.)
- [x] Planner call time limit 90 s; a timeout replies plainly instead of hanging (decision 11).
      (Step 4: the 90 s limit is enforced by every adapter, ferb/runtimes/catalog.call_limit. Step 8c: one turn
      deadline (`Supervisor.planner_limit_s`) covers intake, planner calls, repair and the restart; TIMED_OUT reply.)
- [x] Outcome-based done and the answer-first format (decision 9); rewrite `_report` (S:4097-4139); disturbance and
      tool-record facts to the checker.
      (Step 8b: `_report` removed; `_final` writes the answer first; `_next` maps the checker's choice to done when every
      outcome condition is verified, done-unconfirmed only when an outcome is unknown; the report condition leads to
      fix_answer or the one "Couldn't confirm" line. The checker gets `evidence.how_facts`: the screen changes recorded on
      each call, foreground calls, ActionFacts, targeted accounts/sites, and the tabs the task holds.)
- [x] Loops on the plan path as a repeat rule; remove LoopRunner's separate pause (loops.py:446-464).
      (Step 8a: `Step.repeat`; LoopRunner pauses through the supervisor's one wait, `wait_owner`.)
- [x] One status/finish/stop function; remove the 17 scattered status writes (list in the execution audit: tree.py:54,
      worker.py:333/335, S:2265-2913, 3075, 3177, 3230, 3790/3812/3820, 2360, 2371, 2386, tree.py:122-129, M:3645).
- [x] needs_user in a plan step pauses and asks (fixes the endless relaunch).
- [x] Step-level done-unverified kept (S:2614, 2627 store `done`).
- [x] `task.finished` published once per tree.
- [x] Injected clock for plan waits (plan_executor.py:41), loops (S:3070), tree defaults (tree.py:92, 129).
- [x] Paused loop in an unattended scheduled run never ends (loops.py:454-460; S:440): unattended declines at once.
- [x] `every` schedule resumed less than one interval before its first run skips it (schedules.py:76-77; M:3527).
- [x] Conversation-turn rows end `interrupted` though they succeeded (S:2386 sweep).
- [x] After a crash, an auto-resuming plan also gets an "interrupted… pick it up again?" result (M:484-494).
- [x] A resumed scheduled plan loses `approvals.unattended` (in memory, questions.py:45) and bypasses
      `schedules.ran`/retry/watch handling (M:914).
- [x] Clean shutdown runs `on_task_finish` for suspended trees and deletes change records startup means to keep
      (S:2343-2345; M:1002-1004, 918-921).
- [x] Scheduled runs can overlap (no guard in `_run_schedule`).
- [x] Scheduled runs respect the global stop; schedules without a pending timer are reconciled at start-up
      (found in notes/storage.md).
- [x] Dead: `PlanExecutor.notify`, `LoopStore.running`, `tasks.depends_json`, `run_tree` `limit`, legacy statuses.
      (Step 3: `PlanExecutor.notify` and `LoopStore.running` removed. Step 8a: `depends_json` dropped (migration 0049),
      `limit` gone, plan status `steps_done`/`blocked` no longer written; task statuses `skipped`/`unknown` stay in the
      CHECK only for old rows.)

### Intake
- [x] Typed turn object; one exit path; one reply recorder with origin always set.
      (Step 8c: `_Turn` replaces `_env` (also for scheduled and resumed runs); `_exit` / `_start`; `_record_reply`.)
- [x] One quote check shared by remember, until, timers, schedules, schedule changes, reminder cancels, forget,
      conversation changes. (Step 8c: `memory.items.quoted`: whole words of the owner's typed words, blank-line parts
      each; `clause=True` for a forget.)
- [x] Remove `steer` from code paths; one continue/correct verb (S:1407, 1433, 1473; ST).
      (Step 3: `TaskControl.action` is stop/retry/continue; the branches use continue; prompt lines say continue.)
- [x] Cross-chat control: the planner sees every chat's and schedule's running tasks (S:1773) but `problems_for`
      rejects a task outside this chat (S:796). Live failure 2026-10-24.
      (Step 8c: `_control_conversation`: any running task or this chat's; the task stays in its own chat.)
- [x] Retry on a busy task can end as only a stop (S:1438-1456). (Step 8c: after 10 s FERB says it will start
      again once the stop settles, lets the chat go, waits, then carries the task on.)
- [x] Stop/retry after the worker finished records finished work as cancelled (S:1438, 3176, 3948).
      (Step 8c: the checker's accept marks the task's own work done (`_Tree.work_done`); `stop_tree` refuses then:
      "had already finished". A correction during the clean-up still supersedes the attempt (8b), clearing the mark.)
- [x] Mid-task identity/worker_type/plan changes silently dropped (ST:215-221; S:1488-1494): apply or say so.
      (Step 8c: worker type, folders and keep-tabs applied; identity and plan steps said in the reply (KEPT_*); the
      prompt says a retry with the new task changes them.)
- [x] Side-entry replies release the chat lock before the task is registered (S:1643-1654, 1150).
      (Step 8c: the chat is let go only by `_exit` or the task's plan line.)
- [x] Misrouted answer discards the planner's reply (S:1375-1389, 1194, 1198). (Step 8c: FERB's line opens it.)
- [x] Planner shown open questions it can't answer (S:1768-1771 vs ROUTABLE_QUESTIONS S:147).
      (Step 8c: `_questions_block` lists only owner questions of ROUTABLE_QUESTIONS.)
- [x] Worker-type list rides in the droppable activity block (M:3943-3946; S:1750).
      (Already in the protected Layer 1 since step 2 (environment.py `_environment`); confirmed in step 8c.)
- [x] Remove notification rules (schema field, `_side_entries` S:1643-1654, M:3125-3129, perception policy hook).
      (Step 3: schema field and model, `_side_entries` branch, NotifyPort field, `_notification_rule_from_chat`, `notifications.rules`/`notifications.label` RPCs, the CLI and the policy's `add`/`rules`/`muted`/`label`; prompt text removed. Tables left for step 12.)
- [x] Dead: `recall` hook path, `relevant_source_refs`, `since_last_talk` public, `_attempts_text`, `prior is None`
      branch (S:1393-1405), legacy continuity shims after a one-time migration.
      (Step 8c: removed, with `tasks.conversation_roots`/`max_rowid`/`recent_roots` and `ChatSession.task_floor`;
      `continuity.upgrade_legacy` runs once at start (journaled `store.upgraded`).)

### Answers in steps (decisions 19-20)
- [x] Planner schema `parts` and code validation. (Step 8d: `plans.Part`, `part_problems`, `answer_steps`; quote check in
      `Supervisor._problems`; TASK_SCHEMA `parts`; minimal supervisor.md lines.)
- [x] First reply: direct answers, receipts, plan line; the card with a row per other part. (Step 8d: `_plan_event`;
      `plan_line`, `direct`, `receipts`, `lines`, `work_in`, `card` fields; `plan_delta` streams `plan_line`.)
- [x] Each part posts once when its answer step's check accepts it, recorded durably, never twice also across restarts.
      (Step 8d: `ferb/orchestrator/parts.py` Card, journal `chat.part`, event `part` / bus `task.part`, `_post_part`,
      `_close_rows`; resumed plan event carries the card from the record; `chat.status` running tasks carry `card`.)
- [x] No final summarizing call; code adds only its fixed lines and "N of M done". (Step 8d: final worker removed;
      `tree.answering` for one-ask plans; `_final_card`; `_code_lines`.)
- [x] Per-part states: asks (question and approval tied to the part, `part` on `approval.requested`), failed ("Couldn't
      do: reason"), paused (5-attempt check-in), updated (`part_updates`, one record, no new message).
- [x] Structured fields for the app: outcome `note` / `correction` / `lines`; plan `work_in`; `runtime_switch`, `merged`
      as before.
- [x] The notch UI (decision 20): turn card, row states, footer, in-row questions, typing dots (step 8d part 2, `ui/`).
      (Step 8d UI: NotchCore `TurnCard`/`CardRow` (rows keyed by task + part, a change applied once by its `seq`, a
      snapshot never replaces a newer live row), saved on the turn so History draws it; TurnCardView.swift draws the
      mock: first-reply bubble (direct answers, receipts, plan line under a rule with "I'll work in …"), a row per
      non-direct ask with its state, "Couldn't confirm"/"Correction"/"Updated: why"/"You chose …" lines, questions
      with buttons inside their row (FERB's own by `part`/approval id, gateway confirmations by `step_task_id` prefix),
      the "N of M done" footer with code's lines, typing dots from `ack` to the first words, the streamed `plan_delta`
      where the plan line goes. Rebuilt from `chat.status` `running[].card` and `results[].card`; a resumed task's card
      from `task.resumed_event`; a task with no turn gets one. One-ask outcomes draw `note`, `correction`, `lines` on
      their own (cut from the answer text only when it ends with them). `merged` ends the message quietly with "Read
      together with your message above/below". Runtime switch: the in-place question's buttons (no typed answer); a
      failed answer's "Use <runtime>" calls `runtimes.use` for the failed role. Decision 21: rows fill quietly; a row's
      question opens the notch (approval.requested, as before) and so does a finished task, now also
      `done_unverified`; a banner the notch would show itself waits 1 s and posts only if the notch still isn't
      showing (the daemon sends a task's banner before its result). Tests: AnswersInStepsTests,
      NotificationPosterTests.)

### Planner context
- [x] Recent conversation + summary instead of filling the window (S:1709-1710; core/context.py:185, 284).
      (Step 8c: the summary plus every entry after it, append-only; a chunk beyond the last 20 exchanges folds,
      `_keep_recent`.)
- [x] FERB task reports shown short in history; full text via `chat.history`. (Step 8c: `_short_report`.)
- [x] Order blocks most-stable first; move live values (profile connection state M:3853, schedules/reminders in
      `facts()` M:4470) after the conversation. (Step 8c: `_live_facts` and connections in "Layer 2 — right now";
      the planner's instructions get `facts()` only.)
- [x] Task state shown once (today up to 4 times: S:1765, 1772, 1783, 1926). (Step 8c: `_task_state`; the
      timeline's task rows and since-last-talk's task answers removed.)
- [ ] Memory and activity blocks shrink per notes/memory.md (hybrid; accessibility out of the activity search).

### Attempt loop
- [x] Merge dispatcher into the checker; remove `_dispatch`, NOTHING_TO_RECHECK, RECHECK_NOTHING_STOP (S:3213-3218,
      3330-3346, 3391-3417).
      (Step 8b: CHECKER_SCHEMA `next` = accept / fix_answer / retry / ask / accept_unconfirmed / fail; dispatch.md
      merged into verifier.md minimally (step 10 rewrites); recheck and CHECK_DIDNT_LOOK gone with it.)
- [x] "Fix only the answer" path for report-only failures. (Step 8b: `answer_edits`, applied by `_apply_edits`.)
- [x] Resource reviewer folds into the merged checker (notes/prompts.md decision 1): remove `_reconcile_resources`
      (S:3652-3693, called S:3298-3303 and S:2326-2328).
      (Step 8b: the checker sees the tabs the task holds and what FERB releases at the end; a release error becomes a
      code line in the answer.)
- [x] Worker report: done / not done + reason + optional question; loop/probe/conversation fields only for those roles;
      a `how` note for memory (notes/memory.md); the task gains a structured `folders` field (notes/hands.md
      decision 4).
      (Step 8b: `worker_schema(...)` per role, `WorkerReport`; `how` kept on the outcome and the episode (migration
      0050) and the card body. The planner's `folders` field is step 8c, with the planner.)
- [x] Done-unverified includes the worker's answer (`_report` S:4097-4139 never shows it).
- [x] Confirmed conditions survive retries (verification reset each attempt S:3223, `finished` S:3260).
      (Step 8b: `tree.confirmed` and `_carry`: a later null keeps it, a later false drops it; planner conditions only.)
- [x] Artifacts from failed attempts no longer listed (accumulate S:3252-3254).
- [x] One steering checkpoint per attempt (today 5: S:3255, 3282, 3307, 3316, 3328).
      (Step 8b: a correction the worker never saw supersedes the attempt after its check; a done report is kept in the
      attempts the next worker reads.)
- [x] Code writes the failure only for facts no model can change (usage limit, sign-in needed); retry choices stay
      with the checker (CLAUDE.md rule 6).
      (Step 8b with decision 16: adapters tag `account` from structured fields (Codex CodexErrorInfo, the Agent SDK's
      AssistantMessageError, HTTP status, FERB's missing sign-in); `ServiceDown`; the task asks in place
      (`runtime.switch`, options "Use <runtime>" / Stop, `runtime_switch` on the event) and the app's one tap is
      `runtimes.switch_task` (task or role). A planner turn's failure carries `runtime_switch` for `runtimes.use`.
      The app's buttons are UI work.)
