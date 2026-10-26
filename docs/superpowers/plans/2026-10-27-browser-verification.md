# Built-in abilities and browser verification

Date: 2026-10-27. **Draft for owner review. Do not execute this plan yet.**

The owner requested research and documentation only, then clarified that browser verification must be a separate,
reusable feature in FERB, with a home for future abilities. That clarification is included here. No runtime code,
skill resource, test, migration or extension change is authorized by this planning session.

## Goal

Ship browser verification as FERB's first built-in ability: immediately discoverable, backed by a saved procedure
and code, able to attempt visible checkboxes, holds and puzzles in the task's authorized browser identity, and able
to verify the result or preserve a precise handoff. Establish `ferb/abilities/` as the home for future reviewed
abilities. Stop requiring each worker to invent the process again.

The feature promises a complete interaction/recovery path, not universal acceptance by anti-bot providers or an
override of a model provider's refusal. Those failure sources must have separate evidence and results.

## Architecture

- `ferb/abilities/`: explicit built-in catalog, packaged procedure resources, typed invocation/result/checkpoint
  contracts, a narrow gateway-backed recipe runner, and the `browser_verification` feature package.
- Existing owner skills remain at `~/.ferb/skills`, read by `ferb/orchestrator/skills.py`. They keep their provenance
  and cannot shadow a packaged executable ability. Learned procedures remain evidence-backed hints, not code.
- Planner/worker context contains names and concise descriptions from the first task. Bodies load on demand.
  `abilities.list`, `abilities.read`, resumable `abilities.run` and read-only `abilities.status` expose the feature.
- The current worker supplies semantic decisions. Code owns validated execution, mechanical receipts and recipe
  state; it never infers permission from a CAPTCHA label. No additional model service, agent role or marketplace.
- Every inner browser action passes through the existing gateway. A run operation holds no outer resource locks
  while invoking inner calls. Preserve nested cancellation accounting, provenance, inner confirmations and P19.
- Generic frame targeting, screenshots, pointer receipts and held input stay in browser tools/bridge/extension.
- Use a small `ability_runs` persistence record linked to an existing task. `PlanStore` stores plan steps, while
  abilities can also be invoked by a worker without a plan (`ferb/orchestrator/plan_store.py:13`); do not fabricate
  plan steps or overwrite a completed step to persist an ability. Reuse the existing DB/redactor/retention paths.
- Retain a checkpoint, not live input, across interruption. No press is restored after restart. The app's existing
  questions and task continuation handle unresolved checks; the original workflow resumes only after a fresh look.

## Spec

[Research, evidence and behavioral specification](../specs/2026-10-27-browser-verification-design.md).
BV1–BV8 in that document are proposal references, not approved decision IDs. BV8 covers the owner's additional
requirement for a reusable abilities library. BV1–BV7 cover browser verification and its evidence/acceptance rules.

Research baseline: HEAD `9686aaddd156ee127a99254b245fd621cac1be16`, plus the working tree observed 2026-10-27.
Source paths and line numbers in the spec are navigation aids at that checkpoint, not permission to overwrite
ongoing changes. The observed failure is task `490777c8a2cf461b8e013f5c7ff44120`, click span
`47ef354b73794546957b84e275889bba`; 0.15.12 live acceptance remains unverified by this planning work.
During final research HEAD advanced to `1bc3a1a`; separate uncommitted changes added `BrowserHandoff`, a proposed
0.15.13 `show_tab`, notification UI and `tests/test_human_check_handoff.py`. This plan did not write that code.
Task 6 must integrate with its eventual reviewed implementation. The human-only prompt in that work and this
proposal's automatic-puzzle behavior require explicit reconciliation in the owner's review before implementation.

## Global Constraints

1. **Execution gate:** wait for owner review and an explicit instruction to implement. This document is not a
   background task, approval or release request. No live runs, deployments, installs or restarts were performed.
2. Follow CLAUDE.md and AGENTS.md. Production code is written fresh in `ferb/`; extension code stays in
   `extensions/chromium/`. No `vibed/` searches or spike imports. Read any applicable subdirectory agreement.
3. Use existing P21/D32 for abilities/runtime separation, W1/W2/W4 for worker/action/checker boundaries, D17 for
   acceptance, D8/D12/D28/D29/MP2/S4/D31/D33 for secrets and authority, and P19 for unknown outcomes. Reconcile this
   approved feature with the plan of record before implementation; do not mark the present proposal Decided now.
4. Background first. Same authorized profile/account. The extension never moves the OS cursor or activates a tab
   to solve a check. Foreground help and browser settings remain subject to the owner's existing rules.
5. No new retry count, total hold ceiling, poll quota, task budget, retention period or rate limit. A requested
   hold interval is explicit action input, not a hidden global cap. Any new numeric product limit goes to the owner.
6. No screenshot/response token/cookie leakage. No vendor callback patch, token replay, disguised request, external
   solver subscription or silent model-provider switch. Provider test controls apply only to operator-owned tests.
7. Keep the ability's method in its packaged procedure and code; the global worker prompt should only explain
   when/how to discover and invoke it. Do not duplicate a long solver procedure across every runtime prompt.
8. Every implementation task starts with a meaningful failing regression, then focused tests, then
   `nice -n 10 uv run pytest -q`, `uv run mypy ferb/core`, and `git diff --check`. One full suite at a time. New ability
   modules also get focused type checking where practical; do not silently narrow the required core check.
9. Each task gets a fresh implementer and a fresh read-only reviewer (spec compliance, then quality), per AGENTS.md.
   Fix/re-review until no Critical or Important finding remains; use its prescribed escalation after three rounds.
   Only after integration/review push `main`, without force. Do not implement or integrate concurrent tasks.
10. Live tests require the owner's observation and use the Codex runtime only (`-m real`). Never run
    `-m real_claude`. Do not replace D17 acceptance with a new local toy app or call test-key results live puzzle passes.
11. Update affected current docs and CHANGELOG with each implemented slice, accurately marked offline/pending live.
    The owner installs the eventual release. Do not run `ferb setup --update` against the current release app.

## Review Focus

- First-run discovery is real: a clean installation sees the built-in ability and its packaged body without
  learned runs, owner skill setup or access to this repository.
- A library is not an uncontrolled plugin loader. Built-in origin, versioning, duplicate names and path containment
  must be explicit; no content from a page can supply an executable module or replace the procedure.
- The runner cannot bypass gateway checks. Nested calls must not remove their parent's `_in_flight` entry on return
  (`ferb/daemon/gateway.py:469`) or release its cancellation protection. No outer tab lock may deadlock inner calls.
- Unknown outcomes stay attached to the actual inner action across new ability IDs, changed model decisions and
  resumes. An outer invocation receipt never certifies all its inner effects succeeded.
- A hold includes the entire lifecycle and observable progress. Its watchdog must remain effective without another
  model call. Terminal receipts must distinguish release attempted, acknowledged and unknown.
- During a hold, normal progress animation may change content; a different document, shifted target or lost frame
  must not be ignored. Screenshot refresh must not accidentally erase the active gesture's binding.
- Model refusal, missing browser capability, failed delivery, unresolved puzzle and website rejection are distinct.
  A string classifier for refusals must not become authorization or an automatic alternate-provider route.
- Token values never become progress evidence, test fixtures or learning material. Completion is observed in the
  application, not inferred from DOM token presence or a green checkbox.
- The current checkbox rule (`worker.md:147`) and stale broad limitation (`docs/FEATURES.md:919`) need deliberate
  reconciliation. Retain the badge-is-not-a-block rule and the existing foreground/security boundaries.

## Execution preflight — only after approval

Read this plan's ledger, current agreements, current task/skill/registry code and `git status`. Identify the first
task without a completion entry. Review intervening commits and dirty changes with their owner; never absorb them
in an ability commit. Confirm whether the 0.15.12 release has actually reached the target profile before planning a
live assertion. Read the old task trace if new diagnosis depends on it.

Register the approved feature's scope under the plan of record using the appropriate existing decisions or a new
decision chosen consistently with the current registry. Record the dated owner clarification about built-in
abilities. Every task commit cites actual registered decisions; BV labels remain intra-spec references.

Ledger location: `.superpowers/sdd/2026-10-27-browser-verification/progress.md`. Record every dispatch, exact base/head,
test output, review, ruling and residual. Each implementation report separates source validation, installed version
and owner live results. The planning ledger has **no completed implementation tasks**.

## Task 1: Built-in ability catalog and first-run discovery

**Depends on:** owner approval and preflight. **Spec:** BV8. **Decisions:** P21, D32, MP2.

**Files:** new `ferb/abilities/{__init__,catalog,contracts,tools}.py`; new
`ferb/abilities/browser_verification/{__init__,definition}.py` and `SKILL.md`;
`ferb/registry.py`; `ferb/daemon/main.py`; `ferb/orchestrator/supervisor.py`;
`tests/abilities/test_catalog.py`; existing `tests/test_owner_skills.py`; packaging tests/`pyproject.toml` only if
resource inclusion needs it. Add a short authoring contract at `ferb/abilities/README.md` during implementation.

**Interfaces:** explicit `AbilityDefinition` from BV8; catalog/read return `source_kind=builtin`, trust origin=`ferb`, version, supported
interaction types and dependency availability. A definition can be discoverable while execution is unavailable;
never advertise a working hold or solver before those tasks land. No dynamic import path in model arguments.

**Failing tests first:**

- A fresh task with empty learning and no owner skills can discover browser verification and read its shipped
  procedure; global prompts contain the catalog description, not the whole body.
- An owner skill sharing its name does not replace the built-in definition or acquire built-in provenance.
- Packaged metadata uses the existing FERB trust label; web observations/model decisions inside a result retain
  external/inferred provenance rather than becoming trusted just because the ability returned them.
- Invalid names, traversal and symlink paths cannot escape the allowed resource; a page-supplied path cannot load
  Python. Duplicate built-in IDs are an explicit startup error.
- A built wheel contains and reads the same procedure resource without a source checkout; missing dependencies
  produce an unavailable reason, not a fictitious successful registration.

**Implementation:** add the minimal explicit catalog and read-only adapters. Load resources through package APIs;
preserve the existing owner skill loader. Publish metadata to both planner and worker context through the existing
capability path. Document adding a future ability (definition, procedure, implementation, registration, tests).
Do not re-enable any model runtime's private skills loader.

**Validate/review:** focused catalog, owner-skill, registry and runtime-isolation tests; required combined checks.
Review origin separation, fresh-install packaging and honest availability. Suggested commit:
`feat(P21, D32): add packaged FERB abilities and browser verification discovery`.

## Task 2: Gateway-backed invocation and durable recipe checkpoints

**Depends on:** Task 1. **Spec:** BV8, BV4, BV7. **Decisions:** P21, D32, W1/W2/W4, P19, MP2.

**Files:** new `ferb/abilities/{runner,store}.py`; contracts/tools/catalog from Task 1;
`ferb/daemon/gateway.py`, `ferb/core/types.py`, `ferb/daemon/main.py`;
task-end integration in `ferb/orchestrator/worker.py` / `supervisor.py` as required by the existing lifecycle;
next available `ferb/store/migrations/*_ability_runs.sql`; retention/forget integration in the actual store paths;
new `tests/abilities/test_runner.py` and focused additions to existing gateway/stop/retention tests.

**Interfaces:**

- `abilities.run(name, tab, run_id?, decision?, observation_refs, declared effect/destination)` returns the BV8
  result. With no run ID it binds a new run; with a run ID it resumes exactly that task's recipe. The gateway, not
  model arguments, supplies task/worker identity, authority and effective scopes.
- Supply a narrow dispatcher bound to the current validated grant, parent span, stop signal and permitted tool
  family. It invokes normal `Gateway.call`; the recipe cannot call raw drivers. Do not pass a serializable run token
  or secret through tool arguments. If `ToolContext` gains a callback, keep it non-serializable and narrowly typed.
- Store run ID, ability/version, task ID, original step, stage, tab/profile binding, revision and redacted checkpoint.
  Completed-action/evidence refs point to existing records. Stage transitions use a revision check; simultaneous
  resumes cannot both dispatch the same next action. Use existing retention/forget ownership, with no new period.
- Each invocation makes grounded mechanical progress and returns `needs_decision`, `waiting_site` or another
  explicit state before a model/owner wait. `abilities.status` only reads. No detached worker/helper continues
  using a revoked grant after the outer invocation or parent task ends.

**Failing tests first:**

- A recorded verification click through the recipe produces normal gateway inner spans and sees the same denial,
  confirmation and unknown-outcome handling as direct browser input.
- Read-only checker cannot run; wrong identity/scope/task cannot resume/read another task's protected checkpoint;
  status never dispatches. A declared payment/send effect is not dropped by the wrapper.
- An inner call finishes, then the parent is stopped: nested in-flight accounting still cancels the outer run.
  Inner locks do not deadlock, and stop/steer cancels further recipe dispatch.
- Duplicate/concurrent resume, lost response and crash between dispatch and checkpoint update do not replay the
  inner action. Persist intent before dispatch; reconcile against its existing receipt on recovery. An unresolved
  outcome stays unknown across a new ability run ID.
- A checkpoint survives restart with fresh-grant validation; incompatible definition versions do not execute.
  Redaction and task-forgetting remove/protect its data using existing retention rules.

**Implementation:** wire the dispatcher without expanding the general tool context into an unrestricted service
locator. Account for nested gateway calls explicitly; preserve image/source provenance and parent span links.
Persist minimal run state and reconciliation hooks. Use a test recipe reflecting the observed checkbox sequence
for runner tests, not a shipped fake second feature. Keep real verification execution unavailable until Task 5.

**Validate/review:** runner, gateway, confirmations, P19, stop and persistence tests plus required combined checks.
Review the failure windows and scope propagation first. Suggested commit:
`feat(P21, W2, P19): execute resumable abilities through the existing gateway`.

## Task 3: Mechanical input receipts and compatibility

**Depends on:** Task 2. **Spec:** BV1, BV4. **Decisions:** P19, W2, L25.

**Files:** `extensions/chromium/{pointer,background}.js`; `ferb/act/browser/{ext_bridge,ext_protocol,tools}.py`;
`tests/test_pointer.py`, `tests/test_closed_shadow_frames.py`, `tests/test_ext_bridge.py`;
protocol/version tests including `tests/test_downloads.py`, `tests/test_browser_background.py`;
`extensions/chromium/manifest.json` when the contract changes.

**Interface:** pointer replies report factual `input_phase` (`not_started`, `pressed`, `released`, `unknown`) and
`release_state` (`not_needed`, `acknowledged`, `unknown`), with a stable operation ID and mechanical reason.
“Released” requires acknowledgement. Failure after a press may still have a released pointer but an unknown page
effect. Successful dispatch never returns `challenge_passed`.

**Failing tests first:** extend the real closed-shadow incident to assert structured phases; a changed target
before press sends nothing; loss of acknowledgement after press and failed cleanup preserve uncertainty; release
acknowledged after an interrupted drag does not claim the application completed. Existing known-failure tests must
remain green. Old extension replies with absent fields are explicitly unknown/unavailable, not fabricated phases.

**Implementation:** add factual receipts at the point input is sent/acknowledged. Keep target/frame checks and the
0.15.12 known-failure fix. Propagate facts through `ActionResult` and existing trace evidence. Coordinate manifest,
JavaScript/Python protocol and minimum-version changes in one slice, choosing the next actual versions at build
time. Report incompatible installed versions without silently using OS input or scripted page events.

**Validate/review:** focused pointer, closed-shadow, bridge and version-contract tests, then combined checks. No
test harness is a live provider result. Suggested commit:
`fix(P19, W2): distinguish pointer delivery and release from website outcomes`.

## Task 4: Complete background press-and-hold lifecycle

**Depends on:** Task 3. **Spec:** BV3, BV7. **Decisions:** L25, W2/W4, P19, D31/D33.

**Files:** `ferb/act/browser/tools.py`, `ext_bridge.py`, `ext_protocol.py`; browser interfaces/types as needed;
`ferb/registry.py`; `extensions/chromium/{pointer,background,content}.js` and manifest/version contracts;
`tests/test_pointer.py`, `tests/test_ext_bridge.py`, `tests/test_browser_tools.py`;
new `tests/test_browser_hold.py`; task/bridge shutdown and stop hooks actually owning the tab.

**Interface:** `browser.hold`/`ferb_browser.hold` and their `hold_status` variants exactly as BV3 specifies. One
opaque gesture handle binds task, worker, profile, tab, document/frame, point and generation. Start requires a fresh
screenshot point. Extend/release preserve the effect/destination of the entire gesture; status is read-only.
Positive finite `hold_for_ms` is caller-selected with no hidden default/ceiling. Reject malformed/non-representable
values at both Python and extension boundaries. Record actual elapsed time and release acknowledgement.

**Failing tests first:**

- Start sends one down event; observations do not release/re-press; extension changes only the deadline; explicit
  release sends one up and cannot resurrect the handle. A duplicate release returns its stored terminal receipt.
- A progress-bar change permits continued holding. Navigation, frame replacement, shifted target, detached target,
  owner cancellation, tool timeout, expired interval, daemon disconnect and task stop each end input and record the
  observed cleanup outcome. A failed release cannot report success.
- No model activity is needed for expiry. Simulate delayed timers/sleep with a fake monotonic clock; waking an
  expired gesture releases instead of extending. Do not use wall-clock adjustments as duration evidence.
- Wrong task/worker/tab/generation cannot extend/release a handle; ordinary input in the same tab cannot interleave.
  Other tabs can run. Screenshots remain redacted and do not invalidate the separately frozen hold binding.
- Hold start/release obey action declarations, final-checker restrictions and existing code floors. Stop cleanup
  releases input even if further discretionary input has lost authority; it never initiates another press.
- All required gesture confirmations happen before press. If an extension needs fresh authorization, it releases
  before asking; no nested gateway question leaves a button held or silently broadens the original grant.

**Implementation:** implement the lifecycle as a single complete slice. The extension owns expiry and cancellation;
the bridge reconciles receipts and invalidates handles on reconnect. Update active-gesture guards for non-pointer
mutating commands, disconnect/keepalive paths and task cleanup. Never rely on `finally` alone to claim cleanup after
the debugger is gone. Do not auto-close/activate/reconfigure a personal browser to regain input control.

**Validate/review:** focused lifecycle/policy/cancellation tests, version contracts and required combined checks.
Actual background timing and visible progress are pending supervised acceptance. Suggested commit:
`feat(L25, W2, P19): add observable background holds with owned cleanup`.

## Task 5: Implement the browser-verification ability and reusable procedure

**Depends on:** Tasks 1–4. **Spec:** BV1/BV2/BV4/BV7/BV8. **Decisions:** P21, P27, L25, MP2, W2/W4, P19.

**Files:** `ferb/abilities/browser_verification/{workflow,definition}.py`, its `SKILL.md`;
`ferb/abilities/{contracts,tools,runner}.py` only for demonstrated contract gaps;
`ferb/orchestrator/prompts/{worker,interaction,dispatch}.md`; `tests/abilities/test_browser_verification.py`;
`tests/test_prompt_contract.py`; `tests/security/test_catalog_replay.py` / existing scenario-14 harness.

**Interface:** typed semantic decisions carry the fresh observation ID, visible instruction/evidence, interaction
type, chosen target(s) and expected next state. The code recipe returns a decision request when interpretation is
needed, dispatches only supported operations through the bound gateway, and demands new evidence after changes.
Provider names annotate evidence; they do not grant authority or select hidden bypass scripts.

**Failing tests first, grounded in the documented cases:**

- The visa closed-shadow sequence reaches a resolved control and can recover from a known no-press failure using
  fresh evidence; an unknown click cannot be treated as the same failure.
- A visible second-stage image puzzle requests interpretation instead of immediate human handoff. A changed tile
  invalidates the next stale choice. A jigsaw/slider uses two points from the same current image and checks the result.
- A HUMAN-like observed hold instruction uses start → fresh progress observations → release; no provider duration
  constant or repeated click substitutes for holding. Expiry before a decision returns a factual expired result.
- Badge-only and ordinary form-error observations do not assert a challenge; silent scoring does not invent a
  clickable target. Delivered input followed by continued rejection remains unresolved.
- The configured model declines interpretation: record `unavailable` with that reason, preserve the task and release
  input; do not call it website rejection, claim success, or switch providers. An agent declining invocation is an
  evaluation finding, not something a hidden text trigger “fixes.”
- Scenario 14's fake CAPTCHA commands/secret requests remain external data. No callback/token value or stale
  solution enters a reusable procedure. A check that auto-submits a consequential form retains the original declared
  effect and authorization scope; it is not relabelled as a harmless checkbox.

**Implementation:** keep procedural expertise in the ability package. Global instructions advertise discovery and
invocation, replacing only the premature-handoff rule. The default interpretation source remains the current worker;
do not add a new inference provider or secretly run a second model. Mechanical retries require new facts/decisions;
no hidden attempt cap and no endless code-level same-action loop. Mark unavailable modes honestly.

**Validate/review:** workflow behavior tests, injection/provenance tests, prompt contract regression and combined
checks. Prompt string assertions alone are insufficient; scripted model decisions must drive the actual runner.
Suggested commit: `feat(P21, P27, L25): ship browser verification as a reusable FERB ability`.

## Task 6: Handoff, interrupted runs and original-task resumption

**Depends on:** Task 5. **Spec:** BV5/BV8. **Decisions:** P21, P19, L3, P27, D33.

**Files:** ability workflow/store/runner; `ferb/act/ask.py`, `ferb/daemon/questions.py`,
`ferb/orchestrator/{supervisor,worker,plan_store,plan_executor}.py` only where required by actual continuation;
`ferb/act/browser/ext_bridge.py` tab ownership/cleanup; existing `tests/test_questions_persistence.py`,
`tests/test_waits_and_loop.py`; new `tests/abilities/test_verification_resume.py`.
Reuse the separately developed `BrowserHandoff`/notification/automatic-resume path and
`tests/test_human_check_handoff.py` after confirming what actually landed. Use existing app questions for choices.
If the app cannot express this handoff, specify the concrete gap, then read `ui/AGENTS.md`
and add the smallest reviewed change with Swift tests; a new CAPTCHA settings screen is not planned.

**Failing tests first:**

- An unresolved check releases its hold and sends one concrete handoff notification while retaining the correct
  tab/profile, original step and unsent form. Clicking it opens that tab; other tasks and the composer stay usable.
- Automatic completion detection leads to a fresh observation; message disappearance alone never marks passed.
  A “done” reply is not required. If the challenge remains, do not mark passed. If the form already submitted,
  reconcile the observed result rather than submitting again. P19 survives the handoff.
- No answer does not expire an actual question or grant foreground input. Owner stop/cancel ends the ability and prevents
  later answers from resuming it. No automated restart secretly re-presses a held button.
- A lost tab, changed account, restart, procedure-version mismatch or changed task scope produces a specific
  recoverable state. Never pick the frontmost tab/account as a substitute. Resume uses the current valid worker grant;
  obsolete gesture handles are discarded even when the durable ability run is retained.

**Implementation:** reuse handoff notifications, question IDs where relevant, app events, current continuation and task persistence. Keep the checkpoint
attached to the original step and trace; handle a worker without a plan as well as a planned/scheduled step. Do not
widen a scheduled task's scope. Show the actual missing capability, model response or site evidence, not generic
“CAPTCHA blocked.” Emit normal progress; keep technical run IDs in Inspect/trace rather than everyday app wording.

**Validate/review:** ability resume, question, stop, plan and browser ownership tests plus required checks; Swift tests
only if Swift changed, after sourcing `scripts/swiftpm-anonymous.sh`. Suggested commit:
`feat(P21, L3, P19): preserve verification handoffs and resume the original task`.

## Task 7: Documented acceptance, runtime evaluation and release readiness

**Depends on:** Task 6. **Spec:** BV6 and all feature contracts. **Decisions:** D17, P21, P27, W4, L25.

**Files:** new `tests/test_browser_verification_real.py`; reusable support only under `tests/support/`;
redacted replay/evaluation cases in the existing eval framework; `docs/OWNER_ACCEPTANCE.md`,
`docs/CAPABILITIES.md`, `docs/FEATURES.md`, `README.md`, `ARCHITECTURE.md`, `CHANGELOG.md`,
`docs/0.27_PLAN.md`, `docs/0.27_DECISIONS.toml`; `tests/test_decision_coverage.py` only if a necessary mapping changes.

**Prerequisites:** obtain the owner's concrete choice of real workflow/account before live runs. Use the existing
kartikey.xyz request-logging surface for an actual protected form/task and record its route and expected stored
result. Deploying protection or creating paid provider resources needs separate scope. Vendor account/test-key
availability is not assumed. The owner's visa flow is a known scenario but still needs approval for the new run.

**Failing tests first:** add acceptance assertions against the selected documented workflow before claiming a pass.
Offline replay cases exercise the actual runner with sanitized observations from owner-controlled cases. Real tests
use the Codex runtime and shipped ability discovery, not direct driver calls alone. Without the relevant fixture,
credentials or approval, record the test as pending/skipped with its reason; never replace it with a toy and a pass.

**Acceptance matrix to execute only with the owner:**

| Scenario | Required observed result |
| --- | --- |
| Clean install / empty learned memory | The ability is available and invoked without learning/training or manual skill installation. |
| Known Cloudflare checkbox incident | Current version/profile recorded; point resolves the checkbox; delivered/not-delivered and actual site result reported independently. |
| Controlled pass, failure and expiry | Widget result and protected application result agree; rejection/expiry never becomes success. |
| Real visible image puzzle | Current image drives each choice; changed tiles are re-observed; actual acceptance recorded. Test keys that omit puzzles do not satisfy this row. |
| Press-and-hold | Progress observed while held; release acknowledged; intended form continues or the actual rejection is retained. |
| Slider/jigsaw | Grounded start/end, no stale drag, observed outcome and original task continuation. |
| Silent challenge/scoring | No invented input or diagnosis; the application's real response is reported. |
| Runtime compatibility | The configured Codex runtime discovers/invokes the ability; any interpretation refusal is recorded separately. No real-Claude run by this agent. |
| Handoff and resume | Same task/profile/tab, no lost form, automatic fresh verification after the owner's interaction, no required “done” reply and no duplicate consequential submit. |
| Stop/disconnect/restart | No continued input or automatic re-press; release uncertainty is reported honestly. |
| Background behavior | Owner cursor, keyboard focus and Space are undisturbed by the verification operations; the existing accepted initial-profile-window exception is recorded separately. |
| Fake verification instruction | No command, secret disclosure or settings change obtains authority from the page. |

**Evidence:** per observed case record ability version, app/daemon/extension/browser versions, task/trace IDs,
profile, check type, input phase/release result, before/after evidence references, application result and owner verdict.
Report observed counts and outcomes without inventing a required sample size or success percentage. Vendor-test,
offline replay and production results have separate rows. Never store reusable response tokens or raw cookies.

**Implementation/validation:** add the real scenario harness and evaluation cases, run authorized checks, then the
required combined offline checks. Reconcile feature docs with measured support: remove the blanket human-only
claim only for the implemented path, and retain explicit pending/unavailable coverage. Preserve 2FA's sealed route.
Suggested commit: `test(D17, P21, L25): validate built-in verification on documented owner workflows`.

## Final review and owner installation

After task reviews, obtain one whole-branch review on the most capable available model and complete one fix wave
with focused re-review and required checks. The owner then installs the approved build; do not install/restart the
release app from this plan automatically. Extension distribution and daemon compatibility must be coordinated.

Record the owner's verdicts and remaining provider cases in `docs/OWNER_ACCEPTANCE.md`. An unavailable vendor
account or a production rejection prevents claiming coverage for that case, not reporting the completed source
work. Do not declare the whole feature live-accepted while required cases are pending. A clean source checkout,
offline green suite and public vendor test token are each insufficient for that claim.

## Self-Review

- **Owner's new scope included:** browser verification is a separate built-in feature with code and a saved
  procedure; future abilities have a supported package/registration pattern and no first-run learning prerequisite.
- **Architecture is grounded:** current skills are text readers, capabilities are registered explicitly, workers
  carry scoped grants, and gateway locks/in-flight accounting constrain a nested runner. Tasks test those boundaries.
- **No hidden implementation:** this plan and its spec are the only tracked additions from the planning work.
  All runtime filenames above are proposed; no ability package or SKILL.md has been installed.
- **Feasibility is honest:** correct input, model interpretation and server acceptance are separate. No universal
  CAPTCHA solve promise, claim that a skill overrides a refusal, or invented live pass.
- **Hold safety is complete:** feedback, finite requested intervals, concurrent-input exclusion, cancellation,
  unknown release, disconnect and restart are covered in the same deliverable.
- **Tests follow evidence:** the visa incident anchors the targeting regression; D17 controls new live scenarios;
  vendor test keys and screenshot replay never masquerade as a production puzzle pass.
- **No invented numeric limits:** existing limits are not silently tightened; new product ceilings require the owner.
- **No broadened authority:** builtin/owner/external provenance is preserved; every inner action stays behind the
  gateway, and retries/resumes cannot discard the prior unknown outcome or consequential effect.
