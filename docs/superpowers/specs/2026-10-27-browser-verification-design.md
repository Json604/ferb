# Browser verification that FERB can finish

Date: 2026-10-27. **Owner-approved for implementation; source work in progress and live acceptance pending.**

Owner request: research the verification systems in the supplied table and plan a durable fix for personal,
owner-directed browser work. This includes the checkbox, press-and-hold and visible-puzzle cases. It is not a
request to deploy a release, change a website, run live challenges, or change the owner's browser settings.

Owner clarification during planning (2026-10-27): make this a separate feature set and code within FERB, under a
reusable skills/abilities area that can receive other abilities later. FERB should know the procedure from its first
run instead of rediscovering it each time. The owner then approved this design and its implementation plan:
“good, make it now under the folder we discussed. complete and come” (2026-10-27). The approval includes attempting
visible interactions before a concrete handoff. It does not authorize live third-party runs, installs or deployment.

Implementation plan: [browser verification](../plans/2026-10-27-browser-verification.md).

## Goal

FERB should handle the verification UI it can see in the authorized browser task: click a checkbox, hold a button,
select pictured objects, or place a slider using the same observation and input tools it uses elsewhere. It should
then establish whether the website accepted the check and continue the original task. It should ask for help only
with a concrete remaining obstacle, preserving the task, account and page.

“Once and for all” means fixing FERB's missing capabilities, premature handoff, failure diagnosis and recovery path.
It cannot mean guaranteed admission to every website. The site's server controls that decision, including when
there is no visible puzzle. Cloudflare explicitly lists automated browsers as unsupported for production challenges
([supported browsers](https://developers.cloudflare.com/cloudflare-challenges/reference/supported-browsers/)).
This is a technical compatibility limit, not a conclusion about the owner's purpose.

## Evidence and current state

Code references were inspected on 2026-10-27, with HEAD `9686aaddd156ee127a99254b245fd621cac1be16` at the final
research checkpoint. The working tree was changing during research; uncommitted browser/pairing work was initially
present, and unrelated gateway/confirmation edits remained at that checkpoint. Re-read these paths before building.
No implementation, prompt, configuration, installed app or browser was changed during that research pass.

Later research checkpoint: HEAD advanced to `1bc3a1a` while separate uncommitted work added `BrowserHandoff`,
`show_tab` (proposed extension 0.15.13), a human-check notification and automatic resumption. Its worker prompt says
not to solve puzzles. That working diff was inspected, not implemented or live-verified here. Integrate its handoff
as the fallback. The later owner approval adopts the automatic visible-puzzle ability here; implementation Task 5
replaces the human-only instruction and keeps the handoff as fallback. Source line references below refer to the
earlier checkpoint, not current implementation or live proof.

### What the current mechanism actually does

1. The worker decides to act. Its current instruction explicitly permits one checkbox click, then tells it to hand
   any further challenge to the owner: `ferb/orchestrator/prompts/worker.md:147`. The shared interaction instruction
   already demands evidence of a real block rather than merely a vendor badge:
   `ferb/orchestrator/prompts/interaction.md:27`. Thus **premature handoff is also a prompt rule**, not only a tool gap.
2. `BrowserAct` accepts click, hover, drag, typing and key presses. Its `press` means a keyboard key; neither its
   schema nor `BrowserAction` offers a pointer hold: `ferb/act/browser/tools.py:610`, `ferb/core/types.py:343`.
3. A screenshot point resolves into a normal task-owned element. The bridge requires fresh points, consumes the
   screenshot on a gesture, and sends `pointer` through the paired extension:
   `ferb/act/browser/tools.py:588`, `ferb/act/browser/ext_bridge.py:1193`, `:1638`, `:1665`.
4. The extension follows frame bindings, checks the actual point again, waits for a move to reach the intended
   frame, and dispatches tab-local mouse events. It supports click, double-click, right-click, hover and drag;
   drags currently require both ends in the same frame: `extensions/chromium/pointer.js:17`, `:53`, `:66`, `:82`.
   A drag uses a fixed straight interpolation today (`:113`); that proves an input primitive, not any vendor's
   acceptance of that gesture. Existing cleanup attempts a release in `finally` (`:130`); a failed release must not
   be described as a confirmed release.
5. Closed shadow roots are now traversed for point resolution, DOM binding and frame discovery:
   `extensions/chromium/content.js:61`, `:70`, `:229`, `:433`. The pre-press frame failure now returns a known failure
   with nothing pressed: `extensions/chromium/pointer.js:100`. Tests exercise these paths in an isolated Node DOM:
   `tests/test_closed_shadow_frames.py:1`. This is the 0.15.12 fix, not proof of a production challenge pass.
6. The transport journals command IDs before execution and distinguishes done, failed and unknown outcomes:
   `extensions/chromium/background.js:283`. The bridge never blindly resends a lost command and sends pointer
   cancellation on timeout/cancellation: `ferb/act/browser/ext_bridge.py:120`. These semantics must survive a hold.
7. Gateway tool spans and redacted image evidence provide the audit path:
   `ferb/daemon/gateway.py:8`, `:815`; screenshots are masked before delivery by the browser bridge
   (`ferb/act/browser/ext_bridge.py:1411`). `ferb trace` reads the daemon's `trace.get` result
   (`ferb/cli/admin.py:115`). The plan extends these records, rather than creating an independent CAPTCHA database.

### Live evidence inspected, separately from code and test definitions

A read-only SQLite connection to `~/.ferb/ferb.db` was used to inspect task trees, tool spans and owner verdicts.
Task `490777c8a2cf461b8e013f5c7ff44120`, created 2026-10-26 at 18:24:49 +05:30, is recorded as failed. Its tree has
156 spans, including **eight tool calls**, in this order:

| Time (+05:30) | Tool | Relevant fact from the stored record |
| --- | --- | --- |
| 18:24:59 | `browser.profiles` | Profile inventory read. |
| 18:25:04 | `browser.open` | Browser tab opened. |
| 18:25:09 | `browser.snapshot` | Four `AX target has no accessible DOM binding` messages. |
| 18:25:17 | `browser.screenshot` | Image evidence and screenshot handle returned. |
| 18:25:20 | `browser.point` | Target `s2e5`, role `generic`, empty label. |
| 18:25:24 | `browser.act`, click | `OutcomeUnknown: pointer: pointer_did_not_reach_target_frame; outcome unknown`. |
| 18:25:30 | `browser.wait` | A wait was attempted. |
| 18:25:33 | `browser.snapshot` | The same four binding errors remained. |

The failed click's span is `47ef354b73794546957b84e275889bba`. No owner-verdict row exists for this task tree.
The returned snapshot text did not contain the checkbox's words; the identification of this incident as the
Cloudflare checkbox comes from the existing incident documentation and regression, not an invented text quotation
from that snapshot. I did not inspect the stored screenshot in this research pass.

`docs/OWNER_ACCEPTANCE.md:1865` still labels the 0.15.12 checks pending and specifically separates delivery of the
click from Cloudflare's decision. I did not verify today's installed extension version or Web Store review status,
run tests, or reproduce a challenge. The older trees `9e788c88…` and `0a5fbe31…` were checked for status/counts and
verdicts, not exhaustively reviewed as evidence of successful verification. No claim here depends on their success.

The iframe tests in `tests/test_browser_lab_real.py:101` and pointer tests in `tests/test_pointer.py:1` demonstrate
what is covered by test code. The former uses a local lab page. Neither a test definition nor that fixture establishes
a live reCAPTCHA or hCaptcha pass. New acceptance must follow D17's documented owner-controlled scenarios.

## Corrected provider map

Primary documentation was retrieved on 2026-10-27. Vendor product names are context for the agent, not a code-level
permission list or a permanent selector map.

| System | What can actually appear | Implication for FERB |
| --- | --- | --- |
| Cloudflare Turnstile | Managed checkbox, non-interactive widget, or invisible check. A full-page Cloudflare challenge is a related but distinct flow. | The closed-shadow fix repairs targeting. Observe the next page state; the provider can still reject the browser. [Turnstile](https://developers.cloudflare.com/cloudflare-challenges/challenge-types/turnstile/) |
| reCAPTCHA v2 | Checkbox or invisible invocation, with a possible further challenge. | Frame input is necessary; fresh visual reasoning and verification are additional requirements. [Versions](https://developers.google.com/recaptcha/docs/versions) |
| hCaptcha | Widget-based verification; separate invisible configurations also exist. | Do not infer a puzzle from a badge. Handle the interaction actually rendered. The normal form/site validates the response. [Developer guide](https://docs.hcaptcha.com/) |
| reCAPTCHA v3 | A score used by the site's own decision logic. | There may be nothing to click. Inspect the site's actual response; FERB cannot assert or change the server's score. [Versions](https://developers.google.com/recaptcha/docs/versions) |
| AWS WAF Challenge | A browser script challenge, distinct from CAPTCHA. | Wait for browser execution and inspect the result. Status 202 alone is insufficient identification; AWS documents its own action header. [Action behavior](https://docs.aws.amazon.com/waf/latest/developerguide/waf-captcha-and-challenge-actions.html) |
| Akamai | Configurable challenges, including reCAPTCHA; not exclusively invisible scoring. | Remove “nothing to click” as a blanket classification. [Challenge action](https://techdocs.akamai.com/terraform/docs/bmgr-ds-challenge-action) |
| HUMAN / PerimeterX | Press, hold until the displayed bar fills, then release. Passing the gesture and passing detection are distinct. | Add a real held-pointer lifecycle with observation while held. A universal hard-coded hold duration is not justified. [HUMAN testing guide](https://docs.humansecurity.com/applications/human-challenge-integration-testing-process) |
| DataDome | Slider and an available audio alternative; evaluation includes technical and interaction signals. | A correctly placed slider still needs observed acceptance. No guarantee based on distance alone. [Slider](https://docs.datadome.co/docs/datadome-captcha) |
| AWS WAF CAPTCHA | Visual or audio challenge, distinct from silent Challenge. | Observe the actual UI and ensuing application result. [Comparison](https://docs.aws.amazon.com/waf/latest/developerguide/waf-managed-protections-comparison-table-token.html) |
| Arkose | Evolving interactive challenges. Its documentation records replacement of Pick-a-tile with MatchKey. | Follow current visible instructions; a fixed tile-only solver would age badly. [Challenge migration](https://developer.arkoselabs.com/docs/pick-a-tile-end-of-life-communication) |
| GeeTest | Multiple forms, including slide, icon, word, nine-grid and one-click modes. | Treat this as several interaction types, not one jigsaw algorithm. [Types](https://docs.geetest.com/gt4/apirefer/api/miniprogram/swan), [web integration](https://docs.geetest.com/gt4/apirefer/api/web) |

## Approved behavior

The labels BV1–BV8 below are **specification references**, not decision registry IDs. Existing decisions remain in
force; P21 records the dated approval of packaged abilities, with D32 and MP2 governing runtime and origin separation.

### BV8 — A built-in abilities library; browser verification is its first feature

Use **`ferb/abilities/`** for shipped abilities. Each ability has a versioned description, a reusable procedure,
typed inputs/results, its own implementation and tests. Browser verification owns its orchestration there; generic
browser mechanics such as frame targeting, screenshots and held input stay in `ferb/act/browser/` and the extension.
This keeps a future ability from having to copy browser or gateway code.

Approved layout (implementation and review progress live in the plan's ledger):

```text
ferb/abilities/
  __init__.py
  catalog.py                  # explicit built-in definitions; no arbitrary directory imports
  contracts.py                # definition, invocation/checkpoint and result types
  tools.py                    # catalog/read/run/status capability adapters
  runner.py                   # resumable recipe execution through the existing gateway
  browser_verification/
    __init__.py
    definition.py             # id, version, description, typed input and dependencies
    SKILL.md                  # shipped procedure, decision rules and verification criteria
    workflow.py               # this ability's state transitions and mechanical steps
tests/abilities/
  test_catalog.py
  test_runner.py
  test_browser_verification.py
```

This is a small built-in feature library, not a marketplace, dynamic-code installer or new model runtime. The owner
can add another reviewed ability package later, register it and supply its tests. No second placeholder ability is
needed to ship this one. Package resources must work in the wheel and FERB.app, not just the source checkout.

There is already **source plumbing** for owner-written skills: `ferb/orchestrator/skills.py:13` confines reads to
`~/.ferb/skills`, `:47` catalogs metadata, and `:66` exposes `skills.read`; `ferb/daemon/main.py:908` wires the catalog
to `ferb/orchestrator/supervisor.py:653`. P21 specifies that arrangement in `docs/0.27_PLAN.md:950`. I inspected this
code and its tests (`tests/test_owner_skills.py:1`), not live skill-use traces. Do not claim that it already executes
the proposed abilities. Keep the existing owner-skill source and its `owner` provenance distinct from packaged
`builtin` abilities; a local owner skill with the same display name must not replace executable built-in code.

Discovery and invocation:

1. At startup, explicitly register the packaged definitions. Add a compact available-abilities catalog to planner
   and worker context, using the existing capability-discovery path (`ferb/act/catalog.py:23`, `ferb/registry.py:33`).
   Browser verification is advertised from the first task; it does not wait for learning's verified-run threshold.
   Bodies are read only when relevant, through FERB's own loader, independent of the selected runtime's skill loader.
2. `abilities.list` / `abilities.read` expose the installed definition, procedure, version, supported interaction
   types and concrete missing dependencies. `abilities.run(name="browser_verification", tab=..., ...)` starts the
   packaged workflow for the already-authorized task. A worker encountering a check invokes it; the orchestrator
   can also select it as the method for that step. The name and description plainly identify browser verification.
3. `abilities.run` is resumable: a subsequent call carries its run ID and a typed decision grounded in the observation
   it requested. The runner executes deterministic mechanics, asks the existing worker for a semantic decision when
   needed, and returns control promptly. It does not keep a tool request open across model reasoning or owner help.
   `abilities.status` is observation-only. No extra specialist model or new paid service is assumed.
4. The worker chooses intent, target, puzzle interpretation, retry and escalation. The stored procedure provides
   the method, required observations and next-step schema. Code checks applicability facts (tab ownership, identity,
   freshness, supported input), executes the selected operation and advances the recipe. Thus selecting a pictured
   object may still require a model; pointer delivery, hold management and receipt handling do not.
5. Each underlying call goes through `Gateway.call` under the existing task's grant and stop signal. The runner
   receives a narrow dispatcher, never an unrestricted browser driver, filesystem or network client. It holds no
   outer tab/resource lock while calling the gateway; each inner action obtains its ordinary locks and confirmations.
   The acting run entry cannot be offered to a read-only verifier. A read/list/status call never starts execution.

Approved contract:

```text
AbilityDefinition: id, version, description, procedure_resource, input_schema,
                   supported_interactions, required_capabilities
AbilityInvocation: ability_id, version, task_id, worker_id, tab/profile binding,
                   declared_effect/destination, original_step, run_id,
                   observation_refs, decision (if resuming)
AbilityResult: run_id, status, reason, evidence_refs, next_decision_schema,
               checkpoint_ref, original_step
status: observing | needs_decision | acting | waiting_site | passed |
        needs_owner | unavailable | unknown | cancelled
```

`passed` means the observed verification step passed, not that the original purchase/booking/send completed.
`unavailable` includes a missing input capability or a model that declines required interpretation, with the actual
reason recorded separately from a website rejection. A new invocation cannot erase an earlier unknown inner action.
Parent and inner spans link the ability/version, original task and mechanical receipts; semantic judgments retain
their external evidence references. Existing confirmation, P19 and task ownership records remain authoritative.

The checkpoint stores the procedure version, stage, bound tab/profile, completed action references and pending
decision in a minimal `ability_runs` record owned by the existing task. The inspected `PlanStore` records plan steps;
a worker without a plan still needs an ability checkpoint, so do not fabricate a plan or overwrite step outputs.
Reuse the database, redactor, task lifecycle and retention infrastructure. Never persist live pressed state for replay. Resume refreshes
observations and reconciles prior receipts. An incompatible procedure version returns a migration/unavailable fact;
it does not replay an old action under new code. Stopping the parent also stops the ability and releases its input.

Reuse means **saved expertise, fresh perception**. Learned site-specific successes can supply evidence-backed hints,
but cannot overwrite packaged code or the trusted procedure from a website's text. Neither a three-success learning
gate nor a first-run training phase is required to use this shipped ability.

Packaged metadata uses `source_kind=builtin` and FERB's existing `ferb` trust origin; owner-skill bodies retain
`owner` origin. Do not introduce `builtin` as an unrecognized trust label: `ferb/core/origin.py:11` treats unknown
labels as outside. Observed web content and model judgments keep their respective external/inferred origins even
when included in an ability result; the wrapper must not relabel them as FERB authority.

The owner's concern about model refusals is addressed honestly: an LLM can delegate the operational work to this
explicit FERB capability instead of devising every action. Code can execute a grounded checkbox/hold sequence without
asking a model to synthesize its mechanics. A skill file does not override a model service's own behavior, and a
model can still decline to invoke the ability or interpret a novel puzzle. The feature must report that separately;
it must not disguise the task, claim a refusal was solved, or silently switch providers. Compatibility testing of
the configured runtime belongs in acceptance. No refusal-bypass promise is part of the feature contract.

### BV1 — Observe the obstacle before naming it

Use the existing snapshot and masked screenshot. Record the visible instruction, relevant frame/element or image
reference, current profile/tab and the expected next application state. A footer, logo, 403, failed login or empty
search result is not sufficient evidence of a CAPTCHA. When the page cannot be read, say so; do not fabricate a
provider diagnosis. Separate transport/input errors, an unmet challenge, a site error and an uncertain outcome.

The agent interprets the page. Code validates references, provenance and input safety. There is no keyword-based
“CAPTCHA mode” that changes authorization, and no vendor-name routing table that decides whether FERB may act.

### BV2 — Complete ordinary visible interactions in the existing scope

Replace the current “one checkbox, then hand off any further challenge” instruction with invoking the built-in
ability. Its packaged procedure owns this observation/action loop:

1. Establish that the check belongs to the task and its current browser identity.
2. Choose the visible control from a fresh observation. Click checkboxes normally. For a picture task, read the
   current instruction and image; choose the relevant targets. For a slider, ground the handle and destination in
   the same screenshot. For a hold, use BV3.
3. After an action, get new evidence before choosing further targets. Replaced image tiles, changed instructions,
   navigation, viewport changes or an expired widget invalidate earlier choices. Do not replay a stored answer.
4. Read the result and continue the original workflow when it actually progresses. A green widget alone does not
   prove a form submission, booking, payment or message succeeded.

This proposes that FERB **attempt visible puzzles itself**, rather than declaring every puzzle human-only. It does
not assert that the current model can solve every puzzle. Measure its results on the owner's real cases. Existing
background screenshots, click and drag should be used first; add a new perception primitive only for a demonstrated
visibility gap, with its own reviewed test and design. Do not quietly add an external solver service or subscription.

Audio is a separate capability check. Use an offered alternative only if the existing scoped, on-device audio path
can actually receive it without taking the owner's desktop/audio over. L26's microphone support alone does not prove
tab-audio capture. This proposal does not require a new audio capture subsystem to claim visual support.

### BV3 — A held pointer with feedback and reliable cleanup

Use a managed background hold, not a long key press or a page-script click. The reason to support observation while
held is concrete: HUMAN tells the visitor to release after the displayed bar fills. Merely sleeping for an invented
provider-specific duration would not implement that instruction.

Proposed tools in both existing families (`browser.*`, `ferb_browser.*`):

| Interface | Contract |
| --- | --- |
| `hold(action=start, tab, element, hold_for_ms, effect, why)` | Start on one fresh screenshot-resolved point, through the normal policy gateway. Return an opaque gesture ID, input phase, release state and remaining interval. `hold_for_ms` is an explicit positive, finite, representable requested interval; no hidden default or new product-wide maximum. |
| `hold(action=extend, tab, gesture, hold_for_ms, effect, why)` | Extend only that still-held gesture, owned by this task/worker and connection generation. A new finite interval starts at acceptance; an expired/released gesture cannot be revived. |
| `hold(action=release, tab, gesture, effect, why)` | Release only that gesture. Repeating release returns its terminal receipt without a second press. Release can activate a control, so it remains an acting operation. |
| `hold_status(tab, gesture)` | Observation only. Return held/released/cancelled/unknown, elapsed/remaining time and release facts. Reading never renews the interval. |
| Existing snapshot/screenshot | Allow masked, non-input observations during a hold; keep the held target's document/frame/geometry binding separately from the latest screenshot generation. |

The finite interval is the requested action duration and an expiry that releases input if the worker disappears;
it is not a new retry/time budget or a claim about a provider's solve time. The worker observes and releases on the
visible completion cue, or extends while fresh evidence shows the same interaction still needs holding. If this
cannot be established, release and report uncertainty. Do not hold while asking the owner a question.

Code invariants:

- Exactly one active pointer gesture per tab. Unrelated tabs may continue. Overlapping mutating input in the held
  tab is refused; release/cancel and non-mutating observations remain available. A navigation/close explicitly
  cancels and settles input before proceeding. No hidden replay or target change.
- Bind the handle to task, worker, profile, tab, document/frame identity, resolved point, connection generation
  and the original declared effect. Extensions and releases cannot weaken that declaration or bypass P19 by using
  a fresh handle. Every policy check still applies.
- Obtain the required authorization for the whole gesture before pressing. Extend/release use only that exact
  gesture's valid authorization; if changed scope requires another question, release first. Neither an ability
  wrapper nor a held button may keep input active while an owner confirmation waits.
- Revalidate before the press. During a hold, ignore expected progress-content changes but detect document/frame
  replacement, target disappearance, shifted geometry, secret/unavailable targets and loss of the owning session.
  Stop/steer/task cancellation, expiry, navigation, shutdown and bridge disconnect all trigger release attempts.
- An acknowledged press, an acknowledged release and website acceptance are separate facts. Record uncertainty
  when a call or release acknowledgement is lost. A cleanup attempt is not proof that input was released.
- A debugger detach/browser exit can prevent a release command from reaching the original target. Preserve the
  unknown receipt, discard the handle, and require fresh observation after reconnect; never reattach and re-press
  to “complete” the gesture. No active hold is restored after a daemon/extension/browser restart.
- The extension owns the expiry/release watchdog, so cleanup does not depend on another model call. Use elapsed
  monotonic time and measure background timing in acceptance; do not promise precise timer delivery under sleep,
  browser suspension or process death. Reconnection reconciles terminal/unknown receipts before further action.

Chrome exposes tab-targeted debugger commands and access to closed shadow roots; neither API promises that a
verification service will admit its input. Sources: [debugger](https://developer.chrome.com/docs/extensions/reference/api/debugger),
[DOM](https://developer.chrome.com/docs/extensions/reference/api/dom). Keep the current CDP input route; do not
replace it with OS mouse movement. The full hold lifecycle is one deliverable: do not ship a press without cleanup.

### BV4 — Verify, reconcile and make forward progress

Track these facts in existing tool/evidence records: observation reference, action/gesture ID, dispatch phase,
release acknowledgement, fresh after-observation and the agent's interpretation of the website result. Input
receipts may say “delivered”; only the observed application outcome can establish success.

When there is no interactive control, allow the page's ordinary browser script to run and inspect the next state.
The site may accept, show a puzzle, reject, or fail to load. Retry decisions belong to the agent and remain subject
to P19. Repeating the same failed interaction with no changed evidence is not progress. This proposal adds no retry
count, polling quota, retention period, site rate or per-task budget. A new numeric ceiling would require the owner.

Keep the same authorized profile and normal session. Do not treat deleting cookies, changing networks, rotating
identities, weakening certificate checks, disabling extensions or changing browser settings as an ordinary retry.
The existing rules for those actions continue to apply. No token extraction, token replay, success-callback
patching or claim that a test bypass proves production acceptance is part of this design.

### BV5 — A specific handoff, followed by verified resumption

If the current model cannot interpret a puzzle, the site continues to reject it, a necessary input route is
unavailable, or the next consequential action has an unknown outcome, release any held input first. Reuse the
human-check handoff being developed in the shared checkout if it lands: one app notification opens the exact tab
on the owner's click, and FERB observes and resumes automatically. Do not require a “done” reply or a CLI command.
Use the existing inline questions for actual choices or P19 approval, not as a substitute for observing completion.

Example: “This page still says ‘[actual visible message]’ after the check. Open the check in [profile/site]; I'll
continue when it passes.” The owner's click is the foreground action under rule 17. A task tab must be retained
through the handoff; background work must not silently close it or switch accounts.

A disappearing message, an answered notification or an unsolicited “done” is a reason to inspect again, not proof
that the challenge passed. Reacquire the same tab where possible, refresh all references, check identity and page
state, and continue the unsatisfied original step only on affirmative evidence of the expected page state.
If the tab/session disappeared, report that fact and rebuild only within the original task authorization. Preserve
unsent form state when the existing page still has it. Never replay a possibly completed submit/payment to test
whether the challenge is gone. Existing non-expiring questions and owner stop/cancel semantics remain.

### BV6 — Tests that distinguish different claims

| Evidence class | What it can prove | What it cannot prove |
| --- | --- | --- |
| Offline regressions using real incident shapes | Frame routing, stale references, lifecycle cleanup, redaction, provenance and receipt semantics. | Provider acceptance or model puzzle skill. |
| Vendor test integration on a documented owner-controlled workflow | Rendering, delivery, success/error handling and resumed form submission. | Production risk scoring or general image-puzzle solving. |
| Redacted replay of owner-observed challenge images | Model interpretation and tool-selection behavior on those images. | A live pass, server validation or timing-sensitive interaction. |
| Supervised live owner task | The actual observed case, profile, browser/extension version and end-to-end result. | Universal compatibility with the vendor. |

Test keys are specifically limited evidence:

- Cloudflare supplies success, failure and forced-interaction test modes; they are integration controls, not a
  production acceptance result. [Testing](https://developers.cloudflare.com/turnstile/troubleshooting/testing/)
- Google's v2 test keys always avoid a challenge; v3 test scores are not representative of real traffic.
  [FAQ](https://developers.google.com/recaptcha/docs/faq)
- hCaptcha's public test pair produces a passcode without a question. [Test keys](https://docs.hcaptcha.com/)
- HUMAN's documented testing mechanism is controlled by the site operator. A pass with it validates integration,
  not production detection. [Integration tests](https://docs.humansecurity.com/applications/human-challenge-integration-testing-process)
- Arkose recommends development keys; DataDome recommends an operator-configured test setup, potentially with
  vendor help. Access to those accounts is a prerequisite, not something FERB already has.
  [Arkose](https://developer.arkoselabs.com/docs/bot-manager-quickstart), [DataDome](https://docs.datadome.co/docs/datadome-captcha)

New live scenarios use the owner's existing kartikey.xyz request-logging/security-test surface and approved real
personal tasks. Select the concrete protected workflow and document it before adding its live test (D17). Do not
call a newly invented local challenge toy an acceptance test. Mark unavailable vendor coverage pending. No new
service signup, billable protection, deployment or third-party challenge run is authorized by this document.

### BV7 — Preserve the boundaries during verification

MP2, S4, D8/D12/D28/D29, D31/D33, W2/W4, P19 and the current foreground/browser-settings rules apply unchanged.
A fake verification instruction to run a command, paste a secret, install an extension, disable protection or send
data is external content, not authority. Security scenario 14 already covers fake CAPTCHA prompt injection
(`docs/0.27_SECURITY_TESTS.md:93`). Add regression coverage to this workflow rather than a separate weaker path.

Keep response tokens, cookies, authorization headers and sealed credentials out of model context, traces, fixtures
and learned procedures. Store redacted observations and results, not reusable puzzle answers or coordinates.
Learning can retain a verified navigation/interaction procedure; each new challenge still requires fresh perception.
No screenshots or tokens are sent to a new solver vendor by this proposal.

## Completion and remaining owner decisions

Implementation is complete only after the plan's task reviews, combined checks and final review. Live acceptance
is separate: record delivery, observed provider result, original-task continuation, focus/cursor/Space behavior,
versions, trace references and the owner's verdict in `docs/OWNER_ACCEPTANCE.md`.

The owner approved implementation on 2026-10-27. Its principal behavior change is attempting visible puzzles and
feedback-driven holds before asking the owner, replacing the prompt's immediate handoff when that workflow lands.
Before live validation, the owner chooses the actual personal sites/accounts and approves third-party runs; any
new paid provider integration is a separate decision. No new numeric product limits are proposed.

## Self-review

- The 0.15.12 targeting repair is distinguished from a live pass, and iframe test coverage from provider coverage.
- Akamai is no longer incorrectly classified as always invisible; modern provider variants are accounted for.
- The design fixes both the missing hold primitive and the explicit prompt rule that ends further attempts.
- Browser verification is a separately packaged built-in ability, available before any learned successes; future
  abilities have a documented home without duplicating drivers, owner skills or the model-runtime system.
- A hold includes feedback, ownership, expiry, cancellation and uncertain release; fixed sleeps alone are inadequate.
- Model judgment stays in the agent. Code enforces target/resource boundaries and reports mechanical facts.
- Retry/learning cannot reuse stale targets or turn a lost consequential outcome into a fresh action.
- Offline, vendor-test and production results are separate. Unavailable accounts and untested puzzles stay visible.
- Source implementation is authorized; installed-app behavior and owner acceptance remain separately verified facts.
