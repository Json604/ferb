# Fix order

Proposed 2026-10-30 from the cross-check of all notes; each step lands as its own change with its tests, then the
owner installs it (`ferb setup --update`) and checks it live before the next step.

0. **Open questions settled** (hidden folders writable; CLAUDE.md rewritten 2026-10-30; Apple voice for now). Both sandbox measurements
   done 2026-10-30 (notes/hands.md, Measurements). **Step 0 complete.**
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
8d. **Answers in steps + notch UI** (orchestration decisions 19-20): parts in the planner schema and validation,
    once-only part posting after the checker, no final summarizing worker, the `part` event and its durable record;
    the notch turn card, row states, footer, in-row questions, typing-dots bubble.
    Live preview fixes (notes/preview.md): no queuing on worker locks, status over the frozen last frame, hidden
    from screen sharing, app-only access, frame counters for the step-13 heat measurement.
8e. **Typed cards in the notch** (owner, 2026-10-31; designs from ~/Desktop/shapeshift, MIT, rebuilt in SwiftUI and
    credited in THIRD_PARTY_NOTICES.md): the daemon attaches a typed card payload from its own structured data (never
    from classifying text) and the notch renders it: reminders firing (reminder card + countdown to the deadline) and
    new schedules/reminders (event card, time, "Repeats" badge, replacing the "🗓 Scheduled" line); remembered facts
    (note card, tap to correct/forget); payments and sends (expense row: amount, merchant, card ••last4; contact card:
    recipient, channel); results with links/files (link cards and file chips replacing "Sites contacted"/artifact
    lines). Text stays in the payload for the history, voice and older apps.
    (Step 8e: `ferb/orchestrator/cards.py`; `cards` on replies, plan events, outcomes and scheduled runs' results
    (stored in `task_result_cards`, migration 0051); notch `ResultCard`/`CardLayout` and ResultCards.swift;
    Shapeshift credited. Also the 8d protocol gaps: `chat.status` questions carry `source_task_id`, `part`,
    `runtime_switch`; error codes reach the app (FERB.app-only refusal -32005); part events carry `done`/`total`.
    Rulings in notes/orchestration.md. Live checks with step 13.)
9. **Memory**: Tier 1-3, local embeddings, how-notes, simple forget, perception fixes.
   (Step 9a: Tier 1 core profile, no Keep questions, decisions 11-16. Step 9b: the local embedding model
   (potion-base-8M, fetched once) and one fused search over facts, past tasks, playbooks and old chat within per-role
   limits; chat.history by meaning and date; how-notes, playbooks and the repeat measure; bounded card sources;
   perception fixes; decisions 17-18; the bot-check fact. Rulings and measurements in notes/memory.md.)
9c. **Memory import from other AI assistants** (notes/memory-import.md decisions 1-6): ChatGPT, claude.ai, Gemini
    exports and Claude Code / Codex CLI transcripts, last 3 months, read in memory only; recall → verification → merge,
    saved as FERB's understanding; a background job (estimate, progress, resume, cancel) with RPCs `memory.import.*` and
    a Memory page section; migration 0055. (Built 2026-10-31; live checks with step 13.)
10. **Prompts**: rewrite to contracts; patch mapping table reviewed by the owner; owner-confirmed memory items.
    (Step 10a: notes/prompt-patches.md, approved 2026-10-31. Step 10b: every runtime prompt rewritten to its contract,
    the patches placed (rules, tool descriptions, code, memory), role blocks only when a run needs them, no tool-catalog
    copy, the effect contract once, the approved memory items seeded once in the owner's store; sizes measured in
    notes/prompts.md "Expected size"; the live re-test list in notes/tests.md, run in step 13.)
11. **Voice**: speaker factory (macOS voice first), replies from structured events.
    (Step 11: `[voice] tts/voice/rate` and the provider registry in ferb/voice/speak.py, one redacting speech path;
    `chat.send speak: true` answers a turn aloud from its reply/plan/answer events, interrupted by any new message or a
    stop; the app's "Send dictation when I let go of the keys" and spoken voice turns; hearing's time limit, alignment
    off the loop, FERB Voice spared mid-transcription, record-with-audio exclusion check, strings to the app. Rulings
    in notes/voice.md; live checks with step 13.)
12. **Storage**: retention, `source_refs` removal, incremental vacuum, one-time cleanup (owner OK), upgrade path
    tested on copies of real older databases and configs.
    (Step 12a: retention in one place (ferb/store/retention.py: reads off the loop, 200-row batches, cascades,
    operational records 90 d), migration 0056 (reply ref lists stripped under a remade append-only trigger, one tombstone
    table; no owner data deleted), incremental vacuum + WAL truncate after each pass, `busy_timeout`, the one-time
    clean-up as `ferb setup --cleanup [--ok DIGEST]` (preview, owner's OK, daemon stopped, journaled, compacted; it also
    drops the four tables no code reads), the
    continued-tree/non-ASCII/exposure-origin/timer-kind fixes, upgrade tests from older schemas and configs. Measured on
    a copy of the owner's store: notes/storage.md. The app's clean-up screen is step 12b.)
    (Step 12b: the daemon never imports from a checkout (`-P`, FERB's home as its working folder); FERB.app's service
    definition inside the app, registered with SMAppService by the app, the old user LaunchAgent retired, the
    definition protected (FERB_SERVICE); a checkout keeps a dev LaunchAgent; setup fixes (permissions recorded only
    when all granted, new-venv swap, lockfile installs, Codex only when used, doctor, Keychain purge, plain errors);
    every permission needed; Jev off after an upgrade checked; the user CLI commands removed (`--claude-logout` kept for
    the app's new Sign out of Claude); the embedding model shipped in FERB.app; app-word runtime errors; more raw SQL
    into the stores; the footprint read. Rulings, footprint and live checks in notes/daemon.md. Not done: the app's
    clean-up screen (notes/storage.md decision 4).)
    (Step 12c: the app screens and their daemon side: "About FERB" built from code and config (an always-on facts line,
    `about.read` for workers, Settings → About; what FERB keeps from the new retention rule table; the app's how-tos and
    shortcuts kept with the UI and sent with `about.guide`); FERB's own account as an optional setup step and Settings →
    Browsers section (`identity.find` / `identity.choose` through the settings path, then `identity.pair`; skipped, the
    owner picks the profile FERB browses in); Settings → Data with Restore, saved secrets and cards (Delete through the
    signed helper) and the clean-up screen; "Uninstall FERB and delete all its data" (`ferb uninstall --purge --yes`);
    the owner-data RPCs FERB.app-only; memory import's page card and `.jsonl.zst` Codex rollouts (localized Takeout
    left: no structural marker). Rulings, UI pieces for the owner to polish and live checks in notes/daemon.md.)
    (Step 12d: the suite cut to the target design's shape: 3 new chat-level safety runs (tests/test_safety_runs.py),
    an import check, a redaction fuzz that found and fixed a truncated-private-key leak; 109 test files deleted (listed
    in notes/tests.md), the safety files and past-bug guards kept; 300 → 193 files, 92.9k → 67.6k lines, 5,545 → 4,246
    offline items, 20 min → 10 min (both measured with other work on this Mac). Per-test trimming inside the kept files
    is left (notes/tests.md). The egress-proxy flake fixed at its cause (listen backlog 5 → SOMAXCONN; notes/hands.md).
    The daemon footprint: not reproduced idle without watchers; Python objects ~32 KB a task; the native holder found
    — Cocoa objects on threads that never drained an autorelease pool, 210 KB per image recognized — and fixed with one
    pool per unit of work; on-device recognition was failing on macOS 27.2 (every image withheld), fixed. Gateway
    seams finished (`GatewayDeps.standing`, `Gateway.attach`); dead eval code and functions removed. Notes: daemon.md
    "Footprint (step 12d)" and its rulings, gateway.md, hands.md, tests.md.)
13. **Live checks**: per-adapter live tests, the owner's P20 injection eval, the patch re-test list, the safety suite.
14. **Docs cleanup** last: extract `notes/test-tasks.md`, settle CLAUDE.md/AGENTS.md, delete the old docs, rewrite
    the README.
