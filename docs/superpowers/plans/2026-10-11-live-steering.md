# Direct owner steering (D3, D32, MP2)

## Goal
Owner corrections reach active workers without cancelling their task, releasing resources, or starting replacement workers. Unrelated requests stay independent; Stop/retry retain explicit semantics.

## Architecture
Route a correction to the existing task tree, persist its redacted owner message and provenance, update current goals/check conditions, and deliver via AgentRuntime.continue_. Codex workers use the existing isolated app-server transport with turn/steer; Claude and API adapters accept same-run follow-ups. Runtime protocol acknowledgement means accepted, not completed. Pending updates remain available across worker startup and subsequent plan steps.

## Spec
- Exact task identity and session ownership are required. Model interprets relationship, never keyword routing.
- Same task/run and open tools survive correction; no stop_tree in active correction path.
- Tool containment, secret redaction, owner authority, checker observe-only scope and source boundaries remain enforced.
- Startup/completion races and concurrent messages cannot silently drop a correction. Delivery failures report accurately.
- Do not replace an in-flight consequential action or claim it was undone. Updates affect subsequent decisions.

## Global Constraints
No new arbitrary budgets, polling, caps or model changes. Existing astra_resume branch explicitly authorized. Preserve owner AGENTS.md. Basic focused regressions then combined checks and fresh review; no unattended live third-party/model actions. Installation uses existing authorization, and must not interrupt owner tasks.

## Review Focus
Concurrent RPC readers, completed-turn races, stale final reports, authority/pasted content, pending questions, parallel plan children, checker scope, containment and ephemeral credential cleanup.

## Tasks
1. Runtime transport: failing adapter tests first; extend codex_stream/codex_exec to steer active worker turns with MCP lockdown; support same-run updates in other adapters. Focused runtime checks, report beside ledger.
2. Orchestration: failing active correction tests first; change supervisor routing/worker delivery and durable context, prompt distinction. Verify no cancellation/replacement, scope isolation and completion races. Update affected docs/registry. Commit citing D3/D32/MP2 after combined tests and independent review.

## Self-Review
Existing exec workers cannot accept new stdin messages mid-turn; reuse app-server rather than disguise restart as injection. Runtime support alone is insufficient: task acceptance criteria and later plan steps need the correction too. Open approvals and newly requested resources must not gain permissions from model-written context.
