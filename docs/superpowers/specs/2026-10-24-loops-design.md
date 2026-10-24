# Loops — design (2026-10-24)

Status: behavior approved by the owner on 2026-10-24 ("yes numbers work, go ahead"); this written design awaits the
owner's review before an implementation plan. Decision id: L31. Insomnia (keeping the Mac awake for standing work) is
its own design, L32, and plugs into the "standing work" this design publishes.

## Why

The owner wants general abilities that any task can use, not per-task additions (owner, 2026-10-24: "there should be
general capabilities like loop, or long running task so that it can use them here. not a per task addition
everytime."). The plan's target tasks already need one: "watch a PR's GitHub Actions page; when checks fail or pass,
tell that agent; loop until the checks pass" and "keep a WhatsApp chat going until the other person says bye"
(`docs/0.27_PLAN.md:548-549, 570`), and its gap list names "long-running tasks (hours to days) that survive restarts …
and wake-ups on events or time" (`:575-576`).

As built, a task cannot repeat:
- A plan is a one-pass graph of `do / map / reduce / wait / ask / converse` steps (`ferb/orchestrator/plans.py:22`).
- The planner is told "Never wait for a condition with a loop or sleep inside a task" (`prompts/supervisor.md:123-124`).
- Watches tell the owner when something becomes true, but each check is a fresh full task every N minutes
  (`ferb/daemon/main.py:2858-2908`) and a watch never acts.
- Live case, 2026-10-24 (task e9c65477): "keep it running, only stop when i say" kept a Brave Instagram tab playing
  once; nothing re-played it when the site paused it, and the plan asked the owner to switch apps to verify unfocused
  playback because FERB never moves focus.

What exists to build on: plans survive restarts (`supervisor.py:822-866`, migration 0028); waits use the wall clock
(`plan_executor.py:26-48`, L22); the converse runner already loops — one checked worker turn, then a durable sleep that
holds no model session, woken by events, until handed over, paused or stopped (`conversations.py:282-317`); the gateway
already runs observation-only calls under a read-only grant for the final checker and for conversation reads
(`daemon/auth.py:65`, `supervisor.py:2353-2354`).

## What the owner gets

"Keep this tab playing until I say stop", "loop until the PR's checks pass", "keep this folder sorted": one task with a
`loop` step. The plan line says what the loop keeps true and how it ends, and work starts at once (rule 9, D30). Between
rounds no model runs: code checks one exact fact every 10 seconds and starts a round only when that fact shows the job
needs doing again. Chat stays quiet; the notch's task list shows the loop as standing work with Stop. The loop runs
until the owner says stop, or until an end condition the owner stated is verified.

Owner decisions (2026-10-24):
- A loop with no stated end runs until the owner says stop, across restarts and updates ("Until you say stop").
- When rounds keep failing to last, FERB backs off and tells the owner once, and keeps going ("Back off and tell you").
- The numbers below ("yes numbers work").

## Design

### 1. Plan vocabulary: a `loop` step
- `Step.kind` gains `loop` (`plans.py:22`, `STEP_SCHEMA` in `schemas.py`). Its fields:
  - `goal`: what the loop keeps true or repeats, in the planner's words grounded in the owner's.
  - `end_when`: the end condition in the owner's words, or empty for "until I say stop". Code never interprets it; the
    round's worker reports `finished` against it and the checker verifies that report (below).
  - `conditions`: as for `do` steps, what each round must achieve.
  - `event` / `event_match`: optional, as for `wait` (exact daemon event plus exact field matches), as one more waker.
- A loop may depend on earlier steps, and later steps may depend on a loop that has an `end_when` (they run once it
  finishes). Code's plan check refuses a step that depends on a loop with an empty `end_when`, which could never run, and
  sends the plan back for repair (`repair.md`, as other feasibility failures). A plan holds any number of loops;
  independent loops run side by side like other independent steps.
- `plan.stated` (`supervisor.py:2141-2147`) states each loop in one line: what it keeps true, its end ("until you say
  stop" or the owner's `end_when` words), and that it checks without the model between rounds.

### 2. A round
- A round is an ordinary worker run for the step, followed by the checker: same tools, gateway, confirmations, sink
  rule, P19 unknown-outcome rule and standing scope as any plan step (`supervisor.py:2149-2175`). A loop grants nothing:
  only the owner's words and the plan's stated standing permission authorize (MP2, D30).
- Owner decision, 2026-10-25 ("Skip it for look-only rounds"; live loop cc5debe4 waited 14 s for a checker after a
  round that only looked at a reel already playing): a round that only looked skips the model checker, and its probe's
  first read (the baseline, when the probe is armed) is the check. A round only looked when it made at least one call,
  every call was a W2 `read` of an observation tool (W4) or only took or kept hold of a tab (`use_tab`, `keep_open`:
  kind `open_tab`, which count as non-acting here) with no declared effect, and it reports `continue` with a probe.
  Code also requires that the probe reads a tab or id the round actually looked at: what the probe names (a string in
  its tool's own arguments, or a field segment that is no key, such as a tab_ref or media id) must be among the values
  the round's own calls sent or got back, so a wrong-but-consistent probe field can't pass unchecked. Any acting call
  (media, act, navigate or a reload, a send, a question to the owner) or a `finished` report with an `end_when` still
  runs the checker. The trace records a `verify.skipped` span saying why (`loops.look_only_round`).
- The round's context (`step_context`) carries the loop goal and `end_when`, why this round started (first round, probe
  tripped with the observed field value as untrusted data, event payload, backup interval, or the owner's steer), and a
  digest of earlier rounds: count, the last few rounds' summaries and outcomes, the tabs the loop holds (`held_tabs`,
  added 2026-10-25 so a later round doesn't find and borrow its tab again), and the action receipts, like
  `ConversationStore.digest` (`conversations.py:262-270`). Never the whole history.
- The worker schema gains two nullable fields, used only by a loop round (as `conversation_action` and `watch_met` are
  used only by their steps, `schemas.py:131-140`):
  - `loop_action`: `continue` | `finished` | `blocked`.
  - `probe`: `{tool, args_json, field, op, value_json}` — one observation tool, its arguments as JSON, a dot path into
    its result (`media.0.playing`), an operator `equals` | `not_equals` | `changed`, and the compared value as JSON
    (unused for `changed`). `continue` needs a probe or relies on the backup interval and wakers alone.
- `finished` needs observed evidence that the `end_when` condition holds, and the checker must verify it; an unverified
  `finished` is treated as `continue` (as a converse hand-over needs evidence, `conversations.py:186-188`). With an empty
  `end_when`, `finished` is refused and the round counts as `continue`: only the owner ends such a loop.
- `blocked`, a round that ends `needs_user`, or a P19 unknown outcome pauses the loop: no probes, no rounds, until the
  owner answers or steers. The question is asked once and never expires while the loop waits (rule 9). Added
  2026-10-25 (owner, after closing the Instagram tab a loop kept: "shouldn't it ask 'hey, should I stop the task since
  you closed the tab' when it notices the task is not there"): the question is a real owner question with buttons, not
  only the chat line. A tab the loop held that is closed: "The <site> tab you asked me to keep going was closed. Stop
  this task?" with Stop / Keep watching ("I'll reopen it; resume"); any other pause: the round's own question with
  Stop. Stop ends the loop and its task; Keep watching (or typed words) starts the next round; a chat reply steers as
  before. The chat line names the loop in a few words (its goal's first clause, at most 60 characters).

### 3. Probes (code compares; the model chose what and when)
- Code validates a probe before arming it: the tool exists, is in the task's scopes, its arguments validate against its
  `Input` model, it is marked `observation = True` (`gateway.py:130-133`) and it is probe-safe. Probe-safe is a new
  capability attribute (`probe_safe = True`) on observation tools that read state without moving anything or producing
  a picture, and return at once: browser `tabs`, `snapshot`, `find`; `shell.list`, `shell.read`; the file
  tools' stat and read; `mac.snapshot`. Not `scroll`, `open`, `screenshot` or `point` (they move a view or return
  images), nor browser `wait` or `shell.wait` (they block until something happens), nor browser `read` (it pages the
  last saved snapshot, which nothing renews between rounds, so it could never change). A refused
  probe is reported back as the round's error and the loop falls back to the backup interval.
- Arming: right after the round, code runs the probe once as the baseline (for `changed`, the baseline value is the
  comparison value). Then every `PROBE_SECONDS` (10) it runs the probe through `Gateway.call` with an observe-only grant
  under the loop's task id, and compares the field exactly. It trips on a match, on a tool error, or when the field is
  missing (the error or absence goes to the next round as its reason).
- Code only compares JSON values; it never judges meaning (R2). The model decided what fact to watch and which value
  means "do it again", exactly as `event_match` today.
- Recording: the gateway's checks and provenance apply to every probe call, but the span store keeps the baseline, each
  trip, each error and a running count per armed probe — not one span per unchanged read — so a loop that runs for days
  doesn't flood the database (the resource log, L30, is the measure). A probe's result never reaches a model; only the
  tripping field value does, wrapped as untrusted outside data (`sanitize/untrusted.py`).

### 4. Waking and pacing
- A round starts when the probe trips, the step's `event` matches (`supervisor.py:828-835`), the owner steers the task
  (a plain reply routed as a steer, `supervisor.py:1115-1132`), or `BACKUP_SECONDS` (1,800) pass since the last round —
  so a probe that silently stopped meaning anything still gets a model's look.
- Times use the wall clock and recheck at least every 30 seconds (`clock/timers.py:21-27`), so after Mac sleep the
  probe runs at once on wake.
- Churn back-off (the owner's "back off and tell you"; numbers approved 2026-10-24):
  - A round "didn't last" when the probe trips again within `NO_LASTING_FIX_SECONDS` (120) of that round ending.
  - After `CHURN_ROUNDS` (3) rounds that didn't last within `CHURN_WINDOW_SECONDS` (600), the minimum gap between rounds
    becomes 120 seconds and doubles on each further round that doesn't last, up to `MAX_GAP_SECONDS` (3,600). FERB tells
    the owner once per back-off episode, in plain words: what keeps breaking, how often it was fixed, that it now waits
    longer between fixes, and that "stop" ends it.
  - A round whose fix lasts `RESET_SECONDS` (1,800) returns the loop to normal pace and ends the episode.
  - While backing off, trips inside the gap are remembered and handled when the gap ends.
- These constants are the owner's; changing them is an owner decision.

### 5. Durable state and restarts
- Loop state lives with the plan: a `loop_state` row per loop step (migration): round number, state
  (`acting` / `watching` / `backing_off` / `paused` / `finished` / `stopped`), the armed probe and its baseline, last
  round end, back-off gap and episode, and the round counts. All rounds share the task id
  `<tree>.loop.<step>`; each round is a `loop.round` span under it (as built, 2026-10-24). A child task per round was
  dropped because a loop that runs for days would leave thousands of task records, and because one id lets Stop, the
  gateway's task ownership and the loop's resources all name the same task. A round's checker judges only the calls
  made in that round; earlier rounds reach the next round through the digest's one-line results.
- Daemon shutdown suspends the plan (`suspend_plans`); start-up resumes it (`resume_plans`) and re-arms the probe from
  the stored row; a probe that now errors (e.g. the tab went with a browser restart) trips a round that re-establishes
  the state. Resuming a loop posts nothing to chat.
- Resources a round starts that the loop needs between rounds (a shell session watching CI, a FERB tab) belong to the
  loop step, not the round: they are released when the loop ends, not after each round (today a converse turn releases
  its resources per turn, `supervisor.py:2373-2376`). A restart loses them, and the next round re-creates them.

### 6. Ending
- Stop (the task's Stop, `task.stop`, or "stop" in chat) ends the loop at once through the existing stop path
  (`supervisor.py:752-759, 2461-2475`): the probe disarms, a running round is cancelled, loop resources are released, and
  FERB says it stopped.
- A verified `finished` ends the loop; its answer says what was reached, with the evidence, and the plan moves on.
- Nothing else ends a loop: no count, time or cost limit (the owner's choice).

### 7. What the owner sees
- Chat: the plan line when the loop starts; then nothing per round. Messages only for a verified finish, a pause that
  needs the owner (blocked, a question, P19), a back-off notice, and the stop confirmation.
- The notch (`ui/mac`): the daemon publishes `loop.status` (task id, goal, state, rounds acted, last round's time, back-off
  gap) on each state change. The tasks list shows a loop as standing work — "Keeping: the Instagram tab playing ·
  fixed 4 times · last 2 min ago" — with Stop and Inspect (every round's record). The "active tasks" row counts loops
  separately ("1 thing kept going") and shows the spinner only while a round is acting.
- `ferb trace` shows the rounds as `loop.round` spans and the probe's baseline, trips and errors as `loop.probe_read` spans.

### 8. Planner guidance
- `supervisor.md`: replace "Never wait for a condition with a loop or sleep inside a task …" with: work that must be
  repeated or kept true ("keep …", "loop until …", "every time … do …" while the owner's request is running) is a plan
  with a `loop` step, its `end_when` taken from the owner's words or empty; a request only to be told when something
  outside FERB's work becomes true stays a watch; work at set times stays a schedule. Added 2026-10-25 (live loops
  cc5debe4, a1f16a32, 4a8b1d73: every plan bound the tab in a `do` step first, 43–55 s, and that step's tab was let go
  of when it ended, so round 1 did it all again): a loop's first round does its own setup (finds the target, starts or
  fixes it, arms its probe); its rounds share one task and keep what they open; an earlier step only for different work
  the loop needs.
- `worker.md`: in a loop round, act once toward the goal, verify it, and report `loop_action` with a probe that would show
  the job needs doing again, choosing a probe-safe observation tool and an exact field; `finished` only with evidence of
  the owner's `end_when`.
- No keyword list routes requests to loops (R2): the planner decides.

### 9. The two live cases, end to end
- Instagram (task e9c65477's request): the plan has one loop step, goal "keep the front Instagram tab playing", `end_when`
  empty. Round 1 plays the tab and verifies the media time moves (`extensions/chromium/content.js:492-513`), then reports
  `continue` with probe `{browser snapshot of that tab, field media.0.playing, equals, false}`. The site pauses it while
  Brave is unfocused, the probe trips within 10 seconds, round 2 plays it again. No question to switch apps: the probe
  observes unfocused playback itself (this is option (b) of the 2026-10-24 diagnosis, now general).
- CI: "watch PR 12's checks; when they fail, tell the Claude session; loop until they pass". Round 1 starts
  `gh pr checks 12 --watch` with `shell.start` (a loop resource) and probes `shell.list`'s `running` for that id,
  `equals false`. When it exits, the round reads the result: on failure it sends the failure to the agent's session and
  re-arms; on success it reports `finished` with the checks' output as evidence, and the checker verifies it.

## Not in this design
- Keeping the Mac awake while a loop runs: L32 (insomnia), which reads the running loops as standing work.
- Moving watches onto probes (faster, cheaper "tell me when"): possible later; watches stay as built.
- The ⌘⇧T-style tips, proactive offers and the other release items from the 2026-10-24 audit: separate designs.

## Testing
- Offline (new `tests/test_loops.py`, shared fakes in `tests/support/`): the step validates, and a step depending on a
  loop with no `end_when` is refused for repair; rounds run through the
  worker and checker; probe validation refuses non-observation, non-probe-safe, out-of-scope and invalid-argument
  probes; exact comparisons for `equals` / `not_equals` / `changed`, errors and missing fields trip; the backup interval
  fires; event and steer wakers; back-off thresholds and reset with a fake clock, one notice per episode; `finished`
  refused without `end_when` and without verified evidence; pause on `blocked`, `needs_user` and P19; stop disarms and
  releases loop resources; restart resumes with the same probe and no chat line; the wall clock after a paused monotonic
  clock (as `tests/test_waits_and_loop.py`); probe recording keeps counts, not a span per unchanged read; quiet chat;
  `loop.status` events; the planner prompt routes "keep …" to a loop and "tell me when …" to a watch (contract tests).
- Swift: the tasks list renders `loop.status` as standing work with Stop; the active-tasks row counts loops apart.
- Live, owner-observed (Codex runtime, `-m real`): the Instagram case on the owner's own Brave tab; a CI loop on a PR in
  an owner-controlled repository; a daemon restart in the middle of a loop; Stop during a round. The resource log is
  compared before and during a loop (L30).
- Decision L31 is registered in `docs/0.27_DECISIONS.toml` with these tests; ARCHITECTURE.md, README.md (Limitations:
  the "Never wait for a condition" limit goes) and CHANGELOG.md are updated as built.
