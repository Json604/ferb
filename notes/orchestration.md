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

## Fix list

### Execution
- [ ] Single task as a one-step plan; remove the separate single-worker branch (S:3165).
- [ ] Fold `reduce` into `do` (plans.py:108 is its only special rule).
- [ ] Fold `map` into `do` with an input list; keep per-input coverage (plans.py problems; plan_executor.py:62-87;
      the duplicate map-level check plan_executor.py:76-81 goes).
- [ ] Check-in after 5 attempts per task/step (decision 7).
- [ ] Pause / resume (decision 8): a control action; paused status; resume = continue in place.
- [ ] Restart the planner turn on a new message in the same chat (decision 10).
- [ ] Planner call time limit 90 s; a timeout replies plainly instead of hanging (decision 11).
- [ ] Outcome-based done and the answer-first format (decision 9); rewrite `_report` (S:4097-4139); disturbance and
      tool-record facts to the checker.
- [ ] Loops on the plan path as a repeat rule; remove LoopRunner's separate pause (loops.py:446-464).
- [ ] One status/finish/stop function; remove the 17 scattered status writes (list in the execution audit: tree.py:54,
      worker.py:333/335, S:2265-2913, 3075, 3177, 3230, 3790/3812/3820, 2360, 2371, 2386, tree.py:122-129, M:3645).
- [ ] needs_user in a plan step pauses and asks (fixes the endless relaunch).
- [ ] Step-level done-unverified kept (S:2614, 2627 store `done`).
- [ ] `task.finished` published once per tree.
- [ ] Injected clock for plan waits (plan_executor.py:41), loops (S:3070), tree defaults (tree.py:92, 129).
- [ ] Paused loop in an unattended scheduled run never ends (loops.py:454-460; S:440): unattended declines at once.
- [ ] `every` schedule resumed less than one interval before its first run skips it (schedules.py:76-77; M:3527).
- [ ] Conversation-turn rows end `interrupted` though they succeeded (S:2386 sweep).
- [ ] After a crash, an auto-resuming plan also gets an "interrupted… pick it up again?" result (M:484-494).
- [ ] A resumed scheduled plan loses `approvals.unattended` (in memory, questions.py:45) and bypasses
      `schedules.ran`/retry/watch handling (M:914).
- [ ] Clean shutdown runs `on_task_finish` for suspended trees and deletes change records startup means to keep
      (S:2343-2345; M:1002-1004, 918-921).
- [ ] Scheduled runs can overlap (no guard in `_run_schedule`).
- [ ] Scheduled runs respect the global stop; schedules without a pending timer are reconciled at start-up
      (found in notes/storage.md).
- [ ] Dead: `PlanExecutor.notify`, `LoopStore.running`, `tasks.depends_json`, `run_tree` `limit`, legacy statuses.
      (Step 3: `PlanExecutor.notify` and `LoopStore.running` removed; the rest left.)

### Intake
- [ ] Typed turn object; one exit path; one reply recorder with origin always set.
- [ ] One quote check shared by remember, until, timers, schedules, schedule changes, reminder cancels, forget,
      conversation changes.
- [x] Remove `steer` from code paths; one continue/correct verb (S:1407, 1433, 1473; ST).
      (Step 3: `TaskControl.action` is stop/retry/continue; the branches use continue; prompt lines say continue.)
- [ ] Cross-chat control: the planner sees every chat's and schedule's running tasks (S:1773) but `problems_for`
      rejects a task outside this chat (S:796). Live failure 2026-10-24.
- [ ] Retry on a busy task can end as only a stop (S:1438-1456).
- [ ] Stop/retry after the worker finished records finished work as cancelled (S:1438, 3176, 3948).
- [ ] Mid-task identity/worker_type/plan changes silently dropped (ST:215-221; S:1488-1494): apply or say so.
- [ ] Side-entry replies release the chat lock before the task is registered (S:1643-1654, 1150).
- [ ] Misrouted answer discards the planner's reply (S:1375-1389, 1194, 1198).
- [ ] Planner shown open questions it can't answer (S:1768-1771 vs ROUTABLE_QUESTIONS S:147).
- [ ] Worker-type list rides in the droppable activity block (M:3943-3946; S:1750).
- [x] Remove notification rules (schema field, `_side_entries` S:1643-1654, M:3125-3129, perception policy hook).
      (Step 3: schema field and model, `_side_entries` branch, NotifyPort field, `_notification_rule_from_chat`, `notifications.rules`/`notifications.label` RPCs, the CLI and the policy's `add`/`rules`/`muted`/`label`; prompt text removed. Tables left for step 12.)
- [ ] Dead: `recall` hook path, `relevant_source_refs`, `since_last_talk` public, `_attempts_text`, `prior is None`
      branch (S:1393-1405), legacy continuity shims after a one-time migration.

### Planner context
- [ ] Recent conversation + summary instead of filling the window (S:1709-1710; core/context.py:185, 284).
- [ ] FERB task reports shown short in history; full text via `chat.history`.
- [ ] Order blocks most-stable first; move live values (profile connection state M:3853, schedules/reminders in
      `facts()` M:4470) after the conversation.
- [ ] Task state shown once (today up to 4 times: S:1765, 1772, 1783, 1926).
- [ ] Memory and activity blocks shrink per notes/memory.md (hybrid; accessibility out of the activity search).

### Attempt loop
- [ ] Merge dispatcher into the checker; remove `_dispatch`, NOTHING_TO_RECHECK, RECHECK_NOTHING_STOP (S:3213-3218,
      3330-3346, 3391-3417).
- [ ] "Fix only the answer" path for report-only failures.
- [ ] Resource reviewer folds into the merged checker (notes/prompts.md decision 1): remove `_reconcile_resources`
      (S:3652-3693, called S:3298-3303 and S:2326-2328).
- [ ] Worker report: done / not done + reason + optional question; loop/probe/conversation fields only for those roles;
      a `how` note for memory (notes/memory.md); the task gains a structured `folders` field (notes/hands.md
      decision 4).
- [ ] Done-unverified includes the worker's answer (`_report` S:4097-4139 never shows it).
- [ ] Confirmed conditions survive retries (verification reset each attempt S:3223, `finished` S:3260).
- [ ] Artifacts from failed attempts no longer listed (accumulate S:3252-3254).
- [ ] One steering checkpoint per attempt (today 5: S:3255, 3282, 3307, 3316, 3328).
- [ ] Code writes the failure only for facts no model can change (usage limit, sign-in needed); retry choices stay
      with the checker (CLAUDE.md rule 6).
