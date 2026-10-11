# Approval attention, task reconciliation, rich replies and navigation policy

## Goal
Implement the owner’s six requests: open real questions with yellow trace glow; eliminate phantom task counts;
audit generated reports and enforced limitations; replace noisy routine navigation confirmations with checks on
actual outgoing private content/external commands; explain the limits of task visibility; render Markdown properly.

## Architecture
Native attention is event-driven and deduplicated, distinct from quiet progress/completion. Active task state
reconciles with daemon facts across every local turn and backend root. Render supported Markdown as native text,
links, lists, code and tables without executing HTML or fetching remote images. Preserve full report details.
Backend policy changes are grounded in actual refusal traces and explicit owner decisions, not generic allowlists.

## Spec
Owner supplied screenshots from 14:37 (local report approval), 14:40 (three phantom requests) and 14:58 (raw table).
Recent relevant roots: local analysis726ab98b8edb4181b3937dbdc9e172c6; open reportd6d52ab01af942fc937ca430e0740832;
tickets6eb4bffd1c3c4c6ca3f5ed348df1dc91. Read trace/artifact before claims. Audit current event claims against primary
public sources, with unverified stock/demand distinguished from facts. Local report audit uses actual local sources.

## Global Constraints
Existing task continuity, stop, exact account/resource identity, secret sealing and provenance remain. User explicitly
changes prior product preference: genuine approval/clarification opens with amber glow, routine replies stay quiet.
Task visibility is supervision, not prevention of an already-started transfer. Never claim encrypted shell traffic
is inspected. Owner decisions: retain destructive/payment/account-security floors; public research passes and network shell
approval covers an exact command rather than separate hosts. Preserve independent progress.
No new arbitrary caps. Preserve owner AGENTS and frontend dirty baseline. Install only after checks and idle guard;
prior update/run authorization persists. No live purchases, submissions, secret inputs or model tasks for tests.

## Review Focus
No ambiguous Stop or phantom in-flight counts; attention no repeat/focus stealing; no HTML/code execution in rendering;
actual private payload remains protected; no whole-context token collision used as evidence of exfiltration;
external commands cannot become trusted simply because the agent saw them. Product limitations are explicit.

## Tasks
1. Frontend implementer: failing regressions for attention, stale counts and Markdown; implement native behavior,
   focused suite/release build. Files in ferb-UI; report ui-report.md.
2. Policy implementer: inspect real sink/shell refusals, propose precise changes and owner rulings; add regressions
   reproducing local index.html/search false positives and real exfil counterexamples; implement without loosening
   unrelated boundaries. Report policy-report.md.
3. Independent report audit: inspect complete traces/artifacts, verify substantive claims and concrete restrictions,
   record sources and verdict separately from FERB’s own verified label. Report report-audit.md.
4. Controller: integrate, independent scoped review/fix wave, combined checks, update current docs/registry/owner
   acceptance, commit/push astra_resume citing S4/MP2/D3/D27/P27, then idle installation and read-only health check.

## Self-Review
Offline regressions prove contracts, not live quality or safety for arbitrary code. Keep model claims, trace evidence,
independent verification and owner acceptance distinct. Do not conflate HTTP request metadata with private content
exfiltration or a localhost navigation with sending to an external host.
