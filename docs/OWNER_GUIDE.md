# FERB 0.26: the owner’s guide

FERB is a CLI conversation backed by one local daemon. Models choose what to do. The daemon gives them tools,
keeps resources separate, records what happened and honors stop. A model’s confident answer is a claim; the trace
and the actual result are the evidence.

Start with [capabilities](CAPABILITIES.md) for what has actually been tested and [acceptance](OWNER_ACCEPTANCE.md)
for checks you can run. The [0.26 audit](AUDIT_0_26.md) records the defects, incident evidence and remaining limits.
This guide explains where to look when you want to understand or change the product.

## One task through ten files

Imagine asking: “Use this local PDF in the test form, and leave my desktop alone.”

1. **`ferb/cli/chat.py`** sends your complete message over local RPC. Closing chat disconnects the viewer; it does
   not stop the work. `ferb stop` stops all work, and `ferb resume-tasks` lifts that stop. `ferb pause` instead pauses
   sensing. They are different controls.
2. **`ferb/daemon/main.py`** connects the conversation to the supervisor and the installed tools. It also reconnects
   chat to questions and finished results. This is the composition point, not a place to interpret “this PDF”.
3. **`ferb/orchestrator/supervisor.py`** builds the planner’s context, obtains a plan, starts the worker and handles
   recovery. The full request travels alongside the plan’s shorter goal, including pasted material marked as data.
   Memory saved at intake gets a receipt: the worker need not save it again to prove it exists.
4. **`ferb/orchestrator/prompts/supervisor.md`** tells the planning model how to make the plan. The shared
   `interaction.md` tells every role to preserve source, target and background constraints. Prompts are versioned
   files, read through `ferb/core/prompts.py`; doctor hashes them. No hidden Python prose defines an alternative policy.
5. **`ferb/orchestrator/worker.py`** runs the tool-using model through a provider adapter. The worker sees your words,
   the task specification, receipts and available tools. A requested study guide is a deliverable, not a short chat
   report. The model must plan enough teaching content, examples and practice to meet the request.
6. **`ferb/daemon/gateway.py`** validates a tool call, checks the caller’s task and resource ownership, locks the
   resource, executes, redacts and records. It does not infer recipients from labels or give permission to switch
   from a local file to Drive. A failed mechanism returns a fact for the model to interpret.
7. **`ferb/act/browser/ext_bridge.py`** transports browser work through the paired extension. An upload identifies
   an actual file input and exact frame, then sets local files through CDP. The receipt records the canonical local
   source, size and hash, and the selected bytes are compared. An unsupported target is a refusal, not a source
   substitution. The model must still inspect the form’s attachment and eventual submitted result.
8. **`ferb/daemon/disturbance.py`** observes the desktop throughout execution and cleanup. It samples before and
   after actions as well as periodically. It records changes; it does not block the worker, approve foreground use
   or move you back to an earlier app.
9. **`ferb/orchestrator/verify.py`** handles literal observations such as whether a specified file exists. Every
   condition then goes to a checker with your full request, artifact data, intake receipts and recorded evidence.
   A filename is not file identity, and a list of topic names is not a useful course. Missing evidence is unknown.
10. **`ferb/store/spans.py`** preserves the redacted record for `ferb trace`. The supervisor releases task resources,
    considers cleanup outcomes, and formats the final answer. Every failed or unverified condition is printed
    before any outcome facts. Unchecked success prose cannot erase those conditions.

Most tasks use this same path. A browser task and a native-app task differ mainly at step 7. A coding session uses
`ferb/act/shell/manager.py`; files use `ferb/act/fs/files.py`; native apps use `ferb/act/mac/driver.py`.
The [interactive walkthrough](walkthrough/index.html) lets you step through examples without running anything.

## What agents decide, and what code decides

| Agent judgment | Code mechanics |
|---|---|
| What you meant; which file/account/contact/profile/source | Parse typed arguments; return all candidates or an explicit page |
| Whether evidence identifies the intended target | Report canonical path/URL, account, size, time, hash and observations |
| Which supported method to try after a refusal | Execute exactly that method; report platform refusal and delivery uncertainty |
| Whether an action is authorized and needs confirmation | Carry the owner’s words and answer through retries; transport the question |
| Whether a deliverable teaches enough or fulfills the task | Preserve full text, artifact references and verification results |
| Whether to retry, ask, change the plan or report a blocker | Stop, locking, resource ownership, redaction, recording and cleanup facts |

For an ambiguous **what/who/which**, FERB first consults your request and existing evidence, then asks with the
candidates and their differences if ambiguity remains. It does not ask you to choose a technical method. Before an
irreversible or hard-to-undo outcome it prepares the concrete action and obtains confirmation. A confirmation already
given for that prepared action carries through retries; a changed target or conflicting evidence needs a new decision.
These are model instructions, not a second phrase-matching authorization engine.

Source identity is more than a name. Two `assignment.pdf` files can differ. The useful evidence is source
(local/Drive/attachment), exact path or URL/ID, owner/account, modified time, bytes, and content where available.
Missing fields must stay missing. A hash verifies equal bytes; it cannot tell the model which file you intended.

## Context without losing your request

`ferb/core/context.py` assembles whole blocks within an explicit token budget. It protects the current request and
required task evidence. Optional blocks omitted for space get a visible marker with a retrieval route. If protected
content alone exceeds the assembly budget, the overflow is explicit; it is not cut to fit. Provider context windows
still exist, and an oversized provider request can fail visibly.

`task.evidence` reads recorded task observations, reports, intake receipts and context sources. `chat.history` reads
older messages. `memory.read` reads complete items and pages candidates. Long tool output is paged with a cursor;
file writes and artifact content are not shortened for a chat-sized display. A compact trace is a preview; `--full`
or `--json` is the way to inspect all recorded fields. Redaction still removes secrets intentionally.

## Debug a bad task

Run these from the checkout or installed CLI:

```sh
ferb timeline
ferb trace TASK --full
ferb trace TASK --json > /tmp/ferb-task.json
ferb doctor
```

Keep the exported file private: redaction protects secrets, but requests and ordinary document content may be present.
A short task prefix must identify one root. `last` explicitly chooses the latest task; an ambiguous prefix is refused.

Read the trace in this order:

- Compare the original `request` with `task_spec`. Did the plan preserve source, scope, depth and background needs?
- Read the exact recorded runtime context. Was a source omitted with a fetch reference, and did the model fetch it?
- Find the first wrong action. Compare its arguments, observations and local source receipt. Separate an API
  delivery acknowledgment from an observed change in the target.
- Inspect every condition and its evidence. Did the checker inspect the correct layer? A memory receipt proves an
  intake save; a worker need not call a nonexistent memory-writing tool. An artifact path need not also appear in prose.
- Read cleanup and disturbances. A successful edit can still fail the task if its useful result was closed or it
  took your focus without permission. Look at action span IDs near each change, not just the final summary.

`trace --json` contains an `outcome` object with the request, task specification, planner/recovery/question/resource-review decisions, worker runs and context sources, actions/results,
verification, intake, disturbances, cleanup and final outcome records. Older traces explicitly show missing full-request
or sentinel coverage. `owner_verdict` shows your latest `ferb verdict TASK pass|fail` for the task, or null when you
have not given one. Acceptance notes and `ferb watch` observations are separate. Verdicts are collected, but no
learning loop uses them yet.

## Background work and the sentinel

The requested order is AX actions/values on the target, app/PID-directed events, non-activating scripting, URL schemes,
extension/CDP, then shell/file routes as appropriate. The agent chooses a route based on evidence; code does not
silently turn a failed background action into global input. Shell can still run arbitrary commands: prompts carry the
background contract, and the sentinel exposes violations. A command keyword blocklist would be a brittle substitute.

Browser opening reuses an existing FERB window. On a cold start it returns window evidence; the agent chooses
an exact existing window for an inactive tab. Code never guesses the first/focused window. Creating a new window
requires foreground permission because requested minimization did not prevent an observed full-screen Brave jump.
A stopped task now carries a separate `stop_reason` beside its unresolved conditions. Runtime tool-name observations
are retained in `runtime.tools`; these distinguish recorded provider evidence from a worker's availability claim.

The sentinel records frontmost app, focused key-window identity, cursor location, focused-window full-screen state,
visible window order and a Space-transition epoch. It uses public APIs; it does not manipulate Spaces. Space change
notifications do not give a stable Space ID. Sampling is normally every 250 ms plus action boundaries; brief
transitions can be missed, and unavailable AX fields are recorded as unavailable. It does not collect window titles,
pixels or keys. Overlapping actions and recent owner input accompany a change, with **temporal correlation only**:
the daemon cannot honestly decide who caused every concurrent change. You and the checker judge that evidence.

No foreground restoration runs afterward. If you deliberately moved to another app, FERB must leave you there.
A zero-change test is evidence for that test on that Mac, not a guarantee for every app, OS version or future task.
Read the per-app entries in [capabilities](CAPABILITIES.md), especially other-Space, minimized and full-screen cases.

## If you want to change X, go to Y

| Change | Start here |
|---|---|
| Planning, deliverable quality, recovery or questions | `ferb/orchestrator/prompts/`, then `supervisor.py` |
| Context budget and omissions | `ferb/core/context.py`, supervisor context assembly |
| Completion truth and evidence access | `ferb/orchestrator/verify.py`, `evidence.py`, supervisor final formatting |
| Add a tool | Its existing `ferb/act/` family and `ferb/registry.py`; do not add a parallel gateway |
| Uploads or browser frame handling | `extensions/chromium/background.js`, `ferb/act/browser/ext_bridge.py` |
| Native background input | `ferb/act/mac/driver.py`, `accessibility.py`, `window_input.py` |
| Memory, activity and timers | `ferb/memory/`, `ferb/sense/`, `ferb/clock/` |
| New client | [daemon RPC contract](RPC.md); keep orchestration in the daemon |
| Desktop observation | `ferb/daemon/disturbance.py`; never add focus restoration here |
| Trace/export shape | `ferb/store/spans.py`, `ferb/cli/admin.py` |
| Installation or diagnostic checks | `ferb/device/`; doctor must remain observational |

The **harness** is transport, validation, ownership, locks, redaction, sentinel, recording and stop. The **approach**
is prompts, planning, dispatch and checker judgment. `Supervisor._approach` is the single approach-selection seam:
a later verified override can be tried there with a default fallback. Today it selects the existing default approach;
0.26 does not add an optimizer, evaluator or learning loop. Keep the harness independent of approach names.

`ferb/decide/` is intentional, even without a configured Jev key. To add a future typed decision, start with the
registry in `ferb/decide/questions.py`, the typed parsing/client path in `decider.py`, and a narrowly owned call site.
Keep recorded baseline/answer/effect evidence and tests. Present hooks enrich memory/activity; they do not choose
accounts or authorize actions. Oversized Jev state is refused visibly rather than sending cut JSON. No new decision
families are shipped in this pass.

## Small glossary

- **Task:** one requested outcome, with worker attempts and success conditions.
- **Span:** one timed recorded operation; its ID is an evidence handle.
- **Receipt:** recorded facts about an operation or artifact; stronger than an ungrounded summary.
- **Condition:** an outcome to establish, with true/false/unknown and evidence.
- **Ledger:** records consequential action delivery, including “unknown” after lost acknowledgment.
- **Profile:** a browser’s isolated account/cookie data. Display names are not unique identifiers.
- **AX / CDP:** macOS Accessibility / Chrome DevTools Protocol, two background control mechanisms.
- **Harness / approach:** reliable execution machinery / replaceable agent strategy.
- **Untrusted data:** page/document/tool content that is evidence, never new owner authorization.
