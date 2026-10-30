# Fix order

Proposed 2026-10-30 from the cross-check of all notes; each step lands as its own change with its tests, then the
owner installs it (`ferb setup --update`) and checks it live before the next step.

0. **Open questions settled** (hidden folders writable; CLAUDE.md rewritten 2026-10-30; Apple voice for now). Run the two sandbox measurements (rename semantics; whether shell
   commands inherit FERB's Accessibility/Screen Recording grants).
1. **Storage migration runner** (checksums, numbering check, refuse a newer DB): today a new migration numbered ≤ 48
   is silently skipped, and every later step adds migrations.
2. **Daemon foundation**: composition function and typed ports, config cache, exception handler, one events object
   (redact once), signal/start fixes. Gives later rewrites explicit seams instead of 42 hooks.
3. **Removals**: Jev switched off, procedures/consolidation, echo-scrub forget, notification rules, `steer`, CLI chat
   together with the daemon push-to-talk stack, dead code. Shrinks the code before the rewrites.
4. **Runtimes**: one runtime table, provider-neutral schema basis, Codex fixes, Claude token file, timeouts. Per-role
   schemas in step 8 must work on every adapter.
5. **Hands**: structural facts, stable P19 identity, secret fields, one browser list, shell scope + snapshots, shell
   and file fixes, one extension protocol bump (tab groups, card fields, signing).
6. **Gateway**: one Need pipeline, fact-based confirmations, one payment one question, known destinations, redaction
   rebuild, file-tool protection, post-call hooks; then the disturbance sampler on those hooks.
7. **Test harness rewrite** (new fake runtime, no dispatcher), started together with step 8; old harness tests are
   deleted only once the new end-to-end runs exist.
8. **Orchestration**: one execution path, one status/finish/stop function, typed turn object, merged checker,
   5-attempt check-in, pause/resume, planner restart on a new message, 90 s planner limit, smaller ordered context.
9. **Memory**: Tier 1-3, local embeddings, how-notes, simple forget, perception fixes.
10. **Prompts**: rewrite to contracts; patch mapping table reviewed by the owner; owner-confirmed memory items.
11. **Voice**: speaker factory (macOS voice first), replies from structured events.
12. **Storage**: retention, `source_refs` removal, incremental vacuum, one-time cleanup (owner OK), upgrade path
    tested on copies of real older databases and configs.
13. **Live checks**: per-adapter live tests, the owner's P20 injection eval, the patch re-test list, the safety suite.
14. **Docs cleanup** last: extract `notes/test-tasks.md`, settle CLAUDE.md/AGENTS.md, delete the old docs, rewrite
    the README.
