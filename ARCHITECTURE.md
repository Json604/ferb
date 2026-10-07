# FERB architecture — 0.26.1

The [owner guide](docs/OWNER_GUIDE.md) is the starting point for understanding the code. This document is the
implementation contract. [Capabilities](docs/CAPABILITIES.md) distinguishes measured behavior from offline tests;
[owner acceptance](docs/OWNER_ACCEPTANCE.md) contains the remaining live checks. Historical designs remain in Git.

## 1. Decisions and execution

Agents decide intent, authorization, target/account/source, methods, recovery and whether outcomes meet the request.
Code owns typed validation, transport, locks, resource ownership, redaction, recording and stop. Code returns facts;
it does not switch a local source to Drive, resolve a contact by first match or infer a send from an English label.

The harness consists of daemon RPC, gateway, stores, ownership, locks, sentinel, recording and stop. The approach
consists of prompts, planning, dispatch, success-condition generation and checker judgment. All prompts are versioned
files loaded through `ferb/core/prompts.py`. `Supervisor._approach` is the single selection seam for a later verified
override with default fallback. No learning loop or additional Jev decision families are implemented.

<!-- source: ferb/orchestrator/supervisor.py#Supervisor._approach -->
<!-- source: ferb/core/prompts.py#load_prompt -->

## 2. Main files and boundaries

| Responsibility | Source |
|---|---|
| CLI, streaming presentation, reconnect | [`ferb/cli/chat.py`](ferb/cli/chat.py), [`client.py`](ferb/cli/client.py) |
| Composition and daemon lifecycle | [`ferb/daemon/main.py`](ferb/daemon/main.py) |
| Authentication and RPC | [`server.py`](ferb/daemon/server.py), [`auth.py`](ferb/daemon/auth.py); [client contract](docs/RPC.md) |
| Planning, attempts, questions, recovery, final answer | [`supervisor.py`](ferb/orchestrator/supervisor.py), [`schemas.py`](ferb/orchestrator/schemas.py) |
| Context and durable continuation | [`core/context.py`](ferb/core/context.py), [`continuity.py`](ferb/orchestrator/continuity.py) |
| Runtime execution and liveness | [`worker.py`](ferb/orchestrator/worker.py), [`runtimes/`](ferb/runtimes/) |
| Tool gateway and ledger | [`gateway.py`](ferb/daemon/gateway.py), [`ledger.py`](ferb/orchestrator/ledger.py), [`registry.py`](ferb/registry.py) |
| Evidence/checking | [`verify.py`](ferb/orchestrator/verify.py), [`evidence.py`](ferb/orchestrator/evidence.py), [`store/spans.py`](ferb/store/spans.py) |
| Background observation | [`daemon/disturbance.py`](ferb/daemon/disturbance.py) |
| Browser | [`act/browser/`](ferb/act/browser/), [`native_host/`](ferb/native_host/), [`extension`](extensions/chromium/) |
| Native, processes, files, recordings | [`act/mac/`](ferb/act/mac/), [`act/shell/`](ferb/act/shell/), [`act/fs/files.py`](ferb/act/fs/files.py), [`act/recording.py`](ferb/act/recording.py) |
| Memory, sensing, reminders and schedules | [`memory/`](ferb/memory/), [`sense/`](ferb/sense/), [`clock/`](ferb/clock/), [`schedules.py`](ferb/orchestrator/schedules.py) |
| Optional typed enrichment | [`decide/`](ferb/decide/) |
| Installation/diagnostics and voice transport | [`device/`](ferb/device/), [`voice/`](ferb/voice/), [`macapp/`](macapp/) |
| Measurement: regression runner, security test harness | [`eval/regress.py`](ferb/eval/regress.py), [`eval/security/`](ferb/eval/security/) |

Public store methods own their queries. Keep edits within these responsibilities. Split a module only for a real
ownership boundary; do not add wrappers to preserve dead behavior.

## 3. Request and resource lifetime

`Supervisor.send` preserves the current message, including pasted text marked as external data. The planner receives
full request text, memory, retrievable chat/activity and installed environment facts. Its task specification has
outcome conditions, worker type and chosen identity. Intake memory writes produce receipts before the worker starts.
The planner-context snapshot attached to a task holds the non-protected blocks only; the capped chat history and
resumable attempts (protected since F1) are not in it, and workers fetch history with `chat.history` (revisited in F6).

`run_tree` records the full request and specification, starts the sentinel and worker, and retains artifacts and
uncertain actions across recovery. Workers get the installed tool registry; `actions.search` supports complete paged
discovery. `_dispatch` decides retry, question or finish. There is no fixed action or attempt count or task deadline.
A liveness watchdog observes the runtime; registered owner/process/sleep waits do not count as silence.

A worker’s completion report is a claim. Literal predicates are collected as mechanical observations; every condition then receives checker judgment
against the full request, including account identity, artifact quality and uncertain outcomes. The checker receives the full request, specification,
intake receipts, artifacts and actual evidence. It must judge the state where it exists, not require a redundant
worker action or paths repeated in prose. Uploaded/sent artifacts require source/content identity evidence.

Cleanup stops recordings before their target is released. Ordinary task-owned tabs close; borrowed/kept-open tabs
detach without focus restoration. Ordinary task processes stop; explicitly retained background processes survive.
Cleanup observations feed resource reconciliation. They cannot erase existing failed checks. Final reporting lists
every false/unknown condition verbatim before any established facts, and suppresses unchecked success prose.
The sentinel finishes after task cleanup. Disconnect does not stop work; explicit stop cancels it. Restart marks
active tasks interrupted and unresolved ledger effects unknown, never replays them as though they failed harmlessly.

## 4. Context, output and evidence

The token-budget assembler uses whole blocks, protects the current request and required task data, and marks omitted
optional blocks with a fetch reference. A protected overflow is explicit instead of clipping. The exact assembled
runtime context is recorded. Provider context/output capacity remains a provider constraint; a refusal is not a
truncated success. Brevity instructions apply to status reports, never requested artifacts.

As built in F1: chat history (`HISTORY_TOKENS` 12000) and resumable attempts (`ATTEMPTS_TOKENS` 2000) keep their newest
entries whole (`ferb/orchestrator/supervisor.py`): history is listed oldest first with an omission note, attempts
newest first with a note about the older ones. Blocks are ordered stable first: device facts (installed apps), then
history, whose chat tags count from the oldest (`c1`) so earlier lines never change as the conversation grows; the
owner's message is last and the budget report at the end (P20). The Codex catalog lockdown sets
`supports_search_tool` false so FERB's tools are directly visible; a worker or checker whose runtime shows no FERB
tool loaded is re-run once, traced as `runtime.tools_not_loaded` (`ferb/orchestrator/worker.py`).

`task.evidence` pages the full redacted observations, reports, intake and registered task-root context sources.
`chat.history` and `memory.read` retrieve complete source text and candidate pages. Browser/native snapshots and
file/shell reads expose page cursors or full-source routes. File writes do not impose a deliverable byte cap.
Incoming RPC frames retain an explicit 1 MiB bound: oversize is refused, never cut and executed. Page sizes and
provider transport bounds are documented in the audit’s limits ledger.

The trace forest includes the pre-task planner by exact request ID. `trace --json` also exports one structured
outcome with request, plan, actions/results, verification, disturbances, cleanup and final records. `ferb verdict`
(RPC `verdict.set`) appends the owner's pass/fail to the `owner_verdicts` table for a root task; the export shows the
latest one as `owner_verdict`, and null means no verdict, not acceptance. Compact CLI previews use `--full`/`--json`
for all fields. Redacted secrets and unstored screenshots are intentionally unrecoverable from tool text.

`ferb/eval/` holds the 0.27 measurement tools. `ferb regress` submits a TOML task set (default
`ferb/eval/sets/smoke.toml`) in one fresh conversation, declines owner questions, writes one row per task to
`~/.ferb/regression/` and never decides pass or fail. `ferb baseline` reads the database read-only into
`~/.ferb/baselines/<timestamp>.json`. `ferb/eval/security/` has canary tokens, a localhost request logger and
`scan_trace`, which flags canary egress and unexpected recipients in an exported trace.

## 5. Gateway, questions and stop

The owner-only Unix socket authenticates client roles. Worker run tokens are scoped, expiring and revocable.
`Gateway.call` validates, binds the task, locks the actual resource, records the action, invokes the chosen tool and
returns redacted facts. Canonical filesystem locks preserve the exact requested path. Ledger transitions distinguish
intended, executing, done, failed and unknown delivery. A delivered event is not proof of the requested outcome.
Each `tool.call` span records the action `kind` and its non-empty `target` fields, so exported traces can be scanned
offline. `Gateway.test_send_allowlist` is for tests only: when set, send-class actions to unlisted or unidentifiable
recipients are refused before the ledger; it is None (inert) in normal running. GUI typing into a messaging app is
kind `mac_input`, not send-class, so the allowlist does not cover it.

Ordinary `owner.ask` first goes to the orchestrator with context. If what/who/which remains ambiguous, the owner gets
candidates and their evidence. Before a hard-to-undo action, preparation and target confirmation are concrete; an
already-given confirmation carries across retries. Foreground permission follows the same task-wide principle.
Free text is preserved; duplicate option labels are not resolved to the first. OTP is a separate sealed path.

The question wait is 600 seconds and expires without consent. The worker/checker silence watchdog currently polls
at 5 seconds and detects 180 seconds without activity; no-tool provider calls have a separate 900-second liveness
bound. These are transport/liveness safeguards, not task-duration or artifact-length goals. Stop revokes tokens,
cancels active handles, stops owned inputs/processes and persists across restart until explicitly lifted.

## 6. Background interaction and observation

The model chooses AX actions/values, PID-directed events, non-activating scripting, URL schemes, extension/CDP or
shell/file operations according to evidence. Native input takes one explicit mechanism. Missing AX actions, target
ambiguity or an unsupported route return facts; no automatic ancestor press, synthetic Return, append-to-value,
global replay or focus restoration follows. Global foreground input requires the agent’s explicit tool choice and
owner direction. Arbitrary shell/AppleScript remain powerful; the shared prompt and sentinel cover indirect routes,
without a brittle denylist of command words.

`DisturbanceSentinel` samples frontmost app, focused key-window identity, cursor, focused-window full-screen state,
visible window order and public Space-transition notifications throughout every task. It records action boundaries,
250 ms periodic observations, overlapping actions, owner input, missing fields and measured sampling time. It neither
blocks actions nor asserts authorization or causal certainty. Short changes between samples may be missed. Public
Space notifications give transitions, not stable Space IDs. There is no private-API Space/window manipulation.

[Apple’s Space notification](https://developer.apple.com/documentation/appkit/nsworkspace/activespacedidchangenotification)
is observed through the workspace notification center; window ordering uses
[public Core Graphics observations](https://developer.apple.com/documentation/coregraphics/cgwindowlistcopywindowinfo(_:_:)).
Per-app support is evidence-based. A background request does not prove an app respects it; see capability caveats.

## 7. Browser and sealed identity

The model selects from profile candidates; display names, email hints and directory identities are returned together.
No last-used/default-owner-account resolver or first-tab account extractor chooses for it. FERB’s own profile remains
a stated approach preference for tasks that call for it, not permission to replace an explicitly requested source.

Extension **0.13.1 / protocol 9** uses native messaging, paired sessions and per-tab CDP. Task tabs are inactive in an existing FERB window or an explicitly selected window. If none is selected/owned,
open returns all window candidates without creating anything. New-window creation requires explicit foreground
permission: macOS can switch Spaces before minimized/unfocused creation finishes. Borrowed tabs retain ownership. Exact tab/frame/generation references avoid stale-target reuse.
Local uploads use `DOM.setFileInputFiles` on the identified input, including supported iframe/OOPIF routes. Canonical
path, size, modified time and SHA-256 receipts tie the chosen local bytes to the selected files. Unknown/unsupported
routes return structured facts; no Google Drive replacement is performed. Remote attachment identity still needs
model inspection and final observed content evidence.

Dialogs are explicit pending observations and explicit accept/dismiss choices. Email uses observed browser primitives,
not a fixed Gmail compose/send recipe. OTP inbox candidates have opaque task/account/origin-bound IDs and masked
context; no latest-message/year/sender-substring selection is treated as meaning. Raw secrets remain sealed.
Managed extension reload is version/protocol transport maintenance, deferred while task resources are active.

## 8. Memory, sensing, schedules and Jev

Owner-stated memory and observed proposals retain source provenance. Retrieval is an explicit search, not authority
to pick a file or recipient. Complete items are available through `memory.read`; proposal and narrative text are not
silently clipped. `query_terms` (`ferb/memory/items.py`) tokenizes a query and `Memory.recall` drops terms that match most memories; the FTS
index uses `unicode61 categories 'L* N* Co M*'` (migration 0014) so Devanagari words stay whole. Observed items
credit identical content to its matching item first, and otherwise accumulate evidence by kind/subject/key only while proposed; an active one matches identical content only. Daily
summaries retain source handles; raw activity retention requires a persisted summary before purge. Privacy exclusions
and redaction deliberately omit protected data and are separate from context budgeting.

Calendar arithmetic operates on model-supplied typed values. Missing local time or invalid timezone is reported;
there is no hidden 09:00 default or one-year horizon. Schedule recipients remain as the model recorded them; code
never replaces “me” with the first remembered email address. Recurrence, late runs and prior failures are facts for
the next planner. A failed run does not silently pause a schedule.

The Jev question registry, typed client and shadow/effect recording remain extension seams with tests. Oversized
state is explicitly unavailable rather than partial JSON. No task authorization or target-routing rule depends on Jev.

## 9. Model runtimes (F2, D32)

Every model call goes through one interface. `ModelRequest(system, parts, cacheable)` ([`core/types.py`](ferb/core/types.py))
carries FERB's instructions and the context as ordered parts; the first `cacheable` parts repeat across a role's calls and
adapters cache them (P20). `assemble_parts` ([`core/context.py`](ferb/core/context.py)) returns the parts and that count from
the `ContextBlock`s whose `cache=True` leads the list. The supervisor marks the device facts and the chat history for the
orchestrator, and the owner's original request and the task specification for task calls
([`supervisor.py`](ferb/orchestrator/supervisor.py)). Everything model-specific stays in the adapter.

| Runtime | Adapter | Billing | Cache |
|---|---|---|---|
| `codex` | [`codex_exec.py`](ferb/runtimes/codex_exec.py) | subscription | automatic prefix; no FERB cache key |
| `claude` | [`claude_sdk.py`](ferb/runtimes/claude_sdk.py) | subscription | automatic prefix |
| `anthropic` | [`anthropic_api.py`](ferb/runtimes/anthropic_api.py) | per token | FERB places cache markers |
| `openai` | [`openai_api.py`](ferb/runtimes/openai_api.py) | per token | explicit breakpoints on gpt-6 and gpt-5.6, else implicit; a per-role `prompt_cache_key` |
| `google` | [`google_api.py`](ferb/runtimes/google_api.py) | per token | implicit |

`CATALOG` ([`runtimes/catalog.py`](ferb/runtimes/catalog.py)) holds each runtime's default model, its `Capabilities`
(tools, cache, cache key, typed output, reasoning effort, image input, continuation, live messaging, billing) and whether
it is usable on this Mac; `ferb runtimes` prints it. A role picks its runtime in `config.toml`: `[runtimes]`
`orchestrator`, `worker`, `checker` (the checker falls back to the worker's) and `[models]` `<runtime>`; `ferb runtimes use ROLE RUNTIME
[--model M]` writes them and applies from the next call. Only usable runtimes are offered to the planner as worker types.

`ApiRuntime` ([`runtimes/api.py`](ferb/runtimes/api.py)) is FERB's own tool loop for the three APIs. Each tool call runs
through `GatewayBridge`, the same gateway path as the MCP shim, so the run token's scopes decide what is offered; a provider
class per API translates requests, responses, cache markers, typed output and usage. Behavior:
- A model that calls a tool it was not offered ends the run with a `violation` event; every name in a turn is checked before
  any call in it runs.
- Transport errors and `RETRY_STATUS` replies (408, 409, 429, 5xx, 529) are retried with backoff, honouring `Retry-After`;
  other errors surface as an `error` event with the key removed from the text.
- Tool calls run as their own tasks. `cancel` ends the run; the gateway cancelling a job (an owner stop) ends that tool call
  and the model is told, not the whole worker; cancellation of the consuming task propagates.
  The cancel flag is checked before each provider call, before each tool call and after each tool call.
- Each adapter accepts only a normal end of turn and raises `ProviderError` for any other stop state (cut off at the output
  limit, refusal, early stop), so a partial answer is never taken as final.
- OpenAI requests include `reasoning.encrypted_content` and are stateless (`store` false), so reasoning is replayed.
- No turn or token budget; stop and the stall watchdog end a run. Provider-hosted tools are never enabled.

Typed answers are checked in one place, `check_final` ([`core/structured.py`](ferb/core/structured.py)), whatever the runtime.
Every runtime reports `Usage` ([`runtimes/usage.py`](ferb/runtimes/usage.py)) in one meaning: input includes cached, plus cache
write, output includes reasoning, and the cost from the provider if it reported one, else from the dated
[`prices.py`](ferb/runtimes/prices.py) table (an unlisted model has no cost). Subscription runtimes report an
API-equivalent estimate labelled `billing: subscription`.

API keys live in the Keychain (service `ferb`, account `api/<provider>`, [`runtimes/keys.py`](ferb/runtimes/keys.py)), stored
by `ferb setup --key PROVIDER` and read at call time; the daemon adds each key it reads to its `Redactor`
([`sanitize/redact.py`](ferb/sanitize/redact.py)). The `Redactor` is reconfigured in place on a config reload, so the journal, spans and
gateway keep the one instance that knows exact secret values (API keys, OTPs); a Google API key pattern (`AIza...`) is
also redacted by shape. Codex runs with its own empty HOME (`~/.ferb/codex-user`) and
`skill_mcp_dependency_install` disabled, so the owner's personal skills never load.

Remaining: Codex sends its per-process session id as the cache key, so its calls rarely reach the previous prefix's cache
and FERB cannot change that. The remedy is an API runtime for the orchestrator (the owner's choice); the P20 re-check with
back-to-back turns is in [owner acceptance](docs/OWNER_ACCEPTANCE.md).

## 10. Documentation and diagrams

The [offline walkthrough](docs/walkthrough/index.html) is a local teaching aid, not an executable agent or live trace.
The [HLD Mermaid source](docs/diagrams/ferb-hld.mmd) maps responsibilities and the
[LLD source](docs/diagrams/ferb-lld.mmd) shows one task lifecycle. Obsolete 0.24 SVG exports were deleted; Git preserves
them. Current diagrams and walkthrough describe 0.26. Source-reference checks flag broken current-doc paths and
explicit qualified Python symbols. Historical changelog/audit evidence is intentionally exempt.

## 11. Change rule

Do not fix a failure by adding a keyword rule for that one failure. Expose platform facts, give the agent full evidence,
and test the general boundary. A filename match does not prove identity; a topic match does not prove teaching depth;
a delivered event does not prove completion; silence in an old trace does not prove zero desktop disturbance.
